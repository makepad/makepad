//! LineMenu — a table of contents that takes almost no room.
//!
//! A vertical stack of short horizontal lines, one per section of a
//! document, whose lengths hint at the heading level. Resting the pointer on
//! the stack, or focusing it, lengthens the lines and shows each section's
//! name beside them on a small card. The line of the section being read is
//! lit and follows the linked scroll view as it scrolls, and pressing a row
//! scrolls that view so the section's heading sits near the top.
//!
//! It answers "where am I in this long page, and how do I get to the part I
//! want" without giving up a column to a list of headings.
//!
//! Sections are data naming widgets by id, not headings found by walking the
//! page: every heading preset is the same `Label` underneath, so a walk
//! cannot tell one level from another, the words in a contents list are often
//! shorter than the heading, and a section can start at a picture or a code
//! block as well as at a heading.
//!
//! The view it follows raises nothing the menu can hear when it scrolls, and
//! a scroll redraws only the view. So the menu looks: any event arms a watch
//! that reads the view's offset and each heading's place once a frame, and
//! stops three quiet frames after the last change. An idle page costs nothing.
//!
//! A menu inside the view it follows is not supported: that view is busy
//! dispatching to its children whenever the menu runs, so it can never be
//! asked where it is or told to move. Put the menu beside the view.

use crate::{
    animator::Ease,
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::event::TouchState,
    menu_bar::{for_each_element, obj_field, obj_string},
    nav_list::DrawNavGround,
    overlay_place::{claim_escape, orphan_sweep_locks, release_orphaned_sweep_locks, span_inboard},
    pill_nav::{smootherstep, DrawMorphSurface, UnitTween},
    scroll_bars::ScrollExtent,
    scroll_fade::ScrollShadowView,
    tour::{askable_area, id_path},
    view::View,
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    /** One section's line: `current` lights it, `ancestor` tints the lit
     * section's parents, `hot` lifts the one under the pointer or keys. */
    set_type_default() do #(DrawLineMenuLine::script_shader(vm)){
        ..mod.draw.DrawQuad
        hot: 0.0
        current: 0.0
        ancestor: 0.0
        opacity: 1.0
        /** a line at rest */
        color: uniform(theme.color_outline)
        /** the lit section's parents */
        color_ancestor: uniform(theme.color_on_surface_variant)
        /** the line under the pointer or the keys */
        color_hot: uniform(theme.color_on_surface)
        /** the section being read */
        color_current: uniform(theme.color_primary)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let r = min(self.rect_size.x, self.rect_size.y) * 0.5
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, r * 0.5)
            let ink = self.color
                .mix(self.color_ancestor, self.ancestor)
                .mix(self.color_hot, self.hot)
                .mix(self.color_current, self.current)
            sdf.fill(ink)
            return sdf.result * self.opacity
        }
    }

    /** A section's name, faded with the card. */
    set_type_default() do #(DrawLineMenuText::script_shader(vm)){
        ..mod.draw.DrawText
        opacity: 1.0
        get_color: fn() {
            return vec4(self.color.rgb, self.color.a * self.opacity)
        }
    }

    mod.widgets.LineMenuBase = #(LineMenu::register_widget(vm))

    /** A table of contents as a stack of short lines; resting on it shows
     * the names, the section being read is lit, and a press scrolls there. */
    mod.widgets.LineMenu = set_type_default() do mod.widgets.LineMenuBase{
        width: Fit
        height: Fit
        padding: Inset{top: 4. right: 4. bottom: 4. left: 4.}

        /** dotted id path of the view to follow, searched outward; empty is unlinked */
        scroll_view: ""
        /** the sections: `{target: "intro" label: "Introduction" level: 1}` */
        sections: []
        /** deeper sections get no line but still count 1..6 step 1 */
        max_level: 3.0
        /** track the linked view; off, the host lights sections itself */
        follow_scroll: true
        /** the names always drawn inline, with no card */
        always_show_labels: false
        /** lines hug the right edge and the names open to their left */
        mirror: false
        /** one section's row 10..40 step 1 */
        row_height: 18.0
        /** a line's thickness 1..6 step 0.5 */
        line_thickness: 2.0
        /** a level-1 line at rest 4..80 step 1 */
        line_length: 20.0
        /** how much shorter each deeper level is 0..20 step 1 */
        line_step: 5.0
        /** no line is shorter than this 2..40 step 1 */
        line_min: 6.0
        /** added to every line while the names show 0..40 step 1 */
        hover_extra: 10.0
        /** added to the lit line at all times 0..20 step 1 */
        current_extra: 4.0
        /** from the longest line to the names 0..32 step 1 */
        label_gap: 10.0
        /** longer names end in an ellipsis 60..480 step 4 */
        label_max_width: 220.0
        /** room between the card's edge and the rows 0..24 step 1 */
        card_pad: 8.0
        /** the card's corner rounding 0..24 step 1 */
        card_radius: theme.radius_l
        /** how far down the view a heading counts as reached 0..1 step 0.05 */
        spy_line: 0.25
        /** room left above a heading after a jump 0..96 step 1 */
        scroll_margin: 12.0
        /** rest before the names appear 0..1 step 0.01 */
        reveal_delay: 0.06
        /** grace after the pointer leaves the stack and the card 0..1 step 0.01 */
        conceal_delay: 0.2
        // The names arrive fast and settle, and go slowly then quickly: the
        // theme's enter and exit curves.
        /** the names appearing 0..1 step 0.01 */
        reveal_secs: theme.motion_short_4
        /** the curve the lines lengthen and the names come in along; a theme easing */
        reveal_ease: theme.motion_ease_emphasized_decelerate
        /** the names going 0..1 step 0.01 */
        conceal_secs: theme.motion_short_3
        /** the curve the lines shorten and the names go along; a theme easing */
        conceal_ease: theme.motion_ease_standard_accelerate
        // The page starts and stops on screen, which is what the standard
        // curve is for.
        /** the scroll to a section 0..1.5 step 0.05 */
        jump_secs: theme.motion_medium_4
        /** the curve the page scrolls along; a theme easing */
        jump_ease: theme.motion_ease_standard
        /** the names and the jump land at once; delays still apply */
        reduced_motion: false
        /** alpha of a row whose target is missing 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity

        /** the lines */
        draw_line +: {
            color: theme.color_outline
            color_ancestor: theme.color_on_surface_variant
            color_hot: theme.color_on_surface
            color_current: theme.color_primary
        }
        /** the card the names show on */
        draw_card +: {
            color: theme.color_surface_container
            color_from: theme.color_surface_container
            border_color: theme.color_outline_variant
            border_size: 1.0
            shadow_color: theme.color_elevation_2
            shadow_radius: theme.elevation_2_radius
            shadow_offset: vec2(0.0, theme.elevation_2_offset_y)
        }
        /** the tint behind a hot or keyed row */
        draw_row +: {
            color: theme.color_surface_container_high
            color_from: theme.color_surface_container_high
            border_size: 0.0
            shadow_radius: 0.0
            shadow_color: vec4(0.0, 0.0, 0.0, 0.0)
        }
        /** the ring around the keyboard's row */
        draw_focus +: {
            color: vec4(0.0, 0.0, 0.0, 0.0)
            color_from: vec4(0.0, 0.0, 0.0, 0.0)
            border_color: theme.color_primary
            border_size: theme.size_focus_ring
            shadow_radius: 0.0
            shadow_color: vec4(0.0, 0.0, 0.0, 0.0)
        }
        /** a section's name */
        draw_label +: {
            text_style: theme.font_regular{font_size: theme.font_size_p}
            color: theme.color_on_surface_variant
        }
        /** the lit section's name, in the face every name is measured in */
        draw_label_current +: {
            text_style: theme.font_bold{font_size: theme.font_size_p}
            color: theme.color_on_surface
        }
    }

    /** The stack with its names always showing, as a plain table of contents. */
    mod.widgets.LineMenuLabeled = mod.widgets.LineMenu{
        always_show_labels: true
    }

    /** The stack for the right-hand side of a page: lines hug the right edge
     * and the names open to their left. */
    mod.widgets.LineMenuMirrored = mod.widgets.LineMenu{
        mirror: true
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLineMenuLine {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub hot: f32,
    #[live]
    pub current: f32,
    #[live]
    pub ancestor: f32,
    #[live(1.0)]
    pub opacity: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLineMenuText {
    #[deref]
    draw_super: DrawText,
    #[live(1.0)]
    pub opacity: f32,
}

// ---------------------------------------------------------------------------
// data
// ---------------------------------------------------------------------------

/// One section of the document the menu follows.
#[derive(Clone, Debug, PartialEq)]
pub struct LineSection {
    pub id: LiveId,
    pub label: String,
    /// 1 to 6; a section's parent is the nearest earlier one with a smaller
    /// level.
    pub level: u8,
    /// Dotted id path of the widget the section starts at, inside the view.
    pub target: String,
}

impl LineSection {
    /// A section whose id is the last segment of `target`.
    pub fn new(target: &str, label: &str, level: u8) -> Self {
        Self { id: section_id(target, None), label: label.to_string(), level: level.clamp(1, 6), target: target.to_string() }
    }

    pub fn with_id(mut self, id: LiveId) -> Self {
        self.id = id;
        self
    }
}

/// What the menu reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum LineMenuAction {
    /// The lit section changed: by scrolling, by a jump, or by `set_current`.
    Changed(LiveId),
    /// A row was activated by pointer or key. Raised even when the menu
    /// scrolls the view itself, and even when it is unlinked, so a host can
    /// scroll something of its own.
    Jumped(LiveId),
    #[default]
    None,
}

fn parse_sections(vm: &mut ScriptVm, value: ScriptValue) -> Vec<LineSection> {
    let mut sections = Vec::new();
    for_each_element(vm, value, &mut |vm, entry| {
        let Some(obj) = entry.as_object() else {
            return;
        };
        // A section with nothing to say has no row to say it in.
        let Some(label) = obj_string(vm, obj, id!(label)) else {
            return;
        };
        let target = obj_string(vm, obj, id!(target)).unwrap_or_default();
        let explicit = obj_field(vm, obj, id!(id)).as_id();
        let level = obj_field(vm, obj, id!(level)).as_number().map_or(1.0, |n| n.round()).clamp(1.0, 6.0) as u8;
        sections.push(LineSection { id: section_id(&target, explicit), label, level, target });
    });
    sections
}

// ---------------------------------------------------------------------------
// pure helpers
// ---------------------------------------------------------------------------

/// Inset kept between the card and the window's edges, as the popover keeps.
const EDGE: f64 = 6.0;
/// How far a focus ring stands off what it rings.
const RING_OUT: f64 = 2.0;
/// The narrowest the names shrink to before the card stops giving ground.
const MIN_LABEL: f64 = 40.0;
/// Frames the watch keeps looking after the last change it saw.
const QUIET_FRAMES: u8 = 3;

/// Which section is being read. `tops` is each section's heading top
/// relative to the viewport top, in document order; `None` for one not
/// drawn. `line` is how far down the viewport a heading must come to count
/// as reached.
pub fn current_section(tops: &[Option<f64>], viewport_height: f64, line: f64, at_end: bool) -> Option<usize> {
    let mut drawn: Vec<(usize, f64)> = tops.iter().enumerate().filter_map(|(i, top)| top.map(|t| (i, t))).collect();
    if drawn.is_empty() {
        return None;
    }
    drawn.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    // A short final section can never climb to the line; at the very end the
    // last heading on screen is what the reader is looking at.
    if at_end {
        if let Some((i, _)) = drawn.iter().rev().find(|(_, top)| *top < viewport_height) {
            return Some(*i);
        }
    }
    if let Some((i, _)) = drawn.iter().rev().find(|(_, top)| *top <= line + 0.5) {
        return Some(*i);
    }
    // Nothing has reached the line yet: the first section, because a menu
    // with nothing lit does not say where you are.
    Some(drawn[0].0)
}

/// The sections a section sits under, nearest first.
pub fn ancestors(levels: &[u8], index: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(mut level) = levels.get(index).copied() else {
        return out;
    };
    for i in (0..index).rev() {
        if levels[i] < level {
            out.push(i);
            level = levels[i];
            if level <= 1 {
                break;
            }
        }
    }
    out
}

/// The section whose row stands for `index`: itself when its level has a
/// row, else its nearest ancestor that does.
pub fn shown_row(levels: &[u8], max_level: u8, index: usize) -> Option<usize> {
    let level = *levels.get(index)?;
    if level <= max_level {
        return Some(index);
    }
    ancestors(levels, index).into_iter().find(|a| levels[*a] <= max_level)
}

/// Where to scroll so a heading `top_in_view` below the viewport's top sits
/// `margin` below it, kept inside what the view can scroll.
pub fn jump_target(scroll: f64, top_in_view: f64, margin: f64, total: f64, visible: f64) -> f64 {
    (scroll + top_in_view - margin).clamp(0.0, (total - visible).max(0.0))
}

/// A jump's light is held until the reader moves the page by more than two
/// points: a view that settles a hair off its target has not been scrolled.
pub fn pin_released(landed: f64, now: f64) -> bool {
    (now - landed).abs() > 2.0
}

/// Where a jump from `from` to `to` has the view `t` of the way through its
/// time, along `ease`. Exactly `to` once the time is up, because a curve read
/// at its last sample can land a hair off; and never outside `0..=furthest`,
/// so a curve that overshoots swings past the heading but never past either
/// end of the page.
pub fn jump_offset(from: f64, to: f64, t: f64, ease: Ease, furthest: f64) -> f64 {
    if t >= 1.0 {
        return to;
    }
    (from + (to - from) * ease.map(t.max(0.0))).clamp(0.0, furthest.max(0.0))
}

/// The line lengths, as `line_length` reads them.
#[derive(Clone, Copy, Debug)]
pub struct LineMetrics {
    pub line_length: f64,
    pub line_step: f64,
    pub line_min: f64,
    pub hover_extra: f64,
    pub current_extra: f64,
}

/// How long a section's line is: shorter by level down to a floor, longer
/// while the names show, and a little longer again when it is lit. `reveal`
/// is already eased, so a spring that swings it past 1 lengthens the line
/// past its revealed length and back.
pub fn line_length(level: u8, m: &LineMetrics, reveal: f64, lit: bool) -> f64 {
    let rest = (m.line_length - level.saturating_sub(1) as f64 * m.line_step).max(m.line_min);
    rest + m.hover_extra * reveal + if lit { m.current_extra } else { 0.0 }
}

/// The lowest and the highest a run along `ease` goes: 0 and 1 for a curve
/// that stays between its ends. Sampled, because an easing is a curve the
/// theme names, not a formula this file knows.
pub fn ease_bounds(ease: Ease) -> (f64, f64) {
    const SAMPLES: usize = 240;
    (0..=SAMPLES)
        .map(|i| ease.map(i as f64 / SAMPLES as f64))
        .fold((0.0, 1.0), |(low, high), v| (low.min(v), high.max(v)))
}

/// The furthest the reveal can carry a line, in shares of `hover_extra`:
/// 1 unless a curve overshoots. A reveal from nothing reaches the top of its
/// curve; a conceal from fully shown reaches 1 less the bottom of its own.
pub fn reveal_reach(reveal_ease: Ease, conceal_ease: Ease) -> f64 {
    ease_bounds(reveal_ease).1.max(1.0 - ease_bounds(conceal_ease).0)
}

/// A section's id: the one written, else the last segment of its target.
pub fn section_id(target: &str, explicit: Option<LiveId>) -> LiveId {
    explicit.unwrap_or_else(|| LiveId::from_str(target.rsplit('.').next().unwrap_or(target)))
}

/// The card's spacing, as `card_layout` reads it.
#[derive(Clone, Copy, Debug)]
pub struct CardMetrics {
    pub label_gap: f64,
    pub card_pad: f64,
    pub label_max_width: f64,
    pub min_label: f64,
}

/// Where the card goes and where its names start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardLayout {
    pub card: Rect,
    pub label_x: f64,
    pub label_w: f64,
    /// The names are left of the lines.
    pub labels_before: bool,
}

