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
//! - Linux / BSD: fontconfig (`libfontconfig.so.1`, opened at runtime, so a
//!   system without it builds and runs; the fallback then finds nothing): the
//!   sorted match for a pattern carrying the characters, the first face whose
//!   charset covers them.
//! - Windows: the fonts Windows' own fallback uses per script, in the system
//!   font folder.
//! - Android: the system font folder's Noto faces for the script.
//!
//! The caller checks a candidate's cmap before using it: a candidate is a
//! suggestion, never trusted to cover the text.

use std::path::PathBuf;

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

/// The face of `data` (a font or a collection) named `postscript_name`.
#[allow(dead_code)]
fn collection_index_of(data: &[u8], postscript_name: &str) -> Option<u32> {
    let count = rustybuzz::ttf_parser::fonts_in_collection(data).unwrap_or(1);
    (0..count).find(|&index| {
        rustybuzz::ttf_parser::Face::parse(data, index).ok().is_some_and(|face| {
            face.names().into_iter().any(|name| {
                name.name_id == rustybuzz::ttf_parser::name_id::POST_SCRIPT_NAME
                    && name.to_string().is_some_and(|n| n == postscript_name)
            })
        })
    })
}

#[cfg(target_vendor = "apple")]
mod imp {
    //! CoreText: the font the OS's own text system falls back to.
    use super::SystemFont;
    use std::ffi::c_void;

    type CFIndex = isize;

    #[repr(C)]
    struct CFRange {
        location: CFIndex,
        length: CFIndex,
    }

    const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const KCT_FONT_UI_FONT_SYSTEM: u32 = 2;
    const KCT_FONT_UI_FONT_EMPHASIZED_SYSTEM: u32 = 3;

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
        static kCTFontURLAttribute: *const c_void;
    }

    unsafe fn cf_to_string(string: *const c_void) -> Option<String> {
        if string.is_null() {
            return None;
        }
        let mut buffer = vec![0u8; 512];
        let ok = CFStringGetCString(
            string,
            buffer.as_mut_ptr(),
            buffer.len() as CFIndex,
            KCF_STRING_ENCODING_UTF8,
        );
        if !ok {
            return None;
        }
        let len = buffer.iter().position(|&b| b == 0)?;
        buffer.truncate(len);
        String::from_utf8(buffer).ok()
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
    //! fontconfig, opened at runtime.
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

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
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

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let dir = std::path::PathBuf::from(windir).join("Fonts");
        let Some(script) = text.chars().map(fallback_script).find(|s| !matches!(s, Script::Common | Script::Inherited)) else {
            return files_for(Script::Common, bold).iter().map(|f| SystemFont { path: dir.join(f), index: 0 }).filter(|f| f.path.exists()).collect();
        };
        files_for(script, bold)
            .iter()
            .chain(["seguisym.ttf"].iter())
            .map(|f| SystemFont { path: dir.join(f), index: 0 })
            .filter(|f| f.path.exists())
            .collect()
    }
}

#[cfg(target_os = "android")]
mod imp {
    //! The Noto faces in the system font folder named after the script.
    use super::{fallback_script, Script, SystemFont};

    pub(super) fn fallback_candidates(text: &str, bold: bool) -> Vec<SystemFont> {
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
