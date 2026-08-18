//! Doom / Freedoom IWAD conversion (WAD lumps, maps, patches, sprites).

use super::shared::*;
use makepad_asset_data::AssetKind;
use makepad_gltf::{write_glb_mesh_textured, GlbTexturedMesh};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

// ---------------------------------------------------------------------------
// Doom WAD
// ---------------------------------------------------------------------------

pub(crate) struct WadLump {
    pub name: String,
    pub data: Vec<u8>,
}

pub(crate) struct WadFile {
    pub lumps: Vec<WadLump>,
    pub playpal: Option<[[u8; 3]; 256]>,
}

pub(crate) fn parse_wad(bytes: &[u8]) -> Result<WadFile, String> {
    if bytes.len() < 12 {
        return Err("WAD too small".into());
    }
    let magic = &bytes[0..4];
    if magic != b"IWAD" && magic != b"PWAD" {
        return Err("not IWAD/PWAD".into());
    }
    let n = u32_le(bytes, 4) as usize;
    let diroff = u32_le(bytes, 8) as usize;
    if n > 65_536 {
        return Err("too many lumps".into());
    }
    if diroff.saturating_add(n * 16) > bytes.len() {
        return Err("WAD directory truncated".into());
    }
    let mut lumps = Vec::with_capacity(n);
    let mut playpal = None;
    for i in 0..n {
        let e = diroff + i * 16;
        let pos = u32_le(bytes, e) as usize;
        let size = u32_le(bytes, e + 4) as usize;
        let name = lump_name(&bytes[e + 8..e + 16]);
        if pos.saturating_add(size) > bytes.len() {
            continue;
        }
        let data = bytes[pos..pos + size].to_vec();
        if name == "PLAYPAL" && data.len() >= 768 {
            let mut pal = [[0u8; 3]; 256];
            for i in 0..256 {
                pal[i] = [data[i * 3], data[i * 3 + 1], data[i * 3 + 2]];
            }
            playpal = Some(pal);
        }
        lumps.push(WadLump { name, data });
    }
    Ok(WadFile { lumps, playpal })
}

pub(crate) fn convert_wad(
    path: &Path,
    rel: &str,
    staged: &Path,
    fallback_pal: &[[u8; 3]; 256],
    source: ClassicSource,
    mut on_map: impl FnMut(usize, usize, &str, Option<Vec<u8>>) -> bool + Send,
) -> Result<Vec<ClassicAsset>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let wad = parse_wad(&bytes)?;
    let palette = wad.playpal.as_ref().unwrap_or(fallback_pal);
    let mut assets = Vec::new();
    let wad_slug = stem_slug(rel);
    let on_map = std::sync::Mutex::new(&mut on_map);
    let tick = |i: usize, n: usize, name: &str, preview: Option<Vec<u8>>| -> bool {
        on_map
            .lock()
            .map(|mut cb| cb(i, n, name, preview))
            .unwrap_or(false)
    };

    // Collect map marker groups: ExMy / MAPxx followed by standard lumps.
    let map_names = find_doom_maps(&wad.lumps);
    let map_n = map_names.len();
    if map_n > 0 {
        let worlds_dir = staged.join("worlds").join(&wad_slug);
        std::fs::create_dir_all(&worlds_dir).map_err(|e| e.to_string())?;
        let threads = crate::ao_bake::bake_thread_count().min(map_n);
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let finished = std::sync::atomic::AtomicUsize::new(0);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let out = std::sync::Mutex::new(Vec::<ClassicAsset>::new());
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    use std::sync::atomic::Ordering::{Relaxed, SeqCst};
                    loop {
                        if cancel.load(SeqCst) {
                            break;
                        }
                        let map_i = cursor.fetch_add(1, Relaxed);
                        let Some(map_name) = map_names.get(map_i) else {
                            break;
                        };
                        let mut preview = None;
                        let mut asset = None;
                        if let Some(mesh) = doom_map_to_mesh(&wad.lumps, map_name, palette) {
                            let key = format!("worlds/{wad_slug}/{}", sanitize_slug(map_name));
                            let rel_path = format!("{key}.glb");
                            let dest = staged.join(&rel_path);
                            if std::fs::write(&dest, &mesh.glb).is_ok() {
                                if let Some(spawn) = doom_map_spawn(&wad.lumps, map_name) {
                                    write_spawn_sidecar(&dest, spawn);
                                }
                                let place = doom_map_place(
                                    &wad.lumps,
                                    map_name,
                                    source.id(),
                                    &key,
                                    &wad_slug,
                                );
                                let _ = crate::world_place::write_place_sidecar(&dest, &place);
                                let icon_rel = crate::world_preview::write_spawn_preview(&dest)
                                    .ok()
                                    .map(|_| format!("{key}.png"));
                                if let Some(rel) = &icon_rel {
                                    preview = std::fs::read(staged.join(rel)).ok();
                                }
                                asset = Some(ClassicAsset {
                                    key,
                                    kind: AssetKind::World,
                                    rel_path,
                                    tags: tags_for(
                                        AssetKind::World,
                                        &[source.id(), "map", map_name, "no-portals"],
                                    ),
                                    icon_rel,
                                });
                            }
                        }
                        if let Some(asset) = asset {
                            if let Ok(mut bag) = out.lock() {
                                bag.push(asset);
                            }
                        }
                        let done = finished.fetch_add(1, Relaxed);
                        if !tick(done, map_n, map_name, preview) {
                            cancel.store(true, SeqCst);
                            break;
                        }
                    }
                });
            }
        });
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        assets.append(&mut out.into_inner().unwrap_or_default());
    }

    // Sprites between S_START/S_END (or SS_START/SS_END).
    let sprite_range = lump_range(&wad.lumps, &["S_START", "SS_START"], &["S_END", "SS_END"]);
    if let Some((start, end)) = sprite_range {
        let mut seen_prefix = BTreeSet::new();
        let mut sprite_i = 0usize;
        for lump in &wad.lumps[start + 1..end] {
            if lump.data.is_empty() || lump.name.starts_with("S_") {
                continue;
            }
            if let Ok(png) = doom_patch_to_png(&lump.data, palette) {
                let key = format!("billboards/{wad_slug}/{}", sanitize_slug(&lump.name));
                let rel_path = format!("{key}.png");
                let dest = staged.join(&rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&dest, &png).map_err(|e| e.to_string())?;
                let prefix: String = lump.name.chars().take(4).collect();
                let first = seen_prefix.insert(prefix);
                sprite_i += 1;
                if first || sprite_i % 48 == 0 {
                    if !tick(
                        map_n.saturating_sub(1),
                        map_n.max(1),
                        &lump.name,
                        Some(png.clone()),
                    ) {
                        return Err("cancelled".into());
                    }
                }
                let mut tags = tags_for(
                    AssetKind::Billboard,
                    &[source.id(), "sprite", &lump.name],
                );
                if is_weapon_sprite(&lump.name) {
                    tags.push("weapon".into());
                }
                if is_character_sprite(&lump.name) {
                    tags.push("character-sprite".into());
                }
                assets.push(ClassicAsset {
                    key,
                    kind: AssetKind::Billboard,
                    rel_path,
                    tags,
                    icon_rel: None,
                });
            }
        }
    }

    // Flats → textures (optional catalog; maps embed their own atlas).
    let flat_range = lump_range(&wad.lumps, &["F_START", "FF_START"], &["F_END", "FF_END"]);
    if let Some((start, end)) = flat_range {
        for lump in &wad.lumps[start + 1..end] {
            if lump.data.len() != 64 * 64 {
                continue;
            }
            let rgba = indexed_to_rgba(&lump.data, palette, 0);
            // Flats are opaque floors; index 0 is not a sprite key here.
            let mut opaque = rgba;
            for px in opaque.chunks_exact_mut(4) {
                px[3] = 255;
            }
            if let Ok(png) = encode_png_rgba(&opaque, 64, 64) {
                let key = format!("textures/{wad_slug}/flat-{}", sanitize_slug(&lump.name));
                let rel_path = format!("{key}.png");
                let dest = staged.join(&rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&dest, &png).map_err(|e| e.to_string())?;
                assets.push(ClassicAsset {
                    key,
                    kind: AssetKind::Texture,
                    rel_path,
                    tags: tags_for(AssetKind::Texture, &[source.id(), "flat", &lump.name]),
                    icon_rel: None,
                });
            }
        }
    }

    Ok(assets)
}

pub(crate) fn is_weapon_sprite(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("PISG")
        || n.starts_with("PISF")
        || n.starts_with("SHTG")
        || n.starts_with("SHTF")
        || n.starts_with("CHGG")
        || n.starts_with("CHGF")
        || n.starts_with("MISG")
        || n.starts_with("MISF")
        || n.starts_with("SAWG")
        || n.starts_with("PLSG")
        || n.starts_with("PLSF")
        || n.starts_with("BFGG")
        || n.starts_with("BFGF")
        || n.contains("WEAPON")
}

pub(crate) fn is_character_sprite(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    const PREFIXES: &[&str] = &[
        "PLAY", "TROO", "SARG", "BOSS", "HEAD", "SKUL", "SPOS", "CPOS", "POSS",
        "CYBR", "SPID", "BSPI", "VILE", "SKEL", "FATT", "CPOS", "PAIN", "KEEN",
        "BOS2", "SPOS", "SSWV", "SPID",
    ];
    PREFIXES.iter().any(|p| n.starts_with(p)) || n.contains("CHARACTER")
}

pub(crate) fn is_character_mdl(slug: &str) -> bool {
    const NAMES: &[&str] = &[
        "player", "soldier", "ogre", "shambler", "wizard", "demon", "dog", "fish",
        "enforcer", "knight", "hknight", "zombie", "tarbaby", "shalrath", "boss",
        "oldone",
    ];
    NAMES.iter().any(|n| slug == *n)
}

pub(crate) fn lump_range(lumps: &[WadLump], starts: &[&str], ends: &[&str]) -> Option<(usize, usize)> {
    let start = lumps.iter().position(|l| starts.iter().any(|s| l.name == *s))?;
    let end = lumps
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| ends.iter().any(|e| l.name == *e))
        .map(|(i, _)| i)?;
    Some((start, end))
}

pub(crate) fn find_doom_maps(lumps: &[WadLump]) -> Vec<String> {
    let mut maps = Vec::new();
    for l in lumps {
        let n = &l.name;
        // Doom 2: MAP01..MAP99 (5 chars). Doom 1: E1M1..E4M9 (4 chars).
        let is_map = (n.len() == 5
            && n.starts_with("MAP")
            && n.as_bytes()[3].is_ascii_digit()
            && n.as_bytes()[4].is_ascii_digit())
            || (n.len() == 4
                && n.as_bytes()[0] == b'E'
                && n.as_bytes()[2] == b'M'
                && n.as_bytes()[1].is_ascii_digit()
                && n.as_bytes()[3].is_ascii_digit());
        if is_map {
            maps.push(n.clone());
        }
    }
    maps
}

pub(crate) fn lump_by_name_after<'a>(lumps: &'a [WadLump], map: &str, name: &str) -> Option<&'a [u8]> {
    let start = lumps.iter().position(|l| l.name == map)?;
    lumps[start..]
        .iter()
        .find(|l| l.name == name)
        .map(|l| l.data.as_slice())
}

pub(crate) struct MapMesh {
    pub glb: Vec<u8>,
    pub tris: usize,
}

pub(crate) fn doom_map_spawn(lumps: &[WadLump], map: &str) -> Option<WorldSpawn> {
    let things = lump_by_name_after(lumps, map, "THINGS")?;
    let sectors = lump_by_name_after(lumps, map, "SECTORS")?;
    let linedefs = lump_by_name_after(lumps, map, "LINEDEFS")?;
    let sidedefs = lump_by_name_after(lumps, map, "SIDEDEFS")?;
    let n_sec = sectors.len() / 26;
    let n_line = linedefs.len() / 14;
    let n_side = sidedefs.len() / 30;
    let n = things.len() / 10;
    for i in 0..n {
        let o = i * 10;
        let typ = u16_le(things, o + 6);
        if typ != 1 {
            continue;
        }
        let tx = i16_le(things, o) as f32;
        let ty = i16_le(things, o + 2) as f32;
        let angle = u16_le(things, o + 4) as f32;
        let mut floor_z = if n_sec > 0 {
            i16_le(sectors, 0) as f32 / 64.0
        } else {
            0.0
        };
        // Nearest linedef's sidedef sector — sector 0 is often a pit.
        let mut best_d = f32::INFINITY;
        for li in 0..n_line {
            let lo = li * 14;
            let v1 = u16_le(linedefs, lo) as usize;
            let v2 = u16_le(linedefs, lo + 2) as usize;
            let vertexes = lump_by_name_after(lumps, map, "VERTEXES")?;
            let n_vert = vertexes.len() / 4;
            if v1 >= n_vert || v2 >= n_vert {
                continue;
            }
            let ax = i16_le(vertexes, v1 * 4) as f32;
            let ay = i16_le(vertexes, v1 * 4 + 2) as f32;
            let bx = i16_le(vertexes, v2 * 4) as f32;
            let by = i16_le(vertexes, v2 * 4 + 2) as f32;
            let dx = bx - ax;
            let dy = by - ay;
            let len2 = dx * dx + dy * dy;
            if len2 < 1.0 {
                continue;
            }
            let t = ((tx - ax) * dx + (ty - ay) * dy) / len2;
            let t = t.clamp(0.0, 1.0);
            let px = ax + dx * t;
            let py = ay + dy * t;
            let d = (tx - px) * (tx - px) + (ty - py) * (ty - py);
            if d >= best_d {
                continue;
            }
            let cross = (bx - ax) * (ty - ay) - (by - ay) * (tx - ax);
            // Doom's front side is where the point is right of the line
            // (cross < 0, same convention as R_PointOnSide).
            let side = if cross < 0.0 {
                u16_le(linedefs, lo + 10)
            } else {
                u16_le(linedefs, lo + 12)
            };
            if side == 0xFFFF || side as usize >= n_side {
                continue;
            }
            let sec = u16_le(sidedefs, side as usize * 30 + 28) as usize;
            if sec >= n_sec {
                continue;
            }
            best_d = d;
            floor_z = i16_le(sectors, sec * 26) as f32 / 64.0;
        }
        let x = tx / 64.0;
        let y = ty / 64.0;
        let yaw = std::f32::consts::FRAC_PI_2 + angle.to_radians();
        return Some(WorldSpawn {
            // Doom VIEWHEIGHT = 41 map units.
            pos: [x, floor_z + 41.0 / 64.0, y],
            yaw,
            pitch: 0.0,
        });
    }
    None
}

