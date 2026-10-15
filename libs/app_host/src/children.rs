//! Hosted children: the process side of hosting `--stdin-loop` apps.
//!
//! [`Children`] is the one lifecycle every host of child apps shares (it
//! started as Stage DECK's live app layers): one [`AppHub`], started with the
//! first child; per child a [`Slot`] whose process the registry builds (when
//! the policy allows) and launches hidden, with the [`HostedApp`] of each of
//! its windows; the beat that pumps the hub while a child is starting or
//! running; a protocol shutdown (`Kill`) with a process-group kill as the
//! fallback when a child is quit; and the routing of the hub's messages to
//! the right window. What a host shows, when it starts or quits a child and
//! where its input goes stay the owner's: it keys its slots by its own `K`
//! and keeps its own per-slot state in `X`.
//!
//! A child may open several windows ([`ChildSpec::multi_window`]): each has
//! its own [`HostedApp`] (swapchain, geometry, frames, input) and its own
//! origin in the child's window space, and the first one paces the child's
//! `Tick`s for all of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use makepad_app_registry::{AppEntry, AppLaunch, BuildPolicy, LaunchEvent, LaunchKind, ProcessOutput};
use makepad_error_log::log;
use makepad_platform::thread::{SignalToUI, TaskHandle, TaskPool, ThreadOptions, ThreadSpawner};
use makepad_platform::*;
use makepad_studio_protocol::{AppToStudio, ChildRelay, HostRelay, StudioToApp};

use crate::hub::{AppHub, ClientId, HubEvent};
use crate::launch;
use crate::{Clock, HostedApp};

/// The windows of one multi-window child are this far apart in its window
/// space, so the child's hit test tells them apart.
const WINDOW_SPACING: f64 = 100_000.0;

/// Where a slot is in its life.
#[derive(Clone, Debug, PartialEq)]
pub enum Life {
    /// Prepared, not launched yet (the owner decides when).
    Ready,
    /// The registry is building the missing release binary.
    Building,
    /// Launched; its window has not announced itself yet.
    Starting,
    /// Its window is up (frames may still be on their way).
    Live,
    /// Launch or build failed, and why.
    Missing(String),
    /// It ended on its own.
    Exited(String),
}

/// The home folder a child runs with.
#[derive(Clone, Debug, PartialEq)]
pub enum Home {
    /// A fresh folder of its own (`SANDBOX_HOME`/`MAKEPAD_HOME`), deleted
    /// after it exits: a demo starts from a clean state.
    Scratch,
    /// The person's own home: what the child keeps (a browser profile)
    /// survives it.
    Inherit,
}

/// What to start and how.
#[derive(Clone, Debug, PartialEq)]
pub struct ChildSpec {
    /// Registry id, binary or package of the app.
    pub app: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Audible; otherwise it runs with `SANDBOX_MUTE=1`.
    pub audio: bool,
    pub home: Home,
    pub policy: BuildPolicy,
    /// Every window the child opens is hosted (see the module docs);
    /// otherwise only its first.
    pub multi_window: bool,
    /// The child shows its windows' content another way (frame rings):
    /// no swapchains are shared, no frames awaited (`HostedApp::
    /// set_frames_external`).
    pub frames_external: bool,
}

impl ChildSpec {
    pub fn new(app: impl Into<String>) -> ChildSpec {
        ChildSpec {
            app: app.into(),
            args: Vec::new(),
            env: Vec::new(),
            audio: false,
            home: Home::Scratch,
            policy: BuildPolicy::IfMissing,
            multi_window: false,
            frames_external: false,
        }
    }

    /// Whether a running child of `self` is the process `other` asks for:
    /// geometry and automation are not process identity.
    pub fn same_process(&self, other: &ChildSpec) -> bool {
        self.app == other.app && self.args == other.args && self.env == other.env && self.audio == other.audio && self.home == other.home
    }
}

/// The registry row of an app named by id, binary or package.
pub fn registry_entry(app: &str) -> Option<&'static AppEntry> {
    makepad_app_registry::find(app)
        .or_else(|| makepad_app_registry::find_by_bin(app))
        .or_else(|| makepad_app_registry::apps().iter().find(|entry| entry.package == app))
}

/// What happened to a slot since the owner last looked.
#[derive(Clone, Debug)]
pub enum ChildEvent<K> {
    /// Its life changed (built, started, exited, failed), or its loading
    /// status did while it has no frame yet.
    Changed(K),
    /// The child opened a window (its first one, or another of a
    /// multi-window child).
    Window { key: K, window_id: usize },
    /// An application message of the child (`AppToStudio::Custom`) that is
    /// not one of the protocol's own.
    Custom { key: K, json: String },
    /// Anything else the protocol leaves to the owner: cursor, clipboard,
    /// relays.
    Message { key: K, msg: AppToStudio },
}

