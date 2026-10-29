//! The operating system's own fonts as the last link of a font chain.
//!
//! When every face a text style names misses a character (Devanagari, Arabic,
//! Thai, Hangul, ... in a UI themed with a Latin face), the text stack asks
//! [`fallback_candidates`] for the installed fonts the OS itself would draw
//! those characters with, and loads the first that really covers them. Nothing
//! is bundled and nothing is read at startup: the OS is asked on the first miss
//! of a script, once.
//!
//! - macOS / iOS: CoreText's own cascade (`CTFontCreateForString` on the system
//!   UI font), the font file found through its URL, the face of a collection by
//!   PostScript name.
//! - Linux / BSD: an installed-font scan, with optional fontconfig (opened at
//!   runtime) only when the scan finds no face covering the text.
//! - Windows: the fonts Windows' own fallback uses per script, in the system
//!   font folder, followed by scanned installed fonts.
//! - Android: the system font folder's Noto faces for the script, followed
//!   by scanned installed fonts.
//!
//! The caller checks a candidate's cmap before using it: a candidate is a
//! suggestion, never trusted to cover the text.

use std::path::PathBuf;

#[cfg(not(target_vendor = "apple"))]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[path = "font_scan.rs"]
mod font_scan;

pub use unicode_script::{Script, UnicodeScript};

/// One face of an installed font file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SystemFont {
    pub path: PathBuf,
    /// The face index inside a collection (`.ttc`); 0 for a single font.
    pub index: u32,
}

/// The script a character is grouped under for fallback: one OS query per
/// group. Characters of no script of their own (`Common`, `Inherited`) and
/// the Japanese kana are folded so they travel with what they usually
/// accompany.
pub fn fallback_script(ch: char) -> Script {
    match ch.script() {
        Script::Hiragana => Script::Katakana,
        script => script,
    }
}

/// Installed fonts that may draw `text` (every character of one
/// [`fallback_script`]), best first. `bold` asks for a heavy face where the
/// OS has one. Empty when the platform has none or no font discovery.
pub fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
    if text.is_empty() {
        return Vec::new();
    }
    imp::fallback_candidates(text, bold)
}

/// The family name that asks for the OS's own UI face (San Francisco, Segoe
/// UI, Roboto, the desktop's sans-serif), whatever it is called there.
pub const SYSTEM_UI_FAMILY: &str = "system-ui";

/// The installed face of family `name` (case-insensitive; [`SYSTEM_UI_FAMILY`]
/// for the OS's UI face) closest to `weight` (CSS 100..=900, 400 regular)
/// and `italic`. Nothing is read before the first call; a platform without
/// font discovery (wasm) answers None.
pub fn find_family(name: &str, weight: u16, italic: bool) -> Option<SystemFont> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    imp::find_family(name, weight.clamp(1, 1000), italic)
}

/// Whether this platform discovers fonts through an index of its font
/// folders that [`prewarm`] builds (Linux, Windows, Android); CoreText and
/// the web need none.
pub fn needs_prewarm() -> bool {
    cfg!(all(not(target_vendor = "apple"), not(target_arch = "wasm32")))
}

/// Builds the installed-font index (names, styles and every face's
/// character coverage) on the calling thread. Called from a background
/// thread after the first frame, so the first by-name lookup or OS fallback
/// does not wait for the scan.
pub fn prewarm() {
    #[cfg(all(not(target_vendor = "apple"), not(target_arch = "wasm32")))]
    font_scan::prewarm();
}

/// Whether a lookup now answers without scanning the font folders: false
/// until [`prewarm`] has run once, so the UI thread can come back later
/// instead of waiting for the scan.
pub fn index_ready() -> bool {
    // Windows and Android answer from their own per-script table first and
    // add scanned faces only once the index is there.
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    return font_scan::prewarmed();
    #[cfg(not(all(unix, not(target_vendor = "apple"), not(target_os = "android"))))]
    return true;
}

/// The face of `data` (a font or a collection) named `postscript_name`.
#[allow(dead_code)]
fn collection_index_of(data: &[u8], postscript_name: &str) -> Option<u32> {
    let count = rustybuzz::ttf_parser::fonts_in_collection(data).unwrap_or(1);
    (0..count).find(|&index| {
        rustybuzz::ttf_parser::Face::parse(data, index).ok().is_some_and(|face| {
            face.names().into_iter().any(|name| {
                // Apple's own collections name their faces in Mac Roman
                // records only, which ttf-parser does not decode: ASCII bytes.
                name.name_id == rustybuzz::ttf_parser::name_id::POST_SCRIPT_NAME
                    && (name.to_string().is_some_and(|n| n == postscript_name)
                        || (name.platform_id == rustybuzz::ttf_parser::PlatformId::Macintosh
                            && name.name == postscript_name.as_bytes()))
            })
        })
    })
}

