//! Import bestehender Server: Ordnerauswahl auf dem Host, automatische Erkennung (Spiel, Software,
//! Version, Startdatei, Java, RAM, Port, Welten), Suche unter den Import-Wurzeln, Übernahme.
//!
//! Routen: `/api/fs/browse`, `/api/import/analyze`, `/api/import/scan`, `/api/import`.

pub mod detect;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use futures::StreamExt;
use nrz_gp_common::{
    AnalyzeRequest, Game, HostDirEntry, HostDirListing, ImportAnalysis, ImportMode, ImportRequest, LaunchKind, RconConfig, ScanRequest,
    ServerConfig, ServerSummary,
};
use serde::Deserialize;

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    java, process, servers,
    state::AppState,
    store::{new_id, now, slugify},
    util,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/fs/browse", get(browse))
        .route("/api/import/analyze", post(analyze_route))
        .route("/api/import/scan", post(scan))
        .route("/api/import", post(import))
}

/* ------------------------------------------------------------------ */
/*                          Ordnerauswahl                             */
/* ------------------------------------------------------------------ */

#[derive(Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    path: Option<String>,
}

async fn browse(State(app): State<AppState>, _a: Authed, Query(q): Query<BrowseQuery>) -> AppResult<Json<HostDirListing>> {
    let roots: Vec<String> = app.config.import_roots.iter().map(|p| p.to_string_lossy().into()).collect();
    let path = q.path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let listing = tokio::task::spawn_blocking(move || -> AppResult<HostDirListing> {
        let target = match path {
            Some(p) => PathBuf::from(p),
            None if roots.len() > 1 => {
                // Liste der Wurzeln
                let dirs = roots
                    .iter()
                    .map(|r| HostDirEntry { name: r.clone(), path: r.clone(), looks_like_server: detect::looks_like_server(Path::new(r)) })
                    .collect();
                return Ok(HostDirListing { path: String::new(), parent: None, dirs, roots });
            }
            None => PathBuf::from(roots.first().cloned().unwrap_or_else(|| "/".into())),
        };
        if !target.is_absolute() {
            return Err(AppError::bad("Bitte einen absoluten Pfad angeben"));
        }
        let target = std::fs::canonicalize(&target).map_err(|_| AppError::not_found("Ordner nicht gefunden"))?;
        if !target.is_dir() {
            return Err(AppError::bad("Kein Ordner"));
        }
        let mut dirs: Vec<HostDirEntry> = std::fs::read_dir(&target)?
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter(|e| e.path().is_dir())
            .map(|e| {
                let p = e.path();
                HostDirEntry { name: e.file_name().to_string_lossy().into(), looks_like_server: detect::looks_like_server(&p), path: p.to_string_lossy().into() }
            })
            .collect();
        dirs.sort_by_key(|d| d.name.to_lowercase());
        Ok(HostDirListing {
            path: target.to_string_lossy().into(),
            parent: target.parent().map(|p| p.to_string_lossy().into()),
            dirs,
            roots,
        })
    })
    .await??;
    Ok(Json(listing))
}

/* ------------------------------------------------------------------ */
/*                             Analyse                                */
/* ------------------------------------------------------------------ */

async fn analyze_route(State(app): State<AppState>, _a: Authed, Json(r): Json<AnalyzeRequest>) -> AppResult<Json<ImportAnalysis>> {
    Ok(Json(analyze(&app, &r.path).await?))
}

/// Arbeitsordner aller laufenden Prozesse (um „läuft noch woanders“ zu erkennen).
fn process_cwds() -> Vec<PathBuf> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always));
    // Nur Spielserver-Prozesse zählen (ein offenes Terminal im Ordner ist kein laufender Server).
    sys.processes()
        .values()
        .filter(|p| {
            let n = p.name().to_string_lossy().to_lowercase();
            n.contains("java") || n.contains("factorio")
        })
        .filter_map(|p| p.cwd().map(Path::to_path_buf))
        .collect()
}

/// Ordner analysieren.
pub async fn analyze(app: &AppState, path: &str) -> AppResult<ImportAnalysis> {
    let cwds = Arc::new(tokio::task::spawn_blocking(process_cwds).await.unwrap_or_default());
    analyze_with(app, path, cwds).await
}

