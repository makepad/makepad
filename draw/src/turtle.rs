use crate::{
    cx_2d::Cx2d,
    makepad_platform::*,
    size_expr::{SizeExprContext, SizeExprId, SizeExprSimple, SizeExprStore, SizeExprUnit},
};

script_mod! {
    mod.turtle = {
        Base: mod.std.set_type_default() do #(Base::script_api(vm))
        FitBound: mod.std.set_type_default() do #(FitBound::script_api(vm))
        Size: mod.std.set_type_default() do #(Size::script_api(vm)),
        ..me.Size,
        Metrics: mod.std.set_type_default() do #(Metrics::script_api(vm))
        RowAlign: mod.std.set_type_default() do #(RowAlign::script_api(vm))
        Distribute: mod.std.set_type_default() do #(Distribute::script_api(vm))
        Flow: mod.std.set_type_default() do #(Flow::script_api(vm)),
        ..me.Flow,
        Align: mod.std.set_type_default() do #(Align::script_api(vm))
        Inset: mod.std.set_type_default() do #(Inset::script_api(vm))
        Layout: mod.std.set_type_default() do #(Layout::script_api(vm))
        CellAlign: mod.std.set_type_default() do #(CellAlign::script_api(vm))
        CellPlacement: mod.std.set_type_default() do #(CellPlacement::script_api(vm))
        Walk: mod.std.set_type_default() do #(Walk::script_api(vm)),
        TopLeft: me.Align{x:0., y:0.}
        Center: me.Align{x:0.5, y:0.5}
        HCenter: me.Align{x:0.5, y:0.}
        VCenter: me.Align{x:0., y:0.5}
    }
}

/// Alignment of a grid child within one cell axis.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Script, ScriptHook)]
pub enum CellAlign {
    #[default]
    #[pick]
    Stretch,
    Start,
    Center,
    End,
}

/// Optional grid placement carried by every widget's flattened `Walk`.
/// Rows and columns are one-based; zero selects automatic placement.
#[derive(Copy, Clone, Debug, Default, PartialEq, Script, ScriptHook)]
pub struct CellPlacement {
    #[live]
    pub col: u32,
    #[live]
    pub row: u32,
    #[live]
    pub col_span: u32,
    #[live]
    pub row_span: u32,
    #[live]
    pub area: LiveId,
    #[live]
    pub align_self: Option<CellAlign>,
    #[live]
    pub justify_self: Option<CellAlign>,
}

#[derive(Clone, Debug)]
struct DeferredFill {
    grow: f64,
    shrink: f64,
    unclamped_basis: f64,
    basis: f64,
    max: Option<f64>,
    min: Option<f64>,
    delta: f64,
    frozen: bool,
}

#[derive(Debug)]
pub enum AlignEntry {
    Unset,
    Area(Area),
    ShiftTurtle {
        area: Area,
        shift: Vec2d,
        skip: usize,
    },
    SkipTurtle {
        skip: usize,
    },
    BeginClip(Vec2d, Vec2d),
    EndClip,
}

/// Specifies how a turtle should walk.
#[derive(Copy, Clone, Default, Debug, Script, ScriptHook)]
pub struct Walk {
    #[doc(hidden)]
    #[live]
    pub abs_pos: Option<Vec2d>,

    /// The margin around this walk's rectangle.
    #[live]
    pub margin: Inset,

    /// The desired width of this walk's rectangle.
    #[live]
    pub width: Size,

    /// The desired height of this walk's rectangle.
    #[live]
    pub height: Size,

    /// Content-box constraints. These are independent of the historical
    /// margin-box bounds carried by `Size::Fit` and `Size::Fill`.
    #[live]
    pub min_width: Option<FitBound>,
    #[live]
    pub max_width: Option<FitBound>,
    #[live]
    pub min_height: Option<FitBound>,
    #[live]
    pub max_height: Option<FitBound>,

    /// Preferred content-box width divided by height.
    #[live]
    pub aspect: Option<f64>,

    /// Definite-grid placement. Other turtle flows ignore this metadata.
    #[live]
    pub cell: Option<CellPlacement>,

    #[live]
    pub metrics: Metrics,

    /// True only for an internally-materialized deferred walk. This keeps
    /// absolute placement internal to the flex pass distinguishable from a
    /// user-authored `abs_pos` without exposing layout provenance to script.
    #[doc(hidden)]
    #[rust]
    pub deferred: bool,

    #[doc(hidden)]
    #[rust]
    pub flow_index: u32,
}

impl Walk {
    #[inline]
    fn needs_resolve(self) -> bool {
        let size_needs_resolve = |size| {
            matches!(size, Size::Rel { .. } | Size::Expr(_))
                || matches!(
                    size,
                    Size::Fill {
                        basis,
                        min,
                        max,
                        ..
                    } if !matches!(basis, FitBound::Abs(_)) || min.is_some() || max.is_some()
                )
                || matches!(size, Size::Fit { min, max } if min.is_some() || max.is_some())
        };
        size_needs_resolve(self.width)
            || size_needs_resolve(self.height)
            || self.min_width.is_some()
            || self.max_width.is_some()
            || self.min_height.is_some()
            || self.max_height.is_some()
            || self.aspect.is_some()
    }

    /// Returns a `Walk` with `width` and `height` set to the given value, and no margin.
    pub fn new(width: Size, height: Size) -> Self {
        Self {
            width,
            height,
            ..Self::default()
        }
    }

    /// Returns a `Walk` with both `width` and `height` set to 0.0, and no margin.
    pub fn empty() -> Self {
        Self::fixed(0.0, 0.0)
    }

    /// Returns a `Walk` with both `width` and `height` set to `Size::fill()`, and no margin.
    pub fn fill() -> Self {
        Self {
            width: Size::fill(),
            height: Size::fill(),
            ..Self::default()
        }
    }

    /// Returns a `Walk` with `width` and `height` set to the given fixed values, and no margin.
    pub fn fixed(width: f64, height: f64) -> Self {
        Self {
            width: Size::Fixed(width),
            height: Size::Fixed(height),
            ..Self::default()
        }
    }

    /// Returns a `Walk` with both `width` and `height` set to `Size::fit()`, and no margin.
    pub fn fit() -> Self {
        Self {
            width: Size::fit(),
            height: Size::fit(),
            ..Self::default()
        }
    }

    /// Returns a `Walk` with `width` set to `Size::fill()`, `height` set to `Size::fit()`, and no
    /// margin.
    pub fn fill_fit() -> Self {
        Self {
            width: Size::fill(),
            height: Size::fit(),
            ..Self::default()
        }
    }

    /// Returns a copy of this `Walk` with `margin` set to the given value.
    pub fn with_margin(self, margin: Inset) -> Self {
        Self { margin, ..self }
    }

    /// Returns a copy of this `Walk` with the left margin set to the given value.
    pub fn with_margin_left(self, left: f64) -> Self {
        Self {
            margin: self.margin.with_left(left),
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the right margin set to the given value.
    pub fn with_margin_top(self, top: f64) -> Self {
        Self {
            margin: self.margin.with_top(top),
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the bottom margin set to the given value.
    pub fn with_margin_right(self, right: f64) -> Self {
        Self {
            margin: self.margin.with_right(right),
            ..self
        }
    }

    /// Returns a copy of this `Walk` with the bottom margin set to the given value.
    pub fn with_margin_bottom(self, v: f64) -> Self {
        Self {
            margin: self.margin.with_bottom(v),
            ..self
        }
    }
}

#[derive(Copy, Clone, Debug, Script, ScriptHook)]
pub struct Metrics {
    #[live]
    pub descender: f64,
    #[live]
    pub line_gap: f64,
    #[live(1.0)]
    pub line_scale: f64,
}

impl Metrics {
    fn max(self, other: Self) -> Self {
        Self {
            descender: self.descender.max(other.descender),
            line_gap: self.line_gap.max(other.line_gap),
            line_scale: self.line_scale.max(other.line_scale),
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            descender: 0.0,
            line_gap: 0.0,
            line_scale: 1.0,
        }
    }
}
/// Specifies the desired width/height of a walk's rectangle.
///
/// See `Turtle::next_walk_width` and `Turtle::next_walk_height` for details on how the actual
/// width/height is computed based on the desired width/height.
#[derive(Copy, Clone, Debug, PartialEq, Script)]
pub enum Size {
    #[pick {
        weight: 100.0,
        basis: FitBound::Abs(0.0),
        shrink: 0.0,
        min: None,
        max: None,
    }]
    Fill {
        weight: f64,
        basis: FitBound,
        shrink: f64,
        min: Option<f64>,
        max: Option<f64>,
    },
    #[live(200.0)]
    Fixed(f64),
    #[live {
        min: None,
        max: None,
    }]
    Fit {
        min: Option<FitBound>,
        max: Option<FitBound>,
    },
    #[live {
        base: Base::Parent,
        factor: 1.0
    }]
    Rel { base: Base, factor: f64 },
    #[live(SizeExprId(u32::MAX))]
    Expr(SizeExprId),
}

impl Size {
    /// Returns a `Size::Fill` with a default `weight` of `100.0``, and without `min` or `max`
    /// constraints.
    pub fn fill() -> Self {
        Self::Fill {
            weight: 100.0,
            basis: FitBound::Abs(0.0),
            shrink: 0.0,
            min: None,
            max: None,
        }
    }

    /// Returns a `Size::Fit` without `min` or `max` constraints.
    pub fn fit() -> Self {
        Self::Fit {
            min: None,
            max: None,
        }
    }

    /// Returns `true` if this is a `Size::Fill`, or `false` otherwise.
    pub fn is_fill(self) -> bool {
        matches!(self, Self::Fill { .. })
    }

    /// Returns `true` if this is a `Size::Fixed`, or `false` otherwise.
    pub fn is_fixed(self) -> bool {
        matches!(self, Self::Fixed(_))
    }

    /// Returns `true` if this is a `Size::Fit`, or `false` otherwise.
    pub fn is_fit(self) -> bool {
        matches!(self, Self::Fit { .. })
    }

    /// Returns whether this declaration denotes a definite size rather than
    /// content- or distribution-dependent sizing. A contextual declaration
    /// can still fail to resolve when its required context is unknown.
    pub fn is_definite(self) -> bool {
        matches!(self, Self::Fixed(_) | Self::Rel { .. } | Self::Expr(_))
    }

    /// Returns the fixed size if this is a `Size::Fixed`, or `None` otherwise.
    pub fn to_fixed(self) -> Option<f64> {
        match self {
            Self::Fixed(size) => Some(size),
            _ => None,
        }
    }
}

impl Default for Size {
    fn default() -> Self {
        Size::fill()
    }
}

impl ScriptHook for Size {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.as_f64().is_some() || value.as_number().is_some() || value.is_string_like()
    }

    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        // Handle numeric values as Size::Fixed
        if let Some(v) = value.as_f64() {
            *self = Size::Fixed(v);
            return true;
        }
        if let Some(v) = value.as_number() {
            *self = Size::Fixed(v);
            return true;
        }
        if let Some(source) = script_string(vm, value) {
            match intern_size_expression(vm, &source) {
                Ok(SizeExprSimple::Abs(value)) => *self = Size::Fixed(value),
                Ok(SizeExprSimple::Rel { unit, factor }) => {
                    *self = Size::Rel {
                        base: unit.into(),
                        factor,
                    }
                }
                Ok(SizeExprSimple::Compound(id)) => *self = Size::Expr(id),
                Err(error) => {
                    error!("invalid Size expression {:?}: {}", source, error);
                }
            }
            return true;
        }
        // Return false to let the generated code handle normal enum objects
        false
    }

    fn on_custom_to_value(&self, vm: &mut ScriptVm) -> Option<ScriptValue> {
        let Self::Expr(id) = self else {
            return None;
        };
        size_expr_source_to_value(vm, *id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Script)]
pub enum FitBound {
    #[pick(100.0)]
    Abs(f64),
    #[live {
        base: Base::Full,
        factor: 1.0
    }]
    Rel { base: Base, factor: f64 },
    #[live(SizeExprId(u32::MAX))]
    Expr(SizeExprId),
}

impl ScriptHook for FitBound {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.as_number().is_some() || value.is_string_like()
    }

    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        if let Some(value) = value.as_number() {
            *self = Self::Abs(value);
            return true;
        }
        if let Some(source) = script_string(vm, value) {
            match intern_size_expression(vm, &source) {
                Ok(SizeExprSimple::Abs(value)) => *self = Self::Abs(value),
                Ok(SizeExprSimple::Rel { unit, factor }) => {
                    *self = Self::Rel {
                        base: unit.into(),
                        factor,
                    }
                }
                Ok(SizeExprSimple::Compound(id)) => *self = Self::Expr(id),
                Err(error) => error!("invalid FitBound expression {:?}: {}", source, error),
            }
            return true;
        }
        false
    }

    fn on_custom_to_value(&self, vm: &mut ScriptVm) -> Option<ScriptValue> {
        let Self::Expr(id) = self else {
            return None;
        };
        size_expr_source_to_value(vm, *id)
    }
}

impl FitBound {
    pub fn eval_width(self, cx: &Cx2d<'_, '_>) -> Option<f64> {
        let turtle_index = cx.turtles.len().checked_sub(1)?;
        cx.eval_fit_bound_for_turtle(self, Axis::Width, turtle_index)
    }

