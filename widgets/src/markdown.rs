use crate::{
    link_label::LinkLabel, makepad_derive_widget::*, makepad_draw::*, text_flow::TextFlow,
    widget::*, widget_async::ScriptAsyncResult, WidgetMatchEvent,
};

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MarkdownLinkBase = #(MarkdownLink::register_widget(vm))

    mod.widgets.MarkdownBase = #(Markdown::register_widget(vm))

    mod.widgets.MarkdownLink = set_type_default() do mod.widgets.MarkdownLinkBase{
        width: Fit height: Fit
        align: Align{x: 0. y: 0.}

        label_walk: Walk{width: Fit height: Fit}

        draw_icon +: {
            hover: instance(0.0)
            pressed: instance(0.0)

            get_color: fn() {
                return mix(
                    mix(
                        theme.color_label_inner,
                        theme.color_label_inner_hover,
                        self.hover
                    ),
                    theme.color_label_inner_down,
                    self.pressed
                )
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {pressed: 0.0 hover: 0.0}
                        draw_icon: {pressed: 0.0 hover: 0.0}
                        draw_text: {pressed: 0.0 hover: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_bg: {pressed: 0.0 hover: snap(1.0)}
                        draw_icon: {pressed: 0.0 hover: snap(1.0)}
                        draw_text: {pressed: 0.0 hover: snap(1.0)}
                    }
                }

                pressed: AnimatorState{
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_bg: {pressed: snap(1.0) hover: 1.0}
                        draw_icon: {pressed: snap(1.0) hover: 1.0}
                        draw_text: {pressed: snap(1.0) hover: 1.0}
                    }
                }
            }
        }

        draw_bg +: {
            pressed: instance(0.0)
            hover: instance(0.0)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let offset_y = 1.0
                sdf.move_to(0. self.rect_size.y-offset_y)
                sdf.line_to(self.rect_size.x self.rect_size.y-offset_y)
                return sdf.stroke(mix(
                    theme.color_label_inner,
                    theme.color_label_inner_down,
                    self.pressed
                ), mix(0.0, 0.8, self.hover))
            }
        }

        draw_text +: {
            pressed: instance(0.0)
            hover: instance(0.0)

            color_hover: uniform(theme.color_label_inner_hover)
            color_pressed: uniform(theme.color_label_inner_down)

            color: theme.color_label_inner
            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
            get_color: fn() {
                return mix(
                    mix(
                        self.color,
                        self.color_hover,
                        self.hover
                    ),
                    self.color_pressed,
                    self.pressed
                )
            }
        }
    }

    mod.widgets.Markdown = set_type_default() do mod.widgets.MarkdownBase{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        padding: theme.mspace_1

        font_size: theme.font_size_p
        font_color: theme.color_label_inner

        paragraph_spacing: 16
        pre_code_spacing: 8
        inline_code_padding: theme.mspace_1
        inline_code_margin: theme.mspace_1
        heading_base_scale: 1.8

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
            padding: Inset{left: theme.space_3, right: theme.space_3, top: theme.space_2, bottom: 10}
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

        sep_walk: Walk{
            width: Fill height: 4.
            margin: theme.mspace_v_1
        }

        draw_table_bg +: {
            color: #x1f2937
        }

        draw_table_header_bg +: {
            color: #x334155
        }

        draw_table_line +: {
            color: #x475569
        }

        draw_block +: {
            line_color: theme.color_label_inner
            sep_color: theme.color_shadow
            quote_bg_color: theme.color_bg_highlight
            quote_fg_color: theme.color_label_inner
            code_color: theme.color_bg_highlight
            selection_color: theme.color_selection_focus
            space_1: uniform(theme.space_1)
            space_2: uniform(theme.space_2)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                match self.block_type {
                    FlowBlockType.Quote => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.quote_bg_color)
                        sdf.box(self.space_1 self.space_1 self.space_1 self.rect_size.y-self.space_2 1.5)
                        sdf.fill(self.quote_fg_color)
                        return sdf.result
                    }
                    FlowBlockType.Sep => {
                        sdf.box(0. 1. self.rect_size.x-1. self.rect_size.y-2. 2.)
                        sdf.fill(self.sep_color)
                        return sdf.result
                    }
                    FlowBlockType.Code => {
                        sdf.box(0. 0. self.rect_size.x self.rect_size.y 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.InlineCode => {
                        sdf.box(1. 1. self.rect_size.x-2. self.rect_size.y-2. 2.)
                        sdf.fill(self.code_color)
                        return sdf.result
                    }
                    FlowBlockType.Underline => {
                        sdf.box(0. self.rect_size.y-2. self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Strikethrough => {
                        sdf.box(0. self.rect_size.y * 0.45 self.rect_size.x 2.0 0.5)
                        sdf.fill(self.line_color)
                        return sdf.result
                    }
                    FlowBlockType.Selection => {
                        return vec4(self.selection_color.rgb * self.selection_color.a, self.selection_color.a)
                    }
                }
                return #f00
            }
        }

        link := mod.widgets.MarkdownLink{}
    }
}

