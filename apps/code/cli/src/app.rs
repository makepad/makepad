//! The session: what the agent says becomes rows (`render`), what the
//! person types becomes commands. One `App` serves the interactive
//! terminal (keys, live prompt) and the line mode (pipes, `-p`).

use crate::line::{Edit, LineEditor};
use crate::render::{self, Mark, Screen, Style};
use crate::term::Key;
use crate::text;
use crate::Options;
use makepad_code_agent::diff::preview_edit;
use makepad_code_agent::event::*;
use makepad_code_agent::index::FileIndex;
use makepad_code_agent::port::{self, AgentPort};
use makepad_code_agent::session_store;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Lines of a running command shown live, and kept when it is done.
const LIVE_OUTPUT: usize = 8;
const FOLDED_OUTPUT: usize = 4;

pub fn parse_mode(s: &str) -> Option<ApprovalMode> {
    match s {
        "ask" => Some(ApprovalMode::Ask),
        "allowlist" | "allow" => Some(ApprovalMode::Allowlist),
        "auto" => Some(ApprovalMode::Auto),
        _ => None,
    }
}

fn mode_name(m: ApprovalMode) -> &'static str {
    match m {
        ApprovalMode::Ask => "ask",
        ApprovalMode::Allowlist => "allowlist",
        ApprovalMode::Auto => "auto",
    }
}

/// `32768`, `32k`, `32K` -> tokens (k = 1024).
pub fn parse_tokens(s: &str) -> Option<u32> {
    let s = s.trim().to_ascii_lowercase();
    match s.strip_suffix('k') {
        Some(n) => n.parse::<u32>().ok().map(|n| n * 1024),
        None => s.parse().ok(),
    }
}

/// What a stream of text is: it changes, the open line ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Answer,
    Thinking,
    Sub,
}

struct RunningTool {
    id: ToolId,
    lane: LaneId,
    name: String,
    title: String,
    args: Vec<(String, String)>,
    output: String,
    /// It returned while its command runs on: the run number.
    run: Option<u64>,
}

struct Approval {
    id: ToolId,
    prompt: String,
    /// Per-hunk ticks for an edit; `None` for anything else.
    picks: Option<Vec<bool>>,
}

struct Completion {
    start: usize,
    prefix: &'static str,
    items: Vec<String>,
    next: usize,
}

pub struct App {
    port: Box<dyn AgentPort>,
    cfg: AgentConfig,
    screen: Screen,
    style: Style,
    editor: LineEditor,
    pub quit: bool,
    busy: bool,
    busy_since: Instant,
    tick: usize,
    last_tick: Instant,
    status: Option<Status>,
    usage: Option<Usage>,
    report: Option<ContextReport>,
    show_report: bool,
    listed: Vec<SessionInfo>,
    session: Option<String>,
    mode: ApprovalMode,
    refs: Vec<String>,
    tools: Vec<RunningTool>,
    /// run_commands that returned while their command runs on in the
    /// background; each ends with `RunEnded` (or the session's end).
    background: Vec<RunningTool>,
    approval: Option<Approval>,
    question: Option<ToolId>,
    /// Edits whose diff was shown at approval time (not shown again).
    previewed: Vec<ToolId>,
    /// The open line of streaming text, not yet in the scrollback.
    open: String,
    open_kind: Kind,
    ctrl_c_armed: bool,
    /// The draft put aside while an approval or a question has the prompt.
    stashed: Option<String>,
    index: Option<FileIndex>,
    completion: Option<Completion>,
    // Line mode.
    print_mode: bool,
    turns_done: usize,
    deferred: Option<(String, Vec<String>)>,
    had_error: bool,
    dirty: bool,
}

impl App {
    pub fn new(options: Options, screen: Screen, history: Option<PathBuf>) -> App {
        let style = screen.style;
        let port = port::start(options.config.clone(), Arc::new(|| {}));
        let mut app = App {
            port,
            mode: options.config.approval,
            cfg: options.config,
            screen,
            style,
            editor: LineEditor::with_history(history),
            quit: false,
            busy: false,
            busy_since: Instant::now(),
            tick: 0,
            last_tick: Instant::now(),
            status: None,
            usage: None,
            report: None,
            show_report: false,
            listed: Vec::new(),
            session: None,
            refs: options.refs,
            tools: Vec::new(),
            background: Vec::new(),
            approval: None,
            question: None,
            previewed: Vec::new(),
            open: String::new(),
            open_kind: Kind::Answer,
            ctrl_c_armed: false,
            stashed: None,
            index: None,
            completion: None,
            print_mode: options.print.is_some(),
            turns_done: 0,
            deferred: None,
            had_error: false,
            dirty: true,
        };
        if app.screen.live {
            let s = app.style;
            app.commit(vec![
                format!("{} {}", s.bold("makepad code"), s.dim(&app.cfg.root.display().to_string())),
                s.dim("/help for commands; Esc interrupts; Ctrl+D exits"),
            ]);
        }
        if let Some(id) = options.resume {
            app.resume(&id);
        }
        if let Some(task) = options.print {
            let refs = std::mem::take(&mut app.refs);
            if app.session.is_some() {
                // Wait for the conversation to be back before the task.
                app.deferred = Some((task, refs));
            } else {
                app.submit(task, refs);
            }
        }
        app
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// An approval or a question is open.
    pub fn waiting(&self) -> bool {
        self.approval.is_some() || self.question.is_some()
    }

    pub fn done_one(&self) -> bool {
        self.turns_done > 0 && self.deferred.is_none()
    }

    pub fn failed(&self) -> bool {
        self.had_error
    }

    // ------------------------------------------------------------- output

    fn commit(&mut self, rows: Vec<String>) {
        self.close_open();
        self.screen.commit(&rows);
        self.dirty = true;
    }

    fn width(&self) -> usize {
        self.screen.width()
    }

    /// The streaming line into the scrollback.
    fn close_open(&mut self) {
        if self.screen.live {
            if !self.open.is_empty() {
                let line = std::mem::take(&mut self.open);
                let rows = self.stream_rows(&line, self.open_kind);
                self.screen.commit(&rows);
            }
        } else if !self.open.is_empty() {
            self.open.clear();
            self.screen.stream_raw("\n");
        }
    }

    fn indent_of(kind: Kind) -> &'static str {
        match kind {
            Kind::Sub => "  ",
            _ => "",
        }
    }

