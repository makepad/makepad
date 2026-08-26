//! 128² animation tiles packed into a ~1024-wide PNG sheet.
//!
//! Quake MDL is vertex animation (not a skeleton). Doom/Quake sprites are
//! already discrete frames. Both become one sheet the library can show as
//! an animated-icon preview. KayKit rigs use the same packer after CPU-skin.

use crate::classic_import::encode_png_rgba;
use makepad_asset_data::{ThumbnailCells, ThumbnailView, ThumbnailViewKind};

pub const TILE: usize = 128;
pub const SHEET_W: usize = 1024;
pub const MAX_TILES: usize = 64;
const TILES_PER_ROW: usize = SHEET_W / TILE;
const CLEAR: [u8; 4] = [26, 31, 41, 255];

/// One 128² RGBA tile.
pub type TileRgba = Vec<u8>;

/// Letterbox `src` into a 128² tile. Nearest-neighbour, keep aspect.
pub fn fit_tile(src: &[u8], sw: usize, sh: usize) -> TileRgba {
    let mut out = vec![0u8; TILE * TILE * 4];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&CLEAR);
    }
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return out;
    }
    let scale = (TILE as f32 / sw as f32).min(TILE as f32 / sh as f32);
    let dw = ((sw as f32 * scale).round() as usize).clamp(1, TILE);
    let dh = ((sh as f32 * scale).round() as usize).clamp(1, TILE);
    let ox = (TILE - dw) / 2;
    let oy = (TILE - dh) / 2;
    for y in 0..dh {
        let sy = (y as f32 * sh as f32 / dh as f32).floor() as usize;
        let sy = sy.min(sh - 1);
        for x in 0..dw {
            let sx = (x as f32 * sw as f32 / dw as f32).floor() as usize;
            let sx = sx.min(sw - 1);
            let si = (sy * sw + sx) * 4;
            let di = ((oy + y) * TILE + ox + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

/// Soft-shaded raster of a textured triangle mesh into one 128² tile.
/// `yaw` is applied around Y after centering (engine Y-up).
pub fn raster_mesh_tile(
    positions: &[[f32; 3]],
    indices: &[u32],
    uvs: &[[f32; 2]],
    tex: &[u8],
    tex_w: usize,
    tex_h: usize,
    yaw: f32,
) -> TileRgba {
    raster_mesh_icon(positions, indices, uvs, tex, tex_w, tex_h, yaw, TILE)
}

/// Same raster as [`raster_mesh_tile`], at an arbitrary square size (library
/// thumbs need ≥256 so pack_import accepts them).
pub fn raster_mesh_icon(
    positions: &[[f32; 3]],
    indices: &[u32],
    uvs: &[[f32; 2]],
    tex: &[u8],
    tex_w: usize,
    tex_h: usize,
    yaw: f32,
    dim: usize,
) -> Vec<u8> {
    raster_mesh_icon_clear(positions, indices, uvs, tex, tex_w, tex_h, yaw, dim, CLEAR)
}

/// Billboard frames use a transparent clear so they composite like Doom sprites.
pub fn raster_mesh_icon_clear(
    positions: &[[f32; 3]],
    indices: &[u32],
    uvs: &[[f32; 2]],
    tex: &[u8],
    tex_w: usize,
    tex_h: usize,
    yaw: f32,
    dim: usize,
    clear: [u8; 4],
) -> Vec<u8> {
    raster_mesh_icon_orient(
        positions, indices, uvs, tex, tex_w, tex_h, yaw, 0.0, dim, clear,
    )
}

/// Same as [`raster_mesh_icon_clear`] plus a pitch (radians, + looks down).
pub fn raster_mesh_icon_orient(
    positions: &[[f32; 3]],
    indices: &[u32],
    uvs: &[[f32; 2]],
    tex: &[u8],
    tex_w: usize,
    tex_h: usize,
    yaw: f32,
    pitch: f32,
    dim: usize,
    clear: [u8; 4],
) -> Vec<u8> {
    let dim = dim.max(1);
    let mut out = vec![0u8; dim * dim * 4];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&clear);
    }
    if positions.is_empty() || indices.len() < 3 || tex_w == 0 || tex_h == 0 {
        return out;
    }
    let (cos_y, sin_y) = (yaw.cos(), yaw.sin());
    let (cos_p, sin_p) = (pitch.cos(), pitch.sin());
    let mut xf = Vec::with_capacity(positions.len());
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for p in positions {
        let x = p[0] * cos_y + p[2] * sin_y;
        let y0 = p[1];
        let z0 = -p[0] * sin_y + p[2] * cos_y;
        let y = y0 * cos_p - z0 * sin_p;
        let z = y0 * sin_p + z0 * cos_p;
        xf.push([x, y, z]);
        for i in 0..3 {
            min[i] = min[i].min([x, y, z][i]);
            max[i] = max[i].max([x, y, z][i]);
        }
    }
    let cx = (min[0] + max[0]) * 0.5;
    let cy = (min[1] + max[1]) * 0.5;
    let ext = (max[0] - min[0]).max(max[1] - min[1]).max(0.08);
    let scale = (dim as f32 * 0.84) / ext;
    let mut depth = vec![f32::INFINITY; dim * dim];
    let light = [0.35f32, 0.85, 0.40];
    let llen = (light[0] * light[0] + light[1] * light[1] + light[2] * light[2]).sqrt();
    let light = [light[0] / llen, light[1] / llen, light[2] / llen];

    let sample = |u: f32, v: f32| -> [u8; 4] {
        let u = u.fract().abs();
        let v = v.fract().abs();
        let x = ((u * tex_w as f32) as usize).min(tex_w - 1);
        let y = ((v * tex_h as f32) as usize).min(tex_h - 1);
        let i = (y * tex_w + x) * 4;
        if i + 3 < tex.len() {
            [tex[i], tex[i + 1], tex[i + 2], tex[i + 3]]
        } else {
            [180, 180, 180, 255]
        }
    };

    let mut t = 0;
    while t + 2 < indices.len() {
        let ia = indices[t] as usize;
        let ib = indices[t + 1] as usize;
        let ic = indices[t + 2] as usize;
        t += 3;
        if ia >= xf.len() || ib >= xf.len() || ic >= xf.len() {
            continue;
        }
        let a = xf[ia];
        let b = xf[ib];
        let c = xf[ic];
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];
        let nl = (nx * nx + ny * ny + nz * nz).sqrt();
        if nl < 1.0e-8 {
            continue;
        }
        let ndot = ((nx * light[0] + ny * light[1] + nz * light[2]) / nl).clamp(0.15, 1.0);
        let shade = 0.35 + 0.65 * ndot;

        let to_s = |p: [f32; 3]| -> [f32; 3] {
            [
                (p[0] - cx) * scale + dim as f32 * 0.5,
                dim as f32 * 0.5 - (p[1] - cy) * scale,
                p[2],
            ]
        };
        let pa = to_s(a);
        let pb = to_s(b);
        let pc = to_s(c);
        let ua = uvs.get(ia).copied().unwrap_or([0.0, 0.0]);
        let ub = uvs.get(ib).copied().unwrap_or([0.0, 0.0]);
        let uc = uvs.get(ic).copied().unwrap_or([0.0, 0.0]);

        let minx = pa[0].min(pb[0]).min(pc[0]).floor() as i32;
        let maxx = pa[0].max(pb[0]).max(pc[0]).ceil() as i32;
        let miny = pa[1].min(pb[1]).min(pc[1]).floor() as i32;
        let maxy = pa[1].max(pb[1]).max(pc[1]).ceil() as i32;
        let minx = minx.clamp(0, dim as i32 - 1);
        let maxx = maxx.clamp(0, dim as i32 - 1);
        let miny = miny.clamp(0, dim as i32 - 1);
        let maxy = maxy.clamp(0, dim as i32 - 1);
        let area = edge(pa, pb, pc);
        if area.abs() < 1.0e-4 {
            continue;
        }
        for y in miny..=maxy {
            for x in minx..=maxx {
                let p = [x as f32 + 0.5, y as f32 + 0.5, 0.0];
                let w0 = edge(pb, pc, p) / area;
                let w1 = edge(pc, pa, p) / area;
                let w2 = edge(pa, pb, p) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = pa[2] * w0 + pb[2] * w1 + pc[2] * w2;
                let di = y as usize * dim + x as usize;
                if z >= depth[di] {
                    continue;
                }
                let u = ua[0] * w0 + ub[0] * w1 + uc[0] * w2;
                let v = ua[1] * w0 + ub[1] * w1 + uc[1] * w2;
                let mut rgba = sample(u, v);
                if rgba[3] < 16 {
                    continue;
                }
                depth[di] = z;
                for c in 0..3 {
                    rgba[c] = ((rgba[c] as f32) * shade) as u8;
                }
                let o = di * 4;
                out[o..o + 4].copy_from_slice(&rgba);
            }
        }
    }
    out
}

