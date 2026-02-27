use crate::os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider};
use std::path::PathBuf;

pub struct AppleSystemFontProvider;

impl SystemFontProvider for AppleSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        for candidate in font_candidates(family) {
            if let Ok(data) = std::fs::read(&candidate) {
                return Ok(SystemFontData { data, index: 0 });
            }
        }
        Err(SystemFontError::NotFound)
    }
}

fn font_candidates(family: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if family.eq_ignore_ascii_case("apple color emoji") {
        out.push("/System/Library/Fonts/Apple Color Emoji.ttc".into());
    } else if family.eq_ignore_ascii_case("stheiti") {
        out.push("/System/Library/Fonts/STHeiti Light.ttc".into());
        out.push("/System/Library/Fonts/STHeiti Medium.ttc".into());
    } else if family.eq_ignore_ascii_case("pingfang sc") {
        out.push("/System/Library/Fonts/PingFang.ttc".into());
    }
    out
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    AppleSystemFontProvider.query_font(family)
}
