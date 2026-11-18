//! Laufzeit eines Servers: Prozess starten/stoppen, Konsole (Puffer + Broadcast), Spielerliste,
//! RCON, Statistik, automatischer Neustart nach Absturz.

use std::{
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use nrz_gp_common::{
    ConsoleLine, ConsoleSource, EventKind, Game, LaunchKind, ServerConfig, ServerStats, ServerStatus, ServerWarning, WsMessage,
};
use once_cell::sync::Lazy;
use regex::Regex;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::broadcast,
};

use crate::{
    error::{AppError, AppResult},
    events::{self, Parsed},
    java,
    rcon::RconClient,
    state::AppState,
    store::now_ms,
    util,
};

const CONSOLE_LINES: usize = 3000;
const STOP_TIMEOUT: Duration = Duration::from_secs(90);

struct Inner {
    status: ServerStatus,
    game: Game,
    pid: Option<u32>,
    started_at: Option<Instant>,
    stop_requested: bool,
    restart_requested: bool,
    console: VecDeque<ConsoleLine>,
    seq: u64,
    players: BTreeSet<String>,
    players_max: u32,
    stats: ServerStats,
    crashes: Vec<Instant>,
}

pub struct ServerRuntime {
    pub id: String,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<WsMessage>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    rcon: tokio::sync::Mutex<Option<RconClient>>,
}

