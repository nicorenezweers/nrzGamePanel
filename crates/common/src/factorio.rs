//! Factorio-Integration: Spielstände, Mods, server-settings.json, Admins.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FactorioSave {
    /// Dateiname ohne `.zip`.
    pub name: String,
    pub size: u64,
    pub modified: i64,
    /// Wird beim nächsten Start geladen.
    pub active: bool,
}

/// `GET /api/servers/{id}/factorio/saves`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FactorioSaves {
    pub saves: Vec<FactorioSave>,
    /// None = neuester Spielstand wird geladen.
    pub selected: Option<String>,
}

/// `POST /api/servers/{id}/factorio/saves` (neu erstellen) und `…/saves/select`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SaveName {
    /// None bei `select` = neuester.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FactorioMod {
    pub name: String,
    pub enabled: bool,
    /// Datei im mods-Ordner (None bei `base` und eingebauten DLCs).
    pub file: Option<String>,
    pub version: Option<String>,
}

/// `GET /api/servers/{id}/factorio/mods`, Umschalten per `POST …/mods/toggle` ([`crate::AddonToggle`] mit `file` = Mod-Name).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FactorioMods {
    pub mods: Vec<FactorioMod>,
}

/// `GET/PUT /api/servers/{id}/factorio/settings` – Inhalt von server-settings.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FactorioSettings {
    pub settings: serde_json::Value,
}

/// `GET/PUT /api/servers/{id}/factorio/admins` – server-adminlist.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NameList {
    pub names: Vec<String>,
}
