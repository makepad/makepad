//! Platform filesystem watcher: one callback per observed change, stamped
//! with a per-watcher sequence number, an ownership epoch and receipt times.
//!
//! Filesystem watching is an OS-only service and is never linked into wasm
//! builds.
//!
//! # What each backend delivers (derived from the backend sources)
//!
//! Every backend runs on its own thread(s) and hands observations to the
//! shared stamping layer, which serialises admission: sequence numbers are
//! allocated and the consumer is called under one lock, so callbacks never
//! run out of sequence order even when a backend has one thread per root.
//! The shared layer stamps every event with a strictly increasing `seq`,
//! the current `epoch`, `received_at` (monotonic) and `wall_time`. Events
//! of one watcher are therefore totally ordered by `seq` in callback
//! order; nothing else about ordering is promised across roots.
//!
//! Every backend applies the watcher's [`ExcludePolicy`] (by default
//! `.git` and `target*` at any depth) before traversal and before any
//! probe: excluded paths are never walked, never stat'ed, never reported.
//!
//! * **macOS** (`macos.rs`, FSEvents): one stream per root with file-level
//!   events and a 0.1 s latency window. Within that window FSEvents
//!   coalesces flags per path, and flags such as *created* or *renamed* can
//!   stay attached to later events for the same path, so flags alone are
//!   not trusted. The kind is derived from **a metadata probe at receipt
//!   time plus the backend's inventory** (identity per path, seeded per
//!   directory on first touch and breadth-first from the roots within a
//!   budget at start, maintained from the events themselves): a path that
//!   no longer exists is `Removed`; a *renamed* missing path is paired to
//!   the one renamed path in the same batch that now carries the identity
//!   the missing path had (`Renamed`), otherwise the halves degrade to
//!   `Removed` + `Created`; an existing path unknown to the inventory is
//!   `Created`; a known path is `Changed`. Records are classified in
//!   stream-id order. The *must-scan-subdirs*, *user-dropped*,
//!   *kernel-dropped*, *event-ids-wrapped* and *history-done* flags become
//!   `RescanRequired { reason: Overflow }`, *root-changed* becomes
//!   `RescanRequired { reason: RootChanged }`.
//!   If FSEvents cannot start, the backend falls back to a 220 ms polling
//!   loop that emits `Changed` for the *root path* whenever a tree
//!   fingerprint (names, sizes, mtimes of regular files and real
//!   directories; symlinks are never followed; exclusions honoured) changes,
//!   plus one forced `Changed` per root every second; that fallback never
//!   reports per-file kinds.
//! * **Linux** (`linux.rs`, inotify): one watch per directory, recursively
//!   (re-scanned when a directory is created, moved or deleted), skipping
//!   excluded directories. The watches are installed before `start`
//!   returns. Kinds map from the mask exactly: `IN_CREATE` → `Created`,
//!   `IN_DELETE` → `Removed`, `IN_MOVED_FROM`/`IN_MOVED_TO` with the same
//!   cookie inside one `read` → `Renamed`, an unpaired `IN_MOVED_FROM` →
//!   `Removed`, an unpaired `IN_MOVED_TO` → `Created`,
//!   `IN_MODIFY`/`IN_CLOSE_WRITE`/`IN_ATTRIB` → `Changed`,
//!   `IN_DELETE_SELF`/`IN_MOVE_SELF` → `Removed` for the directory.
//!   `IN_MODIFY` fires per `write(2)`, so a save can produce several
//!   `Changed` events; identical (mount, path, kind) triples are
//!   deduplicated only within one `read`. `IN_Q_OVERFLOW` (which arrives
//!   without a watch descriptor) rebuilds the watch coverage of every root
//!   (directories created while events were lost gain watches) and then
//!   becomes `RescanRequired { reason: Overflow, missing }` for every root,
//!   where `missing` is the wall-clock interval between the last successful
//!   read and the overflow. Symlinked directories are never watched.
//!   Delivery order is the kernel's order.
//! * **Windows** (`windows.rs`, `ReadDirectoryChangesW`): one synchronous
//!   reader thread per root, recursive; a root that is a reparse point is
//!   refused and `start` fails. `FILE_NOTIFY_INFORMATION` records map to
//!   `Created`/`Removed`/`Changed`; an *old name* record directly followed
//!   by a *new name* record inside one buffer becomes `Renamed`, otherwise
//!   the halves become `Removed`/`Created`. A read that returns zero bytes,
//!   or `ERROR_NOTIFY_ENUM_DIR`, means the OS dropped changes and becomes
//!   `RescanRequired { reason: Overflow }` for the root. `is_dir` comes
//!   from `symlink_metadata`, so reparse points are never followed.
//!   Records are delivered in buffer order; admission is serialised across
//!   the root threads by the stamping layer.
//!
//! # Journal
//!
//! Every stamped observation is recorded in the watcher's [`Journal`]
//! before it is forwarded. Forwarding is bounded: a consumer started with
//! [`FileSystemWatcher::start_with_journal`] returns `false` from its
//! callback to refuse an event for now; the event waits in the journal's
//! backlog behind a forwarding cursor and is retried in order on the next
//! observation or on [`Journal::pump`]. When the backlog outgrows the
//! journal's byte budget the oldest unforwarded records are dropped, a
//! sticky [`Gap`] is recorded and a `RescanRequired { reason: JournalGap }`
//! is emitted per root in-band. The legacy [`FileSystemWatcher::start`]
//! callback always admits.
//!
//! # Settling
//!
//! Raw events are not safe to act on: files are written in pieces, saved
//! several times per second, or replaced through a temporary. [`Settler`]
//! turns the raw stream into settled observations (one per stable content
//! version with the bytes and their git blob id retained, delete tombstones
//! held through settlement, atomic replacement as one update, `Unchanged`
//! for a re-settlement on the same bytes, `StillChanging` with capped
//! backoff for files that never quiesce). The consumer feeds it events and
//! polls it with a clock and a reader, so it is testable without a
//! filesystem.
//!
//! # Brackets: startup and root changes
//!
//! `start_with_journal` returns only once the backend is watching, and
//! the first events of epoch 0 are one `RescanRequired { reason: Startup }`
//! per root: a consumer brackets its initial inventory between them and
//! the events that follow, nothing observed after the marker predates the
//! inventory's coverage. `set_roots` replaces the watched roots without
//! stopping the watcher from the caller's point of view: the exchange runs
//! on the watcher's manager thread, the old backend is stopped and joined
//! **before** the epoch is bumped, so every event with the old epoch
//! belongs to the old roots and every event with the new epoch to the new
//! ones. Observations the replacement backend produces while the exchange
//! completes are buffered; `set_roots` returns after the replacement
//! backend is watching, and the first events of the new epoch are one
//! `RescanRequired { reason: Restart }` per new root, followed by the
//! buffered observations in order. `start` establishes epoch 0.

