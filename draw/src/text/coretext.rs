//! CoreText outline fallback (macOS only).
//!
//! Apple has started shipping system fonts whose only outline table is the
//! proprietary `hvgl` ("hierarchical variable glyphs") format — e.g.
//! PingFangUI.ttc on macOS 26+ carries no `glyf`/`CFF`/`CFF2` at all, so
//! `ttf_parser::Face::outline_glyph` returns `None` for every glyph and the
//! text stack renders nothing. Everything else in such fonts (`cmap`,
//! `hmtx`/`HVAR`, `GSUB`) is standard and keeps working through ttf_parser /
//! rustybuzz.
//!
//! This module fills exactly that gap: when ttf_parser yields no outline,
//! `Font::glyph_outline` asks CoreText — whose in-OS decoder understands
//! `hvgl` — for the glyph path via `CTFontCreatePathForGlyph`, and converts
//! the resulting `CGPath` into the same [`GlyphOutline`] commands the SDF
//! rasterizer already consumes. Shaping, metrics, caching and the atlas are
//! untouched.
//!
//! Getting a `CTFont` for the face is the subtle part. CoreGraphics rejects
//! hvgl-only fonts loaded from raw data (`CGFontCreateWithDataProvider`
//! validates for classic outline tables) and `CTFontManager` refuses them by
//! URL too — the decoder is only reachable through *system-registered*
//! fonts. `CTFontCreateWithName` in turn silently substitutes a fallback for
//! the dot-prefixed hidden names these fonts use. So resolution goes:
//!
//! 1. by the face's PostScript name (works for non-hidden fonts), then
//! 2. by scanning the system UI font's default cascade lists for CJK
//!    languages — hidden system fonts like `.PingFangUITextSC-Default` are
//!    reachable there — then
//! 3. by writing the face out as a standalone sfnt and loading it via
//!    `CTFontManagerCreateFontDescriptorsFromURL` (for any future
//!    ttf_parser-unreadable font that CoreText *can* load from disk).
//!
//! Every candidate is validated against the ttf_parser view of the face —
//! glyph count plus a handful of cmap probes — so a substituted or merely
//! similar font can never smuggle mismatched glyph IDs into the atlas.
//!
//! The `CTFont` is sized at `units_per_em`, so glyph paths come back 1:1 in
//! font units, y-up — the same space ttf_parser outlines use.

use {
    super::{
        geom::{Point, Rect},
        glyph_outline::{Builder, GlyphOutline},
    },
    rustybuzz,
    rustybuzz::ttf_parser,
    rustybuzz::ttf_parser::OutlineBuilder,
    std::ffi::c_void,
    std::path::PathBuf,
};

// ─── Minimal CoreFoundation / CoreGraphics / CoreText FFI ───

type CGFloat = f64;
type CFIndex = isize;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: CGFloat,
    y: CGFloat,
}

#[repr(C)]
struct CGSize {
    width: CGFloat,
    height: CGFloat,
}

#[repr(C)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(C)]
struct CGPathElement {
    kind: i32,
    points: *const CGPoint,
}

// Opaque layout stand-ins for CFDictionaryKeyCallBacks / ValueCallBacks /
// CFArrayCallBacks (version field + function pointers); only their addresses
// are ever used.
#[repr(C)]
struct CFCallBacks {
    _priv: [usize; 7],
}

const KCF_NUMBER_SINT64_TYPE: CFIndex = 4;
const KCF_NUMBER_FLOAT64_TYPE: CFIndex = 6;
const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const KCT_FONT_UI_FONT_SYSTEM: u32 = 2;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const CFCallBacks,
        value_callbacks: *const CFCallBacks,
    ) -> *const c_void;
    fn CFNumberCreate(
        allocator: *const c_void,
        the_type: CFIndex,
        value_ptr: *const c_void,
    ) -> *const c_void;
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        num_bytes: CFIndex,
        encoding: u32,
        is_external_representation: bool,
    ) -> *const c_void;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: *const c_void,
        buffer: *const u8,
        buf_len: CFIndex,
        is_directory: bool,
    ) -> *const c_void;
    fn CFArrayCreate(
        allocator: *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        callbacks: *const CFCallBacks,
    ) -> *const c_void;
    fn CFArrayGetCount(array: *const c_void) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: *const c_void, idx: CFIndex) -> *const c_void;
    static kCFTypeDictionaryKeyCallBacks: CFCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFCallBacks;
    static kCFTypeArrayCallBacks: CFCallBacks;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathApply(
        path: *const c_void,
        info: *mut c_void,
        function: extern "C" fn(*mut c_void, *const CGPathElement),
    );
    fn CGPathGetBoundingBox(path: *const c_void) -> CGRect;
}

