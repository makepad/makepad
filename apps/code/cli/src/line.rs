//! The prompt's line editor: a buffer of characters and a cursor, the
//! history (kept in a file between runs), and the layout of the prompt
//! into terminal rows with the cursor's place.

use crate::term::Key;
use crate::text::char_width;
use std::path::PathBuf;

/// What a key did to the prompt.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    /// The buffer or cursor changed (redraw).
    Changed,
    /// Enter on a finished line: here is the text.
    Submit(String),
    /// Not an editing key: the caller decides (Ctrl+C, Ctrl+D, Esc, Tab...).
    Pass(Key),
}

#[derive(Default)]
pub struct LineEditor {
    pub buf: Vec<char>,
    pub cursor: usize,
    history: Vec<String>,
    /// Where Up/Down is in the history; `None` at the draft.
    browsing: Option<usize>,
    /// The line being typed before browsing started.
    draft: Vec<char>,
    file: Option<PathBuf>,
}

const HISTORY_MAX: usize = 1000;

impl LineEditor {
    /// An editor whose history lives in `file` (one entry per line,
    /// newlines and backslashes escaped).
    pub fn with_history(file: Option<PathBuf>) -> Self {
        let history = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| s.lines().filter(|l| !l.is_empty()).map(unescape).collect())
            .unwrap_or_default();
        LineEditor { history, file, ..Default::default() }
    }

    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.browsing = None;
    }

    pub fn set(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.buf.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// Remember a submitted line (also on disk).
    pub fn remember(&mut self, line: &str) {
        if line.trim().is_empty() || self.history.last().is_some_and(|l| l == line) {
            return;
        }
        self.history.push(line.to_string());
        if self.history.len() > HISTORY_MAX {
            let cut = self.history.len() - HISTORY_MAX;
            self.history.drain(..cut);
        }
        if let Some(file) = &self.file {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let body: String = self.history.iter().map(|l| escape(l) + "\n").collect();
            let _ = std::fs::write(file, body);
        }
    }

    pub fn key(&mut self, key: Key) -> Edit {
        match key {
            Key::Char(c) => {
                self.buf.insert(self.cursor, c);
                self.cursor += 1;
            }
            Key::Paste(text) => self.insert_str(&text),
            Key::AltEnter => self.insert_str("\n"),
            Key::Enter => {
                // A trailing backslash continues the prompt on a new line.
                if self.cursor == self.buf.len() && self.buf.last() == Some(&'\\') {
                    self.buf.pop();
                    self.buf.push('\n');
                    self.cursor = self.buf.len();
                    return Edit::Changed;
                }
                let line = self.text();
                self.clear();
                return Edit::Submit(line);
            }
            Key::Left | Key::Ctrl('b') => self.cursor = self.cursor.saturating_sub(1),
            Key::Right | Key::Ctrl('f') => self.cursor = (self.cursor + 1).min(self.buf.len()),
            Key::Home | Key::Ctrl('a') => self.cursor = self.line_start(),
            Key::End | Key::Ctrl('e') => self.cursor = self.line_end(),
            Key::WordLeft => self.cursor = self.word_start(),
            Key::WordRight => {
                while self.cursor < self.buf.len() && self.buf[self.cursor].is_whitespace() {
                    self.cursor += 1;
                }
                while self.cursor < self.buf.len() && !self.buf[self.cursor].is_whitespace() {
                    self.cursor += 1;
                }
            }
            Key::Backspace | Key::Ctrl('h') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.buf.remove(self.cursor);
                }
            }
            Key::Delete => {
                if self.cursor < self.buf.len() {
                    self.buf.remove(self.cursor);
                }
            }
            Key::Ctrl('d') if !self.buf.is_empty() => {
                if self.cursor < self.buf.len() {
                    self.buf.remove(self.cursor);
                }
            }
            Key::Ctrl('k') => {
                let end = self.line_end();
                // At a line's end, join the next line.
                let end = if end == self.cursor && end < self.buf.len() { end + 1 } else { end };
                self.buf.drain(self.cursor..end);
            }
            Key::Ctrl('u') => {
                let start = self.line_start();
                self.buf.drain(start..self.cursor);
                self.cursor = start;
            }
            Key::Ctrl('w') | Key::Alt('\u{8}') => {
                let start = self.word_start();
                self.buf.drain(start..self.cursor);
                self.cursor = start;
            }
            Key::Up | Key::Ctrl('p') => self.history_step(true),
            Key::Down | Key::Ctrl('n') => self.history_step(false),
            other => return Edit::Pass(other),
        }
        if !matches!(self.browsing, Some(_)) {
            self.draft.clear();
        }
        Edit::Changed
    }

    fn line_start(&self) -> usize {
        self.buf[..self.cursor].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.buf[self.cursor..].iter().position(|&c| c == '\n').map_or(self.buf.len(), |i| self.cursor + i)
    }

    fn word_start(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.buf[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.buf[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn history_step(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.browsing, back) {
            (None, true) => {
                self.draft = self.buf.clone();
                Some(self.history.len() - 1)
            }
            (None, false) => return,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
        };
        self.browsing = next;
        self.buf = match next {
            Some(i) => self.history[i].chars().collect(),
            None => std::mem::take(&mut self.draft),
        };
        self.cursor = self.buf.len();
    }

    /// The prompt laid out in rows at most `width` cells wide: the first row
    /// starts with `prompt`, later ones with `cont` (same width). Returns the
    /// rows and the cursor's (row, column).
    pub fn layout(&self, prompt: &str, cont: &str, width: usize) -> (Vec<String>, (usize, usize)) {
        let pw = crate::text::width(prompt);
        let width = width.max(pw + 4);
        let mut rows = vec![prompt.to_string()];
        let mut col = pw;
        let mut cursor = (0, pw);
        for (i, &c) in self.buf.iter().enumerate() {
            if i == self.cursor {
                cursor = (rows.len() - 1, col);
            }
            if c == '\n' {
                rows.push(cont.to_string());
                col = pw;
                continue;
            }
            let shown = if c.is_control() { '?' } else { c };
            let cw = char_width(shown);
            if col + cw > width {
                rows.push(cont.to_string());
                col = pw;
                if i == self.cursor {
                    cursor = (rows.len() - 1, col);
                }
            }
            rows.last_mut().unwrap().push(shown);
            col += cw;
        }
        if self.cursor >= self.buf.len() {
            if col >= width {
                rows.push(cont.to_string());
                col = pw;
            }
            cursor = (rows.len() - 1, col);
        }
        (rows, cursor)
    }

    /// The word the cursor ends, from its start: for completion.
    pub fn word_before_cursor(&self) -> (usize, String) {
        let start = self.buf[..self.cursor].iter().rposition(|c| c.is_whitespace()).map_or(0, |i| i + 1);
        (start, self.buf[start..self.cursor].iter().collect())
    }

    /// Replace `buf[start..cursor]` with `with`.
    pub fn replace_before_cursor(&mut self, start: usize, with: &str) {
        self.buf.drain(start..self.cursor);
        self.cursor = start;
        self.insert_str(with);
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(keys: Vec<Key>) -> LineEditor {
        let mut e = LineEditor::default();
        for k in keys {
            e.key(k);
        }
        e
    }

    fn chars(s: &str) -> Vec<Key> {
        s.chars().map(Key::Char).collect()
    }

    #[test]
    fn typing_and_moving() {
        let mut keys = chars("helo");
        keys.extend([Key::Left, Key::Char('l'), Key::Home, Key::Char('>'), Key::End, Key::Char('!')]);
        let e = typed(keys);
        assert_eq!(e.text(), ">hello!");
        assert_eq!(e.cursor, 7);
    }

    #[test]
    fn deleting() {
        let mut keys = chars("one two three");
        keys.push(Key::Ctrl('w'));
        assert_eq!(typed(keys.clone()).text(), "one two ");
        keys.extend([Key::Ctrl('a'), Key::Delete, Key::Ctrl('e'), Key::Backspace]);
        assert_eq!(typed(keys).text(), "ne two");
        let mut keys = chars("abc def");
        keys.extend([Key::Left, Key::Left, Key::Ctrl('k')]);
        assert_eq!(typed(keys.clone()).text(), "abc d");
        keys.push(Key::Ctrl('u'));
        assert_eq!(typed(keys).text(), "");
    }

    #[test]
    fn words() {
        let mut keys = chars("alpha beta");
        keys.extend([Key::WordLeft, Key::Char('X')]);
        assert_eq!(typed(keys).text(), "alpha Xbeta");
    }

    #[test]
    fn newlines_and_submit() {
        let mut e = LineEditor::default();
        for k in chars("first\\") {
            e.key(k);
        }
        assert_eq!(e.key(Key::Enter), Edit::Changed);
        e.key(Key::Char('x'));
        e.key(Key::AltEnter);
        e.key(Key::Paste("p1\np2".into()));
        assert_eq!(e.key(Key::Enter), Edit::Submit("first\nx\np1\np2".into()));
        assert!(e.is_empty());
        assert_eq!(e.key(Key::Ctrl('c')), Edit::Pass(Key::Ctrl('c')));
        // Ctrl+D deletes on a line with text, passes on an empty one.
        assert_eq!(e.key(Key::Ctrl('d')), Edit::Pass(Key::Ctrl('d')));
    }

    #[test]
    fn history_browsing_and_file() {
        let file = std::env::temp_dir().join(format!("code-cli-history-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut e = LineEditor::with_history(Some(file.clone()));
        e.remember("one");
        e.remember("two\nlines \\ here");
        for k in chars("draft") {
            e.key(k);
        }
        e.key(Key::Up);
        assert_eq!(e.text(), "two\nlines \\ here");
        e.key(Key::Up);
        assert_eq!(e.text(), "one");
        e.key(Key::Up);
        assert_eq!(e.text(), "one");
        e.key(Key::Down);
        e.key(Key::Down);
        assert_eq!(e.text(), "draft");
        let again = LineEditor::with_history(Some(file.clone()));
        assert_eq!(again.history, ["one", "two\nlines \\ here"]);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn layout_wraps_and_places_the_cursor() {
        let mut e = LineEditor::default();
        e.set("abcdefgh");
        let (rows, cur) = e.layout("> ", "  ", 6);
        // The cursor after a full row starts the next one.
        assert_eq!(rows, ["> abcd", "  efgh", "  "]);
        assert_eq!(cur, (2, 2));
        e.set("ab\ncd");
        e.cursor = 1;
        let (rows, cur) = e.layout("> ", "  ", 20);
        assert_eq!(rows, ["> ab", "  cd"]);
        assert_eq!(cur, (0, 3));
        e.set("日本語");
        let (rows, _) = e.layout("> ", "  ", 6);
        assert_eq!(rows, ["> 日本", "  語"]);
    }

    #[test]
    fn completion_word() {
        let mut e = LineEditor::default();
        e.set("look at @src/ma");
        let (start, word) = e.word_before_cursor();
        assert_eq!(word, "@src/ma");
        e.replace_before_cursor(start, "@src/main.rs ");
        assert_eq!(e.text(), "look at @src/main.rs ");
    }
}