#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Instant, SystemTime};

#[derive(Clone, Debug)]
pub struct WatchRoot {
    pub mount: String,
    pub path: PathBuf,
}

/// Why a consumer must re-inventory a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RescanReason {
    /// The watcher just started watching: bracket the initial inventory.
    Startup,
    /// The root set was replaced: bracket the reconciliation of the new roots.
    Restart,
    /// The backend lost events (queue overflow, coalescing, dropped records).
    Overflow,
    /// The root itself changed underneath the backend.
    RootChanged,
    /// The journal dropped unforwarded observations (see [`Journal::gap`]).
    JournalGap,
}

/// What happened to `FileSystemEvent::path`, as far as the backend can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileSystemEventKind {
    /// Contents or metadata changed, or the backend cannot be more precise.
    Changed,
    /// The path appeared.
    Created,
    /// The path disappeared.
    Removed,
    /// The path is the new name; `from` is the old one.
    Renamed { from: PathBuf },
    /// The backend lost events, the root changed underneath it, or a
    /// bracket opened: the consumer must re-inventory `root`. `missing` is
    /// the wall-clock interval during which observations may be lost, when
    /// the backend can bound it.
    RescanRequired {
        root: PathBuf,
        reason: RescanReason,
        missing: Option<(SystemTime, SystemTime)>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSystemEvent {
    pub mount: String,
    pub path: PathBuf,
    pub kind: FileSystemEventKind,
    /// Strictly increasing per watcher, in callback order.
    pub seq: u64,
    /// The root-set generation this event was observed under.
    pub epoch: u64,
    pub received_at: Instant,
    pub wall_time: SystemTime,
    /// `None` when the path no longer exists or the backend did not say.
    pub is_dir: Option<bool>,
}

/// One observation a backend produced while the stamping layer was holding
/// admission (a root exchange in progress).
struct Held {
    mount: String,
    path: PathBuf,
    kind: FileSystemEventKind,
    is_dir: Option<bool>,
    received_at: Instant,
    wall_time: SystemTime,
}

struct EmitterState {
    seq: u64,
    held: Option<Vec<Held>>,
    roots: Vec<WatchRoot>,
}

/// The stamping layer between a backend and the consumer: serialised
/// admission, sequence and epoch stamps, journal-before-forward.
pub(crate) struct Emitter {
    state: Mutex<EmitterState>,
    epoch: AtomicU64,
    exclude: ExcludePolicy,
    journal: Journal,
}

impl Emitter {
    fn new(exclude: ExcludePolicy, journal: Journal) -> Self {
        Self {
            state: Mutex::new(EmitterState {
                seq: 0,
                held: None,
                roots: Vec::new(),
            }),
            epoch: AtomicU64::new(0),
            exclude,
            journal,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, EmitterState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn exclude(&self) -> &ExcludePolicy {
        &self.exclude
    }

    pub(crate) fn emit(
        &self,
        mount: String,
        path: PathBuf,
        kind: FileSystemEventKind,
        is_dir: Option<bool>,
    ) {
        let received_at = Instant::now();
        let wall_time = SystemTime::now();
        let mut state = self.lock();
        let held = Held {
            mount,
            path,
            kind,
            is_dir,
            received_at,
            wall_time,
        };
        if let Some(buffer) = state.held.as_mut() {
            buffer.push(held);
            return;
        }
        self.deliver(&mut state, held);
    }

    fn deliver(&self, state: &mut EmitterState, held: Held) {
        state.seq += 1;
        let event = FileSystemEvent {
            mount: held.mount,
            path: held.path,
            kind: held.kind,
            seq: state.seq,
            epoch: self.epoch.load(Ordering::Acquire),
            received_at: held.received_at,
            wall_time: held.wall_time,
            is_dir: held.is_dir,
        };
        if self.journal.admit(event) {
            // The journal dropped unforwarded observations: say so in-band
            // for every root. A gap opened by these markers themselves is
            // already sticky on the journal and not chased further.
            let roots = state.roots.clone();
            for root in roots {
                state.seq += 1;
                let _ = self.journal.admit(FileSystemEvent {
                    mount: root.mount,
                    path: root.path.clone(),
                    kind: FileSystemEventKind::RescanRequired {
                        root: root.path,
                        reason: RescanReason::JournalGap,
                        missing: None,
                    },
                    seq: state.seq,
                    epoch: self.epoch.load(Ordering::Acquire),
                    received_at: Instant::now(),
                    wall_time: SystemTime::now(),
                    is_dir: Some(true),
                });
            }
        }
    }

    fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Release);
    }

    fn set_roots(&self, roots: Vec<WatchRoot>) {
        self.lock().roots = roots;
    }

    /// Buffer observations until `release`.
    fn hold(&self) {
        let mut state = self.lock();
        if state.held.is_none() {
            state.held = Some(Vec::new());
        }
    }

    /// Deliver `markers` first, then every observation buffered since
    /// `hold`, in order, and resume direct delivery.
    fn release(&self, markers: Vec<(WatchRoot, RescanReason)>) {
        let mut state = self.lock();
        let buffered = state.held.take().unwrap_or_default();
        for (root, reason) in markers {
            self.deliver(
                &mut state,
                Held {
                    mount: root.mount,
                    path: root.path.clone(),
                    kind: FileSystemEventKind::RescanRequired {
                        root: root.path,
                        reason,
                        missing: None,
                    },
                    is_dir: Some(true),
                    received_at: Instant::now(),
                    wall_time: SystemTime::now(),
                },
            );
        }
        for held in buffered {
            self.deliver(&mut state, held);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as imp;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use unsupported as imp;

enum Control {
    SetRoots {
        roots: Vec<WatchRoot>,
        epoch: u64,
        done: mpsc::Sender<Result<(), String>>,
    },
    Stop,
}

/// How a watcher is configured beyond its roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatcherConfig {
    pub exclude: ExcludePolicy,
    pub journal: JournalConfig,
    /// Emit one `RescanRequired { reason: Startup }` per root once the
    /// backend is watching, so the consumer's initial inventory is
    /// bracketed.
    pub startup_rescan: bool,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            exclude: ExcludePolicy::default(),
            journal: JournalConfig::default(),
            startup_rescan: true,
        }
    }
}

pub struct FileSystemWatcher {
    control: mpsc::Sender<Control>,
    emitter: Arc<Emitter>,
    next_epoch: AtomicU64,
    last_error: Arc<Mutex<Option<String>>>,
    manager: Option<JoinHandle<()>>,
    journal: Journal,
}

impl FileSystemWatcher {
    /// Start watching `roots` at epoch 0 with the default exclusions and a
    /// small journal, without a startup bracket. The callback always
    /// admits. Fails when the backend cannot start.
    pub fn start<F>(roots: Vec<WatchRoot>, on_event: F) -> Result<Self, String>
    where
        F: Fn(FileSystemEvent) + Send + Sync + 'static,
    {
        let config = WatcherConfig {
            journal: JournalConfig {
                ring_bytes: 1024 * 1024,
                spool: None,
            },
            startup_rescan: false,
            ..WatcherConfig::default()
        };
        let (watcher, _journal) = Self::start_with_journal(roots, config, move |event| {
            on_event(event.clone());
            true
        })?;
        Ok(watcher)
    }

    /// Start watching `roots` at epoch 0 with journal-before-forward
    /// intake. `forward` returns whether it admitted the event; a refused
    /// event waits in the journal's backlog and is retried in order. The
    /// call returns once the backend is watching; with
    /// `config.startup_rescan` the first events are the startup bracket.
    pub fn start_with_journal<F>(
        roots: Vec<WatchRoot>,
        config: WatcherConfig,
        forward: F,
    ) -> Result<(Self, Journal), String>
    where
        F: Fn(&FileSystemEvent) -> bool + Send + Sync + 'static,
    {
        let journal = Journal::new(config.journal, Arc::new(forward))?;
        let emitter = Arc::new(Emitter::new(config.exclude, journal.clone()));
        emitter.set_roots(roots.clone());
        emitter.hold();
        let backend = match imp::PlatformWatcher::start(roots.clone(), Arc::clone(&emitter)) {
            Ok(backend) => backend,
            Err(error) => {
                emitter.release(Vec::new());
                return Err(error);
            }
        };
        let markers = if config.startup_rescan {
            roots
                .iter()
                .map(|root| (root.clone(), RescanReason::Startup))
                .collect()
        } else {
            Vec::new()
        };
        emitter.release(markers);
        let (control, commands) = mpsc::channel();
        let last_error = Arc::new(Mutex::new(None));
        let manager = {
            let emitter = Arc::clone(&emitter);
            let last_error = Arc::clone(&last_error);
            thread::Builder::new()
                .name("fswatch-manager".to_string())
                .spawn(move || manager_loop(backend, commands, emitter, last_error))
                .map_err(|err| format!("failed to spawn watcher manager thread: {}", err))?
        };
        Ok((
            Self {
                control,
                emitter,
                next_epoch: AtomicU64::new(1),
                last_error,
                manager: Some(manager),
                journal: journal.clone(),
            },
            journal,
        ))
    }

    /// Replace the watched roots. Returns the epoch the new roots are
    /// stamped with, once the replacement backend is watching and the
    /// `Restart` markers for the new roots have been emitted. Events that
    /// arrived from the old roots keep the previous epoch. When the
    /// replacement backend cannot start the error is returned (and kept in
    /// `last_error`); the markers are still emitted and a later `set_roots`
    /// can recover.
    pub fn set_roots(&self, roots: Vec<WatchRoot>) -> Result<u64, String> {
        let epoch = self.next_epoch.fetch_add(1, Ordering::AcqRel);
        let (done, ready) = mpsc::channel();
        self.control
            .send(Control::SetRoots { roots, epoch, done })
            .map_err(|_| "watcher manager has stopped".to_string())?;
        match ready.recv() {
            Ok(Ok(())) => Ok(epoch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("watcher manager has stopped".to_string()),
        }
    }

    /// The epoch currently stamped onto events.
    pub fn epoch(&self) -> u64 {
        self.emitter.epoch.load(Ordering::Acquire)
    }

    /// The last backend error from a `set_roots` exchange, if any.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn exclude(&self) -> &ExcludePolicy {
        self.emitter.exclude()
    }
}

impl Drop for FileSystemWatcher {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(manager) = self.manager.take() {
            let _ = manager.join();
        }
    }
}

