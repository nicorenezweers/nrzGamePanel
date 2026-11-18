//! MCP-Server (Model Context Protocol, Streamable HTTP, JSON-RPC 2.0) unter `POST /mcp`.
//! Anmeldung mit API-Token: `Authorization: Bearer nrzgp_…`.

use std::path::PathBuf;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use nrz_gp_common::{Game, InventoryOp, PlayerAction};
use serde_json::{Value, json};

use crate::{
    auth::authenticate,
    backups,
    error::{AppError, AppResult},
    files::PathQuery,
    minecraft, servers,
    state::AppState,
    util::{rel_path, safe_join},
};

const PROTOCOL: &str = "2025-06-18";

pub async fn get_info() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, "nrzGamePanel MCP: POST JSON-RPC an /mcp (Bearer-Token)").into_response()
}

pub async fn handle(State(app): State<AppState>, headers: HeaderMap, Json(req): Json<Value>) -> Response {
    if authenticate(&app, &headers, None).await.is_none() {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "API-Token fehlt oder ungültig"}))).into_response();
    }
    // Batch oder Einzelanfrage
    if let Value::Array(items) = req {
        let mut out = Vec::new();
        for item in items {
            if let Some(r) = dispatch(&app, item).await {
                out.push(r);
            }
        }
        return Json(Value::Array(out)).into_response();
    }
    match dispatch(&app, req).await {
        Some(r) => Json(r).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn dispatch(app: &AppState, req: Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    // Benachrichtigungen (ohne id) bekommen keine Antwort.
    let id = id?;
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "nrzGamePanel", "version": nrz_gp_common::PANEL_VERSION },
            "instructions": "Steuert Minecraft- und Factorio-Server im nrzGamePanel: Server auflisten, starten/stoppen, Konsolenbefehle, Konsole lesen, Dateien, Spieler inkl. Inventar und Endertruhe, Backups, Ereignisse."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call(app, &name, &args).await {
                Ok(v) => json!({
                    "content": [{ "type": "text", "text": if let Value::String(s) = &v { s.clone() } else { serde_json::to_string_pretty(&v).unwrap_or_default() } }],
                    "isError": false
                }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true }),
            })
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("Unbekannte Methode: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, msg)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } }),
    })
}

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required }
    })
}

fn tools() -> Vec<Value> {
    let sid = json!({ "type": "string", "description": "Server-ID (aus list_servers)" });
    vec![
        tool("list_servers", "Alle Server mit Status, Spielern, CPU/RAM.", json!({}), &[]),
        tool("get_server", "Details eines Servers inkl. Warnungen und Konfiguration.", json!({ "server_id": sid }), &["server_id"]),
        tool(
            "power",
            "Server starten, stoppen, neu starten oder hart beenden.",
            json!({ "server_id": sid, "action": { "type": "string", "enum": ["start", "stop", "restart", "kill"] } }),
            &["server_id", "action"],
        ),
        tool(
            "send_command",
            "Konsolenbefehl ausführen (ohne führenden /). Liefert die Antwort, wenn RCON aktiv ist.",
            json!({ "server_id": sid, "command": { "type": "string" } }),
            &["server_id", "command"],
        ),
        tool(
            "read_console",
            "Letzte Konsolenzeilen lesen.",
            json!({ "server_id": sid, "lines": { "type": "integer", "default": 100 } }),
            &["server_id"],
        ),
        tool(
            "list_events",
            "Letzte Ereignisse (Beitritte, Chat, Tode, Abstürze, Backups …).",
            json!({ "server_id": sid, "limit": { "type": "integer", "default": 50 } }),
            &["server_id"],
        ),
        tool("list_files", "Dateien eines Ordners im Server-Verzeichnis.", json!({ "server_id": sid, "path": { "type": "string", "default": "" } }), &["server_id"]),
        tool("read_file", "Textdatei lesen (max. 512 KB).", json!({ "server_id": sid, "path": { "type": "string" } }), &["server_id", "path"]),
        tool(
            "write_file",
            "Textdatei schreiben (überschreibt).",
            json!({ "server_id": sid, "path": { "type": "string" }, "content": { "type": "string" } }),
            &["server_id", "path", "content"],
        ),
        tool("list_players", "Minecraft: Spieler (online, bekannt, OPs, Whitelist, Banns).", json!({ "server_id": sid }), &["server_id"]),
        tool(
            "get_player",
            "Minecraft: Spieler mit Position, Leben, Inventar und Endertruhe.",
            json!({ "server_id": sid, "player": { "type": "string", "description": "Name oder UUID" } }),
            &["server_id", "player"],
        ),
        tool(
            "player_action",
            "Minecraft: Spieleraktion. action: kick, ban, pardon, op, deop, whitelist_add, whitelist_remove, gamemode (mode), teleport (x,y,z), heal, feed, kill, message (text).",
            json!({
                "server_id": sid,
                "player": { "type": "string" },
                "action": { "type": "string" },
                "reason": { "type": "string" }, "mode": { "type": "string" }, "text": { "type": "string" },
                "x": { "type": "number" }, "y": { "type": "number" }, "z": { "type": "number" }
            }),
            &["server_id", "player", "action"],
        ),
        tool(
            "give_item",
            "Minecraft: Gegenstand an einen Online-Spieler geben.",
            json!({ "server_id": sid, "player": { "type": "string" }, "item": { "type": "string", "description": "z. B. minecraft:diamond" }, "count": { "type": "integer", "default": 1 } }),
            &["server_id", "player", "item"],
        ),
        tool("player_positions", "Minecraft: Positionen aller Spieler (für Karte).", json!({ "server_id": sid }), &["server_id"]),
        tool(
            "create_backup",
            "Backup erstellen (worlds_only = nur Welten).",
            json!({ "server_id": sid, "worlds_only": { "type": "boolean", "default": false } }),
            &["server_id"],
        ),
    ]
}

