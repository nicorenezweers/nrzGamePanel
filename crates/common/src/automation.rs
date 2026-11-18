//! Automatisierungen: Auslöser (Ereignis, Intervall, täglich) → Aktionen.

use serde::{Deserialize, Serialize};

use crate::events::EventKind;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// Bei einem Ereignis. `filter`: optionaler Text, der in Spieler/Meldung vorkommen muss.
    Event { event: EventKind, filter: Option<String> },
    /// Alle `minutes` Minuten (solange der Server läuft, ausser bei Start/Backup-Aktionen).
    Interval { minutes: u32 },
    /// Täglich um `hour:minute` (Ortszeit des Panels).
    Daily { hour: u8, minute: u8 },
}

impl Default for Trigger {
    fn default() -> Self {
        Self::Daily { hour: 4, minute: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Konsolenbefehl. Platzhalter: `{player}`, `{message}`, `{server}`.
    Command { command: String },
    /// Nachricht an alle (Minecraft `say`, Factorio Chat).
    Broadcast { message: String },
    Start,
    Stop,
    Restart,
    Backup { worlds_only: bool },
    /// HTTP-POST mit JSON. `discord`: Format für Discord-Webhooks (`{"content": …}`).
    Webhook { url: String, discord: bool },
    /// Warten (zwischen Aktionen), in Sekunden.
    Delay { seconds: u32 },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Automation {
    pub id: String,
    pub server_id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    pub actions: Vec<Action>,
    pub last_run: Option<i64>,
}

/// `POST /api/servers/{id}/automations` und `PUT …/{aid}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SaveAutomation {
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    pub actions: Vec<Action>,
}
