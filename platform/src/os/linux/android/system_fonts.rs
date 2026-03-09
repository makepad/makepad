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
    if family.eq_ignore_ascii_case("roboto") {
        out.push("/system/fonts/Roboto-Regular.ttf".into());
        out.push("/product/fonts/Roboto-Regular.ttf".into());
    } else if family.eq_ignore_ascii_case("roboto bold")
        || family.eq_ignore_ascii_case("roboto medium")
    {
        out.push("/system/fonts/Roboto-Bold.ttf".into());
        out.push("/product/fonts/Roboto-Bold.ttf".into());
        out.push("/system/fonts/Roboto-Medium.ttf".into());
        out.push("/product/fonts/Roboto-Medium.ttf".into());
    } else if family.eq_ignore_ascii_case("roboto italic") {
        out.push("/system/fonts/Roboto-Italic.ttf".into());
        out.push("/product/fonts/Roboto-Italic.ttf".into());
    } else if family.eq_ignore_ascii_case("roboto bold italic") {
        out.push("/system/fonts/Roboto-BoldItalic.ttf".into());
        out.push("/product/fonts/Roboto-BoldItalic.ttf".into());
    } else if family.eq_ignore_ascii_case("noto sans") {
        out.push("/system/fonts/NotoSans-Regular.ttf".into());
        out.push("/product/fonts/NotoSans-Regular.ttf".into());
    } else if family.eq_ignore_ascii_case("noto sans bold") {
        out.push("/system/fonts/NotoSans-Bold.ttf".into());
        out.push("/product/fonts/NotoSans-Bold.ttf".into());
    } else if family.eq_ignore_ascii_case("noto sans italic") {
        out.push("/system/fonts/NotoSans-Italic.ttf".into());
        out.push("/product/fonts/NotoSans-Italic.ttf".into());
    } else if family.eq_ignore_ascii_case("noto sans cjk sc")
        || family.eq_ignore_ascii_case("noto sans cjk")
        || family.eq_ignore_ascii_case("noto sans sc")
        || family.eq_ignore_ascii_case("droid sans fallback")
    {
        out.push("/system/fonts/NotoSansCJK-Regular.ttc".into());
        out.push("/product/fonts/NotoSansCJK-Regular.ttc".into());
        out.push("/system/fonts/NotoSansSC-Regular.otf".into());
        out.push("/product/fonts/NotoSansSC-Regular.otf".into());
        out.push("/system/fonts/DroidSansFallback.ttf".into());
    } else if family.eq_ignore_ascii_case("noto sans cjk sc bold")
        || family.eq_ignore_ascii_case("noto sans sc bold")
    {
        out.push("/system/fonts/NotoSansCJK-Bold.ttc".into());
        out.push("/product/fonts/NotoSansCJK-Bold.ttc".into());
        out.push("/system/fonts/NotoSansSC-Bold.otf".into());
        out.push("/product/fonts/NotoSansSC-Bold.otf".into());
        out.push("/system/fonts/NotoSansCJK-Regular.ttc".into());
        out.push("/product/fonts/NotoSansCJK-Regular.ttc".into());
        out.push("/system/fonts/NotoSansSC-Regular.otf".into());
        out.push("/product/fonts/NotoSansSC-Regular.otf".into());
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

#[cfg(test)]
mod tests {
    use super::font_candidates;

    #[test]
    fn has_style_specific_candidates() {
        for family in [
            "Roboto",
            "Roboto Bold",
            "Roboto Italic",
            "Roboto Bold Italic",
            "Noto Sans CJK SC",
            "Noto Sans CJK SC Bold",
            "Noto Color Emoji",
        ] {
            assert!(
                !font_candidates(family).is_empty(),
                "expected candidates for {family}"
            );
        }
    }
}
