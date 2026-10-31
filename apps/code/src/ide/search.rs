//! Project-wide text search. Contract: `focus(cx, query)` puts the keyboard
//! in the query field (with the query when given); a match row (click, or
//! Enter from the field) emits `IdeAction::OpenFile` at its line and column.
//!
//! Typing waits 150 ms, then the search runs on worker threads over the
//! project's files (the workspace index when it has them, else a git-ignore
//! aware walk), skipping binary and huge files. A newer query cancels the
//! running one (generation counter); results stream to the UI in batches
//! and stop at `MAX_MATCHES`. Literal queries take a plain substring path
//! (case-sensitive) or an escaped `makepad-regex` (ignoring case); regex
//! queries go to `makepad-regex` directly; whole word is checked against
//! the full line around each match.

use super::{IdeAction, Workspace};
use crate::ui::theme::{palette, vec};
use makepad_regex::{Cursor as ReCursor, Input as ReInput, ParseOptions, Regex};
use makepad_widgets::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.SearchPanelBase = #(SearchPanel::register_widget(vm))

    let SMono = Label{
        width: Fit
        height: Fit
        padding: 0
        text: ""
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    let SField = TextInput{
        width: Fill
        height: 22
        padding: Inset{left: 2 right: 2 top: 3 bottom: 3}
        draw_text +: {
            color: mod.look.text_hi
            color_hover: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            color_down: uniform(mod.look.text_hi)
            color_empty: uniform(mod.look.dim)
            color_empty_hover: uniform(mod.look.dim)
            color_empty_focus: uniform(mod.look.dim)
            text_style: mod.look.mono{font_size: 9.0}
        }
        draw_cursor +: { color: mod.look.agent }
        draw_selection +: {
            color: mod.look.agent_select
            color_hover: mod.look.agent_select
            color_focus: mod.look.agent_select
        }
        draw_bg +: {
            color: #0000
            color_hover: #0000
            color_focus: #0000
            color_empty: #0000
            color_empty_hover: #0000
            color_empty_focus: #0000
            border_size: 0.0
        }
    }

    /// A hairline frame under a field.
    let SWell = View{
        width: Fill
        height: Fit
        flow: Right
        align: Align{y: 0.5}
        padding: Inset{left: 6 right: 6 top: 1 bottom: 1}
        show_bg: true
        draw_bg +: {
            color: mod.look.well
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let line = 1.0 - step(px, self.rect_size.y - p.y)
                let c = mix(self.color, mod.look.hair_hi, line)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    /// A small toggle key: chamfered hairline frame, mono label.
    let SKey = ButtonFlat{
        width: 26
        height: 20
        padding: 0
        margin: 0
        align: Align{x: 0.5 y: 0.5}
        draw_text +: {
            color: mod.look.meta
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono_bold{font_size: 8.0}
        }
        draw_bg +: {
            ink: uniform(mod.look.dim)
            on: uniform(0.0)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 3.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let wash = 0.04 + 0.16 * self.on + 0.10 * self.hover + 0.2 * self.down
                let a = clamp(fill * wash + edge * (0.5 + 0.5 * max(self.hover, self.on)), 0.0, 1.0)
                return vec4(self.ink.rgb * a, a)
            }
        }
    }

    /// Line text between matches.
    let SPlain = SMono{
        max_lines: 1
        draw_text +: { color: mod.look.meta }
    }

    /// One match on a line, lit with the agent's wash.
    let SLit = View{
        width: Fit
        height: Fit
        padding: Inset{left: 1 right: 1}
        show_bg: true
        draw_bg +: {
            color: mod.look.agent_select
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
    }

    let SRow = View{
        width: Fill
        height: 20
        flow: Right
        spacing: 6
        align: Align{y: 0.5}
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {
            color: #0000
            pixel: fn() {
                let p = self.pos * self.rect_size
                let bar = (1.0 - step(2.0, p.x)) * step(0.001, self.color.a)
                let c = mix(self.color, mod.look.agent, bar)
                return vec4(c.rgb * c.a, c.a)
            }
        }
    }

    mod.widgets.SearchPanel = set_type_default() do mod.widgets.SearchPanelBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 6
        padding: Inset{left: 8 right: 8 top: 8 bottom: 4}

        SWell{
            query := SField{ empty_text: "search the project  ⏎" }
        }
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 4
            align: Align{y: 0.5}
            case_key := SKey{ text: "Aa" }
            regex_key := SKey{ text: ".*" }
            word_key := SKey{ text: "W" }
            View{ width: 4 height: 1 }
            SWell{
                include := SField{ height: 20 empty_text: "files to include  *.rs, apps/code, !tests" draw_text +: { text_style: mod.look.mono{font_size: 8.0} } }
            }
        }
        status := SMono{
            width: Fill
            max_lines: 1
            text_overflow: Ellipsis
            draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} }
        }
        results := PortalList{
            width: Fill
            height: Fill
            FileRow := SRow{
                padding: Inset{left: 4 right: 4}
                file_mark := SMono{
                    width: 10
                    draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 7.5} }
                }
                file_path := SMono{
                    width: Fill
                    max_lines: 1
                    text_overflow: Ellipsis
                    draw_text +: { color: mod.look.text_hi text_style: mod.look.mono_bold{font_size: 8.5} }
                }
                file_count := SMono{
                    draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} }
                }
            }
            LineRow := SRow{
                spacing: 0
                padding: Inset{left: 4 right: 4}
                match_line := SMono{
                    width: 30
                    align: Align{x: 1.0}
                    margin: Inset{right: 8}
                    draw_text +: { color: mod.look.dim text_style: mod.look.mono{font_size: 8.0} }
                }
                p0 := SPlain{}
                b0 := SLit{ h0 := SMono{ max_lines: 1 draw_text +: { color: mod.look.text_hi } } }
                p1 := SPlain{}
                b1 := SLit{ h1 := SMono{ max_lines: 1 draw_text +: { color: mod.look.text_hi } } }
                p2 := SPlain{}
                b2 := SLit{ h2 := SMono{ max_lines: 1 draw_text +: { color: mod.look.text_hi } } }
                p3 := SPlain{}
                b3 := SLit{ h3 := SMono{ max_lines: 1 draw_text +: { color: mod.look.text_hi } } }
                p4 := SPlain{}
                b4 := SLit{ h4 := SMono{ max_lines: 1 draw_text +: { color: mod.look.text_hi } } }
                p5 := SPlain{}
                b5 := SLit{ h5 := SMono{ max_lines: 1 draw_text +: { color: mod.look.text_hi } } }
                tail := SMono{
                    width: Fill
                    max_lines: 1
                    text_overflow: Ellipsis
                    draw_text +: { color: mod.look.meta }
                }
            }
        }
    }
}

