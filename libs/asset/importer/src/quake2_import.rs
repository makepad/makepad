//! Quake II shareware / demo classic importer.
//!
//! Reads official demo/shareware data (and later a retail install) in the
//! original formats after classic_import expands PAK (`PACK` magic):
//! BSP version 38 (`IBSP` + i32 38), WAL textures, MD2 models, PCX / WAV.
//! Catalog payloads only: World GLB, Character/Weapon/Prop GLB, Texture PNG,
//! Audio WAV. Never ships `.pak` / `.bsp` / `.md2` as catalog types.
//!
//! Official foothold (do **not** download from this module):
//! - Demo installer: `q2-314-demo-x86.exe` (~39 MB), SHA1
//!   `5B4DEDC59CEEE306956A3E48A8BDF6DD33BC91ED`
//!   commonly at http://tastyspleen.net/quake/downloads/q2-314-demo-x86.exe
//! - Extract `Install/Data/baseq2/pak0.pak` (7-Zip can open the exe)
//! - Point-release extras: `q2-3.20-x86-full-ctf.exe` (NOT the retail pak0)
//! - Retail later: same parser on purchased `baseq2/pak0.pak` + `pak1.pak` +
//!   `pak2.pak`
//!
//! There is no Freedoom-equivalent for Q2. Shareware is the foothold for
//! commercial packs.

use crate::classic_import::{encode_png_rgba, ClassicAsset};
use crate::vertex_skin;
use makepad_asset_data::AssetKind;
use makepad_gltf::{write_glb_mesh_textured, GlbTexturedMesh};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const QUAKE2_SOURCE_ID: &str = "quake2";
pub const QUAKE2_SOURCE_TITLE: &str = "Quake II (shareware)";
pub const QUAKE2_LICENSE: &str = "id-Software-shareware";
pub const QUAKE2_HOME: &str = "https://www.idsoftware.com/";
pub const QUAKE2_CREDITS: &str = "id Software Quake II demo / shareware";

/// Quake II units → metres (engine Y-up).
pub const SCALE: f32 = 1.0 / 64.0;

const SURF_SKY: i32 = 0x4;
const SURF_WARP: i32 = 0x8;
const SURF_NODRAW: i32 = 0x80;
const SURF_HINT: i32 = 0x100;
const SURF_SKIP: i32 = 0x200;
const SURF_NODRAW_MASK: i32 = SURF_SKY | SURF_NODRAW | SURF_HINT | SURF_SKIP;

const ATLAS_GUTTER: u32 = 2;
const ATLAS_MAX: u32 = 4096;
const VIEW_HEIGHT: f32 = 22.0;
/// A spawn entity's origin sits this far above the floor (player bbox mins z).
const ORIGIN_ABOVE_FLOOR: f32 = 24.0;
/// Quake 2 `STEPSIZE`.
const STEP_HEIGHT: f32 = 18.0;

#[derive(Clone, Debug, Default)]
pub struct WalBank {
    pub tiles: BTreeMap<String, WalImage>,
}

#[derive(Clone, Debug)]
pub struct WalImage {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Load every `.wal` under `root` (after PAK expand) keyed by shader-ish name
/// (`e1u1/floor3_3` style, lowercase, no extension).
pub fn load_wal_bank(root: &Path) -> WalBank {
    let mut bank = WalBank::default();
    let mut stack = vec![root.to_path_buf()];
    let mut n = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            n += 1;
            if n > 32_768 {
                return bank;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .eq_ignore_ascii_case("wal")
            {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Some((name, img)) = decode_wal(&bytes) else {
                continue;
            };
            insert_wal(&mut bank, &wal_key(&name), img.clone());
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_s = rel.to_string_lossy().replace('\\', "/");
                let keyed = wal_key(&rel_s);
                let key = strip_textures_prefix(&keyed);
                insert_wal(&mut bank, key, img);
            }
        }
    }
    bank
}

fn insert_wal(bank: &mut WalBank, key: &str, img: WalImage) {
    if key.is_empty() {
        return;
    }
    bank.tiles.entry(key.to_string()).or_insert(img);
}

pub fn convert_bsp38(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    tex_lookup: &WalBank,
    source_id: &str,
) -> Result<Vec<ClassicAsset>, String> {
    if bytes.len() < 8 || &bytes[0..4] != b"IBSP" {
        return Err("not IBSP".into());
    }
    let ver = i32_le(bytes, 4);
    if ver != 38 {
        return Err(format!("unsupported IBSP version {ver}"));
    }
    let (glb, spawn, liquid) = bsp38_to_glb(bytes, tex_lookup)?;
    write_world(rel, staged, source_id, glb, spawn, liquid)
}

