//! Render Apple color-emoji glyphs via CoreText's `CTFontDrawGlyphs` on
//! iOS/tvOS/macOS.
//!
//! `AppleColorEmoji` stores its color bitmaps in the `sbix` table using Apple's
//! private, undocumented `emjc` graphicType (image magic `emj1`), NOT the `png `
//! the older Apple docs describe. `ttf_parser` only decodes `png `/`dupe`, so
//! `face.glyph_raster_image()` returns `None` for every emoji glyph and they
//! fall to a monochrome outline → tofu.
//!
//! Worse, that `sbix` table is ~179MB (99.5% of the 188MB font). To keep it out
//! of the resident, parsed `ttf_parser` face, the reassembly path
//! (`sfnt_bytes_from_ctfont`) strips `sbix` before handing bytes to
//! `ttf_parser`. So we can no longer rebuild a `CGFont` from those bytes — they
//! have no color bitmaps left.
//!
//! Instead we hand the glyph back to Apple by re-querying CoreText for the font
//! *by its PostScript name* (`CTFontCreateWithName`). CoreText opens the real
//! system font file, lazily mmapped — the ~179MB `sbix` is never materialized
//! (measured ~8MB resident for a queried font that has drawn a few glyphs). We
//! then draw the glyph into a BGRA `CGBitmapContext` with `CTFontDrawGlyphs`,
//! read the pixels back, and feed them into makepad's existing shared
//! color-atlas pipeline (`AtlasKind::Color`).
//!
//! To make sure `CTFontCreateWithName` resolves the *same* face whose glyph ids
//! `ttf_parser` gave us (the emoji font lives in a `.ttc` with more than one
//! face), we compare `CTFontGetGlyphCount` against the count `ttf_parser` sees;
//! a mismatch declines the renderer so the caller falls back to the outline
//! path rather than draw wrong glyphs.
//!
//! We `dlopen` CoreGraphics / CoreText / CoreFoundation at runtime (the `draw`
//! crate has no `apple_sys` dependency), mirroring `hvgl_render.rs`. If the
//! dylibs or symbols are unavailable we return `None` and the caller falls back
//! to the outline path, exactly as before.

#![cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]

use {
    super::{
        geom::{Point, Size},
        image::{Bgra, SubimageMut},
    },
    std::{
        cell::RefCell,
        ffi::c_void,
        sync::OnceLock,
    },
};

// ---- CoreGraphics / CoreText / CoreFoundation C ABI ------------------------
//
// All handle types are opaque pointers. `CGFloat` is `f64` on 64-bit Apple
// platforms (the only targets we ship on today).

type CGFloat = f64;
type CGGlyph = u16;

// Opaque CF/CG/CT handles.
type CGColorSpaceRef = *mut c_void;
type CGContextRef = *mut c_void;
type CTFontRef = *mut c_void;
type CFStringRef = *const c_void;
type CFTypeRef = *const c_void;

type CFIndex = isize;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: CGFloat,
    y: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: CGFloat,
    height: CGFloat,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

/// `CGBitmapInfo`: `kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little`.
/// In little-endian 32-bit words this lays the bytes out as `[B, G, R, A]` in
/// memory (matching `Bgra`), with color premultiplied by alpha.
const K_CG_BITMAP_BGRA_PREMULTIPLIED: u32 = 2 | 0x2000;

/// `kCFStringEncodingUTF8`.
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

type FnCGColorSpaceCreateDeviceRGB = unsafe extern "C" fn() -> CGColorSpaceRef;
type FnCGBitmapContextCreate = unsafe extern "C" fn(
    data: *mut c_void,
    width: usize,
    height: usize,
    bits_per_component: usize,
    bytes_per_row: usize,
    space: CGColorSpaceRef,
    bitmap_info: u32,
) -> CGContextRef;
type FnCGBitmapContextGetData = unsafe extern "C" fn(ctx: CGContextRef) -> *mut c_void;
type FnCGBitmapContextGetBytesPerRow = unsafe extern "C" fn(ctx: CGContextRef) -> usize;
type FnCGColorSpaceRelease = unsafe extern "C" fn(space: CGColorSpaceRef);
type FnCGContextRelease = unsafe extern "C" fn(ctx: CGContextRef);

