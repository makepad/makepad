use crate::{
    makepad_derive_widget::*,
    makepad_draw::shader::draw_text::TextOverflow,
    makepad_draw::text::{
        layouter::{LaidoutText, SelectionRect},
        selection::{Cursor, Selection},
    },
    makepad_draw::*,
    widget::*,
    widget_async::ScriptAsyncResult,
};
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LabelBase = #(Label::register_widget(vm))
    mod.widgets.Label = set_type_default() do mod.widgets.LabelBase{
        width: Fit
        height: Fit
        padding: theme.mspace_1

        // The compact face this wears when its row runs out of width. Declared
        // with `:=` so it lands in the vec: a widget proto is frozen VALIDATED,
        // so a key its props do not list is a hard error at construction -- but
        // the checked path looks in the vec first, and declaring it once here
        // makes `tight: {...}` legal on every instance and every preset below.
        /** the face this wears when its row runs out of width */
        tight := {}

        draw_text +: {
            // A label is a box with text in it, and the boxes apps put labels
            // in are centered by their align, not by their baselines: center
            // the ink, so a label reads as centered when its parent says it is.
            ink_centered: true

            color_dither: uniform(1.0)
            color: theme.color_label_outer
            color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            gradient_fill_horizontal: uniform(0.0)

            get_color: fn() {
                let dither = Math.random_2d(self.pos.xy) * 0.04 * self.color_dither
                let mut color_2 = self.color_2

                let mut gradient_fill_dir = self.pos.y + dither
                if (self.gradient_fill_horizontal > 0.5) {
                    gradient_fill_dir = self.pos.x + dither
                }

                if (self.color_2.x < -0.5) {
                    color_2 = self.color
                }

                return mix(self.color, color_2, gradient_fill_dir)
            }
            text_style: theme.font_regular{
                line_spacing: theme.font_wdgt_line_spacing
            }
        }
        /** the wash under selected text (a list or row that selects across its labels sets it) */
        draw_selection +: {
            // Its own draw-call layer, opened before the text (see draw_walk).
            draw_call_group: @selection
            color: theme.color_selection_focus
        }
        /** the line under a hovered link span (see `LabelRef::set_links`) */
        draw_link +: {
            color: theme.color_label_inner_hover
        }
        /** the ink of link spans */
        link_color: theme.color_label_inner_hover
    }

    mod.widgets.Labelbold = mod.widgets.Label{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
        }
    }

    mod.widgets.LabelGradientX = mod.widgets.Label{
        width: Fit
        height: Fit
        draw_text +: {
            color: #f00
            color_2: #ff0
            gradient_fill_horizontal: 1.0
        }
    }

    mod.widgets.LabelGradientY = mod.widgets.Label{
        draw_text +: {
            color: #f00
            color_2: #ff0
        }
    }

    mod.widgets.TextBox = mod.widgets.Label{
        width: Fill
        height: Fit
        padding: Inset{left: 0., right: 0., top: theme.space_1, bottom: 0.}
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{
                line_spacing: theme.font_longform_line_spacing
                font_size: theme.font_size_p
            }
        }
        text: "TextBox"
    }

    mod.widgets.H1 = mod.widgets.Label{
        width: Fill
        padding: 0.
        draw_text +: {
            color: theme.color_text_hl
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_1
            }
        }
        text: "H1"
    }

    mod.widgets.H1italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_1
            }
        }
        text: "H1 italic"
    }

    mod.widgets.H2 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_2
            }
        }
        text: "H2"
    }

    mod.widgets.H2italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_2
            }
        }
        text: "H2 italic"
    }

    mod.widgets.H3 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_3
            }
        }
        text: "H3"
    }

    mod.widgets.H3italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_3
            }
        }
        text: "H3 italic"
    }

    mod.widgets.H4 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_4
            }
        }
        text: "H4"
    }

    mod.widgets.H4italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_4
            }
        }
        text: "H4 italic"
    }

    mod.widgets.H5 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_5
            }
        }
        text: "H5"
    }

    mod.widgets.H5italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_5
            }
        }
        text: "H5 italic"
    }

    mod.widgets.H6 = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_6
            }
        }
        text: "H6"
    }

    mod.widgets.H6italic = mod.widgets.H1{
        draw_text +: {
            text_style: theme.font_bold_italic{
                line_spacing: theme.font_hl_line_spacing
                font_size: theme.font_size_6
            }
        }
        text: "H6 italic"
    }

    mod.widgets.P = mod.widgets.TextBox{
        text: "Paragraph"
    }

    mod.widgets.Pbold = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_bold{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.Pitalic = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_italic{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.Pbolditalic = mod.widgets.TextBox{
        draw_text +: {
            text_style: theme.font_bold_italic{
                font_size: theme.font_size_p
            }
        }
        text: "Paragraph"
    }

    mod.widgets.IconSet = mod.widgets.Label{
        width: Fit
        draw_text +: {
            // An icon font's cap height describes a capital nobody is drawing:
            // these glyphs are pictures placed in their own box, so leave them
            // on the baseline the font asks for.
            ink_centered: false
            text_style: theme.font_icons{
                line_spacing: theme.font_wdgt_line_spacing
                font_size: 100.
            }
            color: theme.color_text
        }
        text: "Car"
    }
}

#[derive(Clone, Debug, Default)]
pub enum LabelAction {
    HoverIn(Rect),
    HoverOut,
    /// An accessibility press activated this span index from `set_links`.
    LinkPressed(usize),
    #[default]
    None,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct Label {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub draw_text: DrawText,
    /// The selection wash, drawn over the text where a host (a selectable
    /// list, a row) selected part of it.
    #[live]
    draw_selection: DrawColor,
    /// The underline of the hovered link span.
    #[live]
    draw_link: DrawColor,
    /// The ink of link spans.
    #[live]
    link_color: Vec4f,

    #[walk]
    pub walk: Walk,
    #[live]
    pub align: Align,
    #[live(Flow::right_wrap())]
    flow: Flow,
    #[live]
    padding: Inset,

    /// Maximum number of lines to display. 0 means unlimited (default).
    /// Combined with `text_overflow: Ellipsis`, truncated text shows "…".
    #[live(0usize)]
    pub max_lines: usize,
    /// Controls how text overflow is handled when text exceeds the container.
    #[live]
    pub text_overflow: TextOverflow,

    #[rust]
    area: Area,
    #[live]
    text: ArcStringMut,

    #[live(true)]
    visible: bool,

    /// Report where the pointer is over this label, for a tooltip to hang
    /// off. Off by default: it is one more widget asking about every mouse
    /// move, and most labels have nothing to say about it.
    ///
    /// It does NOT consume presses. A label reports the pointer; it never
    /// owns it. See `handle_event`.
    #[live(false)]
    hover_actions_enabled: bool,

    /// Whether a host may select this label's text (a list selecting across
    /// rows). Off for marks and icons whose text is not words.
    #[live(true)]
    selectable: bool,

    /// Where the text was drawn and how it was laid out, for mapping points
    /// to text and back.
    #[rust]
    laid: Option<(Rect, Rc<LaidoutText>)>,
    /// Where the label's area was when it was drawn (see `shift`).
    #[rust]
    drawn_pos: DVec2,
    /// Selected byte range of `text`, set by a host.
    #[rust]
    selection: Option<(usize, usize)>,
    /// Byte ranges of `text` drawn as links (in `link_color`), set by a host
    /// that also does the hit-testing (`selection_point_to_char_index`).
    #[rust]
    links: Vec<(usize, usize)>,
    /// The link span under the pointer: underlined.
    #[rust]
    link_hover: Option<usize>,
    /// Byte ranges of `text` drawn in their own colour, set by a host: (start, end, colour).
    #[rust]
    inks: Vec<(usize, usize, Vec4f)>,
}

impl WidgetNode for Label {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn visible(&self) -> bool {
        self.visible
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.redraw(cx);
        }
    }
    fn area(&self) -> Area {
        self.area
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_text.redraw(cx);
    }

    // Selection: a label's text is selectable by its host, in bytes of the
    // text. An empty label (drawn as one space) has none.
    fn selection_text_len(&self) -> usize {
        if self.visible && self.selectable && !self.text.as_ref().trim().is_empty() {
            self.text.as_ref().len()
        } else {
            0
        }
    }
    fn selection_point_to_char_index(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        if self.selection_text_len() == 0 {
            return None;
        }
        let (rect, laid) = self.laid.as_ref()?;
        // The text was laid out where the label was drawn; a parent's
        // alignment (a Fill sibling, a right-aligned row) or a list scrolled
        // without drawing it again moved it since. Map the point back.
        let abs = abs - self.shift(cx);
        if !rect.contains(abs) {
            return None;
        }
        let scale = self.draw_text.font_scale.max(0.0001) as f64;
        let shift = if self.draw_text.ink_centered { laid.ink_center_offset_in_lpxs() as f64 } else { 0.0 };
        let local = (abs - rect.pos) / scale - dvec2(0.0, shift);
        let cursor = laid.point_in_lpxs_to_cursor(makepad_draw::text::geom::Point::new(local.x as f32, local.y as f32));
        Some(cursor.index.min(self.text.as_ref().len()))
    }
    fn selection_set(&mut self, anchor: usize, cursor: usize) {
        let len = self.selection_text_len();
        let (a, b) = (anchor.min(cursor).min(len), anchor.max(cursor).min(len));
        let next = (a < b).then_some((a, b));
        // The host redraws the item after changing its selection.
        self.selection = next;
    }
    fn selection_clear(&mut self) {
        self.selection = None;
    }
    fn selection_select_all(&mut self) {
        let len = self.selection_text_len();
        self.selection_set(0, len);
    }
    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        let text = self.text.as_ref();
        let (a, b) = (start.min(end).min(text.len()), start.max(end).min(text.len()));
        text.get(a..b).unwrap_or("").to_string()
    }
    fn selection_get_full_text(&self) -> String {
        if self.selection_text_len() == 0 {
            String::new()
        } else {
            self.text.as_ref().to_string()
        }
    }
}

