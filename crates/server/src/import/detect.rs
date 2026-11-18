//! Erkennung eines Server-Ordners (synchron, ohne Panel-Zustand): Spiel, Software, Version,
//! Startdatei-Kandidaten, RAM/JVM-Argumente, Port, Welten.
//!
//! Robust: nie panic, Zips defensiv lesen, keine rekursive Suche (nur gezielte Pfade).

use std::{
    cmp::Ordering,
    collections::HashMap,
    fs,
    io::Read,
    path::Path,
};

use nrz_gp_common::{Confidence, Flavor, Game, ImportAnalysis, LaunchCandidate, LaunchKind};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::util;

/// Startskripte, die nach `-jar …`, `-Xmx` und JVM-Flags durchsucht werden.
pub const SCRIPTS: [&str; 4] = ["start.sh", "run.sh", "start.bat", "start.command"];

/// Bekannte Hybrid-Server (Forge/Fabric + Bukkit), am Dateinamen erkannt.
const HYBRIDS: [&str; 10] = ["mohist", "arclight", "magma", "catserver", "ketting", "crucible", "banner", "youer", "cardboard", "thermos"];

/// Ordnernamen, die als Vorschlag für den Servernamen nichts taugen.
const GENERIC_NAMES: [&str; 10] = ["server", "minecraft", "mc", "data", "factorio", "srv", "game", "server1", "minecraft-server", "mcserver"];

const MAX_ZIP_ENTRY: u64 = 1024 * 1024;

/* ------------------------------------------------------------------ */
/*                         Schneller Check                            */
/* ------------------------------------------------------------------ */

/// Sieht der Ordner nach einem Spielserver aus? (nur wenige Dateisystem-Zugriffe)
pub fn looks_like_server(dir: &Path) -> bool {
    if dir.join("server.properties").is_file()
        || dir.join("eula.txt").is_file()
        || dir.join("bin/x64/factorio").is_file()
        || dir.join("data/base/info.json").is_file()
    {
        return true;
    }
    let Ok(rd) = fs::read_dir(dir) else { return false };
    for e in rd.flatten().take(400) {
        let name = e.file_name().to_string_lossy().to_lowercase();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_file() && name.ends_with(".jar") {
            return true;
        }
        if ft.is_dir() && !name.starts_with('.') && e.path().join("level.dat").is_file() {
            return true;
        }
    }
    false
}

/* ------------------------------------------------------------------ */
/*                             Helfer                                 */
/* ------------------------------------------------------------------ */

/// Dateinamen mit Endung (klein geschrieben verglichen) direkt im Ordner, sortiert.
fn files_with_ext(dir: &Path, ext: &str) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) || e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.to_lowercase().ends_with(ext) && !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

/// Unterordner (ohne versteckte), sortiert.
fn subdirs(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

/// Höchsten Eintrag nach Versionsreihenfolge.
fn newest<'a>(items: impl IntoIterator<Item = &'a String>) -> Option<&'a String> {
    items.into_iter().max_by(|a, b| cmp_versions(a, b))
}

fn looks_like_version(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_digit()) && s.contains('.')
}

/// Versionen vergleichen: `1.20.10` > `1.20.4`, `1.20.4` > `1.20.4-rc1`.
pub fn cmp_versions(a: &str, b: &str) -> Ordering {
    let split = |s: &str| s.split(['.', '-', '_', '+']).filter(|p| !p.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (pa, pb) = (split(a), split(b));
    for (x, y) in pa.iter().zip(pb.iter()) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Greater,
            (Err(_), Ok(_)) => Ordering::Less,
            _ => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    // Längere Version: numerischer Rest = neuer (1.20.1 > 1.20), sonst Vorabversion (1.20-rc1 < 1.20)
    let rest_cmp = |longer: &[String]| if longer.first().is_some_and(|p| p.parse::<u64>().is_ok()) { Ordering::Greater } else { Ordering::Less };
    match pa.len().cmp(&pb.len()) {
        Ordering::Greater => rest_cmp(&pa[pb.len()..]),
        Ordering::Less => rest_cmp(&pb[pa.len()..]).reverse(),
        Ordering::Equal => Ordering::Equal,
    }
}

/// Text-Datei aus einem Zip/Jar lesen (höchstens 1 MB). Fehler → None.
pub fn read_zip_text(path: &Path, entry: &str) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
    let e = z.by_name(entry).ok()?;
    let mut buf = Vec::new();
    e.take(MAX_ZIP_ENTRY).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// `META-INF/MANIFEST.MF` zerlegen (Fortsetzungszeilen beginnen mit einem Leerzeichen).
pub fn parse_manifest(raw: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(cont) = line.strip_prefix(' ') {
            if let Some(last) = out.last_mut() {
                last.1.push_str(cont);
            }
        } else if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

fn manifest_value<'a>(m: &'a [(String, String)], key: &str) -> Option<&'a str> {
    m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

/// `4G` → 4096, `4096M` → 4096, `1048576k` → 1024, `2147483648` (Bytes) → 2048.
pub fn parse_mem_mb(s: &str) -> Option<u32> {
    let s = s.trim();
    let (num, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&s[..i], s[i..].to_ascii_lowercase()),
        None => (s, String::new()),
    };
    let n: u64 = num.parse().ok()?;
    let mb = match unit.as_str() {
        "g" | "gb" => n * 1024,
        "m" | "mb" => n,
        "k" | "kb" => n / 1024,
        "t" | "tb" => n * 1024 * 1024,
        "" => n / (1024 * 1024),
        _ => return None,
    };
    u32::try_from(mb).ok().filter(|v| *v > 0)
}

