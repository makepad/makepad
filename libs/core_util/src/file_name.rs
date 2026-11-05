//! What a file or folder name can't be.

/// Whether Windows can't hold `name`, one path component, as a file or folder: it names a device
/// (with any extension too: `NUL.txt`), Win32 would drop its trailing dot or space and so open
/// another file, or it holds a separator, a stream's `:` or another character the file system refuses.
pub fn windows_refuses(name: &str) -> bool {
    if name.is_empty() || name.ends_with(['.', ' ']) {
        return true;
    }
    if name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(&stem[3..], "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{b9}" | "\u{b2}" | "\u{b3}")
}

#[cfg(test)]
mod tests {
    use super::windows_refuses;

    #[test]
    fn devices_trailing_dots_and_reserved_characters_are_refused() {
        for name in ["", ".", "..", "a.", "a ", "CON", "con", "Nul.txt", "aux.tar.gz", "AUX .h", "prn", "COM1", "com0.log", "LPT9", "COM\u{b2}", "CONIN$", "a:b", "a/b", r"a\b", "a?", "a\tb"] {
            assert!(windows_refuses(name), "accepted {name:?}");
        }
        for name in ["console.h", "COM10", "COM", "LPT", "nullable.rs", "auxiliary", ".rels", "[Content_Types].xml", "a b.txt", "\u{e9}.txt"] {
            assert!(!windows_refuses(name), "refused {name:?}");
        }
    }
}
