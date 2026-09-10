//! LogList — the pane of arriving messages: a level mark per line, the
//! newest one always in view, and the file references inside the text made
//! pressable.
//!
//! The platform already keeps every `log!` and `error!` line, with its
//! level, in a process-wide ring (`makepad_platform::log_ring`); it was
//! moved out of the automation surface precisely so an app could show its
//! own log. Nothing was ever built to show it, so two applications in this
//! repository grew their own pane instead — one drawing the tail into a
//! single multi-line label, one a hand-written virtual list with its own
//! icons. This is the third one, written once.
//!
//! **It does not read the ring.** A library widget that reached into a
//! global would draw lines nobody handed it, could not be tested without a
//! process that had logged something, and would be useless to the host whose
//! lines come from a build server, a device or a file. The host pumps lines
//! in with [`LogListRef::push`]; feeding it straight from the ring is three
//! lines in the host's own tick, and the level type is the platform's own so
//! that pump needs no conversion.
//!
//! **Sticking is the whole point.** A console that does not follow the
//! newest line is a console you scroll for a living; one that follows it
//! while you are reading the line above is worse. The underlying virtual
//! list already resolves that (`auto_tail`): it holds the bottom while lines
//! arrive, lets go the instant a reader scrolls away from it, and takes hold
//! again when they scroll back down. What this adds is the report — the pane
//! says when it lets go, and counts what arrived while nobody was looking,
//! so a host can offer the way back.
//!
//! **What a reference is.** Anything shaped like `path:line` or
//! `path:line:column`, which is what this platform's log sink writes and
//! what every compiler on every platform writes. The scan is deliberately
//! syntactic: the widget marks what LOOKS like a reference and reports the
//! press; whether that path can actually be opened is the host's question,
//! not the pane's. Windows drive letters survive (`C:\src\a.rs:12:3` is one
//! reference, not a line number of 3 in a file called `C`), clock times and
//! URLs with ports are left alone, and trailing punctuation stays outside
//! the link.
//!
//! **What it costs.** The list virtualises, so the number of rows that exist
//! as widgets is the number on screen — about thirty — whether the pane
//! holds two hundred lines or ten thousand. Per held line the cost is its
//! own text plus about fifty-six bytes of book-keeping, so ten thousand
//! ordinary log lines is on the order of a megabyte and a half. Pushing a
//! line is constant work: one filter test and one scan over that line's own
//! bytes for references. Changing the filter or the level floor is one pass
//! over every held line — ten thousand string tests, once per change, not
//! once per frame. Nothing here is quadratic, and nothing grows without a
//! bound: `cap` is the number of lines kept, 2000 by default to match the
//! platform ring, and the oldest go first.
//!
//! Two things it does not do. It never asks how tall a row is, so with
//! wrapped lines of different heights the scroll bar is the virtual list's
//! estimate rather than a measurement — at ten thousand lines the thumb
//! drifts as you drag. And it has no notion of a line arriving twice: a
//! message repeated a thousand times is a thousand rows, because collapsing
//! them is a policy about the host's messages, not about panes.
//!
//! One registration order matters: this module's `script_mod` must run after
//! `text_flow`'s, because a row's link preset is derived from that module's
//! at the moment the preset is evaluated.
use crate::{
    makepad_derive_widget::*, makepad_draw::*,
    portal_list::{PortalList, PortalListWidgetExt, PortalListWidgetRefExt},
    text_flow::TextFlowWidgetRefExt, view::View, widget::*,
};
use crate::makepad_platform::log::LogLevel;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LogListFloor = set_type_default() do #(LogListFloor::script_api(vm))

    mod.widgets.LogListBase = #(LogList::register_widget(vm))

    // A reference inside a line. The link preset's own colours are fixed
    // greys that are barely legible on a light ground, so the three states
    // are restated here from theme roles.
    mod.widgets.LogListLink = mod.widgets.TextFlowLink{
        color: theme.color_info
        color_hover: theme.color_text
        color_down: theme.color_info
        margin: Inset{top: 0., right: 2., bottom: 0., left: 0.}
    }

    // One arrived line: a stripe and a word for the level, then the message
    // with its references drawn as links.
    //
    // The stripe is painted by the row's own background rather than being a
    // child of it, because the row is Fit around its text and a Fill child
    // inside a Fit resolves to nothing and never paints.
    mod.widgets.LogListRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        padding: Inset{top: 2., right: theme.space_2, bottom: 2., left: theme.space_2}
        show_bg: true
        draw_bg +: {
            color_row: uniform(theme.color_bg_even)
            color_row_alt: uniform(theme.color_bg_odd)
            color_mark: instance(theme.color_text_meta)
            alternate: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 0.5)
                sdf.fill(self.color_row.mix(self.color_row_alt, self.alternate))
                sdf.box(0.0, 0.0, 3.0, self.rect_size.y, 0.5)
                sdf.fill(self.color_mark)
                return sdf.result
            }
        }
        tag := Label{
            // Wide enough for the longest level word with room to spare, and
            // its own padding taken off: a label carries three points either
            // side by default, and the two five-letter words wrapped to a
            // second line in a column sized without counting them.
            width: 46.
            padding: 0.
            text: ""
            draw_text +: {
                text_style: theme.font_code{font_size: theme.font_size_p}
                color: theme.color_text_meta
            }
        }
        body := TextFlow{
            width: Fill
            height: Fit
            selectable: true
            text_style_normal: theme.font_code{font_size: theme.font_size_p}
            draw_text +: {
                color: theme.color_text
            }
            // A template of this row's own rather than an override of the
            // text flow's inherited `link`: adding a name is a thing the
            // template machinery plainly does, while whether a same-named
            // one at instance level wins over the inherited one is a
            // question nothing in this repository answers.
            reference := mod.widgets.LogListLink{}
        }
    }

    // What the pane shows instead of rows. A blank rectangle cannot say
    // whether nothing has been logged or the filter matched nothing, and
    // those are the two states this pane spends most of its life in.
    mod.widgets.LogListEmpty = View{
        width: Fill
        height: Fit
        padding: Inset{top: 6., right: theme.space_2, bottom: 6., left: theme.space_2}
        note := Label{
            width: Fill
            text: ""
            draw_text +: {
                text_style: theme.font_code{font_size: theme.font_size_p}
                color: theme.color_text_meta
            }
        }
    }

    mod.widgets.LogList = set_type_default() do mod.widgets.LogListBase{
        width: Fill
        height: Fill
        // The field the rows sit on, and all anyone sees of the pane before
        // the first line arrives. Declared rather than assigned: a plain
        // View's quad has no colour of its own, so the input has to be
        // brought into being here the way the solid view preset does it.
        show_bg: true
        draw_bg +: {
            color: instance(theme.color_bg_container)
            pixel: fn() {
                return Pal.premul(self.color)
            }
        }

        /** lines kept before the oldest are forgotten 100..20000 step 100 */
        cap: 2000
        /** a fixture, one string each: "level | text" */
        lines: []
        floor: mod.widgets.LogListFloor.Everything
        empty_text: "nothing logged yet"
        filtered_text: "nothing matches the filter"

        color_note: theme.color_text_meta
        color_waiting: theme.color_info
        color_warning: theme.color_warning
        color_error: theme.color_error
        color_panic: theme.color_panic

        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            // The sticking rule itself. Everything else here keeps a console
            // out of the way of the app it is embedded in: no key focus
            // stolen from the thing being debugged, no drag-to-scroll
            // fighting a text selection, no rubber band at the top of a
            // list whose top is ancient history.
            auto_tail: true
            bounce_at_start: false
            capture_overload: false
            grab_key_focus: false
            drag_scrolling: false
            selectable: true
            Row := mod.widgets.LogListRow{}
            Empty := mod.widgets.LogListEmpty{}
        }
    }
}

