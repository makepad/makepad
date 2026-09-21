use crate::{
    pipeline::{self, Command},
    process::Control,
    report::{self, Branch, ScriptState, Update},
    wall::{CiWallWidgetRefExt, Tile},
};
use makepad_widgets::*;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{atomic::Ordering, mpsc, Arc},
    time::Instant,
};
app_main!(App);
pub fn run() {
    crate::window_geometry::initialize(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    main();
}
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Makepad CI"
                window.position: vec2(#(crate::window_geometry::geometry().x), #(crate::window_geometry::geometry().y))
                window.inner_size: vec2(#(crate::window_geometry::geometry().width), #(crate::window_geometry::geometry().height))
                body +: {
                    padding: 0 spacing: 0
                    // An OLED shows this all day: near-black, grey text, and
                    // nothing bright but a failed tile.
                    SolidView{
                        width: Fill height: Fill flow: Down padding: 12 spacing: 8
                        draw_bg.color: #050607
                        controls := View{
                            width: Fill height: Fit spacing: 10 align: Align{y: 0.5}
                            run := Button{text: "Run now"}
                            stop := Button{text: "Stop"}
                            install := Button{text: "Install model (accept license)" visible: false}
                            status := Label{width: Fill height: Fit text: "Starting watcher" draw_text +: {color: #7d8590 text_style +: {font_size: 11}}}
                        }
                        branches := Label{width: Fill height: Fit text: "work — waiting" draw_text +: {color: #aab1bb text_style +: {font_size: 16}}}
                        progress := Label{width: Fill height: Fit text: "" draw_text +: {color: #8e96a1 text_style +: {font_size: 13}}}
                        wall := CiWall{}
                        detail := View{
                            width: Fill height: 290 flow: Right spacing: 12
                            View{
                                width: Fill height: Fill flow: Down spacing: 6
                                selected := Label{width: Fill height: Fit text: "Waiting for scripts" draw_text +: {color: #c9ced6 text_style +: {font_size: 17}}}
                                ScrollYView{
                                    width: Fill height: Fill
                                    steps := Label{width: Fill height: Fit text: "" draw_text +: {color: #8e96a1 text_style +: {font_size: 11}}}
                                }
                            }
                            View{
                                width: 400 height: Fill flow: Down spacing: 6
                                grab := Image{width: Fill height: 150 fit: ImageFit.Smallest}
                                View{
                                    width: Fill height: Fit spacing: 8 align: Align{y: 0.5}
                                    previous := Button{text: "Previous grab"}
                                    next := Button{text: "Next grab"}
                                    grab_path := Label{width: Fill height: Fit text: "No grabs yet" draw_text +: {color: #7d8590 text_style +: {font_size: 10}}}
                                }
                                model := Label{width: Fill height: Fit text: "Model idle" draw_text +: {color: #7d8590 text_style +: {font_size: 10}}}
                                SolidView{
                                    width: Fill height: Fill draw_bg.color: #0b0d10 padding: 8
                                    ScrollYView{
                                        width: Fill height: Fill
                                        log := Label{width: Fill height: Fit text: "" draw_text +: {color: #7d8590 text_style +: {font_size: 9}}}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
struct Worker {
    sender: mpsc::SyncSender<Command>,
    receiver: mpsc::Receiver<Update>,
    control: Control,
}
impl Worker {
    fn start(cx: &Cx) -> Result<Self, String> {
        let (sender, commands) = mpsc::sync_channel(2);
        let (updates, receiver) = mpsc::channel();
        let control = Control::default();
        let worker_control = control.clone();
        cx.thread_spawner()
            .spawn_worker(Default::default(), move || {
                let notify: report::Notify = Arc::new(move |update| {
                    let _ = updates.send(update);
                    SignalToUI::set_ui_signal();
                });
                let result = (|| {
                    let base = std::env::current_dir().map_err(|e| e.to_string())?;
                    let config = crate::Options::parse()?.config(&base, &worker_control)?;
                    let installed = config.no_vision || crate::uihub::model_installed(&config.model);
                    notify(Update::Model(if config.no_vision {
                        "vision off (--no-vision)".into()
                    } else if installed {
                        format!("{}: installed", config.model)
                    } else {
                        format!("{}: NOT installed, vision checks are skipped", config.model)
                    }));
                    notify(Update::ModelInstalled(installed));
                    pipeline::watch_loop(base, config, commands, worker_control, notify.clone())
                })();
                if let Err(e) = result {
                    notify(Update::Failed(e));
                }
                notify(Update::Exited);
            })
            .map_err(|e| format!("worker startup: {e:?}"))?
            .detach();
        Ok(Self {
            sender,
            receiver,
            control,
        })
    }
}
#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    worker: Option<Worker>,
    #[rust]
    active: bool,
    #[rust]
    quitting: bool,
    #[rust]
    pending: Option<Command>,
    #[rust]
    log_text: String,
    #[rust]
    branches: Vec<Branch>,
    #[rust]
    scripts: BTreeMap<String, ScriptState>,
    #[rust]
    selected: Option<String>,
    #[rust]
    pinned: bool,
    #[rust]
    timer: Timer,
    #[rust]
    images: BTreeMap<PathBuf, Arc<Vec<u8>>>,
    #[rust]
    image_index: usize,
    #[rust]
    shown: Option<PathBuf>,
}
/// What a tile is called: `apps/wm` is `wm`, the root script is `workspace`.
fn short_name(name: &str) -> String {
    match name {
        "." | "" => "workspace".into(),
        name => name.strip_prefix("apps/").unwrap_or(name).into(),
    }
}
fn key(branch: &str, name: &str) -> String {
    format!("{branch}\n{name}")
}
impl App {
    fn log(&mut self, cx: &mut Cx, line: &str) {
        self.log_text.push_str(line);
        self.log_text.push('\n');
        report::trim_log(&mut self.log_text);
        if self.selected.is_none() {
            self.ui.label(cx, ids!(log)).set_text(cx, &self.log_text);
        }
    }
    fn wall(&self, cx: &mut Cx) {
        let multiple = self.branches.len() > 1;
        let tiles = self
            .scripts
            .iter()
            .map(|(key, state)| Tile {
                key: key.clone(),
                label: if multiple {
                    format!("{} {}", key.split('\n').next().unwrap_or(""), short_name(&state.name))
                } else {
                    short_name(&state.name)
                },
                state: state.clone(),
            })
            .collect();
        self.ui
            .ci_wall(cx, ids!(wall))
            .set_tiles(cx, tiles, self.selected.clone());
    }
    fn choose(&mut self, candidate: Option<String>) {
        if self.pinned {
            return;
        }
        let red = |s: &ScriptState| s.color_verdict() == "red";
        if let Some(k) = candidate {
            if self.scripts.get(&k).is_some_and(red) {
                if self.selected.as_ref() != Some(&k) {
                    self.image_index = 0;
                    self.shown = None;
                }
                self.selected = Some(k);
                return;
            }
        }
        let selected_red = self
            .selected
            .as_ref()
            .and_then(|k| self.scripts.get(k))
            .is_some_and(red);
        if !selected_red {
            let next = self
                .scripts
                .iter()
                .filter(|(_, s)| red(s))
                .max_by_key(|(_, s)| s.failed_at)
                .or_else(|| self.scripts.iter().find(|(_, s)| s.verdict == "running"))
                .or_else(|| self.scripts.iter().next())
                .map(|(k, _)| k.clone());
            if self.selected != next {
                self.image_index = 0;
                self.shown = None;
            }
            self.selected = next;
        }
    }
    /// How far the run is, in words: what is done, what failed, what runs now.
    fn progress_line(&self) -> String {
        let total = self.scripts.len();
        let count = |v: &str| self.scripts.values().filter(|s| s.color_verdict() == v).count();
        let (failed, warned, passed) = (count("red"), count("orange"), count("green"));
        let done = failed + warned + passed;
        if total == 0 {
            return String::new();
        }
        let mut line = format!("{done} of {total} tested  ·  {passed} passed  ·  {warned} warnings  ·  {failed} failed");
        for s in self.scripts.values().filter(|s| s.verdict == "running") {
            let step = s.steps.iter().rev().find(|s| s.state == "running")
                .map(|s| s.name.rsplit(" / ").next().unwrap_or(&s.name).to_string())
                .unwrap_or_else(|| "starting".into());
            let secs = s.started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            line.push_str(&format!("\nnow: {} — {step}  ({}m{:02}s)", short_name(&s.name), secs / 60, secs % 60));
        }
        line
    }
    fn refresh(&mut self, cx: &mut Cx) {
        let branches = self
            .branches
            .iter()
            .map(|b| {
                let running = self.scripts.iter().any(|(k, s)| {
                    s.verdict == "running" && k.split('\n').next() == Some(b.name.as_str())
                });
                let words = if running {
                    "testing now".to_string()
                } else {
                    let verdict = match b.verdict.as_str() {
                        "green" => "passing",
                        "orange" => "passing with warnings",
                        "red" => "FAILING",
                        "running" => "testing now",
                        _ => "not tested yet",
                    };
                    match report::now().saturating_sub(b.finished) {
                        _ if b.finished == 0 => verdict.into(),
                        s if s < 120 => format!("{verdict}  ·  tested {s}s ago"),
                        s if s < 7200 => format!("{verdict}  ·  tested {} min ago", s / 60),
                        s => format!("{verdict}  ·  tested {} h ago", s / 3600),
                    }
                };
                format!("{}  {}  ·  {words}", b.name, &b.tip[..b.tip.len().min(10)])
            })
            .collect::<Vec<_>>()
            .join("    ");
        self.ui.label(cx, ids!(branches)).set_text(cx, &branches);
        self.ui.label(cx, ids!(progress)).set_text(cx, &self.progress_line());
        let Some(state) = self.selected.as_ref().and_then(|k| self.scripts.get(k)) else {
            return;
        };
        self.ui
            .label(cx, ids!(selected))
            .set_text(cx, &format!("{} — {}", state.name, match state.color_verdict() {
                "green" => "passed",
                "orange" => "passed with warnings",
                "red" => "FAILED",
                "running" => "testing now",
                _ => "not tested yet",
            }));
        let steps = state
            .steps
            .iter()
            .map(|s| {
                let seconds = if s.state == "running" {
                    s.started
                        .map(|t| t.elapsed().as_secs_f64())
                        .unwrap_or(s.seconds)
                } else {
                    s.seconds
                };
                // A failed or warned step says why; a passed one is one line.
                let mut block = format!(
                    "{} — {} ({seconds:.1}s)",
                    s.name.rsplit(" / ").next().unwrap_or(&s.name),
                    s.state
                );
                if matches!(s.state.as_str(), "failed" | "warning" | "running") {
                    for extra in [&s.command, &s.detail] {
                        if !extra.is_empty() {
                            block.push('\n');
                            block.push_str(extra);
                        }
                    }
                }
                block
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.ui.label(cx, ids!(steps)).set_text(cx, &steps);
        self.ui.label(cx, ids!(log)).set_text(cx, &state.log);
        let path = if state.grabs.is_empty() {
            None
        } else {
            self.image_index = self.image_index.min(state.grabs.len() - 1);
            Some(PathBuf::from(&state.grabs[self.image_index]))
        };
        if path != self.shown {
            self.ui.image(cx, ids!(grab)).set_texture(cx, None);
            self.shown = path.clone();
            if let Some(path) = path {
                self.ui
                    .label(cx, ids!(grab_path))
                    .set_text(cx, &path.file_name().unwrap_or_default().to_string_lossy());
                if let Some(bytes) = self.images.get(&path) {
                    if let Err(e) = self.ui.image(cx, ids!(grab)).load_png_from_data(cx, bytes) {
                        self.log(cx, &format!("grab decode: {e:?}"));
                    }
                } else {
                    self.pending = Some(Command::LoadGrab(path));
                    self.dispatch(cx);
                }
            } else {
                self.ui.label(cx, ids!(grab_path)).set_text(cx, "No grabs yet");
            }
        }
    }
    fn scoped(&mut self, cx: &mut Cx, branch: String, name: String, update: Update) -> bool {
        let k = key(&branch, &name);
        let state = self
            .scripts
            .entry(k.clone())
            .or_insert_with(|| ScriptState::waiting(&name));
        let old_failure_stamp = state.failed_at;
        let mut wall = true;
        match update {
            Update::Begin(_, _) => {
                state.verdict = "running".into();
                state.started = Some(Instant::now());
                state.steps.clear();
                state.log.clear();
                state.grabs.clear();
            }
            Update::Stages(steps) => {
                if steps.iter().filter(|s| s.state == "failed").count() > state.counts().2 {
                    state.failed_at = report::stamp();
                }
                state.steps = steps;
                state.detail = state
                    .steps
                    .iter()
                    .find(|s| s.state == "failed")
                    .or_else(|| state.steps.iter().find(|s| s.state == "warning"))
                    .map(|s| s.detail.clone())
                    .unwrap_or_default();
            }
            Update::Failed(detail) => {
                state.detail = detail;
            }
            Update::Log(s) => {
                state.log.push_str(&s);
                state.log.push('\n');
                report::trim_log(&mut state.log);
                wall = false;
            }
            Update::Grab(path, bytes) => {
                let p = path.display().to_string();
                if !state.grabs.contains(&p) {
                    state.grabs.push(p);
                }
                self.images.insert(path, bytes);
                if self.selected.as_ref() == Some(&k) {
                    self.image_index = state.grabs.len() - 1;
                    self.shown = None;
                }
                wall = false;
            }
            Update::Done(passed, _) => {
                state.seconds = state
                    .started
                    .map(|s| s.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                state.verdict = report::verdict(&state.steps, !passed).into();
            }
            Update::Model(s) => {
                self.ui.label(cx, ids!(model)).set_text(cx, &s);
                wall = false;
            }
            _ => {}
        }
        let newest = (state.failed_at != old_failure_stamp).then_some(k);
        self.choose(newest);
        wall
    }
    fn drain(&mut self, cx: &mut Cx) {
        let mut wall = false;
        let mut count = 0;
        for _ in 0..256 {
            let Some(update) = self
                .worker
                .as_ref()
                .and_then(|w| w.receiver.try_recv().ok())
            else {
                break;
            };
            count += 1;
            match update {
                Update::Script(branch, name, update) => {
                    wall |= self.scoped(cx, branch, name, *update)
                }
                Update::Scripts(branch, states) => {
                    let names: Vec<_> = states.iter().map(|s| key(&branch, &s.name)).collect();
                    self.scripts
                        .retain(|k, _| !k.starts_with(&format!("{branch}\n")) || names.contains(k));
                    for state in states {
                        let k = key(&branch, &state.name);
                        if state.verdict != "waiting"
                            || !self.scripts.get(&k).is_some_and(|s| s.started.is_some())
                        {
                            self.scripts.insert(k, state);
                        }
                    }
                    self.choose(None);
                    wall = true;
                }
                Update::Branches(branches) => {
                    for b in &branches {
                        for s in &b.scripts {
                            self.scripts.insert(key(&b.name, &s.name), s.clone());
                        }
                    }
                    self.branches = branches;
                    self.choose(None);
                    wall = true;
                }
                Update::Begin(branch, tip) => {
                    self.active = true;
                    self.ui.label(cx, ids!(status)).set_text(
                        cx,
                        &format!("Running {branch} {}", &tip[..tip.len().min(10)]),
                    );
                    cx.stop_timer(self.timer);
                    self.timer = cx.start_interval(1.0);
                }
                Update::Stages(stages) => {
                    if let Some(s) = stages.iter().find(|s| s.state == "running") {
                        self.ui.label(cx, ids!(status)).set_text(cx, &s.name);
                    }
                }
                Update::Log(s) => self.log(cx, &s),
                Update::Failed(s) => {
                    // One line in the header; the whole text is in the log.
                    let first: String = s.lines().next().unwrap_or("").chars().take(110).collect();
                    self.ui
                        .label(cx, ids!(status))
                        .set_text(cx, &format!("Problem: {first}"));
                    self.log(cx, &s);
                }
                Update::Model(s) => self.ui.label(cx, ids!(model)).set_text(cx, &s),
                Update::ModelInstalled(installed) => {
                    self.ui.widget(cx, ids!(install)).set_visible(cx, !installed);
                }
                Update::Grab(path, bytes) => {
                    if self.shown.as_ref() == Some(&path) {
                        self.shown = None;
                    }
                    self.images.insert(path, bytes);
                }
                Update::Done(passed, path) => {
                    self.active = false;
                    cx.stop_timer(self.timer);
                    self.ui
                        .label(cx, ids!(status))
                        .set_text(cx, if passed { "Run complete" } else { "Run FAILED" });
                    self.log(cx, &format!("Evidence: {}", path.display()));
                }
                Update::Idle => {}
                Update::Exited => {
                    self.worker = None;
                    cx.stop_timer(self.timer);
                    if self.quitting {
                        cx.quit();
                    } else {
                        self.log(cx, "Watcher exited");
                    }
                }
            }
        }
        if count == 256 {
            SignalToUI::set_ui_signal();
        }
        if wall {
            self.wall(cx);
        }
        self.refresh(cx);
    }
    fn dispatch(&mut self, cx: &mut Cx) {
        let Some(command) = self.pending.take() else {
            return;
        };
        let Some(worker) = &self.worker else {
            return;
        };
        match worker.sender.try_send(command) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(command)) => {
                self.pending = Some(command);
                self.log(cx, "Worker queue full; retrying on a subsequent frame");
                if !self.active {
                    self.timer = cx.start_timeout(0.1);
                }
            }
            Err(mpsc::TrySendError::Disconnected(_)) => self.log(cx, "Worker unavailable"),
        }
    }
    fn stop(&self) {
        if let Some(w) = &self.worker {
            w.control.stop.store(true, Ordering::Release);
            w.control.model_cancel.cancel();
        }
    }
}
impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        match Worker::start(cx) {
            Ok(w) => self.worker = Some(w),
            Err(e) => self.log(cx, &e),
        }
    }
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(run)).clicked(actions) {
            self.pending = Some(Command::RunNow);
            self.dispatch(cx);
        }
        if self.ui.button(cx, ids!(install)).clicked(actions) {
            self.pending = Some(Command::Install);
            self.dispatch(cx);
        }
        if self.ui.button(cx, ids!(stop)).clicked(actions) {
            self.stop();
        }
        if self.ui.button(cx, ids!(previous)).clicked(actions) {
            self.image_index = self.image_index.saturating_sub(1);
            self.shown = None;
            self.refresh(cx);
        }
        if self.ui.button(cx, ids!(next)).clicked(actions) {
            self.image_index += 1;
            self.shown = None;
            self.refresh(cx);
        }
    }
}
impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        crate::wall::script_mod(vm);
        self::script_mod(vm)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.worker.is_some() {
            let close = match event {
                Event::WindowCloseRequested(ev) => {
                    ev.accept_close.set(false);
                    true
                }
                Event::QuitRequested(ev) => {
                    ev.handle();
                    true
                }
                _ => false,
            };
            if close {
                self.quitting = true;
                self.stop();
                self.worker
                    .as_ref()
                    .unwrap()
                    .control
                    .shutdown
                    .store(true, Ordering::Release);
                return;
            }
        }
        if matches!(event, Event::Signal) {
            self.drain(cx);
            self.dispatch(cx);
        }
        if self.timer.is_event(event).is_some() {
            self.refresh(cx);
            self.dispatch(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        let selected = self.ui.ci_wall(cx, ids!(wall)).selected();
        if selected != self.selected && selected.is_some() {
            self.selected = selected;
            self.pinned = true;
            self.image_index = 0;
            self.shown = None;
            self.refresh(cx);
        }
    }
}
