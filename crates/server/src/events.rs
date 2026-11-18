//! Ereignisse: Erkennung aus Konsolenzeilen, Verteilung (broadcast) und Verlauf (jsonl pro Server).

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use nrz_gp_common::{EventKind, Game, GameEvent};
use once_cell::sync::Lazy;
use regex::Regex;
use tokio::{io::AsyncWriteExt, sync::broadcast};

use crate::store::now_ms;

const MEMORY: usize = 2000;

pub struct EventBus {
    pub tx: broadcast::Sender<GameEvent>,
    next_id: AtomicU64,
    recent: Mutex<VecDeque<GameEvent>>,
    dir: PathBuf,
}

impl EventBus {
    pub fn new(dir: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(1024);
        let mut recent = VecDeque::new();
        let mut max_id = 0;
        // Verlauf der letzten Ereignisse aller Server laden.
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if let Ok(raw) = std::fs::read_to_string(e.path()) {
                    for line in raw.lines().rev().take(MEMORY / 4) {
                        if let Ok(ev) = serde_json::from_str::<GameEvent>(line) {
                            max_id = max_id.max(ev.id);
                            recent.push_back(ev);
                        }
                    }
                }
            }
        }
        let mut v: Vec<_> = recent.into_iter().collect();
        v.sort_by_key(|e: &GameEvent| e.ts);
        Self { tx, next_id: AtomicU64::new(max_id + 1), recent: Mutex::new(v.into_iter().collect()), dir }
    }

    /// Ereignis erzeugen, verteilen und speichern.
    pub fn emit(&self, server_id: &str, kind: EventKind, player: Option<String>, message: impl Into<String>) -> GameEvent {
        let ev = GameEvent {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            server_id: server_id.to_string(),
            ts: now_ms(),
            kind,
            player,
            message: message.into(),
        };
        {
            let mut r = self.recent.lock().unwrap_or_else(|e| e.into_inner());
            r.push_back(ev.clone());
            while r.len() > MEMORY {
                r.pop_front();
            }
        }
        let _ = self.tx.send(ev.clone());
        let path = self.dir.join(format!("{server_id}.jsonl"));
        let line = serde_json::to_string(&ev).unwrap_or_default();
        tokio::spawn(async move {
            if let Ok(mut f) = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await {
                let _ = f.write_all(format!("{line}\n").as_bytes()).await;
            }
        });
        ev
    }

    /// Neueste zuerst; `server_id` None = alle.
    pub fn recent(&self, server_id: Option<&str>, limit: usize) -> Vec<GameEvent> {
        let r = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        r.iter().rev().filter(|e| server_id.is_none_or(|id| e.server_id == id)).take(limit).cloned().collect()
    }
}

/// Erkanntes Ereignis aus einer Konsolenzeile.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub kind: EventKind,
    pub player: Option<String>,
    pub message: String,
}

static ANSI: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
// `[12:34:56] [Server thread/INFO]: …` bzw. `[12:34:56 INFO]: …` (Paper) – alles nach dem Präfix.
static MC_PREFIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\[[^\]]*?(?:\d{2}:\d{2}:\d{2})[^\]]*\](?: \[([^\]]+)\])?(?: \[[^\]]+\])?: (.*)$").unwrap());
static MC_DONE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^Done \([\d.,]+s\)! For help, type "help""#).unwrap());
static MC_JOIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Za-z0-9_.]{2,17}) joined the game").unwrap());
static MC_LEAVE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Za-z0-9_.]{2,17}) left the game").unwrap());
static MC_CHAT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:\[Not Secure\] )?<([A-Za-z0-9_.]{2,17})> (.*)$").unwrap());
static MC_ADV: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^([A-Za-z0-9_.]{2,17}) has (?:made the advancement|completed the challenge|reached the goal) \[(.+)\]").unwrap()
});
static MC_DEATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^([A-Za-z0-9_.]{2,17}) (was (?:slain|shot|killed|blown up|fireballed|pummeled|squashed|pricked|impaled|stung|skewered|struck|frozen|poked|squished|obliterated|burnt|doomed|knocked)|drowned|died|fell|hit the ground|went up in flames|went off with a bang|burned to death|tried to swim in lava|starved|suffocated|froze to death|withered away|experienced kinetic energy|walked into|blew up|discovered the floor was lava|left the confines|didn't want to live|was roasted|walked into fire|was squashed)(.*)$",
    )
    .unwrap()
});
static FAC_JOIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[JOIN\] (\S+) joined the game").unwrap());
static FAC_LEAVE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[LEAVE\] (\S+) left the game").unwrap());
static FAC_CHAT: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[CHAT\] ([^:]+): (.*)$").unwrap());

pub fn strip_ansi(s: &str) -> String {
    ANSI.replace_all(s, "").to_string()
}

