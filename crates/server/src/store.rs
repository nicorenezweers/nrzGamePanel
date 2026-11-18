//! Dauerhafter Zustand in `<data>/state.json` (atomar geschrieben).

use std::{collections::HashMap, path::PathBuf};

use nrz_gp_common::{Automation, ServerConfig};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct User {
    pub username: String,
    pub password_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub username: String,
    pub expires: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub id: String,
    pub name: String,
    pub prefix: String,
    /// SHA-256 (hex) des Klartexts.
    pub hash: String,
    pub created_at: i64,
    pub last_used: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub users: Vec<User>,
    /// Sitzungstoken (SHA-256 hex) → Sitzung.
    #[serde(default)]
    pub sessions: HashMap<String, Session>,
    #[serde(default)]
    pub tokens: Vec<StoredToken>,
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub automations: Vec<Automation>,
}

pub struct Store {
    path: PathBuf,
    state: RwLock<State>,
}

impl Store {
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let state = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str(&raw)?
        } else {
            State::default()
        };
        Ok(Self { path, state: RwLock::new(state) })
    }

    /// Lesezugriff (Kopie der benötigten Daten herausnehmen, Sperre kurz halten).
    pub async fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        let s = self.state.read().await;
        f(&s)
    }

    /// Ändern und sofort speichern.
    pub async fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> anyhow::Result<R> {
        let mut s = self.state.write().await;
        let r = f(&mut s);
        let json = serde_json::to_vec_pretty(&*s)?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(r)
    }

    pub async fn server(&self, id: &str) -> Option<ServerConfig> {
        self.read(|s| s.servers.iter().find(|c| c.id == id).cloned()).await
    }

    pub async fn servers(&self) -> Vec<ServerConfig> {
        self.read(|s| s.servers.clone()).await
    }
}

/// Unix-Zeit in Sekunden.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Unix-Zeit in Millisekunden.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Zufällige ID (hex), z. B. für Server und Automatisierungen.
pub fn new_id(bytes: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// Ordnername aus einem Anzeigenamen: `Mein Server!` → `mein-server`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        match c {
            'a'..='z' | '0'..='9' => out.push(c),
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            _ if !out.ends_with('-') && !out.is_empty() => out.push('-'),
            _ => {}
        }
    }
    let s = out.trim_end_matches('-').to_string();
    if s.is_empty() { "server".into() } else { s }
}
