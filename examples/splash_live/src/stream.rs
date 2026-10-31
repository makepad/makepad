//! A scripted code stream: a list of versions of one Splash app (each a real
//! `.splash` file that evaluates on its own), played as the edits that turn
//! each version into the next, at a fixed rate, so the audience watches the
//! code being written and the app following it.
//!
//! The edits come from a line diff of consecutive versions; a changed line
//! that keeps its start and end (a property added inside it, a handler added
//! before its closing brace) becomes one insertion in the middle of the line,
//! so the text grows where a writer would add it. Everything is a pure
//! function of the files and the clock: a replay is identical.

/// One edit of the text, at a byte offset of the text as it is when the edit runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Type `text` at `at`.
    Insert { at: usize, text: String },
    /// Remove `len` bytes at `at` (shown as a quick backspace).
    Delete { at: usize, len: usize },
    /// Rest, in seconds (between hunks and between versions).
    Pause(f64),
    /// Version `n` (0-based) is written whole.
    Reached(usize),
}

/// The rest after a block of new code, and after a whole version.
pub const HUNK_PAUSE: f64 = 0.22;
pub const VERSION_PAUSE: f64 = 0.7;

/// The steps that turn `from` into `to`.
pub fn diff_steps(from: &str, to: &str) -> Vec<Step> {
    let a: Vec<&str> = from.split_inclusive('\n').collect();
    let b: Vec<&str> = to.split_inclusive('\n').collect();
    // lcs[i][j]: the longest common subsequence of a[i..] and b[j..].
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let mut steps = Vec::new();
    let (mut i, mut j, mut pos) = (0, 0, 0usize);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            pos += a[i].len();
            i += 1;
            j += 1;
            continue;
        }
        // A hunk: the run of removed and added lines up to the next common line.
        let (mut gone, mut new) = (Vec::new(), Vec::new());
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] { break; }
            if j >= b.len() || (i < a.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
                gone.push(a[i]);
                i += 1;
            } else {
                new.push(b[j]);
                j += 1;
            }
        }
        let paired = gone.len().min(new.len());
        for k in 0..paired {
            pos = line_edit(&mut steps, pos, gone[k], new[k]);
        }
        for line in &gone[paired..] {
            steps.push(Step::Delete { at: pos, len: line.len() });
        }
        let rest: String = new[paired..].concat();
        if !rest.is_empty() {
            steps.push(Step::Insert { at: pos, text: rest.clone() });
            pos += rest.len();
        }
        steps.push(Step::Pause(HUNK_PAUSE));
    }
    steps
}

/// One old line becoming one new line at `pos`: the common start and end
/// stay, the middle is replaced. Returns the offset after the new line.
fn line_edit(steps: &mut Vec<Step>, pos: usize, old: &str, new: &str) -> usize {
    let prefix = common_prefix(old, new);
    let suffix = common_suffix(&old[prefix..], &new[prefix..]);
    let gone = old.len() - prefix - suffix;
    if gone > 0 { steps.push(Step::Delete { at: pos + prefix, len: gone }); }
    let typed = &new[prefix..new.len() - suffix];
    if !typed.is_empty() { steps.push(Step::Insert { at: pos + prefix, text: typed.to_string() }); }
    pos + new.len()
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices().zip(b.chars()).take_while(|((_, x), y)| x == y).map(|((i, x), _)| i + x.len_utf8()).last().unwrap_or(0)
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars().rev().zip(b.chars().rev()).take_while(|(x, y)| x == y).map(|(x, _)| x.len_utf8()).sum()
}

/// The whole play: from empty text through every version.
pub fn plan(versions: &[String]) -> Vec<Step> {
    plan_from("", versions, 0)
}

