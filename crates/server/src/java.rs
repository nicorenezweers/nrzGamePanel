//! Java-Installationen finden, passend zur Minecraft-Version wählen und bei Bedarf
//! automatisch herunterladen (Eclipse Temurin nach `<data>/java/temurin-<major>`).

use std::path::{Path, PathBuf};

use nrz_gp_common::{Flavor, JavaInstall};
use once_cell::sync::Lazy;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
    util,
};

/// Ordner für automatisch geladene Java-Versionen.
pub fn java_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("java")
}

/// Sucht Java in `/opt/java/*` (Docker-Image), `/usr/lib/jvm/*`, macOS-JVMs, `JAVA_HOME`,
/// `NRZ_GP_JAVA_DIRS`, `extra_dirs` (z. B. `<data>/java`) und `PATH`.
pub fn detect(extra_dirs: &[PathBuf]) -> Vec<JavaInstall> {
    let mut homes: Vec<PathBuf> = Vec::new();
    let mut roots: Vec<PathBuf> = vec!["/opt/java".into(), "/usr/lib/jvm".into(), "/usr/java".into(), "/Library/Java/JavaVirtualMachines".into()];
    roots.extend(extra_dirs.iter().cloned());
    if let Ok(extra) = std::env::var("NRZ_GP_JAVA_DIRS") {
        roots.extend(extra.split(',').map(PathBuf::from));
    }
    if let Some(h) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(h).join(".sdkman/candidates/java"));
    }
    for root in roots {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let p = e.path();
                let mac = p.join("Contents/Home");
                homes.push(if mac.is_dir() { mac } else { p });
            }
        }
        if root.join("bin/java").exists() {
            homes.push(root);
        }
    }
    if let Some(jh) = std::env::var_os("JAVA_HOME") {
        homes.push(PathBuf::from(jh));
    }
    let mut out: Vec<JavaInstall> = Vec::new();
    for home in homes {
        let bin = home.join("bin/java");
        if !bin.is_file() {
            continue;
        }
        let bin = std::fs::canonicalize(&bin).unwrap_or(bin);
        if out.iter().any(|j| Path::new(&j.path) == bin) {
            continue;
        }
        if let Some(j) = from_release_file(&home, &bin) {
            out.push(j);
        }
    }
    // `java` im PATH (falls nicht schon gefunden)
    if let Some(p) = which("java") {
        let real = std::fs::canonicalize(&p).unwrap_or(p.clone());
        if !out.iter().any(|j| Path::new(&j.path) == real) {
            if let Some(j) = real.parent().and_then(Path::parent).and_then(|h| from_release_file(h, &real)).or_else(|| from_exec(&real)) {
                out.push(j);
            }
        }
    }
    out.sort_by(|a, b| a.major.cmp(&b.major));
    out
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn from_release_file(home: &Path, bin: &Path) -> Option<JavaInstall> {
    let raw = std::fs::read_to_string(home.join("release")).ok()?;
    let get = |k: &str| {
        raw.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=').map(|v| v.trim_matches('"').to_string()))
    };
    let version = get("JAVA_VERSION")?;
    Some(JavaInstall { path: bin.to_string_lossy().into(), major: major_of(&version)?, version, vendor: get("IMPLEMENTOR") })
}

fn from_exec(bin: &Path) -> Option<JavaInstall> {
    let out = std::process::Command::new(bin).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let version = text.split('"').nth(1)?.to_string();
    Some(JavaInstall { path: bin.to_string_lossy().into(), major: major_of(&version)?, version, vendor: None })
}

/// `1.8.0_392` → 8, `21.0.5` → 21.
pub fn major_of(v: &str) -> Option<u32> {
    let mut it = v.split(['.', '_', '-', '+']);
    let first: u32 = it.next()?.parse().ok()?;
    if first == 1 { it.next()?.parse().ok() } else { Some(first) }
}

/// Benötigte Java-Hauptversion für eine Minecraft-Version.
///
/// - bis 1.16.5 → 8 (alte Forge-Versionen laufen nur mit 8)
/// - 1.17 → 16, 1.18 – 1.20.4 → 17
/// - 1.20.5 – 1.21.x → 21
/// - neue Jahresversionen (26.1 …) → 25
/// - Snapshots (`24w14a`) nach Jahr
pub fn required_for(version: Option<&str>, flavor: Flavor) -> u32 {
    let _ = flavor;
    let Some(v) = version else { return 21 };
    if let Some((yy, _)) = v.split_once('w') {
        if let Ok(yy) = yy.parse::<u32>() {
            return match yy {
                0..=20 => 8,
                21 => 16,
                22 | 23 => 17,
                24 | 25 => 21,
                _ => 25,
            };
        }
    }
    let base = v.split(['-', ' ']).next().unwrap_or(v);
    let parts: Vec<u32> = base.split('.').filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [1, minor, rest @ ..] => {
            let patch = rest.first().copied().unwrap_or(0);
            match *minor {
                0..=16 => 8,
                17 => 16,
                18 | 19 => 17,
                20 if patch <= 4 => 17,
                _ => 21,
            }
        }
        [year, ..] if *year >= 26 => 25,
        _ => 21,
    }
}

/// Bestes Java für die benötigte Version: Java 8 möglichst genau (alte Forge-Versionen
/// brechen mit neuerem Java), sonst die kleinste Version ≥ benötigt, sonst die höchste.
pub fn pick(installs: &[JavaInstall], required: u32) -> Option<&JavaInstall> {
    if required <= 8 {
        if let Some(j) = installs.iter().find(|j| j.major == 8) {
            return Some(j);
        }
    }
    installs.iter().filter(|j| j.major >= required).min_by_key(|j| j.major).or_else(|| installs.iter().max_by_key(|j| j.major))
}

