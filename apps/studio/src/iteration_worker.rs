//! Durable flow host. Commands, Git, Cargo, process ownership and feedback
//! ingestion live on one worker; the UI only sends bounded requests/snapshots.
use crate::iteration::{self, Command as FlowCommand, Effect, Engine, Observation, Transition};
use crate::iteration_git::{self as git, OwnedWorktree};
use makepad_strict_json::{self as json, Value};
use makepad_widgets::makepad_platform::thread::{
    SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Snapshot {
    pub engine: Engine,
    pub operations: Value,
    pub note: String,
    pub attachments: Vec<Attachment>,
    pub previews: Vec<Arc<Preview>>,
    pub presentation: Value,
    pub widths: Value,
    pub recordings: Vec<RecordingTile>,
    pub full_preview: Option<Arc<Preview>>,
    pub full_preview_selection: Option<(String, String)>,
    pub full_preview_error: Option<String>,
    pub embedded: Vec<EmbeddedRun>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            engine: Engine::default(),
            operations: Value::Arr(vec![]),
            note: "Starting iteration host".into(),
            attachments: Vec::new(),
            previews: Vec::new(),
            presentation: Value::Null,
            widths: Value::Obj(vec![]),
            recordings: Vec::new(),
            full_preview: None,
            full_preview_selection: None,
            full_preview_error: None,
            embedded: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    TerminalBusy {
        flow: String,
        busy: bool,
    },
    ClearHistory {
        flow: String,
    },
    SplitLane {
        flow: String,
        item: String,
        title: String,
    },
    FeedbackReport {
        flow: String,
        requirement: Option<FlowCommand>,
        todos: FlowCommand,
    },
    Test {
        flow: String,
        action: TestRequest,
    },
    SetLifecycle {
        flow: String,
        state: iteration::FlowLifecycle,
    },
    HostPort {
        port: u16,
    },
    HostRegistered {
        client: u64,
    },
    FullPreview {
        recording: Option<String>,
    },
    OpenImage {
        flow: String,
        preview_id: String,
    },
    DeleteVideos {
        flow: String,
    },
    ImportAttachment {
        flow: String,
        path: PathBuf,
        delivered: bool,
    },
    AttachmentDelivered {
        id: String,
    },
    ClearAttachmentTray {
        flow: String,
    },
    LaneWidth {
        flow: String,
        width: f64,
    },
    TerminalHeight {
        flow: String,
        height: f64,
    },
    Flow(FlowCommand),
    Close {
        flow: String,
    },
    PopOut {
        flow: String,
        run_id: String,
    },
    GitInspect {
        flow: String,
    },
    /// Remove an archived lane's owned checkout. Dirty work is refused
    /// unless `force`; the branch and its checkpoints always stay.
    ReleaseWorkspace {
        flow: String,
        force: bool,
    },
    GitPreview {
        flow: String,
        target: String,
    },
    GitApply {
        flow: String,
        target: String,
        source: String,
        destination: String,
        title: String,
    },
    SyncPreview {
        flow: String,
        source: String,
        target: String,
    },
    SyncApply {
        flow: String,
        source: String,
        target: String,
        source_oid: String,
        target_oid: String,
    },
    Fetch {
        flow: String,
    },
    Diff {
        flow: String,
        artifact: Option<String>,
    },
    /// A code-intelligence call on behalf of a lane (see iteration_code.rs).
    CodeTool {
        flow: String,
        tool: String,
        args: Value,
    },
}
struct Envelope {
    id: String,
    request: Request,
}
pub struct Reply {
    pub id: String,
    pub result: Result<Value, String>,
}
pub struct IterationWorker {
    embedding: crate::iteration_host::IterationHost,
    requests: SyncSender<Envelope>,
    replies: Receiver<Reply>,
    snapshots: Receiver<Arc<Snapshot>>,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
}
impl IterationWorker {
    pub fn start(spawner: &ThreadSpawner, directory: PathBuf) -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (spawner, directory);
            Err("Local iteration processes require desktop Studio".into())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let embedding = crate::iteration_host::IterationHost::start(spawner)?;
            let embedding_commands = embedding.command_sender();
            let (requests, rx) = mpsc::sync_channel(32);
            let (tx, replies) = mpsc::sync_channel(32);
            let (snap_tx, snapshots) = mpsc::sync_channel(1);
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let code_spawner = spawner.clone();
            let task = spawner
                .spawn_worker(
                    ThreadOptions {
                        name: Some("studio-iterations".into()),
                        ..Default::default()
                    },
                    move || host(directory, rx, tx, snap_tx, stopping, embedding_commands, code_spawner),
                )
                .map_err(|e| e.to_string())?;
            Ok(Self {
                embedding,
                requests,
                replies,
                snapshots,
                stop,
                task,
            })
        }
    }
    pub fn submit(&self, id: String, request: Request) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) || self.task.is_finished() {
            return Err("Iteration host stopped".into());
        }
        self.requests
            .try_send(Envelope { id, request })
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    "Iteration queue is full; input was not accepted. Retry when it drains".into()
                }
                TrySendError::Disconnected(_) => "Iteration host disconnected".into(),
            })
    }
    pub fn poll(&self) -> (Vec<Reply>, Option<Arc<Snapshot>>) {
        let replies = self.replies.try_iter().collect();
        let snapshot = self.snapshots.try_iter().last();
        (replies, snapshot)
    }
    pub fn poll_host(&self) -> Vec<crate::iteration_host::HostEvent> {
        let mut events = Vec::new();
        for _ in 0..128 {
            match self.embedding.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        events
    }
    pub fn host_send(
        &self,
        command: crate::iteration_host::HostCommand,
    ) -> Result<(), TrySendError<crate::iteration_host::HostCommand>> {
        self.embedding.try_send(command)
    }
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        if self.task.is_finished() {
            self.embedding.request_stop();
            self.embedding.is_finished()
        } else {
            false
        }
    }
}
impl Drop for IterationWorker {
    fn drop(&mut self) {
        self.request_stop();
    }
}

include!("iteration_build.rs");
include!("iteration_media.rs");
include!("iteration_recordings.rs");
include!("iteration_cli.rs");
include!("iteration_http.rs");
include!("iteration_embedding.rs");
include!("iteration_testing.rs");
include!("iteration_split.rs");
include!("iteration_code.rs");

#[derive(Clone, PartialEq, Eq)]
struct FullPreviewStamp {
    path: PathBuf,
    file: (u64, u128),
    sequence: u64,
}

struct PendingLifecycle {
    state: iteration::FlowLifecycle,
    started: Instant,
    build_term_sent: bool,
    build_kill_sent: bool,
    app_group: Option<u32>,
    #[cfg(unix)]
    app_term_at: Option<Instant>,
    #[cfg(unix)]
    app_kill_sent: bool,
}

