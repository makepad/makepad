use crate::shared_bytes::SharedBytes;

#[derive(Clone, Debug)]
pub struct SystemFontData {
    pub data: SharedBytes,
    pub index: u32,
}

#[derive(Clone, Debug)]
pub enum SystemFontError {
    NotFound,
    Io(String),
    Unsupported,
}

pub trait SystemFontProvider: Send + Sync {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinThemeFontRole {
    SansRegular,
    SansBold,
    SansItalic,
    SansBoldItalic,
    CjkRegular,
    CjkBold,
    Emoji,
}

/// Returns the bundled widgets-theme role for known text font resource paths.
///
/// This matcher only targets built-in widgets text fonts and ignores app-specific
/// or non-text resources.
pub fn builtin_theme_font_role_for_resource_path(path: &str) -> Option<BuiltinThemeFontRole> {
    if !is_widgets_theme_resource_path(path) {
        return None;
    }

    let basename = resource_basename(path)?;
    match basename.to_ascii_lowercase().as_str() {
        "ibmplexsans-text.ttf" => Some(BuiltinThemeFontRole::SansRegular),
        "ibmplexsans-semibold.ttf" => Some(BuiltinThemeFontRole::SansBold),
        "ibmplexsans-italic.ttf" => Some(BuiltinThemeFontRole::SansItalic),
        "ibmplexsans-bolditalic.ttf" => Some(BuiltinThemeFontRole::SansBoldItalic),
        "lxgwwenkairegular.ttf" => Some(BuiltinThemeFontRole::CjkRegular),
        "lxgwwenkaibold.ttf" => Some(BuiltinThemeFontRole::CjkBold),
        "notocoloremoji.ttf" => Some(BuiltinThemeFontRole::Emoji),
        _ => None,
    }
}

pub fn is_builtin_theme_bundled_text_font_path(path: &str) -> bool {
    builtin_theme_font_role_for_resource_path(path).is_some()
}

fn is_widgets_theme_resource_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/widgets/resources/") || lower.contains("\\widgets\\resources\\")
}

fn resource_basename(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\']).next()
}

/// Query the operating system for a font by family name and return raw font data.
///
/// Supported on macOS/iOS/tvOS, Windows, Linux desktop, and Android. Other targets return
/// `SystemFontError::Unsupported`.
#[cfg(all(
    not(headless),
    any(target_os = "macos", target_os = "ios", target_os = "tvos")
))]
pub fn query_system_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    crate::os::apple::system_fonts::query_font(family)
}

#[cfg(all(not(headless), target_os = "windows"))]
pub fn query_system_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    crate::os::windows::system_fonts::query_font(family)
}

#[cfg(all(not(headless), target_os = "linux", not(target_env = "ohos")))]
pub fn query_system_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    crate::os::linux::system_fonts::query_font(family)
}

#[cfg(all(not(headless), target_os = "android"))]
pub fn query_system_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    crate::os::linux::android::system_fonts::query_font(family)
}

#[cfg(any(headless, target_arch = "wasm32", target_env = "ohos"))]
pub fn query_system_font(_family: &str) -> Result<SystemFontData, SystemFontError> {
    Err(SystemFontError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_theme_font_role_for_resource_path, is_builtin_theme_bundled_text_font_path,
        BuiltinThemeFontRole,
    };

    #[test]
    fn maps_bundled_theme_resources_to_roles() {
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/widgets/resources/IBMPlexSans-Text.ttf"),
            Some(BuiltinThemeFontRole::SansRegular)
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path(
                "/tmp/widgets/resources/IBMPlexSans-SemiBold.ttf"
            ),
            Some(BuiltinThemeFontRole::SansBold)
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/widgets/resources/IBMPlexSans-Italic.ttf"),
            Some(BuiltinThemeFontRole::SansItalic)
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path(
                "/tmp/widgets/resources/IBMPlexSans-BoldItalic.ttf"
            ),
            Some(BuiltinThemeFontRole::SansBoldItalic)
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/widgets/resources/LXGWWenKaiRegular.ttf"),
            Some(BuiltinThemeFontRole::CjkRegular)
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path(
                "C:\\tmp\\widgets\\resources\\LXGWWenKaiBold.ttf"
            ),
            Some(BuiltinThemeFontRole::CjkBold)
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/widgets/resources/NotoColorEmoji.ttf"),
            Some(BuiltinThemeFontRole::Emoji)
        );
    }

    #[test]
    fn ignores_non_theme_or_non_text_resources() {
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/widgets/resources/LiberationMono-Regular.ttf"),
            None
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/widgets/resources/fa-solid-900.ttf"),
            None
        );
        assert_eq!(
            builtin_theme_font_role_for_resource_path("/tmp/app/resources/IBMPlexSans-Text.ttf"),
            None
        );
    }

    #[test]
    fn bundled_text_path_boolean_matches_role_matcher() {
        let yes = "/tmp/widgets/resources/IBMPlexSans-Text.ttf";
        let no = "/tmp/widgets/resources/LiberationMono-Regular.ttf";
        assert_eq!(
            is_builtin_theme_bundled_text_font_path(yes),
            builtin_theme_font_role_for_resource_path(yes).is_some()
        );
        assert_eq!(
            is_builtin_theme_bundled_text_font_path(no),
            builtin_theme_font_role_for_resource_path(no).is_some()
        );
    }
}
