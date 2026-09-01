//! Quake III Arena demo classic importer.
//!
//! Local-folder only. Converts original formats into catalog payloads — never
//! ships `.pk3` / `.bsp` / `.md3` as asset types:
//!
//! - PK3 (zip) → extracted files for the parsers below
//! - BSP `IBSP` version 46 → one `World` GLB (one primitive per shader, tiling UVs)
//! - MD3 → `Character` / `Weapon` / `Prop` GLB (first mesh, first frame)
//! - TGA (and stored PNG) → atlas tiles / `Texture` PNG
//! - WAV is copied by [`crate::classic_import`] after expand
//!
//! Official foothold (do **not** download from this crate):
//! - Demo: `linuxq3ademo-1.11-6.x86.gz` / `q3ademo.exe` — extract
//!   `demoq3/pak0.pk3`
//! - ioquake3 notes: https://ioquake3.org/help/players-guide/
//!   (“Using Demo Data Files”)
//! - Point-release paks 1–8 are separately downloadable from
//!   https://ioquake3.org/extras/patch-data/ AFTER agreeing to the license —
//!   optional overlay, not required
//! - Retail later: same parser on purchased `baseq3/pak0.pk3`
//! - OpenArena is a *different* art set — do **not** treat it as Q3 shareware.
//!   The parser may still read its pk3s because the format is the same.
//!
//! There is no Freedoom-equivalent that replaces Q3 retail art. Demo is the
//! foothold.

use crate::classic_import::{decode_png_stored, encode_png_rgba, ClassicAsset};
use crate::vertex_skin;
use makepad_asset_data::AssetKind;
use makepad_gltf::{
    write_glb_mesh_textured, write_glb_mesh_textured_parts, GlbTexturedMesh, GlbTexturedPart,
};
use makepad_zip_file::zip_read_central_directory;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

pub const QUAKE3_SOURCE_ID: &str = "quake3";
pub const QUAKE3_SOURCE_TITLE: &str = "Quake III Arena (demo)";
pub const QUAKE3_LICENSE: &str = "id-Software-demo";
pub const QUAKE3_HOME: &str = "https://ioquake3.org/";
pub const QUAKE3_CREDITS: &str = "id Software Quake III Arena demo";

/// Metres per Quake III unit: the 56-unit player (`playerMins`/`playerMaxs`
/// in `bg_pmove.c`, −24..32) stands `PERSON_HEIGHT` — 1/32 m, the same
/// number as Doom, Quake and Quake II. It was 1/64 (a 0.875 m Sarge) until
/// 2026-08-26.
const SCALE: f32 = crate::dimensions::PERSON_HEIGHT / 56.0;
/// Q3 `DEFAULT_VIEWHEIGHT` (standing).
const VIEW_HEIGHT: f32 = 26.0;
/// A spawn entity's origin sits this far above the floor (player bbox mins z).
const ORIGIN_ABOVE_FLOOR: f32 = 24.0;
/// Q3 `STEPSIZE`.
const STEP_HEIGHT: f32 = 18.0;

const LUMP_ENTITIES: usize = 0;
const LUMP_TEXTURES: usize = 1;
/// Brush models (`dmodel_t`): index 0 is the world, 1.. are the submodels a
/// `func_*` entity references as `"model" "*N"`.
const LUMP_MODELS: usize = 7;
const LUMP_VERTICES: usize = 10;
const LUMP_MESHVERTS: usize = 11;
const LUMP_FACES: usize = 13;
const LUMP_LIGHTMAPS: usize = 14;
const LUMP_COUNT: usize = 17;
const LIGHTMAP_W: usize = 128;
const LIGHTMAP_BYTES: usize = LIGHTMAP_W * LIGHTMAP_W * 3;

const TEX_SIZE: usize = 72;
const VERT_SIZE: usize = 44;
const FACE_SIZE: usize = 104;
/// `float mins[3]; float maxs[3]; int face; int n_faces; int brush; int n_brushes;`
const MODEL_SIZE: usize = 40;

const FACE_POLYGON: i32 = 1;
const FACE_PATCH: i32 = 2;
const FACE_MESH: i32 = 3;

const SURF_SKY: i32 = 0x4;
const SURF_NODRAW: i32 = 0x80;
const SURF_SKIP: i32 = 0x200;

// `contentFlags` of a shader lump entry (`q_shared.h`). Q3 marks a liquid on
// the shader, not on the face — a brush textured `lavahell` is lava.
const CONTENTS_LAVA: i32 = 0x8;
const CONTENTS_SLIME: i32 = 0x10;
const CONTENTS_WATER: i32 = 0x20;

/// Q3's default door/plat `lip`: how much of the brush stays showing when it
/// has finished travelling (`SP_func_door` / `SP_func_plat`, `g_mover.c`).
const Q3_MOVER_LIP: f32 = 8.0;

const MD3_IDENT: &[u8; 4] = b"IDP3";
const MD3_VERSION: i32 = 15;
const MD3_XYZ_SCALE: f32 = 1.0 / 64.0;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pk3File {
    pub path: PathBuf,
    /// PK3-internal path, e.g. `maps/q3dm1.bsp`.
    pub rel: String,
}

#[derive(Clone, Debug)]
pub struct Q3Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Q3TexBank {
    /// Lowercase path without extension (`textures/base_wall/concrete`).
    pub images: BTreeMap<String, Q3Image>,
    /// On-disk images not decoded yet (`dds/models/foo` → extracted file).
    pub files: BTreeMap<String, PathBuf>,
    /// Material / shader name → diffusemap path (from `.mtr`).
    pub aliases: BTreeMap<String, String>,
    /// Shader `detail` stage: high-frequency overlay + `tcMod scale`.
    pub details: BTreeMap<String, ShaderDetail>,
    /// Multi-stage look (alpha-over underlayer, additive fire).
    pub surfaces: BTreeMap<String, ShaderSurface>,
    /// Sky shader → `skyParms` FARBOX base path (`env/xnight2`). Only shaders
    /// that name a box are here: a `skyparms - 512 -` cloud-layer sky has no
    /// environment to resample and does not appear.
    pub sky_boxes: BTreeMap<String, String>,
}

/// Q3 / Unreal-style close-up overlay. Mean ~0.5 so `2 * dest * src` is identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderDetail {
    pub map: String,
    pub scale: [f32; 2],
}

/// Baked Q3 stage stack used when a single albedo PNG has to stand in
/// for `animMap` / `blendFunc` / scrolled underlayers.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderSurface {
    pub albedo: String,
    pub under: Option<String>,
    pub under_scale: [f32; 2],
    pub additive: bool,
    pub two_sided: bool,
}

impl Q3TexBank {
    pub fn get(&self, name: &str) -> Option<&Q3Image> {
        for key in candidate_tex_keys(name) {
            if let Some(img) = self.images.get(&key) {
                return Some(img);
            }
        }
        None
    }

    pub fn add_alias(&mut self, material: &str, map: &str) {
        let mat = normalize_tex_name(material);
        let map = normalize_tex_name(map);
        if mat.is_empty() || map.is_empty() {
            return;
        }
        self.aliases.entry(mat).or_insert(map);
    }

    pub fn extend_bank(&mut self, other: Q3TexBank) {
        self.images.extend(other.images);
        for (k, v) in other.files {
            self.files.entry(k).or_insert(v);
        }
        self.aliases.extend(other.aliases);
        self.details.extend(other.details);
        self.surfaces.extend(other.surfaces);
        self.sky_boxes.extend(other.sky_boxes);
    }

    /// The `skyParms` farbox base of a sky shader, if it named one.
    pub fn sky_box_for(&self, name: &str) -> Option<&String> {
        for key in candidate_tex_keys(name) {
            if let Some(b) = self.sky_boxes.get(&key) {
                return Some(b);
            }
        }
        None
    }

    pub fn detail_for(&self, name: &str) -> Option<&ShaderDetail> {
        for key in candidate_tex_keys(name) {
            if let Some(d) = self.details.get(&key) {
                return Some(d);
            }
        }
        None
    }

    pub fn surface_for(&self, name: &str) -> Option<&ShaderSurface> {
        for key in candidate_tex_keys(name) {
            if let Some(s) = self.surfaces.get(&key) {
                return Some(s);
            }
        }
        None
    }

    /// Albedo for a BSP shader: follows aliases, composites an alpha
    /// underlayer (blood / fire holes), and punches additive fire.
    pub fn bake(&self, name: &str) -> Option<Q3Image> {
        let surf = self.surface_for(name);
        let albedo_name = surf
            .map(|s| s.albedo.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(name);
        let mut img = self
            .resolve(albedo_name)
            .or_else(|| self.resolve(name))?;
        if let Some(s) = surf {
            if let Some(under_name) = s.under.as_ref() {
                if let Some(under) = self.resolve(under_name) {
                    img = composite_under(&img, &under, s.under_scale);
                } else {
                    force_opaque(&mut img);
                }
            }
            if s.additive {
                img = luma_to_alpha(img);
            }
        }
        Some(img)
    }

    /// Decoded image for a shader / material / file path. Follows `.mtr`
    /// aliases and lazily decodes indexed DDS/TGA/PNG files.
    pub fn resolve(&self, name: &str) -> Option<Q3Image> {
        let mut tried = BTreeSet::new();
        let mut queue = candidate_tex_keys(name);
        if let Some(mapped) = self.aliases.get(&normalize_tex_name(name)) {
            queue.extend(candidate_tex_keys(mapped));
        }
        // Front-to-back keeps the same priority order as `get` (exact
        // path first, bare-stem fallback last).
        let mut qi = 0usize;
        while qi < queue.len() {
            let key = queue[qi].clone();
            qi += 1;
            if !tried.insert(key.clone()) {
                continue;
            }
            if let Some(img) = self.images.get(&key) {
                return Some(img.clone());
            }
            if let Some(path) = self.files.get(&key) {
                if let Some(img) = decode_image_file(path) {
                    return Some(limit_image(img, 512));
                }
            }
            if let Some(mapped) = self.aliases.get(&key) {
                queue.extend(candidate_tex_keys(mapped));
            }
        }
        None
    }
}

fn candidate_tex_keys(name: &str) -> Vec<String> {
    let n = normalize_tex_name(name);
    if n.is_empty() {
        return Vec::new();
    }
    let mut out = vec![n.clone()];
    if let Some(rest) = n.strip_prefix("textures/") {
        out.push(rest.to_string());
    } else {
        out.push(format!("textures/{n}"));
    }
    if let Some(rest) = n.strip_prefix("dds/") {
        out.push(rest.to_string());
    } else {
        out.push(format!("dds/{n}"));
        if !n.starts_with("textures/") {
            out.push(format!("dds/textures/{n}"));
        }
    }
    if let Some(stem) = n.rsplit('/').next() {
        if stem != n {
            out.push(stem.to_string());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PK3 = zip
// ---------------------------------------------------------------------------

pub fn expand_pk3(
    path: &Path,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<Pk3File>, String> {
    let mut zip = std::fs::File::open(path).map_err(|e| format!("open pk3: {e}"))?;
    let dir = zip_read_central_directory(&mut zip)
        .map_err(|e| format!("pk3 central directory: {e:?}"))?;
    std::fs::create_dir_all(out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let mut out = Vec::new();
    for header in &dir.file_headers {
        let Some(rel) = safe_rel(&header.file_name) else {
            if !header.file_name.is_empty() && !header.file_name.ends_with('/') {
                warnings.push(format!("skip unsafe pk3 path {}", header.file_name));
            }
            continue;
        };
        match header.extract(&mut zip) {
            Ok(bytes) => {
                let dest = out_dir.join(&rel);
                if let Some(parent) = dest.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        warnings.push(format!("{rel}: {e}"));
                        continue;
                    }
                }
                if let Err(e) = std::fs::write(&dest, &bytes) {
                    warnings.push(format!("{rel}: {e}"));
                    continue;
                }
                out.push(Pk3File { path: dest, rel });
            }
            Err(e) => warnings.push(format!("{rel}: extract {e:?}")),
        }
    }
    Ok(out)
}

fn safe_rel(name: &str) -> Option<String> {
    let rel = name.replace('\\', "/");
    if rel.is_empty() || rel.ends_with('/') {
        return None;
    }
    if rel.starts_with('/') || rel.starts_with("\\") {
        return None;
    }
    if rel
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == ".." || c.contains(':'))
    {
        return None;
    }
    if rel.contains('\0') {
        return None;
    }
    Some(rel)
}

// ---------------------------------------------------------------------------
// Texture bank (TGA foothold; stored PNG accepted)
// ---------------------------------------------------------------------------

pub fn load_tex_bank(root: &Path) -> Q3TexBank {
    let mut bank = Q3TexBank::default();
    if !root.exists() {
        return bank;
    }
    let mut files = Vec::new();
    walk_files(root, &mut files, 0);
    files.sort_by_key(|p| tex_file_rank(p));
    for path in files {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "tga" | "png" | "dds" | "jpg" | "jpeg") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let key = normalize_tex_name(&rel);
        if key.is_empty() {
            continue;
        }
        index_tex_keys(&mut bank.files, &key, path.clone());
        // Quake 3 packs are small — decode TGA/PNG eagerly so BSP atlas
        // lookup stays a borrow. id Tech 4 DDS stays a path until resolve().
        if ext == "dds" {
            continue;
        }
        let decoded = match ext.as_str() {
            "tga" => std::fs::read(&path).ok().and_then(|b| decode_tga(&b).ok()),
            "png" => std::fs::read(&path)
                .ok()
                .and_then(|b| decode_png_stored(&b).ok()),
            "jpg" | "jpeg" => std::fs::read(&path).ok().and_then(|b| decode_jpeg(&b).ok()),
            _ => None,
        };
        let Some((rgba, w, h)) = decoded else {
            continue;
        };
        if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
            continue;
        }
        let img = Q3Image { w, h, rgba };
        insert_decoded(&mut bank.images, &key, img);
    }
    bank
}

/// `.shader` `map` / `qer_editorimage` → albedo path aliases.
pub fn apply_shader_aliases(bank: &mut Q3TexBank, root: &Path) {
    if !root.exists() {
        return;
    }
    let mut files = Vec::new();
    walk_files(root, &mut files, 0);
    for path in files {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "shader" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        parse_shader_aliases(&text, bank);
    }
}

#[derive(Clone, Debug)]
struct ShaderStage {
    map: String,
    additive: bool,
    alpha: bool,
    scale: [f32; 2],
}

fn parse_shader_aliases(text: &str, bank: &mut Q3TexBank) {
    let mut shader = String::new();
    let mut depth = 0i32;
    let mut editor = String::new();
    let mut stages: Vec<ShaderStage> = Vec::new();
    let mut detail_map = String::new();
    let mut detail_scale = [1.0f32, 1.0];
    let mut stage_map = String::new();
    let mut stage_detail = false;
    let mut stage_additive = false;
    let mut stage_alpha = false;
    let mut stage_scale = [1.0f32, 1.0];
    let mut cull_none = false;
    let flush = |shader: &str,
                 stages: &[ShaderStage],
                 editor: &str,
                 detail_map: &str,
                 detail_scale: [f32; 2],
                 cull_none: bool,
                 bank: &mut Q3TexBank| {
        if shader.is_empty() {
            return;
        }
        // Prefer the last non-additive surface (stone / lava / sky).
        // Additive-only stacks (animMap flame) fall back to the first frame.
        let albedo_idx = stages
            .iter()
            .rposition(|s| !s.additive)
            .or_else(|| stages.first().map(|_| 0));
        let albedo = albedo_idx
            .map(|i| stages[i].map.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(editor);
        if !albedo.is_empty() {
            bank.add_alias(shader, albedo);
        }
        if !detail_map.is_empty() {
            bank.details.insert(
                normalize_tex_name(shader),
                ShaderDetail {
                    map: normalize_tex_name(detail_map),
                    scale: detail_scale,
                },
            );
        }
        if let Some(i) = albedo_idx {
            let a = &stages[i];
            let mut surface = ShaderSurface {
                albedo: normalize_tex_name(&a.map),
                under: None,
                under_scale: [1.0, 1.0],
                additive: a.additive,
                two_sided: cull_none || a.additive,
            };
            if a.alpha {
                if let Some(under) = stages[..i].iter().rev().find(|s| !s.additive && !s.alpha) {
                    surface.under = Some(normalize_tex_name(&under.map));
                    surface.under_scale = under.scale;
                }
            }
            if surface.under.is_some() || surface.additive || surface.two_sided {
                bank.surfaces.insert(normalize_tex_name(shader), surface);
            }
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if depth == 0 && !line.starts_with('{') && !line.starts_with('}') {
            if !shader.is_empty() {
                flush(
                    &shader,
                    &stages,
                    &editor,
                    &detail_map,
                    detail_scale,
                    cull_none,
                    bank,
                );
            }
            shader = line.split_whitespace().next().unwrap_or("").to_string();
            editor.clear();
            stages.clear();
            detail_map.clear();
            detail_scale = [1.0, 1.0];
            cull_none = false;
            continue;
        }
        if line.starts_with('{') {
            depth += 1;
            if depth == 2 {
                stage_map.clear();
                stage_detail = false;
                stage_additive = false;
                stage_alpha = false;
                stage_scale = [1.0, 1.0];
            }
            continue;
        }
        if line.starts_with('}') {
            if depth == 2 {
                if stage_detail && !stage_map.is_empty() {
                    detail_map = stage_map.clone();
                    detail_scale = stage_scale;
                } else if !stage_map.is_empty() {
                    stages.push(ShaderStage {
                        map: stage_map.clone(),
                        additive: stage_additive,
                        alpha: stage_alpha,
                        scale: stage_scale,
                    });
                }
            }
            depth = (depth - 1).max(0);
            if depth == 0 {
                flush(
                    &shader,
                    &stages,
                    &editor,
                    &detail_map,
                    detail_scale,
                    cull_none,
                    bank,
                );
                shader.clear();
                editor.clear();
                stages.clear();
                detail_map.clear();
                detail_scale = [1.0, 1.0];
                cull_none = false;
            }
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if depth == 1 && lower.starts_with("qer_editorimage") {
            if let Some(p) = line.split_whitespace().nth(1) {
                editor = p.to_string();
            }
            continue;
        }
        // `skyParms <farbox> <cloudheight> <nearbox>`. Only the farbox names an
        // environment we can resample; `-`, a bare number and `full`/`half`
        // are cloud-layer skies with no box at all.
        if depth == 1 && lower.starts_with("skyparms") {
            if let Some(farbox) = line.split_whitespace().nth(1) {
                let f = farbox.to_ascii_lowercase();
                let placeholder =
                    f == "-" || f == "full" || f == "half" || f.parse::<f32>().is_ok();
                if !placeholder && !shader.is_empty() {
                    bank.sky_boxes
                        .insert(normalize_tex_name(&shader), normalize_tex_name(farbox));
                }
            }
            continue;
        }
        if depth == 1 && (lower == "cull none" || lower == "cull disable"
            || lower.starts_with("cull none")
            || lower.starts_with("cull disable"))
        {
            cull_none = true;
            continue;
        }
        if depth != 2 {
            continue;
        }
        if lower == "detail" || lower.starts_with("detail ") {
            stage_detail = true;
            continue;
        }
        if lower.starts_with("blendfunc") {
            let kind = classify_blend(&lower);
            stage_additive = kind == BlendKind::Additive;
            stage_alpha = kind == BlendKind::Alpha;
            continue;
        }
        if lower.starts_with("tcmod scale") || lower.starts_with("tcmod  scale") {
            let mut it = line.split_whitespace();
            let _ = it.next();
            let _ = it.next();
            let sx = it.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let sy = it.next().and_then(|s| s.parse().ok()).unwrap_or(sx);
            stage_scale = [sx, sy];
            continue;
        }
        if lower.starts_with("animmap ") {
            let mut it = line.split_whitespace();
            let _ = it.next();
            let _ = it.next();
            if let Some(p) = it.next() {
                let pl = p.to_ascii_lowercase();
                if !pl.starts_with('$') {
                    stage_map = p.to_string();
                }
            }
            continue;
        }
        if lower.starts_with("map ") || lower.starts_with("clampmap ") {
            if let Some(p) = line.split_whitespace().nth(1) {
                let pl = p.to_ascii_lowercase();
                if !pl.starts_with('$') {
                    stage_map = p.to_string();
                }
            }
        }
    }
    flush(
        &shader,
        &stages,
        &editor,
        &detail_map,
        detail_scale,
        cull_none,
        bank,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlendKind {
    Replace,
    Alpha,
    Additive,
    Other,
}

fn classify_blend(lower: &str) -> BlendKind {
    let mut it = lower.split_whitespace();
    let _ = it.next();
    let a = it.next().unwrap_or("");
    let b = it.next().unwrap_or("");
    if a == "add" || (a == "gl_one" && b == "gl_one") {
        return BlendKind::Additive;
    }
    if a == "blend"
        || (a == "gl_src_alpha" && b == "gl_one_minus_src_alpha")
        || (a.contains("src_alpha") && b.contains("one_minus_src_alpha"))
    {
        return BlendKind::Alpha;
    }
    if a == "gl_one" && b == "gl_zero" {
        return BlendKind::Replace;
    }
    BlendKind::Other
}

fn composite_under(over: &Q3Image, under: &Q3Image, scale: [f32; 2]) -> Q3Image {
    let (ow, oh) = (over.w as usize, over.h as usize);
    let (uw, uh) = (under.w as usize, under.h as usize);
    if ow == 0 || oh == 0 || uw == 0 || uh == 0 {
        let mut img = over.clone();
        force_opaque(&mut img);
        return img;
    }
    let sx = if scale[0].abs() < 1e-4 { 1.0 } else { scale[0] };
    let sy = if scale[1].abs() < 1e-4 { 1.0 } else { scale[1] };
    let mut rgba = over.rgba.clone();
    for y in 0..oh {
        for x in 0..ow {
            let i = (y * ow + x) * 4;
            let a = rgba[i + 3] as f32 / 255.0;
            if a >= 0.995 {
                rgba[i + 3] = 255;
                continue;
            }
            let ux = ((x as f32 + 0.5) / ow as f32 * sx * uw as f32).rem_euclid(uw as f32) as usize;
            let uy = ((y as f32 + 0.5) / oh as f32 * sy * uh as f32).rem_euclid(uh as f32) as usize;
            let ui = (uy.min(uh - 1) * uw + ux.min(uw - 1)) * 4;
            let ua = 1.0 - a;
            for c in 0..3 {
                let o = rgba[i + c] as f32;
                let u = under.rgba[ui + c] as f32;
                rgba[i + c] = (o * a + u * ua).round().clamp(0.0, 255.0) as u8;
            }
            rgba[i + 3] = 255;
        }
    }
    Q3Image {
        w: over.w,
        h: over.h,
        rgba,
    }
}

fn force_opaque(img: &mut Q3Image) {
    for px in img.rgba.chunks_exact_mut(4) {
        px[3] = 255;
    }
}

fn luma_to_alpha(mut img: Q3Image) -> Q3Image {
    // Official flame JPGs are mostly dim orange on black (mean ~15).
    // A raw luma alpha falls under the walker's tex.w < 0.5 discard
    // and the whole card vanishes. Keep any glow, punch only ink.
    for px in img.rgba.chunks_exact_mut(4) {
        let l = px[0].max(px[1]).max(px[2]);
        if l < 10 {
            px[3] = 0;
            continue;
        }
        if l < 80 {
            let g = 80.0 / l as f32;
            px[0] = ((px[0] as f32) * g).min(255.0) as u8;
            px[1] = ((px[1] as f32) * g).min(255.0) as u8;
            px[2] = ((px[2] as f32) * g).min(255.0) as u8;
        }
        px[3] = 255;
    }
    img
}

/// Catalog cards for unique world/model albedo images (not aux maps).
pub fn emit_texture_assets(
    bank: &Q3TexBank,
    staged: &Path,
    source_id: &str,
) -> Vec<ClassicAsset> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (key, img) in &bank.images {
        if is_aux_tex_name(key) || key == "_default" {
            continue;
        }
        // One card per unique bitmap. Alias keys (stem / no-prefix) share pixels.
        let sig = (
            img.w,
            img.h,
            img.rgba.len(),
            img.rgba.get(0..16).unwrap_or(&[]).to_vec(),
        );
        if !seen.insert(format!("{sig:?}")) {
            continue;
        }
        let slug = sanitize_slug(key);
        let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) else {
            continue;
        };
        let tkey = format!("textures/{slug}");
        let trel = format!("{tkey}.png");
        let dest = staged.join(&trel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if dest.exists() {
            out.push(ClassicAsset {
                key: tkey,
                kind: AssetKind::Texture,
                rel_path: trel,
                tags: vec!["texture".into(), source_id.into(), slug],
                icon_rel: None,
            });
            continue;
        }
        if std::fs::write(&dest, png).is_ok() {
            out.push(ClassicAsset {
                key: tkey,
                kind: AssetKind::Texture,
                rel_path: trel,
                tags: vec!["texture".into(), source_id.into(), slug],
                icon_rel: None,
            });
        }
    }
    out
}

fn tex_file_rank(path: &Path) -> u8 {
    let n = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    let aux = is_aux_tex_name(&n);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = match ext.as_str() {
        "dds" => 0,
        "png" => 1,
        "jpg" | "jpeg" => 2,
        "tga" => 3,
        _ => 5,
    };
    if aux {
        kind + 10
    } else {
        kind
    }
}

pub fn is_aux_tex_name(name: &str) -> bool {
    let stem = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .rsplit('.')
        .next()
        .unwrap_or(name);
    for suf in [
        "_local", "_local2", "_loc", "_s", "_spec", "_ed", "_h", "_height", "_n",
        "_norm", "_normal", "_bump", "_glow",
    ] {
        if stem.ends_with(suf) {
            return true;
        }
    }
    false
}

fn index_tex_keys(files: &mut BTreeMap<String, PathBuf>, key: &str, path: PathBuf) {
    files.entry(key.to_string()).or_insert_with(|| path.clone());
    if let Some(rest) = key.strip_prefix("dds/") {
        files.entry(rest.to_string()).or_insert_with(|| path.clone());
    }
    if let Some(rest) = key.strip_prefix("textures/") {
        files.entry(rest.to_string()).or_insert_with(|| path.clone());
    }
    if let Some(stem) = key.rsplit('/').next() {
        if !is_aux_tex_name(stem) {
            files.entry(stem.to_string()).or_insert(path);
        }
    }
}

fn insert_decoded(images: &mut BTreeMap<String, Q3Image>, key: &str, img: Q3Image) {
    images.entry(key.to_string()).or_insert_with(|| img.clone());
    if let Some(rest) = key.strip_prefix("textures/") {
        images.entry(rest.to_string()).or_insert_with(|| img.clone());
    }
    if let Some(stem) = key.rsplit('/').next() {
        if !is_aux_tex_name(stem) {
            images.entry(stem.to_string()).or_insert(img);
        }
    }
}

fn decode_image_file(path: &Path) -> Option<Q3Image> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = std::fs::read(path).ok()?;
    let (rgba, w, h) = match ext.as_str() {
        "tga" => decode_tga(&bytes).ok()?,
        "png" => decode_png_stored(&bytes).ok()?,
        "jpg" | "jpeg" => decode_jpeg(&bytes).ok()?,
        "dds" => decode_dds(&bytes).ok()?,
        _ => return None,
    };
    if w == 0 || h == 0 || rgba.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    Some(Q3Image { w, h, rgba })
}

fn limit_image(img: Q3Image, max_dim: u32) -> Q3Image {
    if img.w == 0 || img.h == 0 || img.w.max(img.h) <= max_dim {
        return img;
    }
    let scale = max_dim as f32 / img.w.max(img.h) as f32;
    let nw = ((img.w as f32 * scale).round() as u32).max(1);
    let nh = ((img.h as f32 * scale).round() as u32).max(1);
    let mut rgba = vec![0u8; nw as usize * nh as usize * 4];
    for y in 0..nh as usize {
        let sy = (y as u32 * img.h / nh) as usize;
        for x in 0..nw as usize {
            let sx = (x as u32 * img.w / nw) as usize;
            let si = (sy * img.w as usize + sx) * 4;
            let di = (y * nw as usize + x) * 4;
            if si + 4 <= img.rgba.len() {
                rgba[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
            }
        }
    }
    Q3Image {
        w: nw,
        h: nh,
        rgba,
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 24 || out.len() > 65_536 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_files(&path, out, depth + 1);
        } else {
            out.push(path);
        }
    }
}

fn normalize_tex_name(name: &str) -> String {
    let mut n = name.replace('\\', "/").to_ascii_lowercase();
    if let Some(stripped) = n.strip_prefix("./") {
        n = stripped.to_string();
    }
    for ext in [".tga", ".jpg", ".jpeg", ".png", ".pcx", ".bmp", ".dds"] {
        if let Some(stripped) = n.strip_suffix(ext) {
            n = stripped.to_string();
            break;
        }
    }
    while n.ends_with('/') {
        n.pop();
    }
    n
}

pub(crate) fn decode_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    use makepad_zune_jpeg::makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_jpeg::makepad_zune_core::colorspace::ColorSpace;
    use makepad_zune_jpeg::makepad_zune_core::options::DecoderOptions;
    use makepad_zune_jpeg::JpegDecoder;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|e| format!("jpeg headers: {e}"))?;
    let (w, h) = decoder.dimensions().ok_or("jpeg has no size")?;
    if w == 0 || h == 0 {
        return Err("jpeg empty".into());
    }
    let pixels = decoder.decode().map_err(|e| format!("jpeg decode: {e}"))?;
    if pixels.len() >= w * h * 4 {
        return Ok((pixels, w as u32, h as u32));
    }
    if pixels.len() >= w * h * 3 {
        let mut rgba = Vec::with_capacity(w * h * 4);
        for c in pixels.chunks_exact(3) {
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        return Ok((rgba, w as u32, h as u32));
    }
    Err("jpeg pixel count".into())
}

/// Uncompressed (and RLE) 24/32-bit TGA → RGBA, row 0 = top.
pub fn decode_tga(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.len() < 18 {
        return Err("tga header truncated".into());
    }
    let id_len = bytes[0] as usize;
    let cmap_type = bytes[1];
    let image_type = bytes[2];
    let cmap_len = u16_le(bytes, 5) as usize;
    let cmap_bpp = bytes[7] as usize;
    let w = u16_le(bytes, 12) as u32;
    let h = u16_le(bytes, 14) as u32;
    let bpp = bytes[16];
    let desc = bytes[17];
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("tga size".into());
    }
    if !matches!(image_type, 2 | 10) {
        return Err(format!("tga type {image_type} (need uncompressed/RLE truecolor)"));
    }
    if bpp != 24 && bpp != 32 {
        return Err(format!("tga bpp {bpp}"));
    }
    let mut off = 18 + id_len;
    if cmap_type != 0 {
        off = off.saturating_add(cmap_len.saturating_mul(cmap_bpp.div_ceil(8)));
    }
    if off > bytes.len() {
        return Err("tga colormap truncated".into());
    }
    let px_n = (w as usize).saturating_mul(h as usize);
    let src_bpp = (bpp / 8) as usize;
    let raw = match image_type {
        2 => {
            let need = px_n.saturating_mul(src_bpp);
            if off + need > bytes.len() {
                return Err("tga pixels truncated".into());
            }
            bytes[off..off + need].to_vec()
        }
        10 => decode_tga_rle(&bytes[off..], px_n, src_bpp)?,
        _ => unreachable!(),
    };
    let mut rgba = Vec::with_capacity(px_n * 4);
    for i in 0..px_n {
        let p = i * src_bpp;
        let b = raw[p];
        let g = raw[p + 1];
        let r = raw[p + 2];
        let a = if src_bpp == 4 { raw[p + 3] } else { 255 };
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    // Many Q3 32-bit TGAs store unused alpha as 0. Default shaders are
    // opaque; the walk viewer discards tex.w < 0.5, which deleted whole
    // arch frames (skullarch_a/c) and left only the soffit C.
    if src_bpp == 4 && rgba.chunks_exact(4).all(|p| p[3] == 0) {
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    let origin_top = desc & 0x20 != 0;
    if !origin_top {
        flip_rows(&mut rgba, w as usize, h as usize);
    }
    if desc & 0x10 != 0 {
        flip_cols(&mut rgba, w as usize, h as usize);
    }
    Ok((rgba, w, h))
}

fn decode_tga_rle(src: &[u8], px_n: usize, bpp: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(px_n * bpp);
    let mut i = 0usize;
    while out.len() / bpp < px_n {
        if i >= src.len() {
            return Err("tga rle truncated".into());
        }
        let packet = src[i];
        i += 1;
        let count = (packet & 0x7f) as usize + 1;
        if packet & 0x80 != 0 {
            if i + bpp > src.len() {
                return Err("tga rle run truncated".into());
            }
            for _ in 0..count {
                out.extend_from_slice(&src[i..i + bpp]);
            }
            i += bpp;
        } else {
            let n = count * bpp;
            if i + n > src.len() {
                return Err("tga rle raw truncated".into());
            }
            out.extend_from_slice(&src[i..i + n]);
            i += n;
        }
    }
    out.truncate(px_n * bpp);
    Ok(out)
}

fn flip_rows(rgba: &mut [u8], w: usize, h: usize) {
    let stride = w * 4;
    if stride == 0 {
        return;
    }
    for y in 0..h / 2 {
        let a = y * stride;
        let b = (h - 1 - y) * stride;
        for x in 0..stride {
            rgba.swap(a + x, b + x);
        }
    }
}

fn flip_cols(rgba: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        let row = y * w * 4;
        for x in 0..w / 2 {
            let a = row + x * 4;
            let b = row + (w - 1 - x) * 4;
            for k in 0..4 {
                rgba.swap(a + k, b + k);
            }
        }
    }
}

/// DXT1 / DXT3 / DXT5 / uncompressed RGBA. id Tech 4 stores albedos as DDS.
pub fn decode_dds(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    if bytes.len() < 128 || &bytes[0..4] != b"DDS " {
        return Err("not a DDS".into());
    }
    let height = u32_le(bytes, 12);
    let width = u32_le(bytes, 16);
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("dds size".into());
    }
    let fourcc = &bytes[84..88];
    let rgb_bits = u32_le(bytes, 88);
    let mut data_off = 128usize;
    let fmt = match fourcc {
        b"DXT1" => DdsFmt::Dxt1,
        b"DXT2" | b"DXT3" => DdsFmt::Dxt3,
        b"DXT4" | b"DXT5" => DdsFmt::Dxt5,
        b"DX10" => {
            if bytes.len() < 148 {
                return Err("dds dx10 header".into());
            }
            let dxgi = u32_le(bytes, 128);
            data_off = 148;
            match dxgi {
                71 | 72 => DdsFmt::Dxt1, // BC1
                74 | 75 => DdsFmt::Dxt3, // BC2
                77 | 78 => DdsFmt::Dxt5, // BC3
                28 | 87 => DdsFmt::Rgba8,
                _ => return Err(format!("dds dxgi {dxgi}")),
            }
        }
        b"\0\0\0\0" => {
            if rgb_bits == 32 {
                DdsFmt::Rgba8
            } else if rgb_bits == 24 {
                DdsFmt::Rgb8
            } else {
                return Err(format!("dds uncompressed {rgb_bits}bpp"));
            }
        }
        _ => return Err(format!("dds fourcc {}", String::from_utf8_lossy(fourcc))),
    };
    let w = width as usize;
    let h = height as usize;
    let mut rgba = vec![0u8; w * h * 4];
    match fmt {
        DdsFmt::Dxt1 => decode_dxt1(&bytes[data_off..], &mut rgba, w, h)?,
        DdsFmt::Dxt3 => decode_dxt3(&bytes[data_off..], &mut rgba, w, h)?,
        DdsFmt::Dxt5 => decode_dxt5(&bytes[data_off..], &mut rgba, w, h)?,
        DdsFmt::Rgba8 => {
            let need = w * h * 4;
            if data_off + need > bytes.len() {
                return Err("dds rgba truncated".into());
            }
            rgba.copy_from_slice(&bytes[data_off..data_off + need]);
        }
        DdsFmt::Rgb8 => {
            let need = w * h * 3;
            if data_off + need > bytes.len() {
                return Err("dds rgb truncated".into());
            }
            for i in 0..w * h {
                let s = data_off + i * 3;
                let d = i * 4;
                rgba[d] = bytes[s];
                rgba[d + 1] = bytes[s + 1];
                rgba[d + 2] = bytes[s + 2];
                rgba[d + 3] = 255;
            }
        }
    }
    Ok((rgba, width, height))
}

#[derive(Clone, Copy)]
enum DdsFmt {
    Dxt1,
    Dxt3,
    Dxt5,
    Rgba8,
    Rgb8,
}

fn decode_dxt1(src: &[u8], out: &mut [u8], w: usize, h: usize) -> Result<(), String> {
    let blocks_x = w.div_ceil(4);
    let blocks_y = h.div_ceil(4);
    let need = blocks_x * blocks_y * 8;
    if src.len() < need {
        return Err("dxt1 truncated".into());
    }
    let mut i = 0;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            blit_dxt_colors(&src[i..i + 8], out, w, h, bx * 4, by * 4, None);
            i += 8;
        }
    }
    Ok(())
}

