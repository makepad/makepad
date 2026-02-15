/// Rust source code parsing utilities for extracting script blocks.

use std::path::{Path, PathBuf};

/// The kind of script macro found in Rust source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptMacroKind {
    /// `script_mod! { ... }` - module-level script definitions.
    ScriptMod,
    /// `script! { ... }` - inline script expressions.
    Script,
}

/// A script block extracted from Rust source code.
#[derive(Debug, Clone)]
pub struct ScriptBlock {
    /// Path to the source file.
    pub path: PathBuf,
    /// Line offset (0-based) where the script block starts.
    pub line_offset: u32,
    /// The script code content.
    pub code: String,
    /// The kind of script macro.
    pub macro_kind: ScriptMacroKind,
}

impl ScriptBlock {
    pub fn new(path: PathBuf, line_offset: u32, code: String, macro_kind: ScriptMacroKind) -> Self {
        Self { path, line_offset, code, macro_kind }
    }
}

/// Try to skip a Rust raw string literal starting at `start`.
/// Returns the position after the raw string if found, or None.
pub fn skip_rust_raw_string(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    if i >= bytes.len() {
        return None;
    }

    // Handle byte raw strings (br"...")
    if bytes[i] == b'b' {
        if i + 1 >= bytes.len() || bytes[i + 1] != b'r' {
            return None;
        }
        i += 2;
    } else if bytes[i] == b'r' {
        i += 1;
    } else {
        return None;
    }

    // Count hash marks
    let mut hash_count = 0usize;
    while i < bytes.len() && bytes[i] == b'#' {
        hash_count += 1;
        i += 1;
    }

    // Expect opening quote
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    i += 1;

    // Find matching closing quote with same number of hashes
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut j = i + 1;
            let mut matched = 0usize;
            while matched < hash_count && j < bytes.len() && bytes[j] == b'#' {
                matched += 1;
                j += 1;
            }
            if matched == hash_count {
                return Some(j);
            }
        }
        i += 1;
    }

    Some(bytes.len())
}

/// Skip a regular string or character literal starting at position `i`.
/// Returns the position after the closing quote.
pub fn skip_string_literal(bytes: &[u8], i: usize, quote: u8) -> usize {
    let mut pos = i + 1;
    while pos < bytes.len() {
        let b = bytes[pos];
        if b == b'\\' && pos + 1 < bytes.len() {
            pos += 2;
            continue;
        }
        pos += 1;
        if b == quote {
            break;
        }
    }
    pos
}

/// Skip a line comment starting at position `i`.
/// Returns the position after the newline.
pub fn skip_line_comment(bytes: &[u8], i: usize) -> usize {
    let mut pos = i + 2;
    while pos < bytes.len() && bytes[pos] != b'\n' {
        pos += 1;
    }
    pos
}

/// Skip a block comment starting at position `i`.
/// Returns the position after the closing `*/`.
pub fn skip_block_comment(bytes: &[u8], i: usize) -> usize {
    let mut pos = i + 2;
    while pos + 1 < bytes.len() {
        if bytes[pos] == b'*' && bytes[pos + 1] == b'/' {
            return pos + 2;
        }
        pos += 1;
    }
    pos
}

/// Maximum Rust value index in script (#(0), #(1), ... or @(0) etc).
/// Parser expects values.len() > max.
pub fn max_rust_value_index(code: &str) -> usize {
    let mut max = 0usize;
    let bytes = code.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        let is_hash = bytes[i] == b'#' && bytes[i + 1] == b'(';
        let is_at = bytes[i] == b'@' && bytes[i + 1] == b'(';
        if is_hash || is_at {
            i += 2;
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i > start {
                if let Ok(n) = std::str::from_utf8(&bytes[start..i]).unwrap_or("0").parse::<usize>() {
                    max = max.max(n);
                }
            }
            continue;
        }
        i += 1;
    }
    max
}