pub fn convert_md2(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<ClassicAsset, String> {
    convert_md2_inner(bytes, rel, staged, source_id, None)
}

pub fn convert_md2_file(
    path: &Path,
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<ClassicAsset, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    convert_md2_inner(&bytes, rel, staged, source_id, path.parent())
}

pub fn convert_wal(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<Option<ClassicAsset>, String> {
    convert_wal_bytes(bytes, rel, staged, source_id)
}

pub fn convert_wal_file(
    path: &Path,
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<Option<ClassicAsset>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    convert_wal_bytes(&bytes, rel, staged, source_id)
}

fn convert_wal_bytes(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
) -> Result<Option<ClassicAsset>, String> {
    let Some((name, img)) = decode_wal(bytes) else {
        return Ok(None);
    };
    if img.w == 0 || img.h == 0 {
        return Ok(None);
    }
    let png = encode_png_rgba(&img.rgba, img.w, img.h)?;
    let slug = if name.is_empty() {
        stem_slug(rel)
    } else {
        sanitize(&name)
    };
    let key = format!("textures/{source_id}/{slug}");
    let rel_path = format!("{key}.png");
    write_bytes(staged, &rel_path, &png)?;
    Ok(Some(ClassicAsset {
        key,
        kind: AssetKind::Texture,
        rel_path,
        tags: vec!["texture".into(), source_id.into(), "wal".into()],
        icon_rel: None,
    }))
}

fn write_world(
    rel: &str,
    staged: &Path,
    source_id: &str,
    glb: Vec<u8>,
    spawn: Option<[f32; 5]>,
    liquid: bool,
) -> Result<Vec<ClassicAsset>, String> {
    let slug = stem_slug(rel);
    let key = format!("worlds/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, glb).map_err(|e| e.to_string())?;
    if let Some(s) = spawn {
        // Quake 2 player: origin 24 units above the floor, eye +22 from
        // there, STEPSIZE 18 — all at this converter's 1/64 map scale.
        let eye = (VIEW_HEIGHT + ORIGIN_ABOVE_FLOOR) * SCALE;
        let nav = crate::world_nav::WorldNav::single([s[0], s[1], s[2]], s[3], s[4])
            .with_heights(s[1] - eye, eye, STEP_HEIGHT * SCALE);
        let _ = std::fs::write(dest.with_extension("spawn"), nav.to_text());
    }
    let icon_rel = crate::world_preview::write_spawn_preview(&dest)
        .ok()
        .map(|_| format!("{key}.png"));
    let mut tags = vec![
        "world".into(),
        source_id.into(),
        "map".into(),
        "bsp38".into(),
        "no-portals".into(),
    ];
    if liquid {
        tags.push("double-sided".into());
    }
    Ok(vec![ClassicAsset {
        key,
        kind: AssetKind::World,
        rel_path,
        tags,
        icon_rel,
    }])
}

fn write_bytes(staged: &Path, rel: &str, data: &[u8]) -> Result<(), String> {
    let dest = staged.join(rel);
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(dest, data).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// BSP 38
// ---------------------------------------------------------------------------

fn bsp38_to_glb(
    bytes: &[u8],
    bank: &WalBank,
) -> Result<(Vec<u8>, Option<[f32; 5]>, bool), String> {
    if bytes.len() < 8 + 19 * 8 {
        return Err("BSP38 too small".into());
    }
    let lump = |i: usize| -> (usize, usize) {
        let o = 8 + i * 8;
        (i32_le(bytes, o) as usize, i32_le(bytes, o + 4) as usize)
    };
    let (voff, vlen) = lump(2);
    let (fioff, filen) = lump(5);
    let (foff, flen) = lump(6);
    let (loff, llen) = lump(7);
    let (eoff, elen) = lump(11);
    let (seoff, selen) = lump(12);
    if voff.saturating_add(vlen) > bytes.len()
        || fioff.saturating_add(filen) > bytes.len()
        || foff.saturating_add(flen) > bytes.len()
        || eoff.saturating_add(elen) > bytes.len()
        || seoff.saturating_add(selen) > bytes.len()
    {
        return Err("BSP38 lump truncated".into());
    }
    let lighting = if loff.saturating_add(llen) <= bytes.len() {
        &bytes[loff..loff + llen]
    } else {
        &[]
    };
    let n_verts = (vlen / 12).min(65_536);
    let n_edges = (elen / 4).min(128_000);
    let n_se = (selen / 4).min(256_000);
    let n_faces = (flen / 20).min(65_536);
    let n_tex = (filen / 76).min(8192);

    let mut verts = Vec::with_capacity(n_verts);
    for i in 0..n_verts {
        let o = voff + i * 12;
        verts.push([f32_le(bytes, o), f32_le(bytes, o + 4), f32_le(bytes, o + 8)]);
    }
    let mut edges = Vec::with_capacity(n_edges);
    for i in 0..n_edges {
        let o = eoff + i * 4;
        edges.push((u16_le(bytes, o) as usize, u16_le(bytes, o + 2) as usize));
    }
    let mut surfedges = Vec::with_capacity(n_se);
    for i in 0..n_se {
        surfedges.push(i32_le(bytes, seoff + i * 4));
    }

    let mut images: BTreeMap<String, WalImage> = BTreeMap::new();
    images.insert("_default".into(), gray_tile(16));
    let mut texinfos: Vec<TexInfo> = Vec::with_capacity(n_tex);
    for i in 0..n_tex {
        let o = fioff + i * 76;
        let mut svec = [0.0f32; 4];
        let mut tvec = [0.0f32; 4];
        for k in 0..4 {
            svec[k] = f32_le(bytes, o + k * 4);
            tvec[k] = f32_le(bytes, o + 16 + k * 4);
        }
        let flags = i32_le(bytes, o + 32);
        let name = cstr(&bytes[o + 40..o + 72]);
        let key = wal_key(&name);
        if let Some(img) = lookup_wal(bank, &key) {
            images.entry(key.clone()).or_insert_with(|| img.clone());
        }
        texinfos.push(TexInfo {
            svec,
            tvec,
            flags,
            name: key,
        });
    }
    let (atlas, uv_map) = pack_atlas(&images);

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut colors = Vec::new();
    let mut indices = Vec::new();
    let mut liquid_planes: BTreeSet<(u16, String)> = BTreeSet::new();
    let mut any_liquid = false;
    let mut any_lit = false;

    for fi in 0..n_faces {
        let o = foff + fi * 20;
        let planenum = u16_le(bytes, o);
        let side = i16_le(bytes, o + 2);
        let first = i32_le(bytes, o + 4) as usize;
        let num = i16_le(bytes, o + 8) as usize;
        let ti = i16_le(bytes, o + 10);
        let styles = [
            bytes.get(o + 12).copied().unwrap_or(255),
            bytes.get(o + 13).copied().unwrap_or(255),
            bytes.get(o + 14).copied().unwrap_or(255),
            bytes.get(o + 15).copied().unwrap_or(255),
        ];
        let lightofs = i32_le(bytes, o + 16);
        if num < 3 || first + num > n_se {
            continue;
        }
        let tex = if ti >= 0 {
            texinfos.get(ti as usize)
        } else {
            None
        };
        let (tname, flags, svec, tvec) = match tex {
            Some(t) => (t.name.as_str(), t.flags, t.svec, t.tvec),
            None => ("_default", 0, [1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]),
        };
        if flags & SURF_NODRAW_MASK != 0 || skip_tex_name(tname) {
            continue;
        }
        let liquid = tname.starts_with('*') || flags & SURF_WARP != 0;
        if liquid {
            any_liquid = true;
            if side != 0 || !liquid_planes.insert((planenum, tname.to_string())) {
                continue;
            }
        }
        let slot = lookup_slot(&uv_map, tname);
        let tw = slot.w.max(1) as f32;
        let th = slot.h.max(1) as f32;

        let mut face_verts: Vec<[f32; 3]> = Vec::with_capacity(num);
        let mut face_st: Vec<[f32; 2]> = Vec::with_capacity(num);
        let mut face_raw: Vec<[f32; 2]> = Vec::with_capacity(num);
        for k in 0..num {
            let se = surfedges[first + k];
            let (a, _b) = if se >= 0 {
                edges.get(se as usize).copied().unwrap_or((0, 0))
            } else {
                let e = edges.get((-se) as usize).copied().unwrap_or((0, 0));
                (e.1, e.0)
            };
            let Some(p) = verts.get(a).copied() else {
                continue;
            };
            face_verts.push([p[0] * SCALE, p[2] * SCALE, -p[1] * SCALE]);
            let u = p[0] * svec[0] + p[1] * svec[1] + p[2] * svec[2] + svec[3];
            let v = p[0] * tvec[0] + p[1] * tvec[1] + p[2] * tvec[2] + tvec[3];
            face_st.push([u / tw, v / th]);
            face_raw.push([u, v]);
        }
        if face_verts.len() < 3 {
            continue;
        }
        let lm = face_lightmap(lighting, lightofs, styles, &face_raw);
        if lightofs >= 0 && !lighting.is_empty() {
            any_lit = true;
        }
        for i in 1..face_verts.len() - 1 {
            let start = positions.len();
            emit_tri_st_atlas(
                &mut positions,
                &mut uvs,
                &mut indices,
                face_verts[0],
                face_verts[i],
                face_verts[i + 1],
                face_st[0],
                face_st[i],
                face_st[i + 1],
                slot,
            );
            // Atlas clip may emit extra verts; assign this source tri's
            // average shipped light so COLOR_0 stays 1:1 with positions.
            let a = lm[0];
            let b = lm[i];
            let c = lm[i + 1];
            let avg = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            for _ in start..positions.len() {
                colors.push(avg);
            }
        }
    }

    if indices.is_empty() {
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
        base_color_png: &atlas,
        metallic_roughness_png: None,
        double_sided: any_liquid,
        colors: if any_lit && colors.len() == positions.len() {
            Some(&colors)
        } else {
            None
        },
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB encode failed".into());
    }
    Ok((glb, entity_spawn(bytes, lump(0)), any_liquid))
}

struct TexInfo {
    svec: [f32; 4],
    tvec: [f32; 4],
    flags: i32,
    name: String,
}

fn skip_tex_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("clip")
        || n.starts_with("trigger")
        || n == "skip"
        || n == "hint"
        || n.starts_with("sky")
        || n == "aaatrigger"
}

fn lookup_wal<'a>(bank: &'a WalBank, key: &str) -> Option<&'a WalImage> {
    bank.tiles
        .get(key)
        .or_else(|| bank.tiles.get(&strip_textures_prefix(key).to_string()))
        .or_else(|| {
            let tail = key.rsplit('/').next().unwrap_or(key);
            bank.tiles.get(tail)
        })
}

fn entity_spawn(bytes: &[u8], lump: (usize, usize)) -> Option<[f32; 5]> {
    let (off, len) = lump;
    if len == 0 || off.saturating_add(len) > bytes.len() {
        return None;
    }
    let text = std::str::from_utf8(&bytes[off..off + len]).ok()?;
    let mut best: Option<(i32, [f32; 5])> = None;
    let flush = |class: &str, origin: Option<[f32; 3]>, angle: f32| -> Option<(i32, [f32; 5])> {
        let o = origin?;
        let rank = match class {
            "info_player_start" => 0,
            "info_player_coop" => 1,
            "info_player_deathmatch" => 2,
            _ => return None,
        };
        let pos = [
            o[0] * SCALE,
            o[2] * SCALE + VIEW_HEIGHT * SCALE,
            -o[1] * SCALE,
        ];
        let yaw = std::f32::consts::FRAC_PI_2 - angle.to_radians();
        Some((rank, [pos[0], pos[1], pos[2], yaw, 0.0]))
    };
    for raw in text.split(|c| c == '{' || c == '}') {
        let block = raw.trim();
        if block.is_empty() {
            continue;
        }
        let mut class = String::new();
        let mut origin = None;
        let mut angle = 0.0f32;
        for line in block.lines() {
            let kv: Vec<&str> = line.split('"').filter(|s| !s.trim().is_empty()).collect();
            if kv.len() < 2 {
                continue;
            }
            match kv[0] {
                "classname" => class = kv[1].to_string(),
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
                    angle = kv[1].parse().unwrap_or(0.0);
                }
                _ => {}
            }
        }
        if let Some((rank, spawn)) = flush(&class, origin, angle) {
            match best {
                Some((br, _)) if br <= rank => {}
                _ => best = Some((rank, spawn)),
            }
            if rank == 0 {
                return Some(spawn);
            }
        }
    }
    best.map(|(_, s)| s)
}

