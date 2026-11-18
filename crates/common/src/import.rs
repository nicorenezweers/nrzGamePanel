//! Import bestehender Server: Analyse eines Ordners mit automatischer Erkennung von Spiel,
//! Software, Version, Startdatei, Java, Port, RAM und Welten.

use serde::{Deserialize, Serialize};

use crate::server::{Flavor, Game, LaunchKind};

/// Wie sicher die Erkennung ist.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    #[default]
    Low,
    Medium,
    High,
}

/// Eine mögliche Startdatei mit Begründung (für die Auswahl, falls mehrere gefunden).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LaunchCandidate {
    pub launch: LaunchKind,
    pub flavor: Flavor,
    /// z. B. „Main-Class io.papermc.paperclip.Main“ oder „in start.sh referenziert“.
    pub reason: String,
    /// Höher = besser.
    pub score: i32,
}

/// `POST /api/import/analyze` → Ergebnis.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImportAnalysis {
    pub path: String,
    /// Vorgeschlagener Name (Ordnername bzw. MOTD).
    pub suggested_name: String,
    pub game: Option<Game>,
    pub flavor: Flavor,
    pub version: Option<String>,
    pub loader_version: Option<String>,
    /// Woher die Version stammt (z. B. „logs/latest.log“).
    pub version_source: Option<String>,
    /// Beste Startdatei.
    pub launch: Option<LaunchKind>,
    /// Alle gefundenen Möglichkeiten, beste zuerst.
    pub candidates: Vec<LaunchCandidate>,
    pub confidence: Confidence,
    /// Benötigte Java-Hauptversion (z. B. 21).
    pub java_required: Option<u32>,
    /// Automatisch gewähltes Java.
    pub java_path: Option<String>,
    pub memory_mb: Option<u32>,
    /// Zusätzliche JVM-Argumente aus Startskripten (ohne -Xmx/-Xms/-jar).
    pub jvm_args: Option<String>,
    pub port: Option<u16>,
    pub worlds: Vec<String>,
    pub eula_accepted: Option<bool>,
    pub has_rcon: bool,
    /// Plugins- bzw. Mods-Anzahl.
    pub plugin_count: u32,
    pub mod_count: u32,
    /// Ordner ist bereits als Server im Panel.
    pub already_imported: Option<String>,
    /// Hinweise für den Benutzer.
    pub notes: Vec<String>,
}

/// `POST /api/import/analyze`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnalyzeRequest {
    pub path: String,
}

/// `POST /api/import/scan` – sucht Server unter den Import-Wurzeln (oder `root`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ScanRequest {
    pub root: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Ordner bleibt wo er ist (Standard).
    #[default]
    InPlace,
    /// Ordner wird in den Datenordner des Panels kopiert.
    Copy,
}

/// `POST /api/import` → `ServerSummary`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImportRequest {
    pub path: String,
    pub name: String,
    pub mode: ImportMode,
    /// Überschreibungen; None = Erkennung übernehmen.
    pub game: Option<Game>,
    pub flavor: Option<Flavor>,
    pub version: Option<String>,
    pub launch: Option<LaunchKind>,
    pub java_path: Option<String>,
    pub memory_mb: Option<u32>,
    pub port: Option<u16>,
    /// RCON automatisch einrichten (empfohlen, für Live-Daten).
    pub enable_rcon: bool,
    /// EULA akzeptieren (falls noch nicht).
    pub accept_eula: bool,
    pub auto_start: bool,
}