/// The least severe level a row may be and still show.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum LogListFloor {
    /// Every line, including the ones marking work in progress.
    #[pick]
    #[default]
    Everything,
    /// Ordinary notes and worse; drops in-progress markers.
    Notes,
    /// Warnings and worse.
    Warnings,
    /// Only what actually went wrong.
    Errors,
}

/// How bad a level is. The platform's enum is not ordered — its variants are
/// in the order somebody wrote them — so the order that decides what a floor
/// keeps is written here, once.
pub fn severity(level: LogLevel) -> u8 {
    match level {
        LogLevel::Wait => 0,
        LogLevel::Log => 1,
        LogLevel::Warning => 2,
        LogLevel::Error => 3,
        LogLevel::Panic => 4,
    }
}

impl LogListFloor {
    /// The lowest severity this floor lets through.
    pub fn severity(self) -> u8 {
        match self {
            LogListFloor::Everything => 0,
            LogListFloor::Notes => 1,
            LogListFloor::Warnings => 2,
            LogListFloor::Errors => 3,
        }
    }
}

/// The word in the gutter. Short enough that the widest of them fits the
/// fixed tag column, so the messages line up down the pane.
pub fn tag_of(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Wait => "wait",
        LogLevel::Log => "log",
        LogLevel::Warning => "warn",
        LogLevel::Error => "error",
        LogLevel::Panic => "panic",
    }
}

/// One markup line as `level | text`.
///
/// A head that is not one of the level words means the line named no level:
/// the whole of it is the message at note level, which is what stops a
/// message that happens to contain a bar from losing its front.
pub fn split_line(line: &str) -> (LogLevel, &str) {
    if let Some((head, rest)) = line.split_once('|') {
        if let Some(level) = level_named(head.trim()) {
            return (level, rest.trim());
        }
    }
    (LogLevel::Log, line)
}

/// The gutter words, read back. One spelling apiece: a markup vocabulary
/// with synonyms in it is one nobody can remember the shape of.
fn level_named(word: &str) -> Option<LogLevel> {
    match word {
        "wait" => Some(LogLevel::Wait),
        "log" => Some(LogLevel::Log),
        "warn" => Some(LogLevel::Warning),
        "error" => Some(LogLevel::Error),
        "panic" => Some(LogLevel::Panic),
        _ => None,
    }
}

