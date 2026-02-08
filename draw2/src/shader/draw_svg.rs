use crate::{
    cx_2d::*,
    makepad_platform::*,
    shader::draw_vector::DrawVector,
    svg::{self, SvgDocument},
    turtle::*,
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res

    mod.draw.DrawSvg = mod.std.set_type_default() do #(DrawSvg::script_shader(vm)){
        ..mod.draw.DrawVector

        // Tint color: vec4(-1,-1,-1,-1) means "use original SVG colors"
        // Any non-negative color replaces the SVG color, preserving per-vertex alpha.
        tint: vec4(-1.0, -1.0, -1.0, -1.0)

        get_color: fn() {
            let base = self.eval_gradient()
            if self.tint.x >= 0.0 {
                // Tint mode: replace color with tint, keep alpha from SVG
                return vec4(self.tint.rgb * self.tint.a * base.a, self.tint.a * base.a)
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
    #[rust]
    svg_size: DVec2,
    #[live(true)]
    pub preserve_aspect: bool,
    #[deref]
    pub draw_super: DrawVector,
    #[live(vec4(-1.0, -1.0, -1.0, -1.0))]
    pub tint: Vec4f,
}

impl DrawSvg {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        self.load_svg(cx);
        let Some(ref doc) = self.svg_doc else {
            return Rect::default();
        };

        let walk = self.resolve_walk(walk);
        let rect = cx.walk_turtle(walk);
        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;
        let w = rect.size.x as f32;
        let h = rect.size.y as f32;

        self.draw_super.begin();
        svg::render_svg(&mut self.draw_super, doc, ox, oy, w, h, 0.0);
        self.draw_super.end(cx);

        rect
    }

    pub fn draw_walk_time(&mut self, cx: &mut Cx2d, walk: Walk, time: f32) -> Rect {
        self.load_svg(cx);
        let Some(ref doc) = self.svg_doc else {
            return Rect::default();
        };

        let walk = self.resolve_walk(walk);
        let rect = cx.walk_turtle(walk);
        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;
        let w = rect.size.x as f32;
        let h = rect.size.y as f32;

        self.draw_super.begin();
        svg::render_svg(&mut self.draw_super, doc, ox, oy, w, h, time);
        self.draw_super.end(cx);

        rect
    }

    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.load_svg(cx);
        let Some(ref doc) = self.svg_doc else { return };

        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;
        let w = rect.size.x as f32;
        let h = rect.size.y as f32;

        self.draw_super.begin();
        svg::render_svg(&mut self.draw_super, doc, ox, oy, w, h, 0.0);
        self.draw_super.end(cx);
    }

    fn resolve_walk(&self, walk: Walk) -> Walk {
        if self.svg_size.x <= 0.0 || self.svg_size.y <= 0.0 {
            return walk;
        }

        if self.preserve_aspect {
            let svg_aspect = self.svg_size.x / self.svg_size.y;
            match (walk.width, walk.height) {
                (Size::Fit { .. }, Size::Fit { .. }) => Walk {
                    width: Size::Fixed(self.svg_size.x),
                    height: Size::Fixed(self.svg_size.y),
                    ..walk
                },
                (Size::Fixed(w), Size::Fit { .. }) => Walk {
                    width: Size::Fixed(w),
                    height: Size::Fixed(w / svg_aspect),
                    ..walk
                },
                (Size::Fit { .. }, Size::Fixed(h)) => Walk {
                    width: Size::Fixed(h * svg_aspect),
                    height: Size::Fixed(h),
                    ..walk
                },
                _ => walk,
            }
        } else {
            Walk {
                width: match walk.width {
                    Size::Fit { .. } => Size::Fixed(self.svg_size.x),
                    other => other,
                },
                height: match walk.height {
                    Size::Fit { .. } => Size::Fixed(self.svg_size.y),
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
        let (w, h) = doc.logical_size();
        self.svg_size = dvec2(w as f64, h as f64);
        self.svg_doc = Some(doc);
    }

    pub fn load_from_str(&mut self, svg_str: &str) {
        let doc = svg::parse_svg(svg_str);
        let (w, h) = doc.logical_size();
        self.svg_size = dvec2(w as f64, h as f64);
        self.svg_doc = Some(doc);
        self.svg_loaded = true;
    }

    pub fn svg_size(&self) -> Option<DVec2> {
        if self.svg_doc.is_some() {
            Some(self.svg_size)
        } else {
            None
        }
    }
}