impl ServerRuntime {
    pub fn new(id: &str) -> Self {
        let (tx, _) = broadcast::channel(2048);
        Self {
            id: id.to_string(),
            inner: Mutex::new(Inner {
                status: ServerStatus::Stopped,
                game: Game::Minecraft,
                pid: None,
                started_at: None,
                stop_requested: false,
                restart_requested: false,
                console: VecDeque::new(),
                seq: 0,
                players: BTreeSet::new(),
                players_max: 0,
                stats: ServerStats::default(),
                crashes: Vec::new(),
            }),
            tx,
            stdin: tokio::sync::Mutex::new(None),
            rcon: tokio::sync::Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsMessage> {
        self.tx.subscribe()
    }

    pub fn status(&self) -> ServerStatus {
        self.lock().status
    }

    pub fn pid(&self) -> Option<u32> {
        self.lock().pid
    }

    pub fn players(&self) -> Vec<String> {
        self.lock().players.iter().cloned().collect()
    }

    pub fn stats(&self) -> ServerStats {
        let i = self.lock();
        let mut s = i.stats.clone();
        s.players_online = i.players.len() as u32;
        s.players_max = i.players_max;
        s.uptime_secs = i.started_at.map_or(0, |t| t.elapsed().as_secs());
        s
    }

    /// Letzte `limit` Konsolenzeilen (älteste zuerst).
    pub fn history(&self, limit: usize) -> Vec<ConsoleLine> {
        let i = self.lock();
        let skip = i.console.len().saturating_sub(limit);
        i.console.iter().skip(skip).cloned().collect()
    }

    /// Zeile in die Konsole schreiben (auch für Panel-Meldungen von Installern).
    pub fn push(&self, source: ConsoleSource, text: impl Into<String>) -> ConsoleLine {
        let line = {
            let mut i = self.lock();
            i.seq += 1;
            let line = ConsoleLine { seq: i.seq, ts: now_ms(), source, text: text.into() };
            i.console.push_back(line.clone());
            while i.console.len() > CONSOLE_LINES {
                i.console.pop_front();
            }
            line
        };
        let _ = self.tx.send(WsMessage::Line { line: line.clone() });
        line
    }

    /// Panel-Meldung (grau in der Konsole).
    pub fn panel(&self, text: impl Into<String>) {
        self.push(ConsoleSource::Panel, text);
    }

    pub fn set_status(&self, status: ServerStatus) {
        let changed = {
            let mut i = self.lock();
            let c = i.status != status;
            i.status = status;
            if !status.is_online() {
                i.players.clear();
            }
            c
        };
        if changed {
            let _ = self.tx.send(WsMessage::Status { status });
        }
    }

    pub fn set_players_max(&self, max: u32) {
        self.lock().players_max = max;
    }

    /// Spielerliste ersetzen (z. B. nach RCON `list`).
    pub fn set_players(&self, players: impl IntoIterator<Item = String>) {
        self.lock().players = players.into_iter().collect();
    }

    pub fn set_disk(&self, bytes: u64) {
        self.lock().stats.disk_bytes = Some(bytes);
    }

    pub fn broadcast_stats(&self) {
        let _ = self.tx.send(WsMessage::Stats { stats: self.stats(), players: self.players() });
    }

    /* ------------------------------ Start ------------------------------ */

    /// Server starten. Fester Box-Typ, weil `on_exit` (Absturz-Neustart) wieder `start` aufruft.
    pub fn start<'a>(self: &'a Arc<Self>, app: &'a AppState) -> futures::future::BoxFuture<'a, AppResult<()>> {
        Box::pin(self.start_inner(app))
    }

    async fn start_inner(self: &Arc<Self>, app: &AppState) -> AppResult<()> {
        let cfg = app.store.server(&self.id).await.ok_or_else(|| AppError::not_found("Server nicht gefunden"))?;
        {
            let i = self.lock();
            match i.status {
                ServerStatus::Starting | ServerStatus::Running | ServerStatus::Stopping => {
                    return Err(AppError::conflict("Server läuft bereits"));
                }
                ServerStatus::Installing => return Err(AppError::conflict("Server wird noch installiert")),
                _ => {}
            }
        }
        let dir = PathBuf::from(&cfg.dir);
        if !dir.is_dir() {
            return Err(AppError::bad(format!("Server-Ordner fehlt: {}", cfg.dir)));
        }
        if cfg.game == Game::Minecraft {
            if !eula_accepted(&dir) {
                return Err(AppError::bad("Die Minecraft-EULA ist noch nicht akzeptiert"));
            }
            sync_minecraft_properties(&dir, &cfg)?;
            let props = util::read_properties(&dir.join("server.properties"));
            self.set_players_max(util::value(&props, "max-players").and_then(|v| v.parse().ok()).unwrap_or(20));
        }
        let mut java_list = app.java.read().await.clone();
        if cfg.java_path.is_none() && !matches!(cfg.launch, LaunchKind::Factorio { .. } | LaunchKind::Script { .. }) {
            let req = java::required_for(cfg.version.as_deref(), cfg.flavor);
            if !java::has_suitable(&java_list, req) {
                self.panel(format!("Lade Java {req} …"));
                let me = self.clone();
                match java::ensure_java_with(app, req, &move |m| me.panel(m)).await {
                    Ok(_) => java_list = app.java.read().await.clone(),
                    // Download nicht möglich, aber ein anderes Java vorhanden → damit versuchen.
                    Err(e) if resolve_java(&cfg, &java_list).is_some() => self.panel(format!("{e} – verwende vorhandenes Java.")),
                    Err(e) => return Err(e),
                }
            }
        }
        let mut cmd = build_command(&cfg, &java_list)?;
        self.panel(format!("▶ Starte {} …", cfg.name));
        let program = cmd.as_std().get_program().to_string_lossy().to_string();
        let args: Vec<String> = cmd.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect();
        self.panel(format!("$ {} {}", program, args.join(" ")));
        cmd.current_dir(&dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(false);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| AppError::internal(format!("Start fehlgeschlagen ({program}): {e}")))?;
        let pid = child.id();
        {
            let mut i = self.lock();
            i.game = cfg.game;
            i.pid = pid;
            i.started_at = Some(Instant::now());
            i.stop_requested = false;
            i.restart_requested = false;
            i.stats = ServerStats { disk_bytes: i.stats.disk_bytes, ..Default::default() };
        }
        *self.stdin.lock().await = child.stdin.take();
        *self.rcon.lock().await = None;
        self.set_status(ServerStatus::Starting);
        app.events.emit(&self.id, EventKind::ServerStarting, None, format!("{} startet", cfg.name));

        if let Some(out) = child.stdout.take() {
            self.spawn_reader(app.clone(), out, ConsoleSource::Stdout);
        }
        if let Some(err) = child.stderr.take() {
            self.spawn_reader(app.clone(), err, ConsoleSource::Stderr);
        }

        let me = self.clone();
        let app2 = app.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            me.on_exit(&app2, status.ok().and_then(|s| s.code())).await;
        });
        Ok(())
    }

