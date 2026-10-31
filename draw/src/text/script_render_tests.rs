//! Renders sample lines of complex and CJK scripts through the text stack
//! (theme face IBM Plex, every other script through the OS font fallback,
//! rustybuzz shaping, glyph outlines) into PNGs, and on macOS compares them
//! with CoreText drawing the same strings (glyph ids, positions, pixels).
//!
//! `SCRIPT_RENDER_OUT=<dir> cargo test --release -p makepad-draw script_render -- --ignored --nocapture`

use {
    super::{
        font::{Font, FontId},
        font_family::FontFamilyId,
        glyph_outline::Command,
        layouter::{BorrowedLayoutParams, LayoutOptions, Layouter, Settings, Style},
        loader::{FontDefinition, FontFamilyDefinition},
        system_fallback::SystemFallback,
    },
    crate::makepad_platform::SharedBytes,
    std::{fmt::Write as _, path::PathBuf},
};

pub(crate) const SAMPLES: &[(&str, &str)] = &[
    // conjuncts (स्त्र, क्ष, न्द), the i-matra reordered before its consonant
    // (हि, कि), reph (र्म), nukta (ज़), half forms, candrabindu/anusvara
    ("hindi", "हिन्दी गीत: धर्म कर्म, स्त्री क्षत्रिय, प्रेम की ज़िंदगी"),
    // joining forms, lam-alef, harakat stacking, RTL with Arabic-Indic digits
    ("arabic", "مرحبا بالعالم، السَّلَامُ عَلَيْكُمْ ١٢٣"),
    // tone marks over upper vowels (ที่ นี่ น้ำ), sara am
    ("thai", "สวัสดีครับ ภาษาไทย น้ำ ที่นี่"),
    ("japanese", "こんにちは世界、日本語のテキスト"),
    ("korean", "안녕하세요 세계, 한국어 텍스트"),
];

const PX: f32 = 32.0;
const W: usize = 1100;
const H: usize = 60;
const BASELINE: f32 = 42.0;
const LEFT: f32 = 8.0;

pub(crate) struct OurLine {
    pub coverage: Vec<u8>,
    /// (font PostScript name, glyph id, pen x + offset, y offset), visual order.
    pub glyphs: Vec<(String, u16, f32, f32)>,
    pub missing: usize,
    pub fonts: Vec<String>,
}

fn postscript_name(font: &Font) -> String {
    font.with_ttf_parser_face(|face| {
        face.names()
            .into_iter()
            .filter(|n| n.name_id == 6)
            .find_map(|n| n.to_string())
            .unwrap_or_default()
    })
}

/// A layouter with two families: IBM Plex alone (every other script from
/// the OS), and the International theme chain (IBM Plex, then the bundled
/// LXGW WenKai for CJK).
fn theme_layouter() -> (Layouter, FontFamilyId, FontFamilyId) {
    let mut layouter = Layouter::new(Settings::default());
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources");
    let mut ids = Vec::new();
    for (n, file) in ["IBMPlexSans-Text.ttf", "LXGWWenKaiRegular.ttf"].iter().enumerate() {
        let data = SharedBytes::from_file_mmap_or_read(resources.join(file)).expect("theme font");
        let font_id = FontId::from(0x5C21_0001_u64 + n as u64);
        layouter.define_font(
            font_id,
            FontDefinition { data, index: 0, ascender_fudge_in_ems: 0.0, descender_fudge_in_ems: 0.0, weight: None, variations: Vec::new() },
        );
        ids.push(font_id);
    }
    let plex = FontFamilyId::from(0x5C21_0101_u64);
    let theme = FontFamilyId::from(0x5C21_0102_u64);
    for (family, font_ids) in [(plex, vec![ids[0]]), (theme, ids.clone())] {
        layouter.define_font_family(
            family,
            FontFamilyDefinition { expected_member_count: font_ids.len(), font_ids, diagnostics: Default::default() },
        );
    }
    (layouter, plex, theme)
}

