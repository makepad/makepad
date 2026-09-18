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
        /** hold the align to the floors as a law: each pane its whole floor beside the bar, and a room too small for both shared in proportion */
        exact_floors: false
        /** points an arrow key moves the bar, and 0 keeps the bar off the keyboard altogether 0..96 step 1 */
        key_step: 0.0
        /** a drag or an arrow key on the bar of a folded pane brings the pane back */
        bar_reopens: false

        draw_bg +: {
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)

            /** how much of the strip the bar runs along, in pixels 8..400 step 2 */
            bar_size: uniform(110.0)

            color: uniform(theme.color_d_hidden)
            color_hover: uniform(theme.color_outset_hover)
            color_drag: uniform(theme.color_outset_drag)
            color_focus: uniform(theme.color_val_focus)
            // The strip's GROUND (the gutter the grip bar floats in).
            // Overridable so a dark app is not forced to carry the theme's
            // panel gray through every splitter.
            color_bg: uniform(theme.color_bg_app)

            /** corner rounding radius 0..8 step 0.5 */
            border_radius: uniform(1.0)
            /** the bar's inset from the two long edges in pixels 0..6 step 0.5 */
            splitter_pad: uniform(1.0)

            // Three marks on the bar, so it reads as something to take hold
            // of rather than as a seam between two panels. A radius of
            // nothing is no marks at all, which is what the dock's bars
            // have always had.
            /** radius of one grip mark in pixels, 0 for none 0..4 step 0.25 */
            grip_dot: uniform(0.0)
            /** centre-to-centre spacing of the grip marks in pixels 2..16 step 0.5 */
            grip_gap: uniform(5.0)
            grip_color: uniform(theme.color_label_outer_off)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(self.color_bg)

                // With no focus this is the hover mix and nothing else: a
                // mix by zero hands back its first argument untouched.
                let bar = self.color
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_focus, self.focus)

                let mx = self.rect_size.x * 0.5
                let my = self.rect_size.y * 0.5
                let gap = self.grip_gap

                // The marks are laid ALONG the bar. Marks across it would
                // be wider than the bar is thick and would be cut off by it.
                if self.is_vertical > 0.5 {
                    sdf.box(
                        self.splitter_pad
                        my - self.bar_size * 0.5
                        self.rect_size.x - 2.0 * self.splitter_pad
                        self.bar_size
                        self.border_radius
                    )
                    sdf.fill(bar)
                    if self.grip_dot > 0.0 {
                        sdf.circle(mx, my - gap, self.grip_dot)
                        sdf.fill(self.grip_color)
                        sdf.circle(mx, my, self.grip_dot)
                        sdf.fill(self.grip_color)
                        sdf.circle(mx, my + gap, self.grip_dot)
                        sdf.fill(self.grip_color)
                    }
                }
                else {
                    sdf.box(
                        mx - self.bar_size * 0.5
                        self.splitter_pad
                        self.bar_size
                        self.rect_size.y - 2.0 * self.splitter_pad
                        self.border_radius
                    )
                    sdf.fill(bar)
                    if self.grip_dot > 0.0 {
                        sdf.circle(mx - gap, my, self.grip_dot)
                        sdf.fill(self.grip_color)
                        sdf.circle(mx, my, self.grip_dot)
                        sdf.fill(self.grip_color)
                        sdf.circle(mx + gap, my, self.grip_dot)
                        sdf.fill(self.grip_color)
                    }
                }

                return sdf.result
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
            // Only a bar with a `key_step` is ever given the focus, so on
            // every other splitter this track is never played.
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

    // Splitter and the enums are written out in full: this block's
    // `use mod.widgets.*` is a snapshot taken before any of them was
    // registered, so the bare names are not in scope here.
    /** Two panes and a draggable bar whose per-pane minimums are a law: a
     * grip on the bar, the keyboard, and a folded pane the bar brings back. */
    mod.widgets.SplitPane = mod.widgets.Splitter{
        exact_floors: true
        key_step: 16.0
        bar_reopens: true

        /** the bar's thickness in pixels 2..24 step 1 */
        size: 8.0
        align: mod.widgets.SplitterAlign.FromA(240.0)
        // All four, because the floors are named for the bar and which
        // pair is read follows the axis: a SplitPane stood on end keeps
        // the same floors it had lying down.
        min_vertical: 80.0
        max_vertical: 80.0
        min_horizontal: 80.0
        max_horizontal: 80.0

        draw_bg +: {
            bar_size: 44.0
            border_radius: 2.0
            grip_dot: 1.25
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
    /// The same kind of align, saying `position` in a room this long.
    fn with_position(self, position: f64, room: f64) -> Self {
        match self {
            Self::FromA(_) => Self::FromA(position),
            Self::FromB(_) => Self::FromB(room - position),
            // No room is no share of it either; the share it had is as good
            // an answer as any and better than a division by nothing.
            Self::Weighted(weight) => {
                Self::Weighted(if room > 0.0 { position / room } else { weight })
            }
        }
    }

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

/// Clamp a split position into the room both panes' floors allow.
///
/// This is applied to a drag, and by the layout pass to an align while
/// neither pane is folded. **It is never applied to a fold**, and that is
/// the division that matters: a host closing a panel is making a decision,
/// not sliding a bar, and a floor applied to that does not enforce a rule,
/// it overrules the caller — the panel stops closing and leaves a gutter
/// the width of its own floor. So a host closes a panel with `collapse` and
/// not by writing a position of zero, which the floor would bring back.
///
/// The floors are measured against the WHOLE room, the bar included, so the
/// bar's thickness comes out of what the second pane is left. The dock is
/// laid out by exactly this and it is not to change; `Law` is the
/// arithmetic that gives each pane its whole floor.
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

/// The room a split has this pass and the floors that govern it, for a
/// splitter that holds its align to them as a law (`exact_floors`).
///
/// Pure arithmetic in a plain struct, for two reasons. It can be tested
/// without a script heap; and the drag, the keyboard, the host's setter and
/// the layout pass all read the rule from this one place, which is what
/// makes it a law rather than a habit that three of the four share.
///
/// It is not `clamp_split_position` with the bar taken off, and the two
/// cannot be folded into one: that one refuses floors larger than the room
/// by going to the middle, this one by sharing the shortfall, and the dock
/// is laid out by that one.
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

    /// Where the bar actually stands: hard against an edge when a pane is
    /// folded, and inside the floors otherwise.
    ///
    /// The floors are not consulted for a folded pane. That is what folding
    /// means, and it is the one hole in the law — a deliberate one, and the
    /// reason the law can be absolute everywhere else.
    fn resolve(&self, position: f64, collapse: SplitterCollapse) -> f64 {
        match collapse {
            SplitterCollapse::A => 0.0,
            // The room LESS the bar, for the reason `resolve_split_position`
            // gives: a first pane handed everything leaves the bar nothing,
            // and the handle that reopens the panel goes with it.
            SplitterCollapse::B => self.free(),
            SplitterCollapse::None => self.clamp(position),
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplitter {
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

    // Runtime state: a dragged split and a host-set axis (`set_axis`, e.g. a
    // compact-mode relayout) must survive a stylesheet `ScriptReapply`, which
    // walks the same DSL source again. Explicit edits and source reloads still apply.
    #[live(SplitterAxis::Horizontal)]
    #[apply_state]
    pub axis: SplitterAxis,
    #[live(SplitterAlign::Weighted(0.5))]
    #[apply_state]
    pub align: SplitterAlign,
    /// Which pane is folded away, if either. Kept apart from `align` so
    /// unfolding restores the bar the user left, with nobody remembering it.
    #[live(SplitterCollapse::None)]
    pub collapse: SplitterCollapse,

    /// Hold the align to the floors as a law, not as the rule of thumb the
    /// dock was built on.
    ///
    /// Off, the floors are the ones this widget has always had. Each stops
    /// the bar that many points short of an edge of the WHOLE room, so the
    /// bar's own thickness comes out of the second pane's floor; a room too
    /// small for both puts the bar in the middle; a drag starts from what
    /// the align asks for, even while the floors are holding the bar
    /// somewhere else; and the drag rewrites the align as a share near the
    /// middle and as points near an edge.
    ///
    /// On, the floors are measured in the room the two panes actually share
    /// and a room too small for both is divided so each misses its floor by
    /// the same share (`Law`). `align` is then what was ASKED for and
    /// `position()` is where the bar stands, which is what lets a window
    /// squeezed and opened out again put the bar back. A hand takes the bar
    /// from where it stands and its answer is written back in the align's
    /// own kind, so a host that persists points is not handed a share; and
    /// `position()` answers a `set_align` at once rather than at the next
    /// draw, because a host that wrote an ask wants to know what it got.
    #[live(false)]
    pub exact_floors: bool,
    /// Points one arrow key press moves the bar. Nothing, which is the
    /// default, keeps the bar off the keyboard altogether: no focus stop,
    /// and a press does not take the focus from whatever had it. Anything
    /// more makes the bar a focus stop, the arrows move it, and Home and End
    /// send it to the two clamps, which is the only way to reach them
    /// exactly.
    #[live(0.0)]
    pub key_step: f64,
    /// Whether the bar of a folded pane brings the pane back: a drag out of
    /// the fold, or an arrow key when there is a `key_step`. Off, a folded
    /// splitter's bar is dead and only the host unfolds it, which is what
    /// the hosts that fold from a button of their own rely on.
    #[live(false)]
    pub bar_reopens: bool,

    #[rust]
    rect: Rect,
    #[rust]
    position: f64,
    #[rust]
    drag_start_align: Option<SplitterAlign>,
    /// Whether the drag in hand has the bar anywhere but where it took it.
    #[rust]
    drag_moved: bool,
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
    /// The bar moved. Sent on every frame of a drag and on every key press,
    /// for a readout that follows the hand.
    Changed {
        axis: SplitterAxis,
        align: SplitterAlign,
    },
    /// The gesture is over, and this is the align to persist. A click on
    /// the way past that moved nothing does not send it.
    Settled {
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
            // A folded splitter's bar is dead unless it was told to bring
            // the pane back: a press that landed on it must not drag the
            // pane out again, which is why three callers used to force the
            // align back to zero every pass.
            Hit::FingerDown(_)
                if self.collapse != SplitterCollapse::None && !self.bar_reopens => {}
            Hit::FingerDown(fe) if self.drag_start_align.is_none() && fe.is_primary_hit() => {
                if self.key_step > 0.0 {
                    cx.set_key_focus(self.draw_bg.area());
                }
                match self.axis {
                    SplitterAxis::Horizontal => cx.set_cursor(MouseCursor::ColResize),
                    SplitterAxis::Vertical => cx.set_cursor(MouseCursor::RowResize),
                }
                self.animator_play(cx, ids!(hover.drag));
                // Under the law, and out of a fold, the hand takes the bar
                // from where it STANDS, so the bar tracks the hand rather
                // than waiting for it to make up the difference between the
                // ask and the floor. Otherwise it starts from the align, as
                // it always has.
                self.drag_start_align = Some(
                    if self.exact_floors || self.collapse != SplitterCollapse::None {
                        SplitterAlign::FromA(self.position)
                    } else {
                        self.align
                    },
                );
                self.drag_moved = false;
            }
            Hit::FingerUp(f) => {
                self.drag_start_align = None;
                if f.is_over && f.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                // Only a drag that actually moved the bar has an align
                // worth writing down; a click on the way past does not.
                if std::mem::take(&mut self.drag_moved) {
                    cx.widget_action_with_data(
                        &self.action_data,
                        uid,
                        SplitterAction::Settled {
                            axis: self.axis,
                            align: self.align,
                        },
                    );
                }
            }
            Hit::FingerMove(f) => {
                if let Some(drag_start_align) = self.drag_start_align {
                    let delta = match self.axis {
                        SplitterAxis::Horizontal => f.abs.x - f.abs_start.x,
                        SplitterAxis::Vertical => f.abs.y - f.abs_start.y,
                    };
                    // A press alone must not bring a folded pane back: the
                    // bar is also the thing you click on your way to
                    // something else.
                    let reopening = self.bar_reopens && self.collapse != SplitterCollapse::None;
                    if !reopening || delta != 0.0 {
                        // A pane folded away comes back at its floor, which
                        // is the nearest legal size to where the bar is
                        // standing. The bar then waits at the floor until
                        // the finger has travelled past it, so the two meet
                        // up rather than the bar leaping to the hand.
                        if reopening {
                            self.collapse = SplitterCollapse::None;
                            cx.widget_action(uid, SplitterAction::Collapsed(SplitterCollapse::None));
                            self.redraw(cx);
                        }
                        let start = drag_start_align.to_position(self.axis, self.rect);
                        let new_position = self.clamp(start + delta);
                        self.drag_moved = new_position != start;
                        self.bar_to(cx, new_position);
                    }
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            Hit::KeyDown(ke) if self.key_step > 0.0 => {
                let (lo, hi) = self.limits();
                let step = self.key_step.max(1.0);
                // Left and up take room from the first pane whichever way
                // the split runs, so the key that means "less" is the one
                // pointing at the pane that loses.
                let want = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowUp => Some(self.clamp(self.position - step)),
                    KeyCode::ArrowRight | KeyCode::ArrowDown => Some(self.clamp(self.position + step)),
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
                    if self.collapse != SplitterCollapse::None {
                        if self.bar_reopens {
                            self.set_collapse(cx, SplitterCollapse::None);
                        }
                    } else {
                        self.bar_to(cx, want);
                        // The next layout pass would say the same. Said now,
                        // a second key before that pass steps from here and
                        // not from where the bar was.
                        self.position = want;
                        cx.widget_action_with_data(
                            &self.action_data,
                            uid,
                            SplitterAction::Settled {
                                axis: self.axis,
                                align: self.align,
                            },
                        );
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
        // Folded hard to an edge, or wherever the align says inside the
        // floors. The align itself is left alone, so a window squeezed
        // until a pane is against its floor and then opened out again
        // returns the bar to what was asked for.
        self.position = self.resolve();

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
        // The bar is the control, so the bar is what the keyboard reaches.
        if self.key_step > 0.0 {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::Slider, Inset::default());
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

    /// The whole span along the axis, as last laid out: both panes and the
    /// bar together.
    fn room(&self) -> f64 {
        match self.axis {
            SplitterAxis::Horizontal => self.rect.size.x,
            SplitterAxis::Vertical => self.rect.size.y,
        }
    }

    /// Whether the widget has been laid out yet. Before that there is no
    /// room, so there is nothing for the floors to work with.
    fn laid_out(&self) -> bool {
        self.room() > 0.0
    }

    /// The room and the floors as they stand this pass. Built fresh every
    /// time: all of them are live properties and the tweaker may have moved
    /// any of them since the last draw.
    fn law(&self) -> Law {
        let (min_a, min_b) = self.axis_min_max();
        Law {
            room: self.room(),
            bar: self.size.max(0.0),
            min_a: min_a.max(0.0),
            min_b: min_b.max(0.0),
        }
    }

    /// A position brought inside the floors, by whichever arithmetic this
    /// splitter keeps them with.
    fn clamp(&self, position: f64) -> f64 {
        if self.exact_floors {
            self.law().clamp(position)
        } else {
            let (min_a, min_b) = self.axis_min_max();
            clamp_split_position(position, self.room(), min_a, min_b)
        }
    }

    /// Where the bar stands for the align, the fold and the room as they
    /// are now.
    fn resolve(&self) -> f64 {
        let asked = self.align.to_position(self.axis, self.rect);
        if self.exact_floors {
            self.law().resolve(asked, self.collapse)
        } else {
            let (min_a, min_b) = self.axis_min_max();
            resolve_split_position(asked, self.room(), min_a, min_b, self.collapse, self.size)
        }
    }

    /// Put the bar at a position the floors have already passed, on behalf
    /// of a hand or a key, and say so.
    ///
    /// The align is set to the clamped position rather than the raw one:
    /// this came from somebody pushing the bar, and a push past the floor is
    /// a request for the floor. Only a host may hold an ask the window
    /// cannot currently honour, because only a host has a reason to — it is
    /// asking for a layout, not for a place on this screen.
    fn bar_to(&mut self, cx: &mut Cx, new_position: f64) {
        let uid = self.widget_uid();
        let room = self.room();
        if self.exact_floors {
            // A hand leaning on a clamp has moved nothing.
            if new_position == self.position {
                return;
            }
            self.position = new_position;
            self.align = self.align.with_position(new_position, room);
        } else {
            let center = room / 2.0;
            self.align = if new_position < center - 30.0 {
                SplitterAlign::FromA(new_position)
            } else if new_position > center + 30.0 {
                SplitterAlign::FromB(room - new_position)
            } else {
                SplitterAlign::Weighted(new_position / room)
            };
        }
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
            self.answer_at_once();
            let uid = self.widget_uid();
            cx.widget_action(uid, SplitterAction::Collapsed(collapse));
            self.redraw(cx);
        }
    }

    /// Where the bar stands, in points from the first pane's edge: the
    /// align, brought inside the floors and the room that exists.
    pub fn position(&self) -> f64 {
        // Under the law, a splitter not laid out yet has no room to bring
        // anything inside, and the ask is the only honest answer to give.
        if self.exact_floors && !self.laid_out() {
            self.asked_position()
        } else {
            self.position
        }
    }

    /// What the align asks for, in the same points. It parts company with
    /// `position()` exactly when the floors or the window cannot honour it,
    /// and it is the align, not the position, that a host persists: a
    /// window that was briefly too small must not shrink a saved layout for
    /// good.
    pub fn asked_position(&self) -> f64 {
        self.align.to_position(self.axis, self.rect)
    }

    /// The two positions the floors allow, as the keyboard's Home and End
    /// reach them. A host that wants to show them can ask.
    pub fn limits(&self) -> (f64, f64) {
        if self.exact_floors {
            self.law().limits()
        } else {
            (self.clamp(f64::NEG_INFINITY), self.clamp(f64::INFINITY))
        }
    }

    pub fn set_align(&mut self, align: SplitterAlign) {
        self.align = align;
        self.answer_at_once();
    }

    /// Under the law a host that wrote an ask can read what it got without
    /// waiting for a draw. Otherwise the position is the layout pass's to
    /// set and nobody else's, as it always was.
    fn answer_at_once(&mut self) {
        if self.exact_floors && self.laid_out() {
            self.position = self.resolve();
        }
    }

    /// The bar between the panes, widened by the slop a drag grabs it
    /// with. Empty until it has drawn.
    ///
    /// The clipped rect, because this answers a question about a point
    /// on screen and that is the frame `hits` works in. Six points of
    /// painted bar is too thin to aim anything at, which is why the
    /// drag has slop in the first place; whatever wants to know where
    /// the bar IS wants the same answer.
    pub fn bar_grab_rect(&self, cx: &Cx) -> Rect {
        let area = self.draw_bg.area();
        if !area.is_valid(cx) {
            return Rect::default();
        }
        // axis_inset is symmetric and zero along the axis, so one call
        // widens the strip across it and leaves its length alone.
        let slop = self.margin();
        area.clipped_rect(cx).add_margin(dvec2(slop.left, slop.top))
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

    // These read every action of the pass and not the first alone: a key
    // press reports a change and its settling together, and a drag out of a
    // fold reports the unfolding and the change together.
    pub fn changed(&self, actions: &Actions) -> Option<(SplitterAxis, SplitterAlign)> {
        actions
            .filter_widget_actions_cast::<SplitterAction>(self.widget_uid())
            .find_map(|action| match action {
                SplitterAction::Changed { axis, align } => Some((axis, align)),
                _ => None,
            })
    }

    /// The align the gesture settled on: the one to write down.
    pub fn settled(&self, actions: &Actions) -> Option<(SplitterAxis, SplitterAlign)> {
        actions
            .filter_widget_actions_cast::<SplitterAction>(self.widget_uid())
            .find_map(|action| match action {
                SplitterAction::Settled { axis, align } => Some((axis, align)),
                _ => None,
            })
    }

    /// The fold this splitter reports this pass, if it changed.
    pub fn collapsed(&self, actions: &Actions) -> Option<SplitterCollapse> {
        actions
            .filter_widget_actions_cast::<SplitterAction>(self.widget_uid())
            .find_map(|action| match action {
                SplitterAction::Collapsed(collapse) => Some(collapse),
                _ => None,
            })
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
        self.borrow().and_then(|inner| inner.collapsed(actions))
    }

    /// The align the gesture settled on: the one to write down.
    pub fn settled(&self, actions: &Actions) -> Option<(SplitterAxis, SplitterAlign)> {
        self.borrow().and_then(|inner| inner.settled(actions))
    }

    pub fn position(&self) -> Option<f64> {
        self.borrow().map(|inner| inner.position())
    }

    /// What the align asks for, in points from the first pane's edge.
    pub fn asked_position(&self) -> Option<f64> {
        self.borrow().map(|inner| inner.asked_position())
    }

    /// The two positions the floors allow.
    pub fn limits(&self) -> Option<(f64, f64)> {
        self.borrow().map(|inner| inner.limits())
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
    /// not applied to it — when the host says so with a fold. A position of
    /// zero written on an unfolded splitter meets the floor like any other,
    /// which left an empty gutter the width of the floor where three apps
    /// wanted a closed panel, and is why folding became a state. This test
    /// is that division, written down against what the layout pass calls.
    #[test]
    fn a_host_may_collapse_past_a_floor_that_a_drag_may_not() {
        let (room, bar) = (900.0, 6.0);
        // What the layout pass does with a deliberate collapse.
        assert_eq!(
            resolve_split_position(244.0, room, 180.0, 50.0, SplitterCollapse::A, bar),
            0.0,
            "a host folding a pane away gets nothing left of it"
        );
        // What a DRAG to the same place does, on the same splitter.
        assert_eq!(
            clamp_split_position(0.0, room, 180.0, 50.0),
            180.0,
            "a finger on the bar still stops at the floor"
        );
        // And what it does with a zero that was written rather than folded.
        assert_eq!(
            resolve_split_position(0.0, room, 180.0, 50.0, SplitterCollapse::None, bar),
            180.0,
            "which is why a host folds and does not write zero"
        );
        // The layout pass still refuses a pane wider than the window.
        assert_eq!(
            resolve_split_position(2000.0, room, 180.0, 50.0, SplitterCollapse::None, bar),
            room - 50.0
        );
    }
}

/// The floors as a law, which is what `exact_floors` turns on. These came
/// with the arithmetic from the widget that used to keep it to itself.
#[cfg(test)]
mod law_tests {
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

    /// A host writing past either clamp is brought back to it, and the far
    /// clamp leaves the second pane its WHOLE floor. The other arithmetic
    /// stops the bar fifty points short of the room's edge and lets the bar
    /// take its eight out of those fifty.
    #[test]
    fn an_align_past_either_clamp_is_pulled_back_to_it() {
        let law = wide();
        assert_eq!(law.clamp(0.0), 180.0, "the first pane keeps its floor");
        assert_eq!(law.clamp(-500.0), 180.0, "however far past it the ask went");
        assert_eq!(law.clamp(2000.0), 842.0, "and the second keeps its own");
        // 842 is the room, less the bar, less pane B's fifty points.
        assert_eq!(law.limits(), (180.0, 842.0));
        assert_eq!(
            clamp_split_position(2000.0, 900.0, 180.0, 50.0),
            850.0,
            "where the plain floors put the same ask"
        );
    }

    /// A window squeezed until a pane is against its floor moves the bar, but
    /// does not touch what was asked for — so opening the window out again
    /// puts the bar back where it was. This is why the ask and the position
    /// are two numbers and not one.
    #[test]
    fn a_squeeze_moves_the_bar_and_not_the_ask() {
        let asked = 400.0;
        let wide = wide();
        assert_eq!(wide.resolve(asked, SplitterCollapse::None), 400.0);

        // The same split in a window dragged down to 300 points. Pane B's
        // fifty are still B's, so the bar gives way.
        let narrow = Law { room: 300.0, ..wide };
        assert_eq!(narrow.resolve(asked, SplitterCollapse::None), 242.0);

        // And the ask, which was never written, brings the bar back.
        assert_eq!(wide.resolve(asked, SplitterCollapse::None), 400.0);
    }

    /// Squeezed past the point where both floors can be met, both panes miss
    /// their floor by the same share of it. Handing the whole shortfall to
    /// one of them is what makes a squeezed layout stop looking like itself.
    #[test]
    fn floors_that_cannot_both_be_met_are_missed_by_the_same_share() {
        let law = Law { room: 200.0, ..wide() };
        let a = law.resolve(400.0, SplitterCollapse::None);
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
        assert_eq!(law.resolve(400.0, SplitterCollapse::None), 0.0);
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
    /// returns the exact bar that went away, because the align was never
    /// written when it did.
    #[test]
    fn a_fold_goes_past_the_floors_and_reopening_returns_the_bar() {
        let law = wide();
        let left_at = 244.0;
        assert_eq!(
            law.resolve(left_at, SplitterCollapse::A),
            0.0,
            "folding the first pane leaves it nothing, floor or no floor"
        );
        assert_eq!(
            law.resolve(left_at, SplitterCollapse::B),
            892.0,
            "folding the second leaves the first everything except the bar, \
             which has to stay: it is the only way back"
        );
        assert_eq!(
            law.resolve(left_at, SplitterCollapse::None),
            left_at,
            "and reopening is the bar the person left"
        );
    }

    /// A pane folded away while the window was wide, reopened after the
    /// window has shrunk, comes back legal rather than coming back wrong.
    #[test]
    fn a_bar_remembered_from_a_wider_window_reopens_inside_the_floors() {
        let asked = 700.0;
        assert_eq!(wide().resolve(asked, SplitterCollapse::A), 0.0);
        let narrow = Law { room: 400.0, ..wide() };
        assert_eq!(narrow.resolve(asked, SplitterCollapse::None), 342.0);
    }

    /// One arrow key press is a step that stops at the clamp rather than
    /// walking through it, so holding a key down parks the bar on the floor.
    #[test]
    fn an_arrow_key_stops_at_the_clamp() {
        let law = wide();
        assert_eq!(law.clamp(400.0 + 16.0), 416.0);
        assert_eq!(law.clamp(190.0 - 16.0), 180.0, "not 174");
        assert_eq!(law.clamp(180.0 - 16.0), 180.0, "and it stays there");
        assert_eq!(law.clamp(840.0 + 16.0), 842.0);
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

    /// A drag under the law answers in the align's own kind, so a host that
    /// persists points is never handed a share of the window to put back.
    /// The plain splitter turns a bar left near the middle into a share,
    /// which is right for the dock and wrong for a saved sidebar width.
    #[test]
    fn a_moved_bar_keeps_the_kind_of_align_it_had() {
        let room = 900.0;
        assert!(matches!(
            SplitterAlign::FromA(240.0).with_position(450.0, room),
            SplitterAlign::FromA(p) if p == 450.0
        ));
        assert!(matches!(
            SplitterAlign::FromB(240.0).with_position(450.0, room),
            SplitterAlign::FromB(p) if p == 450.0
        ));
        assert!(matches!(
            SplitterAlign::Weighted(0.2).with_position(450.0, room),
            SplitterAlign::Weighted(w) if w == 0.5
        ));
        // No room is no share of it: the share it had is kept.
        assert!(matches!(
            SplitterAlign::Weighted(0.2).with_position(450.0, 0.0),
            SplitterAlign::Weighted(w) if w == 0.2
        ));
    }
}

/// The two declarations, as the script hands them over. A DSL slip shows up
/// nowhere in a build, so what each name sets is pinned here.
#[cfg(test)]
mod preset_tests {
    use super::*;

    fn built(vm: &mut ScriptVm, name: &str) -> Splitter {
        let value = match name {
            "Splitter" => crate::script_eval!(vm, {use mod.widgets.* Splitter{}}),
            _ => crate::script_eval!(vm, {use mod.widgets.* SplitPane{}}),
        };
        Splitter::script_from_value(vm, value)
    }

    /// Every new property is off on the plain splitter, which is what the
    /// dock is built from: its floors, its drag and its dead folded bar are
    /// the ones it has always had.
    #[test]
    fn the_plain_splitter_has_none_of_it_switched_on() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let splitter = built(vm, "Splitter");
            assert!(!splitter.exact_floors);
            assert_eq!(splitter.key_step, 0.0);
            assert!(!splitter.bar_reopens);
            assert_eq!(splitter.size, 6.0);
            assert_eq!(splitter.axis_min_max(), (50.0, 50.0));
            assert!(matches!(splitter.align(), SplitterAlign::Weighted(w) if w == 0.5));
        });
    }

    /// SplitPane is that widget with the law, the keyboard and the way back
    /// switched on, and no Rust of its own.
    #[test]
    fn split_pane_is_the_splitter_with_the_law_switched_on() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let mut pane = built(vm, "SplitPane");
            assert!(pane.exact_floors);
            assert_eq!(pane.key_step, 16.0);
            assert!(pane.bar_reopens);
            assert_eq!(pane.size, 8.0);
            assert!(matches!(pane.align(), SplitterAlign::FromA(p) if p == 240.0));
            assert_eq!(pane.axis_min_max(), (80.0, 80.0));
            // Stood on end it keeps the same floors: the preset sets both
            // pairs, because which pair is read follows the axis.
            pane.set_axis(SplitterAxis::Vertical);
            assert_eq!(pane.axis_min_max(), (80.0, 80.0));
        });
    }

    /// The same ask on the two of them, in the same room. The plain one
    /// answers as it always has; the law gives the second pane its whole
    /// floor, shares a shortfall, and answers a host at once.
    #[test]
    fn the_two_answer_the_same_ask_by_their_own_arithmetic() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let room = Rect { pos: dvec2(0.0, 0.0), size: dvec2(900.0, 300.0) };

            let mut plain = built(vm, "Splitter");
            plain.rect = room;
            plain.set_align(SplitterAlign::FromA(2000.0));
            assert_eq!(plain.position, 0.0, "the layout pass sets it, and none has run");
            assert_eq!(plain.resolve(), 850.0);
            assert_eq!(plain.limits(), (50.0, 850.0));

            let mut pane = built(vm, "SplitPane");
            pane.rect = room;
            pane.set_align(SplitterAlign::FromA(2000.0));
            assert_eq!(pane.position(), 812.0, "900 less the bar's 8 less B's 80");
            assert_eq!(pane.asked_position(), 2000.0, "and the ask is left alone");
            assert_eq!(pane.limits(), (80.0, 812.0));

            // Too narrow for both floors: the middle, against equal shares.
            let narrow = Rect { pos: dvec2(0.0, 0.0), size: dvec2(90.0, 300.0) };
            plain.rect = narrow;
            pane.rect = narrow;
            assert_eq!(plain.resolve(), 45.0);
            assert_eq!(pane.resolve(), 41.0, "half of the 82 the bar leaves");
        });
    }
}

#[cfg(test)]
mod style_reapply_tests {
    use super::*;
    use crate::desktop_style::{install, DesktopStyle, StyleSheet};

    #[test]
    fn style_reapply_preserves_dragged_split_and_axis() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let original = crate::script_eval!(vm, {use mod.widgets.* Splitter{}});
            let mut splitter = Splitter::script_from_value(vm, original);
            assert!(matches!(splitter.align(), SplitterAlign::Weighted(w) if w == 0.5));
            assert!(matches!(splitter.axis(), SplitterAxis::Horizontal));
            // A drag and a host relayout (compact mode) happened at runtime.
            splitter.set_align(SplitterAlign::FromA(120.0));
            splitter.set_axis(SplitterAxis::Vertical);
            for (style, dark) in [(DesktopStyle::Windows, false), (DesktopStyle::Macos, true), (DesktopStyle::Omarchy, false)] {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.with_reload(crate::script_mod);
                splitter.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), original);
                assert!(
                    matches!(splitter.align(), SplitterAlign::FromA(p) if p == 120.0),
                    "{}: a dragged split must survive a style change, got {:?}",
                    style.id(),
                    splitter.align()
                );
                assert!(matches!(splitter.axis(), SplitterAxis::Vertical), "{}", style.id());
            }
        });
    }
}