/// NeoForge-Version → Minecraft-Version: `20.4.237` → `1.20.4`, `21.0.1` → `1.21`,
/// `21.1.77` → `1.21.1`, `26.1.0.5` → `26.1`, `26.1.1.3` → `26.1.1`.
pub fn neoforge_mc(v: &str) -> Option<String> {
    let base = v.split(['-', '+']).next()?;
    let p: Vec<u32> = base.split('.').map(|x| x.parse().ok()).collect::<Option<Vec<_>>>()?;
    if p.len() < 2 {
        return None;
    }
    if p[0] >= 26 {
        if p.len() >= 4 && p[2] != 0 { Some(format!("{}.{}.{}", p[0], p[1], p[2])) } else { Some(format!("{}.{}", p[0], p[1])) }
    } else if p[0] >= 20 {
        if p[1] == 0 { Some(format!("1.{}", p[0])) } else { Some(format!("1.{}.{}", p[0], p[1])) }
    } else {
        None
    }
}

/// Anzeigename aus einem Ordnernamen: `my_cool-server` → `My Cool Server`.
pub fn pretty_name(s: &str) -> String {
    s.split(['-', '_', ' ', '.'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Minecraft-Farbcodes (`§a`) entfernen.
fn strip_formatting(s: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for c in s.chars() {
        if skip {
            skip = false;
        } else if c == '§' {
            skip = true;
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/* ------------------------------------------------------------------ */
/*                          Startskripte                              */
/* ------------------------------------------------------------------ */

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScriptInfo {
    /// Per `-jar X` referenzierte Jars (ohne `./`).
    pub jars: Vec<String>,
    /// Per `@…unix_args.txt` referenzierte Argumentdateien.
    pub args_files: Vec<String>,
    pub xmx_mb: Option<u32>,
    /// `-D…`/`-XX:…`-Flags (ohne -Xmx/-Xms/-jar/nogui).
    pub jvm_flags: Vec<String>,
}

static ASSIGN: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^(?:export\s+|set\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$"#).unwrap());
static VAR_REF: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)|%([A-Za-z_][A-Za-z0-9_]*)%").unwrap());

/// Startskript auswerten (einfache Variablen wie `RAM=4G` … `-Xmx$RAM` werden aufgelöst).
pub fn parse_script(raw: &str) -> ScriptInfo {
    let is_comment = |l: &str| {
        let lower = l.to_ascii_lowercase();
        l.starts_with('#') || l.starts_with("::") || lower.starts_with("rem ") || lower.starts_with("@rem")
    };
    let mut vars: HashMap<String, String> = HashMap::new();
    for line in raw.lines().map(str::trim) {
        if is_comment(line) {
            continue;
        }
        if let Some(c) = ASSIGN.captures(line) {
            let v = c[2].trim().trim_matches(['"', '\'']).to_string();
            vars.insert(c[1].to_string(), v);
        }
    }
    let mut info = ScriptInfo::default();
    for line in raw.lines().map(str::trim) {
        if line.is_empty() || is_comment(line) {
            continue;
        }
        let line = VAR_REF.replace_all(line, |c: &regex::Captures| {
            let name = c.get(1).or(c.get(2)).or(c.get(3)).map_or("", |m| m.as_str());
            vars.get(name).cloned().unwrap_or_else(|| c[0].to_string())
        });
        let is_assign = ASSIGN.is_match(&line);
        let mut tokens: Vec<String> = Vec::new();
        for t in util::split_args(&line) {
            // `JAVA_OPTS=-Xmx4G -XX:…` → Wert erneut zerlegen
            match ASSIGN.captures(&t) {
                Some(c) if c[2].starts_with('-') => tokens.extend(util::split_args(&c[2])),
                _ => tokens.push(t),
            }
        }
        let relevant = line.contains("java") || line.contains("-jar") || line.contains('@') || is_assign;
        if !relevant {
            continue;
        }
        let mut i = 0;
        while i < tokens.len() {
            let t = tokens[i].as_str();
            if t == "-jar" {
                if let Some(j) = tokens.get(i + 1) {
                    let j = j.trim_start_matches("./").replace('\\', "/");
                    if j.to_lowercase().ends_with(".jar") && !info.jars.contains(&j) {
                        info.jars.push(j);
                    }
                }
                i += 2;
                continue;
            }
            if let Some(a) = t.strip_prefix('@') {
                if a.ends_with("unix_args.txt") || a.ends_with("win_args.txt") {
                    let a = a.replace('\\', "/").replace("win_args.txt", "unix_args.txt");
                    if !info.args_files.contains(&a) {
                        info.args_files.push(a);
                    }
                }
            } else if let Some(m) = t.strip_prefix("-Xmx") {
                if let Some(mb) = parse_mem_mb(m) {
                    info.xmx_mb = Some(mb);
                }
            } else if (t.starts_with("-D") || t.starts_with("-XX:")) && !t.contains('$') && !t.contains('%') && !info.jvm_flags.iter().any(|f| f == t) {
                info.jvm_flags.push(t.to_string());
            }
            i += 1;
        }
    }
    info
}

/// Alle Startskripte im Ordner zusammengefasst (+ `user_jvm_args.txt` für -Xmx).
pub fn scripts_info(dir: &Path) -> (ScriptInfo, Vec<String>) {
    let mut all = ScriptInfo::default();
    let mut found = Vec::new();
    for s in SCRIPTS {
        let Ok(raw) = fs::read_to_string(dir.join(s)) else { continue };
        found.push(s.to_string());
        let info = parse_script(&raw);
        for j in info.jars {
            if !all.jars.contains(&j) {
                all.jars.push(j);
            }
        }
        for a in info.args_files {
            if !all.args_files.contains(&a) {
                all.args_files.push(a);
            }
        }
        all.xmx_mb = all.xmx_mb.or(info.xmx_mb);
        for f in info.jvm_flags {
            if !all.jvm_flags.contains(&f) {
                all.jvm_flags.push(f);
            }
        }
    }
    if let Ok(raw) = fs::read_to_string(dir.join("user_jvm_args.txt")) {
        for t in raw.lines().map(str::trim).filter(|l| !l.starts_with('#')).flat_map(util::split_args) {
            if let Some(mb) = t.strip_prefix("-Xmx").and_then(parse_mem_mb) {
                all.xmx_mb = Some(mb);
            }
        }
    }
    (all, found)
}

/* ------------------------------------------------------------------ */
/*                        Jar-Klassifizierung                         */
/* ------------------------------------------------------------------ */

/// Jar anhand von Dateiname und `Main-Class` einordnen → (Software, Grundpunkte, Begründung).
pub fn classify_jar(file_name: &str, main_class: Option<&str>) -> (Flavor, i32, String) {
    let lower = file_name.to_lowercase();
    if let Some(h) = HYBRIDS.iter().find(|h| lower.contains(*h)) {
        return (Flavor::Unknown, 70, format!("Hybrid-Server ({h}) am Dateinamen erkannt"));
    }
    let Some(mc) = main_class.map(str::trim).filter(|m| !m.is_empty()) else {
        // Ohne Manifest nur am Namen raten.
        let guess = if lower.starts_with("paper") {
            Some(Flavor::Paper)
        } else if lower.starts_with("purpur") {
            Some(Flavor::Purpur)
        } else if lower.starts_with("spigot") {
            Some(Flavor::Spigot)
        } else if lower.starts_with("fabric-server") {
            Some(Flavor::Fabric)
        } else if lower.starts_with("forge-") {
            Some(Flavor::Forge)
        } else if lower == "server.jar" || lower.starts_with("minecraft_server") {
            Some(Flavor::Vanilla)
        } else {
            None
        };
        return match guess {
            Some(f) => (f, 30, format!("am Dateinamen erkannt ({})", f.label())),
            None => (Flavor::Unknown, 5, "keine Main-Class".into()),
        };
    };
    let reason = format!("Main-Class {mc}");
    if mc.starts_with("io.papermc.paperclip") || mc.starts_with("com.destroystokyo.paperclip") {
        let f = if lower.starts_with("purpur") {
            Flavor::Purpur
        } else if lower.starts_with("folia") {
            Flavor::Folia
        } else {
            Flavor::Paper
        };
        (f, 80, reason)
    } else if mc.starts_with("net.fabricmc.") {
        (Flavor::Fabric, 85, reason)
    } else if mc.starts_with("org.quiltmc.") {
        (Flavor::Quilt, 85, reason)
    } else if matches!(mc, "net.minecraft.server.Main" | "net.minecraft.bundler.Main" | "net.minecraft.server.MinecraftServer") {
        (Flavor::Vanilla, 60, reason)
    } else if mc.starts_with("org.bukkit.craftbukkit") {
        (if lower.starts_with("spigot") { Flavor::Spigot } else { Flavor::Bukkit }, 75, reason)
    } else if mc.starts_with("net.minecraftforge.") || mc.starts_with("cpw.mods.") {
        (Flavor::Forge, 80, reason)
    } else if mc.starts_with("net.neoforged.") {
        (Flavor::NeoForge, 80, reason)
    } else {
        (Flavor::Unknown, 10, format!("unbekannte Main-Class {mc}"))
    }
}

/// Main-Class eines Jars (None bei Fehlern).
pub fn jar_main_class(path: &Path) -> Option<String> {
    let raw = read_zip_text(path, "META-INF/MANIFEST.MF")?;
    manifest_value(&parse_manifest(&raw), "Main-Class").map(str::to_string)
}

/* ------------------------------------------------------------------ */
/*                       Startdatei-Kandidaten                        */
/* ------------------------------------------------------------------ */

/// Argumentdateien von Forge ≥ 1.17 und NeoForge (relativ, neueste zuerst).
fn args_files(dir: &Path) -> Vec<(String, Flavor, String)> {
    let mut out = Vec::new();
    for (base, flavor) in [("libraries/net/neoforged/neoforge", Flavor::NeoForge), ("libraries/net/minecraftforge/forge", Flavor::Forge)] {
        let mut vers: Vec<String> = subdirs(&dir.join(base)).into_iter().filter(|v| dir.join(base).join(v).join("unix_args.txt").is_file()).collect();
        vers.sort_by(|a, b| cmp_versions(b, a));
        for v in vers {
            out.push((format!("{base}/{v}/unix_args.txt"), flavor, v));
        }
    }
    out
}

/// Alle möglichen Startdateien, beste zuerst.
pub fn launch_candidates(dir: &Path, scripts: &ScriptInfo, script_files: &[String]) -> Vec<LaunchCandidate> {
    let mut out: Vec<LaunchCandidate> = Vec::new();
    for (i, (path, flavor, ver)) in args_files(dir).into_iter().enumerate() {
        let mut score = if i == 0 { 100 } else { 95 };
        let mut reason = format!("{} {ver} (Argumentdatei)", flavor.label());
        if scripts.args_files.iter().any(|a| a == &path) {
            score += 10;
            reason.push_str(", im Startskript referenziert");
        }
        out.push(LaunchCandidate { launch: LaunchKind::ArgsFile { args_file: path }, flavor, reason, score });
    }
    for jar in files_with_ext(dir, ".jar") {
        let lower = jar.to_lowercase();
        if lower.contains("installer") || lower.ends_with("-sources.jar") {
            continue;
        }
        let main = jar_main_class(&dir.join(&jar));
        let (flavor, mut score, mut reason) = classify_jar(&jar, main.as_deref());
        if scripts.jars.iter().any(|j| j.eq_ignore_ascii_case(&jar)) {
            score += 30;
            reason.push_str(", im Startskript referenziert");
        }
        out.push(LaunchCandidate { launch: LaunchKind::Jar { jar }, flavor, reason, score });
    }
    for s in script_files.iter().filter(|s| !s.ends_with(".bat")) {
        out.push(LaunchCandidate {
            launch: LaunchKind::Script { script: s.clone() },
            flavor: out.first().map_or(Flavor::Unknown, |c| c.flavor),
            reason: "eigenes Startskript".into(),
            score: 15,
        });
    }
    out.sort_by(|a, b| b.score.cmp(&a.score));
    out
}

/* ------------------------------------------------------------------ */
/*                          Versionserkennung                         */
/* ------------------------------------------------------------------ */

static LOG_VERSION: Lazy<Regex> = Lazy::new(|| Regex::new(r"Starting minecraft server version (\S+)").unwrap());
static MC_IN_MANIFEST: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(MC: ([^)\s]+)\)").unwrap());
static VERSION_IN_TEXT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|[-_.\s])((?:1\.\d+(?:\.\d+)?)|(?:2\d\.\d+(?:\.\d+)?))(?:[-_.\s]|$)").unwrap());
static FABRIC_JAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"^fabric-server-mc\.([^-]+)-loader\.([^-]+)-launcher").unwrap());
static FORGE_JAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"^forge-(1\.[\d.]+)-([\d.]+)").unwrap());

