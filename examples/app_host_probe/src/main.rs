//! Hosts a counter child with `HostedApp` and checks the whole loop: a
//! presented frame arrives, the widget snapshot names the label and the
//! button (with the id path), a click forwarded at the button's snapshot
//! rect reaches the child, and the label's text changes.

pub use makepad_widgets;

use makepad_app_host::{launch, AppHub, Clock, HostedApp, HubEvent};
use makepad_studio_protocol::{AppToStudio, MouseButton, WidgetSnapshot};
use makepad_widgets::*;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;

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
    SnapBefore(u64),
    SnapAfter(u64),
}

struct Probe {
    hub: AppHub,
    hosted: HostedApp,
    child: Child,
    sender: Option<Sender<Vec<u8>>>,
    timer: Timer,
    phase: Phase,
    started: f64,
    button: Option<(f64, f64)>,
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
    let pool = cx.task_pool();
    launch::kill_child_group(&mut probe.child, launch::GROUP_KILL_GRACE, &pool);
    let _ = probe.child.wait();
    std::process::exit(if ok { 0 } else { 1 });
}

fn find<'a>(widgets: &'a [WidgetSnapshot], id: &str) -> Option<&'a WidgetSnapshot> {
    widgets.iter().find(|w| w.id == id && w.visible && w.width > 0)
}

impl Probe {
    fn start(cx: &mut Cx) -> Probe {
        let hub = AppHub::start(cx.thread_spawner()).expect("no hub port");
        let mut hosted = HostedApp::new(Clock { now: Cx::monotonic_now, wall_now: Cx::time_now });
        hosted.start(cx);
        let binary = std::env::var("APP_HOST_PROBE_CHILD")
            .unwrap_or_else(|_| "target/release/makepad-example-counter".to_string());
        let mut cmd = Command::new(binary);
        cmd.arg("--stdin-loop")
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
            button: None,
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
                Phase::SnapBefore(_) => "no first snapshot",
                Phase::SnapAfter(_) => "the label never changed",
            };
            finish(self, cx, false, &format!("timeout: {phase}"));
        }
        match self.phase {
            Phase::WaitFrame => {
                if self.hosted.texture().is_some() {
                    let id = self.hosted.snapshot();
                    self.phase = Phase::SnapBefore(id);
                }
            }
            Phase::SnapBefore(id) => {
                let Some(widgets) = self.hosted.take_snapshot(id) else { return };
                let label = find(&widgets, "counter_label");
                let button = find(&widgets, "increment_button");
                let (Some(label), Some(button)) = (label, button) else {
                    // The first answers can precede the first render.
                    let id = self.hosted.snapshot();
                    self.phase = Phase::SnapBefore(id);
                    return;
                };
                println!(
                    "frame ok; label {:?} path {:?}; button rect {},{} {}x{} path {:?}",
                    label.text, label.path, button.x, button.y, button.width, button.height, button.path
                );
                if label.path.is_none() || button.path.is_none() {
                    finish(self, cx, false, "named widgets report no id path");
                }
                let at = dvec2(
                    button.x as f64 + button.width as f64 * 0.5,
                    button.y as f64 + button.height as f64 * 0.5,
                );
                self.button = Some((at.x, at.y));
                let modifiers = KeyModifiers::default();
                let time = Cx::time_now();
                self.hosted.mouse_down(at, MouseButton::PRIMARY, time, &modifiers);
                self.hosted.mouse_up(Some(at), MouseButton::PRIMARY, time, &modifiers, false);
                let id = self.hosted.snapshot();
                self.phase = Phase::SnapAfter(id);
            }
            Phase::SnapAfter(id) => {
                let Some(widgets) = self.hosted.take_snapshot(id) else { return };
                let text = find(&widgets, "counter_label").and_then(|w| w.text.clone());
                if text.as_deref().is_some_and(|t| t.ends_with('1')) {
                    finish(self, cx, true, &format!("click at {:?} changed the label to {:?}", self.button, text));
                }
                let id = self.hosted.snapshot();
                self.phase = Phase::SnapAfter(id);
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
