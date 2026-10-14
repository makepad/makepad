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
    /// A release fence for the session's submitted work; `notify` when it signals.
    fn fence(&mut self, session: u32, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Fence, String>;
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
        self.with_dmabuf(session, |api, s| api.fence(s.id, notify))
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

// ---- the session-scoped DMA-BUF ops: JIT-compiled code (makepad run) reaches `Hal::global`'s
// DMA-BUF state only through these. Descriptors cross as raw FDs plus `DmabufDesc`; a failure's
// text is the calling thread's last error (`hal_dmabuf_error`).

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn failed(error: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = error);
}

/// The text of this thread's last failed DMA-BUF op: its first `out.len()` bytes are written,
/// its length is returned.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_error(out: &mut [u8]) -> u64 {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        let n = e.len().min(out.len());
        out[..n].copy_from_slice(&e.as_bytes()[..n]);
        e.len() as u64
    })
}

/// A DMA-BUF descriptor without its FDs (`DmaBuf::into_parts` / `from_parts`).
#[makepad_relax_api::relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DmabufDesc {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub sample_only: bool,
    /// OPAQUE_FD memory of a native hosted parent (`memory_type_index`, the UUIDs)
    pub opaque: bool,
    pub memory_type_index: u32,
    pub modifier: u64,
    pub offset: u64,
    pub row_pitch: u64,
    pub allocation_size: u64,
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

impl DmaBuf {
    /// The descriptor's FD, its timeline FD and the rest as plain data.
    pub fn into_parts(self) -> (OwnedFd, Option<OwnedFd>, DmabufDesc) {
        let opaque = self.opaque.unwrap_or(OpaqueMemory { memory_type_index: 0, device_uuid: [0; 16], driver_uuid: [0; 16] });
        let desc = DmabufDesc { width: self.width, height: self.height, fourcc: self.fourcc, sample_only: self.sample_only,
            opaque: self.opaque.is_some(), memory_type_index: opaque.memory_type_index, modifier: self.modifier, offset: self.offset,
            row_pitch: self.row_pitch, allocation_size: self.allocation_size, device_uuid: opaque.device_uuid, driver_uuid: opaque.driver_uuid };
        (self.fd, self.timeline_fd, desc)
    }

    pub fn from_parts(fd: OwnedFd, timeline_fd: Option<OwnedFd>, desc: &DmabufDesc) -> DmaBuf {
        DmaBuf { fd, width: desc.width, height: desc.height, fourcc: desc.fourcc, modifier: desc.modifier, offset: desc.offset,
            row_pitch: desc.row_pitch, allocation_size: desc.allocation_size, sample_only: desc.sample_only,
            opaque: desc.opaque.then_some(OpaqueMemory { memory_type_index: desc.memory_type_index, device_uuid: desc.device_uuid, driver_uuid: desc.driver_uuid }),
            timeline_fd }
    }
}

fn owned(fd: i32) -> Option<OwnedFd> {
    use std::os::fd::FromRawFd;
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Registers the image of descriptor `fd` (and timeline `timeline_fd`, -1 for none; both taken
/// over) with `session` (`Hal::dmabuf_register`): its ticket, 0 when refused. The producer must
/// have released the allocation to external ownership.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_import(session: u32, fd: i32, timeline_fd: i32, desc: &DmabufDesc) -> u64 {
    let (Some(fd), timeline) = (owned(fd), owned(timeline_fd)) else {
        failed("no DMA-BUF descriptor".into());
        return 0;
    };
    match unsafe { Hal::global().dmabuf_register(session, DmaBuf::from_parts(fd, timeline, desc)) } {
        Ok(ticket) => ticket,
        Err(error) => { failed(error); 0 }
    }
}

/// `Hal::dmabuf_release`: lets go of `session`'s registration `ticket`.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_unregister(session: u32, ticket: u64) {
    Hal::global().dmabuf_release(session, ticket)
}

/// New copies of the descriptor and timeline FDs of `session`'s shared image `ticket` (the caller
/// owns them; `fds[1]` -1 when it has no timeline) and its layout; false when unknown.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_presentable(session: u32, ticket: u64, desc: &mut DmabufDesc, fds: &mut [i32]) -> bool {
    use std::os::fd::IntoRawFd;
    let Some(native) = Hal::global().dmabuf_handle(session, ticket) else { failed("shared image ticket expired".into()); return false };
    if fds.len() < 2 {
        failed("hal_dmabuf_presentable: fds holds 2".into());
        return false;
    }
    let fd = match native.fd.try_clone() { Ok(fd) => fd, Err(e) => { failed(e.to_string()); return false } };
    let timeline = match native.timeline_fd.as_ref().map(|t| t.try_clone()).transpose() { Ok(t) => t, Err(e) => { failed(e.to_string()); return false } };
    let opaque = native.opaque.unwrap_or(OpaqueMemory { memory_type_index: 0, device_uuid: [0; 16], driver_uuid: [0; 16] });
    *desc = DmabufDesc { width: native.width, height: native.height, fourcc: native.fourcc, sample_only: native.sample_only,
        opaque: native.opaque.is_some(), memory_type_index: opaque.memory_type_index, modifier: native.modifier, offset: native.offset,
        row_pitch: native.row_pitch, allocation_size: native.allocation_size, device_uuid: opaque.device_uuid, driver_uuid: opaque.driver_uuid };
    fds[0] = fd.into_raw_fd();
    fds[1] = timeline.map_or(-1, |t| t.into_raw_fd());
    true
}

