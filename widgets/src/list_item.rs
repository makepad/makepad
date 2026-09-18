//! ListItem — one row of a list: something at the leading edge, one to three
//! lines of text, and something at the trailing edge.
//!
//! A row is the piece an application writes most often and shares least. The
//! parts never differ: a slot before the text, the text, a slot after it.
//! What differs between two hand-written rows is the height, the two type
//! sizes, where the rule between rows starts, and whether the row says
//! anything when it is pressed or chosen. So those are what this owns.
//!
//! **The shape decides the height and the line count.** `lines` picks one,
//! two or three, and the row takes its height from that, so two lists in one
//! application cannot disagree by four points about how tall a two-line row
//! is. A caller that wants a different height still writes one; the shape is
//! the number nobody has to derive again.
//!
//! **The rule belongs to the row.** A rule between rows has to start where
//! the TEXT starts, not where the row starts, or it cuts under the avatars.
//! Only the row knows where its own text starts — it depends on the leading
//! slot's width, which is a caller's widget — so a separate rule between
//! rows has to be told a number, and that number is the one that goes stale
//! the day the leading slot changes size. Here it is measured every draw.
//!
//! **The gap only exists between things that are there.** The row lays its
//! three columns out with no turtle spacing and puts the gaps in itself,
//! because turtle spacing sits between every pair of walks — including an
//! empty leading slot and the text, which indents the text of every row that
//! has no leading widget by a gap the caller never asked for.
//!
//! **A press a slot answers is not a press on the row.** The slots handle
//! the event first, so a trailing switch or a leading checkbox marks the
//! press handled and the row's own hit test sees nothing. A decorative icon
//! answers nothing, so a press on it still opens the row, which is what
//! someone aiming at an avatar meant.
//!
//! **What it is not.** It is not a list: it does not scroll, does not
//! recycle and does not decide which row is chosen — `selected` is a flag
//! the list sets and the row draws. It has no keyboard of its own for the
//! reason the destination list records: one tab stop per row means Tab walks
//! every row of a long list, so the keyboard belongs to whatever owns the
//! rows. Swipe actions, drag handles and per-row menus are the host's
//! widgets, dropped into the trailing slot.

use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_tree::CxWidgetExt,
};

/// How many lines of text the row is shaped for.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum ListItemLines {
    /// A name, and something at the end of it.
    #[pick]
    #[default]
    One,
    /// A name with a line about it underneath.
    Two,
    /// A name and two lines about it.
    Three,
}

/// One shape's numbers, in layout points.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ListItemMetrics {
    /// The row's height when the caller leaves it to the shape.
    pub height: f64,
    /// How many of the three text properties are drawn.
    pub lines: usize,
}

impl ListItemLines {
    /// The ladder every caller was deriving for itself. The steps are one
    /// section gap apart at this theme's density, so a list of one-line rows
    /// and a list of two-line rows look like the same application.
    pub fn metrics(self) -> ListItemMetrics {
        match self {
            ListItemLines::One => ListItemMetrics { height: 36.0, lines: 1 },
            ListItemLines::Two => ListItemMetrics { height: 48.0, lines: 2 },
            ListItemLines::Three => ListItemMetrics { height: 60.0, lines: 3 },
        }
    }
}

/// Where the rule under a row starts and stops.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum ListItemDivider {
    /// No rule. A list that groups its rows another way wants none.
    #[pick]
    #[default]
    None,
    /// Edge to edge.
    Full,
    /// From where the text starts, so the rule does not cut under the
    /// leading slot.
    Text,
    /// Off both edges by the row's own padding.
    Inset,
}

/// What a row reports. The row states that it was pressed and changes
/// nothing outside itself: which row is chosen is the list's answer, not the
/// row's, because only the list knows how many may be chosen at once.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ListItemAction {
    /// Pressed and released over the row.
    Clicked,
    #[default]
    None,
}

