//! Plugins (`plugins/`) bzw. Mods (`mods/`): auflisten mit Metadaten, (de)aktivieren, löschen;
//! Modrinth-Suche und -Installation.

use std::path::{Path, PathBuf};

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
};
use nrz_gp_common::{Addon, AddonList, AddonToggle, Flavor, ModrinthHit, ModrinthInstall, Ok as OkResp};
use serde::Deserialize;
use serde_json::Value as J;

use super::{Ctx, assets, ctx, safe_name};
use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    state::AppState,
    util,
};

const MODRINTH: &str = "https://api.modrinth.com/v2";

/// `plugins` oder `mods`.
fn folder_name(c: &Ctx) -> &'static str {
    if c.cfg.flavor.uses_plugins() {
        "plugins"
    } else if c.cfg.flavor.uses_mods() {
        "mods"
    } else if c.dir.join("plugins").is_dir() {
        "plugins"
    } else {
        "mods"
    }
}

fn valid_addon_file(f: &str) -> bool {
    safe_name(f) && !f.starts_with('.') && (f.ends_with(".jar") || f.ends_with(".jar.disabled"))
}

/* ------------------------------ Metadaten ------------------------------ */

fn yaml_str(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
    .filter(|s| !s.trim().is_empty())
}

fn json_str(v: Option<&J>) -> Option<String> {
    match v? {
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        _ => None,
    }
    .filter(|s| !s.trim().is_empty())
}