    pub fn eval_height(self, cx: &Cx2d<'_, '_>) -> Option<f64> {
        let turtle_index = cx.turtles.len().checked_sub(1)?;
        cx.eval_fit_bound_for_turtle(self, Axis::Height, turtle_index)
    }
}
/*
impl LiveHook for FitBound {
    fn skip_apply(&mut self, _cx: &mut Cx, _apply: &Apply, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match nodes[index].value {
            LiveValue::Int64(value) => {
                *self = Self::Abs(value as f64);
                Some(index + 1)
            }
            LiveValue::Float32(value) => {
                *self = Self::Abs(value as f64);
                Some(index + 1)
            }
            LiveValue::Float64(value) => {
                *self = Self::Abs(value);
                Some(index + 1)
            }
            _ => None
        }
    }
}*/

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cx_draw::CxDraw;

    fn with_turtle(size: Vec2d, layout: Layout, test: impl FnOnce(&mut Cx2d)) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx = Cx2d::new(&mut draw);
        cx.begin_root_turtle(size, layout);
        test(&mut cx);
        while !cx.turtles.is_empty() {
            cx.end_turtle();
        }
    }

    fn with_window_child_pass(test: impl FnOnce(&mut Cx2d)) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let window = WindowHandle::new(&mut cx);
        let window_id = window.window_id();
        cx.windows[window_id].is_created = true;
        cx.windows[window_id].window_geom.inner_size = dvec2(1000.0, 800.0);
        let root_pass = DrawPass::new(&mut cx);
        window.set_pass(&mut cx, &root_pass);
        let child_pass = DrawPass::new(&mut cx);
        child_pass.set_pass_parent(&mut cx, &root_pass);
        child_pass.set_size(&mut cx, dvec2(64.0, 48.0));

        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        draw.begin_pass(&child_pass, None);
        {
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_root_turtle(dvec2(64.0, 48.0), Layout::default());
            test(&mut cx);
            while !cx.turtles.is_empty() {
                cx.end_turtle();
            }
        }
        draw.end_pass(&child_pass);
    }

    #[test]
    fn legacy_fixed_fit_fill_flows_remain_stable() {
        with_turtle(dvec2(100.0, 80.0), Layout::flow_right(), |cx| {
            let a = cx.walk_turtle(Walk::fixed(20.0, 10.0));
            let b = cx.walk_turtle(Walk::new(Size::fill(), Size::Fixed(10.0)));
            assert_eq!(a.pos, dvec2(0.0, 0.0));
            assert_eq!(b.pos, dvec2(20.0, 0.0));
            assert_eq!(b.size.x, 80.0);
        });
        with_turtle(dvec2(50.0, 80.0), Layout::flow_right_wrap(), |cx| {
            let _ = cx.walk_turtle(Walk::fixed(30.0, 10.0));
            let wrapped = cx.walk_turtle(Walk::fixed(30.0, 12.0));
            assert_eq!(wrapped.pos, dvec2(0.0, 10.0));
        });
        with_turtle(dvec2(100.0, 80.0), Layout::flow_down(), |cx| {
            let _ = cx.walk_turtle(Walk::fixed(20.0, 10.0));
            let next = cx.walk_turtle(Walk::fixed(20.0, 15.0));
            assert_eq!(next.pos, dvec2(0.0, 10.0));
        });
        with_turtle(dvec2(100.0, 80.0), Layout::flow_overlay(), |cx| {
            let a = cx.walk_turtle(Walk::fixed(20.0, 10.0));
            let b = cx.walk_turtle(Walk::fixed(30.0, 15.0));
            assert_eq!(a.pos, b.pos);
        });
        with_turtle(dvec2(100.0, 80.0), Layout::flow_right(), |cx| {
            cx.begin_turtle(Walk::fit(), Layout::default());
            cx.walk_turtle(Walk::fixed(23.0, 17.0));
            let fitted = cx.end_turtle();
            assert_eq!(fitted.size, dvec2(23.0, 17.0));
        });
    }

    #[test]
    fn parent_resolution_is_strict_and_phase_correct() {
        with_turtle(
            dvec2(300.0, 200.0),
            Layout::default().with_padding_all(10.0),
            |cx| {
                let relative = Walk::new(
                    Size::Rel {
                        base: Base::Parent,
                        factor: 0.5,
                    },
                    Size::Fixed(10.0),
                );
                let before = cx.resolve_walk(relative, ResolveAt::BeforeBegin);
                assert_eq!(before.width.to_fixed(), Some(140.0));
                cx.begin_turtle(Walk::fixed(50.0, 50.0), Layout::default());
                let at_close = cx.resolve_walk(relative, ResolveAt::AtClose);
                assert_eq!(at_close.width.to_fixed(), Some(140.0));
                assert_eq!(cx.resolve_walk(at_close, ResolveAt::AtClose).width.to_fixed(), Some(140.0));
                cx.end_turtle();

                cx.begin_turtle(Walk::new(Size::fit(), Size::Fixed(20.0)), Layout::default());
                let unresolved = cx.resolve_walk(relative, ResolveAt::BeforeBegin);
                assert!(unresolved.width.is_fit(), "Parent must not skip an unknown immediate parent");
                cx.end_turtle();
            },
        );
    }

    #[test]
    fn close_time_bounds_remain_declarative_until_the_turtle_closes() {
        let line = FitBound::Rel {
            base: Base::Line,
            factor: 1.0,
        };
        let unused = FitBound::Rel {
            base: Base::Unused,
            factor: 1.0,
        };

        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            cx.walk_turtle(Walk::fixed(40.0, 10.0));
            let declaration = Walk {
                width: Size::Fit {
                    min: Some(unused),
                    max: Some(line),
                },
                height: Size::fit(),
                max_width: Some(unused),
                ..Default::default()
            };
            let before = cx.resolve_walk(declaration, ResolveAt::BeforeBegin);
            assert_eq!(before.width, declaration.width);
            assert_eq!(before.max_width, declaration.max_width);

            let full = cx.resolve_walk(
                Walk {
                    width: Size::Fit {
                        min: None,
                        max: Some(FitBound::Rel {
                            base: Base::Full,
                            factor: 0.5,
                        }),
                    },
                    height: Size::fit(),
                    ..Default::default()
                },
                ResolveAt::BeforeBegin,
            );
            assert_eq!(
                full.width,
                Size::Fit {
                    min: None,
                    max: Some(FitBound::Abs(100.0))
                }
            );

            for base in [Base::Line, Base::Unused] {
                let contextual_size = Size::Rel { base, factor: 0.5 };
                assert!(cx
                    .resolve_walk(
                        Walk::new(contextual_size, Size::Fixed(1.0)),
                        ResolveAt::BeforeBegin,
                    )
                    .width
                    .is_fit());
            }
        });

        for bound in [line, unused] {
            with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
                cx.walk_turtle(Walk::fixed(40.0, 10.0));
                cx.begin_turtle(
                    Walk {
                        width: Size::Fit {
                            min: None,
                            max: Some(bound),
                        },
                        height: Size::fit(),
                        ..Default::default()
                    },
                    Layout::default(),
                );
                assert_eq!(
                    cx.turtle().walk().width,
                    Size::Fit {
                        min: None,
                        max: Some(bound)
                    }
                );
                cx.walk_turtle(Walk::fixed(400.0, 10.0));
                assert_eq!(cx.end_turtle().size.x, 160.0);
            });
        }

        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            cx.begin_turtle(Walk::fit(), Layout::default());
            for base in [Base::Line, Base::Unused] {
                assert!(cx
                    .resolve_walk(
                        Walk::new(Size::Rel { base, factor: 1.0 }, Size::Fixed(1.0)),
                        ResolveAt::AtClose,
                    )
                    .width
                    .is_fit());
            }
            cx.end_turtle();
        });
    }

    #[test]
    fn viewport_and_container_context_follow_the_reviewed_fallbacks() {
        with_window_child_pass(|cx| {
            let viewport_walk = Walk::new(
                Size::Rel { base: Base::Vw, factor: 0.5 },
                Size::Rel { base: Base::Vh, factor: 0.25 },
            );
            let resolved = cx.resolve_walk(viewport_walk, ResolveAt::BeforeBegin);
            assert_eq!(resolved.width.to_fixed(), Some(500.0));
            assert_eq!(resolved.height.to_fixed(), Some(200.0));

            let outer_id = LiveId(11);
            let inner_id = LiveId(22);
            cx.begin_turtle(
                Walk::fixed(400.0, 300.0),
                Layout { container_id: outer_id, ..Default::default() },
            );
            cx.begin_turtle(
                Walk::fixed(200.0, 100.0),
                Layout { container_id: inner_id, ..Default::default() },
            );
            let nearest = cx.resolve_walk(
                Walk::new(Size::Rel { base: Base::Cqw, factor: 0.5 }, Size::Fixed(1.0)),
                ResolveAt::BeforeBegin,
            );
            assert_eq!(nearest.width.to_fixed(), Some(100.0));
            let named = cx.resolve_walk(
                Walk::new(Size::Rel { base: Base::Named(outer_id), factor: 0.5 }, Size::Fixed(1.0)),
                ResolveAt::BeforeBegin,
            );
            assert_eq!(named.width.to_fixed(), Some(200.0));
            let missing = cx.resolve_walk(
                Walk::new(Size::Rel { base: Base::Named(LiveId(99)), factor: 0.5 }, Size::Fixed(1.0)),
                ResolveAt::BeforeBegin,
            );
            assert_eq!(missing.width.to_fixed(), Some(500.0));
            cx.end_turtle();
            cx.end_turtle();

            cx.begin_turtle(
                Walk::new(Size::fit(), Size::Fixed(20.0)),
                Layout { container_id: inner_id, ..Default::default() },
            );
            let unknown = cx.resolve_walk(
                Walk::new(Size::Rel { base: Base::Cqw, factor: 0.5 }, Size::Fixed(1.0)),
                ResolveAt::BeforeBegin,
            );
            assert!(unknown.width.is_fit());
            cx.end_turtle();
        });

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let root_pass = DrawPass::new(&mut cx);
        root_pass.set_size(&mut cx, dvec2(500.0, 400.0));
        let child_pass = DrawPass::new(&mut cx);
        child_pass.set_pass_parent(&mut cx, &root_pass);
        child_pass.set_size(&mut cx, dvec2(64.0, 48.0));
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        draw.begin_pass(&child_pass, None);
        {
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_root_turtle(dvec2(64.0, 48.0), Layout::default());
            let resolved = cx.resolve_walk(
                Walk::new(
                    Size::Rel {
                        base: Base::Vw,
                        factor: 0.5,
                    },
                    Size::Rel {
                        base: Base::Vh,
                        factor: 0.25,
                    },
                ),
                ResolveAt::BeforeBegin,
            );
            assert_eq!(resolved.width.to_fixed(), Some(250.0));
            assert_eq!(resolved.height.to_fixed(), Some(100.0));
            cx.end_turtle();
        }
        draw.end_pass(&child_pass);
    }

    #[test]
    fn constraints_keep_size_margin_box_and_walk_content_box_semantics() {
        with_turtle(dvec2(200.0, 200.0), Layout::default(), |cx| {
            let margin = Inset { left: 10.0, right: 10.0, top: 5.0, bottom: 5.0 };
            let size_bound = cx.walk_turtle(Walk {
                abs_pos: Some(dvec2(0.0, 0.0)),
                margin,
                width: Size::Fill {
                    weight: 100.0,
                    basis: FitBound::Abs(0.0),
                    shrink: 0.0,
                    min: None,
                    max: Some(100.0),
                },
                height: Size::Fixed(10.0),
                ..Default::default()
            });
            assert_eq!(size_bound.size.x, 80.0);

            let walk_bound = cx.walk_turtle(Walk {
                abs_pos: Some(dvec2(0.0, 30.0)),
                margin,
                width: Size::Fill {
                    weight: 100.0,
                    basis: FitBound::Abs(0.0),
                    shrink: 0.0,
                    min: None,
                    max: None,
                },
                height: Size::Fixed(10.0),
                max_width: Some(FitBound::Abs(100.0)),
                ..Default::default()
            });
            assert_eq!(walk_bound.size.x, 100.0);

            let inverted = cx.peek_walk_turtle(Walk {
                width: Size::Fixed(70.0),
                height: Size::Fixed(10.0),
                min_width: Some(FitBound::Abs(120.0)),
                max_width: Some(FitBound::Abs(80.0)),
                ..Default::default()
            });
            assert_eq!(inverted.size.x, 120.0);

            let relative = cx.peek_walk_turtle(Walk {
                width: Size::Rel { base: Base::Parent, factor: 0.25 },
                height: Size::Fixed(10.0),
                min_width: Some(FitBound::Abs(60.0)),
                ..Default::default()
            });
            assert_eq!(relative.size.x, 60.0);

            let expression = match cx
                .global::<SizeExprStore>()
                .intern("50% + 10px")
                .unwrap()
            {
                SizeExprSimple::Compound(id) => id,
                other => panic!("expected compound expression, got {other:?}"),
            };
            let expression = cx.peek_walk_turtle(Walk {
                width: Size::Expr(expression),
                height: Size::Fixed(10.0),
                max_width: Some(FitBound::Abs(90.0)),
                ..Default::default()
            });
            assert_eq!(expression.size.x, 90.0);
        });

        with_turtle(dvec2(200.0, 200.0), Layout::default(), |cx| {
            cx.begin_turtle(
                Walk {
                    margin: Inset { left: 10.0, right: 10.0, ..Default::default() },
                    width: Size::Fit { min: None, max: Some(FitBound::Abs(80.0)) },
                    height: Size::fit(),
                    ..Default::default()
                },
                Layout::default(),
            );
            cx.walk_turtle(Walk::fixed(150.0, 10.0));
            let size_fit = cx.end_turtle();
            assert_eq!(size_fit.size.x, 60.0);

            cx.begin_turtle(
                Walk {
                    abs_pos: Some(dvec2(0.0, 40.0)),
                    margin: Inset { left: 10.0, right: 10.0, ..Default::default() },
                    width: Size::fit(),
                    height: Size::fit(),
                    max_width: Some(FitBound::Abs(80.0)),
                    ..Default::default()
                },
                Layout::default(),
            );
            cx.walk_turtle(Walk::fixed(150.0, 10.0));
            let walk_fit = cx.end_turtle();
            assert_eq!(walk_fit.size.x, 80.0);
        });
    }

    #[test]
    fn inverted_size_fill_bounds_normalize_for_immediate_and_deferred_axes() {
        let horizontal = Walk {
            margin: Inset {
                left: 8.0,
                right: 12.0,
                ..Default::default()
            },
            width: Size::Fill {
                weight: 1.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: Some(120.0),
                max: Some(80.0),
            },
            height: Size::Fixed(10.0),
            ..Default::default()
        };
        let mut immediate_width = 0.0;
        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            immediate_width = cx.walk_turtle(horizontal).size.x;
        });
        assert_eq!(immediate_width, 100.0);
        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            let mut deferred = cx.defer_walk_turtle(horizontal).unwrap();
            let materialized = deferred.resolve(cx);
            assert_eq!(materialized.width, Size::Fixed(immediate_width));
            assert_eq!(cx.walk_turtle(materialized).size.x, immediate_width);
        });

        let vertical = Walk {
            margin: Inset {
                top: 4.0,
                bottom: 6.0,
                ..Default::default()
            },
            width: Size::Fixed(10.0),
            height: Size::Fill {
                weight: 1.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: Some(90.0),
                max: Some(40.0),
            },
            ..Default::default()
        };
        let mut immediate_height = 0.0;
        with_turtle(dvec2(100.0, 150.0), Layout::flow_down(), |cx| {
            immediate_height = cx.walk_turtle(vertical).size.y;
        });
        assert_eq!(immediate_height, 80.0);
        with_turtle(dvec2(100.0, 150.0), Layout::flow_down(), |cx| {
            let mut deferred = cx.defer_walk_turtle(vertical).unwrap();
            let materialized = deferred.resolve(cx);
            assert_eq!(materialized.height, Size::Fixed(immediate_height));
            assert_eq!(cx.walk_turtle(materialized).size.y, immediate_height);
        });
    }

    #[test]
    fn deferred_fill_matches_immediate_margin_box_constraints() {
        let right_walk = Walk {
            margin: Inset {
                left: 10.0,
                right: 20.0,
                ..Default::default()
            },
            width: Size::Fill {
                weight: 100.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: Some(80.0),
                max: Some(120.0),
            },
            height: Size::Fixed(10.0),
            ..Default::default()
        };
        for (parent_width, expected_content) in [(200.0, 90.0), (60.0, 50.0)] {
            let mut immediate_size = Vec2d::default();
            with_turtle(
                dvec2(parent_width, 100.0),
                Layout::flow_right(),
                |cx| immediate_size = cx.walk_turtle(right_walk).size,
            );
            with_turtle(
                dvec2(parent_width, 100.0),
                Layout::flow_right(),
                |cx| {
                    let mut deferred = cx.defer_walk_turtle(right_walk).unwrap();
                    let materialized = deferred.resolve(cx);
                    let deferred_rect = cx.walk_turtle(materialized);
                    assert_eq!(immediate_size.x, expected_content);
                    assert_eq!(deferred_rect.size.x, immediate_size.x);
                },
            );
        }

        let down_walk = Walk {
            margin: Inset {
                top: 7.0,
                bottom: 13.0,
                ..Default::default()
            },
            width: Size::Fixed(10.0),
            height: Size::Fill {
                weight: 100.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: Some(70.0),
                max: Some(110.0),
            },
            ..Default::default()
        };
        for (parent_height, expected_content) in [(200.0, 90.0), (50.0, 50.0)] {
            let mut immediate_size = Vec2d::default();
            with_turtle(
                dvec2(100.0, parent_height),
                Layout::flow_down(),
                |cx| immediate_size = cx.walk_turtle(down_walk).size,
            );
            with_turtle(
                dvec2(100.0, parent_height),
                Layout::flow_down(),
                |cx| {
                    let mut deferred = cx.defer_walk_turtle(down_walk).unwrap();
                    let materialized = deferred.resolve(cx);
                    let deferred_rect = cx.walk_turtle(materialized);
                    assert_eq!(immediate_size.y, expected_content);
                    assert_eq!(deferred_rect.size.y, immediate_size.y);
                },
            );
        }
    }

    #[test]
    fn deferred_main_axis_fill_applies_aspect_after_materialization() {
        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            let walk = Walk {
                margin: Inset {
                    left: 10.0,
                    right: 10.0,
                    ..Default::default()
                },
                width: Size::Fill {
                    weight: 100.0,
                    basis: FitBound::Abs(0.0),
                    shrink: 0.0,
                    min: None,
                    max: Some(120.0),
                },
                height: Size::fit(),
                aspect: Some(2.0),
                ..Default::default()
            };
            let mut deferred = cx.defer_walk_turtle(walk).unwrap();
            let materialized = deferred.resolve(cx);
            assert_eq!(materialized.width.to_fixed(), Some(100.0));
            assert_eq!(materialized.height.to_fixed(), Some(50.0));
            assert_eq!(cx.walk_turtle(materialized).size, dvec2(100.0, 50.0));
        });
    }

    #[test]
    fn current_turtle_bounds_combine_legacy_and_walk_limits() {
        with_turtle(dvec2(300.0, 300.0), Layout::default(), |cx| {
            cx.begin_turtle(
                Walk {
                    margin: Inset {
                        left: 10.0,
                        right: 10.0,
                        top: 5.0,
                        bottom: 5.0,
                    },
                    width: Size::Fit {
                        min: None,
                        max: Some(FitBound::Abs(140.0)),
                    },
                    height: Size::Fit {
                        min: None,
                        max: Some(FitBound::Abs(110.0)),
                    },
                    max_width: Some(FitBound::Abs(90.0)),
                    max_height: Some(FitBound::Abs(80.0)),
                    ..Default::default()
                },
                Layout::default(),
            );
            assert_eq!(cx.current_turtle_max_width(), Some(90.0));
            assert_eq!(cx.current_turtle_max_height(), Some(80.0));
            cx.end_turtle();
        });
    }

    #[test]
    fn aspect_transfer_clamps_only_the_derived_axis_and_matches_peek_and_absolute() {
        with_turtle(dvec2(300.0, 200.0), Layout::flow_right(), |cx| {
            let fixed = Walk { width: Size::Fixed(120.0), height: Size::fit(), aspect: Some(2.0), ..Default::default() };
            assert_eq!(cx.peek_walk_turtle(fixed).size, dvec2(120.0, 60.0));

            let relative = Walk { width: Size::Rel { base: Base::Parent, factor: 0.5 }, height: Size::fit(), aspect: Some(2.0), ..Default::default() };
            assert_eq!(cx.peek_walk_turtle(relative).size, dvec2(150.0, 75.0));

            let cross_fill = Walk { width: Size::fit(), height: Size::fill(), aspect: Some(2.0), ..Default::default() };
            assert_eq!(cx.peek_walk_turtle(cross_fill).size, dvec2(400.0, 200.0));

            let clamped = Walk { width: Size::Fixed(120.0), height: Size::fit(), max_height: Some(FitBound::Abs(40.0)), aspect: Some(2.0), ..Default::default() };
            assert_eq!(cx.peek_walk_turtle(clamped).size, dvec2(120.0, 40.0));

            let both_definite = Walk { width: Size::Fixed(120.0), height: Size::Fixed(20.0), aspect: Some(2.0), ..Default::default() };
            assert_eq!(cx.peek_walk_turtle(both_definite).size, dvec2(120.0, 20.0));
            let both_fit = Walk { width: Size::fit(), height: Size::fit(), aspect: Some(2.0), ..Default::default() };
            let fit_size = cx.peek_walk_turtle(both_fit).size;
            assert!(fit_size.x.is_nan() && fit_size.y.is_nan());

            let absolute = fixed.with_abs_pos(dvec2(9.0, 13.0));
            let peek = cx.peek_walk_turtle(absolute);
            let walked = cx.walk_turtle(absolute);
            assert_eq!(peek, walked);

            let main_fill = cx.resolve_walk(
                Walk { width: Size::fill(), height: Size::fit(), aspect: Some(2.0), ..Default::default() },
                ResolveAt::BeforeBegin,
            );
            assert!(main_fill.width.is_fill() && main_fill.height.is_fit());
        });
    }

    #[test]
    fn ancestor_max_and_adjacent_bug_fixes_are_covered() {
        let layout = Layout {
            padding: Inset { left: 3.0, right: 11.0, top: 7.0, bottom: 13.0 },
            ..Default::default()
        };
        with_turtle(dvec2(200.0, 100.0), layout, |cx| {
            assert_eq!(cx.turtle().rel_pos_padded(), dvec2(0.0, 0.0));
            let height = cx.turtle().max_height(Walk::new(Size::Fixed(150.0), Size::Fixed(25.0)));
            assert_eq!(height, Some(25.0));
            assert_eq!(
                cx.turtle().max_width(Walk::new(
                    Size::Rel {
                        base: Base::Parent,
                        factor: 0.5,
                    },
                    Size::Fixed(1.0),
                )),
                None,
            );

            cx.begin_turtle(
                Walk { width: Size::fit(), height: Size::fit(), max_height: Some(FitBound::Abs(70.0)), ..Default::default() },
                Layout::default(),
            );
            cx.begin_turtle(Walk::fit(), Layout::default());
            assert_eq!(cx.compute_max_height_from_ancestors(), 70.0);
            cx.end_turtle();
            cx.end_turtle();

            cx.begin_turtle(
                Walk {
                    abs_pos: Some(dvec2(0.0, 0.0)),
                    margin: Inset { top: 8.0, bottom: 8.0, ..Default::default() },
                    width: Size::fit(),
                    height: Size::Fit { min: None, max: Some(FitBound::Abs(10.0)) },
                    ..Default::default()
                },
                Layout::default(),
            );
            cx.walk_turtle(Walk::fixed(10.0, 20.0));
            assert_eq!(cx.end_turtle().size.y, 0.0);
        });
    }

    #[test]
    fn script_strings_accept_inline_and_heap_values_and_round_trip_compounds() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let mut size = Size::fill();
            let inline = ScriptValue::from_inline_string("50%").unwrap();
            size.script_apply(vm, &Apply::New, &mut Scope::empty(), inline);
            assert!(matches!(size, Size::Rel { base: Base::Parent, factor } if factor == 0.5));

            let heap = vm.bx.heap.new_string_from_str("calc(10px + 25vw)");
            size.script_apply(vm, &Apply::New, &mut Scope::empty(), heap);
            let Size::Expr(id) = size else { panic!("heap string did not compile to Expr") };
            assert_eq!(vm.cx().get_global_ref::<SizeExprStore>().unwrap().source(id), Some("calc(10px + 25vw)"));
            let emitted = size.script_to_value(vm);
            let round_trip = vm.bx.heap.string_with(emitted, |_, source| source.to_string());
            assert_eq!(round_trip.as_deref(), Some("calc(10px + 25vw)"));

            let old = size;
            let invalid = vm.bx.heap.new_string_from_str("10px + 2");
            size.script_apply(vm, &Apply::New, &mut Scope::empty(), invalid);
            assert!(matches!((old, size), (Size::Expr(a), Size::Expr(b)) if a == b));

            let mut bound = FitBound::Abs(1.0);
            let bound_value = vm.bx.heap.new_string_from_str("max(10px, 5vw)");
            bound.script_apply(vm, &Apply::New, &mut Scope::empty(), bound_value);
            assert!(matches!(bound, FitBound::Expr(_)));

            let mut direct = SizeExprId::default();
            assert!(!SizeExprId::on_type_check(
                &vm.bx.heap,
                ScriptValue::from_f64(0.0)
            ));
            direct.script_apply(
                vm,
                &Apply::New,
                &mut Scope::empty(),
                ScriptValue::from_f64(0.0),
            );
            assert_eq!(direct, SizeExprId::INVALID);

            let simple = ScriptValue::from_inline_string("50%").unwrap();
            direct.script_apply(vm, &Apply::New, &mut Scope::empty(), simple);
            assert_ne!(direct, SizeExprId::INVALID);
            assert_eq!(
                vm.cx()
                    .get_global_ref::<SizeExprStore>()
                    .unwrap()
                    .source(direct),
                Some("50%")
            );
            let emitted = direct.script_to_value(vm);
            let emitted = vm
                .bx
                .heap
                .string_with(emitted, |_, source| source.to_string());
            assert_eq!(emitted.as_deref(), Some("50%"));

            let pixels = ScriptValue::from_inline_string("10px").unwrap();
            direct.script_apply(vm, &Apply::New, &mut Scope::empty(), pixels);
            assert_eq!(
                vm.cx()
                    .get_global_ref::<SizeExprStore>()
                    .unwrap()
                    .source(direct),
                Some("10px")
            );

            let compound = vm.bx.heap.new_string_from_str("10px + 25vw");
            direct.script_apply(vm, &Apply::New, &mut Scope::empty(), compound);
            assert_eq!(
                vm.cx()
                    .get_global_ref::<SizeExprStore>()
                    .unwrap()
                    .source(direct),
                Some("10px + 25vw")
            );
            let old = direct;
            let invalid = vm.bx.heap.new_string_from_str("10px + 2");
            direct.script_apply(vm, &Apply::New, &mut Scope::empty(), invalid);
            assert_eq!(direct, old);
        });
    }

    fn flex(grow: f64, basis: f64, shrink: f64) -> Size {
        Size::Fill {
            weight: grow,
            basis: FitBound::Abs(basis),
            shrink,
            min: None,
            max: None,
        }
    }

    fn resolve_flex_widths(parent_width: f64, sizes: &[Size]) -> Vec<f64> {
        let mut result = Vec::new();
        with_turtle(
            dvec2(parent_width, 100.0),
            Layout::flow_right(),
            |cx| {
                let mut deferred: Vec<_> = sizes
                    .iter()
                    .map(|size| {
                        cx.defer_walk_turtle(Walk::new(*size, Size::Fixed(10.0)))
                            .unwrap()
                    })
                    .collect();
                for walk in &mut deferred {
                    result.push(walk.resolve(cx).width.to_fixed().unwrap());
                }
            },
        );
        result
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn definite_basis_flex_grows_and_shrinks_with_finite_positive_factors() {
        let grown = resolve_flex_widths(300.0, &[flex(1.0, 50.0, 0.0), flex(2.0, 50.0, 0.0)]);
        assert_close(grown[0], 350.0 / 3.0);
        assert_close(grown[1], 550.0 / 3.0);

        let three = resolve_flex_widths(
            300.0,
            &[
                flex(1.0, 30.0, 0.0),
                flex(2.0, 30.0, 0.0),
                flex(0.0, 30.0, 0.0),
            ],
        );
        assert_eq!(three, vec![100.0, 170.0, 30.0]);

        let shrunk = resolve_flex_widths(
            120.0,
            &[flex(0.0, 100.0, 1.0), flex(0.0, 100.0, 3.0)],
        );
        assert_eq!(shrunk, vec![80.0, 40.0]);

        let three_shrunk = resolve_flex_widths(
            180.0,
            &[
                flex(0.0, 100.0, 1.0),
                flex(0.0, 100.0, 2.0),
                flex(0.0, 100.0, 0.0),
            ],
        );
        assert_eq!(three_shrunk, vec![60.0, 20.0, 100.0]);

        let ignored = resolve_flex_widths(
            120.0,
            &[
                flex(f64::NAN, 100.0, f64::INFINITY),
                flex(-1.0, 100.0, 1.0),
            ],
        );
        assert_eq!(ignored, vec![100.0, 20.0]);

        let no_factors = resolve_flex_widths(
            120.0,
            &[flex(0.0, 100.0, 0.0), flex(f64::NAN, 100.0, -1.0)],
        );
        assert_eq!(no_factors, vec![100.0, 100.0]);

        let large_finite = resolve_flex_widths(
            200.0,
            &[flex(f64::MAX, 0.0, 0.0), flex(f64::MAX, 0.0, 0.0)],
        );
        assert_eq!(large_finite, vec![100.0, 100.0]);

        with_turtle(dvec2(100.0, 120.0), Layout::flow_down(), |cx| {
            let mut a = cx
                .defer_walk_turtle(Walk::new(
                    Size::Fixed(10.0),
                    flex(0.0, 100.0, 1.0),
                ))
                .unwrap();
            let mut b = cx
                .defer_walk_turtle(Walk::new(
                    Size::Fixed(10.0),
                    flex(0.0, 100.0, 3.0),
                ))
                .unwrap();
            assert_eq!(a.resolve(cx).height, Size::Fixed(80.0));
            assert_eq!(b.resolve(cx).height, Size::Fixed(40.0));
        });

        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            let mut contextual = cx
                .defer_walk_turtle(Walk::new(
                    Size::Fill {
                        weight: 0.0,
                        basis: FitBound::Rel {
                            base: Base::Parent,
                            factor: 0.25,
                        },
                        shrink: 0.0,
                        min: None,
                        max: None,
                    },
                    Size::Fixed(10.0),
                ))
                .unwrap();
            assert_eq!(contextual.resolve(cx).width, Size::Fixed(50.0));
        });
    }

    #[test]
    fn flex_accounts_for_fixed_gaps_margins_and_iterative_bounds() {
        let layout = Layout {
            spacing: 10.0,
            padding: Inset {
                left: 10.0,
                right: 10.0,
                ..Default::default()
            },
            ..Layout::flow_right()
        };
        with_turtle(dvec2(500.0, 100.0), layout, |cx| {
            cx.walk_turtle(Walk::fixed(50.0, 10.0));
            let margin = Inset {
                left: 10.0,
                right: 10.0,
                ..Default::default()
            };
            let mut a = cx
                .defer_walk_turtle(Walk {
                    margin,
                    width: Size::Fill {
                        weight: 1.0,
                        basis: FitBound::Abs(100.0),
                        shrink: 1.0,
                        min: None,
                        max: Some(150.0),
                    },
                    height: Size::Fixed(10.0),
                    ..Default::default()
                })
                .unwrap();
            let mut b = cx
                .defer_walk_turtle(Walk::new(flex(1.0, 100.0, 1.0), Size::Fixed(10.0)))
                .unwrap();
            assert_eq!(a.resolve(cx).width.to_fixed(), Some(130.0));
            assert_eq!(b.resolve(cx).width.to_fixed(), Some(260.0));
        });

        let bounded = [
            Size::Fill {
                weight: 1.0,
                basis: FitBound::Abs(100.0),
                shrink: 1.0,
                min: None,
                max: Some(120.0),
            },
            Size::Fill {
                weight: 1.0,
                basis: FitBound::Abs(100.0),
                shrink: 1.0,
                min: None,
                max: Some(200.0),
            },
            flex(1.0, 100.0, 1.0),
        ];
        assert_eq!(resolve_flex_widths(540.0, &bounded), vec![120.0, 200.0, 220.0]);

        let floors = [
            Size::Fill {
                weight: 0.0,
                basis: FitBound::Abs(100.0),
                shrink: 1.0,
                min: Some(90.0),
                max: None,
            },
            Size::Fill {
                weight: 0.0,
                basis: FitBound::Abs(100.0),
                shrink: 1.0,
                min: Some(20.0),
                max: None,
            },
        ];
        assert_eq!(resolve_flex_widths(150.0, &floors), vec![90.0, 60.0]);

        let permuted = [bounded[2], bounded[0], bounded[1]];
        let mut original = resolve_flex_widths(540.0, &bounded);
        let mut reordered = resolve_flex_widths(540.0, &permuted);
        original.sort_by(f64::total_cmp);
        reordered.sort_by(f64::total_cmp);
        assert_eq!(original, reordered);
    }

    #[test]
    fn indefinite_flex_materializes_clamped_basis_without_nan() {
        with_turtle(dvec2(f64::NAN, 100.0), Layout::flow_right(), |cx| {
            let mut deferred = cx
                .defer_walk_turtle(Walk::new(
                    Size::Fill {
                        weight: 1.0,
                        basis: FitBound::Abs(40.0),
                        shrink: 1.0,
                        min: Some(60.0),
                        max: None,
                    },
                    Size::Fixed(10.0),
                ))
                .unwrap();
            let walk = deferred.resolve(cx);
            assert_eq!(walk.width, Size::Fixed(60.0));
            assert!(!matches!(walk.width, Size::Fixed(value) if value.is_nan()));
            assert_eq!(cx.walk_turtle(walk).size.x, 60.0);
        });
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn late_defer_is_rejected_after_one_shot_resolution() {
        with_turtle(dvec2(100.0, 100.0), Layout::flow_right(), |cx| {
            let mut first = cx
                .defer_walk_turtle(Walk::new(flex(1.0, 0.0, 0.0), Size::Fixed(10.0)))
                .unwrap();
            first.resolve(cx);
            assert!(cx
                .defer_walk_turtle(Walk::new(flex(1.0, 0.0, 0.0), Size::Fixed(10.0)))
                .is_none());
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "cannot defer another fill after flex resolution")]
    fn late_defer_debug_asserts_after_one_shot_resolution() {
        with_turtle(dvec2(100.0, 100.0), Layout::flow_right(), |cx| {
            let mut first = cx
                .defer_walk_turtle(Walk::new(flex(1.0, 0.0, 0.0), Size::Fixed(10.0)))
                .unwrap();
            first.resolve(cx);
            let _ = cx.defer_walk_turtle(Walk::new(
                flex(1.0, 0.0, 0.0),
                Size::Fixed(10.0),
            ));
        });
    }

    #[test]
    fn deferred_walk_preserves_metadata_aspect_and_child_round_trip_provenance() {
        with_turtle(dvec2(200.0, 100.0), Layout::flow_right(), |cx| {
            let metrics = Metrics {
                descender: 3.0,
                line_gap: 4.0,
                line_scale: 1.5,
            };
            let original = Walk {
                margin: Inset {
                    left: 5.0,
                    right: 7.0,
                    ..Default::default()
                },
                width: flex(0.0, 80.0, 1.0),
                height: Size::fit(),
                min_height: Some(FitBound::Abs(20.0)),
                max_height: Some(FitBound::Abs(50.0)),
                aspect: Some(2.0),
                metrics,
                ..Default::default()
            };
            let mut deferred = cx.defer_walk_turtle(original).unwrap();
            let materialized = deferred.resolve(cx);
            assert!(materialized.deferred);
            assert_eq!(materialized.margin.left, original.margin.left);
            assert_eq!(materialized.margin.right, original.margin.right);
            assert_eq!(materialized.margin.top, original.margin.top);
            assert_eq!(materialized.margin.bottom, original.margin.bottom);
            assert_eq!(materialized.min_height, original.min_height);
            assert_eq!(materialized.max_height, original.max_height);
            assert_eq!(materialized.aspect, original.aspect);
            assert_eq!(materialized.width, Size::Fixed(80.0));
            assert_eq!(materialized.height, Size::Fixed(40.0));

            cx.begin_turtle(materialized, Layout::default());
            cx.walk_turtle(Walk::fixed(1.0, 1.0));
            cx.end_turtle();
            let finished = cx.finished_walks.last().unwrap();
            assert!(finished.in_flow);
            assert_eq!(finished.metrics.descender, metrics.descender);
            assert_eq!(finished.metrics.line_gap, metrics.line_gap);
            assert_eq!(finished.metrics.line_scale, metrics.line_scale);

            cx.walk_turtle(Walk::fixed(5.0, 5.0).with_abs_pos(dvec2(2.0, 3.0)));
            assert!(!cx.finished_walks.last().unwrap().in_flow);
        });
    }

    #[test]
    fn wrapped_fill_uses_current_remainder_or_a_full_fresh_row() {
        let layout = Layout {
            spacing: 10.0,
            ..Layout::flow_right_wrap()
        };
        with_turtle(dvec2(100.0, 100.0), layout, |cx| {
            cx.walk_turtle(Walk::fixed(30.0, 10.0));
            let fill_walk = Walk::new(flex(1.0, 20.0, 1.0), Size::Fixed(10.0));
            assert!(cx.defer_walk_turtle(fill_walk).is_none());
            let fill = cx.walk_turtle(fill_walk);
            assert_eq!(fill.pos, dvec2(40.0, 0.0));
            assert_eq!(fill.size.x, 60.0);
        });

        with_turtle(dvec2(100.0, 100.0), layout, |cx| {
            cx.walk_turtle(Walk::fixed(30.0, 10.0));
            let fresh = cx.walk_turtle(Walk {
                width: Size::Fill {
                    weight: 1.0,
                    basis: FitBound::Abs(20.0),
                    shrink: 1.0,
                    min: Some(70.0),
                    max: None,
                },
                height: Size::Fixed(10.0),
                ..Default::default()
            });
            assert_eq!(fresh.pos, dvec2(0.0, 10.0));
            assert_eq!(fresh.size.x, 100.0);
            assert!(cx.turtle().deferred_fills.is_empty());
        });
    }

    #[test]
    fn wrapped_inverted_fill_bounds_use_the_current_or_one_fresh_row() {
        let layout = Layout {
            spacing: 10.0,
            wrap_spacing: 5.0,
            ..Layout::flow_right_wrap()
        };
        let margin = Inset {
            left: 4.0,
            right: 6.0,
            ..Default::default()
        };

        with_turtle(dvec2(100.0, 100.0), layout, |cx| {
            cx.walk_turtle(Walk::fixed(20.0, 10.0));
            let current = cx.walk_turtle(Walk {
                margin,
                width: Size::Fill {
                    weight: 1.0,
                    basis: FitBound::Abs(0.0),
                    shrink: 0.0,
                    min: Some(70.0),
                    max: Some(40.0),
                },
                height: Size::Fixed(10.0),
                ..Default::default()
            });
            assert_eq!(current.pos, dvec2(34.0, 0.0));
            assert_eq!(current.size.x, 60.0);
        });

        with_turtle(dvec2(100.0, 100.0), layout, |cx| {
            cx.walk_turtle(Walk::fixed(30.0, 10.0));
            let fresh = cx.walk_turtle(Walk {
                margin,
                width: Size::Fill {
                    weight: 1.0,
                    basis: FitBound::Abs(0.0),
                    shrink: 0.0,
                    min: Some(120.0),
                    max: Some(80.0),
                },
                height: Size::Fixed(10.0),
                ..Default::default()
            });
            assert_eq!(fresh.pos, dvec2(4.0, 15.0));
            assert_eq!(fresh.size.x, 110.0);
        });
    }

    fn tracked_walk(cx: &mut Cx2d, walk: Walk) -> usize {
        let rect = cx.walk_turtle(walk);
        let marker = cx.align_list.len();
        cx.align_list
            .push(AlignEntry::BeginClip(rect.pos, rect.pos + rect.size));
        marker
    }

    fn marker_pos(cx: &Cx2d, marker: usize) -> Vec2d {
        match cx.align_list[marker] {
            AlignEntry::BeginClip(pos, _) => pos,
            ref other => panic!("unexpected marker {other:?}"),
        }
    }

    fn nowrap_distribution_positions(mode: Distribute) -> (Vec2d, Vec2d) {
        let mut positions = (Vec2d::default(), Vec2d::default());
        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: mode,
                ..Layout::flow_right()
            },
            |cx| {
                let a = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let b = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                cx.end_turtle();
                positions = (marker_pos(cx, a), marker_pos(cx, b));
            },
        );
        positions
    }

    #[test]
    fn nowrap_right_distribution_modes_and_small_groups_are_defined() {
        assert_eq!(
            nowrap_distribution_positions(Distribute::Start),
            (dvec2(0.0, 0.0), dvec2(10.0, 0.0))
        );
        assert_eq!(
            nowrap_distribution_positions(Distribute::SpaceBetween),
            (dvec2(0.0, 0.0), dvec2(90.0, 0.0))
        );
        let around = nowrap_distribution_positions(Distribute::SpaceAround);
        assert_close(around.0.x, 20.0);
        assert_close(around.1.x, 70.0);
        let evenly = nowrap_distribution_positions(Distribute::SpaceEvenly);
        assert_close(evenly.0.x, 80.0 / 3.0);
        assert_close(evenly.1.x, 190.0 / 3.0);

        for mode in [
            Distribute::Start,
            Distribute::SpaceBetween,
            Distribute::SpaceAround,
            Distribute::SpaceEvenly,
        ] {
            with_turtle(
                dvec2(100.0, 100.0),
                Layout {
                    distribute: mode,
                    ..Layout::flow_right()
                },
                |cx| {
                    if mode != Distribute::Start {
                        let marker = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                        cx.end_turtle();
                        let expected = if mode == Distribute::SpaceBetween {
                            0.0
                        } else {
                            45.0
                        };
                        assert_close(marker_pos(cx, marker).x, expected);
                    }
                },
            );
        }
        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: Distribute::SpaceEvenly,
                ..Layout::flow_right()
            },
            |cx| {
                cx.end_turtle();
            },
        );
    }

    #[test]
    fn wrapped_rows_and_down_flow_distribute_independently() {
        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: Distribute::SpaceEvenly,
                ..Layout::flow_right_wrap()
            },
            |cx| {
                let a = tracked_walk(cx, Walk::fixed(40.0, 10.0));
                let b = tracked_walk(cx, Walk::fixed(40.0, 10.0));
                let c = tracked_walk(cx, Walk::fixed(40.0, 10.0));
                cx.end_turtle();
                assert_close(marker_pos(cx, a).x, 20.0 / 3.0);
                assert_close(marker_pos(cx, b).x, 160.0 / 3.0);
                assert_eq!(marker_pos(cx, c), dvec2(30.0, 10.0));
            },
        );

        for mode in [
            Distribute::Start,
            Distribute::SpaceBetween,
            Distribute::SpaceAround,
            Distribute::SpaceEvenly,
        ] {
            with_turtle(
                dvec2(100.0, 100.0),
                Layout {
                    distribute: mode,
                    ..Layout::flow_down()
                },
                |cx| {
                    let a = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                    let b = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                    cx.end_turtle();
                    let (expected_a, expected_b) = match mode {
                        Distribute::Start => (0.0, 10.0),
                        Distribute::SpaceBetween => (0.0, 90.0),
                        Distribute::SpaceAround => (20.0, 70.0),
                        Distribute::SpaceEvenly => (80.0 / 3.0, 190.0 / 3.0),
                    };
                    assert_close(marker_pos(cx, a).y, expected_a);
                    assert_close(marker_pos(cx, b).y, expected_b);
                },
            );
        }
    }

    #[test]
    fn deferred_prefix_delta_moves_anchor_and_fixed_followers() {
        with_turtle(dvec2(100.0, 40.0), Layout::flow_right(), |cx| {
            let mut deferred = cx
                .defer_walk_turtle(Walk::new(
                    Size::Fill {
                        weight: 1.0,
                        basis: FitBound::Abs(20.0),
                        shrink: 0.0,
                        min: None,
                        max: Some(40.0),
                    },
                    Size::Fixed(10.0),
                ))
                .unwrap();
            let following = tracked_walk(cx, Walk::fixed(10.0, 10.0));
            cx.finished_walks.last_mut().unwrap().align_role = RowAlignRole::Anchor;
            let materialized = deferred.resolve(cx);
            let fill = tracked_walk(cx, materialized);
            cx.end_turtle();
            assert_eq!(marker_pos(cx, fill), dvec2(0.0, 0.0));
            assert_eq!(marker_pos(cx, following), dvec2(40.0, 0.0));
        });

        with_turtle(dvec2(40.0, 100.0), Layout::flow_down(), |cx| {
            let mut deferred = cx
                .defer_walk_turtle(Walk::new(
                    Size::Fixed(10.0),
                    Size::Fill {
                        weight: 1.0,
                        basis: FitBound::Abs(20.0),
                        shrink: 0.0,
                        min: None,
                        max: Some(40.0),
                    },
                ))
                .unwrap();
            let following = tracked_walk(cx, Walk::fixed(10.0, 10.0));
            cx.finished_walks.last_mut().unwrap().align_role = RowAlignRole::Fixed;
            let materialized = deferred.resolve(cx);
            let fill = tracked_walk(cx, materialized);
            cx.end_turtle();
            assert_eq!(marker_pos(cx, fill), dvec2(0.0, 0.0));
            assert_eq!(marker_pos(cx, following), dvec2(0.0, 40.0));
        });
    }

    #[test]
    fn mixed_roles_keep_start_main_and_cross_axis_alignment() {
        with_turtle(
            dvec2(100.0, 40.0),
            Layout {
                align: Align { x: 0.5, y: 1.0 },
                ..Layout::flow_right()
            },
            |cx| {
                let anchor = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let normal = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                cx.finished_walks[0].align_role = RowAlignRole::Anchor;
                cx.end_turtle();
                assert_eq!(marker_pos(cx, anchor), dvec2(40.0, 30.0));
                assert_eq!(marker_pos(cx, normal), dvec2(50.0, 30.0));
            },
        );

        with_turtle(
            dvec2(40.0, 100.0),
            Layout {
                align: Align { x: 1.0, y: 0.5 },
                ..Layout::flow_down()
            },
            |cx| {
                let fixed = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let normal = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                cx.finished_walks[0].align_role = RowAlignRole::Fixed;
                cx.end_turtle();
                assert_eq!(marker_pos(cx, fixed), dvec2(30.0, 40.0));
                assert_eq!(marker_pos(cx, normal), dvec2(30.0, 50.0));
            },
        );
    }

    #[test]
    fn space_distribution_falls_back_for_the_whole_mixed_role_group() {
        for flow in [Flow::right(), Flow::right_wrap()] {
            let expected_y = if matches!(flow, Flow::Right { wrap: true, .. }) {
                0.0
            } else {
                30.0
            };
            with_turtle(
                dvec2(100.0, 40.0),
                Layout {
                    flow,
                    align: Align { x: 0.5, y: 1.0 },
                    distribute: Distribute::SpaceBetween,
                    ..Default::default()
                },
                |cx| {
                    let anchor = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                    let normal = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                    cx.finished_walks[0].align_role = RowAlignRole::Anchor;
                    cx.end_turtle();
                    assert_eq!(marker_pos(cx, anchor), dvec2(40.0, expected_y));
                    assert_eq!(marker_pos(cx, normal), dvec2(50.0, expected_y));
                },
            );
        }
    }

    #[test]
    fn overlay_ignores_space_distribution() {
        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                align: Align { x: 0.5, y: 0.5 },
                distribute: Distribute::SpaceEvenly,
                ..Layout::flow_overlay()
            },
            |cx| {
                let small = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let large = tracked_walk(cx, Walk::fixed(20.0, 20.0));
                cx.end_turtle();
                assert_eq!(marker_pos(cx, small), dvec2(45.0, 45.0));
                assert_eq!(marker_pos(cx, large), dvec2(40.0, 40.0));
            },
        );
    }

    #[test]
    fn distribution_handles_constrained_fill_absolute_and_immovable_walks() {
        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: Distribute::SpaceEvenly,
                ..Layout::flow_right()
            },
            |cx| {
                let mut deferred = cx
                    .defer_walk_turtle(Walk::new(
                        Size::Fill {
                            weight: 1.0,
                            basis: FitBound::Abs(0.0),
                            shrink: 0.0,
                            min: None,
                            max: Some(40.0),
                        },
                        Size::Fixed(10.0),
                    ))
                    .unwrap();
                let walk = deferred.resolve(cx);
                assert_eq!(walk.width, Size::Fixed(40.0));
                let fill = tracked_walk(cx, walk);
                cx.end_turtle();
                assert_eq!(marker_pos(cx, fill), dvec2(30.0, 0.0));
            },
        );

        // A following normal walk receives the deferred fill's signed delta
        // and its own distribution offset in one close-time range move.
        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: Distribute::SpaceEvenly,
                ..Layout::flow_right()
            },
            |cx| {
                let mut deferred = cx
                    .defer_walk_turtle(Walk::new(
                        Size::Fill {
                            weight: 1.0,
                            basis: FitBound::Abs(20.0),
                            shrink: 0.0,
                            min: None,
                            max: Some(40.0),
                        },
                        Size::Fixed(10.0),
                    ))
                    .unwrap();
                let following = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let materialized = deferred.resolve(cx);
                let fill = tracked_walk(cx, materialized);
                cx.end_turtle();
                assert_close(marker_pos(cx, fill).x, 50.0 / 3.0);
                assert_close(marker_pos(cx, following).x, 220.0 / 3.0);
            },
        );

        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: Distribute::SpaceBetween,
                ..Layout::flow_right()
            },
            |cx| {
                let absolute = tracked_walk(
                    cx,
                    Walk::fixed(80.0, 10.0).with_abs_pos(dvec2(5.0, 20.0)),
                );
                let a = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let b = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                cx.end_turtle();
                assert_eq!(marker_pos(cx, absolute), dvec2(5.0, 20.0));
                assert_eq!(marker_pos(cx, a), dvec2(0.0, 0.0));
                assert_eq!(marker_pos(cx, b), dvec2(90.0, 0.0));
            },
        );

        with_turtle(
            dvec2(100.0, 100.0),
            Layout {
                distribute: Distribute::SpaceBetween,
                ..Layout::flow_right()
            },
            |cx| {
                let a = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                let b = tracked_walk(cx, Walk::fixed(10.0, 10.0));
                cx.finished_walks[0].align_role = RowAlignRole::Anchor;
                cx.end_turtle();
                assert_eq!(marker_pos(cx, a), dvec2(0.0, 0.0));
                assert_eq!(marker_pos(cx, b), dvec2(10.0, 0.0));
            },
        );
    }

    #[test]
    fn new_defaults_preserve_legacy_fill_behavior() {
        assert_eq!(
            Size::fill(),
            Size::Fill {
                weight: 100.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: None,
                max: None,
            }
        );
        assert_eq!(Layout::default().distribute, Distribute::Start);
        assert!(!Walk::default().deferred);
    }

    #[test]
    fn walk_and_layout_size_gates() {
        assert!(std::mem::size_of::<Walk>() <= 384);
        // Layout was 96 bytes before the one-word container id. Keep the
        // reviewed foundation within the measured 112-byte baseline.
        assert!(std::mem::size_of::<Layout>() <= 112);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
pub enum Base {
    #[pick]
    Full,
    Parent,
    Vw,
    Vh,
    Cqw,
    Cqh,
    #[live(LiveId(0))]
    Named(LiveId),
    Unused,
    /// The width available on the enclosing line for inline content,
    /// accounting for the enclosing widget's own leading geometry and
    /// trailing insets; see [`Cx2d::find_line_available_width`]. Widths
    /// only: as a height base this resolves to nothing.
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveAt {
    /// The walk has not been pushed; the top turtle is its immediate parent.
    BeforeBegin,
    /// The walk owns the top turtle; its immediate parent is one slot below.
    AtClose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    Width,
    Height,
}

#[derive(Clone, Copy, Debug, Default)]
struct ContentBounds {
    min: Option<f64>,
    max: Option<f64>,
}

impl ContentBounds {
    fn from_fill(min: Option<f64>, max: Option<f64>, margin: f64) -> Self {
        let mut bounds = Self::default();
        bounds.add_min(min.map(|value| (value - margin).max(0.0)));
        bounds.add_max(max.map(|value| (value - margin).max(0.0)));
        bounds.normalize();
        bounds
    }

    fn add_min(&mut self, value: Option<f64>) {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            self.min = Some(self.min.map_or(value, |old| old.max(value)));
        }
    }

    fn add_max(&mut self, value: Option<f64>) {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            self.max = Some(self.max.map_or(value, |old| old.min(value)));
        }
    }

    fn normalize(&mut self) {
        if let (Some(min), Some(max)) = (self.min, self.max) {
            if min > max {
                self.max = Some(min);
            }
        }
    }

    fn clamp(self, value: f64) -> f64 {
        let mut value = value;
        if let Some(max) = self.max {
            value = value.min(max);
        }
        if let Some(min) = self.min {
            value = value.max(min);
        }
        value.max(0.0)
    }

    fn clamp_available(self, value: f64) -> f64 {
        if value.is_nan() && self.min.is_none() && self.max.is_none() {
            value
        } else {
            self.clamp(value)
        }
    }
}

