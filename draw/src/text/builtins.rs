use super::loader::FontData;
use std::borrow::Cow;
use std::rc::Rc;

/// These filenames are used for path matching even when bundled-fonts is disabled.
pub const LXG_WEN_KAI_REGULAR_FILENAME: &str = "LXGWWenKaiRegular.ttf";
pub const NOTO_COLOR_EMOJI_FILENAME: &str = "NotoColorEmoji.ttf";

pub const IBM_PLEX_SANS_TEXT: &[u8] =
    include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
#[cfg(feature = "bundled-fonts")]
pub const LXG_WEN_KAI_REGULAR: &[u8] =
    include_bytes!("../../../widgets/fonts/LXGWWenKaiRegular.ttf");
#[cfg(feature = "bundled-fonts")]
pub const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../widgets/fonts/NotoColorEmoji.ttf");
pub const LIBERATION_MONO_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LiberationMono-Regular.ttf");

/// Returns static font data for a known builtin font, matched by filename
/// suffix of the resource's abs_path.
pub fn get_builtin_font_data(abs_path: &str) -> Option<FontData> {
    let filename = abs_path.rsplit('/').next().unwrap_or(abs_path);
    match filename {
        "IBMPlexSans-Text.ttf" => Some(Rc::new(Cow::Borrowed(IBM_PLEX_SANS_TEXT))),
        #[cfg(feature = "bundled-fonts")]
        LXG_WEN_KAI_REGULAR_FILENAME => Some(Rc::new(Cow::Borrowed(LXG_WEN_KAI_REGULAR))),
        #[cfg(feature = "bundled-fonts")]
        NOTO_COLOR_EMOJI_FILENAME => Some(Rc::new(Cow::Borrowed(NOTO_COLOR_EMOJI))),
        "LiberationMono-Regular.ttf" => Some(Rc::new(Cow::Borrowed(LIBERATION_MONO_REGULAR))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::get_builtin_font_data;

    #[test]
    fn ibm_plex_builtin_is_available() {
        assert!(get_builtin_font_data("/tmp/IBMPlexSans-Text.ttf").is_some());
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn bundled_fallback_fonts_are_available() {
        assert!(get_builtin_font_data("/tmp/LXGWWenKaiRegular.ttf").is_some());
        assert!(get_builtin_font_data("/tmp/NotoColorEmoji.ttf").is_some());
    }

    #[cfg(not(feature = "bundled-fonts"))]
    #[test]
    fn bundled_fallback_fonts_are_disabled() {
        assert!(get_builtin_font_data("/tmp/LXGWWenKaiRegular.ttf").is_none());
        assert!(get_builtin_font_data("/tmp/NotoColorEmoji.ttf").is_none());
    }
}
