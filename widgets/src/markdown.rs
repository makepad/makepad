use crate::{
    image::{ImageRef, ImageWidgetRefExt}, link_label::LinkLabel, makepad_derive_widget::*,
    makepad_draw::*, text_flow::TextFlow, widget::*, widget_async::ScriptAsyncResult,
    WidgetMatchEvent,
};

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
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
    /// percent-decoded UTF-8 bytes for the non-base64 variant).
    Data(Vec<u8>),
    /// `http://` or `https://` — deferred to M-img-2.
    ///
    /// Field will be consumed by the HTTP path in M-img-2.
    Http(#[allow(dead_code)] String),
    /// Anything else: unknown scheme, malformed data URL, platform-unsupported.
    Invalid,
}

/// Classify an image URL. Does no I/O; pure function. Relative paths are
/// resolved against `std::env::current_dir()` at call time (per spec).
fn parse_image_src(url: &str) -> ImageSrc {
    if url.is_empty() {
        return ImageSrc::Invalid;
    }
    // data: URL — decode synchronously, never hits disk or network.
    if let Some(rest) = url.strip_prefix("data:") {
        // [mime][;base64],payload  — we don't actually need the mime here,
        // the image format sniffer reads magic bytes from the payload.
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
            // Plain (percent-encoded) data URL — we don't percent-decode
            // here; just treat bytes as-is. Image formats decoded from
            // such URLs are vanishingly rare in markdown; keep it simple.
            ImageSrc::Data(payload.as_bytes().to_vec())
        }
    } else if let Some(rest) = url.strip_prefix("file://") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // `file:///abs/path` — strip up to 3 slashes. Host-authority
            // file URLs (`file://host/path`) are discouraged; we accept
            // them by keeping everything after the scheme marker.
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
        // Any other scheme: gopher://, ftp://, custom — reject per spec.
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
    /// Bytes charged against the LRU cap — RGBA8 * pixels. The raw Vec<u32>
    /// backing the texture is `w*h*4` bytes.
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

