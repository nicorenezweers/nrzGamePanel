//! Server: Konfiguration, Status, Steuerung, Konsole, Backups.

use serde::{Deserialize, Serialize};

use crate::events::GameEvent;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Game {
    #[default]
    Minecraft,
    Factorio,
}

impl Game {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Minecraft => "Minecraft",
            Self::Factorio => "Factorio",
        }
    }
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Minecraft => 25565,
            Self::Factorio => 34197,
        }
    }
}

/// Server-Software (Minecraft) bzw. Variante.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Flavor {
    #[default]
    Vanilla,
    Paper,
    Purpur,
    Folia,
    Spigot,
    Bukkit,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    /// Factorio headless.
    Factorio,
    Unknown,
}

impl Flavor {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Paper => "Paper",
            Self::Purpur => "Purpur",
            Self::Folia => "Folia",
            Self::Spigot => "Spigot",
            Self::Bukkit => "CraftBukkit",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
            Self::Factorio => "Factorio",
            Self::Unknown => "Unbekannt",
        }
    }
    /// Lädt Plugins (`plugins/`) statt Mods.
    pub const fn uses_plugins(self) -> bool {
        matches!(self, Self::Paper | Self::Purpur | Self::Folia | Self::Spigot | Self::Bukkit)
    }
    /// Lädt Mods (`mods/`).
    pub const fn uses_mods(self) -> bool {
        matches!(self, Self::Fabric | Self::Quilt | Self::Forge | Self::NeoForge | Self::Factorio)
    }
    /// Loader-Name für Modrinth-Filter.
    pub const fn modrinth_loader(self) -> Option<&'static str> {
        match self {
            Self::Paper => Some("paper"),
            Self::Purpur => Some("purpur"),
            Self::Folia => Some("folia"),
            Self::Spigot => Some("spigot"),
            Self::Bukkit => Some("bukkit"),
            Self::Fabric => Some("fabric"),
            Self::Quilt => Some("quilt"),
            Self::Forge => Some("forge"),
            Self::NeoForge => Some("neoforge"),
            _ => None,
        }
    }
    /// Kann vom Panel neu installiert werden (Assistent „Server erstellen“).
    pub const fn installable() -> &'static [Flavor] {
        &[Self::Paper, Self::Vanilla, Self::Fabric, Self::Purpur, Self::NeoForge, Self::Forge]
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
    /// Wird gerade heruntergeladen / installiert.
    Installing,
}

impl ServerStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Gestoppt",
            Self::Starting => "Startet …",
            Self::Running => "Läuft",
            Self::Stopping => "Stoppt …",
            Self::Crashed => "Abgestürzt",
            Self::Installing => "Installiert …",
        }
    }
    pub const fn is_online(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

/// Wie der Server gestartet wird. Pfade relativ zum Server-Ordner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchKind {
    /// `java <jvm> -jar <jar> nogui`
    Jar { jar: String },
    /// Forge ≥ 1.17 / NeoForge: `java <jvm> @user_jvm_args.txt @<args_file> nogui`
    ArgsFile { args_file: String },
    /// Eigenes Startskript (Notlösung, z. B. `start.sh`).
    Script { script: String },
    /// `bin/x64/factorio --start-server <save>`; `save` None = neuester Spielstand.
    Factorio { binary: String, save: Option<String> },
}

impl Default for LaunchKind {
    fn default() -> Self {
        Self::Jar { jar: "server.jar".into() }
    }
}

impl LaunchKind {
    /// Kurzbeschreibung für die Oberfläche.
    pub fn describe(&self) -> String {
        match self {
            Self::Jar { jar } => format!("java -jar {jar}"),
            Self::ArgsFile { args_file } => format!("java @{args_file}"),
            Self::Script { script } => format!("./{script}"),
            Self::Factorio { binary, save } => match save {
                Some(s) => format!("{binary} --start-server {s}"),
                None => format!("{binary} --start-server-load-latest"),
            },
        }
    }
}

/// RCON-Zugang (Minecraft `enable-rcon`, Factorio `--rcon-port`). Das Panel richtet ihn
/// automatisch ein, damit Spielerliste, Positionen und Inventare live gelesen werden können.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RconConfig {
    pub port: u16,
    pub password: String,
}

/// Gespeicherte Konfiguration eines Servers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    pub game: Game,
    pub flavor: Flavor,
    /// Spielversion, z. B. `1.21.4` oder `2.0.28` (Factorio).
    pub version: Option<String>,
    /// Version des Loaders (Fabric/Forge/NeoForge), falls bekannt.
    #[serde(default)]
    pub loader_version: Option<String>,
    /// Absoluter Pfad zum Server-Ordner.
    pub dir: String,
    pub launch: LaunchKind,
    /// Java-Programm; None = automatisch passend zur Version wählen.
    #[serde(default)]
    pub java_path: Option<String>,
    /// Arbeitsspeicher (Xmx/Xms) in MB, nur Java.
    pub memory_mb: u32,
    /// Zusätzliche JVM-Argumente.
    #[serde(default)]
    pub jvm_args: String,
    /// Zusätzliche Programmargumente.
    #[serde(default)]
    pub extra_args: String,
    /// Spiel-Port (Minecraft TCP, Factorio UDP).
    pub port: u16,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default = "yes")]
    pub auto_restart: bool,
    #[serde(default)]
    pub rcon: Option<RconConfig>,
    /// Importiert (Ordner liegt ausserhalb des Panels und wird beim Löschen nie entfernt).
    #[serde(default)]
    pub imported: bool,
    pub created_at: i64,
}

