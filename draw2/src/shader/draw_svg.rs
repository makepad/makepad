use crate::{
    cx_2d::*,
    makepad_platform::*,
    shader::draw_vector::DrawVector,
    svg::{self, SvgDocument},
    turtle::*,
};
use makepad_svg::document::Transform2d;
use makepad_svg::units::viewbox_transform;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res

    mod.draw.DrawSvg = mod.std.set_type_default() do #(DrawSvg::script_shader(vm)){
        ..mod.draw.DrawVector

        // color: vec4(-1,-1,-1,-1) means "use original SVG colors"
        // Any non-negative color replaces the SVG color, preserving per-vertex alpha.
        color: vec4(-1.0, -1.0, -1.0, -1.0)

        // GPU-side transform for cached SVG geometry
        svg_scale: uniform(vec2(1.0, 1.0))
        svg_offset: uniform(vec2(0.0, 0.0))

        // Animation time in seconds, available for custom shader effects
        svg_time: uniform(float(0.0))

        vertex: fn() {
            let pos = vec2(self.geom.x, self.geom.y);
            // Apply cached SVG transform on GPU
            let transformed = pos * self.svg_scale + self.svg_offset;
            self.v_tcoord = vec2(self.geom.u, self.geom.v);
            self.v_color = vec4(self.geom.color_r, self.geom.color_g, self.geom.color_b, self.geom.color_a);
            self.v_color2 = vec4(self.geom.color2_r, self.geom.color2_g, self.geom.color2_b, self.geom.color2_a);
            self.v_stroke_mult = self.geom.stroke_mult;
            self.v_stroke_dist = self.geom.stroke_dist;
            self.v_shape_id = self.geom.shape_id;
            self.v_param0 = self.geom.param0;
            self.v_param5 = self.geom.param5;
            // Transform gradient geometry params by svg_scale/svg_offset
            let grad_type = self.geom.param0;
            if grad_type > 0.5 && grad_type < 1.5 {
                // Linear gradient: p1,p2 = start point, p3,p4 = end point
                let p0 = vec2(self.geom.param1, self.geom.param2) * self.svg_scale + self.svg_offset;
                let p1 = vec2(self.geom.param3, self.geom.param4) * self.svg_scale + self.svg_offset;
                self.v_param1 = p0.x;
                self.v_param2 = p0.y;
                self.v_param3 = p1.x;
                self.v_param4 = p1.y;
            } else if grad_type > 1.5 {
                // Radial gradient: p1,p2 = center, p3,p4 = rx, ry
                let center = vec2(self.geom.param1, self.geom.param2) * self.svg_scale + self.svg_offset;
                self.v_param1 = center.x;
                self.v_param2 = center.y;
                self.v_param3 = self.geom.param3 * self.svg_scale.x;
                self.v_param4 = self.geom.param4 * self.svg_scale.y;
            } else if self.geom.shape_id > 0.5 {
                // Effect shape with bbox in params: transform bbox by svg_scale/svg_offset
                let bbox_min = vec2(self.geom.param1, self.geom.param2) * self.svg_scale + self.svg_offset;
                let bbox_max = vec2(self.geom.param3, self.geom.param4) * self.svg_scale + self.svg_offset;
                self.v_param1 = bbox_min.x;
                self.v_param2 = bbox_min.y;
                self.v_param3 = bbox_max.x;
                self.v_param4 = bbox_max.y;
            } else {
                self.v_param1 = self.geom.param1;
                self.v_param2 = self.geom.param2;
                self.v_param3 = self.geom.param3;
                self.v_param4 = self.geom.param4;
            }
            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;
            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.draw_call.zbias
                1.
            );
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }

        get_color: fn() {
            let base = self.eval_gradient()
            if self.color.x >= 0.0 {
                return vec4(self.color.rgb * self.color.a * base.a, self.color.a * base.a)
            }
            return base
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSvg {
    #[live]
    pub svg: Option<ScriptHandleRef>,
    #[rust]
    svg_doc: Option<SvgDocument>,
    #[rust]
    svg_loaded: bool,
    // Content bounding box after viewbox transform at 1:1 scale.
    // This is the actual extent of rendered geometry.
    #[rust]
    content_bounds: (f32, f32, f32, f32), // (min_x, min_y, max_x, max_y)
    #[rust]
    content_size: DVec2,
    #[rust]
    cached_verts: Vec<f32>,
    #[rust]
    cached_indices: Vec<u32>,
    #[rust]
    cache_valid: bool,
    #[rust]
    has_animations: bool,
    #[live(true)]
    pub preserve_aspect: bool,
    #[live(1.0)]
    pub scale: f64,
    #[deref]
    pub draw_super: DrawVector,
    #[live(vec4(-1.0, -1.0, -1.0, -1.0))]
    pub color: Vec4f,
}

impl DrawSvg {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        self.load_svg(cx);
        if self.svg_doc.is_none() {
            return Rect::default();
        }
        let walk = self.resolve_walk(walk);
        let rect = cx.walk_turtle(walk);
        self.render_to_rect(cx, &rect, 0.0);
        rect
    }

    pub fn draw_walk_time(&mut self, cx: &mut Cx2d, walk: Walk, time: f32) -> Rect {
        self.load_svg(cx);
        if self.svg_doc.is_none() {
            return Rect::default();
        }
        let walk = self.resolve_walk(walk);
        let rect = cx.walk_turtle(walk);
        self.render_to_rect(cx, &rect, time);
        rect
    }

    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.load_svg(cx);
        if self.svg_doc.is_none() {
            return;
        };
        self.render_to_rect(cx, &rect, 0.0);
    }

    fn render_to_rect(&mut self, cx: &mut Cx2d, rect: &Rect, time: f32) {
        let doc = self.svg_doc.take().unwrap();

        let (lw, lh) = doc.logical_size();

        if self.has_animations {
            // Animated SVGs must re-tessellate every frame
            self.draw_super.begin();
            svg::render_svg(&mut self.draw_super, &doc, 0.0, 0.0, lw, lh, time);
        } else if !self.cache_valid {
            // Tessellate and cache on first render (or after invalidation)
            self.draw_super.begin();
            svg::render_svg(&mut self.draw_super, &doc, 0.0, 0.0, lw, lh, time);
            self.cached_verts = self.draw_super.acc_verts.clone();
            self.cached_indices = self.draw_super.acc_indices.clone();
            self.cache_valid = true;
        } else {
            // Replay cached geometry
            self.draw_super.begin();
            self.draw_super
                .acc_verts
                .extend_from_slice(&self.cached_verts);
            self.draw_super
                .acc_indices
                .extend_from_slice(&self.cached_indices);
        }

        // Compute GPU-side scale + offset from content bounds to target rect
        let (bmin_x, bmin_y, bmax_x, bmax_y) = self.content_bounds;
        let bw = bmax_x - bmin_x;
        let bh = bmax_y - bmin_y;

        if bw > 0.0 && bh > 0.0 {
            let tw = rect.size.x as f32;
            let th = rect.size.y as f32;

            let (sx, sy) = if self.preserve_aspect {
                let s = (tw / bw).min(th / bh);
                (s, s)
            } else {
                (tw / bw, th / bh)
            };

            let offset_x = rect.pos.x as f32 + (tw - bw * sx) * 0.5 - bmin_x * sx;
            let offset_y = rect.pos.y as f32 + (th - bh * sy) * 0.5 - bmin_y * sy;

            // svg_scale at uniform offset 0..1, svg_offset at 2..3, svg_time at 4
            let uniforms = &mut self.draw_super.draw_vars.dyn_uniforms;
            uniforms[0] = sx;
            uniforms[1] = sy;
            uniforms[2] = offset_x;
            uniforms[3] = offset_y;
            uniforms[4] = time;
        } else {
            let uniforms = &mut self.draw_super.draw_vars.dyn_uniforms;
            uniforms[0] = 1.0;
            uniforms[1] = 1.0;
            uniforms[2] = rect.pos.x as f32;
            uniforms[3] = rect.pos.y as f32;
            uniforms[4] = time;
        }

        self.draw_super.end(cx);
        self.svg_doc = Some(doc);
    }

    fn resolve_walk(&self, walk: Walk) -> Walk {
        let sw = self.content_size.x * self.scale;
        let sh = self.content_size.y * self.scale;
        if sw <= 0.0 || sh <= 0.0 {
            return walk;
        }

        if self.preserve_aspect {
            let aspect = sw / sh;
            match (walk.width, walk.height) {
                (Size::Fit { .. }, Size::Fit { .. }) => Walk {
                    width: Size::Fixed(sw),
                    height: Size::Fixed(sh),
                    ..walk
                },
                (Size::Fixed(w), Size::Fit { .. }) => Walk {
                    width: Size::Fixed(w),
                    height: Size::Fixed(w / aspect),
                    ..walk
                },
                (Size::Fit { .. }, Size::Fixed(h)) => Walk {
                    width: Size::Fixed(h * aspect),
                    height: Size::Fixed(h),
                    ..walk
                },
                _ => walk,
            }
        } else {
            Walk {
                width: match walk.width {
                    Size::Fit { .. } => Size::Fixed(sw),
                    other => other,
                },
                height: match walk.height {
                    Size::Fit { .. } => Size::Fixed(sh),
                    other => other,
                },
                ..walk
            }
        }
    }

    fn load_svg(&mut self, cx: &mut Cx) {
        if self.svg_loaded {
            return;
        }
        self.svg_loaded = true;

        let Some(ref handle_ref) = self.svg else {
            return;
        };

        let handle = handle_ref.as_handle();

        let data = if let Some(data) = cx.get_resource(handle) {
            data
        } else {
            cx.script_data.resources.load_all_resources();
            match cx.get_resource(handle) {
                Some(data) => data,
                None => return,
            }
        };

        let svg_str = match std::str::from_utf8(&data) {
            Ok(s) => s,
            Err(_) => return,
        };

        let doc = svg::parse_svg(svg_str);
        self.set_doc_bounds(&doc);
        self.has_animations = doc.has_animations();
        self.svg_doc = Some(doc);
        self.cache_valid = false;
    }

    pub fn load_from_str(&mut self, svg_str: &str) {
        let doc = svg::parse_svg(svg_str);
        self.set_doc_bounds(&doc);
        self.has_animations = doc.has_animations();
        self.svg_doc = Some(doc);
        self.svg_loaded = true;
        self.cache_valid = false;
    }

    fn set_doc_bounds(&mut self, doc: &SvgDocument) {
        // Compute the viewbox transform at 1:1 logical size
        let (lw, lh) = doc.logical_size();
        let base_xf = if let Some(ref vb) = doc.viewbox {
            let (sx, sy, tx, ty) = viewbox_transform(vb, lw, lh);
            Transform2d {
                a: sx,
                c: 0.0,
                e: tx,
                b: 0.0,
                d: sy,
                f: ty,
            }
        } else {
            Transform2d::identity()
        };

        // Compute content bounds with viewbox transform applied
        if let Some((min_x, min_y, max_x, max_y)) = doc.compute_bounds_with_transform(&base_xf) {
            self.content_bounds = (min_x, min_y, max_x, max_y);
            let w = max_x - min_x;
            let h = max_y - min_y;
            self.content_size = dvec2(w as f64, h as f64);
        } else {
            self.content_bounds = (0.0, 0.0, lw, lh);
            self.content_size = dvec2(lw as f64, lh as f64);
        }
    }

    pub fn svg_size(&self) -> Option<DVec2> {
        if self.svg_doc.is_some() {
            Some(self.content_size)
        } else {
            None
        }
    }
}