impl Widget for Label {
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        node.role = a11y_tree::Role::StaticText;
        makepad_platform::a11y::set_string(&mut node.name, self.text.as_ref());
        let text = node.text.get_or_insert_with(Default::default);
        makepad_platform::a11y::set_string(&mut text.content, self.text.as_ref());
        text.selection = self.selection.map_or((0, 0), |(a, b)| (a as u32, b as u32));
        if let Some((rect, layout)) = &self.laid {
            crate::semantics::text_layout(
                text,
                layout,
                rect.pos + self.shift(cx),
                self.draw_text.font_scale,
                false,
            );
        }
    }

    fn semantics_parts(&self, cx: &Cx, out: &mut Vec<(u64, Option<u64>, a11y_tree::Node)>) {
        if !self.visible || self.links.is_empty() {
            return;
        }
        let Some((rect, laid)) = &self.laid else { return };
        if laid.rows.is_empty() {
            return;
        }
        let text = self.text.as_ref();
        let pos = rect.pos + self.shift(cx);
        let scale = self.draw_text.font_scale as f64;
        let ink_shift = if self.draw_text.ink_centered {
            laid.ink_center_offset_in_lpxs() as f64 * scale
        } else {
            0.0
        };
        for (i, &(a, b)) in self.links.iter().enumerate() {
            let Some(slice) = text.get(a..b).filter(|s| !s.is_empty()) else { continue };
            let selection = Selection {
                anchor: Cursor { index: a, prefer_next_row: false },
                cursor: Cursor { index: b, prefer_next_row: false },
            };
            let bounds = laid.selection_rects(selection).into_iter().map(|span| {
                let r = span.rect_in_lpxs;
                Rect {
                    pos: dvec2(pos.x + r.origin.x as f64 * scale, pos.y + ink_shift + r.origin.y as f64 * scale),
                    size: dvec2(r.size.width as f64 * scale, r.size.height as f64 * scale),
                }
            }).filter(|r| r.size.x > 0.0 && r.size.y > 0.0).reduce(|a, b| a.hull(b));
            if let Some(bounds) = bounds {
                out.push((i as u64 + 1, None, a11y_tree::Node {
                    role: a11y_tree::Role::Link,
                    name: slice.to_owned(),
                    actions: a11y_tree::Actions::PRESS,
                    bounds: makepad_platform::a11y::semantic_rect(bounds),
                    ..Default::default()
                }));
            }
        }
    }

    fn handle_action(&mut self, cx: &mut Cx, action: &a11y_tree::ActionRequest, _scope: &mut Scope) -> a11y_tree::ActionResult {
        use a11y_tree::{ActionResult, Actions};
        if self.visible && action.action == Actions::PRESS {
            if let Some(index) = cx.a11y.action_part_key(action.node).and_then(|k| k.checked_sub(1)) {
                let index = index as usize;
                if index < self.links.len() {
                    cx.widget_action(self.uid, LabelAction::LinkPressed(index));
                    return ActionResult::Handled;
                }
            }
        }
        ActionResult::Unhandled
    }

    /// A label reports the pointer at most (hover actions); it never takes
    /// a press, so a host's gestures (selecting, scrolling) pass over it.
    fn is_interactive(&self) -> bool {
        self.hover_actions_enabled
    }

    /// Its text, laid out (hidden labels draw nothing).
    fn scan(&mut self, cx: &mut Cx, _scan: &ScanEvent, _scope: &mut Scope) {
        self.draw_text.scan_text(cx, self.text.as_ref());
    }

    /// What this label would be worth on a row, with `over` in force.
    ///
    /// A label that can wrap answers `None` unless its width is stated: where a
    /// wrapping label breaks is the turtle's business, so its own width is not a
    /// number it can give.
    fn measure_width(
        &mut self,
        cx: &mut Cx2d,
        over: Option<&crate::width_override::WidthOverride>,
    ) -> Option<f64> {
        if over.is_some_and(|o| o.opaque) {
            return None;
        }
        // A block that states `visible` states it: the child may be wearing the
        // opposite right now, and a price taken off what it is wearing is a
        // price of the concession rather than of the face being priced.
        let visible = over.and_then(|o| o.visible).unwrap_or(self.visible);
        if !visible {
            return Some(0.0);
        }

        let margin = over.and_then(|o| o.margin).unwrap_or(self.walk.margin);
        let width = over.and_then(|o| o.width).unwrap_or(self.walk.width);
        if let Size::Fixed(w) = width {
            return Some(w + margin.width());
        }
        // A Fill takes what the others leave, so what it adds to the row's own
        // width is its minimum -- nothing, for a plain spacer. Answering None
        // here would make a row with a spacer in it unpriceable, which is most
        // rows.
        if let Size::Fill { min, .. } = width {
            return Some(min.unwrap_or(0.0) + margin.width());
        }
        // Only a Fit label has a width of its own to report.
        if !width.is_fit() {
            return None;
        }

        let pad = over.and_then(|o| o.padding).unwrap_or(self.padding);
        let text: &str = match over.and_then(|o| o.text.as_deref()) {
            Some(t) => t,
            None => self.text.as_ref(),
        };
        // An empty label draws a space (`draw_walk`), so it is priced as one:
        // priced as nothing, a row on its edge took that space as room.
        let text = if text.is_empty() { " " } else { text };
        let text_w = crate::badge::advance(&self.draw_text, cx, text);
        Some(pad.width() + text_w + margin.width())
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(text) {
            let str_val = vm.new_string(self.text.as_ref());
            return ScriptAsyncResult::Return(str_val.into());
        }
        if method == live_id!(set_text) {
            if let Some(args_obj) = args.as_object() {
                let value = vm.vec_value_trap(args_obj, 0);
                if !value.is_err() {
                    if let Some(new_text) = vm.cast_to_owned_string(value, "copying label text")
                    {
                        vm.with_cx_mut(|cx| {
                            self.set_text(cx, &new_text);
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        let walk = cx.resolve_walk(
            walk.with_add_padding(self.padding),
            ResolveAt::BeforeBegin,
        );
        cx.begin_turtle(
            walk,
            Layout {
                flow: self.flow,
                ..Default::default()
            },
        );
        // Empty text is drawn as a space (an empty draw lays out nothing,
        // not even the line height). The text itself stays empty: setting
        // it here made every host that writes "" again (on each pump) see a
        // change, redraw, and keep the window repainting forever.
        let shown: &str = if self.text.as_ref().is_empty() { " " } else { self.text.as_ref() };
        self.draw_text.max_lines = self.max_lines;
        self.draw_text.text_overflow = self.text_overflow;
        // The selection wash needs the layout, so it is drawn after the text;
        // its call goes in first and its draw_call_group brings the wash back
        // to it, behind the glyphs.
        if self.selection.is_some() {
            self.draw_selection.new_draw_call(cx);
        }
        let base = self.draw_text.color;
        let (links, ink, inks) = (&self.links, self.link_color, &self.inks);
        let restyle = move |laid: &LaidoutText| {
            // Row alignment is computed before ellipsis truncation, so an
            // overflowing row can retain a negative origin even after it fits.
            // Keep the start visible by left-aligning those rows. Use the same
            // corrected layout for drawing, selection and link hit-testing.
            let clips_start = laid.rows.iter().any(|row| row.origin_in_lpxs.x < 0.0);
            let mut out = if !links.is_empty() || !inks.is_empty() {
                // Every glyph gets its colour (a glyph's own colour sticks for
                // the ones after it), the links' in the link ink.
                recolor_links(laid, links, inks, base, ink)
            } else if clips_start {
                laid.clone()
            } else {
                return None;
            };
            if clips_start {
                for row in &mut out.rows {
                    row.origin_in_lpxs.x = row.origin_in_lpxs.x.max(0.0);
                }
            }
            Some(out)
        };
        let (rect, laid) =
            self.draw_text.draw_walk_restyled(cx, walk, self.align, shown, Some(&restyle));
        self.draw_text.color = base;
        if let Some((a, b)) = self.selection {
            self.draw_selection_rects(cx, rect, &laid, a, b);
        }
        if let Some(&(a, b)) = self.link_hover.and_then(|i| self.links.get(i)) {
            self.draw_link_underline(cx, rect, &laid, a, b);
        }
        self.laid = Some((rect, laid));
        cx.end_turtle_with_area(&mut self.area);
        self.drawn_pos = self.area.rect(cx).pos;
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        // Identical text produces identical visuals, so skip the string
        // rebuild and the redraw that would dirty the whole draw list.
        if self.text.as_ref() == v {
            return;
        }
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }

        let uid = self.widget_uid();

        // A label REPORTS where the pointer is. It never takes it.
        //
        // This asked with `capture_overload`, which buys a hover nothing at
        // all -- `hits` reads that flag only on a press -- while costing the
        // label a co-capture of every press that landed on it, and the
        // `handled` mark of the widget that really took that press along with
        // it. Under the pointer-capture rule a capture is a claim on the
        // pointer, and a host that sees one stands its own gesture down: a
        // plain row of text with hover actions on was enough to stop a
        // drag-to-scroll dead.
        //
        // Hovers leave `hits` on these two events and no others, so asking on
        // them alone reports exactly what it reported before and takes
        // nothing. There is no press path left to take anything on.
        if self.hover_actions_enabled && matches!(event, Event::MouseMove(_) | Event::MouseLeave(_))
        {
            match event.hits(cx, self.area) {
                Hit::FingerHoverIn(fh) => {
                    cx.widget_action(uid, LabelAction::HoverIn(fh.rect));
                }
                Hit::FingerHoverOut(_) => {
                    cx.widget_action(uid, LabelAction::HoverOut);
                }
                _ => (),
            }
        }
    }
}

/// A copy of `laid` with every glyph coloured: `ink` inside the `links`
/// byte ranges, an ink range's own colour inside it, `base` elsewhere.
fn recolor_links(laid: &LaidoutText, links: &[(usize, usize)], inks: &[(usize, usize, Vec4f)], base: Vec4f, ink: Vec4f) -> LaidoutText {
    use makepad_draw::text::color::Color;
    let color = |c: Vec4f| {
        let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Color::new(b(c.x), b(c.y), b(c.z), b(c.w))
    };
    let (base_color, ink) = (color(base), color(ink));
    let mut out = laid.clone();
    let text_start = laid.text.start_in_parent();
    for row in out.rows.iter_mut() {
        let row_start = row.text.start_in_parent().saturating_sub(text_start);
        for glyph in row.glyphs.iter_mut() {
            let at = row_start + glyph.cluster;
            let linked = links.iter().any(|&(a, b)| at >= a && at < b);
            let inked = inks.iter().find(|&&(a, b, _)| at >= a && at < b);
            glyph.color = Some(match inked {
                _ if linked => ink,
                Some(&(_, _, c)) => color(c),
                None => base_color,
            });
        }
    }
    out
}

impl Label {
    /// The label's whole laid-out box (its `area()` is the text's draw area, which can be one
    /// glyph's instance): what the window keeps a caption title off.
    pub(crate) fn box_rect(&self, cx: &Cx) -> Rect {
        self.area.rect(cx)
    }
}

impl Label {
    /// How far the label moved since it was drawn.
    fn shift(&self, cx: &Cx) -> DVec2 {
        if self.area.is_valid(cx) {
            self.area.rect(cx).pos - self.drawn_pos
        } else {
            DVec2::default()
        }
    }

    fn draw_link_underline(&mut self, cx: &mut Cx2d, rect: Rect, laid: &LaidoutText, a: usize, b: usize) {
        let scale = self.draw_text.font_scale as f64;
        let shift = if self.draw_text.ink_centered { laid.ink_center_offset_in_lpxs() as f64 * scale } else { 0.0 };
        let selection = Selection {
            anchor: Cursor { index: a, prefer_next_row: false },
            cursor: Cursor { index: b, prefer_next_row: false },
        };
        for SelectionRect { rect_in_lpxs, ascender_in_lpxs } in laid.selection_rects(selection) {
            // One point under the baseline.
            let base_y = rect.pos.y + shift + (rect_in_lpxs.origin.y + ascender_in_lpxs) as f64 * scale + 1.5;
            self.draw_link.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x + rect_in_lpxs.origin.x as f64 * scale, base_y),
                    size: dvec2(rect_in_lpxs.size.width as f64 * scale, 1.0),
                },
            );
        }
    }

    fn draw_selection_rects(&mut self, cx: &mut Cx2d, rect: Rect, laid: &LaidoutText, a: usize, b: usize) {
        let scale = self.draw_text.font_scale as f64;
        let shift = if self.draw_text.ink_centered { laid.ink_center_offset_in_lpxs() as f64 * scale } else { 0.0 };
        let selection = Selection {
            anchor: Cursor { index: a, prefer_next_row: false },
            cursor: Cursor { index: b, prefer_next_row: false },
        };
        for SelectionRect { rect_in_lpxs, .. } in laid.selection_rects(selection) {
            self.draw_selection.draw_abs(
                cx,
                Rect {
                    pos: dvec2(
                        rect.pos.x + rect_in_lpxs.origin.x as f64 * scale,
                        rect.pos.y + shift + rect_in_lpxs.origin.y as f64 * scale,
                    ),
                    size: dvec2(rect_in_lpxs.size.width as f64 * scale, rect_in_lpxs.size.height as f64 * scale),
                },
            );
        }
    }

    /// Sets the text color.
    ///
    /// Does nothing if the color is unchanged.
    pub fn set_text_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.draw_text.color == color {
            return;
        }
        self.draw_text.color = color;
        self.redraw(cx);
    }
}