fn s<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| AppError::bad(format!("Parameter `{key}` fehlt")))
}

async fn call(app: &AppState, name: &str, args: &Value) -> AppResult<Value> {
    let to = |v: &dyn erased::Ser| v.json();
    match name {
        "list_servers" => {
            let mut out = Vec::new();
            for cfg in app.store.servers().await {
                let sum = servers::summary(app, cfg).await;
                out.push(json!({
                    "id": sum.config.id, "name": sum.config.name, "game": sum.config.game, "flavor": sum.config.flavor,
                    "version": sum.config.version, "status": sum.status, "port": sum.config.port, "players": sum.players,
                    "cpu_percent": sum.stats.cpu, "memory_mb": sum.stats.memory_bytes / 1024 / 1024,
                }));
            }
            Ok(Value::Array(out))
        }
        "get_server" => {
            let (cfg, _) = app.server(s(args, "server_id")?).await?;
            Ok(to(&servers::summary(app, cfg).await))
        }
        "power" => {
            let id = s(args, "server_id")?;
            let (_, rt) = app.server(id).await?;
            match s(args, "action")? {
                "start" => rt.start(app).await?,
                "stop" => rt.stop_and_wait(app).await?,
                "restart" => rt.restart(app).await?,
                "kill" => rt.kill(),
                a => return Err(AppError::bad(format!("Unbekannte Aktion {a}"))),
            }
            Ok(json!({ "ok": true, "status": rt.status() }))
        }
        "send_command" => {
            let (_, rt) = app.server(s(args, "server_id")?).await?;
            let out = rt.command(app, s(args, "command")?).await?;
            Ok(json!({ "output": out }))
        }
        "read_console" => {
            let (_, rt) = app.server(s(args, "server_id")?).await?;
            let n = args.get("lines").and_then(Value::as_u64).unwrap_or(100).min(2000) as usize;
            let text = rt.history(n).into_iter().map(|l| crate::events::strip_ansi(&l.text)).collect::<Vec<_>>().join("\n");
            Ok(Value::String(text))
        }
        "list_events" => {
            let id = s(args, "server_id")?;
            let n = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            Ok(to(&app.events.recent(Some(id), n)))
        }
        "list_files" => {
            let (cfg, _) = app.server(s(args, "server_id")?).await?;
            let base = PathBuf::from(&cfg.dir);
            let q = PathQuery { path: args.get("path").and_then(Value::as_str).unwrap_or("").into() };
            let dir = safe_join(&base, &q.path)?;
            let mut out = Vec::new();
            for e in std::fs::read_dir(&dir)?.flatten() {
                let m = e.metadata().ok();
                out.push(json!({
                    "path": rel_path(&base, &e.path()),
                    "dir": m.as_ref().is_some_and(std::fs::Metadata::is_dir),
                    "size": m.as_ref().map_or(0, std::fs::Metadata::len),
                }));
            }
            Ok(Value::Array(out))
        }
        "read_file" => {
            let (cfg, _) = app.server(s(args, "server_id")?).await?;
            let p = safe_join(&PathBuf::from(&cfg.dir), s(args, "path")?)?;
            let meta = tokio::fs::metadata(&p).await?;
            if meta.len() > 512 * 1024 {
                return Err(AppError::bad("Datei zu gross (max. 512 KB)"));
            }
            let bytes = tokio::fs::read(&p).await?;
            Ok(Value::String(String::from_utf8(bytes).map_err(|_| AppError::bad("Binärdatei"))?))
        }
        "write_file" => {
            let (cfg, _) = app.server(s(args, "server_id")?).await?;
            let p = safe_join(&PathBuf::from(&cfg.dir), s(args, "path")?)?;
            if let Some(parent) = p.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&p, s(args, "content")?).await?;
            Ok(json!({ "ok": true }))
        }
        "list_players" => Ok(to(&minecraft::players_overview(app, s(args, "server_id")?).await?)),
        "get_player" => Ok(to(&minecraft::player_detail(app, s(args, "server_id")?, s(args, "player")?).await?)),
        "player_action" => {
            let id = s(args, "server_id")?;
            ensure_minecraft(app, id).await?;
            let opt = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
            let num = |k: &str| args.get(k).and_then(Value::as_f64).ok_or_else(|| AppError::bad(format!("`{k}` fehlt")));
            let action = match s(args, "action")? {
                "kick" => PlayerAction::Kick { reason: opt("reason") },
                "ban" => PlayerAction::Ban { reason: opt("reason") },
                "pardon" => PlayerAction::Pardon,
                "op" => PlayerAction::Op,
                "deop" => PlayerAction::Deop,
                "whitelist_add" => PlayerAction::WhitelistAdd,
                "whitelist_remove" => PlayerAction::WhitelistRemove,
                "gamemode" => PlayerAction::Gamemode { mode: opt("mode").unwrap_or_else(|| "survival".into()) },
                "teleport" => PlayerAction::Teleport { x: num("x")?, y: num("y")?, z: num("z")?, dimension: None },
                "heal" => PlayerAction::Heal,
                "feed" => PlayerAction::Feed,
                "kill" => PlayerAction::Kill,
                "message" => PlayerAction::Message { text: opt("text").unwrap_or_default() },
                a => return Err(AppError::bad(format!("Unbekannte Aktion {a}"))),
            };
            Ok(to(&minecraft::player_action(app, id, s(args, "player")?, action).await?))
        }
        "give_item" => {
            let id = s(args, "server_id")?;
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1) as u32;
            let op = InventoryOp::Give { id: s(args, "item")?.into(), count };
            Ok(to(&minecraft::inventory_op(app, id, s(args, "player")?, op).await?))
        }
        "player_positions" => Ok(to(&minecraft::map_players(app, s(args, "server_id")?).await?)),
        "create_backup" => {
            let wo = args.get("worlds_only").and_then(Value::as_bool).unwrap_or(false);
            Ok(to(&backups::create_backup(app, s(args, "server_id")?, wo).await?))
        }
        _ => Err(AppError::bad(format!("Unbekanntes Werkzeug: {name}"))),
    }
}

async fn ensure_minecraft(app: &AppState, id: &str) -> AppResult<()> {
    let (cfg, _) = app.server(id).await?;
    if cfg.game != Game::Minecraft {
        return Err(AppError::bad("Nur für Minecraft-Server"));
    }
    Ok(())
}

/// Kleine Hilfe, um beliebige `Serialize`-Werte als JSON zu liefern.
mod erased {
    pub trait Ser {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or_default()
        }
    }
}