fn edge(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])
}

/// Library-card playback rate for a walk/idle sheet (matches billboard default).
pub const SHEET_PREVIEW_FPS: f32 = 8.0;

/// LEGACY ONLY: split a decoded BGRA sheet into 128² frames by measuring it.
///
/// This is the guess the views contract replaced. A packed sheet now DECLARES
/// its layout ([`PackedSheet::anim_view`]) and consumers cut the cells it
/// names, so nothing has to ask "is 1024x1024 a 64-tile sheet or a Flux
/// render?" — a question this function gets wrong, and always did.
///
/// It survives for exactly one job: revisions published BEFORE the contract
/// carried views, whose thumbnails say nothing about themselves. Call it only
/// when `ThumbnailMeta::animation()` returned `None`, and delete it when the
/// catalog has no pre-views revisions left.
pub fn legacy_split_sheet_bgra(width: usize, height: usize, data: &[u32]) -> Option<Vec<Vec<u32>>> {
    // The old shape test: the 1024-wide packed sheet, or any single-row
    // 128-tall strip with at least two tiles.
    let is_sheet = width >= TILE * 2
        && height >= TILE
        && width % TILE == 0
        && height % TILE == 0
        && (height == TILE || width == SHEET_W);
    if !is_sheet || data.len() < width * height {
        return None;
    }
    let frames = cut_cells_bgra(width, height, data, TILE as u32, (width / TILE) as u32, 0,
        ((width / TILE) * (height / TILE)) as u32);
    // Uniform (empty) cells are dropped so leftover studio-clear padding is
    // not played — the other half of the guess a declared count replaces.
    let painted: Vec<Vec<u32>> = frames
        .into_iter()
        .filter(|tile| tile.iter().filter(|&&p| !is_sheet_clear(p)).count() >= 16)
        .collect();
    (painted.len() > 1).then_some(painted)
}