#[cfg(target_vendor = "apple")]
mod imp {
    //! CoreText: installed families and the OS's own fallback fonts.
    use super::{SystemFont, SYSTEM_UI_FAMILY};
    use std::ffi::c_void;

    type CFIndex = isize;
    type CFStringRef = *const c_void;

    // Opaque callback tables, as in coretext.rs. Only raw addresses are used.
    #[repr(C)]
    struct CFCallBacks {
        _priv: [usize; 7],
    }

    #[repr(C)]
    struct CFRange {
        location: CFIndex,
        length: CFIndex,
    }

    const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const KCF_NUMBER_SINT32_TYPE: CFIndex = 3;
    const KCF_NUMBER_SINT64_TYPE: CFIndex = 4;
    const KCF_NUMBER_FLOAT64_TYPE: CFIndex = 6;
    const KCF_URL_POSIX_PATH_STYLE: CFIndex = 0;
    const KCT_FONT_UI_FONT_SYSTEM: u32 = 2;
    const KCT_FONT_UI_FONT_EMPHASIZED_SYSTEM: u32 = 3;
    const KCT_FONT_ITALIC_TRAIT: i32 = 1 << 0;

    const WEIGHTS: [(f64, f64); 9] = [
        (100.0, -0.8),
        (200.0, -0.6),
        (300.0, -0.4),
        (400.0, 0.0),
        (500.0, 0.23),
        (600.0, 0.3),
        (700.0, 0.4),
        (800.0, 0.56),
        (900.0, 0.62),
    ];

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            num_bytes: CFIndex,
            encoding: u32,
            is_external_representation: bool,
        ) -> *const c_void;
        fn CFStringGetLength(string: *const c_void) -> CFIndex;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut u8,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> bool;
        fn CFURLGetFileSystemRepresentation(
            url: *const c_void,
            resolve_against_base: bool,
            buffer: *mut u8,
            max_buf_len: CFIndex,
        ) -> bool;
        fn CFURLCopyScheme(url: *const c_void) -> CFStringRef;
        fn CFURLCopyFileSystemPath(url: *const c_void, path_style: CFIndex) -> CFStringRef;
        fn CFNumberCreate(
            allocator: *const c_void,
            number_type: CFIndex,
            value: *const c_void,
        ) -> *const c_void;
        fn CFNumberGetValue(
            number: *const c_void,
            number_type: CFIndex,
            value: *mut c_void,
        ) -> u8;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: CFIndex,
            key_callbacks: *const CFCallBacks,
            value_callbacks: *const CFCallBacks,
        ) -> *const c_void;
        fn CFDictionaryGetValue(dictionary: *const c_void, key: *const c_void) -> *const c_void;
        fn CFSetCreate(
            allocator: *const c_void,
            values: *const *const c_void,
            num_values: CFIndex,
            callbacks: *const CFCallBacks,
        ) -> *const c_void;
        fn CFArrayGetCount(array: *const c_void) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: CFIndex) -> *const c_void;
        static kCFTypeDictionaryKeyCallBacks: CFCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFCallBacks;
        static kCFTypeSetCallBacks: CFCallBacks;
    }

    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        fn CTFontCreateUIFontForLanguage(
            ui_type: u32,
            size: f64,
            language: *const c_void,
        ) -> *const c_void;
        fn CTFontCreateForString(
            current_font: *const c_void,
            string: *const c_void,
            range: CFRange,
        ) -> *const c_void;
        fn CTFontCopyAttribute(font: *const c_void, attribute: *const c_void) -> *const c_void;
        fn CTFontCopyPostScriptName(font: *const c_void) -> *const c_void;
        fn CTFontDescriptorCreateWithAttributes(attributes: *const c_void) -> *const c_void;
        fn CTFontDescriptorCreateMatchingFontDescriptors(
            descriptor: *const c_void,
            mandatory_attributes: *const c_void,
        ) -> *const c_void;
        fn CTFontDescriptorCopyAttribute(
            descriptor: *const c_void,
            attribute: CFStringRef,
        ) -> *const c_void;
        static kCTFontURLAttribute: *const c_void;
        static kCTFontFamilyNameAttribute: CFStringRef;
        static kCTFontNameAttribute: CFStringRef;
        static kCTFontTraitsAttribute: CFStringRef;
        static kCTFontWeightTrait: CFStringRef;
        static kCTFontSymbolicTrait: CFStringRef;
    }

    /// Own one Create/Copy reference, including on early returns.
    struct OwnedCf(*const c_void);

    impl OwnedCf {
        unsafe fn new(value: *const c_void) -> Option<Self> {
            if value.is_null() { None } else { Some(Self(value)) }
        }
    }

    impl Drop for OwnedCf {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }

    unsafe fn cf_string(value: &str) -> CFStringRef {
        CFStringCreateWithBytes(
            std::ptr::null(),
            value.as_ptr(),
            value.len() as CFIndex,
            KCF_STRING_ENCODING_UTF8,
            false,
        )
    }

    unsafe fn cf_number_f64(value: f64) -> *const c_void {
        CFNumberCreate(std::ptr::null(), KCF_NUMBER_FLOAT64_TYPE, (&value as *const f64).cast())
    }

    unsafe fn cf_number_i32(value: i32) -> *const c_void {
        CFNumberCreate(std::ptr::null(), KCF_NUMBER_SINT32_TYPE, (&value as *const i32).cast())
    }

    unsafe fn cf_dict<const N: usize>(entries: [(CFStringRef, *const c_void); N]) -> *const c_void {
        let keys = entries.map(|(key, _)| key);
        let values = entries.map(|(_, value)| value);
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            N as CFIndex,
            std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
            std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
        )
    }

    unsafe fn cf_set(values: &[CFStringRef]) -> *const c_void {
        CFSetCreate(
            std::ptr::null(),
            values.as_ptr(),
            values.len() as CFIndex,
            std::ptr::addr_of!(kCFTypeSetCallBacks),
        )
    }

    unsafe fn cf_to_string(string: *const c_void) -> Option<String> {
        if string.is_null() {
            return None;
        }
        // Each UTF-16 code unit needs at most three bytes in UTF-8.
        let capacity = usize::try_from(CFStringGetLength(string)).ok()?.checked_mul(3)?.checked_add(1)?;
        let mut buffer = vec![0u8; capacity];
        let ok = CFStringGetCString(
            string,
            buffer.as_mut_ptr(),
            CFIndex::try_from(buffer.len()).ok()?,
            KCF_STRING_ENCODING_UTF8,
        );
        if !ok {
            return None;
        }
        let len = buffer.iter().position(|&b| b == 0)?;
        buffer.truncate(len);
        String::from_utf8(buffer).ok()
    }

    fn css_weight_to_coretext(weight: u16) -> f64 {
        let weight = f64::from(weight.clamp(100, 900));
        for pair in WEIGHTS.windows(2) {
            let [(css0, ct0), (css1, ct1)] = [pair[0], pair[1]];
            if weight <= css1 {
                return ct0 + (ct1 - ct0) * (weight - css0) / (css1 - css0);
            }
        }
        WEIGHTS[8].1
    }

    fn coretext_weight_to_css(weight: f64) -> f64 {
        let weight = weight.clamp(WEIGHTS[0].1, WEIGHTS[8].1);
        for pair in WEIGHTS.windows(2) {
            let [(css0, ct0), (css1, ct1)] = [pair[0], pair[1]];
            if weight <= ct1 {
                return css0 + (css1 - css0) * (weight - ct0) / (ct1 - ct0);
            }
        }
        WEIGHTS[8].0
    }

    unsafe fn descriptor_traits(descriptor: *const c_void) -> Option<(f64, bool)> {
        let traits = OwnedCf::new(CTFontDescriptorCopyAttribute(descriptor, kCTFontTraitsAttribute))?;
        let weight_number = CFDictionaryGetValue(traits.0, kCTFontWeightTrait);
        let symbolic_number = CFDictionaryGetValue(traits.0, kCTFontSymbolicTrait);
        let mut weight = 0.0f64;
        // 64-bit: the class mask in the top bits (sans serif = 8 << 28) does
        // not fit an i32, and a lossy read reports failure.
        let mut symbolic = 0i64;
        if !weight_number.is_null()
            && CFNumberGetValue(weight_number, KCF_NUMBER_FLOAT64_TYPE, (&mut weight as *mut f64).cast()) == 0
        {
            return None;
        }
        if !symbolic_number.is_null()
            && CFNumberGetValue(symbolic_number, KCF_NUMBER_SINT64_TYPE, (&mut symbolic as *mut i64).cast()) == 0
        {
            return None;
        }
        if !weight.is_finite() {
            return None;
        }
        Some((coretext_weight_to_css(weight), symbolic & i64::from(KCT_FONT_ITALIC_TRAIT) != 0))
    }

    unsafe fn cf_url_to_path(url: *const c_void) -> Option<std::path::PathBuf> {
        let scheme = OwnedCf::new(CFURLCopyScheme(url))?;
        if !cf_to_string(scheme.0)?.eq_ignore_ascii_case("file") {
            return None;
        }
        let path = OwnedCf::new(CFURLCopyFileSystemPath(url, KCF_URL_POSIX_PATH_STYLE))?;
        Some(cf_to_string(path.0)?.into())
    }

    /// Single fonts need no file read; collections identify their face by name.
    fn font_from_path(path: std::path::PathBuf, postscript_name: Option<&str>) -> Option<SystemFont> {
        let is_collection = path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
            ext.eq_ignore_ascii_case("ttc") || ext.eq_ignore_ascii_case("otc")
        });
        let index = if is_collection {
            let name = postscript_name?;
            let data = makepad_shared_bytes::SharedBytes::from_file_mmap_or_read(&path).ok()?;
            super::collection_index_of(data.as_slice(), name)?
        } else {
            0
        };
        Some(SystemFont { path, index })
    }

    pub(super) fn find_family(name: &str, weight: u16, italic: bool) -> Option<SystemFont> {
        unsafe {
            if name.eq_ignore_ascii_case(SYSTEM_UI_FAMILY) {
                let ui_type = if weight >= 600 { KCT_FONT_UI_FONT_EMPHASIZED_SYSTEM } else { KCT_FONT_UI_FONT_SYSTEM };
                let font = OwnedCf::new(CTFontCreateUIFontForLanguage(ui_type, 16.0, std::ptr::null()))?;
                let url = OwnedCf::new(CTFontCopyAttribute(font.0, kCTFontURLAttribute))?;
                let name = OwnedCf::new(CTFontCopyPostScriptName(font.0));
                let postscript_name = name.as_ref().and_then(|name| cf_to_string(name.0));
                return font_from_path(cf_url_to_path(url.0)?, postscript_name.as_deref());
            }

            let name = OwnedCf::new(cf_string(name))?;
            let weight_number = OwnedCf::new(cf_number_f64(css_weight_to_coretext(weight)))?;
            let symbolic = OwnedCf::new(cf_number_i32(if italic { KCT_FONT_ITALIC_TRAIT } else { 0 }))?;
            let traits = OwnedCf::new(cf_dict([
                (kCTFontWeightTrait, weight_number.0),
                (kCTFontSymbolicTrait, symbolic.0),
            ]))?;
            for name_attribute in [kCTFontFamilyNameAttribute, kCTFontNameAttribute] {
                let attributes = OwnedCf::new(cf_dict([
                    (name_attribute, name.0),
                    (kCTFontTraitsAttribute, traits.0),
                ]))?;
                let descriptor = OwnedCf::new(CTFontDescriptorCreateWithAttributes(attributes.0))?;
                // Requiring the name prevents CoreText from substituting another family.
                let mandatory = OwnedCf::new(cf_set(&[name_attribute]))?;
                let Some(matches) = OwnedCf::new(CTFontDescriptorCreateMatchingFontDescriptors(descriptor.0, mandatory.0)) else {
                    continue;
                };
                let count = CFArrayGetCount(matches.0);
                if count == 0 {
                    continue;
                }
                let mut best: Option<(f64, std::path::PathBuf, Option<String>)> = None;
                for index in 0..count {
                    let candidate = CFArrayGetValueAtIndex(matches.0, index);
                    let Some((candidate_weight, candidate_italic)) = descriptor_traits(candidate) else {
                        continue;
                    };
                    // An italic mismatch outweighs the entire CSS weight range.
                    let score = (candidate_weight - f64::from(weight)).abs()
                        + if candidate_italic == italic { 0.0 } else { 1000.0 };
                    if best.as_ref().is_some_and(|(best_score, _, _)| score >= *best_score) {
                        continue;
                    }
                    let Some(url) = OwnedCf::new(CTFontDescriptorCopyAttribute(candidate, kCTFontURLAttribute)) else {
                        continue;
                    };
                    let Some(path) = cf_url_to_path(url.0) else {
                        continue;
                    };
                    let name = OwnedCf::new(CTFontDescriptorCopyAttribute(candidate, kCTFontNameAttribute));
                    let postscript_name = name.as_ref().and_then(|name| cf_to_string(name.0));
                    best = Some((score, path, postscript_name));
                }
                return best.and_then(|(_, path, name)| font_from_path(path, name.as_deref()));
            }
        }
        None
    }

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
        unsafe {
            let ui_type = if bold { KCT_FONT_UI_FONT_EMPHASIZED_SYSTEM } else { KCT_FONT_UI_FONT_SYSTEM };
            let base = CTFontCreateUIFontForLanguage(ui_type, 16.0, std::ptr::null());
            if base.is_null() {
                return Vec::new();
            }
            let string = CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as CFIndex,
                KCF_STRING_ENCODING_UTF8,
                false,
            );
            if string.is_null() {
                CFRelease(base);
                return Vec::new();
            }
            let range = CFRange { location: 0, length: CFStringGetLength(string) };
            let font = CTFontCreateForString(base, string, range);
            CFRelease(string);
            CFRelease(base);
            if font.is_null() {
                return Vec::new();
            }
            let postscript_name = {
                let name = CTFontCopyPostScriptName(font);
                let out = cf_to_string(name);
                if !name.is_null() {
                    CFRelease(name);
                }
                out
            };
            let url = CTFontCopyAttribute(font, kCTFontURLAttribute);
            CFRelease(font);
            if url.is_null() {
                return Vec::new();
            }
            let mut buffer = vec![0u8; 4096];
            let ok = CFURLGetFileSystemRepresentation(url, true, buffer.as_mut_ptr(), buffer.len() as CFIndex);
            CFRelease(url);
            if !ok {
                return Vec::new();
            }
            let Some(len) = buffer.iter().position(|&b| b == 0) else {
                return Vec::new();
            };
            buffer.truncate(len);
            let Ok(path) = String::from_utf8(buffer) else {
                return Vec::new();
            };
            // CoreText's answer when nothing covers the text: its tofu font.
            if postscript_name.as_deref() == Some("LastResort") {
                return Vec::new();
            }
            let path = std::path::PathBuf::from(path);
            let index = match &postscript_name {
                Some(name) => std::fs::read(&path)
                    .ok()
                    .and_then(|data| super::collection_index_of(&data, name))
                    .unwrap_or(0),
                None => 0,
            };
            vec![SystemFont { path, index }]
        }
    }
}