/// A place in a source file, as a line said it.
///
/// `line` and `column` are as written — one-based in everything that emits
/// this shape, but the pane does not check that and does not renumber.
#[derive(Clone, Debug, PartialEq)]
pub struct LogReference {
    pub path: String,
    pub line: u32,
    /// Absent when the line named only a line.
    pub column: Option<u32>,
}

/// Where a reference sits in a line: byte offsets into that line's text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogSpan {
    pub start: u32,
    pub end: u32,
}

/// Brackets and quotes a reference may be wrapped in.
const OPENERS: &[u8] = b"([{<\"'`";
/// Punctuation a sentence may leave hanging off the end of one. The colon is
/// here so `see foo.rs:12:` links the reference and not the colon.
const CLOSERS: &[u8] = b")]}>\"'`,;.:!?";

/// Every reference in a line, in the order they appear, never overlapping.
///
/// Whitespace-separated tokens are the unit: a reference cannot contain a
/// space, and treating the line as one long string instead would need a
/// parser to say where a path started.
pub fn references(text: &str) -> Vec<LogSpan> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let from = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        // Only ASCII bytes are ever skipped over here, so the offsets stay
        // on character boundaries whatever the line is written in.
        let (start, end) = trim_edges(bytes, from, at);
        if start < end && LogReference::parse(&text[start..end]).is_some() {
            found.push(LogSpan { start: start as u32, end: end as u32 });
        }
    }
    found
}

fn trim_edges(bytes: &[u8], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && OPENERS.contains(&bytes[start]) {
        start += 1;
    }
    while end > start && CLOSERS.contains(&bytes[end - 1]) {
        end -= 1;
    }
    (start, end)
}

fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Could this be a path? A slash either way settles it; failing that it
/// needs an extension — a dot followed by a letter — so that `13:45` is a
/// time and `1.5:30` is not a file.
fn plausible_path(path: &str) -> bool {
    if path.contains('/') || path.contains('\\') {
        return true;
    }
    path.as_bytes().windows(2).any(|pair| pair[0] == b'.' && pair[1].is_ascii_alphabetic())
}

impl LogReference {
    /// Read one already-trimmed token as a reference.
    ///
    /// The numbers are taken from the RIGHT, which is what keeps a Windows
    /// drive letter attached to its path: `C:\a.rs:12:3` has two trailing
    /// number groups and everything before them is the path, colon and all.
    pub fn parse(token: &str) -> Option<LogReference> {
        // A scheme's own `//` would otherwise make every host:port pair a
        // file at a line number.
        if token.contains("://") {
            return None;
        }
        let (head, last) = token.rsplit_once(':')?;
        let last_number = number(last)?;
        let (path, line, column) = match head.rsplit_once(':') {
            Some((before, middle)) => match number(middle) {
                // Two number groups only count as line and column when what
                // is left in front of them still looks like a path; `12:30`
                // in a clock time does not.
                Some(middle_number) if plausible_path(before) => {
                    (before, middle_number, Some(last_number))
                }
                _ => (head, last_number, None),
            },
            None => (head, last_number, None),
        };
        if !plausible_path(path) {
            return None;
        }
        Some(LogReference { path: path.to_string(), line, column })
    }
}

/// Lines kept when nothing says otherwise — the platform ring's own size, so
/// a pane fed from the ring holds everything the ring still has.
pub const DEFAULT_CAP: usize = 2000;

/// One line as the pane holds it.
pub struct LogEntry {
    pub level: LogLevel,
    pub text: String,
    /// Scanned once, when the line arrived. A line is pushed once and drawn
    /// on every frame it is on screen, so the scan belongs here rather than
    /// in the draw.
    pub refs: Vec<LogSpan>,
}

/// Everything the pane knows that is not drawing: the lines, which of them
/// the filter shows, and what arrived while the reader was away.
///
/// It is a plain struct rather than fields on the widget so the rules that
/// decide what a reader sees can be tested without a window.
pub struct LogBuffer {
    lines: VecDeque<LogEntry>,
    /// Serial numbers of the lines the filter keeps, oldest first.
    kept: VecDeque<u64>,
    /// How many lines have fallen off the front since the pane started. A
    /// serial minus this is an index into `lines` — which is what lets the
    /// kept list survive a trim without a pass over every entry in it.
    dropped: u64,
    cap: usize,
    filter: String,
    floor: LogListFloor,
    following: bool,
    unseen: usize,
}

impl Default for LogBuffer {
    fn default() -> LogBuffer {
        LogBuffer {
            lines: VecDeque::new(),
            kept: VecDeque::new(),
            dropped: 0,
            cap: DEFAULT_CAP,
            filter: String::new(),
            floor: LogListFloor::Everything,
            // A pane nobody has scrolled is at the newest line by
            // construction, so it starts stuck rather than starting adrift
            // and counting its own first lines as missed.
            following: true,
            unseen: 0,
        }
    }
}

