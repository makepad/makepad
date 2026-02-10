use crate::{cx_2d::*, draw_list_2d::ManyInstances, makepad_platform::*, turtle::*, vector::*};
use makepad_svg::tessellate::compute_clip_radii;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawVector = mod.std.set_type_default() do #(DrawVector::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.VectorVertex, geom.VectorGeom)
        gradient_texture: texture_2d(float)

        v_tcoord: varying(vec2f)
        v_world: varying(vec2f)
        v_color: varying(vec4f)
        v_stroke_mult: varying(float)
        v_stroke_dist: varying(float)
        v_shape_id: varying(float)
        v_param0: varying(float)
        v_param1: varying(float)
        v_param2: varying(float)
        v_param3: varying(float)
        v_param4: varying(float)
        v_param5: varying(float)

        vertex: fn() {
            let pos = vec2(self.geom.x, self.geom.y);
            self.v_tcoord = vec2(self.geom.u, self.geom.v);
            self.v_color = vec4(self.geom.color_r, self.geom.color_g, self.geom.color_b, self.geom.color_a);
            self.v_stroke_mult = self.geom.stroke_mult;
            self.v_stroke_dist = self.geom.stroke_dist;
            self.v_shape_id = self.geom.shape_id;
            self.v_param0 = self.geom.param0;
            self.v_param1 = self.geom.param1;
            self.v_param2 = self.geom.param2;
            self.v_param3 = self.geom.param3;
            self.v_param4 = self.geom.param4;
            self.v_param5 = self.geom.param5;
            let shifted = pos + self.draw_list.view_shift;
            self.v_world = shifted;

            // Early clip rejection: merge both clip rects (in local space), single check
            let cr = self.geom.clip_radius;
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            );
            if pos.x + cr < clip.x || pos.y + cr < clip.y
                || pos.x - cr > clip.z || pos.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }

            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.draw_call.zbias + self.geom.zbias
                1.
            );
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }

        fragment: fn(){
            self.fb0 = self.pixel()
        }

        // --- Shadow utilities ---

        // Attempt at erf approx via Raph Levien (reciprocal sqrt form, GPU-friendly)
        erf7: fn(x: float) -> float {
            let s = x * 1.1283791671;
            let ss = s * s;
            let t = s + (0.24295 + (0.03395 + 0.0104 * ss) * ss) * (s * ss);
            return t / sqrt(1.0 + t * t)
        }

        // 1D integral of blurred step: integral of gaussian from -inf to x
        // Returns smooth 0..1 transition over blur_radius
        blur_step: fn(x: float, blur: float) -> float {
            return 0.5 + 0.5 * self.erf7(x * (sqrt(0.5) / blur))
        }

        // 1D blurred box: integral of gaussian-convolved box [-half..+half]
        blur_box: fn(x: float, half_size: float, blur: float) -> float {
            return self.blur_step(x + half_size, blur) - self.blur_step(x - half_size, blur)
        }

        // Blurred rounded rect evaluated at point p relative to rect center.
        // half = half extents, corner = corner radius, blur = blur sigma.
        // Returns 0..1 coverage.
        // Uses the Raph Levien approach: SDF + erf for the blur.
        shadow_rounded_rect: fn(p: vec2, half_ext: vec2, corner: float, blur: float) -> float {
            // signed distance to rounded rect
            let q = abs(p) - half_ext + vec2(corner, corner);
            let d = min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0, 0.0))) - corner;
            // map distance through blurred step (erf)
            return self.blur_step(-d, max(blur, 0.001))
        }

        // --- Utility functions (use these in get_color / get_stroke_mask) ---

        // Returns 1.0 if this pixel is a fill, 0.0 if stroke
        is_fill: fn() -> float {
            if self.v_stroke_mult > 1e5 { return 1.0 }
            return 0.0
        }

        // AA-correct dash pattern. on_px = visible length, off_px = gap length.
        // Returns 0.0 in gaps, 1.0 in dashes, with smooth AA transitions.
        dash: fn(on_px: float, off_px: float) -> float {
            let total = on_px + off_px;
            let phase = modf(self.v_stroke_dist, total);
            let fw = length(vec2(dFdx(self.v_stroke_dist), dFdy(self.v_stroke_dist)));
            return clamp((on_px - phase) / max(fw, 0.001), 0.0, 1.0)
                * clamp(phase / max(fw, 0.001), 0.0, 1.0)
        }

        // AA-correct dot pattern. spacing = center-to-center distance.
        // Returns 0.0 between dots, 1.0 on dots.
        dots: fn(spacing: float) -> float {
            let phase = modf(self.v_stroke_dist, spacing);
            let fw = length(vec2(dFdx(self.v_stroke_dist), dFdy(self.v_stroke_dist)));
            let mid = spacing * 0.5;
            return 1.0 - clamp((mid - abs(phase - mid)) / max(fw, 0.001), 0.0, 1.0)
        }

        // --- Overridable hooks ---

        // Evaluate gradient color per-pixel from world position.
        // v_param0 encodes gradient type: 0=solid, 1=linear, 2=radial
        // For linear: param1,2 = start point, param3,4 = end point
        // For radial: param1,2 = center, param3 = radius
        // Sample gradient color from texture row at parameter t.
        // v_param5 encodes the row as a normalized V coordinate (0 = no texture, >0 = row).
        sample_gradient: fn(t: float) -> vec4 {
            let row_v = self.v_param5;
            if row_v > 0.0001 {
                // Inset UV by half-texel to avoid wrapping at edges
                let tex_size = self.gradient_texture.size();
                let half_u = 0.5 / tex_size.x;
                let half_v = 0.5 / tex_size.y;
                let u = clamp(t, 0.0, 1.0) * (1.0 - 2.0 * half_u) + half_u;
                let v = clamp(row_v, half_v, 1.0 - half_v);
                return self.gradient_texture.sample(vec2(u, v))
            }
            // No gradient texture row — return solid vertex color
            return self.v_color
        }

        eval_gradient: fn() {
            let grad_type = self.v_param0;
            // solid or shadow: just return baked vertex color
            if grad_type < 0.5 { return self.v_color }
            // linear gradient: t = dot(world - p0, p1 - p0) / |p1 - p0|^2
            if grad_type < 1.5 {
                let p0 = vec2(self.v_param1, self.v_param2);
                let p1 = vec2(self.v_param3, self.v_param4);
                let d = p1 - p0;
                let len2 = dot(d, d);
                var t = 0.0;
                if len2 > 0.000001 {
                    t = clamp(dot(self.v_world - p0, d) / len2, 0.0, 1.0)
                }
                return self.sample_gradient(t)
            }
            // radial gradient: t = elliptical distance from center
            // param1,2 = center, param3 = rx, param4 = ry
            let center = vec2(self.v_param1, self.v_param2);
            let rx = self.v_param3;
            let ry = self.v_param4;
            let d = self.v_world - center;
            var t = 0.0;
            if rx > 0.000001 && ry > 0.000001 {
                t = clamp(length(vec2(d.x / rx, d.y / ry)), 0.0, 1.0)
            }
            return self.sample_gradient(t)
        }

        // Override to customize fill/stroke color per pixel.
        // Access: v_color (baked vertex color), v_world (world position),
        //         v_shape_id (user-defined shape identifier)
        get_color: fn() {
            return self.eval_gradient()
        }

        // Override to customize stroke appearance (dashes, dots, etc).
        // Access: v_stroke_dist (distance along path), v_shape_id
        // Use self.dash(on, off) or self.dots(spacing) helpers.
        // Return 1.0 = fully visible, 0.0 = hidden.
        get_stroke_mask: fn() {
            return 1.0
        }

        // --- Core pixel shader (usually don't override this) ---

        pixel: fn(){
            // Clip against merged draw_clip + view_clip (in local space)
            let local = self.v_world - self.draw_list.view_shift;
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            );
            if local.x < clip.x || local.y < clip.y
                || local.x > clip.z || local.y > clip.w {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            // geometry shadow mode: stroke_mult == -2.0
            // v interpolates 1.0 (edge) to 0.0 (3*blur out), stroke_dist = blur
            if self.v_stroke_mult < -1.5 {
                let blur = self.v_stroke_dist;
                // v=1.0 means distance=0 (at edge), v=0.0 means distance=3*blur
                let dist = (1.0 - self.v_tcoord.y) * 3.0 * blur;
                let alpha = self.blur_step(-dist, max(blur, 0.001));
                return self.v_color * alpha
            }
            // rect shadow mode: stroke_mult == -1.0
            // params: param0=cx, param1=cy, param2=hx, param3=hy, param4=corner, param5=blur
            if self.v_stroke_mult < -0.5 {
                let shadow_center = vec2(self.v_param0, self.v_param1);
                let shadow_half = vec2(self.v_param2, self.v_param3);
                let shadow_corner = self.v_param4;
                let shadow_blur = self.v_param5;
                let p = self.v_world - shadow_center;
                let alpha = self.shadow_rounded_rect(p, shadow_half, shadow_corner, shadow_blur);
                return self.v_color * alpha
            }
            let color = self.get_color();
            var alpha = 0.0;
            if self.v_stroke_mult > 1e5 {
                let d = self.v_tcoord.x * 2.0;
                let fw = length(vec2(dFdx(d), dFdy(d)));
                alpha = clamp(d / max(fw, 0.001), 0.0, 1.0);
            } else {
                let sd = 1.0 - abs(self.v_tcoord.x * 2.0 - 1.0);
                let fw = length(vec2(dFdx(sd), dFdy(sd)));
                alpha = clamp(sd / max(fw, 0.001), 0.0, 1.0) * min(1.0, self.v_tcoord.y) * self.get_stroke_mask();
            }
            return color * alpha
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawVector {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust]
    pub geometry: Option<Geometry>,
    #[rust]
    pub path: VectorPath,
    #[rust]
    pub tess: Tessellator,
    // reusable scratch buffers for tessellation output
    #[rust]
    tess_verts: Vec<VVertex>,
    #[rust]
    tess_indices: Vec<u32>,
    // accumulated geometry for the entire picture
    #[rust]
    pub acc_verts: Vec<f32>,
    #[rust]
    pub acc_indices: Vec<u32>,
    // current paint state
    #[rust]
    pub cur_paint: VectorPaint,
    #[rust]
    pub cur_stroke_mult: f32,
    #[rust]
    pub cur_shape_id: f32,
    #[rust]
    pub cur_zbias: f32,
    #[rust]
    pub cur_gradient_row_v: f32,
    // Effect bounding box (world-space): [min_x, min_y, max_x, max_y]
    // When set, stored in param1-param4 for solid-painted shapes with shader_id > 0,
    // enabling the pixel shader to compute proper UV coordinates from v_world.
    #[rust]
    pub cur_effect_bbox: Option<[f32; 4]>,
    /// Inherited CSS `color` override from a `<use>` element. When set,
    /// `currentColor` paint values inside symbols resolve to this color.
    #[rust]
    pub cur_use_color: Option<(f32, f32, f32, f32)>,
    // gradient texture: Nx2048 BGRA, one row per gradient
    #[rust]
    pub gradient_texture_data: Vec<u32>,
    #[rust]
    pub gradient_row_count: usize,
    #[rust]
    pub gradient_texture: Option<Texture>,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub rect_pos: Vec2f,
    #[live]
    pub rect_size: Vec2f,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub draw_depth: f32,
    #[live]
    pub pad1: f32,
    #[live]
    pub pad2: f32,
    #[live]
    pub pad3: f32,
}