fn decode_dxt3(src: &[u8], out: &mut [u8], w: usize, h: usize) -> Result<(), String> {
    let blocks_x = w.div_ceil(4);
    let blocks_y = h.div_ceil(4);
    let need = blocks_x * blocks_y * 16;
    if src.len() < need {
        return Err("dxt3 truncated".into());
    }
    let mut i = 0;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let mut alpha = [255u8; 16];
            for p in 0..16 {
                let nibble = src[i + p / 2] >> (4 * (p & 1));
                alpha[p] = (nibble & 0x0f) * 17;
            }
            blit_dxt_colors(&src[i + 8..i + 16], out, w, h, bx * 4, by * 4, Some(&alpha));
            i += 16;
        }
    }
    Ok(())
}

fn decode_dxt5(src: &[u8], out: &mut [u8], w: usize, h: usize) -> Result<(), String> {
    let blocks_x = w.div_ceil(4);
    let blocks_y = h.div_ceil(4);
    let need = blocks_x * blocks_y * 16;
    if src.len() < need {
        return Err("dxt5 truncated".into());
    }
    let mut i = 0;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let alpha = dxt5_alphas(&src[i..i + 8]);
            blit_dxt_colors(&src[i + 8..i + 16], out, w, h, bx * 4, by * 4, Some(&alpha));
            i += 16;
        }
    }
    Ok(())
}

fn dxt5_alphas(block: &[u8]) -> [u8; 16] {
    let a0 = block[0];
    let a1 = block[1];
    let mut table = [0u8; 8];
    table[0] = a0;
    table[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            table[i + 1] = ((u16::from(a0) * (7 - i as u16) + u16::from(a1) * i as u16) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            table[i + 1] = ((u16::from(a0) * (5 - i as u16) + u16::from(a1) * i as u16) / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let mut bits: u64 = 0;
    for (k, b) in block[2..8].iter().enumerate() {
        bits |= (*b as u64) << (8 * k);
    }
    let mut out = [0u8; 16];
    for p in 0..16 {
        out[p] = table[((bits >> (3 * p)) & 7) as usize];
    }
    out
}

fn blit_dxt_colors(
    block: &[u8],
    out: &mut [u8],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    alpha: Option<&[u8; 16]>,
) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let mut colors = [[0u8; 4]; 4];
    colors[0] = rgb565(c0);
    colors[1] = rgb565(c1);
    if c0 > c1 {
        colors[2] = lerp_rgb(colors[0], colors[1], 2, 1);
        colors[3] = lerp_rgb(colors[0], colors[1], 1, 2);
    } else {
        colors[2] = lerp_rgb(colors[0], colors[1], 1, 1);
        colors[3] = [0, 0, 0, 0];
    }
    let idx = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for py in 0..4 {
        for px in 0..4 {
            let x = x0 + px;
            let y = y0 + py;
            if x >= w || y >= h {
                continue;
            }
            let pi = py * 4 + px;
            let ci = ((idx >> (2 * pi)) & 3) as usize;
            let mut px4 = colors[ci];
            if let Some(a) = alpha {
                px4[3] = a[pi];
            }
            let o = (y * w + x) * 4;
            out[o..o + 4].copy_from_slice(&px4);
        }
    }
}

fn rgb565(c: u16) -> [u8; 4] {
    let r = ((c >> 11) & 0x1f) as u8;
    let g = ((c >> 5) & 0x3f) as u8;
    let b = (c & 0x1f) as u8;
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2, 255]
}

fn lerp_rgb(a: [u8; 4], b: [u8; 4], wa: u16, wb: u16) -> [u8; 4] {
    let d = wa + wb;
    [
        ((u16::from(a[0]) * wa + u16::from(b[0]) * wb) / d) as u8,
        ((u16::from(a[1]) * wa + u16::from(b[1]) * wb) / d) as u8,
        ((u16::from(a[2]) * wa + u16::from(b[2]) * wb) / d) as u8,
        255,
    ]
}

// ---------------------------------------------------------------------------
// BSP 46
// ---------------------------------------------------------------------------

pub fn convert_bsp46(
    bytes: &[u8],
    rel: &str,
    staged: &Path,
    textures: &Q3TexBank,
    source_id: &str,
) -> Result<Vec<ClassicAsset>, String> {
    let world = bsp46_to_glb(bytes, textures)?;
    let slug = stem_slug(rel);
    for w in &world.warnings {
        eprintln!("[quake3-import] {slug}: {w}");
    }
    // Classic Quake maps — splash game worlds stay `worlds/`.
    let key = format!("maps/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &world.glb).map_err(|e| e.to_string())?;
    let nav = &world.nav;
    if !nav.starts.is_empty()
        || !nav.doors.is_empty()
        || !nav.lifts.is_empty()
        || !nav.teleports.is_empty()
    {
        write_spawn_sidecar(&dest, nav);
    }
    {
        let place = bsp46_place(&entities_of(bytes), source_id, &key);
        let _ = crate::world_place::write_place_sidecar(&dest, &place);
    }
    let icon_rel = crate::world_preview::write_spawn_preview(&dest)
        .ok()
        .map(|_| format!("{key}.png"));
    let mut assets = vec![ClassicAsset {
        key,
        kind: AssetKind::World,
        rel_path,
        tags: vec![
            "map".into(),
            source_id.into(),
            "bsp46".into(),
            slug.clone(),
            "no-portals".into(),
        ],
        icon_rel,
    }];
    let mut seen = BTreeSet::new();
    for name in world.used {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(img) = textures.get(&name) else {
            continue;
        };
        let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) else {
            continue;
        };
        let tslug = sanitize_slug(&name);
        let tkey = format!("textures/{tslug}");
        let trel = format!("{tkey}.png");
        let tdest = staged.join(&trel);
        if let Some(parent) = tdest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&tdest, png).is_ok() {
            assets.push(ClassicAsset {
                key: tkey,
                kind: AssetKind::Texture,
                rel_path: trel,
                tags: vec![
                    "texture".into(),
                    source_id.into(),
                    "shader".into(),
                    tslug,
                ],
                icon_rel: None,
            });
        }
    }
    Ok(assets)
}

/// A converted Quake III map: the GLB, what a walker needs to stand in it,
/// the shaders it used, and every fact the BSP could not express.
struct Bsp46World {
    glb: Vec<u8>,
    nav: crate::world_nav::WorldNav,
    used: Vec<String>,
    warnings: Vec<String>,
}

fn bsp46_to_glb(bytes: &[u8], textures: &Q3TexBank) -> Result<Bsp46World, String> {
    if bytes.len() < 8 + LUMP_COUNT * 8 {
        return Err("BSP too small".into());
    }
    if &bytes[0..4] != b"IBSP" {
        return Err("not an IBSP".into());
    }
    let version = i32_le(bytes, 4);
    if version != 46 {
        return Err(format!("unsupported BSP version {version}"));
    }

    let tex_lump = lump(bytes, LUMP_TEXTURES)?;
    let vert_lump = lump(bytes, LUMP_VERTICES)?;
    let idx_lump = lump(bytes, LUMP_MESHVERTS)?;
    let face_lump = lump(bytes, LUMP_FACES)?;

    let n_tex = tex_lump.1 / TEX_SIZE;
    let n_vert = vert_lump.1 / VERT_SIZE;
    let n_idx = idx_lump.1 / 4;
    let n_face = face_lump.1 / FACE_SIZE;

    let mut shaders: Vec<Q3Shader> = Vec::with_capacity(n_tex);
    for i in 0..n_tex {
        let o = tex_lump.0 + i * TEX_SIZE;
        shaders.push(Q3Shader {
            name: cstr(&bytes[o..o + 64]),
            surface: i32_le(bytes, o + 64),
            contents: i32_le(bytes, o + 68),
        });
    }

    let mut verts = Vec::with_capacity(n_vert);
    for i in 0..n_vert {
        let o = vert_lump.0 + i * VERT_SIZE;
        verts.push(DrawVert {
            pos: [
                f32_le(bytes, o),
                f32_le(bytes, o + 4),
                f32_le(bytes, o + 8),
            ],
            st: [f32_le(bytes, o + 12), f32_le(bytes, o + 16)],
            lmst: [f32_le(bytes, o + 20), f32_le(bytes, o + 24)],
            normal: [
                f32_le(bytes, o + 28),
                f32_le(bytes, o + 32),
                f32_le(bytes, o + 36),
            ],
            color: [
                bytes.get(o + 40).copied().unwrap_or(255) as f32 / 255.0,
                bytes.get(o + 41).copied().unwrap_or(255) as f32 / 255.0,
                bytes.get(o + 42).copied().unwrap_or(255) as f32 / 255.0,
            ],
        });
    }

    let mut meshverts = Vec::with_capacity(n_idx);
    for i in 0..n_idx {
        meshverts.push(i32_le(bytes, idx_lump.0 + i * 4));
    }

    let lm_bytes = lump(bytes, LUMP_LIGHTMAPS).ok();
    let lm_atlas = pack_lightmap_atlas(bytes, lm_bytes);

    let mut parts: BTreeMap<String, PartGeom> = BTreeMap::new();
    let mut used = Vec::new();
    let mut seen = BTreeSet::new();

    // What leaves the static mesh, and why.
    let entities = entities_of(bytes);
    let models = bsp46_models(bytes);
    let (movers, mut warnings) = bsp46_movers(&entities, &models);
    let mut mover_of_face: BTreeMap<usize, usize> = BTreeMap::new();
    for (mi, m) in movers.iter().enumerate() {
        for f in m.first_face..m.first_face + m.num_faces {
            mover_of_face.insert(f, mi);
        }
    }
    let mut mover_geom: Vec<MoverGeom> = vec![MoverGeom::default(); movers.len()];
    let mut hazard_geom: BTreeMap<String, PartGeom> = BTreeMap::new();
    // The sky only leaves the mesh when its picture can be BUILT: a
    // `skyParms` cloud-layer sky has no environment to resample, and a hole
    // where the lid was is worse than the lid.
    let (sky_png, sky_shader) = bsp46_sky_panorama(&shaders, textures, &mut warnings);
    let mut sky_geom = PartGeom::default();

    for fi in 0..n_face {
        let o = face_lump.0 + fi * FACE_SIZE;
        let tex = i32_le(bytes, o);
        let typ = i32_le(bytes, o + 8);
        let first_vert = i32_le(bytes, o + 12);
        let n_vertexes = i32_le(bytes, o + 16);
        let first_idx = i32_le(bytes, o + 20);
        let n_meshverts = i32_le(bytes, o + 24);
        let lm_num = i32_le(bytes, o + 28);
        let patch_w = i32_le(bytes, o + 96);
        let patch_h = i32_le(bytes, o + 100);
        let shader = match shaders.get(tex as usize) {
            Some(t) => t,
            None => continue,
        };
        let name = &shader.name;
        if skip_shader(name, shader.surface) {
            continue;
        }
        if typ != FACE_POLYGON && typ != FACE_MESH && typ != FACE_PATCH {
            continue;
        }
        if seen.insert(name.clone()) {
            used.push(name.clone());
        }
        // Sky first (it is never a mover), then the movers, then the liquids,
        // then the level itself.
        let geom = if sky_png.is_some() && shader.surface & SURF_SKY != 0 {
            &mut sky_geom
        } else if let Some(&mi) = mover_of_face.get(&fi) {
            let m = &mut mover_geom[mi];
            *m.shaders.entry(name.clone()).or_insert(0) += 1;
            &mut m.geom
        } else if shader.liquid() {
            hazard_geom.entry(name.clone()).or_default()
        } else {
            parts.entry(name.clone()).or_default()
        };
        match typ {
            FACE_POLYGON | FACE_MESH => emit_indexed_face(
                geom,
                &verts,
                &meshverts,
                first_vert,
                n_vertexes,
                first_idx,
                n_meshverts,
                lm_num,
                lm_atlas.as_ref(),
            ),
            FACE_PATCH => {
                // q3map already triangulated some degenerate Bevel nets
                // for the lightmap. Prefer that fill when it exists.
                if n_meshverts >= 3 {
                    emit_indexed_face(
                        geom,
                        &verts,
                        &meshverts,
                        first_vert,
                        n_vertexes,
                        first_idx,
                        n_meshverts,
                        lm_num,
                        lm_atlas.as_ref(),
                    );
                } else {
                    emit_patch(
                        geom,
                        &verts,
                        first_vert,
                        n_vertexes,
                        patch_w,
                        patch_h,
                        lm_num,
                        lm_atlas.as_ref(),
                    );
                }
            }
            _ => {}
        }
    }

    for (name, geom) in &mut parts {
        let two = textures
            .surface_for(name)
            .map(|s| s.two_sided)
            .unwrap_or(false);
        if two && geom.indices.len() >= 3 {
            let extra: Vec<u32> = geom
                .indices
                .chunks_exact(3)
                .flat_map(|t| [t[0], t[2], t[1]])
                .collect();
            geom.indices.extend(extra);
        }
    }

    let gray = gray_image(64, 64);
    let gray_png = encode_png_rgba(&gray.rgba, gray.w, gray.h).unwrap_or_default();
    let mut pngs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut detail_pngs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut detail_scales: BTreeMap<String, [f32; 2]> = BTreeMap::new();
    let mut glb_parts: Vec<GlbTexturedPart<'_>> = Vec::new();
    let lm_png = lm_atlas.as_ref().map(|a| a.png.as_slice());
    // Keep PNG bytes alive for the writer borrow.
    for name in parts.keys() {
        if let Some(img) = textures.bake(name) {
            if let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) {
                pngs.insert(name.clone(), png);
            } else {
                pngs.insert(name.clone(), gray_png.clone());
            }
        } else {
            pngs.insert(name.clone(), gray_png.clone());
        }
        if let Some(det) = textures.detail_for(name) {
            if let Some(img) = textures.resolve(&det.map) {
                if let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) {
                    detail_pngs.insert(name.clone(), png);
                    detail_scales.insert(name.clone(), det.scale);
                }
            }
        }
    }
    // Weld the T-junctions this converter's own tessellation leaves behind.
    //
    // `q3map` fixes the ones between brush faces, which is why nothing
    // welded here for a long time. But a Q3 map is not only brushwork: this
    // converter subdivides every patch itself, and where a curve meets the
    // flat wall it abuts, neither side knows where the other cut. q3dm1
    // came out of a pre-welded BSP with 668 cracks. Merge first (a pair of
    // corners a hair apart poisons each other's cuts, so the splitter
    // declines them), then split, over the world parts, the sky, the
    // liquids and every mover from one grid — the cracks that show up most
    // are the ones BETWEEN parts.
    {
        let merge = {
            let mut all: Vec<&[[f32; 3]]> = parts.values().map(|g| &g.positions[..]).collect();
            all.push(&sky_geom.positions[..]);
            all.extend(hazard_geom.values().map(|g| &g.positions[..]));
            all.extend(mover_geom.iter().map(|m| &m.geom.positions[..]));
            crate::classic_import::merge_near_corners(&all)
        };
        let weld_one = |geom: &mut PartGeom, pass: &dyn Fn(crate::classic_import::WeldSoup)| {
            let n = geom.positions.len();
            let mut normals = std::mem::take(&mut geom.normals);
            let mut colors = std::mem::take(&mut geom.colors);
            let has_normals = normals.len() == n;
            let has_colors = colors.len() == n;
            pass(crate::classic_import::WeldSoup {
                positions: &mut geom.positions,
                uvs: &mut geom.uvs,
                normals: has_normals.then_some(&mut normals),
                colors: has_colors.then_some(&mut colors),
                indices: &mut geom.indices,
            });
            geom.normals = normals;
            geom.colors = colors;
            // The lightmap UVs are a second stream the weld does not know
            // about; a split would leave them the wrong length, and a
            // length mismatch is what the writer checks. Rebuild them to
            // the marker form rather than ship a half-updated stream.
            if geom.lm_uvs.len() != geom.positions.len() {
                geom.lm_uvs = vec![[0.0, 0.0]; geom.positions.len()];
                geom.own_marker = true;
            }
        };
        if !merge.is_empty() {
            let apply = |soup: crate::classic_import::WeldSoup| {
                merge.apply(soup);
            };
            for geom in parts
                .values_mut()
                .chain(std::iter::once(&mut sky_geom))
                .chain(hazard_geom.values_mut())
                .chain(mover_geom.iter_mut().map(|m| &mut m.geom))
            {
                weld_one(geom, &apply);
            }
        }
        let weld = {
            let mut all: Vec<&[[f32; 3]]> = parts.values().map(|g| &g.positions[..]).collect();
            all.push(&sky_geom.positions[..]);
            all.extend(hazard_geom.values().map(|g| &g.positions[..]));
            all.extend(mover_geom.iter().map(|m| &m.geom.positions[..]));
            crate::classic_import::weld_parts(&all)
        };
        let split = |soup: crate::classic_import::WeldSoup| {
            weld.split(soup);
        };
        for geom in parts
            .values_mut()
            .chain(std::iter::once(&mut sky_geom))
            .chain(hazard_geom.values_mut())
            .chain(mover_geom.iter_mut().map(|m| &mut m.geom))
        {
            weld_one(geom, &split);
        }
    }
    // A Q3 level is PRELIT: its light is already in COLOR_0, from the
    // lightmap atlas where there is one and from the drawvert colours where
    // there is not. The `lightmapTexture` marker is what tells the renderer
    // so; without it the analytic sun multiplies an already-lit level. A
    // vertex-lit-only map has no atlas to point at, so — exactly as the Doom
    // converter does — the marker points back at the part's own albedo with
    // zero UVs, which a prelit shader never samples.
    for geom in parts.values_mut() {
        let has_lm = lm_png.is_some() && geom.lm_uvs.iter().any(|uv| uv[0] >= 0.0);
        if !has_lm {
            geom.lm_uvs = vec![[0.0, 0.0]; geom.positions.len()];
            geom.own_marker = true;
        }
    }
    for (name, geom) in &parts {
        if geom.indices.len() < 3 {
            continue;
        }
        let png = pngs.get(name).unwrap_or(&gray_png);
        let detail = detail_pngs.get(name).map(|p| p.as_slice());
        let dscale = detail_scales.get(name).copied().unwrap_or([0.0, 0.0]);
        glb_parts.push(GlbTexturedPart {
            positions: &geom.positions,
            uvs: &geom.uvs,
            indices: &geom.indices,
            base_color_png: png,
            normals: Some(&geom.normals),
            base_color_factor: None,
            colors: Some(&geom.colors),
            lightmap_png: Some(if geom.own_marker { png } else { lm_png.unwrap_or(png) }),
            lightmap_uvs: Some(&geom.lm_uvs),
            detail_png: detail,
            detail_scale: dscale,
        });
    }

    let glb = if glb_parts.is_empty() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        let uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let indices = vec![0, 1, 2, 0, 2, 3];
        write_glb_mesh_textured(&GlbTexturedMesh {
            positions: &positions,
            normals: None,
            uvs: &uvs,
            indices: &indices,
            base_color_png: &gray_png,
            metallic_roughness_png: None,
            double_sided: true,
            colors: None,
        })
    } else {
        write_glb_mesh_textured_parts(&glb_parts, true)
    };
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }

    // ---- the contract nodes -------------------------------------------
    //
    // `inject_nodes` paints an added node with the LEVEL's material 0 unless
    // the node brings its own image. Quake III already writes one material
    // per shader, so material 0 is whichever shader sorted first — a door
    // repainted with it would be visibly wrong. Every node here therefore
    // carries its own image.
    //
    // A node has ONE mesh with ONE material, and a Q3 face keeps its raw
    // TILING st (values far outside 0..1, drawn with GL_REPEAT). Packing a
    // per-node atlas would have to clamp those, so a door's wall would stop
    // tiling: the atlas is not an option here, and the honest choice is one
    // image per node. Measured on the demo pack, every `func_door` is a
    // single shader (q3dm7 `*5` 12 faces / 1 shader, `*6` 78 faces / 1
    // shader), so this repaints nothing in practice; a door that does span
    // shaders takes its most-used one and says so.
    let mut extra: Vec<crate::glb_nodes::ExtraNode> = Vec::new();
    for (mi, mover) in movers.iter().enumerate() {
        let m = &mover_geom[mi];
        if m.geom.indices.len() < 3 {
            continue;
        }
        let shader = m
            .shaders
            .iter()
            .max_by_key(|(name, n)| (**n, std::cmp::Reverse(name.as_str())))
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        if m.shaders.len() > 1 {
            warnings.push(format!(
                "{} spans {} shaders; painted with `{shader}`",
                mover.name,
                m.shaders.len()
            ));
        }
        let png = shader_png(textures, &shader, &gray_png);
        // A `start_open` door is baked OPEN; the contract authors it CLOSED.
        let positions: Vec<[f32; 3]> = if mover.shift == [0.0; 3] {
            m.geom.positions.clone()
        } else {
            m.geom
                .positions
                .iter()
                .map(|p| {
                    [
                        p[0] + mover.shift[0],
                        p[1] + mover.shift[1],
                        p[2] + mover.shift[2],
                    ]
                })
                .collect()
        };
        let mut node = match mover.kind {
            MoverKind::Door => crate::glb_nodes::ExtraNode::door_vector(
                mover.name.clone(),
                positions,
                m.geom.uvs.clone(),
                m.geom.indices.clone(),
                mover.travel,
                mover.axis,
            ),
            MoverKind::Lift => crate::glb_nodes::ExtraNode::lift(
                mover.name.clone(),
                positions,
                m.geom.uvs.clone(),
                m.geom.indices.clone(),
                m.geom.colors.clone(),
                mover.top_y,
                mover.top_y + mover.travel[1],
            ),
        };
        // Baked light travels with the geometry, like the level mesh's:
        // `door_vector` has no colour argument, so it is set here.
        node.colors = m.geom.colors.clone();
        node.images = vec![png];
        extra.push(node);
    }
    for (n, (name, geom)) in hazard_geom.iter().enumerate() {
        if geom.indices.len() < 3 {
            continue;
        }
        let stem = name.rsplit('/').next().unwrap_or(name);
        let mut node = crate::glb_nodes::ExtraNode::hazard(
            format!("hazard_{}", n + 1),
            geom.positions.clone(),
            geom.uvs.clone(),
            geom.indices.clone(),
            geom.colors.clone(),
            q3_liquid_damage(name),
            stem,
            true,
            // A Q3 liquid is a volume you swim through, not a floor you stand
            // on — the surface must not stop a walker.
            false,
        );
        node.images = vec![shader_png(textures, name, &gray_png)];
        extra.push(node);
    }
    if let (Some(png), false) = (sky_png, sky_geom.indices.len() < 3) {
        extra.push(crate::glb_nodes::ExtraNode::sky(
            std::mem::take(&mut sky_geom.positions),
            std::mem::take(&mut sky_geom.uvs),
            std::mem::take(&mut sky_geom.indices),
            vec![png],
            // The renderer has no cube sampler: `cube` is sampled as the
            // equirect twin `crate::skybox` just built, one image, no wrap
            // multiplier and no phase offset.
            "cube",
            1.0,
            0.0,
            &sky_shader,
            None,
            None,
        ));
    }
    let glb = match crate::glb_nodes::inject_nodes(&glb, &extra) {
        Ok(g) => g,
        Err(e) => {
            warnings.push(format!("contract nodes not injected: {e}"));
            glb
        }
    };

    let (nav, nav_warnings) = bsp46_nav(&entities, &models, &movers);
    warnings.extend(nav_warnings);
    Ok(Bsp46World {
        glb,
        nav,
        used,
        warnings,
    })
}