    fn spawn_reader<R: AsyncRead + Unpin + Send + 'static>(self: &Arc<Self>, app: AppState, r: R, source: ConsoleSource) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(r);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let text = clean_line(&String::from_utf8_lossy(&buf));
                        if text.is_empty() {
                            continue;
                        }
                        me.push(source, text.clone());
                        me.handle_line(&app, &text);
                    }
                }
            }
        });
    }

    fn handle_line(&self, app: &AppState, text: &str) {
        let game = self.lock().game;
        let Some(Parsed { kind, player, message }) = events::parse_line(game, text) else {
            return;
        };
        match kind {
            EventKind::ServerStarted => self.set_status(ServerStatus::Running),
            EventKind::PlayerJoin => {
                if let Some(p) = &player {
                    self.lock().players.insert(p.clone());
                }
                self.broadcast_stats();
            }
            EventKind::PlayerLeave => {
                if let Some(p) = &player {
                    self.lock().players.remove(p);
                }
                self.broadcast_stats();
            }
            _ => {}
        }
        let ev = app.events.emit(&self.id, kind, player, message);
        let _ = self.tx.send(WsMessage::Event { event: ev });
    }

    async fn on_exit(self: &Arc<Self>, app: &AppState, code: Option<i32>) {
        *self.stdin.lock().await = None;
        *self.rcon.lock().await = None;
        let (stop_requested, restart_requested) = {
            let mut i = self.lock();
            i.pid = None;
            i.started_at = None;
            (i.stop_requested, i.restart_requested)
        };
        let name = app.store.server(&self.id).await.map(|c| c.name).unwrap_or_default();
        let crashed = !stop_requested && code != Some(0);
        if crashed {
            self.panel(format!("✖ Prozess beendet (Code {}) – abgestürzt", code.map_or("?".into(), |c| c.to_string())));
            self.set_status(ServerStatus::Crashed);
            let ev = app.events.emit(&self.id, EventKind::ServerCrashed, None, format!("{name} ist abgestürzt (Code {code:?})"));
            let _ = self.tx.send(WsMessage::Event { event: ev });
        } else {
            self.panel("■ Server gestoppt");
            self.set_status(ServerStatus::Stopped);
            let ev = app.events.emit(&self.id, EventKind::ServerStopped, None, format!("{name} wurde gestoppt"));
            let _ = self.tx.send(WsMessage::Event { event: ev });
        }
        self.broadcast_stats();

        let auto_restart = app.store.server(&self.id).await.is_some_and(|c| c.auto_restart);
        let should_restart = restart_requested
            || (crashed && auto_restart && {
                let mut i = self.lock();
                i.crashes.retain(|t| t.elapsed() < Duration::from_secs(600));
                i.crashes.push(Instant::now());
                i.crashes.len() <= 3
            });
        if crashed && auto_restart && !should_restart {
            self.panel("Automatischer Neustart ausgesetzt: mehr als 3 Abstürze in 10 Minuten.");
        }
        if should_restart {
            if crashed {
                self.panel("↻ Automatischer Neustart in 5 Sekunden …");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            if let Err(e) = self.start(app).await {
                self.panel(format!("Neustart fehlgeschlagen: {e}"));
            }
        }
    }

    /* ------------------------------ Stopp ------------------------------ */

    /// Sauber stoppen und warten (höchstens 90 s, danach Kill).
    pub async fn stop_and_wait(self: &Arc<Self>, app: &AppState) -> AppResult<()> {
        let (status, game, pid) = {
            let i = self.lock();
            (i.status, i.game, i.pid)
        };
        if !status.is_online() || pid.is_none() {
            return Ok(());
        }
        self.lock().stop_requested = true;
        self.set_status(ServerStatus::Stopping);
        self.panel("■ Stoppe Server …");
        match game {
            Game::Minecraft => {
                if self.write_stdin("stop").await.is_err() {
                    let _ = self.command(app, "stop").await;
                }
            }
            Game::Factorio => signal(pid, nix::sys::signal::Signal::SIGINT),
        }
        let t = Instant::now();
        while self.pid().is_some() {
            if t.elapsed() > STOP_TIMEOUT {
                self.panel("Server reagiert nicht – wird beendet (SIGKILL).");
                self.kill();
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        // auf on_exit warten
        for _ in 0..40 {
            if !self.status().is_online() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Stoppen im Hintergrund.
    pub fn stop(self: &Arc<Self>, app: &AppState) {
        let me = self.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let _ = me.stop_and_wait(&app).await;
        });
    }

    pub async fn restart(self: &Arc<Self>, app: &AppState) -> AppResult<()> {
        if self.status().is_online() {
            self.lock().restart_requested = true;
            self.stop(app);
            Ok(())
        } else {
            self.start(app).await
        }
    }

    pub fn kill(&self) {
        let pid = {
            let mut i = self.lock();
            i.stop_requested = true;
            i.pid
        };
        signal(pid, nix::sys::signal::Signal::SIGKILL);
    }

    /* ---------------------------- Befehle ----------------------------- */

    async fn write_stdin(&self, cmd: &str) -> AppResult<()> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or_else(|| AppError::conflict("Server läuft nicht"))?;
        stdin.write_all(format!("{cmd}\n").as_bytes()).await.map_err(|e| AppError::internal(e.to_string()))?;
        stdin.flush().await.ok();
        Ok(())
    }

    /// RCON-Befehl (nur Antwort, nichts in der Konsole). Fehler, wenn RCON nicht verfügbar.
    pub async fn rcon(&self, app: &AppState, cmd: &str) -> AppResult<String> {
        if !self.status().is_online() {
            return Err(AppError::conflict("Server läuft nicht"));
        }
        let cfg = app.store.server(&self.id).await.ok_or_else(|| AppError::not_found("Server nicht gefunden"))?;
        let rc = cfg.rcon.ok_or_else(|| AppError::conflict("RCON ist nicht eingerichtet"))?;
        let mut guard = self.rcon.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                match RconClient::connect(rc.port, &rc.password).await {
                    Ok(c) => *guard = Some(c),
                    Err(e) => {
                        if attempt == 1 {
                            return Err(AppError::conflict(format!("RCON nicht erreichbar: {e}")));
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        continue;
                    }
                }
            }
            if let Some(c) = guard.as_mut() {
                match c.exec(cmd).await {
                    Ok(out) => return Ok(out),
                    Err(e) => {
                        *guard = None;
                        if attempt == 1 {
                            return Err(AppError::internal(format!("RCON-Fehler: {e}")));
                        }
                    }
                }
            }
        }
        Err(AppError::internal("RCON-Fehler"))
    }

    /// Befehl ausführen: über RCON mit Antwort (wenn bereit), sonst über die Konsole.
    /// Wird in der Konsole angezeigt (inkl. Antwort).
    pub async fn command(&self, app: &AppState, cmd: &str) -> AppResult<Option<String>> {
        let cmd = cmd.trim().trim_start_matches('/').to_string();
        if cmd.is_empty() {
            return Ok(None);
        }
        let game = self.lock().game;
        // Factorio: Chat-/Befehle brauchen führenden Slash, ausser normale Chatnachrichten.
        let rcon_cmd = if game == Game::Factorio { format!("/{cmd}") } else { cmd.clone() };
        if self.status() == ServerStatus::Running {
            if let Ok(out) = self.rcon(app, &rcon_cmd).await {
                self.push(ConsoleSource::Input, format!("> {cmd}"));
                for l in out.lines().filter(|l| !l.trim().is_empty()) {
                    self.push(ConsoleSource::Panel, l.to_string());
                }
                return Ok(Some(out));
            }
        }
        let stdin_cmd = if game == Game::Factorio { format!("/{cmd}") } else { cmd };
        self.write_stdin(&stdin_cmd).await?;
        self.push(ConsoleSource::Input, format!("> {stdin_cmd}"));
        Ok(None)
    }
}

