/// A software rasterizer that interpolates float varyings and calls a fragment
/// shader callback per pixel.

pub struct Framebuffer {
    pub width: usize,
    pub height: usize,
    pub color: Vec<[f32; 4]>, // RGBA linear premultiplied
    pub depth: Vec<f32>,
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        Self::with_depth(width, height, true)
    }

    /// `depth: false` leaves the depth buffer unallocated — for a pass with no
    /// depth attachment, which is every data pass in a bake chain.
    pub fn with_depth(width: usize, height: usize, depth: bool) -> Self {
        let pixels = width * height;
        Self {
            width,
            height,
            color: vec![[0.0; 4]; pixels],
            depth: if depth { vec![1.0; pixels] } else { Vec::new() },
        }
    }

    pub fn has_depth(&self) -> bool {
        !self.depth.is_empty()
    }

    /// Allocate or release the depth buffer to match the pass's attachment.
    pub fn set_has_depth(&mut self, depth: bool) {
        match (depth, self.depth.is_empty()) {
            (true, true) => self.depth = vec![1.0; self.width * self.height],
            (false, false) => self.depth = Vec::new(),
            _ => {}
        }
    }

    pub fn bytes(&self) -> usize {
        self.color.len() * std::mem::size_of::<[f32; 4]>()
            + self.depth.len() * std::mem::size_of::<f32>()
    }

    pub fn clear(&mut self, color: [f32; 4], depth: f32) {
        self.color.fill(color);
        self.depth.fill(depth);
    }

    pub fn clear_color(&mut self, color: [f32; 4]) {
        self.color.fill(color);
    }

    pub fn clear_depth(&mut self, depth: f32) {
        self.depth.fill(depth);
    }


    /// Reuse this buffer at a new size. Only reallocates when the size really
    /// changed — an offscreen pass that redraws every frame at a steady size
    /// keeps one allocation for the life of the process.
    /// Returns true when the contents were discarded by the resize.
    pub fn resize(&mut self, width: usize, height: usize) -> bool {
        if self.width == width && self.height == height {
            return false;
        }
        let pixels = width * height;
        let had_depth = !self.depth.is_empty();
        self.width = width;
        self.height = height;
        self.color.clear();
        self.color.resize(pixels, [0.0; 4]);
        self.depth.clear();
        if had_depth {
            self.depth.resize(pixels, 1.0);
        }
        true
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height * 4];
        for (i, c) in self.color.iter().enumerate() {
            // c is premultiplied alpha - unpremultiply for PNG
            let a = c[3].clamp(0.0, 1.0);
            let inv_a = if a > 0.0 { 1.0 / a } else { 0.0 };
            let r = (c[0] * inv_a).clamp(0.0, 1.0);
            let g = (c[1] * inv_a).clamp(0.0, 1.0);
            let b = (c[2] * inv_a).clamp(0.0, 1.0);
            let base = i * 4;
            out[base] = (r * 255.0).round() as u8;
            out[base + 1] = (g * 255.0).round() as u8;
            out[base + 2] = (b * 255.0).round() as u8;
            out[base + 3] = (a * 255.0).round() as u8;
        }
        out
    }
}

/// Per-fragment derivative deltas.
/// `dvary_dx[i]` ~= varying(i) at (x+1,y) minus current varying(i),
/// `dvary_dy[i]` ~= varying(i) at (x,y+1) minus current varying(i).
#[derive(Default)]
pub struct TriangleDerivatives {
    pub dvary_dx: Vec<f32>,
    pub dvary_dy: Vec<f32>,
}

#[derive(Default)]
pub struct RasterScratch {
    pub interp: Vec<f32>,
    pub interp_dx: Vec<f32>,
    pub interp_dy: Vec<f32>,
    pub derivs: TriangleDerivatives,
}