#[link(name = "CoreText", kind = "framework")]
extern "C" {
    fn CTFontCreateWithName(
        name: *const c_void,
        size: CGFloat,
        matrix: *const c_void,
    ) -> *const c_void;
    fn CTFontCreateWithFontDescriptor(
        descriptor: *const c_void,
        size: CGFloat,
        matrix: *const c_void,
    ) -> *const c_void;
    fn CTFontCreateCopyWithAttributes(
        font: *const c_void,
        size: CGFloat,
        matrix: *const c_void,
        attributes: *const c_void,
    ) -> *const c_void;
    fn CTFontDescriptorCreateWithAttributes(attributes: *const c_void) -> *const c_void;
    fn CTFontManagerCreateFontDescriptorsFromURL(file_url: *const c_void) -> *const c_void;
    fn CTFontCreateUIFontForLanguage(
        ui_type: u32,
        size: CGFloat,
        language: *const c_void,
    ) -> *const c_void;
    fn CTFontCopyDefaultCascadeListForLanguages(
        font: *const c_void,
        language_pref_list: *const c_void,
    ) -> *const c_void;
    fn CTFontCreatePathForGlyph(
        font: *const c_void,
        glyph: u16,
        matrix: *const c_void,
    ) -> *const c_void;
    fn CTFontGetGlyphCount(font: *const c_void) -> CFIndex;
    fn CTFontGetGlyphsForCharacters(
        font: *const c_void,
        characters: *const u16,
        glyphs: *mut u16,
        count: CFIndex,
    ) -> bool;
    static kCTFontVariationAttribute: *const c_void;
}

unsafe fn cf_string(s: &str) -> *const c_void {
    CFStringCreateWithBytes(
        std::ptr::null(),
        s.as_ptr(),
        s.len() as CFIndex,
        KCF_STRING_ENCODING_UTF8,
        false,
    )
}

// ─── Single-face sfnt extraction ───