/// Cut `count` cells out of a decoded BGRA sheet, starting at cell `first`,
/// using a DECLARED layout. No measuring, no emptiness test: the producer
/// said how many frames it wrote and where they are.
pub fn cut_cells_bgra(
    width: usize,
    height: usize,
    data: &[u32],
    cell: u32,
    cols: u32,
    first: u32,
    count: u32,
) -> Vec<Vec<u32>> {
    let (cell, cols) = (cell.max(1) as usize, cols.max(1) as usize);
    let mut frames = Vec::new();
    for i in 0..count as usize {
        let index = first as usize + i;
        let (ox, oy) = ((index % cols) * cell, (index / cols) * cell);
        if ox + cell > width || oy + cell > height || data.len() < width * height {
            break;
        }
        let mut tile = vec![0u32; cell * cell];
        for y in 0..cell {
            let src = (oy + y) * width + ox;
            tile[y * cell..(y + 1) * cell].copy_from_slice(&data[src..src + cell]);
        }
        frames.push(tile);
    }
    frames
}

const CLEAR_BGRA: u32 = {
    let [r, g, b, a] = CLEAR;
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
};

fn is_sheet_clear(p: u32) -> bool {
    let a = (p >> 24) & 0xFF;
    a == 0 || p == CLEAR_BGRA
}

