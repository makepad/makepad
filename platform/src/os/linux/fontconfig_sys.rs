//! Resolve the platform's default fonts at runtime via fontconfig.
//!
//! fontconfig is loaded dynamically via dlopen (consistent with makepad's
//! GStreamer / EGL style); if `libfontconfig.so.1` is missing we degrade to
//! `None` and the caller renders no glyphs for that member. We never bundle
//! text fonts — glyph coverage is delegated to whatever the OS has installed.

use super::module_loader::ModuleLoader;
use crate::cx_api::{SystemFontQuery, SystemFontResult, SystemFontRole};
use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int, c_uchar};

pub type FcConfig = c_void;
pub type FcPattern = c_void;
pub type FcCharSet = c_void;

// FcMatchKind
const FC_MATCH_PATTERN: c_int = 0;
// FcResult
const FC_RESULT_MATCH: c_int = 0;

// Object names (fontconfig property keys)
const FC_FAMILY: &[u8] = b"family\0";
const FC_WEIGHT: &[u8] = b"weight\0";
const FC_SLANT: &[u8] = b"slant\0";
const FC_LANG: &[u8] = b"lang\0";
const FC_FILE: &[u8] = b"file\0";
const FC_INDEX: &[u8] = b"index\0";
const FC_SPACING: &[u8] = b"spacing\0";
const FC_CHARSET: &[u8] = b"charset\0";

// FC_SLANT values
const FC_SLANT_ROMAN: c_int = 0;
const FC_SLANT_ITALIC: c_int = 100;

// FC_WEIGHT values
const FC_WEIGHT_REGULAR: c_int = 80;
const FC_WEIGHT_BOLD: c_int = 200;

// FC_SPACING values
const FC_MONO: c_int = 100;

struct LibFontconfig {
    _lib: ModuleLoader,
    config: *mut FcConfig,

    fc_pattern_create: unsafe extern "C" fn() -> *mut FcPattern,
    fc_pattern_destroy: unsafe extern "C" fn(*mut FcPattern),
    fc_pattern_add_string:
        unsafe extern "C" fn(*mut FcPattern, *const c_char, *const c_uchar) -> c_int,
    fc_pattern_add_integer: unsafe extern "C" fn(*mut FcPattern, *const c_char, c_int) -> c_int,
    fc_pattern_get_string:
        unsafe extern "C" fn(*const FcPattern, *const c_char, c_int, *mut *mut c_uchar) -> c_int,
    // Reads the face index for a matched `.ttc` collection. Optional: if missing
    // (very old fontconfig) we default the index to 0.
    fc_pattern_get_integer:
        Option<unsafe extern "C" fn(*const FcPattern, *const c_char, c_int, *mut c_int) -> c_int>,
    fc_config_substitute:
        unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, c_int) -> c_int,
    fc_default_substitute: unsafe extern "C" fn(*mut FcPattern),
    fc_font_match:
        unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, *mut c_int) -> *mut FcPattern,

    // Charset functions, used for per-glyph fallback (when `query.sample` is
    // set). Optional: if any is missing we just skip the charset hint and fall
    // back to family/lang matching.
    fc_charset_create: Option<unsafe extern "C" fn() -> *mut FcCharSet>,
    fc_charset_add_char: Option<unsafe extern "C" fn(*mut FcCharSet, u32) -> c_int>,
    fc_charset_destroy: Option<unsafe extern "C" fn(*mut FcCharSet)>,
    fc_pattern_add_charset:
        Option<unsafe extern "C" fn(*mut FcPattern, *const c_char, *const FcCharSet) -> c_int>,
}

impl LibFontconfig {
    unsafe fn try_load() -> Option<Self> {
        let lib = ModuleLoader::load("libfontconfig.so.1")
            .or_else(|_| ModuleLoader::load("libfontconfig.so"))
            .ok()?;
        let fc_init_load_config_and_fonts: unsafe extern "C" fn() -> *mut FcConfig =
            lib.get_symbol("FcInitLoadConfigAndFonts").ok()?;
        let config = fc_init_load_config_and_fonts();
        if config.is_null() {
            return None;
        }
        Some(LibFontconfig {
            fc_pattern_create: lib.get_symbol("FcPatternCreate").ok()?,
            fc_pattern_destroy: lib.get_symbol("FcPatternDestroy").ok()?,
            fc_pattern_add_string: lib.get_symbol("FcPatternAddString").ok()?,
            fc_pattern_add_integer: lib.get_symbol("FcPatternAddInteger").ok()?,
            fc_pattern_get_string: lib.get_symbol("FcPatternGetString").ok()?,
            fc_pattern_get_integer: lib.get_symbol("FcPatternGetInteger").ok(),
            fc_config_substitute: lib.get_symbol("FcConfigSubstitute").ok()?,
            fc_default_substitute: lib.get_symbol("FcDefaultSubstitute").ok()?,
            fc_font_match: lib.get_symbol("FcFontMatch").ok()?,
            fc_charset_create: lib.get_symbol("FcCharSetCreate").ok(),
            fc_charset_add_char: lib.get_symbol("FcCharSetAddChar").ok(),
            fc_charset_destroy: lib.get_symbol("FcCharSetDestroy").ok(),
            fc_pattern_add_charset: lib.get_symbol("FcPatternAddCharSet").ok(),
            _lib: lib,
            config,
        })
    }
}

thread_local! {
    static FONTCONFIG: RefCell<Option<Option<LibFontconfig>>> = const { RefCell::new(None) };
}