/// Specifies how walks should be laid out with respect to each other.
#[derive(Copy, Clone, Debug, Script, ScriptHook)]
pub struct Layout {
    #[live]
    pub scroll: Vec2d,
    #[live(true)]
    pub clip_x: bool,
    #[live(true)]
    pub clip_y: bool,

    /// The direction in which each walk is laid out.
    #[live]
    pub flow: Flow,

    /// The spacing between each walk.
    #[live]
    pub spacing: f64,

    /// The vertical spacing between rows when wrapping in a `Flow::Right { wrap: true }` layout.
    #[live]
    pub wrap_spacing: f64,

    /// The padding around the inner rectangle of each walk.
    #[live]
    pub padding: Inset,

    /// The alignment of each walk with respect to their turtle's rectangle.
    #[live]
    pub align: Align,

    /// Distribution of positive slack along the main axis.
    #[live]
    pub distribute: Distribute,

    /// Nonzero ids make this turtle a query container for cqw/cqh and
    /// `Base::Named` resolution.
    #[live]
    pub container_id: LiveId,
}

impl Layout {
    /// Creates a `Layout` in which walks are laid out from left to right, and all other fields
    /// are set to their default values.
    pub fn flow_right() -> Self {
        Self {
            flow: Flow::right(),
            ..Self::default()
        }
    }

