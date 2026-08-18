//! SDF silhouette shadow atlas — the bake half of THE dynamic shadow
//! system: every character rig and every dynamic model (the driveable cars)
//! casts through one of these.
//!
//! One small R8 sprite per (relative yaw × gait phase) cell: the posed
//! mesh's ground-projected silhouette, rasterised, then signed-
//! distance-transformed. SDFs survive low resolution (the font-atlas trick),
//! and — the property this whole tier exists for — LERPING TWO SDFs MORPHS
//! the silhouette between them. A sprite crossfade double-exposes: mid-blend
//! a walker grows four ghost legs. The SDF lerp moves the iso-line instead,
//! so the runtime can tween 16 yaws × (idle + 8 walk phases) of baked cells
//! into a continuous limb-swinging shadow with FOUR texture taps and zero
//! per-frame geometry. A RIGID caster (a car) bakes the degenerate case:
//! 16 yaws × 1 row (~16 KB of R8), two texture taps, phase inert.
//!
//! The bake projects along a CANONICAL light azimuth (+X, at the scene sun's
//! elevation); the 16 yaw cells cover the RELATIVE yaw between character
//! facing and the owning light's azimuth, and the runtime quad rotates the
//! sprite into the light's world frame. That is what lets one atlas serve
//! the sun from any direction AND a street lamp: rotation is free, only
//! elevation is baked. Sun fixed after load (the project's standing
//! assumption): atlases rebake with the static shadows when it moves.
//!
//! Every cell of one rig shares a single world-space window (`rect`), which
//! is what makes the 4-tap lerp geometrically meaningful — texel (u,v) is
//! the same ground offset in every cell.

use makepad_draw::*;

use crate::sun::SunLight;

/// Magic + version for the `.shadowsdf` disk sidecar
/// ([`ShadowSdfAtlas::to_shadowsdf`]). Bumped whenever the atlas layout or
/// its metadata change meaning, so a stale cache is IGNORED (falling back to
/// the off-thread bake) rather than sampled with the wrong geometry.
const SHADOW_SDF_MAGIC: &[u8; 8] = b"SHDWSDF\x01";

/// Yaw stations per atlas row, 22.5 degrees apart: the distance lerp
/// between two stations wobbles the iso-line by at most half a station,
/// invisible under a soft-edged shadow.
pub const SDF_YAWS: usize = 16;

/// Walk-cycle stations for a CHARACTER bake: the gait loop is sampled at
/// this many evenly spaced phases, each getting its own atlas row. 8
/// catches both stride extremes and both crossovers; the runtime lerp
/// hides the quantisation the same way the yaw lerp does.
pub const SDF_GAIT_PHASES: usize = 8;

/// Pose rows a full character bake carries: idle + the walk cycle.
pub const SDF_POSE_ROWS: usize = 1 + SDF_GAIT_PHASES;

/// Cell resolution. 32x32 is enough because the payload is a distance
/// field, not coverage — the iso-line lands between texels.
pub const SDF_CELL: usize = 32;

/// Supersample factor for the mask + distance transform; only the final
/// signed DISTANCES are averaged down (the region_sun_sdf recipe — averaging
/// distances places the reconstructed edge between mask texels).
const SDF_SS: usize = 2;

/// Signed-distance band encoded into a byte, in 1x texels: 128 = the
/// silhouette boundary, 0 = deep inside, 255 = far outside.
pub const SDF_BAND_TEXELS: f32 = 4.0;

/// One rig's baked silhouette-SDF atlas: `SDF_YAWS` columns × `rows` rows of
/// [`SDF_CELL`]² R8 cells. `pixels[row_major]`, width = `SDF_YAWS * SDF_CELL`.
#[derive(Clone, PartialEq, Debug)]
pub struct ShadowSdfAtlas {
    pub pixels: Vec<u8>,
    /// 1 for a rigid/yaw-only bake, [`SDF_POSE_ROWS`] for a full gait bake.
    pub rows: usize,
    /// The shared world-space window every cell maps, as offsets from the
    /// rig's ground anchor in the CANONICAL light frame: (min_along,
    /// min_across, size_along, size_across). Model units — the instance
    /// scale applies at runtime.
    pub rect: (f32, f32, f32, f32),
    /// World units covered by the encoded byte band's half range: byte 128
    /// ± 127 spans ± this distance. The runtime converts edge softness into
    /// d units with it.
    pub band_world: f32,
    /// Ground shadow offset per unit of source height the bake projected
    /// with — the runtime's contact-hardening term maps distance-along-tip
    /// back to source height through it.
    pub len_per_unit: f32,
}

impl ShadowSdfAtlas {
    pub fn width(&self) -> usize {
        SDF_YAWS * SDF_CELL
    }
    pub fn height(&self) -> usize {
        self.rows * SDF_CELL
    }
    /// Decoded signed distance at a cell texel, world units, positive =
    /// outside the silhouette.
    pub fn distance_at(&self, yaw: usize, row: usize, x: usize, y: usize) -> f32 {
        let w = self.width();
        let px = self.pixels[(row * SDF_CELL + y) * w + yaw * SDF_CELL + x];
        (px as f32 / 255.0 - 0.5) * 2.0 * self.band_world
    }

    /// Serialise this atlas to `.shadowsdf` sidecar bytes: the EXACT pixel
    /// bytes plus the addressing metadata the runtime SDF tier consumes, so
    /// loading is a copy, not a transformation. `source_hash` keys a rig
    /// sidecar to its mesh (the `.skinao` scheme —
    /// [`crate::skin::SkinnedModel::rest_hash`]); model sidecars, which are
    /// keyed by mtime against their glb instead, write 0.
    ///
    /// The bake also depends on the SUN (the canonical-azimuth projection
    /// bakes the sun's elevation in as `len_per_unit`), which is why that
    /// field doubles as the loader's validity gate: a sidecar baked for a
    /// different sun must lose to a live re-bake, not draw every shadow at
    /// the wrong length.
    pub fn to_shadowsdf(&self, source_hash: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(44 + self.pixels.len());
        out.extend_from_slice(SHADOW_SDF_MAGIC);
        out.extend_from_slice(&source_hash.to_le_bytes());
        out.extend_from_slice(&(self.rows as u32).to_le_bytes());
        for f in [self.rect.0, self.rect.1, self.rect.2, self.rect.3, self.band_world, self.len_per_unit] {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&self.pixels);
        out
    }

