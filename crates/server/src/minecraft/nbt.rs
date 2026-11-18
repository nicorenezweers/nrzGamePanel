//! NBT-Hilfen: Spielerdaten (playerdata/<uuid>.dat) lesen und verlustfrei ändern, Item-Formate
//! (alt bis 1.20.4: `Count:b` + `tag`, neu ab 1.20.5: `count:i` + `components`), Slot-Zuordnung.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
};

use fastnbt::Value;
use nrz_gp_common::{Container, ItemStack, Position};

use crate::error::{AppError, AppResult};

/// DataVersion ab der Items `count`/`components` nutzen (1.20.5).
pub const DV_COMPONENTS: i32 = 3837;
/// DataVersion ab der Rüstung/Zweithand im `equipment`-Compound liegen (1.21.5).
pub const DV_EQUIPMENT: i32 = 4325;

/* ------------------------------ Datei-IO ------------------------------ */

/// NBT lesen (gzip, zlib oder unkomprimiert).
pub fn read_nbt(path: &Path) -> AppResult<Value> {
    let raw = std::fs::read(path)?;
    parse_nbt(&raw)
}

pub fn parse_nbt(raw: &[u8]) -> AppResult<Value> {
    let data = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(raw).read_to_end(&mut out).map_err(|e| AppError::internal(format!("NBT entpacken: {e}")))?;
        out
    } else if raw.first() == Some(&0x78) {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(raw).read_to_end(&mut out).map_err(|e| AppError::internal(format!("NBT entpacken: {e}")))?;
        out
    } else {
        raw.to_vec()
    };
    fastnbt::from_bytes(&data).map_err(|e| AppError::internal(format!("NBT ungültig: {e}")))
}

/// NBT gzip-komprimiert und atomar schreiben (vorher Kopie `<datei>.nrzgp-bak`).
pub fn write_nbt_atomic(path: &Path, root: &Value) -> AppResult<()> {
    let bytes = fastnbt::to_bytes(root).map_err(|e| AppError::internal(format!("NBT schreiben: {e}")))?;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&bytes)?;
    let gz = enc.finish()?;
    if path.exists() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".nrzgp-bak");
        std::fs::copy(path, &bak)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".nrzgp-tmp");
    std::fs::write(&tmp, &gz)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/* ---------------------------- Value-Zugriff ---------------------------- */

pub fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Compound(m) => m.get(key),
        _ => None,
    }
}

pub fn path<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(v, |cur, k| get(cur, k))
}

pub fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Byte(b) => Some(*b as i64),
        Value::Short(s) => Some(*s as i64),
        Value::Int(i) => Some(*i as i64),
        Value::Long(l) => Some(*l),
        Value::Float(f) => Some(*f as i64),
        Value::Double(d) => Some(*d as i64),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f as f64),
        Value::Double(d) => Some(*d),
        other => as_i64(other).map(|i| i as f64),
    }
}

pub fn as_str(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s),
        _ => None,
    }
}

pub fn int_of(v: &Value, key: &str) -> Option<i64> {
    get(v, key).and_then(as_i64)
}

/// Ganzzahl-Liste aus IntArray oder List.
pub fn ints(v: &Value) -> Option<Vec<i64>> {
    match v {
        Value::IntArray(a) => Some(a.iter().map(|i| *i as i64).collect()),
        Value::LongArray(a) => Some(a.iter().copied().collect()),
        Value::List(l) => l.iter().map(as_i64).collect(),
        _ => None,
    }
}

fn compound_mut(v: &mut Value) -> Option<&mut HashMap<String, Value>> {
    match v {
        Value::Compound(m) => Some(m),
        _ => None,
    }
}

/* ------------------------------ Textkomponenten ------------------------------ */

/// JSON-Text (`{"text":"A","extra":[…]}`) bzw. reiner String → Klartext.
pub fn json_text_to_plain(s: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(j) => json_plain(&j),
        Err(_) => s.to_string(),
    }
}

