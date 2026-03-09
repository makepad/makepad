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
    if family.eq_ignore_ascii_case("segoe ui") {
        out.push("C:\\Windows\\Fonts\\segoeui.ttf".into());
    } else if family.eq_ignore_ascii_case("segoe ui bold")
        || family.eq_ignore_ascii_case("segoe ui semibold")
    {
        out.push("C:\\Windows\\Fonts\\segoeuib.ttf".into());
        out.push("C:\\Windows\\Fonts\\seguisb.ttf".into());
    } else if family.eq_ignore_ascii_case("segoe ui italic") {
        out.push("C:\\Windows\\Fonts\\segoeuii.ttf".into());
    } else if family.eq_ignore_ascii_case("segoe ui bold italic")
        || family.eq_ignore_ascii_case("segoe ui semibold italic")
    {
        out.push("C:\\Windows\\Fonts\\segoeuiz.ttf".into());
        out.push("C:\\Windows\\Fonts\\seguisbi.ttf".into());
    } else if family.eq_ignore_ascii_case("arial") {
        out.push("C:\\Windows\\Fonts\\arial.ttf".into());
    } else if family.eq_ignore_ascii_case("arial bold") {
        out.push("C:\\Windows\\Fonts\\arialbd.ttf".into());
    } else if family.eq_ignore_ascii_case("arial italic") {
        out.push("C:\\Windows\\Fonts\\ariali.ttf".into());
    } else if family.eq_ignore_ascii_case("arial bold italic") {
        out.push("C:\\Windows\\Fonts\\arialbi.ttf".into());
    } else if family.eq_ignore_ascii_case("microsoft yahei")
        || family.eq_ignore_ascii_case("microsoft jhenghei")
    {
        out.push("C:\\Windows\\Fonts\\msyh.ttc".into());
        out.push("C:\\Windows\\Fonts\\msjh.ttc".into());
    } else if family.eq_ignore_ascii_case("microsoft yahei bold")
        || family.eq_ignore_ascii_case("microsoft jhenghei bold")
    {
        out.push("C:\\Windows\\Fonts\\msyhbd.ttc".into());
        out.push("C:\\Windows\\Fonts\\msjhbd.ttc".into());
    } else if family.eq_ignore_ascii_case("simsun") {
        out.push("C:\\Windows\\Fonts\\simsun.ttc".into());
    } else if family.eq_ignore_ascii_case("segoe ui emoji") {
        out.push("C:\\Windows\\Fonts\\seguiemj.ttf".into());
    }
    out
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