fn layout_line(layouter: &mut Layouter, family_id: FontFamilyId, text: &str) -> std::rc::Rc<super::layouter::LaidoutText> {
    layouter.get_or_layout(BorrowedLayoutParams {
        text,
        style: Style { font_family_id: family_id, font_size_in_pts: PX * 72.0 / 96.0, color: None },
        options: LayoutOptions::default(),
    })
}

pub(crate) fn render_ours(layouter: &mut Layouter, fallback: &mut SystemFallback, family_id: FontFamilyId, text: &str) -> OurLine {
    let mut laidout = layout_line(layouter, family_id, text);
    if laidout.rows.iter().flat_map(|r| &r.glyphs).any(|g| g.id == 0) && fallback.request(layouter, family_id, text) {
        laidout = layout_line(layouter, family_id, text);
    }
    let mut rasterizer = ab_glyph_rasterizer::Rasterizer::new(W, H);
    let mut glyphs = Vec::new();
    let mut missing = 0;
    let mut fonts: Vec<String> = Vec::new();
    for row in &laidout.rows {
        for glyph in &row.glyphs {
            let name = postscript_name(&glyph.font);
            if !fonts.contains(&name) {
                fonts.push(name.clone());
            }
            if glyph.id == 0 {
                missing += 1;
            }
            let x0 = LEFT + row.origin_in_lpxs.x + glyph.origin_in_lpxs.x + glyph.offset_in_lpxs();
            let y0 = BASELINE + glyph.origin_in_lpxs.y;
            glyphs.push((name, glyph.id, x0 - LEFT, -glyph.origin_in_lpxs.y));
            let Some(outline) = glyph.font.glyph_outline(glyph.id) else { continue };
            let scale = glyph.font_size_in_lpxs / glyph.font.units_per_em();
            let t = |p: super::geom::Point<f32>| ab_glyph_rasterizer::point(x0 + p.x * scale, y0 - p.y * scale);
            let mut last = super::geom::Point::new(0.0, 0.0);
            let mut first = last;
            for command in outline.commands().iter().copied() {
                match command {
                    Command::MoveTo(p) => {
                        // a contour ends where the next starts: close it (as `GlyphOutline::rasterize`)
                        if last != first {
                            rasterizer.draw_line(t(last), t(first));
                        }
                        last = p;
                        first = p;
                    }
                    Command::LineTo(p) => {
                        rasterizer.draw_line(t(last), t(p));
                        last = p;
                    }
                    Command::QuadTo(p1, p) => {
                        rasterizer.draw_quad(t(last), t(p1), t(p));
                        last = p;
                    }
                    Command::CurveTo(p1, p2, p) => {
                        rasterizer.draw_cubic(t(last), t(p1), t(p2), t(p));
                        last = p;
                    }
                    Command::Close => {
                        rasterizer.draw_line(t(last), t(first));
                        last = first;
                    }
                }
            }
            if last != first {
                rasterizer.draw_line(t(last), t(first));
            }
        }
    }
    let mut coverage = vec![0u8; W * H];
    rasterizer.for_each_pixel_2d(|x, y, a| {
        let i = y as usize * W + x as usize;
        coverage[i] = (coverage[i] as f32 + a.min(1.0) * 255.0).min(255.0) as u8;
    });
    OurLine { coverage, glyphs, missing, fonts }
}

pub(crate) fn write_png(path: &std::path::Path, width: usize, height: usize, gray: &[u8]) {
    use makepad_zune_png::{
        makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
        PngEncoder,
    };
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::Luma);
    // dark text on white, as a page shows it
    let page: Vec<u8> = gray.iter().map(|&v| 255 - v).collect();
    let mut out = Vec::new();
    PngEncoder::new(&page, options).encode(&mut out).expect("png");
    std::fs::write(path, out).expect("write png");
}