fn json_plain(j: &serde_json::Value) -> String {
    match j {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(json_plain).collect(),
        serde_json::Value::Object(o) => {
            let mut out = String::new();
            if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                out.push_str(t);
            } else if let Some(t) = o.get("translate").and_then(|t| t.as_str()) {
                out.push_str(o.get("fallback").and_then(|f| f.as_str()).unwrap_or(t));
            }
            if let Some(extra) = o.get("extra") {
                out.push_str(&json_plain(extra));
            }
            out
        }
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
    }
}

/// NBT-Textkomponente (ab 1.21.5) oder JSON-String (1.20.5–1.21.4) → Klartext.
pub fn nbt_text_to_plain(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let t = s.trim_start();
            if t.starts_with('{') || t.starts_with('[') || t.starts_with('"') { json_text_to_plain(s) } else { s.clone() }
        }
        Value::List(l) => l.iter().map(nbt_text_to_plain).collect(),
        Value::Compound(m) => {
            let mut out = String::new();
            if let Some(t) = m.get("text").and_then(as_str) {
                out.push_str(t);
            } else if let Some(t) = m.get("translate").and_then(as_str) {
                out.push_str(m.get("fallback").and_then(as_str).unwrap_or(t));
            }
            if let Some(extra) = m.get("extra") {
                out.push_str(&nbt_text_to_plain(extra));
            }
            out
        }
        other => as_i64(other).map(|i| i.to_string()).unwrap_or_default(),
    }
}

/* ------------------------------ Items ------------------------------ */

/// Haltbarkeit der Standard-Gegenstände (im neuen Format implizit, nicht gespeichert).
pub fn default_max_damage(id: &str) -> Option<u32> {
    let n = id.strip_prefix("minecraft:").unwrap_or(id);
    let tool = |mat: &str| -> Option<u32> {
        Some(match mat {
            "wooden" => 59,
            "stone" => 131,
            "iron" => 250,
            "golden" => 32,
            "diamond" => 1561,
            "netherite" => 2031,
            "copper" => 190,
            _ => return None,
        })
    };
    for t in ["_sword", "_pickaxe", "_axe", "_shovel", "_hoe"] {
        if let Some(mat) = n.strip_suffix(t) {
            return tool(mat);
        }
    }
    let armor = |mat: &str, piece: usize| -> Option<u32> {
        let base: [u32; 4] = match mat {
            "leather" => [55, 80, 75, 65],
            "chainmail" | "iron" => [165, 240, 225, 195],
            "golden" => [77, 112, 105, 91],
            "diamond" => [363, 528, 495, 429],
            "netherite" => [407, 592, 555, 481],
            "copper" => [121, 176, 165, 143],
            _ => return None,
        };
        Some(base[piece])
    };
    for (i, p) in ["_helmet", "_chestplate", "_leggings", "_boots"].iter().enumerate() {
        if let Some(mat) = n.strip_suffix(p) {
            if mat == "turtle" && i == 0 {
                return Some(275);
            }
            return armor(mat, i);
        }
    }
    Some(match n {
        "bow" => 384,
        "crossbow" => 465,
        "trident" => 250,
        "shield" => 336,
        "elytra" => 432,
        "fishing_rod" => 64,
        "shears" => 238,
        "flint_and_steel" => 64,
        "mace" => 500,
        "carrot_on_a_stick" => 25,
        "warped_fungus_on_a_stick" => 100,
        "brush" => 64,
        "wolf_armor" => 64,
        _ => return None,
    })
}

fn collect_enchantments(v: Option<&Value>, out: &mut Vec<(String, u32)>) {
    match v {
        // Alt: [{id:"minecraft:sharpness", lvl:5s}] (vor 1.13: id als Short)
        Some(Value::List(list)) => {
            for e in list {
                let id = match get(e, "id") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => as_i64(other).map(|i| format!("#{i}")).unwrap_or_default(),
                    None => continue,
                };
                let lvl = int_of(e, "lvl").unwrap_or(1).max(0) as u32;
                out.push((id, lvl));
            }
        }
        // Neu: {levels:{id:lvl}} (1.20.5–1.21.4) oder direkt {id:lvl} (ab 1.21.5)
        Some(Value::Compound(m)) => {
            let levels = match m.get("levels") {
                Some(Value::Compound(l)) => l,
                _ => m,
            };
            let mut v: Vec<(String, u32)> =
                levels.iter().filter_map(|(k, v)| as_i64(v).map(|l| (k.clone(), l.max(0) as u32))).collect();
            v.sort();
            out.extend(v);
        }
        _ => {}
    }
}