/// Matches past this stop the search ("first N").
pub const MAX_MATCHES: usize = 5000;
/// At most this many matches are kept from one line.
const MAX_PER_LINE: usize = 64;
/// Files larger than this are skipped.
const MAX_FILE_BYTES: u64 = 4 << 20;
const DEBOUNCE: f64 = 0.15;
const SKIP_DIRS: [&str; 4] = [".git", "target", "node_modules", "wt.noindex"];

// ---- the matcher -----------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
}

enum Kind {
    /// Case-sensitive literal: a plain substring scan.
    Literal(String),
    Regex(Regex),
}

/// One query, compiled. Not `Send` (the regex keeps its scratch space in
/// `Rc`s): each worker compiles its own.
pub struct Matcher {
    kind: Kind,
    whole_word: bool,
}

/// A line read by the regex from a byte offset on: the cursor starts at the
/// offset but sees the whole line, so `^`, `\b` and look-behind context
/// are the line's, not a slice's. Indices are the line's.
#[derive(Clone, Copy)]
struct LineFrom<'a> {
    line: &'a str,
    start: usize,
    end: usize,
}

#[derive(Clone)]
struct LineCursor<'a> {
    line: &'a str,
    index: usize,
}

impl<'a> ReInput<'a> for LineFrom<'a> {
    type Cursor = LineCursor<'a>;

    fn slice(&self, range: std::ops::Range<usize>) -> Self {
        LineFrom { line: self.line, start: range.start, end: range.end }
    }

    fn cursor_start(&self) -> LineCursor<'a> {
        LineCursor { line: self.line, index: self.start }
    }

    fn cursor_end(&self) -> LineCursor<'a> {
        LineCursor { line: self.line, index: self.end }
    }
}

impl ReCursor for LineCursor<'_> {
    fn is_start(&self) -> bool {
        self.index == 0
    }

    fn is_end(&self) -> bool {
        self.index == self.line.len()
    }

    fn index(&self) -> usize {
        self.index
    }

    fn current_byte(&mut self) -> Option<u8> {
        self.line.as_bytes().get(self.index).copied()
    }

    fn current_char(&mut self) -> Option<char> {
        self.line[self.index..].chars().next()
    }

    fn prev_byte(&mut self) -> Option<u8> {
        self.line.as_bytes()[..self.index].last().copied()
    }

    fn prev_char(&mut self) -> Option<char> {
        self.line[..self.index].chars().next_back()
    }

    fn move_next_byte(&mut self) -> bool {
        if self.is_end() {
            return false;
        }
        self.index += 1;
        true
    }

    fn move_next_char(&mut self) -> bool {
        if !self.move_next_byte() {
            return false;
        }
        while !self.line.is_char_boundary(self.index) {
            self.index += 1;
        }
        true
    }

    fn move_prev_byte(&mut self) -> bool {
        if self.is_start() {
            return false;
        }
        self.index -= 1;
        true
    }

    fn move_prev_char(&mut self) -> bool {
        if !self.move_prev_byte() {
            return false;
        }
        while !self.line.is_char_boundary(self.index) {
            self.index -= 1;
        }
        true
    }
}

