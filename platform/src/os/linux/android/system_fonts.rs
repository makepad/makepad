use crate::{
    os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider},
    shared_bytes::SharedBytes,
};
use std::path::PathBuf;

pub struct AndroidSystemFontProvider;

impl SystemFontProvider for AndroidSystemFontProvider {
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
    if family.eq_ignore_ascii_case("noto sans cjk sc")
        || family.eq_ignore_ascii_case("noto sans cjk")
        || family.eq_ignore_ascii_case("droid sans fallback")
    {
        out.push("/system/fonts/NotoSansCJK-Regular.ttc".into());
        out.push("/product/fonts/NotoSansCJK-Regular.ttc".into());
        out.push("/system/fonts/NotoSansSC-Regular.otf".into());
        out.push("/product/fonts/NotoSansSC-Regular.otf".into());
        out.push("/system/fonts/DroidSansFallback.ttf".into());
    } else if family.eq_ignore_ascii_case("noto color emoji")
        || family.eq_ignore_ascii_case("emoji")
    {
        out.push("/system/fonts/NotoColorEmoji.ttf".into());
        out.push("/product/fonts/NotoColorEmoji.ttf".into());
        out.push("/system/fonts/SamsungColorEmoji.ttf".into());
    }
    out
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    AndroidSystemFontProvider.query_font(family)
}