/// Place the card beside a stack. The names open after the lines, or before
/// them when mirrored; a side that overruns `bounds` gives way to the other
/// when that one fits, and with room on neither the names shrink to the
/// roomier side. The card never moves vertically: the rows are where the
/// stack is. A `bounds` with no width is unbounded.
pub fn card_layout(
    stack: Rect,
    lines_start: f64,
    lines_end: f64,
    widest_label: f64,
    m: &CardMetrics,
    bounds: Rect,
    mirror: bool,
) -> CardLayout {
    let label_w = widest_label.min(m.label_max_width).max(0.0);
    let unbounded = bounds.size.x <= 0.0;
    let lo = bounds.pos.x;
    let hi = bounds.pos.x + bounds.size.x;
    // (label_x, card left, card right) for a side and a width.
    let span = |before: bool, w: f64| {
        if before {
            let label_x = lines_start - m.label_gap - w;
            (label_x, label_x - m.card_pad, stack.pos.x + stack.size.x)
        } else {
            let label_x = lines_end + m.label_gap;
            (label_x, stack.pos.x, label_x + w + m.card_pad)
        }
    };
    let fits = |before: bool, w: f64| {
        let (_, left, right) = span(before, w);
        unbounded || (left >= lo && right <= hi)
    };
    let room = |before: bool| {
        if before {
            lines_start - m.label_gap - m.card_pad - lo
        } else {
            hi - (lines_end + m.label_gap + m.card_pad)
        }
    };
    let before = if fits(mirror, label_w) {
        mirror
    } else if fits(!mirror, label_w) {
        !mirror
    } else if room(!mirror) > room(mirror) {
        !mirror
    } else {
        mirror
    };
    let w = if fits(before, label_w) { label_w } else { room(before).min(label_w).max(m.min_label) };
    let (label_x, left, right) = span(before, w);
    let card = Rect {
        pos: dvec2(left, stack.pos.y - m.card_pad),
        size: dvec2(right - left, stack.size.y + m.card_pad * 2.0),
    };
    let shifted = span_inboard(card.pos.x, card.size.x, lo, if unbounded { 0.0 } else { bounds.size.x });
    let dx = shifted - card.pos.x;
    CardLayout {
        card: Rect { pos: dvec2(shifted, card.pos.y), size: card.size },
        label_x: label_x + dx,
        label_w: w,
        labels_before: before,
    }
}

/// Whether the names show. Pure: the widget feeds it the pointer, the focus
/// and the clock, and animates toward what it answers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reveal {
    pub shown: bool,
    pub pending: Option<(bool, f64)>,
}

impl Reveal {
    /// The pointer is over the stack or the card, or not, at `now`.
    pub fn pointer(&mut self, over: bool, now: f64, reveal_delay: f64, conceal_delay: f64) -> Option<bool> {
        if over == self.shown {
            // Already what the pointer asks for: a change the other way is
            // no longer wanted.
            self.pending = None;
            return None;
        }
        let delay = if over { reveal_delay } else { conceal_delay };
        if delay <= 0.0 {
            self.pending = None;
            self.shown = over;
            return Some(over);
        }
        if self.pending.map(|(want, _)| want) != Some(over) {
            self.pending = Some((over, now + delay));
        }
        None
    }

    /// Focus shows the names at once. Losing it hides them at once, unless
    /// the pointer is over them, which is a reason of its own to keep them.
    pub fn focus(&mut self, focused: bool, pointer_over: bool) -> Option<bool> {
        if focused {
            self.pending = None;
            if !self.shown {
                self.shown = true;
                return Some(true);
            }
            return None;
        }
        if pointer_over {
            return None;
        }
        self.pending = None;
        if self.shown {
            self.shown = false;
            return Some(false);
        }
        None
    }

    /// A tap has no hover to rest with, so it flips the names at once.
    pub fn touch_toggle(&mut self) -> Option<bool> {
        self.pending = None;
        self.shown = !self.shown;
        Some(self.shown)
    }

    pub fn tick(&mut self, now: f64) -> Option<bool> {
        match self.pending {
            Some((want, at)) if at <= now => {
                self.pending = None;
                if self.shown != want {
                    self.shown = want;
                    Some(want)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn deadline(&self) -> Option<f64> {
        self.pending.map(|(_, at)| at)
    }
}

/// One line of text's drawn size.
fn measure(dt: &DrawText, cx: &mut Cx, text: &str) -> DVec2 {
    if text.is_empty() {
        return dvec2(0.0, 0.0);
    }
    let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = dt.font_scale as f64;
    dvec2(laidout.size_in_lpxs.width as f64 * scale, laidout.size_in_lpxs.height as f64 * scale)
}

/// `text` cut to `max_w` with an ellipsis.
fn elide(dt: &DrawText, cx: &mut Cx, text: &str, max_w: f64) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let full = measure(dt, cx, text).x;
    if full <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = ((chars.len() as f64) * (max_w / full.max(1.0))).floor() as usize;
    keep = keep.min(chars.len());
    while keep > 0 {
        let mut cut: String = chars[..keep].iter().collect();
        cut.push('\u{2026}');
        if measure(dt, cx, &cut).x <= max_w {
            return cut;
        }
        keep -= 1;
    }
    String::new()
}

// ---------------------------------------------------------------------------
// the widget
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkedKind {
    View,
    Shadow,
}

/// The view the menu follows, found once and kept.
#[derive(Clone)]
struct Linked {
    widget: WidgetRef,
    kind: LinkedKind,
    /// Each section's target inside the view, in section order.
    targets: Vec<WidgetRef>,
}

/// The box a followed view shows, window-absolute, from its area's rect.
///
/// A scrolling `View` records its area from its own scrolled turtle, which
/// is the box. A `ScrollShadowView` answers with its inner view's area, and
/// that view is drawn at full height inside the scroll: its rect is all of
/// the content, moved up by the offset. Headings measured against that never
/// move as the page scrolls, and a jump from anywhere but the top overshoots
/// by the offset, so the box is put back together from the extent. An axis
/// that does not scroll reads zero there and keeps the area's own size.
fn shown_box(kind: LinkedKind, area: Rect, extent: ScrollExtent) -> Rect {
    match kind {
        LinkedKind::View => area,
        LinkedKind::Shadow => Rect {
            pos: area.pos + extent.pos,
            size: dvec2(
                if extent.visible.x > 0.0 { extent.visible.x } else { area.size.x },
                if extent.visible.y > 0.0 { extent.visible.y } else { area.size.y },
            ),
        },
    }
}

impl Linked {
    fn extent(&self) -> Option<ScrollExtent> {
        // A widget mid-dispatch cannot be asked anything.
        self.widget.try_widget_uid()?;
        match self.kind {
            LinkedKind::View => self.widget.borrow::<View>()?.scroll_extent(),
            LinkedKind::Shadow => Some(self.widget.borrow::<ScrollShadowView>()?.scroll_extent()),
        }
    }
}

/// A section lit by a jump, held until the page is scrolled again.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pin {
    index: usize,
    landed: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct JumpAnim {
    from: f64,
    to: f64,
    start: f64,
    secs: f64,
    /// Taken when the jump starts, so a curve changed mid-jump does not bend
    /// the one under way.
    ease: Ease,
    /// The furthest the view could scroll when the jump started.
    furthest: f64,
}

/// What a watch frame compares to know whether anything moved.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Probe {
    pos: f64,
    total: f64,
    visible: f64,
    viewport: Rect,
    first_top: Option<f64>,
}

/// Who showed the names, which decides whether Escape may take them away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RevealedBy {
    #[default]
    Pointer,
    Keys,
    Touch,
}

#[derive(Script, Widget)]
pub struct LineMenu {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    /// The menu's own rect, painted as nothing: the lines are drawn in Rust,
    /// so without it the menu has no rect to focus, redraw or be found by.
    #[redraw]
    #[live]
    draw_bg: DrawNavGround,
    #[live]
    draw_line: DrawLineMenuLine,
    #[live]
    draw_card: DrawMorphSurface,
    #[live]
    draw_row: DrawMorphSurface,
    #[live]
    draw_focus: DrawMorphSurface,
    #[live]
    draw_label: DrawLineMenuText,
    #[live]
    draw_label_current: DrawLineMenuText,

    #[live]
    pub scroll_view: String,
    /// The sections as written in the DSL, parsed when the raw value changes.
    #[live]
    sections: ScriptValue,
    #[live(3.0)]
    pub max_level: f64,
    #[live(true)]
    pub follow_scroll: bool,
    #[live(false)]
    pub always_show_labels: bool,
    #[live(false)]
    pub mirror: bool,
    #[live(18.0)]
    pub row_height: f64,
    #[live(2.0)]
    pub line_thickness: f64,
    #[live(20.0)]
    pub line_length: f64,
    #[live(5.0)]
    pub line_step: f64,
    #[live(6.0)]
    pub line_min: f64,
    #[live(10.0)]
    pub hover_extra: f64,
    #[live(4.0)]
    pub current_extra: f64,
    #[live(10.0)]
    pub label_gap: f64,
    #[live(220.0)]
    pub label_max_width: f64,
    #[live(8.0)]
    pub card_pad: f64,
    #[live(8.0)]
    pub card_radius: f64,
    #[live(0.25)]
    pub spy_line: f64,
    #[live(12.0)]
    pub scroll_margin: f64,
    #[live(0.06)]
    pub reveal_delay: f64,
    #[live(0.2)]
    pub conceal_delay: f64,
    #[live(0.2)]
    pub reveal_secs: f64,
    #[live(Ease::Bezier { cp0: 0.05, cp1: 0.7, cp2: 0.1, cp3: 1.0 })]
    pub reveal_ease: Ease,
    #[live(0.15)]
    pub conceal_secs: f64,
    #[live(Ease::Bezier { cp0: 0.3, cp1: 0.0, cp2: 1.0, cp3: 1.0 })]
    pub conceal_ease: Ease,
    #[live(0.4)]
    pub jump_secs: f64,
    #[live(Ease::Bezier { cp0: 0.2, cp1: 0.0, cp2: 0.0, cp3: 1.0 })]
    pub jump_ease: Ease,
    #[live(false)]
    pub reduced_motion: bool,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    defs: Vec<LineSection>,
    #[rust]
    sections_source: ScriptValue,
    #[rust]
    linked: Option<Linked>,
    /// The path the cached link was looked up for.
    #[rust]
    linked_path: Option<String>,
    /// Each section's heading top in the viewport, from the last watch.
    #[rust]
    tops: Vec<Option<f64>>,
    /// Sections whose target is not in the linked view or not drawn.
    #[rust]
    missing: Vec<bool>,
    #[rust]
    lit: Option<usize>,
    #[rust]
    pin: Option<Pin>,
    #[rust]
    jump: Option<JumpAnim>,
    #[rust]
    probe: Option<Probe>,
    #[rust]
    quiet: u8,
    #[rust]
    watch: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    reveal_state: Reveal,
    #[rust]
    revealed_by: RevealedBy,
    /// 0 collapsed, 1 names fully showing, and past either end while an
    /// overshooting curve carries it.
    #[rust]
    reveal: UnitTween,
    #[rust]
    timer: Timer,
    #[rust]
    timer_deadline: f64,
    /// Row positions, not section indices.
    #[rust]
    hot_row: Option<usize>,
    #[rust]
    key_row: Option<usize>,
    #[rust]
    keyboard_mode: bool,
    #[rust]
    focused: bool,
    #[rust]
    focus_by_press: bool,
    #[rust]
    bg_hovered: bool,
    #[rust]
    pointer_over: bool,
    #[rust]
    locked: bool,
    /// The stack's rect as the event side reads it.
    #[rust]
    rect: Rect,
    #[rust]
    drawn_card: Option<CardLayout>,
    /// The pass the card was last drawn over, which is all that cuts it.
    #[rust]
    overlay_bounds: Rect,
    #[rust]
    overlay: Option<DrawList2d>,
}

impl ScriptHook for LineMenu {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        let parsed = if self.sections.raw() != self.sections_source.raw() {
            self.sections_source = self.sections;
            Some(parse_sections(vm, self.sections))
        } else {
            None
        };
        let cx = vm.cx_mut();
        if let Some(sections) = parsed {
            self.set_sections(cx, sections);
        }
        if self.linked_path.as_deref() != Some(self.scroll_view.as_str()) {
            self.linked = None;
            self.linked_path = None;
        }
        if self.always_show_labels {
            self.reveal = UnitTween::at(1.0);
        }
        self.quiet = 0;
        self.watch = cx.new_next_frame();
        self.repaint(cx);
    }
}