/// The state of a list at a given nesting level.
struct ListState {
    // Current item number for ordered lists.
    current_number: u64,
    // Start number for ordered lists, None for unordered.
    start_number: Option<u64>,
}

#[derive(Script, Widget)]
pub struct Markdown {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    pub text_flow: TextFlow,
    #[live]
    body: ArcStringMut,
    #[live]
    paragraph_spacing: f64,
    #[live]
    pre_code_spacing: f64,
    #[live(false)]
    use_code_block_widget: bool,
    #[rust]
    in_code_block: bool,
    #[rust]
    code_block_string: String,
    #[rust]
    in_splash_block: bool,
    #[rust]
    splash_block_string: String,
    /// Set while reading the body of a ```mermaid fenced block. Mirrors
    /// `in_splash_block` / `in_code_block`. The accumulated source is
    /// dispatched to the `mermaid_block` template at CodeBlock end — the
    /// template is expected to contain a widget whose `set_text` accepts
    /// raw mermaid source (typically a `MermaidSvgView`).
    #[rust]
    in_mermaid_block: bool,
    #[rust]
    mermaid_block_string: String,
    #[live(false)]
    use_math_widget: bool,
    #[rust]
    auto_id: u64,
    #[live]
    heading_base_scale: f64,

    // --- Table rendering state ---
    // Table rendering is a two-pass process. During the first pass we buffer
    // every cell's text into `table_rows` (respecting in_table_head for the
    // header). During the second pass, triggered at End(Tag::Table), we
    // measure each column's width via the real font layouter and draw the
    // grid with DrawColor/DrawText primitives inside a single
    // `walk_turtle(Walk::fixed(W,H))` reserved region.
    /// Set to the link's destination URL while reading the body of a
    /// `[text](url)` construct. We buffer the link's display text between
    /// Start(Link) and End(Link) into `link_text`, then instantiate the
    /// `link` template with both href AND text at End(Link). The previous
    /// approach instantiated an empty LinkLabel at Start and let link text
    /// flow into the outer turtle as plain text — the net effect was a
    /// zero-width invisible LinkLabel followed by unstyled inline text
    /// with no click handler.
    #[rust]
    in_link: Option<String>,
    #[rust]
    link_text: String,

    #[rust]
    in_table: bool,
    #[rust]
    in_table_head: bool,
    #[rust]
    table_has_header: bool,
    #[rust]
    table_rows: Vec<Vec<String>>,
    #[rust]
    table_current_row: Vec<String>,
    #[rust]
    table_current_cell: String,

    /// Background fill for the table container (drawn behind the grid).
    #[live]
    draw_table_bg: DrawColor,
    /// Header-row background tint — drawn on top of the main bg only behind
    /// row 0 when `has_header` is true. Makes the header visually distinct
    /// even if the bold-font override is absent in the consuming app.
    #[live]
    draw_table_header_bg: DrawColor,
    /// Grid line color (borders + dividers between cells).
    #[live]
    draw_table_line: DrawColor,
}