/// A shader lump entry: its name and the two flag words q3map wrote.
#[derive(Clone, Debug)]
struct Q3Shader {
    name: String,
    surface: i32,
    contents: i32,
}

impl Q3Shader {
    /// Quake III marks a liquid on the shader's CONTENTS, not on the face.
    fn liquid(&self) -> bool {
        self.contents & (CONTENTS_LAVA | CONTENTS_SLIME | CONTENTS_WATER) != 0
    }
}

/// What a Q3 liquid does to a swimmer, on the same percent-per-second scale
/// the Doom and Quake 1 hazards use: lava is lethal, slime hurts, water is
/// harmless. Q3 itself deals 30/10/0 per second in `G_CheckWaterEvents`;
/// these are the shared classic numbers so one walker reads them all.
fn q3_liquid_damage(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n.contains("lava") {
        20
    } else if n.contains("slime") {
        10
    } else {
        0
    }
}

/// The albedo PNG of one shader, falling back to the level's gray.
fn shader_png(textures: &Q3TexBank, name: &str, gray_png: &[u8]) -> Vec<u8> {
    textures
        .bake(name)
        .and_then(|img| encode_png_rgba(&img.rgba, img.w, img.h).ok())
        .unwrap_or_else(|| gray_png.to_vec())
}

/// The sky's equirect panorama, if the map's sky shader names a `skyParms`
/// farbox whose six `env/` faces are all in the pack.
///
/// Returns the PNG and the sky shader's own name. Without one, the caller
/// leaves those faces in the level mesh: Q3 draws a `q3map_sun` cloud-layer
/// sky on the same brushes, and a hole where the lid was would be a worse lie
/// than the lid.
fn bsp46_sky_panorama(
    shaders: &[Q3Shader],
    textures: &Q3TexBank,
    warnings: &mut Vec<String>,
) -> (Option<Vec<u8>>, String) {
    let sky: Vec<&Q3Shader> = shaders.iter().filter(|s| s.surface & SURF_SKY != 0).collect();
    if sky.is_empty() {
        return (None, String::new());
    }
    for s in &sky {
        let Some(base) = textures.sky_box_for(&s.name) else {
            warnings.push(format!(
                "sky shader `{}` has no skyParms farbox (cloud-layer sky) — \
                 its faces stay opaque geometry",
                s.name
            ));
            continue;
        };
        let mut faces: [Option<crate::skybox::CubeFace>; 6] = Default::default();
        let mut missing = Vec::new();
        for (i, suffix) in crate::skybox::FACE_SUFFIXES.iter().enumerate() {
            match textures.resolve(&format!("{base}_{suffix}")) {
                Some(img) => faces[i] = crate::skybox::CubeFace::new(img.w, img.h, img.rgba),
                None => missing.push(*suffix),
            }
        }
        match crate::skybox::cube_to_equirect_png(&faces) {
            Some(png) => {
                if !missing.is_empty() {
                    warnings.push(format!(
                        "sky box `{base}` is missing {} — those directions are black",
                        missing.join("/")
                    ));
                }
                return (Some(png), s.name.clone());
            }
            None => warnings.push(format!(
                "sky box `{base}` is missing {} — its faces stay opaque geometry",
                if missing.is_empty() {
                    "usable faces".to_string()
                } else {
                    missing.join("/")
                }
            )),
        }
    }
    (None, String::new())
}

#[derive(Clone, Default)]
struct PartGeom {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 3]>,
    lm_uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    /// The part carries no real lightmap, so its prelit marker points back at
    /// its own albedo (see the writer loop).
    own_marker: bool,
}

/// One mover's geometry plus which shaders its faces used, so the node can be
/// painted with the one that covers most of it.
#[derive(Clone, Default)]
struct MoverGeom {
    geom: PartGeom,
    shaders: BTreeMap<String, usize>,
}

struct LightmapAtlas {
    png: Vec<u8>,
    cols: u32,
    rows: u32,
    rgba: Vec<u8>,
    w: u32,
    h: u32,
}

fn pack_lightmap_atlas(bytes: &[u8], lm: Option<(usize, usize)>) -> Option<LightmapAtlas> {
    let (off, len) = lm?;
    if len < LIGHTMAP_BYTES {
        return None;
    }
    let pages = (len / LIGHTMAP_BYTES).max(1);
    let cols = (pages as f32).sqrt().ceil() as u32;
    let rows = pages.div_ceil(cols as usize) as u32;
    let w = cols * LIGHTMAP_W as u32;
    let h = rows * LIGHTMAP_W as u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for p in 0..pages {
        let src = off + p * LIGHTMAP_BYTES;
        if src + LIGHTMAP_BYTES > bytes.len() {
            break;
        }
        let col = (p as u32) % cols;
        let row = (p as u32) / cols;
        for y in 0..LIGHTMAP_W {
            for x in 0..LIGHTMAP_W {
                let si = src + (y * LIGHTMAP_W + x) * 3;
                let dx = col as usize * LIGHTMAP_W + x;
                let dy = row as usize * LIGHTMAP_W + y;
                let di = (dy * w as usize + dx) * 4;
                rgba[di] = bytes[si];
                rgba[di + 1] = bytes[si + 1];
                rgba[di + 2] = bytes[si + 2];
                rgba[di + 3] = 255;
            }
        }
    }
    let png = encode_png_rgba(&rgba, w, h).ok()?;
    Some(LightmapAtlas {
        png,
        cols,
        rows,
        rgba,
        w,
        h,
    })
}

fn atlas_lm_uv(atlas: &LightmapAtlas, lm_num: i32, st: [f32; 2]) -> [f32; 2] {
    if lm_num < 0 {
        return [-1.0, -1.0];
    }
    let page = lm_num as u32;
    let col = page % atlas.cols;
    let row = page / atlas.cols;
    let u = (col as f32 + st[0].clamp(0.0, 1.0)) / atlas.cols as f32;
    let v = (row as f32 + st[1].clamp(0.0, 1.0)) / atlas.rows as f32;
    [u, v]
}

fn sample_atlas(atlas: &LightmapAtlas, uv: [f32; 2]) -> [f32; 3] {
    if uv[0] < 0.0 {
        return [1.0, 1.0, 1.0];
    }
    let x = ((uv[0] * (atlas.w - 1) as f32).round() as u32).min(atlas.w - 1);
    let y = ((uv[1] * (atlas.h - 1) as f32).round() as u32).min(atlas.h - 1);
    let i = ((y * atlas.w + x) * 4) as usize;
    [
        atlas.rgba[i] as f32 / 255.0,
        atlas.rgba[i + 1] as f32 / 255.0,
        atlas.rgba[i + 2] as f32 / 255.0,
    ]
}

/// Q3 lightmaps are stored dark (overbright). Do **not** multiply by the
/// drawvert color — that is a separate vertex-lit path and made rooms black.
fn apply_lightmap(c: [f32; 3]) -> [f32; 3] {
    [
        (c[0] * 4.0).clamp(0.22, 1.0),
        (c[1] * 4.0).clamp(0.22, 1.0),
        (c[2] * 4.0).clamp(0.22, 1.0),
    ]
}

fn vert_lit(
    v: &DrawVert,
    lm_num: i32,
    atlas: Option<&LightmapAtlas>,
) -> ([f32; 3], [f32; 2]) {
    if let Some(atlas) = atlas {
        if lm_num >= 0 {
            let uv = atlas_lm_uv(atlas, lm_num, v.lmst);
            return (apply_lightmap(sample_atlas(atlas, uv)), uv);
        }
    }
    (
        [
            (v.color[0] * 2.0).clamp(0.22, 1.0),
            (v.color[1] * 2.0).clamp(0.22, 1.0),
            (v.color[2] * 2.0).clamp(0.22, 1.0),
        ],
        [-1.0, -1.0],
    )
}

fn skip_shader(name: &str, flags: i32) -> bool {
    // Keep SURF_SKY: those faces are the outdoor hull. Skipping them
    // left a huge gray void in the courtyard corner.
    if flags & (SURF_NODRAW | SURF_SKIP) != 0 {
        return true;
    }
    let n = name.to_ascii_lowercase();
    let stem = n.rsplit('/').next().unwrap_or(&n);
    stem == "noshader" || stem == "skip" || n == "noshader" || n.is_empty()
}

fn emit_indexed_face(
    geom: &mut PartGeom,
    verts: &[DrawVert],
    meshverts: &[i32],
    first_vert: i32,
    n_vertexes: i32,
    first_idx: i32,
    n_meshverts: i32,
    lm_num: i32,
    atlas: Option<&LightmapAtlas>,
) {
    if n_meshverts < 3 || n_vertexes <= 0 {
        return;
    }
    let start = first_idx as usize;
    let count = n_meshverts as usize;
    if start.saturating_add(count) > meshverts.len() {
        return;
    }
    let base_vert = first_vert as usize;
    let nverts = n_vertexes as usize;
    let mut cache: HashMap<usize, u32> = HashMap::new();
    for tri in meshverts[start..start + count].chunks_exact(3) {
        let mut corners = [0u32; 3];
        let mut ok = true;
        for k in 0..3 {
            let idx = tri[k];
            let vi = resolve_meshvert(idx, base_vert, nverts);
            if vi >= verts.len() {
                ok = false;
                break;
            }
            corners[k] = *cache
                .entry(vi)
                .or_insert_with(|| emit_vert(geom, &verts[vi], lm_num, atlas));
        }
        if !ok {
            continue;
        }
        emit_indexed_tri(geom, corners[0], corners[1], corners[2]);
    }
}

fn resolve_meshvert(idx: i32, first_vert: usize, n_vertexes: usize) -> usize {
    if idx < 0 {
        return usize::MAX;
    }
    let u = idx as usize;
    if u < n_vertexes {
        first_vert.saturating_add(u)
    } else {
        u
    }
}

/// ioquake3 `r_subdivisions` default. Remaining sagitta (Quake units)
/// after uniform UV splits; a 90° gothic arch then gets 4 on-curve
/// segments (~22° facets). The dark C was the control hull, not a
/// need for 8–16 samples per cell.
const PATCH_SUBDIVISIONS: f32 = 4.0;
const PATCH_LEVEL_MIN: usize = 2;
const PATCH_LEVEL_MAX: usize = 6;

fn emit_patch(
    geom: &mut PartGeom,
    verts: &[DrawVert],
    first_vert: i32,
    n_vertexes: i32,
    patch_w: i32,
    patch_h: i32,
    lm_num: i32,
    atlas: Option<&LightmapAtlas>,
) {
    if patch_w < 3 || patch_h < 3 {
        return;
    }
    if patch_w % 2 == 0 || patch_h % 2 == 0 {
        return;
    }
    let w = patch_w as usize;
    let h = patch_h as usize;
    if w.saturating_mul(h) as i32 != n_vertexes {
        return;
    }
    let base = first_vert as usize;
    if base.saturating_add(w * h) > verts.len() {
        return;
    }
    // Evaluate on the quadratic, not the control hull. One level for the
    // whole patch so shared cell edges weld. Index the grid — do not
    // emit three unique verts per triangle.
    let ctrl = |r: usize, c: usize| &verts[base + r * w + c];
    let cells_x = (w - 1) / 2;
    let cells_y = (h - 1) / 2;
    let mut level = PATCH_LEVEL_MIN;
    for cy in 0..cells_y {
        for cx in 0..cells_x {
            let r = cy * 2;
            let c = cx * 2;
            let cell = [
                [ctrl(r, c), ctrl(r, c + 1), ctrl(r, c + 2)],
                [ctrl(r + 1, c), ctrl(r + 1, c + 1), ctrl(r + 1, c + 2)],
                [ctrl(r + 2, c), ctrl(r + 2, c + 1), ctrl(r + 2, c + 2)],
            ];
            level = level.max(patch_cell_level(cell));
        }
    }
    let nx = cells_x * level;
    let ny = cells_y * level;
    let stride = nx + 1;
    let mut grid = vec![DrawVert::default(); stride * (ny + 1)];
    for iy in 0..=ny {
        let cell_y = (iy / level).min(cells_y.saturating_sub(1));
        let ty = (iy - cell_y * level) as f32 / level as f32;
        for ix in 0..=nx {
            let cell_x = (ix / level).min(cells_x.saturating_sub(1));
            let tx = (ix - cell_x * level) as f32 / level as f32;
            let r = cell_y * 2;
            let c = cell_x * 2;
            let cell = [
                [ctrl(r, c), ctrl(r, c + 1), ctrl(r, c + 2)],
                [ctrl(r + 1, c), ctrl(r + 1, c + 1), ctrl(r + 1, c + 2)],
                [ctrl(r + 2, c), ctrl(r + 2, c + 1), ctrl(r + 2, c + 2)],
            ];
            grid[iy * stride + ix] = bezier_patch_eval(cell, tx, ty);
        }
    }
    // ioquake3 drops columns/rows whose midpoints sit on the chord
    // (Radiant Bevel / IBevel repeat a corner 7×). Keeping them and
    // always splitting 00–11 deletes the spandrel and leaves a C.
    let (grid, cols, rows) = collapse_patch_grid(grid, stride, ny + 1);
    let mut ids = vec![0u32; cols * rows];
    for iy in 0..rows {
        for ix in 0..cols {
            ids[iy * cols + ix] = emit_vert(geom, &grid[iy * cols + ix], lm_num, atlas);
        }
    }
    for iy in 0..rows.saturating_sub(1) {
        for ix in 0..cols.saturating_sub(1) {
            let i00 = ids[iy * cols + ix];
            let i10 = ids[iy * cols + ix + 1];
            let i01 = ids[(iy + 1) * cols + ix];
            let i11 = ids[(iy + 1) * cols + ix + 1];
            emit_patch_quad(geom, i00, i10, i01, i11);
        }
    }
}

/// Drop a column/row when every sample is within 0.1 Quake units of the
/// previous kept line — the `maxLen < 0.1` test in `R_SubdividePatchToGrid`.
fn collapse_patch_grid(
    grid: Vec<DrawVert>,
    cols: usize,
    rows: usize,
) -> (Vec<DrawVert>, usize, usize) {
    if cols == 0 || rows == 0 || grid.len() < cols * rows {
        return (grid, cols, rows);
    }
    let keep_col = {
        let mut keep = vec![true; cols];
        let mut prev = 0usize;
        for c in 1..cols {
            let mut max_d = 0.0f32;
            for r in 0..rows {
                max_d = max_d.max(vert_dist(&grid[r * cols + c], &grid[r * cols + prev]));
            }
            if max_d < 0.1 {
                keep[c] = false;
            } else {
                prev = c;
            }
        }
        keep
    };
    let keep_row = {
        let mut keep = vec![true; rows];
        let mut prev = 0usize;
        for r in 1..rows {
            let mut max_d = 0.0f32;
            for c in 0..cols {
                if !keep_col[c] {
                    continue;
                }
                max_d = max_d.max(vert_dist(&grid[r * cols + c], &grid[prev * cols + c]));
            }
            if max_d < 0.1 {
                keep[r] = false;
            } else {
                prev = r;
            }
        }
        keep
    };
    let nc = keep_col.iter().filter(|k| **k).count().max(1);
    let nr = keep_row.iter().filter(|k| **k).count().max(1);
    if nc == cols && nr == rows {
        return (grid, cols, rows);
    }
    let mut out = Vec::with_capacity(nc * nr);
    for r in 0..rows {
        if !keep_row[r] {
            continue;
        }
        for c in 0..cols {
            if keep_col[c] {
                out.push(grid[r * cols + c]);
            }
        }
    }
    (out, nc, nr)
}

