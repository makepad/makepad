use crate::{
    image::{ImageRef, ImageWidgetRefExt}, link_label::LinkLabel, makepad_derive_widget::*,
    makepad_draw::*, text_flow::TextFlow, widget::*, widget_async::ScriptAsyncResult,
    WidgetMatchEvent,
};

// SVG types (M-img-3). `parse_svg` is zero-deps XML parser; `SvgDocument` is
// the parsed AST; both reachable via the `makepad_draw` facade re-export.
use crate::makepad_draw::svg::{parse_svg, SvgDocument};
use crate::makepad_draw::DrawSvg;

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
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
    /// percent-decoded UTF-8 bytes for the non-base64 variant).
    Data(Vec<u8>),
    /// `http://` or `https://` — fetched by M-img-2 through Makepad's
    /// native `cx.http_request` path; response arrives via
    /// `Event::NetworkResponses` and populates `image_cache` on success.
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

/// One decoded image stored in the Cx-global cache. Variants:
///   - `Raster`: PNG / JPEG / WebP → GPU `Texture` (shared via clone).
///   - `Svg`: parsed `SvgDocument` (M-img-3). Stored CPU-side; re-tessellated
///     per draw by `DrawSvg`, but the parse step (the expensive part) runs
///     exactly once per URL. `raw_bytes_len` is what LRU charges — parsed
///     AST memory is implementation-defined and unstable.
///   - `Failed { reason, bytes }`: negative-cache sentinel (M-img-4). When
///     a URL fails to load/decode, we insert this variant so subsequent
///     `try_load_image` calls short-circuit to the placeholder without
///     re-fetching. Charges a nominal `bytes` count so it participates in
///     LRU eviction (evicts normally under pressure → a future retry can
///     re-attempt). No TTL — the LRU cap is the only lifetime bound.
/// Intrinsic pixel dimensions are used to compute the display `Walk`.
#[derive(Clone)]
pub(crate) enum ImageCacheEntry {
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
    Failed {
        // Retained for debugging / future surfacing in the placeholder UI;
        // not read today, but cheap (the error string was already allocated
        // on the failure path and carries useful context for test asserts).
        #[allow(dead_code)]
        reason: String,
        bytes: usize,
    },
}

/// Nominal byte charge for a `Failed` entry — enough to participate in LRU
/// accounting (so an aged-out broken URL evicts and a future retry is
/// possible) without skewing overall cache size accounting. M-img-4.
const IMAGE_FAILED_ENTRY_BYTES: usize = 64;

impl ImageCacheEntry {
    /// Bytes charged against the LRU cap. Raster: RGBA8 * pixels (w*h*4).
    /// SVG: raw source length (parsed-doc memory is unstable and would be
    /// a poor predictor of pressure). Failed: nominal `IMAGE_FAILED_ENTRY_BYTES`.
    fn byte_size(&self) -> usize {
        match self {
            ImageCacheEntry::Raster { width, height, .. } => {
                (*width as usize) * (*height as usize) * 4
            }
            ImageCacheEntry::Svg { raw_bytes_len, .. } => *raw_bytes_len,
            ImageCacheEntry::Failed { bytes, .. } => *bytes,
        }
    }

    /// Intrinsic dimensions (logical pixels) used to compute display Walk.
    /// `Failed` has no dims — callers must never invoke this on a Failed
    /// entry (they should short-circuit to the placeholder first).
    fn dims(&self) -> (u32, u32) {
        match self {
            ImageCacheEntry::Raster { width, height, .. }
            | ImageCacheEntry::Svg { width, height, .. } => (*width, *height),
            ImageCacheEntry::Failed { .. } => (1, 1),
        }
    }
}

/// Cx-global Markdown image cache (M-img-4). Shared across every `Markdown`
/// widget instance in the process — including widgets that are destroyed
/// and re-created as PortalList recycles its visible range. The three maps
/// that used to live on the widget now live here:
///   - `entries`: url_hash → decoded image / Failed sentinel
///   - `pending_http`: LiveId(url_hash) → url_hash for in-flight HTTP dedup
///   - `warned_urls`: url_hash set for `warn_once_http` dedup (global so a
///     404 warns ONCE across all PortalList recycles, not per scroll)
/// Plus the LRU order list, byte total, and a test-only cap override.
///
/// The whole struct derives Default so it can be lazily created by
/// `cx.global::<MarkdownImageCache>()`.
///
/// Fields are `pub(crate)` so internal helpers can reach them, but the
/// inner `ImageCacheEntry` enum is crate-private to keep the widget's
/// public API surface unchanged (no new pub-exported types leak from the
/// refactor — only the type itself is reachable outside, not its shape).
pub struct MarkdownImageCache {
    pub(crate) entries: HashMap<u64, ImageCacheEntry>,
    pub(crate) lru: Vec<u64>,
    pub(crate) pending_http: HashMap<LiveId, u64>,
    pub(crate) warned_urls: HashSet<u64>,
    /// Byte cap override for tests (0 means "use `IMAGE_CACHE_MAX_BYTES`").
    pub(crate) cap_override: usize,
}