impl Widget for Markdown {
    fn is_interactive(&self) -> bool {
        false
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(text) {
            let str_val = vm.bx.heap.new_string_from_str(self.body.as_ref());
            return ScriptAsyncResult::Return(str_val.into());
        }
        if method == live_id!(set_text) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    let new_text = vm.bx.heap.temp_string_with(|heap, out| {
                        heap.cast_to_string(value, out);
                        out.to_string()
                    });
                    vm.with_cx_mut(|cx| {
                        self.set_text(cx, &new_text);
                    });
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.auto_id = 0;

        // If code_block template is missing, try to inherit it from the type default.
        // This handles Splash eval where the Markdown is created with only `body`
        // but the type default (set_type_default) includes code_block and use_code_block_widget.
        if !self.text_flow.has_template(live_id!(code_block)) && !self.source.is_zero() {
            let source_obj = self.source.as_object();
            cx.with_vm(|vm| {
                if let Some(td) = vm.bx.heap.type_default_for_object(source_obj) {
                    vm.vec_with(td, |vm, vec| {
                        for kv in vec {
                            if let Some(id) = kv.key.as_id() {
                                if !self.text_flow.has_template(id) {
                                    if let Some(template_obj) = kv.value.as_object() {
                                        self.text_flow.register_template(id,
                                            vm.bx.heap.new_object_ref(template_obj));
                                    }
                                }
                            }
                        }
                    });
                }
            });
        }

        // If code_block template exists (from type default or explicit), enable it
        if !self.use_code_block_widget && self.text_flow.has_template(live_id!(code_block)) {
            self.use_code_block_widget = true;
        }

        // If use_code_block_widget is true but no code_block template registered,
        // fall back to default monospace rendering.
        if self.use_code_block_widget && !self.text_flow.has_template(live_id!(code_block)) {
            self.use_code_block_widget = false;
        }

        self.begin(cx, walk);
        self.process_markdown_doc(cx);
        self.end(cx);

        DrawStep::done()
    }

    fn text(&self) -> String {
        self.body.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.body.as_ref() != v {
            self.body.set(v);
            self.redraw(cx);
        }
    }
}

impl ScriptHook for Markdown {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Forward to TextFlow's ScriptHook (handles templates from apply value)
        self.text_flow.on_after_apply(vm, apply, scope, value);

        // Also register templates from the apply value's vec (for compiled path)
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.text_flow.apply_template(vm, apply, scope, id, template_obj);
                            }
                        }
                    }
                });
            }
        }
    }
}

impl Markdown {