/// The PNG text-chunk key a packed sheet stamps its own layout into.
///
/// The staged-pack path hands a bare PNG file from the importer that PACKED
/// it to the importer that PUBLISHES it, days and processes apart, with
/// nothing but a directory between them. Rather than invent a sidecar file
/// (a new pack media kind, a new attach rule, a new way to lose one), the
/// sheet carries its layout inside itself, in a chunk every PNG reader
/// already knows to skip. What the packer wrote is what the manifest
/// declares, with no third party to keep in step.
pub const SHEET_LAYOUT_KEY: &str = "makepad-sheet";

/// Write `cells` + `fps` into the PNG as a `tEXt` chunk, before `IEND`.
///
/// A restamp REPLACES: a producer that knows the real frame count stamps
/// over the packer's default, and the picture ends up carrying one layout,
/// not a history of them.
pub fn stamp_layout(png: &[u8], cells: ThumbnailCells, fps: f32) -> Vec<u8> {
    let png = strip_layout(png);
    let png = png.as_slice();
    let iend = match find_iend(png) {
        Some(at) => at,
        None => return png.to_vec(),
    };
    let text = format!(
        "{SHEET_LAYOUT_KEY}\0cells {} {} {} {} {} {}",
        cells.cols, cells.cell_w, cells.cell_h, cells.first, cells.count, fps
    );
    let mut out = png[..iend].to_vec();
    crate::classic_import::push_png_chunk(&mut out, b"tEXt", text.as_bytes());
    out.extend_from_slice(&png[iend..]);
    out
}

/// Read back a stamped layout, if the picture carries one. Understands
/// both containers a sheet ships in: a PNG's `tEXt` chunk and an MP4's
/// layout trailer box ([`stamp_layout_mp4`]).
pub fn read_layout(png: &[u8]) -> Option<(ThumbnailCells, f32)> {
    let body = if is_mp4(png) {
        mp4_layout_text(png)?
    } else {
        png_text_chunk(png, SHEET_LAYOUT_KEY)?
    };
    parse_layout_text(&body)
}

fn parse_layout_text(body: &str) -> Option<(ThumbnailCells, f32)> {
    let mut parts = body.split_whitespace();
    if parts.next()? != "cells" {
        return None;
    }
    let mut num = || parts.next().and_then(|s| s.parse::<u32>().ok());
    let (cols, cell_w, cell_h, first, count) = (num()?, num()?, num()?, num()?, num()?);
    let fps = parts.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(SHEET_PREVIEW_FPS);
    if cols == 0 || cell_w == 0 || cell_h == 0 || count == 0 {
        return None;
    }
    Some((ThumbnailCells { cols, cell_w, cell_h, first, count }, fps))
}

/// An ISO-BMFF container (mp4): every file starts with a box whose type is
/// `ftyp`.
fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

/// Walk an mp4's TOP-LEVEL boxes, calling `f(kind, payload, whole-box
/// range)` for each. Defensive: a malformed size ends the walk rather than
/// reading junk. 64-bit `largesize` boxes are skipped over correctly.
fn mp4_boxes(bytes: &[u8], mut f: impl FnMut(&[u8; 4], &[u8], std::ops::Range<usize>)) {
    let mut off = 0usize;
    while off + 8 <= bytes.len() {
        let size32 = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = bytes[off + 4..off + 8].try_into().unwrap();
        let (size, head) = if size32 == 1 {
            if off + 16 > bytes.len() {
                return;
            }
            let large = u64::from_be_bytes(bytes[off + 8..off + 16].try_into().unwrap());
            (large as usize, 16usize)
        } else if size32 == 0 {
            // "To end of file".
            (bytes.len() - off, 8usize)
        } else {
            (size32, 8usize)
        };
        if size < head || off + size > bytes.len() {
            return;
        }
        f(&kind, &bytes[off + head..off + size], off..off + size);
        off += size;
    }
}