/// Passt eine vorhandene Installation? Java 8 nur exakt (alte Forge-Versionen), sonst ≥ benötigt.
pub fn suitable(installs: &[JavaInstall], required: u32) -> Option<&JavaInstall> {
    if required <= 8 {
        return installs.iter().find(|j| j.major == 8);
    }
    installs.iter().filter(|j| j.major >= required).min_by_key(|j| j.major)
}

pub fn has_suitable(installs: &[JavaInstall], required: u32) -> bool {
    suitable(installs, required).is_some()
}

static DOWNLOAD_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

/// Java neu suchen (inkl. `<data>/java`) und im Zustand speichern.
pub async fn redetect(app: &AppState) -> Vec<JavaInstall> {
    let extra = vec![java_dir(&app.config.data_dir)];
    let list = tokio::task::spawn_blocking(move || detect(&extra)).await.unwrap_or_default();
    *app.java.write().await = list.clone();
    list
}

/// Passendes Java sicherstellen (ohne Fortschrittsmeldungen).
#[allow(dead_code)] // Teil der öffentlichen API (z. B. MCP, Einstellungen)
pub async fn ensure_java(app: &AppState, major: u32) -> AppResult<JavaInstall> {
    ensure_java_with(app, major, &|_| {}).await
}

/// Temurin-Download-URL für dieses System.
fn temurin_url(major: u32) -> AppResult<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "mac",
        o => return Err(AppError::internal(format!("Automatischer Java-Download wird auf {o} nicht unterstützt"))),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        a => return Err(AppError::internal(format!("Automatischer Java-Download wird für {a} nicht unterstützt"))),
    };
    Ok(format!("https://api.adoptium.net/v3/binary/latest/{major}/ga/{os}/{arch}/jre/hotspot/normal/eclipse"))
}

/// Passendes Java sicherstellen: vorhandenes zurückgeben, sonst Eclipse Temurin (JRE) laden und
/// nach `<data>/java/temurin-<major>` entpacken. `progress` bekommt Meldungen für die Konsole.
pub async fn ensure_java_with(app: &AppState, major: u32, progress: &(dyn Fn(String) + Send + Sync)) -> AppResult<JavaInstall> {
    if let Some(j) = suitable(&app.java.read().await, major) {
        return Ok(j.clone());
    }
    let _guard = DOWNLOAD_LOCK.lock().await;
    // Evtl. hat ein anderer Aufruf schon geladen oder Java wurde nachinstalliert.
    if let Some(j) = suitable(&redetect(app).await, major).cloned() {
        return Ok(j);
    }
    let url = temurin_url(major)?;
    let base = java_dir(&app.config.data_dir);
    let target = base.join(format!("temurin-{major}"));
    let tmp = base.join(format!(".temurin-{major}.tmp"));
    let archive = app.config.cache_dir().join(format!("temurin-{major}.tar.gz"));
    tokio::fs::create_dir_all(&base).await?;
    progress(format!("Lade Java {major} (Eclipse Temurin) …"));
    let mut last = 0u64;
    util::download(&app.http, &url, &archive, |done, total| {
        if let Some(t) = total.filter(|t| *t > 0) {
            let pct = done * 100 / t;
            if pct >= last + 10 || (pct == 100 && last < 100) {
                last = pct - pct % 10;
                progress(format!("Java {major}: {pct} % ({} von {})", util::human_bytes(done), util::human_bytes(t)));
            }
        }
    })
    .await
    .map_err(|e| AppError::internal(format!("Java {major} konnte nicht geladen werden: {e}")))?;
    progress(format!("Entpacke Java {major} …"));
    let (a, t, dst) = (archive.clone(), tmp.clone(), target.clone());
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t)?;
        crate::install::archive::extract_strip(&a, &t)?;
        let _ = std::fs::remove_dir_all(&dst);
        std::fs::rename(&t, &dst)?;
        Ok(())
    })
    .await??;
    let _ = tokio::fs::remove_file(&archive).await;
    let list = redetect(app).await;
    let j = suitable(&list, major)
        .cloned()
        .ok_or_else(|| AppError::internal(format!("Java {major} wurde geladen, aber nicht gefunden ({})", target.display())))?;
    progress(format!("Java {} bereit: {}", j.version, j.path));
    Ok(j)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(required_for(Some("1.12.2"), Flavor::Forge), 8);
        assert_eq!(required_for(Some("1.18.2"), Flavor::Vanilla), 17);
        assert_eq!(required_for(Some("1.20.4"), Flavor::Paper), 17);
        assert_eq!(required_for(Some("1.20.6"), Flavor::Paper), 21);
        assert_eq!(required_for(Some("1.21.4"), Flavor::Fabric), 21);
        assert_eq!(required_for(Some("26.1"), Flavor::Vanilla), 25);
        assert_eq!(required_for(Some("24w14a"), Flavor::Vanilla), 21);
        assert_eq!(major_of("1.8.0_392"), Some(8));
        assert_eq!(major_of("21.0.5"), Some(21));
    }

    #[test]
    fn suitable_java() {
        let j = |m: u32| JavaInstall { path: format!("/j{m}"), major: m, version: m.to_string(), vendor: None };
        let list = vec![j(17), j(21)];
        assert!(suitable(&list, 8).is_none());
        assert_eq!(suitable(&list, 17).map(|x| x.major), Some(17));
        assert_eq!(suitable(&list, 18).map(|x| x.major), Some(21));
        assert!(suitable(&list, 25).is_none());
    }
}
