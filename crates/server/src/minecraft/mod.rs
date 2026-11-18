//! Minecraft-Integration: server.properties, Spieler (inkl. Inventar + Endertruhe aus NBT bzw. live
//! per RCON), Welten-Karte (Region-Dateien → PNG-Kacheln), Texturen, Plugins/Mods, Modrinth.
//!
//! Routen siehe `nrz_gp_common::routes` (alle `/api/servers/{id}/mc/*` und `/api/mc/head/{player}`).

mod addons;
mod assets;
mod map;
mod nbt;
mod players;
mod worlds;

use std::{path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use nrz_gp_common::{
    CommandResponse, Game, InventoryOp, MapPlayer, Ok as OkResp, PlayerAction, PlayerDetail, PlayersOverview, Properties,
    ServerConfig, ServerStatus, ToggleRequest,
};

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    process::ServerRuntime,
    state::AppState,
    util,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/servers/{id}/mc/properties", get(get_properties).put(put_properties))
        .route("/api/servers/{id}/mc/players", get(players::h_overview))
        .route("/api/servers/{id}/mc/players/{player}", get(players::h_detail))
        .route("/api/servers/{id}/mc/players/{player}/inventory", post(players::h_inventory))
        .route("/api/servers/{id}/mc/players/{player}/action", post(players::h_action))
        .route("/api/servers/{id}/mc/whitelist", post(toggle_whitelist))
        .route("/api/servers/{id}/mc/worlds", get(worlds::h_worlds))
        .route("/api/servers/{id}/mc/map/players", get(players::h_map_players))
        .route("/api/servers/{id}/mc/map/{world}/regions", get(worlds::h_regions))
        .route("/api/servers/{id}/mc/map/{world}/tile/{rx}/{rz}", get(map::h_tile))
        .route("/api/servers/{id}/mc/items", get(assets::h_items))
        .route("/api/servers/{id}/mc/texture/{item}", get(assets::h_texture))
        .route("/api/mc/head/{player}", get(assets::h_head))
        .route("/api/servers/{id}/mc/addons", get(addons::h_list))
        .route("/api/servers/{id}/mc/addons/toggle", post(addons::h_toggle))
        .route("/api/servers/{id}/mc/addons/{file}", delete(addons::h_delete))
        .route("/api/servers/{id}/mc/modrinth/search", get(addons::h_search))
        .route("/api/servers/{id}/mc/modrinth/install", post(addons::h_install))
}

/* ------------------------------ öffentliche API (MCP) ------------------------------ */

/// Spielerübersicht (online, bekannt, OPs, Whitelist, Banns).
pub async fn players_overview(app: &AppState, id: &str) -> AppResult<PlayersOverview> {
    players::overview(app, id).await
}

/// Spielerdetails inkl. Inventar und Endertruhe. `player`: UUID oder Name.
pub async fn player_detail(app: &AppState, id: &str, player: &str) -> AppResult<PlayerDetail> {
    players::detail(app, id, player).await
}

/// Inventar/Endertruhe ändern (online per Befehl, offline in der NBT-Datei).
pub async fn inventory_op(app: &AppState, id: &str, player: &str, op: InventoryOp) -> AppResult<PlayerDetail> {
    players::inventory(app, id, player, op).await
}

/// Spieleraktion (kick, ban, op, gamemode, teleport …).
pub async fn player_action(app: &AppState, id: &str, name: &str, action: PlayerAction) -> AppResult<CommandResponse> {
    players::action(app, id, name, action).await
}

/// Positionen aller Spieler (online live, sonst aus playerdata).
pub async fn map_players(app: &AppState, id: &str) -> AppResult<Vec<MapPlayer>> {
    players::map_players(app, id).await
}

/* ------------------------------ gemeinsamer Kontext ------------------------------ */

pub(crate) struct Ctx {
    pub cfg: ServerConfig,
    pub rt: Arc<ServerRuntime>,
    pub dir: PathBuf,
}

pub(crate) async fn ctx(app: &AppState, id: &str) -> AppResult<Ctx> {
    let (cfg, rt) = app.server(id).await?;
    if cfg.game != Game::Minecraft {
        return Err(AppError::bad("Nur für Minecraft-Server"));
    }
    let dir = PathBuf::from(&cfg.dir);
    Ok(Ctx { cfg, rt, dir })
}