    /// Creates a `Layout` in which walks are laid out from left to right, wrapping to the next row
    /// if we run out of space, and all other fields are set to their default values.
    pub fn flow_right_wrap() -> Self {
        Self {
            flow: Flow::right_wrap(),
            ..Self::default()
        }
    }

    /// Creates a `Layout` in which walks are laid out from top to bottom, and all other fields
    /// are set to their default values.
    pub fn flow_down() -> Self {
        Self {
            flow: Flow::Down,
            ..Self::default()
        }
    }

    /// Creates a `Layout` in which walks are laid out on top of each other, and all other fields
    /// are set to their default values.
    pub fn flow_overlay() -> Self {
        Self {
            flow: Flow::Overlay,
            ..Self::default()
        }
    }

    /// Creates a copy of this `Layout` with `padding` set to the given value.
    pub fn with_padding(self, padding: Inset) -> Self {
        Self { padding, ..self }
    }

    /// Creates a copy of this `Layout` with the top padding set to the given value.
    pub fn with_padding_top(self, top: f64) -> Self {
        Self {
            padding: self.padding.with_top(top),
            ..self
        }
    }

    /// Creates a copy of this `Layout` with the right padding set to the given value.
    pub fn with_padding_right(self, right: f64) -> Self {
        Self {
            padding: self.padding.with_right(right),
            ..self
        }
    }

    /// Creates a copy of this `Layout` with the bottom padding set to the given value.
    pub fn with_padding_bottom(self, bottom: f64) -> Self {
        Self {
            padding: self.padding.with_bottom(bottom),
            ..self
        }
    }

    /// Creates a copy of this `Layout` with the left padding set to the given value.
    pub fn with_padding_left(self, left: f64) -> Self {
        Self {
            padding: self.padding.with_left(left),
            ..self
        }
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            scroll: dvec2(0.0, 0.0),
            clip_x: true,
            clip_y: true,
            padding: Inset::default(),
            align: Align::default(),
            distribute: Distribute::default(),
            flow: Flow::default(),
            spacing: 0.0,
            wrap_spacing: 0.0,
            container_id: LiveId(0),
        }
    }
}

impl From<SizeExprUnit> for Base {
    fn from(unit: SizeExprUnit) -> Self {
        match unit {
            SizeExprUnit::Parent => Base::Parent,
            SizeExprUnit::Vw => Base::Vw,
            SizeExprUnit::Vh => Base::Vh,
            SizeExprUnit::Cqw => Base::Cqw,
            SizeExprUnit::Cqh => Base::Cqh,
        }
    }
}

fn script_string(vm: &mut ScriptVm, value: ScriptValue) -> Option<String> {
    vm.bx
        .heap
        .string_with(value, |_, source| source.to_string())
}

fn intern_size_expression(
    vm: &mut ScriptVm,
    source: &str,
) -> Result<SizeExprSimple, String> {
    vm.cx_mut().global::<SizeExprStore>().intern(source)
}

fn size_expr_source_to_value(vm: &mut ScriptVm, id: SizeExprId) -> Option<ScriptValue> {
    let source = vm
        .cx()
        .get_global_ref::<SizeExprStore>()?
        .source(id)?
        .to_string();
    Some(vm.bx.heap.new_string_from_str(&source))
}

impl ScriptHook for SizeExprId {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_string_like()
    }
}

impl ScriptNew for SizeExprId {
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Self::default()
    }
}

impl ScriptApply for SizeExprId {
    fn script_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(source) = script_string(vm, value) {
            match vm.cx_mut().global::<SizeExprStore>().intern_id(&source) {
                Ok(id) => *self = id,
                Err(error) => error!("invalid SizeExprId expression {:?}: {}", source, error),
            }
        } else {
            error!("SizeExprId requires a string expression");
        }
    }

    fn script_to_value(&self, vm: &mut ScriptVm) -> ScriptValue {
        let source = vm
            .cx()
            .get_global_ref::<SizeExprStore>()
            .and_then(|store| store.source(*self))
            .map(str::to_string);
        source.map_or(NIL, |source| vm.bx.heap.new_string_from_str(&source))
    }
}

/// Specifies the alignment of each walk with respect to their turtle's rectangle.
#[derive(Clone, Copy, Default, Debug, Script, ScriptHook)]
pub struct Align {
    /// The fraction of the turtle's unused inner width that will be added to the left of each walks:
    /// - Setting this to 0.0 will align each walk to the left.
    /// - Setting this to 0.5 will center each walk horizontally.
    /// - Setting this to 1.0 will align each walk to the right.
    #[live]
    pub x: f64,

    /// The fraction of the turtle's unused inner height that will be added above each walks:
    /// - Setting this to 0.0 will align each walk to the top.
    /// - Setting this to 0.5 will center each walk vertically.
    /// - Setting this to 1.0 will align each walk to the bottom.
    #[live]
    pub y: f64,
}

/// Specifies the direction in which walks are laid out.
#[derive(Copy, Clone, Debug, Script, ScriptHook, PartialEq)]
pub enum Flow {
    // Walks are laid out from left to right.
    #[pick {
        row_align: RowAlign::Top,
        wrap: false,
    }]
    Right {
        row_align: RowAlign,
        wrap: bool,
    },

    // Walks are laid out from top to bottom.
    Down,

    // Walks are laid out on top of each other.
    Overlay,
}

impl Flow {
    pub fn right() -> Self {
        Flow::Right {
            row_align: RowAlign::Top,
            wrap: false,
        }
    }

    pub fn right_wrap() -> Self {
        Flow::Right {
            row_align: RowAlign::Top,
            wrap: true,
        }
    }
}

impl Default for Flow {
    fn default() -> Self {
        Flow::Right {
            row_align: RowAlign::Top,
            wrap: false,
        }
    }
}

/// How each walk in a `Flow::Right { wrap: true }` row is vertically aligned
/// relative to the row's other walks.
///
/// All alignment is performed at row-finish time (i.e., when the row wraps
/// or when the turtle ends) by shifting the already-rendered items in the
/// turtle's align list. `Top` (the default) is a no-op.
#[derive(Copy, Clone, PartialEq, Debug, Script, ScriptHook)]
pub enum RowAlign {
    /// Each walk is placed at the top of the row (no post-adjustment). This
    /// is the default and matches the historical layout behavior.
    #[pick]
    Top,
    /// Each walk is shifted down so its bottom sits on the row baseline,
    /// using `Walk::metrics.descender` to separate baseline from bottom.
    /// Useful for text-heavy rows with mixed font sizes.
    Bottom,
    /// Each walk is shifted down so its vertical center aligns with the row's
    /// vertical center. Useful for mixing inline block widgets (e.g. pills)
    /// with surrounding text: the block stays put (it's the tallest item on
    /// the row), and the text slides down to the row's center so that the
    /// block's internal content visually aligns with the surrounding text.
    Center,
}

/// How positive slack is distributed between in-flow walks on the main axis.
#[derive(Copy, Clone, Default, Debug, PartialEq, Script, ScriptHook)]
pub enum Distribute {
    /// Keep walks at the main-axis start. This is the historical behavior.
    #[pick]
    #[default]
    Start,
    /// Put all positive slack between adjacent walks.
    SpaceBetween,
    /// Give each walk an equal share of slack on both sides.
    SpaceAround,
    /// Give every edge and every inter-walk boundary equal slack.
    SpaceEvenly,
}

/// The turtle is the main layout primitive in Makepad.
///
/// A turtle can be walked to allocate space on the screen. Each walk produces a rectangle that
/// represents the area allocated by the walk.
///
/// Turtles can be nested. When a nested turtle is created, the parent turtle starts a new walk. The
/// nested turtle then walks inside the rectangle of the parent turtle's walk. When the nested turtle
/// is finished, the parent turtle finishes its walk.
///
/// +-----------------+
/// | Padding Inset   |
/// | +-------------+ |
/// | | Margin Inset| |
/// | | +---------+ | |
/// | | | Content | | |
/// | | +---------+ | |
/// | +-------------+ |
/// +-----------------+
///
/// Inner rectangle = content
/// Rectangle       = content + padding
/// Outer rectangle = content + padding + margin
#[derive(Clone, Default, Debug)]
pub struct Turtle {
    walk: Walk,
    layout: Layout,
    width: f64,
    height: f64,
    used_width: f64,
    used_height: f64,
    prev_row_metrics: Metrics,
    current_row_metrics: Metrics,
    wrap_spacing: f64,
    align_start: usize,
    finished_rows_start: usize,
    finished_walks_start: usize,
    deferred_fills: Vec<DeferredFill>,
    flex_resolved: bool,
    next_flow_index: u32,
    pos: Vec2d,
    origin: Vec2d,
    guard: Area,
}

impl Turtle {
    /// Returns the `Walk` with which this turtle was created.
    pub fn walk(&self) -> Walk {
        self.walk
    }

    /// Returns the `Layout`` with which this turtle was created.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Return the margin around this turtle's rectangle.
    pub fn margin(&self) -> Inset {
        self.walk.margin
    }

    /// Returns the direction in which each walk of this turtle is laid out.
    pub fn flow(&self) -> Flow {
        self.layout.flow
    }

    /// Returns the spacing between each walk of this turtle.
    pub fn spacing(&self) -> f64 {
        self.layout.spacing
    }

    /// Returns the padding around the inner rectangle of each walk of this turtle.
    pub fn padding(&self) -> Inset {
        self.layout.padding
    }

    /// Sets the left padding of this turtle's layout.
    ///
    /// This is useful for adjusting the hanging indent of list items
    /// after measuring the actual width of the bullet/marker text.
    pub fn set_padding_left(&mut self, left: f64) {
        self.layout.padding.left = left;
    }

    /// Sets the right padding of this turtle's layout.
    ///
    /// Useful for temporarily reserving space at the right edge of an
    /// in-flow turtle (e.g. so wrapping text leaves room for trailing
    /// decoration drawn after the layout call). Save the previous value
    /// via [`Turtle::padding`] and restore it when done.
    pub fn set_padding_right(&mut self, right: f64) {
        self.layout.padding.right = right;
    }

    /// Sets whether this turtle's `Flow::Right` layout wraps onto a new row,
    /// leaving any other flow untouched.
    ///
    /// Turning wrapping off confines the following walks to the current row,
    /// which is how a caller that has run out of rows to give keeps an
    /// oversized walk from opening one anyway. Such a walk overruns the row's
    /// width instead, exactly as unwrappable text does. Save the previous
    /// setting via [`Turtle::layout`] and restore it when done.
    pub fn set_flow_wrap(&mut self, wrap: bool) {
        if let Flow::Right { wrap: flow_wrap, .. } = &mut self.layout.flow {
            *flow_wrap = wrap;
        }
    }

    /// Returns the alignment of each walk of this turtle with respect to it's rectangle.
    pub fn align(&self) -> Align {
        self.layout.align
    }

    /// Returns this turtle's inner rectangle.
    pub fn inner_rect(&self) -> Rect {
        Rect {
            pos: self.inner_origin(),
            size: self.inner_size(),
        }
    }

    /// Returns this turtle's inner rectangle, without scrolling applied.
    pub fn unscrolled_inner_rect(&self) -> Rect {
        Rect {
            pos: self.unscrolled_inner_origin(),
            size: self.inner_size(),
        }
    }

    /// Returns the origin of this turtle's inner rectangle.
    pub fn inner_origin(&self) -> Vec2d {
        self.origin + self.padding().left_top()
    }

    /// Returns the origin of this turtle's inner rectangle, without scrolling applied.
    pub fn unscrolled_inner_origin(&self) -> Vec2d {
        self.origin + self.scroll()
    }

    /// Returns the size of this turtle's inner rectangle.
    pub fn inner_size(&self) -> Vec2d {
        dvec2(self.inner_width(), self.inner_height())
    }

    /// Returns the width of this turtle's inner rectangle.
    ///
    /// If the inner width is unknown, then NaN is returned.
    pub fn inner_width(&self) -> f64 {
        self.width() - self.padding().width().min(self.width())
    }

    /// Returns the height of this turtle's inner rectangle.
    ///
    /// If the inner height is unknown, then NaN is returned.
    pub fn inner_height(&self) -> f64 {
        self.height() - self.padding().height().min(self.height())
    }

    /// Returns the used width of this turtle's inner rectangle.
    pub fn inner_used_width(&self) -> f64 {
        self.used_width() - self.padding().left.min(self.used_width())
    }

    /// Returns the used width of this turtle's inner rectangle on the current row.
    pub fn inner_used_width_current_row(&self) -> f64 {
        self.used_width_current_row() - self.padding().left.min(self.used_width_current_row())
    }

    /// Returns the used height of this turtle's inner rectangle.
    pub fn inner_used_height(&self) -> f64 {
        self.used_height() - self.padding().top.min(self.used_height())
    }

    /// Returns the unused width of this turtle's inner rectangle.
    ///
    /// If the unused inner width is unknown, then NaN is returned.
    pub fn unused_inner_width(&self) -> f64 {
        self.inner_width() - self.inner_used_width().min(self.inner_width())
    }

    /// Returns the unused width of this turtle's inner rectangle for the current row.
    ///
    /// If the unused inner width on the current row is unknown, then NaN is returned.
    pub fn unused_inner_width_for_current_row(&self) -> f64 {
        self.inner_width() - self.inner_used_width_current_row().min(self.inner_width())
    }

    /// Returns the unused height of this turtle's inner rectangle.
    ///
    /// If the unused inner height is unknown, then NaN is returned.
    pub fn unused_inner_height(&self) -> f64 {
        self.inner_height() - self.inner_used_height().min(self.inner_height())
    }

    /// Returns the effective width of this turtle's inner rectangle.
    ///
    /// This is either the inner width, or the used inner width if the inner width is unknown.
    pub fn effective_inner_width(&self) -> f64 {
        if !self.inner_width().is_nan() {
            self.inner_width()
        } else {
            self.inner_used_width()
        }
    }

    /// Returns the effective height of this turtle's inner rectangle.
    ///
    /// This is either the inner height, or the used inner height if the inner height is unknown.
    pub fn inner_effective_height(&self) -> f64 {
        if !self.inner_height().is_nan() {
            self.inner_height()
        } else {
            self.inner_used_height()
        }
    }

    /// Returns this turtle's rectangle.
    pub fn rect(&self) -> Rect {
        Rect {
            pos: self.origin(),
            size: self.size(),
        }
    }

    /// Returns this turtle's rectangle, without scrolling applied.
    pub fn rect_unscrolled(&self) -> Rect {
        Rect {
            pos: self.origin_unscrolled(),
            size: self.size(),
        }
    }

    /// Returns the origin of this turtle's rectangle.
    pub fn origin(&self) -> Vec2d {
        self.origin
    }

    /// Returns the origin of this turtle's rectangle, without scrolling applied.
    pub fn origin_unscrolled(&self) -> Vec2d {
        self.origin + self.layout.scroll
    }

    /// Returns the size of this turtle's rectangle.
    pub fn size(&self) -> Vec2d {
        dvec2(self.width(), self.height())
    }

    /// Returns the width of this turtle's rectangle.
    ///
    /// If the width is unknown, then NaN is returned.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// Returns the height of this turtle's rectangle.
    ///
    /// If the height is unknown, then NaN is returned.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// Sets the width of this turtle's rectangle.
    pub fn set_width(&mut self, width: f64) {
        self.width = width;
    }

    /// Sets the height of this turtle's rectangle.
    pub fn set_height(&mut self, height: f64) {
        self.height = height;
    }

    /// Returns the used width of this turtle's rectangle.
    pub fn used_width(&self) -> f64 {
        self.used_width
    }

    /// Returns the used width of this turtle's rectangle on the current row.
    pub fn used_width_current_row(&self) -> f64 {
        self.pos.x - self.origin.x
    }

    /// Returns the used height of this turtle's rectangle.
    pub fn used_height(&self) -> f64 {
        self.used_height
    }

    /// Returns the unused width of this turtle's rectangle.
    ///
    /// If the unused width is unknown, then NaN is returned.
    pub fn unused_width(&self) -> f64 {
        self.width() - self.used_width().min(self.width())
    }

    /// Returns the unused width of this turtle's rectangle on the current row.
    ///
    /// If the unused width on the current row is unknown, then NaN is returned.
    pub fn unused_width_current_row(&self) -> f64 {
        self.width() - self.used_width_current_row().min(self.width())
    }

    /// Returns the unused height of this turtle's rectangle.
    ///
    /// If the unused height is unknown, then NaN is returned.
    pub fn unused_height(&self) -> f64 {
        self.height() - self.used_height().min(self.height())
    }

    /// Returns the effective width of this turtle's rectangle.
    ///
    /// This is either the width, or the used width if the width is unknown.
    pub fn effective_width(&self) -> f64 {
        if !self.width().is_nan() {
            self.width()
        } else {
            self.used_width()
        }
    }

    /// Returns the effective height of this turtle's rectangle.
    ///
    /// This is either the height, or the used height if the height is unknown.
    pub fn effective_height(&self) -> f64 {
        if !self.height().is_nan() {
            self.height()
        } else {
            self.used_height()
        }
    }

    /// Returns this turtle's outer rectangle.
    pub fn outer_rect(&self) -> Rect {
        Rect {
            pos: self.outer_origin(),
            size: self.outer_size(),
        }
    }

    /// Returns this turtle's outer rectangle, without scrolling applied.
    pub fn unscrolled_outer_rectangle(&self) -> Rect {
        Rect {
            pos: self.unscrolled_outer_origin(),
            size: self.outer_size(),
        }
    }

    /// Returns the origin of this turtle's outer rectangle.
    pub fn outer_origin(&self) -> Vec2d {
        self.origin() - self.margin().left_top()
    }

    /// Returns the origin of this turtle's outer rectangle, without scrolling applied.
    pub fn unscrolled_outer_origin(&self) -> Vec2d {
        self.origin_unscrolled() - self.margin().left_top()
    }

    /// Returns the size of this turtle's outer rectangle.
    pub fn outer_size(&self) -> Vec2d {
        dvec2(self.outer_width(), self.outer_height())
    }

    /// Returns the width of this turtle's outer rectangle.
    ///
    /// If the outer width is unknown, then NaN is returned.
    pub fn outer_width(&self) -> f64 {
        self.width() + self.margin().width()
    }

    /// Returns the width of this turtle's outer rectangle.
    ///
    /// If the outer height is unknown, then NaN is returned.
    pub fn outer_height(&self) -> f64 {
        self.height() + self.margin().height()
    }

    /// Returns the used width of this turtle's outer rectangle.
    ///
    pub fn used_outer_width(&self) -> f64 {
        self.used_width() + self.margin().left
    }

    /// Returns the used width of this turtle's outer rectangle on the current row.
    pub fn used_outer_width_current_row(&self) -> f64 {
        self.used_width_current_row() + self.margin().left
    }

    /// Returns the used height of this turtle's outer rectangle.
    pub fn used_outer_height(&self) -> f64 {
        self.used_height() + self.margin().top
    }

    /// Returns the unused width of this turtle's outer rectangle.
    ///
    /// If the unused outer width is unknown, then NaN is returned.
    pub fn unused_outer_width(&self) -> f64 {
        self.outer_width() - self.used_outer_width().min(self.outer_width())
    }

    /// Returns the unused width of this turtle's outer rectangle on the current row.
    ///
    /// If the unused outer width on the current row is unknown, then NaN is returned.
    pub fn unused_outer_width_current_row(&self) -> f64 {
        self.outer_width() - self.used_outer_width_current_row().min(self.outer_width())
    }

    /// Returns the unused height of this turtle's outer rectangle.
    ///
    /// If the unused outer height is unknown, then NaN is returned.
    pub fn unused_outer_height(&self) -> f64 {
        self.outer_height() - self.used_outer_height().min(self.outer_height())
    }

    /// Returns the effective width of this turtle's outer rectangle.
    ///
    /// This is either the outer width, or the used outer width if the outer width is unknown.
    pub fn effective_outer_width(&self) -> f64 {
        if !self.outer_width().is_nan() {
            self.outer_width()
        } else {
            self.used_outer_width()
        }
    }

    /// Returns the effective height of this turtle's outer rectangle.
    ///
    /// This is either the outer height, or the used outer height if the outer height is unknown.
    pub fn effective_outer_height(&self) -> f64 {
        if !self.outer_height().is_nan() {
            self.outer_height()
        } else {
            self.used_outer_height()
        }
    }

    /// Returns the size of the rectangle of this turtle's next walk, based on the given desired
    /// `width`, `height`, and `margin`.
    pub fn next_walk_size(&self, width: Size, height: Size, margin: Inset) -> Vec2d {
        dvec2(
            self.next_walk_width(width, margin),
            self.next_walk_height(height, margin),
        )
    }

    /// Returns the width of the rectangle of this turtle's next walk, based on the given desired
    /// `width` and `margin`.
    ///
    /// - If the desired width is `Size::Fill`, then the actual width is computed as follows:
    ///
    ///   First, we compute the actual outer width. This depends on the direction in which this
    ///   turtle's walks are laid out:
    ///   - If this is `Flow::Right`, and wrapping is disabled, then the actual outer width of this
    ///     turtle's next walk is this turtle's remaining unused inner width.
    ///   - If this is `Flow::Right`, and wrapping is enabled, then the actual outer width of this
    ///     turtle's next walk is this turtle's remaining unused inner width on the current row.
    ///   - If this is either `Flow::Down` or `Flow::Overlay`, then the actual outer width of this
    ///     turtle's next walk is this turtle's effective inner width.
    ///   
    ///   Next, the actual outer width is clamped to the given `min` and `max`` constraints, if any.
    ///
    ///   Finally, the actual width is computed from the actual outer width by subtracting the
    ///   margin width.
    ///
    /// - If the desired width is `Size::Fixed`, then the actual width is simply the given width,
    ///   clamped to be at least 0.0.
    ///
    /// - If the desired width is `Size::Fit`, then the actual width cannot be computed until this
    ///   turtle's final unused inner width is known, so we return NaN to indicate that the actual
    ///   width is not yet known.
    pub fn next_walk_width(&self, width: Size, margin: Inset) -> f64 {
        match width {
            Size::Fill { min, max, .. } => {
                let outer_width = match self.layout.flow {
                    Flow::Right { wrap: false, .. } => self.unused_inner_width(),
                    Flow::Right { wrap: true, .. } => self.unused_inner_width_for_current_row(),
                    Flow::Down | Flow::Overlay => self.effective_inner_width(),
                };
                ContentBounds::from_fill(min, max, margin.width())
                    .clamp_available(outer_width - margin.width())
            }
            Size::Fixed(width) => width.max(0.0),
            Size::Fit { .. } => f64::NAN,
            Size::Rel { .. } | Size::Expr(_) => f64::NAN,
        }
    }

