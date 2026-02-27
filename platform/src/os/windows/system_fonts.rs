use crate::{
    os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider},
    shared_bytes::SharedBytes,
};
use std::path::PathBuf;

pub struct WindowsSystemFontProvider;

impl SystemFontProvider for WindowsSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        for candidate in font_candidates(family) {
            if let Ok(data) = SharedBytes::from_file_mmap_or_read(&candidate) {
                return Ok(SystemFontData { data, index: 0 });
            }
        }
        Err(SystemFontError::NotFound)
    }
}

fn font_candidates(family: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if family.eq_ignore_ascii_case("microsoft yahei") {
        out.push("C:\\Windows\\Fonts\\msyh.ttc".into());
    } else if family.eq_ignore_ascii_case("segoe ui emoji") {
        out.push("C:\\Windows\\Fonts\\seguiemj.ttf".into());
    }
    out
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    WindowsSystemFontProvider.query_font(family)
}
