//! Karten-Kacheln: eine Region (32×32 Chunks) → 512×512-PNG (1 px = 1 Block), gerendert mit
//! fastanvil (`TopShadeRenderer`) und eigener Palette (Durchschnittsfarben der echten Blocktexturen,
//! Biom-Einfärbung, sonst eingebaute Farbtabelle).

use std::{
    cell::RefCell,
    collections::HashMap,
    io::{Cursor, Read},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    extract::{Path as AxPath, Query, State},
    http::HeaderMap,
    response::Response,
};
use fastanvil::{Block, Chunk, HeightMode, JavaChunk, Palette, RCoord, RegionFileLoader, RegionLoader, Rgba, TopShadeRenderer, biome::Biome};
use once_cell::sync::Lazy;

use super::{
    assets, ctx, mtime_secs, png_response, safe_name,
    worlds::{DimQuery, check_world, normalize_dim, region_dir},
};
use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    state::AppState,
};

/// Erhöhen, wenn sich die Farbberechnung ändert (verwirft gecachte Kacheln).
const PALETTE_REV: u32 = 1;

static RENDER_SEM: Lazy<tokio::sync::Semaphore> = Lazy::new(|| tokio::sync::Semaphore::new(2));
static PALETTES: Lazy<Mutex<HashMap<String, Arc<McPalette>>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static BUILTIN: Lazy<Arc<McPalette>> = Lazy::new(|| Arc::new(McPalette::builtin()));

/* ------------------------------ Palette ------------------------------ */

pub struct McPalette {
    textured: bool,
    /// Texturname (ohne `block/`) → Durchschnittsfarbe (Alpha = Deckkraft).
    colors: HashMap<String, Rgba>,
    grass: Option<image::RgbaImage>,
    foliage: Option<image::RgbaImage>,
}

/// Durchschnittsfarbe einer Textur (erstes Quadrat, transparente Pixel ignoriert).
pub fn average_color(img: &image::RgbaImage) -> Option<Rgba> {
    let (w, h) = img.dimensions();
    let h = h.min(w.max(1));
    let (mut r, mut g, mut b, mut a_sum, mut wsum) = (0f64, 0f64, 0f64, 0f64, 0f64);
    let total = (w * h) as f64;
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x, y).0;
            if p[3] == 0 {
                continue;
            }
            let wt = p[3] as f64 / 255.0;
            r += p[0] as f64 * wt;
            g += p[1] as f64 * wt;
            b += p[2] as f64 * wt;
            wsum += wt;
            a_sum += wt;
        }
    }
    if wsum <= 0.0 || total <= 0.0 {
        return None;
    }
    let cover = (a_sum / total * 1.3).min(1.0);
    let alpha = if cover > 0.85 { 255 } else { (cover * 255.0).max(40.0) as u8 };
    Some([(r / wsum) as u8, (g / wsum) as u8, (b / wsum) as u8, alpha])
}

impl McPalette {
    fn builtin() -> Self {
        Self { textured: false, colors: HashMap::new(), grass: None, foliage: None }
    }

    /// Palette aus dem Client-Jar.
    fn from_jar(jar: &Path) -> AppResult<Self> {
        let mut zip = assets::open_zip(jar)?;
        let mut colors = HashMap::new();
        let names: Vec<String> = zip
            .file_names()
            .filter(|n| n.starts_with("assets/minecraft/textures/block/") && n.ends_with(".png"))
            .map(str::to_string)
            .collect();
        for n in names {
            let Some(tex) = n.strip_prefix("assets/minecraft/textures/block/").and_then(|s| s.strip_suffix(".png")) else { continue };
            if tex.contains('/') {
                continue;
            }
            let Some(raw) = assets::zip_read(&mut zip, &n) else { continue };
            let Ok(img) = image::load_from_memory_with_format(&raw, image::ImageFormat::Png) else { continue };
            if let Some(c) = average_color(&img.to_rgba8()) {
                colors.insert(tex.to_string(), c);
            }
        }
        let mut colormap = |name: &str| {
            let raw = assets::zip_read(&mut zip, &format!("assets/minecraft/textures/colormap/{name}.png"))?;
            image::load_from_memory_with_format(&raw, image::ImageFormat::Png).ok().map(|i| i.to_rgba8())
        };
        let grass = colormap("grass");
        let foliage = colormap("foliage");
        Ok(Self { textured: !colors.is_empty(), colors, grass, foliage })
    }