impl Ctx {
    pub fn props_path(&self) -> PathBuf {
        self.dir.join("server.properties")
    }
    pub fn props(&self) -> Vec<(String, String)> {
        util::read_properties(&self.props_path())
    }
    pub fn prop(&self, key: &str) -> Option<String> {
        util::value(&self.props(), key).map(str::to_string)
    }
    /// Aktive Welt (`level-name`, Standard `world`).
    pub fn level_name(&self) -> String {
        self.prop("level-name").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "world".into())
    }
    pub fn world_dir(&self) -> PathBuf {
        let name = self.level_name();
        util::safe_join(&self.dir, &name).unwrap_or_else(|_| self.dir.join("world"))
    }
    pub fn running(&self) -> bool {
        self.rt.status() == ServerStatus::Running
    }
    /// RCON grundsätzlich nutzbar (Server läuft, RCON eingerichtet).
    pub fn rcon_possible(&self) -> bool {
        self.running() && self.cfg.rcon.is_some()
    }
    /// Ist der Spieler online (laut letzter Spielerliste)?
    pub fn is_online(&self, name: &str) -> bool {
        self.running() && self.rt.players().iter().any(|p| p.eq_ignore_ascii_case(name))
    }
}

/* ------------------------------ properties ------------------------------ */

async fn get_properties(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<Properties>> {
    let c = ctx(&app, &id).await?;
    let p = c.props_path();
    let entries = tokio::task::spawn_blocking(move || util::read_properties(&p)).await?;
    Ok(Json(Properties { entries }))
}

async fn put_properties(
    State(app): State<AppState>,
    _a: Authed,
    AxPath(id): AxPath<String>,
    Json(req): Json<Properties>,
) -> AppResult<Json<OkResp>> {
    let c = ctx(&app, &id).await?;
    for (k, v) in &req.entries {
        let k = k.trim();
        if k.is_empty() || k.contains(['=', ':', '\n', '\r', '#']) || k.contains(char::is_whitespace) {
            return Err(AppError::bad(format!("Ungültiger Schlüssel „{k}“")));
        }
        if v.contains(['\r']) {
            return Err(AppError::bad(format!("Ungültiger Wert für „{k}“")));
        }
    }
    let entries: Vec<(String, String)> = req.entries.into_iter().map(|(k, v)| (k.trim().to_string(), v)).collect();
    let p = c.props_path();
    tokio::task::spawn_blocking(move || util::set_properties(&p, &entries)).await??;
    Ok(Json(OkResp { ok: true }))
}

async fn toggle_whitelist(
    State(app): State<AppState>,
    _a: Authed,
    AxPath(id): AxPath<String>,
    Json(req): Json<ToggleRequest>,
) -> AppResult<Json<OkResp>> {
    let c = ctx(&app, &id).await?;
    let p = c.props_path();
    let val = req.enabled.to_string();
    tokio::task::spawn_blocking(move || util::set_properties(&p, &[("white-list".to_string(), val)])).await??;
    if c.rt.status().is_online() {
        c.rt.command(&app, if req.enabled { "whitelist on" } else { "whitelist off" }).await?;
    }
    Ok(Json(OkResp { ok: true }))
}

/* ------------------------------ Hilfen ------------------------------ */

/// Unix-Zeit (Sekunden) der letzten Änderung.
pub(crate) fn mtime_secs(m: &std::fs::Metadata) -> Option<i64> {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64)
}

/// HTTP-Datum (RFC 7231).
pub(crate) fn http_date(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default().format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// PNG-Antwort mit Cache-Headern; 304 bei passendem ETag.
pub(crate) fn png_response(bytes: Vec<u8>, cache_control: &'static str, mtime: Option<i64>, headers: &HeaderMap) -> Response {
    let etag = mtime.map(|t| format!("\"{t:x}-{:x}\"", bytes.len()));
    if let (Some(tag), Some(inm)) = (&etag, headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok())) {
        if inm.split(',').any(|t| t.trim() == tag) {
            let mut r = StatusCode::NOT_MODIFIED.into_response();
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
            if let Ok(v) = HeaderValue::from_str(tag) {
                r.headers_mut().insert(header::ETAG, v);
            }
            return r;
        }
    }
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
    if let Some(tag) = etag.and_then(|t| HeaderValue::from_str(&t).ok()) {
        h.insert(header::ETAG, tag);
    }
    if let Some(v) = mtime.and_then(|t| HeaderValue::from_str(&http_date(t)).ok()) {
        h.insert(header::LAST_MODIFIED, v);
    }
    r
}

/// Einzelner, sicherer Dateiname/Ordnername (kein `/`, `\`, `..`).
pub(crate) fn safe_name(s: &str) -> bool {
    !s.is_empty() && s != "." && !s.contains(['/', '\\', '\0']) && !s.contains("..")
}

#[cfg(test)]
mod tests {
    #[test]
    fn router_builds_without_conflicts() {
        let _ = super::router();
        assert!(super::safe_name("world"));
        assert!(!super::safe_name("../x"));
    }
}