/// Ink bounds (x0, y0, x1, y1) of coverage above a threshold.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn ink_bounds(c: &[u8]) -> Option<(usize, usize, usize, usize)> {
    let mut b: Option<(usize, usize, usize, usize)> = None;
    for y in 0..H {
        for x in 0..W {
            if c[y * W + x] > 64 {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

/// Pixel agreement of two renders of one line: mean absolute coverage
/// difference over the union of their ink (0..255), and the IoU of their
/// ink masks.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn compare(a: &[u8], b: &[u8]) -> (f32, f32) {
    let (Some(ba), Some(bb)) = (ink_bounds(a), ink_bounds(b)) else {
        return (255.0, 0.0);
    };
    let (x0, y0, x1, y1) = (ba.0.min(bb.0), ba.1.min(bb.1), ba.2.max(bb.2), ba.3.max(bb.3));
    let (mut diff, mut n, mut inter, mut uni) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (va, vb) = (a[y * W + x], b[y * W + x]);
            if va > 0 || vb > 0 {
                diff += (va as i32 - vb as i32).unsigned_abs() as u64;
                n += 1;
            }
            let (ia, ib) = (va > 127, vb > 127);
            inter += (ia && ib) as u64;
            uni += (ia || ib) as u64;
        }
    }
    (diff as f32 / n.max(1) as f32, inter as f32 / uni.max(1) as f32)
}

#[cfg(target_os = "macos")]
mod coretext_reference {
    //! The OS's own rendering: CoreText draws the string with IBM Plex and
    //! its own fallback cascade, into a gray bitmap.
    use super::{BASELINE, H, LEFT, PX, W};
    use std::ffi::c_void;

