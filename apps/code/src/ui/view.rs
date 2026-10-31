//! The code-chat, the dock's CODE-CHAT tab. It owns the agent port and the
//! transcript, drains the port when the agent wakes the UI, and draws the
//! transcript, the composer (with its reference chips and
//! the @ file picker), the status bar and the palette from `model`. The
//! layout is in `layout`. File paths in the text are links that open the
//! file in the workspace (`IdeAction::OpenFile`). Idle costs nothing: a
//! redraw happens on an event, and the one timer runs only while something
//! animates (an ease, a running turn).

use std::collections::{HashMap, VecDeque};
use super::charts::{HeatStrip, Sparkline};
use super::model::*;
use super::palette::{self, Cmd, Pane};
use super::paths::{self, PathSpan};
use makepad_code_agent::port::{self, AgentPort};
use super::hud::CodeHud;
use super::theme::Palette;
use makepad_code_agent::event::*;
use crate::ide::{IdeAction, Workspace};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Reference chips the composer holds (its pool of chip widgets).
const MAX_CHIPS: usize = 8;
/// A press and release this close together is a click, not a drag.
const CLICK_SLOP: f64 = 4.0;
const TODO_ROW: f64 = 20.0;
/// Glyphs of the bundled icon font (Font Awesome solid), so a state mark is
/// a real icon and not whatever the text fonts fall back to.
const ICON_CHECK: &str = "\u{f00c}";
const ICON_CROSS: &str = "\u{f00d}";
const ICON_RUN: &str = "\u{f1ce}";
const ICON_DOT: &str = "\u{f111}";
/// Lines of a running command's live tail.
const TAIL_LINES: usize = 2;
/// Raw bytes of a command's output looked at for the live tail.
const TAIL_BYTES: usize = 6000;
const MAX_TODO_ROWS: usize = 6;

/// A value that glides to its target (panes opening, the plan folding).
#[derive(Default, Clone, Copy)]
struct Ease {
    cur: f64,
    to: f64,
}

impl Ease {
    fn at(v: f64) -> Self {
        Ease { cur: v, to: v }
    }

    /// One frame of motion.
    fn step(&mut self) {
        let d = self.to - self.cur;
        if d.abs() < 0.6 {
            self.cur = self.to;
        } else {
            self.cur += d * 0.28;
        }
    }