/// Where the rule under a row starts and stops, in the same coordinates as
/// the row's own `left` and `right` edges. `text_x` is where the first
/// line's ink starts, and the padding is the row's own. `None` draws no rule.
pub fn divider_span(
    kind: ListItemDivider,
    left: f64,
    right: f64,
    text_x: f64,
    pad_left: f64,
    pad_right: f64,
) -> Option<(f64, f64)> {
    match kind {
        ListItemDivider::None => None,
        ListItemDivider::Full => Some((left, right)),
        // A rule that stops short of the trailing edge reads as a gap in the
        // list rather than a line under a row, so only the leading end is
        // held back to the text.
        ListItemDivider::Text => Some((text_x, right)),
        ListItemDivider::Inset => Some((left + pad_left, right - pad_right)),
    }
}

/// The top of each line's band: the lines stacked with `gap` between them
/// and the block centred in a band of `height` starting at `top`.
pub fn stack_tops(sizes: &[f64], gap: f64, top: f64, height: f64) -> Vec<f64> {
    if sizes.is_empty() {
        return Vec::new();
    }
    let total: f64 = sizes.iter().sum::<f64>() + gap * (sizes.len() - 1) as f64;
    let mut y = top + (height - total) * 0.5;
    let mut tops = Vec::with_capacity(sizes.len());
    for size in sizes {
        tops.push(y);
        y += size + gap;
    }
    tops
}

/// What a drawn line's ink sits below the top of its line box, as a
/// fraction of the font size. `draw_abs` takes the line box, so a line
/// centred in a band has to be lifted by this much or it rides low.
const INK_TOP: f64 = 0.30;

/// The character standing for what did not fit. The layouter cuts a walked
/// run with this one; a line placed by hand cuts its own, with the same one.
const ELLIPSIS: &str = "…";