impl DrawVector {
    // Path building
    pub fn clear(&mut self) {
        self.path.clear();
    }
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x, y);
    }
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x, y);
    }
    pub fn bezier_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, x: f32, y: f32) {
        self.path.bezier_to(cx1, cy1, cx2, cy2, x, y);
    }
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(cx, cy, x, y);
    }
    pub fn close(&mut self) {
        self.path.close();
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.path.rect(x, y, w, h);
    }
    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        self.path.rounded_rect(x, y, w, h, r);
    }
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.path.circle(cx, cy, r);
    }
    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32) {
        self.path.ellipse(cx, cy, rx, ry);
    }

    // Paint setters
    pub fn set_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.cur_paint = VectorPaint::solid(r, g, b, a);
    }

    pub fn set_color_hex(&mut self, hex: u32, alpha: f32) {
        self.cur_paint = VectorPaint::from_hex(hex, alpha);
    }

    pub fn set_paint(&mut self, paint: VectorPaint) {
        self.cur_paint = paint;
    }

    pub fn set_shape_id(&mut self, id: f32) {
        self.cur_shape_id = id;
    }

    /// Start accumulating a vector picture
    pub fn begin(&mut self) {
        self.acc_verts.clear();
        self.acc_indices.clear();
        self.gradient_texture_data.clear();
        self.gradient_row_count = 0;
        self.cur_gradient_row_v = -1.0; // sentinel: no gradient texture row
        self.cur_zbias = 0.0;
        self.cur_effect_bbox = None;
        self.cur_use_color = None;
    }

    /// Rasterize gradient stops into a new texture row.
    /// Returns the normalized V coordinate for sampling this row.
    pub fn add_gradient_row(&mut self, stops: &[GradientStop]) -> f32 {
        const TEX_WIDTH: usize = 2048;
        let row = self.gradient_row_count;
        self.gradient_row_count += 1;

        // Rasterize stops into TEX_WIDTH pixels
        for i in 0..TEX_WIDTH {
            let t = i as f32 / (TEX_WIDTH - 1) as f32;
            let (r, g, b, a) = sample_gradient_stops(stops, t);
            // Pack as BGRA u32 (premultiplied alpha already in stops)
            let rb = (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            let rg = (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            let rr = (r.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            let ra = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            self.gradient_texture_data
                .push(rb | (rg << 8) | (rr << 16) | (ra << 24));
        }

        // Return center of the row in normalized texture V coordinates.
        // We'll finalize the actual texture height in end().
        // For now, store the row index; we convert to V in end().
        row as f32
    }

    pub fn stroke(&mut self, stroke_width: f32) {
        self.stroke_opts(stroke_width, LineCap::Butt, LineJoin::Miter, 4.0, 1.0);
    }

    pub fn stroke_opts(
        &mut self,
        stroke_width: f32,
        cap: LineCap,
        join: LineJoin,
        miter_limit: f32,
        aa: f32,
    ) {
        let mut tv = std::mem::take(&mut self.tess_verts);
        let mut ti = std::mem::take(&mut self.tess_indices);
        self.cur_stroke_mult = tessellate_path_stroke(
            &mut self.path,
            &mut self.tess,
            &mut tv,
            &mut ti,
            stroke_width,
            cap,
            join,
            miter_limit,
            aa,
        );
        self.append_geometry(&tv, &ti);
        self.tess_verts = tv;
        self.tess_indices = ti;
    }

    pub fn fill(&mut self) {
        self.fill_opts(LineJoin::Miter, 4.0, 1.0);
    }

    /// Fill with GPU-expandable fringe encoding (used by DrawSvg cache remapping).
    pub fn fill_gpu(&mut self) {
        self.fill_opts_mode(LineJoin::Miter, 4.0, 1.0, true);
    }

    pub fn fill_opts(&mut self, join: LineJoin, miter_limit: f32, aa: f32) {
        self.fill_opts_mode(join, miter_limit, aa, false);
    }

    fn fill_opts_mode(&mut self, join: LineJoin, miter_limit: f32, aa: f32, gpu_expand_fill: bool) {
        let mut tv = std::mem::take(&mut self.tess_verts);
        let mut ti = std::mem::take(&mut self.tess_indices);
        tessellate_path_fill(
            &mut self.path,
            &mut self.tess,
            &mut tv,
            &mut ti,
            join,
            miter_limit,
            aa,
            gpu_expand_fill,
        );
        self.cur_stroke_mult = 1e6;
        self.append_geometry(&tv, &ti);
        self.tess_verts = tv;
        self.tess_indices = ti;
    }

    /// Draw a geometry-based shadow for any filled path (stars, polygons, etc).
    /// Uses tessellation with a wide fringe + erf blur in the pixel shader.
    /// Build your path first, then call shape_shadow(blur).
    pub fn shape_shadow(&mut self, blur: f32) {
        self.tess.flatten(&self.path, 0.25);
        let mut tv = std::mem::take(&mut self.tess_verts);
        let mut ti = std::mem::take(&mut self.tess_indices);
        self.tess
            .fill_shadow(blur, LineJoin::Bevel, 1.0, &mut tv, &mut ti);
        compute_clip_radii(&mut tv, &ti);
        self.cur_stroke_mult = -2.0; // sentinel for geometry shadow mode
        self.append_geometry(&tv, &ti);
        self.tess_verts = tv;
        self.tess_indices = ti;
        self.path.clear();
    }

    /// Draw a blurred rounded-rect shadow.
    /// (x,y,w,h) = rect, corner = corner radius, blur = blur sigma,
    /// offset_x/y = shadow offset. Uses current paint color.
    pub fn shadow(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        corner: f32,
        blur: f32,
        offset_x: f32,
        offset_y: f32,
    ) {
        let cx = x + w * 0.5 + offset_x;
        let cy = y + h * 0.5 + offset_y;
        let hx = w * 0.5;
        let hy = h * 0.5;
        // pad the quad by 3*blur so the gaussian tail is captured
        let pad = blur * 3.0;
        let x0 = cx - hx - pad;
        let y0 = cy - hy - pad;
        let x1 = cx + hx + pad;
        let y1 = cy + hy + pad;
        let color = self.cur_paint.color_at(cx, cy);
        let base = (self.acc_verts.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
        // emit 4 corner verts for the shadow quad
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        for &(vx, vy) in &corners {
            self.acc_verts.push(vx); // x
            self.acc_verts.push(vy); // y
            self.acc_verts.push(0.5); // u (unused for shadow)
            self.acc_verts.push(1.0); // v
            self.acc_verts.push(color[0]);
            self.acc_verts.push(color[1]);
            self.acc_verts.push(color[2]);
            self.acc_verts.push(color[3]);
            self.acc_verts.push(-1.0); // stroke_mult = -1 signals rect shadow mode
            self.acc_verts.push(0.0); // stroke_dist
            self.acc_verts.push(self.cur_shape_id);
            // params: cx, cy, hx, hy, corner, blur
            self.acc_verts.push(cx);
            self.acc_verts.push(cy);
            self.acc_verts.push(hx);
            self.acc_verts.push(hy);
            self.acc_verts.push(corner);
            self.acc_verts.push(blur);
            // clip_radius: 0 = don't use for clip rejection on shadow quads
            self.acc_verts.push(0.0);
            self.acc_verts.push(self.cur_zbias);
        }
        // two triangles
        self.acc_indices.push(base);
        self.acc_indices.push(base + 1);
        self.acc_indices.push(base + 2);
        self.acc_indices.push(base);
        self.acc_indices.push(base + 2);
        self.acc_indices.push(base + 3);
    }

    fn append_geometry(&mut self, verts: &[VVertex], indices: &[u32]) {
        if verts.is_empty() || indices.is_empty() {
            return;
        }
        // compute gradient params and color from current paint
        let (grad_type, grad_params, color0) = match &self.cur_paint {
            VectorPaint::Solid { color } => {
                // For shapes with shader effects, store the world-space bounding box
                // in param1-param4 so the pixel shader can compute proper UVs.
                let params = if let Some(bbox) = self.cur_effect_bbox {
                    bbox
                } else {
                    [0.0; 4]
                };
                (0.0, params, *color)
            }
            VectorPaint::LinearGradient {
                x0,
                y0,
                x1,
                y1,
                stops,
            } => {
                let c0 = if !stops.is_empty() {
                    stops[0].color
                } else {
                    [1.0; 4]
                };
                (1.0, [*x0, *y0, *x1, *y1], c0)
            }
            VectorPaint::RadialGradient {
                cx,
                cy,
                rx,
                ry,
                stops,
            } => {
                let c0 = if !stops.is_empty() {
                    stops[0].color
                } else {
                    [1.0; 4]
                };
                (2.0, [*cx, *cy, *rx, *ry], c0)
            }
        };
        append_tessellated_geometry(
            verts,
            indices,
            &mut self.acc_verts,
            &mut self.acc_indices,
            VectorRenderParams {
                color: color0,
                stroke_mult: self.cur_stroke_mult,
                shape_id: self.cur_shape_id,
                params: [
                    grad_type,
                    grad_params[0],
                    grad_params[1],
                    grad_params[2],
                    grad_params[3],
                    self.cur_gradient_row_v,
                ],
                zbias: self.cur_zbias,
            },
        );
        self.cur_zbias += VECTOR_ZBIAS_STEP;
    }

    /// Flush accumulated geometry as a single draw call
    pub fn end(&mut self, cx: &mut Cx2d) {
        if self.acc_verts.is_empty() || self.acc_indices.is_empty() {
            return;
        }

        // Build and upload gradient texture if we have gradient rows
        if self.gradient_row_count > 0 {
            const TEX_WIDTH: usize = 2048;
            let height = self.gradient_row_count;

            let tex = self.gradient_texture.get_or_insert_with(|| {
                Texture::new_with_format(
                    cx.cx.cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: TEX_WIDTH,
                        height,
                        data: None,
                        updated: TextureUpdated::Empty,
                    },
                )
            });

            // Update texture format with current dimensions and data
            let format = tex.get_format(cx.cx.cx);
            *format = TextureFormat::VecBGRAu8_32 {
                width: TEX_WIDTH,
                height,
                data: Some(std::mem::take(&mut self.gradient_texture_data)),
                updated: TextureUpdated::Full,
            };

            self.draw_vars.texture_slots[0] = Some(tex.clone());

            // Convert row indices in param5 to normalized V coordinates.
            // param5 is at float offset 16 within each FLOATS_PER_VERTEX block.
            // Non-gradient vertices have param5 = -1.0 (sentinel), gradient vertices
            // have param5 >= 0.0 (row index).
            let param5_offset = 16; // x,y,u,v, r,g,b,a, sm,sd,sid, p0,p1,p2,p3,p4,p5 -> p5 is at 16
            let num_verts = self.acc_verts.len() / VECTOR_FLOATS_PER_VERTEX;
            for vi in 0..num_verts {
                let idx = vi * VECTOR_FLOATS_PER_VERTEX + param5_offset;
                let row_idx = self.acc_verts[idx];
                if row_idx >= 0.0 {
                    // Map row index to center of texel in V
                    self.acc_verts[idx] = (row_idx + 0.5) / height as f32;
                } else {
                    // No gradient - set to 0.0 so shader uses 2-stop fallback
                    self.acc_verts[idx] = 0.0;
                }
            }
        }

        let geom = self.geometry.get_or_insert_with(|| Geometry::new(cx.cx.cx));
        geom.update(cx.cx.cx, self.acc_indices.clone(), self.acc_verts.clone());
        self.draw_vars.geometry_id = Some(geom.geometry_id());
        cx.new_draw_call(&self.draw_vars);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }

    /// Convenience: walk_turtle, begin, call draw_fn, end
    pub fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        draw_fn: impl FnOnce(&mut Self, f32, f32),
    ) {
        let rect = cx.walk_turtle(walk);
        self.begin();
        draw_fn(self, rect.pos.x as f32, rect.pos.y as f32);
        self.end(cx);
    }
}

/// Sample multi-stop gradient at parameter t (0..1). Returns (r, g, b, a).
/// Stops must be sorted by offset. Colors in stops are premultiplied RGBA.
fn sample_gradient_stops(stops: &[GradientStop], t: f32) -> (f32, f32, f32, f32) {
    if stops.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    if stops.len() == 1 || t <= stops[0].offset {
        let c = &stops[0].color;
        return (c[0], c[1], c[2], c[3]);
    }
    let last = stops.len() - 1;
    if t >= stops[last].offset {
        let c = &stops[last].color;
        return (c[0], c[1], c[2], c[3]);
    }
    // Find the segment
    for i in 1..stops.len() {
        if t <= stops[i].offset {
            let range = stops[i].offset - stops[i - 1].offset;
            let seg_t = if range > 1e-6 {
                (t - stops[i - 1].offset) / range
            } else {
                0.0
            };
            let a = &stops[i - 1].color;
            let b = &stops[i].color;
            return (
                a[0] + (b[0] - a[0]) * seg_t,
                a[1] + (b[1] - a[1]) * seg_t,
                a[2] + (b[2] - a[2]) * seg_t,
                a[3] + (b[3] - a[3]) * seg_t,
            );
        }
    }
    let c = &stops[last].color;
    (c[0], c[1], c[2], c[3])
}