impl LogBuffer {
    /// Keep one arrived line.
    pub fn push(&mut self, level: LogLevel, text: &str) {
        let entry = LogEntry { level, text: text.to_string(), refs: references(text) };
        let serial = self.dropped + self.lines.len() as u64;
        if self.keeps(&entry) {
            self.kept.push_back(serial);
            if !self.following {
                self.unseen = self.unseen.saturating_add(1);
            }
        }
        self.lines.push_back(entry);
        self.trim();
    }

    /// Forget every line, and everything owed about them.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.kept.clear();
        self.unseen = 0;
        // The serials do not restart. Nothing outside holds one, and a
        // counter that goes backwards is a bug waiting for a reader.
    }

    /// How many lines to keep. A cap of nothing would empty the pane on the
    /// next push, which is never what a caller means.
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.max(1);
        self.trim();
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Show only lines whose text contains `filter`, ignoring case, and
    /// whose level is at least `floor`. Unchanged terms cost one comparison;
    /// a change costs one pass over every held line.
    pub fn filter_by(&mut self, filter: &str, floor: LogListFloor) {
        if self.filter == filter && self.floor == floor {
            return;
        }
        self.filter = filter.to_string();
        self.floor = floor;
        self.refilter();
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn floor(&self) -> LogListFloor {
        self.floor
    }

    /// Whether the pane is holding the newest line. Told to it by the list,
    /// which is the only thing that knows where the reader is.
    pub fn set_following(&mut self, following: bool) {
        self.following = following;
        if following {
            self.unseen = 0;
        }
    }

    pub fn following(&self) -> bool {
        self.following
    }

    /// Lines that have arrived and been shown since the reader scrolled
    /// away. Zero whenever the pane is following.
    pub fn unseen(&self) -> usize {
        self.unseen
    }

    /// Rows the filter shows.
    pub fn len(&self) -> usize {
        self.kept.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }

    /// Lines held, whether the filter shows them or not.
    pub fn held(&self) -> usize {
        self.lines.len()
    }

    /// The line drawn in row `row`, counting the shown rows only.
    pub fn row(&self, row: usize) -> Option<&LogEntry> {
        let serial = *self.kept.get(row)?;
        self.lines.get((serial - self.dropped) as usize)
    }

    fn keeps(&self, entry: &LogEntry) -> bool {
        if severity(entry.level) < self.floor.severity() {
            return false;
        }
        if self.filter.is_empty() {
            return true;
        }
        entry.text.to_lowercase().contains(&self.filter.to_lowercase())
    }

    fn refilter(&mut self) {
        self.kept.clear();
        for (index, entry) in self.lines.iter().enumerate() {
            if severity(entry.level) >= self.floor.severity()
                && (self.filter.is_empty()
                    || entry.text.to_lowercase().contains(&self.filter.to_lowercase()))
            {
                self.kept.push_back(self.dropped + index as u64);
            }
        }
    }

    fn trim(&mut self) {
        while self.lines.len() > self.cap {
            self.lines.pop_front();
            let gone = self.dropped;
            self.dropped += 1;
            if self.kept.front() == Some(&gone) {
                self.kept.pop_front();
            }
        }
    }
}

/// What the pane tells its host.
#[derive(Clone, Debug, Default)]
pub enum LogListAction {
    /// A reference inside a line was pressed. The pane does nothing about
    /// it: opening an editor, or deciding the path means nothing here, is
    /// the host's business.
    ReferencePressed(LogReference),
    /// The pane took hold of the newest line, or let go of it because the
    /// reader scrolled away. A host that offers a way back listens for this.
    Following(bool),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct LogList {
    #[deref]
    view: View,
    /// Lines kept before the oldest are forgotten.
    #[live(DEFAULT_CAP)]
    cap: usize,
    /// A fixture written straight from markup, one string each:
    /// `"level | text"`. For a catalogue page, a screenshot or a test with
    /// no live source. It REPLACES whatever the pane holds whenever markup
    /// changes it, so a host that pushes its own lines must leave it empty.
    #[live]
    pub lines: Vec<String>,
    /// The fixture as it was last read, so a live reload of `lines` lands
    /// without a hook to catch it.
    #[rust]
    from_lines: Vec<String>,
    /// Shows only lines whose text contains this, ignoring case.
    #[live]
    filter: String,
    /// Shows only lines this severe or worse.
    #[live]
    floor: LogListFloor,
    /// What an empty pane says when nothing has been logged.
    #[live]
    empty_text: String,
    /// What it says when there are lines but the filter shows none of them.
    /// Not the same sentence: one is a quiet app, the other is a filter to
    /// undo, and a pane that says "nothing logged yet" to the second sends
    /// the reader looking for a bug in the app.
    #[live]
    filtered_text: String,
    #[live]
    color_note: Vec4f,
    #[live]
    color_waiting: Vec4f,
    #[live]
    color_warning: Vec4f,
    #[live]
    color_error: Vec4f,
    #[live]
    color_panic: Vec4f,
    #[rust]
    buffer: LogBuffer,
    /// Whether the list has drawn at least once. Until it has, it cannot say
    /// where the reader is.
    #[rust]
    drawn: bool,
}

impl LogList {
    /// The level's colour, for the stripe and the word alike. One colour per
    /// level rather than one for the stripe and another for the text: two
    /// would let a row say two different things about how bad it is.
    fn color_of(&self, level: LogLevel) -> Vec4f {
        match level {
            LogLevel::Wait => self.color_waiting,
            LogLevel::Log => self.color_note,
            LogLevel::Warning => self.color_warning,
            LogLevel::Error => self.color_error,
            LogLevel::Panic => self.color_panic,
        }
    }