    fn process_markdown_doc(&mut self, cx: &mut Cx2d) {
        let tf = &mut self.text_flow;
        // Track state for nested formatting
        let mut list_stack: Vec<ListState> = Vec::new();
        let mut is_first_block = true;

        let parser = Parser::new_ext(
            self.body.as_ref(),
            Options::ENABLE_TABLES | Options::ENABLE_MATH,
        );

        for event in parser.into_iter() {
            match event {
                MdEvent::Start(Tag::Heading { level, .. }) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    let heading_base = self.heading_base_scale;
                    let scale = match level {
                        HeadingLevel::H1 => heading_base,
                        HeadingLevel::H2 => heading_base * 0.75,
                        HeadingLevel::H3 => heading_base * 0.58,
                        HeadingLevel::H4 => heading_base * 0.5,
                        HeadingLevel::H5 => heading_base * 0.42,
                        HeadingLevel::H6 => heading_base * 0.33,
                    };
                    tf.push_size_abs_scale(scale);
                    tf.bold.push();
                }
                MdEvent::End(TagEnd::Heading(_level)) => {
                    tf.bold.pop();
                    tf.font_sizes.pop();
                    tf.new_line_collapsed(cx);
                }
                MdEvent::Start(Tag::Paragraph) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                }
                MdEvent::End(TagEnd::Paragraph) => {
                    // No special handling needed, turtle position is managed by content/following blocks
                }
                MdEvent::Start(Tag::BlockQuote(_)) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    tf.begin_quote(cx);
                }
                MdEvent::End(TagEnd::BlockQuote(_quote_kind)) => {
                    tf.end_quote(cx);
                }
                MdEvent::Start(Tag::List(first_number)) => {
                    list_stack.push(ListState {
                        start_number: first_number,
                        current_number: first_number.unwrap_or(1),
                    });
                }
                MdEvent::End(TagEnd::List(_is_ordered)) => {
                    list_stack.pop();
                }
                MdEvent::Start(Tag::Item) => {
                    if !is_first_block {
                        tf.new_line_collapsed(cx);
                    }
                    is_first_block = false;
                    let marker = if let Some(state) = list_stack.last_mut() {
                        if state.start_number.is_some() {
                            // Ordered list - use and increment the counter
                            let num = state.current_number;
                            state.current_number += 1;
                            format!("{}.", num)
                        } else {
                            // Unordered list - use bullet
                            "•".to_string()
                        }
                    } else {
                        "•".to_string()
                    };
                    tf.begin_list_item(cx, &marker, 2.5);
                }
                MdEvent::End(TagEnd::Item) => {
                    tf.end_list_item(cx);
                }
                MdEvent::Start(Tag::Emphasis) => {
                    tf.italic.push();
                }
                MdEvent::End(TagEnd::Emphasis) => {
                    tf.italic.pop();
                }
                MdEvent::Start(Tag::Strong) => {
                    tf.bold.push();
                }
                MdEvent::End(TagEnd::Strong) => {
                    tf.bold.pop();
                }
                MdEvent::Start(Tag::Strikethrough) => {
                    tf.underline.push();
                }
                MdEvent::End(TagEnd::Strikethrough) => {
                    tf.underline.pop();
                }
                MdEvent::Start(Tag::Link { dest_url, .. }) => {
                    // Inside a table, links flatten to plain text in the
                    // cell buffer (no widget instancing, no click). Outside
                    // a table, buffer the link's text now; instantiate the
                    // LinkLabel at End(Link) with both href AND set_text.
                    if !self.in_table {
                        self.in_link = Some(dest_url.into_string());
                        self.link_text.clear();
                    }
                }
                MdEvent::End(TagEnd::Link) => {
                    if let Some(href) = self.in_link.take() {
                        let text = std::mem::take(&mut self.link_text);
                        if !text.is_empty() {
                            self.auto_id += 1;
                            let item = tf.item(cx, LiveId(self.auto_id), live_id!(link));
                            let link = item.as_markdown_link();
                            link.set_href(&href);
                            item.set_text(cx, &text);
                            item.draw_all_unscoped(cx);
                        }
                    }
                }
                MdEvent::Start(Tag::Image {
                    dest_url, title, ..
                }) => {
                    // Images require async URL fetch + decode + inline
                    // DrawImage placement, which is substantial work we
                    // haven't done yet. For now render a compact inline
                    // placeholder so the image's presence is at least
                    // visible. Alt text (pulldown's "title" field carries
                    // this in GFM) is included when present.
                    tf.draw_text(cx, "🖼 ");
                    let label = if !title.is_empty() {
                        title.as_ref()
                    } else {
                        dest_url.as_ref()
                    };
                    tf.draw_text(cx, label);
                }
                MdEvent::Start(Tag::CodeBlock(kind)) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.pre_code_spacing);
                    }
                    is_first_block = false;
                    // Two fenced-block language hooks:
                    //   ```runsplash  → dispatch to `splash_block` template
                    //   ```mermaid    → dispatch to `mermaid_block` template
                    // Any other language falls through to the generic
                    // `code_block` template (or inline styling if that
                    // template is not registered).
                    let lang = if let CodeBlockKind::Fenced(l) = &kind {
                        Some(l.as_ref())
                    } else {
                        None
                    };
                    let has_mermaid_tpl = tf.has_template(live_id!(mermaid_block));
                    if lang == Some("runsplash") {
                        self.in_splash_block = true;
                        self.splash_block_string.clear();
                    } else if lang == Some("mermaid") && has_mermaid_tpl {
                        self.in_mermaid_block = true;
                        self.mermaid_block_string.clear();
                    } else if self.use_code_block_widget {
                        self.in_code_block = true;
                        self.code_block_string.clear();
                    } else {
                        const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                        tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                        tf.combine_spaces.push(false);
                        tf.fixed.push();
                        tf.begin_code(cx);
                    }
                }
                MdEvent::End(TagEnd::CodeBlock) => {
                    if self.in_splash_block {
                        self.in_splash_block = false;
                        let entry_id = tf.new_counted_id();
                        let sbs = &self.splash_block_string;

                        // Draw the splash block using the $splash_block template
                        tf.item_with(cx, entry_id, id!(splash_block), |cx, item, _tf| {
                            item.widget(cx, ids!(splash_view)).set_text(cx, sbs);
                            item.draw_all_unscoped(cx);
                        });
                    } else if self.in_mermaid_block {
                        self.in_mermaid_block = false;
                        let entry_id = tf.new_counted_id();
                        let mbs = self.mermaid_block_string.clone();
                        // Dispatch the raw mermaid source to the template's
                        // `mermaid_view` widget. The template provider
                        // (e.g. aichat/MermaidSvgView) implements
                        // `Widget::set_text` to render source → SVG in place.
                        tf.item_with(cx, entry_id, id!(mermaid_block), |cx, item, _tf| {
                            item.widget(cx, ids!(mermaid_view)).set_text(cx, &mbs);
                            item.draw_all_unscoped(cx);
                        });
                    } else if self.in_code_block {
                        self.in_code_block = false;
                        let entry_id = tf.new_counted_id();
                        let cbs = &self.code_block_string;

                        // Draw the code block and capture the CodeView widget ref
                        let mut code_view_ref = WidgetRef::empty();
                        tf.item_with(cx, entry_id, id!(code_block), |cx, item, _tf| {
                            item.widget(cx, ids!(code_view)).set_text(cx, cbs);
                            item.draw_all_unscoped(cx);
                            code_view_ref = item.widget(cx, ids!(code_view));
                        });

                        // Register the code view widget for cross-child selection
                        // (its area will be queried at event time, not draw time)
                        tf.push_widget_text_for_selection(code_view_ref, &self.code_block_string);
                    } else {
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.combine_spaces.pop();
                        tf.end_code(cx);
                    }
                }
                // Inline code
                MdEvent::Code(text) => {
                    if self.in_table {
                        // v1 cells accept plain text only — collapse inline
                        // code runs into the cell's buffer without the
                        // fixed-font styling, so row geometry stays clean.
                        self.table_current_cell.push_str(&text);
                    } else {
                        const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                        tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                        tf.fixed.push();
                        tf.inline_code.push();
                        tf.draw_text(cx, &text);
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.inline_code.pop();
                    }
                }
                // Inline math ($...$)
                MdEvent::InlineMath(text) => {
                    // Inside a table we buffer the raw math source into the
                    // cell as plain text — MathView is its own sub-widget and
                    // firing it during the buffering phase would draw live
                    // into the parent turtle, corrupting the delayed grid.
                    if self.in_table {
                        self.table_current_cell.push_str(&text);
                    } else if self.use_math_widget {
                        let entry_id = tf.new_counted_id();
                        tf.item_with(cx, entry_id, live_id!(inline_math), |cx, item, _tf| {
                            item.set_text(cx, &text);
                            item.draw_all_unscoped(cx);
                        });
                    } else {
                        // Fallback: render as inline code style
                        const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                        tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                        tf.fixed.push();
                        tf.inline_code.push();
                        tf.draw_text(cx, &text);
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.inline_code.pop();
                    }
                }
                // Display math ($$...$$)
                MdEvent::DisplayMath(text) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;

                    if self.use_math_widget {
                        let entry_id = tf.new_counted_id();
                        tf.item_with(cx, entry_id, live_id!(display_math), |cx, item, _tf| {
                            item.set_text(cx, &text);
                            item.draw_all_unscoped(cx);
                        });
                    } else {
                        // Fallback: render as code block style
                        tf.begin_code(cx);
                        tf.fixed.push();
                        tf.draw_text(cx, &text);
                        tf.fixed.pop();
                        tf.end_code(cx);
                    }
                }
                MdEvent::Text(text) => {
                    if self.in_link.is_some() {
                        self.link_text.push_str(&text);
                    } else if self.in_table {
                        self.table_current_cell.push_str(&text);
                    } else if self.in_splash_block {
                        self.splash_block_string.push_str(&text);
                    } else if self.in_mermaid_block {
                        self.mermaid_block_string.push_str(&text);
                    } else if self.in_code_block {
                        self.code_block_string.push_str(&text);
                    } else {
                        tf.draw_text(cx, &text.trim_end_matches("\n"));
                    }
                }
                MdEvent::SoftBreak => {
                    if self.in_table {
                        self.table_current_cell.push(' ');
                    } else if self.in_splash_block {
                        self.splash_block_string.push('\n');
                    } else if self.in_mermaid_block {
                        self.mermaid_block_string.push('\n');
                    } else if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        tf.draw_text(cx, " ");
                    }
                }
                MdEvent::HardBreak => {
                    if self.in_table {
                        self.table_current_cell.push(' ');
                    } else if self.in_splash_block {
                        self.splash_block_string.push('\n');
                    } else if self.in_mermaid_block {
                        self.mermaid_block_string.push('\n');
                    } else if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        tf.new_line_collapsed(cx);
                    }
                }
                MdEvent::Rule => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    tf.sep(cx);
                    tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                }
                MdEvent::TaskListMarker(_) => {
                    // TODO: Implement task list markers
                }
                // Tables use a two-pass approach: buffer all cell text first,
                // then measure + draw the grid in End(Tag::Table) via the
                // Markdown::draw_table associated fn. See struct field docs
                // on `table_rows`.
                MdEvent::Start(Tag::Table(_alignments)) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    self.in_table = true;
                    self.table_has_header = false;
                    self.table_rows.clear();
                    self.table_current_row.clear();
                    self.table_current_cell.clear();
                }
                MdEvent::End(TagEnd::Table) => {
                    // Drive the grid draw via an associated fn so the
                    // &mut self.text_flow loan held by `tf` stays disjoint
                    // from the other fields we need to touch here.
                    Self::draw_table(
                        cx,
                        tf,
                        &mut self.draw_table_bg,
                        &mut self.draw_table_header_bg,
                        &mut self.draw_table_line,
                        &self.table_rows,
                        self.table_has_header,
                    );
                    self.in_table = false;
                    self.table_rows.clear();
                    self.table_current_row.clear();
                    self.table_current_cell.clear();
                    tf.first_thing_on_a_line = true;
                }
                MdEvent::Start(Tag::TableHead) => {
                    self.in_table_head = true;
                    self.table_has_header = true;
                    self.table_current_row.clear();
                }
                MdEvent::End(TagEnd::TableHead) => {
                    self.table_rows.push(std::mem::take(&mut self.table_current_row));
                    self.in_table_head = false;
                }
                MdEvent::Start(Tag::TableRow) => {
                    self.table_current_row.clear();
                }
                MdEvent::End(TagEnd::TableRow) => {
                    self.table_rows.push(std::mem::take(&mut self.table_current_row));
                }
                MdEvent::Start(Tag::TableCell) => {
                    self.table_current_cell.clear();
                }
                MdEvent::End(TagEnd::TableCell) => {
                    self.table_current_row.push(std::mem::take(&mut self.table_current_cell));
                }
                _ => {} // Unimplemented or unnecessary events
            }
        }

        // Streaming partial-render: if the parser reached EOF while still
        // inside an unclosed table, flush whatever we've collected and
        // draw it now. Without this, the table stays invisible across
        // every streaming chunk until the closing `|` row arrives —
        // producing a "whole table pops in at the end" UX.
        if self.in_table {
            if !self.table_current_cell.is_empty() {
                self.table_current_row.push(std::mem::take(&mut self.table_current_cell));
            }
            if !self.table_current_row.is_empty() {
                self.table_rows.push(std::mem::take(&mut self.table_current_row));
            }
            if !self.table_rows.is_empty() {
                let tf = &mut self.text_flow;
                Self::draw_table(
                    cx,
                    tf,
                    &mut self.draw_table_bg,
                    &mut self.draw_table_header_bg,
                    &mut self.draw_table_line,
                    &self.table_rows,
                    self.table_has_header,
                );
            }
            self.in_table = false;
            self.table_rows.clear();
        }
    }

    /// Draws the collected table grid at the current turtle position.
    ///
    /// Takes disjoint `&mut` borrows so the caller can keep its outer
    /// `&mut self.text_flow` loan live. Responsibilities:
    ///   1. Measure each column's max content width via the font layouter.
    ///   2. Reserve a `Walk::fixed(W, H)` rectangle in the parent turtle.
    ///   3. Paint a background, then cell text, then grid lines (in that
    ///      order so lines render on top of both).
    fn draw_table(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        draw_bg: &mut DrawColor,
        draw_header_bg: &mut DrawColor,
        draw_line: &mut DrawColor,
        rows: &[Vec<String>],
        has_header: bool,
    ) {
        if rows.is_empty() || rows[0].is_empty() {
            return;
        }

        // Layout constants. Tune in the DSL eventually; these match the
        // visual target spec (~8px horizontal, 4-6px vertical padding).
        const CELL_PAD_H: f64 = 8.0;
        const CELL_PAD_V: f64 = 5.0;
        const MAX_COL_W: f64 = 400.0;
        const LINE_W: f64 = 1.0;
        const LINE_HEIGHT_MULT: f64 = 1.4;

        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if ncols == 0 {
            return;
        }

        let font_size = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f32;

        // --- Pass 1: measure column widths ---
        //
        // `DrawText::layout` returns an Rc<LaidoutText> with size_in_lpxs.
        // We swap in the bold style for header cells to get accurate widths
        // when the header is rendered bold.
        let mut col_widths: Vec<f64> = vec![0.0; ncols];
        let normal_style = tf.text_style_normal.clone();
        let bold_style = tf.text_style_bold.clone();
        for (r, row) in rows.iter().enumerate() {
            let is_header_row = has_header && r == 0;
            let style = if is_header_row { &bold_style } else { &normal_style };
            tf.draw_text.text_style = style.clone();
            tf.draw_text.text_style.font_size = font_size;
            for (c, cell) in row.iter().enumerate() {
                if c >= ncols { break; }
                if cell.is_empty() { continue; }
                let laid = tf.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), cell);
                let w = laid.size_in_lpxs.width as f64;
                if w > col_widths[c] {
                    col_widths[c] = w.min(MAX_COL_W);
                }
            }
        }
        // Columns with no content get a minimum so dividers still render.
        for w in col_widths.iter_mut() {
            if *w < font_size as f64 { *w = font_size as f64; }
        }

        // Row height is derived from font size (single-line cells in v1).
        let row_h = (font_size as f64 * LINE_HEIGHT_MULT) + CELL_PAD_V * 2.0;
        let col_total_w: f64 = col_widths.iter().sum::<f64>() + (CELL_PAD_H * 2.0) * ncols as f64;
        let total_w = col_total_w + LINE_W; // +1 for trailing right border
        let total_h = row_h * rows.len() as f64 + LINE_W;

        // --- Pass 2: reserve space and draw ---
        let rect = cx.walk_turtle(Walk::fixed(total_w, total_h));
        let ox = rect.pos.x;
        let oy = rect.pos.y;

        // Background fill first.
        draw_bg.draw_abs(cx, Rect { pos: dvec2(ox, oy), size: dvec2(total_w, total_h) });

        // Header-row bg tint overlaid on main bg so the header is visually
        // distinct even when the bold-font override is missing / subtle.
        if has_header {
            draw_header_bg.draw_abs(cx, Rect { pos: dvec2(ox, oy), size: dvec2(total_w, row_h) });
        }

        // Cell text. Per-cell: set the font style + color + size, then
        // draw_abs at (origin + x-pad, origin + y-pad).
        let mut y = oy;
        for (r, row) in rows.iter().enumerate() {
            let is_header_row = has_header && r == 0;
            let style = if is_header_row { &bold_style } else { &normal_style };
            tf.draw_text.text_style = style.clone();
            tf.draw_text.text_style.font_size = font_size;
            tf.draw_text.color = tf.font_color;
            tf.draw_text.temp_y_shift = style.top_drop;

            let mut x = ox;
            for c in 0..ncols {
                let col_w = col_widths[c] + CELL_PAD_H * 2.0;
                if let Some(cell) = row.get(c) {
                    if !cell.is_empty() {
                        let text_y = y + CELL_PAD_V;
                        let text_x = x + CELL_PAD_H;
                        tf.draw_text.draw_abs(cx, dvec2(text_x, text_y), cell);
                    }
                }
                x += col_w;
            }
            y += row_h;
        }

        // Grid lines last (draw on top). Outer border.
        let border = draw_line;
        // Top
        border.draw_abs(cx, Rect { pos: dvec2(ox, oy), size: dvec2(total_w, LINE_W) });
        // Bottom
        border.draw_abs(cx, Rect { pos: dvec2(ox, oy + total_h - LINE_W), size: dvec2(total_w, LINE_W) });
        // Left
        border.draw_abs(cx, Rect { pos: dvec2(ox, oy), size: dvec2(LINE_W, total_h) });
        // Right
        border.draw_abs(cx, Rect { pos: dvec2(ox + total_w - LINE_W, oy), size: dvec2(LINE_W, total_h) });

        // Horizontal separator below each row (between rows and after header).
        let mut y = oy + row_h;
        for _ in 1..rows.len() {
            border.draw_abs(cx, Rect { pos: dvec2(ox, y), size: dvec2(total_w, LINE_W) });
            y += row_h;
        }

        // Vertical separators between each column.
        let mut x = ox;
        for c in 0..ncols.saturating_sub(1) {
            x += col_widths[c] + CELL_PAD_H * 2.0;
            border.draw_abs(cx, Rect { pos: dvec2(x, oy), size: dvec2(LINE_W, total_h) });
        }
    }
}

