//! ButtonGroup, SegmentedControl and the two helpers they are built from —
//! the widgets that say "these controls belong together, and here is how".
//!
//! Two shapes, one file, because they are the same idea seen from two
//! sides. A BUTTON GROUP is a set of separate verbs that happen to sit
//! together: cut, copy, paste. A SEGMENTED CONTROL is one question with a
//! fixed set of answers: day, week, month. The first reports which button
//! was pressed and forgets; the second remembers which answer is current
//! and shows it. Confusing them is the commonest mistake in this corner of
//! a design system, so the two are named apart and behave apart, and the
//! doc for each says which question it answers.
//!
//! `ButtonGroup` only lays its children out and shapes their corners:
//! CONNECTED collapses the inner radii so the row reads as one object with
//! square joints and full outer corners, SPACED leaves each button its own
//! shape. It does not touch what a button does, so any button in the
//! library can go in one, and the group can be handed a fresh preset it
//! has never heard of.
//!
//! `SegmentedControl` owns its answers as data rather than as children:
//! `options` is a list of words, and the control draws them, tracks which
//! is current, and reports the index. That is deliberate. A segmented
//! control built from child widgets makes the caller keep the children and
//! the selection in step by hand, and every caller gets it slightly wrong;
//! a control that owns the list cannot get out of step with itself.
//!
//! The moving pill is a [`SlidingIndicator`]: a rectangle that eases from
//! where it was to where it should be over `theme.motion_short_4`. It is
//! its own small type because the tab strip, the segmented control and the
//! nav bar all need the same thing, and three copies of an easing rectangle
//! is how three of them come to disagree about how fast a selection moves.
//!
//! [`OverflowRow`] measures a row of items against the width it was given
//! and answers how many fit, so a toolbar can put the rest behind a "more"
//! control. It is arithmetic with no widget attached, so the thing that
//! shows the remainder — a menu, a popover, a second row — stays the
//! caller's choice.
//!
//! [`ConcessionLadder`] answers the other crowding question. An overflowing
//! row hides whole items and counts them; a row that is merely tight gives
//! things up in an order someone decided — a label becomes a glyph, a
//! title goes, a word shortens — and stops the moment it fits. That is a
//! priority ladder rather than a count of what fits, and the hard half of
//! it is not going down but coming back up without the row flapping between
//! two states while the window is dragged, so it lives here as arithmetic
//! too.

use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    button::ButtonWidgetRefExt,
    badge::{measure, BadgeIntent, BadgePalette},
    chip::ChipSelection,
    makepad_derive_widget::*,
    makepad_draw::*,
    menu::{MenuAction, MenuPlace, MenuRow},
    view::View,
    widget::*,
};

/// How a group joins its children.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum GroupJoin {
    /// One object: inner corners square, outer corners rounded, no gaps.
    #[pick]
    Connected = 0,
    /// Separate objects sharing a row: every corner rounded, gaps between.
    Spaced = 1,
}

/// Which way a group runs.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum GroupAxis {
    #[pick]
    Horizontal = 0,
    Vertical = 1,
}

/// A rectangle that eases from where it was to where it should be.
///
/// The indicator holds two rects and a clock: the one it started from, the
/// one it is going to, and how far along it is. Callers set the target
/// every draw and read [`Self::current`]; nothing else about the moving
/// selection has to be stored anywhere.
#[derive(Clone, Debug, Default)]
pub struct SlidingIndicator {
    from: Rect,
    to: Rect,
    /// 0 at the start of the glide, 1 when it has arrived.
    t: f64,
    seconds: f64,
}

impl SlidingIndicator {
    /// Aim at `target`. A first target is taken instantly (there is nowhere
    /// to slide from); a new one starts a glide from wherever the indicator
    /// is right now, so a second press mid-flight bends the path instead of
    /// jumping back.
    pub fn aim(&mut self, target: Rect, seconds: f64) {
        if self.to == target {
            return;
        }
        if self.to.size.x == 0.0 && self.to.size.y == 0.0 {
            self.from = target;
            self.to = target;
            self.t = 1.0;
        } else {
            self.from = self.current();
            self.to = target;
            self.t = 0.0;
        }
        self.seconds = seconds.max(0.0);
    }

    /// Advance the glide by `dt` seconds. Answers whether it is still
    /// moving, so a caller knows whether to ask for another frame.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.t >= 1.0 {
            return false;
        }
        if self.seconds <= 0.0 {
            self.t = 1.0;
            return false;
        }
        self.t = (self.t + dt / self.seconds).min(1.0);
        self.t < 1.0
    }

    /// Where the indicator is now.
    pub fn current(&self) -> Rect {
        let e = ease_out_cubic(self.t);
        Rect {
            pos: dvec2(
                self.from.pos.x + (self.to.pos.x - self.from.pos.x) * e,
                self.from.pos.y + (self.to.pos.y - self.from.pos.y) * e,
            ),
            size: dvec2(
                self.from.size.x + (self.to.size.x - self.from.size.x) * e,
                self.from.size.y + (self.to.size.y - self.from.size.y) * e,
            ),
        }
    }

    /// True while the indicator has never been aimed at anything.
    pub fn is_empty(&self) -> bool {
        self.to.size.x == 0.0 && self.to.size.y == 0.0
    }

    /// How far along the glide is, 0 at its start and 1 once it has
    /// arrived. A caller that fades something with the glide reads this
    /// rather than keeping a second clock that could drift from the first.
    pub fn progress(&self) -> f64 {
        self.t
    }

    /// Land on the target now. For motion that has been switched off: the
    /// next `current` is the target, and `step` asks for no more frames.
    pub fn settle(&mut self) {
        self.t = 1.0;
    }
}