    fn draw_rows(&self, cx: &mut Cx2d, list: &mut PortalList) {
        if self.buffer.is_empty() {
            let text =
                if self.buffer.held() == 0 { &self.empty_text } else { &self.filtered_text };
            list.set_item_range(cx, 0, 1);
            while let Some(row) = list.next_visible_item(cx) {
                // The list keeps offering rows until one of them draws
                // nothing — it does not stop at the range on its own. So the
                // note is drawn once and the offer after it declined, or the
                // same sentence fills the pane six times over.
                if row > 0 {
                    continue;
                }
                let item = list.item(cx, row, id!(Empty));
                item.widget(&**cx, ids!(note)).set_text(cx, text);
                item.draw_all(cx, &mut Scope::empty());
            }
            return;
        }
        list.set_item_range(cx, 0, self.buffer.len());
        while let Some(row) = list.next_visible_item(cx) {
            let Some(entry) = self.buffer.row(row) else { continue };
            let mut item = list.item(cx, row, id!(Row));
            let mark = self.color_of(entry.level);
            let alternate = if row & 1 == 1 { 1.0f32 } else { 0.0f32 };
            script_apply_eval!(cx, item, {
                draw_bg +: {alternate: #(alternate), color_mark: #(mark)}
            });
            let mut tag = item.widget(&**cx, ids!(tag));
            script_apply_eval!(cx, tag, {
                draw_text +: {color: #(mark)}
            });
            tag.set_text(cx, tag_of(entry.level));
            // The message is laid out by a text flow rather than a label
            // because the references have to sit INSIDE the wrapped text —
            // a link at the end of a row would be a different widget saying
            // a different thing.
            while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                let flow_ref = step.as_text_flow();
                let Some(mut flow) = flow_ref.borrow_mut() else { continue };
                let text = entry.text.as_str();
                let mut at = 0usize;
                for span in &entry.refs {
                    let (start, end) = (span.start as usize, span.end as usize);
                    if start > at {
                        flow.draw_text(cx, &text[at..start]);
                    }
                    match LogReference::parse(&text[start..end]) {
                        Some(reference) => {
                            flow.draw_link(cx, id!(reference), reference, &text[start..end])
                        }
                        // Unreachable while the scan and the parse agree,
                        // and plain text is the right answer if they ever
                        // stop: a reader loses a link, not the message.
                        None => flow.draw_text(cx, &text[start..end]),
                    }
                    at = end;
                }
                if at < text.len() {
                    flow.draw_text(cx, &text[at..]);
                }
            }
        }
    }
}

impl Widget for LogList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Read from the live fields every frame so that a filter typed into
        // the DSL, a live reload, and a host calling the setters all take
        // the same path. Both calls return immediately when nothing changed.
        let (cap, floor) = (self.cap, self.floor);
        self.buffer.set_cap(cap);
        // Markup wins whenever its list differs from what was last read, the
        // way the timeline reads its events: a live reload and a control
        // that rewrites `lines` both land with no hook to catch them.
        if self.from_lines != self.lines {
            self.from_lines = self.lines.clone();
            self.buffer.clear();
            for line in &self.lines {
                let (level, text) = split_line(line);
                self.buffer.push(level, text);
            }
        }
        let filter = std::mem::take(&mut self.filter);
        self.buffer.filter_by(&filter, floor);
        self.filter = filter;
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_rows(cx, &mut list);
                self.drawn = true;
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else { return };
        // A list that has not drawn yet answers "not at the end", because
        // that answer is worked out while drawing. Reporting before the
        // first draw would say the pane had let go of a line it had not
        // shown, and would count the app's first lines as missed.
        if !self.drawn {
            return;
        }
        let uid = self.widget_uid();
        let list = self.view.portal_list(cx, ids!(list));
        // Asked after the list has handled the frame: whether the pane is
        // still holding the newest line is the list's answer, and it changes
        // inside the scroll it just processed.
        let following = list.is_at_end();
        if following != self.buffer.following() {
            self.buffer.set_following(following);
            cx.widget_action(uid, LogListAction::Following(following));
        }
        if !list.any_items_with_actions(actions) {
            return;
        }
        for reference in actions.filter_actions_data::<LogReference>() {
            cx.widget_action(uid, LogListAction::ReferencePressed(reference.clone()));
        }
    }
}

