//! Routen: Server-Liste, Anlegen, Ändern, Löschen, Steuerung, Konsole (WebSocket), Ereignisse.

use std::path::{Path, PathBuf};

use axum::{
    Json,
    extract::{
        Path as AxPath, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use nrz_gp_common::{
    CommandRequest, CommandResponse, CreateServer, Game, GameEvent, LaunchKind, Ok as OkResp, PortCheck, PowerAction, PowerRequest,
    RconConfig, ServerConfig, ServerStatus, ServerSummary, SystemInfo, UpdateServer, WsClientMessage, WsMessage,
};
use serde::Deserialize;

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    install, process,
    state::AppState,
    store::{new_id, now, slugify},
    util,
};

pub async fn summary(app: &AppState, cfg: ServerConfig) -> ServerSummary {
    let rt = app.runtime(&cfg.id).await;
    let installs = app.java.read().await.clone();
    let java = if matches!(cfg.launch, LaunchKind::Factorio { .. } | LaunchKind::Script { .. }) {
        None
    } else {
        process::resolve_java(&cfg, &installs)
    };
    let warnings = process::warnings(&cfg, &installs);
    ServerSummary { status: rt.status(), stats: rt.stats(), players: rt.players(), warnings, java, config: cfg }
}

pub async fn list(State(app): State<AppState>, _a: Authed) -> Json<Vec<ServerSummary>> {
    let mut out = Vec::new();
    for cfg in app.store.servers().await {
        out.push(summary(&app, cfg).await);
    }
    Json(out)
}

pub async fn get(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<ServerSummary>> {
    let (cfg, _) = app.server(&id).await?;
    Ok(Json(summary(&app, cfg).await))
}

/// Freien Ordner unter `<data>/servers/` finden.
fn unique_dir(base: &Path, slug: &str) -> PathBuf {
    let mut p = base.join(slug);
    let mut n = 2;
    while p.exists() {
        p = base.join(format!("{slug}-{n}"));
        n += 1;
    }
    p
}

/// Nächster freier Port ab `start` (nicht von anderen Panel-Servern belegt und auf dem Host frei).
pub async fn free_port(app: &AppState, start: u16, udp: bool) -> u16 {
    let used: Vec<u16> = app.store.read(|s| s.servers.iter().flat_map(|c| [Some(c.port), c.rcon.as_ref().map(|r| r.port)]).flatten().collect()).await;
    let mut p = start;
    loop {
        let host_free = if udp { util::udp_port_free(p) } else { util::tcp_port_free(p) };
        if !used.contains(&p) && host_free {
            return p;
        }
        p = p.wrapping_add(1).max(1024);
    }
}

/// RCON-Zugang erzeugen (freier Port ab 25575 bzw. 27015).
pub async fn new_rcon(app: &AppState, game: Game) -> RconConfig {
    let start = match game {
        Game::Minecraft => 25575,
        Game::Factorio => 27015,
    };
    RconConfig { port: free_port(app, start, false).await, password: new_id(12) }
}

pub async fn create(State(app): State<AppState>, _a: Authed, Json(req): Json<CreateServer>) -> AppResult<Json<ServerSummary>> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad("Name fehlt"));
    }
    if req.game == Game::Minecraft && !req.accept_eula {
        return Err(AppError::bad("Bitte die Minecraft-EULA akzeptieren"));
    }
    let base = app.config.servers_dir();
    tokio::fs::create_dir_all(&base).await?;
    let dir = unique_dir(&base, &slugify(&name));
    tokio::fs::create_dir_all(&dir).await?;
    let port = if req.port == 0 { free_port(&app, req.game.default_port(), req.game == Game::Factorio).await } else { req.port };
    let cfg = ServerConfig {
        id: new_id(6),
        name,
        game: req.game,
        flavor: if req.game == Game::Factorio { nrz_gp_common::Flavor::Factorio } else { req.flavor },
        version: req.version.clone(),
        loader_version: req.loader_version.clone(),
        dir: dir.to_string_lossy().into(),
        launch: match req.game {
            Game::Minecraft => LaunchKind::Jar { jar: "server.jar".into() },
            Game::Factorio => LaunchKind::Factorio { binary: "bin/x64/factorio".into(), save: None },
        },
        java_path: None,
        memory_mb: if req.memory_mb == 0 { 4096 } else { req.memory_mb },
        jvm_args: String::new(),
        extra_args: String::new(),
        port,
        auto_start: req.auto_start,
        auto_restart: true,
        rcon: Some(new_rcon(&app, req.game).await),
        imported: false,
        created_at: now(),
    };
    app.store.update(|s| s.servers.push(cfg.clone())).await?;
    let rt = app.runtime(&cfg.id).await;
    rt.set_status(ServerStatus::Installing);
    install::spawn_install(app.clone(), cfg.id.clone(), req);
    Ok(Json(summary(&app, cfg).await))
}

