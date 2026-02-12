use crate::makepad_draw::svg::*;
use crate::makepad_draw::vector::{GradientStop, VectorPath};
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Shapes (script_api — no children)
    mod.widgets.Path = #(VectorPathShape::script_api(vm))
    mod.widgets.Rect = #(VectorRect::script_api(vm))
    mod.widgets.Circle = #(VectorCircle::script_api(vm))
    mod.widgets.Ellipse = #(VectorEllipse::script_api(vm))
    mod.widgets.Line = #(VectorLine::script_api(vm))
    mod.widgets.Polyline = #(VectorPolyline::script_api(vm))
    mod.widgets.Polygon = #(VectorPolygon::script_api(vm))
    mod.widgets.Stop = #(VectorStop::script_api(vm))
    mod.widgets.DropShadow = #(VectorDropShadow::script_api(vm))
    mod.widgets.Tween = #(VectorTween::script_api(vm))
    mod.widgets.Rotate = #(VectorRotate::script_api(vm))
    mod.widgets.Scale = #(VectorScale::script_api(vm))
    mod.widgets.Translate = #(VectorTranslate::script_api(vm))
    mod.widgets.SkewX = #(VectorSkewX::script_api(vm))
    mod.widgets.SkewY = #(VectorSkewY::script_api(vm))

    // Containers (script_component — have vec children)
    mod.widgets.Group = #(VectorGroup::script_component(vm))
    mod.widgets.Gradient = #(VectorGradient::script_component(vm))
    mod.widgets.RadGradient = #(VectorRadGradient::script_component(vm))
    mod.widgets.Filter = #(VectorFilter::script_component(vm))

    // The widget
    mod.widgets.VectorBase = #(Vector::register_widget(vm))
    mod.widgets.Vector = set_type_default() do mod.widgets.VectorBase{
        width: Fit
        height: Fit
    }
}

// ---- Helpers ----

fn sv_f32(v: &ScriptValue) -> Option<f32> {
    v.as_number().map(|n| n as f32)
}

fn color_u32_to_rgba(c: u32) -> (f32, f32, f32, f32) {
    let r = ((c >> 24) & 0xff) as f32 / 255.0;
    let g = ((c >> 16) & 0xff) as f32 / 255.0;
    let b = ((c >> 8) & 0xff) as f32 / 255.0;
    let a = (c & 0xff) as f32 / 255.0;
    (r, g, b, a)
}

fn sv_to_anim_string(vm: &mut ScriptVm, v: &ScriptValue) -> String {
    if v.is_nil() {
        return String::new();
    }
    if let Some(c) = v.as_color() {
        let (r, g, b, a) = color_u32_to_rgba(c);
        return format!(
            "rgba({},{},{},{})",
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
            a
        );
    }
    if let Some(f) = sv_f32(v) {
        return format!("{}", f);
    }
    if let Some(_s) = v.as_string() {
        let mut out = String::new();
        vm.bx.heap.string_with(*v, |_, content| {
            out = content.to_string();
        });
        return out;
    }
    String::new()
}

fn sv_opt_str(v: &ScriptValue) -> Option<String> {
    if v.is_nil() {
        return None;
    }
    sv_f32(v).map(|f| format!("{}", f))
}

fn sv_opt_str_vec(v: &[ScriptValue]) -> Option<Vec<String>> {
    if v.is_empty() {
        return None;
    }
    Some(
        v.iter()
            .filter_map(|v| sv_f32(v).map(|f| format!("{}", f)))
            .collect(),
    )
}

fn resolve_paint(
    vm: &mut ScriptVm,
    value: &ScriptValue,
    defs: &mut SvgDefs,
    gc: &mut usize,
) -> Option<SvgPaint> {
    if value.is_nil() {
        return None;
    }
    if let Some(b) = value.as_bool() {
        return if !b { Some(SvgPaint::None) } else { None };
    }
    if let Some(c) = value.as_color() {
        let (r, g, b, a) = color_u32_to_rgba(c);
        return Some(SvgPaint::Color(r, g, b, a));
    }
    if let Some(obj) = value.as_object() {
        let tid = vm.bx.heap.object_type_id(obj)?;
        let grad = if tid == VectorGradient::script_type_id_static() {
            VectorGradient::script_from_value(vm, *value).to_svg_gradient()
        } else if tid == VectorRadGradient::script_type_id_static() {
            VectorRadGradient::script_from_value(vm, *value).to_svg_gradient()
        } else {
            return None;
        };
        let id = format!("__vg_{}", *gc);
        *gc += 1;
        defs.gradients.insert(id.clone(), grad);
        return Some(SvgPaint::GradientRef(id));
    }
    None
}

