//! Offizielles Client-Jar (Texturen, Item-Liste) und Spielerköpfe.

use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::HeaderMap,
    response::Response,
};
use nrz_gp_common::ItemCatalog;
use once_cell::sync::Lazy;
use regex::Regex;

use super::{ctx, map, mtime_secs, png_response};
use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    state::AppState,
    util,
};

const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

static VERSION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9._\- ]{1,64}$").expect("regex"));
static JAR_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

fn valid_version(v: &str) -> bool {
    VERSION_RE.is_match(v) && !v.contains("..")
}

pub fn client_dir(app: &AppState, version: &str) -> PathBuf {
    app.config.cache_dir().join("mc-client").join(version)
}

pub fn client_jar_path(app: &AppState, version: &str) -> PathBuf {
    client_dir(app, version).join("client.jar")
}

/// Client-Jar für die Version schon vorhanden?
pub fn client_jar_present(app: &AppState, version: Option<&str>) -> bool {
    version.filter(|v| valid_version(v)).is_some_and(|v| client_jar_path(app, v).is_file())
}

/// Client-Jar laden (einmalig, gegen Doppel-Download geschützt).
pub async fn ensure_client_jar(app: &AppState, version: Option<&str>) -> AppResult<PathBuf> {
    let v = version.map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| AppError::not_found("Minecraft-Version unbekannt"))?;
    if !valid_version(v) {
        return Err(AppError::not_found("Minecraft-Version unbekannt"));
    }
    let path = client_jar_path(app, v);
    if path.is_file() {
        return Ok(path);
    }
    let _g = JAR_LOCK.lock().await;
    if path.is_file() {
        return Ok(path);
    }
    let manifest: serde_json::Value = app.http.get(MANIFEST).send().await?.error_for_status()?.json().await?;
    let url = manifest
        .get("versions")
        .and_then(|v| v.as_array())
        .and_then(|a| a.iter().find(|e| e.get("id").and_then(|i| i.as_str()) == Some(v)))
        .and_then(|e| e.get("url"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| AppError::not_found(format!("Minecraft-Version {v} bei Mojang nicht gefunden")))?
        .to_string();
    let vj: serde_json::Value = app.http.get(&url).send().await?.error_for_status()?.json().await?;
    let client = vj
        .pointer("/downloads/client/url")
        .and_then(|u| u.as_str())
        .ok_or_else(|| AppError::not_found("Kein Client-Download für diese Version"))?
        .to_string();
    tracing::info!("Lade Minecraft-Client {v} für Texturen");
    util::download(&app.http, &client, &path, |_, _| {}).await?;
    Ok(path)
}

