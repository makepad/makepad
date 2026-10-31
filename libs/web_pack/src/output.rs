//! An output folder for a packed app: only the pack's own files are
//! written there (its fixed names, and content-named files
//! `<stem>.<16 hex>.<ext>`), each written whole then renamed in, with its
//! brotli size measured; and only content-named files of an earlier pack
//! that this one did not write are removed.

use std::path::{Path, PathBuf};

/// A written file: name, bytes, brotli bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct SizedFile {
    pub name: String,
    pub bytes: usize,
    pub brotli: usize,
}

/// The output folder: only the export's own files are written there, and
/// only its own earlier files are removed.
pub struct Output {
    dir: PathBuf,
    fixed: Vec<String>,
    written: Vec<SizedFile>,
}

/// Whether `name` is a content-named file (`<stem>.<16 hex>.<ext>`), the
/// form the export gives the music and pictures.
pub fn is_content_named(name: &str) -> bool {
    let parts: Vec<&str> = name.rsplitn(3, '.').collect();
    parts.len() == 3 && parts[1].len() == 16 && parts[1].bytes().all(|b| b.is_ascii_hexdigit())
}

impl Output {
    /// An output folder whose fixed file names are `fixed` (`index.html`,
    /// `app.js`, ...).
    pub fn new(dir: &Path, fixed: &[&str]) -> Result<Output, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Output { dir: dir.to_path_buf(), fixed: fixed.iter().map(|s| s.to_string()).collect(), written: Vec::new() })
    }

    pub fn write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.write_sized(name, bytes, None)
    }

    /// [`Self::write`] with the brotli size already known (a file whose
    /// `.br` sibling the caller made).
    pub fn write_sized(&mut self, name: &str, bytes: &[u8], brotli: Option<usize>) -> Result<(), String> {
        if name.contains('/') || name.contains('\\') || name.starts_with('.') || !(self.fixed.iter().any(|f| f == name) || is_content_named(name)) {
            return Err(format!("refusing to write `{name}`: not an export file name"));
        }
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
        let brotli = match brotli {
            Some(n) => n,
            None if name.ends_with(".br") => bytes.len(),
            None => crate::brotli(bytes).len(),
        };
        self.written.push(SizedFile { name: name.to_string(), bytes: bytes.len(), brotli });
        Ok(())
    }

    /// Removes content-named files an earlier export wrote and this one
    /// did not (a changed picture or song), and nothing else.
    pub fn prune(&self) -> Result<(), String> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return Ok(()) };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_content_named(&name) && !self.written.iter().any(|w| w.name == name) && entry.path().is_file() {
                std::fs::remove_file(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
            }
        }
        Ok(())
    }

    pub fn written(&self) -> &[SizedFile] {
        &self.written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pack_names_are_written() {
        assert!(is_content_named("pdoom.0123456789abcdef.mp3"));
        assert!(!is_content_named("pdoom.mp3"));
        assert!(!is_content_named("notes.0123456789abcdeg.txt"));
    }
}
