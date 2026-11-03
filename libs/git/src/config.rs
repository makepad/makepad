//! Git configuration files (`config`, `config.worktree`) read and edited as
//! text, in git's own format:
//!
//! ```text
//! [section]               ; comment
//! 	name = value          # comment
//! [section "sub.section"]
//! 	flag                  (a bare name is boolean true)
//! 	quoted = "a ; b" \
//! 	         continued
//! ```
//!
//! Section and variable names compare case-insensitively, subsections
//! exactly; the last of several values wins, as for `git config --get`.
//! `[include]` directives are not followed. Edits keep every other byte of
//! the file and are written through `<file>.lock`, like git.
use std::{fs, io::Write, path::Path};

use crate::error::GitError;

/// One `name = value` line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    section: String,
    subsection: Option<String>,
    name: String,
    /// `None`: a bare name (boolean true)
    value: Option<String>,
    /// byte range of the whole line(s), continuations and newline included
    start: usize,
    end: usize,
}

/// A section header and where its body ends.
#[derive(Debug, Clone)]
struct Header {
    section: String,
    subsection: Option<String>,
    /// end of the section's last line (the next header's start, or the file end)
    end: usize,
}

struct Parsed {
    entries: Vec<Entry>,
    headers: Vec<Header>,
}

/// `section.name` or `section.sub.section.name` split at its first and last
/// dot, names lower-cased.
fn split_key(key: &str) -> Result<(String, Option<String>, String), GitError> {
    let (section, rest) = key.split_once('.').ok_or_else(|| GitError::InvalidRef(format!("config key without a section: {key}")))?;
    let (subsection, name) = match rest.rsplit_once('.') {
        Some((sub, name)) => (Some(sub.to_string()), name),
        None => (None, rest),
    };
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !valid(section) || !valid(name) || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err(GitError::InvalidRef(format!("invalid config key: {key}")));
    }
    Ok((section.to_ascii_lowercase(), subsection, name.to_ascii_lowercase()))
}

fn parse(text: &str) -> Parsed {
    let bytes = text.as_bytes();
    let mut entries = Vec::new();
    let mut headers: Vec<Header> = Vec::new();
    let (mut section, mut subsection) = (String::new(), None::<String>);
    let mut i = 0;
    while i < bytes.len() {
        let line_start = i;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'[' {
            if let Some(last) = headers.last_mut() {
                last.end = line_start;
            }
            i += 1;
            let name_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'.') {
                i += 1;
            }
            let raw = &text[name_start..i];
            let (name, mut sub) = match raw.split_once('.') {
                // deprecated `[section.sub]`: the subsection is lower-cased
                Some((s, rest)) => (s.to_ascii_lowercase(), Some(rest.to_ascii_lowercase())),
                None => (raw.to_ascii_lowercase(), None),
            };
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
                let mut s = String::new();
                while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\n' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] != b'\n' {
                        i += 1;
                    }
                    let c = text[i..].chars().next().unwrap_or('\0');
                    s.push(c);
                    i += c.len_utf8().max(1);
                }
                if i < bytes.len() && bytes[i] == b'"' {
                    i += 1;
                }
                sub = Some(s);
            }
            while i < bytes.len() && bytes[i] != b']' && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b']' {
                i += 1;
            }
            section = name;
            subsection = sub;
            headers.push(Header { section: section.clone(), subsection: subsection.clone(), end: bytes.len() });
            // a variable may follow the header on the same line
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            if i >= bytes.len() || bytes[i] == b'\n' || bytes[i] == b'#' || bytes[i] == b';' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                i += 1;
                continue;
            }
        }
        if i >= bytes.len() || bytes[i] == b'\n' || bytes[i] == b'#' || bytes[i] == b';' || !bytes[i].is_ascii_alphabetic() {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            i += 1;
            continue;
        }
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
            i += 1;
        }
        let name = text[name_start..i].to_ascii_lowercase();
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            let (v, next) = parse_value(text, i);
            i = next;
            Some(v)
        } else {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            None
        };
        if i < bytes.len() {
            i += 1; // the newline
        }
        if !section.is_empty() {
            entries.push(Entry { section: section.clone(), subsection: subsection.clone(), name, value, start: line_start, end: i.min(bytes.len()) });
        }
    }
    if let Some(last) = headers.last_mut() {
        last.end = bytes.len();
    }
    Parsed { entries, headers }
}