    /// Grundfarbe eines Blocks (ohne Biom-Einfärbung).
    fn base_color(&self, name: &str) -> Rgba {
        let (ns, short) = name.split_once(':').unwrap_or(("minecraft", name));
        if ns == "minecraft" {
            if self.textured {
                for cand in block_texture_candidates(short) {
                    if let Some(c) = self.colors.get(&cand) {
                        return *c;
                    }
                }
            }
            if let Some(c) = builtin_color(short) {
                return c;
            }
            for cand in block_texture_candidates(short) {
                if let Some(c) = builtin_color(&cand) {
                    return c;
                }
            }
        } else if let Some(c) = guess_mod_color(short) {
            return c;
        }
        hash_color(name)
    }

    fn sample(map: &Option<image::RgbaImage>, biome: Option<Biome>) -> Option<[u8; 3]> {
        let map = map.as_ref()?;
        let (t, r) = climate(biome);
        let (w, h) = map.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        let x = (((1.0 - t) * (w - 1) as f64) as u32).min(w - 1);
        let y = (((1.0 - r * t) * (h - 1) as f64) as u32).min(h - 1);
        let p = map.get_pixel(x, y).0;
        Some([p[0], p[1], p[2]])
    }

    fn grass_color(&self, biome: Option<Biome>) -> [u8; 3] {
        use Biome::*;
        match biome {
            Some(Swamp | SwampHills | MangroveSwamp) => return rgb(0x6A7039),
            Some(Badlands | BadlandsPlateau | ErodedBadlands | WoodedBadlands | WoodedBadlandsPlateau | ModifiedBadlandsPlateau | ModifiedWoodedBadlandsPlateau) => {
                return rgb(0x90814D);
            }
            _ => {}
        }
        let c = Self::sample(&self.grass, biome).unwrap_or_else(|| triangle(biome, rgb(0x80B497), rgb(0xBFB755), rgb(0x47CD33)));
        if matches!(biome, Some(DarkForest | DarkForestHills)) {
            // Dunkler Wald: Mischung mit 0x28340A
            return [((c[0] as u16 + 0x28) / 2) as u8, ((c[1] as u16 + 0x34) / 2) as u8, ((c[2] as u16 + 0x0A) / 2) as u8];
        }
        c
    }

    fn foliage_color(&self, biome: Option<Biome>) -> [u8; 3] {
        use Biome::*;
        match biome {
            Some(Swamp | SwampHills) => return rgb(0x6A7039),
            Some(MangroveSwamp) => return rgb(0x8DB127),
            Some(Badlands | BadlandsPlateau | ErodedBadlands | WoodedBadlands | WoodedBadlandsPlateau | ModifiedBadlandsPlateau | ModifiedWoodedBadlandsPlateau) => {
                return rgb(0x9E814D);
            }
            _ => {}
        }
        Self::sample(&self.foliage, biome).unwrap_or_else(|| triangle(biome, rgb(0x60A17B), rgb(0xAEA42A), rgb(0x1ABF00)))
    }
}

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn climate(biome: Option<Biome>) -> (f64, f64) {
    let c = biome.unwrap_or(Biome::Plains).climate();
    (c.temperature.clamp(0.0, 1.0), c.rainfall.clamp(0.0, 1.0))
}

/// Einfache Klima-Interpolation (kalt / trocken / feucht), falls keine Farbtabelle vorhanden.
fn triangle(biome: Option<Biome>, cold: [u8; 3], dry: [u8; 3], wet: [u8; 3]) -> [u8; 3] {
    let (t, r) = climate(biome);
    let mix = |i: usize| {
        let warm = dry[i] as f64 * (1.0 - r) + wet[i] as f64 * r;
        (cold[i] as f64 * (1.0 - t) + warm * t) as u8
    };
    [mix(0), mix(1), mix(2)]
}

pub fn water_color(biome: Option<Biome>) -> [u8; 3] {
    use Biome::*;
    rgb(match biome {
        Some(Swamp | SwampHills) => 0x617B64,
        Some(MangroveSwamp) => 0x3A7A6A,
        Some(WarmOcean | DeepWarmOcean) => 0x43D5EE,
        Some(LukewarmOcean | DeepLukewarmOcean) => 0x45ADF2,
        Some(ColdOcean | DeepColdOcean | SnowyTaiga | SnowyBeach) => 0x3D57D6,
        Some(FrozenOcean | DeepFrozenOcean | FrozenRiver) => 0x3938C9,
        Some(Meadow) => 0x0E4ECF,
        _ => 0x3F76E4,
    })
}