    /// Returns the height of the rectangle of this turtle's next walk, based on the given desired
    /// `height` and `margin`.
    ///
    /// - If the desired height is `Size::Fill`, then the actual height is computed as follows:
    ///   
    ///   First, we compute the actual outer height. This depends on the direction in which this
    ///   turtle's walks are laid out:
    ///   - If this is `Flow::Right`, or `Flow::Overlay``, then the actual outer height of this
    ///     turtle's next walk is this turtle's effective inner height.
    ///   - If this is `Flow::Down`, then the actual outer height of this turtle's next walk is
    ///     this turtle's remaining unused inner height.
    ///
    ///   Next, the actual outer height is clamped to the given `min` and `max` constraints, if any.
    ///
    ///   Finally, the actual height is computed from the actual outer height by subtracting the
    ///   margin height.
    ///
    /// - If the desired height is `Size::Fixed`, then the actual height is simply the given height,
    ///   clamped to be at least 0.0.
    ///
    /// - If the desired height is `Size::Fit`, then the actual height cannot be computed until this
    ///   turtle's final unused inner height is known, so we return NaN to indicate that the actual
    ///   height is not yet known.
    pub fn next_walk_height(&self, height: Size, margin: Inset) -> f64 {
        match height {
            Size::Fill { min, max, .. } => {
                let outer_height = match self.layout.flow {
                    Flow::Right { .. } | Flow::Overlay => self.inner_effective_height(),
                    Flow::Down => self.unused_inner_height(),
                };
                ContentBounds::from_fill(min, max, margin.height())
                    .clamp_available(outer_height - margin.height())
            }
            Size::Fixed(height) => height.max(0.0),
            Size::Fit { .. } => f64::NAN,
            Size::Rel { .. } | Size::Expr(_) => f64::NAN,
        }
    }

    /// Moves this turtle to the given position.
    pub fn move_to(&mut self, pos: Vec2d) {
        self.pos = pos
    }

    /// Moves this turtle right and down by the given amount.
    pub fn move_right_down(&mut self, amount: Vec2d) {
        self.move_to(self.pos() + amount);
    }

    /// Moves this turtle right by the given amount.
    pub fn move_right(&mut self, amount: f64) {
        self.move_right_down(dvec2(amount, 0.0))
    }

    /// Moves this turtle down by the given amount.
    pub fn move_down(&mut self, amount: f64) {
        self.move_right_down(dvec2(0.0, amount))
    }

    /// Allocates additional size to the right of and below this turtle's position.
    pub fn allocate_size(&mut self, additional: Vec2d) {
        self.allocate_width(additional.x);
        self.allocate_height(additional.y);
    }

    /// Allocates additional width to the right of this turtle's position.
    ///
    /// This will increase this turtle's used width if necessary.
    pub fn allocate_width(&mut self, additional: f64) {
        self.used_width = self
            .used_width
            .max(self.pos().x + additional - self.origin().x);
    }

    /// Allocates additional height below this turtle's position.
    ///
    /// This will increase this turtle's used height if necessary.
    pub fn allocate_height(&mut self, additional: f64) {
        self.used_height = self
            .used_height
            .max(self.pos().y + additional - self.origin().y);
    }

    fn _deferred_fill_count(&self) -> usize {
        self.deferred_fills.len()
    }

    fn total_resolved_delta_to(&self, index: usize) -> f64 {
        self.deferred_fills[..index]
            .iter()
            .map(|fill| fill.delta)
            .sum()
    }

    fn inner_free_length(&self) -> f64 {
        match self.layout.flow {
            Flow::Right { wrap: false, .. } => self.inner_width() - self.inner_used_width(),
            Flow::Down => self.inner_height() - self.inner_used_height(),
            _ => panic!(),
        }
    }

    fn resolve_fill(&mut self, index: usize) -> f64 {
        if !self.flex_resolved {
            self.solve_fills();
        }
        self.deferred_fills[index].basis + self.deferred_fills[index].delta
    }

    fn solve_fills(&mut self) {
        debug_assert!(!self.flex_resolved, "flex solver must run exactly once");
        self.flex_resolved = true;

        let mut free = self.inner_free_length();
        if !free.is_finite() || free == 0.0 {
            return;
        }
        let growing = free > 0.0;

        for _ in 0..self.deferred_fills.len() {
            let factor_scale = self
                .deferred_fills
                .iter()
                .filter(|fill| !fill.frozen)
                .map(|fill| fill.flex_factor(growing))
                .fold(0.0_f64, f64::max);
            if factor_scale <= 0.0 {
                break;
            }
            let factor_sum: f64 = self
                .deferred_fills
                .iter()
                .filter(|fill| !fill.frozen)
                .map(|fill| fill.flex_factor(growing) / factor_scale)
                .sum();

            let mut froze_any = false;
            for fill in &mut self.deferred_fills {
                if fill.frozen {
                    continue;
                }
                let factor = fill.flex_factor(growing) / factor_scale;
                let proposal = fill.basis + free * (factor / factor_sum);
                let resolved = fill.clamp(proposal);
                let blocked = if growing {
                    resolved < proposal
                } else {
                    resolved > proposal
                };
                fill.delta = resolved - fill.basis;
                if blocked {
                    fill.frozen = true;
                    froze_any = true;
                }
            }

            if !froze_any {
                break;
            }

            free = self.inner_free_length()
                - self
                    .deferred_fills
                    .iter()
                    .filter(|fill| fill.frozen)
                    .map(|fill| fill.delta)
                    .sum::<f64>();
        }
    }

    fn push_deferred_fill(&mut self, fill: DeferredFill) {
        debug_assert!(
            !self.flex_resolved,
            "cannot defer another fill after flex resolution"
        );
        self.deferred_fills.push(fill);
    }
}

impl DeferredFill {
    fn clamp(&self, value: f64) -> f64 {
        let mut value = value;
        if let Some(max) = self.max {
            value = value.min(max);
        }
        if let Some(min) = self.min {
            value = value.max(min);
        }
        value.max(0.0)
    }

    fn flex_factor(&self, growing: bool) -> f64 {
        let factor = if growing {
            self.grow
        } else {
            self.shrink * self.unclamped_basis.max(0.0)
        };
        if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            0.0
        }
    }
}

/// Represents a deferred walk.
///
/// A deferred walk is a walk for which the width/height is not yet known. It must be resolved when
/// its turtle has finished walking.
#[derive(Clone, Debug)]
pub enum DeferredWalk {
    /// An unresolved deferred walk.
    Unresolved {
        index: usize,
        pos: Vec2d,
        walk: Walk,
    },
    /// A resolved deferred walk.
    Resolved(Walk),
}

impl DeferredWalk {
    pub fn resolve(&mut self, cx: &mut Cx2d) -> Walk {
        match *self {
            Self::Unresolved {
                index,
                pos,
                mut walk,
            } => {
                {
                    let turtle = cx.turtles.last_mut().unwrap();
                    match turtle.flow() {
                        Flow::Right { wrap: false, .. } => {
                            let length = turtle.resolve_fill(index);
                            walk.abs_pos = Some(
                                pos + dvec2(turtle.total_resolved_delta_to(index), 0.0),
                            );
                            walk.width = Size::Fixed(length);
                        }
                        Flow::Down => {
                            let length = turtle.resolve_fill(index);
                            walk.abs_pos = Some(
                                pos + dvec2(0.0, turtle.total_resolved_delta_to(index)),
                            );
                            walk.height = Size::Fixed(length);
                        }
                        _ => panic!(),
                    }
                }
                // Main-axis Fill is now definite, so the ordinary resolver can
                // transfer `aspect` to a Fit cross axis and apply its bounds.
                walk.deferred = true;
                walk = cx.resolve_walk(walk, ResolveAt::BeforeBegin);
                *self = DeferredWalk::Resolved(walk);
                walk
            }
            Self::Resolved(walk) => walk,
        }
    }
}

/// Represents a finished walk.
#[derive(Clone, Default, Debug)]
pub struct FinishedWalk {
    /// The start of the align list of this finished walk.
    ///
    /// The end of the align list of this finished walk is implicit: it is either the start of the
    /// align tree of the next finished walk, or the end of the global align list if this is the
    /// last finished walk.
    align_list_start: usize,

    /// The number of deferred walks before this finished walk.
    deferred_before_count: usize,

    /// The size of the outer rectangle of this finished walk.
    outer_size: Vec2d,

    /// Whether this walk participates in normal flow and distribution.
    in_flow: bool,

    /// Declaration order within the owning turtle.
    flow_index: u32,

    metrics: Metrics,

    /// How row alignment may treat this walk (see [`RowAlignRole`]).
    align_role: RowAlignRole,

    /// Height to center by under `RowAlign::Center` instead of `outer_size.y`.
    /// Text runs pass their line's height here so mixed-font runs on a row all
    /// get the same shift and keep their relative baselines.
    align_height: Option<f64>,
}

/// How row alignment may treat a finished walk.
///
/// A multi-row text run draws all of its glyphs in one instance batch, so no
/// individual row of it can ever be repositioned; the run's per-row walks are
/// therefore immovable and declare one of the non-`Shiftable` roles.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum RowAlignRole {
    /// Row alignment may shift this walk's align range.
    #[default]
    Shiftable,
    /// Immovable, and under `RowAlign::Center` the row's center line anchors
    /// to this walk's own center instead of the tallest walk's, so every
    /// shiftable walk on the row — including a taller one, which then shifts
    /// UP — centers on it. Declared by a wrapped run's rows that hold visible
    /// text.
    Anchor,
    /// Immovable and inert: neither shifted nor an anchor. Declared by a
    /// wrapped run's first-row walk when that row holds no glyphs (the run
    /// wrapped immediately), so a row of other content is not anchored to an
    /// invisible line.
    Fixed,
}

/// The horizontal shift that centers a `Flow::Right` row's actually-drawn content
/// within its inner width, per `align.x`.
///
/// Deferred fills are given all the slack up front, so the normal align path skips
/// align.x when any exist. But a fill that draws narrower than its slot (an image
/// that aspect-fits, say) leaves genuine slack, and this reclaims it. Returns 0 when
/// the content fills the row or the inner width is unknown.
fn row_align_x_shift(
    align_x: f64,
    inner_width: f64,
    spacing: f64,
    walks: &[FinishedWalk],
) -> f64 {
    if align_x == 0.0 || walks.is_empty() || inner_width.is_nan() {
        return 0.0;
    }
    let gaps = spacing * walks.len().saturating_sub(1) as f64;
    let used: f64 = walks.iter().map(|w| w.outer_size.x).sum::<f64>() + gaps;
    align_x * (inner_width - used).max(0.0)
}

/// The vertical counterpart of [`row_align_x_shift`] for a `Flow::Down` column.
///
/// A `height: Fill` child that draws shorter than its slot leaves genuine slack;
/// this reclaims it for `align.y`. Returns 0 when the column fills, `align.y` is 0,
/// or the inner height is unknown.
fn col_align_y_shift(
    align_y: f64,
    inner_height: f64,
    spacing: f64,
    walks: &[FinishedWalk],
) -> f64 {
    if align_y == 0.0 || walks.is_empty() || inner_height.is_nan() {
        return 0.0;
    }
    let gaps = spacing * walks.len().saturating_sub(1) as f64;
    let used: f64 = walks.iter().map(|w| w.outer_size.y).sum::<f64>() + gaps;
    align_y * (inner_height - used).max(0.0)
}

#[derive(Clone, Copy)]
struct DistributionGroup {
    count: usize,
    slack: f64,
}

impl DistributionGroup {
    fn for_walks(
        inner: f64,
        spacing: f64,
        walks: &[FinishedWalk],
        axis: Axis,
    ) -> Option<Self> {
        let mut count: usize = 0;
        let mut used = 0.0;
        let mut immovable = false;
        for walk in walks.iter().filter(|walk| walk.in_flow) {
            count += 1;
            used += match axis {
                Axis::Width => walk.outer_size.x,
                Axis::Height => walk.outer_size.y,
            };
            immovable |= walk.align_role != RowAlignRole::Shiftable;
        }
        if immovable {
            #[cfg(debug_assertions)]
            log!("main-axis distribution skipped for a group containing immovable text");
            return None;
        }
        if count == 0 {
            return None;
        }
        used += spacing * count.saturating_sub(1) as f64;
        let slack = inner - used;
        Some(Self {
            count,
            slack: if slack.is_finite() && slack > 0.0 {
                slack
            } else {
                0.0
            },
        })
    }

    fn offset(self, mode: Distribute, rank: usize) -> f64 {
        match mode {
            Distribute::Start => 0.0,
            Distribute::SpaceBetween if self.count <= 1 => 0.0,
            Distribute::SpaceBetween => self.slack * rank as f64 / (self.count - 1) as f64,
            Distribute::SpaceAround => {
                self.slack * (rank as f64 + 0.5) / self.count as f64
            }
            Distribute::SpaceEvenly => {
                self.slack * (rank + 1) as f64 / (self.count + 1) as f64
            }
        }
    }
}

fn distribution_rank(walks: &[FinishedWalk], flow_index: u32) -> usize {
    walks
        .iter()
        .filter(|walk| walk.in_flow && walk.flow_index < flow_index)
        .count()
}

impl<'a, 'b> Cx2d<'a, 'b> {
    /// Returns a reference to the current turtle.
    pub fn turtle(&self) -> &Turtle {
        self.turtles.last().unwrap()
    }

    pub fn turtle_is_at_first_row(&self) -> bool {
        self.turtle().finished_rows_start == self.finished_rows.len()
    }

    /// Returns true if the current turtle's next walk would be it's first.
    pub fn turtle_next_walk_is_first(&self) -> bool {
        self.turtle().next_flow_index == 0
    }

    fn turtle_current_row_has_in_flow_walks(&self) -> bool {
        self.finished_walks[self.current_row_walks_start()..]
            .iter()
            .any(|walk| walk.in_flow)
    }

    /// Returns the offset to the current turtle's next walk.
    ///
    /// This is either zero if the current turtle's next walk would be its first, or the current
    /// turtle's spacing in the direction of it's flow otherwise.
    pub fn turtle_next_walk_offset(&self) -> Vec2d {
        let first_on_wrapped_row = matches!(
            self.turtle().layout.flow,
            Flow::Right { wrap: true, .. }
        ) && !self.turtle_current_row_has_in_flow_walks();
        if self.turtle_next_walk_is_first() || first_on_wrapped_row {
            dvec2(0.0, 0.0)
        } else {
            match self.turtle().layout.flow {
                Flow::Right { .. } => dvec2(self.turtle().spacing(), 0.0),
                Flow::Down => dvec2(0.0, self.turtle().spacing()),
                Flow::Overlay => dvec2(0.0, 0.0),
            }
        }
    }

    /// Returns a mutable reference to the current turtle.
    pub fn turtle_mut(&mut self) -> &mut Turtle {
        self.turtles.last_mut().unwrap()
    }

    /// Resolves contextual sizes, content-box constraints, and generic aspect
    /// sizing for the requested layout phase. Calling this repeatedly is safe.
    pub fn resolve_walk(&self, walk: Walk, at: ResolveAt) -> Walk {
        if !walk.needs_resolve() {
            return walk;
        }

        let mut walk = walk;
        walk.width = self.resolve_size(walk.width, Axis::Width, at);
        walk.height = self.resolve_size(walk.height, Axis::Height, at);
        walk.width = self.resolve_size_bounds(walk.width, Axis::Width, at);
        walk.height = self.resolve_size_bounds(walk.height, Axis::Height, at);
        walk.min_width = self.resolve_bound_declaration(walk.min_width, Axis::Width, at);
        walk.max_width = self.resolve_bound_declaration(walk.max_width, Axis::Width, at);
        walk.min_height = self.resolve_bound_declaration(walk.min_height, Axis::Height, at);
        walk.max_height = self.resolve_bound_declaration(walk.max_height, Axis::Height, at);

        let width_bounds = self.content_bounds(&walk, Axis::Width, at);
        let height_bounds = self.content_bounds(&walk, Axis::Height, at);
        walk.width = self.apply_before_bounds(walk.width, width_bounds, walk.margin.width());
        walk.height = self.apply_before_bounds(walk.height, height_bounds, walk.margin.height());

        if let Some(aspect) = walk.aspect.filter(|aspect| aspect.is_finite() && *aspect > 0.0) {
            match (walk.width, walk.height) {
                (Size::Fit { .. }, Size::Fixed(height)) => {
                    walk.width = Size::Fixed(width_bounds.clamp(height * aspect));
                }
                (Size::Fixed(width), Size::Fit { .. }) => {
                    walk.height = Size::Fixed(height_bounds.clamp(width / aspect));
                }
                (Size::Fit { .. }, Size::Fill { .. })
                    if self.fill_is_immediate_cross_axis(Axis::Height, at) =>
                {
                    if let Some(height) = self.materialize_fill(walk.height, Axis::Height, walk.margin, at) {
                        walk.height = Size::Fixed(height_bounds.clamp(height));
                        walk.width = Size::Fixed(width_bounds.clamp(height * aspect));
                    }
                }
                (Size::Fill { .. }, Size::Fit { .. })
                    if self.fill_is_immediate_cross_axis(Axis::Width, at) =>
                {
                    if let Some(width) = self.materialize_fill(walk.width, Axis::Width, walk.margin, at) {
                        walk.width = Size::Fixed(width_bounds.clamp(width));
                        walk.height = Size::Fixed(height_bounds.clamp(width / aspect));
                    }
                }
                _ => {}
            }
        }

        if let Size::Fixed(width) = walk.width {
            walk.width = Size::Fixed(width_bounds.clamp(width));
        }
        if let Size::Fixed(height) = walk.height {
            walk.height = Size::Fixed(height_bounds.clamp(height));
        }
        walk
    }

    fn resolve_size(&self, size: Size, axis: Axis, at: ResolveAt) -> Size {
        match size {
            // These bases describe close-time layout state and are supported
            // for Fit bounds only. A Size object must never sample the moving
            // pen, at either phase.
            Size::Rel {
                base: Base::Line | Base::Unused,
                ..
            } => Size::fit(),
            Size::Rel { base, factor } => self
                .resolve_base(base, axis, at)
                .filter(|value| value.is_finite())
                .map_or_else(Size::fit, |value| Size::Fixed(value * factor)),
            Size::Expr(id) => self
                .eval_size_expr(id, axis, at)
                .filter(|value| value.is_finite())
                .map_or_else(Size::fit, Size::Fixed),
            other => other,
        }
    }

    fn resolve_size_bounds(&self, size: Size, axis: Axis, at: ResolveAt) -> Size {
        match size {
            Size::Fill {
                weight,
                basis,
                shrink,
                min,
                max,
            } => Size::Fill {
                weight,
                basis: self
                    .resolve_bound_declaration(Some(basis), axis, at)
                    .unwrap_or(basis),
                shrink,
                min,
                max,
            },
            Size::Fit { min, max } => Size::Fit {
                min: self.resolve_bound_declaration(min, axis, at),
                max: self.resolve_bound_declaration(max, axis, at),
            },
            other => other,
        }
    }

    fn resolve_bound_declaration(
        &self,
        bound: Option<FitBound>,
        axis: Axis,
        at: ResolveAt,
    ) -> Option<FitBound> {
        bound.map(|bound| {
            self.eval_fit_bound(bound, axis, at)
                .filter(|value| value.is_finite())
                .map_or(bound, FitBound::Abs)
        })
    }

    fn content_bounds(&self, walk: &Walk, axis: Axis, at: ResolveAt) -> ContentBounds {
        self.content_bounds_with_parent(
            walk,
            axis,
            self.parent_index(at),
            at == ResolveAt::AtClose,
        )
    }

    fn content_bounds_with_parent(
        &self,
        walk: &Walk,
        axis: Axis,
        parent_index: Option<usize>,
        allow_close_bases: bool,
    ) -> ContentBounds {
        let (size, walk_min, walk_max, margin) = match axis {
            Axis::Width => (walk.width, walk.min_width, walk.max_width, walk.margin.width()),
            Axis::Height => (walk.height, walk.min_height, walk.max_height, walk.margin.height()),
        };
        let mut bounds = ContentBounds::default();
        match size {
            Size::Fill { min, max, .. } => {
                bounds = ContentBounds::from_fill(min, max, margin);
            }
            Size::Fit { min, max } => {
                bounds.add_min(
                    min.and_then(|bound| {
                        self.eval_fit_bound_with_parent(
                            bound,
                            axis,
                            parent_index,
                            allow_close_bases,
                        )
                    })
                        .map(|value| (value - margin).max(0.0)),
                );
                bounds.add_max(
                    max.and_then(|bound| {
                        self.eval_fit_bound_with_parent(
                            bound,
                            axis,
                            parent_index,
                            allow_close_bases,
                        )
                    })
                        .map(|value| (value - margin).max(0.0)),
                );
            }
            _ => {}
        }
        bounds.add_min(walk_min.and_then(|bound| {
            self.eval_fit_bound_with_parent(bound, axis, parent_index, allow_close_bases)
        }));
        bounds.add_max(walk_max.and_then(|bound| {
            self.eval_fit_bound_with_parent(bound, axis, parent_index, allow_close_bases)
        }));
        bounds.normalize();
        bounds
    }

    pub fn walk_max_width(&self, walk: Walk) -> Option<f64> {
        self.content_bounds(&walk, Axis::Width, ResolveAt::BeforeBegin).max
    }

    pub fn walk_max_height(&self, walk: Walk) -> Option<f64> {
        self.content_bounds(&walk, Axis::Height, ResolveAt::BeforeBegin).max
    }

    pub fn current_turtle_max_width(&self) -> Option<f64> {
        let index = self.turtles.len().checked_sub(1)?;
        self.content_bounds_for_turtle(&self.turtles[index].walk, Axis::Width, index)
            .max
    }

    pub fn current_turtle_max_height(&self) -> Option<f64> {
        let index = self.turtles.len().checked_sub(1)?;
        self.content_bounds_for_turtle(&self.turtles[index].walk, Axis::Height, index)
            .max
    }

    fn content_bounds_for_turtle(
        &self,
        walk: &Walk,
        axis: Axis,
        turtle_index: usize,
    ) -> ContentBounds {
        self.content_bounds_with_parent(walk, axis, turtle_index.checked_sub(1), true)
    }

    fn apply_before_bounds(&self, size: Size, bounds: ContentBounds, margin: f64) -> Size {
        match size {
            Size::Fixed(value) => Size::Fixed(bounds.clamp(value)),
            Size::Fill {
                weight,
                basis,
                shrink,
                ..
            } => Size::Fill {
                weight,
                basis,
                shrink,
                min: bounds.min.map(|value| value + margin),
                max: bounds.max.map(|value| value + margin),
            },
            other => other,
        }
    }