impl RasterScratch {
    fn ensure_vary_len(&mut self, vary_len: usize, compute_derivatives: bool) {
        if self.interp.len() < vary_len {
            self.interp.resize(vary_len, 0.0);
        }
        if compute_derivatives {
            if self.interp_dx.len() < vary_len {
                self.interp_dx.resize(vary_len, 0.0);
            }
            if self.interp_dy.len() < vary_len {
                self.interp_dy.resize(vary_len, 0.0);
            }
            if self.derivs.dvary_dx.len() < vary_len {
                self.derivs.dvary_dx.resize(vary_len, 0.0);
            }
            if self.derivs.dvary_dy.len() < vary_len {
                self.derivs.dvary_dy.resize(vary_len, 0.0);
            }
        }
    }
}

/// Per-draw-call raster state the GPU backends put in the pipeline descriptor.
#[derive(Clone, Copy)]
pub struct RasterState {
    /// Premultiplied source-over blending. Off for the data-pass colour
    /// formats (`Bgra8NoBlend`, `Rf32`), where the fragment REPLACES the
    /// destination because alpha is payload, not opacity.
    pub blend: bool,
    /// Whether the attachment has a depth buffer at all. A pass without a depth
    /// attachment does no depth testing on a GPU — and here it also carries no
    /// depth allocation, which is a fifth of a large data target's memory.
    pub has_depth: bool,
    /// Whether fragments update the depth buffer.
    pub depth_write: bool,
    /// Clamp written components to [0,1] — what an 8-bit unorm attachment does
    /// on a GPU. Float attachments (`RenderRf32`, `RenderRGBAf16/f32`) keep the
    /// raw value, which is the whole point of using them.
    pub clamp_unorm: bool,
}

impl Default for RasterState {
    fn default() -> Self {
        Self {
            blend: true,
            has_depth: true,
            depth_write: true,
            clamp_unorm: true,
        }
    }
}

/// One triangle reduced to everything the per-pixel loop needs: screen-space
/// corners, edge functions, the perspective divisors and the pixel bounding box.
///
/// Setup is separated from rasterization because it is *per triangle*, while
/// rasterization is sliced *per row band* across threads. Doing it inline meant
/// every worker re-derived the same clip-space transform, winding fix and
/// bounding box for every triangle in the draw call — N threads paid N times the
/// setup to rasterize one Nth of the pixels, which is most of why the row-split
/// scaled so poorly. Now setup runs once and each band only visits the triangles
/// whose bounding box reaches it.
#[derive(Clone, Copy)]
pub struct TriSetup {
    sx: [f32; 3],
    sy: [f32; 3],
    sz: [f32; 3],
    inv_clip_w: [f32; 3],
    /// Vertex index per corner, after any winding swap, into the shaded arrays.
    vary_idx: [u32; 3],
    inv_area: f32,
    /// Edge deltas for a one-pixel step in +x / +y.
    e_dx: [f32; 3],
    e_dy: [f32; 3],
    top_left: [bool; 3],
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}

/// Below this the top-left tie-break applies; see [`edge_pass`].
const EDGE_EPS: f32 = 1.0e-6;