/// A value from `start` (just past `=`) to its line end, read the way git's
/// `parse_value` reads it: each unquoted whitespace character becomes a
/// space and counts only between words, `"` toggles quoting, `;`/`#` start
/// a comment outside quotes, a backslash escapes `n t b " \` and a newline. Returns
/// the value and the index of the newline (or the end) that ends it.
fn parse_value(text: &str, start: usize) -> (String, usize) {
    let mut out = String::new();
    let (mut quoted, mut comment, mut space) = (false, false, 0usize);
    let mut it = text[start..].char_indices().peekable();
    while let Some((off, c)) = it.next() {
        let c = if c == '\r' && it.peek().is_some_and(|(_, n)| *n == '\n') { it.next(); '\n' } else { c };
        if c == '\n' {
            let at = start + off + if text.as_bytes()[start + off] == b'\r' { 1 } else { 0 };
            return (out, at);
        }
        if comment {
            continue;
        }
        if c.is_ascii_whitespace() && !quoted {
            if !out.is_empty() {
                space += 1;
            }
            continue;
        }
        if !quoted && (c == ';' || c == '#') {
            comment = true;
            continue;
        }
        out.extend(std::iter::repeat(' ').take(space));
        space = 0;
        match c {
            '\\' => match it.next().map(|(_, e)| e) {
                Some('\n') | None => {}
                Some('t') => out.push('\t'),
                Some('b') => out.push('\u{8}'),
                Some('n') => out.push('\n'),
                Some(e) => out.push(e),
            },
            '"' => quoted = !quoted,
            c => out.push(c),
        }
    }
    (out, text.len())
}

fn matches(e_section: &str, e_sub: &Option<String>, section: &str, sub: &Option<String>) -> bool {
    e_section == section && e_sub == sub
}

/// The value of `key` in config text (the last one), `None` when unset. A
/// bare name reads as `"true"`.
pub fn get(text: &str, key: &str) -> Result<Option<String>, GitError> {
    let (section, sub, name) = split_key(key)?;
    Ok(parse(text).entries.into_iter().rev().find(|e| e.name == name && matches(&e.section, &e.subsection, &section, &sub)).map(|e| e.value.unwrap_or_else(|| "true".into())))
}

/// A boolean as git reads one (`true`/`yes`/`on`/`1`, `false`/`no`/`off`/`0`/empty).
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" | "" => Some(false),
        _ => value.trim().parse::<i64>().ok().map(|n| n != 0),
    }
}

/// `value` as git's `write_pair` writes it: quoted when it starts or ends
/// with a space or holds a comment character; backslash, `"`, newline and
/// tab escaped.
fn quote(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    if value.starts_with(' ') || value.ends_with(' ') || value.contains(['#', ';']) {
        format!("\"{out}\"")
    } else {
        out
    }
}

/// The text with `key` set to `value`: the last existing line replaced, else
/// a line added at the end of the last matching section, else a new section.
pub fn set(text: &str, key: &str, value: &str) -> Result<String, GitError> {
    let (section, sub, name) = split_key(key)?;
    let p = parse(text);
    let line = format!("\t{name} = {}\n", quote(value));
    if let Some(e) = p.entries.iter().rev().find(|e| e.name == name && matches(&e.section, &e.subsection, &section, &sub)) {
        // keep a header that shares the line (`[core] bare = true`)
        let line_text = &text[e.start..e.end];
        let head = line_text.find(']').filter(|_| line_text.trim_start().starts_with('[')).map(|i| &line_text[..=i]);
        return Ok(format!("{}{}{}{}", &text[..e.start], head.map_or(String::new(), |h| format!("{h}\n")), line, &text[e.end..]));
    }
    if let Some(h) = p.headers.iter().rev().find(|h| matches(&h.section, &h.subsection, &section, &sub)) {
        let mut out = text[..h.end].to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        out.push_str(&text[h.end..]);
        return Ok(out);
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    match &sub {
        Some(s) => out.push_str(&format!("[{section} \"{}\"]\n", s.replace('\\', "\\\\").replace('"', "\\\""))),
        None => out.push_str(&format!("[{section}]\n")),
    }
    out.push_str(&line);
    Ok(out)
}

/// The text with every `key` line removed (`git config --unset-all`).
pub fn unset_all(text: &str, key: &str) -> Result<String, GitError> {
    let (section, sub, name) = split_key(key)?;
    let p = parse(text);
    let mut out = String::new();
    let mut at = 0;
    for e in p.entries.iter().filter(|e| e.name == name && matches(&e.section, &e.subsection, &section, &sub)) {
        out.push_str(&text[at..e.start]);
        let line_text = &text[e.start..e.end];
        if line_text.trim_start().starts_with('[') {
            if let Some(i) = line_text.find(']') {
                out.push_str(&line_text[..=i]);
                out.push('\n');
            }
        }
        at = e.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// A config file's text; empty when the file does not exist.
pub fn read_file(path: &Path) -> Result<String, GitError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(GitError::Io(e)),
    }
}

/// Replaces a config file through `<file>.lock` (refused while another
/// writer holds the lock), like git.
pub fn write_file(path: &Path, text: &str) -> Result<(), GitError> {
    let lock = path.with_file_name(format!("{}.lock", path.file_name().and_then(|n| n.to_str()).unwrap_or("config")));
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&lock).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            GitError::InvalidRef(format!("{} is locked", path.display()))
        } else {
            GitError::Io(e)
        }
    })?;
    let written = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = written.and_then(|_| fs::rename(&lock, path)) {
        let _ = fs::remove_file(&lock);
        return Err(GitError::Io(e));
    }
    Ok(())
}