fn tint(base: Rgba, t: [u8; 3]) -> Rgba {
    [
        (base[0] as u16 * t[0] as u16 / 255) as u8,
        (base[1] as u16 * t[1] as u16 / 255) as u8,
        (base[2] as u16 * t[2] as u16 / 255) as u8,
        base[3],
    ]
}

enum Tint {
    Grass,
    Foliage,
    Water,
    Fixed(u32),
}

fn tint_kind(short: &str) -> Option<Tint> {
    Some(match short {
        "grass_block" | "short_grass" | "grass" | "tall_grass" | "fern" | "large_fern" | "potted_fern" | "sugar_cane" | "bush" => Tint::Grass,
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "mangrove_leaves" | "vine" => Tint::Foliage,
        "water" | "bubble_column" | "water_cauldron" => Tint::Water,
        "birch_leaves" => Tint::Fixed(0x80A755),
        "spruce_leaves" => Tint::Fixed(0x619961),
        "lily_pad" => Tint::Fixed(0x208030),
        _ => return None,
    })
}

/// Palette für einen Render-Durchlauf (mit Zwischenspeicher je Blockname).
struct RenderPalette<'a> {
    pal: &'a McPalette,
    memo: RefCell<HashMap<String, Rgba>>,
}

impl Palette for RenderPalette<'_> {
    fn pick(&self, block: &Block, biome: Option<Biome>) -> Rgba {
        let name = block.name();
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        if matches!(short, "air" | "cave_air" | "void_air" | "barrier" | "light" | "structure_void") {
            return [0, 0, 0, 0];
        }
        if block.snowy() || short == "snow" {
            return self.color("minecraft:snow");
        }
        let base = self.color(name);
        if !name.starts_with("minecraft:") {
            return base;
        }
        match tint_kind(short) {
            Some(Tint::Grass) => self.tinted(base, self.pal.grass_color(biome)),
            Some(Tint::Foliage) => self.tinted(base, self.pal.foliage_color(biome)),
            Some(Tint::Water) => {
                let c = self.tinted(base, water_color(biome));
                [c[0], c[1], c[2], 255]
            }
            Some(Tint::Fixed(c)) => self.tinted(base, rgb(c)),
            None => base,
        }
    }
}

impl RenderPalette<'_> {
    fn color(&self, name: &str) -> Rgba {
        if let Some(c) = self.memo.borrow().get(name) {
            return *c;
        }
        let c = self.pal.base_color(name);
        self.memo.borrow_mut().insert(name.to_string(), c);
        c
    }

    /// Texturfarben sind grau → multiplizieren; eingebaute Farben → Tönung direkt (leicht abgedunkelt).
    fn tinted(&self, base: Rgba, t: [u8; 3]) -> Rgba {
        if self.pal.textured {
            tint(base, t)
        } else {
            [(t[0] as u16 * 9 / 10) as u8, (t[1] as u16 * 9 / 10) as u8, (t[2] as u16 * 9 / 10) as u8, base[3]]
        }
    }
}

