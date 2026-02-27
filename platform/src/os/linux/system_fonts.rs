use crate::os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider};
use std::process::Command;

pub struct LinuxSystemFontProvider;

impl SystemFontProvider for LinuxSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        let output = Command::new("fc-match")
            .args(["-f", "%{file}\n", family])
            .output()
            .map_err(|err| {
                SystemFontError::Io(format!(
                    "failed to execute fc-match (fontconfig must be installed on the system; try `apt install fontconfig` or `dnf install fontconfig`): {err}"
                ))
            })?;
        if !output.status.success() {
            return Err(SystemFontError::NotFound);
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            return Err(SystemFontError::NotFound);
        }
        let data = std::fs::read(&path).map_err(|err| SystemFontError::Io(err.to_string()))?;
        Ok(SystemFontData { data, index: 0 })
    }
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    LinuxSystemFontProvider.query_font(family)
}