/// `Hal::dmabuf_acquire`; false: failed.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_acquire(session: u32, texture: Id) -> bool {
    Hal::global().dmabuf_acquire(session, texture).map_err(failed).is_ok()
}

/// `Hal::dmabuf_write_available`: 1 the writer may write, 0 not yet, -1 failed.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_write_available(session: u32, texture: Id) -> i32 {
    match Hal::global().dmabuf_write_available(session, texture) {
        Ok(available) => available as i32,
        Err(error) => { failed(error); -1 }
    }
}

/// `Hal::dmabuf_sequence` into `sequence`; false: failed.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_sequence(session: u32, texture: Id, sequence: &mut u64) -> bool {
    match Hal::global().dmabuf_sequence(session, texture) {
        Ok(value) => { *sequence = value; true }
        Err(error) => { failed(error); false }
    }
}

/// `Hal::dmabuf_copy_frame`; false: failed.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_copy_frame(session: u32, source: Id, destination: Id, sequence: u64) -> bool {
    Hal::global().dmabuf_copy_frame(session, source, destination, sequence).map_err(failed).is_ok()
}

/// What `hal_dmabuf_support` found besides the lists.
#[makepad_relax_api::relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DmabufSupportInfo {
    /// the session's renderer imports DMA-BUFs
    pub found: bool,
    /// dev_t of the GPU's render node
    pub render_device: u64,
    /// bytes of the render node's path (more than `node` held: call again with room)
    pub node_len: u64,
    /// importable (fourcc, modifier) pairs (more than `formats` held: call again with room)
    pub format_count: u64,
}

/// `Hal::dmabuf_support`: the render node's path into `node`, the importable formats into
/// `formats` as (fourcc, modifier) word pairs.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_support(session: u32, formats: &mut [u64], node: &mut [u8]) -> DmabufSupportInfo {
    use std::os::unix::ffi::OsStrExt;
    let Some(support) = Hal::global().dmabuf_support(session) else { return DmabufSupportInfo::default() };
    let path = support.render_node.as_os_str().as_bytes();
    let n = path.len().min(node.len());
    node[..n].copy_from_slice(&path[..n]);
    for (pair, (fourcc, modifier)) in formats.chunks_exact_mut(2).zip(&support.formats) {
        pair[0] = *fourcc as u64;
        pair[1] = *modifier;
    }
    DmabufSupportInfo { found: true, render_device: support.render_device, node_len: path.len() as u64, format_count: support.formats.len() as u64 }
}

/// The fences `hal_dmabuf_fence` handed out, by number.
static FENCES: Mutex<Option<std::collections::HashMap<u64, Fence>>> = Mutex::new(None);
static NEXT_FENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// `Hal::dmabuf_fence`: a queue-ordered release of `session`'s foreign images; `notify` is called
/// (any thread) once it is published. Its number for `hal_dmabuf_fence_poll`, 0 when failed.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_fence(session: u32, notify: extern "C" fn()) -> u64 {
    match Hal::global().dmabuf_fence(session, Arc::new(move || notify())) {
        Ok(fence) => {
            let id = NEXT_FENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::lock(&FENCES).get_or_insert_with(Default::default).insert(id, fence);
            id
        }
        Err(error) => { failed(error); 0 }
    }
}

/// Polls fence `fence`: 0 pending; 1 signaled, `fd` the sync_file the caller owns (-1: the GPU
/// work was complete already); -1 failed (or an unknown fence). A fence that answered is gone.
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_fence_poll(fence: u64, fd: &mut i32) -> i32 {
    use std::os::fd::IntoRawFd;
    let mut fences = crate::lock(&FENCES);
    let Some(pending) = fences.get_or_insert_with(Default::default).get_mut(&fence) else { failed("unknown DMA-BUF fence".into()); return -1 };
    let answer = pending.poll();
    if answer.is_some() {
        fences.as_mut().map(|f| f.remove(&fence));
    }
    match answer {
        None => 0,
        Some(Ok(sync)) => { *fd = sync.map_or(-1, |s| s.into_raw_fd()); 1 }
        Some(Err(error)) => { failed(error); -1 }
    }
}

/// Forgets fence `fence` unanswered (its release still happens).
#[makepad_relax_api::relax_api]
pub fn hal_dmabuf_fence_drop(fence: u64) {
    crate::lock(&FENCES).as_mut().map(|f| f.remove(&fence));
}