/// Mögliche Texturnamen für einen Block (ohne Namensraum).
pub fn block_texture_candidates(short: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !v.contains(&s) {
            v.push(s);
        }
    };
    let special: &[&str] = match short {
        "water" | "bubble_column" | "water_cauldron" => &["water_still"],
        "lava" | "lava_cauldron" => &["lava_still"],
        "fire" => &["fire_0"],
        "soul_fire" => &["soul_fire_0"],
        "grass_block" => &["grass_block_top"],
        "dirt_path" => &["dirt_path_top"],
        "grass" | "short_grass" => &["short_grass", "grass"],
        "tall_grass" => &["tall_grass_top"],
        "large_fern" => &["large_fern_top"],
        "sunflower" => &["sunflower_front"],
        "lilac" | "rose_bush" | "peony" | "pitcher_plant" => &[],
        "cave_vines" | "cave_vines_plant" => &["cave_vines"],
        "redstone_wire" => &["redstone_dust_dot"],
        "wall_torch" => &["torch"],
        "soul_wall_torch" => &["soul_torch"],
        "redstone_wall_torch" => &["redstone_torch"],
        "smooth_stone_slab" => &["smooth_stone"],
        "smooth_sandstone" | "smooth_sandstone_slab" | "smooth_sandstone_stairs" => &["sandstone_top"],
        "smooth_red_sandstone" | "smooth_red_sandstone_slab" | "smooth_red_sandstone_stairs" => &["red_sandstone_top"],
        "smooth_quartz" | "smooth_quartz_slab" | "smooth_quartz_stairs" | "quartz_slab" | "quartz_stairs" => &["quartz_block_top"],
        "petrified_oak_slab" => &["oak_planks"],
        "moss_carpet" => &["moss_block"],
        "pale_moss_carpet" => &["pale_moss_block"],
        "snow" | "snow_block" | "powder_snow" => &["snow"],
        "frosted_ice" => &["frosted_ice_0"],
        "kelp" | "kelp_plant" => &["kelp_plant"],
        "seagrass" | "tall_seagrass" => &["seagrass"],
        "bamboo" => &["bamboo_stalk"],
        "farmland" => &["farmland_moist"],
        "melon" => &["melon_top"],
        "pumpkin" | "carved_pumpkin" | "jack_o_lantern" => &["pumpkin_top"],
        "cake" => &["cake_top"],
        "beacon" => &["beacon"],
        "chest" | "trapped_chest" | "barrel" => &["barrel_top", "oak_planks"],
        "ender_chest" => &["obsidian"],
        "bell" => &["gold_block"],
        _ => &[],
    };
    for s in special {
        push(s.to_string());
    }
    push(format!("{short}_top"));
    push(short.to_string());
    let mut base_names: Vec<String> = Vec::new();
    for prefix in ["potted_", "infested_", "waxed_"] {
        if let Some(b) = short.strip_prefix(prefix) {
            base_names.push(b.to_string());
        }
    }
    for b in base_names.clone() {
        push(format!("{b}_top"));
        push(b);
    }
    let name = short.strip_prefix("waxed_").unwrap_or(short);
    let suffixes = [
        "_wall_hanging_sign",
        "_hanging_sign",
        "_wall_sign",
        "_sign",
        "_stairs",
        "_slab",
        "_wall",
        "_fence_gate",
        "_fence",
        "_pressure_plate",
        "_button",
        "_door",
        "_trapdoor",
    ];
    for suf in suffixes {
        if let Some(base) = name.strip_suffix(suf) {
            for cand in [format!("{base}_top"), base.to_string(), format!("{base}s"), format!("{base}_planks"), format!("{base}_block")] {
                push(cand);
            }
            if base.ends_with("_brick") {
                push(format!("{base}s"));
            }
            break;
        }
    }
    for (suf, repl) in [
        ("_carpet", "_wool"),
        ("_bed", "_wool"),
        ("_wall_banner", "_wool"),
        ("_banner", "_wool"),
        ("_wood", "_log"),
        ("_hyphae", "_stem"),
        ("_glass_pane", "_glass"),
        ("_coral_wall_fan", "_coral_fan"),
        ("_candle_cake", "_candle"),
    ] {
        if let Some(base) = name.strip_suffix(suf) {
            push(format!("{base}{repl}"));
            push(format!("{base}{repl}_top"));
        }
    }
    if name == "glass_pane" {
        push("glass".into());
    }
    v
}