impl LabelRef {
    pub fn text(&self) -> String {
        if let Some(inner) = self.borrow() {
            inner.text()
        } else {
            String::new()
        }
    }

    pub fn set_text(&self, cx: &mut Cx, text: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text(cx, text);
        }
    }

    /// See [`Label::set_text_color()`].
    pub fn set_text_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_text_color(cx, color);
        }
    }

    pub fn hover_in(&self, actions: &Actions) -> Option<Rect> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                LabelAction::HoverIn(rect) => Some(rect),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn hover_out(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            match item.cast() {
                LabelAction::HoverOut => true,
                _ => false,
            }
        } else {
            false
        }
    }

    /// Returns the link span index activated by an accessibility press on this label.
    pub fn link_pressed(&self, actions: &Actions) -> Option<usize> {
        actions.filter_widget_actions(self.widget_uid()).find_map(|item| {
            match item.cast::<LabelAction>() {
                LabelAction::LinkPressed(index) => Some(index),
                _ => None,
            }
        })
    }

    /// Draw these byte ranges of the text as links (in `link_color`), the
    /// `hover` one underlined. The host hit-tests them itself (through
    /// `selection_point_to_char_index`); the label takes no pointer presses.
    pub fn set_links(&self, cx: &mut Cx, links: &[(usize, usize)], hover: Option<usize>) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.links != links || inner.link_hover != hover {
                inner.links = links.to_vec();
                inner.link_hover = hover;
                inner.redraw(cx);
            }
        }
    }

    /// Draw these byte ranges of the text each in its own colour: (start, end, colour).
    pub fn set_inks(&self, cx: &mut Cx, inks: &[(usize, usize, Vec4f)]) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.inks != inks {
                inner.inks = inks.to_vec();
                inner.redraw(cx);
            }
        }
    }

    pub fn set_text_with<F: FnOnce(&mut String)>(&self, f: F) {
        if let Some(mut inner) = self.borrow_mut() {
            f(inner.text.as_mut())
        }
    }
}