/// `text` with every ASCII punctuation character escaped, so the regex
/// matches it literally.
pub fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        // Only the metacharacters: the parser refuses escapes like `\_`.
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Matcher {
    pub fn new(query: &str, opts: SearchOptions) -> Result<Matcher, String> {
        if query.is_empty() {
            return Err("empty query".into());
        }
        let kind = if !opts.regex && opts.case_sensitive {
            Kind::Literal(query.to_string())
        } else {
            let pattern = if opts.regex { query.to_string() } else { escape_regex(query) };
            let options = ParseOptions { ignore_case: !opts.case_sensitive, multiline: true, ..ParseOptions::default() };
            Kind::Regex(Regex::new_with_options(&pattern, options).map_err(|e| format!("{e:?}"))?)
        };
        Ok(Matcher { kind, whole_word: opts.whole_word })
    }

    /// Whether the text can hold a match at all (a whole-file prefilter: it
    /// may say yes for a match that spans lines, never no for a line match).
    pub fn could_match(&self, text: &str) -> bool {
        match &self.kind {
            Kind::Literal(s) => text.contains(s.as_str()),
            Kind::Regex(r) => r.earliest_match(text).is_some(),
        }
    }

    /// The first match at or after byte `from`, matched against the whole
    /// line (so anchors and word boundaries see what comes before `from`).
    fn find_from(&self, line: &str, from: usize) -> Option<(usize, usize)> {
        match &self.kind {
            Kind::Literal(n) => line[from..].find(n.as_str()).map(|s| (from + s, from + s + n.len())),
            Kind::Regex(r) => {
                let (s, e) = r.earliest_match(LineFrom { line, start: from, end: line.len() })?;
                // The reverse pass may find a start before `from` (a longer
                // match overlapping the previous one): the scan resumes here.
                Some((s.max(from), e))
            }
        }
    }

    fn word_bounded(line: &str, start: usize, end: usize) -> bool {
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        let first = line[start..end].chars().next();
        let last = line[start..end].chars().next_back();
        let ok_before = !(before.is_some_and(is_word_char) && first.is_some_and(is_word_char));
        let ok_after = !(after.is_some_and(is_word_char) && last.is_some_and(is_word_char));
        ok_before && ok_after
    }

    /// The byte ranges of the matches in one line (no line break in it),
    /// left to right, never empty, never overlapping.
    pub fn find_in_line(&self, line: &str, out: &mut Vec<(usize, usize)>) {
        out.clear();
        let mut from = 0;
        while from <= line.len() && out.len() < MAX_PER_LINE {
            let Some((s, e)) = self.find_from(line, from) else { break };
            if e == s {
                // An empty match says nothing: step a character on.
                from = next_char(line, e);
                if from > line.len() {
                    break;
                }
                continue;
            }
            if self.whole_word && !Self::word_bounded(line, s, e) {
                from = next_char(line, s);
                continue;
            }
            out.push((s, e));
            from = e;
        }
    }
}

fn next_char(line: &str, at: usize) -> usize {
    line[at..].chars().next().map_or(line.len() + 1, |c| at + c.len_utf8())
}

/// The 1-based column (in characters) of a byte offset in a line.
pub fn column_of(line: &str, byte: usize) -> u32 {
    line[..byte].chars().count() as u32 + 1
}

/// Matches lit on one row; later ones on the line are counted, not lit.
pub const MAX_LIT: usize = 6;

/// What a line row shows, as segments alternating plain and lit, starting
/// and ending plain: the text before the first match (leading whitespace
/// gone, cut to its last `PRE_CHARS` characters), each match and the text
/// between them, then the rest of the line. Spans are byte ranges, left to
/// right; past `MAX_LIT` they stay in the tail.
pub fn segments(line: &str, spans: &[(usize, usize)]) -> Vec<String> {
    const PRE_CHARS: usize = 12;
    const MAX_CHARS: usize = 160;
    let clean = |t: &str| -> String { t.chars().take(MAX_CHARS).collect::<String>().replace('\t', "  ") };
    let Some(&(first, _)) = spans.first() else { return vec![clean(line.trim())] };
    let pre = line[..first].trim_start();
    let n = pre.chars().count();
    let pre = if n > PRE_CHARS { format!("…{}", pre.chars().skip(n - PRE_CHARS).collect::<String>()) } else { pre.to_string() };
    let mut out = vec![pre.replace('\t', "  ")];
    let shown = &spans[..spans.len().min(MAX_LIT)];
    for (i, &(s, e)) in shown.iter().enumerate() {
        out.push(clean(&line[s..e]));
        let next = shown.get(i + 1).map_or(line.len(), |&(s, _)| s);
        let gap = &line[e..next];
        out.push(clean(if i + 1 == shown.len() { gap.trim_end() } else { gap }));
    }
    out
}

