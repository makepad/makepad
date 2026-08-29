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
use crate::glb_nodes::ExtraNode;
use crate::skybox::{cube_to_equirect_png, CubeFace, FACE_SUFFIXES};
use crate::vertex_skin;
use crate::world_nav::{deathmatch_name, player_start_name, NavDoor, NavStart, NavTeleport, WorldNav};
use makepad_asset_data::AssetKind;
use makepad_gltf::{write_glb_mesh_textured, GlbTexturedMesh};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const QUAKE2_SOURCE_ID: &str = "quake2";
pub const QUAKE2_SOURCE_TITLE: &str = "Quake II (shareware)";
pub const QUAKE2_LICENSE: &str = "id-Software-shareware";
pub const QUAKE2_HOME: &str = "https://www.idsoftware.com/";
pub const QUAKE2_CREDITS: &str = "id Software Quake II demo / shareware";

/// Quake II units → metres (engine Y-up): the 56-unit player (`mins.z −24`
/// to `maxs.z 32`) stands `PERSON_HEIGHT`, so 1/32 m — the id Tech 2 unit
/// every other id importer uses. It was 1/64 until 2026-08-26.
pub const SCALE: f32 = crate::dimensions::PERSON_HEIGHT / 56.0;

const SURF_SKY: i32 = 0x4;
const SURF_WARP: i32 = 0x8;
const SURF_NODRAW: i32 = 0x80;
const SURF_HINT: i32 = 0x100;
const SURF_SKIP: i32 = 0x200;
/// Faces the compiler marked "never draw me". `SURF_SKY` is NOT in here: a
/// sky face is drawn, direction-mapped, by the `sky` node.
const SURF_NODRAW_MASK: i32 = SURF_NODRAW | SURF_HINT | SURF_SKIP;

/// BSP38 lump indices (`qfiles.h`).
const LUMP_ENTITIES: usize = 0;
const LUMP_VERTEXES: usize = 2;
const LUMP_TEXINFO: usize = 5;
const LUMP_FACES: usize = 6;
const LUMP_LIGHTING: usize = 7;
const LUMP_EDGES: usize = 11;
const LUMP_SURFEDGES: usize = 12;
const LUMP_MODELS: usize = 13;

const ATLAS_GUTTER: u32 = 2;
const ATLAS_MAX: u32 = 4096;
const VIEW_HEIGHT: f32 = 22.0;
/// A spawn entity's origin sits this far above the floor (player bbox mins z).
const ORIGIN_ABOVE_FLOOR: f32 = 24.0;
/// Quake 2 `STEPSIZE`.
const STEP_HEIGHT: f32 = 18.0;
/// `SP_func_door` / `SP_func_plat`: `if (!st.lip) st.lip = 8;`
const MOVER_LIP: f32 = 8.0;
/// `DOOR_START_OPEN` — the brush is authored where it looks shut but the
/// entity is moved to the far pose at spawn, and the two roles swap.
const DOOR_START_OPEN: i32 = 1;
/// `SECRET_1ST_LEFT` / `SECRET_1ST_DOWN` of `func_door_secret`.
const SECRET_1ST_LEFT: i32 = 1;
const SECRET_1ST_DOWN: i32 = 2;
/// The sky set `SP_worldspawn` falls back to when `worldspawn` has no `sky`
/// key. Retail `baseq2` ships `env/unit1_*`; the shareware pak ships only
/// `env/sky1*`, which is why the SURF_SKY texture's own basename is tried
/// first — see [`resolve_sky`].
const DEFAULT_SKY: &str = "unit1_";

#[derive(Clone, Debug, Default)]
pub struct WalBank {
    /// Level textures, keyed `e1u1/floor3_3` style. Environment-box faces
    /// live here too, keyed `env/<set><suffix>` — one map so the caller's
    /// `bank.tiles.extend(other.tiles)` merge (which is all
    /// `classic_import` does) cannot silently drop the sky.
    pub tiles: BTreeMap<String, WalImage>,
}