/// Eingebaute Farbtabelle für häufige Blöcke (ohne Client-Jar).
fn builtin_color(short: &str) -> Option<Rgba> {
    const T: &[(&str, u32)] = &[
        ("stone", 0x7D7D7D),
        ("granite", 0x9A6B57),
        ("diorite", 0xBDBDBD),
        ("andesite", 0x888888),
        ("deepslate", 0x505053),
        ("cobbled_deepslate", 0x4D4D50),
        ("tuff", 0x6C6D66),
        ("calcite", 0xDFE0DC),
        ("bedrock", 0x555555),
        ("dirt", 0x866043),
        ("coarse_dirt", 0x77553B),
        ("rooted_dirt", 0x90684D),
        ("podzol_top", 0x5B3F18),
        ("mycelium_top", 0x6F6265),
        ("mud", 0x3C393D),
        ("clay", 0xA0A6B3),
        ("gravel", 0x837F7E),
        ("sand", 0xDBCFA3),
        ("red_sand", 0xBE6621),
        ("sandstone_top", 0xDFD6AA),
        ("red_sandstone_top", 0xB5621F),
        ("terracotta", 0x985E43),
        ("snow", 0xF9FEFE),
        ("ice", 0x91B7FD),
        ("packed_ice", 0x8DB4FA),
        ("blue_ice", 0x74A8FD),
        ("water_still", 0x3F76E4),
        ("lava_still", 0xCF5B13),
        ("grass_block_top", 0x7CBD6B),
        ("short_grass", 0x7CBD6B),
        ("oak_leaves", 0x4C7A2C),
        ("spruce_leaves", 0x3E5F3E),
        ("birch_leaves", 0x6B8C45),
        ("jungle_leaves", 0x4A8A1E),
        ("acacia_leaves", 0x5A8A1E),
        ("dark_oak_leaves", 0x3C6A1A),
        ("mangrove_leaves", 0x5E8C2A),
        ("cherry_leaves", 0xE9B1C8),
        ("azalea_leaves", 0x5A7A2A),
        ("pale_oak_leaves", 0xA0A69A),
        ("oak_log_top", 0xA88754),
        ("spruce_log_top", 0x6B4F2F),
        ("birch_log_top", 0xC5B57D),
        ("oak_planks", 0xA2834F),
        ("spruce_planks", 0x735531),
        ("birch_planks", 0xC0AF79),
        ("jungle_planks", 0xA07351),
        ("acacia_planks", 0xA85A32),
        ("dark_oak_planks", 0x432B14),
        ("cherry_planks", 0xE2B2AA),
        ("mangrove_planks", 0x763631),
        ("cobblestone", 0x7A7A7A),
        ("mossy_cobblestone", 0x6E7A5C),
        ("stone_bricks", 0x7A7979),
        ("bricks", 0x976253),
        ("netherrack", 0x612626),
        ("nether_bricks", 0x2C1519),
        ("soul_sand", 0x513E32),
        ("soul_soil", 0x4B3A2E),
        ("basalt_top", 0x515156),
        ("blackstone_top", 0x2A2328),
        ("crimson_nylium", 0x831F1F),
        ("warped_nylium", 0x2B7265),
        ("glowstone", 0xAB8654),
        ("magma", 0x8E3F1F),
        ("end_stone", 0xDBDE9E),
        ("purpur_block", 0xA97DA9),
        ("obsidian", 0x0F0B19),
        ("coal_ore", 0x737373),
        ("iron_ore", 0x88827D),
        ("gold_ore", 0x8F8B76),
        ("diamond_ore", 0x7D8E8D),
        ("iron_block", 0xDCDCDC),
        ("gold_block", 0xF6D03D),
        ("diamond_block", 0x62DBD5),
        ("emerald_block", 0x2ACB57),
        ("redstone_block", 0xAF1805),
        ("lapis_block", 0x1F438C),
        ("copper_block", 0xC06C50),
        ("quartz_block_top", 0xEBE5DE),
        ("white_wool", 0xE9ECEC),
        ("white_concrete", 0xCFD5D6),
        ("glass", 0xC0F5FE),
        ("pumpkin_top", 0xC57617),
        ("melon_top", 0x6F9118),
        ("hay_block_top", 0xA68B0C),
        ("cactus_top", 0x5A8A2A),
        ("sugar_cane", 0x94C065),
        ("farmland_moist", 0x512E14),
        ("dirt_path_top", 0x947A41),
        ("moss_block", 0x596E2D),
        ("torch", 0xFFD877),
        ("poppy", 0xBE2A1B),
        ("dandelion", 0xF2D43B),
        ("lily_pad", 0x208030),
        ("kelp_plant", 0x58822C),
        ("seagrass", 0x3A7A2A),
        ("bamboo_stalk", 0x6B9A2A),
        ("sculk", 0x0D1E24),
        ("prismarine", 0x63A097),
        ("dripstone_block", 0x866B5C),
        ("amethyst_block", 0x8562C0),
        ("rail", 0x7D6A4A),
        ("flower_pot", 0x7C4535),
        ("barrel_top", 0x86643A),
    ];
    T.iter().find(|(n, _)| *n == short).map(|(_, c)| {
        let [r, g, b] = rgb(*c);
        [r, g, b, if matches!(short, "glass") { 90 } else { 255 }]
    })
}

/// Mod-Blöcke: grob nach Namen schätzen.
fn guess_mod_color(short: &str) -> Option<Rgba> {
    let keys = ["leaves", "log", "planks", "stone", "dirt", "sand", "grass", "water", "ore"];
    let k = keys.iter().find(|k| short.contains(*k))?;
    Some(match *k {
        "leaves" => [0x4C, 0x7A, 0x2C, 255],
        "log" | "planks" => [0x8A, 0x6A, 0x40, 255],
        "stone" | "ore" => [0x7D, 0x7D, 0x7D, 255],
        "dirt" => [0x86, 0x60, 0x43, 255],
        "sand" => [0xDB, 0xCF, 0xA3, 255],
        "grass" => [0x7C, 0xBD, 0x6B, 255],
        _ => [0x3F, 0x76, 0xE4, 255],
    })
}