pub async fn update(
    State(app): State<AppState>,
    _a: Authed,
    AxPath(id): AxPath<String>,
    Json(u): Json<UpdateServer>,
) -> AppResult<Json<ServerSummary>> {
    if let Some(n) = &u.name {
        if n.trim().is_empty() {
            return Err(AppError::bad("Name darf nicht leer sein"));
        }
    }
    let cfg = app
        .update_server(&id, |c| {
            if let Some(v) = u.name {
                c.name = v.trim().to_string();
            }
            if let Some(v) = u.version {
                c.version = Some(v).filter(|s| !s.trim().is_empty());
            }
            if let Some(v) = u.flavor {
                c.flavor = v;
            }
            if let Some(v) = u.launch {
                c.launch = v;
            }
            if let Some(v) = u.java_path {
                c.java_path = v.filter(|s| !s.trim().is_empty());
            }
            if let Some(v) = u.memory_mb {
                c.memory_mb = v.max(256);
            }
            if let Some(v) = u.jvm_args {
                c.jvm_args = v;
            }
            if let Some(v) = u.extra_args {
                c.extra_args = v;
            }
            if let Some(v) = u.port {
                c.port = v;
            }
            if let Some(v) = u.auto_start {
                c.auto_start = v;
            }
            if let Some(v) = u.auto_restart {
                c.auto_restart = v;
            }
        })
        .await?;
    if cfg.game == Game::Minecraft && Path::new(&cfg.dir).join("server.properties").exists() {
        let _ = process::sync_minecraft_properties(Path::new(&cfg.dir), &cfg);
    }
    Ok(Json(summary(&app, cfg).await))
}

#[derive(Deserialize)]
pub struct DeleteQuery {
    #[serde(default)]
    delete_files: bool,
}

pub async fn delete(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Query(q): Query<DeleteQuery>) -> AppResult<Json<OkResp>> {
    let (cfg, rt) = app.server(&id).await?;
    if rt.status().is_online() {
        rt.stop_and_wait(&app).await?;
    }
    app.store
        .update(|s| {
            s.servers.retain(|c| c.id != id);
            s.automations.retain(|a| a.server_id != id);
        })
        .await?;
    app.runtimes.write().await.remove(&id);
    // Nur Ordner im Panel-Datenordner löschen, importierte nie.
    if q.delete_files && !cfg.imported && Path::new(&cfg.dir).starts_with(app.config.servers_dir()) {
        tokio::fs::remove_dir_all(&cfg.dir).await.ok();
    }
    Ok(Json(OkResp { ok: true }))
}

pub async fn power(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(p): Json<PowerRequest>) -> AppResult<Json<OkResp>> {
    let (_, rt) = app.server(&id).await?;
    match p.action {
        PowerAction::Start => rt.start(&app).await?,
        PowerAction::Stop => rt.stop(&app),
        PowerAction::Restart => rt.restart(&app).await?,
        PowerAction::Kill => rt.kill(),
    }
    Ok(Json(OkResp { ok: true }))
}

pub async fn command(
    State(app): State<AppState>,
    _a: Authed,
    AxPath(id): AxPath<String>,
    Json(c): Json<CommandRequest>,
) -> AppResult<Json<CommandResponse>> {
    let (_, rt) = app.server(&id).await?;
    let output = rt.command(&app, &c.command).await?;
    Ok(Json(CommandResponse { output }))
}

