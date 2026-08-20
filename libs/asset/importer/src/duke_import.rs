//! Duke Nukem 3D shareware GRP + BUILD MAP v7 + optional HRP overlay.
//!
//! Local-folder only. Official foothold (do not fetch here):
//! - Shareware data: https://hendricks266.duke4.net/files/3dduke13_data.7z
//!   (`DUKE3D.GRP` v1.3D, episode 1 / L.A. Meltdown)
//! - HRP overlay: https://hrp.duke4.net/  (`duke3d_hrp.zip` extracted beside the GRP)
//! Retail Atomic `DUKE3D.GRP` uses the same MAP/ART/GRP parsers.

use crate::classic_import::{encode_png_rgba, ClassicAsset};
use makepad_asset_data::AssetKind;
use makepad_gltf::{write_glb_mesh_textured_parts, GlbTexturedPart};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const SCALE: f32 = 1.0 / 512.0;
/// BUILD Z is 16× finer than XY. Wall V and slope height use this ratio.
const Z_PER_XY: f32 = 16.0;
/// World Y scale so a 8192-Z storey is one unit, matching 512-XY tiles.
const SCALE_Z: f32 = SCALE / Z_PER_XY;
/// Floor/ceiling: world XY / this many units = one texel. Stat bit 3
/// uses 8 instead (denser mapping).
const FLOOR_UNITS_PER_TEXEL: f32 = 16.0;
/// Wall V: ΔZ * yrepeat / this = texels (matches the 2048 scale used by
/// BUILD's OpenGL wall span).
const WALL_V_DIV: f32 = 2048.0;

#[derive(Clone, Debug)]
pub struct ExtractedFile {
    pub path: PathBuf,
    pub rel: String,
}

#[derive(Clone, Debug)]
pub struct TileRgba {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// BUILD ART picanm x-offset. Face sprites: center, then shift `-xofs`.
    pub xoff: i8,
    pub yoff: i8,
}

#[derive(Clone, Debug)]
pub struct ArtBank {
    pub tiles: BTreeMap<u16, TileRgba>,
    pub palette: [[u8; 3]; 256],
}

impl Default for ArtBank {
    fn default() -> Self {
        Self {
            tiles: BTreeMap::new(),
            palette: [[0u8; 3]; 256],
        }
    }
}

pub fn expand_grp(
    path: &Path,
    out_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<ExtractedFile>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let files = parse_grp(&bytes)?;
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (name, data) in files {
        if name.contains("..") {
            continue;
        }
        let rel = name.replace('\\', "/");
        let dest = out_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&dest, &data) {
            warnings.push(format!("grp write {rel}: {e}"));
            continue;
        }
        out.push(ExtractedFile { path: dest, rel });
    }
    Ok(out)
}

pub fn parse_grp(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if bytes.len() < 16 || &bytes[0..12] != b"KenSilverman" {
        return Err("not a BUILD GRP (KenSilverman)".into());
    }
    let n = u32_le(bytes, 12) as usize;
    if n > 16_384 {
        return Err("GRP file count too large".into());
    }
    let header = 16 + n * 16;
    if bytes.len() < header {
        return Err("GRP header truncated".into());
    }
    let mut out = Vec::with_capacity(n);
    let mut data_off = header;
    for i in 0..n {
        let o = 16 + i * 16;
        let raw = &bytes[o..o + 12];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(12);
        let name = String::from_utf8_lossy(&raw[..end]).trim().to_string();
        let size = u32_le(bytes, o + 12) as usize;
        if data_off.saturating_add(size) > bytes.len() {
            return Err(format!("GRP entry {name} truncated"));
        }
        out.push((name, bytes[data_off..data_off + size].to_vec()));
        data_off += size;
    }
    Ok(out)
}

pub fn load_tileset(extracted: &[PathBuf], pack_dir: &Path) -> ArtBank {
    let mut bank = ArtBank {
        tiles: BTreeMap::new(),
        palette: default_vga6(),
    };
    if let Some(pal) = find_duke_palette(extracted, pack_dir) {
        bank.palette = pal;
    }
    let mut arts = Vec::new();
    for p in extracted {
        if p.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .eq_ignore_ascii_case("art")
        {
            arts.push(p.clone());
        }
    }
    walk_ext(pack_dir, "art", &mut arts);
    for art in arts {
        if let Ok(bytes) = std::fs::read(&art) {
            if let Err(e) = parse_art_into(&bytes, &bank.palette, &mut bank.tiles) {
                let _ = e;
            }
        }
    }
    apply_hrp_overlay(pack_dir, &mut bank);
    bank
}

pub fn convert_map(
    path: &Path,
    rel: &str,
    staged: &Path,
    art: &ArtBank,
    source_id: &str,
    used_face: &mut BTreeSet<u16>,
) -> Result<Vec<ClassicAsset>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let map = parse_map_v7(&bytes)?;
    let world = map_to_world(&map, art)?;
    let glb = world.glb;
    let spawn = world.spawn;
    let used_sprites = world.sprites;
    let slug = stem_slug(rel);
    let key = format!("worlds/{slug}");
    let rel_path = format!("{key}.glb");
    let dest = staged.join(&rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&dest, &glb).map_err(|e| e.to_string())?;
    if let Some(spawn) = spawn {
        let mut nav = map_nav(&map, spawn);
        // The moving parts, named exactly as the GLB names them.
        nav.doors = world.doors;
        nav.lifts = world.lifts;
        let _ = std::fs::write(dest.with_extension("spawn"), nav.to_text());
    }
    let place = map_to_place(&map, art, source_id, &key, spawn);
    for p in &place.places {
        if let Ok(pic) = p.class.parse::<u16>() {
            used_face.insert(pic);
        }
    }
    let _ = crate::world_place::write_place_sidecar(&dest, &place);
    let icon_rel = crate::world_preview::write_spawn_preview(&dest)
        .ok()
        .map(|_| format!("{key}.png"));
    let assets = vec![ClassicAsset {
        key,
        kind: AssetKind::World,
        rel_path,
        tags: vec![
            "world".into(),
            source_id.into(),
            "map".into(),
            slug.clone(),
            "no-portals".into(),
        ],
        icon_rel,
    }];
    let _ = used_sprites;
    Ok(assets)
}

fn stem_slug(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    stem.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn find_duke_palette(extracted: &[PathBuf], pack_dir: &Path) -> Option<[[u8; 3]; 256]> {
    let mut pals = vec![pack_dir.join("PALETTE.DAT")];
    for p in extracted {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if name.eq_ignore_ascii_case("PALETTE.DAT") {
            pals.push(p.clone());
        }
        if let Some(parent) = p.parent() {
            pals.push(parent.join("PALETTE.DAT"));
        }
        if name.to_ascii_lowercase().ends_with(".grp") {
            if let Some(pal) = palette_from_grp(p) {
                cache_palette_dat(pack_dir, p);
                return Some(pal);
            }
        }
    }
    for p in pals {
        if let Ok(bytes) = std::fs::read(&p) {
            if let Some(pal) = parse_palette_dat(&bytes) {
                return Some(pal);
            }
        }
    }
    let mut grps = Vec::new();
    walk_ext(pack_dir, "grp", &mut grps);
    for grp in grps {
        if let Some(pal) = palette_from_grp(&grp) {
            cache_palette_dat(pack_dir, &grp);
            return Some(pal);
        }
    }
    None
}

fn palette_from_grp(path: &Path) -> Option<[[u8; 3]; 256]> {
    let bytes = std::fs::read(path).ok()?;
    let files = parse_grp(&bytes).ok()?;
    for (name, data) in files {
        if name.eq_ignore_ascii_case("PALETTE.DAT") {
            return parse_palette_dat(&data);
        }
    }
    None
}

fn cache_palette_dat(pack_dir: &Path, grp: &Path) {
    let dest = pack_dir.join("PALETTE.DAT");
    if dest.is_file() {
        return;
    }
    let Ok(bytes) = std::fs::read(grp) else {
        return;
    };
    let Ok(files) = parse_grp(&bytes) else {
        return;
    };
    if let Some((_, data)) = files
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("PALETTE.DAT"))
    {
        let _ = std::fs::create_dir_all(pack_dir);
        let _ = std::fs::write(dest, data);
    }
}

fn parse_palette_dat(bytes: &[u8]) -> Option<[[u8; 3]; 256]> {
    if bytes.len() < 768 {
        return None;
    }
    let mut pal = [[0u8; 3]; 256];
    let mut max = 0u8;
    for i in 0..256 {
        pal[i] = [bytes[i * 3], bytes[i * 3 + 1], bytes[i * 3 + 2]];
        max = max.max(pal[i][0]).max(pal[i][1]).max(pal[i][2]);
    }
    if max <= 63 {
        for c in &mut pal {
            c[0] = c[0].saturating_mul(4);
            c[1] = c[1].saturating_mul(4);
            c[2] = c[2].saturating_mul(4);
        }
    }
    Some(pal)
}

fn default_vga6() -> [[u8; 3]; 256] {
    let mut pal = [[0u8; 3]; 256];
    for i in 0..256 {
        let v = i as u8;
        pal[i] = [v, v, v];
    }
    pal
}

fn parse_art_into(
    bytes: &[u8],
    pal: &[[u8; 3]; 256],
    tiles: &mut BTreeMap<u16, TileRgba>,
) -> Result<(), String> {
    if bytes.len() < 16 {
        return Err("ART too small".into());
    }
    let version = i32_le(bytes, 0);
    if version != 1 {
        return Err(format!("unsupported ART version {version}"));
    }
    let start = i32_le(bytes, 8);
    let end = i32_le(bytes, 12);
    if start < 0 || end < start {
        return Err("ART tile range".into());
    }
    let n = (end - start + 1) as usize;
    let header = 16 + n * 8;
    if bytes.len() < header {
        return Err("ART header truncated".into());
    }
    let mut off = header;
    for i in 0..n {
        let w = u16_le(bytes, 16 + i * 2) as usize;
        let h = u16_le(bytes, 16 + n * 2 + i * 2) as usize;
        let pix = w.saturating_mul(h);
        if w == 0 || h == 0 {
            continue;
        }
        if off + pix > bytes.len() {
            break;
        }
        if w > 1024 || h > 1024 {
            off += pix;
            continue;
        }
        let anim = u32_le(bytes, 16 + n * 4 + i * 4);
        let xoff = ((anim >> 8) & 0xff) as i8;
        let yoff = ((anim >> 16) & 0xff) as i8;
        let mut rgba = vec![0u8; pix * 4];
        // ART is column-major.
        for x in 0..w {
            for y in 0..h {
                let idx = bytes[off + x * h + y] as usize;
                let c = pal[idx];
                let di = (y * w + x) * 4;
                // Index 255 and magenta are the BUILD punch-through key.
                if idx == 255 || is_key_rgb(c[0], c[1], c[2]) {
                    rgba[di] = 0;
                    rgba[di + 1] = 0;
                    rgba[di + 2] = 0;
                    rgba[di + 3] = 0;
                } else {
                    rgba[di] = c[0];
                    rgba[di + 1] = c[1];
                    rgba[di + 2] = c[2];
                    rgba[di + 3] = 255;
                }
            }
        }
        off += pix;
        tiles.insert(
            (start as u16).saturating_add(i as u16),
            TileRgba {
                w: w as u32,
                h: h as u32,
                rgba,
                xoff,
                yoff,
            },
        );
    }
    Ok(())
}

fn apply_hrp_overlay(pack_dir: &Path, bank: &mut ArtBank) {
    let mut images: BTreeMap<u16, PathBuf> = BTreeMap::new();
    collect_hrp_images(pack_dir, &mut images);
    let mut zips = Vec::new();
    walk_ext(pack_dir, "zip", &mut zips);
    for zip in zips {
        if let Ok(file) = std::fs::File::open(&zip) {
            load_hrp_zip(file, bank, &mut images);
        }
    }
    for (tile, path) in images {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Some(mut img) = decode_image_rgba(&bytes) {
                key_build_rgba(&mut img.rgba);
                bank.tiles.insert(tile, img);
            }
        }
    }
}

fn collect_hrp_images(root: &Path, images: &mut BTreeMap<u16, PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    let mut n = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            n += 1;
            if n > 32_768 {
                return;
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
            if !matches!(ext.as_str(), "png" | "tga" | "jpg" | "jpeg") {
                continue;
            }
            if let Some(id) = tile_id_from_name(&path) {
                images.entry(id).or_insert(path);
            }
        }
    }
}

fn tile_id_from_name(path: &Path) -> Option<u16> {
    let stem = path.file_stem()?.to_str()?;
    let digits: String = stem.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // Prefer a trailing number (highres/01234.png, tile_1234).
    let tail = stem
        .rsplit(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())?;
    tail.parse().ok()
}

fn load_hrp_zip(
    file: std::fs::File,
    bank: &mut ArtBank,
    loose: &mut BTreeMap<u16, PathBuf>,
) {
    let mut reader = std::io::BufReader::new(file);
    let Ok(dir) = makepad_zip_file::zip_read_central_directory(&mut reader) else {
        return;
    };
    let _ = loose;
    for header in dir.file_headers {
        let name = header.file_name.replace('\\', "/");
        if name.ends_with('/') || name.contains("..") {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".png") && !lower.ends_with(".tga") {
            continue;
        }
        let Some(id) = tile_id_from_name(Path::new(&name)) else {
            continue;
        };
        if let Ok(bytes) = header.extract(&mut reader) {
            if let Some(mut img) = decode_image_rgba(&bytes) {
                key_build_rgba(&mut img.rgba);
                bank.tiles.insert(id, img);
            }
        }
    }
}

fn decode_image_rgba(bytes: &[u8]) -> Option<TileRgba> {
    if bytes.starts_with(b"\x89PNG") {
        return decode_png(bytes);
    }
    decode_tga(bytes)
}

fn decode_png(bytes: &[u8]) -> Option<TileRgba> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    let mut dec = makepad_zune_png::PngDecoder::new(ZCursor::new(bytes));
    dec.decode_headers().ok()?;
    let (w, h) = dec.dimensions()?;
    let comps = dec.colorspace()?.num_components();
    let raw = dec.decode_raw().ok()?;
    let rgba = rgba_from_channels(&raw, w as u32, h as u32, comps)?;
    Some(TileRgba {
        w: w as u32,
        h: h as u32,
        rgba,
        xoff: 0,
        yoff: 0,
    })
}

fn rgba_from_channels(v: &[u8], w: u32, h: u32, components: usize) -> Option<Vec<u8>> {
    let n = w as usize * h as usize;
    match components {
        4 if v.len() >= n * 4 => Some(v[..n * 4].to_vec()),
        3 if v.len() >= n * 3 => {
            let mut o = Vec::with_capacity(n * 4);
            for c in v.chunks_exact(3).take(n) {
                o.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            Some(o)
        }
        1 if v.len() >= n => {
            let mut o = Vec::with_capacity(n * 4);
            for &c in v.iter().take(n) {
                o.extend_from_slice(&[c, c, c, 255]);
            }
            Some(o)
        }
        _ => None,
    }
}

fn decode_tga(bytes: &[u8]) -> Option<TileRgba> {
    if bytes.len() < 18 {
        return None;
    }
    let itype = bytes[2];
    let w = u16_le(bytes, 12) as usize;
    let h = u16_le(bytes, 14) as usize;
    let bpp = bytes[16];
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let origin_top = bytes[17] & 0x20 != 0;
    let src = &bytes[18 + bytes[0] as usize..];
    let ch = (bpp / 8) as usize;
    if !matches!(itype, 2 | 10) || !matches!(ch, 3 | 4) {
        return None;
    }
    let raw = if itype == 2 {
        src.get(..w * h * ch)?.to_vec()
    } else {
        unpack_tga_rle(src, w * h, ch)?
    };
    if raw.len() < w * h * ch {
        return None;
    }
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if origin_top { y } else { h - 1 - y };
        for x in 0..w {
            let si = (sy * w + x) * ch;
            let di = (y * w + x) * 4;
            rgba[di] = raw[si + 2];
            rgba[di + 1] = raw[si + 1];
            rgba[di + 2] = raw[si];
            rgba[di + 3] = if ch == 4 { raw[si + 3] } else { 255 };
        }
    }
    Some(TileRgba {
        w: w as u32,
        h: h as u32,
        rgba,
        xoff: 0,
        yoff: 0,
    })
}

fn unpack_tga_rle(src: &[u8], pixels: usize, ch: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(pixels * ch);
    let mut i = 0usize;
    while out.len() < pixels * ch {
        if i >= src.len() {
            return None;
        }
        let pkt = src[i];
        i += 1;
        let count = (pkt & 0x7f) as usize + 1;
        if pkt & 0x80 != 0 {
            if i + ch > src.len() {
                return None;
            }
            for _ in 0..count {
                out.extend_from_slice(&src[i..i + ch]);
            }
            i += ch;
        } else {
            let n = count * ch;
            if i + n > src.len() {
                return None;
            }
            out.extend_from_slice(&src[i..i + n]);
            i += n;
        }
    }
    Some(out)
}

fn walk_ext(root: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    let mut n = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            n += 1;
            if n > 16_384 {
                return;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .eq_ignore_ascii_case(ext)
            {
                out.push(path);
            }
        }
    }
}

/// `ST_20_CEILING_DOOR` and `ST_21_FLOOR_DOOR`: the two BUILD door tags
/// whose sector is authored CLOSED, so a static bake seals the doorway.
pub(crate) const ST_CEILING_DOOR: i16 = 20;
pub(crate) const ST_FLOOR_DOOR: i16 = 21;
/// `ST_15..17`: elevators. Authored at one end of their travel.
pub(crate) const ST_LIFT_TAGS: &[i16] = &[15, 16, 17];
/// `ST_1_ABOVE_WATER` / `ST_2_UNDERWATER`: the two halves of a BUILD water
/// pair. The first sector's floor is the SURFACE you drop through; the
/// second's is the bottom you stand on.
pub(crate) const ST_ABOVE_WATER: i16 = 1;
pub(crate) const ST_UNDERWATER: i16 = 2;

/// BUILD control tiles this exporter reads. The rest stay decoration.
/// `SECTOREFFECTOR` lotag 7 is a teleporter; `ACCESSCARD`'s palette is the
/// key's colour; `NUKEBUTTON` ends the level; `APLAYER` is a multiplayer
/// start.
pub(crate) const TILE_SECTOREFFECTOR: i16 = 1;
pub(crate) const TILE_ACCESSCARD: i16 = 60;
pub(crate) const TILE_NUKEBUTTON: i16 = 142;
pub(crate) const TILE_APLAYER: i16 = 1405;
/// `SE_7_TELEPORT`: two effectors sharing a hitag are the two ends of one
/// teleporter.
pub(crate) const SE_TELEPORT: i16 = 7;

/// What a moving BUILD sector IS, in the contract's vocabulary.
///
/// A ceiling door and an elevator are different NODES because they are
/// different to walk into: a door is authored closed and must be baked open
/// or the level is sealed, while a lift is authored where you meet it and
/// travels away. A BUILD `ST_21_FLOOR_DOOR` is a floor that drops and comes
/// back — the same motion as an elevator and the same thing to stand on —
/// so it is a lift here, exactly as Doom files every plat special under
/// `lift_N`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoverKind {
    Door,
    Lift,
}

/// One BUILD sector that moves, and where it moves to (BUILD z units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BuildMover {
    pub sector: usize,
    /// Ceiling/floor as authored, and where the effect takes it.
    pub closed_z: i32,
    pub open_z: i32,
    /// True for a ceiling door (the ceiling rises); false for a floor door
    /// or lift (the floor drops).
    pub ceiling: bool,
    pub kind: MoverKind,
}

/// One damaging or swimmable group of BUILD sectors, grouped the way Doom
/// groups its nukage: by what the surface DOES, not one node per sector.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BuildHazard {
    /// `hazard_1`, `hazard_2`, … in a stable order.
    pub name: String,
    pub sectors: Vec<usize>,
    pub damage: u8,
    pub flat: String,
    pub liquid: bool,
    /// False for a water SURFACE (you drop through it), true for the bottom
    /// you stand on.
    pub solid: bool,
}