async fn analyze_with(app: &AppState, path: &str, cwds: Arc<Vec<PathBuf>>) -> AppResult<ImportAnalysis> {
    let p = PathBuf::from(path.trim());
    if !p.is_absolute() {
        return Err(AppError::bad("Bitte einen absoluten Pfad angeben"));
    }
    let dir = std::fs::canonicalize(&p).map_err(|_| AppError::not_found(format!("Ordner nicht gefunden: {path}")))?;
    if !dir.is_dir() {
        return Err(AppError::bad("Kein Ordner"));
    }
    let d = dir.clone();
    let mut a = tokio::task::spawn_blocking(move || detect::analyze_dir(&d)).await?;

    // Bereits im Panel?
    let existing = app.store.read(|s| s.servers.iter().map(|c| (c.id.clone(), c.name.clone(), c.dir.clone())).collect::<Vec<_>>()).await;
    let mut panel_owned = false;
    for (id, name, cdir) in existing {
        if same_dir(Path::new(&cdir), &dir) {
            a.already_imported = Some(id.clone());
            a.notes.insert(0, format!("Dieser Ordner ist bereits als Server „{name}“ im Panel."));
            panel_owned = app.runtime(&id).await.status().is_online();
        }
    }
    if !panel_owned && cwds.iter().any(|c| c.starts_with(&dir)) {
        let msg = if a.game == Some(Game::Factorio) {
            "Ein Prozess läuft gerade in diesem Ordner – läuft der Server noch woanders?"
        } else {
            "session.lock wird gerade benutzt – läuft der Server noch woanders?"
        };
        a.notes.insert(0, msg.into());
    }

    // Java (nur Minecraft mit Java-Start)
    if a.game == Some(Game::Minecraft) && !matches!(a.launch, Some(LaunchKind::Script { .. })) {
        let req = java::required_for(a.version.as_deref(), a.flavor);
        a.java_required = Some(req);
        let installs = app.java.read().await;
        a.java_path = java::pick(&installs, req).filter(|j| j.major >= req || req <= 8).map(|j| j.path.clone());
        if !java::has_suitable(&installs, req) {
            a.notes.push(format!("Java {req} ist nicht installiert – wird beim ersten Start automatisch geladen."));
        }
    }
    Ok(a)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let ca = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let cb = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

/* ------------------------------------------------------------------ */
/*                              Suche                                 */
/* ------------------------------------------------------------------ */

const SCAN_DEPTH: usize = 4;
const SCAN_MAX_HITS: usize = 50;
const SCAN_MAX_DIRS: usize = 20_000;
const SCAN_SKIP: [&str; 11] =
    ["libraries", "mods", "plugins", "node_modules", "versions", "cache", "logs", "crash-reports", "proc", "sys", "dev"];

/// Server-Ordner unter `roots` suchen (Breitensuche, Server-Ordner werden nicht weiter betreten).
pub fn find_server_dirs(roots: &[PathBuf], data_dir: &Path) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut visited = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        if hits.len() >= SCAN_MAX_HITS || visited >= SCAN_MAX_DIRS {
            break;
        }
        visited += 1;
        let Ok(real) = std::fs::canonicalize(&dir) else { continue };
        if !seen.insert(real.clone()) || real.starts_with(data_dir) {
            continue;
        }
        if detect::looks_like_server(&real) {
            hits.push(real);
            continue;
        }
        if depth >= SCAN_DEPTH {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&real) else { continue };
        let mut children: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                !n.starts_with('.') && !SCAN_SKIP.contains(&n.as_str())
            })
            // Weltordner nicht betreten
            .filter(|e| !e.path().join("level.dat").is_file())
            .map(|e| e.path())
            .collect();
        children.sort();
        queue.extend(children.into_iter().map(|c| (c, depth + 1)));
    }
    hits
}

async fn scan(State(app): State<AppState>, _a: Authed, Json(r): Json<ScanRequest>) -> AppResult<Json<Vec<ImportAnalysis>>> {
    let roots: Vec<PathBuf> = match r.root.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(root) => vec![PathBuf::from(root)],
        None => app.config.import_roots.clone(),
    };
    let data_dir = app.config.data_dir.clone();
    let dirs = tokio::task::spawn_blocking(move || find_server_dirs(&roots, &data_dir)).await?;
    let cwds = Arc::new(tokio::task::spawn_blocking(process_cwds).await.unwrap_or_default());
    let mut out: Vec<ImportAnalysis> = futures::stream::iter(dirs)
        .map(|d| {
            let app = app.clone();
            let cwds = cwds.clone();
            async move { analyze_with(&app, &d.to_string_lossy(), cwds).await.ok() }
        })
        .buffer_unordered(4)
        .filter_map(|a| async move { a.filter(|a| a.game.is_some()) })
        .collect()
        .await;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(out))
}

/* ------------------------------------------------------------------ */
/*                             Übernahme                              */
/* ------------------------------------------------------------------ */

fn unique_dir(base: &Path, slug: &str) -> PathBuf {
    let mut p = base.join(slug);
    let mut n = 2;
    while p.exists() {
        p = base.join(format!("{slug}-{n}"));
        n += 1;
    }
    p
}

/// Ordner rekursiv kopieren (Symlinks als Symlinks).
fn copy_dir(src: &Path, dst: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dst)?;
    for e in walkdir::WalkDir::new(src).follow_links(false) {
        let e = e.map_err(|e| AppError::internal(format!("Kopieren fehlgeschlagen: {e}")))?;
        let Ok(rel) = e.path().strip_prefix(src) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        let ft = e.file_type();
        if ft.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            if let Ok(link) = std::fs::read_link(e.path()) {
                let _ = std::os::unix::fs::symlink(link, &target);
            }
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// Vorhandenen RCON-Zugang aus server.properties übernehmen.
fn existing_rcon(dir: &Path) -> Option<RconConfig> {
    let props = util::read_properties(&dir.join("server.properties"));
    if !util::value(&props, "enable-rcon").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
        return None;
    }
    let port = util::value(&props, "rcon.port").and_then(|p| p.trim().parse().ok())?;
    let password = util::value(&props, "rcon.password").filter(|p| !p.is_empty())?.to_string();
    Some(RconConfig { port, password })
}