impl MarkdownRef {
    pub fn set_text(&mut self, cx: &mut Cx, v: &str) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.set_text(cx, v)
    }

    /// Start streaming text animation with fade-in effect.
    pub fn start_streaming_animation(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.text_flow.start_streaming_animation();
        }
    }

    /// Reset and start streaming animation (for reused widgets).
    pub fn reset_streaming_animation(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.text_flow.reset_streaming_animation();
        }
    }

    /// Stop streaming animation (fade will complete naturally).
    pub fn stop_streaming_animation(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.text_flow.stop_streaming_animation();
        }
    }

    /// Check if streaming animation is completely done.
    pub fn is_streaming_animation_done(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.text_flow.is_streaming_animation_done()
        } else {
            true
        }
    }

    /// Reset all streaming animations (text fade).
    pub fn reset_all_streaming_animations(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.text_flow.reset_all_streaming_animations();
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
struct MarkdownLink {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    link: LinkLabel,
    #[live]
    href: String,
}

impl WidgetMatchEvent for MarkdownLink {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        if let Some(modifiers) = self.link.clicked_modifiers(actions) {
            cx.widget_action(
                self.widget_uid(),
                MarkdownAction::LinkNavigated {
                    url: self.href.clone(),
                    modifiers,
                },
            );
        }
    }
}

impl Widget for MarkdownLink {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.link.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope)
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.link.draw_walk(cx, scope, walk)
    }

    fn text(&self) -> String {
        self.link.text()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.link.set_text(cx, v);
    }
}

impl MarkdownLinkRef {
    pub fn set_href(&self, v: &str) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.href = v.to_string();
    }
}

#[derive(Clone, Debug, Default)]
pub enum MarkdownAction {
    #[default]
    None,
    /// Emitted when a `[text](url)` link is clicked. The app decides what
    /// to do with it (e.g., open the URL only when `modifiers.logo` is set
    /// to avoid conflicting with drag-selection on the Markdown widget).
    LinkNavigated {
        url: String,
        modifiers: makepad_platform::KeyModifiers,
    },
}