#[derive(Clone, Debug)]
pub struct WalImage {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Load every `.wal` under `root` (after PAK expand) keyed by shader-ish name
/// (`e1u1/floor3_3` style, lowercase, no extension), plus every environment
/// box face (`env/*.pcx`, `env/*.tga`) keyed `env/<stem>`.
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
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if in_env_dir(&path) && matches!(ext.as_str(), "pcx" | "tga") {
                load_env_face(&mut bank, &path, &ext);
                continue;
            }
            if ext != "wal" {
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

/// A cube face is identified by its FOLDER, not by a path prefix: the same
/// `env/` directory turns up at the pack root, under `baseq2/`, and under the
/// PAK scratch dir, and all three must key the same way.
fn in_env_dir(path: &Path) -> bool {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("env"))
}

/// Both encodings of a face ship side by side in `baseq2` (`sky1rt.pcx` and
/// `sky1rt.tga`). PCX is the palette-exact original, so it always wins,
/// whichever the directory walk reaches first.
fn load_env_face(bank: &mut WalBank, path: &Path, ext: &str) {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    let key = format!("env/{}", stem.to_ascii_lowercase());
    if ext == "tga" && bank.tiles.contains_key(&key) {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let img = match ext {
        "pcx" => decode_pcx(&bytes),
        _ => crate::quake3_import::decode_tga(&bytes)
            .ok()
            .map(|(rgba, w, h)| WalImage { w, h, rgba }),
    };
    let Some(img) = img else { return };
    match ext {
        "pcx" => {
            bank.tiles.insert(key, img);
        }
        _ => {
            bank.tiles.entry(key).or_insert(img);
        }
    }
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
    let map = bsp38_to_glb(bytes, tex_lookup)?;
    write_world(rel, staged, source_id, map)
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
    map: Bsp38Map,
) -> Result<Vec<ClassicAsset>, String> {
    let slug = stem_slug(rel);
    let key = format!("worlds/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, map.glb).map_err(|e| e.to_string())?;
    if let Some(nav) = &map.nav {
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
    if map.liquid {
        tags.push("double-sided".into());
    }
    if map.sky_unmapped {
        // A recorded warning rather than a log line nobody reads: the map
        // HAS sky faces but the pack shipped no `env/` set for them, so the
        // ceiling is a hole exactly as it was before the sky node existed.
        tags.push("sky-unmapped".into());
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

/// A converted Quake II map: the GLB, the walker facts it publishes, and the
/// two things the World asset tags itself with.
struct Bsp38Map {
    glb: Vec<u8>,
    nav: Option<WorldNav>,
    liquid: bool,
    /// The map has `SURF_SKY` faces but the pack shipped no `env/` set for
    /// them, so no `sky` node was written.
    sky_unmapped: bool,
}

/// One vertex stream leaving the BSP: the level itself, or one of the nodes
/// that leaves it (`sky`, `hazard_N`, `door_N`, `lift_N`).
#[derive(Clone, Debug, Default)]
struct Soup {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Soup {
    fn is_empty(&self) -> bool {
        self.indices.len() < 3
    }
    /// COLOR_0 only rides along when it is 1:1 with positions — the atlas
    /// clip can split a triangle, so this is checked and not assumed.
    fn colors(&self) -> Vec<[f32; 3]> {
        if self.colors.len() == self.positions.len() {
            self.colors.clone()
        } else {
            Vec::new()
        }
    }
}

fn bsp38_to_glb(bytes: &[u8], bank: &WalBank) -> Result<Bsp38Map, String> {
    if bytes.len() < 8 + 19 * 8 {
        return Err("BSP38 too small".into());
    }
    let lump = |i: usize| -> (usize, usize) {
        let o = 8 + i * 8;
        (i32_le(bytes, o) as usize, i32_le(bytes, o + 4) as usize)
    };
    let (voff, vlen) = lump(LUMP_VERTEXES);
    let (fioff, filen) = lump(LUMP_TEXINFO);
    let (foff, flen) = lump(LUMP_FACES);
    let (loff, llen) = lump(LUMP_LIGHTING);
    let (eoff, elen) = lump(LUMP_EDGES);
    let (seoff, selen) = lump(LUMP_SURFEDGES);
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

    // What moves, where the player starts, and what a brush entity's own
    // `origin` key does to its geometry — all of it from the entity lump
    // read once, alongside the brush models of lump 13.
    let (moff, mlen) = lump(LUMP_MODELS);
    let models = brush_models(bytes, moff, mlen);
    let ent_text = entity_text(bytes, lump(LUMP_ENTITIES));
    let entities = parse_entities(&ent_text);
    let movers = q2_movers(&entities, &models);

    // `qbsp3` bakes an `origin` brush out of the geometry and hands the
    // offset back as the entity's `origin` key, so a submodel with one is
    // authored around zero. The engine translates it at draw time
    // (`R_DrawBrushModel`) and so must this: without it a rotating door or
    // fan lands at the map's centre.
    let mut offset_of_face: BTreeMap<usize, [f32; 3]> = BTreeMap::new();
    for e in &entities {
        let Some(mi) = model_index(e) else { continue };
        let Some(m) = models.get(mi) else { continue };
        let origin = ent_vec3(e, "origin").unwrap_or([0.0; 3]);
        if origin == [0.0; 3] {
            continue;
        }
        let glb = to_glb(origin);
        for f in m.first_face..m.first_face.saturating_add(m.num_faces) {
            offset_of_face.insert(f, glb);
        }
    }
    let mut mover_of_face: BTreeMap<usize, usize> = BTreeMap::new();
    for (mi, mover) in movers.iter().enumerate() {
        for f in mover.first_face..mover.first_face.saturating_add(mover.num_faces) {
            mover_of_face.insert(f, mi);
            offset_of_face.insert(f, mover.shift);
        }
    }

    let mut world = Soup::default();
    let mut sky = Soup::default();
    let mut hazards: BTreeMap<String, Soup> = BTreeMap::new();
    let mut mover_geom: Vec<Soup> = vec![Soup::default(); movers.len()];
    let mut liquid_planes: BTreeSet<(u16, String)> = BTreeSet::new();
    let mut any_liquid = false;
    let mut any_lit = false;
    let mut any_sky_face = false;
    let mut sky_texture = String::new();

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
        let sky_face = flags & SURF_SKY != 0;
        // Quake 1 spelled its liquids `*name`; Quake 2 spells them
        // `SURF_WARP`. Both are accepted so one rule covers the family.
        let liquid = !sky_face && (tname.starts_with('*') || flags & SURF_WARP != 0);
        if sky_face {
            any_sky_face = true;
            if sky_texture.is_empty() {
                sky_texture = tname.to_string();
            }
        }
        if liquid {
            any_liquid = true;
            // A liquid is two-sided in the BSP and the level material is
            // double-sided — emitting both faces z-fights every surface.
            if side != 0 || !liquid_planes.insert((planenum, tname.to_string())) {
                continue;
            }
        }
        let slot = lookup_slot(&uv_map, tname);
        let tw = slot.w.max(1) as f32;
        let th = slot.h.max(1) as f32;
        let shift = offset_of_face.get(&fi).copied().unwrap_or([0.0; 3]);

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
            let g = to_glb(p);
            face_verts.push([g[0] + shift[0], g[1] + shift[1], g[2] + shift[2]]);
            // Texture and lightmap coordinates come from the UNSHIFTED
            // vertex: `qbsp3` generated the texinfo vectors in the
            // submodel's own space, exactly as the engine samples them.
            let u = p[0] * svec[0] + p[1] * svec[1] + p[2] * svec[2] + svec[3];
            let v = p[0] * tvec[0] + p[1] * tvec[1] + p[2] * tvec[2] + tvec[3];
            face_st.push([u / tw, v / th]);
            face_raw.push([u, v]);
        }
        if face_verts.len() < 3 {
            continue;
        }
        let lm = face_lightmap(lighting, lightofs, styles, &face_raw);
        // Where this face's triangles go: the sky node, the hazard node of
        // its liquid, the door or lift that moves it, or the level mesh.
        let out = if sky_face {
            &mut sky
        } else if liquid {
            hazards.entry(tname.to_string()).or_default()
        } else if let Some(&mi) = mover_of_face.get(&fi) {
            &mut mover_geom[mi]
        } else {
            if lightofs >= 0 && !lighting.is_empty() {
                any_lit = true;
            }
            &mut world
        };
        for i in 1..face_verts.len() - 1 {
            let start = out.positions.len();
            emit_tri_st_atlas(
                &mut out.positions,
                &mut out.uvs,
                &mut out.indices,
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
            for _ in start..out.positions.len() {
                out.colors.push(avg);
            }
        }
    }

    if world.indices.is_empty() {
        world.positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        world.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        world.colors.clear();
        world.indices = vec![0, 1, 2, 0, 2, 3];
        any_lit = false;
    }
    // Weld the T-junctions this converter's OWN atlas split leaves behind.
    //
    // `qbsp3` does fix Quake II's T-junctions, which is why nothing welded
    // here for a long time — but that is about the BSP's own faces. This
    // converter then re-cuts every face wherever it crosses a texture cell,
    // because the tiles are packed into an atlas and a UV cannot wrap
    // through one; neighbouring faces do not agree where those cuts fall,
    // and the cracks come straight back. Doom, Quake 1 and Build all weld
    // after the same split, so Quake II does too — over the world, the sky,
    // the liquids and every mover, from one grid spanning all of them,
    // because the cracks that show up most are the ones BETWEEN parts.
    {
        // A pair of corners a hair apart poisons each other's cuts, so the
        // splitter declines them; merging those first is what takes the
        // count to zero rather than near it. The merge runs FIRST and the
        // weld grid is built AFTER it, because a merge removes positions
        // and a grid holding one the mesh no longer has would cut a fresh
        // T-junction of its own.
        let merge = {
            let mut parts: Vec<&[[f32; 3]]> = vec![&world.positions[..], &sky.positions[..]];
            parts.extend(hazards.values().map(|s| &s.positions[..]));
            parts.extend(mover_geom.iter().map(|s| &s.positions[..]));
            crate::classic_import::merge_near_corners(&parts)
        };
        if !merge.is_empty() {
            for soup in std::iter::once(&mut world)
                .chain(std::iter::once(&mut sky))
                .chain(hazards.values_mut())
                .chain(mover_geom.iter_mut())
            {
                let has_colors = soup.colors.len() == soup.positions.len();
                let mut colors = std::mem::take(&mut soup.colors);
                merge.apply(crate::classic_import::WeldSoup {
                    positions: &mut soup.positions,
                    uvs: &mut soup.uvs,
                    normals: None,
                    colors: has_colors.then_some(&mut colors),
                    indices: &mut soup.indices,
                });
                soup.colors = colors;
            }
        }
        let weld = {
            let mut parts: Vec<&[[f32; 3]]> = vec![&world.positions[..], &sky.positions[..]];
            parts.extend(hazards.values().map(|s| &s.positions[..]));
            parts.extend(mover_geom.iter().map(|s| &s.positions[..]));
            crate::classic_import::weld_parts(&parts)
        };
        for soup in std::iter::once(&mut world)
            .chain(std::iter::once(&mut sky))
            .chain(hazards.values_mut())
            .chain(mover_geom.iter_mut())
        {
            let has_colors = soup.colors.len() == soup.positions.len();
            let mut colors = std::mem::take(&mut soup.colors);
            weld.split(crate::classic_import::WeldSoup {
                positions: &mut soup.positions,
                uvs: &mut soup.uvs,
                normals: None,
                colors: has_colors.then_some(&mut colors),
                indices: &mut soup.indices,
            });
            soup.colors = colors;
        }
    }
    let colors = world.colors();
    // Quake II's shipped lightmaps ride in COLOR_0, and the material carries
    // the `lightmapTexture` marker that tells the renderer this level is
    // PRELIT — otherwise the analytic sun multiplies an already-lit level
    // and every interior goes black. The marker points back at the atlas
    // itself: a prelit shader never samples it, and a second image would
    // trip pack_import's one-texture rule.
    let marker_uvs = vec![[0.0f32, 0.0]; world.positions.len()];
    let lit = any_lit && colors.len() == world.positions.len();
    let glb = makepad_gltf::write_glb_mesh_textured_parts(
        &[makepad_gltf::GlbTexturedPart {
            positions: &world.positions,
            uvs: &world.uvs,
            indices: &world.indices,
            base_color_png: &atlas,
            normals: None,
            base_color_factor: None,
            colors: lit.then_some(&colors[..]),
            lightmap_png: Some(&atlas),
            lightmap_uvs: Some(&marker_uvs),
            detail_png: None,
            detail_scale: [0.0, 0.0],
        }],
        any_liquid,
    );
    if !glb.starts_with(b"glTF") {
        return Err("GLB encode failed".into());
    }

    let mut extra: Vec<ExtraNode> = Vec::new();
    let mut nav_doors: Vec<NavDoor> = Vec::new();
    let mut nav_lifts: Vec<NavDoor> = Vec::new();
    for (mi, mover) in movers.iter().enumerate() {
        let g = &mover_geom[mi];
        if g.is_empty() {
            continue;
        }
        match mover.kind {
            MoverKind::Door => {
                let mut node = ExtraNode::door_vector(
                    mover.name.clone(),
                    g.positions.clone(),
                    g.uvs.clone(),
                    g.indices.clone(),
                    mover.travel,
                    mover.axis,
                );
                node.colors = g.colors();
                extra.push(node);
                nav_doors.push(NavDoor {
                    name: mover.name.clone(),
                    pos: mover.centre,
                    closed_y: mover.centre[1],
                    // A Quake II door mostly slides SIDEWAYS: the Y pair is
                    // only the vertical part of the move, and `offset`
                    // carries the whole of it.
                    open_y: mover.centre[1] + mover.travel[1],
                    offset: mover.travel,
                });
            }
            MoverKind::Lift => {
                let up_y = mover.centre[1];
                let down_y = up_y + mover.travel[1];
                extra.push(ExtraNode::lift(
                    mover.name.clone(),
                    g.positions.clone(),
                    g.uvs.clone(),
                    g.indices.clone(),
                    g.colors(),
                    up_y,
                    down_y,
                ));
                nav_lifts.push(NavDoor::vertical(
                    mover.name.clone(),
                    mover.centre,
                    up_y,
                    down_y,
                ));
            }
        }
    }
    let mut hazard_n = 0usize;
    for (name, g) in hazards.iter() {
        if g.is_empty() {
            continue;
        }
        hazard_n += 1;
        extra.push(ExtraNode::hazard(
            format!("hazard_{hazard_n}"),
            g.positions.clone(),
            g.uvs.clone(),
            g.indices.clone(),
            g.colors(),
            liquid_damage(name),
            &liquid_flat(name),
            true,
            // A Quake liquid is a volume you SWIM through, not a floor you
            // stand on — the surface must not stop a walker.
            false,
        ));
    }
    let mut sky_unmapped = false;
    if !sky.is_empty() {
        match resolve_sky(bank, &entities, &sky_texture) {
            Some((name, png)) => extra.push(ExtraNode::sky(
                std::mem::take(&mut sky.positions),
                std::mem::take(&mut sky.uvs),
                std::mem::take(&mut sky.indices),
                vec![png],
                // The renderer has no cube sampler: `cube` reads the ONE
                // equirect twin `skybox` built from the six env faces, so
                // it wraps once per turn with no phase of its own.
                "cube",
                1.0,
                0.0,
                &name,
                None,
                None,
            )),
            // No `env/` set shipped: drop the sky faces exactly as this
            // converter always did, rather than paint a wrong sky.
            None => sky_unmapped = true,
        }
    } else if any_sky_face {
        sky_unmapped = true;
    }
    // Fail loudly: every node above was built from this same soup, so a
    // rejection here is a bug in this converter, and silently shipping a map
    // whose doors and sky vanished is the kind of defect that hides for
    // months.
    let glb = crate::glb_nodes::inject_nodes(&glb, &extra)
        .map_err(|e| format!("BSP38 node injection: {e}"))?;

    let mut nav = q2_nav(&entities);
    if let Some(nav) = nav.as_mut() {
        nav.doors = nav_doors;
        nav.lifts = nav_lifts;
        nav.teleports = q2_teleports(&entities, &models);
    }
    Ok(Bsp38Map {
        glb,
        nav,
        liquid: any_liquid,
        sky_unmapped,
    })
}

/// Quake II map space is Z-up: `(x, y, z)` → GLB `(x, z, −y)`, scaled to
/// metres. Every position, bound and offset in this module goes through here.
pub(crate) fn to_glb(p: [f32; 3]) -> [f32; 3] {
    [p[0] * SCALE, p[2] * SCALE, -p[1] * SCALE]
}

// ---------------------------------------------------------------------------
// Entities, brush models and the movers they name
// ---------------------------------------------------------------------------

/// One entity block's key/value pairs. Quake II repeats no key inside a
/// block, so a map is the whole of it.
type Entity = BTreeMap<String, String>;

fn entity_text(bytes: &[u8], lump: (usize, usize)) -> String {
    let (off, len) = lump;
    if len == 0 || off.saturating_add(len) > bytes.len() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes[off..off + len]).to_string()
}

/// Every `{ "key" "value" … }` block of the entity lump, in map order.
fn parse_entities(text: &str) -> Vec<Entity> {
    let mut out = Vec::new();
    for raw in text.split(|c| c == '{' || c == '}') {
        let block = raw.trim();
        if block.is_empty() {
            continue;
        }
        let mut ent = Entity::new();
        for line in block.lines() {
            let kv: Vec<&str> = line
                .trim()
                .split('"')
                .filter(|s| !s.trim().is_empty())
                .collect();
            if kv.len() < 2 {
                continue;
            }
            ent.entry(kv[0].trim().to_ascii_lowercase())
                .or_insert_with(|| kv[1].to_string());
        }
        if !ent.is_empty() {
            out.push(ent);
        }
    }
    out
}

fn classname(e: &Entity) -> &str {
    e.get("classname").map(String::as_str).unwrap_or("")
}

fn ent_f32(e: &Entity, key: &str) -> Option<f32> {
    e.get(key).and_then(|v| v.trim().parse().ok())
}

fn ent_i32(e: &Entity, key: &str) -> Option<i32> {
    e.get(key)
        .and_then(|v| v.trim().parse::<f32>().ok())
        .map(|v| v as i32)
}

fn ent_vec3(e: &Entity, key: &str) -> Option<[f32; 3]> {
    let mut it = e.get(key)?.split_whitespace();
    let x = it.next()?.trim().parse().ok()?;
    let y = it.next()?.trim().parse().ok()?;
    let z = it.next()?.trim().parse().ok()?;
    Some([x, y, z])
}

/// `"model" "*7"` → 7. Submodel 0 is the world itself and never a mover.
fn model_index(e: &Entity) -> Option<usize> {
    let n: usize = e.get("model")?.trim().strip_prefix('*')?.parse().ok()?;
    (n > 0).then_some(n)
}

/// One brush model of `LUMP_MODELS` (13). 48 bytes: `mins[3] f32`,
/// `maxs[3] f32`, `origin[3] f32`, `headnode i32`, `firstface i32`,
/// `numfaces i32`. Only the bounds and the face range are needed here.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BrushModel {
    mins: [f32; 3],
    maxs: [f32; 3],
    first_face: usize,
    num_faces: usize,
}

impl BrushModel {
    fn size(&self) -> [f32; 3] {
        [
            self.maxs[0] - self.mins[0],
            self.maxs[1] - self.mins[1],
            self.maxs[2] - self.mins[2],
        ]
    }
    fn centre(&self) -> [f32; 3] {
        [
            (self.mins[0] + self.maxs[0]) * 0.5,
            (self.mins[1] + self.maxs[1]) * 0.5,
            (self.mins[2] + self.maxs[2]) * 0.5,
        ]
    }
}

fn brush_models(bytes: &[u8], off: usize, len: usize) -> Vec<BrushModel> {
    let mut out = Vec::new();
    let n = (len / 48).min(4096);
    for i in 0..n {
        let o = off + i * 48;
        if o + 48 > bytes.len() {
            break;
        }
        let mut mins = [0.0f32; 3];
        let mut maxs = [0.0f32; 3];
        for k in 0..3 {
            mins[k] = f32_le(bytes, o + k * 4);
            maxs[k] = f32_le(bytes, o + 12 + k * 4);
        }
        out.push(BrushModel {
            mins,
            maxs,
            first_face: i32_le(bytes, o + 40).max(0) as usize,
            num_faces: i32_le(bytes, o + 44).max(0) as usize,
        });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MoverKind {
    Door,
    Lift,
}

/// A brush submodel that leaves the static mesh for a node of its own.
///
/// This is the Quake II twin of `classic_import::quake`'s `quake_doors` —
/// the same `angle` / `lip` arithmetic over a different record layout, kept
/// here because that module owns Quake 1 and this one owns Quake 2.
#[derive(Clone, Debug, PartialEq)]
struct Q2Mover {
    kind: MoverKind,
    /// `door_1`, `lift_1`, … — the glTF node, the animation and the
    /// `.spawn` line all use this one string.
    name: String,
    first_face: usize,
    num_faces: usize,
    /// Added to every vertex of this brush before it enters its node: the
    /// entity's `origin` key, plus the `START_OPEN` shift that moves a
    /// door authored at `pos1` onto the pose it actually rests shut at.
    shift: [f32; 3],
    /// Brush centre in GLB metres at the CLOSED (lift: UP) pose.
    centre: [f32; 3],
    /// GLB metres, CLOSED → OPEN (lift: UP → DOWN, so `travel.y` < 0).
    travel: [f32; 3],
    axis: &'static str,
}

/// `func_door` / `func_door_secret` / `func_plat`, resolved to face ranges
/// and their open offsets.
///
/// Quake II moves a door along `angle` (`-1` up, `-2` down, otherwise a
/// compass direction) by the brush's own size on that axis minus `lip`
/// (`SP_func_door` + `G_SetMovedir`), and drops a plat by `height` or by its
/// own Z size minus `lip` (`SP_func_plat`). Both author the brush at the
/// pose the map is compiled in.
///
/// `func_door_rotating` is NOT here: it swings, and the node contract in
/// `glb_nodes` carries a LINEAR translation clip only. Its brushes stay in
/// the static mesh rather than slide somewhere they never go.
fn q2_movers(entities: &[Entity], models: &[BrushModel]) -> Vec<Q2Mover> {
    let mut out: Vec<Q2Mover> = Vec::new();
    let mut doors = 0usize;
    let mut lifts = 0usize;
    for e in entities {
        let class = classname(e);
        let Some(mi) = model_index(e) else { continue };
        let Some(m) = models.get(mi).copied() else {
            continue;
        };
        if m.num_faces == 0 {
            continue;
        }
        let origin = ent_vec3(e, "origin").unwrap_or([0.0; 3]);
        let lip = ent_f32(e, "lip").unwrap_or(MOVER_LIP);
        let spawnflags = ent_i32(e, "spawnflags").unwrap_or(0);
        let size = m.size();
        let (kind, mut travel, mut shift) = match class {
            "func_door" => {
                let dir = movedir(ent_f32(e, "angle").unwrap_or(0.0));
                let distance =
                    (dir[0].abs() * size[0] + dir[1].abs() * size[1] + dir[2].abs() * size[2]
                        - lip)
                        .max(0.0);
                if distance <= 0.0 {
                    continue;
                }
                let mut travel = [
                    dir[0] * distance,
                    dir[1] * distance,
                    dir[2] * distance,
                ];
                let mut shift = [0.0f32; 3];
                if spawnflags & DOOR_START_OPEN != 0 {
                    // `SP_func_door`: pos1 and pos2 swap and the entity is
                    // moved to the far pose, so the brush's authored place
                    // is the OPEN one. Push the geometry to where it rests
                    // shut and run the clip back.
                    shift = travel;
                    travel = [-travel[0], -travel[1], -travel[2]];
                }
                (MoverKind::Door, travel, shift)
            }
            "func_door_secret" => {
                // `SP_func_door_secret`: the leaf slides SIDEWAYS by its own
                // width, then FORWARD by its own length. Only two poses fit
                // in one LINEAR clip, so the node interpolates straight to
                // the final open pose — both rest states are exact and only
                // the path between them is a chord instead of an L.
                let yaw = ent_f32(e, "angle").unwrap_or(0.0).to_radians();
                let forward = [yaw.cos(), yaw.sin(), 0.0f32];
                let right = [yaw.sin(), -yaw.cos(), 0.0f32];
                let side = 1.0 - (spawnflags & SECRET_1ST_LEFT) as f32;
                let length = dot3(forward, size).abs();
                let first = if spawnflags & SECRET_1ST_DOWN != 0 {
                    [0.0, 0.0, -size[2].abs()]
                } else {
                    let width = dot3(right, size).abs();
                    [right[0] * side * width, right[1] * side * width, 0.0]
                };
                let travel = [
                    first[0] + forward[0] * length,
                    first[1] + forward[1] * length,
                    first[2] + forward[2] * length,
                ];
                if travel == [0.0; 3] {
                    continue;
                }
                (MoverKind::Door, travel, [0.0; 3])
            }
            "func_plat" => {
                // `SP_func_plat`: pos1 is the authored (top) pose, pos2 is
                // `height` below it, or the brush's own Z size minus `lip`.
                let drop = ent_f32(e, "height")
                    .filter(|h| *h > 0.0)
                    .unwrap_or(size[2] - lip)
                    .max(0.0);
                if drop <= 0.0 {
                    continue;
                }
                (MoverKind::Lift, [0.0, 0.0, -drop], [0.0; 3])
            }
            _ => continue,
        };
        // Everything above is Quake II map space; the node lives in GLB
        // space, and so does the brush centre the `.spawn` sidecar quotes.
        shift = to_glb([shift[0] + origin[0], shift[1] + origin[1], shift[2] + origin[2]]);
        travel = to_glb(travel);
        let c = to_glb(m.centre());
        let centre = [c[0] + shift[0], c[1] + shift[1], c[2] + shift[2]];
        let name = match kind {
            MoverKind::Door => {
                doors += 1;
                format!("door_{doors}")
            }
            MoverKind::Lift => {
                lifts += 1;
                format!("lift_{lifts}")
            }
        };
        out.push(Q2Mover {
            kind,
            name,
            first_face: m.first_face,
            num_faces: m.num_faces,
            shift,
            centre,
            travel,
            axis: dominant_axis(travel),
        });
    }
    out
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `G_SetMovedir` with `ED_ParseField`'s angle hack: `-1` is straight up,
/// `-2` straight down, anything else a yaw in degrees. Quake II map space.
fn movedir(angle: f32) -> [f32; 3] {
    if (angle - -1.0).abs() < 0.01 {
        [0.0, 0.0, 1.0]
    } else if (angle - -2.0).abs() < 0.01 {
        [0.0, 0.0, -1.0]
    } else {
        let r = angle.to_radians();
        [r.cos(), r.sin(), 0.0]
    }
}

fn dominant_axis(v: [f32; 3]) -> &'static str {
    if v[1].abs() >= v[0].abs() && v[1].abs() >= v[2].abs() {
        "y"
    } else if v[0].abs() >= v[2].abs() {
        "x"
    } else {
        "z"
    }
}

// ---------------------------------------------------------------------------
// Navigation facts
// ---------------------------------------------------------------------------

/// Every `info_player_*` entity, the floor under the primary one, the engine
/// constants, and the named points a navigator needs but cannot see.
///
/// `info_player_start` is the primary; further starts and `info_player_coop`
/// become `player_start_2`…; `info_player_deathmatch` becomes `deathmatch_N`.
fn q2_nav(entities: &[Entity]) -> Option<WorldNav> {
    let mut primary: Option<NavStart> = None;
    let mut coop: Vec<NavStart> = Vec::new();
    let mut deathmatch: Vec<NavStart> = Vec::new();
    let mut markers: Vec<NavStart> = Vec::new();
    let mut floor_y = None;
    for e in entities {
        let class = classname(e);
        let Some(o) = ent_vec3(e, "origin") else {
            // A brush entity without an explicit `origin` would land at the
            // map centre; Quake 1 skips those and so does this.
            continue;
        };
        let angle = ent_f32(e, "angle").unwrap_or(0.0);
        let start = NavStart {
            name: String::new(),
            pos: [
                o[0] * SCALE,
                o[2] * SCALE + VIEW_HEIGHT * SCALE,
                -o[1] * SCALE,
            ],
            yaw: std::f32::consts::FRAC_PI_2 - angle.to_radians(),
            pitch: 0.0,
        };
        if let Some(name) = marker_name(class) {
            if o != [0.0; 3] && !markers.iter().any(|m| m.name == name) {
                markers.push(NavStart { name, ..start });
            }
            continue;
        }
        match class {
            "info_player_start" if primary.is_none() => {
                floor_y = Some((o[2] - ORIGIN_ABOVE_FLOOR) * SCALE);
                primary = Some(start);
            }
            "info_player_start" | "info_player_coop" => coop.push(start),
            "info_player_deathmatch" => deathmatch.push(start),
            _ => {}
        }
    }
    if primary.is_none() && coop.is_empty() && deathmatch.is_empty() {
        return None;
    }
    let mut starts = Vec::with_capacity(1 + coop.len() + deathmatch.len());
    for (i, mut s) in primary.into_iter().chain(coop).enumerate() {
        s.name = player_start_name(i);
        starts.push(s);
    }
    for (i, mut s) in deathmatch.into_iter().enumerate() {
        s.name = deathmatch_name(i);
        starts.push(s);
    }
    // Quake 2 player: origin 24 units above the floor, eye +22 from there,
    // STEPSIZE 18 — all at this converter's 1/64 map scale.
    let eye = (VIEW_HEIGHT + ORIGIN_ABOVE_FLOOR) * SCALE;
    let floor_y = floor_y.unwrap_or(starts[0].pos[1] - eye);
    Some(WorldNav {
        starts,
        floor_y: Some(floor_y),
        step_height: Some(STEP_HEIGHT * SCALE),
        eye_height: Some(eye),
        doors: Vec::new(),
        lifts: Vec::new(),
        teleports: Vec::new(),
        markers,
    })
}

/// The named points of a Quake II map: the level exit and the keys.
///
/// Quake II's `key_*` classnames spell the item, so one rule covers the
/// whole family and a walker sees them as the same `key_…` names Doom's
/// `key_red` / `key_blue` publish. There is NO secret-exit marker: the BSP
/// never says which `target_changelevel` is the hidden one (`map
/// "demo2$base1"` names a destination SPAWNPOINT, not a secret), so none is
/// invented.
fn marker_name(class: &str) -> Option<String> {
    if class == "target_changelevel" {
        return Some("exit".into());
    }
    let item = class.strip_prefix("key_")?;
    let item = item.strip_suffix("_key").unwrap_or(item);
    if item.is_empty() {
        return None;
    }
    Some(format!("key_{item}"))
}

/// `trigger_teleport` pads and where they land. The trigger is a brush
/// entity — its submodel bounds ARE the pad — and its `target` names a
/// destination entity's `targetname`.
fn q2_teleports(entities: &[Entity], models: &[BrushModel]) -> Vec<NavTeleport> {
    let mut dests: BTreeMap<&str, &Entity> = BTreeMap::new();
    for e in entities {
        if !matches!(
            classname(e),
            "misc_teleporter_dest"
                | "info_teleport_destination"
                | "target_position"
                | "info_notnull"
        ) {
            continue;
        }
        if let Some(t) = e.get("targetname") {
            dests.entry(t.as_str()).or_insert(e);
        }
    }
    let mut out = Vec::new();
    for e in entities {
        if classname(e) != "trigger_teleport" {
            continue;
        }
        let Some(dest) = e.get("target").and_then(|t| dests.get(t.as_str())) else {
            continue;
        };
        let Some(o) = ent_vec3(dest, "origin") else {
            continue;
        };
        let Some(m) = model_index(e).and_then(|mi| models.get(mi).copied()) else {
            continue;
        };
        let shift = ent_vec3(e, "origin").unwrap_or([0.0; 3]);
        let lo = to_glb([
            m.mins[0] + shift[0],
            m.mins[1] + shift[1],
            m.mins[2] + shift[2],
        ]);
        let hi = to_glb([
            m.maxs[0] + shift[0],
            m.maxs[1] + shift[1],
            m.maxs[2] + shift[2],
        ]);
        // The Z-up → Y-up flip negates map Y, so the pad's z bounds swap.
        let angle = ent_f32(dest, "angle").unwrap_or(0.0);
        out.push(NavTeleport {
            name: format!("teleport_{}", out.len() + 1),
            pad_min: [lo[0].min(hi[0]), lo[2].min(hi[2])],
            pad_max: [lo[0].max(hi[0]), lo[2].max(hi[2])],
            dst: [
                o[0] * SCALE,
                o[2] * SCALE + VIEW_HEIGHT * SCALE,
                -o[1] * SCALE,
            ],
            yaw: std::f32::consts::FRAC_PI_2 - angle.to_radians(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Sky and liquids
// ---------------------------------------------------------------------------

/// Which `env/` set this map's sky is, and its equirect twin.
///
/// In order: the `worldspawn` `sky` key (what the engine is told), then the
/// basename of the `SURF_SKY` texture the compiler wrote (`e1u1/sky1` →
/// `sky1`, which is the ONLY set the shareware pak ships and the only way to
/// find it, since those maps carry no `sky` key), then `SP_worldspawn`'s own
/// `unit1_` default. The first candidate that yields a panorama wins.
fn resolve_sky(bank: &WalBank, entities: &[Entity], sky_texture: &str) -> Option<(String, Vec<u8>)> {
    let declared = entities
        .iter()
        .find(|e| classname(e) == "worldspawn")
        .and_then(|e| e.get("sky"))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    let from_face = sky_texture
        .rsplit('/')
        .next()
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    let mut tried: Vec<String> = Vec::new();
    for name in declared
        .into_iter()
        .chain(from_face)
        .chain(std::iter::once(DEFAULT_SKY.to_string()))
    {
        if tried.contains(&name) {
            continue;
        }
        if let Some(png) = env_equirect(bank, &name) {
            return Some((name, png));
        }
        tried.push(name);
    }
    None
}

/// The six `env/<set><suffix>` faces of a set, resampled into the one
/// equirectangular image the renderer's `cube` projection samples.
fn env_equirect(bank: &WalBank, set: &str) -> Option<Vec<u8>> {
    let mut faces: [Option<CubeFace>; 6] = [None, None, None, None, None, None];
    let mut found = 0usize;
    for (i, suffix) in FACE_SUFFIXES.iter().enumerate() {
        let Some(img) = bank.tiles.get(&format!("env/{set}{suffix}")) else {
            continue;
        };
        faces[i] = CubeFace::new(img.w, img.h, img.rgba.clone());
        if faces[i].is_some() {
            found += 1;
        }
    }
    if found == 0 {
        return None;
    }
    cube_to_equirect_png(&faces)
}

/// What a Quake II liquid does to a swimmer, on the same percent-per-second
/// scale the Doom and Quake 1 hazards use. The BSP's per-face record carries
/// no contents field — `CONTENTS_LAVA` / `CONTENTS_SLIME` live on the brush,
/// which no face points at — so this reads the texture name, exactly as the
/// Quake 1 converter does. Everything else (water, sewage) is harmless.
fn liquid_damage(name: &str) -> u8 {
    let n = liquid_flat(name);
    if n.starts_with("lava") {
        20
    } else if n.starts_with("slime") {
        10
    } else {
        0
    }
}

/// The texture's own name without its WAD folder: `e1u1/bluwter` →
/// `bluwter`, the string a walker reads out of `extras.flat`.
fn liquid_flat(name: &str) -> String {
    name.trim_start_matches('*')
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

struct TexInfo {
    svec: [f32; 4],
    tvec: [f32; 4],
    flags: i32,
    name: String,
}

/// Compiler-only surfaces that never reach a renderer. A `sky*` name is NOT
/// one of them: the sky is drawn, direction-mapped, by the `sky` node, and
/// `SURF_SKY` is what says so.
fn skip_tex_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("clip")
        || n.starts_with("trigger")
        || n == "skip"
        || n == "hint"
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

    /// One axis-aligned quad of the fixture: four map-space corners and the
    /// texinfo that paints them.
    struct Quad {
        corners: [[f32; 3]; 4],
        tex: usize,
    }

    /// A texinfo record: the WAL name and its `SURF_*` flags. The s/t vectors
    /// are the identity x/y planar mapping every fixture quad uses.
    struct TexDef {
        name: String,
        flags: i32,
    }

    fn tex(name: &str, flags: i32) -> TexDef {
        TexDef {
            name: name.into(),
            flags,
        }
    }

    /// A `LUMP_MODELS` record: bounds plus the face range it owns.
    struct ModelDef {
        mins: [f32; 3],
        maxs: [f32; 3],
        first: i32,
        num: i32,
    }

    /// Assemble a BSP38 out of quads, texinfos, brush models and an entity
    /// lump. `lighting` bytes of mid-grey light are attached to every face
    /// when non-zero, which is what makes the map PRELIT.
    fn build_bsp38(
        quads: &[Quad],
        texes: &[TexDef],
        models: &[ModelDef],
        ents: &str,
        lighting: usize,
    ) -> Vec<u8> {
        let ents_b = ents.as_bytes().to_vec();

        let mut verts = Vec::new();
        let mut edges = Vec::new();
        let mut se = Vec::new();
        let mut faces = Vec::new();
        for (qi, quad) in quads.iter().enumerate() {
            let base = (qi * 4) as u16;
            for c in &quad.corners {
                push_f32(&mut verts, c[0]);
                push_f32(&mut verts, c[1]);
                push_f32(&mut verts, c[2]);
            }
            for k in 0..4u16 {
                push_u16(&mut edges, base + k);
                push_u16(&mut edges, base + (k + 1) % 4);
            }
            for k in 0..4 {
                push_i32(&mut se, (qi * 4 + k) as i32);
            }
            push_u16(&mut faces, qi as u16); // planenum: one per quad
            push_i16(&mut faces, 0); // side
            push_i32(&mut faces, (qi * 4) as i32); // firstedge
            push_i16(&mut faces, 4); // numedges
            push_i16(&mut faces, quad.tex as i16);
            if lighting > 0 {
                faces.extend_from_slice(&[0u8, 255, 255, 255]);
                push_i32(&mut faces, 0);
            } else {
                faces.extend_from_slice(&[255u8, 255, 255, 255]);
                push_i32(&mut faces, -1);
            }
        }
        assert_eq!(faces.len(), quads.len() * 20);

        let mut tex = Vec::new();
        for t in texes {
            for v in [1.0f32, 0.0, 0.0, 0.0] {
                push_f32(&mut tex, v); // s vector
            }
            for v in [0.0f32, 1.0, 0.0, 0.0] {
                push_f32(&mut tex, v); // t vector
            }
            push_i32(&mut tex, t.flags);
            push_i32(&mut tex, 0); // value
            let mut name = [0u8; 32];
            let nb = t.name.as_bytes();
            name[..nb.len().min(31)].copy_from_slice(&nb[..nb.len().min(31)]);
            tex.extend_from_slice(&name);
            push_i32(&mut tex, -1); // nexttexinfo
        }
        assert_eq!(tex.len(), texes.len() * 76);

        let mut mdl = Vec::new();
        for m in models {
            for v in m.mins {
                push_f32(&mut mdl, v);
            }
            for v in m.maxs {
                push_f32(&mut mdl, v);
            }
            for _ in 0..3 {
                push_f32(&mut mdl, 0.0); // origin (unused by this reader)
            }
            push_i32(&mut mdl, 0); // headnode
            push_i32(&mut mdl, m.first);
            push_i32(&mut mdl, m.num);
        }
        assert_eq!(mdl.len(), models.len() * 48);

        let light = vec![128u8; lighting];

        const HEADER: usize = 8 + 19 * 8;
        let mut lumps = [(0i32, 0i32); 19];
        let mut off = HEADER as i32;
        let place = |lump: &mut (i32, i32), data: &[u8], off: &mut i32| {
            *lump = (*off, data.len() as i32);
            *off += data.len() as i32;
        };
        place(&mut lumps[LUMP_ENTITIES], &ents_b, &mut off);
        place(&mut lumps[LUMP_VERTEXES], &verts, &mut off);
        place(&mut lumps[LUMP_EDGES], &edges, &mut off);
        place(&mut lumps[LUMP_SURFEDGES], &se, &mut off);
        place(&mut lumps[LUMP_TEXINFO], &tex, &mut off);
        place(&mut lumps[LUMP_FACES], &faces, &mut off);
        place(&mut lumps[LUMP_MODELS], &mdl, &mut off);
        place(&mut lumps[LUMP_LIGHTING], &light, &mut off);

        let mut out = Vec::new();
        out.extend_from_slice(b"IBSP");
        push_i32(&mut out, 38);
        for (o, l) in lumps {
            push_i32(&mut out, o);
            push_i32(&mut out, l);
        }
        assert_eq!(out.len(), HEADER);
        for chunk in [&ents_b, &verts, &edges, &se, &tex, &faces, &mdl, &light] {
            out.extend_from_slice(chunk);
        }
        out
    }

    fn make_bsp38_quad(tex_name: &str) -> Vec<u8> {
        build_bsp38(
            &[Quad {
                corners: [
                    [0.0, 0.0, 0.0],
                    [64.0, 0.0, 0.0],
                    [64.0, 64.0, 0.0],
                    [0.0, 64.0, 0.0],
                ],
                tex: 0,
            }],
            &[tex(tex_name, 0)],
            &[ModelDef {
                mins: [0.0; 3],
                maxs: [64.0, 64.0, 0.0],
                first: 0,
                num: 1,
            }],
            "{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 16\"\n\"angle\" \"90\"\n}\n",
            0,
        )
    }

    /// The whole map contract in one synthetic BSP: a world floor, a sky
    /// face, a liquid face, a `func_door` submodel, a `func_plat` submodel,
    /// a `trigger_teleport` pad with its destination, an exit, a key, coop
    /// and deathmatch starts, and a lightmap lump.
    fn make_bsp38_contract() -> Vec<u8> {
        let quad = |x0: f32, y0: f32, z: f32, tex: usize| Quad {
            corners: [
                [x0, y0, z],
                [x0 + 64.0, y0, z],
                [x0 + 64.0, y0 + 64.0, z],
                [x0, y0 + 64.0, z],
            ],
            tex,
        };
        let ents = concat!(
            "{\n\"classname\" \"worldspawn\"\n\"sky\" \"sky1\"\n}\n",
            "{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 16\"\n\"angle\" \"90\"\n}\n",
            "{\n\"classname\" \"info_player_coop\"\n\"origin\" \"64 32 16\"\n}\n",
            "{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"96 32 16\"\n}\n",
            "{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n}\n",
            "{\n\"classname\" \"func_plat\"\n\"model\" \"*2\"\n}\n",
            "{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*3\"\n\"target\" \"tdest\"\n}\n",
            "{\n\"classname\" \"misc_teleporter_dest\"\n\"targetname\" \"tdest\"\n",
            "\"origin\" \"640 128 32\"\n\"angle\" \"180\"\n}\n",
            "{\n\"classname\" \"target_changelevel\"\n\"origin\" \"128 32 16\"\n",
            "\"map\" \"demo2$base1\"\n}\n",
            "{\n\"classname\" \"key_blue_key\"\n\"origin\" \"160 32 16\"\n}\n",
        );
        build_bsp38(
            &[
                quad(0.0, 0.0, 0.0, 0),      // 0: world floor
                quad(0.0, 0.0, 256.0, 1),    // 1: sky
                quad(128.0, 0.0, 32.0, 2),   // 2: liquid
                quad(256.0, 128.0, 0.0, 0),  // 3: door leaf
                quad(384.0, 128.0, 32.0, 0), // 4: lift floor
            ],
            &[
                tex("e1u1/floor3_3", 0),
                tex("e1u1/sky1", SURF_SKY),
                tex("e1u1/bluwter", SURF_WARP),
            ],
            &[
                ModelDef {
                    mins: [0.0, 0.0, 0.0],
                    maxs: [448.0, 256.0, 256.0],
                    first: 0,
                    num: 3,
                },
                // Door: 64 tall, opens UP by 64 - lip 8 = 56 units.
                ModelDef {
                    mins: [256.0, 128.0, 0.0],
                    maxs: [320.0, 192.0, 64.0],
                    first: 3,
                    num: 1,
                },
                // Plat: 32 tall, drops by 32 - lip 8 = 24 units.
                ModelDef {
                    mins: [384.0, 128.0, 0.0],
                    maxs: [448.0, 192.0, 32.0],
                    first: 4,
                    num: 1,
                },
                // Teleport trigger: a brush with no drawn faces.
                ModelDef {
                    mins: [512.0, 0.0, 0.0],
                    maxs: [576.0, 64.0, 64.0],
                    first: 5,
                    num: 0,
                },
            ],
            ents,
            4096,
        )
    }

    /// A run-length-encoded 8-bit PCX of one palette index — the shape the
    /// `env/` cube faces ship in.
    fn make_pcx(w: u16, h: u16, index: u8, color: [u8; 3]) -> Vec<u8> {
        let mut out = vec![0u8; 128];
        out[0] = 0x0A;
        out[1] = 5;
        out[2] = 1; // RLE
        out[3] = 8; // bits per pixel
        out[8..10].copy_from_slice(&(w - 1).to_le_bytes());
        out[10..12].copy_from_slice(&(h - 1).to_le_bytes());
        out[65] = 1; // planes
        out[66..68].copy_from_slice(&w.to_le_bytes());
        for _ in 0..h {
            for _ in 0..w {
                // Any byte with the top two bits set must be escaped.
                if index & 0xC0 == 0xC0 {
                    out.push(0xC1);
                }
                out.push(index);
            }
        }
        out.push(0x0C);
        let mut pal = [0u8; 768];
        pal[index as usize * 3] = color[0];
        pal[index as usize * 3 + 1] = color[1];
        pal[index as usize * 3 + 2] = color[2];
        out.extend_from_slice(&pal);
        out
    }

    /// Write the six `env/sky1*` faces of a cube set, one flat colour each.
    fn write_env_set(root: &Path, set: &str) {
        let dir = root.join("env");
        std::fs::create_dir_all(&dir).unwrap();
        let colors: [[u8; 3]; 6] = [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [0, 255, 255],
            [255, 0, 255],
        ];
        for (i, suffix) in FACE_SUFFIXES.iter().enumerate() {
            let pcx = make_pcx(16, 16, (i + 1) as u8, colors[i]);
            std::fs::write(dir.join(format!("{set}{suffix}.pcx")), pcx).unwrap();
        }
    }

    fn glb_json(glb: &[u8]) -> String {
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        String::from_utf8_lossy(&glb[20..20 + len]).to_string()
    }

    fn glb_bin(glb: &[u8]) -> Vec<u8> {
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let at = 20 + json_len;
        let bin_len = u32::from_le_bytes(glb[at..at + 4].try_into().unwrap()) as usize;
        glb[at + 8..at + 8 + bin_len].to_vec()
    }

    /// Read a float accessor's values through its bufferView.
    fn read_accessor(
        root: &makepad_asset_client::json::Value,
        bin: &[u8],
        index: &makepad_asset_client::json::Value,
    ) -> Vec<f32> {
        use makepad_asset_client::json::Value;
        let i = index.as_i64().unwrap() as usize;
        let acc = &root.get("accessors").unwrap().as_arr().unwrap()[i];
        let vi = acc.get("bufferView").and_then(Value::as_i64).unwrap() as usize;
        let view = &root.get("bufferViews").unwrap().as_arr().unwrap()[vi];
        let off = view.get("byteOffset").and_then(Value::as_i64).unwrap_or(0) as usize;
        let len = view.get("byteLength").and_then(Value::as_i64).unwrap() as usize;
        bin[off..off + len]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    fn num(v: &makepad_asset_client::json::Value) -> f64 {
        use makepad_asset_client::json::Value;
        match v {
            Value::F64(f) => *f,
            Value::Int(i) => *i as f64,
            _ => f64::NAN,
        }
    }

    fn node<'a>(
        root: &'a makepad_asset_client::json::Value,
        name: &str,
    ) -> &'a makepad_asset_client::json::Value {
        use makepad_asset_client::json::Value;
        root.get("nodes")
            .and_then(Value::as_arr)
            .unwrap()
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("no `{name}` node"))
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

    /// Convert [`make_bsp38_contract`] with a bank holding its WALs and its
    /// `env/` cube set. Returns (staged dir, source dir, the World asset).
    fn convert_contract_map(tag: &str) -> (PathBuf, PathBuf, ClassicAsset) {
        let root = tmp_dir(&format!("{tag}_src"));
        let staged = tmp_dir(&format!("{tag}_stage"));
        let tex_dir = root.join("textures/e1u1");
        std::fs::create_dir_all(&tex_dir).unwrap();
        for (name, fill) in [("floor3_3", 15u8), ("bluwter", 200), ("sky1", 8)] {
            let wal = make_wal(&format!("e1u1/{name}"), 64, 64, fill);
            std::fs::write(tex_dir.join(format!("{name}.wal")), &wal).unwrap();
        }
        write_env_set(&root, "sky1");
        let bank = load_wal_bank(&root);
        assert!(
            bank.tiles.contains_key("env/sky1rt"),
            "the env walk must key cube faces: {:?}",
            bank.tiles.keys().collect::<Vec<_>>()
        );
        let mut assets =
            convert_bsp38(&make_bsp38_contract(), "maps/base1.bsp", &staged, &bank, QUAKE2_SOURCE_ID)
                .expect("convert contract bsp38");
        assert_eq!(assets.len(), 1);
        (staged, root, assets.remove(0))
    }

    /// The whole unified map contract, from one synthetic BSP: a door and a
    /// lift that animate, a hazard that does not, a sky with its own picture,
    /// and a level mesh marked PRELIT.
    #[test]
    fn a_quake2_map_exports_doors_lifts_hazards_and_a_sky() {
        use makepad_asset_client::json::{self, Value};
        let (staged, root, asset) = convert_contract_map("contract");
        let glb = std::fs::read(staged.join(&asset.rel_path)).unwrap();
        let bin = glb_bin(&glb);
        let js = json::parse(glb_json(&glb).as_bytes()).expect("glb json");

        // --- door -----------------------------------------------------
        // 64 units tall, `angle -1` (up), default lip 8 -> 56 units = 1.75 m.
        let door = node(&js, "door_1");
        let extras = door.get("extras").expect("door extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("door"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("open"));
        assert_eq!(extras.get("axis").and_then(Value::as_str), Some("y"));
        assert!((num(extras.get("travel").unwrap()) - (56.0 * SCALE) as f64).abs() < 1e-5);
        let rest = door.get("translation").and_then(Value::as_arr).unwrap();
        assert!(
            (num(&rest[1]) - (56.0 * SCALE) as f64).abs() < 1e-5 && num(&rest[0]).abs() < 1e-6,
            "a door rests OPEN: {rest:?}"
        );
        // One LINEAR translation clip: t=0 CLOSED (the authored pose) ->
        // t=seconds OPEN (the rest pose).
        let anims = js.get("animations").and_then(Value::as_arr).unwrap();
        let clip = anims
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door_1 animation");
        let sampler = &clip.get("samplers").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(
            sampler.get("interpolation").and_then(Value::as_str),
            Some("LINEAR")
        );
        let channel = &clip.get("channels").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(
            channel
                .get("target")
                .and_then(|t| t.get("path"))
                .and_then(Value::as_str),
            Some("translation")
        );
        let times = read_accessor(&js, &bin, sampler.get("input").unwrap());
        let values = read_accessor(&js, &bin, sampler.get("output").unwrap());
        assert_eq!(times.len(), 2, "two keyframes: closed and open");
        assert_eq!(values.len(), 6);
        assert_eq!(&values[0..3], &[0.0, 0.0, 0.0], "t=0 is the closed pose");
        assert!((values[4] - 56.0 * SCALE).abs() < 1e-5, "t=end is the open pose");

        // --- lift -----------------------------------------------------
        // 32 units tall, default lip 8 -> drops 24 units.
        let lift = node(&js, "lift_1");
        let extras = lift.get("extras").expect("lift extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("lift"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("up"));
        // Brush centre z = 16 units up, dropping 24 units.
        assert!((num(extras.get("up").unwrap()) - (16.0 * SCALE) as f64).abs() < 1e-5);
        assert!((num(extras.get("down").unwrap()) + (8.0 * SCALE) as f64).abs() < 1e-5);
        assert!(
            lift.get("translation").is_none(),
            "a lift rests UP, where the level is baked"
        );
        assert!(anims
            .iter()
            .any(|a| a.get("name").and_then(Value::as_str) == Some("lift_1")));

        // --- hazard ---------------------------------------------------
        let hazard = node(&js, "hazard_1");
        let extras = hazard.get("extras").expect("hazard extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("hazard"));
        assert_eq!(extras.get("flat").and_then(Value::as_str), Some("bluwter"));
        assert_eq!(extras.get("damage").and_then(Value::as_i64), Some(0));
        assert_eq!(extras.get("liquid").and_then(Value::as_bool), Some(true));
        assert_eq!(
            extras.get("solid").and_then(Value::as_bool),
            Some(false),
            "a Quake liquid is swum through, not stood on"
        );
        assert!(hazard.get("translation").is_none());

        // --- sky ------------------------------------------------------
        let sky = node(&js, "sky");
        let extras = sky.get("extras").expect("sky extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("sky"));
        assert_eq!(
            extras.get("projection").and_then(Value::as_str),
            Some("cube"),
            "Quake II ships a six-face environment box"
        );
        assert_eq!(num(extras.get("repeat").unwrap()), 1.0);
        assert_eq!(num(extras.get("offset").unwrap()), 0.0);
        assert_eq!(extras.get("texture").and_then(Value::as_str), Some("sky1"));
        assert!(
            extras.get("layers").is_none(),
            "the cube sky is ONE equirect image, not a layer stack"
        );
        // Its own material and image, not the level atlas (material 0).
        let material = sky
            .get("mesh")
            .and_then(Value::as_i64)
            .and_then(|m| js.get("meshes").and_then(Value::as_arr).unwrap().get(m as usize))
            .and_then(|m| m.get("primitives"))
            .and_then(Value::as_arr)
            .and_then(|p| p[0].get("material"))
            .and_then(Value::as_i64)
            .expect("sky material");
        assert!(material > 0, "the sky does not paint with the level atlas");

        // --- the level itself is PRELIT --------------------------------
        let materials = js.get("materials").and_then(Value::as_arr).unwrap();
        assert!(
            materials[0]
                .get("extras")
                .and_then(|e| e.get("lightmapTexture"))
                .is_some(),
            "a Quake II map ships lightmaps: the sun must not light it again"
        );
        // And the moving / liquid / sky faces really did leave it.
        let parts = crate::world_preview::extract_glb_parts(&glb).expect("parts");
        assert!(parts.len() >= 5, "level + door + lift + hazard + sky");
        let level_tris = parts[0].indices.len() / 3;
        assert_eq!(level_tris, 2, "only the world floor quad is left: {level_tris}");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    /// The `.spawn` sidecar and the anchors a catalog World publishes.
    #[test]
    fn a_quake2_map_publishes_its_starts_movers_and_markers() {
        let (staged, root, _asset) = convert_contract_map("nav");
        let text = std::fs::read_to_string(staged.join("worlds/base1.spawn")).unwrap();
        // Lines 1-3 stay byte-identical to the original `world-spawn 1`.
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), "world-spawn 1");
        assert_eq!(lines.next().unwrap().split_whitespace().count(), 3);
        assert_eq!(lines.next().unwrap().split_whitespace().count(), 2);
        for needle in [
            "\nstart player_start ",
            "\nstart player_start_2 ",
            "\nstart deathmatch_1 ",
            "\nfloor ",
            "\nstep ",
            "\neye ",
            "\ndoor door_1 ",
            "\nlift lift_1 ",
            "\nmarker exit ",
            "\nmarker key_blue ",
            "\nteleport teleport_1 ",
        ] {
            assert!(text.contains(needle), "`{needle}` missing from:\n{text}");
        }

        let nav = crate::world_nav::WorldNav::parse(&text).expect("round trip");
        // origin 16 units up, eye 22 above a 24-unit origin offset.
        let start = &nav.starts[0];
        assert_eq!(start.name, "player_start");
        assert!((start.pos[0] - 32.0 * SCALE).abs() < 1e-4, "{start:?}");
        assert!((start.pos[1] - (16.0 + 22.0) * SCALE).abs() < 1e-4, "{start:?}");
        assert!((start.pos[2] + 32.0 * SCALE).abs() < 1e-4, "{start:?}");
        assert!((start.yaw - (std::f32::consts::FRAC_PI_2 - 90f32.to_radians())).abs() < 1e-4);
        // The sidecar quotes four decimals, so these are compared as such.
        let near = |got: Option<f32>, want: f32| got.is_some_and(|v| (v - want).abs() < 1e-4);
        assert!(near(nav.floor_y, (16.0 - 24.0) * SCALE), "{:?}", nav.floor_y);
        assert!(near(nav.eye_height, (22.0 + 24.0) * SCALE), "{:?}", nav.eye_height);
        assert!(near(nav.step_height, 18.0 * SCALE), "{:?}", nav.step_height);

        // Door centre (288, 160, 32) map units, GLB axes.
        let door = &nav.doors[0];
        assert_eq!(door.name, "door_1");
        assert!((door.pos[0] - 288.0 * SCALE).abs() < 1e-4, "{door:?}");
        assert!((door.pos[2] + 160.0 * SCALE).abs() < 1e-4, "{door:?}");
        assert!((door.closed_y - 32.0 * SCALE).abs() < 1e-4, "{door:?}");
        assert!((door.open_y - 88.0 * SCALE).abs() < 1e-4, "{door:?}");
        // A lift rests UP and travels DOWN, so its travel is negative.
        let lift = &nav.lifts[0];
        assert!((lift.closed_y - 16.0 * SCALE).abs() < 1e-4, "{lift:?}");
        assert!((lift.open_y + 8.0 * SCALE).abs() < 1e-4, "{lift:?}");
        // Pad from the trigger brush, destination from `misc_teleporter_dest`.
        let tp = &nav.teleports[0];
        assert!((tp.pad_min[0] - 512.0 * SCALE).abs() < 1e-4, "{tp:?}");
        assert!((tp.pad_max[0] - 576.0 * SCALE).abs() < 1e-4, "{tp:?}");
        assert!((tp.pad_min[1] + 64.0 * SCALE).abs() < 1e-4, "{tp:?}");
        assert!((tp.pad_max[1] - 0.0).abs() < 1e-4, "{tp:?}");
        assert!((tp.dst[0] - 640.0 * SCALE).abs() < 1e-4, "{tp:?}");
        assert!((tp.dst[1] - (32.0 + 22.0) * SCALE).abs() < 1e-4, "{tp:?}");
        assert!((tp.dst[2] + 128.0 * SCALE).abs() < 1e-4, "{tp:?}");

        let names: Vec<String> = nav.anchors().into_iter().map(|a| a.name).collect();
        for want in [
            "floor_height",
            "step_height",
            "eye_height",
            "player_start",
            "deathmatch_1",
            "door_1",
            "lift_1",
            "exit",
            "key_blue",
            "teleport_1",
        ] {
            assert!(names.contains(&want.to_string()), "{want} missing: {names:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    /// Without an `env/` set there is no honest sky, so the faces are dropped
    /// exactly as they always were and the World says so on its own card.
    #[test]
    fn a_map_whose_pack_ships_no_env_set_says_so_instead_of_guessing() {
        let root = tmp_dir("nosky_src");
        let staged = tmp_dir("nosky_stage");
        let tex_dir = root.join("textures/e1u1");
        std::fs::create_dir_all(&tex_dir).unwrap();
        for name in ["floor3_3", "bluwter"] {
            let wal = make_wal(&format!("e1u1/{name}"), 64, 64, 15);
            std::fs::write(tex_dir.join(format!("{name}.wal")), &wal).unwrap();
        }
        let bank = load_wal_bank(&root);
        let assets = convert_bsp38(
            &make_bsp38_contract(),
            "maps/base1.bsp",
            &staged,
            &bank,
            QUAKE2_SOURCE_ID,
        )
        .expect("convert");
        assert!(
            assets[0].tags.iter().any(|t| t == "sky-unmapped"),
            "tags: {:?}",
            assets[0].tags
        );
        let glb = std::fs::read(staged.join(&assets[0].rel_path)).unwrap();
        let js = glb_json(&glb);
        assert!(!js.contains("\"sky\""), "no env images, no sky node");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    /// `DOOR_START_OPEN` authors the brush where it looks shut but leaves it
    /// standing open, so the two poses swap: the node's geometry moves onto
    /// the far pose and the clip runs back.
    #[test]
    fn a_start_open_door_swaps_its_two_poses() {
        let models = [
            BrushModel::default(),
            BrushModel {
                mins: [0.0, 0.0, 0.0],
                maxs: [64.0, 64.0, 64.0],
                first_face: 0,
                num_faces: 1,
            },
        ];
        let plain = parse_entities("{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n}\n");
        let open = parse_entities(
            "{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n\"spawnflags\" \"1\"\n}\n",
        );
        let a = &q2_movers(&plain, &models)[0];
        let b = &q2_movers(&open, &models)[0];
        assert!((a.travel[1] - 56.0 * SCALE).abs() < 1e-6, "{a:?}");
        assert_eq!(a.shift, [0.0; 3]);
        assert!((b.travel[1] + 56.0 * SCALE).abs() < 1e-6, "start-open runs back: {b:?}");
        assert!((b.shift[1] - 56.0 * SCALE).abs() < 1e-6, "and is authored open: {b:?}");
        // Both agree about where the door is when it is shut.
        assert!((a.centre[1] + 56.0 * SCALE - b.centre[1]).abs() < 1e-6);
    }

    /// A brush entity with an `origin` key is compiled around zero and moved
    /// at draw time. Miss that and every rotating door lands at the map's
    /// centre.
    #[test]
    fn a_submodel_with_an_origin_key_is_placed_by_it() {
        let models = [
            BrushModel::default(),
            BrushModel {
                mins: [-32.0, -32.0, -32.0],
                maxs: [32.0, 32.0, 32.0],
                first_face: 0,
                num_faces: 1,
            },
        ];
        let ents = parse_entities(
            "{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n\"origin\" \"640 128 64\"\n}\n",
        );
        let m = &q2_movers(&ents, &models)[0];
        // (640, 128, 64) map units, and the brush's own centre is zero.
        assert!((m.centre[0] - 640.0 * SCALE).abs() < 1e-5, "{m:?}");
        assert!((m.centre[1] - 64.0 * SCALE).abs() < 1e-5, "{m:?}");
        assert!((m.centre[2] + 128.0 * SCALE).abs() < 1e-5, "{m:?}");
        assert_eq!(m.shift, [640.0 * SCALE, 64.0 * SCALE, -128.0 * SCALE]);
    }

    /// Quake II's `key_*` classnames spell the item, so one rule names the
    /// whole family the way Doom's `key_red` / `key_blue` are named.
    #[test]
    fn key_classnames_become_one_marker_family() {
        for (class, want) in [
            ("key_blue_key", "key_blue"),
            ("key_red_key", "key_red"),
            ("key_pyramid", "key_pyramid"),
            ("key_data_spinner", "key_data_spinner"),
            ("key_power_cube", "key_power_cube"),
            ("key_data_cd", "key_data_cd"),
            ("key_airstrike_target", "key_airstrike_target"),
            ("key_commander_head", "key_commander_head"),
            ("key_pass", "key_pass"),
            ("target_changelevel", "exit"),
        ] {
            assert_eq!(marker_name(class).as_deref(), Some(want), "{class}");
        }
        assert_eq!(marker_name("item_health").as_deref(), None);
        // `map "demo2$base1"` names a destination SPAWNPOINT, not a secret,
        // so no `exit_secret` is invented from it.
        assert_eq!(marker_name("target_secret").as_deref(), None);
    }

    /// A Quake II liquid's damage comes from its texture name — the face
    /// record carries no contents field — on the same scale Doom and Quake 1
    /// publish.
    #[test]
    fn liquid_damage_matches_the_classic_scale() {
        assert_eq!(liquid_damage("e1u1/lava1"), 20);
        assert_eq!(liquid_damage("e2u2/slime2"), 10);
        assert_eq!(liquid_damage("e1u1/bluwter"), 0);
        assert_eq!(liquid_damage("*water1"), 0);
        assert_eq!(liquid_flat("e1u1/bluwter"), "bluwter");
    }

    /// Extract named entries from a Quake PAK for the real-data tests.
    fn pak_entries(pak: &Path, wanted: &[&str]) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let Ok(bytes) = std::fs::read(pak) else {
            return out;
        };
        if bytes.len() < 12 || &bytes[0..4] != b"PACK" {
            return out;
        }
        let dirofs = u32_le(&bytes, 4) as usize;
        let dirlen = u32_le(&bytes, 8) as usize;
        if dirlen % 64 != 0 || dirofs.saturating_add(dirlen) > bytes.len() {
            return out;
        }
        for i in 0..dirlen / 64 {
            let off = dirofs + i * 64;
            let raw = &bytes[off..off + 56];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(56);
            let name = String::from_utf8_lossy(&raw[..end])
                .replace('\\', "/")
                .to_ascii_lowercase();
            let want = wanted
                .iter()
                .any(|w| name == *w || name.starts_with(w.trim_end_matches('*')));
            if !want {
                continue;
            }
            let fo = u32_le(&bytes, off + 56) as usize;
            let fl = u32_le(&bytes, off + 60) as usize;
            if fo.saturating_add(fl) <= bytes.len() {
                out.insert(name, bytes[fo..fo + fl].to_vec());
            }
        }
        out
    }

    /// Convert one map straight out of the local `baseq2/pak0.pak`, with the
    /// pak's own `env/` set and `textures/` bank behind it. `None` when the
    /// pak is not in this checkout, so the real-data tests skip silently.
    fn convert_pak_map(map: &str) -> Option<(PathBuf, PathBuf, Vec<ClassicAsset>)> {
        let pak = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/quake2/baseq2/pak0.pak");
        if !pak.is_file() {
            return None;
        }
        let rel = format!("maps/{map}.bsp");
        let files = pak_entries(&pak, &[&rel, "env/", "textures/"]);
        let bsp = files.get(&rel)?.clone();
        let root = tmp_dir(&format!("q2_{map}_src"));
        let staged = tmp_dir(&format!("q2_{map}_stage"));
        for (name, data) in &files {
            if *name == rel {
                continue;
            }
            let dest = root.join(name);
            if let Some(p) = dest.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(dest, data);
        }
        let bank = load_wal_bank(&root);
        let assets = convert_bsp38(&bsp, &rel, &staged, &bank, QUAKE2_SOURCE_ID)
            .unwrap_or_else(|e| panic!("convert {map}: {e}"));
        Some((root, staged, assets))
    }

    /// T-junctions left in `demo1` after conversion: **zero**, MEASURED
    /// 2026-08-20 over `local/packs/quake2/baseq2/pak0.pak`.
    ///
    /// `qbsp3` does run its own T-junction fix, so the BSP's own faces
    /// arrive welded — which is why nothing welded here for a long time.
    /// But this converter then re-cuts every face at its texture cell
    /// borders, because the tiles are packed into an atlas and a UV cannot
    /// wrap through one, and neighbouring faces do not agree about where
    /// those cuts fall: 6211 fresh cracks, all of this converter's own
    /// making. Doom, Quake 1 and Build weld after the same split, and now so
    /// does this. The number may fall, never rise.
    const Q2_DEMO1_T_JUNCTIONS: usize = 0;

    /// The retail/shareware `pak0.pak`: the whole contract over real data.
    /// Silently skipped when the pak is not in the checkout.
    #[test]
    fn local_quake2_demo1_exports_the_map_contract() {
        use makepad_asset_client::json::{self, Value};
        let Some((root, staged, assets)) = convert_pak_map("demo1") else {
            return;
        };
        let glb = std::fs::read(staged.join(&assets[0].rel_path)).unwrap();
        let js = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        let nodes = js.get("nodes").and_then(Value::as_arr).unwrap();
        let named = |prefix: &str| -> Vec<String> {
            nodes
                .iter()
                .filter_map(|n| n.get("name").and_then(Value::as_str))
                .filter(|n| n.starts_with(prefix))
                .map(String::from)
                .collect()
        };
        let doors = named("door_");
        let lifts = named("lift_");
        let hazards = named("hazard_");
        // demo1 ships four `func_door` brushes and one warped liquid.
        assert!(!doors.is_empty(), "doors: {doors:?}");
        assert!(!hazards.is_empty(), "hazards: {hazards:?}");
        // The sky: one equirect twin of `env/sky1*`, 1024x512.
        let sky = node(&js, "sky");
        assert_eq!(
            sky.get("extras")
                .and_then(|e| e.get("projection"))
                .and_then(Value::as_str),
            Some("cube")
        );
        assert!(
            !assets[0].tags.iter().any(|t| t == "sky-unmapped"),
            "demo1 ships env/sky1*: {:?}",
            assets[0].tags
        );
        let bin = glb_bin(&glb);
        let images = js.get("images").and_then(Value::as_arr).unwrap();
        let equirect = images
            .iter()
            .filter_map(|img| {
                let vi = img.get("bufferView").and_then(Value::as_i64)? as usize;
                let view = &js.get("bufferViews").and_then(Value::as_arr)?[vi];
                let off = view.get("byteOffset").and_then(Value::as_i64).unwrap_or(0) as usize;
                let len = view.get("byteLength").and_then(Value::as_i64)? as usize;
                let png = &bin[off..off + len];
                crate::classic_import::decode_png_stored(png)
                    .ok()
                    .map(|(rgba, w, h)| (rgba, w, h, png.to_vec()))
            })
            .find(|(_, w, h, _)| {
                (*w, *h) == (crate::skybox::EQUIRECT_W, crate::skybox::EQUIRECT_H)
            });
        assert!(equirect.is_some(), "the sky node carries a 1024x512 panorama");
        // The panorama is the one thing here that can be silently WRONG — a
        // quarter turn or a mirror still renders a sky — so leave it on disk
        // for a human to look at rather than only counting its pixels.
        if let Some((_, _, _, png)) = &equirect {
            let out = std::env::temp_dir().join("makepad-q2-demo1-sky.png");
            let _ = std::fs::write(&out, png);
            eprintln!("demo1 sky: {}", out.display());
        }

        // Anchors and the walker facts.
        let text = std::fs::read_to_string(staged.join("worlds/demo1.spawn")).unwrap();
        let nav = crate::world_nav::WorldNav::parse(&text).expect("spawn");
        let anchors: Vec<String> = nav.anchors().into_iter().map(|a| a.name).collect();
        assert!(anchors.contains(&"player_start".to_string()), "{anchors:?}");
        assert!(anchors.contains(&"exit".to_string()), "{anchors:?}");
        // demo1 has no `info_player_deathmatch` at all — id stripped the
        // deathmatch spots out of the shareware maps — so this reports the
        // count rather than demanding one.
        let dm = nav
            .starts
            .iter()
            .filter(|s| s.name.starts_with("deathmatch"))
            .count();

        // The weld runs over this converter's own atlas split (see
        // `Q2_DEMO1_T_JUNCTIONS`); this is the measurement, which may fall
        // but must not rise.
        let parts = crate::world_preview::extract_glb_parts(&glb).expect("parts");
        let soup: Vec<(&[[f32; 3]], &[u32])> = parts
            .iter()
            .map(|part| (&part.pos[..], &part.indices[..]))
            .collect();
        let left = crate::classic_import::weld_t_junctions_left(&soup);
        eprintln!(
            "demo1: {} parts, {} triangles, {} doors, {} lifts, {} hazards, \
             {} starts ({dm} deathmatch), {} markers, {left} T-junctions",
            parts.len(),
            soup.iter().map(|(_, i)| i.len() / 3).sum::<usize>(),
            doors.len(),
            lifts.len(),
            hazards.len(),
            nav.starts.len(),
            nav.markers.len(),
        );
        assert!(
            left <= Q2_DEMO1_T_JUNCTIONS,
            "demo1 T-junctions rose to {left} (ratchet {Q2_DEMO1_T_JUNCTIONS})"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);
    }

    /// demo1 has neither a `func_plat` nor a key, so the rest of the contract
    /// is gated on the two maps that do. Skipped with the pak, like demo1.
    #[test]
    fn local_quake2_demo2_and_demo3_export_their_lifts_and_keys() {
        use makepad_asset_client::json;
        let Some((root, staged, _)) = convert_pak_map("demo2") else {
            return;
        };
        let glb = std::fs::read(staged.join("worlds/demo2.glb")).unwrap();
        let js = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        // demo2's one `func_plat` has `lip 132` over a 320-unit brush: it
        // drops 188 units = 2.9375 m.
        let lift = node(&js, "lift_1");
        let extras = lift.get("extras").expect("lift extras");
        let travel = num(extras.get("travel").unwrap());
        assert!(
            (travel + (188.0 * SCALE) as f64).abs() < 1e-4,
            "plat travel {travel} (expected -2.9375, `lip 132` off a 320-unit brush)"
        );
        let nav = crate::world_nav::WorldNav::parse(
            &std::fs::read_to_string(staged.join("worlds/demo2.spawn")).unwrap(),
        )
        .expect("spawn");
        assert_eq!(nav.lifts.len(), 1, "{:?}", nav.lifts);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&staged);

        let Some((root, staged, _)) = convert_pak_map("demo3") else {
            return;
        };
        let nav = crate::world_nav::WorldNav::parse(
            &std::fs::read_to_string(staged.join("worlds/demo3.spawn")).unwrap(),
        )
        .expect("spawn");
        let markers: Vec<&str> = nav.markers.iter().map(|m| m.name.as_str()).collect();
        assert!(
            markers.contains(&"key_blue"),
            "demo3 ships a key_blue_key: {markers:?}"
        );
        assert!(markers.contains(&"exit"), "{markers:?}");
        assert_eq!(nav.lifts.len(), 2, "demo3 has two plats: {:?}", nav.lifts);
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
