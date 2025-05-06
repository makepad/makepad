use super::loader::{FontDefinition, FontFamilyDefinition, Loader};

pub const IBM_PLEX_SANS_TEXT: &[u8] =
    include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
pub const LXG_WEN_KAI_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LXGWWenKaiRegular.ttf");
pub const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../widgets/resources/NotoColorEmoji.ttf");
pub const LIBERATION_MONO_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LiberationMono-Regular.ttf");

pub fn define(loader: &mut Loader) {
    loader.define_font_family(
        "Sans".into(),
        FontFamilyDefinition {
            font_ids: [
                "IBM Plex Sans Text".into(),
                "LXG WWen Kai Regular".into(),
                "Noto Color Emoji".into(),
            ]
            .into(),
        },
    );
    loader.define_font_family(
        "Monospace".into(),
        FontFamilyDefinition {
            font_ids: ["Liberation Mono Regular".into()].into(),
        },
    );
    loader.define_font(
        "IBM Plex Sans Text".into(),
        FontDefinition {
            data: IBM_PLEX_SANS_TEXT.to_vec().into(),
            index: 0,
        },
    );
    loader.define_font(
        "LXG WWen Kai Regular".into(),
        FontDefinition {
            data: LXG_WEN_KAI_REGULAR.to_vec().into(),
            index: 0,
        },
    );
    loader.define_font(
        "Noto Color Emoji".into(),
        FontDefinition {
            data: NOTO_COLOR_EMOJI.to_vec().into(),
            index: 0,
        },
    );
    loader.define_font(
        "Liberation Mono Regular".into(),
        FontDefinition {
            data: LIBERATION_MONO_REGULAR.to_vec().into(),
            index: 0,
        },
    );
}