    fn stream_rows(&self, line: &str, kind: Kind) -> Vec<String> {
        let rows = render::prose_rows(line, self.width(), Self::indent_of(kind));
        match kind {
            Kind::Answer => rows,
            _ => rows.into_iter().map(|r| self.style.dim(&r)).collect(),
        }
    }

    /// Streaming text: whole rows go to the scrollback as they fill, the
    /// rest stays open in the live area.
    fn stream(&mut self, kind: Kind, piece: &str) {
        if kind != self.open_kind {
            self.close_open();
            self.open_kind = kind;
        }
        if !self.screen.live {
            let shown = if kind == Kind::Answer { piece.to_string() } else { self.style.dim(piece) };
            self.screen.stream_raw(&shown);
            self.open.push_str(piece);
            if let Some(i) = self.open.rfind('\n') {
                self.open.drain(..=i);
            }
            return;
        }
        let mut parts = piece.split('\n').peekable();
        while let Some(part) = parts.next() {
            self.open.push_str(part);
            if parts.peek().is_some() {
                let line = std::mem::take(&mut self.open);
                let rows = self.stream_rows(&line, kind);
                self.screen.commit(&rows);
            }
        }
        let width = self.width().saturating_sub(text::width(Self::indent_of(kind)));
        let rows = text::wrap_line(&text::sanitize(&self.open), width, "");
        if rows.len() > 1 {
            // Everything but the last row is final; the last row is the
            // start of the open line from now on (its words may still move).
            let keep = rows.last().cloned().unwrap_or_default();
            let full: Vec<String> = rows[..rows.len() - 1].to_vec();
            let done: Vec<String> = full.iter().map(|r| format!("{}{r}", Self::indent_of(kind))).map(|r| if kind == Kind::Answer { r } else { self.style.dim(&r) }).collect();
            self.screen.commit(&done);
            self.open = keep;
        }
        self.dirty = true;
    }

    fn notice(&mut self, text: &str) {
        let rows = render::prose_rows(&self.style.plain(text), self.width(), "");
        let rows = rows.into_iter().map(|r| self.style.dim(&r)).collect();
        self.commit(rows);
    }

    fn error(&mut self, text: &str) {
        let rows = render::prose_rows(&self.style.plain(text), self.width(), "");
        let rows = rows.into_iter().map(|r| self.style.red(&r)).collect();
        self.commit(rows);
    }

    // ------------------------------------------------------------- events

    pub fn pump(&mut self) {
        for event in self.port.poll() {
            self.event(event);
        }
        if (self.busy || !self.background.is_empty()) && self.last_tick.elapsed().as_millis() >= 100 {
            self.tick += 1;
            self.last_tick = Instant::now();
            self.dirty = true;
        }
    }

