use std::collections::{HashMap, HashSet};

use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    fold_button::{FoldButton, FoldButtonAction},
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_html::*,
    text_flow::TextFlow,
    widget::*,
    WidgetMatchEvent,
};

const BULLET: &str = "•";

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.HtmlLinkBase = #(HtmlLink::register_widget(vm))

    mod.widgets.HtmlBase = #(Html::register_widget(vm))

    mod.widgets.HtmlLink = set_type_default() do mod.widgets.HtmlLinkBase{
        width: Fit height: Fit
        align: Align{x: 0. y: 0.}

        color: #x0000EE
        hover_color: #x00EE00
        pressed_color: #xEE0000

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: 0.0
                        pressed: 0.0
                    }
                }

                on: AnimatorState{
                    redraw: true
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        hovered: snap(1.0)
                        pressed: snap(1.0)
                    }
                }

                pressed: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.01}}
                    apply: {
                        hovered: snap(1.0)
                        pressed: snap(1.0)
                    }
                }
            }
        }
    }

    mod.widgets.Html = set_type_default() do mod.widgets.HtmlBase{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        padding: theme.mspace_1

        ul_markers: ["•", "-"]
        ol_separator: "."

        heading_margin: Inset{top: 1.0, bottom: 0.1}
        paragraph_margin: Inset{top: 0.33, bottom: 0.33}

        font_size: theme.font_size_p
        font_color: theme.color_label_inner

        draw_text +: {
            color: theme.color_label_inner
        }

        text_style_normal: theme.font_regular{
            font_size: theme.font_size_p
        }

        text_style_italic: theme.font_italic{
            font_size: theme.font_size_p
        }

        text_style_bold: theme.font_bold{
            font_size: theme.font_size_p
        }

        text_style_bold_italic: theme.font_bold_italic{
            font_size: theme.font_size_p
        }

        text_style_fixed: theme.font_code{
            font_size: theme.font_size_p
        }

        code_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        code_walk: Walk{width: Fill height: Fit}

        quote_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: theme.space_2}
        }
        quote_walk: Walk{width: Fill height: Fit}

        list_item_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: theme.mspace_1
        }
        list_item_walk: Walk{
            height: Fit width: Fill
        }

        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1

        sep_walk: Walk{
            width: Fill height: 4.
            margin: theme.mspace_v_1
        }

        a := mod.widgets.HtmlLink{}

        // Triangle that expands/collapses a <details> section.
        //
        // The default FoldButton shader draws a hard-coded 5-wide triangle at
        // x=5, which doesn't scale with the button's rect. We override the
        // pixel function so the triangle is centered in `rect_size` and sized
        // proportionally, letting the widget set the walk at runtime based on
        // the surrounding summary font size. Width, height, and margin are
        // computed per-draw and passed via `draw_walk_fold_button`.
        details_arrow := mod.widgets.FoldButton{
            draw_bg +: {
                pixel: fn() {
                    let c = self.rect_size * 0.5
                    let sz = self.rect_size.y * 0.28
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.clear(vec4(0.))
                    sdf.rotate(self.active * 0.5 * PI + 0.5 * PI, c.x, c.y)
                    sdf.move_to(c.x - sz, c.y + sz)
                    sdf.line_to(c.x, c.y - sz)
                    sdf.line_to(c.x + sz, c.y + sz)
                    sdf.close_path()
                    sdf.fill(
                        mix(
                            mix(self.color, self.color_hover, self.hover)
                            mix(self.color_active, self.color_hover, self.hover)
                            self.active
                        )
                    )
                    return sdf.result * self.fade
                }
            }
        }

        draw_block +: {
            line_color: theme.color_label_inner
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_label_inner
            code_color: theme.color_bg_highlight
            selection_color: theme.color_selection_focus
            table_header_bg_color: theme.color_bg_highlight
            table_border_color: theme.color_shadow
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)
        }
    }
}

/// Whether to trim leading and trailing whitespace in the text body of an HTML tag.
///
/// Currently, *all* Unicode whitespace characters are trimmed, not just ASCII whitespace.
///
/// The default is to keep all whitespace.
#[derive(Copy, Clone, PartialEq, Default)]
pub enum TrimWhitespaceInText {
    /// Leading and trailing whitespace will be preserved in the text.
    #[default]
    Keep,
    /// Leading and trailing whitespace will be trimmed from the text.
    Trim,
}

#[derive(Script, Widget)]
pub struct Html {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    pub text_flow: TextFlow,
    #[live]
    pub body: ArcStringMut,
    #[rust]
    pub doc: HtmlDoc,

    /// Markers used for unordered lists, indexed by the list's nesting level.
    /// The marker can be an arbitrary string, such as a bullet point or a custom icon.
    #[live]
    ul_markers: Vec<String>,
    /// Markers used for ordered lists, indexed by the list's nesting level.
    #[rust]
    ol_markers: Vec<OrderedListType>,
    /// The character used to separate an ordered list's item number from the content.
    #[live]
    ol_separator: String,

    /// The stack of list levels encountered so far, used to track nested lists.
    #[rust]
    list_stack: Vec<ListLevel>,

    /// The stack of currently-open `<details>` tags while traversing the
    /// document. Rebuilt on each draw.
    #[rust]
    details_stack: Vec<DetailsLevel>,

