//! How things look in the terminal: the style (colour or plain, Unicode or
//! ASCII marks), the screen (finished rows go to the scrollback; the live
//! area at the bottom, with the prompt, is redrawn in place), and the rows
//! for tools, diffs and command output.

use crate::text::{self, truncate, wrap};
use makepad_code_agent::event::{Diff, DiffLine};
use std::io::Write;

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
    pub ascii: bool,
}

impl Style {
    /// From the environment: colour unless NO_COLOR or `tty` is false;
    /// ASCII marks on the bare console (TERM=linux) or without a UTF-8 locale.
    pub fn detect(tty: bool) -> Style {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) || var("TERM") == "dumb";
        let locale = [var("LC_ALL"), var("LC_CTYPE"), var("LANG")].into_iter().find(|v| !v.is_empty()).unwrap_or_default();
        let utf8 = locale.to_ascii_lowercase().replace('-', "").contains("utf8");
        Style { color: tty && !no_color, ascii: var("TERM") == "linux" || !utf8 }
    }

    fn sgr(&self, code: &str, s: &str) -> String {
        if self.color && !s.is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn dim(&self, s: &str) -> String {
        self.sgr("2", s)
    }
    pub fn bold(&self, s: &str) -> String {
        self.sgr("1", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.sgr("31", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.sgr("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.sgr("33", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.sgr("36", s)
    }

    pub fn prompt(&self) -> &'static str {
        if self.ascii { "> " } else { "› " }
    }
    pub fn ok(&self) -> &'static str {
        if self.ascii { "+" } else { "✓" }
    }
    pub fn fail(&self) -> &'static str {
        if self.ascii { "x" } else { "✗" }
    }
    /// A command that returned early and runs on in the background.
    pub fn yielded(&self) -> &'static str {
        if self.ascii { ">" } else { "»" }
    }
    pub fn ellipsis(&self) -> &'static str {
        if self.ascii { "..." } else { "…" }
    }
    pub fn dot(&self) -> &'static str {
        if self.ascii { " - " } else { " · " }
    }
    pub fn spinner(&self, tick: usize) -> &'static str {
        const UNI: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        const ASC: [&str; 4] = ["|", "/", "-", "\\"];
        if self.ascii { ASC[tick % 4] } else { UNI[tick % 10] }
    }

    /// Text from elsewhere (the agent's status lines) with its Unicode marks
    /// swapped for ASCII when needed.
    pub fn plain<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        if self.ascii && !s.is_ascii() {
            s.replace(" · ", " - ").replace('·', "-").replace('…', "...").replace(['—', '–'], "-").replace('→', "->").into()
        } else {
            s.into()
        }
    }
}

// ------------------------------------------------------------------- screen

/// The terminal as two parts: rows written once into the scrollback, and
/// the live area under them (streaming line, running tools, status,
/// prompt), cleared and redrawn on every change. Without a live area
/// (output not a terminal) rows are just printed.
pub struct Screen {
    pub style: Style,
    pub live: bool,
    pub cols: usize,
    pub rows: usize,
    /// Rows of the live area on screen, and the one the cursor is on.
    drawn: usize,
    cursor_row: usize,
    /// Text written without a newline (plain mode streaming).
    open_line: bool,
    out: Vec<u8>,
}

impl Screen {
    pub fn new(style: Style, live: bool) -> Screen {
        let (cols, rows) = if live { crate::term::size(1) } else { (100, 40) };
        Screen { style, live, cols, rows, drawn: 0, cursor_row: 0, open_line: false, out: Vec::new() }
    }

    /// The width rows are wrapped to: one short of the edge, so no row
    /// leaves the cursor in the terminal's pending-wrap state.
    pub fn width(&self) -> usize {
        self.cols.saturating_sub(1).max(20)
    }

    pub fn resized(&mut self) {
        if self.live {
            let (cols, rows) = crate::term::size(1);
            self.cols = cols;
            self.rows = rows;
        }
    }

    fn clear_live(&mut self) {
        if self.live && self.drawn > 0 {
            self.out.extend_from_slice(b"\r");
            if self.cursor_row > 0 {
                self.out.extend_from_slice(format!("\x1b[{}A", self.cursor_row).as_bytes());
            }
            self.out.extend_from_slice(b"\x1b[J");
            self.drawn = 0;
            self.cursor_row = 0;
        }
    }

