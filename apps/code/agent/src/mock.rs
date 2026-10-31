//! A scripted agent for building the window before the core is ready. It
//! runs on its own thread like the real one: commands arrive on a channel,
//! events leave on another, and `wake` tells the window to drain them.
//! Not a model: the replies are canned, only the event shapes and timing
//! are plausible (token-sized deltas, tool cards, usage).

use crate::port::{AgentPort, Wake};
use crate::event::*;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// An offline, synthetic session that never inspects the workspace.
pub fn presentation() -> bool {
    std::env::var("CODE_MOCK").is_ok_and(|mode| mode == "presentation")
}

pub struct MockAgent {
    commands: Sender<Command>,
    events: Receiver<AgentEvent>,
}

impl MockAgent {
    pub fn start(config: AgentConfig, wake: Wake) -> Self {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        std::thread::Builder::new()
            .name("mock-agent".into())
            .spawn(move || {
                let mut script = Script { config, rx: command_rx, tx: event_tx, wake, generated: 0, context: 3_200, kv: "q8".into(), saved: fresh_saved(), engine: None };
                script.run();
            })
            .expect("spawn mock agent");
        Self { commands, events }
    }
}

impl AgentPort for MockAgent {
    fn send(&mut self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        self.events.try_iter().collect()
    }
}

/// Why a scripted wait ended early.
enum Stop {
    Interrupt,
    Shutdown,
}

struct Script {
    config: AgentConfig,
    rx: Receiver<Command>,
    tx: Sender<AgentEvent>,
    wake: Wake,
    generated: u64,
    context: u32,
    kv: String,
    /// The conversation as saved (to the config's session folder, like the
    /// core does), so the sessions list and resume work without a model.
    saved: Saved,
    /// The core's tools for `CODE_MOCK_COMMAND` turns; kept so a command
    /// that runs on in the background is followed to its end.
    engine: Option<crate::engine::Engine>,
}

/// What the mock keeps of its conversation for the session store.
#[derive(Default)]
struct Saved {
    id: String,
    started: u64,
    title: String,
    items: Vec<crate::context::Item>,
}

fn fresh_saved() -> Saved {
    use crate::session_store::now_secs;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    Saved { id: format!("m{}-{nanos}", now_secs()), started: now_secs(), ..Default::default() }
}

impl Script {
    /// The saved sessions, or a made-up list when nothing is stored.
    fn sessions(&self) -> Vec<SessionInfo> {
        match &self.config.session_dir {
            Some(dir) if !presentation() => crate::session_store::list(dir),
            _ => {
                let session = |id: &str, title: &str, when: &str, turns| SessionInfo { id: id.into(), title: title.into(), when: when.into(), turns };
                vec![
                    session("s3", "fix the parser's error recovery", "today 14:02", 6),
                    session("s2", "add a --verbose flag", "yesterday", 3),
                    session("s1", "explain the build script", "Mon", 2),
                ]
            }
        }
    }

    /// A finished turn goes to the store, as the core saves after each one.
    fn save_turn(&mut self, text: &str, refs: &[String], answer: &str) {
        use crate::context::Item;
        let Some(dir) = self.config.session_dir.clone().filter(|_| !presentation()) else { return };
        if self.saved.title.is_empty() {
            self.saved.title = text.lines().next().unwrap_or("").chars().take(60).collect();
        }
        self.saved.items.push(Item::User { text: text.to_string(), refs: refs.to_vec(), context: String::new() });
        self.saved.items.push(Item::Assistant(answer.to_string()));
        let doc = crate::session_store::Saved {
            id: self.saved.id.clone(),
            title: self.saved.title.clone(),
            started: self.saved.started,
            items: self.saved.items.clone(),
        };
        match crate::session_store::save(&dir, &doc) {
            Ok(()) => {
                self.emit(AgentEvent::SessionSaved(self.saved.id.clone()));
                self.emit(AgentEvent::Sessions(self.sessions()));
            }
            Err(e) => self.emit(AgentEvent::Error(format!("cannot save the session: {e}"))),
        }
    }

    fn emit(&self, event: AgentEvent) {
        let _ = self.tx.send(event);
        (self.wake)();
    }

    /// Wait `ms`, but a command that stops the turn ends the wait at once.
    fn pause(&mut self, ms: u64) -> Result<(), Stop> {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        loop {
            self.pump_engine();
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            match self.rx.recv_timeout(left.min(Duration::from_millis(100))) {
                Err(_) => {}
                Ok(Command::Interrupt) => return Err(Stop::Interrupt),
                Ok(Command::Shutdown) => return Err(Stop::Shutdown),
                Ok(_) => return Ok(()),
            }
        }
    }