/// The play from `text` on: it writes version `from` from `text`, then every
/// version after it from the one before.
pub fn plan_from(text: &str, versions: &[String], from: usize) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut prev = text.to_string();
    for (index, version) in versions.iter().enumerate().skip(from) {
        steps.extend(diff_steps(&prev, version));
        steps.push(Step::Reached(index));
        steps.push(Step::Pause(VERSION_PAUSE));
        prev = version.clone();
    }
    steps
}

/// A place in the text as (line, byte in the line): how an editor document
/// addresses it.
pub type LinePos = (usize, usize);

/// One edit the player made, in the text as it was just before: the range
/// `start..end` is replaced by `text` (an insertion has `start == end`, a
/// deletion an empty `text`). A host applies it to its editor document as is.
#[derive(Clone, Debug, PartialEq)]
pub struct EditOp {
    pub start: LinePos,
    pub end: LinePos,
    pub text: String,
}

/// The (line, byte) of byte `offset` in `text`.
pub fn line_pos(text: &str, offset: usize) -> LinePos {
    let before = &text[..offset];
    match before.rfind('\n') {
        Some(newline) => (before.bytes().filter(|&b| b == b'\n').count(), offset - newline - 1),
        None => (0, offset),
    }
}

/// The player: advances through the steps on the clock.
pub struct Player {
    steps: Vec<Step>,
    /// Characters per second while typing; deleting runs three times as fast.
    cps: f64,
    step: usize,
    /// Bytes of the current step done.
    done: usize,
    /// Seconds of the current pause done.
    rested: f64,
    /// Character budget carried between frames.
    budget: f64,
    pub text: String,
    /// Where the last edit happened (for the caret and the scroll).
    pub caret: usize,
    /// Whether the text stands at a boundary of the writing: a step done, or
    /// a line of an insertion finished. Mid-word text is never evaluated.
    pub at_boundary: bool,
    /// The edits of the last `advance`, in order (at most one per frame).
    pub edits: Vec<EditOp>,
    versions: Vec<String>,
    /// How many versions are written whole.
    reached: usize,
}

impl Player {
    pub fn new(versions: &[String], cps: f64) -> Self {
        Player { steps: plan(versions), cps, step: 0, done: 0, rested: 0.0, budget: 0.0, text: String::new(), caret: 0, at_boundary: false, edits: Vec::new(), versions: versions.to_vec(), reached: 0 }
    }

    /// Go on from `text` (the presenter edited what was written): the
    /// version being written is written from it, then the rest. Deterministic
    /// from the text it is given.
    pub fn resume_from(&mut self, text: &str) {
        if text == self.text { return; }
        self.steps = plan_from(text, &self.versions, self.reached);
        self.step = 0;
        self.done = 0;
        self.rested = 0.0;
        self.budget = 0.0;
        self.text = text.to_string();
        self.caret = self.caret.min(text.len());
        while !self.text.is_char_boundary(self.caret) { self.caret -= 1; }
    }

    /// Write everything that is left at once: the last version, as one edit
    /// of the whole text.
    pub fn finish(&mut self) {
        let Some(last) = self.versions.last().cloned() else { return };
        self.edits = vec![EditOp { start: (0, 0), end: line_pos(&self.text, self.text.len()), text: last.clone() }];
        self.text = last;
        self.caret = self.text.len();
        self.step = self.steps.len();
        self.reached = self.versions.len();
        self.at_boundary = true;
    }

    pub fn finished(&self) -> bool { self.step >= self.steps.len() }