    /// Rows for the scrollback, above the live area.
    pub fn commit(&mut self, rows: &[String]) {
        if rows.is_empty() {
            return;
        }
        if self.live {
            self.out.extend_from_slice(b"\x1b[?25l");
        }
        self.clear_live();
        if self.open_line {
            self.out.extend_from_slice(b"\n");
            self.open_line = false;
        }
        for row in rows {
            self.out.extend_from_slice(row.as_bytes());
            self.out.extend_from_slice(if self.live { b"\x1b[K\r\n" } else { b"\n" });
        }
    }

    /// Text as it streams, when there is no live area to hold a partial
    /// line: written straight through.
    pub fn stream_raw(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
        self.open_line = !text.ends_with('\n') && (self.open_line || !text.is_empty());
        if text.ends_with('\n') {
            self.open_line = false;
        }
    }

    /// Draw the live area: `rows` (each within `width()`), the cursor at
    /// (row, column) of them.
    pub fn draw_live(&mut self, rows: &[String], cursor: (usize, usize)) {
        if !self.live {
            return;
        }
        self.out.extend_from_slice(b"\x1b[?25l");
        self.clear_live();
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                self.out.extend_from_slice(b"\r\n");
            }
            self.out.extend_from_slice(row.as_bytes());
            self.out.extend_from_slice(b"\x1b[K");
        }
        let last = rows.len().saturating_sub(1);
        let (row, col) = (cursor.0.min(last), cursor.1);
        if last > row {
            self.out.extend_from_slice(format!("\x1b[{}A", last - row).as_bytes());
        }
        self.out.extend_from_slice(b"\r");
        if col > 0 {
            self.out.extend_from_slice(format!("\x1b[{col}C").as_bytes());
        }
        self.out.extend_from_slice(b"\x1b[?25h");
        self.drawn = rows.len();
        self.cursor_row = row;
    }

    /// Clear the whole screen (the scrollback above is left alone).
    pub fn clear_screen(&mut self) {
        self.out.extend_from_slice(b"\x1b[H\x1b[2J");
        self.drawn = 0;
        self.cursor_row = 0;
    }

    /// Leave the live area as it is and move under it (on exit).
    pub fn finish(&mut self) {
        if self.live && self.drawn > 0 {
            let below = self.drawn - 1 - self.cursor_row.min(self.drawn - 1);
            if below > 0 {
                self.out.extend_from_slice(format!("\x1b[{below}B").as_bytes());
            }
            self.out.extend_from_slice(b"\r\n");
            self.drawn = 0;
        } else if self.open_line {
            self.out.extend_from_slice(b"\n");
            self.open_line = false;
        }
        self.flush();
    }

    pub fn flush(&mut self) {
        if self.out.is_empty() {
            return;
        }
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(&self.out);
        let _ = stdout.flush();
        self.out.clear();
    }

    /// What was written since the last flush (tests).
    #[cfg(test)]
    pub fn take(&mut self) -> String {
        String::from_utf8_lossy(&std::mem::take(&mut self.out)).into_owned()
    }
}

// --------------------------------------------------------------------- rows

/// A tool's state, as its row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Fail,
    /// Running; the spinner's frame.
    Running(usize),
    /// Waiting for the person's approval.
    Asking,
    /// Started (line mode, where nothing redraws).
    Started,
    /// Returned early (a failure showed, or its yield time passed); the
    /// command runs on.
    Yielded,
    /// Still running in the background; the spinner's frame.
    Background(usize),
}

impl Style {
    fn mark(&self, mark: Mark) -> (&'static str, fn(&Style, &str) -> String) {
        match mark {
            Mark::Ok => (self.ok(), Style::green),
            Mark::Fail => (self.fail(), Style::red),
            Mark::Running(t) => (self.spinner(t), Style::cyan),
            Mark::Asking => ("?", Style::yellow),
            Mark::Started => ("-", Style::dim),
            Mark::Yielded => (self.yielded(), Style::yellow),
            Mark::Background(t) => (self.spinner(t), Style::yellow),
        }
    }
}