pub(crate) fn doom_map_place(
    lumps: &[WadLump],
    map: &str,
    source: &str,
    world_key: &str,
    wad_slug: &str,
) -> crate::world_place::WorldPlace {
    let spawn = doom_map_spawn(lumps, map).map(|s| (s.pos, s.yaw, s.pitch));
    let mut places = Vec::new();
    if let Some(things) = lump_by_name_after(lumps, map, "THINGS") {
        let n = things.len() / 10;
        for i in 0..n {
            let o = i * 10;
            let typ = u16_le(things, o + 6);
            let Some((kind, prefix)) = crate::world_place::doom_thing_actor(typ) else {
                continue;
            };
            let tx = i16_le(things, o) as f32 / 64.0;
            let ty = i16_le(things, o + 2) as f32 / 64.0;
            let angle = u16_le(things, o + 4) as f32;
            let yaw = std::f32::consts::FRAC_PI_2 + angle.to_radians();
            let asset = if prefix.is_empty() {
                String::new()
            } else {
                format!("billboards/{wad_slug}/{prefix}")
            };
            places.push(crate::world_place::Place {
                id: format!("thing-{i}"),
                kind: kind.into(),
                asset,
                pos: [tx, 0.0, ty],
                yaw,
                class: typ.to_string(),
                width: 0.0,
                height: 0.0,
                align: String::new(),
            });
        }
    }
    crate::world_place::WorldPlace {
        source: source.into(),
        world: world_key.into(),
        spawn,
        places,
    }
}

pub(crate) fn quake_bsp_spawn(bytes: &[u8]) -> Option<WorldSpawn> {
    if bytes.len() < 4 + 8 {
        return None;
    }
    let off = u32_le(bytes, 4) as usize;
    let len = u32_le(bytes, 8) as usize;
    if off + len > bytes.len() || len == 0 {
        return None;
    }
    let text = std::str::from_utf8(&bytes[off..off + len]).ok()?;
    let mut best: Option<WorldSpawn> = None;
    let mut block_class = String::new();
    let mut origin;
    let mut angle;
    let flush = |class: &str, origin: Option<[f32; 3]>, angle: f32| -> Option<WorldSpawn> {
        let o = origin?;
        let rank = match class {
            "info_player_start" => 0,
            "info_player_coop" => 1,
            "info_player_deathmatch" => 2,
            _ => return None,
        };
        let _ = rank;
        // Quake Z-up: (x,y,z) → engine (x, z, −y) / 32.
        // info_player_start origin is the player origin; view is +22.
        let pos = [o[0] / 32.0, o[2] / 32.0 + 22.0 / 32.0, -o[1] / 32.0];
        let yaw = std::f32::consts::FRAC_PI_2 - angle.to_radians();
        Some(WorldSpawn {
            pos,
            yaw,
            pitch: 0.0,
        })
    };
    for raw in text.split(|c| c == '{' || c == '}') {
        let block = raw.trim();
        if block.is_empty() {
            continue;
        }
        block_class.clear();
        origin = None;
        angle = 0.0;
        for line in block.lines() {
            let line = line.trim();
            if !line.starts_with('"') {
                continue;
            }
            let mut parts = line.splitn(4, '"').filter(|s| !s.is_empty() && *s != " ");
            let key = parts.next().unwrap_or("");
            let _sep = parts.next();
            let val = parts.next().unwrap_or("");
            // splitn on " is messy — parse "key" "value"
            let kv: Vec<&str> = line
                .split('"')
                .filter(|s| !s.trim().is_empty())
                .collect();
            if kv.len() < 2 {
                let _ = (key, val);
                continue;
            }
            match kv[0] {
                "classname" => block_class = kv[1].to_string(),
                "origin" => {
                    let mut it = kv[1].split_whitespace();
                    if let (Some(x), Some(y), Some(z)) = (it.next(), it.next(), it.next()) {
                        if let (Ok(x), Ok(y), Ok(z)) =
                            (x.parse::<f32>(), y.parse::<f32>(), z.parse::<f32>())
                        {
                            origin = Some([x, y, z]);
                        }
                    }
                }
                "angle" => {
                    if let Ok(a) = kv[1].trim().parse::<f32>() {
                        angle = a;
                    }
                }
                _ => {}
            }
        }
        if let Some(spawn) = flush(&block_class, origin, angle) {
            if block_class == "info_player_start" {
                return Some(spawn);
            }
            if best.is_none() {
                best = Some(spawn);
            }
        }
    }
    best
}

pub(crate) fn quake_bsp_place(bytes: &[u8], source: &str, world_key: &str) -> crate::world_place::WorldPlace {
    let spawn = quake_bsp_spawn(bytes).map(|s| (s.pos, s.yaw, s.pitch));
    let mut places = Vec::new();
    if bytes.len() >= 12 {
        let off = u32_le(bytes, 4) as usize;
        let len = u32_le(bytes, 8) as usize;
        if off + len <= bytes.len() {
            if let Ok(text) = std::str::from_utf8(&bytes[off..off + len]) {
                let mut i = 0usize;
                for raw in text.split(|c| c == '{' || c == '}') {
                    let block = raw.trim();
                    if block.is_empty() {
                        continue;
                    }
                    let mut class = String::new();
                    let mut origin = None;
                    let mut angle = 0.0f32;
                    let mut model = String::new();
                    for line in block.lines() {
                        let kv: Vec<&str> = line
                            .split('"')
                            .filter(|s| !s.trim().is_empty())
                            .collect();
                        if kv.len() < 2 {
                            continue;
                        }
                        match kv[0] {
                            "classname" => class = kv[1].to_string(),
                            "origin" => {
                                let mut it = kv[1].split_whitespace();
                                if let (Some(x), Some(y), Some(z)) =
                                    (it.next(), it.next(), it.next())
                                {
                                    if let (Ok(x), Ok(y), Ok(z)) =
                                        (x.parse::<f32>(), y.parse::<f32>(), z.parse::<f32>())
                                    {
                                        origin = Some([x, y, z]);
                                    }
                                }
                            }
                            "angle" => {
                                if let Ok(a) = kv[1].trim().parse::<f32>() {
                                    angle = a;
                                }
                            }
                            "model" => model = kv[1].to_string(),
                            _ => {}
                        }
                    }
                    let Some(o) = origin else { continue };
                    let mapped = crate::world_place::quake_class_actor(&class);
                    let (kind, asset) = if let Some((k, key)) = mapped {
                        (k, key.to_string())
                    } else if model.ends_with(".mdl") {
                        let slug = model
                            .rsplit('/')
                            .next()
                            .and_then(|n| n.strip_suffix(".mdl"))
                            .unwrap_or("model");
                        ("prop", format!("props/{slug}"))
                    } else {
                        continue;
                    };
                    let pos = [o[0] / 32.0, o[2] / 32.0, -o[1] / 32.0];
                    let yaw = std::f32::consts::FRAC_PI_2 - angle.to_radians();
                    places.push(crate::world_place::Place {
                        id: format!("ent-{i}"),
                        kind: kind.into(),
                        asset,
                        pos,
                        yaw,
                        class,
                        width: 0.0,
                        height: 0.0,
                        align: String::new(),
                    });
                    i += 1;
                }
            }
        }
    }
    crate::world_place::WorldPlace {
        source: source.into(),
        world: world_key.into(),
        spawn,
        places,
    }
}

pub(crate) fn doom_map_to_mesh(
    lumps: &[WadLump],
    map: &str,
    palette: &[[u8; 3]; 256],
) -> Option<MapMesh> {
    let vertexes = lump_by_name_after(lumps, map, "VERTEXES")?;
    let linedefs = lump_by_name_after(lumps, map, "LINEDEFS")?;
    let sidedefs = lump_by_name_after(lumps, map, "SIDEDEFS")?;
    let sectors = lump_by_name_after(lumps, map, "SECTORS")?;
    if vertexes.len() < 4 || linedefs.len() < 14 || sectors.len() < 26 {
        return None;
    }
    let n_vert = vertexes.len() / 4;
    let n_line = linedefs.len() / 14;
    let n_side = sidedefs.len() / 30;
    let n_sec = sectors.len() / 26;

    let mut verts: Vec<[f32; 2]> = Vec::with_capacity(n_vert);
    for i in 0..n_vert {
        let x = i16_le(vertexes, i * 4) as f32;
        let y = i16_le(vertexes, i * 4 + 2) as f32;
        verts.push([x, y]);
    }
    let mut sec_floor = vec![0i16; n_sec];
    let mut sec_ceil = vec![0i16; n_sec];
    let mut sec_floor_tex = vec![String::new(); n_sec];
    let mut sec_ceil_tex = vec![String::new(); n_sec];
    for i in 0..n_sec {
        let o = i * 26;
        sec_floor[i] = i16_le(sectors, o);
        sec_ceil[i] = i16_le(sectors, o + 2);
        sec_floor_tex[i] = lump_name(&sectors[o + 4..o + 12]);
        sec_ceil_tex[i] = lump_name(&sectors[o + 12..o + 20]);
    }

    // Build texture atlas from referenced flats + wall patches (best-effort).
    let mut atlas_images: BTreeMap<String, RgbaImage> = BTreeMap::new();
    // Default gray tile (opaque — alpha 0x80 used to tint slivers against the sky).
    atlas_images.insert("_default".into(), opaque_gray_tile());

    // Collect flats used by sectors.
    for i in 0..n_sec {
        for name in [&sec_floor_tex[i], &sec_ceil_tex[i]] {
            if name.is_empty() || is_sky_flat(name) {
                continue;
            }
            if atlas_images.contains_key(name) {
                continue;
            }
            if let Some(lump) = lumps.iter().find(|l| l.name == *name) {
                if lump.data.len() == 64 * 64 {
                    let mut rgba = indexed_to_rgba(&lump.data, palette, 255);
                    for px in rgba.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                    atlas_images.insert(
                        name.clone(),
                        RgbaImage {
                            w: 64,
                            h: 64,
                            rgba,
                        },
                    );
                }
            }
        }
    }

    // Only textures this map actually paints. Packing the whole TEXTURE1
    // directory overflows a 2k atlas, flats get dropped, and lookup falls
    // back to UVs over the entire atlas (the mosaic thumbs).
    let mut needed: BTreeSet<String> = BTreeSet::new();
    for i in 0..n_sec {
        for name in [&sec_floor_tex[i], &sec_ceil_tex[i]] {
            if !name.is_empty() && !is_sky_flat(name) {
                needed.insert(name.clone());
            }
        }
    }
    for i in 0..n_side {
        let o = i * 30;
        for name_off in [4usize, 12, 20] {
            let name = lump_name(&sidedefs[o + name_off..o + name_off + 8]);
            if !name.is_empty() && name != "-" {
                needed.insert(name);
            }
        }
    }
    let composed = compose_doom_textures(lumps, palette);
    for (name, img) in composed {
        if needed.contains(&name) && !atlas_images.contains_key(&name) {
            atlas_images.insert(name, img);
        }
    }
    // Side texture names (upper/lower/mid) — patch fallback if no TEXTURE1.
    for i in 0..n_side {
        let o = i * 30;
        for name_off in [4usize, 12, 20] {
            let name = lump_name(&sidedefs[o + name_off..o + name_off + 8]);
            if name.is_empty() || name == "-" {
                continue;
            }
            if atlas_images.contains_key(&name) {
                continue;
            }
            if let Some(lump) = lumps.iter().find(|l| l.name == name) {
                if let Ok(img) = doom_patch_to_rgba(&lump.data, palette) {
                    atlas_images.insert(name, img);
                }
            }
        }
    }

    let (atlas_png, uv_map) = pack_atlas(&atlas_images);

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let scale = 1.0 / 64.0; // Doom units → rough meters

    // Floors / ceilings. Vanilla SSECTORs are not closed polygons — the
    // node builder only stores linedef segs, so a leaf can be one seg plus
    // implicit partition edges (and a corner that is not in VERTEXES).
    // GL ports add minisegs; we close each leaf by clipping the map box
    // against ancestor partitions and segs. Linedef loops only fill maps
    // that have no BSP. Walls come from segs so they share those verts.
    let segs = lump_by_name_after(lumps, map, "SEGS").unwrap_or(&[]);
    let ssectors = lump_by_name_after(lumps, map, "SSECTORS").unwrap_or(&[]);
    let nodes = lump_by_name_after(lumps, map, "NODES").unwrap_or(&[]);
    let has_bsp = segs.len() >= 12 && ssectors.len() >= 4;

    if has_bsp {
        emit_walls_from_segs(
            &mut positions,
            &mut uvs,
            &mut indices,
            segs,
            linedefs,
            sidedefs,
            &verts,
            n_vert,
            n_line,
            n_side,
            n_sec,
            &sec_floor,
            &sec_ceil,
            &sec_ceil_tex,
            &uv_map,
            scale,
        );
    } else {
        emit_walls_from_linedefs(
            &mut positions,
            &mut uvs,
            &mut indices,
            linedefs,
            sidedefs,
            &verts,
            n_vert,
            n_line,
            n_side,
            n_sec,
            &sec_floor,
            &sec_ceil,
            &sec_ceil_tex,
            &uv_map,
            scale,
        );
    }
    let covered = emit_subsector_flats(
        &mut positions,
        &mut uvs,
        &mut indices,
        segs,
        ssectors,
        nodes,
        linedefs,
        sidedefs,
        &verts,
        n_vert,
        n_line,
        n_side,
        n_sec,
        &sec_floor,
        &sec_ceil,
        &sec_floor_tex,
        &sec_ceil_tex,
        &uv_map,
        scale,
    );
    if !has_bsp {
        let mut sector_edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n_sec];
        for li in 0..n_line {
            let o = li * 14;
            let v1 = u16_le(linedefs, o) as usize;
            let v2 = u16_le(linedefs, o + 2) as usize;
            if v1 >= n_vert || v2 >= n_vert {
                continue;
            }
            let side_r = u16_le(linedefs, o + 10);
            let side_l = u16_le(linedefs, o + 12);
            if side_r != 0xFFFF && (side_r as usize) < n_side {
                let sector = u16_le(sidedefs, side_r as usize * 30 + 28) as usize;
                if sector < n_sec {
                    sector_edges[sector].push((v1, v2));
                }
            }
            if side_l != 0xFFFF && (side_l as usize) < n_side {
                let sector = u16_le(sidedefs, side_l as usize * 30 + 28) as usize;
                if sector < n_sec {
                    sector_edges[sector].push((v2, v1));
                }
            }
        }
        for edges in &mut sector_edges {
            drop_self_ref_pairs(edges);
        }
        for si in 0..n_sec {
            if covered.contains(&si) || sec_ceil[si] <= sec_floor[si] {
                continue;
            }
            let floor_tex = if sec_floor_tex[si].is_empty() {
                "_default"
            } else {
                sec_floor_tex[si].as_str()
            };
            let ceil_tex = if sec_ceil_tex[si].is_empty() || is_sky_flat(&sec_ceil_tex[si]) {
                ""
            } else {
                sec_ceil_tex[si].as_str()
            };
            let floor_z = sec_floor[si] as f32 * scale;
            let ceil_z = sec_ceil[si] as f32 * scale;
            emit_sector_grid(
                &mut positions,
                &mut uvs,
                &mut indices,
                &sector_edges[si],
                &verts,
                scale,
                floor_z,
                lookup_slot(&uv_map, floor_tex),
                false,
            );
            if !ceil_tex.is_empty() {
                emit_sector_grid(
                    &mut positions,
                    &mut uvs,
                    &mut indices,
                    &sector_edges[si],
                    &verts,
                    scale,
                    ceil_z,
                    lookup_slot(&uv_map, ceil_tex),
                    true,
                );
            }
        }
    }

    if positions.is_empty() || indices.is_empty() {
        // Degenerate map — emit a unit floor so convert still produces a World GLB.
        positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        indices = vec![0, 1, 2, 0, 2, 3];
    }

    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: None,
        uvs: &uvs,
        indices: &indices,
        base_color_png: &atlas_png,
        metallic_roughness_png: None,
        double_sided: true,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return None;
    }
    Some(MapMesh {
        glb,
        tris: indices.len() / 3,
    })
}

