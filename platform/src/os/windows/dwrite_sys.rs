//! Resolve the platform's default fonts at runtime via DirectWrite.
//!
//! The vendored `windows-rs` bindings in this repo are pre-generated and do not
//! include the DirectWrite surface, so we hand-declare the minimal COM
//! interfaces we need (consumer side only) using the same `windows_core`
//! `Interface` machinery the rest of the Windows backend uses. We never bundle
//! text fonts — glyph coverage is delegated to whatever the OS has installed.
//!
//! Everything degrades to `None` on any failure.
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(dead_code)]

use crate::cx_api::{SystemFontQuery, SystemFontResult, SystemFontRole};
use crate::windows::core::{self as wcore, Interface, BOOL, HRESULT, PCWSTR};

type DWRITE_FACTORY_TYPE = i32;
const DWRITE_FACTORY_TYPE_SHARED: DWRITE_FACTORY_TYPE = 0;

type DWRITE_FONT_WEIGHT = i32;
const DWRITE_FONT_WEIGHT_REGULAR: DWRITE_FONT_WEIGHT = 400;
const DWRITE_FONT_WEIGHT_BOLD: DWRITE_FONT_WEIGHT = 700;

type DWRITE_FONT_STRETCH = i32;
const DWRITE_FONT_STRETCH_NORMAL: DWRITE_FONT_STRETCH = 5;

type DWRITE_FONT_STYLE = i32;
const DWRITE_FONT_STYLE_NORMAL: DWRITE_FONT_STYLE = 0;
const DWRITE_FONT_STYLE_ITALIC: DWRITE_FONT_STYLE = 2;

wcore::link!("dwrite.dll" "system" fn DWriteCreateFactory(factorytype: DWRITE_FACTORY_TYPE, iid: *const wcore::GUID, factory: *mut *mut core::ffi::c_void) -> HRESULT);

// --- IDWriteFactory (only GetSystemFontCollection used) ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFactory(wcore::IUnknown);
unsafe impl Interface for IDWriteFactory {
    type Vtable = IDWriteFactory_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0xb859ee5a_d838_4b5b_a2e8_1adc7d93db48);
}
#[repr(C)]
struct IDWriteFactory_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    GetSystemFontCollection: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        collection: *mut *mut core::ffi::c_void,
        check_for_updates: BOOL,
    ) -> HRESULT,
    // Remaining methods are unused; we never call past GetSystemFontCollection,
    // so we don't need to declare them (the vtable layout only matters up to the
    // last slot we invoke).
}
impl IDWriteFactory {
    unsafe fn GetSystemFontCollection(
        &self,
        check_for_updates: bool,
    ) -> Option<IDWriteFontCollection> {
        let mut out: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).GetSystemFontCollection)(
            Interface::as_raw(self),
            &mut out,
            BOOL(check_for_updates as i32),
        );
        if hr.is_err() || out.is_null() {
            return None;
        }
        Some(IDWriteFontCollection::from_raw(out))
    }
}

// --- IDWriteFontCollection ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFontCollection(wcore::IUnknown);
unsafe impl Interface for IDWriteFontCollection {
    type Vtable = IDWriteFontCollection_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0xa84cee02_3eea_4eee_a827_87c1a02a0fcc);
}
#[repr(C)]
struct IDWriteFontCollection_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    GetFontFamilyCount: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
    GetFontFamily: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        index: u32,
        family: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    FindFamilyName: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        familyname: PCWSTR,
        index: *mut u32,
        exists: *mut BOOL,
    ) -> HRESULT,
}
impl IDWriteFontCollection {
    unsafe fn FindFamilyName(&self, name: &[u16]) -> Option<u32> {
        let mut index: u32 = 0;
        let mut exists = BOOL(0);
        let hr = (Interface::vtable(self).FindFamilyName)(
            Interface::as_raw(self),
            PCWSTR(name.as_ptr()),
            &mut index,
            &mut exists,
        );
        if hr.is_err() || exists.0 == 0 {
            return None;
        }
        Some(index)
    }
    unsafe fn GetFontFamilyCount(&self) -> u32 {
        (Interface::vtable(self).GetFontFamilyCount)(Interface::as_raw(self))
    }
    unsafe fn GetFontFamily(&self, index: u32) -> Option<IDWriteFontFamily> {
        let mut out: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr =
            (Interface::vtable(self).GetFontFamily)(Interface::as_raw(self), index, &mut out);
        if hr.is_err() || out.is_null() {
            return None;
        }
        Some(IDWriteFontFamily::from_raw(out))
    }
}