fn is_tween(vm: &ScriptVm, value: &ScriptValue) -> bool {
    value.as_object().map_or(false, |obj| {
        vm.bx
            .heap
            .type_matches_id(obj, VectorTween::script_type_id_static())
    })
}

fn resolve_repeat(v: &ScriptValue) -> RepeatCount {
    if let Some(b) = v.as_bool() {
        if b {
            RepeatCount::Indefinite
        } else {
            RepeatCount::Count(1.0)
        }
    } else if let Some(n) = sv_f32(v) {
        RepeatCount::Count(n)
    } else {
        RepeatCount::Count(1.0)
    }
}

fn resolve_transform(
    vm: &mut ScriptVm,
    value: &ScriptValue,
) -> (Transform2d, Vec<SvgAnimateTransform>) {
    let mut xf = Transform2d::identity();
    let mut anims = Vec::new();
    if value.is_nil() {
        return (xf, anims);
    }
    if let Some(obj) = value.as_object() {
        resolve_one_transform(vm, obj, *value, &mut xf, &mut anims);
        return (xf, anims);
    }
    if let Some(arr) = value.as_array() {
        let len = vm.bx.heap.array_len(arr);
        for i in 0..len {
            let item = vm.bx.heap.array_index_unchecked(arr, i);
            if let Some(obj) = item.as_object() {
                resolve_one_transform(vm, obj, item, &mut xf, &mut anims);
            }
        }
    }
    (xf, anims)
}

fn resolve_one_transform(
    vm: &mut ScriptVm,
    obj: ScriptObject,
    value: ScriptValue,
    xf: &mut Transform2d,
    anims: &mut Vec<SvgAnimateTransform>,
) {
    let Some(tid) = vm.bx.heap.object_type_id(obj) else {
        return;
    };
    let deg_to_rad = std::f32::consts::PI / 180.0;
    if tid == VectorRotate::script_type_id_static() {
        let t = VectorRotate::script_from_value(vm, value);
        if t.dur > 0.0 {
            anims.push(t.to_animate_transform());
        } else {
            *xf = Transform2d::rotate(sv_f32(&t.deg).unwrap_or(0.0) * deg_to_rad).then(xf);
        }
    } else if tid == VectorScale::script_type_id_static() {
        let t = VectorScale::script_from_value(vm, value);
        if t.dur > 0.0 {
            anims.push(t.to_animate_transform());
        } else {
            let sx = sv_f32(&t.x).unwrap_or(1.0);
            *xf = Transform2d::scale(sx, sv_f32(&t.y).unwrap_or(sx)).then(xf);
        }
    } else if tid == VectorTranslate::script_type_id_static() {
        let t = VectorTranslate::script_from_value(vm, value);
        if t.dur > 0.0 {
            anims.push(t.to_animate_transform());
        } else {
            *xf = Transform2d::translate(sv_f32(&t.x).unwrap_or(0.0), sv_f32(&t.y).unwrap_or(0.0))
                .then(xf);
        }
    } else if tid == VectorSkewX::script_type_id_static() {
        let t = VectorSkewX::script_from_value(vm, value);
        if t.dur > 0.0 {
            anims.push(t.to_animate_transform());
        } else {
            *xf = Transform2d::skew_x(sv_f32(&t.deg).unwrap_or(0.0) * deg_to_rad).then(xf);
        }
    } else if tid == VectorSkewY::script_type_id_static() {
        let t = VectorSkewY::script_from_value(vm, value);
        if t.dur > 0.0 {
            anims.push(t.to_animate_transform());
        } else {
            *xf = Transform2d::skew_y(sv_f32(&t.deg).unwrap_or(0.0) * deg_to_rad).then(xf);
        }
    }
}

fn resolve_linecap(vm: &ScriptVm, v: &ScriptValue) -> LineCap {
    vm.bx
        .heap
        .string_with(*v, |_, s| match s {
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => LineCap::Butt,
        })
        .unwrap_or(LineCap::Butt)
}

fn resolve_linejoin(vm: &ScriptVm, v: &ScriptValue) -> LineJoin {
    vm.bx
        .heap
        .string_with(*v, |_, s| match s {
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => LineJoin::Miter,
        })
        .unwrap_or(LineJoin::Miter)
}