    fn event(&mut self, event: AgentEvent) {
        self.dirty = true;
        match event {
            AgentEvent::Status(st) => {
                let before = self.status.as_ref().map(|s| (s.route.clone(), s.ready));
                if !self.screen.live && st.ready && before.as_ref().map_or(true, |b| !b.1) && st.route != "finding a model" {
                    let line = format!("model: {}", st.route);
                    self.notice(&line);
                }
                self.status = Some(st);
            }
            AgentEvent::Delta { lane, text } => self.stream(if lane == 0 { Kind::Answer } else { Kind::Sub }, &text),
            AgentEvent::Thinking { text, .. } => self.stream(Kind::Thinking, &text),
            AgentEvent::ToolStarted(call) => {
                self.close_open();
                if !self.screen.live && call.name == "run_command" {
                    let indent = if call.lane == 0 { "" } else { "  " };
                    let row = render::tool_row(&self.style, Mark::Started, &call.title, "", indent, self.width());
                    self.commit(vec![row]);
                }
                self.tools.push(RunningTool { id: call.id, lane: call.lane, name: call.name, title: call.title, args: call.args, output: String::new(), run: None });
            }
            AgentEvent::ToolOutput { id, text } => {
                let live = self.screen.live;
                let Some(tool) = self.tools.iter_mut().chain(self.background.iter_mut()).find(|t| t.id == id) else { return };
                tool.output.push_str(&text);
                if !live {
                    // Whole lines as they come.
                    let mut rows = Vec::new();
                    while let Some(i) = tool.output.find('\n') {
                        let line: String = tool.output.drain(..=i).collect();
                        rows.push(makepad_code_agent::tools::clean_terminal(line.trim_end_matches('\n')));
                    }
                    let rows = render::output_rows(&self.style, &rows, 0, "    ", self.screen.width());
                    self.commit(rows);
                }
            }
            AgentEvent::ToolFinished { id, ok, summary, output, diff } => self.tool_finished(id, ok, summary, output, diff),
            AgentEvent::RunBackground { id, run, .. } => {
                if let Some(tool) = self.tools.iter_mut().find(|t| t.id == id) {
                    tool.run = Some(run);
                }
            }
            AgentEvent::RunEnded { id, run, exit, ok, secs, summary } => {
                let Some(at) = self.background.iter().position(|t| t.id == id) else { return };
                let tool = self.background.remove(at);
                self.close_open();
                let s = self.style;
                let indent = if tool.lane == 0 { "" } else { "  " };
                let line = format!("run {run} {exit} after {secs:.1} s{}{summary}", s.dot());
                let row = render::tool_row(&s, if ok { Mark::Ok } else { Mark::Fail }, &tool.title, &line, indent, self.width());
                self.commit(vec![row]);
            }
            AgentEvent::ApprovalNeeded { id, prompt } => self.approval_needed(id, prompt),
            AgentEvent::Question { id, text } => {
                self.close_open();
                let s = self.style;
                let mut rows = vec![format!("{} {}", s.yellow("?"), s.bold("the agent asks:"))];
                rows.extend(render::prose_rows(&text, self.width(), "  "));
                if !self.screen.live {
                    rows.push(s.dim("  (answer on the next line)"));
                }
                self.commit(rows);
                self.question = Some(id);
                self.stash_draft();
            }
            AgentEvent::Todos(todos) => {
                let s = self.style;
                let rows = todos
                    .iter()
                    .map(|t| {
                        let mark = match (t.state, s.ascii) {
                            (TodoState::Done, true) => "[x]",
                            (TodoState::Done, false) => "[✓]",
                            (TodoState::Active, _) => "[>]",
                            (TodoState::Pending, _) => "[ ]",
                        };
                        let row = format!("  {mark} {}", text::truncate(&text::sanitize(&t.text), self.width().saturating_sub(6), s.ellipsis()));
                        if t.state == TodoState::Active { s.bold(&row) } else { s.dim(&row) }
                    })
                    .collect::<Vec<_>>();
                if !rows.is_empty() {
                    let mut all = vec![s.dim("plan")];
                    all.extend(rows);
                    self.commit(all);
                }
            }
            AgentEvent::Usage(u) => self.usage = Some(u),
            AgentEvent::ContextReport(r) => {
                self.report = Some(r);
                if std::mem::take(&mut self.show_report) {
                    self.print_model();
                }
            }
            AgentEvent::Notice(n) => self.notice(&n),
            AgentEvent::FileTouched { .. } => {}
            AgentEvent::LaneStarted { lane, title, .. } => {
                let s = self.style;
                self.commit(vec![s.dim(&format!("  sub-agent {lane}: {}", text::truncate(&title, self.width().saturating_sub(16), s.ellipsis())))]);
            }
            AgentEvent::LaneFinished { lane, summary } => {
                let s = self.style;
                let line = format!("  sub-agent {lane} done: {}", summary.lines().next().unwrap_or(""));
                self.commit(vec![s.dim(&text::truncate(&line, self.width(), s.ellipsis()))]);
            }
            AgentEvent::Summarised { upto, tokens_saved } => {
                self.notice(&format!("folded {upto} older items into a summary (saved {} tokens)", short_tokens(tokens_saved)))
            }
            AgentEvent::Idle => {
                if self.busy {
                    self.close_open();
                    self.busy = false;
                    self.tools.clear();
                    self.approval = None;
                    self.question = None;
                    self.restore_draft();
                    self.turns_done += 1;
                    if self.screen.live {
                        self.commit(vec![String::new()]);
                    }
                }
            }
            AgentEvent::Error(e) => {
                self.had_error = true;
                self.error(&format!("error: {e}"));
                if self.deferred.is_some() {
                    // A resume that failed: run the task anyway.
                    if let Some((task, refs)) = self.deferred.take() {
                        self.submit(task, refs);
                    }
                }
            }
            AgentEvent::Sessions(list) => {
                if self.listed.is_empty() {
                    self.listed = list;
                }
            }
            AgentEvent::SessionSaved(id) => self.session = Some(id),
            AgentEvent::Restored(items) => {
                self.restored(items);
                if let Some((task, refs)) = self.deferred.take() {
                    self.submit(task, refs);
                }
            }
        }
    }

