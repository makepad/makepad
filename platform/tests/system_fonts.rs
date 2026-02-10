//! Integration tests for system font providers.

#[cfg(all(test, feature = "system-fonts"))]
mod tests {
    use makepad_platform::os::system_fonts::SystemFontProvider;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_macos_query_font() {
        let provider = makepad_platform::os::get_system_font_provider();
        let result = provider.query_font("Helvetica");
        assert!(result.is_ok(), "Should find Helvetica");
        assert!(!result.unwrap().data.is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_macos_list_families() {
        let provider = makepad_platform::os::get_system_font_provider();
        let families = provider.list_families();
        assert!(!families.is_empty());
        assert!(families.iter().any(|f| f.contains("Helvetica")));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_macos_cjk_font() {
        // STHeiti is used for CJK fallback (PingFang SC is a stub font without outlines)
        let provider = makepad_platform::os::get_system_font_provider();
        let result = provider.query_font("STHeiti");
        assert!(result.is_ok(), "STHeiti should be available for CJK");
        assert!(!result.unwrap().data.is_empty());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn test_windows_query_font() {
        let provider = makepad_platform::os::get_system_font_provider();
        let result = provider.query_font("Arial");
        assert!(result.is_ok(), "Should find Arial");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_linux_query_font() {
        if let Ok(provider) = makepad_platform::os::get_system_font_provider() {
            // Try common fonts
            let fonts = ["DejaVu Sans", "Liberation Sans", "Noto Sans"];
            assert!(fonts.iter().any(|f| provider.query_font(f).is_ok()));
        }
    }
}