    /// IDs of `<details>` FoldButtons whose initial open/closed state has
    /// already been seeded from their HTML `open` attribute. Persists across
    /// redraws so user clicks aren't overwritten.
    #[rust]
    seen_details: HashSet<LiveId>,

    /// When `Some`, the draw walk is skipping nodes because the enclosing
    /// `<details>` is collapsed. The inner counter tracks the nested open-tag
    /// depth while skipping, so the matching `</details>` can be recognized.
    #[rust]
    skip_details_depth: Option<i32>,

    /// Transparent DrawQuad emitted over each `<summary>` so the whole summary
    /// line is clickable, not just the fold triangle. The quad's default
    /// shader produces `#0000`, so it's invisible but its instance area
    /// participates in normal `event.hits` hit-testing.
    #[live]
    draw_summary_hit: DrawQuad,

    /// Per-draw list of `(details_id, hit_area)` for each `<summary>` we
    /// rendered, used by `handle_event` to route clicks on the summary line
    /// to the matching FoldButton.
    #[rust]
    summary_click_areas: Vec<(LiveId, Area)>,

    /// Previous-frame hit area per details id, used to preserve hover/capture
    /// state across redraws via `update_area_refs`.
    #[rust]
    summary_area_cache: HashMap<LiveId, Area>,
}

impl ScriptHook for Html {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        // Initialize ol_markers with default values
        if self.ol_markers.is_empty() {
            self.ol_markers = vec![
                OrderedListType::Numbers,
                OrderedListType::LowerAlpha,
                OrderedListType::LowerRoman,
            ];
        }
    }

    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let mut errors = Some(Vec::new());
        let new_doc = parse_html(self.body.as_ref(), &mut errors, InternLiveId::No);
        if new_doc != self.doc {
            self.doc = new_doc;
            self.text_flow.clear_items();
            self.seen_details.clear();
            self.summary_area_cache.clear();
        }
        if errors.as_ref().unwrap().len() > 0 {
            log!("HTML parser returned errors {:?}", errors)
        }
    }
}

impl Html {
    /// Vertical spacing inserted before a `<details>` opens and after it
    /// closes, in pixels, scaled by the current font size. Keeps the block
    /// from butting up against surrounding content. A single helper so the
    /// open and close handlers can't drift out of sync.
    fn details_margin_em(&self) -> f64 {
        let fs = *self
            .text_flow
            .font_sizes
            .last()
            .unwrap_or(&self.text_flow.font_size) as f64;
        fs * 0.22
    }

    fn count_table_columns(nodes: &[HtmlNode], start_index: usize) -> usize {
        let mut count = 0;
        let mut in_first_row = false;
        let mut depth = 0;
        for node in &nodes[start_index + 1..] {
            match node {
                HtmlNode::OpenTag { lc, .. } => {
                    if *lc == live_id!(table) {
                        depth += 1;
                    } else if depth == 0 && *lc == live_id!(tr) && !in_first_row {
                        in_first_row = true;
                    } else if depth == 0
                        && in_first_row
                        && (*lc == live_id!(td) || *lc == live_id!(th))
                    {
                        count += 1;
                    }
                }
                HtmlNode::CloseTag { lc, .. } => {
                    if *lc == live_id!(table) {
                        if depth > 0 {
                            depth -= 1;
                        } else {
                            return count;
                        }
                    }
                    if depth == 0 && *lc == live_id!(tr) && in_first_row {
                        return count;
                    }
                }
                _ => {}
            }
        }
        count
    }

