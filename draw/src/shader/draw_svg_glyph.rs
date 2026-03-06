use crate::{
    cx_2d::*,
    makepad_platform::*,
    shader::draw_glyph::{DrawGlyph, GlyphShape, GlyphShapeId},
    svg::{self, SvgDefs, SvgDocument, SvgNode, SvgPaint, SvgStyle, SvgUse, Transform2d},
    turtle::*,
};
use makepad_svg::path::{PathCmd, VectorPath};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res

    mod.draw.DrawSvgGlyph = mod.std.set_type_default() do #(DrawSvgGlyph::script_shader(vm)){
        ..mod.draw.DrawGlyph
        // color: vec4(-1,-1,-1,-1) means "use original SVG colors"
        // Any non-negative color overrides layer RGB while preserving per-layer alpha.
        color: vec4(-1.0, -1.0, -1.0, -1.0)
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawSvgGlyph {
    #[live]
    pub svg: Option<ScriptHandleRef>,
    #[rust]
    pub svg_doc: Option<SvgDocument>,
    #[rust]
    pub svg_loaded: bool,
    #[rust]
    pub content_bounds: (f32, f32, f32, f32), // (min_x, min_y, max_x, max_y)
    #[rust]
    pub content_size: DVec2,
    #[rust]
    pub cache_valid: bool,
    #[rust]
    pub cached_shape: Option<GlyphShapeId>,
    #[live(true)]
    pub preserve_aspect: bool,
    #[live(1.0)]
    pub scale: f64,
    #[deref]
    pub draw_super: DrawGlyph,
}