/// The longest prefix of at most `count` items for which `fits` holds, where
/// `fits` is true of every shorter prefix and false of every longer one. A
/// chop rather than a walk: cutting a long line costs a handful of
/// measurements instead of one per character.
fn longest_fitting(count: usize, mut fits: impl FnMut(usize) -> bool) -> usize {
    let mut low = 0;
    let mut high = count;
    while low < high {
        let mid = (low + high + 1) / 2;
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// `text` cut to `max_w` with an ellipsis where it was cut, or empty when
/// there is no room even for the ellipsis: half a word is worse than none.
fn elide(draw_text: &DrawText, cx: &mut Cx2d, text: &str, max_w: f64) -> String {
    if text.is_empty() || max_w <= 0.0 {
        return String::new();
    }
    // The ordinary line fits, and costs one measurement.
    if measure(draw_text, cx, text) <= max_w {
        return text.to_string();
    }
    let starts: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    let keep = longest_fitting(starts.len() - 1, |count| {
        measure(draw_text, cx, &format!("{}{}", &text[..starts[count]], ELLIPSIS)) <= max_w
    });
    if keep == 0 && measure(draw_text, cx, ELLIPSIS) > max_w {
        return String::new();
    }
    format!("{}{}", &text[..starts[keep]], ELLIPSIS)
}

/// One line, cut to the width it has and placed by its ink rather than by
/// its line box.
fn draw_line(draw_text: &mut DrawText, cx: &mut Cx2d, at: Vec2d, max_w: f64, text: &str) {
    let fitted = elide(draw_text, cx, text, max_w);
    if fitted.is_empty() {
        return;
    }
    let font_size = draw_text.text_style.font_size as f64;
    draw_text.draw_abs(cx, dvec2(at.x, at.y - font_size * INK_TOP), &fitted);
}

fn dimmed(color: Vec4f, opacity: f32) -> Vec4f {
    Vec4f { w: color.w * opacity, ..color }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ListItemLines = #(ListItemLines::script_api(vm))
    mod.widgets.ListItemDivider = #(ListItemDivider::script_api(vm))

    mod.widgets.DrawListItemBase = #(DrawListItem::script_component(vm))
    set_type_default() do #(DrawListItem::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.ListItemBase = #(ListItem::register_widget(vm))

    /** One row of a list: a leading slot, one to three lines of text, and a
     * trailing slot. The shape decides the height and the line count. */
    mod.widgets.ListItem = set_type_default() do mod.widgets.ListItemBase{
        width: Fill
        // Fit, and the row takes the shape's height; a number wins over it.
        height: Fit
        // The flow and the spacing are the row's own and are overwritten
        // every draw: the three columns are laid out left to right with no
        // turtle spacing, and the gaps come from `gap`.
        flow: Right
        /** room at the leading and trailing edges */
        padding: Inset{top: 0., bottom: 0., left: theme.space_4, right: theme.space_4}
        margin: 0.

        /** the first line: the name of the thing the row stands for */
        text: ""
        /** the second line */
        secondary: ""
        /** the third line */
        tertiary: ""
        /** how many lines the row is shaped for: One, Two or Three */
        lines: mod.widgets.ListItemLines.One
        /** the rule under the row: None, Full, Text or Inset */
        divider: mod.widgets.ListItemDivider.None
        /** the rule's thickness in points 0..4 step 0.5 */
        divider_size: theme.size_divider
        /** a fixed width for the leading column, so text lines up down a
         * list of rows whose leading widgets are not all the same size; 0
         * leaves each row its own 0..96 step 1 */
        leading_width: 0.
        /** gap between the leading slot, the text and the trailing slot 0..48 step 1 */
        gap: theme.space_4
        /** gap between the lines of text 0..16 step 0.5 */
        line_gap: 2.
        /** the row is the chosen one */
        selected: false
        /** hover, press and report; false is a row that only reads out */
        interactive: true
        /** dimmed and inert */
        disabled: false
        /** ink left on a disabled row 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        /** drawn at all */
        visible: true

        /** The row's ground: nothing at all until it is chosen, hovered or
         * pressed. */
        draw_bg +: {
            // Plain, not uniform(): each of these has a field on the draw
            // struct behind it, and the animator writes two of them.
            hover: 0.0
            down: 0.0
            selected: 0.0
            disabled: 0.0

            /** the row's ground; transparent lets the list's surface through */
            color: uniform(#00000000)
            /** the ground under the chosen row */
            color_selected: uniform(theme.color_secondary_container)
            /** the ink the hover and press layers are made of */
            color_state: uniform(theme.color_on_surface)
            /** the state layer under the pointer 0..1 step 0.01 */
            hover_opacity: uniform(theme.state_hover_opacity)
            /** the state layer while pressed 0..1 step 0.01 */
            press_opacity: uniform(theme.state_press_opacity)
            /** what is left of the ground while disabled 0..1 step 0.05 */
            ground_disabled_opacity: uniform(theme.state_disabled_container_opacity)
            /** corner rounding; a row in a list is square 0..24 step 0.5 */
            radius: uniform(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // sdf.box draws twice the radius it is given, and a radius
                // past half the height is simply a pill.
                let r = min(self.radius, self.rect_size.y * 0.5)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, r * 0.5)
                let mut ground = mix(self.color, self.color_selected, self.selected)
                ground = vec4(ground.xyz, ground.a * mix(1.0, self.ground_disabled_opacity, self.disabled))
                // Hover and press are the state ink laid over the ground at
                // the theme's opacities: the same arithmetic every other
                // control uses, so a row answers with a button's weight. A
                // disabled row answers with neither.
                let layer = (self.hover * self.hover_opacity + self.down * self.press_opacity) * (1.0 - self.disabled)
                sdf.fill(mix(ground, vec4(self.color_state.xyz, 1.0), layer * self.color_state.a))
                return sdf.result
            }
        }

        /** The rule under the row. */
        draw_divider +: {
            /** the rule's colour */
            color: theme.color_outline_variant
        }

        /** The first line's ink. */
        draw_text +: {
            text_style: theme.font_body_m{
                /** first line type size 6..32 step 0.5 */
                font_size: theme.type_body_m_size
                // The lines are placed by hand from their own bands, which
                // is arithmetic that only holds while a line box is exactly
                // one font size tall.
                line_spacing: 1.0
            }
            /** first line ink */
            color: theme.color_on_surface
        }

        /** The ink of every line after the first. */
        draw_text_2 +: {
            text_style: theme.font_body_s{
                /** support line type size 6..32 step 0.5 */
                font_size: theme.type_body_s_size
                line_spacing: 1.0
            }
            /** support line ink */
            color: theme.color_on_surface_variant
        }

        // Bare slots, not named instances: a slot takes a value, so a caller
        // writes `leading: Icon{}` and never `leading := Icon{}`, which
        // makes a named child and leaves the slot empty.
        leading: View{width: Fit height: Fit}
        trailing: View{width: Fit height: Fit}

        animator: Animator{
            /** pointer state: drives the hover and press layers */
            hover: {
                default: @off
                /** pointer away: both layers fade out together */
                off: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_3}}
                    ease: theme.motion_ease_standard
                    apply: {draw_bg: {hover: 0.0, down: 0.0}}
                }
                /** pointer over: the hover layer only */
                on: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_2}}
                    ease: theme.motion_ease_standard_decelerate
                    apply: {draw_bg: {hover: 1.0, down: 0.0}}
                }
                /** pressed: the press layer, taken instantly */
                down: AnimatorState{
                    from: {all: Forward{duration: theme.motion_short_1}}
                    apply: {draw_bg: {hover: 1.0, down: 1.0}}
                }
            }
        }
    }

    /** A one-line row: a name, and something at the end of it. */
    mod.widgets.ListItemOne = mod.widgets.ListItem{
        lines: mod.widgets.ListItemLines.One
    }

    /** A two-line row: a name with a line about it underneath. */
    mod.widgets.ListItemTwo = mod.widgets.ListItem{
        lines: mod.widgets.ListItemLines.Two
    }

    /** A three-line row: a name and two lines about it. */
    mod.widgets.ListItemThree = mod.widgets.ListItem{
        lines: mod.widgets.ListItemLines.Three
    }

    /** A row in a list that draws a rule between its rows, lined up with
     * the text rather than the leading slot. */
    mod.widgets.ListItemRuled = mod.widgets.ListItem{
        divider: mod.widgets.ListItemDivider.Text
    }
}

