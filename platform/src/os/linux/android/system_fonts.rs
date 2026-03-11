use crate::os::system_fonts::{
    query_first_readable_font, SystemFontData, SystemFontError, SystemFontProvider,
};

pub struct AndroidSystemFontProvider;

impl SystemFontProvider for AndroidSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        query_first_readable_font(font_candidates(family))
    }
}

fn font_candidates(family: &str) -> &'static [&'static str] {
    if family.eq_ignore_ascii_case("roboto") {
        &["/system/fonts/Roboto-Regular.ttf", "/product/fonts/Roboto-Regular.ttf"]
    } else if family.eq_ignore_ascii_case("roboto bold")
        || family.eq_ignore_ascii_case("roboto medium")
    {
        &[
            "/system/fonts/Roboto-Bold.ttf",
            "/product/fonts/Roboto-Bold.ttf",
            "/system/fonts/Roboto-Medium.ttf",
            "/product/fonts/Roboto-Medium.ttf",
        ]
    } else if family.eq_ignore_ascii_case("roboto italic") {
        &["/system/fonts/Roboto-Italic.ttf", "/product/fonts/Roboto-Italic.ttf"]
    } else if family.eq_ignore_ascii_case("roboto bold italic") {
        &[
            "/system/fonts/Roboto-BoldItalic.ttf",
            "/product/fonts/Roboto-BoldItalic.ttf",
        ]
    } else if family.eq_ignore_ascii_case("noto sans") {
        &["/system/fonts/NotoSans-Regular.ttf", "/product/fonts/NotoSans-Regular.ttf"]
    } else if family.eq_ignore_ascii_case("noto sans bold") {
        &["/system/fonts/NotoSans-Bold.ttf", "/product/fonts/NotoSans-Bold.ttf"]
    } else if family.eq_ignore_ascii_case("noto sans italic") {
        &["/system/fonts/NotoSans-Italic.ttf", "/product/fonts/NotoSans-Italic.ttf"]
    } else if family.eq_ignore_ascii_case("noto sans cjk sc")
        || family.eq_ignore_ascii_case("noto sans cjk")
        || family.eq_ignore_ascii_case("noto sans sc")
        || family.eq_ignore_ascii_case("droid sans fallback")
    {
        &[
            "/system/fonts/NotoSansCJK-Regular.ttc",
            "/product/fonts/NotoSansCJK-Regular.ttc",
            "/system/fonts/NotoSansSC-Regular.otf",
            "/product/fonts/NotoSansSC-Regular.otf",
            "/system/fonts/DroidSansFallback.ttf",
        ]
    } else if family.eq_ignore_ascii_case("noto sans cjk sc bold")
        || family.eq_ignore_ascii_case("noto sans sc bold")
    {
        &[
            "/system/fonts/NotoSansCJK-Bold.ttc",
            "/product/fonts/NotoSansCJK-Bold.ttc",
            "/system/fonts/NotoSansSC-Bold.otf",
            "/product/fonts/NotoSansSC-Bold.otf",
            "/system/fonts/NotoSansCJK-Regular.ttc",
            "/product/fonts/NotoSansCJK-Regular.ttc",
            "/system/fonts/NotoSansSC-Regular.otf",
            "/product/fonts/NotoSansSC-Regular.otf",
        ]
    } else if family.eq_ignore_ascii_case("noto color emoji")
        || family.eq_ignore_ascii_case("emoji")
    {
        &[
            "/system/fonts/NotoColorEmoji.ttf",
            "/product/fonts/NotoColorEmoji.ttf",
            "/system/fonts/SamsungColorEmoji.ttf",
        ]
    } else {
        &[]
    }
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