async fn import(State(app): State<AppState>, _a: Authed, Json(req): Json<ImportRequest>) -> AppResult<Json<ServerSummary>> {
    let analysis = analyze(&app, &req.path).await?;
    if let Some(id) = &analysis.already_imported {
        return Err(AppError::conflict(format!("Dieser Ordner ist bereits im Panel (Server {id})")));
    }
    let game = req.game.or(analysis.game).ok_or_else(|| AppError::bad("Kein Server erkannt – bitte das Spiel angeben"))?;
    let launch = req.launch.clone().or(analysis.launch.clone()).ok_or_else(|| AppError::bad("Keine Startdatei gefunden – bitte angeben"))?;
    let flavor = req.flavor.unwrap_or(if game == Game::Factorio { nrz_gp_common::Flavor::Factorio } else { analysis.flavor });
    let name = Some(req.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| analysis.suggested_name.clone());
    let src = PathBuf::from(&analysis.path);

    let (dir, imported) = match req.mode {
        ImportMode::InPlace => (src.clone(), true),
        ImportMode::Copy => {
            let base = app.config.servers_dir();
            if base.starts_with(&src) {
                return Err(AppError::bad("Dieser Ordner enthält den Datenordner des Panels und kann nicht kopiert werden"));
            }
            tokio::fs::create_dir_all(&base).await?;
            let dst = unique_dir(&base, &slugify(&name));
            let (s, d) = (src.clone(), dst.clone());
            let res = tokio::task::spawn_blocking(move || copy_dir(&s, &d)).await?;
            if let Err(e) = res {
                let _ = tokio::fs::remove_dir_all(&dst).await;
                return Err(e);
            }
            (dst, false)
        }
    };

    let rcon = if req.enable_rcon {
        Some(servers::new_rcon(&app, game).await)
    } else if game == Game::Minecraft {
        existing_rcon(&dir)
    } else {
        None
    };
    let cfg = ServerConfig {
        id: new_id(6),
        name,
        game,
        flavor,
        version: req.version.clone().filter(|v| !v.trim().is_empty()).or(analysis.version.clone()),
        loader_version: analysis.loader_version.clone(),
        dir: dir.to_string_lossy().into(),
        launch,
        java_path: req.java_path.clone().filter(|p| !p.trim().is_empty()),
        memory_mb: req.memory_mb.filter(|m| *m > 0).or(analysis.memory_mb).unwrap_or(4096),
        jvm_args: analysis.jvm_args.clone().unwrap_or_default(),
        extra_args: String::new(),
        port: req.port.filter(|p| *p > 0).or(analysis.port).unwrap_or(game.default_port()),
        auto_start: req.auto_start,
        auto_restart: true,
        rcon,
        imported,
        created_at: now(),
    };

    if game == Game::Minecraft {
        if req.accept_eula {
            process::accept_eula(&dir)?;
        }
        process::sync_minecraft_properties(&dir, &cfg)?;
    }

    let c = cfg.clone();
    let dup = app
        .store
        .update(move |s| {
            if s.servers.iter().any(|x| same_dir(Path::new(&x.dir), Path::new(&c.dir))) {
                return true;
            }
            s.servers.push(c);
            false
        })
        .await?;
    if dup {
        return Err(AppError::conflict("Dieser Ordner ist bereits im Panel"));
    }
    let rt = app.runtime(&cfg.id).await;
    rt.panel(format!("Server importiert aus {}", analysis.path));
    if req.auto_start {
        if let Err(e) = rt.start(&app).await {
            rt.panel(format!("Start fehlgeschlagen: {e}"));
        }
    }
    Ok(Json(servers::summary(&app, cfg).await))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::import::detect::tests::TempDir;

    #[test]
    fn scan_finds_servers_and_skips() {
        let t = TempDir::new("scan");
        let root = &t.0;
        fs::create_dir_all(root.join("a/survival/world")).unwrap();
        fs::write(root.join("a/survival/server.properties"), "server-port=25565\n").unwrap();
        fs::write(root.join("a/survival/world/level.dat"), b"x").unwrap();
        fs::create_dir_all(root.join("b/node_modules/x")).unwrap();
        fs::write(root.join("b/node_modules/x/eula.txt"), "eula=true").unwrap();
        fs::create_dir_all(root.join("data/servers/s1")).unwrap();
        fs::write(root.join("data/servers/s1/eula.txt"), "eula=true").unwrap();
        let data = fs::canonicalize(root.join("data")).unwrap();
        let hits = find_server_dirs(&[root.clone()], &data);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].ends_with("a/survival"));
    }
}