impl LineMenu {
    // -- reading ----------------------------------------------------------

    fn line_metrics(&self) -> LineMetrics {
        LineMetrics {
            line_length: self.line_length,
            line_step: self.line_step,
            line_min: self.line_min,
            hover_extra: self.hover_extra,
            current_extra: self.current_extra,
        }
    }

    fn max_level_u8(&self) -> u8 {
        self.max_level.round().clamp(1.0, 6.0) as u8
    }

    fn levels(&self) -> Vec<u8> {
        self.defs.iter().map(|s| s.level).collect()
    }

    /// The sections that get a row, in order.
    fn rows(&self) -> Vec<usize> {
        let max = self.max_level_u8();
        (0..self.defs.len()).filter(|i| self.defs[*i].level <= max).collect()
    }


    fn section_enabled(&self, index: usize) -> bool {
        !(self.is_linked() && self.missing.get(index).copied().unwrap_or(true))
    }

    /// The section whose row is lit: the lit one, or its shown parent.
    fn lit_row_section(&self) -> Option<usize> {
        shown_row(&self.levels(), self.max_level_u8(), self.lit?)
    }

    fn labeled(&self) -> bool {
        self.always_show_labels
    }

    fn card_showing(&self) -> bool {
        !self.labeled() && !self.reveal.is_at(0.0)
    }

    /// The row rects a pointer lands on right now: the card's rows while the
    /// names show, else the stack's own rows.
    fn row_rects(&self) -> Vec<Rect> {
        let rows = self.rows();
        let pad = self.layout.padding;
        let (x, w) = match self.drawn_card.filter(|_| self.card_showing() && self.reveal_state.shown) {
            Some(card) => (card.card.pos.x, card.card.size.x),
            None => (self.rect.pos.x, self.rect.size.x),
        };
        (0..rows.len())
            .map(|r| Rect {
                pos: dvec2(x, self.rect.pos.y + pad.top + r as f64 * self.row_height),
                size: dvec2(w, self.row_height),
            })
            .collect()
    }

    /// The longest a line can ever be, revealed and lit, with room for a
    /// curve that swings past fully revealed. The stack is that wide and the
    /// names start past it, so no line is cut by the menu's own rect or runs
    /// into the names mid-swing.
    fn max_line(&self, rows: &[usize]) -> f64 {
        let m = self.line_metrics();
        let reach = if self.labeled() { 1.0 } else { reveal_reach(self.reveal_ease, self.conceal_ease) };
        rows.iter().map(|i| line_length(self.defs[*i].level, &m, reach, true)).fold(0.0, f64::max)
    }

    fn row_at(&self, abs: DVec2) -> Option<usize> {
        self.row_rects().iter().position(|r| r.contains(abs))
    }

    fn enabled_rows(&self) -> Vec<bool> {
        self.rows().iter().map(|i| self.section_enabled(*i)).collect()
    }

    // -- motion -----------------------------------------------------------

    fn repaint(&self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
        if let Some(list) = &self.overlay {
            list.redraw(cx);
        }
    }

    /// Read the stack's rect on the event side, where it is honest.
    fn refresh_rect(&mut self, cx: &mut Cx) {
        let rect = self.draw_bg.area().rect(cx);
        if (rect.size.x > 0.0 || rect.size.y > 0.0) && rect != self.rect {
            self.rect = rect;
            // The card is placed from this rect, so it follows a page that
            // moved the stack.
            if self.card_showing() {
                self.repaint(cx);
            }
        }
    }

    fn arm_watch(&mut self, cx: &mut Cx) {
        self.quiet = 0;
        self.watch = cx.new_next_frame();
    }

    fn arm_timer(&mut self, cx: &mut Cx) {
        match self.reveal_state.deadline() {
            Some(deadline) if deadline == self.timer_deadline && !self.timer.is_empty() => {}
            Some(deadline) => {
                cx.stop_timer(self.timer);
                let now = cx.seconds_since_app_start();
                self.timer = cx.start_timeout((deadline - now).max(0.001));
                self.timer_deadline = deadline;
            }
            None => {
                if !self.timer.is_empty() {
                    cx.stop_timer(self.timer);
                    self.timer = Timer::empty();
                }
            }
        }
    }

    /// Animate the names toward what the reducer says, along the reveal's or
    /// the conceal's curve. Answers whether they are still moving.
    fn advance_reveal(&mut self, dt: f64) -> bool {
        if self.labeled() {
            self.reveal = UnitTween::at(1.0);
            return false;
        }
        let (goal, secs, ease) = if self.reveal_state.shown {
            (1.0, self.reveal_secs, self.reveal_ease)
        } else {
            (0.0, self.conceal_secs, self.conceal_ease)
        };
        // Aimed on every frame: names put away mid-reveal, or called back
        // mid-conceal, turn back from where they are.
        self.reveal.aim(goal, secs, ease);
        if self.reduced_motion {
            self.reveal.settle();
            return false;
        }
        self.reveal.step(dt)
    }

    fn apply_reveal(&mut self, cx: &mut Cx, change: Option<bool>, by: RevealedBy) {
        match change {
            Some(true) => {
                self.revealed_by = by;
                // A card over the page takes the pointer, or a press on a
                // name would also land on the words under it. Keys involve no
                // pointer, so a keyboard reveal takes nothing.
                if by != RevealedBy::Keys && !self.locked {
                    let area = self.draw_bg.area();
                    if !area.is_empty() {
                        // A lock a dropped overlay left behind goes first.
                        // The window does this on every event; this is for a
                        // tree with no window above it, where that lock would
                        // otherwise sit under this one and outlive it.
                        release_orphaned_sweep_locks(cx);
                        self.locked = true;
                        cx.sweep_lock(area);
                    }
                }
                self.arm_watch(cx);
                self.repaint(cx);
            }
            Some(false) => {
                self.unlock(cx);
                self.hot_row = None;
                self.arm_watch(cx);
                self.repaint(cx);
            }
            None => {}
        }
        self.arm_timer(cx);
    }

    fn unlock(&mut self, cx: &mut Cx) {
        if self.locked {
            self.locked = false;
            cx.sweep_unlock(self.draw_bg.area());
        }
    }

    fn hide(&mut self, cx: &mut Cx) {
        if self.reveal_state.shown {
            self.reveal_state.shown = false;
            self.reveal_state.pending = None;
            self.apply_reveal(cx, Some(false), self.revealed_by);
        }
    }

    // -- the linked view ----------------------------------------------------

    fn resolve_linked(&mut self, cx: &Cx) {
        if let Some(linked) = &self.linked {
            if !linked.widget.is_empty() && self.linked_path.as_deref() == Some(self.scroll_view.as_str()) {
                if linked.targets.len() == self.defs.len() {
                    return;
                }
            }
        }
        self.linked = None;
        let path = id_path(&self.scroll_view);
        if path.is_empty() {
            return;
        }
        // Outward from the menu, as a tour step finds its target; the menu
        // has no children of its own to hand the tree.
        let found = cx.widget_tree().find_flood_from_borrowed(self.uid, &path, |_| {});
        if found.is_empty() || found.try_widget_uid().is_none() {
            return;
        }
        let kind = if found.borrow::<View>().map_or(false, |view| view.scroll_extent().is_some()) {
            LinkedKind::View
        } else if found.borrow::<ScrollShadowView>().is_some() {
            LinkedKind::Shadow
        } else {
            return;
        };
        let targets = self.defs.iter().map(|s| found.widget(cx, &id_path(&s.target))).collect();
        self.linked = Some(Linked { widget: found, kind, targets });
        self.linked_path = Some(self.scroll_view.clone());
    }

    /// Read where the view is and where each heading sits in it.
    fn read_geometry(&mut self, cx: &Cx) -> Option<(Probe, ScrollExtent)> {
        self.resolve_linked(cx);
        let n = self.defs.len();
        let Some(linked) = &self.linked else {
            self.tops = vec![None; n];
            self.missing = vec![false; n];
            return None;
        };
        let extent = linked.extent()?;
        let viewport = shown_box(linked.kind, askable_area(&linked.widget)?.rect(cx), extent);
        let mut tops = Vec::with_capacity(n);
        for target in &linked.targets {
            let rect = if target.is_empty() { None } else { askable_area(target).map(|area| area.rect(cx)) };
            tops.push(rect.filter(|r| r.size.x > 0.0 && r.size.y > 0.0).map(|r| r.pos.y - viewport.pos.y));
        }
        self.missing = tops.iter().map(|t| t.is_none()).collect();
        let first_top = tops.iter().flatten().next().copied();
        self.tops = tops;
        Some((
            Probe { pos: extent.pos.y, total: extent.total.y, visible: extent.visible.y, viewport, first_top },
            extent,
        ))
    }

    fn scroll_linked(&mut self, cx: &mut Cx, y: f64) {
        let Some(linked) = self.linked.clone() else {
            return;
        };
        let Some(extent) = linked.extent() else {
            return;
        };
        let to = dvec2(extent.pos.x, y);
        match linked.kind {
            LinkedKind::View => {
                linked.widget.set_scroll_pos(cx, to);
                cx.redraw_area_and_children(linked.widget.area());
            }
            LinkedKind::Shadow => {
                if let Some(mut shadow) = linked.widget.borrow_mut::<ScrollShadowView>() {
                    shadow.set_scroll(cx, to);
                }
            }
        }
    }

    fn set_lit(&mut self, cx: &mut Cx, lit: Option<usize>) {
        if lit == self.lit {
            return;
        }
        self.lit = lit;
        self.repaint(cx);
        if let Some(section) = lit.and_then(|i| self.defs.get(i)) {
            let uid = self.widget_uid();
            cx.widget_action(uid, LineMenuAction::Changed(section.id));
        }
    }

