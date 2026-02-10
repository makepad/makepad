use super::loader::{FontDefinition, FontFamilyDefinition, Loader};

#[cfg(feature = "bundled-fonts")]
pub const IBM_PLEX_SANS_TEXT: &[u8] =
    include_bytes!("../../../widgets/resources/IBMPlexSans-Text.ttf");
#[cfg(feature = "bundled-fonts")]
pub const LXG_WEN_KAI_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LXGWWenKaiRegular.ttf");
#[cfg(feature = "bundled-fonts")]
pub const NOTO_COLOR_EMOJI: &[u8] = include_bytes!("../../../widgets/resources/NotoColorEmoji.ttf");
#[cfg(feature = "bundled-fonts")]
pub const LIBERATION_MONO_REGULAR: &[u8] =
    include_bytes!("../../../widgets/resources/LiberationMono-Regular.ttf");

#[cfg(feature = "bundled-fonts")]
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
        FontDefinition::from_data(IBM_PLEX_SANS_TEXT.to_vec().into(), 0),
    );
    loader.define_font(
        "LXG WWen Kai Regular".into(),
        FontDefinition::from_data(LXG_WEN_KAI_REGULAR.to_vec().into(), 0),
    );
    loader.define_font(
        "Noto Color Emoji".into(),
        FontDefinition::from_data(NOTO_COLOR_EMOJI.to_vec().into(), 0),
    );
    loader.define_font(
        "Liberation Mono Regular".into(),
        FontDefinition::from_data(LIBERATION_MONO_REGULAR.to_vec().into(), 0),
    );
}

#[cfg(all(feature = "system-fonts", not(feature = "bundled-fonts")))]
pub fn define(loader: &mut Loader) {
    // Platform-specific font names
    // Note: PingFang SC on macOS is a stub font without glyph outlines, use STHeiti instead
    #[cfg(target_os = "macos")]
    const FONTS: (&str, &str, &str) = ("Helvetica Neue", "STHeiti", "Menlo");
    #[cfg(target_os = "windows")]
    const FONTS: (&str, &str, &str) = ("Segoe UI", "Microsoft YaHei", "Consolas");
    #[cfg(target_os = "linux")]
    const FONTS: (&str, &str, &str) = ("DejaVu Sans", "Noto Sans CJK SC", "DejaVu Sans Mono");

    let (sans_font, cjk_font, mono_font) = FONTS;

    loader.define_font_family(
        "Sans".into(),
        FontFamilyDefinition {
            font_ids: ["System Sans".into(), "System CJK".into()].into(),
        },
    );
    loader.define_font_family(
        "Monospace".into(),
        FontFamilyDefinition {
            font_ids: ["System Mono".into()].into(),
        },
    );
    loader.define_font("System Sans".into(), FontDefinition::from_system(sans_font));
    loader.define_font("System CJK".into(), FontDefinition::from_system(cjk_font));
    loader.define_font("System Mono".into(), FontDefinition::from_system(mono_font));
}

#[cfg(not(any(feature = "bundled-fonts", feature = "system-fonts")))]
pub fn define(_loader: &mut Loader) {
    // No fonts defined - user must define their own
}