fn json_id(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get("id").or_else(|| v.get("name")).and_then(|x| x.as_str()).map(str::to_string)
}

/// Version aus einem Jar (version.json, Paperclip `versions.list`, Manifest `(MC: …)`).
fn version_from_jar(path: &Path) -> Option<(String, &'static str)> {
    if let Some(id) = read_zip_text(path, "version.json").and_then(|r| json_id(&r)) {
        // Forge-/Fabric-Jars: `1.12.2-forge-14.23.5.2860` → `1.12.2`
        let lower = id.to_lowercase();
        let id = if lower.contains("forge") || lower.contains("fabric") { id.split('-').next().unwrap_or(&id).to_string() } else { id };
        return Some((id, "version.json"));
    }
    if let Some(list) = read_zip_text(path, "META-INF/versions.list") {
        for line in list.lines() {
            for field in line.split('\t').skip(1) {
                if let Some(c) = VERSION_IN_TEXT.captures(field) {
                    return Some((c[1].to_string(), "META-INF/versions.list"));
                }
            }
        }
    }
    if let Some(m) = read_zip_text(path, "META-INF/MANIFEST.MF") {
        let m = parse_manifest(&m);
        if let Some(c) = manifest_value(&m, "Implementation-Version").and_then(|v| MC_IN_MANIFEST.captures(v)) {
            return Some((c[1].to_string(), "Manifest"));
        }
    }
    None
}

