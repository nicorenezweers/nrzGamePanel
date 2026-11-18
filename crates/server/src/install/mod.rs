//! Installation neuer Server (Vanilla, Paper, Purpur, Folia, Fabric, Forge, NeoForge, Factorio
//! headless) und Versionslisten.
//!
//! Route: `/api/versions/{game}/{flavor}`.

pub mod archive;
pub mod versions;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    routing::get,
};
use nrz_gp_common::{CreateServer, Flavor, Game, LaunchKind, ServerConfig, ServerStatus, VersionList};
use serde::Deserialize;
use tokio::process::Command;

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    import::detect,
    java,
    process::{self, ServerRuntime},
    state::AppState,
    util,
};

pub use archive::run_logged;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/versions/{game}/{flavor}", get(versions_route))
}

#[derive(Deserialize)]
pub struct VersionQuery {
    #[serde(default)]
    game_version: Option<String>,
}

async fn versions_route(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((game, flavor)): AxPath<(Game, Flavor)>,
    Query(q): Query<VersionQuery>,
) -> AppResult<Json<VersionList>> {
    let gv = q.game_version.filter(|s| !s.trim().is_empty());
    Ok(Json(versions::version_list(&app, game, flavor, gv.as_deref()).await?))
}

/// Installation im Hintergrund starten. Status ist bereits `Installing`; Fortschritt als
/// Panel-Zeilen in die Konsole; danach Konfiguration (launch, version …) aktualisieren und
/// Status `Stopped` (bzw. starten, wenn `auto_start`).
pub fn spawn_install(app: AppState, id: String, req: CreateServer) {
    tokio::spawn(async move {
        let rt = app.runtime(&id).await;
        match install(&app, &rt, &id, &req).await {
            Ok(()) => {
                rt.panel("✔ Installation abgeschlossen.");
                rt.set_status(ServerStatus::Stopped);
                if req.auto_start {
                    if let Err(e) = rt.start(&app).await {
                        rt.panel(format!("Start fehlgeschlagen: {e}"));
                    }
                }
            }
            Err(e) => {
                rt.panel(format!("✖ Installation fehlgeschlagen: {e}"));
                rt.panel("Du kannst den Server löschen und neu erstellen oder die Dateien selbst ergänzen.");
                rt.set_status(ServerStatus::Stopped);
            }
        }
    });
}

/// Download mit Fortschritt (etwa alle 10 %) in der Konsole.
async fn download(app: &AppState, rt: &ServerRuntime, url: &str, dest: &Path, label: &str) -> AppResult<()> {
    rt.panel(format!("Lade {label} …"));
    let mut last = 0u64;
    util::download(&app.http, url, dest, |done, total| match total.filter(|t| *t > 0) {
        Some(t) => {
            let pct = done * 100 / t;
            if pct >= last + 10 {
                last = pct - pct % 10;
                rt.panel(format!("{label}: {pct} % ({} von {})", util::human_bytes(done), util::human_bytes(t)));
            }
        }
        None => {
            // Grösse unbekannt: alle ~25 MB melden
            if done / (25 * 1024 * 1024) > last {
                last = done / (25 * 1024 * 1024);
                rt.panel(format!("{label}: {} geladen", util::human_bytes(done)));
            }
        }
    })
    .await
}

async fn install(app: &AppState, rt: &Arc<ServerRuntime>, id: &str, req: &CreateServer) -> AppResult<()> {
    let cfg = app.store.server(id).await.ok_or_else(|| AppError::not_found("Server nicht gefunden"))?;
    let dir = PathBuf::from(&cfg.dir);
    tokio::fs::create_dir_all(&dir).await?;
    match cfg.game {
        Game::Minecraft => install_minecraft(app, rt, &cfg, &dir, req).await,
        Game::Factorio => install_factorio(app, rt, &cfg, &dir, req).await,
    }
}

/* ------------------------------------------------------------------ */
/*                             Minecraft                              */
/* ------------------------------------------------------------------ */