    fn fill_is_immediate_cross_axis(&self, axis: Axis, at: ResolveAt) -> bool {
        let Some(parent_index) = self.parent_index(at) else {
            return false;
        };
        match (self.turtles[parent_index].flow(), axis) {
            (Flow::Right { .. }, Axis::Height)
            | (Flow::Down, Axis::Width)
            | (Flow::Overlay, _) => true,
            _ => false,
        }
    }

    fn materialize_fill(
        &self,
        size: Size,
        axis: Axis,
        margin: Inset,
        at: ResolveAt,
    ) -> Option<f64> {
        let parent = &self.turtles[self.parent_index(at)?];
        let value = match axis {
            Axis::Width => parent.next_walk_width(size, margin),
            Axis::Height => parent.next_walk_height(size, margin),
        };
        value.is_finite().then_some(value)
    }

    fn parent_index(&self, at: ResolveAt) -> Option<usize> {
        self.turtles.len().checked_sub(match at {
            ResolveAt::BeforeBegin => 1,
            ResolveAt::AtClose => 2,
        })
    }

    fn resolve_base(&self, base: Base, axis: Axis, at: ResolveAt) -> Option<f64> {
        self.resolve_base_from_parent(base, axis, self.parent_index(at))
    }

    fn resolve_base_from_parent(
        &self,
        base: Base,
        axis: Axis,
        parent_index: Option<usize>,
    ) -> Option<f64> {
        let dimension = |turtle: &Turtle, axis| match axis {
            Axis::Width => turtle.inner_width(),
            Axis::Height => turtle.inner_height(),
        };
        match base {
            Base::Parent => {
                let value = dimension(self.turtles.get(parent_index?)?, axis);
                value.is_finite().then_some(value)
            }
            Base::Vw => self.viewport_size().x.is_finite().then_some(self.viewport_size().x),
            Base::Vh => self.viewport_size().y.is_finite().then_some(self.viewport_size().y),
            Base::Cqw => self.resolve_container_dimension(None, Axis::Width, parent_index),
            Base::Cqh => self.resolve_container_dimension(None, Axis::Height, parent_index),
            Base::Named(id) => self.resolve_container_dimension(Some(id), axis, parent_index),
            Base::Line => {
                if axis == Axis::Width {
                    let turtle_index = parent_index.map_or(0, |index| index + 1);
                    self.find_line_available_width_for_turtle(turtle_index)
                } else {
                    None
                }
            }
            Base::Full | Base::Unused => {
                let mut index = parent_index?;
                loop {
                    let turtle = &self.turtles[index];
                    let value = match (base, axis) {
                        (Base::Full, Axis::Width) => turtle.width(),
                        (Base::Full, Axis::Height) => turtle.height(),
                        (Base::Unused, Axis::Width) => turtle.unused_width(),
                        (Base::Unused, Axis::Height) => turtle.unused_height(),
                        _ => unreachable!(),
                    };
                    if value.is_finite() {
                        return Some(value);
                    }
                    let Some(next) = index.checked_sub(1) else {
                        return None;
                    };
                    index = next;
                }
            }
        }
    }

    fn resolve_container_dimension(
        &self,
        named: Option<LiveId>,
        axis: Axis,
        parent_index: Option<usize>,
    ) -> Option<f64> {
        let mut index = parent_index;
        while let Some(current) = index {
            let turtle = &self.turtles[current];
            let id = turtle.layout.container_id;
            let matches = named.map_or(id != LiveId(0), |wanted| id == wanted && id != LiveId(0));
            if matches {
                let value = match axis {
                    Axis::Width => turtle.inner_width(),
                    Axis::Height => turtle.inner_height(),
                };
                // A matching-but-unknown container is intentionally terminal:
                // do not skip outward or use viewport fallback.
                return value.is_finite().then_some(value);
            }
            index = current.checked_sub(1);
        }
        let viewport = self.viewport_size();
        let value = match axis {
            Axis::Width => viewport.x,
            Axis::Height => viewport.y,
        };
        value.is_finite().then_some(value)
    }

    fn viewport_size(&self) -> Vec2d {
        self.owning_window_or_root_pass_size()
    }

    pub(crate) fn eval_size_expr(
        &self,
        id: SizeExprId,
        axis: Axis,
        at: ResolveAt,
    ) -> Option<f64> {
        self.eval_size_expr_with_parent(id, axis, self.parent_index(at))
    }

    /// Evaluates a stored size expression against the current turtle's inner
    /// axis. This is the shared contextual-size seam used by definite Grid
    /// tracks; `horizontal == false` selects the vertical axis.
    pub fn eval_size_expr_in_current_turtle(
        &self,
        id: SizeExprId,
        horizontal: bool,
    ) -> Option<f64> {
        self.eval_size_expr(
            id,
            if horizontal { Axis::Width } else { Axis::Height },
            ResolveAt::BeforeBegin,
        )
    }

    /// Whether an expression can be resolved without a parent/container base.
    pub fn size_expr_is_content_independent(&self, id: SizeExprId) -> bool {
        self.get_global_ref::<SizeExprStore>()
            .is_some_and(|store| !store.requires_parent_or_container(id))
    }

    fn size_expr_context(
        &self,
        axis: Axis,
        parent_index: Option<usize>,
    ) -> SizeExprContext {
        let viewport = self.viewport_size();
        SizeExprContext {
            parent: self
                .resolve_base_from_parent(Base::Parent, axis, parent_index)
                .unwrap_or(f64::NAN),
            viewport_width: viewport.x,
            viewport_height: viewport.y,
            container_width: self
                .resolve_container_dimension(None, Axis::Width, parent_index)
                .unwrap_or(f64::NAN),
            container_height: self
                .resolve_container_dimension(None, Axis::Height, parent_index)
                .unwrap_or(f64::NAN),
        }
    }

    fn eval_size_expr_with_parent(
        &self,
        id: SizeExprId,
        axis: Axis,
        parent_index: Option<usize>,
    ) -> Option<f64> {
        let value = self
            .get_global_ref::<SizeExprStore>()?
            .eval(id, self.size_expr_context(axis, parent_index));
        value.is_finite().then_some(value)
    }

    fn eval_fit_bound(&self, bound: FitBound, axis: Axis, at: ResolveAt) -> Option<f64> {
        self.eval_fit_bound_with_parent(
            bound,
            axis,
            self.parent_index(at),
            at == ResolveAt::AtClose,
        )
    }

    fn eval_fit_bound_with_parent(
        &self,
        bound: FitBound,
        axis: Axis,
        parent_index: Option<usize>,
        allow_close_bases: bool,
    ) -> Option<f64> {
        match bound {
            FitBound::Abs(value) => Some(value),
            FitBound::Rel {
                base: Base::Line | Base::Unused,
                ..
            } if !allow_close_bases => None,
            FitBound::Rel { base, factor } => {
                Some(self.resolve_base_from_parent(base, axis, parent_index)? * factor)
            }
            FitBound::Expr(id) => self.eval_size_expr_with_parent(id, axis, parent_index),
        }
    }

    fn eval_fit_bound_for_turtle(
        &self,
        bound: FitBound,
        axis: Axis,
        turtle_index: usize,
    ) -> Option<f64> {
        self.eval_fit_bound_with_parent(bound, axis, turtle_index.checked_sub(1), true)
    }

    pub fn find_base_width(&self, base: Base) -> Option<f64> {
        self.resolve_base(base, Axis::Width, ResolveAt::AtClose)
    }

    pub fn find_base_height(&self, base: Base) -> Option<f64> {
        self.resolve_base(base, Axis::Height, ResolveAt::AtClose)
    }

    /// Returns the width available on the enclosing line for the current
    /// turtle's content, for a [`Base::Line`] bound.
    ///
    /// The line is the nearest enclosing turtle with a known width; the
    /// unresolved `Fit` turtles between it and the current one (an inline
    /// widget's nesting levels) contribute their leading geometry and
    /// trailing insets. The line's flow selects between two measurements,
    /// each of which is final at the moment it is taken:
    ///
    /// - A wrapping line can relocate the inline widget whole onto a fresh
    ///   row, so the bound is what a fresh row offers: the line's inner
    ///   width minus the widget-internal lead-in before this turtle and
    ///   the trailing insets after it. Content sized to this bound either
    ///   fits where it is, or fits the row the widget is relocated to.
    /// - A non-wrapping line (including one held non-wrapping by an
    ///   inline-content clamp on the last permitted row) keeps the widget
    ///   where it is, so the bound is the remnant: the distance from the
    ///   pen to the line's inner right edge, minus the trailing insets.
    ///
    /// The current turtle's own trailing margin is left for the consumer,
    /// which resolves a `Fit` max bound against the walk's margin box. Its
    /// leading margin is part of the measured lead-in (the turtle's origin
    /// lies past it), so a consumer that subtracts the full margin width
    /// counts the leading side twice — a deliberately conservative overlap
    /// of a few pixels that keeps a remnant-fitted walk safely inside the
    /// line's overrun tolerance.
    pub fn find_line_available_width(&self) -> Option<f64> {
        let turtle_index = self.turtles.len().checked_sub(1)?;
        self.find_line_available_width_for_turtle(turtle_index)
    }

    fn find_line_available_width_for_turtle(&self, turtle_index: usize) -> Option<f64> {
        let current = self.turtles.get(turtle_index)?;
        let outer_turtles = &self.turtles[..turtle_index];
        // Measure from the closing turtle's content origin, not its moving pen.
        let start_x = current.origin().x + current.padding().left;
        let mut trailing = current.padding().right;
        // The outermost unresolved turtle inside the line so far: the
        // inline widget's root, whose origin marks where its lead-in
        // (icons, padding, spacing before this content) begins.
        let mut widget_origin_x = current.origin().x;
        let mut widget_margin_left = 0.0;
        for turtle in outer_turtles.iter().rev() {
            if !turtle.width().is_nan() {
                let inner = turtle.inner_rect();
                let available = match turtle.layout().flow {
                    Flow::Right { wrap: false, .. } => {
                        inner.pos.x + inner.size.x - start_x - trailing
                    }
                    Flow::Right { wrap: true, .. } | Flow::Down | Flow::Overlay => {
                        inner.size.x - widget_margin_left - (start_x - widget_origin_x) - trailing
                    }
                };
                return Some(available.max(0.0));
            }
            trailing += turtle.padding().right + turtle.walk().margin.right;
            widget_origin_x = turtle.origin().x;
            widget_margin_left = turtle.walk().margin.left;
        }
        None
    }

    /// Starts a root turtle.
    pub fn begin_root_turtle(&mut self, size: Vec2d, layout: Layout) {
        self.align_list
            .push(AlignEntry::BeginClip(dvec2(0.0, 0.0), size));

        let turtle = Turtle {
            walk: Walk::fixed(size.x, size.y),
            layout,
            align_start: self.align_list.len() - 1,
            finished_rows_start: self.finished_rows.len(),
            finished_walks_start: self.finished_walks.len(),
            deferred_fills: Vec::new(),
            flex_resolved: false,
            next_flow_index: 0,
            pos: Vec2d {
                x: layout.padding.left,
                y: layout.padding.top,
            },
            wrap_spacing: layout.wrap_spacing,
            origin: dvec2(0.0, 0.0),
            width: size.x,
            height: size.y,
            used_width: layout.padding.left,
            used_height: layout.padding.top,
            prev_row_metrics: Metrics::default(),
            current_row_metrics: Metrics::default(),
            guard: Area::Empty,
        };

        self.turtles.push(turtle);
    }

    /// Starts a root turtle with clipping disabled.
    pub fn begin_unclipped_root_turtle(&mut self, size: Vec2d, layout: Layout) {
        self.begin_root_turtle(size, layout);
        *self.align_list.last_mut().unwrap() = AlignEntry::Unset;
    }

    /// Starts a root turtle for the current pass.
    pub fn begin_root_turtle_for_pass(&mut self, layout: Layout) {
        let size = self.current_pass_size();
        self.begin_root_turtle(size, layout)
    }

    /// Starts a root turtle with clipping disabled for the current pass.
    pub fn begin_unclipped_root_turtle_for_pass(&mut self, layout: Layout) {
        let size = self.current_pass_size();
        self.begin_unclipped_root_turtle(size, layout)
    }

    /// Starts a nested turtle.
    ///
    /// When a nested turtle is started, the parent turtle starts a new walk with the given `walk`.
    /// The nested turtle then walks inside the rectangle of the parent turtle's walk. When the
    /// nested turtle is finished, the parent turtle finishes its walk.
    ///
    /// The given `layout` determines how the nested turtle's walks are laid out with respect to
    /// each other.
    ///
    /// The nested turtle's rectangle is that of the parent turtle's walk. Since the width/height
    /// of this walk may be `Size::Fit`, the width/height of this rectangle may not be known until
    /// the nested turtle is finished.
    pub fn begin_turtle(&mut self, walk: Walk, layout: Layout) {
        self.begin_turtle_with_guard(walk, layout, Area::Empty)
    }

    /// Starts a nested turtle, with a guard area.
    ///
    /// When the nested turtle is later finished, it should be finished with the same guard area
    /// that was used to start it.
    ///
    /// See [`begin_turtle`] for more information.
    pub fn begin_turtle_with_guard(&mut self, walk: Walk, layout: Layout, guard: Area) {
        let walk = self.resolve_walk(walk, ResolveAt::BeforeBegin);
        let parent = self.turtle();

        let outer_origin = if let Some(outer_origin) = walk.abs_pos {
            outer_origin
        } else {
            parent.pos() + self.turtle_next_walk_offset()
        };
        let origin = outer_origin + walk.margin.left_top();

        let size = parent.next_walk_size(walk.width, walk.height, walk.margin);

        let clip_min = dvec2(
            if layout.clip_x { origin.x } else { f64::NAN },
            if layout.clip_y { origin.y } else { f64::NAN },
        );

        let clip_max = dvec2(
            if layout.clip_x {
                origin.x + size.x
            } else {
                f64::NAN
            },
            if layout.clip_y {
                origin.y + size.y
            } else {
                f64::NAN
            },
        );

        let origin = origin - layout.scroll;

        self.align_list
            .push(AlignEntry::BeginClip(clip_min, clip_max));

        let turtle = Turtle {
            walk,
            layout,
            align_start: self.align_list.len() - 1,
            finished_rows_start: self.finished_rows.len(),
            finished_walks_start: self.finished_walks.len(),
            deferred_fills: Vec::new(),
            flex_resolved: false,
            next_flow_index: 0,
            wrap_spacing: layout.wrap_spacing,
            pos: Vec2d {
                x: origin.x + layout.padding.left,
                y: origin.y + layout.padding.top,
            },
            origin,
            width: size.x,
            height: size.y,
            used_width: layout.padding.left,
            used_height: layout.padding.top,
            prev_row_metrics: Metrics::default(),
            current_row_metrics: Metrics::default(),
            guard,
        };

        self.turtles.push(turtle);
    }

    /// Finishes the current turtle.
    pub fn end_turtle(&mut self) -> Rect {
        self.end_turtle_with_guard(Area::Empty)
    }

    /// Finishes the current turtle, with a guard area.
    ///
    /// The current turtle should be finished with the same guard area that was used to start it.
    pub fn end_turtle_with_guard(&mut self, guard: Area) -> Rect {
        // The final row's bottom forgiveness is deliberately discarded: the
        // turtle's used height keeps that row's full physical extent, so
        // up-centered walks on a last row stay inside the reported rect and
        // its clip bottom.
        let _ = self.finish_row(self.align_list.len());
        self.compute_final_size();

        let (
            turtle_align_start,
            turtle_walks_start,
            turtle_rows_start,
            scope_rect,
            flow,
            align,
            distribute,
            has_deferred,
            inner_width,
            inner_height,
            inner_effective_width,
            inner_effective_height,
            unused_inner_width,
            unused_inner_height,
            spacing,
        ) = {
            let turtle = self.turtles.last().unwrap();
            if guard != turtle.guard {
                panic!(
                    "End turtle guard area misaligned!, begin/end pair not matched begin {:?} end {:?}",
                    turtle.guard, guard
                )
            }
            (
                turtle.align_start,
                turtle.finished_walks_start,
                turtle.finished_rows_start,
                turtle.rect(),
                turtle.flow(),
                turtle.align(),
                turtle.layout.distribute,
                !turtle.deferred_fills.is_empty(),
                turtle.inner_width(),
                turtle.inner_height(),
                turtle.effective_inner_width(),
                turtle.inner_effective_height(),
                turtle.unused_inner_width(),
                turtle.unused_inner_height(),
                turtle.spacing(),
            )
        };

        // Close-time placement is one normalized pass. Every align-list range
        // moves at most once, combining a preceding flex delta with either the
        // historical Start alignment or its distribution offset.
        match flow {
            Flow::Right { wrap: false, .. } => {
                let walks_end = self.finished_walks.len();
                let group = (distribute != Distribute::Start)
                    .then(|| {
                        DistributionGroup::for_walks(
                            inner_width,
                            spacing,
                            &self.finished_walks[turtle_walks_start..walks_end],
                            Axis::Width,
                        )
                    })
                    .flatten();
                let use_start = distribute == Distribute::Start
                    || (group.is_none()
                        && self.finished_walks[turtle_walks_start..walks_end]
                            .iter()
                            .any(|walk| walk.in_flow));
                let start_dx = if use_start {
                    if has_deferred {
                        row_align_x_shift(
                            align.x,
                            inner_width,
                            spacing,
                            &self.finished_walks[turtle_walks_start..walks_end],
                        )
                    } else {
                        align.x * unused_inner_width
                    }
                } else {
                    0.0
                };

                for index in turtle_walks_start..walks_end {
                    let (range_start, outer_height, in_flow, flow_index, role, delta) = {
                        let walk = &self.finished_walks[index];
                        (
                            walk.align_list_start,
                            walk.outer_size.y,
                            walk.in_flow,
                            walk.flow_index,
                            walk.align_role,
                            self.turtle()
                                .total_resolved_delta_to(walk.deferred_before_count),
                        )
                    };
                    let rank = distribution_rank(
                        &self.finished_walks[turtle_walks_start..walks_end],
                        flow_index,
                    );
                    let distributed = if in_flow {
                        group.map_or(0.0, |group| group.offset(distribute, rank))
                    } else {
                        0.0
                    };
                    let main = delta
                        + if use_start {
                            start_dx
                        } else if role == RowAlignRole::Shiftable {
                            distributed
                        } else {
                            0.0
                        };
                    let cross = align.y * (inner_effective_height - outer_height).max(0.0);
                    let range_end = self.finished_walk_align_list_end(index);
                    self.move_align_list(range_start, range_end, main, cross, false);
                }
            }
            Flow::Right { wrap: true, .. } => {
                debug_assert!(!has_deferred);
                let mut row_start = turtle_walks_start;
                for row_index in turtle_rows_start..self.finished_rows.len() {
                    let row_end = self.finished_rows[row_index];
                    let group = (distribute != Distribute::Start)
                        .then(|| {
                            DistributionGroup::for_walks(
                                inner_width,
                                spacing,
                                &self.finished_walks[row_start..row_end],
                                Axis::Width,
                            )
                        })
                        .flatten();
                    let use_start = distribute == Distribute::Start
                        || (group.is_none()
                            && self.finished_walks[row_start..row_end]
                                .iter()
                                .any(|walk| walk.in_flow));
                    let start_dx = if use_start {
                        row_align_x_shift(
                            align.x,
                            inner_width,
                            spacing,
                            &self.finished_walks[row_start..row_end],
                        )
                    } else {
                        0.0
                    };
                    for index in row_start..row_end {
                        let (range_start, in_flow, flow_index, role) = {
                            let walk = &self.finished_walks[index];
                            (
                                walk.align_list_start,
                                walk.in_flow,
                                walk.flow_index,
                                walk.align_role,
                            )
                        };
                        let rank = distribution_rank(
                            &self.finished_walks[row_start..row_end],
                            flow_index,
                        );
                        let dx = if use_start {
                            start_dx
                        } else if in_flow && role == RowAlignRole::Shiftable {
                            group.map_or(0.0, |group| group.offset(distribute, rank))
                        } else {
                            0.0
                        };
                        let range_end = self.finished_walk_align_list_end(index);
                        self.move_align_list(range_start, range_end, dx, 0.0, false);
                    }
                    row_start = row_end;
                }
            }
            Flow::Down => {
                let walks_end = self.finished_walks.len();
                let group = (distribute != Distribute::Start)
                    .then(|| {
                        DistributionGroup::for_walks(
                            inner_height,
                            spacing,
                            &self.finished_walks[turtle_walks_start..walks_end],
                            Axis::Height,
                        )
                    })
                    .flatten();
                let use_start = distribute == Distribute::Start
                    || (group.is_none()
                        && self.finished_walks[turtle_walks_start..walks_end]
                            .iter()
                            .any(|walk| walk.in_flow));
                let start_dy = if use_start {
                    if has_deferred {
                        col_align_y_shift(
                            align.y,
                            inner_height,
                            spacing,
                            &self.finished_walks[turtle_walks_start..walks_end],
                        )
                    } else {
                        align.y * unused_inner_height
                    }
                } else {
                    0.0
                };

                for index in turtle_walks_start..walks_end {
                    let (range_start, outer_width, in_flow, flow_index, role, delta) = {
                        let walk = &self.finished_walks[index];
                        (
                            walk.align_list_start,
                            walk.outer_size.x,
                            walk.in_flow,
                            walk.flow_index,
                            walk.align_role,
                            self.turtle()
                                .total_resolved_delta_to(walk.deferred_before_count),
                        )
                    };
                    let rank = distribution_rank(
                        &self.finished_walks[turtle_walks_start..walks_end],
                        flow_index,
                    );
                    let distributed = if in_flow {
                        group.map_or(0.0, |group| group.offset(distribute, rank))
                    } else {
                        0.0
                    };
                    let main = delta
                        + if use_start {
                            start_dy
                        } else if role == RowAlignRole::Shiftable {
                            distributed
                        } else {
                            0.0
                        };
                    let cross = align.x * (inner_effective_width - outer_width).max(0.0);
                    let range_end = self.finished_walk_align_list_end(index);
                    self.move_align_list(range_start, range_end, cross, main, false);
                }
            }
            Flow::Overlay => {
                for index in turtle_walks_start..self.finished_walks.len() {
                    let walk = &self.finished_walks[index];
                    let dx = align.x * (inner_effective_width - walk.outer_size.x).max(0.0);
                    let dy = align.y * (inner_effective_height - walk.outer_size.y).max(0.0);
                    let range_start = walk.align_list_start;
                    let range_end = self.finished_walk_align_list_end(index);
                    self.move_align_list(range_start, range_end, dx, dy, false);
                }
            }
        }

        // Exploded z-layer view: give this scope a visible frame. Emitted here
        // — after the scope's own contents are aligned, before the parent's
        // pass — so it sits inside this turtle's align range and rides every
        // shift the parent later applies to the whole walk.
        self.draw_sploded_hairline(scope_rect);

        self.align_list.push(AlignEntry::EndClip);
        self.finished_rows.truncate(turtle_rows_start);
        self.finished_walks.truncate(turtle_walks_start);
        let turtle = self.turtles.pop().unwrap();

        if self.turtles.is_empty() {
            Rect {
                pos: dvec2(0.0, 0.0),
                size: turtle.size(),
            }
        } else {
            self.walk_turtle_internal(
                Walk {
                    abs_pos: turtle.walk().abs_pos,
                    width: Size::Fixed(turtle.width()),
                    height: Size::Fixed(turtle.height()),
                    ..turtle.walk()
                },
                turtle_align_start,
            )
        }
    }

