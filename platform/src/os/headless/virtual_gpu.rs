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

/// A transformed vertex from the vertex shader: clip-space position + varyings.
#[derive(Clone)]
pub struct ShadedVertex {
    pub pos: [f32; 4],      // clip-space (x, y, z, w)
    pub varyings: Vec<f32>, // interpolated data for fragment shader
}

/// Per-triangle derivative data computed from barycentric coordinate gradients.
/// `dvary_dx[i]` = dVarying[i]/dScreenX, `dvary_dy[i]` = dVarying[i]/dScreenY.
pub struct TriangleDerivatives {
    pub dvary_dx: Vec<f32>,
    pub dvary_dy: Vec<f32>,
}

/// Rasterize a triangle with varying interpolation. For each covered pixel,
/// interpolates the varyings and calls `fragment_fn` to get the output color.
///
/// `fragment_fn(interpolated_varyings, derivatives) -> Option<[r, g, b, a]>`
/// Returns `None` for discard, `Some(color)` for premultiplied alpha color.
pub fn rasterize_triangle<F>(
    fb: &mut Framebuffer,
    v0: &ShadedVertex,
    v1: &ShadedVertex,
    v2: &ShadedVertex,
    fragment_fn: &mut F,
) where
    F: FnMut(&[f32], &TriangleDerivatives) -> Option<[f32; 4]>,
{
    let w = fb.width as f32;
    let h = fb.height as f32;

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

    let (sx0, sy0, sz0) = ndc_to_screen(&v0.pos);
    let (sx1, sy1, sz1) = ndc_to_screen(&v1.pos);
    let (sx2, sy2, sz2) = ndc_to_screen(&v2.pos);

    let area = edge(sx0, sy0, sx1, sy1, sx2, sy2);
    if area.abs() <= f32::EPSILON {
        return;
    }

    let min_x = sx0.min(sx1).min(sx2).floor().max(0.0) as i32;
    let min_y = sy0.min(sy1).min(sy2).floor().max(0.0) as i32;
    let max_x = sx0.max(sx1).max(sx2).ceil().min(w - 1.0) as i32;
    let max_y = sy0.max(sy1).max(sy2).ceil().min(h - 1.0) as i32;

    if max_x < min_x || max_y < min_y {
        return;
    }

    let vary_len = v0.varyings.len();
    let mut interp = vec![0.0f32; vary_len];

    // Compute per-triangle varying derivatives from barycentric coordinate gradients.
    // dw0/dx = (sy1 - sy2) / area, dw1/dx = (sy2 - sy0) / area, dw2/dx = (sy0 - sy1) / area
    // dw0/dy = (sx2 - sx1) / area, dw1/dy = (sx0 - sx2) / area, dw2/dy = (sx1 - sx0) / area
    // dvary[i]/dx = dw0/dx * v0[i] + dw1/dx * v1[i] + dw2/dx * v2[i]
    let inv_area = 1.0 / area;
    let dw0_dx = (sy1 - sy2) * inv_area;
    let dw1_dx = (sy2 - sy0) * inv_area;
    let dw2_dx = (sy0 - sy1) * inv_area;
    let dw0_dy = (sx2 - sx1) * inv_area;
    let dw1_dy = (sx0 - sx2) * inv_area;
    let dw2_dy = (sx1 - sx0) * inv_area;

    let mut derivs = TriangleDerivatives {
        dvary_dx: vec![0.0f32; vary_len],
        dvary_dy: vec![0.0f32; vary_len],
    };
    for i in 0..vary_len {
        derivs.dvary_dx[i] =
            dw0_dx * v0.varyings[i] + dw1_dx * v1.varyings[i] + dw2_dx * v2.varyings[i];
        derivs.dvary_dy[i] =
            dw0_dy * v0.varyings[i] + dw1_dy * v1.varyings[i] + dw2_dy * v2.varyings[i];
    }

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let w0 = edge(sx1, sy1, sx2, sy2, px, py) / area;
            let w1 = edge(sx2, sy2, sx0, sy0, px, py) / area;
            let w2 = edge(sx0, sy0, sx1, sy1, px, py) / area;

            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }

            let depth = sz0 * w0 + sz1 * w1 + sz2 * w2;
            let index = y as usize * fb.width + x as usize;

            // Depth test (less-or-equal for overlapping widgets with same zbias)
            if depth > fb.depth[index] {
                continue;
            }

            // Interpolate varyings
            for i in 0..vary_len {
                interp[i] = v0.varyings[i] * w0 + v1.varyings[i] * w1 + v2.varyings[i] * w2;
            }

            // Call fragment shader — returns None for discard
            let frag_color = match fragment_fn(&interp, &derivs) {
                Some(c) => c,
                None => continue,
            };

            // Premultiplied alpha blending (source-over)
            let src_a = frag_color[3];
            let inv_src_a = 1.0 - src_a;
            let dst = fb.color[index];
            fb.color[index] = [
                frag_color[0] + dst[0] * inv_src_a,
                frag_color[1] + dst[1] * inv_src_a,
                frag_color[2] + dst[2] * inv_src_a,
                frag_color[3] + dst[3] * inv_src_a,
            ];
            fb.depth[index] = depth;
        }
    }
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}
