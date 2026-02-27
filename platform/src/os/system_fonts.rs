#[derive(Clone, Debug)]
pub struct SystemFontData {
    pub data: Vec<u8>,
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

/// Query the operating system for a font by family name and return raw font data.
///
/// Supported on macOS/iOS/tvOS, Windows, Linux desktop, and Android. Other targets return
/// `SystemFontError::Unsupported`.
#[cfg(all(not(headless), any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
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

#[cfg(any(
    headless,
    target_arch = "wasm32",
    target_env = "ohos",
))]
pub fn query_system_font(_family: &str) -> Result<SystemFontData, SystemFontError> {
    Err(SystemFontError::Unsupported)
}