    pub fn compute_final_size(&mut self) {
        let walk = self.turtles.last().unwrap().walk;
        let width_bounds = self.content_bounds(&walk, Axis::Width, ResolveAt::AtClose);
        let height_bounds = self.content_bounds(&walk, Axis::Height, ResolveAt::AtClose);

        if self.turtles.last().unwrap().width.is_nan() {
            let natural = {
                let turtle = self.turtles.last().unwrap();
                turtle.used_width() + turtle.padding().right
            };
            let turtle = self.turtles.last_mut().unwrap();
            turtle.width = width_bounds.clamp(natural);
            if let AlignEntry::BeginClip(clip_min, clip_max) =
                &mut self.align_list[turtle.align_start]
            {
                clip_max.x = clip_min.x + turtle.width();
            }
        }

        if self.turtles.last().unwrap().height.is_nan() {
            let natural = {
                let turtle = self.turtles.last().unwrap();
                turtle.used_height() + turtle.padding().bottom
            };
            let turtle = self.turtles.last_mut().unwrap();
            // `ContentBounds::clamp` also floors at zero. This deliberately
            // fixes the historical negative-height path when a margin-box max
            // is smaller than the vertical margins.
            turtle.height = height_bounds.clamp(natural);
            if let AlignEntry::BeginClip(clip_min, clip_max) =
                &mut self.align_list[turtle.align_start]
            {
                clip_max.y = clip_min.y + turtle.height();
            }
        }
    }

    /// Computes the maximum available height for the current turtle by walking up
    /// the ancestor turtle stack and evaluating any `Fit { max }` constraints.
    ///
    /// Returns the tightest (smallest) max height found, accounting for padding
    /// consumed by each ancestor layer. Returns `f64::MAX` if no ancestor has a
    /// max constraint.
    ///
    /// This is useful for widgets like TextInput that need to know when to start
    /// scrolling, even when their own walk height is unbounded `Fit`.
    pub fn compute_max_height_from_ancestors(&self) -> f64 {
        let mut max_height = f64::MAX;
        let current = self.turtles.last().unwrap();
        let mut consumed_padding = current.padding().height();

        // Walk ancestors (skip self)
        for ancestor_index in (0..self.turtles.len().saturating_sub(1)).rev() {
            let ancestor = &self.turtles[ancestor_index];
            if let Some(ancestor_max) = self
                .content_bounds_for_turtle(&ancestor.walk, Axis::Height, ancestor_index)
                .max
            {
                let available = ancestor_max - consumed_padding;
                max_height = max_height.min(available);
            }

            // If this ancestor has a known (non-NaN) height, it already constrains
            // children via layout, so we don't need to look further up.
            if !ancestor.height().is_nan() {
                let available =
                    ancestor.inner_height() - consumed_padding + current.padding().height();
                max_height = max_height.min(available);
                break;
            }

            consumed_padding += ancestor.padding().height();
        }
        max_height
    }

    /// Walks up the turtle stack looking for the tightest `Fit { max }` width
    /// constraint on any ancestor, accounting for padding/spacing consumed by
    /// each ancestor layer. Returns `f64::MAX` if no ancestor has a max constraint.
    ///
    /// This is useful for widgets like TextInput that need to know when to start
    /// horizontal scrolling, even when their own walk width is unbounded `Fit`.
    pub fn compute_max_width_from_ancestors(&self) -> f64 {
        let mut max_width = f64::MAX;
        let current = self.turtles.last().unwrap();
        let mut consumed_padding = current.padding().width();

        // Walk ancestors (skip self)
        for ancestor_index in (0..self.turtles.len().saturating_sub(1)).rev() {
            let ancestor = &self.turtles[ancestor_index];
            if let Some(ancestor_max) = self
                .content_bounds_for_turtle(&ancestor.walk, Axis::Width, ancestor_index)
                .max
            {
                let available = ancestor_max - consumed_padding;
                max_width = max_width.min(available);
            }

            if !ancestor.width().is_nan() {
                let available =
                    ancestor.inner_width() - consumed_padding + current.padding().width();
                max_width = max_width.min(available);
                break;
            }

            consumed_padding += ancestor.padding().width();
        }
        max_width
    }

    /// Pushes a clip rect entry and returns the index of that entry in the align list.
    /// The clip can later be modified via [`update_clip_rect_at`].
    pub fn push_clip_rect_tracked(&mut self, rect: Rect) -> usize {
        let index = self.align_list.len();
        self.align_list
            .push(AlignEntry::BeginClip(rect.pos, rect.pos + rect.size));
        index
    }

    /// Updates a previously pushed clip rect at the given align list index.
    pub fn update_clip_rect_at(&mut self, index: usize, rect: Rect) {
        if let AlignEntry::BeginClip(clip_min, clip_max) = &mut self.align_list[index] {
            *clip_min = rect.pos;
            *clip_max = rect.pos + rect.size;
        }
    }

    // Returns the end of the align list of the finished walk with the given index.
    fn finished_walk_align_list_end(&self, index: usize) -> usize {
        if index + 1 < self.finished_walks.len() {
            self.finished_walks[index + 1].align_list_start
        } else {
            self.align_list.len()
        }
    }

    /// Walks the turtle with the given `walk` to allocate space on the screen.
    ///
    /// Each walk produces a rectangle that represents the area allocated by the walk.
    pub fn walk_turtle(&mut self, walk: Walk) -> Rect {
        self.walk_turtle_internal(walk, self.align_list.len())
    }

    fn walk_turtle_internal(&mut self, walk: Walk, align_list_start: usize) -> Rect {
        let mut walk = self.resolve_walk(walk, ResolveAt::BeforeBegin);

        // A wrapping Fill is intentionally a one-pass row fill, not part of
        // the multi-item flex solver. Keep it on the current row when its
        // minimum outer width fits; otherwise start fresh and use that row's
        // full available width.
        if matches!(self.turtle().flow(), Flow::Right { wrap: true, .. })
            && walk.abs_pos.is_none()
        {
            if let Size::Fill { min, max, .. } = walk.width {
                let bounds = ContentBounds::from_fill(min, max, walk.margin.width());
                let spacing = self.turtle_next_walk_offset().x;
                let remaining = self.turtle().unused_inner_width_for_current_row() - spacing;
                let minimum_outer = bounds
                    .min
                    .map_or(walk.margin.width(), |min| min + walk.margin.width());
                if self.turtle_current_row_has_in_flow_walks() && minimum_outer > remaining {
                    self.wrap_turtle(align_list_start);
                }

                let spacing = self.turtle_next_walk_offset().x;
                let mut outer = self.turtle().unused_inner_width_for_current_row() - spacing;
                if !outer.is_finite() {
                    outer = minimum_outer;
                }
                walk.width = Size::Fixed(bounds.clamp(outer - walk.margin.width()));
                // Main-axis Fill is definite now, so transfer aspect to a Fit
                // cross axis through the ordinary foundation resolver.
                walk = self.resolve_walk(walk, ResolveAt::BeforeBegin);
            }
        }
        let flow_spacing = self.turtle_next_walk_offset();
        let in_flow = walk.abs_pos.is_none() || walk.deferred;
        let flow_index = if walk.deferred {
            walk.flow_index
        } else if in_flow {
            let turtle = self.turtles.last_mut().unwrap();
            let index = turtle.next_flow_index;
            turtle.next_flow_index = turtle.next_flow_index.wrapping_add(1);
            index
        } else {
            u32::MAX
        };
        let current_row_has_in_flow_walks = self.turtle_current_row_has_in_flow_walks();
        let turtle = self.turtles.last_mut().unwrap();

        let size = turtle.next_walk_size(walk.width, walk.height, walk.margin);
        let outer_size = size + walk.margin.size();

        if let Some(outer_origin) = walk.abs_pos {
            let old_pos = turtle.pos();

            turtle.move_to(outer_origin);

            match turtle.flow() {
                Flow::Right { .. } => turtle.allocate_height(outer_size.y),
                Flow::Down => turtle.allocate_width(outer_size.x),
                Flow::Overlay => turtle.allocate_size(outer_size),
            }

            turtle.move_to(old_pos);

            self.finished_walks.push(FinishedWalk {
                align_list_start,
                deferred_before_count: 0,
                outer_size: size + walk.margin.size(),
                in_flow,
                flow_index,
                metrics: walk.metrics,
                align_role: RowAlignRole::Shiftable,
                align_height: None,
            });

            let origin = outer_origin + walk.margin.left_top();
            Rect { pos: origin, size }
        } else {
            let spacing = flow_spacing;
            let turtle = self.turtles.last_mut().unwrap();

            let outer_origin = match turtle.flow() {
                Flow::Right { wrap: true, .. }
                    if current_row_has_in_flow_walks
                        && spacing.x + outer_size.x > turtle.unused_inner_width_for_current_row() =>
                {
                    self.wrap_turtle(align_list_start);
                    let turtle = self.turtles.last_mut().unwrap();

                    let outer_origin = turtle.pos();
                    turtle.allocate_size(outer_size);
                    turtle.move_right(outer_size.x);
                    outer_origin
                }
                Flow::Right { .. } => {
                    turtle.move_right(spacing.x);
                    let outer_origin = turtle.pos();
                    turtle.allocate_size(outer_size);
                    turtle.move_right(outer_size.x);
                    outer_origin
                }

                Flow::Down => {
                    turtle.move_down(spacing.y);
                    let outer_origin = turtle.pos();
                    turtle.allocate_size(outer_size);
                    turtle.move_down(outer_size.y);
                    outer_origin
                }
                Flow::Overlay => {
                    let outer_origin = turtle.pos();
                    turtle.allocate_size(outer_size);
                    outer_origin
                }
            };


            let defer_index = self.turtle().deferred_fills.len();
            self.turtle_mut().current_row_metrics =
                self.turtle().current_row_metrics.max(walk.metrics);
            self.finished_walks.push(FinishedWalk {
                align_list_start,
                deferred_before_count: defer_index,
                outer_size,
                in_flow: true,
                flow_index,
                metrics: walk.metrics,
                align_role: RowAlignRole::Shiftable,
                align_height: None,
            });

            let origin = outer_origin + walk.margin.left_top();
            Rect { pos: origin, size }
        }
    }

    /// Defers walking the turtle with the given `Walk`.
    pub fn defer_walk_turtle(&mut self, walk: Walk) -> Option<DeferredWalk> {
        let mut walk = self.resolve_walk(walk, ResolveAt::BeforeBegin);
        if walk.abs_pos.is_some() {
            return None;
        }

        match self.turtle().flow() {
            Flow::Right { wrap: false, .. } => {
                let Size::Fill {
                    weight,
                    basis,
                    shrink,
                    ..
                } = walk.width
                else {
                    return None;
                };

                debug_assert!(
                    !self.turtle().flex_resolved,
                    "cannot defer another fill after flex resolution"
                );
                if self.turtle().flex_resolved {
                    return None;
                }

                let spacing = self.turtle_next_walk_offset();
                walk.flow_index = self.turtle().next_flow_index;
                self.turtle_mut().next_flow_index = walk.flow_index.wrapping_add(1);

                let bounds = self.content_bounds(&walk, Axis::Width, ResolveAt::BeforeBegin);
                let unclamped_basis = self
                    .eval_fit_bound(basis, Axis::Width, ResolveAt::BeforeBegin)
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
                let basis = bounds.clamp(unclamped_basis);

                let old_pos = self.turtle().pos();

                let turtle = self.turtles.last_mut().unwrap();
                let size = dvec2(basis, turtle.next_walk_height(walk.height, walk.margin));
                let outer_size = size + walk.margin.size();

                turtle.move_right(spacing.x);
                turtle.allocate_size(outer_size);
                turtle.move_right(outer_size.x);

                let index = turtle.deferred_fills.len();
                turtle.push_deferred_fill(DeferredFill {
                    grow: weight,
                    shrink,
                    unclamped_basis,
                    basis,
                    min: bounds.min,
                    max: bounds.max,
                    delta: 0.0,
                    frozen: false,
                });

                Some(DeferredWalk::Unresolved {
                    index,
                    pos: old_pos + spacing,
                    walk,
                })
            }
            Flow::Down => {
                let Size::Fill {
                    weight,
                    basis,
                    shrink,
                    ..
                } = walk.height
                else {
                    return None;
                };

                debug_assert!(
                    !self.turtle().flex_resolved,
                    "cannot defer another fill after flex resolution"
                );
                if self.turtle().flex_resolved {
                    return None;
                }

                let spacing = self.turtle_next_walk_offset();
                walk.flow_index = self.turtle().next_flow_index;
                self.turtle_mut().next_flow_index = walk.flow_index.wrapping_add(1);

                let bounds = self.content_bounds(&walk, Axis::Height, ResolveAt::BeforeBegin);
                let unclamped_basis = self
                    .eval_fit_bound(basis, Axis::Height, ResolveAt::BeforeBegin)
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
                let basis = bounds.clamp(unclamped_basis);

                let old_pos = self.turtle().pos();

                let turtle = self.turtles.last_mut().unwrap();
                let size = dvec2(turtle.next_walk_width(walk.width, walk.margin), basis);
                let outer_size = size + walk.margin.size();

                turtle.move_down(spacing.y);
                turtle.allocate_size(outer_size);
                turtle.move_down(outer_size.y);

                let index = turtle.deferred_fills.len();
                turtle.push_deferred_fill(DeferredFill {
                    grow: weight,
                    shrink,
                    unclamped_basis,
                    basis,
                    min: bounds.min,
                    max: bounds.max,
                    delta: 0.0,
                    frozen: false,
                });

                Some(DeferredWalk::Unresolved {
                    index,
                    pos: old_pos + spacing,
                    walk,
                })
            }
            Flow::Right { wrap: true, .. } if walk.width.is_fill() => {
                None
            }
            _ => None,
        }
    }