#[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
mod imp {
    //! Installed fonts first, then optional fontconfig opened at runtime.
    use super::SystemFont;
    use std::{
        ffi::{c_char, c_int, c_void, CStr},
        sync::{Mutex, OnceLock},
    };

    const RTLD_NOW: c_int = 2;
    const FC_MATCH_PATTERN: c_int = 0;
    const FC_RESULT_MATCH: c_int = 0;
    const FC_WEIGHT_REGULAR: c_int = 80;
    const FC_WEIGHT_BOLD: c_int = 200;

    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    #[repr(C)]
    struct FcFontSet {
        nfont: c_int,
        sfont: c_int,
        fonts: *mut *mut c_void,
    }

    struct Fc {
        config: *mut c_void,
        pattern_create: unsafe extern "C" fn() -> *mut c_void,
        pattern_destroy: unsafe extern "C" fn(*mut c_void),
        pattern_add_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const u8) -> c_int,
        pattern_add_integer: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_int,
        pattern_add_charset: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> c_int,
        pattern_get_string: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *const u8) -> c_int,
        pattern_get_integer: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_int) -> c_int,
        pattern_get_charset: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void) -> c_int,
        charset_create: unsafe extern "C" fn() -> *mut c_void,
        charset_destroy: unsafe extern "C" fn(*mut c_void),
        charset_add_char: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
        charset_has_char: unsafe extern "C" fn(*mut c_void, u32) -> c_int,
        config_substitute: unsafe extern "C" fn(*mut c_void, *mut c_void, c_int) -> c_int,
        default_substitute: unsafe extern "C" fn(*mut c_void),
        font_sort: unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut *mut c_void, *mut c_int) -> *mut FcFontSet,
        font_set_destroy: unsafe extern "C" fn(*mut FcFontSet),
    }

    // The config and the function pointers are used under `FC`'s lock only.
    unsafe impl Send for Fc {}

    static FC: OnceLock<Option<Mutex<Fc>>> = OnceLock::new();

    unsafe fn load() -> Option<Fc> {
        let mut handle = dlopen(c"libfontconfig.so.1".as_ptr(), RTLD_NOW);
        if handle.is_null() {
            handle = dlopen(c"libfontconfig.so".as_ptr(), RTLD_NOW);
        }
        if handle.is_null() {
            return None;
        }
        macro_rules! sym {
            ($name:literal) => {{
                let ptr = dlsym(handle, concat!($name, "\0").as_ptr() as *const c_char);
                if ptr.is_null() {
                    return None;
                }
                std::mem::transmute::<*mut c_void, _>(ptr)
            }};
        }
        let init: unsafe extern "C" fn() -> *mut c_void = sym!("FcInitLoadConfigAndFonts");
        let config = init();
        if config.is_null() {
            return None;
        }
        Some(Fc {
            config,
            pattern_create: sym!("FcPatternCreate"),
            pattern_destroy: sym!("FcPatternDestroy"),
            pattern_add_string: sym!("FcPatternAddString"),
            pattern_add_integer: sym!("FcPatternAddInteger"),
            pattern_add_charset: sym!("FcPatternAddCharSet"),
            pattern_get_string: sym!("FcPatternGetString"),
            pattern_get_integer: sym!("FcPatternGetInteger"),
            pattern_get_charset: sym!("FcPatternGetCharSet"),
            charset_create: sym!("FcCharSetCreate"),
            charset_destroy: sym!("FcCharSetDestroy"),
            charset_add_char: sym!("FcCharSetAddChar"),
            charset_has_char: sym!("FcCharSetHasChar"),
            config_substitute: sym!("FcConfigSubstitute"),
            default_substitute: sym!("FcDefaultSubstitute"),
            font_sort: sym!("FcFontSort"),
            font_set_destroy: sym!("FcFontSetDestroy"),
        })
    }

    pub(super) fn find_family(name: &str, weight: u16, italic: bool) -> Option<SystemFont> {
        if name.eq_ignore_ascii_case(super::SYSTEM_UI_FAMILY) {
            let kde = std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains("kde");
            let families = if kde {
                ["noto sans", "cantarell", "ubuntu", "dejavu sans", "liberation sans", "freesans"]
            } else {
                ["cantarell", "ubuntu", "noto sans", "dejavu sans", "liberation sans", "freesans"]
            };
            families.iter().find_map(|family| super::font_scan::find(family, weight, italic))
        } else {
            super::font_scan::find(name, weight, italic)
        }
    }

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
        let scanned = super::font_scan::covering(text, bold, 4);
        if !scanned.is_empty() {
            return scanned;
        }
        let Some(fc) = FC.get_or_init(|| unsafe { load() }.map(Mutex::new)) else {
            return Vec::new();
        };
        let Ok(fc) = fc.lock() else {
            return Vec::new();
        };
        let mut chars: Vec<u32> = text.chars().map(|c| c as u32).collect();
        chars.sort_unstable();
        chars.dedup();
        let mut out = Vec::new();
        unsafe {
            let pattern = (fc.pattern_create)();
            let charset = (fc.charset_create)();
            if pattern.is_null() || charset.is_null() {
                return out;
            }
            for &c in &chars {
                (fc.charset_add_char)(charset, c);
            }
            (fc.pattern_add_charset)(pattern, c"charset".as_ptr(), charset);
            (fc.pattern_add_string)(pattern, c"family".as_ptr(), b"sans-serif\0".as_ptr());
            (fc.pattern_add_integer)(
                pattern,
                c"weight".as_ptr(),
                if bold { FC_WEIGHT_BOLD } else { FC_WEIGHT_REGULAR },
            );
            (fc.config_substitute)(fc.config, pattern, FC_MATCH_PATTERN);
            (fc.default_substitute)(pattern);
            let mut result: c_int = 0;
            let set = (fc.font_sort)(fc.config, pattern, 1, std::ptr::null_mut(), &mut result);
            if !set.is_null() {
                // The sorted set, best match first: keep the faces covering the
                // most of the text, in that order.
                let mut best = 0usize;
                let fonts = std::slice::from_raw_parts((*set).fonts, (*set).nfont.max(0) as usize);
                for &font in fonts {
                    let mut font_charset: *mut c_void = std::ptr::null_mut();
                    if (fc.pattern_get_charset)(font, c"charset".as_ptr(), 0, &mut font_charset) != FC_RESULT_MATCH {
                        continue;
                    }
                    let covered = chars.iter().filter(|&&c| (fc.charset_has_char)(font_charset, c) != 0).count();
                    if covered == 0 || covered < best {
                        continue;
                    }
                    let mut file: *const u8 = std::ptr::null();
                    if (fc.pattern_get_string)(font, c"file".as_ptr(), 0, &mut file) != FC_RESULT_MATCH || file.is_null() {
                        continue;
                    }
                    let Ok(path) = CStr::from_ptr(file as *const c_char).to_str() else {
                        continue;
                    };
                    let lower = path.to_ascii_lowercase();
                    if ![".ttf", ".otf", ".ttc", ".otc"].iter().any(|ext| lower.ends_with(ext)) {
                        continue;
                    }
                    let mut index: c_int = 0;
                    (fc.pattern_get_integer)(font, c"index".as_ptr(), 0, &mut index);
                    if covered > best {
                        best = covered;
                        out.clear();
                    }
                    out.push(SystemFont { path: path.into(), index: (index.max(0) as u32) & 0xffff });
                    if out.len() >= 4 {
                        break;
                    }
                }
                (fc.font_set_destroy)(set);
            }
            (fc.pattern_destroy)(pattern);
            (fc.charset_destroy)(charset);
        }
        out
    }
}

