//! SplitPane — two panes and a bar, where the least each pane may be is a
//! law and not a courtesy extended to the hand.
//!
//! The library already has a splitter, and on that one a floor governs the
//! drag and nothing else. That is deliberate there and it is right there:
//! folding a panel away IS writing a position of zero on it, so a floor
//! applied to the layout would quietly reopen every panel an application
//! tried to close. The cost is that the rule ends up in the host. Every
//! caller that declares a floor and also writes positions — restoring a
//! saved layout, snapping a sidebar to a preset, nudging a pane from a menu
//! — has to apply the floor again itself, in each of those places, or watch
//! a pane come back too narrow to use.
//!
//! Here the fold is a state of its own rather than a position, so the floors
//! have nothing left to fight: closing a pane says `fold`, never
//! `position: 0`, and the floors can then hold in every other place — a
//! drag, a `set_position`, a layout restored from disk, and a window pulled
//! narrower than the two panes together need. `position` therefore means
//! something different here than it does there, which is why this is a
//! widget and not a preset of the other one.
//!
//! # What it adds
//!
//! * **The floors hold everywhere.** A drag, a host's `set_position` and
//!   the keyboard all pass through the same arithmetic.
//! * **A window too small is a temporary condition.** `position` is what was
//!   asked for; the bar stands wherever the law allows that this pass. Squeeze
//!   the window until a pane is against its floor and open it out again, and
//!   the bar returns to the number that was asked for rather than staying
//!   where the squeeze left it.
//! * **A grip.** The bar carries three marks, so it reads as something to
//!   take hold of rather than as a seam between two panels.
//! * **The keyboard.** The bar is a focus stop: the arrows move it by
//!   `key_step`, and Home and End send it to the two clamps, which is the
//!   only way to reach them exactly.
//! * **A number to persist.** It reports where the bar settled, in points,
//!   for a host that wants the layout back next time.
//!
//! # What it deliberately does not do
//!
//! There is no weighted or edge-relative align: the position is a number of
//! points measured from the first pane's edge, and that is all it is. A
//! share of the window and a minimum size in points cannot both be honoured
//! while the window shrinks, and when they disagree it is always the share
//! that gives way — so offering one would be offering a promise this widget
//! cannot keep. It splits two panes and does not nest, tile or reorder them;
//! three panes are two of these.
use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    splitter::SplitterAxis,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Not splatted. `Open`, `A` and `B` are far too plain to export into
    // every namespace that says `use mod.widgets.*`, and a bare `A` would
    // shadow whatever the next module means by it. Written out in full as
    // SplitFold.A.
    mod.widgets.SplitFold = #(SplitFold::script_api(vm))

    set_type_default() do #(DrawSplitPane::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.SplitPaneBase = #(SplitPane::register_widget(vm))

    /** Two panes and a draggable bar whose per-pane minimums always hold. */
    mod.widgets.SplitPane = set_type_default() do mod.widgets.SplitPaneBase{
        width: Fill
        height: Fill

        /** the bar's thickness in pixels 2..24 step 1 */
        size: 8.0
        /** the first pane's size in points, and the number a host persists 0..2000 step 1 */
        position: 240.0
        /** the least the first pane may be, in points 0..600 step 4 */
        min_a: 80.0
        /** the least the second pane may be, in points 0..600 step 4 */
        min_b: 80.0
        /** points an arrow key moves the bar 1..96 step 1 */
        key_step: 16.0
        // Written out in full: this block's `use mod.widgets.*` is a
        // snapshot taken before the line above that registers the enum, so
        // the bare name is not in scope here however plainly it reads.
        /** fold one pane away: SplitFold.Open A B */
        fold: mod.widgets.SplitFold.Open

        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)

            /** how much of the bar the grip runs along, in pixels 8..400 step 2 */
            grip_length: uniform(44.0)
            /** the grip's inset from the two long edges in pixels 0..6 step 0.5 */
            grip_pad: uniform(1.0)
            /** radius of one grip mark in pixels 0.5..4 step 0.25 */
            grip_dot: uniform(1.25)
            /** centre-to-centre spacing of the grip marks in pixels 2..16 step 0.5 */
            grip_gap: uniform(5.0)
            /** corner rounding radius 0..8 step 0.5 */
            border_radius: uniform(2.0)

            color: uniform(theme.color_d_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_drag: uniform(theme.color_outset_drag)
            color_focus: uniform(theme.color_val_focus)
            // The strip's ground: the gutter the grip floats in. Overridable
            // so an app is not forced to carry the theme's panel colour
            // through every split it makes.
            color_bg: uniform(theme.color_bg_app)
            grip_color: uniform(theme.color_label_outer_off)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(self.color_bg)

                let bar = self.color
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_focus, self.focus)

                let mx = self.rect_size.x * 0.5
                let my = self.rect_size.y * 0.5
                let gap = self.grip_gap

                // Three marks, laid ALONG the bar. Marks across it would be
                // wider than the bar is thick and would be cut off by it.
                if self.is_vertical > 0.5 {
                    sdf.box(
                        self.grip_pad
                        my - self.grip_length * 0.5
                        self.rect_size.x - 2.0 * self.grip_pad
                        self.grip_length
                        self.border_radius
                    )
                    sdf.fill(bar)
                    sdf.circle(mx, my - gap, self.grip_dot)
                    sdf.fill(self.grip_color)
                    sdf.circle(mx, my, self.grip_dot)
                    sdf.fill(self.grip_color)
                    sdf.circle(mx, my + gap, self.grip_dot)
                    sdf.fill(self.grip_color)
                }
                else {
                    sdf.box(
                        mx - self.grip_length * 0.5
                        self.grip_pad
                        self.grip_length
                        self.rect_size.y - 2.0 * self.grip_pad
                        self.border_radius
                    )
                    sdf.fill(bar)
                    sdf.circle(mx - gap, my, self.grip_dot)
                    sdf.fill(self.grip_color)
                    sdf.circle(mx, my, self.grip_dot)
                    sdf.fill(self.grip_color)
                    sdf.circle(mx + gap, my, self.grip_dot)
                    sdf.fill(self.grip_color)
                }

                return sdf.result
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {drag: 0.0, hover: 0.0}}
                }
                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.1}
                        drag: Forward {duration: 0.01}
                    }
                    apply: {draw_bg: {drag: 0.0, hover: snap(1.0)}}
                }
                drag: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {drag: snap(1.0), hover: 1.0}}
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
                }
            }
        }
    }
}

