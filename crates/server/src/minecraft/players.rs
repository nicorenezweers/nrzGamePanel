//! Spieler: Übersicht (usercache/ops/whitelist/banns/playerdata), Details inkl. Inventar,
//! Inventar-Änderungen (online per RCON, offline in der NBT-Datei), Aktionen, Kartenpositionen.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{
    Json,
    extract::{Path as AxPath, State},
};
use nrz_gp_common::{
    BanEntry, CommandResponse, Container, InventoryOp, KnownPlayer, MapPlayer, PlayerAction, PlayerDetail, PlayerRef,
    PlayersOverview, Position,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Value as J, json};

use super::{Ctx, ctx, mtime_secs, nbt};
use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    process,
    state::AppState,
    util,
};

/* ------------------------------ Handler ------------------------------ */

pub async fn h_overview(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<PlayersOverview>> {
    Ok(Json(overview(&app, &id).await?))
}

pub async fn h_detail(State(app): State<AppState>, _a: Authed, AxPath((id, player)): AxPath<(String, String)>) -> AppResult<Json<PlayerDetail>> {
    Ok(Json(detail(&app, &id, &player).await?))
}

pub async fn h_inventory(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((id, player)): AxPath<(String, String)>,
    Json(op): Json<InventoryOp>,
) -> AppResult<Json<PlayerDetail>> {
    Ok(Json(inventory(&app, &id, &player, op).await?))
}

pub async fn h_action(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((id, player)): AxPath<(String, String)>,
    Json(action): Json<PlayerAction>,
) -> AppResult<Json<CommandResponse>> {
    Ok(Json(action_impl(&app, &id, &player, action).await?))
}

pub async fn h_map_players(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<Vec<MapPlayer>>> {
    Ok(Json(map_players(&app, &id).await?))
}

/* ------------------------------ JSON-Listen ------------------------------ */

fn read_json_list(path: &Path) -> Vec<J> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Vec<J>>(&s).ok()).unwrap_or_default()
}

fn write_json_list(path: &Path, list: &[J]) -> AppResult<()> {
    let s = serde_json::to_string_pretty(list).map_err(|e| AppError::internal(e.to_string()))?;
    let tmp = path.with_extension("json.nrzgp-tmp");
    std::fs::write(&tmp, s + "\n")?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn js(v: &J, key: &str) -> Option<String> {
    v.get(key).and_then(J::as_str).map(str::to_string).filter(|s| !s.is_empty())
}

/// UUID normalisieren (klein, mit Bindestrichen). None, wenn kein UUID-Format.
pub fn normalize_uuid(s: &str) -> Option<String> {
    let hex: String = s.trim().chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]))
}

fn player_ref(v: &J) -> Option<PlayerRef> {
    Some(PlayerRef { uuid: js(v, "uuid").and_then(|u| normalize_uuid(&u)), name: js(v, "name")? })
}

fn ban_entry(v: &J, ip: bool) -> Option<BanEntry> {
    Some(BanEntry {
        name: if ip { js(v, "ip")? } else { js(v, "name")? },
        uuid: js(v, "uuid").and_then(|u| normalize_uuid(&u)),
        reason: js(v, "reason"),
        created: js(v, "created"),
        source: js(v, "source"),
        expires: js(v, "expires"),
    })
}

/// Inhalt der Spielerlisten eines Servers.
#[derive(Default)]
struct Lists {
    cache: Vec<PlayerRef>,
    ops: Vec<PlayerRef>,
    whitelist: Vec<PlayerRef>,
    banned: Vec<BanEntry>,
    banned_ips: Vec<BanEntry>,
}

impl Lists {
    fn load(dir: &Path) -> Self {
        Self {
            cache: read_json_list(&dir.join("usercache.json")).iter().filter_map(player_ref).collect(),
            ops: read_json_list(&dir.join("ops.json")).iter().filter_map(player_ref).collect(),
            whitelist: read_json_list(&dir.join("whitelist.json")).iter().filter_map(player_ref).collect(),
            banned: read_json_list(&dir.join("banned-players.json")).iter().filter_map(|v| ban_entry(v, false)).collect(),
            banned_ips: read_json_list(&dir.join("banned-ips.json")).iter().filter_map(|v| ban_entry(v, true)).collect(),
        }
    }

    fn all_refs(&self) -> impl Iterator<Item = (Option<&String>, &String)> {
        self.cache
            .iter()
            .chain(&self.ops)
            .chain(&self.whitelist)
            .map(|p| (p.uuid.as_ref(), &p.name))
            .chain(self.banned.iter().map(|b| (b.uuid.as_ref(), &b.name)))
    }

    fn name_for(&self, uuid: &str) -> Option<String> {
        self.all_refs().find(|(u, _)| u.is_some_and(|u| u == uuid)).map(|(_, n)| n.clone())
    }