pub(crate) fn ease_out_cubic(t: f64) -> f64 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// How many of `widths` fit in `room`, once space is kept for the control
/// that shows the rest.
///
/// Answers `(fitting, hidden)`. The rule that matters is the second pass:
/// if everything fits, no "more" control is needed and none is measured; if
/// it does not, room for it must come out of the same width, which can
/// push one more item out. Getting that wrong is how a toolbar ends up with
/// a "more" button that overlaps its last item.
pub fn overflow_split(widths: &[f64], gap: f64, room: f64, more_width: f64) -> (usize, usize) {
    let total: f64 = widths.iter().sum::<f64>() + gap * widths.len().saturating_sub(1) as f64;
    if total <= room {
        return (widths.len(), 0);
    }
    let mut used = more_width;
    let mut fitting = 0;
    for w in widths {
        let step = if fitting == 0 { *w } else { *w + gap };
        if used + step > room {
            break;
        }
        used += step;
        fitting += 1;
    }
    (fitting, widths.len() - fitting)
}

/// The measuring half of an overflowing row, with no widget attached: the
/// thing that shows the remainder stays the caller's choice.
#[derive(Clone, Debug, Default)]
pub struct OverflowRow {
    widths: Vec<f64>,
    gap: f64,
    more_width: f64,
}

impl OverflowRow {
    pub fn new(gap: f64, more_width: f64) -> Self {
        Self { widths: Vec::new(), gap, more_width }
    }

    pub fn set_widths(&mut self, widths: Vec<f64>) {
        self.widths = widths;
    }

    /// `(fitting, hidden)` for the room given.
    pub fn split(&self, room: f64) -> (usize, usize) {
        overflow_split(&self.widths, self.gap, room, self.more_width)
    }
}

/// How far down a ladder of concessions a crowded row must go before it
/// fits, and how far back up it may come once it is given room again.
///
/// A CONCESSION is something a tight row gives up to save width, and the
/// ladder is those concessions in the order someone decided they should be
/// made: the cheapest loss first. `costs[i]` is what concession `i` saves,
/// `applied` is how many are in force, and `slack` is what the row had left
/// over when it was last drawn AT THAT LEVEL — for a row that ends in a
/// filler, the filler's own width. The answer is the level to be at now.
///
/// A level, not a verb and not a pair like [`overflow_split`]: there is
/// nothing on the other side to count, and a caller whose answer equals
/// `applied` has nothing to do, which is how a row that has settled costs
/// nothing per frame instead of relaying itself out forever.
///
/// ONE RULE GOVERNS BOTH DIRECTIONS: the row keeps at least `margin` of
/// slack. Concessions are made while it has less than that, and one is
/// given back only when giving it back would still leave that much. The two
/// thresholds for a single step therefore sit `costs[i]` apart rather than
/// on top of each other, and the comparisons are complementary — `<` going
/// down, `>=` coming back — so a slack sitting exactly on a boundary has
/// one answer and not two. A row that decides both ways at one value flaps
/// between two states on a single pixel while the window is dragged, which
/// is worse than a row that stays narrow.
///
/// `margin` of zero is allowed and still does not flap, but a caller
/// measuring a filler should not ask for it: a filler is never narrower
/// than nothing, so an exactly full row and a badly overfull one both
/// report zero, and a row with no margin has no threshold it can actually
/// observe. A pixel or two buys both a visible gap and a measurable one.
///
/// A cost of zero, or one that is not a finite number, means NOT KNOWN. A
/// caller only learns what a concession saves by making it and measuring
/// the row again, so the first pass down the ladder is walked blind, and
/// the two directions are not equally forgiving about that. Going down, an
/// unknown step is taken and the pass ENDS: what that step saves decides
/// everything after it, and one more draw settles what no arithmetic here
/// could. Coming back up, an unknown step is NOT given back at all, because
/// handing back a saving of unknown size is exactly how a row comes to
/// overflow a second time.
pub fn concession_level(costs: &[f64], margin: f64, applied: usize, slack: f64) -> usize {
    let mut level = applied.min(costs.len());
    // No measurement is not a reason to move: an unmeasurable width leaves
    // the row exactly as it was drawn.
    if !slack.is_finite() {
        return level;
    }
    let known = |i: usize| costs.get(i).copied().filter(|c| c.is_finite() && *c > 0.0);
    let margin = margin.max(0.0);
    let mut slack = slack;
    if slack < margin {
        while slack < margin && level < costs.len() {
            let Some(cost) = known(level) else {
                return level + 1;
            };
            slack += cost;
            level += 1;
        }
        return level;
    }
    // Back up in the reverse of the order they were made: the last thing
    // given up is the first thing owed back.
    while level > 0 {
        let Some(cost) = known(level - 1) else {
            break;
        };
        if slack - cost < margin {
            break;
        }
        slack -= cost;
        level -= 1;
    }
    level
}

/// The measuring half of a row that gives way, with no widget attached:
/// WHAT each concession does to the row — a label swapped for a glyph, a
/// title dropped — stays the caller's business.
#[derive(Clone, Debug, Default)]
pub struct ConcessionLadder {
    costs: Vec<f64>,
    margin: f64,
    level: usize,
}

impl ConcessionLadder {
    /// A ladder of `steps` concessions, none of their costs known yet.
    pub fn new(steps: usize, margin: f64) -> Self {
        Self { costs: vec![0.0; steps], margin, level: 0 }
    }

    /// What a concession turned out to save, once the caller has seen the
    /// row drawn with and without it.
    pub fn set_cost(&mut self, step: usize, cost: f64) {
        if let Some(slot) = self.costs.get_mut(step) {
            *slot = cost;
        }
    }

    /// How many concessions are in force.
    /// Forget every learned cost, leaving the level where it is.
    ///
    /// A cost is only true for the strings and the theme it was measured under.
    /// A row that re-prices from scratch each frame clears them first rather
    /// than carrying a number it can no longer stand behind; zero reads as NOT
    /// KNOWN, which is the honest state for a cost nobody has measured.
    pub fn clear_costs(&mut self) {
        for c in self.costs.iter_mut() {
            *c = 0.0;
        }
    }

    /// What each rung is currently believed to save. Zero means NOT KNOWN.
    pub fn costs(&self) -> &[f64] {
        &self.costs
    }

