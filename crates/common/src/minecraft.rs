//! Minecraft-Integration: server.properties, Spieler, Inventar + Endertruhe, Karte, Plugins/Mods.

use serde::{Deserialize, Serialize};

/* ------------------------------------------------------------------ */
/*                          server.properties                         */
/* ------------------------------------------------------------------ */

/// `GET/PUT /api/servers/{id}/mc/properties` – Reihenfolge wie in der Datei.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Properties {
    pub entries: Vec<(String, String)>,
}

/* ------------------------------------------------------------------ */
/*                              Spieler                               */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlayerRef {
    pub uuid: Option<String>,
    pub name: String,
}

/// Bekannter Spieler (usercache.json + playerdata).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KnownPlayer {
    pub uuid: String,
    pub name: String,
    pub online: bool,
    pub op: bool,
    pub whitelisted: bool,
    pub banned: bool,
    /// Letzte Änderung der Spielerdatei (≈ zuletzt gesehen), Unix-Sekunden.
    pub last_seen: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BanEntry {
    pub name: String,
    pub uuid: Option<String>,
    pub reason: Option<String>,
    pub created: Option<String>,
    pub source: Option<String>,
    pub expires: Option<String>,
}

/// `GET /api/servers/{id}/mc/players`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlayersOverview {
    pub online: Vec<PlayerRef>,
    pub max_players: u32,
    pub players: Vec<KnownPlayer>,
    pub ops: Vec<PlayerRef>,
    pub whitelist: Vec<PlayerRef>,
    pub whitelist_enabled: bool,
    pub banned_players: Vec<BanEntry>,
    pub banned_ips: Vec<BanEntry>,
    /// Live-Daten verfügbar (RCON verbunden).
    pub live: bool,
}

/// Ein Gegenstand in einem Inventar-Platz.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ItemStack {
    /// NBT-Slot: 0–8 Schnellleiste, 9–35 Inventar, 100–103 Rüstung (Füsse … Kopf),
    /// -106 Zweithand; Endertruhe 0–26.
    pub slot: i32,
    /// z. B. `minecraft:diamond_sword`.
    pub id: String,
    pub count: u32,
    /// Eigener Name (Klartext, ohne JSON).
    pub custom_name: Option<String>,
    /// Verzauberungen `(id, stufe)`.
    pub enchantments: Vec<(String, u32)>,
    pub damage: Option<u32>,
    pub max_damage: Option<u32>,
    /// Weitere Daten vorhanden (Komponenten/Tags), die beim Online-Ersetzen verloren gingen.
    pub has_extra_data: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    #[default]
    Inventory,
    EnderChest,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// z. B. `minecraft:overworld`.
    pub dimension: String,
}

/// `GET /api/servers/{id}/mc/players/{uuid}`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PlayerDetail {
    pub uuid: String,
    pub name: String,
    pub online: bool,
    pub op: bool,
    pub whitelisted: bool,
    pub banned: bool,
    pub health: Option<f32>,
    pub food: Option<i32>,
    pub xp_level: Option<i32>,
    /// `survival`, `creative`, `adventure`, `spectator`.
    pub gamemode: Option<String>,
    pub position: Option<Position>,
    pub spawn: Option<Position>,
    pub inventory: Vec<ItemStack>,
    pub ender_chest: Vec<ItemStack>,
    /// Woher die Daten stammen: `live` (RCON) oder `file` (playerdata).
    pub source: String,
    pub last_seen: Option<i64>,
}

/// `POST /api/servers/{id}/mc/players/{uuid}/inventory`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum InventoryOp {
    /// Platz setzen (`id` leer oder `count` 0 = leeren).
    Set { container: Container, slot: i32, id: String, count: u32 },
    /// Nur Anzahl ändern (behält alle Daten des Gegenstands).
    SetCount { container: Container, slot: i32, count: u32 },
    /// Platz leeren.
    Clear { container: Container, slot: i32 },
    /// Verschieben/Tauschen (auch zwischen Inventar und Endertruhe).
    Move { from: Container, from_slot: i32, to: Container, to_slot: i32 },
    /// Gegenstand geben (`/give`), nur online.
    Give { id: String, count: u32 },
    /// Ganzen Behälter leeren.
    ClearAll { container: Container },
}