    fn uuid_for(&self, name: &str) -> Option<String> {
        self.all_refs().find(|(u, n)| u.is_some() && n.eq_ignore_ascii_case(name)).and_then(|(u, _)| u.cloned())
    }

    fn is_op(&self, uuid: Option<&str>, name: &str) -> bool {
        contains(&self.ops, uuid, name)
    }
    fn is_whitelisted(&self, uuid: Option<&str>, name: &str) -> bool {
        contains(&self.whitelist, uuid, name)
    }
    fn is_banned(&self, uuid: Option<&str>, name: &str) -> bool {
        self.banned.iter().any(|b| match (uuid, &b.uuid) {
            (Some(u), Some(bu)) => u == bu,
            _ => b.name.eq_ignore_ascii_case(name),
        })
    }
}

fn contains(list: &[PlayerRef], uuid: Option<&str>, name: &str) -> bool {
    list.iter().any(|p| match (uuid, &p.uuid) {
        (Some(u), Some(pu)) => u == pu,
        _ => p.name.eq_ignore_ascii_case(name),
    })
}

/// Ordner der Spielerdaten: ab Minecraft 26.x `players/data`, vorher `playerdata`.
pub fn playerdata_dir(world: &Path) -> PathBuf {
    let new = world.join("players").join("data");
    if new.is_dir() || !world.join("playerdata").is_dir() && world.join("dimensions").is_dir() { new } else { world.join("playerdata") }
}

/// Spielerdaten (`*.dat`) → (uuid, mtime).
fn playerdata_files(world: &Path) -> Vec<(String, Option<i64>, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(playerdata_dir(world)) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let uuid = normalize_uuid(name.strip_suffix(".dat")?)?;
            let m = e.metadata().ok()?;
            Some((uuid, mtime_secs(&m), e.path()))
        })
        .collect()
}

fn playerdata_path(world: &Path, uuid: &str) -> PathBuf {
    playerdata_dir(world).join(format!("{uuid}.dat"))
}

/* ------------------------------ Übersicht ------------------------------ */