    pub fn level(&self) -> usize {
        self.level
    }

    /// Whether concession `step` is in force, which is the question the
    /// drawing side actually asks: is the title still there, is that label
    /// still a word.
    pub fn is_conceded(&self, step: usize) -> bool {
        step < self.level
    }

    /// Feed in the slack the row had when it was last drawn. Answers
    /// whether the level MOVED, so a caller asks for a redraw only when
    /// something actually changed — a row that has settled must cost
    /// nothing per frame, or the measuring itself becomes the jitter.
    pub fn measured(&mut self, slack: f64) -> bool {
        let next = concession_level(&self.costs, self.margin, self.level, slack);
        let moved = next != self.level;
        self.level = next;
        moved
    }
}

/// What a segmented control reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum SegmentedControlAction {
    /// The current answer changed, with its index into `options`.
    Selected(usize),
    #[default]
    None,
}

/// The pill behind the current answer, and the row it sits in.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSegmentedBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    radius: f32,
}

/// The chevron on a split button's trailing half. Drawn rather than typed:
/// the text font carries no such glyph, and an icon file for one triangle is
/// a resource to keep in step for no gain.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChevron {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.GroupJoin = set_type_default() do #(GroupJoin::script_api(vm))
    mod.widgets.splat(mod.widgets.GroupJoin)
    mod.widgets.GroupAxis = set_type_default() do #(GroupAxis::script_api(vm))

    use mod.widgets.*

    mod.widgets.DrawChevronBase = #(DrawChevron::script_component(vm))
    set_type_default() do #(DrawChevron::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: theme.color_text
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            let w = min(self.rect_size.x, self.rect_size.y) * 0.26
            sdf.move_to(c.x - w, c.y - w * 0.55)
            sdf.line_to(c.x, c.y + w * 0.55)
            sdf.line_to(c.x + w, c.y - w * 0.55)
            sdf.stroke(self.color, max(1.0, w * 0.30))
            return sdf.result
        }
    }

    mod.widgets.DrawSegmentedBgBase = #(DrawSegmentedBg::script_component(vm))
    set_type_default() do #(DrawSegmentedBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ButtonGroupBase = #(ButtonGroup::register_widget(vm))
    /** Buttons that belong together: separate verbs sharing a row. */
    mod.widgets.ButtonGroup = set_type_default() do mod.widgets.ButtonGroupBase{
        width: Fit
        height: Fit
        flow: Right
        /** Connected reads as one object, Spaced as several: Connected Spaced */
        join: Connected
        /** Horizontal Vertical */
        axis: GroupAxis.Horizontal
        /** gap between spaced buttons; a connected group closes it 0..24 step 1 */
        spacing: 0.
        /** the corner the outside of a connected group keeps 0..24 step 0.5 */
        radius: theme.radius_m
    }

    /** A group whose buttons keep their own shape and their own gaps. */
    mod.widgets.ButtonGroupSpaced = mod.widgets.ButtonGroup{
        join: Spaced
        spacing: theme.space_1
    }

    mod.widgets.SegmentedControlBase = #(SegmentedControl::register_widget(vm))
    /** One question with a fixed set of answers, and a pill on the current one. */
    mod.widgets.SegmentedControlFlat = set_type_default() do mod.widgets.SegmentedControlBase{
        width: Fit
        height: Fit
        /** the answers, in order */
        options: []
        /** which answer is current, as an index */
        selected: 0
        /** how many answers may be current at once: Any Single Multi */
        selection: Single
        /** every segment as wide as the widest, so the row does not shuffle */
        equal_width: false
        /** the row runs down instead of across */
        vertical: false
        /** height of a segment 0..64 step 1 */
        segment_height: theme.size_control_m
        /** space either side of a segment's word 0..40 step 1 */
        segment_padding: theme.space_3
        /** how long the pill takes to glide 0..1 step 0.01 */
        glide_secs: theme.motion_short_4
        /** dimmed and inert */
        disabled: false
        /** ink and fill alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        /** the role the pill is drawn in */
        intent: Neutral
        /** the shared role palette */
        palette: mod.widgets.BadgePalette{}

        draw_bg +: {
            color: theme.color_surface_container
            border_color: theme.color_outline
            border_size: 1.0
            radius: 8.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius * 0.5)
                sdf.fill_keep(self.color)
                if self.border_size > 0.0 {
                    sdf.stroke_keep(self.border_color, self.border_size)
                }
                return sdf.result
            }
        }
        draw_pill +: {
            color: theme.color_surface_bright
            border_color: theme.color_outline_variant
            border_size: 0.0
            radius: 6.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius * 0.5)
                sdf.fill_keep(self.color)
                if self.border_size > 0.0 {
                    sdf.stroke_keep(self.border_color, self.border_size)
                }
                return sdf.result
            }
        }
        draw_text +: {
            text_style: theme.font_regular{}
            color: theme.color_text
        }
        draw_text_selected +: {
            text_style: theme.font_bold{}
            color: theme.color_text
        }
    }

    /** The segmented control with the outset bevel on its pill. */
    mod.widgets.SegmentedControl = mod.widgets.SegmentedControlFlat{
        draw_pill +: {
            border_size: 1.0
        }
    }

    /** The answers stacked instead of laid across. */
    mod.widgets.SegmentedControlVertical = mod.widgets.SegmentedControl{
        vertical: true
        equal_width: true
    }

    /** Answers that can all be on at once: a set of switches sharing a shape. */
    mod.widgets.ToggleGroup = mod.widgets.SegmentedControl{
        selection: Multi
    }

    mod.widgets.SplitButtonBase = #(SplitButton::register_widget(vm))
    /** One button that does the usual thing, beside a half that offers the
     * rest. */
    mod.widgets.SplitButton = set_type_default() do mod.widgets.SplitButtonBase{
        width: Fit
        height: Fit
        flow: Right
        /** where the menu hangs: Below BelowRight At */
        place: BelowRight
        action := Button{
            text: "Save"
            draw_bg +: {
                border_radius_tr: uniform(0.)
                border_radius_br: uniform(0.)
            }
        }
        more := ButtonIcon{
            /** the chevron is drawn over this half, not typed into it */
            text: ""
            width: 22.
            draw_bg +: {
                border_radius_tl: uniform(0.)
                border_radius_bl: uniform(0.)
            }
        }
        draw_chevron +: {
            color: theme.color_on_surface
        }
    }

    mod.widgets.MenuButtonBase = #(MenuButton::register_widget(vm))
    /** A button whose whole job is to offer a menu: the label never changes
     * to whatever was picked, because it is a verb, not a value. */
    mod.widgets.MenuButton = set_type_default() do mod.widgets.MenuButtonBase{
        width: Fit
        height: Fit
        /** where the menu hangs: Below BelowRight At */
        place: Below
        button := Button{text: "Actions"}
    }
}