    /// Advance by `dt` seconds. Returns whether the text changed.
    pub fn advance(&mut self, dt: f64) -> bool {
        let mut changed = false;
        self.at_boundary = false;
        self.edits.clear();
        self.budget += dt * self.cps;
        let mut time = dt;
        while self.step < self.steps.len() {
            match &self.steps[self.step] {
                Step::Reached(index) => {
                    self.reached = index + 1;
                    self.next();
                }
                Step::Pause(secs) => {
                    let need = secs - self.rested;
                    if time < need { self.rested += time; self.budget = 0.0; return changed; }
                    time -= need;
                    self.budget = time * self.cps;
                    self.next();
                }
                Step::Insert { at, text } => {
                    let rest = &text[self.done..];
                    let mut take = 0;
                    // A frame types up to the end of a line at most, so every
                    // finished line is a frame the preview can follow.
                    for ch in rest.chars() {
                        if self.budget < 1.0 { break; }
                        self.budget -= 1.0;
                        take += ch.len_utf8();
                        if ch == '\n' { break; }
                    }
                    if take == 0 { return changed; }
                    let at = at + self.done;
                    let pos = line_pos(&self.text, at);
                    self.edits.push(EditOp { start: pos, end: pos, text: rest[..take].to_string() });
                    self.text.insert_str(at, &rest[..take]);
                    self.done += take;
                    self.caret = at + take;
                    self.at_boundary = rest[..take].ends_with('\n') || self.done == text.len();
                    changed = true;
                    if self.done == text.len() { self.next(); }
                    return changed;
                }
                Step::Delete { at, len } => {
                    let left = len - self.done;
                    let can = ((self.budget * 3.0) as usize).min(left);
                    if can == 0 { return changed; }
                    self.budget -= can as f64 / 3.0;
                    // Backspace from the end of the range.
                    let end = at + left;
                    let mut start = end - can;
                    while !self.text.is_char_boundary(start) { start -= 1; }
                    let removed = end - start;
                    self.edits.push(EditOp { start: line_pos(&self.text, start), end: line_pos(&self.text, end), text: String::new() });
                    self.text.replace_range(start..end, "");
                    self.done += removed;
                    self.caret = start;
                    self.at_boundary = self.done >= *len;
                    changed = true;
                    if self.done >= *len { self.next(); }
                    return changed;
                }
            }
        }
        changed
    }

    fn next(&mut self) {
        self.step += 1;
        self.done = 0;
        self.rested = 0.0;
    }

    /// Seconds the whole play takes.
    pub fn duration(&self) -> f64 {
        self.steps.iter().map(|step| match step {
            Step::Pause(secs) => *secs,
            Step::Reached(_) => 0.0,
            Step::Insert { text, .. } => text.chars().count() as f64 / self.cps,
            Step::Delete { len, .. } => *len as f64 / (self.cps * 3.0),
        }).sum()
    }
}

