use crate::{
    os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider},
    shared_bytes::SharedBytes,
};
use std::path::PathBuf;

pub struct AppleSystemFontProvider;

impl SystemFontProvider for AppleSystemFontProvider {
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
    if family.eq_ignore_ascii_case("apple color emoji") {
        out.push("/System/Library/Fonts/Apple Color Emoji.ttc".into());
    } else if family.eq_ignore_ascii_case(".sf ns text")
        || family.eq_ignore_ascii_case("sf pro text")
    {
        out.push("/System/Library/Fonts/Supplemental/Arial.ttf".into());
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
        out.push("/System/Library/Fonts/SFNS.ttf".into());
        out.push("/System/Library/Fonts/SFNSDisplay.ttf".into());
    } else if family.eq_ignore_ascii_case(".sf ns text bold")
        || family.eq_ignore_ascii_case("sf pro text bold")
    {
        out.push("/System/Library/Fonts/Supplemental/Arial Bold.ttf".into());
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
        out.push("/System/Library/Fonts/SFNSBold.ttf".into());
        out.push("/System/Library/Fonts/SFNSDisplay-Bold.ttf".into());
    } else if family.eq_ignore_ascii_case(".sf ns text italic")
        || family.eq_ignore_ascii_case("sf pro text italic")
    {
        out.push("/System/Library/Fonts/Supplemental/Arial Italic.ttf".into());
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
        out.push("/System/Library/Fonts/SFNSItalic.ttf".into());
        out.push("/System/Library/Fonts/SFNSDisplay-Italic.ttf".into());
    } else if family.eq_ignore_ascii_case(".sf ns text bold italic")
        || family.eq_ignore_ascii_case("sf pro text bold italic")
    {
        out.push("/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf".into());
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
        out.push("/System/Library/Fonts/SFNSBoldItalic.ttf".into());
        out.push("/System/Library/Fonts/SFNSDisplay-BoldItalic.ttf".into());
    } else if family.eq_ignore_ascii_case("helvetica neue") {
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
    } else if family.eq_ignore_ascii_case("helvetica neue bold") {
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
    } else if family.eq_ignore_ascii_case("helvetica neue italic") {
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
    } else if family.eq_ignore_ascii_case("helvetica neue bold italic") {
        out.push("/System/Library/Fonts/Helvetica.ttc".into());
    } else if family.eq_ignore_ascii_case("arial") {
        out.push("/System/Library/Fonts/Supplemental/Arial.ttf".into());
    } else if family.eq_ignore_ascii_case("arial bold") {
        out.push("/System/Library/Fonts/Supplemental/Arial Bold.ttf".into());
    } else if family.eq_ignore_ascii_case("arial italic") {
        out.push("/System/Library/Fonts/Supplemental/Arial Italic.ttf".into());
    } else if family.eq_ignore_ascii_case("arial bold italic") {
        out.push("/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf".into());
    } else if family.eq_ignore_ascii_case("pingfang sc") || family.eq_ignore_ascii_case("stheiti") {
        out.push("/System/Library/Fonts/PingFang.ttc".into());
        out.push("/System/Library/Fonts/STHeiti Light.ttc".into());
        out.push("/System/Library/Fonts/STHeiti Medium.ttc".into());
    } else if family.eq_ignore_ascii_case("pingfang sc bold")
        || family.eq_ignore_ascii_case("stheiti medium")
    {
        out.push("/System/Library/Fonts/PingFang.ttc".into());
        out.push("/System/Library/Fonts/STHeiti Medium.ttc".into());
    } else if family.eq_ignore_ascii_case("hiragino sans gb")
        || family.eq_ignore_ascii_case("hiragino sans gb w6")
    {
        out.push("/System/Library/Fonts/PingFang.ttc".into());
        out.push("/System/Library/Fonts/STHeiti Medium.ttc".into());
    }
    out
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    AppleSystemFontProvider.query_font(family)
}

#[cfg(test)]
mod tests {
    use super::font_candidates;

    #[test]
    fn has_style_specific_candidates() {
        for family in [
            ".SF NS Text",
            ".SF NS Text Bold",
            ".SF NS Text Italic",
            ".SF NS Text Bold Italic",
            "PingFang SC",
            "PingFang SC Bold",
            "Apple Color Emoji",
        ] {
            assert!(
                !font_candidates(family).is_empty(),
                "expected candidates for {family}"
            );
        }
    }
}
