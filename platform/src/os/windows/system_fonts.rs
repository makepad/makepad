//! Windows system font provider using DirectWrite.

use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteFontFace,
    IDWriteFontFile, IDWriteFontFileLoader, IDWriteLocalFontFileLoader,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL,
};

use crate::os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider};

/// Windows system font provider using DirectWrite.
pub struct WindowsFontProvider {
    factory: IDWriteFactory,
}

impl WindowsFontProvider {
    /// Create a new Windows font provider.
    pub fn new() -> Result<Self, SystemFontError> {
        unsafe {
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|_| SystemFontError::PlatformNotSupported)?;
            Ok(Self { factory })
        }
    }

    /// Get the font file path from a font face.
    fn get_font_file_path(&self, font_face: &IDWriteFontFace) -> Result<PathBuf, SystemFontError> {
        unsafe {
            // Get the number of font files
            let mut file_count = 0u32;
            font_face
                .GetFiles(&mut file_count, None)
                .map_err(|_| SystemFontError::AccessDenied)?;

            if file_count == 0 {
                return Err(SystemFontError::AccessDenied);
            }

            // Get the font files
            let mut files: Vec<Option<IDWriteFontFile>> = vec![None; file_count as usize];
            font_face
                .GetFiles(&mut file_count, Some(files.as_mut_ptr()))
                .map_err(|_| SystemFontError::AccessDenied)?;

            let file = files
                .into_iter()
                .next()
                .flatten()
                .ok_or(SystemFontError::AccessDenied)?;

            // Get the font file loader and reference key
            let mut loader: Option<IDWriteFontFileLoader> = None;
            let mut key_ptr: *const std::ffi::c_void = std::ptr::null();
            let mut key_size = 0u32;

            file.GetReferenceKey(&mut key_ptr, &mut key_size)
                .map_err(|_| SystemFontError::AccessDenied)?;
            file.GetLoader(&mut loader)
                .map_err(|_| SystemFontError::AccessDenied)?;

            let loader = loader.ok_or(SystemFontError::AccessDenied)?;

            // Try to cast to local font file loader to get the file path
            if let Ok(local_loader) = loader.cast::<IDWriteLocalFontFileLoader>() {
                let mut path_len = 0u32;
                local_loader
                    .GetFilePathLengthFromKey(key_ptr, key_size, &mut path_len)
                    .map_err(|_| SystemFontError::AccessDenied)?;

                let mut path_buf: Vec<u16> = vec![0; (path_len + 1) as usize];
                local_loader
                    .GetFilePathFromKey(key_ptr, key_size, &mut path_buf)
                    .map_err(|_| SystemFontError::AccessDenied)?;

                let path = String::from_utf16_lossy(&path_buf[..path_len as usize]);
                return Ok(PathBuf::from(path));
            }

            Err(SystemFontError::AccessDenied)
        }
    }
}

impl Default for WindowsFontProvider {
    fn default() -> Self {
        Self::new().expect("Failed to create DirectWrite factory")
    }
}

impl SystemFontProvider for WindowsFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        unsafe {
            let mut collection: Option<IDWriteFontCollection> = None;
            self.factory
                .GetSystemFontCollection(&mut collection, false)
                .map_err(|_| SystemFontError::PlatformNotSupported)?;
            let collection = collection.ok_or(SystemFontError::PlatformNotSupported)?;

            let family_wide: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
            let mut index = 0u32;
            let mut exists = false.into();
            collection
                .FindFamilyName(PCWSTR(family_wide.as_ptr()), &mut index, &mut exists)
                .map_err(|_| SystemFontError::FontNotFound(family.to_string()))?;

            if !exists.as_bool() {
                return Err(SystemFontError::FontNotFound(family.to_string()));
            }

            let font_family = collection
                .GetFontFamily(index)
                .map_err(|_| SystemFontError::FontNotFound(family.to_string()))?;

            let font = font_family
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .map_err(|_| SystemFontError::FontNotFound(family.to_string()))?;

            let font_face = font
                .CreateFontFace()
                .map_err(|_| SystemFontError::AccessDenied)?;

            let path = self.get_font_file_path(&font_face)?;

            let data = std::fs::read(&path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    SystemFontError::AccessDenied
                } else {
                    SystemFontError::ReadError(format!("{}: {}", path.display(), e))
                }
            })?;

            if data.is_empty() {
                return Err(SystemFontError::ReadError("Empty font file".to_string()));
            }

            Ok(SystemFontData {
                data,
                index: font_face.GetIndex(),
                family_name: family.to_string(),
            })
        }
    }

    fn list_families(&self) -> Vec<String> {
        unsafe {
            let mut collection: Option<IDWriteFontCollection> = None;
            if self
                .factory
                .GetSystemFontCollection(&mut collection, false)
                .is_err()
            {
                return Vec::new();
            }
            let Some(collection) = collection else {
                return Vec::new();
            };

            let count = collection.GetFontFamilyCount();
            let mut result = Vec::with_capacity(count as usize);

            for i in 0..count {
                if let Ok(family) = collection.GetFontFamily(i) {
                    if let Ok(names) = family.GetFamilyNames() {
                        let mut len = 0u32;
                        if names.GetStringLength(0, &mut len).is_ok() {
                            let mut name: Vec<u16> = vec![0; (len + 1) as usize];
                            if names.GetString(0, &mut name).is_ok() {
                                let s = String::from_utf16_lossy(&name[..len as usize]);
                                result.push(s);
                            }
                        }
                    }
                }
            }

            result
        }
    }
}

/// Get the system font provider for Windows
pub fn get_system_font_provider() -> WindowsFontProvider {
    WindowsFontProvider::new().expect("Failed to initialize DirectWrite")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_families() {
        let provider = WindowsFontProvider::new().expect("Failed to create provider");
        let families = provider.list_families();
        assert!(!families.is_empty());
    }

    #[test]
    fn test_query_arial() {
        let provider = WindowsFontProvider::new().expect("Failed to create provider");
        let result = provider.query_font("Arial");
        assert!(result.is_ok());
        let font_data = result.unwrap();
        assert!(!font_data.data.is_empty());
        assert_eq!(font_data.family_name, "Arial");
    }

    #[test]
    fn test_query_nonexistent_font() {
        let provider = WindowsFontProvider::new().expect("Failed to create provider");
        let result = provider.query_font("ThisFontDefinitelyDoesNotExist12345");
        assert!(result.is_err());
    }
}