/// The MP4 twin of [`stamp_layout`]: append the same layout text as a
/// top-level `free` box (every demuxer skips it; it survives a byte copy,
/// which is the whole point of stamping the file rather than a sidecar).
/// A restamp REPLACES any layout box already present.
pub fn stamp_layout_mp4(mp4: &[u8], cells: ThumbnailCells, fps: f32) -> Vec<u8> {
    let mut out = strip_layout_mp4(mp4);
    let text = format!(
        "{SHEET_LAYOUT_KEY}\0cells {} {} {} {} {} {}",
        cells.cols, cells.cell_w, cells.cell_h, cells.first, cells.count, fps
    );
    out.extend_from_slice(&((8 + text.len()) as u32).to_be_bytes());
    out.extend_from_slice(b"free");
    out.extend_from_slice(text.as_bytes());
    out
}

/// Drop any layout trailer already in the file, so a restamp replaces
/// rather than stacks.
fn strip_layout_mp4(bytes: &[u8]) -> Vec<u8> {
    if !is_mp4(bytes) {
        return bytes.to_vec();
    }
    let mut drop: Vec<std::ops::Range<usize>> = Vec::new();
    mp4_boxes(bytes, |kind, payload, range| {
        if kind == b"free"
            && payload.starts_with(SHEET_LAYOUT_KEY.as_bytes())
            && payload.get(SHEET_LAYOUT_KEY.len()) == Some(&0)
        {
            drop.push(range);
        }
    });
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    for range in drop {
        out.extend_from_slice(&bytes[at..range.start]);
        at = range.end;
    }
    out.extend_from_slice(&bytes[at..]);
    out
}

fn mp4_layout_text(bytes: &[u8]) -> Option<String> {
    let mut found = None;
    mp4_boxes(bytes, |kind, payload, _| {
        if found.is_none()
            && kind == b"free"
            && payload.starts_with(SHEET_LAYOUT_KEY.as_bytes())
            && payload.get(SHEET_LAYOUT_KEY.len()) == Some(&0)
        {
            found = String::from_utf8(payload[SHEET_LAYOUT_KEY.len() + 1..].to_vec()).ok();
        }
    });
    found
}

/// The declared views of a thumbnail image: an `anim` view when the picture
/// is a stamped sheet, nothing at all when it is an ordinary still. A still
/// that says nothing is honest; guessing from its dimensions is not.
pub fn views_of_png(png: &[u8]) -> Vec<ThumbnailView> {
    match read_layout(png) {
        Some((cells, fps)) => vec![ThumbnailView::cells(ThumbnailViewKind::Anim, cells, fps)],
        None => Vec::new(),
    }
}

/// Drop any layout chunk already in the picture, so a restamp replaces
/// rather than stacks.
fn strip_layout(png: &[u8]) -> Vec<u8> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png.to_vec();
    }
    let mut out = png[..8].to_vec();
    let mut off = 8usize;
    while off + 12 <= png.len() {
        let Ok(len) = png[off..off + 4].try_into() else {
            break;
        };
        let n = u32::from_be_bytes(len) as usize;
        if off + 12 + n > png.len() {
            break;
        }
        let ours = &png[off + 4..off + 8] == b"tEXt"
            && png[off + 8..off + 8 + n].starts_with(SHEET_LAYOUT_KEY.as_bytes())
            && png.get(off + 8 + SHEET_LAYOUT_KEY.len()) == Some(&0);
        if !ours {
            out.extend_from_slice(&png[off..off + 12 + n]);
        }
        off += 12 + n;
    }
    out
}

fn find_iend(png: &[u8]) -> Option<usize> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut off = 8usize;
    while off + 12 <= png.len() {
        let n = u32::from_be_bytes(png[off..off + 4].try_into().ok()?) as usize;
        if &png[off + 4..off + 8] == b"IEND" {
            return Some(off);
        }
        off = off.checked_add(12)?.checked_add(n)?;
    }
    None
}

