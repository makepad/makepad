//! A lossless view of the supported note syntax. Offsets are always UTF-8 bytes.
use crate::{engine, model::*};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParagraphStyle {
    NoteTitle,
    Title,
    Heading,
    #[default]
    Body,
    Bullet,
    Number(usize),
    Check(bool),
}
#[derive(Clone, Debug)]
pub struct Run {
    pub range: Range<usize>,
    pub marks: Marks,
}
#[derive(Clone, Debug)]
pub struct Paragraph {
    pub source_start: usize,
    pub content_start: usize,
    pub range: Range<usize>,
    pub style: ParagraphStyle,
    pub runs: Vec<Run>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Boundary {
    /// The source side before / after any hidden syntax at this display boundary.
    pub before: usize,
    pub after: usize,
}
#[derive(Clone, Debug)]
pub struct Emphasis {
    pub source: Range<usize>,
    pub display: Range<usize>,
    pub delimiter: String,
}
#[derive(Clone, Debug, Default)]
pub struct Projection {
    pub text: String,
    pub paragraphs: Vec<Paragraph>,
    pub boundaries: Vec<Boundary>,
    pub emphasis: Vec<Emphasis>,
}

pub fn prefix(line: &str) -> (usize, ParagraphStyle) {
    if line.starts_with("- [ ] ") {
        (6, ParagraphStyle::Check(false))
    } else if line.starts_with("- [x] ") {
        (6, ParagraphStyle::Check(true))
    } else if line.starts_with("## ") {
        (3, ParagraphStyle::Heading)
    } else if line.starts_with("# ") {
        (2, ParagraphStyle::Title)
    } else if line.starts_with("- ") {
        (2, ParagraphStyle::Bullet)
    } else {
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && line[digits..].starts_with(". ") {
            if let Ok(n) = line[..digits].parse() {
                return (digits + 2, ParagraphStyle::Number(n));
            }
        }
        (0, ParagraphStyle::Body)
    }
}

impl Projection {
    pub fn new(source: &str) -> Self {
        let mut out = Self {
            boundaries: vec![Boundary::default()],
            ..Self::default()
        };
        let mut offset = 0;
        let mut fenced = false;
        for (index, line) in source.split('\n').enumerate() {
            let start = out.text.len();
            let literal = fenced || line.trim_start().starts_with("```");
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            let (skip, mut style) = if literal {
                (0, ParagraphStyle::Body)
            } else {
                prefix(line)
            };
            let skip = if index == 0 {
                style = ParagraphStyle::NoteTitle;
                if line.starts_with("# ") || line.starts_with("## ") {
                    skip
                } else {
                    0
                }
            } else {
                skip
            };
            out.hide(offset, offset + skip);
            let mut runs = Vec::new();
            if literal {
                out.literal(
                    source,
                    offset,
                    offset + line.len(),
                    Marks::default(),
                    &mut runs,
                );
            } else {
                out.inline(
                    source,
                    offset + skip,
                    offset + line.len(),
                    Marks::default(),
                    &mut runs,
                );
            }
            out.paragraphs.push(Paragraph {
                source_start: offset,
                content_start: offset + skip,
                range: start..out.text.len(),
                style,
                runs,
            });
            offset += line.len();
            if offset < source.len() {
                out.literal(
                    source,
                    offset,
                    offset + 1,
                    Marks::default(),
                    &mut Vec::new(),
                );
                offset += 1;
            }
        }
        out
    }

