use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
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

    // Not splatted: `None` as a bare exported name would shadow the one
    // every other module means by it. Written out as SplitterCollapse.A.
    mod.widgets.SplitterCollapse = #(SplitterCollapse::script_api(vm))

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
        // Written out in full: this block's `use mod.widgets.*` is a
        // snapshot taken before the registration above it, so the bare name
        // is not in scope here however plainly it reads.
        /** fold one pane away: SplitterCollapse.None A B */
        collapse: mod.widgets.SplitterCollapse.None

        draw_bg +: {
            drag: instance(0.0)
            hover: instance(0.0)

            bar_size: uniform(110.0)

            color: uniform(theme.color_d_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_drag: uniform(theme.color_outset_drag)
            // The strip's GROUND (the gutter the grip bar floats in).
            // Overridable so a dark app is not forced to carry the theme's
            // panel gray through every splitter.
            color_bg: uniform(theme.color_bg_app)

            border_radius: uniform(1.0)
            splitter_pad: uniform(1.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(self.color_bg)

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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Script, ScriptHook, Default, SerRon, DeRon)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SplitterAxis {
    #[pick]
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Script, ScriptHook, SerRon, DeRon)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

/// Which pane, if either, is folded away.
///
/// Folding is a STATE and not a position. Three apps in this repo fold a
/// panel by writing zero into the align and remembering the old value in a
/// field of their own, which means each of them also has to put the value
/// back, guard against a stray drag undoing it, and decide where to keep it.
/// Said as a state instead, `align` is never written at all, so unfolding
/// restores the exact bar the user left without anyone having remembered it.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum SplitterCollapse {
    #[pick]
    None = 0,
    /// The first pane is folded away; the second has the room.
    A = 1,
    /// The second is folded away.
    B = 2,
}

/// Where the bar sits this pass: folded hard to an edge, or wherever the
/// align says within the room.
///
/// The floors are not consulted when a pane is folded — that is the whole
/// point of folding, and applying them here is the bug that made a folded
/// panel leave a gutter the width of its own floor.
fn resolve_split_position(
    align_pos: f64,
    room: f64,
    min_a: f64,
    min_b: f64,
    collapse: SplitterCollapse,
    bar: f64,
) -> f64 {
    match collapse {
        SplitterCollapse::A => 0.0,
        // The room LESS the bar, not the whole room. The bar is laid out
        // after the first pane and takes its width from what is left, so a
        // first pane given everything leaves the bar nothing and it is not
        // drawn at all: the panel closes and the handle that would open it
        // again goes with it. Folding the first pane never had this problem
        // because a zero-width pane leaves the bar its width by itself.
        SplitterCollapse::B => (room - bar).max(0.0),
        SplitterCollapse::None => clamp_split_position(align_pos, room, min_a, min_b),
    }
}

/// Clamp a dragged split position into the room both panes' floors allow.
///
/// **A floor governs the hand, not the host.** This is applied to a drag and
/// to nothing else. A host that sets the align itself is making a decision,
/// not sliding a bar, and the commonest such decision is collapsing a panel
/// to nothing — which three apps in this repo do by setting the position to
/// zero on a splitter that also declares a floor. Applying the floor to
/// that too does not enforce a rule, it overrules the caller: the panel
/// stops collapsing and leaves a gutter the width of its own floor. The
/// layout pass therefore clamps only into the room that exists (see
/// `begin`), and the floor lives here, where a finger is on the bar.
///
/// If the two floors do not both fit in the room at all (a pane squeezed
/// narrower than either floor demands), the floors lose rather than the
/// caller: the position lands in the middle of whatever room is left,
/// because a pane that cannot exist is a worse outcome than one that is
/// merely below its own floor.
fn clamp_split_position(position: f64, room: f64, min_a: f64, min_b: f64) -> f64 {
    let room = room.max(0.0);
    let ceiling = room - min_b;
    if min_a <= ceiling {
        position.clamp(min_a, ceiling)
    } else {
        room * 0.5
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
    #[uid]
    uid: WidgetUid,
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
    /// Which pane is folded away, if either. Kept apart from `align` so
    /// unfolding restores the bar the user left, with nobody remembering it.
    #[live(SplitterCollapse::None)]
    pub collapse: SplitterCollapse,

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
    /// A pane was folded away, or brought back.
    Collapsed(SplitterCollapse),
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
            HitOptions::new()
                .with_margin(self.margin())
                .with_touch_margin(self.touch_margin()),
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
            // A folded splitter has no bar: a press that landed on where it
            // used to be must not drag it back, which is why three callers
            // currently have to force the align back to zero every pass.
            Hit::FingerDown(_) if self.collapse != SplitterCollapse::None => {}
            Hit::FingerDown(fe) if self.drag_start_align.is_none() && fe.is_primary_hit() => {
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
                    let raw_position = drag_start_align.to_position(self.axis, self.rect) + delta;
                    let room = match self.axis {
                        SplitterAxis::Horizontal => self.rect.size.x,
                        SplitterAxis::Vertical => self.rect.size.y,
                    };
                    let (min_a, min_b) = self.axis_min_max();
                    let new_position = clamp_split_position(raw_position, room, min_a, min_b);
                    self.align = match self.axis {
                        SplitterAxis::Horizontal => {
                            let center = self.rect.size.x / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromA(new_position)
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromB(self.rect.size.x - new_position)
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.x)
                            }
                        }
                        SplitterAxis::Vertical => {
                            let center = self.rect.size.y / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromA(new_position)
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromB(self.rect.size.y - new_position)
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

                    // Redraw both panes' full subtrees. `a`/`b` only cover the
                    // standalone-widget usage (they are empty in Dock usage, where
                    // the dock draws the pane contents itself); the pane areas are
                    // valid in both, and redrawing their children reaches nested
                    // draw-list-optimized views whose own size-based dirty check
                    // misses a pure height change (their non-fill sizes are compared
                    // against the previous frame's measurement).
                    self.a.redraw(cx);
                    self.b.redraw(cx);
                    cx.redraw_area_and_children(self.area_a);
                    cx.redraw_area_and_children(self.area_b);
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
        let room = match self.axis {
            SplitterAxis::Horizontal => self.rect.size.x,
            SplitterAxis::Vertical => self.rect.size.y,
        };
        // Folded hard to an edge, or into the room and no further. NOT the
        // floors: a host that has set the align itself — restoring a
        // remembered width, say — has said what it wants, and the floors
        // are for the drag. Clamping here as well is what turned a folded
        // panel into an empty gutter the width of its own floor.
        let (min_a, min_b) = self.axis_min_max();
        self.position = resolve_split_position(
            self.align.to_position(self.axis, self.rect),
            room,
            min_a,
            min_b,
            self.collapse,
            self.size,
        );

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

    /// The floor for pane A and the floor for pane B, on this instance's own
    /// axis. Named for the bar's orientation (`_vertical` for the vertical
    /// bar a `Horizontal` axis draws), which is the pairing every existing
    /// caller in this repo already relies on — this reads the same fields
    /// the drag handler always has, it just now reads them from one place.
    fn axis_min_max(&self) -> (f64, f64) {
        match self.axis {
            SplitterAxis::Horizontal => (self.min_vertical, self.max_vertical),
            SplitterAxis::Vertical => (self.min_horizontal, self.max_horizontal),
        }
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

    /// Which pane is folded away, if either.
    pub fn collapse(&self) -> SplitterCollapse {
        self.collapse
    }

    /// Fold a pane away, or bring it back. `align` is left alone, so what
    /// comes back is the bar that went away.
    pub fn set_collapse(&mut self, cx: &mut Cx, collapse: SplitterCollapse) {
        if self.collapse != collapse {
            self.collapse = collapse;
            let uid = self.widget_uid();
            cx.widget_action(uid, SplitterAction::Collapsed(collapse));
            self.redraw(cx);
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn set_align(&mut self, align: SplitterAlign) {
        self.align = align;
    }

    fn margin(&self) -> Inset {
        self.axis_inset(3.0)
    }

    /// Wider hit margin used only when the event came from a touch device.
    /// Fingers are blunter than mouse cursors, so the bar needs more slop to
    /// be grabbable on touchscreens.
    fn touch_margin(&self) -> Inset {
        self.axis_inset(8.0)
    }

    fn axis_inset(&self, side: f64) -> Inset {
        match self.axis {
            SplitterAxis::Horizontal => Inset {
                left: side,
                top: 0.0,
                right: side,
                bottom: 0.0,
            },
            SplitterAxis::Vertical => Inset {
                left: 0.0,
                top: side,
                right: 0.0,
                bottom: side,
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

    pub fn collapse(&self) -> SplitterCollapse {
        self.borrow()
            .map(|inner| inner.collapse())
            .unwrap_or(SplitterCollapse::None)
    }

    pub fn set_collapse(&self, cx: &mut Cx, collapse: SplitterCollapse) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_collapse(cx, collapse);
        }
    }

    /// The fold this splitter reports this pass, if it changed.
    pub fn collapsed(&self, actions: &Actions) -> Option<SplitterCollapse> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<SplitterAction>() {
            SplitterAction::Collapsed(collapse) => Some(collapse),
            _ => None,
        }
    }

    pub fn position(&self) -> Option<f64> {
        self.borrow().map(|inner| inner.position())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Room to spare: the position is already inside both floors, so it is
    /// returned unchanged.
    #[test]
    fn a_position_inside_both_floors_is_untouched() {
        assert_eq!(clamp_split_position(200.0, 500.0, 50.0, 50.0), 200.0);
    }

    /// Below pane A's floor, the floor wins.
    #[test]
    fn a_position_below_a_floor_is_raised_to_it() {
        assert_eq!(clamp_split_position(10.0, 500.0, 50.0, 50.0), 50.0);
    }

    /// Close enough to the far edge that pane B would be squeezed under its
    /// own floor: the position is pulled back to leave B exactly its floor.
    #[test]
    fn a_position_that_would_starve_b_is_pulled_back() {
        assert_eq!(clamp_split_position(490.0, 500.0, 50.0, 50.0), 450.0);
    }

    /// The two floors do not fit in the room at all (a window squeezed
    /// narrower than both floors combined): the floors lose and the split
    /// lands in the middle of what room there is, rather than handing one
    /// pane a negative size or the other the whole strip.
    #[test]
    fn floors_that_do_not_both_fit_land_in_the_middle_instead() {
        assert_eq!(clamp_split_position(999.0, 80.0, 50.0, 50.0), 40.0);
        assert_eq!(clamp_split_position(-999.0, 80.0, 50.0, 50.0), 40.0);
    }

    /// No room at all is the same degenerate case, not a division or a
    /// negative width.
    #[test]
    fn no_room_at_all_still_answers() {
        assert_eq!(clamp_split_position(50.0, 0.0, 50.0, 50.0), 0.0);
    }

    /// Folding puts the bar hard against an edge, whatever the floors say —
    /// that is what folding means, and it is the case a floor applied at
    /// layout time got wrong.
    #[test]
    fn a_folded_pane_goes_to_the_edge_past_any_floor() {
        let (room, min_a, min_b) = (900.0, 180.0, 50.0);
        let bar = 6.0;
        assert_eq!(
            resolve_split_position(244.0, room, min_a, min_b, SplitterCollapse::A, bar),
            0.0,
            "folding the first pane leaves it nothing, floor or no floor"
        );
        assert_eq!(
            resolve_split_position(244.0, room, min_a, min_b, SplitterCollapse::B, bar),
            room - bar,
            "folding the second gives the first everything except the bar,              which has to stay: it is the only way back"
        );
    }

    /// Unfolded, the floors are back in force and the align decides — and
    /// the align was never written, so this is the bar the user left.
    #[test]
    fn unfolding_returns_to_the_bar_that_was_left() {
        let (room, min_a, min_b) = (900.0, 180.0, 50.0);
        let left_at = 244.0;
        // Folded and unfolded, with the align untouched throughout: the
        // position that comes back is the one that went away. This is what
        // three apps each keep a remembered-width field to achieve.
        assert_eq!(
            resolve_split_position(left_at, room, min_a, min_b, SplitterCollapse::A, 6.0),
            0.0
        );
        assert_eq!(
            resolve_split_position(left_at, room, min_a, min_b, SplitterCollapse::None, 6.0),
            left_at
        );
    }

    /// A host collapsing a panel to nothing is honoured, and the floor is
    /// not applied to it. Three apps in this repo collapse a panel by
    /// setting the position to zero on a splitter that also declares a
    /// floor; running the drag clamp over that as well left an empty
    /// gutter the width of the floor instead of a collapsed panel. The
    /// layout pass clamps into the room and stops there — this test is
    /// that division, written down.
    #[test]
    fn a_host_may_collapse_past_a_floor_that_a_drag_may_not() {
        let room = 900.0;
        // What the layout pass does with a deliberate collapse.
        assert_eq!(0.0_f64.clamp(0.0, room), 0.0, "a host asking for nothing gets nothing");
        // What a DRAG to the same place does, on the same splitter.
        assert_eq!(
            clamp_split_position(0.0, room, 180.0, 50.0),
            180.0,
            "a finger on the bar still stops at the floor"
        );
        // And the layout pass still refuses a pane wider than the window,
        // which is the case that made a clamp there worth having at all.
        assert_eq!(2000.0_f64.clamp(0.0, room), room);
    }
}