fn json_people(v: Option<&J>) -> Vec<String> {
    match v {
        Some(J::Array(a)) => a
            .iter()
            .filter_map(|p| match p {
                J::String(s) => Some(s.clone()),
                J::Object(o) => o.get("name").and_then(J::as_str).map(str::to_string),
                _ => None,
            })
            .collect(),
        Some(J::Object(o)) => o.keys().cloned().collect(),
        Some(J::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

#[derive(Default)]
struct Meta {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    authors: Vec<String>,
}

fn read_meta(path: &Path) -> Meta {
    let Ok(mut zip) = assets::open_zip(path) else { return Meta::default() };
    let mut read = |n: &str| assets::zip_read(&mut zip, n).map(|b| String::from_utf8_lossy(&b).to_string());
    for n in ["plugin.yml", "paper-plugin.yml", "bungee.yml"] {
        if let Some(s) = read(n) {
            if let Ok(y) = serde_yaml::from_str::<serde_yaml::Value>(&s) {
                let mut authors: Vec<String> = Vec::new();
                if let Some(a) = y.get("author").and_then(yaml_str) {
                    authors.push(a);
                }
                if let Some(serde_yaml::Value::Sequence(list)) = y.get("authors") {
                    authors.extend(list.iter().filter_map(yaml_str));
                }
                authors.dedup();
                return Meta {
                    name: y.get("name").and_then(yaml_str),
                    version: y.get("version").and_then(yaml_str),
                    description: y.get("description").and_then(yaml_str),
                    authors,
                };
            }
        }
    }
    if let Some(s) = read("fabric.mod.json") {
        if let Ok(j) = serde_json::from_str::<J>(&s.replace(['\n', '\r', '\t'], " ")) {
            return Meta {
                name: json_str(j.get("name")).or_else(|| json_str(j.get("id"))),
                version: json_str(j.get("version")),
                description: json_str(j.get("description")),
                authors: json_people(j.get("authors")),
            };
        }
    }
    if let Some(s) = read("quilt.mod.json") {
        if let Ok(j) = serde_json::from_str::<J>(&s) {
            let ql = j.get("quilt_loader");
            let md = ql.and_then(|q| q.get("metadata"));
            return Meta {
                name: json_str(md.and_then(|m| m.get("name"))).or_else(|| json_str(ql.and_then(|q| q.get("id")))),
                version: json_str(ql.and_then(|q| q.get("version"))),
                description: json_str(md.and_then(|m| m.get("description"))),
                authors: json_people(md.and_then(|m| m.get("contributors"))),
            };
        }
    }
    for n in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(s) = read(n) {
            if let Ok(t) = s.parse::<toml::Table>() {
                let m = t.get("mods").and_then(|m| m.as_array()).and_then(|a| a.first()).and_then(|m| m.as_table());
                let get = |k: &str| m.and_then(|m| m.get(k)).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let mut version = get("version");
                if version.as_deref().is_some_and(|v| v.contains("${")) {
                    version = read("META-INF/MANIFEST.MF").and_then(|mf| {
                        mf.lines().find_map(|l| l.strip_prefix("Implementation-Version:").map(|v| v.trim().to_string()))
                    });
                }
                let authors = get("authors")
                    .or_else(|| t.get("authors").and_then(|v| v.as_str()).map(str::to_string))
                    .map(|a| a.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                    .unwrap_or_default();
                return Meta { name: get("displayName").or_else(|| get("modId")), version, description: get("description"), authors };
            }
        }
    }
    Meta::default()
}

fn addon_for(path: &Path) -> Option<Addon> {
    let file = path.file_name()?.to_string_lossy().to_string();
    if !valid_addon_file(&file) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let m = read_meta(path);
    Some(Addon {
        enabled: file.ends_with(".jar"),
        file,
        name: m.name,
        version: m.version,
        description: m.description,
        authors: m.authors,
        size: meta.len(),
    })
}

fn list_addons(dir: &Path) -> Vec<Addon> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Addon> = rd.flatten().filter_map(|e| addon_for(&e.path())).collect();
    v.sort_by_key(|a| a.file.to_lowercase());
    v
}

/* ------------------------------ Handler ------------------------------ */

pub async fn h_list(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<AddonList>> {
    let c = ctx(&app, &id).await?;
    let folder = folder_name(&c);
    let dir = c.dir.join(folder);
    let addons = tokio::task::spawn_blocking(move || list_addons(&dir)).await?;
    Ok(Json(AddonList { folder: folder.into(), addons }))
}

pub async fn h_toggle(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(req): Json<AddonToggle>) -> AppResult<Json<OkResp>> {
    let c = ctx(&app, &id).await?;
    if !valid_addon_file(&req.file) {
        return Err(AppError::bad("Ungültiger Dateiname"));
    }
    let dir = c.dir.join(folder_name(&c));
    let src = dir.join(&req.file);
    if !src.is_file() {
        return Err(AppError::not_found("Datei nicht gefunden"));
    }
    let target = match (req.enabled, req.file.strip_suffix(".disabled")) {
        (true, Some(base)) => dir.join(base),
        (false, None) => dir.join(format!("{}.disabled", req.file)),
        _ => return Ok(Json(OkResp { ok: true })), // schon im gewünschten Zustand
    };
    if target.exists() {
        return Err(AppError::conflict("Eine Datei mit diesem Namen existiert bereits"));
    }
    tokio::fs::rename(&src, &target).await?;
    Ok(Json(OkResp { ok: true }))
}

pub async fn h_delete(State(app): State<AppState>, _a: Authed, AxPath((id, file)): AxPath<(String, String)>) -> AppResult<Json<OkResp>> {
    let c = ctx(&app, &id).await?;
    if !valid_addon_file(&file) {
        return Err(AppError::bad("Ungültiger Dateiname"));
    }
    let p = c.dir.join(folder_name(&c)).join(&file);
    if !p.is_file() {
        return Err(AppError::not_found("Datei nicht gefunden"));
    }
    tokio::fs::remove_file(&p).await?;
    Ok(Json(OkResp { ok: true }))
}

/* ------------------------------ Modrinth ------------------------------ */

/// Erlaubte Modrinth-Loader und Projekttyp für den Server.
fn loaders(c: &Ctx) -> AppResult<(Vec<&'static str>, &'static str)> {
    let l: Vec<&'static str> = match c.cfg.flavor {
        Flavor::Paper => vec!["paper", "spigot", "bukkit"],
        Flavor::Purpur => vec!["purpur", "paper", "spigot", "bukkit"],
        Flavor::Spigot => vec!["spigot", "bukkit"],
        Flavor::Quilt => vec!["quilt", "fabric"],
        f => match f.modrinth_loader() {
            Some(l) => vec![l],
            None if folder_name(c) == "plugins" => vec!["paper", "spigot", "bukkit"],
            None => return Err(AppError::bad("Für diesen Server-Typ gibt es keine Plugins/Mods")),
        },
    };
    let kind = if folder_name(c) == "plugins" { "plugin" } else { "mod" };
    Ok((l, kind))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
}

pub async fn h_search(
    State(app): State<AppState>,
    _a: Authed,
    AxPath(id): AxPath<String>,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<Vec<ModrinthHit>>> {
    let c = ctx(&app, &id).await?;
    let (loaders, kind) = loaders(&c)?;
    let mut facets: Vec<Vec<String>> = vec![loaders.iter().map(|l| format!("categories:{l}")).collect()];
    if let Some(v) = c.cfg.version.as_deref().filter(|v| !v.is_empty()) {
        facets.push(vec![format!("versions:{v}")]);
    }
    facets.push(vec![format!("project_type:{kind}")]);
    let facets = serde_json::to_string(&facets).map_err(|e| AppError::internal(e.to_string()))?;
    let resp: J = app
        .http
        .get(format!("{MODRINTH}/search"))
        .query(&[("query", q.q.trim()), ("facets", facets.as_str()), ("limit", "20")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let hits = resp
        .get("hits")
        .and_then(J::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    let s = |k: &str| h.get(k).and_then(J::as_str).unwrap_or_default().to_string();
                    Some(ModrinthHit {
                        project_id: h.get("project_id")?.as_str()?.to_string(),
                        slug: s("slug"),
                        title: s("title"),
                        description: s("description"),
                        author: s("author"),
                        downloads: h.get("downloads").and_then(J::as_u64).unwrap_or(0),
                        icon_url: h.get("icon_url").and_then(J::as_str).filter(|u| !u.is_empty()).map(str::to_string),
                        categories: h
                            .get("categories")
                            .and_then(J::as_array)
                            .map(|c| c.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(hits))
}

pub async fn h_install(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(req): Json<ModrinthInstall>) -> AppResult<Json<Addon>> {
    let c = ctx(&app, &id).await?;
    let pid = req.project_id.trim();
    if pid.is_empty() || !pid.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
        return Err(AppError::bad("Ungültige Projekt-ID"));
    }
    let (loaders, _) = loaders(&c)?;
    let loaders_json = serde_json::to_string(&loaders).map_err(|e| AppError::internal(e.to_string()))?;
    let mut query: Vec<(&str, String)> = vec![("loaders", loaders_json)];
    if let Some(v) = c.cfg.version.as_deref().filter(|v| !v.is_empty()) {
        query.push(("game_versions", serde_json::to_string(&[v]).map_err(|e| AppError::internal(e.to_string()))?));
    }
    let versions: Vec<J> = app
        .http
        .get(format!("{MODRINTH}/project/{pid}/version"))
        .query(&query)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let newest = |only_release: bool| {
        versions
            .iter()
            .filter(|v| !only_release || v.get("version_type").and_then(J::as_str) == Some("release"))
            .max_by_key(|v| v.get("date_published").and_then(J::as_str).unwrap_or_default().to_string())
    };
    let version = newest(true).or_else(|| newest(false)).ok_or_else(|| AppError::not_found("Keine passende Version für diesen Server gefunden"))?;
    let files = version.get("files").and_then(J::as_array).ok_or_else(|| AppError::not_found("Version ohne Dateien"))?;
    let file = files
        .iter()
        .find(|f| f.get("primary").and_then(J::as_bool) == Some(true))
        .or_else(|| files.iter().find(|f| f.get("filename").and_then(J::as_str).is_some_and(|n| n.ends_with(".jar"))))
        .or_else(|| files.first())
        .ok_or_else(|| AppError::not_found("Version ohne Dateien"))?;
    let url = file.get("url").and_then(J::as_str).ok_or_else(|| AppError::internal("Download-URL fehlt"))?;
    let filename = file.get("filename").and_then(J::as_str).unwrap_or_default().to_string();
    if !valid_addon_file(&filename) || !filename.ends_with(".jar") {
        return Err(AppError::bad(format!("Unerwarteter Dateiname „{filename}“")));
    }
    let dir: PathBuf = c.dir.join(folder_name(&c));
    let dest = util::safe_join(&dir, &filename)?;
    util::download(&app.http, url, &dest, |_, _| {}).await?;
    let addon = tokio::task::spawn_blocking(move || addon_for(&dest))
        .await?
        .ok_or_else(|| AppError::internal("Installierte Datei nicht lesbar"))?;
    Ok(Json(addon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_names() {
        assert!(valid_addon_file("EssentialsX-2.20.jar"));
        assert!(valid_addon_file("sodium.jar.disabled"));
        assert!(!valid_addon_file("../x.jar"));
        assert!(!valid_addon_file("a/b.jar"));
        assert!(!valid_addon_file("x.zip"));
        assert!(!valid_addon_file(".hidden.jar"));
    }

    #[test]
    fn plugin_yml_meta() {
        let p = std::env::temp_dir().join(format!("nrzgp-addon-{}.jar", std::process::id()));
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("plugin.yml", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"name: Demo\nversion: 1.2\nauthor: Ann\nauthors: [Bob]\ndescription: Test\n").unwrap();
            z.finish().unwrap();
        }
        let m = read_meta(&p);
        assert_eq!(m.name.as_deref(), Some("Demo"));
        assert_eq!(m.version.as_deref(), Some("1.2"));
        assert_eq!(m.authors, vec!["Ann".to_string(), "Bob".to_string()]);
        let a = addon_for(&p).unwrap();
        assert!(a.enabled);
        let _ = std::fs::remove_file(&p);
    }
}