/// Stabile, gedämpfte Farbe aus dem Namen.
pub fn hash_color(name: &str) -> Rgba {
    let mut h: u32 = 0x811C9DC5;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    let hue = (h % 360) as f64;
    let (s, v) = (0.35, 0.65);
    let c = v * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8, 255]
}

/// Palette holen (Client-Jar → Texturfarben, sonst eingebaut). Blockierend.
fn palette_for(jar: Option<&Path>) -> Arc<McPalette> {
    let Some(jar) = jar else { return BUILTIN.clone() };
    let key = jar.to_string_lossy().to_string();
    let mut guard = PALETTES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = guard.get(&key) {
        return p.clone();
    }
    match McPalette::from_jar(jar) {
        Ok(p) => {
            let p = Arc::new(p);
            guard.insert(key, p.clone());
            p
        }
        Err(e) => {
            tracing::warn!("Kartenpalette aus Client-Jar fehlgeschlagen: {e}");
            BUILTIN.clone()
        }
    }
}

/* ------------------------------ Rendern ------------------------------ */

/// Region rendern → RGBA-Bild 512×512. Fehlerhafte Chunks bleiben transparent; nie panic.
pub fn render_region_image(region_dir: &Path, rx: i32, rz: i32, pal: &McPalette) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(512, 512);
    let loader = RegionFileLoader::new(region_dir.to_path_buf());
    let mut region = match loader.region(RCoord(rx as isize), RCoord(rz as isize)) {
        Ok(Some(r)) => r,
        _ => return img,
    };
    let rp = RenderPalette { pal, memo: RefCell::new(HashMap::new()) };
    let renderer = TopShadeRenderer::new(&rp, HeightMode::Trust);
    let parse = |bytes: &[u8]| -> Option<JavaChunk> {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            JavaChunk::from_bytes(bytes).ok().or_else(|| normalize_chunk(bytes).and_then(|b| JavaChunk::from_bytes(&b).ok()))
        }))
        .ok()
        .flatten()
    };
    // Nördliche Nachbarn (letzte Reihe der Region darüber) für die Schattierung
    let mut north: Vec<Option<JavaChunk>> = (0..32).map(|_| None).collect();
    if let Ok(Some(mut r)) = loader.region(RCoord(rx as isize), RCoord(rz as isize - 1)) {
        for (x, slot) in north.iter_mut().enumerate() {
            *slot = r.read_chunk(x, 31).ok().flatten().and_then(|b| parse(&b));
        }
    }
    for cz in 0..32usize {
        for cx in 0..32usize {
            let chunk = match region.read_chunk(cx, cz) {
                Ok(Some(b)) => parse(&b),
                _ => None,
            };
            let Some(chunk) = chunk else {
                north[cx] = None;
                continue;
            };
            let rendered = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // Chunk ohne Blöcke/Höhen → überspringen
                let _ = chunk.y_range();
                renderer.render(&chunk, north[cx].as_ref())
            }));
            if let Ok(px) = rendered {
                for z in 0..16usize {
                    for x in 0..16usize {
                        let c = px[z * 16 + x];
                        if c[3] > 0 {
                            img.put_pixel((cx * 16 + x) as u32, (cz * 16 + z) as u32, image::Rgba(c));
                        }
                    }
                }
            }
            north[cx] = Some(chunk);
        }
    }
    img
}

