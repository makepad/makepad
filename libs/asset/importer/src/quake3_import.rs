//! Quake III Arena demo classic importer.
//!
//! Local-folder only. Converts original formats into catalog payloads — never
//! ships `.pk3` / `.bsp` / `.md3` as asset types:
//!
//! - PK3 (zip) → extracted files for the parsers below
//! - BSP `IBSP` version 46 → one flat `World` GLB (visible faces + atlas)
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
use makepad_gltf::{write_glb_mesh_textured, GlbTexturedMesh};
use makepad_zip_file::zip_read_central_directory;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const QUAKE3_SOURCE_ID: &str = "quake3";
pub const QUAKE3_SOURCE_TITLE: &str = "Quake III Arena (demo)";
pub const QUAKE3_LICENSE: &str = "id-Software-demo";
pub const QUAKE3_HOME: &str = "https://ioquake3.org/";
pub const QUAKE3_CREDITS: &str = "id Software Quake III Arena demo";

/// 64 Quake units = 1 metre (same scale as the Doom/Freedoom importer).
const SCALE: f32 = 1.0 / 64.0;
/// Q3 `DEFAULT_VIEWHEIGHT` (standing).
const VIEW_HEIGHT: f32 = 26.0;

const LUMP_ENTITIES: usize = 0;
const LUMP_TEXTURES: usize = 1;
const LUMP_VERTICES: usize = 10;
const LUMP_MESHVERTS: usize = 11;
const LUMP_FACES: usize = 13;
const LUMP_COUNT: usize = 17;

const TEX_SIZE: usize = 72;
const VERT_SIZE: usize = 44;
const FACE_SIZE: usize = 104;

const FACE_POLYGON: i32 = 1;
const FACE_PATCH: i32 = 2;
const FACE_MESH: i32 = 3;

const SURF_SKY: i32 = 0x4;
const SURF_NODRAW: i32 = 0x80;
const SURF_SKIP: i32 = 0x200;

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