fn resolve_filter(
    vm: &mut ScriptVm,
    filter: &ScriptValue,
    defs: &mut SvgDefs,
    gc: &mut usize,
) -> Option<String> {
    if filter.is_nil() {
        return None;
    }
    if let Some(obj) = filter.as_object() {
        if vm
            .bx
            .heap
            .type_matches_id(obj, VectorFilter::script_type_id_static())
        {
            let f = VectorFilter::script_from_value(vm, *filter);
            let id = format!("__vf_{}", *gc);
            *gc += 1;
            defs.filters.insert(id.clone(), f.to_svg_filter(id.clone()));
            return Some(id);
        }
    }
    None
}

fn is_transform_type(vm: &ScriptVm, obj: ScriptObject) -> bool {
    let Some(tid) = vm.bx.heap.object_type_id(obj) else {
        return false;
    };
    tid == VectorTranslate::script_type_id_static()
        || tid == VectorScale::script_type_id_static()
        || tid == VectorRotate::script_type_id_static()
        || tid == VectorSkewX::script_type_id_static()
        || tid == VectorSkewY::script_type_id_static()
}

fn build_style(
    vm: &mut ScriptVm,
    fill: &ScriptValue,
    fill_opacity: Option<f32>,
    stroke: &ScriptValue,
    stroke_width: &ScriptValue,
    stroke_opacity: &ScriptValue,
    opacity: &ScriptValue,
    stroke_linecap: &ScriptValue,
    stroke_linejoin: &ScriptValue,
    filter: &ScriptValue,
    shader_id: f32,
    defs: &mut SvgDefs,
    gc: &mut usize,
    anims: &mut Vec<SvgAnimate>,
) -> SvgStyle {
    let mut style = SvgStyle::default();
    if is_tween(vm, fill) {
        anims.push(
            VectorTween::script_from_value(vm, *fill).to_svg_animate(vm, AnimateAttribute::Fill),
        );
    } else if let Some(p) = resolve_paint(vm, fill, defs, gc) {
        style.fill = Some(p);
    }
    if is_tween(vm, stroke) {
        anims.push(
            VectorTween::script_from_value(vm, *stroke)
                .to_svg_animate(vm, AnimateAttribute::Stroke),
        );
    } else if let Some(p) = resolve_paint(vm, stroke, defs, gc) {
        style.stroke = Some(p);
    }
    if is_tween(vm, stroke_width) {
        anims.push(
            VectorTween::script_from_value(vm, *stroke_width)
                .to_svg_animate(vm, AnimateAttribute::StrokeWidth),
        );
    } else {
        style.stroke_width = sv_f32(stroke_width).unwrap_or(0.0);
    }
    if is_tween(vm, stroke_opacity) {
        anims.push(
            VectorTween::script_from_value(vm, *stroke_opacity)
                .to_svg_animate(vm, AnimateAttribute::StrokeOpacity),
        );
    } else {
        style.stroke_opacity = sv_f32(stroke_opacity).unwrap_or(1.0);
    }
    if is_tween(vm, opacity) {
        anims.push(
            VectorTween::script_from_value(vm, *opacity)
                .to_svg_animate(vm, AnimateAttribute::Opacity),
        );
    } else {
        style.opacity = sv_f32(opacity).unwrap_or(1.0);
    }
    style.fill_opacity = fill_opacity.unwrap_or(1.0);
    style.stroke_linecap = resolve_linecap(vm, stroke_linecap);
    style.stroke_linejoin = resolve_linejoin(vm, stroke_linejoin);
    style.filter = resolve_filter(vm, filter, defs, gc);
    style.shader_id = shader_id;
    style
}

// ---- Data types ----

#[derive(Script, ScriptHook, Default)]
pub struct VectorStop {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub offset: f32,
    #[live]
    pub color: ScriptValue,
    #[live(1.0)]
    pub opacity: f32,
}

