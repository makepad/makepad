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
        let pixels = width * height;
        Self {
            width,
            height,
            color: vec![[0.0; 4]; pixels],
            depth: vec![1.0; pixels],
        }
    }

    pub fn clear(&mut self, color: [f32; 4], depth: f32) {
        self.color.fill(color);
        self.depth.fill(depth);
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

/// Rasterize only a row range `[row_start, row_end)` of the framebuffer.
/// `color`/`depth_buf` are row-contiguous slices sized `(row_end-row_start)*width`.
pub fn rasterize_triangle_rows<F>(
    width: usize,
    height: usize,
    row_start: usize,
    row_end: usize,
    color: &mut [[f32; 4]],
    depth_buf: &mut [f32],
    p0: &[f32; 4],
    vary0: &[f32],
    p1: &[f32; 4],
    vary1: &[f32],
    p2: &[f32; 4],
    vary2: &[f32],
    flat_slots: usize,
    compute_derivatives: bool,
    scratch: &mut RasterScratch,
    fragment_fn: &mut F,
) where
    F: FnMut(&[f32], &TriangleDerivatives, u32, u32, i32, i32) -> Option<[f32; 4]>,
{
    if width == 0 || height == 0 {
        return;
    }
    let row_start = row_start.min(height);
    let row_end = row_end.min(height);
    if row_start >= row_end {
        return;
    }
    let expected_len = (row_end - row_start) * width;
    if color.len() < expected_len || depth_buf.len() < expected_len {
        return;
    }
    if vary0.len() != vary1.len() || vary1.len() != vary2.len() {
        return;
    }

    let w = width as f32;
    let h = height as f32;

    // Convert from clip space [-1,1] to screen space [0, width/height].
    let ndc_to_screen = |pos: &[f32; 4]| -> (f32, f32, f32) {
        let inv_w = if pos[3] != 0.0 { 1.0 / pos[3] } else { 1.0 };
        let ndc_x = pos[0] * inv_w;
        let ndc_y = pos[1] * inv_w;
        let ndc_z = pos[2] * inv_w;
        let sx = (ndc_x * 0.5 + 0.5) * w;
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * h; // flip Y
        let sz = ndc_z * 0.5 + 0.5; // depth [0, 1]
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
    let mut vary_src = [vary0, vary1, vary2];

    // Ensure a positive area so a single top-left rule works for all triangles.
    let mut area = edge(sx[0], sy[0], sx[1], sy[1], sx[2], sy[2]);
    if area.abs() <= f32::EPSILON {
        return;
    }
    if area < 0.0 {
        sx.swap(1, 2);
        sy.swap(1, 2);
        sz.swap(1, 2);
        inv_clip_w.swap(1, 2);
        vary_src.swap(1, 2);
        area = -area;
    }

    let min_x = sx[0]
        .min(sx[1])
        .min(sx[2])
        .floor()
        .max(0.0) as i32;
    let min_y = sy[0]
        .min(sy[1])
        .min(sy[2])
        .floor()
        .max(row_start as f32) as i32;
    let max_x = sx[0]
        .max(sx[1])
        .max(sx[2])
        .ceil()
        .min(w - 1.0) as i32;
    let max_y = sy[0]
        .max(sy[1])
        .max(sy[2])
        .ceil()
        .min(row_end as f32 - 1.0) as i32;

    if max_x < min_x || max_y < min_y {
        return;
    }

    let vary_len = vary_src[0].len();
    let flat_slots = flat_slots.min(vary_len);
    scratch.ensure_vary_len(vary_len, compute_derivatives);
    let empty_derivs = TriangleDerivatives::default();

    let inv_area = 1.0 / area;

    // Edge increments for stepping one pixel in +x/+y.
    let e0_dx = sy[2] - sy[1];
    let e1_dx = sy[0] - sy[2];
    let e2_dx = sy[1] - sy[0];
    let e0_dy = sx[1] - sx[2];
    let e1_dy = sx[2] - sx[0];
    let e2_dy = sx[0] - sx[1];

    let top_left_0 = is_top_left(sx[1], sy[1], sx[2], sy[2]);
    let top_left_1 = is_top_left(sx[2], sy[2], sx[0], sy[0]);
    let top_left_2 = is_top_left(sx[0], sy[0], sx[1], sy[1]);

    let interpolate_perspective = |w0: f32, w1: f32, w2: f32, out: &mut [f32]| -> bool {
            let a0 = w0 * inv_clip_w[0];
            let a1 = w1 * inv_clip_w[1];
            let a2 = w2 * inv_clip_w[2];
            let denom = a0 + a1 + a2;
            if denom.abs() <= f32::EPSILON {
                return false;
            }
            let inv_denom = 1.0 / denom;
            for i in 0..vary_len {
                out[i] = (a0 * vary_src[0][i] + a1 * vary_src[1][i] + a2 * vary_src[2][i])
                    * inv_denom;
            }
            true
    };

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let e0 = edge(sx[1], sy[1], sx[2], sy[2], px, py);
            let e1 = edge(sx[2], sy[2], sx[0], sy[0], px, py);
            let e2 = edge(sx[0], sy[0], sx[1], sy[1], px, py);

            // GPU-like top-left rule avoids shared-edge gaps and overlaps.
            if !edge_pass(e0, top_left_0) || !edge_pass(e1, top_left_1) || !edge_pass(e2, top_left_2) {
                continue;
            }

            let w0 = e0 * inv_area;
            let w1 = e1 * inv_area;
            let w2 = e2 * inv_area;

            let depth = sz[0] * w0 + sz[1] * w1 + sz[2] * w2;
            let local_y = y as usize - row_start;
            let index = local_y * width + x as usize;

            // Depth test (less-or-equal for overlapping widgets with same zbias)
            if depth > depth_buf[index] {
                continue;
            }

            if !interpolate_perspective(w0, w1, w2, &mut scratch.interp[..vary_len]) {
                continue;
            }

            let lane_x = (x as u32) & 1;
            let lane_y = (y as u32) & 1;
            // Dyn/rust instance slots are constant across the primitive.
            // Keep them bit-stable (no interpolation drift) for shader equality tests.
            for i in 0..flat_slots {
                scratch.interp[i] = vary_src[0][i];
            }

            let frag_color = if compute_derivatives {
                // Build dFdx/dFdy-style deltas by evaluating at neighboring pixel centers.
                // GPU derivatives are pairwise across a 2x2 quad:
                // dFdx for odd x lanes uses (current - left), even x uses (right - current).
                // dFdy for odd y lanes uses (current - up), even y uses (down - current).
                let dx_sign = if lane_x == 0 { 1.0 } else { -1.0 };
                let dy_sign = if lane_y == 0 { 1.0 } else { -1.0 };

                let wx0 = (e0 + dx_sign * e0_dx) * inv_area;
                let wx1 = (e1 + dx_sign * e1_dx) * inv_area;
                let wx2 = (e2 + dx_sign * e2_dx) * inv_area;
                let wy0 = (e0 + dy_sign * e0_dy) * inv_area;
                let wy1 = (e1 + dy_sign * e1_dy) * inv_area;
                let wy2 = (e2 + dy_sign * e2_dy) * inv_area;

                if !interpolate_perspective(wx0, wx1, wx2, &mut scratch.interp_dx[..vary_len])
                    || !interpolate_perspective(wy0, wy1, wy2, &mut scratch.interp_dy[..vary_len])
                {
                    continue;
                }

                for i in 0..vary_len {
                    scratch.derivs.dvary_dx[i] = scratch.interp_dx[i] - scratch.interp[i];
                    scratch.derivs.dvary_dy[i] = scratch.interp_dy[i] - scratch.interp[i];
                }
                for i in 0..flat_slots {
                    scratch.derivs.dvary_dx[i] = 0.0;
                    scratch.derivs.dvary_dy[i] = 0.0;
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

            // Premultiplied alpha blending (source-over)
            let src_a = frag_color[3];
            let dst = color[index];
            color[index] = blend_premul_src_over(frag_color, dst);
            // Match common UI blending behavior: fully transparent pixels should
            // not occlude subsequent geometry in depth.
            if src_a > 0.02 {
                depth_buf[index] = depth;
            }
        }
    }
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
