use crate::{cx_2d::*, draw_list_2d::ManyInstances, makepad_platform::*, turtle::*, vector::*};

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

        v_tcoord: varying(vec2f)
        v_world: varying(vec2f)
        v_color: varying(vec4f)
        v_color2: varying(vec4f)
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
            self.v_color2 = vec4(self.geom.color2_r, self.geom.color2_g, self.geom.color2_b, self.geom.color2_a);
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
            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.draw_call.zbias
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
        // v_color = first stop color, v_color2 = last stop color
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
                return mix(self.v_color, self.v_color2, t)
            }
            // radial gradient: t = distance(world, center) / radius
            let center = vec2(self.v_param1, self.v_param2);
            let radius = self.v_param3;
            var t = 0.0;
            if radius > 0.000001 {
                t = clamp(length(self.v_world - center) / radius, 0.0, 1.0)
            }
            return mix(self.v_color, self.v_color2, t)
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
            if self.v_world.x < self.draw_clip.x
                || self.v_world.y < self.draw_clip.y
                || self.v_world.x > self.draw_clip.z
                || self.v_world.y > self.draw_clip.w {
                discard()
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

const FLOATS_PER_VERTEX: usize = 21; // x,y,u,v, r,g,b,a, stroke_mult, stroke_dist, shape_id, param0-5, color2_r,g,b,a

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawVector {
    #[rust]
    pub many_instances: Option<ManyInstances>,
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
    #[rust]
    pub geometry: Option<Geometry>,
    #[rust]
    pub path: VectorPath,
    #[rust]
    pub tess: Tessellator,
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
        self.tess.flatten(&self.path, 0.25);
        let (verts, indices) = self.tess.stroke(stroke_width, cap, join, miter_limit, aa);
        let sm = if aa > 0.0 {
            (stroke_width * 0.5 + aa * 0.5) / aa
        } else {
            1e6
        };
        self.cur_stroke_mult = sm;
        self.append_geometry(&verts, &indices);
        self.path.clear();
    }

    pub fn fill(&mut self) {
        self.fill_opts(LineJoin::Miter, 4.0, 1.0);
    }

    pub fn fill_opts(&mut self, join: LineJoin, miter_limit: f32, aa: f32) {
        self.tess.flatten(&self.path, 0.25);
        let (verts, indices) = self.tess.fill(aa, join, miter_limit);
        self.cur_stroke_mult = 1e6;
        self.append_geometry(&verts, &indices);
        self.path.clear();
    }

    /// Draw a geometry-based shadow for any filled path (stars, polygons, etc).
    /// Uses tessellation with a wide fringe + erf blur in the pixel shader.
    /// Build your path first, then call shape_shadow(blur).
    pub fn shape_shadow(&mut self, blur: f32) {
        self.tess.flatten(&self.path, 0.25);
        // Bevel joins prevent miter spikes at sharp corners
        let (verts, indices) = self.tess.fill_shadow(blur, LineJoin::Bevel, 1.0);
        self.cur_stroke_mult = -2.0; // sentinel for geometry shadow mode
        self.append_geometry(&verts, &indices);
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
        let base = (self.acc_verts.len() / FLOATS_PER_VERTEX) as u32;
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
            // color2 (unused for shadow)
            self.acc_verts.push(0.0);
            self.acc_verts.push(0.0);
            self.acc_verts.push(0.0);
            self.acc_verts.push(0.0);
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
        let base = (self.acc_verts.len() / FLOATS_PER_VERTEX) as u32;
        // compute gradient params and endpoint colors from current paint
        let (grad_type, grad_params, color0, color1) = match &self.cur_paint {
            VectorPaint::Solid { color } => (0.0, [0.0; 4], *color, *color),
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
                let c1 = if !stops.is_empty() {
                    stops[stops.len() - 1].color
                } else {
                    [1.0; 4]
                };
                (1.0, [*x0, *y0, *x1, *y1], c0, c1)
            }
            VectorPaint::RadialGradient { cx, cy, r, stops } => {
                let c0 = if !stops.is_empty() {
                    stops[0].color
                } else {
                    [1.0; 4]
                };
                let c1 = if !stops.is_empty() {
                    stops[stops.len() - 1].color
                } else {
                    [1.0; 4]
                };
                (2.0, [*cx, *cy, *r, 0.0], c0, c1)
            }
        };
        for v in verts {
            self.acc_verts.push(v.x);
            self.acc_verts.push(v.y);
            self.acc_verts.push(v.u);
            self.acc_verts.push(v.v);
            // color (first stop for gradients, solid color otherwise)
            self.acc_verts.push(color0[0]);
            self.acc_verts.push(color0[1]);
            self.acc_verts.push(color0[2]);
            self.acc_verts.push(color0[3]);
            self.acc_verts.push(self.cur_stroke_mult);
            self.acc_verts.push(v.stroke_dist);
            self.acc_verts.push(self.cur_shape_id);
            // params: grad_type, then 4 gradient geometry params, then 0
            self.acc_verts.push(grad_type);
            self.acc_verts.push(grad_params[0]);
            self.acc_verts.push(grad_params[1]);
            self.acc_verts.push(grad_params[2]);
            self.acc_verts.push(grad_params[3]);
            self.acc_verts.push(0.0);
            // color2 (last stop for gradients)
            self.acc_verts.push(color1[0]);
            self.acc_verts.push(color1[1]);
            self.acc_verts.push(color1[2]);
            self.acc_verts.push(color1[3]);
        }
        for &i in indices {
            self.acc_indices.push(base + i);
        }
    }

    /// Flush accumulated geometry as a single draw call
    pub fn end(&mut self, cx: &mut Cx2d) {
        if self.acc_verts.is_empty() || self.acc_indices.is_empty() {
            return;
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