type FnCFStringCreateWithBytes = unsafe extern "C" fn(
    alloc: *const c_void,
    bytes: *const u8,
    num_bytes: CFIndex,
    encoding: u32,
    is_external_representation: u8,
) -> CFStringRef;
type FnCTFontCreateWithName = unsafe extern "C" fn(
    name: CFStringRef,
    size: CGFloat,
    matrix: *const c_void,
) -> CTFontRef;
type FnCTFontGetGlyphCount = unsafe extern "C" fn(font: CTFontRef) -> CFIndex;
type FnCTFontGetBoundingRectsForGlyphs = unsafe extern "C" fn(
    font: CTFontRef,
    orientation: u32,
    glyphs: *const CGGlyph,
    bounding_rects: *mut CGRect,
    count: isize,
) -> CGRect;
type FnCTFontDrawGlyphs = unsafe extern "C" fn(
    font: CTFontRef,
    glyphs: *const CGGlyph,
    positions: *const CGPoint,
    count: usize,
    context: CGContextRef,
);

type FnCFRelease = unsafe extern "C" fn(cf: CFTypeRef);

struct Lib {
    color_space_create_device_rgb: FnCGColorSpaceCreateDeviceRGB,
    bitmap_context_create: FnCGBitmapContextCreate,
    bitmap_context_get_data: FnCGBitmapContextGetData,
    bitmap_context_get_bytes_per_row: FnCGBitmapContextGetBytesPerRow,
    color_space_release: FnCGColorSpaceRelease,
    context_release: FnCGContextRelease,
    cf_string_create_with_bytes: FnCFStringCreateWithBytes,
    ct_font_create_with_name: FnCTFontCreateWithName,
    ct_font_get_glyph_count: FnCTFontGetGlyphCount,
    ct_font_get_bounding_rects: FnCTFontGetBoundingRectsForGlyphs,
    ct_font_draw_glyphs: FnCTFontDrawGlyphs,
    cf_release: FnCFRelease,
}

// SAFETY: the resolved function pointers are immutable code addresses in dylibs
// that stay loaded for the process lifetime.
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

fn lib() -> Option<&'static Lib> {
    static LIB: OnceLock<Option<Lib>> = OnceLock::new();
    LIB.get_or_init(|| unsafe {
        unsafe extern "C" {
            fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
            fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
        }
        let open = |path: &[u8]| dlopen(path.as_ptr() as *const i8, 1); // RTLD_LAZY
        let cg = open(b"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics\0");
        let ct = open(b"/System/Library/Frameworks/CoreText.framework/CoreText\0");
        let cf = open(b"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation\0");
        if cg.is_null() || ct.is_null() || cf.is_null() {
            return None;
        }
        let sym = |handle: *mut c_void, name: &[u8]| dlsym(handle, name.as_ptr() as *const i8);

        let color_space_create_device_rgb = sym(cg, b"CGColorSpaceCreateDeviceRGB\0");
        let bitmap_context_create = sym(cg, b"CGBitmapContextCreate\0");
        let bitmap_context_get_data = sym(cg, b"CGBitmapContextGetData\0");
        let bitmap_context_get_bytes_per_row = sym(cg, b"CGBitmapContextGetBytesPerRow\0");
        let color_space_release = sym(cg, b"CGColorSpaceRelease\0");
        let context_release = sym(cg, b"CGContextRelease\0");
        let cf_string_create_with_bytes = sym(cf, b"CFStringCreateWithBytes\0");
        let ct_font_create_with_name = sym(ct, b"CTFontCreateWithName\0");
        let ct_font_get_glyph_count = sym(ct, b"CTFontGetGlyphCount\0");
        let ct_font_get_bounding_rects = sym(ct, b"CTFontGetBoundingRectsForGlyphs\0");
        let ct_font_draw_glyphs = sym(ct, b"CTFontDrawGlyphs\0");
        let cf_release = sym(cf, b"CFRelease\0");

        if color_space_create_device_rgb.is_null()
            || bitmap_context_create.is_null()
            || bitmap_context_get_data.is_null()
            || bitmap_context_get_bytes_per_row.is_null()
            || color_space_release.is_null()
            || context_release.is_null()
            || cf_string_create_with_bytes.is_null()
            || ct_font_create_with_name.is_null()
            || ct_font_get_glyph_count.is_null()
            || ct_font_get_bounding_rects.is_null()
            || ct_font_draw_glyphs.is_null()
            || cf_release.is_null()
        {
            return None;
        }

        Some(Lib {
            color_space_create_device_rgb: std::mem::transmute::<
                *mut c_void,
                FnCGColorSpaceCreateDeviceRGB,
            >(color_space_create_device_rgb),
            bitmap_context_create: std::mem::transmute::<*mut c_void, FnCGBitmapContextCreate>(
                bitmap_context_create,
            ),
            bitmap_context_get_data: std::mem::transmute::<*mut c_void, FnCGBitmapContextGetData>(
                bitmap_context_get_data,
            ),
            bitmap_context_get_bytes_per_row: std::mem::transmute::<
                *mut c_void,
                FnCGBitmapContextGetBytesPerRow,
            >(bitmap_context_get_bytes_per_row),
            color_space_release: std::mem::transmute::<*mut c_void, FnCGColorSpaceRelease>(
                color_space_release,
            ),
            context_release: std::mem::transmute::<*mut c_void, FnCGContextRelease>(
                context_release,
            ),
            cf_string_create_with_bytes: std::mem::transmute::<
                *mut c_void,
                FnCFStringCreateWithBytes,
            >(cf_string_create_with_bytes),
            ct_font_create_with_name: std::mem::transmute::<*mut c_void, FnCTFontCreateWithName>(
                ct_font_create_with_name,
            ),
            ct_font_get_glyph_count: std::mem::transmute::<*mut c_void, FnCTFontGetGlyphCount>(
                ct_font_get_glyph_count,
            ),
            ct_font_get_bounding_rects: std::mem::transmute::<
                *mut c_void,
                FnCTFontGetBoundingRectsForGlyphs,
            >(ct_font_get_bounding_rects),
            ct_font_draw_glyphs: std::mem::transmute::<*mut c_void, FnCTFontDrawGlyphs>(
                ct_font_draw_glyphs,
            ),
            cf_release: std::mem::transmute::<*mut c_void, FnCFRelease>(cf_release),
        })
    })
    .as_ref()
}