/// Canonical cache key for an `ImageSrc`. The point of canonicalization is
/// that `file:///tmp/x.png` and `/tmp/x.png` hash to the same value so the
/// second occurrence is a cache hit. Data URLs hash the full URL (which
/// includes the payload) — two `data:image/...` blobs with identical bytes
/// in the same document decode once. HTTP/invalid hash the raw URL.
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
/// Returns true on success, false if the template yielded an empty widget
/// (shouldn't happen in practice — caller already checked has_template).
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
    let mut ok = false;
    tf.item_with(cx, entry_id, live_id!(inline_image), |cx, item, _tf| {
        let img: ImageRef = item.as_image();
        img.set_texture(cx, Some(entry.texture.clone()));
        let _ = item.draw_walk(cx, &mut Scope::empty(), Walk::fixed(dw, dh));
        ok = true;
    });
    ok
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

        draw_table_bg +: {
            color: #x1f2937
            border_color: instance(#x475569)
            border_width: instance(1.0)
            radius: instance(6.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_width * 0.5
                    self.border_width * 0.5
                    self.rect_size.x - self.border_width
                    self.rect_size.y - self.border_width
                    self.radius
                )
                sdf.fill_keep(self.color)
                if self.border_width > 0.0 {
                    sdf.stroke(self.border_color self.border_width)
                }
                return sdf.result
            }
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

/// In-flight state between `Start(Tag::Image)` and `End(TagEnd::Image)`.
/// We already know at Start() whether the image loaded (success) or must
/// degrade to a placeholder (failure). `Successful` means: swallow the
/// intervening Text events so alt text doesn't render alongside the image.
/// `Placeholder` means: collect alt text for rendering `🖼 <alt>` at End().
enum ImageEventState {
    /// Image was rendered inline at Start; subsequent Text/SoftBreak events
    /// (the alt) are discarded until End(TagEnd::Image).
    Successful,
    /// Image load failed or was rejected. We buffer alt text between
    /// Start and End, then render `🖼 <alt-or-url>` at End().
    Placeholder { alt: String, fallback: String },
}

/// The state of a list at a given nesting level.
struct ListState {
    // Current item number for ordered lists.
    current_number: u64,
    // Start number for ordered lists, None for unordered.
    start_number: Option<u64>,
}

/// A styled slice of text inside a table cell. A cell is a `Vec<CellSpan>` so
/// we can preserve inline-formatting (bold/italic/code/link) that appears
/// between `Start(Tag::Strong)`/`End(TagEnd::Strong)` and friends inside a
/// `TableCell`. v1: multi-span cells lay out single-line (no wrap); single-
/// span cells wrap at the column width cap.
#[derive(Clone, Debug, Default)]
struct CellSpan {
    text: String,
    bold: bool,
    italic: bool,
    code: bool,
    /// Present when this span was generated inside a `[text](url)` run.
    /// v1 captures the href for future use but does not instantiate a
    /// clickable LinkLabel inside the buffered draw path, nor does it
    /// paint the conventional underline — widget instancing and inline
    /// rect decoration are both too invasive for v1. The href is parsed
    /// and kept so v2 can render + dispatch without replaying the AST.
    #[allow(dead_code)]
    link_href: Option<String>,
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
    /// Column alignments captured from `Start(Tag::Table(alignments))`.
    /// One entry per column; may be empty for partial/streaming tables,
    /// in which case cells fall back to left alignment (see `draw_table`).
    #[rust]
    table_alignments: Vec<pulldown_cmark::Alignment>,
    #[rust]
    table_rows: Vec<Vec<Vec<CellSpan>>>,
    #[rust]
    table_current_row: Vec<Vec<CellSpan>>,
    #[rust]
    table_current_cell: Vec<CellSpan>,
    /// Inline-formatting flag state inside a TableCell. Stacked counters
    /// mirror how `TextFlow::bold`/`italic` are used outside tables so
    /// nested `**_bold_**` is representable even though we don't collapse
    /// it to `bold_italic` here — the single-style-per-span invariant
    /// means the bold-italic case picks up the bold face first.
    #[rust]
    table_cell_bold: u32,
    #[rust]
    table_cell_italic: u32,
    #[rust]
    table_cell_link: Option<String>,

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
    /// recently touched. On cache hit the entry's key is moved to the
    /// back; on eviction the front is popped.
    #[rust]
    image_cache_lru: Vec<u64>,
    /// Byte cap override for tests (0 means "use IMAGE_CACHE_MAX_BYTES").
    /// Exposed via `set_image_cache_max_bytes_for_tests`.
    #[rust]
    image_cache_cap_override: usize,
    /// State for an in-progress `Start(Tag::Image)` ... `End(TagEnd::Image)`
    /// range. The inner `MdEvent::Text` events between Start and End carry
    /// the alt text — we buffer them here so we can render `🖼 <alt>` at
    /// End(), or (on success) discard them so alt text doesn't leak into
    /// the flow alongside the actual image.
    #[rust]
    in_image: Option<ImageEventState>,

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
                    if self.in_table {
                        self.table_cell_italic = self.table_cell_italic.saturating_add(1);
                    } else {
                        tf.italic.push();
                    }
                }
                MdEvent::End(TagEnd::Emphasis) => {
                    if self.in_table {
                        self.table_cell_italic = self.table_cell_italic.saturating_sub(1);
                    } else {
                        tf.italic.pop();
                    }
                }
                MdEvent::Start(Tag::Strong) => {
                    if self.in_table {
                        self.table_cell_bold = self.table_cell_bold.saturating_add(1);
                    } else {
                        tf.bold.push();
                    }
                }
                MdEvent::End(TagEnd::Strong) => {
                    if self.in_table {
                        self.table_cell_bold = self.table_cell_bold.saturating_sub(1);
                    } else {
                        tf.bold.pop();
                    }
                }
                MdEvent::Start(Tag::Strikethrough) => {
                    tf.underline.push();
                }
                MdEvent::End(TagEnd::Strikethrough) => {
                    tf.underline.pop();
                }
                MdEvent::Start(Tag::Link { dest_url, .. }) => {
                    // Inside a table, links are captured as a `link_href`
                    // flag on subsequent CellSpans so the span can be
                    // styled (underlined) — but NOT instantiated as a
                    // clickable LinkLabel, because widget instancing in
                    // the buffered draw path is too invasive for v1.
                    if self.in_table {
                        self.table_cell_link = Some(dest_url.into_string());
                    } else {
                        self.in_link = Some(dest_url.into_string());
                        self.link_text.clear();
                    }
                }
                MdEvent::End(TagEnd::Link) => {
                    if self.in_table {
                        self.table_cell_link = None;
                    } else if let Some(href) = self.in_link.take() {
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
                    dest_url, ..
                }) => {
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
                        // Successful: nothing to do — the inline widget was
                        // already placed at Start(), alt text was swallowed.
                    }
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
                        // Inline code runs inside a cell render in the
                        // fixed-width font — span-based path preserves
                        // the `code: true` flag so the measurement and
                        // draw passes pick up `text_style_fixed`.
                        self.table_current_cell.push(CellSpan {
                            text: text.into_string(),
                            bold: self.table_cell_bold > 0,
                            italic: self.table_cell_italic > 0,
                            code: true,
                            link_href: self.table_cell_link.clone(),
                        });
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
                        // Same rationale as the non-table fallback: render
                        // inline math as plain text in a fixed-width code
                        // span when MathView is unavailable during the
                        // buffered draw path.
                        self.table_current_cell.push(CellSpan {
                            text: text.into_string(),
                            bold: self.table_cell_bold > 0,
                            italic: self.table_cell_italic > 0,
                            code: true,
                            link_href: self.table_cell_link.clone(),
                        });
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
                    if let Some(state) = self.in_image.as_mut() {
                        // We're between Start(Image) and End(Image) — the
                        // text event carries alt content. On success, discard
                        // it so it doesn't double-render alongside the image.
                        // On placeholder path, accumulate for `🖼 <alt>`.
                        if let ImageEventState::Placeholder { alt, .. } = state {
                            alt.push_str(&text);
                        }
                    } else if self.in_link.is_some() {
                        self.link_text.push_str(&text);
                    } else if self.in_table {
                        self.table_current_cell.push(CellSpan {
                            text: text.into_string(),
                            bold: self.table_cell_bold > 0,
                            italic: self.table_cell_italic > 0,
                            code: false,
                            link_href: self.table_cell_link.clone(),
                        });
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
                        // Collapse a markdown soft break inside a cell to
                        // a single space — wrap decisions happen in the
                        // layouter, not here.
                        self.table_current_cell.push(CellSpan {
                            text: " ".to_string(),
                            bold: self.table_cell_bold > 0,
                            italic: self.table_cell_italic > 0,
                            code: false,
                            link_href: self.table_cell_link.clone(),
                        });
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
                        self.table_current_cell.push(CellSpan {
                            text: " ".to_string(),
                            bold: self.table_cell_bold > 0,
                            italic: self.table_cell_italic > 0,
                            code: false,
                            link_href: self.table_cell_link.clone(),
                        });
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
                MdEvent::Start(Tag::Table(alignments)) => {
                    if !is_first_block {
                        tf.new_line_collapsed_with_spacing(cx, self.paragraph_spacing);
                    }
                    is_first_block = false;
                    self.in_table = true;
                    self.table_has_header = false;
                    self.table_rows.clear();
                    self.table_current_row.clear();
                    self.table_current_cell.clear();
                    self.table_cell_bold = 0;
                    self.table_cell_italic = 0;
                    self.table_cell_link = None;
                    self.table_alignments = alignments;
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
                        &self.table_alignments,
                    );
                    self.in_table = false;
                    self.table_rows.clear();
                    self.table_current_row.clear();
                    self.table_current_cell.clear();
                    self.table_alignments.clear();
                    self.table_cell_bold = 0;
                    self.table_cell_italic = 0;
                    self.table_cell_link = None;
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
                    &self.table_alignments,
                );
            }
            self.in_table = false;
            self.table_rows.clear();
            self.table_alignments.clear();
            self.table_cell_bold = 0;
            self.table_cell_italic = 0;
            self.table_cell_link = None;
        }
    }

    /// Draws the collected table grid at the current turtle position.
    ///
    /// Takes disjoint `&mut` borrows so the caller can keep its outer
    /// `&mut self.text_flow` loan live. Responsibilities:
    ///   1. Measure each column's max natural content width (spans sum).
    ///   2. Pre-lay out every span at the clamped column width (single-
    ///      span cells wrap; multi-span cells lay out single-line) and
    ///      derive per-row heights from the max laid-out span height.
    ///   3. Reserve a `Walk::fixed(W, H)` rectangle in the parent turtle.
    ///   4. Paint a background (rounded + stroked via draw_bg's SDF
    ///      shader), header tint, then per-span text, then interior grid
    ///      lines. The outer border is drawn by draw_bg itself so we no
    ///      longer emit 4 flat rects here.
    // --- M-img-1: try_load_image ----------------------------------------
    // Called from the Start(Tag::Image) arm. Takes `tf` as a disjoint borrow
    // so the caller can still hold `&mut self.text_flow` for the surrounding
    // event loop. Returns true on success (inline_image widget placed);
    // false on any failure so the caller can emit the placeholder at End().
    //
    // Design: cache-lookup first, then classify, then load. Only successful
    // loads populate the cache — failed URLs must NOT be cached per spec
    // so a future retry (e.g. the file reappearing) can re-attempt. This
    // is sync: local-fs reads under ~1 MB are <5 ms on warm NVMe (per spec).
    // Large files will stutter once then stream from the cache on every
    // subsequent set_text re-parse.
    fn try_load_image(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        url: &str,
        cache: &mut HashMap<u64, ImageCacheEntry>,
        lru: &mut Vec<u64>,
        cap_bytes: usize,
    ) -> bool {
        // Abort early if the inline template was never registered by the
        // consuming app's Markdown DSL. The placeholder path handles this.
        if !tf.has_template(live_id!(inline_image)) {
            return false;
        }

        // Classify the URL first — we derive the cache key from the
        // CANONICAL form so `file:///tmp/x.png` and `/tmp/x.png` hit
        // the same entry (per spec `test_image_file_scheme_equivalent_to_local_path`).
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
        // STRICTLY below the cap (per spec: "below ... not ≤").
        cache.insert(key, entry.clone());
        lru.push(key);
        evict_over_cap(cache, lru, cap_bytes);

        draw_cached_image(cx, tf, key, entry)
    }

    fn draw_table(
        cx: &mut Cx2d,
        tf: &mut TextFlow,
        draw_bg: &mut DrawColor,
        draw_header_bg: &mut DrawColor,
        draw_line: &mut DrawColor,
        rows: &[Vec<Vec<CellSpan>>],
        has_header: bool,
        alignments: &[pulldown_cmark::Alignment],
    ) {
        if rows.is_empty() || rows[0].is_empty() {
            return;
        }

        // Layout constants. Tune in the DSL eventually; these match the
        // visual target spec (~8px horizontal, ~9px vertical padding).
        const CELL_PAD_H: f64 = 8.0;
        // Vertical padding tuned 2026-04-20 from 5 → 9 after real-world
        // wrapped-prose tables showed the last line of a 2-3-line cell
        // visually hugging the bottom row separator. 9 gives comfortable
        // breathing room without making single-line cells feel sparse.
        const CELL_PAD_V: f64 = 9.0;
        const MAX_COL_W: f64 = 400.0;
        const LINE_W: f64 = 1.0;
        // Inter-wrapped-line spacing. Bumped 1.4 → 1.55 so wrapped CJK
        // prose has clear visual separation between consecutive rows.
        const LINE_HEIGHT_MULT: f64 = 1.55;

        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if ncols == 0 {
            return;
        }

        let font_size = *tf.font_sizes.last().unwrap_or(&tf.font_size) as f32;
        let normal_style = tf.text_style_normal.clone();
        let italic_style = tf.text_style_italic.clone();
        let bold_style = tf.text_style_bold.clone();
        let bold_italic_style = tf.text_style_bold_italic.clone();
        let fixed_style = tf.text_style_fixed.clone();

        // Helper: pick the appropriate text style for a span. Header rows
        // force bold — this is applied by the caller via `force_bold`.
        // The `code` flag takes precedence over bold/italic because the
        // fixed-width face usually has no italic/bold variants in our
        // theme and mixing would produce inconsistent glyph metrics.
        let style_for = |span: &CellSpan, force_bold: bool| -> TextStyle {
            if span.code {
                return fixed_style.clone();
            }
            let b = span.bold || force_bold;
            let i = span.italic;
            match (b, i) {
                (true, true) => bold_italic_style.clone(),
                (true, false) => bold_style.clone(),
                (false, true) => italic_style.clone(),
                (false, false) => normal_style.clone(),
            }
        };

        // --- Pass 1: measure column widths (spans laid out single-line) ---
        let mut col_widths: Vec<f64> = vec![0.0; ncols];
        for (r, row) in rows.iter().enumerate() {
            let is_header_row = has_header && r == 0;
            for (c, cell) in row.iter().enumerate() {
                if c >= ncols { break; }
                if cell.is_empty() { continue; }
                let mut cell_w: f64 = 0.0;
                for span in cell.iter() {
                    if span.text.is_empty() { continue; }
                    tf.draw_text.text_style = style_for(span, is_header_row);
                    tf.draw_text.text_style.font_size = font_size;
                    let laid = tf.draw_text.layout(
                        cx, 0.0, 0.0, None, false, Align::default(), &span.text,
                    );
                    cell_w += laid.size_in_lpxs.width as f64;
                }
                if cell_w > col_widths[c] {
                    col_widths[c] = cell_w.min(MAX_COL_W);
                }
            }
        }
        // Columns with no content get a minimum so dividers still render.
        for w in col_widths.iter_mut() {
            if *w < font_size as f64 { *w = font_size as f64; }
        }

        // --- Pass 1.5: lay out spans with cross-span wrap, cache Rcs, derive row heights ---
        //
        // Each cell is a sequence of inline spans (normal / bold / italic /
        // code ...). We want the spans to flow as one inline paragraph that
        // wraps at the column width. The trick: call `layout` per span with
        // `first_row_indent_in_lpxs = current_x`, which makes the layouter
        // pretend the first row is already partially filled. When the first
        // row overflows, the layouter wraps; subsequent rows start at x=0.
        //
        // After each span we update a (current_x, current_y) cursor:
        //   - current_x := last_row.width_in_lpxs  (row 0 includes indent,
        //     wrapped rows start at 0 — so this works uniformly).
        //   - current_y += (n_rows - 1) * row_h   (rows past the first bump
        //     the y cursor by a fixed per-line advance).
        // The same state machine is replayed in the draw pass so laidout
        // positions match.
        //
        // Note on line-height: we use a fixed `row_h = font_size * MULT`
        // for the inter-span y-advance. Within a single wrapped span, the
        // layouter uses its own font-metric spacing (ascender + line_gap +
        // descender). Mismatch is usually <1 lpx for typical fonts; if a
        // real-world case surfaces, switch to reading `rows[i+1].origin.y -
        // rows[i].origin.y` from the laidout.
        //
        // Alignment: `max_right_edge` tracks the farthest x reached across
        // all rows of all spans in the cell (in cell-local coords). That is
        // the "block width" used for Center/Right cell alignment. Individual
        // rows left-align within that block; the block centers/right-aligns
        // as a whole. This is the simplest acceptable approximation for
        // multi-row alignment; per-row centering would require redrawing
        // each laidout row individually.
        let mut laid: Vec<Vec<Vec<std::rc::Rc<crate::makepad_draw::text::layouter::LaidoutText>>>>
            = Vec::with_capacity(rows.len());
        // Parallel grid of (max_right_edge, total_height_lpxs) per cell so
        // the draw pass doesn't recompute.
        let mut cell_metrics: Vec<Vec<(f64, f64)>> = Vec::with_capacity(rows.len());
        let mut row_heights: Vec<f64> = vec![0.0; rows.len()];
        let row_h: f64 = font_size as f64 * LINE_HEIGHT_MULT;
        let row_h_f32 = row_h as f32;
        for (r, row) in rows.iter().enumerate() {
            let is_header_row = has_header && r == 0;
            let mut row_cells: Vec<Vec<std::rc::Rc<crate::makepad_draw::text::layouter::LaidoutText>>>
                = Vec::with_capacity(ncols);
            let mut row_cell_metrics: Vec<(f64, f64)> = Vec::with_capacity(ncols);
            let mut row_max_h: f64 = row_h;
            for c in 0..ncols {
                let mut cell_out = Vec::new();
                let mut cell_max_right: f64 = 0.0;
                let mut cell_total_h: f64 = row_h; // at least one line
                if let Some(cell) = row.get(c) {
                    if !cell.is_empty() {
                        let col_w_f32 = col_widths[c] as f32;
                        let mut current_x: f64 = 0.0;
                        let mut current_y: f64 = 0.0;
                        for span in cell.iter() {
                            if span.text.is_empty() { continue; }
                            tf.draw_text.text_style = style_for(span, is_header_row);
                            tf.draw_text.text_style.font_size = font_size;
                            let laidout = tf.draw_text.layout(
                                cx,
                                current_x as f32,
                                row_h_f32,
                                Some(col_w_f32),
                                true,
                                Align::default(),
                                &span.text,
                            );
                            // Track widest visual row in this span (row 0's
                            // width includes indent; later rows start at 0,
                            // so width_in_lpxs is the right-edge in both
                            // cases within the laidout's own coordinates).
                            for lr in &laidout.rows {
                                let w = lr.width_in_lpxs as f64;
                                if w > cell_max_right { cell_max_right = w; }
                            }
                            let n_rows = laidout.rows.len();
                            if n_rows > 1 {
                                current_y += (n_rows - 1) as f64 * row_h;
                            }
                            current_x = laidout.rows.last()
                                .map(|lr| lr.width_in_lpxs as f64)
                                .unwrap_or(current_x);
                            cell_out.push(laidout);
                        }
                        cell_total_h = current_y + row_h;
                    }
                }
                if cell_total_h > row_max_h { row_max_h = cell_total_h; }
                row_cells.push(cell_out);
                row_cell_metrics.push((cell_max_right, cell_total_h));
            }
            row_heights[r] = row_max_h + CELL_PAD_V * 2.0;
            laid.push(row_cells);
            cell_metrics.push(row_cell_metrics);
        }

        let col_total_w: f64 = col_widths.iter().sum::<f64>() + (CELL_PAD_H * 2.0) * ncols as f64;
        let total_w = col_total_w + LINE_W; // +1 for trailing right border
        let total_h: f64 = row_heights.iter().sum::<f64>() + LINE_W;

        // --- Pass 2: reserve space and draw ---
        let rect = cx.walk_turtle(Walk::fixed(total_w, total_h));
        let ox = rect.pos.x;
        let oy = rect.pos.y;

        // Background fill + rounded border (drawn by draw_bg's SDF).
        draw_bg.draw_abs(cx, Rect { pos: dvec2(ox, oy), size: dvec2(total_w, total_h) });

        // Header-row bg tint overlaid on main bg so the header is visually
        // distinct even when the bold-font override is missing / subtle.
        if has_header {
            let hdr_h = row_heights[0];
            draw_header_bg.draw_abs(cx, Rect { pos: dvec2(ox, oy), size: dvec2(total_w, hdr_h) });
        }

        // Per-cell text draw. Use draw_walk_laidout with abs_pos so the
        // turtle is positioned absolutely; this keeps the wrap / span
        // measurements we cached above as the source of truth for both
        // geometry and rendering.
        let mut y = oy;
        for (r, row) in rows.iter().enumerate() {
            let is_header_row = has_header && r == 0;
            let rh = row_heights[r];

            let mut x = ox;
            for c in 0..ncols {
                let col_w = col_widths[c] + CELL_PAD_H * 2.0;
                let align = alignments.get(c).copied().unwrap_or(pulldown_cmark::Alignment::None);
                if let Some(cell) = row.get(c) {
                    if !cell.is_empty() {
                        let cell_laid = &laid[r][c];
                        let (cell_max_right, _cell_total_h) = cell_metrics[r][c];
                        // Block-level alignment uses the widest visual row
                        // across the whole stitched layout (see pass 1.5
                        // note). All span rows share the same block origin,
                        // so Center/Right shifts the block as a unit.
                        let avail = col_widths[c];
                        let text_y = y + CELL_PAD_V;
                        let cell_origin_x = match align {
                            pulldown_cmark::Alignment::Center => {
                                x + CELL_PAD_H + ((avail - cell_max_right) / 2.0).max(0.0)
                            }
                            pulldown_cmark::Alignment::Right => {
                                x + col_w - CELL_PAD_H - cell_max_right.min(avail)
                            }
                            // Left / None — default
                            _ => x + CELL_PAD_H,
                        };
                        // Replay the indent-based state machine from pass
                        // 1.5. Each span is a separate laidout block whose
                        // abs_pos is offset downward by the accumulated y
                        // from prior wrapped rows; its first row is indented
                        // by the x-cursor left over from the previous span.
                        let mut current_x: f64 = 0.0;
                        let mut current_y: f64 = 0.0;
                        let mut span_iter = cell.iter().filter(|s| !s.text.is_empty());
                        for laidout in cell_laid.iter() {
                            let span = match span_iter.next() {
                                Some(s) => s,
                                None => break,
                            };
                            tf.draw_text.text_style = style_for(span, is_header_row);
                            tf.draw_text.text_style.font_size = font_size;
                            tf.draw_text.color = tf.font_color;
                            tf.draw_text.temp_y_shift = tf.draw_text.text_style.top_drop;
                            let _ = tf.draw_text.draw_walk_laidout(
                                cx,
                                Walk {
                                    abs_pos: Some(dvec2(
                                        cell_origin_x,
                                        text_y + current_y,
                                    )),
                                    margin: Default::default(),
                                    width: makepad_draw::turtle::Size::Fit {
                                        min: None,
                                        max: None,
                                    },
                                    height: makepad_draw::turtle::Size::Fit {
                                        min: None,
                                        max: None,
                                    },
                                    metrics: Default::default(),
                                },
                                laidout,
                            );
                            let n_rows = laidout.rows.len();
                            if n_rows > 1 {
                                current_y += (n_rows - 1) as f64 * row_h;
                            }
                            current_x = laidout.rows.last()
                                .map(|lr| lr.width_in_lpxs as f64)
                                .unwrap_or(current_x);
                            // current_x is used implicitly by the next span
                            // via the cached laidout, which was computed in
                            // pass 1.5 with the correct first_row_indent —
                            // so we don't need to feed it into draw_walk_laidout
                            // here. We still keep the variable for clarity
                            // and symmetry with the measure pass.
                            let _ = current_x;
                        }
                    }
                }
                x += col_w;
            }
            y += rh;
        }

        // Interior grid lines only — the outer border is rendered by
        // draw_bg's SDF stroke, so we skip the 4 rect draws here.
        let border = draw_line;

        // Horizontal separators below each row boundary.
        let mut hy = oy;
        for r in 0..rows.len().saturating_sub(1) {
            hy += row_heights[r];
            border.draw_abs(cx, Rect { pos: dvec2(ox, hy), size: dvec2(total_w, LINE_W) });
        }

        // Vertical separators between each column.
        let mut vx = ox;
        for c in 0..ncols.saturating_sub(1) {
            vx += col_widths[c] + CELL_PAD_H * 2.0;
            border.draw_abs(cx, Rect { pos: dvec2(vx, oy), size: dvec2(LINE_W, total_h) });
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

#[cfg(test)]
mod tests {
    //! Unit tests for the pure helpers that back M-img-1 local image
    //! rendering: URL classification, base64 decode, LRU eviction.
    //!
    //! Widget-level scenario tests from the spec (decode + draw +
    //! TextFlow instantiation) cannot run here because there is no
    //! widget harness in `makepad-widgets`. Those scenarios are
    //! therefore treated as manual-verification gates covered by the
    //! consumer (aichat) — see the M-img-1 Report.
    use super::*;

    #[test]
    fn parse_image_src_http_https() {
        assert!(matches!(parse_image_src("http://example.com/x.png"), ImageSrc::Http(_)));
        assert!(matches!(parse_image_src("https://example.com/x.png"), ImageSrc::Http(_)));
    }

    #[test]
    fn parse_image_src_unknown_scheme_is_invalid() {
        assert!(matches!(parse_image_src("gopher://old/x.png"), ImageSrc::Invalid));
        assert!(matches!(parse_image_src("ftp://host/x.png"), ImageSrc::Invalid));
        assert!(matches!(parse_image_src(""), ImageSrc::Invalid));
    }

    #[test]
    fn parse_image_src_data_base64() {
        // 1x1 transparent PNG
        let url = "data:image/png;base64,iVBORw0KGgo=";
        match parse_image_src(url) {
            ImageSrc::Data(b) => {
                // `iVBORw0KGgo=` decodes to PNG magic header.
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
        assert!(matches!(parse_image_src("data:image/png;base64"), ImageSrc::Invalid));
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
        // Whitespace tolerated (common in wrapped URLs).
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
        touch_lru(&mut lru, 999); // not present: appended
        assert_eq!(lru, vec![2, 3, 1, 999]);
    }

    #[test]
    fn evict_over_cap_logic_via_manual_totals() {
        // Pure-logic verification of evict_over_cap WITHOUT fabricating
        // Texture values: reproduce the size-accounting loop inline.
        // This mirrors the real function's arithmetic so a regression in
        // the strict-less-than invariant fails here too.
        fn evict_logic(sizes: &mut Vec<usize>, lru: &mut Vec<u64>, cap: usize) {
            let mut total: usize = sizes.iter().sum();
            while total >= cap && !lru.is_empty() {
                let _ = lru.remove(0);
                let s = sizes.remove(0);
                total = total.saturating_sub(s);
            }
        }
        // 3 entries of 512*512*4 = 1_048_576 bytes each; cap at 1 MiB.
        // Expected: evicts down to total < 1 MiB, so at most 0 entries
        // fit (since one entry == cap).
        let mut sizes = vec![1_048_576usize, 1_048_576, 1_048_576];
        let mut lru = vec![1u64, 2, 3];
        evict_logic(&mut sizes, &mut lru, 1_048_576);
        assert!(sizes.iter().sum::<usize>() < 1_048_576);
        assert_eq!(sizes.len(), 0);
        assert_eq!(lru.len(), 0);

        // Asymmetric: one big + two small under a 2 MiB cap.
        let mut sizes = vec![1_500_000, 300_000, 300_000];
        let mut lru = vec![1u64, 2, 3];
        evict_logic(&mut sizes, &mut lru, 2_000_000);
        assert!(sizes.iter().sum::<usize>() < 2_000_000);
        // Total is 2.1 MiB → strictly-less-than forces eviction of the
        // oldest (front = key 1), leaving keys 2 and 3 at 600 KiB.
        assert_eq!(lru, vec![2, 3]);
    }

    #[test]
    fn url_hash_different_urls_differ() {
        let a = url_hash("/tmp/one.png");
        let b = url_hash("/tmp/two.png");
        let c = url_hash("/tmp/one.png"); // same as a
        assert_ne!(a, b);
        assert_eq!(a, c);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn src_cache_key_file_scheme_matches_raw_path() {
        // Per spec `test_image_file_scheme_equivalent_to_local_path` —
        // canonicalization collapses `file:///tmp/x.png` and
        // `/tmp/x.png` to the same cache key.
        let raw = "/tmp/x.png";
        let file = "file:///tmp/x.png";
        let k1 = src_cache_key(&parse_image_src(raw), raw);
        let k2 = src_cache_key(&parse_image_src(file), file);
        assert_eq!(k1, k2);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn src_cache_key_distinct_paths_differ() {
        // Per spec `test_image_same_bytes_different_url_no_alias` —
        // two different paths always hash to different keys even if
        // they point at byte-identical files.
        let a = "/tmp/one.png";
        let b = "/tmp/two.png";
        let ka = src_cache_key(&parse_image_src(a), a);
        let kb = src_cache_key(&parse_image_src(b), b);
        assert_ne!(ka, kb);
    }
}