/// Buttons that belong together.
///
/// The group lays its children out and shapes their corners; what a button
/// DOES stays entirely the button's business, so any button in the library
/// can sit in one, including a preset the group has never heard of.
#[derive(Script, ScriptHook, Widget)]
pub struct ButtonGroup {
    #[deref]
    view: View,
    /// Connected reads as one object, Spaced as several.
    #[live]
    pub join: GroupJoin,
    #[live]
    pub axis: GroupAxis,
    /// The corner the outside of a connected group keeps.
    #[live(6.0)]
    pub radius: f64,
    #[rust]
    shaped: bool,
}

impl ButtonGroup {
    /// Shape every child's corners for its place in the row: a connected
    /// group keeps the outer corners and squares the joints, so the row
    /// reads as one object rather than as buttons pushed together.
    fn shape_children(&mut self, cx: &mut Cx) {
        if self.join != GroupJoin::Connected {
            return;
        }
        let mut children = Vec::new();
        self.view.children(&mut |_id, child| children.push(child));
        let last = children.len().saturating_sub(1);
        let horizontal = self.axis == GroupAxis::Horizontal;
        for (i, child) in children.iter_mut().enumerate() {
            let (first_corner, last_corner) = (i == 0, i == last);
            let r = self.radius as f32;
            let (tl, tr, bl, br) = if horizontal {
                (
                    if first_corner { r } else { 0.0 },
                    if last_corner { r } else { 0.0 },
                    if first_corner { r } else { 0.0 },
                    if last_corner { r } else { 0.0 },
                )
            } else {
                (
                    if first_corner { r } else { 0.0 },
                    if first_corner { r } else { 0.0 },
                    if last_corner { r } else { 0.0 },
                    if last_corner { r } else { 0.0 },
                )
            };
            // A child that has no such shader simply ignores this.
            script_apply_eval!(cx, child, {
                draw_bg +: {
                    border_radius_tl: #(tl)
                    border_radius_tr: #(tr)
                    border_radius_bl: #(bl)
                    border_radius_br: #(br)
                }
            });
        }
    }
}

impl Widget for ButtonGroup {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Shaped on the way into the first draw, and again after a live
        // edit: the children only exist once the DSL has been applied, and
        // an edit may have replaced them.
        if !self.shaped {
            self.shaped = true;
            self.shape_children(cx.cx.cx);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            self.shaped = false;
        }
        self.view.handle_event(cx, event, scope);
    }
}

/// One question with a fixed set of answers.
///
/// The answers are DATA, not children: `options` is a list of words the
/// control draws itself. A segmented control built from child widgets makes
/// the caller keep the children and the selection in step by hand, and
/// every caller gets that slightly wrong; a control that owns its list
/// cannot disagree with itself.
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct SegmentedControl {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    pub draw_bg: DrawSegmentedBg,
    #[live]
    draw_pill: DrawSegmentedBg,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_selected: DrawText,
    #[apply_default]
    animator: Animator,

    /// The answers, in order.
    #[live]
    pub options: Vec<String>,
    /// Which answer is current, as an index into `options`.
    #[live]
    pub selected: usize,
    /// How many answers may be current at once.
    #[live]
    pub selection: ChipSelection,
    /// Every segment as wide as the widest, so the row does not shuffle
    /// when the words change.
    #[live]
    pub equal_width: bool,
    /// The row runs down instead of across.
    #[live]
    pub vertical: bool,
    #[live(30.0)]
    pub segment_height: f64,
    #[live(12.0)]
    pub segment_padding: f64,
    #[live(0.2)]
    pub glide_secs: f64,
    #[live]
    pub disabled: bool,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live]
    pub intent: BadgeIntent,
    #[live]
    pub palette: BadgePalette,
    #[live(true)]
    #[visible]
    visible: bool,

    /// Which answers are on, while `selection` is Multi.
    #[rust]
    multi: Vec<bool>,
    /// The pill's glide.
    #[rust]
    pill: SlidingIndicator,
    /// Where each segment was drawn, so a press can be placed and the pill
    /// can be aimed.
    #[rust]
    segments: Vec<Rect>,
    /// The row's corner when the segments were measured: they are relative
    /// to it, whatever the layout moved the row by afterwards.
    #[rust]
    drawn_at: DVec2,
    /// Which segment the keyboard is on.
    #[rust]
    focused: usize,
    #[rust]
    last_time: f64,
}

impl SegmentedControl {
    fn is_on(&self, index: usize) -> bool {
        match self.selection {
            ChipSelection::Multi => self.multi.get(index).copied().unwrap_or(false),
            _ => index == self.selected,
        }
    }

    fn set_on(&mut self, index: usize, on: bool) {
        if self.selection == ChipSelection::Multi {
            if self.multi.len() < self.options.len() {
                self.multi.resize(self.options.len(), false);
            }
            if let Some(slot) = self.multi.get_mut(index) {
                *slot = on;
            }
        } else if on {
            self.selected = index;
        }
    }