/// Konsolenzeile säubern: Zeilenende, `\r` und Prompt-Reste der JLine-Konsole (`> `, auch nach
/// einem ANSI-Reset wie `\x1b[m> `) entfernen. Farbcodes bleiben erhalten.
pub fn clean_line(raw: &str) -> String {
    let t = raw.trim_end_matches(['\n', '\r']).replace('\r', "");
    let mut rest = t.as_str();
    loop {
        let before = rest.len();
        rest = rest.trim_start_matches(|c: char| c == '>' || c == ' ' || c == '\t');
        if let Some(r) = rest.strip_prefix("\x1b[m").or_else(|| rest.strip_prefix("\x1b[0m")) {
            rest = r;
        }
        if rest.len() == before {
            break;
        }
    }
    rest.to_string()
}

fn signal(pid: Option<u32>, sig: nix::sys::signal::Signal) {
    if let Some(pid) = pid {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), sig);
    }
}

/* -------------------------- Startbefehl bauen -------------------------- */

/// Java für einen Server: fest eingestellt oder automatisch passend zur Version.
pub fn resolve_java(cfg: &ServerConfig, installs: &[nrz_gp_common::JavaInstall]) -> Option<String> {
    if let Some(p) = cfg.java_path.as_ref().filter(|p| !p.is_empty()) {
        return Some(p.clone());
    }
    let req = java::required_for(cfg.version.as_deref(), cfg.flavor);
    java::pick(installs, req).map(|j| j.path.clone())
}