/// Quantize a requested render ppem to the nearest integer, bounded to a sane
/// range, so tiny per-frame jitter in `dpxs_per_em` doesn't spawn a fresh CTFont
/// (and a fresh atlas slot) every frame.
fn quantize_ppem(dpxs_per_em: f32) -> u32 {
    (dpxs_per_em.round() as i64).clamp(1, 512) as u32
}

/// The pixels produced by drawing one glyph, plus its placement, ready to copy
/// into the color atlas. Holds the live `CGContextRef` until `decode`; released
/// on `Drop`.
pub struct ColorRaster {
    size: Size<usize>,
    origin_in_dpxs: Point<f32>,
    dpxs_per_em: f32,
    ctx: CGContextRef,
}

impl ColorRaster {
    pub fn size(&self) -> Size<usize> {
        self.size
    }

    pub fn origin_in_dpxs(&self) -> Point<f32> {
        self.origin_in_dpxs
    }

    pub fn dpxs_per_em(&self) -> f32 {
        self.dpxs_per_em
    }

    /// Copy the drawn pixels into `image` (sized exactly `self.size()`),
    /// un-premultiplying alpha back to straight alpha (the color atlas stores
    /// straight alpha — the shader multiplies alpha itself). No vertical flip:
    /// drawing with an identity CTM lands the pixels top-down (see below).
    pub fn decode(&self, image: &mut SubimageMut<Bgra>) {
        let Some(lib) = lib() else { return };
        let w = self.size.width;
        let h = self.size.height;
        if w == 0 || h == 0 {
            return;
        }
        let data = unsafe { (lib.bitmap_context_get_data)(self.ctx) } as *const u8;
        if data.is_null() {
            return;
        }
        let stride = unsafe { (lib.bitmap_context_get_bytes_per_row)(self.ctx) };

        #[inline]
        fn unpremultiply(c: u8, a: u8) -> u8 {
            if a == 0 {
                0
            } else {
                let v = (c as u32 * 255 + a as u32 / 2) / a as u32;
                v.min(255) as u8
            }
        }

        for dst_y in 0..h {
            // Empirically, drawing a glyph with an identity CTM into this bitmap
            // context lands the pixels already top-down (row 0 = top), matching our
            // `SubimageMut` orientation — no vertical flip needed. (Flipping here
            // rendered every emoji upside-down on-device.)
            let row = unsafe { data.add(dst_y * stride) };
            for x in 0..w {
                let px = unsafe { row.add(x * 4) };
                // Memory byte order is [B, G, R, A] (see bitmap_info).
                let b = unsafe { *px };
                let g = unsafe { *px.add(1) };
                let r = unsafe { *px.add(2) };
                let a = unsafe { *px.add(3) };
                image[Point::new(x, dst_y)] = Bgra::new(
                    unpremultiply(b, a),
                    unpremultiply(g, a),
                    unpremultiply(r, a),
                    a,
                );
            }
        }
    }
}