/// The row's ground, and the hover, press, chosen and disabled states it is
/// drawn in.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawListItem {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    selected: f32,
    #[live]
    disabled: f32,
}

#[derive(Script, Widget, Animator)]
pub struct ListItem {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[live]
    pub draw_bg: DrawListItem,
    #[live]
    draw_divider: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_2: DrawText,

    /// Before the text: an icon, an avatar, a checkbox.
    #[find]
    #[live]
    pub leading: WidgetRef,
    /// After it: a value, a chevron, a control.
    #[find]
    #[live]
    pub trailing: WidgetRef,

    /// The first line: the name of the thing the row stands for.
    #[live]
    pub text: String,
    /// The second line.
    #[live]
    pub secondary: String,
    /// The third line.
    #[live]
    pub tertiary: String,
    #[live]
    pub lines: ListItemLines,
    #[live]
    pub divider: ListItemDivider,
    #[live(1.0)]
    pub divider_size: f64,
    /// A fixed width for the leading column, so the text lines up down a
    /// list whose rows carry leading widgets of different sizes; 0 leaves
    /// each row its own.
    #[live]
    pub leading_width: f64,
    /// Gap between the leading slot, the text and the trailing slot.
    #[live(12.0)]
    pub gap: f64,
    /// Gap between the lines of text.
    #[live(2.0)]
    pub line_gap: f64,
    /// The row is the chosen one. The list sets it; the row draws it.
    #[live]
    pub selected: bool,
    /// Hover, press and report. False is a row that only reads out, and a
    /// row that lights up under the pointer and then does nothing is a
    /// promise the widget cannot keep.
    #[live(true)]
    pub interactive: bool,
    /// Dimmed and inert.
    #[live]
    pub disabled: bool,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[visible]
    #[live(true)]
    visible: bool,
}

impl ScriptHook for ListItem {
    /// A `disabled: true` written in the markup sets the field directly and
    /// never reaches `set_disabled`, so without this the row would dim while
    /// the control in its trailing slot stayed bright and answering.
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        if self.disabled {
            vm.with_cx_mut(|cx| {
                for slot in [&self.leading, &self.trailing] {
                    slot.set_disabled(cx, true);
                }
            });
        }
    }
}

impl ListItem {
    /// The height and line count of the shape this row is in.
    pub fn metrics(&self) -> ListItemMetrics {
        self.lines.metrics()
    }

