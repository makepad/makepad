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

        // Render SVG at logical size with no offset.
        // The viewbox transform maps SVG content into (0..lw, 0..lh).
        self.draw_super.begin();
        svg::render_svg(&mut self.draw_super, &doc, 0.0, 0.0, lw, lh, time);

        // Now transform all vertices from the logical-size coordinate space
        // to the target rect, mapping the content bounding box to fill the rect.
        let (bmin_x, bmin_y, bmax_x, bmax_y) = self.content_bounds;
        let bw = bmax_x - bmin_x;
        let bh = bmax_y - bmin_y;

        if bw > 0.0 && bh > 0.0 {
            let tw = rect.size.x as f32;
            let th = rect.size.y as f32;

            // Aspect-aware scale: fit content bounds into target rect
            let (sx, sy) = if self.preserve_aspect {
                let s = (tw / bw).min(th / bh);
                (s, s)
            } else {
                (tw / bw, th / bh)
            };

            // Center within target rect if aspect-preserving leaves slack
            let offset_x = rect.pos.x as f32 + (tw - bw * sx) * 0.5 - bmin_x * sx;
            let offset_y = rect.pos.y as f32 + (th - bh * sy) * 0.5 - bmin_y * sy;

            let stride = 21; // FLOATS_PER_VERTEX
            let verts = &mut self.draw_super.acc_verts;
            let num_verts = verts.len() / stride;
            for i in 0..num_verts {
                verts[i * stride] = verts[i * stride] * sx + offset_x;
                verts[i * stride + 1] = verts[i * stride + 1] * sy + offset_y;
            }
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
        self.svg_doc = Some(doc);
    }

    pub fn load_from_str(&mut self, svg_str: &str) {
        let doc = svg::parse_svg(svg_str);
        self.set_doc_bounds(&doc);
        self.svg_doc = Some(doc);
        self.svg_loaded = true;
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
