//! Cell widths and wrapping. Widths come from the terminal app's Unicode
//! tables (`crate::unicode`), so a wide or combining character takes the
//! cells the terminal gives it.

/// Cells one character takes: 0 for controls and combining marks, 2 for
/// wide ones.
pub fn char_width(c: char) -> usize {
    makepad_cell_width::char_width(c as u32) as usize
}

/// Cells a string takes (no escape sequences in it).
pub fn width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Tabs to spaces (to the next multiple of 4) and other controls dropped,
/// so every character left has a known width.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    for c in s.chars() {
        match c {
            '\t' => {
                let n = 4 - col % 4;
                out.extend(std::iter::repeat(' ').take(n));
                col += n;
            }
            '\n' => {
                out.push('\n');
                col = 0;
            }
            c if c.is_control() => {}
            c => {
                out.push(c);
                col += char_width(c);
            }
        }
    }
    out
}

/// One logical line (no '\n') cut into rows of at most `width` cells, at
/// spaces where it can, mid-word where a word is longer than a row. Rows
/// after the first start with `indent` (counted in the width). An empty
/// line is one empty row.
pub fn wrap_line(line: &str, cols: usize, indent: &str) -> Vec<String> {
    let cols = cols.max(width_of_indent(indent) + 8);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_w = 0;
    // Split into words with their trailing spaces kept.
    let mut words: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut in_space = false;
    for (i, c) in line.char_indices() {
        if c == ' ' {
            in_space = true;
        } else if in_space {
            words.push(&line[start..i]);
            start = i;
            in_space = false;
        }
    }
    words.push(&line[start..]);
    for word in words {
        let word_w = width(word);
        let bare_w = width(word.trim_end_matches(' '));
        if row_w + bare_w <= cols {
            row.push_str(word);
            row_w += word_w;
            continue;
        }
        if row_w > width_of_indent_row(&rows, indent) && bare_w <= cols - width_of_indent(indent) {
            // The word fits on a fresh row.
            rows.push(row.trim_end_matches(' ').to_string());
            row = format!("{indent}{word}");
            row_w = width_of_indent(indent) + word_w;
            continue;
        }
        // Too long for any row: cut it.
        for c in word.chars() {
            let cw = char_width(c);
            if row_w + cw > cols {
                rows.push(row.trim_end_matches(' ').to_string());
                row = indent.to_string();
                row_w = width_of_indent(indent);
                if c == ' ' {
                    continue;
                }
            }
            row.push(c);
            row_w += cw;
        }
    }
    rows.push(row.trim_end_matches(' ').to_string());
    rows
}

fn width_of_indent(indent: &str) -> usize {
    width(indent)
}

/// What a row holds before any word: the indent on continuation rows.
fn width_of_indent_row(rows: &[String], indent: &str) -> usize {
    if rows.is_empty() {
        0
    } else {
        width_of_indent(indent)
    }
}

/// Every line of `text`, wrapped.
pub fn wrap(text: &str, width: usize, indent: &str) -> Vec<String> {
    text.split('\n').flat_map(|l| wrap_line(l, width, indent)).collect()
}

/// `s` cut to `width` cells, ending in `ellipsis` when cut.
pub fn truncate(s: &str, width: usize, ellipsis: &str) -> String {
    if self::width(s) <= width {
        return s.to_string();
    }
    let room = width.saturating_sub(self::width(ellipsis));
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > room {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push_str(ellipsis);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("日本"), 4);
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(sanitize("a\tb\x07"), "a   b");
    }

    #[test]
    fn wraps_at_spaces() {
        assert_eq!(wrap_line("the quick brown fox", 10, ""), ["the quick", "brown fox"]);
        assert_eq!(wrap_line("", 10, ""), [""]);
        assert_eq!(wrap_line("short", 10, ""), ["short"]);
    }

    #[test]
    fn wraps_long_words_and_indents() {
        assert_eq!(wrap_line("abcdefghijklmnop", 10, ""), ["abcdefghij", "klmnop"]);
        assert_eq!(wrap_line("aaaa bbbb cccc dddd", 10, "  "), ["aaaa bbbb", "  cccc", "  dddd"]);
        for row in wrap_line("一二三四五六七八九十", 9, "") {
            assert!(width(&row) <= 9);
        }
    }

    #[test]
    fn truncates() {
        assert_eq!(truncate("hello world", 8, "..."), "hello...");
        assert_eq!(truncate("hi", 8, "..."), "hi");
    }
}