// ---------------------------------------------------------------------------
// WAL
// ---------------------------------------------------------------------------

fn decode_wal(bytes: &[u8]) -> Option<(String, WalImage)> {
    if bytes.len() < 100 {
        return None;
    }
    let name = cstr(&bytes[0..32]);
    let w = u32_le(bytes, 32) as usize;
    let h = u32_le(bytes, 36) as usize;
    let mut off0 = u32_le(bytes, 40) as usize;
    if off0 == 0 {
        off0 = 100;
    }
    if w == 0 || h == 0 || w > 1024 || h > 1024 || off0 + w * h > bytes.len() {
        return None;
    }
    let pal = q2_palette();
    let fence = name.starts_with('{');
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &idx in &bytes[off0..off0 + w * h] {
        if fence && idx == 255 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let c = pal[idx as usize];
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    Some((
        name,
        WalImage {
            w: w as u32,
            h: h as u32,
            rgba,
        },
    ))
}

fn wal_key(name: &str) -> String {
    let mut s = name.replace('\\', "/").to_ascii_lowercase();
    if let Some(stripped) = s.strip_suffix(".wal") {
        s = stripped.to_string();
    }
    if let Some(stripped) = s.strip_suffix(".pcx") {
        s = stripped.to_string();
    }
    s.trim_start_matches('/').to_string()
}

fn strip_textures_prefix(key: &str) -> &str {
    key.strip_prefix("textures/").unwrap_or(key)
}

fn gray_tile(size: u32) -> WalImage {
    WalImage {
        w: size,
        h: size,
        rgba: vec![0x90; (size * size * 4) as usize],
    }
}

// ---------------------------------------------------------------------------
// MD2
// ---------------------------------------------------------------------------

fn convert_md2_inner(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    source_id: &str,
    source_dir: Option<&Path>,
) -> Result<ClassicAsset, String> {
    if bytes.len() < 68 || &bytes[0..4] != b"IDP2" {
        return Err("not MD2 (IDP2)".into());
    }
    let version = i32_le(bytes, 4);
    if version != 8 {
        return Err(format!("unsupported MD2 version {version}"));
    }
    let skinwidth = i32_le(bytes, 8).max(1) as usize;
    let skinheight = i32_le(bytes, 12).max(1) as usize;
    let framesize = i32_le(bytes, 16) as usize;
    let num_skins = i32_le(bytes, 20) as usize;
    let num_xyz = i32_le(bytes, 24) as usize;
    let num_st = i32_le(bytes, 28) as usize;
    let num_tris = i32_le(bytes, 32) as usize;
    let num_glcmds = i32_le(bytes, 36) as usize;
    let num_frames = i32_le(bytes, 40) as usize;
    let ofs_skins = i32_le(bytes, 44) as usize;
    let ofs_st = i32_le(bytes, 48) as usize;
    let ofs_tris = i32_le(bytes, 52) as usize;
    let ofs_frames = i32_le(bytes, 56) as usize;
    let ofs_glcmds = i32_le(bytes, 60) as usize;
    if num_xyz == 0 || num_tris == 0 || num_frames == 0 || framesize < 40 {
        return Err("empty MD2".into());
    }
    if num_xyz > 50_000 || num_tris > 50_000 || num_frames > 512 {
        return Err("MD2 mesh bounds invalid".into());
    }

    let mut st = vec![[0.0f32; 2]; num_st.max(1)];
    let sw = skinwidth.max(1) as f32;
    let sh = skinheight.max(1) as f32;
    for i in 0..num_st {
        let o = ofs_st + i * 4;
        if o + 4 > bytes.len() {
            break;
        }
        st[i] = [i16_le(bytes, o) as f32 / sw, i16_le(bytes, o + 2) as f32 / sh];
    }

    // Quake II draws `glcmds` (float UVs). The triangle ST table is often
    // wrong on official meshes — using it paints keys/guns with garbage.
    let (corners, uvs, indices) = md2_mesh(
        bytes,
        num_xyz,
        num_st,
        num_tris,
        num_glcmds,
        ofs_st,
        ofs_tris,
        ofs_glcmds,
        &st,
    );
    if indices.len() < 3 {
        return Err("MD2 produced no tris".into());
    }

    let mut frames: Vec<Md2Frame> = Vec::new();
    for fi in 0..num_frames {
        let fo = ofs_frames + fi * framesize;
        if fo + 40 + num_xyz * 4 > bytes.len() {
            break;
        }
        let scale = [f32_le(bytes, fo), f32_le(bytes, fo + 4), f32_le(bytes, fo + 8)];
        let translate = [
            f32_le(bytes, fo + 12),
            f32_le(bytes, fo + 16),
            f32_le(bytes, fo + 20),
        ];
        let name = cstr(&bytes[fo + 24..fo + 40]);
        let mut packed = Vec::with_capacity(num_xyz);
        for v in 0..num_xyz {
            let vo = fo + 40 + v * 4;
            packed.push([bytes[vo], bytes[vo + 1], bytes[vo + 2]]);
        }
        frames.push(Md2Frame {
            name,
            scale,
            translate,
            packed,
        });
    }
    if frames.is_empty() {
        return Err("MD2 frames missing".into());
    }

    let unpack = |frame: &Md2Frame, vi: usize| -> [f32; 3] {
        let p = frame.packed.get(vi).copied().unwrap_or([0, 0, 0]);
        let x = p[0] as f32 * frame.scale[0] + frame.translate[0];
        let y = p[1] as f32 * frame.scale[1] + frame.translate[1];
        let z = p[2] as f32 * frame.scale[2] + frame.translate[2];
        q2_face_camera([x * SCALE, z * SCALE, -y * SCALE])
    };
    let pose = |frame: &Md2Frame| -> Vec<[f32; 3]> {
        corners.iter().map(|&vi| unpack(frame, vi)).collect()
    };

    let (skin_rgba, skin_w, skin_h) = resolve_md2_skin(
        bytes,
        ofs_skins,
        num_skins,
        skinwidth,
        skinheight,
        source_dir,
        rel,
    );
    let skin_png = encode_png_rgba(&skin_rgba, skin_w, skin_h)?;
    let slug = md2_slug(rel);
    let kind = md2_kind(rel, &slug);
    let rest_pos = pose(&frames[0]);
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &rest_pos,
        normals: None,
        uvs: &uvs,
        indices: &indices,
        base_color_png: &skin_png,
        metallic_roughness_png: None,
        // Thin pickups (CD, keys) disappear from the 3/4 icon if culled.
        double_sided: kind != AssetKind::Character,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("MD2 GLB write failed".into());
    }

    let names: Vec<String> = frames.iter().map(|f| f.name.clone()).collect();
    let loop_idx = crate::anim_icon::pick_loop_indices(&names);
    // Characters: front (+180°) so the face is the card. Items: 3/4 + a
    // slight look-down — face-on a disc/head is a pie / gem.
    let (yaw, pitch) = if kind == AssetKind::Character {
        (std::f32::consts::PI + 0.35, 0.0)
    } else {
        (std::f32::consts::PI + 1.15, 0.62)
    };
    let mut tiles = Vec::new();
    let pick = if loop_idx.is_empty() { vec![0] } else { loop_idx };
    for &fi in &pick {
        let Some(frame) = frames.get(fi) else {
            continue;
        };
        tiles.push(crate::anim_icon::raster_mesh_icon_orient(
            &pose(frame),
            &indices,
            &uvs,
            &skin_rgba,
            skin_w as usize,
            skin_h as usize,
            yaw,
            pitch,
            crate::anim_icon::TILE,
            [26, 31, 41, 255],
        ));
    }
    let icon_png = if tiles.len() >= 2 {
        crate::anim_icon::pack_sheet(&tiles).ok().map(|sheet| sheet.png)
    } else if let Some(tile) = tiles.first() {
        encode_png_rgba(tile, crate::anim_icon::TILE as u32, crate::anim_icon::TILE as u32).ok()
    } else {
        None
    };
    let folder = match kind {
        AssetKind::Character => "characters",
        AssetKind::Weapon => "weapons",
        _ => "props",
    };
    let mut glb = glb;
    let mut clip_names: Vec<String> = Vec::new();
    if kind == AssetKind::Character && frames.len() > 1 {
        let names: Vec<String> = frames.iter().map(|f| f.name.clone()).collect();
        let clips = vertex_skin::alias_loco_clips(vertex_skin::group_named_clips(&names));
        let unique_rest: Vec<[f32; 3]> = (0..num_xyz).map(|vi| unpack(&frames[0], vi)).collect();
        let unique_frames: Vec<Vec<[f32; 3]>> = frames
            .iter()
            .map(|frame| (0..num_xyz).map(|vi| unpack(frame, vi)).collect())
            .collect();
        if let Ok(skinned) = vertex_skin::write_skinned_from_vertex_frames(
            &unique_rest,
            &unique_frames,
            &corners,
            &uvs,
            &indices,
            &clips,
            &skin_png,
        ) {
            glb = skinned;
            clip_names = clips.into_iter().map(|c| c.name).collect();
        }
    }
    let key = format!("{folder}/{source_id}/{slug}");
    let rel_path = format!("{key}.glb");
    write_bytes(staged, &rel_path, &glb)?;
    let mut icon_rel = None;
    if let Some(png) = icon_png {
        let icon_path = format!("{key}.png");
        if write_bytes(staged, &icon_path, &png).is_ok() {
            icon_rel = Some(icon_path);
        }
    }
    let mut tags = vec![
        kind_tag(kind).into(),
        source_id.into(),
        "md2".into(),
        slug.clone(),
    ];
    if frames.len() > 1 {
        tags.push("vertex-anim".into());
        tags.push(format!("frames-{}", frames.len()));
    }
    if !clip_names.is_empty() {
        tags.push("skinned".into());
        for name in clip_names {
            tags.push(format!("clip-{name}"));
        }
    }
    Ok(ClassicAsset {
        key,
        kind,
        rel_path,
        tags,
        icon_rel,
    })
}