/// One tool as a row: state mark, title, then the summary dim.
pub fn tool_row(style: &Style, mark: Mark, title: &str, summary: &str, indent: &str, width: usize) -> String {
    let (glyph, paint) = style.mark(mark);
    let title = text::sanitize(&title.replace('\n', " "));
    let summary = text::sanitize(&summary.replace('\n', " "));
    let room = width.saturating_sub(text::width(indent) + text::width(glyph) + 1);
    let title = truncate(&title, room.saturating_sub(if summary.is_empty() { 0 } else { 12.min(room / 3) }), style.ellipsis());
    let left = room.saturating_sub(text::width(&title));
    let summary = if summary.is_empty() || left < 6 { String::new() } else { truncate(&summary, left - 2, style.ellipsis()) };
    let mut row = format!("{indent}{} {}", paint(style, glyph), style.bold(&title));
    if !summary.is_empty() {
        row.push_str("  ");
        row.push_str(&style.dim(&summary));
    }
    row
}

/// The last `keep` lines of command output, and how many came before them.
pub fn fold_output(output: &str, keep: usize) -> (usize, Vec<String>) {
    let lines: Vec<&str> = output.trim_end_matches('\n').split('\n').collect();
    if output.trim().is_empty() {
        return (0, Vec::new());
    }
    let cut = lines.len().saturating_sub(keep);
    (cut, lines[cut..].iter().map(|l| l.to_string()).collect())
}

/// Command output rows: indented, dim, cut to the width (output is not
/// prose; a cut line beats a broken table).
pub fn output_rows(style: &Style, lines: &[String], hidden: usize, indent: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    if hidden > 0 {
        rows.push(style.dim(&format!("{indent}{} {hidden} more line{} above", style.ellipsis(), if hidden == 1 { "" } else { "s" })));
    }
    let room = width.saturating_sub(text::width(indent));
    for line in lines {
        rows.push(style.dim(&format!("{indent}{}", truncate(&text::sanitize(line), room, style.ellipsis()))));
    }
    rows
}

/// A diff in colour: the path, then each hunk's head and its lines.
/// `picks` (approval time) marks each hunk as ticked or not, numbered.
pub fn diff_rows(style: &Style, diff: &Diff, picks: Option<&[bool]>, indent: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for h in &diff.hunks {
        for (kind, _) in &h.lines {
            match kind {
                DiffLine::Added => added += 1,
                DiffLine::Removed => removed += 1,
                DiffLine::Context => {}
            }
        }
    }
    rows.push(format!("{indent}{}  {}", style.bold(&diff.path), style.dim(&format!("+{added} -{removed}"))));
    if diff.hunks.is_empty() {
        rows.push(style.dim(&format!("{indent}  (no changes)")));
    }
    let room = width.saturating_sub(text::width(indent) + 1);
    for (i, h) in diff.hunks.iter().enumerate() {
        let (old_n, new_n) = h.lines.iter().fold((0, 0), |(o, n), (k, _)| match k {
            DiffLine::Context => (o + 1, n + 1),
            DiffLine::Added => (o, n + 1),
            DiffLine::Removed => (o + 1, n),
        });
        let mut head = format!("@@ -{},{old_n} +{},{new_n} @@", h.old_start, h.new_start);
        if let Some(picks) = picks {
            let on = picks.get(i).copied().unwrap_or(true);
            let tick = if on { if style.ascii { "[x]" } else { "[✓]" } } else { "[ ]" };
            head = format!("{} {tick} {head}", i + 1);
            rows.push(format!("{indent}{}", if on { style.cyan(&head) } else { style.dim(&head) }));
        } else {
            rows.push(format!("{indent}{}", style.cyan(&head)));
        }
        for (kind, line) in &h.lines {
            let line = text::sanitize(line.trim_end_matches(['\n', '\r']));
            let (sign, paint): (&str, fn(&Style, &str) -> String) = match kind {
                DiffLine::Added => ("+", Style::green),
                DiffLine::Removed => ("-", Style::red),
                DiffLine::Context => (" ", Style::dim),
            };
            let shown = truncate(&format!("{sign}{line}"), room, style.ellipsis());
            rows.push(format!("{indent}{}", paint(style, &shown)));
        }
    }
    rows
}

