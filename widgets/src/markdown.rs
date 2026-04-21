use crate::{
    image::{ImageRef, ImageWidgetRefExt},
    link_label::LinkLabel, makepad_derive_widget::*, makepad_draw::*,
    text_flow::TextFlow, widget::*, WidgetMatchEvent,
};

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Image rendering support (M-img-1 — local + data URL only; HTTP is M-img-2).
// ---------------------------------------------------------------------------

/// Upper bound on the inline display width (logical px). Larger images are
/// scaled down proportionally; smaller ones keep their intrinsic size.
const IMAGE_MAX_DISPLAY_W: f64 = 480.0;
/// Widget-local texture cache cap — decoded texture bytes (w*h*4) beyond this
/// value trigger LRU eviction. 16 MiB ≈ 1–2 large photos or ~16 modest icons.
const IMAGE_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Classification of an `![alt](url)` URL after parsing but before I/O.
#[derive(Debug, Clone)]
enum ImageSrc {
    /// Plain on-disk path (absolute or relative, no scheme).
    #[cfg(not(target_arch = "wasm32"))]
    Local(PathBuf),
    /// `file://` URL resolved to a path.
    #[cfg(not(target_arch = "wasm32"))]
    File(PathBuf),
    /// `data:[<mime>][;base64],<payload>` already base64-decoded (or
    /// raw bytes for the non-base64 variant).
    Data(Vec<u8>),
    /// `http://` or `https://` — deferred to M-img-2.
    #[allow(dead_code)]
    Http(String),
    /// Anything else: unknown scheme, malformed data URL, platform-unsupported.
    Invalid,
}

/// Classify an image URL. Does no I/O; pure function. Relative paths are
/// resolved against `std::env::current_dir()` at call time (per spec).
fn parse_image_src(url: &str) -> ImageSrc {
    if url.is_empty() {
        return ImageSrc::Invalid;
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, payload) = match rest.split_once(',') {
            Some(x) => x,
            None => return ImageSrc::Invalid,
        };
        if meta.split(';').any(|s| s.eq_ignore_ascii_case("base64")) {
            match decode_base64(payload.trim()) {
                Some(bytes) => ImageSrc::Data(bytes),
                None => ImageSrc::Invalid,
            }
        } else {
            ImageSrc::Data(payload.as_bytes().to_vec())
        }
    } else if let Some(rest) = url.strip_prefix("file://") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = if let Some(s) = rest.strip_prefix("/") {
                format!("/{}", s)
            } else {
                rest.to_string()
            };
            ImageSrc::File(PathBuf::from(path))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = rest;
            ImageSrc::Invalid
        }
    } else if url.starts_with("http://") || url.starts_with("https://") {
        ImageSrc::Http(url.to_string())
    } else if url.contains("://") {
        ImageSrc::Invalid
    } else {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let p = std::path::Path::new(url);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                match std::env::current_dir() {
                    Ok(cwd) => cwd.join(p),
                    Err(_) => p.to_path_buf(),
                }
            };
            ImageSrc::Local(abs)
        }
        #[cfg(target_arch = "wasm32")]
        {
            ImageSrc::Invalid
        }
    }
}

/// Minimal standard-alphabet base64 decoder. Handles optional `=` padding and
/// skips whitespace (newlines are common in wrapped data URLs). Returns None
/// on invalid input — caller falls back to the placeholder path.
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &c in input.as_bytes() {
        if c.is_ascii_whitespace() || c == b'=' {
            continue;
        }
        let v = val(c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8 & 0xFF);
        }
    }
    Some(out)
}

/// One decoded image stored in the widget-local cache. Owns a ref-counted
/// Makepad `Texture` (clones share the GPU-side handle) plus intrinsic pixel
/// dimensions used to compute display Walk.
#[derive(Clone)]
struct ImageCacheEntry {
    texture: Texture,
    width: u32,
    height: u32,
}

impl ImageCacheEntry {
    /// Bytes charged against the LRU cap — RGBA8 * pixels.
    fn byte_size(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }
}