/// Project one triangle into screen space and bound it, or `None` when it is
/// degenerate or entirely outside the viewport.
///
/// `i0/i1/i2` are indices into `positions`; the returned setup carries them back
/// (possibly swapped, when the winding had to be flipped) so the caller can find
/// the matching varyings.
pub fn setup_triangle(
    width: usize,
    height: usize,
    viewport: (usize, usize),
    positions: &[[f32; 4]],
    i0: u32,
    i1: u32,
    i2: u32,
) -> Option<TriSetup> {
    if width == 0 || height == 0 {
        return None;
    }
    let p0 = positions.get(i0 as usize)?;
    let p1 = positions.get(i1 as usize)?;
    let p2 = positions.get(i2 as usize)?;

    // The viewport is anchored at the target's top-left corner and may be
    // smaller than the target itself (a `TextureSize::Fixed` attachment larger
    // than the pass rect) — clip space maps onto it, not onto the whole buffer,
    // exactly as `setViewport` does on the GPU backends.
    let vp_w = viewport.0.min(width).max(1);
    let vp_h = viewport.1.min(height).max(1);
    let w = vp_w as f32;
    let h = vp_h as f32;

    // Convert from clip space [-1,1] to screen space [0, width/height].
    let ndc_to_screen = |pos: &[f32; 4]| -> (f32, f32, f32) {
        let inv_w = if pos[3] != 0.0 { 1.0 / pos[3] } else { 1.0 };
        let ndc_x = pos[0] * inv_w;
        let ndc_y = pos[1] * inv_w;
        let ndc_z = pos[2] * inv_w;
        let sx = (ndc_x * 0.5 + 0.5) * w;
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h; // flip Y
                                                  // Makepad shaders output depth in [0, 1] clip space in practice.
                                                  // Keep it as-is to avoid collapsing depth precision.
        let sz = ndc_z;
        (sx, sy, sz)
    };

    let (sx0, sy0, sz0) = ndc_to_screen(p0);
    let (sx1, sy1, sz1) = ndc_to_screen(p1);
    let (sx2, sy2, sz2) = ndc_to_screen(p2);

    let mut sx = [sx0, sx1, sx2];
    let mut sy = [sy0, sy1, sy2];
    let mut sz = [sz0, sz1, sz2];
    let mut inv_clip_w = [
        if p0[3].abs() > f32::EPSILON {
            1.0 / p0[3]
        } else {
            0.0
        },
        if p1[3].abs() > f32::EPSILON {
            1.0 / p1[3]
        } else {
            0.0
        },
        if p2[3].abs() > f32::EPSILON {
            1.0 / p2[3]
        } else {
            0.0
        },
    ];
    let mut vary_idx = [i0, i1, i2];

    // Ensure a positive area so a single top-left rule works for all triangles.
    let mut area = edge(sx[0], sy[0], sx[1], sy[1], sx[2], sy[2]);
    if area.abs() <= f32::EPSILON {
        return None;
    }
    if area < 0.0 {
        sx.swap(1, 2);
        sy.swap(1, 2);
        sz.swap(1, 2);
        inv_clip_w.swap(1, 2);
        vary_idx.swap(1, 2);
        area = -area;
    }

    let min_x = sx[0].min(sx[1]).min(sx[2]).floor().max(0.0) as i32;
    let min_y = sy[0].min(sy[1]).min(sy[2]).floor().max(0.0) as i32;
    let max_x = sx[0].max(sx[1]).max(sx[2]).ceil().min(w - 1.0) as i32;
    let max_y = sy[0].max(sy[1]).max(sy[2]).ceil().min(h - 1.0) as i32;

    if max_x < min_x || max_y < min_y {
        return None;
    }

    Some(TriSetup {
        sx,
        sy,
        sz,
        inv_clip_w,
        vary_idx,
        inv_area: 1.0 / area,
        // Edge increments for stepping one pixel in +x/+y.
        e_dx: [sy[2] - sy[1], sy[0] - sy[2], sy[1] - sy[0]],
        e_dy: [sx[1] - sx[2], sx[2] - sx[0], sx[0] - sx[1]],
        top_left: [
            is_top_left(sx[1], sy[1], sx[2], sy[2]),
            is_top_left(sx[2], sy[2], sx[0], sy[0]),
            is_top_left(sx[0], sy[0], sx[1], sy[1]),
        ],
        min_x,
        max_x,
        min_y,
        max_y,
    })
}