/// Prose (assistant text, notices) as rows within the width.
pub fn prose_rows(text: &str, width: usize, indent: &str) -> Vec<String> {
    wrap(&text::sanitize(text), width.saturating_sub(text::width(indent)), "")
        .into_iter()
        .map(|r| format!("{indent}{r}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_code_agent::event::Hunk;

    const PLAIN: Style = Style { color: false, ascii: true };
    const COLOR: Style = Style { color: true, ascii: false };

    fn sample() -> Diff {
        Diff {
            path: "src/main.rs".into(),
            hunks: vec![
                Hunk { old_start: 3, new_start: 3, lines: vec![(DiffLine::Context, "fn main() {".into()), (DiffLine::Removed, "    old();".into()), (DiffLine::Added, "    new();".into()), (DiffLine::Added, "    more();".into())] },
                Hunk { old_start: 20, new_start: 21, lines: vec![(DiffLine::Removed, "gone".into())] },
            ],
        }
    }

    #[test]
    fn diff_plain() {
        let rows = diff_rows(&PLAIN, &sample(), None, "  ", 60);
        assert_eq!(rows, [
            "  src/main.rs  +2 -2",
            "  @@ -3,2 +3,3 @@",
            "   fn main() {",
            "  -    old();",
            "  +    new();",
            "  +    more();",
            "  @@ -20,1 +21,0 @@",
            "  -gone",
        ]);
    }

    #[test]
    fn diff_colours_and_picks() {
        let rows = diff_rows(&COLOR, &sample(), Some(&[true, false]), "", 60);
        assert!(rows[1].starts_with("\x1b[36m1 [✓] @@ -3,2 +3,3 @@"));
        assert_eq!(rows[3], "\x1b[31m-    old();\x1b[0m");
        assert_eq!(rows[4], "\x1b[32m+    new();\x1b[0m");
        assert!(rows[6].starts_with("\x1b[2m2 [ ] @@"));
    }

    #[test]
    fn long_diff_lines_are_cut() {
        let mut d = sample();
        d.hunks[0].lines[1].1 = "x".repeat(200);
        let rows = diff_rows(&PLAIN, &d, None, "", 40);
        assert!(rows.iter().all(|r| text::width(r) <= 39), "{rows:?}");
        assert!(rows[3].ends_with("..."));
    }

    #[test]
    fn tool_rows_and_folding() {
        let row = tool_row(&PLAIN, Mark::Ok, "read src/main.rs:1-80", "80 lines", "", 60);
        assert_eq!(row, "+ read src/main.rs:1-80  80 lines");
        let row = tool_row(&PLAIN, Mark::Fail, &"$ cargo build ".repeat(10), "exit 0", "", 40);
        assert!(text::width(&row) <= 40, "{row}");
        let (hidden, tail) = fold_output("a\nb\nc\nd\n", 2);
        assert_eq!((hidden, tail.as_slice()), (2, &["c".to_string(), "d".to_string()][..]));
        let rows = output_rows(&PLAIN, &tail, hidden, "    ", 40);
        assert_eq!(rows, ["    ... 2 more lines above", "    c", "    d"]);
        assert_eq!(fold_output("", 3), (0, vec![]));
        let row = tool_row(&PLAIN, Mark::Yielded, "$ cargo test", "early yield `error[E` - run 7", "", 60);
        assert_eq!(row, "> $ cargo test  early yield `error[E` - run 7");
    }

    #[test]
    fn screen_commits_above_the_live_area() {
        let mut s = Screen { style: PLAIN, live: true, cols: 40, rows: 10, drawn: 0, cursor_row: 0, open_line: false, out: Vec::new() };
        s.draw_live(&["status".into(), "> hi".into()], (1, 4));
        let first = s.take();
        assert!(first.contains("status\x1b[K\r\n> hi\x1b[K\r\x1b[4C"));
        s.commit(&["done".into()]);
        let second = s.take();
        // Back to the top of the live area, clear it, then the row.
        assert!(second.contains("\r\x1b[1A\x1b[Jdone\x1b[K\r\n"), "{second:?}");
    }

    #[test]
    fn ascii_marks() {
        assert_eq!(PLAIN.plain("Loading · 42% · about 3 s left"), "Loading - 42% - about 3 s left");
        assert_eq!(PLAIN.prompt(), "> ");
    }
}