    fn handle_open_tag(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        node: &mut HtmlWalker,
        list_stack: &mut Vec<ListLevel>,
        ul_markers: &Vec<String>,
        ol_markers: &Vec<OrderedListType>,
        ol_separator: &str,
    ) -> (Option<LiveId>, TrimWhitespaceInText) {
        let mut trim_whitespace_in_text = TrimWhitespaceInText::default();

        fn open_header_tag(
            cx: &mut Cx2d,
            tf: &mut TextFlow,
            scale: f64,
            trim: &mut TrimWhitespaceInText,
        ) {
            *trim = TrimWhitespaceInText::Trim;
            tf.bold.push();
            tf.push_size_abs_scale(scale);
            let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
            tf.new_line_collapsed_with_spacing(cx, fs * tf.heading_margin.top);
        }

        match node.open_tag_lc() {
            some_id!(h1) => open_header_tag(cx, tf, 2.0, &mut trim_whitespace_in_text),
            some_id!(h2) => open_header_tag(cx, tf, 1.5, &mut trim_whitespace_in_text),
            some_id!(h3) => open_header_tag(cx, tf, 1.17, &mut trim_whitespace_in_text),
            some_id!(h4) => open_header_tag(cx, tf, 1.0, &mut trim_whitespace_in_text),
            some_id!(h5) => open_header_tag(cx, tf, 0.83, &mut trim_whitespace_in_text),
            some_id!(h6) => open_header_tag(cx, tf, 0.67, &mut trim_whitespace_in_text),

            some_id!(p) => {
                let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
                tf.new_line_collapsed_with_spacing(cx, fs * tf.paragraph_margin.top);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(code) => {
                const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                tf.combine_spaces.push(false);
                tf.fixed.push();
                tf.inline_code.push();
            }
            some_id!(pre) => {
                tf.new_line_collapsed(cx);
                tf.fixed.push();
                tf.ignore_newlines.push(false);
                tf.combine_spaces.push(false);
                tf.begin_code(cx);
            }
            some_id!(blockquote) => {
                tf.new_line_collapsed(cx);
                tf.ignore_newlines.push(false);
                tf.combine_spaces.push(false);
                tf.begin_quote(cx);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(br) => {
                tf.new_line_with_wrap_spacing(cx);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(hr) | some_id!(sep) => {
                tf.new_line_collapsed(cx);
                tf.sep(cx);
                tf.new_line_collapsed(cx);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(u) => tf.underline.push(),
            some_id!(del) | some_id!(s) | some_id!(strike) => tf.strikethrough.push(),

            some_id!(b) | some_id!(strong) => tf.bold.push(),
            some_id!(i) | some_id!(em) => tf.italic.push(),

            some_id!(sub) => {
                tf.push_size_rel_scale(0.7);
                // Shift the subscript baseline downward, relative to the
                // subscript's own (smaller) font size. The value has to
                // also cancel the natural upward drift that comes from a
                // smaller font having a smaller ascender.
                tf.y_shift_scales.push(0.55);
            }
            some_id!(sup) => {
                tf.push_size_rel_scale(0.7);
                // Shift the superscript baseline upward, relative to the
                // superscript's own (smaller) font size. Smaller-than-the
                // subscript shift because the smaller ascender already
                // raises the baseline a bit on its own.
                tf.y_shift_scales.push(-0.2);
            }
            some_id!(ul) => {
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
                list_stack.push(ListLevel {
                    list_kind: ListKind::Unordered,
                    numbering_type: None,
                    li_count: 1,
                    padding: 2.5,
                });
            }
            some_id!(ol) => {
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
                let start_attr = node.find_attr_lc(live_id!(start));
                let start: i32 = start_attr.and_then(|s| s.parse().ok()).unwrap_or(1);

                let type_attr = node.find_attr_lc(live_id!(type));
                let numbering_type = type_attr.and_then(OrderedListType::from_type_attribute);

                list_stack.push(ListLevel {
                    list_kind: ListKind::Ordered,
                    numbering_type,
                    li_count: start,
                    padding: 2.5,
                });
            }
            some_id!(li) => {
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
                let indent_level = list_stack.len();
                let index = indent_level.saturating_sub(1);
                let marker_and_pad = list_stack.last_mut().map(|ll| {
                    let marker = match ll.list_kind {
                        ListKind::Unordered => ul_markers
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| BULLET.into()),
                        ListKind::Ordered => {
                            let value_attr = node.find_attr_lc(live_id!(value));
                            let value: i32 = value_attr
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(ll.li_count);

                            let type_attr = node.find_attr_lc(live_id!(type));
                            let numbering_type =
                                type_attr.and_then(OrderedListType::from_type_attribute);

                            numbering_type
                                .as_ref()
                                .or_else(|| ll.numbering_type.as_ref())
                                .or_else(|| ol_markers.get(index))
                                .map(|ol_type| ol_type.marker(value, ol_separator))
                                .unwrap_or_else(|| "#".into())
                        }
                    };
                    ll.li_count += 1;
                    (marker, ll.padding)
                });
                let (marker, pad) = marker_and_pad
                    .as_ref()
                    .map(|(m, p)| (m.as_str(), *p))
                    .unwrap_or((BULLET, 2.5));

                tf.new_line_collapsed(cx);
                tf.begin_list_item(cx, marker, pad);
            }
            some_id!(table) => {
                tf.new_line_collapsed(cx);
                let col_count = Self::count_table_columns(node.nodes, node.index);
                tf.begin_table(cx, col_count);
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(thead) => {
                tf.in_table_header = true;
            }
            some_id!(tbody) => {}
            some_id!(tr) => {
                if tf.in_table_header {
                    tf.begin_table_header_row(cx);
                } else {
                    tf.begin_table_row(cx);
                }
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(th) => {
                tf.table_row_is_header = true;
                tf.begin_table_cell(cx, cell_align_x(node));
                tf.bold.push();
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            some_id!(td) => {
                tf.begin_table_cell(cx, cell_align_x(node));
                trim_whitespace_in_text = TrimWhitespaceInText::Trim;
            }
            Some(x) => return (Some(x), trim_whitespace_in_text),
            _ => (),
        }
        (None, trim_whitespace_in_text)
    }

    fn handle_close_tag(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        node: &mut HtmlWalker,
        list_stack: &mut Vec<ListLevel>,
    ) -> Option<LiveId> {
        match node.close_tag_lc() {
            some_id!(h1)
            | some_id!(h2)
            | some_id!(h3)
            | some_id!(h4)
            | some_id!(h5)
            | some_id!(h6) => {
                let size = tf.font_sizes.pop();
                tf.bold.pop();
                tf.new_line_collapsed_with_spacing(
                    cx,
                    size.unwrap_or(0.0) as f64 * tf.heading_margin.bottom,
                );
            }
            some_id!(b) | some_id!(strong) => tf.bold.pop(),
            some_id!(i) | some_id!(em) => tf.italic.pop(),
            some_id!(p) => {
                let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
                tf.new_line_collapsed_with_spacing(cx, fs * tf.paragraph_margin.bottom);
            }
            some_id!(blockquote) => {
                tf.ignore_newlines.pop();
                tf.combine_spaces.pop();
                tf.end_quote(cx);
            }
            some_id!(code) => {
                tf.inline_code.pop();
                tf.font_sizes.pop();
                tf.combine_spaces.pop();
                tf.fixed.pop();
            }
            some_id!(pre) => {
                tf.fixed.pop();
                tf.ignore_newlines.pop();
                tf.combine_spaces.pop();
                tf.end_code(cx);
            }
            some_id!(sub) => {
                tf.font_sizes.pop();
                tf.y_shift_scales.pop();
            }
            some_id!(sup) => {
                tf.font_sizes.pop();
                tf.y_shift_scales.pop();
            }
            some_id!(ul) | some_id!(ol) => {
                list_stack.pop();
            }
            some_id!(li) => tf.end_list_item(cx),
            some_id!(u) => tf.underline.pop(),
            some_id!(del) | some_id!(s) | some_id!(strike) => tf.strikethrough.pop(),
            some_id!(table) => tf.end_table(cx),
            some_id!(thead) => {
                tf.in_table_header = false;
            }
            some_id!(tbody) => {}
            some_id!(tr) => {
                tf.end_table_row(cx);
            }
            some_id!(th) => {
                tf.bold.pop();
                tf.end_table_cell(cx);
            }
            some_id!(td) => tf.end_table_cell(cx),
            _ => (),
        }
        None
    }

    pub fn handle_text_node(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        node: &mut HtmlWalker,
        trim: TrimWhitespaceInText,
    ) -> bool {
        if let Some(text) = node.text() {
            let text = if trim == TrimWhitespaceInText::Trim {
                text.trim_matches(char::is_whitespace)
            } else {
                text
            };
            if tf.table_num_columns > 0 && node.text_is_all_ws() {
                return false;
            }
            tf.draw_text(cx, text);
            true
        } else {
            false
        }
    }
}

impl Widget for Html {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Route clicks on the summary line to the matching FoldButton so the
        // whole summary toggles, not just the triangle. Hit-test on the
        // transparent DrawQuad areas emitted over each summary rect — those
        // are `Area::Instance`s, so `event.hits` handles mouse/touch capture
        // uniformly, the same way HtmlLink does.
        //
        // `LiveId` and `Area` are both `Copy`, so we can iterate by value
        // without cloning the Vec.
        let mut details_toggle: Option<LiveId> = None;
        for &(details_id, area) in &self.summary_click_areas {
            match event.hits(cx, area) {
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(MouseCursor::Hand);
                }
                Hit::FingerUp(fu)
                    if fu.is_over && fu.is_primary_hit() && fu.was_tap() =>
                {
                    details_toggle = Some(details_id);
                    break;
                }
                _ => {}
            }
        }

        if let Some(details_id) = details_toggle {
            let fb_ref = self.text_flow.existing_item(details_id);
            // Scope the RefMut so it drops before we reborrow text_flow for
            // redraw. The `drop(fb)` trick won't satisfy the borrow checker
            // here — an explicit block cleanly ends the borrow.
            let toggled = {
                if let Some(mut fb) = fb_ref.borrow_mut::<FoldButton>() {
                    let new_state = !fb.is_open(cx);
                    fb.set_is_open(cx, new_state, Animate::Yes);
                    true
                } else {
                    false
                }
            };
            if toggled {
                self.text_flow.redraw(cx);
            }
        }

        self.text_flow.handle_event(cx, event, scope);

        // When a `<details>` FoldButton toggles from its own click handler,
        // redraw so the collapsed body appears/disappears. We filter by
        // widget_uid so an unrelated FoldButton elsewhere in the app doesn't
        // trigger an Html redraw. Animator frames fire `Animating` actions
        // too, but those already drive the FoldButton's own redraw — we only
        // need to rebuild the TextFlow on the open/close edge.
        if let Event::Actions(actions) = event {
            'outer: for action in actions {
                let Some(widget_action) = action.as_widget_action() else {
                    continue;
                };
                if !matches!(
                    widget_action.cast::<FoldButtonAction>(),
                    FoldButtonAction::Opening | FoldButtonAction::Closing
                ) {
                    continue;
                }
                // Scan our own fold buttons for a uid match. `seen_details`
                // is the set of details ids we've instantiated buttons for,
                // and existing_item resolves each to its WidgetRef without
                // going through the global widget tree. Typical details
                // counts per Html are small (<10), so this is cheap.
                let ids: SmallVec<[LiveId; 8]> =
                    self.seen_details.iter().copied().collect();
                for id in ids {
                    let fb_ref = self.text_flow.existing_item(id);
                    if fb_ref.widget_uid() == widget_action.widget_uid {
                        self.text_flow.redraw(cx);
                        break 'outer;
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.text_flow.begin(cx, walk);
        let mut node = self.doc.new_walker();
        let mut auto_id: u64 = 0;
        let mut details_auto_id: u64 = 0;
        self.details_stack.clear();
        self.skip_details_depth = None;
        self.summary_click_areas.clear();
        while !node.done() {
            // If the enclosing <details> is collapsed, fast-skip nodes until
            // the matching </details> close tag at depth 0.
            if let Some(depth) = self.skip_details_depth.as_mut() {
                if node.open_tag_lc().is_some() {
                    *depth += 1;
                    node.walk();
                    continue;
                } else if let Some(close_tag) = node.close_tag_lc() {
                    if *depth == 0 && close_tag == live_id!(details) {
                        // Reached the matching </details>; leave skip mode and
                        // fall through so the close handler runs normally.
                        self.skip_details_depth = None;
                    } else {
                        if *depth > 0 {
                            *depth -= 1;
                        }
                        node.walk();
                        continue;
                    }
                } else {
                    node.walk();
                    continue;
                }
            }

            // Intercept <details> / <summary> open tags before the generic
            // handler, so <details> never falls through to handle_custom_widget
            // (which would jump_to_close and hide all content).
            if let Some(tag) = node.open_tag_lc() {
                if tag == live_id!(details) {
                    let details_id =
                        if let Some(id_str) = node.find_attr_lc(live_id!(id)) {
                            LiveId::from_str(id_str)
                        } else {
                            details_auto_id += 1;
                            // Offset into the high bits to avoid colliding with
                            // HtmlLink's auto_id (small ints) in the items map.
                            LiveId(0xd37a_115_0000_0000u64
                                .wrapping_add(details_auto_id))
                        };
                    let initial_open = node.find_attr_lc(live_id!(open)).is_some();
                    self.details_stack.push(DetailsLevel {
                        id: details_id,
                        is_open: initial_open,
                    });
                    // Small top margin so a `<details>` doesn't butt up
                    // against the preceding content. Scaled by the current
                    // font size so it tracks headings, sub/superscript, etc.
                    // A matching bottom margin is applied at `</details>`.
                    self.text_flow
                        .new_line_collapsed_with_spacing(cx, self.details_margin_em());
                    node.walk();
                    continue;
                }
                if tag == live_id!(summary) {
                    if let Some(&DetailsLevel { id: details_id, is_open: initial_open }) =
                        self.details_stack.last()
                    {
                        let fb_ref = self.text_flow.item_with_scope(
                            cx,
                            &mut Scope::empty(),
                            details_id,
                            live_id!(details_arrow),
                        );
                        if let Some(fb_ref) = fb_ref {
                            // Read these before borrowing fb so we don't
                            // hold two borrows of self.text_flow at once.
                            let summary_color = *self
                                .text_flow
                                .font_colors
                                .last()
                                .unwrap_or(&self.text_flow.font_color);
                            let font_size = *self
                                .text_flow
                                .font_sizes
                                .last()
                                .unwrap_or(&self.text_flow.font_size)
                                as f64;
                            let needs_seed =
                                !self.seen_details.contains(&details_id);
                            // Walk scaled to the current summary font size so
                            // the triangle tracks headings, `<sub>`, etc. The
                            // right margin is the gap between triangle and
                            // summary text. The top margin pushes the box
                            // down so the triangle's center lines up with
                            // the text's optical middle — `Flow::Right` uses
                            // `RowAlign::Top`, and a font_size-tall box on
                            // its own sits above the text baseline.
                            let triangle_walk = Walk {
                                abs_pos: None,
                                width: Size::Fixed(font_size),
                                height: Size::Fixed(font_size),
                                margin: Inset {
                                    left: 0.0,
                                    right: font_size * 0.2,
                                    top: font_size * 0.25,
                                    bottom: 0.0,
                                },
                                metrics: Metrics::default(),
                            };
                            // One borrow for all FoldButton mutations: seed
                            // the animator state on first sight (so the
                            // `open` HTML attribute is honored before any
                            // user click), override the triangle color so it
                            // matches the summary text, read the current
                            // open state back so `</summary>` knows whether
                            // to enter skip mode, and draw the triangle with
                            // a runtime-computed walk.
                            if let Some(mut fb) = fb_ref.borrow_mut::<FoldButton>() {
                                if needs_seed {
                                    fb.set_is_open(cx, initial_open, Animate::No);
                                }
                                fb.set_draw_color(cx, summary_color);
                                let is_open = fb.is_open(cx);
                                if let Some(dl) = self.details_stack.last_mut() {
                                    dl.is_open = is_open;
                                }
                                fb.draw_walk_fold_button(cx, triangle_walk);
                            }
                            if needs_seed {
                                self.seen_details.insert(details_id);
                            }
                        }
                    }
                    // Start tracking the glyph rects that get drawn for the
                    // summary text (excluding the FoldButton, which doesn't
                    // feed into areas_tracker) so the whole line becomes a
                    // click target in handle_event.
                    self.text_flow.areas_tracker.push_tracker();
                    self.text_flow.bold.push();
                    node.walk();
                    continue;
                }
            }

            // Intercept </summary> and </details> close tags.
            if let Some(close_tag) = node.close_tag_lc() {
                if close_tag == live_id!(summary) {
                    self.text_flow.bold.pop();
                    let (start, end) = self.text_flow.areas_tracker.pop_tracker();
                    if let Some(dl) = self.details_stack.last() {
                        // Compute the bounding rect from the laid-out glyph
                        // rects so we know where to put the invisible hit
                        // target quad. We use `Area::rect` (raw) not
                        // `clipped_rect`, because the draw_clip on rect-areas
                        // isn't populated inside a nested turtle.
                        let mut bounds: Option<Rect> = None;
                        for a in &self.text_flow.areas_tracker.areas[start..end] {
                            let r = a.rect(cx);
                            if r.size.x > 0.0 && r.size.y > 0.0 {
                                bounds = Some(match bounds {
                                    None => r,
                                    Some(b) => {
                                        let x0 = b.pos.x.min(r.pos.x);
                                        let y0 = b.pos.y.min(r.pos.y);
                                        let x1 = (b.pos.x + b.size.x)
                                            .max(r.pos.x + r.size.x);
                                        let y1 = (b.pos.y + b.size.y)
                                            .max(r.pos.y + r.size.y);
                                        Rect {
                                            pos: dvec2(x0, y0),
                                            size: dvec2(x1 - x0, y1 - y0),
                                        }
                                    }
                                });
                            }
                        }
                        if let Some(b) = bounds {
                            // Emit an invisible DrawQuad covering the summary
                            // line. Its `Area::Instance` has valid rect_pos
                            // and rect_size in the shader instance data, so
                            // `event.hits` can hit-test it correctly — unlike
                            // the glyph-run rect-areas, which need the outer
                            // pass turtle to close before their draw_clip is
                            // populated.
                            //
                            // Seeding `draw_vars.area` from the cache lets
                            // `update_area_refs` (called inside `draw_abs`)
                            // carry hover/capture state from the previous
                            // frame to the fresh instance.
                            let prev_area = self
                                .summary_area_cache
                                .get(&dl.id)
                                .copied()
                                .unwrap_or(Area::Empty);
                            self.draw_summary_hit.draw_vars.area = prev_area;
                            self.draw_summary_hit.draw_abs(cx, b);
                            let new_area = self.draw_summary_hit.draw_vars.area;
                            self.summary_area_cache.insert(dl.id, new_area);
                            self.summary_click_areas.push((dl.id, new_area));
                        }
                    }
                    // Only enter skip mode when there is an enclosing
                    // `<details>` that is collapsed. A stray `<summary>` with
                    // no parent `<details>` must not skip to the end of the
                    // document, which is what `map_or(true, ...)` would do.
                    if matches!(self.details_stack.last(), Some(dl) if !dl.is_open) {
                        self.skip_details_depth = Some(0);
                    }
                    node.walk();
                    continue;
                }
                if close_tag == live_id!(details) {
                    self.details_stack.pop();
                    // Matching bottom margin (see `<details>` open handler).
                    self.text_flow
                        .new_line_collapsed_with_spacing(cx, self.details_margin_em());
                    node.walk();
                    continue;
                }
            }

            // Regular tag/text handling for everything else.
            let tf = &mut self.text_flow;
            let mut trim = TrimWhitespaceInText::default();
            match Self::handle_open_tag(
                cx,
                tf,
                &mut node,
                &mut self.list_stack,
                &self.ul_markers,
                &self.ol_markers,
                &self.ol_separator,
            ) {
                (Some(_), _tws) => {
                    handle_custom_widget(cx, scope, tf, &self.doc, &mut node, &mut auto_id);
                }
                (None, tws) => {
                    trim = tws;
                }
            }
            let _ = Self::handle_close_tag(cx, tf, &mut node, &mut self.list_stack);
            Self::handle_text_node(cx, tf, &mut node, trim);
            node.walk();
        }
        self.text_flow.end(cx);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.body.set(v);
        let mut errors = Some(Vec::new());
        self.doc = parse_html(self.body.as_ref(), &mut errors, InternLiveId::No);
        self.seen_details.clear();
        self.summary_area_cache.clear();
        if errors.as_ref().unwrap().len() > 0 {
            log!("HTML parser returned errors {:?}", errors)
        }
        self.redraw(cx);
    }
}

fn handle_custom_widget(
    cx: &mut Cx2d,
    _scope: &mut Scope,
    tf: &mut TextFlow,
    doc: &HtmlDoc,
    node: &mut HtmlWalker,
    auto_id: &mut u64,
) {
    let id = if let Some(id) = node.find_attr_lc(live_id!(id)) {
        LiveId::from_str(id)
    } else {
        *auto_id += 1;
        LiveId(*auto_id)
    };

    let template = node.open_tag_nc().unwrap();
    let mut scope_with_attrs = Scope::with_props_index(doc, node.index);

    if let Some(item) = tf.item_with_scope(cx, &mut scope_with_attrs, id, template) {
        item.set_text(cx, node.find_text().unwrap_or(""));
        let mut draw_scope = Scope::with_data(tf);
        item.draw_all(cx, &mut draw_scope);
    }

    node.jump_to_close();
}

#[derive(Clone, Debug, Default)]
pub enum HtmlLinkAction {
    #[default]
    None,
    Clicked {
        url: String,
        key_modifiers: KeyModifiers,
    },
    SecondaryClicked {
        url: String,
        key_modifiers: KeyModifiers,
    },
}

#[derive(Script, Widget, Animator)]
pub struct HtmlLink {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,

    #[redraw]
    #[area]
    area: Area,

    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[rust]
    drawn_areas: SmallVec<[Area; 2]>,
    #[live(true)]
    grab_key_focus: bool,

    #[live]
    hovered: f32,
    #[live]
    pressed: f32,

    /// The default font color for the link when not hovered on or pressed.
    #[live]
    color: Option<Vec4f>,
    /// The font color used when the link is hovered on.
    #[live]
    hover_color: Option<Vec4f>,
    /// The font color used when the link is pressed.
    #[live]
    pressed_color: Option<Vec4f>,

    #[live]
    pub text: ArcStringMut,
    #[live]
    pub url: String,
}

impl ScriptHook for HtmlLink {
    fn on_after_new_scoped(&mut self, _vm: &mut ScriptVm, scope: &mut Scope) {
        // After an HtmlLink instance has been instantiated,
        // populate its struct fields from the `<a>` tag's attributes.
        if let Some(doc) = scope.props.get::<HtmlDoc>() {
            let mut walker = doc.new_walker_with_index(scope.index + 1);
            while let Some((lc, attr)) = walker.while_attr_lc() {
                match lc {
                    live_id!(href) => {
                        self.url = attr.into();
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

impl WidgetMatchEvent for HtmlLink {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions, _scope: &mut Scope) {
        // No actions needed for now
    }
}

impl Widget for HtmlLink {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            if let Some(tf) = scope.data.get_mut::<TextFlow>() {
                tf.redraw(cx);
            } else {
                self.drawn_areas.iter().for_each(|area| area.redraw(cx));
            }
        }

        self.widget_match_event(cx, event, scope);

        for area in self.drawn_areas.clone().into_iter() {
            match event.hits(cx, area) {
                Hit::FingerDown(fe) => {
                    if fe.is_primary_hit() {
                        if self.grab_key_focus {
                            cx.set_key_focus(self.area());
                        }
                        self.animator_play(cx, ids!(hover.pressed));
                    } else if fe.mouse_button().is_some_and(|mb| mb.is_secondary()) {
                        cx.widget_action(
                            self.widget_uid(),
                            HtmlLinkAction::SecondaryClicked {
                                url: self.url.clone(),
                                key_modifiers: fe.modifiers,
                            },
                        );
                    }
                }
                Hit::FingerHoverIn(_) => {
                    cx.set_cursor(MouseCursor::Hand);
                    self.animator_play(cx, ids!(hover.on));
                }
                Hit::FingerHoverOut(_) => {
                    self.animator_play(cx, ids!(hover.off));
                }
                Hit::FingerLongPress(_) => {
                    cx.widget_action(
                        self.widget_uid(),
                        HtmlLinkAction::SecondaryClicked {
                            url: self.url.clone(),
                            key_modifiers: Default::default(),
                        },
                    );
                }
                Hit::FingerUp(fu) => {
                    if fu.is_over {
                        cx.set_cursor(MouseCursor::Hand);
                        self.animator_play(cx, ids!(hover.on));
                    } else {
                        self.animator_play(cx, ids!(hover.off));
                    }

                    if fu.is_over && fu.is_primary_hit() && fu.was_tap() {
                        cx.widget_action(
                            self.widget_uid(),
                            HtmlLinkAction::Clicked {
                                url: self.url.clone(),
                                key_modifiers: fu.modifiers,
                            },
                        );
                    }
                }
                _ => (),
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let Some(tf) = scope.data.get_mut::<TextFlow>() else {
            return DrawStep::done();
        };

        tf.underline.push();
        tf.areas_tracker.push_tracker();
        let mut pushed_color = false;
        if self.hovered > 0.0 {
            if let Some(color) = self.hover_color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else if self.pressed > 0.0 {
            if let Some(color) = self.pressed_color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        } else {
            if let Some(color) = self.color {
                tf.font_colors.push(color);
                pushed_color = true;
            }
        }
        tf.draw_text(cx, self.text.as_ref());

        if pushed_color {
            tf.font_colors.pop();
        }
        tf.underline.pop();

        let (start, end) = tf.areas_tracker.pop_tracker();

        if self.drawn_areas.len() == end - start {
            for i in 0..end - start {
                self.drawn_areas[i] =
                    cx.update_area_refs(self.drawn_areas[i], tf.areas_tracker.areas[i + start]);
            }
        } else {
            self.drawn_areas = SmallVec::from(&tf.areas_tracker.areas[start..end]);
        }

        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.text.as_mut_empty().push_str(v);
        self.redraw(cx);
    }
}

impl HtmlRef {
    pub fn set_text(&mut self, cx: &mut Cx, v: &str) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.set_text(cx, v)
    }
}

impl HtmlLinkRef {
    pub fn set_url(&mut self, url: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.url = url.to_string();
        }
    }

    pub fn url(&self) -> Option<String> {
        if let Some(inner) = self.borrow() {
            Some(inner.url.clone())
        } else {
            None
        }
    }
}

/// The state of a single `<details>` element as tracked during a draw walk.
#[derive(Debug, Clone, Copy)]
struct DetailsLevel {
    /// Stable per-document-position id, also used as the LiveId for this
    /// details element's embedded FoldButton in the TextFlow item map.
    id: LiveId,
    /// Set at `<summary>` time to the current FoldButton animator state.
    /// Controls whether the body content between `</summary>` and
    /// `</details>` is drawn or skipped.
    is_open: bool,
}

/// The format and metadata of a list at a given nesting level.
#[derive(Debug)]
struct ListLevel {
    /// The kind of list, either ordered or unordered.
    list_kind: ListKind,
    /// The type of marker formatting for ordered lists,
    /// if overridden for this particular list level.
    numbering_type: Option<OrderedListType>,
    /// The number of list items encountered so far at this level of nesting.
    /// This is a 1-indexed value, so the default initial value should be 1.
    /// This is an integer because negative numbering values are supported.
    li_count: i32,
    /// The padding space inserted to the left of each list item,
    /// where the list marker is drawn.
    padding: f64,
}

/// List kinds: ordered (numbered) and unordered (bulleted).
#[derive(Debug)]
enum ListKind {
    Unordered,
    Ordered,
}

/// The type of marker used for ordered lists.
///
/// See the ["type" attribute docs](https://developer.mozilla.org/en-US/docs/Web/HTML/Element/ol#attributes)
/// for more info.
#[derive(Copy, Clone, Debug, Default)]
pub enum OrderedListType {
    #[default]
    /// Decimal integers: 1, 2, 3, 4, ...
    ///
    /// This *does* support negative integer values, e.g., -2, -1, 0, 1, 2 ...
    Numbers,
    /// Uppercase letters: A, B, C, D, ...
    UpperAlpha,
    /// Lowercase letters: a, b, c, d, ...
    LowerAlpha,
    /// Uppercase roman numerals: I, II, III, IV, ...
    UpperRoman,
    /// Lowercase roman numerals: i, ii, iii, iv, ...
    LowerRoman,
}

impl OrderedListType {
    /// Returns the marker for the given count and separator character.
    ///
    /// ## Notes on behavior
    /// * A negative or zero `count` will always return an integer number marker.
    /// * Currently, for `UpperApha` and `LowerAlpha`, a `count` higher than 25 will result in a wrong character.
    /// * Roman numerals >= 4000 will return an integer number marker.
    pub fn marker(&self, count: i32, separator: &str) -> String {
        let to_number = || format!("{count}{separator}");
        if count <= 0 {
            return to_number();
        }

        match self {
            OrderedListType::Numbers => to_number(),
            OrderedListType::UpperAlpha => {
                format!("{}{separator}", ('A' as u8 + count as u8 - 1) as char)
            }
            OrderedListType::LowerAlpha => {
                format!("{}{separator}", ('a' as u8 + count as u8 - 1) as char)
            }
            OrderedListType::UpperRoman => to_roman_numeral(count)
                .map(|m| format!("{}{separator}", m))
                .unwrap_or_else(to_number),
            OrderedListType::LowerRoman => to_roman_numeral(count)
                .map(|m| format!("{}{separator}", m.to_lowercase()))
                .unwrap_or_else(to_number),
        }
    }

    /// Returns an ordered list type based on the given HTML `type` attribute value string `s`.
    ///
    /// Returns `None` if an invalid value is given.
    pub fn from_type_attribute(s: &str) -> Option<Self> {
        match s {
            "a" => Some(OrderedListType::LowerAlpha),
            "A" => Some(OrderedListType::UpperAlpha),
            "i" => Some(OrderedListType::LowerRoman),
            "I" => Some(OrderedListType::UpperRoman),
            "1" => Some(OrderedListType::Numbers),
            _ => None,
        }
    }
}

/// Converts an integer into an uppercase roman numeral string.
///
/// Returns `None` if the input is not between 1 and 3999 inclusive.
///
/// This code was adapted from the [`roman` crate](https://crates.io/crates/roman).
pub fn to_roman_numeral(mut count: i32) -> Option<String> {
    const MAX: i32 = 3999;
    static NUMERALS: &[(i32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];

    if count <= 0 || count > MAX {
        return None;
    }
    let mut output = String::new();
    for &(value, s) in NUMERALS.iter() {
        while count >= value {
            count -= value;
            output.push_str(s);
        }
    }
    if count == 0 {
        Some(output)
    } else {
        None
    }
}

/// Returns the horizontal alignment (`Layout::align.x`) for a `<td>` / `<th>`
/// cell, based on an inline `style="text-align: ..."` declaration or the
/// legacy HTML `align` attribute. `style` wins when both are set, matching
/// CSS precedence over presentational attributes. Defaults to left (0.0)
/// when unspecified or unrecognized.
fn cell_align_x(node: &HtmlWalker) -> f64 {
    if let Some(style) = node.find_attr_lc(live_id!(style)) {
        for decl in style.split(';') {
            let Some((prop, value)) = decl.split_once(':') else {
                continue;
            };
            if prop.trim().eq_ignore_ascii_case("text-align") {
                if let Some(x) = align_keyword_to_x(value.trim()) {
                    return x;
                }
            }
        }
    }
    if let Some(align) = node.find_attr_lc(live_id!(align)) {
        if let Some(x) = align_keyword_to_x(align) {
            return x;
        }
    }
    0.0
}

fn align_keyword_to_x(keyword: &str) -> Option<f64> {
    match keyword.trim().to_ascii_lowercase().as_str() {
        "left" | "start" | "justify" => Some(0.0),
        "center" => Some(0.5),
        "right" | "end" => Some(1.0),
        _ => None,
    }
}