    /// The width every segment needs for its word.
    fn segment_widths(&mut self, cx: &mut Cx2d) -> Vec<f64> {
        let pad = self.segment_padding * 2.0;
        let mut widths: Vec<f64> = self
            .options
            .iter()
            .map(|option| {
                let text = option.clone();
                measure(&self.draw_text_selected, cx, &text) + pad
            })
            .collect();
        if self.equal_width {
            let widest = widths.iter().cloned().fold(0.0f64, f64::max);
            widths.iter_mut().for_each(|w| *w = widest);
        }
        widths
    }

    pub fn set_selected(&mut self, cx: &mut Cx, index: usize) {
        if index < self.options.len() && self.selected != index {
            self.selected = index;
            // The arrows resume from the answer, not from wherever the keys
            // were last standing: a host that sets the answer itself has
            // moved the row, and the keyboard's place moved with it.
            self.focused = index;
            self.draw_bg.redraw(cx);
        }
    }

    /// The word that is current, or the words that are, joined by commas.
    pub fn selected_text(&self) -> String {
        if self.selection == ChipSelection::Multi {
            return self
                .options
                .iter()
                .enumerate()
                .filter(|(i, _)| self.is_on(*i))
                .map(|(_, option)| option.clone())
                .collect::<Vec<_>>()
                .join(", ");
        }
        self.options.get(self.selected).cloned().unwrap_or_default()
    }

    fn press(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.options.len() {
            return;
        }
        self.focused = index;
        match self.selection {
            ChipSelection::Multi => {
                let on = !self.is_on(index);
                self.set_on(index, on);
            }
            _ => {
                if self.selected == index {
                    return;
                }
                self.selected = index;
            }
        }
        cx.widget_action(self.uid, SegmentedControlAction::Selected(index));
        self.draw_bg.redraw(cx);
    }
}

impl Widget for SegmentedControl {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let widths = self.segment_widths(cx);
        let gap = 2.0;
        let (w, h) = if self.vertical {
            let widest = widths.iter().cloned().fold(0.0f64, f64::max);
            (widest + gap * 2.0, self.segment_height * widths.len() as f64 + gap * 2.0)
        } else {
            (widths.iter().sum::<f64>() + gap * 2.0, self.segment_height + gap * 2.0)
        };
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(w),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(h),
                other => other,
            },
            ..walk
        };
        let rect = self.draw_bg.draw_walk(cx, walk);
        self.drawn_at = rect.pos;

        // Where each answer sits, and where the pill should be.
        self.segments.clear();
        let mut x = rect.pos.x + gap;
        let mut y = rect.pos.y + gap;
        for (i, width) in widths.iter().enumerate() {
            let seg = if self.vertical {
                Rect {
                    pos: dvec2(rect.pos.x + gap, y),
                    size: dvec2(rect.size.x - gap * 2.0, self.segment_height),
                }
            } else {
                Rect { pos: dvec2(x, rect.pos.y + gap), size: dvec2(*width, self.segment_height) }
            };
            self.segments.push(seg);
            let _ = i;
            x += width;
            y += self.segment_height;
        }

        let family = self.palette.family(self.intent);
        let alpha = if self.disabled { self.disabled_opacity } else { 1.0 };

        // The pill glides to the current answer; every other answer that is
        // on gets a still pill, since only one thing can be gliding.
        if self.selection == ChipSelection::Multi {
            for (i, seg) in self.segments.clone().iter().enumerate() {
                if self.is_on(i) {
                    self.draw_pill.draw_abs(cx, *seg);
                }
            }
        } else if let Some(target) = self.segments.get(self.selected).copied() {
            // The glide runs in the row's own frame: a new answer slides,
            // but the row moving (a parent re-laid out around it) moves the
            // pill with it at once instead of sending it after the row.
            self.pill.aim(Rect { pos: target.pos - rect.pos, size: target.size }, self.glide_secs);
            let now = cx.seconds_since_app_start();
            let dt = if self.last_time > 0.0 { (now - self.last_time).max(0.0) } else { 0.0 };
            self.last_time = now;
            if self.pill.step(dt) {
                cx.new_next_frame();
            }
            let pill = self.pill.current();
            self.draw_pill.draw_abs(cx, Rect { pos: pill.pos + rect.pos, size: pill.size });
        }

        for (i, seg) in self.segments.clone().iter().enumerate() {
            let Some(option) = self.options.get(i).cloned() else {
                continue;
            };
            let on = self.is_on(i);
            let draw = if on { &mut self.draw_text_selected } else { &mut self.draw_text };
            let rest = draw.color;
            draw.color = Vec4f {
                w: rest.w * alpha,
                ..if on { family.on_container } else { rest }
            };
            let text_w = measure(draw, cx, &option);
            // Centred by the laid-out line (ascender to descender), which is
            // what `draw_abs` places at `pos`: the point size is shorter
            // than the line, and centring by it set every label low.
            let text_h = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), &option).size_in_lpxs.height as f64;
            let pos = dvec2(
                seg.pos.x + (seg.size.x - text_w) * 0.5,
                seg.pos.y + (seg.size.y - text_h) * 0.5,
            );
            draw.draw_abs(cx, pos, &option);
            draw.color = rest;
        }
        // The comment below the key handler has said "the row is one tab
        // stop" since it was written, and nothing ever registered one: the
        // arrows worked, but only for someone who had already put the mouse
        // on the row. A control a person cannot reach by keyboard has no
        // keyboard.
        if !self.disabled {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        if self.disabled {
            return;
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerHoverOut(_) => cx.set_cursor(MouseCursor::Default),
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.draw_bg.area());
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && fe.is_over && fe.was_tap() => {
                // The segments were placed where the row was first laid
                // out; a parent that aligns its children (centred, or after
                // a Fill sibling) moves the row afterwards. Place the press
                // against the row as drawn (`fe.rect`), then in the frame
                // the segments were measured in.
                let at = fe.abs - fe.rect.pos + self.drawn_at;
                if let Some(index) = self.segments.iter().position(|seg| seg.contains(at)) {
                    self.press(cx, index);
                }
            }
            // The row is one tab stop and the arrows walk it, the way a set
            // of answers should behave for someone working by keyboard.
            Hit::KeyDown(ke) => {
                let count = self.options.len();
                if count == 0 {
                    return;
                }
                let step = |i: usize, d: isize| -> usize {
                    let len = count as isize;
                    (((i as isize + d) % len + len) % len) as usize
                };
                match ke.key_code {
                    KeyCode::ArrowRight | KeyCode::ArrowDown => {
                        let next = step(self.focused, 1);
                        // Single selection moves the answer with the focus:
                        // that is what makes an arrow key useful here.
                        if self.selection == ChipSelection::Multi {
                            self.focused = next;
                            self.draw_bg.redraw(cx);
                        } else {
                            self.press(cx, next);
                        }
                    }
                    KeyCode::ArrowLeft | KeyCode::ArrowUp => {
                        let next = step(self.focused, -1);
                        if self.selection == ChipSelection::Multi {
                            self.focused = next;
                            self.draw_bg.redraw(cx);
                        } else {
                            self.press(cx, next);
                        }
                    }
                    KeyCode::Home => self.press(cx, 0),
                    KeyCode::End => self.press(cx, count - 1),
                    KeyCode::ReturnKey | KeyCode::Space => {
                        let focused = self.focused;
                        self.press(cx, focused);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The current answer, or the current answers joined by commas.
    fn text(&self) -> String {
        self.selected_text()
    }

    /// A word selects the answer with that name; a number selects by index.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if let Some(index) = self.options.iter().position(|option| option == v) {
            self.set_selected(cx, index);
        } else if let Ok(index) = v.trim().parse::<usize>() {
            self.set_selected(cx, index);
        }
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    /// The index of the current answer, so a test can wait on the number
    /// rather than on the word.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.selected.to_string())
    }

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        Some(self.selected_text())
    }
}