/// Minecraft-Version + Quelle; Reihenfolge siehe Modul-Doku.
pub fn detect_mc_version(dir: &Path, jars_by_score: &[String]) -> Option<(String, String)> {
    // 1. logs/latest.log
    if let Ok(f) = fs::File::open(dir.join("logs/latest.log")) {
        let mut buf = Vec::new();
        if f.take(256 * 1024).read_to_end(&mut buf).is_ok() {
            if let Some(c) = LOG_VERSION.captures(&String::from_utf8_lossy(&buf)) {
                return Some((c[1].to_string(), "logs/latest.log".into()));
            }
        }
    }
    // 2. Server-Jars (beste Kandidaten zuerst)
    for jar in jars_by_score.iter().filter(|j| !j.to_lowercase().contains("installer")) {
        if let Some((v, src)) = version_from_jar(&dir.join(jar)) {
            return Some((v, format!("{jar} ({src})")));
        }
        if let Some(c) = FABRIC_JAR.captures(jar) {
            return Some((c[1].to_string(), format!("Dateiname {jar}")));
        }
    }
    // 3. versions/<ver>/ (Paper, Vanilla-Bundler)
    if let Some(v) = newest(&subdirs(&dir.join("versions")).into_iter().filter(|v| looks_like_version(v)).collect::<Vec<_>>()) {
        return Some((v.clone(), format!("versions/{v}/")));
    }
    // 4. libraries/net/minecraft/server/<ver>/
    if let Some(v) = newest(&subdirs(&dir.join("libraries/net/minecraft/server")).into_iter().filter(|v| looks_like_version(v)).collect::<Vec<_>>()) {
        let v = v.split('-').next().unwrap_or(v).to_string();
        return Some((v, "libraries/net/minecraft/server/".into()));
    }
    // 5. .fabric/server/<ver>-server.jar
    let fabric: Vec<String> = files_with_ext(&dir.join(".fabric/server"), ".jar").into_iter().filter_map(|n| n.strip_suffix("-server.jar").map(str::to_string)).collect();
    if let Some(v) = newest(&fabric) {
        return Some((v.clone(), ".fabric/server/".into()));
    }
    // 6. Forge-Bibliothek <mc>-<forge>
    if let Some(v) = newest(&subdirs(&dir.join("libraries/net/minecraftforge/forge"))) {
        if let Some((mc, _)) = v.split_once('-') {
            return Some((mc.to_string(), "libraries/net/minecraftforge/forge/".into()));
        }
    }
    // 7. NeoForge-Bibliothek
    if let Some(mc) = newest(&subdirs(&dir.join("libraries/net/neoforged/neoforge"))).and_then(|v| neoforge_mc(v)) {
        return Some((mc, "libraries/net/neoforged/neoforge/".into()));
    }
    // 8. Dateinamen (forge-1.12.2-…, minecraft_server.1.12.2.jar, paper-1.20.4-435.jar)
    for jar in jars_by_score {
        if let Some(c) = FORGE_JAR.captures(jar) {
            return Some((c[1].trim_end_matches('.').to_string(), format!("Dateiname {jar}")));
        }
        if let Some(c) = VERSION_IN_TEXT.captures(jar.trim_end_matches(".jar")) {
            return Some((c[1].to_string(), format!("Dateiname {jar}")));
        }
    }
    None
}