    fn tool_finished(&mut self, id: ToolId, ok: bool, summary: String, output: String, diff: Option<Diff>) {
        let Some(at) = self.tools.iter().position(|t| t.id == id) else { return };
        let tool = self.tools.remove(at);
        if self.approval.as_ref().is_some_and(|a| a.id == id) {
            self.approval = None;
            self.restore_draft();
        }
        let s = self.style;
        let indent = if tool.lane == 0 { "" } else { "  " };
        let width = self.width();
        let mark = if tool.run.is_some() { Mark::Yielded } else if ok { Mark::Ok } else { Mark::Fail };
        let mut rows = vec![render::tool_row(&s, mark, &tool.title, &s.plain(&summary), indent, width)];
        if tool.name == "run_command" && self.screen.live {
            let streamed = if tool.output.is_empty() { output.clone() } else { tool.output.clone() };
            let clean = makepad_code_agent::tools::clean_terminal(&streamed);
            let (hidden, tail) = render::fold_output(&clean, FOLDED_OUTPUT);
            rows.extend(render::output_rows(&s, &tail, hidden, &format!("{indent}    "), width));
        } else if tool.name == "run_command" && !tool.output.trim().is_empty() {
            let line = makepad_code_agent::tools::clean_terminal(&tool.output);
            rows.extend(render::output_rows(&s, &[line], 0, "    ", width));
        }
        if let Some(diff) = diff {
            if !self.previewed.contains(&id) {
                rows.extend(render::diff_rows(&s, &diff, None, &format!("{indent}    "), width));
            }
        }
        self.previewed.retain(|p| *p != id);
        self.commit(rows);
        if tool.run.is_some() {
            // It runs on: its live rows show only what comes from now on.
            self.background.push(RunningTool { output: String::new(), ..tool });
        }
    }

    fn approval_needed(&mut self, id: ToolId, prompt: String) {
        self.close_open();
        let s = self.style;
        let tool = self.tools.iter().find(|t| t.id == id);
        let edit = tool.filter(|t| matches!(t.name.as_str(), "edit_file" | "write_file"));
        let preview = edit.and_then(|t| preview_edit(&self.cfg.root, &t.name, &t.args));
        let mut rows = vec![format!("{} {}", s.yellow("!"), s.bold(&text::truncate(&text::sanitize(&prompt.replace('\n', " ")), self.width().saturating_sub(2), s.ellipsis())))];
        let picks = match &preview {
            Some(diff) => {
                let picks = vec![true; diff.hunks.len()];
                rows.extend(render::diff_rows(&s, diff, Some(&picks), "    ", self.width()));
                self.previewed.push(id);
                Some(picks)
            }
            None => None,
        };
        if !self.screen.live {
            rows.push(s.dim(&format!("  {}", approval_keys(picks.is_some()))));
        }
        self.commit(rows);
        self.approval = Some(Approval { id, prompt, picks });
        self.stash_draft();
    }

    fn restored(&mut self, items: Vec<RestoredItem>) {
        let s = self.style;
        let width = self.width();
        let mut rows = vec![s.dim(&format!("-- resumed {} --", self.session.clone().unwrap_or_default()))];
        for item in items {
            match item {
                RestoredItem::User { text, refs } => {
                    rows.extend(self.user_rows(&text));
                    if !refs.is_empty() {
                        rows.push(s.dim(&format!("  with {}", refs.join(", "))));
                    }
                }
                RestoredItem::Assistant(t) if t.trim().is_empty() => {}
                RestoredItem::Assistant(t) => rows.extend(render::prose_rows(t.trim(), width, "")),
                RestoredItem::Summary(t) => rows.extend(render::prose_rows(&format!("summary: {t}"), width, "").into_iter().map(|r| s.dim(&r))),
                RestoredItem::Tool { title, ok, .. } => {
                    rows.push(render::tool_row(&s, if ok { Mark::Ok } else { Mark::Fail }, &title, "", "", width));
                }
            }
        }
        rows.push(s.dim("-- end of the saved conversation --"));
        self.commit(rows);
    }

    fn user_rows(&self, text: &str) -> Vec<String> {
        let s = self.style;
        let mut rows = Vec::new();
        for (i, line) in text.split('\n').enumerate() {
            let lead = if i == 0 { s.prompt() } else { "  " };
            for (j, row) in text::wrap_line(&text::sanitize(line), self.width().saturating_sub(2), "").into_iter().enumerate() {
                let lead = if j == 0 { lead } else { "  " };
                rows.push(s.bold(&format!("{lead}{row}")));
            }
        }
        rows
    }

    // ------------------------------------------------------------- input

    /// The draft is put aside while an approval or question has the prompt.
    fn stash_draft(&mut self) {
        if !self.editor.is_empty() {
            self.stashed = Some(self.editor.text());
        }
        self.editor.clear();
    }

    fn restore_draft(&mut self) {
        if let Some(draft) = self.stashed.take() {
            self.editor.set(&draft);
        }
    }

    pub fn ctrl_c(&mut self) {
        if self.busy {
            self.interrupt();
        } else if !self.editor.is_empty() {
            self.editor.clear();
            self.completion = None;
        } else if self.ctrl_c_armed {
            self.quit = true;
        } else {
            self.ctrl_c_armed = true;
        }
        self.dirty = true;
    }

    fn interrupt(&mut self) {
        self.port.send(Command::Interrupt);
        self.notice("interrupted");
    }

    pub fn key(&mut self, key: Key) {
        self.dirty = true;
        if key != Key::Ctrl('c') {
            self.ctrl_c_armed = false;
        }
        if !matches!(key, Key::Tab | Key::BackTab) {
            self.completion = None;
        }
        if self.approval.is_some() {
            return self.approval_key(key);
        }
        match key {
            Key::Ctrl('c') => return self.ctrl_c(),
            Key::Esc => {
                if self.busy {
                    self.interrupt();
                }
                return;
            }
            Key::Ctrl('l') => {
                self.screen.clear_screen();
                return;
            }
            Key::Tab | Key::BackTab => return self.complete(key == Key::BackTab),
            _ => {}
        }
        match self.editor.key(key) {
            Edit::Changed => {}
            Edit::Submit(text) => self.entered(text),
            Edit::Pass(Key::Ctrl('d')) => {
                if !self.busy && self.question.is_none() {
                    self.quit = true;
                }
            }
            Edit::Pass(_) => {}
        }
    }