fn manager_loop(
    mut backend: imp::PlatformWatcher,
    commands: mpsc::Receiver<Control>,
    emitter: Arc<Emitter>,
    last_error: Arc<Mutex<Option<String>>>,
) {
    loop {
        match commands.recv() {
            Ok(Control::SetRoots { roots, epoch, done }) => {
                // Stop and join the old backend first: every event it emitted
                // carries the old epoch, every event after this carries the new.
                backend.stop();
                emitter.set_epoch(epoch);
                emitter.set_roots(roots.clone());
                // Hold admission while the replacement starts, so the
                // Restart markers precede everything the new backend sees.
                emitter.hold();
                let result = match imp::PlatformWatcher::start(roots.clone(), Arc::clone(&emitter)) {
                    Ok(next) => {
                        backend = next;
                        if let Ok(mut slot) = last_error.lock() {
                            *slot = None;
                        }
                        Ok(())
                    }
                    Err(error) => {
                        if let Ok(mut slot) = last_error.lock() {
                            *slot = Some(error.clone());
                        }
                        // Keep serving control commands with an idle backend
                        // so a later set_roots can recover.
                        backend = imp::PlatformWatcher::idle();
                        Err(error)
                    }
                };
                emitter.release(
                    roots
                        .iter()
                        .map(|root| (root.clone(), RescanReason::Restart))
                        .collect(),
                );
                let _ = done.send(result);
            }
            Ok(Control::Stop) | Err(_) => {
                backend.stop();
                return;
            }
        }
    }
}

pub mod exclude;
pub use exclude::ExcludePolicy;
pub mod journal;
pub use journal::{Gap, GapReason, Journal, JournalConfig, JournalStats};
pub mod settle;
pub use settle::{probe_file, FileProbe, SettleConfig, Settlement, Settler};

#[cfg(test)]
mod tests;
