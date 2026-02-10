//! Fontconfig FFI bindings for Linux system font access.
//! Uses dynamic loading to avoid hard dependency on fontconfig.

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_uchar, c_void};
use super::module_loader::ModuleLoader;

// Fontconfig types
pub type FcConfig = c_void;
pub type FcPattern = c_void;
pub type FcFontSet = c_void;
pub type FcObjectSet = c_void;
pub type FcResult = c_int;
pub type FcChar8 = c_uchar;
pub type FcBool = c_int;

// FcResult values
pub const FcResultMatch: FcResult = 0;
pub const FcResultNoMatch: FcResult = 1;
pub const FcResultTypeMismatch: FcResult = 2;
pub const FcResultNoId: FcResult = 3;
pub const FcResultOutOfMemory: FcResult = 4;

// FcMatchKind
pub const FcMatchPattern: c_int = 0;
pub const FcMatchFont: c_int = 1;

// Property names
pub const FC_FAMILY: &[u8] = b"family\0";
pub const FC_FILE: &[u8] = b"file\0";
pub const FC_INDEX: &[u8] = b"index\0";
pub const FC_STYLE: &[u8] = b"style\0";
pub const FC_WEIGHT: &[u8] = b"weight\0";
pub const FC_SLANT: &[u8] = b"slant\0";

// FcFontSet structure (we need to access its fields)
#[repr(C)]
pub struct FcFontSetRaw {
    pub nfont: c_int,
    pub sfont: c_int,
    pub fonts: *mut *mut FcPattern,
}

/// Dynamically loaded fontconfig functions
pub struct Fontconfig {
    _lib: ModuleLoader,
    pub FcInitLoadConfigAndFonts: unsafe extern "C" fn() -> *mut FcConfig,
    pub FcPatternCreate: unsafe extern "C" fn() -> *mut FcPattern,
    pub FcPatternDestroy: unsafe extern "C" fn(*mut FcPattern),
    pub FcPatternAddString: unsafe extern "C" fn(*mut FcPattern, *const c_char, *const FcChar8) -> FcBool,
    pub FcConfigSubstitute: unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, c_int) -> FcBool,
    pub FcDefaultSubstitute: unsafe extern "C" fn(*mut FcPattern),
    pub FcFontMatch: unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, *mut FcResult) -> *mut FcPattern,
    pub FcPatternGetString: unsafe extern "C" fn(*const FcPattern, *const c_char, c_int, *mut *mut FcChar8) -> FcResult,
    pub FcPatternGetInteger: unsafe extern "C" fn(*const FcPattern, *const c_char, c_int, *mut c_int) -> FcResult,
    pub FcFontList: unsafe extern "C" fn(*mut FcConfig, *mut FcPattern, *mut FcObjectSet) -> *mut FcFontSetRaw,
    pub FcFontSetDestroy: unsafe extern "C" fn(*mut FcFontSetRaw),
    pub FcObjectSetCreate: unsafe extern "C" fn() -> *mut FcObjectSet,
    pub FcObjectSetAdd: unsafe extern "C" fn(*mut FcObjectSet, *const c_char) -> FcBool,
    pub FcObjectSetDestroy: unsafe extern "C" fn(*mut FcObjectSet),
    pub FcConfigDestroy: unsafe extern "C" fn(*mut FcConfig),
}

impl Fontconfig {
    pub fn load() -> Result<Self, ()> {
        let lib = ModuleLoader::load("libfontconfig.so.1")
            .or_else(|_| ModuleLoader::load("libfontconfig.so"))?;

        Ok(Self {
            FcInitLoadConfigAndFonts: lib.get_symbol("FcInitLoadConfigAndFonts")?,
            FcPatternCreate: lib.get_symbol("FcPatternCreate")?,
            FcPatternDestroy: lib.get_symbol("FcPatternDestroy")?,
            FcPatternAddString: lib.get_symbol("FcPatternAddString")?,
            FcConfigSubstitute: lib.get_symbol("FcConfigSubstitute")?,
            FcDefaultSubstitute: lib.get_symbol("FcDefaultSubstitute")?,
            FcFontMatch: lib.get_symbol("FcFontMatch")?,
            FcPatternGetString: lib.get_symbol("FcPatternGetString")?,
            FcPatternGetInteger: lib.get_symbol("FcPatternGetInteger")?,
            FcFontList: lib.get_symbol("FcFontList")?,
            FcFontSetDestroy: lib.get_symbol("FcFontSetDestroy")?,
            FcObjectSetCreate: lib.get_symbol("FcObjectSetCreate")?,
            FcObjectSetAdd: lib.get_symbol("FcObjectSetAdd")?,
            FcObjectSetDestroy: lib.get_symbol("FcObjectSetDestroy")?,
            FcConfigDestroy: lib.get_symbol("FcConfigDestroy")?,
            _lib: lib,
        })
    }
}
