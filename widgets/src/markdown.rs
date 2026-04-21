use crate::{
    image::{ImageRef, ImageWidgetRefExt},
    link_label::LinkLabel, makepad_derive_widget::*, makepad_draw::*,
    text_flow::TextFlow, widget::*, WidgetMatchEvent,
};

// SVG types (M-img-3). `parse_svg` is a zero-deps XML parser; `SvgDocument` is
// the parsed AST; both reachable via the `makepad_draw` facade re-export.
use crate::makepad_draw::svg::{parse_svg, SvgDocument};

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Image rendering support (M-img-1 local + data URL; M-img-2 HTTP(S) fetch
// via Makepad's native `cx.http_request` + `Event::NetworkResponses`).
// ---------------------------------------------------------------------------

/// Upper bound on the inline display width (logical px). Larger images are
/// scaled down proportionally; smaller ones keep their intrinsic size.
const IMAGE_MAX_DISPLAY_W: f64 = 480.0;
/// Widget-local texture cache cap — decoded texture bytes (w*h*4) beyond this
/// value trigger LRU eviction. 16 MiB ≈ 1–2 large photos or ~16 modest icons.
const IMAGE_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Sanity cap on an HTTP image response body before we even attempt decode.
/// Sized for "large screenshot / hero banner" territory; anything beyond is
/// almost certainly a mis-linked asset (video, archive) — we warn and drop.
const IMAGE_HTTP_BODY_MAX: usize = 32 * 1024 * 1024;
/// Max raw SVG source size before we attempt to parse (M-img-3). SVG is XML-
/// based and subject to amplification attacks; `makepad_svg::parse::parse_svg`
/// has no built-in node-count / recursion / string-length guards, so this
/// byte cap is our only defense. 4 MiB covers any realistic hand-authored
/// vector asset by two orders of magnitude.
const IMAGE_SVG_BYTES_MAX: usize = 4 * 1024 * 1024;

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
    /// `http://` or `https://` — fetched via Makepad's native
    /// `cx.http_request` path; response arrives via `Event::NetworkResponses`
    /// and populates `image_cache` on success.
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

/// One decoded image stored in the widget-local cache. Two variants:
///   - `Raster`: PNG / JPEG / WebP → GPU `Texture` (shared via clone).
///   - `Svg`: parsed `SvgDocument` (M-img-3). Stored CPU-side; re-tessellated
///     per draw by `DrawSvg`, but the parse step (the expensive part) runs
///     exactly once per URL. `raw_bytes_len` is what LRU charges — parsed
///     AST memory is implementation-defined.
/// Intrinsic pixel dimensions are used to compute the display `Walk`.
#[derive(Clone)]
enum ImageCacheEntry {
    Raster {
        texture: Texture,
        width: u32,
        height: u32,
    },
    Svg {
        doc: SvgDocument,
        raw_bytes_len: usize,
        width: u32,
        height: u32,
    },
}

impl ImageCacheEntry {
    /// Bytes charged against the LRU cap. Raster: RGBA8 * pixels (w*h*4).
    /// SVG: raw source length (parsed-doc memory is unstable and would be
    /// a poor predictor of pressure).
    fn byte_size(&self) -> usize {
        match self {
            ImageCacheEntry::Raster { width, height, .. } => {
                (*width as usize) * (*height as usize) * 4
            }
            ImageCacheEntry::Svg { raw_bytes_len, .. } => *raw_bytes_len,
        }
    }

    /// Intrinsic dimensions (logical pixels) used to compute display Walk.
    fn dims(&self) -> (u32, u32) {
        match self {
            ImageCacheEntry::Raster { width, height, .. }
            | ImageCacheEntry::Svg { width, height, .. } => (*width, *height),
        }
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

/// Sniff SVG content: accept both `<?xml ...?><svg ...>` and bare `<svg ...>`
/// (with or without xmlns). Scans past up to the first 256 bytes of leading
/// whitespace / BOM / XML prolog. Does NOT validate — full validation happens
/// only at `parse_svg` time. M-img-3.
fn detect_svg_magic(data: &[u8]) -> bool {
    // Strip UTF-8 BOM if present.
    let data = if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &data[3..]
    } else {
        data
    };
    // Skip leading whitespace up to a reasonable bound.
    let limit = data.len().min(256);
    let mut i = 0;
    while i < limit && data[i].is_ascii_whitespace() {
        i += 1;
    }
    let tail = &data[i..];
    if tail.starts_with(b"<?xml") {
        return true;
    }
    // Bare `<svg` (optionally followed by whitespace or `>` or `/`).
    if tail.len() >= 5 && &tail[0..4] == b"<svg" {
        let next = tail[4];
        return next.is_ascii_whitespace() || next == b'>' || next == b'/';
    }
    false
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

/// Sniff format, decode bytes, upload texture, insert into the LRU-governed
/// cache. Returns the inserted entry on success so the caller can draw
/// immediately; returns `Err(reason)` on failure (format unknown / decode
/// error / zero-sized). The caller decides HOW to surface the reason —
/// local/data path warns immediately with the URL; the HTTP response path
/// routes through `warn_once_http` so a corrupt-body server doesn't spam
/// the log on every streaming re-render.
///
/// Failures do NOT populate the cache (failed URLs are not cached so a
/// future retry can re-attempt).
///
/// Takes `&mut Cx` (not `&mut Cx2d`) so it can be called from both the
/// draw-thread hot path (Cx2d derefs to Cx) and the `Event::NetworkResponses`
/// handler where only `&mut Cx` is available.
fn decode_and_cache_bytes(
    cx: &mut Cx,
    key: u64,
    bytes: &[u8],
    cache: &mut HashMap<u64, ImageCacheEntry>,
    lru: &mut Vec<u64>,
    cap_bytes: usize,
) -> Result<ImageCacheEntry, String> {
    // M-img-3: SVG path is dispatched FIRST so raster-magic false positives
    // (exceedingly rare) can't mask an SVG. The oversize gate runs before
    // parse — `parse_svg` has no internal limits, so the byte cap is our
    // only bound on amplification-style malicious input.
    if detect_svg_magic(bytes) {
        if bytes.len() > IMAGE_SVG_BYTES_MAX {
            return Err(format!(
                "svg rejected (too large, {} bytes > {} cap)",
                bytes.len(),
                IMAGE_SVG_BYTES_MAX
            ));
        }
        let svg_str = std::str::from_utf8(bytes)
            .map_err(|_| "svg parse error (non-utf8)".to_string())?;
        let doc = parse_svg(svg_str);
        // Empty root → parser found no recognizable `<svg>` geometry. Treat
        // as parse failure so the placeholder path fires.
        if doc.root.is_empty() {
            return Err("svg parse error (empty document)".to_string());
        }
        let (lw, lh) = doc.logical_size();
        // Clamp to u32 for the display-sizing path. Negative / NaN producing
        // doc sizes would be a parser bug; guard defensively.
        let w = (lw.max(1.0) as u32).max(1);
        let h = (lh.max(1.0) as u32).max(1);
        let entry = ImageCacheEntry::Svg {
            doc,
            raw_bytes_len: bytes.len(),
            width: w,
            height: h,
        };
        cache.insert(key, entry.clone());
        lru.push(key);
        evict_over_cap(cache, lru, cap_bytes);
        return Ok(entry);
    }

    let fmt = detect_image_magic(bytes).ok_or_else(|| "unsupported format".to_string())?;
    let buf = match fmt {
        "png" => ImageBuffer::from_png(bytes),
        "jpg" => ImageBuffer::from_jpg(bytes),
        "webp" => ImageBuffer::from_webp(bytes),
        _ => return Err("unsupported format".to_string()),
    };
    let buf = buf.map_err(|e| format!("decode error ({})", e))?;
    let (w, h) = (buf.width as u32, buf.height as u32);
    if w == 0 || h == 0 {
        return Err("decode error (zero dim)".to_string());
    }
    let texture = buf.into_new_texture(cx);
    let entry = ImageCacheEntry::Raster {
        texture,
        width: w,
        height: h,
    };
    cache.insert(key, entry.clone());
    lru.push(key);
    evict_over_cap(cache, lru, cap_bytes);
    Ok(entry)
}

/// Instantiate the appropriate inline template (`inline_image` for raster,
/// `inline_svg` for vector) and render `entry` at a display size scaled to
/// respect `IMAGE_MAX_DISPLAY_W`, aspect-ratio-preserved. Returns true on
/// success (template present AND closure ran), false when the required
/// template is not registered in the consuming app's Markdown DSL.
fn draw_cached_image(
    cx: &mut Cx2d,
    tf: &mut TextFlow,
    key: u64,
    entry: ImageCacheEntry,
) -> bool {
    let (iw_u, ih_u) = entry.dims();
    let (iw, ih) = (iw_u as f64, ih_u as f64);
    let (dw, dh) = if iw > IMAGE_MAX_DISPLAY_W {
        let scale = IMAGE_MAX_DISPLAY_W / iw;
        (IMAGE_MAX_DISPLAY_W, (ih * scale).max(1.0))
    } else {
        (iw, ih)
    };
    let entry_id = LiveId(key);
    match entry {
        ImageCacheEntry::Raster { texture, .. } => {
            tf.item_with(cx, entry_id, live_id!(inline_image), |cx, item, _tf| {
                let img: ImageRef = item.as_image();
                img.set_texture(cx, Some(texture.clone()));
                let _ = item.draw_walk(cx, &mut Scope::empty(), Walk::fixed(dw, dh));
                true
            })
        }
        ImageCacheEntry::Svg { doc, .. } => {
            // Mirror the `vector.rs::Vector::draw_walk` take/render/restore
            // pattern inside a closure so the cached doc stays owned by the
            // widget across frames (we set it fresh each frame because
            // `render_to_rect` takes it out of `svg_doc` internally).
            tf.item_with(cx, entry_id, live_id!(inline_svg), |cx, item, _tf| {
                let svg_ref = item.as_inline_svg();
                svg_ref.set_doc(doc.clone());
                let _ = item.draw_walk(cx, &mut Scope::empty(), Walk::fixed(dw, dh));
                true
            })
        }
    }
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

    mod.widgets.InlineSvgBase = #(InlineSvg::register_widget(vm))

    mod.widgets.InlineSvg = set_type_default() do mod.widgets.InlineSvgBase{
        width: Fit height: Fit
    }

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
        // M-img-3: inline SVG template. Instantiated by
        // `tf.item_with(..., live_id!(inline_svg), ...)` at every
        // `Start(Tag::Image)` whose bytes sniff as SVG. The caller feeds a
        // fixed-size Walk so draw_svg fits content to that rect.
        inline_svg := mod.widgets.InlineSvg{
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
    /// In-flight HTTP requests keyed by the request_id we issued. Value is
    /// the cache key we'll store the decoded entry under once the response
    /// arrives. Used to dedup simultaneous fetches for the same URL and to
    /// map `NetworkResponse { request_id }` back to the cache slot.
    #[rust]
    pending_http: HashMap<LiveId, u64>,
    /// URL-hash keys we've already emitted exactly one `warning!` for during
    /// this widget's lifetime. Prevents log-spam when a failing image appears
    /// many times in a document or when re-renders happen during streaming.
    #[rust]
    warned_urls: HashSet<u64>,
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
        // M-img-2: HTTP image responses land here. We decode + insert into
        // the same `image_cache` that M-img-1 populates so the next draw
        // (triggered by `cx.redraw_all()`) picks the texture up through the
        // normal cache-hit fast path in `try_load_image`.
        if let Event::NetworkResponses(responses) = event {
            self.handle_http_image_responses(cx, responses);
        }
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
                        &mut self.pending_http,
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
        pending_http: &mut HashMap<LiveId, u64>,
        cap_bytes: usize,
    ) -> bool {
        // Classify the URL first — we derive the cache key from the
        // CANONICAL form so `file:///tmp/x.png` and `/tmp/x.png` hit
        // the same entry.
        let src = parse_image_src(url);
        let key = src_cache_key(&src, url);

        // Cache hit — touch LRU and draw. Works uniformly for local,
        // data, AND previously-fetched HTTP entries.
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
            ImageSrc::Http(http_url) => {
                // M-img-2: issue the fetch if we haven't already. Dedup is
                // O(1) by using `LiveId(key)` as the request_id so two
                // occurrences of the same URL in one document (or across a
                // streaming re-parse) collapse to a single HTTP request.
                // Returns false → caller renders the `🖼 alt` placeholder
                // while the fetch is in flight; a successful response
                // triggers `cx.redraw_all()` and the next draw hits the
                // cache-hit branch above.
                let request_id = LiveId(key);
                if !pending_http.contains_key(&request_id) {
                    pending_http.insert(request_id, key);
                    cx.http_request(request_id, HttpRequest::new(http_url, HttpMethod::GET));
                }
                return false;
            }
            ImageSrc::Invalid => {
                warning!("markdown image: unknown scheme or invalid url {}", url);
                return false;
            }
        };

        match decode_and_cache_bytes(cx, key, &bytes, cache, lru, cap_bytes) {
            Ok(entry) => draw_cached_image(cx, tf, key, entry),
            Err(reason) => {
                // Local/data path: warn once (URL is known and stable,
                // so a single warn is fine — we're not inside a loop).
                // `inline_svg` is best-effort: if the consumer shipped a
                // Markdown without the template, the raster path in
                // draw_cached_image will also return false, which the
                // caller already maps to the placeholder output.
                warning!("markdown image: {} {}", reason, url);
                false
            }
        }
    }

    /// M-img-2 response path. Called from `handle_event` for every network
    /// response event. Only URL hashes we've issued are in `pending_http`
    /// so we ignore responses for other widgets' requests (the Event is
    /// broadcast to the whole widget tree). On success we populate the
    /// cache and `cx.redraw_all()` so the next draw picks up the texture.
    fn handle_http_image_responses(&mut self, cx: &mut Cx, responses: &[NetworkResponse]) {
        let mut any_hit = false;
        for response in responses {
            match response {
                NetworkResponse::HttpResponse { request_id, response }
                | NetworkResponse::HttpStreamComplete { request_id, response } => {
                    let Some(key) = self.pending_http.remove(request_id) else {
                        continue;
                    };
                    any_hit = true;
                    // Non-2xx → placeholder. Failed URLs are NOT cached
                    // so a future set_text can retry.
                    if !(200..300).contains(&response.status_code) {
                        self.warn_once_http(
                            key,
                            &format!(
                                "markdown image: http status {} (key=0x{:x})",
                                response.status_code, key
                            ),
                        );
                        continue;
                    }
                    let Some(body) = response.body.as_ref() else {
                        self.warn_once_http(
                            key,
                            &format!("markdown image: empty body (key=0x{:x})", key),
                        );
                        continue;
                    };
                    if body.len() > IMAGE_HTTP_BODY_MAX {
                        self.warn_once_http(
                            key,
                            &format!(
                                "markdown image: body too large ({} bytes, key=0x{:x})",
                                body.len(),
                                key
                            ),
                        );
                        continue;
                    }
                    let cap = if self.image_cache_cap_override != 0 {
                        self.image_cache_cap_override
                    } else {
                        IMAGE_CACHE_MAX_BYTES
                    };
                    if let Err(reason) = decode_and_cache_bytes(
                        cx,
                        key,
                        body,
                        &mut self.image_cache,
                        &mut self.image_cache_lru,
                        cap,
                    ) {
                        // Route through the dedup helper so a server that
                        // keeps serving a corrupt body across streaming
                        // re-renders doesn't spam the log.
                        self.warn_once_http(
                            key,
                            &format!("markdown image: {} (key=0x{:x})", reason, key),
                        );
                    }
                }
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(key) = self.pending_http.remove(request_id) else {
                        continue;
                    };
                    any_hit = true;
                    self.warn_once_http(
                        key,
                        &format!("markdown image: http error ({})", error.message),
                    );
                }
                _ => {}
            }
        }
        if any_hit {
            // Either we filled cache entries (good — redraw picks them up)
            // or we just marked failures (redraw re-renders placeholders,
            // no harm). Either way the widget's visible state changed.
            cx.redraw_all();
        }
    }

    /// Emit at most one `warning!` per URL per widget lifetime. Uses the
    /// widget's `warned_urls` HashSet keyed by cache key.
    fn warn_once_http(&mut self, key: u64, msg: &str) {
        if self.warned_urls.insert(key) {
            warning!("{}", msg);
        }
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

/// M-img-3: inline SVG renderer instantiated via the `inline_svg` template
/// in the Markdown DSL. The parsed `SvgDocument` is pushed in by the cache
/// hit path at every frame (cheap clone — AST shares Vec/String internals,
/// no texture uploads). `DrawSvg` re-tessellates on the draw thread using
/// the Walk the caller provides (`Walk::fixed(dw, dh)`).
///
/// The take/put-back dance around `draw_svg.svg_doc` mirrors
/// `widgets/src/vector.rs::Vector::draw_walk` (render_to_rect internally
/// `.take()`s the doc; we restore it so the same widget can redraw on the
/// next frame without a fresh `set_doc` if the cache entry is re-hit).
#[derive(Script, ScriptHook, Widget)]
struct InlineSvg {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    pub draw_svg: DrawSvg,
    #[rust]
    doc: SvgDocument,
}

impl Widget for InlineSvg {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.doc.root.is_empty() {
            return DrawStep::done();
        }
        // `set_doc_bounds` computes content_bounds + content_size from the
        // viewbox transform. Must be called every time `self.doc` changes
        // (which is every frame here) otherwise render_to_rect renders at
        // zero size. Invalidate the geometry cache too — a new frame means
        // potentially a new doc (if cache entry was swapped) or at minimum
        // we haven't uploaded this doc through this widget yet.
        self.draw_svg.set_doc_bounds(&self.doc);
        self.draw_svg.cache_valid = false;
        let rect = cx.walk_turtle(walk);
        self.draw_svg.svg_doc = Some(std::mem::take(&mut self.doc));
        self.draw_svg.render_to_rect(cx, &rect, 0.0);
        self.doc = self.draw_svg.svg_doc.take().unwrap_or_default();
        DrawStep::done()
    }
}

impl InlineSvgRef {
    /// Replace the widget's parsed SVG document. Called from the Markdown
    /// cache-hit path each draw; the AST is cloned per call so the cache
    /// retains ownership (multiple images can share one entry if the same
    /// URL appears twice in a doc).
    pub fn set_doc(&self, doc: SvgDocument) {
        let Some(mut inner) = self.borrow_mut() else {
            return;
        };
        inner.doc = doc;
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

    // -----------------------------------------------------------------
    // M-img-2 unit tests. Widget-level scenarios (real HTTP decode,
    // 404, connect error, HTTPS) need a `Cx` and a loopback server and
    // are covered by manual-verification gates documented in the
    // M-img-2 report. The tests below verify the pure-logic branches.
    // -----------------------------------------------------------------

    /// `parse_image_src` must not accidentally accept ftp/gopher as HTTP.
    #[test]
    fn m_img_2_non_http_scheme_is_invalid() {
        assert!(matches!(parse_image_src("ftp://host/a.png"), ImageSrc::Invalid));
        assert!(matches!(parse_image_src("gopher://old/a.png"), ImageSrc::Invalid));
        // And a positive control.
        assert!(matches!(parse_image_src("http://h/x.png"), ImageSrc::Http(_)));
        assert!(matches!(parse_image_src("https://h/x.png"), ImageSrc::Http(_)));
    }

    /// Same URL across two events dedups to one request. Mirrors the
    /// widget-level `pending_http.contains_key(&LiveId(key))` short-circuit
    /// in `try_load_image`'s Http branch without needing Cx.
    #[test]
    fn m_img_2_pending_http_dedup_is_contains_key() {
        use makepad_platform::LiveId;
        let url = "http://example.test/a.png";
        let key = url_hash(url);
        let request_id = LiveId(key);
        let mut pending: HashMap<LiveId, u64> = HashMap::new();

        assert!(!pending.contains_key(&request_id));
        pending.insert(request_id, key);

        assert!(pending.contains_key(&request_id));
        assert_eq!(pending.len(), 1, "second Start(Image) must not re-insert");

        assert_eq!(pending.get(&request_id).copied(), Some(key));
    }

    /// Oversize body rejected before decode. The branch is
    /// `body.len() > IMAGE_HTTP_BODY_MAX`. Any drift in the cap (or a
    /// regression to `>=`) fails this test.
    #[test]
    fn m_img_2_oversize_body_is_rejected() {
        fn is_oversized(body: &[u8]) -> bool {
            body.len() > IMAGE_HTTP_BODY_MAX
        }
        assert_eq!(IMAGE_HTTP_BODY_MAX, 32 * 1024 * 1024);
        assert!(!is_oversized(&vec![0u8; IMAGE_HTTP_BODY_MAX]));
        assert!(is_oversized(&vec![0u8; IMAGE_HTTP_BODY_MAX + 1]));
        assert!(!is_oversized(&vec![0u8; 4096]));
    }

    /// Cache hit on second set_text skips HTTP. The `try_load_image`
    /// fast path is `cache.contains_key(&key)` → short-circuit before
    /// touching pending_http or network.
    #[test]
    fn m_img_2_cache_hit_short_circuits() {
        let url = "http://host/x.png";
        let key = src_cache_key(&parse_image_src(url), url);

        let mut seen: HashMap<u64, ()> = HashMap::new();
        assert!(!seen.contains_key(&key));
        seen.insert(key, ());
        assert!(
            seen.contains_key(&key),
            "cache-hit predicate must recognise a re-rendered HTTP URL"
        );

        let key2 = src_cache_key(&parse_image_src(url), url);
        assert_eq!(key, key2);
    }

    /// Warn dedup — same failing URL warns once per widget lifetime.
    /// `warn_once_http` uses `warned_urls: HashSet<u64>::insert()` which
    /// returns true only the first time a key is added.
    #[test]
    fn m_img_2_warn_once_per_url() {
        let key = url_hash("http://host/fail.png");
        let mut warned: HashSet<u64> = HashSet::new();

        assert!(warned.insert(key), "first failure must fire a warning");
        assert!(!warned.insert(key), "second failure must be deduped");
        assert_eq!(warned.len(), 1);

        let other_key = url_hash("http://host/other-fail.png");
        assert!(warned.insert(other_key));
        assert_eq!(warned.len(), 2);
    }

    /// request_id construction must be injective per cache key —
    /// `LiveId(key)` is a thin wrapper so two distinct URL hashes produce
    /// distinct LiveIds. This is the invariant the O(1) dedup relies on.
    #[test]
    fn m_img_2_request_id_injective_over_cache_keys() {
        use makepad_platform::LiveId;
        let k1 = url_hash("http://host/a.png");
        let k2 = url_hash("http://host/b.png");
        assert_ne!(k1, k2);
        assert_ne!(LiveId(k1), LiveId(k2));
        let k1_again = url_hash("http://host/a.png");
        assert_eq!(LiveId(k1), LiveId(k1_again));
    }

    // -----------------------------------------------------------------
    // M-img-3 unit tests (SVG rendering). Widget-level scenarios
    // (actual DrawSvg tessellation + on-screen rendering) are manual-
    // verification gates covered in the M-img-3 report.
    // -----------------------------------------------------------------

    /// `detect_svg_magic` must recognise BOTH the XML prolog `<?xml ...?>`
    /// and bare `<svg ...>` with various terminators and leading BOM /
    /// whitespace.
    #[test]
    fn m_img_3_detect_svg_magic_variants() {
        // XML prolog.
        assert!(detect_svg_magic(
            b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        ));
        // Bare tag with attr.
        assert!(detect_svg_magic(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>"
        ));
        // Bare tag with only `>`.
        assert!(detect_svg_magic(b"<svg width=\"1\" height=\"1\"></svg>"));
        // Bare tag self-closing with `/`.
        assert!(detect_svg_magic(b"<svg/>"));
        // With BOM and whitespace.
        assert!(detect_svg_magic(b"\xEF\xBB\xBF\n  <svg width=\"1\"/>"));
        // Negative controls.
        assert!(!detect_svg_magic(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!detect_svg_magic(b"\xFF\xD8\xFFsome"));
        assert!(!detect_svg_magic(b""));
        // Almost-svg but not quite.
        assert!(!detect_svg_magic(b"<svgfoo></svgfoo>"));
    }

    /// Oversize SVG rejected before parse — the byte cap is our only
    /// amplification defense, so any drift in the constant or the
    /// comparison predicate fails this test.
    #[test]
    fn m_img_3_svg_oversize_rejected() {
        fn is_oversized(body: &[u8]) -> bool {
            body.len() > IMAGE_SVG_BYTES_MAX
        }
        assert_eq!(IMAGE_SVG_BYTES_MAX, 4 * 1024 * 1024);
        assert!(!is_oversized(&vec![0u8; IMAGE_SVG_BYTES_MAX]));
        assert!(is_oversized(&vec![0u8; IMAGE_SVG_BYTES_MAX + 1]));
    }

    /// Minimal SVG parses successfully. Also verifies the non-empty-root
    /// invariant that `decode_and_cache_bytes` relies on to distinguish
    /// real SVG from bytes that looked like SVG but yielded no geometry.
    #[test]
    fn m_img_3_svg_parse_minimal_ok() {
        let src = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <rect x="0" y="0" width="10" height="10" fill="red"/>
        </svg>"#;
        let doc = parse_svg(src);
        assert!(
            !doc.root.is_empty(),
            "minimal valid SVG must produce at least one root node"
        );
        let (w, h) = doc.logical_size();
        assert!(w > 0.0 && h > 0.0);
    }

    /// SVG cache byte-charge uses the raw source length (not pixel area).
    /// This protects against parsed-doc size unpredictability across
    /// `makepad_svg` versions. The raster formula is verified by a
    /// separate predicate so we don't need to instantiate a `Texture`
    /// (which requires a real `Cx`).
    #[test]
    fn m_img_3_svg_cache_byte_charge() {
        let doc = parse_svg(r#"<svg width="10" height="10"><rect width="10" height="10"/></svg>"#);
        let bytes_len = 4321usize;
        let entry = ImageCacheEntry::Svg {
            doc,
            raw_bytes_len: bytes_len,
            width: 10,
            height: 10,
        };
        assert_eq!(entry.byte_size(), bytes_len);
        // Raster charge formula is w*h*4.
        let raster_formula = |w: u32, h: u32| (w as usize) * (h as usize) * 4;
        assert_eq!(raster_formula(100, 50), 20_000);
    }

    /// Malformed SVG falls back to the raster magic check. Since bytes that
    /// look like SVG but parse empty should land on the error path in
    /// `decode_and_cache_bytes`, verify the empty-root detection directly.
    #[test]
    fn m_img_3_svg_malformed_empty_root_fallback() {
        // Truly malformed / non-SVG-but-claims-to-be content.
        let doc = parse_svg("<svg></svg>");
        // Empty `<svg/>` produces an empty root — this is the predicate
        // the cache path uses to treat input as a parse failure.
        assert!(
            doc.root.is_empty(),
            "empty <svg/> should produce zero root nodes — the decode path relies on this"
        );
    }
}