    fn watch_frame(&mut self, cx: &mut Cx, time: f64) {
        let dt = if self.last_time > 0.0 { (time - self.last_time).clamp(0.0, 0.1) } else { 0.0 };
        let before = self.reveal;
        let revealing = self.advance_reveal(dt);
        // Only what changed is redrawn. A watch that redrew every frame
        // would wake itself: the redraw is an event, and every event arms it.
        if self.reveal != before {
            self.repaint(cx);
        }
        self.refresh_rect(cx);

        // The jump moves the view before it is read, so what is read is where
        // it went.
        if let Some(jump) = self.jump {
            let now = cx.seconds_since_app_start();
            let t = if jump.secs > 0.0 { (now - jump.start) / jump.secs } else { 1.0 };
            let y = jump_offset(jump.from, jump.to, t, jump.ease, jump.furthest);
            self.scroll_linked(cx, y);
            if t >= 1.0 {
                self.jump = None;
            }
        }

        let missing_before = self.missing.clone();
        let read = self.read_geometry(cx);
        let probe = read.map(|(p, _)| p);
        if probe != self.probe {
            self.probe = probe;
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        if self.missing != missing_before {
            self.repaint(cx);
        }

        if let Some((probe, extent)) = read {
            if let Some(pin) = &mut self.pin {
                if self.jump.is_none() && pin_released(pin.landed, probe.pos) {
                    self.pin = None;
                }
            }
            let lit = if let Some(pin) = self.pin {
                Some(pin.index)
            } else if self.follow_scroll {
                let line = (probe.viewport.size.y * self.spy_line).max(self.scroll_margin + 1.0);
                current_section(&self.tops, probe.viewport.size.y, line, extent.at_end_y(1.0))
            } else {
                self.lit
            };
            self.set_lit(cx, lit);
        }

        if self.quiet < QUIET_FRAMES || self.jump.is_some() || revealing {
            self.last_time = time;
            self.watch = cx.new_next_frame();
        } else {
            self.last_time = 0.0;
        }
    }

    fn start_jump(&mut self, cx: &mut Cx, index: usize, user: bool) {
        let Some(id) = self.defs.get(index).map(|s| s.id) else {
            return;
        };
        if user {
            let uid = self.widget_uid();
            cx.widget_action(uid, LineMenuAction::Jumped(id));
        }
        let Some((_, extent)) = self.read_geometry(cx) else {
            return;
        };
        let Some(top) = self.tops.get(index).copied().flatten() else {
            return;
        };
        let from = extent.pos.y;
        let to = jump_target(from, top, self.scroll_margin, extent.total.y, extent.visible.y);
        self.pin = Some(Pin { index, landed: to });
        // Lit on the press rather than at the end of the scroll.
        self.set_lit(cx, Some(index));
        if self.reduced_motion || self.jump_secs <= 0.0 || (to - from).abs() < 1.0 {
            self.jump = None;
            self.scroll_linked(cx, to);
        } else {
            let start = cx.seconds_since_app_start();
            self.jump = Some(JumpAnim { from, to, start, secs: self.jump_secs, ease: self.jump_ease, furthest: extent.max().y });
        }
        // Where the view actually stopped, which a view that clamps its own
        // offset may make a hair different from where it was sent.
        if let Some(landed) = self.linked.as_ref().and_then(|l| l.extent()).map(|e| e.pos.y) {
            if self.jump.is_none() {
                if let Some(pin) = &mut self.pin {
                    pin.landed = landed;
                }
            }
        }
        self.arm_watch(cx);
    }

    // -- the public surface -------------------------------------------------

    /// Replace the sections. The lit section is kept by id when it is still
    /// there; the pin and any jump are dropped.
    pub fn set_sections(&mut self, cx: &mut Cx, sections: Vec<LineSection>) {
        let lit_id = self.lit.and_then(|i| self.defs.get(i)).map(|s| s.id);
        self.defs = sections;
        self.lit = lit_id.and_then(|id| self.defs.iter().position(|s| s.id == id));
        self.linked = None;
        self.tops = vec![None; self.defs.len()];
        self.missing = vec![false; self.defs.len()];
        self.pin = None;
        self.jump = None;
        self.probe = None;
        self.hot_row = None;
        self.key_row = None;
        self.arm_watch(cx);
        self.repaint(cx);
    }

    pub fn sections(&self) -> &[LineSection] {
        &self.defs
    }

    pub fn set_scroll_view(&mut self, cx: &mut Cx, path: &str) {
        self.scroll_view = path.to_string();
        self.linked = None;
        self.linked_path = None;
        self.pin = None;
        self.jump = None;
        self.arm_watch(cx);
    }

    pub fn current(&self) -> Option<LiveId> {
        self.lit.and_then(|i| self.defs.get(i)).map(|s| s.id)
    }

    /// Light a section as a jump would, and hold it until the view has been
    /// scrolled by more than 2 pt. Raises Changed.
    pub fn set_current(&mut self, cx: &mut Cx, id: LiveId) {
        let Some(index) = self.defs.iter().position(|s| s.id == id) else {
            return;
        };
        // Found first: asked before any watch frame has looked for the view,
        // the pin would hold against an offset of 0, and on a page already
        // scrolled the first frame would let go of it at once.
        self.read_geometry(cx);
        let landed = self.linked.as_ref().and_then(|l| l.extent()).map_or(0.0, |e| e.pos.y);
        self.pin = Some(Pin { index, landed });
        self.set_lit(cx, Some(index));
        self.arm_watch(cx);
    }

    /// Scroll the linked view to the section, as a press would, without
    /// raising Jumped.
    pub fn jump_to(&mut self, cx: &mut Cx, id: LiveId) {
        if let Some(index) = self.defs.iter().position(|s| s.id == id) {
            self.start_jump(cx, index, false);
        }
    }

    /// Read the geometry again now, for a host that knows its content moved
    /// without an event.
    pub fn refresh(&mut self, cx: &mut Cx) {
        self.linked = None;
        self.arm_watch(cx);
    }

    /// Whether a view to follow was found.
    pub fn is_linked(&self) -> bool {
        self.linked.is_some()
    }

    // -- drawing ------------------------------------------------------------

    fn draw_line_at(&mut self, cx: &mut Cx2d, x_left: f64, x_right: f64, mirror: bool, y: f64, len: f64, state: (f32, f32, f32, f32)) {
        let x = if mirror { x_right - len } else { x_left };
        self.draw_line.hot = state.0;
        self.draw_line.current = state.1;
        self.draw_line.ancestor = state.2;
        self.draw_line.opacity = state.3;
        self.draw_line.draw_abs(cx, Rect { pos: dvec2(x, y), size: dvec2(len.max(0.0), self.line_thickness) });
    }

    fn draw_overlay(&mut self, cx: &mut Cx2d, rows: &[usize], widest: f64) {
        let Some(mut list) = self.overlay.take() else {
            return;
        };
        // Begun on every draw, shown or not: a list that is not begun keeps
        // showing what it showed last.
        list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        self.overlay_bounds = Rect { pos: dvec2(0.0, 0.0), size: pass };
        cx.begin_root_turtle(pass, Layout::flow_down());
        if self.card_showing() && !rows.is_empty() && self.rect.size.y > 0.0 {
            self.draw_card_now(cx, pass, rows, widest);
        } else {
            self.drawn_card = None;
        }
        cx.end_pass_sized_turtle();
        list.end(cx);
        self.overlay = Some(list);
    }

    fn draw_card_now(&mut self, cx: &mut Cx2d, pass: DVec2, rows: &[usize], widest: f64) {
        let m = self.line_metrics();
        let pad = self.layout.padding;
        let stack = self.rect;
        let eased = self.reveal.value();
        let levels = self.levels();
        let lit = self.lit_row_section();
        let lit_ancestors = lit.map(|l| ancestors(&levels, l)).unwrap_or_default();
        let max_line = self.max_line(rows);
        let x_left = stack.pos.x + pad.left;
        let x_right = stack.pos.x + stack.size.x - pad.right;
        let (lines_start, lines_end) = if self.mirror { (x_right - max_line, x_right) } else { (x_left, x_left + max_line) };
        let card = card_layout(
            stack,
            lines_start,
            lines_end,
            widest,
            &CardMetrics { label_gap: self.label_gap, card_pad: self.card_pad, label_max_width: self.label_max_width, min_label: MIN_LABEL },
            Rect { pos: dvec2(EDGE, EDGE), size: pass - dvec2(EDGE * 2.0, EDGE * 2.0) },
            self.mirror,
        );
        self.drawn_card = Some(card);
        // Read off the furthest the reveal has come, so a bounce that dips
        // back does not dim names it has already shown.
        let alpha = smootherstep((self.reveal.reached() - 0.3) / 0.7) as f32;

        self.draw_card.radius = self.card_radius as f32;
        self.draw_card.morph = 1.0;
        self.draw_card.opacity = alpha;
        self.draw_card.draw_abs(cx, card.card);

        let key = if self.keyboard_mode && self.focused { self.key_row } else { None };
        for (r, &section) in rows.iter().enumerate() {
            let y = stack.pos.y + pad.top + r as f64 * self.row_height;
            let band = Rect { pos: dvec2(card.card.pos.x, y), size: dvec2(card.card.size.x, self.row_height) };
            let enabled = self.section_enabled(section);
            let hot = enabled && self.hot_row == Some(r);
            if hot || key == Some(r) {
                self.draw_row.radius = (self.row_height * 0.25).max(2.0) as f32;
                self.draw_row.morph = 1.0;
                self.draw_row.opacity = alpha;
                self.draw_row.draw_abs(cx, band.add_margin(dvec2(-self.card_pad * 0.5, 0.0)));
            }
            // The line again, over the card, exactly where the stack drew it,
            // so nothing ghosts through while the card fades.
            let is_lit = lit == Some(section);
            let len = line_length(levels[section], &m, eased, is_lit).min(max_line);
            let dim = if enabled { 1.0 } else { self.disabled_opacity };
            let state = (
                if hot || key == Some(r) { 1.0 } else { 0.0 },
                if is_lit { 1.0 } else { 0.0 },
                if lit_ancestors.contains(&section) { 1.0 } else { 0.0 },
                dim,
            );
            let line_y = y + (self.row_height - self.line_thickness) * 0.5;
            self.draw_line_at(cx, x_left, x_right, self.mirror, line_y, len, state);

            let label = self.defs[section].label.clone();
            let on_surface = self.draw_label_current.color;
            let draw = if is_lit { &mut self.draw_label_current } else { &mut self.draw_label };
            let rest = draw.color;
            if hot && !is_lit {
                // The hot name takes the lit name's ink for this one draw.
                draw.color = on_surface;
            }
            draw.opacity = alpha * dim;
            let text = elide(draw, cx, &label, card.label_w);
            let size = measure(draw, cx, &text);
            let x = if card.labels_before { card.label_x + card.label_w - size.x } else { card.label_x };
            draw.draw_abs(cx, dvec2(x, y + (self.row_height - size.y) * 0.5), &text);
            draw.color = rest;

            if key == Some(r) && self.reveal.is_at(1.0) {
                self.draw_focus.radius = ((self.row_height * 0.25).max(2.0) + RING_OUT) as f32;
                self.draw_focus.opacity = 1.0;
                self.draw_focus.draw_abs(cx, band.add_margin(dvec2(RING_OUT - self.card_pad * 0.5, RING_OUT)));
            }
        }
    }

    // -- events -------------------------------------------------------------

    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let enabled = self.enabled_rows();
        let first = enabled.iter().position(|e| *e);
        let last = enabled.iter().rposition(|e| *e);
        // Keys that start from nowhere start from the lit row: focus given by
        // a press sets no row, and the first Down should be the row after the
        // one being read, not the top of the list.
        let rows = self.rows();
        let lit_row = self
            .lit_row_section()
            .and_then(|s| rows.iter().position(|r| *r == s))
            .filter(|r| enabled.get(*r).copied().unwrap_or(false));
        let from = self.key_row.filter(|r| *r < enabled.len()).or(lit_row);
        let moved = match ke.key_code {
            // Clamped, not wrapped: a table of contents has a top and a
            // bottom, and leaping from the end to the start is a surprise.
            KeyCode::ArrowDown => Some(match from {
                Some(r) => enabled[r + 1..].iter().position(|e| *e).map(|k| r + 1 + k).or(Some(r)),
                None => first,
            }),
            KeyCode::ArrowUp => Some(match from {
                Some(r) => enabled[..r].iter().rposition(|e| *e).or(Some(r)),
                None => last,
            }),
            KeyCode::Home => Some(first),
            KeyCode::End => Some(last),
            KeyCode::ReturnKey | KeyCode::Space => {
                if let Some(section) = from.and_then(|r| rows.get(r).copied()) {
                    if self.section_enabled(section) {
                        self.start_jump(cx, section, true);
                    }
                }
                None
            }
            _ => return,
        };
        let was = self.keyboard_mode;
        self.keyboard_mode = true;
        if !was {
            let change = self.reveal_state.focus(true, self.pointer_over);
            self.apply_reveal(cx, change, RevealedBy::Keys);
        }
        if let Some(to) = moved {
            self.key_row = to;
        }
        self.arm_watch(cx);
        self.repaint(cx);
    }

    fn pointer_moved(&mut self, cx: &mut Cx, abs: Option<DVec2>) {
        if !(self.bg_hovered || self.reveal_state.shown || self.reveal_state.pending.is_some()) {
            return;
        }
        // A pointer another control holds is not this menu's to read. From
        // the press until the release the interaction is locked to whatever
        // took it, and a row lighting up — or a card of names unfolding —
        // under a hand that is dragging a scroll bar somewhere else is
        // exactly the hover the rule forbids. This is read off the raw move,
        // which never learns of that press the way `hits` does.
        //
        // Held still rather than cleared: the drag ends, the next move finds
        // the pointer where it is and the menu catches up. Clearing would
        // fold the names away mid-drag, which is the same flinch seen from
        // the other side.
        let mine = [self.draw_bg.area(), self.draw_card.area()];
        if cx.fingers.is_mouse_held_outside(&mine) {
            return;
        }
        let over = abs.map_or(false, |abs| {
            (self.bg_hovered && self.rect.contains(abs))
                || (self.reveal_state.shown && self.drawn_card.map_or(false, |card| card.card.contains(abs)))
        });
        self.pointer_over = over;
        let hot = if over { abs.and_then(|abs| self.row_at(abs)) } else { None };
        let hot = hot.filter(|r| self.rows().get(*r).map_or(false, |s| self.section_enabled(*s)));
        if over && self.reveal_state.shown {
            cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
        }
        if hot != self.hot_row {
            self.hot_row = hot;
            self.repaint(cx);
        }
        if self.labeled() {
            return;
        }
        // Keys that showed the names keep them; the pointer leaving is no
        // reason to take them from someone reading with the keys.
        if self.keyboard_mode && self.focused {
            return;
        }
        let now = cx.seconds_since_app_start();
        let change = self.reveal_state.pointer(over, now, self.reveal_delay, self.conceal_delay);
        self.apply_reveal(cx, change, RevealedBy::Pointer);
    }

