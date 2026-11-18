//! Hilfsfunktionen: sichere Pfade, `.properties`-Dateien, Ordnergrösse, Downloads.

use std::path::{Component, Path, PathBuf};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, AppResult};

/// Relativen Pfad (aus der Oberfläche) sicher an `base` anhängen. Verhindert `..`, absolute
/// Pfade und das Verlassen von `base` über Symlinks (soweit der Pfad existiert).
pub fn safe_join(base: &Path, rel: &str) -> AppResult<PathBuf> {
    let rel = rel.trim().trim_start_matches('/');
    let mut out = base.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => return Err(AppError::bad("Ungültiger Pfad")),
        }
    }
    if let (Ok(real_base), Ok(real)) = (std::fs::canonicalize(base), std::fs::canonicalize(&out)) {
        if !real.starts_with(&real_base) {
            return Err(AppError::Forbidden("Pfad liegt ausserhalb des Server-Ordners".into()));
        }
    }
    Ok(out)
}

/// Pfad relativ zu `base` mit `/` (für die Oberfläche).
pub fn rel_path(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/* ---------------------------- .properties ---------------------------- */

/// Einträge einer `.properties`-Datei (Reihenfolge wie in der Datei, ohne Kommentare).
pub fn read_properties(path: &Path) -> Vec<(String, String)> {
    let Ok(raw) = std::fs::read_to_string(path) else { return Vec::new() };
    parse_properties(&raw)
}

pub fn parse_properties(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .map(str::trim_start)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .filter_map(|l| {
            let i = l.find(['=', ':'])?;
            Some((l[..i].trim().to_string(), unescape(l[i + 1..].trim_start())))
        })
        .collect()
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) > 0x7e => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn value<'a>(entries: &'a [(String, String)], key: &str) -> Option<&'a str> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Werte setzen (vorhandene Zeilen ersetzen, neue anhängen); Kommentare bleiben erhalten.
pub fn set_properties(path: &Path, updates: &[(String, String)]) -> std::io::Result<()> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let mut done = vec![false; updates.len()];
    let mut lines: Vec<String> = raw
        .lines()
        .map(|line| {
            let t = line.trim_start();
            if t.starts_with('#') || t.starts_with('!') {
                return line.to_string();
            }
            if let Some(i) = t.find(['=', ':']) {
                let k = t[..i].trim();
                if let Some(pos) = updates.iter().position(|(uk, _)| uk == k) {
                    done[pos] = true;
                    return format!("{k}={}", escape(&updates[pos].1));
                }
            }
            line.to_string()
        })
        .collect();
    for (i, (k, v)) in updates.iter().enumerate() {
        if !done[i] {
            lines.push(format!("{k}={}", escape(v)));
        }
    }
    std::fs::write(path, lines.join("\n") + "\n")
}

/* ------------------------------ Diverses ----------------------------- */

/// Grösse eines Ordners (rekursiv, Symlinks nicht folgen).
pub fn dir_size(path: &Path) -> u64 {
    walkdir::WalkDir::new(path).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()
}

/// Ist ein TCP-Port auf dem Host frei? (Host-Netzwerk: gilt direkt für den Host.)
pub fn tcp_port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

pub fn udp_port_free(port: u16) -> bool {
    std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok()
}

/// Datei herunterladen (Streaming). `progress` bekommt (geladen, gesamt).
pub async fn download(
    http: &reqwest::Client,
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, Option<u64>),
) -> AppResult<()> {
    let resp = http.get(url).send().await?.error_for_status()?;
    let total = resp.content_length();
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = dest.with_extension("part");
    let mut f = tokio::fs::File::create(&tmp).await?;
    let mut stream = resp.bytes_stream();
    let mut done = 0u64;
    let mut last = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        f.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if done - last > 4 * 1024 * 1024 {
            last = done;
            progress(done, total);
        }
    }
    f.flush().await?;
    drop(f);
    progress(done, total);
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Bytes lesbar: `1.5 GB`.
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

/// Einfache Shell-Wort-Zerlegung (Leerzeichen, "…" und '…').
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_roundtrip() {
        let e = parse_properties("#c\nmotd=A Minecraft Server\nserver-port=25565\nx\\:y=1\n");
        assert_eq!(value(&e, "motd"), Some("A Minecraft Server"));
        assert_eq!(value(&e, "server-port"), Some("25565"));
    }

    #[test]
    fn safe_join_rejects_parent() {
        assert!(safe_join(Path::new("/tmp"), "../etc").is_err());
        assert!(safe_join(Path::new("/tmp"), "a/b").is_ok());
    }

    #[test]
    fn args() {
        assert_eq!(split_args(r#"-Xmx2G "-Dfoo=a b" -jar x.jar"#), vec!["-Xmx2G", "-Dfoo=a b", "-jar", "x.jar"]);
    }
}