    type CFIndex = isize;
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    #[repr(C)]
    struct CFRange {
        location: CFIndex,
        length: CFIndex,
    }
    #[repr(C)]
    struct CFCallBacks {
        _priv: [usize; 7],
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
        fn CFStringCreateWithBytes(a: *const c_void, b: *const u8, n: CFIndex, e: u32, x: bool) -> *const c_void;
        fn CFStringGetCString(s: *const c_void, buf: *mut u8, len: CFIndex, e: u32) -> bool;
        fn CFDictionaryCreate(
            a: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            n: CFIndex,
            kc: *const CFCallBacks,
            vc: *const CFCallBacks,
        ) -> *const c_void;
        fn CFDictionaryGetValue(d: *const c_void, key: *const c_void) -> *const c_void;
        fn CFAttributedStringCreate(a: *const c_void, s: *const c_void, attrs: *const c_void) -> *const c_void;
        fn CFArrayGetCount(a: *const c_void) -> CFIndex;
        fn CFCopyDescription(cf: *const c_void) -> *const c_void;
        fn CFURLCreateFromFileSystemRepresentation(a: *const c_void, b: *const u8, n: CFIndex, dir: bool) -> *const c_void;
        fn CFArrayGetValueAtIndex(a: *const c_void, i: CFIndex) -> *const c_void;
        static kCFTypeDictionaryKeyCallBacks: CFCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFCallBacks;
        static kCFBooleanTrue: *const c_void;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorSpaceCreateDeviceGray() -> *const c_void;
        fn CGColorSpaceRelease(c: *const c_void);
        fn CGBitmapContextCreate(d: *mut c_void, w: usize, h: usize, bpc: usize, bpr: usize, cs: *const c_void, info: u32) -> *const c_void;
        fn CGContextRelease(c: *const c_void);
        fn CGContextSetGrayFillColor(c: *const c_void, g: f64, a: f64);
        fn CGContextFillRect(c: *const c_void, r: CGRect);
        fn CGContextSetTextPosition(c: *const c_void, x: f64, y: f64);
        fn CGContextSetShouldSmoothFonts(c: *const c_void, b: bool);
        fn CGContextSetShouldAntialias(c: *const c_void, b: bool);
        fn CGContextSetShouldSubpixelPositionFonts(c: *const c_void, b: bool);
        fn CGContextSetAllowsFontSubpixelPositioning(c: *const c_void, b: bool);
        fn CGContextSetShouldSubpixelQuantizeFonts(c: *const c_void, b: bool);
    }
    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        fn CTFontCreateUIFontForLanguage(t: u32, size: f64, lang: *const c_void) -> *const c_void;
        fn CTFontCreateForString(f: *const c_void, s: *const c_void, r: CFRange) -> *const c_void;
        fn CTFontCreateWithName(n: *const c_void, size: f64, m: *const c_void) -> *const c_void;
        fn CTFontCopyVariation(f: *const c_void) -> *const c_void;
        fn CTFontManagerCreateFontDescriptorsFromURL(url: *const c_void) -> *const c_void;
        fn CTFontCreateWithFontDescriptor(d: *const c_void, size: f64, m: *const c_void) -> *const c_void;
        fn CTFontCopyPostScriptName(f: *const c_void) -> *const c_void;
        fn CTLineCreateWithAttributedString(s: *const c_void) -> *const c_void;
        fn CTLineDraw(l: *const c_void, c: *const c_void);
        fn CTLineGetGlyphRuns(l: *const c_void) -> *const c_void;
        fn CTRunGetGlyphCount(r: *const c_void) -> CFIndex;
        fn CTRunGetGlyphs(r: *const c_void, range: CFRange, out: *mut u16);
        fn CTRunGetPositions(r: *const c_void, range: CFRange, out: *mut CGPoint);
        fn CTRunGetAttributes(r: *const c_void) -> *const c_void;
        static kCTFontAttributeName: *const c_void;
        static kCTForegroundColorFromContextAttributeName: *const c_void;
    }

    unsafe fn cf_str(s: &str) -> *const c_void {
        CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, 0x0800_0100, false)
    }

    unsafe fn to_string(s: *const c_void) -> String {
        let mut buf = [0u8; 256];
        if s.is_null() || !CFStringGetCString(s, buf.as_mut_ptr(), 256, 0x0800_0100) {
            return String::new();
        }
        let n = buf.iter().position(|&b| b == 0).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    /// Coverage (row 0 at the top) and the glyphs CoreText chose:
    /// (font PostScript name, glyph id, x, y up from the baseline).
    /// `plex`: cascade from IBM Plex (as the theme); else from the system UI
    /// font, whose cascade is the one our OS lookup asks for.
    pub fn render(text: &str, plex: bool) -> (Vec<u8>, Vec<(String, u16, f32, f32)>) {
        unsafe {
            let font = base_font(plex);
            render_with_font(text, font)
        }
    }

    pub unsafe fn base_font(plex: bool) -> *const c_void {
        {
            if plex {
                // the theme face, as our side: CoreText cascades from it to the OS fonts
                let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../widgets/resources/IBMPlexSans-Text.ttf");
                let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), path.as_ptr(), path.len() as CFIndex, false);
                let descriptors = CTFontManagerCreateFontDescriptorsFromURL(url);
                let font = CTFontCreateWithFontDescriptor(CFArrayGetValueAtIndex(descriptors, 0), PX as f64, std::ptr::null());
                CFRelease(descriptors);
                CFRelease(url);
                font
            } else {
                CTFontCreateUIFontForLanguage(2, PX as f64, std::ptr::null())
            }
        }
    }

    /// The face CoreText's UI cascade picks for `text`, and its variation
    /// (axis values CoreText applies, e.g. optical size), described.
    pub unsafe fn ui_fallback_font(text: &str) -> (*const c_void, String) {
        let base = base_font(false);
        let string = cf_str(text);
        let n = text.encode_utf16().count() as CFIndex;
        let font = CTFontCreateForString(base, string, CFRange { location: 0, length: n });
        CFRelease(string);
        CFRelease(base);
        let variation = CTFontCopyVariation(font);
        let described = if variation.is_null() {
            "none".to_string()
        } else {
            let d = CFCopyDescription(variation);
            let mut buf = vec![0u8; 2048];
            let out = if CFStringGetCString(d, buf.as_mut_ptr(), 2048, 0x0800_0100) {
                let n = buf.iter().position(|&b| b == 0).unwrap_or(0);
                String::from_utf8_lossy(&buf[..n]).split_whitespace().collect::<Vec<_>>().join(" ")
            } else {
                String::new()
            };
            CFRelease(d);
            CFRelease(variation);
            out
        };
        (font, described)
    }

    pub unsafe fn named_font(name: &str) -> *const c_void {
        let n = cf_str(name);
        let font = CTFontCreateWithName(n, PX as f64, std::ptr::null());
        CFRelease(n);
        font
    }

    /// Draws and lists `text` set in `font` (released here).
    pub unsafe fn render_with_font(text: &str, font: *const c_void) -> (Vec<u8>, Vec<(String, u16, f32, f32)>) {
        {
            let keys = [kCTFontAttributeName, kCTForegroundColorFromContextAttributeName];
            let values = [font, kCFBooleanTrue];
            let attrs = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let string = cf_str(text);
            let attributed = CFAttributedStringCreate(std::ptr::null(), string, attrs);
            let line = CTLineCreateWithAttributedString(attributed);

            let mut pixels = vec![0u8; W * H];
            let space = CGColorSpaceCreateDeviceGray();
            let cx = CGBitmapContextCreate(pixels.as_mut_ptr() as *mut c_void, W, H, 8, W, space, 0);
            CGContextSetGrayFillColor(cx, 0.0, 1.0);
            CGContextFillRect(cx, CGRect { x: 0.0, y: 0.0, w: W as f64, h: H as f64 });
            CGContextSetShouldAntialias(cx, true);
            CGContextSetShouldSmoothFonts(cx, false);
            CGContextSetAllowsFontSubpixelPositioning(cx, true);
            CGContextSetShouldSubpixelPositionFonts(cx, true);
            CGContextSetShouldSubpixelQuantizeFonts(cx, false);
            CGContextSetGrayFillColor(cx, 1.0, 1.0);
            CGContextSetTextPosition(cx, LEFT as f64, (H as f32 - BASELINE) as f64);
            CTLineDraw(line, cx);

            let mut glyphs = Vec::new();
            let runs = CTLineGetGlyphRuns(line);
            for i in 0..CFArrayGetCount(runs) {
                let run = CFArrayGetValueAtIndex(runs, i);
                let n = CTRunGetGlyphCount(run);
                let mut ids = vec![0u16; n as usize];
                let mut pos = vec![CGPoint::default(); n as usize];
                CTRunGetGlyphs(run, CFRange { location: 0, length: 0 }, ids.as_mut_ptr());
                CTRunGetPositions(run, CFRange { location: 0, length: 0 }, pos.as_mut_ptr());
                let run_font = CFDictionaryGetValue(CTRunGetAttributes(run), kCTFontAttributeName);
                let ps = CTFontCopyPostScriptName(run_font);
                let name = to_string(ps);
                if !ps.is_null() {
                    CFRelease(ps);
                }
                for (id, p) in ids.iter().zip(&pos) {
                    glyphs.push((name.clone(), *id, p.x as f32, p.y as f32));
                }
            }

            CGContextRelease(cx);
            CGColorSpaceRelease(space);
            CFRelease(line);
            CFRelease(attributed);
            CFRelease(string);
            CFRelease(attrs);
            CFRelease(font);
            (pixels, glyphs)
        }
    }
}