/// Atlas UV rect in 0..1 (min_u, min_v, max_u, max_v).
pub(crate) type UvRect = [f32; 4];

#[derive(Clone, Copy, Debug)]
pub(crate) struct AtlasSlot {
    pub uv: UvRect,
    pub w: u32,
    pub h: u32,
}

/// Map 0..1 onto the inner tile. A small overshoot is allowed so bilinear
/// / overlapping cell edges sample the wrap gutter, not the next atlas image.
pub(crate) fn slot_uv(slot: AtlasSlot, local_u: f32, local_v: f32) -> [f32; 2] {
    let slop_u = 2.0 / slot.w.max(1) as f32;
    let slop_v = 2.0 / slot.h.max(1) as f32;
    let local_u = local_u.clamp(-slop_u, 1.0 + slop_u);
    let local_v = local_v.clamp(-slop_v, 1.0 + slop_v);
    let du = slot.uv[2] - slot.uv[0];
    let dv = slot.uv[3] - slot.uv[1];
    [slot.uv[0] + du * local_u, slot.uv[1] + dv * local_v]
}

/// Extra texels around each atlas tile, filled with a wrap of the tile so
/// bilinear at u=0/1 (and a 1-unit cell overlap) does not show a seam or
/// sample the neighbour image.
const ATLAS_GUTTER: u32 = 2;

pub(crate) fn blit_wrapped(
    rgba: &mut [u8],
    atlas_w: u32,
    px: u32,
    py: u32,
    packed_w: u32,
    packed_h: u32,
    img: &RgbaImage,
    gutter: u32,
) {
    let iw = img.w.max(1);
    let ih = img.h.max(1);
    for row in 0..packed_h {
        for col in 0..packed_w {
            let sx = (col as i32 - gutter as i32).rem_euclid(iw as i32) as u32;
            let sy = (row as i32 - gutter as i32).rem_euclid(ih as i32) as u32;
            if sx >= img.w || sy >= img.h {
                continue;
            }
            let src = ((sy * img.w + sx) * 4) as usize;
            let dst = (((py + row) * atlas_w + px + col) * 4) as usize;
            if dst + 4 <= rgba.len() && src + 4 <= img.rgba.len() {
                rgba[dst..dst + 4].copy_from_slice(&img.rgba[src..src + 4]);
            }
        }
    }
}

pub(crate) fn pack_atlas(images: &BTreeMap<String, RgbaImage>) -> (Vec<u8>, BTreeMap<String, AtlasSlot>) {
    let mut entries: Vec<(&String, &RgbaImage)> = images.iter().collect();
    entries.sort_by(|a, b| b.1.w.cmp(&a.1.w).then(b.1.h.cmp(&a.1.h)));
    let mut atlas_w = 64u32;
    let mut atlas_h = 64u32;
    let mut placed: BTreeMap<String, (u32, u32, u32, u32)> = BTreeMap::new();
    // Simple shelf pack. Stored rect is the packed (guttered) box.
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_h = 0u32;
    const MAX: u32 = 4096;
    let pack_one = |iw: u32, ih: u32| -> (u32, u32) {
        (iw.max(1) + 2 * ATLAS_GUTTER, ih.max(1) + 2 * ATLAS_GUTTER)
    };
    // Pin _default at the origin so it cannot be dropped when the shelf fills.
    if let Some(def) = images.get("_default") {
        let (w, h) = pack_one(def.w, def.h);
        placed.insert("_default".into(), (0, 0, w, h));
        x = w;
        row_h = h;
        atlas_w = atlas_w.max(w);
        atlas_h = atlas_h.max(h);
    }
    for (name, img) in &entries {
        if placed.contains_key(*name) {
            continue;
        }
        let (w, h) = pack_one(img.w, img.h);
        if x + w > atlas_w {
            if atlas_w < MAX {
                atlas_w = (atlas_w * 2).min(MAX).max(x + w);
            }
            if x + w > atlas_w {
                x = 0;
                y += row_h;
                row_h = 0;
            }
        }
        if y + h > atlas_h {
            atlas_h = (atlas_h * 2).min(MAX).max(y + h);
        }
        if y + h > MAX || x + w > MAX {
            // Skip oversized leftovers.
            continue;
        }
        placed.insert((*name).clone(), (x, y, w, h));
        x += w;
        row_h = row_h.max(h);
    }
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];
    let mut uv_map = BTreeMap::new();
    for (name, (px, py, packed_w, packed_h)) in &placed {
        if let Some(img) = images.get(name) {
            blit_wrapped(
                &mut rgba,
                atlas_w,
                *px,
                *py,
                *packed_w,
                *packed_h,
                img,
                ATLAS_GUTTER,
            );
            // UV covers the inner (unguttered) tile so 0 and 1 are the
            // texel edges; filtering reads the wrap gutter outside.
            let u0 = (*px + ATLAS_GUTTER) as f32 / atlas_w as f32;
            let v0 = (*py + ATLAS_GUTTER) as f32 / atlas_h as f32;
            let u1 = (*px + ATLAS_GUTTER + img.w.max(1)) as f32 / atlas_w as f32;
            let v1 = (*py + ATLAS_GUTTER + img.h.max(1)) as f32 / atlas_h as f32;
            uv_map.insert(
                name.clone(),
                AtlasSlot {
                    uv: [u0, v0, u1, v1],
                    w: img.w.max(1),
                    h: img.h.max(1),
                },
            );
        }
    }
    // Never map a missing tile across the whole atlas — that is the mosaic
    // thumb. Steal a 2×2 of the first placed texel if _default did not fit.
    if !uv_map.contains_key("_default") {
        let slot = uv_map
            .values()
            .next()
            .copied()
            .unwrap_or(AtlasSlot {
                uv: [0.0, 0.0, 0.02, 0.02],
                w: 64,
                h: 64,
            });
        uv_map.insert("_default".into(), slot);
    }
    let png = encode_png_rgba(&rgba, atlas_w, atlas_h).unwrap_or_else(|_| {
        encode_png_rgba(&vec![0x80; 64 * 64 * 4], 64, 64).unwrap()
    });
    (png, uv_map)
}

/// Snap world positions to 1/16 Doom unit so floors and walls share edges.
pub(crate) fn snap_pos(p: [f32; 3]) -> [f32; 3] {
    const Q: f32 = 1.0 / 1024.0;
    [
        (p[0] / Q).round() * Q,
        (p[1] / Q).round() * Q,
        (p[2] / Q).round() * Q,
    ]
}

pub(crate) fn push_tri_uv(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    ua: [f32; 2],
    ub: [f32; 2],
    uc: [f32; 2],
) {
    let base = positions.len() as u32;
    positions.extend_from_slice(&[snap_pos(a), snap_pos(b), snap_pos(c)]);
    uvs.extend_from_slice(&[ua, ub, uc]);
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}

pub(crate) fn push_quad_uv(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    ua: [f32; 2],
    ub: [f32; 2],
    uc: [f32; 2],
    ud: [f32; 2],
) {
    push_tri_uv(positions, uvs, indices, a, b, c, ua, ub, uc);
    push_tri_uv(positions, uvs, indices, a, c, d, ua, uc, ud);
}

pub(crate) fn splits_along(a: f32, b: f32) -> Vec<f32> {
    let mut out = vec![a];
    let lo = a.min(b);
    let hi = a.max(b);
    let mut i = lo.ceil() as i32;
    let last = hi.floor() as i32;
    while i <= last {
        let t = i as f32;
        if t > lo + 1e-4 && t < hi - 1e-4 {
            out.push(t);
        }
        i += 1;
    }
    out.push(b);
    if b < a {
        out.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
    } else {
        out.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    }
    out.dedup_by(|x, y| (*x - *y).abs() < 1e-5);
    out
}

/// Map a strip `[a, b]` that already lies in one tile period onto 0..1 of
/// that tile, preserving direction.
///
/// Using `a.floor()` as the origin (the old `period_local`) collapses a
/// *decreasing* strip to a single texel. Doom walls are almost always
/// decreasing in V (pegged from the top), which painted a 1px row of the
/// texture stretched up the whole wall.
pub(crate) fn period_span(a: f32, b: f32) -> (f32, f32) {
    let base = a.min(b).floor();
    ((a - base).clamp(0.0, 1.0), (b - base).clamp(0.0, 1.0))
}

/// Wall tessellated so no triangle spans more than one atlas tile period.
pub(crate) fn push_wall_tiled(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    p0: [f32; 2],
    p1: [f32; 2],
    z0: f32,
    z1: f32,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    slot: AtlasSlot,
) {
    let tw = slot.w.max(1) as f32;
    let th = slot.h.max(1) as f32;
    let su0 = u0 / tw;
    let su1 = u1 / tw;
    let sv0 = v0 / th;
    let sv1 = v1 / th;
    let us = splits_along(su0, su1);
    let vs = splits_along(sv0, sv1);
    if us.len() < 2 || vs.len() < 2 {
        return;
    }
    let du = su1 - su0;
    let dv = sv1 - sv0;
    for ui in 0..us.len() - 1 {
        let ua = us[ui];
        let ub = us[ui + 1];
        let t0 = if du.abs() < 1e-6 { 0.0 } else { (ua - su0) / du };
        let t1 = if du.abs() < 1e-6 { 1.0 } else { (ub - su0) / du };
        let ax = p0[0] + (p1[0] - p0[0]) * t0;
        let ay = p0[1] + (p1[1] - p0[1]) * t0;
        let bx = p0[0] + (p1[0] - p0[0]) * t1;
        let by = p0[1] + (p1[1] - p0[1]) * t1;
        for vi in 0..vs.len() - 1 {
            let va = vs[vi];
            let vb = vs[vi + 1];
            let s0 = if dv.abs() < 1e-6 { 0.0 } else { (va - sv0) / dv };
            let s1 = if dv.abs() < 1e-6 { 1.0 } else { (vb - sv0) / dv };
            let za = z0 + (z1 - z0) * s0;
            let zb = z0 + (z1 - z0) * s1;
            let (lu0, lu1) = period_span(ua, ub);
            let (lv0, lv1) = period_span(va, vb);
            push_quad_uv(
                positions,
                uvs,
                indices,
                [ax, za, ay],
                [bx, za, by],
                [bx, zb, by],
                [ax, zb, ay],
                slot_uv(slot, lu0, lv0),
                slot_uv(slot, lu1, lv0),
                slot_uv(slot, lu1, lv1),
                slot_uv(slot, lu0, lv1),
            );
        }
    }
}