    fn hide(&mut self, start: usize, end: usize) {
        let b = self.boundaries.last_mut().unwrap();
        b.before = b.before.min(start);
        b.after = end;
    }
    fn literal(
        &mut self,
        source: &str,
        start: usize,
        end: usize,
        marks: Marks,
        runs: &mut Vec<Run>,
    ) {
        if start == end {
            return;
        }
        let display_start = self.text.len();
        self.boundaries.last_mut().unwrap().after = start;
        self.text.push_str(&source[start..end]);
        for at in start + 1..=end {
            self.boundaries.push(Boundary {
                before: at,
                after: at,
            });
        }
        if let Some(last) = runs.last_mut() {
            if last.marks == marks && last.range.end == display_start {
                last.range.end = self.text.len();
                return;
            }
        }
        runs.push(Run {
            range: display_start..self.text.len(),
            marks,
        });
    }
    fn inline(
        &mut self,
        source: &str,
        start: usize,
        end: usize,
        marks: Marks,
        runs: &mut Vec<Run>,
    ) {
        // Pair delimiter runs before projecting. A run can close one mark and
        // open another, or close part of a combined bold/italic opener.
        struct Delimiter {
            start: usize,
            len: usize,
            remaining: usize,
            open: bool,
            close: bool,
        }
        let mut delimiters: Vec<Delimiter> = Vec::new();
        let mut pairs = Vec::new();
        let mut at = start;
        while at < end {
            let ch = source[at..end].chars().next().unwrap();
            if ch == '\\' {
                at += 1;
                if at < end {
                    at += source[at..end].chars().next().unwrap().len_utf8();
                }
                continue;
            }
            if ch != '*' {
                at += ch.len_utf8();
                continue;
            }
            let len = source[at..end].bytes().take_while(|b| *b == b'*').count();
            let previous = source[start..at].chars().next_back();
            let next = source[at + len..end].chars().next();
            let open = next.is_some();
            let close = previous.is_some();
            let mut current = Delimiter { start: at, len, remaining: len, open, close };
            // Unsupported runs such as **** remain literal.
            if len <= 3 || close {
                while current.close && current.remaining > 0 {
                    let opener = delimiters.iter().rposition(|d| {
                        d.open && d.remaining > 0
                            && !source[d.start + d.len..current.start].trim().is_empty()
                            && !((d.close || current.open)
                                && (d.remaining + current.remaining) % 3 == 0
                                && (d.remaining % 3 != 0 || current.remaining % 3 != 0))
                    });
                    let Some(index) = opener else { break };
                    let d = &mut delimiters[index];
                    let count = if d.remaining >= 2 && current.remaining >= 2 { 2 } else { 1 };
                    let left = d.start + d.remaining - count;
                    let right = current.start + current.len - current.remaining;
                    pairs.push((left, right, count));
                    d.remaining -= count;
                    current.remaining -= count;
                    // Unmatched inner openers cannot cross this closed span.
                    delimiters.truncate(index + 1);
                }
                if current.open && current.remaining > 0 && current.remaining <= 3 {
                    // Remaining opening markers are at the right of this run.
                    current.start += current.len - current.remaining;
                    current.len = current.remaining;
                    delimiters.push(current);
                }
            }
            at += len;
        }
        let mut hidden = vec![false; end - start];
        let mut styled = vec![marks; end - start];
        for &(left, right, count) in &pairs {
            hidden[left - start..left + count - start].fill(true);
            hidden[right - start..right + count - start].fill(true);
            for mark in &mut styled[left + count - start..right - start] {
                if count == 2 { mark.bold = true; } else { mark.italic = true; }
            }
        }
        let display_base = self.text.len();
        let mut display_offsets = vec![0; end - start + 1];
        at = start;
        while at < end {
            display_offsets[at - start] = self.text.len();
            if hidden[at - start] {
                self.hide(at, at + 1);
                at += 1;
                continue;
            }
            let ch = source[at..end].chars().next().unwrap();
            if ch == '\\' && source[at + 1..end].starts_with(['*', '\\']) {
                self.hide(at, at + 1);
                at += 1;
                display_offsets[at - start] = self.text.len();
            }
            let next = at + source[at..end].chars().next().unwrap().len_utf8();
            self.literal(source, at, next, styled[at - start], runs);
            at = next;
        }
        display_offsets[end - start] = self.text.len();
        for (left, right, count) in pairs {
            let display_start = display_offsets[left + count - start].max(display_base);
            let display_end = display_offsets[right - start];
            self.emphasis.push(Emphasis {
                source: left..right + count,
                display: display_start..display_end,
                delimiter: "*".repeat(count),
            });
        }
    }
    pub fn source_position(&self, display: usize, trailing: bool) -> usize {
        let b = self.boundaries[display.min(self.text.len())];
        if trailing {
            b.after
        } else {
            b.before
        }
    }
    pub fn display_position(&self, source: usize) -> usize {
        self.boundaries
            .partition_point(|b| b.after < source)
            .min(self.text.len())
    }
    pub fn source_selection(&self, sel: ByteSelection) -> ByteSelection {
        if sel.is_empty() {
            return ByteSelection::caret(self.source_position(sel.cursor, true));
        }
        let start = self.source_position(sel.start(), true);
        let end = self.source_position(sel.end(), false);
        if sel.anchor <= sel.cursor {
            ByteSelection {
                anchor: start,
                cursor: end,
            }
        } else {
            ByteSelection {
                anchor: end,
                cursor: start,
            }
        }
    }
    pub fn display_selection(&self, sel: ByteSelection) -> ByteSelection {
        ByteSelection {
            anchor: self.display_position(sel.anchor),
            cursor: self.display_position(sel.cursor),
        }
    }
    pub fn marks_at(&self, at: usize) -> Marks {
        self.paragraphs
            .iter()
            .flat_map(|p| &p.runs)
            .find(|r| r.range.start < at && r.range.end >= at)
            .or_else(|| {
                self.paragraphs
                    .iter()
                    .flat_map(|p| &p.runs)
                    .find(|r| r.range.start == at)
            })
            .map(|r| r.marks)
            .unwrap_or_default()
    }
    pub fn format_selection(
        &self,
        source: &str,
        sel: ByteSelection,
        style: InlineStyle,
    ) -> (String, ByteSelection) {
        let sel = clamp_selection(&self.text, sel);
        if sel.is_empty() {
            return (source.to_string(), self.source_selection(sel));
        }
        let active = |m: Marks| match style {
            InlineStyle::Bold => m.bold,
            InlineStyle::Italic => m.italic,
        };
        let remove = self.paragraphs.iter().flat_map(|p| &p.runs)
            .filter(|r| r.range.start < sel.end() && r.range.end > sel.start())
            .all(|r| active(r.marks));
        let mut out = String::new();
        let mut copied = 0;
        for paragraph in &self.paragraphs {
            if paragraph.range.start >= sel.end() || paragraph.range.end <= sel.start() {
                continue;
            }
            let end = source[paragraph.source_start..].find('\n')
                .map_or(source.len(), |n| paragraph.source_start + n);
            out.push_str(&source[copied..paragraph.content_start]);
            let mut content = styled_chars(self, paragraph.range.clone());
            let mut offset = paragraph.range.start;
            for (ch, marks) in &mut content {
                if offset >= sel.start() && offset < sel.end() {
                    match style {
                        InlineStyle::Bold => marks.bold = !remove,
                        InlineStyle::Italic => marks.italic = !remove,
                    }
                }
                offset += ch.len_utf8();
            }
            write_styled(&content, &mut out);
            copied = end;
        }
        out.push_str(&source[copied..]);
        let next = Projection::new(&out);
        let selection = next.source_selection(sel);
        (out, selection)
    }

}

/// Apply display input to canonical source without round-tripping the document through plain text.
pub fn replace_display(
    source: &str,
    projection: &Projection,
    selection: ByteSelection,
    input: &str,
    pending: Option<Marks>,
) -> (String, ByteSelection) {
    let selection = clamp_selection(&projection.text, selection);
    let mut sel = projection.source_selection(selection);
    let mut insert = engine::normalize_newlines(input);
    if insert.contains('\n') {
        return replace_lines(source, projection, selection, &insert, pending);
    }
    let mut inherited = projection.marks_at(selection.start());
    let mut left_close = String::new();
    let mut right_open = String::new();
    if !selection.is_empty() {
        inherited = projection
            .paragraphs
            .iter()
            .flat_map(|p| &p.runs)
            .find(|r| r.range.start <= selection.start() && r.range.end > selection.start())
            .map(|r| r.marks)
            .unwrap_or_default();
        let mut start = sel.start();
        let mut end = sel.end();
        for span in &projection.emphasis {
            if selection.start() <= span.display.start && selection.end() >= span.display.end {
                start = start.min(span.source.start);
                end = end.max(span.source.end);
            }
        }
        let mut left: Vec<_> = projection
            .emphasis
            .iter()
            .filter(|s| s.source.start < start && s.source.end > start && s.source.end <= end)
            .collect();
        left.sort_by_key(|s| std::cmp::Reverse(s.source.start));
        for span in left {
            left_close.push_str(&span.delimiter);
        }
        let mut right: Vec<_> = projection
            .emphasis
            .iter()
            .filter(|s| s.source.start >= start && s.source.start < end && s.source.end > end)
            .collect();
        right.sort_by_key(|s| s.source.start);
        for span in right {
            right_open.push_str(&span.delimiter);
        }
        sel = ByteSelection {
            anchor: start,
            cursor: end,
        };
        // When an entire marked span was removed, its new replacement owns
        // its own marks. No empty delimiters are left in the source.
        if !projection
            .emphasis
            .iter()
            .any(|s| s.source.start < start && s.source.end > start)
        {
            if pending.is_none()
                && !insert.is_empty()
                && !insert.contains('\n')
                && inherited != Marks::default()
            {
                let marker = markers(inherited);
                insert = format!("{marker}{insert}{marker}");
            }
            inherited = Marks::default();
        }
    }
    let mut inner_end = insert.len();
    if !insert.is_empty() && !insert.contains('\n') && selection.is_empty() {
        let marks = pending.unwrap_or(inherited);
        let before = projection.source_position(selection.cursor, false);
        let after = projection.source_position(selection.cursor, true);
        let at_closing = projection
            .emphasis
            .iter()
            .any(|s| s.display.end == selection.cursor && s.source.end <= after);
        let at_opening = projection
            .emphasis
            .iter()
            .any(|s| s.display.start == selection.cursor && s.source.start >= before);
        if marks != inherited && at_closing {
            sel = ByteSelection::caret(after);
            let marker = markers(marks);
            inner_end = marker.len() + insert.len();
            insert = format!("{marker}{insert}{marker}");
        } else if marks != inherited && at_opening {
            sel = ByteSelection::caret(before);
            let marker = markers(marks);
            inner_end = marker.len() + insert.len();
            insert = format!("{marker}{insert}{marker}");
        } else {
            if at_closing {
                sel = ByteSelection::caret(before);
            }
            if marks != inherited {
                let old = markers(inherited);
                let new = markers(marks);
                inner_end += old.len() + new.len();
                insert = format!("{old}{new}{insert}{new}{old}");
            }
        }
    }
    inner_end += left_close.len();
    insert.push_str(&left_close);
    insert.push_str(&right_open);
    let start = sel.start();
    let end = sel.end();
    let mut next = String::with_capacity(source.len() + insert.len());
    next.push_str(&source[..start]);
    next.push_str(&insert);
    next.push_str(&source[end..]);
    (next, ByteSelection::caret(start + inner_end))
}

fn clamp_selection(text: &str, selection: ByteSelection) -> ByteSelection {
    let boundary = |at: usize| {
        let mut at = at.min(text.len());
        while !text.is_char_boundary(at) { at -= 1; }
        at
    };
    ByteSelection { anchor: boundary(selection.anchor), cursor: boundary(selection.cursor) }
}

fn styled_chars(projection: &Projection, range: Range<usize>) -> Vec<(char, Marks)> {
    let mut runs = projection.paragraphs.iter().flat_map(|p| &p.runs).peekable();
    projection.text[range.clone()].char_indices().map(|(i, ch)| {
        let at = range.start + i;
        while runs.peek().is_some_and(|r| r.range.end <= at) { runs.next(); }
        let marks = runs.peek().filter(|r| r.range.start <= at)
            .map(|r| r.marks).unwrap_or_default();
        (ch, marks)
    }).collect()
}

// Nest shared marks across adjacent runs instead of concatenating wrappers
// (which would create ambiguous runs of four or more asterisks).
fn write_styled(content: &[(char, Marks)], out: &mut String) {
    let mut at = 0;
    while at < content.len() {
        let marks = content[at].1;
        if marks == Marks::default() {
            let ch = content[at].0;
            if matches!(ch, '*' | '\\') { out.push('\\'); }
            out.push(ch);
            at += 1;
            continue;
        }
        let bold_end = if marks.bold {
            at + content[at..].iter().take_while(|(_, m)| m.bold).count()
        } else { at };
        let italic_end = if marks.italic {
            at + content[at..].iter().take_while(|(_, m)| m.italic).count()
        } else { at };
        let bold = bold_end >= italic_end;
        let end = bold_end.max(italic_end);
        let delimiter = if bold { "**" } else { "*" };
        let inner: Vec<_> = content[at..end].iter().map(|&(ch, mut m)| {
            if bold { m.bold = false; } else { m.italic = false; }
            (ch, m)
        }).collect();
        if inner.iter().any(|(ch, _)| !ch.is_whitespace()) {
            out.push_str(delimiter);
            write_styled(&inner, out);
            out.push_str(delimiter);
        } else {
            // Whitespace-only emphasis is unsupported; keep its text literal.
            for (ch, _) in inner { out.push(ch); }
        }
        at = end;
    }
}

fn replace_lines(
    source: &str,
    projection: &Projection,
    selection: ByteSelection,
    input: &str,
    pending: Option<Marks>,
) -> (String, ByteSelection) {
    let first = projection.paragraphs.iter()
        .rposition(|p| p.range.start <= selection.start()).unwrap();
    let last = projection.paragraphs.iter()
        .rposition(|p| p.range.start <= selection.end()).unwrap();
    let p = &projection.paragraphs[first];
    let q = &projection.paragraphs[last];
    let end = source[q.source_start..].find('\n').map_or(source.len(), |n| q.source_start + n);
    let continuation = if input == "\n" && selection.is_empty() && first > 0 {
        match p.style {
            ParagraphStyle::Bullet => "- ".to_string(),
            ParagraphStyle::Check(_) => "- [ ] ".to_string(),
            ParagraphStyle::Number(n) => format!("{}. ", n.saturating_add(1)),
            _ => String::new(),
        }
    } else { String::new() };
    if !continuation.is_empty() && projection.text[p.range.clone()].trim().is_empty() {
        let mut out = source.to_string();
        out.replace_range(p.source_start..p.content_start, "");
        return (out, ByteSelection::caret(p.source_start));
    }
    let mut content = styled_chars(projection, p.range.start..selection.start());
    let inherited = projection.marks_at(selection.start());
    content.extend(input.chars().map(|ch| (ch, pending.unwrap_or(inherited))));
    content.extend(styled_chars(projection, selection.end()..q.range.end));
    let mut out = source[..p.content_start].to_string();
    for (index, line) in content.split(|(ch, _)| *ch == '\n').enumerate() {
        if index > 0 {
            out.push('\n');
            out.push_str(&continuation);
        }
        write_styled(line, &mut out);
    }
    out.push_str(&source[end..]);
    let next = Projection::new(&out);
    let caret = next.source_position(selection.start() + input.len(), true);
    (out, ByteSelection::caret(caret))
}

fn markers(marks: Marks) -> &'static str {
    match (marks.bold, marks.italic) {
        (true, true) => "***",
        (true, false) => "**",
        (false, true) => "*",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formatting_transforms_marks_without_removing_other_styles() {
        for word in ["hello", "Café 漢字"] {
            let source = format!("T\n**{word}**");
            let p = Projection::new(&source);
            let selection = ByteSelection { anchor: 2, cursor: p.text.len() };
            let (out, selection) = p.format_selection(&source, selection, InlineStyle::Italic);
            assert_eq!(out, format!("T\n***{word}***"));
            let p = Projection::new(&out);
            assert_eq!(p.text, format!("T\n{word}"));
            assert_eq!(p.marks_at(3), Marks { bold: true, italic: true });
            let (out, _) = p.format_selection(&out, p.display_selection(selection), InlineStyle::Bold);
            assert_eq!(out, format!("T\n*{word}*"));
        }
        let source = "T\n**hello**";
        let p = Projection::new(source);
        let (out, _) = p.format_selection(source, ByteSelection { anchor: 3, cursor: 6 }, InlineStyle::Italic);
        assert_eq!(out, "T\n**h*ell*o**");
        let p = Projection::new(&out);
        assert_eq!(p.text, "T\nhello");
        assert_eq!(p.marks_at(4), Marks { bold: true, italic: true });
        assert_eq!(p.marks_at(7), Marks { bold: true, italic: false });
    }

    #[test]
    fn serialized_mixed_marks_round_trip_without_literal_delimiters() {
        let options = [Marks::default(), Marks { bold: true, italic: false },
            Marks { bold: false, italic: true }, Marks { bold: true, italic: true }];
        for letters in [['é', '漢', '字'], ['*', '\\', '🙂'], [' ', 'é', ' ']] {
            for a in options { for b in options { for c in options {
                let chars = [(letters[0], a), (letters[1], b), (letters[2], c)];
                let mut source = "T\n".to_string();
                write_styled(&chars, &mut source);
                let p = Projection::new(&source);
                assert_eq!(p.text, format!("T\n{}", letters.into_iter().collect::<String>()), "{source}");
                let visible: Vec<_> = styled_chars(&p, 2..p.text.len()).into_iter()
                    .filter(|(ch, _)| !ch.is_whitespace()).collect();
                let expected: Vec<_> = chars.into_iter().filter(|(ch, _)| !ch.is_whitespace()).collect();
                assert_eq!(visible, expected, "{source}");
            } } }
        }
    }

    #[test]
    fn multiline_paste_balances_each_line_and_preserves_unicode_selection() {
        for (input, expected) in [("A\nB", "T\nheA\nBllo"),
            ("é\r\n漢\n\n🙂", "T\nheé\n漢\n\n🙂llo")] {
            let source = "T\n**hello**";
            let p = Projection::new(source);
            let (out, caret) = replace_display(source, &p, ByteSelection::caret(4), input, None);
            let p = Projection::new(&out);
            assert_eq!(p.text, expected, "{out}");
            assert!(out.is_char_boundary(caret.cursor));
            assert_eq!(p.display_selection(caret).cursor, 4 + engine::normalize_newlines(input).len());
            assert!(p.paragraphs.iter().skip(1).flat_map(|p| &p.runs).all(|r| r.marks.bold));
        }
        let source = "T\n**Café**\n- [x] *漢字*";
        let p = Projection::new(source);
        let (out, _) = replace_display(source, &p, ByteSelection {
            anchor: p.text.len() - "字".len(), cursor: 3,
        }, "A\nB", None);
        let next = Projection::new(&out);
        assert_eq!(next.text, "T\nCA\nB字", "{out}");
        assert!(next.marks_at(next.text.len()).italic);
    }

    #[test]
    fn projection_preserves_literals_and_maps_unicode() {
        let src = "Title\n\n**Café 👨‍👩‍👧‍👦** and *é*\n- [ ] Milk\n2 * 3\n****\n```\n**literal**\n```";
        let p = Projection::new(src);
        assert!(p.text.contains("Café 👨‍👩‍👧‍👦 and é"));
        assert!(p.text.contains("2 * 3\n****"));
        assert!(p.text.contains("**literal**"));
        let start = p.text.find("Café").unwrap();
        let end = start + "Café 👨‍👩‍👧‍👦".len();
        let sel = p.source_selection(ByteSelection {
            anchor: start,
            cursor: end,
        });
        assert_eq!(&src[sel.start()..sel.end()], "Café 👨‍👩‍👧‍👦");
        for (at, _) in p.text.char_indices() {
            assert!(src.is_char_boundary(p.source_position(at, true)));
            assert_eq!(p.display_position(p.source_position(at, true)), at);
        }
    }
    #[test]
    fn formatted_selection_toggles_and_pending_marks_type_without_empty_delimiters() {
        let src = "Gift ideas\n**Check dates before booking.**";
        let p = Projection::new(src);
        let start = p.text.find("Check").unwrap();
        let (plain, _) = p.format_selection(
            src,
            ByteSelection {
                anchor: start,
                cursor: p.text.len(),
            },
            InlineStyle::Bold,
        );
        assert_eq!(plain, "Gift ideas\nCheck dates before booking.");
        let p = Projection::new(&plain);
        let (typed, sel) = replace_display(
            &plain,
            &p,
            ByteSelection::caret(p.text.len()),
            "Hi",
            Some(Marks {
                bold: true,
                italic: false,
            }),
        );
        assert!(typed.ends_with("**Hi**"));
        assert!(!typed.contains("****"));
        let p = Projection::new(&typed);
        let (typed, _) = replace_display(&typed, &p, p.display_selection(sel), "!", None);
        assert!(typed.ends_with("**Hi!**"));
    }
    #[test]
    fn return_continues_and_exits_lists() {
        for (src, expected) in [
            ("T", "T\n"),
            ("T\n- [x] Milk", "T\n- [x] Milk\n- [ ] "),
            ("T\n8. Eight", "T\n8. Eight\n9. "),
            ("T\n- ", "T\n"),
        ] {
            let p = Projection::new(src);
            assert_eq!(
                replace_display(src, &p, ByteSelection::caret(p.text.len()), "\n", None).0,
                expected
            );
        }
    }
    #[test]
    fn editing_mark_boundaries_never_creates_empty_markers() {
        let src = "T\n**hello**";
        let p = Projection::new(src);
        let (out, _) = replace_display(
            src,
            &p,
            ByteSelection::caret(p.text.len()),
            " world",
            Some(Marks::default()),
        );
        assert_eq!(out, "T\n**hello** world");
        let start = p.text.find("hello").unwrap();
        assert_eq!(
            replace_display(
                src,
                &p,
                ByteSelection {
                    anchor: start,
                    cursor: p.text.len()
                },
                "",
                None
            )
            .0,
            "T\n"
        );
        assert_eq!(
            replace_display(
                src,
                &p,
                ByteSelection {
                    anchor: 0,
                    cursor: p.text.len()
                },
                "",
                None
            )
            .0,
            ""
        );
        let (out, _) = replace_display(src, &p, ByteSelection::caret(start + 2), "\n", None);
        assert_eq!(Projection::new(&out).text, "T\nhe\nllo");
        let src = "T\n**hello** world";
        let p = Projection::new(src);
        let start = p.text.find("llo").unwrap();
        let (out, _) = replace_display(
            src,
            &p,
            ByteSelection {
                anchor: start,
                cursor: p.text.len(),
            },
            "!",
            None,
        );
        assert_eq!(Projection::new(&out).text, "T\nhe!");
    }
}