/// One hosted child.
pub struct Slot<K, X> {
    pub key: K,
    pub spec: ChildSpec,
    /// The app's name for a human.
    pub label: String,
    pub life: Life,
    /// Its first window. It paces the child.
    pub hosted: HostedApp,
    /// Its further windows (a multi-window child).
    windows: Vec<HostedApp>,
    /// The owner holds a build that has finished from turning into the
    /// launch (DECK starts apps only on a slide that has settled).
    pub hold_build: bool,
    pub launch: Option<AppLaunch>,
    pub client: ClientId,
    child: Option<Child>,
    sender: Option<Sender<Vec<u8>>>,
    /// When it was launched.
    pub born: f64,
    /// Its scratch folder: the logs, and the home of a `Home::Scratch` child.
    pub folder: PathBuf,
    /// What the owner keeps per slot.
    pub extra: X,
}

impl<K, X> Slot<K, X> {
    /// The hosted window `window_id` of this child.
    pub fn window(&self, window_id: usize) -> Option<&HostedApp> {
        if self.hosted.target().is_some_and(|target| target.window_id == window_id) {
            return Some(&self.hosted);
        }
        self.windows.iter().find(|hosted| hosted.target().is_some_and(|target| target.window_id == window_id))
    }

    pub fn window_mut(&mut self, window_id: usize) -> Option<&mut HostedApp> {
        if self.hosted.target().is_some_and(|target| target.window_id == window_id) {
            return Some(&mut self.hosted);
        }
        self.windows.iter_mut().find(|hosted| hosted.target().is_some_and(|target| target.window_id == window_id))
    }

    /// Host window `window_id` of a multi-window child: one the child opened
    /// (its `CreateWindow`), or one it reuses after the owner closed it.
    pub fn open_window(&mut self, cx: &mut Cx, window_id: usize) {
        if !self.spec.multi_window || self.window(window_id).is_some() {
            return;
        }
        let Some(target) = self.hosted.target() else { return };
        let mut hosted = HostedApp::new(Clock { now: Cx::monotonic_now, wall_now: Cx::time_now });
        hosted.set_frames_external(self.spec.frames_external);
        hosted.set_paces_ticks(cx, false);
        hosted.start(cx);
        hosted.app_ready(cx, target.client, window_id);
        hosted.set_window_origin(window_origin(window_id));
        self.windows.push(hosted);
    }

    /// The owner is done with window `window_id` (the child closed it):
    /// its frames are dropped. The first window stays the pacer.
    pub fn close_window(&mut self, window_id: usize) {
        self.windows.retain(|hosted| hosted.target().is_none_or(|target| target.window_id != window_id));
    }

    /// The child's socket is up: what is sent now reaches it.
    pub fn is_connected(&self) -> bool {
        self.sender.is_some()
    }

    /// The process id while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    fn windows_mut(&mut self) -> impl Iterator<Item = &mut HostedApp> {
        std::iter::once(&mut self.hosted).chain(self.windows.iter_mut())
    }
}

/// The owner's per-slot state reads as the slot's own fields.
impl<K, X> std::ops::Deref for Slot<K, X> {
    type Target = X;
    fn deref(&self) -> &X {
        &self.extra
    }
}

impl<K, X> std::ops::DerefMut for Slot<K, X> {
    fn deref_mut(&mut self) -> &mut X {
        &mut self.extra
    }
}

/// A quit child whose exit is still being awaited.
struct Closing {
    label: String,
    child: Option<Child>,
    reaper: Option<TaskHandle<()>>,
    busy_logged: bool,
    folder: PathBuf,
}

pub struct Children<K, X> {
    /// Log lines start with it ("deck app", "browser host").
    name: &'static str,
    pool: TaskPool,
    hub: Option<AppHub>,
    hub_failed: bool,
    slots: Vec<Slot<K, X>>,
    closing: Vec<Closing>,
    sockets: HashMap<u64, ClientId>,
    next_client: ClientId,
    /// Each slot's folder is `<scratch>/<client>-<label>`.
    scratch: PathBuf,
    /// The owner's one-shot wake (`wake_at`): when it fires the hub is
    /// pumped like on a signal. Nothing else is timed.
    wake: Option<(Timer, f64)>,
    output_tx: Sender<(ClientId, ProcessOutput)>,
    output_rx: Receiver<(ClientId, ProcessOutput)>,
    events: Vec<ChildEvent<K>>,
    /// Clipboard reads the children asked the host for: host read id -> (client, the child's id).
    clipboard_reads: HashMap<u64, (ClientId, u64)>,
}