impl SegmentedControlRef {
    /// The index chosen this round, if the answer changed.
    pub fn selected(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<SegmentedControlAction>() {
            SegmentedControlAction::Selected(index) => Some(index),
            _ => None,
        }
    }

    pub fn set_selected(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, index);
        }
    }

    /// The current answer as a word.
    pub fn selected_text(&self) -> String {
        self.borrow().map(|inner| inner.selected_text()).unwrap_or_default()
    }
}

/// One button that does the usual thing, beside a half that offers the rest.
///
/// The two halves are separate targets on purpose. Pressing the wide half
/// does the thing it names, with no menu in the way; pressing the chevron
/// opens the menu and does nothing else. A control where the main action is
/// only reachable through a menu is a menu button, which is the widget
/// below, and the two should never be confused: the split button promises
/// that its label is one press away.
#[derive(Script, ScriptHook, Widget)]
pub struct SplitButton {
    #[deref]
    view: View,
    #[live]
    draw_chevron: DrawChevron,
    /// Where the menu hangs off the button.
    #[live]
    pub place: MenuPlace,
    /// The rows the chevron offers. A host sets these before the press, or
    /// answers [`SplitButtonAction::MenuRequested`] and opens its own.
    #[rust]
    rows: Vec<MenuRow>,
}

/// What a split button reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum SplitButtonAction {
    /// The wide half was pressed: do the thing the label names.
    Clicked,
    /// The chevron was pressed and this button has no rows of its own, so
    /// the host should raise the menu it wants against `anchor`.
    MenuRequested,
    #[default]
    None,
}

impl SplitButton {
    /// The rows the chevron offers.
    pub fn set_rows(&mut self, rows: Vec<MenuRow>) {
        self.rows = rows;
    }

    /// The rect a menu should hang off: the whole control, so the menu
    /// lines up with the button rather than with the chevron alone.
    pub fn anchor(&self, cx: &Cx) -> Rect {
        self.view.area().rect(cx)
    }
}

impl Widget for SplitButton {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if !step.is_done() {
            return step;
        }
        // Over the trailing half, after it: the chevron belongs to the
        // button's face, and the face has only just been drawn.
        let more = self.view.widget(cx, ids!(more)).area().rect(cx);
        if more.size.x > 0.0 {
            self.draw_chevron.draw_abs(cx, more);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let uid = self.widget_uid();
        if self.view.widget(cx, ids!(action)).as_button().clicked(actions) {
            cx.widget_action(uid, SplitButtonAction::Clicked);
        }
        if self.view.widget(cx, ids!(more)).as_button().clicked(actions) {
            let anchor = self.anchor(cx);
            if self.rows.is_empty() {
                cx.widget_action(uid, SplitButtonAction::MenuRequested);
            } else {
                cx.action(MenuAction::Open {
                    owner: LiveId(uid.0),
                    rows: self.rows.clone(),
                    anchor,
                    place: self.place,
                });
            }
        }
    }

    /// The label of the half that acts.
    fn text(&self) -> String {
        String::new()
    }
}

impl SplitButtonRef {
    /// The rows the chevron offers.
    pub fn set_rows(&self, rows: Vec<MenuRow>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rows(rows);
        }
    }

    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<SplitButtonAction>(), SplitButtonAction::Clicked);
        }
        false
    }

    /// The anchor to open a menu against, when the host builds its own.
    pub fn menu_requested(&self, cx: &Cx, actions: &Actions) -> Option<Rect> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<SplitButtonAction>() {
            SplitButtonAction::MenuRequested => {
                Some(self.borrow().map(|inner| inner.anchor(cx)).unwrap_or_default())
            }
            _ => None,
        }
    }

    /// The id the menu raised by this button carries, for reading its pick.
    pub fn menu_owner(&self) -> LiveId {
        LiveId(self.widget_uid().0)
    }
}

/// A button whose whole job is to offer a menu.
///
/// Its label is a verb and stays put: a control that renames itself to
/// whatever was last picked is a select, and a select carries a value while
/// this carries a set of commands.
#[derive(Script, ScriptHook, Widget)]
pub struct MenuButton {
    #[deref]
    view: View,
    #[live]
    pub place: MenuPlace,
    #[rust]
    rows: Vec<MenuRow>,
}