/// Resolve `query` to font bytes + face index using fontconfig, or `None` on
/// failure. The index matters for `.ttc` collections (fontconfig's `FC_INDEX`).
pub fn load_system_font(query: &SystemFontQuery) -> Option<SystemFontResult> {
    FONTCONFIG.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(unsafe { LibFontconfig::try_load() });
        }
        let fc = slot.as_ref().unwrap().as_ref()?;
        unsafe { load_system_font_inner(fc, query) }
    })
}

unsafe fn load_system_font_inner(
    fc: &LibFontconfig,
    query: &SystemFontQuery,
) -> Option<SystemFontResult> {
    let pat = (fc.fc_pattern_create)();
    if pat.is_null() {
        return None;
    }

    // Map our role to a generic fontconfig family + language hint. fontconfig
    // resolves the generic alias ("sans-serif" etc.) to the user's configured
    // default, and the lang/charset to a font that covers that script.
    let (family, lang): (&str, Option<&str>) = match query.role {
        SystemFontRole::Ui => ("sans-serif", None),
        SystemFontRole::Serif => ("serif", None),
        SystemFontRole::Mono => ("monospace", None),
        SystemFontRole::Cjk => (
            "sans-serif",
            Some(match query.lang.as_str() {
                "ja" => "ja",
                "ko" => "ko",
                _ => "zh",
            }),
        ),
        // "emoji" is a recognized generic family in modern fontconfig.
        SystemFontRole::Emoji => ("emoji", None),
    };

    let family_c = CString::new(family).ok();
    if let Some(family_c) = &family_c {
        (fc.fc_pattern_add_string)(
            pat,
            FC_FAMILY.as_ptr() as *const c_char,
            family_c.as_ptr() as *const c_uchar,
        );
    }
    if let Some(lang) = lang {
        if let Ok(lang_c) = CString::new(lang) {
            (fc.fc_pattern_add_string)(
                pat,
                FC_LANG.as_ptr() as *const c_char,
                lang_c.as_ptr() as *const c_uchar,
            );
        }
    }

    // Per-glyph fallback: if the caller passed the actual uncovered characters,
    // require the matched font to cover them via a charset. fontconfig then
    // picks whatever installed font covers the script, which is exactly the
    // browser/Flutter behaviour. Built and destroyed within this scope; once
    // added to the pattern fontconfig holds its own reference.
    if !query.sample.is_empty() {
        if let (Some(create), Some(add_char), Some(destroy), Some(add_charset)) = (
            fc.fc_charset_create,
            fc.fc_charset_add_char,
            fc.fc_charset_destroy,
            fc.fc_pattern_add_charset,
        ) {
            let charset = create();
            if !charset.is_null() {
                for ch in query.sample.chars() {
                    add_char(charset, ch as u32);
                }
                add_charset(pat, FC_CHARSET.as_ptr() as *const c_char, charset);
                destroy(charset);
            }
        }
    }

    let weight = if query.weight >= 600 {
        FC_WEIGHT_BOLD
    } else {
        FC_WEIGHT_REGULAR
    };
    (fc.fc_pattern_add_integer)(pat, FC_WEIGHT.as_ptr() as *const c_char, weight);

    let slant = if query.italic {
        FC_SLANT_ITALIC
    } else {
        FC_SLANT_ROMAN
    };
    (fc.fc_pattern_add_integer)(pat, FC_SLANT.as_ptr() as *const c_char, slant);

    if matches!(query.role, SystemFontRole::Mono) {
        (fc.fc_pattern_add_integer)(pat, FC_SPACING.as_ptr() as *const c_char, FC_MONO);
    }

    // Standard fontconfig matching dance.
    (fc.fc_config_substitute)(fc.config, pat, FC_MATCH_PATTERN);
    (fc.fc_default_substitute)(pat);

    let mut result: c_int = 0;
    let matched = (fc.fc_font_match)(fc.config, pat, &mut result);
    (fc.fc_pattern_destroy)(pat);

    if matched.is_null() || result != FC_RESULT_MATCH {
        if !matched.is_null() {
            (fc.fc_pattern_destroy)(matched);
        }
        return None;
    }

    let mut file_ptr: *mut c_uchar = std::ptr::null_mut();
    let got = (fc.fc_pattern_get_string)(
        matched,
        FC_FILE.as_ptr() as *const c_char,
        0,
        &mut file_ptr,
    );
    let path = if got == FC_RESULT_MATCH && !file_ptr.is_null() {
        cstr_to_string(file_ptr as *const c_char)
    } else {
        None
    };

    // Face index within a `.ttc` collection (fontconfig's `FC_INDEX`). Absent
    // symbol / non-match → 0 (the common single-face case).
    let index = fc
        .fc_pattern_get_integer
        .and_then(|get_int| {
            let mut idx: c_int = 0;
            let got = get_int(matched, FC_INDEX.as_ptr() as *const c_char, 0, &mut idx);
            (got == FC_RESULT_MATCH && idx >= 0).then_some(idx as u32)
        })
        .unwrap_or(0);
    (fc.fc_pattern_destroy)(matched);

    let path = path?;
    if path.is_empty() {
        return None;
    }
    // mmap the font file (falling back to a read) so unused pages of a large
    // font — e.g. the sbix color-bitmap table of a color emoji font, or
    // uncovered CJK regions — are never made resident.
    let bytes = crate::shared_bytes::SharedBytes::from_file_mmap_or_read(&path).ok()?;
    Some(SystemFontResult { bytes, index })
}

unsafe fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    std::ffi::CStr::from_ptr(ptr)
        .to_str()
        .ok()
        .map(|s| s.to_string())
}