pub async fn accept_eula(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<OkResp>> {
    let (cfg, _) = app.server(&id).await?;
    process::accept_eula(Path::new(&cfg.dir))?;
    Ok(Json(OkResp { ok: true }))
}

pub async fn enable_rcon(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<ServerSummary>> {
    let (cfg, rt) = app.server(&id).await?;
    let rcon = new_rcon(&app, cfg.game).await;
    let cfg = app.update_server(&id, |c| c.rcon = Some(rcon)).await?;
    if cfg.game == Game::Minecraft {
        process::sync_minecraft_properties(Path::new(&cfg.dir), &cfg)?;
    }
    if rt.status().is_online() {
        rt.panel("RCON eingerichtet – wird nach dem nächsten Neustart aktiv.");
    }
    Ok(Json(summary(&app, cfg).await))
}

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<usize>,
}

pub async fn console(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Query(q): Query<LimitQuery>) -> AppResult<Json<Vec<nrz_gp_common::ConsoleLine>>> {
    let (_, rt) = app.server(&id).await?;
    Ok(Json(rt.history(q.limit.unwrap_or(500))))
}

pub async fn server_events(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Query(q): Query<LimitQuery>) -> AppResult<Json<Vec<GameEvent>>> {
    app.server(&id).await?;
    Ok(Json(app.events.recent(Some(&id), q.limit.unwrap_or(200))))
}

pub async fn all_events(State(app): State<AppState>, _a: Authed, Query(q): Query<LimitQuery>) -> Json<Vec<GameEvent>> {
    Json(app.events.recent(None, q.limit.unwrap_or(100)))
}

/// WebSocket: Verlauf, Live-Zeilen, Status, Statistik, Ereignisse; nimmt Befehle an.
pub async fn ws(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, upgrade: WebSocketUpgrade) -> AppResult<Response> {
    let (_, rt) = app.server(&id).await?;
    Ok(upgrade.on_upgrade(move |socket| handle_ws(app, rt, socket)))
}

async fn handle_ws(app: AppState, rt: std::sync::Arc<process::ServerRuntime>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = rt.subscribe();
    let initial = [
        WsMessage::History { lines: rt.history(1000) },
        WsMessage::Status { status: rt.status() },
        WsMessage::Stats { stats: rt.stats(), players: rt.players() },
    ];
    for m in initial {
        if let Ok(t) = serde_json::to_string(&m) {
            if sink.send(Message::Text(t.into())).await.is_err() {
                return;
            }
        }
    }
    let send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    let Ok(t) = serde_json::to_string(&m) else { continue };
                    if sink.send(Message::Text(t.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    while let Some(Ok(msg)) = stream.next().await {
        if let Message::Text(t) = msg {
            if let Ok(WsClientMessage::Command { command }) = serde_json::from_str(&t) {
                if let Err(e) = rt.command(&app, &command).await {
                    rt.panel(format!("Befehl fehlgeschlagen: {e}"));
                }
            }
        }
    }
    send_task.abort();
}

#[derive(Deserialize)]
pub struct PortQuery {
    port: u16,
    game: Option<Game>,
}

pub async fn check_port(State(app): State<AppState>, _a: Authed, Query(q): Query<PortQuery>) -> Json<PortCheck> {
    let used_by = app
        .store
        .read(|s| s.servers.iter().find(|c| c.port == q.port || c.rcon.as_ref().is_some_and(|r| r.port == q.port)).map(|c| c.name.clone()))
        .await;
    let free = used_by.is_none()
        && match q.game {
            Some(Game::Factorio) => util::udp_port_free(q.port),
            _ => util::tcp_port_free(q.port),
        };
    Json(PortCheck { port: q.port, free, used_by })
}

pub async fn system(State(app): State<AppState>, _a: Authed) -> Json<SystemInfo> {
    // Java neu suchen (günstig) – so erscheinen nachinstallierte Versionen sofort.
    let java = crate::java::redetect(&app).await;
    let data_dir = app.config.data_dir.clone();
    let (cpu_count, cpu_usage, memory_total, memory_used) = {
        let mut sys = app.sys.lock().unwrap_or_else(|e| e.into_inner());
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        (sys.cpus().len() as u32, sys.global_cpu_usage(), sys.total_memory(), sys.used_memory())
    };
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk = disks.iter().filter(|d| data_dir.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len());
    let hostname = sysinfo::System::host_name().unwrap_or_else(|| "localhost".into());
    Json(SystemInfo {
        panel_version: nrz_gp_common::PANEL_VERSION.into(),
        public_host: app.config.public_host.clone().unwrap_or_else(|| hostname.clone()),
        hostname,
        os: sysinfo::System::long_os_version().unwrap_or_default(),
        cpu_count,
        cpu_usage,
        memory_total,
        memory_used,
        disk_total: disk.map_or(0, |d| d.total_space()),
        disk_free: disk.map_or(0, |d| d.available_space()),
        data_dir: data_dir.to_string_lossy().into(),
        java,
        import_roots: app.config.import_roots.iter().map(|p| p.to_string_lossy().into()).collect(),
    })
}