#[cfg(target_os = "windows")]
mod imp {
    //! The faces Windows' own font fallback uses per script.
    use super::{fallback_script, Script, SystemFont};

    fn files_for(script: Script, bold: bool) -> &'static [&'static str] {
        macro_rules! pick {
            ($regular:expr, $bold:expr) => {
                if bold { $bold } else { $regular }
            };
        }
        match script {
            Script::Devanagari | Script::Bengali | Script::Gurmukhi | Script::Gujarati
            | Script::Oriya | Script::Tamil | Script::Telugu | Script::Kannada
            | Script::Malayalam | Script::Sinhala | Script::Ol_Chiki => {
                pick!(&["Nirmala.ttc", "Nirmala.ttf"], &["Nirmala.ttc", "NirmalaB.ttf", "Nirmala.ttf"])
            }
            Script::Arabic | Script::Hebrew | Script::Armenian | Script::Georgian
            | Script::Greek | Script::Cyrillic | Script::Latin => {
                pick!(&["segoeui.ttf"], &["segoeuib.ttf", "segoeui.ttf"])
            }
            Script::Syriac => &["estre.ttf"],
            Script::Thaana => &["mvboli.ttf"],
            Script::Thai | Script::Lao | Script::Khmer | Script::Buginese => {
                pick!(&["LeelawUI.ttf", "leelawad.ttf"], &["LeelaUIb.ttf", "LeelawUI.ttf"])
            }
            Script::Myanmar => pick!(&["mmrtext.ttf"], &["mmrtextb.ttf", "mmrtext.ttf"]),
            Script::Ethiopic | Script::Nko | Script::Tifinagh | Script::Vai => {
                pick!(&["ebrima.ttf"], &["ebrimabd.ttf", "ebrima.ttf"])
            }
            Script::Tibetan => &["himalaya.ttf"],
            Script::Mongolian => &["monbaiti.ttf"],
            Script::Cherokee | Script::Canadian_Aboriginal | Script::Yi => &["gadugi.ttf"],
            Script::Hangul => pick!(&["malgun.ttf"], &["malgunbd.ttf", "malgun.ttf"]),
            Script::Katakana => {
                pick!(&["YuGothM.ttc", "msgothic.ttc"], &["YuGothB.ttc", "YuGothM.ttc", "msgothic.ttc"])
            }
            Script::Han | Script::Bopomofo => {
                pick!(&["msyh.ttc", "simsun.ttc"], &["msyhbd.ttc", "msyh.ttc", "simsun.ttc"])
            }
            _ => &["seguisym.ttf", "segoeui.ttf"],
        }
    }

    pub(super) fn find_family(name: &str, weight: u16, italic: bool) -> Option<SystemFont> {
        let name = if name.eq_ignore_ascii_case(super::SYSTEM_UI_FAMILY) { "segoe ui" } else { name };
        super::font_scan::find(name, weight, italic)
    }

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let dir = std::path::PathBuf::from(windir).join("Fonts");
        let script = text.chars().map(fallback_script).find(|s| !matches!(s, Script::Common | Script::Inherited));
        let extra: &[&str] = if script.is_some() { &["seguisym.ttf"] } else { &[] };
        let mut out: Vec<SystemFont> = files_for(script.unwrap_or(Script::Common), bold)
            .iter()
            .chain(extra.iter())
            .map(|f| SystemFont { path: dir.join(f), index: 0 })
            .filter(|f| f.path.exists())
            .collect();
        // Scanned faces once the background index is there (never a scan here).
        let scanned = if super::font_scan::prewarmed() { super::font_scan::covering(text, bold, 2) } else { Vec::new() };
        for font in scanned {
            if !out.iter().any(|existing| {
                existing.index == font.index
                    && std::fs::canonicalize(&existing.path).unwrap_or_else(|_| existing.path.clone()) == font.path
            }) {
                out.push(font);
            }
        }
        out
    }
}