struct Host {
    directory: PathBuf,
    engine: Engine,
    seen_feedback: BTreeSet<String>,
    effects: VecDeque<Effect>,
    cli_archive_cursor: usize,
    builds: BTreeMap<String, Build>,
    apps: BTreeMap<String, AppRun>,
    test_operations: BTreeMap<String, TestOperation>,
    lifecycle_pending: BTreeMap<String, PendingLifecycle>,
    terminal_busy: BTreeSet<String>,
    fingerprints: BTreeMap<String, String>,
    reports: BTreeMap<String, Value>,
    storage_error: Option<String>,
    attachments: Vec<Attachment>,
    previews: BTreeMap<String, Arc<Preview>>,
    preview_failed: BTreeSet<String>,
    terminal_heights: BTreeMap<String, f64>,
    lane_widths: BTreeMap<String, f64>,
    recordings: RecordingState,
    embedding_commands: SyncSender<crate::iteration_host::HostCommand>,
    embedding_port: Option<u16>,
    embedding_pending: BTreeMap<u64, PendingEmbedded>,
    embedding_unregister: BTreeSet<u64>,
    full_preview: Option<Arc<Preview>>,
    full_preview_selection: Option<(String, String)>,
    full_preview_stamp: Option<FullPreviewStamp>,
    full_preview_error: Option<String>,
    design_snapshots: BTreeMap<String, (u64, String)>,
    code: CodeRuntime,
    note: String,
    changed: bool,
}

impl Host {
    fn require_active_lane(&self, flow: &str) -> Result<(), String> {
        if self.lifecycle_pending.contains_key(flow) {
            return Err("This lane is stopping; wait for its owned processes to finish".into());
        }
        if self.flow(flow)?.lifecycle != iteration::FlowLifecycle::Active {
            return Err(
                "Start or restore this lane before preparing or running another build".into(),
            );
        }
        Ok(())
    }

    fn set_lifecycle(
        &mut self,
        flow: &str,
        state: iteration::FlowLifecycle,
    ) -> Result<Value, String> {
        let current = self.flow(flow)?;
        if state == iteration::FlowLifecycle::Active {
            if self.lifecycle_pending.contains_key(flow) {
                return Err(
                    "Lane shutdown is still in progress; start it after its processes have exited"
                        .into(),
                );
            }
            if current.lifecycle != state {
                self.observe(Observation::LifecycleChanged {
                    flow: flow.into(),
                    state,
                })?;
            }
            return Ok(json::obj(vec![
                ("flow", s(flow)),
                ("state", s(state.as_str())),
                ("complete", Value::Bool(true)),
            ]));
        }
        if current.runs.iter().any(|run| !run.closed)
            && !self.apps.get(flow).is_some_and(|owned| {
                current
                    .runs
                    .iter()
                    .any(|run| !run.closed && run.id == owned.run)
            })
        {
            return Err("A previous app run has unresolved process ownership; reconcile it before stopping or archiving the lane".into());
        }
        let current_lifecycle = current.lifecycle;
        if let Some(pending) = self.lifecycle_pending.get_mut(flow) {
            pending.state = state;
            self.lifecycle_report(flow, state, "stopping", None);
            return Ok(json::obj(vec![
                ("flow", s(flow)),
                ("state", s(state.as_str())),
                ("complete", Value::Bool(false)),
            ]));
        }
        if current_lifecycle == state
            && !self.apps.contains_key(flow)
            && !self.builds.contains_key(flow)
        {
            return Ok(json::obj(vec![
                ("flow", s(flow)),
                ("state", s(state.as_str())),
                ("complete", Value::Bool(true)),
            ]));
        }
        self.observe(Observation::WorkCanceled {
            flow: flow.into(),
            reason: format!(
                "Lane {} requested; waiting for owned processes to exit",
                state.as_str()
            ),
        })?;
        self.effects.retain(|effect| effect_flow(effect) != flow);
        self.cancel_pending_embedding(flow, "Lane stopped before the app launched");
        let app_group = self.apps.get(flow).map(|run| run.child.id());
        if self.apps.contains_key(flow) {
            self.close(flow)?;
        }
        self.lifecycle_pending.insert(
            flow.into(),
            PendingLifecycle {
                state,
                started: Instant::now(),
                build_term_sent: false,
                build_kill_sent: false,
                app_group,
                #[cfg(unix)]
                app_term_at: None,
                #[cfg(unix)]
                app_kill_sent: false,
            },
        );
        self.lifecycle_report(flow, state, "stopping", None);
        self.poll_lifecycle();
        let complete =
            !self.lifecycle_pending.contains_key(flow) && self.flow(flow)?.lifecycle == state;
        Ok(json::obj(vec![
            ("flow", s(flow)),
            ("state", s(state.as_str())),
            ("complete", Value::Bool(complete)),
        ]))
    }

    fn lifecycle_report(
        &mut self,
        flow: &str,
        state: iteration::FlowLifecycle,
        phase: &str,
        error: Option<&str>,
    ) {
        let report = json::obj(vec![
            ("kind", s("flow_lifecycle")),
            ("flow", s(flow)),
            ("state", s(state.as_str())),
            ("phase", s(phase)),
            ("complete", Value::Bool(phase == "complete")),
            ("error", error.map(s).unwrap_or(Value::Null)),
        ]);
        let key = format!("lifecycle:{flow}");
        if self.reports.get(&key) != Some(&report) {
            self.reports.insert(key, report);
            self.changed = true;
        }
    }

    fn poll_active_builds(&mut self) {
        // Retiring Cargo remains owned so test recording tiles stay live,
        // but it cannot advance to another check/build/test phase.
        let retiring: Vec<_> = self
            .lifecycle_pending
            .keys()
            .filter_map(|flow| self.builds.remove(flow).map(|build| (flow.clone(), build)))
            .collect();
        self.poll_builds();
        self.builds.extend(retiring);
    }