/// Loader-Version (Forge, NeoForge, Fabric), falls erkennbar.
pub fn detect_loader_version(dir: &Path, flavor: Flavor, jars: &[String]) -> Option<String> {
    match flavor {
        Flavor::Forge => {
            if let Some(v) = newest(&subdirs(&dir.join("libraries/net/minecraftforge/forge"))) {
                return v.split_once('-').map(|(_, f)| f.to_string());
            }
            jars.iter().find_map(|j| FORGE_JAR.captures(j).map(|c| c[2].trim_end_matches('.').to_string()))
        }
        Flavor::NeoForge => newest(&subdirs(&dir.join("libraries/net/neoforged/neoforge"))).cloned(),
        Flavor::Fabric => {
            if let Some(v) = jars.iter().find_map(|j| FABRIC_JAR.captures(j).map(|c| c[2].to_string())) {
                return Some(v);
            }
            newest(&subdirs(&dir.join("libraries/net/fabricmc/fabric-loader"))).cloned()
        }
        _ => None,
    }
}

/* ------------------------------------------------------------------ */
/*                            Analyse                                 */
/* ------------------------------------------------------------------ */

/// Ordner analysieren (ohne Java-Auswahl und Panel-Abgleich, das macht `import::analyze`).
pub fn analyze_dir(dir: &Path) -> ImportAnalysis {
    let mut a = ImportAnalysis { path: dir.to_string_lossy().into(), ..Default::default() };
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if dir.join("bin/x64/factorio").is_file() || dir.join("data/base/info.json").is_file() {
        analyze_factorio(dir, &mut a);
    } else {
        analyze_minecraft(dir, &mut a);
    }
    if a.suggested_name.is_empty() {
        a.suggested_name = pretty_name(&folder);
    }
    if a.suggested_name.is_empty() {
        a.suggested_name = "Importierter Server".into();
    }
    a
}

fn is_generic(folder: &str) -> bool {
    GENERIC_NAMES.contains(&folder.to_lowercase().as_str())
}

fn analyze_factorio(dir: &Path, a: &mut ImportAnalysis) {
    a.game = Some(Game::Factorio);
    a.flavor = Flavor::Factorio;
    if let Some(v) = fs::read_to_string(dir.join("data/base/info.json"))
        .ok()
        .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
        .and_then(|j| j.get("version").and_then(|v| v.as_str()).map(str::to_string))
    {
        a.version = Some(v);
        a.version_source = Some("data/base/info.json".into());
    }
    if dir.join("bin/x64/factorio").is_file() {
        let launch = LaunchKind::Factorio { binary: "bin/x64/factorio".into(), save: None };
        a.candidates.push(LaunchCandidate { launch: launch.clone(), flavor: Flavor::Factorio, reason: "Factorio headless (bin/x64/factorio)".into(), score: 100 });
        a.launch = Some(launch);
    } else {
        a.notes.push("Programm bin/x64/factorio fehlt – ist das eine Linux-Installation (headless)?".into());
    }
    a.port = Some(
        fs::read_to_string(dir.join("config/config.ini"))
            .ok()
            .and_then(|raw| {
                raw.lines().map(str::trim).filter(|l| !l.starts_with(';') && !l.starts_with('#')).find_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    if k.trim() == "port" { v.trim().parse().ok() } else { None }
                })
            })
            .unwrap_or(Game::Factorio.default_port()),
    );
    let mut saves: Vec<(String, std::time::SystemTime)> = files_with_ext(&dir.join("saves"), ".zip")
        .into_iter()
        .map(|n| {
            let t = fs::metadata(dir.join("saves").join(&n)).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            (n.trim_end_matches(".zip").to_string(), t)
        })
        .collect();
    saves.sort_by(|x, y| y.1.cmp(&x.1));
    a.worlds = saves.into_iter().map(|(n, _)| n).collect();
    if a.worlds.is_empty() {
        a.notes.push("Kein Spielstand in saves/ – vor dem ersten Start eine Karte erzeugen.".into());
    }
    a.mod_count = files_with_ext(&dir.join("mods"), ".zip").len() as u32;
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if is_generic(&folder) {
        let settings = ["data/server-settings.json", "server-settings.json", "config/server-settings.json"]
            .iter()
            .find_map(|p| fs::read_to_string(dir.join(p)).ok())
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok());
        if let Some(n) = settings.and_then(|s| s.get("name").and_then(|n| n.as_str()).map(str::to_string)).filter(|n| !n.trim().is_empty() && n != "Name of the game as it will appear in the game listing") {
            a.suggested_name = n;
        }
    }
    if std::env::consts::ARCH != "x86_64" {
        a.notes.push("Factorio headless läuft nur auf Linux x86_64 – dieser Host hat eine andere Architektur.".into());
    }
    a.confidence = if a.launch.is_some() && a.version.is_some() { Confidence::High } else if a.launch.is_some() { Confidence::Medium } else { Confidence::Low };
}