    fn approval_key(&mut self, key: Key) {
        let Some(a) = &mut self.approval else { return };
        let id = a.id;
        let decision = match (key, &mut a.picks) {
            (Key::Char(c @ '1'..='9'), Some(picks)) => {
                let i = c as usize - '1' as usize;
                if let Some(p) = picks.get_mut(i) {
                    *p = !*p;
                    let picks = picks.clone();
                    self.redraw_picks(id, &picks);
                }
                return;
            }
            (Key::Char('y' | 'Y') | Key::Enter, Some(picks)) => {
                if picks.iter().all(|p| *p) {
                    Decision::Allow
                } else if picks.iter().any(|p| *p) {
                    Decision::Hunks(picks.iter().enumerate().filter(|(_, p)| **p).map(|(i, _)| i).collect())
                } else {
                    Decision::Deny
                }
            }
            (Key::Char('a' | 'A'), Some(_)) => Decision::Allow,
            (Key::Char('y' | 'Y'), None) => Decision::Allow,
            (Key::Char('a' | 'A'), None) => Decision::AllowAlways,
            (Key::Char('r' | 'R'), None) => Decision::AllowRepo,
            (Key::Char('n' | 'N'), _) => Decision::Deny,
            (Key::Esc | Key::Ctrl('c'), _) => {
                self.interrupt();
                return;
            }
            _ => return,
        };
        self.decide(id, decision);
    }

    fn redraw_picks(&mut self, id: ToolId, picks: &[bool]) {
        // In line mode the ticks are shown again; live, the prompt shows them.
        if !self.screen.live {
            let ticks: Vec<String> = picks.iter().enumerate().map(|(i, p)| format!("{}{}", i + 1, if *p { "+" } else { "-" })).collect();
            self.notice(&format!("hunks for {id}: {}", ticks.join(" ")));
        }
    }

    fn decide(&mut self, id: ToolId, decision: Decision) {
        let s = self.style;
        let said = match &decision {
            Decision::Allow => "allowed".to_string(),
            Decision::AllowAlways => "allowed for this session".into(),
            Decision::AllowRepo => "allowed for this repo".into(),
            Decision::Deny => "denied".into(),
            Decision::Hunks(h) => format!("applying hunks {}", h.iter().map(|i| (i + 1).to_string()).collect::<Vec<_>>().join(", ")),
        };
        if decision == Decision::Deny {
            self.previewed.retain(|p| *p != id);
        }
        self.port.send(Command::Approve { id, decision });
        self.approval = None;
        self.commit(vec![s.dim(&format!("  {said}"))]);
        self.restore_draft();
    }

