//! Backups als ZIP unter `<data>/backups/<server-id>/`.

use std::path::{Path, PathBuf};

use axum::{
    Json,
    body::Body,
    extract::{Path as AxPath, State},
    response::Response,
};
use nrz_gp_common::{Backup, CreateBackup, EventKind, Game, Ok as OkResp, ServerConfig};
use tokio_util::io::ReaderStream;

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    files::{attachment, extract_archive, write_zip},
    state::AppState,
    util,
};

fn dir(app: &AppState, id: &str) -> PathBuf {
    app.config.backups_dir().join(id)
}

fn checked_name(name: &str) -> AppResult<&str> {
    if name.contains('/') || name.contains("..") || !name.ends_with(".zip") {
        return Err(AppError::bad("Ungültiger Backup-Name"));
    }
    Ok(name)
}

pub async fn list(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<Vec<Backup>>> {
    app.server(&id).await?;
    Ok(Json(list_backups(&dir(&app, &id))))
}

fn list_backups(d: &Path) -> Vec<Backup> {
    let mut v: Vec<Backup> = std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".zip"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some(Backup {
                name: e.file_name().to_string_lossy().into(),
                size: m.len(),
                created_at: m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
            })
        })
        .collect();
    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    v
}

/// Welt-Ordner eines Minecraft-Servers (`level-name` + `_nether` + `_the_end`) bzw. Factorio `saves`.
pub fn world_paths(cfg: &ServerConfig) -> Vec<PathBuf> {
    let base = Path::new(&cfg.dir);
    match cfg.game {
        Game::Minecraft => {
            let props = util::read_properties(&base.join("server.properties"));
            let level = util::value(&props, "level-name").unwrap_or("world").to_string();
            [level.clone(), format!("{level}_nether"), format!("{level}_the_end")]
                .into_iter()
                .map(|n| base.join(n))
                .filter(|p| p.is_dir())
                .collect()
        }
        Game::Factorio => vec![base.join("saves")].into_iter().filter(|p| p.is_dir()).collect(),
    }
}

/// Backup erstellen (auch von Automatisierungen genutzt).
pub async fn create_backup(app: &AppState, id: &str, worlds_only: bool) -> AppResult<Backup> {
    let (cfg, rt) = app.server(id).await?;
    let running_mc = cfg.game == Game::Minecraft && rt.status() == nrz_gp_common::ServerStatus::Running;
    if running_mc {
        let _ = rt.command(app, "save-off").await;
        let _ = rt.command(app, "save-all flush").await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    rt.panel(if worlds_only { "Erstelle Backup der Welten …" } else { "Erstelle Backup des Server-Ordners …" });
    let d = dir(app, id);
    tokio::fs::create_dir_all(&d).await?;
    let name = format!("{}{}.zip", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"), if worlds_only { "_welt" } else { "" });
    let target = d.join(&name);
    let base = PathBuf::from(&cfg.dir);
    let paths = if worlds_only { world_paths(&cfg) } else { vec![base.clone()] };
    let t2 = target.clone();
    let res = tokio::task::spawn_blocking(move || -> AppResult<()> {
        let f = std::fs::File::create(&t2)?;
        write_zip(std::io::BufWriter::new(f), &base, &paths, &["session.lock"])
    })
    .await?;
    if running_mc {
        let _ = rt.command(app, "save-on").await;
    }
    if let Err(e) = res {
        let _ = tokio::fs::remove_file(&target).await;
        rt.panel(format!("Backup fehlgeschlagen: {e}"));
        return Err(e);
    }
    let size = tokio::fs::metadata(&target).await.map(|m| m.len()).unwrap_or(0);
    rt.panel(format!("Backup erstellt: {name} ({})", util::human_bytes(size)));
    app.events.emit(id, EventKind::BackupCreated, None, format!("Backup {name} ({})", util::human_bytes(size)));
    Ok(Backup { name, size, created_at: crate::store::now() })
}

pub async fn create(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(c): Json<CreateBackup>) -> AppResult<Json<Backup>> {
    Ok(Json(create_backup(&app, &id, c.worlds_only).await?))
}

pub async fn restore(State(app): State<AppState>, _a: Authed, AxPath((id, name)): AxPath<(String, String)>) -> AppResult<Json<OkResp>> {
    let (cfg, rt) = app.server(&id).await?;
    if rt.status().is_online() {
        return Err(AppError::conflict("Bitte den Server zuerst stoppen"));
    }
    let path = dir(&app, &id).join(checked_name(&name)?);
    if !path.is_file() {
        return Err(AppError::not_found("Backup nicht gefunden"));
    }
    let base = PathBuf::from(&cfg.dir);
    // Welt-Backups: vorhandene Welt-Ordner vorher entfernen, damit keine alten Chunks bleiben.
    if name.ends_with("_welt.zip") {
        for p in world_paths(&cfg) {
            tokio::fs::remove_dir_all(p).await.ok();
        }
    }
    rt.panel(format!("Stelle Backup {name} wieder her …"));
    tokio::task::spawn_blocking(move || extract_archive(&path, &base)).await??;
    rt.panel("Backup wiederhergestellt.");
    Ok(Json(OkResp { ok: true }))
}

pub async fn download(State(app): State<AppState>, _a: Authed, AxPath((id, name)): AxPath<(String, String)>) -> AppResult<Response> {
    app.server(&id).await?;
    let path = dir(&app, &id).join(checked_name(&name)?);
    let f = tokio::fs::File::open(&path).await?;
    Ok(attachment(Body::from_stream(ReaderStream::new(f)), &name, "application/zip"))
}

pub async fn delete(State(app): State<AppState>, _a: Authed, AxPath((id, name)): AxPath<(String, String)>) -> AppResult<Json<OkResp>> {
    app.server(&id).await?;
    tokio::fs::remove_file(dir(&app, &id).join(checked_name(&name)?)).await?;
    Ok(Json(OkResp { ok: true }))
}