/// Item-Compound → `ItemStack` (beide Formate).
pub fn parse_item(v: &Value, slot: i32) -> Option<ItemStack> {
    let id = match get(v, "id")? {
        Value::String(s) => s.clone(),
        other => format!("#{}", as_i64(other)?),
    };
    if id.is_empty() || id == "minecraft:air" {
        return None;
    }
    let mut item = ItemStack { slot, id, ..Default::default() };
    if let Some(c) = int_of(v, "count") {
        item.count = c.max(0) as u32;
    } else if let Some(c) = int_of(v, "Count") {
        item.count = c.max(0) as u32;
    } else {
        item.count = 1; // neues Format: count fehlt = 1
    }
    if let Some(comps) = get(v, "components") {
        let has_any = matches!(comps, Value::Compound(m) if !m.is_empty());
        item.has_extra_data = has_any;
        if let Some(n) = get(comps, "minecraft:custom_name") {
            item.custom_name = Some(nbt_text_to_plain(n)).filter(|s| !s.is_empty());
        }
        collect_enchantments(get(comps, "minecraft:enchantments"), &mut item.enchantments);
        collect_enchantments(get(comps, "minecraft:stored_enchantments"), &mut item.enchantments);
        item.damage = int_of(comps, "minecraft:damage").map(|d| d.max(0) as u32);
        item.max_damage = int_of(comps, "minecraft:max_damage").map(|d| d.max(0) as u32);
    }
    if let Some(tag) = get(v, "tag") {
        let has_any = matches!(tag, Value::Compound(m) if !m.is_empty());
        item.has_extra_data |= has_any;
        if let Some(n) = path(tag, &["display", "Name"]) {
            item.custom_name = Some(nbt_text_to_plain(n)).filter(|s| !s.is_empty());
        }
        collect_enchantments(get(tag, "Enchantments"), &mut item.enchantments);
        collect_enchantments(get(tag, "StoredEnchantments"), &mut item.enchantments);
        collect_enchantments(get(tag, "ench"), &mut item.enchantments);
        if item.damage.is_none() {
            item.damage = int_of(tag, "Damage").map(|d| d.max(0) as u32);
        }
    }
    // Sehr alte Formate: Damage direkt am Item
    if item.damage.is_none() {
        if let Some(d) = int_of(v, "Damage").filter(|d| *d > 0) {
            item.damage = Some(d as u32);
        }
    }
    if item.max_damage.is_none() {
        item.max_damage = default_max_damage(&item.id);
    }
    Some(item)
}

/// Neues Item im passenden Format.
pub fn new_item(id: &str, count: u32, new_format: bool) -> Value {
    let mut m = HashMap::new();
    m.insert("id".to_string(), Value::String(id.to_string()));
    if new_format {
        m.insert("count".to_string(), Value::Int(count.clamp(1, 99) as i32));
    } else {
        m.insert("Count".to_string(), Value::Byte(count.clamp(1, 127) as i8));
    }
    Value::Compound(m)
}

/// Anzahl eines bestehenden Items ändern (Format des Items beibehalten).
pub fn set_item_count(item: &mut Value, count: u32) {
    if let Some(m) = compound_mut(item) {
        if m.contains_key("Count") && !m.contains_key("count") {
            m.insert("Count".into(), Value::Byte(count.clamp(1, 127) as i8));
        } else {
            m.insert("count".into(), Value::Int(count.clamp(1, 99) as i32));
        }
    }
}

/* ------------------------------ Slots ------------------------------ */

