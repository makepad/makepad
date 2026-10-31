//! File paths in the chat's text: found, checked against the workspace, and
//! turned into links (`ide::IdeAction::OpenFile`). Pure text work; whether a
//! path is a file is asked of the caller (the view caches the answer).

/// A path found in a text: its byte range (the path and its line suffix),
/// the path relative to the root, and where to open it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSpan {
    pub start: usize,
    pub end: usize,
    pub path: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

fn path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | '+' | '@' | '~' | '\\')
}

/// `12`, `12:9`, `1-80` (the first line), from the text after a `:`.
fn line_col(rest: &str) -> (Option<u32>, Option<u32>, usize) {
    let digits = |s: &str| s.chars().take_while(|c| c.is_ascii_digit()).count();
    let n = digits(rest);
    if n == 0 {
        return (None, None, 0);
    }
    let line = rest[..n].parse().ok();
    let mut used = n;
    let mut col = None;
    let after = &rest[n..];
    if let Some(r) = after.strip_prefix(':') {
        let m = digits(r);
        if m > 0 {
            col = r[..m].parse().ok();
            used += 1 + m;
        }
    } else if let Some(r) = after.strip_prefix('-') {
        // A range: the line it starts on.
        let m = digits(r);
        if m > 0 {
            used += 1 + m;
        }
    }
    (line, col, used)
}

/// Every existing file path in `text`: relative to the root or absolute
/// inside it (`root` with a trailing `/`), with an optional `:line[:col]`,
/// `:a-b` or `#L12` suffix. `is_file` is asked once per candidate.
pub fn find_paths(text: &str, root: &str, is_file: &mut dyn FnMut(&str) -> bool) -> Vec<PathSpan> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        if !path_char(c) {
            i += c.len_utf8();
            continue;
        }
        let start = i;
        let mut end = i;
        for (k, ch) in text[i..].char_indices() {
            if !path_char(ch) {
                break;
            }
            end = i + k + ch.len_utf8();
        }
        i = end;
        // Trailing sentence punctuation is not part of a path.
        let mut tok_end = end;
        while tok_end > start && matches!(bytes[tok_end - 1], b'.' | b'-' | b'/' | b'\\') {
            tok_end -= 1;
        }
        let tok = &text[start..tok_end];
        // A path has a folder or an extension; words never stat.
        if tok.is_empty() || !(tok.contains('/') || tok.char_indices().skip(1).any(|(_, c)| c == '.')) || tok.contains("://") {
            continue;
        }
        // An `@path` mention or a URL-ish prefix: the path is after it.
        let (tok_start, tok) = match tok.strip_prefix('@') {
            Some(t) => (start + 1, t),
            None => (start, tok),
        };
        let rel = if !root.is_empty() && tok.starts_with(root) {
            &tok[root.len()..]
        } else if tok.starts_with('/') {
            continue;
        } else {
            tok.strip_prefix("./").unwrap_or(tok)
        };
        if rel.is_empty() || rel.split('/').any(|p| p == "..") || !is_file(rel) {
            continue;
        }
        let mut span = PathSpan { start: tok_start, end: tok_end, path: rel.to_string(), line: None, col: None };
        if tok_end == end {
            let rest = &text[end..];
            if let Some(r) = rest.strip_prefix(':') {
                let (line, col, used) = line_col(r);
                if used > 0 {
                    span.line = line;
                    span.col = col;
                    span.end = end + 1 + used;
                }
            } else if let Some(r) = rest.strip_prefix("#L") {
                let (line, _, used) = line_col(r);
                if used > 0 {
                    span.line = line;
                    span.end = end + 2 + used;
                    // `#L12-L20`
                    if let Some(r2) = text[span.end..].strip_prefix("-L") {
                        let n = r2.chars().take_while(|c| c.is_ascii_digit()).count();
                        if n > 0 {
                            span.end += 2 + n;
                        }
                    }
                }
            }
        }
        i = i.max(span.end);
        out.push(span);
    }
    out
}

/// Markdown with every file path made a link (`[text](<text>)`), so the
/// Markdown widget draws it in the link ink. Inline code that is exactly a
/// path becomes a link around the code; bare paths in prose become links;
/// fenced code blocks and existing links are left alone. The rendered text
/// is unchanged, so hit-testing the rendered text finds the same paths.
pub fn linkify_markdown(md: &str, root: &str, is_file: &mut dyn FnMut(&str) -> bool) -> String {
    let mut out = String::with_capacity(md.len() + 32);
    let mut fenced = false;
    for line in md.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced || line.starts_with("    ") || line.starts_with('\t') {
            out.push_str(line);
            continue;
        }
        linkify_line(line, root, is_file, &mut out);
    }
    out
}

fn link(out: &mut String, shown: &str, url: &str) {
    out.push('[');
    out.push_str(shown);
    out.push_str("](<");
    out.push_str(url);
    out.push_str(">)");
}

fn linkify_line(line: &str, root: &str, is_file: &mut dyn FnMut(&str) -> bool, out: &mut String) {
    // Leave lines with link syntax as they are.
    if line.contains("](") {
        out.push_str(line);
        return;
    }
    let mut rest = line;
    while !rest.is_empty() {
        match rest.find('`') {
            Some(tick) => {
                prose(&rest[..tick], root, is_file, out);
                let after = &rest[tick..];
                let run = after.chars().take_while(|c| *c == '`').count();
                let fence = &after[..run];
                match after[run..].find(fence) {
                    Some(close) => {
                        let code = &after[run..run + close];
                        let whole = &after[..run + close + run];
                        let spans = find_paths(code.trim(), root, is_file);
                        match spans.first() {
                            Some(s) if s.start == 0 && s.end == code.trim().len() => link(out, whole, code.trim()),
                            _ => out.push_str(whole),
                        }
                        rest = &after[run + close + run..];
                    }
                    None => {
                        out.push_str(after);
                        rest = "";
                    }
                }
            }
            None => {
                prose(rest, root, is_file, out);
                rest = "";
            }
        }
    }
}