/// The "files to include" filter: comma or space separated terms; `*.rs`
/// matches the end of the path, `!term` excludes, anything else is a
/// substring of the path. With no including term every file is in.
pub fn include_filter(spec: &str) -> impl Fn(&str) -> bool {
    let mut inc = Vec::new();
    let mut exc = Vec::new();
    for term in spec.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        let (list, term) = match term.strip_prefix('!') {
            Some(t) => (&mut exc, t),
            None => (&mut inc, term),
        };
        if !term.is_empty() {
            list.push(term.to_string());
        }
    }
    fn hit(term: &str, path: &str) -> bool {
        match term.strip_prefix('*') {
            Some(suffix) if !suffix.contains('*') => path.ends_with(suffix),
            _ => {
                let t = term.trim_matches('*');
                path.contains(t)
            }
        }
    }
    move |path: &str| (inc.is_empty() || inc.iter().any(|t| hit(t, path))) && !exc.iter().any(|t| hit(t, path))
}

// ---- the worker ------------------------------------------------------------

/// A line with matches: one result row.
#[derive(Clone, Debug)]
pub struct LineHit {
    /// 1-based.
    pub line: u32,
    /// Each match's 1-based column (in characters), left to right.
    pub cols: Vec<u32>,
    /// What the row shows (see `segments`).
    pub segs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct FileHits {
    pub path: String,
    pub lines: Vec<LineHit>,
    /// Matches in the file (a line can hold several).
    pub matches: usize,
}

/// Searches one file's text; `None` when it has no match.
pub fn search_text(m: &Matcher, path: &str, text: &str, budget: &AtomicUsize) -> Option<FileHits> {
    if !m.could_match(text) {
        return None;
    }
    let mut lines = Vec::new();
    let mut matches = 0;
    let mut spans = Vec::new();
    for (i, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        m.find_in_line(line, &mut spans);
        if spans.is_empty() {
            continue;
        }
        let before = budget.fetch_add(spans.len(), Ordering::Relaxed);
        if before >= MAX_MATCHES {
            break;
        }
        spans.truncate(MAX_MATCHES - before);
        matches += spans.len();
        let cols = spans.iter().map(|&(s, _)| column_of(line, s)).collect();
        lines.push(LineHit { line: i as u32 + 1, cols, segs: segments(line, &spans) });
    }
    (!lines.is_empty()).then(|| FileHits { path: path.to_string(), lines, matches })
}

/// The project's files (relative, `/`-separated, sorted), git-ignore aware.
fn walk_files(root: &Path, cancel: &dyn Fn() -> bool) -> Vec<String> {
    let mut ignore = makepad_git::ignore::GitIgnore::new(root).ok();
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(dir) = stack.pop() {
        if cancel() {
            return Vec::new();
        }
        let Ok(rd) = std::fs::read_dir(root.join(&dir)) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let rel = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
            if ft.is_dir() && SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if let Some(ig) = ignore.as_mut() {
                if ig.ignored(&rel, ft.is_dir()).unwrap_or(false) {
                    continue;
                }
            }
            if ft.is_dir() {
                stack.push(rel);
            } else {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// What the workers send the UI.
#[derive(Default)]
struct Inbox {
    generation: u64,
    files: Vec<FileHits>,
    searched: usize,
    done: bool,
}

struct Job {
    generation: u64,
    root: PathBuf,
    files: Arc<Vec<String>>,
    include: String,
    query: String,
    opts: SearchOptions,
}

fn run_job(job: Job, current: Arc<AtomicU64>, inbox: Arc<Mutex<Inbox>>) {
    let generation = job.generation;
    let stale = || current.load(Ordering::Relaxed) != generation;
    let files = if job.files.is_empty() { Arc::new(walk_files(&job.root, &stale)) } else { job.files.clone() };
    let filter = include_filter(&job.include);
    let files: Arc<Vec<&str>> = Arc::new(files.iter().map(String::as_str).filter(|p| filter(p)).collect());
    let next = AtomicUsize::new(0);
    let budget = AtomicUsize::new(0);
    let searched = AtomicUsize::new(0);
    let full = AtomicBool::new(false);
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (files, next, budget, searched, full, inbox, root) = (&files, &next, &budget, &searched, &full, &inbox, &job.root);
            let (stale, query, opts) = (&stale, &job.query, job.opts);
            scope.spawn(move || {
                let Ok(m) = Matcher::new(query, opts) else { return };
                let mut batch = Vec::new();
                let mut last_send = std::time::Instant::now();
                loop {
                    if stale() || full.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&rel) = files.get(i) else { break };
                    searched.fetch_add(1, Ordering::Relaxed);
                    if let Some(text) = read_text(&root.join(rel)) {
                        if let Some(hits) = search_text(&m, rel, &text, budget) {
                            batch.push(hits);
                        }
                        if budget.load(Ordering::Relaxed) >= MAX_MATCHES {
                            full.store(true, Ordering::Relaxed);
                        }
                    }
                    if !batch.is_empty() && last_send.elapsed().as_millis() > 40 {
                        send(inbox, generation, &mut batch, searched.load(Ordering::Relaxed), false);
                        last_send = std::time::Instant::now();
                    }
                }
                send(inbox, generation, &mut batch, searched.load(Ordering::Relaxed), false);
            });
        }
    });
    if !stale() {
        send(&inbox, generation, &mut Vec::new(), searched.load(Ordering::Relaxed), true);
    }
}