    pub fn set_selected(&mut self, cx: &mut Cx, selected: bool) {
        if self.selected != selected {
            self.selected = selected;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_lines(&mut self, cx: &mut Cx, lines: ListItemLines) {
        if self.lines != lines {
            self.lines = lines;
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_divider(&mut self, cx: &mut Cx, divider: ListItemDivider) {
        if self.divider != divider {
            self.divider = divider;
            self.draw_bg.redraw(cx);
        }
    }

    /// The lines this row actually draws, in order: the shape caps how many
    /// there are, and an empty property is skipped rather than left as a
    /// hole in the middle of the block.
    fn drawn_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for text in [&self.text, &self.secondary, &self.tertiary] {
            if lines.len() < self.lines.metrics().lines && !text.is_empty() {
                lines.push(text.clone());
            }
        }
        lines
    }

    /// Draw one slot inside a turtle of its own, and answer the column it
    /// took. The turtle is the row's full height with the slot centred in
    /// it, so the ROW's align pass has nothing left to move — which is what
    /// makes every rect measured during this draw the rect that is drawn.
    fn draw_slot(&mut self, cx: &mut Cx2d, scope: &mut Scope, name: LiveId) -> Rect {
        let leading = name == live_id!(leading);
        let slot = if leading { self.leading.clone() } else { self.trailing.clone() };
        let width = match (leading, self.leading_width > 0.0) {
            (true, true) => Size::Fixed(self.leading_width),
            _ => Size::fit(),
        };
        // The slots are drawn here rather than by a container, so nothing
        // else puts them in the tree: without this a host cannot reach
        // ids!(row.trailing) at all.
        cx.widget_tree_insert_child(self.uid, name, slot.clone());
        cx.begin_turtle(
            Walk { width, height: Size::fill(), ..Walk::default() },
            Layout {
                flow: Flow::right(),
                align: Align { x: 0.5, y: 0.5 },
                ..Layout::default()
            },
        );
        let slot_walk = slot.walk(cx.cx.cx);
        let _ = slot.draw_walk(cx, scope, slot_walk);
        cx.end_turtle()
    }
}

impl Widget for ListItem {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let metrics = self.metrics();
        let mut walk = walk;
        // A row puts things at both ends of a width it is given, so a Fit
        // width is a row with no ends: it would be laid out and never
        // painted. Taken as Fill instead of drawn as nothing.
        if walk.width.is_fit() {
            walk.width = Size::fill();
        }
        if walk.height.is_fit() {
            walk.height = Size::Fixed(metrics.height);
        }
        self.draw_bg.selected = if self.selected { 1.0 } else { 0.0 };
        self.draw_bg.disabled = if self.disabled { 1.0 } else { 0.0 };

        let mut layout = self.layout;
        // The three columns are the row, so their direction is not a
        // caller's choice: the middle one is a deferred fill, which only
        // resolves in a row flow. The spacing is zero for the same reason
        // the gaps are put in by hand — see the module note.
        layout.flow = Flow::right();
        layout.spacing = 0.0;
        // Nor is the row's own align: every column below is drawn at the
        // row's full height and measured while the turtle is still open, so
        // an align that moved them afterwards would move the text and the
        // rule away from the columns they were measured against.
        layout.align = Align::default();
        self.draw_bg.begin(cx, walk, layout);

        let lead = self.draw_slot(cx, scope, live_id!(leading));
        // The text column is deferred: drawn now it would take the whole
        // row and leave the trailing slot nowhere to be.
        let text_walk = Walk { width: Size::fill(), height: Size::fill(), ..Walk::default() };
        let mut deferred = cx.defer_walk_turtle(text_walk);
        let mut text_rect = if deferred.is_some() {
            None
        } else {
            Some(cx.walk_turtle(text_walk))
        };
        let trail = self.draw_slot(cx, scope, live_id!(trailing));
        if let Some(deferred) = deferred.as_mut() {
            let resolved = deferred.resolve(cx);
            text_rect = Some(cx.walk_turtle(resolved));
        }
        let text_rect = text_rect.unwrap_or_default();

        // A gap only exists between two things that are there: an empty
        // leading slot must not indent the text.
        let lead_gap = if lead.size.x > 0.0 { self.gap } else { 0.0 };
        let trail_gap = if trail.size.x > 0.0 { self.gap } else { 0.0 };
        let text_x = text_rect.pos.x + lead_gap;
        let text_w = (text_rect.size.x - lead_gap - trail_gap).max(0.0);

        let lines = self.drawn_lines();
        let first_size = self.draw_text.text_style.font_size as f64;
        let rest_size = self.draw_text_2.text_style.font_size as f64;
        let sizes: Vec<f64> = (0..lines.len())
            .map(|index| if index == 0 { first_size } else { rest_size })
            .collect();
        let tops = stack_tops(&sizes, self.line_gap, text_rect.pos.y, text_rect.size.y);
        let dim = self.disabled.then_some(self.disabled_opacity);
        for (index, (text, top)) in lines.iter().zip(tops.iter()).enumerate() {
            // The FIRST line drawn takes the first style, whichever property
            // it came from: a row whose name is empty still reads as a row
            // with a name rather than as two support lines.
            let layer = if index == 0 { &mut self.draw_text } else { &mut self.draw_text_2 };
            let ink = layer.color;
            if let Some(opacity) = dim {
                layer.color = dimmed(ink, opacity);
            }
            draw_line(layer, cx, dvec2(text_x, *top), text_w, text);
            layer.color = ink;
        }

        if self.divider_size > 0.0 {
            // The row's own edges, worked back from the column that resolved
            // last: the leading and trailing columns are measured before the
            // row's turtle has finished, so their POSITIONS are not final
            // yet but their widths are.
            let pad = self.layout.padding;
            let left = text_rect.pos.x - lead.size.x - pad.left;
            let right = text_rect.pos.x + text_rect.size.x + trail.size.x + pad.right;
            if let Some((x0, x1)) =
                divider_span(self.divider, left, right, text_x, pad.left, pad.right)
            {
                let bottom = text_rect.pos.y + text_rect.size.y + pad.bottom;
                self.draw_divider.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x0, bottom - self.divider_size),
                        size: dvec2((x1 - x0).max(0.0), self.divider_size),
                    },
                );
            }
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The slots go first, and that is what keeps a trailing switch from
        // also opening the row: a control that answers the press marks the
        // event handled, and the row's own hit test then sees nothing. A
        // decorative icon answers nothing, so the press falls through to the
        // row, which is what someone aiming at an avatar meant.
        for slot in [&self.leading, &self.trailing] {
            slot.handle_event(cx, event, scope);
        }
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        if !self.interactive || self.disabled {
            return;
        }
        let uid = self.widget_uid();
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Default);
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.animator_play(cx, ids!(hover.down));
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if fe.is_over {
                    self.animator_play(cx, ids!(hover.on));
                    if fe.was_tap() {
                        cx.widget_action(uid, ListItemAction::Clicked);
                    }
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => {}
        }
    }

    /// The row's first line, so a test reads a row the way a person does.
    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    /// One `disabled` reaches both slots, rather than each caller
    /// remembering to set three things.
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            for slot in [&self.leading, &self.trailing] {
                slot.set_disabled(cx, disabled);
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }
}