fn analyze_minecraft(dir: &Path, a: &mut ImportAnalysis) {
    let props = util::read_properties(&dir.join("server.properties"));
    let (scripts, script_files) = scripts_info(dir);
    let candidates = launch_candidates(dir, &scripts, &script_files);
    let jars_by_score: Vec<String> = {
        let mut v: Vec<String> = candidates
            .iter()
            .filter_map(|c| match &c.launch {
                LaunchKind::Jar { jar } => Some(jar.clone()),
                _ => None,
            })
            .collect();
        // Installer zuletzt trotzdem für die Versionssuche (Dateiname) berücksichtigen
        for j in files_with_ext(dir, ".jar") {
            if !v.contains(&j) {
                v.push(j);
            }
        }
        v
    };
    let worlds: Vec<String> = subdirs(dir).into_iter().filter(|d| dir.join(d).join("level.dat").is_file()).collect();
    let has_marker = !props.is_empty() || dir.join("eula.txt").is_file() || !worlds.is_empty();
    let strong_candidate = candidates.iter().any(|c| c.score >= 30 && !matches!(c.launch, LaunchKind::Script { .. }));

    if !has_marker && !strong_candidate {
        a.candidates = candidates;
        if a.candidates.is_empty() {
            a.notes.push("Kein Server gefunden – weder server.properties noch eine Server-Jar oder Factorio.".into());
        } else {
            a.notes.push("Kein eindeutiger Server gefunden – bitte Spiel und Startdatei selbst wählen.".into());
            a.launch = a.candidates.first().map(|c| c.launch.clone());
        }
        return;
    }

    a.game = Some(Game::Minecraft);
    let best = candidates.first().cloned();
    a.flavor = best.as_ref().map_or(Flavor::Unknown, |c| c.flavor);
    a.launch = best.as_ref().map(|c| c.launch.clone());

    if let Some((v, src)) = detect_mc_version(dir, &jars_by_score) {
        a.version = Some(v);
        a.version_source = Some(src);
    }
    a.loader_version = detect_loader_version(dir, a.flavor, &jars_by_score);
    a.memory_mb = scripts.xmx_mb;
    if !scripts.jvm_flags.is_empty() {
        a.jvm_args = Some(scripts.jvm_flags.join(" "));
    }
    a.port = util::value(&props, "server-port").and_then(|p| p.trim().parse().ok()).or(Some(Game::Minecraft.default_port()));
    // Hauptwelt (level-name) zuerst
    let level = util::value(&props, "level-name").unwrap_or("world").to_string();
    let mut worlds = worlds;
    if let Some(i) = worlds.iter().position(|w| *w == level) {
        let w = worlds.remove(i);
        worlds.insert(0, w);
    }
    a.worlds = worlds;
    a.eula_accepted = Some(crate::process::eula_accepted(dir));
    a.has_rcon = util::value(&props, "enable-rcon").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
    a.plugin_count = files_with_ext(&dir.join("plugins"), ".jar").len() as u32;
    a.mod_count = files_with_ext(&dir.join("mods"), ".jar").len() as u32;

    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let motd = util::value(&props, "motd").map(strip_formatting).filter(|m| !m.is_empty() && m != "A Minecraft Server" && m.chars().count() <= 40);
    a.suggested_name = match motd {
        Some(m) if is_generic(&folder) || folder.is_empty() => m,
        _ => pretty_name(&folder),
    };

    // Hinweise
    match &best {
        None => a.notes.push("Keine Startdatei gefunden – bitte Server-Jar oder Startskript angeben.".into()),
        Some(b) => {
            if let Some(second) = candidates.get(1) {
                if second.score >= b.score - 5 && !matches!(second.launch, LaunchKind::Script { .. }) {
                    a.notes.push(format!("Mehrere mögliche Startdateien – gewählt: {}. Bitte prüfen.", b.launch.describe()));
                }
            }
            if matches!(b.launch, LaunchKind::Script { .. }) {
                a.notes.push("Nur ein Startskript gefunden – Live-Werte wie RAM und Java werden dann vom Skript bestimmt.".into());
            }
            if b.flavor == Flavor::Unknown {
                a.notes.push("Server-Software nicht eindeutig erkannt.".into());
            }
        }
    }
    for j in &scripts.jars {
        if !dir.join(j).is_file() && !j.contains('$') && !j.contains('%') {
            a.notes.push(format!("Startskript verweist auf {j}, die Datei fehlt."));
        }
    }
    if a.version.is_none() {
        a.notes.push("Minecraft-Version nicht erkannt – bitte angeben (wichtig für die Java-Auswahl).".into());
    }
    if a.eula_accepted == Some(false) {
        a.notes.push("Die Minecraft-EULA ist noch nicht akzeptiert.".into());
    }
    if !a.has_rcon {
        a.notes.push("RCON ist aus – beim Import einschalten, damit Spielerliste und Live-Daten funktionieren.".into());
    }
    let clear_launch = best.as_ref().is_some_and(|b| b.score >= 60);
    a.confidence = if a.version.is_some() && clear_launch {
        Confidence::High
    } else if a.version.is_some() || clear_launch {
        Confidence::Medium
    } else {
        Confidence::Low
    };
    a.candidates = candidates;
}

/* ------------------------------------------------------------------ */
/*                               Tests                                */
/* ------------------------------------------------------------------ */

#[cfg(test)]
pub(crate) mod tests {
    use std::{io::Write, path::PathBuf};

    use super::*;

