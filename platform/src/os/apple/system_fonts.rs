//! macOS system font provider using Core Text.

use std::ffi::CStr;
use std::path::PathBuf;

use crate::os::apple::apple_sys::{
    msg_send, sel, sel_impl, nil, ObjcId, CFStringRef,
    CTFontCreateWithName, CTFontCopyAttribute, CTFontManagerCopyAvailableFontFamilyNames,
    kCTFontURLAttribute,
};
use crate::os::apple::apple_util::str_to_nsstring;
use crate::os::system_fonts::{SystemFontProvider, SystemFontData, SystemFontError};

/// macOS system font provider using Core Text.
pub struct MacOSFontProvider;

impl MacOSFontProvider {
    pub fn new() -> Self {
        MacOSFontProvider
    }
}

impl Default for MacOSFontProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemFontProvider for MacOSFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        unsafe {
            let family_cfstring = str_to_nsstring(family) as CFStringRef;
            let font: ObjcId = CTFontCreateWithName(family_cfstring, 12.0, std::ptr::null());

            if font == nil {
                return Err(SystemFontError::FontNotFound(family.to_string()));
            }

            // Get font file path from URL attribute
            let url: ObjcId = CTFontCopyAttribute(font, kCTFontURLAttribute);
            if url == nil {
                let () = msg_send![font, release];
                return Err(SystemFontError::FontNotFound(family.to_string()));
            }

            let path_string: ObjcId = msg_send![url, path];
            let utf8_ptr: *const std::os::raw::c_char = msg_send![path_string, UTF8String];
            let path = PathBuf::from(CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned());

            let () = msg_send![url, release];
            let () = msg_send![font, release];

            // Read font file
            let data = std::fs::read(&path).map_err(|e| {
                SystemFontError::ReadError(format!("{}: {}", path.display(), e))
            })?;

            Ok(SystemFontData {
                data,
                index: 0,
                family_name: family.to_string(),
            })
        }
    }

    fn list_families(&self) -> Vec<String> {
        unsafe {
            let array: ObjcId = CTFontManagerCopyAvailableFontFamilyNames();
            if array == nil {
                return Vec::new();
            }

            let count: u64 = msg_send![array, count];
            let mut families = Vec::with_capacity(count as usize);

            for i in 0..count {
                let ns_string: ObjcId = msg_send![array, objectAtIndex: i];
                if ns_string != nil {
                    let utf8_ptr: *const std::os::raw::c_char = msg_send![ns_string, UTF8String];
                    if !utf8_ptr.is_null() {
                        families.push(CStr::from_ptr(utf8_ptr).to_string_lossy().into_owned());
                    }
                }
            }

            let () = msg_send![array, release];
            families
        }
    }
}

pub fn get_system_font_provider() -> MacOSFontProvider {
    MacOSFontProvider::new()
}