fn prose(text: &str, root: &str, is_file: &mut dyn FnMut(&str) -> bool, out: &mut String) {
    let mut at = 0;
    for s in find_paths(text, root, is_file) {
        out.push_str(&text[at..s.start]);
        link(out, &text[s.start..s.end], &text[s.start..s.end]);
        at = s.end;
    }
    out.push_str(&text[at..]);
}

/// The new-side line ranges (1-based, inclusive) a diff added or changed:
/// runs of added lines; a pure deletion lights the line after it.
pub fn changed_ranges(diff: &makepad_code_agent::event::Diff, new_side: bool) -> Vec<(u32, u32)> {
    use makepad_code_agent::event::DiffLine;
    let mut out: Vec<(u32, u32)> = Vec::new();
    for hunk in &diff.hunks {
        let mut line = if new_side { hunk.new_start } else { hunk.old_start }.max(1);
        let mut run: Option<(u32, u32)> = None;
        let mut gap = false;
        for (kind, _) in &hunk.lines {
            let (mine, other) = if new_side { (DiffLine::Added, DiffLine::Removed) } else { (DiffLine::Removed, DiffLine::Added) };
            if *kind == mine {
                run = Some(match run {
                    Some((a, _)) => (a, line),
                    None => (line, line),
                });
                line += 1;
            } else if *kind == other {
                gap = true;
            } else {
                if let Some(r) = run.take() {
                    out.push(r);
                } else if gap {
                    out.push((line, line));
                }
                gap = false;
                line += 1;
            }
        }
        if let Some(r) = run {
            out.push(r);
        } else if gap {
            out.push((line.saturating_sub(1).max(1), line.saturating_sub(1).max(1)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &'static [&'static str]) -> impl FnMut(&str) -> bool {
        move |p| list.contains(&p)
    }

    #[test]
    fn finds_paths_with_lines_and_columns() {
        let mut f = files(&["src/main.rs", "Cargo.toml", "apps/code/src/ui/view.rs"]);
        let t = "see src/main.rs:12, then Cargo.toml. --> src/main.rs:12:9 and apps/code/src/ui/view.rs#L40-L52 but not src/nope.rs or the.";
        let s = find_paths(t, "/repo/", &mut f);
        assert_eq!(s.len(), 4, "{s:?}");
        assert_eq!((&t[s[0].start..s[0].end], s[0].line, s[0].col), ("src/main.rs:12", Some(12), None));
        assert_eq!((&t[s[1].start..s[1].end], s[1].line), ("Cargo.toml", None));
        assert_eq!((s[2].line, s[2].col), (Some(12), Some(9)));
        assert_eq!((&t[s[3].start..s[3].end], s[3].line), ("apps/code/src/ui/view.rs#L40-L52", Some(40)));
    }

    #[test]
    fn ranges_absolute_mentions_and_grep_lines() {
        let mut f = files(&["src/main.rs"]);
        let s = find_paths("read src/main.rs:1-80", "/repo/", &mut f);
        assert_eq!((s[0].path.as_str(), s[0].line, s[0].end), ("src/main.rs", Some(1), 21));
        let s = find_paths("open /repo/src/main.rs:3 and /etc/passwd", "/repo/", &mut f);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].path.as_str(), s[0].line, s[0].start), ("src/main.rs", Some(3), 5));
        let s = find_paths("src/main.rs:41: fn handle_event(&mut self)", "/repo/", &mut f);
        assert_eq!((s[0].line, s[0].end), (Some(41), 14));
        let s = find_paths("fix @src/main.rs please", "/repo/", &mut f);
        assert_eq!((s[0].start, s[0].path.as_str()), (5, "src/main.rs"));
        assert!(find_paths("../src/main.rs", "/repo/", &mut f).is_empty());
    }

    #[test]
    fn markdown_links_keep_the_rendered_text() {
        let mut f = files(&["src/main.rs", "a_b.rs"]);
        let md = "Look at `src/main.rs:12` and src/main.rs.\n\n```rust\n// src/main.rs\n```\nnot `let x = 1;`, [x](y) src/main.rs\n";
        let out = linkify_markdown(md, "/r/", &mut f);
        assert!(out.contains("[`src/main.rs:12`](<src/main.rs:12>)"), "{out}");
        assert!(out.contains("and [src/main.rs](<src/main.rs>)."), "{out}");
        assert!(out.contains("// src/main.rs\n"), "{out}");
        assert!(out.contains("`let x = 1;`"));
        assert!(out.ends_with("[x](y) src/main.rs\n"), "{out}");
    }

    #[test]
    fn changed_lines_from_hunks() {
        use makepad_code_agent::event::{Diff, DiffLine, Hunk};
        let l = |k, t: &str| (k, t.to_string());
        let diff = Diff {
            path: "a".into(),
            hunks: vec![Hunk {
                old_start: 10,
                new_start: 10,
                lines: vec![
                    l(DiffLine::Context, "a"),
                    l(DiffLine::Removed, "b"),
                    l(DiffLine::Added, "B"),
                    l(DiffLine::Added, "B2"),
                    l(DiffLine::Context, "c"),
                    l(DiffLine::Removed, "d"),
                    l(DiffLine::Context, "e"),
                ],
            }],
        };
        assert_eq!(changed_ranges(&diff, true), vec![(11, 12), (14, 14)]);
        assert_eq!(changed_ranges(&diff, false), vec![(11, 11), (13, 13)]);
    }
}