    /// Follow the core's background runs (output, ends).
    fn pump_engine(&mut self) {
        if let Some(eng) = self.engine.as_mut() {
            eng.pump_background();
        }
    }

    /// The next command; background runs are followed while waiting.
    fn next_command(&mut self) -> Result<Command, ()> {
        loop {
            self.pump_engine();
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(c) => return Ok(c),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(()),
            }
        }
    }

    fn end_runs(&mut self) {
        if let Some(eng) = self.engine.as_mut() {
            eng.end_runs("the session ended");
        }
    }

    fn run(&mut self) {
        self.emit(AgentEvent::Status(Status {
            route: "scripted / offline".into(),
            ready: true,
            load: None,
            detail: self.config.root.display().to_string(),
        }));
        self.usage(0.0, &[(SegmentKind::System, 1_400), (SegmentKind::Project, 1_800)]);
        self.emit(AgentEvent::Sessions(self.sessions()));
        loop {
            match self.next_command() {
                Ok(Command::Submit { text, refs }) => match self.submitted(&text, &refs) {
                    Err(Stop::Shutdown) => return self.end_runs(),
                    Err(Stop::Interrupt) => {
                        self.emit(AgentEvent::Error("interrupted".into()));
                        self.emit(AgentEvent::Idle);
                    }
                    Ok(()) => {
                        let answer = if presentation() { "" } else { REPLY.lines().next().unwrap_or("") };
                        self.save_turn(&text, &refs, answer);
                        self.emit(AgentEvent::Idle)
                    }
                },
                Ok(Command::NewSession) => {
                    self.end_runs();
                    self.saved = fresh_saved()
                }
                Ok(Command::Resume(id)) => {
                    if let Some(loaded) = self.config.session_dir.as_ref().and_then(|d| crate::session_store::load(d, &id).ok()) {
                        use crate::context::Item;
                        let shown = loaded
                            .items
                            .iter()
                            .map(|i| match i {
                                Item::User { text, refs, .. } => RestoredItem::User { text: text.clone(), refs: refs.clone() },
                                Item::Assistant(t) => RestoredItem::Assistant(t.clone()),
                                Item::Summary(t) => RestoredItem::Summary(t.clone()),
                                Item::Tool { name, args, result, ok } => RestoredItem::Tool { title: format!("{name} {args}"), ok: *ok, output: result.clone() },
                            })
                            .collect();
                        self.saved = Saved { id: loaded.id, started: loaded.started, title: loaded.title, items: loaded.items };
                        self.emit(AgentEvent::Restored(shown));
                        self.emit(AgentEvent::Idle);
                        continue;
                    }
                    self.emit(AgentEvent::Restored(vec![
                        RestoredItem::User { text: format!("(session {id}) why does the parser lose its place after an error?"), refs: vec!["src/parse.rs".into()] },
                        RestoredItem::Tool { title: "grep \"recover\" src".into(), ok: true, output: "src/parse.rs:88: fn recover(&mut self)".into() },
                        RestoredItem::Assistant("It never skips to the next statement boundary, so one bad token cascades.".into()),
                    ]));
                    self.emit(AgentEvent::Idle);
                }
                Ok(Command::SetContext(tokens)) => {
                    self.config.context_tokens = tokens;
                    self.emit(AgentEvent::Notice(format!("Context is now {}.", short_tokens(tokens))));
                    self.emit(AgentEvent::ContextReport(self.report()));
                    self.usage(0.0, &[(SegmentKind::System, 2_400)]);
                }
                Ok(Command::SetKv(kv)) => {
                    if matches!(kv.as_str(), "q8" | "f16") {
                        self.kv = kv;
                        self.emit(AgentEvent::Notice(format!("KV cache is now {}.", kv_label(&self.kv))));
                    } else {
                        self.emit(AgentEvent::Error(format!("KV cache stays at {}: unknown KV precision {kv}; expected q8 or f16", kv_label(&self.kv))));
                    }
                    self.emit(AgentEvent::ContextReport(self.report()));
                    self.usage(0.0, &self.segments());
                }
                Ok(Command::QueryContext) => self.emit(AgentEvent::ContextReport(self.report())),
                // The scripted edit is never written, so there is nothing to
                // put back: report the file as the core does after an undo.
                Ok(Command::Undo(_)) => self.emit(AgentEvent::FileTouched { path: "apps/code/src/main.rs".into(), how: Touch::Edited }),
                Ok(Command::Shutdown) | Err(_) => return self.end_runs(),
                Ok(_) => {}
            }
        }
    }

    /// A made-up node report: 16 GB of weights plus 36/64 KiB of KV per token.
    fn report(&self) -> ContextReport {
        let kv_kib_per_token = if self.kv == "f16" { 64 } else { 36 };
        let vram = |t: u32| 17_300 + t * kv_kib_per_token / 1024;
        let free = 22_000u32;
        let options: Vec<_> = [16384u32, 32768, 49152, 65536]
            .iter()
            .map(|&tokens| ContextOption { tokens, vram_mb: Some(vram(tokens)), fits: vram(tokens) <= free })
            .collect();
        let max_fit = options.iter().filter(|o| o.fits).map(|o| o.tokens).max().unwrap_or(0);
        ContextReport { current: self.config.context_tokens, kv: Some(self.kv.clone()), max_fit, options }
    }

    fn usage(&mut self, rate: f32, segments: &[(SegmentKind, u32)]) {
        self.emit(AgentEvent::Usage(Usage {
            context_used: self.context,
            context_max: self.config.context_tokens.max(1),
            tokens_per_sec: rate,
            total_generated: self.generated,
            segments: segments.iter().map(|&(kind, tokens)| ContextSegment { kind, tokens }).collect(),
        }));
    }

    /// Stream `text` as token-sized deltas at about `tps` tokens a second.
    fn stream(&mut self, lane: LaneId, text: &str, tps: f32) -> Result<(), Stop> {
        let mut chars = text.chars().peekable();
        let per_token = Duration::from_secs_f32(1.0 / tps).as_millis().max(1) as u64;
        let mut batch = 0u32;
        while chars.peek().is_some() {
            // A token is about four characters, cut at a word edge when near.
            let mut piece = String::new();
            for _ in 0..4 {
                match chars.next() {
                    Some(c) => piece.push(c),
                    None => break,
                }
            }
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() && piece.chars().last().is_some_and(|l| l.is_alphanumeric()) && piece.len() < 7 {
                    piece.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            self.emit(AgentEvent::Delta { lane, text: piece });
            self.generated += 1;
            self.context += 1;
            batch += 1;
            if batch % 12 == 0 {
                self.usage(tps, &self.segments());
            }
            self.pause(per_token)?;
        }
        Ok(())
    }

    fn segments(&self) -> Vec<(SegmentKind, u32)> {
        let rest = self.context.saturating_sub(3_200);
        vec![
            (SegmentKind::System, 1_400),
            (SegmentKind::Project, 1_800),
            (SegmentKind::User, 220),
            (SegmentKind::ToolResult, rest / 3),
            (SegmentKind::Assistant, rest - rest / 3),
        ]
    }

    fn tool(&mut self, id: ToolId, name: &str, title: &str, args: &[(&str, &str)], ms: u64, summary: &str, output: &str) -> Result<(), Stop> {
        self.emit(AgentEvent::ToolStarted(ToolCall {
            id,
            lane: 0,
            name: name.into(),
            title: title.into(),
            args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }));
        self.pause(ms)?;
        self.context += 380;
        self.emit(AgentEvent::ToolFinished { id, ok: true, summary: summary.into(), output: output.into(), diff: None });
        Ok(())
    }

    /// Block until the person decides, as the core does.
    fn wait_decision(&mut self, id: ToolId) -> Result<Decision, Stop> {
        loop {
            match self.rx.recv() {
                Ok(Command::Approve { id: got, decision }) if got == id => return Ok(decision),
                Ok(Command::Interrupt) => return Err(Stop::Interrupt),
                Ok(Command::Shutdown) | Err(_) => return Err(Stop::Shutdown),
                Ok(_) => {}
            }
        }
    }

    fn edit_with_approval(&mut self) -> Result<(), Stop> {
        let id = 3;
        self.emit(AgentEvent::ToolStarted(ToolCall {
            id,
            lane: 0,
            name: "edit_file".into(),
            title: "edit apps/code/src/main.rs".into(),
            args: vec![
                ("path".into(), "apps/code/src/main.rs".into()),
                ("old_string".into(), "app_main!(App);".into()),
                ("new_string".into(), "app_main!(App); // the agent was here".into()),
                ("replace_all".into(), "true".into()),
            ],
        }));
        self.emit(AgentEvent::ApprovalNeeded { id, prompt: "edit apps/code/src/main.rs".into() });
        let picked = match self.wait_decision(id)? {
            Decision::Deny => {
                self.emit(AgentEvent::ToolFinished { id, ok: false, summary: "denied".into(), output: String::new(), diff: None });
                return Ok(());
            }
            Decision::Hunks(h) => h,
            _ => (0..2).collect(),
        };
        let args = vec![
            ("path".to_string(), "apps/code/src/main.rs".to_string()),
            ("old_string".to_string(), "app_main!(App);".to_string()),
            ("new_string".to_string(), "app_main!(App); // the agent was here".to_string()),
            ("replace_all".to_string(), "true".to_string()),
        ];
        let mut diff = crate::diff::preview_edit(&self.config.root, "edit_file", &args)
            .unwrap_or(Diff { path: "apps/code/src/main.rs".into(), hunks: Vec::new() });
        let mut keep = 0usize;
        diff.hunks.retain(|_| {
            keep += 1;
            picked.contains(&(keep - 1))
        });
        self.emit(AgentEvent::FileTouched { path: "apps/code/src/main.rs".into(), how: Touch::Edited });
        self.emit(AgentEvent::ToolFinished { id, ok: true, summary: "+4 -1".into(), output: String::new(), diff: Some(diff) });
        Ok(())
    }

    fn run_command(&mut self) -> Result<(), Stop> {
        let id = 4;
        self.emit(AgentEvent::ToolStarted(ToolCall {
            id,
            lane: 0,
            name: "run_command".into(),
            title: "$ cargo check --release".into(),
            args: vec![("command".into(), "cargo check --release".into())],
        }));
        for line in ["    Checking demo v0.1.0", "   Compiling serde v1.0.200", "   Compiling demo v0.1.0", "warning: unused variable: `x`", "  --> apps/code/src/main.rs:12:9", "    Finished `release` profile in 2.31s"] {
            self.pause(600)?;
            self.emit(AgentEvent::ToolOutput { id, text: format!("{line}\n") });
        }
        self.emit(AgentEvent::ToolFinished { id, ok: true, summary: "exit 0".into(), output: String::new(), diff: None });
        Ok(())
    }

    /// A request arrived: say what came with it (the log line is what a
    /// test run reads), then play the turn.
    fn submitted(&mut self, text: &str, refs: &[String]) -> Result<(), Stop> {
        eprintln!("code mock: submit {:?} with refs {:?}", text, refs);
        if !refs.is_empty() {
            let found = refs.iter().filter(|r| self.config.root.join(r.as_str()).exists()).count();
            self.emit(AgentEvent::Notice(format!("attached as context: {} ({found} of {} on disk)", refs.join(", "), refs.len())));
        }
        self.turn(text)
    }

    fn turn(&mut self, prompt: &str) -> Result<(), Stop> {
        if presentation() {
            return self.presentation_turn(prompt);
        }
        if let Ok(command) = std::env::var("CODE_MOCK_COMMAND") {
            return self.command_turn(&command);
        }
        self.context += (prompt.len() / 4) as u32 + 12;
        let todo = |text: &str, state| Todo { text: text.into(), state };
        self.emit(AgentEvent::Todos(vec![
            todo("find where the request is handled", TodoState::Active),
            todo("apply the change", TodoState::Pending),
            todo("run the build", TodoState::Pending),
        ]));
        self.emit(AgentEvent::Thinking { lane: 0, text: "Looking for where the request is handled.".into() });
        self.pause(500)?;
        self.emit(AgentEvent::LaneStarted { lane: 1, parent: 0, title: "survey the event handlers".into() });
        self.stream(1, "Reading the handlers.\n\n", 60.0)?;
        self.emit(AgentEvent::ToolStarted(ToolCall { id: 20, lane: 1, name: "read_file".into(), title: "read apps/code/src/main.rs:1-80".into(), args: vec![] }));
        self.emit(AgentEvent::FileTouched { path: "apps/code/src/main.rs".into(), how: Touch::Read });
        self.pause(900)?;
        self.emit(AgentEvent::ToolFinished { id: 20, ok: true, summary: "80 lines".into(), output: String::new(), diff: None });
        self.emit(AgentEvent::LaneFinished { lane: 1, summary: "one handler matches on the event kind".into() });
        self.stream(0, "I'll start by finding where this is handled.\n\n", 70.0)?;
        self.tool(1, "grep", "grep \"fn handle\" apps/code/src", &[("pattern", "fn handle"), ("path", "apps/code/src")], 600, "7 matches in 3 files", "apps/code/src/main.rs:41: fn handle_event(&mut self)\napps/code/src/main.rs:97: fn handle_key(&mut self)\napps/code/src/ui/view.rs:212: fn handle_actions(&mut self)\napps/code/src/ui/view.rs:380: fn handle_signal(&mut self)\napps/code/src/agent/mod.rs:55: fn handle(&mut self, cmd: Command)\napps/code/src/agent/engine.rs:140: fn handle_tool(&mut self)\napps/code/src/agent/tools.rs:9: fn handle_call(&self)")?;
        self.tool(5, "run_command", "$ cargo build --release --offline -p makepad-app-code --features internal 2>&1 | grep -E 'warning|error' | sort | uniq -c | sort -rn | head -40 && echo done-with-the-build-and-the-long-command-line-wraps", &[], 300, "exit 0", "")?;
        self.tool(2, "read_file", "read apps/code/src/main.rs:1-80", &[("path", "apps/code/src/main.rs"), ("range", "1-80")], 400, "80 lines", "fn main() {\n    println!(\"hello\");\n}")?;
        self.emit(AgentEvent::Todos(vec![
            todo("find where the request is handled", TodoState::Done),
            todo("apply the change", TodoState::Active),
            todo("run the build", TodoState::Pending),
        ]));
        self.stream(0, "Applying the change now.\n\n", 90.0)?;
        self.edit_with_approval()?;
        self.run_command()?;
        self.stream(0, REPLY, 90.0)?;
        self.usage(0.0, &self.segments());
        Ok(())
    }

    /// `CODE_MOCK_COMMAND`: one real run_command through the core's own tool
    /// (approvals off), then a short reply. Stderr gets when the tool's
    /// result was handed back, which is when a model could react to it.
    fn command_turn(&mut self, command: &str) -> Result<(), Stop> {
        self.stream(0, "Running it.\n\n", 90.0)?;
        let mut cfg = self.config.clone();
        cfg.approval = ApprovalMode::Auto;
        cfg.session_dir = None;
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        let eng = self.engine.get_or_insert_with(|| crate::engine::Engine::tools_only(cfg, tx, wake));
        let notes = eng.take_run_notes();
        if !notes.is_empty() {
            eprintln!("code mock: notes for the model's turn:\n{notes}");
        }
        let started = std::time::Instant::now();
        eprintln!("code mock: run_command {command:?} started");
        let (text, is_error) = eng.run_tool(0, "run_command", &[("command".to_string(), command.to_string())]);
        eprintln!("code mock: tool result after {:.2} s (error: {is_error}):\n{text}\ncode mock: end of tool result", started.elapsed().as_secs_f32());
        self.stream(0, "Done.", 90.0)
    }

    fn presentation_turn(&mut self, prompt: &str) -> Result<(), Stop> {
        self.context += (prompt.len() / 4) as u32;
        self.emit(AgentEvent::Todos(vec![
            Todo { text: "inspect the layout".into(), state: TodoState::Active },
            Todo { text: "preview the change".into(), state: TodoState::Pending },
        ]));
        self.emit(AgentEvent::Thinking { lane: 0, text: "Checking the label and spacing in the example.".into() });
        self.pause(450)?;
        // These are event cards only. No file arguments, command execution,
        // edit previews, session store or activity logger participate.
        self.tool(1, "inspect_layout", "inspect the button layout", &[], 650, "one control found", "The label needs more room.")?;
        self.stream(0, "The label is wider than its button. I will give it room and keep the controls aligned.\n\n", 65.0)?;
        self.tool(2, "preview_layout", "preview the wider button", &[], 700, "preview ready", "The controls remain aligned at both window sizes.")?;
        self.emit(AgentEvent::Todos(vec![
            Todo { text: "inspect the layout".into(), state: TodoState::Done },
            Todo { text: "preview the change".into(), state: TodoState::Done },
        ]));
        self.stream(0, "The button now has room for its label. The preview is ready to review.", 65.0)?;
        self.usage(0.0, &self.segments());
        Ok(())
    }
}

const REPLY: &str = "The request flows through `handle_event` in `apps/code/src/main.rs:12`, which matches on the event kind and forwards to the widget tree. Two things stand out:\n\n1. **Draw is cheap.** Nothing redraws unless an action arrives.\n2. **State lives in one struct**, so the change is small.\n\nHere is the shape of the fix:\n\n```rust\nfn handle_event(&mut self, cx: &mut Cx, event: &Event) {\n    if let Event::Signal = event {\n        for ev in self.port.poll() {\n            self.transcript.apply(ev);\n        }\n        self.redraw(cx);\n    }\n    self.ui.handle_event(cx, event, &mut Scope::empty());\n}\n```\n\nThe signal arrives only when the agent has something to say, so an idle window stays idle (see apps/code/src/ui/view.rs#L713 for the drain). Want me to apply it?\n";