/// Datei aus einem Zip lesen.
pub fn zip_read(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok()?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

pub fn open_zip(path: &Path) -> AppResult<zip::ZipArchive<std::fs::File>> {
    let f = std::fs::File::open(path)?;
    zip::ZipArchive::new(f).map_err(|e| AppError::internal(format!("Jar ungültig: {e}")))
}

/// PNG dekodieren, animierte Texturen (hoch) auf das erste Quadrat zuschneiden, neu kodieren.
pub fn first_frame_png(raw: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(raw, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let img = if h > w && w > 0 { image::imageops::crop_imm(&img, 0, 0, w, w).to_image() } else { img };
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// Texturpfade (ohne `assets/minecraft/textures/`) für ein Item.
fn texture_candidates(name: &str) -> Vec<String> {
    let mut v = vec![
        format!("item/{name}"),
        format!("item/{name}_00"),
        format!("item/{name}_standby"),
        format!("block/{name}"),
        format!("block/{name}_top"),
        format!("block/{name}_front"),
        format!("block/{name}_side"),
    ];
    for c in map::block_texture_candidates(name) {
        let p = format!("block/{c}");
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

pub async fn h_texture(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((id, item)): AxPath<(String, String)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let c = ctx(&app, &id).await?;
    let item = item.trim().trim_end_matches(".png").to_ascii_lowercase();
    let (ns, name) = item.split_once(':').unwrap_or(("minecraft", item.as_str()));
    if ns != "minecraft" {
        return Err(AppError::not_found("Keine Textur (Mod-Gegenstand)"));
    }
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-') || name.contains("..") {
        return Err(AppError::bad("Ungültige Item-ID"));
    }
    let name = name.to_string();
    let jar = ensure_client_jar(&app, c.cfg.version.as_deref()).await?;
    let version = c.cfg.version.clone().unwrap_or_default();
    let tex_dir = client_dir(&app, &version).join("textures");
    let cached = tex_dir.join(format!("{name}.png"));
    let missing = tex_dir.join(format!("{name}.missing"));
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        let mt = tokio::fs::metadata(&cached).await.ok().as_ref().and_then(mtime_secs);
        return Ok(png_response(bytes, "public, max-age=86400", mt, &headers));
    }
    if missing.exists() {
        return Err(AppError::not_found("Keine Textur gefunden"));
    }
    let name2 = name.clone();
    let found = tokio::task::spawn_blocking(move || -> AppResult<Option<Vec<u8>>> {
        let mut zip = open_zip(&jar)?;
        for cand in texture_candidates(&name2) {
            if let Some(raw) = zip_read(&mut zip, &format!("assets/minecraft/textures/{cand}.png")) {
                if let Some(png) = first_frame_png(&raw) {
                    return Ok(Some(png));
                }
            }
        }
        Ok(None)
    })
    .await??;
    let _ = tokio::fs::create_dir_all(&tex_dir).await;
    match found {
        Some(png) => {
            let _ = tokio::fs::write(&cached, &png).await;
            Ok(png_response(png, "public, max-age=86400", None, &headers))
        }
        None => {
            let _ = tokio::fs::write(&missing, b"").await;
            Err(AppError::not_found("Keine Textur gefunden"))
        }
    }
}

/// Item-Namen aus dem Client-Jar.
fn item_names(jar: &Path) -> AppResult<Vec<String>> {
    let zip = open_zip(jar)?;
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    let pick = |prefix: &str| -> Vec<String> {
        names
            .iter()
            .filter_map(|n| n.strip_prefix(prefix)?.strip_suffix(".json"))
            .filter(|n| !n.contains('/'))
            .map(str::to_string)
            .collect()
    };
    let mut items = pick("assets/minecraft/items/");
    if items.is_empty() {
        static NUM_SUFFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"_\d+$").expect("regex"));
        items = pick("assets/minecraft/models/item/")
            .into_iter()
            .filter(|n| {
                !n.starts_with("template_")
                    && !matches!(n.as_str(), "generated" | "handheld" | "handheld_rod" | "handheld_mace" | "amethyst_bud")
                    && !n.contains("_pulling_")
                    && !n.ends_with("_in_hand")
                    && !n.ends_with("_cast")
                    && !NUM_SUFFIX.is_match(n)
            })
            .collect();
    }
    let mut items: Vec<String> = items.into_iter().map(|n| format!("minecraft:{n}")).collect();
    items.sort();
    items.dedup();
    Ok(items)
}

pub async fn h_items(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<ItemCatalog>> {
    let c = ctx(&app, &id).await?;
    let jar = ensure_client_jar(&app, c.cfg.version.as_deref()).await?;
    let cache = jar.with_file_name("items.json");
    if let Ok(s) = tokio::fs::read_to_string(&cache).await {
        if let Ok(items) = serde_json::from_str::<Vec<String>>(&s) {
            return Ok(Json(ItemCatalog { items }));
        }
    }
    let items = tokio::task::spawn_blocking(move || item_names(&jar)).await??;
    if let Ok(s) = serde_json::to_string(&items) {
        let _ = tokio::fs::write(&cache, s).await;
    }
    Ok(Json(ItemCatalog { items }))
}

/* ------------------------------ Köpfe ------------------------------ */

static HEAD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_\-]{1,36}$").expect("regex"));

/// Graues 8×8-Ersatzbild.
static FALLBACK_HEAD: Lazy<Vec<u8>> = Lazy::new(|| {
    let img = image::RgbaImage::from_fn(8, 8, |x, y| {
        let v = if (x + y) % 2 == 0 { 120 } else { 132 };
        image::Rgba([v, v, v, 255])
    });
    let mut out = Vec::new();
    let _ = img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png);
    out
});

pub async fn h_head(State(app): State<AppState>, _a: Authed, AxPath(player): AxPath<String>, headers: HeaderMap) -> AppResult<Response> {
    let player = player.trim().trim_end_matches(".png").to_string();
    if !HEAD_RE.is_match(&player) {
        return Ok(png_response(FALLBACK_HEAD.clone(), "public, max-age=3600", None, &headers));
    }
    let dir = app.config.cache_dir().join("heads");
    let file = dir.join(format!("{}.png", player.to_ascii_lowercase()));
    let meta = tokio::fs::metadata(&file).await.ok();
    let fresh = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(86400));
    if fresh {
        if let Ok(bytes) = tokio::fs::read(&file).await {
            return Ok(png_response(bytes, "public, max-age=3600", meta.as_ref().and_then(mtime_secs), &headers));
        }
    }
    let url = format!("https://mc-heads.net/avatar/{player}/64");
    let fetched = async {
        let r = app.http.get(&url).timeout(Duration::from_secs(10)).send().await.ok()?.error_for_status().ok()?;
        let b = r.bytes().await.ok()?;
        b.starts_with(b"\x89PNG").then(|| b.to_vec())
    }
    .await;
    match fetched {
        Some(bytes) => {
            let _ = tokio::fs::create_dir_all(&dir).await;
            let tmp = file.with_extension("part");
            if tokio::fs::write(&tmp, &bytes).await.is_ok() {
                let _ = tokio::fs::rename(&tmp, &file).await;
            }
            Ok(png_response(bytes, "public, max-age=3600", None, &headers))
        }
        None => match tokio::fs::read(&file).await {
            Ok(old) => Ok(png_response(old, "public, max-age=600", None, &headers)),
            Err(_) => Ok(png_response(FALLBACK_HEAD.clone(), "public, max-age=600", None, &headers)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_animated() {
        let img = image::RgbaImage::from_pixel(16, 64, image::Rgba([10, 20, 30, 255]));
        let mut raw = Vec::new();
        img.write_to(&mut Cursor::new(&mut raw), image::ImageFormat::Png).unwrap();
        let out = first_frame_png(&raw).unwrap();
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (16, 16));
        assert!(!FALLBACK_HEAD.is_empty());
    }
}