impl DrawSvgGlyph {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        self.load_svg(cx.cx.cx);
        if self.svg_doc.is_none() {
            return Rect::default();
        }
        let walk = self.resolve_walk(walk);
        let rect = cx.walk_turtle(walk);
        self.render_to_rect(cx, &rect);
        rect
    }

    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.load_svg(cx.cx.cx);
        if self.svg_doc.is_none() {
            return;
        }
        self.render_to_rect(cx, &rect);
    }

    pub fn render_to_rect(&mut self, cx: &mut Cx2d, rect: &Rect) {
        self.rebuild_cache();
        let Some(shape_id) = self.cached_shape else {
            return;
        };
        let Some(shape) = self.draw_super.shape(shape_id) else {
            return;
        };

        let override_color = self.draw_super.color;
        let content_rect = self.compute_render_rect(rect);
        let shape_rect = self.map_shape_to_content_rect(&content_rect, shape);
        if override_color.x >= 0.0 {
            let mut layers = shape.layers.clone();
            for layer in &mut layers {
                layer.color = vec4(
                    override_color.x,
                    override_color.y,
                    override_color.z,
                    override_color.w * layer.color.w,
                );
            }
            self.draw_super.draw_layers_abs(cx, shape_rect, &layers);
        } else {
            self.draw_super.draw_shape_abs(cx, shape_id, shape_rect);
        }
        self.draw_super.color = override_color;
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

    fn compute_render_rect(&self, target_rect: &Rect) -> Rect {
        if !self.preserve_aspect {
            return *target_rect;
        }
        let sw = self.content_size.x as f32;
        let sh = self.content_size.y as f32;
        if sw <= 0.0 || sh <= 0.0 {
            return *target_rect;
        }
        let tw = target_rect.size.x as f32;
        let th = target_rect.size.y as f32;
        let scale = (tw / sw).min(th / sh);
        let rw = sw * scale;
        let rh = sh * scale;
        let rx = target_rect.pos.x as f32 + (tw - rw) * 0.5;
        let ry = target_rect.pos.y as f32 + (th - rh) * 0.5;
        rect(rx as f64, ry as f64, rw as f64, rh as f64)
    }

    fn map_shape_to_content_rect(&self, content_rect: &Rect, shape: &GlyphShape) -> Rect {
        let (min_x, min_y, max_x, max_y) = self.content_bounds;
        let content_w = (max_x - min_x) as f64;
        let content_h = (max_y - min_y) as f64;
        if content_w <= 0.0 || content_h <= 0.0 {
            return *content_rect;
        }
        let shape_w = shape.size.x as f64;
        let shape_h = shape.size.y as f64;
        if shape_w <= 0.0 || shape_h <= 0.0 {
            return *content_rect;
        }

        let sx = content_rect.size.x / content_w;
        let sy = content_rect.size.y / content_h;
        let x = content_rect.pos.x + (shape.origin.x as f64 - min_x as f64) * sx;
        let y = content_rect.pos.y + (shape.origin.y as f64 - min_y as f64) * sy;
        let w = shape_w * sx;
        let h = shape_h * sy;
        rect(x, y, w, h)
    }

    fn rebuild_cache(&mut self) {
        if self.cache_valid {
            return;
        }
        let Some(doc) = self.svg_doc.as_ref() else {
            self.cached_shape = None;
            return;
        };

        let (lw, lh) = doc.logical_size();
        let base_xf = if let Some(ref vb) = doc.viewbox {
            let (sx, sy, tx, ty) = svg::viewbox_transform(vb, lw, lh);
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

        self.draw_super.clear_shapes();
        self.draw_super.begin_shape();
        emit_nodes_fill_only(&mut self.draw_super, &doc.root, &doc.defs, &base_xf);
        // DrawGlyph's one-axis band acceleration can miss intersections when
        // sampling the transposed axis. For SVG icons we favor correctness.
        self.cached_shape = self.draw_super.commit_shape(Some(0));
        self.cache_valid = true;
    }

    fn load_svg(&mut self, cx: &mut Cx) {
        if self.svg_loaded {
            return;
        }

        let Some(ref handle_ref) = self.svg else {
            self.svg_loaded = true;
            return;
        };
        let handle = handle_ref.as_handle();
        let data = if let Some(data) = cx.get_resource(handle) {
            data
        } else {
            cx.load_script_resource(handle);
            match cx.get_resource(handle) {
                Some(data) => data,
                None => return,
            }
        };
        self.svg_loaded = true;
        let svg_str = match std::str::from_utf8(&data) {
            Ok(s) => s,
            Err(_) => return,
        };
        let doc = svg::parse_svg(svg_str);
        self.set_doc_bounds(&doc);
        self.svg_doc = Some(doc);
        self.cache_valid = false;
        self.cached_shape = None;
    }

    pub fn load_from_str(&mut self, svg_str: &str) {
        let doc = svg::parse_svg(svg_str);
        self.set_doc_bounds(&doc);
        self.svg_doc = Some(doc);
        self.svg_loaded = true;
        self.cache_valid = false;
        self.cached_shape = None;
    }

    pub fn set_doc_bounds(&mut self, doc: &SvgDocument) {
        let (lw, lh) = doc.logical_size();
        let base_xf = if let Some(ref vb) = doc.viewbox {
            let (sx, sy, tx, ty) = svg::viewbox_transform(vb, lw, lh);
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
        if let Some((min_x, min_y, max_x, max_y)) = doc.compute_bounds_with_transform(&base_xf) {
            self.content_bounds = (min_x, min_y, max_x, max_y);
            self.content_size = dvec2((max_x - min_x) as f64, (max_y - min_y) as f64);
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

fn emit_nodes_fill_only(
    dg: &mut DrawGlyph,
    nodes: &[SvgNode],
    defs: &SvgDefs,
    parent_xf: &Transform2d,
) {
    for node in nodes {
        match node {
            SvgNode::Group(group) => {
                let xf = group.transform.then(parent_xf);
                emit_nodes_fill_only(dg, &group.children, defs, &xf);
            }
            SvgNode::Path(path) => {
                let xf = path.transform.then(parent_xf);
                if let Some(color) = resolve_fill_color(&path.style, defs) {
                    dg.set_color_vec4(color);
                    emit_transformed_path(dg, &path.path, &xf);
                    dg.fill_layer();
                }
            }
            SvgNode::Rect(rect) => {
                let xf = rect.transform.then(parent_xf);
                if let Some(color) = resolve_fill_color(&rect.style, defs) {
                    dg.set_color_vec4(color);
                    let mut path = VectorPath::new();
                    let r = rect.rx.max(rect.ry);
                    if r > 0.0 {
                        path.rounded_rect(rect.x, rect.y, rect.width, rect.height, r);
                    } else {
                        path.rect(rect.x, rect.y, rect.width, rect.height);
                    }
                    emit_transformed_path(dg, &path, &xf);
                    dg.fill_layer();
                }
            }
            SvgNode::Circle(circle) => {
                let xf = circle.transform.then(parent_xf);
                if let Some(color) = resolve_fill_color(&circle.style, defs) {
                    dg.set_color_vec4(color);
                    let mut path = VectorPath::new();
                    path.circle(circle.cx, circle.cy, circle.r);
                    emit_transformed_path(dg, &path, &xf);
                    dg.fill_layer();
                }
            }
            SvgNode::Ellipse(ellipse) => {
                let xf = ellipse.transform.then(parent_xf);
                if let Some(color) = resolve_fill_color(&ellipse.style, defs) {
                    dg.set_color_vec4(color);
                    let mut path = VectorPath::new();
                    path.ellipse(ellipse.cx, ellipse.cy, ellipse.rx, ellipse.ry);
                    emit_transformed_path(dg, &path, &xf);
                    dg.fill_layer();
                }
            }
            SvgNode::Polyline(poly) => {
                let xf = poly.transform.then(parent_xf);
                if let Some(color) = resolve_fill_color(&poly.style, defs) {
                    dg.set_color_vec4(color);
                    emit_points(dg, &poly.points, false, &xf);
                    dg.fill_layer();
                }
            }
            SvgNode::Polygon(poly) => {
                let xf = poly.transform.then(parent_xf);
                if let Some(color) = resolve_fill_color(&poly.style, defs) {
                    dg.set_color_vec4(color);
                    emit_points(dg, &poly.points, true, &xf);
                    dg.fill_layer();
                }
            }
            SvgNode::Line(_) => {}
            SvgNode::Use(use_node) => emit_use_fill_only(dg, use_node, defs, parent_xf),
        }
    }
}

fn emit_use_fill_only(
    dg: &mut DrawGlyph,
    use_node: &SvgUse,
    defs: &SvgDefs,
    parent_xf: &Transform2d,
) {
    let Some(symbol) = defs.symbols.get(&use_node.href) else {
        return;
    };

    let mut local_xf = use_node.transform.clone();
    let offset = Transform2d::translate(use_node.x, use_node.y);
    local_xf = offset.then(&local_xf);

    if let Some(ref vb) = symbol.viewbox {
        let w = use_node.width.unwrap_or(vb.width);
        let h = use_node.height.unwrap_or(vb.height);
        let (sx, sy, tx, ty) = svg::viewbox_transform(vb, w, h);
        let vb_xf = Transform2d {
            a: sx,
            c: 0.0,
            e: tx,
            b: 0.0,
            d: sy,
            f: ty,
        };
        local_xf = vb_xf.then(&local_xf);
    }

    let xf = local_xf.then(parent_xf);
    emit_nodes_fill_only(dg, &symbol.children, defs, &xf);
}

fn emit_points(dg: &mut DrawGlyph, points: &[(f32, f32)], closed: bool, xf: &Transform2d) {
    if points.is_empty() {
        return;
    }
    let (x0, y0) = xf.apply(points[0].0, points[0].1);
    dg.move_to(x0, y0);
    for &(x, y) in &points[1..] {
        let (tx, ty) = xf.apply(x, y);
        dg.line_to(tx, ty);
    }
    if closed {
        dg.close();
    }
}

fn emit_transformed_path(dg: &mut DrawGlyph, path: &VectorPath, xf: &Transform2d) {
    for command in &path.cmds {
        match *command {
            PathCmd::MoveTo(x, y) => {
                let (tx, ty) = xf.apply(x, y);
                dg.move_to(tx, ty);
            }
            PathCmd::LineTo(x, y) => {
                let (tx, ty) = xf.apply(x, y);
                dg.line_to(tx, ty);
            }
            PathCmd::BezierTo(cx1, cy1, cx2, cy2, x, y) => {
                let (tcx1, tcy1) = xf.apply(cx1, cy1);
                let (tcx2, tcy2) = xf.apply(cx2, cy2);
                let (tx, ty) = xf.apply(x, y);
                dg.bezier_to(tcx1, tcy1, tcx2, tcy2, tx, ty);
            }
            PathCmd::Close => dg.close(),
            PathCmd::Winding(_) => {}
        }
    }
}

fn resolve_fill_color(style: &SvgStyle, defs: &SvgDefs) -> Option<Vec4f> {
    let paint = style.fill.as_ref()?;
    let alpha = (style.opacity * style.fill_opacity).clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return None;
    }
    match paint {
        SvgPaint::None => None,
        SvgPaint::Color(r, g, b, a) => Some(vec4(*r, *g, *b, *a * alpha)),
        SvgPaint::CurrentColor => {
            let (r, g, b, a) = style.color;
            Some(vec4(r, g, b, a * alpha))
        }
        SvgPaint::GradientRef(id) => {
            let grad = defs.gradients.get(id)?;
            let stop = grad.stops.first()?;
            let c = stop.color;
            Some(vec4(c[0], c[1], c[2], c[3] * alpha))
        }
    }
}
