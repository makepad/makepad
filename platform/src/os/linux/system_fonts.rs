use crate::{
    os::system_fonts::{SystemFontData, SystemFontError, SystemFontProvider},
    shared_bytes::SharedBytes,
};
use std::{
    collections::HashMap,
    process::Command,
    sync::{Mutex, OnceLock},
};

pub struct LinuxSystemFontProvider;

impl SystemFontProvider for LinuxSystemFontProvider {
    fn query_font(&self, family: &str) -> Result<SystemFontData, SystemFontError> {
        let key = font_cache_key(family);
        if let Some(cached) = cached_query_result(&key) {
            return cached;
        }
        let result = query_font_uncached(family);
        cache_query_result(key, result.clone());
        result
    }
}

fn query_font_uncached(family: &str) -> Result<SystemFontData, SystemFontError> {
    let (path, index) = resolve_fc_match(family)?;
    let data = SharedBytes::from_file_mmap_or_read(&path)
        .map_err(|err| SystemFontError::Io(err.to_string()))?;
    Ok(SystemFontData { data, index })
}

fn resolve_fc_match(family: &str) -> Result<(String, u32), SystemFontError> {
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
    parse_fc_match_output(&output.stdout)
}

fn parse_fc_match_output(stdout: &[u8]) -> Result<(String, u32), SystemFontError> {
    let stdout = String::from_utf8_lossy(stdout);
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
    Ok((path, index))
}

fn font_cache_key(family: &str) -> String {
    family.trim().to_ascii_lowercase()
}

fn query_cache() -> &'static Mutex<HashMap<String, Result<SystemFontData, SystemFontError>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Result<SystemFontData, SystemFontError>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_query_result(key: &str) -> Option<Result<SystemFontData, SystemFontError>> {
    let cache = query_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.get(key).cloned()
}

fn cache_query_result(key: String, result: Result<SystemFontData, SystemFontError>) {
    let mut cache = query_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(key, result);
}

#[cfg(test)]
mod tests {
    use super::{font_cache_key, parse_fc_match_output};
    use crate::os::system_fonts::SystemFontError;

    #[test]
    fn parse_fc_match_output_reads_path_and_index() {
        let (path, index) = parse_fc_match_output(b"/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf\n2\n")
            .expect("fc-match output should parse");
        assert_eq!(path, "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf");
        assert_eq!(index, 2);
    }

    #[test]
    fn parse_fc_match_output_rejects_empty_path() {
        let err = parse_fc_match_output(b"\n0\n").expect_err("empty path should fail");
        assert!(matches!(err, SystemFontError::NotFound));
    }

    #[test]
    fn parse_fc_match_output_rejects_invalid_index() {
        let err = parse_fc_match_output(b"/tmp/font.ttf\nabc\n").expect_err("invalid index should fail");
        match err {
            SystemFontError::Io(msg) => {
                assert!(msg.contains("failed to parse fc-match face index"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn cache_key_normalizes_case_and_whitespace() {
        assert_eq!(font_cache_key("  Noto Sans  "), "noto sans");
        assert_eq!(font_cache_key("Noto Sans"), "noto sans");
    }
}

pub fn query_font(family: &str) -> Result<SystemFontData, SystemFontError> {
    LinuxSystemFontProvider.query_font(family)
}