// --- IDWriteFontFamily (derives IDWriteFontList) ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFontFamily(wcore::IUnknown);
unsafe impl Interface for IDWriteFontFamily {
    type Vtable = IDWriteFontFamily_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0xda20d8ef_812a_4c43_9802_62ec4abd7adf);
}
#[repr(C)]
struct IDWriteFontFamily_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    // IDWriteFontList methods (this interface derives from it):
    GetFontCollection: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        collection: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    GetFontCount: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
    GetFont: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        index: u32,
        font: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    // IDWriteFontFamily methods:
    GetFamilyNames: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        names: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    GetFirstMatchingFont: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        weight: DWRITE_FONT_WEIGHT,
        stretch: DWRITE_FONT_STRETCH,
        style: DWRITE_FONT_STYLE,
        font: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
}
impl IDWriteFontFamily {
    unsafe fn GetFirstMatchingFont(
        &self,
        weight: DWRITE_FONT_WEIGHT,
        stretch: DWRITE_FONT_STRETCH,
        style: DWRITE_FONT_STYLE,
    ) -> Option<IDWriteFont> {
        let mut out: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).GetFirstMatchingFont)(
            Interface::as_raw(self),
            weight,
            stretch,
            style,
            &mut out,
        );
        if hr.is_err() || out.is_null() {
            return None;
        }
        Some(IDWriteFont::from_raw(out))
    }
}

// --- IDWriteFont (only CreateFontFace used; it is the last vtable slot we call) ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFont(wcore::IUnknown);
unsafe impl Interface for IDWriteFont {
    type Vtable = IDWriteFont_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0xacd16696_8c14_4f5d_877e_fe3fc1d32737);
}
#[repr(C)]
struct IDWriteFont_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    GetFontFamily: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        family: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    GetWeight: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> DWRITE_FONT_WEIGHT,
    GetStretch: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> DWRITE_FONT_STRETCH,
    GetStyle: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> DWRITE_FONT_STYLE,
    IsSymbolFont: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> BOOL,
    GetFaceNames: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        names: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    GetInformationalStrings: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        info_string_id: i32,
        strings: *mut *mut core::ffi::c_void,
        exists: *mut BOOL,
    ) -> HRESULT,
    GetSimulations: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> i32,
    GetMetrics: unsafe extern "system" fn(this: *mut core::ffi::c_void, metrics: *mut core::ffi::c_void),
    HasCharacter: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        unicode_value: u32,
        exists: *mut BOOL,
    ) -> HRESULT,
    CreateFontFace: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        fontface: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
}
impl IDWriteFont {
    /// True if this font covers `ch` (DirectWrite's cmap lookup).
    unsafe fn HasCharacter(&self, ch: char) -> bool {
        let mut exists = BOOL(0);
        let hr = (Interface::vtable(self).HasCharacter)(
            Interface::as_raw(self),
            ch as u32,
            &mut exists,
        );
        hr.is_ok() && exists.0 != 0
    }

    unsafe fn CreateFontFace(&self) -> Option<IDWriteFontFace> {
        let mut out: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr =
            (Interface::vtable(self).CreateFontFace)(Interface::as_raw(self), &mut out);
        if hr.is_err() || out.is_null() {
            return None;
        }
        Some(IDWriteFontFace::from_raw(out))
    }
}