impl<K: Clone + PartialEq, X> Children<K, X> {
    /// `scratch` holds one folder per child (logs, scratch homes); it is
    /// removed when empty.
    pub fn new(cx: &mut Cx, name: &'static str, scratch: PathBuf) -> Self {
        let (output_tx, output_rx) = mpsc::channel();
        Children {
            name,
            pool: cx.task_pool(),
            hub: None,
            hub_failed: false,
            slots: Vec::new(),
            closing: Vec::new(),
            sockets: HashMap::new(),
            next_client: 1,
            scratch,
            wake: None,
            output_tx,
            output_rx,
            events: Vec::new(),
            clipboard_reads: HashMap::new(),
        }
    }

    pub fn slot(&self, key: &K) -> Option<&Slot<K, X>> {
        self.slots.iter().find(|slot| slot.key == *key)
    }

    pub fn slot_mut(&mut self, key: &K) -> Option<&mut Slot<K, X>> {
        self.slots.iter_mut().find(|slot| slot.key == *key)
    }

    pub fn slots(&self) -> impl Iterator<Item = &Slot<K, X>> {
        self.slots.iter()
    }

    pub fn slots_mut(&mut self) -> impl Iterator<Item = &mut Slot<K, X>> {
        self.slots.iter_mut()
    }

    /// Whether a quit child of this label is still shutting down (a
    /// replacement waits for it: it would share its files).
    pub fn is_closing(&self, label: &str) -> bool {
        self.closing.iter().any(|closing| closing.label == label)
    }

    /// What happened since the last call.
    pub fn take_events(&mut self) -> Vec<ChildEvent<K>> {
        std::mem::take(&mut self.events)
    }

    /// A slot in `Life::Ready`; `launch` starts it. Nothing when the key
    /// has one already.
    pub fn prepare(&mut self, key: K, spec: ChildSpec, label: String, extra: X) -> &mut Slot<K, X> {
        if let Some(at) = self.slots.iter().position(|slot| slot.key == key) {
            return &mut self.slots[at];
        }
        let client = self.next_client;
        self.next_client += 1;
        let safe: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let folder = self.scratch.join(format!("{client}-{safe}"));
        let mut hosted = HostedApp::new(Clock { now: Cx::monotonic_now, wall_now: Cx::time_now });
        hosted.set_frames_external(spec.frames_external);
        self.slots.push(Slot {
            key,
            spec,
            label,
            life: Life::Ready,
            hosted,
            windows: Vec::new(),
            hold_build: false,
            launch: None,
            client,
            child: None,
            sender: None,
            born: Cx::monotonic_now(),
            folder,
            extra,
        });
        self.slots.last_mut().unwrap()
    }

    /// Start a `Ready` slot: build it first when the policy says so. The
    /// hub (and the beat that pumps it) start with the first child.
    pub fn launch(&mut self, cx: &mut Cx, key: &K) {
        let Some(at) = self.slots.iter().position(|slot| slot.key == *key && slot.life == Life::Ready) else { return };
        let mut slot = self.slots.remove(at);
        let policy = slot.spec.policy;
        match self.spawn(cx, &mut slot, policy) {
            Ok(child) => {
                if slot.life != Life::Building {
                    log!("{} {}: launch pid {}", self.name, slot.label, child.id());
                    slot.born = Cx::monotonic_now();
                    slot.hosted.start(cx);
                }
                slot.child = Some(child);
            }
            Err(why) => {
                log!("{} {}: not launched: {why}", self.name, slot.label);
                slot.life = Life::Missing(why);
            }
        }
        self.events.push(ChildEvent::Changed(slot.key.clone()));
        self.slots.insert(at, slot);
    }