#[cfg(test)]
mod style_reapply_tests {
    use super::*;
    use crate::desktop_style::{install, DesktopStyle, StyleSheet};

    #[test]
    fn style_reapply_preserves_dragged_split_and_axis() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let original = crate::script_eval!(vm, {use mod.widgets.* Splitter{}});
            let mut splitter = Splitter::script_from_value(vm, original);
            assert!(matches!(splitter.align(), SplitterAlign::Weighted(w) if w == 0.5));
            assert!(matches!(splitter.axis(), SplitterAxis::Horizontal));
            // A drag and a host relayout (compact mode) happened at runtime.
            splitter.set_align(SplitterAlign::FromA(120.0));
            splitter.set_axis(SplitterAxis::Vertical);
            for (style, dark) in [(DesktopStyle::Windows, false), (DesktopStyle::Macos, true), (DesktopStyle::Omarchy, false)] {
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.with_reload(crate::script_mod);
                splitter.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), original);
                assert!(
                    matches!(splitter.align(), SplitterAlign::FromA(p) if p == 120.0),
                    "{}: a dragged split must survive a style change, got {:?}",
                    style.id(),
                    splitter.align()
                );
                assert!(matches!(splitter.axis(), SplitterAxis::Vertical), "{}", style.id());
            }
        });
    }
}
