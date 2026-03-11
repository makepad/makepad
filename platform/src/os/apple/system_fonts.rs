use crate::os::system_fonts::{
    query_first_readable_font, SystemFontData, SystemFontError, SystemFontProvider,
};

pub struct AppleSystemFontProvider;

impl SystemFontProvider for AppleSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        query_first_readable_font(font_candidates(family))
    }
}

fn font_candidates(family: &str) -> &'static [&'static str] {
    if family.eq_ignore_ascii_case("apple color emoji") {
        &["/System/Library/Fonts/Apple Color Emoji.ttc"]
    } else if family.eq_ignore_ascii_case(".sf ns text")
        || family.eq_ignore_ascii_case("sf pro text")
    {
        &[
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            "/System/Library/Fonts/SFNSDisplay.ttf",
        ]
    } else if family.eq_ignore_ascii_case(".sf ns text bold")
        || family.eq_ignore_ascii_case("sf pro text bold")
    {
        &[
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNSBold.ttf",
            "/System/Library/Fonts/SFNSDisplay-Bold.ttf",
        ]
    } else if family.eq_ignore_ascii_case(".sf ns text italic")
        || family.eq_ignore_ascii_case("sf pro text italic")
    {
        &[
            "/System/Library/Fonts/Supplemental/Arial Italic.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNSItalic.ttf",
            "/System/Library/Fonts/SFNSDisplay-Italic.ttf",
        ]
    } else if family.eq_ignore_ascii_case(".sf ns text bold italic")
        || family.eq_ignore_ascii_case("sf pro text bold italic")
    {
        &[
            "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNSBoldItalic.ttf",
            "/System/Library/Fonts/SFNSDisplay-BoldItalic.ttf",
        ]
    } else if family.eq_ignore_ascii_case("helvetica neue")
        || family.eq_ignore_ascii_case("helvetica neue bold")
        || family.eq_ignore_ascii_case("helvetica neue italic")
        || family.eq_ignore_ascii_case("helvetica neue bold italic")
    {
        &["/System/Library/Fonts/Helvetica.ttc"]
    } else if family.eq_ignore_ascii_case("arial") {
        &["/System/Library/Fonts/Supplemental/Arial.ttf"]
    } else if family.eq_ignore_ascii_case("arial bold") {
        &["/System/Library/Fonts/Supplemental/Arial Bold.ttf"]
    } else if family.eq_ignore_ascii_case("arial italic") {
        &["/System/Library/Fonts/Supplemental/Arial Italic.ttf"]
    } else if family.eq_ignore_ascii_case("arial bold italic") {
        &["/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf"]
    } else if family.eq_ignore_ascii_case("pingfang sc") || family.eq_ignore_ascii_case("stheiti") {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
        ]
    } else if family.eq_ignore_ascii_case("pingfang sc bold")
        || family.eq_ignore_ascii_case("stheiti medium")
    {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
        ]
    } else if family.eq_ignore_ascii_case("hiragino sans gb")
        || family.eq_ignore_ascii_case("hiragino sans gb w6")
    {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
        ]
    } else {
        &[]
    }
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