/// Replace Rust interpolation forms (`#(<rust expr>)`, `@(<rust expr>)`) with indexed placeholders
/// (`#(0)`, `#(1)`, ...) so parser-only validation can run on source extracted from Rust macros.
pub fn normalize_rust_interpolations(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let bytes = code.as_bytes();
    let mut i = 0usize;
    let mut next_index = 0usize;

    while i < bytes.len() {
        let c = bytes[i] as char;

        // Handle raw strings
        if let Some(next) = skip_rust_raw_string(bytes, i) {
            out.push_str(&code[i..next]);
            i = next;
            continue;
        }

        // Preserve regular strings
        if c == '"' || c == '\'' {
            let quote = c as u8;
            out.push(c);
            i += 1;
            while i < bytes.len() {
                let b = bytes[i];
                out.push(b as char);
                i += 1;
                if b == b'\\' && i < bytes.len() {
                    out.push(bytes[i] as char);
                    i += 1;
                    continue;
                }
                if b == quote {
                    break;
                }
            }
            continue;
        }

        // Preserve comments
        if c == '/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                // Line comment
                out.push('/');
                out.push('/');
                i += 2;
                while i < bytes.len() {
                    let b = bytes[i];
                    out.push(b as char);
                    i += 1;
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                // Block comment
                out.push('/');
                out.push('*');
                i += 2;
                while i + 1 < bytes.len() {
                    let b0 = bytes[i];
                    let b1 = bytes[i + 1];
                    out.push(b0 as char);
                    i += 1;
                    if b0 == b'*' && b1 == b'/' {
                        out.push('/');
                        i += 1;
                        break;
                    }
                }
                continue;
            }
        }

        // Normalize #(...) and @(...) to indexed placeholders
        let is_hash_interp = c == '#' && i + 1 < bytes.len() && bytes[i + 1] == b'(';
        let is_at_interp = c == '@' && i + 1 < bytes.len() && bytes[i + 1] == b'(';
        if is_hash_interp || is_at_interp {
            let marker = c;
            i += 2; // skip marker and '('

            let mut depth = 1u32;
            while i < bytes.len() && depth > 0 {
                // Skip nested raw strings
                if let Some(next) = skip_rust_raw_string(bytes, i) {
                    i = next;
                    continue;
                }

                let b = bytes[i];

                // Skip string literals
                if b == b'"' || b == b'\'' {
                    i = skip_string_literal(bytes, i, b);
                    continue;
                }

                // Skip comments
                if b == b'/' && i + 1 < bytes.len() {
                    if bytes[i + 1] == b'/' {
                        i = skip_line_comment(bytes, i);
                        continue;
                    }
                    if bytes[i + 1] == b'*' {
                        // Handle nested block comments
                        i += 2;
                        let mut comment_depth = 1u32;
                        while i + 1 < bytes.len() && comment_depth > 0 {
                            if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                                comment_depth += 1;
                                i += 2;
                            } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                                comment_depth -= 1;
                                i += 2;
                            } else {
                                i += 1;
                            }
                        }
                        continue;
                    }
                }

                // Track parenthesis depth
                if b == b'(' {
                    depth += 1;
                } else if b == b')' {
                    depth -= 1;
                }
                i += 1;
            }

            out.push(marker);
            out.push('(');
            out.push_str(&next_index.to_string());
            out.push(')');
            next_index += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }

    out
}

/// Given content starting after an opening `{`, find the matching `}` and return the content between.
/// Skips braces inside strings, raw strings, and comments.
fn extract_balanced_brace_block(content: &str) -> Option<(String, usize)> {
    let mut depth = 1u32;
    let mut i = 0;
    let mut out = String::new();
    let bytes = content.as_bytes();

    while i < bytes.len() && depth > 0 {
        let c = bytes[i] as char;

        // Handle raw strings
        if let Some(next) = skip_rust_raw_string(bytes, i) {
            out.push_str(&content[i..next]);
            i = next;
            continue;
        }

        // Handle regular strings
        if c == '"' || c == '\'' {
            let quote = c;
            out.push(c);
            i += 1;
            while i < bytes.len() {
                let b = bytes[i];
                if b == b'\\' && i + 1 < bytes.len() {
                    out.push(bytes[i] as char);
                    out.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                out.push(b as char);
                i += 1;
                if b == quote as u8 {
                    break;
                }
            }
            continue;
        }

        // Handle comments
        if c == '/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                // Line comment
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                // Block comment
                out.push('/');
                out.push('*');
                i += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        out.push('*');
                        out.push('/');
                        i += 2;
                        break;
                    }
                    out.push(bytes[i] as char);
                    i += 1;
                }
                continue;
            }
        }

        // Track brace depth
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some((out, i));
            }
        }

        out.push(c);
        i += 1;
    }

    if depth == 0 {
        Some((out, i))
    } else {
        None
    }
}

fn is_ident_char(c: u8) -> bool {
    c == b'_' || (c as char).is_ascii_alphanumeric()
}

/// Find the next `script_mod!` or `script!` macro starting from position `search`.
fn find_next_script_macro(content: &str, mut i: usize) -> Option<(usize, &'static str, ScriptMacroKind)> {
    let bytes = content.as_bytes();

    while i < bytes.len() {
        let c = bytes[i];

        // Skip raw strings
        if let Some(next) = skip_rust_raw_string(bytes, i) {
            i = next;
            continue;
        }

        // Skip regular strings
        if c == b'"' || c == b'\'' {
            i = skip_string_literal(bytes, i, c);
            continue;
        }

        // Skip comments
        if c == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                i = skip_line_comment(bytes, i);
                continue;
            }
            if bytes[i + 1] == b'*' {
                i = skip_block_comment(bytes, i);
                continue;
            }
        }

        let rem = &content[i..];
        let (macro_name, macro_kind) = if rem.starts_with("script_mod!") {
            ("script_mod!", ScriptMacroKind::ScriptMod)
        } else if rem.starts_with("script!") {
            ("script!", ScriptMacroKind::Script)
        } else {
            i += 1;
            continue;
        };

        // Ensure it's not part of a larger identifier
        let prev_ok = i == 0 || !is_ident_char(bytes[i - 1]);
        let next_pos = i + macro_name.len();
        let next_ok = next_pos >= bytes.len() || !is_ident_char(bytes[next_pos]);

        if prev_ok && next_ok {
            return Some((i, macro_name, macro_kind));
        }
        i += 1;
    }
    None
}