    fn moving(&self) -> bool {
        self.cur != self.to
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AgentView {
    #[deref]
    view: View,
    /// The @ file picker, drawn on its own list over the transcript.
    #[live]
    at_popup: WidgetRef,
    #[rust]
    at_list: Option<DrawList2d>,
    #[rust]
    port: Option<Box<dyn AgentPort>>,
    /// The session `open_session` asked for, resumed once the agent runs.
    #[rust]
    pending_resume: Option<String>,
    #[rust]
    config: Option<AgentConfig>,
    #[rust]
    root: PathBuf,
    #[rust]
    t: Transcript,
    #[rust]
    history: Vec<String>,
    /// Which history line the composer shows, `None` for the live draft.
    #[rust]
    history_at: Option<usize>,
    #[rust]
    draft: String,
    /// The assistant entry whose text is fading in (until its fade is done).
    #[rust]
    fade: Fade,
    #[rust]
    focused: bool,
    /// A y/a/n key decided an approval: drop the letter the composer receives.
    #[rust]
    swallow: bool,
    /// The saved session the transcript came from.
    #[rust]
    active_session: Option<String>,
    #[rust]
    mode: Option<ApprovalMode>,
    /// Follow-ups typed while a turn runs (with their references); sent one
    /// by one when it is idle.
    #[rust]
    queued: VecDeque<(String, Vec<String>)>,
    #[rust]
    plan_folded: bool,
    /// The tab's title was sent for this conversation.
    #[rust]
    title_sent: bool,
    #[rust]
    plan_ease: Ease,
    /// Files attached to the next message (paths relative to the root).
    #[rust]
    chips: Vec<String>,
    /// The @ file picker, while open.
    #[rust]
    at: Option<AtPicker>,
    /// The @ at this byte offset was dismissed with Esc: typing after it
    /// does not reopen the picker.
    #[rust]
    at_dismissed: Option<usize>,
    /// Files are dragged over the chat: the composer shows it takes them.
    #[rust]
    drop_hover: bool,
    /// Where the primary button went down in the transcript (a release near
    /// it is a click on a link, not a selection drag).
    #[rust]
    press_at: Option<DVec2>,
    /// The press just released opened a link: the row's own click is not
    /// acted on too (a hunk head would flip its tick).
    #[rust]
    link_opened: bool,
    /// The link under the pointer.
    #[rust]
    hover: Option<LinkHit>,
    /// Undone edits waiting for the agent to put their file back.
    #[rust]
    undoing: Vec<Diff>,
    /// Whether a relative path is a file under the root, asked once each.
    #[rust]
    exists: HashMap<String, bool>,
    #[rust]
    tick: Timer,
    /// Seconds, for pulses; advanced by the tick.
    #[rust]
    clock: f64,
    #[rust]
    palette_items: Vec<palette::Item>,
    #[rust]
    palette_rank: Vec<usize>,
    #[rust]
    palette_sel: usize,
    #[rust]
    palette_first: usize,
    /// The palette is open (mirrors the modal, readable while drawing).
    #[rust]
    palette_open_flag: bool,
    /// The open palette is the context-size menu (it refreshes when the node reports).
    #[rust]
    palette_is_context: bool,
    /// The palette's field takes the keyboard once its modal has been drawn.
    #[rust]
    palette_focus: bool,
    #[rust]
    palette_query: String,
    #[rust]
    hud_moving: bool,
    /// Body row heights as drawn at rest, by (entry, body index), so a card
    /// folds through exactly the heights it showed.
    #[rust]
    body_h: BodyHeights,
    /// One diff line's height, as drawn.
    #[rust(15.0)]
    line_h: f64,
    /// Seconds at the previous tick, for the motion's time step.
    #[rust]
    last_tick: f64,
}

/// The @ file picker: the `@query` being typed and what it matches.
struct AtPicker {
    /// Byte offset of the `@` in the composer's text.
    start: usize,
    query: String,
    items: Vec<String>,
    sel: usize,
}

/// A link under the pointer: the transcript row, the text widget in it, the
/// span's index in that widget's links, and where it leads.
#[derive(Clone, Debug, PartialEq)]
struct LinkHit {
    row: usize,
    widget: LiveId,
    span: usize,
    /// The span's text (a Markdown link's url).
    href: String,
    target: PathSpan,
}

thread_local! {
    /// The chat tab drawn last: the one on screen (hidden tabs are not drawn).
    static SHOWN_CHAT: std::cell::Cell<Option<WidgetUid>> = const { std::cell::Cell::new(None) };
}

/// The paths in `text` that are files under `root` (`exists` caches the
/// answer per path, so a row redrawn every frame stats nothing).
fn path_spans(exists: &mut HashMap<String, bool>, root: &Path, text: &str) -> Vec<PathSpan> {
    if root.as_os_str().is_empty() {
        return Vec::new();
    }
    let root_s = format!("{}/", root.display());
    paths::find_paths(text, &root_s, &mut |rel| *exists.entry(rel.to_string()).or_insert_with(|| root.join(rel).is_file()))
}

fn linkify(exists: &mut HashMap<String, bool>, root: &Path, md: &str) -> String {
    if root.as_os_str().is_empty() {
        return md.to_string();
    }
    let root_s = format!("{}/", root.display());
    paths::linkify_markdown(md, &root_s, &mut |rel| *exists.entry(rel.to_string()).or_insert_with(|| root.join(rel).is_file()))
}

/// The `@query` the cursor is in: the byte offset of its `@` and the query.
/// An `@` starts one at the text's start or after whitespace; the query runs
/// to the cursor without whitespace.
pub fn at_token(text: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    if at > 0 && !before[..at].ends_with(char::is_whitespace) {
        return None;
    }
    Some((at, query.to_string()))
}

impl AgentView {
    /// This tab's conversation: a saved session to resume, or a new one.
    /// Called by the shell right after the tab is made, before the agent
    /// starts on the first draw.
    pub fn open_session(&mut self, cx: &mut Cx, session: Option<String>) {
        if self.port.is_none() {
            // Started on the first draw; resumed then.
            self.pending_resume = session;
            return;
        }
        match session {
            Some(id) if self.active_session.as_deref() == Some(id.as_str()) => {}
            Some(id) if self.t.busy => {
                // Stop the running turn first; resumed once it is idle.
                self.interrupt(cx);
                self.pending_resume = Some(id);
            }
            Some(id) => self.resume(cx, id),
            None if self.active_session.is_some() || !self.t.entries.is_empty() => {
                if self.t.busy {
                    self.interrupt(cx);
                }
                self.new_session(cx);
            }
            None => {}
        }
    }

    /// Open the folder: start the agent in it.
    pub fn start(&mut self, cx: &mut Cx, config: AgentConfig) {
        self.root = config.root.clone();
        self.t.root = config.root.clone();
        self.mode = Some(config.approval);
        self.config = Some(config.clone());
        self.plan_ease = Ease::at(0.0);
        let wake: port::Wake = Arc::new(|| SignalToUI::set_ui_signal());
        self.port = Some(port::start(config, wake));
        if let Some(id) = self.pending_resume.take() {
            self.resume(cx, id);
        }
        self.view.redraw(cx);
    }

    fn pal(&self) -> &'static Palette {
        super::theme::palette()
    }

    // ---- motion ---------------------------------------------------------

    fn animating(&self) -> bool {
        self.plan_ease.moving()
            || self.fade.entry.is_some()
            || self.t.entries.iter().any(|e| matches!(e, Entry::Tool(t) if t.unfolding() || t.fresh() || super::look::resolve_scan(t.started, t.took) < 1.0))
            || self.hud_moving
    }

    fn pulsing(&self) -> bool {
        self.t.busy || self.t.running_lanes().next().is_some() || self.t.entries.iter().any(|e| matches!(e, Entry::Tool(t) if t.live()))
    }

    /// Run the timer while there is something to move; never otherwise.
    fn kick(&mut self, cx: &mut Cx) {
        if !self.tick.is_empty() {
            return;
        }
        if self.animating() {
            self.tick = cx.start_timeout(0.016);
        } else if self.pulsing() {
            self.tick = cx.start_timeout(0.09);
        }
    }

    fn on_tick(&mut self, cx: &mut Cx) {
        self.tick = Timer::empty();
        let now = cx.seconds_since_app_start();
        let dt = if self.last_tick > 0.0 { (now - self.last_tick).clamp(0.001, 0.1) as f32 } else { 0.016 };
        self.last_tick = now;
        self.clock = now;
        let streaming = self.t.streaming_index().is_some();
        self.fade.frame(streaming);
        for entry in self.t.entries.iter_mut() {
            if let Entry::Tool(tool) = entry {
                tool.step_unfold(dt);
            }
        }
        self.plan_ease.step();
        self.view.redraw(cx);
        self.kick(cx);
    }

    /// 0..1 breathing, for the live dots and the composer glow.
    fn pulse(&self) -> f32 {
        (0.5 + 0.5 * (self.clock * 5.0).sin()) as f32
    }

    // ---- actions --------------------------------------------------------

    fn submit(&mut self, cx: &mut Cx, text: String) {
        let text = text.trim().to_string();
        let input = self.view.text_input(cx, ids!(input));
        input.set_text(cx, "");
        input.take_key_focus(cx);
        self.close_at(cx);
        self.at_dismissed = None;
        if text.is_empty() && self.chips.is_empty() {
            return;
        }
        let refs = std::mem::take(&mut self.chips);
        if self.t.busy && self.t.question.is_none() {
            // Typed while the agent works: keep it, send it when the turn ends.
            self.queued.push_back((text, refs));
            self.view.redraw(cx);
            return;
        }
        self.send_now(cx, text, refs);
    }

    fn send_now(&mut self, cx: &mut Cx, text: String, refs: Vec<String>) {
        if !text.is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        self.history_at = None;
        self.draft.clear();
        if !self.title_sent && !text.is_empty() {
            self.title_sent = true;
            let first = text.lines().next().unwrap_or("");
            self.emit(cx, IdeAction::ChatTitle(clip(first, 24)));
        }
        let answering = self.t.question.take();
        self.t.submit(text.clone(), refs.clone());
        if let Some(port) = self.port.as_mut() {
            match answering {
                Some(id) => port.send(Command::Answer { id, text }),
                None => port.send(Command::Submit { text, refs }),
            }
        }
        self.view.redraw(cx);
        self.kick(cx);
    }

    // ---- references -------------------------------------------------------

    /// Attach a file (relative to the root) to the next message.
    pub fn add_reference(&mut self, cx: &mut Cx, rel_path: &str) {
        let rel = rel_path.trim_start_matches("./").trim_end_matches('/').to_string();
        if rel.is_empty() || self.chips.contains(&rel) {
            return;
        }
        if self.chips.len() >= MAX_CHIPS {
            self.t.entries.push(Entry::Notice(format!("at most {MAX_CHIPS} references per message: {rel} was not added")));
        } else {
            log!("code: attached {rel}");
            self.chips.push(rel);
        }
        self.view.redraw(cx);
    }

    fn remove_reference(&mut self, cx: &mut Cx, index: usize) {
        if index < self.chips.len() {
            self.chips.remove(index);
            self.view.redraw(cx);
        }
    }

    /// Files dropped on the chat: each one inside the root becomes a chip.
    fn drop_files(&mut self, cx: &mut Cx, items: &[DragItem]) {
        let root = Workspace::of(cx).map(|w| w.root.clone()).unwrap_or_else(|| self.root.clone());
        for item in items {
            let DragItem::FilePath { path, internal_id: None } = item else { continue };
            let abs = Path::new(path);
            match abs.strip_prefix(&root) {
                Ok(rel) if !rel.as_os_str().is_empty() => {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    self.add_reference(cx, &rel);
                }
                // A picture may come from anywhere (a screenshot): the model looks at it.
                _ if makepad_code_agent::pictures::is_picture_ref(path) => self.add_reference(cx, path),
                _ => self.t.entries.push(Entry::Notice(format!("{path} is outside the workspace: not attached"))),
            }
        }
        self.view.text_input(cx, ids!(input)).take_key_focus(cx);
        self.view.redraw(cx);
    }

    // ---- the @ picker -------------------------------------------------------

    /// The composer's text changed: open, refresh or close the picker.
    fn update_at(&mut self, cx: &mut Cx, text: &str) {
        let cursor = self.view.text_input(cx, ids!(input)).cursor().index;
        match at_token(text, cursor) {
            Some((start, _)) if self.at_dismissed == Some(start) => self.close_at(cx),
            Some((start, query)) => {
                self.at_dismissed = None;
                let items = self.rank_files(cx, &query);
                let sel = match &self.at {
                    Some(at) if at.query == query => at.sel.min(items.len().saturating_sub(1)),
                    _ => 0,
                };
                self.at = Some(AtPicker { start, query, items, sel });
                self.view.redraw(cx);
            }
            None => self.close_at(cx),
        }
    }

    /// The workspace's files best matching `query`, at most 8.
    fn rank_files(&self, cx: &Cx, query: &str) -> Vec<String> {
        let Some(ws) = Workspace::of(cx) else { return Vec::new() };
        let found: Vec<String> = ws.index.search(query, 8).into_iter().map(|m| m.path).collect();
        if !found.is_empty() {
            return found;
        }
        let files = ws.index.files();
        let mut hits: Vec<(i32, &String)> = files.iter().filter_map(|f| palette::score(query, f).map(|s| (s, f))).collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
        hits.into_iter().take(8).map(|(_, f)| f.clone()).collect()
    }

    fn close_at(&mut self, cx: &mut Cx) {
        if self.at.take().is_some() {
            self.view.redraw(cx);
        }
    }

    /// The picked file replaces its `@query` with a chip.
    fn pick_at(&mut self, cx: &mut Cx, index: usize) {
        let Some(at) = self.at.take() else { return };
        let Some(path) = at.items.get(index).cloned() else { return };
        let input = self.view.text_input(cx, ids!(input));
        let text = input.text();
        let end = (at.start + 1 + at.query.len()).min(text.len());
        let mut next = String::with_capacity(text.len());
        next.push_str(&text[..at.start]);
        next.push_str(text[end..].trim_start_matches(' '));
        input.set_text(cx, &next);
        input.set_cursor(cx, makepad_widgets::makepad_draw::text::selection::Cursor { index: at.start, prefer_next_row: false }, false);
        input.take_key_focus(cx);
        self.add_reference(cx, &path);
    }

    /// Keys for the picker and the chips while the composer has the keyboard;
    /// true when the key was theirs (the field does not see it).
    fn composer_keys(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        if ke.modifiers.is_primary() || ke.modifiers.alt || !self.composer_focused(cx) {
            return false;
        }
        if let Some(at) = &mut self.at {
            let last = at.items.len().saturating_sub(1);
            match ke.key_code {
                KeyCode::ArrowDown => at.sel = (at.sel + 1).min(last),
                KeyCode::ArrowUp => at.sel = at.sel.saturating_sub(1),
                KeyCode::ReturnKey | KeyCode::Tab if !at.items.is_empty() => {
                    let sel = at.sel;
                    self.pick_at(cx, sel);
                    return true;
                }
                KeyCode::Escape => {
                    self.at_dismissed = Some(at.start);
                    self.close_at(cx);
                    return true;
                }
                _ => return false,
            }
            self.view.redraw(cx);
            return true;
        }
        if ke.key_code == KeyCode::Backspace && !self.chips.is_empty() {
            let input = self.view.text_input(cx, ids!(input));
            let sel = input.selection();
            if sel.cursor.index == 0 && sel.anchor.index == 0 {
                self.chips.pop();
                self.view.redraw(cx);
                return true;
            }
        }
        false
    }

    // ---- focus ----------------------------------------------------------------

    fn composer_focused(&self, cx: &mut Cx) -> bool {
        let area = self.view.text_input(cx, ids!(input)).area();
        cx.has_key_focus(area)
    }

    /// Whether keys are the chat's: the composer or the palette has the
    /// keyboard, or the last press was inside the chat and no field
    /// elsewhere took the keyboard since.
    fn emit(&self, cx: &mut Cx, action: IdeAction) {
        cx.widget_action(self.widget_uid(), action);
    }

    fn chat_focused(&self, cx: &mut Cx) -> bool {
        // Several chat tabs exist; only the one last drawn is on screen.
        if SHOWN_CHAT.with(|c| c.get()) != Some(self.widget_uid()) {
            return false;
        }
        if self.composer_focused(cx) || (self.palette_open_flag && cx.has_key_focus(self.view.text_input(cx, ids!(palette_input)).area())) {
            return true;
        }
        // A press in the chat (not in a field) leaves the keyboard with the
        // chat's own area or its transcript.
        cx.has_key_focus(self.view.area()) || cx.has_key_focus(self.view.portal_list(cx, ids!(transcript)).area())
    }

    // ---- links ----------------------------------------------------------------

    /// The text widgets of a transcript row whose paths are links, and
    /// whether each is the Markdown answer.
    fn link_widgets(line: Line, entry: Option<&Entry>) -> &'static [(&'static [LiveId; 1], bool)] {
        match (line, entry) {
            (Line::Entry(_), Some(Entry::User { .. })) => &[(ids!(user_text), false)],
            (Line::Entry(_), Some(Entry::Assistant { .. })) => &[(ids!(assistant_md), true)],
            (Line::Entry(_), Some(Entry::Tool(_))) => &[(ids!(tool_title), false), (ids!(tool_summary), false)],
            (Line::Entry(_), Some(Entry::Notice(_))) => &[(ids!(notice_text), false)],
            (Line::Entry(_), Some(Entry::Error(_))) => &[(ids!(error_text), false)],
            (Line::Output(_), _) => &[(ids!(output_text), false)],
            (Line::Tail(_), _) => &[(ids!(tail_text), false)],
            (Line::Hunk(..), _) => &[(ids!(hunk_text), false)],
            _ => &[],
        }
    }

    fn entry_of(&self, line: Line) -> Option<&Entry> {
        let e = match line {
            Line::Entry(e) | Line::Approval(e) | Line::Hunk(e, _) | Line::Diff(e, _, _) | Line::Output(e) | Line::Tail(e) => e,
        };
        self.t.entries.get(e)
    }

    /// The links in a row's label text: the paths in it; a hunk head links
    /// its file name and its `@@` to the hunk's first new line.
    fn spans_for(&mut self, line: Line, text: &str) -> Vec<PathSpan> {
        if let Line::Hunk(e, h) = line {
            let Some(Entry::Tool(tool)) = self.t.entries.get(e) else { return Vec::new() };
            let Some(diff) = tool.shown_diff() else { return Vec::new() };
            let Some(hunk) = diff.hunks.get(h) else { return Vec::new() };
            let rel = self.relative(&diff.path);
            let line = Some(hunk.new_start.max(1));
            let mut out = Vec::new();
            if h == 0 && text.starts_with(diff.path.as_str()) {
                out.push(PathSpan { start: 0, end: diff.path.len(), path: rel.clone(), line, col: None });
            }
            if let Some(at) = text.find("@@") {
                let plus = text[at..].find('+').map(|p| at + p + 1).unwrap_or(at + 2);
                let end = plus + text[plus..].chars().take_while(|c| c.is_ascii_digit()).count();
                out.push(PathSpan { start: at, end, path: rel, line, col: None });
            }
            return out;
        }
        path_spans(&mut self.exists, &self.root, text)
    }

    /// Give a drawn row's labels their links (and the hovered one its line).
    fn draw_links(&mut self, cx: &mut Cx, row: &WidgetRef, index: usize, line: Line) {
        let widgets = Self::link_widgets(line, self.entry_of(line));
        for (id, md) in widgets {
            if *md {
                let href = self.hover.as_ref().filter(|h| h.row == index && h.widget == id[0]).map(|h| h.href.clone());
                row.markdown(cx, *id).set_hovered_link(cx, href.as_deref());
                continue;
            }
            let label = row.label(cx, *id);
            let text = label.text();
            let spans: Vec<(usize, usize)> = self.spans_for(line, &text).iter().map(|s| (s.start, s.end)).collect();
            let hover = self.hover.as_ref().filter(|h| h.row == index && h.widget == id[0]).map(|h| h.span);
            label.set_links(cx, &spans, hover);
        }
    }

    /// The link under `abs` in the transcript, if any.
    fn link_at(&mut self, cx: &mut Cx, abs: DVec2) -> Option<LinkHit> {
        let list = self.view.portal_list(cx, ids!(transcript));
        if !list.area().clipped_rect(cx).contains(abs) {
            return None;
        }
        let row = list.item_at(cx, abs)?;
        let line = *self.rows().get(row)?;
        let (_, item) = list.get_item(row)?;
        let widgets = Self::link_widgets(line, self.entry_of(line));
        for (id, md) in widgets {
            let w = item.widget(cx, *id);
            let Some(at) = w.selection_point_to_char_index(cx, abs) else { continue };
            let text = if *md { w.selection_get_full_text() } else { w.text() };
            let spans = if *md { path_spans(&mut self.exists, &self.root, &text) } else { self.spans_for(line, &text) };
            let Some(span) = spans.iter().position(|s| at >= s.start && at < s.end) else { continue };
            let target = spans[span].clone();
            let href = text.get(target.start..target.end).unwrap_or("").to_string();
            return Some(LinkHit { row, widget: id[0], span, href, target });
        }
        None
    }

    fn open_link(&mut self, cx: &mut Cx, target: &PathSpan) {
        log!("code: open {}{}", target.path, target.line.map(|l| format!(":{l}")).unwrap_or_default());
        cx.widget_action(
            self.widget_uid(),
            IdeAction::OpenFile { path: target.path.clone(), line: target.line, col: target.col },
        );
    }

    /// Presses, moves and releases over the transcript: hover a link, open
    /// it on a click (a drag selects as before).
    fn pointer(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::MouseDown(e) => {
                if self.view.area().rect(cx).contains(e.abs) && !self.composer_focused(cx) {
                    cx.set_key_focus(self.view.area());
                }
                self.link_opened = false;
                let list = self.view.portal_list(cx, ids!(transcript));
                self.press_at = (e.button.contains(MouseButton::PRIMARY) && list.area().clipped_rect(cx).contains(e.abs)).then_some(e.abs);
            }
            Event::MouseMove(e) => {
                let hit = self.link_at(cx, e.abs);
                if hit != self.hover {
                    self.hover = hit;
                    self.view.redraw(cx);
                }
            }
            Event::MouseLeave(_) => {
                if self.hover.take().is_some() {
                    self.view.redraw(cx);
                }
            }
            Event::MouseUp(e) => {
                let Some(down) = self.press_at.take() else { return };
                if (e.abs - down).length() > CLICK_SLOP || e.modifiers.shift {
                    return;
                }
                if let Some(hit) = self.link_at(cx, e.abs) {
                    self.link_opened = true;
                    self.open_link(cx, &hit.target);
                }
            }
            _ => {}
        }
    }

    /// Files dragged over or dropped on the chat (tree rows, the desktop).
    fn drag_drop(&mut self, cx: &mut Cx, event: &Event) {
        let files = |items: &[DragItem]| items.iter().any(|i| matches!(i, DragItem::FilePath { internal_id: None, .. }));
        let ours = match event {
            Event::Drag(e) => files(&e.items),
            Event::Drop(e) => files(&e.items),
            Event::DragEnd => true,
            _ => false,
        };
        if !ours {
            return;
        }
        match event.drag_hits(cx, self.view.area()) {
            DragHit::Drag(d) => {
                let over = !matches!(d.state, DragState::Out);
                if over {
                    *d.response.lock().unwrap() = DragResponse::Copy;
                }
                if over != self.drop_hover {
                    self.drop_hover = over;
                    self.view.redraw(cx);
                }
            }
            DragHit::Drop(d) => {
                self.drop_hover = false;
                self.drop_files(cx, &d.items);
            }
            DragHit::DragEnd | DragHit::NoHit => {
                if self.drop_hover {
                    self.drop_hover = false;
                    self.view.redraw(cx);
                }
            }
        }
    }

    /// An edit finished (or was undone): the workspace reloads the file if
    /// it is open and lights the lines.
    fn emit_edited(&mut self, cx: &mut Cx, diff: &Diff, new_side: bool) {
        let path = self.relative(&diff.path);
        let lines = paths::changed_ranges(diff, new_side);
        log!("code: agent edited {path} {lines:?}");
        cx.widget_action(self.widget_uid(), IdeAction::AgentEdited { path, lines });
    }

    fn decide(&mut self, cx: &mut Cx, id: ToolId, decision: Decision) {
        self.t.clear_approval(id);
        if let Some(port) = self.port.as_mut() {
            port.send(Command::Approve { id, decision });
        }
        self.view.redraw(cx);
    }

    fn drain(&mut self, cx: &mut Cx) {
        let Some(port) = self.port.as_mut() else { return };
        let events = port.poll();
        if events.is_empty() {
            return;
        }
        let mut restored = false;
        let mut reported = false;
        for event in events {
            if let AgentEvent::ToolFinished { id, ok: true, diff: Some(diff), .. } = &event {
                let edit = self.t.entries.iter().rev().any(|e| matches!(e, Entry::Tool(t) if t.id == *id && t.is_edit()));
                if edit {
                    let diff = diff.clone();
                    self.emit_edited(cx, &diff, true);
                }
            }
            if let AgentEvent::FileTouched { path, how: Touch::Edited } = &event {
                let rel = self.relative(path);
                if let Some(i) = self.undoing.iter().position(|d| self.relative(&d.path) == rel) {
                    let diff = self.undoing.remove(i);
                    self.emit_edited(cx, &diff, false);
                }
            }
            reported |= matches!(event, AgentEvent::ContextReport(_));
            restored |= matches!(event, AgentEvent::Restored(_));
            // The agent saved (or listed) its sessions: the sidebar re-reads.
            // The tab now shows a saved session (a new chat's first save).
            if let AgentEvent::SessionSaved(id) = &event {
                self.active_session = Some(id.clone());
                self.emit(cx, IdeAction::ChatSession(id.clone()));
            }
            if matches!(event, AgentEvent::Sessions(_)) {
                self.emit(cx, IdeAction::SessionsChanged);
            }
            if let AgentEvent::FileTouched { path, how } = event {
                let path = self.relative(&path);
                self.t.apply(AgentEvent::FileTouched { path, how });
            } else {
                self.t.apply(event);
            }
        }
        if restored {
            self.fade.clear();
            if !self.title_sent {
                self.title_sent = true;
                let id = self.active_session.clone().unwrap_or_default();
                let title = self.t.sessions.iter().find(|s| s.id == id).map(|s| s.title.clone()).or_else(|| {
                    self.t.entries.iter().find_map(|e| match e {
                        Entry::User { text, .. } => Some(text.lines().next().unwrap_or("").to_string()),
                        _ => None,
                    })
                });
                if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
                    self.emit(cx, IdeAction::ChatTitle(clip(&title, 24)));
                }
            }
        }
        if reported && self.palette_is_context && self.palette_open_flag {
            self.palette_items = palette::context_items(self.t.context.as_ref(), self.t.usage.context_max);
            self.palette_rank = palette::rank(&self.palette_items, &self.palette_query);
        }
        self.t.follow = None;
        self.plan_target();
        if !self.t.busy {
            if let Some(id) = self.pending_resume.take() {
                self.resume(cx, id);
            } else if let Some((next, refs)) = self.queued.pop_front() {
                self.send_now(cx, next, refs);
            }
        }
        self.view.redraw(cx);
        self.kick(cx);
    }

    /// A path the agent reports, relative to the workspace.
    fn relative(&self, path: &str) -> String {
        match std::path::Path::new(path).strip_prefix(&self.root) {
            Ok(rel) => rel.to_string_lossy().to_string(),
            Err(_) => path.to_string(),
        }
    }

    fn resume(&mut self, cx: &mut Cx, id: String) {
        if self.t.busy {
            self.pending_resume = Some(id);
            return;
        }
        self.title_sent = false;
        self.queued.clear();
        self.t.clear();
        self.fade.clear();
        self.active_session = Some(id.clone());
        if let Some(port) = self.port.as_mut() {
            port.send(Command::Resume(id));
        }
        self.view.redraw(cx);
    }

    fn new_session(&mut self, cx: &mut Cx) {
        self.title_sent = false;
        self.emit(cx, IdeAction::ChatTitle("CHAT".into()));
        self.queued.clear();
        self.active_session = None;
        self.t.clear();
        self.fade.clear();
        if let Some(port) = self.port.as_mut() {
            port.send(Command::NewSession);
        }
        self.view.redraw(cx);
    }

    fn set_mode(&mut self, cx: &mut Cx, mode: ApprovalMode) {
        self.mode = Some(mode);
        if let Some(port) = self.port.as_mut() {
            port.send(Command::SetApprovalMode(mode));
        }
        self.view.redraw(cx);
    }

    /// Open the context menu with the node's sizes and KV precision choices.
    /// The node is asked again so the numbers and current precision are fresh.
    fn open_context_menu(&mut self, cx: &mut Cx) {
        if let Some(port) = self.port.as_mut() {
            port.send(Command::QueryContext);
        }
        self.palette_items = palette::context_items(self.t.context.as_ref(), self.t.usage.context_max);
        self.palette_is_context = true;
        self.open_palette_with_items(cx);
    }

    fn toggle_pane(&mut self, cx: &mut Cx, pane: Pane) {
        match pane {
            Pane::Plan => {
                self.plan_folded = !self.plan_folded;
                self.plan_target();
            }
        }
        self.view.redraw(cx);
        self.kick(cx);
    }

    fn plan_target(&mut self) {
        let rows = self.t.todos.len().min(MAX_TODO_ROWS) as f64;
        self.plan_ease.to = if self.plan_folded { 0.0 } else { rows * TODO_ROW };
        if self.t.todos.is_empty() {
            self.plan_ease.cur = 0.0;
        }
    }

    fn copy_last_answer(&mut self, cx: &mut Cx) {
        let last = self.t.entries.iter().rev().find_map(|e| match e {
            Entry::Assistant { text, lane: 0, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        });
        if let Some(text) = last {
            cx.copy_to_clipboard(&text);
        }
    }

    fn interrupt(&mut self, cx: &mut Cx) {
        if let Some(id) = self.t.pending_approval() {
            self.decide(cx, id, Decision::Deny);
        }
        if let Some(port) = self.port.as_mut() {
            port.send(Command::Interrupt);
        }
    }

    fn run(&mut self, cx: &mut Cx, cmd: Cmd) {
        match cmd {
            // Each chat tab is one conversation: these open a tab.
            Cmd::NewSession => self.emit(cx, IdeAction::OpenChat { session: None }),
            Cmd::Resume(id) => self.emit(cx, IdeAction::OpenChat { session: Some(id) }),
            Cmd::Approval(mode) => self.set_mode(cx, mode),
            Cmd::ContextMenu => self.open_context_menu(cx),
            Cmd::SetContext(n) => {
                if let Some(port) = self.port.as_mut() {
                    port.send(Command::SetContext(n));
                }
            }
            Cmd::SetKv(kv) => {
                if let Some(port) = self.port.as_mut() {
                    port.send(Command::SetKv(kv));
                }
            }
            Cmd::Toggle(pane) => self.toggle_pane(cx, pane),
            Cmd::Interrupt => self.interrupt(cx),
            Cmd::CopyLastAnswer => self.copy_last_answer(cx),
        }
        self.view.text_input(cx, ids!(input)).take_key_focus(cx);
    }

    // ---- palette --------------------------------------------------------

    fn open_palette(&mut self, cx: &mut Cx) {
        self.palette_items = palette::items(&self.t.sessions);
        self.palette_is_context = false;
        self.open_palette_with_items(cx);
    }

    fn open_palette_with_items(&mut self, cx: &mut Cx) {
        self.palette_query.clear();
        self.palette_sel = 0;
        self.palette_first = 0;
        self.palette_rank = palette::rank(&self.palette_items, "");
        self.palette_open_flag = true;
        self.palette_focus = true;
        self.view.modal(cx, ids!(palette)).open(cx);
        self.view.text_input(cx, ids!(palette_input)).set_text(cx, "");
        self.view.redraw(cx);
    }

    fn close_palette(&mut self, cx: &mut Cx) {
        self.palette_open_flag = false;
        self.view.modal(cx, ids!(palette)).close(cx);
        self.view.text_input(cx, ids!(input)).take_key_focus(cx);
        self.view.redraw(cx);
    }

    fn palette_open(&self, cx: &mut Cx) -> bool {
        self.view.modal(cx, ids!(palette)).is_open()
    }

    fn run_palette_pick(&mut self, cx: &mut Cx, rank_index: usize) {
        let Some(&item) = self.palette_rank.get(rank_index) else { return };
        let cmd = self.palette_items[item].cmd.clone();
        self.close_palette(cx);
        self.run(cx, cmd);
    }

    // ---- composer helpers -------------------------------------------------

    /// Up/Down at the edge of the composer walk the sent lines.
    fn recall(&mut self, cx: &mut Cx, up: bool) {
        if self.history.is_empty() {
            return;
        }
        let input = self.view.text_input(cx, ids!(input));
        let next = match (self.history_at, up) {
            (None, true) => {
                self.draft = input.text();
                Some(self.history.len() - 1)
            }
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.history.len() => Some(i + 1),
            (Some(_), false) => None,
            (None, false) => return,
        };
        self.history_at = next;
        let text = match next {
            Some(i) => self.history[i].clone(),
            None => std::mem::take(&mut self.draft),
        };
        input.set_text(cx, &text);
        input.set_cursor(cx, makepad_widgets::makepad_draw::text::selection::Cursor { index: text.len(), prefer_next_row: false }, false);
        self.view.redraw(cx);
    }

    // ---- drawing: chrome ------------------------------------------------

    fn draw_chrome(&mut self, cx: &mut Cx2d) {
        let folder = self.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

        // The plan: folds smoothly, hidden when there is none.
        self.view.view(cx, ids!(plan)).set_visible(cx, !self.t.todos.is_empty());
        let h = self.plan_ease.cur;
        let mut todos = self.view.portal_list(cx, ids!(todos));
        script_apply_eval!(cx, todos, { height: #(h) });
        let done = self.t.todos.iter().filter(|t| t.state == TodoState::Done).count();
        let arrow = if self.plan_folded { "▶" } else { "▼" };
        self.view
            .label(cx, ids!(plan_title))
            .set_text(cx, &format!("{arrow}  PLAN   {done} of {} done", self.t.todos.len()));

        // The composer's frame breathes the agent's colour while a turn runs
        // and yours while the agent waits on you.
        let mut composer = self.view.view(cx, ids!(composer));
        let glow = if self.t.pending_approval().is_some() || self.t.question.is_some() {
            c(self.pal().human, 0.45 + 0.55 * self.pulse())
        } else if self.t.busy {
            c(self.pal().info, 0.35 + 0.65 * self.pulse())
        } else {
            c(self.pal().outline_hi, 1.0)
        };
        let (glow, drop) = if self.drop_hover { (c(self.pal().primary, 1.0), 1.0) } else { (glow, 0.0) };
        script_apply_eval!(cx, composer, { draw_bg +: { border_color: #(glow) drop: #(drop) } });
        self.draw_chips(cx);
        self.draw_at_rows(cx);

        // Before the first message the stage shows the mark and the keys.
        let empty = self.t.entries.is_empty();
        self.view.view(cx, ids!(empty_state)).set_visible(cx, empty);
        if empty {
            let route = self.t.status.as_ref().map(|s| s.route.clone()).unwrap_or_else(|| "starting the model".into());
            self.view.label(cx, ids!(empty_where)).set_text(cx, &format!("{folder}  ·  {route}"));
        }
        if let Some(mut hud) = self.view.widget(cx, ids!(hud)).borrow_mut::<CodeHud>() {
            hud.set(&self.t, self.clock);
            self.hud_moving = hud.animating();
        }

        if self.palette_open_flag {
            self.draw_palette_rows(cx);
        }
        self.draw_status(cx);
    }

    /// The composer's chips: one per attached file, from a fixed pool.
    fn draw_chips(&mut self, cx: &mut Cx2d) {
        self.view.view(cx, ids!(chips)).set_visible(cx, !self.chips.is_empty());
        for (slot, path) in chip_slots().iter().enumerate() {
            let chip = self.view.view(cx, *path);
            match self.chips.get(slot) {
                Some(rel) => {
                    chip.set_visible(cx, true);
                    let name = rel.rsplit('/').next().unwrap_or(rel);
                    let dir = self.root.join(rel).is_dir();
                    chip.label(cx, ids!(chip_icon)).set_text(cx, if dir { ICON_FOLDER } else { ICON_FILE });
                    chip.label(cx, ids!(chip_name)).set_text(cx, name);
                }
                None => chip.set_visible(cx, false),
            }
        }
    }

    /// The @ picker's rows: file name bright, its folder dim.
    fn draw_at_rows(&mut self, cx: &mut Cx2d) {
        if !self.at_open() {
            return;
        }
        let at = self.at.as_ref().unwrap();
        for (slot, path) in at_rows().iter().enumerate() {
            let mut row = self.at_popup.view(cx, *path);
            let Some(file) = at.items.get(slot) else {
                row.set_visible(cx, false);
                continue;
            };
            row.set_visible(cx, true);
            let tint = if slot == at.sel { c(self.pal().primary, 0.22) } else { CLEAR };
            script_apply_eval!(cx, row, { draw_bg +: { color: #(tint) } });
            let (dir, name) = match file.rfind('/') {
                Some(i) => (&file[..i], &file[i + 1..]),
                None => ("", file.as_str()),
            };
            row.label(cx, ids!(palette_title)).set_text(cx, name);
            row.label(cx, ids!(palette_hint)).set_text(cx, dir);
        }
    }

    fn at_open(&self) -> bool {
        self.at.as_ref().is_some_and(|a| !a.items.is_empty())
    }

    /// The picker over the transcript: its bottom just above the composer,
    /// its left edge at the caret where it fits.
    fn draw_at_popup(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        if !self.at_open() {
            return;
        }
        let composer = self.view.view(cx, ids!(composer)).area().rect(cx);
        if composer.size.x <= 0.0 {
            return;
        }
        let rows = self.at.as_ref().map_or(0, |a| a.items.len().min(8)) as f64;
        let height = rows * 24.0 + 8.0;
        let width = 420.0f64.min(composer.size.x);
        let caret_x = self
            .view
            .text_input(cx, ids!(input))
            .cursor_rect_in_absolute(cx)
            .map_or(composer.pos.x, |r| r.pos.x - 16.0);
        let x = caret_x.clamp(composer.pos.x, composer.pos.x + composer.size.x - width);
        let y = composer.pos.y - height - 4.0;
        let mut list = self.at_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        list.begin_always(cx);
        let mut walk = self.at_popup.walk(cx);
        walk.width = Size::Fixed(width);
        walk.abs_pos = Some(dvec2(x, y));
        cx.widget_tree_insert_child(self.widget_uid(), live_id!(at_popup), self.at_popup.clone());
        let _ = self.at_popup.draw_walk(cx, scope, walk);
        list.end(cx);
        self.at_list = Some(list);
    }

    fn draw_status(&mut self, cx: &mut Cx2d) {
        let ready = self.t.status.as_ref().map(|s| s.ready).unwrap_or(false);
        let route = self.t.status.as_ref().map(|s| s.route.clone()).unwrap_or_else(|| "starting".into());
        let mut dot = self.view.label(cx, ids!(status_dot));
        let color = if !ready {
            c(self.pal().disabled, 1.0)
        } else if self.t.busy {
            c(self.pal().info, 0.4 + 0.6 * self.pulse())
        } else {
            c(self.pal().ok, 1.0)
        };
        script_apply_eval!(cx, dot, { draw_text +: { color: #(color) } });
        self.view.label(cx, ids!(status_route)).set_text(cx, &route);
        let state = match (self.t.busy, self.queued.len()) {
            (true, 0) => "working  ·  Esc to interrupt".to_string(),
            (true, n) => format!("working  ·  {n} queued  ·  Esc to interrupt"),
            (false, _) => "ready".to_string(),
        };
        let state = state.as_str();
        self.view.label(cx, ids!(status_state)).set_text(cx, state);
        let mode = match self.mode {
            Some(ApprovalMode::Ask) => "approvals: ask",
            Some(ApprovalMode::Auto) => "approvals: auto",
            _ => "approvals: allowlist",
        };
        self.view.label(cx, ids!(status_mode)).set_text(cx, mode);
        let usage = &self.t.usage;
        let rate = if self.t.busy && usage.tokens_per_sec > 0.5 { format!("{:.0} tok/s", usage.tokens_per_sec) } else { String::new() };
        self.view.label(cx, ids!(status_rate)).set_text(cx, &rate);
        let ctx = if usage.context_max > 0 {
            format!("{} / ctx {}", short_tokens(usage.context_used), short_tokens(usage.context_max))
        } else {
            String::new()
        };
        self.view.label(cx, ids!(status_context)).set_text(cx, &ctx);
        if let Some(mut heat) = self.view.widget(cx, ids!(heat)).borrow_mut::<HeatStrip>() {
            heat.set(&usage.segments, usage.context_max);
        }
        if let Some(mut spark) = self.view.widget(cx, ids!(rate_spark)).borrow_mut::<Sparkline>() {
            spark.set(&self.t.rate);
        }
    }
}

const ICON_FILE: &str = "\u{f15b}";
const ICON_FOLDER: &str = "\u{f07b}";

fn chip_slots() -> [&'static [LiveId; 1]; MAX_CHIPS] {
    [ids!(chip_0), ids!(chip_1), ids!(chip_2), ids!(chip_3), ids!(chip_4), ids!(chip_5), ids!(chip_6), ids!(chip_7)]
}

fn at_rows() -> [&'static [LiveId; 1]; 8] {
    [ids!(at_row_0), ids!(at_row_1), ids!(at_row_2), ids!(at_row_3), ids!(at_row_4), ids!(at_row_5), ids!(at_row_6), ids!(at_row_7)]
}

fn palette_rows() -> [&'static [LiveId; 1]; 9] {
    [
        ids!(pal_row_0),
        ids!(pal_row_1),
        ids!(pal_row_2),
        ids!(pal_row_3),
        ids!(pal_row_4),
        ids!(pal_row_5),
        ids!(pal_row_6),
        ids!(pal_row_7),
        ids!(pal_row_8),
    ]
}

/// A palette colour with an alpha.
fn c(rgb: u32, alpha: f32) -> Vec4f {
    super::theme::vec(rgb, alpha)
}

const CLEAR: Vec4f = Vec4f { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}


impl Widget for AgentView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Right-click copies the thing under the pointer, wherever it is:
        // a transcript row (or the selection), a file, a session, a palette
        // entry, a HUD value, the status line, the editor's path.
        if let Event::MouseDown(e) = event {
            if e.button.contains(MouseButton::SECONDARY) {
                if let Some((region, text)) = self.text_under(cx, e.abs) {
                    self.copy_out(cx, region, &text);
                }
            }
        }
        // The floating @ picker sees pointer events first (it lies over the
        // transcript).
        if self.at_open() {
            let actions = cx.capture_actions(|cx| self.at_popup.handle_event(cx, event, scope));
            for (slot, path) in at_rows().iter().enumerate() {
                if self.at_popup.view(cx, *path).finger_up(&actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
                    self.pick_at(cx, slot);
                    break;
                }
            }
        }
        self.pointer(cx, event);
        self.drag_drop(cx, event);
        // Copy from the transcript: rows scrolled out of view (Cmd+A takes
        // them all) come from the transcript itself, as plain text.
        // A test run reads what a copy would take from the log (no
        // clipboard): after a selection drag and on Cmd/Ctrl+C.
        if std::env::var_os("CODE_LOG_COPY").is_some() {
            let copy_key = matches!(event, Event::KeyDown(ke) if ke.key_code == KeyCode::KeyC && (ke.modifiers.control || ke.modifiers.logo));
            if copy_key || matches!(event, Event::MouseUp(_)) {
                let list = self.view.portal_list(cx, ids!(transcript));
                if list.has_selection() {
                    let rows = self.rows();
                    let text = list.selected_text_with(&|id| rows.get(id).map(|line| self.row_text(*line)));
                    log!("code: copy {:?}", text);
                }
            }
        }
        if let Event::TextCopy(tc) = event {
            let list = self.view.portal_list(cx, ids!(transcript));
            if list.has_selection() && tc.response.borrow().is_none() {
                let rows = self.rows();
                let text = list.selected_text_with(&|id| rows.get(id).map(|line| self.row_text(*line)));
                if !text.is_empty() {
                    *tc.response.borrow_mut() = Some(text);
                }
            }
        }
        if let Event::Signal = event {
            self.drain(cx);
        }
        if self.tick.is_event(event).is_some() {
            self.on_tick(cx);
        }
        // The chat's keys act only while the chat has the keyboard (not
        // while typing in an editor or a terminal).
        if let Event::KeyDown(ke) = event {
            if self.composer_keys(cx, ke) {
                return;
            }
        }
        let chat_key = match event {
            Event::KeyDown(ke) if self.chat_focused(cx) => Some(ke),
            _ => None,
        };
        if let Some(ke) = chat_key {
            let palette_open = self.palette_open(cx);
            if ke.modifiers.is_primary() && !ke.modifiers.shift && !ke.modifiers.alt {
                match ke.key_code {
                    KeyCode::KeyK => {
                        if palette_open {
                            self.close_palette(cx);
                        } else {
                            self.open_palette(cx);
                        }
                    }
                    KeyCode::KeyN => self.emit(cx, IdeAction::OpenChat { session: None }),
                    _ => {}
                }
            }
            if !palette_open {
                if ke.key_code == KeyCode::Escape && self.t.busy {
                    self.interrupt(cx);
                }
                self.approval_keys(cx, ke);
            }
        }
        // The chat's own widgets' actions are taken here (the dock's shell
        // captures everything its tabs emit, so they never come back as an
        // Actions event).
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !actions.is_empty() {
            self.handle_actions(cx, &actions, scope);
        }
        if matches!(event, Event::MouseMove(_)) && self.hover.is_some() {
            cx.set_cursor(MouseCursor::Hand);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The chat tab is made by the dock on its first draw: start the
        // agent then, in the workspace's folder.
        if self.port.is_none() {
            if let Some(config) = crate::ide::Workspace::of(cx).map(|w| w.config.clone()) {
                self.start(cx, config);
            }
        }
        let uid = self.widget_uid();
        SHOWN_CHAT.with(|c| c.set(Some(uid)));
        self.draw_chrome(cx);
        let streaming = self.t.streaming_index();
        let todos = self.view.portal_list(cx, ids!(todos)).widget_uid();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let uid = item.widget_uid();
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                if uid == todos {
                    self.draw_todos(cx, &mut list);
                } else {
                    self.draw_transcript(cx, &mut list, streaming);
                }
            }
        }
        self.draw_at_popup(cx, scope);
        if self.palette_focus && self.palette_open_flag {
            self.palette_focus = false;
            self.view.text_input(cx, ids!(palette_input)).take_key_focus(cx);
        }
        if !self.focused {
            self.focused = true;
            self.view.text_input(cx, ids!(input)).take_key_focus(cx);
        }
        DrawStep::done()
    }
}

impl AgentView {
    /// y / a / n answer a waiting approval while the composer is empty; for an
    /// edit, the digits flip hunks first.
    fn approval_keys(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let Some(id) = self.t.pending_approval() else { return };
        if ke.modifiers.any() || !self.view.text_input(cx, ids!(input)).text().is_empty() {
            return;
        }
        let edit = self.t.entries.iter().rev().find_map(|e| match e {
            Entry::Tool(t) if t.id == id && t.is_edit() && !t.picks.is_empty() => Some(t.hunk_decision()),
            _ => None,
        });
        let digit = match ke.key_code {
            KeyCode::Key1 => Some(0),
            KeyCode::Key2 => Some(1),
            KeyCode::Key3 => Some(2),
            KeyCode::Key4 => Some(3),
            KeyCode::Key5 => Some(4),
            KeyCode::Key6 => Some(5),
            KeyCode::Key7 => Some(6),
            KeyCode::Key8 => Some(7),
            KeyCode::Key9 => Some(8),
            _ => None,
        };
        if let Some(h) = digit {
            self.swallow = true;
            self.toggle_hunk(cx, id, h);
            return;
        }
        let decision = match (ke.key_code, edit) {
            // Apply what is ticked; with nothing ticked that is a rejection.
            (KeyCode::KeyY, Some(Decision::Hunks(h))) if h.is_empty() => Some(Decision::Deny),
            (KeyCode::KeyY, Some(decision)) => Some(decision),
            (KeyCode::KeyY, None) => Some(Decision::Allow),
            (KeyCode::KeyA, Some(_)) => Some(Decision::Allow),
            (KeyCode::KeyA, None) => Some(Decision::AllowAlways),
            (KeyCode::KeyR, None) => Some(Decision::AllowRepo),
            (KeyCode::KeyN, _) => Some(Decision::Deny),
            _ => None,
        };
        if let Some(decision) = decision {
            self.swallow = true;
            self.decide(cx, id, decision);
        }
    }

    fn toggle_hunk(&mut self, cx: &mut Cx, id: ToolId, hunk: usize) {
        for entry in self.t.entries.iter_mut().rev() {
            if let Entry::Tool(t) = entry {
                if t.id == id {
                    if let Some(p) = t.picks.get_mut(hunk) {
                        *p = !*p;
                    }
                    break;
                }
            }
        }
        self.view.redraw(cx);
    }

    fn draw_todos(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let total = self.t.todos.len();
        list.set_item_range(cx, 0, total);
        let pulse = self.pulse();
        while let Some(index) = list.next_visible_item(cx) {
            let Some(todo) = self.t.todos.get(index) else { continue };
            let row = list.item(cx, index, id!(TodoRow));
            let (glyph, ink) = match todo.state {
                TodoState::Pending => (ICON_DOT, c(self.pal().disabled, 1.0)),
                TodoState::Active => (ICON_RUN, c(self.pal().info, 0.45 + 0.55 * pulse)),
                TodoState::Done => (ICON_CHECK, c(self.pal().ok, 1.0)),
            };
            let mut g = row.label(cx, ids!(todo_glyph));
            g.set_text(cx, glyph);
            script_apply_eval!(cx, g, { draw_text +: { color: #(ink) } });
            let text_ink = if todo.state == TodoState::Done {
                c(self.pal().meta, 1.0)
            } else {
                c(self.pal().text, 1.0)
            };
            let mut t = row.label(cx, ids!(todo_text));
            t.set_text(cx, &todo.text);
            script_apply_eval!(cx, t, { draw_text +: { color: #(text_ink) } });
            row.draw_all(cx, &mut Scope::empty());
        }
    }

    /// The palette's rows: a window of the ranked list that keeps the
    /// selection in view. A fixed pool, because a modal's content is drawn on
    /// its own layer and cannot host a deferred list.
    fn draw_palette_rows(&mut self, cx: &mut Cx2d) {
        let rows = palette_rows();
        if self.palette_sel < self.palette_first {
            self.palette_first = self.palette_sel;
        } else if self.palette_sel >= self.palette_first + rows.len() {
            self.palette_first = self.palette_sel + 1 - rows.len();
        }
        for (slot, path) in rows.iter().enumerate() {
            let mut row = self.view.view(cx, *path);
            let index = self.palette_first + slot;
            let Some(&item) = self.palette_rank.get(index) else {
                row.set_visible(cx, false);
                continue;
            };
            let it = &self.palette_items[item];
            row.set_visible(cx, true);
            let tint = if index == self.palette_sel { c(self.pal().primary, 0.22) } else { CLEAR };
            script_apply_eval!(cx, row, { draw_bg +: { color: #(tint) } });
            row.label(cx, ids!(palette_title)).set_text(cx, &it.title);
            row.label(cx, ids!(palette_hint)).set_text(cx, &it.hint);
        }
    }

    /// The transcript flattened into list rows: a tool card can carry a diff
    /// (a header and one row per line per hunk), its live or final output and
    /// an approval prompt.
    fn rows(&self) -> Vec<Line> {
        let mut rows = Vec::with_capacity(self.t.entries.len() + 8);
        for (i, entry) in self.t.entries.iter().enumerate() {
            rows.push(Line::Entry(i));
            if let Entry::Tool(tool) = entry {
                if tool.unfold > 0.0 {
                    // The whole body is laid out; while the card unfolds its
                    // rows are clipped to a height that grows pixel by pixel
                    // (`body_clip`).
                    if let Some(diff) = tool.shown_diff() {
                        for (h, hunk) in diff.hunks.iter().enumerate() {
                            rows.push(Line::Hunk(i, h));
                            for l in 0..hunk.lines.len() {
                                rows.push(Line::Diff(i, h, l));
                            }
                        }
                    }
                }
                let has_output = !tool.output.trim().is_empty();
                if tool.unfold > 0.0 && has_output {
                    rows.push(Line::Output(i));
                } else if tool.live() && has_output {
                    rows.push(Line::Tail(i));
                }
                if tool.approval.is_some() {
                    rows.push(Line::Approval(i));
                }
            }
        }
        rows
    }

    /// How far a row is indented: sub-agent work sits under its lane.
    fn depth_of(&self, line: Line) -> usize {
        let entry = match line {
            Line::Entry(e) | Line::Approval(e) | Line::Hunk(e, _) | Line::Diff(e, _, _) | Line::Output(e) | Line::Tail(e) => e,
        };
        match self.t.entries.get(entry) {
            Some(Entry::Assistant { lane, .. }) | Some(Entry::Thinking { lane, .. }) => lane_depth(&self.t.lanes, *lane),
            Some(Entry::Tool(tool)) => lane_depth(&self.t.lanes, tool.lane),
            Some(Entry::Lane { lane, .. }) => lane_depth(&self.t.lanes, *lane).saturating_sub(1),
            _ => 0,
        }
    }

    fn draw_transcript(&mut self, cx: &mut Cx2d, list: &mut PortalList, streaming: Option<usize>) {
        let rows = self.rows();
        list.set_item_range(cx, 0, rows.len());
        let lanes_in_play = !self.t.lanes.is_empty();
        while let Some(index) = list.next_visible_item(cx) {
            let Some(&kind) = rows.get(index) else { continue };
            let row = match kind {
                Line::Approval(e) => {
                    let row = list.item(cx, index, id!(ApprovalRow));
                    if let Some(Entry::Tool(tool)) = self.t.entries.get(e) {
                        row.label(cx, ids!(approval_text)).set_text(cx, tool.approval.as_deref().unwrap_or(""));
                        let edit = tool.is_edit() && !tool.picks.is_empty();
                        let on = tool.picks.iter().filter(|p| **p).count();
                        let (keys, apply, all, deny) = if edit {
                            (
                                format!("{on} of {} hunks ticked.  1-9 flip a hunk,  y applies the ticked ones,  a accepts all,  r remembers this repo,  n rejects.", tool.picks.len()),
                                if on == tool.picks.len() { "Apply  (y)" } else { "Apply ticked  (y)" },
                                "Accept all  (a)",
                                "Reject  (n)",
                            )
                        } else {
                            (String::new(), "Allow  (y)", "This session  (a)", "Deny  (n)")
                        };
                        let hint = row.label(cx, ids!(approval_keys));
                        hint.set_text(cx, &keys);
                        hint.set_visible(cx, !keys.is_empty());
                        row.button(cx, ids!(allow_button)).set_text(cx, apply);
                        row.button(cx, ids!(always_button)).set_text(cx, all);
                        row.button(cx, ids!(repo_button)).set_text(cx, "This repo  (r)");
                        row.button(cx, ids!(deny_button)).set_text(cx, deny);
                    }
                    row
                }
                Line::Output(e) => {
                    let row = list.item(cx, index, id!(OutputRow));
                    if let Some(Entry::Tool(tool)) = self.t.entries.get(e) {
                        let live = tool.live();
                        let clean = terminal_text(&tool.output, usize::MAX);
                        let all: Vec<&str> = clean.lines().collect();
                        let from = all.len().saturating_sub(40);
                        let lines = &all[from..];
                        let mut text = lines.join("\n");
                        if live {
                            text.push_str(if self.pulse() > 0.5 { " _" } else { "  " });
                        }
                        row.label(cx, ids!(output_text)).set_text(cx, &text);
                        let mut frame = row.clone();
                        let border = if live { c(self.pal().info, 0.3 + 0.4 * self.pulse()) } else { c(self.pal().meta, 0.2) };
                        script_apply_eval!(cx, frame, { draw_bg +: { border_color: #(border) } });
                    }
                    row
                }
                Line::Tail(e) => {
                    let row = list.item(cx, index, id!(TailRow));
                    if let Some(Entry::Tool(tool)) = self.t.entries.get(e) {
                        row.label(cx, ids!(tail_text)).set_text(cx, &terminal_text(&tool.output, TAIL_LINES));
                    }
                    row
                }
                Line::Hunk(e, h) => {
                    let row = list.item(cx, index, id!(HunkRow));
                    if let Some(Entry::Tool(tool)) = self.t.entries.get(e) {
                        if let Some(diff) = tool.shown_diff() {
                            let hunk = &diff.hunks[h];
                            let pending = tool.approval.is_some();
                            let picked = tool.picks.get(h).copied().unwrap_or(true);
                            let check = if !pending { "" } else if picked { "●" } else { "○" };
                            row.label(cx, ids!(hunk_check)).set_text(cx, check);
                            let file = if h == 0 { format!("{}   ", diff.path) } else { String::new() };
                            let n = diff.hunks.len();
                            let key = if pending && n > 1 && h < 9 { format!("   [{}]", h + 1) } else { String::new() };
                            let undone = if tool.undone && h == 0 { "   (undone)" } else { "" };
                            row.label(cx, ids!(hunk_text)).set_text(
                                cx,
                                &format!("{file}hunk {}/{n}  @@ -{} +{}{key}{undone}", h + 1, hunk.old_start, hunk.new_start),
                            );
                            row.button(cx, ids!(undo_button)).set_visible(cx, h == 0 && tool.can_undo());
                        }
                    }
                    row
                }
                Line::Diff(e, h, l) => {
                    let row = list.item(cx, index, id!(DiffRow));
                    let mut text = String::new();
                    let mut tint = CLEAR;
                    if let Some(Entry::Tool(tool)) = self.t.entries.get(e) {
                        if let Some(diff) = tool.shown_diff() {
                            let rejected = tool.approval.is_some() && !tool.picks.get(h).copied().unwrap_or(true);
                            let (kind, line) = &diff.hunks[h].lines[l];
                            let (mark, color) = match kind {
                                DiffLine::Added => ('+', c(self.pal().add, 0.16)),
                                DiffLine::Removed => ('-', c(self.pal().del, 0.16)),
                                DiffLine::Context => (' ', CLEAR),
                            };
                            tint = if rejected { c(self.pal().meta, 0.05) } else { color };
                            text = format!("{mark} {line}");
                        }
                    }
                    row.label(cx, ids!(diff_text)).set_text(cx, &text);
                    let mut bg = row.clone();
                    script_apply_eval!(cx, bg, { draw_bg +: { color: #(tint) } });
                    row
                }
                Line::Entry(index_in) => self.draw_entry(cx, list, index, index_in, streaming),
            };
            if lanes_in_play {
                let left = self.depth_of(kind) as f64 * 14.0;
                let (top, bottom) = base_margin(kind);
                let mut shifted = row.clone();
                script_apply_eval!(cx, shifted, { margin: mod.turtle.Inset{left: #(left) top: #(top) right: 0.0 bottom: #(bottom)} });
            }
            // A card's body unfolds as one block of pixels: each body row is
            // clipped to its share of the eased height, so the rows below
            // slide continuously and nothing re-lays out per line.
            let clip = self.body_clip(kind);
            let mut sized = row.clone();
            match clip {
                Some(h) => script_apply_eval!(cx, sized, { height: #(h) }),
                None if is_body(kind) => script_apply_eval!(cx, sized, { height: mod.turtle.Fit }),
                None => {}
            }
            self.draw_links(cx, &row, index, kind);
            row.draw_all(cx, &mut Scope::empty());
            if clip.is_none() && is_body(kind) {
                let h = row.area().rect(cx).size.y;
                if h > 0.0 {
                    if let Some(key) = self.body_key(kind) {
                        self.body_h.insert(key, h);
                    }
                    if matches!(kind, Line::Diff(..)) {
                        self.line_h = h;
                    }
                }
            }
        }
    }

    /// (entry, index of the row in its card's body: hunk heads, diff lines,
    /// then the output).
    fn body_key(&self, line: Line) -> Option<(usize, usize)> {
        let (Line::Hunk(e, _) | Line::Diff(e, _, _) | Line::Output(e)) = line else { return None };
        let Some(Entry::Tool(tool)) = self.t.entries.get(e) else { return None };
        let hunks = tool.shown_diff().map(|d| d.hunks.as_slice()).unwrap_or(&[]);
        let before = |h: usize| hunks[..h.min(hunks.len())].iter().map(|k| 1 + k.lines.len()).sum::<usize>();
        Some(match line {
            Line::Hunk(_, h) => (e, before(h)),
            Line::Diff(_, h, l) => (e, before(h) + 1 + l),
            _ => (e, before(hunks.len())),
        })
    }

    /// The height a body row is clipped to while its card unfolds or folds,
    /// `None` when the card is at rest (the row takes its own height).
    fn body_clip(&self, line: Line) -> Option<f64> {
        let (e, idx) = self.body_key(line)?;
        let Some(Entry::Tool(tool)) = self.t.entries.get(e) else { return None };
        if tool.unfold >= 1.0 || tool.unfold <= 0.0 {
            return None;
        }
        // Heights as last drawn at rest, else from the line height.
        let line_h = self.line_h;
        let hunks = tool.shown_diff().map(|d| d.hunks.clone()).unwrap_or_default();
        let mut natural: Vec<f64> = Vec::new();
        for hunk in &hunks {
            natural.push(HUNK_H);
            natural.extend(std::iter::repeat(line_h).take(hunk.lines.len()));
        }
        if !tool.output.trim().is_empty() {
            let n = terminal_text(&tool.output, usize::MAX).lines().count().min(40) as f64;
            natural.push(OUTPUT_PAD + n * line_h);
        }
        for (i, h) in natural.iter_mut().enumerate() {
            if let Some(m) = self.body_h.get(&(e, i)) {
                *h = *m;
            }
        }
        let total: f64 = natural.iter().sum();
        let u = tool.unfold as f64;
        let reveal = total * u * u * (3.0 - 2.0 * u);
        let before: f64 = natural[..idx.min(natural.len())].iter().sum();
        let own = natural.get(idx).copied().unwrap_or(0.0);
        Some((reveal - before).clamp(0.0, own))
    }

    fn draw_entry(&mut self, cx: &mut Cx2d, list: &mut PortalList, index: usize, e: usize, streaming: Option<usize>) -> WidgetRef {
        let pulse = self.pulse();
        let entry = &self.t.entries[e];
        match entry {
            Entry::User { text, refs } => {
                let row = list.item(cx, index, id!(UserRow));
                row.label(cx, ids!(user_text)).set_text(cx, text);
                let names: Vec<String> = refs.iter().map(|r| format!("@{}", r.rsplit('/').next().unwrap_or(r))).collect();
                let refs_label = row.label(cx, ids!(user_refs));
                refs_label.set_visible(cx, !refs.is_empty());
                refs_label.set_text(cx, &names.join("   "));
                row
            }
            Entry::Assistant { text, .. } => {
                let row = list.item(cx, index, id!(AssistantRow));
                row.set_visible(cx, !text.trim().is_empty());
                let mut md = row.markdown(cx, ids!(assistant_md));
                // Paths become links (same rendered text, in the link ink).
                let body = linkify(&mut self.exists, &self.root, text);
                if streaming == Some(e) {
                    md.set_text(cx, &format!("{body} |"));
                } else {
                    md.set_text(cx, &body);
                }
                // Every row ends its fade (a row that stopped streaming while
                // another began would stay half transparent otherwise).
                match self.fade.row(e, streaming, md.is_streaming_animation_done()) {
                    FadeOp::Reset => md.reset_all_streaming_animations(),
                    FadeOp::Start => md.start_streaming_animation(),
                    FadeOp::Stop => md.stop_streaming_animation(),
                }
                row
            }
            Entry::Thinking { text, .. } => {
                let row = list.item(cx, index, id!(ThinkingRow));
                let mut label = row.label(cx, ids!(thinking_text));
                label.set_text(cx, text);
                // A slow shimmer while the turn is still going.
                let alpha = if self.t.busy { 0.55 + 0.4 * pulse } else { 0.75 };
                let ink = c(self.pal().meta, alpha);
                script_apply_eval!(cx, label, { draw_text +: { color: #(ink) } });
                row
            }
            Entry::Lane { lane, title, done } => {
                let row = list.item(cx, index, id!(LaneRow));
                let running = done.is_none();
                let mut dot = row.label(cx, ids!(lane_row_dot));
                let ink = if running { c(self.pal().info, 0.35 + 0.65 * pulse) } else { c(self.pal().ok, 1.0) };
                script_apply_eval!(cx, dot, { draw_text +: { color: #(ink) } });
                row.label(cx, ids!(lane_row_title)).set_text(cx, &format!("sub-agent {lane}  ·  {}", clip(title, 70)));
                let state = match done {
                    None => "working".to_string(),
                    Some(s) => clip(s, 60),
                };
                row.label(cx, ids!(lane_row_state)).set_text(cx, &state);
                let mut frame = row.clone();
                let border = if running { c(self.pal().info, 0.25 + 0.4 * pulse) } else { c(self.pal().meta, 0.2) };
                script_apply_eval!(cx, frame, { draw_bg +: { border_color: #(border) } });
                row
            }
            Entry::Tool(tool) => {
                let row = list.item(cx, index, id!(ToolRow));
                let glyph = match tool.state {
                    _ if tool.in_background() => ICON_RUN,
                    ToolState::Running => ICON_RUN,
                    ToolState::Ok => ICON_CHECK,
                    ToolState::Failed => ICON_CROSS,
                };
                // A new line fades in; a running one breathes, then settles
                // into its check mark.
                let age = tool.started.map_or(1.0, |t| (t.elapsed().as_secs_f32() / 0.25).min(1.0));
                let mut g = row.label(cx, ids!(tool_glyph));
                g.set_text(cx, glyph);
                let ink = match tool.state {
                    // Returned early, still running: amber, breathing.
                    _ if tool.in_background() => c(self.pal().warning, (0.35 + 0.65 * pulse) * age),
                    ToolState::Running => c(self.pal().info, (0.35 + 0.65 * pulse) * age),
                    ToolState::Ok => c(self.pal().ok, age),
                    ToolState::Failed => c(self.pal().error, age),
                };
                let ink_state = ink;
                script_apply_eval!(cx, g, { draw_text +: { color: #(ink) } });
                let mut title = row.label(cx, ids!(tool_title));
                title.set_text(cx, &tool.title);
                let ink = c(self.pal().text, age);
                script_apply_eval!(cx, title, { draw_text +: { color: #(ink) } });
                let summary = if tool.running() {
                    tool.started.map(|t| format!("{:.1}s", t.elapsed().as_secs_f32())).unwrap_or_default()
                } else if let Some(bg) = tool.background.as_ref().filter(|b| b.ended.is_none()) {
                    let secs = tool.started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                    match &bg.pattern {
                        Some(p) => format!("early yield `{p}`  ·  running in background (run {})  ·  {secs:.1}s", bg.run),
                        None => format!("running in background (run {})  ·  {secs:.1}s", bg.run),
                    }
                } else if tool.background.is_some() {
                    tool.summary.clone()
                } else {
                    match tool.took {
                        Some(secs) if secs >= 0.5 && !tool.summary.is_empty() => format!("{}  ·  {:.1}s", tool.summary, secs),
                        _ => tool.summary.clone(),
                    }
                };
                let mut summary_label = row.label(cx, ids!(tool_summary));
                summary_label.set_text(cx, &summary);
                // A command running on reads amber, with its pattern.
                let summary_ink = if tool.in_background() { c(self.pal().warning, 0.9) } else { c(self.pal().meta, 1.0) };
                script_apply_eval!(cx, summary_label, { draw_text +: { color: #(summary_ink) } });
                // The resolve scan: the outcome's ink sweeps the row once.
                let scan = super::look::resolve_scan(tool.started, tool.took);
                let mut frame = row.clone();
                script_apply_eval!(cx, frame, { draw_bg +: { scan: #(scan) ink: #(ink_state) } });
                row
            }
            Entry::Notice(text) => {
                let row = list.item(cx, index, id!(NoticeRow));
                row.label(cx, ids!(notice_text)).set_text(cx, text);
                row
            }
            Entry::Question(text) => {
                let row = list.item(cx, index, id!(QuestionRow));
                row.label(cx, ids!(question_text)).set_text(cx, text);
                row
            }
            Entry::Error(text) => {
                let row = list.item(cx, index, id!(ErrorRow));
                row.label(cx, ids!(error_text)).set_text(cx, text);
                row
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Line {
    Entry(usize),
    Approval(usize),
    /// entry, hunk
    Hunk(usize, usize),
    /// entry, hunk, line
    Diff(usize, usize, usize),
    Output(usize),
    /// A running command's last lines.
    Tail(usize),
}

impl AgentView {
    fn copy_out(&mut self, cx: &mut Cx, region: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        cx.copy_to_clipboard(text);
        log!("code: copied {} chars ({region})", text.chars().count());
        let clock = self.clock.max(cx.seconds_since_app_start());
        self.clock = clock;
        if let Some(mut hud) = self.view.widget(cx, ids!(hud)).borrow_mut::<CodeHud>() {
            hud.flash_copied(text, clock);
        }
        self.view.redraw(cx);
        self.kick(cx);
    }

    /// What a right-click at `abs` copies: (region, text).
    fn text_under(&self, cx: &mut Cx, abs: DVec2) -> Option<(&'static str, String)> {
        let inside = |cx: &mut Cx, view: &ViewRef| view.area().clipped_rect(cx).contains(abs);
        if self.palette_open_flag {
            for (slot, path) in palette_rows().iter().enumerate() {
                let row = self.view.view(cx, *path);
                if row.visible() && inside(cx, &row) {
                    let item = self.palette_rank.get(self.palette_first + slot)?;
                    return Some(("palette", self.palette_items[*item].title.clone()));
                }
            }
        }
        let list = self.view.portal_list(cx, ids!(transcript));
        if list.area().clipped_rect(cx).contains(abs) {
            if list.has_selection() {
                let rows = self.rows();
                return Some(("selection", list.selected_text_with(&|id| rows.get(id).map(|line| self.row_text(*line)))));
            }
            let id = list.item_at(cx, abs)?;
            let line = *self.rows().get(id)?;
            return Some(("transcript", self.row_text(line)));
        }
        if let Some(hud) = self.view.widget(cx, ids!(hud)).borrow::<CodeHud>() {
            if hud.contains(cx, abs) {
                return Some(("hud", hud.text_at(cx, abs).unwrap_or_else(|| hud.full_text())));
            }
        }
        let status = self.view.view(cx, ids!(status_bar));
        if inside(cx, &status) {
            let parts: Vec<String> = [ids!(status_route), ids!(status_state), ids!(status_mode), ids!(status_rate), ids!(status_context)]
                .iter()
                .map(|p| self.view.label(cx, *p).text())
                .filter(|t| !t.trim().is_empty())
                .collect();
            return Some(("status", parts.join("  ·  ")));
        }
        None
    }

    /// A transcript row as plain text, for copying rows that are not drawn.
    fn row_text(&self, line: Line) -> String {
        let tool = |e: usize| match self.t.entries.get(e) {
            Some(Entry::Tool(t)) => Some(t),
            _ => None,
        };
        match line {
            Line::Entry(e) => match self.t.entries.get(e) {
                Some(Entry::User { text, refs }) if refs.is_empty() => format!("> {text}"),
                Some(Entry::User { text, refs }) => format!("> {text}\n  {}", refs.iter().map(|r| format!("@{r}")).collect::<Vec<_>>().join(" ")),
                Some(Entry::Assistant { text, .. }) => text.clone(),
                Some(Entry::Thinking { text, .. }) => text.clone(),
                Some(Entry::Tool(t)) => if t.summary.is_empty() { t.title.clone() } else { format!("{} {}", t.title, t.summary) },
                Some(Entry::Notice(text)) | Some(Entry::Question(text)) | Some(Entry::Error(text)) => text.clone(),
                Some(Entry::Lane { lane, title, done }) => format!("sub-agent {lane} · {title} {}", done.as_deref().unwrap_or("")).trim_end().to_string(),
                None => String::new(),
            },
            Line::Approval(e) => tool(e).and_then(|t| t.approval.clone()).unwrap_or_default(),
            Line::Hunk(e, h) => tool(e)
                .and_then(|t| t.shown_diff())
                .and_then(|d| d.hunks.get(h))
                .map(|hunk| format!("@@ -{} +{} @@", hunk.old_start, hunk.new_start))
                .unwrap_or_default(),
            Line::Diff(e, h, l) => tool(e)
                .and_then(|t| t.shown_diff())
                .and_then(|d| d.hunks.get(h))
                .and_then(|hunk| hunk.lines.get(l))
                .map(|(kind, text)| {
                    let mark = match kind {
                        DiffLine::Added => '+',
                        DiffLine::Removed => '-',
                        DiffLine::Context => ' ',
                    };
                    format!("{mark} {text}")
                })
                .unwrap_or_default(),
            Line::Output(e) | Line::Tail(e) => tool(e).map(|t| t.output.trim_end().to_string()).unwrap_or_default(),
        }
    }
}

/// Measured body row heights by (entry, body index).
type BodyHeights = std::collections::HashMap<(usize, usize), f64>;

/// A row of a card's body (what unfolds).
fn is_body(line: Line) -> bool {
    matches!(line, Line::Hunk(..) | Line::Diff(..) | Line::Output(_))
}

/// Natural heights before a row has been measured: a hunk head (20 + 2
/// padding) and the output block's vertical padding.
const HUNK_H: f64 = 22.0;
const OUTPUT_PAD: f64 = 9.0;

/// The vertical margins a row template declares (indenting sets all four).
fn base_margin(line: Line) -> (f64, f64) {
    match line {
        Line::Approval(_) => (3.0, 3.0),
        Line::Entry(_) | Line::Hunk(..) | Line::Output(_) | Line::Diff(..) | Line::Tail(_) => (0.0, 0.0),
    }
}

/// Terminal output as plain lines (escapes stripped, a carriage return starts
/// the line over, so cargo's progress counts once), the last `n` of them.
/// Only the end of a long output is looked at.
fn terminal_text(raw: &str, n: usize) -> String {
    let mut start = raw.len().saturating_sub(if n == usize::MAX { usize::MAX } else { TAIL_BYTES });
    while !raw.is_char_boundary(start) {
        start += 1;
    }
    let clean = makepad_code_agent::tools::clean_terminal(&raw[start..]);
    let lines: Vec<&str> = clean.trim_end().lines().collect();
    let skip = lines.len().saturating_sub(n);
    lines[skip..].join("\n")
}

impl WidgetMatchEvent for AgentView {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        let input = self.view.text_input(cx, ids!(input));
        if let Some((text, _)) = input.returned(actions) {
            self.submit(cx, text);
        }
        if let Some(text) = input.changed(actions) {
            if std::mem::take(&mut self.swallow) && text.chars().count() == 1 {
                input.set_text(cx, "");
            } else {
                self.update_at(cx, &text);
            }
        }
        if let Some(key) = input.key_down_unhandled(actions) {
            if !key.modifiers.any() {
                match key.key_code {
                    KeyCode::ArrowUp => self.recall(cx, true),
                    KeyCode::ArrowDown => self.recall(cx, false),
                    _ => {}
                }
            }
        }
        if self.view.view(cx, ids!(ctx_chip)).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
            self.open_context_menu(cx);
        }
        self.palette_actions(cx, actions);

        for (slot, path) in chip_slots().iter().enumerate() {
            let chip = self.view.view(cx, *path);
            if chip.view(cx, ids!(chip_x)).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
                self.remove_reference(cx, slot);
                break;
            }
        }

        if self.view.view(cx, ids!(plan_head)).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
            self.toggle_pane(cx, Pane::Plan);
        }

        let list = self.view.portal_list(cx, ids!(transcript));
        let items = list.items_with_actions(actions);
        if !items.is_empty() {
            let rows = self.rows();
            for (index, item) in items {
                let Some(&line) = rows.get(index) else { continue };
                match line {
                    Line::Hunk(e, h) => {
                        if item.button(cx, ids!(undo_button)).clicked(actions) {
                            let mut undone = None;
                            if let Some(Entry::Tool(tool)) = self.t.entries.get_mut(e) {
                                tool.undone = true;
                                let id = tool.id;
                                undone = tool.diff.clone();
                                if let Some(port) = self.port.as_mut() {
                                    port.send(Command::Undo(id));
                                }
                            }
                            // Once the file is back as it was, its old lines light up.
                            if let Some(diff) = undone {
                                self.undoing.push(diff);
                            }
                            self.view.redraw(cx);
                        } else if self.link_opened {
                        } else if item.view(cx, ids!(hunk_head)).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
                            if let Some(Entry::Tool(tool)) = self.t.entries.get(e) {
                                if tool.approval.is_some() {
                                    let id = tool.id;
                                    self.toggle_hunk(cx, id, h);
                                }
                            }
                        }
                    }
                    Line::Entry(e) | Line::Approval(e) => {
                        let Some(Entry::Tool(tool)) = self.t.entries.get(e) else { continue };
                        let id = tool.id;
                        let edit = tool.is_edit() && !tool.picks.is_empty();
                        let picked = tool.hunk_decision();
                        let picked_none = matches!(&picked, Decision::Hunks(h) if h.is_empty());
                        if item.button(cx, ids!(allow_button)).clicked(actions) {
                            let decision = if !edit {
                                Decision::Allow
                            } else if picked_none {
                                Decision::Deny
                            } else {
                                picked
                            };
                            self.decide(cx, id, decision);
                        } else if item.button(cx, ids!(always_button)).clicked(actions) {
                            self.decide(cx, id, if edit { Decision::Allow } else { Decision::AllowAlways });
                        } else if item.button(cx, ids!(repo_button)).clicked(actions) {
                            self.decide(cx, id, Decision::AllowRepo);
                        } else if item.button(cx, ids!(deny_button)).clicked(actions) {
                            self.decide(cx, id, Decision::Deny);
                        } else if item.view(cx, ids!(tool_head)).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
                            if let Some(Entry::Tool(tool)) = self.t.entries.get_mut(e) {
                                tool.expanded = !tool.expanded;
                            }
                            self.view.redraw(cx);
                            self.kick(cx);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

impl AgentView {
    fn palette_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let modal = self.view.modal(cx, ids!(palette));
        if modal.dismissed(actions) {
            self.palette_open_flag = false;
            self.view.text_input(cx, ids!(input)).take_key_focus(cx);
            return;
        }
        if !modal.is_open() {
            return;
        }
        let input = self.view.text_input(cx, ids!(palette_input));
        if let Some(text) = input.changed(actions) {
            if text != self.palette_query {
                self.palette_rank = palette::rank(&self.palette_items, &text);
                self.palette_query = text;
                self.palette_sel = 0;
                self.view.redraw(cx);
            }
        }
        if let Some(key) = input.key_down_unhandled(actions) {
            let last = self.palette_rank.len().saturating_sub(1);
            match key.key_code {
                KeyCode::ArrowDown => self.palette_sel = (self.palette_sel + 1).min(last),
                KeyCode::ArrowUp => self.palette_sel = self.palette_sel.saturating_sub(1),
                _ => {}
            }
            self.view.redraw(cx);
        }
        if input.returned(actions).is_some() {
            let pick = self.palette_sel;
            self.run_palette_pick(cx, pick);
            return;
        }
        for (slot, path) in palette_rows().iter().enumerate() {
            if self.view.view(cx, *path).finger_up(actions).is_some_and(|f| !f.cancelled && f.device.is_primary_hit()) {
                let index = self.palette_first + slot;
                self.run_palette_pick(cx, index);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::at_token;

    #[test]
    fn at_queries_start_at_a_word_start_and_end_at_the_cursor() {
        assert_eq!(at_token("explain @main", 13), Some((8, "main".into())));
        assert_eq!(at_token("@", 1), Some((0, String::new())));
        assert_eq!(at_token("mail me@example.com", 19), None);
        assert_eq!(at_token("fix @main.rs now", 16), None);
        assert_eq!(at_token("fix @main.rs now", 12), Some((4, "main.rs".into())));
        assert_eq!(at_token("no at here", 10), None);
    }
}