/// Gültiger Slot für den Behälter?
pub fn valid_slot(container: Container, slot: i32) -> bool {
    match container {
        Container::EnderChest => (0..27).contains(&slot),
        Container::Inventory => (0..36).contains(&slot) || (100..=103).contains(&slot) || slot == -106,
    }
}

/// Slot → Argument für `/item replace entity <spieler> <slot>`.
pub fn command_slot(container: Container, slot: i32) -> AppResult<String> {
    if !valid_slot(container, slot) {
        return Err(AppError::bad(format!("Ungültiger Platz {slot}")));
    }
    Ok(match (container, slot) {
        (Container::EnderChest, s) => format!("enderchest.{s}"),
        (Container::Inventory, 100) => "armor.feet".into(),
        (Container::Inventory, 101) => "armor.legs".into(),
        (Container::Inventory, 102) => "armor.chest".into(),
        (Container::Inventory, 103) => "armor.head".into(),
        (Container::Inventory, -106) => "weapon.offhand".into(),
        (Container::Inventory, s) => format!("container.{s}"),
    })
}

/// `equipment`-Schlüssel (ab 1.21.5) ↔ NBT-Slot.
pub const EQUIPMENT_SLOTS: [(&str, i32); 5] = [("head", 103), ("chest", 102), ("legs", 101), ("feet", 100), ("offhand", -106)];

pub fn equipment_key(slot: i32) -> Option<&'static str> {
    EQUIPMENT_SLOTS.iter().find(|(_, s)| *s == slot).map(|(k, _)| *k)
}

/* ------------------------------ Spielerdaten ------------------------------ */

pub fn data_version(root: &Value) -> Option<i64> {
    int_of(root, "DataVersion")
}

/// Neues Item-Format (count/components)?
pub fn uses_new_item_format(root: &Value) -> bool {
    if let Some(dv) = data_version(root) {
        return dv >= DV_COMPONENTS as i64;
    }
    // ohne DataVersion: an vorhandenen Items erkennen
    for key in ["Inventory", "EnderItems"] {
        if let Some(Value::List(l)) = get(root, key) {
            for it in l {
                if get(it, "Count").is_some() || get(it, "tag").is_some() {
                    return false;
                }
                if get(it, "count").is_some() || get(it, "components").is_some() {
                    return true;
                }
            }
        }
    }
    true
}

/// Rüstung/Zweithand im `equipment`-Compound?
pub fn uses_equipment(root: &Value) -> bool {
    get(root, "equipment").is_some() || data_version(root).is_some_and(|dv| dv >= DV_EQUIPMENT as i64)
}

pub fn normalize_dimension(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => match as_i64(other) {
            Some(-1) => "minecraft:the_nether".into(),
            Some(1) => "minecraft:the_end".into(),
            _ => "minecraft:overworld".into(),
        },
    }
}

pub fn position(root: &Value) -> Option<Position> {
    let pos = match get(root, "Pos")? {
        Value::List(l) => l.iter().map(as_f64).collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if pos.len() < 3 {
        return None;
    }
    let dimension = get(root, "Dimension").map(normalize_dimension).unwrap_or_else(|| "minecraft:overworld".into());
    Some(Position { x: pos[0], y: pos[1], z: pos[2], dimension })
}

pub fn spawn(root: &Value) -> Option<Position> {
    if let Some(r) = get(root, "respawn") {
        let p = get(r, "pos").and_then(ints)?;
        if p.len() >= 3 {
            let dimension = get(r, "dimension").map(normalize_dimension).unwrap_or_else(|| "minecraft:overworld".into());
            return Some(Position { x: p[0] as f64, y: p[1] as f64, z: p[2] as f64, dimension });
        }
    }
    let x = int_of(root, "SpawnX")?;
    let y = int_of(root, "SpawnY")?;
    let z = int_of(root, "SpawnZ")?;
    let dimension = get(root, "SpawnDimension").map(normalize_dimension).unwrap_or_else(|| "minecraft:overworld".into());
    Some(Position { x: x as f64, y: y as f64, z: z as f64, dimension })
}

pub fn gamemode_name(mode: i64) -> Option<&'static str> {
    Some(match mode {
        0 => "survival",
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => return None,
    })
}