    fn press_row(&mut self, cx: &mut Cx, row: usize, touch: bool) {
        let Some(section) = self.rows().get(row).copied() else {
            return;
        };
        if !self.section_enabled(section) {
            return;
        }
        self.start_jump(cx, section, true);
        // A touch jump puts the names away; a pointer jump leaves them while
        // the pointer is still over them, so the reader can go straight on.
        if touch && !self.labeled() {
            self.hide(cx);
        }
    }

    fn handle_card_hits(&mut self, cx: &mut Cx, event: &Event) {
        if !(self.card_showing() && self.reveal_state.shown) {
            return;
        }
        match event.hits_with_sweep_area(cx, self.draw_card.area(), self.draw_bg.area()) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.focus_by_press = !self.focused;
                cx.set_key_focus(self.draw_bg.area());
                self.keyboard_mode = false;
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && fe.is_over && fe.was_tap() => {
                if let Some(row) = self.row_at(fe.abs) {
                    self.press_row(cx, row, fe.device.is_touch());
                }
            }
            _ => {}
        }
    }

    fn handle_bg_hits(&mut self, cx: &mut Cx, event: &Event) {
        // Its own sweep area, so the lock this menu takes does not turn its
        // own stack away.
        let bg = self.draw_bg.area();
        match event.hits_with_sweep_area(cx, bg, bg) {
            Hit::KeyFocus(_) => {
                self.focused = true;
                if !self.focus_by_press {
                    // Arrived by Tab: show the names and stand on the lit row.
                    self.keyboard_mode = true;
                    let rows = self.rows();
                    let enabled = self.enabled_rows();
                    self.key_row = self
                        .lit_row_section()
                        .and_then(|s| rows.iter().position(|r| *r == s))
                        .filter(|r| enabled.get(*r).copied().unwrap_or(false))
                        .or_else(|| enabled.iter().position(|e| *e));
                    let change = self.reveal_state.focus(true, self.pointer_over);
                    self.apply_reveal(cx, change, RevealedBy::Keys);
                }
                self.focus_by_press = false;
                self.repaint(cx);
            }
            Hit::KeyFocusLost(_) => {
                self.focused = false;
                self.keyboard_mode = false;
                if !self.labeled() {
                    let change = self.reveal_state.focus(false, self.pointer_over);
                    self.apply_reveal(cx, change, self.revealed_by);
                }
                self.repaint(cx);
            }
            Hit::KeyDown(ke) => self.handle_key(cx, &ke),
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                self.bg_hovered = true;
            }
            Hit::FingerHoverOut(_) => {
                self.bg_hovered = false;
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.focus_by_press = !self.focused;
                cx.set_key_focus(self.draw_bg.area());
                self.keyboard_mode = false;
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && fe.is_over && fe.was_tap() => {
                let touch = fe.device.is_touch();
                if touch && !self.labeled() && !self.reveal_state.shown {
                    // A tap has no hover to rest with: the first one shows the
                    // names, and the row is chosen on the card.
                    let change = self.reveal_state.touch_toggle();
                    self.apply_reveal(cx, change, RevealedBy::Touch);
                } else if let Some(row) = self.row_at(fe.abs) {
                    self.press_row(cx, row, touch);
                }
            }
            _ => {}
        }
    }

    fn inside(&self, abs: DVec2) -> bool {
        self.rect.contains(abs) || self.drawn_card.map_or(false, |card| card.card.contains(abs))
    }

    fn handle_raw(&mut self, cx: &mut Cx, event: &Event) {
        // A jump hands the scroll back to the reader the moment they reach
        // for the page.
        if self.jump.is_some() {
            let viewport = self.probe.map(|p| p.viewport).unwrap_or_default();
            let cancel = match event {
                Event::Scroll(se) => viewport.contains(se.abs),
                Event::MouseDown(me) => viewport.contains(me.abs),
                Event::TouchUpdate(te) => te.touches.iter().any(|t| t.state == TouchState::Start && viewport.contains(t.abs)),
                _ => false,
            };
            if cancel {
                self.jump = None;
                self.pin = None;
                self.arm_watch(cx);
            }
        }
        match event {
            Event::MouseMove(me) => {
                if self.keyboard_mode {
                    self.keyboard_mode = false;
                    self.repaint(cx);
                }
                self.pointer_moved(cx, Some(me.abs));
            }
            Event::MouseLeave(_) | Event::ClearHover => self.pointer_moved(cx, None),
            Event::MouseDown(me) if self.locked => {
                if !self.inside(me.abs) {
                    if me.handled.get().is_empty() {
                        me.handled.set(self.draw_bg.area());
                    }
                    self.hide(cx);
                }
            }
            Event::TouchUpdate(te) => {
                if self.keyboard_mode {
                    self.keyboard_mode = false;
                    self.repaint(cx);
                }
                if self.locked
                    && !te.touches.is_empty()
                    && te.touches.iter().all(|t| t.state == TouchState::Start && !self.inside(t.abs))
                {
                    self.hide(cx);
                }
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape && self.reveal_state.shown && !self.labeled() => {
                // Only names the keys or a tap showed are the keys' to take
                // away; hovered names go when the pointer does.
                if matches!(self.revealed_by, RevealedBy::Keys | RevealedBy::Touch) {
                    let top = cx.sweep_lock_area();
                    let mine = top.is_none() || (self.locked && top == Some(self.draw_bg.area()));
                    if mine && claim_escape(cx) {
                        self.hide(cx);
                    }
                }
            }
            Event::WindowLostFocus(_) => {
                if self.locked {
                    self.hide(cx);
                }
            }
            _ => {}
        }
    }
}

impl Drop for LineMenu {
    fn drop(&mut self) {
        // Dropped holding the pointer, as when its page is swapped for
        // another while the card is out: nothing else would ever let go of
        // the lock, and every press in the window would go on being turned
        // away. A drop has no `Cx`, so the lock is left for the next event to
        // release.
        if self.locked {
            orphan_sweep_locks(&[self.draw_bg.area()]);
        }
    }
}

impl Widget for LineMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            self.draw_overlay(cx, &[], 0.0);
            return DrawStep::done();
        }
        let m = self.line_metrics();
        let pad = self.layout.padding;
        let rows = self.rows();
        let levels = self.levels();
        let max_line = self.max_line(&rows);
        // Every name is measured in the bold face, so the column does not
        // shift when the lit name, drawn bold, moves.
        let labels: Vec<String> = rows.iter().map(|i| self.defs[*i].label.clone()).collect();
        let widest = labels.iter().map(|label| measure(&self.draw_label_current, cx, label).x).fold(0.0, f64::max);
        let labeled = self.labeled();
        let label_w = widest.min(self.label_max_width);
        let natural = dvec2(
            pad.left + pad.right + max_line + if labeled { self.label_gap + label_w } else { 0.0 },
            pad.top + pad.bottom + rows.len() as f64 * self.row_height,
        );
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural.x),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural.y),
                other => other,
            },
            ..walk
        };
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        if labeled {
            self.reveal = UnitTween::at(1.0);
        }

        let lit = self.lit_row_section();
        let lit_ancestors = lit.map(|l| ancestors(&levels, l)).unwrap_or_default();
        let key = if self.keyboard_mode && self.focused { self.key_row } else { None };
        let x_left = rect.pos.x + pad.left;
        let x_right = rect.pos.x + rect.size.x - pad.right;
        let eased = self.reveal.value();
        for (r, &section) in rows.iter().enumerate() {
            let y = rect.pos.y + pad.top + r as f64 * self.row_height;
            let enabled = self.section_enabled(section);
            let hot = enabled && self.hot_row == Some(r);
            let is_lit = lit == Some(section);
            let band = Rect { pos: dvec2(rect.pos.x, y), size: dvec2(rect.size.x, self.row_height) };
            if labeled && (hot || key == Some(r)) {
                self.draw_row.radius = (self.row_height * 0.25).max(2.0) as f32;
                self.draw_row.morph = 1.0;
                self.draw_row.opacity = 1.0;
                self.draw_row.draw_abs(cx, band);
            }
            let len = line_length(levels[section], &m, eased, is_lit).min(max_line);
            let state = (
                if hot || key == Some(r) { 1.0 } else { 0.0 },
                if is_lit { 1.0 } else { 0.0 },
                if lit_ancestors.contains(&section) { 1.0 } else { 0.0 },
                if enabled { 1.0 } else { self.disabled_opacity },
            );
            let line_y = y + (self.row_height - self.line_thickness) * 0.5;
            self.draw_line_at(cx, x_left, x_right, self.mirror, line_y, len, state);

            if labeled {
                let label = labels[r].clone();
                let on_surface = self.draw_label_current.color;
                let dim = if enabled { 1.0 } else { self.disabled_opacity };
                let draw = if is_lit { &mut self.draw_label_current } else { &mut self.draw_label };
                let rest = draw.color;
                if hot && !is_lit {
                    draw.color = on_surface;
                }
                draw.opacity = dim;
                let text = elide(draw, cx, &label, label_w);
                let size = measure(draw, cx, &text);
                let x = if self.mirror { x_right - max_line - self.label_gap - size.x } else { x_left + max_line + self.label_gap };
                draw.draw_abs(cx, dvec2(x, y + (self.row_height - size.y) * 0.5), &text);
                draw.color = rest;
            }
            if key == Some(r) && !self.card_showing() {
                self.draw_focus.radius = ((self.row_height * 0.25).max(2.0) + RING_OUT) as f32;
                self.draw_focus.opacity = 1.0;
                self.draw_focus.draw_abs(cx, band.add_margin(dvec2(0.0, RING_OUT)));
            }
        }

        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        if self.locked {
            cx.sweep_lock(self.draw_bg.area());
        }
        // The first draw arms the watch, so a page opened part way down lights
        // the right section without waiting for the reader to move.
        if self.probe.is_none() && !self.defs.is_empty() {
            self.watch = cx.new_next_frame();
        }
        self.draw_overlay(cx, &rows, widest);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(ne) = self.watch.is_event(event) {
            self.watch_frame(cx, ne.time);
        }
        if self.timer.is_event(event).is_some() {
            self.timer = Timer::empty();
            let now = cx.seconds_since_app_start();
            let change = self.reveal_state.tick(now);
            self.apply_reveal(cx, change, RevealedBy::Pointer);
        }
        // Scrolling, dragging, flinging, resizing and a host's own
        // `set_scroll_pos` all arrive with or after an event of some kind;
        // frames and timers are the watch's own and do not re-arm it.
        if !matches!(event, Event::NextFrame(_) | Event::Timer(_)) {
            self.arm_watch(cx);
        }
        self.refresh_rect(cx);
        if !self.visible {
            return;
        }
        self.handle_card_hits(cx, event);
        self.handle_bg_hits(cx, event);
        self.handle_raw(cx, event);
    }

    /// The lit section's label.
    fn text(&self) -> String {
        self.lit.and_then(|i| self.defs.get(i)).map(|s| s.label.clone()).unwrap_or_default()
    }

    fn snapshot_selected(&self, _cx: &Cx) -> Option<String> {
        self.lit.and_then(|i| self.defs.get(i)).map(|s| s.label.clone())
    }

    /// The lit section's index, so a test can wait on a number.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.lit.map(|i| i.to_string()).unwrap_or_default())
    }

    /// Each row where it can be pressed now, cut to what shows: an ancestor
    /// that clips the stack clips its rows, and the card, on an overlay, is
    /// cut only by the window. A row none of which shows is left out.
    fn snapshot_parts(&self, cx: &Cx) -> Vec<SnapshotPart> {
        let area = self.draw_bg.area();
        // Not drawn this frame (another tab, a page swapped away): a stale
        // area reads as a zero rect, and rows placed from it would send a
        // test to press empty space by the window's corner.
        if !area.is_valid(cx) {
            return Vec::new();
        }
        let stack = area.rect(cx);
        let pad = self.layout.padding;
        let lit = self.lit_row_section();
        let card = self.drawn_card.filter(|_| self.card_showing() && self.reveal.is_at(1.0));
        let (x, w, shown) = match card {
            Some(card) => (card.card.pos.x, card.card.size.x, self.overlay_bounds),
            None => (stack.pos.x, stack.size.x, area.clipped_rect(cx)),
        };
        self.rows()
            .into_iter()
            .enumerate()
            .filter_map(|(r, section)| {
                let row = Rect {
                    pos: dvec2(x, stack.pos.y + pad.top + r as f64 * self.row_height),
                    size: dvec2(w, self.row_height),
                };
                let rect = row.clip((shown.pos, shown.pos + shown.size));
                (rect.size.x > 0.0 && rect.size.y > 0.0).then(|| SnapshotPart {
                    id: self.defs[section].id,
                    widget_type: "LineMenuSection",
                    rect,
                    text: self.defs[section].label.clone(),
                    selected: lit == Some(section),
                    enabled: self.section_enabled(section),
                })
            })
            .collect()
    }
}

