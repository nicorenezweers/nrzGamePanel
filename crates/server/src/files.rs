//! Datei-Explorer: auflisten, lesen, schreiben, hoch-/herunterladen, umbenennen, löschen, (ent)packen.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path as AxPath, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use nrz_gp_common::{DirListing, FileContent, FileEntry, Ok as OkResp, PathRequest, PathsRequest, RenameRequest, WriteFile};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    state::AppState,
    util::{rel_path, safe_join},
};

const MAX_EDIT: u64 = 5 * 1024 * 1024;

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
}

async fn base(app: &AppState, id: &str) -> AppResult<PathBuf> {
    let (cfg, _) = app.server(id).await?;
    Ok(PathBuf::from(cfg.dir))
}

fn ok() -> Json<OkResp> {
    Json(OkResp { ok: true })
}

fn mtime(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

pub async fn list(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Query(q): Query<PathQuery>) -> AppResult<Json<DirListing>> {
    let base = base(&app, &id).await?;
    let dir = safe_join(&base, &q.path)?;
    let listing = tokio::task::spawn_blocking(move || -> AppResult<DirListing> {
        let mut entries = Vec::new();
        for e in std::fs::read_dir(&dir)?.flatten() {
            let Ok(meta) = std::fs::metadata(e.path()) else { continue };
            entries.push(FileEntry {
                name: e.file_name().to_string_lossy().into(),
                path: rel_path(&base, &e.path()),
                is_dir: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.len() },
                modified: mtime(&meta),
            });
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(DirListing { path: rel_path(&base, &dir), entries })
    })
    .await??;
    Ok(Json(listing))
}

pub async fn read(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Query(q): Query<PathQuery>) -> AppResult<Json<FileContent>> {
    let base = base(&app, &id).await?;
    let path = safe_join(&base, &q.path)?;
    let meta = tokio::fs::metadata(&path).await?;
    if meta.is_dir() {
        return Err(AppError::bad("Ist ein Ordner"));
    }
    let size = meta.len();
    if size > MAX_EDIT {
        return Ok(Json(FileContent { path: q.path, content: String::new(), binary: true, size }));
    }
    let bytes = tokio::fs::read(&path).await?;
    let binary = bytes.iter().take(8000).any(|b| *b == 0) || std::str::from_utf8(&bytes).is_err();
    let content = if binary { String::new() } else { String::from_utf8_lossy(&bytes).into() };
    Ok(Json(FileContent { path: q.path, content, binary, size }))
}

pub async fn write(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(w): Json<WriteFile>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    let path = safe_join(&base, &w.path)?;
    if let Some(p) = path.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    tokio::fs::write(&path, w.content).await?;
    Ok(ok())
}

pub async fn mkdir(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(p): Json<PathRequest>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    tokio::fs::create_dir_all(safe_join(&base, &p.path)?).await?;
    Ok(ok())
}

pub async fn touch(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(p): Json<PathRequest>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    let path = safe_join(&base, &p.path)?;
    if path.exists() {
        return Err(AppError::conflict("Datei existiert bereits"));
    }
    tokio::fs::write(path, b"").await?;
    Ok(ok())
}

pub async fn rename(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(r): Json<RenameRequest>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    let from = safe_join(&base, &r.from)?;
    let to = safe_join(&base, &r.to)?;
    if from == base || to == base {
        return Err(AppError::bad("Ungültiger Pfad"));
    }
    if to.exists() {
        return Err(AppError::conflict("Ziel existiert bereits"));
    }
    if let Some(p) = to.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    tokio::fs::rename(from, to).await?;
    Ok(ok())
}

pub async fn delete(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(r): Json<PathsRequest>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    for p in &r.paths {
        let path = safe_join(&base, p)?;
        if path == base {
            return Err(AppError::bad("Der Server-Ordner selbst kann nicht gelöscht werden"));
        }
        let meta = tokio::fs::symlink_metadata(&path).await?;
        if meta.is_dir() {
            tokio::fs::remove_dir_all(&path).await?;
        } else {
            tokio::fs::remove_file(&path).await?;
        }
    }
    Ok(ok())
}

pub async fn upload(
    State(app): State<AppState>,
    _a: Authed,
    AxPath(id): AxPath<String>,
    Query(q): Query<PathQuery>,
    mut mp: Multipart,
) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    let dir = safe_join(&base, &q.path)?;
    tokio::fs::create_dir_all(&dir).await?;
    while let Some(mut field) = mp.next_field().await.map_err(|e| AppError::bad(e.to_string()))? {
        let Some(name) = field.file_name().map(str::to_string) else { continue };
        // Ordner-Uploads liefern relative Pfade (`a/b.txt`)
        let target = safe_join(&dir, &name)?;
        if let Some(p) = target.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        let mut f = tokio::fs::File::create(&target).await?;
        while let Some(chunk) = field.chunk().await.map_err(|e| AppError::bad(e.to_string()))? {
            f.write_all(&chunk).await?;
        }
        f.flush().await?;
    }
    Ok(ok())
}