pub fn build_command(cfg: &ServerConfig, installs: &[nrz_gp_common::JavaInstall]) -> AppResult<Command> {
    let dir = Path::new(&cfg.dir);
    let java_cmd = || -> AppResult<Command> {
        let java = resolve_java(cfg, installs).ok_or_else(|| AppError::bad("Keine Java-Installation gefunden"))?;
        let mut c = Command::new(java);
        let mem = cfg.memory_mb.max(512);
        c.arg(format!("-Xmx{mem}M")).arg(format!("-Xms{}M", mem.min(1024)));
        c.args(util::split_args(&cfg.jvm_args));
        Ok(c)
    };
    let mut cmd = match &cfg.launch {
        LaunchKind::Jar { jar } => {
            if !dir.join(jar).is_file() {
                return Err(AppError::bad(format!("Startdatei fehlt: {jar}")));
            }
            let mut c = java_cmd()?;
            c.arg("-jar").arg(jar).arg("nogui");
            c
        }
        LaunchKind::ArgsFile { args_file } => {
            if !dir.join(args_file).is_file() {
                return Err(AppError::bad(format!("Argumentdatei fehlt: {args_file}")));
            }
            let mut c = java_cmd()?;
            if dir.join("user_jvm_args.txt").is_file() {
                c.arg("@user_jvm_args.txt");
            }
            c.arg(format!("@{args_file}")).arg("nogui");
            c
        }
        LaunchKind::Script { script } => {
            if !dir.join(script).is_file() {
                return Err(AppError::bad(format!("Startskript fehlt: {script}")));
            }
            let mut c = Command::new("sh");
            c.arg(script);
            c
        }
        LaunchKind::Factorio { binary, save } => {
            let bin = dir.join(binary);
            if !bin.is_file() {
                return Err(AppError::bad(format!("Factorio-Programm fehlt: {binary}")));
            }
            let mut c = Command::new(bin);
            match save {
                Some(s) => {
                    let file = if s.ends_with(".zip") { s.clone() } else { format!("{s}.zip") };
                    c.arg("--start-server").arg(format!("saves/{file}"));
                }
                None => {
                    c.arg("--start-server-load-latest");
                }
            }
            c.arg("--port").arg(cfg.port.to_string());
            for candidate in ["data/server-settings.json", "server-settings.json", "config/server-settings.json"] {
                if dir.join(candidate).is_file() {
                    c.arg("--server-settings").arg(candidate);
                    break;
                }
            }
            if dir.join("server-adminlist.json").is_file() {
                c.arg("--server-adminlist").arg("server-adminlist.json");
            }
            if let Some(r) = &cfg.rcon {
                c.arg("--rcon-port").arg(r.port.to_string()).arg("--rcon-password").arg(&r.password);
            }
            c
        }
    };
    cmd.args(util::split_args(&cfg.extra_args));
    Ok(cmd)
}