fn list_items(root: &Value, key: &str) -> Vec<ItemStack> {
    let Some(Value::List(l)) = get(root, key) else { return Vec::new() };
    l.iter().filter_map(|it| parse_item(it, int_of(it, "Slot")? as i32)).collect()
}

pub fn inventory(root: &Value) -> Vec<ItemStack> {
    let mut items = list_items(root, "Inventory");
    if let Some(eq) = get(root, "equipment") {
        for (k, slot) in EQUIPMENT_SLOTS {
            if let Some(it) = get(eq, k).and_then(|v| parse_item(v, slot)) {
                items.retain(|i| i.slot != slot);
                items.push(it);
            }
        }
    }
    items.sort_by_key(|i| i.slot);
    items
}

pub fn ender_chest(root: &Value) -> Vec<ItemStack> {
    let mut items = list_items(root, "EnderItems");
    items.sort_by_key(|i| i.slot);
    items
}

/// Name aus Bukkit/Paper-Zusatzdaten (falls vorhanden).
pub fn last_known_name(root: &Value) -> Option<String> {
    path(root, &["bukkit", "lastKnownName"])
        .or_else(|| path(root, &["Paper", "LastKnownName"]))
        .and_then(as_str)
        .map(str::to_string)
}

/* ------------------------------ Offline-Änderungen ------------------------------ */

fn list_key(container: Container) -> &'static str {
    match container {
        Container::Inventory => "Inventory",
        Container::EnderChest => "EnderItems",
    }
}

fn in_equipment(root: &Value, container: Container, slot: i32) -> Option<&'static str> {
    if container == Container::Inventory && uses_equipment(root) { equipment_key(slot) } else { None }
}

/// Item aus einem Platz entfernen und zurückgeben (ohne `Slot`).
pub fn take_item(root: &mut Value, container: Container, slot: i32) -> Option<Value> {
    let eq_key = in_equipment(root, container, slot);
    let m = compound_mut(root)?;
    let mut found = None;
    if let Some(k) = eq_key {
        if let Some(eq) = m.get_mut("equipment").and_then(compound_mut) {
            found = eq.remove(k);
        }
    }
    // auch in der Liste suchen (ältere Daten oder gemischt)
    if let Some(Value::List(list)) = m.get_mut(list_key(container)) {
        if let Some(pos) = list.iter().position(|it| int_of(it, "Slot") == Some(slot as i64)) {
            let it = list.remove(pos);
            if found.is_none() {
                found = Some(it);
            }
        }
    }
    let mut it = found?;
    if let Some(c) = compound_mut(&mut it) {
        c.remove("Slot");
    }
    Some(it)
}

/// Item in einen Platz legen (ersetzt den bisherigen Inhalt).
pub fn put_item(root: &mut Value, container: Container, slot: i32, mut item: Value) {
    let _ = take_item(root, container, slot);
    let eq_key = in_equipment(root, container, slot);
    let Some(m) = compound_mut(root) else { return };
    if let Some(k) = eq_key {
        if let Some(c) = compound_mut(&mut item) {
            c.remove("Slot");
        }
        let eq = m.entry("equipment".to_string()).or_insert_with(|| Value::Compound(HashMap::new()));
        if let Some(eq) = compound_mut(eq) {
            eq.insert(k.to_string(), item);
        }
        return;
    }
    if let Some(c) = compound_mut(&mut item) {
        c.insert("Slot".into(), Value::Byte(slot as i8));
    }
    let list = m.entry(list_key(container).to_string()).or_insert_with(|| Value::List(Vec::new()));
    if !matches!(list, Value::List(_)) {
        *list = Value::List(Vec::new());
    }
    if let Value::List(l) = list {
        l.push(item);
    }
}

#[cfg(test)]
pub fn item_at(root: &Value, container: Container, slot: i32) -> Option<&Value> {
    if let Some(k) = in_equipment(root, container, slot) {
        if let Some(v) = path(root, &["equipment", k]) {
            return Some(v);
        }
    }
    match get(root, list_key(container))? {
        Value::List(l) => l.iter().find(|it| int_of(it, "Slot") == Some(slot as i64)),
        _ => None,
    }
}