fn png_text_chunk(png: &[u8], key: &str) -> Option<String> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut off = 8usize;
    while off + 12 <= png.len() {
        let n = u32::from_be_bytes(png[off..off + 4].try_into().ok()?) as usize;
        if off + 12 + n > png.len() {
            return None;
        }
        if &png[off + 4..off + 8] == b"tEXt" {
            let data = &png[off + 8..off + 8 + n];
            if let Some(split) = data.iter().position(|b| *b == 0) {
                if &data[..split] == key.as_bytes() {
                    return String::from_utf8(data[split + 1..].to_vec()).ok();
                }
            }
        }
        off += 12 + n;
    }
    None
}

/// A packed sheet and the layout it ACTUALLY has: the cell size, how many
/// cells per row, and how many of those cells are frames rather than the
/// clear padding that buys a published thumbnail its height floor.
///
/// This is the thing consumers used to have to guess. A producer hands the
/// layout to the manifest ([`Self::anim_view`]) and nobody measures pixels
/// again.
pub struct PackedSheet {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub cell: u32,
    pub cols: u32,
    /// Real frames, padding excluded.
    pub count: u32,
}

impl PackedSheet {
    /// The declared cell layout of this sheet.
    pub fn cells(&self) -> ThumbnailCells {
        ThumbnailCells {
            cols: self.cols,
            cell_w: self.cell,
            cell_h: self.cell,
            first: 0,
            count: self.count,
        }
    }

    /// The manifest view a thumbnail carries for this sheet.
    pub fn anim_view(&self, fps: f32) -> ThumbnailView {
        ThumbnailView::cells(ThumbnailViewKind::Anim, self.cells(), fps)
    }
}

/// Pack 128² tiles into a 1024-wide sheet (height = 128 x rows).
pub fn pack_sheet(tiles: &[TileRgba]) -> Result<PackedSheet, String> {
    pack_grid(tiles, TILE, TILES_PER_ROW)
}

/// Pack `cell`-square tiles `cols` per row. Empty cells stay the studio
/// clear so the grid stays readable — and are excluded from `count`, so a
/// consumer plays the frames and not the padding.
pub fn pack_grid(tiles: &[TileRgba], cell: usize, cols: usize) -> Result<PackedSheet, String> {
    if tiles.is_empty() {
        return Err("no animation tiles".into());
    }
    let (cell, cols) = (cell.max(1), cols.max(1));
    let n = tiles.len().min(MAX_TILES);
    let rows = n.div_ceil(cols);
    let (w, h) = (cols * cell, rows * cell);
    let mut rgba = vec![0u8; w * h * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&CLEAR);
    }
    for (i, tile) in tiles.iter().take(n).enumerate() {
        if tile.len() < cell * cell * 4 {
            continue;
        }
        let (ox, oy) = ((i % cols) * cell, (i / cols) * cell);
        for y in 0..cell {
            let src = y * cell * 4;
            let dst = ((oy + y) * w + ox) * 4;
            rgba[dst..dst + cell * 4].copy_from_slice(&tile[src..src + cell * 4]);
        }
    }
    let cells = ThumbnailCells {
        cols: cols as u32,
        cell_w: cell as u32,
        cell_h: cell as u32,
        first: 0,
        count: n as u32,
    };
    Ok(PackedSheet {
        // Stamped on the way out: a sheet that leaves this function already
        // says what it is, wherever it ends up.
        png: stamp_layout(&encode_png_rgba(&rgba, w as u32, h as u32)?, cells, SHEET_PREVIEW_FPS),
        width: w as u32,
        height: h as u32,
        cell: cell as u32,
        cols: cols as u32,
        count: n as u32,
    })
}

/// Prefer a named loop (stand / idle / walk) then fall back to the first
/// frames. Caps at [`MAX_TILES`].
pub fn pick_loop_indices(names: &[String]) -> Vec<usize> {
    if names.is_empty() {
        return Vec::new();
    }
    const PREF: &[&str] = &["stand", "idle", "walk", "run", "frame"];
    for pref in PREF {
        let hit: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                let l = n.to_ascii_lowercase();
                l.starts_with(pref) || l.contains(pref)
            })
            .map(|(i, _)| i)
            .collect();
        if hit.len() >= 2 {
            return subsample(&hit, MAX_TILES);
        }
    }
    if names.len() == 1 {
        return vec![0];
    }
    subsample(&(0..names.len()).collect::<Vec<_>>(), 16.min(MAX_TILES))
}

