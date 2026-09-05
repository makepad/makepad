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
}

fn ease_out_cubic(t: f64) -> f64 {
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
            self.pill.aim(target, self.glide_secs);
            let now = cx.seconds_since_app_start();
            let dt = if self.last_time > 0.0 { (now - self.last_time).max(0.0) } else { 0.0 };
            self.last_time = now;
            if self.pill.step(dt) {
                cx.new_next_frame();
            }
            self.draw_pill.draw_abs(cx, self.pill.current());
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
            let size = draw.text_style.font_size as f64;
            let text_w = measure(draw, cx, &option);
            let pos = dvec2(
                seg.pos.x + (seg.size.x - text_w) * 0.5,
                seg.pos.y + (seg.size.y - size) * 0.5,
            );
            draw.draw_abs(cx, pos, &option);
            draw.color = rest;
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
                if let Some(index) = self.segments.iter().position(|seg| seg.contains(fe.abs)) {
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

    /// Multi keeps its own answers; single keeps one.
    #[test]
    fn multi_and_single_keep_different_answers() {
        assert_ne!(ChipSelection::Multi, ChipSelection::Single);
    }
}
