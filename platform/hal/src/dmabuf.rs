//! Linux external-image transport facade. Descriptors own their FDs; command buffers
//! carry registry tickets. Driver objects never escape the Vulkan backend.
use crate::{backend::vulkan::share, session::Session, Hal, Id};
use std::os::fd::OwnedFd;
use std::sync::{mpsc::{sync_channel, Receiver, SyncSender, TryRecvError}, Arc, Mutex};

pub use share::{DmaBufSupport as Support, NativeHandle as DmaBuf, OpaqueMemory};

/// A queue-ordered release. `Some(Ok(None))` means GPU work is already complete;
/// `None` means the submit worker has not published the result yet.
pub struct Fence { rx: Receiver<Result<Option<OwnedFd>, String>> }
impl Fence {
    pub fn poll(&mut self) -> Option<Result<Option<OwnedFd>, String>> {
        match self.rx.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("external-image release worker stopped".into())),
        }
    }
}

pub(crate) struct FenceReply {
    tx: Mutex<Option<SyncSender<Result<Option<OwnedFd>, String>>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl FenceReply {
    pub fn send(&self, value: Result<Option<OwnedFd>, String>) {
        let tx = crate::lock(&self.tx).take();
        if let Some(tx) = tx { let _ = tx.try_send(value); (self.wake)(); }
    }
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> (Fence, Self) {
        let (tx, rx) = sync_channel(1);
        (Fence { rx }, Self { tx: Mutex::new(Some(tx)), wake })
    }
}

/// Backend implementation of the native descriptor facade.
pub trait Api {
    fn supported(&self) -> Result<(), String>;
    fn support(&self) -> Option<Support>;
    fn register(&self, session: u32, descriptor: DmaBuf) -> Result<u64, String>;
    fn handle(&self, session: u32, ticket: u64) -> Option<Arc<DmaBuf>>;
    fn release(&self, session: u32, ticket: u64);
    fn acquire(&mut self, session: u32, texture: Id) -> Result<(), String>;
    fn fence(&mut self, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Fence, String>;
    fn available(&self, session: u32, texture: Id) -> Result<bool, String>;
    fn sequence(&self, session: u32, texture: Id) -> Result<u64, String>;
    fn copy_frame(&mut self, session: &mut Session, source: Id, destination: Id, sequence: u64) -> Result<(), String>;
}

impl Hal {
    fn with_dmabuf<R>(&self, session: u32, f: impl FnOnce(&mut dyn Api, &mut Session) -> Result<R, String>) -> Result<R, String> {
        let mut state = crate::lock(&self.state);
        let state = &mut *state;
        let session = state.world.session_mut(session).filter(|s| s.attached && !s.closing).ok_or("closed HAL session")?;
        let api = state.backend.as_mut().and_then(|(_, b)| b.dmabuf()).ok_or("renderer has no DMA-BUF interop")?;
        f(api, session)
    }
    pub fn dmabuf_supported(&self, session: u32) -> Result<(), String> { self.with_dmabuf(session, |api, _| api.supported()) }
    pub fn dmabuf_support(&self, session: u32) -> Option<Support> { self.with_dmabuf(session, |api, _| Ok(api.support())).ok().flatten() }
    /// Register owned transport handles. The producer must have released the allocation
    /// to external ownership; its writes must be complete or protected by `timeline_fd`.
    pub unsafe fn dmabuf_register(&self, session: u32, descriptor: DmaBuf) -> Result<u64, String> {
        self.with_dmabuf(session, |api, s| api.register(s.id, descriptor))
    }
    pub fn dmabuf_handle(&self, session: u32, ticket: u64) -> Option<Arc<DmaBuf>> {
        self.with_dmabuf(session, |api, s| Ok(api.handle(s.id, ticket))).ok().flatten()
    }
    pub fn dmabuf_release(&self, session: u32, ticket: u64) { let _ = self.with_dmabuf(session, |api, s| { api.release(s.id, ticket); Ok(()) }); }
    /// The caller has observed the writer's implicit/explicit acquire fence before this call.
    pub fn dmabuf_acquire(&self, session: u32, texture: Id) -> Result<(), String> {
        self.with_dmabuf(session, |api, s| api.acquire(s.id, texture))
    }
    pub fn dmabuf_fence(&self, session: u32, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Fence, String> {
        self.with_dmabuf(session, |api, _| api.fence(notify))
    }
    pub fn dmabuf_write_available(&self, session: u32, texture: Id) -> Result<bool, String> {
        self.with_dmabuf(session, |api, s| api.available(s.id, texture))
    }
    pub fn dmabuf_sequence(&self, session: u32, texture: Id) -> Result<u64, String> {
        self.with_dmabuf(session, |api, s| api.sequence(s.id, texture))
    }
    /// Preserve a hosted frame in private GPU storage and return the writer's timeline
    /// lease only after the copy. Destination must already exist in this session.
    pub fn dmabuf_copy_frame(&self, session: u32, source: Id, destination: Id, sequence: u64) -> Result<(), String> {
        self.with_dmabuf(session, |api, s| api.copy_frame(s, source, destination, sequence))
    }
}

/// DMA-BUF copies outside any session (frame rings: a browser's paint into a slot, a slot read
/// back for a still or an encoder).
pub use share::copy;

/// Exports `session`'s shared texture by its `TextureShared` ticket: returns a new descriptor
/// the caller owns and writes its layout (`copy::Layout::to_words`, 6 words) to `layout`; -1 when
/// the ticket is unknown or `layout` is short.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_export(session: u32, ticket: u64, layout: &mut [u64]) -> i32 {
    use std::os::fd::IntoRawFd;
    if layout.len() < 6 {
        return -1;
    }
    let Some(native) = Hal::global().dmabuf_handle(session, ticket) else { return -1 };
    let Ok(fd) = native.fd.try_clone() else { return -1 };
    let words = copy::Layout { width: native.width, height: native.height, fourcc: native.fourcc, modifier: native.modifier,
        offset: native.offset, stride: native.row_pitch }.to_words();
    layout[..6].copy_from_slice(&words);
    fd.into_raw_fd()
}

/// Names the image of descriptor `fd` (taken over, closed on failure) laid out as `layout`
/// (6 words) for [`hal_dmabuf_read`]; 0 when refused.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_register(fd: i32, layout: &[u64]) -> u64 {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return 0;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    match copy::Layout::from_words(layout) {
        Some(layout) => copy::register(fd, layout),
        None => 0,
    }
}

#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_release(handle: u64) {
    copy::release(handle)
}

/// Reads `region` (x, y, width, height) of a registered image into `out` as BGRA: the bytes
/// written, or -1 (an unknown handle, a region outside it, `out` too short, a GPU error).
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_read(handle: u64, region: &[u32], out: &mut [u8]) -> i64 {
    let Ok(region) = <[u32; 4]>::try_from(region) else { return -1 };
    match copy::read(handle, region) {
        Ok(pixels) if pixels.len() <= out.len() => {
            out[..pixels.len()].copy_from_slice(&pixels);
            pixels.len() as i64
        }
        Ok(_) => -1,
        Err(error) => {
            eprintln!("[hal-dmabuf] read: {error}");
            -1
        }
    }
}
