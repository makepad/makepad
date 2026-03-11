use crate::os::system_fonts::{
    query_first_readable_font, SystemFontData, SystemFontError, SystemFontProvider,
};

pub struct WindowsSystemFontProvider;

impl SystemFontProvider for WindowsSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        query_first_readable_font(font_candidates(family))
    }
}

fn font_candidates(family: &str) -> &'static [&'static str] {
    if family.eq_ignore_ascii_case("segoe ui") {
        &["C:\\Windows\\Fonts\\segoeui.ttf"]
    } else if family.eq_ignore_ascii_case("segoe ui bold")
        || family.eq_ignore_ascii_case("segoe ui semibold")
    {
        &["C:\\Windows\\Fonts\\segoeuib.ttf", "C:\\Windows\\Fonts\\seguisb.ttf"]
    } else if family.eq_ignore_ascii_case("segoe ui italic") {
        &["C:\\Windows\\Fonts\\segoeuii.ttf"]
    } else if family.eq_ignore_ascii_case("segoe ui bold italic")
        || family.eq_ignore_ascii_case("segoe ui semibold italic")
    {
        &["C:\\Windows\\Fonts\\segoeuiz.ttf", "C:\\Windows\\Fonts\\seguisbi.ttf"]
    } else if family.eq_ignore_ascii_case("arial") {
        &["C:\\Windows\\Fonts\\arial.ttf"]
    } else if family.eq_ignore_ascii_case("arial bold") {
        &["C:\\Windows\\Fonts\\arialbd.ttf"]
    } else if family.eq_ignore_ascii_case("arial italic") {
        &["C:\\Windows\\Fonts\\ariali.ttf"]
    } else if family.eq_ignore_ascii_case("arial bold italic") {
        &["C:\\Windows\\Fonts\\arialbi.ttf"]
    } else if family.eq_ignore_ascii_case("microsoft yahei")
        || family.eq_ignore_ascii_case("microsoft jhenghei")
    {
        &["C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\msjh.ttc"]
    } else if family.eq_ignore_ascii_case("microsoft yahei bold")
        || family.eq_ignore_ascii_case("microsoft jhenghei bold")
    {
        &["C:\\Windows\\Fonts\\msyhbd.ttc", "C:\\Windows\\Fonts\\msjhbd.ttc"]
    } else if family.eq_ignore_ascii_case("simsun") {
        &["C:\\Windows\\Fonts\\simsun.ttc"]
    } else if family.eq_ignore_ascii_case("segoe ui emoji") {
        &["C:\\Windows\\Fonts\\seguiemj.ttf"]
    } else {
        &[]
    }
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    WindowsSystemFontProvider.query_font(family)
}

#[cfg(test)]
mod tests {
    use super::font_candidates;

    #[test]
    fn has_style_specific_candidates() {
        for family in [
            "Segoe UI",
            "Segoe UI Bold",
            "Segoe UI Italic",
            "Segoe UI Bold Italic",
            "Microsoft YaHei",
            "Microsoft YaHei Bold",
            "Segoe UI Emoji",
        ] {
            assert!(
                !font_candidates(family).is_empty(),
                "expected candidates for {family}"
            );
        }
    }
}