/// URL-hash used as both cache key and TextFlow template item id.
fn url_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Canonical cache key for an `ImageSrc`. `file:///tmp/x.png` and
/// `/tmp/x.png` hash to the same value (second occurrence is a cache hit).
/// Data URLs hash the full URL (which contains the payload). HTTP/invalid
/// hash the raw URL.
fn src_cache_key(src: &ImageSrc, raw_url: &str) -> u64 {
    match src {
        #[cfg(not(target_arch = "wasm32"))]
        ImageSrc::Local(p) | ImageSrc::File(p) => {
            let s = p.to_string_lossy();
            url_hash(&s)
        }
        _ => url_hash(raw_url),
    }
}

/// Sniff image format from the first few bytes. Mirrors
/// `makepad_draw::image_cache::detect_image_format` (which is private).
fn detect_image_magic(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        Some("png")
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        Some("jpg")
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Promote `key` to the back of the LRU list (most-recently-used). No-op
/// if the key isn't already present.
fn touch_lru(lru: &mut Vec<u64>, key: u64) {
    if let Some(pos) = lru.iter().position(|k| *k == key) {
        lru.remove(pos);
    }
    lru.push(key);
}

/// Evict oldest entries until total bytes are STRICTLY BELOW `cap_bytes`.
/// Spec mandates strict-less-than, not ≤.
fn evict_over_cap(
    cache: &mut HashMap<u64, ImageCacheEntry>,
    lru: &mut Vec<u64>,
    cap_bytes: usize,
) {
    let mut total: usize = cache.values().map(|e| e.byte_size()).sum();
    while total >= cap_bytes && !lru.is_empty() {
        let oldest = lru.remove(0);
        if let Some(entry) = cache.remove(&oldest) {
            total = total.saturating_sub(entry.byte_size());
        }
    }
}

/// Instantiate the `inline_image` template and render `entry` at a display
/// size scaled to respect `IMAGE_MAX_DISPLAY_W`, aspect-ratio-preserved.
/// Returns true on success (template present AND closure ran), false when
/// the template is not registered in the consuming app's Markdown DSL.
fn draw_cached_image(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    key: u64,
    entry: ImageCacheEntry,
) -> bool {
    let (iw, ih) = (entry.width as f64, entry.height as f64);
    let (dw, dh) = if iw > IMAGE_MAX_DISPLAY_W {
        let scale = IMAGE_MAX_DISPLAY_W / iw;
        (IMAGE_MAX_DISPLAY_W, (ih * scale).max(1.0))
    } else {
        (iw, ih)
    };
    let entry_id = LiveId(key);
    // `item_with` returns the closure's value when the template exists,
    // or `R::default()` (here `false`) when it doesn't. This lets callers
    // distinguish "rendered" from "template missing, fall back to placeholder".
    tf.item_with(cx, entry_id, live_id!(inline_image), |cx, item, _tf| {
        let img: ImageRef = item.as_image();
        img.set_texture(cx, Some(entry.texture.clone()));
        let _ = item.draw_walk(cx, &mut Scope::empty(), Walk::fixed(dw, dh));
        true
    })
}

/// In-flight state between `Start(Tag::Image)` and `End(TagEnd::Image)`.
/// `Successful` swallows alt-text events so they don't render alongside the
/// image. `Placeholder` collects alt text for rendering `🖼 <alt>` at End().
enum ImageEventState {
    /// Image was rendered inline at Start; subsequent Text/SoftBreak events
    /// (the alt) are discarded until End(TagEnd::Image).
    Successful,
    /// Image load failed or was rejected. We buffer alt text between
    /// Start and End, then render `🖼 <alt-or-url>` at End().
    Placeholder { alt: String, fallback: String },
}

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
        // Inline image template instantiated by `tf.item_with(..., live_id!(inline_image), ...)`
        // at every `Start(Tag::Image)`. ImageFit.Stretch means the Walk we pass
        // (computed from intrinsic dimensions + IMAGE_MAX_DISPLAY_W cap) is
        // honoured verbatim — no aspect-math from the widget itself.
        inline_image := mod.widgets.Image{
            fit: ImageFit.Stretch
            width: 1 height: 1
        }
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

    // --- Image rendering state (M-img-1) ---
    /// Widget-local texture cache keyed by `url_hash`. Survives across
    /// `set_text` re-parses (streaming), so a partially-typed prefix that
    /// later completes without the image URL changing hits the cache on
    /// every re-render. Only successful decodes are inserted — failed
    /// URLs are NEVER cached (so a later retry can re-attempt). LRU
    /// eviction keyed off `image_cache_lru` order.
    #[rust]
    image_cache: HashMap<u64, ImageCacheEntry>,
    /// Insertion / access order for LRU. Front = oldest, back = most
    /// recently touched.
    #[rust]
    image_cache_lru: Vec<u64>,
    /// Byte cap override for tests (0 means "use IMAGE_CACHE_MAX_BYTES").
    #[rust]
    image_cache_cap_override: usize,
    /// State for an in-progress `Start(Tag::Image)` ... `End(TagEnd::Image)`
    /// range. The inner `MdEvent::Text` events carry the alt text.
    #[rust]
    in_image: Option<ImageEventState>,
}