struct Md2Frame {
    name: String,
    scale: [f32; 3],
    translate: [f32; 3],
    packed: Vec<[u8; 3]>,
}

/// Quake II +X is model forward. Studio / MeshView look along −Z, so yaw −90°.
fn q2_face_camera(p: [f32; 3]) -> [f32; 3] {
    let yaw = -std::f32::consts::FRAC_PI_2;
    let (s, c) = (yaw.sin(), yaw.cos());
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}

/// Prefer `glcmds` (float S/T + xyz index). Official triangle ST is often a
/// leftover and paints the same xyz with one UV — seams and guns smear.
fn md2_mesh(
    bytes: &[u8],
    num_xyz: usize,
    _num_st: usize,
    num_tris: usize,
    num_glcmds: usize,
    _ofs_st: usize,
    ofs_tris: usize,
    ofs_glcmds: usize,
    st: &[[f32; 2]],
) -> (Vec<usize>, Vec<[f32; 2]>, Vec<u32>) {
    if let Some(mesh) = md2_mesh_glcmds(bytes, num_xyz, num_glcmds, ofs_glcmds) {
        if mesh.2.len() >= 3 {
            return mesh;
        }
    }
    md2_mesh_tris(bytes, num_xyz, num_tris, ofs_tris, st)
}

fn md2_mesh_glcmds(
    bytes: &[u8],
    num_xyz: usize,
    num_glcmds: usize,
    ofs_glcmds: usize,
) -> Option<(Vec<usize>, Vec<[f32; 2]>, Vec<u32>)> {
    if num_glcmds < 4 || ofs_glcmds + 4 > bytes.len() {
        return None;
    }
    let end = (ofs_glcmds + num_glcmds.saturating_mul(4)).min(bytes.len());
    let mut corners = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut weld: BTreeMap<(usize, i32, i32), u32> = BTreeMap::new();
    let mut off = ofs_glcmds;
    let mut prims = 0usize;
    while off + 4 <= end {
        let count = i32_le(bytes, off);
        off += 4;
        if count == 0 {
            break;
        }
        let n = count.unsigned_abs() as usize;
        if n < 3 || n > 4096 || off + n * 12 > end {
            return None;
        }
        let strip = count > 0;
        let mut prim: Vec<u32> = Vec::with_capacity(n);
        for _ in 0..n {
            let s = f32_le(bytes, off);
            let t = f32_le(bytes, off + 4);
            let idx = i32_le(bytes, off + 8);
            off += 12;
            if idx < 0 || idx as usize >= num_xyz {
                return None;
            }
            let xyz = idx as usize;
            let key = (xyz, (s * 1024.0).round() as i32, (t * 1024.0).round() as i32);
            let out = if let Some(&existing) = weld.get(&key) {
                existing
            } else {
                let id = corners.len() as u32;
                weld.insert(key, id);
                corners.push(xyz);
                uvs.push([s, t]);
                id
            };
            prim.push(out);
        }
        if strip {
            for i in 0..prim.len().saturating_sub(2) {
                let (a, b, c) = if i % 2 == 0 {
                    (prim[i], prim[i + 1], prim[i + 2])
                } else {
                    (prim[i + 1], prim[i], prim[i + 2])
                };
                if a != b && b != c && a != c {
                    indices.extend_from_slice(&[a, b, c]);
                }
            }
        } else {
            for i in 1..prim.len().saturating_sub(1) {
                let (a, b, c) = (prim[0], prim[i], prim[i + 1]);
                if a != b && b != c && a != c {
                    indices.extend_from_slice(&[a, b, c]);
                }
            }
        }
        prims += 1;
        if prims > 16_384 {
            return None;
        }
    }
    if indices.len() < 3 {
        return None;
    }
    Some((corners, uvs, indices))
}