/// Extract script content from a Rust file: finds `script_mod! { ... }` and `script! { ... }` blocks.
/// Returns (line_number_1based, code, macro kind) for each block.
pub fn extract_script_blocks_from_rust(content: &str) -> Vec<(usize, String, ScriptMacroKind)> {
    let mut blocks = Vec::new();
    let mut search = 0;

    loop {
        let (start, macro_name, macro_kind) = match find_next_script_macro(content, search) {
            Some(found) => found,
            None => break,
        };

        search = start + macro_name.len();

        // Skip to opening brace
        let open = match content[search..].find('{') {
            Some(i) => search + i,
            None => break,
        };
        search = open + 1;

        // Extract balanced brace content
        let (block_content, _) = match extract_balanced_brace_block(&content[search..]) {
            Some(result) => result,
            None => continue,
        };

        let line_1based = content[..open].lines().count();
        blocks.push((line_1based, block_content.clone(), macro_kind));
        search = open + 1 + block_content.len() + 1; // past closing }
    }

    blocks
}

/// Collect all .rs files under a directory recursively.
pub fn collect_rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map_or(false, |e| e == "rs") {
                out.push(path);
            } else if path.is_dir() {
                out.extend(collect_rust_files(&path));
            }
        }
    }

    out.sort();
    out
}

/// Discover script blocks inside Rust files under src/.
pub fn discover_script_blocks(cwd: &Path) -> Vec<ScriptBlock> {
    let src = cwd.join("src");
    if !src.is_dir() {
        return Vec::new();
    }

    let mut blocks = Vec::new();

    for path in collect_rust_files(&src) {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for (line_1based, code, macro_kind) in extract_script_blocks_from_rust(&content) {
                blocks.push(ScriptBlock::new(
                    path.clone(),
                    (line_1based as u32).saturating_sub(1),
                    code,
                    macro_kind,
                ));
            }
        }
    }

    blocks
}

/// Discover module declarations from Rust source code.
/// Returns module names declared with `mod name;` syntax.
pub fn discover_declared_modules(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending_cfg_attr = false;

    for raw_line in content.lines() {
        let line = raw_line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        // Track cfg attributes
        if line.starts_with("#[") {
            if line.contains("cfg(") || line.contains("cfg_attr(") {
                pending_cfg_attr = true;
            }
            continue;
        }

        // Module declarations end with semicolon
        if !line.ends_with(';') {
            pending_cfg_attr = false;
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            pending_cfg_attr = false;
            continue;
        }

        let mod_index = match tokens.iter().position(|t| *t == "mod") {
            Some(idx) => idx,
            None => {
                pending_cfg_attr = false;
                continue;
            }
        };

        // Skip cfg-conditional modules
        if pending_cfg_attr {
            pending_cfg_attr = false;
            continue;
        }

        if mod_index + 1 >= tokens.len() {
            pending_cfg_attr = false;
            continue;
        }

        let mut name = tokens[mod_index + 1].trim_end_matches(';');
        if let Some(i) = name.find('{') {
            name = &name[..i];
        }

        if !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) {
            out.push(name.to_string());
        }

        pending_cfg_attr = false;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_interpolation_preserves_literals() {
        let with_string = r#"x = #(foo(")")) y"#;
        assert_eq!(normalize_rust_interpolations(with_string), "x = #(0) y");

        let with_comment = "x = #(foo(/* ) */ bar)) y";
        assert_eq!(normalize_rust_interpolations(with_comment), "x = #(0) y");
    }

    #[test]
    fn test_extract_script_blocks() {
        let content = r#"
fn main() {
    script_mod! {
        mod.test = { value: 42 }
    }
    script! { x = 1 }
}
"#;
        let blocks = extract_script_blocks_from_rust(content);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].2, ScriptMacroKind::ScriptMod);
        assert_eq!(blocks[1].2, ScriptMacroKind::Script);
    }

    #[test]
    fn test_discover_declared_modules() {
        let content = r#"
pub mod foo;
mod bar;
#[cfg(test)]
mod tests;
pub use other::thing;
"#;
        let modules = discover_declared_modules(content);
        assert_eq!(modules, vec!["foo", "bar"]);
    }
}