    /// Parse `.shadowsdf` sidecar bytes back into the atlas plus its source
    /// hash. `None` on any mismatch — magic, version, truncation, or
    /// nonsense metadata — so a foreign or stale file falls back to the
    /// off-thread bake rather than to a scrambled shadow. The caller still
    /// owns the freshness gates (mtime vs the glb, source hash, sun).
    pub fn from_shadowsdf(bytes: &[u8]) -> Option<(ShadowSdfAtlas, u64)> {
        let mut at = 0usize;
        let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
            let s = bytes.get(*at..*at + n)?;
            *at += n;
            Some(s)
        };
        if take(&mut at, 8)? != SHADOW_SDF_MAGIC {
            return None;
        }
        let source_hash = u64::from_le_bytes(take(&mut at, 8)?.try_into().ok()?);
        let rows = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?) as usize;
        if rows == 0 || rows > SDF_POSE_ROWS {
            return None;
        }
        let rd_f32 = |at: &mut usize| -> Option<f32> {
            Some(f32::from_le_bytes(take(at, 4)?.try_into().ok()?))
        };
        let rect = (rd_f32(&mut at)?, rd_f32(&mut at)?, rd_f32(&mut at)?, rd_f32(&mut at)?);
        let band_world = rd_f32(&mut at)?;
        let len_per_unit = rd_f32(&mut at)?;
        for f in [rect.0, rect.1, rect.2, rect.3, band_world, len_per_unit] {
            if !f.is_finite() {
                return None;
            }
        }
        if rect.2 <= 0.0 || rect.3 <= 0.0 || band_world <= 0.0 || len_per_unit < 0.0 {
            return None;
        }
        let npix = SDF_YAWS * SDF_CELL * rows * SDF_CELL;
        let pixels = bytes.get(at..)?.to_vec();
        if pixels.len() != npix {
            return None;
        }
        Some((ShadowSdfAtlas { pixels, rows, rect, band_world, len_per_unit }, source_hash))
    }

    /// Peek a sidecar's baked sun term from its header alone (the first
    /// [`SHADOWSDF_HEADER_LEN`] bytes) — no pixel load. `None` when the
    /// bytes are not a current-version header. The offline baker's resume
    /// uses this to notice a sidecar baked for a DIFFERENT sun: such a file
    /// is mtime-fresh forever, yet the runtime rejects it every launch —
    /// "0 sidecars written" while the cast silently blob-tiers.
    pub fn header_len_per_unit(header: &[u8]) -> Option<f32> {
        if header.get(..8)? != SHADOW_SDF_MAGIC {
            return None;
        }
        let lpu = f32::from_le_bytes(header.get(40..44)?.try_into().ok()?);
        lpu.is_finite().then_some(lpu)
    }
}

/// Byte length of the `.shadowsdf` header ([`ShadowSdfAtlas::to_shadowsdf`]):
/// magic 8 + source hash 8 + rows 4 + rect 16 + band 4 + len_per_unit 4.
pub const SHADOWSDF_HEADER_LEN: usize = 44;

/// Positions out of a packed vertex stream — the runtime's derivation of a
/// bake's input, shared by CPU-skinned pose meshes
/// ([`crate::skin::SKIN_VERTEX_FLOATS`]) and static models
/// ([`crate::model::MODEL_VERTEX_FLOATS`]): the two strides are asserted
/// equal in skin.rs because characters and props share one shader. Lives
/// here so the offline baker and the renderer derive a bake's positions
/// from the SAME code — byte-identical atlases by construction.
pub fn packed_positions(vertices: &[f32]) -> Vec<Vec3f> {
    let stride = crate::skin::SKIN_VERTEX_FLOATS;
    (0..vertices.len() / stride)
        .map(|i| {
            vec3f(
                vertices[i * stride],
                vertices[i * stride + 1],
                vertices[i * stride + 2],
            )
        })
        .collect()
}