/// Doom units walls poke past the flat so residual T-junctions hide in
/// the floor/ceiling. Mids (railings) stay flush.
const WALL_APRON: f32 = 4.0;

pub(crate) fn sidedef_has_mid(sidedefs: &[u8], snum: u16, n_side: usize) -> bool {
    if snum == 0xFFFF || snum as usize >= n_side {
        return false;
    }
    let name = lump_name(&sidedefs[snum as usize * 30 + 20..snum as usize * 30 + 28]);
    name != "-" && !name.is_empty()
}

pub(crate) fn emit_wall_piece(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    p1: [f32; 2],
    p2: [f32; 2],
    u0: f32,
    u1: f32,
    flags: u16,
    two_sided: bool,
    side: u16,
    snum: u16,
    other_snum: u16,
    sidedefs: &[u8],
    n_side: usize,
    n_sec: usize,
    sec_floor: &[i16],
    sec_ceil: &[i16],
    sec_ceil_tex: &[String],
    uv_map: &BTreeMap<String, AtlasSlot>,
    scale: f32,
) {
    if snum == 0xFFFF || snum as usize >= n_side {
        return;
    }
    let so = snum as usize * 30;
    let sector = u16_le(sidedefs, so + 28) as usize;
    if sector >= n_sec {
        return;
    }
    let mid = lump_name(&sidedefs[so + 20..so + 28]);
    let lower = lump_name(&sidedefs[so + 12..so + 20]);
    let upper = lump_name(&sidedefs[so + 4..so + 12]);
    let yoff = i16_le(sidedefs, so + 2) as f32;
    let floor_z = sec_floor[sector] as f32 * scale;
    let ceil_z = sec_ceil[sector] as f32 * scale;
    let ax = p1[0] * scale;
    let ay = p1[1] * scale;
    let bx = p2[0] * scale;
    let by = p2[1] * scale;
    let apron = WALL_APRON * scale;
    let lower_unpeg = flags & 0x0010 != 0;
    let upper_unpeg = flags & 0x0008 != 0;

    if !two_sided {
        let tex = if mid != "-" && !mid.is_empty() {
            mid.as_str()
        } else {
            "_default"
        };
        let slot = lookup_slot(uv_map, tex);
        let th = slot.h.max(1) as f32;
        let wall_h = ((sec_ceil[sector] - sec_floor[sector]) as f32).abs();
        let (v_top, v_bot) = if lower_unpeg {
            let v_bot = yoff + th;
            (v_bot - wall_h, v_bot)
        } else {
            (yoff, yoff + wall_h)
        };
        push_wall_tiled(
            positions,
            uvs,
            indices,
            [ax, ay],
            [bx, by],
            floor_z - apron,
            ceil_z + apron,
            u0,
            u1,
            v_bot + WALL_APRON,
            v_top - WALL_APRON,
            slot,
        );
        return;
    }
    let other = if other_snum != 0xFFFF && (other_snum as usize) < n_side {
        let os = u16_le(sidedefs, other_snum as usize * 30 + 28) as usize;
        if os < n_sec {
            Some(os)
        } else {
            None
        }
    } else {
        None
    };
    let Some(os) = other else {
        return;
    };
    let of = sec_floor[os] as f32 * scale;
    let oc = sec_ceil[os] as f32 * scale;
    if floor_z < of && !is_blank_tex(&lower) {
        let slot = lookup_slot(uv_map, lower.as_str());
        let th = slot.h.max(1) as f32;
        let step_h = (sec_floor[os] - sec_floor[sector]) as f32;
        let (v_top, v_bot) = if lower_unpeg {
            (yoff, yoff + step_h)
        } else {
            let v_bot = yoff + th;
            (v_bot - step_h, v_bot)
        };
        push_wall_tiled(
            positions,
            uvs,
            indices,
            [ax, ay],
            [bx, by],
            floor_z - apron,
            of + apron,
            u0,
            u1,
            v_bot + WALL_APRON,
            v_top - WALL_APRON,
            slot,
        );
    }
    if ceil_z > oc
        && !is_sky_flat(&sec_ceil_tex[sector])
        && !is_sky_flat(&sec_ceil_tex[os])
        && !is_blank_tex(&upper)
    {
        let slot = lookup_slot(uv_map, upper.as_str());
        let th = slot.h.max(1) as f32;
        let step_h = (sec_ceil[sector] - sec_ceil[os]) as f32;
        let (v_top, v_bot) = if upper_unpeg {
            (yoff, yoff + step_h)
        } else {
            let v_top = yoff;
            (v_top, v_top + step_h.min(th))
        };
        push_wall_tiled(
            positions,
            uvs,
            indices,
            [ax, ay],
            [bx, by],
            oc - apron,
            ceil_z + apron,
            u0,
            u1,
            v_bot + WALL_APRON,
            v_top - WALL_APRON,
            slot,
        );
    }
    // Two-sided mid (grates, fences) lives on both sidedefs and both
    // segs. Emitting it twice is coplanar z-fight. Keep the front only
    // unless the back is the only side that painted a mid.
    if mid != "-"
        && !mid.is_empty()
        && (side == 0 || !sidedef_has_mid(sidedefs, other_snum, n_side))
    {
        let slot = lookup_slot(uv_map, &mid);
        let th = slot.h.max(1) as f32;
        let mid_lo = floor_z.max(of);
        let mid_hi = ceil_z.min(oc);
        if mid_hi > mid_lo + 0.01 {
            let wall_h = (mid_hi - mid_lo) / scale;
            let (v_top, v_bot) = if lower_unpeg {
                let v_bot = yoff + th;
                (v_bot - wall_h, v_bot)
            } else {
                (yoff, yoff + wall_h)
            };
            push_wall_tiled(
                positions,
                uvs,
                indices,
                [ax, ay],
                [bx, by],
                mid_lo,
                mid_hi,
                u0,
                u1,
                v_bot,
                v_top,
                slot,
            );
        }
    }
}

pub(crate) fn emit_walls_from_segs(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    segs: &[u8],
    linedefs: &[u8],
    sidedefs: &[u8],
    verts: &[[f32; 2]],
    n_vert: usize,
    n_line: usize,
    n_side: usize,
    n_sec: usize,
    sec_floor: &[i16],
    sec_ceil: &[i16],
    sec_ceil_tex: &[String],
    uv_map: &BTreeMap<String, AtlasSlot>,
    scale: f32,
) {
    let n_seg = segs.len() / 12;
    for si in 0..n_seg {
        let o = si * 12;
        let v1 = u16_le(segs, o) as usize;
        let v2 = u16_le(segs, o + 2) as usize;
        let line = u16_le(segs, o + 6) as usize;
        let side = u16_le(segs, o + 8);
        let offset = i16_le(segs, o + 10) as f32;
        if v1 >= n_vert || v2 >= n_vert || line >= n_line {
            continue;
        }
        let lo = line * 14;
        let flags = u16_le(linedefs, lo + 4);
        let side_r = u16_le(linedefs, lo + 10);
        let side_l = u16_le(linedefs, lo + 12);
        let two_sided = flags & 0x0004 != 0;
        let (snum, other) = if side == 0 {
            (side_r, side_l)
        } else {
            (side_l, side_r)
        };
        if snum == 0xFFFF || snum as usize >= n_side {
            continue;
        }
        let xoff = i16_le(sidedefs, snum as usize * 30) as f32;
        let p1 = verts[v1];
        let p2 = verts[v2];
        let len = (p2[0] - p1[0]).hypot(p2[1] - p1[1]).max(1.0);
        emit_wall_piece(
            positions,
            uvs,
            indices,
            p1,
            p2,
            xoff + offset,
            xoff + offset + len,
            flags,
            two_sided,
            side,
            snum,
            other,
            sidedefs,
            n_side,
            n_sec,
            sec_floor,
            sec_ceil,
            sec_ceil_tex,
            uv_map,
            scale,
        );
    }
}

pub(crate) fn emit_walls_from_linedefs(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    linedefs: &[u8],
    sidedefs: &[u8],
    verts: &[[f32; 2]],
    n_vert: usize,
    n_line: usize,
    n_side: usize,
    n_sec: usize,
    sec_floor: &[i16],
    sec_ceil: &[i16],
    sec_ceil_tex: &[String],
    uv_map: &BTreeMap<String, AtlasSlot>,
    scale: f32,
) {
    for li in 0..n_line {
        let o = li * 14;
        let v1 = u16_le(linedefs, o) as usize;
        let v2 = u16_le(linedefs, o + 2) as usize;
        let flags = u16_le(linedefs, o + 4);
        let side_r = u16_le(linedefs, o + 10);
        let side_l = u16_le(linedefs, o + 12);
        if v1 >= n_vert || v2 >= n_vert {
            continue;
        }
        let p1 = verts[v1];
        let p2 = verts[v2];
        let two_sided = flags & 0x0004 != 0;
        let len = (p2[0] - p1[0]).hypot(p2[1] - p1[1]).max(1.0);
        for (side, snum, other, a, b) in [
            (0u16, side_r, side_l, p1, p2),
            (1u16, side_l, side_r, p2, p1),
        ] {
            if snum == 0xFFFF || snum as usize >= n_side {
                continue;
            }
            let xoff = i16_le(sidedefs, snum as usize * 30) as f32;
            emit_wall_piece(
                positions,
                uvs,
                indices,
                a,
                b,
                xoff,
                xoff + len,
                flags,
                two_sided,
                side,
                snum,
                other,
                sidedefs,
                n_side,
                n_sec,
                sec_floor,
                sec_ceil,
                sec_ceil_tex,
                uv_map,
                scale,
            );
        }
    }
}

pub(crate) fn emit_floor_cell(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    ua: f32,
    va: f32,
    ub: f32,
    vb: f32,
    uc: f32,
    vc: f32,
    slot: AtlasSlot,
) {
    let base_u = ua.min(ub).min(uc).floor();
    let base_v = va.min(vb).min(vc).floor();
    push_tri_uv(
        positions,
        uvs,
        indices,
        a,
        b,
        c,
        slot_uv(slot, ua - base_u, va - base_v),
        slot_uv(slot, ub - base_u, vb - base_v),
        slot_uv(slot, uc - base_u, vc - base_v),
    );
}

pub(crate) fn same_uv_cell(ua: f32, va: f32, ub: f32, vb: f32, uc: f32, vc: f32) -> bool {
    let umin = ua.min(ub).min(uc);
    let umax = ua.max(ub).max(uc);
    let vmin = va.min(vb).min(vc);
    let vmax = va.max(vb).max(vc);
    umax <= umin.floor() + 1.0 + 1e-4 && vmax <= vmin.floor() + 1.0 + 1e-4
}

pub(crate) fn first_integer_crossing(a: f32, b: f32) -> Option<f32> {
    let lo = a.min(b);
    let hi = a.max(b);
    let mut i = lo.ceil();
    if (i - lo).abs() < 1e-4 {
        i += 1.0;
    }
    if i < hi - 1e-4 {
        Some(i)
    } else {
        None
    }
}

pub(crate) fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub(crate) fn push_floor_tri_tiled(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    ua: f32,
    va: f32,
    ub: f32,
    vb: f32,
    uc: f32,
    vc: f32,
    slot: AtlasSlot,
    depth: u32,
) {
    if depth > 10 || same_uv_cell(ua, va, ub, vb, uc, vc) {
        emit_floor_cell(positions, uvs, indices, a, b, c, ua, va, ub, vb, uc, vc, slot);
        return;
    }
    // Split on an integer U or V isoline so a triangle never spans more
    // than one atlas tile. Midpoint-split + clamp used to squash extra
    // periods into the last texel (1px stretch on Quake faces).
    let edges = [
        (a, b, ua, va, ub, vb, c, uc, vc),
        (b, c, ub, vb, uc, vc, a, ua, va),
        (c, a, uc, vc, ua, va, b, ub, vb),
    ];
    for (p, q, up, vp, uq, vq, r, ur, vr) in edges {
        if let Some(iu) = first_integer_crossing(up, uq) {
            let t = (iu - up) / (uq - up);
            if t > 1e-4 && t < 1.0 - 1e-4 {
                let m = lerp3(p, q, t);
                let vm = vp + (vq - vp) * t;
                push_floor_tri_tiled(
                    positions, uvs, indices, p, m, r, up, vp, iu, vm, ur, vr, slot, depth + 1,
                );
                push_floor_tri_tiled(
                    positions, uvs, indices, m, q, r, iu, vm, uq, vq, ur, vr, slot, depth + 1,
                );
                return;
            }
        }
        if let Some(iv) = first_integer_crossing(vp, vq) {
            let t = (iv - vp) / (vq - vp);
            if t > 1e-4 && t < 1.0 - 1e-4 {
                let m = lerp3(p, q, t);
                let um = up + (uq - up) * t;
                push_floor_tri_tiled(
                    positions, uvs, indices, p, m, r, up, vp, um, iv, ur, vr, slot, depth + 1,
                );
                push_floor_tri_tiled(
                    positions, uvs, indices, m, q, r, um, iv, uq, vq, ur, vr, slot, depth + 1,
                );
                return;
            }
        }
    }
    emit_floor_cell(positions, uvs, indices, a, b, c, ua, va, ub, vb, uc, vc, slot);
}