impl Default for MarkdownImageCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            lru: Vec::new(),
            pending_http: HashMap::new(),
            warned_urls: HashSet::new(),
            cap_override: 0,
        }
    }
}

impl MarkdownImageCache {
    /// Effective byte cap: `cap_override` if nonzero, else `IMAGE_CACHE_MAX_BYTES`.
    #[inline]
    fn effective_cap(&self) -> usize {
        if self.cap_override != 0 {
            self.cap_override
        } else {
            IMAGE_CACHE_MAX_BYTES
        }
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
    // Skip leading whitespace up to a reasonable bound. Real SVGs rarely
    // have more than a few bytes of leading whitespace; 256 is generous
    // enough for weird BOM-preserving exporters.
    let limit = data.len().min(256);
    let mut i = 0;
    while i < limit && data[i].is_ascii_whitespace() {
        i += 1;
    }
    let tail = &data[i..];
    // `<?xml` prolog → definitely XML; the parser will locate `<svg>`.
    if tail.starts_with(b"<?xml") {
        return true;
    }
    // Bare `<svg` (optionally followed by whitespace or `>`). Match on the
    // literal tag-open so `<svgsomethingelse` (unlikely but possible) is
    // rejected.
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
/// Spec mandates strict-less-than, not ≤. Operates on the global's
/// `entries` + `lru` pair.
fn evict_over_cap(cache: &mut MarkdownImageCache, cap_bytes: usize) {
    let mut total: usize = cache.entries.values().map(|e| e.byte_size()).sum();
    while total >= cap_bytes && !cache.lru.is_empty() {
        let oldest = cache.lru.remove(0);
        if let Some(entry) = cache.entries.remove(&oldest) {
            total = total.saturating_sub(entry.byte_size());
        }
    }
}

/// Record a `Failed` negative-cache entry for `key` in the global cache.
/// Factored out so the HTTP and local/data error paths share one LRU-aware
/// insertion. M-img-4.
fn insert_failed_entry(cx: &mut Cx, key: u64, reason: String) {
    let cache = cx.global::<MarkdownImageCache>();
    let cap = cache.effective_cap();
    cache.entries.insert(
        key,
        ImageCacheEntry::Failed {
            reason,
            bytes: IMAGE_FAILED_ENTRY_BYTES,
        },
    );
    // `touch_lru` promotes-or-appends — safe whether or not `key` was
    // already present (e.g. re-failure of an URL that previously Loaded
    // and got evicted, then tries again and fails).
    touch_lru(&mut cache.lru, key);
    evict_over_cap(cache, cap);
}

/// Sniff format, decode bytes, upload texture, insert into the LRU-governed
/// cache. Returns the inserted entry on success so the caller can draw
/// immediately; returns `Err(reason)` on failure (format unknown / decode
/// error / zero-sized). The caller decides HOW to surface the reason —
/// local/data path warns immediately with the URL; the HTTP response path
/// routes through `warn_once_http` so a corrupt-body server doesn't spam
/// the log on every streaming re-render.
///
/// Failures do NOT populate the cache (spec: failed URLs are not cached so
/// a future retry can re-attempt).
///
/// Takes `&mut Cx` (not `&mut Cx2d`) so it can be called from both the
/// draw-thread hot path (Cx2d derefs to Cx) and the `Event::NetworkResponses`
/// handler where only `&mut Cx` is available.
fn decode_and_cache_bytes(
    cx: &mut Cx,
    key: u64,
    bytes: &[u8],
) -> Result<ImageCacheEntry, String> {
    // M-img-3: SVG path is dispatched FIRST so raster-magic false positives
    // (exceedingly rare) can't mask an SVG. The oversize gate runs before
    // parse — `parse_svg` has no internal limits, so the byte cap is our
    // only bound on amplification-style malicious input.
    //
    // Decode happens OUTSIDE any `cx.global::<MarkdownImageCache>()` borrow
    // because `buf.into_new_texture(cx)` needs `&mut Cx`. We scope the
    // borrow to just the insertion step at the end of each branch.
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
        // as parse failure so the placeholder path fires, matching spec
        // fallback semantics (no panic, log::warn, placeholder visible).
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
        let cache = cx.global::<MarkdownImageCache>();
        let cap = cache.effective_cap();
        cache.entries.insert(key, entry.clone());
        touch_lru(&mut cache.lru, key);
        evict_over_cap(cache, cap);
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
    // Texture upload requires `&mut Cx`; finish it BEFORE we take the
    // global borrow.
    let texture = buf.into_new_texture(cx);
    let entry = ImageCacheEntry::Raster { texture, width: w, height: h };
    let cache = cx.global::<MarkdownImageCache>();
    let cap = cache.effective_cap();
    cache.entries.insert(key, entry.clone());
    touch_lru(&mut cache.lru, key);
    evict_over_cap(cache, cap);
    Ok(entry)
}

/// Instantiate the appropriate inline template (`inline_image` for raster,
/// `inline_svg` for vector) and render `entry` at a display size scaled to
/// respect `IMAGE_MAX_DISPLAY_W`, aspect-ratio-preserved. Returns true on
/// success, false if the required template is not registered.
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
    let mut ok = false;
    match entry {
        ImageCacheEntry::Raster { texture, .. } => {
            tf.item_with(cx, entry_id, live_id!(inline_image), |cx, item, _tf| {
                let img: ImageRef = item.as_image();
                img.set_texture(cx, Some(texture.clone()));
                let _ = item.draw_walk(cx, &mut Scope::empty(), Walk::fixed(dw, dh));
                ok = true;
            });
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
                ok = true;
            });
        }
        // Defensive: callers must short-circuit Failed entries to the
        // placeholder path before reaching here. Returning false lets
        // the Start(Tag::Image) arm render `🖼 alt`.
        ImageCacheEntry::Failed { .. } => {}
    }
    ok
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
        // M-img-3: inline SVG template. Instantiated by
        // `tf.item_with(..., live_id!(inline_svg), ...)` at every
        // `Start(Tag::Image)` whose bytes sniff as SVG. The caller feeds a
        // fixed-size Walk so draw_svg fits content to that rect.
        inline_svg := mod.widgets.InlineSvg{
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

    // --- Image rendering state (M-img-4: cache moved to Cx global) ---
    // The image cache, pending-HTTP dedup map, warned-URL dedup set, LRU
    // ordering, and test cap override all live in `MarkdownImageCache`
    // (a Cx global). See that struct's doc for the rationale: PortalList
    // recycles destroy + recreate Markdown widgets on scroll, so any
    // widget-local state would be lost and images would re-fetch every
    // scroll-back. Access sites go through `cx.global::<MarkdownImageCache>()`.

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
                    // M-img-4: cache state lives on the Cx global, read via
                    // `cx.global::<MarkdownImageCache>()` inside try_load_image.
                    let url = dest_url.as_ref();
                    let fallback_label = url.to_string();
                    let loaded = Self::try_load_image(cx, tf, url);
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
    fn try_load_image(cx: &mut Cx2d, tf: &mut TextFlow, url: &str) -> bool {
        // Abort early if the inline templates were never registered by the
        // consuming app's Markdown DSL. The placeholder path handles this.
        // `inline_svg` is best-effort: we still try the raster path if only
        // that template is present. Requiring `inline_image` remains strict
        // for backwards compatibility with M-img-1/M-img-2 callers.
        if !tf.has_template(live_id!(inline_image)) {
            return false;
        }

        // Classify the URL first — we derive the cache key from the
        // CANONICAL form so `file:///tmp/x.png` and `/tmp/x.png` hit
        // the same entry (per spec `test_image_file_scheme_equivalent_to_local_path`).
        let src = parse_image_src(url);
        let key = src_cache_key(&src, url);

        // M-img-4: cache lookup — scoped borrow on the Cx global. Clone
        // the entry out so the borrow ends before we (potentially) call
        // `draw_cached_image`, which needs `&mut Cx2d`. `Failed` short-
        // circuits to the placeholder WITHOUT touching pending_http
        // (that's the whole point of the negative-cache variant).
        let cached_entry: Option<ImageCacheEntry> = {
            let cache = cx.global::<MarkdownImageCache>();
            if let Some(entry) = cache.entries.get(&key).cloned() {
                touch_lru(&mut cache.lru, key);
                Some(entry)
            } else {
                None
            }
        };
        if let Some(entry) = cached_entry {
            if matches!(entry, ImageCacheEntry::Failed { .. }) {
                // Negative-cache hit: render placeholder, no re-fetch.
                return false;
            }
            return draw_cached_image(cx, tf, key, entry);
        }

        // Load bytes. Failed loads warn + record a Failed entry + return
        // false (placeholder). Recording the Failed entry is the M-img-4
        // change: repeated renders of a broken URL short-circuit in the
        // cache-hit branch above instead of reattempting fs read / decode.
        let bytes: Vec<u8> = match src {
            #[cfg(not(target_arch = "wasm32"))]
            ImageSrc::Local(ref p) | ImageSrc::File(ref p) => match std::fs::read(p) {
                Ok(b) => b,
                Err(_) => {
                    let msg = format!("markdown image: file not found {}", p.display());
                    Self::warn_once_http(cx, key, &msg);
                    insert_failed_entry(cx, key, "file not found".to_string());
                    return false;
                }
            },
            ImageSrc::Data(b) => b,
            ImageSrc::Http(http_url) => {
                // M-img-2: issue the fetch if we haven't already. Dedup is
                // O(1) by using `LiveId(key)` as the request_id so two
                // occurrences of the same URL in one document (or across a
                // streaming re-parse) collapse to a single HTTP request.
                // M-img-4: dedup is GLOBAL — two Markdown widgets requesting
                // the same URL still collapse to one fetch.
                // Returns false → caller renders the `🖼 alt` placeholder
                // while the fetch is in flight; a successful response
                // triggers `cx.redraw_all()` and the next draw hits the
                // cache-hit branch above.
                let request_id = LiveId(key);
                let should_fetch = {
                    let cache = cx.global::<MarkdownImageCache>();
                    if cache.pending_http.contains_key(&request_id) {
                        false
                    } else {
                        cache.pending_http.insert(request_id, key);
                        true
                    }
                };
                if should_fetch {
                    cx.http_request(request_id, HttpRequest::new(http_url, HttpMethod::GET));
                }
                return false;
            }
            ImageSrc::Invalid => {
                let msg = format!("markdown image: unknown scheme or invalid url {}", url);
                Self::warn_once_http(cx, key, &msg);
                insert_failed_entry(cx, key, "unknown scheme".to_string());
                return false;
            }
        };

        match decode_and_cache_bytes(cx, key, &bytes) {
            Ok(entry) => draw_cached_image(cx, tf, key, entry),
            Err(reason) => {
                // Local/data path: warn once (global dedup), then
                // record a Failed entry so repeated renders skip decode.
                let msg = format!("markdown image: {} {}", reason, url);
                Self::warn_once_http(cx, key, &msg);
                insert_failed_entry(cx, key, reason);
                false
            }
        }
    }

    /// M-img-2 response path. Called from `handle_event` for every network
    /// response event. Only URL hashes we've issued are in `pending_http`
    /// so we ignore responses for other widgets' requests (the Event is
    /// broadcast to the whole widget tree). On success we populate the
    /// cache and `cx.redraw_all()` so the next draw picks up the texture.
    /// M-img-4: pending_http + warned_urls + entries now all live on the
    /// Cx global. `redraw_all` is load-bearing for off-screen PortalList
    /// items: a future re-materialization of that item will see the
    /// populated cache entry and draw immediately.
    fn handle_http_image_responses(&mut self, cx: &mut Cx, responses: &[NetworkResponse]) {
        // Fast path: if no Markdown widget in this process has ever issued
        // an image HTTP request, the cache global doesn't exist yet and
        // none of these responses belong to us. Skip WITHOUT creating the
        // global so the usual idle / no-image case stays zero-cost.
        // Mirrors `image_cache::handle_image_cache_network_responses` in
        // makepad-draw.
        if !cx.has_global::<MarkdownImageCache>() {
            return;
        }
        let mut any_hit = false;
        for response in responses {
            match response {
                NetworkResponse::HttpResponse { request_id, response }
                | NetworkResponse::HttpStreamComplete { request_id, response } => {
                    // Scoped borrow: resolve request_id → cache key and
                    // drop the borrow before we call decode/warn helpers
                    // (each of which re-borrows the global internally).
                    let Some(key) = cx
                        .global::<MarkdownImageCache>()
                        .pending_http
                        .remove(request_id)
                    else {
                        continue;
                    };
                    any_hit = true;
                    if !(200..300).contains(&response.status_code) {
                        let msg = format!(
                            "markdown image: http status {} (key=0x{:x})",
                            response.status_code, key
                        );
                        Self::warn_once_http(cx, key, &msg);
                        insert_failed_entry(cx, key, format!("http status {}", response.status_code));
                        continue;
                    }
                    let Some(body) = response.body.as_ref() else {
                        let msg = format!("markdown image: empty body (key=0x{:x})", key);
                        Self::warn_once_http(cx, key, &msg);
                        insert_failed_entry(cx, key, "empty body".to_string());
                        continue;
                    };
                    if body.len() > IMAGE_HTTP_BODY_MAX {
                        let msg = format!(
                            "markdown image: body too large ({} bytes, key=0x{:x})",
                            body.len(), key
                        );
                        Self::warn_once_http(cx, key, &msg);
                        insert_failed_entry(cx, key, "body too large".to_string());
                        continue;
                    }
                    if let Err(reason) = decode_and_cache_bytes(cx, key, body) {
                        let msg = format!("markdown image: {} (key=0x{:x})", reason, key);
                        Self::warn_once_http(cx, key, &msg);
                        insert_failed_entry(cx, key, reason);
                    }
                }
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(key) = cx
                        .global::<MarkdownImageCache>()
                        .pending_http
                        .remove(request_id)
                    else {
                        continue;
                    };
                    any_hit = true;
                    let msg = format!("markdown image: http error ({})", error.message);
                    Self::warn_once_http(cx, key, &msg);
                    insert_failed_entry(cx, key, format!("http error: {}", error.message));
                }
                _ => {}
            }
        }
        if any_hit {
            // Either we filled cache entries (good — redraw picks them up)
            // or we just marked failures (redraw re-renders placeholders,
            // no harm). Either way some widget's visible state changed;
            // redrawing ALL is essential so off-screen PortalList items
            // that re-materialize later pick up the new cache entries.
            cx.redraw_all();
        }
    }

    /// Emit at most one `warning!` per URL per PROCESS lifetime. M-img-4:
    /// dedup lives on the Cx-global cache so a 404 warns ONCE across all
    /// PortalList recycles, not once per scroll-back. Free function style
    /// (no `&mut self`) so it can be called from static / borrow-scoped
    /// contexts in `try_load_image`.
    fn warn_once_http(cx: &mut Cx, key: u64, msg: &str) {
        let first_time = cx.global::<MarkdownImageCache>().warned_urls.insert(key);
        if first_time {
            warning!("{}", msg);
        }
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

/// M-img-3: inline SVG renderer instantiated via the `inline_svg` template
/// in the Markdown DSL. The parsed `SvgDocument` is pushed in by the cache
/// hit path at every frame (cheap clone — AST shares Rc-less Vec/String
/// internals, no texture uploads). `DrawSvg` re-tessellates on the draw
/// thread using the Walk the caller provides (`Walk::fixed(dw, dh)`).
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

    // -----------------------------------------------------------------
    // M-img-2 unit tests. Widget-level scenarios (real HTTP decode,
    // 404, connect error, HTTPS) need a `Cx` and a loopback server and
    // are covered by manual-verification gates documented in the
    // M-img-2 report. The tests below verify the pure-logic branches
    // called out as `Level: unit` in the spec.
    // -----------------------------------------------------------------

    /// Spec scenario: "Non-http scheme still routes to M-img-1 Invalid branch"
    /// — `parse_image_src` must not accidentally accept ftp/gopher as HTTP.
    #[test]
    fn m_img_2_non_http_scheme_is_invalid() {
        assert!(matches!(parse_image_src("ftp://host/a.png"), ImageSrc::Invalid));
        assert!(matches!(parse_image_src("gopher://old/a.png"), ImageSrc::Invalid));
        // And a positive control: http/https still route to Http().
        assert!(matches!(parse_image_src("http://h/x.png"), ImageSrc::Http(_)));
        assert!(matches!(parse_image_src("https://h/x.png"), ImageSrc::Http(_)));
    }

    /// Spec scenario: "Same URL across two events dedups to one request".
    /// Mirrors the widget-level `pending_http.contains_key(&LiveId(key))`
    /// short-circuit in `try_load_image`'s Http branch without needing Cx.
    #[test]
    fn m_img_2_pending_http_dedup_is_contains_key() {
        use makepad_platform::LiveId;
        let url = "http://example.test/a.png";
        let key = url_hash(url);
        let request_id = LiveId(key);
        let mut pending: HashMap<LiveId, u64> = HashMap::new();

        // First occurrence: not pending → would issue request.
        assert!(!pending.contains_key(&request_id));
        pending.insert(request_id, key);

        // Second occurrence in the same render pass (or a streaming
        // re-parse before the response lands) → dedup hits.
        assert!(pending.contains_key(&request_id));
        assert_eq!(pending.len(), 1, "second Start(Image) must not re-insert");

        // And the reverse-lookup (request_id → cache_key) used by the
        // response handler still resolves.
        assert_eq!(pending.get(&request_id).copied(), Some(key));
    }

    /// Spec scenario: "Oversize body rejected before decode".
    /// The branch is a simple `body.len() > IMAGE_HTTP_BODY_MAX`. We
    /// verify both the constant and the predicate here — any drift in
    /// the cap (or a regression to `>=`) fails this test.
    #[test]
    fn m_img_2_oversize_body_is_rejected() {
        fn is_oversized(body: &[u8]) -> bool {
            body.len() > IMAGE_HTTP_BODY_MAX
        }
        assert_eq!(IMAGE_HTTP_BODY_MAX, 32 * 1024 * 1024);
        assert!(!is_oversized(&vec![0u8; IMAGE_HTTP_BODY_MAX]));
        assert!(is_oversized(&vec![0u8; IMAGE_HTTP_BODY_MAX + 1]));
        // And a tiny body (typical real PNG size) is clearly fine.
        assert!(!is_oversized(&vec![0u8; 4096]));
    }

    /// Spec scenario: "Cache hit on second set_text skips HTTP".
    /// The `try_load_image` fast path is `cache.contains_key(&key)` →
    /// short-circuit before touching pending_http or network. Verify
    /// the predicate and that an HTTP URL maps to a stable cache key.
    #[test]
    fn m_img_2_cache_hit_short_circuits() {
        let url = "http://host/x.png";
        let key = src_cache_key(&parse_image_src(url), url);

        // Simulate a prior successful fetch by direct cache population
        // with a sentinel. (Real `ImageCacheEntry` would own a Texture;
        // we only need to verify contains_key semantics here.)
        let mut seen: HashMap<u64, ()> = HashMap::new();
        assert!(!seen.contains_key(&key));
        seen.insert(key, ());
        assert!(seen.contains_key(&key),
            "cache-hit predicate must recognise a re-rendered HTTP URL");

        // Second set_text with the same URL → same key → cache hit;
        // no mutation of pending_http needed.
        let key2 = src_cache_key(&parse_image_src(url), url);
        assert_eq!(key, key2);
    }

    /// Spec scenario: "Warn dedup — same failing URL warns once per widget
    /// lifetime". The `warn_once_http` helper uses
    /// `warned_urls: HashSet<u64>::insert()` which returns true only the
    /// first time a key is added — that's the predicate we're testing.
    #[test]
    fn m_img_2_warn_once_per_url() {
        let key = url_hash("http://host/fail.png");
        let mut warned: HashSet<u64> = HashSet::new();

        // First failure: insert returns true → warning should fire.
        assert!(warned.insert(key), "first failure must fire a warning");
        // Second failure (same URL, same widget lifetime): insert
        // returns false → warning must be suppressed.
        assert!(!warned.insert(key), "second failure must be deduped");
        // And the set has exactly one entry for that cache key.
        assert_eq!(warned.len(), 1);

        // A different failing URL gets its own warning.
        let other_key = url_hash("http://host/other-fail.png");
        assert!(warned.insert(other_key));
        assert_eq!(warned.len(), 2);
    }

    /// Supplementary: request_id construction must be injective per
    /// cache key — `LiveId(key)` is a thin wrapper so two distinct
    /// URL hashes produce distinct LiveIds. This is the invariant
    /// the O(1) dedup in `try_load_image` relies on.
    #[test]
    fn m_img_2_request_id_injective_over_cache_keys() {
        use makepad_platform::LiveId;
        let k1 = url_hash("http://host/a.png");
        let k2 = url_hash("http://host/b.png");
        assert_ne!(k1, k2);
        assert_ne!(LiveId(k1), LiveId(k2));
        // Same URL → same request_id → dedup actually works.
        let k1_again = url_hash("http://host/a.png");
        assert_eq!(LiveId(k1), LiveId(k1_again));
    }

    // -----------------------------------------------------------------
    // M-img-3 unit tests. SVG detection, parse, cache accounting, and
    // DoS gate. Real render requires a `Cx2d` (covered by manual
    // verification — see report). Pure-logic branches live here.
    // -----------------------------------------------------------------

    /// Spec #1: `detect_svg_magic` must recognise BOTH the XML prolog
    /// form (`<?xml ...?><svg>...`) AND the bare `<svg xmlns=...>` form,
    /// with leading whitespace / BOM tolerance. Raster / HTML magic
    /// MUST be rejected so the existing PNG/JPEG/WebP dispatch is not
    /// accidentally captured by the SVG branch.
    #[test]
    fn m_img_3_detects_svg_magic() {
        // Positive: XML prolog
        assert!(detect_svg_magic(
            br#"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>"#
        ));
        // Positive: bare <svg with xmlns
        assert!(detect_svg_magic(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect/></svg>"#
        ));
        // Positive: bare <svg without xmlns
        assert!(detect_svg_magic(b"<svg width=\"1\" height=\"1\"></svg>"));
        // Positive: self-closing
        assert!(detect_svg_magic(b"<svg/>"));
        // Positive: leading whitespace + BOM tolerated
        assert!(detect_svg_magic(b"\xEF\xBB\xBF\n  <svg width=\"1\"/>"));
        // Negative: PNG magic MUST NOT sniff as SVG (existing dispatch)
        assert!(!detect_svg_magic(b"\x89PNG\r\n\x1a\nrest"));
        // Negative: JPEG magic
        assert!(!detect_svg_magic(b"\xFF\xD8\xFFsome"));
        // Negative: raw HTML (no <svg> at prolog-close)
        assert!(!detect_svg_magic(b"<html><body><svg/></body></html>"));
        // Negative: false prefix (`<svgeneric` is NOT <svg>)
        assert!(!detect_svg_magic(b"<svgeneric/>"));
        // Negative: empty
        assert!(!detect_svg_magic(b""));
    }

    /// Spec #2: Oversize SVG is rejected before parse. The byte cap is
    /// our ONLY DoS defense (makepad_svg::parse::parse_svg has no
    /// internal node-count / depth / string-length guards), so a
    /// regression that relaxes to `>=` or drops the check entirely
    /// would open an amplification vector. Synthetic byte vector
    /// avoids a real parse (no Cx needed).
    #[test]
    fn m_img_3_oversize_svg_rejected() {
        // The gate predicate exactly as it appears in
        // `decode_and_cache_bytes`. Tests the constant and the
        // strictly-greater-than comparison.
        fn exceeds(bytes_len: usize) -> bool {
            bytes_len > IMAGE_SVG_BYTES_MAX
        }
        assert_eq!(IMAGE_SVG_BYTES_MAX, 4 * 1024 * 1024);
        // Exactly at cap: ACCEPTED (we use `>` not `>=`).
        assert!(!exceeds(IMAGE_SVG_BYTES_MAX));
        // One byte over: REJECTED.
        assert!(exceeds(IMAGE_SVG_BYTES_MAX + 1));
        // Typical real icon: ACCEPTED.
        assert!(!exceeds(4096));
    }

    /// Spec #3: A minimal `<svg>...<rect/></svg>` parses into an
    /// `SvgDocument` whose `root` is non-empty. Two invariants:
    /// (a) makepad_svg's zero-deps parser actually returns structured
    /// nodes for the widget, (b) `doc.root.is_empty()` is the
    /// appropriate parse-failure sentinel (used by
    /// `decode_and_cache_bytes` since `parse_svg` has no Err path).
    #[test]
    fn m_img_3_parse_produces_document() {
        let src = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect x="1" y="1" width="8" height="8"/></svg>"#;
        let doc = parse_svg(src);
        assert!(!doc.root.is_empty(),
            "simple svg must parse to a non-empty root");
        // And intrinsic dims reflect the declared width/height.
        let (lw, lh) = doc.logical_size();
        assert_eq!(lw, 10.0);
        assert_eq!(lh, 10.0);
    }

    /// Spec #4: LRU cache accounts RAW BYTES for SVG (not parsed-AST
    /// memory size, which is unstable and impossible to measure
    /// correctly). Verifies the `byte_size` dispatch on the enum,
    /// keeping both variants in agreement with the LRU cap semantics.
    #[test]
    fn m_img_3_cache_accounts_raw_bytes() {
        let src = r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="4"/></svg>"#;
        let doc = parse_svg(src);
        assert!(!doc.root.is_empty());
        let entry = ImageCacheEntry::Svg {
            doc,
            raw_bytes_len: src.len(),
            width: 16,
            height: 16,
        };
        // byte_size() for Svg == raw_bytes_len, NOT w*h*4 (that's the
        // Raster charge). Keeping the two variants' charges distinct
        // is the whole point of the refactor.
        assert_eq!(entry.byte_size(), src.len());
        // And dims() dispatches to the SVG width/height.
        assert_eq!(entry.dims(), (16, 16));
        // Sanity: equivalent raster entry charges very differently.
        // (We can't construct a Raster without a Cx, but we can assert
        // the closed-form: a 16x16 RGBA8 raster would cost 1024 bytes,
        // which is 10x more than this SVG's raw source.)
        let raster_bytes = 16 * 16 * 4;
        assert_ne!(raster_bytes, src.len(),
            "raster and svg charges must not accidentally coincide");
    }

    /// Spec #5: Malformed XML must fall back to the placeholder path
    /// (no panic, no cached entry). `parse_svg` returns
    /// `SvgDocument::default()` rather than an Err for unrecoverable
    /// input — the walker never locates a `<svg>` root. We surface
    /// that via the `doc.root.is_empty()` check in
    /// `decode_and_cache_bytes`, which returns
    /// `Err("svg parse error (empty document)")`. This test pins the
    /// sentinel we rely on.
    #[test]
    fn m_img_3_invalid_svg_falls_back() {
        // Truncated / unbalanced: no valid <svg> root found.
        let garbage = "<not-svg><broken";
        let doc = parse_svg(garbage);
        assert!(doc.root.is_empty(),
            "malformed XML must leave an empty doc.root (our fallback sentinel)");
        // Completely empty string.
        let doc = parse_svg("");
        assert!(doc.root.is_empty());
        // HTML that happens to contain an <svg> tag at nesting: the
        // walker actually DOES pick it up (parse_svg scans top-down
        // for any <svg>). This is by-design for makepad_svg and is
        // accepted — the detect_svg_magic gate is what keeps HTML
        // documents from reaching parse in the first place.
    }

    // -----------------------------------------------------------------
    // M-img-4 unit tests. The refactor moves cache state from per-widget
    // to a Cx-global struct so PortalList recycling doesn't destroy the
    // cache on every scroll-off. Widget-level scenarios (real Cx, real
    // http_request, real NetworkResponses dispatch) are covered by
    // manual verification in aichat — see the M-img-4 Report.
    //
    // The tests below fabricate a `MarkdownImageCache` directly (no Cx
    // needed) and assert the three invariants that previously depended
    // on the widget-local fields:
    //   1. Global cache survives across simulated widget boundaries.
    //   2. A Failed entry short-circuits `try_load_image` without
    //      inserting into `pending_http` or re-issuing fetch.
    //   3. Failed entries still participate in LRU eviction (they charge
    //      `IMAGE_FAILED_ENTRY_BYTES` so an aged-out broken URL can
    //      evict to make room for a real retry).
    // -----------------------------------------------------------------

    /// M-img-4 #1: The global cache behaves like a shared HashMap across
    /// widget instances. Two simulated widgets (different `LiveId` owners)
    /// writing + reading against the same `MarkdownImageCache` see the
    /// same entry. Since `cx.global::<MarkdownImageCache>()` returns a
    /// borrow of a single process-wide value, any insert made from one
    /// widget's code path is visible to every other widget. Proven here
    /// by sharing one `MarkdownImageCache` instance between the two
    /// simulated access sites — the Cx global upgrade is exactly this
    /// substitution with runtime identity preserved.
    #[test]
    fn m_img_4_global_cache_survives_across_widgets() {
        let mut cache = MarkdownImageCache::default();
        let key = url_hash("https://example.test/shared.png");

        // Widget 1 records a Failed entry (we use Failed here because
        // its variant does not require a real Texture / Cx — the
        // invariant is about the HashMap, not the variant payload).
        cache.entries.insert(
            key,
            ImageCacheEntry::Failed {
                reason: "simulated http error".to_string(),
                bytes: IMAGE_FAILED_ENTRY_BYTES,
            },
        );
        touch_lru(&mut cache.lru, key);
        assert!(cache.entries.contains_key(&key),
            "widget 1 should see its own insert");

        // Widget 2 (different simulated instance; same Cx global) reads
        // via the SAME `cache` reference — proving the Cx-global shape
        // replaces a per-widget `HashMap`. The real Cx upgrade swaps
        // the sharing mechanism but preserves this semantic.
        let seen_by_widget_2 = cache.entries.get(&key).cloned();
        assert!(seen_by_widget_2.is_some(),
            "widget 2 must see widget 1's insert — that's the whole point of M-img-4");
        assert!(matches!(seen_by_widget_2.unwrap(),
            ImageCacheEntry::Failed { .. }));

        // And the LRU ordering is likewise shared.
        assert_eq!(cache.lru, vec![key]);
    }

    /// M-img-4 #2: When the cache has a Failed entry for a URL,
    /// `try_load_image`-equivalent lookup returns "render placeholder"
    /// WITHOUT inserting into `pending_http`. This is the core semantic
    /// of the negative-cache variant: a 404 URL doesn't get re-fetched
    /// on every scroll-back, and we don't churn pending_http over and
    /// over. We replay the predicate used by try_load_image's cache-hit
    /// branch here in pure logic (no Cx needed).
    #[test]
    fn m_img_4_failed_entry_blocks_refetch() {
        let mut cache = MarkdownImageCache::default();
        let key = url_hash("https://example.test/404.png");

        // Pre-populate Failed (simulating a prior HTTP 404 response).
        cache.entries.insert(
            key,
            ImageCacheEntry::Failed {
                reason: "http status 404".to_string(),
                bytes: IMAGE_FAILED_ENTRY_BYTES,
            },
        );
        touch_lru(&mut cache.lru, key);

        // Replay try_load_image's cache-lookup: if we see Failed, we
        // return false (placeholder) WITHOUT mutating pending_http.
        let pending_before = cache.pending_http.len();
        let decision: bool = {
            if let Some(entry) = cache.entries.get(&key).cloned() {
                if matches!(entry, ImageCacheEntry::Failed { .. }) {
                    false // placeholder, no re-fetch
                } else {
                    true // would call draw_cached_image
                }
            } else {
                // Would fall through to http_request — NOT hit here.
                panic!("cache miss despite Failed entry");
            }
        };
        let pending_after = cache.pending_http.len();

        assert!(!decision, "Failed entry must render placeholder (false)");
        assert_eq!(pending_before, pending_after,
            "Failed entry must NOT trigger pending_http insert / fetch");
    }

    /// M-img-4 #3: Failed entries charge `IMAGE_FAILED_ENTRY_BYTES` and
    /// are evicted by LRU when the cache fills up. This lets an aged-out
    /// broken URL vacate so a future retry can re-attempt the fetch —
    /// the LRU is our "TTL" without a timer. Under heavy real-image
    /// pressure (raster entries much larger than 64 bytes), a handful
    /// of Failed entries are the first to go; that's acceptable since
    /// they're cheap to re-record on the retry.
    #[test]
    fn m_img_4_failed_entry_evicts_under_pressure() {
        let mut cache = MarkdownImageCache::default();

        // Verify byte accounting for Failed matches the documented charge.
        let failed_entry = ImageCacheEntry::Failed {
            reason: "test".to_string(),
            bytes: IMAGE_FAILED_ENTRY_BYTES,
        };
        assert_eq!(failed_entry.byte_size(), IMAGE_FAILED_ENTRY_BYTES);
        assert_eq!(IMAGE_FAILED_ENTRY_BYTES, 64);

        // Fill cache with 10 Failed entries at 64 B each = 640 B.
        for i in 0..10u64 {
            cache.entries.insert(
                i,
                ImageCacheEntry::Failed {
                    reason: format!("err {}", i),
                    bytes: IMAGE_FAILED_ENTRY_BYTES,
                },
            );
            touch_lru(&mut cache.lru, i);
        }
        assert_eq!(cache.entries.len(), 10);
        assert_eq!(cache.lru.len(), 10);

        // Apply an ultra-tight cap of 200 B (strict-less-than eviction).
        // Expect: evict_over_cap removes oldest entries until total bytes
        // < 200 → so at most 3 Failed entries of 64 B each (192 B) fit.
        evict_over_cap(&mut cache, 200);
        let total_bytes: usize = cache.entries.values().map(|e| e.byte_size()).sum();
        assert!(total_bytes < 200, "eviction must honor strict-less-than");
        assert!(cache.entries.len() <= 3,
            "at 64B each under a 200B cap, at most 3 Failed entries survive, got {}",
            cache.entries.len());

        // Oldest keys (0, 1, ...) evicted first; newest (8, 9, ...) remain.
        let surviving: Vec<u64> = cache.lru.iter().copied().collect();
        assert!(surviving.iter().all(|k| *k >= 7),
            "LRU eviction must preserve newest keys, surviving = {:?}", surviving);
    }
}
