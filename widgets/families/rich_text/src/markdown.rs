use crate::{
    image::{ImageRef, ImageWidgetRefExt},
    image_cache::{looks_like_svg, AsyncImageLoad},
    link_label::LinkLabel,
    makepad_derive_widget::*,
    makepad_draw::*,
    text_flow::TextFlow,
    widget::*,
    WidgetMatchEvent,
};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd,
};

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
        // Restated from TextFlow rather than inherited. These reach TextFlow
        // through a Rust `#[deref]`, not through the prototype chain, so on a
        // reload -- and a theme switch is a reload -- any of them this block
        // does not name is reset to its FIELD TYPE's default instead of to
        // what TextFlow's own block says. The `Layout` default flows Right,
        // which laid every table row side by side: the header took the whole
        // width, each body row was left zero wide, and a table drew its box
        // and its header and nothing else. `heading_margin` and
        // `paragraph_margin` went to zero the same way.
        table_walk: Walk{width: Fill, height: Fit}
        table_layout: Layout{flow: Flow.Down}
        table_row_walk: Walk{width: Fill, height: Fit}
        table_row_layout: Layout{flow: Flow.Right}
        table_cell_layout: Layout{
            flow: Flow.Right{wrap: true}
            padding: Inset{left: 6, right: 6, top: 4, bottom: 4}
        }
        heading_margin: Inset{top: 1.0, bottom: 0.1}
        paragraph_margin: Inset{top: 0.33, bottom: 0.33}

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

        link := mod.widgets.MarkdownLink{}
        /** An inline `![alt](src)` image, sized by the Markdown to the
         * picture's natural size, scaled down to the available width. */
        inline_image := mod.widgets.Image{width: Fit height: Fit}
    }
}

/// The state of a list at a given nesting level.
struct ListState {
    // Current item number for ordered lists.
    current_number: u64,
    // Start number for ordered lists, None for unordered.
    start_number: Option<u64>,
}

#[derive(Script, ScriptHook, Widget)]
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
    #[live(false)]
    use_math_widget: bool,
    #[rust]
    auto_id: u64,
    #[live]
    heading_base_scale: f64,
    /// Fetch `http(s)://` images over the network. Off by default, so a
    /// Markdown body from an untrusted source makes no requests; local
    /// paths, `file://` and `data:` images always load.
    #[live(false)]
    load_http_images: bool,
    /// Draw `[text](url)` links as text in the flow, in `link_color`, the
    /// hovered one (`set_hovered_link`) underlined, instead of link widgets.
    /// The host hit-tests them through the selection API (the flow takes no
    /// presses for them), so a drag over a link still selects.
    #[live(false)]
    inline_links: bool,
    #[live]
    link_color: Vec4f,
    #[rust]
    hovered_link: Option<String>,
    /// Each inline image widget's source and the image-cache key its load
    /// completes under (`None` when it loaded synchronously or not at all),
    /// so a redraw does not request the same source again.
    #[rust]
    image_requests: HashMap<WidgetUid, (String, Option<PathBuf>)>,
    /// The inline image widgets drawn this pass; requests of the others are
    /// dropped at the end of the draw.
    #[rust]
    images_drawn: Vec<WidgetUid>,
}

