//! RCON-Client (Source-RCON-Protokoll; Minecraft und Factorio).

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const AUTH: i32 = 3;
const EXEC: i32 = 2;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RconClient {
    stream: TcpStream,
    next_id: i32,
}

impl RconClient {
    pub async fn connect(port: u16, password: &str) -> anyhow::Result<Self> {
        let stream = timeout(IO_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await??;
        stream.set_nodelay(true).ok();
        let mut c = Self { stream, next_id: 1 };
        let id = c.send(AUTH, password).await?;
        // Antwort(en) lesen: Minecraft schickt direkt AUTH_RESPONSE (Typ 2), Factorio/Source zuerst
        // ein leeres RESPONSE_VALUE (Typ 0).
        loop {
            let (rid, kind, _) = c.read_packet().await?;
            if kind == 2 {
                if rid == -1 {
                    anyhow::bail!("RCON-Passwort falsch");
                }
                if rid == id {
                    break;
                }
            }
        }
        Ok(c)
    }

    async fn send(&mut self, kind: i32, body: &str) -> anyhow::Result<i32> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let len = 4 + 4 + body.len() + 2;
        let mut buf = Vec::with_capacity(len + 4);
        buf.extend_from_slice(&(len as i32).to_le_bytes());
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&kind.to_le_bytes());
        buf.extend_from_slice(body.as_bytes());
        buf.extend_from_slice(&[0, 0]);
        timeout(IO_TIMEOUT, self.stream.write_all(&buf)).await??;
        Ok(id)
    }

    async fn read_packet(&mut self) -> anyhow::Result<(i32, i32, String)> {
        let mut len = [0u8; 4];
        timeout(IO_TIMEOUT, self.stream.read_exact(&mut len)).await??;
        let len = i32::from_le_bytes(len);
        if !(10..=1_048_576).contains(&len) {
            anyhow::bail!("Ungültiges RCON-Paket ({len} Bytes)");
        }
        let mut data = vec![0u8; len as usize];
        timeout(IO_TIMEOUT, self.stream.read_exact(&mut data)).await??;
        let id = i32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let kind = i32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let body = String::from_utf8_lossy(&data[8..data.len().saturating_sub(2)]).to_string();
        Ok((id, kind, body))
    }

    /// Befehl ausführen und Antwort lesen. Minecraft verarbeitet nur ein Paket pro Lesevorgang,
    /// darum wird erst nach der ersten Antwort weitergemacht: Ist sie (fast) voll (4096 Bytes),
    /// folgt ein leeres Paket als Endmarke, bis zu der die Fortsetzungen gesammelt werden.
    pub async fn exec(&mut self, command: &str) -> anyhow::Result<String> {
        let id = self.send(EXEC, command).await?;
        let mut out = String::new();
        loop {
            let (rid, _kind, body) = self.read_packet().await?;
            if rid == id {
                let full = body.len() >= 4000;
                out.push_str(&body);
                if !full {
                    return Ok(strip_formatting(&out));
                }
                break;
            }
        }
        let marker = self.send(EXEC, "").await?;
        loop {
            let (rid, _kind, body) = self.read_packet().await?;
            if rid == id {
                out.push_str(&body);
            } else if rid == marker {
                break;
            }
        }
        Ok(strip_formatting(&out))
    }
}

/// Entfernt Minecraft-Farbcodes (`§a`).
pub fn strip_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Gegen einen laufenden Server: `RCON_TEST=port:passwort cargo test rcon_live -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn rcon_live() {
        let v = std::env::var("RCON_TEST").unwrap();
        let (port, pw) = v.split_once(':').unwrap();
        let mut c = super::RconClient::connect(port.parse().unwrap(), pw).await.unwrap();
        let out = c.exec("list").await.unwrap();
        assert!(out.contains("players online"), "{out}");
    }
}