fn md2_mesh_tris(
    bytes: &[u8],
    num_xyz: usize,
    num_tris: usize,
    ofs_tris: usize,
    st: &[[f32; 2]],
) -> (Vec<usize>, Vec<[f32; 2]>, Vec<u32>) {
    let mut corners = Vec::with_capacity(num_tris * 3);
    let mut uvs = Vec::with_capacity(num_tris * 3);
    let mut indices = Vec::with_capacity(num_tris * 3);
    for i in 0..num_tris {
        let o = ofs_tris + i * 12;
        if o + 12 > bytes.len() {
            break;
        }
        let xyz = [
            u16_le(bytes, o) as usize,
            u16_le(bytes, o + 2) as usize,
            u16_le(bytes, o + 4) as usize,
        ];
        let sti = [
            u16_le(bytes, o + 6) as usize,
            u16_le(bytes, o + 8) as usize,
            u16_le(bytes, o + 10) as usize,
        ];
        if xyz.iter().any(|&v| v >= num_xyz) {
            continue;
        }
        let base = corners.len() as u32;
        for k in 0..3 {
            corners.push(xyz[k]);
            uvs.push(st.get(sti[k]).copied().unwrap_or([0.0, 0.0]));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    (corners, uvs, indices)
}

fn md2_kind(rel: &str, slug: &str) -> AssetKind {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    if lower.contains("/players/")
        || lower.starts_with("players/")
        || lower.contains("/monsters/")
        || lower.contains("/deadbods/")
    {
        return AssetKind::Character;
    }
    if lower.contains("/weapons/")
        || lower.contains("weapons/")
        || slug.starts_with("w-")
        || slug.starts_with("v-")
        || slug.starts_with("g-")
        || slug.contains("-v-")
        || slug.contains("-g-")
    {
        return AssetKind::Weapon;
    }
    AssetKind::Prop
}

/// Quake II stores almost every mesh as `tris.md2`. The folder is the identity:
/// `models/monsters/soldier/tris.md2` → `monsters-soldier`.
fn md2_slug(rel: &str) -> String {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('/').filter(|s| !s.is_empty()).collect();
    while matches!(
        parts.first().copied(),
        Some("pak" | "_pak" | "baseq2" | "install" | "data")
    ) {
        parts.remove(0);
    }
    if parts.first() == Some(&"models") {
        parts.remove(0);
    }
    if let Some(last) = parts.last().copied() {
        if last == "tris.md2" || last.starts_with("tris.") {
            parts.pop();
        } else if let Some(stem) = last.strip_suffix(".md2") {
            parts.pop();
            return sanitize(&format!("{}-{stem}", parts.join("-")));
        }
    }
    let joined = parts.join("-");
    let slug = sanitize(&joined);
    if slug.is_empty() {
        "model".into()
    } else {
        slug
    }
}

fn resolve_md2_skin(
    bytes: &[u8],
    ofs_skins: usize,
    num_skins: usize,
    skin_w: usize,
    skin_h: usize,
    source_dir: Option<&Path>,
    rel: &str,
) -> (Vec<u8>, u32, u32) {
    let mut names = Vec::new();
    for i in 0..num_skins.min(32) {
        let o = ofs_skins + i * 64;
        if o + 64 > bytes.len() {
            break;
        }
        let n = cstr(&bytes[o..o + 64]);
        if !n.is_empty() {
            names.push(n);
        }
    }
    if let Some(dir) = source_dir {
        for name in &names {
            if let Some(img) = load_skin_file(dir, name) {
                return (img.rgba, img.w, img.h);
            }
        }
        if let Some(parent) = dir.parent() {
            for name in &names {
                if let Some(img) = load_skin_file(parent, name) {
                    return (img.rgba, img.w, img.h);
                }
            }
        }
        // Same-folder PCX/WAL next to the MD2.
        if let Some(img) = load_sibling_skin(dir, rel) {
            return (img.rgba, img.w, img.h);
        }
    }
    let w = skin_w.max(1).min(256) as u32;
    let h = skin_h.max(1).min(256) as u32;
    (vec![0xB4; (w * h * 4) as usize], w, h)
}

fn load_sibling_skin(dir: &Path, rel: &str) -> Option<WalImage> {
    let stem = Path::new(rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut names = vec![
        format!("{stem}.pcx"),
        format!("{stem}.wal"),
        "skin.pcx".into(),
        "weapon.pcx".into(),
        "skin.wal".into(),
    ];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "pcx" || ext == "wal" {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    names.push(name.to_string());
                }
            }
        }
    }
    for name in names {
        let p = dir.join(&name);
        if let Some(img) = load_image_file(&p) {
            return Some(img);
        }
    }
    None
}

fn load_skin_file(dir: &Path, name: &str) -> Option<WalImage> {
    let cleaned = name.replace('\\', "/");
    let file = cleaned.rsplit('/').next().unwrap_or(&cleaned);
    let candidates = [
        dir.join(&cleaned),
        dir.join(file),
        dir.join("..").join(&cleaned),
    ];
    for p in candidates {
        if let Some(img) = load_image_file(&p) {
            return Some(img);
        }
        let stem = p.with_extension("");
        for ext in ["pcx", "wal", "PCX", "WAL"] {
            if let Some(img) = load_image_file(&stem.with_extension(ext)) {
                return Some(img);
            }
        }
    }
    None
}

fn load_image_file(path: &Path) -> Option<WalImage> {
    let bytes = std::fs::read(path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "wal" => decode_wal(&bytes).map(|(_, img)| img),
        "pcx" => decode_pcx(&bytes),
        _ => decode_pcx(&bytes).or_else(|| decode_wal(&bytes).map(|(_, img)| img)),
    }
}

fn decode_pcx(bytes: &[u8]) -> Option<WalImage> {
    if bytes.len() < 128 || bytes[0] != 0x0A {
        return None;
    }
    let encoding = bytes[2];
    let bpp = bytes[3];
    let xmin = u16_le(bytes, 4) as i32;
    let ymin = u16_le(bytes, 6) as i32;
    let xmax = u16_le(bytes, 8) as i32;
    let ymax = u16_le(bytes, 10) as i32;
    let planes = bytes[65] as usize;
    let bpl = u16_le(bytes, 66) as usize;
    let w = (xmax - xmin + 1) as usize;
    let h = (ymax - ymin + 1) as usize;
    if w == 0 || h == 0 || w > 1024 || h > 1024 || bpp != 8 || planes != 1 {
        return None;
    }
    let mut indices = vec![0u8; w * h];
    let mut src = 128usize;
    for row in 0..h {
        let mut x = 0usize;
        while x < bpl {
            if src >= bytes.len() {
                return None;
            }
            let b = bytes[src];
            src += 1;
            let (val, count) = if encoding == 1 && b & 0xC0 == 0xC0 {
                if src >= bytes.len() {
                    return None;
                }
                let v = bytes[src];
                src += 1;
                (v, (b & 0x3F) as usize)
            } else {
                (b, 1)
            };
            for _ in 0..count {
                if x < w {
                    indices[row * w + x] = val;
                }
                x += 1;
            }
        }
    }
    let pal = if bytes.len() >= 769 && bytes[bytes.len() - 769] == 0x0C {
        let mut p = [[0u8; 3]; 256];
        let raw = &bytes[bytes.len() - 768..];
        for i in 0..256 {
            p[i] = [raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]];
        }
        p
    } else {
        q2_palette()
    };
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &idx in &indices {
        let c = pal[idx as usize];
        rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    Some(WalImage {
        w: w as u32,
        h: h as u32,
        rgba,
    })
}

// ---------------------------------------------------------------------------
// Atlas
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct AtlasSlot {
    uv: [f32; 4],
    w: u32,
    h: u32,
}