    fn spawn(&mut self, cx: &mut Cx, slot: &mut Slot<K, X>, policy: BuildPolicy) -> Result<Child, String> {
        let entry = registry_entry(&slot.spec.app).ok_or_else(|| format!("unknown hosted app: {}", slot.spec.app))?;
        if self.hub.is_none() && !self.hub_failed {
            self.hub = AppHub::start(cx.thread_spawner());
            self.hub_failed = self.hub.is_none();
        }
        let port = self.hub.as_ref().map(|hub| hub.port).ok_or("no port for the app hub")?;
        std::fs::create_dir_all(&slot.folder).map_err(|error| format!("{}: {error}", slot.folder.display()))?;
        if slot.spec.home == Home::Scratch {
            let home = slot.folder.join("home");
            std::fs::create_dir_all(&home).map_err(|error| format!("{}: {error}", home.display()))?;
        }
        let spawner = cx.thread_spawner();
        let pool = self.pool.clone();
        let output = self.output_tx.clone();
        let (spec, folder, client) = (&slot.spec, &slot.folder, slot.client);
        let (pending, child) = AppLaunch::start(entry, makepad_app_registry::repo_root(), spec.args.clone(), policy, slot.hold_build, |cmd, kind| {
            spawn_process(&spawner, &pool, spec, folder, client, port, entry.bin, cmd, kind, output.clone())
        })?;
        slot.life = if pending.is_building() { Life::Building } else { Life::Starting };
        slot.launch = Some(pending);
        slot.hosted.set_run_target(cx, slot.client, 0, port);
        Ok(child)
    }

    /// Drop its frames at once; ask the child to run its shutdown handler.
    pub fn quit(&mut self, cx: &mut Cx, key: &K) {
        let Some(at) = self.slots.iter().position(|slot| slot.key == *key) else { return };
        let mut slot = self.slots.remove(at);
        self.sockets.retain(|_, client| *client != slot.client);
        if slot.life == Life::Ready {
            log!("{} {}: not started", self.name, slot.label);
        }
        for hosted in slot.windows_mut() {
            hosted.clear_run_target(cx);
        }
        if let Some(mut child) = slot.child.take() {
            if slot.life == Life::Building {
                log!("{} {}: cancel build pid {}", self.name, slot.label, child.id());
            } else {
                log!("{} {}: quit pid {}", self.name, slot.label, child.id());
            }
            if let Some(sender) = &slot.sender {
                crate::hub::send_to_app(sender, vec![StudioToApp::Kill]);
            } else {
                launch::kill_child_group(&mut child, launch::GROUP_KILL_GRACE, &self.pool);
            }
            self.closing.push(Closing { label: slot.label.clone(), child: Some(child), reaper: None, busy_logged: false, folder: slot.folder.clone() });
        }
        self.retry_closing(cx);
    }

    /// Quit every child.
    pub fn shutdown(&mut self, cx: &mut Cx) {
        let all: Vec<_> = self.slots.iter().map(|slot| slot.key.clone()).collect();
        for key in all {
            self.quit(cx, &key);
        }
        if let Some((timer, _)) = self.wake.take() {
            cx.stop_timer(timer);
        }
    }

    fn retry_closing(&mut self, cx: &mut Cx) {
        if self.poll_closing() {
            self.wake_at(cx, Cx::monotonic_now() + 0.05);
        }
    }

    /// True when a reaper could not start (the pool is busy): the owner
    /// of the pump retries a moment later.
    fn poll_closing(&mut self) -> bool {
        let mut busy = false;
        self.closing.retain(|closing| !closing.reaper.as_ref().is_some_and(TaskHandle::is_finished));
        for closing in &mut self.closing {
            let Some(child) = closing.child.take() else { continue };
            let folder = closing.folder.clone();
            match launch::reap_closed_child(child, &self.pool, move || clear_folder(&folder)) {
                Ok(task) => closing.reaper = Some(task),
                Err(child) => {
                    if !closing.busy_logged {
                        log!("{} {}: shutdown worker busy; retrying", self.name, closing.label);
                    }
                    closing.busy_logged = true;
                    closing.child = Some(child);
                    busy = true;
                }
            }
        }
        busy
    }

    /// Pump the hub once at monotonic time `at` (an owner's deadline: a
    /// settle delay, a held build's release). One wake is kept, the
    /// earliest; the owner arms the next after it fired. Everything else
    /// wakes the loop by a signal: a child's message or frame, the end of
    /// a build or of a process, a finished reaper.
    pub fn wake_at(&mut self, cx: &mut Cx, at: f64) {
        match self.wake {
            Some((_, due)) if due <= at => return,
            Some((timer, _)) => cx.stop_timer(timer),
            None => {}
        }
        self.wake = Some((cx.start_timeout((at - Cx::monotonic_now()).max(0.0)), at));
    }