/// CPU-skin a rigged GLB (KayKit) into an 8-tile walk/idle sheet.
pub fn skinned_anim_sheet(glb: &[u8]) -> Option<Vec<u8>> {
    use makepad_render::skin::{
        PoseBuffer, SkinnedModel, SKIN_VERTEX_FLOATS, GAIT_IDLE_CLIPS, GAIT_WALK_CLIPS,
    };
    let model = SkinnedModel::parse_glb(glb).ok()?;
    if model.clips.is_empty() {
        return None;
    }
    let clip = model
        .clip_index_any(GAIT_WALK_CLIPS)
        .or_else(|| model.clip_index_any(GAIT_IDLE_CLIPS))
        .or(Some(0))?;
    let dur = model.clips.get(clip).map(|c| c.duration).unwrap_or(1.0).max(0.04);
    let n = 8usize;
    let mut pose = PoseBuffer::new();
    let mut pal = Vec::new();
    let mut packed = Vec::new();
    let mut tiles = Vec::new();
    let white = [200u8, 198, 190, 255];
    let indices = model.indices();
    for i in 0..n {
        let t = i as f32 / n as f32 * dur;
        model.sample_clip(clip, t, &mut pose);
        model.palette(&pose, &mut pal);
        model.skin_to_packed(&pal, &mut packed);
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut i = 0;
        while i + SKIN_VERTEX_FLOATS <= packed.len() {
            positions.push([packed[i], packed[i + 1], packed[i + 2]]);
            uvs.push([0.5, 0.5]);
            i += SKIN_VERTEX_FLOATS;
        }
        tiles.push(raster_mesh_tile(
            &positions,
            indices,
            &uvs,
            &white,
            1,
            1,
            -std::f32::consts::FRAC_PI_2 + 0.35,
        ));
    }
    // The bytes carry their own layout, stamped by the packer.
    pack_sheet(&tiles).ok().map(|sheet| sheet.png)
}