pub async fn overview(app: &AppState, id: &str) -> AppResult<PlayersOverview> {
    let c = ctx(app, id).await?;
    let mut live = false;
    if c.rcon_possible() && c.rt.rcon(app, "list").await.is_ok() {
        live = true;
        process::refresh_players(app, &c.rt).await;
    }
    let online_names = if c.running() { c.rt.players() } else { Vec::new() };
    let dir = c.dir.clone();
    let world = c.world_dir();
    let props = c.props();
    let max_rt = c.rt.stats().players_max;
    let online2 = online_names.clone();
    let ov = tokio::task::spawn_blocking(move || {
        let lists = Lists::load(&dir);
        let files = playerdata_files(&world);
        let mut players: Vec<KnownPlayer> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut add = |uuid: String, name: String, last_seen: Option<i64>, players: &mut Vec<KnownPlayer>| {
            if !uuid.is_empty() && !seen.insert(uuid.clone()) {
                if let Some(p) = players.iter_mut().find(|p| p.uuid == uuid) {
                    if p.last_seen.is_none() {
                        p.last_seen = last_seen;
                    }
                }
                return;
            }
            let u = Some(uuid.as_str()).filter(|u| !u.is_empty());
            players.push(KnownPlayer {
                online: online2.iter().any(|n| n.eq_ignore_ascii_case(&name)),
                op: lists.is_op(u, &name),
                whitelisted: lists.is_whitelisted(u, &name),
                banned: lists.is_banned(u, &name),
                uuid,
                name,
                last_seen,
            });
        };
        for (uuid, mtime, path) in &files {
            let name = lists
                .name_for(uuid)
                .or_else(|| nbt::read_nbt(path).ok().and_then(|r| nbt::last_known_name(&r)))
                .unwrap_or_else(|| uuid[..8].to_string());
            add(uuid.clone(), name, *mtime, &mut players);
        }
        for (u, n) in lists.all_refs().map(|(u, n)| (u.cloned(), n.clone())).collect::<Vec<_>>() {
            if let Some(u) = u {
                add(u, n, None, &mut players);
            }
        }
        for n in &online2 {
            if !players.iter().any(|p| p.name.eq_ignore_ascii_case(n)) {
                let uuid = lists.uuid_for(n).unwrap_or_default();
                add(uuid, n.clone(), None, &mut players);
            }
        }
        players.sort_by(|a, b| {
            b.online.cmp(&a.online).then(b.last_seen.cmp(&a.last_seen)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let online = online2
            .iter()
            .map(|n| PlayerRef { uuid: lists.uuid_for(n), name: n.clone() })
            .collect::<Vec<_>>();
        let max_players =
            util::value(&props, "max-players").and_then(|v| v.trim().parse().ok()).filter(|m: &u32| *m > 0).unwrap_or(max_rt);
        PlayersOverview {
            online,
            max_players,
            players,
            ops: lists.ops,
            whitelist: lists.whitelist,
            whitelist_enabled: util::value(&props, "white-list").is_some_and(|v| v.trim() == "true"),
            banned_players: lists.banned,
            banned_ips: lists.banned_ips,
            live,
        }
    })
    .await?;
    Ok(ov)
}

/* ------------------------------ Spieler auflösen ------------------------------ */

static NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9_.*\-]{1,32}$").expect("regex"));

fn check_name(name: &str) -> AppResult<()> {
    if NAME_RE.is_match(name) { Ok(()) } else { Err(AppError::bad(format!("Ungültiger Spielername „{name}“"))) }
}

/// UUID oder Name → (UUID falls bekannt, Name).
async fn resolve(app: &AppState, c: &Ctx, player: &str) -> AppResult<(Option<String>, String)> {
    let player = player.trim();
    let dir = c.dir.clone();
    let world = c.world_dir();
    let lists = {
        let d = dir.clone();
        tokio::task::spawn_blocking(move || Lists::load(&d)).await?
    };
    if let Some(uuid) = normalize_uuid(player) {
        let name = match lists.name_for(&uuid) {
            Some(n) => n,
            None => {
                let p = playerdata_path(&world, &uuid);
                tokio::task::spawn_blocking(move || nbt::read_nbt(&p).ok().and_then(|r| nbt::last_known_name(&r)))
                    .await?
                    .unwrap_or_else(|| uuid[..8].to_string())
            }
        };
        return Ok((Some(uuid), name));
    }
    check_name(player)?;
    if let Some(u) = lists.uuid_for(player) {
        let name = lists.name_for(&u).unwrap_or_else(|| player.to_string());
        return Ok((Some(u), name));
    }
    // Online: UUID per RCON abfragen
    if c.is_online(player) && c.rcon_possible() {
        if let Ok(out) = c.rt.rcon(app, &format!("data get entity {player} UUID")).await {
            if let Some(u) = parse_entity_data(&out).and_then(parse_int_array).and_then(|i| nbt::uuid_from_ints(&i)) {
                return Ok((Some(u), player.to_string()));
            }
        }
    }
    // Bukkit/Paper: Name in den Spielerdaten
    let pname = player.to_string();
    let found = tokio::task::spawn_blocking(move || {
        playerdata_files(&world).into_iter().find_map(|(u, _, p)| {
            let r = nbt::read_nbt(&p).ok()?;
            nbt::last_known_name(&r).filter(|n| n.eq_ignore_ascii_case(&pname)).map(|n| (u, n))
        })
    })
    .await?;
    if let Some((u, n)) = found {
        return Ok((Some(u), n));
    }
    Ok((None, player.to_string()))
}

/* ------------------------------ Details ------------------------------ */

pub async fn detail(app: &AppState, id: &str, player: &str) -> AppResult<PlayerDetail> {
    let c = ctx(app, id).await?;
    let (uuid, name) = resolve(app, &c, player).await?;
    detail_with(app, &c, uuid, name).await
}

async fn save_all(app: &AppState, c: &Ctx) -> bool {
    if !c.rcon_possible() {
        return false;
    }
    let ok = c.rt.rcon(app, "save-all").await.is_ok();
    if ok {
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    ok
}

async fn detail_with(app: &AppState, c: &Ctx, uuid: Option<String>, name: String) -> AppResult<PlayerDetail> {
    let online = c.is_online(&name);
    let live = online && save_all(app, c).await;
    let dir = c.dir.clone();
    let world = c.world_dir();
    tokio::task::spawn_blocking(move || -> AppResult<PlayerDetail> {
        let lists = Lists::load(&dir);
        let u = uuid.as_deref();
        let mut d = PlayerDetail {
            uuid: uuid.clone().unwrap_or_default(),
            op: lists.is_op(u, &name),
            whitelisted: lists.is_whitelisted(u, &name),
            banned: lists.is_banned(u, &name),
            name,
            online,
            source: if live { "live".into() } else { "file".into() },
            ..Default::default()
        };
        let Some(uuid) = uuid else { return Ok(d) };
        let path = playerdata_path(&world, &uuid);
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => return Ok(d),
        };
        d.last_seen = mtime_secs(&meta);
        let root = nbt::read_nbt(&path)?;
        fill_detail(&mut d, &root);
        Ok(d)
    })
    .await?
}

fn fill_detail(d: &mut PlayerDetail, root: &fastnbt::Value) {
    d.health = nbt::get(root, "Health").and_then(nbt::as_f64).map(|h| h as f32);
    d.food = nbt::int_of(root, "foodLevel").map(|v| v as i32);
    d.xp_level = nbt::int_of(root, "XpLevel").map(|v| v as i32);
    d.gamemode = nbt::int_of(root, "playerGameType").and_then(nbt::gamemode_name).map(str::to_string);
    d.position = nbt::position(root);
    d.spawn = nbt::spawn(root);
    d.inventory = nbt::inventory(root);
    d.ender_chest = nbt::ender_chest(root);
}

/* ------------------------------ Inventar ------------------------------ */

static ITEM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([a-z0-9_.\-]+:)?[a-z0-9_./\-]+$").expect("regex"));

fn normalize_item_id(id: &str) -> AppResult<String> {
    let id = id.trim().to_ascii_lowercase();
    if !ITEM_RE.is_match(&id) {
        return Err(AppError::bad(format!("Ungültige Item-ID „{id}“")));
    }
    Ok(if id.contains(':') { id } else { format!("minecraft:{id}") })
}

/// Typische Fehlermeldungen von Minecraft-Befehlen erkennen.
pub fn looks_like_error(out: &str) -> bool {
    let t = out.trim();
    t.contains("<--[HERE]")
        || ["Unknown", "Incorrect", "No ", "Expected", "Invalid", "Can't", "Could not", "That ", "Target", "Player not found"]
            .iter()
            .any(|p| t.starts_with(p))
}

async fn rcon_checked(app: &AppState, c: &Ctx, cmd: &str) -> AppResult<String> {
    let out = c.rt.rcon(app, cmd).await?;
    if looks_like_error(&out) {
        return Err(AppError::bad(format!("Minecraft: {}", out.trim())));
    }
    Ok(out)
}

pub async fn inventory(app: &AppState, id: &str, player: &str, op: InventoryOp) -> AppResult<PlayerDetail> {
    let c = ctx(app, id).await?;
    let (uuid, name) = resolve(app, &c, player).await?;
    if c.is_online(&name) {
        if !c.rcon_possible() {
            return Err(AppError::conflict("Spieler ist online, aber RCON ist nicht verfügbar – Inventar kann gerade nicht geändert werden"));
        }
        inventory_online(app, &c, uuid.clone(), &name, op).await?;
        return detail_with(app, &c, uuid, name).await;
    }
    if matches!(op, InventoryOp::Give { .. }) {
        return Err(AppError::conflict("Spieler muss online sein, um Gegenstände zu geben"));
    }
    let uuid = uuid.ok_or_else(|| AppError::not_found("Keine Spielerdaten gefunden (Spieler war noch nie auf dem Server)"))?;
    let path = playerdata_path(&c.world_dir(), &uuid);
    if !path.exists() {
        return Err(AppError::not_found("Keine Spielerdaten gefunden (Spieler war noch nie auf dem Server)"));
    }
    let op2 = op.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let mut root = nbt::read_nbt(&path)?;
        apply_offline(&mut root, op2)?;
        nbt::write_nbt_atomic(&path, &root)
    })
    .await??;
    detail_with(app, &c, Some(uuid), name).await
}