    /// Every event: the hosted windows' own beats, and the hub's messages
    /// when it is woken (a signal, or the owner's `wake_at`). True when the
    /// hub was pumped.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        for slot in &mut self.slots {
            // A window that does not pace hands its wish for a frame to
            // the one that does.
            if slot.windows.iter().any(HostedApp::wants_tick) {
                slot.hosted.request_tick(cx);
            }
            for hosted in slot.windows_mut() {
                hosted.handle_event(cx, event);
            }
        }
        // the host clipboard's answer to a child's read
        if let Event::Clipboard(e) = event {
            if let Some((client, request_id)) = self.clipboard_reads.remove(&e.request) {
                let png = match &e.content.image {
                    Some(makepad_platform::clipboard_read::ClipboardImage::Png(png)) => png.to_vec(),
                    _ => Vec::new(),
                };
                let text = e.content.text.clone().unwrap_or_default();
                if let Some(slot) = self.slots.iter_mut().find(|slot| slot.client == client) {
                    slot.hosted.send(vec![StudioToApp::Relay(HostRelay::Clipboard { request_id, text, png })]);
                }
            }
        }
        let beat = match event {
            Event::Timer(timer) => {
                let fired = self.wake.is_some_and(|(wake, _)| wake.is_timer(timer).is_some());
                if fired {
                    self.wake = None;
                }
                fired
            }
            Event::Signal => true,
            _ => false,
        };
        if beat {
            self.pump(cx);
        }
        self.flush();
        beat
    }

    fn pump(&mut self, cx: &mut Cx) {
        self.retry_closing(cx);
        while let Some(event) = self.hub.as_ref().and_then(|hub| hub.rx.try_recv().ok()) {
            match event {
                HubEvent::Connected { client, socket, sender } => {
                    self.sockets.insert(socket, client);
                    if let Some(slot) = self.slots.iter_mut().find(|slot| slot.client == client) {
                        slot.sender = Some(sender);
                    }
                }
                HubEvent::Disconnected { socket } => {
                    if let Some(client) = self.sockets.remove(&socket) {
                        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.client == client) {
                            slot.sender = None;
                        }
                    }
                }
                HubEvent::Mismatch { client, wire } => self.stale(cx, client, wire),
                HubEvent::FromApp { client, msgs } => {
                    let Some(slot) = self.slots.iter_mut().find(|slot| slot.client == client) else { continue };
                    for msg in msgs {
                        route(cx, slot, client, msg, &mut self.events, &mut self.clipboard_reads);
                    }
                }
            }
        }
        while let Ok((client, output)) = self.output_rx.try_recv() {
            if let Some(slot) = self.slots.iter_mut().find(|slot| slot.client == client) {
                if let Some(pending) = &mut slot.launch {
                    if pending.output(&output).is_some() {
                        self.events.push(ChildEvent::Changed(slot.key.clone()));
                    }
                }
            }
        }
        let now = Cx::monotonic_now();
        let port = self.hub.as_ref().map_or(0, |hub| hub.port);
        let spawner = cx.thread_spawner();
        for slot in &mut self.slots {
            if !matches!(slot.life, Life::Building | Life::Live | Life::Starting) {
                continue;
            }
            let hold = slot.hold_build;
            let Some(pending) = slot.launch.as_mut().filter(|pending| !pending.is_building() || !hold) else { continue };
            let (spec, folder, client) = (&slot.spec, &slot.folder, slot.client);
            let polled = pending.poll(&mut slot.child, |cmd, kind| {
                let entry = registry_entry(&spec.app).ok_or_else(|| format!("unknown hosted app: {}", spec.app))?;
                spawn_process(&spawner, &self.pool, spec, folder, client, port, entry.bin, cmd, kind, self.output_tx.clone())
            });
            match polled {
                Ok(Some(LaunchEvent::Started)) => {
                    self.events.push(ChildEvent::Changed(slot.key.clone()));
                    slot.life = Life::Starting;
                    if let Some(child) = &slot.child {
                        log!("{} {}: launch pid {}", self.name, slot.label, child.id());
                    }
                    slot.born = now;
                    slot.hosted.start(cx);
                }
                Ok(Some(LaunchEvent::Exited(status))) => {
                    self.events.push(ChildEvent::Changed(slot.key.clone()));
                    log!("{} {}: exited ({status})", self.name, slot.label);
                    slot.life = Life::Exited(status.to_string());
                    slot.sender = None;
                    for hosted in slot.windows_mut() {
                        hosted.clear_run_target(cx);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.events.push(ChildEvent::Changed(slot.key.clone()));
                    slot.life = Life::Missing(format!("{error} — see {}", slot.folder.join("build.log").display()));
                    slot.sender = None;
                    for hosted in slot.windows_mut() {
                        hosted.clear_run_target(cx);
                    }
                }
            }
        }
    }

    /// The child of `client` was built from other protocol sources than this host (`wire`:
    /// what it stated): it would hang in its first steps. It is stopped and the slot says
    /// why. From a checkout a child is started with `cargo run`, so it is built from this
    /// host's sources; a mismatch is a binary from elsewhere (an installation of another
    /// version), which a rebuild here would not fix.
    fn stale(&mut self, cx: &mut Cx, client: ClientId, wire: Option<u64>) {
        let Some(at) = self.slots.iter().position(|slot| slot.client == client && matches!(slot.life, Life::Starting | Life::Live)) else { return };
        let mut slot = self.slots.remove(at);
        let label = registry_entry(&slot.spec.app).map_or(slot.spec.app.as_str(), |entry| entry.label).to_string();
        log!("{} {}: built from other protocol sources (it states wire {}, this host {:016x})", self.name, slot.label,
            wire.map_or("none".to_string(), |wire| format!("{wire:016x}")), makepad_studio_protocol::WIRE);
        if let Some(mut child) = slot.child.take() {
            // It may not read this host's Kill: its process group goes.
            launch::kill_child_group(&mut child, launch::GROUP_KILL_GRACE, &self.pool);
            self.closing.push(Closing { label: slot.label.clone(), child: Some(child), reaper: None, busy_logged: false, folder: slot.folder.clone() });
        }
        slot.sender = None;
        slot.launch = None;
        for hosted in slot.windows_mut() {
            hosted.clear_run_target(cx);
        }
        slot.windows.clear();
        slot.life = Life::Missing(stale_message(&label, makepad_app_registry::repo_root().is_some()));
        self.events.push(ChildEvent::Changed(slot.key.clone()));
        self.slots.insert(at, slot);
        self.retry_closing(cx);
    }

    /// Send what the hosted windows queued.
    pub fn flush(&mut self) {
        for slot in &mut self.slots {
            let sender = slot.sender.clone();
            for hosted in slot.windows_mut() {
                let outgoing = hosted.take_outgoing();
                let Some(sender) = &sender else { continue };
                for (_, bin) in outgoing {
                    let _ = sender.send(bin);
                }
            }
        }
    }

    /// Send an application message to the child (it arrives as
    /// `Event::Custom`).
    pub fn send_custom(&mut self, key: &K, json: String) {
        if let Some(slot) = self.slot_mut(key) {
            slot.hosted.send(vec![StudioToApp::Custom(json)]);
        }
    }
}