/// Sets `key` in the config file at `path` when it differs; true when the
/// file changed.
pub fn set_in_file(path: &Path, key: &str, value: &str) -> Result<bool, GitError> {
    let text = read_file(path)?;
    if get(&text, key)?.as_deref() == Some(value) {
        return Ok(false);
    }
    write_file(path, &set(&text, key, value)?)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_as_git_does() {
        let t = "[core]\n\tbare = false\n\tFileMode\n[Merge \"ci-proof\"]\n\tname = ci.toml proofs ; why\n\tdriver = cargo run '/a b/c' -- %O \"%A\" # x\n[extensions] worktreeConfig = TRUE\n[s \"q\"]\n\tv = \"  pad ; # \"\n\tw = one \\\n  two\n\tw = last\n";
        assert_eq!(get(t, "core.bare").unwrap().as_deref(), Some("false"));
        assert_eq!(get(t, "core.filemode").unwrap().as_deref(), Some("true"));
        assert_eq!(get(t, "merge.ci-proof.name").unwrap().as_deref(), Some("ci.toml proofs"));
        assert_eq!(get(t, "merge.ci-proof.driver").unwrap().as_deref(), Some("cargo run '/a b/c' -- %O %A"));
        assert_eq!(get(t, "merge.CI-PROOF.name").unwrap(), None);
        assert_eq!(get(t, "extensions.worktreeconfig").unwrap().as_deref().and_then(parse_bool), Some(true));
        assert_eq!(get(t, "s.q.v").unwrap().as_deref(), Some("  pad ; # "));
        assert_eq!(get(t, "s.q.w").unwrap().as_deref(), Some("last"));
        assert_eq!(get("[s \"q\"]\n w = one \\\n  two\n", "s.q.w").unwrap().as_deref(), Some("one   two"));
        assert_eq!(get(t, "core.missing").unwrap(), None);
    }

    #[test]
    fn edits_round_trip_and_keep_the_rest() {
        let t = "# top\n[core]\n\tbare = true\n[extensions] worktreeConfig = false\n";
        let s = set(t, "merge.ci-proof.driver", "cargo run '/p q' -- %O ; x").unwrap();
        assert!(s.starts_with(t));
        assert_eq!(get(&s, "merge.ci-proof.driver").unwrap().as_deref(), Some("cargo run '/p q' -- %O ; x"));
        let s = set(&s, "extensions.worktreeConfig", "true").unwrap();
        assert_eq!(get(&s, "extensions.worktreeconfig").unwrap().as_deref(), Some("true"));
        assert!(s.contains("[extensions]\n"));
        let s = set(&s, "core.worktree", "/w").unwrap();
        assert!(s.contains("[core]\n\tbare = true\n\tworktree = /w\n"));
        let s = unset_all(&s, "core.bare").unwrap();
        assert_eq!(get(&s, "core.bare").unwrap(), None);
        assert_eq!(get(&s, "core.worktree").unwrap().as_deref(), Some("/w"));
        assert!(s.starts_with("# top\n[core]\n"));
        assert!(set(t, "nodot", "x").is_err());
    }
}