fn vert_dist(a: &DrawVert, b: &DrawVert) -> f32 {
    let dx = a.pos[0] - b.pos[0];
    let dy = a.pos[1] - b.pos[1];
    let dz = a.pos[2] - b.pos[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn emit_patch_quad(geom: &mut PartGeom, i00: u32, i10: u32, i01: u32, i11: u32) {
    let a = tri_area_l1(geom, i00, i10, i11) + tri_area_l1(geom, i00, i11, i01);
    let b = tri_area_l1(geom, i00, i10, i01) + tri_area_l1(geom, i10, i11, i01);
    if a >= b {
        emit_indexed_tri(geom, i00, i10, i11);
        emit_indexed_tri(geom, i00, i11, i01);
    } else {
        emit_indexed_tri(geom, i00, i10, i01);
        emit_indexed_tri(geom, i10, i11, i01);
    }
}

fn tri_area_l1(geom: &PartGeom, ia: u32, ib: u32, ic: u32) -> f32 {
    let pa = geom.positions[ia as usize];
    let pb = geom.positions[ib as usize];
    let pc = geom.positions[ic as usize];
    let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
    (e1[1] * e2[2] - e1[2] * e2[1]).abs()
        + (e1[2] * e2[0] - e1[0] * e2[2]).abs()
        + (e1[0] * e2[1] - e1[1] * e2[0]).abs()
}

/// Uniform splits so remaining sagitta stays under `r_subdivisions`.
/// Quadratic error scales as 1/n², so n = ceil(sqrt(err / 4)).
fn patch_cell_level(cell: [[&DrawVert; 3]; 3]) -> usize {
    let mut err2 = 0.0f32;
    for i in 0..3 {
        err2 = err2.max(bezier_chord_err2(cell[i][0], cell[i][1], cell[i][2]));
        err2 = err2.max(bezier_chord_err2(cell[0][i], cell[1][i], cell[2][i]));
    }
    let n = (err2.sqrt() / PATCH_SUBDIVISIONS).sqrt().ceil() as usize;
    n.clamp(PATCH_LEVEL_MIN, PATCH_LEVEL_MAX)
}

fn bezier_chord_err2(a: &DrawVert, b: &DrawVert, c: &DrawVert) -> f32 {
    let lin = [
        (a.pos[0] + c.pos[0]) * 0.5,
        (a.pos[1] + c.pos[1]) * 0.5,
        (a.pos[2] + c.pos[2]) * 0.5,
    ];
    let bez = bezier_pos(a, b, c, 0.5);
    let dx = lin[0] - bez[0];
    let dy = lin[1] - bez[1];
    let dz = lin[2] - bez[2];
    dx * dx + dy * dy + dz * dz
}

fn bezier_patch_eval(ctrl: [[&DrawVert; 3]; 3], u: f32, v: f32) -> DrawVert {
    let c0 = bezier_vert(ctrl[0][0], ctrl[0][1], ctrl[0][2], u);
    let c1 = bezier_vert(ctrl[1][0], ctrl[1][1], ctrl[1][2], u);
    let c2 = bezier_vert(ctrl[2][0], ctrl[2][1], ctrl[2][2], u);
    bezier_vert(&c0, &c1, &c2, v)
}

fn bezier_vert(a: &DrawVert, b: &DrawVert, c: &DrawVert, t: f32) -> DrawVert {
    let s = 1.0 - t;
    let w0 = s * s;
    let w1 = 2.0 * s * t;
    let w2 = t * t;
    let mix3 = |x: [f32; 3], y: [f32; 3], z: [f32; 3]| {
        [
            x[0] * w0 + y[0] * w1 + z[0] * w2,
            x[1] * w0 + y[1] * w1 + z[1] * w2,
            x[2] * w0 + y[2] * w1 + z[2] * w2,
        ]
    };
    let mix2 = |x: [f32; 2], y: [f32; 2], z: [f32; 2]| {
        [
            x[0] * w0 + y[0] * w1 + z[0] * w2,
            x[1] * w0 + y[1] * w1 + z[1] * w2,
        ]
    };
    DrawVert {
        pos: mix3(a.pos, b.pos, c.pos),
        st: mix2(a.st, b.st, c.st),
        lmst: mix2(a.lmst, b.lmst, c.lmst),
        normal: mix3(a.normal, b.normal, c.normal),
        color: mix3(a.color, b.color, c.color),
    }
}

fn bezier_pos(a: &DrawVert, b: &DrawVert, c: &DrawVert, t: f32) -> [f32; 3] {
    let s = 1.0 - t;
    let w0 = s * s;
    let w1 = 2.0 * s * t;
    let w2 = t * t;
    [
        a.pos[0] * w0 + b.pos[0] * w1 + c.pos[0] * w2,
        a.pos[1] * w0 + b.pos[1] * w1 + c.pos[1] * w2,
        a.pos[2] * w0 + b.pos[2] * w1 + c.pos[2] * w2,
    ]
}

fn emit_vert(
    geom: &mut PartGeom,
    v: &DrawVert,
    lm_num: i32,
    atlas: Option<&LightmapAtlas>,
) -> u32 {
    let i = geom.positions.len() as u32;
    geom.positions.push(xform(v.pos));
    // Raw tiling ST. Q3 v=0 is OpenGL bottom; glTF v=0 is top.
    geom.uvs.push([v.st[0], 1.0 - v.st[1]]);
    geom.normals.push(xform_dir(v.normal));
    let (color, lm_uv) = vert_lit(v, lm_num, atlas);
    geom.colors.push(color);
    geom.lm_uvs.push(lm_uv);
    i
}

fn emit_indexed_tri(geom: &mut PartGeom, ia: u32, ib: u32, ic: u32) {
    let pa = geom.positions[ia as usize];
    let pb = geom.positions[ib as usize];
    let pc = geom.positions[ic as usize];
    let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
    let area = (e1[1] * e2[2] - e1[2] * e2[1]).abs()
        + (e1[2] * e2[0] - e1[0] * e2[2]).abs()
        + (e1[0] * e2[1] - e1[1] * e2[0]).abs();
    if area < 1e-10 {
        return;
    }
    geom.indices.extend_from_slice(&[ia, ib, ic]);
}

fn xform(p: [f32; 3]) -> [f32; 3] {
    [p[0] * SCALE, p[2] * SCALE, -p[1] * SCALE]
}

fn xform_dir(n: [f32; 3]) -> [f32; 3] {
    let v = [n[0], n[2], -n[1]];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-8 {
        [0.0, 1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct DrawVert {
    pos: [f32; 3],
    st: [f32; 2],
    lmst: [f32; 2],
    normal: [f32; 3],
    color: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
struct WorldSpawn {
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
}

fn write_spawn_sidecar(glb: &Path, nav: &crate::world_nav::WorldNav) {
    let _ = std::fs::write(glb.with_extension("spawn"), nav.to_text());
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

/// One entity block of the entity lump, keys in file order.
///
/// Quake II and Quake III write the same `{ "key" "value" }` lump, so the
/// Quake II importer parses its own entities with this too.
#[derive(Clone, Debug, Default)]
pub(crate) struct Q3Entity {
    pub keys: Vec<(String, String)>,
}

impl Q3Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn class(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.get(key).and_then(|v| v.trim().parse::<f32>().ok())
    }

    pub fn origin(&self) -> Option<[f32; 3]> {
        let mut it = self.get("origin")?.split_whitespace();
        let x = it.next()?.parse::<f32>().ok()?;
        let y = it.next()?.parse::<f32>().ok()?;
        let z = it.next()?.parse::<f32>().ok()?;
        Some([x, y, z])
    }

    /// Q3's `angle` is the yaw alone; `angles` is `pitch yaw roll`
    /// (`F_ANGLEHACK` in `g_spawn.c` turns `angle` into `angles[1]`).
    pub fn angles(&self) -> (f32, f32) {
        let mut pitch = 0.0f32;
        let mut yaw = self.float("angle").unwrap_or(0.0);
        if let Some(v) = self.get("angles") {
            let mut it = v.split_whitespace();
            if let (Some(p), Some(y)) = (it.next(), it.next()) {
                pitch = p.parse().unwrap_or(0.0);
                yaw = y.parse().unwrap_or(yaw);
            }
        }
        (pitch, yaw)
    }

    /// The brush submodel this entity moves, from `"model" "*N"`.
    pub fn submodel(&self) -> Option<usize> {
        self.get("model")?.strip_prefix('*')?.trim().parse().ok()
    }
}

/// Every `{ "key" "value" … }` block of the entity lump.
///
/// The lump is flat — an entity never nests — so brace depth is enough, and
/// a quoted value may contain spaces, braces or backslashes (`"music"
/// "music\sonic5.wav"`), which a `split('"')` scan mangles.
pub(crate) fn parse_entities(text: &str) -> Vec<Q3Entity> {
    let mut out = Vec::new();
    let mut current: Option<Q3Entity> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('{') {
            current = Some(Q3Entity::default());
            continue;
        }
        if line.starts_with('}') {
            if let Some(e) = current.take() {
                if !e.keys.is_empty() {
                    out.push(e);
                }
            }
            continue;
        }
        let Some(entity) = current.as_mut() else {
            continue;
        };
        let bytes = line.as_bytes();
        let mut quote = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'"')
            .map(|(i, _)| i);
        let (Some(a), Some(b), Some(c), Some(d)) =
            (quote.next(), quote.next(), quote.next(), quote.next())
        else {
            continue;
        };
        entity.keys.push((line[a + 1..b].to_string(), line[c + 1..d].to_string()));
    }
    out
}

/// The same, from the raw entity lump: NUL-terminated, and not always UTF-8
/// (mappers typed map titles in whatever code page they had).
pub(crate) fn parse_entity_lump(text: &[u8]) -> Vec<Q3Entity> {
    let text = &text[..text.iter().position(|b| *b == 0).unwrap_or(text.len())];
    match std::str::from_utf8(text) {
        Ok(t) => parse_entities(t),
        Err(_) => parse_entities(&text.iter().map(|b| *b as char).collect::<String>()),
    }
}

fn entities_of(bytes: &[u8]) -> Vec<Q3Entity> {
    match lump(bytes, LUMP_ENTITIES) {
        Ok((off, len)) => parse_entity_lump(&bytes[off..off + len]),
        Err(_) => Vec::new(),
    }
}

/// Q3 is Z-up: map `(x, y, z)` → GLB `(x, z, −y)`, scaled to metres.
pub(crate) fn map_to_glb(o: [f32; 3]) -> [f32; 3] {
    [o[0] * SCALE, o[2] * SCALE, -o[1] * SCALE]
}

/// Yaw 0 looks down −Z and grows toward −X, the convention every classic
/// converter in this crate writes.
fn map_yaw(angle: f32) -> f32 {
    std::f32::consts::FRAC_PI_2 - angle.to_radians()
}

/// An eye position above a spawn/teleport-destination origin.
fn eye_of(o: [f32; 3]) -> [f32; 3] {
    let mut p = map_to_glb(o);
    p[1] += VIEW_HEIGHT * SCALE;
    p
}

// ---------------------------------------------------------------------------
// Brush models and the movers that reference them
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Bsp46Model {
    mins: [f32; 3],
    maxs: [f32; 3],
    first_face: usize,
    num_faces: usize,
}

fn bsp46_models(bytes: &[u8]) -> Vec<Bsp46Model> {
    let Ok((off, len)) = lump(bytes, LUMP_MODELS) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(len / MODEL_SIZE);
    for i in 0..len / MODEL_SIZE {
        let o = off + i * MODEL_SIZE;
        if o + MODEL_SIZE > bytes.len() {
            break;
        }
        out.push(Bsp46Model {
            mins: [f32_le(bytes, o), f32_le(bytes, o + 4), f32_le(bytes, o + 8)],
            maxs: [
                f32_le(bytes, o + 12),
                f32_le(bytes, o + 16),
                f32_le(bytes, o + 20),
            ],
            first_face: i32_le(bytes, o + 24).max(0) as usize,
            num_faces: i32_le(bytes, o + 28).max(0) as usize,
        });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoverKind {
    Door,
    Lift,
}

/// A brush submodel that MOVES, resolved to its face range and its open pose.
#[derive(Clone, Debug)]
struct Q3Mover {
    /// Same string as the glTF node, the clip and the nav entry.
    name: String,
    kind: MoverKind,
    first_face: usize,
    num_faces: usize,
    /// Brush centre in GLB space (metres) at the pose the BSP bakes.
    centre: [f32; 3],
    /// Offset from the pose the node's geometry is AUTHORED in to the far
    /// pose, GLB metres.
    travel: [f32; 3],
    /// What to add to the baked vertices so they sit in the authored pose.
    /// Zero for everything except a `start_open` door, whose brushes q3map
    /// baked OPEN (see [`bsp46_movers`]).
    shift: [f32; 3],
    /// Dominant axis of `travel`, for the node extras.
    axis: &'static str,
    /// The walkable surface a lift rests at (its brush top), GLB metres.
    top_y: f32,
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

/// `func_door` and `func_plat` submodels, with the travel Q3's own movers use.
///
/// The arithmetic is `SP_func_door` / `SP_func_plat` in `g_mover.c`, and it is
/// the same one [`crate::classic_import::quake`]'s `quake_doors` implements for
/// Quake 1 (that file is another lane's; this is a local copy, not a call):
///
/// * a door travels along `angle` — `-1` up, `-2` down, otherwise a compass
///   yaw — by its own size on that axis minus `lip` (default 8). The BSP bakes
///   the CLOSED pose, unless `spawnflags & 1` (`START_OPEN`) swapped `pos1`
///   and `pos2`, in which case it baked the OPEN one.
/// * a plat travels DOWN by `height`, or by its own Z size minus `lip` when
///   `height` is unset. The BSP bakes the RAISED pose. (`SP_func_plat` has no
///   `START_OPEN`.)
fn bsp46_movers(entities: &[Q3Entity], models: &[Bsp46Model]) -> (Vec<Q3Mover>, Vec<String>) {
    let mut out: Vec<Q3Mover> = Vec::new();
    let mut warnings = Vec::new();
    let mut doors = 0usize;
    let mut lifts = 0usize;
    for e in entities {
        let kind = match e.class() {
            "func_door" => MoverKind::Door,
            "func_plat" => MoverKind::Lift,
            // A sinusoid on `height`/`phase` with no open/closed pair:
            // `world_nav::NavDoor` and `ExtraNode` cannot express it, so its
            // brushes stay in the static mesh where a walker meets them.
            "func_bobbing" | "func_train" | "func_rotating" | "func_pendulum" => {
                warnings.push(format!(
                    "{}: not a door or a lift — its brushes stay static",
                    e.class()
                ));
                continue;
            }
            // A damage volume, but Q3 marks one with the `common/trigger`
            // shader, which is SURF_NODRAW: the submodel has no drawable
            // faces at all, so there is nothing to carve into a hazard node.
            // Both demo maps that have one (q3dm17, q3tourney2) are like
            // this. Record the fact rather than invent geometry for it.
            "trigger_hurt" => {
                let faces = e
                    .submodel()
                    .and_then(|i| models.get(i))
                    .map(|m| m.num_faces)
                    .unwrap_or(0);
                warnings.push(format!(
                    "trigger_hurt (dmg {}) has {faces} drawable faces — a damage \
                     volume is not geometry, so it is not published as a hazard",
                    e.get("dmg").unwrap_or("2")
                ));
                continue;
            }
            _ => continue,
        };
        let Some(index) = e.submodel() else { continue };
        let Some(model) = models.get(index).copied() else {
            warnings.push(format!("{} references missing *{index}", e.class()));
            continue;
        };
        if model.num_faces == 0 {
            warnings.push(format!(
                "{} *{index} has no drawable faces — nothing to move",
                e.class()
            ));
            continue;
        }
        let size = [
            model.maxs[0] - model.mins[0],
            model.maxs[1] - model.mins[1],
            model.maxs[2] - model.mins[2],
        ];
        let lip = e.float("lip").unwrap_or(Q3_MOVER_LIP);
        let mut shift = [0.0f32; 3];
        let (travel, axis) = match kind {
            MoverKind::Door => {
                let (_, angle) = e.angles();
                let dir = if (angle + 1.0).abs() < 0.01 {
                    [0.0, 0.0, 1.0]
                } else if (angle + 2.0).abs() < 0.01 {
                    [0.0, 0.0, -1.0]
                } else {
                    let r = angle.to_radians();
                    [r.cos(), r.sin(), 0.0]
                };
                let span = (dir[0] * size[0]).abs()
                    + (dir[1] * size[1]).abs()
                    + (dir[2] * size[2]).abs();
                let distance = (span - lip).max(0.0);
                if distance <= 0.0 {
                    warnings.push(format!("func_door *{index} travels nowhere (lip {lip})"));
                    continue;
                }
                let mut t = [
                    dir[0] * distance * SCALE,
                    dir[2] * distance * SCALE,
                    -dir[1] * distance * SCALE,
                ];
                // `START_OPEN` swaps `pos1`/`pos2`, so the baked brush IS the
                // open pose and CLOSED sits one travel further along. The
                // contract wants the geometry authored closed and the node
                // resting open, so shift the vertices to closed and open back
                // the other way — which lands exactly on the baked pose.
                if (e.float("spawnflags").unwrap_or(0.0) as i32) & 1 != 0 {
                    shift = t;
                    t = [-t[0], -t[1], -t[2]];
                }
                (t, dominant_axis(t))
            }
            MoverKind::Lift => {
                let height = e.float("height").filter(|h| *h > 0.0).unwrap_or(size[2] - lip);
                if height <= 0.0 {
                    warnings.push(format!("func_plat *{index} travels nowhere (lip {lip})"));
                    continue;
                }
                ([0.0, -height * SCALE, 0.0], "y")
            }
        };
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
        let mid = [
            (model.mins[0] + model.maxs[0]) * 0.5,
            (model.mins[1] + model.maxs[1]) * 0.5,
            (model.mins[2] + model.maxs[2]) * 0.5,
        ];
        out.push(Q3Mover {
            name,
            kind,
            first_face: model.first_face,
            num_faces: model.num_faces,
            centre: map_to_glb(mid),
            travel,
            shift,
            axis,
            top_y: model.maxs[2] * SCALE,
        });
    }
    (out, warnings)
}

// ---------------------------------------------------------------------------
// Navigation facts
// ---------------------------------------------------------------------------

/// Everything a walker needs: every start, the engine's height constants, the
/// movers, and the teleport pads with where they land.
fn bsp46_nav(
    entities: &[Q3Entity],
    models: &[Bsp46Model],
    movers: &[Q3Mover],
) -> (crate::world_nav::WorldNav, Vec<String>) {
    use crate::world_nav::{
        deathmatch_name, player_start_name, NavDoor, NavStart, NavTeleport, WorldNav,
    };
    let mut warnings = Vec::new();
    let mut primary: Option<NavStart> = None;
    let mut extra_starts: Vec<NavStart> = Vec::new();
    let mut deathmatch: Vec<NavStart> = Vec::new();
    let mut floor_y = None;
    for e in entities {
        // `team_CTF_*player` is the ONE spot a team starts a round on, so it
        // reads as a player start; `team_CTF_*spawn` is a respawn point among
        // many, which is what `deathmatch_N` means.
        let rank = match e.class() {
            "info_player_start" => 0u8,
            "info_player_coop" | "team_CTF_redplayer" | "team_CTF_blueplayer" => 1,
            "info_player_deathmatch" | "team_CTF_redspawn" | "team_CTF_bluespawn" => 2,
            _ => continue,
        };
        let Some(o) = e.origin() else { continue };
        let (pitch, yaw) = e.angles();
        let start = NavStart {
            name: String::new(),
            pos: eye_of(o),
            yaw: map_yaw(yaw),
            pitch: -pitch.to_radians(),
        };
        if rank == 0 && primary.is_none() {
            floor_y = Some((o[2] - ORIGIN_ABOVE_FLOOR) * SCALE);
            primary = Some(start);
        } else if rank == 2 {
            deathmatch.push(start);
        } else {
            extra_starts.push(start);
        }
    }
    let mut starts = Vec::with_capacity(1 + extra_starts.len() + deathmatch.len());
    for (i, mut s) in primary.into_iter().chain(extra_starts).enumerate() {
        s.name = player_start_name(i);
        starts.push(s);
    }
    for (i, mut s) in deathmatch.into_iter().enumerate() {
        s.name = deathmatch_name(i);
        starts.push(s);
    }
    let eye_height = (VIEW_HEIGHT + ORIGIN_ABOVE_FLOOR) * SCALE;
    // An arena map with no `info_player_start` (q3dm1 is one) leads with its
    // first deathmatch spot, exactly as the Quake 1 converter does — the
    // sidecar's three-line header must name somewhere a walker can stand.
    let floor_y = floor_y.or_else(|| starts.first().map(|s| s.pos[1] - eye_height));

    let mut teleports = Vec::new();
    for e in entities {
        if e.class() != "trigger_teleport" {
            continue;
        }
        let Some(index) = e.submodel() else { continue };
        let Some(model) = models.get(index).copied() else {
            continue;
        };
        let Some(target) = e.get("target") else {
            warnings.push(format!("trigger_teleport *{index} has no target"));
            continue;
        };
        let dest = entities.iter().find(|d| {
            d.get("targetname") == Some(target)
                && matches!(d.class(), "misc_teleporter_dest" | "target_position")
        });
        let Some(dest) = dest.and_then(|d| d.origin().map(|o| (d, o))) else {
            warnings.push(format!("trigger_teleport target `{target}` has no destination"));
            continue;
        };
        let (_, yaw) = dest.0.angles();
        teleports.push(NavTeleport {
            name: format!("teleport_{}", teleports.len() + 1),
            // Map +y becomes GLB −z, so the AABB flips on that axis.
            pad_min: [model.mins[0] * SCALE, -model.maxs[1] * SCALE],
            pad_max: [model.maxs[0] * SCALE, -model.mins[1] * SCALE],
            dst: eye_of(dest.1),
            yaw: map_yaw(yaw),
        });
    }

    let mut doors = Vec::new();
    let mut lifts = Vec::new();
    for m in movers {
        match m.kind {
            // A Q3 door slides along its own vector; the nav summary says
            // WHERE it is and how far it goes, the GLB clip says which way —
            // the same split the Quake 1 converter publishes.
            MoverKind::Door => {
                let travel = (m.travel[0] * m.travel[0]
                    + m.travel[1] * m.travel[1]
                    + m.travel[2] * m.travel[2])
                    .sqrt();
                // The doorway centre at the CLOSED pose — which for a
                // `start_open` door is the baked centre plus the shift.
                let closed = [
                    m.centre[0] + m.shift[0],
                    m.centre[1] + m.shift[1],
                    m.centre[2] + m.shift[2],
                ];
                doors.push(NavDoor {
                    name: m.name.clone(),
                    pos: closed,
                    closed_y: closed[1],
                    // A Quake III door mostly slides SIDEWAYS: the Y pair is
                    // only the vertical part of the move, and `offset`
                    // carries the whole of it.
                    open_y: closed[1] + m.travel[1],
                    offset: m.travel,
                });
                let _ = travel;
            }
            // A lift rests UP on the surface you stand on, and drops.
            MoverKind::Lift => lifts.push(NavDoor::vertical(
                m.name.clone(),
                [m.centre[0], m.top_y, m.centre[2]],
                m.top_y,
                m.top_y + m.travel[1],
            )),
        }
    }

    (
        WorldNav {
            starts,
            floor_y,
            step_height: Some(STEP_HEIGHT * SCALE),
            eye_height: Some(eye_height),
            doors,
            lifts,
            teleports,
            // Q3 arena maps have no exit and no keys: a match ends on a frag
            // count, not on a switch. Publishing an invented `exit` would send
            // a walker somewhere the map never meant.
            markers: Vec::new(),
        },
        warnings,
    )
}

/// The single spawn the `.place` sidecar's header carries: `info_player_start`
/// outranks `info_player_deathmatch`, as it always did.
fn bsp46_spawn(entities: &[Q3Entity]) -> Option<WorldSpawn> {
    let mut best: Option<(u8, WorldSpawn)> = None;
    for e in entities {
        let rank = match e.class() {
            "info_player_start" => 0u8,
            "info_player_deathmatch" => 1,
            _ => continue,
        };
        let Some(o) = e.origin() else { continue };
        let (pitch, yaw) = e.angles();
        let spawn = WorldSpawn {
            pos: eye_of(o),
            yaw: map_yaw(yaw),
            pitch: -pitch.to_radians(),
        };
        match best {
            Some((r, _)) if r <= rank => {}
            _ => best = Some((rank, spawn)),
        }
    }
    best.map(|(_, s)| s)
}

fn bsp46_place(
    entities: &[Q3Entity],
    source: &str,
    world_key: &str,
) -> crate::world_place::WorldPlace {
    let spawn = bsp46_spawn(entities).map(|s| (s.pos, s.yaw, s.pitch));
    let mut places = Vec::new();
    let mut i = 0usize;
    for e in entities {
        let Some(o) = e.origin() else { continue };
        let class = e.class().to_string();
        let model = e.get("model").unwrap_or("").to_string();
        let (kind, asset) = if let Some((k, key)) = crate::world_place::q3_class_actor(&class) {
            (k, key.to_string())
        } else if class == "misc_model" && model.to_ascii_lowercase().ends_with(".md3") {
            ("prop", crate::world_place::q3_md3_catalog_key(&model))
        } else {
            continue;
        };
        let (_, yaw) = e.angles();
        places.push(crate::world_place::Place {
            id: format!("ent-{i}"),
            kind: kind.into(),
            asset,
            pos: map_to_glb(o),
            yaw: map_yaw(yaw),
            class,
            width: 0.0,
            height: 0.0,
            align: String::new(),
            flags: 0,
            ..Default::default()
        });
        i += 1;
    }
    crate::world_place::WorldPlace {
        source: source.into(),
        world: world_key.into(),
        spawn,
        places,
        family: Default::default(),
    }
}

fn lump(bytes: &[u8], i: usize) -> Result<(usize, usize), String> {
    let o = 8 + i * 8;
    if o + 8 > bytes.len() {
        return Err(format!("lump {i} header oob"));
    }
    let off = u32_le(bytes, o) as usize;
    let len = u32_le(bytes, o + 4) as usize;
    if off.saturating_add(len) > bytes.len() {
        return Err(format!("lump {i} truncated"));
    }
    Ok((off, len))
}

// ---------------------------------------------------------------------------
// MD3
// ---------------------------------------------------------------------------

pub fn is_lod_md3(rel: &str) -> bool {
    let name = rel
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or(rel)
        .to_ascii_lowercase();
    name.ends_with("_1.md3") || name.ends_with("_2.md3")
}

pub fn convert_md3(
    bytes: &[u8],
    rel: &str,
    src_path: &Path,
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<ClassicAsset, String> {
    let skins = load_md3_skins(src_path);
    let (glb, _name) = md3_to_glb(bytes, textures, &skins)?;
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    let kind = if lower.contains("/players/") || lower.starts_with("players/") {
        AssetKind::Character
    } else if lower.contains("/weapons/")
        || lower.starts_with("weapons/")
        || lower.contains("/weapon")
    {
        AssetKind::Weapon
    } else {
        AssetKind::Prop
    };
    let folder = match kind {
        AssetKind::Weapon => "weapons",
        AssetKind::Character => "characters",
        _ => "props",
    };
    let slug = path_slug(rel);
    let key = format!("{folder}/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &glb).map_err(|e| e.to_string())?;
    let mut icon_rel = None;
    if let Some(png) = raster_md3_icon(&glb) {
        let icon_path = dest.with_extension("png");
        if std::fs::write(&icon_path, png).is_ok() {
            icon_rel = Some(format!("{key}.png"));
        }
    }
    let kind_tag = match kind {
        AssetKind::Weapon => "weapon",
        AssetKind::Character => "character",
        _ => "prop",
    };
    Ok(ClassicAsset {
        key,
        kind,
        rel_path,
        tags: vec![
            kind_tag.into(),
            source_id.into(),
            "md3".into(),
            slug,
        ],
        icon_rel,
    })
}

fn load_md3_skins(src_path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(parent) = src_path.parent() else {
        return map;
    };
    let stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut candidates = vec![
        parent.join(format!("{stem}_default.skin")),
        parent.join(format!("{stem}.skin")),
    ];
    if let Ok(rd) = std::fs::read_dir(parent) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.ends_with(".skin") && name.contains("default") {
                candidates.push(entry.path());
            }
        }
    }
    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        parse_md3_skin(&text, &mut map);
        if !map.is_empty() {
            break;
        }
    }
    map
}

fn parse_md3_skin(text: &str, map: &mut BTreeMap<String, String>) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.to_ascii_lowercase().starts_with("tag_") {
            continue;
        }
        let Some((mesh, shader)) = line.split_once(',') else {
            continue;
        };
        let mesh = mesh.trim().to_ascii_lowercase();
        let shader = shader.trim();
        if mesh.is_empty() || shader.is_empty() {
            continue;
        }
        map.insert(mesh, shader.to_string());
    }
}

fn raster_md3_icon(glb: &[u8]) -> Option<Vec<u8>> {
    let parts = crate::world_preview::extract_glb_parts(glb).ok()?;
    let mut pos = Vec::new();
    let mut uv = Vec::new();
    let mut idx = Vec::new();
    let mut tex = vec![180u8, 170, 160, 255];
    let mut tw = 1usize;
    let mut th = 1usize;
    for p in &parts {
        let base = pos.len() as u32;
        pos.extend_from_slice(&p.pos);
        if p.uv.len() == p.pos.len() {
            uv.extend_from_slice(&p.uv);
        } else {
            uv.extend(std::iter::repeat_n([0.0, 0.0], p.pos.len()));
        }
        idx.extend(p.indices.iter().map(|i| i + base));
        if let Some(t) = &p.tex {
            if !t.is_empty() {
                tex = t.clone();
                tw = p.tex_size.0.max(1);
                th = p.tex_size.1.max(1);
            }
        }
    }
    if pos.is_empty() || idx.len() < 3 {
        return None;
    }
    let tile = crate::anim_icon::raster_mesh_icon(&pos, &idx, &uv, &tex, tw, th, 0.35, 256);
    encode_png_rgba(&tile, 256, 256).ok()
}

fn md3_to_glb(
    bytes: &[u8],
    textures: &Q3TexBank,
    skins: &BTreeMap<String, String>,
) -> Result<(Vec<u8>, String), String> {
    if bytes.len() < 108 || &bytes[0..4] != MD3_IDENT {
        return Err("not an MD3 (IDP3)".into());
    }
    let version = i32_le(bytes, 4);
    if version != MD3_VERSION {
        return Err(format!("unsupported MD3 version {version}"));
    }
    let name = cstr(&bytes[8..72]);
    // md3Header_t: flags@72, numFrames@76, numTags@80, numSurfaces@84,
    // ofsSurfaces@100. Reading flags as numFrames rejected every demo model.
    let num_frames = i32_le(bytes, 76);
    let num_surfaces = i32_le(bytes, 84);
    let ofs_surfaces = i32_le(bytes, 100) as usize;
    if num_frames < 1 || num_surfaces < 1 {
        return Err("MD3 has no frames/surfaces".into());
    }
    if ofs_surfaces + 108 > bytes.len() {
        return Err("MD3 surface truncated".into());
    }

    let mut images: BTreeMap<String, Q3Image> = BTreeMap::new();
    images.insert("_default".into(), gray_image(64, 64));
    let mut surfaces = Vec::new();
    let mut s = ofs_surfaces;
    for _ in 0..num_surfaces {
        if s + 108 > bytes.len() || &bytes[s..s + 4] != MD3_IDENT {
            return Err("MD3 surface ident".into());
        }
        let surf_name = cstr(&bytes[s + 4..s + 68]);
        let surf_frames = i32_le(bytes, s + 72);
        let num_shaders = i32_le(bytes, s + 76) as usize;
        let num_verts = i32_le(bytes, s + 80) as usize;
        let num_tris = i32_le(bytes, s + 84) as usize;
        let ofs_tris = i32_le(bytes, s + 88) as usize;
        let ofs_shaders = i32_le(bytes, s + 92) as usize;
        let ofs_st = i32_le(bytes, s + 96) as usize;
        let ofs_xyz = i32_le(bytes, s + 100) as usize;
        let ofs_end = i32_le(bytes, s + 104) as usize;
        if surf_frames < 1 || num_verts == 0 || num_tris == 0 {
            if ofs_end > 0 {
                s += ofs_end as usize;
            }
            continue;
        }
        if s + ofs_tris + num_tris * 12 > bytes.len()
            || s + ofs_st + num_verts * 8 > bytes.len()
            || s + ofs_xyz + num_verts * 8 > bytes.len()
        {
            return Err("MD3 surface data truncated".into());
        }
        let mut shader = skins
            .get(&surf_name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        if shader.is_empty() && num_shaders > 0 && s + ofs_shaders + 64 <= bytes.len() {
            shader = cstr(&bytes[s + ofs_shaders..s + ofs_shaders + 64]);
        }
        let tex_key = if shader.is_empty() {
            "_default".into()
        } else if let Some(img) = textures.resolve(&shader) {
            let key = normalize_tex_name(&shader);
            images.entry(key.clone()).or_insert(img);
            key
        } else {
            "_default".into()
        };
        surfaces.push((s, num_verts, num_tris, ofs_tris, ofs_st, ofs_xyz, tex_key, surf_name));
        if ofs_end <= 0 {
            break;
        }
        s += ofs_end as usize;
    }
    if surfaces.is_empty() {
        return Err("MD3 produced no surfaces".into());
    }
    let (atlas_png, uv_map) = pack_atlas(&images);

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut label = name.clone();
    for (s, num_verts, num_tris, ofs_tris, ofs_st, ofs_xyz, tex_key, surf_name) in surfaces {
        if label.is_empty() {
            label = surf_name;
        }
        let slot = lookup_slot(&uv_map, &tex_key);
        let xyz_off = s + ofs_xyz;
        for t in 0..num_tris {
            let to = s + ofs_tris + t * 12;
            let i0 = i32_le(bytes, to) as usize;
            let i1 = i32_le(bytes, to + 4) as usize;
            let i2 = i32_le(bytes, to + 8) as usize;
            if i0 >= num_verts || i1 >= num_verts || i2 >= num_verts {
                continue;
            }
            for vi in [i0, i1, i2] {
                let vo = xyz_off + vi * 8;
                let x = i16_le(bytes, vo) as f32 * MD3_XYZ_SCALE;
                let y = i16_le(bytes, vo + 2) as f32 * MD3_XYZ_SCALE;
                let z = i16_le(bytes, vo + 4) as f32 * MD3_XYZ_SCALE;
                let enc = u16_le(bytes, vo + 6);
                // Q3 +X forward → studio −Z look, same yaw as Q2 MD2.
                positions.push(q3_face_camera(xform([x, y, z])));
                normals.push(q3_face_camera_dir(xform_dir(md3_normal(enc))));
                let so = s + ofs_st + vi * 8;
                let u = f32_le(bytes, so);
                let v = f32_le(bytes, so + 4);
                uvs.push(slot_uv(slot, u, 1.0 - v));
                indices.push(indices.len() as u32);
            }
        }
    }
    if positions.is_empty() {
        return Err("MD3 produced no triangles".into());
    }
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: Some(&normals),
        uvs: &uvs,
        indices: &indices,
        base_color_png: &atlas_png,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }
    Ok((glb, label))
}

fn q3_face_camera(p: [f32; 3]) -> [f32; 3] {
    let yaw = -std::f32::consts::FRAC_PI_2;
    let (s, c) = (yaw.sin(), yaw.cos());
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}

fn q3_face_camera_dir(n: [f32; 3]) -> [f32; 3] {
    let v = q3_face_camera(n);
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-8 {
        [0.0, 1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Fold split player parts / weapon flash+barrel into whole cards.
/// `files` is `(extracted path, pk3-relative path)` for every MD3.
pub fn assemble_players_and_weapons(
    files: &[(PathBuf, String)],
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> (Vec<ClassicAsset>, BTreeSet<String>) {
    let mut assets = Vec::new();
    let mut drop = BTreeSet::new();
    let mut by_rel: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (path, rel) in files {
        by_rel.insert(rel.replace('\\', "/").to_ascii_lowercase(), path.clone());
    }
    let mut players: BTreeSet<String> = BTreeSet::new();
    let mut weapons: BTreeSet<String> = BTreeSet::new();
    for rel in by_rel.keys() {
        if let Some(name) = rel.strip_prefix("models/players/") {
            if let Some((who, _)) = name.split_once('/') {
                players.insert(who.to_string());
            }
        }
        if let Some(rest) = rel.strip_prefix("models/weapons2/") {
            if let Some((gun, file)) = rest.split_once('/') {
                if file == format!("{gun}.md3") {
                    weapons.insert(gun.to_string());
                }
            }
        }
    }
    for name in players {
        match assemble_player(&by_rel, &name, staged, source_id, textures) {
            Ok(asset) => {
                drop.insert(format!("characters/{name}-head"));
                drop.insert(format!("characters/{name}-lower"));
                drop.insert(format!("characters/{name}-upper"));
                assets.push(asset);
            }
            Err(e) => {
                // Keep the split parts if the whole character cannot be built.
                let _ = e;
            }
        }
    }
    for name in weapons {
        match assemble_weapon(&by_rel, &name, staged, source_id, textures) {
            Ok(asset) => {
                drop.insert(format!("weapons/{name}-{name}"));
                drop.insert(format!("weapons/{name}-{name}_barrel"));
                drop.insert(format!("weapons/{name}-{name}_flash"));
                drop.insert(format!("weapons/{name}-{name}_hand"));
                assets.push(asset);
            }
            Err(_) => {}
        }
    }
    (assets, drop)
}

fn assemble_player(
    by_rel: &BTreeMap<String, PathBuf>,
    name: &str,
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<ClassicAsset, String> {
    let lower_rel = format!("models/players/{name}/lower.md3");
    let upper_rel = format!("models/players/{name}/upper.md3");
    let head_rel = format!("models/players/{name}/head.md3");
    let lower_path = by_rel.get(&lower_rel).ok_or("missing lower.md3")?;
    let upper_path = by_rel.get(&upper_rel).ok_or("missing upper.md3")?;
    let head_path = by_rel.get(&head_rel).ok_or("missing head.md3")?;
    let lower_b = std::fs::read(lower_path).map_err(|e| e.to_string())?;
    let upper_b = std::fs::read(upper_path).map_err(|e| e.to_string())?;
    let head_b = std::fs::read(head_path).map_err(|e| e.to_string())?;
    let lower = Md3File::parse(&lower_b)?;
    let upper = Md3File::parse(&upper_b)?;
    let head = Md3File::parse(&head_b)?;
    let skins = {
        let mut m = load_md3_skins(lower_path);
        m.extend(load_md3_skins(upper_path));
        m.extend(load_md3_skins(head_path));
        m
    };
    let anims = by_rel
        .get(&format!("models/players/{name}/animation.cfg"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .or_else(|| {
            lower_path
                .parent()
                .and_then(|d| std::fs::read_to_string(d.join("animation.cfg")).ok())
        })
        .map(|t| parse_animation_cfg(&t))
        .unwrap_or_default();
    let torso_stand = anim_first(&anims, "TORSO_STAND").unwrap_or(0);
    let legs_idle = legs_file_frame(&anims, anim_first(&anims, "LEGS_IDLE").unwrap_or(0));
    let rest = pose_player(
        &lower,
        &upper,
        &head,
        legs_idle.max(0) as usize,
        torso_stand.max(0) as usize,
        0,
        textures,
        &skins,
    )?;
    if rest.unique.is_empty() || rest.indices.len() < 3 {
        return Err("assembled player empty".into());
    }
    let mut images: BTreeMap<String, Q3Image> = BTreeMap::new();
    images.insert("_default".into(), gray_image(64, 64));
    for key in &rest.tex_keys {
        if key != "_default" {
            if let Some(img) = textures.resolve(key) {
                images.entry(key.clone()).or_insert(img);
            }
        }
    }
    let (atlas_png, uv_map) = pack_atlas(&images);
    let uvs: Vec<[f32; 2]> = rest
        .corners
        .iter()
        .zip(rest.tex_keys.iter())
        .map(|(&vi, key)| {
            let slot = lookup_slot(&uv_map, key);
            let uv = rest.uvs.get(vi).copied().unwrap_or([0.0, 0.0]);
            slot_uv(slot, uv[0], uv[1])
        })
        .collect();

    let mut clip_defs: Vec<(String, Vec<(usize, usize)>)> = Vec::new();
    push_player_clip(&mut clip_defs, &anims, "idle", "LEGS_IDLE", "TORSO_STAND", 10);
    push_player_clip(&mut clip_defs, &anims, "walk", "LEGS_WALK", "TORSO_STAND", 12);
    push_player_clip(&mut clip_defs, &anims, "run", "LEGS_RUN", "TORSO_STAND", 11);
    push_player_clip(&mut clip_defs, &anims, "death", "BOTH_DEATH1", "BOTH_DEATH1", 16);

    let mut unique_frames = Vec::new();
    let mut named = Vec::new();
    unique_frames.push(rest.unique.clone());
    for (clip_name, pairs) in &clip_defs {
        if pairs.is_empty() {
            continue;
        }
        let start = unique_frames.len();
        for &(lf, uf) in pairs {
            let posed = pose_player(&lower, &upper, &head, lf, uf, 0, textures, &skins)?;
            if posed.unique.len() != rest.unique.len() {
                continue;
            }
            unique_frames.push(posed.unique);
        }
        if unique_frames.len() > start {
            named.push(vertex_skin::NamedClip {
                name: clip_name.clone(),
                frames: (start..unique_frames.len()).collect(),
            });
        }
    }
    if named.is_empty() {
        unique_frames.truncate(1);
    }
    let clips = vertex_skin::alias_loco_clips(named);
    let mut glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &rest
            .corners
            .iter()
            .map(|&i| rest.unique.get(i).copied().unwrap_or([0.0; 3]))
            .collect::<Vec<_>>(),
        normals: None,
        uvs: &uvs,
        indices: &rest.indices,
        base_color_png: &atlas_png,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if unique_frames.len() > 1 {
        if let Ok(skinned) = vertex_skin::write_skinned_from_vertex_frames(
            &rest.unique,
            &unique_frames,
            &rest.corners,
            &uvs,
            &rest.indices,
            &clips,
            &atlas_png,
        ) {
            glb = skinned;
        }
    }
    if !glb.starts_with(b"glTF") {
        return Err("player GLB failed".into());
    }
    let key = format!("characters/{name}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &glb).map_err(|e| e.to_string())?;
    let mut icon_rel = None;
    if let Some(png) = raster_md3_icon(&glb) {
        let icon_path = dest.with_extension("png");
        if std::fs::write(&icon_path, png).is_ok() {
            icon_rel = Some(format!("{key}.png"));
        }
    }
    let mut tags = vec![
        "character".into(),
        source_id.into(),
        "md3".into(),
        "assembled".into(),
        name.to_string(),
    ];
    if unique_frames.len() > 1 {
        tags.push("vertex-anim".into());
        tags.push("skinned".into());
        for clip in &clips {
            tags.push(format!("clip-{}", clip.name));
        }
    }
    Ok(ClassicAsset {
        key,
        kind: AssetKind::Character,
        rel_path,
        tags,
        icon_rel,
    })
}

fn assemble_weapon(
    by_rel: &BTreeMap<String, PathBuf>,
    name: &str,
    staged: &Path,
    source_id: &str,
    textures: &Q3TexBank,
) -> Result<ClassicAsset, String> {
    let gun_rel = format!("models/weapons2/{name}/{name}.md3");
    let gun_path = by_rel.get(&gun_rel).ok_or("missing weapon md3")?;
    let gun_b = std::fs::read(gun_path).map_err(|e| e.to_string())?;
    let gun = Md3File::parse(&gun_b)?;
    let barrel_rel = format!("models/weapons2/{name}/{name}_barrel.md3");
    let barrel_path = by_rel.get(&barrel_rel).cloned();
    let barrel_bytes = barrel_path.as_ref().and_then(|p| std::fs::read(p).ok());
    let barrel = barrel_bytes
        .as_deref()
        .and_then(|b| Md3File::parse(b).ok());
    let mut skins = load_md3_skins(gun_path);
    if let Some(bp) = &barrel_path {
        skins.extend(load_md3_skins(bp));
    }
    let posed = pose_weapon(&gun, barrel.as_ref(), textures, &skins)?;
    if posed.unique.is_empty() || posed.indices.len() < 3 {
        return Err("assembled weapon empty".into());
    }
    let mut images: BTreeMap<String, Q3Image> = BTreeMap::new();
    images.insert("_default".into(), gray_image(64, 64));
    for key in &posed.tex_keys {
        if key != "_default" {
            if let Some(img) = textures.resolve(key) {
                images.entry(key.clone()).or_insert(img);
            }
        }
    }
    let (atlas_png, uv_map) = pack_atlas(&images);
    let positions: Vec<[f32; 3]> = posed
        .corners
        .iter()
        .map(|&i| posed.unique.get(i).copied().unwrap_or([0.0; 3]))
        .collect();
    let uvs: Vec<[f32; 2]> = posed
        .corners
        .iter()
        .zip(posed.tex_keys.iter())
        .map(|(&vi, key)| {
            let slot = lookup_slot(&uv_map, key);
            let uv = posed.uvs.get(vi).copied().unwrap_or([0.0, 0.0]);
            slot_uv(slot, uv[0], uv[1])
        })
        .collect();
    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: None,
        uvs: &uvs,
        indices: &posed.indices,
        base_color_png: &atlas_png,
        metallic_roughness_png: None,
        double_sided: false,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("weapon GLB failed".into());
    }
    let key = format!("weapons/{name}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &glb).map_err(|e| e.to_string())?;
    let mut icon_rel = None;
    if let Some(png) = raster_md3_icon(&glb) {
        let icon_path = dest.with_extension("png");
        if std::fs::write(&icon_path, png).is_ok() {
            icon_rel = Some(format!("{key}.png"));
        }
    }
    Ok(ClassicAsset {
        key,
        kind: AssetKind::Weapon,
        rel_path,
        tags: vec![
            "weapon".into(),
            source_id.into(),
            "md3".into(),
            "assembled".into(),
            name.to_string(),
        ],
        icon_rel,
    })
}

struct Md3File<'a> {
    bytes: &'a [u8],
    num_frames: usize,
    num_tags: usize,
    num_surfaces: usize,
    ofs_tags: usize,
    ofs_surfaces: usize,
}

impl<'a> Md3File<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 108 || &bytes[0..4] != MD3_IDENT {
            return Err("not an MD3 (IDP3)".into());
        }
        if i32_le(bytes, 4) != MD3_VERSION {
            return Err("unsupported MD3 version".into());
        }
        let num_frames = i32_le(bytes, 76);
        let num_tags = i32_le(bytes, 80);
        let num_surfaces = i32_le(bytes, 84);
        if num_frames < 1 || num_surfaces < 1 {
            return Err("MD3 has no frames/surfaces".into());
        }
        Ok(Self {
            bytes,
            num_frames: num_frames as usize,
            num_tags: num_tags.max(0) as usize,
            num_surfaces: num_surfaces as usize,
            ofs_tags: i32_le(bytes, 96) as usize,
            ofs_surfaces: i32_le(bytes, 100) as usize,
        })
    }

    fn tag(&self, frame: usize, name: &str) -> Option<TagXform> {
        if self.num_tags == 0 {
            return None;
        }
        let frame = frame.min(self.num_frames.saturating_sub(1));
        let want = name.to_ascii_lowercase();
        for t in 0..self.num_tags {
            let o = self.ofs_tags + (frame * self.num_tags + t) * 112;
            if o + 112 > self.bytes.len() {
                return None;
            }
            let tag_name = cstr(&self.bytes[o..o + 64]).to_ascii_lowercase();
            if tag_name == want {
                return Some(read_tag(self.bytes, o));
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
struct TagXform {
    origin: [f32; 3],
    axis: [[f32; 3]; 3],
}

fn identity_tag() -> TagXform {
    TagXform {
        origin: [0.0; 3],
        axis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    }
}

fn read_tag(bytes: &[u8], o: usize) -> TagXform {
    TagXform {
        origin: [f32_le(bytes, o + 64), f32_le(bytes, o + 68), f32_le(bytes, o + 72)],
        axis: [
            [f32_le(bytes, o + 76), f32_le(bytes, o + 80), f32_le(bytes, o + 84)],
            [f32_le(bytes, o + 88), f32_le(bytes, o + 92), f32_le(bytes, o + 96)],
            [f32_le(bytes, o + 100), f32_le(bytes, o + 104), f32_le(bytes, o + 108)],
        ],
    }
}

fn xform_point(t: TagXform, p: [f32; 3]) -> [f32; 3] {
    [
        t.origin[0] + t.axis[0][0] * p[0] + t.axis[1][0] * p[1] + t.axis[2][0] * p[2],
        t.origin[1] + t.axis[0][1] * p[0] + t.axis[1][1] * p[1] + t.axis[2][1] * p[2],
        t.origin[2] + t.axis[0][2] * p[0] + t.axis[1][2] * p[1] + t.axis[2][2] * p[2],
    ]
}

fn apply_tag_dir(t: TagXform, p: [f32; 3]) -> [f32; 3] {
    [
        t.axis[0][0] * p[0] + t.axis[1][0] * p[1] + t.axis[2][0] * p[2],
        t.axis[0][1] * p[0] + t.axis[1][1] * p[1] + t.axis[2][1] * p[2],
        t.axis[0][2] * p[0] + t.axis[1][2] * p[1] + t.axis[2][2] * p[2],
    ]
}

fn mul_tag(a: TagXform, b: TagXform) -> TagXform {
    TagXform {
        origin: xform_point(a, b.origin),
        axis: [
            apply_tag_dir(a, b.axis[0]),
            apply_tag_dir(a, b.axis[1]),
            apply_tag_dir(a, b.axis[2]),
        ],
    }
}

fn invert_tag(t: TagXform) -> TagXform {
    let axis = [
        [t.axis[0][0], t.axis[1][0], t.axis[2][0]],
        [t.axis[0][1], t.axis[1][1], t.axis[2][1]],
        [t.axis[0][2], t.axis[1][2], t.axis[2][2]],
    ];
    let inv = TagXform {
        origin: [0.0; 3],
        axis,
    };
    let neg = [-t.origin[0], -t.origin[1], -t.origin[2]];
    TagXform {
        origin: apply_tag_dir(inv, neg),
        axis,
    }
}

struct PosedMesh {
    unique: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    corners: Vec<usize>,
    tex_keys: Vec<String>,
    indices: Vec<u32>,
}

fn pose_player(
    lower: &Md3File,
    upper: &Md3File,
    head: &Md3File,
    legs_f: usize,
    torso_f: usize,
    head_f: usize,
    textures: &Q3TexBank,
    skins: &BTreeMap<String, String>,
) -> Result<PosedMesh, String> {
    let lower_torso = lower.tag(legs_f, "tag_torso").unwrap_or_else(identity_tag);
    let upper_torso = upper.tag(torso_f, "tag_torso").unwrap_or_else(identity_tag);
    let upper_head = upper.tag(torso_f, "tag_head").unwrap_or_else(identity_tag);
    let head_tag = head.tag(head_f, "tag_head").unwrap_or_else(identity_tag);
    let upper_xf = mul_tag(lower_torso, invert_tag(upper_torso));
    let head_xf = mul_tag(mul_tag(upper_xf, upper_head), invert_tag(head_tag));
    let mut out = PosedMesh {
        unique: Vec::new(),
        uvs: Vec::new(),
        corners: Vec::new(),
        tex_keys: Vec::new(),
        indices: Vec::new(),
    };
    emit_md3_posed(&mut out, lower, legs_f, identity_tag(), textures, skins)?;
    emit_md3_posed(&mut out, upper, torso_f, upper_xf, textures, skins)?;
    emit_md3_posed(&mut out, head, head_f, head_xf, textures, skins)?;
    Ok(out)
}

fn pose_weapon(
    gun: &Md3File,
    barrel: Option<&Md3File>,
    textures: &Q3TexBank,
    skins: &BTreeMap<String, String>,
) -> Result<PosedMesh, String> {
    let mut out = PosedMesh {
        unique: Vec::new(),
        uvs: Vec::new(),
        corners: Vec::new(),
        tex_keys: Vec::new(),
        indices: Vec::new(),
    };
    emit_md3_posed(&mut out, gun, 0, identity_tag(), textures, skins)?;
    if let Some(barrel) = barrel {
        let gun_bar = gun.tag(0, "tag_barrel").unwrap_or_else(identity_tag);
        let bar_tag = barrel.tag(0, "tag_barrel").unwrap_or_else(identity_tag);
        let xf = mul_tag(gun_bar, invert_tag(bar_tag));
        emit_md3_posed(&mut out, barrel, 0, xf, textures, skins)?;
    }
    Ok(out)
}

fn emit_md3_posed(
    out: &mut PosedMesh,
    md3: &Md3File,
    frame: usize,
    xf: TagXform,
    textures: &Q3TexBank,
    skins: &BTreeMap<String, String>,
) -> Result<(), String> {
    let bytes = md3.bytes;
    let frame = frame.min(md3.num_frames.saturating_sub(1));
    let mut s = md3.ofs_surfaces;
    for _ in 0..md3.num_surfaces {
        if s + 108 > bytes.len() || &bytes[s..s + 4] != MD3_IDENT {
            return Err("MD3 surface ident".into());
        }
        let surf_name = cstr(&bytes[s + 4..s + 68]);
        let num_shaders = i32_le(bytes, s + 76) as usize;
        let num_verts = i32_le(bytes, s + 80) as usize;
        let num_tris = i32_le(bytes, s + 84) as usize;
        let ofs_tris = i32_le(bytes, s + 88) as usize;
        let ofs_shaders = i32_le(bytes, s + 92) as usize;
        let ofs_st = i32_le(bytes, s + 96) as usize;
        let ofs_xyz = i32_le(bytes, s + 100) as usize;
        let ofs_end = i32_le(bytes, s + 104) as usize;
        if num_verts == 0 || num_tris == 0 {
            if ofs_end > 0 {
                s += ofs_end as usize;
            }
            continue;
        }
        let xyz_stride = num_verts * 8;
        let xyz_off = s + ofs_xyz + frame * xyz_stride;
        if s + ofs_tris + num_tris * 12 > bytes.len()
            || s + ofs_st + num_verts * 8 > bytes.len()
            || xyz_off + xyz_stride > bytes.len()
        {
            return Err("MD3 surface data truncated".into());
        }
        let mut shader = skins
            .get(&surf_name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        if shader.is_empty() && num_shaders > 0 && s + ofs_shaders + 64 <= bytes.len() {
            shader = cstr(&bytes[s + ofs_shaders..s + ofs_shaders + 64]);
        }
        let tex_key = if shader.is_empty() {
            "_default".into()
        } else if textures.resolve(&shader).is_some() {
            normalize_tex_name(&shader)
        } else {
            "_default".into()
        };
        let base = out.unique.len();
        for vi in 0..num_verts {
            let vo = xyz_off + vi * 8;
            let p = [
                i16_le(bytes, vo) as f32 * MD3_XYZ_SCALE,
                i16_le(bytes, vo + 2) as f32 * MD3_XYZ_SCALE,
                i16_le(bytes, vo + 4) as f32 * MD3_XYZ_SCALE,
            ];
            let world = xform_point(xf, p);
            // +X (Q3 forward) → +Z after xform + q3_face_camera, which is
            // the play controller's authored forward. A further 180° flip
            // made them moonwalk: travel was correct, facing was reversed.
            out.unique.push(q3_face_camera(xform(world)));
            let so = s + ofs_st + vi * 8;
            out.uvs.push([f32_le(bytes, so), 1.0 - f32_le(bytes, so + 4)]);
        }
        for t in 0..num_tris {
            let to = s + ofs_tris + t * 12;
            let i0 = i32_le(bytes, to) as usize;
            let i1 = i32_le(bytes, to + 4) as usize;
            let i2 = i32_le(bytes, to + 8) as usize;
            if i0 >= num_verts || i1 >= num_verts || i2 >= num_verts {
                continue;
            }
            let c0 = out.corners.len();
            for vi in [i0, i1, i2] {
                out.corners.push(base + vi);
                out.tex_keys.push(tex_key.clone());
            }
            out.indices
                .extend_from_slice(&[c0 as u32, c0 as u32 + 1, c0 as u32 + 2]);
        }
        if ofs_end <= 0 {
            break;
        }
        s += ofs_end as usize;
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Q3Anim {
    name: String,
    first: i32,
    num: i32,
}

fn parse_animation_cfg(text: &str) -> Vec<Q3Anim> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let (body, comment) = match raw.split_once("//") {
            Some((a, b)) => (a, b),
            None => (raw, ""),
        };
        let cols: Vec<&str> = body.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let (Ok(first), Ok(num), Ok(_looped), Ok(_fps)) = (
            cols[0].parse::<i32>(),
            cols[1].parse::<i32>(),
            cols[2].parse::<i32>(),
            cols[3].parse::<i32>(),
        ) else {
            continue;
        };
        let name = comment
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        out.push(Q3Anim { name, first, num });
    }
    out
}

fn anim_first(anims: &[Q3Anim], name: &str) -> Option<i32> {
    anims.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.first)
}

fn anim_get<'a>(anims: &'a [Q3Anim], name: &str) -> Option<&'a Q3Anim> {
    anims.iter().find(|a| a.name.eq_ignore_ascii_case(name))
}

fn legs_file_frame(anims: &[Q3Anim], first: i32) -> i32 {
    let gesture = anim_first(anims, "TORSO_GESTURE").unwrap_or(0);
    let walkcr = anim_first(anims, "LEGS_WALKCR").unwrap_or(gesture);
    first - (walkcr - gesture)
}

fn push_player_clip(
    out: &mut Vec<(String, Vec<(usize, usize)>)>,
    anims: &[Q3Anim],
    clip: &str,
    legs_name: &str,
    torso_name: &str,
    max_frames: usize,
) {
    let Some(legs) = anim_get(anims, legs_name) else {
        return;
    };
    let torso = anim_get(anims, torso_name);
    let count = (legs.num.max(1) as usize).min(max_frames);
    let mut pairs = Vec::with_capacity(count);
    for i in 0..count {
        let lf = legs_file_frame(anims, legs.first + i as i32).max(0) as usize;
        let uf = if legs_name.starts_with("BOTH_") {
            (legs.first + i as i32).max(0) as usize
        } else {
            torso.map(|t| t.first.max(0) as usize).unwrap_or(0)
        };
        pairs.push((lf, uf));
    }
    if !pairs.is_empty() {
        out.push((clip.into(), pairs));
    }
}

fn md3_normal(enc: u16) -> [f32; 3] {
    let lat = ((enc >> 8) & 0xff) as f32 * std::f32::consts::TAU / 255.0;
    let lng = (enc & 0xff) as f32 * std::f32::consts::TAU / 255.0;
    [lat.cos() * lng.sin(), lat.sin() * lng.sin(), lng.cos()]
}

// ---------------------------------------------------------------------------
// Atlas
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct AtlasSlot {
    uv: [f32; 4],
    #[allow(dead_code)]
    w: u32,
    #[allow(dead_code)]
    h: u32,
}

fn slot_uv(slot: AtlasSlot, local_u: f32, local_v: f32) -> [f32; 2] {
    let u = local_u.rem_euclid(1.0);
    let v = local_v.rem_euclid(1.0);
    let du = slot.uv[2] - slot.uv[0];
    let dv = slot.uv[3] - slot.uv[1];
    [slot.uv[0] + du * u, slot.uv[1] + dv * v]
}

fn lookup_slot(uv: &BTreeMap<String, AtlasSlot>, name: &str) -> AtlasSlot {
    let n = normalize_tex_name(name);
    uv.get(&n)
        .or_else(|| n.strip_prefix("textures/").and_then(|s| uv.get(s)))
        .or_else(|| uv.get(n.rsplit('/').next().unwrap_or(&n)))
        .or_else(|| uv.get("_default"))
        .copied()
        .unwrap_or(AtlasSlot {
            uv: [0.0, 0.0, 1.0, 1.0],
            w: 64,
            h: 64,
        })
}

fn pack_atlas(images: &BTreeMap<String, Q3Image>) -> (Vec<u8>, BTreeMap<String, AtlasSlot>) {
    const G: u32 = 2;
    const MAX: u32 = 4096;
    let mut entries: Vec<(&String, &Q3Image)> = images.iter().collect();
    entries.sort_by(|a, b| b.1.w.cmp(&a.1.w).then(b.1.h.cmp(&a.1.h)));
    let mut placed: BTreeMap<String, (u32, u32, u32, u32)> = BTreeMap::new();
    let mut atlas_w = 64u32;
    let mut atlas_h = 64u32;
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_h = 0u32;
    if let Some(def) = images.get("_default") {
        let w = def.w.max(1) + 2 * G;
        let h = def.h.max(1) + 2 * G;
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
        let w = img.w.max(1) + 2 * G;
        let h = img.h.max(1) + 2 * G;
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
            continue;
        }
        placed.insert((*name).clone(), (x, y, w, h));
        x += w;
        row_h = row_h.max(h);
    }
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];
    let mut uv_map = BTreeMap::new();
    for (name, (px, py, pw, ph)) in &placed {
        let Some(img) = images.get(name) else {
            continue;
        };
        for row in 0..*ph {
            for col in 0..*pw {
                let sx = (col as i32 - G as i32).rem_euclid(img.w.max(1) as i32) as u32;
                let sy = (row as i32 - G as i32).rem_euclid(img.h.max(1) as i32) as u32;
                let si = ((sy * img.w + sx) * 4) as usize;
                let di = (((py + row) * atlas_w + px + col) * 4) as usize;
                if si + 4 <= img.rgba.len() && di + 4 <= rgba.len() {
                    rgba[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
                }
            }
        }
        uv_map.insert(
            name.clone(),
            AtlasSlot {
                uv: [
                    (*px + G) as f32 / atlas_w as f32,
                    (*py + G) as f32 / atlas_h as f32,
                    (*px + G + img.w.max(1)) as f32 / atlas_w as f32,
                    (*py + G + img.h.max(1)) as f32 / atlas_h as f32,
                ],
                w: img.w.max(1),
                h: img.h.max(1),
            },
        );
    }
    if !uv_map.contains_key("_default") {
        uv_map.insert(
            "_default".into(),
            AtlasSlot {
                uv: [0.0, 0.0, 0.02, 0.02],
                w: 64,
                h: 64,
            },
        );
    }
    let png = encode_png_rgba(&rgba, atlas_w, atlas_h).unwrap_or_else(|_| {
        encode_png_rgba(&vec![0x80; 64 * 64 * 4], 64, 64).unwrap()
    });
    (png, uv_map)
}

fn gray_image(w: u32, h: u32) -> Q3Image {
    Q3Image {
        w,
        h,
        rgba: vec![0x80; (w as usize) * (h as usize) * 4],
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stem_slug(rel: &str) -> String {
    let name = rel.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or(&name);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    sanitize_slug(stem)
}

fn path_slug(rel: &str) -> String {
    let n = rel.replace('\\', "/");
    let n = n.rsplit_once('.').map(|(s, _)| s).unwrap_or(&n);
    let parts: Vec<&str> = n.split('/').filter(|s| !s.is_empty()).collect();
    let take = if parts.len() >= 2 {
        &parts[parts.len() - 2..]
    } else {
        &parts[..]
    };
    sanitize_slug(&take.join("-"))
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
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

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn u16_le(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        return 0;
    }
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn i16_le(b: &[u8], o: usize) -> i16 {
    u16_le(b, o) as i16
}

fn u32_le(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn i32_le(b: &[u8], o: usize) -> i32 {
    u32_le(b, o) as i32
}

fn f32_le(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_le(b, o))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_zip_file::{ZipMethod, ZipWriter};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("q3imp-{name}-{nanos}"));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    #[test]
    fn expand_pk3_stored_zip_one_bsp() {
        let mut w = ZipWriter::new();
        w.add("maps/q3dm1.bsp", b"not-a-real-bsp", ZipMethod::Store)
            .unwrap();
        let bytes = w.finish().unwrap();
        let root = tmp_dir("pk3");
        let pk3 = root.join("pak0.pk3");
        std::fs::write(&pk3, bytes).unwrap();
        let out = root.join("out");
        let mut warnings = Vec::new();
        let files = expand_pk3(&pk3, &out, &mut warnings).expect("expand");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].rel, "maps/q3dm1.bsp");
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"not-a-real-bsp");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn safe_rel_rejects_escapes_and_drive_prefixes() {
        assert_eq!(safe_rel("maps/q3dm1.bsp").as_deref(), Some("maps/q3dm1.bsp"));
        assert_eq!(safe_rel("../evil.cfg"), None);
        assert_eq!(safe_rel("/etc/passwd"), None);
        assert_eq!(safe_rel("c:/users/public/evil.exe"), None);
        assert_eq!(safe_rel("c:\\users\\public\\evil.exe"), None);
    }

    #[test]
    fn reject_wrong_bsp_version() {
        let mut bytes = vec![0u8; 8 + LUMP_COUNT * 8];
        bytes[0..4].copy_from_slice(b"IBSP");
        bytes[4..8].copy_from_slice(&38i32.to_le_bytes());
        let staged = tmp_dir("badbsp");
        let err = convert_bsp46(&bytes, "maps/bad.bsp", &staged, &Q3TexBank::default(), "quake3")
            .unwrap_err();
        assert!(
            err.contains("unsupported BSP version 38"),
            "got {err}"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn minimal_bsp46_one_triangle_gltf() {
        let bsp = build_minimal_bsp46();
        let staged = tmp_dir("bsp46");
        let assets = convert_bsp46(
            &bsp,
            "maps/q3tri.bsp",
            &staged,
            &Q3TexBank::default(),
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let world = assets
            .iter()
            .find(|a| a.kind == AssetKind::World)
            .expect("map");
        assert_eq!(world.rel_path, "maps/q3tri.glb");
        let glb = std::fs::read(staged.join(&world.rel_path)).expect("glb");
        assert!(glb.starts_with(b"glTF"), "not a glTF/GLB");
        assert!(glb.len() > 200);
        let place_text =
            std::fs::read_to_string(staged.join("maps/q3tri.place")).expect("place sidecar");
        let place = crate::world_place::WorldPlace::parse(&place_text).expect("parse place");
        assert_eq!(place.source, QUAKE3_SOURCE_ID);
        assert_eq!(place.world, "maps/q3tri");
        assert_eq!(place.places.len(), 1);
        assert_eq!(place.places[0].kind, "player");
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn arched_patch_subdivides_instead_of_two_chords() {
        // Tight 3×3 arch: end posts on the floor, peak 128 units up.
        // Control peak is z=128; the curve peak is z=64. Dense eval
        // must sit on the curve, not the hull.
        let mut ctrl = vec![DrawVert::default(); 9];
        for r in 0..3 {
            for c in 0..3 {
                let x = (c as f32 - 1.0) * 128.0;
                let z = if c == 1 { 128.0 } else { 0.0 };
                ctrl[r * 3 + c].pos = [x, (r as f32 - 1.0) * 16.0, z];
                ctrl[r * 3 + c].normal = [0.0, 0.0, 1.0];
            }
        }
        let mut geom = PartGeom::default();
        emit_patch(&mut geom, &ctrl, 0, 9, 3, 3, -1, None);
        let tris = geom.indices.len() / 3;
        assert!(
            (16..=48).contains(&tris),
            "r_subdivisions=4 90° arch should be ~32 tris, got {tris}"
        );
        assert!(
            geom.positions.len() < tris,
            "patch grid must be indexed, got {} verts for {tris} tris",
            geom.positions.len()
        );
        let max_z = geom
            .positions
            .iter()
            .map(|p| p[1] / SCALE)
            .fold(f32::MIN, f32::max);
        assert!(
            max_z < 80.0,
            "off-curve control peak leaked into the mesh: max_z={max_z}"
        );
        let near_peak = geom
            .positions
            .iter()
            .filter(|p| {
                let x = p[0] / SCALE;
                let z = p[1] / SCALE;
                x.abs() < 4.0 && (z - 64.0).abs() < 4.0
            })
            .count();
        assert!(near_peak > 0, "curve peak (0, 64) missing from tessellation");
    }

    #[test]
    fn inner_arch_mesh_follows_curve_not_control_hull() {
        // q3dm1 `textures/gothic_door/xian_tourneyarch_inside2` 5×3: the
        // interior control row sits at z=380, the true Bezier mid at z=357.
        // Emitting the hull is the dark faceted cavity in the doorway.
        let xs = [572.0f32, 572.0, 672.0, 772.0, 772.0];
        let zs = [288.0f32, 380.0, 380.0, 380.0, 288.0];
        let mut pts = vec![DrawVert::default(); 15];
        for r in 0..3 {
            for c in 0..5 {
                pts[r * 5 + c].pos = [xs[c], 1664.0 - r as f32 * 236.0, zs[c]];
            }
        }
        let mut geom = PartGeom::default();
        emit_patch(&mut geom, &pts, 0, 15, 5, 3, -1, None);
        // Engine Y is Q3 Z. Control posts (572,380)/(772,380) are off-curve;
        // the crown (672,380) and mids (~597,357)/(~747,357) are on it.
        let xz: Vec<(f32, f32)> = geom
            .positions
            .iter()
            .map(|p| (p[0] / SCALE, p[1] / SCALE))
            .collect();
        // Tangent at t=0 is vertical, so on-curve samples stay near x=572
        // while z climbs. Only the control posts themselves are off-curve.
        let leaked_posts = xz
            .iter()
            .filter(|(x, z)| {
                ((*x - 572.0).abs() < 1.5 && (*z - 380.0).abs() < 1.5)
                    || ((*x - 772.0).abs() < 1.5 && (*z - 380.0).abs() < 1.5)
            })
            .count();
        assert_eq!(
            leaked_posts, 0,
            "off-curve control posts leaked into the mesh"
        );
        // The C-cavity is the control shelf across the corners. On the
        // curve, z stays below ~371 for x<620 (and x>724).
        let left_shelf = xz.iter().filter(|(x, z)| *x < 620.0 && *z > 375.0).count();
        let right_shelf = xz.iter().filter(|(x, z)| *x > 724.0 && *z > 375.0).count();
        assert_eq!(
            left_shelf + right_shelf,
            0,
            "z=380 control shelf leaked into the corners (left={left_shelf} right={right_shelf})"
        );
        // t=0.5 of each cell is (597,357)/(747,357). Level 3 samples
        // 0, 1/3, 2/3, 1 — still on the curve, just not at that exact mid.
        let left_rise = xz
            .iter()
            .any(|(x, z)| *x > 575.0 && *x < 620.0 && *z > 320.0 && *z < 373.0);
        let right_rise = xz
            .iter()
            .any(|(x, z)| *x > 724.0 && *x < 769.0 && *z > 320.0 && *z < 373.0);
        assert!(
            left_rise && right_rise,
            "on-curve rise missing (left={left_rise} right={right_rise})"
        );
        let tris = geom.indices.len() / 3;
        assert!(
            (16..=80).contains(&tris),
            "on-curve r_subdivisions=4 should be a few dozen tris, got {tris}"
        );
        assert!(
            geom.positions.len() < tris,
            "patch grid must be indexed, got {} verts for {tris} tris",
            geom.positions.len()
        );
    }

    #[test]
    fn radiant_ibevel_fills_the_spandrel() {
        // Radiant IBevel 3×3: p1 repeated 7×. The fill is the triangle
        // from the elbow (0,64) to the curve (0,0)→(64,64), not a C
        // along that curve.
        let p0 = [0.0f32, 0.0, 0.0];
        let p1 = [0.0, 0.0, 64.0];
        let p2 = [64.0, 0.0, 64.0];
        let net = [p0, p1, p1, p1, p1, p1, p2, p1, p1];
        let mut ctrl = vec![DrawVert::default(); 9];
        for (i, p) in net.iter().enumerate() {
            ctrl[i].pos = *p;
        }
        let mut geom = PartGeom::default();
        emit_patch(&mut geom, &ctrl, 0, 9, 3, 3, -1, None);
        assert!(
            geom.indices.len() >= 3,
            "IBevel emitted no triangles"
        );
        let mut covered = false;
        for tri in geom.indices.chunks_exact(3) {
            let pts: Vec<(f32, f32)> = tri
                .iter()
                .map(|&i| {
                    let p = geom.positions[i as usize];
                    (p[0] / SCALE, p[1] / SCALE)
                })
                .collect();
            let cx = (pts[0].0 + pts[1].0 + pts[2].0) / 3.0;
            let cz = (pts[0].1 + pts[1].1 + pts[2].1) / 3.0;
            if cx < 32.0 && cz > 32.0 {
                covered = true;
                break;
            }
        }
        assert!(
            covered,
            "IBevel lost the spandrel (only the inner curve remains)"
        );
    }

    #[test]
    fn shader_detail_stage_is_not_the_albedo() {
        let mut bank = Q3TexBank::default();
        parse_shader_aliases(
            r#"
textures/gothic_block/blocks17_sandy
{
qer_editorimage textures/gothic_block/blocks17.tga
{
map $lightmap
rgbGen identity
}
{
map textures/gothic_block/sand2.tga
blendfunc GL_DST_COLOR GL_SRC_COLOR
detail
tcMod scale 2.90 2.234
}
{
map textures/gothic_block/blocks17.tga
tcMod scale 0.25 0.25
blendfunc GL_DST_COLOR GL_ZERO
}
}
"#,
            &mut bank,
        );
        assert_eq!(
            bank.aliases.get("textures/gothic_block/blocks17_sandy").map(|s| s.as_str()),
            Some("textures/gothic_block/blocks17")
        );
        let d = bank
            .detail_for("textures/gothic_block/blocks17_sandy")
            .expect("detail stage");
        assert_eq!(d.map, "textures/gothic_block/sand2");
        assert!((d.scale[0] - 2.90).abs() < 1e-4);
        assert!((d.scale[1] - 2.234).abs() < 1e-4);
    }

    #[test]
    fn animmap_becomes_the_albedo() {
        let mut bank = Q3TexBank::default();
        parse_shader_aliases(
            r#"
textures/sfx/flame1side
{
surfaceparm trans
cull none
{
animMap 10 textures/sfx/flame1.tga textures/sfx/flame2.tga
blendFunc GL_ONE GL_ONE
}
{
animMap 10 textures/sfx/flame2.tga textures/sfx/flame1.tga
blendFunc GL_ONE GL_ONE
}
{
map textures/sfx/flameball.tga
blendFunc GL_ONE GL_ONE
}
}
"#,
            &mut bank,
        );
        assert_eq!(
            bank.aliases.get("textures/sfx/flame1side").map(|s| s.as_str()),
            Some("textures/sfx/flame1")
        );
        let s = bank
            .surface_for("textures/sfx/flame1side")
            .expect("additive flame surface");
        assert!(s.additive);
        assert_eq!(s.albedo, "textures/sfx/flame1");
    }

    #[test]
    fn alpha_stage_keeps_the_scrolled_underlayer() {
        let mut bank = Q3TexBank::default();
        parse_shader_aliases(
            r#"
textures/gothic_floor/largerblock3b_ow
{
{
map textures/sfx/firegorre.tga
tcmod scroll 0 1
tcmod scale 4 4
blendFunc GL_ONE GL_ZERO
}
{
map textures/gothic_floor/largerblock3b_ow.tga
blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
}
{
map $lightmap
blendFunc GL_DST_COLOR GL_ONE_MINUS_DST_ALPHA
}
}
"#,
            &mut bank,
        );
        assert_eq!(
            bank.aliases
                .get("textures/gothic_floor/largerblock3b_ow")
                .map(|s| s.as_str()),
            Some("textures/gothic_floor/largerblock3b_ow")
        );
        let s = bank
            .surface_for("textures/gothic_floor/largerblock3b_ow")
            .expect("blood puddle surface");
        assert_eq!(s.under.as_deref(), Some("textures/sfx/firegorre"));
        assert!((s.under_scale[0] - 4.0).abs() < 1e-4);
        assert!(!s.additive);
    }

    #[test]
    fn bake_composites_under_alpha_and_stays_opaque() {
        let mut bank = Q3TexBank::default();
        let mut over = vec![0u8; 4 * 4];
        // checker: opaque stone / punched hole
        over[0..4].copy_from_slice(&[80, 70, 60, 255]);
        over[4..8].copy_from_slice(&[80, 70, 60, 0]);
        over[8..12].copy_from_slice(&[80, 70, 60, 0]);
        over[12..16].copy_from_slice(&[80, 70, 60, 255]);
        bank.images.insert(
            "textures/gothic_floor/largerblock3b_ow".into(),
            Q3Image {
                w: 2,
                h: 2,
                rgba: over,
            },
        );
        bank.images.insert(
            "textures/sfx/firegorre".into(),
            Q3Image {
                w: 1,
                h: 1,
                rgba: vec![180, 20, 20, 255],
            },
        );
        parse_shader_aliases(
            r#"
textures/gothic_floor/largerblock3b_ow
{
{
map textures/sfx/firegorre.tga
blendFunc GL_ONE GL_ZERO
}
{
map textures/gothic_floor/largerblock3b_ow.tga
blendFunc blend
}
}
"#,
            &mut bank,
        );
        let img = bank
            .bake("textures/gothic_floor/largerblock3b_ow")
            .expect("bake");
        assert_eq!(img.rgba[3], 255, "stone stays opaque");
        assert_eq!(&img.rgba[4..8], &[180, 20, 20, 255], "hole shows blood");
        assert!(img.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn bake_additive_fire_punches_black() {
        let mut bank = Q3TexBank::default();
        bank.images.insert(
            "textures/sfx/flame1".into(),
            Q3Image {
                w: 1,
                h: 2,
                rgba: vec![0, 0, 0, 255, 255, 180, 20, 255],
            },
        );
        parse_shader_aliases(
            r#"
textures/sfx/flame1side
{
{
animMap 10 textures/sfx/flame1.tga textures/sfx/flame2.tga
blendFunc GL_ONE GL_ONE
}
}
"#,
            &mut bank,
        );
        let img = bank.bake("textures/sfx/flame1side").expect("bake");
        assert_eq!(img.rgba[3], 0, "black fire background is transparent");
        assert_eq!(img.rgba[7], 255, "bright fire stays solid");
    }

    #[test]
    fn bake_dim_flame_survives_alpha_discard() {
        let mut bank = Q3TexBank::default();
        bank.images.insert(
            "textures/sfx/flame1".into(),
            Q3Image {
                w: 1,
                h: 1,
                rgba: vec![40, 18, 6, 255],
            },
        );
        parse_shader_aliases(
            r#"
textures/sfx/flame1_hell
{
cull none
{
animMap 10 textures/sfx/flame1.tga textures/sfx/flame2.tga
blendFunc GL_ONE GL_ONE
}
}
"#,
            &mut bank,
        );
        let s = bank.surface_for("textures/sfx/flame1_hell").expect("surf");
        assert!(s.two_sided);
        let img = bank.bake("textures/sfx/flame1_hell").expect("bake");
        assert!(img.rgba[3] >= 128, "dim fire must beat tex.w < 0.5 discard");
        assert!(img.rgba[0] >= 80, "dim fire must be lifted so it reads");
    }

    #[test]
    fn animation_cfg_parses_legs_shift() {
        let text = "0\t30\t0\t20\t\t// BOTH_DEATH1\r\n90\t40\t0\t18\t\t// TORSO_GESTURE\r\n151\t1\t0\t15\t\t// TORSO_STAND\r\n153\t8\t8\t20\t\t// LEGS_WALKCR\r\n161\t12\t12\t20\t\t// LEGS_WALK\r\n229\t10\t10\t15\t\t// LEGS_IDLE\r\n";
        let anims = parse_animation_cfg(text);
        assert_eq!(anim_first(&anims, "TORSO_STAND"), Some(151));
        assert_eq!(legs_file_frame(&anims, 229), 166);
        assert_eq!(legs_file_frame(&anims, 161), 98);
    }

    #[test]
    fn md3_header_uses_num_frames_not_flags() {
        let bytes = build_minimal_md3();
        let staged = tmp_dir("md3hdr");
        let src = staged.join("models/players/sarge/lower.md3");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, &bytes).unwrap();
        let asset = convert_md3(
            &bytes,
            "models/players/sarge/lower.md3",
            &src,
            &staged,
            QUAKE3_SOURCE_ID,
            &Q3TexBank::default(),
        )
        .expect("md3 with flags=0 must still convert");
        assert_eq!(asset.kind, AssetKind::Character);
        let glb = std::fs::read(staged.join(&asset.rel_path)).expect("glb");
        assert!(glb.starts_with(b"glTF"), "not a glTF/GLB");
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn demo_pk3_expands_maps_and_player_md3() {
        let pk3 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/quake3/demoq3/pak0.pk3");
        if !pk3.is_file() {
            return;
        }
        let root = tmp_dir("demopk3");
        let out = root.join("pk3");
        let mut warnings = Vec::new();
        let files = expand_pk3(&pk3, &out, &mut warnings).expect("expand demo pk3");
        let md3_ok = warnings
            .iter()
            .filter(|w| w.contains(".md3") && w.contains("extract"))
            .count();
        let bsp_ok = files.iter().filter(|f| f.rel.ends_with(".bsp")).count();
        let md3_n = files.iter().filter(|f| f.rel.ends_with(".md3")).count();
        assert!(
            bsp_ok >= 4 && md3_n >= 20,
            "expected maps+models, got bsp={bsp_ok} md3={md3_n} extract_fail={md3_ok} warn={}",
            warnings.len()
        );
        let mut bank = load_tex_bank(&out);
        apply_shader_aliases(&mut bank, &out);
        assert!(
            bank.images.len() > 50,
            "demo jpg/tga bank too small: {}",
            bank.images.len()
        );
        assert!(
            bank.aliases.contains_key("textures/sfx/flame1side"),
            "animMap flame1side must alias to a flame frame"
        );
        assert!(
            bank.surface_for("textures/gothic_floor/largerblock3b_ow")
                .and_then(|s| s.under.as_ref())
                .is_some(),
            "blood puddle must keep the firegorre underlayer"
        );
        let bsp = files
            .iter()
            .find(|f| f.rel == "maps/q3dm1.bsp")
            .expect("q3dm1");
        let bytes = std::fs::read(&bsp.path).unwrap();
        let staged = root.join("staged");
        let worlds = convert_bsp46(&bytes, &bsp.rel, &staged, &bank, QUAKE3_SOURCE_ID)
            .expect("q3dm1 convert");
        assert!(worlds.iter().any(|a| a.kind == AssetKind::World));
        let glb_bytes = std::fs::read(staged.join("maps/q3dm1.glb")).expect("q3dm1 glb");
        assert!(
            glb_bytes.len() < 22_000_000,
            "q3dm1 GLB still bloated after r_subdivisions=4: {} bytes",
            glb_bytes.len()
        );
        let place_text =
            std::fs::read_to_string(staged.join("maps/q3dm1.place")).expect("q3dm1 place");
        let place = crate::world_place::WorldPlace::parse(&place_text).expect("parse q3dm1 place");
        assert!(
            place.places.iter().any(|p| p.kind == "weapon"),
            "q3dm1 should place weapons"
        );
        assert!(
            place
                .places
                .iter()
                .any(|p| p.class == "misc_model" && p.asset.starts_with("props/")),
            "q3dm1 should place misc_model props"
        );
        // ---- the contract, on the real map ----------------------------
        let nav = crate::world_nav::WorldNav::parse(
            &std::fs::read_to_string(staged.join("maps/q3dm1.spawn")).expect("q3dm1 spawn"),
        )
        .expect("parse q3dm1 spawn");
        // q3dm1 has SIX `info_player_deathmatch` and no `info_player_start`
        // at all, so — exactly as the Quake 1 converter does — the first
        // deathmatch spot leads and the sidecar header names somewhere a
        // walker can stand.
        assert!(
            nav.starts.len() >= 6,
            "q3dm1 publishes every spawn, got {:?}",
            nav.starts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
        );
        assert!(nav.starts.iter().any(|s| s.name == "deathmatch_1"));
        assert!(nav.anchors().iter().any(|a| a.name == "deathmatch_1"));
        assert!(nav.floor_y.is_some() && nav.eye_height.is_some());
        // …and it is a single-brush-model map: no door, no plat, no
        // teleporter. Asserting one here would be asserting a fiction.
        assert!(
            nav.doors.is_empty() && nav.lifts.is_empty() && nav.teleports.is_empty(),
            "q3dm1 has one brush model and no movers"
        );
        let root_json = makepad_asset_client::json::parse(glb_json(&glb_bytes).as_bytes())
            .expect("q3dm1 glb json");
        // Its sky (`textures/skies/tim_hell`) is a `q3map_sun` cloud-layer
        // sky: `skyparms - 384 -` names no box, and the demo pak0.pk3 ships
        // no `env/` directory at all, so the lid stays where id drew one.
        assert!(
            !root_json
                .get("nodes")
                .unwrap()
                .as_arr()
                .unwrap()
                .iter()
                .any(|n| n.get("name").and_then(makepad_asset_client::json::Value::as_str)
                    == Some("sky")),
            "no farbox, no sky node"
        );
        // Every level material declares the map PRELIT: q3dm1 has a lightmap
        // atlas, so the marker points at it.
        let materials = root_json.get("materials").unwrap().as_arr().unwrap();
        let level_prims = root_json.get("meshes").unwrap().as_arr().unwrap()[0]
            .get("primitives")
            .unwrap()
            .as_arr()
            .unwrap()
            .to_vec();
        for p in &level_prims {
            let mi = p
                .get("material")
                .and_then(makepad_asset_client::json::Value::as_i64)
                .expect("material") as usize;
            assert!(
                materials[mi]
                    .get("extras")
                    .and_then(|e| e.get("lightmapTexture"))
                    .is_some(),
                "a prelit map must never be lit twice"
            );
        }
        // `q3map` runs its own T-junction fix, so a Q3 BSP arrives welded and
        // this converter adds no weld pass. Measure it rather than assume it.
        let parts = crate::world_preview::extract_glb_parts(&glb_bytes).expect("parts");
        let soup: Vec<(&[[f32; 3]], &[u32])> = parts
            .iter()
            .map(|part| (&part.pos[..], &part.indices[..]))
            .collect();
        let left = crate::classic_import::weld_t_junctions_left(&soup);
        eprintln!(
            "q3dm1: {} parts, {} triangles, {left} T-junctions",
            parts.len(),
            soup.iter().map(|(_, i)| i.len() / 3).sum::<usize>()
        );
        // 668 before the weld, 2 after. They were never brush seams —
        // `q3map` really did weld those — they are where this converter's
        // own patch tessellation meets the brushwork it abuts: `emit_patch`
        // picks a subdivision level per patch and `collapse_patch_grid`
        // drops chordal rows, neither of which the neighbouring polygon
        // knows about. Welding does not fight q3map, it finishes what this
        // converter started. The two that survive are corner pairs further
        // apart than the merge tolerance, which is a physical six
        // millimetres and does not stretch for a coarser source grid. A
        // ratchet: it may only fall.
        assert!(
            left <= 2,
            "q3dm1 T-junctions rose to {left} (ratchet 2) — \
             surfaces crack where they meet"
        );

        // q3dm7 is the demo map that HAS movers: two `func_door`s.
        if let Some(bsp) = files.iter().find(|f| f.rel == "maps/q3dm7.bsp") {
            let bytes = std::fs::read(&bsp.path).unwrap();
            convert_bsp46(&bytes, &bsp.rel, &staged, &bank, QUAKE3_SOURCE_ID).expect("q3dm7");
            let dm7 = std::fs::read(staged.join("maps/q3dm7.glb")).expect("q3dm7 glb");
            let dm7_json = makepad_asset_client::json::parse(glb_json(&dm7).as_bytes())
                .expect("q3dm7 glb json");
            let names: Vec<String> = dm7_json
                .get("nodes")
                .unwrap()
                .as_arr()
                .unwrap()
                .iter()
                .filter_map(|n| {
                    n.get("name")
                        .and_then(makepad_asset_client::json::Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            assert!(
                names.iter().filter(|n| n.starts_with("door_")).count() == 2,
                "q3dm7 has two func_doors, got {names:?}"
            );
            assert!(
                names.iter().any(|n| n.starts_with("hazard_")),
                "q3dm7's lava is a hazard node, got {names:?}"
            );
            let clips: Vec<String> = dm7_json
                .get("animations")
                .unwrap()
                .as_arr()
                .unwrap()
                .iter()
                .filter_map(|a| {
                    a.get("name")
                        .and_then(makepad_asset_client::json::Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            assert!(clips.contains(&"door_1".to_string()), "{clips:?}");
            assert!(clips.contains(&"door_2".to_string()), "{clips:?}");
            let dm7_nav = crate::world_nav::WorldNav::parse(
                &std::fs::read_to_string(staged.join("maps/q3dm7.spawn")).expect("q3dm7 spawn"),
            )
            .expect("parse q3dm7 spawn");
            assert_eq!(dm7_nav.doors.len(), 2, "{:?}", dm7_nav.doors);
            assert_eq!(dm7_nav.teleports.len(), 1, "{:?}", dm7_nav.teleports);
            for want in ["door_1", "door_2", "teleport_1"] {
                assert!(
                    dm7_nav.anchors().iter().any(|a| a.name == want),
                    "missing anchor {want}"
                );
            }
        }

        let lower = files
            .iter()
            .find(|f| f.rel == "models/players/sarge/lower.md3")
            .expect("sarge lower");
        let bytes = std::fs::read(&lower.path).unwrap();
        let asset = convert_md3(
            &bytes,
            &lower.rel,
            &lower.path,
            &staged,
            QUAKE3_SOURCE_ID,
            &bank,
        )
        .expect("sarge lower");
        assert_eq!(asset.kind, AssetKind::Character);
        let md3s: Vec<(PathBuf, String)> = files
            .iter()
            .filter(|f| f.rel.ends_with(".md3"))
            .map(|f| (f.path.clone(), f.rel.clone()))
            .collect();
        let (assembled, drop) =
            assemble_players_and_weapons(&md3s, &staged, QUAKE3_SOURCE_ID, &bank);
        assert!(
            assembled.iter().any(|a| a.key == "characters/sarge"),
            "sarge assembled, got {:?}",
            assembled.iter().map(|a| a.key.as_str()).collect::<Vec<_>>()
        );
        assert!(drop.contains("characters/sarge-lower"));
        assert!(
            assembled.iter().any(|a| a.key == "weapons/shotgun"),
            "shotgun assembled"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tga_2x2_decode() {
        let mut tga = vec![0u8; 18 + 2 * 2 * 3];
        tga[2] = 2; // uncompressed truecolor
        tga[12..14].copy_from_slice(&2u16.to_le_bytes());
        tga[14..16].copy_from_slice(&2u16.to_le_bytes());
        tga[16] = 24;
        tga[17] = 0x20; // top-left origin
        // BGR pixels: red, green / blue, white
        let px = [
            0u8, 0, 255, // red
            0, 255, 0, // green
            255, 0, 0, // blue
            255, 255, 255, // white
        ];
        tga[18..].copy_from_slice(&px);
        let (rgba, w, h) = decode_tga(&tga).expect("tga");
        assert_eq!((w, h), (2, 2));
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(&rgba[8..12], &[0, 0, 255, 255]);
        assert_eq!(&rgba[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn tga_32bit_unused_alpha_is_opaque() {
        let mut tga = vec![0u8; 18 + 2 * 2 * 4];
        tga[2] = 2;
        tga[12..14].copy_from_slice(&2u16.to_le_bytes());
        tga[14..16].copy_from_slice(&2u16.to_le_bytes());
        tga[16] = 32;
        tga[17] = 0x20;
        // BGRA: red with alpha 0 (Q3 "unused channel" convention).
        tga[18..].copy_from_slice(&[0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0]);
        let (rgba, w, h) = decode_tga(&tga).expect("tga");
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgba[3], 255, "all-zero alpha must not punch the mesh");
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
    }

    /// One structure for every map: whatever a map HAS, it publishes the same
    /// way, and what it does not have it does not fake.
    #[test]
    fn all_demo_maps_publish_the_same_map_contract() {
        use crate::world_nav::WorldNav;
        use makepad_asset_client::json::Value;
        let pk3 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/quake3/demoq3/pak0.pk3");
        if !pk3.is_file() {
            return;
        }
        let root = tmp_dir("q3contract");
        let out = root.join("pk3");
        let mut warnings = Vec::new();
        let files = expand_pk3(&pk3, &out, &mut warnings).expect("expand");
        let mut bank = load_tex_bank(&out);
        apply_shader_aliases(&mut bank, &out);
        let staged = root.join("staged");
        // The demo pack ships NO `env/` directory, so no map in it can build
        // an equirect sky. If that ever changes, this is where it shows.
        assert!(
            !out.join("env").is_dir(),
            "the demo pak0.pk3 has no skybox faces"
        );
        // (map, doors, lifts, teleports, starts at least)
        let expect = [
            ("q3dm1", 0usize, 0usize, 0usize, 6usize),
            ("q3dm7", 2, 0, 1, 14),
            ("q3dm17", 0, 0, 3, 11),
            ("q3tourney2", 0, 0, 2, 8),
        ];
        for (slug, doors, lifts, teleports, starts) in expect {
            let rel = format!("maps/{slug}.bsp");
            let Some(bsp) = files.iter().find(|f| f.rel == rel) else {
                panic!("{rel} missing from the demo pk3");
            };
            let bytes = std::fs::read(&bsp.path).unwrap();
            convert_bsp46(&bytes, &bsp.rel, &staged, &bank, QUAKE3_SOURCE_ID).expect(&rel);
            let glb = std::fs::read(staged.join(format!("maps/{slug}.glb"))).expect("glb");
            let json = makepad_asset_client::json::parse(glb_json(&glb).as_bytes()).expect("json");
            let names: Vec<String> = json
                .get("nodes")
                .unwrap()
                .as_arr()
                .unwrap()
                .iter()
                .filter_map(|n| n.get("name").and_then(Value::as_str).map(str::to_string))
                .collect();
            let count = |prefix: &str| names.iter().filter(|n| n.starts_with(prefix)).count();
            assert_eq!(count("door_"), doors, "{slug} doors: {names:?}");
            assert_eq!(count("lift_"), lifts, "{slug} lifts: {names:?}");
            assert!(
                !names.iter().any(|n| n == "sky"),
                "{slug}: every demo sky is a cloud-layer sky with no farbox"
            );
            let nav = WorldNav::parse(
                &std::fs::read_to_string(staged.join(format!("maps/{slug}.spawn")))
                    .expect("spawn sidecar"),
            )
            .expect("parse spawn");
            assert_eq!(nav.doors.len(), doors, "{slug} nav doors");
            assert_eq!(nav.lifts.len(), lifts, "{slug} nav lifts");
            assert_eq!(nav.teleports.len(), teleports, "{slug} nav teleports");
            assert!(
                nav.starts.len() >= starts,
                "{slug} starts: {:?}",
                nav.starts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
            );
            // Heights are the same engine constants everywhere.
            assert!((nav.eye_height.unwrap() - (VIEW_HEIGHT + ORIGIN_ABOVE_FLOOR) * SCALE).abs() < 1e-4);
            assert!((nav.step_height.unwrap() - STEP_HEIGHT * SCALE).abs() < 1e-4);
            assert!(nav.floor_y.is_some(), "{slug} floor");
            // Q3 has no exit and no keys: nothing is invented.
            assert!(nav.markers.is_empty(), "{slug} markers");
            // Every mover and pad reaches the catalog under its own name.
            let anchors = nav.anchors();
            for d in nav.doors.iter().chain(&nav.lifts) {
                assert!(anchors.iter().any(|a| a.name == d.name), "{slug} {}", d.name);
            }
            for t in &nav.teleports {
                assert!(anchors.iter().any(|a| a.name == t.name), "{slug} {}", t.name);
            }
            eprintln!(
                "{slug}: {} nodes, {doors} doors, {lifts} lifts, {} hazards, {teleports} teleports, {} starts",
                names.len(),
                count("hazard_"),
                nav.starts.len()
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reconvert_local_q3dm1_work_source() {
        let pk3 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/quake3/demoq3/pak0.pk3");
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/ai_content_app/import/quake3/work/source");
        if !pk3.is_file() || (!staged.join("worlds").is_dir() && !staged.join("maps").is_dir()) {
            return;
        }
        let root = tmp_dir("q3dm1bake");
        let out = root.join("pk3");
        let mut warnings = Vec::new();
        let files = expand_pk3(&pk3, &out, &mut warnings).expect("expand");
        let mut bank = load_tex_bank(&out);
        apply_shader_aliases(&mut bank, &out);
        for rel in [
            "maps/q3dm1.bsp",
            "maps/q3dm7.bsp",
            "maps/q3dm17.bsp",
            "maps/q3tourney2.bsp",
        ] {
            let Some(bsp) = files.iter().find(|f| f.rel == rel) else {
                continue;
            };
            let bytes = std::fs::read(&bsp.path).unwrap();
            convert_bsp46(&bytes, &bsp.rel, &staged, &bank, QUAKE3_SOURCE_ID)
                .expect(rel);
        }
        let glb = staged.join("maps/q3dm1.glb");
        assert!(glb.is_file(), "q3dm1.glb missing after bake reconvert");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dxt1_4x4_solid_red() {
        let mut dds = vec![0u8; 128 + 8];
        dds[0..4].copy_from_slice(b"DDS ");
        dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        dds[12..16].copy_from_slice(&4u32.to_le_bytes());
        dds[16..20].copy_from_slice(&4u32.to_le_bytes());
        dds[76..80].copy_from_slice(&32u32.to_le_bytes());
        dds[80..84].copy_from_slice(&4u32.to_le_bytes()); // DDPF_FOURCC
        dds[84..88].copy_from_slice(b"DXT1");
        // color0 = red 565 0xF800, color1 = 0, all indices 0
        dds[128..130].copy_from_slice(&0xF800u16.to_le_bytes());
        dds[130..132].copy_from_slice(&0u16.to_le_bytes());
        let (rgba, w, h) = decode_dds(&dds).expect("dds");
        assert_eq!((w, h), (4, 4));
        assert!(rgba[0] > 240 && rgba[1] < 16 && rgba[2] < 16, "{:?}", &rgba[0..4]);
        assert_eq!(rgba[3], 255);
    }

    struct ShaderSpec {
        name: &'static str,
        surface: i32,
        contents: i32,
    }

    /// One convex polygon face, wound as the BSP would: positions in Q3 map
    /// space (Z-up, map units), tiling `st`.
    struct FaceSpec {
        tex: usize,
        verts: Vec<([f32; 3], [f32; 2])>,
    }

    struct ModelSpec {
        mins: [f32; 3],
        maxs: [f32; 3],
        first_face: usize,
        n_faces: usize,
    }

    /// A hand-built IBSP 46 with the six lumps this converter reads. Every
    /// contract fixture below is one of these, so a CI machine with no `.pk3`
    /// still exercises doors, lifts, hazards, the sky and the `.spawn`.
    fn build_bsp46(
        entities: &str,
        shaders: &[ShaderSpec],
        faces: &[FaceSpec],
        models: &[ModelSpec],
    ) -> Vec<u8> {
        let mut ent = entities.as_bytes().to_vec();
        ent.push(0);
        let mut tex = Vec::new();
        for s in shaders {
            let mut t = vec![0u8; TEX_SIZE];
            let n = s.name.as_bytes();
            assert!(n.len() < 64, "shader name too long");
            t[..n.len()].copy_from_slice(n);
            write_i32(&mut t, 64, s.surface);
            write_i32(&mut t, 68, s.contents);
            tex.extend_from_slice(&t);
        }
        let mut verts = Vec::new();
        let mut idx = Vec::new();
        let mut face_lump = Vec::new();
        let mut first_vert = 0i32;
        let mut first_idx = 0i32;
        for f in faces {
            for (pos, st) in &f.verts {
                let mut v = vec![0u8; VERT_SIZE];
                write_f32(&mut v, 0, pos[0]);
                write_f32(&mut v, 4, pos[1]);
                write_f32(&mut v, 8, pos[2]);
                write_f32(&mut v, 12, st[0]);
                write_f32(&mut v, 16, st[1]);
                write_f32(&mut v, 28, 0.0);
                write_f32(&mut v, 32, 0.0);
                write_f32(&mut v, 36, 1.0);
                v[40..44].copy_from_slice(&[255, 255, 255, 255]);
                verts.extend_from_slice(&v);
            }
            // Meshverts are face-relative, which is what `resolve_meshvert`
            // expects for anything under `n_vertexes`.
            let n = f.verts.len() as i32;
            let mut count = 0i32;
            for i in 1..n - 1 {
                for k in [0, i, i + 1] {
                    idx.extend_from_slice(&k.to_le_bytes());
                    count += 1;
                }
            }
            let mut face = vec![0u8; FACE_SIZE];
            write_i32(&mut face, 0, f.tex as i32);
            write_i32(&mut face, 4, -1); // effect
            write_i32(&mut face, 8, FACE_POLYGON);
            write_i32(&mut face, 12, first_vert);
            write_i32(&mut face, 16, n);
            write_i32(&mut face, 20, first_idx);
            write_i32(&mut face, 24, count);
            write_i32(&mut face, 28, -1); // no lightmap page
            face_lump.extend_from_slice(&face);
            first_vert += n;
            first_idx += count;
        }
        let mut model_lump = Vec::new();
        for m in models {
            let mut b = vec![0u8; MODEL_SIZE];
            for k in 0..3 {
                write_f32(&mut b, k * 4, m.mins[k]);
                write_f32(&mut b, 12 + k * 4, m.maxs[k]);
            }
            write_i32(&mut b, 24, m.first_face as i32);
            write_i32(&mut b, 28, m.n_faces as i32);
            model_lump.extend_from_slice(&b);
        }

        let header = 8 + LUMP_COUNT * 8;
        let mut lumps = [(0usize, 0usize); LUMP_COUNT];
        let mut off = header;
        let payloads = [
            (LUMP_ENTITIES, ent.as_slice()),
            (LUMP_TEXTURES, tex.as_slice()),
            (LUMP_MODELS, model_lump.as_slice()),
            (LUMP_VERTICES, verts.as_slice()),
            (LUMP_MESHVERTS, idx.as_slice()),
            (LUMP_FACES, face_lump.as_slice()),
        ];
        for (i, payload) in payloads {
            lumps[i] = (off, payload.len());
            off += payload.len();
        }

        let mut out = vec![0u8; off];
        out[0..4].copy_from_slice(b"IBSP");
        out[4..8].copy_from_slice(&46i32.to_le_bytes());
        for (i, (lo, llen)) in lumps.iter().enumerate() {
            let o = 8 + i * 8;
            out[o..o + 4].copy_from_slice(&(*lo as u32).to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&(*llen as u32).to_le_bytes());
        }
        let mut cur = header;
        for (_, payload) in payloads {
            out[cur..cur + payload.len()].copy_from_slice(payload);
            cur += payload.len();
        }
        out
    }

    fn build_minimal_bsp46() -> Vec<u8> {
        build_bsp46(
            "{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 64\"\n}\n",
            &[ShaderSpec {
                name: "textures/test/a",
                surface: 0,
                contents: 1,
            }],
            // Triangle in XY (Z-up): (0,0,0) (64,0,0) (0,64,0)
            // → engine (0,0,0) (1,0,0) (0,0,-1)
            &[FaceSpec {
                tex: 0,
                verts: vec![
                    ([0.0, 0.0, 0.0], [0.0, 0.0]),
                    ([64.0, 0.0, 0.0], [1.0, 0.0]),
                    ([0.0, 64.0, 0.0], [0.0, 1.0]),
                ],
            }],
            &[ModelSpec {
                mins: [0.0; 3],
                maxs: [64.0, 64.0, 0.0],
                first_face: 0,
                n_faces: 1,
            }],
        )
    }

    // -----------------------------------------------------------------------
    // The unified map contract
    // -----------------------------------------------------------------------

    /// A door on model `*1`, a plat on `*2`, a teleport trigger on `*3`, a sky
    /// face, a lava face, and three spawns.
    fn build_contract_bsp46() -> Vec<u8> {
        let entities = concat!(
            "{\n\"classname\" \"worldspawn\"\n\"message\" \"Contract Arena\"\n",
            "\"music\" \"music\\test.wav\"\n}\n",
            "{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 64\"\n\"angle\" \"90\"\n}\n",
            "{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"128 0 64\"\n",
            "\"angle\" \"180\"\n}\n",
            "{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"256 0 64\"\n}\n",
            "{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n}\n",
            "{\n\"classname\" \"func_plat\"\n\"model\" \"*2\"\n\"height\" \"64\"\n}\n",
            "{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*3\"\n\"target\" \"tdest\"\n}\n",
            "{\n\"classname\" \"misc_teleporter_dest\"\n\"targetname\" \"tdest\"\n",
            "\"origin\" \"512 64 32\"\n\"angle\" \"270\"\n}\n",
        );
        let quad = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]| {
            vec![
                (a, [0.0, 0.0]),
                (b, [1.0, 0.0]),
                (c, [1.0, 1.0]),
                (d, [0.0, 1.0]),
            ]
        };
        build_bsp46(
            entities,
            &[
                ShaderSpec { name: "textures/test/a", surface: 0, contents: 1 },
                ShaderSpec { name: "textures/test/door", surface: 0, contents: 1 },
                ShaderSpec { name: "textures/test/plat", surface: 0, contents: 1 },
                ShaderSpec {
                    name: "textures/skies/testsky",
                    surface: SURF_SKY | 0x400,
                    contents: 1,
                },
                ShaderSpec {
                    name: "textures/liquids/lavatest",
                    surface: 0,
                    contents: CONTENTS_LAVA,
                },
            ],
            &[
                // 0: world floor
                FaceSpec {
                    tex: 0,
                    verts: quad(
                        [0.0, 0.0, 0.0],
                        [256.0, 0.0, 0.0],
                        [256.0, 256.0, 0.0],
                        [0.0, 256.0, 0.0],
                    ),
                },
                // 1: the sky lid
                FaceSpec {
                    tex: 3,
                    verts: quad(
                        [0.0, 0.0, 512.0],
                        [256.0, 0.0, 512.0],
                        [256.0, 256.0, 512.0],
                        [0.0, 256.0, 512.0],
                    ),
                },
                // 2: a lava pool
                FaceSpec {
                    tex: 4,
                    verts: quad(
                        [0.0, -256.0, 0.0],
                        [256.0, -256.0, 0.0],
                        [256.0, -128.0, 0.0],
                        [0.0, -128.0, 0.0],
                    ),
                },
                // 3: the door leaf, authored CLOSED
                FaceSpec {
                    tex: 1,
                    verts: quad(
                        [0.0, 0.0, 0.0],
                        [64.0, 0.0, 0.0],
                        [64.0, 0.0, 128.0],
                        [0.0, 0.0, 128.0],
                    ),
                },
                // 4: the plat top, authored RAISED
                FaceSpec {
                    tex: 2,
                    verts: quad(
                        [128.0, 0.0, 64.0],
                        [192.0, 0.0, 64.0],
                        [192.0, 64.0, 64.0],
                        [128.0, 64.0, 64.0],
                    ),
                },
            ],
            &[
                ModelSpec {
                    mins: [0.0, -256.0, 0.0],
                    maxs: [256.0, 256.0, 512.0],
                    first_face: 0,
                    n_faces: 3,
                },
                // *1 func_door: 128 tall, opens UP by 128 - lip 8 = 120.
                ModelSpec {
                    mins: [0.0, 0.0, 0.0],
                    maxs: [64.0, 16.0, 128.0],
                    first_face: 3,
                    n_faces: 1,
                },
                // *2 func_plat: top at z=64, `height` 64 down.
                ModelSpec {
                    mins: [128.0, 0.0, 0.0],
                    maxs: [192.0, 64.0, 64.0],
                    first_face: 4,
                    n_faces: 1,
                },
                // *3 trigger_teleport: a clip brush, no drawable faces at all.
                ModelSpec {
                    mins: [300.0, -32.0, 0.0],
                    maxs: [364.0, 32.0, 128.0],
                    first_face: 0,
                    n_faces: 0,
                },
            ],
        )
    }

    /// A 4x4 solid-colour 24-bit TGA, top-left origin.
    fn write_tga(path: &Path, color: [u8; 3]) {
        let (w, h) = (4usize, 4usize);
        let mut tga = vec![0u8; 18 + w * h * 3];
        tga[2] = 2;
        tga[12..14].copy_from_slice(&(w as u16).to_le_bytes());
        tga[14..16].copy_from_slice(&(h as u16).to_le_bytes());
        tga[16] = 24;
        tga[17] = 0x20;
        for px in tga[18..].chunks_exact_mut(3) {
            px.copy_from_slice(&[color[2], color[1], color[0]]);
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, tga).unwrap();
    }

    /// A pack whose `textures/skies/testsky` shader names a `skyParms` farbox
    /// and whose six `env/` faces are all present.
    fn contract_sky_bank(root: &Path) -> Q3TexBank {
        const COLORS: [[u8; 3]; 6] = [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [0, 255, 255],
            [255, 0, 255],
        ];
        for (i, suf) in crate::skybox::FACE_SUFFIXES.iter().enumerate() {
            write_tga(&root.join(format!("env/testenv_{suf}.tga")), COLORS[i]);
        }
        let _ = std::fs::create_dir_all(root.join("scripts"));
        std::fs::write(
            root.join("scripts/testsky.shader"),
            "textures/skies/testsky\n{\n\tsurfaceparm sky\n\tskyParms env/testenv - -\n}\n\
             textures/skies/cloudsky\n{\n\tsurfaceparm sky\n\tskyparms - 512 -\n}\n",
        )
        .unwrap();
        let mut bank = load_tex_bank(root);
        apply_shader_aliases(&mut bank, root);
        bank
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

    fn as_f32(v: &makepad_asset_client::json::Value) -> f32 {
        use makepad_asset_client::json::Value;
        match v {
            Value::F64(f) => *f as f32,
            Value::Int(i) => *i as f32,
            _ => f32::NAN,
        }
    }

    fn extra_f32(extras: &makepad_asset_client::json::Value, key: &str) -> f32 {
        extras.get(key).map(as_f32).unwrap_or(f32::NAN)
    }

    /// Read a float accessor's values through its bufferView.
    fn read_accessor(
        root: &makepad_asset_client::json::Value,
        bin: &[u8],
        index: i64,
    ) -> Vec<f32> {
        use makepad_asset_client::json::Value;
        let acc = &root.get("accessors").unwrap().as_arr().unwrap()[index as usize];
        let vi = acc.get("bufferView").and_then(Value::as_i64).unwrap() as usize;
        let view = &root.get("bufferViews").unwrap().as_arr().unwrap()[vi];
        let off = view.get("byteOffset").and_then(Value::as_i64).unwrap_or(0) as usize;
        let len = view.get("byteLength").and_then(Value::as_i64).unwrap() as usize;
        bin[off..off + len]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    /// The PNG bytes a node's own material paints with.
    fn node_image(
        root: &makepad_asset_client::json::Value,
        bin: &[u8],
        node: &makepad_asset_client::json::Value,
    ) -> Vec<u8> {
        use makepad_asset_client::json::Value;
        let mi = root
            .get("meshes")
            .unwrap()
            .as_arr()
            .unwrap()
            .get(node.get("mesh").and_then(Value::as_i64).unwrap() as usize)
            .and_then(|m| m.get("primitives"))
            .and_then(Value::as_arr)
            .and_then(|p| p[0].get("material"))
            .and_then(Value::as_i64)
            .expect("material");
        let ti = root.get("materials").unwrap().as_arr().unwrap()[mi as usize]
            .get("pbrMetallicRoughness")
            .and_then(|p| p.get("baseColorTexture"))
            .and_then(|t| t.get("index"))
            .and_then(Value::as_i64)
            .expect("baseColorTexture");
        let si = root.get("textures").unwrap().as_arr().unwrap()[ti as usize]
            .get("source")
            .and_then(Value::as_i64)
            .expect("source");
        let vi = root.get("images").unwrap().as_arr().unwrap()[si as usize]
            .get("bufferView")
            .and_then(Value::as_i64)
            .expect("bufferView") as usize;
        let view = &root.get("bufferViews").unwrap().as_arr().unwrap()[vi];
        let off = view.get("byteOffset").and_then(Value::as_i64).unwrap_or(0) as usize;
        let len = view.get("byteLength").and_then(Value::as_i64).unwrap() as usize;
        bin[off..off + len].to_vec()
    }

    #[test]
    fn a_func_door_exports_as_an_animated_node_and_leaves_the_level_mesh() {
        use makepad_asset_client::json::{self, Value};
        let staged = tmp_dir("q3door");
        let bank = contract_sky_bank(&staged.join("pack"));
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        let bin = glb_bin(&glb);
        let nodes = root.get("nodes").unwrap().as_arr().unwrap();
        let (node_index, door) = nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door_1 node");
        let extras = door.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("door"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("open"));
        assert_eq!(extras.get("axis").and_then(Value::as_str), Some("y"));
        assert_eq!(
            extras.get("states").and_then(Value::as_arr).map(|a| a.len()),
            Some(2)
        );
        // `angle -1` is UP; a 128-tall brush minus the default lip 8 travels
        // 120 units in metres.
        let travel = 120.0 * SCALE;
        assert!(
            (extra_f32(extras, "travel") - travel).abs() < 1e-4,
            "travel {}",
            extra_f32(extras, "travel")
        );
        let offset = extras.get("offset").unwrap().as_arr().unwrap();
        assert!(as_f32(&offset[0]).abs() < 1e-6);
        assert!((as_f32(&offset[1]) - travel).abs() < 1e-4);
        assert!(as_f32(&offset[2]).abs() < 1e-6);
        // The geometry is authored CLOSED, so the node RESTS open.
        let rest = door.get("translation").expect("rest pose").as_arr().unwrap();
        assert!((as_f32(&rest[1]) - travel).abs() < 1e-4);

        // One LINEAR translation clip named like the node: t=0 closed,
        // t=seconds open.
        let anims = root.get("animations").unwrap().as_arr().unwrap();
        let clip = anims
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door_1 clip");
        let sampler = &clip.get("samplers").unwrap().as_arr().unwrap()[0];
        assert_eq!(
            sampler.get("interpolation").and_then(Value::as_str),
            Some("LINEAR")
        );
        let times = read_accessor(&root, &bin, sampler.get("input").unwrap().as_i64().unwrap());
        assert_eq!(times.len(), 2, "two keyframes");
        assert!(times[0].abs() < 1e-6);
        assert!((times[1] - crate::glb_nodes::DOOR_SECONDS).abs() < 1e-6);
        let values = read_accessor(&root, &bin, sampler.get("output").unwrap().as_i64().unwrap());
        assert_eq!(values.len(), 6, "two VEC3 keys");
        assert_eq!(&values[0..3], &[0.0, 0.0, 0.0], "t=0 is the authored pose");
        assert!((values[4] - travel).abs() < 1e-4, "t=1 is open: {values:?}");
        let channel = &clip.get("channels").unwrap().as_arr().unwrap()[0];
        let target = channel.get("target").unwrap();
        assert_eq!(
            target.get("node").and_then(Value::as_i64),
            Some(node_index as i64)
        );
        assert_eq!(
            target.get("path").and_then(Value::as_str),
            Some("translation")
        );

        // The leaf left the static mesh: only the world shader is still a
        // primitive of mesh 0 (sky, lava, door and plat all went to nodes).
        let level = &root.get("meshes").unwrap().as_arr().unwrap()[0];
        assert_eq!(
            level.get("primitives").and_then(Value::as_arr).map(|p| p.len()),
            Some(1),
            "door/plat/sky/lava must not be baked into the level mesh"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn a_func_plat_exports_as_a_lift_node_resting_up() {
        use makepad_asset_client::json::{self, Value};
        let staged = tmp_dir("q3plat");
        let bank = contract_sky_bank(&staged.join("pack"));
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        let bin = glb_bin(&glb);
        let nodes = root.get("nodes").unwrap().as_arr().unwrap();
        let lift = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("lift_1"))
            .expect("lift_1 node");
        let extras = lift.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("lift"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("up"));
        // Top of the brush at z=64; `height 64` drops it to 0.
        assert!((extra_f32(extras, "up") - 64.0 * SCALE).abs() < 1e-4);
        assert!(extra_f32(extras, "down").abs() < 1e-4);
        assert!((extra_f32(extras, "travel") + 64.0 * SCALE).abs() < 1e-4);
        assert!(
            (extra_f32(extras, "wait") - crate::glb_nodes::LIFT_WAIT_SECONDS).abs() < 1e-4
        );
        // A lift RESTS up — the level is baked with it raised.
        assert!(
            lift.get("translation").is_none(),
            "a lift rests at the authored (up) pose"
        );
        let clip = root
            .get("animations")
            .unwrap()
            .as_arr()
            .unwrap()
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some("lift_1"))
            .expect("lift_1 clip")
            .clone();
        let sampler = &clip.get("samplers").unwrap().as_arr().unwrap()[0];
        let values = read_accessor(&root, &bin, sampler.get("output").unwrap().as_i64().unwrap());
        assert_eq!(&values[0..3], &[0.0, 0.0, 0.0], "t=0 is UP");
        assert!((values[4] + 64.0 * SCALE).abs() < 1e-4, "t=1 is DOWN: {values:?}");
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn a_lava_shader_exports_as_its_own_hazard_node() {
        use makepad_asset_client::json::{self, Value};
        let staged = tmp_dir("q3lava");
        let bank = contract_sky_bank(&staged.join("pack"));
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        let nodes = root.get("nodes").unwrap().as_arr().unwrap();
        let hazard = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("hazard_1"))
            .expect("hazard_1 node");
        let extras = hazard.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("hazard"));
        assert_eq!(extras.get("damage").and_then(Value::as_i64), Some(20));
        assert_eq!(extras.get("flat").and_then(Value::as_str), Some("lavatest"));
        assert_eq!(extras.get("liquid").and_then(Value::as_bool), Some(true));
        // A Q3 liquid is a volume you swim through, not a floor.
        assert_eq!(extras.get("solid").and_then(Value::as_bool), Some(false));
        assert!(hazard.get("translation").is_none(), "a hazard does not move");
        // Its triangles are ONLY in that node: nothing in the level mesh sits
        // on the lava plane (map y −256..−128 → GLB z +2..+4).
        let parts = crate::world_preview::extract_glb_parts(&glb).expect("parts");
        let on_lava = parts[0].indices.chunks_exact(3).any(|tri| {
            [
                parts[0].pos[tri[0] as usize],
                parts[0].pos[tri[1] as usize],
                parts[0].pos[tri[2] as usize],
            ]
            .iter()
            .all(|p| p[2] > 1.9)
        });
        assert!(!on_lava, "lava must leave the level mesh");
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn a_skyparms_farbox_becomes_one_equirect_sky_node() {
        use makepad_asset_client::json::{self, Value};
        let staged = tmp_dir("q3sky");
        let bank = contract_sky_bank(&staged.join("pack"));
        assert_eq!(
            bank.sky_box_for("textures/skies/testsky").map(|s| s.as_str()),
            Some("env/testenv"),
            "skyParms farbox must be parsed"
        );
        assert!(
            bank.sky_box_for("textures/skies/cloudsky").is_none(),
            "`skyparms - 512 -` names no box"
        );
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        let bin = glb_bin(&glb);
        let nodes = root.get("nodes").unwrap().as_arr().unwrap();
        let sky = nodes
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some("sky"))
            .expect("sky node");
        let extras = sky.get("extras").expect("extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("sky"));
        // The renderer has no cube sampler: `cube` means "the equirect twin",
        // one image, no wrap multiplier and no phase offset.
        assert_eq!(
            extras.get("projection").and_then(Value::as_str),
            Some("cube")
        );
        assert!((extra_f32(extras, "repeat") - 1.0).abs() < 1e-6);
        assert!(extra_f32(extras, "offset").abs() < 1e-6);
        assert_eq!(
            extras.get("texture").and_then(Value::as_str),
            Some("textures/skies/testsky")
        );
        assert!(
            extras.get("layers").is_none(),
            "a cube sky is ONE image, not a layer stack"
        );
        // Its own picture, at the size `skybox` declares.
        let png = node_image(&root, &bin, sky);
        let (rgba, w, h) = decode_png_stored(&png).expect("sky png");
        assert_eq!((w, h), (crate::skybox::EQUIRECT_W, crate::skybox::EQUIRECT_H));
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        // Equirect u=0.5 is the renderer's GLB +Z, which the geometry map
        // (`map (x, y, z) → GLB (x, z, −y)`) makes map −Y — the `ft` face,
        // yellow here. v≈0 is GLB +Y = map +Z, the `up` cap. This is the
        // orientation `skybox` asserts, checked end to end through a BSP.
        let px = |u: f32, v: f32| {
            let x = ((u * w as f32) as u32).min(w - 1) as usize;
            let y = ((v * h as f32) as u32).min(h - 1) as usize;
            let o = (y * w as usize + x) * 4;
            [rgba[o], rgba[o + 1], rgba[o + 2]]
        };
        assert_eq!(px(0.5, 0.5), [255, 255, 0], "GLB +Z is the `ft` face");
        assert_eq!(px(0.5, 0.01), [0, 255, 255], "the zenith is the `up` face");
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn a_cloud_layer_sky_keeps_its_faces_rather_than_punching_a_hole() {
        use makepad_asset_client::json::{self, Value};
        let staged = tmp_dir("q3cloudsky");
        // Same map, but the pack has no `env/` box for its sky shader — which
        // is every sky in the Quake III demo pak0.pk3.
        let bank = Q3TexBank::default();
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        assert!(
            !root
                .get("nodes")
                .unwrap()
                .as_arr()
                .unwrap()
                .iter()
                .any(|n| n.get("name").and_then(Value::as_str) == Some("sky")),
            "no picture, no sky node"
        );
        // The lid stays where the engine drew one, so the courtyard is not a
        // hole: the level mesh keeps the sky shader's own primitive.
        let level = &root.get("meshes").unwrap().as_arr().unwrap()[0];
        assert_eq!(
            level.get("primitives").and_then(Value::as_arr).map(|p| p.len()),
            Some(2),
            "world + sky lid stay in the level mesh"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn every_level_part_declares_itself_prelit() {
        use makepad_asset_client::json::{self, Value};
        let staged = tmp_dir("q3prelit");
        let bank = contract_sky_bank(&staged.join("pack"));
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let root = json::parse(glb_json(&glb).as_bytes()).expect("glb json");
        // This fixture is vertex-lit (no lightmap lump), which used to ship
        // COLOR_0 with no marker at all — the analytic sun then multiplied an
        // already-lit level. Every level primitive's material must say prelit.
        let level = &root.get("meshes").unwrap().as_arr().unwrap()[0];
        let materials = root.get("materials").unwrap().as_arr().unwrap();
        let prims = level.get("primitives").unwrap().as_arr().unwrap();
        assert!(!prims.is_empty());
        for p in prims {
            let mi = p.get("material").and_then(Value::as_i64).expect("material") as usize;
            assert!(
                materials[mi]
                    .get("extras")
                    .and_then(|e| e.get("lightmapTexture"))
                    .is_some(),
                "a prelit map must never be lit twice"
            );
            assert!(
                p.get("attributes")
                    .and_then(|a| a.get("TEXCOORD_1"))
                    .is_some(),
                "the marker needs its (zero) uv set"
            );
        }
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    fn a_converted_map_publishes_every_start_door_lift_and_teleport() {
        use crate::world_nav::WorldNav;
        let staged = tmp_dir("q3spawn");
        let bank = contract_sky_bank(&staged.join("pack"));
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let text =
            std::fs::read_to_string(staged.join("maps/contract.spawn")).expect("spawn sidecar");
        // The first three lines are still the `world-spawn 1` the library reads.
        assert!(text.starts_with("world-spawn 1\n"));
        let nav = WorldNav::parse(&text).expect("parse spawn");

        // Every start, not one winner.
        let names: Vec<&str> = nav.starts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["player_start", "deathmatch_1", "deathmatch_2"]);
        let start = &nav.starts[0];
        // origin (0,0,64) → GLB (0, 64/64 + 26/64, 0), yaw 90° → 0.
        assert!(start.pos[0].abs() < 1e-4);
        assert!((start.pos[1] - (64.0 + VIEW_HEIGHT) * SCALE).abs() < 1e-4);
        assert!(start.yaw.abs() < 1e-4, "angle 90 looks down −Z");
        assert!((nav.floor_y.unwrap() - (64.0 - ORIGIN_ABOVE_FLOOR) * SCALE).abs() < 1e-4);
        assert!((nav.eye_height.unwrap() - (VIEW_HEIGHT + ORIGIN_ABOVE_FLOOR) * SCALE).abs() < 1e-4);
        assert!((nav.step_height.unwrap() - STEP_HEIGHT * SCALE).abs() < 1e-4);

        assert_eq!(nav.doors.len(), 1);
        assert_eq!(nav.doors[0].name, "door_1");
        // Brush centre (32, 8, 64), GLB axes; it opens 120 up.
        assert!((nav.doors[0].pos[0] - 32.0 * SCALE).abs() < 1e-4, "{:?}", nav.doors[0]);
        assert!((nav.doors[0].closed_y - 64.0 * SCALE).abs() < 1e-4, "{:?}", nav.doors[0]);
        assert!(
            (nav.doors[0].open_y - (64.0 + 120.0) * SCALE).abs() < 1e-4,
            "{:?}",
            nav.doors[0]
        );

        assert_eq!(nav.lifts.len(), 1);
        assert_eq!(nav.lifts[0].name, "lift_1");
        assert!((nav.lifts[0].closed_y - 64.0 * SCALE).abs() < 1e-4, "rests UP");
        assert!(nav.lifts[0].open_y.abs() < 1e-4, "drops to 0");

        assert_eq!(nav.teleports.len(), 1);
        let t = &nav.teleports[0];
        assert_eq!(t.name, "teleport_1");
        // Pad x 300..364, map y −32..32 mirrored into GLB z.
        assert!((t.pad_min[0] - 300.0 * SCALE).abs() < 1e-4, "{t:?}");
        assert!((t.pad_max[0] - 364.0 * SCALE).abs() < 1e-4, "{t:?}");
        assert!((t.pad_min[1] + 32.0 * SCALE).abs() < 1e-4, "{t:?}");
        assert!((t.pad_max[1] - 32.0 * SCALE).abs() < 1e-4, "{t:?}");
        // Destination (512, 64, 32) at eye height, facing `angle 270`.
        assert!((t.dst[0] - 512.0 * SCALE).abs() < 1e-4, "{t:?}");
        assert!((t.dst[1] - (32.0 + VIEW_HEIGHT) * SCALE).abs() < 1e-4, "{t:?}");
        assert!((t.dst[2] + 64.0 * SCALE).abs() < 1e-4, "{t:?}");
        assert!(
            (t.yaw - (std::f32::consts::FRAC_PI_2 - 270f32.to_radians())).abs() < 1e-4,
            "{t:?}"
        );

        // A Q3 arena ends on a frag count: there is no exit and there are no
        // keys, so no marker is invented.
        assert!(nav.markers.is_empty());

        // And the same facts arrive on the catalog anchors.
        let anchors = nav.anchors();
        for want in [
            "floor_height",
            "step_height",
            "eye_height",
            "player_start",
            "deathmatch_1",
            "deathmatch_2",
            "door_1",
            "lift_1",
            "teleport_1",
        ] {
            assert!(
                anchors.iter().any(|a| a.name == want),
                "missing anchor {want}: {:?}",
                anchors.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
            );
        }
        let lift = anchors.iter().find(|a| a.name == "lift_1").unwrap();
        assert!(
            (lift.transform.pos.y - 64.0 * SCALE).abs() < 1e-4,
            "the lift anchor sits at the UP floor"
        );
        let _ = std::fs::remove_dir_all(&staged);
    }

    /// The exported sky read back by the RENDERER's own parser and its CPU
    /// twin of the sky shader: the picture is only right if the consumer
    /// lands on the face the engine draws in that direction.
    #[test]
    fn the_renderer_reads_our_cube_sky_the_way_quake3_drew_it() {
        use makepad_render::model::{SkyProjection, StaticModel};
        let staged = tmp_dir("q3skyread");
        let bank = contract_sky_bank(&staged.join("pack"));
        convert_bsp46(
            &build_contract_bsp46(),
            "maps/contract.bsp",
            &staged,
            &bank,
            QUAKE3_SOURCE_ID,
        )
        .expect("convert");
        let glb = std::fs::read(staged.join("maps/contract.glb")).expect("glb");
        let model = StaticModel::parse_glb(&glb).expect("renderer parses it");
        assert!(model.prelit, "a Q3 map is prelit — the sun must not light it");
        let sky = model.sky.expect("renderer found the sky part");
        assert_eq!(sky.projection, SkyProjection::Cube);
        assert_eq!(sky.repeat, 1.0);
        assert_eq!(sky.offset, 0.0);
        assert_eq!(sky.images.len(), 1, "one equirect twin, not six faces");
        assert!(sky.images[0].starts_with(b"\x89PNG"));
        assert!(!sky.vertices.is_empty(), "sky faces are real geometry");
        assert!(sky.indices.len() >= 3);

        let (rgba, w, h) = decode_png_stored(&sky.images[0]).expect("panorama");
        // A direction of the model API's own vector type, without naming its
        // math crate (not a dependency here).
        let dir = |x: f32, y: f32, z: f32| {
            let mut v = model.min;
            v.x = x;
            v.y = y;
            v.z = z;
            v
        };
        let look = |x: f32, y: f32, z: f32| -> [u8; 3] {
            let uv = sky.direction_uv(dir(x, y, z), 0, 0.0);
            let px = ((uv[0].rem_euclid(1.0) * w as f32) as u32).min(w - 1) as usize;
            let py = ((uv[1].clamp(0.0, 0.999) * h as f32) as u32).min(h - 1) as usize;
            let o = (py * w as usize + px) * 4;
            [rgba[o], rgba[o + 1], rgba[o + 2]]
        };
        // The geometry map is `map (x, y, z) → GLB (x, z, −y)`, so reading it
        // back: GLB +X is map +X (`rt`), GLB +Z is map −Y (`ft`), GLB −Z is
        // map +Y (`bk`), GLB +Y is map +Z (`up`). Note the map's +Y — the
        // direction Radiant calls "back" — is what lands on the GLB's own −Z
        // forward. These are the assertions a picture would be judged on: no
        // quarter turn, no mirror, no swapped poles.
        assert_eq!(look(1.0, 0.0, 0.0), [255, 0, 0], "GLB +X is `rt`");
        assert_eq!(look(-1.0, 0.0, 0.0), [0, 0, 255], "GLB −X is `lf`");
        assert_eq!(look(0.0, 0.0, 1.0), [255, 255, 0], "GLB +Z is `ft`");
        assert_eq!(look(0.0, 0.0, -1.0), [0, 255, 0], "GLB −Z is `bk`");
        assert_eq!(look(0.0, 1.0, 0.0), [0, 255, 255], "the zenith is `up`");
        assert_eq!(look(0.0, -1.0, 0.0), [255, 0, 255], "the nadir is `dn`");
        // The horizon is continuous: every column of the panorama's middle
        // row is one of the four walls, never the caps and never a gap.
        let walls = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]];
        for i in 0..64 {
            let a = (i as f32 / 64.0) * std::f32::consts::TAU;
            let c = look(a.sin(), 0.0, a.cos());
            assert!(walls.contains(&c), "horizon gap at {a}: {c:?}");
        }
        let _ = std::fs::remove_dir_all(&staged);
    }

    #[test]
    #[ignore = "visual: writes PNGs for a human to look at"]
    fn zz_visual_grab_q3_map_with_a_cube_sky() {
        use makepad_render::model::StaticModel;
        let pk3 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/quake3/demoq3/pak0.pk3");
        if !pk3.is_file() {
            return;
        }
        let grabs = std::env::var("Q3_GRAB_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let _ = std::fs::create_dir_all(&grabs);
        let root = tmp_dir("q3visual");
        let out = root.join("pk3");
        let mut warnings = Vec::new();
        let files = expand_pk3(&pk3, &out, &mut warnings).expect("expand");
        // A recognisable environment box: sky above, ground below, a bright
        // band exactly at the horizon, one hue per wall.
        const WALL: [[u8; 3]; 4] = [
            [220, 90, 90],
            [90, 220, 90],
            [90, 120, 240],
            [230, 210, 90],
        ];
        for (i, suf) in crate::skybox::FACE_SUFFIXES.iter().enumerate() {
            let (w, h) = (64usize, 64usize);
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let c = match i {
                        4 => [120, 180, 255],
                        5 => [80, 60, 40],
                        _ => {
                            let t = y as f32 / (h - 1) as f32;
                            if (t - 0.5).abs() < 0.03 {
                                [255, 255, 255]
                            } else {
                                let base = WALL[i];
                                let k = if t < 0.5 { 1.0 } else { 0.45 };
                                [
                                    (base[0] as f32 * k) as u8,
                                    (base[1] as f32 * k) as u8,
                                    (base[2] as f32 * k) as u8,
                                ]
                            }
                        }
                    };
                    // A tick every 16 columns pins yaw.
                    let c = if x % 16 == 0 { [255, 255, 255] } else { c };
                    rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
            let png = encode_png_rgba(&rgba, w as u32, h as u32).unwrap();
            let (px, pw, ph) = decode_png_stored(&png).unwrap();
            assert_eq!((pw, ph), (w as u32, h as u32));
            let mut tga = vec![0u8; 18 + w * h * 3];
            tga[2] = 2;
            tga[12..14].copy_from_slice(&(w as u16).to_le_bytes());
            tga[14..16].copy_from_slice(&(h as u16).to_le_bytes());
            tga[16] = 24;
            tga[17] = 0x20;
            for (j, p) in px.chunks_exact(4).enumerate() {
                tga[18 + j * 3..18 + j * 3 + 3].copy_from_slice(&[p[2], p[1], p[0]]);
            }
            let dest = out.join(format!("env/testenv_{suf}.tga"));
            let _ = std::fs::create_dir_all(dest.parent().unwrap());
            std::fs::write(dest, tga).unwrap();
        }
        let mut bank = load_tex_bank(&out);
        apply_shader_aliases(&mut bank, &out);
        let staged = root.join("staged");
        for (slug, sky_shader) in [
            ("q3dm7", "textures/skies/toxicskytim_dm8"),
            ("q3dm1", "textures/skies/tim_hell"),
        ] {
            // The demo pack's skies are cloud-layer skies with no farbox, so
            // stand one in to exercise the path a retail/point-release map
            // takes (`nightsky_xian_dm1` → `env/xnight2`).
            let mut b = bank.clone();
            b.sky_boxes
                .insert(sky_shader.to_string(), "env/testenv".to_string());
            let bsp = files
                .iter()
                .find(|f| f.rel == format!("maps/{slug}.bsp"))
                .expect("bsp");
            let bytes = std::fs::read(&bsp.path).unwrap();
            convert_bsp46(&bytes, &bsp.rel, &staged, &b, QUAKE3_SOURCE_ID).expect(slug);
            let glb_path = staged.join(format!("maps/{slug}.glb"));
            let glb = std::fs::read(&glb_path).unwrap();
            let model = StaticModel::parse_glb(&glb).expect("parse");
            let sky = model.sky.expect("sky node");
            let (pan, pw, ph) = decode_png_stored(&sky.images[0]).unwrap();
            let nav = crate::world_nav::WorldNav::parse(
                &std::fs::read_to_string(glb_path.with_extension("spawn")).unwrap(),
            )
            .unwrap();
            let s = nav.primary().unwrap();
            let world =
                crate::world_preview::raster_glb_from_spawn(&glb, (s.pos, s.yaw, s.pitch))
                    .expect("raster");
            let (mut rgba, w, h) = decode_png_stored(&world).unwrap();
            // Same camera basis `raster_glb_from_spawn` used, inverted.
            let pitch = s.pitch.clamp(-0.6, 0.4);
            let (cy, sy) = (s.yaw.cos(), s.yaw.sin());
            let (cp, sp) = (pitch.cos(), pitch.sin());
            let fwd = [sy * cp, sp, -cy * cp];
            let right = [cy, 0.0, sy];
            let up = [
                right[1] * fwd[2] - right[2] * fwd[1],
                right[2] * fwd[0] - right[0] * fwd[2],
                right[0] * fwd[1] - right[1] * fwd[0],
            ];
            let focal = (w as f32 * 0.5) / (75.0f32.to_radians() * 0.5).tan();
            let dir_of = |v: [f32; 3]| {
                let mut d = model.min;
                d.x = v[0];
                d.y = v[1];
                d.z = v[2];
                d
            };
            for py in 0..h as usize {
                for px in 0..w as usize {
                    let o = (py * w as usize + px) * 4;
                    if rgba[o..o + 3] != [26, 31, 41] {
                        continue;
                    }
                    let cx = (px as f32 + 0.5 - w as f32 * 0.5) / focal;
                    let cyy = -(py as f32 + 0.5 - h as f32 * 0.5) / focal;
                    let d = [
                        right[0] * cx + up[0] * cyy + fwd[0],
                        right[1] * cx + up[1] * cyy + fwd[1],
                        right[2] * cx + up[2] * cyy + fwd[2],
                    ];
                    let uv = sky.direction_uv(dir_of(d), 0, 0.0);
                    let sx = ((uv[0].rem_euclid(1.0) * pw as f32) as u32).min(pw - 1) as usize;
                    let sv = ((uv[1].clamp(0.0, 0.999) * ph as f32) as u32).min(ph - 1) as usize;
                    let si = (sv * pw as usize + sx) * 4;
                    rgba[o..o + 3].copy_from_slice(&pan[si..si + 3]);
                }
            }
            let png = encode_png_rgba(&rgba, w, h).unwrap();
            let dest = grabs.join(format!("{slug}-sky.png"));
            std::fs::write(&dest, png).unwrap();
            eprintln!("grab {}", dest.display());
            let pan_dest = grabs.join(format!("{slug}-panorama.png"));
            std::fs::write(&pan_dest, &sky.images[0]).unwrap();
            eprintln!("grab {}", pan_dest.display());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn func_bobbing_is_reported_not_forced_into_a_door() {
        let entities = concat!(
            "{\n\"classname\" \"func_bobbing\"\n\"model\" \"*1\"\n\"height\" \"32\"\n}\n",
            "{\n\"classname\" \"func_door\"\n\"model\" \"*2\"\n\"angle\" \"90\"\n}\n",
            "{\n\"classname\" \"trigger_hurt\"\n\"model\" \"*3\"\n\"dmg\" \"100\"\n}\n",
        );
        let ents = parse_entities(entities);
        let models = vec![
            Bsp46Model::default(),
            Bsp46Model {
                mins: [0.0; 3],
                maxs: [64.0, 64.0, 64.0],
                first_face: 0,
                num_faces: 2,
            },
            Bsp46Model {
                mins: [0.0; 3],
                maxs: [64.0, 128.0, 64.0],
                first_face: 2,
                num_faces: 2,
            },
            // A damage volume: `common/trigger` is SURF_NODRAW, so q3map
            // wrote the brush with no drawable face at all.
            Bsp46Model {
                mins: [0.0; 3],
                maxs: [512.0, 512.0, 8.0],
                first_face: 0,
                num_faces: 0,
            },
        ];
        let (movers, warnings) = bsp46_movers(&ents, &models);
        // A sinusoid has no open/closed pair, so it is NOT a door: its faces
        // stay in the static mesh and the fact is reported instead of faked.
        assert_eq!(movers.len(), 1);
        assert_eq!(movers[0].name, "door_1");
        assert_eq!(movers[0].kind, MoverKind::Door);
        assert!(
            warnings.iter().any(|w| w.contains("func_bobbing")),
            "{warnings:?}"
        );
        // The same for a damage volume: no geometry, so no hazard node.
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("trigger_hurt") && w.contains("0 drawable faces")),
            "{warnings:?}"
        );
        // `angle 90` is +Y in map space, which is −Z in the GLB.
        let travel = (128.0 - Q3_MOVER_LIP) * SCALE;
        assert!(movers[0].travel[0].abs() < 1e-4, "{:?}", movers[0]);
        assert!(
            (movers[0].travel[2] + travel).abs() < 1e-4,
            "{:?}",
            movers[0]
        );
        assert_eq!(movers[0].axis, "z");
    }

    #[test]
    fn a_start_open_door_is_re_authored_closed_so_it_rests_where_the_bsp_baked_it() {
        // `SP_func_door` swaps pos1/pos2 on spawnflag 1, so q3map baked the
        // brush OPEN. The contract wants geometry authored CLOSED resting
        // OPEN — which must land back exactly on the baked pose, or the door
        // is a slab across its own doorway.
        let model = Bsp46Model {
            mins: [0.0; 3],
            maxs: [64.0, 16.0, 128.0],
            first_face: 0,
            num_faces: 1,
        };
        let models = vec![Bsp46Model::default(), model];
        let plain = parse_entities("{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n}\n");
        let (plain, _) = bsp46_movers(&plain, &models);
        let open = parse_entities(
            "{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n\"spawnflags\" \"1\"\n}\n",
        );
        let (open, _) = bsp46_movers(&open, &models);
        let travel = (128.0 - Q3_MOVER_LIP) * SCALE;
        // Ordinary door: baked closed, opens UP.
        assert_eq!(plain[0].shift, [0.0; 3]);
        assert!((plain[0].travel[1] - travel).abs() < 1e-4, "{:?}", plain[0]);
        // start_open: baked OPEN, so the vertices move UP to closed and the
        // door opens back DOWN by the same amount.
        assert!((open[0].shift[1] - travel).abs() < 1e-4, "{:?}", open[0]);
        assert!((open[0].travel[1] + travel).abs() < 1e-4, "{:?}", open[0]);
        // shift + travel = 0: resting open IS the pose the BSP baked.
        for k in 0..3 {
            assert!(
                (open[0].shift[k] + open[0].travel[k]).abs() < 1e-5,
                "{:?}",
                open[0]
            );
        }
        // The nav summary reports the CLOSED height, not the baked one.
        let nav = bsp46_nav(
            &parse_entities(
                "{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"-1\"\n\"spawnflags\" \"1\"\n}\n",
            ),
            &models,
            &open,
        )
        .0;
        let baked_centre = 64.0 * SCALE;
        assert!(
            (nav.doors[0].closed_y - (baked_centre + travel)).abs() < 1e-4,
            "{:?}",
            nav.doors[0]
        );
    }

    #[test]
    fn entity_values_keep_their_spaces_and_backslashes() {
        let ents = parse_entities(
            "{\n\"classname\" \"worldspawn\"\n\"message\" \"The Longest Yard\"\n\
             \"music\" \"music\\sonic5.wav\"\n\"origin\" \"1 2 3\"\n}\n",
        );
        assert_eq!(ents.len(), 1);
        assert_eq!(ents[0].class(), "worldspawn");
        assert_eq!(ents[0].get("message"), Some("The Longest Yard"));
        assert_eq!(ents[0].get("music"), Some("music\\sonic5.wav"));
        assert_eq!(ents[0].origin(), Some([1.0, 2.0, 3.0]));
    }

    fn build_minimal_md3() -> Vec<u8> {
        const SURF: usize = 108;
        const TRI: usize = 12;
        const SHADER: usize = 68;
        const ST: usize = 24;
        const XYZ: usize = 24;
        const SURF_END: usize = SURF + TRI + SHADER + ST + XYZ;
        let mut out = vec![0u8; 108 + SURF_END];
        out[0..4].copy_from_slice(b"IDP3");
        out[4..8].copy_from_slice(&15i32.to_le_bytes());
        out[8..13].copy_from_slice(b"test\0");
        // flags = 0 (old parser treated this as numFrames and rejected the file)
        out[72..76].copy_from_slice(&0i32.to_le_bytes());
        out[76..80].copy_from_slice(&1i32.to_le_bytes()); // numFrames
        out[80..84].copy_from_slice(&0i32.to_le_bytes()); // numTags
        out[84..88].copy_from_slice(&1i32.to_le_bytes()); // numSurfaces
        out[100..104].copy_from_slice(&108i32.to_le_bytes()); // ofsSurfaces
        out[104..108].copy_from_slice(&((108 + SURF_END) as i32).to_le_bytes());
        let s = 108;
        out[s..s + 4].copy_from_slice(b"IDP3");
        out[s + 4..s + 11].copy_from_slice(b"l_legs\0");
        write_i32(&mut out, s + 72, 1); // surf frames
        write_i32(&mut out, s + 76, 1); // shaders
        write_i32(&mut out, s + 80, 3); // verts
        write_i32(&mut out, s + 84, 1); // tris
        write_i32(&mut out, s + 88, SURF as i32);
        write_i32(&mut out, s + 92, (SURF + TRI) as i32);
        write_i32(&mut out, s + 96, (SURF + TRI + SHADER) as i32);
        write_i32(&mut out, s + 100, (SURF + TRI + SHADER + ST) as i32);
        write_i32(&mut out, s + 104, SURF_END as i32);
        let to = s + SURF;
        write_i32(&mut out, to, 0);
        write_i32(&mut out, to + 4, 1);
        write_i32(&mut out, to + 8, 2);
        let xyz = s + SURF + TRI + SHADER + ST;
        // triangle in XY
        for (i, (x, y, z)) in [(0i16, 0i16, 0i16), (64, 0, 0), (0, 64, 0)]
            .into_iter()
            .enumerate()
        {
            let o = xyz + i * 8;
            out[o..o + 2].copy_from_slice(&x.to_le_bytes());
            out[o + 2..o + 4].copy_from_slice(&y.to_le_bytes());
            out[o + 4..o + 6].copy_from_slice(&z.to_le_bytes());
        }
        out
    }

    fn write_f32(buf: &mut [u8], o: usize, v: f32) {
        buf[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn write_i32(buf: &mut [u8], o: usize, v: i32) {
        buf[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}
