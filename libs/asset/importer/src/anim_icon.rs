//! 128² animation tiles packed into a ~1024-wide PNG sheet.
//!
//! Quake MDL is vertex animation (not a skeleton). Doom/Quake sprites are
//! already discrete frames. Both become one sheet the library can show as
//! an animated-icon preview. KayKit rigs use the same packer after CPU-skin.

use crate::classic_import::encode_png_rgba;

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

/// True for the 1024-wide packed sheet, or any single-row 128-tall strip
/// with at least two tiles. Regular square thumbs (256/512/1024) stay still.
pub fn is_anim_sheet(width: usize, height: usize) -> bool {
    if width < TILE * 2 || height < TILE {
        return false;
    }
    if width % TILE != 0 || height % TILE != 0 {
        return false;
    }
    height == TILE || width == SHEET_W
}

/// Split a decoded BGRA sheet into 128² frames. Uniform (empty) cells are
/// dropped so leftover studio-clear padding is not played.
pub fn split_sheet_bgra(width: usize, height: usize, data: &[u32]) -> Option<Vec<Vec<u32>>> {
    if !is_anim_sheet(width, height) || data.len() < width * height {
        return None;
    }
    let cols = width / TILE;
    let rows = height / TILE;
    let mut frames = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let mut tile = vec![0u32; TILE * TILE];
            for y in 0..TILE {
                let src = (row * TILE + y) * width + col * TILE;
                let dst = y * TILE;
                tile[dst..dst + TILE].copy_from_slice(&data[src..src + TILE]);
            }
            if tile_is_empty(&tile) {
                continue;
            }
            frames.push(tile);
        }
    }
    (frames.len() > 1).then_some(frames)
}

const CLEAR_BGRA: u32 = {
    let [r, g, b, a] = CLEAR;
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
};

fn tile_is_empty(tile: &[u32]) -> bool {
    const MIN_PAINTED: usize = 16;
    tile.iter().filter(|&&p| !is_sheet_clear(p)).count() < MIN_PAINTED
}

fn is_sheet_clear(p: u32) -> bool {
    let a = (p >> 24) & 0xFF;
    a == 0 || p == CLEAR_BGRA
}

/// Pack tiles into a 1024-wide sheet (height = 128 × rows). Empty cells stay
/// the studio clear so the grid stays readable.
pub fn pack_sheet(tiles: &[TileRgba]) -> Result<Vec<u8>, String> {
    if tiles.is_empty() {
        return Err("no animation tiles".into());
    }
    let n = tiles.len().min(MAX_TILES);
    let rows = (n + TILES_PER_ROW - 1) / TILES_PER_ROW;
    let w = SHEET_W;
    let h = rows * TILE;
    let mut rgba = vec![0u8; w * h * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&CLEAR);
    }
    for (i, tile) in tiles.iter().take(n).enumerate() {
        if tile.len() < TILE * TILE * 4 {
            continue;
        }
        let col = i % TILES_PER_ROW;
        let row = i / TILES_PER_ROW;
        let ox = col * TILE;
        let oy = row * TILE;
        for y in 0..TILE {
            let src = y * TILE * 4;
            let dst = ((oy + y) * w + ox) * 4;
            rgba[dst..dst + TILE * 4].copy_from_slice(&tile[src..src + TILE * 4]);
        }
    }
    encode_png_rgba(&rgba, w as u32, h as u32)
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
    pack_sheet(&tiles).ok()
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
        let png = pack_sheet(&[tile.clone(), tile]).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        // IHDR width at bytes 16..20
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!(w, SHEET_W as u32);
        assert_eq!(h, TILE as u32);
    }

    #[test]
    fn split_skips_empty_cells_and_needs_two_frames() {
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
        let frames = split_sheet_bgra(SHEET_W, TILE, &sheet).expect("two painted tiles");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], red_bgra);
        assert_eq!(frames[1], green_bgra);
        assert!(split_sheet_bgra(512, 512, &vec![1u32; 512 * 512]).is_none());
        assert!(split_sheet_bgra(TILE, TILE, &red_bgra).is_none());
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
