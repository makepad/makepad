//! What a stream encoder reads a GPU picture from, the same on every OS: a shared surface
//! ([`SurfaceHandle`]: the platform's handle, its size and pixel layout) and the completion of the
//! GPU work that drew it ([`GpuCompletion`]). Cx makes both (`Cx::encode_surface`,
//! `Cx::frame_completion`); capture sources make their own handles.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The pixels a surface holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    /// NV12 packed into BGRA8 texels (`width/4` texels a row, the luma rows then the chroma
    /// rows): what the `draw_nv12_pack` shaders write.
    PackedNv12,
    /// A BGRA8 picture (a window, a screen capture).
    Bgra8,
    /// A native biplanar NV12 picture (a screen capture's).
    Nv12,
}

/// Which platform handle `SurfaceHandle::id` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Linux: a DMA-BUF registered with `makepad_hal::dmabuf::hal_dmabuf_register`.
    DmaBuf,
    /// Apple: an IOSurface id.
    IoSurface,
    /// Windows: the wire key of a keyed-mutex shared texture (`makepad_hal::shared_texture`).
    D3d11Keyed,
}

/// A GPU picture an encoder may read: the producer keeps the surface alive while the handle
/// lives (`keep`: Linux's DMA-BUF registration, released with the last clone).
#[derive(Clone)]
pub struct SurfaceHandle {
    pub id: u64,
    pub kind: SurfaceKind,
    /// The surface's size in pixels of `format` (a `PackedNv12` surface of a `w` x `h` picture
    /// is `w` x `h`, not its texel size).
    pub size: [u32; 2],
    pub format: SurfaceFormat,
    keep: Option<Arc<dyn Send + Sync>>,
}

impl SurfaceHandle {
    pub fn new(id: u64, kind: SurfaceKind, size: [u32; 2], format: SurfaceFormat) -> SurfaceHandle {
        SurfaceHandle { id, kind, size, format, keep: None }
    }

    /// This handle keeps `owner` alive (dropped with the last clone of the handle).
    pub fn keeping(mut self, owner: Arc<dyn Send + Sync>) -> SurfaceHandle {
        self.keep = Some(owner);
        self
    }

    /// The same surface read as another layout or size (a capture's frame of the size it came).
    pub fn with(&self, size: [u32; 2], format: SurfaceFormat) -> SurfaceHandle {
        SurfaceHandle { size, format, ..self.clone() }
    }
}

impl std::fmt::Debug for SurfaceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} {} {}x{} {:?}", self.kind, self.id, self.size[0], self.size[1], self.format)
    }
}

/// Every completion's answer wakes the waiters (`completion_notified`); each one re-polls its own.
static ANSWERS: Mutex<u64> = Mutex::new(0);
static ANSWERED: Condvar = Condvar::new();

/// A frame completion answered (the HAL's notify for `hal_frame_completion`, any thread): the
/// waiting readers look at theirs.
pub fn completion_notified() {
    *ANSWERS.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    ANSWERED.notify_all();
}

/// A sync_file the reader's GPU waits on (Linux).
#[cfg(target_os = "linux")]
pub type SyncFile = std::os::fd::OwnedFd;
/// No sync_file off Linux (never made there).
#[cfg(not(target_os = "linux"))]
pub struct SyncFile {
    _never: (),
}

/// Where a completion's answer comes from (Cx: the HAL's frame completion): `None` while
/// pending; complete, with a sync_file the reader's GPU waits on or none; or failed.
pub trait CompletionSource: Send {
    fn poll(&mut self) -> Option<Result<Option<SyncFile>, String>>;
}

struct Inner {
    /// None once answered (or complete from the start).
    source: Option<Box<dyn CompletionSource>>,
    answer: Option<Result<(), String>>,
    #[cfg(target_os = "linux")]
    sync_file: Option<std::os::fd::OwnedFd>,
}

/// When a frame's pictures may be read (`Cx::frame_completion`; one per frame, cloned for every
/// reader). [`Self::wait`] returns when the reader may go on; on Linux the reader's GPU then
/// waits for [`Self::sync_file`] (the CPU never waits for the GPU there).
#[derive(Clone)]
pub struct GpuCompletion(Arc<Mutex<Inner>>);

impl GpuCompletion {
    /// Complete already (a picture whose GPU work is done, a CPU picture).
    pub fn done() -> GpuCompletion {
        GpuCompletion(Arc::new(Mutex::new(Inner {
            source: None,
            answer: Some(Ok(())),
            #[cfg(target_os = "linux")]
            sync_file: None,
        })))
    }

    /// Answered by `source`, which calls [`completion_notified`] when it has its answer.
    pub fn from_source(source: Box<dyn CompletionSource>) -> GpuCompletion {
        GpuCompletion(Arc::new(Mutex::new(Inner {
            source: Some(source),
            answer: None,
            #[cfg(target_os = "linux")]
            sync_file: None,
        })))
    }

    /// The answer, once there is one.
    pub fn poll(&self) -> Option<Result<(), String>> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.answer.is_none() {
            let answer = s.source.as_mut()?.poll()?;
            s.source = None;
            match answer {
                Ok(sync) => {
                    #[cfg(target_os = "linux")]
                    {
                        s.sync_file = sync;
                    }
                    #[cfg(not(target_os = "linux"))]
                    let _ = sync;
                    s.answer = Some(Ok(()));
                }
                Err(e) => s.answer = Some(Err(e)),
            }
        }
        s.answer.clone()
    }

    /// Waits (at most `timeout`) for the answer; woken by it, never by a timer.
    pub fn wait(&self, timeout: Duration) -> Result<(), String> {
        let until = Instant::now() + timeout;
        loop {
            let seen = *ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(answer) = self.poll() {
                return answer;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the frame's GPU work never completed".into());
            }
            let guard = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
            let _ = ANSWERED.wait_timeout_while(guard, left, |n| *n == seen);
        }
    }

    /// A sync_file of its own for the reader's GPU to wait on (Linux), once answered; `None`
    /// when the work is complete already.
    #[cfg(target_os = "linux")]
    pub fn sync_file(&self) -> Option<std::os::fd::OwnedFd> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).sync_file.as_ref().and_then(|fd| fd.try_clone().ok())
    }
}