/* ---------------------------- Minecraft-Helfer ---------------------------- */

pub fn eula_accepted(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("eula.txt")).is_ok_and(|s| {
        s.lines().any(|l| {
            let l = l.trim().replace(' ', "");
            l.eq_ignore_ascii_case("eula=true")
        })
    })
}

pub fn accept_eula(dir: &Path) -> std::io::Result<()> {
    std::fs::write(
        dir.join("eula.txt"),
        format!(
            "#By changing the setting below to TRUE you are indicating your agreement to our EULA (https://aka.ms/MinecraftEULA).\n#Akzeptiert über nrzGamePanel am {}\neula=true\n",
            chrono::Local::now().format("%d.%m.%Y %H:%M")
        ),
    )
}

/// Port und RCON aus der Panel-Konfiguration in server.properties übernehmen.
pub fn sync_minecraft_properties(dir: &Path, cfg: &ServerConfig) -> AppResult<()> {
    let path = dir.join("server.properties");
    let mut updates = vec![("server-port".to_string(), cfg.port.to_string())];
    if let Some(r) = &cfg.rcon {
        updates.push(("enable-rcon".into(), "true".into()));
        updates.push(("rcon.port".into(), r.port.to_string()));
        updates.push(("rcon.password".into(), r.password.clone()));
        updates.push(("broadcast-rcon-to-ops".into(), "false".into()));
    }
    util::set_properties(&path, &updates)?;
    Ok(())
}

/// Warnungen für die Oberfläche.
pub fn warnings(cfg: &ServerConfig, installs: &[nrz_gp_common::JavaInstall]) -> Vec<ServerWarning> {
    let mut w = Vec::new();
    let dir = Path::new(&cfg.dir);
    if !dir.is_dir() {
        w.push(ServerWarning::Other { message: format!("Server-Ordner nicht gefunden: {}", cfg.dir) });
        return w;
    }
    match &cfg.launch {
        LaunchKind::Jar { jar } if !dir.join(jar).is_file() => w.push(ServerWarning::LaunchFileMissing { path: jar.clone() }),
        LaunchKind::ArgsFile { args_file } if !dir.join(args_file).is_file() => {
            w.push(ServerWarning::LaunchFileMissing { path: args_file.clone() })
        }
        LaunchKind::Script { script } if !dir.join(script).is_file() => w.push(ServerWarning::LaunchFileMissing { path: script.clone() }),
        LaunchKind::Factorio { binary, .. } if !dir.join(binary).is_file() => {
            w.push(ServerWarning::LaunchFileMissing { path: binary.clone() })
        }
        _ => {}
    }
    if cfg.game == Game::Minecraft {
        if !eula_accepted(dir) {
            w.push(ServerWarning::EulaNotAccepted);
        }
        if !matches!(cfg.launch, LaunchKind::Script { .. }) && resolve_java(cfg, installs).is_none() {
            w.push(ServerWarning::JavaMissing { required: java::required_for(cfg.version.as_deref(), cfg.flavor) });
        } else if cfg.java_path.is_none() {
            let req = java::required_for(cfg.version.as_deref(), cfg.flavor);
            if !installs.iter().any(|j| j.major >= req) {
                w.push(ServerWarning::JavaMissing { required: req });
            }
        }
    }
    if cfg.rcon.is_none() {
        w.push(ServerWarning::RconDisabled);
    }
    w
}