/// Minecraft 26.x speichert Paletten-Einträge ohne Eigenschaften als Text (`"minecraft:stone"`, in
/// gemischten Listen als `{"": "minecraft:stone"}`), mit Eigenschaften als `{id, properties}`;
/// fastanvil erwartet `{Name, Properties}`.
/// Wandelt um und serialisiert neu.
fn normalize_chunk(bytes: &[u8]) -> Option<Vec<u8>> {
    use fastnbt::Value;
    let mut v: Value = fastnbt::from_bytes(bytes).ok()?;
    let mut changed = false;
    if let Value::Compound(root) = &mut v {
        if let Some(Value::List(sections)) = root.get_mut("sections") {
            for sec in sections.iter_mut() {
                let Value::Compound(sec) = sec else { continue };
                let Some(Value::Compound(bs)) = sec.get_mut("block_states") else { continue };
                let Some(Value::List(pal)) = bs.get_mut("palette") else { continue };
                for entry in pal.iter_mut() {
                    // Gemischte Listen (Text + Compound) speichert Minecraft als {"": "minecraft:air"}.
                    let wrapped = match entry {
                        Value::Compound(c) if c.len() == 1 && !c.contains_key("Name") => match c.remove("") {
                            Some(Value::String(n)) => Some(n),
                            Some(other) => {
                                c.insert(String::new(), other);
                                None
                            }
                            None => None,
                        },
                        Value::String(n) => Some(std::mem::take(n)),
                        _ => None,
                    };
                    // 26.x: {id, properties} statt {Name, Properties}
                    if let Value::Compound(c) = entry {
                        if !c.contains_key("Name") {
                            if let Some(id) = c.remove("id") {
                                c.insert("Name".to_string(), id);
                                if let Some(p) = c.remove("properties") {
                                    c.insert("Properties".to_string(), p);
                                }
                                changed = true;
                            }
                        }
                    }
                    if let Some(name) = wrapped {
                        let mut m = std::collections::HashMap::new();
                        m.insert("Name".to_string(), Value::String(name));
                        *entry = Value::Compound(m);
                        changed = true;
                    }
                }
            }
        }
    }
    if changed { fastnbt::to_bytes(&v).ok() } else { None }
}

fn encode_png(img: &image::RgbaImage) -> AppResult<Vec<u8>> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| AppError::internal(format!("PNG: {e}")))?;
    Ok(out)
}

fn file_mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

fn read_marker(p: &Path) -> String {
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open(p) {
        let _ = f.read_to_string(&mut s);
    }
    s.trim().to_string()
}