/// Rasterize one prepared triangle into the row range `[row_start, row_end)`.
/// `color`/`depth_buf` are row-contiguous slices sized `(row_end-row_start)*width`,
/// and `varyings` is the whole shaded-varying array indexed by `vary_idx`.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_setup_rows<F>(
    tri: &TriSetup,
    width: usize,
    state: RasterState,
    row_start: usize,
    row_end: usize,
    color: &mut [[f32; 4]],
    depth_buf: &mut [f32],
    varyings: &[f32],
    vary_len: usize,
    flat_slots: usize,
    compute_derivatives: bool,
    scratch: &mut RasterScratch,
    fragment_fn: &mut F,
) where
    F: FnMut(&[f32], &TriangleDerivatives, u32, u32, i32, i32) -> Option<[f32; 4]>,
{
    let min_y = tri.min_y.max(row_start as i32);
    let max_y = tri.max_y.min(row_end as i32 - 1);
    if max_y < min_y || tri.max_x < tri.min_x {
        return;
    }
    let expected_len = (row_end - row_start) * width;
    if color.len() < expected_len || (state.has_depth && depth_buf.len() < expected_len) {
        return;
    }

    let sx = &tri.sx;
    let sy = &tri.sy;
    let sz = &tri.sz;
    let inv_area = tri.inv_area;

    // Resolve the three corners' varying rows once per triangle.
    let (Some(vary0), Some(vary1), Some(vary2)) = (
        vary_row(varyings, tri.vary_idx[0], vary_len),
        vary_row(varyings, tri.vary_idx[1], vary_len),
        vary_row(varyings, tri.vary_idx[2], vary_len),
    ) else {
        return;
    };
    let vary_src = [vary0, vary1, vary2];

    let flat_slots = flat_slots.min(vary_len);
    scratch.ensure_vary_len(vary_len, compute_derivatives);
    let empty_derivs = TriangleDerivatives::default();

    // Dyn/rust instance slots are constant across the primitive, so they are
    // written once per triangle rather than re-copied (and, worse, first
    // interpolated) at every pixel. Their derivatives are identically zero.
    scratch.interp[..flat_slots].copy_from_slice(&vary_src[0][..flat_slots]);
    if compute_derivatives {
        scratch.derivs.dvary_dx[..flat_slots].fill(0.0);
        scratch.derivs.dvary_dy[..flat_slots].fill(0.0);
    }

    // Only the non-flat tail is interpolated; the flat head is already correct.
    let interp_lo = flat_slots;
    let interpolate_perspective = |w0: f32, w1: f32, w2: f32, out: &mut [f32]| -> bool {
        let a0 = w0 * tri.inv_clip_w[0];
        let a1 = w1 * tri.inv_clip_w[1];
        let a2 = w2 * tri.inv_clip_w[2];
        let denom = a0 + a1 + a2;
        if denom.abs() <= f32::EPSILON {
            return false;
        }
        let inv_denom = 1.0 / denom;
        let (s0, s1, s2) = (
            &vary_src[0][interp_lo..vary_len],
            &vary_src[1][interp_lo..vary_len],
            &vary_src[2][interp_lo..vary_len],
        );
        let out = &mut out[interp_lo..vary_len];
        for i in 0..out.len() {
            out[i] = (a0 * s0[i] + a1 * s1[i] + a2 * s2[i]) * inv_denom;
        }
        true
    };

    let [e0_dx, e1_dx, e2_dx] = tri.e_dx;
    let [e0_dy, e1_dy, e2_dy] = tri.e_dy;
    let [top_left_0, top_left_1, top_left_2] = tri.top_left;

    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        let px0 = tri.min_x as f32 + 0.5;

        // Edge values at the first pixel of the row; every further pixel is one
        // add away, which replaces three two-multiply edge evaluations per pixel.
        let e0_row = edge(sx[1], sy[1], sx[2], sy[2], px0, py);
        let e1_row = edge(sx[2], sy[2], sx[0], sy[0], px0, py);
        let e2_row = edge(sx[0], sy[0], sx[1], sy[1], px0, py);

        // Clip the row to the span where all three edges can still pass, so the
        // scan skips the (up to half a bounding box of) pixels that a per-pixel
        // test would only reject. The exact `edge_pass` test below still decides
        // every pixel — the span is deliberately conservative, so it can never
        // change which pixels are covered.
        let Some((x_lo, x_hi)) = row_span(
            tri.min_x,
            tri.max_x,
            [e0_row, e1_row, e2_row],
            [e0_dx, e1_dx, e2_dx],
        ) else {
            continue;
        };

        let local_y = y as usize - row_start;
        let row_base = local_y * width;

        for x in x_lo..=x_hi {
            // Evaluated, not accumulated. Stepping the edge functions along the
            // row is cheaper, but the accumulated rounding drifts far enough
            // over a 2000-pixel row to flip the coverage test on pixels that sit
            // exactly on an edge — a visible one-pixel seam, for a saving that
            // is noise next to the fragment shader call below.
            let px = x as f32 + 0.5;
            let ce0 = edge(sx[1], sy[1], sx[2], sy[2], px, py);
            let ce1 = edge(sx[2], sy[2], sx[0], sy[0], px, py);
            let ce2 = edge(sx[0], sy[0], sx[1], sy[1], px, py);

            // GPU-like top-left rule avoids shared-edge gaps and overlaps.
            if !edge_pass(ce0, top_left_0)
                || !edge_pass(ce1, top_left_1)
                || !edge_pass(ce2, top_left_2)
            {
                continue;
            }

            let w0 = ce0 * inv_area;
            let w1 = ce1 * inv_area;
            let w2 = ce2 * inv_area;

            let depth = sz[0] * w0 + sz[1] * w1 + sz[2] * w2;
            let index = row_base + x as usize;

            // Depth test (less-or-equal for overlapping widgets with same zbias)
            if state.has_depth && depth > depth_buf[index] {
                continue;
            }

            if !interpolate_perspective(w0, w1, w2, &mut scratch.interp[..vary_len]) {
                continue;
            }

            let lane_x = (x as u32) & 1;
            let lane_y = (y as u32) & 1;

            let frag_color = if compute_derivatives {
                // Build dFdx/dFdy-style deltas by evaluating at neighboring pixel centers.
                // GPU derivatives are pairwise across a 2x2 quad:
                // dFdx for odd x lanes uses (current - left), even x uses (right - current).
                // dFdy for odd y lanes uses (current - up), even y uses (down - current).
                let dx_sign = if lane_x == 0 { 1.0 } else { -1.0 };
                let dy_sign = if lane_y == 0 { 1.0 } else { -1.0 };

                let wx0 = (ce0 + dx_sign * e0_dx) * inv_area;
                let wx1 = (ce1 + dx_sign * e1_dx) * inv_area;
                let wx2 = (ce2 + dx_sign * e2_dx) * inv_area;
                let wy0 = (ce0 + dy_sign * e0_dy) * inv_area;
                let wy1 = (ce1 + dy_sign * e1_dy) * inv_area;
                let wy2 = (ce2 + dy_sign * e2_dy) * inv_area;

                if !interpolate_perspective(wx0, wx1, wx2, &mut scratch.interp_dx[..vary_len])
                    || !interpolate_perspective(wy0, wy1, wy2, &mut scratch.interp_dy[..vary_len])
                {
                    continue;
                }

                let (interp, dx, dy) = (
                    &scratch.interp[flat_slots..vary_len],
                    &scratch.interp_dx[flat_slots..vary_len],
                    &scratch.interp_dy[flat_slots..vary_len],
                );
                let (ddx, ddy) = (
                    &mut scratch.derivs.dvary_dx[flat_slots..vary_len],
                    &mut scratch.derivs.dvary_dy[flat_slots..vary_len],
                );
                for i in 0..ddx.len() {
                    ddx[i] = dx[i] - interp[i];
                    ddy[i] = dy[i] - interp[i];
                }

                match fragment_fn(
                    &scratch.interp[..vary_len],
                    &scratch.derivs,
                    lane_x,
                    lane_y,
                    x,
                    y,
                ) {
                    Some(c) => c,
                    None => continue,
                }
            } else {
                match fragment_fn(
                    &scratch.interp[..vary_len],
                    &empty_derivs,
                    lane_x,
                    lane_y,
                    x,
                    y,
                ) {
                    Some(c) => c,
                    None => continue,
                }
            };

            let frag_color = if state.clamp_unorm {
                [
                    frag_color[0].clamp(0.0, 1.0),
                    frag_color[1].clamp(0.0, 1.0),
                    frag_color[2].clamp(0.0, 1.0),
                    frag_color[3].clamp(0.0, 1.0),
                ]
            } else {
                frag_color
            };
            let src_a = frag_color[3];
            if state.blend {
                let dst = color[index];
                color[index] = blend_premul_src_over(frag_color, dst);
            } else {
                // Blending disabled: a raw component write, alpha included.
                color[index] = frag_color;
            }
            // Match common UI blending behavior: fully transparent pixels
            // should not occlude subsequent geometry in depth. A data pass
            // has no such convention — its alpha is payload, so every
            // surviving fragment owns the depth slot.
            if state.has_depth && state.depth_write && (!state.blend || src_a > 0.02) {
                depth_buf[index] = depth;
            }
        }
    }
}