fn subsample(idx: &[usize], max: usize) -> Vec<usize> {
    if idx.len() <= max {
        return idx.to_vec();
    }
    let mut out = Vec::with_capacity(max);
    for i in 0..max {
        let t = i as f32 * (idx.len() - 1) as f32 / (max - 1) as f32;
        out.push(idx[t.round() as usize]);
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_is_1024_wide_with_128_tiles() {
        let tile = fit_tile(&[255, 0, 0, 255, 0, 255, 0, 255], 2, 1);
        assert_eq!(tile.len(), TILE * TILE * 4);
        let sheet = pack_sheet(&[tile.clone(), tile]).unwrap();
        let png = &sheet.png;
        assert!(png.starts_with(b"\x89PNG"));
        // IHDR width at bytes 16..20
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!(w, SHEET_W as u32);
        assert_eq!(h, TILE as u32);
        assert_eq!((sheet.width, sheet.height), (w, h));
    }

    /// A packed sheet says what it is, in its own bytes: the layout the
    /// packer WROTE travels with the file through a staged pack directory,
    /// so the manifest declares it instead of a consumer measuring it.
    #[test]
    fn a_packed_sheet_carries_its_own_layout() {
        let tile = fit_tile(&[255, 0, 0, 255], 1, 1);
        let sheet = pack_sheet(&[tile.clone(), tile.clone(), tile]).unwrap();
        let (cells, fps) = read_layout(&sheet.png).expect("the sheet declares itself");
        assert_eq!(cells, sheet.cells());
        assert_eq!(cells.cols, TILES_PER_ROW as u32);
        assert_eq!((cells.cell_w, cells.cell_h), (TILE as u32, TILE as u32));
        assert_eq!(cells.count, 3, "three frames, not eight cells");
        assert_eq!(fps, SHEET_PREVIEW_FPS);
        // The stamp is a tEXt chunk: the picture is unchanged, and a reader
        // that knows nothing about it decodes the same pixels.
        let (plain, w, h) = crate::classic_import::decode_png_stored(&sheet.png).unwrap();
        assert_eq!((w, h), (sheet.width, sheet.height));
        assert_eq!(plain.len(), (w * h * 4) as usize);
        // An ordinary picture declares nothing rather than guessing.
        let still = crate::classic_import::encode_png_rgba(&[9, 9, 9, 255], 1, 1).unwrap();
        assert_eq!(read_layout(&still), None);
        assert!(views_of_png(&still).is_empty());
        // A restamp replaces the count with the producer's real one.
        let mine = ThumbnailCells { count: 2, ..sheet.cells() };
        let restamped = stamp_layout(&sheet.png, mine, 12.0);
        assert_eq!(read_layout(&restamped), Some((mine, 12.0)));
    }

    /// The declared cutter takes the frames the layout NAMES, in order,
    /// including cells the old emptiness guess would have thrown away.
    #[test]
    fn declared_cells_are_cut_without_measuring() {
        let (cell, cols) = (2usize, 2usize);
        // 4x4 sheet of 2x2 cells, each filled with its own index.
        let mut data = vec![0u32; 4 * 4];
        for i in 0..4usize {
            let (ox, oy) = ((i % cols) * cell, (i / cols) * cell);
            for y in 0..cell {
                for x in 0..cell {
                    data[(oy + y) * 4 + ox + x] = i as u32 + 1;
                }
            }
        }
        let frames = cut_cells_bgra(4, 4, &data, cell as u32, cols as u32, 1, 2);
        assert_eq!(frames.len(), 2);
        assert!(frames[0].iter().all(|p| *p == 2));
        assert!(frames[1].iter().all(|p| *p == 3));
        // A range that runs off the picture stops rather than reading
        // someone else's pixels.
        assert_eq!(cut_cells_bgra(4, 4, &data, cell as u32, cols as u32, 3, 8).len(), 1);
    }

    #[test]
    fn legacy_split_skips_empty_cells_and_needs_two_frames() {
        let red = fit_tile(&[255, 0, 0, 255], 1, 1);
        let green = fit_tile(&[0, 255, 0, 255], 1, 1);
        let mut red_bgra = vec![0u32; TILE * TILE];
        let mut green_bgra = vec![0u32; TILE * TILE];
        for y in 0..TILE {
            for x in 0..TILE {
                let i = (y * TILE + x) * 4;
                red_bgra[y * TILE + x] = (red[i + 3] as u32) << 24
                    | (red[i] as u32) << 16
                    | (red[i + 1] as u32) << 8
                    | red[i + 2] as u32;
                green_bgra[y * TILE + x] = (green[i + 3] as u32) << 24
                    | (green[i] as u32) << 16
                    | (green[i + 1] as u32) << 8
                    | green[i + 2] as u32;
            }
        }
        let mut sheet = vec![0u32; SHEET_W * TILE];
        for y in 0..TILE {
            let src = y * TILE;
            let row = y * SHEET_W;
            sheet[row..row + TILE].copy_from_slice(&red_bgra[src..src + TILE]);
            sheet[row + TILE..row + TILE * 2].copy_from_slice(&green_bgra[src..src + TILE]);
        }
        let frames = legacy_split_sheet_bgra(SHEET_W, TILE, &sheet).expect("two painted tiles");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], red_bgra);
        assert_eq!(frames[1], green_bgra);
        assert!(legacy_split_sheet_bgra(512, 512, &vec![1u32; 512 * 512]).is_none());
        assert!(legacy_split_sheet_bgra(TILE, TILE, &red_bgra).is_none());
    }

    #[test]
    fn pick_loop_prefers_stand() {
        let names = ["pain1", "stand1", "stand2", "stand3", "walk1"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let idx = pick_loop_indices(&names);
        assert_eq!(idx, vec![1, 2, 3]);
    }
}