async fn install_minecraft(app: &AppState, rt: &Arc<ServerRuntime>, cfg: &ServerConfig, dir: &Path, req: &CreateServer) -> AppResult<()> {
    let flavor = cfg.flavor;
    let wanted = req.version.clone().filter(|v| !v.trim().is_empty());
    rt.panel(format!("Installiere {} {} …", flavor.label(), wanted.as_deref().unwrap_or("(neueste Version)")));
    let (version, loader, launch) = match flavor {
        Flavor::Vanilla | Flavor::Paper | Flavor::Folia | Flavor::Purpur => {
            let v = match wanted {
                Some(v) => v,
                None => versions::latest(app, Game::Minecraft, flavor).await?,
            };
            let url = match flavor {
                Flavor::Vanilla => versions::vanilla_download_url(app, &v).await?,
                Flavor::Purpur => versions::purpur_download_url(&v),
                Flavor::Folia => versions::paper_download_url(app, "folia", &v).await?,
                _ => versions::paper_download_url(app, "paper", &v).await?,
            };
            download(app, rt, &url, &dir.join("server.jar"), &format!("{} {v}", flavor.label())).await?;
            (v, None, LaunchKind::Jar { jar: "server.jar".into() })
        }
        Flavor::Fabric => {
            let v = match wanted {
                Some(v) => v,
                None => versions::latest(app, Game::Minecraft, Flavor::Fabric).await?,
            };
            let loader = match req.loader_version.clone().filter(|l| !l.trim().is_empty()) {
                Some(l) => l,
                None => versions::fabric_loaders(app).await?.into_iter().next().ok_or_else(|| AppError::internal("Kein Fabric-Loader gefunden"))?,
            };
            let installer = versions::fabric_installer(app).await?;
            let url = versions::fabric_server_jar_url(&v, &loader, &installer);
            download(app, rt, &url, &dir.join("fabric-server-launch.jar"), &format!("Fabric {v} (Loader {loader})")).await?;
            rt.panel("Fabric lädt den Minecraft-Server beim ersten Start selbst herunter.");
            (v, Some(loader), LaunchKind::Jar { jar: "fabric-server-launch.jar".into() })
        }
        Flavor::Forge => {
            let mc = match wanted {
                Some(v) => v,
                None => versions::latest(app, Game::Minecraft, Flavor::Forge).await?,
            };
            let (forge, full) = versions::resolve_forge(app, &mc, req.loader_version.as_deref()).await?;
            let installer = format!("forge-{full}-installer.jar");
            download(app, rt, &versions::forge_installer_url(&full), &dir.join(&installer), &format!("Forge-Installer {full}")).await?;
            run_installer(app, rt, dir, &installer, &mc, Flavor::Forge).await?;
            let args = format!("libraries/net/minecraftforge/forge/{full}/unix_args.txt");
            let launch = if dir.join(&args).is_file() { LaunchKind::ArgsFile { args_file: args } } else { detected_launch(dir, Flavor::Forge)? };
            (mc, Some(forge), launch)
        }
        Flavor::NeoForge => {
            let neo = versions::resolve_neoforge(app, wanted.as_deref(), req.loader_version.as_deref()).await?;
            let mc = wanted.or_else(|| detect::neoforge_mc(&neo)).ok_or_else(|| AppError::bad(format!("Unbekannte NeoForge-Version {neo}")))?;
            let installer = format!("neoforge-{neo}-installer.jar");
            download(app, rt, &versions::neoforge_installer_url(&neo), &dir.join(&installer), &format!("NeoForge-Installer {neo}")).await?;
            run_installer(app, rt, dir, &installer, &mc, Flavor::NeoForge).await?;
            let args = format!("libraries/net/neoforged/neoforge/{neo}/unix_args.txt");
            let launch = if dir.join(&args).is_file() { LaunchKind::ArgsFile { args_file: args } } else { detected_launch(dir, Flavor::NeoForge)? };
            (mc, Some(neo), launch)
        }
        other => return Err(AppError::bad(format!("{} kann nicht automatisch installiert werden", other.label()))),
    };

    if req.accept_eula {
        process::accept_eula(dir)?;
    }
    let mut props: Vec<(String, String)> = Vec::new();
    if let Some(m) = req.motd.as_ref().filter(|m| !m.trim().is_empty()) {
        props.push(("motd".into(), m.trim().into()));
    }
    if let Some(n) = req.max_players {
        props.push(("max-players".into(), n.to_string()));
    }
    if let Some(o) = req.online_mode {
        props.push(("online-mode".into(), o.to_string()));
    }
    if let Some(d) = req.difficulty.as_ref().filter(|d| !d.trim().is_empty()) {
        props.push(("difficulty".into(), d.trim().to_lowercase()));
    }
    if let Some(g) = req.gamemode.as_ref().filter(|g| !g.trim().is_empty()) {
        props.push(("gamemode".into(), g.trim().to_lowercase()));
    }
    if let Some(s) = req.seed.as_ref().filter(|s| !s.trim().is_empty()) {
        props.push(("level-seed".into(), s.trim().into()));
    }
    if !props.is_empty() {
        util::set_properties(&dir.join("server.properties"), &props)?;
    }
    let launch2 = launch.clone();
    let (v2, l2) = (version.clone(), loader.clone());
    let cfg = app
        .update_server(&cfg.id, move |c| {
            c.version = Some(v2);
            c.loader_version = l2;
            c.launch = launch2;
        })
        .await?;
    process::sync_minecraft_properties(dir, &cfg)?;
    rt.panel(format!("Start mit: {}", launch.describe()));
    Ok(())
}

