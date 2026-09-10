use std::borrow::Cow;
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

/// The default text color of an [`HtmlLink`]: #0000EE, the classic browser link blue.
pub const HTML_LINK_COLOR: Vec4f = vec4(0.0, 0.0, 238.0 / 255.0, 1.0);
/// The default text color of an [`HtmlLink`] while pressed: #EE0000, the classic
/// browser active-link red.
pub const HTML_LINK_PRESSED_COLOR: Vec4f = vec4(238.0 / 255.0, 0.0, 0.0, 1.0);

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.HtmlLinkBase = #(HtmlLink::register_widget(vm))

    mod.widgets.HtmlBase = #(Html::register_widget(vm))

    mod.widgets.HtmlLink = set_type_default() do mod.widgets.HtmlLinkBase{
        width: Fit height: Fit
        align: Align{x: 0. y: 0.}

        color: #(HTML_LINK_COLOR)
        // Standard browsers don't recolor links on hover, so no hover_color here.
        pressed_color: #(HTML_LINK_PRESSED_COLOR)

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

    /// The elements this widget has actually opened, innermost last, for the
    /// draw currently in progress.
    ///
    /// A close tag only runs its handler when a matching open tag is on this
    /// stack. Without that, a stray `</td>` or `</li>` in a message reaches
    /// `cx.end_turtle()` with nothing to end and unbalances the frame.
    #[rust]
    open_elements: Vec<LiveId>,

    /// How many of each tag `open_elements` holds, so an unmatched close tag
    /// is rejected without scanning the stack. A message can carry both deep
    /// nesting and many stray close tags.
    #[rust]
    open_element_counts: HashMap<LiveId, u32>,

    /// The `<summary>` elements currently open, innermost last. `</summary>`
    /// pops a tracker that `<summary>` pushed, so a message containing only
    /// the close tag used to pop an empty stack; and each is tied to the
    /// `<details>` that owns it, so a `<details>` opened inside a summary
    /// cannot be mistaken for the owner.
    #[rust]
    open_summaries: Vec<OpenSummary>,

    /// Column counts already computed for a `<table>` at a given node index.
    /// Counting scans forward to the first row, so a document that is
    /// thousands of unclosed `<table>` tags would otherwise be quadratic.
    #[rust]
    table_columns_cache: HashMap<usize, usize>,

    /// The stack of currently-open `<details>` tags while traversing the
    /// document. Rebuilt on each draw.
    #[rust]
    details_stack: Vec<DetailsLevel>,

    /// IDs of `<details>` FoldButtons whose initial open/closed state has
    /// already been seeded from their HTML `open` attribute. Persists across
    /// redraws so user clicks aren't overwritten.
    #[rust]
    seen_details: HashSet<LiveId>,

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
    /// Moves from inside a collapsed `<details>` to where its body ends: its
    /// own `</details>` when it has one, otherwise the close tag of the
    /// enclosing element that ended it, or the end of the document. The
    /// parser resolved that end, so no scan is needed — a scan that counted
    /// every tag got void elements wrong, and one that matched only
    /// `details` got a `<details>` ended by an ancestor wrong.
    fn skip_details_body(node: &mut HtmlWalker<'_>, details_open: usize) {
        let at = node.at(details_open);
        node.index = at
            .close_index()
            .or(at.end_index())
            .unwrap_or(node.nodes.len());
    }

    /// Ends the innermost open `<summary>`: pops the bold run and the glyph
    /// tracker `<summary>` pushed, and lays the invisible click target over
    /// the summary line. Returns whether the enclosing `<details>` is
    /// collapsed, in which case the caller skips its body.
    ///
    /// Called for `</summary>`, and also for `</details>` and the end of the
    /// document when the `<summary>` was never closed — otherwise its bold
    /// and its tracker would leak into everything drawn after it.
    fn close_summary(&mut self, cx: &mut Cx2d, owner: Option<usize>) -> bool {
        self.text_flow.bold.pop();
        let (start, end) = self.text_flow.areas_tracker.pop_tracker();
        if let Some(dl) = owner.and_then(|i| self.details_stack.get(i)) {
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
                            let x1 = (b.pos.x + b.size.x).max(r.pos.x + r.size.x);
                            let y1 = (b.pos.y + b.size.y).max(r.pos.y + r.size.y);
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
        owner
            .and_then(|i| self.details_stack.get(i))
            .is_some_and(|dl| !dl.is_open)
    }

    /// Ends the innermost open `<details>`: anything still open inside it —
    /// elements, and a `<summary>` that never closed — ends first.
    fn pop_details_level(&mut self, cx: &mut Cx2d) {
        let Some(level) = self.details_stack.last() else {
            return;
        };
        let (depth, index) = (level.open_depth, self.details_stack.len() - 1);
        self.unwind_open_elements(cx, depth);
        while self.open_summaries.last().is_some_and(|s| s.owner == Some(index)) {
            let summary = self.open_summaries.pop();
            self.close_summary(cx, summary.and_then(|s| s.owner));
        }
        self.details_stack.pop();
        // Matching bottom margin (see `<details>` open handler).
        self.text_flow
            .new_line_collapsed_with_spacing(cx, self.details_margin_em());
    }

    /// After a collapsed `<details>`'s summary closes: the index to resume at
    /// so its body is not drawn. That is its own `</details>` when it has
    /// one, so that handler still runs; otherwise the element was ended by
    /// an enclosing element, its level is closed here, and drawing resumes
    /// at that enclosing close tag.
    fn skip_collapsed_body(&mut self, cx: &mut Cx2d, node: &HtmlWalker) -> Option<usize> {
        let open_index = self.details_stack.last()?.open_index;
        let mut probe = node.at(open_index);
        Self::skip_details_body(&mut probe, open_index);
        if node.at(open_index).close_index().is_none() {
            self.pop_details_level(cx);
        }
        Some(probe.index)
    }

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

    /// Number of cells in a table's first row, which sizes its columns. The
    /// first row ends at `</tr>`, at the next `<tr>`, or where the table
    /// ends — a row written without `</tr>` used to have the second row's
    /// cells counted too, halving every column. A nested table, and each
    /// cell's own content, is stepped over whole.
    fn count_table_columns(node: &HtmlWalker) -> usize {
        let end = node
            .close_index()
            .or(node.end_index())
            .unwrap_or(node.nodes.len());
        let mut count = 0;
        let mut in_row = false;
        let mut i = node.index + 1;
        while i < end {
            match &node.nodes[i] {
                HtmlNode::OpenTag { lc, .. } => {
                    let at = node.at(i);
                    if *lc == live_id!(table) {
                        i = at.end_index().unwrap_or(i + 1);
                        continue;
                    }
                    if *lc == live_id!(tr) {
                        if in_row {
                            break;
                        }
                        in_row = true;
                    } else if *lc == live_id!(td) || *lc == live_id!(th) {
                        in_row = true;
                        count += 1;
                        i = at.end_index().unwrap_or(i + 1);
                        continue;
                    }
                }
                HtmlNode::CloseTag { lc, .. } => {
                    if in_row
                        && (*lc == live_id!(tr) || *lc == live_id!(thead) || *lc == live_id!(tbody))
                    {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
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
        table_columns_cache: &mut HashMap<usize, usize>,
    ) -> Option<LiveId> {
        fn open_header_tag(cx: &mut Cx2d, tf: &mut TextFlow, scale: f64) {
            tf.bold.push();
            tf.push_size_abs_scale(scale);
            let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
            tf.new_line_collapsed_with_spacing(cx, fs * tf.heading_margin.top);
        }

        match node.open_tag_lc() {
            some_id!(h1) => open_header_tag(cx, tf, 2.0),
            some_id!(h2) => open_header_tag(cx, tf, 1.5),
            some_id!(h3) => open_header_tag(cx, tf, 1.17),
            some_id!(h4) => open_header_tag(cx, tf, 1.0),
            some_id!(h5) => open_header_tag(cx, tf, 0.83),
            some_id!(h6) => open_header_tag(cx, tf, 0.67),

            some_id!(p) => {
                let fs = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f64;
                tf.new_line_collapsed_with_spacing(cx, fs * tf.paragraph_margin.top);
            }
            some_id!(code) => {
                tf.push_size_rel_scale(tf.fixed_font_size_scale);
                tf.fixed.push();
                tf.inline_code.push();
            }
            some_id!(pre) => {
                tf.new_line_collapsed(cx);
                tf.fixed.push();
                tf.begin_code(cx);
            }
            some_id!(blockquote) => {
                tf.new_line_collapsed(cx);
                tf.begin_quote(cx);
            }
            some_id!(br) => {
                tf.new_line_with_wrap_spacing(cx);
            }
            some_id!(hr) | some_id!(sep) => {
                tf.new_line_collapsed(cx);
                tf.sep(cx);
                tf.new_line_collapsed(cx);
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
                list_stack.push(ListLevel {
                    list_kind: ListKind::Unordered,
                    numbering_type: None,
                    li_count: 1,
                    padding: 2.5,
                });
            }
            some_id!(ol) => {
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
                let indent_level = list_stack.len();
                let index = indent_level.saturating_sub(1);
                let marker_and_pad = list_stack.last_mut().map(|ll| {
                    // Borrowed for the common case: a fresh String per item
                    // per draw was most of a list's draw cost.
                    let marker: Cow<'_, str> = match ll.list_kind {
                        ListKind::Unordered => ul_markers
                            .get(index)
                            .map(|m| Cow::Borrowed(m.as_str()))
                            .unwrap_or(Cow::Borrowed(BULLET)),
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
                                .map(|ol_type| Cow::Owned(ol_type.marker(value, ol_separator)))
                                .unwrap_or(Cow::Borrowed("#"))
                        }
                    };
                    ll.li_count = ll.li_count.saturating_add(1);
                    (marker, ll.padding)
                });
                let (marker, pad) = marker_and_pad
                    .as_ref()
                    .map(|(m, p)| (m.as_ref(), *p))
                    .unwrap_or((BULLET, 2.5));

                tf.new_line_collapsed(cx);
                tf.begin_list_item(cx, marker, pad);
            }
            some_id!(table) => {
                tf.new_line_collapsed(cx);
                let col_count = *table_columns_cache
                    .entry(node.index)
                    .or_insert_with(|| Self::count_table_columns(node));
                tf.begin_table(cx, col_count);
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
            }
            some_id!(th) => {
                tf.table_row_is_header = true;
                tf.begin_table_cell(cx, cell_align_x(node));
                tf.bold.push();
            }
            some_id!(td) => {
                tf.begin_table_cell(cx, cell_align_x(node));
            }
            Some(x) => return Some(x),
            _ => (),
        }
        None
    }

    /// Drops one occurrence of `lc` from the open-element tally.
    fn release_open_element(counts: &mut HashMap<LiveId, u32>, lc: LiveId) {
        if let Some(n) = counts.get_mut(&lc) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                counts.remove(&lc);
            }
        }
    }

    /// Tags whose close handler changes `TextFlow` state, so their open tag
    /// has to be remembered and their close tag ignored when unmatched.
    fn element_is_tracked(lc: LiveId) -> bool {
        // Every tag `handle_close_tag` has an arm for, minus the void ones
        // (`<br>`, `<hr>`, `<sep>`), which never carry a close tag.
        const TRACKED: &[LiveId] = &[
            live_id!(h1), live_id!(h2), live_id!(h3),
            live_id!(h4), live_id!(h5), live_id!(h6),
            live_id!(b), live_id!(strong), live_id!(i), live_id!(em),
            live_id!(p), live_id!(blockquote), live_id!(code), live_id!(pre),
            live_id!(sub), live_id!(sup),
            live_id!(ul), live_id!(ol), live_id!(li),
            live_id!(u), live_id!(del), live_id!(s), live_id!(strike),
            live_id!(table), live_id!(thead), live_id!(tbody),
            live_id!(tr), live_id!(th), live_id!(td),
        ];
        TRACKED.contains(&lc)
    }

    /// Runs the close handler for everything above `depth` on the open stack,
    /// innermost first, and drops it.
    fn unwind_open_elements(&mut self, cx: &mut Cx2d, depth: usize) {
        while self.open_elements.len() > depth {
            let Some(lc) = self.open_elements.pop() else {
                break;
            };
            Self::release_open_element(&mut self.open_element_counts, lc);
            let _ = Self::handle_close_tag(cx, &mut self.text_flow, lc, &mut self.list_stack);
        }
    }

    /// Closes the outermost open element in `targets` found before reaching
    /// one in `scope`, and everything above it. With `top_only`, only the
    /// innermost open element is considered.
    fn close_implicitly(
        &mut self,
        cx: &mut Cx2d,
        targets: &[LiveId],
        scope: &[LiveId],
        top_only: bool,
    ) {
        let mut close_to = None;
        for (depth, open) in self.open_elements.iter().enumerate().rev() {
            if targets.contains(open) {
                close_to = Some(depth);
            } else if scope.contains(open) {
                break;
            }
            if top_only {
                break;
            }
        }
        if let Some(depth) = close_to {
            self.unwind_open_elements(cx, depth);
        }
    }

    /// What opening `tag` implicitly closes, following the tree builder's
    /// rules: a heading closes a heading it directly follows, `<li>` closes
    /// an open item up to the enclosing list, a cell closes an open cell up
    /// to its row, a row closes an open row and its cells, and any block
    /// element closes an open `<p>`. This is what makes the shorthand real
    /// documents use — `<li>a<li>b`, `<td>a<td>b`, `<p>a<p>b` — nest the
    /// way a browser reads it.
    fn apply_implicit_closes(&mut self, cx: &mut Cx2d, tag: LiveId) {
        const HEADINGS: &[LiveId] = &[
            live_id!(h1), live_id!(h2), live_id!(h3),
            live_id!(h4), live_id!(h5), live_id!(h6),
        ];
        const ITEM: &[LiveId] = &[live_id!(li)];
        const LISTS: &[LiveId] = &[live_id!(ul), live_id!(ol)];
        const CELLS: &[LiveId] = &[live_id!(td), live_id!(th)];
        const ROW_SCOPE: &[LiveId] = &[live_id!(tr), live_id!(table)];
        const ROW_AND_CELLS: &[LiveId] = &[live_id!(tr), live_id!(td), live_id!(th)];
        const TABLE_SCOPE: &[LiveId] = &[live_id!(table), live_id!(thead), live_id!(tbody)];
        const PARAGRAPH: &[LiveId] = &[live_id!(p)];
        const BUTTON_SCOPE: &[LiveId] = &[live_id!(table), live_id!(td), live_id!(th)];
        const CLOSES_PARAGRAPH: &[LiveId] = &[
            live_id!(h1), live_id!(h2), live_id!(h3), live_id!(h4), live_id!(h5), live_id!(h6),
            live_id!(p), live_id!(blockquote), live_id!(pre),
            live_id!(ul), live_id!(ol), live_id!(li), live_id!(table),
        ];
        // The tree builder closes a `<p>` in button scope first, and only
        // then pops a heading that is the current node.
        if CLOSES_PARAGRAPH.contains(&tag) {
            self.close_implicitly(cx, PARAGRAPH, BUTTON_SCOPE, false);
        }
        if HEADINGS.contains(&tag) {
            self.close_implicitly(cx, HEADINGS, &[], true);
        }
        if tag == live_id!(li) {
            self.close_implicitly(cx, ITEM, LISTS, false);
        } else if CELLS.contains(&tag) {
            self.close_implicitly(cx, CELLS, ROW_SCOPE, false);
        } else if tag == live_id!(tr) {
            self.close_implicitly(cx, ROW_AND_CELLS, TABLE_SCOPE, false);
        }
    }

    fn handle_close_tag(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        close_lc: LiveId,
        list_stack: &mut Vec<ListLevel>,
    ) -> Option<LiveId> {
        // Takes the id rather than reading it off the walker, so the caller can
        // also drive it for elements a document left open.
        match Some(close_lc) {
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
                tf.end_quote(cx);
            }
            some_id!(code) => {
                tf.inline_code.pop();
                tf.font_sizes.pop();
                tf.fixed.pop();
            }
            some_id!(pre) => {
                tf.fixed.pop();
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

    pub fn handle_text_node(cx: &mut Cx2d, tf: &mut TextFlow, node: &mut HtmlWalker) -> bool {
        if let Some(text) = node.text() {
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
                Hit::FingerUp(fu) if fu.is_over && fu.is_primary_hit() && fu.was_tap() => {
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
                let ids: SmallVec<[LiveId; 8]> = self.seen_details.iter().copied().collect();
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
        // The walker borrows the document for the whole draw and the handlers
        // below need `&mut self`, so the document lives in a local for the
        // duration and goes back at the end.
        let doc = std::mem::take(&mut self.doc);
        let mut node = doc.new_walker();
        self.details_stack.clear();
        self.summary_click_areas.clear();
        // These describe the draw in progress; a previous draw that ended
        // inside an unclosed element must not bleed into this one.
        self.list_stack.clear();
        self.open_elements.clear();
        self.open_element_counts.clear();
        self.open_summaries.clear();
        self.table_columns_cache.clear();
        while !node.done() {
            // Intercept <details> / <summary> open tags before the generic
            // handler, so <details> never falls through to handle_custom_widget
            // (which would jump_to_close and hide all content).
            if let Some(tag) = node.open_tag_lc() {
                if tag == live_id!(details) {
                    // Key by the node's stable doc index (a visit-order counter
                    // would renumber when a collapsed <details> hides nodes).
                    // Offset into the high bits to avoid colliding with the
                    // custom-widget ids (small ints) in the items map. The
                    // `id` attribute is deliberately not used: a document that
                    // repeats one would make two `<details>` share a fold state.
                    let details_id =
                        LiveId(0xd37a_115_0000_0000u64.wrapping_add(node.index as u64));
                    let initial_open = node.find_attr_lc(live_id!(open)).is_some();
                    self.details_stack.push(DetailsLevel {
                        id: details_id,
                        is_open: initial_open,
                        open_index: node.index,
                        open_depth: self.open_elements.len(),
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
                    if let Some(&DetailsLevel {
                        id: details_id,
                        is_open: initial_open,
                        ..
                    }) = self.details_stack.last()
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
                            let needs_seed = !self.seen_details.contains(&details_id);
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
                    self.open_summaries.push(OpenSummary {
                        owner: self.details_stack.len().checked_sub(1),
                        depth: self.open_elements.len(),
                    });
                    node.walk();
                    continue;
                }
            }

            // Intercept </summary> and </details> close tags.
            if let Some(close_tag) = node.close_tag_lc() {
                if close_tag == live_id!(summary) {
                    // A `</summary>` with no `<summary>` has no tracker to pop.
                    let Some(summary) = self.open_summaries.pop() else {
                        node.walk();
                        continue;
                    };
                    // Anything opened inside the summary ends with it — an
                    // `<li>` or a table cell, and any `<details>` nested in it.
                    self.unwind_open_elements(cx, summary.depth);
                    if let Some(owner) = summary.owner {
                        while self.details_stack.len() > owner + 1 {
                            self.pop_details_level(cx);
                        }
                    }
                    if self.close_summary(cx, summary.owner) {
                        if let Some(resume) = self.skip_collapsed_body(cx, &node) {
                            node.index = resume;
                            continue;
                        }
                    }
                    node.walk();
                    continue;
                }
                if close_tag == live_id!(details) {
                    // Only balance a `<details>` this draw opened.
                    if !self.details_stack.is_empty() {
                        self.pop_details_level(cx);
                    }
                    node.walk();
                    continue;
                }
            }

            // Regular tag/text handling for everything else.
            let open_lc = node.open_tag_lc();
            if let Some(open_lc) = open_lc {
                self.apply_implicit_closes(cx, open_lc);
            }

            let tf = &mut self.text_flow;
            match Self::handle_open_tag(
                cx,
                tf,
                &mut node,
                &mut self.list_stack,
                &self.ul_markers,
                &self.ol_markers,
                &self.ol_separator,
                &mut self.table_columns_cache,
            ) {
                Some(_) => {
                    node.index = handle_custom_widget(cx, scope, tf, &doc, &mut node);
                    continue;
                }
                None => {
                    if let Some(lc) = open_lc {
                        if Self::element_is_tracked(lc) {
                            self.open_elements.push(lc);
                            *self.open_element_counts.entry(lc).or_insert(0) += 1;
                        }
                    }
                }
            }

            // Run a close handler only for an element this draw actually
            // opened, unwinding anything left open inside it.
            if let Some(close_lc) = node.close_tag_lc() {
                if self.open_element_counts.get(&close_lc).is_some_and(|n| *n > 0) {
                    let depth = self
                        .open_elements
                        .iter()
                        .rposition(|t| *t == close_lc)
                        .unwrap_or(0);
                    self.unwind_open_elements(cx, depth);
                }
            }
            Self::handle_text_node(cx, &mut self.text_flow, &mut node);
            node.walk();
        }
        // Close anything the document left open, so `<ul><li>item` hands a
        // balanced turtle stack back to `TextFlow::end`, and a `<summary>`
        // that never closed doesn't leave its bold run and tracker behind.
        self.unwind_open_elements(cx, 0);
        while let Some(summary) = self.open_summaries.pop() {
            self.close_summary(cx, summary.owner);
        }
        self.details_stack.clear();
        self.text_flow.end(cx);
        self.doc = doc;
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        // Skip the parse entirely when the text is unchanged; callers commonly
        // re-populate widgets with identical content on every draw frame.
        if self.body.as_ref() == v {
            return;
        }
        self.body.set(v);
        let mut errors = Some(Vec::new());
        let new_doc = parse_html(self.body.as_ref(), &mut errors, InternLiveId::No);
        // Rebuild only when the content actually changed, mirroring
        // on_after_apply. Sub-widgets are keyed by position and capture their
        // node attributes at creation, so stale items must be dropped when the
        // doc changes; when it is unchanged, keeping them preserves
        // user-toggled <details> state across re-populates.
        if new_doc != self.doc {
            self.doc = new_doc;
            self.text_flow.clear_items();
            self.seen_details.clear();
            self.summary_area_cache.clear();
        }
        if errors.as_ref().unwrap().len() > 0 {
            log!("HTML parser returned errors {:?}", errors)
        }
        self.redraw(cx);
    }
}

/// Draws the sub-widget for the custom element the walker is on, and returns
/// the node index at which the caller should resume: past the element's own
/// close tag, or at the enclosing close tag that ended it, or past a void
/// element's attributes. The element's content is the widget's, so the
/// caller must not walk into it.
fn handle_custom_widget(
    cx: &mut Cx2d,
    _scope: &mut Scope,
    tf: &mut TextFlow,
    doc: &HtmlDoc,
    node: &mut HtmlWalker,
) -> usize {
    let open_index = node.index;
    let resume = node.end_index().unwrap_or(open_index + 1);
    let content_end = node.close_index().unwrap_or(resume);

    // A custom widget draws straight into the turtle rather than through
    // `TextFlow::draw_text`, so it has to honour the line budget itself; once
    // the widget is skipped, its inner text must not spill into the flow.
    if tf.is_content_truncated() {
        return resume;
    }
    let Some(template) = node.open_tag_nc() else {
        return resume;
    };

    // Key by the node's stable index in the parsed doc rather than a
    // visit-order counter: skip mode (a collapsed <details>) doesn't visit
    // hidden nodes, so a counter would renumber every widget after the
    // details on toggle, rebinding links and spans to the wrong nodes. The
    // `id` attribute is deliberately not used: a document that repeats one
    // would bind two elements to a single cached widget — and one `href`.
    let id = LiveId(open_index as u64);
    let mut scope_with_attrs = Scope::with_props_index(doc, open_index);

    // The label is every text run inside the element, up to wherever the
    // parser resolved its end — so `<li><a href=u>link</li>` keeps its
    // label, and a void element, which has no content, gets none.
    let label = element_text(doc, open_index, content_end);

    if let Some(item) = tf.item_with_scope(cx, &mut scope_with_attrs, id, template) {
        item.set_text(cx, &label);
        // A widget is walked atomically, so on the last allowed line it has to
        // be kept there rather than relocated onto a row the budget cannot pay
        // for; when it overruns that line it is cut at the edge with an
        // ellipsis. `end_inline_content` also charges the row the widget landed
        // on, which costs a line even when no text run follows to notice it.
        let hold = tf.begin_inline_content(cx);
        let mut draw_scope = Scope::with_data(tf);
        item.draw_all(cx, &mut draw_scope);
        tf.end_inline_content(cx, hold);
    }
    resume
}

/// The text between the open tag at `open` and its close tag at `close`:
/// borrowed when it is a single run, joined otherwise, so
/// `<a href="x"><b>Click</b> me</a>` labels its link "Click me" rather than
/// stopping at the first child.
fn element_text(doc: &HtmlDoc, open: usize, close: usize) -> Cow<'_, str> {
    let mut runs = doc.nodes[open..close].iter().filter_map(|n| match n {
        HtmlNode::Text { start, end, .. } if start != end => Some(&doc.decoded[*start..*end]),
        _ => None,
    });
    let Some(first) = runs.next() else {
        return Cow::Borrowed("");
    };
    let Some(second) = runs.next() else {
        return Cow::Borrowed(first);
    };
    let mut joined = String::with_capacity(first.len() + second.len());
    joined.push_str(first);
    joined.push_str(second);
    joined.extend(runs);
    Cow::Owned(joined)
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
        if let Event::ClearHover = event {
            self.animator_cut(cx, ids!(hover.off));
        }
        if self.animator_handle_event(cx, event).must_redraw() {
            if let Some(tf) = scope.data.get_mut::<TextFlow>() {
                tf.redraw(cx);
            } else {
                self.drawn_areas.iter().for_each(|area| area.redraw(cx));
            }
        }

        self.widget_match_event(cx, event, scope);

        for i in 0..self.drawn_areas.len() {
            let area = self.drawn_areas[i];
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
        // Pressed wins over hovered (a press also sets `hovered`), and both fall
        // back to the base color when unset, so a link with no hover_color keeps
        // its color while hovered instead of dropping to the ambient text color.
        let state_color = if self.pressed > 0.0 {
            self.pressed_color.or(self.color)
        } else if self.hovered > 0.0 {
            self.hover_color.or(self.color)
        } else {
            self.color
        };
        let mut pushed_color = false;
        if let Some(color) = state_color {
            tf.font_colors.push(color);
            pushed_color = true;
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
        if self.text.as_ref() == v { return }
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

impl HtmlLink {
    /// Sets the link's default (non-hovered) font color.
    /// `None` makes the link inherit the surrounding text's font color.
    ///
    /// Does nothing if the color is unchanged.
    pub fn set_color(&mut self, cx: &mut Cx, color: Option<Vec4f>) {
        if self.color == color {
            return;
        }
        self.color = color;
        // This widget is drawn by its parent TextFlow, so redraw the areas
        // it was actually drawn into.
        for area in self.drawn_areas.iter() {
            area.redraw(cx);
        }
    }
}

impl HtmlLinkRef {
    /// See [`HtmlLink::set_color()`].
    pub fn set_color(&self, cx: &mut Cx, color: Option<Vec4f>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_color(cx, color);
        }
    }

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
    /// Node index of the `<details>` open tag, so a collapsed body can be
    /// skipped to the element's resolved end.
    open_index: usize,
    /// `open_elements.len()` when the element opened; `</details>` unwinds
    /// back to it, ending anything left open in the body.
    open_depth: usize,
}

/// A `<summary>` that has been opened and not yet closed.
struct OpenSummary {
    /// Index into `details_stack` of the `<details>` it belongs to, if any.
    owner: Option<usize>,
    /// `open_elements.len()` when it opened; `</summary>` unwinds back to it.
    depth: usize,
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
    /// ## Notes on behavior
    /// * A negative or zero `count` will always return an integer number marker.
    /// * `UpperAlpha` and `LowerAlpha` continue past `z` as `aa`, `ab`, ...
    /// * Roman numerals >= 4000 will return an integer number marker.
    pub fn marker(&self, count: i32, separator: &str) -> String {
        let to_number = || format!("{count}{separator}");
        if count <= 0 {
            return to_number();
        }

        match self {
            OrderedListType::Numbers => to_number(),
            OrderedListType::UpperAlpha => {
                format!("{}{separator}", alphabetic_marker(count, b'A'))
            }
            OrderedListType::LowerAlpha => {
                format!("{}{separator}", alphabetic_marker(count, b'a'))
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
/// Numbers an alphabetic list item the way `list-style-type: lower-alpha`
/// does: `a`..`z`, then `aa`, `ab`, and so on.
///
/// `count` comes from the `start` and `value` attributes, so it is
/// attacker-controlled and must not be truncated into a byte.
fn alphabetic_marker(count: i32, base: u8) -> String {
    let mut remaining = count as i64;
    let mut letters = Vec::new();
    while remaining > 0 {
        let digit = ((remaining - 1) % 26) as u8;
        letters.push((base + digit) as char);
        remaining = (remaining - 1) / 26;
    }
    letters.iter().rev().collect()
}

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
    // Compared in place rather than lowercased into a fresh String, which ran
    // once per aligned cell on every draw.
    let keyword = keyword.trim();
    let eq = |s: &str| keyword.eq_ignore_ascii_case(s);
    if eq("left") || eq("start") || eq("justify") {
        Some(0.0)
    } else if eq("center") {
        Some(0.5)
    } else if eq("right") || eq("end") {
        Some(1.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_details(doc: &HtmlDoc) -> usize {
        doc.nodes
            .iter()
            .position(|node| matches!(node, HtmlNode::OpenTag { lc, .. } if *lc == live_id!(details)))
            .expect("fixture must contain a <details> tag")
    }

    fn summary_close(doc: &HtmlDoc) -> HtmlWalker<'_> {
        let mut node = doc.new_walker();
        while !node.done() && node.close_tag_lc() != Some(live_id!(summary)) {
            node.walk();
        }
        assert!(!node.done(), "fixture must contain a closing summary tag");
        node
    }

    #[test]
    fn collapsed_details_with_breaks_preserves_table_closures() {
        let doc = parse_html(
            "<table><tr><td><details><summary>View Bio</summary>\
             <br>Hidden biography<br><br><a href='profile'>Profile</a></details>\
             </td><td>Next cell</td></tr><tr><td>Next row</td></tr></table>\
             <p>After table</p>",
            &mut None,
            InternLiveId::No,
        );
        let mut node = summary_close(&doc);
        Html::skip_details_body(&mut node, first_details(&doc));
        assert_eq!(node.close_tag_lc(), Some(live_id!(details)));

        let remaining_closures: Vec<_> = node.nodes[node.index..].iter().filter_map(|node| {
            match node {
                HtmlNode::CloseTag { lc, .. } => Some(*lc),
                _ => None,
            }
        }).collect();
        assert_eq!(remaining_closures, vec![
            live_id!(details), live_id!(td), live_id!(td), live_id!(tr),
            live_id!(td), live_id!(tr), live_id!(table), live_id!(p),
        ]);
        let mut remaining_text = String::new();
        while !node.done() {
            if let Some(text) = node.text() {
                remaining_text.push_str(text);
            }
            node.walk();
        }
        assert_eq!(remaining_text, "Next cellNext rowAfter table");
    }

    #[test]
    fn collapsed_details_skips_nested_details_and_void_elements() {
        let doc = parse_html(
            "<details><summary>Outer</summary><br>\
             <details open><summary>Inner</summary><BR class='space'>\
             <b>Hidden</b></details>\
             <DETAILS><summary>Second</summary><img src='picture'><br /></DETAILS>\
             <hr>Outer hidden</details><p>After details</p>",
            &mut None,
            InternLiveId::No,
        );
        let outer_close = doc.nodes.iter().rposition(|node| {
            matches!(node, HtmlNode::CloseTag { lc, .. } if *lc == live_id!(details))
        }).unwrap();
        let mut node = summary_close(&doc);
        Html::skip_details_body(&mut node, first_details(&doc));
        assert_eq!(node.index, outer_close);
        assert_eq!(node.find_tag_text(live_id!(p)), Some("After details"));
    }

    #[test]
    fn collapsed_details_without_close_stops_at_document_end() {
        let doc = parse_html(
            "<details><summary>Outer</summary><br>Hidden<details>Inner</details>",
            &mut None,
            InternLiveId::No,
        );
        let mut node = summary_close(&doc);
        Html::skip_details_body(&mut node, first_details(&doc));
        assert!(node.done());
    }
}
