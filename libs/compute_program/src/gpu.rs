//! The GPU device as a session sees it: a [`GpuLink`] to the thread that owns the GPU (an app's
//! Cx, or a node's), which runs each session's ops in program order, in passes it sizes and
//! schedules against everything else on that GPU, and answers the session's reads. The device
//! itself needs the platform (`makepad-compute-program-gpu`); this side does not.
//!
//! A session's GPU work is plain data ([`GpuOp`]): buffers by the session's slot, kernels by the
//! session's index, compiled on the session's thread for the device's [`GpuInfo::target`].
//! Messages go in order ([`GpuMsg`]); the device answers a message's reads together, once they
//! are back. An interrupted or cancelled session moves its stop epoch on: the device drops what
//! computes in the messages sent before it (it still installs kernels, allocates and frees) and
//! answers their reads [`GpuError::Stopped`].

use makepad_script::shader_ir_compute::{CompiledKernel, ComputeTarget};
use makepad_script::LiveId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

/// What a GPU device is, fixed for its life.
#[derive(Clone, Debug)]
pub struct GpuInfo {
    /// What its kernels compile to.
    pub target: ComputeTarget,
    /// Bytes its programs may hold at once (the device's budget less what the app keeps).
    pub budget_bytes: u64,
}

/// One op of a session on its GPU buffers (by the session's slot) and kernels (by the
/// session's GPU kernel index), in program order.
pub enum GpuOp {
    /// Kernel `kernel`, compiled for the device's target; `key` names its code (kernels with the
    /// same key run alike, so the device's measured speed carries over between sessions).
    Kernel { kernel: usize, key: u64, compiled: CompiledKernel, label: String },
    /// A buffer of `bytes` zero bytes.
    Alloc { buf: usize, bytes: u64 },
    Free { buf: usize },
    /// `bytes` into the buffer at byte `offset`.
    Upload { buf: usize, offset: u64, bytes: Vec<u8> },
    /// Every word of the buffer.
    Fill { buf: usize, word: u32 },
    /// A buffer's words into another of the same size.
    Copy { src: usize, dst: usize },
    /// Element kernel `kernel` over elements `0..count`. `args` are its args words (the run's
    /// context words, then each host buffer's length in words), `buffers` its storage buffers
    /// in its layout's order: a session buffer, or None for the args.
    Run { kernel: usize, count: u32, args: Vec<u32>, buffers: Vec<Option<usize>> },
    /// Shader kernel `kernel` as `groups` workgroups: its params' bytes by name, its buffers
    /// in its layout's order.
    Dispatch { kernel: usize, groups: [u32; 3], params: Vec<(LiveId, Vec<u8>)>, buffers: Vec<usize> },
    /// The buffer's bytes `offset..offset + bytes`, answered with the message's other reads.
    Read { buf: usize, offset: u64, bytes: u64 },
}

/// Why a session's GPU work did not run.
#[derive(Clone, Debug, PartialEq)]
pub enum GpuError {
    /// The session was interrupted or cancelled: the work not yet run was dropped.
    Stopped,
    /// The GPU was lost (a reset, a device removed) or its device ended.
    DeviceLost,
    /// The device refused an op (a buffer it did not admit, a kernel it does not take): why.
    Failed(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::Stopped => write!(f, "stopped"),
            GpuError::DeviceLost => write!(f, "the GPU was lost"),
            GpuError::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// A message's reads, in order (or why there are none).
pub type GpuReads = Result<Vec<Vec<u8>>, GpuError>;

pub enum GpuMsg {
    /// A session's start: `stop` is its stop epoch. `watched`: someone waits on it or watches
    /// it as it goes (class interactive or visible), so the app they interact with draws on this
    /// GPU now: while it runs, every pass stays short.
    Open { session: u64, label: String, stop: Arc<AtomicU64>, watched: bool },
    /// Ops sent at stop epoch `epoch`. `reply` takes the message's reads once they are all back
    /// (none: once its ops ran), or the first error of the session's work since its last reply.
    Ops { session: u64, epoch: u64, ops: Vec<GpuOp>, reply: Option<mpsc::Sender<GpuReads>> },
    /// The session's end: its buffers and kernels go.
    Close { session: u64 },
}

/// A link to a GPU device, which its host hands each session it opens.
#[derive(Clone)]
pub struct GpuLink {
    info: GpuInfo,
    tx: mpsc::Sender<GpuMsg>,
    wake: Arc<dyn Fn() + Send + Sync>,
    next: Arc<AtomicU64>,
}

impl GpuLink {
    /// A link to the device that takes `rx`'s messages; `wake` makes the device's thread look
    /// at them (the platform's UI signal).
    pub fn new(info: GpuInfo, wake: Arc<dyn Fn() + Send + Sync>) -> (GpuLink, mpsc::Receiver<GpuMsg>) {
        let (tx, rx) = mpsc::channel();
        (GpuLink { info, tx, wake, next: Arc::new(AtomicU64::new(1)) }, rx)
    }

    pub fn info(&self) -> &GpuInfo {
        &self.info
    }

    /// Makes the device look at what came.
    pub(crate) fn wake(&self) {
        (self.wake)()
    }

    /// A new session on the device; false where the device has ended.
    pub(crate) fn open(&self, label: &str, stop: Arc<AtomicU64>, watched: bool) -> Option<u64> {
        let session = self.next.fetch_add(1, Ordering::Relaxed);
        self.send(GpuMsg::Open { session, label: label.to_string(), stop, watched }).then_some(session)
    }

    /// Sends `msg`; false where the device has ended.
    pub(crate) fn send(&self, msg: GpuMsg) -> bool {
        let sent = self.tx.send(msg).is_ok();
        self.wake();
        sent
    }
}
