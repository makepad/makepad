//! Linux system font provider using Fontconfig.

use crate::os::system_fonts::{SystemFontProvider, SystemFontData, SystemFontError};
use super::fontconfig_sys::*;
use std::ffi::CStr;
use std::ptr;

/// Linux system font provider using Fontconfig.
pub struct LinuxFontProvider {
    fc: Fontconfig,
    config: *mut FcConfig,
}

unsafe impl Send for LinuxFontProvider {}
unsafe impl Sync for LinuxFontProvider {}

impl LinuxFontProvider {
    pub fn new() -> Result<Self, SystemFontError> {
        let fc = Fontconfig::load().map_err(|_| SystemFontError::PlatformNotSupported)?;
        let config = unsafe { (fc.FcInitLoadConfigAndFonts)() };
        if config.is_null() {
            return Err(SystemFontError::PlatformNotSupported);
        }
        Ok(Self { fc, config })
    }
}

impl Drop for LinuxFontProvider {
    fn drop(&mut self) {
        if !self.config.is_null() {
            unsafe { (self.fc.FcConfigDestroy)(self.config) };
        }
    }
}

impl SystemFontProvider for LinuxFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        unsafe {
            let pattern = (self.fc.FcPatternCreate)();
            if pattern.is_null() {
                return Err(SystemFontError::PlatformNotSupported);
            }

            let family_cstr = std::ffi::CString::new(family).unwrap();
            (self.fc.FcPatternAddString)(
                pattern,
                FC_FAMILY.as_ptr() as *const i8,
                family_cstr.as_ptr() as *const u8,
            );

            (self.fc.FcConfigSubstitute)(self.config, pattern, FcMatchPattern);
            (self.fc.FcDefaultSubstitute)(pattern);

            let mut result: FcResult = 0;
            let matched = (self.fc.FcFontMatch)(self.config, pattern, &mut result);
            (self.fc.FcPatternDestroy)(pattern);

            if matched.is_null() || result != FcResultMatch {
                return Err(SystemFontError::FontNotFound(family.to_string()));
            }

            // Get file path
            let mut file_ptr: *mut FcChar8 = ptr::null_mut();
            if (self.fc.FcPatternGetString)(matched, FC_FILE.as_ptr() as *const i8, 0, &mut file_ptr) != FcResultMatch {
                (self.fc.FcPatternDestroy)(matched);
                return Err(SystemFontError::AccessDenied);
            }

            let path = CStr::from_ptr(file_ptr as *const i8).to_string_lossy().into_owned();

            // Get font index
            let mut index: i32 = 0;
            (self.fc.FcPatternGetInteger)(matched, FC_INDEX.as_ptr() as *const i8, 0, &mut index);

            (self.fc.FcPatternDestroy)(matched);

            let data = std::fs::read(&path).map_err(|e| {
                SystemFontError::ReadError(format!("{}: {}", path, e))
            })?;

            Ok(SystemFontData {
                data,
                index: index as u32,
                family_name: family.to_string(),
            })
        }
    }

    fn list_families(&self) -> Vec<String> {
        unsafe {
            let pattern = (self.fc.FcPatternCreate)();
            if pattern.is_null() {
                return Vec::new();
            }

            let os = (self.fc.FcObjectSetCreate)();
            if os.is_null() {
                (self.fc.FcPatternDestroy)(pattern);
                return Vec::new();
            }
            (self.fc.FcObjectSetAdd)(os, FC_FAMILY.as_ptr() as *const i8);

            let font_set = (self.fc.FcFontList)(self.config, pattern, os);
            (self.fc.FcPatternDestroy)(pattern);
            (self.fc.FcObjectSetDestroy)(os);

            if font_set.is_null() {
                return Vec::new();
            }

            let mut result = Vec::new();
            let mut seen = std::collections::HashSet::new();

            for i in 0..(*font_set).nfont {
                let font = *(*font_set).fonts.offset(i as isize);
                let mut family_ptr: *mut FcChar8 = ptr::null_mut();

                if (self.fc.FcPatternGetString)(font, FC_FAMILY.as_ptr() as *const i8, 0, &mut family_ptr) == FcResultMatch {
                    let family = CStr::from_ptr(family_ptr as *const i8).to_string_lossy().into_owned();
                    if seen.insert(family.clone()) {
                        result.push(family);
                    }
                }
            }

            (self.fc.FcFontSetDestroy)(font_set);
            result.sort();
            result
        }
    }
}

pub fn get_system_font_provider() -> Result<LinuxFontProvider, SystemFontError> {
    LinuxFontProvider::new()
}