    pub fn end_pass_sized_turtle_no_clip(&mut self) {
        let turtle = self.turtles.pop().unwrap();

        self.clip_and_shift_align_list(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::SkipTurtle {
            skip: self.align_list.len(),
        };
        self.finished_walks.truncate(turtle.finished_walks_start);
    }

    pub fn end_pass_sized_turtle(&mut self) {
        let turtle = self.turtles.pop().unwrap();
        // lets perform clipping on our alignlist.
        self.align_list.push(AlignEntry::EndClip);

        self.clip_and_shift_align_list(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::SkipTurtle {
            skip: self.align_list.len(),
        };
        self.finished_walks.truncate(turtle.finished_walks_start);
    }

    pub fn end_pass_sized_turtle_with_shift(&mut self, area: Area, shift: Vec2d) {
        let turtle = self.turtles.pop().unwrap();
        // lets perform clipping on our alignlist.
        self.align_list.push(AlignEntry::EndClip);

        self.clip_and_shift_align_list(turtle.align_start, self.align_list.len());
        //log!("{:?}", self.align_list[turtle.align_start]);
        self.align_list[turtle.align_start] = AlignEntry::ShiftTurtle {
            area,
            shift,
            skip: self.align_list.len(),
        };
        self.finished_walks.truncate(turtle.finished_walks_start);
    }

    pub fn turtle_has_align_items(&mut self) -> bool {
        self.align_list.len() != self.turtle().align_start + 1
    }

    pub fn end_turtle_with_area(&mut self, area: &mut Area) -> Rect {
        let rect = self.end_turtle_with_guard(Area::Empty);
        self.add_aligned_rect_area(area, rect);
        rect
    }

    pub fn set_turtle_wrap_spacing(&mut self, spacing: f64) {
        self.turtle_mut().wrap_spacing = spacing;
    }

    pub fn walk_turtle_with_area(&mut self, area: &mut Area, walk: Walk) -> Rect {
        let rect = self.walk_turtle_internal(walk, self.align_list.len());
        self.add_aligned_rect_area(area, rect);
        rect
    }

    pub fn peek_walk_turtle(&self, walk: Walk) -> Rect {
        self.walk_turtle_peek(walk)
    }

    pub fn walk_turtle_would_be_visible(&mut self, walk: Walk) -> bool {
        let rect = self.walk_turtle_peek(walk);
        self.turtle().rect_is_visible(rect)
    }

    pub fn peek_walk_pos(&self, walk: Walk) -> Vec2d {
        let walk = self.resolve_walk(walk, ResolveAt::BeforeBegin);
        if let Some(pos) = walk.abs_pos {
            pos + walk.margin.left_top()
        } else {
            let turtle = self.turtles.last().unwrap();
            turtle.pos + walk.margin.left_top()
        }
    }

    /// Returns the current length of the turtle's align list. Call this
    /// BEFORE drawing something you later intend to register with
    /// [`emit_turtle_walk`], and pass the captured value as that call's
    /// `align_list_start` argument.
    pub fn align_list_len(&self) -> usize {
        self.align_list.len()
    }

    /// Records that the current turtle has emitted a `Rect`-shaped walk.
    ///
    /// `align_list_start` must be the value of [`Cx2d::align_list_len`]
    /// captured BEFORE the caller added any align entries for this walk. This
    /// mirrors how `walk_turtle_internal` records `align_list_start`, and lets
    /// `finish_row`'s row-alignment passes (e.g., `RowAlign::Center`) correctly
    /// identify and shift the walk's own align entries.
    pub fn emit_turtle_walk(&mut self, rect: Rect, align_list_start: usize) {
        self.emit_turtle_walk_with_metrics(rect, align_list_start, Metrics::default())
    }

    /// Like [`emit_turtle_walk`] but lets the caller specify the walk's
    /// `Metrics` (descender/line_gap/line_scale) for baseline-aware row
    /// alignment (`RowAlign::Bottom`).
    pub fn emit_turtle_walk_with_metrics(
        &mut self,
        rect: Rect,
        align_list_start: usize,
        metrics: Metrics,
    ) {
        self.emit_turtle_walk_with_align_height(rect, align_list_start, metrics, None)
    }

    /// Like [`emit_turtle_walk_with_metrics`] but also sets the walk's
    /// centering height for `RowAlign::Center` (see `FinishedWalk::align_height`).
    pub fn emit_turtle_walk_with_align_height(
        &mut self,
        rect: Rect,
        align_list_start: usize,
        metrics: Metrics,
        align_height: Option<f64>,
    ) {
        self.emit_turtle_walk_with_role(rect, align_list_start, metrics, align_height, RowAlignRole::Shiftable)
    }

    /// Like [`emit_turtle_walk_with_align_height`] but with an explicit
    /// [`RowAlignRole`].
    pub fn emit_turtle_walk_with_role(
        &mut self,
        rect: Rect,
        align_list_start: usize,
        metrics: Metrics,
        align_height: Option<f64>,
        align_role: RowAlignRole,
    ) {
        let turtle = self.turtles.last_mut().unwrap();
        let flow_index = turtle.next_flow_index;
        turtle.next_flow_index = turtle.next_flow_index.wrapping_add(1);
        self.finished_walks.push(FinishedWalk {
            align_list_start,
            deferred_before_count: turtle.deferred_fills.len(),
            outer_size: rect.size,
            in_flow: true,
            flow_index,
            metrics,
            align_role,
            align_height,
        });
    }

    fn walk_turtle_peek(&self, walk: Walk) -> Rect {
        if self.turtles.is_empty() {
            return Rect::default();
        }
        let walk = self.resolve_walk(walk, ResolveAt::BeforeBegin);
        let turtle = self.turtles.last().unwrap();
        let size = dvec2(
            turtle.next_walk_width(walk.width, walk.margin),
            turtle.next_walk_height(walk.height, walk.margin),
        );

        if let Some(pos) = walk.abs_pos {
            Rect {
                pos: pos + walk.margin.left_top(),
                size,
            }
        } else {
            let spacing = self.turtle_next_walk_offset();
            let pos = turtle.pos;
            Rect {
                pos: pos + walk.margin.left_top() + spacing,
                size,
            }
        }
    }

    fn wrap_turtle(&mut self, align_list_start: usize) {
        let old_pos = self.turtle().pos() + self.turtle_next_walk_offset();
        self.turtle_new_line_internal(self.turtle().wrap_spacing, align_list_start);
        let new_pos = self.turtle().pos();
        let shift = new_pos - old_pos;
        self.move_align_list(
            align_list_start,
            self.align_list.len(),
            shift.x,
            shift.y,
            false,
        );
    }

    pub fn turtle_new_line(&mut self) {
        self.turtle_new_line_with_spacing(0.0);
    }

    pub fn turtle_new_line_with_spacing(&mut self, spacing: f64) {
        self.turtle_new_line_internal(spacing, self.align_list.len());
    }

    pub fn turtle_new_line_internal(&mut self, spacing: f64, align_list_start: usize) {
        let row_bottom_forgiveness = self.finish_row(align_list_start);
        if row_bottom_forgiveness > 0.0 {
            // An anchored row's up-centered walks overhang the row's anchor
            // symmetrically, and the top overhang already intrudes into the
            // gap above the row; forgiving the same amount below it keeps the
            // gaps on both sides of the row equal. The reduction happens only
            // on the new-line path — a turtle's final row is finished by
            // `end_turtle_with_guard`, which discards the forgiveness — so a
            // turtle's reported height always covers its last row's full
            // physical extent.
            let used_width = self.turtle().used_width();
            let reduced_used_height = self.turtle().used_height() - row_bottom_forgiveness;
            self.turtle_mut().set_used(used_width, reduced_used_height);
        }
        let new_pos = dvec2(
            self.turtle().origin.x + self.turtle().padding().left,
            self.turtle().origin.y + self.turtle().used_height() + spacing,
        );
        self.turtle_mut().move_to(new_pos);
        self.turtle_mut().allocate_height(0.0);
    }

    /// Finishes the current row: applies its row alignment and rolls the row
    /// bookkeeping forward.
    ///
    /// Returns the row's bottom forgiveness (see [`Cx2d::finish_row_center`]);
    /// rows under `RowAlign::Top` and `RowAlign::Bottom` always return zero.
    fn finish_row(&mut self, align_list_start: usize) -> f64 {
        let row_align = if let Flow::Right { row_align, .. } = self.turtle().flow() {
            row_align
        } else {
            RowAlign::Top
        };

        let row_bottom_forgiveness = match row_align {
            RowAlign::Top => {
                // No per-walk shifts needed — items stay at the row top.
                0.0
            }
            RowAlign::Bottom => {
                self.finish_row_bottom(align_list_start);
                0.0
            }
            RowAlign::Center => self.finish_row_center(align_list_start),
        };

        self.turtle_mut().prev_row_metrics = self.turtle().current_row_metrics;
        self.turtle_mut().current_row_metrics = Metrics::default();
        self.finished_rows.push(self.finished_walks.len());
        row_bottom_forgiveness
    }

    /// Baseline-aligns every finished walk in the current row so that its
    /// descender sits on the row's baseline. Requires each walk's
    /// `metrics.descender` to describe the distance from its bottom to its
    /// internal baseline.
    fn finish_row_bottom(&mut self, align_list_start: usize) {
        let current_row_height = self.turtle().row_height();
        let current_row_metrics = self.turtle().current_row_metrics;

        // We're going to push down each finished walk for the current row so that their
        // baseline aligns with the bottom of the current row. Therefore, the height of the
        // ascender of the current row will be the height of the current row, minus the height
        // of the descender of the current row.
        let current_row_ascender = current_row_height - current_row_metrics.descender;

        // If the current row is not the first row, compute the amount by which we have to shift
        // each finished walk for the current row so that the actual spacing between the
        // baseline of the previous and the current row is equal to the desired spacing.
        let line_spacing_shift = if self.turtle_is_at_first_row() {
            0.0
        } else {
            // After we've pushed down each finished walk for the current row so that their
            // baseline aligns with the bottom of the current row, the actual spacing
            // between the baseline of the previous and current row will be the height of
            // the descender of the previous row, plus the height of the current row.
            let prev_row_metrics = self.turtle().prev_row_metrics;
            let actual_line_spacing = prev_row_metrics.descender + current_row_height;

            // The desired spacing between the baseline of the previous and current row is
            // the sum of the height of the descender and line gap of the previous row, and
            // the ascender of the current row, scaled up by the line scale of the current
            // row.
            let desired_line_spacing =
                (prev_row_metrics.descender + prev_row_metrics.line_gap + current_row_ascender)
                    * current_row_metrics.line_scale;

            // The amount by which we have to shift each finished walk is the difference between
            // the desired and the actual spacing.
            desired_line_spacing - actual_line_spacing
        };

        // Update the height of the row to account for the shifts we're about to do.
        self.turtle_mut().used_height += current_row_metrics.descender + line_spacing_shift;

        let finished_walks_start = self.current_row_walks_start();
        let finished_walks_end = self.finished_walks.len();
        for finished_walk_index in finished_walks_start..finished_walks_end {
            // Immovable walks (a wrapped run's rows — their glyphs live in one
            // shared batch) cannot be baseline-shifted either.
            if self.finished_walks[finished_walk_index].align_role != RowAlignRole::Shiftable {
                continue;
            }
            let finished_walk_height = self.finished_walks[finished_walk_index].outer_size.y;
            let finished_walk_metrics = self.finished_walks[finished_walk_index].metrics;

            // The amount by which we have to shift the current finished walk so that its
            // descender aligns with the bottom of the current row.
            let descender_shift = current_row_height - finished_walk_height;

            // The amount by which we have to shift the current finished walk so that its
            // baseline aligns with the bottom of the current row.
            let baseline_shift = finished_walk_metrics.descender;

            // The total amount by which we have to shift the current finished walk.
            let shift = descender_shift + baseline_shift + line_spacing_shift;


            let start = self.finished_walks[finished_walk_index].align_list_start;
            let end = if finished_walk_index + 1 < self.finished_walks.len() {
                self.finished_walks[finished_walk_index + 1].align_list_start
            } else {
                align_list_start
            };
            self.move_align_list(start, end, 0.0, shift, false);
        }
    }

    /// Returns the `finished_walks` index where the current (unfinished) row's
    /// walks begin for the current turtle.
    ///
    /// * If this turtle has no finished rows yet, the current row's walks start
    ///   at `turtle.finished_walks_start`.
    /// * Otherwise, they start just after the last finished row of this turtle.
    ///   `finished_rows` holds cumulative `finished_walks.len()` snapshots at
    ///   each row boundary; nested turtles truncate their own entries when they
    ///   end, so `finished_rows.last()` is always the current turtle's most
    ///   recent row end (as long as this turtle has any).
    fn current_row_walks_start(&self) -> usize {
        if self.turtle().finished_rows_start == self.finished_rows.len() {
            self.turtle().finished_walks_start
        } else {
            // Safe: finished_rows.len() > finished_rows_start, so at least one
            // entry exists for this turtle and it's at the end of the vec.
            *self.finished_rows.last().unwrap()
        }
    }

    /// Vertically centers every finished walk in the current row on the row's
    /// vertical center line.
    ///
    /// Without an anchor walk, the row centers on its tallest walk: shifts are
    /// downward only and every walk stays entirely within the row's bounds, so
    /// `used_height` is untouched. With an anchor walk (an immovable wrapped-run
    /// row), every shiftable walk centers on the anchor's center line instead,
    /// so a walk taller than the anchor shifts UP and overhangs the anchor's
    /// box symmetrically — by equal amounts above and below it.
    ///
    /// An up-shift is clamped so that no walk's top rises above the turtle's
    /// own rectangle top: that edge is also the turtle's clip top, and content
    /// shifted above it renders with its top edge cut off. The clamp can only
    /// restrict an up-shift; it never turns one into a downward shift.
    ///
    /// Returns the row's bottom forgiveness: how far the row's allocated
    /// bottom extent hangs below the bottom of its anchor-centered content,
    /// capped at twice the largest up-shift actually applied. The new-line
    /// path subtracts this from the advance to the next row, so a centered
    /// walk's symmetric overhang intrudes equally into the row gaps above and
    /// below it instead of pushing the next row further down. Anchorless rows
    /// return zero, which leaves the advance untouched.
    fn finish_row_center(&mut self, align_list_start: usize) -> f64 {
        let current_row_height = self.turtle().row_height();

        let finished_walks_start = self.current_row_walks_start();
        let finished_walks_end = self.finished_walks.len();

        // An anchor walk stands for content that cannot be shifted, so the
        // row's center line is anchored to its center rather than the tallest
        // walk's. Shiftable walks then move toward that line in either
        // direction: a taller item (an inline pill beside a wrapped run's
        // text row) moves UP to center on the text. Without an anchor, the
        // row centers on its tallest walk and shifts are downward only.
        let mut anchor_center: Option<f64> = None;
        for finished_walk_index in finished_walks_start..finished_walks_end {
            let finished_walk = &self.finished_walks[finished_walk_index];
            if finished_walk.align_role == RowAlignRole::Anchor {
                let center = finished_walk
                    .align_height
                    .unwrap_or(finished_walk.outer_size.y)
                    * 0.5;
                anchor_center = Some(anchor_center.map_or(center, |c: f64| c.max(center)));
            }
        }


        // The largest post-shift "effective bottom" of any walk on this row,
        // relative to the row top: the walk's own box displaced by the shift
        // that was actually applied to it. An up-shifted walk's bottom rises
        // by exactly its shift, so the row's visual extent ends that much
        // above its allocation, and only that surplus may be forgiven.
        let mut max_effective_bottom: f64 = 0.0;
        // The largest upward shift actually applied on this row; it bounds
        // the returned forgiveness so that allocations no walk accounts for
        // (pre-allocated text row boxes, vertical margins) are never forgiven.
        let mut max_up_overhang: f64 = 0.0;

        for finished_walk_index in finished_walks_start..finished_walks_end {
            let finished_walk = &self.finished_walks[finished_walk_index];
            if finished_walk.align_role != RowAlignRole::Shiftable {
                max_effective_bottom = max_effective_bottom.max(finished_walk.outer_size.y);
                continue;
            }
            let finished_walk_height = finished_walk
                .align_height
                .unwrap_or(finished_walk.outer_size.y);
            let shift = match anchor_center {
                Some(center) => {
                    // The row top is the turtle's position while the row
                    // finishes; the walk's top after shifting is the row top
                    // plus the shift. A negative bound keeps the walk's top at
                    // or below the turtle's own rectangle top (its clip top);
                    // the `min(0.0)` keeps the bound from ever forcing a
                    // downward shift.
                    let min_shift =
                        (self.turtle().origin().y - self.turtle().pos().y).min(0.0);
                    (center - finished_walk_height * 0.5).max(min_shift)
                }
                None => (current_row_height - finished_walk_height) * 0.5,
            };

            let applied = !((anchor_center.is_none() && shift <= 0.0) || shift == 0.0);
            let applied_shift = if applied { shift } else { 0.0 };
            max_up_overhang = max_up_overhang.max((-applied_shift).max(0.0));
            max_effective_bottom =
                max_effective_bottom.max(finished_walk.outer_size.y + applied_shift);


            if !applied {
                continue;
            }

            let start = self.finished_walks[finished_walk_index].align_list_start;
            let end = if finished_walk_index + 1 < self.finished_walks.len() {
                self.finished_walks[finished_walk_index + 1].align_list_start
            } else {
                align_list_start
            };
            self.move_align_list(start, end, 0.0, shift, false);
        }

        if anchor_center.is_none() {
            return 0.0;
        }
        let row_bottom_forgiveness = (current_row_height - max_effective_bottom.max(0.0))
            .clamp(0.0, max_up_overhang);
        row_bottom_forgiveness
    }

    /// Shifts the rendered content in the align list range `[start, end)` by
    /// `(dx, dy)` logical pixels. This moves already-drawn instances and rect
    /// areas without changing the turtle's allocation.
    ///
    /// Use [`Cx2d::align_list_len`] to capture `start` before drawing and
    /// `end` after drawing, then call this to reposition the drawn content.
    pub fn shift_align_entries(&mut self, start: usize, end: usize, dx: f64, dy: f64) {
        self.move_align_list(start, end, dx, dy, false);
    }

    fn move_align_list(&mut self, start: usize, end: usize, dx: f64, dy: f64, shift_clip: bool) {
        debug_assert!(!dx.is_nan());
        debug_assert!(!dy.is_nan());

        let d = dvec2(dx, dy);
        let mut c = start;
        while c < end {
            let align_item = &mut self.align_list[c];
            match align_item {
                AlignEntry::Area(Area::Instance(inst)) => {
                    let draw_list = &mut self.cx.cx.draw_lists[inst.draw_list_id];
                    let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                    let draw_call = draw_item.draw_call().unwrap();
                    let sh = &self.cx.cx.draw_shaders[draw_call.draw_shader_id.index];
                    let inst_buf = draw_item.instances.as_mut().unwrap();
                    for i in 0..inst.instance_count {
                        if let Some(rect_pos) = sh.mapping.rect_pos {
                            inst_buf[(inst.instance_offset + rect_pos)
                                + i * sh.mapping.instances.total_slots] += dx as f32;
                            inst_buf[inst.instance_offset
                                + rect_pos
                                + 1
                                + i * sh.mapping.instances.total_slots] += dy as f32;
                            if shift_clip {
                                if let Some(draw_clip) = sh.mapping.draw_clip {
                                    inst_buf[(inst.instance_offset + draw_clip)
                                        + i * sh.mapping.instances.total_slots] += dx as f32;
                                    inst_buf[inst.instance_offset
                                        + draw_clip
                                        + 1
                                        + i * sh.mapping.instances.total_slots] += dy as f32;
                                    inst_buf[inst.instance_offset
                                        + draw_clip
                                        + 2
                                        + i * sh.mapping.instances.total_slots] += dx as f32;
                                    inst_buf[inst.instance_offset
                                        + draw_clip
                                        + 3
                                        + i * sh.mapping.instances.total_slots] += dy as f32;
                                }
                            }
                        }
                    }
                }
                AlignEntry::Area(Area::Rect(ra)) => {
                    let draw_list = &mut self.cx.draw_lists[ra.draw_list_id];
                    if draw_list.redraw_id == ra.redraw_id {
                        if let Some(rect_area) = draw_list.rect_areas.get_mut(ra.rect_id) {
                            rect_area.rect.pos += d;
                            if shift_clip {
                                rect_area.draw_clip.0 += d;
                                rect_area.draw_clip.1 += d;
                            }
                        }
                    }
                }
                AlignEntry::BeginClip(clip0, clip1) => {
                    *clip0 += d;
                    *clip1 += d;
                }
                AlignEntry::SkipTurtle { skip } | AlignEntry::ShiftTurtle { skip, .. } => {
                    c = *skip;
                    continue;
                }
                _ => (),
            }
            c += 1;
        }
    }

    fn clip_and_shift_align_list(&mut self, start: usize, end: usize) {
        self.turtle_clips.clear();
        let mut i = start;
        while i < end {
            let align_item = &self.align_list[i];
            match align_item {
                AlignEntry::SkipTurtle { skip } => {
                    i = *skip;
                    continue;
                }
                AlignEntry::ShiftTurtle { area, shift, skip } => {
                    let rect = area.rect(self);
                    let skip = *skip;
                    self.move_align_list(
                        i + 1,
                        skip,
                        rect.pos.x + shift.x,
                        rect.pos.y + shift.y,
                        true,
                    );
                    i = skip;
                    continue;
                }
                AlignEntry::BeginClip(clip0, clip1) => {
                    if let Some((tclip0, tclip1)) = self.turtle_clips.last() {
                        self.turtle_clips.push((
                            dvec2(clip0.x.max(tclip0.x), clip0.y.max(tclip0.y)),
                            dvec2(clip1.x.min(tclip1.x), clip1.y.min(tclip1.y)),
                        ));
                    } else {
                        self.turtle_clips.push((*clip0, *clip1));
                    }
                }
                AlignEntry::EndClip => {
                    self.turtle_clips.pop().unwrap();
                }
                AlignEntry::Area(Area::Instance(inst)) => {
                    if let Some((clip0, clip1)) = self.turtle_clips.last() {
                        let draw_list = &mut self.cx.cx.draw_lists[inst.draw_list_id];
                        let draw_item = &mut draw_list.draw_items[inst.draw_item_id];
                        let draw_call = draw_item.draw_call().unwrap();
                        let sh = &self.cx.cx.draw_shaders[draw_call.draw_shader_id.index];
                        let inst_buf = draw_item.instances.as_mut().unwrap();
                        for i in 0..inst.instance_count {
                            if let Some(draw_clip) = sh.mapping.draw_clip {
                                inst_buf[(inst.instance_offset + draw_clip)
                                    + i * sh.mapping.instances.total_slots] = clip0.x as f32;
                                inst_buf[inst.instance_offset
                                    + draw_clip
                                    + 1
                                    + i * sh.mapping.instances.total_slots] = clip0.y as f32;
                                inst_buf[inst.instance_offset
                                    + draw_clip
                                    + 2
                                    + i * sh.mapping.instances.total_slots] = clip1.x as f32;
                                inst_buf[inst.instance_offset
                                    + draw_clip
                                    + 3
                                    + i * sh.mapping.instances.total_slots] = clip1.y as f32;
                            }
                        }
                    }
                }
                AlignEntry::Area(Area::Rect(ra)) => {
                    if let Some((clip0, clip1)) = self.turtle_clips.last() {
                        let draw_list = &mut self.cx.draw_lists[ra.draw_list_id];
                        if draw_list.redraw_id == ra.redraw_id {
                            if let Some(rect_area) = draw_list.rect_areas.get_mut(ra.rect_id) {
                                rect_area.draw_clip.0 = *clip0;
                                rect_area.draw_clip.1 = *clip1;
                            }
                        }
                    }
                }
                AlignEntry::Unset => {}
                AlignEntry::Area(_) => {}
            }
            i += 1;
        }
    }

    pub fn get_turtle_align_range(&self) -> TurtleAlignRange {
        TurtleAlignRange {
            start: self.turtles.last().unwrap().align_start,
            end: self.align_list.len(),
        }
    }

    pub fn shift_align_range(&mut self, range: &TurtleAlignRange, shift: Vec2d) {
        self.move_align_list(range.start, range.end, shift.x, shift.y, true);
    }

    pub fn add_rect_area(&mut self, area: &mut Area, rect: Rect) {
        //let turtle = self.turtle();
        self.add_aligned_rect_area(area, rect)
    }

    /// Push a clip rectangle onto the clip stack. All draw calls until the
    /// matching `pop_clip_rect` will have their GPU `draw_clip` intersected
    /// with this rect. Must be balanced with `pop_clip_rect`.
    pub fn push_clip_rect(&mut self, rect: Rect) {
        self.align_list
            .push(AlignEntry::BeginClip(rect.pos, rect.pos + rect.size));
    }

    /// Pop a clip rectangle previously pushed by `push_clip_rect`.
    pub fn pop_clip_rect(&mut self) {
        self.align_list.push(AlignEntry::EndClip);
    }
}

pub struct TurtleAlignRange {
    pub start: usize,
    pub end: usize,
}

impl Turtle {
    /// Returns the y-position of the current row.
    pub fn row_y(&self) -> f64 {
        self.pos().y - self.origin().y
    }

    /// Returns the height of the current row so far.
    ///
    /// This is the used height of the turtle's rectangle so far, minus the y-position of the
    /// current row.
    pub fn row_height(&self) -> f64 {
        self.used_height - self.row_y()
    }

    /// Returns the offset to the next row.
    ///
    /// This is the height of the current row so far, plus the wrap spacing.
    pub fn next_row_offset(&self) -> f64 {
        self.row_height() + self.wrap_spacing
    }

    pub fn used(&self) -> Vec2d {
        dvec2(self.used_width, self.used_height)
    }

    pub fn set_used(&mut self, width_used: f64, height_used: f64) {
        self.used_width = width_used;
        self.used_height = height_used;
    }

    /// Returns the current wrap spacing, which is the extra vertical space
    /// added between rows when text wraps (based on the line spacing of the
    /// most recently drawn text).
    pub fn wrap_spacing(&self) -> f64 {
        self.wrap_spacing
    }

    pub fn set_wrap_spacing(&mut self, value: f64) {
        self.wrap_spacing = self.wrap_spacing.max(value);
    }

    pub fn rect_is_visible(&self, geom: Rect) -> bool {
        let view = Rect {
            pos: self.origin + self.layout.scroll,
            size: dvec2(self.width, self.height),
        };
        view.intersects(geom)
    }

    pub fn rel_pos(&self) -> Vec2d {
        Vec2d {
            x: self.pos.x - self.origin.x,
            y: self.pos.y - self.origin.y,
        }
    }

    pub fn rel_pos_padded(&self) -> Vec2d {
        Vec2d {
            x: self.pos.x - self.origin.x - self.layout.padding.left,
            y: self.pos.y - self.origin.y - self.layout.padding.top,
        }
    }

    pub fn pos(&self) -> Vec2d {
        self.pos
    }

    pub fn scroll(&self) -> Vec2d {
        self.layout.scroll
    }

    pub fn max_width(&self, walk: Walk) -> Option<f64> {
        if walk.width.is_fit() {
            return None;
        }
        let width = self.next_walk_width(walk.width, walk.margin);
        width.is_finite().then_some(width)
    }

    pub fn max_height(&self, walk: Walk) -> Option<f64> {
        if walk.height.is_fit() {
            return None;
        }
        let height = self.next_walk_height(walk.height, walk.margin);
        height.is_finite().then_some(height)
    }
}

impl Walk {
    pub fn abs_rect(rect: Rect) -> Self {
        Self {
            abs_pos: Some(rect.pos),
            width: Size::Fixed(rect.size.x),
            height: Size::Fixed(rect.size.y),
            ..Self::default()
        }
    }

    pub fn with_abs_pos(mut self, v: Vec2d) -> Self {
        self.abs_pos = Some(v);
        self
    }
    pub fn with_margin_all(mut self, v: f64) -> Self {
        self.margin = Inset {
            left: v,
            right: v,
            top: v,
            bottom: v,
        };
        self
    }

    pub fn with_add_padding(mut self, v: Inset) -> Self {
        self.margin.top += v.top;
        self.margin.left += v.left;
        self.margin.right += v.right;
        self.margin.bottom += v.bottom;
        self
    }
}

impl Layout {
    pub fn with_scroll(mut self, v: Vec2d) -> Self {
        self.scroll = v;
        self
    }

    pub fn with_align(mut self, v: Align) -> Self {
        self.align = v;
        self
    }

    pub fn with_align_x(mut self, v: f64) -> Self {
        self.align.x = v;
        self
    }

    pub fn with_align_y(mut self, v: f64) -> Self {
        self.align.y = v;
        self
    }

    pub fn with_clip(mut self, clip_x: bool, clip_y: bool) -> Self {
        self.clip_x = clip_x;
        self.clip_y = clip_y;
        self
    }

    pub fn with_padding_all(mut self, v: f64) -> Self {
        self.padding = Inset {
            left: v,
            right: v,
            top: v,
            bottom: v,
        };
        self
    }
}
/*
impl LiveHook for Flow {
    fn skip_apply(&mut self, _cx: &mut Cx, _apply: &Apply, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::BareEnum(live_id!(Right))=>{
                *self = Self::right();
                Some(index + 1)
            }
            LiveValue::BareEnum(live_id!(RightWrap))=>{
                *self = Self::right_wrap();
                Some(index + 1)
            }
            _ => None
        }
    }
}

impl LiveHook for Size {
    fn skip_apply(&mut self, cx: &mut Cx, _apply: &Apply, index: usize, nodes: &[LiveNode]) -> Option<usize> {
        match &nodes[index].value {
            LiveValue::Array => {
                fn last_keyframe_value_from_array(index: usize, nodes: &[LiveNode]) -> Option<usize> {
                    if let Some(index) = nodes.last_child(index) {
                        if nodes[index].value.is_object() {
                            return nodes.child_by_name(index, live_id!(value).as_field());
                        }
                        else {
                            return Some(index)
                        }
                    }
                    None
                }

                if let Some(inner_index) = last_keyframe_value_from_array(index, nodes) {
                    match &nodes[inner_index].value {
                        LiveValue::Float64(val) => {
                            *self = Self::Fixed(*val);
                        }
                        LiveValue::Int64(val) => {
                            *self = Self::Fixed(*val as f64);
                        }
                        _ => {
                            cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Animation array");
                        }
                    }
                }
                else {
                    cx.apply_error_wrong_value_type_for_primitive(live_error_origin!(), index, nodes, "Animation array");
                }
                Some(nodes.skip_node(index))
            }
            LiveValue::BareEnum(live_id!(Fill))=>{
                *self = Self::fill();
                Some(index + 1)
            }
            LiveValue::BareEnum(live_id!(Fit))=>{
                *self = Self::fit();
                Some(index + 1)
            }
            LiveValue::Expr {..} => {
                panic!("Expr node found whilst deserialising DSL")
            },
            LiveValue::Float32(v) => {
                *self = Self::Fixed(*v as f64);
                Some(index + 1)
            }
            LiveValue::Float64(v) => {
                *self = Self::Fixed(*v);
                Some(index + 1)
            }
            LiveValue::Int64(v) => {
                *self = Self::Fixed(*v as f64);
                Some(index + 1)
            }
            _ => None
        }
    }
}*/
