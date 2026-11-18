//! Ereignisse: aus der Konsole erkannt (Beitritt, Chat, Tod …) oder vom Panel ausgelöst.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ServerStarting,
    ServerStarted,
    ServerStopped,
    ServerCrashed,
    PlayerJoin,
    PlayerLeave,
    Chat,
    PlayerDeath,
    Advancement,
    BackupCreated,
    /// Warnung/Fehler im Log (`WARN`/`ERROR`).
    LogError,
    /// Automatisierung ausgeführt.
    AutomationRun,
}

impl EventKind {
    pub const ALL: &'static [EventKind] = &[
        Self::ServerStarting,
        Self::ServerStarted,
        Self::ServerStopped,
        Self::ServerCrashed,
        Self::PlayerJoin,
        Self::PlayerLeave,
        Self::Chat,
        Self::PlayerDeath,
        Self::Advancement,
        Self::BackupCreated,
        Self::LogError,
        Self::AutomationRun,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ServerStarting => "Server startet",
            Self::ServerStarted => "Server bereit",
            Self::ServerStopped => "Server gestoppt",
            Self::ServerCrashed => "Server abgestürzt",
            Self::PlayerJoin => "Spieler beigetreten",
            Self::PlayerLeave => "Spieler gegangen",
            Self::Chat => "Chat-Nachricht",
            Self::PlayerDeath => "Spieler gestorben",
            Self::Advancement => "Fortschritt erzielt",
            Self::BackupCreated => "Backup erstellt",
            Self::LogError => "Fehler im Log",
            Self::AutomationRun => "Automatisierung ausgeführt",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GameEvent {
    pub id: u64,
    pub server_id: String,
    /// Unix-Zeit in Millisekunden.
    pub ts: i64,
    pub kind: EventKind,
    /// Betroffener Spieler (falls vorhanden).
    pub player: Option<String>,
    /// Lesbarer Text, z. B. Chat-Nachricht oder Todesmeldung.
    pub message: String,
}