/// Route one message of a child to the window it is for.
fn route<K: Clone, X>(cx: &mut Cx, slot: &mut Slot<K, X>, client: ClientId, msg: AppToStudio, events: &mut Vec<ChildEvent<K>>,
    clipboard_reads: &mut HashMap<u64, (ClientId, u64)>) {
    match msg {
        // The children share the host's clipboard: a child's copy is the host's (which, hosted
        // itself, hands it on to its own host), a child's read is answered from it.
        AppToStudio::SetClipboard(text) => cx.copy_to_clipboard(&text),
        AppToStudio::Relay(ChildRelay::SetPrimarySelection { text }) => cx.set_primary_selection(&text),
        AppToStudio::Relay(ChildRelay::ReadClipboard { request_id }) => {
            clipboard_reads.insert(cx.read_clipboard(), (client, request_id));
        }
        AppToStudio::CreateWindow { window_id, .. } => {
            let first = slot.life == Life::Starting;
            let pacer = slot.hosted.target().is_some_and(|target| target.window_id == window_id);
            if first || pacer && slot.spec.multi_window {
                slot.hosted.app_ready(cx, client, window_id);
                if slot.spec.multi_window {
                    slot.hosted.set_window_origin(window_origin(window_id));
                }
                slot.life = Life::Live;
                events.push(ChildEvent::Changed(slot.key.clone()));
            } else if slot.spec.multi_window && slot.window(window_id).is_none() {
                slot.open_window(cx, window_id);
            } else {
                return;
            }
            events.push(ChildEvent::Window { key: slot.key.clone(), window_id });
        }
        AppToStudio::DrawCompleteAndFlip(draw) => {
            if let Some(hosted) = slot.window_mut(draw.window_id) {
                hosted.set_presentable_draw(cx, draw);
            } else {
                slot.hosted.set_presentable_draw(cx, draw);
            }
        }
        AppToStudio::Custom(json) => {
            if let Some(caps) = makepad_platform::ime::HostedPointerCaps::parse(&json) {
                for hosted in slot.windows_mut() {
                    hosted.set_pointer_caps(caps.clone());
                }
                return;
            }
            let status = slot.hosted.status().cloned();
            let msg = AppToStudio::Custom(json);
            if slot.hosted.on_app_message(cx, &msg) {
                if !slot.hosted.has_frame() && slot.hosted.status() != status.as_ref() {
                    events.push(ChildEvent::Changed(slot.key.clone()));
                }
            } else if let AppToStudio::Custom(json) = msg {
                events.push(ChildEvent::Custom { key: slot.key.clone(), json });
            }
        }
        other => {
            if !slot.hosted.on_app_message(cx, &other) {
                events.push(ChildEvent::Message { key: slot.key.clone(), msg: other });
            }
        }
    }
}