/// Offline-Änderung in den NBT-Daten.
fn apply_offline(root: &mut fastnbt::Value, op: InventoryOp) -> AppResult<()> {
    let check = |c: Container, s: i32| -> AppResult<()> {
        if nbt::valid_slot(c, s) { Ok(()) } else { Err(AppError::bad(format!("Ungültiger Platz {s}"))) }
    };
    let new_format = nbt::uses_new_item_format(root);
    match op {
        InventoryOp::Set { container, slot, id, count } => {
            check(container, slot)?;
            if id.trim().is_empty() || count == 0 {
                nbt::take_item(root, container, slot);
            } else {
                let id = normalize_item_id(&id)?;
                nbt::put_item(root, container, slot, nbt::new_item(&id, count, new_format));
            }
        }
        InventoryOp::SetCount { container, slot, count } => {
            check(container, slot)?;
            if count == 0 {
                nbt::take_item(root, container, slot);
            } else {
                let mut it = nbt::take_item(root, container, slot).ok_or_else(|| AppError::not_found("Platz ist leer"))?;
                nbt::set_item_count(&mut it, count);
                nbt::put_item(root, container, slot, it);
            }
        }
        InventoryOp::Clear { container, slot } => {
            check(container, slot)?;
            nbt::take_item(root, container, slot);
        }
        InventoryOp::Move { from, from_slot, to, to_slot } => {
            check(from, from_slot)?;
            check(to, to_slot)?;
            if from == to && from_slot == to_slot {
                return Ok(());
            }
            let a = nbt::take_item(root, from, from_slot).ok_or_else(|| AppError::not_found("Quellplatz ist leer"))?;
            let b = nbt::take_item(root, to, to_slot);
            nbt::put_item(root, to, to_slot, a);
            if let Some(b) = b {
                nbt::put_item(root, from, from_slot, b);
            }
        }
        InventoryOp::Give { .. } => return Err(AppError::conflict("Spieler muss online sein, um Gegenstände zu geben")),
        InventoryOp::ClearAll { container } => nbt::clear_container(root, container),
    }
    Ok(())
}

