use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl},
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_micro_serde::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SplitterAxis = #(SplitterAxis::script_api(vm))
    mod.widgets.splat(mod.widgets.SplitterAxis)

    mod.widgets.SplitterAlign = #(SplitterAlign::script_api(vm))
    mod.widgets.splat(mod.widgets.SplitterAlign)

    set_type_default() do #(DrawSplitter::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.SplitterBase = #(Splitter::register_widget(vm))

    mod.widgets.Splitter = set_type_default() do mod.widgets.SplitterBase{
        width: Fill
        height: Fill

        size: 6.0
        min_horizontal: 50.0
        max_horizontal: 50.0
        min_vertical: 50.0
        max_vertical: 50.0

        draw_bg +: {
            drag: instance(0.0)
            hover: instance(0.0)

            bar_size: uniform(110.0)

            color: uniform(theme.color_d_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_drag: uniform(theme.color_outset_drag)

            border_radius: uniform(1.0)
            splitter_pad: uniform(1.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(theme.color_bg_app)

                if self.is_vertical > 0.5 {
                    sdf.box(
                        self.splitter_pad
                        self.rect_size.y * 0.5 - self.bar_size * 0.5
                        self.rect_size.x - 2.0 * self.splitter_pad
                        self.bar_size
                        self.border_radius
                    )
                }
                else {
                    sdf.box(
                        self.rect_size.x * 0.5 - self.bar_size * 0.5
                        self.splitter_pad
                        self.bar_size
                        self.rect_size.y - 2.0 * self.splitter_pad
                        self.border_radius
                    )
                }

                return sdf.fill_keep(
                    mix(
                        self.color
                        mix(
                            self.color_hover
                            self.color_drag
                            self.drag
                        )
                        self.hover
                    )
                )
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {drag: 0.0, hover: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.1}
                        drag: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {
                            drag: 0.0,
                            hover: snap(1.0)
                        }
                    }
                }

                drag: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {
                            drag: snap(1.0),
                            hover: 1.0
                        }
                    }
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Script, ScriptHook, Default, SerRon, DeRon)]
pub enum SplitterAxis {
    #[pick]
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Script, ScriptHook, SerRon, DeRon)]
pub enum SplitterAlign {
    #[live(50.0)]
    FromA(f64),
    #[live(50.0)]
    FromB(f64),
    #[pick(0.5)]
    Weighted(f64),
}

impl Default for SplitterAlign {
    fn default() -> Self {
        SplitterAlign::Weighted(0.5)
    }
}

