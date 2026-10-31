//! What the window shows: the transcript and the status, built by folding
//! `AgentEvent`s. No widget types here; the view draws from this.

use makepad_code_agent::diff::preview_edit;
use makepad_code_agent::event::*;

#[derive(Clone, Debug)]
pub enum Entry {
    /// A request and the files attached to it.
    User { text: String, refs: Vec<String> },
    Assistant { lane: LaneId, text: String, streaming: bool },
    Thinking { lane: LaneId, text: String },
    Tool(ToolEntry),
    Notice(String),
    /// A sub-agent's strip: where it began and, once finished, its result.
    Lane { lane: LaneId, title: String, done: Option<String> },
    Question(String),
    Error(String),
}

/// What to do with an assistant row's text fade when it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadeOp {
    Reset,
    Start,
    Stop,
}

/// Keeps the newest assistant text's fade going until it has finished: the
/// window asks for frames while `entry` is set, and not after.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fade {
    pub entry: Option<usize>,
    /// Frames since the entry stopped streaming.
    idle_frames: u32,
}

impl Fade {
    /// Frames a finished fade may stay unconfirmed (its row not drawn, say)
    /// before the window stops asking for more.
    const GRACE_FRAMES: u32 = 120;

    pub fn clear(&mut self) {
        self.entry = None;
        self.idle_frames = 0;
    }

    /// One frame passed. True while frames are still wanted.
    pub fn frame(&mut self, streaming: bool) -> bool {
        if self.entry.is_none() {
            return false;
        }
        if streaming {
            self.idle_frames = 0;
        } else {
            self.idle_frames += 1;
            if self.idle_frames > Self::GRACE_FRAMES {
                self.clear();
            }
        }
        self.entry.is_some()
    }