async fn inventory_online(app: &AppState, c: &Ctx, uuid: Option<String>, name: &str, op: InventoryOp) -> AppResult<()> {
    check_name(name)?;
    let current = |container: Container, slot: i32| {
        let uuid = uuid.clone();
        let name = name.to_string();
        async move {
            let d = detail_with(app, c, uuid, name).await?;
            let list = if container == Container::Inventory { d.inventory } else { d.ender_chest };
            AppResult::Ok(list.into_iter().find(|i| i.slot == slot))
        }
    };
    match op {
        InventoryOp::Set { container, slot, id, count } => {
            let s = nbt::command_slot(container, slot)?;
            let cmd = if id.trim().is_empty() || count == 0 {
                format!("item replace entity {name} {s} with minecraft:air")
            } else {
                format!("item replace entity {name} {s} with {} {count}", normalize_item_id(&id)?)
            };
            rcon_checked(app, c, &cmd).await?;
        }
        InventoryOp::Clear { container, slot } => {
            let s = nbt::command_slot(container, slot)?;
            rcon_checked(app, c, &format!("item replace entity {name} {s} with minecraft:air")).await?;
        }
        InventoryOp::SetCount { container, slot, count } => {
            let s = nbt::command_slot(container, slot)?;
            let it = current(container, slot).await?.ok_or_else(|| AppError::not_found("Platz ist leer"))?;
            if count == 0 {
                rcon_checked(app, c, &format!("item replace entity {name} {s} with minecraft:air")).await?;
            } else {
                if it.has_extra_data {
                    return Err(AppError::conflict(
                        "Der Gegenstand hat Zusatzdaten (Name, Verzauberungen …) – Anzahl bitte offline bearbeiten (Spieler muss offline sein)",
                    ));
                }
                rcon_checked(app, c, &format!("item replace entity {name} {s} with {} {count}", it.id)).await?;
            }
        }
        InventoryOp::Move { from, from_slot, to, to_slot } => {
            let fs = nbt::command_slot(from, from_slot)?;
            let ts = nbt::command_slot(to, to_slot)?;
            if from == to && from_slot == to_slot {
                return Ok(());
            }
            if current(from, from_slot).await?.is_none() {
                return Err(AppError::not_found("Quellplatz ist leer"));
            }
            if current(to, to_slot).await?.is_some() {
                return Err(AppError::conflict("Zielplatz ist belegt – Tauschen geht nur offline"));
            }
            rcon_checked(app, c, &format!("item replace entity {name} {ts} from entity {name} {fs}")).await?;
            rcon_checked(app, c, &format!("item replace entity {name} {fs} with minecraft:air")).await?;
        }
        InventoryOp::Give { id, count } => {
            let id = normalize_item_id(&id)?;
            rcon_checked(app, c, &format!("give {name} {id} {}", count.max(1))).await?;
        }
        InventoryOp::ClearAll { container } => match container {
            Container::Inventory => {
                // „No items were found“ ist hier kein Fehler
                c.rt.rcon(app, &format!("clear {name}")).await?;
            }
            Container::EnderChest => {
                for s in 0..27 {
                    c.rt.rcon(app, &format!("item replace entity {name} enderchest.{s} with minecraft:air")).await?;
                }
            }
        },
    }
    Ok(())
}

/* ------------------------------ Aktionen ------------------------------ */