pub async fn h_tile(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((id, world, rx, rz)): AxPath<(String, String, String, String)>,
    Query(q): Query<DimQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let c = ctx(&app, &id).await?;
    check_world(&c, &world)?;
    if !safe_name(&id) {
        return Err(AppError::bad("Ungültige Server-ID"));
    }
    let dim = normalize_dim(q.dim.as_deref());
    let rx: i32 = rx.trim().parse().map_err(|_| AppError::bad("Ungültige Region"))?;
    let rz: i32 = rz.trim().trim_end_matches(".png").parse().map_err(|_| AppError::bad("Ungültige Region"))?;
    let dir = c.dir.clone();
    let (w2, d2) = (world.clone(), dim.clone());
    let rdir = tokio::task::spawn_blocking(move || region_dir(&dir, &w2, &d2))
        .await?
        .ok_or_else(|| AppError::not_found("Dimension nicht gefunden"))?;
    let mca = rdir.join(format!("r.{rx}.{rz}.mca"));
    let mca_mtime = file_mtime(&mca).ok_or_else(|| AppError::not_found("Region nicht vorhanden"))?;

    let jar = c.cfg.version.as_deref().filter(|_| assets::client_jar_present(&app, c.cfg.version.as_deref())).map(|v| assets::client_jar_path(&app, v));
    let kind = match (&jar, &c.cfg.version) {
        (Some(_), Some(v)) => format!("tex-{v}-r{PALETTE_REV}"),
        _ => format!("builtin-r{PALETTE_REV}"),
    };
    let dim_safe: String = dim.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' { ch } else { '_' }).collect();
    let cache_dir: PathBuf = app.config.cache_dir().join("map").join(&id).join(&world).join(dim_safe);
    let tile = cache_dir.join(format!("{rx}.{rz}.png"));
    let marker = cache_dir.join(".palette");

    let fresh = |tile: &Path, marker: &Path, kind: &str| file_mtime(tile).is_some_and(|t| t >= mca_mtime) && read_marker(marker) == kind;
    if fresh(&tile, &marker, &kind) {
        if let Ok(bytes) = tokio::fs::read(&tile).await {
            let mt = tokio::fs::metadata(&tile).await.ok().as_ref().and_then(mtime_secs);
            return Ok(png_response(bytes, "no-cache", mt, &headers));
        }
    }
    let _permit = RENDER_SEM.acquire().await.map_err(|_| AppError::internal("Renderer nicht verfügbar"))?;
    if fresh(&tile, &marker, &kind) {
        if let Ok(bytes) = tokio::fs::read(&tile).await {
            let mt = tokio::fs::metadata(&tile).await.ok().as_ref().and_then(mtime_secs);
            return Ok(png_response(bytes, "no-cache", mt, &headers));
        }
    }
    let (tile2, marker2, kind2) = (tile.clone(), marker.clone(), kind.clone());
    let png = tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
        std::fs::create_dir_all(&cache_dir)?;
        if read_marker(&marker2) != kind2 {
            // Palette gewechselt → alte Kacheln verwerfen
            if let Ok(rd) = std::fs::read_dir(&cache_dir) {
                for e in rd.flatten() {
                    if e.path().extension().is_some_and(|x| x == "png") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            std::fs::write(&marker2, &kind2)?;
        }
        let pal = palette_for(jar.as_deref());
        let img = render_region_image(&rdir, rx, rz, &pal);
        let png = encode_png(&img)?;
        let tmp = tile2.with_extension("png.part");
        std::fs::write(&tmp, &png)?;
        std::fs::rename(&tmp, &tile2)?;
        Ok(png)
    })
    .await??;
    let mt = tokio::fs::metadata(&tile).await.ok().as_ref().and_then(mtime_secs);
    Ok(png_response(png, "no-cache", mt, &headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates() {
        let c = block_texture_candidates("oak_stairs");
        assert!(c.contains(&"oak_planks".to_string()));
        let c = block_texture_candidates("stone_brick_slab");
        assert!(c.contains(&"stone_bricks".to_string()));
        let c = block_texture_candidates("white_carpet");
        assert!(c.contains(&"white_wool".to_string()));
        assert_eq!(block_texture_candidates("water")[0], "water_still");
        assert!(block_texture_candidates("waxed_copper_block").contains(&"copper_block".to_string()));
    }

    #[test]
    fn avg_ignores_transparent() {
        let mut img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        for x in 0..4 {
            for y in 0..4 {
                if (x + y) % 2 == 0 {
                    img.put_pixel(x, y, image::Rgba([200, 100, 50, 255]));
                }
            }
        }
        let c = average_color(&img).unwrap();
        assert_eq!(&c[..3], &[200, 100, 50]);
        assert!(c[3] < 255);
        let full = image::RgbaImage::from_pixel(4, 8, image::Rgba([10, 20, 30, 255]));
        assert_eq!(average_color(&full), Some([10, 20, 30, 255]));
    }

    #[test]
    fn builtin_palette_colors() {
        let p = McPalette::builtin();
        assert_eq!(&p.base_color("minecraft:stone")[..3], &rgb(0x7D7D7D));
        assert_eq!(&p.base_color("minecraft:oak_stairs")[..3], &rgb(0xA2834F));
        let a = p.base_color("somemod:weird_thing");
        assert_eq!(a, p.base_color("somemod:weird_thing"));
        assert_ne!(water_color(Some(Biome::Swamp)), water_color(None));
        let g = p.grass_color(Some(Biome::Desert));
        assert_eq!(g, rgb(0xBFB755));
    }

    #[test]
    fn missing_region_is_transparent() {
        let img = render_region_image(Path::new("/nonexistent-nrzgp"), 0, 0, &McPalette::builtin());
        assert_eq!(img.dimensions(), (512, 512));
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
    }
}




#[cfg(test)]
mod normalize_tests {
    use fastnbt::Value;
    use std::collections::HashMap;

    fn comp(kv: Vec<(&str, Value)>) -> Value {
        Value::Compound(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>())
    }

    #[test]
    fn palette_26x_formats() {
        let palette = Value::List(vec![
            comp(vec![("", Value::String("minecraft:air".into()))]),
            comp(vec![("id", Value::String("minecraft:oak_log".into())), ("properties", comp(vec![("axis", Value::String("y".into()))]))]),
            comp(vec![("Name", Value::String("minecraft:stone".into()))]),
        ]);
        let chunk = comp(vec![("sections", Value::List(vec![comp(vec![("block_states", comp(vec![("palette", palette)]))])]))]);
        let bytes = fastnbt::to_bytes(&chunk).unwrap();
        let out: Value = fastnbt::from_bytes(&super::normalize_chunk(&bytes).unwrap()).unwrap();
        let s = format!("{out:?}");
        assert!(s.contains("\"Name\": String(\"minecraft:air\")"), "{s}");
        assert!(s.contains("\"Name\": String(\"minecraft:oak_log\")"), "{s}");
        assert!(s.contains("\"Properties\""), "{s}");
        assert!(!s.contains("\"id\""), "{s}");
    }
}