    /// An assistant row is being drawn; `done` is whether its own fade has
    /// run out. Every row that is not streaming is told to stop, so none is
    /// left half transparent.
    pub fn row(&mut self, e: usize, streaming: Option<usize>, done: bool) -> FadeOp {
        if streaming == Some(e) {
            self.idle_frames = 0;
            if self.entry == Some(e) {
                FadeOp::Start
            } else {
                self.entry = Some(e);
                FadeOp::Reset
            }
        } else {
            if self.entry == Some(e) && done {
                self.entry = None;
            }
            FadeOp::Stop
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolEntry {
    pub id: ToolId,
    pub lane: LaneId,
    pub name: String,
    pub args: Vec<(String, String)>,
    pub title: String,
    pub state: ToolState,
    pub output: String,
    pub summary: String,
    pub diff: Option<Diff>,
    pub expanded: bool,
    /// The core is blocked on this prompt until the person decides.
    pub approval: Option<String>,
    /// The diff an edit approval is about, shown before anything is written.
    pub preview: Option<Diff>,
    /// Which hunks of `preview` the person still wants applied.
    pub picks: Vec<bool>,
    /// How much of the expanded body (diff, output) is revealed, 0..1; it
    /// glides toward `expanded` so a card unfolds line by line.
    pub unfold: f32,
    /// When the call began (live tools only; restored ones have none).
    pub started: Option<std::time::Instant>,
    /// How long the call took, once finished.
    pub took: Option<f32>,
    /// The person asked to undo the applied edit.
    pub undone: bool,
    /// A run_command that returned while its command runs on.
    pub background: Option<BackgroundRun>,
}

/// A command that went on running after its tool call returned.
#[derive(Clone, Debug)]
pub struct BackgroundRun {
    pub run: u64,
    /// The failure pattern that ended the wait early; None when its yield
    /// time passed.
    pub pattern: Option<String>,
    /// How it ended (`run 7 exit 1 after 22.0 s · ...`), once it has.
    pub ended: Option<String>,
}

impl ToolEntry {
    /// Seconds a card takes to unfold or fold.
    const UNFOLD_SECS: f32 = 0.18;

    /// Move `unfold` toward the expanded state by `dt` seconds. True while it
    /// is still moving (the caller keeps frames coming only then).
    pub fn step_unfold(&mut self, dt: f32) -> bool {
        let target = if self.expanded { 1.0 } else { 0.0 };
        let step = dt / Self::UNFOLD_SECS;
        if self.unfold < target {
            self.unfold = (self.unfold + step).min(target);
        } else if self.unfold > target {
            self.unfold = (self.unfold - step).max(target);
        }
        self.unfold != target
    }

    pub fn unfolding(&self) -> bool {
        self.unfold != if self.expanded { 1.0 } else { 0.0 }
    }

    /// Just begun: its line fades in.
    pub fn fresh(&self) -> bool {
        self.started.is_some_and(|t| t.elapsed().as_secs_f32() < 0.3)
    }

    pub fn running(&self) -> bool {
        self.state == ToolState::Running
    }

    /// Its command still runs in the background (the call has returned).
    pub fn in_background(&self) -> bool {
        self.background.as_ref().is_some_and(|b| b.ended.is_none())
    }

    /// Running, or running on in the background: its output is live.
    pub fn live(&self) -> bool {
        self.running() || self.in_background()
    }

    /// An applied edit the person can still take back.
    pub fn can_undo(&self) -> bool {
        self.is_edit() && self.state == ToolState::Ok && self.approval.is_none() && self.diff.is_some() && !self.undone
    }

    pub fn is_edit(&self) -> bool {
        matches!(self.name.as_str(), "edit_file" | "write_file")
    }

    /// The diff to draw: what was applied, else what is being asked about.
    pub fn shown_diff(&self) -> Option<&Diff> {
        self.diff.as_ref().or(if self.approval.is_some() { self.preview.as_ref() } else { None })
    }

    /// The decision the per-hunk picks amount to.
    pub fn hunk_decision(&self) -> Decision {
        if self.picks.iter().all(|p| *p) {
            return Decision::Allow;
        }
        Decision::Hunks(self.picks.iter().enumerate().filter(|(_, p)| **p).map(|(i, _)| i).collect())
    }
}

#[derive(Clone, Debug)]
pub struct LaneInfo {
    pub id: LaneId,
    pub parent: LaneId,
    pub title: String,
    /// `None` while it runs, the first line of its result once it is done.
    pub done: Option<String>,
}

/// How deep a lane is nested under the main agent (lane 0 is depth 0).
pub fn lane_depth(lanes: &[LaneInfo], mut lane: LaneId) -> usize {
    let mut depth = 0;
    while lane != 0 && depth < 8 {
        depth += 1;
        lane = lanes.iter().find(|l| l.id == lane).map(|l| l.parent).unwrap_or(0);
    }
    depth
}

/// What the agent did to a file, strongest last wins (Read < Edited/Created).
pub fn touch_rank(how: Touch) -> u8 {
    match how {
        Touch::Read => 1,
        Touch::Edited => 2,
        Touch::Created => 3,
        Touch::Searched | Touch::Ran => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Ok,
    Failed,
}

#[derive(Default)]
pub struct Transcript {
    pub entries: Vec<Entry>,
    /// A turn is in flight (from Submit until Idle).
    pub busy: bool,
    pub status: Option<Status>,
    pub usage: Usage,
    /// The node's last context report (sizes it offers, VRAM per size).
    pub context: Option<ContextReport>,
    pub sessions: Vec<SessionInfo>,
    /// ask_user: the next line the person sends answers it.
    pub question: Option<ToolId>,
    /// Rate samples for the sparkline in the status bar.
    pub rate: Vec<f32>,
    pub todos: Vec<Todo>,
    pub lanes: Vec<LaneInfo>,
    /// Files the agent read or changed, oldest first, each once.
    pub touched: Vec<(String, Touch)>,
    /// A touch the editor pane has not shown yet.
    pub follow: Option<String>,
    /// Lanes before this index started in earlier turns.
    pub turn_lanes_from: usize,
    /// The workspace root, for edit previews.
    pub root: std::path::PathBuf,
}

impl Transcript {
    pub fn submit(&mut self, text: String, refs: Vec<String>) {
        self.entries.push(Entry::User { text, refs });
        self.turn_lanes_from = self.lanes.len();
        self.busy = true;
    }

    /// The tool waiting for an Allow/Always/Deny.
    pub fn pending_approval(&self) -> Option<ToolId> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool(t) if t.approval.is_some() => Some(t.id),
            _ => None,
        })
    }

    pub fn clear_approval(&mut self, id: ToolId) {
        if let Some(tool) = self.tool_mut(id) {
            tool.approval = None;
        }
    }

    pub fn clear(&mut self) {
        self.question = None;
        self.entries.clear();
        self.rate.clear();
        self.todos.clear();
        self.lanes.clear();
        self.turn_lanes_from = 0;
        self.touched.clear();
        self.follow = None;
        self.busy = false;
    }

    /// The lane that is still running, in start order.
    pub fn running_lanes(&self) -> impl Iterator<Item = &LaneInfo> {
        self.lanes.iter().filter(|l| l.done.is_none())
    }

    /// Index of the assistant entry currently streaming, if any.
    pub fn streaming_index(&self) -> Option<usize> {
        match self.entries.last() {
            Some(Entry::Assistant { streaming: true, .. }) => Some(self.entries.len() - 1),
            _ => None,
        }
    }

    fn end_streaming(&mut self) {
        for entry in self.entries.iter_mut().rev() {
            match entry {
                Entry::Assistant { streaming, .. } => {
                    if !*streaming {
                        break;
                    }
                    *streaming = false;
                }
                _ => break,
            }
        }
    }

    fn tool_mut(&mut self, id: ToolId) -> Option<&mut ToolEntry> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool(t) if t.id == id => Some(t),
            _ => None,
        })
    }

    pub fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Status(status) => self.status = Some(status),
            AgentEvent::Delta { lane, text } => {
                if let Some(Entry::Assistant { lane: l, text: t, streaming: true }) = self.entries.last_mut() {
                    if *l == lane {
                        t.push_str(&text);
                        return;
                    }
                }
                self.end_streaming();
                self.entries.push(Entry::Assistant { lane, text, streaming: true });
            }
            AgentEvent::Thinking { lane, text } => {
                if let Some(Entry::Thinking { lane: l, text: t }) = self.entries.last_mut() {
                    if *l == lane {
                        t.push_str(&text);
                        return;
                    }
                }
                self.end_streaming();
                self.entries.push(Entry::Thinking { lane, text });
            }
            AgentEvent::ToolStarted(call) => {
                self.end_streaming();
                self.entries.push(Entry::Tool(ToolEntry {
                    id: call.id,
                    lane: call.lane,
                    name: call.name,
                    args: call.args,
                    title: call.title,
                    state: ToolState::Running,
                    output: String::new(),
                    summary: String::new(),
                    diff: None,
                    expanded: false,
                    approval: None,
                    preview: None,
                    picks: Vec::new(),
                    unfold: 0.0,
                    started: Some(std::time::Instant::now()),
                    took: None,
                    undone: false,
                    background: None,
                }));
            }
            AgentEvent::ToolOutput { id, text } => {
                if let Some(tool) = self.tool_mut(id) {
                    tool.output.push_str(&text);
                }
            }
            AgentEvent::ToolFinished { id, ok, summary, output, diff } => {
                if let Some(tool) = self.tool_mut(id) {
                    tool.state = if ok { ToolState::Ok } else { ToolState::Failed };
                    tool.took = tool.started.map(|t| t.elapsed().as_secs_f32());
                    tool.summary = summary;
                    // A command running on keeps its live output; the model's
                    // compact copy would cut it off.
                    if !output.is_empty() && tool.background.is_none() {
                        tool.output = output;
                    }
                    tool.approval = None;
                    tool.preview = None;
                    if diff.is_some() {
                        tool.expanded = true;
                    }
                    tool.diff = diff;
                }
            }
            AgentEvent::RunBackground { id, run, pattern } => {
                if let Some(tool) = self.tool_mut(id) {
                    tool.background = Some(BackgroundRun { run, pattern, ended: None });
                }
            }
            AgentEvent::RunEnded { id, run, exit, ok, secs, summary } => {
                if let Some(tool) = self.tool_mut(id) {
                    let line = format!("run {run} {exit} after {secs:.1} s  ·  {summary}");
                    tool.state = if ok { ToolState::Ok } else { ToolState::Failed };
                    // The outcome's scan sweeps the row again now.
                    tool.took = tool.started.map(|t| t.elapsed().as_secs_f32());
                    tool.summary = line.clone();
                    if let Some(b) = tool.background.as_mut() {
                        b.ended = Some(line);
                    }
                }
            }
            AgentEvent::ContextReport(report) => self.context = Some(report),
            AgentEvent::Notice(text) => self.entries.push(Entry::Notice(text)),
            AgentEvent::Usage(usage) => {
                if self.rate.len() >= 48 {
                    self.rate.remove(0);
                }
                self.rate.push(usage.tokens_per_sec);
                self.usage = usage;
            }
            AgentEvent::ApprovalNeeded { id, prompt } => {
                let root = self.root.clone();
                if let Some(tool) = self.tool_mut(id) {
                    tool.approval = Some(prompt);
                    if tool.is_edit() {
                        tool.preview = preview_edit(&root, &tool.name, &tool.args);
                        tool.picks = vec![true; tool.preview.as_ref().map_or(0, |d| d.hunks.len())];
                        tool.expanded = true;
                    }
                }
            }
            AgentEvent::Question { id, text } => {
                self.end_streaming();
                self.question = Some(id);
                self.entries.push(Entry::Question(text));
            }
            AgentEvent::Idle => {
                self.end_streaming();
                self.busy = false;
            }
            AgentEvent::Error(message) => {
                self.end_streaming();
                self.entries.push(Entry::Error(message));
            }
            AgentEvent::Sessions(list) => self.sessions = list,
            AgentEvent::SessionSaved(_) => {}
            AgentEvent::Restored(items) => {
                self.end_streaming();
                self.entries.clear();
                self.todos.clear();
                self.lanes.clear();
                self.touched.clear();
                self.follow = None;
                self.busy = false;
                for item in items {
                    let entry = match item {
                        RestoredItem::User { text, refs } => Entry::User { text, refs },
                        RestoredItem::Assistant(text) => Entry::Assistant { lane: 0, text, streaming: false },
                        RestoredItem::Summary(t) => Entry::Notice(t),
                        RestoredItem::Tool { title, ok, output } => Entry::Tool(ToolEntry {
                            id: 0,
                            lane: 0,
                            name: String::new(),
                            args: Vec::new(),
                            preview: None,
                            picks: Vec::new(),
                            title,
                            state: if ok { ToolState::Ok } else { ToolState::Failed },
                            output,
                            summary: String::new(),
                            diff: None,
                            expanded: false,
                            approval: None,
                            unfold: 0.0,
                            started: None,
                            took: None,
                            undone: false,
                            background: None,
                        }),
                    };
                    self.entries.push(entry);
                }
            }
            AgentEvent::Todos(list) => self.todos = list,
            AgentEvent::FileTouched { path, how } => {
                if touch_rank(how) == 0 {
                    return;
                }
                match self.touched.iter_mut().find(|(p, _)| *p == path) {
                    Some((_, old)) => {
                        if touch_rank(how) > touch_rank(*old) {
                            *old = how;
                        }
                    }
                    None => self.touched.push((path.clone(), how)),
                }
                self.follow = Some(path);
            }
            AgentEvent::LaneStarted { lane, parent, title } => {
                self.end_streaming();
                self.lanes.retain(|l| l.id != lane);
                self.lanes.push(LaneInfo { id: lane, parent, title: title.clone(), done: None });
                self.entries.push(Entry::Lane { lane, title, done: None });
            }
            AgentEvent::LaneFinished { lane, summary } => {
                if let Some(info) = self.lanes.iter_mut().find(|l| l.id == lane) {
                    info.done = Some(summary.clone());
                }
                for entry in self.entries.iter_mut().rev() {
                    if let Entry::Lane { lane: l, done, .. } = entry {
                        if *l == lane && done.is_none() {
                            *done = Some(summary);
                            break;
                        }
                    }
                }
            }
            AgentEvent::Summarised { upto, tokens_saved } => {
                self.entries.push(Entry::Notice(format!(
                    "Earlier turns folded into a summary ({upto} items, {tokens_saved} tokens saved)"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every animation runs on its own to the end and then asks for no more frames.
    #[test]
    fn fade_runs_to_completion_then_idles() {
        let mut fade = Fade::default();
        // Streaming entry 3: first draw resets, later draws start.
        assert_eq!(fade.row(3, Some(3), false), FadeOp::Reset);
        assert_eq!(fade.row(3, Some(3), false), FadeOp::Start);
        // Another entry begins while 3 still fades: 3 is told to stop, not left half transparent.
        assert_eq!(fade.row(4, Some(4), false), FadeOp::Reset);
        assert_eq!(fade.row(3, Some(4), false), FadeOp::Stop);
        // The turn ends; the row's own fade runs out after some frames.
        let mut frames = 0;
        let mut done = false;
        while fade.frame(false) {
            frames += 1;
            assert!(frames < 1000, "frames never stopped");
            assert_eq!(fade.row(4, None, done), FadeOp::Stop);
            done = frames >= 25; // the row reports its fade finished
        }
        assert!(frames >= 25 && frames < 40, "idled after {frames} frames");
        assert!(fade.entry.is_none());
        // A row that is never drawed again still lets the frames stop.
        let mut stuck = Fade::default();
        stuck.row(7, Some(7), false);
        let mut n = 0;
        while stuck.frame(false) {
            n += 1;
            assert!(n < 1000);
        }
    }

    #[test]
    fn tool_cards_unfold_and_fold_then_idle() {
        let mut t = Transcript::default();
        t.apply(AgentEvent::ToolStarted(ToolCall { id: 1, lane: 0, name: "grep".into(), title: "grep".into(), args: vec![] }));
        let Some(Entry::Tool(tool)) = t.entries.last_mut() else { panic!() };
        assert!(!tool.unfolding());
        tool.expanded = true;
        let mut frames = 0;
        while tool.step_unfold(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 200);
        }
        assert_eq!(tool.unfold, 1.0);
        assert!(frames > 5, "it must take several frames, not jump");
        assert!(!tool.unfolding());
        tool.expanded = false;
        while tool.step_unfold(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 400);
        }
        assert_eq!(tool.unfold, 0.0);
        assert!(!tool.unfolding());
    }

    fn tool(name: &str, id: ToolId, lane: LaneId) -> AgentEvent {
        AgentEvent::ToolStarted(ToolCall { id, lane, name: name.into(), title: name.into(), args: vec![] })
    }

    #[test]
    fn hunk_picks_become_the_decision() {
        let mut t = Transcript::default();
        t.apply(tool("edit_file", 1, 0));
        let Some(Entry::Tool(e)) = t.entries.last_mut() else { panic!() };
        e.picks = vec![true, true, true];
        assert_eq!(e.hunk_decision(), Decision::Allow);
        e.picks[1] = false;
        assert_eq!(e.hunk_decision(), Decision::Hunks(vec![0, 2]));
        e.picks = vec![false, false];
        assert_eq!(e.hunk_decision(), Decision::Hunks(vec![]));
    }

    #[test]
    fn an_edit_approval_carries_a_preview_of_the_file_on_disk() {
        let dir = std::env::temp_dir().join(format!("code-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (0..40).map(|i| if i == 2 || i == 35 { "x = 1;\n".into() } else { format!("line {i}\n") }).collect();
        std::fs::write(dir.join("a.txt"), body).unwrap();
        let mut t = Transcript { root: dir.clone(), ..Default::default() };
        t.apply(AgentEvent::ToolStarted(ToolCall {
            id: 7,
            lane: 0,
            name: "edit_file".into(),
            title: "edit a.txt".into(),
            args: vec![
                ("path".into(), "a.txt".into()),
                ("old_string".into(), "x = 1;".into()),
                ("new_string".into(), "x = 2;".into()),
                ("replace_all".into(), "true".into()),
            ],
        }));
        t.apply(AgentEvent::ApprovalNeeded { id: 7, prompt: "edit a.txt".into() });
        let Some(Entry::Tool(e)) = t.entries.last() else { panic!() };
        assert_eq!(e.preview.as_ref().map(|d| d.hunks.len()), Some(2));
        assert_eq!(e.picks, vec![true, true]);
        assert!(e.shown_diff().is_some());
        // Once finished, what was applied replaces the preview.
        t.apply(AgentEvent::ToolFinished { id: 7, ok: true, summary: "".into(), output: String::new(), diff: None });
        let Some(Entry::Tool(e)) = t.entries.last() else { panic!() };
        assert!(e.shown_diff().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_yielded_command_runs_on_then_ends() {
        let mut t = Transcript::default();
        t.apply(tool("run_command", 4, 0));
        t.apply(AgentEvent::ToolOutput { id: 4, text: "error[E0308]: mismatched types\n".into() });
        t.apply(AgentEvent::RunBackground { id: 4, run: 7, pattern: Some("error[E".into()) });
        t.apply(AgentEvent::ToolFinished { id: 4, ok: false, summary: "early yield `error[E` · running in background (run 7)".into(), output: "[early yield: ...]".into(), diff: None });
        let Some(Entry::Tool(e)) = t.entries.last() else { panic!() };
        assert!(e.in_background() && e.live() && !e.running());
        assert_eq!(e.output, "error[E0308]: mismatched types\n", "the live output stays");
        t.apply(AgentEvent::ToolOutput { id: 4, text: "   Compiling c\n".into() });
        t.apply(AgentEvent::RunEnded { id: 4, run: 7, exit: "exit 1".into(), ok: false, secs: 22.0, summary: "1 error".into() });
        let Some(Entry::Tool(e)) = t.entries.last() else { panic!() };
        assert!(!e.live());
        assert_eq!(e.state, ToolState::Failed);
        assert!(e.output.ends_with("Compiling c\n"));
        assert_eq!(e.summary, "run 7 exit 1 after 22.0 s  ·  1 error");
    }

    #[test]
    fn lanes_nest_and_finish() {
        let mut t = Transcript::default();
        t.apply(AgentEvent::LaneStarted { lane: 1, parent: 0, title: "a".into() });
        t.apply(AgentEvent::LaneStarted { lane: 2, parent: 1, title: "b".into() });
        assert_eq!(lane_depth(&t.lanes, 0), 0);
        assert_eq!(lane_depth(&t.lanes, 2), 2);
        assert_eq!(t.running_lanes().count(), 2);
        t.apply(AgentEvent::LaneFinished { lane: 1, summary: "done".into() });
        assert_eq!(t.running_lanes().count(), 1);
        assert!(matches!(t.entries[0], Entry::Lane { done: Some(_), .. }));
    }

    #[test]
    fn touches_keep_the_strongest_and_ignore_searches() {
        let mut t = Transcript::default();
        t.apply(AgentEvent::FileTouched { path: "a.rs".into(), how: Touch::Read });
        t.apply(AgentEvent::FileTouched { path: "a.rs".into(), how: Touch::Edited });
        t.apply(AgentEvent::FileTouched { path: "a.rs".into(), how: Touch::Read });
        t.apply(AgentEvent::FileTouched { path: "grep foo".into(), how: Touch::Searched });
        assert_eq!(t.touched, vec![("a.rs".to_string(), Touch::Edited)]);
        assert_eq!(t.follow.as_deref(), Some("a.rs"));
    }

    #[test]
    fn todos_and_resume_reset() {
        let mut t = Transcript::default();
        t.apply(AgentEvent::Todos(vec![Todo { text: "x".into(), state: TodoState::Active }]));
        assert_eq!(t.todos.len(), 1);
        t.apply(AgentEvent::Restored(vec![RestoredItem::User { text: "hi".into(), refs: vec!["a.rs".into()] }]));
        assert!(t.todos.is_empty());
        assert_eq!(t.entries.len(), 1);
    }
}
