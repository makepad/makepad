//! Hosts a counter child with `HostedApp` and checks the whole loop: a
//! presented frame arrives, the semantic tree names the label and the
//! button (with the id path), a semantic press reaches the child, and the
//! label's text changes.

pub use makepad_widgets;

use makepad_app_host::{launch, AppHub, Clock, HostedApp, HubEvent};
use makepad_studio_protocol::{AppToStudio};
use makepad_widgets::*;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc::Sender, OnceLock};
use makepad_platform::thread::{Lane, TaskHandle};

static REMOTE_UI: OnceLock<makepad_app_host::a11y::RemoteUi> = OnceLock::new();
fn dispatch_hosted(req: &makepad_remote::Req, out: &mut makepad_remote::Out) {
    REMOTE_UI.get().unwrap().dispatch(req, out);
}

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(200, 100)
                body +: {}
            }
        }
    }
}

const CLIENT: u64 = 1;
const SIZE: Vec2d = Vec2d { x: 420.0, y: 220.0 };
const TIMEOUT: f64 = 60.0;

#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    WaitFrame,
    SnapBefore,
    SnapAfter,
}

struct Probe {
    hub: AppHub,
    hosted: HostedApp,
    child: Child,
    sender: Option<Sender<Vec<u8>>>,
    timer: Timer,
    phase: Phase,
    started: f64,
    action: Option<TaskHandle<Result<(), String>>>,
    trace: bool,
    sent: usize,
    last_trace: f64,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    probe: Option<Probe>,
}

fn finish(probe: &mut Probe, cx: &mut Cx, ok: bool, message: &str) -> ! {
    println!("{} {}", if ok { "PASS" } else { "FAIL" }, message);
    probe.hosted.send(vec![makepad_studio_protocol::StudioToApp::Kill]);
    probe.flush();
    let deadline = std::time::Instant::now() + launch::GROUP_KILL_GRACE;
    while probe.child.try_wait().ok().flatten().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if probe.child.try_wait().ok().flatten().is_none() {
        let pool = cx.task_pool();
        launch::kill_child_group(&mut probe.child, launch::GROUP_KILL_GRACE, &pool);
    }
    let _ = probe.child.wait();
    std::process::exit(if ok { 0 } else { 1 });
}

impl Probe {
    fn start(cx: &mut Cx) -> Probe {
        let hub = AppHub::start(cx.thread_spawner()).expect("no hub port");
        let mut hosted = HostedApp::new(Clock { now: Cx::monotonic_now, wall_now: Cx::time_now });
        hosted.start(cx);
        let binary = std::env::var("APP_HOST_PROBE_CHILD")
            .unwrap_or_else(|_| "target/release/makepad-example-counter".to_string());
        let mut cmd = Command::new(binary);
        cmd.arg("--stdin-loop").arg("--remote")
            .env("MAKEPAD_HIDE_WINDOWS", "1")
            .env("SANDBOX_MUTE", "1")
            .stdin(Stdio::null());
        launch::configure(&mut cmd, hub.port, CLIENT, "makepad-example-counter");
        let child = cmd.spawn().expect("spawn the counter");
        hosted.set_run_target(cx, CLIENT, 0, hub.port);
        Probe {
            hub,
            hosted,
            child,
            sender: None,
            timer: cx.start_interval(0.02),
            phase: Phase::WaitFrame,
            started: Cx::monotonic_now(),
            action: None,
            trace: std::env::var_os("APP_HOST_PROBE_TRACE").is_some(),
            sent: 0,
            last_trace: 0.0,
        }
    }

    fn pump(&mut self, cx: &mut Cx) {
        while let Ok(event) = self.hub.rx.try_recv() {
            match event {
                HubEvent::Connected { sender, .. } => {
                    self.trace("child connected");
                    self.sender = Some(sender)
                }
                HubEvent::Disconnected { .. } => self.sender = None,
                HubEvent::Mismatch { client, wire } => {
                    eprintln!("probe: child {client:?} was built from other protocol sources (wire {wire:?}); not admitted");
                }
                HubEvent::FromApp { client, msgs } => {
                    for msg in msgs {
                        if self.trace && !matches!(msg, AppToStudio::TickDone) {
                            eprintln!("probe: from child: {}", format!("{msg:?}").chars().take(120).collect::<String>());
                        }
                        match msg {
                            AppToStudio::CreateWindow { window_id, .. } => {
                                self.hosted.app_ready(cx, client, window_id);
                            }
                            other => {
                                self.hosted.on_app_message(cx, &other);
                            }
                        }
                    }
                }
            }
        }
        self.hosted.set_geometry(cx, Rect { pos: dvec2(0.0, 0.0), size: SIZE }, 1.0);
    }

