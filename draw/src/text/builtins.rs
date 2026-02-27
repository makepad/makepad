use super::loader::FontData;
use std::borrow::Cow;
use std::rc::Rc;

pub const IBM_PLEX_SANS_TEXT: &[u8] =
    include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
pub const LXG_WEN_KAI_REGULAR: &[u8] =
    include_bytes!("../../../widgets/fonts/LXGWWenKaiRegular.ttf");
pub const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../widgets/fonts/NotoColorEmoji.ttf");
pub const LIBERATION_MONO_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LiberationMono-Regular.ttf");

/// Returns static font data for a known builtin font, matched by filename
/// suffix of the resource's abs_path.
pub fn get_builtin_font_data(abs_path: &str) -> Option<FontData> {
    let filename = abs_path.rsplit('/').next().unwrap_or(abs_path);
    match filename {
        "IBMPlexSans-Text.ttf" => Some(Rc::new(Cow::Borrowed(IBM_PLEX_SANS_TEXT))),
        "LXGWWenKaiRegular.ttf" => Some(Rc::new(Cow::Borrowed(LXG_WEN_KAI_REGULAR))),
        "NotoColorEmoji.ttf" => Some(Rc::new(Cow::Borrowed(NOTO_COLOR_EMOJI))),
        "LiberationMono-Regular.ttf" => Some(Rc::new(Cow::Borrowed(LIBERATION_MONO_REGULAR))),
        _ => None,
    }
}