fn yes() -> bool {
    true
}

/// Laufzeitwerte eines Servers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ServerStats {
    /// CPU in Prozent eines Kerns (kann > 100 sein).
    pub cpu: f32,
    pub memory_bytes: u64,
    pub uptime_secs: u64,
    pub players_online: u32,
    pub players_max: u32,
    /// Grösse des Server-Ordners (gelegentlich aktualisiert).
    pub disk_bytes: Option<u64>,
}

/// `GET /api/servers` (Liste) und `GET /api/servers/{id}`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ServerSummary {
    pub config: ServerConfig,
    pub status: ServerStatus,
    pub stats: ServerStats,
    /// Namen der Spieler, die gerade online sind.
    pub players: Vec<String>,
    /// Hinweise, die der Benutzer sehen sollte (z. B. EULA nicht akzeptiert, Java fehlt).
    pub warnings: Vec<ServerWarning>,
    /// Java, das beim Start verwendet wird (automatisch oder fest).
    pub java: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerWarning {
    /// `eula.txt` fehlt oder `eula=false` → `POST /api/servers/{id}/eula`.
    EulaNotAccepted,
    /// Keine passende Java-Version gefunden.
    JavaMissing { required: u32 },
    /// RCON aus → keine Live-Daten → `POST /api/servers/{id}/rcon/enable`.
    RconDisabled,
    /// Startdatei fehlt.
    LaunchFileMissing { path: String },
    Other { message: String },
}

/// `POST /api/servers` – neuen Server installieren.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CreateServer {
    pub name: String,
    pub game: Game,
    pub flavor: Flavor,
    /// Spielversion; None = neueste stabile.
    pub version: Option<String>,
    /// Loader-Version (Fabric/Forge/NeoForge); None = neueste.
    pub loader_version: Option<String>,
    pub memory_mb: u32,
    pub port: u16,
    /// Minecraft-EULA akzeptiert (Pflicht für Minecraft).
    pub accept_eula: bool,
    /// Optionale Grundeinstellungen (`server.properties` bzw. `server-settings.json`).
    pub motd: Option<String>,
    pub max_players: Option<u32>,
    pub online_mode: Option<bool>,
    pub difficulty: Option<String>,
    pub gamemode: Option<String>,
    pub seed: Option<String>,
    pub auto_start: bool,
}

/// `PATCH /api/servers/{id}` – nur gesetzte Felder werden geändert.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UpdateServer {
    pub name: Option<String>,
    pub version: Option<String>,
    pub flavor: Option<Flavor>,
    pub launch: Option<LaunchKind>,
    /// `Some(None)` = zurück auf automatisch.
    pub java_path: Option<Option<String>>,
    pub memory_mb: Option<u32>,
    pub jvm_args: Option<String>,
    pub extra_args: Option<String>,
    pub port: Option<u16>,
    pub auto_start: Option<bool>,
    pub auto_restart: Option<bool>,
}

/// `POST /api/servers/{id}/power`
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    Start,
    Stop,
    Restart,
    /// Sofort beenden (SIGKILL).
    Kill,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PowerRequest {
    pub action: PowerAction,
}

/// `POST /api/servers/{id}/command`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CommandRequest {
    pub command: String,
}

/// Antwort auf einen Befehl (über RCON, sonst leer).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CommandResponse {
    pub output: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleSource {
    #[default]
    Stdout,
    Stderr,
    /// Vom Benutzer eingegebener Befehl.
    Input,
    /// Meldung des Panels (Start, Stopp, Installation …).
    Panel,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConsoleLine {
    /// Laufende Nummer (pro Server, steigt monoton).
    pub seq: u64,
    /// Unix-Zeit in Millisekunden.
    pub ts: i64,
    pub source: ConsoleSource,
    /// Rohtext, kann ANSI-Farbcodes enthalten.
    pub text: String,
}

/// Nachrichten über `GET /api/servers/{id}/ws` (Server → Browser).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Erste Nachricht: letzte Konsolenzeilen.
    History { lines: Vec<ConsoleLine> },
    Line { line: ConsoleLine },
    Status { status: ServerStatus },
    Stats { stats: ServerStats, players: Vec<String> },
    Event { event: GameEvent },
}

/// Nachrichten Browser → Server über denselben WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMessage {
    Command { command: String },
}

/* ------------------------------------------------------------------ */
/*                              Backups                               */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Backup {
    /// Dateiname, z. B. `2026-11-15_14-03-22.zip`.
    pub name: String,
    pub size: u64,
    pub created_at: i64,
}

/// `POST /api/servers/{id}/backups`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CreateBackup {
    /// Nur die Welt(en) sichern statt des ganzen Ordners.
    pub worlds_only: bool,
}
