use crate::{
    pipeline::{self, Command},
    process::Control,
    report::{self, Branch, ScriptState, Update},
    wall::{BranchLine, CiBranchesWidgetRefExt, CiStepsWidgetRefExt, CiWallWidgetRefExt, Tile},
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
    let QuietLabel = Label{
        padding: 0
        draw_text +: {color: #x90969e text_style: theme.font_regular{font_size: 11}}
    }
    let SingleLine = QuietLabel{max_lines: 1 text_overflow: TextOverflow.Ellipsis}
    let QuietButton = Button{
        margin: 0 padding: Inset{left: 11 right: 11 top: 7 bottom: 7}
        draw_text +: {color: #x989da4 color_hover: #b0b4ba color_focus: #a0a5ac color_down: #b0b4ba text_style +: {font_size: 10}}
        draw_bg +: {
            color: #x1b1e22 color_hover: #x272b30 color_focus: #x22262b color_down: #x30343a
            color_2: vec4(-1) color_2_hover: vec4(-1) color_2_focus: vec4(-1) color_2_down: vec4(-1)
            border_color: #x30353a border_color_hover: #x474c52 border_color_focus: #x555a61 border_color_down: #x555a61
            border_radius: 5.0 border_size: 1.0
        }
    }
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Makepad CI"
                window.position: vec2(#(crate::window_geometry::geometry().x), #(crate::window_geometry::geometry().y))
                window.inner_size: vec2(#(crate::window_geometry::geometry().width), #(crate::window_geometry::geometry().height))
                caption_bar +: {draw_bg.color: #x0b0d10}
                body +: {
                    padding: 0 spacing: 0
                    SolidView{
                        width: Fill height: Fill flow: Down padding: 18 spacing: 12
                        draw_bg.color: #x050607
                        header := View{
                            width: Fill height: Fit flow: Down spacing: 8
                            View{
                                width: Fill height: Fit spacing: 20 align: Align{y: 0.5}
                                branches := CiBranches{}
                                View{
                                    width: Fit height: Fit spacing: 10
                                    run := QuietButton{text: "Run now"}
                                    stop := QuietButton{text: "Stop"}
                                }
                            }
                            run_progress := SolidView{
                                width: Fill height: 2
                                draw_bg +: {
                                    progress: instance(-1.0)
                                    pixel: fn(){if self.pos.x <= self.progress {return #x737d87}; return #x20252a}
                                }
                            }
                        }
                        content := View{
                            width: Fill height: Fill flow: Down spacing: 12
                            wall := CiWall{}
                            detail := RoundedView{
                                width: Fill height: 224 flow: Down padding: 16 spacing: 12
                                draw_bg +: {color: #x0e1115 border_color: #x292e35 border_size: 1.0 border_radius: 8.0}
                                View{
                                    width: Fill height: Fit spacing: 12 align: Align{y: 0.5}
                                    selected := QuietLabel{width: Fill flow: Right{wrap: true} text: "Waiting for scripts" draw_text +: {color: #b9c0c9 text_style: theme.font_bold{font_size: 16}}}
                                    selected_state := QuietLabel{text: "" draw_text.color: #x939ca7}
                                }
                                detail_body := View{
                                    width: Fill height: Fill spacing: 20
                                    step_column := View{
                                        width: Fill height: Fill flow: Down spacing: 8
                                        QuietLabel{text: "STEPS" draw_text +: {color: #x69737e text_style: theme.font_bold{font_size: 9}}}
                                        history := QuietLabel{width: Fill visible: false text: "" draw_text.color: #x69737e}
                                        steps := CiSteps{}
                                    }
                                    evidence_column := View{
                                        width: 350 height: Fill flow: Down spacing: 8
                                        View{
                                            width: Fill height: Fit spacing: 8 align: Align{y: 0.5}
                                            QuietLabel{text: "CAPTURES" draw_text +: {color: #x69737e text_style: theme.font_bold{font_size: 9}}}
                                            View{width: Fill}
                                            previous := QuietButton{text: "Previous"}
                                            next := QuietButton{text: "Next"}
                                            grab_path := QuietLabel{text: "0 / 0"}
                                        }
                                        capture_frame := View{
                                            width: Fill height: Fit align: Align{x: 0.5}
                                            grab := Image{width: Fill height: 132 fit: ImageFit.Smallest visible: false}
                                        }
                                        no_grab := QuietLabel{width: Fill text: "No captures recorded for this script." draw_text.color: #x626c78}
                                        QuietLabel{text: "LOG TAIL" draw_text +: {color: #x69737e text_style: theme.font_bold{font_size: 9}}}
                                        ScrollYView{
                                            width: Fill height: Fill
                                            log := QuietLabel{width: Fill text: "No log output recorded." draw_text +: {color: #x707b87 text_style: theme.font_code{font_size: 9}}}
                                        }
                                    }
                                }
                            }
                        }
                        View{
                            width: Fill height: Fit spacing: 10 align: Align{y: 0.5}
                            footer := SingleLine{width: Fill text: "Watcher starting" draw_text +: {color: #x535f6b text_style: theme.font_code{font_size: 9}}}
                            install := QuietButton{text: "Install model (accept license)" visible: false}
                        }
                    }
                }
            }
        }
    }
}

struct DashboardMeta {
    poll_secs: u64,
    poll_anchor: Instant,
    base: PathBuf,
    targets: usize,
    model: String,
}
struct Worker {
    metadata: mpsc::Receiver<DashboardMeta>,
    sender: mpsc::SyncSender<Command>,
    receiver: mpsc::Receiver<Update>,
    control: Control,
}
impl Worker {
    fn start(cx: &Cx) -> Result<Self, String> {
        let (sender, commands) = mpsc::sync_channel(2);
        let (meta_sender, metadata) = mpsc::sync_channel(1);
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
                    let _ = meta_sender.try_send(DashboardMeta {
                        poll_secs: config.poll_secs,
                        poll_anchor: Instant::now(),
                        base: base.clone(),
                        targets: config.targets.len(),
                        model: config.model.clone(),
                    });
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
            metadata,
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
    metadata: Option<DashboardMeta>,
    #[rust]
    poll_timer: Timer,
    #[rust]
    window_width: f64,
    #[rust]
    layout_signature: Option<(bool, bool)>,
    #[rust]
    run_directory: Option<PathBuf>,
    #[rust]
    active: bool,
    #[rust]
    run_started: Option<Instant>,
    #[rust]
    run_branch: String,
    #[rust]
    model_status: String,
    #[rust]
    watcher_problem: String,
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
        name => {
            let name = name.strip_prefix("apps/").unwrap_or(name);
            if name.contains('/') {
                let mut parts = name.rsplit('/');
                let leaf = parts.next().unwrap_or(name);
                format!("{}/{leaf}", parts.next().unwrap_or(""))
            } else { name.into() }
        },
    }
}
fn key(branch: &str, name: &str) -> String {
    format!("{branch}\n{name}")
}
impl App {
    fn adapt(&mut self, cx: &mut Cx, width: f64) {
        self.window_width = width;
        let captures = self.selected.as_ref().and_then(|k|self.scripts.get(k)).is_some_and(|s|!s.grabs.is_empty());
        let signature = (width >= 1250.0, captures);
        if self.layout_signature == Some(signature) {return;}
        self.layout_signature = Some(signature);
        if width >= 1250.0 {
            let mut widget = self.ui.widget(cx, ids!(content));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; flow: Right});
            let mut widget = self.ui.widget(cx, ids!(detail));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; width: 420 height: Fill});
            let mut widget = self.ui.widget(cx, ids!(detail_body));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; flow: Down});
            let mut widget = self.ui.widget(cx, ids!(step_column));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; width: Fill height: 240});
            let mut widget = self.ui.widget(cx, ids!(evidence_column));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; width: Fill height: Fill});
            let mut widget = self.ui.widget(cx, ids!(grab));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; height: 220});
        } else {
            let mut widget = self.ui.widget(cx, ids!(content));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; flow: Down});
            let mut widget = self.ui.widget(cx, ids!(detail));
            let height = if captures {352.0} else {224.0};
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; width: Fill height: #(height)});
            let mut widget = self.ui.widget(cx, ids!(detail_body));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; flow: Right});
            let mut widget = self.ui.widget(cx, ids!(step_column));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; width: Fill height: Fill});
            let mut widget = self.ui.widget(cx, ids!(evidence_column));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; width: 350 height: Fill});
            let mut widget = self.ui.widget(cx, ids!(grab));
            script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; height: 132});
        }
    }
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
        let mut tiles: Vec<Tile> = self
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
        let running = self.active || self.scripts.values().any(|s| s.verdict == "running");
        let priority = |tile: &Tile| if running { 0 } else { match tile.state.color_verdict() {
            "red" => 0, "orange" => 1, "running" => 2, "green" => 3, _ => 4,
        }};
        tiles.sort_by(|a, b| (priority(a), a.state.name != ".", &a.key).cmp(&(priority(b), b.state.name != ".", &b.key)));
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
    fn refresh(&mut self, cx: &mut Cx) {
        self.adapt(cx, self.window_width);
        let branches = self.branches.iter().map(|b| {
            let scripts = self.scripts.iter().filter(|(k, _)| k.split('\n').next() == Some(b.name.as_str()));
            let (mut total, mut done, mut failed, mut warned) = (0, 0, 0, 0);
            for (_, script) in scripts {
                total += 1;
                match script.color_verdict() {
                    "red" => { done += 1; failed += 1; }
                    "orange" => { done += 1; warned += 1; }
                    "green" => done += 1,
                    _ => {}
                }
            }
            let running = self.active && self.run_branch == b.name;
            let state = if running { "" } else { match b.verdict.as_str() {
                "green" | "orange" => "passing", "red" => "FAILING", _ => "not tested yet",
            }};
            let age = if running {
                let seconds = self.run_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                if seconds >= 60 { format!("{}m {:02}s", seconds / 60, seconds % 60) } else { format!("{seconds}s") }
            } else { match report::now().saturating_sub(b.finished) {
                _ if b.finished == 0 => String::new(),
                seconds if seconds < 60 => "just now".into(),
                seconds if seconds < 7200 => format!("{} min ago", seconds / 60),
                seconds => format!("{} hr ago", seconds / 3600),
            }};
            BranchLine {
                name: b.name.clone(),
                tip: if b.tip.is_empty() { "no tip".into() } else { b.tip.chars().take(8).collect() },
                progress: format!("{done} / {total}"), failed, warned,
                state: state.into(), age,
            }
        }).collect();
        self.ui.ci_branches(cx, ids!(branches)).set_branches(cx, branches, self.watcher_problem.clone());
        let done = self.scripts.values().filter(|s| matches!(s.color_verdict(), "green" | "orange" | "red")).count();
        let fraction = if self.active { done as f64 / self.scripts.len().max(1) as f64 } else { -1.0 };
        let mut widget = self.ui.widget(cx, ids!(run_progress));
        script_apply_eval!(cx, widget, {use mod.prelude.widgets.*; draw_bg +: {progress: #(fraction)}});
        if let Some(meta) = &self.metadata {
            let period = meta.poll_secs.max(1);
            let left = period - meta.poll_anchor.elapsed().as_secs() % period;
            let next = if left >= 60 {format!("next poll ≈ {}m", left.div_ceil(60))} else {format!("next poll ≈ {left}s")};
            let tip = self.branches.first().map(|b| b.tip.chars().take(8).collect::<String>()).filter(|s| !s.is_empty()).unwrap_or_else(|| "pending".into());
            let run = self.run_directory.as_ref().map(|p|p.strip_prefix(&meta.base).unwrap_or(p).display().to_string()).unwrap_or_else(|| if self.active {"in progress".into()} else {"no record".into()});
            let model = if self.model_status.starts_with("vision off") {
                format!("{} · vision off", meta.model)
            } else if self.model_status.is_empty() {
                format!("{} · idle", meta.model)
            } else if self.model_status.starts_with(&meta.model) {
                self.model_status.clone()
            } else {
                format!("{} · {}", meta.model, self.model_status)
            };
            self.ui.label(cx, ids!(footer)).set_text(cx, &format!("checkout {tip}  ·  {} target{}  ·  {next}  ·  {model}  ·  run: {run}", meta.targets, if meta.targets == 1 {""} else {"s"}));
        }
        let Some(state) = self.selected.as_ref().and_then(|k| self.scripts.get(k)) else {
            return;
        };
        self.ui.label(cx, ids!(selected)).set_text(cx, if state.name=="." {"workspace"} else {&state.name});
        let verdict = match state.color_verdict() {
            "green" => "Passed", "orange" => "Warnings", "red" => "FAILED", "running" => "Testing now", _ => "Not tested",
        };
        let seconds = state.started.filter(|_| state.verdict=="running").map(|t|t.elapsed().as_secs_f64()).unwrap_or(state.seconds);
        self.ui.label(cx, ids!(selected_state)).set_text(cx, &format!("{verdict}  ·  {seconds:.1}s"));
        let history = if state.color_verdict()=="waiting" {match state.previous.as_str(){
            "red" => "Last finished run failed; this run is untested.",
            "orange" => "Last finished run had warnings; this run is untested.",
            "green" => "Last finished run passed; this run is untested.",
            _ => "",
        }} else {""};
        self.ui.widget(cx, ids!(history)).set_visible(cx, !history.is_empty());
        self.ui.label(cx, ids!(history)).set_text(cx, history);
        self.ui.ci_steps(cx, ids!(steps)).set_steps(cx, state.steps.clone());
        let log = state.log.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        self.ui.label(cx, ids!(log)).set_text(cx, if log.is_empty() {"No log output recorded."} else {&log});
        self.ui.widget(cx, ids!(grab)).set_visible(cx, !state.grabs.is_empty());
        self.ui.widget(cx, ids!(no_grab)).set_visible(cx, state.grabs.is_empty());
        self.ui.button(cx, ids!(previous)).set_enabled(cx, !state.grabs.is_empty() && self.image_index > 0);
        self.ui.button(cx, ids!(next)).set_enabled(cx, self.image_index + 1 < state.grabs.len());
        self.ui.label(cx, ids!(grab_path)).set_text(cx, &format!("{} / {}", if state.grabs.is_empty() {0} else {self.image_index + 1}, state.grabs.len()));
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
                if let Some(bytes) = self.images.get(&path) {
                    if let Err(e) = self.ui.image(cx, ids!(grab)).load_png_from_data(cx, bytes) {
                        self.log(cx, &format!("grab decode: {e:?}"));
                    }
                } else {
                    self.pending = Some(Command::LoadGrab(path));
                    self.dispatch(cx);
                }
            } else {

            }
        }
    }
    fn scoped(&mut self, branch: String, name: String, update: Update) -> bool {
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
                self.model_status = s;
                wall = false;
            }
            _ => {}
        }
        let newest = (state.failed_at != old_failure_stamp).then_some(k);
        self.choose(newest);
        wall
    }
    fn drain(&mut self, cx: &mut Cx) {
        if let Some(meta) = self.worker.as_ref().and_then(|w|w.metadata.try_recv().ok()) {
            cx.stop_timer(self.poll_timer);
            self.poll_timer = cx.start_interval(if meta.poll_secs >= 120 {60.0} else {1.0});
            self.metadata = Some(meta);
        }
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
                    wall |= self.scoped(branch, name, *update)
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
                    self.run_directory = None;
                    if let Some(b) = self.branches.iter_mut().find(|b|b.name==branch) {b.tip=tip.clone();}
                    self.run_started = Some(Instant::now());
                    self.run_branch = branch;
                    self.watcher_problem.clear();
                    cx.stop_timer(self.timer);
                    self.timer = cx.start_interval(1.0);
                }
                Update::Stages(_) => {}
                Update::Log(s) => self.log(cx, &s),
                Update::Failed(s) => {
                    // One line in the header; the whole text is in the log.
                    let first: String = s.lines().next().unwrap_or("").chars().take(110).collect();
                    self.watcher_problem = if first.contains("does not appear to be a git repository") {
                        "Watcher · remote unavailable".into()
                    } else {format!("Watcher · {first}")};
                    self.log(cx, &s);
                }
                Update::Model(s) => self.model_status = s,
                Update::ModelInstalled(installed) => {
                    self.ui.widget(cx, ids!(install)).set_visible(cx, !installed);
                }
                Update::Grab(path, bytes) => {
                    if self.shown.as_ref() == Some(&path) {
                        self.shown = None;
                    }
                    self.images.insert(path, bytes);
                }
                Update::Done(_, path) => {
                    self.run_directory = Some(path.clone());
                    self.active = false;
                    cx.stop_timer(self.timer);
                    self.log(cx, &format!("Evidence: {}", path.display()));
                }
                Update::Idle => {
                    self.active = false;
                    cx.stop_timer(self.timer);
                    wall = true;
                }
                Update::Exited => {
                    self.worker = None;
                    cx.stop_timer(self.timer);
                    cx.stop_timer(self.poll_timer);
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
        self.adapt(cx, crate::window_geometry::geometry().width);
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
        if let Event::WindowGeomChange(ev) = event {
            self.adapt(cx, ev.new_geom.inner_size.x);
        }
        if matches!(event, Event::Signal) {
            self.drain(cx);
            self.dispatch(cx);
        }
        if self.poll_timer.is_event(event).is_some() {
            self.refresh(cx);
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