/// Rebuild face `index` of an sfnt/`ttcf` blob as a standalone single-face
/// sfnt (header + table directory with rewritten offsets + table data).
/// Table checksums are copied verbatim; CoreText does not verify them.
/// Non-collection data is passed through (copied) as-is.
fn extract_single_face_sfnt(data: &[u8], index: u32) -> Option<Vec<u8>> {
    fn be32(data: &[u8], offset: usize) -> Option<u32> {
        data.get(offset..offset + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn be16(data: &[u8], offset: usize) -> Option<u16> {
        data.get(offset..offset + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    if data.get(..4) != Some(b"ttcf") {
        return if index == 0 { Some(data.to_vec()) } else { None };
    }

    let num_fonts = be32(data, 8)?;
    if index >= num_fonts {
        return None;
    }
    let face_offset = be32(data, 12 + 4 * index as usize)? as usize;

    let num_tables = be16(data, face_offset + 4)? as usize;
    let dir_len = 12 + 16 * num_tables;
    let header = data.get(face_offset..face_offset + dir_len)?;

    let mut out = Vec::with_capacity(dir_len + 1024);
    out.extend_from_slice(header);

    for i in 0..num_tables {
        let record = face_offset + 12 + 16 * i;
        let table_offset = be32(data, record + 8)? as usize;
        let table_len = be32(data, record + 12)? as usize;
        let table = data.get(table_offset..table_offset.checked_add(table_len)?)?;

        // 4-byte align each table, then point its directory entry at the new
        // location within the standalone blob.
        while out.len() % 4 != 0 {
            out.push(0);
        }
        let new_offset = out.len() as u32;
        out[12 + 16 * i + 8..12 + 16 * i + 12].copy_from_slice(&new_offset.to_be_bytes());
        out.extend_from_slice(table);
    }
    Some(out)
}

// ─── Face identity reference (for validating a CTFont candidate) ───

/// What a candidate CTFont must agree with to be usable as an outline
/// source: the same glyph count and the same char→glyph mapping as the
/// ttf_parser view of the face. Catches CoreText silently substituting a
/// fallback font for hidden/unknown names.
struct FaceReference {
    glyph_count: CFIndex,
    postscript_name: Option<String>,
    // (BMP code unit, glyph id) probes sampled from the face's cmap.
    cmap_probes: Vec<(u16, u16)>,
}

impl FaceReference {
    fn from_face(face: &ttf_parser::Face<'_>) -> Self {
        let mut postscript_name = None;
        for name in face.names() {
            if name.name_id == ttf_parser::name_id::POST_SCRIPT_NAME {
                if let Some(name) = name.to_string() {
                    postscript_name = Some(name);
                    break;
                }
            }
        }

        // Sample a few mapped BMP characters spread across scripts. Scanning
        // ranges is cheap (cmap lookups) and stops as soon as enough probes
        // are collected.
        let mut cmap_probes = Vec::new();
        'outer: for range in [0x0020..0x0250u32, 0x4E00..0x9FFFu32, 0x3040..0x30FFu32] {
            for code_point in range {
                if cmap_probes.len() >= 4 {
                    break 'outer;
                }
                let Some(ch) = char::from_u32(code_point) else {
                    continue;
                };
                if let Some(glyph) = face.glyph_index(ch) {
                    cmap_probes.push((code_point as u16, glyph.0));
                }
            }
        }

        Self {
            glyph_count: face.number_of_glyphs() as CFIndex,
            postscript_name,
            cmap_probes,
        }
    }

    /// Does this CTFont look like the exact same face?
    unsafe fn matches(&self, ct_font: *const c_void) -> bool {
        if ct_font.is_null() {
            return false;
        }
        if CTFontGetGlyphCount(ct_font) != self.glyph_count {
            return false;
        }
        for &(code_unit, expected_glyph) in &self.cmap_probes {
            let mut glyph: u16 = 0;
            if !CTFontGetGlyphsForCharacters(ct_font, &code_unit, &mut glyph, 1) {
                return false;
            }
            if glyph != expected_glyph {
                return false;
            }
        }
        true
    }
}

// ─── CoreText face wrapper ───

/// A `CTFont` resolved to the same face ttf_parser parsed, used purely as an
/// outline source for glyphs ttf_parser cannot read.
pub(super) struct CoreTextFace {
    ct_font: *const c_void,
    /// Standalone sfnt written to the temp dir when the font could only be
    /// loaded by URL; kept for the CTFont's lifetime, removed on drop.
    temp_sfnt_path: Option<PathBuf>,
}

impl Drop for CoreTextFace {
    fn drop(&mut self) {
        unsafe {
            if !self.ct_font.is_null() {
                CFRelease(self.ct_font);
            }
        }
        if let Some(path) = self.temp_sfnt_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl CoreTextFace {
    /// Resolve a CoreText font over `data[index]`, sized at `units_per_em`
    /// so glyph paths come back in font units. `variations` (e.g. the `wght`
    /// axis for bold) are applied through a font descriptor, keeping outline
    /// weights consistent with the HVAR-adjusted metrics ttf_parser /
    /// rustybuzz produce for the same variations.
    pub(super) fn new(
        data: &[u8],
        index: u32,
        units_per_em: f32,
        variations: &[rustybuzz::Variation],
    ) -> Option<Self> {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let reference = FaceReference::from_face(&face);

        let mut ct_font = std::ptr::null();
        let mut temp_sfnt_path = None;

        // Route 1: resolve by PostScript name (non-hidden fonts).
        if let Some(ps_name) = &reference.postscript_name {
            unsafe {
                let candidate = create_ct_font_by_postscript_name(ps_name, units_per_em);
                if reference.matches(candidate) {
                    ct_font = candidate;
                } else if !candidate.is_null() {
                    CFRelease(candidate);
                }
            }
        }

        // Route 2: hidden system UI fonts (dot-prefixed names, which
        // CTFontCreateWithName silently substitutes) are reachable through
        // the system font's default cascade lists.
        if ct_font.is_null() {
            unsafe {
                ct_font = create_ct_font_via_system_cascade(&reference, units_per_em);
            }
        }

        // Route 3: write the face out as a standalone sfnt and load it by
        // URL through the font manager.
        if ct_font.is_null() {
            unsafe {
                if let Some((candidate, path)) =
                    create_ct_font_from_temp_sfnt(data, index, units_per_em)
                {
                    if reference.matches(candidate) {
                        ct_font = candidate;
                        temp_sfnt_path = Some(path);
                    } else {
                        if !candidate.is_null() {
                            CFRelease(candidate);
                        }
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }

        if ct_font.is_null() {
            return None;
        }

        if !variations.is_empty() {
            unsafe {
                if let Some(descriptor) = variation_descriptor(variations) {
                    let varied = CTFontCreateCopyWithAttributes(
                        ct_font,
                        units_per_em as CGFloat,
                        std::ptr::null(),
                        descriptor,
                    );
                    CFRelease(descriptor);
                    if !varied.is_null() {
                        CFRelease(ct_font);
                        ct_font = varied;
                    }
                }
            }
        }

        Some(Self {
            ct_font,
            temp_sfnt_path,
        })
    }

    /// Extract a glyph outline via CoreText. Returns `None` for empty glyphs
    /// (spaces) and glyphs CoreText cannot path.
    pub(super) fn glyph_outline(&self, glyph_id: u16, units_per_em: f32) -> Option<GlyphOutline> {
        unsafe {
            let path = CTFontCreatePathForGlyph(self.ct_font, glyph_id, std::ptr::null());
            if path.is_null() {
                return None;
            }

            let mut builder = Builder::new();
            extern "C" fn apply(info: *mut c_void, element: *const CGPathElement) {
                let builder = unsafe { &mut *(info as *mut Builder) };
                let element = unsafe { &*element };
                let pt = |i: usize| unsafe { *element.points.add(i) };
                match element.kind {
                    0 => builder.move_to(pt(0).x as f32, pt(0).y as f32),
                    1 => builder.line_to(pt(0).x as f32, pt(0).y as f32),
                    2 => builder.quad_to(
                        pt(0).x as f32,
                        pt(0).y as f32,
                        pt(1).x as f32,
                        pt(1).y as f32,
                    ),
                    3 => builder.curve_to(
                        pt(0).x as f32,
                        pt(0).y as f32,
                        pt(1).x as f32,
                        pt(1).y as f32,
                        pt(2).x as f32,
                        pt(2).y as f32,
                    ),
                    4 => builder.close(),
                    _ => {}
                }
            }
            CGPathApply(path, &mut builder as *mut Builder as *mut c_void, apply);

            let bbox = CGPathGetBoundingBox(path);
            CFRelease(path);

            let min = Point::new(bbox.origin.x as f32, bbox.origin.y as f32);
            let max = Point::new(
                (bbox.origin.x + bbox.size.width) as f32,
                (bbox.origin.y + bbox.size.height) as f32,
            );
            Some(builder.finish(Rect::new(min, max - min), units_per_em))
        }
    }
}

unsafe fn create_ct_font_by_postscript_name(ps_name: &str, units_per_em: f32) -> *const c_void {
    let cf_name = cf_string(ps_name);
    if cf_name.is_null() {
        return std::ptr::null();
    }
    let ct_font = CTFontCreateWithName(cf_name, units_per_em as CGFloat, std::ptr::null());
    CFRelease(cf_name);
    ct_font
}

/// Scan the system UI font's default cascade lists for CJK languages for a
/// font matching `reference`. This is how hidden system fonts (e.g.
/// `.PingFangUITextSC-Default`) are reachable through public API.
unsafe fn create_ct_font_via_system_cascade(
    reference: &FaceReference,
    units_per_em: f32,
) -> *const c_void {
    for lang in ["zh-Hans", "zh-Hant", "ja", "ko"] {
        let cf_lang = cf_string(lang);
        if cf_lang.is_null() {
            continue;
        }
        let system_font = CTFontCreateUIFontForLanguage(
            KCT_FONT_UI_FONT_SYSTEM,
            units_per_em as CGFloat,
            cf_lang,
        );
        if system_font.is_null() {
            CFRelease(cf_lang);
            continue;
        }
        // The system font itself might be the face we want.
        if reference.matches(system_font) {
            CFRelease(cf_lang);
            return system_font;
        }

        let langs = CFArrayCreate(
            std::ptr::null(),
            &cf_lang as *const *const c_void,
            1,
            &kCFTypeArrayCallBacks,
        );
        let cascade = CTFontCopyDefaultCascadeListForLanguages(system_font, langs);
        if !langs.is_null() {
            CFRelease(langs);
        }
        CFRelease(system_font);
        CFRelease(cf_lang);
        if cascade.is_null() {
            continue;
        }

        let count = CFArrayGetCount(cascade);
        for i in 0..count {
            let descriptor = CFArrayGetValueAtIndex(cascade, i);
            let candidate = CTFontCreateWithFontDescriptor(
                descriptor,
                units_per_em as CGFloat,
                std::ptr::null(),
            );
            if reference.matches(candidate) {
                CFRelease(cascade);
                return candidate;
            }
            if !candidate.is_null() {
                CFRelease(candidate);
            }
        }
        CFRelease(cascade);
    }
    std::ptr::null()
}

unsafe fn create_ct_font_from_temp_sfnt(
    data: &[u8],
    index: u32,
    units_per_em: f32,
) -> Option<(*const c_void, PathBuf)> {
    use std::hash::{Hash, Hasher};

    let sfnt = extract_single_face_sfnt(data, index)?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sfnt.len().hash(&mut hasher);
    sfnt.get(..4096).hash(&mut hasher);
    index.hash(&mut hasher);
    let path = std::env::temp_dir().join(format!(
        "makepad-coretext-fallback-{:016x}.otf",
        hasher.finish()
    ));
    if std::fs::metadata(&path).map(|m| m.len() as usize).ok() != Some(sfnt.len()) {
        std::fs::write(&path, &sfnt).ok()?;
    }

    let path_bytes = path.as_os_str().as_encoded_bytes();
    let url = CFURLCreateFromFileSystemRepresentation(
        std::ptr::null(),
        path_bytes.as_ptr(),
        path_bytes.len() as CFIndex,
        false,
    );
    if url.is_null() {
        return None;
    }
    let descriptors = CTFontManagerCreateFontDescriptorsFromURL(url);
    CFRelease(url);
    if descriptors.is_null() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let mut ct_font = std::ptr::null();
    if CFArrayGetCount(descriptors) > 0 {
        let descriptor = CFArrayGetValueAtIndex(descriptors, 0);
        ct_font =
            CTFontCreateWithFontDescriptor(descriptor, units_per_em as CGFloat, std::ptr::null());
    }
    CFRelease(descriptors);
    if ct_font.is_null() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some((ct_font, path))
}

/// Build a `CTFontDescriptor` carrying `kCTFontVariationAttribute`: a
/// dictionary of axis tag (CFNumber) → axis value (CFNumber).
unsafe fn variation_descriptor(variations: &[rustybuzz::Variation]) -> Option<*const c_void> {
    let mut keys = Vec::with_capacity(variations.len());
    let mut values = Vec::with_capacity(variations.len());
    for variation in variations {
        let tag = variation.tag.0 as i64;
        let value = variation.value as f64;
        let key = CFNumberCreate(
            std::ptr::null(),
            KCF_NUMBER_SINT64_TYPE,
            &tag as *const i64 as *const c_void,
        );
        let val = CFNumberCreate(
            std::ptr::null(),
            KCF_NUMBER_FLOAT64_TYPE,
            &value as *const f64 as *const c_void,
        );
        if key.is_null() || val.is_null() {
            for cf in keys.iter().chain(values.iter()) {
                CFRelease(*cf);
            }
            if !key.is_null() {
                CFRelease(key);
            }
            if !val.is_null() {
                CFRelease(val);
            }
            return None;
        }
        keys.push(key);
        values.push(val);
    }

    let variation_dict = CFDictionaryCreate(
        std::ptr::null(),
        keys.as_ptr(),
        values.as_ptr(),
        keys.len() as CFIndex,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks,
    );
    for cf in keys.iter().chain(values.iter()) {
        CFRelease(*cf);
    }
    if variation_dict.is_null() {
        return None;
    }

    let attr_keys = [kCTFontVariationAttribute];
    let attr_values = [variation_dict];
    let attributes = CFDictionaryCreate(
        std::ptr::null(),
        attr_keys.as_ptr() as *const *const c_void,
        attr_values.as_ptr(),
        1,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks,
    );
    CFRelease(variation_dict);
    if attributes.is_null() {
        return None;
    }

    let descriptor = CTFontDescriptorCreateWithAttributes(attributes);
    CFRelease(attributes);
    if descriptor.is_null() {
        return None;
    }
    Some(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) const PINGFANG: &str = "/System/Library/PrivateFrameworks/FontServices.framework/Resources/Reserved/PingFangUI.ttc";

    #[test]
    fn coretext_outlines_hvgl_pingfang() {
        let Ok(data) = std::fs::read(PINGFANG) else {
            eprintln!("PingFangUI.ttc not present, skipping");
            return;
        };
        let face = ttf_parser::Face::parse(&data, 0).expect("face 0 should parse");
        let upem = face.units_per_em() as f32;
        let glyph = face.glyph_index('性').expect("性 should map").0;

        let ct =
            CoreTextFace::new(&data, 0, upem, &[]).expect("CoreText should load PingFang face 0");
        let outline = ct
            .glyph_outline(glyph, upem)
            .expect("CoreText should outline an hvgl glyph");
        assert!(
            !outline.commands().is_empty(),
            "outline should carry path commands"
        );
        let bounds = outline.bounds_in_ems();
        assert!(
            bounds.size.width > 0.1 && bounds.size.height > 0.1,
            "CJK glyph bounds should be roughly em-sized, got {bounds:?}"
        );
    }

    #[test]
    fn coretext_weight_variation_changes_outline() {
        let Ok(data) = std::fs::read(PINGFANG) else {
            eprintln!("PingFangUI.ttc not present, skipping");
            return;
        };
        let face = ttf_parser::Face::parse(&data, 0).expect("face 0 should parse");
        let upem = face.units_per_em() as f32;
        let glyph = face.glyph_index('性').expect("性 should map").0;

        let regular = CoreTextFace::new(&data, 0, upem, &[]).unwrap();
        let bold_variation = [rustybuzz::Variation {
            tag: ttf_parser::Tag::from_bytes(b"wght"),
            value: 700.0,
        }];
        let bold = CoreTextFace::new(&data, 0, upem, &bold_variation).unwrap();

        let fmt = |o: &GlyphOutline| format!("{:?}", o.commands());
        let regular_outline = regular.glyph_outline(glyph, upem).unwrap();
        let bold_outline = bold.glyph_outline(glyph, upem).unwrap();
        assert_ne!(
            fmt(&regular_outline),
            fmt(&bold_outline),
            "wght 700 should change outline geometry"
        );
    }

    #[test]
    fn single_face_font_passes_through() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../widgets/resources/IBMPlexSans-Text.ttf"
        );
        let data = std::fs::read(path).expect("bundled font should exist");
        let extracted = extract_single_face_sfnt(&data, 0).expect("pass-through should work");
        assert_eq!(extracted, data);
        assert!(extract_single_face_sfnt(&data, 1).is_none());
    }
}