impl ListItemRef {
    /// Pressed and released over the row this pass.
    pub fn clicked(&self, actions: &Actions) -> bool {
        if let Some(action) = actions.find_widget_action(self.widget_uid()) {
            return matches!(action.cast::<ListItemAction>(), ListItemAction::Clicked);
        }
        false
    }

    pub fn selected(&self) -> bool {
        self.borrow().map(|inner| inner.selected).unwrap_or(false)
    }

    pub fn set_selected(&self, cx: &mut Cx, selected: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, selected);
        }
    }

    pub fn set_lines(&self, cx: &mut Cx, lines: ListItemLines) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_lines(cx, lines);
        }
    }

    pub fn set_divider(&self, cx: &mut Cx, divider: ListItemDivider) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_divider(cx, divider);
        }
    }

    /// The second and third lines, for a host that fills a row it did not
    /// declare the text of.
    pub fn set_secondary(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.secondary != text {
                inner.secondary = text.to_string();
                inner.draw_bg.redraw(cx);
            }
        }
    }

    pub fn set_tertiary(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.tertiary != text {
                inner.tertiary = text.to_string();
                inner.draw_bg.redraw(cx);
            }
        }
    }

    /// The widget in a slot, for a caller that needs the real thing.
    pub fn leading(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.leading.clone()).unwrap_or_default()
    }

    pub fn trailing(&self) -> WidgetRef {
        self.borrow().map(|inner| inner.trailing.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block of lines sits in the middle of the row's height whatever
    /// the count, and the gap only ever falls BETWEEN two lines. Getting
    /// that wrong is the defect a hand-written three-line row has: the text
    /// rides high and the row looks bottom-heavy.
    #[test]
    fn a_block_of_lines_is_centred_in_the_row() {
        let one = stack_tops(&[10.0], 2.0, 0.0, 36.0);
        assert_eq!(one, vec![13.0], "one line: as much above as below");

        let three = stack_tops(&[10.0, 9.0, 9.0], 2.0, 0.0, 60.0);
        let top = three[0];
        let bottom = three[2] + 9.0;
        assert!((top - (60.0 - bottom)).abs() < 1e-9, "as much above the block as below it");
        assert_eq!(three[1] - (three[0] + 10.0), 2.0, "one gap between the first two");
        assert_eq!(three[2] - (three[1] + 9.0), 2.0, "one gap between the last two");

        assert!(stack_tops(&[], 2.0, 0.0, 36.0).is_empty(), "no lines, no bands");
    }

    /// A taller shape carries more lines. The point of the ladder is that
    /// nobody derives these numbers again, so they must move together.
    #[test]
    fn the_shapes_grow_with_their_lines() {
        let ladder = [ListItemLines::One, ListItemLines::Two, ListItemLines::Three];
        let mut last = ListItemMetrics { height: 0.0, lines: 0 };
        for shape in ladder {
            let now = shape.metrics();
            assert!(now.height > last.height, "{shape:?} is taller than the shape below it");
            assert!(now.lines > last.lines, "{shape:?} carries more lines than the shape below it");
            last = now;
        }
        assert_eq!(ListItemLines::default().metrics().lines, 1, "a row is one line unless it says otherwise");
    }

    /// The rule under a row starts where the TEXT starts, not where the row
    /// starts: a rule that cuts under the avatars is the mark of a list
    /// whose separator was written as a sibling widget and told a number.
    #[test]
    fn the_rule_starts_where_the_text_starts() {
        let (left, right, text_x, pad) = (0.0, 300.0, 60.0, 12.0);
        assert_eq!(divider_span(ListItemDivider::None, left, right, text_x, pad, pad), None);
        assert_eq!(
            divider_span(ListItemDivider::Full, left, right, text_x, pad, pad),
            Some((0.0, 300.0)),
            "edge to edge"
        );
        assert_eq!(
            divider_span(ListItemDivider::Text, left, right, text_x, pad, pad),
            Some((60.0, 300.0)),
            "held back to the text, and out to the trailing edge"
        );
        assert_eq!(
            divider_span(ListItemDivider::Inset, left, right, text_x, pad, pad),
            Some((12.0, 288.0)),
            "off both edges by the row's own padding"
        );
    }

    /// The chop finds the last prefix that fits, and answers 0 rather than
    /// running off either end when nothing fits or there is nothing to cut.
    #[test]
    fn the_longest_fitting_prefix_is_found() {
        assert_eq!(longest_fitting(20, |count| count <= 7), 7);
        assert_eq!(longest_fitting(20, |count| count == 0), 0, "nothing but the ellipsis fits");
        assert_eq!(longest_fitting(20, |_| true), 20, "everything offered fits");
        assert_eq!(longest_fitting(0, |_| true), 0, "nothing offered");

        // Every prefix the chop lands on is one the caller said fits.
        let mut asked = Vec::new();
        let found = longest_fitting(31, |count| {
            asked.push(count);
            count <= 13
        });
        assert_eq!(found, 13);
        assert!(asked.iter().all(|count| *count <= 31), "never measured past the end");
    }
}
