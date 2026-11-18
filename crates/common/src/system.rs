//! Panel und Host: Systemwerte, Java-Installationen, Versionen.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JavaInstall {
    pub path: String,
    /// Hauptversion, z. B. 21.
    pub major: u32,
    /// Volle Version, z. B. `21.0.5`.
    pub version: String,
    pub vendor: Option<String>,
}

/// `GET /api/system`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SystemInfo {
    pub panel_version: String,
    pub hostname: String,
    pub os: String,
    pub cpu_count: u32,
    /// Auslastung gesamt in Prozent.
    pub cpu_usage: f32,
    pub memory_total: u64,
    pub memory_used: u64,
    pub disk_total: u64,
    pub disk_free: u64,
    pub data_dir: String,
    pub java: Vec<JavaInstall>,
    pub import_roots: Vec<String>,
    /// Adresse, unter der Spieler verbinden (aus `NRZ_GP_PUBLIC_HOST` oder Hostname).
    pub public_host: String,
}

/// `GET /api/versions/{game}/{flavor}` – Versionen, neueste zuerst.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VersionList {
    pub versions: Vec<VersionEntry>,
    /// Loader-Versionen (Fabric/Forge/NeoForge) für die gewählte Spielversion; leer sonst.
    pub loader_versions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VersionEntry {
    pub id: String,
    pub stable: bool,
}

/// `POST /api/check-port` – ist ein Port frei?
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PortCheck {
    pub port: u16,
    pub free: bool,
    /// Belegt durch diesen Panel-Server (Name).
    pub used_by: Option<String>,
}