    /// Temporärer Ordner, wird beim Drop gelöscht.
    pub struct TempDir(pub PathBuf);
    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("nrzgp-test-{tag}-{}", crate::store::new_id(6)));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    pub fn write_jar(path: &Path, files: &[(&str, &str)]) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        let f = fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for (name, content) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(content.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    fn manifest(main: &str) -> String {
        format!("Manifest-Version: 1.0\r\nMain-Class: {main}\r\n\r\n")
    }

    #[test]
    fn neoforge_to_mc() {
        assert_eq!(neoforge_mc("20.4.237").as_deref(), Some("1.20.4"));
        assert_eq!(neoforge_mc("21.1.77").as_deref(), Some("1.21.1"));
        assert_eq!(neoforge_mc("21.0.0-beta").as_deref(), Some("1.21"));
        assert_eq!(neoforge_mc("20.2.3-beta").as_deref(), Some("1.20.2"));
        assert_eq!(neoforge_mc("26.1.0.5").as_deref(), Some("26.1"));
        assert_eq!(neoforge_mc("26.1.12").as_deref(), Some("26.1"));
        assert_eq!(neoforge_mc("26.1.1.3").as_deref(), Some("26.1.1"));
        assert_eq!(neoforge_mc("abc"), None);
    }

    #[test]
    fn manifest_classification() {
        let m = parse_manifest("Manifest-Version: 1.0\r\nMain-Class: io.papermc.paperc\r\n lip.Main\r\n");
        assert_eq!(manifest_value(&m, "main-class"), Some("io.papermc.paperclip.Main"));
        assert_eq!(classify_jar("paper-1.20.4.jar", Some("io.papermc.paperclip.Main")).0, Flavor::Paper);
        assert_eq!(classify_jar("purpur-1.21.jar", Some("io.papermc.paperclip.Main")).0, Flavor::Purpur);
        assert_eq!(classify_jar("folia.jar", Some("io.papermc.paperclip.Main")).0, Flavor::Folia);
        assert_eq!(classify_jar("fabric-server-launch.jar", Some("net.fabricmc.loader.impl.launch.server.FabricServerLauncher")).0, Flavor::Fabric);
        assert_eq!(classify_jar("quilt-server-launch.jar", Some("org.quiltmc.loader.impl.launch.server.QuiltServerLauncher")).0, Flavor::Quilt);
        assert_eq!(classify_jar("server.jar", Some("net.minecraft.bundler.Main")).0, Flavor::Vanilla);
        assert_eq!(classify_jar("spigot-1.20.4.jar", Some("org.bukkit.craftbukkit.bootstrap.Main")).0, Flavor::Spigot);
        assert_eq!(classify_jar("craftbukkit.jar", Some("org.bukkit.craftbukkit.Main")).0, Flavor::Bukkit);
        assert_eq!(classify_jar("forge-1.12.2-14.23.5.2860.jar", Some("net.minecraftforge.fml.relauncher.ServerLaunchWrapper")).0, Flavor::Forge);
        let (f, score, _) = classify_jar("mohist-1.20.1.jar", Some("com.mohistmc.MohistMCStart"));
        assert_eq!(f, Flavor::Unknown);
        assert!(score >= 60);
        assert!(classify_jar("fabric-server-launch.jar", Some("net.fabricmc.x")).1 > classify_jar("server.jar", Some("net.minecraft.server.Main")).1);
    }

    #[test]
    fn xmx_parsing() {
        assert_eq!(parse_mem_mb("4G"), Some(4096));
        assert_eq!(parse_mem_mb("4096M"), Some(4096));
        assert_eq!(parse_mem_mb("2g"), Some(2048));
        assert_eq!(parse_mem_mb("1048576k"), Some(1024));
        assert_eq!(parse_mem_mb("2147483648"), Some(2048));
        assert_eq!(parse_mem_mb("x"), None);
        let s = parse_script("#!/bin/sh\nRAM=6G\njava -Xms1G -Xmx$RAM -XX:+UseG1GC -Dlog4j2.formatMsgNoLookups=true -jar ./paper.jar nogui\n");
        assert_eq!(s.xmx_mb, Some(6144));
        assert_eq!(s.jars, vec!["paper.jar"]);
        assert_eq!(s.jvm_flags, vec!["-XX:+UseG1GC", "-Dlog4j2.formatMsgNoLookups=true"]);
        let s = parse_script("@echo off\r\nrem Kommentar -Xmx1G\r\njava -Xmx3072M @user_jvm_args.txt @libraries/net/minecraftforge/forge/1.20.1-47.2.0/win_args.txt %*\r\n");
        assert_eq!(s.xmx_mb, Some(3072));
        assert_eq!(s.args_files, vec!["libraries/net/minecraftforge/forge/1.20.1-47.2.0/unix_args.txt"]);
        let s = parse_script("JAVA_OPTS=\"-Xmx8G -XX:+UseZGC\"\njava $JAVA_OPTS -jar server.jar\n");
        assert_eq!(s.xmx_mb, Some(8192));
        assert_eq!(s.jvm_flags, vec!["-XX:+UseZGC"]);
    }

    #[test]
    fn version_order() {
        assert_eq!(cmp_versions("1.20.10", "1.20.4"), Ordering::Greater);
        assert_eq!(cmp_versions("1.20.4", "1.20.4-rc1"), Ordering::Greater);
        assert_eq!(cmp_versions("1.21", "1.21.1"), Ordering::Less);
        assert_eq!(cmp_versions("26.1", "1.21.4"), Ordering::Greater);
    }

    #[test]
    fn ranking_fabric_over_vanilla() {
        let t = TempDir::new("fabric");
        let d = &t.0;
        write_jar(&d.join("server.jar"), &[("META-INF/MANIFEST.MF", &manifest("net.minecraft.bundler.Main")), ("version.json", r#"{"id":"1.20.4","name":"1.20.4"}"#)]);
        write_jar(&d.join("fabric-server-launch.jar"), &[("META-INF/MANIFEST.MF", &manifest("net.fabricmc.loader.impl.launch.server.FabricServerLauncher"))]);
        write_jar(&d.join("fabric-installer-1.0.1.jar"), &[("META-INF/MANIFEST.MF", &manifest("net.fabricmc.installer.Main"))]);
        fs::write(d.join("server.properties"), "server-port=25570\nenable-rcon=false\nmotd=Hallo\n").unwrap();
        fs::write(d.join("eula.txt"), "eula=true\n").unwrap();
        fs::create_dir_all(d.join("world")).unwrap();
        fs::write(d.join("world/level.dat"), b"x").unwrap();
        let a = analyze_dir(d);
        assert_eq!(a.game, Some(Game::Minecraft));
        assert_eq!(a.flavor, Flavor::Fabric);
        assert_eq!(a.launch, Some(LaunchKind::Jar { jar: "fabric-server-launch.jar".into() }));
        assert!(!a.candidates.iter().any(|c| matches!(&c.launch, LaunchKind::Jar { jar } if jar.contains("installer"))));
        assert_eq!(a.version.as_deref(), Some("1.20.4"));
        assert_eq!(a.port, Some(25570));
        assert_eq!(a.worlds, vec!["world"]);
        assert_eq!(a.eula_accepted, Some(true));
        assert_eq!(a.confidence, Confidence::High);
    }

    #[test]
    fn ranking_script_bonus_and_paper() {
        let t = TempDir::new("paper");
        let d = &t.0;
        write_jar(&d.join("paper-1.20.4-435.jar"), &[("META-INF/MANIFEST.MF", &manifest("io.papermc.paperclip.Main"))]);
        write_jar(&d.join("old-server.jar"), &[("META-INF/MANIFEST.MF", &manifest("io.papermc.paperclip.Main"))]);
        fs::write(d.join("start.sh"), "#!/bin/sh\njava -Xmx4G -jar old-server.jar nogui\n").unwrap();
        fs::create_dir_all(d.join("versions/1.20.4")).unwrap();
        let a = analyze_dir(d);
        assert_eq!(a.launch, Some(LaunchKind::Jar { jar: "old-server.jar".into() }));
        assert_eq!(a.flavor, Flavor::Paper);
        assert_eq!(a.memory_mb, Some(4096));
        assert_eq!(a.version.as_deref(), Some("1.20.4"));
        assert_eq!(a.version_source.as_deref(), Some("versions/1.20.4/"));
    }

    #[test]
    fn ranking_neoforge_args_file() {
        let t = TempDir::new("neo");
        let d = &t.0;
        let args = d.join("libraries/net/neoforged/neoforge/21.1.77");
        fs::create_dir_all(&args).unwrap();
        fs::write(args.join("unix_args.txt"), "-cp x").unwrap();
        fs::write(d.join("run.sh"), "#!/usr/bin/env sh\njava @user_jvm_args.txt @libraries/net/neoforged/neoforge/21.1.77/unix_args.txt \"$@\"\n").unwrap();
        fs::write(d.join("user_jvm_args.txt"), "# -Xmx2G\n-Xmx6G\n").unwrap();
        write_jar(&d.join("neoforge-21.1.77-installer.jar"), &[("META-INF/MANIFEST.MF", &manifest("net.minecraftforge.installer.SimpleInstaller"))]);
        fs::write(d.join("eula.txt"), "eula=false\n").unwrap();
        let a = analyze_dir(d);
        assert_eq!(a.flavor, Flavor::NeoForge);
        assert_eq!(a.launch, Some(LaunchKind::ArgsFile { args_file: "libraries/net/neoforged/neoforge/21.1.77/unix_args.txt".into() }));
        assert_eq!(a.version.as_deref(), Some("1.21.1"));
        assert_eq!(a.loader_version.as_deref(), Some("21.1.77"));
        assert_eq!(a.memory_mb, Some(6144));
        assert_eq!(a.eula_accepted, Some(false));
        assert_eq!(a.candidates.iter().filter(|c| matches!(c.launch, LaunchKind::Jar { .. })).count(), 0);
    }

    #[test]
    fn script_only_and_empty() {
        let t = TempDir::new("script");
        let d = &t.0;
        fs::write(d.join("start.sh"), "#!/bin/sh\n./something\n").unwrap();
        fs::write(d.join("server.properties"), "motd=§aMein Server\n").unwrap();
        let a = analyze_dir(d);
        assert_eq!(a.game, Some(Game::Minecraft));
        assert_eq!(a.launch, Some(LaunchKind::Script { script: "start.sh".into() }));
        let e = TempDir::new("empty");
        let a = analyze_dir(&e.0);
        assert_eq!(a.game, None);
        assert!(a.notes.iter().any(|n| n.contains("Kein Server gefunden")));
        assert!(!looks_like_server(&e.0));
        assert!(looks_like_server(d));
    }

    #[test]
    fn factorio_detect() {
        let t = TempDir::new("factorio");
        let d = &t.0;
        fs::create_dir_all(d.join("bin/x64")).unwrap();
        fs::write(d.join("bin/x64/factorio"), b"").unwrap();
        fs::create_dir_all(d.join("data/base")).unwrap();
        fs::write(d.join("data/base/info.json"), r#"{"name":"base","version":"2.0.28"}"#).unwrap();
        fs::create_dir_all(d.join("saves")).unwrap();
        fs::write(d.join("saves/world.zip"), b"").unwrap();
        let a = analyze_dir(d);
        assert_eq!(a.game, Some(Game::Factorio));
        assert_eq!(a.version.as_deref(), Some("2.0.28"));
        assert_eq!(a.port, Some(34197));
        assert_eq!(a.worlds, vec!["world"]);
        assert!(matches!(a.launch, Some(LaunchKind::Factorio { .. })));
    }
}
