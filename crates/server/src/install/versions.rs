//! Versionslisten und Download-Adressen (Mojang, PaperMC, Purpur, Fabric, Forge, NeoForge,
//! Factorio). Antworten werden 10 Minuten im Speicher zwischengespeichert.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Mutex,
    time::{Duration, Instant},
};

use nrz_gp_common::{Flavor, Game, VersionEntry, VersionList};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    import::detect::{cmp_versions, neoforge_mc},
    state::AppState,
};

const TTL: Duration = Duration::from_secs(600);

pub const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const PAPER_API: &str = "https://fill.papermc.io/v3/projects";
const PURPUR_API: &str = "https://api.purpurmc.org/v2/purpur";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
const NEOFORGE_VERSIONS: &str = "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
const FORGE_PROMOS: &str = "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const FORGE_METADATA: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const FACTORIO_RELEASES: &str = "https://factorio.com/api/latest-releases";

static CACHE: Lazy<Mutex<HashMap<String, (Instant, String)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Text holen (mit 10-Minuten-Cache).
pub async fn fetch_text(app: &AppState, url: &str) -> AppResult<String> {
    if let Some((t, body)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get(url) {
        if t.elapsed() < TTL {
            return Ok(body.clone());
        }
    }
    let body = app.http.get(url).timeout(Duration::from_secs(30)).send().await?.error_for_status()?.text().await?;
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(url.to_string(), (Instant::now(), body.clone()));
    Ok(body)
}

pub async fn fetch_json(app: &AppState, url: &str) -> AppResult<Value> {
    let raw = fetch_text(app, url).await?;
    serde_json::from_str(&raw).map_err(|e| AppError::internal(format!("Ungültige Antwort von {url}: {e}")))
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Reine Release-Version (`1.21.4`, `26.1`) – keine Snapshots/Vorabversionen.
pub fn is_release(v: &str) -> bool {
    !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') && v.contains('.')
}

fn entries(mut ids: Vec<String>, stable: impl Fn(&str) -> bool) -> Vec<VersionEntry> {
    ids.sort_by(|a, b| cmp_versions(b, a));
    ids.dedup();
    ids.into_iter().map(|id| VersionEntry { stable: stable(&id), id }).collect()
}

/// Forge-Spielversion und Forge-Version aus einer Maven-Version (`1.7.10-10.13.4.1614-1.7.10`).
pub fn split_forge(full: &str) -> Option<(String, String)> {
    let (mc, rest) = full.split_once('-')?;
    let forge = rest.strip_suffix(&format!("-{mc}")).unwrap_or(rest);
    Some((mc.to_string(), forge.to_string()))
}

static XML_VERSION: Lazy<Regex> = Lazy::new(|| Regex::new(r"<version>([^<]+)</version>").unwrap());

/// Alle Forge-Maven-Versionen (`<mc>-<forge>[-<mc>]`).
pub async fn forge_all(app: &AppState) -> AppResult<Vec<String>> {
    let xml = fetch_text(app, FORGE_METADATA).await?;
    Ok(XML_VERSION.captures_iter(&xml).map(|c| c[1].to_string()).collect())
}

pub async fn forge_promos(app: &AppState) -> AppResult<HashMap<String, String>> {
    let v = fetch_json(app, FORGE_PROMOS).await?;
    Ok(v.get("promos")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default())
}

/// Forge-Versionen für eine Spielversion, neueste zuerst.
fn forge_for(all: &[String], mc: &str) -> Vec<String> {
    let mut v: Vec<String> = all.iter().filter_map(|f| split_forge(f)).filter(|(m, _)| m == mc).map(|(_, f)| f).collect();
    v.sort_by(|a, b| cmp_versions(b, a));
    v.dedup();
    v
}

/// NeoForge-Versionen (alle, wie von Maven geliefert).
pub async fn neoforge_all(app: &AppState) -> AppResult<Vec<String>> {
    let v = fetch_json(app, NEOFORGE_VERSIONS).await?;
    Ok(v.get("versions").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

fn neoforge_for(all: &[String], mc: &str) -> Vec<String> {
    let mut v: Vec<String> = all.iter().filter(|n| neoforge_mc(n).as_deref() == Some(mc)).cloned().collect();
    v.sort_by(|a, b| cmp_versions(b, a));
    v
}

/// Fabric-Loader-Versionen (neueste zuerst, nur stabile).
pub async fn fabric_loaders(app: &AppState) -> AppResult<Vec<String>> {
    let v = fetch_json(app, &format!("{FABRIC_META}/versions/loader")).await?;
    let all: Vec<&Value> = v.as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let stable: Vec<String> = all.iter().filter(|x| x.get("stable").and_then(Value::as_bool).unwrap_or(true)).filter_map(|x| str_of(x, "version")).collect();
    Ok(if stable.is_empty() { all.iter().filter_map(|x| str_of(x, "version")).collect() } else { stable })
}

/// Neuester Fabric-Installer.
pub async fn fabric_installer(app: &AppState) -> AppResult<String> {
    let v = fetch_json(app, &format!("{FABRIC_META}/versions/installer")).await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    arr.iter()
        .find(|x| x.get("stable").and_then(Value::as_bool).unwrap_or(true))
        .or(arr.first())
        .and_then(|x| str_of(x, "version"))
        .ok_or_else(|| AppError::internal("Kein Fabric-Installer gefunden"))
}

pub fn fabric_server_jar_url(game: &str, loader: &str, installer: &str) -> String {
    format!("{FABRIC_META}/versions/loader/{game}/{loader}/{installer}/server/jar")
}

pub fn purpur_download_url(v: &str) -> String {
    format!("{PURPUR_API}/{v}/latest/download")
}

pub fn neoforge_installer_url(v: &str) -> String {
    format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{v}/neoforge-{v}-installer.jar")
}

pub fn forge_installer_url(full: &str) -> String {
    format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{full}/forge-{full}-installer.jar")
}

/// Download-Adresse des neuesten Paper-/Folia-Builds.
pub async fn paper_download_url(app: &AppState, project: &str, v: &str) -> AppResult<String> {
    let b = fetch_json(app, &format!("{PAPER_API}/{project}/versions/{v}/builds/latest")).await?;
    b.get("downloads")
        .and_then(|d| d.get("server:default"))
        .and_then(|d| str_of(d, "url"))
        .ok_or_else(|| AppError::internal(format!("Kein {project}-Build für {v} gefunden")))
}

/// Download-Adresse des Vanilla-Servers.
pub async fn vanilla_download_url(app: &AppState, v: &str) -> AppResult<String> {
    let m = fetch_json(app, MOJANG_MANIFEST).await?;
    let url = m
        .get("versions")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x.get("id").and_then(Value::as_str) == Some(v)))
        .and_then(|x| str_of(x, "url"))
        .ok_or_else(|| AppError::bad(format!("Minecraft-Version {v} nicht gefunden")))?;
    let meta = fetch_json(app, &url).await?;
    meta.get("downloads")
        .and_then(|d| d.get("server"))
        .and_then(|s| str_of(s, "url"))
        .ok_or_else(|| AppError::bad(format!("Für Minecraft {v} gibt es keinen offiziellen Server")))
}

/// Versionsliste (neueste zuerst) inkl. Loader-Versionen für `game_version`.
pub async fn version_list(app: &AppState, game: Game, flavor: Flavor, game_version: Option<&str>) -> AppResult<VersionList> {
    if game == Game::Factorio || flavor == Flavor::Factorio {
        let v = fetch_json(app, FACTORIO_RELEASES).await?;
        let get = |ch: &str| v.get(ch).and_then(|c| c.get("headless")).and_then(Value::as_str).map(str::to_string);
        let mut versions = Vec::new();
        if let Some(s) = get("stable") {
            versions.push(VersionEntry { id: s, stable: true });
        }
        if let Some(e) = get("experimental") {
            if !versions.iter().any(|x| x.id == e) {
                versions.insert(0, VersionEntry { id: e, stable: false });
            }
        }
        return Ok(VersionList { versions, loader_versions: Vec::new() });
    }
    match flavor {
        Flavor::Vanilla => {
            let m = fetch_json(app, MOJANG_MANIFEST).await?;
            let versions = m
                .get("versions")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            let id = str_of(x, "id")?;
                            match x.get("type").and_then(Value::as_str) {
                                Some("release") => Some(VersionEntry { id, stable: true }),
                                Some("snapshot") => Some(VersionEntry { id, stable: false }),
                                _ => None,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(VersionList { versions, loader_versions: Vec::new() })
        }
        Flavor::Paper | Flavor::Folia => {
            let project = if flavor == Flavor::Folia { "folia" } else { "paper" };
            let p = fetch_json(app, &format!("{PAPER_API}/{project}")).await?;
            let mut ids = Vec::new();
            match p.get("versions") {
                Some(Value::Object(m)) => {
                    for list in m.values() {
                        ids.extend(list.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)));
                    }
                }
                Some(Value::Array(a)) => ids.extend(a.iter().filter_map(|x| x.as_str().map(str::to_string))),
                _ => {}
            }
            ids.retain(|v| is_release(v));
            Ok(VersionList { versions: entries(ids, |_| true), loader_versions: Vec::new() })
        }
        Flavor::Purpur => {
            let p = fetch_json(app, PURPUR_API).await?;
            let mut ids: Vec<String> = p.get("versions").and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
            ids.retain(|v| is_release(v));
            Ok(VersionList { versions: entries(ids, |_| true), loader_versions: Vec::new() })
        }
        Flavor::Fabric => {
            let g = fetch_json(app, &format!("{FABRIC_META}/versions/game")).await?;
            let versions: Vec<VersionEntry> = g
                .as_array()
                .into_iter()
                .flatten()
                .filter(|x| x.get("stable").and_then(Value::as_bool).unwrap_or(false))
                .filter_map(|x| str_of(x, "version"))
                .map(|id| VersionEntry { id, stable: true })
                .collect();
            Ok(VersionList { versions, loader_versions: fabric_loaders(app).await? })
        }
        Flavor::NeoForge => {
            let all = neoforge_all(app).await?;
            let mut stable_mc = BTreeSet::new();
            let mut ids = Vec::new();
            for v in &all {
                if let Some(mc) = neoforge_mc(v) {
                    if !v.contains("beta") && !v.contains("alpha") {
                        stable_mc.insert(mc.clone());
                    }
                    ids.push(mc);
                }
            }
            let versions = entries(ids, |id| stable_mc.contains(id));
            let mc = game_version.map(str::to_string).or_else(|| versions.iter().find(|v| v.stable).or(versions.first()).map(|v| v.id.clone()));
            let loader_versions = mc.map(|mc| neoforge_for(&all, &mc)).unwrap_or_default();
            Ok(VersionList { versions, loader_versions })
        }
        Flavor::Forge => {
            let all = forge_all(app).await?;
            let ids: Vec<String> = all.iter().filter_map(|f| split_forge(f)).map(|(mc, _)| mc).filter(|mc| is_release(mc)).collect();
            let versions = entries(ids, |_| true);
            let mc = game_version.map(str::to_string).or_else(|| versions.first().map(|v| v.id.clone()));
            let loader_versions = mc.map(|mc| forge_for(&all, &mc)).unwrap_or_default();
            Ok(VersionList { versions, loader_versions })
        }
        other => Err(AppError::bad(format!("Für {} gibt es keine Versionsliste", other.label()))),
    }
}

/// Neueste stabile Version (bzw. neueste überhaupt).
pub async fn latest(app: &AppState, game: Game, flavor: Flavor) -> AppResult<String> {
    let l = version_list(app, game, flavor, None).await?;
    l.versions
        .iter()
        .find(|v| v.stable)
        .or(l.versions.first())
        .map(|v| v.id.clone())
        .ok_or_else(|| AppError::internal("Keine Version gefunden"))
}

/// Forge-Version wählen: angegeben → empfohlen → neueste. Ergebnis: volle Maven-Version.
pub async fn resolve_forge(app: &AppState, mc: &str, wanted: Option<&str>) -> AppResult<(String, String)> {
    let all = forge_all(app).await?;
    let forge = match wanted.filter(|w| !w.trim().is_empty()) {
        Some(w) => w.trim().to_string(),
        None => {
            let promos = forge_promos(app).await.unwrap_or_default();
            promos
                .get(&format!("{mc}-recommended"))
                .or_else(|| promos.get(&format!("{mc}-latest")))
                .cloned()
                .or_else(|| forge_for(&all, mc).into_iter().next())
                .ok_or_else(|| AppError::bad(format!("Keine Forge-Version für Minecraft {mc} gefunden")))?
        }
    };
    let full = all
        .iter()
        .find(|f| split_forge(f).is_some_and(|(m, x)| m == mc && x == forge))
        .cloned()
        .unwrap_or_else(|| format!("{mc}-{forge}"));
    Ok((forge, full))
}

/// NeoForge-Version wählen: angegeben → neueste stabile für die Spielversion → neueste.
pub async fn resolve_neoforge(app: &AppState, mc: Option<&str>, wanted: Option<&str>) -> AppResult<String> {
    if let Some(w) = wanted.filter(|w| !w.trim().is_empty()) {
        return Ok(w.trim().to_string());
    }
    let all = neoforge_all(app).await?;
    let mc = match mc {
        Some(m) => m.to_string(),
        None => latest(app, Game::Minecraft, Flavor::NeoForge).await?,
    };
    let list = neoforge_for(&all, &mc);
    list.iter()
        .find(|v| !v.contains("beta") && !v.contains("alpha"))
        .or(list.first())
        .cloned()
        .ok_or_else(|| AppError::bad(format!("Keine NeoForge-Version für Minecraft {mc} gefunden")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_split_and_filter() {
        assert_eq!(split_forge("1.20.4-49.0.30"), Some(("1.20.4".into(), "49.0.30".into())));
        assert_eq!(split_forge("1.7.10-10.13.4.1614-1.7.10"), Some(("1.7.10".into(), "10.13.4.1614".into())));
        let all = vec!["1.20.4-49.0.3".to_string(), "1.20.4-49.0.30".into(), "1.20.1-47.2.0".into()];
        assert_eq!(forge_for(&all, "1.20.4"), vec!["49.0.30", "49.0.3"]);
        assert!(is_release("1.21.4") && is_release("26.1") && !is_release("24w14a") && !is_release("1.21-pre1"));
        let neo = vec!["21.1.1".to_string(), "21.1.77".into(), "21.0.5-beta".into()];
        assert_eq!(neoforge_for(&neo, "1.21.1"), vec!["21.1.77", "21.1.1"]);
    }
}