/// Whether `text` is worth evaluating: every bracket closed and no string
/// left open (comments skipped). A half-typed line is not evaluated; the
/// last picture stays instead.
pub fn balanced(text: &str) -> bool {
    let mut depth: Vec<char> = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() { if c == '\n' { break; } }
            }
            '"' => {
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '\\' { chars.next(); continue; }
                    if c == '"' { closed = true; break; }
                }
                if !closed { return false; }
            }
            '{' | '[' | '(' => depth.push(ch),
            '}' => if depth.pop() != Some('{') { return false; },
            ']' => if depth.pop() != Some('[') { return false; },
            ')' => if depth.pop() != Some('(') { return false; },
            _ => {}
        }
    }
    depth.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(versions: &[&str]) -> Vec<String> {
        let versions: Vec<String> = versions.iter().map(|v| v.to_string()).collect();
        let mut player = Player::new(&versions, 100.0);
        let mut seen = Vec::new();
        while !player.finished() {
            player.advance(1.0 / 60.0);
            seen.push(player.text.clone());
        }
        seen
    }

    #[test]
    fn a_play_ends_on_the_last_version_and_passes_through_each() {
        let v = ["View{\n}\n", "View{\n    Label{text: \"a\"}\n}\n", "View{\n    Label{text: \"a\" color: #fff}\n}\n"];
        let seen = play(&v);
        assert_eq!(seen.last().unwrap(), v[2]);
        assert!(seen.iter().any(|t| t == v[0]));
        assert!(seen.iter().any(|t| t == v[1]));
    }

    #[test]
    fn a_property_added_inside_a_line_is_typed_in_place() {
        let steps = diff_steps("Key{text: \"7\"}\n", "Key{text: \"7\" on_click: || go()}\n");
        assert_eq!(steps[0], Step::Insert { at: 13, text: " on_click: || go()".into() });
    }

    #[test]
    fn removed_lines_are_deleted_and_the_text_still_lands() {
        let v = ["a\nb\nc\n", "a\nc\nd\n"];
        assert_eq!(play(&v).last().unwrap(), v[1]);
    }

    #[test]
    fn a_play_is_deterministic() {
        let v = ["x{\n}\n", "x{\n  y{}\n  z{}\n}\n"];
        assert_eq!(play(&v), play(&v));
    }

    #[test]
    fn a_boundary_is_a_finished_line_or_step() {
        let versions = vec!["a{\n}\n".to_string(), "a{\n  bb{}\n}\n".to_string()];
        let mut player = Player::new(&versions, 1000.0);
        let mut boundaries = Vec::new();
        while !player.finished() {
            if player.advance(1.0 / 600.0) && player.at_boundary { boundaries.push(player.text.clone()); }
        }
        assert!(boundaries.iter().all(|t| balanced(t) || t.ends_with('\n')));
        assert!(boundaries.contains(&versions[1]));
    }

    /// The edits, applied to a line-addressed copy the way an editor
    /// document applies them, keep it equal to the player's text.
    #[test]
    fn edits_replayed_on_lines_match_the_text() {
        let versions: Vec<String> = ["a{\n}\n", "a{\n  b{x: 1}\n  c{}\n}\n", "a{\n  b{x: 2 y: 3}\n}\n"].iter().map(|v| v.to_string()).collect();
        let mut player = Player::new(&versions, 50.0);
        let mut lines: Vec<String> = vec![String::new()];
        while !player.finished() {
            player.advance(1.0 / 60.0);
            for edit in &player.edits {
                let joined = lines.join("\n");
                let offset = |(l, b): LinePos| lines[..l].iter().map(|line| line.len() + 1).sum::<usize>() + b;
                let (start, end) = (offset(edit.start), offset(edit.end));
                let next = format!("{}{}{}", &joined[..start], edit.text, &joined[end..]);
                lines = next.split('\n').map(str::to_string).collect();
            }
            assert_eq!(lines.join("\n"), player.text);
        }
        assert_eq!(player.text, versions[2]);
    }

    #[test]
    fn a_resume_from_an_edited_text_still_ends_on_the_last_version() {
        let versions: Vec<String> = ["a{\n}\n", "a{\n  b{c: #fff}\n}\n", "a{\n  b{c: #fff}\n  d{}\n}\n"].iter().map(|v| v.to_string()).collect();
        let mut player = Player::new(&versions, 100.0);
        for _ in 0..20 { player.advance(1.0 / 60.0); }
        let edited = player.text.replace("a{", "a{ x: 1");
        player.resume_from(&edited);
        while !player.finished() { player.advance(1.0 / 60.0); }
        assert_eq!(player.text, versions[2]);
    }

    #[test]
    fn finish_writes_the_last_version_as_one_edit() {
        let versions: Vec<String> = ["a{\n}\n", "a{\n  b{}\n}\n"].iter().map(|v| v.to_string()).collect();
        let mut player = Player::new(&versions, 100.0);
        for _ in 0..5 { player.advance(1.0 / 60.0); }
        player.finish();
        assert!(player.finished());
        assert_eq!(player.text, versions[1]);
        assert_eq!(player.edits.len(), 1);
    }

    #[test]
    fn balance_needs_closed_brackets_and_strings() {
        assert!(balanced("View{ Label{text: \"}\"} } // {"));
        assert!(!balanced("View{ Label{text: \"a"));
        assert!(!balanced("View{ Label{"));
    }
}