/// Union AABB over a bake's pose set — the renderer's exact fold (order and
/// ops), shared with the offline baker for the same reason as
/// [`packed_positions`].
pub fn pose_bounds(poses: &[Vec<Vec3f>]) -> (Vec3f, Vec3f) {
    let (mut min, mut max) = (
        vec3f(f32::MAX, f32::MAX, f32::MAX),
        vec3f(f32::MIN, f32::MIN, f32::MIN),
    );
    for p in poses.iter().flatten() {
        min = vec3f(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
        max = vec3f(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
    }
    (min, max)
}

/// A dynamic MODEL's yaw-only atlas from its baked static mesh — the same
/// input derivation `renderer::load_model` feeds the off-thread bake
/// (`sil_positions` out of the packed stream, the model's own bounds), so
/// the offline sidecar and the runtime fallback produce byte-identical
/// atlases for the same model and sun.
pub fn bake_model_atlas(
    model: &crate::model::StaticModel,
    sun: &SunLight,
) -> Option<ShadowSdfAtlas> {
    let positions = packed_positions(&model.vertices);
    bake_silhouette_atlas(
        std::slice::from_ref(&positions),
        &model.indices,
        model.min,
        model.max,
        sun,
    )
}

/// A character RIG's gait atlas straight from its `SkinnedModel` — the
/// offline mirror of the runtime seed path (host CPU-skins the pose set via
/// [`crate::skin::SkinnedModel::shadow_pose_meshes`], renderer extracts
/// positions and bounds, then bakes). Every step is the shared code above,
/// so a sidecar baked here is byte-identical to what the off-thread bake
/// would deliver for the same rig and sun. `None` when the rig has no
/// recognisable gait pair or bakes degenerate.
pub fn bake_rig_atlas(
    model: &crate::skin::SkinnedModel,
    sun: &SunLight,
) -> Option<ShadowSdfAtlas> {
    let (idle, walk) = model.gait_clips()?;
    let meshes = model.shadow_pose_meshes(idle, walk);
    let poses: Vec<Vec<Vec3f>> = meshes.iter().map(|m| packed_positions(m)).collect();
    let (min, max) = pose_bounds(&poses);
    bake_silhouette_atlas(&poses, model.indices(), min, max, sun)
}

/// Ground projection of one posed vertex under the canonical light: rotate
/// the point by `yaw` (the renderer's `Mat4f::rotation` convention), then
/// slide it down-sun with azimuth +X.
#[inline]
fn project(p: Vec3f, anchor: Vec2f, min_y: f32, yaw_c: f32, yaw_s: f32, len: f32) -> Vec2f {
    let (lx, lz) = (p.x - anchor.x, p.z - anchor.y);
    let (rx, rz) = (lx * yaw_c + lz * yaw_s, -lx * yaw_s + lz * yaw_c);
    let h = (p.y - min_y).max(0.0);
    vec2f(rx - h * len, rz)
}

/// Rasterise projected triangles into an `n`×`n` boolean mask over `rect`
/// (cell-centre inside-test — the mesh is a closed volume, its projection a
/// filled region).
fn rasterize_mask(
    flat: &[Vec2f],
    indices: &[u32],
    rect: (f32, f32, f32, f32),
    n: usize,
) -> Vec<bool> {
    let (x0, z0, w, h) = rect;
    let (tx, tz) = (w / n as f32, h / n as f32);
    let mut mask = vec![false; n * n];
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (
            flat[t[0] as usize],
            flat[t[1] as usize],
            flat[t[2] as usize],
        );
        let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if area.abs() < 1.0e-9 {
            continue;
        }
        let sgn = area.signum();
        let cx0 = (((a.x.min(b.x).min(c.x) - x0) / tx).floor().max(0.0)) as usize;
        let cy0 = (((a.y.min(b.y).min(c.y) - z0) / tz).floor().max(0.0)) as usize;
        let cx1 = ((((a.x.max(b.x).max(c.x) - x0) / tx).ceil() as usize) + 1).min(n);
        let cy1 = ((((a.y.max(b.y).max(c.y) - z0) / tz).ceil() as usize) + 1).min(n);
        for cy in cy0..cy1 {
            for cx in cx0..cx1 {
                if mask[cy * n + cx] {
                    continue;
                }
                let p = vec2f(x0 + (cx as f32 + 0.5) * tx, z0 + (cy as f32 + 0.5) * tz);
                let w0 = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                let w1 = (c.x - b.x) * (p.y - b.y) - (c.y - b.y) * (p.x - b.x);
                let w2 = (a.x - c.x) * (p.y - c.y) - (a.y - c.y) * (p.x - c.x);
                if w0 * sgn >= 0.0 && w1 * sgn >= 0.0 && w2 * sgn >= 0.0 {
                    mask[cy * n + cx] = true;
                }
            }
        }
    }
    mask
}

/// Two-pass chamfer distance transform, adapted from lightmap.rs's, but with
/// WORLD-unit axial weights: the atlas window is elongated along the light
/// azimuth, so its texels are anisotropic, and an unweighted chamfer would
/// give direction-dependent edge softness. Weighted, the encoded distance is
/// isotropic in world space no matter the window's aspect.
fn chamfer_weighted(d: &mut [f32], n: usize, wx: f32, wy: f32) {
    let diag = (wx * wx + wy * wy).sqrt();
    let idx = |x: usize, y: usize| y * n + x;
    for y in 0..n {
        for x in 0..n {
            let mut v = d[idx(x, y)];
            if x > 0 {
                v = v.min(d[idx(x - 1, y)] + wx);
            }
            if y > 0 {
                v = v.min(d[idx(x, y - 1)] + wy);
                if x > 0 {
                    v = v.min(d[idx(x - 1, y - 1)] + diag);
                }
                if x + 1 < n {
                    v = v.min(d[idx(x + 1, y - 1)] + diag);
                }
            }
            d[idx(x, y)] = v;
        }
    }
    for y in (0..n).rev() {
        for x in (0..n).rev() {
            let mut v = d[idx(x, y)];
            if x + 1 < n {
                v = v.min(d[idx(x + 1, y)] + wx);
            }
            if y + 1 < n {
                v = v.min(d[idx(x, y + 1)] + wy);
                if x + 1 < n {
                    v = v.min(d[idx(x + 1, y + 1)] + diag);
                }
                if x > 0 {
                    v = v.min(d[idx(x - 1, y + 1)] + diag);
                }
            }
            d[idx(x, y)] = v;
        }
    }
}

/// Mask → signed distances in world units (positive = outside), at the
/// mask's own resolution.
fn signed_field(mask: &[bool], n: usize, wx: f32, wy: f32) -> Vec<f32> {
    const FAR: f32 = 1.0e9;
    let mut d_in = vec![FAR; n * n];
    let mut d_out = vec![FAR; n * n];
    for (i, m) in mask.iter().enumerate() {
        if *m {
            d_in[i] = 0.0;
        } else {
            d_out[i] = 0.0;
        }
    }
    chamfer_weighted(&mut d_in, n, wx, wy);
    chamfer_weighted(&mut d_out, n, wx, wy);
    // Inside texels: d_in = 0, so sd = -d_out (negative). Outside: +d_in.
    d_in.iter().zip(d_out.iter()).map(|(i, o)| i - o).collect()
}

/// Bake one caster's silhouette-SDF atlas. `poses[0]` idle, `poses[1..]`
/// the walk cycle at evenly spaced phases (a single entry bakes a yaw-only
/// atlas — the rigid-model/car case); all poses share `indices`. Fallback
/// contract: a walk pose that projects to nothing copies the idle row
/// rather than failing the bake; an idle that projects to nothing fails
/// it. `None` for degenerate meshes.
pub fn bake_silhouette_atlas(
    poses: &[Vec<Vec3f>],
    indices: &[u32],
    min: Vec3f,
    max: Vec3f,
    sun: &SunLight,
) -> Option<ShadowSdfAtlas> {
    let first = poses.first()?;
    if indices.len() < 3 || first.is_empty() {
        return None;
    }
    let anchor = vec2f((min.x + max.x) * 0.5, (min.z + max.z) * 0.5);
    let len = sun.shadow_len_per_unit();
    let rows = poses.len();
    let yaw_at = |k: usize| {
        let yaw = k as f32 / SDF_YAWS as f32 * std::f32::consts::TAU;
        (yaw.cos(), yaw.sin())
    };
    // Pass 1: the SHARED window — union bounds of every cell's projection.
    // One mapping for all cells is what makes inter-cell SDF lerp a morph
    // rather than a smear, and it must be computed before any cell bakes.
    let (mut lo, mut hi) = (vec2f(f32::MAX, f32::MAX), vec2f(f32::MIN, f32::MIN));
    for pose in poses {
        if pose.len() != first.len() {
            continue; // falls back to idle below; idle already in bounds
        }
        for k in 0..SDF_YAWS {
            let (c, s) = yaw_at(k);
            for p in pose {
                let q = project(*p, anchor, min.y, c, s, len);
                lo = vec2f(lo.x.min(q.x), lo.y.min(q.y));
                hi = vec2f(hi.x.max(q.x), hi.y.max(q.y));
            }
        }
    }
    let span = (hi.x - lo.x).max(hi.y - lo.y);
    if !span.is_finite() || span < 1.0e-4 {
        return None;
    }
    // Pad by the encode band plus a texel: the window border must decode as
    // "far outside" so the quad's own edge is already fully transparent.
    let pad_frac = (SDF_BAND_TEXELS + 1.0) / SDF_CELL as f32;
    let pad = vec2f((hi.x - lo.x) * pad_frac, (hi.y - lo.y) * pad_frac);
    // Keep a sane minimum across-thickness: an edge-on plane otherwise
    // collapses the window to zero height.
    let pad = vec2f(pad.x.max(span * 0.02), pad.y.max(span * 0.02));
    let rect = (
        lo.x - pad.x,
        lo.y - pad.y,
        (hi.x - lo.x) + 2.0 * pad.x,
        (hi.y - lo.y) + 2.0 * pad.y,
    );
    let band_world = SDF_BAND_TEXELS * (rect.2 / SDF_CELL as f32).max(rect.3 / SDF_CELL as f32);

    let n2 = SDF_CELL * SDF_SS;
    let (wx2, wy2) = (rect.2 / n2 as f32, rect.3 / n2 as f32);
    let width = SDF_YAWS * SDF_CELL;
    let mut pixels = vec![255u8; width * rows * SDF_CELL];
    let mut flat: Vec<Vec2f> = Vec::with_capacity(first.len());
    let mut row_ok = vec![false; rows];
    for (row, pose) in poses.iter().enumerate() {
        if pose.len() != first.len() {
            continue;
        }
        let mut any = false;
        for k in 0..SDF_YAWS {
            let (c, s) = yaw_at(k);
            flat.clear();
            flat.extend(pose.iter().map(|p| project(*p, anchor, min.y, c, s, len)));
            let mask = rasterize_mask(&flat, indices, rect, n2);
            if !mask.iter().any(|m| *m) {
                continue;
            }
            any = true;
            let sd2 = signed_field(&mask, n2, wx2, wy2);
            // Average the supersampled DISTANCES down to the cell, then
            // encode: 128 = boundary, ±band_world across the byte range.
            for y in 0..SDF_CELL {
                for x in 0..SDF_CELL {
                    let mut acc = 0.0;
                    for sy in 0..SDF_SS {
                        for sx in 0..SDF_SS {
                            acc += sd2[(y * SDF_SS + sy) * n2 + x * SDF_SS + sx]
                                .clamp(-2.0 * band_world, 2.0 * band_world);
                        }
                    }
                    let sd = acc / (SDF_SS * SDF_SS) as f32;
                    let v = (0.5 + sd / (2.0 * band_world)).clamp(0.0, 1.0);
                    pixels[(row * SDF_CELL + y) * width + k * SDF_CELL + x] =
                        (v * 255.0).round() as u8;
                }
            }
        }
        row_ok[row] = any;
    }
    if !row_ok[0] {
        return None; // idle is mandatory — it is the fallback for the rest
    }
    // A bad walk row copies the idle row: a shadow that stops breathing
    // beats a character demoted to a blob for good (the ring bake's rule).
    for row in 1..rows {
        if !row_ok[row] {
            for y in 0..SDF_CELL {
                let src = y * width;
                let dst = (row * SDF_CELL + y) * width;
                let line: Vec<u8> = pixels[src..src + width].to_vec();
                pixels[dst..dst + width].copy_from_slice(&line);
            }
        }
    }
    Some(ShadowSdfAtlas {
        pixels,
        rows,
        rect,
        band_world,
        len_per_unit: len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed box mesh, offset so "legs" can be posed by moving vertices.
    fn box_mesh(cx: f32, cz: f32, w: f32, h: f32, d: f32, y0: f32) -> (Vec<Vec3f>, Vec<u32>) {
        let p = vec![
            vec3f(cx - w, y0, cz - d),
            vec3f(cx + w, y0, cz - d),
            vec3f(cx + w, y0, cz + d),
            vec3f(cx - w, y0, cz + d),
            vec3f(cx - w, y0 + h, cz - d),
            vec3f(cx + w, y0 + h, cz - d),
            vec3f(cx + w, y0 + h, cz + d),
            vec3f(cx - w, y0 + h, cz + d),
        ];
        let i = vec![
            0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7,
            2, 7, 6, 3, 0, 4, 3, 4, 7,
        ];
        (p, i)
    }

    /// Body + two legs sharing one index buffer, legs scissored to `fwd` /
    /// `-fwd` along x — the sweep a walk cycle makes. Legs overlap the torso
    /// in height and footprint so every pose projects one connected
    /// silhouette (as a real character does).
    fn walker(fwd: f32) -> (Vec<Vec3f>, Vec<u32>) {
        let (torso, ti) = box_mesh(0.0, 0.0, 0.4, 1.2, 0.25, 0.5);
        let (l0, _) = box_mesh(fwd, -0.12, 0.12, 0.9, 0.10, 0.0);
        let (l1, _) = box_mesh(-fwd, 0.12, 0.12, 0.9, 0.10, 0.0);
        let mut p = torso;
        let mut i = ti.clone();
        i.extend(ti.iter().map(|k| k + 8));
        i.extend(ti.iter().map(|k| k + 16));
        p.extend(l0);
        p.extend(l1);
        (p, i)
    }

    fn sun() -> SunLight {
        SunLight {
            dir: vec3f(0.35, 0.8, 0.45).normalize(),
            ..SunLight::default()
        }
    }

    fn bounds(p: &[Vec3f]) -> (Vec3f, Vec3f) {
        let (mut lo, mut hi) = (
            vec3f(f32::MAX, f32::MAX, f32::MAX),
            vec3f(f32::MIN, f32::MIN, f32::MIN),
        );
        for v in p {
            lo = vec3f(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
            hi = vec3f(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
        }
        (lo, hi)
    }

    /// A cell's thresholded mask, `true` = inside the silhouette.
    fn cell_mask(atlas: &ShadowSdfAtlas, yaw: usize, row: usize) -> Vec<bool> {
        let mut out = vec![false; SDF_CELL * SDF_CELL];
        for y in 0..SDF_CELL {
            for x in 0..SDF_CELL {
                out[y * SDF_CELL + x] = atlas.distance_at(yaw, row, x, y) < 0.0;
            }
        }
        out
    }

    fn area(mask: &[bool]) -> usize {
        mask.iter().filter(|m| **m).count()
    }

    /// 4-connected component count of a mask.
    fn components(mask: &[bool], n: usize) -> usize {
        let mut seen = vec![false; mask.len()];
        let mut count = 0;
        for start in 0..mask.len() {
            if !mask[start] || seen[start] {
                continue;
            }
            count += 1;
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(at) = stack.pop() {
                let (x, y) = (at % n, at / n);
                let mut push = |nx: usize, ny: usize, stack: &mut Vec<usize>| {
                    let i = ny * n + nx;
                    if mask[i] && !seen[i] {
                        seen[i] = true;
                        stack.push(i);
                    }
                };
                if x > 0 {
                    push(x - 1, y, &mut stack);
                }
                if x + 1 < n {
                    push(x + 1, y, &mut stack);
                }
                if y > 0 {
                    push(x, y - 1, &mut stack);
                }
                if y + 1 < n {
                    push(x, y + 1, &mut stack);
                }
            }
        }
        count
    }

    /// A car-shaped rigid mesh: long low body + a shorter, taller cabin box
    /// offset toward the rear, sharing one index buffer — enough asymmetry
    /// that its silhouette is visibly NOT an ellipse.
    fn car() -> (Vec<Vec3f>, Vec<u32>) {
        let (body, bi) = box_mesh(0.0, 0.0, 1.0, 0.45, 0.45, 0.0);
        let (cabin, _) = box_mesh(-0.25, 0.0, 0.45, 0.42, 0.40, 0.42);
        let mut p = body;
        let mut i = bi.clone();
        i.extend(bi.iter().map(|k| k + 8));
        p.extend(cabin);
        (p, i)
    }

    /// The yaw-only bake a dynamic MODEL (car) gets: single pose row,
    /// deterministic byte-for-byte, and exactly the ~16 KB the runtime
    /// budgets per model.
    #[test]
    fn yaw_only_bake_is_deterministic_and_16k() {
        let (p, i) = car();
        let (min, max) = bounds(&p);
        let a = bake_silhouette_atlas(&[p.clone()], &i, min, max, &sun()).expect("bake a");
        let b = bake_silhouette_atlas(&[p], &i, min, max, &sun()).expect("bake b");
        assert_eq!(a.rows, 1);
        assert_eq!(a.pixels, b.pixels);
        assert_eq!(a.rect, b.rect);
        assert_eq!(a.band_world, b.band_world);
        assert_eq!(a.pixels.len(), SDF_YAWS * SDF_CELL * SDF_CELL); // 16384
    }

    /// Car atlas threshold round-trip: decoding each yaw cell at d = 0 must
    /// reconstruct the silhouette the rasteriser drew — the cabin bump
    /// included, which is the whole point of migrating cars off ellipses.
    #[test]
    fn car_atlas_threshold_round_trips() {
        let (pose, i) = car();
        let (min, max) = bounds(&pose);
        let s = sun();
        let atlas = bake_silhouette_atlas(&[pose.clone()], &i, min, max, &s).expect("bake");
        assert_eq!(atlas.rows, 1);
        let anchor = vec2f((min.x + max.x) * 0.5, (min.z + max.z) * 0.5);
        for yaw in 0..SDF_YAWS {
            let yawf = yaw as f32 / SDF_YAWS as f32 * std::f32::consts::TAU;
            let (c, sn) = (yawf.cos(), yawf.sin());
            let flat: Vec<Vec2f> = pose
                .iter()
                .map(|p| project(*p, anchor, min.y, c, sn, s.shadow_len_per_unit()))
                .collect();
            let reference = rasterize_mask(&flat, &i, atlas.rect, SDF_CELL);
            let got = cell_mask(&atlas, yaw, 0);
            let (mut inter, mut union) = (0usize, 0usize);
            for (a, b) in reference.iter().zip(got.iter()) {
                if *a && *b {
                    inter += 1;
                }
                if *a || *b {
                    union += 1;
                }
            }
            assert!(union > 0, "yaw {yaw}: empty silhouette");
            let iou = inter as f32 / union as f32;
            assert!(iou > 0.8, "yaw {yaw}: IoU {iou} too low");
        }
        // The cabin must read in the decoded outline: against a body-only
        // raster over the SAME window, the car's mask differs where the
        // cabin projects — an ellipse-ish body blob would not. Checked at a
        // quarter turn, where the tall cabin's longer shadow clears the
        // narrow body profile.
        let q = SDF_YAWS / 4;
        let qf = q as f32 / SDF_YAWS as f32 * std::f32::consts::TAU;
        let (qc, qs) = (qf.cos(), qf.sin());
        let (body, bi) = box_mesh(0.0, 0.0, 1.0, 0.45, 0.45, 0.0);
        let flat_body: Vec<Vec2f> = body
            .iter()
            .map(|p| project(*p, anchor, min.y, qc, qs, s.shadow_len_per_unit()))
            .collect();
        let body_mask = rasterize_mask(&flat_body, &bi, atlas.rect, SDF_CELL);
        let car_mask = cell_mask(&atlas, q, 0);
        let extra = car_mask
            .iter()
            .zip(body_mask.iter())
            .filter(|(c, b)| **c && !**b)
            .count();
        assert!(
            extra > 6,
            "cabin adds only {extra} texels over the bare body — outline lost"
        );
    }

    #[test]
    fn bake_is_deterministic() {
        let (idle, i) = walker(0.05);
        let (stride, _) = walker(0.4);
        let poses = vec![idle.clone(), stride.clone(), idle.clone()];
        let (min, max) = bounds(&stride);
        let a = bake_silhouette_atlas(&poses, &i, min, max, &sun()).expect("bake a");
        let b = bake_silhouette_atlas(&poses, &i, min, max, &sun()).expect("bake b");
        assert_eq!(a.pixels, b.pixels);
        assert_eq!(a.rect, b.rect);
        assert_eq!(a.rows, 3);
        assert_eq!(a.pixels.len(), SDF_YAWS * SDF_CELL * 3 * SDF_CELL);
    }

    /// Threshold round-trip: decoding a cell at d = 0 must reconstruct the
    /// silhouette the rasteriser drew — the SDF is a lossless-enough carrier
    /// at 32² for a mask baked from the same window.
    #[test]
    fn threshold_reconstructs_the_mask() {
        let (pose, i) = walker(0.3);
        let (min, max) = bounds(&pose);
        let s = sun();
        let atlas =
            bake_silhouette_atlas(&[pose.clone()], &i, min, max, &s).expect("bake");
        assert_eq!(atlas.rows, 1);
        for yaw in [0usize, 3, 7, 12] {
            // Reference: rasterize the same projection directly at 1x.
            let anchor = vec2f((min.x + max.x) * 0.5, (min.z + max.z) * 0.5);
            let yawf = yaw as f32 / SDF_YAWS as f32 * std::f32::consts::TAU;
            let (c, sn) = (yawf.cos(), yawf.sin());
            let flat: Vec<Vec2f> = pose
                .iter()
                .map(|p| project(*p, anchor, min.y, c, sn, s.shadow_len_per_unit()))
                .collect();
            let reference = rasterize_mask(&flat, &i, atlas.rect, SDF_CELL);
            let got = cell_mask(&atlas, yaw, 0);
            let (mut inter, mut union) = (0usize, 0usize);
            for (a, b) in reference.iter().zip(got.iter()) {
                if *a && *b {
                    inter += 1;
                }
                if *a || *b {
                    union += 1;
                }
            }
            assert!(union > 0, "yaw {yaw}: empty silhouette");
            let iou = inter as f32 / union as f32;
            assert!(iou > 0.8, "yaw {yaw}: IoU {iou} too low");
        }
    }

    /// The morph guarantee: lerping the DISTANCES of two adjacent walk
    /// phases at t = 0.5 yields ONE connected silhouette whose area sits
    /// near the endpoints' — while a sprite crossfade would show the union
    /// of both poses at half alpha (ghost legs). Adjacent phases move a limb
    /// by less than its own width, which is exactly the regime SDF
    /// interpolation morphs instead of fading.
    #[test]
    fn midpoint_morph_is_single_and_not_double_exposed() {
        let (idle, i) = walker(0.05);
        let (a, _) = walker(0.45);
        let (b, _) = walker(0.30);
        let poses = vec![idle, a, b];
        let (mut min, mut max) = bounds(&poses[1]);
        let (bmin, bmax) = bounds(&poses[2]);
        min = vec3f(min.x.min(bmin.x), min.y.min(bmin.y), min.z.min(bmin.z));
        max = vec3f(max.x.max(bmax.x), max.y.max(bmax.y), max.z.max(bmax.z));
        let atlas = bake_silhouette_atlas(&poses, &i, min, max, &sun()).expect("bake");
        for yaw in [0usize, 4, 10] {
            let m1 = cell_mask(&atlas, yaw, 1);
            let m2 = cell_mask(&atlas, yaw, 2);
            // Morph: threshold of the lerped distances.
            let mut mid = vec![false; SDF_CELL * SDF_CELL];
            for y in 0..SDF_CELL {
                for x in 0..SDF_CELL {
                    let d = 0.5 * atlas.distance_at(yaw, 1, x, y)
                        + 0.5 * atlas.distance_at(yaw, 2, x, y);
                    mid[y * SDF_CELL + x] = d < 0.0;
                }
            }
            assert_eq!(
                components(&mid, SDF_CELL),
                1,
                "yaw {yaw}: morph midpoint splits into ghosts"
            );
            let (am, a1, a2) = (area(&mid), area(&m1), area(&m2));
            let (lo, hi) = (a1.min(a2), a1.max(a2));
            assert!(am > 0, "yaw {yaw}: empty midpoint");
            // Area stays in the endpoints' neighbourhood — a morph, not a
            // union (which would exceed hi by the whole swept strip) and
            // not a fade-out (which would collapse below lo).
            assert!(
                am as f32 >= lo as f32 * 0.75 && am as f32 <= hi as f32 * 1.2,
                "yaw {yaw}: midpoint area {am} outside endpoint band ({a1}, {a2})"
            );
            // Where the endpoints actually differ, the crossfade's union
            // must exceed the morph — the double exposure the SDF lerp is
            // here to prevent.
            let union: Vec<bool> = m1.iter().zip(m2.iter()).map(|(a, b)| *a || *b).collect();
            let differ = m1.iter().zip(m2.iter()).filter(|(a, b)| a != b).count();
            if differ > 8 {
                assert!(
                    am < area(&union),
                    "yaw {yaw}: morph area {am} matches the union {} — double exposure",
                    area(&union)
                );
            }
        }
    }

    /// Rotating the character a quarter turn must swap the silhouette's
    /// long axis in the ACROSS direction the same way the ring bake's does —
    /// the yaw cells really are rotations, not copies.
    #[test]
    fn yaw_cells_differ() {
        let (pose, i) = walker(0.45);
        let (min, max) = bounds(&pose);
        let atlas = bake_silhouette_atlas(&[pose], &i, min, max, &sun()).expect("bake");
        let m0 = cell_mask(&atlas, 0, 0);
        let m4 = cell_mask(&atlas, SDF_YAWS / 4, 0);
        let differ = m0
            .iter()
            .zip(m4.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            differ > area(&m0) / 4,
            "quarter-turn cell nearly identical ({differ} texels differ)"
        );
    }

    /// A degenerate walk pose copies the idle row instead of failing.
    #[test]
    fn bad_walk_row_falls_back_to_idle() {
        let (idle, i) = walker(0.1);
        let flat: Vec<Vec3f> = idle.iter().map(|p| vec3f(0.0, p.y, 0.0)).collect();
        let (min, max) = bounds(&idle);
        let atlas =
            bake_silhouette_atlas(&[idle, flat], &i, min, max, &sun()).expect("bake");
        assert_eq!(atlas.rows, 2);
        let w = atlas.width();
        assert_eq!(
            atlas.pixels[..SDF_CELL * w],
            atlas.pixels[SDF_CELL * w..2 * SDF_CELL * w],
            "bad row should copy idle"
        );
    }

    /// GPU-faithful atlas fetch: bilinear with CLAMP-TO-EDGE addressing over
    /// the whole atlas texture (the platform's default sampler —
    /// shader_output.rs `ShaderSampler::default`), coordinates in texel
    /// units. Clamping at the ATLAS border is load-bearing: it is what an
    /// out-of-range cell index actually reads on the GPU (the border
    /// padding, "far outside"), so a wrap bug reproduces here instead of
    /// panicking on an index.
    fn tex_bilinear(atlas: &ShadowSdfAtlas, x: f32, y: f32) -> f32 {
        let (w, h) = (atlas.width() as isize, atlas.height() as isize);
        let (xf, yf) = (x - 0.5, y - 0.5);
        let (x0, y0) = (xf.floor(), yf.floor());
        let (fx, fy) = (xf - x0, yf - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let at = |xi: isize, yi: isize| {
            let xi = xi.clamp(0, w - 1) as usize;
            let yi = yi.clamp(0, h - 1) as usize;
            atlas.pixels[yi * w as usize + xi] as f32 / 255.0
        };
        at(x0, y0) * (1.0 - fx) * (1.0 - fy)
            + at(x0 + 1, y0) * fx * (1.0 - fy)
            + at(x0, y0 + 1) * (1.0 - fx) * fy
            + at(x0 + 1, y0 + 1) * fx * fy
    }

    fn stepf(edge: f32, x: f32) -> f32 {
        if x < edge {
            0.0
        } else {
            1.0
        }
    }

    fn fractf(x: f32) -> f32 {
        x - x.floor()
    }

    /// CPU transcription of DrawSceneShadowSdf's `cell_d` (shaders.rs).
    fn cell_d(atlas: &ShadowSdfAtlas, k: f32, row: f32, uv: Vec2f) -> f32 {
        let tx = (uv.x * 32.0).clamp(0.5, 31.5);
        let ty = (uv.y * 32.0).clamp(0.5, 31.5);
        tex_bilinear(atlas, k * 32.0 + tx, row * 32.0 + ty)
    }

    /// CPU transcription of DrawSceneShadowSdf's pixel-stage distance
    /// reconstruction: yaw-pair lerp (with the 15 -> 0 wrap), then the
    /// phase-row pair mixed in by the gait blend. Encoded units: 0.5 = the
    /// silhouette boundary, above = outside. MUST stay in lockstep with the
    /// shader's `pixel` fn.
    fn shader_distance(
        atlas: &ShadowSdfAtlas,
        rel: f32,
        phase: f32,
        blend: f32,
        uv: Vec2f,
    ) -> f32 {
        let station = fractf(rel / std::f32::consts::TAU) * 16.0;
        let k0 = station.floor();
        let kf = station - k0;
        let k1 = k0 + 1.0 - 16.0 * stepf(15.5, k0 + 1.0);
        let yaw_d = |row: f32| {
            cell_d(atlas, k0, row, uv) * (1.0 - kf) + cell_d(atlas, k1, row, uv) * kf
        };
        let mut d = yaw_d(0.0);
        let rows = atlas.rows as f32;
        if blend > 0.001 {
            let g = rows - 1.0;
            if g > 0.5 {
                let pp = fractf(phase) * g;
                let p0 = pp.floor();
                let pf = pp - p0;
                let p1 = p0 + 1.0 - g * stepf(g - 0.5, p0 + 1.0);
                let dw = yaw_d(1.0 + p0) * (1.0 - pf) + yaw_d(1.0 + p1) * pf;
                d = d * (1.0 - blend) + dw * blend;
            }
        }
        d
    }

    /// Texels inside the reconstructed silhouette (encoded d < 0.5) over
    /// the sprite window, sampled at the shader's own resolution.
    fn silhouette_area(atlas: &ShadowSdfAtlas, rel: f32, phase: f32, blend: f32) -> usize {
        let mut count = 0;
        for y in 0..SDF_CELL {
            for x in 0..SDF_CELL {
                let uv = vec2f(
                    (x as f32 + 0.5) / SDF_CELL as f32,
                    (y as f32 + 0.5) / SDF_CELL as f32,
                );
                if shader_distance(atlas, rel, phase, blend, uv) < 0.5 {
                    count += 1;
                }
            }
        }
        count
    }

    /// `MAKEPAD_SDF_DUMP_DIR=<dir>` writes a baked atlas as a binary PGM
    /// (16 yaw columns x `rows` pose rows) for eyeballing degenerate cells.
    fn maybe_dump_pgm(atlas: &ShadowSdfAtlas, name: &str) {
        let Ok(dir) = std::env::var("MAKEPAD_SDF_DUMP_DIR") else {
            return;
        };
        let (w, h) = (atlas.width(), atlas.height());
        let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
        out.extend_from_slice(&atlas.pixels);
        let path = std::path::Path::new(&dir).join(format!("sdf_{name}.pgm"));
        match std::fs::write(&path, out) {
            Ok(()) => eprintln!("dumped {}", path.display()),
            Err(e) => eprintln!("dump {} failed: {e}", path.display()),
        }
    }

    /// The full-turn guarantee: sweeping relative yaw through -2pi..2pi (the
    /// range the CPU actually hands the shader — rel = yaw - alpha is never
    /// pre-wrapped), the reconstructed silhouette area must never collapse
    /// below 60% of its mean. This is the permanent net for the cell 15 -> 0
    /// wrap (a k1 of 16 reads the clamped atlas border — "far outside" —
    /// and the shadow vanished across exactly one 22.5-degree heading band)
    /// and for any bake-side degenerate yaw cell. Checked in idle AND
    /// mid-walk configurations, including the phase pair that wraps row
    /// 8 -> 1, and on a car's yaw-only atlas with gait inputs inert.
    #[test]
    fn yaw_sweep_never_collapses() {
        let (idle, i) = walker(0.05);
        let mut poses = vec![idle];
        for k in 0..SDF_GAIT_PHASES {
            let ph = k as f32 / SDF_GAIT_PHASES as f32 * std::f32::consts::TAU;
            poses.push(walker(0.45 * ph.sin()).0);
        }
        let (mut min, mut max) = bounds(&poses[0]);
        for pose in &poses[1..] {
            let (lo, hi) = bounds(pose);
            min = vec3f(min.x.min(lo.x), min.y.min(lo.y), min.z.min(lo.z));
            max = vec3f(max.x.max(hi.x), max.y.max(hi.y), max.z.max(hi.z));
        }
        let atlas = bake_silhouette_atlas(&poses, &i, min, max, &sun()).expect("gait bake");
        assert_eq!(atlas.rows, SDF_POSE_ROWS);
        let (car_pose, ci) = car();
        let (cmin, cmax) = bounds(&car_pose);
        let car_atlas =
            bake_silhouette_atlas(&[car_pose], &ci, cmin, cmax, &sun()).expect("car bake");
        maybe_dump_pgm(&atlas, "walker");
        maybe_dump_pgm(&car_atlas, "car");
        // (atlas, phase, blend, label): idle, mid-walk, the phase pair that
        // wraps (fract(0.99) * 8 = 7.92 -> rows 8 and 1), and the rigid car.
        let configs: [(&ShadowSdfAtlas, f32, f32, &str); 4] = [
            (&atlas, 0.0, 0.0, "idle"),
            (&atlas, 0.25, 1.0, "mid-walk"),
            (&atlas, 0.99, 1.0, "phase-wrap"),
            (&car_atlas, 0.0, 0.0, "car"),
        ];
        const STEPS: usize = 256;
        for (atlas, phase, blend, name) in configs {
            let rel_at = |s: usize| {
                (s as f32 / STEPS as f32) * 2.0 * std::f32::consts::TAU
                    - std::f32::consts::TAU
            };
            let areas: Vec<f32> = (0..STEPS)
                .map(|s| silhouette_area(atlas, rel_at(s), phase, blend) as f32)
                .collect();
            let mean = areas.iter().sum::<f32>() / STEPS as f32;
            assert!(mean > 0.0, "{name}: no silhouette at any yaw");
            for (s, a) in areas.iter().enumerate() {
                assert!(
                    *a >= 0.6 * mean,
                    "{name}: silhouette area {a} at rel yaw {:.4} collapsed \
                     below 60% of mean {mean:.1}",
                    rel_at(s)
                );
            }
        }
    }

    /// `.shadowsdf` bytes round-trip every field and the source hash, and
    /// anything unrecognised parses to `None` — the fallback contract that
    /// keeps a foreign or truncated file from becoming a scrambled shadow.
    #[test]
    fn shadowsdf_round_trips_and_rejects() {
        let (idle, i) = walker(0.05);
        let (stride, _) = walker(0.4);
        let poses = vec![idle.clone(), stride, idle];
        let (min, max) = bounds(&poses[1]);
        let atlas = bake_silhouette_atlas(&poses, &i, min, max, &sun()).expect("bake");
        let bytes = atlas.to_shadowsdf(0xDEAD_BEEF_1234_5678);
        let (back, hash) = ShadowSdfAtlas::from_shadowsdf(&bytes).expect("round trip");
        assert_eq!(hash, 0xDEAD_BEEF_1234_5678);
        assert!(back == atlas, "round-tripped atlas differs");
        // Rejections: empty, bad magic, wrong version, truncation, trailing
        // garbage, and a rows count the runtime never bakes.
        assert!(ShadowSdfAtlas::from_shadowsdf(b"").is_none());
        assert!(ShadowSdfAtlas::from_shadowsdf(b"not-a-shadow-sdf-file").is_none());
        let mut wrong_version = bytes.clone();
        wrong_version[7] = 0x7F;
        assert!(ShadowSdfAtlas::from_shadowsdf(&wrong_version).is_none());
        assert!(ShadowSdfAtlas::from_shadowsdf(&bytes[..bytes.len() - 1]).is_none());
        let mut padded = bytes.clone();
        padded.push(0);
        assert!(ShadowSdfAtlas::from_shadowsdf(&padded).is_none());
        let mut bad_rows = bytes.clone();
        bad_rows[16..20].copy_from_slice(&(SDF_POSE_ROWS as u32 + 1).to_le_bytes());
        assert!(ShadowSdfAtlas::from_shadowsdf(&bad_rows).is_none());
    }

    /// The offline pipeline against the runtime's own bake, on a REAL
    /// dynamic-caster model (a Kenney car): baking through
    /// [`bake_model_atlas`], serialising to `.shadowsdf` bytes and loading
    /// them back must reproduce the off-thread bake's atlas BYTE FOR BYTE
    /// for the same input — the sidecar is a cache of that computation, and
    /// any transformation would be a second code path to keep honest.
    /// Prefers the shipped `.aomesh` (the very mesh the runtime loads and
    /// the baker resumes from); skips when the asset library is absent.
    #[test]
    fn sidecar_matches_off_thread_bake_for_real_model() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/car-kit");
        let model = match std::fs::read(root.join("ambulance.aomesh"))
            .ok()
            .and_then(|b| crate::model::StaticModel::from_aomesh(&b))
        {
            Some(m) => m,
            None => match std::fs::read(root.join("ambulance.glb")) {
                Ok(b) => crate::model::StaticModel::parse_glb(&b).expect("parse glb"),
                Err(_) => {
                    eprintln!("kenney assets absent — skipped");
                    return;
                }
            },
        };
        // The sandbox worlds' standing sun — what ao-bake defaults to.
        let sun = SunLight { dir: vec3f(0.55, 0.62, 0.56).normalize(), ..SunLight::default() };
        // The off-thread bake's exact input derivation (renderer::load_model
        // feeds seed_model_sdf positions out of the packed stream).
        let positions = packed_positions(&model.vertices);
        let runtime = bake_silhouette_atlas(
            std::slice::from_ref(&positions),
            &model.indices,
            model.min,
            model.max,
            &sun,
        )
        .expect("off-thread bake");
        // The offline pipeline: bake, serialise, load.
        let offline = bake_model_atlas(&model, &sun).expect("offline bake");
        let (loaded, hash) =
            ShadowSdfAtlas::from_shadowsdf(&offline.to_shadowsdf(0)).expect("parse sidecar");
        assert_eq!(hash, 0);
        assert_eq!(loaded.rows, 1, "a rigid model bakes yaw-only");
        assert!(
            loaded == runtime,
            "sidecar-loaded atlas differs from the off-thread bake"
        );
    }

    /// Same contract for a character RIG (a real Kenney mini-character):
    /// [`bake_rig_atlas`] → sidecar bytes → load must equal what the
    /// renderer's off-thread bake delivers when the host attaches
    /// `shadow_pose_meshes` — reconstructed here step for step
    /// (packed streams → positions → pose bounds → bake). Keyed by the
    /// rig's rest hash, the `.skinao` scheme.
    #[test]
    fn sidecar_matches_off_thread_bake_for_real_rig() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../apps/sandbox/resources/models/kenney/mini-characters/character-female-a.glb",
        );
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("kenney assets absent — skipped");
            return;
        };
        let model = crate::skin::SkinnedModel::parse_glb(&bytes).expect("parse rig");
        let (idle, walk) = model.gait_clips().expect("gait clips");
        let sun = SunLight { dir: vec3f(0.55, 0.62, 0.56).normalize(), ..SunLight::default() };
        // The runtime seed path, step for step: host CPU-skins the pose set,
        // renderer extracts positions and bounds, background thread bakes.
        let meshes = model.shadow_pose_meshes(idle, walk);
        assert_eq!(meshes.len(), SDF_POSE_ROWS);
        let poses: Vec<Vec<Vec3f>> = meshes.iter().map(|m| packed_positions(m)).collect();
        let (min, max) = pose_bounds(&poses);
        let runtime = bake_silhouette_atlas(&poses, model.indices(), min, max, &sun)
            .expect("off-thread bake");
        assert_eq!(runtime.rows, SDF_POSE_ROWS);
        // The offline pipeline, hash-keyed like `.skinao`.
        let offline = bake_rig_atlas(&model, &sun).expect("offline bake");
        let (loaded, hash) = ShadowSdfAtlas::from_shadowsdf(
            &offline.to_shadowsdf(model.rest_hash()),
        )
        .expect("parse sidecar");
        assert_eq!(hash, model.rest_hash());
        assert!(
            loaded == runtime,
            "sidecar-loaded rig atlas differs from the off-thread bake"
        );
    }

    #[test]
    fn degenerate_meshes_bake_to_none() {
        assert!(bake_silhouette_atlas(
            &[],
            &[],
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
            &sun()
        )
        .is_none());
        let (idle, i) = walker(0.1);
        let flat: Vec<Vec3f> = idle.iter().map(|p| vec3f(0.0, p.y, 0.0)).collect();
        let (min, max) = bounds(&idle);
        // Idle itself degenerate: whole bake fails.
        assert!(bake_silhouette_atlas(&[flat], &i, min, max, &sun()).is_none());
    }
}