/// `POST /api/servers/{id}/mc/players/{name}/action`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PlayerAction {
    Kick { reason: Option<String> },
    Ban { reason: Option<String> },
    Pardon,
    Op,
    Deop,
    WhitelistAdd,
    WhitelistRemove,
    Gamemode { mode: String },
    Teleport { x: f64, y: f64, z: f64, dimension: Option<String> },
    /// Zu einem anderen Spieler teleportieren.
    TeleportTo { target: String },
    Heal,
    Feed,
    Kill,
    Message { text: String },
}

/// `POST /api/servers/{id}/mc/whitelist` – `enabled` umschalten.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToggleRequest {
    pub enabled: bool,
}

/* ------------------------------------------------------------------ */
/*                           Welten & Karte                           */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorldDimension {
    /// `minecraft:overworld`, `minecraft:the_nether`, `minecraft:the_end` oder Mod-Dimension.
    pub id: String,
    /// Anzahl Region-Dateien.
    pub regions: u32,
}

/// `GET /api/servers/{id}/mc/worlds`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorldInfo {
    /// Ordnername, z. B. `world`.
    pub name: String,
    /// Aktive Welt (`level-name`).
    pub active: bool,
    pub size_bytes: u64,
    pub seed: Option<String>,
    pub spawn: Option<(i32, i32, i32)>,
    pub dimensions: Vec<WorldDimension>,
}

/// `GET /api/servers/{id}/mc/map/{world}/regions?dim=` – vorhandene Regionen.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MapRegions {
    pub dimension: String,
    /// `(rx, rz)`; Kachel = 512×512 Blöcke, Bild unter
    /// `GET /api/servers/{id}/mc/map/{world}/tile/{rx}/{rz}.png?dim=`.
    pub regions: Vec<(i32, i32)>,
    /// Weltspawn (x, z).
    pub spawn: Option<(i32, i32)>,
    /// Farben stammen aus echten Texturen (sonst eingebaute Farbtabelle).
    pub textured: bool,
}

/// Spieler auf der Karte: `GET /api/servers/{id}/mc/map/players`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MapPlayer {
    pub uuid: Option<String>,
    pub name: String,
    pub online: bool,
    pub position: Position,
    pub health: Option<f32>,
}

/* ------------------------------------------------------------------ */
/*                           Plugins & Mods                           */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Addon {
    /// Dateiname (inkl. `.disabled`, falls deaktiviert).
    pub file: String,
    /// Aus plugin.yml / fabric.mod.json / mods.toml.
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub enabled: bool,
    pub size: u64,
}

/// `GET /api/servers/{id}/mc/addons`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AddonList {
    /// `plugins` oder `mods`.
    pub folder: String,
    pub addons: Vec<Addon>,
}

/// `POST /api/servers/{id}/mc/addons/toggle`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AddonToggle {
    pub file: String,
    pub enabled: bool,
}

/// Treffer der Modrinth-Suche: `GET /api/servers/{id}/mc/modrinth/search?q=`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModrinthHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub categories: Vec<String>,
}

/// `POST /api/servers/{id}/mc/modrinth/install`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModrinthInstall {
    pub project_id: String,
}

/// Bekannte Item-IDs für die Auswahl: `GET /api/servers/{id}/mc/items`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ItemCatalog {
    pub items: Vec<String>,
}

/// Hilfsfunktion: Slot-Beschriftung.
pub fn slot_label(container: Container, slot: i32) -> String {
    match container {
        Container::EnderChest => format!("Endertruhe {}", slot + 1),
        Container::Inventory => match slot {
            0..=8 => format!("Schnellleiste {}", slot + 1),
            9..=35 => format!("Inventar {}", slot - 8),
            100 => "Stiefel".into(),
            101 => "Hose".into(),
            102 => "Brustpanzer".into(),
            103 => "Helm".into(),
            -106 => "Zweithand".into(),
            s => format!("Platz {s}"),
        },
    }
}

/// Lesbarer Name aus einer Item-ID: `minecraft:diamond_sword` → `Diamond Sword`.
pub fn item_display_name(id: &str) -> String {
    let base = id.rsplit(':').next().unwrap_or(id);
    base.split('_')
        .filter(|s| !s.is_empty())
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
