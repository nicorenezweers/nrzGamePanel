//! Gemeinsamer Zustand aller Routen.

use std::{collections::HashMap, sync::Arc};

use nrz_gp_common::{JavaInstall, ServerConfig};
use tokio::sync::RwLock;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    events::EventBus,
    process::ServerRuntime,
    store::Store,
};

pub struct AppInner {
    pub config: Config,
    pub store: Store,
    /// Laufzeitdaten je Server-ID (wird beim Start für jeden gespeicherten Server angelegt).
    pub runtimes: RwLock<HashMap<String, Arc<ServerRuntime>>>,
    pub events: EventBus,
    /// HTTP-Client für Downloads (Mojang, PaperMC, Modrinth, Factorio …).
    pub http: reqwest::Client,
    /// Gefundene Java-Installationen (beim Start und auf Anfrage neu gesucht).
    pub java: RwLock<Vec<JavaInstall>>,
    pub sys: std::sync::Mutex<sysinfo::System>,
}

pub type AppState = Arc<AppInner>;

pub const USER_AGENT: &str = concat!("nrzGamePanel/", env!("CARGO_PKG_VERSION"), " (self-hosted game server panel)");

impl AppInner {
    /// Laufzeit eines Servers (legt sie an, falls noch nicht vorhanden).
    pub async fn runtime(&self, id: &str) -> Arc<ServerRuntime> {
        if let Some(r) = self.runtimes.read().await.get(id) {
            return r.clone();
        }
        let mut w = self.runtimes.write().await;
        w.entry(id.to_string()).or_insert_with(|| Arc::new(ServerRuntime::new(id))).clone()
    }

    /// Konfiguration + Laufzeit, 404 wenn unbekannt.
    pub async fn server(&self, id: &str) -> AppResult<(ServerConfig, Arc<ServerRuntime>)> {
        let cfg = self.store.server(id).await.ok_or_else(|| AppError::not_found("Server nicht gefunden"))?;
        Ok((cfg, self.runtime(id).await))
    }

    /// Konfiguration eines Servers ändern und speichern.
    pub async fn update_server(&self, id: &str, f: impl FnOnce(&mut ServerConfig)) -> AppResult<ServerConfig> {
        let r = self
            .store
            .update(|s| {
                s.servers.iter_mut().find(|c| c.id == id).map(|c| {
                    f(c);
                    c.clone()
                })
            })
            .await?;
        r.ok_or_else(|| AppError::not_found("Server nicht gefunden"))
    }
}
