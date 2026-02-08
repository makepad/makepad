use crate::paint::GradientStop;
use crate::path::{FillRule, LineCap, LineJoin, VectorPath};
use std::collections::HashMap;

// ---- Transform ----

#[derive(Clone, Debug)]
pub struct Transform2d {
    pub a: f32,
    pub c: f32,
    pub e: f32,
    pub b: f32,
    pub d: f32,
    pub f: f32,
}

impl Default for Transform2d {
    fn default() -> Self {
        Self {
            a: 1.0,
            c: 0.0,
            e: 0.0,
            b: 0.0,
            d: 1.0,
            f: 0.0,
        }
    }
}

impl Transform2d {
    pub fn identity() -> Self {
        Self::default()
    }

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self {
            a: 1.0,
            c: 0.0,
            e: tx,
            b: 0.0,
            d: 1.0,
            f: ty,
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            c: 0.0,
            e: 0.0,
            b: 0.0,
            d: sy,
            f: 0.0,
        }
    }

    pub fn rotate(angle_rad: f32) -> Self {
        let (s, c) = angle_rad.sin_cos();
        Self {
            a: c,
            c: -s,
            e: 0.0,
            b: s,
            d: c,
            f: 0.0,
        }
    }

    pub fn skew_x(angle_rad: f32) -> Self {
        Self {
            a: 1.0,
            c: angle_rad.tan(),
            e: 0.0,
            b: 0.0,
            d: 1.0,
            f: 0.0,
        }
    }

    pub fn skew_y(angle_rad: f32) -> Self {
        Self {
            a: 1.0,
            c: 0.0,
            e: 0.0,
            b: angle_rad.tan(),
            d: 1.0,
            f: 0.0,
        }
    }

    pub fn then(&self, other: &Transform2d) -> Transform2d {
        Transform2d {
            a: other.a * self.a + other.c * self.b,
            c: other.a * self.c + other.c * self.d,
            e: other.a * self.e + other.c * self.f + other.e,
            b: other.b * self.a + other.d * self.b,
            d: other.b * self.c + other.d * self.d,
            f: other.b * self.e + other.d * self.f + other.f,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    pub fn scale_factor(&self) -> f32 {
        ((self.a * self.a + self.b * self.b).sqrt() + (self.c * self.c + self.d * self.d).sqrt())
            * 0.5
    }
}

// ---- Paint ----

#[derive(Clone, Debug)]
pub enum SvgPaint {
    None,
    Color(f32, f32, f32, f32),
    GradientRef(String),
}

impl Default for SvgPaint {
    fn default() -> Self {
        SvgPaint::None
    }
}

// ---- Style ----

#[derive(Clone, Debug)]
pub struct SvgStyle {
    pub fill: Option<SvgPaint>,
    pub fill_opacity: f32,
    pub fill_rule: FillRule,
    pub stroke: Option<SvgPaint>,
    pub stroke_opacity: f32,
    pub stroke_width: f32,
    pub stroke_linecap: LineCap,
    pub stroke_linejoin: LineJoin,
    pub stroke_miterlimit: f32,
    pub stroke_dasharray: Option<Vec<f32>>,
    pub stroke_dashoffset: f32,
    pub opacity: f32,
}

impl Default for SvgStyle {
    fn default() -> Self {
        Self {
            fill: Some(SvgPaint::Color(1.0, 1.0, 1.0, 1.0)), // default fill white for dark UI
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: None,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            stroke_linecap: LineCap::Butt,
            stroke_linejoin: LineJoin::Miter,
            stroke_miterlimit: 4.0,
            stroke_dasharray: None,
            stroke_dashoffset: 0.0,
            opacity: 1.0,
        }
    }
}

// ---- Gradient ----

#[derive(Clone, Debug)]
pub enum GradientKind {
    Linear,
    Radial,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientUnits {
    UserSpaceOnUse,
    ObjectBoundingBox,
}

impl Default for GradientUnits {
    fn default() -> Self {
        GradientUnits::ObjectBoundingBox
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpreadMethod {
    Pad,
    Reflect,
    Repeat,
}

impl Default for SpreadMethod {
    fn default() -> Self {
        SpreadMethod::Pad
    }
}

#[derive(Clone, Debug)]
pub struct SvgGradient {
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
    pub transform: Transform2d,
    pub units: GradientUnits,
    pub spread: SpreadMethod,
    // Linear-specific
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    // Radial-specific
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub fx: f32,
    pub fy: f32,
    // Inheritance
    pub href: Option<String>,
}

impl SvgGradient {
    pub fn new_linear() -> Self {
        Self {
            kind: GradientKind::Linear,
            stops: Vec::new(),
            transform: Transform2d::identity(),
            units: GradientUnits::default(),
            spread: SpreadMethod::default(),
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 0.0,
            cx: 0.5,
            cy: 0.5,
            r: 0.5,
            fx: 0.5,
            fy: 0.5,
            href: None,
        }
    }

    pub fn new_radial() -> Self {
        Self {
            kind: GradientKind::Radial,
            stops: Vec::new(),
            transform: Transform2d::identity(),
            units: GradientUnits::default(),
            spread: SpreadMethod::default(),
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 0.0,
            cx: 0.5,
            cy: 0.5,
            r: 0.5,
            fx: 0.5,
            fy: 0.5,
            href: None,
        }
    }

    pub fn resolve_href(&mut self, gradients: &HashMap<String, SvgGradient>) {
        if let Some(href) = self.href.take() {
            if let Some(parent) = gradients.get(&href) {
                if self.stops.is_empty() {
                    self.stops = parent.stops.clone();
                }
            }
        }
    }
}

// ---- Animation ----

#[derive(Clone, Debug)]
pub enum AnimateAttribute {
    Fill,
    Stroke,
    StrokeWidth,
    Opacity,
    FillOpacity,
    StrokeOpacity,
    Transform,
    D, // path data morphing
    Custom(String),
}

#[derive(Clone, Debug)]
pub enum AnimateCalcMode {
    Linear,
    Discrete,
    Paced,
    Spline,
}

impl Default for AnimateCalcMode {
    fn default() -> Self {
        AnimateCalcMode::Linear
    }
}

#[derive(Clone, Debug)]
pub enum AnimateFill {
    Remove,
    Freeze,
}

impl Default for AnimateFill {
    fn default() -> Self {
        AnimateFill::Remove
    }
}

#[derive(Clone, Debug)]
pub struct SvgAnimate {
    pub attribute: AnimateAttribute,
    pub from: Option<String>,
    pub to: Option<String>,
    pub values: Option<Vec<String>>,
    pub key_times: Option<Vec<f32>>,
    pub key_splines: Option<Vec<[f32; 4]>>,
    pub dur: f32,   // duration in seconds
    pub begin: f32, // begin offset in seconds
    pub repeat_count: RepeatCount,
    pub calc_mode: AnimateCalcMode,
    pub fill: AnimateFill,
}

#[derive(Clone, Debug)]
pub enum RepeatCount {
    Count(f32),
    Indefinite,
}

impl Default for RepeatCount {
    fn default() -> Self {
        RepeatCount::Count(1.0)
    }
}

impl Default for SvgAnimate {
    fn default() -> Self {
        Self {
            attribute: AnimateAttribute::Custom(String::new()),
            from: None,
            to: None,
            values: None,
            key_times: None,
            key_splines: None,
            dur: 0.0,
            begin: 0.0,
            repeat_count: RepeatCount::default(),
            calc_mode: AnimateCalcMode::default(),
            fill: AnimateFill::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SvgAnimateTransform {
    pub kind: AnimateTransformType,
    pub from: Option<String>,
    pub to: Option<String>,
    pub values: Option<Vec<String>>,
    pub key_times: Option<Vec<f32>>,
    pub dur: f32,
    pub begin: f32,
    pub repeat_count: RepeatCount,
    pub calc_mode: AnimateCalcMode,
    pub fill: AnimateFill,
}

#[derive(Clone, Debug)]
pub enum AnimateTransformType {
    Translate,
    Scale,
    Rotate,
    SkewX,
    SkewY,
}

impl Default for SvgAnimateTransform {
    fn default() -> Self {
        Self {
            kind: AnimateTransformType::Translate,
            from: None,
            to: None,
            values: None,
            key_times: None,
            dur: 0.0,
            begin: 0.0,
            repeat_count: RepeatCount::default(),
            calc_mode: AnimateCalcMode::default(),
            fill: AnimateFill::default(),
        }
    }
}

// ---- Nodes ----

#[derive(Clone, Debug, Default)]
pub struct SvgGroup {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub children: Vec<SvgNode>,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgPath {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub path: VectorPath,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgRect {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rx: f32,
    pub ry: f32,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgCircle {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgEllipse {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub cx: f32,
    pub cy: f32,
    pub rx: f32,
    pub ry: f32,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgLine {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgPolyline {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub points: Vec<(f32, f32)>,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub struct SvgPolygon {
    pub id: Option<String>,
    pub style: SvgStyle,
    pub transform: Transform2d,
    pub points: Vec<(f32, f32)>,
    pub animations: Vec<SvgAnimate>,
    pub animate_transforms: Vec<SvgAnimateTransform>,
}

#[derive(Clone, Debug)]
pub enum SvgNode {
    Group(SvgGroup),
    Path(SvgPath),
    Rect(SvgRect),
    Circle(SvgCircle),
    Ellipse(SvgEllipse),
    Line(SvgLine),
    Polyline(SvgPolyline),
    Polygon(SvgPolygon),
}

// ---- Defs ----

#[derive(Clone, Debug, Default)]
pub struct SvgDefs {
    pub gradients: HashMap<String, SvgGradient>,
}

// ---- ViewBox ----

#[derive(Clone, Debug)]
pub struct ViewBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

// ---- Document ----

#[derive(Clone, Debug)]
pub struct SvgDocument {
    pub viewbox: Option<ViewBox>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub defs: SvgDefs,
    pub root: Vec<SvgNode>,
}

impl Default for SvgDocument {
    fn default() -> Self {
        Self {
            viewbox: None,
            width: None,
            height: None,
            defs: SvgDefs::default(),
            root: Vec::new(),
        }
    }
}

impl SvgDocument {
    pub fn logical_size(&self) -> (f32, f32) {
        if let (Some(w), Some(h)) = (self.width, self.height) {
            return (w, h);
        }
        if let Some(ref vb) = self.viewbox {
            return (vb.width, vb.height);
        }
        (300.0, 150.0) // SVG default
    }

    /// Compute the actual bounding box of all geometry in the document
    /// by walking all nodes and applying their transforms to path control points.
    /// Returns (min_x, min_y, max_x, max_y), or None if no geometry.
    pub fn compute_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut bounds = BoundsAccum::new();
        Self::bounds_nodes(&self.root, &Transform2d::identity(), &mut bounds);
        bounds.result()
    }

    fn bounds_nodes(nodes: &[SvgNode], parent_xf: &Transform2d, bounds: &mut BoundsAccum) {
        for node in nodes {
            match node {
                SvgNode::Group(g) => {
                    let xf = parent_xf.then(&g.transform);
                    Self::bounds_nodes(&g.children, &xf, bounds);
                }
                SvgNode::Path(p) => {
                    let xf = parent_xf.then(&p.transform);
                    Self::bounds_path(&p.path, &xf, bounds);
                }
                SvgNode::Rect(r) => {
                    let xf = parent_xf.then(&r.transform);
                    bounds.add_point_xf(r.x, r.y, &xf);
                    bounds.add_point_xf(r.x + r.width, r.y + r.height, &xf);
                }
                SvgNode::Circle(c) => {
                    let xf = parent_xf.then(&c.transform);
                    bounds.add_point_xf(c.cx - c.r, c.cy - c.r, &xf);
                    bounds.add_point_xf(c.cx + c.r, c.cy + c.r, &xf);
                }
                SvgNode::Ellipse(e) => {
                    let xf = parent_xf.then(&e.transform);
                    bounds.add_point_xf(e.cx - e.rx, e.cy - e.ry, &xf);
                    bounds.add_point_xf(e.cx + e.rx, e.cy + e.ry, &xf);
                }
                SvgNode::Line(l) => {
                    let xf = parent_xf.then(&l.transform);
                    bounds.add_point_xf(l.x1, l.y1, &xf);
                    bounds.add_point_xf(l.x2, l.y2, &xf);
                }
                SvgNode::Polyline(p) => {
                    let xf = parent_xf.then(&p.transform);
                    for &(px, py) in &p.points {
                        bounds.add_point_xf(px, py, &xf);
                    }
                }
                SvgNode::Polygon(p) => {
                    let xf = parent_xf.then(&p.transform);
                    for &(px, py) in &p.points {
                        bounds.add_point_xf(px, py, &xf);
                    }
                }
            }
        }
    }

    fn bounds_path(path: &VectorPath, xf: &Transform2d, bounds: &mut BoundsAccum) {
        use crate::path::PathCmd;
        for cmd in &path.cmds {
            match cmd {
                PathCmd::MoveTo(x, y) | PathCmd::LineTo(x, y) => {
                    bounds.add_point_xf(*x, *y, xf);
                }
                PathCmd::BezierTo(cx1, cy1, cx2, cy2, x, y) => {
                    bounds.add_point_xf(*cx1, *cy1, xf);
                    bounds.add_point_xf(*cx2, *cy2, xf);
                    bounds.add_point_xf(*x, *y, xf);
                }
                PathCmd::Close | PathCmd::Winding(_) => {}
            }
        }
    }

    pub fn resolve_gradient_hrefs(&mut self) {
        let grad_clone = self.defs.gradients.clone();
        for grad in self.defs.gradients.values_mut() {
            grad.resolve_href(&grad_clone);
        }
    }
}

struct BoundsAccum {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    has_points: bool,
}

impl BoundsAccum {
    fn new() -> Self {
        Self {
            min_x: f32::MAX,
            min_y: f32::MAX,
            max_x: f32::MIN,
            max_y: f32::MIN,
            has_points: false,
        }
    }

    fn add_point_xf(&mut self, x: f32, y: f32, xf: &Transform2d) {
        let (tx, ty) = xf.apply(x, y);
        self.min_x = self.min_x.min(tx);
        self.min_y = self.min_y.min(ty);
        self.max_x = self.max_x.max(tx);
        self.max_y = self.max_y.max(ty);
        self.has_points = true;
    }

    fn result(&self) -> Option<(f32, f32, f32, f32)> {
        if self.has_points {
            Some((self.min_x, self.min_y, self.max_x, self.max_y))
        } else {
            None
        }
    }
}