impl VectorStop {
    fn to_gradient_stop(&self) -> GradientStop {
        let (r, g, b, a) = self
            .color
            .as_color()
            .map(color_u32_to_rgba)
            .unwrap_or((0.0, 0.0, 0.0, 1.0));
        let fa = a * self.opacity;
        GradientStop {
            offset: self.offset,
            color: [r * fa, g * fa, b * fa, fa],
        }
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorTween {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub from: ScriptValue,
    #[live]
    pub to: ScriptValue,
    #[live]
    pub values: Vec<ScriptValue>,
    #[live]
    pub dur: f32,
    #[live]
    pub loop_: ScriptValue,
    #[live]
    pub begin: f32,
    #[live]
    pub calc: ScriptValue,
    #[live]
    pub fill_mode: ScriptValue,
}

impl VectorTween {
    fn to_svg_animate(&self, vm: &mut ScriptVm, attr: AnimateAttribute) -> SvgAnimate {
        let from_str = sv_to_anim_string(vm, &self.from);
        let to_str = sv_to_anim_string(vm, &self.to);
        let values_strs = if !self.values.is_empty() {
            Some(
                self.values
                    .iter()
                    .map(|v| sv_to_anim_string(vm, v))
                    .collect(),
            )
        } else {
            None
        };
        let calc_mode = vm
            .bx
            .heap
            .string_with(self.calc, |_, s| match s {
                "discrete" => AnimateCalcMode::Discrete,
                "paced" => AnimateCalcMode::Paced,
                "spline" => AnimateCalcMode::Spline,
                _ => AnimateCalcMode::Linear,
            })
            .unwrap_or(AnimateCalcMode::Linear);
        let fill = vm
            .bx
            .heap
            .string_with(self.fill_mode, |_, s| match s {
                "freeze" => AnimateFill::Freeze,
                _ => AnimateFill::Remove,
            })
            .unwrap_or(AnimateFill::Remove);
        SvgAnimate {
            attribute: attr,
            from: if from_str.is_empty() {
                None
            } else {
                Some(from_str)
            },
            to: if to_str.is_empty() {
                None
            } else {
                Some(to_str)
            },
            values: values_strs,
            key_times: None,
            key_splines: None,
            dur: if self.dur > 0.0 { self.dur } else { 1.0 },
            begin: self.begin,
            repeat_count: resolve_repeat(&self.loop_),
            calc_mode,
            fill,
        }
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorDropShadow {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub dx: f32,
    #[live]
    pub dy: f32,
    #[live]
    pub blur: f32,
    #[live]
    pub color: ScriptValue,
    #[live(1.0)]
    pub opacity: f32,
}

impl VectorDropShadow {
    fn to_svg_filter_effect(&self) -> SvgFilterEffect {
        let (r, g, b, a) = self
            .color
            .as_color()
            .map(color_u32_to_rgba)
            .unwrap_or((0.0, 0.0, 0.0, 1.0));
        SvgFilterEffect::DropShadow {
            dx: self.dx,
            dy: self.dy,
            std_dev: self.blur,
            color: (r, g, b, a * self.opacity),
        }
    }
}

// ---- Transform types ----

#[derive(Script, ScriptHook, Default)]
pub struct VectorRotate {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub deg: ScriptValue,
    #[live]
    pub cx: f32,
    #[live]
    pub cy: f32,
    #[live]
    pub from: ScriptValue,
    #[live]
    pub to: ScriptValue,
    #[live]
    pub values: Vec<ScriptValue>,
    #[live]
    pub dur: f32,
    #[live]
    pub loop_: ScriptValue,
    #[live]
    pub begin: f32,
}

impl VectorRotate {
    fn to_animate_transform(&self) -> SvgAnimateTransform {
        SvgAnimateTransform {
            kind: AnimateTransformType::Rotate,
            from: sv_opt_str(&self.from),
            to: sv_opt_str(&self.to),
            values: sv_opt_str_vec(&self.values),
            key_times: None,
            dur: if self.dur > 0.0 { self.dur } else { 1.0 },
            begin: self.begin,
            repeat_count: resolve_repeat(&self.loop_),
            calc_mode: AnimateCalcMode::Linear,
            fill: AnimateFill::Remove,
        }
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorScale {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub x: ScriptValue,
    #[live]
    pub y: ScriptValue,
    #[live]
    pub from: ScriptValue,
    #[live]
    pub to: ScriptValue,
    #[live]
    pub values: Vec<ScriptValue>,
    #[live]
    pub dur: f32,
    #[live]
    pub loop_: ScriptValue,
    #[live]
    pub begin: f32,
}

impl VectorScale {
    fn to_animate_transform(&self) -> SvgAnimateTransform {
        SvgAnimateTransform {
            kind: AnimateTransformType::Scale,
            from: sv_opt_str(&self.from),
            to: sv_opt_str(&self.to),
            values: sv_opt_str_vec(&self.values),
            key_times: None,
            dur: if self.dur > 0.0 { self.dur } else { 1.0 },
            begin: self.begin,
            repeat_count: resolve_repeat(&self.loop_),
            calc_mode: AnimateCalcMode::Linear,
            fill: AnimateFill::Remove,
        }
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorTranslate {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub x: ScriptValue,
    #[live]
    pub y: ScriptValue,
    #[live]
    pub from: ScriptValue,
    #[live]
    pub to: ScriptValue,
    #[live]
    pub values: Vec<ScriptValue>,
    #[live]
    pub dur: f32,
    #[live]
    pub loop_: ScriptValue,
    #[live]
    pub begin: f32,
}

impl VectorTranslate {
    fn to_animate_transform(&self) -> SvgAnimateTransform {
        SvgAnimateTransform {
            kind: AnimateTransformType::Translate,
            from: sv_opt_str(&self.from),
            to: sv_opt_str(&self.to),
            values: sv_opt_str_vec(&self.values),
            key_times: None,
            dur: if self.dur > 0.0 { self.dur } else { 1.0 },
            begin: self.begin,
            repeat_count: resolve_repeat(&self.loop_),
            calc_mode: AnimateCalcMode::Linear,
            fill: AnimateFill::Remove,
        }
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorSkewX {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub deg: ScriptValue,
    #[live]
    pub from: ScriptValue,
    #[live]
    pub to: ScriptValue,
    #[live]
    pub values: Vec<ScriptValue>,
    #[live]
    pub dur: f32,
    #[live]
    pub loop_: ScriptValue,
    #[live]
    pub begin: f32,
}

impl VectorSkewX {
    fn to_animate_transform(&self) -> SvgAnimateTransform {
        SvgAnimateTransform {
            kind: AnimateTransformType::SkewX,
            from: sv_opt_str(&self.from),
            to: sv_opt_str(&self.to),
            values: sv_opt_str_vec(&self.values),
            key_times: None,
            dur: if self.dur > 0.0 { self.dur } else { 1.0 },
            begin: self.begin,
            repeat_count: resolve_repeat(&self.loop_),
            calc_mode: AnimateCalcMode::Linear,
            fill: AnimateFill::Remove,
        }
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorSkewY {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub deg: ScriptValue,
    #[live]
    pub from: ScriptValue,
    #[live]
    pub to: ScriptValue,
    #[live]
    pub values: Vec<ScriptValue>,
    #[live]
    pub dur: f32,
    #[live]
    pub loop_: ScriptValue,
    #[live]
    pub begin: f32,
}

impl VectorSkewY {
    fn to_animate_transform(&self) -> SvgAnimateTransform {
        SvgAnimateTransform {
            kind: AnimateTransformType::SkewY,
            from: sv_opt_str(&self.from),
            to: sv_opt_str(&self.to),
            values: sv_opt_str_vec(&self.values),
            key_times: None,
            dur: if self.dur > 0.0 { self.dur } else { 1.0 },
            begin: self.begin,
            repeat_count: resolve_repeat(&self.loop_),
            calc_mode: AnimateCalcMode::Linear,
            fill: AnimateFill::Remove,
        }
    }
}

// ---- Shape types ----

#[derive(Script, ScriptHook, Default)]
pub struct VectorPathShape {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub d: ScriptValue,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorPathShape {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        let mut path = VectorPath::new();
        if is_tween(vm, &self.d) {
            let tween = VectorTween::script_from_value(vm, self.d);
            let anim = tween.to_svg_animate(vm, AnimateAttribute::D);
            if let Some(ref values) = anim.values {
                if let Some(first) = values.first() {
                    parse_path_data(first, &mut path);
                }
            } else if let Some(ref from) = anim.from {
                parse_path_data(from, &mut path);
            }
            anims.push(anim);
        } else {
            vm.bx.heap.string_with(self.d, |_, s| {
                parse_path_data(s, &mut path);
            });
        }
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Path(SvgPath {
            id: None,
            style,
            transform: xf,
            path,
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorRect {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub x: ScriptValue,
    #[live]
    pub y: ScriptValue,
    #[live]
    pub w: ScriptValue,
    #[live]
    pub h: ScriptValue,
    #[live]
    pub rx: f32,
    #[live]
    pub ry: f32,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorRect {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        for (val, attr) in [
            (&self.x, AnimateAttribute::X),
            (&self.y, AnimateAttribute::Y),
            (&self.w, AnimateAttribute::Width),
            (&self.h, AnimateAttribute::Height),
        ] {
            if is_tween(vm, val) {
                anims.push(VectorTween::script_from_value(vm, *val).to_svg_animate(vm, attr));
            }
        }
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Rect(SvgRect {
            id: None,
            style,
            transform: xf,
            x: sv_f32(&self.x).unwrap_or(0.0),
            y: sv_f32(&self.y).unwrap_or(0.0),
            width: sv_f32(&self.w).unwrap_or(0.0),
            height: sv_f32(&self.h).unwrap_or(0.0),
            rx: self.rx,
            ry: self.ry,
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorCircle {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub cx: ScriptValue,
    #[live]
    pub cy: ScriptValue,
    #[live]
    pub r: ScriptValue,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorCircle {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        for (val, attr) in [
            (&self.cx, AnimateAttribute::Cx),
            (&self.cy, AnimateAttribute::Cy),
            (&self.r, AnimateAttribute::R),
        ] {
            if is_tween(vm, val) {
                anims.push(VectorTween::script_from_value(vm, *val).to_svg_animate(vm, attr));
            }
        }
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Circle(SvgCircle {
            id: None,
            style,
            transform: xf,
            cx: sv_f32(&self.cx).unwrap_or(0.0),
            cy: sv_f32(&self.cy).unwrap_or(0.0),
            r: sv_f32(&self.r).unwrap_or(0.0),
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorEllipse {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub cx: f32,
    #[live]
    pub cy: f32,
    #[live]
    pub rx: f32,
    #[live]
    pub ry: f32,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorEllipse {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Ellipse(SvgEllipse {
            id: None,
            style,
            transform: xf,
            cx: self.cx,
            cy: self.cy,
            rx: self.rx,
            ry: self.ry,
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorLine {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub x1: f32,
    #[live]
    pub y1: f32,
    #[live]
    pub x2: f32,
    #[live]
    pub y2: f32,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorLine {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Line(SvgLine {
            id: None,
            style,
            transform: xf,
            x1: self.x1,
            y1: self.y1,
            x2: self.x2,
            y2: self.y2,
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorPolyline {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub pts: Vec<ScriptValue>,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorPolyline {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        let points = sv_to_points(&self.pts);
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Polyline(SvgPolyline {
            id: None,
            style,
            transform: xf,
            points,
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, ScriptHook, Default)]
pub struct VectorPolygon {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub pts: Vec<ScriptValue>,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
}

impl VectorPolygon {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        let points = sv_to_points(&self.pts);
        let (xf, at) = resolve_transform(vm, &self.transform);
        SvgNode::Polygon(SvgPolygon {
            id: None,
            style,
            transform: xf,
            points,
            animations: anims,
            animate_transforms: at,
        })
    }
}

fn sv_to_points(pts: &[ScriptValue]) -> Vec<(f32, f32)> {
    let floats: Vec<f32> = pts.iter().filter_map(|v| sv_f32(v)).collect();
    floats
        .chunks(2)
        .filter_map(|c| {
            if c.len() == 2 {
                Some((c[0], c[1]))
            } else {
                None
            }
        })
        .collect()
}

// ---- Container types ----

#[derive(Script, Default)]
pub struct VectorGroup {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub transform: ScriptValue,
    #[live]
    pub fill: ScriptValue,
    #[live]
    pub fill_opacity: Option<f32>,
    #[live]
    pub stroke: ScriptValue,
    #[live]
    pub stroke_width: ScriptValue,
    #[live]
    pub stroke_opacity: ScriptValue,
    #[live]
    pub opacity: ScriptValue,
    #[live]
    pub stroke_linecap: ScriptValue,
    #[live]
    pub stroke_linejoin: ScriptValue,
    #[live]
    pub filter: ScriptValue,
    #[live]
    pub shader_id: f32,
    #[rust]
    child_values: Vec<ScriptValue>,
    #[rust]
    extra_transforms: Vec<ScriptValue>,
}

impl ScriptHook for VectorGroup {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        self.child_values.clear();
        self.extra_transforms.clear();
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(val_obj) = kv.value.as_object() {
                        if is_transform_type(vm, val_obj) {
                            self.extra_transforms.push(kv.value);
                        } else {
                            self.child_values.push(kv.value);
                        }
                    }
                }
            });
        }
    }
}

impl VectorGroup {
    fn to_svg_node(&self, vm: &mut ScriptVm, defs: &mut SvgDefs, gc: &mut usize) -> SvgNode {
        let mut children = Vec::new();
        for cv in &self.child_values {
            if let Some(val_obj) = cv.as_object() {
                if let Some(node) = dispatch_shape(vm, val_obj, *cv, defs, gc) {
                    children.push(node);
                }
            }
        }
        let mut anims = Vec::new();
        let style = build_style(
            vm,
            &self.fill,
            self.fill_opacity,
            &self.stroke,
            &self.stroke_width,
            &self.stroke_opacity,
            &self.opacity,
            &self.stroke_linecap,
            &self.stroke_linejoin,
            &self.filter,
            self.shader_id,
            defs,
            gc,
            &mut anims,
        );
        let (mut xf, mut at) = resolve_transform(vm, &self.transform);
        for extra in &self.extra_transforms {
            if let Some(obj) = extra.as_object() {
                resolve_one_transform(vm, obj, *extra, &mut xf, &mut at);
            }
        }
        SvgNode::Group(SvgGroup {
            id: None,
            style,
            transform: xf,
            children,
            animations: anims,
            animate_transforms: at,
        })
    }
}

#[derive(Script, Default)]
pub struct VectorGradient {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub x1: f32,
    #[live]
    pub y1: f32,
    #[live]
    pub x2: f32,
    #[live(1.0)]
    pub y2: f32,
    #[live]
    pub units: ScriptValue,
    #[live]
    pub spread: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[rust]
    stops: Vec<VectorStop>,
}

impl ScriptHook for VectorGradient {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        self.stops.clear();
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(val_obj) = kv.value.as_object() {
                        if vm
                            .bx
                            .heap
                            .type_matches_id(val_obj, VectorStop::script_type_id_static())
                        {
                            self.stops.push(VectorStop::script_from_value(vm, kv.value));
                        }
                    }
                }
            });
        }
    }
}

impl VectorGradient {
    fn to_svg_gradient(&self) -> SvgGradient {
        let mut grad = SvgGradient::new_linear();
        grad.x1 = self.x1;
        grad.y1 = self.y1;
        grad.x2 = self.x2;
        grad.y2 = self.y2;
        grad.stops = self.stops.iter().map(|s| s.to_gradient_stop()).collect();
        grad
    }
}

#[derive(Script, Default)]
pub struct VectorRadGradient {
    #[source]
    source: ScriptObjectRef,
    #[live(0.5)]
    pub cx: f32,
    #[live(0.5)]
    pub cy: f32,
    #[live(0.5)]
    pub r: f32,
    #[live]
    pub fx: f32,
    #[live]
    pub fy: f32,
    #[live]
    pub units: ScriptValue,
    #[live]
    pub spread: ScriptValue,
    #[live]
    pub transform: ScriptValue,
    #[rust]
    stops: Vec<VectorStop>,
}

impl ScriptHook for VectorRadGradient {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        self.stops.clear();
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(val_obj) = kv.value.as_object() {
                        if vm
                            .bx
                            .heap
                            .type_matches_id(val_obj, VectorStop::script_type_id_static())
                        {
                            self.stops.push(VectorStop::script_from_value(vm, kv.value));
                        }
                    }
                }
            });
        }
    }
}

impl VectorRadGradient {
    fn to_svg_gradient(&self) -> SvgGradient {
        let mut grad = SvgGradient::new_radial();
        grad.cx = self.cx;
        grad.cy = self.cy;
        grad.r = self.r;
        grad.fx = if self.fx != 0.0 { self.fx } else { self.cx };
        grad.fy = if self.fy != 0.0 { self.fy } else { self.cy };
        grad.stops = self.stops.iter().map(|s| s.to_gradient_stop()).collect();
        grad
    }
}

#[derive(Script, Default)]
pub struct VectorFilter {
    #[source]
    source: ScriptObjectRef,
    #[rust]
    effects: Vec<SvgFilterEffect>,
}

impl ScriptHook for VectorFilter {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        self.effects.clear();
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(val_obj) = kv.value.as_object() {
                        if vm
                            .bx
                            .heap
                            .type_matches_id(val_obj, VectorDropShadow::script_type_id_static())
                        {
                            self.effects.push(
                                VectorDropShadow::script_from_value(vm, kv.value)
                                    .to_svg_filter_effect(),
                            );
                        }
                    }
                }
            });
        }
    }
}

impl VectorFilter {
    fn to_svg_filter(&self, id: String) -> SvgFilter {
        SvgFilter {
            id,
            effects: self.effects.clone(),
        }
    }
}

// ---- Type dispatch ----

fn dispatch_shape(
    vm: &mut ScriptVm,
    val_obj: ScriptObject,
    value: ScriptValue,
    defs: &mut SvgDefs,
    gc: &mut usize,
) -> Option<SvgNode> {
    let tid = vm.bx.heap.object_type_id(val_obj)?;
    macro_rules! shape {
        ($T:ty) => {{
            Some(<$T>::script_from_value(vm, value).to_svg_node(vm, defs, gc))
        }};
    }
    match tid {
        t if t == VectorRect::script_type_id_static() => shape!(VectorRect),
        t if t == VectorCircle::script_type_id_static() => shape!(VectorCircle),
        t if t == VectorPathShape::script_type_id_static() => shape!(VectorPathShape),
        t if t == VectorEllipse::script_type_id_static() => shape!(VectorEllipse),
        t if t == VectorLine::script_type_id_static() => shape!(VectorLine),
        t if t == VectorPolyline::script_type_id_static() => shape!(VectorPolyline),
        t if t == VectorPolygon::script_type_id_static() => shape!(VectorPolygon),
        t if t == VectorGroup::script_type_id_static() => shape!(VectorGroup),
        _ => None,
    }
}

// ---- The Vector Widget ----

#[derive(Script, Widget)]
pub struct Vector {
    #[uid] uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    pub viewbox: Vec4f,
    #[redraw]
    #[live]
    draw_svg: DrawSvg,
    #[rust]
    doc: SvgDocument,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    has_animations: bool,
}

impl ScriptHook for Vector {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if !apply.is_eval() {
            self.doc = SvgDocument {
                viewbox: None,
                width: None,
                height: None,
                defs: SvgDefs::default(),
                root: Vec::new(),
            };
            self.draw_svg.cache_valid = false;
            self.has_animations = false;
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if apply.is_eval() {
            return;
        }
        if self.viewbox.z > 0.0 || self.viewbox.w > 0.0 {
            self.doc.viewbox = Some(ViewBox {
                x: self.viewbox.x,
                y: self.viewbox.y,
                width: self.viewbox.z,
                height: self.viewbox.w,
            });
            self.doc.width = Some(self.viewbox.z);
            self.doc.height = Some(self.viewbox.w);
        }
        let mut gc = 0usize;
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    if let Some(val_obj) = kv.value.as_object() {
                        if let Some(node) =
                            dispatch_shape(vm, val_obj, kv.value, &mut self.doc.defs, &mut gc)
                        {
                            self.doc.root.push(node);
                        }
                    }
                }
            });
        }
        self.has_animations = self.doc.has_animations();
        self.draw_svg.set_doc_bounds(&self.doc);
        if let Some(ref vb) = self.doc.viewbox {
            self.draw_svg.content_bounds = (vb.x, vb.y, vb.x + vb.width, vb.y + vb.height);
            self.draw_svg.content_size = dvec2(vb.width as f64, vb.height as f64);
        }
    }
}

impl Widget for Vector {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.has_animations {
            if let Event::NextFrame(ne) = event {
                self.time = ne.time;
                self.draw_svg.cache_valid = false;
                self.draw_svg.draw_super.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
            if let Event::Startup = event {
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.doc.root.is_empty() {
            return DrawStep::done();
        }
        let sw = self.draw_svg.content_size.x;
        let sh = self.draw_svg.content_size.y;
        if sw <= 0.0 || sh <= 0.0 {
            return DrawStep::done();
        }
        let walk = Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(sw),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(sh),
                other => other,
            },
            metrics: walk.metrics,
        };
        let rect = cx.walk_turtle(walk);
        self.draw_svg.svg_doc = Some(std::mem::take(&mut self.doc));
        self.draw_svg.has_animations = self.has_animations;
        self.draw_svg.render_to_rect(cx, &rect, self.time as f32);
        self.doc = self.draw_svg.svg_doc.take().unwrap_or_default();
        DrawStep::done()
    }
}