impl Widget for Markdown {
    fn is_interactive(&self) -> bool {
        false
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.text_flow.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.auto_id = 0;

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

impl Markdown {
    fn process_markdown_doc(&mut self, cx: &mut Cx2d) {
        let tf = &mut self.text_flow;
        // Track state for nested formatting
        let mut list_stack: Vec<ListState> = Vec::new();
        let mut is_first_block = true;
        // Per-column alignments for the current table, and the current cell's
        // column index within its row. Both are reset when a new table starts.
        let mut table_alignments: Vec<Alignment> = Vec::new();
        let mut table_cell_index: usize = 0;

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
                    self.auto_id += 1;
                    let item = tf.item(cx, LiveId(self.auto_id), live_id!(link));
                    item.as_markdown_link().set_href(&dest_url);
                    item.draw_all_unscoped(cx);
                }
                MdEvent::End(TagEnd::Link) => {
                    // Link handling is done in Start event
                }
                MdEvent::Start(Tag::Image { dest_url, .. }) => {
                    // Try to load + decode the image (cache hit short-circuits).
                    // On success we draw it inline here and mark the state so the
                    // intervening MdEvent::Text (alt) events get swallowed until
                    // End(TagEnd::Image). On failure we buffer alt text and
                    // render `🖼 <alt-or-url>` at End() — so the placeholder
                    // path produces exactly ONE output per image per set_text.
                    let url = dest_url.as_ref();
                    let fallback_label = url.to_string();
                    let cap = if self.image_cache_cap_override != 0 {
                        self.image_cache_cap_override
                    } else {
                        IMAGE_CACHE_MAX_BYTES
                    };
                    let loaded = Self::try_load_image(
                        cx,
                        tf,
                        url,
                        &mut self.image_cache,
                        &mut self.image_cache_lru,
                        cap,
                    );
                    self.in_image = Some(if loaded {
                        ImageEventState::Successful
                    } else {
                        ImageEventState::Placeholder {
                            alt: String::new(),
                            fallback: fallback_label,
                        }
                    });
                }
                MdEvent::End(TagEnd::Image) => {
                    if let Some(state) = self.in_image.take() {
                        if let ImageEventState::Placeholder { alt, fallback } = state {
                            tf.draw_text(cx, "🖼 ");
                            let label = if !alt.is_empty() { &alt } else { &fallback };
                            tf.draw_text(cx, label);
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
                            //let tree = item.widget_tree();
                            //cx.with_vm(|vm| {
                            //    log!("$splash_block widget tree:\n{}", tree.display(vm.heap()));
                            //});
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
                        tf.font_sizes.pop();
                        tf.fixed.pop();
                        tf.combine_spaces.pop();
                        tf.end_code(cx);
                    }
                }
                // Inline code
                MdEvent::Code(text) => {
                    const FIXED_FONT_SIZE_SCALE: f64 = 0.85;
                    tf.push_size_rel_scale(FIXED_FONT_SIZE_SCALE);
                    tf.fixed.push();
                    tf.inline_code.push();
                    tf.draw_text(cx, &text);
                    tf.font_sizes.pop();
                    tf.fixed.pop();
                    tf.inline_code.pop();
                }
                // Inline math ($...$)
                MdEvent::InlineMath(text) => {
                    if self.use_math_widget {
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
                    if self.in_splash_block {
                        self.splash_block_string.push_str(&text);
                    } else if self.in_code_block {
                        self.code_block_string.push_str(&text);
                    } else if let Some(state) = self.in_image.as_mut() {
                        // Inside an image tag: alt text is either swallowed
                        // (Successful) or accumulated for the placeholder.
                        if let ImageEventState::Placeholder { alt, .. } = state {
                            alt.push_str(&text);
                        }
                    } else {
                        tf.draw_text(cx, &text.trim_end_matches("\n"));
                    }
                }
                MdEvent::SoftBreak => {
                    if self.in_splash_block {
                        self.splash_block_string.push('\n');
                    } else if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else if let Some(state) = self.in_image.as_mut() {
                        if let ImageEventState::Placeholder { alt, .. } = state {
                            alt.push(' ');
                        }
                    } else {
                        tf.draw_text(cx, " ");
                    }
                }
                MdEvent::HardBreak => {
                    if self.in_splash_block {
                        self.splash_block_string.push('\n');
                    } else if self.in_code_block {
                        self.code_block_string.push('\n');
                    } else if let Some(state) = self.in_image.as_mut() {
                        if let ImageEventState::Placeholder { alt, .. } = state {
                            alt.push(' ');
                        }
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
                    // matching the pre-existing behavior.
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
                        _ => {}
                    }
                }
                _ => {} // Unimplemented or unnecessary events
            }
        }
    }
}

impl Markdown {
    /// Try to render `url` as an inline image. Returns `true` on success
    /// (image placed into the flow via the `inline_image` template);
    /// returns `false` on any failure — caller falls back to the
    /// `🖼 <alt>` placeholder. Failed URLs are NOT cached, so a later
    /// retry is free to re-attempt.
    #[allow(clippy::too_many_arguments)]
    fn try_load_image(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        url: &str,
        cache: &mut HashMap<u64, ImageCacheEntry>,
        lru: &mut Vec<u64>,
        cap_bytes: usize,
    ) -> bool {
        // Classify the URL first — we derive the cache key from the
        // CANONICAL form so `file:///tmp/x.png` and `/tmp/x.png` hit
        // the same entry.
        let src = parse_image_src(url);
        let key = src_cache_key(&src, url);

        // Cache hit — touch LRU and draw.
        if cache.contains_key(&key) {
            touch_lru(lru, key);
            let entry = cache.get(&key).unwrap().clone();
            return draw_cached_image(cx, tf, key, entry);
        }

        // Load bytes. Failed loads warn + return false (placeholder).
        let bytes: Vec<u8> = match src {
            #[cfg(not(target_arch = "wasm32"))]
            ImageSrc::Local(ref p) | ImageSrc::File(ref p) => match std::fs::read(p) {
                Ok(b) => b,
                Err(_) => {
                    warning!("markdown image: file not found {}", p.display());
                    return false;
                }
            },
            ImageSrc::Data(b) => b,
            ImageSrc::Http(_) => {
                // HTTP is M-img-2. Placeholder path; do NOT cache.
                warning!("markdown image: HTTP deferred (M-img-2) {}", url);
                return false;
            }
            ImageSrc::Invalid => {
                warning!("markdown image: unknown scheme or invalid url {}", url);
                return false;
            }
        };

        // Format sniff + decode. Any error → placeholder + warn; no cache.
        let fmt = match detect_image_magic(&bytes) {
            Some(f) => f,
            None => {
                warning!("markdown image: unsupported format {}", url);
                return false;
            }
        };
        let buf = match fmt {
            "png" => ImageBuffer::from_png(&bytes),
            "jpg" => ImageBuffer::from_jpg(&bytes),
            "webp" => ImageBuffer::from_webp(&bytes),
            _ => {
                warning!("markdown image: unsupported format {}", url);
                return false;
            }
        };
        let buf = match buf {
            Ok(b) => b,
            Err(e) => {
                warning!("markdown image: decode error {} ({})", url, e);
                return false;
            }
        };

        let (w, h) = (buf.width as u32, buf.height as u32);
        if w == 0 || h == 0 {
            warning!("markdown image: decode error (zero dim) {}", url);
            return false;
        }
        let texture = buf.into_new_texture(cx);
        let entry = ImageCacheEntry {
            texture,
            width: w,
            height: h,
        };

        // Insert + enforce byte cap via LRU. Eviction keeps the total
        // STRICTLY below the cap.
        cache.insert(key, entry.clone());
        lru.push(key);
        evict_over_cap(cache, lru, cap_bytes);

        draw_cached_image(cx, tf, key, entry)
    }
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
        if self.link.clicked(actions) {
            cx.widget_action(
                self.widget_uid(),
                MarkdownAction::LinkNavigated(self.href.clone()),
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
    LinkNavigated(String),
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure helpers behind M-img-1 inline-image rendering:
    //! URL classification, base64 decode, magic-byte detection, LRU math.
    //!
    //! Widget-level scenarios (decode + draw + TextFlow instantiation)
    //! cannot run here because `makepad-widgets` has no widget harness.
    //! Those are manual-verification gates covered by the consumer app.
    use super::*;

    #[test]
    fn parse_image_src_http_https() {
        assert!(matches!(
            parse_image_src("http://example.com/x.png"),
            ImageSrc::Http(_)
        ));
        assert!(matches!(
            parse_image_src("https://example.com/x.png"),
            ImageSrc::Http(_)
        ));
    }

    #[test]
    fn parse_image_src_unknown_scheme_is_invalid() {
        assert!(matches!(
            parse_image_src("gopher://old/x.png"),
            ImageSrc::Invalid
        ));
        assert!(matches!(
            parse_image_src("ftp://host/x.png"),
            ImageSrc::Invalid
        ));
        assert!(matches!(parse_image_src(""), ImageSrc::Invalid));
    }

    #[test]
    fn parse_image_src_data_base64() {
        // 1x1 transparent PNG
        let url = "data:image/png;base64,iVBORw0KGgo=";
        match parse_image_src(url) {
            ImageSrc::Data(b) => {
                assert!(b.len() >= 8);
                assert_eq!(&b[0..8], b"\x89PNG\r\n\x1a\n");
            }
            other => panic!("expected Data, got {:?}", other),
        }
    }

    #[test]
    fn parse_image_src_data_malformed_base64() {
        let url = "data:image/png;base64,!!!not-valid!!!";
        assert!(matches!(parse_image_src(url), ImageSrc::Invalid));
    }

    #[test]
    fn parse_image_src_data_missing_comma_invalid() {
        assert!(matches!(
            parse_image_src("data:image/png;base64"),
            ImageSrc::Invalid
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn parse_image_src_file_scheme() {
        match parse_image_src("file:///tmp/x.png") {
            ImageSrc::File(p) => assert_eq!(p, std::path::PathBuf::from("/tmp/x.png")),
            other => panic!("expected File, got {:?}", other),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn parse_image_src_absolute_local_path() {
        match parse_image_src("/tmp/foo.png") {
            ImageSrc::Local(p) => assert_eq!(p, std::path::PathBuf::from("/tmp/foo.png")),
            other => panic!("expected Local, got {:?}", other),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn parse_image_src_relative_path_resolved_against_cwd() {
        match parse_image_src("./foo.png") {
            ImageSrc::Local(p) => {
                assert!(p.is_absolute(), "relative path should resolve to absolute");
                assert!(p.ends_with("foo.png"));
            }
            other => panic!("expected Local, got {:?}", other),
        }
    }

    #[test]
    fn decode_base64_standard() {
        assert_eq!(decode_base64("TWFu").unwrap(), b"Man".to_vec());
        assert_eq!(decode_base64("TWE=").unwrap(), b"Ma".to_vec());
        assert_eq!(decode_base64("TQ==").unwrap(), b"M".to_vec());
        assert_eq!(decode_base64("TWFu\n").unwrap(), b"Man".to_vec());
    }

    #[test]
    fn decode_base64_invalid_char_returns_none() {
        assert!(decode_base64("!!!").is_none());
        assert!(decode_base64("T@Fu").is_none());
    }

    #[test]
    fn detect_image_magic_recognises_formats() {
        assert_eq!(detect_image_magic(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(detect_image_magic(b"\xFF\xD8\xFFsomejpegdata"), Some("jpg"));
        let mut webp = Vec::from(&b"RIFF"[..]);
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(detect_image_magic(&webp), Some("webp"));
        assert_eq!(detect_image_magic(b"\x00garbage"), None);
    }

    #[test]
    fn touch_lru_moves_to_back() {
        let mut lru = vec![1u64, 2, 3];
        touch_lru(&mut lru, 1);
        assert_eq!(lru, vec![2, 3, 1]);
        touch_lru(&mut lru, 999);
        assert_eq!(lru, vec![2, 3, 1, 999]);
    }

    #[test]
    fn evict_over_cap_logic_via_manual_totals() {
        // Pure-logic verification of evict_over_cap WITHOUT fabricating
        // Texture values: reproduce the size-accounting loop inline.
        fn evict_logic(sizes: &mut Vec<usize>, lru: &mut Vec<u64>, cap: usize) {
            let mut total: usize = sizes.iter().sum();
            while total >= cap && !lru.is_empty() {
                let _ = lru.remove(0);
                let s = sizes.remove(0);
                total = total.saturating_sub(s);
            }
        }
        // 3 entries of 1 MiB each; cap at 1 MiB — strict-less-than evicts all.
        let mut sizes = vec![1_048_576usize, 1_048_576, 1_048_576];
        let mut lru = vec![1u64, 2, 3];
        evict_logic(&mut sizes, &mut lru, 1_048_576);
        assert!(sizes.iter().sum::<usize>() < 1_048_576);
        assert_eq!(sizes.len(), 0);
        assert_eq!(lru.len(), 0);

        // Asymmetric: 1 big + 2 small under 2 MiB cap.
        let mut sizes = vec![1_500_000, 300_000, 300_000];
        let mut lru = vec![1u64, 2, 3];
        evict_logic(&mut sizes, &mut lru, 2_000_000);
        assert!(sizes.iter().sum::<usize>() < 2_000_000);
        assert_eq!(lru, vec![2, 3]);
    }

    #[test]
    fn url_hash_different_urls_differ() {
        let a = url_hash("/tmp/one.png");
        let b = url_hash("/tmp/two.png");
        let c = url_hash("/tmp/one.png");
        assert_ne!(a, b);
        assert_eq!(a, c);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn src_cache_key_file_scheme_matches_raw_path() {
        let raw = "/tmp/x.png";
        let file = "file:///tmp/x.png";
        let k1 = src_cache_key(&parse_image_src(raw), raw);
        let k2 = src_cache_key(&parse_image_src(file), file);
        assert_eq!(k1, k2);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn src_cache_key_distinct_paths_differ() {
        let a = "/tmp/one.png";
        let b = "/tmp/two.png";
        let ka = src_cache_key(&parse_image_src(a), a);
        let kb = src_cache_key(&parse_image_src(b), b);
        assert_ne!(ka, kb);
    }
}