impl Drop for ColorRaster {
    fn drop(&mut self) {
        if let Some(lib) = lib() {
            if !self.ctx.is_null() {
                unsafe { (lib.context_release)(self.ctx) };
            }
        }
    }
}

/// A CoreText color-glyph rasterizer bound to a single color-emoji font,
/// re-queried from CoreText by PostScript name. Holds a small ppem→`CTFont`
/// cache (each `CTFont` is created lazily at its render size). Nothing here
/// references the reassembled sfnt bytes — CoreText opens the real (lazily
/// mmapped) system font itself.
pub struct ColorEmojiRenderer {
    // PostScript name of the font (UTF-8), used to (re)create sized CTFonts.
    ps_name: String,
    // ppem -> CTFont (size baked in). Small: a handful of distinct render sizes.
    ct_fonts: RefCell<Vec<(u32, CTFontRef)>>,
}

impl std::fmt::Debug for ColorEmojiRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColorEmojiRenderer").finish_non_exhaustive()
    }
}

// SAFETY: the CG/CT handles are only ever touched behind `Font`'s `RefCell` on
// the single UI thread; `ColorEmojiRenderer` is never actually shared across
// threads. This mirrors `HvglRenderer`.
unsafe impl Send for ColorEmojiRenderer {}
unsafe impl Sync for ColorEmojiRenderer {}

/// Create a CFString from a UTF-8 `&str` (caller must CFRelease). `None` on
/// failure.
unsafe fn cfstring(lib: &Lib, s: &str) -> Option<CFStringRef> {
    let cf = unsafe {
        (lib.cf_string_create_with_bytes)(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as CFIndex,
            K_CF_STRING_ENCODING_UTF8,
            0,
        )
    };
    if cf.is_null() {
        None
    } else {
        Some(cf)
    }
}

impl ColorEmojiRenderer {
    /// Open a renderer by re-querying CoreText for the color-emoji font.
    ///
    /// `expected_glyph_count` is the glyph count `ttf_parser` sees for the
    /// reassembled face; we only accept a CoreText font whose `CTFontGetGlyphCount`
    /// matches it — a mismatch means CoreText resolved a *different* `.ttc` face
    /// (Apple's emoji `.ttc` holds several: the full face, a `…UI` variant, etc.)
    /// whose glyph ids wouldn't line up with the ones `ttf_parser` gave us.
    ///
    /// Complication: the reassembled face's PostScript name (name id 6) is
    /// `.AppleColorEmojiUI` — a *system-reserved* name (leading dot) that
    /// `CTFontCreateWithName` refuses to resolve (it silently substitutes Times),
    /// and which anyway names a different-sized variant than the face we hold. So
    /// we don't trust the name verbatim: we derive a few candidate names from it
    /// (strip the leading dot, strip a trailing `UI`) plus the well-known
    /// `AppleColorEmoji`, and pick the first whose glyph count matches. The
    /// glyph-count check is what guarantees correctness; the candidate list just
    /// widens the search. `None` if none match or CoreGraphics/CoreText are
    /// unavailable.
    pub fn open_by_name(ps_name: &str, expected_glyph_count: u16) -> Option<ColorEmojiRenderer> {
        let lib = lib()?;

        // Candidate names, in priority order, de-duplicated. Order doesn't matter
        // for correctness (glyph count decides) — it's just fewer CoreText calls.
        let stripped = ps_name.strip_prefix('.').unwrap_or(ps_name);
        let no_ui = stripped.strip_suffix("UI").unwrap_or(stripped);
        let mut candidates: Vec<&str> = Vec::new();
        for c in [ps_name, stripped, no_ui, "AppleColorEmoji"] {
            if !c.is_empty() && !c.starts_with('.') && !candidates.contains(&c) {
                candidates.push(c);
            }
        }

        for name in candidates {
            // Probe at an arbitrary size (size doesn't affect glyph count).
            let Some(cf_name) = (unsafe { cfstring(lib, name) }) else { continue };
            let probe = unsafe { (lib.ct_font_create_with_name)(cf_name, 16.0, std::ptr::null()) };
            unsafe { (lib.cf_release)(cf_name) };
            if probe.is_null() {
                continue;
            }
            let glyph_count = unsafe { (lib.ct_font_get_glyph_count)(probe) };
            unsafe { (lib.cf_release)(probe as CFTypeRef) };
            if glyph_count == expected_glyph_count as CFIndex {
                return Some(ColorEmojiRenderer {
                    ps_name: name.to_string(),
                    ct_fonts: RefCell::new(Vec::new()),
                });
            }
        }
        None
    }