#[inline]
fn vary_row(varyings: &[f32], idx: u32, vary_len: usize) -> Option<&[f32]> {
    if vary_len == 0 {
        return Some(&[]);
    }
    let off = (idx as usize).checked_mul(vary_len)?;
    varyings.get(off..off.checked_add(vary_len)?)
}

/// The `[x_lo, x_hi]` sub-range of `[min_x, max_x]` that can still satisfy all
/// three edge tests on this row, or `None` when the row misses the triangle.
///
/// Conservative by one pixel on each side: the caller keeps the exact per-pixel
/// `edge_pass` test, so this only ever skips pixels that would have been
/// rejected anyway and can never change coverage.
#[inline]
fn row_span(min_x: i32, max_x: i32, e: [f32; 3], e_dx: [f32; 3]) -> Option<(i32, i32)> {
    let mut lo = min_x;
    let mut hi = max_x;
    for i in 0..3 {
        if e_dx[i] == 0.0 {
            // Constant across the row: it either passes everywhere or nowhere.
            if e[i] < -EDGE_EPS {
                return None;
            }
            continue;
        }
        // Pass needs e[i] + t*e_dx[i] >= -EDGE_EPS, with t = x - min_x. The
        // float->int cast saturates, and the offsets are saturating, so a
        // near-horizontal edge simply leaves the bound where it was.
        let t = (-EDGE_EPS - e[i]) / e_dx[i];
        if e_dx[i] > 0.0 {
            lo = lo.max(min_x.saturating_add(t.ceil() as i32).saturating_sub(1));
        } else {
            hi = hi.min(min_x.saturating_add(t.floor() as i32).saturating_add(1));
        }
    }
    let lo = lo.max(min_x);
    let hi = hi.min(max_x);
    (lo <= hi).then_some((lo, hi))
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

#[inline]
fn is_top_left(ax: f32, ay: f32, bx: f32, by: f32) -> bool {
    let dy = by - ay;
    let dx = bx - ax;
    // Screen-space Y grows downward, so top-left differs from Y-up convention.
    dy > 0.0 || (dy == 0.0 && dx < 0.0)
}

#[inline]
fn edge_pass(edge_value: f32, top_left: bool) -> bool {
    const EDGE_EPS: f32 = 1.0e-6;
    if edge_value < -EDGE_EPS {
        false
    } else if edge_value > 0.0 {
        true
    } else {
        top_left
    }
}

#[inline]
fn blend_premul_src_over(src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    let inv_src_a = 1.0 - src[3];
    [
        src[0] + dst[0] * inv_src_a,
        src[1] + dst[1] * inv_src_a,
        src[2] + dst[2] * inv_src_a,
        src[3] + dst[3] * inv_src_a,
    ]
}