/// Shaping agreement on the script's own font: the glyphs of fonts both
/// sides drew with (other than the Latin base faces), in visual order, cut
/// into segments where another font's glyphs intervene, each segment's
/// positions relative to its first glyph (so a different comma width before
/// it does not count). Returns (glyph ids equal, glyphs compared, largest
/// relative position difference in px, the shared fonts).
fn compare_glyphs(ours: &[(String, u16, f32, f32)], theirs: &[(String, u16, f32, f32)]) -> (usize, usize, f32, Vec<String>) {
    let base = |name: &str| name.contains("IBMPlex") || name.starts_with(".SFNS");
    let shared: Vec<String> = ours
        .iter()
        .map(|g| g.0.clone())
        .filter(|name| !base(name) && theirs.iter().any(|t| &t.0 == name))
        .fold(Vec::new(), |mut v, n| {
            if !v.contains(&n) {
                v.push(n)
            }
            v
        });
    let segments = |g: &[(String, u16, f32, f32)]| {
        let mut v: Vec<&(String, u16, f32, f32)> = g.iter().collect();
        v.sort_by(|a, b| ((a.2 * 4.0).round() as i32, a.1).cmp(&((b.2 * 4.0).round() as i32, b.1)));
        let mut out: Vec<Vec<(u16, f32, f32)>> = Vec::new();
        let mut open = false;
        for glyph in v {
            if shared.contains(&glyph.0) {
                if !open {
                    out.push(Vec::new());
                    open = true;
                }
                out.last_mut().unwrap().push((glyph.1, glyph.2, glyph.3));
            } else {
                open = false;
            }
        }
        out
    };
    let (a, b) = (segments(ours), segments(theirs));
    let (mut same, mut of, mut max_d) = (0, 0, 0.0f32);
    for (sa, sb) in a.iter().zip(&b) {
        of += sa.len().max(sb.len());
        let (xa, xb) = (sa[0].1, sb[0].1);
        for (ga, gb) in sa.iter().zip(sb) {
            if ga.0 == gb.0 {
                same += 1;
                max_d = max_d.max(((ga.1 - xa) - (gb.1 - xb)).abs()).max((ga.2 - gb.2).abs());
            }
        }
    }
    for extra in a.iter().skip(b.len()).chain(b.iter().skip(a.len())) {
        of += extra.len();
    }
    (same, of, max_d, shared)
}