// --- IDWriteFontFace (GetFiles + GetIndex used) ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFontFace(wcore::IUnknown);
unsafe impl Interface for IDWriteFontFace {
    type Vtable = IDWriteFontFace_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0x5f49804d_7024_4d43_bfa9_d25984f53849);
}
#[repr(C)]
struct IDWriteFontFace_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    GetType: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> i32,
    GetFiles: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        number_of_files: *mut u32,
        font_files: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    // Next vtable slot after GetFiles: the zero-based face index within the
    // font file. Non-zero for a face inside a `.ttc` collection.
    GetIndex: unsafe extern "system" fn(this: *mut core::ffi::c_void) -> u32,
}
impl IDWriteFontFace {
    /// The zero-based index of this face within its (possibly `.ttc`) file.
    unsafe fn GetIndex(&self) -> u32 {
        (Interface::vtable(self).GetIndex)(Interface::as_raw(self))
    }

    /// Return the first font file backing this face.
    unsafe fn GetFirstFile(&self) -> Option<IDWriteFontFile> {
        // First query the count.
        let mut count: u32 = 0;
        let hr = (Interface::vtable(self).GetFiles)(
            Interface::as_raw(self),
            &mut count,
            core::ptr::null_mut(),
        );
        if hr.is_err() || count == 0 {
            return None;
        }
        // Fetch the file pointers (DirectWrite AddRefs each one).
        let mut files: Vec<*mut core::ffi::c_void> = vec![core::ptr::null_mut(); count as usize];
        let hr = (Interface::vtable(self).GetFiles)(
            Interface::as_raw(self),
            &mut count,
            files.as_mut_ptr(),
        );
        if hr.is_err() {
            return None;
        }
        // Take the first; release the rest.
        let mut first = None;
        for (i, &p) in files.iter().enumerate() {
            if p.is_null() {
                continue;
            }
            if i == 0 {
                first = Some(IDWriteFontFile::from_raw(p));
            } else {
                // from_raw takes ownership → drop releases the extra refs.
                drop(IDWriteFontFile::from_raw(p));
            }
        }
        first
    }
}

// --- IDWriteFontFile ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFontFile(wcore::IUnknown);
unsafe impl Interface for IDWriteFontFile {
    type Vtable = IDWriteFontFile_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0x739d886a_cef5_47dc_8769_1a8b41bebbb0);
}
#[repr(C)]
struct IDWriteFontFile_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    GetReferenceKey: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        reference_key: *mut *const core::ffi::c_void,
        reference_key_size: *mut u32,
    ) -> HRESULT,
    GetLoader: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        loader: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    Analyze: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        is_supported: *mut BOOL,
        file_type: *mut i32,
        face_type: *mut i32,
        number_of_faces: *mut u32,
    ) -> HRESULT,
}
impl IDWriteFontFile {
    unsafe fn read_bytes(&self) -> Option<Vec<u8>> {
        let mut key: *const core::ffi::c_void = core::ptr::null();
        let mut key_size: u32 = 0;
        let hr = (Interface::vtable(self).GetReferenceKey)(
            Interface::as_raw(self),
            &mut key,
            &mut key_size,
        );
        if hr.is_err() || key.is_null() {
            return None;
        }
        let mut loader_raw: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr =
            (Interface::vtable(self).GetLoader)(Interface::as_raw(self), &mut loader_raw);
        if hr.is_err() || loader_raw.is_null() {
            return None;
        }
        let loader = IDWriteFontFileLoader::from_raw(loader_raw);
        let stream = loader.CreateStreamFromKey(key, key_size)?;
        stream.read_all()
    }
}

// --- IDWriteFontFileLoader ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFontFileLoader(wcore::IUnknown);
unsafe impl Interface for IDWriteFontFileLoader {
    type Vtable = IDWriteFontFileLoader_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0x727cad4e_d6af_4c9e_8a08_d695b11caa49);
}
#[repr(C)]
struct IDWriteFontFileLoader_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    CreateStreamFromKey: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        reference_key: *const core::ffi::c_void,
        reference_key_size: u32,
        stream: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
}
impl IDWriteFontFileLoader {
    unsafe fn CreateStreamFromKey(
        &self,
        key: *const core::ffi::c_void,
        key_size: u32,
    ) -> Option<IDWriteFontFileStream> {
        let mut out: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).CreateStreamFromKey)(
            Interface::as_raw(self),
            key,
            key_size,
            &mut out,
        );
        if hr.is_err() || out.is_null() {
            return None;
        }
        Some(IDWriteFontFileStream::from_raw(out))
    }
}

