//! Factorio-Integration: Spielstände, Mods, server-settings.json, Admins.
//!
//! Routen: `/api/servers/{id}/factorio/*`.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    routing::{get, post},
};
use nrz_gp_common::{
    AddonToggle, FactorioMod, FactorioMods, FactorioSave, FactorioSaves, FactorioSettings, Game, LaunchKind, NameList, Ok as OkResp, SaveName,
    ServerConfig, ServerStatus,
};
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    import::detect::cmp_versions,
    install,
    state::AppState,
};

/// Wo `process::build_command` server-settings.json sucht (gleiche Reihenfolge).
const SETTINGS_PATHS: [&str; 3] = ["data/server-settings.json", "server-settings.json", "config/server-settings.json"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/servers/{id}/factorio/saves", get(saves).post(create_save))
        .route("/api/servers/{id}/factorio/saves/select", post(select_save))
        .route("/api/servers/{id}/factorio/mods", get(mods))
        .route("/api/servers/{id}/factorio/mods/toggle", post(toggle_mod))
        .route("/api/servers/{id}/factorio/settings", get(get_settings).put(put_settings))
        .route("/api/servers/{id}/factorio/admins", get(get_admins).put(put_admins))
}

fn ok() -> Json<OkResp> {
    Json(OkResp { ok: true })
}

async fn factorio_server(app: &AppState, id: &str) -> AppResult<(ServerConfig, PathBuf)> {
    let (cfg, _) = app.server(id).await?;
    if cfg.game != Game::Factorio {
        return Err(AppError::bad("Kein Factorio-Server"));
    }
    let dir = PathBuf::from(&cfg.dir);
    Ok((cfg, dir))
}

fn write_json(path: &Path, v: &impl serde::Serialize) -> AppResult<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let s = serde_json::to_string_pretty(v).map_err(|e| AppError::internal(e.to_string()))?;
    std::fs::write(path, s + "\n")?;
    Ok(())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Gültiger Spielstandname (Dateiname ohne Pfad), `.zip` wird entfernt.
fn save_name(name: &str) -> AppResult<String> {
    let n = name.trim().trim_end_matches(".zip").trim();
    if n.is_empty() || n.len() > 100 || n.starts_with('.') || !n.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')) {
        return Err(AppError::bad("Ungültiger Name (erlaubt: Buchstaben, Ziffern, Leerzeichen, - _ .)"));
    }
    Ok(n.to_string())
}

/* ----------------------------- Spielstände ----------------------------- */

fn list_saves(dir: &Path, selected: Option<&str>) -> Vec<FactorioSave> {
    let Ok(rd) = std::fs::read_dir(dir.join("saves")) else { return Vec::new() };
    let mut saves: Vec<FactorioSave> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stem = name.strip_suffix(".zip")?.to_string();
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            Some(FactorioSave { name: stem, size: m.len(), modified, active: false })
        })
        .collect();
    saves.sort_by(|a, b| b.modified.cmp(&a.modified));
    let active = match selected {
        Some(s) => saves.iter().position(|x| x.name == s),
        None => (!saves.is_empty()).then_some(0),
    };
    if let Some(i) = active {
        saves[i].active = true;
    }
    saves
}

fn selected_save(cfg: &ServerConfig) -> Option<String> {
    match &cfg.launch {
        LaunchKind::Factorio { save, .. } => save.as_ref().map(|s| s.trim_end_matches(".zip").to_string()),
        _ => None,
    }
}