    fn poll_lifecycle(&mut self) {
        let flows = self.lifecycle_pending.keys().cloned().collect::<Vec<_>>();
        for flow in flows {
            let Some(mut pending) = self.lifecycle_pending.remove(&flow) else {
                continue;
            };
            self.effects.retain(|effect| effect_flow(effect) != flow);
            let build_result = self
                .builds
                .get_mut(&flow)
                .map(|build| retire_lane_build(build, &mut pending))
                .unwrap_or(Ok(true));
            let build_stopped = match build_result {
                Ok(stopped) => stopped,
                Err(error) => {
                    self.lifecycle_report(
                        &flow,
                        pending.state,
                        "waiting for build exit",
                        Some(&error),
                    );
                    self.note = format!("{flow}: {error}");
                    self.lifecycle_pending.insert(flow, pending);
                    continue;
                }
            };
            if build_stopped {
                if let Some(build) = self.builds.remove(&flow) {
                    if let Err(error) = self.publish_build(
                        &flow,
                        &build,
                        "canceled",
                        false,
                        Some("Lane stopped before validation completed"),
                    ) {
                        self.lifecycle_report(
                            &flow,
                            pending.state,
                            "evidence write failed",
                            Some(&error),
                        );
                        self.lifecycle_pending.insert(flow, pending);
                        continue;
                    }
                }
            }
            let app_stopped = !self.apps.contains_key(&flow);
            let descendants_stopped = match if app_stopped {
                retire_lane_app_group(&mut pending)
            } else {
                Ok(false)
            } {
                Ok(stopped) => stopped,
                Err(error) => {
                    self.lifecycle_report(
                        &flow,
                        pending.state,
                        "waiting for app exit",
                        Some(&error),
                    );
                    self.note = format!("{flow}: {error}");
                    self.lifecycle_pending.insert(flow, pending);
                    continue;
                }
            };
            if !build_stopped || !app_stopped || !descendants_stopped {
                self.lifecycle_pending.insert(flow, pending);
                continue;
            }
            if let Some(error) = self.storage_error.clone() {
                self.lifecycle_report(&flow, pending.state, "evidence write failed", Some(&error));
                self.lifecycle_pending.insert(flow, pending);
                continue;
            }
            let state = pending.state;
            match self.observe(Observation::LifecycleChanged {
                flow: flow.clone(),
                state,
            }) {
                Ok(_) => {
                    self.lifecycle_report(&flow, state, "complete", None);
                    self.note = format!(
                        "{flow}: {}; history and local workspace preserved",
                        state.as_str()
                    );
                    if state == iteration::FlowLifecycle::Archived {
                        // Processes and the lease are gone; free the checkout
                        // unless it holds uncheckpointed work.
                        match self.release_workspace(&flow, false) {
                            Ok(None) => {
                                self.note = format!(
                                    "{flow}: archived; local workspace removed, branch and checkpoints kept"
                                );
                            }
                            Ok(Some(reason)) | Err(reason) => {
                                self.note = format!("{flow}: archived; local workspace retained: {reason}");
                            }
                        }
                    }
                    self.changed = true;
                    if let Err(error) = self.persist() {
                        self.note = format!(
                            "Lane state saved; lifecycle report could not be saved: {error}"
                        );
                    }
                }
                Err(error) => {
                    self.lifecycle_report(&flow, state, "waiting for reconciliation", Some(&error));
                    self.lifecycle_pending.insert(flow, pending);
                }
            }
        }
    }

    fn full_preview_file_stamp(&self, flow: &str, selection: &str) -> Option<FullPreviewStamp> {
        let tile = self.recordings.tiles.values().find(|tile| {
            self.recording_owner(tile) == flow
                && (tile.id == selection || tile.preview_id == selection)
        });
        let (path, sequence) = if let Some(tile) = tile {
            (tile.full_path.clone()?, tile.frame_sequence)
        } else {
            (self.evidence_preview_path(flow, selection).ok()?, 0)
        };
        Some(FullPreviewStamp {
            path: path.clone(),
            file: recording_file_stamp(&path, RECORDING_MAX_IMAGE_BYTES as u64).ok()?,
            sequence,
        })
    }

    fn select_full_preview(&mut self, selection: Option<String>) -> Result<Value, String> {
        if let Some(id) = selection {
            let flow = self
                .recordings
                .tiles
                .values()
                .find(|tile| tile.id == id || tile.preview_id == id)
                .map(|tile| self.recording_owner(tile).to_owned())
                .ok_or("Unknown recording tile")?;
            return self.select_image_preview(&flow, &id);
        }
        self.full_preview_selection = None;
        self.full_preview_stamp = None;
        self.full_preview = None;
        self.full_preview_error = None;
        self.changed = true;
        Ok(json::obj(vec![("selected", Value::Bool(false))]))
    }

    fn select_image_preview(&mut self, flow: &str, id: &str) -> Result<Value, String> {
        self.flow(flow)?;
        if !self.recordings.tiles.values().any(|tile| {
            self.recording_owner(tile) == flow && (tile.id == id || tile.preview_id == id)
        }) {
            self.evidence_preview_path(flow, id)?;
        }
        let selection = Some((flow.to_owned(), id.to_owned()));
        if self.full_preview_selection != selection {
            self.full_preview_selection = selection;
            self.full_preview_stamp = None;
            self.full_preview = None;
            self.full_preview_error = None;
            self.changed = true;
        }
        self.refresh_full_preview();
        Ok(json::obj(vec![
            (
                "selected",
                Value::Bool(self.full_preview_selection.is_some()),
            ),
            (
                "pending",
                Value::Bool(self.full_preview_selection.is_some() && self.full_preview.is_none()),
            ),
        ]))
    }

    fn refresh_full_preview(&mut self) {
        let Some((flow, id)) = self.full_preview_selection.clone() else {
            return;
        };
        let before = self.full_preview_file_stamp(&flow, &id);
        if before.is_some()
            && before == self.full_preview_stamp
            && (self.full_preview.is_some() || self.full_preview_error.is_some())
        {
            return;
        }
        let recording = self.recordings.tiles.values().any(|tile| {
            self.recording_owner(tile) == flow && (tile.id == id || tile.preview_id == id)
        });
        let result = if recording {
            self.load_full_recording_preview(&id)
        } else {
            self.load_full_evidence_preview(&flow, &id)
        };
        match result {
            Ok(preview) => {
                let after = self.full_preview_file_stamp(&flow, &id);
                // An atomic frame replacement may race the decode. Show the
                // valid decoded pixels, then retry if publication moved on.
                self.full_preview_stamp = if before == after { after } else { None };
                self.full_preview = Some(preview);
                self.full_preview_error = None;
                self.changed = true;
            }
            Err(error) => {
                // Avoid repeatedly decoding a stable invalid file. Atomic
                // replacement or a new frame sequence retries it; a missing
                // live frame has no stamp and keeps retrying publication.
                let after = self.full_preview_file_stamp(&flow, &id);
                self.full_preview_stamp = if before == after { after } else { None };
                if self.full_preview_error.as_ref() != Some(&error) {
                    self.full_preview_error = Some(error);
                    self.changed = true;
                }
            }
        }
    }
}

fn effect_flow(effect: &Effect) -> &str {
    match effect {
        Effect::CreateWorkspace { flow, .. }
        | Effect::RequestCheckpoint { flow, .. }
        | Effect::Build { flow, .. }
        | Effect::Launch { flow, .. }
        | Effect::RequestClose { flow, .. } => flow,
    }
}

fn retire_lane_build(build: &mut Build, pending: &mut PendingLifecycle) -> Result<bool, String> {
    let Some(child) = &mut build.child else {
        return Ok(true);
    };
    if !pending.build_term_sent {
        signal_owned_lane_group(child, false)?;
        pending.build_term_sent = true;
        build.tainted = Some("Lane stopped before validation completed".into());
    }
    let status = child.try_wait().map_err(err)?;
    let group_alive = owned_lane_group_alive(child.id(), status.is_none())?;
    if (status.is_none() || group_alive)
        && pending.started.elapsed() > Duration::from_secs(8)
        && !pending.build_kill_sent
    {
        signal_owned_lane_group(child, true)?;
        pending.build_kill_sent = true;
    }
    if let Some(status) = status.filter(|_| !group_alive) {
        build.failed_exit = status.code();
        build.child = None;
        return Ok(true);
    }
    Ok(false)
}