#[test]
#[ignore]
fn script_render_samples() {
    let out = std::env::var_os("SCRIPT_RENDER_OUT").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("script_render"));
    std::fs::create_dir_all(&out).unwrap();
    makepad_font_core::system_fonts::prewarm();
    let (mut layouter, plex, theme) = theme_layouter();
    let mut fallback = SystemFallback::default();
    let mut report = String::new();
    let mut sheet: Vec<u8> = Vec::new();
    let mut boxes = 0;
    let runs = SAMPLES.iter().map(|(name, text)| (name.to_string(), *text, plex)).chain(
        SAMPLES.iter().filter(|(name, _)| matches!(*name, "japanese" | "korean")).map(|(name, text)| (format!("{name}.theme"), *text, theme)),
    );
    for (name, text, family_id) in runs {
        let ours = render_ours(&mut layouter, &mut fallback, family_id, text);
        write_png(&out.join(format!("{name}.ours.png")), W, H, &ours.coverage);
        sheet.extend_from_slice(&ours.coverage);
        boxes += ours.missing;
        let _ = writeln!(report, "{name}: fonts {:?}, {} glyphs, {} missing", ours.fonts, ours.glyphs.len(), ours.missing);
        #[cfg(target_os = "macos")]
        {
            // shaping: CoreText from the system UI font picks the faces our OS lookup picks
            let (ui_pixels, ui_glyphs) = coretext_reference::render(text, false);
            let (same, of, max_d, shared) = compare_glyphs(&ours.glyphs, &ui_glyphs);
            // pixels: CoreText cascading from the theme face (IBM Plex), as drawn on a page
            let (theirs, their_glyphs) = coretext_reference::render(text, true);
            write_png(&out.join(format!("{name}.coretext-ui.png")), W, H, &ui_pixels);
            write_png(&out.join(format!("{name}.coretext.png")), W, H, &theirs);
            sheet.extend_from_slice(&theirs);
            let fonts_of = |g: &[(String, u16, f32, f32)]| {
                let mut v: Vec<String> = Vec::new();
                for x in g {
                    if !v.contains(&x.0) {
                        v.push(x.0.clone());
                    }
                }
                v
            };
            let (mad, iou) = compare(&ours.coverage, &theirs);
            let (mad_ui, iou_ui) = compare(&ours.coverage, &ui_pixels);
            let _ = writeln!(
                report,
                "  shaping vs CoreText (system UI cascade {:?}): shared {shared:?}, glyph ids agree {same}/{of}, max relative position diff {max_d:.2} px; pixels vs it: mean |diff| {mad_ui:.1}/255, ink IoU {iou_ui:.3}",
                fonts_of(&ui_glyphs)
            );
            let _ = writeln!(
                report,
                "  CoreText cascading from IBM Plex picks {:?}; pixels vs it: mean |diff| {mad:.1}/255, ink IoU {iou:.3}",
                fonts_of(&their_glyphs)
            );
        }
        sheet.extend(std::iter::repeat(128u8).take(W * 2));
    }
    let rows = sheet.len() / W;
    write_png(&out.join("sheet.png"), W, rows, &sheet);
    std::fs::write(out.join("report.txt"), &report).unwrap();
    let tsv: String = SAMPLES.iter().map(|(name, text)| format!("{name}\t{text}\n")).collect();
    std::fs::write(out.join("samples.tsv"), tsv).unwrap();
    println!("{report}");
    println!("SCRIPT-RENDER boxes={boxes} out={}", out.display());
}