/// Forge-/NeoForge-Installer mit passendem Java ausführen und danach aufräumen.
async fn run_installer(app: &AppState, rt: &Arc<ServerRuntime>, dir: &Path, installer: &str, mc: &str, flavor: Flavor) -> AppResult<()> {
    let major = java::required_for(Some(mc), flavor);
    let r = rt.clone();
    let j = java::ensure_java_with(app, major, &move |m| r.panel(m)).await?;
    rt.panel(format!("Führe den {}-Installer aus (Java {}) – das dauert einen Moment …", flavor.label(), j.major));
    let mut cmd = Command::new(&j.path);
    cmd.arg("-jar").arg(installer).arg("--installServer").current_dir(dir);
    let res = run_logged(rt, cmd).await;
    for f in [installer.to_string(), format!("{installer}.log"), "installer.log".into()] {
        let _ = tokio::fs::remove_file(dir.join(f)).await;
    }
    res
}

/// Startdatei nach der Installation über die Import-Erkennung finden.
fn detected_launch(dir: &Path, flavor: Flavor) -> AppResult<LaunchKind> {
    let (scripts, files) = detect::scripts_info(dir);
    let cands = detect::launch_candidates(dir, &scripts, &files);
    cands
        .iter()
        .find(|c| c.flavor == flavor && !matches!(c.launch, LaunchKind::Script { .. }))
        .or(cands.first())
        .map(|c| c.launch.clone())
        .ok_or_else(|| AppError::internal(format!("{}-Installer hat keine Startdatei erzeugt", flavor.label())))
}

/* ------------------------------------------------------------------ */
/*                              Factorio                              */
/* ------------------------------------------------------------------ */

/// Factorio-Headless gibt es nur für Linux x86_64.
pub fn factorio_supported() -> AppResult<()> {
    if std::env::consts::ARCH != "x86_64" || std::env::consts::OS != "linux" {
        return Err(AppError::bad(format!(
            "Factorio-Headless gibt es nur für Linux x86_64 (dieser Host: {} {}).",
            std::env::consts::OS,
            std::env::consts::ARCH
        )));
    }
    Ok(())
}

async fn install_factorio(app: &AppState, rt: &Arc<ServerRuntime>, cfg: &ServerConfig, dir: &Path, req: &CreateServer) -> AppResult<()> {
    factorio_supported()?;
    let wanted = req.version.clone().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "stable".into());
    let url = format!("https://factorio.com/get-download/{wanted}/headless/linux64");
    let archive = app.config.cache_dir().join(format!("factorio-{}-{wanted}.tar.xz", cfg.id));
    download(app, rt, &url, &archive, &format!("Factorio headless ({wanted})")).await?;
    rt.panel("Entpacke Factorio …");
    let (a, d) = (archive.clone(), dir.to_path_buf());
    let res = tokio::task::spawn_blocking(move || archive::extract_strip(&a, &d)).await;
    let _ = tokio::fs::remove_file(&archive).await;
    res??;
    let binary = "bin/x64/factorio";
    if !dir.join(binary).is_file() {
        return Err(AppError::internal("Nach dem Entpacken fehlt bin/x64/factorio"));
    }
    let version = std::fs::read_to_string(dir.join("data/base/info.json"))
        .ok()
        .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
        .and_then(|j| j.get("version").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or(wanted);

    // server-settings.json aus dem Beispiel
    let example = dir.join("data/server-settings.example.json");
    let target = dir.join("data/server-settings.json");
    let mut settings: serde_json::Value = std::fs::read_to_string(&example)
        .ok()
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(o) = settings.as_object_mut() {
        o.insert("name".into(), cfg.name.clone().into());
        o.insert("description".into(), req.motd.clone().unwrap_or_default().into());
        o.insert("max_players".into(), req.max_players.unwrap_or(0).into());
        o.insert("visibility".into(), serde_json::json!({ "public": false, "lan": true }));
        if req.online_mode == Some(false) {
            o.insert("require_user_verification".into(), false.into());
        }
    }
    std::fs::write(&target, serde_json::to_string_pretty(&settings).map_err(|e| AppError::internal(e.to_string()))?)?;

    // Karte erzeugen
    tokio::fs::create_dir_all(dir.join("saves")).await?;
    rt.panel("Erzeuge Karte saves/world.zip …");
    let mut cmd = Command::new(dir.join(binary));
    cmd.arg("--create").arg("saves/world.zip").current_dir(dir);
    if let Some(seed) = req.seed.as_ref().and_then(|s| s.trim().parse::<u32>().ok()) {
        cmd.arg("--map-gen-seed").arg(seed.to_string());
    }
    run_logged(rt, cmd).await?;

    app.update_server(&cfg.id, move |c| {
        c.version = Some(version);
        c.loader_version = None;
        c.launch = LaunchKind::Factorio { binary: binary.into(), save: None };
    })
    .await?;
    Ok(())
}