/// Water sectors of a BUILD map. `ST_1_ABOVE_WATER` is the surface you swim
/// down through, `ST_2_UNDERWATER` the bottom of the pool — the only liquid
/// BUILD declares as data rather than as a texture name, so it is the only
/// one published here.
pub(crate) fn build_hazards(map: &BuildMap) -> Vec<BuildHazard> {
    let mut surface = Vec::new();
    let mut bottom = Vec::new();
    for (si, sec) in map.sectors.iter().enumerate() {
        match sec.lotag {
            ST_ABOVE_WATER => surface.push(si),
            ST_UNDERWATER => bottom.push(si),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (sectors, solid) in [(surface, false), (bottom, true)] {
        if sectors.is_empty() {
            continue;
        }
        out.push(BuildHazard {
            name: format!("hazard_{}", out.len() + 1),
            sectors,
            damage: 0,
            flat: "water".into(),
            liquid: true,
            solid,
        });
    }
    out
}

#[derive(Clone)]
pub(crate) struct BuildMap {
    start: [i32; 3],
    start_ang: i16,
    start_sec: i16,
    sectors: Vec<Sector>,
    walls: Vec<Wall>,
    sprites: Vec<Sprite>,
}

#[derive(Clone)]
struct Sector {
    wallptr: u16,
    wallnum: u16,
    ceilingz: i32,
    floorz: i32,
    ceilingstat: i16,
    floorstat: i16,
    ceilingpicnum: i16,
    ceilingheinum: i16,
    ceilingshade: i8,
    ceilingxpanning: u8,
    ceilingypanning: u8,
    floorpicnum: i16,
    floorheinum: i16,
    floorshade: i8,
    floorxpanning: u8,
    floorypanning: u8,
    /// BUILD sector tag: what this sector DOES (20/21 doors, 15-17 lifts,
    /// 1/2 water).
    lotag: i16,
    /// The tag that GROUPS sectors — a lift's floor target, the partner of a
    /// water pair. Parsed because a contract fact that is only in `hitag`
    /// cannot be published without it.
    hitag: i16,
}

#[derive(Clone)]
struct Wall {
    x: i32,
    y: i32,
    point2: i16,
    nextwall: i16,
    nextsector: i16,
    cstat: i16,
    picnum: i16,
    overpicnum: i16,
    shade: i8,
    xrepeat: u8,
    yrepeat: u8,
    xpanning: u8,
    ypanning: u8,
}

#[derive(Clone)]
struct Sprite {
    x: i32,
    y: i32,
    z: i32,
    picnum: i16,
    cstat: i16,
    shade: i8,
    /// Palette swap. On an `ACCESSCARD` it IS the key's colour, so a keycard
    /// cannot be published without it.
    pal: u8,
    xrepeat: u8,
    yrepeat: u8,
    /// Sector the sprite stands in — the pad of a teleporter effector.
    sectnum: i16,
    ang: i16,
    /// `SECTOREFFECTOR` lotag (7 = teleport) and, on a `NUKEBUTTON`, which
    /// exit it is.
    lotag: i16,
    /// What a `SECTOREFFECTOR` is paired with.
    hitag: i16,
}

/// Duke's `PHEIGHT`: the eye sits `38 << 8` BUILD z units above the floor.
///
/// The map header's `posz` is NOT this. It is wherever Mapster's 3D camera
/// last stood — 4 metres up over E1L1's street — so deriving an eye height
/// from it publishes a fact the map never stated and stands the walker in
/// mid-air. Every other classic converter publishes its engine's own view
/// height (Doom `VIEWHEIGHT` 41, Quake 22+24); this is Build's.
///
/// At this converter's scale that is 1.19 m, in rooms whose median height is
/// 3 m — Duke's world is about twice Doom's, which is exactly why the eye
/// has to be DECLARED rather than assumed by the walker.
const DUKE_VIEW_HEIGHT_Z: i32 = 38 << 8;

/// Every navigation fact a BUILD map states about itself.
///
/// Nothing here is invented. The start position and the floor under it are
/// in the map header and the start sector; the eye above that floor is
/// Build's own constant. The STEP height is not in either, so it is not
/// published: that is the one thing the walker's own Build preset is for.
fn map_nav(map: &BuildMap, spawn: [f32; 5]) -> crate::world_nav::WorldNav {
    use crate::world_nav::{deathmatch_name, NavStart, NavTeleport, WorldNav};
    let floor_y = start_floor_y(map);
    let eye_height = DUKE_VIEW_HEIGHT_Z as f32 * SCALE_Z;
    let mut nav = WorldNav::single(
        [spawn[0], floor_y + eye_height, spawn[2]],
        spawn[3],
        spawn[4],
    );
    nav.floor_y = Some(floor_y);
    nav.eye_height = Some(eye_height);

    // Multiplayer starts. BUILD keeps them as `APLAYER` sprites; the header
    // start is player 1, so these are the deathmatch spots.
    for s in map.sprites.iter().filter(|s| s.picnum == TILE_APLAYER) {
        nav.starts.push(NavStart {
            name: deathmatch_name(nav.starts.len() - 1),
            pos: sprite_eye(map, s, eye_height),
            yaw: build_yaw(s.ang),
            pitch: 0.0,
        });
    }

    // Keycards and the exit. A walker that cannot see which door wants which
    // card walks the level in the wrong order forever.
    for s in &map.sprites {
        let name = match s.picnum {
            TILE_ACCESSCARD => match s.pal {
                0 => "key_blue",
                21 => "key_red",
                23 => "key_yellow",
                _ => continue,
            }
            .to_string(),
            // `NUKEBUTTON` ends the level; a non-zero lotag is the one that
            // ends it into the secret level.
            TILE_NUKEBUTTON => if s.lotag == 0 { "exit" } else { "exit_secret" }.to_string(),
            _ => continue,
        };
        if nav.markers.iter().any(|m| m.name == name) {
            continue;
        }
        nav.markers.push(NavStart {
            name,
            pos: sprite_eye(map, s, eye_height),
            yaw: build_yaw(s.ang),
            pitch: 0.0,
        });
    }

    // Teleporters: two `SE_7` effectors sharing a hitag are the two ends of
    // one. Step anywhere in the first one's sector and you arrive at the
    // second — a room reachable ONLY that way is unreachable to a navigator
    // that does not know.
    let mut ends: BTreeMap<i16, Vec<&Sprite>> = BTreeMap::new();
    for s in map
        .sprites
        .iter()
        .filter(|s| s.picnum == TILE_SECTOREFFECTOR && s.lotag == SE_TELEPORT)
    {
        ends.entry(s.hitag).or_default().push(s);
    }
    for pair in ends.values() {
        if pair.len() < 2 {
            continue;
        }
        for (from, to) in [(pair[0], pair[1]), (pair[1], pair[0])] {
            let Some((pad_min, pad_max)) = sector_pad(map, from.sectnum) else {
                continue;
            };
            nav.teleports.push(NavTeleport {
                name: format!("teleport_{}", nav.teleports.len() + 1),
                pad_min,
                pad_max,
                dst: sprite_eye(map, to, eye_height),
                yaw: build_yaw(to.ang),
            });
        }
    }
    nav
}

/// BUILD's 2048-step compass to the engine's yaw about +Y.
fn build_yaw(ang: i16) -> f32 {
    (ang as f32) * std::f32::consts::PI / 1024.0
}

/// A sprite's position as an EYE, like every other start: the floor of the
/// sector it stands in, plus the map's own eye height.
fn sprite_eye(map: &BuildMap, s: &Sprite, eye_height: f32) -> [f32; 3] {
    let x = s.x as f32;
    let y = s.y as f32;
    let floor = map
        .sectors
        .get(s.sectnum.max(0) as usize)
        .map(|sec| slope_z(sec, map, x, y, false))
        .unwrap_or(-s.z as f32 * SCALE_Z);
    [x * SCALE, floor + eye_height, -y * SCALE]
}

/// A sector's footprint in the GLB's x/z plane — the pad a teleport
/// effector covers. BUILD y runs the opposite way to the engine's z, so the
/// bounds swap ends.
fn sector_pad(map: &BuildMap, sectnum: i16) -> Option<([f32; 2], [f32; 2])> {
    let sec = map.sectors.get(sectnum.max(0) as usize)?;
    let first = sec.wallptr as usize;
    let count = sec.wallnum as usize;
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for w in first..first.saturating_add(count) {
        let wall = map.walls.get(w)?;
        lo[0] = lo[0].min(wall.x as f32);
        lo[1] = lo[1].min(wall.y as f32);
        hi[0] = hi[0].max(wall.x as f32);
        hi[1] = hi[1].max(wall.y as f32);
    }
    if lo[0] > hi[0] {
        return None;
    }
    Some((
        [lo[0] * SCALE, -hi[1] * SCALE],
        [hi[0] * SCALE, -lo[1] * SCALE],
    ))
}

/// Centre of a sector's wall loop, in BUILD x/y units.
fn sector_centre(map: &BuildMap, sector: &Sector) -> [f32; 2] {
    let first = sector.wallptr as usize;
    let count = sector.wallnum as usize;
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for w in first..first.saturating_add(count) {
        let Some(wall) = map.walls.get(w) else { continue };
        lo[0] = lo[0].min(wall.x as f32);
        lo[1] = lo[1].min(wall.y as f32);
        hi[0] = hi[0].max(wall.x as f32);
        hi[1] = hi[1].max(wall.y as f32);
    }
    if lo[0] > hi[0] {
        return [0.0, 0.0];
    }
    [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]
}

/// Doors and lifts of a BUILD map: sector tags 20/21 (doors) and 15-17
/// (elevators). A closed BUILD door has its ceiling on its floor exactly
/// like Doom's, so the static bake must open it or the level is sealed.
///
/// The open height is the neighbouring sector the effect reveals: the
/// HIGHEST neighbouring ceiling for a ceiling door, the LOWEST neighbouring
/// floor for a floor door or lift.
pub(crate) fn build_movers(map: &BuildMap) -> Vec<BuildMover> {
    let mut out = Vec::new();
    for (si, sector) in map.sectors.iter().enumerate() {
        let ceiling = sector.lotag == ST_CEILING_DOOR;
        let floor = sector.lotag == ST_FLOOR_DOOR || ST_LIFT_TAGS.contains(&sector.lotag);
        if !ceiling && !floor {
            continue;
        }
        let first = sector.wallptr as usize;
        let count = sector.wallnum as usize;
        let mut best: Option<i32> = None;
        for w in first..first.saturating_add(count) {
            let Some(wall) = map.walls.get(w) else { continue };
            let next = wall.nextsector;
            if next < 0 {
                continue;
            }
            let Some(n) = map.sectors.get(next as usize) else {
                continue;
            };
            if n.lotag == sector.lotag {
                continue;
            }
            let z = if ceiling { n.ceilingz } else { n.floorz };
            best = Some(match best {
                // BUILD z grows DOWNWARD: a higher ceiling is a smaller z.
                Some(b) if ceiling => b.min(z),
                Some(b) => b.max(z),
                None => z,
            });
        }
        let Some(open_z) = best else { continue };
        let closed_z = if ceiling { sector.ceilingz } else { sector.floorz };
        // Only a sector that actually travels the right way is a door.
        let moves = if ceiling { open_z < closed_z } else { open_z > closed_z };
        if !moves {
            continue;
        }
        out.push(BuildMover {
            sector: si,
            closed_z,
            open_z,
            ceiling,
            kind: if ceiling { MoverKind::Door } else { MoverKind::Lift },
        });
    }
    out
}

/// Bake every CEILING door at its open position, so the level a walker gets
/// is the one the game lets them walk through.
///
/// Floor movers are deliberately left where BUILD authored them. A lift is
/// something you MEET — its platform is up, its shaft is under it, and the
/// contract asks for the floor to leave the level mesh for a `lift_N` node
/// that rests exactly there. Baking it down (which this used to do for every
/// mover alike) deleted the platform from the picture and left the player
/// standing in the shaft.
pub(crate) fn open_movers(map: &mut BuildMap, movers: &[BuildMover]) {
    for m in movers {
        if !m.ceiling {
            continue;
        }
        let Some(sector) = map.sectors.get_mut(m.sector) else {
            continue;
        };
        sector.ceilingz = m.open_z;
    }
}

pub(crate) fn parse_map_v7(bytes: &[u8]) -> Result<BuildMap, String> {
    if bytes.len() < 26 {
        return Err("MAP too small".into());
    }
    let version = i32_le(bytes, 0);
    if version != 7 && version != 8 && version != 9 {
        return Err(format!("unsupported BUILD MAP version {version}"));
    }
    let start = [i32_le(bytes, 4), i32_le(bytes, 8), i32_le(bytes, 12)];
    let start_ang = i16_le(bytes, 16);
    let start_sec = i16_le(bytes, 18);
    let mut o = 20;
    let nsec = u16_le(bytes, o) as usize;
    o += 2;
    if nsec > 4096 || o + nsec * 40 > bytes.len() {
        return Err("MAP sectors truncated".into());
    }
    let mut sectors = Vec::with_capacity(nsec);
    for _ in 0..nsec {
        sectors.push(Sector {
            wallptr: u16_le(bytes, o),
            wallnum: u16_le(bytes, o + 2),
            ceilingz: i32_le(bytes, o + 4),
            floorz: i32_le(bytes, o + 8),
            ceilingstat: i16_le(bytes, o + 12),
            floorstat: i16_le(bytes, o + 14),
            ceilingpicnum: i16_le(bytes, o + 16),
            ceilingheinum: i16_le(bytes, o + 18),
            ceilingshade: bytes[o + 20] as i8,
            ceilingxpanning: bytes[o + 22],
            ceilingypanning: bytes[o + 23],
            floorpicnum: i16_le(bytes, o + 24),
            floorheinum: i16_le(bytes, o + 26),
            floorshade: bytes[o + 28] as i8,
            floorxpanning: bytes[o + 30],
            floorypanning: bytes[o + 31],
            lotag: i16_le(bytes, o + 34),
            hitag: i16_le(bytes, o + 36),
        });
        o += 40;
    }
    let nwal = u16_le(bytes, o) as usize;
    o += 2;
    if nwal > 16_384 || o + nwal * 32 > bytes.len() {
        return Err("MAP walls truncated".into());
    }
    let mut walls = Vec::with_capacity(nwal);
    for _ in 0..nwal {
        walls.push(Wall {
            x: i32_le(bytes, o),
            y: i32_le(bytes, o + 4),
            point2: i16_le(bytes, o + 8),
            nextwall: i16_le(bytes, o + 10),
            nextsector: i16_le(bytes, o + 12),
            cstat: i16_le(bytes, o + 14),
            picnum: i16_le(bytes, o + 16),
            overpicnum: i16_le(bytes, o + 18),
            shade: bytes[o + 20] as i8,
            xrepeat: bytes[o + 22],
            yrepeat: bytes[o + 23],
            xpanning: bytes[o + 24],
            ypanning: bytes[o + 25],
        });
        o += 32;
    }
    let nspr = if o + 2 <= bytes.len() {
        u16_le(bytes, o) as usize
    } else {
        0
    };
    o += 2;
    let mut sprites = Vec::new();
    if nspr <= 16_384 && o + nspr * 44 <= bytes.len() {
        for _ in 0..nspr {
            sprites.push(Sprite {
                x: i32_le(bytes, o),
                y: i32_le(bytes, o + 4),
                z: i32_le(bytes, o + 8),
                cstat: i16_le(bytes, o + 12),
                picnum: i16_le(bytes, o + 14),
                shade: bytes[o + 16] as i8,
                pal: bytes[o + 17],
                xrepeat: bytes[o + 20],
                yrepeat: bytes[o + 21],
                sectnum: i16_le(bytes, o + 24),
                ang: i16_le(bytes, o + 28),
                lotag: i16_le(bytes, o + 38),
                hitag: i16_le(bytes, o + 40),
            });
            o += 44;
        }
    }
    Ok(BuildMap {
        start,
        start_ang,
        start_sec,
        sectors,
        walls,
        sprites,
    })
}

/// Everything one BUILD map becomes: the GLB, and the facts about it that
/// only the exporter knows — where the contract nodes ended up.
pub(crate) struct BuildWorld {
    pub glb: Vec<u8>,
    pub spawn: Option<[f32; 5]>,
    pub sprites: BTreeSet<u16>,
    pub doors: Vec<crate::world_nav::NavDoor>,
    pub lifts: Vec<crate::world_nav::NavDoor>,
}

/// The GLB alone — what the mesh tests want.
fn map_to_glb(
    src: &BuildMap,
    art: &ArtBank,
) -> Result<(Vec<u8>, Option<[f32; 5]>, BTreeSet<u16>), String> {
    let w = map_to_world(src, art)?;
    Ok((w.glb, w.spawn, w.sprites))
}

fn map_to_world(src: &BuildMap, art: &ArtBank) -> Result<BuildWorld, String> {
    let fallback = TileRgba {
        w: 16,
        h: 16,
        rgba: vec![0x90; 16 * 16 * 4],
        xoff: 0,
        yoff: 0,
    };
    // The moving sectors are found on the map AS AUTHORED — the geometry a
    // door node carries is its closed pose, and the level is what is left
    // once the ceiling doors are lifted out of the way.
    let movers = build_movers(src);
    let hazards = build_hazards(src);
    let mut open = src.clone();
    open_movers(&mut open, &movers);
    let map = &open;
    let mover_of_sector: BTreeMap<usize, usize> =
        movers.iter().enumerate().map(|(i, m)| (m.sector, i)).collect();
    let mut hazard_of_sector: BTreeMap<usize, usize> = BTreeMap::new();
    for (hi, h) in hazards.iter().enumerate() {
        for &si in &h.sectors {
            hazard_of_sector.insert(si, hi);
        }
    }

    let mut sink = MeshSink::new(movers.len(), hazards.len());
    for (si, _sec) in map.sectors.iter().enumerate() {
        // A lift's floor is the thing that travels: it leaves the level for
        // its own node, authored where BUILD drew it. A water surface leaves
        // for a hazard node so collision can tag it in one pass. Everything
        // else, including a ceiling door's own planes, stays level.
        let floor_target = match mover_of_sector.get(&si) {
            Some(&mi) if !movers[mi].ceiling => SinkTarget::Mover(mi),
            _ => match hazard_of_sector.get(&si) {
                Some(&hi) => SinkTarget::Hazard(hi),
                None => SinkTarget::Level,
            },
        };
        emit_sector_planes(&mut sink, map, si, art, &fallback, floor_target);
        emit_sector_walls(&mut sink, map, si, art, &fallback, &movers, &mover_of_sector);
    }
    emit_sprites(&mut sink, map, art, &fallback);
    let mut sky = emit_parallax_sky(map, art, &fallback);
    // Darkening shades ride the material baseColorFactor over one shared
    // unshaded image per tile; the renderer's tint lane clamps at 1.0, so
    // brightening (negative) shades stay baked into their own PNG.
    let mut pngs: BTreeMap<(i16, i8), Vec<u8>> = BTreeMap::new();
    for key in sink.all().flat_map(|m| m.keys()) {
        let baked = key.1.min(0);
        pngs.entry((key.0, baked)).or_insert_with(|| {
            let t = tile_ref(art, &fallback, key.0);
            shaded_png(t, baked)
        });
    }
    // BUILD shade is already baked (baseColorFactor for darkening, into the
    // PNG for brightening), so the level is PRELIT: without the
    // `lightmapTexture` marker the analytic sun multiplies it again and
    // interiors go black. The marker points at the part's own image — a
    // prelit shader never samples it, and a second image would trip
    // pack_import's one-texture rule.
    // Weld the T-junctions BUILD leaves behind: a sector's floor and its
    // neighbour's wall are cut at different points, and each texture bucket
    // is a mesh of its own, so the grid spans every bucket and each is
    // welded against it. Splitting only inserts positions the level already
    // has, so bucket order does not matter.
    {
        let mut parts: Vec<&[[f32; 3]]> = sink
            .all()
            .flat_map(|m| m.values())
            .map(|b| &b.positions[..])
            .collect();
        if let Some(ref sky) = sky {
            // The sky hull is a shell around the map rather than level
            // surface, but it is cut from the same sector rings and can pick
            // a corner up where the world does not.
            parts.push(&sky.positions[..]);
        }
        let weld = crate::classic_import::weld_parts(&parts);
        for bucket in sink.all_mut().flat_map(|m| m.values_mut()) {
            weld.split(crate::classic_import::WeldSoup {
                positions: &mut bucket.positions,
                uvs: &mut bucket.uvs,
                normals: None,
                colors: None,
                indices: &mut bucket.indices,
            });
        }
        if let Some(sky) = sky.as_mut() {
            weld.split(crate::classic_import::WeldSoup {
                positions: &mut sky.positions,
                uvs: &mut sky.uvs,
                normals: Some(&mut sky.normals),
                colors: None,
                indices: &mut sky.indices,
            });
        }
    }
    let marker_uvs: Vec<[f32; 2]> = vec![
        [0.0, 0.0];
        sink.all()
            .flat_map(|m| m.values())
            .map(|b| b.positions.len())
            .max()
            .unwrap_or(0)
    ];
    // The level's own materials, in the order `write_glb_mesh_textured_parts`
    // writes them — a moving node's primitives name these by index, so the
    // door leaf keeps the tile it was drawn with instead of being repainted
    // with whichever tile happened to sort first.
    let mut parts = Vec::new();
    let mut material_of_key: BTreeMap<(i16, i8), usize> = BTreeMap::new();
    for (key, bucket) in &sink.level {
        if bucket.indices.len() < 3 {
            continue;
        }
        let Some(png) = pngs.get(&(key.0, key.1.min(0))) else {
            continue;
        };
        material_of_key.insert(*key, parts.len());
        parts.push(GlbTexturedPart {
            positions: &bucket.positions,
            uvs: &bucket.uvs,
            indices: &bucket.indices,
            base_color_png: png,
            normals: None,
            base_color_factor: shade_factor(key.1),
            colors: None,
            lightmap_png: Some(png),
            lightmap_uvs: Some(&marker_uvs[..bucket.positions.len()]),
            detail_png: None,
            detail_scale: [0.0, 0.0],
        });
    }
    // A tile a door leaf uses but the level never does still needs a
    // material. It goes in as an empty part so the index space stays the one
    // `parts` defines, and an empty primitive draws nothing.
    let empty_pos: Vec<[f32; 3]> = Vec::new();
    let empty_uv: Vec<[f32; 2]> = Vec::new();
    let empty_idx: Vec<u32> = Vec::new();
    let mut node_only_keys: Vec<(i16, i8)> = Vec::new();
    for key in sink
        .movers
        .iter()
        .chain(sink.hazards.iter())
        .flat_map(|m| m.keys())
    {
        if !material_of_key.contains_key(key) && !node_only_keys.contains(key) {
            node_only_keys.push(*key);
        }
    }
    for key in &node_only_keys {
        let Some(png) = pngs.get(&(key.0, key.1.min(0))) else {
            continue;
        };
        material_of_key.insert(*key, parts.len());
        parts.push(GlbTexturedPart {
            positions: &empty_pos,
            uvs: &empty_uv,
            indices: &empty_idx,
            base_color_png: png,
            normals: None,
            base_color_factor: shade_factor(key.1),
            colors: None,
            lightmap_png: Some(png),
            lightmap_uvs: Some(&empty_uv),
            detail_png: None,
            detail_scale: [0.0, 0.0],
        });
    }
    let glb = write_glb_mesh_textured_parts(&parts, true);
    if !glb.starts_with(b"glTF") {
        return Err("GLB encode failed".into());
    }
    // Doors, lifts, water and the sky leave the level mesh for nodes of their
    // own, exactly as Doom and Quake write them.
    let mut extra: Vec<crate::glb_nodes::ExtraNode> = Vec::new();
    let mut door_meta: Vec<crate::world_nav::NavDoor> = Vec::new();
    let mut lift_meta: Vec<crate::world_nav::NavDoor> = Vec::new();
    for (mi, m) in movers.iter().enumerate() {
        let node_parts = node_primitives(&sink.movers[mi], &material_of_key);
        if node_parts.is_empty() {
            continue;
        }
        let closed_y = -m.closed_z as f32 * SCALE_Z;
        let open_y = -m.open_z as f32 * SCALE_Z;
        let centre = src
            .sectors
            .get(m.sector)
            .map(|s| sector_centre(src, s))
            .unwrap_or([0.0, 0.0]);
        // Geometry is in level space; the anchor is the doorway centre.
        let pos = [centre[0] * SCALE, closed_y, -centre[1] * SCALE];
        match m.kind {
            MoverKind::Door => {
                let name = format!("door_{}", door_meta.len() + 1);
                let mut node = crate::glb_nodes::ExtraNode::door(
                    name.clone(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    closed_y,
                    open_y,
                    false,
                    None,
                );
                node.parts = node_parts;
                extra.push(node);
                door_meta.push(crate::world_nav::NavDoor {
                    name,
                    pos,
                    closed_y,
                    open_y,
                });
            }
            MoverKind::Lift => {
                let name = format!("lift_{}", lift_meta.len() + 1);
                let mut node = crate::glb_nodes::ExtraNode::lift(
                    name.clone(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    closed_y,
                    open_y,
                );
                node.parts = node_parts;
                extra.push(node);
                lift_meta.push(crate::world_nav::NavDoor {
                    name,
                    pos,
                    closed_y,
                    open_y,
                });
            }
        }
    }
    for (hi, h) in hazards.iter().enumerate() {
        let node_parts = node_primitives(&sink.hazards[hi], &material_of_key);
        if node_parts.is_empty() {
            continue;
        }
        let mut node = crate::glb_nodes::ExtraNode::hazard(
            h.name.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            h.damage,
            &h.flat,
            h.liquid,
            h.solid,
        );
        node.parts = node_parts;
        extra.push(node);
    }
    if let Some(sky) = sky.take() {
        // BUILD's parallax ceiling wraps its strip exactly once around the
        // compass, and the hull is built centred on the player start, so the
        // phase is already in the geometry: repeat 1, offset 0.
        extra.push(crate::glb_nodes::ExtraNode::sky(
            sky.positions,
            sky.uvs,
            sky.indices,
            vec![sky.png],
            "cylinder",
            1.0,
            0.0,
            &format!("tile-{}", sky.pic),
            None,
            None,
        ));
    }
    let glb = crate::glb_nodes::inject_nodes(&glb, &extra).unwrap_or(glb);
    let yaw = build_yaw(map.start_ang);
    // The start is an EYE above the start sector's floor, like every other
    // classic converter's — not the map header's `posz`, which is only where
    // the editor's 3D camera was parked.
    let spawn = Some([
        map.start[0] as f32 * SCALE,
        start_floor_y(map) + DUKE_VIEW_HEIGHT_Z as f32 * SCALE_Z,
        -map.start[1] as f32 * SCALE,
        yaw,
        -0.08,
    ]);
    let mut sprites = BTreeSet::new();
    for s in &map.sprites {
        if s.picnum >= 0 && (s.cstat as u16) & 0x8000 == 0 {
            sprites.insert(s.picnum as u16);
        }
    }
    Ok(BuildWorld {
        glb,
        spawn,
        sprites,
        doors: door_meta,
        lifts: lift_meta,
    })
}

/// A darkening BUILD shade rides the material's `baseColorFactor`; a
/// brightening one is already baked into its own PNG.
fn shade_factor(shade: i8) -> Option<[f32; 4]> {
    if shade > 0 {
        let m = shade_mul(shade);
        Some([m, m, m, 1.0])
    } else {
        None
    }
}

/// One node's buckets as glTF primitives, each keeping the level material
/// its tile already has.
fn node_primitives(
    buckets: &BTreeMap<(i16, i8), MeshBucket>,
    material_of_key: &BTreeMap<(i16, i8), usize>,
) -> Vec<crate::glb_nodes::ExtraPart> {
    let mut out = Vec::new();
    for (key, bucket) in buckets {
        if bucket.indices.len() < 3 || bucket.positions.is_empty() {
            continue;
        }
        let Some(&material) = material_of_key.get(key) else {
            continue;
        };
        out.push(crate::glb_nodes::ExtraPart {
            positions: bucket.positions.clone(),
            uvs: bucket.uvs.clone(),
            indices: bucket.indices.clone(),
            colors: Vec::new(),
            material,
        });
    }
    out
}

fn sector_loops(map: &BuildMap, sec: &Sector) -> Vec<Vec<usize>> {
    let start = sec.wallptr as usize;
    let n = sec.wallnum as usize;
    if start >= map.walls.len() {
        return Vec::new();
    }
    let end = (start + n).min(map.walls.len());
    let mut used = vec![false; end - start];
    let mut loops = Vec::new();
    for i in 0..end - start {
        if used[i] {
            continue;
        }
        let mut ring = Vec::new();
        let first = start + i;
        let mut w = first;
        for _ in 0..n + 2 {
            if w < start || w >= end {
                break;
            }
            let li = w - start;
            if used[li] {
                break;
            }
            used[li] = true;
            ring.push(w);
            let next = map.walls[w].point2 as usize;
            if next == first {
                break;
            }
            w = next;
        }
        if ring.len() >= 3 {
            loops.push(ring);
        }
    }
    loops
}

fn ring_area_ids(map: &BuildMap, ids: &[usize]) -> f32 {
    let pts: Vec<[f32; 2]> = ids
        .iter()
        .filter_map(|&i| map.walls.get(i).map(world_xy))
        .collect();
    signed_area(&pts)
}

fn world_xy(w: &Wall) -> [f32; 2] {
    [w.x as f32, w.y as f32]
}

fn slope_z(sec: &Sector, map: &BuildMap, x: f32, y: f32, ceil: bool) -> f32 {
    let base = if ceil { sec.ceilingz } else { sec.floorz };
    let sloped = if ceil {
        sec.ceilingstat & 2 != 0
    } else {
        sec.floorstat & 2 != 0
    };
    if !sloped || sec.wallnum == 0 {
        return -base as f32 * SCALE_Z;
    }
    let heinum = if ceil {
        sec.ceilingheinum
    } else {
        sec.floorheinum
    } as f32;
    let w0 = sec.wallptr as usize;
    if w0 >= map.walls.len() {
        return -base as f32 * SCALE_Z;
    }
    let a = &map.walls[w0];
    let b = map
        .walls
        .get(a.point2 as usize)
        .unwrap_or(a);
    let dx = (b.x - a.x) as f32;
    let dy = (b.y - a.y) as f32;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let nx = -dy / len;
    let ny = dx / len;
    let dist = (x - a.x as f32) * nx + (y - a.y as f32) * ny;
    // getzsofslope: dmulscale3(cross)/ (nsqrt(len²)<<5) = dist * heinum / 256.
    // /4096 is the editor's 45° number; the Z store is 16× finer than XY.
    let z = base as f32 + dist * heinum / 256.0;
    -z * SCALE_Z
}

#[derive(Default)]
struct MeshBucket {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// Where the triangle an emitter is about to write belongs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SinkTarget {
    #[default]
    Level,
    /// Index into the map's movers — a `door_N` or `lift_N` node.
    Mover(usize),
    /// Index into the map's hazard groups — a `hazard_N` node.
    Hazard(usize),
}

/// Where a Build emitter sends its triangles.
///
/// BUILD buckets geometry by `(picnum, shade)` because it keeps one material
/// per tile rather than one atlas — a BUILD wall tiles its texture along its
/// length and an atlas cannot wrap. That bucketing is what used to make a
/// door impossible to lift out of the level mesh: by the time triangles
/// existed, the sector they came from was gone.
///
/// So the bucket map is no longer the thing emitters write to. They write to
/// a sink, which keeps one bucket map per destination and routes by whatever
/// `target` is set to at that moment — the same shape Doom's `DoorSink` /
/// `LiftSink` / `HazardSink` have. Each destination keeps its own
/// `(picnum, shade)` split, and each becomes one primitive of the node.
#[derive(Default)]
struct MeshSink {
    level: BTreeMap<(i16, i8), MeshBucket>,
    movers: Vec<BTreeMap<(i16, i8), MeshBucket>>,
    hazards: Vec<BTreeMap<(i16, i8), MeshBucket>>,
    target: SinkTarget,
}

impl MeshSink {
    fn new(movers: usize, hazards: usize) -> Self {
        Self {
            level: BTreeMap::new(),
            movers: (0..movers).map(|_| BTreeMap::new()).collect(),
            hazards: (0..hazards).map(|_| BTreeMap::new()).collect(),
            target: SinkTarget::Level,
        }
    }

    fn bucket(&mut self, key: (i16, i8)) -> &mut MeshBucket {
        let map = match self.target {
            SinkTarget::Level => &mut self.level,
            SinkTarget::Mover(i) => self.movers.get_mut(i).unwrap_or(&mut self.level),
            SinkTarget::Hazard(i) => self.hazards.get_mut(i).unwrap_or(&mut self.level),
        };
        map.entry(key).or_default()
    }

    /// Run `f` with the sink pointed somewhere else, then put it back.
    fn into_target<R>(&mut self, target: SinkTarget, f: impl FnOnce(&mut Self) -> R) -> R {
        let was = std::mem::replace(&mut self.target, target);
        let out = f(self);
        self.target = was;
        out
    }

    /// Every bucket map, level first — what the weld pass spans.
    fn all(&self) -> impl Iterator<Item = &BTreeMap<(i16, i8), MeshBucket>> {
        std::iter::once(&self.level)
            .chain(self.movers.iter())
            .chain(self.hazards.iter())
    }

    fn all_mut(&mut self) -> impl Iterator<Item = &mut BTreeMap<(i16, i8), MeshBucket>> {
        std::iter::once(&mut self.level)
            .chain(self.movers.iter_mut())
            .chain(self.hazards.iter_mut())
    }
}

fn tile_wh(tile: &TileRgba) -> (f32, f32) {
    (tile.w.max(1) as f32, tile.h.max(1) as f32)
}

fn tile_ref<'a>(art: &'a ArtBank, fallback: &'a TileRgba, pic: i16) -> &'a TileRgba {
    art.tiles.get(&(pic.max(0) as u16)).unwrap_or(fallback)
}

fn is_key_rgb(r: u8, g: u8, b: u8) -> bool {
    r >= 200 && b >= 200 && g <= 48
}

fn key_build_rgba(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        if px[3] < 8 || is_key_rgb(px[0], px[1], px[2]) {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 0;
        }
    }
}

/// BUILD shade tables are 32 steps: 0 is fullbright, 31 is nearly black.
fn shade_mul(shade: i8) -> f32 {
    ((32.0 - shade as f32) / 32.0).clamp(0.05, 1.4)
}

fn apply_shade_rgb(rgba: &mut [u8], shade: i8) {
    if shade == 0 {
        return;
    }
    let m = shade_mul(shade);
    for px in rgba.chunks_exact_mut(4) {
        if px[3] == 0 {
            continue;
        }
        px[0] = ((px[0] as f32) * m).round().clamp(0.0, 255.0) as u8;
        px[1] = ((px[1] as f32) * m).round().clamp(0.0, 255.0) as u8;
        px[2] = ((px[2] as f32) * m).round().clamp(0.0, 255.0) as u8;
    }
}

fn shaded_png(tile: &TileRgba, shade: i8) -> Vec<u8> {
    if shade == 0 {
        return encode_png_rgba(&tile.rgba, tile.w, tile.h).unwrap_or_else(|_| vec![0x90; 16]);
    }
    let mut rgba = tile.rgba.clone();
    apply_shade_rgb(&mut rgba, shade);
    encode_png_rgba(&rgba, tile.w, tile.h).unwrap_or_else(|_| vec![0x90; 16])
}

fn emit_sector_planes(
    sink: &mut MeshSink,
    map: &BuildMap,
    si: usize,
    art: &ArtBank,
    fallback: &TileRgba,
    floor_target: SinkTarget,
) {
    let sec = &map.sectors[si];
    let mut loops = sector_loops(map, sec);
    if loops.is_empty() {
        return;
    }
    loops.sort_by(|a, b| {
        ring_area_ids(map, b)
            .abs()
            .partial_cmp(&ring_area_ids(map, a).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let outer = unique_ring(
        loops[0]
            .iter()
            .filter_map(|&i| map.walls.get(i).map(world_xy))
            .collect(),
    );
    if outer.len() < 3 {
        return;
    }
    // Extra wall cycles are often concave (hatch + a stair jog). Subtract
    // each interior child sector's own ring instead — those are simple.
    let holes = interior_holes(map, si, &outer);
    // Paper sectors (floorz == ceilingz) are triggers / thin doors. Emitting
    // both planes puts two coplanar faces at the same height and they z-fight
    // (darker vs lighter). Skip both; walls of the sector still draw.
    let paper = (sec.floorz - sec.ceilingz).abs() < 64;
    // A lift platform is the one paper sector that must still draw: BUILD
    // authors a thin elevator with its floor on its ceiling, and skipping it
    // for the z-fight rule would delete the thing the node is FOR.
    let keep_floor = !paper || floor_target != SinkTarget::Level;
    if keep_floor && sec.floorstat & 1 == 0 {
        sink.into_target(floor_target, |sink| {
            emit_plane(
                sink,
                &outer,
                &holes,
                map,
                si,
                false,
                tile_ref(art, fallback, sec.floorpicnum),
            );
        });
    }
    if !paper && sec.ceilingstat & 1 == 0 {
        emit_plane(
            sink,
            &outer,
            &holes,
            map,
            si,
            true,
            tile_ref(art, fallback, sec.ceilingpicnum),
        );
    }
}

fn emit_plane(
    sink: &mut MeshSink,
    outer: &[[f32; 2]],
    holes: &[Vec<[f32; 2]>],
    map: &BuildMap,
    si: usize,
    ceil: bool,
    tile: &TileRgba,
) {
    let sec = &map.sectors[si];
    let pic = if ceil {
        sec.ceilingpicnum
    } else {
        sec.floorpicnum
    };
    let shade = if ceil {
        sec.ceilingshade
    } else {
        sec.floorshade
    };
    let tw = tile.w.max(1) as f32;
    let th = tile.h.max(1) as f32;
    let mut pts = outer.to_vec();
    let area = signed_area(&pts);
    if (area > 0.0 && ceil) || (area < 0.0 && !ceil) {
        pts.reverse();
    }
    let tris = ear_clip(&pts);
    let mesh = sink.bucket((pic, shade));
    for [ia, ib, ic] in tris {
        let a = pts[ia];
        let b = pts[ib];
        let c = pts[ic];
        for [pa, pb, pc] in subtract_holes([a, b, c], holes) {
            emit_tri(
                mesh,
                pa,
                pb,
                pc,
                |x, y| slope_z(sec, map, x, y, ceil),
                |x, y| plane_uv(sec, map, x, y, ceil, tw, th),
            );
        }
    }
}

fn is_parallax_floor(sec: &Sector) -> bool {
    sec.floorstat & 1 != 0
}

fn is_parallax_ceil(sec: &Sector) -> bool {
    sec.ceilingstat & 1 != 0
}

fn is_sky_sector(sec: &Sector) -> bool {
    is_parallax_floor(sec) && is_parallax_ceil(sec)
}

/// BUILD bit 1 on floor/ceiling is parallax: that plane is sky, not a
/// surface. Every map uses the same flag. The sky tile (city, clouds, …)
/// is a horizon cylinder around the start, not a box around the whole map.
struct SkyHull {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    png: Vec<u8>,
    /// The parallax tile this strip was blitted from — the `texture` a sky
    /// node reports.
    pic: i16,
}

/// BUILD default lognumtiles=3: eight panels around 360°, each one tile
/// wide. LA city (tile 89) swaps in neighbouring pics per slot.
const SKY_PANELS: usize = 8;
const SKY_SIDES: usize = 24;
/// LA city panel offsets from the ceiling pic (89 → 90,91,90,92,93,89,91,92).
const LA_SKY_PANELS: [i16; SKY_PANELS] = [1, 2, 1, 3, 4, 0, 2, 3];
/// Map units (512 = 1 world). Courtyard on E1L1 reaches ~19 world from spawn.
const SKY_PAD: f32 = 1536.0;
const SKY_MIN_R: f32 = 4096.0;
const SKY_MAX_R: f32 = 20480.0;
/// Only same-storey outdoor rooms count for radius (drops the street pit).
const SKY_STOREY: f32 = 6.0;

fn sky_pic(map: &BuildMap) -> Option<i16> {
    for sec in &map.sectors {
        if is_parallax_ceil(sec) && sec.ceilingpicnum >= 0 {
            return Some(sec.ceilingpicnum);
        }
    }
    for sec in &map.sectors {
        if is_parallax_floor(sec) && sec.floorpicnum >= 0 {
            return Some(sec.floorpicnum);
        }
    }
    None
}

fn start_floor_y(map: &BuildMap) -> f32 {
    let y_eye = -map.start[2] as f32 * SCALE_Z;
    let si = map.start_sec;
    if si >= 0 {
        if let Some(sec) = map.sectors.get(si as usize) {
            return slope_z(sec, map, map.start[0] as f32, map.start[1] as f32, false);
        }
    }
    y_eye - 1.0
}

fn sky_panel_offsets(pic: i16) -> [i16; SKY_PANELS] {
    if pic == 89 {
        LA_SKY_PANELS
    } else {
        [0; SKY_PANELS]
    }
}

fn blit_tile(dst: &mut [u8], dw: u32, dh: u32, dx: u32, tile: &TileRgba) {
    let copy_w = tile.w.min(dw.saturating_sub(dx));
    let copy_h = tile.h.min(dh);
    for y in 0..copy_h {
        let src = (y * tile.w) as usize * 4;
        let dst_i = (y * dw + dx) as usize * 4;
        let n = copy_w as usize * 4;
        dst[dst_i..dst_i + n].copy_from_slice(&tile.rgba[src..src + n]);
    }
}

fn sky_atlas_png(art: &ArtBank, fallback: &TileRgba, pic: i16) -> (Vec<u8>, u32, u32) {
    let base = tile_ref(art, fallback, pic);
    let tw = base.w.max(1);
    let th = base.h.max(1);
    let aw = tw * SKY_PANELS as u32;
    let mut rgba = vec![0u8; (aw * th * 4) as usize];
    for (i, off) in sky_panel_offsets(pic).iter().enumerate() {
        let want = pic.saturating_add(*off).max(0) as u16;
        let tile = art
            .tiles
            .get(&want)
            .or(art.tiles.get(&(pic.max(0) as u16)))
            .unwrap_or(fallback);
        blit_tile(&mut rgba, aw, th, i as u32 * tw, tile);
    }
    key_build_rgba(&mut rgba);
    let png = encode_png_rgba(&rgba, aw, th).unwrap_or_else(|_| vec![0x90; 16]);
    (png, aw, th)
}

fn emit_parallax_sky(map: &BuildMap, art: &ArtBank, fallback: &TileRgba) -> Option<SkyHull> {
    let pic = sky_pic(map)?;
    let (png, tw, th) = sky_atlas_png(art, fallback, pic);
    let sx = map.start[0] as f32;
    let sy = map.start[1] as f32;
    let y_floor = start_floor_y(map);
    let y_eye = -map.start[2] as f32 * SCALE_Z;
    let mut radius = SKY_MIN_R;
    for (si, sec) in map.sectors.iter().enumerate() {
        let same_storey = if si as i16 == map.start_sec {
            true
        } else if is_parallax_ceil(sec) && !is_sky_sector(sec) {
            let fy = slope_z(sec, map, sx, sy, false);
            (fy - y_floor).abs() <= SKY_STOREY
        } else {
            false
        };
        if !same_storey {
            continue;
        }
        let start = sec.wallptr as usize;
        let end = (start + sec.wallnum as usize).min(map.walls.len());
        for wi in start..end {
            let w = &map.walls[wi];
            let d = (w.x as f32 - sx).hypot(w.y as f32 - sy);
            radius = radius.max(d + SKY_PAD);
        }
    }
    radius = radius.min(SKY_MAX_R).max(SKY_MIN_R);
    // Eight panels around the circle; square texels so the city is not
    // smeared. Mid-tile sits on the look horizon (BUILD yoffset 0).
    let circ = std::f32::consts::TAU * radius * SCALE;
    let texel = circ / (tw.max(1) as f32);
    let height = texel * th.max(1) as f32;
    let y_mid = y_eye.max(y_floor + 1.0);
    let y_lo = y_mid - height * 0.5;
    let y_hi = y_mid + height * 0.5;
    let _ = y_floor;
    let mut mesh = MeshBucket::default();
    let n = SKY_SIDES as f32;
    for i in 0..SKY_SIDES {
        let a0 = (i as f32) / n * std::f32::consts::TAU;
        let a1 = ((i + 1) as f32) / n * std::f32::consts::TAU;
        let p0 = [sx + radius * a0.cos(), sy + radius * a0.sin()];
        let p1 = [sx + radius * a1.cos(), sy + radius * a1.sin()];
        let u0 = (i as f32) / n;
        let u1 = ((i + 1) as f32) / n;
        // Both windings: the walk viewer culls back faces.
        emit_quad(
            &mut mesh,
            p0,
            p1,
            y_lo,
            y_lo,
            y_hi,
            y_hi,
            [u0, 1.0],
            [u1, 1.0],
            [u1, 0.0],
            [u0, 0.0],
        );
        emit_quad(
            &mut mesh,
            p1,
            p0,
            y_lo,
            y_lo,
            y_hi,
            y_hi,
            [u1, 1.0],
            [u0, 1.0],
            [u0, 0.0],
            [u1, 0.0],
        );
    }
    if mesh.indices.len() < 3 {
        return None;
    }
    // Up normals: same lighting on every face so the night city is not a
    // gray sun-lit wall. World hemi * overhead sun then ≈ authored color.
    let normals = vec![[0.0, 1.0, 0.0]; mesh.positions.len()];
    Some(SkyHull {
        positions: mesh.positions,
        uvs: mesh.uvs,
        normals,
        indices: mesh.indices,
        png,
        pic,
    })
}

fn emit_sector_walls(
    sink: &mut MeshSink,
    map: &BuildMap,
    si: usize,
    art: &ArtBank,
    fallback: &TileRgba,
    movers: &[BuildMover],
    mover_of_sector: &BTreeMap<usize, usize>,
) {
    let sec = &map.sectors[si];
    // A fully parallax sector is the outdoor sky volume. Its walls are not
    // buildings — they are the pit/cliff that would hide the city sky.
    if is_sky_sector(sec) {
        return;
    }
    let start = sec.wallptr as usize;
    let end = (start + sec.wallnum as usize).min(map.walls.len());
    for wi in start..end {
        let w = &map.walls[wi];
        let p2 = w.point2 as usize;
        if p2 >= map.walls.len() {
            continue;
        }
        let a = world_xy(w);
        let b = world_xy(&map.walls[p2]);
        let next = w.nextsector;
        if next < 0 {
            let zf0 = slope_z(sec, map, a[0], a[1], false);
            let zf1 = slope_z(sec, map, b[0], b[1], false);
            let zc0 = slope_z(sec, map, a[0], a[1], true);
            let zc1 = slope_z(sec, map, b[0], b[1], true);
            emit_wall_face(
                sink,
                w,
                a,
                b,
                zf0,
                zf1,
                zc0,
                zc1,
                w.picnum,
                tile_ref(art, fallback, w.picnum),
            );
            continue;
        }
        let nsec = match map.sectors.get(next as usize) {
            Some(s) => s,
            None => continue,
        };
        let nf0 = slope_z(nsec, map, a[0], a[1], false);
        let nf1 = slope_z(nsec, map, b[0], b[1], false);
        let nc0 = slope_z(nsec, map, a[0], a[1], true);
        let nc1 = slope_z(nsec, map, b[0], b[1], true);
        let zf0 = slope_z(sec, map, a[0], a[1], false);
        let zf1 = slope_z(sec, map, b[0], b[1], false);
        let zc0 = slope_z(sec, map, a[0], a[1], true);
        let zc1 = slope_z(sec, map, b[0], b[1], true);
        // The DOOR LEAF: the band this sector sees across the doorway when
        // the door beyond is shut. The level mesh is baked with that door
        // open, so nothing here draws it — it belongs to the `door_N` node,
        // authored closed, and the node's rest translation lifts it to the
        // lintel. Only a sector that is not itself a door emits it, or two
        // adjacent doors would each carry the other's leaf.
        if !mover_of_sector.contains_key(&si) {
            if let Some(&mi) = mover_of_sector.get(&(next as usize)) {
                let m = movers[mi];
                if m.ceiling {
                    let shut = -m.closed_z as f32 * SCALE_Z;
                    if zc0 > shut + 0.01 || zc1 > shut + 0.01 {
                        sink.into_target(SinkTarget::Mover(mi), |sink| {
                            emit_wall_face(
                                sink,
                                w,
                                a,
                                b,
                                shut.min(zc0),
                                shut.min(zc1),
                                zc0,
                                zc1,
                                w.picnum,
                                tile_ref(art, fallback, w.picnum),
                            );
                        });
                    }
                }
            }
        }
        let lower_pic = if w.cstat & 2 != 0 {
            map.walls
                .get(w.nextwall as usize)
                .map(|nw| nw.picnum)
                .unwrap_or(w.picnum)
        } else {
            w.picnum
        };
        if !is_parallax_floor(nsec) && (zf0 + 0.01 < nf0 || zf1 + 0.01 < nf1) {
            emit_wall_face(
                sink,
                w,
                a,
                b,
                zf0,
                zf1,
                nf0.max(zf0),
                nf1.max(zf1),
                lower_pic,
                tile_ref(art, fallback, lower_pic),
            );
        }
        if !is_parallax_ceil(nsec) && (zc0 > nc0 + 0.01 || zc1 > nc1 + 0.01) {
            emit_wall_face(
                sink,
                w,
                a,
                b,
                nc0.min(zc0),
                nc1.min(zc1),
                zc0,
                zc1,
                w.picnum,
                tile_ref(art, fallback, w.picnum),
            );
        }
        if w.cstat & 16 != 0 && w.overpicnum > 0 && (w.nextwall < 0 || wi as i16 <= w.nextwall) {
            // One quad per red-wall pair. The mesh is already double-sided;
            // emitting the partner copies the same plane and z-fights
            // (darker vs lighter).
            let bot0 = zf0.max(nf0);
            let bot1 = zf1.max(nf1);
            let top0 = zc0.min(nc0);
            let top1 = zc1.min(nc1);
            if top0 > bot0 + 0.02 || top1 > bot1 + 0.02 {
                emit_wall_face(
                    sink,
                    w,
                    a,
                    b,
                    bot0,
                    bot1,
                    top0,
                    top1,
                    w.overpicnum,
                    tile_ref(art, fallback, w.overpicnum),
                );
            }
        }
    }
}

fn emit_wall_face(
    sink: &mut MeshSink,
    w: &Wall,
    a: [f32; 2],
    b: [f32; 2],
    z_bot0: f32,
    z_bot1: f32,
    z_top0: f32,
    z_top1: f32,
    pic: i16,
    tile: &TileRgba,
) {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    if len < 0.5 {
        return;
    }
    if (z_top0 - z_bot0).abs() < 0.004 && (z_top1 - z_bot1).abs() < 0.004 {
        return;
    }
    let tw = tile.w.max(1) as f32;
    let th = tile.h.max(1) as f32;
    let th_pow2 = tile.h.max(1).next_power_of_two() as f32;
    let xrep = w.xrepeat.max(1) as f32;
    let yrep = w.yrepeat.max(1) as f32;
    // U runs 0..xrepeat*8 texels along the whole wall (not × length).
    let mut u0 = w.xpanning as f32 / tw;
    let mut u1 = u0 + (xrep * 8.0) / tw;
    if w.cstat & 8 != 0 {
        std::mem::swap(&mut u0, &mut u1);
    }
    let zb0 = world_y_to_build(z_bot0);
    let zb1 = world_y_to_build(z_bot1);
    let zt0 = world_y_to_build(z_top0);
    let zt1 = world_y_to_build(z_top1);
    let align_bottom = w.cstat & 4 != 0;
    let (ref0, ref1) = if align_bottom {
        (zb0, zb1)
    } else {
        (zt0, zt1)
    };
    let pan_v = (w.ypanning as f32) * th_pow2 / 256.0 / th;
    let mut v_top0 = wall_v(zt0, ref0, yrep, th) + pan_v;
    let mut v_top1 = wall_v(zt1, ref1, yrep, th) + pan_v;
    let mut v_bot0 = wall_v(zb0, ref0, yrep, th) + pan_v;
    let mut v_bot1 = wall_v(zb1, ref1, yrep, th) + pan_v;
    if w.cstat & 256 != 0 {
        v_top0 = -v_top0;
        v_top1 = -v_top1;
        v_bot0 = -v_bot0;
        v_bot1 = -v_bot1;
    }
    emit_quad(
        sink.bucket((pic, w.shade)),
        a,
        b,
        z_bot0,
        z_bot1,
        z_top0,
        z_top1,
        [u0, v_bot0],
        [u1, v_bot1],
        [u1, v_top1],
        [u0, v_top0],
    );
}

fn wall_v(z_build: f32, z_ref: f32, yrepeat: f32, th: f32) -> f32 {
    (z_build - z_ref) * yrepeat / WALL_V_DIV / th.max(1.0)
}

fn world_y_to_build(y: f32) -> f32 {
    -y / SCALE_Z
}

fn plane_uv(sec: &Sector, map: &BuildMap, x: f32, y: f32, ceil: bool, tw: f32, th: f32) -> [f32; 2] {
    let mut stat = if ceil { sec.ceilingstat } else { sec.floorstat };
    let pan_x = if ceil {
        sec.ceilingxpanning
    } else {
        sec.floorxpanning
    } as f32;
    let pan_y = if ceil {
        sec.ceilingypanning
    } else {
        sec.floorypanning
    } as f32;
    let step = if stat & 8 != 0 { 8.0 } else { FLOOR_UNITS_PER_TEXEL };
    let tw_p = (tw.max(1.0) as u32).next_power_of_two() as f32;
    let th_p = (th.max(1.0) as u32).next_power_of_two() as f32;
    let (ox, oy, ux, uy, vx, vy) = if stat & 64 != 0 {
        let w0 = sec.wallptr as usize;
        let a = map.walls.get(w0).map(world_xy).unwrap_or([x, y]);
        let b = map
            .walls
            .get(w0)
            .and_then(|w| map.walls.get(w.point2 as usize))
            .map(world_xy)
            .unwrap_or([a[0] + 1.0, a[1]]);
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        (a[0], a[1], dx / len, dy / len, -dy / len, dx / len)
    } else {
        (0.0, 0.0, 1.0, 0.0, 0.0, 1.0)
    };
    let rx = x - ox;
    let ry = y - oy;
    let mut tu = rx * ux + ry * uy;
    let mut tv = rx * vx + ry * vy;
    // Relative floors flip the axis that swap-xy does not, then the usual
    // 16/32 bits apply. World V is also −Y (south).
    if stat & 64 != 0 {
        if stat & 4 == 0 {
            stat ^= 32;
        } else {
            stat ^= 16;
        }
    }
    // World mapping: u*step = X, v*step = Y (texels). The OpenGL path's
    // −Y is a camera-matrix convention, not the baked UV.
    tu /= step * tw.max(1.0);
    tv /= step * th.max(1.0);
    if stat & 4 != 0 {
        std::mem::swap(&mut tu, &mut tv);
    }
    if stat & 16 != 0 {
        tu = -tu;
    }
    if stat & 32 != 0 {
        tv = -tv;
    }
    if stat & 2 != 0 {
        let heinum = if ceil {
            sec.ceilingheinum
        } else {
            sec.floorheinum
        } as f32;
        let stretch = (1.0 + (heinum / 4096.0) * (heinum / 4096.0)).sqrt();
        tv *= stretch;
    }
    [
        tu + pan_x / 256.0 * tw_p / tw.max(1.0),
        tv + pan_y / 256.0 * th_p / th.max(1.0),
    ]
}

fn map_to_place(
    map: &BuildMap,
    art: &ArtBank,
    source_id: &str,
    world_key: &str,
    spawn: Option<[f32; 5]>,
) -> crate::world_place::WorldPlace {
    let fallback = TileRgba {
        w: 16,
        h: 16,
        rgba: vec![0x90; 16 * 16 * 4],
        xoff: 0,
        yoff: 0,
    };
    let mut places = Vec::new();
    for (i, s) in map.sprites.iter().enumerate() {
        if (s.cstat as u16) & 0x8000 != 0 || s.picnum < 0 {
            continue;
        }
        // Tiles 1..=10 are BUILD control sprites (effectors, activators,
        // sound markers). The game never draws them.
        if (1..=CONTROL_TILE_MAX).contains(&s.picnum) {
            continue;
        }
        if s.xrepeat == 0 || s.yrepeat == 0 {
            continue;
        }
        let align = s.cstat & 48;
        if align != 0 {
            continue;
        }
        let tile = tile_ref(art, &fallback, s.picnum);
        let tw = tile.w.max(1) as f32;
        let th = tile.h.max(1) as f32;
        let width = tw * s.xrepeat as f32 * 0.25 * SCALE;
        let height = th * s.yrepeat as f32 * 4.0 * SCALE_Z;
        if width < 0.04 || height < 0.04 {
            continue;
        }
        let y = if s.cstat & 64 != 0 {
            -s.z as f32 * SCALE_Z
        } else {
            -s.z as f32 * SCALE_Z + height * 0.5
        };
        let yaw = s.ang as f32 * std::f32::consts::PI / 1024.0;
        places.push(crate::world_place::Place {
            id: format!("spr-{i}"),
            kind: "billboard".into(),
            asset: format!("billboards/{source_id}/tile-{}", s.picnum),
            pos: [s.x as f32 * SCALE, y, -s.y as f32 * SCALE],
            yaw,
            class: s.picnum.to_string(),
            width,
            height,
            align: "face".into(),
        });
    }
    crate::world_place::WorldPlace {
        source: source_id.into(),
        world: world_key.into(),
        spawn: spawn.map(|s| ([s[0], s[1], s[2]], s[3], s[4])),
        places,
    }
}

/// Tiles 1..=10 are BUILD control sprites (sector effectors, activators,
/// touchplates, sound markers, locators, cyclers, respawn/speed markers).
/// The game never renders them; they must not become cards or placements.
const CONTROL_TILE_MAX: i16 = 10;

fn emit_sprites(
    sink: &mut MeshSink,
    map: &BuildMap,
    art: &ArtBank,
    fallback: &TileRgba,
) {
    for s in &map.sprites {
        if (s.cstat as u16) & 0x8000 != 0 || s.picnum < 0 {
            continue;
        }
        if s.xrepeat == 0 || s.yrepeat == 0 {
            continue;
        }
        let tile = tile_ref(art, fallback, s.picnum);
        let tw = tile.w.max(1) as f32;
        let th = tile.h.max(1) as f32;
        // width = tile * xrepeat / 4 map units; height = tile * yrepeat * 4 in Z.
        let width = tw * s.xrepeat as f32 * 0.25;
        let height_z = th * s.yrepeat as f32 * 4.0;
        if width < 1.0 || height_z < 1.0 {
            continue;
        }
        let align = s.cstat & 48;
        if align == 0 {
            // Face sprites are catalog billboards, placed at runtime.
            continue;
        }
        if align == 32 {
            let depth = th * s.yrepeat as f32 * 0.25;
            emit_floor_sprite(sink, s, width, depth);
        } else {
            emit_upright_sprite(sink, s, width, height_z);
        }
    }
}

fn emit_upright_sprite(
    sink: &mut MeshSink,
    s: &Sprite,
    width: f32,
    height_z: f32,
) {
    let rad = s.ang as f32 * std::f32::consts::PI / 1024.0;
    // Width runs along (sin, −cos): the sprite faces `ang`.
    let ax = rad.sin();
    let ay = -rad.cos();
    let hw = width * 0.5;
    let a = [s.x as f32 - ax * hw, s.y as f32 - ay * hw];
    let b = [s.x as f32 + ax * hw, s.y as f32 + ay * hw];
    let (z_bot, z_top) = if s.cstat & 64 != 0 {
        (s.z as f32 + height_z * 0.5, s.z as f32 - height_z * 0.5)
    } else {
        (s.z as f32, s.z as f32 - height_z)
    };
    let zb = -z_bot * SCALE_Z;
    let zt = -z_top * SCALE_Z;
    let mut u0 = 0.0;
    let mut u1 = 1.0;
    let mut v0 = 1.0;
    let mut v1 = 0.0;
    if s.cstat & 4 != 0 {
        std::mem::swap(&mut u0, &mut u1);
    }
    if s.cstat & 8 != 0 {
        std::mem::swap(&mut v0, &mut v1);
    }
    emit_quad(
        sink.bucket((s.picnum, s.shade)),
        a,
        b,
        zb,
        zb,
        zt,
        zt,
        [u0, v0],
        [u1, v0],
        [u1, v1],
        [u0, v1],
    );
}

fn emit_floor_sprite(
    sink: &mut MeshSink,
    s: &Sprite,
    width: f32,
    depth: f32,
) {
    let rad = s.ang as f32 * std::f32::consts::PI / 1024.0;
    let c = rad.cos();
    let sn = rad.sin();
    let hx = width * 0.5;
    let hy = depth * 0.5;
    let cx = s.x as f32;
    let cy = s.y as f32;
    let y = -s.z as f32 * SCALE_Z;
    let corners = [
        [-hx, -hy],
        [hx, -hy],
        [hx, hy],
        [-hx, hy],
    ];
    let mut pts = [[0.0f32; 3]; 4];
    for (i, [lx, ly]) in corners.iter().enumerate() {
        let wx = cx + lx * c - ly * sn;
        let wy = cy + lx * sn + ly * c;
        pts[i] = [wx * SCALE, y, -wy * SCALE];
    }
    let mut uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    if s.cstat & 4 != 0 {
        for uv in &mut uvs {
            uv[0] = 1.0 - uv[0];
        }
    }
    if s.cstat & 8 != 0 {
        for uv in &mut uvs {
            uv[1] = 1.0 - uv[1];
        }
    }
    let mesh = sink.bucket((s.picnum, s.shade));
    let i = mesh.positions.len() as u32;
    mesh.positions.extend_from_slice(&pts);
    mesh.uvs.extend_from_slice(&uvs);
    mesh.indices
        .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
}

fn emit_quad(
    mesh: &mut MeshBucket,
    a: [f32; 2],
    b: [f32; 2],
    zf0: f32,
    zf1: f32,
    zc0: f32,
    zc1: f32,
    uv00: [f32; 2],
    uv10: [f32; 2],
    uv11: [f32; 2],
    uv01: [f32; 2],
) {
    let p0 = [a[0] * SCALE, zf0, -a[1] * SCALE];
    let p1 = [b[0] * SCALE, zf1, -b[1] * SCALE];
    let p2 = [b[0] * SCALE, zc1, -b[1] * SCALE];
    let p3 = [a[0] * SCALE, zc0, -a[1] * SCALE];
    let i = mesh.positions.len() as u32;
    mesh.positions.extend_from_slice(&[p0, p1, p2, p3]);
    mesh.uvs.extend_from_slice(&[uv00, uv10, uv11, uv01]);
    mesh.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
}

fn emit_tri(
    mesh: &mut MeshBucket,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    z_at: impl Fn(f32, f32) -> f32,
    uv_at: impl Fn(f32, f32) -> [f32; 2],
) {
    let i = mesh.positions.len() as u32;
    for p in [a, b, c] {
        mesh.positions
            .push([p[0] * SCALE, z_at(p[0], p[1]), -p[1] * SCALE]);
        mesh.uvs.push(uv_at(p[0], p[1]));
    }
    mesh.indices.extend_from_slice(&[i, i + 1, i + 2]);
}

fn subtract_holes(tri: [[f32; 2]; 3], holes: &[Vec<[f32; 2]>]) -> Vec<[[f32; 2]; 3]> {
    let mut polys = vec![tri.to_vec()];
    for hole in holes {
        if hole.len() < 3 {
            continue;
        }
        let mut next = Vec::new();
        for poly in polys {
            let mut pieces = vec![poly];
            for i in 0..hole.len() {
                let a = hole[i];
                let b = hole[(i + 1) % hole.len()];
                let mut split = Vec::new();
                for p in pieces {
                    let (l, r) = split_poly_by_line(&p, a, b);
                    if l.len() >= 3 {
                        split.push(l);
                    }
                    if r.len() >= 3 {
                        split.push(r);
                    }
                }
                pieces = split;
            }
            for p in pieces {
                let c = poly_centroid(&p);
                if !point_in_poly(c, hole) {
                    next.push(p);
                }
            }
        }
        polys = next;
    }
    let mut out = Vec::new();
    for p in polys {
        for [ia, ib, ic] in ear_clip(&p) {
            out.push([p[ia], p[ib], p[ic]]);
        }
    }
    out
}

fn poly_centroid(pts: &[[f32; 2]]) -> [f32; 2] {
    let n = pts.len().max(1) as f32;
    let mut x = 0.0;
    let mut y = 0.0;
    for p in pts {
        x += p[0];
        y += p[1];
    }
    [x / n, y / n]
}

fn split_poly_by_line(poly: &[[f32; 2]], a: [f32; 2], b: [f32; 2]) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let side = |p: [f32; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let mut left = Vec::new();
    let mut right = Vec::new();
    if poly.is_empty() {
        return (left, right);
    }
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        let sp = side(p);
        let sq = side(q);
        if sp >= -1e-3 {
            left.push(p);
        }
        if sp <= 1e-3 {
            right.push(p);
        }
        if (sp > 1e-3 && sq < -1e-3) || (sp < -1e-3 && sq > 1e-3) {
            let t = sp / (sp - sq);
            let hit = [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
            left.push(hit);
            right.push(hit);
        }
    }
    (left, right)
}

fn interior_holes(map: &BuildMap, si: usize, outer: &[[f32; 2]]) -> Vec<Vec<[f32; 2]>> {
    let sec = &map.sectors[si];
    let mut loops = sector_loops(map, sec);
    if loops.len() < 2 {
        return Vec::new();
    }
    loops.sort_by(|a, b| {
        ring_area_ids(map, b)
            .abs()
            .partial_cmp(&ring_area_ids(map, a).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Only inner wall cycles are holes. Outer-loop neighbors are adjacent
    // sectors and must not be subtracted (their centroid can still sit
    // inside the parent's parallelogram).
    let mut kids = BTreeSet::new();
    for ring in loops.iter().skip(1) {
        for &wi in ring {
            let ns = map.walls.get(wi).map(|w| w.nextsector).unwrap_or(-1);
            if ns >= 0 {
                kids.insert(ns as usize);
            }
        }
    }
    let outer_area = ring_area_ids_pts(outer).abs();
    let mut holes = Vec::new();
    for k in kids {
        if k == si || k >= map.sectors.len() {
            continue;
        }
        let mut loops = sector_loops(map, &map.sectors[k]);
        if loops.is_empty() {
            continue;
        }
        loops.sort_by(|a, b| {
            ring_area_ids(map, b)
                .abs()
                .partial_cmp(&ring_area_ids(map, a).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let ring = unique_ring(
            loops[0]
                .iter()
                .filter_map(|&i| map.walls.get(i).map(world_xy))
                .collect(),
        );
        if ring.len() < 3 {
            continue;
        }
        let c = poly_centroid(&ring);
        let area = ring_area_ids_pts(&ring).abs();
        if area + 1.0 < outer_area && point_in_poly(c, outer) {
            holes.push(ring);
        }
    }
    holes
}

fn ring_area_ids_pts(pts: &[[f32; 2]]) -> f32 {
    signed_area(pts)
}

fn unique_ring(pts: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for p in pts {
        if out
            .last()
            .is_some_and(|q| (p[0] - q[0]).hypot(p[1] - q[1]) < 0.5)
        {
            continue;
        }
        out.push(p);
    }
    if out.len() >= 2 {
        let first = out[0];
        let last = *out.last().unwrap();
        if (first[0] - last[0]).hypot(first[1] - last[1]) < 0.5 {
            out.pop();
        }
    }
    out
}

/// Corner cross product in f64: BUILD coords reach 2^16, so the cross
/// reaches 2^34 and overflows f32 mantissa precision.
fn corner_cross(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f64 {
    (b[0] as f64 - a[0] as f64) * (c[1] as f64 - a[1] as f64)
        - (b[1] as f64 - a[1] as f64) * (c[0] as f64 - a[0] as f64)
}

/// Below this |cross| a triangle is a sub-texel sliver (area 1024 map
/// units² ≈ 4mm × 1m world) and may be dropped without a visible gap.
const DEGEN_CROSS: f64 = 2048.0;

fn ear_clip(pts: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if signed_area(pts) < 0.0 {
        idx.reverse();
    }
    // BUILD rings can retrace an edge (out-and-back zigzag); ear clipping
    // would cover that stretch twice. Strip zero-area backtrack spikes
    // (prev == next) and consecutive duplicates first.
    let near = |p: [f32; 2], q: [f32; 2]| (p[0] - q[0]).abs() < 0.5 && (p[1] - q[1]).abs() < 0.5;
    let mut changed = true;
    while changed && idx.len() > 3 {
        changed = false;
        let m = idx.len();
        for i in 0..m {
            if near(pts[idx[i]], pts[idx[(i + 1) % m]]) {
                idx.remove(i);
                changed = true;
                break;
            }
            if near(pts[idx[(i + m - 1) % m]], pts[idx[(i + 1) % m]]) {
                let a = i;
                let b = (i + 1) % m;
                idx.remove(a.max(b));
                idx.remove(a.min(b));
                changed = true;
                break;
            }
        }
    }
    let cross_at = |idx: &[usize], i: usize| -> f64 {
        let m = idx.len();
        corner_cross(pts[idx[(i + m - 1) % m]], pts[idx[i]], pts[idx[(i + 1) % m]])
    };
    let mut out = Vec::new();
    let mut guard = 0usize;
    let limit = 2 * n * n + 16;
    while idx.len() >= 3 && guard < limit {
        guard += 1;
        if idx.len() == 3 {
            if cross_at(&idx, 1) > 1e-2 {
                out.push([idx[0], idx[1], idx[2]]);
            }
            break;
        }
        let m = idx.len();
        let mut cut = None;
        for i in 0..m {
            let i0 = idx[(i + m - 1) % m];
            let i1 = idx[i];
            let i2 = idx[(i + 1) % m];
            if is_ear(pts, &idx, i0, i1, i2) {
                cut = Some(i);
                break;
            }
        }
        if let Some(i) = cut {
            let i0 = idx[(i + m - 1) % m];
            let i1 = idx[i];
            let i2 = idx[(i + 1) % m];
            if cross_at(&idx, i) > 1e-2 {
                out.push([i0, i1, i2]);
            }
            idx.remove(i);
            continue;
        }
        // No clean ear. BUILD keyhole rings (a notch reached through a
        // zero-width channel) leave spikes and collinear runs that block
        // every ear; drop the flattest corner and continue.
        let mut best = 0usize;
        let mut best_c = f64::MAX;
        for i in 0..m {
            let c = cross_at(&idx, i).abs();
            if c < best_c {
                best_c = c;
                best = i;
            }
        }
        if best_c > DEGEN_CROSS {
            break;
        }
        idx.remove(best);
    }
    out
}

/// Strict segment intersection: crossings in the segments' interiors only;
/// shared or coincident endpoints do not count.
fn seg_properly_cross(p1: [f32; 2], p2: [f32; 2], q1: [f32; 2], q2: [f32; 2]) -> bool {
    let d1 = corner_cross(p1, p2, q1);
    let d2 = corner_cross(p1, p2, q2);
    let d3 = corner_cross(q1, q2, p1);
    let d4 = corner_cross(q1, q2, p2);
    let e = 1e-9;
    ((d1 > e && d2 < -e) || (d1 < -e && d2 > e))
        && ((d3 > e && d4 < -e) || (d3 < -e && d4 > e))
}

fn is_ear(pts: &[[f32; 2]], idx: &[usize], i0: usize, i1: usize, i2: usize) -> bool {
    let a = pts[i0];
    let b = pts[i1];
    let c = pts[i2];
    if corner_cross(a, b, c) <= 1e-2 {
        return false;
    }
    let near = |p: [f32; 2], q: [f32; 2]| (p[0] - q[0]).abs() < 1e-3 && (p[1] - q[1]).abs() < 1e-3;
    for &k in idx {
        if k == i0 || k == i1 || k == i2 {
            continue;
        }
        let p = pts[k];
        // A keyhole ring passes through the same point twice; the twin of
        // an ear corner sits exactly on it and must not block the ear.
        if near(p, a) || near(p, b) || near(p, c) {
            continue;
        }
        if point_in_tri(p, a, b, c) {
            return false;
        }
    }
    // No side of the ear may cross a remaining ring edge; a pinched ring
    // can otherwise thread an ear through the channel mouth.
    let m = idx.len();
    let sides = [(i0, i1), (i1, i2), (i2, i0)];
    for j in 0..m {
        let u = idx[j];
        let v = idx[(j + 1) % m];
        for (s0, s1) in sides {
            if u == s0 || u == s1 || v == s0 || v == s1 {
                continue;
            }
            if seg_properly_cross(pts[s0], pts[s1], pts[u], pts[v]) {
                return false;
            }
        }
    }
    // Twins skipped above can guard a pinch whose far side is outside the
    // sector (a notch bitten out of the boundary). Even-odd parity on the
    // full ring settles it: doubled channel edges cancel, so genuine ear
    // interior tests inside and a notch tests outside. Probe the centroid
    // plus a point just inside each pinched corner.
    let cen = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
    if !point_in_poly(cen, pts) {
        return false;
    }
    for corner in [a, b, c] {
        let twin = idx
            .iter()
            .any(|&k| k != i0 && k != i1 && k != i2 && near(pts[k], corner));
        if !twin {
            continue;
        }
        let dx = cen[0] - corner[0];
        let dy = cen[1] - corner[1];
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let probe = [corner[0] + dx / len * 0.5, corner[1] + dy / len * 0.5];
        if !point_in_poly(probe, pts) {
            return false;
        }
    }
    true
}

fn point_in_tri(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let s1 = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let s2 = (c[0] - b[0]) * (p[1] - b[1]) - (c[1] - b[1]) * (p[0] - b[0]);
    let s3 = (a[0] - c[0]) * (p[1] - c[1]) - (a[1] - c[1]) * (p[0] - c[0]);
    (s1 >= -1e-2 && s2 >= -1e-2 && s3 >= -1e-2) || (s1 <= 1e-2 && s2 <= 1e-2 && s3 <= 1e-2)
}

fn point_in_poly(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let yi = poly[i][1];
        let yj = poly[j][1];
        if (yi > p[1]) != (yj > p[1]) {
            let x = (poly[j][0] - poly[i][0]) * (p[1] - yi) / (yj - yi + 1e-12) + poly[i][0];
            if p[0] < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}



#[derive(Clone, Copy)]
struct AtlasSlot {
    uv: [f32; 4],
    w: u32,
    h: u32,
}

fn slot_uv(slot: AtlasSlot, local_u: f32, local_v: f32) -> [f32; 2] {
    let u = local_u.clamp(0.0, 1.0);
    let v = local_v.clamp(0.0, 1.0);
    let du = slot.uv[2] - slot.uv[0];
    let dv = slot.uv[3] - slot.uv[1];
    [slot.uv[0] + du * u, slot.uv[1] + dv * v]
}

fn lookup(uv: &BTreeMap<String, AtlasSlot>, name: &str) -> AtlasSlot {
    uv.get(name)
        .or_else(|| uv.get("_default"))
        .copied()
        .unwrap_or(AtlasSlot {
            uv: [0.0, 0.0, 0.02, 0.02],
            w: 64,
            h: 64,
        })
}

fn pack_atlas(images: &BTreeMap<String, TileRgba>) -> (Vec<u8>, BTreeMap<String, AtlasSlot>) {
    const G: u32 = 2;
    const MAX: u32 = 4096;
    let mut entries: Vec<(&String, &TileRgba)> = images.iter().collect();
    entries.sort_by(|a, b| b.1.w.cmp(&a.1.w).then(b.1.h.cmp(&a.1.h)));
    let mut placed: BTreeMap<String, (u32, u32, u32, u32)> = BTreeMap::new();
    let mut atlas_w = 64u32;
    let mut atlas_h = 64u32;
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_h = 0u32;
    for (name, img) in &entries {
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
    let png = encode_png_rgba(&rgba, atlas_w, atlas_h).unwrap_or_else(|_| vec![0x90; 16]);
    (png, uv_map)
}

fn signed_area(pts: &[[f32; 2]]) -> f32 {
    let mut a = 0.0;
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        a += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
    }
    a * 0.5
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

/// Fold ART tiles into per-thing assets instead of one card per tile:
///
/// 1. CON actors → one Doom-style `.billboard` each (8-way views +
///    walk/shoot/death states), consuming their action tile ranges.
///    Character preview is the walk cycle when it exists.
/// 2. CON `*FONT*` / `*ALPHANUM*` defines → one `.billboard` per font run
///    (glyph frames), never per letter.
/// 3. Remaining consecutive same-size sprite runs (fire cycles, pickup
///    spins, dancer loops) → one looping `.billboard`. Map-placed and
///    CON-named runs always land; unplaced irregular sprite strips land
///    too. Typical world-texture sizes stay out unless a map places them.
/// 4. Only leftover map-placed singleton tiles land as PNG cards, tagged
///    `leftover` and named from the CON define when there is one.
///
/// Finally the staged `*.place` sidecars are rewritten so face sprites
/// whose tile got folded reference the owning `.billboard` key.
pub fn assemble_duke_billboards(
    assets: &mut Vec<ClassicAsset>,
    staged: &Path,
    pack_dir: &Path,
    source_id: &str,
    art: &ArtBank,
    used_face: &BTreeSet<u16>,
) -> usize {
    let script = load_con_bundle(pack_dir);
    let parsed = parse_con(&script);
    let mut consumed: BTreeSet<u16> = BTreeSet::new();
    // Tile → owning `.billboard` catalog key, for `.place` rewrites.
    let mut owner: BTreeMap<u16, String> = BTreeMap::new();
    let mut seen_pic: BTreeSet<i32> = BTreeSet::new();
    let mut extra = Vec::new();
    for actor in &parsed.actors {
        if actor.tokens.iter().any(|t| t.eq_ignore_ascii_case("cactor")) {
            continue;
        }
        let Some(&pic) = parsed.defines.get(&actor.name) else {
            continue;
        };
        if !(0..=4095).contains(&pic) {
            continue;
        }
        let actions = actions_for_actor(actor, &parsed);
        if actions.is_empty() {
            continue;
        }
        let dir = staged.join("billboards").join(source_id);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        if !seen_pic.insert(pic) {
            for action in &actions {
                consumed.extend(action_tile_ids(pic, action));
            }
            continue;
        }
        let Some((mut bb, tiles)) =
            build_actor_billboard(&actor.name, pic as u16, &actions, art, &dir)
        else {
            continue;
        };
        for action in &actions {
            consumed.extend(action_tile_ids(pic, action));
        }
        let key = format!("billboards/{source_id}/{}", bb.prefix);
        let rel_path = format!("{key}.billboard");
        let Some(icon_rel) = pack_actor_sheet(staged, &staged.join(&rel_path), &mut bb) else {
            continue;
        };
        for &t in &tiles {
            owner.entry(t).or_insert_with(|| key.clone());
        }
        extra.push(ClassicAsset {
            key,
            kind: AssetKind::Billboard,
            rel_path,
            tags: vec![
                "billboard".into(),
                source_id.into(),
                "sprite".into(),
                "stateful".into(),
                bb.role.as_str().into(),
                actor.name.to_ascii_lowercase(),
            ],
            icon_rel: Some(icon_rel),
        });
        consumed.extend(tiles);
    }

    emit_font_billboards(&mut extra, staged, source_id, art, &parsed, &mut consumed);
    emit_fps_weapon_billboards(
        &mut extra,
        staged,
        source_id,
        art,
        &parsed,
        &mut consumed,
        &mut owner,
    );
    emit_strip_billboards(
        &mut extra,
        staged,
        source_id,
        art,
        &parsed,
        used_face,
        &mut consumed,
        &mut owner,
    );
    emit_leftover_tiles(
        &mut extra,
        staged,
        source_id,
        art,
        &parsed,
        used_face,
        &consumed,
        &mut owner,
    );

    // Defensive: no landing path may ever see a per-frame tile card for a
    // tile that belongs to a sheet.
    assets.retain(|a| {
        if a.kind != AssetKind::Billboard {
            return true;
        }
        let stem = a
            .key
            .rsplit('/')
            .next()
            .unwrap_or(a.key.as_str())
            .to_ascii_lowercase();
        let Some(num) = stem.strip_prefix("tile-") else {
            return true;
        };
        num.parse::<u16>().ok().is_none_or(|n| !consumed.contains(&n))
    });
    let n = extra.len();
    assets.extend(extra);
    rewrite_place_sidecars(staged, &owner);
    n
}

fn tile_slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches('-').to_string()
}

/// Two tiles read as frames of the same strip when their sizes are close.
/// ART frames of one animation often differ by a few pixels of trim.
fn similar_tile(a: &TileRgba, b: &TileRgba) -> bool {
    let dw = a.w.abs_diff(b.w);
    let dh = a.h.abs_diff(b.h);
    dw <= (a.w / 3).max(4) && dh <= (a.h / 4).max(4)
}

/// BUILD face-sprite: the tile is centered on the sprite, then the
/// whole quad is shifted by `-xofs` texels. Left edge in sprite space:
/// `-w/2 - xofs`. Feet stay on the sprite (bottom of the tile).
fn tile_left_x(tile: &TileRgba) -> i32 {
    -(tile.w as i32) / 2 - i32::from(tile.xoff)
}

/// Pad 1-view animation frames onto one canvas using the BUILD face
/// placement. Viewers that just center the PNG then keep the flame
/// planted. Do not use this across 8-way rotations — those are
/// different drawings, not one growing sprite.
fn align_face_strip(src: &[&TileRgba]) -> Vec<TileRgba> {
    if src.len() < 2 {
        return src.iter().map(|t| (*t).clone()).collect();
    }
    let mut min_l = i32::MAX;
    let mut max_r = i32::MIN;
    let mut max_h = 1u32;
    for t in src {
        let l = tile_left_x(t);
        min_l = min_l.min(l);
        max_r = max_r.max(l + t.w as i32);
        max_h = max_h.max(t.h);
    }
    let cw = (max_r - min_l).max(1) as u32;
    src.iter()
        .map(|t| {
            let dx = (tile_left_x(t) - min_l).max(0) as u32;
            let dy = max_h.saturating_sub(t.h);
            pad_tile_to(t, cw, max_h, dx, dy)
        })
        .collect()
}

fn pad_tile_to(src: &TileRgba, cw: u32, ch: u32, dx: u32, dy: u32) -> TileRgba {
    let mut rgba = vec![0u8; cw as usize * ch as usize * 4];
    for y in 0..src.h {
        for x in 0..src.w {
            let si = ((y * src.w + x) * 4) as usize;
            let di = (((y + dy) * cw + (x + dx)) * 4) as usize;
            if di + 4 <= rgba.len() && si + 4 <= src.rgba.len() {
                rgba[di..di + 4].copy_from_slice(&src.rgba[si..si + 4]);
            }
        }
    }
    TileRgba {
        w: cw,
        h: ch,
        rgba,
        xoff: 0,
        yoff: 0,
    }
}

/// Write one `.billboard` (sequential looping frames) plus its contact
/// sheet icon for a run of tiles. Returns the catalog key on success.
fn write_run_billboard(
    extra: &mut Vec<ClassicAsset>,
    staged: &Path,
    source_id: &str,
    art: &ArtBank,
    name: &str,
    run: &[u16],
    role: crate::stateful_billboard::SpriteRole,
    kind_tag: &str,
) -> Option<String> {
    let slug = tile_slug(name);
    if slug.is_empty() {
        return None;
    }
    let dir = staged.join("billboards").join(source_id);
    std::fs::create_dir_all(&dir).ok()?;
    let mut frames = Vec::new();
    let mut tiles: BTreeSet<u16> = BTreeSet::new();
    let mut letter = b'A';
    let existing: Vec<(u16, &TileRgba)> = run
        .iter()
        .filter_map(|&id| art.tiles.get(&id).map(|t| (id, t)))
        .collect();
    let aligned = align_face_strip(&existing.iter().map(|(_, t)| *t).collect::<Vec<_>>());
    for ((id, _), img) in existing.iter().zip(aligned.iter()) {
        let ch = letter as char;
        letter = if letter == b'Z' { b'A' } else { letter + 1 };
        push_frame_img(&mut frames, &mut tiles, *id, ch, 0, img, &dir);
    }
    if frames.len() < 2 {
        return None;
    }
    let mut bb = crate::stateful_billboard::sequential_idle(&slug, frames, role);
    let key = format!("billboards/{source_id}/{slug}");
    let rel_path = format!("{key}.billboard");
    let icon_rel = pack_actor_sheet(staged, &staged.join(&rel_path), &mut bb)?;
    extra.push(ClassicAsset {
        key: key.clone(),
        kind: AssetKind::Billboard,
        rel_path,
        tags: vec![
            "billboard".into(),
            source_id.into(),
            "sprite".into(),
            "stateful".into(),
            kind_tag.into(),
            slug,
        ],
        icon_rel: Some(icon_rel),
    });
    Some(key)
}

/// CON `*FONT*` / `*ALPHANUM*` defines mark glyph runs. One `.billboard`
/// per font — never a card per letter. `START…`/`END…` pairs bound the run
/// explicitly; lone defines walk consecutive tiles forward.
fn emit_font_billboards(
    extra: &mut Vec<ClassicAsset>,
    staged: &Path,
    source_id: &str,
    art: &ArtBank,
    parsed: &ConScript,
    consumed: &mut BTreeSet<u16>,
) {
    const MAX_GLYPHS: usize = 128;
    const MIN_GLYPHS: usize = 8;
    for (name, &val) in &parsed.defines {
        if !(name.contains("FONT") || name.contains("ALPHANUM")) || name.starts_with("END") {
            continue;
        }
        if !(0..=4095).contains(&val) {
            continue;
        }
        let start = val as u16;
        let end = name
            .strip_prefix("START")
            .and_then(|rest| parsed.defines.get(&format!("END{rest}")))
            .copied()
            .filter(|&e| e >= val && e <= 4095)
            .map(|e| e as u16);
        let mut run = Vec::new();
        match end {
            Some(end) => {
                for id in start..=end {
                    if !consumed.contains(&id) && art.tiles.contains_key(&id) {
                        run.push(id);
                    }
                }
            }
            None => {
                let mut id = start;
                while run.len() < MAX_GLYPHS
                    && !consumed.contains(&id)
                    && art.tiles.contains_key(&id)
                {
                    run.push(id);
                    let Some(next) = id.checked_add(1) else {
                        break;
                    };
                    id = next;
                }
            }
        }
        if run.len() < MIN_GLYPHS {
            continue;
        }
        if write_run_billboard(
            extra,
            staged,
            source_id,
            art,
            name,
            &run,
            crate::stateful_billboard::SpriteRole::Item,
            "font",
        )
        .is_some()
        {
            consumed.extend(run);
        }
    }
}

/// Typical BUILD wall/floor sizes. Unplaced runs of these stay out of the
/// catalog so brick/metal tiles do not become fake animation sheets.
fn is_world_texture_size(w: u32, h: u32) -> bool {
    matches!(
        (w, h),
        (32, 32)
            | (64, 64)
            | (128, 128)
            | (64, 128)
            | (128, 64)
            | (32, 64)
            | (64, 32)
            | (32, 128)
            | (128, 32)
    )
}

/// A leftover run is a sprite strip (not a world texture) when it is
/// irregularly sized, not a cinematic still, and not a 1-pixel HUD bar.
fn is_sprite_strip(run: &[u16], art: &ArtBank) -> bool {
    let Some(tile) = run.first().and_then(|id| art.tiles.get(id)) else {
        return false;
    };
    if tile.w >= 256 || tile.h >= 200 {
        return false;
    }
    if tile.w <= 4 || tile.h <= 6 {
        return false;
    }
    !is_world_texture_size(tile.w, tile.h)
}

fn should_emit_strip(
    run: &[u16],
    art: &ArtBank,
    used_face: &BTreeSet<u16>,
    _define_of: &BTreeMap<u16, &str>,
) -> bool {
    // A map-placed face sprite is always a strip, even at texture sizes
    // (switches, crates). Unplaced world-texture and cinematic runs stay
    // out — a CON name like BRICK or ORDERING must not pull them in.
    if run.first().copied().is_some_and(|t| t >= CINEMATIC_TILE_MIN) {
        return false;
    }
    if run.iter().any(|t| used_face.contains(t)) {
        return true;
    }
    is_sprite_strip(run, art)
}

/// TILES012+ is menus, order screens, and cinematic stills — not sprites.
const CINEMATIC_TILE_MIN: u16 = 3072;

fn is_fps_gun_define(name: &str) -> bool {
    matches!(
        name,
        "KNEE"
            | "FIRSTGUN"
            | "FIRSTGUNRELOAD"
            | "CHAINGUN"
            | "RPGGUN"
            | "TRIPBOMB"
            | "HANDHOLDINGACCESS"
            | "HANDREMOTE"
            | "HANDTHROW"
            | "SHOTGUN"
            | "HANDHOLDINGLASER"
            | "SHRINKER"
            | "FLAMETHROWER"
            | "SCUBAMASK"
    )
}

/// First-person HUD guns live in the 2500s and have no CON `action` on an
/// actor, so the character pass never sees them. One sheet per define.
fn emit_fps_weapon_billboards(
    extra: &mut Vec<ClassicAsset>,
    staged: &Path,
    source_id: &str,
    art: &ArtBank,
    parsed: &ConScript,
    consumed: &mut BTreeSet<u16>,
    owner: &mut BTreeMap<u16, String>,
) {
    let mut starts: Vec<(u16, String)> = parsed
        .defines
        .iter()
        .filter(|(n, &v)| is_fps_gun_define(n) && (0..=4095).contains(&v))
        .map(|(n, &v)| (v as u16, n.clone()))
        .collect();
    starts.sort_by_key(|s| s.0);
    starts.dedup_by_key(|s| s.0);
    for i in 0..starts.len() {
        let start = starts[i].0;
        if consumed.contains(&start) {
            continue;
        }
        let stop = starts
            .get(i + 1)
            .map(|s| s.0)
            .unwrap_or(start.saturating_add(12));
        let mut run = Vec::new();
        for id in start..stop {
            if consumed.contains(&id) {
                break;
            }
            if art.tiles.contains_key(&id) {
                run.push(id);
            }
        }
        if run.len() < 2 {
            continue;
        }
        let Some(key) = write_run_billboard(
            extra,
            staged,
            source_id,
            art,
            &starts[i].1,
            &run,
            crate::stateful_billboard::SpriteRole::Weapon,
            "weapon",
        ) else {
            continue;
        };
        for &t in &run {
            owner.entry(t).or_insert_with(|| key.clone());
        }
        consumed.extend(run);
    }
}

/// Group leftover consecutive same-size tile runs (fire cycles, spins,
/// dancer loops) into one looping `.billboard` each. Map-placed and
/// CON-named runs always land; unplaced irregular sprite strips (the
/// orphan fire cycle, steam, etc.) land too. Unplaced world-texture runs
/// stay out.
fn emit_strip_billboards(
    extra: &mut Vec<ClassicAsset>,
    staged: &Path,
    source_id: &str,
    art: &ArtBank,
    parsed: &ConScript,
    used_face: &BTreeSet<u16>,
    consumed: &mut BTreeSet<u16>,
    owner: &mut BTreeMap<u16, String>,
) {
    const MIN_RUN: usize = 4;
    let mut define_of: BTreeMap<u16, &str> = BTreeMap::new();
    for (name, &val) in &parsed.defines {
        if (0..=4095).contains(&val) {
            define_of.entry(val as u16).or_insert(name.as_str());
        }
    }
    let mut runs: Vec<Vec<u16>> = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    for (&id, tile) in &art.tiles {
        if i32::from(id) <= i32::from(CONTROL_TILE_MAX) || consumed.contains(&id) {
            if cur.len() >= MIN_RUN {
                runs.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        let head = cur.first().and_then(|f| art.tiles.get(f));
        let continues = cur
            .last()
            .is_some_and(|&last| id == last + 1 && head.is_some_and(|h| similar_tile(h, tile)));
        if !continues {
            if cur.len() >= MIN_RUN {
                runs.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
        cur.push(id);
    }
    if cur.len() >= MIN_RUN {
        runs.push(cur);
    }
    for run in runs {
        if !should_emit_strip(&run, art, used_face, &define_of) {
            continue;
        }
        let name = run
            .iter()
            .find_map(|t| define_of.get(t).copied())
            .map(str::to_string)
            .unwrap_or_else(|| format!("strip-{}", run[0]));
        let Some(key) = write_run_billboard(
            extra,
            staged,
            source_id,
            art,
            &name,
            &run,
            crate::stateful_billboard::SpriteRole::Effect,
            "effect",
        ) else {
            continue;
        };
        for &t in &run {
            owner.entry(t).or_insert_with(|| key.clone());
        }
        consumed.extend(run);
    }
}

fn leftover_define_is_noise(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    u.contains("_GROWL")
        || u.contains("_TALK")
        || u.contains("_ROAM")
        || u.contains("_RECOG")
        || u.contains("_PAIN")
        || u.contains("_DYING")
        || u.contains("_ATTACK")
        || u.contains("_WEAPON")
        || u.contains("_STRENGTH")
        || u.contains("_AMOUNT")
}

/// True singleton leftovers: tiles a map actually places that no sheet
/// consumed. These are the only per-tile PNG cards the import may produce.
/// Named from the CON define when one exists (`firstgunsprite`), otherwise
/// `tile-N`.
fn emit_leftover_tiles(
    extra: &mut Vec<ClassicAsset>,
    staged: &Path,
    source_id: &str,
    art: &ArtBank,
    parsed: &ConScript,
    used_face: &BTreeSet<u16>,
    consumed: &BTreeSet<u16>,
    owner: &mut BTreeMap<u16, String>,
) {
    let mut define_of: BTreeMap<u16, &str> = BTreeMap::new();
    for actor in &parsed.actors {
        if let Some(&val) = parsed.defines.get(&actor.name) {
            if (0..=4095).contains(&val) {
                define_of.insert(val as u16, actor.name.as_str());
            }
        }
    }
    for (name, &val) in &parsed.defines {
        if !(0..=4095).contains(&val) || leftover_define_is_noise(name) {
            continue;
        }
        define_of.entry(val as u16).or_insert(name.as_str());
    }
    for &pic in used_face {
        if i32::from(pic) <= i32::from(CONTROL_TILE_MAX)
            || pic >= CINEMATIC_TILE_MIN
            || consumed.contains(&pic)
        {
            continue;
        }
        let Some(tile) = art.tiles.get(&pic) else {
            continue;
        };
        let Ok(png) = encode_png_rgba(&tile.rgba, tile.w, tile.h) else {
            continue;
        };
        let slug = define_of
            .get(&pic)
            .copied()
            .map(tile_slug)
            .filter(|s| {
                !s.is_empty()
                    && crate::stateful_billboard::parse_doom_sprite_name(s).is_none()
            })
            .unwrap_or_else(|| format!("tile-{pic}"));
        let key = format!("billboards/{source_id}/{slug}");
        let rel_path = format!("{key}.png");
        let dest = staged.join(&rel_path);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&dest, png).is_err() {
            continue;
        }
        owner.entry(pic).or_insert_with(|| key.clone());
        extra.push(ClassicAsset {
            key,
            kind: AssetKind::Billboard,
            rel_path: rel_path.clone(),
            tags: vec![
                "billboard".into(),
                source_id.into(),
                "sprite".into(),
                "leftover".into(),
                format!("tile-{pic}"),
                slug,
            ],
            icon_rel: Some(rel_path),
        });
    }
}

/// Point `.place` face-sprite rows at the sheet that consumed their tile,
/// so the world walk shows the animated actor instead of a dead link.
fn rewrite_place_sidecars(staged: &Path, owner: &BTreeMap<u16, String>) {
    if owner.is_empty() {
        return;
    }
    let mut sidecars = Vec::new();
    walk_ext(&staged.join("worlds"), "place", &mut sidecars);
    for path in sidecars {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut place) = crate::world_place::WorldPlace::parse(&text) else {
            continue;
        };
        let mut changed = false;
        for p in &mut place.places {
            let Ok(pic) = p.class.parse::<u16>() else {
                continue;
            };
            if let Some(key) = owner.get(&pic) {
                if p.asset != *key {
                    p.asset = key.clone();
                    changed = true;
                }
            }
        }
        if changed {
            let _ = std::fs::write(&path, place.to_text());
        }
    }
}

struct ConScript {
    defines: BTreeMap<String, i32>,
    actions: BTreeMap<String, DukeAction>,
    /// `ai AIPIGSEEKENEMY APIGWALK …` — walk/run live on the AI, not `action`.
    ais: BTreeMap<String, String>,
    states: BTreeMap<String, Vec<String>>,
    actors: Vec<DukeActor>,
}

#[derive(Clone, Debug)]
struct DukeAction {
    name: String,
    start: i32,
    frames: i32,
    viewtype: i32,
    increment: i32,
    delay: i32,
}

struct DukeActor {
    name: String,
    tokens: Vec<String>,
}

fn load_con_bundle(pack_dir: &Path) -> String {
    let mut out = String::new();
    for name in ["DEFS.CON", "GAME.CON", "USER.CON"] {
        if let Ok(text) = std::fs::read_to_string(pack_dir.join(name)) {
            out.push_str(&text);
            out.push('\n');
        }
    }
    let mut grps = Vec::new();
    walk_ext(pack_dir, "grp", &mut grps);
    for grp in grps {
        let Ok(bytes) = std::fs::read(&grp) else {
            continue;
        };
        let Ok(files) = parse_grp(&bytes) else {
            continue;
        };
        for (name, data) in files {
            if name.to_ascii_uppercase().ends_with(".CON") {
                out.push_str(&String::from_utf8_lossy(&data));
                out.push('\n');
            }
        }
    }
    out
}

fn parse_con(text: &str) -> ConScript {
    let tokens = con_tokens(text);
    let mut defines = BTreeMap::new();
    let mut actions = BTreeMap::new();
    let mut ais = BTreeMap::new();
    let mut states = BTreeMap::new();
    let mut actors = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let key = tokens[i].to_ascii_uppercase();
        match key.as_str() {
            "DEFINE" if i + 2 < tokens.len() => {
                if let Ok(n) = tokens[i + 2].parse::<i32>() {
                    defines.insert(tokens[i + 1].to_ascii_uppercase(), n);
                }
                i += 3;
            }
            "ACTION" if i + 1 < tokens.len() => {
                let name = tokens[i + 1].to_ascii_uppercase();
                let mut nums = [0i32, 1, 1, 1, 8];
                let mut got = 0usize;
                let mut j = i + 2;
                while j < tokens.len() && got < 5 {
                    if let Ok(n) = tokens[j].parse::<i32>() {
                        nums[got] = n;
                        got += 1;
                        j += 1;
                    } else {
                        break;
                    }
                }
                if got == 0 {
                    nums[1] = 1;
                }
                actions.insert(
                    name.clone(),
                    DukeAction {
                        name,
                        start: nums[0],
                        frames: nums[1].max(1),
                        viewtype: if got >= 3 { nums[2] } else { 1 },
                        increment: if got >= 4 { nums[3] } else { 1 },
                        delay: if got >= 5 { nums[4] } else { 8 },
                    },
                );
                i = j;
            }
            "STATE" if i + 1 < tokens.len() => {
                let name = tokens[i + 1].to_ascii_uppercase();
                let start = i + 2;
                let mut j = start;
                while j < tokens.len() && !tokens[j].eq_ignore_ascii_case("ends") {
                    j += 1;
                }
                states.insert(name, tokens[start..j].to_vec());
                i = j + 1;
            }
            "ACTOR" | "USERACTOR" if i + 1 < tokens.len() => {
                let mut k = i + 1;
                if key == "USERACTOR" && k < tokens.len() {
                    k += 1; // skip enemy/notenemy/etc
                }
                if k >= tokens.len() {
                    break;
                }
                let name = tokens[k].to_ascii_uppercase();
                k += 1;
                let start = k;
                while k < tokens.len() && !tokens[k].eq_ignore_ascii_case("enda") {
                    k += 1;
                }
                actors.push(DukeActor {
                    name,
                    tokens: tokens[start..k].to_vec(),
                });
                i = k + 1;
            }
            _ => i += 1,
        }
    }
    // Second pass: `ai NAME ACTION move type`. ACTION lines must already
    // be in the map so we do not bind a move name as an action.
    let mut i = 0;
    while i + 2 < tokens.len() {
        if tokens[i].eq_ignore_ascii_case("ai") {
            let act = tokens[i + 2].to_ascii_uppercase();
            if actions.contains_key(&act) {
                ais.insert(tokens[i + 1].to_ascii_uppercase(), act);
            }
        }
        i += 1;
    }
    ConScript {
        defines,
        actions,
        ais,
        states,
        actors,
    }
}

fn con_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or("");
        for t in line.split_whitespace() {
            if !t.is_empty() {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn actions_for_actor<'a>(actor: &DukeActor, script: &'a ConScript) -> Vec<&'a DukeAction> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut pending = vec![actor.tokens.clone()];
    let mut seen_state = BTreeSet::new();
    while let Some(tokens) = pending.pop() {
        let mut i = 0;
        while i < tokens.len() {
            let u = tokens[i].to_ascii_uppercase();
            if u == "STATE" {
                if let Some(name) = tokens.get(i + 1) {
                    let key = name.to_ascii_uppercase();
                    if seen_state.insert(key.clone()) {
                        if let Some(body) = script.states.get(&key) {
                            pending.push(body.clone());
                        }
                    }
                    i += 2;
                    continue;
                }
            } else if u == "ACTION" {
                if let Some(name) = tokens.get(i + 1) {
                    names.insert(name.to_ascii_uppercase());
                    i += 2;
                    continue;
                }
            } else if let Some(act) = script.ais.get(&u) {
                names.insert(act.clone());
            } else if script.actions.contains_key(&u) {
                names.insert(u);
            }
            i += 1;
        }
    }
    names
        .into_iter()
        .filter_map(|n| script.actions.get(&n))
        .collect()
}

/// ACTION viewtype: how many pics are stored per animation frame.
/// 5 and 7 store 5 unique views (front / ¾ / side / ¾-back / back).
/// Facings 6/7/8 are X-flips of 4/3/2 (`mirrors 8` on the sheet).
fn action_tile_ids(pic: i32, action: &DukeAction) -> BTreeSet<u16> {
    let views = i32::from(views_for(action.viewtype).max(1));
    let frames = action.frames.max(1);
    let mut out = BTreeSet::new();
    if views <= 1 {
        let inc = if action.increment == 0 {
            1
        } else {
            action.increment
        };
        for fi in 0..frames {
            let t = pic + action.start + fi * inc;
            if (0..=4095).contains(&t) {
                out.insert(t as u16);
            }
        }
    } else {
        for fi in 0..frames {
            for rot in 0..views {
                let t = pic + action.start + fi * views + rot;
                if (0..=4095).contains(&t) {
                    out.insert(t as u16);
                }
            }
        }
    }
    out
}

fn views_for(viewtype: i32) -> u8 {
    match viewtype {
        5 | 7 => 5,
        8 => 7,
        4 => 4,
        _ => 1,
    }
}

fn action_state_name(action: &str) -> Option<&'static str> {
    let u = action.to_ascii_uppercase();
    // Do not dump unmatched actions into idle — APIGDIVE / ATROOPABOUTHIDE
    // were stealing the stand pose and the gallery preview.
    Some(if u.contains("DEAD") || u.contains("DYING") || u.contains("DIE") {
        "death"
    } else if u.contains("WALKBACK") || u.contains("RUNBACK") {
        "walkback"
    } else if u.contains("WALK") || u.contains("CRAWL") || u.contains("SWIM") {
        "walk"
    } else if u.contains("RUN") {
        "run"
    } else if u.contains("SHOOT") || u.contains("ATTACK") || u.contains("LOB") {
        "attack"
    } else if u.contains("COCK") {
        "cock"
    } else if u.contains("JUMP") {
        "jump"
    } else if u.contains("DUCK") || u.contains("HIDE") {
        "duck"
    } else if u.contains("JET") || u.contains("FLY") {
        "fly"
    } else if u.contains("DIVE") {
        "dive"
    } else if u.contains("PAIN") || u.contains("FLINTCH") || u.contains("HIT") {
        "pain"
    } else if u.contains("STAND") || u.contains("WAIT") || u.contains("THINK") {
        "idle"
    } else {
        return None;
    })
}

fn action_rank(action: &DukeAction, state: &str) -> (i32, i32) {
    let u = action.name.to_ascii_uppercase();
    let quality = match state {
        "idle" if u.contains("STAND") => 3,
        "idle" if u.contains("WAIT") => 2,
        "attack" if u.contains("SHOOT") || u.contains("ATTACK") => 3,
        "attack" if u.contains("LOB") => 2,
        "death" if u.contains("DYING") => 3,
        "death" if u.contains("DEAD") => 1,
        "walk" if u.contains("WALK") && !u.contains("BACK") => 3,
        _ => 1,
    };
    (quality, action.frames)
}

fn pick_character_preview(
    states: &[crate::stateful_billboard::AnimState],
    frames: &[crate::stateful_billboard::SpriteFrame],
) -> String {
    for name in ["walk", "run", "idle", "fly"] {
        let Some(state) = states.iter().find(|s| s.name == name) else {
            continue;
        };
        let slice = &frames[state.first..state.last];
        let nfront = slice.iter().filter(|f| f.rot <= 1).count();
        let mut letters = Vec::new();
        for f in slice {
            if !letters.contains(&f.letter) {
                letters.push(f.letter);
            }
        }
        if letters.len() >= 2 || nfront >= 2 {
            return state.name.clone();
        }
    }
    states
        .iter()
        .find(|s| s.r#loop && s.last.saturating_sub(s.first) >= 2)
        .or_else(|| states.first())
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "idle".into())
}

fn actor_role(name: &str) -> crate::stateful_billboard::SpriteRole {
    use crate::stateful_billboard::SpriteRole;
    let u = name.to_ascii_uppercase();
    if u.contains("EXPLOSION")
        || u.contains("SMOKE")
        || u.contains("FLAME")
        || u.contains("BUBBLE")
        || u.contains("STEAM")
    {
        SpriteRole::Effect
    } else if u.contains("GUN") || u.contains("SHOT") || u.contains("RPG") || u.contains("AMMO")
    {
        SpriteRole::Item
    } else {
        SpriteRole::Character
    }
}

fn build_actor_billboard(
    actor: &str,
    pic: u16,
    actions: &[&DukeAction],
    art: &ArtBank,
    dest_dir: &Path,
) -> Option<(
    crate::stateful_billboard::StatefulBillboard,
    BTreeSet<u16>,
)> {
    use crate::stateful_billboard::{AnimState, StatefulBillboard};
    let mut frames = Vec::new();
    let mut states = Vec::new();
    let mut tiles = BTreeSet::new();
    let mut letter = b'A';
    let mut by_state: BTreeMap<&'static str, &DukeAction> = BTreeMap::new();
    let mut unmatched: Vec<&DukeAction> = Vec::new();
    for action in actions {
        match action_state_name(&action.name) {
            Some(state) => match by_state.get(state) {
                None => {
                    by_state.insert(state, action);
                }
                Some(prev) if action_rank(action, state) > action_rank(prev, state) => {
                    by_state.insert(state, action);
                }
                _ => {}
            },
            None => unmatched.push(action),
        }
    }
    if !by_state.contains_key("idle") {
        if let Some(best) = unmatched.iter().copied().max_by_key(|a| a.frames) {
            by_state.insert("idle", best);
        }
    }
    let mut ordered: Vec<(&str, &DukeAction)> = by_state.into_iter().collect();
    ordered.sort_by_key(|(state, _)| match *state {
        "idle" => 0,
        "walk" => 1,
        "run" => 2,
        "attack" => 3,
        "pain" => 4,
        "jump" => 5,
        "duck" => 6,
        "fly" => 7,
        "dive" => 8,
        "cock" => 9,
        "death" => 10,
        _ => 11,
    });
    for (state, action) in ordered {
        let views = views_for(action.viewtype);
        let first = frames.len();
        let frames_n = action.frames.max(1) as usize;
        let mut batch: Vec<(char, u8, i32)> = Vec::new();
        for fi in 0..frames_n {
            let ch = letter as char;
            letter = if letter == b'Z' { b'A' } else { letter + 1 };
            if views <= 1 {
                let inc = if action.increment == 0 { 1 } else { action.increment };
                let tile = pic as i32 + action.start + (fi as i32) * inc;
                batch.push((ch, 0, tile));
            } else {
                for rot in 0..views {
                    let tile = pic as i32 + action.start + (fi as i32) * i32::from(views) + i32::from(rot);
                    batch.push((ch, rot + 1, tile));
                }
            }
        }
        let existing: Vec<(char, u8, u16, &TileRgba)> = batch
            .iter()
            .filter_map(|(ch, rot, t)| {
                if !(0..=4095).contains(t) {
                    return None;
                }
                art.tiles
                    .get(&(*t as u16))
                    .map(|img| (*ch, *rot, *t as u16, img))
            })
            .collect();
        // Only 1-view strips (fire, explosions) share a canvas. 8-way
        // walk/idle frames are different angles — packing them together
        // made every PNG a wide sheet and the character slid on orbit.
        let imgs: Vec<TileRgba> = if views <= 1 {
            align_face_strip(&existing.iter().map(|e| e.3).collect::<Vec<_>>())
        } else {
            existing.iter().map(|e| e.3.clone()).collect()
        };
        for (i, (ch, rot, id, _)) in existing.iter().enumerate() {
            push_frame_img(&mut frames, &mut tiles, *id, *ch, *rot, &imgs[i], dest_dir);
        }
        let last = frames.len();
        if last > first {
            let looping = matches!(state, "walk" | "run" | "idle" | "fly");
            let fps = (30 / action.delay.max(1)).clamp(if looping { 6 } else { 2 }, 16) as u8;
            states.push(AnimState {
                name: state.into(),
                first,
                last,
                r#loop: looping,
                fps,
            });
        }
    }
    if frames.len() < 2 {
        return None;
    }
    let multi = states.iter().any(|s| {
        frames[s.first..s.last]
            .iter()
            .any(|f| f.rot > 1)
    });
    if !multi && frames.len() < 8 {
        return None;
    }
    let preview = pick_character_preview(&states, &frames);
    let stored = frames.iter().map(|f| f.rot).max().unwrap_or(1);
    Some((
        StatefulBillboard {
            prefix: actor.to_ascii_lowercase(),
            role: actor_role(actor),
            preview,
            // Duke viewtype 5 stores 5 unique views; 6/7/8 are X-flips.
            facings: if stored >= 4 { 8 } else { stored },
            mirrors: if stored >= 4 { 8 } else { 0 },
            states,
            frames,
            sheet: None,
        },
        tiles,
    ))
}

fn push_frame(
    frames: &mut Vec<crate::stateful_billboard::SpriteFrame>,
    tiles: &mut BTreeSet<u16>,
    tile: i32,
    letter: char,
    rot: u8,
    art: &ArtBank,
    dest_dir: &Path,
) -> bool {
    if !(0..=4095).contains(&tile) {
        return false;
    }
    let id = tile as u16;
    let Some(img) = art.tiles.get(&id) else {
        return false;
    };
    push_frame_img(frames, tiles, id, letter, rot, img, dest_dir)
}

fn push_frame_img(
    frames: &mut Vec<crate::stateful_billboard::SpriteFrame>,
    tiles: &mut BTreeSet<u16>,
    id: u16,
    letter: char,
    rot: u8,
    img: &TileRgba,
    dest_dir: &Path,
) -> bool {
    let rel = format!("{:02}/tile-{id:04}.png", id / 100);
    let path = dest_dir.join(&rel);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(png) = encode_png_rgba(&img.rgba, img.w, img.h) else {
        return false;
    };
    if std::fs::write(&path, png).is_err() {
        return false;
    }
    tiles.insert(id);
    frames.push(crate::stateful_billboard::SpriteFrame {
        letter,
        rot,
        w: img.w,
        h: img.h,
        file: rel,
        flip: false,
        cell: None,
    });
    true
}

/// Pack an actor's tiles into ONE sheet beside its manifest, write the
/// manifest that indexes it and drop the per-tile PNGs it replaced. Returns
/// the staged-relative animated preview strip.
fn pack_actor_sheet(
    staged: &Path,
    manifest: &Path,
    bb: &mut crate::stateful_billboard::StatefulBillboard,
) -> Option<String> {
    let written = match crate::billboard_sheet::write_with_sheet(manifest, bb) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[duke-import] billboard sheet {}: {e}", manifest.display());
            return None;
        }
    };
    for frame in &written.consumed {
        let _ = std::fs::remove_file(frame);
        // Tiles live in `billboards/duke3d/NN/` buckets; the bucket goes
        // once its last frame is packed (map-placed leftovers are flat).
        if let Some(parent) = frame.parent() {
            if Some(parent) != manifest.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
    written
        .thumb
        .as_deref()
        .unwrap_or(&written.sheet)
        .strip_prefix(staged)
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A BUILD ceiling door (sector lotag 20) whose ceiling sits on its
    /// floor: the mover opens it to the neighbour's ceiling.
    #[test]
    fn a_build_ceiling_door_bakes_open() {
        let mut map = BuildMap {
            start: [0, 0, 0],
            start_ang: 0,
            start_sec: 0,
            sectors: vec![
                Sector {
                    wallptr: 0,
                    wallnum: 2,
                    ceilingz: -8192,
                    floorz: 0,
                    ceilingstat: 0,
                    floorstat: 0,
                    ceilingpicnum: 0,
                    ceilingheinum: 0,
                    ceilingshade: 0,
                    ceilingxpanning: 0,
                    ceilingypanning: 0,
                    floorpicnum: 0,
                    floorheinum: 0,
                    floorshade: 0,
                    floorxpanning: 0,
                    floorypanning: 0,
                    lotag: 0,
                    hitag: 0,
                },
                Sector {
                    wallptr: 2,
                    wallnum: 2,
                    // Closed: ceiling on the floor.
                    ceilingz: 0,
                    floorz: 0,
                    ceilingstat: 0,
                    floorstat: 0,
                    ceilingpicnum: 0,
                    ceilingheinum: 0,
                    ceilingshade: 0,
                    ceilingxpanning: 0,
                    ceilingypanning: 0,
                    floorpicnum: 0,
                    floorheinum: 0,
                    floorshade: 0,
                    floorxpanning: 0,
                    floorypanning: 0,
                    lotag: ST_CEILING_DOOR,
                    hitag: 0,
                },
            ],
            walls: vec![
                wall_to(0, 0, 1),
                wall_to(1024, 0, -1),
                wall_to(0, 0, 0),
                wall_to(1024, 0, -1),
            ],
            sprites: Vec::new(),
        };
        let movers = build_movers(&map);
        assert_eq!(movers.len(), 1, "{movers:?}");
        assert_eq!(movers[0].sector, 1);
        assert!(movers[0].ceiling);
        assert_eq!(movers[0].closed_z, 0);
        assert_eq!(movers[0].open_z, -8192, "up to the neighbour's ceiling");
        open_movers(&mut map, &movers);
        assert_eq!(map.sectors[1].ceilingz, -8192, "baked open");
        // A sector with no door tag never moves.
        map.sectors[1].lotag = 0;
        assert!(build_movers(&map).is_empty());
    }

    fn wall_to(x: i32, y: i32, nextsector: i16) -> Wall {
        Wall {
            x,
            y,
            point2: 0,
            nextwall: -1,
            nextsector,
            cstat: 0,
            picnum: 0,
            overpicnum: 0,
            shade: 0,
            xrepeat: 8,
            yrepeat: 8,
            xpanning: 0,
            ypanning: 0,
        }
    }

    #[test]
    fn grp_roundtrip_one_file() {
        let mut bytes = b"KenSilverman".to_vec();
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let mut name = [0u8; 12];
        name[..8].copy_from_slice(b"E1L1.MAP");
        bytes.extend_from_slice(&name);
        let payload = b"hello-grp";
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(payload);
        let files = parse_grp(&bytes).expect("grp");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "E1L1.MAP");
        assert_eq!(files[0].1, payload);
    }

    #[test]
    fn grp_palette_is_6bit_and_colors_tiles() {
        let mut pal = vec![0u8; 768];
        pal[3] = 63;
        pal[4] = 0;
        pal[5] = 0; // index 1 = 6-bit red
        pal[765] = 63;
        pal[766] = 0;
        pal[767] = 63; // index 255 magenta
        let parsed = parse_palette_dat(&pal).expect("pal");
        assert_eq!(parsed[1], [252, 0, 0]);
        assert_eq!(parsed[255], [252, 0, 252]);

        let mut art = vec![0u8; 16 + 8 + 1];
        art[0..4].copy_from_slice(&1i32.to_le_bytes());
        art[12..16].copy_from_slice(&0i32.to_le_bytes()); // end = start = 0
        art[16..18].copy_from_slice(&1u16.to_le_bytes());
        art[18..20].copy_from_slice(&1u16.to_le_bytes());
        art[24] = 1; // pixel index 1
        let mut grp = b"KenSilverman".to_vec();
        grp.extend_from_slice(&2u32.to_le_bytes());
        let mut e1 = [0u8; 16];
        e1[..11].copy_from_slice(b"PALETTE.DAT");
        e1[12..16].copy_from_slice(&(pal.len() as u32).to_le_bytes());
        let mut e2 = [0u8; 16];
        e2[..12].copy_from_slice(b"TILES000.ART");
        e2[12..16].copy_from_slice(&(art.len() as u32).to_le_bytes());
        grp.extend_from_slice(&e1);
        grp.extend_from_slice(&e2);
        grp.extend_from_slice(&pal);
        grp.extend_from_slice(&art);

        let dir = std::env::temp_dir().join(format!("duke-pal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let grp_path = dir.join("DUKE3D.GRP");
        std::fs::write(&grp_path, &grp).unwrap();
        std::fs::write(dir.join("TILES000.ART"), &art).unwrap();
        let bank = load_tileset(&[], &dir);
        let tile = bank.tiles.get(&0).expect("tile 0");
        assert_eq!(tile.rgba, vec![252, 0, 0, 255]);
        assert!(dir.join("PALETTE.DAT").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn con_actor_becomes_one_stateful_billboard() {
        let con = r#"
define LIZTROOP 10
action ATROOPSTAND 0 1 5 1 1
action ATROOPWALKING 0 2 5 1 12
action ATROOPSHOOT 16 1 5 1 30
action ATROOPDYING 24 3 1 1 16
state troopcode
    action ATROOPWALKING
    action ATROOPSHOOT
    action ATROOPDYING
ends
actor LIZTROOP 30 ATROOPSTAND state troopcode enda
"#;
        let parsed = parse_con(con);
        assert_eq!(parsed.defines.get("LIZTROOP"), Some(&10));
        assert_eq!(parsed.actions.get("ATROOPWALKING").unwrap().viewtype, 5);
        let actor = parsed.actors.iter().find(|a| a.name == "LIZTROOP").unwrap();
        let acts = actions_for_actor(actor, &parsed);
        assert!(acts.iter().any(|a| a.name == "ATROOPWALKING"));
        assert!(acts.iter().any(|a| a.name == "ATROOPDYING"));

        let mut art = ArtBank::default();
        for id in 10u16..40 {
            art.tiles.insert(
                id,
                TileRgba {
                    w: 8,
                    h: 8,
                    rgba: vec![255, 0, 0, 255].repeat(64),
                    xoff: 0,
                    yoff: 0,
                },
            );
        }
        let dir = std::env::temp_dir().join(format!(
            "duke-bb-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let (bb, tiles) = build_actor_billboard("LIZTROOP", 10, &acts, &art, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(bb.states.iter().any(|s| s.name == "walk"));
        assert!(bb.states.iter().any(|s| s.name == "attack"));
        assert!(bb.states.iter().any(|s| s.name == "death"));
        assert_eq!(bb.preview, "walk");
        assert!(bb.frames.iter().any(|f| f.rot == 5));
        assert_eq!(bb.mirrors, 8);
        assert_eq!(bb.facings, 8);
        assert!(tiles.len() >= 16);
        assert_eq!(bb.prefix, "liztroop");
    }

    #[test]
    fn ai_binding_pulls_walk_like_doom_states() {
        let con = r#"
define PIGCOP 2000
action APIGSTAND 30 1 5 1 1
action APIGWALK 0 4 5 1 20
action APIGSHOOT 30 2 5 1 58
action APIGDYING 55 5 1 1 15
ai AIPIGSEEKENEMY APIGWALK PIGWALKVELS seekplayer
ai AIPIGSHOOTENEMY APIGSHOOT PIGSTOPPED face_player
ai AIPIGDYING APIGDYING PIGSTOPPED face_player
actor PIGCOP 30 APIGSTAND
    ifai AIPIGSEEKENEMY
    ifai AIPIGSHOOTENEMY
    ifai AIPIGDYING
enda
"#;
        let parsed = parse_con(con);
        assert_eq!(
            parsed.ais.get("AIPIGSEEKENEMY").map(String::as_str),
            Some("APIGWALK")
        );
        let actor = parsed.actors.iter().find(|a| a.name == "PIGCOP").unwrap();
        let acts = actions_for_actor(actor, &parsed);
        assert!(acts.iter().any(|a| a.name == "APIGWALK"), "{acts:?}");
        assert!(acts.iter().any(|a| a.name == "APIGSHOOT"));
        let tiles = action_tile_ids(2000, parsed.actions.get("APIGWALK").unwrap());
        assert!(tiles.contains(&2001));
        assert!(tiles.contains(&2016));
        assert_eq!(tiles.len(), 20);
        let mut art = ArtBank::default();
        for id in 2000u16..=2070 {
            art.tiles.insert(id, flat_tile(40, 60));
        }
        let dir = std::env::temp_dir().join(format!(
            "duke-pig-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let (bb, _) = build_actor_billboard("PIGCOP", 2000, &acts, &art, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(bb.preview, "walk", "gallery must cycle the walk, not stand");
        assert!(bb.states.iter().any(|s| s.name == "idle"));
    }

    #[test]
    fn dive_does_not_steal_idle_or_preview() {
        let con = r#"
define PIGCOP 2000
action APIGSTAND 30 1 5 1 1
action APIGWALK 0 4 5 1 20
action APIGDIVE 40 2 5 1 40
action APIGSHOOT 30 2 5 1 58
action APIGDYING 55 5 1 1 15
ai AIPIGSEEKENEMY APIGWALK PIGWALKVELS seekplayer
ai AIPIGDIVING APIGDIVE PIGSTOPPED face_player
actor PIGCOP 30 APIGSTAND
    ifai AIPIGSEEKENEMY
    ifai AIPIGDIVING
    action APIGSHOOT
    action APIGDYING
enda
"#;
        let parsed = parse_con(con);
        let actor = parsed.actors.iter().find(|a| a.name == "PIGCOP").unwrap();
        let acts = actions_for_actor(actor, &parsed);
        let mut art = ArtBank::default();
        for id in 2000u16..=2070 {
            art.tiles.insert(id, flat_tile(40, 60));
        }
        let dir = std::env::temp_dir().join(format!(
            "duke-dive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let (bb, _) = build_actor_billboard("PIGCOP", 2000, &acts, &art, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(bb.preview, "walk");
        assert!(bb.states.iter().any(|s| s.name == "dive"), "{:?}", bb.states);
        let idle = bb.states.iter().find(|s| s.name == "idle").unwrap();
        let idle_files: Vec<&str> = bb.frames[idle.first..idle.last]
            .iter()
            .map(|f| f.file.as_str())
            .collect();
        assert!(
            idle_files.iter().all(|f| f.contains("203")),
            "stand tiles, not dive 2040: {idle_files:?}"
        );
        assert!(
            bb.preview_frames().len() >= 2,
            "gallery needs a cycling walk"
        );
    }

    fn test_dirs(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "duke-group-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let staged = base.join("staged");
        let pack = base.join("pack");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::create_dir_all(&pack).unwrap();
        (staged, pack)
    }

    fn flat_tile(w: u32, h: u32) -> TileRgba {
        TileRgba {
            w,
            h,
            rgba: vec![200, 30, 30, 255].repeat((w * h) as usize),
            xoff: 0,
            yoff: 0,
        }
    }

    #[test]
    fn fire_frames_share_a_pivot_so_they_dont_slide() {
        let a = TileRgba {
            w: 31,
            h: 52,
            rgba: vec![255, 0, 0, 255].repeat(31 * 52),
            xoff: -1,
            yoff: 24,
        };
        let b = TileRgba {
            w: 32,
            h: 67,
            rgba: vec![0, 255, 0, 255].repeat(32 * 67),
            xoff: -1,
            yoff: 33,
        };
        let aligned = align_face_strip(&[&a, &b]);
        assert_eq!(aligned[0].w, aligned[1].w);
        assert_eq!(aligned[0].h, aligned[1].h);
        assert!(aligned[0].h >= 67);
        // Same xofs, 31 vs 32: BUILD only differs by the half-texel of
        // the odd width — the flame column stays put.
        let left_a = tile_left_x(&a);
        let left_b = tile_left_x(&b);
        assert_eq!(left_a - left_b, 1);
    }

    #[test]
    fn orphan_fire_strip_becomes_one_sheet_when_placed() {
        let (staged, pack) = test_dirs("strip");
        let mut art = ArtBank::default();
        for id in 2066u16..=2079 {
            art.tiles.insert(id, flat_tile(24, 32));
        }
        let used: BTreeSet<u16> = [2066u16].into_iter().collect();
        let mut assets = Vec::new();
        assemble_duke_billboards(&mut assets, &staged, &pack, "duke3d", &art, &used);
        let sheets: Vec<_> = assets
            .iter()
            .filter(|a| a.rel_path.ends_with(".billboard"))
            .collect();
        assert_eq!(sheets.len(), 1, "{assets:?}");
        assert_eq!(sheets[0].key, "billboards/duke3d/strip-2066");
        assert!(
            !assets.iter().any(|a| a.key.contains("tile-206")),
            "fire frames must not be cards: {assets:?}"
        );
        let text =
            std::fs::read_to_string(staged.join(&sheets[0].rel_path)).unwrap();
        let bb = crate::stateful_billboard::StatefulBillboard::parse(&text).unwrap();
        assert_eq!(bb.frames.len(), 14);
        assert!(bb.states.iter().any(|s| s.name == "idle" && s.r#loop));
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn unplaced_irregular_strip_becomes_one_sheet() {
        let (staged, pack) = test_dirs("orphan");
        let mut art = ArtBank::default();
        for id in 2066u16..=2079 {
            art.tiles.insert(id, flat_tile(24, 32));
        }
        let mut assets = Vec::new();
        assemble_duke_billboards(
            &mut assets,
            &staged,
            &pack,
            "duke3d",
            &art,
            &BTreeSet::new(),
        );
        let sheets: Vec<_> = assets
            .iter()
            .filter(|a| a.rel_path.ends_with(".billboard"))
            .collect();
        assert_eq!(sheets.len(), 1, "{assets:?}");
        assert_eq!(sheets[0].key, "billboards/duke3d/strip-2066");
        assert!(
            !assets.iter().any(|a| a.key.contains("tile-206")),
            "fire frames must not be cards: {assets:?}"
        );
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn unplaced_named_texture_run_stays_out() {
        let (staged, pack) = test_dirs("namedtex");
        std::fs::write(pack.join("DEFS.CON"), "define BRICK 3000\n").unwrap();
        let mut art = ArtBank::default();
        for id in 3000u16..=3007 {
            art.tiles.insert(id, flat_tile(64, 64));
        }
        let mut assets = Vec::new();
        assemble_duke_billboards(
            &mut assets,
            &staged,
            &pack,
            "duke3d",
            &art,
            &BTreeSet::new(),
        );
        assert!(
            assets.is_empty(),
            "CON-named 64x64 walls must not become sheets: {assets:?}"
        );
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn unplaced_world_texture_run_stays_out() {
        let (staged, pack) = test_dirs("texrun");
        let mut art = ArtBank::default();
        for id in 3000u16..=3013 {
            art.tiles.insert(id, flat_tile(64, 64));
        }
        let mut assets = Vec::new();
        assemble_duke_billboards(
            &mut assets,
            &staged,
            &pack,
            "duke3d",
            &art,
            &BTreeSet::new(),
        );
        assert!(assets.is_empty(), "64x64 walls must not become sheets: {assets:?}");
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn placed_singleton_lands_as_leftover_card() {
        let (staged, pack) = test_dirs("single");
        let mut art = ArtBank::default();
        art.tiles.insert(1234, flat_tile(31, 27));
        // Unplaced neighbor of a different size — not a run, not used.
        art.tiles.insert(1235, flat_tile(64, 128));
        let used: BTreeSet<u16> = [1234u16].into_iter().collect();
        let mut assets = Vec::new();
        assemble_duke_billboards(&mut assets, &staged, &pack, "duke3d", &art, &used);
        assert_eq!(assets.len(), 1, "{assets:?}");
        assert_eq!(assets[0].key, "billboards/duke3d/tile-1234");
        assert!(assets[0].tags.iter().any(|t| t == "leftover"));
        assert!(staged
            .join("billboards/duke3d/tile-1234.png")
            .is_file());
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn font_range_becomes_one_sheet_not_letter_cards() {
        let (staged, pack) = test_dirs("font");
        std::fs::write(
            pack.join("DEFS.CON"),
            "define STARTALPHANUM 100\ndefine ENDALPHANUM 125\ndefine MINIFONT 300\n",
        )
        .unwrap();
        let mut art = ArtBank::default();
        for id in 100u16..=125 {
            art.tiles.insert(id, flat_tile(8 + u32::from(id % 5), 12));
        }
        for id in 300u16..=340 {
            art.tiles.insert(id, flat_tile(4 + u32::from(id % 3), 6));
        }
        // A map placing one glyph must not re-land it as a card.
        let used: BTreeSet<u16> = [105u16].into_iter().collect();
        let mut assets = Vec::new();
        assemble_duke_billboards(&mut assets, &staged, &pack, "duke3d", &art, &used);
        let keys: Vec<&str> = assets.iter().map(|a| a.key.as_str()).collect();
        assert!(
            keys.contains(&"billboards/duke3d/startalphanum"),
            "{keys:?}"
        );
        assert!(keys.contains(&"billboards/duke3d/minifont"), "{keys:?}");
        assert_eq!(assets.len(), 2, "no per-letter cards: {keys:?}");
        assert!(assets
            .iter()
            .all(|a| a.tags.iter().any(|t| t == "font")));
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn actor_consumes_walk_tiles_and_place_points_at_sheet() {
        let (staged, pack) = test_dirs("actor");
        std::fs::write(
            pack.join("GAME.CON"),
            r#"
define PIGCOP 2000
action APIGSTAND 30 1 5 1 1
action APIGWALK 0 4 5 1 20
action APIGSHOOT 30 2 5 1 58
action APIGDYING 55 5 1 1 15
ai AIPIGSEEKENEMY APIGWALK PIGWALKVELS seekplayer
actor PIGCOP 30 APIGSTAND
    ifai AIPIGSEEKENEMY
    action APIGSHOOT
    action APIGDYING
enda
"#,
        )
        .unwrap();
        let mut art = ArtBank::default();
        for id in 2000u16..=2070 {
            art.tiles.insert(id, flat_tile(40, 60));
        }
        std::fs::create_dir_all(staged.join("worlds")).unwrap();
        let place = crate::world_place::WorldPlace {
            source: "duke3d".into(),
            world: "worlds/e1l1".into(),
            spawn: None,
            places: vec![crate::world_place::Place {
                id: "spr-0".into(),
                kind: "billboard".into(),
                asset: "billboards/duke3d/tile-2000".into(),
                pos: [1.0, 0.5, 2.0],
                yaw: 0.0,
                class: "2000".into(),
                width: 0.6,
                height: 1.2,
                align: "face".into(),
            }],
        };
        std::fs::write(staged.join("worlds/e1l1.place"), place.to_text()).unwrap();
        let used: BTreeSet<u16> = [2000u16].into_iter().collect();
        let mut assets = Vec::new();
        assemble_duke_billboards(&mut assets, &staged, &pack, "duke3d", &art, &used);
        assert!(
            assets.iter().any(|a| a.key == "billboards/duke3d/pigcop"),
            "{assets:?}"
        );
        assert!(
            !assets.iter().any(|a| a.key.starts_with("billboards/duke3d/tile-20")),
            "walk tiles must be inside the sheet: {assets:?}"
        );
        let text = std::fs::read_to_string(staged.join("worlds/e1l1.place")).unwrap();
        let rewritten = crate::world_place::WorldPlace::parse(&text).unwrap();
        assert_eq!(rewritten.places[0].asset, "billboards/duke3d/pigcop");
        let _ = std::fs::remove_dir_all(staged.parent().unwrap());
    }

    #[test]
    fn map_v7_one_room_emits_glb() {
        let mut map = vec![0u8; 26 + 40 + 32 * 4 + 2];
        map[0..4].copy_from_slice(&7i32.to_le_bytes());
        // 1 sector
        map[20..22].copy_from_slice(&1u16.to_le_bytes());
        let so = 22;
        map[so..so + 2].copy_from_slice(&0u16.to_le_bytes()); // wallptr
        map[so + 2..so + 4].copy_from_slice(&4u16.to_le_bytes());
        map[so + 4..so + 8].copy_from_slice(&0i32.to_le_bytes()); // ceilingz
        map[so + 8..so + 12].copy_from_slice(&8192i32.to_le_bytes()); // floorz
        map[so + 16..so + 18].copy_from_slice(&1i16.to_le_bytes());
        map[so + 24..so + 26].copy_from_slice(&2i16.to_le_bytes());
        let wo = so + 40;
        map[wo..wo + 2].copy_from_slice(&4u16.to_le_bytes());
        let corners: [(i32, i32); 4] = [(0, 0), (2048, 0), (2048, 2048), (0, 2048)];
        for (i, (x, y)) in corners.into_iter().enumerate() {
            let o = wo + 2 + i * 32;
            map[o..o + 4].copy_from_slice(&x.to_le_bytes());
            map[o + 4..o + 8].copy_from_slice(&y.to_le_bytes());
            map[o + 8..o + 10].copy_from_slice(&(((i + 1) % 4) as i16).to_le_bytes());
            map[o + 12..o + 14].copy_from_slice(&(-1i16).to_le_bytes());
            map[o + 16..o + 18].copy_from_slice(&3i16.to_le_bytes());
            map[o + 22] = 8;
            map[o + 23] = 8;
        }
        let parsed = parse_map_v7(&map).expect("map");
        assert_eq!(parsed.sectors.len(), 1);
        assert_eq!(parsed.walls.len(), 4);
        let mut art = ArtBank::default();
        art.tiles.insert(
            3,
            TileRgba {
                w: 16,
                h: 16,
                rgba: vec![200; 16 * 16 * 4],
                xoff: 0,
                yoff: 0,
            },
        );
        let (glb, spawn, _) = map_to_glb(&parsed, &art).expect("glb");
        assert!(glb.starts_with(b"glTF"));
        assert!(spawn.is_some());
    }

    #[test]
    fn wall_v_uses_finer_z() {
        // One story (8192 Z), yrepeat 8, 64px tile → 0.5 tile (2048 scale).
        let v = wall_v(8192.0, 0.0, 8.0, 64.0);
        assert!((v - 0.5).abs() < 0.01, "v={v}");
        let v0 = wall_v(0.0, 0.0, 8.0, 64.0);
        assert!(v0.abs() < 0.01, "v0={v0}");
    }

    #[test]
    fn plane_uv_is_world_scaled() {
        let sec = Sector {
            wallptr: 0,
            wallnum: 4,
            ceilingz: 0,
            floorz: 8192,
            ceilingstat: 0,
            floorstat: 0,
            ceilingpicnum: 1,
            ceilingheinum: 0,
            ceilingshade: 0,
            ceilingxpanning: 0,
            ceilingypanning: 0,
            floorpicnum: 1,
            floorheinum: 0,
            floorshade: 0,
            floorxpanning: 0,
            floorypanning: 0,
            lotag: 0,
            hitag: 0,
        };
        let map = BuildMap {
            start: [0, 0, 0],
            start_ang: 0,
            start_sec: 0,
            sectors: vec![],
            walls: vec![],
            sprites: vec![],
        };
        // 1024 map units / 16 / 64 = 1 tile.
        let uv = plane_uv(&sec, &map, 1024.0, 0.0, false, 64.0, 64.0);
        assert!((uv[0] - 1.0).abs() < 0.01, "u={}", uv[0]);
        let uv = plane_uv(&sec, &map, 0.0, 1024.0, false, 64.0, 64.0);
        assert!((uv[1] - 1.0).abs() < 0.01, "v={}", uv[1]);
    }

    #[test]
    fn plane_uv_relative_cancels_authored_vflip() {
        let sec = Sector {
            wallptr: 0,
            wallnum: 2,
            ceilingz: 0,
            floorz: 8192,
            ceilingstat: 0,
            floorstat: 64 | 32,
            ceilingpicnum: 1,
            ceilingheinum: 0,
            ceilingshade: 0,
            ceilingxpanning: 0,
            ceilingypanning: 0,
            floorpicnum: 1,
            floorheinum: 0,
            floorshade: 0,
            floorxpanning: 0,
            floorypanning: 0,
            lotag: 0,
            hitag: 0,
        };
        let map = BuildMap {
            start: [0, 0, 0],
            start_ang: 0,
            start_sec: 0,
            sectors: vec![],
            walls: vec![
                Wall {
                    x: 0,
                    y: 0,
                    point2: 1,
                    nextwall: -1,
                    nextsector: -1,
                    cstat: 0,
                    picnum: 1,
                    overpicnum: 0,
                    shade: 0,
                    xrepeat: 8,
                    yrepeat: 8,
                    xpanning: 0,
                    ypanning: 0,
                },
                Wall {
                    x: 1024,
                    y: 0,
                    point2: 0,
                    nextwall: -1,
                    nextsector: -1,
                    cstat: 0,
                    picnum: 1,
                    overpicnum: 0,
                    shade: 0,
                    xrepeat: 8,
                    yrepeat: 8,
                    xpanning: 0,
                    ypanning: 0,
                },
            ],
            sprites: vec![],
        };
        // Relative + authored v-flip XOR each other, so V is +perp.
        let uv = plane_uv(&sec, &map, 0.0, 1024.0, false, 64.0, 64.0);
        assert!((uv[1] - 1.0).abs() < 0.05, "v={}", uv[1]);
    }

    #[test]
    fn hole_is_cut_from_outer_floor() {
        let outer = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let hole = vec![vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]]];
        let mut kept = Vec::new();
        for [ia, ib, ic] in ear_clip(&outer) {
            kept.extend(subtract_holes(
                [outer[ia], outer[ib], outer[ic]],
                &hole,
            ));
        }
        let inside = [2.0, 2.0];
        for [a, b, c] in &kept {
            assert!(
                !point_in_tri(inside, *a, *b, *c),
                "triangle filled the hole"
            );
        }
        assert!(!kept.is_empty(), "outer ring vanished");
        let rim = [0.5, 0.5];
        assert!(
            kept.iter().any(|[a, b, c]| point_in_tri(rim, *a, *b, *c)),
            "floor around the hole was dropped"
        );
    }

    #[test]
    fn wall_sprite_width_runs_perpendicular_to_ang() {
        let s = Sprite {
            x: 0,
            y: 0,
            z: 0,
            picnum: 1,
            cstat: 16,
            shade: 0,
            pal: 0,
            xrepeat: 4,
            yrepeat: 4,
            sectnum: 0,
            ang: 0,
            lotag: 0,
            hitag: 0,
        };
        let mut sink = MeshSink::default();
        // tile width is baked into `width`; ang 0 faces +X so the quad
        // should run along −Y / +Y, not along X.
        emit_upright_sprite(&mut sink, &s, 100.0, 80.0);
        let mesh = sink.level.get(&(1, 0)).expect("sprite bucket");
        let xs: Vec<f32> = mesh.positions.iter().map(|p| p[0] / SCALE).collect();
        let zs: Vec<f32> = mesh.positions.iter().map(|p| -p[2] / SCALE).collect();
        let x_span = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        let y_span = zs.iter().cloned().fold(f32::MIN, f32::max)
            - zs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(x_span < 2.0, "x_span={x_span}");
        assert!((y_span - 100.0).abs() < 2.0, "y_span={y_span}");
    }

    #[test]
    fn shade_mul_darkens_positive() {
        assert!((shade_mul(0) - 1.0).abs() < 0.01);
        assert!((shade_mul(16) - 0.5).abs() < 0.01);
        assert!(shade_mul(12) < 0.7);
    }

    #[test]
    fn slope_z_heinum_4096_is_45_deg_world() {
        // First wall along +X. Left (interior) is +Y.
        let sec = Sector {
            wallptr: 0,
            wallnum: 2,
            ceilingz: 0,
            floorz: 0,
            ceilingstat: 0,
            floorstat: 2,
            ceilingpicnum: 1,
            ceilingheinum: 0,
            ceilingshade: 0,
            ceilingxpanning: 0,
            ceilingypanning: 0,
            floorpicnum: 1,
            floorheinum: 4096,
            floorshade: 0,
            floorxpanning: 0,
            floorypanning: 0,
            lotag: 0,
            hitag: 0,
        };
        let map = BuildMap {
            start: [0, 0, 0],
            start_ang: 0,
            start_sec: 0,
            sectors: vec![],
            walls: vec![
                Wall {
                    x: 0,
                    y: 0,
                    point2: 1,
                    nextwall: -1,
                    nextsector: -1,
                    cstat: 0,
                    picnum: 1,
                    overpicnum: 0,
                    shade: 0,
                    xrepeat: 8,
                    yrepeat: 8,
                    xpanning: 0,
                    ypanning: 0,
                },
                Wall {
                    x: 1024,
                    y: 0,
                    point2: 0,
                    nextwall: -1,
                    nextsector: -1,
                    cstat: 0,
                    picnum: 1,
                    overpicnum: 0,
                    shade: 0,
                    xrepeat: 8,
                    yrepeat: 8,
                    xpanning: 0,
                    ypanning: 0,
                },
            ],
            sprites: vec![],
        };
        // 256 XY * heinum 4096 / 256 = 4096 Z. World ΔX=0.5, ΔY=0.5.
        let y0 = slope_z(&sec, &map, 0.0, 0.0, false);
        let y1 = slope_z(&sec, &map, 0.0, 256.0, false);
        let dy = y1 - y0;
        let dx = 256.0 * SCALE;
        assert!((dy.abs() - dx).abs() < 0.01, "dy={dy} dx={dx} y0={y0} y1={y1}");
    }

    #[test]
    fn ear_clip_covers_concave_without_fan() {
        // C shape: fan from (0,0) would fill the notch.
        let pts = [
            [0.0, 0.0],
            [3.0, 0.0],
            [3.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [3.0, 2.0],
            [3.0, 3.0],
            [0.0, 3.0],
        ];
        let tris = ear_clip(&pts);
        assert!(tris.len() >= 6);
        let notch = [2.0, 1.5];
        for [ia, ib, ic] in &tris {
            assert!(
                !point_in_tri(notch, pts[*ia], pts[*ib], pts[*ic]),
                "triangle filled the notch"
            );
        }
    }

    #[test]
    fn art_column_major_decodes() {
        let mut art = vec![0u8; 16 + 8 + 4];
        art[0..4].copy_from_slice(&1i32.to_le_bytes());
        art[8..12].copy_from_slice(&10i32.to_le_bytes());
        art[12..16].copy_from_slice(&10i32.to_le_bytes());
        art[16..18].copy_from_slice(&2u16.to_le_bytes());
        art[18..20].copy_from_slice(&2u16.to_le_bytes());
        art[24] = 1;
        art[25] = 2;
        art[26] = 3;
        art[27] = 4;
        let mut tiles = BTreeMap::new();
        let pal = {
            let mut p = [[0u8; 3]; 256];
            p[1] = [255, 0, 0];
            p[2] = [0, 255, 0];
            p[3] = [0, 0, 255];
            p[4] = [255, 255, 0];
            p
        };
        parse_art_into(&art, &pal, &mut tiles).unwrap();
        let t = tiles.get(&10).expect("tile");
        assert_eq!((t.w, t.h), (2, 2));
        // column-major: (0,0)=1 red, (0,1)=2 green, (1,0)=3 blue, (1,1)=4 yellow
        assert_eq!(&t.rgba[0..3], &[255, 0, 0]);
        assert_eq!(&t.rgba[8..11], &[0, 255, 0]);
    }

    #[test]
    fn e1l1_start_holes_skip_adjacent_walkway() {
        let grp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/duke3d/DUKE3D.GRP");
        if !grp.is_file() {
            return;
        }
        let bytes = std::fs::read(&grp).unwrap();
        let files = parse_grp(&bytes).unwrap();
        let map_bytes = files
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("E1L1.MAP"))
            .map(|(_, d)| d.clone())
            .expect("E1L1.MAP");
        let map = parse_map_v7(&map_bytes).expect("map");
        assert!(map.sectors.len() > 309);
        let sec = &map.sectors[309];
        let mut loops = sector_loops(&map, sec);
        loops.sort_by(|a, b| {
            ring_area_ids(&map, b)
                .abs()
                .partial_cmp(&ring_area_ids(&map, a).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let outer = unique_ring(
            loops[0]
                .iter()
                .filter_map(|&i| map.walls.get(i).map(world_xy))
                .collect(),
        );
        let holes = interior_holes(&map, 309, &outer);
        let in_hole = |x: f32, y: f32| holes.iter().any(|h| point_in_poly([x, y], h));
        // Metal diamond + darker wood wrap are inner children.
        assert!(in_hole(-28943.0, 9433.0), "234 not cut from 309");
        assert!(in_hole(-29288.0, 10388.0), "235 not cut from 309");
        assert!(in_hole(-29853.0, 9245.0), "236 not cut from 309");
        // Adjacent walkway 271 must stay part of the wood floor.
        assert!(!in_hole(-26388.0, 12556.0), "271 wrongly cut from 309");
    }

    #[test]
    fn shareware_e1l1_converts_if_present() {
        let grp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/duke3d/DUKE3D.GRP");
        if !grp.is_file() {
            return;
        }
        let bytes = std::fs::read(&grp).unwrap();
        let files = parse_grp(&bytes).unwrap();
        let map_bytes = files
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("E1L1.MAP"))
            .map(|(_, d)| d.clone())
            .expect("E1L1.MAP");
        let parsed = parse_map_v7(&map_bytes).expect("map");
        assert!(parsed.sectors.len() > 10);
        let tmp = std::env::temp_dir().join("duke-e1l1-mesh-test");
        let _ = std::fs::create_dir_all(&tmp);
        let mut extracted = Vec::new();
        for (name, data) in &files {
            let dest = tmp.join(name);
            let _ = std::fs::write(&dest, data);
            extracted.push(dest);
        }
        let art = load_tileset(&extracted, &tmp);
        let world = map_to_world(&parsed, &art).expect("world");
        let glb = world.glb.clone();
        let spawn = world.spawn;
        assert!(glb.starts_with(b"glTF"));
        assert!(spawn.is_some());
        // The contract, on a real Build level.
        {
            use makepad_asset_client::json::Value;
            let root = glb_json(&glb);
            let names: Vec<String> = root
                .get("nodes")
                .and_then(Value::as_arr)
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let count = |p: &str| names.iter().filter(|n| n.starts_with(p)).count();
            let nav = map_nav(&parsed, spawn.unwrap());
            eprintln!(
                "e1l1 contract: {} doors, {} lifts, {} hazards, sky={}, \
                 {} markers, {} teleports, {} starts, eye={:?}",
                count("door_"),
                count("lift_"),
                count("hazard_"),
                names.iter().any(|n| n == "sky"),
                nav.markers.len(),
                nav.teleports.len(),
                nav.starts.len(),
                nav.eye_height,
            );
            assert!(count("door_") > 0, "E1L1 has doors: {names:?}");
            assert!(count("lift_") > 0, "E1L1 has elevators: {names:?}");
            assert!(names.iter().any(|n| n == "sky"), "E1L1 is outdoors: {names:?}");
            assert!(
                world.doors.len() == count("door_") && world.lifts.len() == count("lift_"),
                "the sidecar must name exactly the nodes the GLB has"
            );
            // Every animated part must actually carry geometry, or the level
            // has a hole where the door used to be.
            let meshes = root.get("meshes").and_then(Value::as_arr).unwrap();
            for n in names.iter().filter(|n| n.starts_with("door_") || n.starts_with("lift_")) {
                let mesh = meshes
                    .iter()
                    .find(|m| m.get("name").and_then(Value::as_str) == Some(n.as_str()))
                    .unwrap_or_else(|| panic!("{n} has no mesh"));
                let prims = mesh.get("primitives").and_then(Value::as_arr).unwrap();
                assert!(!prims.is_empty(), "{n} carries nothing");
            }
        }
        // Ear-clip + tile splits, not a million-vert dump.
        assert!(glb.len() > 8_000, "glb too small: {}", glb.len());
        assert!(glb.len() < 8_000_000, "glb too large: {}", glb.len());
        eprintln!("e1l1 glb {} bytes", glb.len());
        // BUILD cuts a sector's floor and its neighbour's wall at different
        // points, so E1L1 arrives full of T-junctions; the weld must close
        // every one across every texture bucket.
        let parts = crate::world_preview::extract_glb_parts(&glb).expect("parts");
        let soup: Vec<(&[[f32; 3]], &[u32])> = parts
            .iter()
            .map(|part| (&part.pos[..], &part.indices[..]))
            .collect();
        let left = crate::classic_import::weld_t_junctions_left(&soup);
        eprintln!(
            "e1l1: {} parts, {} triangles, {left} T-junctions",
            parts.len(),
            soup.iter().map(|(_, i)| i.len() / 3).sum::<usize>()
        );
        // Three defects survive, counted from both sides of a shared edge:
        // each is a pair of corners one to three BUILD units apart (a unit
        // is 1/512 m) where cutting one edge puts the cut inside the chord
        // the other needs, so the pass declines rather than grow the mesh
        // forever. Closing them means MOVING a vertex, which this pass never
        // does. The number may only fall.
        assert!(
            left <= 5,
            "Duke E1L1 T-junctions rose to {left} (ratchet 5) — surfaces crack where they meet"
        );
        let dest = tmp.join("e1l1.glb");
        std::fs::write(&dest, &glb).unwrap();
        if let Some(s) = spawn {
            let text = format!(
                "world-spawn 1\n{:.4} {:.4} {:.4}\n{:.5} {:.5}\n",
                s[0], s[1], s[2], s[3], s[4]
            );
            let _ = std::fs::write(dest.with_extension("spawn"), text);
        }
        match crate::world_preview::write_spawn_preview(&dest) {
            Ok(()) => eprintln!("preview ok {}", dest.with_extension("png").display()),
            Err(e) => eprintln!("preview failed: {e}"),
        }
    }

    fn square_sec(wallptr: u16, x0: i32, y0: i32, size: i32, ceil_stat: i16, ceil_pic: i16) -> (Sector, [Wall; 4]) {
        let walls = [
            Wall {
                x: x0,
                y: y0,
                point2: (wallptr + 1) as i16,
                nextwall: -1,
                nextsector: -1,
                cstat: 0,
                picnum: 1,
                overpicnum: 0,
                shade: 0,
                xrepeat: 8,
                yrepeat: 8,
                xpanning: 0,
                ypanning: 0,
            },
            Wall {
                x: x0 + size,
                y: y0,
                point2: (wallptr + 2) as i16,
                nextwall: -1,
                nextsector: -1,
                cstat: 0,
                picnum: 1,
                overpicnum: 0,
                shade: 0,
                xrepeat: 8,
                yrepeat: 8,
                xpanning: 0,
                ypanning: 0,
            },
            Wall {
                x: x0 + size,
                y: y0 + size,
                point2: (wallptr + 3) as i16,
                nextwall: -1,
                nextsector: -1,
                cstat: 0,
                picnum: 1,
                overpicnum: 0,
                shade: 0,
                xrepeat: 8,
                yrepeat: 8,
                xpanning: 0,
                ypanning: 0,
            },
            Wall {
                x: x0,
                y: y0 + size,
                point2: wallptr as i16,
                nextwall: -1,
                nextsector: -1,
                cstat: 0,
                picnum: 1,
                overpicnum: 0,
                shade: 0,
                xrepeat: 8,
                yrepeat: 8,
                xpanning: 0,
                ypanning: 0,
            },
        ];
        let sec = Sector {
            wallptr,
            wallnum: 4,
            ceilingz: 0,
            floorz: 8192,
            ceilingstat: ceil_stat,
            floorstat: 0,
            ceilingpicnum: ceil_pic,
            ceilingheinum: 0,
            ceilingshade: 0,
            ceilingxpanning: 0,
            ceilingypanning: 0,
            floorpicnum: 2,
            floorheinum: 0,
            floorshade: 0,
            floorxpanning: 0,
            floorypanning: 0,
            lotag: 0,
            hitag: 0,
        };
        (sec, walls)
    }

    #[test]
    fn parallax_sky_is_local_and_one_wrap() {
        let (s0, w0) = square_sec(0, -1024, -1024, 2048, 1, 89);
        let (s1, w1) = square_sec(4, 40_000, 40_000, 2048, 0, 3);
        let mut walls = Vec::new();
        walls.extend(w0);
        walls.extend(w1);
        let map = BuildMap {
            start: [0, 0, 4096],
            start_ang: 0,
            start_sec: 0,
            sectors: vec![s0, s1],
            walls,
            sprites: vec![],
        };
        let mut art = ArtBank::default();
        art.tiles.insert(
            89,
            TileRgba {
                w: 128,
                h: 300,
                rgba: {
                    let mut p = vec![0u8; 128 * 300 * 4];
                    for px in p.chunks_exact_mut(4) {
                        px[0] = 40;
                        px[1] = 60;
                        px[2] = 120;
                        px[3] = 255;
                    }
                    p
                },
                xoff: 0,
                yoff: 0,
            },
        );
        let sky = emit_parallax_sky(&map, &art, &TileRgba {
            w: 16,
            h: 16,
            rgba: vec![0x90; 16 * 16 * 4],
            xoff: 0,
            yoff: 0,
        })
        .expect("sky");
        let max_u = sky.uvs.iter().map(|uv| uv[0]).fold(0.0f32, f32::max);
        assert!(
            (max_u - 1.0).abs() < 0.01,
            "atlas should wrap once around, max u={max_u}"
        );
        let sx = 0.0;
        let sz = 0.0;
        let mut max_xz = 0.0f32;
        for p in &sky.positions {
            let d = (p[0] - sx).hypot(p[2] - sz);
            max_xz = max_xz.max(d);
        }
        let cap = SKY_MAX_R * SCALE + 0.5;
        assert!(
            max_xz < cap,
            "sky hull {max_xz} must stay near start, not the far indoor wing"
        );
        let y_min = sky.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let y_max = sky.positions.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        let y_eye = -map.start[2] as f32 * SCALE_Z;
        assert!(
            y_min <= y_eye && y_eye <= y_max,
            "eye {y_eye} must sit inside sky y {y_min}..{y_max}"
        );
        // Eight 128-wide panels of the 300-tall city tile — asserted on the
        // decoded image, not on byte length (the PNG writer compresses).
        let (rgba, aw, ah) = crate::classic_import::decode_png_stored(&sky.png).expect("sky png");
        assert_eq!((aw, ah), (8 * 128, 300), "atlas should be the 8-panel strip");
        assert_eq!(
            &rgba[0..4],
            &[40, 60, 120, 255],
            "sky should carry the city tile, not the gray stub"
        );
    }

    /// Keyhole outer rings (a notch joined through a duplicate vertex, e.g.
    /// the E1L1 wood roofs with grate slits) used to stall ear_clip, which
    /// silently dropped the rest of the polygon and left slit holes in the
    /// mesh. Every non-paper, non-parallax plane must triangulate to its
    /// polygon area minus its interior holes.
    #[test]
    fn e1_maps_planes_cover_their_polygons() {
        let grp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/duke3d/DUKE3D.GRP");
        if !grp.is_file() {
            return;
        }
        let bytes = std::fs::read(&grp).unwrap();
        let files = parse_grp(&bytes).unwrap();
        let tri_area = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
            (corner_cross(a, b, c) as f32).abs() * 0.5
        };
        for (name, data) in &files {
            let upper = name.to_ascii_uppercase();
            if !upper.starts_with("E1L") || !upper.ends_with(".MAP") {
                continue;
            }
            let map = parse_map_v7(data).expect(name);
            for si in 0..map.sectors.len() {
                let sec = &map.sectors[si];
                if (sec.floorz - sec.ceilingz).abs() < 64 {
                    continue;
                }
                if sec.floorstat & 1 != 0 && sec.ceilingstat & 1 != 0 {
                    continue;
                }
                let mut loops = sector_loops(&map, sec);
                if loops.is_empty() {
                    continue;
                }
                loops.sort_by(|a, b| {
                    ring_area_ids(&map, b)
                        .abs()
                        .partial_cmp(&ring_area_ids(&map, a).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let outer = unique_ring(
                    loops[0]
                        .iter()
                        .filter_map(|&i| map.walls.get(i).map(world_xy))
                        .collect(),
                );
                if outer.len() < 3 {
                    continue;
                }
                let holes = interior_holes(&map, si, &outer);
                let outer_area = signed_area(&outer).abs();
                let hole_area: f32 = holes.iter().map(|h| signed_area(h).abs()).sum();
                let mut emitted = 0.0f32;
                for [ia, ib, ic] in ear_clip(&outer) {
                    for [pa, pb, pc] in
                        subtract_holes([outer[ia], outer[ib], outer[ic]], &holes)
                    {
                        emitted += tri_area(pa, pb, pc);
                    }
                }
                let expect = outer_area - hole_area;
                let miss = expect - emitted;
                let tol = outer_area.max(1.0) * 0.002 + 2048.0;
                assert!(
                    miss.abs() <= tol,
                    "{name} sector {si} (fpic {}) plane area expect {expect:.0} emitted {emitted:.0} miss {miss:.0} tol {tol:.0}",
                    sec.floorpicnum
                );
            }
        }
    }

    /// Literal E1L1 outer rings that reach a notch through a duplicated
    /// vertex (keyhole). ear_clip must still cover the full polygon area.
    #[test]
    fn ear_clip_covers_keyhole_rings() {
        let ring_area = |pts: &[[f32; 2]]| -> f64 {
            ear_clip(pts)
                .iter()
                .map(|&[a, b, c]| corner_cross(pts[a], pts[b], pts[c]).abs() * 0.5)
                .sum()
        };
        // E1L1 sector 117: square with a spike channel on one edge.
        let s117 = [
            [18176.0, 57728.0],
            [18176.0, 56704.0],
            [18688.0, 56704.0],
            [18656.0, 56704.0],
            [18688.0, 56704.0],
            [18688.0, 57728.0],
        ];
        let expect = signed_area(&s117).abs() as f64;
        let got = ring_area(&s117);
        assert!(
            (got - expect).abs() < expect * 0.002 + 1024.0,
            "s117 area {got} want {expect}"
        );
        // E1L1 sector 93: rectangle with two keyhole notches; the old
        // ear_clip emitted zero triangles here.
        let s93 = [
            [26368.0, 45056.0],
            [26368.0, 46080.0],
            [25760.0, 46080.0],
            [25760.0, 45568.0],
            [25728.0, 45620.0],
            [25728.0, 46048.0],
            [25760.0, 46080.0],
            [25600.0, 46080.0],
            [25600.0, 45056.0],
            [25760.0, 45056.0],
            [25728.0, 45088.0],
            [25728.0, 45516.0],
            [25760.0, 45566.0],
            [25760.0, 45056.0],
        ];
        let expect = signed_area(&s93).abs() as f64;
        let got = ring_area(&s93);
        assert!(got > 0.0, "s93 emitted nothing");
        assert!(
            (got - expect).abs() < expect * 0.002 + 2048.0,
            "s93 area {got} want {expect}"
        );
        // Convex ring stays exact.
        let quad = [[0.0, 0.0], [512.0, 0.0], [512.0, 512.0], [0.0, 512.0]];
        assert_eq!(ear_clip(&quad).len(), 2);
    }

    #[test]
    #[ignore]
    fn reconvert_local_e1_maps_if_present() {
        let grp = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/duke3d/DUKE3D.GRP");
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/ai_content_app/import/duke3d/work/source");
        if !grp.is_file() || !staged.is_dir() {
            return;
        }
        let bytes = std::fs::read(&grp).unwrap();
        let files = parse_grp(&bytes).unwrap();
        let tmp = std::env::temp_dir().join("duke-reconvert-art");
        let _ = std::fs::create_dir_all(&tmp);
        let mut extracted = Vec::new();
        for (name, data) in &files {
            let dest = tmp.join(name);
            let _ = std::fs::write(&dest, data);
            extracted.push(dest);
        }
        let art = load_tileset(&extracted, &tmp);
        for (name, data) in &files {
            let upper = name.to_ascii_uppercase();
            if !upper.starts_with("E1L") || !upper.ends_with(".MAP") {
                continue;
            }
            let parsed = parse_map_v7(data).expect(name);
            let mut used = BTreeSet::new();
            convert_map(&tmp.join(name), name, &staged, &art, "duke3d", &mut used).expect(name);
            eprintln!("reconverted {name} sectors={}", parsed.sectors.len());
        }
    }

    /// Full shareware reconvert into the app's work dir. Asserts the
    /// grouping contract: no per-frame tile cards, one sheet per strip.
    #[test]
    #[ignore]
    fn reconvert_local_duke_pack_if_present() {
        let pack_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/packs/duke3d");
        if !pack_dir.join("DUKE3D.GRP").is_file() {
            return;
        }
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../local/ai_content_app/import/duke3d/work/source");
        let report = crate::classic_import::convert_classic(
            &pack_dir,
            &staged,
            crate::classic_import::ClassicSource::Duke3d,
        )
        .expect("convert");
        let mut worlds = 0usize;
        let mut sheets = 0usize;
        let mut leftovers = 0usize;
        let mut audio = 0usize;
        for a in &report.assets {
            match a.kind {
                AssetKind::World => worlds += 1,
                AssetKind::Audio => audio += 1,
                AssetKind::Billboard => {
                    if a.rel_path.ends_with(".billboard") {
                        sheets += 1;
                        eprintln!("sheet    {}", a.key);
                    } else {
                        leftovers += 1;
                        assert!(
                            a.tags.iter().any(|t| t == "leftover"),
                            "PNG billboard without leftover tag: {}",
                            a.key
                        );
                    }
                }
                _ => {}
            }
        }
        eprintln!(
            "worlds={worlds} sheets={sheets} leftovers={leftovers} audio={audio} total={}",
            report.assets.len()
        );
        assert_eq!(worlds, 6, "E1L1..E1L6");
        let has = |k: &str| report.assets.iter().any(|a| a.key == format!("billboards/duke3d/{k}"));
        assert!(has("pigcop"), "pig cop actor sheet");
        assert!(has("liztroop"), "liztroop actor sheet");
        // Pig walk tiles live only inside the sheet, never as cards.
        for pic in [2000u16, 2001, 2016, 2031] {
            assert!(
                !report
                    .assets
                    .iter()
                    .any(|a| a.key == format!("billboards/duke3d/tile-{pic}")),
                "tile-{pic} must be consumed by pigcop"
            );
        }
        // Fonts group into single sheets, not per-letter cards.
        assert!(
            report.assets.iter().any(|a| a.tags.iter().any(|t| t == "font")),
            "at least one font sheet"
        );
        assert!(
            leftovers < 120,
            "leftover singles exploded: {leftovers} — grouping regressed"
        );
    }

    // -----------------------------------------------------------------
    // The unified map contract
    // -----------------------------------------------------------------

    fn glb_json(glb: &[u8]) -> makepad_asset_client::json::Value {
        let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        makepad_asset_client::json::parse(&glb[20..20 + len]).expect("glb json")
    }

    fn node_named<'a>(
        root: &'a makepad_asset_client::json::Value,
        name: &str,
    ) -> Option<&'a makepad_asset_client::json::Value> {
        use makepad_asset_client::json::Value;
        root.get("nodes")?
            .as_arr()?
            .iter()
            .find(|n| n.get("name").and_then(Value::as_str) == Some(name))
    }

    fn num(v: &makepad_asset_client::json::Value) -> f64 {
        use makepad_asset_client::json::Value;
        match v {
            Value::F64(f) => *f,
            Value::Int(i) => *i as f64,
            _ => f64::NAN,
        }
    }

    fn square_walls(first: u16, c: [(i32, i32); 4], neighbours: [i16; 4], partners: [i16; 4]) -> Vec<Wall> {
        let mut out = Vec::new();
        for (i, (x, y)) in c.into_iter().enumerate() {
            let mut w = wall_to(x, y, neighbours[i]);
            w.point2 = (first as i16) + ((i as i16 + 1) % 4);
            w.nextwall = partners[i];
            w.picnum = 1;
            out.push(w);
        }
        out
    }

    fn plain_sector(wallptr: u16, floorz: i32, ceilingz: i32, lotag: i16) -> Sector {
        Sector {
            wallptr,
            wallnum: 4,
            ceilingz,
            floorz,
            ceilingstat: 0,
            floorstat: 0,
            ceilingpicnum: 1,
            ceilingheinum: 0,
            ceilingshade: 0,
            ceilingxpanning: 0,
            ceilingypanning: 0,
            floorpicnum: 1,
            floorheinum: 0,
            floorshade: 0,
            floorxpanning: 0,
            floorypanning: 0,
            lotag,
            hitag: 0,
        }
    }

    fn one_tile_art() -> ArtBank {
        let mut art = ArtBank::default();
        art.tiles.insert(1, flat_tile(16, 16));
        art
    }

    /// Room A -- a closed ceiling door -- room B, where room B is also an
    /// elevator. Everything a Build level does that a static mesh cannot say,
    /// in three sectors.
    fn door_and_lift_map() -> BuildMap {
        // Rooms are 2 m tall (16384 BUILD z), the door is shut (ceiling on
        // its floor) and the lift platform stands half a metre up.
        let mut walls = Vec::new();
        walls.extend(square_walls(
            0,
            [(0, 0), (1024, 0), (1024, 1024), (0, 1024)],
            [-1, 1, -1, -1],
            [-1, 7, -1, -1],
        ));
        walls.extend(square_walls(
            4,
            [(1024, 0), (1280, 0), (1280, 1024), (1024, 1024)],
            [-1, 2, -1, 0],
            [-1, 11, -1, 1],
        ));
        walls.extend(square_walls(
            8,
            [(1280, 0), (2304, 0), (2304, 1024), (1280, 1024)],
            [-1, -1, -1, 1],
            [-1, -1, -1, 5],
        ));
        BuildMap {
            start: [512, 512, -4096],
            start_ang: 0,
            start_sec: 0,
            sectors: vec![
                plain_sector(0, 0, -16384, 0),
                plain_sector(4, 0, 0, ST_CEILING_DOOR),
                plain_sector(8, -4096, -16384, ST_LIFT_TAGS[0]),
            ],
            walls,
            sprites: Vec::new(),
        }
    }

    #[test]
    fn a_build_door_leaves_the_level_mesh_for_its_own_animated_node() {
        use makepad_asset_client::json::Value;
        let map = door_and_lift_map();
        let world = map_to_world(&map, &one_tile_art()).expect("world");
        let root = glb_json(&world.glb);

        let door = node_named(&root, "door_1").expect("door_1 node");
        let extras = door.get("extras").expect("door extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("door"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("open"));
        assert_eq!(extras.get("axis").and_then(Value::as_str), Some("y"));
        let states: Vec<&str> = extras
            .get("states")
            .and_then(Value::as_arr)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(states, vec!["closed", "open"]);
        // Shut ceiling is on the floor (0), open is the neighbours' ceiling
        // 16384 BUILD z up, which is two metres at this scale.
        assert!((num(extras.get("closed").unwrap()) - 0.0).abs() < 1e-6);
        assert!((num(extras.get("open").unwrap()) - 2.0).abs() < 1e-4, "{extras:?}");
        assert!((num(extras.get("travel").unwrap()) - 2.0).abs() < 1e-4);

        // Authored CLOSED, resting OPEN: a viewer that plays nothing still
        // walks through the doorway.
        let t = door.get("translation").and_then(Value::as_arr).expect("rest");
        assert!((num(&t[1]) - 2.0).abs() < 1e-4, "{t:?}");

        // One LINEAR clip named exactly like the node.
        let anims = root.get("animations").and_then(Value::as_arr).expect("animations");
        let clip = anims
            .iter()
            .find(|a| a.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door_1 clip");
        let sampler = &clip.get("samplers").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(sampler.get("interpolation").and_then(Value::as_str), Some("LINEAR"));
        let channel = &clip.get("channels").and_then(Value::as_arr).unwrap()[0];
        assert_eq!(
            channel.get("target").unwrap().get("path").and_then(Value::as_str),
            Some("translation")
        );

        // The leaf carries real geometry: two bands, one per room across the
        // doorway, spanning shut ceiling to room ceiling.
        let meshes = root.get("meshes").and_then(Value::as_arr).unwrap();
        let mesh = meshes
            .iter()
            .find(|m| m.get("name").and_then(Value::as_str) == Some("door_1"))
            .expect("door mesh");
        assert!(
            !mesh.get("primitives").and_then(Value::as_arr).unwrap().is_empty(),
            "the door node must carry the band it draws"
        );

        // And the sidecar says the same thing without opening the GLB.
        assert_eq!(world.doors.len(), 1, "{:?}", world.doors);
        assert_eq!(world.doors[0].name, "door_1");
        assert!((world.doors[0].closed_y - 0.0).abs() < 1e-6);
        assert!((world.doors[0].open_y - 2.0).abs() < 1e-4);
    }

    #[test]
    fn a_build_elevator_is_a_lift_node_authored_where_you_meet_it() {
        use makepad_asset_client::json::Value;
        let map = door_and_lift_map();
        let world = map_to_world(&map, &one_tile_art()).expect("world");
        let root = glb_json(&world.glb);

        let lift = node_named(&root, "lift_1").expect("lift_1 node");
        let extras = lift.get("extras").expect("lift extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("lift"));
        assert_eq!(extras.get("default").and_then(Value::as_str), Some("up"));
        let states: Vec<&str> = extras
            .get("states")
            .and_then(Value::as_arr)
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(states, vec!["up", "down"]);
        // Up is where BUILD drew it (half a metre), down is the neighbour's
        // floor, so the travel is negative.
        assert!((num(extras.get("up").unwrap()) - 0.5).abs() < 1e-4, "{extras:?}");
        assert!((num(extras.get("down").unwrap()) - 0.0).abs() < 1e-6);
        assert!(num(extras.get("travel").unwrap()) < 0.0);
        // A lift RESTS up: the platform is in the picture, not in the shaft.
        assert!(
            lift.get("translation").is_none(),
            "a lift's rest pose is the authored one"
        );
        assert_eq!(world.lifts.len(), 1);
        assert_eq!(world.lifts[0].name, "lift_1");
    }

    /// The door's band must not ALSO be in the static mesh, or the doorway
    /// stays sealed however the node moves.
    #[test]
    fn the_level_mesh_is_baked_with_the_door_open() {
        let map = door_and_lift_map();
        let world = map_to_world(&map, &one_tile_art()).expect("world");
        let parts = crate::world_preview::extract_glb_parts(&world.glb).expect("parts");
        // The doorway is the plane x = 1024..1280 between y 0 and 1024. At
        // head height (1.5 m) nothing static may block it.
        let blocked = parts.iter().any(|part| {
            part.indices.chunks_exact(3).any(|t| {
                let v: Vec<[f32; 3]> = t.iter().map(|&i| part.pos[i as usize]).collect();
                let in_doorway = v.iter().all(|p| {
                    p[0] >= 1024.0 * SCALE - 1e-3
                        && p[0] <= 1280.0 * SCALE + 1e-3
                        && p[1] > 1.0
                        && p[1] < 1.9
                });
                in_doorway
            })
        });
        assert!(!blocked, "the level mesh still seals the doorway");
    }

    /// BUILD declares its water as a sector tag, which is the only liquid it
    /// states as DATA rather than as a texture name.
    #[test]
    fn build_water_sectors_become_hazard_nodes() {
        use makepad_asset_client::json::Value;
        let mut map = door_and_lift_map();
        map.sectors[0].lotag = ST_ABOVE_WATER;
        map.sectors[2].lotag = ST_UNDERWATER;
        let hazards = build_hazards(&map);
        assert_eq!(hazards.len(), 2, "{hazards:?}");
        let world = map_to_world(&map, &one_tile_art()).expect("world");
        let root = glb_json(&world.glb);
        let surface = node_named(&root, "hazard_1").expect("hazard_1");
        let extras = surface.get("extras").unwrap();
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("hazard"));
        assert_eq!(extras.get("flat").and_then(Value::as_str), Some("water"));
        assert_eq!(extras.get("liquid"), Some(&Value::Bool(true)));
        assert_eq!(
            extras.get("solid"),
            Some(&Value::Bool(false)),
            "a water surface is something you drop THROUGH"
        );
        let bottom = node_named(&root, "hazard_2").expect("hazard_2");
        assert_eq!(
            bottom.get("extras").unwrap().get("solid"),
            Some(&Value::Bool(true)),
            "the bottom of the pool is something you stand on"
        );
    }

    /// The parallax ceiling stops being a painted cylinder in the level mesh
    /// and becomes the contract's sky node, direction-mapped by the renderer.
    #[test]
    fn the_parallax_ceiling_becomes_a_sky_node() {
        use makepad_asset_client::json::Value;
        let mut map = door_and_lift_map();
        // Parallax ceilings over both rooms.
        map.sectors[0].ceilingstat |= 1;
        map.sectors[2].ceilingstat |= 1;
        map.sectors[0].ceilingpicnum = 1;
        let world = map_to_world(&map, &one_tile_art()).expect("world");
        let root = glb_json(&world.glb);
        let sky = node_named(&root, "sky").expect("sky node");
        let extras = sky.get("extras").expect("sky extras");
        assert_eq!(extras.get("kind").and_then(Value::as_str), Some("sky"));
        assert_eq!(
            extras.get("projection").and_then(Value::as_str),
            Some("cylinder"),
            "BUILD wraps its strip once around the compass, like Doom's"
        );
        assert!((num(extras.get("repeat").unwrap()) - 1.0).abs() < 1e-6);
        assert_eq!(extras.get("texture").and_then(Value::as_str), Some("tile-1"));
    }

    /// Keycards, the exit and the teleporters are all in the sprite list, and
    /// a walker that cannot see them explores the level in the wrong order or
    /// never reaches the room behind the pad at all.
    #[test]
    fn build_sprites_publish_keys_exits_starts_and_teleporters() {
        let mut map = door_and_lift_map();
        let base = Sprite {
            x: 0,
            y: 0,
            z: 0,
            picnum: 0,
            cstat: 0,
            shade: 0,
            pal: 0,
            xrepeat: 8,
            yrepeat: 8,
            sectnum: 0,
            ang: 0,
            lotag: 0,
            hitag: 0,
        };
        map.sprites = vec![
            Sprite { picnum: TILE_ACCESSCARD, pal: 21, x: 300, y: 300, ..base.clone() },
            Sprite { picnum: TILE_NUKEBUTTON, x: 400, y: 400, ..base.clone() },
            Sprite { picnum: TILE_APLAYER, x: 500, y: 500, ang: 512, ..base.clone() },
            Sprite {
                picnum: TILE_SECTOREFFECTOR,
                lotag: SE_TELEPORT,
                hitag: 9,
                x: 200,
                y: 200,
                sectnum: 0,
                ..base.clone()
            },
            Sprite {
                picnum: TILE_SECTOREFFECTOR,
                lotag: SE_TELEPORT,
                hitag: 9,
                x: 1800,
                y: 500,
                sectnum: 2,
                ..base.clone()
            },
        ];
        let world = map_to_world(&map, &one_tile_art()).expect("world");
        let nav = map_nav(&map, world.spawn.expect("spawn"));

        let names: Vec<&str> = nav.markers.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"key_red"), "{names:?}");
        assert!(names.contains(&"exit"), "{names:?}");
        assert!(
            nav.starts.iter().any(|s| s.name == "deathmatch_1"),
            "{:?}",
            nav.starts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(nav.teleports.len(), 2, "both ends: {:?}", nav.teleports);
        let there = &nav.teleports[0];
        assert!(there.pad_max[0] > there.pad_min[0], "{there:?}");
        assert!(there.dst[0] > 1024.0 * SCALE, "lands in the far room: {there:?}");

        // Floor and eye come from the map, not from a constant: the start z
        // IS the eye and the start sector states the floor under it.
        assert!(nav.floor_y.is_some());
        let eye = nav.eye_height.expect("eye");
        assert!(eye > 0.0 && eye < 3.0, "eye {eye}");
        assert_eq!(
            nav.step_height, None,
            "BUILD does not state a step height, so none is published"
        );

        // The whole thing round-trips through the sidecar the library reads.
        let mut full = nav.clone();
        full.doors = world.doors.clone();
        full.lifts = world.lifts.clone();
        let text = full.to_text();
        let back = crate::world_nav::WorldNav::parse(&text).expect("parse");
        assert_eq!(back.doors, full.doors);
        assert_eq!(back.lifts, full.lifts);
        // The sidecar is four decimal places — a tenth of a millimetre — so
        // a position comes back near, not bit-equal.
        assert_eq!(
            back.markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            full.markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
        );
        for (b, f) in back.markers.iter().zip(&full.markers) {
            for (x, y) in b.pos.iter().zip(&f.pos) {
                assert!((x - y).abs() < 1e-3, "{b:?} vs {f:?}");
            }
        }
        assert_eq!(back.teleports.len(), full.teleports.len());
        for (b, f) in back.teleports.iter().zip(&full.teleports) {
            assert_eq!(b.name, f.name);
            for (x, y) in b.dst.iter().zip(&f.dst) {
                assert!((x - y).abs() < 1e-3, "{b:?} vs {f:?}");
            }
        }
        let anchors = back.anchors();
        for want in ["player_start", "door_1", "lift_1", "key_red", "exit", "teleport_1"] {
            assert!(
                anchors.iter().any(|a| a.name == want),
                "{want} missing from {:?}",
                anchors.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
            );
        }
    }
}