impl Widget for Markdown {
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Document;
        makepad_platform::a11y::set_string(&mut node.name, &self.text());
    }

    fn is_interactive(&self) -> bool {
        false
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
        // An inline image's decode landed (its widget took the texture while
        // handling the same actions): lay the text out again around it.
        if let Event::Actions(actions) = event {
            let image_loaded = actions.iter().any(|action| {
                action.downcast_ref::<AsyncImageLoad>().is_some_and(|load| {
                    self.image_requests
                        .values()
                        .any(|(_, key)| key.as_deref() == Some(load.image_path.as_path()))
                })
            });
            if image_loaded {
                self.redraw(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.auto_id = 0;
        self.images_drawn.clear();

        self.begin(cx, walk);
        self.process_markdown_doc(cx);
        self.end(cx);

        let drawn = &self.images_drawn;
        self.image_requests.retain(|uid, _| drawn.contains(uid));

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

/// One run the flow drew while walking markdown events, mapped back to the
/// source the events were parsed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownPiece {
    /// Byte range in the source.
    pub src: Range<usize>,
    /// Byte range in the flow's text.
    pub flat: Range<usize>,
    /// The flow's text spells `src` byte for byte. Otherwise the run stands
    /// for `src` as a whole (an entity, a line break, an image).
    pub identity: bool,
}

/// Colours the text of a code block by its language.
pub trait CodeHighlight {
    /// Appends `(byte range in text, colour)` spans for `text` written in
    /// `language`, ascending and disjoint. Text in no span keeps the code
    /// colour.
    fn highlight(&self, language: &str, text: &str, spans: &mut Vec<(Range<usize>, Vec4f)>);
}

/// A host's hooks into a markdown walk ([`Markdown::draw_events`]): what the
/// flow drew, mapped back to the source; caret places where nothing is
/// drawn; coloured code; and a drawing for every piece of source the plain
/// widget leaves out (HTML shows as muted literal text, task markers and
/// footnotes draw), so no source is hidden from an editor.
#[derive(Default)]
pub struct MarkdownHost<'a> {
    /// Filled by the walk: every run the flow drew, in order. The flow must
    /// track its text (`selectable`) for the flat ranges to mean anything.
    pub pieces: Vec<MarkdownPiece>,
    /// Source offsets that need a caret rect where no text is: an empty
    /// paragraph, heading or item, or a blank line after the last block.
    /// The walk fills in each rect, in draw-time coordinates; a place inside
    /// no empty block gets an empty line after everything else.
    pub anchors: Vec<(usize, Option<Rect>)>,
    /// Colours code blocks.
    pub highlight: Option<&'a dyn CodeHighlight>,
    /// The number footnote references show.
    pub footnote_number: Option<&'a dyn Fn(&str) -> Option<usize>>,
    /// The number of the first item of the outermost ordered list, when the
    /// events are items cut from a longer list.
    pub list_start: Option<u64>,
}

impl MarkdownHost<'_> {
    fn piece(&mut self, src: Range<usize>, flat: Range<usize>, identity: bool) {
        if !flat.is_empty() {
            self.pieces.push(MarkdownPiece {
                src,
                flat,
                identity,
            });
        }
    }
}

/// Draws `text` (whose source is `src`) and records what the flow drew.
fn draw_mapped(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    host: &mut Option<&mut MarkdownHost<'_>>,
    source: &str,
    text: &str,
    src: Range<usize>,
) {
    let drawn = tf.draw_text_range(cx, text);
    let (Some(host), Some(drawn)) = (host.as_deref_mut(), drawn) else {
        return;
    };
    let after = tf.text_len();
    let flat = after.saturating_sub(drawn.len())..after;
    // The event's text sits in the source as it is, or the run stands for its
    // whole source (an entity, text the parser made).
    let identity = source.get(src.start..src.start + text.len()) == Some(text);
    if identity {
        host.piece(src.start + drawn.start..src.start + drawn.end, flat, true);
    } else {
        host.piece(src, flat, false);
    }
}

/// The colour source shows in when the renderer has no drawing for it.
fn muted(color: Vec4f) -> Vec4f {
    vec4(color.x, color.y, color.z, color.w * 0.55)
}

impl Markdown {
    fn process_markdown_doc(&mut self, cx: &mut Cx2d) {
        let body = std::mem::take(&mut self.body);
        let parser = Parser::new_ext(
            body.as_ref(),
            Options::ENABLE_TABLES | Options::ENABLE_MATH | Options::ENABLE_STRIKETHROUGH,
        );
        self.walk_events(cx, body.as_ref(), parser.into_offset_iter(), None);
        self.body = body;
    }

    /// Draws markdown events the host parsed from `source` instead of the
    /// widget's own `body`, with its hooks: the renderer of an editor that
    /// draws a document in parts. Draws as `draw_walk` does.
    pub fn draw_events<'e>(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        source: &str,
        events: impl IntoIterator<Item = (MdEvent<'e>, Range<usize>)>,
        host: &mut MarkdownHost<'_>,
    ) {
        self.auto_id = 0;
        self.images_drawn.clear();
        self.begin(cx, walk);
        self.walk_events(cx, source, events, Some(host));
        self.end(cx);
        let drawn = &self.images_drawn;
        self.image_requests.retain(|uid, _| drawn.contains(uid));
    }

    fn walk_events<'e>(
        &mut self,
        cx: &mut Cx2d,
        source: &str,
        events: impl IntoIterator<Item = (MdEvent<'e>, Range<usize>)>,
        mut host: Option<&mut MarkdownHost<'_>>,
    ) {
        let tf = &mut self.text_flow;
        // Track state for nested formatting
        let mut list_stack: Vec<ListState> = Vec::new();
        let mut is_first_block = true;
        // Per-column alignments for the current table, and the current cell's
        // column index within its row. Both are reset when a new table starts.
        let mut table_alignments: Vec<Alignment> = Vec::new();
        let mut table_cell_index: usize = 0;
        // Nesting depth inside `![alt](src)`, and whether the image is drawn,
        // in which case its alt text is not; until it loads (or when it
        // cannot) the alt text stands in for it.
        let mut image_depth: usize = 0;
        let mut image_drawn = false;
        // Where the flow's text stood when the open image started: the image
        // (or its alt text) is one object for its whole source.
        let mut image_flat_start = 0;
        let load_images = !cx.script_data.std.host_io_only();
        // What the open inline link pushed: (a colour, an underline).
        let mut inline_link: Option<bool> = None;
        // A host's hooks see more of the source than the plain widget draws.
        let hosted = host.is_some();
        let highlight = host.as_deref().and_then(|host| host.highlight);
        // A highlighted code block's language and text events, drawn at its end.
        let mut code_language = String::new();
        let mut code_runs: Vec<(Range<usize>, String)> = Vec::new();
        let mut buffering_code = false;
        // The flow's text length where each open block started, to tell an
        // empty block a caret anchor goes in.
        let mut block_text_starts: Vec<usize> = Vec::new();

        for (event, range) in events {
            // A block that drew nothing gets the caret places inside it.
            let opens_block = hosted
                && matches!(
                    event,
                    MdEvent::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::Item | Tag::TableCell)
                );
            if let (
                Some(host),
                MdEvent::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableCell),
            ) = (host.as_deref_mut(), &event)
            {
                let started = block_text_starts.pop().unwrap_or(0);
                if tf.text_len() == started {
                    let mut place = None;
                    for (at, found) in &mut host.anchors {
                        if found.is_none() && range.start <= *at && *at <= range.end {
                            *found = Some(*place.get_or_insert_with(|| tf.draw_caret_anchor(cx)));
                        }
                    }
                }
            }
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
                    // Items cut from a longer list count on from where the
                    // host says that list is.
                    let first_number = match (list_stack.is_empty(), host.as_deref()) {
                        (true, Some(MarkdownHost { list_start: Some(start), .. })) if first_number.is_some() => {
                            Some(*start)
                        }
                        _ => first_number,
                    };
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
                    tf.strikethrough.push();
                }
                MdEvent::End(TagEnd::Strikethrough) => {
                    tf.strikethrough.pop();
                }
                MdEvent::Start(Tag::Link { dest_url, .. }) if self.inline_links => {
                    tf.font_colors.push(self.link_color);
                    let hovered = self.hovered_link.as_deref() == Some(dest_url.as_ref());
                    if hovered {
                        tf.underline.push();
                    }
                    inline_link = Some(hovered);
                }
                MdEvent::End(TagEnd::Link) if inline_link.is_some() => {
                    tf.font_colors.pop();
                    if inline_link.take() == Some(true) {
                        tf.underline.pop();
                    }
                }
                MdEvent::Start(Tag::Link { dest_url, .. }) => {
                    self.auto_id += 1;
                    let item = tf.item(cx, LiveId(self.auto_id), live_id!(link));
                    item.as_markdown_link().set_href(&dest_url);
                    item.draw_all_unscoped(cx);
                }
                MdEvent::End(TagEnd::Link) => {
                    // Link handling is done in Start event
                }
                MdEvent::Start(Tag::Image { dest_url, .. }) => {
                    image_depth += 1;
                    if image_depth == 1 {
                        image_flat_start = tf.text_len();
                        // A restricted Splash reads no files and makes no
                        // requests on the host's behalf.
                        image_drawn = load_images
                            && draw_inline_image(
                                cx,
                                tf,
                                &mut self.image_requests,
                                &mut self.images_drawn,
                                self.load_http_images,
                                &dest_url,
                            );
                    }
                }
                MdEvent::End(TagEnd::Image) => {
                    image_depth = image_depth.saturating_sub(1);
                    if image_depth == 0 {
                        image_drawn = false;
                        if let Some(host) = host.as_deref_mut() {
                            host.piece(range, image_flat_start..tf.text_len(), false);
                        }
                    }
                }
                MdEvent::Start(Tag::CodeBlock(kind)) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.pre_code_spacing);
                    }
                    is_first_block = false;
                    // Check if this is a runsplash block
                    let is_runsplash = matches!(&kind, CodeBlockKind::Fenced(lang) if lang.as_ref() == "runsplash");
                    if is_runsplash {
                        self.in_splash_block = true;
                        self.splash_block_string.clear();
                    } else if self.use_code_block_widget {
                        self.in_code_block = true;
                        self.code_block_string.clear();
                    } else {
                        tf.push_size_rel_scale(tf.fixed_font_size_scale);
                        tf.fixed.push();
                        tf.begin_code(cx);
                        if highlight.is_some() {
                            buffering_code = true;
                            code_runs.clear();
                            code_language.clear();
                            if let CodeBlockKind::Fenced(info) = &kind {
                                code_language.push_str(info.split_whitespace().next().unwrap_or(""));
                            }
                        }
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
                        if buffering_code {
                            buffering_code = false;
                            if let Some(highlight) = highlight {
                                draw_highlighted_code(
                                    cx,
                                    tf,
                                    &mut host,
                                    source,
                                    highlight,
                                    &code_language,
                                    &code_runs,
                                );
                            }
                        }
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.end_code(cx);
                    }
                }
                // Inline code
                MdEvent::Code(text) => {
                    tf.push_size_rel_scale(tf.fixed_font_size_scale);
                    tf.fixed.push();
                    tf.inline_code.push();
                    if host.is_some() {
                        // The content sits inside the backtick runs (and one
                        // space of padding on each side when both are there).
                        let span = source.get(range.clone()).unwrap_or("");
                        let ticks = span.bytes().take_while(|&b| b == b'`').count();
                        let pad = span.len().saturating_sub(ticks * 2).saturating_sub(text.len()) / 2;
                        let start = range.start + ticks + pad;
                        draw_mapped(cx, tf, &mut host, source, &text, start..start + text.len());
                    } else {
                        tf.draw_text(cx, &text);
                    }
                    tf.font_sizes.pop();
                    tf.fixed.pop();
                    tf.inline_code.pop();
                }
                // Inline math ($...$)
                MdEvent::InlineMath(text) => {
                    let flat_start = tf.text_len();
                    if self.use_math_widget {
                        let entry_id = tf.new_counted_id();
                        tf.item_with(cx, entry_id, live_id!(inline_math), |cx, item, _tf| {
                            item.set_text(cx, &text);
                            item.draw_all_unscoped(cx);
                        });
                    } else {
                        // Fallback: render as inline code style
                        tf.push_size_rel_scale(tf.fixed_font_size_scale);
                        tf.fixed.push();
                        tf.inline_code.push();
                        tf.draw_text(cx, &text);
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.inline_code.pop();
                    }
                    if let Some(host) = host.as_deref_mut() {
                        host.piece(range, flat_start..tf.text_len(), false);
                    }
                }
                // Display math ($$...$$)
                MdEvent::DisplayMath(text) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    let flat_start = tf.text_len();

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
                    if let Some(host) = host.as_deref_mut() {
                        host.piece(range, flat_start..tf.text_len(), false);
                    }
                }
                MdEvent::Text(_) if image_drawn => {}
                // The alt text of an image that is not drawn stands in for
                // the image: one object, recorded at the image's end.
                MdEvent::Text(text) if image_depth > 0 => {
                    tf.draw_text(cx, text.trim_end_matches("\n"));
                }
                MdEvent::Text(text) => {
                    if self.in_splash_block {
                        self.splash_block_string.push_str(&text);
                    } else if self.in_code_block {
                        self.code_block_string.push_str(&text);
                    } else if buffering_code {
                        code_runs.push((range, text.to_string()));
                    } else if host.is_some() {
                        draw_mapped(cx, tf, &mut host, source, text.trim_end_matches("\n"), range);
                    } else {
                        tf.draw_text(cx, &text.trim_end_matches("\n"));
                    }
                }
                MdEvent::SoftBreak => {
                    if self.in_splash_block {
                        self.splash_block_string.push('\n');
                    } else if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        let flat_start = tf.text_len();
                        tf.draw_text(cx, " ");
                        if let Some(host) = host.as_deref_mut() {
                            host.piece(range, flat_start..tf.text_len(), false);
                        }
                    }
                }
                MdEvent::HardBreak => {
                    if self.in_splash_block {
                        self.splash_block_string.push('\n');
                    } else if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else {
                        let flat_start = tf.text_len();
                        tf.new_line_collapsed(cx);
                        if let Some(host) = host.as_deref_mut() {
                            host.piece(range, flat_start..tf.text_len(), false);
                        }
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
                MdEvent::TaskListMarker(checked) => {
                    if host.is_some() {
                        // A ballot box the flow's fonts all have: the marker
                        // as written, in the code face.
                        let flat_start = tf.text_len();
                        tf.fixed.push();
                        tf.font_colors.push(muted(tf.font_color));
                        tf.draw_text(cx, if checked { "[x] " } else { "[ ] " });
                        tf.font_colors.pop();
                        tf.fixed.pop();
                        if let Some(host) = host.as_deref_mut() {
                            host.piece(range, flat_start..tf.text_len(), false);
                        }
                    }
                }
                MdEvent::FootnoteReference(label) => {
                    if let Some(host) = host.as_deref_mut() {
                        let number = host
                            .footnote_number
                            .and_then(|number| number(&label))
                            .map(|number| number.to_string())
                            .unwrap_or_else(|| label.to_string());
                        let flat_start = tf.text_len();
                        tf.push_size_rel_scale(0.7);
                        tf.y_shift_scales.push(-0.2);
                        tf.font_colors.push(self.link_color);
                        tf.draw_text(cx, &number);
                        tf.font_colors.pop();
                        tf.y_shift_scales.pop();
                        tf.font_sizes.pop();
                        host.piece(range, flat_start..tf.text_len(), false);
                    }
                }
                MdEvent::Start(Tag::FootnoteDefinition(label)) if hosted => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    let number = host
                        .as_deref()
                        .and_then(|host| host.footnote_number)
                        .and_then(|number| number(&label))
                        .map(|number| number.to_string())
                        .unwrap_or_else(|| label.to_string());
                    tf.push_size_rel_scale(0.7);
                    tf.y_shift_scales.push(-0.2);
                    tf.font_colors.push(muted(tf.font_color));
                    tf.draw_text(cx, &format!("{number} "));
                    tf.font_colors.pop();
                    tf.y_shift_scales.pop();
                    tf.font_sizes.pop();
                }
                MdEvent::Start(Tag::HtmlBlock) if hosted => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                }
                // Source the renderer has no drawing for shows as itself, muted.
                MdEvent::Html(text) if hosted => {
                    tf.font_colors.push(muted(tf.font_color));
                    let line = text.trim_end_matches('\n');
                    draw_mapped(cx, tf, &mut host, source, line, range.start..range.start + line.len());
                    tf.font_colors.pop();
                    if line.len() < text.len() {
                        let flat_start = tf.text_len();
                        tf.new_line_collapsed(cx);
                        if let Some(host) = host.as_deref_mut() {
                            let at = range.start + line.len();
                            host.piece(at..at + 1, flat_start..tf.text_len(), true);
                        }
                    }
                }
                MdEvent::Start(Tag::Table(alignments)) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    tf.begin_table(cx, alignments.len());
                    table_alignments = alignments;
                    table_cell_index = 0;
                }
                MdEvent::End(TagEnd::Table) => {
                    tf.end_table(cx);
                    tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    table_alignments.clear();
                    table_cell_index = 0;
                }
                MdEvent::Start(Tag::TableHead) => {
                    tf.begin_table_header_row(cx);
                    table_cell_index = 0;
                }
                MdEvent::End(TagEnd::TableHead) => {
                    tf.end_table_row(cx);
                    tf.in_table_header = false;
                }
                MdEvent::Start(Tag::TableRow) => {
                    tf.begin_table_row(cx);
                    table_cell_index = 0;
                }
                MdEvent::End(TagEnd::TableRow) => {
                    tf.end_table_row(cx);
                }
                MdEvent::Start(Tag::TableCell) => {
                    let align_x = table_alignments
                        .get(table_cell_index)
                        .map(alignment_to_x)
                        .unwrap_or(0.0);
                    tf.begin_table_cell(cx, align_x);
                    if tf.in_table_header {
                        tf.bold.push();
                    }
                }
                MdEvent::End(TagEnd::TableCell) => {
                    if tf.in_table_header {
                        tf.bold.pop();
                    }
                    tf.end_table_cell(cx);
                    table_cell_index += 1;
                }
                MdEvent::InlineHtml(text) => {
                    // Support a handful of inline HTML tags that have no
                    // CommonMark equivalent. Anything not matched is ignored,
                    // matching the pre-existing behavior; a host sees it.
                    match text.trim().to_ascii_lowercase().as_str() {
                        "<sub>" => {
                            tf.push_size_rel_scale(0.7);
                            tf.y_shift_scales.push(0.55);
                        }
                        "</sub>" => {
                            tf.font_sizes.pop();
                            tf.y_shift_scales.pop();
                        }
                        "<sup>" => {
                            tf.push_size_rel_scale(0.7);
                            tf.y_shift_scales.push(-0.2);
                        }
                        "</sup>" => {
                            tf.font_sizes.pop();
                            tf.y_shift_scales.pop();
                        }
                        _ if hosted => {
                            tf.font_colors.push(muted(tf.font_color));
                            draw_mapped(cx, tf, &mut host, source, &text, range);
                            tf.font_colors.pop();
                        }
                        _ => {}
                    }
                }
                _ => {} // Unimplemented or unnecessary events
            }
            // Counted after the block's own marker (a list bullet) is drawn.
            if opens_block {
                block_text_starts.push(tf.text_len());
            }
        }
        // Caret places in no empty block: a blank line after the last block.
        if let Some(host) = host.as_deref_mut() {
            if host.anchors.iter().any(|(_, rect)| rect.is_none()) {
                if !is_first_block {
                    tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                }
                let place = tf.draw_caret_anchor(cx);
                for (_, rect) in &mut host.anchors {
                    rect.get_or_insert(place);
                }
            }
        }
    }
}