impl LineMenuRef {
    fn each_action(&self, actions: &Actions) -> Vec<LineMenuAction> {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == uid)
            .map(|action| action.cast::<LineMenuAction>())
            .collect()
    }

    /// The section lit this pass, if it changed.
    pub fn changed(&self, actions: &Actions) -> Option<LiveId> {
        self.each_action(actions).into_iter().rev().find_map(|a| match a {
            LineMenuAction::Changed(id) => Some(id),
            _ => None,
        })
    }

    /// The section a row was activated for this pass.
    pub fn jumped(&self, actions: &Actions) -> Option<LiveId> {
        self.each_action(actions).into_iter().find_map(|a| match a {
            LineMenuAction::Jumped(id) => Some(id),
            _ => None,
        })
    }

    pub fn set_sections(&self, cx: &mut Cx, sections: Vec<LineSection>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_sections(cx, sections);
        }
    }

    pub fn sections(&self) -> Vec<LineSection> {
        self.borrow().map(|inner| inner.sections().to_vec()).unwrap_or_default()
    }

    pub fn set_scroll_view(&self, cx: &mut Cx, path: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_scroll_view(cx, path);
        }
    }

    pub fn current(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.current())
    }

    pub fn set_current(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_current(cx, id);
        }
    }

    pub fn jump_to(&self, cx: &mut Cx, id: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.jump_to(cx, id);
        }
    }

    pub fn refresh(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.refresh(cx);
        }
    }

    pub fn is_linked(&self) -> bool {
        self.borrow().map_or(false, |inner| inner.is_linked())
    }

    /// The label of the section with `id`, for a host turning a report into
    /// words.
    pub fn label_of(&self, id: LiveId) -> String {
        self.borrow()
            .and_then(|inner| inner.sections().iter().find(|s| s.id == id).map(|s| s.label.clone()))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }
    use super::*;
    use crate::makepad_script::script;
    use crate::makepad_script::trap::NoTrap;

    fn metrics() -> LineMetrics {
        LineMetrics { line_length: 20.0, line_step: 5.0, line_min: 6.0, hover_extra: 10.0, current_extra: 4.0 }
    }

    fn card_metrics() -> CardMetrics {
        CardMetrics { label_gap: 10.0, card_pad: 8.0, label_max_width: 220.0, min_label: 40.0 }
    }

    /// The easing a theme token names, read out of the loaded theme as the
    /// Foundations page reads it, so these checks follow the theme and never
    /// restate it.
    fn theme_ease(vm: &mut ScriptVm, token: &str) -> Ease {
        let theme = vm.module(id!(theme));
        let value = vm.bx.heap.value(theme, LiveId::from_str(token).into(), NoTrap);
        Ease::script_from_value(vm, value)
    }

    fn theme_number(vm: &mut ScriptVm, token: &str) -> f64 {
        let theme = vm.module(id!(theme));
        vm.bx
            .heap
            .value(theme, LiveId::from_str(token).into(), NoTrap)
            .as_f64()
            .unwrap_or_else(|| panic!("the theme has no number {token}"))
    }

    /// The reveal's curves and times are the theme's tokens, and a host can
    /// name another token for either curve.
    #[test]
    fn the_reveal_takes_its_curves_and_times_from_the_theme() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                LineMenu{}
            });
            let menu = LineMenu::script_from_value(vm, value);
            assert_eq!(menu.reveal_ease, theme_ease(vm, "motion_ease_emphasized_decelerate"));
            assert_eq!(menu.conceal_ease, theme_ease(vm, "motion_ease_standard_accelerate"));
            assert_eq!(menu.reveal_secs, theme_number(vm, "motion_short_4"));
            assert_eq!(menu.conceal_secs, theme_number(vm, "motion_short_3"));
            assert_eq!(menu.jump_ease, theme_ease(vm, "motion_ease_standard"), "the jump's curve is a theme easing");
            assert_eq!(menu.jump_secs, theme_number(vm, "motion_medium_4"));
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                LineMenu{reveal_ease: theme.motion_ease_spring conceal_ease: theme.motion_ease_linear jump_ease: theme.motion_ease_bounce}
            });
            let sprung = LineMenu::script_from_value(vm, value);
            assert_eq!(sprung.reveal_ease, theme_ease(vm, "motion_ease_spring"));
            assert_eq!(sprung.conceal_ease, theme_ease(vm, "motion_ease_linear"));
            assert_eq!(sprung.jump_ease, theme_ease(vm, "motion_ease_bounce"));
        });
        });
    }

    #[test]
    fn only_a_curve_that_overshoots_widens_the_stack() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let (decelerate, accelerate, spring, bounce) = cx.with_vm(|vm| {
            (
                theme_ease(vm, "motion_ease_emphasized_decelerate"),
                theme_ease(vm, "motion_ease_standard_accelerate"),
                theme_ease(vm, "motion_ease_spring"),
                theme_ease(vm, "motion_ease_bounce"),
            )
        });
        let near = |a: f64, b: f64| (a - b).abs() < 1e-3;
        assert!(near(reveal_reach(decelerate, accelerate), 1.0), "the default curves stay between their ends");
        assert!(near(reveal_reach(bounce, bounce), 1.0), "a bounce never goes past its end");
        let (low, high) = ease_bounds(spring);
        assert!(high > 1.2, "a spring swings past its end: {high}");
        assert_eq!(low, 0.0, "and never below its start");
        assert_eq!(reveal_reach(spring, accelerate), high);
        let m = metrics();
        assert!(line_length(1, &m, high, false) > line_length(1, &m, 1.0, false), "the swing lengthens a line past its revealed length");
        });
    }

    /// The DSL only fails at eval time, so the gate is a real registration:
    /// build the menu from `mod.widgets.LineMenu` with the sections a host
    /// writes and read back what the apply parsed.
    #[test]
    fn the_dsl_registers_and_the_sections_parse() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let menu = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                LineMenu{
                    scroll_view: "a"
                    max_level: 2
                    sections: [
                        {target: "page.intro" label: "Introduction" level: 1}
                        {target: "setup" id: @getting_ready label: "Setup" level: 2}
                        {target: "deep" label: "Deep" level: 9}
                        {target: "flat" label: "No level"}
                        {target: "silent" level: 2}
                    ]
                }
            });
            LineMenu::script_from_value(vm, value)
        });
        assert_eq!(menu.defs.len(), 4, "the section without a label is dropped");
        assert_eq!(menu.defs[0].id, LiveId::from_str("intro"), "the id is the target's last segment");
        assert_eq!(menu.defs[0].target, "page.intro");
        assert_eq!(menu.defs[1].id, live_id!(getting_ready));
        assert_eq!(menu.defs[2].level, 6, "a level past six is six");
        assert_eq!(menu.defs[3].level, 1, "no level is level one");
        assert_eq!(menu.scroll_view, "a");
        assert_eq!(menu.max_level_u8(), 2);
        assert_eq!(menu.rows(), vec![0, 1, 3]);

        let (labeled, mirrored) = cx.with_vm(|vm| {
            let labeled = vm.eval(script! {
                use mod.prelude.widgets.*
                LineMenuLabeled{}
            });
            let mirrored = vm.eval(script! {
                use mod.prelude.widgets.*
                LineMenuMirrored{}
            });
            (LineMenu::script_from_value(vm, labeled), LineMenu::script_from_value(vm, mirrored))
        });
        assert!(labeled.always_show_labels);
        assert!(mirrored.mirror);
        });
    }

    #[test]
    fn the_lines_and_the_names_compile() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for (name, value) in [
                ("line", crate::script_eval!(vm, {
                    mod.shader.test_compile_draw_source(mod.widgets.LineMenu.draw_line, "glsl", false)
                })),
                ("card", crate::script_eval!(vm, {
                    mod.shader.test_compile_draw_source(mod.widgets.LineMenu.draw_card, "glsl", false)
                })),
                ("label", crate::script_eval!(vm, {
                    mod.shader.test_compile_draw_source(mod.widgets.LineMenu.draw_label, "glsl", false)
                })),
            ] {
                let text = vm
                    .bx
                    .heap
                    .string_with(value, |_heap, text| text.to_string())
                    .expect("the compiler answers with source");
                assert!(!text.starts_with("ERRORS:"), "{name} did not compile: {text}");
                assert!(!text.is_empty(), "{name} compiled to nothing");
            }
        });
        });
    }

    /// The tracking rule stays a free function the tests can reach, and both
    /// presets are declared once.
    #[test]
    fn the_tracking_rule_and_the_presets_are_in_the_source() {
        crate::on_test_cx(|| {
        let source = include_str!("line_menu.rs");
        assert!(source.contains(&["pub fn ", "current_section("].concat()));
        let labeled = ["mod.widgets.LineMenuLabeled = ", "mod.widgets.LineMenu{"].concat();
        let mirrored = ["mod.widgets.LineMenuMirrored = ", "mod.widgets.LineMenu{"].concat();
        assert_eq!(source.matches(&labeled).count(), 1);
        assert_eq!(source.matches(&mirrored).count(), 1);
        });
    }

    #[test]
    fn nothing_drawn_lights_nothing() {
        crate::on_test_cx(|| {
        assert_eq!(current_section(&[], 400.0, 100.0, false), None);
        assert_eq!(current_section(&[None, None], 400.0, 100.0, true), None);
        });
    }

    #[test]
    fn before_any_heading_reaches_the_line_the_first_section_is_lit() {
        crate::on_test_cx(|| {
        assert_eq!(current_section(&[Some(150.0), Some(600.0)], 400.0, 100.0, false), Some(0));
        });
    }

    #[test]
    fn the_deepest_heading_past_the_line_is_lit() {
        crate::on_test_cx(|| {
        let tops = [Some(-500.0), Some(-120.0), Some(40.0), Some(300.0)];
        assert_eq!(current_section(&tops, 400.0, 100.0, false), Some(2));
        });
    }

    #[test]
    fn a_heading_exactly_on_the_line_counts_as_reached() {
        crate::on_test_cx(|| {
        assert_eq!(current_section(&[Some(-20.0), Some(100.0)], 400.0, 100.0, false), Some(1));
        assert_eq!(current_section(&[Some(-20.0), Some(100.5)], 400.0, 100.0, false), Some(1), "half a point of rounding");
        assert_eq!(current_section(&[Some(-20.0), Some(101.0)], 400.0, 100.0, false), Some(0));
        });
    }

    #[test]
    fn undrawn_sections_are_skipped_not_counted() {
        crate::on_test_cx(|| {
        let tops = [Some(-300.0), None, Some(-10.0), None];
        assert_eq!(current_section(&tops, 400.0, 100.0, false), Some(2));
        assert_eq!(current_section(&[None, Some(250.0)], 400.0, 100.0, false), Some(1));
        });
    }

    #[test]
    fn sections_out_of_order_are_read_by_position() {
        crate::on_test_cx(|| {
        let tops = [Some(50.0), Some(-200.0), Some(500.0)];
        assert_eq!(current_section(&tops, 400.0, 100.0, false), Some(0));
        let tops = [Some(-50.0), Some(-200.0), Some(500.0)];
        assert_eq!(current_section(&tops, 400.0, 100.0, false), Some(0), "the lower heading on the page wins");
        });
    }

    #[test]
    fn at_the_end_the_last_heading_on_screen_is_lit() {
        crate::on_test_cx(|| {
        let tops = [Some(-900.0), Some(20.0), Some(320.0)];
        assert_eq!(current_section(&tops, 400.0, 100.0, false), Some(1));
        assert_eq!(current_section(&tops, 400.0, 100.0, true), Some(2));
        });
    }

    #[test]
    fn at_the_end_a_heading_below_the_view_is_not_lit() {
        crate::on_test_cx(|| {
        let tops = [Some(-900.0), Some(20.0), Some(420.0)];
        assert_eq!(current_section(&tops, 400.0, 100.0, true), Some(1));
        });
    }

    #[test]
    fn ancestors_are_the_nearest_smaller_levels_back_to_the_top() {
        crate::on_test_cx(|| {
        let levels = [1, 2, 3, 2, 3, 1];
        assert_eq!(ancestors(&levels, 4), vec![3, 0]);
        assert_eq!(ancestors(&levels, 2), vec![1, 0]);
        assert_eq!(ancestors(&levels, 3), vec![0]);
        assert_eq!(ancestors(&levels, 5), Vec::<usize>::new());
        assert_eq!(ancestors(&levels, 0), Vec::<usize>::new());
        assert_eq!(ancestors(&levels, 9), Vec::<usize>::new());
        });
    }

    #[test]
    fn a_level_jump_still_finds_its_parent() {
        crate::on_test_cx(|| {
        assert_eq!(ancestors(&[1, 3], 1), vec![0]);
        });
    }

    #[test]
    fn a_hidden_section_lights_its_nearest_shown_parent() {
        crate::on_test_cx(|| {
        let levels = [1, 2, 3, 2, 3, 1];
        assert_eq!(shown_row(&levels, 1, 4), Some(0));
        assert_eq!(shown_row(&levels, 2, 4), Some(3));
        assert_eq!(shown_row(&levels, 3, 4), Some(4));
        assert_eq!(shown_row(&levels, 1, 5), Some(5));
        });
    }

    #[test]
    fn a_first_section_deeper_than_max_level_has_no_row() {
        crate::on_test_cx(|| {
        assert_eq!(shown_row(&[3, 1, 2], 2, 0), None);
        });
    }

    #[test]
    fn line_length_shortens_by_level_and_stops_at_the_minimum() {
        crate::on_test_cx(|| {
        let m = metrics();
        assert_eq!(line_length(1, &m, 0.0, false), 20.0);
        assert_eq!(line_length(2, &m, 0.0, false), 15.0);
        assert_eq!(line_length(3, &m, 0.0, false), 10.0);
        assert_eq!(line_length(4, &m, 0.0, false), 6.0);
        assert_eq!(line_length(6, &m, 0.0, false), 6.0);
        });
    }

    #[test]
    fn revealing_and_lighting_add_their_extras() {
        crate::on_test_cx(|| {
        let m = metrics();
        assert_eq!(line_length(2, &m, 1.0, false), 25.0);
        assert_eq!(line_length(2, &m, 0.0, true), 19.0);
        assert_eq!(line_length(2, &m, 1.0, true), 29.0);
        assert!(line_length(2, &m, 0.5, false) > 15.0 && line_length(2, &m, 0.5, false) < 25.0);
        });
    }

    #[test]
    fn a_jump_leaves_the_margin_above_the_heading() {
        crate::on_test_cx(|| {
        assert_eq!(jump_target(100.0, 300.0, 12.0, 2000.0, 400.0), 388.0);
        assert_eq!(jump_target(500.0, -200.0, 12.0, 2000.0, 400.0), 288.0, "a heading above the view scrolls back");
        });
    }

    #[test]
    fn a_jump_near_the_end_stops_at_the_end() {
        crate::on_test_cx(|| {
        assert_eq!(jump_target(1400.0, 300.0, 12.0, 2000.0, 400.0), 1600.0);
        assert_eq!(jump_target(0.0, 5.0, 12.0, 2000.0, 400.0), 0.0, "nor before the start");
        });
    }

    #[test]
    fn a_jump_in_content_that_fits_goes_nowhere() {
        crate::on_test_cx(|| {
        assert_eq!(jump_target(0.0, 200.0, 12.0, 300.0, 400.0), 0.0);
        });
    }

    #[test]
    fn the_pin_holds_within_two_points_and_lets_go_past_them() {
        crate::on_test_cx(|| {
        assert!(!pin_released(388.0, 388.0));
        assert!(!pin_released(388.0, 390.0));
        assert!(!pin_released(388.0, 386.0));
        assert!(pin_released(388.0, 390.5));
        assert!(pin_released(388.0, 385.0));
        });
    }

    /// A jump reads each theme easing straight off its clock, lands exactly
    /// on its target when the time is up, and an overshooting curve swings
    /// past the heading but never past either end of the page.
    #[test]
    fn a_jump_follows_its_curve_and_lands_on_its_target() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let tokens = [
            "motion_ease_standard",
            "motion_ease_standard_decelerate",
            "motion_ease_standard_accelerate",
            "motion_ease_emphasized_decelerate",
            "motion_ease_emphasized_accelerate",
            "motion_ease_linear",
            "motion_ease_spring",
            "motion_ease_bounce",
        ];
        let eases: Vec<(&str, Ease)> = cx.with_vm(|vm| tokens.iter().map(|token| (*token, theme_ease(vm, token))).collect());
        for (token, ease) in &eases {
            assert_eq!(jump_offset(100.0, 500.0, 0.0, *ease, 2000.0), 100.0 + 400.0 * ease.map(0.0), "{token} at the start");
            for t in [0.25, 0.5, 0.75] {
                let want = (100.0 + 400.0 * ease.map(t)).clamp(0.0, 2000.0);
                assert!((jump_offset(100.0, 500.0, t, *ease, 2000.0) - want).abs() < 1e-9, "{token} at {t}");
            }
            assert_eq!(jump_offset(100.0, 500.0, 1.0, *ease, 2000.0), 500.0, "{token} lands on its target");
            assert_eq!(jump_offset(100.0, 500.0, 1.7, *ease, 2000.0), 500.0, "{token} stays there");
        }
        let spring = eases.iter().find(|(token, _)| *token == "motion_ease_spring").unwrap().1;
        let peak = (1..100).map(|i| i as f64 / 100.0).max_by(|a, b| spring.map(*a).total_cmp(&spring.map(*b))).unwrap();
        assert!(jump_offset(0.0, 1000.0, peak, spring, 5000.0) > 1000.0, "a spring swings past the heading");
        assert_eq!(jump_offset(0.0, 1000.0, peak, spring, 1000.0), 1000.0, "but not past the end of the page");
        assert_eq!(jump_offset(1000.0, 0.0, peak, spring, 1000.0), 0.0, "nor past its start");
        });
    }

    /// The jump runs along the menu's own `jump_ease`, taken when it
    /// starts: a quarter of the way through its time the view is where that
    /// curve says, not where a fixed one would put it.
    #[test]
    fn the_jump_scrolls_the_view_along_the_menus_curve() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = followed_article();
        let decelerate = cx.with_vm(|vm| theme_ease(vm, "motion_ease_emphasized_decelerate"));
        let toc = root.widget(&cx, ids!(toc));
        {
            let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
            menu.jump_ease = decelerate;
            menu.jump_secs = 1000.0;
            menu.watch_frame(&mut cx, 1.0);
            assert!(menu.is_linked());
            menu.start_jump(&mut cx, 1, true);
            let jump = menu.jump.as_mut().expect("a jump is under way");
            assert_eq!(jump.ease, decelerate, "the jump took the menu's curve");
            assert_eq!(jump.to, 488.0, "the second block's top less the margin");
            // A quarter of the way through a long jump, on the real clock.
            jump.start -= 250.0;
            menu.watch_frame(&mut cx, 1.1);
        }
        target.draw(&mut cx, &root);
        let pos = root.widget(&cx, ids!(article)).borrow::<ScrollShadowView>().unwrap().scroll_extent().pos.y;
        let want = 488.0 * decelerate.map(0.25);
        assert!((pos - want).abs() < 0.5, "the view is at {pos}, the curve says {want}");
        assert!((pos - 488.0 * 0.0625).abs() > 50.0, "and not where the old fixed curve put it");
        });
    }

    fn stack() -> Rect {
        Rect { pos: dvec2(100.0, 100.0), size: dvec2(42.0, 224.0) }
    }

    #[test]
    fn the_card_opens_after_the_lines_when_there_is_room() {
        crate::on_test_cx(|| {
        let bounds = Rect { pos: dvec2(6.0, 6.0), size: dvec2(1000.0, 800.0) };
        let layout = card_layout(stack(), 104.0, 138.0, 150.0, &card_metrics(), bounds, false);
        assert!(!layout.labels_before);
        assert_eq!(layout.label_x, 148.0);
        assert_eq!(layout.label_w, 150.0);
        assert_eq!(layout.card, Rect { pos: dvec2(100.0, 92.0), size: dvec2(148.0 + 150.0 + 8.0 - 100.0, 240.0) });
        });
    }

    #[test]
    fn the_card_takes_the_other_side_near_the_window_edge() {
        crate::on_test_cx(|| {
        let stack = Rect { pos: dvec2(900.0, 100.0), size: dvec2(42.0, 224.0) };
        let bounds = Rect { pos: dvec2(6.0, 6.0), size: dvec2(1000.0, 800.0) };
        let layout = card_layout(stack, 904.0, 938.0, 150.0, &card_metrics(), bounds, false);
        assert!(layout.labels_before);
        assert_eq!(layout.label_x, 904.0 - 10.0 - 150.0);
        assert_eq!(layout.card.pos.x + layout.card.size.x, 942.0, "the card still ends at the stack");
        });
    }

    #[test]
    fn a_mirrored_card_opens_before_the_lines() {
        crate::on_test_cx(|| {
        let stack = Rect { pos: dvec2(600.0, 100.0), size: dvec2(42.0, 224.0) };
        let bounds = Rect { pos: dvec2(6.0, 6.0), size: dvec2(1000.0, 800.0) };
        let layout = card_layout(stack, 604.0, 638.0, 150.0, &card_metrics(), bounds, true);
        assert!(layout.labels_before);
        assert_eq!(layout.label_x, 604.0 - 10.0 - 150.0);
        assert_eq!(layout.card.pos.x, 604.0 - 10.0 - 150.0 - 8.0);
        let unbounded = card_layout(stack, 604.0, 638.0, 150.0, &card_metrics(), Rect::default(), true);
        assert_eq!(unbounded, layout, "no bounds is no flip");
        });
    }

    #[test]
    fn with_room_on_neither_side_the_names_shrink_to_the_roomier_one() {
        crate::on_test_cx(|| {
        // 100 points either side is not room for 220 of names plus gap and pad.
        let stack = Rect { pos: dvec2(106.0, 100.0), size: dvec2(42.0, 224.0) };
        let bounds = Rect { pos: dvec2(6.0, 6.0), size: dvec2(290.0, 800.0) };
        let layout = card_layout(stack, 110.0, 144.0, 400.0, &card_metrics(), bounds, false);
        assert!(!layout.labels_before, "after the lines is the roomier side");
        let room_after = 296.0 - (144.0 + 10.0 + 8.0);
        assert_eq!(layout.label_w, room_after);
        assert!(layout.card.pos.x + layout.card.size.x <= 296.0 + 1e-9);
        // 26 points before and 28 after: the names keep their floor, and the
        // card is pulled back inside rather than running off.
        let narrow = Rect { pos: dvec2(46.0, 100.0), size: dvec2(42.0, 224.0) };
        let tight = Rect { pos: dvec2(6.0, 6.0), size: dvec2(124.0, 800.0) };
        let floored = card_layout(narrow, 50.0, 84.0, 400.0, &card_metrics(), tight, false);
        assert!(!floored.labels_before);
        assert_eq!(floored.label_w, 40.0, "never narrower than the floor");
        assert!(floored.card.pos.x + floored.card.size.x <= 130.0 + 1e-9);
        });
    }

    #[test]
    fn names_show_only_after_the_reveal_delay_and_hide_after_the_grace() {
        crate::on_test_cx(|| {
        let mut reveal = Reveal::default();
        assert_eq!(reveal.pointer(true, 0.0, 0.06, 0.2), None);
        assert_eq!(reveal.tick(0.05), None);
        assert_eq!(reveal.pointer(true, 0.04, 0.06, 0.2), None);
        assert_eq!(reveal.deadline(), Some(0.06), "moving on the stack does not push the deadline back");
        assert_eq!(reveal.tick(0.06), Some(true));
        assert_eq!(reveal.pointer(false, 1.0, 0.06, 0.2), None);
        assert_eq!(reveal.pointer(true, 1.1, 0.06, 0.2), None);
        assert_eq!(reveal.tick(1.3), None, "coming back inside the grace keeps the names");
        assert_eq!(reveal.pointer(false, 2.0, 0.06, 0.2), None);
        assert_eq!(reveal.tick(2.19), None);
        assert_eq!(reveal.tick(2.2), Some(false));
        let mut instant = Reveal::default();
        assert_eq!(instant.pointer(true, 0.0, 0.0, 0.0), Some(true));
        });
    }

    #[test]
    fn focus_shows_at_once_and_does_not_hide_under_the_pointer() {
        crate::on_test_cx(|| {
        let mut reveal = Reveal::default();
        reveal.pointer(true, 0.0, 0.06, 0.2);
        assert_eq!(reveal.focus(true, true), Some(true));
        assert_eq!(reveal.pending, None);
        assert_eq!(reveal.focus(false, true), None, "the pointer is still over the names");
        assert!(reveal.shown);
        assert_eq!(reveal.focus(false, false), Some(false));
        assert_eq!(reveal.touch_toggle(), Some(true));
        assert_eq!(reveal.touch_toggle(), Some(false));
        });
    }

    #[test]
    fn a_section_id_comes_from_the_target_unless_one_is_written() {
        crate::on_test_cx(|| {
        assert_eq!(section_id("article.body.intro", None), LiveId::from_str("intro"));
        assert_eq!(section_id("intro", Some(live_id!(start))), live_id!(start));
        assert_eq!(LineSection::new("a.b", "B", 9).level, 6);
        assert_eq!(LineSection::new("a.b", "B", 2).with_id(live_id!(x)).id, live_id!(x));
        });
    }

    /// Hosted here because it is the contract the menu depends on: a view
    /// with scroll bars can say where it is, and a plain view has nothing to
    /// say.
    #[test]
    fn a_view_with_scroll_bars_reports_an_extent_and_a_plain_view_does_not() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let (scrolling, plain) = cx.with_vm(|vm| {
            let scrolling = vm.eval(script! {
                use mod.prelude.widgets.*
                ScrollYView{}
            });
            let plain = vm.eval(script! {
                use mod.prelude.widgets.*
                View{}
            });
            (View::script_from_value(vm, scrolling), View::script_from_value(vm, plain))
        });
        assert!(scrolling.scroll_extent().is_some());
        assert!(plain.scroll_extent().is_none());
        });
    }

    /// A scrolling view is its own box. A shadow view answers with all of
    /// its content moved up by the offset, so its box is put back together
    /// from the extent, and an axis that does not scroll keeps the area's
    /// own size.
    #[test]
    fn a_shadow_views_box_is_rebuilt_from_its_extent() {
        crate::on_test_cx(|| {
        let extent = ScrollExtent { pos: dvec2(0.0, 250.0), total: dvec2(0.0, 1200.0), visible: dvec2(0.0, 300.0) };
        let content = Rect { pos: dvec2(40.0, 60.0 - 250.0), size: dvec2(500.0, 1200.0) };
        assert_eq!(shown_box(LinkedKind::View, content, extent), content);
        let shown = shown_box(LinkedKind::Shadow, content, extent);
        assert_eq!(shown, Rect { pos: dvec2(40.0, 60.0), size: dvec2(500.0, 300.0) });
        // A heading 400 into the content sits 150 below the box's top here.
        assert_eq!(content.pos.y + 400.0 - shown.pos.y, 150.0);
        });
    }

    /// One frame of `root` into a window-less pass, with the overlay a
    /// window gives the tree to hang its own lists off.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            let size = dvec2(800.0, 400.0);
            self.pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = crate::makepad_draw::cx_draw::CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn moved(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: std::cell::Cell::new(Area::Empty),
        })
    }

    /// A menu of two sections following a 300-point shadow view whose two
    /// 500-point blocks are the sections' targets, drawn once.
    fn followed_article() -> (crate::PooledCx, WidgetRef, Target) {
        let mut cx = test_cx();
        let root = article_root(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        (cx, root, target)
    }

    /// The tree `followed_article` draws, built into a `Cx` already loaded.
    fn article_root(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 400
                    flow: Right
                    toc := LineMenu{
                        scroll_view: "article"
                        sections: [
                            {target: "first" label: "First" level: 1}
                            {target: "second" label: "Second" level: 1}
                        ]
                    }
                    article := ScrollShadowView{
                        width: Fill
                        height: 300
                        first := View{width: Fill height: 500}
                        second := View{width: Fill height: 500}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// Show the card as the pointer would once the reveal delay is over.
    fn reveal_by_pointer(cx: &mut Cx, root: &WidgetRef) {
        let toc = root.widget(cx, ids!(toc));
        let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
        menu.reveal_state.shown = true;
        menu.apply_reveal(cx, Some(true), RevealedBy::Pointer);
        assert!(menu.locked, "the card takes the pointer");
    }

    /// A menu dropped with its card out, as when the page holding it is
    /// swapped for another while the pointer rests on the stack, cannot let
    /// go of the pointer itself: a drop has no `Cx`. A lock nobody holds
    /// turns every press in the window away, so the menu leaves it for the
    /// next event to release, and a menu taking a lock of its own in a tree
    /// with no window above it releases what was left first.
    #[test]
    fn a_menu_dropped_with_its_card_out_leaves_its_lock_for_the_next_event() {
        crate::on_test_cx(|| {
        let (mut cx, root, _target) = followed_article();
        reveal_by_pointer(&mut cx, &root);
        assert!(cx.sweep_lock_area().is_some(), "the card holds the pointer");
        drop(root);
        assert!(cx.sweep_lock_area().is_some(), "nothing could let go of it on drop");
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the next event does");

        let dropped = article_root(&mut cx);
        let survivor = article_root(&mut cx);
        let (mut dropped_target, mut survivor_target) = (Target::new(&mut cx), Target::new(&mut cx));
        dropped_target.draw(&mut cx, &dropped);
        survivor_target.draw(&mut cx, &survivor);
        reveal_by_pointer(&mut cx, &dropped);
        drop(dropped);
        reveal_by_pointer(&mut cx, &survivor);
        survivor.widget(&cx, ids!(toc)).borrow_mut::<LineMenu>().unwrap().hide(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the other menu let go of its own lock and of the one left behind");
        });
    }

    /// The test tree is told where the rows are on screen. A parent that
    /// clips the stack cuts the rows it reports, and a row it hides entirely
    /// is not reported, so a test is never sent to press a row through what
    /// lies over the page.
    #[test]
    fn rows_are_reported_cut_to_what_a_clipping_parent_shows() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 400
                    flow: Down
                    band := View{
                        width: 200
                        height: 50
                        toc := LineMenu{
                            sections: [
                                {target: "a" label: "A" level: 1}
                                {target: "b" label: "B" level: 1}
                                {target: "c" label: "C" level: 1}
                                {target: "d" label: "D" level: 1}
                                {target: "e" label: "E" level: 1}
                                {target: "f" label: "F" level: 1}
                            ]
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let toc = root.widget(&cx, ids!(toc));
        let menu = toc.borrow::<LineMenu>().expect("toc is a line menu");
        let full = menu.draw_bg.area().rect(&cx);
        let shown = menu.draw_bg.area().clipped_rect(&cx);
        assert!(full.size.y > shown.size.y + 50.0, "the band clips the stack: {full:?} shown as {shown:?}");
        let parts = menu.snapshot_parts(&cx);
        let within = |r: Rect| {
            r.size.x > 0.0
                && r.size.y > 0.0
                && r.pos.x >= shown.pos.x - 1e-9
                && r.pos.y >= shown.pos.y - 1e-9
                && r.pos.x + r.size.x <= shown.pos.x + shown.size.x + 1e-9
                && r.pos.y + r.size.y <= shown.pos.y + shown.size.y + 1e-9
        };
        for part in &parts {
            assert!(within(part.rect), "{:?} is reported at {:?}, outside {shown:?}", part.text, part.rect);
        }
        // Rows at 4..22, 22..40 and 40..58 meet the band's 50 points; the rest do not.
        let names: Vec<&str> = parts.iter().map(|part| part.text.as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
        assert_eq!(parts[2].rect.size.y, shown.pos.y + shown.size.y - parts[2].rect.pos.y, "the last row cut at the band's edge");
        });
    }

    /// Following a shadow view, a heading's top moves with the scroll, and
    /// a section made current before any watch frame has looked for the
    /// view is held against where the view is, not against the top.
    #[test]
    fn a_followed_shadow_view_moves_its_headings_and_holds_a_pin_where_it_is() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = followed_article();
        root.widget(&cx, ids!(article))
            .borrow_mut::<ScrollShadowView>()
            .expect("article is a shadow view")
            .set_scroll(&mut cx, dvec2(0.0, 250.0));
        target.draw(&mut cx, &root);

        let toc = root.widget(&cx, ids!(toc));
        {
            let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
            let (probe, extent) = menu.read_geometry(&cx).expect("the article is followed");
            assert_eq!(extent.pos.y, 250.0);
            assert_eq!(probe.viewport.size.y, 300.0, "the box, not the content");
            assert_eq!(menu.tops, vec![Some(-250.0), Some(250.0)], "the headings moved up with the page");
            menu.linked = None;
        }
        toc.as_line_menu().set_current(&mut cx, LiveId::from_str("second"));
        let pin = toc.borrow::<LineMenu>().unwrap().pin.expect("pinned");
        assert_eq!(pin.landed, 250.0, "held where the page is");
        });
    }

    /// With nothing moving the watch repaints nothing and stops arming
    /// itself after a few frames. A repaint on every frame would wake it
    /// for good: the repaint is an event, and every event arms the watch.
    #[test]
    fn a_still_page_lets_the_watch_go_quiet() {
        crate::on_test_cx(|| {
        let (mut cx, root, _target) = followed_article();
        let toc = root.widget(&cx, ids!(toc));
        let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
        // The first frame finds the view and lights a section.
        menu.watch_frame(&mut cx, 1.0);
        assert!(menu.is_linked());
        for frame in 0..QUIET_FRAMES {
            cx.new_draw_event = DrawEvent::default();
            menu.watch_frame(&mut cx, 1.1 + frame as f64 * 0.1);
            assert!(!cx.new_draw_event.will_redraw(), "frame {frame} repainted a page that had not moved");
        }
        assert_eq!(menu.last_time, 0.0, "the watch stopped arming itself");
        });
    }

    /// A pointer another control holds scrubs no row here. The names are out
    /// by the keyboard, which takes no pointer of its own (see
    /// `apply_reveal`), so a button can still hold a press while the card is
    /// over the page — and the raw move would otherwise drag the highlight
    /// across it under a hand that is dragging something else entirely.
    #[test]
    fn a_pointer_another_control_holds_scrubs_no_row() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 400
                    flow: Right
                    toc := LineMenu{
                        sections: [
                            {target: "a" label: "Alpha" level: 1}
                            {target: "b" label: "Beta" level: 1}
                        ]
                    }
                    held := Button{width: 200. height: 40. text: "Held"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let toc = root.widget(&cx, ids!(toc));
        {
            let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
            menu.refresh_rect(&mut cx);
            menu.reveal_state.shown = true;
            menu.apply_reveal(&mut cx, Some(true), RevealedBy::Keys);
            let _ = menu.advance_reveal(0.0);
            menu.reveal.settle();
            assert!(!menu.locked, "a keyboard reveal takes no pointer");
        }
        target.draw(&mut cx, &root);
        let row = {
            let menu = toc.borrow::<LineMenu>().unwrap();
            assert!(menu.drawn_card.is_some(), "the card is drawn");
            menu.row_rects()[1]
        };
        let over = row.pos + row.size * 0.5;

        // With nothing holding the pointer the move reads as it always did.
        root.handle_event(&mut cx, &moved(over), &mut Scope::empty());
        assert_eq!(toc.borrow::<LineMenu>().unwrap().hot_row, Some(1), "the row under the pointer lights");

        // Put out by hand, so what follows cannot pass on leftover state:
        // held still means held still, and a frozen menu keeps whatever it
        // had, so it must be given nothing.
        {
            let mut menu = toc.borrow_mut::<LineMenu>().unwrap();
            menu.hot_row = None;
            menu.pointer_over = false;
        }

        // The button takes a press and holds the mouse with it.
        let button = root.widget(&cx, ids!(held)).area();
        let on_button = button.rect(&cx);
        assert!(on_button.size.x > 0.0, "the button is drawn");
        root.handle_event(&mut cx, &press(on_button.pos + on_button.size * 0.5), &mut Scope::empty());
        assert!(cx.fingers.is_area_captured(button), "the button holds the mouse");

        root.handle_event(&mut cx, &moved(over), &mut Scope::empty());
        let menu = toc.borrow::<LineMenu>().unwrap();
        assert_eq!(menu.hot_row, None, "no row lit under a pointer the button holds");
        assert!(!menu.pointer_over, "and the menu does not count itself hovered");
        });
    }

    /// A spring swings the lines past their revealed length and back. The
    /// stack is widened once, by the curve, so the swing never leaves it;
    /// the card and the rows a pointer lands on stay where they rest, and a
    /// test is told of the card's rows only once they have arrived.
    #[test]
    fn a_spring_swings_the_lines_while_the_rows_and_the_card_stay_where_they_rest() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = followed_article();
        let spring = cx.with_vm(|vm| theme_ease(vm, "motion_ease_spring"));
        let toc = root.widget(&cx, ids!(toc));
        let calm = toc.borrow::<LineMenu>().unwrap().draw_bg.area().rect(&cx);
        {
            let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
            menu.reveal_ease = spring;
            menu.reveal_secs = 1.0;
        }
        target.draw(&mut cx, &root);
        let stack = {
            let mut menu = toc.borrow_mut::<LineMenu>().unwrap();
            menu.refresh_rect(&mut cx);
            let reach = reveal_reach(menu.reveal_ease, menu.conceal_ease);
            let widened = menu.rect.size.x - calm.size.x;
            assert!((widened - menu.hover_extra * (reach - 1.0)).abs() < 1e-6, "the stack widens by the swing, {widened}");
            menu.reveal_state.shown = true;
            assert!(menu.advance_reveal(0.0), "the reveal has started");
            assert!(menu.advance_reveal(0.15));
            assert!(menu.reveal.value() > 1.2, "the spring carries the reveal past 1: {}", menu.reveal.value());
            menu.rect
        };
        target.draw(&mut cx, &root);
        let (rows, card) = {
            let menu = toc.borrow::<LineMenu>().unwrap();
            assert!(menu.card_showing());
            let card = menu.drawn_card.expect("the card is drawn while the names come in");
            let longest = line_length(1, &menu.line_metrics(), menu.reveal.value(), true);
            assert!(longest <= menu.max_line(&menu.rows()), "the longest line mid-swing still fits the stack");
            assert!(
                menu.snapshot_parts(&cx).iter().all(|part| part.rect.pos.x == stack.pos.x),
                "rows still arriving are reported on the stack, not the card"
            );
            (menu.row_rects(), card)
        };
        toc.borrow_mut::<LineMenu>().unwrap().reveal.settle();
        target.draw(&mut cx, &root);
        let menu = toc.borrow::<LineMenu>().unwrap();
        assert!(menu.reveal.is_at(1.0));
        assert_eq!(menu.row_rects(), rows, "the rows are hit where they rest");
        assert_eq!(menu.drawn_card, Some(card), "and the card does not move with the swing");
        assert!(
            menu.snapshot_parts(&cx).iter().all(|part| part.rect.pos.x == card.card.pos.x),
            "settled, the rows are reported on the card"
        );
        });
    }

    #[test]
    fn reduced_motion_shows_and_hides_the_names_at_once_on_any_curve() {
        crate::on_test_cx(|| {
        let (mut cx, root, _target) = followed_article();
        let spring = cx.with_vm(|vm| theme_ease(vm, "motion_ease_spring"));
        let toc = root.widget(&cx, ids!(toc));
        let mut menu = toc.borrow_mut::<LineMenu>().expect("toc is a line menu");
        menu.reveal_ease = spring;
        menu.conceal_ease = spring;
        menu.reduced_motion = true;
        menu.reveal_state.shown = true;
        assert!(!menu.advance_reveal(0.0));
        assert!(menu.reveal.is_at(1.0), "shown whole on the first frame");
        menu.reveal_state.shown = false;
        assert!(!menu.advance_reveal(0.0));
        assert!(menu.reveal.is_at(0.0) && !menu.card_showing(), "and gone on the next");
        });
    }
}