pub(crate) fn signed_area(pts: &[[f32; 2]]) -> f32 {
    let n = pts.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = pts[i];
        let q = pts[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

pub(crate) fn point_in_tri(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let v0 = [c[0] - a[0], c[1] - a[1]];
    let v1 = [b[0] - a[0], b[1] - a[1]];
    let v2 = [p[0] - a[0], p[1] - a[1]];
    let dot00 = v0[0] * v0[0] + v0[1] * v0[1];
    let dot01 = v0[0] * v1[0] + v0[1] * v1[1];
    let dot02 = v0[0] * v2[0] + v0[1] * v2[1];
    let dot11 = v1[0] * v1[0] + v1[1] * v1[1];
    let dot12 = v1[0] * v2[0] + v1[1] * v2[1];
    let den = dot00 * dot11 - dot01 * dot01;
    if den.abs() < 1e-12 {
        return false;
    }
    let u = (dot11 * dot02 - dot01 * dot12) / den;
    let v = (dot00 * dot12 - dot01 * dot02) / den;
    u >= -1e-5 && v >= -1e-5 && u + v <= 1.0 + 1e-5
}

pub(crate) fn is_ear(pts: &[[f32; 2]], idx: &[usize], i0: usize, i1: usize, i2: usize) -> bool {
    let a = pts[i0];
    let b = pts[i1];
    let c = pts[i2];
    let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if cross <= 1e-6 {
        return false;
    }
    for &i in idx {
        if i == i0 || i == i1 || i == i2 {
            continue;
        }
        if point_in_tri(pts[i], a, b, c) {
            return false;
        }
    }
    true
}

pub(crate) fn ear_clip(pts: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if signed_area(pts) < 0.0 {
        idx.reverse();
    }
    let mut tris = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut cut = None;
        for i in 0..m {
            let i0 = idx[(i + m - 1) % m];
            let i1 = idx[i];
            let i2 = idx[(i + 1) % m];
            if !is_ear(pts, &idx, i0, i1, i2) {
                continue;
            }
            cut = Some(i);
            tris.push([i0, i1, i2]);
            break;
        }
        match cut {
            Some(i) => {
                idx.remove(i);
            }
            None => break,
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

pub(crate) fn sector_loops(edges: &[(usize, usize)], verts: &[[f32; 2]]) -> Vec<Vec<usize>> {
    use std::collections::HashMap;
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &(a, _)) in edges.iter().enumerate() {
        adj.entry(a).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for start_ei in 0..edges.len() {
        if used[start_ei] {
            continue;
        }
        let start = edges[start_ei].0;
        let mut vs = vec![start];
        let mut prev = start;
        let mut cur = edges[start_ei].1;
        used[start_ei] = true;
        if cur != start {
            vs.push(cur);
        }
        let mut guard = 0;
        while cur != start && guard < 4096 {
            guard += 1;
            let Some(cands) = adj.get(&cur) else {
                break;
            };
            let p = verts.get(prev).copied().unwrap_or([0.0, 0.0]);
            let c = verts.get(cur).copied().unwrap_or([0.0, 0.0]);
            let back = (p[1] - c[1]).atan2(p[0] - c[0]);
            let mut best: Option<(f32, usize)> = None;
            for &ei in cands {
                if used[ei] {
                    continue;
                }
                let nxt = edges[ei].1;
                let n = verts.get(nxt).copied().unwrap_or([0.0, 0.0]);
                let ang = (n[1] - c[1]).atan2(n[0] - c[0]);
                let mut delta = back - ang;
                while delta <= 1e-4 {
                    delta += std::f32::consts::TAU;
                }
                if best.map_or(true, |(d, _)| delta < d) {
                    best = Some((delta, ei));
                }
            }
            let Some((_, ei)) = best else {
                break;
            };
            used[ei] = true;
            prev = cur;
            cur = edges[ei].1;
            if cur != start {
                vs.push(cur);
            }
        }
        if vs.len() >= 3 {
            loops.push(vs);
        }
    }
    loops
}

pub(crate) fn point_in_sector(p: [f32; 2], edges: &[(usize, usize)], verts: &[[f32; 2]]) -> bool {
    let mut hits = 0u32;
    for &(ia, ib) in edges {
        let Some(a) = verts.get(ia).copied() else {
            continue;
        };
        let Some(b) = verts.get(ib).copied() else {
            continue;
        };
        if (a[1] > p[1]) == (b[1] > p[1]) {
            continue;
        }
        let dy = b[1] - a[1];
        if dy.abs() < 1e-8 {
            continue;
        }
        let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / dy;
        if p[0] < x {
            hits += 1;
        }
    }
    hits % 2 == 1
}

pub(crate) fn push_unique_pt(pts: &mut Vec<[f32; 2]>, p: [f32; 2]) {
    if pts
        .iter()
        .any(|q| (q[0] - p[0]).abs() < 1e-4 && (q[1] - p[1]).abs() < 1e-4)
    {
        return;
    }
    pts.push(p);
}

pub(crate) fn seg_hit_h(a: [f32; 2], b: [f32; 2], y: f32, x0: f32, x1: f32) -> Option<[f32; 2]> {
    let dy = b[1] - a[1];
    if dy.abs() < 1e-12 {
        return None;
    }
    let t = (y - a[1]) / dy;
    if !(-1e-5..=1.0 + 1e-5).contains(&t) {
        return None;
    }
    let x = a[0] + (b[0] - a[0]) * t;
    if x >= x0 - 1e-4 && x <= x1 + 1e-4 {
        Some([x.clamp(x0, x1), y])
    } else {
        None
    }
}

pub(crate) fn seg_hit_v(a: [f32; 2], b: [f32; 2], x: f32, y0: f32, y1: f32) -> Option<[f32; 2]> {
    let dx = b[0] - a[0];
    if dx.abs() < 1e-12 {
        return None;
    }
    let t = (x - a[0]) / dx;
    if !(-1e-5..=1.0 + 1e-5).contains(&t) {
        return None;
    }
    let y = a[1] + (b[1] - a[1]) * t;
    if y >= y0 - 1e-4 && y <= y1 + 1e-4 {
        Some([x, y.clamp(y0, y1)])
    } else {
        None
    }
}

pub(crate) fn clip_cell_points(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    edges: &[(usize, usize)],
    verts: &[[f32; 2]],
    scale: f32,
) -> Vec<[f32; 2]> {
    let mut pts = Vec::new();
    let in_sec = |p: [f32; 2]| point_in_sector([p[0] / scale, p[1] / scale], edges, verts);
    for p in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]] {
        if in_sec(p) {
            push_unique_pt(&mut pts, p);
        }
    }
    for &(ia, ib) in edges {
        let Some(va) = verts.get(ia).copied() else {
            continue;
        };
        let Some(vb) = verts.get(ib).copied() else {
            continue;
        };
        let a = [va[0] * scale, va[1] * scale];
        let b = [vb[0] * scale, vb[1] * scale];
        for p in [a, b] {
            if p[0] >= x0 - 1e-4 && p[0] <= x1 + 1e-4 && p[1] >= y0 - 1e-4 && p[1] <= y1 + 1e-4 {
                push_unique_pt(&mut pts, [p[0].clamp(x0, x1), p[1].clamp(y0, y1)]);
            }
        }
        if let Some(p) = seg_hit_h(a, b, y0, x0, x1) {
            push_unique_pt(&mut pts, p);
        }
        if let Some(p) = seg_hit_h(a, b, y1, x0, x1) {
            push_unique_pt(&mut pts, p);
        }
        if let Some(p) = seg_hit_v(a, b, x0, y0, y1) {
            push_unique_pt(&mut pts, p);
        }
        if let Some(p) = seg_hit_v(a, b, x1, y0, y1) {
            push_unique_pt(&mut pts, p);
        }
    }
    if pts.len() < 3 {
        return pts;
    }
    let n = pts.len() as f32;
    let cx = pts.iter().map(|p| p[0]).sum::<f32>() / n;
    let cy = pts.iter().map(|p| p[1]).sum::<f32>() / n;
    pts.sort_by(|a, b| {
        (a[1] - cy)
            .atan2(a[0] - cx)
            .partial_cmp(&(b[1] - cy).atan2(b[0] - cx))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    pts
}

pub(crate) fn emit_clipped_cell(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    ox: f32,
    oy: f32,
    period: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
    edges: &[(usize, usize)],
    verts: &[[f32; 2]],
    scale: f32,
    depth: u32,
) {
    if x1 - x0 < 1e-4 || y1 - y0 < 1e-4 {
        return;
    }
    let in_sec = |p: [f32; 2]| point_in_sector([p[0] / scale, p[1] / scale], edges, verts);
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let n_in = corners.iter().filter(|p| in_sec(**p)).count();
    let probes = [
        [(x0 + x1) * 0.5, (y0 + y1) * 0.5],
        [x0 + (x1 - x0) * 0.25, (y0 + y1) * 0.5],
        [x1 - (x1 - x0) * 0.25, (y0 + y1) * 0.5],
        [(x0 + x1) * 0.5, y0 + (y1 - y0) * 0.25],
        [(x0 + x1) * 0.5, y1 - (y1 - y0) * 0.25],
    ];
    let hole = n_in == 4 && probes.iter().any(|p| !in_sec(*p));
    let clean = n_in == 4 && !hole;
    if clean {
        emit_cell_quad(
            positions, uvs, indices, corners, ox, oy, period, z, slot, flip,
        );
        return;
    }
    let pts = clip_cell_points(x0, y0, x1, y1, edges, verts, scale);
    // Diagonal sector edges used to hit this leaf with 3–6 points and
    // skip split — angular sort then dropped a triangle (MAP27 streaks,
    // MAP24 water around the pillar). Always carve mixed cells smaller.
    let mixed = n_in > 0 && n_in < 4;
    let crosses = n_in == 0 && pts.len() >= 3;
    // Two splits (16-unit leaves) close diagonal slivers without
    // 4^5-ing every boundary cell into a multi-second GLB.
    if depth < 2 && (mixed || hole || crosses || pts.len() > 4) {
        let mx = (x0 + x1) * 0.5;
        let my = (y0 + y1) * 0.5;
        emit_clipped_cell(
            positions, uvs, indices, x0, y0, mx, my, ox, oy, period, z, slot, flip, edges,
            verts, scale, depth + 1,
        );
        emit_clipped_cell(
            positions, uvs, indices, mx, y0, x1, my, ox, oy, period, z, slot, flip, edges,
            verts, scale, depth + 1,
        );
        emit_clipped_cell(
            positions, uvs, indices, x0, my, mx, y1, ox, oy, period, z, slot, flip, edges,
            verts, scale, depth + 1,
        );
        emit_clipped_cell(
            positions, uvs, indices, mx, my, x1, y1, ox, oy, period, z, slot, flip, edges,
            verts, scale, depth + 1,
        );
        return;
    }
    if n_in == 4 {
        emit_cell_quad(
            positions, uvs, indices, corners, ox, oy, period, z, slot, flip,
        );
        return;
    }
    if pts.len() < 3 {
        // Tiny leftover of a mixed cell: if the centre is inside, keep it.
        if in_sec(probes[0]) {
            emit_cell_quad(
                positions, uvs, indices, corners, ox, oy, period, z, slot, flip,
            );
        }
        return;
    }
    let uv_of = |p: [f32; 2]| -> [f32; 2] {
        slot_uv(
            slot,
            ((p[0] - ox) / period).clamp(0.0, 1.0),
            ((p[1] - oy) / period).clamp(0.0, 1.0),
        )
    };
    let pos_of = |p: [f32; 2]| -> [f32; 3] { [p[0], z, p[1]] };
    for [i0, i1, i2] in ear_clip(&pts) {
        let a = pts[i0];
        let b = pts[i1];
        let c = pts[i2];
        let (p0, p1, p2) = if flip { (a, c, b) } else { (a, b, c) };
        push_tri_uv(
            positions,
            uvs,
            indices,
            pos_of(p0),
            pos_of(p1),
            pos_of(p2),
            uv_of(p0),
            uv_of(p1),
            uv_of(p2),
        );
    }
}

pub(crate) fn emit_cell_quad(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    corners: [[f32; 2]; 4],
    ox: f32,
    oy: f32,
    period: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    let uv_of = |p: [f32; 2]| -> [f32; 2] {
        slot_uv(
            slot,
            ((p[0] - ox) / period).clamp(0.0, 1.0),
            ((p[1] - oy) / period).clamp(0.0, 1.0),
        )
    };
    let pos_of = |p: [f32; 2]| -> [f32; 3] { [p[0], z, p[1]] };
    let tris = if flip {
        [[0usize, 2, 1], [0, 3, 2]]
    } else {
        [[0, 1, 2], [0, 2, 3]]
    };
    for [i0, i1, i2] in tris {
        push_tri_uv(
            positions,
            uvs,
            indices,
            pos_of(corners[i0]),
            pos_of(corners[i1]),
            pos_of(corners[i2]),
            uv_of(corners[i0]),
            uv_of(corners[i1]),
            uv_of(corners[i2]),
        );
    }
}

/// Drop opposite pairs in one sector (self-referencing linedefs). Those
/// make even-odd classify the interior as outside.
pub(crate) fn drop_self_ref_pairs(edges: &mut Vec<(usize, usize)>) {
    let mut i = 0;
    while i < edges.len() {
        let (a, b) = edges[i];
        if a == b {
            edges.remove(i);
            continue;
        }
        if let Some(j) = edges.iter().skip(i + 1).position(|&(c, d)| c == b && d == a) {
            let j = j + i + 1;
            edges.remove(j);
            edges.remove(i);
            continue;
        }
        i += 1;
    }
}

pub(crate) fn loop_pts(ring: &[usize], verts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    ring.iter()
        .filter_map(|&i| verts.get(i).copied())
        .collect()
}

pub(crate) fn loop_area(ring: &[usize], verts: &[[f32; 2]]) -> f32 {
    signed_area(&loop_pts(ring, verts))
}

pub(crate) fn point_in_loop(p: [f32; 2], ring: &[usize], verts: &[[f32; 2]]) -> bool {
    let mut edges = Vec::with_capacity(ring.len());
    for i in 0..ring.len() {
        edges.push((ring[i], ring[(i + 1) % ring.len()]));
    }
    point_in_sector(p, &edges, verts)
}

pub(crate) fn orient2d(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

pub(crate) fn segs_properly_intersect(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let d1 = orient2d(a, b, c);
    let d2 = orient2d(a, b, d);
    let d3 = orient2d(c, d, a);
    let d4 = orient2d(c, d, b);
    d1 * d2 < -1e-6 && d3 * d4 < -1e-6
}

pub(crate) fn ring_edges(rings: &[Vec<usize>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for ring in rings {
        for i in 0..ring.len() {
            out.push((ring[i], ring[(i + 1) % ring.len()]));
        }
    }
    out
}

pub(crate) fn bridge_visible(
    ov: usize,
    hv: usize,
    rings: &[Vec<usize>],
    verts: &[[f32; 2]],
) -> bool {
    if ov == hv {
        return false;
    }
    let Some(a) = verts.get(ov).copied() else {
        return false;
    };
    let Some(b) = verts.get(hv).copied() else {
        return false;
    };
    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    // Midpoint must sit in the sector (outside every hole, inside an outer).
    // Caller already classified rings; even-odd on the union of all rings
    // is the sector interior.
    let all_edges = ring_edges(rings);
    if !point_in_sector(mid, &all_edges, verts) {
        return false;
    }
    for &(c, d) in &all_edges {
        if c == ov || d == ov || c == hv || d == hv {
            continue;
        }
        let Some(pc) = verts.get(c).copied() else {
            continue;
        };
        let Some(pd) = verts.get(d).copied() else {
            continue;
        };
        if segs_properly_intersect(a, b, pc, pd) {
            return false;
        }
    }
    true
}

/// Insert each hole into the outer ring via a non-crossing bridge so the
/// result is one simple (possibly degenerate-at-bridge) cycle ear-clip can
/// consume. Holes that cannot be bridged stay unfilled: their interior is
/// rejected after clip.
pub(crate) fn stitch_holes(
    mut outer: Vec<usize>,
    holes: Vec<Vec<usize>>,
    verts: &[[f32; 2]],
) -> Vec<usize> {
    let mut remaining = holes;
    while !remaining.is_empty() {
        let mut rings = vec![outer.clone()];
        rings.extend(remaining.iter().cloned());
        let mut found = None;
        'search: for (hi, hole) in remaining.iter().enumerate() {
            for &hv in hole {
                for (oi, &ov) in outer.iter().enumerate() {
                    if bridge_visible(ov, hv, &rings, verts) {
                        found = Some((hi, oi, hv));
                        break 'search;
                    }
                }
            }
        }
        let Some((hi, oi, hv)) = found else {
            break;
        };
        let mut hole = remaining.remove(hi);
        if let Some(at) = hole.iter().position(|&v| v == hv) {
            hole.rotate_left(at);
        }
        // outer CCW, hole CW: ov → hv → around hole → hv → ov
        let mut next = Vec::with_capacity(outer.len() + hole.len() + 2);
        next.extend_from_slice(&outer[..=oi]);
        next.extend_from_slice(&hole);
        next.push(hv);
        next.push(outer[oi]);
        next.extend_from_slice(&outer[oi + 1..]);
        outer = next;
    }
    outer
}

pub(crate) fn classify_sector_loops(loops: Vec<Vec<usize>>, verts: &[[f32; 2]]) -> Vec<(Vec<usize>, Vec<Vec<usize>>)> {
    if loops.is_empty() {
        return Vec::new();
    }
    let areas: Vec<f32> = loops.iter().map(|lp| loop_area(lp, verts)).collect();
    let mut parent = vec![None; loops.len()];
    for i in 0..loops.len() {
        let Some(&probe) = loops[i].first() else {
            continue;
        };
        let Some(p) = verts.get(probe).copied() else {
            continue;
        };
        let mut best: Option<(f32, usize)> = None;
        for j in 0..loops.len() {
            if i == j {
                continue;
            }
            if !point_in_loop(p, &loops[j], verts) {
                continue;
            }
            let aa = areas[j].abs();
            if best.map_or(true, |(b, _)| aa < b) {
                best = Some((aa, j));
            }
        }
        parent[i] = best.map(|(_, j)| j);
    }
    let mut groups: Vec<(Vec<usize>, Vec<Vec<usize>>)> = Vec::new();
    let mut index_of = vec![None; loops.len()];
    for i in 0..loops.len() {
        if parent[i].is_some() {
            continue;
        }
        let mut outer = loops[i].clone();
        if areas[i] < 0.0 {
            outer.reverse();
        }
        index_of[i] = Some(groups.len());
        groups.push((outer, Vec::new()));
    }
    for i in 0..loops.len() {
        let Some(p) = parent[i] else {
            continue;
        };
        let Some(gi) = index_of[p] else {
            continue;
        };
        let mut hole = loops[i].clone();
        if loop_area(&hole, verts) > 0.0 {
            hole.reverse();
        }
        groups[gi].1.push(hole);
    }
    groups
}

/// Fallback when half-plane clip collapses: unique seg endpoints, angular.
pub(crate) fn order_subsector_ring(
    segs: &[u8],
    first: usize,
    count: usize,
    n_vert: usize,
    verts: &[[f32; 2]],
) -> Vec<usize> {
    let mut ids: Vec<usize> = Vec::new();
    for k in 0..count {
        let so = (first + k) * 12;
        let v1 = u16_le(segs, so) as usize;
        let v2 = u16_le(segs, so + 2) as usize;
        if v1 < n_vert && v2 < n_vert && v1 != v2 {
            ids.push(v1);
            ids.push(v2);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    angular_convex_ring(&ids, verts)
}

pub(crate) fn angular_convex_ring(ids: &[usize], verts: &[[f32; 2]]) -> Vec<usize> {
    let mut pts: Vec<(usize, [f32; 2])> = ids
        .iter()
        .copied()
        .filter_map(|i| verts.get(i).copied().map(|p| (i, p)))
        .collect();
    if pts.len() < 3 {
        return Vec::new();
    }
    let n = pts.len() as f32;
    let cx = pts.iter().map(|p| p.1[0]).sum::<f32>() / n;
    let cy = pts.iter().map(|p| p.1[1]).sum::<f32>() / n;
    pts.sort_by(|a, b| {
        let aa = (a.1[1] - cy).atan2(a.1[0] - cx);
        let bb = (b.1[1] - cy).atan2(b.1[0] - cx);
        aa.partial_cmp(&bb).unwrap_or(std::cmp::Ordering::Equal)
    });
    pts.into_iter().map(|(i, _)| i).collect()
}

/// Directed line whose interior is the closed half-plane to its right.
#[derive(Clone, Copy)]
pub(crate) struct SplitLine {
    pub origin: [f32; 2],
    pub dir: [f32; 2],
}

pub(crate) fn split_cross(pl: SplitLine, p: [f32; 2]) -> f32 {
    pl.dir[0] * (p[1] - pl.origin[1]) - pl.dir[1] * (p[0] - pl.origin[0])
}

pub(crate) fn split_edge_hit(pl: SplitLine, a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ca = split_cross(pl, a);
    let cb = split_cross(pl, b);
    let d = cb - ca;
    if d.abs() < 1e-12 {
        return a;
    }
    let t = (-ca / d).clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

pub(crate) fn bounds_rect(minx: f32, miny: f32, maxx: f32, maxy: f32) -> Vec<[f32; 2]> {
    vec![
        [minx, miny],
        [maxx, miny],
        [maxx, maxy],
        [minx, maxy],
    ]
}

/// Keep the closed half-plane to the right of `pl`.
pub(crate) fn clip_right_of(poly: Vec<[f32; 2]>, pl: SplitLine) -> Vec<[f32; 2]> {
    const EPS: f32 = 0.05;
    if (pl.dir[0] == 0.0 && pl.dir[1] == 0.0) || poly.len() < 3 {
        return poly;
    }
    clip_poly_half(
        &poly,
        |p| split_cross(pl, p) <= EPS,
        |a, b| split_edge_hit(pl, a, b),
    )
}

/// Close a vanilla leaf by clipping the map bbox against ancestor
/// partitions and the leaf's segs. Successive Sutherland–Hodgman is
/// stabler than pairwise line hits (those drop sliver corners).
pub(crate) fn convex_from_halfplanes(start: Vec<[f32; 2]>, planes: &[SplitLine]) -> Vec<[f32; 2]> {
    let mut poly = start;
    for &pl in planes {
        if poly.len() < 3 {
            return Vec::new();
        }
        poly = clean_ring_pts(&clip_right_of(poly, pl));
    }
    if poly.len() < 3 || signed_area(&poly).abs() < 1.0 {
        return Vec::new();
    }
    poly
}

/// Last NODES entry is the root. Right child = front = right of the
/// partition (same convention as Doom `R_PointOnSide`).
pub(crate) fn collect_ssector_splits(nodes: &[u8], n_ssec: usize) -> Vec<Vec<SplitLine>> {
    let n_nodes = nodes.len() / 28;
    let mut out = vec![Vec::new(); n_ssec];
    if n_nodes == 0 || n_ssec == 0 {
        return out;
    }
    let mut visited = vec![false; n_nodes];
    let mut stack: Vec<(usize, Vec<SplitLine>)> = vec![(n_nodes - 1, Vec::new())];
    while let Some((ni, planes)) = stack.pop() {
        let o = ni * 28;
        if o + 28 > nodes.len() || std::mem::replace(&mut visited[ni], true) {
            continue;
        }
        let origin = [i16_le(nodes, o) as f32, i16_le(nodes, o + 2) as f32];
        let dir = [i16_le(nodes, o + 4) as f32, i16_le(nodes, o + 6) as f32];
        let right = i16_le(nodes, o + 24);
        let left = i16_le(nodes, o + 26);
        let split = if dir[0] != 0.0 || dir[1] != 0.0 {
            Some(SplitLine { origin, dir })
        } else {
            None
        };
        let flip = split.map(|s| SplitLine {
            origin: s.origin,
            dir: [-s.dir[0], -s.dir[1]],
        });
        for (child, extra) in [(right, split), (left, flip)] {
            let mut next = planes.clone();
            if let Some(e) = extra {
                next.push(e);
            }
            if child < 0 {
                let si = (child as u16 & 0x7FFF) as usize;
                if si < n_ssec {
                    out[si] = next;
                }
            } else {
                let ci = child as usize;
                if ci < n_nodes {
                    stack.push((ci, next));
                }
            }
        }
    }
    out
}

pub(crate) fn map_bounds(verts: &[[f32; 2]]) -> Option<(f32, f32, f32, f32)> {
    let mut minx = f32::INFINITY;
    let mut miny = f32::INFINITY;
    let mut maxx = f32::NEG_INFINITY;
    let mut maxy = f32::NEG_INFINITY;
    for p in verts {
        minx = minx.min(p[0]);
        miny = miny.min(p[1]);
        maxx = maxx.max(p[0]);
        maxy = maxy.max(p[1]);
    }
    if !minx.is_finite() {
        return None;
    }
    Some((minx - 64.0, miny - 64.0, maxx + 64.0, maxy + 64.0))
}

pub(crate) fn subsector_poly(
    segs: &[u8],
    first: usize,
    count: usize,
    n_vert: usize,
    verts: &[[f32; 2]],
    ancestors: &[SplitLine],
    bounds: (f32, f32, f32, f32),
) -> Vec<[f32; 2]> {
    let mut planes: Vec<SplitLine> = Vec::with_capacity(ancestors.len() + count);
    // Segs first so the box shrinks before the long ancestor stack.
    for k in 0..count {
        let so = (first + k) * 12;
        let v1 = u16_le(segs, so) as usize;
        let v2 = u16_le(segs, so + 2) as usize;
        if v1 >= n_vert || v2 >= n_vert || v1 == v2 {
            continue;
        }
        let a = verts[v1];
        let b = verts[v2];
        let dir = [b[0] - a[0], b[1] - a[1]];
        if dir[0] == 0.0 && dir[1] == 0.0 {
            continue;
        }
        planes.push(SplitLine { origin: a, dir });
    }
    planes.extend_from_slice(ancestors);
    let start = bounds_rect(bounds.0, bounds.1, bounds.2, bounds.3);
    let poly = convex_from_halfplanes(start, &planes);
    if poly.len() >= 3 {
        return poly;
    }
    order_subsector_ring(segs, first, count, n_vert, verts)
        .into_iter()
        .filter_map(|i| verts.get(i).copied())
        .collect()
}

pub(crate) fn chain_seg_ring(segs: &[(usize, usize)]) -> Option<Vec<usize>> {
    if segs.len() < 3 {
        return None;
    }
    for start in 0..segs.len() {
        let mut unused = segs.to_vec();
        let (a, b) = unused.remove(start);
        let mut ring = vec![a, b];
        let mut ok = true;
        while !unused.is_empty() {
            let cur = *ring.last().unwrap();
            let Some(i) = unused.iter().position(|&(s, _)| s == cur) else {
                ok = false;
                break;
            };
            let (_, nxt) = unused.remove(i);
            if nxt == ring[0] {
                if ring.len() >= 3 {
                    return Some(ring);
                }
                ok = false;
                break;
            }
            if ring.contains(&nxt) {
                ok = false;
                break;
            }
            ring.push(nxt);
        }
        if ok && ring.len() >= 3 {
            return Some(ring);
        }
    }
    None
}

pub(crate) fn convex_vert_hull(ids: &[usize], verts: &[[f32; 2]]) -> Vec<usize> {
    let mut pts: Vec<(usize, [f32; 2])> = ids
        .iter()
        .copied()
        .filter_map(|i| verts.get(i).copied().map(|p| (i, p)))
        .collect();
    if pts.len() < 3 {
        return Vec::new();
    }
    pts.sort_by(|a, b| {
        a.1[0]
            .partial_cmp(&b.1[0])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.1[1]
                    .partial_cmp(&b.1[1])
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    pts.dedup_by(|a, b| (a.1[0] - b.1[0]).abs() < 1e-4 && (a.1[1] - b.1[1]).abs() < 1e-4);
    if pts.len() < 3 {
        return Vec::new();
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower = Vec::new();
    for &(i, p) in &pts {
        while lower.len() >= 2 {
            let (_, a) = lower[lower.len() - 2];
            let (_, b) = lower[lower.len() - 1];
            if cross(a, b, p) > 1e-6 {
                break;
            }
            lower.pop();
        }
        lower.push((i, p));
    }
    let mut upper = Vec::new();
    for &(i, p) in pts.iter().rev() {
        while upper.len() >= 2 {
            let (_, a) = upper[upper.len() - 2];
            let (_, b) = upper[upper.len() - 1];
            if cross(a, b, p) > 1e-6 {
                break;
            }
            upper.pop();
        }
        upper.push((i, p));
    }
    lower.pop();
    upper.pop();
    lower
        .into_iter()
        .chain(upper)
        .map(|(i, _)| i)
        .collect()
}

/// Convex subsectors from the node builder. Each is one sector, no holes,
/// no overlapping neighbours — this is how a Doom port builds flats.
pub(crate) fn emit_subsector_flats(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    segs: &[u8],
    ssectors: &[u8],
    nodes: &[u8],
    linedefs: &[u8],
    sidedefs: &[u8],
    verts: &[[f32; 2]],
    n_vert: usize,
    n_line: usize,
    n_side: usize,
    n_sec: usize,
    sec_floor: &[i16],
    sec_ceil: &[i16],
    sec_floor_tex: &[String],
    sec_ceil_tex: &[String],
    uv_map: &BTreeMap<String, AtlasSlot>,
    scale: f32,
) -> BTreeSet<usize> {
    let mut covered = BTreeSet::new();
    if segs.len() < 12 || ssectors.len() < 4 {
        return covered;
    }
    let n_seg = segs.len() / 12;
    let n_ssec = ssectors.len() / 4;
    let leaf_planes = collect_ssector_splits(nodes, n_ssec);
    let Some(bounds) = map_bounds(verts) else {
        return covered;
    };
    for si in 0..n_ssec {
        let count = u16_le(ssectors, si * 4) as usize;
        let first = u16_le(ssectors, si * 4 + 2) as usize;
        if count < 1 || first.saturating_add(count) > n_seg {
            continue;
        }
        let mut sector = None;
        for k in 0..count {
            let so = (first + k) * 12;
            if sector.is_some() {
                break;
            }
            let line = u16_le(segs, so + 6) as usize;
            let side = u16_le(segs, so + 8);
            if line >= n_line {
                continue;
            }
            let lo = line * 14;
            let snum = if side == 0 {
                u16_le(linedefs, lo + 10)
            } else {
                u16_le(linedefs, lo + 12)
            };
            if snum == 0xFFFF || snum as usize >= n_side {
                continue;
            }
            let sec = u16_le(sidedefs, snum as usize * 30 + 28) as usize;
            if sec < n_sec {
                sector = Some(sec);
            }
        }
        let Some(sec) = sector else {
            continue;
        };
        if sec_ceil[sec] <= sec_floor[sec] {
            continue;
        }
        let ancestors = leaf_planes.get(si).map(|p| p.as_slice()).unwrap_or(&[]);
        let pts = subsector_poly(segs, first, count, n_vert, verts, ancestors, bounds);
        if pts.len() < 3 {
            continue;
        }
        let floor_tex = if sec_floor_tex[sec].is_empty() {
            "_default"
        } else {
            sec_floor_tex[sec].as_str()
        };
        let ceil_tex = if sec_ceil_tex[sec].is_empty() || is_sky_flat(&sec_ceil_tex[sec]) {
            ""
        } else {
            sec_ceil_tex[sec].as_str()
        };
        let floor_z = sec_floor[sec] as f32 * scale;
        let ceil_z = sec_ceil[sec] as f32 * scale;
        emit_convex_ring(
            positions,
            uvs,
            indices,
            &pts,
            scale,
            floor_z,
            lookup_slot(uv_map, floor_tex),
            false,
        );
        covered.insert(sec);
        if !ceil_tex.is_empty() {
            emit_convex_ring(
                positions,
                uvs,
                indices,
                &pts,
                scale,
                ceil_z,
                lookup_slot(uv_map, ceil_tex),
                true,
            );
        }
    }
    covered
}

pub(crate) fn norm2(v: [f32; 2]) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if l < 1e-8 {
        [0.0, 0.0]
    } else {
        [v[0] / l, v[1] / l]
    }
}

/// Grow a convex ring so neighbouring leaves overlap and hide T-junction
/// cracks along diagonal partitions (the leftover MAP27 ceiling hairline).
pub(crate) fn inflate_convex(pts: &[[f32; 2]], amount: f32) -> Vec<[f32; 2]> {
    let n = pts.len();
    if n < 3 || amount <= 0.0 {
        return pts.to_vec();
    }
    let mut p = pts.to_vec();
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let prev = p[(i + n - 1) % n];
        let cur = p[i];
        let next = p[(i + 1) % n];
        let e0 = [cur[0] - prev[0], cur[1] - prev[1]];
        let e1 = [next[0] - cur[0], next[1] - cur[1]];
        // CCW: outward is to the right of each edge.
        let n0 = norm2([e0[1], -e0[0]]);
        let n1 = norm2([e1[1], -e1[0]]);
        let s = [n0[0] + n1[0], n0[1] + n1[1]];
        let sl = (s[0] * s[0] + s[1] * s[1]).sqrt();
        if sl < 1e-6 {
            out.push([cur[0] + n0[0] * amount, cur[1] + n0[1] * amount]);
            continue;
        }
        let miter = (amount * 2.0 / sl).min(amount * 4.0);
        out.push([cur[0] + s[0] / sl * miter, cur[1] + s[1] / sl * miter]);
    }
    out
}

pub(crate) fn emit_convex_ring(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pts: &[[f32; 2]],
    scale: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    let pts = clean_ring_pts(pts);
    if pts.len() < 3 {
        return;
    }
    // Atlas tiles cannot GL_REPEAT. Per-vertex fract() on a tri that
    // crosses a 64-unit boundary lerps the long way and smears the flat
    // (the MAP27 ceiling). Clip each fan ear onto the world-XY tile grid.
    let area = signed_area(&pts);
    let mut ordered = pts;
    if area < 0.0 {
        ordered.reverse();
    }
    ordered = inflate_convex(&ordered, 0.75);
    let step = slot.w.max(1) as f32;
    let i0 = ordered[0];
    for k in 1..ordered.len() - 1 {
        let (a, b, c) = if flip {
            (i0, ordered[k + 1], ordered[k])
        } else {
            (i0, ordered[k], ordered[k + 1])
        };
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if cross.abs() < 1e-3 {
            continue;
        }
        emit_tri_uv_cells(
            positions, uvs, indices, a, b, c, step, scale, z, slot, flip,
        );
    }
}

/// Floors/ceilings from the sector's real linedef loops — not a world-axis
/// grid. The old grid + angular-sort invented extra coplanar slivers
/// (MAP27 origin corridor, sky-hole fans) that z-fought the real floor.
pub(crate) fn emit_sector_surface(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    edges: &[(usize, usize)],
    verts: &[[f32; 2]],
    scale: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    if edges.is_empty() {
        return;
    }
    let period = (slot.w.max(1) as f32) * scale;
    if period < 1e-4 {
        return;
    }
    // Clip the real sector polygon (linedef vertices) so floors/ceilings
    // share wall endpoints — a 64-unit grid leaves T-junction hairlines
    // against walls and dropped mixed-cell triangles. UV-split each ear
    // on the texture grid so atlas tiles do not streak.
    let loops = sector_loops(edges, verts);
    let groups = classify_sector_loops(loops, verts);
    let before = indices.len();
    for (outer, holes) in groups {
        if loop_area(&outer, verts).abs() < 1.0 {
            continue;
        }
        let ring = if holes.is_empty() {
            outer
        } else {
            stitch_holes(outer, holes, verts)
        };
        emit_ring_uv_cells(positions, uvs, indices, &ring, verts, scale, z, slot, flip);
    }
    if indices.len() == before {
        emit_clipped_grid(positions, uvs, indices, edges, verts, scale, z, slot, flip);
    }
}

pub(crate) fn emit_ring_uv_cells(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    ring: &[usize],
    verts: &[[f32; 2]],
    scale: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    let pts = clean_ring_pts(&loop_pts(ring, verts));
    if pts.len() < 3 {
        return;
    }
    let step = (slot.w.max(1) as f32).max(1.0);
    for [i0, i1, i2] in ear_clip(&pts) {
        emit_tri_uv_cells(
            positions,
            uvs,
            indices,
            pts[i0],
            pts[i1],
            pts[i2],
            step,
            scale,
            z,
            slot,
            flip,
        );
    }
}

pub(crate) fn clean_ring_pts(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for &p in pts {
        if out
            .last()
            .is_some_and(|q| (q[0] - p[0]).abs() < 1e-3 && (q[1] - p[1]).abs() < 1e-3)
        {
            continue;
        }
        out.push(p);
    }
    if out.len() >= 2 {
        let a = out[0];
        let b = *out.last().unwrap();
        if (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3 {
            out.pop();
        }
    }
    out
}

pub(crate) fn emit_tri_uv_cells(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    step: f32,
    scale: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    let minx = a[0].min(b[0]).min(c[0]);
    let maxx = a[0].max(b[0]).max(c[0]);
    let miny = a[1].min(b[1]).min(c[1]);
    let maxy = a[1].max(b[1]).max(c[1]);
    let i0 = (minx / step).floor() as i32;
    let i1 = (maxx / step).ceil() as i32;
    let j0 = (miny / step).floor() as i32;
    let j1 = (maxy / step).ceil() as i32;
    if i1 - i0 > 128 || j1 - j0 > 128 {
        return;
    }
    for i in i0..i1 {
        for j in j0..j1 {
            let x0 = i as f32 * step;
            let x1 = (i + 1) as f32 * step;
            let y0 = j as f32 * step;
            let y1 = (j + 1) as f32 * step;
            // Overlap neighbouring cells so T-junctions on the 64-grid
            // hide inside the neighbour instead of showing sky.
            const OVERLAP: f32 = 1.0;
            let clipped = clip_poly_aabb(
                &[a, b, c],
                x0 - OVERLAP,
                y0 - OVERLAP,
                x1 + OVERLAP,
                y1 + OVERLAP,
            );
            if clipped.len() < 3 {
                continue;
            }
            let uv_of = |p: [f32; 2]| {
                slot_uv(slot, (p[0] - x0) / step, (p[1] - y0) / step)
            };
            let pos_of = |p: [f32; 2]| [p[0] * scale, z, p[1] * scale];
            for k in 1..clipped.len() - 1 {
                let (p0, p1, p2) = if flip {
                    (clipped[0], clipped[k + 1], clipped[k])
                } else {
                    (clipped[0], clipped[k], clipped[k + 1])
                };
                push_tri_uv(
                    positions,
                    uvs,
                    indices,
                    pos_of(p0),
                    pos_of(p1),
                    pos_of(p2),
                    uv_of(p0),
                    uv_of(p1),
                    uv_of(p2),
                );
            }
        }
    }
}

pub(crate) fn clip_poly_aabb(poly: &[[f32; 2]], x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
    let mut cur = poly.to_vec();
    cur = clip_poly_half(&cur, |p| p[0] >= x0 - 1e-5, |a, b| intersect_axis(a, b, 0, x0));
    cur = clip_poly_half(&cur, |p| p[0] <= x1 + 1e-5, |a, b| intersect_axis(a, b, 0, x1));
    cur = clip_poly_half(&cur, |p| p[1] >= y0 - 1e-5, |a, b| intersect_axis(a, b, 1, y0));
    cur = clip_poly_half(&cur, |p| p[1] <= y1 + 1e-5, |a, b| intersect_axis(a, b, 1, y1));
    cur
}

pub(crate) fn intersect_axis(a: [f32; 2], b: [f32; 2], axis: usize, cut: f32) -> [f32; 2] {
    let d = b[axis] - a[axis];
    if d.abs() < 1e-12 {
        let mut p = a;
        p[axis] = cut;
        return p;
    }
    let t = (cut - a[axis]) / d;
    let mut p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    p[axis] = cut;
    p
}

pub(crate) fn clip_poly_half(
    poly: &[[f32; 2]],
    inside: impl Fn([f32; 2]) -> bool,
    intersect: impl Fn([f32; 2], [f32; 2]) -> [f32; 2],
) -> Vec<[f32; 2]> {
    if poly.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(poly.len() + 2);
    let n = poly.len();
    for i in 0..n {
        let s = poly[i];
        let e = poly[(i + 1) % n];
        let ein = inside(e);
        let sin = inside(s);
        if ein {
            if !sin {
                out.push(intersect(s, e));
            }
            out.push(e);
        } else if sin {
            out.push(intersect(s, e));
        }
    }
    out
}

/// Clip a convex face triangle in texture-ST space onto atlas tiles.
pub(crate) fn emit_tri_st_atlas(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    p0: [f32; 3],
    p1: [f32; 3],
    p2: [f32; 3],
    st0: [f32; 2],
    st1: [f32; 2],
    st2: [f32; 2],
    slot: AtlasSlot,
) {
    let smin = st0[0].min(st1[0]).min(st2[0]);
    let smax = st0[0].max(st1[0]).max(st2[0]);
    let tmin = st0[1].min(st1[1]).min(st2[1]);
    let tmax = st0[1].max(st1[1]).max(st2[1]);
    let i0 = smin.floor() as i32;
    let i1 = smax.ceil() as i32;
    let j0 = tmin.floor() as i32;
    let j1 = tmax.ceil() as i32;
    if i1 - i0 > 64 || j1 - j0 > 64 {
        push_tri_uv(
            positions,
            uvs,
            indices,
            p0,
            p1,
            p2,
            slot_uv(slot, st0[0].fract().abs(), st0[1].fract().abs()),
            slot_uv(slot, st1[0].fract().abs(), st1[1].fract().abs()),
            slot_uv(slot, st2[0].fract().abs(), st2[1].fract().abs()),
        );
        return;
    }
    if i1 - i0 <= 1 && j1 - j0 <= 1 {
        let ou = i0 as f32;
        let ov = j0 as f32;
        push_tri_uv(
            positions,
            uvs,
            indices,
            p0,
            p1,
            p2,
            slot_uv(slot, (st0[0] - ou).clamp(0.0, 1.0), (st0[1] - ov).clamp(0.0, 1.0)),
            slot_uv(slot, (st1[0] - ou).clamp(0.0, 1.0), (st1[1] - ov).clamp(0.0, 1.0)),
            slot_uv(slot, (st2[0] - ou).clamp(0.0, 1.0), (st2[1] - ov).clamp(0.0, 1.0)),
        );
        return;
    }
    for i in i0..i1 {
        for j in j0..j1 {
            let s0 = i as f32;
            let s1 = (i + 1) as f32;
            let t0 = j as f32;
            let t1 = (j + 1) as f32;
            let mut poly = vec![
                StVert { p: p0, s: st0[0], t: st0[1] },
                StVert { p: p1, s: st1[0], t: st1[1] },
                StVert { p: p2, s: st2[0], t: st2[1] },
            ];
            poly = clip_st_half(poly, |v| v.s >= s0 - 1e-5, |a, b| lerp_st(a, b, true, s0));
            poly = clip_st_half(poly, |v| v.s <= s1 + 1e-5, |a, b| lerp_st(a, b, true, s1));
            poly = clip_st_half(poly, |v| v.t >= t0 - 1e-5, |a, b| lerp_st(a, b, false, t0));
            poly = clip_st_half(poly, |v| v.t <= t1 + 1e-5, |a, b| lerp_st(a, b, false, t1));
            if poly.len() < 3 {
                continue;
            }
            for k in 1..poly.len() - 1 {
                push_tri_uv(
                    positions,
                    uvs,
                    indices,
                    poly[0].p,
                    poly[k].p,
                    poly[k + 1].p,
                    slot_uv(slot, (poly[0].s - s0).clamp(0.0, 1.0), (poly[0].t - t0).clamp(0.0, 1.0)),
                    slot_uv(slot, (poly[k].s - s0).clamp(0.0, 1.0), (poly[k].t - t0).clamp(0.0, 1.0)),
                    slot_uv(
                        slot,
                        (poly[k + 1].s - s0).clamp(0.0, 1.0),
                        (poly[k + 1].t - t0).clamp(0.0, 1.0),
                    ),
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct StVert {
    p: [f32; 3],
    s: f32,
    t: f32,
}

pub(crate) fn lerp_st(a: StVert, b: StVert, on_s: bool, cut: f32) -> StVert {
    let (av, bv) = if on_s { (a.s, b.s) } else { (a.t, b.t) };
    let d = bv - av;
    let t = if d.abs() < 1e-12 { 0.0 } else { (cut - av) / d };
    StVert {
        p: [
            a.p[0] + (b.p[0] - a.p[0]) * t,
            a.p[1] + (b.p[1] - a.p[1]) * t,
            a.p[2] + (b.p[2] - a.p[2]) * t,
        ],
        s: a.s + (b.s - a.s) * t,
        t: a.t + (b.t - a.t) * t,
    }
}

pub(crate) fn clip_st_half(
    poly: Vec<StVert>,
    inside: impl Fn(StVert) -> bool,
    intersect: impl Fn(StVert, StVert) -> StVert,
) -> Vec<StVert> {
    if poly.is_empty() {
        return poly;
    }
    let mut out = Vec::with_capacity(poly.len() + 2);
    let n = poly.len();
    for i in 0..n {
        let s = poly[i];
        let e = poly[(i + 1) % n];
        let ein = inside(e);
        let sin = inside(s);
        if ein {
            if !sin {
                out.push(intersect(s, e));
            }
            out.push(e);
        } else if sin {
            out.push(intersect(s, e));
        }
    }
    out
}

pub(crate) fn emit_clipped_grid(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    edges: &[(usize, usize)],
    verts: &[[f32; 2]],
    scale: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    let mut minx = f32::INFINITY;
    let mut miny = f32::INFINITY;
    let mut maxx = f32::NEG_INFINITY;
    let mut maxy = f32::NEG_INFINITY;
    for &(ia, ib) in edges {
        for i in [ia, ib] {
            let Some(v) = verts.get(i) else {
                continue;
            };
            minx = minx.min(v[0]);
            maxx = maxx.max(v[0]);
            miny = miny.min(v[1]);
            maxy = maxy.max(v[1]);
        }
    }
    if !minx.is_finite() {
        return;
    }
    let step = (slot.w.max(1) as f32).max(1.0);
    let mut i0 = (minx / step).floor() as i32;
    let mut i1 = (maxx / step).ceil() as i32;
    let mut j0 = (miny / step).floor() as i32;
    let mut j1 = (maxy / step).ceil() as i32;
    // Keep a hard cap so a map-sized bbox cannot explode. Stretch the
    // cell rather than skip the sector (skipping is how MAP24/26 lost
    // whole corridors).
    let cap = 80i32;
    if i1 - i0 > cap {
        let mid = (i0 + i1) / 2;
        i0 = mid - cap / 2;
        i1 = i0 + cap;
    }
    if j1 - j0 > cap {
        let mid = (j0 + j1) / 2;
        j0 = mid - cap / 2;
        j1 = j0 + cap;
    }
    if i1 <= i0 || j1 <= j0 {
        return;
    }
    let period = step * scale;
    for i in i0..i1 {
        for j in j0..j1 {
            let x0 = i as f32 * step * scale;
            let x1 = (i + 1) as f32 * step * scale;
            let y0 = j as f32 * step * scale;
            let y1 = (j + 1) as f32 * step * scale;
            emit_clipped_cell(
                positions,
                uvs,
                indices,
                x0,
                y0,
                x1,
                y1,
                x0,
                y0,
                period,
                z,
                slot,
                flip,
                edges,
                verts,
                scale,
                0,
            );
        }
    }
}

pub(crate) fn emit_sector_grid(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    edges: &[(usize, usize)],
    verts: &[[f32; 2]],
    scale: f32,
    z: f32,
    slot: AtlasSlot,
    flip: bool,
) {
    emit_sector_surface(positions, uvs, indices, edges, verts, scale, z, slot, flip);
}

pub(crate) fn lookup_slot<'a>(
    uv_map: &'a BTreeMap<String, AtlasSlot>,
    name: &str,
) -> AtlasSlot {
    uv_map
        .get(name)
        .or_else(|| uv_map.get("_default"))
        .copied()
        .unwrap_or(AtlasSlot {
            uv: [0.0, 0.0, 0.02, 0.02],
            w: 64,
            h: 64,
        })
}

/// Doom patch column format → RGBA (index 0 transparent).
pub(crate) fn doom_patch_to_rgba(data: &[u8], palette: &[[u8; 3]; 256]) -> Result<RgbaImage, String> {
    if data.len() < 8 {
        return Err("patch too small".into());
    }
    let w = u16_le(data, 0) as usize;
    let h = u16_le(data, 2) as usize;
    if w == 0 || h == 0 || w > 2048 || h > 2048 {
        return Err("patch dims invalid".into());
    }
    if data.len() < 8 + w * 4 {
        return Err("patch columnofs truncated".into());
    }
    let mut rgba = vec![0u8; w * h * 4];
    for col in 0..w {
        let mut colofs = u32_le(data, 8 + col * 4) as usize;
        if colofs >= data.len() {
            continue;
        }
        loop {
            if colofs >= data.len() {
                break;
            }
            let rowstart = data[colofs];
            if rowstart == 255 {
                break;
            }
            if colofs + 2 >= data.len() {
                break;
            }
            let len = data[colofs + 1] as usize;
            // skip unused byte
            let pix_off = colofs + 3;
            if pix_off + len > data.len() {
                break;
            }
            for i in 0..len {
                let row = rowstart as usize + i;
                if row >= h {
                    break;
                }
                let idx = data[pix_off + i];
                let o = (row * w + col) * 4;
                // Transparency is encoded by post gaps; every pixel inside
                // a post is opaque (index 0 is a real near-black color).
                let c = palette[idx as usize];
                rgba[o] = c[0];
                rgba[o + 1] = c[1];
                rgba[o + 2] = c[2];
                rgba[o + 3] = 255;
            }
            // post: rowstart, len, unused, pixels[len], unused
            colofs = pix_off + len + 1;
        }
    }
    Ok(RgbaImage {
        w: w as u32,
        h: h as u32,
        rgba,
    })
}

pub fn doom_patch_to_png(data: &[u8], palette: &[[u8; 3]; 256]) -> Result<Vec<u8>, String> {
    let img = doom_patch_to_rgba(data, palette)?;
    encode_png_rgba(&img.rgba, img.w, img.h)
}

pub(crate) fn lump_named<'a>(lumps: &'a [WadLump], name: &str) -> Option<&'a [u8]> {
    lumps.iter().find(|l| l.name == name).map(|l| l.data.as_slice())
}

/// Compose TEXTURE1/TEXTURE2 wall textures from PNAMES + patches.
pub(crate) fn compose_doom_textures(
    lumps: &[WadLump],
    palette: &[[u8; 3]; 256],
) -> Vec<(String, RgbaImage)> {
    let Some(pnames) = lump_named(lumps, "PNAMES") else {
        return Vec::new();
    };
    if pnames.len() < 4 {
        return Vec::new();
    }
    let n_pnames = u32_le(pnames, 0) as usize;
    if n_pnames == 0 || n_pnames > 4096 || 4 + n_pnames * 8 > pnames.len() {
        return Vec::new();
    }
    let mut patch_names = Vec::with_capacity(n_pnames);
    for i in 0..n_pnames {
        patch_names.push(lump_name(&pnames[4 + i * 8..4 + i * 8 + 8]));
    }
    let mut patches: BTreeMap<String, RgbaImage> = BTreeMap::new();
    let mut out = Vec::new();
    for lump_name_s in ["TEXTURE1", "TEXTURE2"] {
        let Some(tex) = lump_named(lumps, lump_name_s) else {
            continue;
        };
        if tex.len() < 4 {
            continue;
        }
        let ntex = i32_le(tex, 0) as usize;
        if ntex == 0 || ntex > 4096 || 4 + ntex * 4 > tex.len() {
            continue;
        }
        for i in 0..ntex {
            let off = i32_le(tex, 4 + i * 4);
            if off < 0 || off as usize + 22 > tex.len() {
                continue;
            }
            let off = off as usize;
            let name = lump_name(&tex[off..off + 8]);
            if name.is_empty() {
                continue;
            }
            let w = i16_le(tex, off + 12) as i32;
            let h = i16_le(tex, off + 14) as i32;
            let n_patches = i16_le(tex, off + 20) as usize;
            if w <= 0 || h <= 0 || w > 1024 || h > 1024 || n_patches == 0 {
                continue;
            }
            let mut canvas = vec![0u8; (w * h * 4) as usize];
            for p in 0..n_patches {
                let po = off + 22 + p * 10;
                if po + 10 > tex.len() {
                    break;
                }
                let ox = i16_le(tex, po) as i32;
                let oy = i16_le(tex, po + 2) as i32;
                let pi = i16_le(tex, po + 4) as usize;
                let Some(pname) = patch_names.get(pi) else {
                    continue;
                };
                if !patches.contains_key(pname) {
                    if let Some(lump) = lumps.iter().find(|l| l.name == *pname) {
                        if let Ok(img) = doom_patch_to_rgba(&lump.data, palette) {
                            patches.insert(pname.clone(), img);
                        }
                    }
                }
                let Some(img) = patches.get(pname) else {
                    continue;
                };
                for row in 0..img.h as i32 {
                    let dy = oy + row;
                    if dy < 0 || dy >= h {
                        continue;
                    }
                    for col in 0..img.w as i32 {
                        let dx = ox + col;
                        if dx < 0 || dx >= w {
                            continue;
                        }
                        let si = ((row as u32 * img.w + col as u32) * 4) as usize;
                        if si + 3 >= img.rgba.len() {
                            continue;
                        }
                        if img.rgba[si + 3] < 8 {
                            continue;
                        }
                        let di = ((dy as u32 * w as u32 + dx as u32) * 4) as usize;
                        canvas[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
                    }
                }
            }
            out.push((
                name,
                RgbaImage {
                    w: w as u32,
                    h: h as u32,
                    rgba: canvas,
                },
            ));
        }
    }
    out
}