/// What a child built from other protocol sources than this host is told.
fn stale_message(label: &str, has_checkout: bool) -> String {
    if has_checkout {
        format!("{label} is not built from this checkout's sources (its hosted protocol differs): start it from this checkout")
    } else {
        format!("{label} was built for another version of this host (its hosted protocol differs): update it")
    }
}

fn window_origin(window_id: usize) -> Vec2d {
    dvec2(window_id as f64 * WINDOW_SPACING, 0.0)
}

impl<K, X> Drop for Children<K, X> {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if let Some(mut child) = slot.child.take() {
                log!("{} {}: quit pid {}", self.name, slot.label, child.id());
                if let Some(sender) = &slot.sender {
                    crate::hub::send_to_app(sender, vec![StudioToApp::Kill]);
                } else {
                    launch::kill_child_group(&mut child, launch::GROUP_KILL_GRACE, &self.pool);
                }
                self.closing.push(Closing { label: slot.label.clone(), child: Some(child), reaper: None, busy_logged: false, folder: slot.folder.clone() });
            }
        }
        for closing in &mut self.closing {
            if let Some(task) = closing.reaper.take() {
                task.detach();
            }
            if let Some(child) = closing.child.take() {
                let folder = closing.folder.clone();
                match launch::reap_closed_child(child, &self.pool, move || clear_folder(&folder)) {
                    Ok(task) => task.detach(),
                    Err(child) => {
                        log!("{} {}: shutdown worker unavailable; stopping process group", self.name, closing.label);
                        launch::reap_child_group(child, launch::GROUP_KILL_GRACE, &self.pool);
                    }
                }
            }
        }
    }
}

/// A child's folder is deleted only after its shutdown, off the UI.
fn clear_folder(folder: &Path) {
    if let Err(error) = std::fs::remove_dir_all(folder) {
        if error.kind() != std::io::ErrorKind::NotFound {
            log!("app host: removing a child's scratch folder: {error}");
        }
    }
    // Other children may still use this scratch. Only remove an empty parent.
    if let Some(scratch) = folder.parent() {
        let _ = std::fs::remove_dir(scratch);
    }
}