/// Per word: the glyph ids rustybuzz (our shaper) and CoreText give for the
/// same Devanagari face, for the face CoreText's UI cascade picks (SF
/// Devanagari) and Kohinoor Devanagari.
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn devanagari_words_vs_coretext() {
    use makepad_font_core::system_fonts;
    let words = ["हिन्दी", "गीत", "धर्म", "कर्म", "स्त्री", "क्षत्रिय", "प्रेम", "की", "ज़िंदगी", "कि", "र्कि", "द्ध", "ह्म"];
    let sf = system_fonts::fallback_candidates("क", false);
    let fonts: Vec<(String, PathBuf, u32)> = vec![
        ("SF Devanagari (UI cascade)".into(), sf[0].path.clone(), sf[0].index),
        ("KohinoorDevanagari-Regular".into(), PathBuf::from("/System/Library/Fonts/Kohinoor.ttc"), u32::MAX),
    ];
    let mut agree = 0;
    let mut total = 0;
    for (label, path, index) in fonts {
        let data = SharedBytes::from_file_mmap_or_read(&path).unwrap();
        let index = if index == u32::MAX {
            let n = rustybuzz::ttf_parser::fonts_in_collection(data.as_slice()).unwrap_or(1);
            (0..n)
                .find(|&i| {
                    rustybuzz::ttf_parser::Face::parse(data.as_slice(), i).ok().is_some_and(|f| {
                        f.names().into_iter().any(|nm| nm.name_id == 6 && nm.to_string().as_deref() == Some("KohinoorDevanagari-Regular"))
                    })
                })
                .unwrap()
        } else {
            index
        };
        let mut layouter = Layouter::new(Settings::default());
        let font_id = FontId::from(0xDE7A_0001_u64 + total as u64);
        layouter.define_font(
            font_id,
            FontDefinition { data, index, ascender_fudge_in_ems: 0.0, descender_fudge_in_ems: 0.0, weight: None, variations: Vec::new() },
        );
        let family = FontFamilyId::from(0xDE7A_1000_u64 + total as u64);
        layouter.define_font_family(family, FontFamilyDefinition { font_ids: vec![font_id], expected_member_count: 1, diagnostics: Default::default() });
        println!("== {label} ({}#{index})", path.display());
        for word in words {
            let ours: Vec<u16> = layout_line(&mut layouter, family, word).rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.id)).collect();
            let (font, variation) = unsafe {
                if label.starts_with("SF") {
                    coretext_reference::ui_fallback_font(word)
                } else {
                    (coretext_reference::named_font("KohinoorDevanagari-Regular"), String::new())
                }
            };
            let (_, theirs) = unsafe { coretext_reference::render_with_font(word, font) };
            let theirs: Vec<u16> = theirs.iter().map(|g| g.1).collect();
            total += 1;
            let same = ours == theirs;
            agree += same as usize;
            println!("{} {word}: ours {ours:?} coretext {theirs:?}{}", if same { "OK  " } else { "DIFF" }, if variation.is_empty() { String::new() } else { format!(" ct-variation {variation}") });
        }
    }
    println!("DEVANAGARI-WORDS agree {agree}/{total}");
}
