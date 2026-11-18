//! Welten: Ordner mit level.dat, Grösse, Seed, Spawn, Dimensionen (Region-Ordner).

use std::path::{Path, PathBuf};

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
};
use nrz_gp_common::{MapRegions, WorldDimension, WorldInfo};
use serde::Deserialize;

use super::{Ctx, ctx, nbt, safe_name};
use crate::{
    auth::Authed,
    error::{AppError, AppResult},
    state::AppState,
    util,
};

#[derive(Deserialize)]
pub struct DimQuery {
    #[serde(default)]
    pub dim: Option<String>,
}

pub const OVERWORLD: &str = "minecraft:overworld";
pub const NETHER: &str = "minecraft:the_nether";
pub const END: &str = "minecraft:the_end";

/// `nether`, `end`, `overworld` … → volle Dimensions-ID.
pub fn normalize_dim(dim: Option<&str>) -> String {
    let d = dim.map(str::trim).filter(|d| !d.is_empty()).unwrap_or(OVERWORLD).to_ascii_lowercase();
    match d.as_str() {
        "overworld" | "world" | "0" => OVERWORLD.into(),
        "nether" | "the_nether" | "-1" | "dim-1" => NETHER.into(),
        "end" | "the_end" | "1" | "dim1" => END.into(),
        _ if d.contains(':') => d,
        _ => format!("minecraft:{d}"),
    }
}

fn count_mca(dir: &Path) -> u32 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("r.") && n.ends_with(".mca") && e.metadata().is_ok_and(|m| m.len() > 0)
        })
        .count() as u32
}

/// Alle Dimensionen einer Welt: (ID, Region-Ordner, Anzahl Regionen).
pub fn dimensions(server_dir: &Path, world: &str) -> Vec<(String, PathBuf, u32)> {
    let w = server_dir.join(world);
    let mut cands: Vec<(String, PathBuf)> = vec![
        (OVERWORLD.into(), w.join("region")),
        (OVERWORLD.into(), w.join("dimensions/minecraft/overworld/region")),
        (NETHER.into(), w.join("DIM-1/region")),
        (NETHER.into(), server_dir.join(format!("{world}_nether/DIM-1/region"))),
        (NETHER.into(), w.join("dimensions/minecraft/the_nether/region")),
        (END.into(), w.join("DIM1/region")),
        (END.into(), server_dir.join(format!("{world}_the_end/DIM1/region"))),
        (END.into(), w.join("dimensions/minecraft/the_end/region")),
    ];
    // Weitere Dimensionen (Mods, Datapacks): dimensions/<ns>/<name…>/region
    let dims_root = w.join("dimensions");
    if dims_root.is_dir() {
        for e in walkdir::WalkDir::new(&dims_root).min_depth(2).max_depth(5).into_iter().flatten() {
            if e.file_type().is_dir() && e.file_name() == "region" {
                if let Some(parent) = e.path().parent() {
                    if let Ok(rel) = parent.strip_prefix(&dims_root) {
                        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
                        if parts.len() >= 2 {
                            let id = format!("{}:{}", parts[0], parts[1..].join("/"));
                            cands.push((id, e.path().to_path_buf()));
                        }
                    }
                }
            }
        }
    }
    let mut out: Vec<(String, PathBuf, u32)> = Vec::new();
    for (id, p) in cands {
        if !p.is_dir() {
            continue;
        }
        let n = count_mca(&p);
        match out.iter_mut().find(|(i, _, _)| *i == id) {
            Some(existing) => {
                if n > existing.2 {
                    *existing = (id, p, n);
                }
            }
            None => out.push((id, p, n)),
        }
    }
    out
}

/// Region-Ordner einer Dimension.
pub fn region_dir(server_dir: &Path, world: &str, dim: &str) -> Option<PathBuf> {
    dimensions(server_dir, world).into_iter().find(|(i, _, _)| i == dim).map(|(_, p, _)| p)
}

/// Weltname prüfen (einzelner Ordner im Server-Verzeichnis).
pub fn check_world(c: &Ctx, world: &str) -> AppResult<PathBuf> {
    if !safe_name(world) {
        return Err(AppError::bad("Ungültiger Weltname"));
    }
    let p = util::safe_join(&c.dir, world)?;
    if !p.is_dir() {
        return Err(AppError::not_found("Welt nicht gefunden"));
    }
    Ok(p)
}

pub struct LevelInfo {
    pub seed: Option<String>,
    pub spawn: Option<(i32, i32, i32)>,
}

pub fn level_info(world_dir: &Path) -> LevelInfo {
    let mut info = LevelInfo { seed: None, spawn: None };
    let Ok(root) = nbt::read_nbt(&world_dir.join("level.dat")) else { return info };
    let Some(data) = nbt::get(&root, "Data") else { return info };
    info.seed = nbt::path(data, &["WorldGenSettings", "seed"])
        .or_else(|| nbt::get(data, "RandomSeed"))
        .and_then(nbt::as_i64)
        .map(|s| s.to_string());
    if let Some(p) = nbt::path(data, &["spawn", "pos"]).and_then(nbt::ints).filter(|p| p.len() >= 3) {
        info.spawn = Some((p[0] as i32, p[1] as i32, p[2] as i32));
    } else if let (Some(x), Some(y), Some(z)) = (nbt::int_of(data, "SpawnX"), nbt::int_of(data, "SpawnY"), nbt::int_of(data, "SpawnZ")) {
        info.spawn = Some((x as i32, y as i32, z as i32));
    }
    info
}