    /// A line the person finished.
    fn entered(&mut self, text: String) {
        if let Some(id) = self.question.take() {
            self.commit(self.user_rows(&text));
            self.port.send(Command::Answer { id, text });
            self.restore_draft();
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.editor.remember(&text);
        if trimmed.starts_with('/') && !trimmed.starts_with("//") {
            return self.slash(trimmed);
        }
        if self.busy {
            // Keep it: one request at a time.
            self.editor.set(&text);
            self.notice("busy: Esc interrupts; Enter again when it is done");
            return;
        }
        let refs = std::mem::take(&mut self.refs);
        self.submit(text, refs);
    }

    fn submit(&mut self, text: String, refs: Vec<String>) {
        let mut rows = self.user_rows(&text);
        if !refs.is_empty() {
            rows.push(self.style.dim(&format!("  with {}", refs.join(", "))));
        }
        self.commit(rows);
        self.port.send(Command::Submit { text, refs });
        self.busy = true;
        self.busy_since = Instant::now();
    }

    /// Line mode: the next line from stdin.
    pub fn wants_line(&self) -> bool {
        self.waiting() || (!self.print_mode && !self.busy)
    }

    pub fn line(&mut self, line: &str) {
        if self.approval.is_some() {
            for c in line.chars().filter(|c| !c.is_whitespace()) {
                self.key(Key::Char(c));
            }
            return;
        }
        if self.question.is_some() {
            return self.entered(line.to_string());
        }
        self.editor.clear();
        self.editor.insert_str(line);
        let text = self.editor.text();
        self.editor.clear();
        self.entered(text);
    }

    /// Line mode, stdin is done: nobody can answer.
    pub fn no_more_input(&mut self) {
        if let Some(a) = &self.approval {
            let id = a.id;
            self.notice("no one to ask (stdin is closed): denied; --approve auto lets it run");
            self.decide(id, Decision::Deny);
        } else if let Some(id) = self.question.take() {
            self.port.send(Command::Answer { id, text: "No user is available; decide yourself.".into() });
        }
    }

    // ---------------------------------------------------------- completion

    /// Tab: `@path` (or the argument of /ref) from the repo's file list;
    /// again cycles through the matches.
    fn complete(&mut self, back: bool) {
        if let Some(c) = &mut self.completion {
            if c.items.is_empty() {
                return;
            }
            c.next = if back { (c.next + c.items.len() - 1) % c.items.len() } else { (c.next + 1) % c.items.len() };
            let with = format!("{}{}", c.prefix, c.items[c.next]);
            let start = c.start;
            self.editor.replace_before_cursor(start, &with);
            return;
        }
        let (start, word) = self.editor.word_before_cursor();
        let text = self.editor.text();
        let (prefix, query) = if let Some(q) = word.strip_prefix('@') {
            ("@", q.to_string())
        } else if text.starts_with("/ref ") && start >= 5 {
            ("", word.clone())
        } else if text.starts_with('/') && start == 0 {
            return self.complete_command(&word);
        } else {
            return;
        };
        let root = self.cfg.root.clone();
        let index = self.index.get_or_insert_with(|| FileIndex::start(&root, Arc::new(|| {})));
        let t0 = Instant::now();
        while !index.ready() && t0.elapsed().as_millis() < 1500 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let items: Vec<String> = index.search(&query, 9).into_iter().map(|m| m.path).collect();
        if items.is_empty() {
            return;
        }
        self.editor.replace_before_cursor(start, &format!("{prefix}{}", items[0]));
        self.completion = Some(Completion { start, prefix, items, next: 0 });
    }

    fn complete_command(&mut self, word: &str) {
        let hits: Vec<&str> = COMMANDS.iter().map(|(c, _)| *c).filter(|c| c.starts_with(word)).collect();
        if hits.len() == 1 {
            self.editor.replace_before_cursor(0, &format!("{} ", hits[0]));
        } else if !hits.is_empty() {
            self.completion = Some(Completion { start: 0, prefix: "", items: hits.iter().map(|h| h.to_string()).collect(), next: usize::MAX });
            if let Some(c) = &mut self.completion {
                c.next = c.items.len() - 1;
            }
        }
    }

    // ------------------------------------------------------- slash commands

    fn slash(&mut self, line: &str) {
        let mut parts = line.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        let s = self.style;
        self.commit(vec![s.dim(&format!("{}{line}", s.prompt()))]);
        match cmd {
            "/help" | "/?" => {
                let mut rows: Vec<String> = COMMANDS.iter().map(|(c, h)| format!("  {}  {}", s.bold(&format!("{c:<14}")), s.dim(h))).collect();
                rows.push(s.dim("  @path in a request attaches that file (Tab completes); Alt+Enter or a"));
                rows.push(s.dim("  trailing \\ adds a line; Esc or Ctrl+C interrupts; Ctrl+D exits."));
                self.commit(rows);
            }
            "/exit" | "/quit" => self.quit = true,
            "/new" => {
                self.port.send(Command::NewSession);
                self.session = None;
                self.refs.clear();
                self.notice("new session");
            }
            "/clear" => {
                self.screen.clear_screen();
            }
            "/resume" | "/sessions" if arg.is_empty() => self.list_sessions(),
            "/resume" => self.resume(arg),
            "/approve" => match parse_mode(arg) {
                Some(m) => {
                    self.mode = m;
                    self.port.send(Command::SetApprovalMode(m));
                    self.notice(&format!("approval: {}", mode_name(m)));
                }
                None if arg.is_empty() => self.notice(&format!("approval: {} (ask, allowlist or auto)", mode_name(self.mode))),
                None => self.error("approval modes: ask, allowlist, auto"),
            },
            "/ref" => {
                if arg.is_empty() {
                    let line = if self.refs.is_empty() { "no files attached; /ref PATH attaches one".to_string() } else { format!("attached: {}", self.refs.join(", ")) };
                    return self.notice(&line);
                }
                let rel = arg.trim_start_matches('@').to_string();
                let path = self.cfg.root.join(&rel);
                if !path.exists() || rel.split('/').any(|c| c == "..") {
                    return self.error(&format!("{rel}: no such file in {}", self.cfg.root.display()));
                }
                if !self.refs.contains(&rel) {
                    self.refs.push(rel.clone());
                }
                self.notice(&format!("attached {rel} to the next request"));
            }
            "/model" => self.model_command(arg),
            _ => self.error(&format!("unknown command {cmd}; /help lists them")),
        }
    }

    fn sessions(&self) -> Vec<SessionInfo> {
        self.cfg.session_dir.as_deref().map(session_store::list).unwrap_or_default()
    }

    fn list_sessions(&mut self) {
        let list = self.sessions();
        let s = self.style;
        if list.is_empty() {
            return self.notice("no saved sessions yet");
        }
        let width = self.width();
        let mut rows = Vec::new();
        for (i, info) in list.iter().take(20).enumerate() {
            let left = format!("{:>3}  {}  {:>3} turn{}  ", i + 1, info.when, info.turns, if info.turns == 1 { " " } else { "s" });
            let title = if info.title.is_empty() { "(untitled)".to_string() } else { info.title.replace('\n', " ") };
            let room = width.saturating_sub(text::width(&left) + info.id.len() + 2);
            rows.push(format!("{}{}  {}", s.dim(&left), text::truncate(&title, room, s.ellipsis()), s.dim(&info.id)));
        }
        rows.push(s.dim("  /resume N or /resume ID continues one"));
        self.listed = list;
        self.commit(rows);
    }

    fn resume(&mut self, arg: &str) {
        let list = if self.listed.is_empty() { self.sessions() } else { self.listed.clone() };
        let id = if arg == "last" {
            match self.sessions().first() {
                Some(s) => s.id.clone(),
                None => return self.notice("no saved sessions to resume"),
            }
        } else if let Ok(n) = arg.parse::<usize>() {
            match list.get(n.wrapping_sub(1)) {
                Some(s) => s.id.clone(),
                None => return self.error(&format!("no session {n}; /resume lists them")),
            }
        } else {
            arg.to_string()
        };
        self.session = Some(id.clone());
        self.port.send(Command::Resume(id));
    }

    fn model_command(&mut self, arg: &str) {
        let mut words = arg.split_whitespace();
        match (words.next(), words.next()) {
            (None, _) => {
                self.print_route();
                if self.report.is_some() {
                    self.print_model();
                } else {
                    self.show_report = true;
                    self.notice("asking the node for its context sizes");
                }
                self.port.send(Command::QueryContext);
            }
            (Some("kv"), Some(kv)) if matches!(kv, "q8" | "f16") => {
                self.port.send(Command::SetKv(kv.to_string()));
                self.notice(&format!("switching the KV cache to {}", kv_label(kv)));
            }
            (Some(size), None) => match parse_tokens(size) {
                Some(n) if n >= 1024 => {
                    self.port.send(Command::SetContext(n));
                    self.notice(&format!("switching the context to {}", short_tokens(n)));
                }
                _ => self.error("usage: /model, /model 32k, /model kv q8|f16"),
            },
            _ => self.error("usage: /model, /model 32k, /model kv q8|f16"),
        }
    }

    fn print_route(&mut self) {
        let line = match &self.status {
            None => "model: finding a model".to_string(),
            Some(st) if st.ready => format!("model: {} (ready)", st.route),
            Some(st) => format!("model: {}{}", st.route, if st.detail.is_empty() { String::new() } else { format!(" ({})", st.detail) }),
        };
        self.notice(&line);
    }

    fn print_model(&mut self) {
        let Some(r) = self.report.clone() else { return };
        let s = self.style;
        let mut rows = vec![format!(
            "  context {}{}{}",
            s.bold(&short_tokens(r.current)),
            r.kv.as_deref().map(|kv| format!("{}KV {}", s.dot(), kv_label(kv))).unwrap_or_default(),
            if r.max_fit > 0 { format!("{}largest that fits {}", s.dot(), short_tokens(r.max_fit)) } else { String::new() }
        )];
        for o in &r.options {
            let vram = o.vram_mb.map(|mb| format!("{:.1} GB", mb as f64 / 1024.0)).unwrap_or_default();
            let row = format!(
                "  {:>6}  {:>8}  {}{}",
                short_tokens(o.tokens),
                vram,
                if o.fits { "fits" } else { "too big" },
                if o.tokens == r.current { "  (now)" } else { "" }
            );
            rows.push(if o.tokens == r.current { s.bold(&row) } else if o.fits { row } else { s.dim(&row) });
        }
        if let Some(u) = &self.usage {
            rows.push(s.dim(&format!("  in use {} of {}", short_tokens(u.context_used), short_tokens(u.context_max))));
        }
        rows.push(s.dim("  /model 32k switches the size; /model kv q8|f16 the KV cache"));
        self.commit(rows);
    }

    // ---------------------------------------------------------------- draw

    pub fn resized(&mut self) {
        self.screen.resized();
        self.dirty = true;
    }

    fn status_row(&self) -> String {
        let s = self.style;
        let width = self.width();
        let mut parts: Vec<String> = Vec::new();
        if self.busy {
            let secs = self.busy_since.elapsed().as_secs();
            let rate = self.usage.as_ref().filter(|u| u.tokens_per_sec > 0.0).map(|u| format!("{}{:.0} tok/s", s.dot(), u.tokens_per_sec)).unwrap_or_default();
            parts.push(format!("{} working {secs}s{rate}{}esc interrupts", s.spinner(self.tick), s.dot()));
        }
        match &self.status {
            None => parts.push("finding a model".into()),
            Some(st) => {
                let mut p = st.route.clone();
                if let Some(load) = &st.load {
                    p.push_str(&format!(": {}", load.line()));
                    if let Some(f) = load.fraction {
                        let n = (f.clamp(0.0, 1.0) * 10.0).round() as usize;
                        p.push_str(&format!(" [{}{}]", "#".repeat(n), "-".repeat(10 - n)));
                    }
                } else if !st.ready && !st.detail.is_empty() {
                    p.push_str(": no model answers (see above); the prompt still works");
                }
                parts.push(p);
            }
        }
        if let Some(u) = &self.usage {
            if u.context_max > 0 {
                parts.push(format!("{}/{}", short_tokens(u.context_used), short_tokens(u.context_max)));
            }
        }
        parts.push(mode_name(self.mode).to_string());
        if !self.refs.is_empty() {
            parts.push(format!("+{}", self.refs.join(" +")));
        }
        let row = parts.join(s.dot());
        s.dim(&text::truncate(&s.plain(&row), width, s.ellipsis()))
    }

    pub fn draw(&mut self) {
        if !self.screen.live {
            self.screen.flush();
            return;
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let s = self.style;
        let width = self.width();
        let mut rows: Vec<String> = Vec::new();
        if !self.open.is_empty() {
            rows.extend(self.stream_rows(&self.open, self.open_kind));
        }
        // Leave room for the prompt: the live area never outgrows the screen.
        let budget = self.screen.rows.saturating_sub(6).max(4);
        for tool in &self.tools {
            if rows.len() >= budget {
                break;
            }
            let indent = if tool.lane == 0 { "" } else { "  " };
            let waiting = self.approval.as_ref().is_some_and(|a| a.id == tool.id);
            let mark = if waiting { Mark::Asking } else { Mark::Running(self.tick) };
            rows.push(render::tool_row(&s, mark, &tool.title, "", indent, width));
            if tool.name == "run_command" && !tool.output.is_empty() {
                let clean = makepad_code_agent::tools::clean_terminal(&tool.output);
                let keep = LIVE_OUTPUT.min(budget.saturating_sub(rows.len()).max(1));
                let (hidden, tail) = render::fold_output(&clean, keep);
                rows.extend(render::output_rows(&s, &tail, hidden, &format!("{indent}    "), width));
            }
        }
        for tool in &self.background {
            if rows.len() >= budget {
                break;
            }
            let indent = if tool.lane == 0 { "" } else { "  " };
            let run = tool.run.unwrap_or(0);
            rows.push(render::tool_row(&s, Mark::Background(self.tick), &tool.title, &format!("running in background (run {run})"), indent, width));
            if !tool.output.is_empty() && rows.len() < budget {
                let clean = makepad_code_agent::tools::clean_terminal(&tool.output);
                let (_, tail) = render::fold_output(&clean, 2);
                rows.extend(render::output_rows(&s, &tail, 0, &format!("{indent}    "), width));
            }
        }
        if let Some(c) = &self.completion {
            if c.items.len() > 1 {
                let shown: Vec<String> = c.items.iter().enumerate().map(|(i, it)| if i == c.next { s.bold(it) } else { s.dim(it) }).collect();
                let plain: String = c.items.join("  ");
                rows.push(if text::width(&plain) <= width { shown.join("  ") } else { s.dim(&text::truncate(&plain, width, s.ellipsis())) });
            }
        }
        if self.ctrl_c_armed {
            rows.push(s.dim("press Ctrl+C again to exit"));
        }
        rows.push(self.status_row());
        let (prompt, cont, lead) = if let Some(a) = &self.approval {
            let keys = approval_keys(a.picks.is_some());
            let ticks = a.picks.as_ref().map(|p| {
                p.iter().enumerate().map(|(i, on)| format!("{}{}", i + 1, if *on { if s.ascii { "+" } else { "✓" } } else { "-" })).collect::<Vec<_>>().join(" ")
            });
            let mut line = format!("{} {}", text::truncate(&s.plain(&a.prompt.replace('\n', " ")), width / 2, s.ellipsis()), keys);
            if let Some(t) = ticks {
                line.push_str(&format!("  hunks {t}"));
            }
            rows.push(s.yellow(&text::truncate(&line, width, s.ellipsis())));
            (String::from("  "), String::from("  "), true)
        } else if self.question.is_some() {
            (String::from("answer: "), String::from("        "), false)
        } else {
            (s.prompt().to_string(), "  ".to_string(), false)
        };
        let top = rows.len();
        if lead {
            // The approval row is the prompt; the cursor waits at its end.
            let end = text::width(&strip_sgr(&rows[top - 1])).min(width);
            self.screen.draw_live(&rows, (top - 1, end));
        } else {
            let (prompt_rows, (r, c)) = self.editor.layout(&prompt, &cont, width);
            for (i, row) in prompt_rows.into_iter().enumerate() {
                if i == 0 {
                    let rest = row.strip_prefix(prompt.as_str()).unwrap_or(&row).to_string();
                    rows.push(format!("{}{rest}", s.bold(&prompt)));
                } else {
                    rows.push(row);
                }
            }
            self.screen.draw_live(&rows, (top + r, c));
        }
        self.screen.flush();
    }

    /// Close the screen and stop the agent. The worker is told to stop,
    /// not waited for: a model request in flight must not hold the exit.
    pub fn finish(mut self) {
        self.close_open();
        self.screen.finish();
        self.port.send(Command::Shutdown);
        // The worker is not waited for, so the commands it left running in
        // the background are stopped here (their process groups only).
        makepad_code_agent::runs::RunManager::global().shutdown();
        std::mem::forget(self);
    }
}

fn approval_keys(edit: bool) -> &'static str {
    if edit {
        "[y] apply ticked  [a] all  [n] deny  1-9 toggle a hunk"
    } else {
        "[y] allow  [a] this session  [r] this repo  [n] deny"
    }
}

/// `s` without its SGR escapes (for measuring a painted row).
fn strip_sgr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

const COMMANDS: &[(&str, &str)] = &[
    ("/new", "start a fresh session"),
    ("/resume", "list saved sessions; /resume N or ID continues one"),
    ("/sessions", "list saved sessions (newest first)"),
    ("/model", "the model, its context size and options; /model 32k switches"),
    ("/approve", "ask | allowlist | auto"),
    ("/ref", "attach a file to the next request: /ref PATH"),
    ("/clear", "clear the screen (the conversation stays)"),
    ("/help", "this list"),
    ("/exit", "leave (Ctrl+D does too)"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(parse_tokens("32k"), Some(32768));
        assert_eq!(parse_tokens("16384"), Some(16384));
        assert_eq!(parse_tokens("x"), None);
        assert_eq!(parse_mode("auto"), Some(ApprovalMode::Auto));
        assert_eq!(strip_sgr("\x1b[2mdim\x1b[0m x"), "dim x");
    }
}