fn clean_text(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

static DIM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[a-z0-9_.\-]+(:[a-z0-9_./\-]+)?$").expect("regex"));

/// Aktion → Minecraft-Befehl.
pub fn action_command(name: &str, action: &PlayerAction) -> AppResult<String> {
    let with_reason = |base: String, r: &Option<String>| match r.as_deref().map(clean_text).filter(|s| !s.is_empty()) {
        Some(r) => format!("{base} {r}"),
        None => base,
    };
    Ok(match action {
        PlayerAction::Kick { reason } => with_reason(format!("kick {name}"), reason),
        PlayerAction::Ban { reason } => with_reason(format!("ban {name}"), reason),
        PlayerAction::Pardon => format!("pardon {name}"),
        PlayerAction::Op => format!("op {name}"),
        PlayerAction::Deop => format!("deop {name}"),
        PlayerAction::WhitelistAdd => format!("whitelist add {name}"),
        PlayerAction::WhitelistRemove => format!("whitelist remove {name}"),
        PlayerAction::Gamemode { mode } => {
            let m = match mode.trim().to_ascii_lowercase().as_str() {
                "0" | "survival" | "s" => "survival",
                "1" | "creative" | "c" => "creative",
                "2" | "adventure" | "a" => "adventure",
                "3" | "spectator" | "sp" => "spectator",
                other => return Err(AppError::bad(format!("Unbekannter Spielmodus „{other}“"))),
            };
            format!("gamemode {m} {name}")
        }
        PlayerAction::Teleport { x, y, z, dimension } => {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(AppError::bad("Ungültige Koordinaten"));
            }
            let tp = format!("tp {name} {x} {y} {z}");
            match dimension.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                Some(d) if DIM_RE.is_match(d) => format!("execute in {d} run {tp}"),
                Some(d) => return Err(AppError::bad(format!("Ungültige Dimension „{d}“"))),
                None => tp,
            }
        }
        PlayerAction::TeleportTo { target } => {
            check_name(target.trim())?;
            format!("tp {name} {}", target.trim())
        }
        PlayerAction::Heal => format!("effect give {name} minecraft:instant_health 1 10"),
        PlayerAction::Feed => format!("effect give {name} minecraft:saturation 1 10"),
        PlayerAction::Kill => format!("kill {name}"),
        PlayerAction::Message { text } => {
            let t = clean_text(text);
            if t.is_empty() {
                return Err(AppError::bad("Nachricht ist leer"));
            }
            format!("tell {name} {t}")
        }
    })
}

pub async fn action(app: &AppState, id: &str, name: &str, action: PlayerAction) -> AppResult<CommandResponse> {
    action_impl(app, id, name, action).await
}

async fn action_impl(app: &AppState, id: &str, player: &str, action: PlayerAction) -> AppResult<CommandResponse> {
    let c = ctx(app, id).await?;
    // UUID im Pfad erlaubt → Namen auflösen
    let name = if normalize_uuid(player).is_some() { resolve(app, &c, player).await?.1 } else { player.trim().to_string() };
    check_name(&name)?;
    let cmd = action_command(&name, &action)?;
    if c.rt.status().is_online() {
        let out = c.rt.command(app, &cmd).await?;
        return Ok(CommandResponse { output: out });
    }
    // Server gestoppt: Listen direkt bearbeiten
    let list_edit = matches!(
        action,
        PlayerAction::Op | PlayerAction::Deop | PlayerAction::WhitelistAdd | PlayerAction::WhitelistRemove | PlayerAction::Ban { .. } | PlayerAction::Pardon
    );
    if !list_edit {
        return Err(AppError::conflict("Server läuft nicht"));
    }
    let (uuid, rname) = resolve(app, &c, &name).await?;
    let uuid = match uuid {
        Some(u) => u,
        None => mojang_uuid(app, &c, &rname).await.ok_or_else(|| {
            AppError::not_found("UUID des Spielers unbekannt (war noch nie auf dem Server) – Server starten und erneut versuchen")
        })?,
    };
    let dir = c.dir.clone();
    let op_level = c.prop("op-permission-level").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(4);
    let msg = tokio::task::spawn_blocking(move || edit_lists_offline(&dir, &uuid, &rname, &action, op_level)).await??;
    Ok(CommandResponse { output: Some(msg) })
}

/// UUID über die Mojang-API (nur bei online-mode=true).
async fn mojang_uuid(app: &AppState, c: &Ctx, name: &str) -> Option<String> {
    if c.prop("online-mode").is_some_and(|v| v.trim() == "false") {
        return None;
    }
    let url = format!("https://api.mojang.com/users/profiles/minecraft/{name}");
    let resp = app.http.get(url).timeout(Duration::from_secs(10)).send().await.ok()?.error_for_status().ok()?;
    let j: J = resp.json().await.ok()?;
    normalize_uuid(j.get("id")?.as_str()?)
}