/* ------------------------- Hintergrund-Aufgaben ------------------------- */

static MC_LIST: Lazy<Regex> = Lazy::new(|| Regex::new(r"There are (\d+) (?:of a max of|/) ?(\d+) players online:?(.*)").unwrap());

/// Statistik alle 2 s, Spielerliste per RCON alle 30 s, Ordnergrösse alle 10 min.
pub fn spawn_background(app: AppState) {
    let a = app.clone();
    tokio::spawn(async move {
        let mut tick: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            tick += 1;
            let runtimes: Vec<_> = a.runtimes.read().await.values().cloned().collect();
            let pids: Vec<(Arc<ServerRuntime>, u32)> = runtimes.iter().filter_map(|r| r.pid().map(|p| (r.clone(), p))).collect();
            if !pids.is_empty() {
                let samples: Vec<(f32, u64)> = {
                    let mut sys = a.sys.lock().unwrap_or_else(|e| e.into_inner());
                    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                    pids.iter().map(|(_, pid)| tree_usage(&sys, *pid)).collect()
                };
                for ((rt, _), (cpu, mem)) in pids.iter().zip(samples) {
                    {
                        let mut i = rt.lock();
                        i.stats.cpu = cpu;
                        i.stats.memory_bytes = mem;
                    }
                    rt.broadcast_stats();
                }
            }
            if tick % 15 == 0 {
                for rt in &runtimes {
                    if rt.status() == ServerStatus::Running {
                        let rt = rt.clone();
                        let a = a.clone();
                        tokio::spawn(async move { refresh_players(&a, &rt).await });
                    }
                }
            }
            if tick % 300 == 1 {
                for cfg in a.store.servers().await {
                    let rt = a.runtime(&cfg.id).await;
                    tokio::task::spawn_blocking(move || {
                        let size = util::dir_size(Path::new(&cfg.dir));
                        rt.set_disk(size);
                    });
                }
            }
        }
    });
}

fn tree_usage(sys: &sysinfo::System, root: u32) -> (f32, u64) {
    let root = sysinfo::Pid::from_u32(root);
    let mut cpu = 0.0;
    let mut mem = 0;
    for (pid, p) in sys.processes() {
        let mut cur = Some(*pid);
        let mut depth = 0;
        while let Some(c) = cur {
            if c == root {
                cpu += p.cpu_usage();
                mem += p.memory();
                break;
            }
            depth += 1;
            if depth > 6 {
                break;
            }
            cur = sys.process(c).and_then(|x| x.parent());
        }
    }
    (cpu, mem)
}

/// Spielerliste per RCON aktualisieren.
pub async fn refresh_players(app: &AppState, rt: &Arc<ServerRuntime>) {
    let game = rt.lock().game;
    match game {
        Game::Minecraft => {
            if let Ok(out) = rt.rcon(app, "list").await {
                if let Some(c) = MC_LIST.captures(&out) {
                    if let Ok(max) = c[2].parse() {
                        rt.set_players_max(max);
                    }
                    let names = c[3].split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                    rt.set_players(names);
                    rt.broadcast_stats();
                }
            }
        }
        Game::Factorio => {
            if let Ok(out) = rt.rcon(app, "/players online").await {
                let names = out
                    .lines()
                    .skip(1)
                    .map(|l| l.trim().trim_end_matches("(online)").trim().to_string())
                    .filter(|s| !s.is_empty());
                rt.set_players(names);
                rt.broadcast_stats();
            }
        }
    }
}

/// Beim Beenden des Panels alle Server sauber stoppen.
pub async fn stop_all(app: &AppState) {
    let runtimes: Vec<_> = app.runtimes.read().await.values().cloned().collect();
    let tasks: Vec<_> = runtimes
        .into_iter()
        .filter(|r| r.status().is_online())
        .map(|r| {
            let app = app.clone();
            tokio::spawn(async move { r.stop_and_wait(&app).await })
        })
        .collect();
    for t in tasks {
        let _ = t.await;
    }
}