pub fn clear_container(root: &mut Value, container: Container) {
    let Some(m) = compound_mut(root) else { return };
    m.insert(list_key(container).to_string(), Value::List(Vec::new()));
    if container == Container::Inventory {
        if let Some(eq) = m.get_mut("equipment").and_then(compound_mut) {
            for (k, _) in EQUIPMENT_SLOTS {
                eq.remove(k);
            }
        }
    }
}

/// `[I; a, b, c, d]` → UUID-Text.
pub fn uuid_from_ints(i: &[i64]) -> Option<String> {
    if i.len() != 4 {
        return None;
    }
    let hex: String = i.iter().map(|v| format!("{:08x}", *v as i32 as u32)).collect();
    Some(format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]))
}

#[cfg(test)]
pub fn uuid_to_int_array(uuid: &str) -> Option<Value> {
    let hex: String = uuid.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let parts: Option<Vec<i32>> = (0..4).map(|k| u32::from_str_radix(&hex[k * 8..k * 8 + 8], 16).ok().map(|v| v as i32)).collect();
    Some(Value::IntArray(fastnbt::IntArray::new(parts?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    #[test]
    fn parse_old_item() {
        let it = c(vec![
            ("id", Value::String("minecraft:diamond_sword".into())),
            ("Count", Value::Byte(1)),
            ("Slot", Value::Byte(0)),
            (
                "tag",
                c(vec![
                    ("Damage", Value::Int(12)),
                    ("display", c(vec![("Name", Value::String(r#"{"text":"Klinge","extra":[{"text":" X"}]}"#.into()))])),
                    (
                        "Enchantments",
                        Value::List(vec![c(vec![("id", Value::String("minecraft:sharpness".into())), ("lvl", Value::Short(5))])]),
                    ),
                ]),
            ),
        ]);
        let s = parse_item(&it, 0).unwrap();
        assert_eq!(s.id, "minecraft:diamond_sword");
        assert_eq!(s.count, 1);
        assert_eq!(s.custom_name.as_deref(), Some("Klinge X"));
        assert_eq!(s.enchantments, vec![("minecraft:sharpness".to_string(), 5)]);
        assert_eq!(s.damage, Some(12));
        assert_eq!(s.max_damage, Some(1561));
        assert!(s.has_extra_data);
    }

    #[test]
    fn parse_new_item_1_20_5() {
        let it = c(vec![
            ("id", Value::String("minecraft:netherite_pickaxe".into())),
            ("count", Value::Int(1)),
            (
                "components",
                c(vec![
                    ("minecraft:custom_name", Value::String(r#""Spitzhacke""#.into())),
                    ("minecraft:damage", Value::Int(3)),
                    (
                        "minecraft:enchantments",
                        c(vec![("levels", c(vec![("minecraft:efficiency", Value::Int(5)), ("minecraft:unbreaking", Value::Int(3))]))]),
                    ),
                ]),
            ),
        ]);
        let s = parse_item(&it, 3).unwrap();
        assert_eq!(s.slot, 3);
        assert_eq!(s.custom_name.as_deref(), Some("Spitzhacke"));
        assert_eq!(s.enchantments, vec![("minecraft:efficiency".into(), 5), ("minecraft:unbreaking".into(), 3)]);
        assert_eq!(s.damage, Some(3));
        assert_eq!(s.max_damage, Some(2031));
        assert!(s.has_extra_data);
    }

    #[test]
    fn parse_new_item_1_21_5_and_plain() {
        let it = c(vec![
            ("id", Value::String("minecraft:bow".into())),
            (
                "components",
                c(vec![
                    ("minecraft:custom_name", c(vec![("text", Value::String("Bogen".into()))])),
                    ("minecraft:enchantments", c(vec![("minecraft:power", Value::Int(4))])),
                    ("minecraft:max_damage", Value::Int(500)),
                ]),
            ),
        ]);
        let s = parse_item(&it, 1).unwrap();
        assert_eq!(s.count, 1);
        assert_eq!(s.custom_name.as_deref(), Some("Bogen"));
        assert_eq!(s.enchantments, vec![("minecraft:power".into(), 4)]);
        assert_eq!(s.max_damage, Some(500));

        let plain = c(vec![("id", Value::String("minecraft:dirt".into())), ("count", Value::Int(64))]);
        let p = parse_item(&plain, 5).unwrap();
        assert_eq!(p.count, 64);
        assert!(!p.has_extra_data);
        assert_eq!(p.max_damage, None);
        assert!(parse_item(&c(vec![("id", Value::String("minecraft:air".into()))]), 0).is_none());
    }

    #[test]
    fn slot_mapping() {
        assert_eq!(command_slot(Container::Inventory, 0).unwrap(), "container.0");
        assert_eq!(command_slot(Container::Inventory, 35).unwrap(), "container.35");
        assert_eq!(command_slot(Container::Inventory, 100).unwrap(), "armor.feet");
        assert_eq!(command_slot(Container::Inventory, 103).unwrap(), "armor.head");
        assert_eq!(command_slot(Container::Inventory, -106).unwrap(), "weapon.offhand");
        assert_eq!(command_slot(Container::EnderChest, 26).unwrap(), "enderchest.26");
        assert!(command_slot(Container::EnderChest, 27).is_err());
        assert!(command_slot(Container::Inventory, 36).is_err());
        assert_eq!(equipment_key(102), Some("chest"));
        assert_eq!(equipment_key(5), None);
    }

    #[test]
    fn offline_edit_equipment_and_move() {
        let mut root = c(vec![
            ("DataVersion", Value::Int(4325)),
            ("Inventory", Value::List(vec![c(vec![("id", Value::String("minecraft:stone".into())), ("count", Value::Int(10)), ("Slot", Value::Byte(0))])])),
            ("EnderItems", Value::List(vec![])),
        ]);
        assert!(uses_new_item_format(&root));
        put_item(&mut root, Container::Inventory, 103, new_item("minecraft:iron_helmet", 1, true));
        assert!(path(&root, &["equipment", "head"]).is_some());
        // Tausch Inventar 0 ↔ Endertruhe 4
        let a = take_item(&mut root, Container::Inventory, 0).unwrap();
        put_item(&mut root, Container::EnderChest, 4, a);
        let inv = inventory(&root);
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].slot, 103);
        let ec = ender_chest(&root);
        assert_eq!(ec[0].slot, 4);
        assert_eq!(ec[0].count, 10);
        let it = take_item(&mut root, Container::EnderChest, 4).unwrap();
        let mut it2 = it.clone();
        set_item_count(&mut it2, 3);
        assert_eq!(int_of(&it2, "count"), Some(3));
        clear_container(&mut root, Container::Inventory);
        assert!(inventory(&root).is_empty());
    }

    #[test]
    fn old_format_count_byte() {
        let mut it = new_item("minecraft:dirt", 200, false);
        assert_eq!(get(&it, "Count"), Some(&Value::Byte(127)));
        set_item_count(&mut it, 5);
        assert_eq!(get(&it, "Count"), Some(&Value::Byte(5)));
    }

    #[test]
    fn uuid_ints() {
        let u = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        let Value::IntArray(a) = uuid_to_int_array(u).unwrap() else { panic!() };
        let v: Vec<i64> = a.iter().map(|i| *i as i64).collect();
        assert_eq!(uuid_from_ints(&v).as_deref(), Some(u));
    }

    #[test]
    fn nbt_roundtrip_gzip() {
        let root = c(vec![("Health", Value::Float(20.0)), ("Pos", Value::List(vec![Value::Double(1.5), Value::Double(64.0), Value::Double(-3.0)]))]);
        let dir = std::env::temp_dir().join(format!("nrzgp-nbt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.dat");
        write_nbt_atomic(&p, &root).unwrap();
        let back = read_nbt(&p).unwrap();
        assert_eq!(back, root);
        assert_eq!(position(&back).unwrap().dimension, "minecraft:overworld");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