/// Draws a code block's text runs in the colours `highlight` gives them,
/// recording what was drawn. A run never ends in a line break (the flow trims
/// those): a colour change right after one moves before it.
fn draw_highlighted_code(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    host: &mut Option<&mut MarkdownHost<'_>>,
    source: &str,
    highlight: &dyn CodeHighlight,
    language: &str,
    runs: &[(Range<usize>, String)],
) {
    let mut spans = Vec::new();
    for (src, text) in runs {
        spans.clear();
        highlight.highlight(language, text, &mut spans);
        // Cut points: every span edge, moved off the end of a line break.
        let mut cuts: Vec<usize> = spans
            .iter()
            .flat_map(|(range, _)| [range.start, range.end])
            .filter(|&at| at > 0 && at < text.len())
            .map(|at| if text.as_bytes()[at - 1] == b'\n' { at - 1 } else { at })
            .filter(|&at| at > 0)
            .collect();
        cuts.push(0);
        cuts.push(text.len());
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            // A run's colour is its first character's, a leading line break aside.
            let probe = if text[from..to].starts_with('\n') { from + 1 } else { from };
            let color = spans
                .iter()
                .find(|(range, _)| range.start <= probe && probe < range.end)
                .map(|(_, color)| *color);
            if let Some(color) = color {
                tf.font_colors.push(color);
            }
            let run = &text[from..to];
            let run = if to == text.len() { run.trim_end_matches('\n') } else { run };
            draw_mapped(cx, tf, host, source, run, src.start + from..src.start + to);
            if color.is_some() {
                tf.font_colors.pop();
            }
        }
    }
}