#[cfg(target_os = "android")]
mod imp {
    //! The Noto faces in the system font folder named after the script.
    use super::{fallback_script, Script, SystemFont};

    pub(super) fn find_family(name: &str, weight: u16, italic: bool) -> Option<SystemFont> {
        if name.eq_ignore_ascii_case(super::SYSTEM_UI_FAMILY) {
            ["roboto", "roboto flex", "roboto static"]
                .iter()
                .find_map(|family| super::font_scan::find(family, weight, italic))
        } else {
            super::font_scan::find(name, weight, italic)
        }
    }

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
        let mut out = noto_candidates(text, bold);
        // Scanned faces once the background index is there (never a scan here).
        let scanned = if super::font_scan::prewarmed() { super::font_scan::covering(text, bold, 2) } else { Vec::new() };
        for font in scanned {
            if !out.iter().any(|existing| {
                existing.index == font.index
                    && std::fs::canonicalize(&existing.path).unwrap_or_else(|_| existing.path.clone()) == font.path
            }) {
                out.push(font);
            }
        }
        out
    }

    fn noto_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
        let Some(script) = text.chars().map(fallback_script).find(|s| !matches!(s, Script::Common | Script::Inherited)) else {
            return Vec::new();
        };
        let key = match script {
            Script::Han | Script::Hangul | Script::Katakana | Script::Bopomofo => "CJK",
            Script::Arabic => "Arabic",
            script => script.full_name(),
        };
        let Ok(entries) = std::fs::read_dir("/system/fonts") else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| name.starts_with("Noto") && name.contains(key))
            .collect();
        // Sans before serif, the UI variant first, the asked weight first.
        names.sort_by_key(|name| {
            (
                !name.contains("Sans"),
                !name.contains("UI"),
                name.contains("Bold") != bold,
                name.clone(),
            )
        });
        names
            .into_iter()
            .take(4)
            .flat_map(|name| {
                let path = std::path::Path::new("/system/fonts").join(&name);
                // A CJK collection holds one face per language: try them all.
                let faces = if name.ends_with(".ttc") { 0..4 } else { 0..1 };
                faces.map(move |index| SystemFont { path: path.clone(), index })
            })
            .collect()
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
mod imp {
    use super::SystemFont;