/// Which pane, if either, is folded away.
///
/// Folding is a state and not a position, and that is the whole design of
/// this widget: because closing a pane never writes a position, the floors
/// are free to hold everywhere else. Unfolding restores the bar the person
/// left without anybody having remembered it, since `position` was never
/// written when it went away.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum SplitFold {
    #[pick]
    Open = 0,
    /// The first pane is folded away; the second has the room.
    A = 1,
    /// The second is folded away.
    B = 2,
}

/// The room a split has this pass and the floors that govern it.
///
/// Pure arithmetic in a plain struct, for two reasons. It can be tested
/// without a script heap; and the drag, the keyboard, the host's setter and
/// the layout pass all read the rule from this one place, which is what
/// makes it a law rather than a habit that three of the four share.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Law {
    /// The whole span along the axis: both panes and the bar together.
    room: f64,
    /// The bar's thickness. It is never given away, folded or not: the bar
    /// is the only way back from a fold.
    bar: f64,
    min_a: f64,
    min_b: f64,
}

impl Law {
    /// The room the two panes have to share, once the bar has taken its own.
    fn free(&self) -> f64 {
        (self.room - self.bar).max(0.0)
    }

    /// The narrowest and the widest the first pane may be.
    ///
    /// When the two floors do not both fit — a window dragged narrower than
    /// the sum of them — there is no range left to report and both bounds
    /// come back as the one position that is left, so a caller that clamps
    /// with these still gets a legal answer.
    fn limits(&self) -> (f64, f64) {
        let free = self.free();
        let lo = self.min_a.clamp(0.0, free);
        let hi = (free - self.min_b).clamp(0.0, free);
        if lo <= hi {
            (lo, hi)
        } else {
            let only = self.shortfall();
            (only, only)
        }
    }