/// Draws the inline image for `src`, requesting its load the first time its
/// widget sees that source. True when the picture was drawn; false while it
/// loads, or when it cannot load, and the caller draws the alt text instead.
fn draw_inline_image(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    requests: &mut HashMap<WidgetUid, (String, Option<PathBuf>)>,
    drawn: &mut Vec<WidgetUid>,
    load_http: bool,
    src: &str,
) -> bool {
    let entry_id = tf.new_counted_id();
    let item = tf.item(cx, entry_id, live_id!(inline_image));
    let uid = item.widget_uid();
    drawn.push(uid);
    let image = item.as_image();
    if requests.get(&uid).map(|(requested, _)| requested.as_str()) != Some(src) {
        let key = request_inline_image(cx, &image, src, load_http);
        requests.insert(uid, (src.to_string(), key));
    }
    let Some((width, height)) = image.size_in_pixels(cx) else {
        return false;
    };
    if width == 0 || height == 0 {
        return false;
    }
    // A texel is a point, as for a CSS pixel; wider than the text column it
    // scales down to fit.
    let (width, height) = (width as f64, height as f64);
    let max_width = cx.turtle().inner_width();
    let scale = if max_width.is_finite() && max_width > 0.0 && width > max_width {
        max_width / width
    } else {
        1.0
    };
    item.draw_walk_all(
        cx,
        &mut Scope::empty(),
        Walk::fixed(width * scale, height * scale),
    );
    true
}