fn send(inbox: &Mutex<Inbox>, generation: u64, batch: &mut Vec<FileHits>, searched: usize, done: bool) {
    let Ok(mut ib) = inbox.lock() else { return };
    if ib.generation != generation {
        return;
    }
    ib.files.append(batch);
    ib.searched = ib.searched.max(searched);
    ib.done |= done;
    drop(ib);
    SignalToUI::set_ui_signal();
}

/// The file as text, unless it is too big or binary (a NUL in its head).
fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    match String::from_utf8(bytes) {
        Ok(s) => Some(s),
        Err(e) => Some(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

// ---- the panel -------------------------------------------------------------

const PLAIN: [&[LiveId]; MAX_LIT] = [ids!(p0), ids!(p1), ids!(p2), ids!(p3), ids!(p4), ids!(p5)];
const LIT: [&[LiveId]; MAX_LIT] = [ids!(h0), ids!(h1), ids!(h2), ids!(h3), ids!(h4), ids!(h5)];
const LIT_BG: [&[LiveId]; MAX_LIT] = [ids!(b0), ids!(b1), ids!(b2), ids!(b3), ids!(b4), ids!(b5)];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    File(usize),
    /// A file's line with matches.
    Line(usize, usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct SearchPanel {
    #[deref]
    view: View,
    #[rust]
    opts: SearchOptions,
    #[rust]
    debounce: Timer,
    /// The query and include filter of the search shown.
    #[rust]
    shown: Option<(String, String, SearchOptions)>,
    #[rust]
    generation: Arc<AtomicU64>,
    #[rust]
    inbox: Arc<Mutex<Inbox>>,
    /// Results, files sorted by path.
    #[rust]
    files: Vec<FileHits>,
    #[rust]
    folded: HashSet<String>,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    selected: Option<usize>,
    #[rust]
    matches: usize,
    #[rust]
    searching: bool,
    #[rust]
    error: Option<String>,
    #[rust]
    want_focus: bool,
}

impl SearchPanel {
    pub fn focus(&mut self, cx: &mut Cx, query: Option<&str>) {
        let input = self.view.text_input(cx, ids!(query));
        if let Some(q) = query {
            input.set_text(cx, q);
            self.run(cx);
        }
        input.take_key_focus(cx);
        self.want_focus = true;
        self.view.redraw(cx);
    }

    fn query(&self, cx: &Cx) -> (String, String) {
        (self.view.text_input(cx, ids!(query)).text(), self.view.text_input(cx, ids!(include)).text())
    }

    /// Starts the search for what the fields say (unless it is the one shown).
    fn run(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.debounce);
        self.debounce = Timer::empty();
        let (query, include) = self.query(cx);
        let key = (query.clone(), include.clone(), self.opts);
        if self.shown.as_ref() == Some(&key) {
            return;
        }
        self.shown = Some(key);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut ib) = self.inbox.lock() {
            *ib = Inbox { generation, ..Inbox::default() };
        }
        self.files.clear();
        self.folded.clear();
        self.selected = None;
        self.matches = 0;
        self.error = None;
        self.searching = false;
        self.rebuild_rows();
        self.view.portal_list(cx, ids!(results)).set_first_id_and_scroll(0, 0.0);
        if query.is_empty() {
            self.view.redraw(cx);
            return;
        }
        if let Err(e) = Matcher::new(&query, self.opts) {
            self.error = Some(e);
            self.view.redraw(cx);
            return;
        }
        let Some(ws) = Workspace::of(cx) else { return };
        let job = Job { generation, root: ws.root.clone(), files: ws.index.files(), include, query, opts: self.opts };
        let (current, inbox) = (self.generation.clone(), self.inbox.clone());
        self.searching = true;
        std::thread::spawn(move || run_job(job, current, inbox));
        self.view.redraw(cx);
    }

    fn drain(&mut self, cx: &mut Cx) {
        let (batch, done) = {
            let Ok(mut ib) = self.inbox.lock() else { return };
            if ib.generation != self.generation.load(Ordering::Relaxed) {
                return;
            }
            (std::mem::take(&mut ib.files), std::mem::take(&mut ib.done))
        };
        if batch.is_empty() && !done {
            return;
        }
        for f in batch {
            let at = self.files.partition_point(|x| x.path < f.path);
            self.matches += f.matches;
            self.files.insert(at, f);
        }
        if done {
            self.searching = false;
        }
        self.rebuild_rows();
        self.view.redraw(cx);
    }

    fn rebuild_rows(&mut self) {
        let sel = self.selected.and_then(|i| self.rows.get(i).copied());
        self.rows.clear();
        for (fi, f) in self.files.iter().enumerate() {
            self.rows.push(Row::File(fi));
            if !self.folded.contains(&f.path) {
                self.rows.extend((0..f.lines.len()).map(|li| Row::Line(fi, li)));
            }
        }
        // Rows shift as files stream in: keep the selection on its line.
        self.selected = sel.and_then(|r| self.rows.iter().position(|x| *x == r));
    }

    fn status(&self) -> String {
        if let Some(e) = &self.error {
            return format!("bad pattern: {e}");
        }
        let Some((q, ..)) = &self.shown else { return String::new() };
        if q.is_empty() {
            return String::new();
        }
        let files = self.files.len();
        let plural = |n: usize, w: &str| if n == 1 { format!("{n} {w}") } else { format!("{n} {w}s") };
        let res = if self.matches >= MAX_MATCHES { format!("first {MAX_MATCHES} results (of more) in {}", plural(files, "file")) } else { format!("{} in {}", plural(self.matches, "result"), plural(files, "file")) };
        if self.searching {
            format!("searching…  {res}")
        } else if self.matches == 0 {
            "no results".to_string()
        } else {
            res
        }
    }

    fn move_selection(&mut self, cx: &mut Cx, down: bool) {
        let is_match = |r: &Row| matches!(r, Row::Line(..));
        let next = match self.selected {
            None => {
                if down {
                    self.rows.iter().position(is_match)
                } else {
                    self.rows.iter().rposition(is_match)
                }
            }
            Some(i) if down => self.rows.iter().skip(i + 1).position(is_match).map(|p| p + i + 1),
            Some(i) => self.rows[..i].iter().rposition(is_match),
        };
        if let Some(n) = next {
            self.selected = Some(n);
            self.view.portal_list(cx, ids!(results)).keep_index_visible(cx, n);
            self.view.redraw(cx);
        }
    }

    /// Opens a line row at its `nth` match (folds or unfolds a file row).
    fn open_row(&mut self, cx: &mut Cx, index: usize, nth: usize) {
        match self.rows.get(index).copied() {
            Some(Row::Line(fi, li)) => {
                let f = &self.files[fi];
                let h = &f.lines[li];
                let col = h.cols.get(nth).or(h.cols.first()).copied();
                self.selected = Some(index);
                cx.widget_action(self.widget_uid(), IdeAction::OpenFile { path: f.path.clone(), line: Some(h.line), col });
                self.view.redraw(cx);
            }
            Some(Row::File(fi)) => {
                let path = self.files[fi].path.clone();
                if !self.folded.remove(&path) {
                    self.folded.insert(path);
                }
                self.rebuild_rows();
                self.view.redraw(cx);
            }
            None => {}
        }
    }

    fn toggle(&mut self, cx: &mut Cx, which: usize) {
        match which {
            0 => self.opts.case_sensitive = !self.opts.case_sensitive,
            1 => self.opts.regex = !self.opts.regex,
            _ => self.opts.whole_word = !self.opts.whole_word,
        }
        self.run(cx);
        self.view.text_input(cx, ids!(query)).take_key_focus(cx);
        self.view.redraw(cx);
    }

    fn draw_keys(&mut self, cx: &mut Cx2d) {
        let p = palette();
        for (on, path) in [(self.opts.case_sensitive, ids!(case_key)), (self.opts.regex, ids!(regex_key)), (self.opts.whole_word, ids!(word_key))] {
            let ink = if on { vec(p.primary, 1.0) } else { vec(p.disabled, 1.0) };
            let text = if on { vec(p.text_hi, 1.0) } else { vec(p.meta, 1.0) };
            let on = if on { 1.0 } else { 0.0 };
            let mut b = self.view.button(cx, path);
            script_apply_eval!(cx, b, { draw_bg +: { ink: #(ink) on: #(on) } draw_text +: { color: #(text) } });
        }
    }

    fn draw_results(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let p = palette();
        list.set_item_range(cx, 0, self.rows.len());
        while let Some(index) = list.next_visible_item(cx) {
            let Some(row) = self.rows.get(index).copied() else { continue };
            let selected = self.selected == Some(index);
            let tint = if selected { vec(p.primary, 0.16) } else { vec(0, 0.0) };
            let item = match row {
                Row::File(fi) => {
                    let f = &self.files[fi];
                    let item = list.item(cx, index, id!(FileRow));
                    item.label(cx, ids!(file_mark)).set_text(cx, if self.folded.contains(&f.path) { "▶" } else { "▼" });
                    item.label(cx, ids!(file_path)).set_text(cx, &f.path);
                    item.label(cx, ids!(file_count)).set_text(cx, &f.matches.to_string());
                    item
                }
                Row::Line(fi, li) => {
                    let h = &self.files[fi].lines[li];
                    let item = list.item(cx, index, id!(LineRow));
                    item.label(cx, ids!(match_line)).set_text(cx, &h.line.to_string());
                    let lit = h.segs.len() / 2;
                    for i in 0..MAX_LIT {
                        let on = i < lit;
                        let plain = item.label(cx, PLAIN[i]);
                        plain.set_visible(cx, on);
                        item.view(cx, LIT_BG[i]).set_visible(cx, on);
                        if on {
                            plain.set_text(cx, &h.segs[2 * i]);
                            item.label(cx, LIT[i]).set_text(cx, &h.segs[2 * i + 1]);
                        }
                    }
                    item.label(cx, ids!(tail)).set_text(cx, h.segs.last().map_or("", String::as_str));
                    item
                }
            };
            let mut v = item.as_view();
            script_apply_eval!(cx, v, { draw_bg +: { color: #(tint) } });
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for SearchPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Signal = event {
            self.drain(cx);
        }
        if self.debounce.is_event(event).is_some() {
            self.debounce = Timer::empty();
            self.run(cx);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if actions.is_empty() {
            return;
        }
        for path in [ids!(query), ids!(include)] {
            let input = self.view.text_input(cx, path);
            if input.changed(&actions).is_some() {
                cx.stop_timer(self.debounce);
                self.debounce = cx.start_timeout(DEBOUNCE);
            }
            if input.returned(&actions).is_some() {
                let pending = self.debounce.0 != 0;
                self.run(cx);
                if !pending {
                    match self.selected {
                        Some(i) => self.open_row(cx, i, 0),
                        None => {
                            self.move_selection(cx, true);
                            if let Some(i) = self.selected {
                                self.open_row(cx, i, 0);
                            }
                        }
                    }
                    input.take_key_focus(cx);
                }
            }
            if let Some(key) = input.key_down_unhandled(&actions) {
                match key.key_code {
                    KeyCode::ArrowDown => self.move_selection(cx, true),
                    KeyCode::ArrowUp => self.move_selection(cx, false),
                    _ => {}
                }
            }
        }
        for (i, path) in [ids!(case_key), ids!(regex_key), ids!(word_key)].into_iter().enumerate() {
            if self.view.button(cx, path).clicked(&actions) {
                self.toggle(cx, i);
            }
        }
        let list = self.view.portal_list(cx, ids!(results));
        for (index, item) in list.items_with_actions(&actions) {
            let Some(up) = item.as_view().finger_up(&actions) else { continue };
            if up.cancelled || !up.device.is_primary_hit() {
                continue;
            }
            // The clicked match: the lit one under the pointer, else the
            // nearest one left of it, else the first.
            let mut nth = 0;
            for (i, path) in LIT_BG.iter().enumerate() {
                let bg = item.view(cx, *path);
                if !bg.visible() {
                    break;
                }
                if bg.area().rect(cx).pos.x <= up.abs.x {
                    nth = i;
                }
            }
            self.open_row(cx, index, nth);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let status = self.status();
        self.view.label(cx, ids!(status)).set_text(cx, &status);
        self.draw_keys(cx);
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                self.draw_results(cx, &mut list);
            }
        }
        if std::mem::take(&mut self.want_focus) {
            self.view.text_input(cx, ids!(query)).take_key_focus(cx);
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(q: &str, opts: SearchOptions, line: &str) -> Vec<(usize, usize)> {
        let m = Matcher::new(q, opts).unwrap();
        let mut out = Vec::new();
        m.find_in_line(line, &mut out);
        out
    }

    const LIT: SearchOptions = SearchOptions { case_sensitive: false, regex: false, whole_word: false };

    #[test]
    fn case() {
        let cs = SearchOptions { case_sensitive: true, ..LIT };
        assert_eq!(spans("foo", LIT, "Foo foo FOO"), vec![(0, 3), (4, 7), (8, 11)]);
        assert_eq!(spans("foo", cs, "Foo foo FOO"), vec![(4, 7)]);
        assert_eq!(spans("été", LIT, "ÉTÉ été"), vec![(0, 5), (6, 11)]);
    }

    #[test]
    fn whole_word() {
        let w = SearchOptions { whole_word: true, ..LIT };
        assert_eq!(spans("id", w, "id ids _id id_ (id)"), vec![(0, 2), (16, 18)]);
        let wcs = SearchOptions { whole_word: true, case_sensitive: true, ..LIT };
        assert_eq!(spans("a", wcs, "aa a_a a"), vec![(7, 8)]);
        // A word match after a rejected one in the same word.
        assert_eq!(spans("ab", w, "abab ab"), vec![(5, 7)]);
    }

    #[test]
    fn regex() {
        let re = SearchOptions { regex: true, case_sensitive: true, ..LIT };
        assert_eq!(spans("Ide(Action|Shell)", re, "IdeAction and IdeShell, IdeX"), vec![(0, 9), (14, 22)]);
        assert_eq!(spans("^fn", re, "fn a() { fn b() }"), vec![(0, 2)]);
        assert_eq!(spans("x*", re, "abc"), vec![]);
        assert!(Matcher::new("(", re).is_err());
    }

    #[test]
    fn literal_escaping() {
        let cs = SearchOptions { case_sensitive: true, ..LIT };
        for q in ["a.b", "(x)", "[1]", "a+b*c?", "\\n", "$^|{}", "#&-~", "fn focus_terminal"] {
            let line = format!("zz {q} zz");
            assert_eq!(spans(q, LIT, &line), vec![(3, 3 + q.len())], "{q}");
            assert_eq!(spans(q, cs, &line), vec![(3, 3 + q.len())], "{q}");
        }
        assert_eq!(spans("a.b", LIT, "axb"), vec![]);
    }

    #[test]
    fn columns_multibyte() {
        let line = "  é→ x = «IdeAction»;";
        let s = line.find("IdeAction").unwrap();
        assert_eq!(column_of(line, s), 11);
        let budget = AtomicUsize::new(0);
        let m = Matcher::new("IdeAction", LIT).unwrap();
        let f = search_text(&m, "a.rs", &format!("first\r\n{line}\n"), &budget).unwrap();
        assert_eq!((f.lines.len(), f.matches), (1, 1));
        assert_eq!((f.lines[0].line, f.lines[0].cols.clone()), (2, vec![11]));
        assert_eq!(f.lines[0].segs, ["é→ x = «", "IdeAction", "»;"]);
    }

    #[test]
    fn segments_cut_long_heads() {
        let line = "\t\tlet some_long_binding_name = IdeAction::None;";
        let s = line.find("IdeAction").unwrap();
        let segs = segments(line, &[(s, s + 9)]);
        assert_eq!(segs, ["…ding_name = ", "IdeAction", "::None;"]);
        assert_eq!(segs[0].chars().count(), 13);
    }

    #[test]
    fn one_row_per_line_every_match_lit() {
        let budget = AtomicUsize::new(0);
        let m = Matcher::new("ab", LIT).unwrap();
        let f = search_text(&m, "x", "ab cd ab\nnone\nxabx  ", &budget).unwrap();
        assert_eq!((f.lines.len(), f.matches), (2, 3));
        assert_eq!(f.lines[0].cols, [1, 7]);
        assert_eq!(f.lines[0].segs, ["", "ab", " cd ", "ab", ""]);
        assert_eq!((f.lines[1].line, f.lines[1].cols.clone()), (3, vec![2]));
        assert_eq!(f.lines[1].segs, ["x", "ab", "x"]);
        // Past MAX_LIT the matches are counted and stay in the tail.
        let line = "a ".repeat(MAX_LIT + 2);
        let m = Matcher::new("a", LIT).unwrap();
        let f = search_text(&m, "x", &line, &budget).unwrap();
        assert_eq!(f.matches, MAX_LIT + 2);
        assert_eq!(f.lines[0].segs.len(), 2 * MAX_LIT + 1);
        assert_eq!(f.lines[0].segs.last().unwrap(), " a a");
    }

    #[test]
    fn anchors_and_boundaries_see_the_whole_line() {
        let re = SearchOptions { regex: true, case_sensitive: true, ..LIT };
        // `^` holds only at the line's start, not after an earlier match.
        assert_eq!(spans("^a", re, "aaa"), vec![(0, 1)]);
        assert_eq!(spans("^\\s*fn", re, "  fn a() { fn b() }"), vec![(0, 4)]);
        // `\b` after an earlier match looks at the character before it.
        assert_eq!(spans("\\bab", re, "ab ab xab"), vec![(0, 2), (3, 5)]);
        assert_eq!(spans("b\\b", re, "abab b"), vec![(3, 4), (5, 6)]);
        // The scan resumes after each match; starts stay the leftmost.
        assert_eq!(spans("a+b", re, "aab b aaab"), vec![(0, 3), (6, 10)]);
        assert_eq!(spans("x?", re, "ab"), vec![]);
    }

    #[test]
    fn budget_caps() {
        let budget = AtomicUsize::new(MAX_MATCHES - 2);
        let m = Matcher::new("a", LIT).unwrap();
        let f = search_text(&m, "x", "a a a a\na", &budget).unwrap();
        assert_eq!((f.lines.len(), f.matches), (1, 2));
        assert_eq!(f.lines[0].cols, [1, 3]);
    }

    #[test]
    fn include() {
        let f = include_filter("*.rs, apps/code !tests");
        assert!(f("libs/x/src/lib.rs"));
        assert!(f("apps/code/Cargo.toml"));
        assert!(!f("libs/x/tests/a.rs"));
        assert!(!f("README.md"));
        assert!(include_filter("")("anything"));
    }
}
