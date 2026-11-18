//! nrzGamePanel – Backend.
//!
//! Läuft im Docker-Host-Modus (`network_mode: host`), damit die Spiel-Ports (Minecraft TCP,
//! Factorio UDP) direkt auf dem Host erreichbar sind, ohne Port-Weiterleitungen.

mod api;
mod auth;
mod automations;
mod backups;
mod config;
mod error;
mod events;
mod factorio;
mod files;
mod import;
mod install;
mod java;
mod mcp;
mod minecraft;
mod process;
mod rcon;
mod servers;
mod state;
mod store;
mod util;

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;

use crate::{config::Config, events::EventBus, state::AppInner, store::Store};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")))
        .init();

    let config = Config::from_env();
    for d in [config.data_dir.clone(), config.servers_dir(), config.backups_dir(), config.cache_dir(), config.events_dir()] {
        std::fs::create_dir_all(&d)?;
    }
    let config = Config { data_dir: std::fs::canonicalize(&config.data_dir)?, ..config };
    let store = Store::load(config.data_dir.join("state.json"))?;
    let java = java::detect(&[java::java_dir(&config.data_dir)]);
    tracing::info!("Java gefunden: {}", java.iter().map(|j| format!("{} ({})", j.major, j.path)).collect::<Vec<_>>().join(", "));

    let app = Arc::new(AppInner {
        events: EventBus::new(config.events_dir()),
        http: reqwest::Client::builder()
            .user_agent(state::USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .build()?,
        config,
        store,
        runtimes: RwLock::new(HashMap::new()),
        java: RwLock::new(java),
        sys: std::sync::Mutex::new(sysinfo::System::new()),
    });

    // Laufzeiten anlegen + Autostart
    for cfg in app.store.servers().await {
        let rt = app.runtime(&cfg.id).await;
        if cfg.auto_start {
            if let Err(e) = rt.start(&app).await {
                tracing::warn!("Autostart von {} fehlgeschlagen: {e}", cfg.name);
            }
        }
    }
    process::spawn_background(app.clone());
    automations::spawn_scheduler(app.clone());

    let listener = tokio::net::TcpListener::bind(&app.config.bind).await?;
    tracing::info!("nrzGamePanel {} läuft auf http://{}", nrz_gp_common::PANEL_VERSION, app.config.bind);
    let router = api::router(app.clone());
    let app2 = app.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("Beende: stoppe laufende Server sauber …");
            process::stop_all(&app2).await;
        })
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
}