fn list_worlds(server_dir: &Path, active: &str, level_seed: Option<String>) -> Vec<WorldInfo> {
    let Ok(rd) = std::fs::read_dir(server_dir) else { return Vec::new() };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().join("level.dat").is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    let all = names.clone();
    // Bukkit/Paper: <welt>_nether / <welt>_the_end gehören zur Hauptwelt
    names.retain(|n| {
        let base = n.strip_suffix("_nether").or_else(|| n.strip_suffix("_the_end"));
        !base.is_some_and(|b| all.iter().any(|a| a == b))
    });
    names
        .into_iter()
        .map(|name| {
            let w = server_dir.join(&name);
            let mut size = util::dir_size(&w);
            for suffix in ["_nether", "_the_end"] {
                let extra = server_dir.join(format!("{name}{suffix}"));
                if extra.is_dir() {
                    size += util::dir_size(&extra);
                }
            }
            let li = level_info(&w);
            let is_active = name == active;
            let seed = li.seed.or_else(|| if is_active { level_seed.clone() } else { None });
            WorldInfo {
                dimensions: dimensions(server_dir, &name).into_iter().map(|(id, _, regions)| WorldDimension { id, regions }).collect(),
                name,
                active: is_active,
                size_bytes: size,
                seed,
                spawn: li.spawn,
            }
        })
        .collect()
}

pub async fn h_worlds(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<Vec<WorldInfo>>> {
    let c = ctx(&app, &id).await?;
    let dir = c.dir.clone();
    let active = c.level_name();
    let seed = c.prop("level-seed").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let worlds = tokio::task::spawn_blocking(move || list_worlds(&dir, &active, seed)).await?;
    Ok(Json(worlds))
}

pub async fn h_regions(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((id, world)): AxPath<(String, String)>,
    Query(q): Query<DimQuery>,
) -> AppResult<Json<MapRegions>> {
    let c = ctx(&app, &id).await?;
    let wdir = check_world(&c, &world)?;
    let dim = normalize_dim(q.dim.as_deref());
    let dir = c.dir.clone();
    let dim2 = dim.clone();
    let (regions, spawn) = tokio::task::spawn_blocking(move || {
        let mut regions = Vec::new();
        if let Some(rd) = region_dir(&dir, &world, &dim2).and_then(|p| std::fs::read_dir(p).ok()) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if !e.metadata().is_ok_and(|m| m.len() > 0) {
                    continue;
                }
                if let Some(xz) = parse_region_name(&n) {
                    regions.push(xz);
                }
            }
        }
        regions.sort();
        let spawn = if dim2 == OVERWORLD { level_info(&wdir).spawn.map(|(x, _, z)| (x, z)) } else { None };
        (regions, spawn)
    })
    .await?;
    let textured = super::assets::client_jar_present(&app, c.cfg.version.as_deref());
    if !textured {
        // Client-Jar im Hintergrund laden, damit die nächsten Kacheln Texturfarben haben.
        if let Some(v) = c.cfg.version.clone() {
            let app2 = app.clone();
            tokio::spawn(async move {
                let _ = super::assets::ensure_client_jar(&app2, Some(&v)).await;
            });
        }
    }
    Ok(Json(MapRegions { dimension: dim, regions, spawn, textured }))
}

/// `r.-1.2.mca` → `(-1, 2)`.
pub fn parse_region_name(n: &str) -> Option<(i32, i32)> {
    let rest = n.strip_prefix("r.")?.strip_suffix(".mca")?;
    let (x, z) = rest.split_once('.')?;
    Some((x.parse().ok()?, z.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dims_and_regions() {
        assert_eq!(normalize_dim(None), OVERWORLD);
        assert_eq!(normalize_dim(Some("nether")), NETHER);
        assert_eq!(normalize_dim(Some("twilightforest:twilight_forest")), "twilightforest:twilight_forest");
        assert_eq!(parse_region_name("r.-1.2.mca"), Some((-1, 2)));
        assert_eq!(parse_region_name("r.1.mca"), None);
    }

    #[test]
    fn bukkit_layout() {
        let base = std::env::temp_dir().join(format!("nrzgp-worlds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for p in ["world/region", "world_nether/DIM-1/region", "world_the_end/DIM1/region", "world/dimensions/mymod/sky/region"] {
            std::fs::create_dir_all(base.join(p)).unwrap();
        }
        std::fs::write(base.join("world/region/r.0.0.mca"), b"x").unwrap();
        std::fs::write(base.join("world_nether/DIM-1/region/r.0.0.mca"), b"x").unwrap();
        let d = dimensions(&base, "world");
        let ids: Vec<&str> = d.iter().map(|(i, _, _)| i.as_str()).collect();
        assert!(ids.contains(&OVERWORLD) && ids.contains(&NETHER) && ids.contains(&END) && ids.contains(&"mymod:sky"));
        assert_eq!(d.iter().find(|x| x.0 == NETHER).unwrap().2, 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}