/// Expand an argument or environment value, once, in one pass: `${NAME}`
/// is the environment variable (empty when unset), `${NAME:-text}` is the
/// variable or, when unset or empty, `text`; a leading `~` (alone or before
/// a `/`), after that, is the home folder. So a host names a folder on any
/// machine: `["--cwd", "${MAKEPAD_DEMO_REPO:-~/makepad}"]`. A `${` without
/// its `}` stays as written.
pub fn expand_home(value: &str) -> String {
    expand_with(value, &|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

fn expand_with(value: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find("${") {
        let Some(close) = rest[at..].find('}') else { break };
        out.push_str(&rest[..at]);
        let body = &rest[at + 2..at + close];
        let (name, default) = match body.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (body, None),
        };
        match lookup(name) {
            Some(found) => out.push_str(&found),
            None => out.push_str(default.unwrap_or("")),
        }
        rest = &rest[at + close + 1..];
    }
    out.push_str(rest);
    match (out.strip_prefix('~'), lookup("HOME")) {
        (Some(tail), Some(home)) if tail.is_empty() || tail.starts_with('/') => format!("{}{tail}", Path::new(&home).display()),
        _ => out,
    }
}

/// The host's environment and process group; command selection belongs to
/// the registry.
#[allow(clippy::too_many_arguments)]
fn spawn_process(
    spawner: &ThreadSpawner,
    pool: &TaskPool,
    spec: &ChildSpec,
    folder: &Path,
    client: ClientId,
    port: u16,
    bin: &str,
    mut command: Command,
    kind: LaunchKind,
    lines: Sender<(ClientId, ProcessOutput)>,
) -> Result<Child, String> {
    let building = kind == LaunchKind::Build;
    // A build's progress and errors, or cargo's (a `cargo run`) then the app's own log. The
    // folder is made here, each time: a temp folder can be gone by the time a launch that
    // waited (a held build, a slide that settled later) opens its log.
    std::fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let log_path = folder.join(if building { "build.log" } else { "app.log" });
    let log_file = std::fs::File::create(&log_path).map_err(|error| format!("{}: {error}", log_path.display()))?;
    // Output piped to the card: a build's, and a `cargo run`'s (cargo's progress first).
    let piped = kind != LaunchKind::App;
    command.stdin(Stdio::null());
    #[cfg(unix)]
    launch::own_process_group(&mut command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    if building {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command
            .envs(spec.env.iter().map(|(key, value)| (key, expand_home(value))))
            .env("MAKEPAD_HIDE_WINDOWS", "1");
        if piped {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else {
            command.stdout(Stdio::null()).stderr(log_file.try_clone().map_err(|error| error.to_string())?);
        }
        if spec.home == Home::Scratch {
            let home = folder.join("home");
            command.env("SANDBOX_HOME", &home).env("MAKEPAD_HOME", &home);
        }
        if !spec.audio {
            command.env("SANDBOX_MUTE", "1");
        }
        launch::configure(&mut command, port, client, bin);
        // A hosted child never outlives this host (quit, crash or kill).
        #[cfg(target_os = "linux")]
        launch::exit_with_parent(&mut command);
    }
    let mut child = command.spawn().map_err(|error| format!("launch {bin}: {error}"))?;
    // Its end wakes the loop (a build finishing, an app dying before it connected).
    if let Err(why) = launch::signal_on_exit(spawner, &child, format!("app-{client}-exit")) {
        launch::reap_child_group(child, launch::GROUP_KILL_GRACE, pool);
        return Err(why);
    }
    // A hosted child never outlives this host: Windows ties it to the host's job here.
    #[cfg(windows)]
    if !building {
        launch::exit_with_parent(&child);
    }
    if piped {
        let streams: Vec<Box<dyn std::io::Read + Send>> = vec![Box::new(child.stdout.take().unwrap()), Box::new(child.stderr.take().unwrap())];
        for stream in streams {
            let log_file = log_file.try_clone().ok();
            let lines = lines.clone();
            let task = spawner.spawn_worker(ThreadOptions { name: Some(format!("app-build-{client}-output").into()), ..Default::default() }, move || {
                makepad_app_registry::read_output(stream, log_file, |output| {
                    if lines.send((client, output)).is_err() {
                        return false;
                    }
                    SignalToUI::set_ui_signal();
                    true
                });
            });
            match task {
                Ok(task) => task.detach(),
                Err(error) => {
                    launch::reap_child_group(child, launch::GROUP_KILL_GRACE, pool);
                    return Err(format!("build output: {error}"));
                }
            }
        }
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::{expand_with, stale_message, window_origin};

    /// A child that states another wire is refused with a clear reason (never "starting…"
    /// forever, never a rebuild loop).
    #[test]
    fn a_stale_child_is_named() {
        assert!(stale_message("Calculator", true).starts_with("Calculator is not built from this checkout"));
        assert!(stale_message("Calculator", false).starts_with("Calculator was built for another version"));
    }

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/arch".to_string()),
            "REPO" => Some("/srv/repo".to_string()),
            _ => None,
        }
    }

    #[test]
    fn home_and_variables() {
        assert_eq!(expand_with("~/makepad", &env), "/home/arch/makepad");
        assert_eq!(expand_with("~", &env), "/home/arch");
        assert_eq!(expand_with("~user/x", &env), "~user/x");
        assert_eq!(expand_with("${REPO}/local", &env), "/srv/repo/local");
        assert_eq!(expand_with("${REPO:-~/makepad}/local", &env), "/srv/repo/local");
        assert_eq!(expand_with("${MISSING:-~/makepad}", &env), "/home/arch/makepad");
        assert_eq!(expand_with("${MISSING:-~/makepad}/local/scope-demo/home", &env), "/home/arch/makepad/local/scope-demo/home");
        assert_eq!(expand_with("a${MISSING}b", &env), "ab");
        assert_eq!(expand_with("${A:-x}-${B:-y}", &env), "x-y");
        assert_eq!(expand_with("${unterminated", &env), "${unterminated");
        assert_eq!(expand_with("plain", &env), "plain");
    }

    /// Windows of one child never overlap in its window space, whatever
    /// size the host gives them.
    #[test]
    fn window_origins_are_apart() {
        assert_eq!(window_origin(0).x, 0.0);
        assert!(window_origin(1).x - window_origin(0).x >= 16384.0);
    }
}