impl SplitterAlign {
    fn to_position(self, axis: SplitterAxis, rect: Rect) -> f64 {
        match axis {
            SplitterAxis::Horizontal => match self {
                Self::FromA(position) => position,
                Self::FromB(position) => rect.size.x - position,
                Self::Weighted(weight) => weight * rect.size.x,
            },
            SplitterAxis::Vertical => match self {
                Self::FromA(position) => position,
                Self::FromB(position) => rect.size.y - position,
                Self::Weighted(weight) => weight * rect.size.y,
            },
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplitter {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    is_vertical: f32,
}

#[derive(Clone)]
enum DrawState {
    DrawA,
    DrawSplit,
    DrawB,
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct Splitter {
    #[uid] uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[walk]
    walk: Walk,
    #[apply_default]
    animator: Animator,

    #[live(SplitterAxis::Horizontal)]
    pub axis: SplitterAxis,
    #[live(SplitterAlign::Weighted(0.5))]
    pub align: SplitterAlign,

    #[rust]
    rect: Rect,
    #[rust]
    position: f64,
    #[rust]
    drag_start_align: Option<SplitterAlign>,
    #[rust]
    area_a: Area,
    #[rust]
    area_b: Area,

    #[live]
    min_vertical: f64,
    #[live]
    max_vertical: f64,
    #[live]
    min_horizontal: f64,
    #[live]
    max_horizontal: f64,

    #[redraw]
    #[live]
    draw_bg: DrawSplitter,
    #[live]
    size: f64,

    // framecomponent mode
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[find]
    #[live]
    a: WidgetRef,
    #[find]
    #[live]
    b: WidgetRef,

    #[action_data]
    #[rust]
    action_data: WidgetActionData,
}

#[derive(Clone, Debug, Default)]
pub enum SplitterAction {
    #[default]
    None,
    Changed {
        axis: SplitterAxis,
        align: SplitterAlign,
    },
}

impl Widget for Splitter {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();

        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        match event.hits_with_options(
            cx,
            self.draw_bg.area(),
            HitOptions::new().with_margin(self.margin()),
        ) {
            Hit::FingerHoverIn(_) => {
                match self.axis {
                    SplitterAxis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                    SplitterAxis::Vertical => cx.set_cursor(MouseCursor::RowResize),
                }
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(_) => {
                match self.axis {
                    SplitterAxis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                    SplitterAxis::Vertical => cx.set_cursor(MouseCursor::RowResize),
                }
                self.animator_play(cx, ids!(hover.drag));
                self.drag_start_align = Some(self.align);
            }
            Hit::FingerUp(f) => {
                self.drag_start_align = None;
                if f.is_over && f.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            Hit::FingerMove(f) => {
                if let Some(drag_start_align) = self.drag_start_align {
                    let delta = match self.axis {
                        SplitterAxis::Horizontal => f.abs.x - f.abs_start.x,
                        SplitterAxis::Vertical => f.abs.y - f.abs_start.y,
                    };
                    let new_position = drag_start_align.to_position(self.axis, self.rect) + delta;
                    self.align = match self.axis {
                        SplitterAxis::Horizontal => {
                            let center = self.rect.size.x / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromA(new_position.max(self.min_vertical))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromB(
                                    (self.rect.size.x - new_position).max(self.max_vertical),
                                )
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.x)
                            }
                        }
                        SplitterAxis::Vertical => {
                            let center = self.rect.size.y / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromA(new_position.max(self.min_horizontal))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromB(
                                    (self.rect.size.y - new_position).max(self.max_horizontal),
                                )
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.y)
                            }
                        }
                    };
                    self.draw_bg.redraw(cx);
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        SplitterAction::Changed {
                            axis: self.axis,
                            align: self.align,
                        },
                    );

                    self.a.redraw(cx);
                    self.b.redraw(cx);
                }
            }
            _ => {}
        }
        self.a.handle_event(cx, event, scope);
        self.b.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::DrawA) {
            self.begin(cx, walk);
        }
        if let Some(DrawState::DrawA) = self.draw_state.get() {
            self.a.draw(cx, scope)?;
            self.draw_state.set(DrawState::DrawSplit);
        }
        if let Some(DrawState::DrawSplit) = self.draw_state.get() {
            self.middle(cx);
            self.draw_state.set(DrawState::DrawB)
        }
        if let Some(DrawState::DrawB) = self.draw_state.get() {
            self.b.draw(cx, scope)?;
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl Splitter {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        // we should start a fill turtle in the layout direction of choice
        match self.axis {
            SplitterAxis::Horizontal => {
                cx.begin_turtle(walk, Layout::flow_right());
            }
            SplitterAxis::Vertical => {
                cx.begin_turtle(walk, Layout::flow_down());
            }
        }

        self.rect = cx.turtle().inner_rect();
        self.position = self.align.to_position(self.axis, self.rect);

        let walk = match self.axis {
            SplitterAxis::Horizontal => Walk::new(Size::Fixed(self.position), Size::fill()),
            SplitterAxis::Vertical => Walk::new(Size::fill(), Size::Fixed(self.position)),
        };
        cx.begin_turtle(walk, Layout::flow_down());
    }

    pub fn middle(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area_a);
        match self.axis {
            SplitterAxis::Horizontal => {
                self.draw_bg.is_vertical = 1.0;
                self.draw_bg
                    .draw_walk(cx, Walk::new(Size::Fixed(self.size), Size::fill()));
            }
            SplitterAxis::Vertical => {
                self.draw_bg.is_vertical = 0.0;
                self.draw_bg
                    .draw_walk(cx, Walk::new(Size::fill(), Size::Fixed(self.size)));
            }
        }
        cx.begin_turtle(Walk::default(), Layout::flow_down());
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area_b);
        cx.end_turtle();
    }

    pub fn axis(&self) -> SplitterAxis {
        self.axis
    }

    pub fn area_a(&self) -> Area {
        self.area_a
    }

    pub fn area_b(&self) -> Area {
        self.area_b
    }

    pub fn set_axis(&mut self, axis: SplitterAxis) {
        self.axis = axis;
    }

    pub fn align(&self) -> SplitterAlign {
        self.align
    }

    pub fn set_align(&mut self, align: SplitterAlign) {
        self.align = align;
    }

    fn margin(&self) -> Inset {
        match self.axis {
            SplitterAxis::Horizontal => Inset {
                left: 3.0,
                top: 0.0,
                right: 3.0,
                bottom: 0.0,
            },
            SplitterAxis::Vertical => Inset {
                left: 0.0,
                top: 3.0,
                right: 0.0,
                bottom: 3.0,
            },
        }
    }

    pub fn changed(&self, actions: &Actions) -> Option<(SplitterAxis, SplitterAlign)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let SplitterAction::Changed { axis, align } = item.cast() {
                return Some((axis, align));
            }
        }
        None
    }
}

impl SplitterRef {
    pub fn changed(&self, actions: &Actions) -> Option<(SplitterAxis, SplitterAlign)> {
        self.borrow().and_then(|inner| inner.changed(actions))
    }

    pub fn set_axis(&self, cx: &mut Cx, axis: SplitterAxis) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_axis(axis);
            inner.redraw(cx);
        }
    }

    pub fn set_align(&self, cx: &mut Cx, align: SplitterAlign) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_align(align);
            inner.redraw(cx);
        }
    }

    pub fn axis(&self) -> Option<SplitterAxis> {
        self.borrow().map(|inner| inner.axis())
    }

    pub fn align(&self) -> Option<SplitterAlign> {
        self.borrow().map(|inner| inner.align())
    }
}