/// Starts loading `src` into `image` through the shared image cache: an
/// `http(s)` URL (when allowed) with the platform's HTTP stack, a `data:`
/// URL from its bytes, anything else as a file path (`file://` or plain,
/// relative to the working directory), all decoded off the UI thread. An
/// SVG, from a `data:` URL or a `.svg` file, is parsed in place. Returns the
/// cache key an asynchronous load completes under.
fn request_inline_image(cx: &mut Cx, image: &ImageRef, src: &str, load_http: bool) -> Option<PathBuf> {
    if src.starts_with("http://") || src.starts_with("https://") {
        if !load_http {
            return None;
        }
        image.load_image_http_by_url_async(cx, src).ok()?;
        return Some(PathBuf::from(src));
    }
    if let Some(data_url) = src.strip_prefix("data:") {
        let bytes = decode_data_url(data_url)?;
        if looks_like_svg(&bytes) {
            image.load_svg_from_data(cx, &bytes).ok()?;
            return None;
        }
        let key = PathBuf::from(format!("data:{:016x}", LiveId::from_str(src).0));
        image
            .load_image_from_data_async(cx, &key, Arc::new(bytes))
            .ok()?;
        return Some(key);
    }
    let path = src.strip_prefix("file://").unwrap_or(src);
    if path.is_empty() || path.contains("://") {
        return None;
    }
    let path = Path::new(path);
    let is_svg = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"));
    if is_svg {
        let bytes = std::fs::read(path).ok()?;
        image.load_svg_from_data(cx, &bytes).ok()?;
        return None;
    }
    image.load_image_file_by_path_async(cx, path).ok()?;
    Some(path.to_path_buf())
}

