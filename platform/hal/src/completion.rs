//! When a session's submitted GPU work may be read by someone else (an encoder, another process):
//! one frame completion for every backend. Linux answers with a sync_file once the frame is on
//! the queue (the reader's GPU waits on it, no CPU wait), Metal once the command buffers ran,
//! D3D11 at once (its shared textures' keyed mutex orders the reader on the GPU).
use crate::Hal;
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

/// A sync_file the reader's GPU waits on (Linux).
#[cfg(unix)]
pub type SyncFile = std::os::fd::OwnedFd;
/// No sync_file off unix (never made there).
#[cfg(not(unix))]
pub struct SyncFile {
    _never: (),
}

/// The answer: complete (`None`), complete once the sync_file signals, or failed.
pub type Answer = Result<Option<SyncFile>, String>;

/// A frame's completion. `None` from `poll` while the backend has not answered yet.
pub struct Completion {
    rx: Receiver<Answer>,
}

impl Completion {
    /// Complete already.
    pub fn done() -> Completion {
        let (tx, rx) = sync_channel(1);
        let _ = tx.try_send(Ok(None));
        Completion { rx }
    }

    pub fn poll(&mut self) -> Option<Answer> {
        match self.rx.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the frame's completion was dropped unanswered".into())),
        }
    }
}

/// The backend's side of a [`Completion`]: answered once, then wakes whoever asked.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) struct CompletionReply {
    tx: Mutex<Option<std::sync::mpsc::SyncSender<Answer>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl CompletionReply {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> (Completion, CompletionReply) {
        let (tx, rx) = sync_channel(1);
        (Completion { rx }, CompletionReply { tx: Mutex::new(Some(tx)), wake })
    }

    pub fn send(&self, value: Answer) {
        let tx = crate::lock(&self.tx).take();
        if let Some(tx) = tx {
            let _ = tx.try_send(value);
            (self.wake)();
        }
    }
}

impl Hal {
    /// The completion of everything `session` submitted so far; `notify` (any thread) when it
    /// answers.
    pub fn frame_completion(&self, session: u32, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Completion, String> {
        let mut state = crate::lock(&self.state);
        let state = &mut *state;
        state.world.session_mut(session).filter(|s| s.attached && !s.closing).ok_or("closed HAL session")?;
        let backend = state.backend.as_mut().map(|(_, b)| b).ok_or("no renderer")?;
        backend.frame_completion(session, notify)
    }
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn failed(error: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = error);
}

/// The completions `hal_frame_completion` handed out, by number.
static PENDING: Mutex<Option<std::collections::HashMap<u64, Completion>>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// [`Hal::frame_completion`]: `notify` is called (any thread) once it answers. Its number for
/// `hal_frame_completion_poll`, 0 when failed (`hal_frame_completion_error`).
#[makepad_relax_api::relax_api]
pub fn hal_frame_completion(session: u32, notify: extern "C" fn()) -> u64 {
    // `notify` runs in the code that asked (`call_scope`), on whatever thread answers
    let scope = crate::call_scope::Scope::here();
    match Hal::global().frame_completion(session, Arc::new(move || scope.call(|| notify()))) {
        Ok(completion) => {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::lock(&PENDING).get_or_insert_with(Default::default).insert(id, completion);
            id
        }
        Err(error) => {
            failed(error);
            0
        }
    }
}

/// Polls completion `id`: 0 pending; 1 complete, `fd` a sync_file the caller owns that the
/// reader's GPU waits on (-1: none, the work is complete); -1 failed (or unknown). A completion
/// that answered is gone.
#[makepad_relax_api::relax_api]
pub fn hal_frame_completion_poll(id: u64, fd: &mut i32) -> i32 {
    let mut pending = crate::lock(&PENDING);
    let Some(completion) = pending.get_or_insert_with(Default::default).get_mut(&id) else {
        failed("unknown frame completion".into());
        return -1;
    };
    let answer = completion.poll();
    if answer.is_some() {
        pending.as_mut().map(|p| p.remove(&id));
    }
    match answer {
        None => 0,
        Some(Ok(sync)) => {
            *fd = sync.map_or(-1, raw_fd);
            1
        }
        Some(Err(error)) => {
            failed(error);
            -1
        }
    }
}

#[cfg(unix)]
fn raw_fd(sync: SyncFile) -> i32 {
    std::os::fd::IntoRawFd::into_raw_fd(sync)
}

#[cfg(not(unix))]
fn raw_fd(_sync: SyncFile) -> i32 {
    -1
}

/// Forgets completion `id` unanswered.
#[makepad_relax_api::relax_api]
pub fn hal_frame_completion_drop(id: u64) {
    crate::lock(&PENDING).as_mut().map(|p| p.remove(&id));
}

/// The text of this thread's last failed completion op: its first `out.len()` bytes are
/// written, its length is returned.
#[makepad_relax_api::relax_api]
pub fn hal_frame_completion_error(out: &mut [u8]) -> u64 {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        let n = e.len().min(out.len());
        out[..n].copy_from_slice(&e.as_bytes()[..n]);
        e.len() as u64
    })
}
