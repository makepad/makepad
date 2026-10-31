//! Blanking unused Splash in a linked wasm.
//!
//! An app embeds every `script_mod!` block as a string literal, so a web
//! build carries the source of definitions and styles it never uses. A
//! collect run decides which top-level definitions are unused and writes
//! each block's text with those ranges blanked: every byte that is not a
//! newline becomes a space, so the text keeps its length and its line
//! structure (error locations in what remains stay exact) and brotli turns
//! the blank runs into almost nothing. [`blank`] makes such a text;
//! [`apply`] finds each original text in the wasm and writes the blanked
//! one over it, byte for byte.

/// `code` with every byte inside `ranges` (byte offsets, end exclusive)
/// that is not `\n` replaced by a space. Multi-byte characters inside a
/// range become one space per byte, so the result is ASCII there and has
/// the same length.
pub fn blank(code: &str, ranges: &[(usize, usize)]) -> String {
    let mut bytes = code.as_bytes().to_vec();
    for &(start, end) in ranges {
        let end = end.min(bytes.len());
        for b in &mut bytes[start.min(end)..end] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    // Ranges are expected on character boundaries; a range that splits a
    // character leaves its remaining continuation bytes, which are blanked
    // too so the text stays valid UTF-8.
    let mut i = 0;
    while i < bytes.len() {
        match std::str::from_utf8(&bytes[i..]) {
            Ok(_) => break,
            Err(e) => {
                let bad = i + e.valid_up_to();
                let len = e.error_len().unwrap_or(bytes.len() - bad);
                for b in &mut bytes[bad..bad + len] {
                    *b = b' ';
                }
                i = bad + len;
            }
        }
    }
    String::from_utf8(bytes).expect("blanked text is valid UTF-8")
}

/// What [`apply`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlankReport {
    /// Texts found and blanked (each may occur more than once).
    pub texts: usize,
    /// Occurrences overwritten.
    pub occurrences: usize,
    /// Bytes turned into spaces.
    pub bytes_blanked: usize,
    /// Texts not found in the wasm (their first 60 characters): the binary
    /// was built from other sources than the collect run.
    pub missing: Vec<String>,
}

/// Writes each `(original, blanked)` pair's blanked text over every
/// occurrence of the original in `wasm`. The pairs must have equal byte
/// lengths and differ only where the blanked text has spaces.
pub fn apply(wasm: &mut [u8], pairs: &[(String, String)]) -> Result<BlankReport, String> {
    let mut report = BlankReport::default();
    for (original, blanked) in pairs {
        if original.len() != blanked.len() {
            return Err(format!("a blanked text changed its length ({} -> {} bytes)", original.len(), blanked.len()));
        }
        if original.bytes().zip(blanked.bytes()).any(|(a, b)| a != b && (b != b' ' || a == b'\n')) {
            return Err("a blanked text changes more than blanked ranges".into());
        }
        if original == blanked || original.is_empty() {
            continue;
        }
        let needle = original.as_bytes();
        let mut found = 0;
        let mut at = 0;
        while let Some(pos) = find(&wasm[at..], needle) {
            let start = at + pos;
            wasm[start..start + needle.len()].copy_from_slice(blanked.as_bytes());
            found += 1;
            at = start + needle.len();
        }
        if found == 0 {
            report.missing.push(original.chars().take(60).collect());
            continue;
        }
        report.texts += 1;
        report.occurrences += found;
        report.bytes_blanked += found * original.bytes().zip(blanked.bytes()).filter(|(a, b)| a != b).count();
    }
    Ok(report)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        match hay[i..hay.len() - needle.len() + 1].iter().position(|&b| b == first) {
            None => return None,
            Some(p) => {
                i += p;
                if &hay[i..i + needle.len()] == needle {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_keeps_newlines_and_length() {
        let code = "let a = 1\nmod.widgets.Foo = View{\n    x: \"é\"\n}\nlet b = 2\n";
        let start = code.find("mod.widgets.Foo").unwrap();
        let end = code.find("let b").unwrap();
        let out = blank(code, &[(start, end)]);
        assert_eq!(out.len(), code.len());
        assert_eq!(out.lines().count(), code.lines().count());
        assert!(out.starts_with("let a = 1\n"));
        assert!(out.ends_with("let b = 2\n"));
        assert!(out[start..end].bytes().all(|b| b == b' ' || b == b'\n'));
        // Every line keeps its start offset.
        let starts = |s: &str| s.match_indices('\n').map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(starts(code), starts(&out));
    }

    #[test]
    fn apply_overwrites_every_occurrence() {
        let code = "mod.widgets.A = 1\nmod.widgets.B = 2;".to_string();
        let blanked = blank(&code, &[(0, 17)]);
        let mut wasm = b"\0\x01head".to_vec();
        wasm.extend_from_slice(code.as_bytes());
        wasm.extend_from_slice(b"\0mid\0");
        wasm.extend_from_slice(code.as_bytes());
        let report = apply(&mut wasm, &[(code.clone(), blanked.clone())]).unwrap();
        assert_eq!(report.occurrences, 2);
        assert_eq!(report.bytes_blanked, 2 * code[..17].bytes().filter(|&b| b != b' ').count());
        assert!(find(&wasm, code.as_bytes()).is_none());
        assert!(find(&wasm, blanked.as_bytes()).is_some());
        let missing = apply(&mut wasm, &[("not there".into(), "         ".into())]).unwrap();
        assert_eq!(missing.missing.len(), 1);
        assert!(apply(&mut wasm, &[("ab".into(), "a".into())]).is_err());
    }
}
