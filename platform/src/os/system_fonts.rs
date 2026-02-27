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

#[cfg(any(
    headless,
    target_os = "android",
    target_arch = "wasm32",
    target_env = "ohos",
))]
pub fn query_system_font(_family: &str) -> Result<SystemFontData, SystemFontError> {
    Err(SystemFontError::Unsupported)
}