async fn saves(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<FactorioSaves>> {
    let (cfg, dir) = factorio_server(&app, &id).await?;
    let selected = selected_save(&cfg);
    let s = selected.clone();
    let saves = tokio::task::spawn_blocking(move || list_saves(&dir, s.as_deref())).await?;
    Ok(Json(FactorioSaves { saves, selected }))
}

async fn create_save(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(r): Json<SaveName>) -> AppResult<Json<OkResp>> {
    let (cfg, dir) = factorio_server(&app, &id).await?;
    let rt = app.runtime(&id).await;
    match rt.status() {
        ServerStatus::Stopped | ServerStatus::Crashed => {}
        ServerStatus::Installing => return Err(AppError::conflict("Server wird noch installiert")),
        _ => return Err(AppError::conflict("Bitte den Server zuerst stoppen")),
    }
    let name = save_name(r.name.as_deref().unwrap_or(""))?;
    let rel = format!("saves/{name}.zip");
    if dir.join(&rel).exists() {
        return Err(AppError::conflict(format!("Spielstand {name} existiert bereits")));
    }
    let binary = match &cfg.launch {
        LaunchKind::Factorio { binary, .. } => binary.clone(),
        _ => "bin/x64/factorio".into(),
    };
    if !dir.join(&binary).is_file() {
        return Err(AppError::bad(format!("Factorio-Programm fehlt: {binary}")));
    }
    tokio::fs::create_dir_all(dir.join("saves")).await?;
    rt.panel(format!("Erzeuge neue Karte {rel} …"));
    let mut cmd = Command::new(dir.join(&binary));
    cmd.arg("--create").arg(&rel).current_dir(&dir);
    match install::run_logged(&rt, cmd).await {
        Ok(()) => {
            rt.panel(format!("✔ Karte {name} erstellt."));
            Ok(ok())
        }
        Err(e) => {
            rt.panel(format!("✖ Karte konnte nicht erstellt werden: {e}"));
            Err(e)
        }
    }
}

async fn select_save(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(r): Json<SaveName>) -> AppResult<Json<OkResp>> {
    let (cfg, dir) = factorio_server(&app, &id).await?;
    if !matches!(cfg.launch, LaunchKind::Factorio { .. }) {
        return Err(AppError::bad("Startart ist nicht Factorio"));
    }
    let wanted = match r.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => {
            let n = save_name(n)?;
            if !dir.join(format!("saves/{n}.zip")).is_file() {
                return Err(AppError::not_found(format!("Spielstand {n} nicht gefunden")));
            }
            Some(n)
        }
        None => None,
    };
    app.update_server(&id, move |c| {
        if let LaunchKind::Factorio { save, .. } = &mut c.launch {
            *save = wanted;
        }
    })
    .await?;
    Ok(ok())
}

/* --------------------------------- Mods --------------------------------- */

/// `mods/<name>_<version>.zip` → (Name, Version).
fn parse_mod_file(file: &str) -> Option<(String, String)> {
    let stem = file.strip_suffix(".zip")?;
    let (name, ver) = stem.rsplit_once('_')?;
    if name.is_empty() || !ver.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((name.to_string(), ver.to_string()))
}

fn list_mods(dir: &Path) -> Vec<FactorioMod> {
    let mods_dir = dir.join("mods");
    // Dateien: Name → (Datei, Version), höchste Version gewinnt
    let mut files: Vec<(String, String, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&mods_dir) {
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().to_string();
            if let Some((name, ver)) = parse_mod_file(&f) {
                match files.iter_mut().find(|(n, _, _)| *n == name) {
                    Some(x) if cmp_versions(&ver, &x.2).is_gt() => *x = (name, f, ver),
                    Some(_) => {}
                    None => files.push((name, f, ver)),
                }
            }
        }
    }
    let list = read_json(&mods_dir.join("mod-list.json"));
    let mut out: Vec<FactorioMod> = Vec::new();
    for m in list.as_ref().and_then(|l| l.get("mods")).and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = m.get("name").and_then(Value::as_str) else { continue };
        let enabled = m.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let file = files.iter().find(|(n, _, _)| n == name);
        out.push(FactorioMod { name: name.to_string(), enabled, file: file.map(|f| f.1.clone()), version: file.map(|f| f.2.clone()) });
    }
    if !out.iter().any(|m| m.name == "base") {
        out.insert(0, FactorioMod { name: "base".into(), enabled: true, file: None, version: None });
    }
    for (name, file, ver) in files {
        if !out.iter().any(|m| m.name == name) {
            // Neue Mods sind in Factorio standardmässig aktiv
            out.push(FactorioMod { name, enabled: true, file: Some(file), version: Some(ver) });
        }
    }
    out
}

async fn mods(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<FactorioMods>> {
    let (_, dir) = factorio_server(&app, &id).await?;
    let mods = tokio::task::spawn_blocking(move || list_mods(&dir)).await?;
    Ok(Json(FactorioMods { mods }))
}

/// Mod in `mods/mod-list.json` ein-/ausschalten (andere Felder bleiben erhalten).
fn set_mod_enabled(dir: &Path, name: &str, enabled: bool) -> AppResult<()> {
    let path = dir.join("mods/mod-list.json");
    let mut list = read_json(&path).unwrap_or_else(|| json!({ "mods": [{ "name": "base", "enabled": true }] }));
    if !list.get("mods").is_some_and(Value::is_array) {
        list = json!({ "mods": [{ "name": "base", "enabled": true }] });
    }
    if let Some(arr) = list.get_mut("mods").and_then(Value::as_array_mut) {
        match arr.iter_mut().find(|m| m.get("name").and_then(Value::as_str) == Some(name)) {
            Some(m) => {
                if let Some(o) = m.as_object_mut() {
                    o.insert("enabled".into(), enabled.into());
                }
            }
            None => arr.push(json!({ "name": name, "enabled": enabled })),
        }
    }
    write_json(&path, &list)
}

async fn toggle_mod(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(t): Json<AddonToggle>) -> AppResult<Json<OkResp>> {
    let (_, dir) = factorio_server(&app, &id).await?;
    let name = t.file.trim().to_string();
    // Dateiname statt Mod-Name erlauben
    let name = parse_mod_file(&name).map(|(n, _)| n).unwrap_or(name);
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(AppError::bad("Ungültiger Mod-Name"));
    }
    if name == "base" && !t.enabled {
        return Err(AppError::bad("Das Basisspiel kann nicht deaktiviert werden"));
    }
    tokio::task::spawn_blocking(move || set_mod_enabled(&dir, &name, t.enabled)).await??;
    Ok(ok())
}