fn retire_lane_app_group(pending: &mut PendingLifecycle) -> Result<bool, String> {
    let Some(pid) = pending.app_group else {
        return Ok(true);
    };
    if !owned_lane_group_alive(pid, false)? {
        pending.app_group = None;
        return Ok(true);
    }
    #[cfg(unix)]
    {
        if pending.app_term_at.is_none() {
            signal_lane_group(pid, 15)?;
            pending.app_term_at = Some(Instant::now());
        } else if pending
            .app_term_at
            .is_some_and(|at| at.elapsed() > Duration::from_secs(8))
            && !pending.app_kill_sent
        {
            signal_lane_group(pid, 9)?;
            pending.app_kill_sent = true;
        }
    }
    Ok(false)
}

fn signal_owned_lane_group(child: &mut Child, force: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        signal_lane_group(child.id(), if force { 9 } else { 15 })
    }
    #[cfg(not(unix))]
    {
        let _ = force;
        if child.try_wait().map_err(err)?.is_none() {
            child.kill().map_err(err)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn signal_lane_group(pid: u32, signal: i32) -> Result<(), String> {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let pid = i32::try_from(pid).map_err(|_| "Owned process group ID exceeds platform bounds")?;
    if unsafe { kill(-pid, signal) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(3) {
        Ok(())
    } else {
        Err(format!("Cannot stop owned process group {pid}: {error}"))
    }
}

fn owned_lane_group_alive(pid: u32, leader_alive: bool) -> Result<bool, String> {
    #[cfg(unix)]
    {
        let _ = leader_alive;
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        let pid =
            i32::try_from(pid).map_err(|_| "Owned process group ID exceeds platform bounds")?;
        if unsafe { kill(-pid, 0) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(3) {
            Ok(false)
        } else {
            Err(format!("Cannot observe owned process group {pid}: {error}"))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Ok(leader_alive)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as u64
}
fn s(value: impl AsRef<str>) -> Value {
    json::s(value.as_ref())
}
fn host(
    directory: PathBuf,
    requests: Receiver<Envelope>,
    replies: SyncSender<Reply>,
    snapshots: SyncSender<Arc<Snapshot>>,
    stop: Arc<AtomicBool>,
    embedding_commands: SyncSender<crate::iteration_host::HostCommand>,
    spawner: ThreadSpawner,
) {
    let code = CodeRuntime::new(Some(spawner), Some(directory.join("code-cache")));
    let mut host = Host {
        directory,
        engine: Engine::default(),
        seen_feedback: BTreeSet::new(),
        effects: VecDeque::new(),
        cli_archive_cursor: 0,
        builds: BTreeMap::new(),
        apps: BTreeMap::new(),
        test_operations: BTreeMap::new(),
        lifecycle_pending: BTreeMap::new(),
        terminal_busy: BTreeSet::new(),
        fingerprints: BTreeMap::new(),
        reports: BTreeMap::new(),
        storage_error: None,
        attachments: Vec::new(),
        previews: BTreeMap::new(),
        preview_failed: BTreeSet::new(),
        terminal_heights: BTreeMap::new(),
        lane_widths: BTreeMap::new(),
        recordings: RecordingState::default(),
        full_preview: None,
        full_preview_selection: None,
        full_preview_stamp: None,
        full_preview_error: None,
        design_snapshots: BTreeMap::new(),
        embedding_commands,
        embedding_port: None,
        embedding_pending: BTreeMap::new(),
        embedding_unregister: BTreeSet::new(),
        code,
        note: String::new(),
        changed: true,
    };
    if let Err(error) = host.restore() {
        host.note = format!("History unavailable: {error}");
        host.storage_error = Some(error);
    }
    let mut http = match IterationHttp::start() {
        Ok(http) => Some(http),
        Err(error) => {
            host.note = format!("Lane HTTP unavailable: {error}");
            host.changed = true;
            None
        }
    };
    let mut pending = VecDeque::new();
    let mut source_at = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        while let Some(reply) = pending.pop_front() {
            match replies.try_send(reply) {
                Ok(()) => {}
                Err(TrySendError::Full(reply)) => {
                    pending.push_front(reply);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        if pending.len() < 32 {
            for _ in 0..8 {
                let Ok(envelope) = requests.try_recv() else {
                    break;
                };
                let result = host.request(envelope.request);
                if let Err(error) = &result {
                    host.note = error.clone();
                    host.changed = true;
                }
                pending.push_back(Reply {
                    id: envelope.id,
                    result,
                });
            }
        }
        if let Some(http) = &mut http {
            http.poll(&mut host);
        }
        if host.storage_error.is_none() {
            host.poll_cli();
            host.poll_code();
            host.poll_embedding();
        }
        for _ in 0..4 {
            let Some(effect) = host.effects.pop_front() else {
                break;
            };
            host.effect(effect);
        }
        host.poll_active_builds();
        host.poll_apps();
        host.poll_testing();
        host.poll_lifecycle();
        if host.storage_error.is_none() {
            host.ingest_feedback();
        }
        if host.storage_error.is_none() && source_at.elapsed() > Duration::from_secs(1) {
            host.observe_sources();
            host.load_previews();
            host.load_recordings();
            host.refresh_full_preview();
            source_at = Instant::now();
        }
        if host.changed {
            let snapshot = Arc::new(Snapshot {
                engine: host.engine.clone(),
                operations: Value::Arr(host.projected_reports()),
                note: host.note.clone(),
                attachments: host.projected_attachments(),
                previews: host.previews.values().cloned().collect(),
                presentation: host.presentation_json(),
                widths: host.lane_widths_json(),
                recordings: host.projected_recordings(),
                full_preview: host.full_preview.clone(),
                full_preview_selection: host.full_preview_selection.clone(),
                full_preview_error: host.full_preview_error.clone(),
                embedded: host.embed_summary(),
            });
            if snapshots.try_send(snapshot).is_ok() {
                host.changed = false;
                SignalToUI::set_ui_signal();
            }
        }
        if !pending.is_empty() {
            SignalToUI::set_ui_signal();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(http);
    host.code.shutdown();
    host.stop_embedding();
    let testing: Vec<_> = host.test_operations.keys().cloned().collect();
    for flow in testing {
        host.cancel_test_operation(&flow, "Studio host is shutting down");
    }
    for build in host.builds.values_mut() {
        if let Some(child) = &mut build.child {
            let _ = signal_owned_lane_group(child, true);
            let _ = child.wait();
        }
    }
    let flows: Vec<_> = host.apps.keys().cloned().collect();
    for flow in flows {
        let _ = host.close(&flow);
    }
    let until = Instant::now() + Duration::from_secs(8);
    while !host.apps.is_empty() && Instant::now() < until {
        host.poll_apps();
        host.poll_lifecycle();
        host.poll_embedding();
        host.ingest_feedback();
        std::thread::sleep(Duration::from_millis(50));
    }
    for run in host.apps.values_mut() {
        let _ = run.child.kill();
        if run.child.wait().is_ok() {
            if let Some(client) = run.embedding_client {
                host.embedding_unregister.insert(client);
            }
        }
    }
    host.poll_apps();
    host.poll_lifecycle();
    host.poll_embedding();
    if host.storage_error.is_none() {
        let _ = host.persist();
    }
}

impl Host {
    fn restore(&mut self) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(err)?;
        self.directory = self.directory.canonicalize().map_err(err)?;
        let path = self.directory.join("state.json");
        if !path.exists() {
            return Ok(());
        }
        let text = bounded_read(&path, 12 * 1024 * 1024)?;
        let value = json::parse(text.as_bytes()).map_err(str::to_owned)?;
        self.restore_media(&value)?;
        self.engine = Engine::decode(
            value
                .get("engine")
                .and_then(Value::as_str)
                .ok_or("Missing iteration engine")?,
        )?;
        if let Some(Value::Arr(ids)) = value.get("feedback_received") {
            for id in ids {
                if let Some(id) = id.as_str() {
                    self.seen_feedback.insert(id.into());
                }
            }
        }
        let ids: Vec<_> = self.engine.flows.keys().cloned().collect();
        for flow in ids {
            self.observe(Observation::Interrupted{flow,reason:"Studio restarted; prior process ownership must be reconciled before another build".into()})?;
        }
        Ok(())
    }
    fn persist(&self) -> Result<(), String> {
        let value = json::obj(vec![
            ("engine", s(self.engine.encode())),
            (
                "attachments",
                Value::Arr(self.attachments.iter().map(Attachment::json).collect()),
            ),
            ("presentation", self.presentation_json()),
            ("lane_widths", self.lane_widths_json()),
            (
                "design_snapshots",
                Value::Obj(
                    self.design_snapshots
                        .iter()
                        .map(|(run, (generation, id))| {
                            (
                                run.clone(),
                                Value::Arr(vec![Value::Int(*generation as i64), s(id)]),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "reports",
                Value::Obj(
                    self.reports
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                ),
            ),
            (
                "feedback_received",
                Value::Arr(self.seen_feedback.iter().map(s).collect()),
            ),
        ]);
        atomic_write(
            &self.directory.join("state.json"),
            value.to_json().as_bytes(),
        )
    }
    fn transition(&mut self, previous: Engine, transition: Transition) -> Result<Value, String> {
        if !transition.events.is_empty() {
            if let Err(error) = self.persist() {
                self.engine = previous;
                return Err(error);
            }
            // State is the recovery boundary. The append-only evidence log is
            // independently retained even when the in-memory overview is bounded.
            let archive = (|| {
                let mut log = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.directory.join("events.jsonl"))
                    .map_err(err)?;
                for event in transition.events {
                    writeln!(log, "{}", event.json().to_json()).map_err(err)?;
                }
                log.sync_data().map_err(err)
            })();
            // The atomic state already contains the complete durable event
            // history. An auxiliary log failure cannot undo accepted input.
            if let Err(error) = archive {
                self.note = format!("Event archive: {error}; history retained in state");
            }
        }
        self.effects.extend(transition.effects);
        self.changed = true;
        Ok(transition.result)
    }
    fn observe(&mut self, observation: Observation) -> Result<Value, String> {
        let previous = self.engine.clone();
        let transition = self.engine.observe(observation, now())?;
        self.transition(previous, transition)
    }
    fn flow(&self, id: &str) -> Result<&iteration::Flow, String> {
        self.engine
            .flows
            .get(id)
            .ok_or_else(|| "Unknown flow".into())
    }
    fn owned(&self, id: &str) -> Result<OwnedWorktree, String> {
        let flow = self.flow(id)?;
        let path = flow
            .worktree
            .as_ref()
            .ok_or("Local workspace unavailable")?;
        let state = git::inspect(path)?;
        let branch = state.branch.ok_or("Local workspace is detached")?;
        if !git::is_private_branch(&branch) {
            return Err("Flow source must stay on a private local branch".into());
        }
        Ok(OwnedWorktree {
            path: path.clone(),
            branch,
            common_dir: state.common_dir,
        })
    }
    /// Remove an archived lane's owned checkout through `makepad_git`,
    /// keeping its branch, checkpoint refs and resume identity. Returns
    /// `Ok(Some(reason))` when dirty work kept the checkout in place.
    fn release_workspace(&mut self, flow: &str, force: bool) -> Result<Option<String>, String> {
        let state = self.flow(flow)?;
        if state.lifecycle != iteration::FlowLifecycle::Archived {
            return Err("Only archived lanes release their local workspace".into());
        }
        if state.worktree.is_none() {
            return Ok(None);
        }
        if self.apps.contains_key(flow) || self.builds.contains_key(flow) {
            return Err("Owned processes are still running".into());
        }
        let owned = self.owned(flow)?;
        match git::remove_flow_worktree(&owned, force) {
            Ok(()) => {
                self.observe(Observation::WorkspaceReleased { flow: flow.into() })?;
                Ok(None)
            }
            Err(reason) if !force && reason.contains("uncommitted change") => Ok(Some(reason)),
            Err(error) => Err(error),
        }
    }
    fn request(&mut self, request: Request) -> Result<Value, String> {
        if let Some(error) = &self.storage_error {
            return Err(format!(
                "History must be recovered before accepting input: {error}"
            ));
        }
        let mutating_flow = match &request {
            Request::SplitLane { flow, .. } | Request::ClearHistory { flow } => Some(flow.as_str()),
            Request::FeedbackReport { flow, .. } => Some(flow.as_str()),
            Request::GitApply { flow, .. }
            | Request::SyncApply { flow, .. }
            | Request::ReleaseWorkspace { flow, .. }
            | Request::Fetch { flow } => Some(flow.as_str()),
            Request::Flow(
                FlowCommand::Todos { flow, .. }
                | FlowCommand::Prepared { flow, .. }
                | FlowCommand::Build { flow, .. },
            ) => Some(flow.as_str()),
            _ => None,
        };
        if mutating_flow.is_some_and(|flow| self.lifecycle_pending.contains_key(flow)) {
            return Err("Lane shutdown is in progress; code and Git commands are paused until its owned processes exit".into());
        }
        match request {
            Request::CodeTool { flow, tool, args } => {
                let control = cli_control_dir(&self.directory, &flow)?;
                let id = cli_request_id();
                match self.code_call(&flow, &flow, &id, &control, None, &tool, &args) {
                    CodeOutcome::Answered(envelope) => Ok(envelope.json()),
                    CodeOutcome::Deferred => Ok(json::obj(vec![
                        ("request_id", s(&id)),
                        ("flow_id", s(&flow)),
                        ("pending", Value::Bool(true)),
                    ])),
                }
            }
            Request::TerminalBusy { flow, busy } => {
                let owner = self.engine.resolve_active_flow(&flow)?.to_owned();
                if busy {
                    self.terminal_busy.insert(owner);
                } else {
                    self.terminal_busy.remove(&owner);
                }
                Ok(Value::Bool(true))
            }
            Request::SplitLane { flow, item, title } => self.split_lane(&flow, &item, &title),
            Request::ClearHistory { flow } => self.clear_lane_history(&flow),
            Request::FeedbackReport {
                flow,
                requirement,
                todos,
            } => self.feedback_report(&flow, requirement, todos),
            Request::Test { flow, action } => self.test_request(&flow, action),
            Request::SetLifecycle { flow, state } => self.set_lifecycle(&flow, state),
            Request::HostPort { port } => self.host_port(port),
            Request::HostRegistered { client } => self.host_registered(client),
            Request::FullPreview { recording } => self.select_full_preview(recording),
            Request::OpenImage { flow, preview_id } => {
                self.select_image_preview(&flow, &preview_id)
            }
            Request::DeleteVideos { flow } => self.delete_flow_videos(&flow),
            Request::ImportAttachment {
                flow,
                path,
                delivered,
            } => {
                if self.flow(&flow)?.successor.is_some() {
                    return Err(
                        "This archived prefix is read-only; add new input to its successor".into(),
                    );
                }
                self.import_attachment(&flow, &path, delivered)
            }
            Request::AttachmentDelivered { id } => self.attachment_delivered(&id),
            Request::ClearAttachmentTray { flow } => self.clear_attachment_tray(&flow),
            Request::LaneWidth { flow, width } => {
                self.flow(&flow)?;
                if !width.is_finite() {
                    return Err("Invalid lane width".into());
                }
                let width = width.clamp(420.0, 1600.0);
                if self.lane_widths.get(&flow) == Some(&width) {
                    return Ok(Value::Bool(true));
                }
                let previous = self.lane_widths.insert(flow.clone(), width);
                if let Err(error) = self.persist() {
                    if let Some(previous) = previous {
                        self.lane_widths.insert(flow, previous);
                    } else {
                        self.lane_widths.remove(&flow);
                    }
                    return Err(error);
                }
                self.changed = true;
                Ok(Value::Bool(true))
            }
            Request::TerminalHeight { flow, height } => {
                self.flow(&flow)?;
                if !height.is_finite() {
                    return Err("Invalid terminal height".into());
                }
                self.terminal_heights
                    .insert(flow, height.clamp(64.0, 640.0));
                self.persist()?;
                self.changed = true;
                Ok(Value::Bool(true))
            }
            Request::PopOut { flow, run_id } => {
                self.require_active_lane(&flow)?;
                self.pop_out(&flow, &run_id)?;
                Ok(json::obj(vec![("requested", Value::Bool(true))]))
            }
            Request::Flow(command) => {
                if let FlowCommand::Build { flow, .. }
                | FlowCommand::Prepared { flow, .. }
                | FlowCommand::Todos { flow, .. } = &command
                {
                    self.require_active_lane(flow)?;
                }
                let previous = self.engine.clone();
                let transition = self.engine.apply(command, now())?;
                let value = self.transition(previous, transition)?;
                Ok(value)
            }
            Request::Close { flow } => {
                self.close(&flow)?;
                Ok(json::obj(vec![(
                    "state",
                    s("closing; build waits for observed process exit"),
                )]))
            }
            Request::ReleaseWorkspace { flow, force } => {
                let flow_state = self.flow(&flow)?;
                if flow_state.lifecycle != iteration::FlowLifecycle::Archived {
                    return Err("Archive the lane before releasing its local workspace".into());
                }
                if self.lifecycle_pending.contains_key(&flow) {
                    return Err("Lane shutdown is still in progress".into());
                }
                let retained = self.release_workspace(&flow, force)?;
                Ok(json::obj(vec![
                    ("flow", s(&flow)),
                    ("released", Value::Bool(retained.is_none())),
                    ("retained", retained.map(s).unwrap_or(Value::Null)),
                ]))
            }
            Request::GitInspect { flow } => {
                let owned = self.owned(&flow)?;
                let state = git::inspect(&owned.path)?;
                Ok(json::obj(vec![
                    ("branch", s(&owned.branch)),
                    ("head", s(&state.head)),
                    ("private", Value::Bool(true)),
                    ("push_allowed", Value::Bool(false)),
                    (
                        "changed_paths",
                        Value::Arr(state.changes.iter().map(|p| s(&p.path)).collect()),
                    ),
                ]))
            }
            Request::GitPreview { flow, target } => {
                let owned = self.owned(&flow)?;
                let source = if target == "dev" {
                    "work"
                } else {
                    &owned.branch
                };
                let preview = git::preview_promotion(&owned.path, source, &target)?;
                Ok(promotion_json(&preview))
            }
            Request::GitApply {
                flow,
                target,
                source,
                destination,
                title,
            } => {
                if self.apps.contains_key(&flow) || self.builds.contains_key(&flow) {
                    return Err(
                        "Close the flow app and finish compilation before source promotion".into(),
                    );
                }
                let owned = self.owned(&flow)?;
                let branch = if target == "dev" {
                    "work"
                } else {
                    &owned.branch
                };
                let preview = git::preview_promotion(&owned.path, branch, &target)?;
                if preview.source_oid != source || preview.target_oid != destination {
                    return Err("Promotion preview is stale".into());
                }
                // Merged-tree validation is mandatory. Never substitute a prior
                // artifact's passing status for a new integration tree.
                let validated = self.reports.values().any(|report| {
                    report.get("validated_tree").and_then(Value::as_str)
                        == Some(preview.result_tree.as_str())
                        && report.get("complete") == Some(&Value::Bool(true))
                });
                if !validated {
                    return Err("Validate the exact promotion result tree before committing this feature/milestone".into());
                }
                let target_worktree = git::ensure_promotion_worktree(
                    &owned.path,
                    &self.directory.join("integration").join(&target),
                    &target,
                )?;
                let commit = git::apply_promotion(&preview, &target_worktree, &title)?;
                Ok(json::obj(vec![
                    ("commit", s(commit.commit)),
                    ("branch", s(target)),
                    ("published", Value::Bool(false)),
                ]))
            }
            Request::SyncPreview {
                flow,
                source,
                target,
            } => {
                let owned = self.owned(&flow)?;
                let preview = git::preview_sync(&owned.path, &source, &target)?;
                Ok(sync_json(&preview))
            }
            Request::SyncApply {
                flow,
                source,
                target,
                source_oid,
                target_oid,
            } => {
                if self.apps.contains_key(&flow) || self.builds.contains_key(&flow) {
                    return Err("Close the flow app and finish compilation before sync".into());
                }
                let owned = self.owned(&flow)?;
                let preview = git::preview_sync(&owned.path, &source, &target)?;
                if preview.source_oid != source_oid || preview.target_oid != target_oid {
                    return Err("Sync preview is stale".into());
                }
                let target_worktree = if target == owned.branch {
                    owned
                } else {
                    git::ensure_promotion_worktree(
                        &owned.path,
                        &self.directory.join("integration").join(&target),
                        &target,
                    )?
                };
                let result = git::apply_sync(
                    &preview,
                    &target_worktree,
                    "Sync public changes for Studio iteration",
                )?;
                Ok(json::obj(vec![("result", s(format!("{result:?}")))]))
            }
            Request::Fetch { flow } => {
                let owned = self.owned(&flow)?;
                git::fetch_public_refs(&owned.path)?;
                Ok(json::obj(vec![
                    ("fetched", Value::Bool(true)),
                    ("pushed", Value::Bool(false)),
                ]))
            }
            Request::Diff { flow, artifact } => {
                let source = if artifact.is_some() {
                    self.engine.resolve_active_flow(&flow)?
                } else {
                    &flow
                };
                let owned = self.owned(source)?;
                let state = self.flow(&flow)?;
                let (from, to) = if let Some(id) = artifact {
                    let index = state
                        .artifacts
                        .iter()
                        .position(|a| a.id == id)
                        .ok_or("Unknown artifact")?;
                    let current = &state.artifacts[index];
                    let parent = if index > 0 {
                        state.artifacts[index - 1].commit.clone()
                    } else {
                        git::parent_commit(&owned.path, &current.commit)?
                    };
                    (parent, current.commit.clone())
                } else {
                    ("HEAD".into(), String::new())
                };
                let diff = git::unified_diff(
                    &owned.path,
                    &from,
                    if to.is_empty() { None } else { Some(to.as_str()) },
                )?;
                Ok(json::obj(vec![
                    ("from", s(from)),
                    (
                        "to",
                        if to.is_empty() {
                            s("unbuilt working source")
                        } else {
                            s(to)
                        },
                    ),
                    ("diff", s(diff)),
                ]))
            }
        }
    }
    fn effect(&mut self, effect: Effect) {
        if self.require_active_lane(effect_flow(&effect)).is_err() {
            return;
        }
        let result: Result<(), String> = (|| {
            match effect {
                Effect::CreateWorkspace { flow, config } => {
                    let branch = if self.engine.flows.len() == 1 {
                        "local".into()
                    } else {
                        format!("local-{flow}")
                    };
                    let guard =
                        std::env::current_exe()
                            .map_err(err)?
                            .with_file_name(if cfg!(windows) {
                                "studio-git-guard.exe"
                            } else {
                                "studio-git-guard"
                            });
                    let destination = self.directory.join("worktrees").join(&flow);
                    match git::ensure_flow_worktree(&config.repo, &destination, &branch, &guard) {
                        Ok(owned) => {
                            cli_control_dir(&self.directory, &flow)?;
                            self.observe(Observation::WorkspaceReady {
                                flow,
                                path: owned.path,
                            })?;
                        }
                        Err(error) => {
                            self.observe(Observation::WorkspaceFailed { flow, error })?;
                        }
                    }
                }
                Effect::RequestCheckpoint { flow, job_id, .. } => {
                    if let Err(error) = self.start_preflight(&flow, &job_id) {
                        self.observe(Observation::BuildFailed {
                            flow,
                            job_id,
                            exit_code: None,
                            error,
                        })?;
                    }
                }
                Effect::Build {
                    flow,
                    job_id,
                    commit,
                    ..
                } => match self.prepare_build(&flow, &job_id, &commit) {
                    Ok(build) => {
                        self.observe(Observation::BuildStarted {
                            flow: flow.clone(),
                            job_id,
                        })?;
                        self.builds.insert(flow, build);
                    }
                    Err(error) => {
                        self.observe(Observation::BuildFailed {
                            flow,
                            job_id,
                            exit_code: None,
                            error,
                        })?;
                    }
                },
                Effect::Launch {
                    flow,
                    job_id,
                    artifact_id,
                    path,
                    mode,
                } => match self.launch(&flow, &artifact_id, &path, mode) {
                    Ok(()) => {}
                    Err(error) => {
                        self.observe(Observation::LaunchFailed {
                            flow,
                            job_id,
                            artifact_id,
                            error,
                        })?;
                    }
                },
                Effect::RequestClose { flow, .. } => {
                    self.note = format!(
                        "{flow}: changes prepared; waiting for you to close the running app"
                    );
                    self.changed = true;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.note = error;
            self.changed = true;
        }
    }
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl Host {
    fn ingest_feedback(&mut self) {
        let engine = &self.engine;
        let runs: Vec<_> = self
            .engine
            .flows
            .values()
            .flat_map(|flow| {
                flow.runs
                    .iter()
                    .filter(move |run| {
                        engine.evidence_origin("run", &run.id) == Some(flow.id.as_str())
                    })
                    .map(move |run| (flow.id.clone(), run.id.clone(), run.artifact_id.clone()))
            })
            .collect();
        for (flow, run, artifact) in runs {
            let directory = self.directory.join("runs").join(&run).join("feedback");
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten().take(256) {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.ends_with(".json") || name.ends_with(".ack.json") {
                    continue;
                }
                if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
                    continue;
                }
                let result = self.receive_feedback(&flow, &run, &artifact, &directory, &path);
                if let Err(error) = result {
                    let note = format!("Feedback pending for {flow}: {error}");
                    if self.note != note {
                        self.note = note;
                        self.changed = true;
                    }
                }
            }
        }
    }
    fn receive_feedback(
        &mut self,
        flow: &str,
        run: &str,
        artifact: &str,
        directory: &Path,
        path: &Path,
    ) -> Result<(), String> {
        let text = bounded_read(path, 128 * 1024)?;
        let event = json::parse(text.as_bytes()).map_err(str::to_owned)?;
        let field = |name: &str| {
            event
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Missing feedback {name}"))
        };
        let id = field("event_id")?;
        if id.is_empty()
            || id.len() > 80
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err("Invalid feedback identity".into());
        }
        if event.get("schema_version") != Some(&Value::Int(1))
            || field("kind")? != "feedback"
            || field("flow_id")? != flow
            || field("run_id")? != run
            || field("artifact_id")? != artifact
        {
            return Err("Feedback does not match its owned app run".into());
        }
        let successor = self.engine.resolve_active_flow(flow)?.to_owned();
        let flow = successor.as_str();
        let revision = &self
            .flow(flow)?
            .artifacts
            .iter()
            .find(|a| a.id == artifact)
            .ok_or("Unknown artifact")?
            .commit;
        if field("revision")? != revision {
            return Err("Feedback source revision differs from the recorded artifact".into());
        }
        let key = format!("{run}:{id}");
        let ack = directory.join(format!("{id}.ack.json"));
        if self.seen_feedback.contains(&key) {
            if !ack.exists() {
                atomic_write(&ack, b"{\"accepted\":true}")?;
            }
            return Ok(());
        }
        if self.seen_feedback.len() >= 65536 {
            return Err(
                "Feedback receipt index is full; archive old flows before continuing".into(),
            );
        }
        let design_generation =
            if event.get("delivery").and_then(Value::as_str) == Some("design_tweak") {
                Some(
                    event
                        .get("tweak_generation")
                        .and_then(Value::as_u64)
                        .ok_or("Missing F12 generation")?,
                )
            } else {
                None
            };
        let previous_design = self.design_snapshots.get(run).cloned();
        if design_generation.is_some_and(|generation| {
            previous_design
                .as_ref()
                .is_some_and(|(latest, _)| generation <= *latest)
        }) {
            self.seen_feedback.insert(key.clone());
            if let Err(error) = self.persist() {
                self.seen_feedback.remove(&key);
                return Err(error);
            }
            return atomic_write(&ack, b"{\"accepted\":true,\"superseded\":true}");
        }
        let mut evidence = Vec::new();
        let previous = self.engine.clone();
        let mut events = Vec::new();
        let result = (|| {
            if let Some(image) = event.get("image").filter(|v| !matches!(v, Value::Null)) {
                let name = image
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or("Missing capture filename")?;
                if !matches!(
                    Path::new(name).components().collect::<Vec<_>>().as_slice(),
                    [std::path::Component::Normal(_)]
                ) {
                    return Err("Capture must be a filename in its run spool".into());
                }
                let capture_path = directory.join(name);
                let metadata = fs::symlink_metadata(&capture_path).map_err(err)?;
                if !metadata.file_type().is_file() || metadata.len() > 64 * 1024 * 1024 {
                    return Err("Capture is not a bounded regular PNG".into());
                }
                let mut header = [0u8; 24];
                File::open(&capture_path)
                    .map_err(err)?
                    .read_exact(&mut header)
                    .map_err(err)?;
                if &header[..8] != b"\x89PNG\r\n\x1a\n" || &header[12..16] != b"IHDR" {
                    return Err("Capture is not a PNG".into());
                }
                let width = u32::from_be_bytes(header[16..20].try_into().map_err(err)?);
                let height = u32::from_be_bytes(header[20..24].try_into().map_err(err)?);
                if width == 0
                    || height == 0
                    || u64::from(width) * u64::from(height) > 32 * 1024 * 1024
                    || image.get("width") != Some(&Value::Int(width.into()))
                    || image.get("height") != Some(&Value::Int(height.into()))
                {
                    return Err("Capture dimensions do not match its image".into());
                }
                let capture_id = format!("capture-{id}");
                let capture = iteration::Capture {
                    id: capture_id.clone(),
                    artifact_id: artifact.into(),
                    run_id: run.into(),
                    path: capture_path,
                    width,
                    height,
                    timestamp_ms: now(),
                };
                let transition = self.engine.observe(
                    Observation::Captured {
                        flow: flow.into(),
                        capture,
                    },
                    now(),
                )?;
                events.extend(transition.events);
                evidence.push(iteration::EvidenceRef {
                    capture_id,
                    region: None,
                });
            }
            let message = field("message")?;
            let text = if design_generation.is_some() {
                format!(
                    "F12 current design state for {run}\n{message}\nFull property evidence: {}",
                    path.display()
                )
            } else {
                message.into()
            };
            let transition = self.engine.apply(
                FlowCommand::Requirement {
                    flow: flow.into(),
                    id: design_generation.and(previous_design.as_ref().map(|(_, id)| id.clone())),
                    text,
                },
                now(),
            )?;
            events.extend(transition.events);
            let feedback = iteration::Feedback {
                artifact_id: artifact.into(),
                run_id: run.into(),
                category: "requirement".into(),
                summary: field("message")?.into(),
                evidence,
            };
            let transition = self.engine.observe(
                Observation::HumanFeedback {
                    flow: flow.into(),
                    feedback,
                },
                now(),
            )?;
            events.extend(transition.events);
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            self.engine = previous;
            return Err(error);
        }
        if let Some(generation) = design_generation {
            let requirement_id = previous_design
                .as_ref()
                .map(|(_, id)| id.clone())
                .or_else(|| {
                    self.engine
                        .flows
                        .get(flow)?
                        .requirements
                        .last()
                        .map(|r| r.id.clone())
                })
                .ok_or("F12 requirement missing after acceptance")?;
            self.design_snapshots
                .insert(run.into(), (generation, requirement_id));
        }
        let attachment_count = self.attachments.len();
        if event.get("delivery").and_then(Value::as_str) == Some("terminal_drop") {
            if self.attachments.len() >= 512 {
                self.engine = previous;
                return Err("Attachment history is full".into());
            }
            if let Some(capture) = self
                .engine
                .flows
                .get(flow)
                .and_then(|f| f.captures.iter().find(|c| c.id == format!("capture-{id}")))
            {
                self.attachments.push(Attachment {
                    id: capture.id.clone(),
                    flow: flow.into(),
                    path: capture.path.clone(),
                    delivered: false,
                    submitted: false,
                });
            }
        }
        self.seen_feedback.insert(key.clone());
        if let Err(error) = self.transition(
            previous,
            Transition {
                result: Value::Null,
                effects: vec![],
                events,
            },
        ) {
            self.seen_feedback.remove(&key);
            self.attachments.truncate(attachment_count);
            if let Some(previous) = previous_design {
                self.design_snapshots.insert(run.into(), previous);
            } else {
                self.design_snapshots.remove(run);
            }
            return Err(error);
        }
        atomic_write(&ack, b"{\"accepted\":true}")?;
        self.note = format!("{flow}: F10 feedback added to the next revision");
        self.changed = true;
        Ok(())
    }
}
fn bounded_read(path: &Path, limit: usize) -> Result<String, String> {
    let file = File::open(path).map_err(err)?;
    if file.metadata().map_err(err)?.len() > limit as u64 {
        return Err(format!("{} exceeds its read limit", path.display()));
    }
    let mut text = String::new();
    file.take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(err)?;
    if text.len() > limit {
        return Err("File grew beyond its read limit".into());
    }
    Ok(text)
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("No parent directory")?;
    fs::create_dir_all(parent).map_err(err)?;
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "tmp-{}-{}-{}",
        std::process::id(),
        now(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(err)?;
    let result = (|| {
        file.write_all(bytes).map_err(err)?;
        file.sync_all().map_err(err)?;
        fs::rename(&temp, path).map_err(err)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
fn promotion_json(p: &git::PromotionPreview) -> Value {
    json::obj(vec![
        ("source", s(&p.source_branch)),
        ("target", s(&p.target_branch)),
        ("source_oid", s(&p.source_oid)),
        ("target_oid", s(&p.target_oid)),
        ("result_tree", s(&p.result_tree)),
        ("diff", s(&p.diff_stat)),
        ("paths", Value::Arr(p.changed_paths.iter().map(s).collect())),
        ("published", Value::Bool(false)),
    ])
}
fn sync_json(p: &git::SyncPreview) -> Value {
    json::obj(vec![
        ("source", s(&p.source_ref)),
        ("target", s(&p.target_branch)),
        ("source_oid", s(&p.source_oid)),
        ("target_oid", s(&p.target_oid)),
        ("result_tree", s(&p.result_tree)),
        ("fast_forward", Value::Bool(p.fast_forward)),
        ("diff", s(&p.diff_stat)),
    ])
}