/// Datei herunterladen; Ordner werden on-the-fly als ZIP gepackt.
pub async fn download(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Query(q): Query<PathQuery>) -> AppResult<Response> {
    let base = base(&app, &id).await?;
    let path = safe_join(&base, &q.path)?;
    let name = path.file_name().map_or("download".into(), |n| n.to_string_lossy().to_string());
    if path.is_dir() {
        let src = path.clone();
        let bytes = tokio::task::spawn_blocking(move || zip_paths(&src, &[src.clone()])).await??;
        return Ok(attachment(Body::from(bytes), &format!("{name}.zip"), "application/zip"));
    }
    let f = tokio::fs::File::open(&path).await?;
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    Ok(attachment(Body::from_stream(ReaderStream::new(f)), &name, mime.as_ref()))
}

pub fn attachment(body: Body, name: &str, mime: &str) -> Response {
    let safe: String = name.chars().filter(|c| !c.is_control() && *c != '"').collect();
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    r
}

/// Dateien/Ordner in ein ZIP (im Speicher) packen; Pfade relativ zu `root`.
pub fn zip_paths(root: &Path, paths: &[PathBuf]) -> AppResult<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    write_zip(&mut buf, root, paths, &[])?;
    Ok(buf.into_inner())
}

/// ZIP schreiben. `skip`: Ordner-/Dateinamen, die ausgelassen werden (z. B. `session.lock`).
pub fn write_zip<W: Write + std::io::Seek>(w: W, root: &Path, paths: &[PathBuf], skip: &[&str]) -> AppResult<()> {
    let mut zip = zip::ZipWriter::new(w);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    for p in paths {
        for e in walkdir::WalkDir::new(p).into_iter().flatten() {
            let rel = rel_path(root, e.path());
            if rel.is_empty() || skip.iter().any(|s| e.file_name().to_string_lossy() == *s) {
                continue;
            }
            if e.file_type().is_dir() {
                zip.add_directory(format!("{rel}/"), opts).map_err(|x| AppError::internal(x.to_string()))?;
            } else if e.file_type().is_file() {
                zip.start_file(rel, opts).map_err(|x| AppError::internal(x.to_string()))?;
                // Dateien, die gerade geschrieben werden (z. B. Region-Dateien), tolerieren
                if let Ok(mut f) = std::fs::File::open(e.path()) {
                    std::io::copy(&mut f, &mut zip)?;
                }
            }
        }
    }
    zip.finish().map_err(|x| AppError::internal(x.to_string()))?;
    Ok(())
}

/// ZIP/TAR(.gz/.xz) entpacken nach `dest` (mit Schutz vor Pfaden ausserhalb).
pub fn extract_archive(archive: &Path, dest: &Path) -> AppResult<()> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let file = std::fs::File::open(archive)?;
    if name.ends_with(".zip") || name.ends_with(".jar") || name.ends_with(".mrpack") {
        let mut z = zip::ZipArchive::new(file).map_err(|e| AppError::bad(format!("Kein gültiges ZIP: {e}")))?;
        for i in 0..z.len() {
            let mut entry = z.by_index(i).map_err(|e| AppError::internal(e.to_string()))?;
            let Some(rel) = entry.enclosed_name() else { continue };
            let out = dest.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out)?;
            } else {
                if let Some(p) = out.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut f = std::fs::File::create(&out)?;
                std::io::copy(&mut entry, &mut f)?;
                #[cfg(unix)]
                if let Some(mode) = entry.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
                }
            }
        }
        return Ok(());
    }
    let reader: Box<dyn Read> = if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Box::new(flate2::read::GzDecoder::new(file))
    } else if name.ends_with(".tar.xz") {
        let mut out = Vec::new();
        lzma_rs::xz_decompress(&mut std::io::BufReader::new(file), &mut out).map_err(|e| AppError::bad(format!("XZ-Fehler: {e}")))?;
        Box::new(std::io::Cursor::new(out))
    } else if name.ends_with(".tar") {
        Box::new(file)
    } else {
        return Err(AppError::bad("Unbekanntes Archivformat (zip, tar, tar.gz, tar.xz)"));
    };
    let mut ar = tar::Archive::new(reader);
    ar.set_preserve_permissions(true);
    for entry in ar.entries()? {
        let mut entry = entry?;
        entry.unpack_in(dest)?;
    }
    Ok(())
}

pub async fn extract(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(p): Json<PathRequest>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    let path = safe_join(&base, &p.path)?;
    let dest = path.parent().map(Path::to_path_buf).unwrap_or(base);
    tokio::task::spawn_blocking(move || extract_archive(&path, &dest)).await??;
    Ok(ok())
}

pub async fn archive(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(r): Json<PathsRequest>) -> AppResult<Json<OkResp>> {
    let base = base(&app, &id).await?;
    let paths = r.paths.iter().map(|p| safe_join(&base, p)).collect::<AppResult<Vec<_>>>()?;
    let Some(first) = paths.first() else { return Err(AppError::bad("Nichts ausgewählt")) };
    let parent = first.parent().map(Path::to_path_buf).unwrap_or_else(|| base.clone());
    let target = parent.join(format!("archiv-{}.zip", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")));
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let f = std::fs::File::create(&target)?;
        write_zip(f, &parent, &paths, &[])
    })
    .await??;
    Ok(ok())
}
