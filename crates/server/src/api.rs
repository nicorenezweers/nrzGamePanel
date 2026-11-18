//! Router: alle Routen zusammensetzen + Frontend (SPA) ausliefern.

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post, put},
};
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
};

use crate::{auth, automations, backups, factorio, files, import, install, mcp, minecraft, servers, state::AppState};

pub fn router(app: AppState) -> Router {
    let api = Router::new()
        // Auth
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/setup", post(auth::setup))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/tokens", get(auth::list_tokens).post(auth::create_token))
        .route("/api/tokens/{tid}", axum::routing::delete(auth::delete_token))
        // System
        .route("/api/system", get(servers::system))
        .route("/api/check-port", get(servers::check_port))
        .route("/api/events", get(servers::all_events))
        // Server
        .route("/api/servers", get(servers::list).post(servers::create))
        .route("/api/servers/{id}", get(servers::get).patch(servers::update).delete(servers::delete))
        .route("/api/servers/{id}/power", post(servers::power))
        .route("/api/servers/{id}/command", post(servers::command))
        .route("/api/servers/{id}/eula", post(servers::accept_eula))
        .route("/api/servers/{id}/rcon/enable", post(servers::enable_rcon))
        .route("/api/servers/{id}/ws", get(servers::ws))
        .route("/api/servers/{id}/console", get(servers::console))
        .route("/api/servers/{id}/events", get(servers::server_events))
        // Dateien
        .route("/api/servers/{id}/files", get(files::list))
        .route("/api/servers/{id}/files/content", get(files::read).put(files::write))
        .route("/api/servers/{id}/files/download", get(files::download))
        .route("/api/servers/{id}/files/upload", post(files::upload))
        .route("/api/servers/{id}/files/mkdir", post(files::mkdir))
        .route("/api/servers/{id}/files/touch", post(files::touch))
        .route("/api/servers/{id}/files/rename", post(files::rename))
        .route("/api/servers/{id}/files/delete", post(files::delete))
        .route("/api/servers/{id}/files/extract", post(files::extract))
        .route("/api/servers/{id}/files/archive", post(files::archive))
        // Backups
        .route("/api/servers/{id}/backups", get(backups::list).post(backups::create))
        .route("/api/servers/{id}/backups/{name}", axum::routing::delete(backups::delete))
        .route("/api/servers/{id}/backups/{name}/restore", post(backups::restore))
        .route("/api/servers/{id}/backups/{name}/download", get(backups::download))
        // Automatisierungen
        .route("/api/servers/{id}/automations", get(automations::list).post(automations::create))
        .route("/api/servers/{id}/automations/{aid}", put(automations::update).delete(automations::delete))
        .route("/api/servers/{id}/automations/{aid}/run", post(automations::run_now))
        // MCP
        .route("/mcp", post(mcp::handle).get(mcp::get_info))
        .merge(minecraft::router())
        .merge(factorio::router())
        .merge(import::router())
        .merge(install::router())
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024 * 1024))
        .with_state(app.clone());

    let web = app.config.web_dir.clone();
    let spa = ServeDir::new(&web).fallback(ServeFile::new(web.join("index.html")));
    api.fallback_service(spa).layer(CompressionLayer::new())
}