    pub(super) fn find_family(_name: &str, _weight: u16, _italic: bool) -> Option<SystemFont> {
        None
    }

    pub(super) fn fallback_candidates(_text: &str, _bold: bool) -> Vec<SystemFont> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana_fold_into_one_group() {
        assert_eq!(fallback_script('あ'), fallback_script('カ'));
        assert_eq!(fallback_script('क'), Script::Devanagari);
        assert_eq!(fallback_script('ি'), Script::Bengali);
    }

    /// Print installed Apple faces without requiring a particular OS font set.
    #[cfg(target_vendor = "apple")]
    #[test]
    fn apple_family_lookup() {
        let regular = find_family("Helvetica", 400, false);
        let bold = find_family("Helvetica", 700, false);
        let system = find_family(SYSTEM_UI_FAMILY, 400, false);
        println!("Helvetica 400: {regular:?}");
        println!("Helvetica 700: {bold:?}");
        println!("system-ui: {system:?}");
        if std::path::Path::new("/System/Library/Fonts/Helvetica.ttc").exists() {
            assert!(regular.as_ref().is_some_and(|font| font.path.exists()));
        }
    }

    /// The OS answers for each script the samples use, where it has a font
    /// (printed, so a node's report shows what it found).
    #[test]
    fn os_fallback_for_sample_scripts() {
        for sample in ["हिन्दी", "العربية", "ภาษาไทย", "ひらがな", "한국어", "中文"] {
            let found = fallback_candidates(sample, false);
            println!("{sample}: {found:?}");
        }
    }
}