/// Konsolenzeile → Ereignis.
pub fn parse_line(game: Game, raw: &str) -> Option<Parsed> {
    let line = strip_ansi(raw);
    let line = line.trim_end().trim_start_matches(|c: char| c == '>' || c.is_whitespace());
    let p = |kind, player: Option<&str>, message: &str| Some(Parsed { kind, player: player.map(str::to_string), message: message.to_string() });
    match game {
        Game::Minecraft => {
            let caps = MC_PREFIX.captures(line)?;
            let thread = caps.get(1).map_or("", |m| m.as_str());
            let msg = caps.get(2)?.as_str();
            if thread.contains("ERROR") || line.contains(" ERROR]") {
                return p(EventKind::LogError, None, msg);
            }
            if MC_DONE.is_match(msg) {
                return p(EventKind::ServerStarted, None, "Server ist bereit");
            }
            if let Some(c) = MC_CHAT.captures(msg) {
                return p(EventKind::Chat, Some(&c[1]), &c[2]);
            }
            if let Some(c) = MC_JOIN.captures(msg) {
                return p(EventKind::PlayerJoin, Some(&c[1]), msg);
            }
            if let Some(c) = MC_LEAVE.captures(msg) {
                return p(EventKind::PlayerLeave, Some(&c[1]), msg);
            }
            if let Some(c) = MC_ADV.captures(msg) {
                return p(EventKind::Advancement, Some(&c[1]), &c[2]);
            }
            if let Some(c) = MC_DEATH.captures(msg) {
                return p(EventKind::PlayerDeath, Some(&c[1]), msg);
            }
            None
        }
        Game::Factorio => {
            if let Some(c) = FAC_JOIN.captures(line) {
                return p(EventKind::PlayerJoin, Some(&c[1]), &format!("{} joined the game", &c[1]));
            }
            if let Some(c) = FAC_LEAVE.captures(line) {
                return p(EventKind::PlayerLeave, Some(&c[1]), &format!("{} left the game", &c[1]));
            }
            if let Some(c) = FAC_CHAT.captures(line) {
                return p(EventKind::Chat, Some(c[1].trim()), &c[2]);
            }
            if line.contains("changing state from(CreatingGame) to(InGame)") {
                return p(EventKind::ServerStarted, None, "Server ist bereit");
            }
            if line.contains("Error") && line.contains(" Error ") {
                return p(EventKind::LogError, None, line);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minecraft_lines() {
        let j = parse_line(Game::Minecraft, "[12:00:01] [Server thread/INFO]: Steve joined the game").unwrap();
        assert_eq!(j.kind, EventKind::PlayerJoin);
        assert_eq!(j.player.as_deref(), Some("Steve"));
        let c = parse_line(Game::Minecraft, "[12:00:01 INFO]: <Alex> hallo zusammen").unwrap();
        assert_eq!(c.kind, EventKind::Chat);
        assert_eq!(c.message, "hallo zusammen");
        let d = parse_line(Game::Minecraft, "[12:00:01] [Server thread/INFO] [minecraft/DedicatedServer]: Done (3.21s)! For help, type \"help\"").unwrap();
        assert_eq!(d.kind, EventKind::ServerStarted);
        let x = parse_line(Game::Minecraft, "[12:00:01] [Server thread/INFO]: Steve was slain by Zombie").unwrap();
        assert_eq!(x.kind, EventKind::PlayerDeath);
        let a = parse_line(Game::Minecraft, "[12:00:01] [Server thread/INFO]: Steve has made the advancement [Stone Age]").unwrap();
        assert_eq!(a.message, "Stone Age");
    }

    #[test]
    fn prompt_residue() {
        let raw = crate::process::clean_line("\x1b[m>   [02:58:39 INFO]: Done (9.554s)! For help, type \"help\"\r\n");
        assert!(raw.starts_with("[02:58:39"));
        assert_eq!(parse_line(Game::Minecraft, &raw).unwrap().kind, EventKind::ServerStarted);
        assert_eq!(parse_line(Game::Minecraft, "  [02:58:39 INFO]: Steve joined the game").unwrap().kind, EventKind::PlayerJoin);
        let warn = crate::process::clean_line("\x1b[m>   \x1b[33;1m[02:58:35 WARN]: Beware.");
        assert!(warn.starts_with("\x1b[33;1m["));
    }

    #[test]
    fn factorio_lines() {
        let j = parse_line(Game::Factorio, "2026-11-15 12:00:00 [JOIN] nico joined the game").unwrap();
        assert_eq!(j.kind, EventKind::PlayerJoin);
        let c = parse_line(Game::Factorio, "2026-11-15 12:00:00 [CHAT] nico: hi").unwrap();
        assert_eq!(c.message, "hi");
    }
}