impl MenuButton {
    pub fn set_rows(&mut self, rows: Vec<MenuRow>) {
        self.rows = rows;
    }

    pub fn anchor(&self, cx: &Cx) -> Rect {
        self.view.area().rect(cx)
    }
}

impl Widget for MenuButton {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        if self.view.widget(cx, ids!(button)).as_button().clicked(actions) {
            let uid = self.widget_uid();
            let anchor = self.anchor(cx);
            if self.rows.is_empty() {
                cx.widget_action(uid, SplitButtonAction::MenuRequested);
            } else {
                cx.action(MenuAction::Open {
                    owner: LiveId(uid.0),
                    rows: self.rows.clone(),
                    anchor,
                    place: self.place,
                });
            }
        }
    }
}

impl MenuButtonRef {
    pub fn set_rows(&self, rows: Vec<MenuRow>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rows(rows);
        }
    }

    /// The anchor to open a menu against, when the host builds its own.
    pub fn menu_requested(&self, cx: &Cx, actions: &Actions) -> Option<Rect> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<SplitButtonAction>() {
            SplitButtonAction::MenuRequested => {
                Some(self.borrow().map(|inner| inner.anchor(cx)).unwrap_or_default())
            }
            _ => None,
        }
    }

    /// The id the menu raised by this button carries.
    pub fn menu_owner(&self) -> LiveId {
        LiveId(self.widget_uid().0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The indicator takes its first target instantly — there is nowhere to
    /// slide from — and glides to every later one.
    #[test]
    fn the_indicator_takes_the_first_target_and_glides_to_the_rest() {
        let a = Rect { pos: dvec2(0.0, 0.0), size: dvec2(10.0, 4.0) };
        let b = Rect { pos: dvec2(20.0, 0.0), size: dvec2(10.0, 4.0) };
        let mut pill = SlidingIndicator::default();
        assert!(pill.is_empty());
        pill.aim(a, 0.2);
        assert_eq!(pill.current(), a, "the first target is taken at once");
        assert!(!pill.step(0.016), "and needs no further frames");
        pill.aim(b, 0.2);
        assert_eq!(pill.current(), a, "the glide starts where it was");
        assert!(pill.step(0.1), "still moving half way through");
        let half = pill.current();
        assert!(half.pos.x > a.pos.x && half.pos.x < b.pos.x, "between the two");
        assert!(!pill.step(0.2), "arrived");
        assert_eq!(pill.current(), b);
    }

    /// A target set mid-glide bends the path from where the pill is, rather
    /// than snapping it back to where the last glide started.
    #[test]
    fn a_new_target_mid_glide_starts_from_where_the_pill_is() {
        let a = Rect { pos: dvec2(0.0, 0.0), size: dvec2(10.0, 4.0) };
        let b = Rect { pos: dvec2(100.0, 0.0), size: dvec2(10.0, 4.0) };
        let c = Rect { pos: dvec2(50.0, 0.0), size: dvec2(10.0, 4.0) };
        let mut pill = SlidingIndicator::default();
        pill.aim(a, 0.2);
        pill.aim(b, 0.2);
        pill.step(0.1);
        let mid = pill.current().pos.x;
        pill.aim(c, 0.2);
        assert_eq!(pill.current().pos.x, mid, "the new glide starts where the old one had got to");
    }

    /// Progress runs from 0 to 1 over the glide and settling lands at once,
    /// so a fade tied to the glide ends exactly where the pill does.
    #[test]
    fn sliding_indicator_progress_reports_the_glide() {
        let a = Rect { pos: dvec2(0.0, 0.0), size: dvec2(10.0, 4.0) };
        let b = Rect { pos: dvec2(40.0, 0.0), size: dvec2(10.0, 4.0) };
        let mut pill = SlidingIndicator::default();
        pill.aim(a, 0.2);
        assert_eq!(pill.progress(), 1.0, "a first target is already reached");
        pill.aim(b, 0.2);
        assert_eq!(pill.progress(), 0.0, "a new glide starts at nothing");
        pill.step(0.05);
        assert!((pill.progress() - 0.25).abs() < 1e-9, "a quarter of the time is a quarter of the way");
        pill.settle();
        assert_eq!(pill.progress(), 1.0);
        assert_eq!(pill.current(), b, "settling lands on the target");
        assert!(!pill.step(0.016), "and asks for no more frames");
    }

    /// Room for the control that shows the rest comes out of the same
    /// width, which can push one more item out of the row.
    #[test]
    fn the_more_control_takes_its_room_from_the_row() {
        let widths = [40.0, 40.0, 40.0];
        assert_eq!(overflow_split(&widths, 4.0, 500.0, 30.0), (3, 0), "everything fits");
        // 128 needed, 120 given. Keeping 30 for the "more" control leaves
        // 90, which takes two items and their gap (84) but not a third.
        assert_eq!(overflow_split(&widths, 4.0, 120.0, 30.0), (2, 1));
        // Widen the "more" control and the second item goes too.
        assert_eq!(overflow_split(&widths, 4.0, 120.0, 60.0), (1, 2));
        assert_eq!(overflow_split(&widths, 4.0, 0.0, 30.0), (0, 3), "nothing fits");
    }

    /// A row drawn at a given level: what the filler is left with once the
    /// concessions in force have come off the content. A filler is never
    /// narrower than nothing, which is the whole difficulty — an overfull
    /// row and an exactly full one measure the same.
    fn filler_width(content: f64, costs: &[f64], level: usize, room: f64) -> f64 {
        let saved: f64 = costs.iter().take(level).sum();
        (room - (content - saved)).max(0.0)
    }

    /// Draw, measure, decide, draw again, until the level stops moving.
    /// Answers where it settled and how many draws that took. A rule that
    /// flaps never settles, and this says so rather than passing quietly.
    fn settle(costs: &[f64], margin: f64, content: f64, room: f64, start: usize) -> (usize, usize) {
        let mut level = start;
        for draw in 1..=64 {
            let slack = filler_width(content, costs, level, room);
            let next = concession_level(costs, margin, level, slack);
            if next == level {
                return (level, draw);
            }
            level = next;
        }
        panic!("the ladder never settled: it is flapping between levels");
    }

    /// A row with room to spare gives nothing up, and knows it on the first
    /// draw rather than trying a step and taking it back.
    #[test]
    fn a_row_with_room_to_spare_concedes_nothing() {
        let costs = [36.0, 120.0, 90.0, 20.0];
        assert_eq!(concession_level(&costs, 8.0, 0, 240.0), 0);
        assert_eq!(settle(&costs, 8.0, 700.0, 940.0, 0), (0, 1), "settled without a second draw");
    }

    /// The ladder is walked in the order it was written and stops at the
    /// first level that fits, rather than jumping to the step that saves
    /// most or conceding the lot.
    #[test]
    fn the_ladder_is_taken_in_order_and_stops_as_soon_as_the_row_fits() {
        let costs = [36.0, 120.0, 90.0, 20.0];
        // Sixty short: the first concession does not cover it, the second
        // does, and the two after it are never reached.
        assert_eq!(concession_level(&costs, 8.0, 0, -60.0), 2);
        // The same through a real draw loop, where the row can only report a
        // filler of nothing however far over it is, so the walk takes a
        // draw per step and still stops at the same level.
        assert_eq!(settle(&costs, 8.0, 700.0, 640.0, 0), (2, 3));
    }

    /// When the whole ladder is not enough, everything goes and nothing
    /// asks for a step that is not there.
    #[test]
    fn everything_is_conceded_when_nothing_helps_enough() {
        let costs = [4.0, 4.0, 4.0, 4.0];
        assert_eq!(concession_level(&costs, 8.0, 0, -100.0), 4, "the whole ladder, and no more");
        assert_eq!(settle(&costs, 8.0, 800.0, 600.0, 0), (4, 3), "and it stops asking for a fifth");
    }

    /// The one that matters: a slack sitting exactly on a boundary has one
    /// answer, and every width the window can be dragged through settles to
    /// the same level whether it is approached from a narrow row or a wide
    /// one.
    #[test]
    fn a_slack_on_the_boundary_does_not_flap_between_two_levels() {
        let costs = [36.0, 120.0, 90.0, 20.0];
        let margin = 8.0;
        // Exactly enough to give the first concession back: the row lands on
        // the margin, not below it, so it does not concede straight again.
        assert_eq!(concession_level(&costs, margin, 1, 44.0), 0);
        assert_eq!(concession_level(&costs, margin, 0, 8.0), 0, "and stays there");
        // A hair less and the concession is kept, rather than taken back and
        // lost again on the next draw.
        assert_eq!(concession_level(&costs, margin, 1, 43.9), 1);
        assert_eq!(concession_level(&costs, margin, 0, 7.9), 1, "which is the same boundary");
        // A margin of nothing is the sharpest case and still has one answer.
        assert_eq!(concession_level(&costs, 0.0, 1, 36.0), 0);
        assert_eq!(concession_level(&costs, 0.0, 0, 0.0), 0);
        for step in 0..1200 {
            let room = 400.0 + step as f64 * 0.5;
            let (narrowing, _) = settle(&costs, margin, 700.0, room, 0);
            let (widening, _) = settle(&costs, margin, 700.0, room, costs.len());
            assert_eq!(narrowing, widening, "room {room} settles to one level, not two");
        }
    }

    /// Walked blind, the ladder still stops as soon as the row fits; it just
    /// spends a draw on each step, and it does not hand back what it cannot
    /// price.
    #[test]
    fn an_unmeasured_step_is_taken_alone_and_never_given_back() {
        let unknown = [0.0, 0.0, 0.0, 0.0];
        assert_eq!(concession_level(&unknown, 8.0, 0, -100.0), 1, "one step, then measure again");
        assert_eq!(concession_level(&unknown, 8.0, 1, -100.0), 2);
        // The costs the row really has are not the ones it has been told.
        let real = [36.0, 120.0, 90.0, 20.0];
        let mut level = 0;
        let mut draws = 0;
        loop {
            draws += 1;
            assert!(draws < 16, "blind, but not endless");
            let slack = filler_width(700.0, &real, level, 640.0);
            let next = concession_level(&unknown, 8.0, level, slack);
            if next == level {
                break;
            }
            level = next;
        }
        assert_eq!((level, draws), (2, 3), "the same level, a draw per step");
        // Given room again, a row that still cannot price its concessions
        // keeps them.
        assert_eq!(concession_level(&unknown, 8.0, 2, 400.0), 2);
        // Told what they save, it gives them back.
        assert_eq!(concession_level(&real, 8.0, 2, 400.0), 0);
    }

    /// The ladder reports movement, not measurement, so a settled row asks
    /// for no redraw however many times it is measured.
    #[test]
    fn the_ladder_reports_only_a_level_that_actually_moved() {
        let mut ladder = ConcessionLadder::new(4, 8.0);
        ladder.set_cost(0, 36.0);
        assert!(ladder.measured(0.0), "a full row gives the first thing up");
        assert_eq!(ladder.level(), 1);
        assert!(ladder.is_conceded(0));
        assert!(!ladder.is_conceded(1), "and nothing further");
        assert!(!ladder.measured(40.0), "settled: nothing moved, so nothing redraws");
        assert!(!ladder.measured(40.0));
        assert!(ladder.measured(200.0), "room enough to take the step back");
        assert_eq!(ladder.level(), 0);
        assert!(!ladder.measured(200.0), "and settled again");
    }

    /// Multi keeps its own answers; single keeps one.
    #[test]
    fn multi_and_single_keep_different_answers() {
        assert_ne!(ChipSelection::Multi, ChipSelection::Single);
    }
}
