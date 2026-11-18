//! Konfiguration über Umgebungsvariablen.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// `NRZ_GP_BIND` (Standard `0.0.0.0:8700`). Im Docker-Host-Modus direkt auf dem Host.
    pub bind: String,
    /// `NRZ_GP_DATA` (Standard `./data`): state.json, servers/, backups/, cache/, events/.
    pub data_dir: PathBuf,
    /// `NRZ_GP_WEB` (Standard `./public`): gebautes Dioxus-Frontend.
    pub web_dir: PathBuf,
    /// `NRZ_GP_IMPORT_ROOTS` (Komma-getrennt): Startordner für Import und Suche.
    pub import_roots: Vec<PathBuf>,
    /// `NRZ_GP_PUBLIC_HOST`: Adresse für „Verbinden mit …“.
    pub public_host: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let data_dir = PathBuf::from(std::env::var("NRZ_GP_DATA").unwrap_or_else(|_| "./data".into()));
        let data_dir = std::fs::canonicalize(&data_dir).unwrap_or(data_dir);
        let import_roots = std::env::var("NRZ_GP_IMPORT_ROOTS")
            .ok()
            .map(|s| s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).collect())
            .unwrap_or_else(default_import_roots);
        Self {
            bind: std::env::var("NRZ_GP_BIND").unwrap_or_else(|_| "0.0.0.0:8700".into()),
            data_dir,
            web_dir: PathBuf::from(std::env::var("NRZ_GP_WEB").unwrap_or_else(|_| "./public".into())),
            import_roots,
            public_host: std::env::var("NRZ_GP_PUBLIC_HOST").ok().filter(|s| !s.is_empty()),
        }
    }

    pub fn servers_dir(&self) -> PathBuf {
        self.data_dir.join("servers")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }
    pub fn events_dir(&self) -> PathBuf {
        self.data_dir.join("events")
    }
}

fn default_import_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["/import", "/srv", "/opt", "/home", "/root", "/mnt", "/data"]
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if home.is_dir() && !v.iter().any(|p| home.starts_with(p)) {
            v.push(home);
        }
    }
    v
}
