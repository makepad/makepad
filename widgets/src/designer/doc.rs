//! One file under design: its base text, the working text, the hunks made
//! since the base, and undo. The document is also where an edit meets the
//! gate: before it is previewed, the text is checked the way hot reload
//! checks it, so an edit the runtime would refuse is refused here with the
//! parser's message and never queued.
//!
//! The document never writes the file. A preview queues the working text as
//! a hot-reload change for the running app; a commit is the caller's, from
//! [`DesignDoc::text`] and [`DesignDoc::base`].

use crate::makepad_draw::*;

/// One applied edit: `range` in the text BEFORE the edit was replaced by
/// `replacement`.
#[derive(Clone, Debug)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    pub removed: String,
    pub label: String,
}

/// What a preview did.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewOutcome {
    /// The text passed the gate and is queued; the next live edit shows it.
    Queued,
    /// The text equals the base: overrides for the file were dropped instead.
    Reverted,
    /// The text equals the base and the file had no overrides: nothing was
    /// queued and no live edit will follow.
    Unchanged,
    /// The gate refused the text; nothing was queued.
    Refused(String),
}

pub struct DesignDoc {
    /// The resolved path of the file.
    pub file: String,
    base: String,
    text: String,
    hunks: Vec<Hunk>,
    undo: Vec<(String, Vec<Hunk>)>,
    redo: Vec<(String, Vec<Hunk>)>,
}

impl DesignDoc {
    /// Open the file as it is on disk.
    pub fn open(file: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {}", file, e))?;
        Ok(Self::from_text(file, text))
    }

    /// A document over text already in hand.
    pub fn from_text(file: &str, text: String) -> Self {
        Self {
            file: file.to_string(),
            base: text.clone(),
            text,
            hunks: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The text as it was when the document was opened.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The working text, with every edit applied.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The edits since the base, oldest first.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    pub fn is_dirty(&self) -> bool {
        self.text != self.base
    }

    /// A stable fingerprint of the base, for a commit to check the disk
    /// still holds what the document started from.
    pub fn base_hash(&self) -> u64 {
        fnv1a(&self.base)
    }

    /// Whether the file on disk still equals the base.
    pub fn base_matches_disk(&self) -> bool {
        std::fs::read_to_string(&self.file)
            .map(|disk| disk == self.base)
            .unwrap_or(false)
    }

    /// Replace `start..end` of the working text. Returns the hunk.
    pub fn edit(&mut self, start: usize, end: usize, replacement: &str, label: &str) -> Result<Hunk, String> {
        if start > end || end > self.text.len() {
            return Err(format!("edit {}..{} lies outside the text ({} bytes)", start, end, self.text.len()));
        }
        if !self.text.is_char_boundary(start) || !self.text.is_char_boundary(end) {
            return Err(format!("edit {}..{} splits a character", start, end));
        }
        // Hot reload binds `#(...)` placeholders by their order in the block,
        // so an edit that moves, removes or adds one would rebind values
        // silently. Such an edit is cold: it needs a Rust rebuild, and is
        // refused here rather than previewed wrong.
        if self.text[start..end].contains("#(") || replacement.contains("#(") {
            return Err("the edit touches a #() placeholder; that needs a rebuild, not a preview".to_string());
        }
        self.undo.push((self.text.clone(), self.hunks.clone()));
        self.redo.clear();
        let hunk = Hunk {
            start,
            end,
            replacement: replacement.to_string(),
            removed: self.text[start..end].to_string(),
            label: label.to_string(),
        };
        self.text.replace_range(start..end, replacement);
        self.hunks.push(hunk.clone());
        Ok(hunk)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back one edit. Returns whether there was one.
    pub fn undo(&mut self) -> bool {
        let Some((text, hunks)) = self.undo.pop() else {
            return false;
        };
        self.redo.push((std::mem::replace(&mut self.text, text), std::mem::replace(&mut self.hunks, hunks)));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((text, hunks)) = self.redo.pop() else {
            return false;
        };
        self.undo.push((std::mem::replace(&mut self.text, text), std::mem::replace(&mut self.hunks, hunks)));
        true
    }

    /// Forget every edit and go back to the base.
    pub fn reset(&mut self) {
        if self.text != self.base {
            self.undo.push((self.text.clone(), self.hunks.clone()));
            self.redo.clear();
            self.text = self.base.clone();
            self.hunks.clear();
        }
    }

    /// Run the gate over the working text: the same checks hot reload makes.
    pub fn check(&self, cx: &mut Cx) -> Result<(), String> {
        cx.validate_live_edit_text(&self.file, &self.text)
    }

    /// Show the working text in the running app. The text goes through the
    /// gate first; when it equals the base, the file's overrides are dropped
    /// instead, so the compiled-in code runs again.
    pub fn preview(&self, cx: &mut Cx) -> PreviewOutcome {
        if self.text == self.base {
            return if cx.revert_live_edit_file(&self.file) > 0 {
                PreviewOutcome::Reverted
            } else {
                PreviewOutcome::Unchanged
            };
        }
        if let Err(err) = self.check(cx) {
            return PreviewOutcome::Refused(err);
        }
        cx.script_data
            .live_reload
            .queue_file_change(self.file.clone(), self.text.clone());
        PreviewOutcome::Queued
    }

    /// A unified diff of the base against the working text, for a commit.
    pub fn unified_diff(&self) -> String {
        unified_diff(&self.file, &self.base, &self.text)
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// A plain unified diff (one hunk per run of changed lines, three lines of
/// context), built on a longest-common-subsequence over lines.
pub fn unified_diff(file: &str, old: &str, new: &str) -> String {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    // LCS table, bounded: a design file is thousands of lines at most.
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Op {
        Keep,
        Del,
        Add,
    }
    let mut ops: Vec<(Op, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((Op::Keep, i, j));
            i += 1;
            j += 1;
        } else if j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            ops.push((Op::Add, i, j));
            j += 1;
        } else {
            ops.push((Op::Del, i, j));
            i += 1;
        }
    }
    let mut out = format!("--- a/{file}\n+++ b/{file}\n");
    let context = 3usize;
    let mut k = 0;
    while k < ops.len() {
        if ops[k].0 == Op::Keep {
            k += 1;
            continue;
        }
        // A hunk: from `context` lines before this change to `context`
        // lines after the last change within 2*context of the one before.
        let start = k.saturating_sub(context);
        let mut last_change = k;
        let mut scan = k;
        while scan < ops.len() {
            if ops[scan].0 != Op::Keep {
                last_change = scan;
            } else if scan - last_change > 2 * context {
                break;
            }
            scan += 1;
        }
        let end = (last_change + context + 1).min(ops.len());
        let (a_start, b_start) = (ops[start].1, ops[start].2);
        let a_count = ops[start..end].iter().filter(|o| o.0 != Op::Add).count();
        let b_count = ops[start..end].iter().filter(|o| o.0 != Op::Del).count();
        // An empty side names the line before it, as diff does.
        let a_line = if a_count == 0 { a_start } else { a_start + 1 };
        let b_line = if b_count == 0 { b_start } else { b_start + 1 };
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", a_line, a_count, b_line, b_count));
        for &(op, ai, bi) in &ops[start..end] {
            let (mark, line) = match op {
                Op::Keep => (' ', a[ai]),
                Op::Del => ('-', a[ai]),
                Op::Add => ('+', b[bi]),
            };
            out.push(mark);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
        k = end;
    }
    out
}