    /// Where the bar goes when the floors cannot both be met.
    ///
    /// Both panes miss their floor by the same share of it, rather than one
    /// of them absorbing the whole shortfall. A two-hundred point sidebar
    /// beside a fifty point gutter stays four times the gutter as the window
    /// closes, so a layout squeezed and then opened out again passes through
    /// sizes that look like the layout, not like a panel being crushed.
    fn shortfall(&self) -> f64 {
        let free = self.free();
        let total = self.min_a + self.min_b;
        if total <= 0.0 {
            free * 0.5
        } else {
            free * (self.min_a / total)
        }
    }

    /// A position, brought inside the floors.
    fn clamp(&self, position: f64) -> f64 {
        let (lo, hi) = self.limits();
        position.clamp(lo, hi)
    }

    /// A position moved by `delta` and clamped, which is what one arrow key
    /// press does.
    fn step(&self, position: f64, delta: f64) -> f64 {
        self.clamp(position + delta)
    }

    /// Where the bar actually stands: hard against an edge when a pane is
    /// folded, and inside the floors otherwise.
    ///
    /// The floors are not consulted for a folded pane. That is what folding
    /// means, and it is the one hole in the law — a deliberate one, and the
    /// reason the law can be absolute everywhere else.
    fn resolve(&self, position: f64, fold: SplitFold) -> f64 {
        match fold {
            SplitFold::A => 0.0,
            // The room LESS the bar, not the whole room: the bar is laid
            // out after the first pane and takes its thickness from what is
            // left, so a first pane handed everything leaves the bar
            // nothing and it is not drawn at all. The panel would close and
            // the handle that reopens it would go with it.
            SplitFold::B => self.free(),
            SplitFold::Open => self.clamp(position),
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplitPane {
    #[deref]
    draw_super: DrawQuad,
    /// Whether the bar itself runs vertically, which is the case a
    /// `Horizontal` axis produces: the panes sit left and right and the bar
    /// between them is upright.
    #[live]
    is_vertical: f32,
}

#[derive(Clone)]
enum DrawState {
    DrawA,
    DrawSplit,
    DrawB,
}

/// What the finger has hold of, so the bar tracks the hand rather than
/// jumping to it.
#[derive(Copy, Clone, Debug)]
struct Drag {
    /// Where the bar stood when the press landed.
    from: f64,
    /// The finger's place along the axis at that same moment.
    abs: f64,
}

#[derive(Clone, Debug, Default)]
pub enum SplitPaneAction {
    /// The bar moved, and this is where it now stands. Sent on every frame
    /// of a drag and on every key press, for a readout that follows the
    /// hand.
    Moved(f64),
    /// The gesture is over. This is the number to persist.
    Settled(f64),
    /// A pane was folded away, or brought back.
    Folded(SplitFold),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct SplitPane {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,

    #[walk]
    walk: Walk,
    #[apply_default]
    animator: Animator,

    /// Side by side or stacked. The same axis the other splitter uses, so a
    /// host swapping one for the other is not also learning a new word.
    #[live(SplitterAxis::Horizontal)]
    pub axis: SplitterAxis,

    /// What was ASKED for: the first pane's size in points, from a host, the
    /// DSL, or the last hand that let go of the bar. Where the bar actually
    /// stands is `position()`, which is this brought inside the floors and
    /// the room that exists this pass.
    #[live(240.0)]
    pub position: f64,
    /// The least the first pane may be, in points.
    #[live(80.0)]
    pub min_a: f64,
    /// The least the second pane may be, in points.
    #[live(80.0)]
    pub min_b: f64,
    /// Which pane is folded away, if either. Kept apart from `position` so
    /// unfolding restores the bar that went away.
    #[live(SplitFold::Open)]
    pub fold: SplitFold,

    /// The bar's thickness.
    #[live(8.0)]
    pub size: f64,
    /// Points one arrow key press moves the bar.
    #[live(16.0)]
    pub key_step: f64,

    #[redraw]
    #[live]
    draw_bg: DrawSplitPane,

    #[find]
    #[live]
    a: WidgetRef,
    #[find]
    #[live]
    b: WidgetRef,

    #[rust]
    rect: Rect,
    /// Where the bar stood last time it was laid out or asked to move.
    #[rust]
    shown: f64,
    #[rust]
    drag: Option<Drag>,
    #[rust]
    area_a: Area,
    #[rust]
    area_b: Area,

    #[rust]
    draw_state: DrawStateWrap<DrawState>,
}

impl Widget for SplitPane {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
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
                self.set_resize_cursor(cx);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.set_resize_cursor(cx);
                self.animator_play(cx, ids!(hover.drag));
                // A press alone must not bring a folded pane back — the bar
                // is also the thing you click on your way to something else.
                self.drag = Some(Drag { from: self.shown, abs: self.along(fe.abs) });
            }
            Hit::FingerMove(fe) => {
                if let Some(drag) = self.drag {
                    let delta = self.along(fe.abs) - drag.abs;
                    if delta != 0.0 {
                        // A pane folded away comes back at its floor, which
                        // is the nearest legal size to where the bar is
                        // standing. The bar then waits at the floor until
                        // the finger has travelled past it, so the two meet
                        // up rather than the bar leaping to the hand.
                        if self.fold != SplitFold::Open {
                            self.fold = SplitFold::Open;
                            cx.widget_action(self.uid, SplitPaneAction::Folded(SplitFold::Open));
                        }
                        self.drag_to(cx, drag.from + delta);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                let started_at = self.drag.take().map(|drag| drag.from);
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                // Only a drag that actually moved the bar has a number worth
                // writing down; a click on the way past does not.
                if started_at.map(|from| from != self.shown).unwrap_or(false) {
                    cx.widget_action(self.uid, SplitPaneAction::Settled(self.position));
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            Hit::KeyDown(ke) => {
                let law = self.law();
                let (lo, hi) = law.limits();
                // Left and up take room from the first pane whichever way
                // the split runs, so the key that means "less" is the one
                // pointing at the pane that loses.
                let want = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowUp => Some(law.step(self.shown, -self.step())),
                    KeyCode::ArrowRight | KeyCode::ArrowDown => Some(law.step(self.shown, self.step())),
                    // The only way to land exactly on a clamp. A drag can be
                    // pushed into one, but nothing else says where it is.
                    KeyCode::Home => Some(lo),
                    KeyCode::End => Some(hi),
                    _ => None,
                };
                if let Some(want) = want {
                    // A folded pane comes back to where it was, not to where
                    // the key points: the keyboard has no position of its
                    // own to follow, so the remembered one is the only
                    // answer here that is not invented.
                    if self.fold != SplitFold::Open {
                        self.set_fold(cx, SplitFold::Open);
                    } else {
                        self.drag_to(cx, want);
                        cx.widget_action(self.uid, SplitPaneAction::Settled(self.position));
                    }
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

impl SplitPane {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        match self.axis {
            SplitterAxis::Horizontal => {
                cx.begin_turtle(walk, Layout::flow_right());
            }
            SplitterAxis::Vertical => {
                cx.begin_turtle(walk, Layout::flow_down());
            }
        }

        self.rect = cx.turtle().inner_rect();
        // The law is applied here and not only where the bar is dragged.
        // `position` is the ask and it is left alone, so a window squeezed
        // until a pane is against its floor and then opened out again
        // returns the bar to the number that was asked for.
        self.shown = self.law().resolve(self.position, self.fold);

        let walk = match self.axis {
            SplitterAxis::Horizontal => Walk::new(Size::Fixed(self.shown), Size::fill()),
            SplitterAxis::Vertical => Walk::new(Size::fill(), Size::Fixed(self.shown)),
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
        // The bar is the control, so the bar is what the keyboard reaches.
        cx.add_nav_stop(self.draw_bg.area(), NavRole::Slider, Inset::default());
        cx.begin_turtle(Walk::default(), Layout::flow_down());
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area_b);
        cx.end_turtle();
    }

    /// The room and the floors as they stand this pass. Built fresh every
    /// time: all four numbers are live properties and the tweaker may have
    /// moved any of them since the last draw.
    fn law(&self) -> Law {
        Law {
            room: self.room(),
            bar: self.size.max(0.0),
            min_a: self.min_a.max(0.0),
            min_b: self.min_b.max(0.0),
        }
    }

    fn room(&self) -> f64 {
        match self.axis {
            SplitterAxis::Horizontal => self.rect.size.x,
            SplitterAxis::Vertical => self.rect.size.y,
        }
    }

    /// A point's place along the axis the split runs on.
    fn along(&self, abs: Vec2d) -> f64 {
        match self.axis {
            SplitterAxis::Horizontal => abs.x,
            SplitterAxis::Vertical => abs.y,
        }
    }

    fn step(&self) -> f64 {
        self.key_step.max(1.0)
    }

    /// Whether the widget has been laid out yet. Before that there is no
    /// room, so there is nothing for the law to work with and the ask is the
    /// only honest answer to give.
    fn laid_out(&self) -> bool {
        self.room() > 0.0
    }

    fn set_resize_cursor(&self, cx: &mut Cx) {
        match self.axis {
            SplitterAxis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
            SplitterAxis::Vertical => cx.set_cursor(MouseCursor::RowResize),
        }
    }

    /// Move the bar on behalf of a hand or a key.
    ///
    /// The ask is set to the clamped position rather than the raw one: this
    /// came from somebody pushing the bar, and a push past the floor is a
    /// request for the floor. Only a host may hold an ask the window cannot
    /// currently honour, because only a host has a reason to — it is asking
    /// for a layout, not for a place on this screen.
    fn drag_to(&mut self, cx: &mut Cx, want: f64) {
        let shown = self.law().resolve(want, self.fold);
        if shown == self.shown {
            return;
        }
        self.shown = shown;
        self.position = shown;
        cx.widget_action(self.uid, SplitPaneAction::Moved(shown));
        self.draw_bg.redraw(cx);

        // Redraw both panes' whole subtrees. The pane areas reach nested
        // draw-list-optimized views whose own size-based dirty check misses
        // a pure size change on this axis.
        self.a.redraw(cx);
        self.b.redraw(cx);
        cx.redraw_area_and_children(self.area_a);
        cx.redraw_area_and_children(self.area_b);
    }

    /// Where the bar stands: the ask, brought inside the floors and the room
    /// that exists.
    pub fn position(&self) -> f64 {
        if self.laid_out() {
            self.shown
        } else {
            self.position
        }
    }

    /// What was last asked for. This is the number to persist: a window that
    /// was briefly too small must not shrink a saved layout for good.
    pub fn asked_position(&self) -> f64 {
        self.position
    }

    /// Ask for a position. The floors hold: unlike the other splitter, a
    /// host cannot talk its way past them by writing a number. Folding is
    /// how a pane is closed, and it is a separate call for that reason.
    pub fn set_position(&mut self, cx: &mut Cx, points: f64) {
        self.position = points;
        self.shown = self.law().resolve(self.position, self.fold);
        self.redraw(cx);
    }

    pub fn fold(&self) -> SplitFold {
        self.fold
    }

    /// Fold a pane away, or bring it back. `position` is left alone, so what
    /// comes back is the bar that went away — and it comes back through the
    /// law, so a bar remembered from a wider window still arrives legal.
    pub fn set_fold(&mut self, cx: &mut Cx, fold: SplitFold) {
        if self.fold != fold {
            self.fold = fold;
            self.shown = self.law().resolve(self.position, self.fold);
            cx.widget_action(self.uid, SplitPaneAction::Folded(fold));
            self.redraw(cx);
        }
    }

    pub fn axis(&self) -> SplitterAxis {
        self.axis
    }

    pub fn set_axis(&mut self, axis: SplitterAxis) {
        self.axis = axis;
    }

    pub fn area_a(&self) -> Area {
        self.area_a
    }

    pub fn area_b(&self) -> Area {
        self.area_b
    }

    /// The two positions the clamps allow, as the keyboard's Home and End
    /// reach them. A host that wants to show them can ask.
    pub fn limits(&self) -> (f64, f64) {
        self.law().limits()
    }

    fn margin(&self) -> Inset {
        self.axis_inset(3.0)
    }

    /// A wider hit margin for touch. Fingers are blunter than cursors, so the
    /// bar needs more slop to be grabbable on a touchscreen.
    fn touch_margin(&self) -> Inset {
        self.axis_inset(8.0)
    }

    fn axis_inset(&self, side: f64) -> Inset {
        match self.axis {
            SplitterAxis::Horizontal => Inset { left: side, top: 0.0, right: side, bottom: 0.0 },
            SplitterAxis::Vertical => Inset { left: 0.0, top: side, right: 0.0, bottom: side },
        }
    }
}

impl SplitPaneRef {
    /// Where the bar stands.
    pub fn position(&self) -> f64 {
        self.borrow().map(|inner| inner.position()).unwrap_or(0.0)
    }

    /// What was last asked for, which is the number to persist.
    pub fn asked_position(&self) -> f64 {
        self.borrow().map(|inner| inner.asked_position()).unwrap_or(0.0)
    }

    pub fn set_position(&self, cx: &mut Cx, points: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_position(cx, points);
        }
    }

    pub fn fold(&self) -> SplitFold {
        self.borrow().map(|inner| inner.fold()).unwrap_or(SplitFold::Open)
    }

    pub fn set_fold(&self, cx: &mut Cx, fold: SplitFold) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_fold(cx, fold);
        }
    }

    pub fn limits(&self) -> (f64, f64) {
        self.borrow().map(|inner| inner.limits()).unwrap_or((0.0, 0.0))
    }

    /// Where the bar is now, while a drag or a key press is moving it.
    pub fn moved(&self, actions: &Actions) -> Option<f64> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            SplitPaneAction::Moved(points) => Some(points),
            _ => None,
        }
    }

    /// The position the gesture settled on: the one to write down.
    pub fn settled(&self, actions: &Actions) -> Option<f64> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            SplitPaneAction::Settled(points) => Some(points),
            _ => None,
        }
    }

    /// The fold this split reports this pass, if it changed.
    pub fn folded(&self, actions: &Actions) -> Option<SplitFold> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            SplitPaneAction::Folded(fold) => Some(fold),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window most of these run in: nine hundred points, an eight point
    /// bar, and floors of 180 and 50 — so the bar may stand anywhere from 180
    /// to 842 and nowhere else.
    fn wide() -> Law {
        Law { room: 900.0, bar: 8.0, min_a: 180.0, min_b: 50.0 }
    }

    /// A position the floors allow is returned untouched.
    #[test]
    fn a_position_inside_both_floors_is_left_alone() {
        assert_eq!(wide().clamp(400.0), 400.0);
    }

    /// A host writing past either clamp is brought back to it. This is the
    /// whole difference from the other splitter, where a written position is
    /// taken as given and the floors apply to the hand alone.
    #[test]
    fn a_set_position_past_either_clamp_is_pulled_back_to_it() {
        let law = wide();
        assert_eq!(law.clamp(0.0), 180.0, "the first pane keeps its floor");
        assert_eq!(law.clamp(-500.0), 180.0, "however far past it the ask went");
        assert_eq!(law.clamp(2000.0), 842.0, "and the second keeps its own");
        // 842 is the room, less the bar, less pane B's fifty points.
        assert_eq!(law.limits(), (180.0, 842.0));
    }

    /// A window squeezed until a pane is against its floor moves the bar, but
    /// does not touch what was asked for — so opening the window out again
    /// puts the bar back where it was. This is why the ask and the position
    /// are two numbers and not one.
    #[test]
    fn a_squeeze_moves_the_bar_and_not_the_ask() {
        let asked = 400.0;
        let wide = wide();
        assert_eq!(wide.resolve(asked, SplitFold::Open), 400.0);

        // The same split in a window dragged down to 300 points. Pane B's
        // fifty are still B's, so the bar gives way.
        let narrow = Law { room: 300.0, ..wide };
        assert_eq!(narrow.resolve(asked, SplitFold::Open), 242.0);

        // And the ask, which was never written, brings the bar back.
        assert_eq!(wide.resolve(asked, SplitFold::Open), 400.0);
    }

    /// Squeezed past the point where both floors can be met, both panes miss
    /// their floor by the same share of it. Handing the whole shortfall to
    /// one of them is what makes a squeezed layout stop looking like itself.
    #[test]
    fn floors_that_cannot_both_be_met_are_missed_by_the_same_share() {
        let law = Law { room: 200.0, ..wide() };
        let a = law.resolve(400.0, SplitFold::Open);
        let b = law.free() - a;
        assert!(a < law.min_a && b < law.min_b, "neither floor could be met");
        let missed_a = 1.0 - a / law.min_a;
        let missed_b = 1.0 - b / law.min_b;
        assert!((missed_a - missed_b).abs() < 1e-9, "{missed_a} against {missed_b}");
        // And the two panes still add up to the room the bar left them.
        assert!((a + b - law.free()).abs() < 1e-9);
    }

    /// No room at all is that same degenerate case and not a division by
    /// zero or a pane of negative width.
    #[test]
    fn no_room_at_all_still_answers() {
        let law = Law { room: 0.0, ..wide() };
        assert_eq!(law.clamp(400.0), 0.0);
        assert_eq!(law.resolve(400.0, SplitFold::Open), 0.0);
    }

    /// Floors of nothing are legal: the bar may go anywhere, and a window
    /// too small for the bar itself still answers.
    #[test]
    fn a_split_with_no_floors_may_go_anywhere() {
        let law = Law { room: 500.0, bar: 8.0, min_a: 0.0, min_b: 0.0 };
        assert_eq!(law.limits(), (0.0, 492.0));
        assert_eq!(law.clamp(0.0), 0.0);
        let tiny = Law { room: 4.0, ..law };
        assert_eq!(tiny.clamp(2.0), 0.0, "the bar takes what there is");
    }

    /// Folding puts the bar hard against an edge, past any floor — that is
    /// what folding is for, and it is the one hole in the law. Reopening
    /// returns the exact bar that went away, because `position` was never
    /// written when it did.
    #[test]
    fn a_fold_goes_past_the_floors_and_reopening_returns_the_bar() {
        let law = wide();
        let left_at = 244.0;
        assert_eq!(
            law.resolve(left_at, SplitFold::A),
            0.0,
            "folding the first pane leaves it nothing, floor or no floor"
        );
        assert_eq!(
            law.resolve(left_at, SplitFold::B),
            892.0,
            "folding the second leaves the first everything except the bar, \
             which has to stay: it is the only way back"
        );
        assert_eq!(
            law.resolve(left_at, SplitFold::Open),
            left_at,
            "and reopening is the bar the person left"
        );
    }

    /// A pane folded away while the window was wide, reopened after the
    /// window has shrunk, comes back legal rather than coming back wrong.
    #[test]
    fn a_bar_remembered_from_a_wider_window_reopens_inside_the_floors() {
        let asked = 700.0;
        assert_eq!(wide().resolve(asked, SplitFold::A), 0.0);
        let narrow = Law { room: 400.0, ..wide() };
        assert_eq!(narrow.resolve(asked, SplitFold::Open), 342.0);
    }

    /// One arrow key press is a step that stops at the clamp rather than
    /// walking through it, so holding a key down parks the bar on the floor.
    #[test]
    fn an_arrow_key_stops_at_the_clamp() {
        let law = wide();
        assert_eq!(law.step(400.0, 16.0), 416.0);
        assert_eq!(law.step(190.0, -16.0), 180.0, "not 174");
        assert_eq!(law.step(180.0, -16.0), 180.0, "and it stays there");
        assert_eq!(law.step(840.0, 16.0), 842.0);
    }

    /// Home and End are the two clamps exactly, which nothing else reaches:
    /// a drag can be pushed into a clamp but never says where it is.
    #[test]
    fn home_and_end_are_the_clamps_themselves() {
        let (lo, hi) = wide().limits();
        assert_eq!(lo, 180.0);
        assert_eq!(hi, 842.0);
        assert_eq!(wide().clamp(lo), lo);
        assert_eq!(wide().clamp(hi), hi);
    }
}