/* ------------------------------ Einstellungen ----------------------------- */

fn settings_path(dir: &Path) -> Option<PathBuf> {
    SETTINGS_PATHS.iter().map(|p| dir.join(p)).find(|p| p.is_file())
}

async fn get_settings(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<FactorioSettings>> {
    let (_, dir) = factorio_server(&app, &id).await?;
    let settings = match settings_path(&dir) {
        Some(p) => {
            let raw = tokio::fs::read_to_string(&p).await?;
            serde_json::from_str(&raw).map_err(|e| AppError::internal(format!("server-settings.json ist ungültig: {e}")))?
        }
        // Noch keine Datei → Beispiel als Vorlage
        None => read_json(&dir.join("data/server-settings.example.json")).unwrap_or_else(|| json!({})),
    };
    Ok(Json(FactorioSettings { settings }))
}

async fn put_settings(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(s): Json<FactorioSettings>) -> AppResult<Json<OkResp>> {
    let (_, dir) = factorio_server(&app, &id).await?;
    if !s.settings.is_object() {
        return Err(AppError::bad("Einstellungen müssen ein JSON-Objekt sein"));
    }
    let path = settings_path(&dir).unwrap_or_else(|| if dir.join("data").is_dir() { dir.join(SETTINGS_PATHS[0]) } else { dir.join(SETTINGS_PATHS[1]) });
    write_json(&path, &s.settings)?;
    Ok(ok())
}

/* --------------------------------- Admins -------------------------------- */

async fn get_admins(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<NameList>> {
    let (_, dir) = factorio_server(&app, &id).await?;
    let names = read_json(&dir.join("server-adminlist.json"))
        .and_then(|v| v.as_array().cloned())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Ok(Json(NameList { names }))
}

async fn put_admins(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(l): Json<NameList>) -> AppResult<Json<OkResp>> {
    let (_, dir) = factorio_server(&app, &id).await?;
    let mut names: Vec<String> = Vec::new();
    for n in l.names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        if !names.iter().any(|x| x.eq_ignore_ascii_case(n)) {
            names.push(n.to_string());
        }
    }
    write_json(&dir.join("server-adminlist.json"), &names)?;
    Ok(ok())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::import::detect::tests::TempDir;

    #[test]
    fn mods_listing_and_toggle() {
        let t = TempDir::new("fmods");
        let d = &t.0;
        fs::create_dir_all(d.join("mods")).unwrap();
        fs::write(d.join("mods/mod-list.json"), r#"{"mods":[{"name":"base","enabled":true},{"name":"space-age","enabled":false},{"name":"Krastorio2","enabled":true}]}"#).unwrap();
        fs::write(d.join("mods/Krastorio2_1.3.0.zip"), b"").unwrap();
        fs::write(d.join("mods/Krastorio2_1.3.10.zip"), b"").unwrap();
        fs::write(d.join("mods/even-distribution_2.0.1.zip"), b"").unwrap();
        let m = list_mods(d);
        assert_eq!(m.len(), 4);
        let k = m.iter().find(|x| x.name == "Krastorio2").unwrap();
        assert_eq!(k.version.as_deref(), Some("1.3.10"));
        assert!(m.iter().find(|x| x.name == "space-age").unwrap().file.is_none());
        set_mod_enabled(d, "even-distribution", false).unwrap();
        set_mod_enabled(d, "Krastorio2", false).unwrap();
        let m = list_mods(d);
        assert!(!m.iter().find(|x| x.name == "even-distribution").unwrap().enabled);
        assert!(!m.iter().find(|x| x.name == "Krastorio2").unwrap().enabled);
        assert_eq!(parse_mod_file("my_mod_name_0.1.2.zip"), Some(("my_mod_name".into(), "0.1.2".into())));
        assert!(save_name("../x").is_err());
        assert_eq!(save_name("Meine Welt.zip").unwrap(), "Meine Welt");
    }
}