/// THE POINTER-CAPTURE RULE, as it applies to a label.
///
/// A label with hover actions on REPORTS where the pointer is. It must not
/// take the pointer to do it: a capture is a claim, and every host that drags,
/// pans or scrolls stands its own gesture down while something else holds the
/// mouse. A row of text that quietly captured every press on it was enough to
/// stop a list scrolling.
#[cfg(test)]
mod pointer_capture_tests {
    #![allow(dead_code)]
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_move(abs: Vec2d) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.02,
            handled: Cell::new(Area::Empty),
        })
    }

    fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    fn middle(cx: &Cx, widget: &WidgetRef) -> Vec2d {
        let rect = widget.area().rect(cx);
        assert!(rect.size.x > 0.0 && rect.size.y > 0.0, "not drawn");
        rect.pos + rect.size * 0.5
    }

    fn scene(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    note := Label{
                        width: 200.
                        height: 40.
                        text: "hover me"
                        hover_actions_enabled: true
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = scene(cx);
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let note = root.widget(cx, ids!(note));
        (root, note)
    }

    fn hovered(actions: &Actions, note: &WidgetRef) -> bool {
        actions
            .iter()
            .filter_map(|a| a.as_widget_action())
            .any(|a| {
                a.widget_uid == note.widget_uid()
                    && matches!(a.cast::<LabelAction>(), LabelAction::HoverIn(_))
            })
    }

    fn link_scene(cx: &mut Cx) -> (WidgetRef, LabelRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    align: Align{y: 1.0}
                    note := Label{text: "See #12 and #34"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let note = root.label(cx, ids!(note));
        note.set_links(cx, &[(4, 7), (12, 15)], None);
        (root, note)
    }

    fn draw_link_tree(cx: &mut Cx, target: &mut Target, root: &WidgetRef) -> a11y_tree::mirror::Tree {
        use makepad_platform::a11y::{semantic_rect, snapshot_rect};
        cx.a11y.on = true;
        cx.a11y.begin_frame();
        let window = cx.a11y.begin_node(WidgetUid::new().0, None);
        cx.a11y.begin_window(WINDOW);
        target.draw(cx, root);
        cx.a11y.end_window();
        cx.a11y.end_node(window, a11y_tree::Node { role: a11y_tree::Role::Window, ..Default::default() }, Area::Empty);
        let mut a11y = std::mem::take(&mut cx.a11y);
        let mut tree = a11y_tree::mirror::Tree::default();
        a11y.finish_frame(
            Area::Empty,
            |_| None,
            |area| snapshot_rect(&area, cx).map(semantic_rect),
            |area| area.is_valid(cx).then(|| {
                let p = area.rect(cx).pos;
                [p.x as f32, p.y as f32]
            }),
            |_, bytes| { tree.apply(a11y_tree::wire::decode_update(&bytes).unwrap()); },
        );
        cx.a11y = a11y;
        tree
    }

    #[test]
    fn label_links_expose_a11y_link_parts_and_press_emits_index() {
        let pressed = Rc::new(Cell::new(None));
        let observed = pressed.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| {
            if let Event::A11yAction(_) = event {
                let root = cx.get_global::<WidgetRef>().clone();
                let actions = send(cx, &root, event);
                observed.set(root.label(cx, ids!(note)).link_pressed(&actions));
            }
        }));
        let (root, note) = link_scene(&mut cx);
        cx.set_global(root.clone());
        let mut target = Target::new(&mut cx);
        let tree = draw_link_tree(&mut cx, &mut target, &root);
        let owner = tree.node(a11y_tree::NodeId::widget(note.widget_uid().0)).expect("the label is in the tree");
        let parts: Vec<_> = tree.children(owner.id).collect();
        assert_eq!(parts.len(), 2, "the label exposes both links as children");
        for (part, name) in parts.iter().zip(["#12", "#34"]) {
            assert_eq!(part.role, a11y_tree::Role::Link, "the span is a link");
            assert_eq!(part.name, name, "the link is named by its text");
            assert!(part.actions.contains(a11y_tree::Actions::PRESS), "the link offers a press");
            let b = part.bounds;
            let label = owner.bounds;
            assert!(!b.is_empty(), "{name} has a non-empty hit target");
            assert!(b.x >= label.x && b.y >= label.y && b.x + b.w <= label.x + label.w && b.y + b.h <= label.y + label.h,
                "{name} bounds {b:?} must be inside the label {label:?}");
        }
        assert_ne!(parts[0].bounds, parts[1].bounds, "the links have distinct hit targets");
        cx.dispatch_a11y_action(WINDOW, a11y_tree::ActionRequest {
            req: 1, node: parts[1].id, action: a11y_tree::Actions::PRESS, arg: a11y_tree::ActionArg::None,
        });
        assert!(cx.take_platform_ops().iter().any(|op| matches!(op,
            CxOsOp::A11yActionDone { req: 1, result } if *result == a11y_tree::ActionResult::Handled as u8)),
            "the label handled the accessibility press");
        assert_eq!(pressed.get(), Some(1), "pressing the second link emits its zero-based span index");
    }

    #[test]
    fn label_link_part_bounds_follow_parent_turtle_shift() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, note) = link_scene(&mut cx);
        note.borrow_mut().unwrap().padding = Inset::default();
        let mut target = Target::new(&mut cx);
        let tree = draw_link_tree(&mut cx, &mut target, &root);
        let label = tree.node(a11y_tree::NodeId::widget(note.widget_uid().0)).expect("the shifted label is in the tree");
        let drawn_y = note.borrow().unwrap().drawn_pos.y as f32;
        assert!(label.bounds.y > drawn_y + 2.0, "the parent moved the label down after its draw");
        let part = tree.children(label.id).next().expect("the shifted label has a link part");
        assert!((part.bounds.y - label.bounds.y).abs() < 2.0,
            "the link at {} must follow its label from {drawn_y} to {}", part.bounds.y, label.bounds.y);
    }

    #[test]
    fn label_with_links_still_never_captures_pointer() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, note) = link_scene(&mut cx);
        let mut target = Target::new(&mut cx);
        let tree = draw_link_tree(&mut cx, &mut target, &root);
        let owner = a11y_tree::NodeId::widget(note.widget_uid().0);
        let part = tree.children(owner).next().expect("the label has a link to press");
        let (x, y) = part.bounds.center();
        assert!(!note.borrow().unwrap().is_interactive(), "links alone do not make a label pointer-interactive");
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let event = press(dvec2(x as f64, y as f64));
        send(&mut cx, &root, &event);
        assert!(!cx.fingers.any_areas_captured(), "a primary press on a linked label leaves the digit free");
    }

    /// The control. Without this the test below would pass on a label that
    /// had stopped reporting hovers altogether.
    #[test]
    fn a_label_with_hover_actions_on_still_reports_the_hover() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, note) = start(&mut cx);
        let at = middle(&cx, &note);
        let actions = send(&mut cx, &root, &mouse_move(at));
        assert!(hovered(&actions, &note), "the pointer arriving is reported");
    }

    /// The bug: reporting a hover used to cost a co-capture of every press,
    /// which under the pointer-capture rule tells every host around it to
    /// stand its own gesture down.
    #[test]
    fn a_press_on_a_label_takes_no_hold_on_the_pointer() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, note) = start(&mut cx);
        let at = middle(&cx, &note);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let event = press(at);
        send(&mut cx, &root, &event);
        assert!(
            !cx.fingers.any_areas_captured(),
            "a label reports the pointer; it never holds it"
        );
        let Event::MouseDown(e) = &event else { unreachable!() };
        assert!(
            e.handled.get().is_empty(),
            "and it does not claim the press out from under whoever the press was for"
        );
        cx.fingers.first_mouse_button = None;
    }
}