fn edit_lists_offline(dir: &Path, uuid: &str, name: &str, action: &PlayerAction, op_level: i64) -> AppResult<String> {
    let same = |v: &J| {
        js(v, "uuid").and_then(|u| normalize_uuid(&u)).is_some_and(|u| u == uuid)
            || js(v, "name").is_some_and(|n| n.eq_ignore_ascii_case(name))
    };
    let (file, add): (&str, Option<J>) = match action {
        PlayerAction::Op => ("ops.json", Some(json!({"uuid": uuid, "name": name, "level": op_level, "bypassesPlayerLimit": false}))),
        PlayerAction::Deop => ("ops.json", None),
        PlayerAction::WhitelistAdd => ("whitelist.json", Some(json!({"uuid": uuid, "name": name}))),
        PlayerAction::WhitelistRemove => ("whitelist.json", None),
        PlayerAction::Ban { reason } => (
            "banned-players.json",
            Some(json!({
                "uuid": uuid,
                "name": name,
                "created": chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z").to_string(),
                "source": "nrzGamePanel",
                "expires": "forever",
                "reason": reason.as_deref().map(clean_text).filter(|s| !s.is_empty()).unwrap_or_else(|| "Banned by an operator.".into()),
            })),
        ),
        PlayerAction::Pardon => ("banned-players.json", None),
        _ => return Err(AppError::conflict("Server läuft nicht")),
    };
    let path = dir.join(file);
    let mut list = read_json_list(&path);
    let before = list.len();
    list.retain(|v| !same(v));
    let removed = before != list.len();
    let added = add.is_some();
    if let Some(entry) = add {
        list.push(entry);
    }
    write_json_list(&path, &list)?;
    Ok(match (added, removed) {
        (true, _) => format!("{name} in {file} eingetragen (Server gestoppt, gilt ab dem nächsten Start)"),
        (false, true) => format!("{name} aus {file} entfernt (Server gestoppt, gilt ab dem nächsten Start)"),
        (false, false) => format!("{name} war nicht in {file} eingetragen"),
    })
}

/* ------------------------------ RCON-Antworten ------------------------------ */

static ENTITY_DATA: Lazy<Regex> = Lazy::new(|| Regex::new(r"has the following entity data:\s*(.*)$").expect("regex"));

/// `Steve has the following entity data: [1.5d, 64.0d]` → `[1.5d, 64.0d]`.
pub fn parse_entity_data(out: &str) -> Option<&str> {
    let line = out.lines().find(|l| l.contains("has the following entity data"))?;
    ENTITY_DATA.captures(line).and_then(|c| c.get(1)).map(|m| m.as_str().trim())
}

fn parse_num(s: &str) -> Option<f64> {
    s.trim().trim_end_matches(['d', 'D', 'f', 'F', 'b', 'B', 's', 'S', 'l', 'L']).parse().ok()
}

/// `[1.5d, 64.0d, -3.2d]` → `[1.5, 64.0, -3.2]`.
pub fn parse_pos(s: &str) -> Option<[f64; 3]> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    let v: Vec<f64> = inner.split(',').map(parse_num).collect::<Option<_>>()?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// `[I; 1, -2, 3, 4]` → Zahlen.
pub fn parse_int_array(s: &str) -> Option<Vec<i64>> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    let inner = inner.trim().strip_prefix("I;").unwrap_or(inner);
    inner.split(',').map(|p| p.trim().parse::<i64>().ok()).collect()
}

/// `"minecraft:overworld"` → `minecraft:overworld`.
pub fn parse_nbt_string(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| t.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(t).to_string()
}

/* ------------------------------ Karte ------------------------------ */