impl LogListRef {
    /// Keep one arrived line.
    pub fn push(&self, cx: &mut Cx, level: LogLevel, text: &str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let cap = inner.cap;
        inner.buffer.set_cap(cap);
        inner.buffer.push(level, text);
        inner.view.redraw(cx);
    }

    /// Keep a batch — one redraw for the lot, which is what a host draining
    /// a ring on its own tick wants.
    pub fn extend<'a>(&self, cx: &mut Cx, lines: impl IntoIterator<Item = (LogLevel, &'a str)>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let cap = inner.cap;
        inner.buffer.set_cap(cap);
        for (level, text) in lines {
            inner.buffer.push(level, text);
        }
        inner.view.redraw(cx);
    }

    /// Forget every line.
    pub fn clear(&self, cx: &mut Cx) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.buffer.clear();
        inner.view.redraw(cx);
    }

    /// Show only lines containing this, ignoring case. Empty shows all.
    pub fn set_filter(&self, cx: &mut Cx, filter: &str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.filter == filter {
            return;
        }
        inner.filter = filter.to_string();
        // Applied here rather than left to the next draw, because a caller
        // that sets a filter and then asks what is showing is asking about
        // the filter it just set.
        let floor = inner.floor;
        inner.buffer.filter_by(filter, floor);
        inner.view.redraw(cx);
    }

    /// Show only lines this severe or worse.
    pub fn set_floor(&self, cx: &mut Cx, floor: LogListFloor) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.floor == floor {
            return;
        }
        inner.floor = floor;
        let filter = std::mem::take(&mut inner.filter);
        inner.buffer.filter_by(&filter, floor);
        inner.filter = filter;
        inner.view.redraw(cx);
    }

    /// Take hold of the newest line again, wherever the reader had scrolled.
    pub fn follow(&self, cx: &mut Cx) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.buffer.set_following(true);
        let list = inner.view.portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.scroll_to_end(cx);
        inner.view.redraw(cx);
    }

    /// Whether the pane is holding the newest line.
    pub fn following(&self) -> bool {
        self.borrow().map(|inner| inner.buffer.following()).unwrap_or(true)
    }

    /// Lines shown since the reader scrolled away from the newest.
    pub fn unseen(&self) -> usize {
        self.borrow().map(|inner| inner.buffer.unseen()).unwrap_or(0)
    }

    /// Rows the filter shows.
    pub fn showing(&self) -> usize {
        self.borrow().map(|inner| inner.buffer.len()).unwrap_or(0)
    }

    /// Lines held, filtered or not.
    pub fn held(&self) -> usize {
        self.borrow().map(|inner| inner.buffer.held()).unwrap_or(0)
    }

    /// The reference pressed this pass, if one was.
    pub fn reference_pressed(&self, actions: &Actions) -> Option<LogReference> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast() {
            LogListAction::ReferencePressed(reference) => Some(reference),
            _ => None,
        }
    }

    /// Whether the pane took hold of, or let go of, the newest line.
    pub fn following_changed(&self, actions: &Actions) -> Option<bool> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast() {
            LogListAction::Following(following) => Some(following),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one span in a line, as text.
    fn only<'a>(text: &'a str, spans: &[LogSpan]) -> &'a str {
        assert_eq!(spans.len(), 1, "expected one reference in {text:?}, got {spans:?}");
        &text[spans[0].start as usize..spans[0].end as usize]
    }

    fn buffer() -> LogBuffer {
        LogBuffer::default()
    }

    #[test]
    fn a_path_with_a_line_number_is_a_reference() {
        let line = "[E] widgets/src/log_list.rs:42 - it went wrong";
        let spans = references(line);
        assert_eq!(only(line, &spans), "widgets/src/log_list.rs:42");
        let reference = LogReference::parse(only(line, &spans)).unwrap();
        assert_eq!(reference.path, "widgets/src/log_list.rs");
        assert_eq!(reference.line, 42);
        assert_eq!(reference.column, None, "the line named no column, so neither do we");
    }

    #[test]
    fn a_reference_carries_the_column_when_the_line_gave_one() {
        let reference = LogReference::parse("widgets/src/log_list.rs:42:9").unwrap();
        assert_eq!(reference.path, "widgets/src/log_list.rs");
        assert_eq!((reference.line, reference.column), (42, Some(9)));
    }

    #[test]
    fn a_drive_letter_stays_part_of_the_path() {
        // Read left to right, the first colon would make the path "C" and
        // the line number 12 into a column.
        let reference = LogReference::parse("C:\\src\\a.rs:12:3").unwrap();
        assert_eq!(reference.path, "C:\\src\\a.rs");
        assert_eq!((reference.line, reference.column), (12, Some(3)));
    }

    #[test]
    fn a_clock_time_is_not_a_reference() {
        assert_eq!(references("12:30:45 the deck ran out"), vec![]);
        assert_eq!(LogReference::parse("12:30"), None, "nor is an hour and a minute");
    }

    #[test]
    fn a_url_with_a_port_is_left_alone() {
        assert_eq!(references("listening on http://127.0.0.1:8080 now"), vec![]);
    }

    #[test]
    fn a_word_with_a_number_after_a_colon_is_not_a_file() {
        assert_eq!(references("frames:31 dropped:0"), vec![], "no path in either");
    }

    #[test]
    fn punctuation_around_a_reference_stays_out_of_it() {
        let line = "(see src/a.rs:9), and src/b.rs:10.";
        let spans = references(line);
        assert_eq!(spans.len(), 2);
        assert_eq!(&line[spans[0].start as usize..spans[0].end as usize], "src/a.rs:9");
        assert_eq!(&line[spans[1].start as usize..spans[1].end as usize], "src/b.rs:10");
    }

    #[test]
    fn the_spans_carve_the_line_up_without_losing_a_character() {
        let line = "[W] a/b.rs:3:4 - see also c/d.rs:5";
        let spans = references(line);
        let mut rebuilt = String::new();
        let mut at = 0usize;
        for span in &spans {
            rebuilt.push_str(&line[at..span.start as usize]);
            rebuilt.push_str(&line[span.start as usize..span.end as usize]);
            at = span.end as usize;
        }
        rebuilt.push_str(&line[at..]);
        assert_eq!(rebuilt, line, "the draw walks the line exactly once");
        assert!(spans.windows(2).all(|pair| pair[0].end <= pair[1].start), "and in order");
    }

    #[test]
    fn a_reference_in_a_line_of_other_alphabets_keeps_its_offsets() {
        // The scan steps over bytes; a multi-byte character before the
        // reference would slice the string mid-character if it counted
        // characters instead.
        let line = "звук a/b.rs:7 пропал";
        assert_eq!(only(line, &references(line)), "a/b.rs:7");
    }

    #[test]
    fn the_oldest_lines_go_when_the_cap_is_reached() {
        let mut log = buffer();
        log.set_cap(3);
        for n in 0..5 {
            log.push(LogLevel::Log, &format!("line {n}"));
        }
        assert_eq!(log.held(), 3);
        assert_eq!(log.row(0).unwrap().text, "line 2", "the oldest two are gone");
        assert_eq!(log.row(2).unwrap().text, "line 4");
        assert_eq!(log.row(3).map(|e| e.text.as_str()), None, "and nothing past the end");
    }

    #[test]
    fn a_row_still_finds_its_line_after_the_front_was_dropped() {
        // Rows are addressed by position among what is showing, while lines
        // are held by a serial that never restarts. Getting that mapping
        // wrong shows the wrong line, or panics, only once the cap is hit.
        let mut log = buffer();
        log.set_cap(4);
        for n in 0..4 {
            log.push(if n % 2 == 0 { LogLevel::Error } else { LogLevel::Log }, &format!("{n}"));
        }
        log.filter_by("", LogListFloor::Errors);
        assert_eq!(log.len(), 2);
        log.push(LogLevel::Error, "4");
        log.push(LogLevel::Error, "5");
        assert_eq!(log.held(), 4, "the cap held");
        let showing: Vec<&str> = (0..log.len()).map(|r| log.row(r).unwrap().text.as_str()).collect();
        assert_eq!(showing, vec!["2", "4", "5"], "the dropped errors left with their lines");
    }

    #[test]
    fn a_filter_hides_lines_without_forgetting_them() {
        let mut log = buffer();
        log.push(LogLevel::Log, "the deck loaded");
        log.push(LogLevel::Log, "the DECK ran out");
        log.push(LogLevel::Log, "a file was written");
        log.filter_by("deck", LogListFloor::Everything);
        assert_eq!(log.len(), 2, "case is not part of the question");
        assert_eq!(log.held(), 3, "and the hidden line is still held");
        log.filter_by("", LogListFloor::Everything);
        assert_eq!(log.len(), 3, "clearing the filter brings it back");
    }

    #[test]
    fn a_line_arriving_under_a_filter_is_tested_against_it() {
        let mut log = buffer();
        log.filter_by("deck", LogListFloor::Everything);
        log.push(LogLevel::Log, "the deck loaded");
        log.push(LogLevel::Log, "a file was written");
        assert_eq!(log.len(), 1);
        assert_eq!(log.row(0).unwrap().text, "the deck loaded");
    }

    #[test]
    fn a_floor_keeps_what_is_at_least_as_bad() {
        let mut log = buffer();
        for level in [LogLevel::Wait, LogLevel::Log, LogLevel::Warning, LogLevel::Error, LogLevel::Panic] {
            log.push(level, tag_of(level));
        }
        log.filter_by("", LogListFloor::Warnings);
        let showing: Vec<&str> = (0..log.len()).map(|r| log.row(r).unwrap().text.as_str()).collect();
        assert_eq!(showing, vec!["warn", "error", "panic"]);
        log.filter_by("", LogListFloor::Errors);
        assert_eq!(log.len(), 2, "a warning is not what went wrong");
        log.filter_by("", LogListFloor::Everything);
        assert_eq!(log.len(), 5);
    }

    #[test]
    fn lines_arriving_while_the_reader_is_away_are_counted() {
        let mut log = buffer();
        log.push(LogLevel::Log, "seen");
        assert_eq!(log.unseen(), 0, "a pane at the newest line has missed nothing");
        log.set_following(false);
        log.push(LogLevel::Log, "missed");
        log.push(LogLevel::Log, "missed too");
        assert_eq!(log.unseen(), 2);
    }

    #[test]
    fn a_line_the_filter_hides_is_not_something_the_reader_missed() {
        let mut log = buffer();
        log.filter_by("deck", LogListFloor::Everything);
        log.set_following(false);
        log.push(LogLevel::Log, "a file was written");
        assert_eq!(log.unseen(), 0, "it would not have been on screen either way");
        log.push(LogLevel::Log, "the deck loaded");
        assert_eq!(log.unseen(), 1);
    }

    #[test]
    fn returning_to_the_newest_forgets_the_count() {
        let mut log = buffer();
        log.set_following(false);
        log.push(LogLevel::Log, "missed");
        log.set_following(true);
        assert_eq!(log.unseen(), 0);
    }

    #[test]
    fn clearing_forgets_the_lines_and_the_count() {
        let mut log = buffer();
        log.set_following(false);
        log.push(LogLevel::Error, "it broke");
        log.clear();
        assert_eq!((log.held(), log.len(), log.unseen()), (0, 0, 0));
        log.push(LogLevel::Error, "it broke again");
        assert_eq!(log.row(0).unwrap().text, "it broke again", "and the rows start over");
    }

    #[test]
    fn a_cap_of_nothing_is_a_cap_of_one_line() {
        let mut log = buffer();
        log.set_cap(0);
        log.push(LogLevel::Log, "the only line");
        assert_eq!(log.cap(), 1);
        assert_eq!(log.held(), 1, "never a pane that empties itself as you watch");
    }

    #[test]
    fn lowering_the_cap_drops_from_the_front_at_once() {
        let mut log = buffer();
        for n in 0..10 {
            log.push(LogLevel::Log, &format!("line {n}"));
        }
        log.set_cap(2);
        assert_eq!(log.held(), 2);
        assert_eq!(log.row(0).unwrap().text, "line 8");
    }

    #[test]
    fn a_line_is_scanned_for_references_once_when_it_arrives() {
        let mut log = buffer();
        log.push(LogLevel::Error, "[E] a/b.rs:1:2 - and c/d.rs:3 too");
        assert_eq!(log.row(0).unwrap().refs.len(), 2);
    }

    #[test]
    fn every_level_has_a_word_that_fits_the_gutter() {
        for level in [LogLevel::Wait, LogLevel::Log, LogLevel::Warning, LogLevel::Error, LogLevel::Panic] {
            let word = tag_of(level);
            assert!(!word.is_empty() && word.len() <= 5, "{word:?} is not a gutter word");
        }
    }

    #[test]
    fn a_markup_line_says_its_level_before_a_bar() {
        assert_eq!(split_line("error | it broke"), (LogLevel::Error, "it broke"));
        assert_eq!(split_line("wait|holding"), (LogLevel::Wait, "holding"));
    }

    #[test]
    fn a_markup_line_with_no_level_word_keeps_the_whole_of_its_text() {
        // "left" is not a level, so the bar is part of the message rather
        // than a separator that eats the first word.
        assert_eq!(split_line("left | right"), (LogLevel::Log, "left | right"));
        assert_eq!(split_line("nothing to report"), (LogLevel::Log, "nothing to report"));
    }

    #[test]
    fn a_message_may_carry_bars_of_its_own() {
        assert_eq!(split_line("log | a | b | c"), (LogLevel::Log, "a | b | c"));
    }

    #[test]
    fn every_gutter_word_reads_back_as_the_level_it_names() {
        for level in [LogLevel::Wait, LogLevel::Log, LogLevel::Warning, LogLevel::Error, LogLevel::Panic] {
            let markup = format!("{} | said so", tag_of(level));
            assert_eq!(split_line(&markup), (level, "said so"));
        }
    }

    #[test]
    fn the_levels_are_ordered_worst_last() {
        let ladder =
            [LogLevel::Wait, LogLevel::Log, LogLevel::Warning, LogLevel::Error, LogLevel::Panic];
        assert!(ladder.windows(2).all(|pair| severity(pair[0]) < severity(pair[1])));
        // A floor is named after the level it lets in, so the two ladders
        // have to agree or a floor would keep the level it is named for out.
        assert_eq!(LogListFloor::Notes.severity(), severity(LogLevel::Log));
        assert_eq!(LogListFloor::Warnings.severity(), severity(LogLevel::Warning));
        assert_eq!(LogListFloor::Errors.severity(), severity(LogLevel::Error));
    }
}