    /// Get (or lazily create) a `CTFont` at `ppem` pixels-per-em. The returned
    /// pointer is owned by the cache; the caller must not release it.
    fn ct_font(&self, lib: &Lib, ppem: u32) -> Option<CTFontRef> {
        {
            let cache = self.ct_fonts.borrow();
            if let Some((_, font)) = cache.iter().find(|(p, _)| *p == ppem) {
                return Some(*font);
            }
        }
        let cf_name = unsafe { cfstring(lib, &self.ps_name)? };
        let font = unsafe {
            (lib.ct_font_create_with_name)(cf_name, ppem as CGFloat, std::ptr::null())
        };
        unsafe { (lib.cf_release)(cf_name) };
        if font.is_null() {
            return None;
        }
        self.ct_fonts.borrow_mut().push((ppem, font));
        Some(font)
    }

    /// Draw `glyph_id` at `dpxs_per_em` into a fresh BGRA bitmap. `None` if the
    /// glyph has no ink (empty bbox) or any CoreGraphics call fails — the caller
    /// then falls back to the outline path.
    pub fn rasterize(&self, glyph_id: u16, dpxs_per_em: f32) -> Option<ColorRaster> {
        let lib = lib()?;
        let ppem = quantize_ppem(dpxs_per_em);
        let ct_font = self.ct_font(lib, ppem)?;

        let gid: CGGlyph = glyph_id;
        let bbox = unsafe {
            (lib.ct_font_get_bounding_rects)(
                ct_font,
                0, // kCTFontOrientationDefault
                &gid,
                std::ptr::null_mut(),
                1,
            )
        };
        if !bbox.size.width.is_finite() || !bbox.size.height.is_finite() {
            return None;
        }
        // y-up ink bbox, already scaled to `ppem`. Snap out to integer pixels.
        let x0 = bbox.origin.x.floor();
        let y0 = bbox.origin.y.floor();
        let w = ((bbox.origin.x + bbox.size.width).ceil() - x0) as i64;
        let h = ((bbox.origin.y + bbox.size.height).ceil() - y0) as i64;
        if w <= 0 || h <= 0 {
            return None;
        }
        let w = w as usize;
        let h = h as usize;

        let color_space = unsafe { (lib.color_space_create_device_rgb)() };
        if color_space.is_null() {
            return None;
        }
        let ctx = unsafe {
            (lib.bitmap_context_create)(
                std::ptr::null_mut(), // let CG allocate the backing store
                w,
                h,
                8,
                0, // 0 => CG picks a suitable (aligned) bytes-per-row
                color_space,
                K_CG_BITMAP_BGRA_PREMULTIPLIED,
            )
        };
        unsafe { (lib.color_space_release)(color_space) };
        if ctx.is_null() {
            return None;
        }

        // Identity CTM: 1 em == `ppem` px; translate the glyph's ink origin to
        // (0, 0) so the ink exactly fills [0, w) x [0, h).
        let pos = CGPoint { x: -x0, y: -y0 };
        unsafe { (lib.ct_font_draw_glyphs)(ct_font, &gid, &pos, 1, ctx) };

        Some(ColorRaster {
            size: Size::new(w, h),
            origin_in_dpxs: Point::new(x0 as f32, y0 as f32),
            dpxs_per_em: ppem as f32,
            ctx,
        })
    }
}

impl Drop for ColorEmojiRenderer {
    fn drop(&mut self) {
        if let Some(lib) = lib() {
            for (_, font) in self.ct_fonts.borrow_mut().drain(..) {
                if !font.is_null() {
                    unsafe { (lib.cf_release)(font as CFTypeRef) };
                }
            }
        }
    }
}