fn decode_jpeg(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
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
    let (glb, spawn, used) = bsp46_to_glb(bytes, textures)?;
    let slug = stem_slug(rel);
    let key = format!("worlds/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &glb).map_err(|e| e.to_string())?;
    if let Some(spawn) = spawn {
        write_spawn_sidecar(&dest, spawn);
    }
    let icon_rel = crate::world_preview::write_spawn_preview(&dest)
        .ok()
        .map(|_| format!("{key}.png"));
    let mut assets = vec![ClassicAsset {
        key,
        kind: AssetKind::World,
        rel_path,
        tags: vec![
            "world".into(),
            source_id.into(),
            "map".into(),
            "bsp46".into(),
            slug.clone(),
            "no-portals".into(),
        ],
        icon_rel,
    }];
    let mut seen = BTreeSet::new();
    for name in used {
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

fn bsp46_to_glb(
    bytes: &[u8],
    textures: &Q3TexBank,
) -> Result<(Vec<u8>, Option<WorldSpawn>, Vec<String>), String> {
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
    let ent_lump = lump(bytes, LUMP_ENTITIES).ok();

    let n_tex = tex_lump.1 / TEX_SIZE;
    let n_vert = vert_lump.1 / VERT_SIZE;
    let n_idx = idx_lump.1 / 4;
    let n_face = face_lump.1 / FACE_SIZE;

    let mut shaders = Vec::with_capacity(n_tex);
    for i in 0..n_tex {
        let o = tex_lump.0 + i * TEX_SIZE;
        let name = cstr(&bytes[o..o + 64]);
        let flags = i32_le(bytes, o + 64);
        shaders.push((name, flags));
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
            normal: [
                f32_le(bytes, o + 28),
                f32_le(bytes, o + 32),
                f32_le(bytes, o + 36),
            ],
        });
    }

    let mut meshverts = Vec::with_capacity(n_idx);
    for i in 0..n_idx {
        meshverts.push(i32_le(bytes, idx_lump.0 + i * 4));
    }

    let mut images: BTreeMap<String, Q3Image> = BTreeMap::new();
    images.insert("_default".into(), gray_image(64, 64));
    let mut used = Vec::new();

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    // First pass: collect referenced shader images so the atlas is complete
    // before we emit UVs.
    for fi in 0..n_face {
        let o = face_lump.0 + fi * FACE_SIZE;
        let tex = i32_le(bytes, o);
        let typ = i32_le(bytes, o + 8);
        if typ != FACE_POLYGON && typ != FACE_MESH && typ != FACE_PATCH {
            continue;
        }
        let Some((name, flags)) = shaders.get(tex as usize) else {
            continue;
        };
        if skip_shader(name, *flags) {
            continue;
        }
        if let Some(img) = textures.resolve(name) {
            used.push(name.clone());
            images.entry(name.clone()).or_insert(img);
        }
    }
    let (atlas_png, uv_map) = pack_atlas(&images);

    for fi in 0..n_face {
        let o = face_lump.0 + fi * FACE_SIZE;
        let tex = i32_le(bytes, o);
        let typ = i32_le(bytes, o + 8);
        let first_vert = i32_le(bytes, o + 12);
        let n_vertexes = i32_le(bytes, o + 16);
        let first_idx = i32_le(bytes, o + 20);
        let n_meshverts = i32_le(bytes, o + 24);
        let patch_w = i32_le(bytes, o + 96);
        let patch_h = i32_le(bytes, o + 100);
        let (name, flags) = match shaders.get(tex as usize) {
            Some(t) => t,
            None => continue,
        };
        if skip_shader(name, *flags) {
            continue;
        }
        let slot = lookup_slot(&uv_map, name);
        match typ {
            FACE_POLYGON | FACE_MESH => emit_indexed_face(
                &mut positions,
                &mut uvs,
                &mut normals,
                &mut indices,
                &verts,
                &meshverts,
                first_vert,
                n_vertexes,
                first_idx,
                n_meshverts,
                slot,
            ),
            FACE_PATCH => emit_patch(
                &mut positions,
                &mut uvs,
                &mut normals,
                &mut indices,
                &verts,
                first_vert,
                n_vertexes,
                patch_w,
                patch_h,
                slot,
            ),
            _ => {}
        }
    }

    if positions.is_empty() || indices.is_empty() {
        positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        normals = vec![[0.0, 1.0, 0.0]; 4];
        indices = vec![0, 1, 2, 0, 2, 3];
    }

    let glb = write_glb_mesh_textured(&GlbTexturedMesh {
        positions: &positions,
        normals: Some(&normals),
        uvs: &uvs,
        indices: &indices,
        base_color_png: &atlas_png,
        metallic_roughness_png: None,
        double_sided: true,
        colors: None,
    });
    if !glb.starts_with(b"glTF") {
        return Err("GLB writer failed".into());
    }
    let spawn = ent_lump.and_then(|(off, len)| bsp46_spawn(&bytes[off..off + len]));
    Ok((glb, spawn, used))
}

fn skip_shader(name: &str, flags: i32) -> bool {
    if flags & (SURF_NODRAW | SURF_SKY | SURF_SKIP) != 0 {
        return true;
    }
    let n = name.to_ascii_lowercase();
    let stem = n.rsplit('/').next().unwrap_or(&n);
    stem == "noshader" || stem == "skip" || n == "noshader" || n.is_empty()
}

fn emit_indexed_face(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    verts: &[DrawVert],
    meshverts: &[i32],
    first_vert: i32,
    n_vertexes: i32,
    first_idx: i32,
    n_meshverts: i32,
    slot: AtlasSlot,
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
    for tri in meshverts[start..start + count].chunks_exact(3) {
        let mut corners = [0usize; 3];
        let mut ok = true;
        for k in 0..3 {
            let idx = tri[k];
            let vi = resolve_meshvert(idx, base_vert, nverts);
            if vi >= verts.len() {
                ok = false;
                break;
            }
            corners[k] = vi;
        }
        if !ok {
            continue;
        }
        emit_tri(
            positions,
            uvs,
            normals,
            indices,
            &verts[corners[0]],
            &verts[corners[1]],
            &verts[corners[2]],
            slot,
        );
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

fn emit_patch(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    verts: &[DrawVert],
    first_vert: i32,
    n_vertexes: i32,
    patch_w: i32,
    patch_h: i32,
    slot: AtlasSlot,
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
    // Cheap 2-sample tessellation (3×3 output per 3×3 Bezier).
    const LEVEL: usize = 2;
    let ctrl = |r: usize, c: usize| &verts[base + r * w + c];
    let mut r = 0usize;
    while r + 2 < h {
        let mut c = 0usize;
        while c + 2 < w {
            let mut grid = vec![DrawVert::default(); (LEVEL + 1) * (LEVEL + 1)];
            for iy in 0..=LEVEL {
                let ty = iy as f32 / LEVEL as f32;
                for ix in 0..=LEVEL {
                    let tx = ix as f32 / LEVEL as f32;
                    grid[iy * (LEVEL + 1) + ix] = bezier_patch(
                        [
                            [ctrl(r, c), ctrl(r, c + 1), ctrl(r, c + 2)],
                            [ctrl(r + 1, c), ctrl(r + 1, c + 1), ctrl(r + 1, c + 2)],
                            [ctrl(r + 2, c), ctrl(r + 2, c + 1), ctrl(r + 2, c + 2)],
                        ],
                        tx,
                        ty,
                    );
                }
            }
            for iy in 0..LEVEL {
                for ix in 0..LEVEL {
                    let i00 = iy * (LEVEL + 1) + ix;
                    let i10 = i00 + 1;
                    let i01 = i00 + (LEVEL + 1);
                    let i11 = i01 + 1;
                    emit_tri(
                        positions, uvs, normals, indices, &grid[i00], &grid[i10], &grid[i11], slot,
                    );
                    emit_tri(
                        positions, uvs, normals, indices, &grid[i00], &grid[i11], &grid[i01], slot,
                    );
                }
            }
            c += 2;
        }
        r += 2;
    }
}

fn bezier_patch(ctrl: [[&DrawVert; 3]; 3], u: f32, v: f32) -> DrawVert {
    let mut col = [DrawVert::default(); 3];
    for i in 0..3 {
        col[i] = bezier_vert(ctrl[i][0], ctrl[i][1], ctrl[i][2], u);
    }
    bezier_vert(&col[0], &col[1], &col[2], v)
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
        normal: mix3(a.normal, b.normal, c.normal),
    }
}

fn emit_tri(
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
    a: &DrawVert,
    b: &DrawVert,
    c: &DrawVert,
    slot: AtlasSlot,
) {
    let pa = xform(a.pos);
    let pb = xform(b.pos);
    let pc = xform(c.pos);
    let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
    let area = (e1[1] * e2[2] - e1[2] * e2[1]).abs()
        + (e1[2] * e2[0] - e1[0] * e2[2]).abs()
        + (e1[0] * e2[1] - e1[1] * e2[0]).abs();
    if area < 1e-10 {
        return;
    }
    let i = positions.len() as u32;
    positions.extend_from_slice(&[pa, pb, pc]);
    // Q3 ST: v=0 is OpenGL bottom. Atlas PNG is top-down (glTF).
    uvs.push(slot_uv(slot, a.st[0], 1.0 - a.st[1]));
    uvs.push(slot_uv(slot, b.st[0], 1.0 - b.st[1]));
    uvs.push(slot_uv(slot, c.st[0], 1.0 - c.st[1]));
    normals.push(xform_dir(a.normal));
    normals.push(xform_dir(b.normal));
    normals.push(xform_dir(c.normal));
    indices.extend_from_slice(&[i, i + 1, i + 2]);
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
    normal: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
struct WorldSpawn {
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
}

fn write_spawn_sidecar(glb: &Path, spawn: WorldSpawn) {
    let text = format!(
        "world-spawn 1\n{:.4} {:.4} {:.4}\n{:.5} {:.5}\n",
        spawn.pos[0], spawn.pos[1], spawn.pos[2], spawn.yaw, spawn.pitch
    );
    let _ = std::fs::write(glb.with_extension("spawn"), text);
}

fn bsp46_spawn(text: &[u8]) -> Option<WorldSpawn> {
    let text = std::str::from_utf8(text).ok()?;
    let mut best: Option<(u8, WorldSpawn)> = None;
    for raw in text.split(|c| c == '{' || c == '}') {
        let block = raw.trim();
        if block.is_empty() {
            continue;
        }
        let mut class = "";
        let mut origin = None;
        let mut angle = 0.0f32;
        let mut pitch = 0.0f32;
        for line in block.lines() {
            let kv: Vec<&str> = line.split('"').filter(|s| !s.trim().is_empty()).collect();
            if kv.len() < 2 {
                continue;
            }
            match kv[0] {
                "classname" => class = kv[1],
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
                "angles" => {
                    let mut it = kv[1].split_whitespace();
                    if let (Some(p), Some(y), Some(_r)) = (it.next(), it.next(), it.next()) {
                        pitch = p.parse().unwrap_or(0.0);
                        angle = y.parse().unwrap_or(angle);
                    }
                }
                _ => {}
            }
        }
        let rank = match class {
            "info_player_start" => 0u8,
            "info_player_deathmatch" => 1,
            _ => continue,
        };
        let o = match origin {
            Some(o) => o,
            None => continue,
        };
        let pos = [
            o[0] * SCALE,
            o[2] * SCALE + VIEW_HEIGHT * SCALE,
            -o[1] * SCALE,
        ];
        let yaw = std::f32::consts::FRAC_PI_2 - angle.to_radians();
        let spawn = WorldSpawn {
            pos,
            yaw,
            pitch: -pitch.to_radians(),
        };
        match best {
            Some((r, _)) if r <= rank => {}
            _ => best = Some((rank, spawn)),
        }
    }
    best.map(|(_, s)| s)
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

fn q3_face_viewer(p: [f32; 3]) -> [f32; 3] {
    // After Q3→studio, +X forward is +Z (away from the camera). Flip 180°.
    let p = q3_face_camera(p);
    [-p[0], p[1], -p[2]]
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
    let mut frame_at = 0usize;
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
        let _ = frame_at;
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
    let barrel = match (barrel_path.as_ref(), barrel_bytes.as_deref()) {
        (Some(p), Some(b)) => Md3File::parse(b).ok().map(|m| (p.clone(), m)),
        _ => None,
    };
    let mut skins = load_md3_skins(gun_path);
    if let Some((bp, _)) = &barrel {
        skins.extend(load_md3_skins(bp));
    }
    let posed = pose_weapon(&gun, barrel.as_ref().map(|(_, m)| m), textures, &skins)?;
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

fn xform_tag_dir(t: TagXform, p: [f32; 3]) -> [f32; 3] {
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
            xform_tag_dir(a, b.axis[0]),
            xform_tag_dir(a, b.axis[1]),
            xform_tag_dir(a, b.axis[2]),
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
        origin: xform_tag_dir(inv, neg),
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
            out.unique.push(q3_face_viewer(xform(world)));
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
            .expect("world");
        assert_eq!(world.rel_path, "worlds/q3tri.glb");
        let glb = std::fs::read(staged.join(&world.rel_path)).expect("glb");
        assert!(glb.starts_with(b"glTF"), "not a glTF/GLB");
        assert!(glb.len() > 200);
        let _ = std::fs::remove_dir_all(&staged);
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
        let bank = load_tex_bank(&out);
        assert!(
            bank.images.len() > 50,
            "demo jpg/tga bank too small: {}",
            bank.images.len()
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

    fn build_minimal_bsp46() -> Vec<u8> {
        let entities = b"{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 64\"\n}\n\0";
        let mut tex = vec![0u8; TEX_SIZE];
        tex[..16].copy_from_slice(b"textures/test/a\0");
        let mut verts = Vec::new();
        // Triangle in XY (Z-up): (0,0,0) (64,0,0) (0,64,0) → engine (0,0,0)(1,0,0)(0,0,-1)
        for (pos, st) in [
            ([0.0f32, 0.0, 0.0], [0.0f32, 0.0]),
            ([64.0, 0.0, 0.0], [1.0, 0.0]),
            ([0.0, 64.0, 0.0], [0.0, 1.0]),
        ] {
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
        let mut idx = Vec::new();
        for i in [0i32, 1, 2] {
            idx.extend_from_slice(&i.to_le_bytes());
        }
        let mut face = vec![0u8; FACE_SIZE];
        write_i32(&mut face, 0, 0); // texture
        write_i32(&mut face, 4, -1); // effect
        write_i32(&mut face, 8, FACE_POLYGON);
        write_i32(&mut face, 12, 0); // first vert
        write_i32(&mut face, 16, 3);
        write_i32(&mut face, 20, 0); // first meshvert
        write_i32(&mut face, 24, 3);

        let header = 8 + LUMP_COUNT * 8;
        let mut lumps = [(0usize, 0usize); LUMP_COUNT];
        let mut off = header;
        lumps[LUMP_ENTITIES] = (off, entities.len());
        off += entities.len();
        lumps[LUMP_TEXTURES] = (off, tex.len());
        off += tex.len();
        lumps[LUMP_VERTICES] = (off, verts.len());
        off += verts.len();
        lumps[LUMP_MESHVERTS] = (off, idx.len());
        off += idx.len();
        lumps[LUMP_FACES] = (off, face.len());
        off += face.len();

        let mut out = vec![0u8; off];
        out[0..4].copy_from_slice(b"IBSP");
        out[4..8].copy_from_slice(&46i32.to_le_bytes());
        for (i, (lo, llen)) in lumps.iter().enumerate() {
            let o = 8 + i * 8;
            out[o..o + 4].copy_from_slice(&(*lo as u32).to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&(*llen as u32).to_le_bytes());
        }
        let mut cur = header;
        for payload in [entities.as_slice(), tex.as_slice(), verts.as_slice(), idx.as_slice(), face.as_slice()]
        {
            out[cur..cur + payload.len()].copy_from_slice(payload);
            cur += payload.len();
        }
        out
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