/// The bytes of a `data:` URL, given what follows `data:`: base64 when the
/// media type ends in `;base64`, percent-encoded otherwise.
fn decode_data_url(data_url: &str) -> Option<Vec<u8>> {
    let (media_type, payload) = data_url.split_once(',')?;
    if media_type.ends_with(";base64") {
        let compact: Vec<u8> = payload
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        return makepad_base64::base64_decode(&compact).ok();
    }
    let bytes = payload.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(out)
}

/// Maps pulldown_cmark table-column alignment to `Layout::align.x`.
fn alignment_to_x(alignment: &Alignment) -> f64 {
    match alignment {
        Alignment::None | Alignment::Left => 0.0,
        Alignment::Center => 0.5,
        Alignment::Right => 1.0,
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
        // A restricted Splash must not hand its URLs to the host, which may open them.
        let Some(modifiers) = self.link.clicked_modifiers(actions) else {
            return;
        };
        if !cx.script_data.std.host_io_only() {
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
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Link;
        makepad_platform::a11y::set_string(&mut node.name, &self.text());
        node.states = a11y_tree::States::FOCUSABLE;
        node.states
            .set(a11y_tree::States::DISABLED, self.disabled(cx));
        node.actions = a11y_tree::Actions::FOCUS | a11y_tree::Actions::PRESS;
    }

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

impl MarkdownRef {
    /// Underline the inline link (`inline_links`) whose url is `url`.
    pub fn set_hovered_link(&self, cx: &mut Cx, url: Option<&str>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.hovered_link.as_deref() != url {
            inner.hovered_link = url.map(str::to_string);
            inner.redraw(cx);
        }
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
    /// A `[text](url)` link was clicked, with the key modifiers held during
    /// the click, so a desktop host can open links on Cmd/Ctrl-click only
    /// and leave plain clicks free for selecting text; touch hosts, which
    /// have no modifiers, open on every tap.
    LinkNavigated {
        url: String,
        modifiers: KeyModifiers,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::ButtonAction;

    #[test]
    fn restricted_links_emit_no_url() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut link = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {use mod.widgets.* MarkdownLink{href: "https://example.invalid/"}});
            MarkdownLink::script_from_value(vm, value)
        });
        let uid = link.widget_uid();
        let clicked = cx.capture_actions(|cx| cx.widget_action(uid, ButtonAction::Clicked(Default::default())));
        let mut navigations = |cx: &mut Cx| cx.capture_actions(|cx| link.handle_actions(cx, &clicked, &mut Scope::empty()))
            .iter()
            .filter(|action| matches!(action.as_widget_action().cast(), MarkdownAction::LinkNavigated { .. }))
            .count();
        assert_eq!(navigations(&mut cx), 1);
        cx.script_data.std.restrict_to_host_io();
        assert_eq!(navigations(&mut cx), 0);
    }
}
