//! Archive entpacken (oberstes Verzeichnis abstreifen) und externe Programme mit Ausgabe in
//! die Server-Konsole ausführen.

use std::{
    fs,
    io::{BufReader, BufWriter, Read},
    path::{Component, Path, PathBuf},
    process::Stdio,
};

use nrz_gp_common::ConsoleSource;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead},
    process::Command,
};

use crate::{
    error::{AppError, AppResult},
    process::ServerRuntime,
};

/// Ersten Pfadteil entfernen und auf sichere Komponenten prüfen (`factorio/bin/x64` → `bin/x64`).
fn strip_first(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components().skip(1) {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() { None } else { Some(out) }
}

/// ZIP oder TAR(.gz/.xz) nach `dest` entpacken und dabei das oberste Verzeichnis abstreifen
/// (wie `tar --strip-components=1`). `.tar.xz` wird gestreamt über eine temporäre Datei entpackt.
pub fn extract_strip(archive: &Path, dest: &Path) -> AppResult<()> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    fs::create_dir_all(dest)?;
    if name.ends_with(".zip") {
        let f = fs::File::open(archive)?;
        let mut z = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| AppError::bad(format!("Kein gültiges ZIP: {e}")))?;
        for i in 0..z.len() {
            let mut entry = z.by_index(i).map_err(|e| AppError::internal(e.to_string()))?;
            let Some(rel) = entry.enclosed_name().and_then(|p| strip_first(&p)) else { continue };
            let out = dest.join(rel);
            if entry.is_dir() {
                fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            let mut f = fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
            }
        }
        return Ok(());
    }
    if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        let tmp = archive.with_extension("tmp-tar");
        {
            let mut input = BufReader::new(fs::File::open(archive)?);
            let mut out = BufWriter::new(fs::File::create(&tmp)?);
            lzma_rs::xz_decompress(&mut input, &mut out).map_err(|e| AppError::bad(format!("XZ-Fehler: {e}")))?;
        }
        let r = fs::File::open(&tmp).map_err(AppError::from).and_then(|f| untar_strip(f, dest));
        let _ = fs::remove_file(&tmp);
        return r;
    }
    let file = fs::File::open(archive)?;
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        untar_strip(flate2::read::GzDecoder::new(BufReader::new(file)), dest)
    } else if name.ends_with(".tar") {
        untar_strip(file, dest)
    } else {
        Err(AppError::bad("Unbekanntes Archivformat (zip, tar, tar.gz, tar.xz)"))
    }
}

fn untar_strip<R: Read>(r: R, dest: &Path) -> AppResult<()> {
    let mut ar = tar::Archive::new(r);
    ar.set_preserve_permissions(true);
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(rel) = strip_first(&path) else { continue };
        let out = dest.join(&rel);
        if let Some(p) = out.parent() {
            fs::create_dir_all(p)?;
        }
        let kind = entry.header().entry_type();
        if kind.is_hard_link() {
            // Harte Links zeigen auf Pfade im Archiv (inkl. oberstem Ordner) → als Kopie anlegen.
            if let Some(src) = entry.link_name()?.and_then(|l| strip_first(&l)) {
                let _ = fs::copy(dest.join(src), &out);
            }
            continue;
        }
        if kind.is_symlink() {
            if let Some(link) = entry.link_name()? {
                // Nur relative Links (Quellen sind Temurin/Factorio)
                if link.is_absolute() {
                    continue;
                }
                #[cfg(unix)]
                {
                    let _ = fs::remove_file(&out);
                    let _ = std::os::unix::fs::symlink(link.as_ref(), &out);
                }
            }
            continue;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}

async fn pump<R: AsyncRead + Unpin>(rt: &ServerRuntime, r: Option<R>, source: ConsoleSource) {
    let Some(r) = r else { return };
    let mut reader = tokio::io::BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf).trim_end_matches(['\n', '\r']).replace('\r', "");
                if !text.trim().is_empty() {
                    rt.push(source, text);
                }
            }
        }
    }
}

/// Programm ausführen, Ausgabe live in die Konsole; Fehler bei Exit-Code ≠ 0.
pub async fn run_logged(rt: &ServerRuntime, mut cmd: Command) -> AppResult<()> {
    let program = cmd.as_std().get_program().to_string_lossy().to_string();
    let args: Vec<String> = cmd.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect();
    rt.panel(format!("$ {} {}", program, args.join(" ")));
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| AppError::internal(format!("{program} konnte nicht gestartet werden: {e}")))?;
    let out = child.stdout.take();
    let err = child.stderr.take();
    futures::join!(pump(rt, out, ConsoleSource::Stdout), pump(rt, err, ConsoleSource::Stderr));
    let status = child.wait().await?;
    if !status.success() {
        return Err(AppError::internal(format!(
            "{} wurde mit Code {} beendet",
            Path::new(&program).file_name().map_or(program.clone(), |n| n.to_string_lossy().to_string()),
            status.code().map_or("?".into(), |c| c.to_string())
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::import::detect::tests::TempDir;

    #[test]
    fn strip_tar_gz_and_zip() {
        let t = TempDir::new("archive");
        let d = &t.0;
        // tar.gz mit oberstem Ordner
        let tgz = d.join("a.tar.gz");
        {
            let f = fs::File::create(&tgz).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let data = b"hello";
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, "factorio/bin/x64/factorio", &data[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let out = d.join("out");
        extract_strip(&tgz, &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("bin/x64/factorio")).unwrap(), "hello");
        // zip
        let z = d.join("b.zip");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&z).unwrap());
            w.start_file("jdk-21/bin/java", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let out2 = d.join("out2");
        extract_strip(&z, &out2).unwrap();
        assert!(out2.join("bin/java").is_file());
    }
}