    fn step(&mut self, cx: &mut Cx) {
        if Cx::monotonic_now() - self.started > TIMEOUT {
            let phase = match self.phase {
                Phase::WaitFrame => "no presented frame",
                Phase::SnapBefore => "no first snapshot",
                Phase::SnapAfter => "the label never changed",
            };
            finish(self, cx, false, &format!("timeout: {phase}"));
        }
        let remote = self.hosted.remote_ui();
        match self.phase {
            Phase::WaitFrame => {
                if self.hosted.texture().is_some() {
                    remote.enable(makepad_platform::a11y_tree::consumer::TEST);
                    self.phase = Phase::SnapBefore;
                }
            }
            Phase::SnapBefore => {
                let trees = remote.trees();
                let find = |path: &str| {
                    let q = makepad_platform::a11y_tree::query::Query::parse(&format!("path={path}")).unwrap();
                    trees.iter().flat_map(|(_,t)| q.find(t)).next().cloned()
                };
                let (Some(label), Some(button)) = (find("counter_label"),find("increment_button")) else { return };
                println!("frame ok; label {:?} path {:?}; button {:?} path {:?}",label.name,label.path,button.bounds,button.path);
                let _ = REMOTE_UI.set(remote);
                let command = format!("ui act node={} press wait=1", button.id.short());
                // Exercise the hosted command interpreter and its action/presentation replies.
                // The UI continues pumping the child while the pool task waits.
                match cx.task_pool().submit(Lane::Heavy, move || {
                    let (bytes, truncated) = makepad_remote::cmd::answer(dispatch_hosted, &command);
                    let reply = String::from_utf8_lossy(&bytes);
                    println!("hosted {command}: {reply}");
                    if truncated || reply.contains("\"err\"") || !reply.starts_with("handled:") {
                        Err(reply.into_owned())
                    } else { Ok(()) }
                }) {
                    Ok(task) => self.action = Some(task),
                    Err(error) => { self.trace(&format!("action queue busy; retry next beat: {error:?}")); return; }
                }
                self.phase = Phase::SnapAfter;
            }
            Phase::SnapAfter => {
                if let Some(task) = &mut self.action {
                    match task.try_take() {
                        None => return,
                        Some(Ok(Ok(()))) => self.action = None,
                        Some(error) => finish(self, cx, false, &format!("hosted ui action failed: {error:?}")),
                    }
                }
                let trees = remote.trees();
                let q = makepad_platform::a11y_tree::query::Query::parse("path=counter_label").unwrap();
                let text = trees.iter().flat_map(|(_,t)| q.find(t)).next().map(|n| n.name.clone());
                if text.as_deref().is_some_and(|t| t.ends_with('1')) {
                    finish(self, cx, true, &format!("semantic press changed the label to {:?}", text));
                }
            }
        }
    }
    fn trace(&self, line: &str) {
        if self.trace {
            eprintln!("probe: {line}");
        }
    }

    fn flush(&mut self) {
        let Some(sender) = &self.sender else { return };
        for (_, bin) in self.hosted.take_outgoing() {
            self.sent += 1;
            let _ = sender.send(bin);
        }
        let now = Cx::monotonic_now();
        if self.trace && now - self.last_trace > 2.0 {
            self.last_trace = now;
            eprintln!("probe: {} batches sent, frame {}", self.sent, self.hosted.has_frame());
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.probe = Some(Probe::start(cx));
        }
        if let Some(mut probe) = self.probe.take() {
            probe.hosted.handle_event(cx, event);
            let beat = match event {
                Event::Timer(t) => probe.timer.is_timer(t).is_some(),
                Event::Signal => true,
                _ => false,
            };
            if beat {
                probe.pump(cx);
                probe.step(cx);
            }
            probe.flush();
            self.probe = Some(probe);
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