pub async fn map_players(app: &AppState, id: &str) -> AppResult<Vec<MapPlayer>> {
    let c = ctx(app, id).await?;
    let mut out: Vec<MapPlayer> = Vec::new();
    let online = if c.running() { c.rt.players() } else { Vec::new() };
    let lists = {
        let d = c.dir.clone();
        tokio::task::spawn_blocking(move || Lists::load(&d)).await?
    };
    let mut live_names: HashSet<String> = HashSet::new();
    if c.rcon_possible() {
        for name in &online {
            if check_name(name).is_err() {
                continue;
            }
            let Ok(pos_out) = c.rt.rcon(app, &format!("data get entity {name} Pos")).await else { continue };
            let Some(pos) = parse_entity_data(&pos_out).and_then(parse_pos) else { continue };
            let dimension = match c.rt.rcon(app, &format!("data get entity {name} Dimension")).await {
                Ok(o) => parse_entity_data(&o).map(parse_nbt_string).unwrap_or_else(|| "minecraft:overworld".into()),
                Err(_) => "minecraft:overworld".into(),
            };
            let health = match c.rt.rcon(app, &format!("data get entity {name} Health")).await {
                Ok(o) => parse_entity_data(&o).and_then(parse_num).map(|h| h as f32),
                Err(_) => None,
            };
            live_names.insert(name.to_lowercase());
            out.push(MapPlayer {
                uuid: lists.uuid_for(name),
                name: name.clone(),
                online: true,
                position: Position { x: pos[0], y: pos[1], z: pos[2], dimension },
                health,
            });
        }
    }
    let world = c.world_dir();
    let online2 = online.clone();
    let files = tokio::task::spawn_blocking(move || {
        playerdata_files(&world)
            .into_iter()
            .filter_map(|(uuid, _, path)| {
                let root = nbt::read_nbt(&path).ok()?;
                let pos = nbt::position(&root)?;
                let health = nbt::get(&root, "Health").and_then(nbt::as_f64).map(|h| h as f32);
                let name = lists.name_for(&uuid).or_else(|| nbt::last_known_name(&root)).unwrap_or_else(|| uuid[..8].to_string());
                Some(MapPlayer {
                    online: online2.iter().any(|n| n.eq_ignore_ascii_case(&name)),
                    uuid: Some(uuid),
                    name,
                    position: pos,
                    health,
                })
            })
            .collect::<Vec<_>>()
    })
    .await?;
    for p in files {
        if live_names.contains(&p.name.to_lowercase()) {
            continue;
        }
        out.push(p);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rcon_entity_data() {
        let out = "Steve has the following entity data: [1.5d, 64.0d, -3.2d]";
        let d = parse_entity_data(out).unwrap();
        assert_eq!(parse_pos(d), Some([1.5, 64.0, -3.2]));
        let dim = parse_entity_data("Steve has the following entity data: \"minecraft:the_nether\"").unwrap();
        assert_eq!(parse_nbt_string(dim), "minecraft:the_nether");
        let h = parse_entity_data("Alex has the following entity data: 18.5f").unwrap();
        assert_eq!(parse_num(h), Some(18.5));
        let u = parse_entity_data("Steve has the following entity data: [I; 110787060, 1156138790, -1514210135, 238594805]").unwrap();
        let ints = parse_int_array(u).unwrap();
        assert_eq!(nbt::uuid_from_ints(&ints).as_deref(), Some("069a79f4-44e9-4726-a5be-fca90e38aaf5"));
        assert!(parse_entity_data("No entity was found").is_none());
        assert!(parse_pos("[1d, 2d]").is_none());
    }

    #[test]
    fn uuid_normalize() {
        assert_eq!(normalize_uuid("069A79F444E94726A5BEFCA90E38AAF5").as_deref(), Some("069a79f4-44e9-4726-a5be-fca90e38aaf5"));
        assert!(normalize_uuid("Notch").is_none());
    }

    #[test]
    fn action_commands() {
        assert_eq!(action_command("Steve", &PlayerAction::Heal).unwrap(), "effect give Steve minecraft:instant_health 1 10");
        assert_eq!(
            action_command("Steve", &PlayerAction::Teleport { x: 1.0, y: 64.0, z: -2.5, dimension: Some("minecraft:the_nether".into()) })
                .unwrap(),
            "execute in minecraft:the_nether run tp Steve 1 64 -2.5"
        );
        assert_eq!(action_command("Steve", &PlayerAction::Gamemode { mode: "1".into() }).unwrap(), "gamemode creative Steve");
        assert_eq!(action_command("Steve", &PlayerAction::Kick { reason: Some("bye\nnow".into()) }).unwrap(), "kick Steve bye now");
        assert!(action_command("Steve", &PlayerAction::Teleport { x: f64::NAN, y: 0.0, z: 0.0, dimension: None }).is_err());
    }

    #[test]
    fn errors_detected() {
        assert!(looks_like_error("Unknown item 'minecraft:foo'"));
        assert!(looks_like_error("No entity was found"));
        assert!(!looks_like_error("Replaced a slot on Steve with [Diamond]"));
        assert!(!looks_like_error("Gave 1 [Diamond] to Steve"));
    }

    #[test]
    fn offline_move_swap() {
        use fastnbt::Value;
        let it = |id: &str, slot: i8| {
            Value::Compound(
                [("id".to_string(), Value::String(id.into())), ("Count".to_string(), Value::Byte(1)), ("Slot".to_string(), Value::Byte(slot))]
                    .into_iter()
                    .collect(),
            )
        };
        let mut root = Value::Compound(
            [
                ("Inventory".to_string(), Value::List(vec![it("minecraft:stone", 0), it("minecraft:dirt", 1)])),
                ("EnderItems".to_string(), Value::List(vec![])),
            ]
            .into_iter()
            .collect(),
        );
        apply_offline(&mut root, InventoryOp::Move { from: Container::Inventory, from_slot: 0, to: Container::Inventory, to_slot: 1 }).unwrap();
        let inv = nbt::inventory(&root);
        assert_eq!(inv[0].id, "minecraft:dirt");
        assert_eq!(inv[1].id, "minecraft:stone");
        apply_offline(&mut root, InventoryOp::Set { container: Container::EnderChest, slot: 2, id: "diamond".into(), count: 5 }).unwrap();
        let ec = nbt::ender_chest(&root);
        assert_eq!(ec[0].id, "minecraft:diamond");
        assert_eq!(ec[0].count, 5);
        assert!(nbt::get(nbt::item_at(&root, Container::EnderChest, 2).unwrap(), "Count").is_some());
        assert!(apply_offline(&mut root, InventoryOp::Give { id: "minecraft:dirt".into(), count: 1 }).is_err());
    }
}
