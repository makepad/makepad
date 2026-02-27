use crate::{
    os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider},
    shared_bytes::SharedBytes,
};
use std::process::Command;

pub struct LinuxSystemFontProvider;

impl SystemFontProvider for LinuxSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        let output = Command::new("fc-match")
            .args(["-f", "%{file}\n%{index}\n", family])
            .output()
            .map_err(|err| {
                SystemFontError::Io(format!(
                    "failed to execute fc-match (fontconfig must be installed on the system; try `apt install fontconfig` or `dnf install fontconfig`): {err}"
                ))
            })?;
        if !output.status.success() {
            return Err(SystemFontError::NotFound);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let path = lines.next().unwrap_or("").trim().to_string();
        if path.is_empty() {
            return Err(SystemFontError::NotFound);
        }
        let index_line = lines.next().unwrap_or("").trim();
        let index = index_line.parse::<u32>().map_err(|err| {
            SystemFontError::Io(format!(
                "failed to parse fc-match face index '{index_line}': {err}"
            ))
        })?;
        let data = SharedBytes::from_file_mmap_or_read(&path)
            .map_err(|err| SystemFontError::Io(err.to_string()))?;
        Ok(SystemFontData { data, index })
    }
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    LinuxSystemFontProvider.query_font(family)
}