// --- IDWriteFontFileStream ---
#[repr(transparent)]
#[derive(Clone)]
struct IDWriteFontFileStream(wcore::IUnknown);
unsafe impl Interface for IDWriteFontFileStream {
    type Vtable = IDWriteFontFileStream_Vtbl;
    const IID: wcore::GUID = wcore::GUID::from_u128(0x6d4865fe_0ab8_4d91_8f62_5dd6be34a3e0);
}
#[repr(C)]
struct IDWriteFontFileStream_Vtbl {
    base__: wcore::IUnknown_Vtbl,
    ReadFileFragment: unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        fragment_start: *mut *const core::ffi::c_void,
        file_offset: u64,
        fragment_size: u64,
        fragment_context: *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    ReleaseFileFragment:
        unsafe extern "system" fn(this: *mut core::ffi::c_void, fragment_context: *mut core::ffi::c_void),
    GetFileSize:
        unsafe extern "system" fn(this: *mut core::ffi::c_void, file_size: *mut u64) -> HRESULT,
    GetLastWriteTime:
        unsafe extern "system" fn(this: *mut core::ffi::c_void, last_write_time: *mut u64) -> HRESULT,
}
impl IDWriteFontFileStream {
    unsafe fn read_all(&self) -> Option<Vec<u8>> {
        let mut size: u64 = 0;
        let hr = (Interface::vtable(self).GetFileSize)(Interface::as_raw(self), &mut size);
        if hr.is_err() || size == 0 {
            return None;
        }
        let mut frag: *const core::ffi::c_void = core::ptr::null();
        let mut ctx: *mut core::ffi::c_void = core::ptr::null_mut();
        let hr = (Interface::vtable(self).ReadFileFragment)(
            Interface::as_raw(self),
            &mut frag,
            0,
            size,
            &mut ctx,
        );
        if hr.is_err() || frag.is_null() {
            return None;
        }
        let bytes = core::slice::from_raw_parts(frag as *const u8, size as usize).to_vec();
        (Interface::vtable(self).ReleaseFileFragment)(Interface::as_raw(self), ctx);
        Some(bytes)
    }
}

/// Resolve `query` to font file bytes using DirectWrite, or `None` on failure.
pub fn load_system_font(query: &SystemFontQuery) -> Option<SystemFontResult> {
    unsafe { load_system_font_inner(query) }
}

/// Cheap validity check: does `bytes` start with a recognized sfnt magic number?
/// Replaces the old `bytes.len() > 1000` heuristic — DirectWrite can hand back a
/// stream that isn't a standalone font file (e.g. an in-memory reference we can't
/// slice), and a byte-length guess doesn't actually verify that.
fn is_sfnt(bytes: &[u8]) -> bool {
    let Some(magic) = bytes.get(0..4) else {
        return false;
    };
    matches!(
        magic,
        [0x00, 0x01, 0x00, 0x00] // TrueType outlines
            | b"OTTO"            // CFF / OpenType outlines
            | b"ttcf"            // TrueType Collection
            | b"true"            // legacy Apple TrueType
            | b"typ1" // legacy Type 1 sfnt
    )
}