fn lookup_slot(uv_map: &BTreeMap<String, AtlasSlot>, name: &str) -> AtlasSlot {
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

fn slot_uv(slot: AtlasSlot, local_u: f32, local_v: f32) -> [f32; 2] {
    let slop_u = 2.0 / slot.w.max(1) as f32;
    let slop_v = 2.0 / slot.h.max(1) as f32;
    let local_u = local_u.clamp(-slop_u, 1.0 + slop_u);
    let local_v = local_v.clamp(-slop_v, 1.0 + slop_v);
    let du = slot.uv[2] - slot.uv[0];
    let dv = slot.uv[3] - slot.uv[1];
    [slot.uv[0] + du * local_u, slot.uv[1] + dv * local_v]
}

fn pack_atlas(images: &BTreeMap<String, WalImage>) -> (Vec<u8>, BTreeMap<String, AtlasSlot>) {
    let mut entries: Vec<(&String, &WalImage)> = images.iter().collect();
    entries.sort_by(|a, b| b.1.w.cmp(&a.1.w).then(b.1.h.cmp(&a.1.h)));
    let mut atlas_w = 64u32;
    let mut atlas_h = 64u32;
    let mut placed: BTreeMap<String, (u32, u32, u32, u32)> = BTreeMap::new();
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_h = 0u32;
    let pack_one = |iw: u32, ih: u32| -> (u32, u32) {
        (iw.max(1) + 2 * ATLAS_GUTTER, ih.max(1) + 2 * ATLAS_GUTTER)
    };
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
            if atlas_w < ATLAS_MAX {
                atlas_w = (atlas_w * 2).min(ATLAS_MAX).max(x + w);
            }
            if x + w > atlas_w {
                x = 0;
                y += row_h;
                row_h = 0;
            }
        }
        if y + h > atlas_h {
            atlas_h = (atlas_h * 2).min(ATLAS_MAX).max(y + h);
        }
        if y + h > ATLAS_MAX || x + w > ATLAS_MAX {
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
            blit_wrapped(&mut rgba, atlas_w, *px, *py, *packed_w, *packed_h, img);
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
    if !uv_map.contains_key("_default") {
        let slot = uv_map.values().next().copied().unwrap_or(AtlasSlot {
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

fn blit_wrapped(
    rgba: &mut [u8],
    atlas_w: u32,
    px: u32,
    py: u32,
    packed_w: u32,
    packed_h: u32,
    img: &WalImage,
) {
    let iw = img.w.max(1);
    let ih = img.h.max(1);
    for row in 0..packed_h {
        for col in 0..packed_w {
            let sx = (col as i32 - ATLAS_GUTTER as i32).rem_euclid(iw as i32) as u32;
            let sy = (row as i32 - ATLAS_GUTTER as i32).rem_euclid(ih as i32) as u32;
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

fn emit_tri_st_atlas(
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
    if i1 - i0 > 64 || j1 - j0 > 64 || (i1 - i0 <= 1 && j1 - j0 <= 1) {
        let ou = i0 as f32;
        let ov = j0 as f32;
        push_tri(
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
                StVert {
                    p: p0,
                    s: st0[0],
                    t: st0[1],
                },
                StVert {
                    p: p1,
                    s: st1[0],
                    t: st1[1],
                },
                StVert {
                    p: p2,
                    s: st2[0],
                    t: st2[1],
                },
            ];
            poly = clip_st_half(poly, |v| v.s >= s0 - 1e-5, |a, b| lerp_st(a, b, true, s0));
            poly = clip_st_half(poly, |v| v.s <= s1 + 1e-5, |a, b| lerp_st(a, b, true, s1));
            poly = clip_st_half(poly, |v| v.t >= t0 - 1e-5, |a, b| lerp_st(a, b, false, t0));
            poly = clip_st_half(poly, |v| v.t <= t1 + 1e-5, |a, b| lerp_st(a, b, false, t1));
            if poly.len() < 3 {
                continue;
            }
            for k in 1..poly.len() - 1 {
                push_tri(
                    positions,
                    uvs,
                    indices,
                    poly[0].p,
                    poly[k].p,
                    poly[k + 1].p,
                    slot_uv(
                        slot,
                        (poly[0].s - s0).clamp(0.0, 1.0),
                        (poly[0].t - t0).clamp(0.0, 1.0),
                    ),
                    slot_uv(
                        slot,
                        (poly[k].s - s0).clamp(0.0, 1.0),
                        (poly[k].t - t0).clamp(0.0, 1.0),
                    ),
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
struct StVert {
    p: [f32; 3],
    s: f32,
    t: f32,
}

fn lerp_st(a: StVert, b: StVert, on_s: bool, cut: f32) -> StVert {
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

fn clip_st_half(
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

fn push_tri(
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
    positions.extend_from_slice(&[a, b, c]);
    uvs.extend_from_slice(&[ua, ub, uc]);
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}

/// Sample the shipped Quake II face lightmap at each ring vertex.
/// Style 0 is the static layer; 255 means unused. `lightofs < 0` is fullbright.
fn face_lightmap(
    lighting: &[u8],
    lightofs: i32,
    styles: [u8; 4],
    raw_st: &[[f32; 2]],
) -> Vec<[f32; 3]> {
    let white = vec![[1.0f32, 1.0, 1.0]; raw_st.len()];
    if lighting.is_empty() || lightofs < 0 || styles[0] == 255 || raw_st.is_empty() {
        return white;
    }
    let mut min_s = f32::INFINITY;
    let mut max_s = f32::NEG_INFINITY;
    let mut min_t = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    for st in raw_st {
        min_s = min_s.min(st[0]);
        max_s = max_s.max(st[0]);
        min_t = min_t.min(st[1]);
        max_t = max_t.max(st[1]);
    }
    let texmin_s = min_s.floor();
    let texmin_t = min_t.floor();
    let extent_s = (max_s.ceil() - texmin_s).max(0.0) as i32;
    let extent_t = (max_t.ceil() - texmin_t).max(0.0) as i32;
    let smax = ((extent_s >> 4) + 1).max(1) as usize;
    let tmax = ((extent_t >> 4) + 1).max(1) as usize;
    let block = smax.saturating_mul(tmax).saturating_mul(3);
    let start = lightofs as usize;
    if block == 0 || start.saturating_add(block) > lighting.len() {
        return white;
    }
    let sample = |s: f32, t: f32| -> [f32; 3] {
        let u = ((s - texmin_s) / 16.0).clamp(0.0, (smax.saturating_sub(1)) as f32);
        let v = ((t - texmin_t) / 16.0).clamp(0.0, (tmax.saturating_sub(1)) as f32);
        let x0 = u.floor() as usize;
        let y0 = v.floor() as usize;
        let x1 = (x0 + 1).min(smax - 1);
        let y1 = (y0 + 1).min(tmax - 1);
        let fu = u - x0 as f32;
        let fv = v - y0 as f32;
        let pix = |x: usize, y: usize| -> [f32; 3] {
            let o = start + (y * smax + x) * 3;
            [
                lighting[o] as f32 / 255.0,
                lighting[o + 1] as f32 / 255.0,
                lighting[o + 2] as f32 / 255.0,
            ]
        };
        let a = pix(x0, y0);
        let b = pix(x1, y0);
        let c = pix(x0, y1);
        let d = pix(x1, y1);
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * fu;
            let bot = c[i] + (d[i] - c[i]) * fu;
            // Quake II overbright: 2x, then clamp.
            out[i] = (top + (bot - top) * fv) * 2.0;
            if out[i] > 1.0 {
                out[i] = 1.0;
            }
        }
        out
    };
    raw_st.iter().map(|st| sample(st[0], st[1])).collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn kind_tag(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::World => "world",
        _ => "mesh",
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

fn stem_slug(rel: &str) -> String {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    sanitize(stem)
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "asset".into()
    } else {
        out
    }
}

fn u16_le(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        0
    } else {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
}
fn i16_le(b: &[u8], o: usize) -> i16 {
    u16_le(b, o) as i16
}
fn u32_le(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        0
    } else {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
}
fn i32_le(b: &[u8], o: usize) -> i32 {
    u32_le(b, o) as i32
}
fn f32_le(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_le(b, o))
}

/// Official Quake II colormap.pcx palette (8-bit RGB).
fn q2_palette() -> [[u8; 3]; 256] {
    const P: [[u8; 3]; 256] = [
        [0, 0, 0], [15, 15, 15], [31, 31, 31], [47, 47, 47], [63, 63, 63], [75, 75, 75],
        [91, 91, 91], [107, 107, 107], [123, 123, 123], [139, 139, 139], [155, 155, 155], [171, 171, 171],
        [187, 187, 187], [203, 203, 203], [219, 219, 219], [235, 235, 235], [99, 75, 35], [91, 67, 31],
        [83, 63, 31], [79, 59, 27], [71, 55, 27], [63, 47, 23], [59, 43, 23], [51, 39, 19],
        [47, 35, 19], [43, 31, 19], [39, 27, 15], [35, 23, 15], [27, 19, 11], [23, 15, 11],
        [19, 15, 7], [15, 11, 7], [95, 95, 111], [91, 91, 103], [91, 83, 95], [87, 79, 91],
        [83, 75, 83], [79, 71, 75], [71, 63, 67], [63, 59, 59], [59, 55, 55], [51, 47, 47],
        [47, 43, 43], [39, 39, 39], [35, 35, 35], [27, 27, 27], [23, 23, 23], [19, 19, 19],
        [143, 119, 83], [123, 99, 67], [115, 91, 59], [103, 79, 47], [207, 151, 75], [167, 123, 59],
        [139, 103, 47], [111, 83, 39], [235, 159, 39], [203, 139, 35], [175, 119, 31], [147, 99, 27],
        [119, 79, 23], [91, 59, 15], [63, 39, 11], [35, 23, 7], [167, 59, 43], [159, 47, 35],
        [151, 43, 27], [139, 39, 19], [127, 31, 15], [115, 23, 11], [103, 23, 7], [87, 19, 0],
        [75, 15, 0], [67, 15, 0], [59, 15, 0], [51, 11, 0], [43, 11, 0], [35, 11, 0],
        [27, 7, 0], [19, 7, 0], [123, 95, 75], [115, 87, 67], [107, 83, 63], [103, 79, 59],
        [95, 71, 55], [87, 67, 51], [83, 63, 47], [75, 55, 43], [67, 51, 39], [63, 47, 35],
        [55, 39, 27], [47, 35, 23], [39, 27, 19], [31, 23, 15], [23, 15, 11], [15, 11, 7],
        [111, 59, 23], [95, 55, 23], [83, 47, 23], [67, 43, 23], [55, 35, 19], [39, 27, 15],
        [27, 19, 11], [15, 11, 7], [179, 91, 79], [191, 123, 111], [203, 155, 147], [215, 187, 183],
        [203, 215, 223], [179, 199, 211], [159, 183, 195], [135, 167, 183], [115, 151, 167], [91, 135, 155],
        [71, 119, 139], [47, 103, 127], [23, 83, 111], [19, 75, 103], [15, 67, 91], [11, 63, 83],
        [7, 55, 75], [7, 47, 63], [7, 39, 51], [0, 31, 43], [0, 23, 31], [0, 15, 19],
        [0, 7, 11], [0, 0, 0], [139, 87, 87], [131, 79, 79], [123, 71, 71], [115, 67, 67],
        [107, 59, 59], [99, 51, 51], [91, 47, 47], [87, 43, 43], [75, 35, 35], [63, 31, 31],
        [51, 27, 27], [43, 19, 19], [31, 15, 15], [19, 11, 11], [11, 7, 7], [0, 0, 0],
        [151, 159, 123], [143, 151, 115], [135, 139, 107], [127, 131, 99], [119, 123, 95], [115, 115, 87],
        [107, 107, 79], [99, 99, 71], [91, 91, 67], [79, 79, 59], [67, 67, 51], [55, 55, 43],
        [47, 47, 35], [35, 35, 27], [23, 23, 19], [15, 15, 11], [159, 75, 63], [147, 67, 55],
        [139, 59, 47], [127, 55, 39], [119, 47, 35], [107, 43, 27], [99, 35, 23], [87, 31, 19],
        [79, 27, 15], [67, 23, 11], [55, 19, 11], [43, 15, 7], [31, 11, 7], [23, 7, 0],
        [11, 0, 0], [0, 0, 0], [119, 123, 207], [111, 115, 195], [103, 107, 183], [99, 99, 167],
        [91, 91, 155], [83, 87, 143], [75, 79, 127], [71, 71, 115], [63, 63, 103], [55, 55, 87],
        [47, 47, 75], [39, 39, 63], [35, 31, 47], [27, 23, 35], [19, 15, 23], [11, 7, 7],
        [155, 171, 123], [143, 159, 111], [135, 151, 99], [123, 139, 87], [115, 131, 75], [103, 119, 67],
        [95, 111, 59], [87, 103, 51], [75, 91, 39], [63, 79, 27], [55, 67, 19], [47, 59, 11],
        [35, 47, 7], [27, 35, 0], [19, 23, 0], [11, 15, 0], [0, 255, 0], [35, 231, 15],
        [63, 211, 27], [83, 187, 39], [95, 167, 47], [95, 143, 51], [95, 123, 51], [255, 255, 255],
        [255, 255, 211], [255, 255, 167], [255, 255, 127], [255, 255, 83], [255, 255, 39], [255, 235, 31],
        [255, 215, 23], [255, 191, 15], [255, 171, 7], [255, 147, 0], [239, 127, 0], [227, 107, 0],
        [211, 87, 0], [199, 71, 0], [183, 59, 0], [171, 43, 0], [155, 31, 0], [143, 23, 0],
        [127, 15, 0], [115, 7, 0], [95, 0, 0], [71, 0, 0], [47, 0, 0], [27, 0, 0],
        [239, 0, 0], [55, 55, 255], [255, 0, 0], [0, 0, 255], [43, 43, 35], [27, 27, 23],
        [19, 19, 15], [235, 151, 127], [195, 115, 83], [159, 87, 51], [123, 63, 27], [235, 211, 199],
        [199, 171, 155], [167, 139, 119], [135, 107, 87], [159, 91, 83],
    ];
    P
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mp_q2_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    fn make_wal(name: &str, w: u32, h: u32, fill: u8) -> Vec<u8> {
        let mut b = vec![0u8; 100 + (w * h) as usize];
        let nb = name.as_bytes();
        b[..nb.len().min(31)].copy_from_slice(&nb[..nb.len().min(31)]);
        b[32..36].copy_from_slice(&w.to_le_bytes());
        b[36..40].copy_from_slice(&h.to_le_bytes());
        b[40..44].copy_from_slice(&100u32.to_le_bytes());
        for p in &mut b[100..] {
            *p = fill;
        }
        b
    }

    fn push_i32(buf: &mut Vec<u8>, v: i32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn push_i16(buf: &mut Vec<u8>, v: i16) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn push_u16(buf: &mut Vec<u8>, v: u16) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn push_f32(buf: &mut Vec<u8>, v: f32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn make_bsp38_quad(tex_name: &str) -> Vec<u8> {
        let ents = format!(
            "{{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 16\"\n\"angle\" \"90\"\n}}\n"
        );
        let ents_b = ents.into_bytes();

        let mut verts = Vec::new();
        for (x, y, z) in [(0.0f32, 0.0, 0.0), (64.0, 0.0, 0.0), (64.0, 64.0, 0.0), (0.0, 64.0, 0.0)]
        {
            push_f32(&mut verts, x);
            push_f32(&mut verts, y);
            push_f32(&mut verts, z);
        }

        let mut edges = Vec::new();
        for (a, b) in [(0u16, 1u16), (1, 2), (2, 3), (3, 0)] {
            push_u16(&mut edges, a);
            push_u16(&mut edges, b);
        }

        let mut se = Vec::new();
        for i in 0i32..4 {
            push_i32(&mut se, i);
        }

        let mut tex = Vec::new();
        // s vecs
        push_f32(&mut tex, 1.0);
        push_f32(&mut tex, 0.0);
        push_f32(&mut tex, 0.0);
        push_f32(&mut tex, 0.0);
        // t vecs
        push_f32(&mut tex, 0.0);
        push_f32(&mut tex, 1.0);
        push_f32(&mut tex, 0.0);
        push_f32(&mut tex, 0.0);
        push_i32(&mut tex, 0); // flags
        push_i32(&mut tex, 0); // value
        let mut name = [0u8; 32];
        let nb = tex_name.as_bytes();
        name[..nb.len().min(31)].copy_from_slice(&nb[..nb.len().min(31)]);
        tex.extend_from_slice(&name);
        push_i32(&mut tex, -1);
        assert_eq!(tex.len(), 76);

        let mut face = Vec::new();
        push_u16(&mut face, 0); // planenum
        push_i16(&mut face, 0); // side
        push_i32(&mut face, 0); // firstedge
        push_i16(&mut face, 4); // numedges
        push_i16(&mut face, 0); // texinfo
        face.extend_from_slice(&[0u8, 255, 255, 255]);
        push_i32(&mut face, -1);
        assert_eq!(face.len(), 20);

        const HEADER: usize = 8 + 19 * 8;
        let mut lumps = [(0i32, 0i32); 19];
        let mut off = HEADER as i32;
        lumps[0] = (off, ents_b.len() as i32);
        off += ents_b.len() as i32;
        lumps[2] = (off, verts.len() as i32);
        off += verts.len() as i32;
        lumps[11] = (off, edges.len() as i32);
        off += edges.len() as i32;
        lumps[12] = (off, se.len() as i32);
        off += se.len() as i32;
        lumps[5] = (off, tex.len() as i32);
        off += tex.len() as i32;
        lumps[6] = (off, face.len() as i32);

        let mut out = Vec::new();
        out.extend_from_slice(b"IBSP");
        push_i32(&mut out, 38);
        for (o, l) in lumps {
            push_i32(&mut out, o);
            push_i32(&mut out, l);
        }
        assert_eq!(out.len(), HEADER);
        out.extend_from_slice(&ents_b);
        out.extend_from_slice(&verts);
        out.extend_from_slice(&edges);
        out.extend_from_slice(&se);
        out.extend_from_slice(&tex);
        out.extend_from_slice(&face);
        out
    }

    #[test]
    fn rejects_non_ibsp_and_wrong_version() {
        let staged = tmp_dir("rej");
        let bank = WalBank::default();
        assert!(convert_bsp38(b"XXXX", "x.bsp", &staged, &bank, "quake2").is_err());
        assert!(convert_bsp38(&[0; 16], "x.bsp", &staged, &bank, "quake2").is_err());
        let mut v29 = b"IBSP".to_vec();
        v29.extend_from_slice(&29i32.to_le_bytes());
        v29.extend_from_slice(&[0u8; 19 * 8]);
        assert!(convert_bsp38(&v29, "x.bsp", &staged, &bank, "quake2").is_err());
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn synthetic_bsp38_quad_with_wal_writes_glb() {
        let root = tmp_dir("bsp_src");
        let staged = tmp_dir("bsp_stage");
        let tex_dir = root.join("textures/e1u1");
        std::fs::create_dir_all(&tex_dir).unwrap();
        let wal = make_wal("e1u1/floor3_3", 16, 16, 15);
        std::fs::write(tex_dir.join("floor3_3.wal"), &wal).unwrap();
        let bank = load_wal_bank(&root);
        assert!(
            bank.tiles.contains_key("e1u1/floor3_3"),
            "keys: {:?}",
            bank.tiles.keys().collect::<Vec<_>>()
        );

        let bsp = make_bsp38_quad("e1u1/floor3_3");
        let assets = convert_bsp38(&bsp, "maps/base1.bsp", &staged, &bank, QUAKE2_SOURCE_ID)
            .expect("convert bsp38");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].kind, AssetKind::World);
        assert!(!assets[0].rel_path.ends_with(".bsp"));
        let glb = std::fs::read(staged.join(&assets[0].rel_path)).unwrap();
        assert!(glb.starts_with(b"glTF"), "must be GLB");
        assert!(
            glb.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"),
            "GLB must embed atlas PNG"
        );
        let spawn = std::fs::read_to_string(staged.join("worlds/base1.spawn")).unwrap();
        assert!(spawn.starts_with("world-spawn 1"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn wal_decode_dimensions() {
        let bytes = make_wal("e1u1/floor3_3", 32, 16, 8);
        let (name, img) = decode_wal(&bytes).expect("wal");
        assert_eq!(name, "e1u1/floor3_3");
        assert_eq!(img.w, 32);
        assert_eq!(img.h, 16);
        assert_eq!(img.rgba.len(), 32 * 16 * 4);
        let staged = tmp_dir("wal");
        let asset = convert_wal(&bytes, "textures/e1u1/floor3_3.wal", &staged, QUAKE2_SOURCE_ID)
            .unwrap()
            .expect("texture asset");
        assert_eq!(asset.kind, AssetKind::Texture);
        assert!(asset.rel_path.ends_with(".png"));
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn md2_folder_slug_keeps_models_apart() {
        assert_eq!(
            md2_slug("models/monsters/soldier/tris.md2"),
            "monsters-soldier"
        );
        assert_eq!(md2_slug("models/monsters/tank/tris.md2"), "monsters-tank");
        assert_eq!(
            md2_slug("models/items/ammo/bullets/medium/tris.md2"),
            "items-ammo-bullets-medium"
        );
        assert_eq!(
            md2_kind("models/monsters/soldier/tris.md2", "monsters-soldier"),
            AssetKind::Character
        );
        assert_eq!(
            md2_kind("models/weapons/v_shotg/tris.md2", "weapons-v-shotg"),
            AssetKind::Weapon
        );
        assert_eq!(
            md2_kind("models/items/armor/body/tris.md2", "items-armor-body"),
            AssetKind::Prop
        );
    }

    #[test]
    fn md2_rejects_garbage() {
        let staged = tmp_dir("md2");
        assert!(convert_md2(b"garbage", "models/x.md2", &staged, QUAKE2_SOURCE_ID).is_err());
        let mut bad_ver = b"IDP2".to_vec();
        bad_ver.extend_from_slice(&7i32.to_le_bytes());
        bad_ver.extend_from_slice(&[0u8; 60]);
        assert!(convert_md2(&bad_ver, "models/x.md2", &staged, QUAKE2_SOURCE_ID).is_err());
        let mut empty = b"IDP2".to_vec();
        empty.extend_from_slice(&8i32.to_le_bytes());
        empty.extend_from_slice(&[0u8; 60]);
        assert!(convert_md2(&empty, "models/x.md2", &staged, QUAKE2_SOURCE_ID).is_err());
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn md2_glcmds_win_over_wrong_triangle_st() {
        // One triangle. ST table is all zeros (the official-MD2 footgun).
        // glcmds carry the real float UVs.
        let mut bytes = b"IDP2".to_vec();
        let push_i32 = |b: &mut Vec<u8>, v: i32| b.extend_from_slice(&v.to_le_bytes());
        let push_f32 = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
        // skinw, skinh, framesize, nskins, nxyz, nst, ntris, ngl, nframes
        // ofs_skins, ofs_st, ofs_tris, ofs_frames, ofs_glcmds, ofs_end
        let skinw = 64i32;
        let skinh = 64i32;
        let nxyz = 3i32;
        let nst = 3i32;
        let ntris = 1i32;
        let nframes = 1i32;
        let framesize = 40 + nxyz * 4;
        // placeholder header; fill offsets after we know them
        bytes.extend_from_slice(&[0u8; 64]);
        let ofs_skins = bytes.len();
        bytes.extend_from_slice(&[0u8; 64]);
        let ofs_st = bytes.len();
        // all-zero ST — if the converter used these, every UV would be 0
        bytes.extend_from_slice(&[0u8; 12]);
        let ofs_tris = bytes.len();
        // xyz 0,1,2 / st 0,0,0
        for v in [0u16, 1, 2, 0, 0, 0] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let ofs_frames = bytes.len();
        for _ in 0..3 {
            push_f32(&mut bytes, 1.0); // scale
        }
        for _ in 0..3 {
            push_f32(&mut bytes, 0.0); // translate
        }
        bytes.extend_from_slice(b"stand1\0\0\0\0\0\0\0\0\0\0");
        bytes.extend_from_slice(&[0u8, 0, 0, 0, 10, 0, 0, 0, 0, 10, 0, 0]); // packed xyz + unused
        let ofs_gl = bytes.len();
        // fan of 3: (s,t,idx) then end
        push_i32(&mut bytes, -3);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 0.0);
        push_i32(&mut bytes, 0);
        push_f32(&mut bytes, 1.0);
        push_f32(&mut bytes, 0.0);
        push_i32(&mut bytes, 1);
        push_f32(&mut bytes, 0.0);
        push_f32(&mut bytes, 1.0);
        push_i32(&mut bytes, 2);
        push_i32(&mut bytes, 0);
        let ofs_end = bytes.len();
        let ngl = ((ofs_end - ofs_gl) / 4) as i32;
        let hdr = [
            8i32, skinw, skinh, framesize, 1, nxyz, nst, ntris, ngl, nframes,
            ofs_skins as i32, ofs_st as i32, ofs_tris as i32, ofs_frames as i32,
            ofs_gl as i32, ofs_end as i32,
        ];
        for (i, v) in hdr.iter().enumerate() {
            bytes[4 + i * 4..8 + i * 4].copy_from_slice(&v.to_le_bytes());
        }

        let st = [[0.0f32, 0.0]; 3];
        let (_c, uvs, idx) = md2_mesh(
            &bytes,
            3,
            3,
            1,
            ngl as usize,
            ofs_st,
            ofs_tris,
            ofs_gl,
            &st,
        );
        assert_eq!(idx.len(), 3);
        assert!(
            uvs.iter().any(|uv| (uv[0] - 1.0).abs() < 1e-5),
            "glcmds UVs must survive, got {uvs:?}"
        );
        assert!(
            uvs.iter().any(|uv| (uv[1] - 1.0).abs() < 1e-5),
            "glcmds UVs must survive, got {uvs:?}"
        );

        let staged = tmp_dir("md2_gl");
        let asset = convert_md2(
            &bytes,
            "pak/models/weapons/g_shotg/tris.md2",
            &staged,
            QUAKE2_SOURCE_ID,
        )
        .expect("convert synthetic md2");
        assert_eq!(asset.key, "weapons/quake2/weapons-g_shotg");
        assert_eq!(asset.kind, AssetKind::Weapon);
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn md2_slug_strips_pak_and_models() {
        assert_eq!(
            md2_slug("pak/models/weapons/g_shotg/tris.md2"),
            "weapons-g_shotg"
        );
        assert_eq!(
            md2_slug("pak/models/monsters/berserk/tris.md2"),
            "monsters-berserk"
        );
    }

    #[test]
    fn q2_face_camera_turns_forward_to_plus_z() {
        // Quake +X forward, after [x,z,-y] then −90° yaw → +Z.
        let p = q2_face_camera([1.0, 0.0, 0.0]);
        assert!(p[2] > 0.9, "forward should face +Z, got {p:?}");
        assert!(p[0].abs() < 1e-5 && p[1].abs() < 1e-5);
    }
}