unsafe fn load_system_font_inner(query: &SystemFontQuery) -> Option<SystemFontResult> {
    let mut factory_raw: *mut core::ffi::c_void = core::ptr::null_mut();
    let hr = DWriteCreateFactory(
        DWRITE_FACTORY_TYPE_SHARED,
        &IDWriteFactory::IID,
        &mut factory_raw,
    );
    if hr.is_err() || factory_raw.is_null() {
        return None;
    }
    let factory = IDWriteFactory::from_raw(factory_raw);
    let collection = factory.GetSystemFontCollection(false)?;

    let weight = if query.weight >= 600 {
        DWRITE_FONT_WEIGHT_BOLD
    } else {
        DWRITE_FONT_WEIGHT_REGULAR
    };
    let style = if query.italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };

    // Per-glyph fallback: if the caller passed the actual uncovered characters,
    // find the first installed family whose representative font covers them all
    // (DirectWrite's `HasCharacter` is a cmap lookup). Enumerating the whole
    // collection is O(families), but this runs at most once per script (the
    // caller caches the result and guards with `attempted_scripts`), so it is
    // acceptable and avoids hand-rolling the `IDWriteFontFallback` /
    // `IDWriteTextAnalysisSource` COM-callback surface.
    if !query.sample.is_empty() {
        let count = collection.GetFontFamilyCount();
        for index in 0..count {
            let Some(family) = collection.GetFontFamily(index) else {
                continue;
            };
            let Some(font) =
                family.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)
            else {
                continue;
            };
            if !query.sample.chars().all(|ch| font.HasCharacter(ch)) {
                continue;
            }
            let Some(face) = font.CreateFontFace() else {
                continue;
            };
            let Some(file) = face.GetFirstFile() else {
                continue;
            };
            if let Some(bytes) = file.read_bytes() {
                if is_sfnt(&bytes) {
                    // `.ttc` collection → the covering face is often not index 0.
                    // DirectWrite reads through a loader stream (not a guaranteed
                    // filesystem path), so there's nothing to mmap — owned variant.
                    return Some(SystemFontResult {
                        bytes: crate::shared_bytes::SharedBytes::from_owned(std::rc::Rc::new(bytes)),
                        index: face.GetIndex(),
                    });
                }
            }
        }
        return None;
    }

    for name in role_family_candidates(query.role, &query.lang) {
        let wide = to_wide(name);
        let Some(index) = collection.FindFamilyName(&wide) else {
            continue;
        };
        let Some(family) = collection.GetFontFamily(index) else {
            continue;
        };
        let Some(font) =
            family.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)
        else {
            continue;
        };
        let Some(face) = font.CreateFontFace() else {
            continue;
        };
        let Some(file) = face.GetFirstFile() else {
            continue;
        };
        if let Some(bytes) = file.read_bytes() {
            if is_sfnt(&bytes) {
                // `.ttc` collection → the covering face is often not index 0.
                // DirectWrite reads through a loader stream (not a guaranteed
                // filesystem path), so there's nothing to mmap — owned variant.
                return Some(SystemFontResult {
                    bytes: crate::shared_bytes::SharedBytes::from_owned(std::rc::Rc::new(bytes)),
                    index: face.GetIndex(),
                });
            }
        }
    }
    None
}

/// Candidate Windows font-family names per role, in priority order. These are
/// the families that ship with Windows 10/11; the first one present wins.
fn role_family_candidates(role: SystemFontRole, lang: &str) -> Vec<&'static str> {
    match role {
        SystemFontRole::Ui => vec!["Segoe UI", "Tahoma", "Arial"],
        SystemFontRole::Serif => vec!["Times New Roman", "Georgia"],
        SystemFontRole::Mono => vec!["Consolas", "Courier New"],
        SystemFontRole::Cjk => match lang {
            "ja" => vec!["Yu Gothic UI", "Meiryo UI", "MS Gothic"],
            "ko" => vec!["Malgun Gothic", "Gulim"],
            // zh-Hant
            "zh-Hant" | "zh-TW" | "zh-HK" => vec!["Microsoft JhengHei UI", "Microsoft JhengHei", "PMingLiU"],
            // default zh-Hans
            _ => vec!["Microsoft YaHei UI", "Microsoft YaHei", "SimSun"],
        },
        SystemFontRole::Emoji => vec!["Segoe UI Emoji", "Segoe UI Symbol"],
    }
}

/// UTF-16 NUL-terminated string for PCWSTR family-name args.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}
