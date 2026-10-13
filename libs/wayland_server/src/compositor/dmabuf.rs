//! zwp_linux_dmabuf_v1 (v4 with feedback): GPU buffers from clients. Buffers are validated here and
//! handed to the host as file descriptors; the host imports them on its GPU. The host answers each
//! buffer with `Command::BufferDone` carrying a fence of its reads, which is attached to the
//! dma-buf and answered with `wl_buffer.release`.
use super::*;
use crate::sys;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Which (format, modifier) pairs the host can import, and its render device.
#[derive(Clone, Debug, Default)]
pub struct DmabufConfig {
    /// dev_t of the host's render node.
    pub main_device: u64,
    pub formats: Vec<(u32, u64)>,
    /// The render node, for DRM syncobj explicit synchronization (offered when it opens).
    pub render_node: Option<std::path::PathBuf>,
}

pub(crate) struct DmabufState {
    pub cfg: DmabufConfig,
    table: std::fs::File,
    table_size: u32,
}

pub struct DmabufPlane { pub fd: OwnedFd, pub offset: u32, pub stride: u32 }

/// A validated client buffer.
pub struct DmabufBuffer {
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub flags: u32,
    pub planes: Vec<DmabufPlane>,
}

/// One commit's buffer as the host sees it.
pub struct DmabufFrame {
    /// The client's wl_buffer id (the host keys its import cache by (client, id)).
    pub buffer: u32,
    pub data: Arc<DmabufBuffer>,
    /// The writer's fence; the event is held back until it signalled, so the host sees `None`.
    pub(crate) acquire: Option<OwnedFd>,
    /// An explicit-sync acquire point that was not submitted yet; retried until it is (or 2 s pass).
    pub(crate) point: Option<(syncobj::Point, std::time::Instant)>,
}
impl std::fmt::Debug for DmabufFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DmabufFrame({} {}x{} {:#x}:{:#x})", self.buffer, self.data.width, self.data.height, self.data.fourcc, self.data.modifier)
    }
}

#[derive(Default)]
pub(crate) struct Params { planes: [Option<(OwnedFd, u32, u32)>; 4], modifier: Option<u64>, used: bool }

fn fd_len(fd: &OwnedFd) -> Option<u64> {
    let n = unsafe { sys::lseek(fd.as_raw_fd(), 0, sys::SEEK_END) };
    (n >= 0).then_some(n as u64)
}

/// The fence of the buffer's pending writes (a sync_file), or `None` when it has none.
pub(crate) fn export_fence(fd: &OwnedFd) -> Option<OwnedFd> {
    let mut a = sys::dma_buf_sync_file { flags: sys::DMA_BUF_SYNC_READ, fd: -1 };
    if unsafe { sys::ioctl(fd.as_raw_fd(), sys::DMA_BUF_IOCTL_EXPORT_SYNC_FILE, &mut a as *mut _) } != 0 || a.fd < 0 { return None; }
    Some(unsafe { OwnedFd::from_raw_fd(a.fd) })
}
/// Whether a sync_file has signalled.
pub(crate) fn fence_ready(fd: &OwnedFd) -> bool {
    let mut p = sys::pollfd { fd: fd.as_raw_fd(), events: sys::POLLIN, revents: 0 };
    unsafe { sys::poll(&mut p, 1, 0) != 0 }
}
/// Makes later writers of the buffer wait for `fence` (our reads).
pub(crate) fn import_read_fence(buffer: &OwnedFd, fence: &OwnedFd) {
    let a = sys::dma_buf_sync_file { flags: sys::DMA_BUF_SYNC_READ, fd: fence.as_raw_fd() };
    unsafe { sys::ioctl(buffer.as_raw_fd(), sys::DMA_BUF_IOCTL_IMPORT_SYNC_FILE, &a as *const _) };
}

fn bytes_u64(v: u64) -> Value { Value::Array(v.to_ne_bytes().to_vec()) }

impl Compositor {
    /// Starts offering zwp_linux_dmabuf_v1. Call once, before clients connect (or later: the
    /// registry announces it).
    pub fn enable_dmabuf(&mut self, core: &mut Core, cfg: DmabufConfig) -> std::io::Result<()> {
        if cfg.formats.is_empty() || self.dmabuf.is_some() { return Ok(()); }
        let mut bytes = Vec::with_capacity(cfg.formats.len() * 16);
        for (f, m) in &cfg.formats {
            bytes.extend_from_slice(&f.to_ne_bytes());
            bytes.extend_from_slice(&0u32.to_ne_bytes());
            bytes.extend_from_slice(&m.to_ne_bytes());
        }
        let fd = unsafe { sys::memfd_create(c"dmabuf-format-table".as_ptr(), sys::MFD_CLOEXEC) };
        if fd < 0 { return Err(std::io::Error::last_os_error()); }
        let mut table = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        table.write_all(&bytes)?;
        core.add_global(&zwp_linux_dmabuf_v1::INTERFACE, 4);
        if let Some(path) = &cfg.render_node {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) {
                self.drm = Some(syncobj::Drm { fd: f.into() });
                core.add_global(&wp_linux_drm_syncobj_manager_v1::INTERFACE, 1);
            }
        }
        self.dmabuf = Some(DmabufState { table_size: bytes.len() as u32, table, cfg });
        Ok(())
    }

    fn send_feedback(&mut self, core: &mut Core, c: ClientId, id: u32) {
        let Some(d) = &self.dmabuf else { return };
        let Ok(fd) = d.table.try_clone() else { return };
        let indices: Vec<u8> = (0..d.cfg.formats.len() as u16).flat_map(|i| i.to_ne_bytes()).collect();
        let dev = d.cfg.main_device;
        let size = d.table_size;
        let s = |core: &mut Core, op, args| { let _ = core.send(c, id, op, args); };
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_FORMAT_TABLE, vec![Value::Fd(fd.into()), Value::Uint(size)]);
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_MAIN_DEVICE, vec![bytes_u64(dev)]);
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_TRANCHE_TARGET_DEVICE, vec![bytes_u64(dev)]);
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_TRANCHE_FLAGS, vec![Value::Uint(0)]);
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_TRANCHE_FORMATS, vec![Value::Array(indices)]);
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_TRANCHE_DONE, vec![]);
        s(core, zwp_linux_dmabuf_feedback_v1::EVT_DONE, vec![]);
    }

    pub(crate) fn dmabuf_bind(&mut self, core: &mut Core, c: ClientId, id: u32, version: u32) {
        // Versions before 4 learn the formats by events.
        if version >= 4 { return; }
        let Some(d) = &self.dmabuf else { return };
        let list = d.cfg.formats.clone();
        let mut seen: Vec<u32> = Vec::new();
        for (f, m) in list {
            if version < 3 {
                if !seen.contains(&f) { seen.push(f); let _ = core.send(c, id, zwp_linux_dmabuf_v1::EVT_FORMAT, vec![Value::Uint(f)]); }
            } else {
                let _ = core.send(c, id, zwp_linux_dmabuf_v1::EVT_MODIFIER, vec![Value::Uint(f), Value::Uint((m >> 32) as u32), Value::Uint(m as u32)]);
            }
        }
    }

    pub(crate) fn dmabuf_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &mut Vec<Value>) -> Result<(), Fatal> {
        match o.iface.name {
            "zwp_linux_dmabuf_v1" => match op {
                zwp_linux_dmabuf_v1::REQ_CREATE_PARAMS => { self.objs.insert((c, a[0].as_uint()), Obj::DmaParams(Params::default())); }
                zwp_linux_dmabuf_v1::REQ_GET_DEFAULT_FEEDBACK | zwp_linux_dmabuf_v1::REQ_GET_SURFACE_FEEDBACK => {
                    let id = a[0].as_uint();
                    self.objs.insert((c, id), Obj::Plain);
                    self.send_feedback(core, c, id);
                }
                _ => {}
            },
            "zwp_linux_buffer_params_v1" => match op {
                zwp_linux_buffer_params_v1::REQ_ADD => {
                    let idx = a[1].as_uint();
                    let (off, stride) = (a[2].as_uint(), a[3].as_uint());
                    let modifier = ((a[4].as_uint() as u64) << 32) | a[5].as_uint() as u64;
                    let Some(Value::Fd(fd)) = a.remove(0).into_fd() else { return Ok(()) };
                    let Some(Obj::DmaParams(p)) = self.objs.get_mut(&(c, o.id)) else { return Ok(()) };
                    if p.used { return Err(Fatal::new(o.id, zwp_linux_buffer_params_v1::ERROR_ALREADY_USED, "params already used")); }
                    if idx >= 4 { return Err(Fatal::new(o.id, zwp_linux_buffer_params_v1::ERROR_PLANE_IDX, "plane index out of bounds")); }
                    if p.planes[idx as usize].is_some() { return Err(Fatal::new(o.id, zwp_linux_buffer_params_v1::ERROR_PLANE_SET, "plane already set")); }
                    p.modifier = Some(modifier);
                    p.planes[idx as usize] = Some((fd, off, stride));
                }
                zwp_linux_buffer_params_v1::REQ_CREATE | zwp_linux_buffer_params_v1::REQ_CREATE_IMMED => {
                    let immed = op == zwp_linux_buffer_params_v1::REQ_CREATE_IMMED;
                    let base = if immed { 1 } else { 0 };
                    let buf_id = if immed { a[0].as_uint() } else { 0 };
                    let (w, h, fmt, flags) = (a[base].as_int(), a[base + 1].as_int(), a[base + 2].as_uint(), a[base + 3].as_uint());
                    let result = self.build_dmabuf(c, o.id, w, h, fmt, flags);
                    match result {
                        Ok(buf) => {
                            let id = if immed { buf_id } else { match core.new_object(c, &wl_buffer::INTERFACE, 1) { Some(i) => i, None => return Err(Fatal::new(o.id, 0, "out of objects")) } };
                            self.objs.insert((c, id), Obj::DmaBuffer(Arc::new(buf)));
                            if !immed { let _ = core.send(c, o.id, zwp_linux_buffer_params_v1::EVT_CREATED, vec![Value::NewId(id)]); }
                        }
                        Err(f) => {
                            if immed || f.code != 0xffff { return Err(Fatal::new(f.object, f.code & 0xff, f.message)); }
                            let _ = core.send(c, o.id, zwp_linux_buffer_params_v1::EVT_FAILED, vec![]);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    /// Takes the planes out of the params object and checks them. Code 0xffff = soft failure.
    fn build_dmabuf(&mut self, c: ClientId, params: u32, w: i32, h: i32, fmt: u32, flags: u32) -> Result<DmabufBuffer, Fatal> {
        use zwp_linux_buffer_params_v1 as e;
        let Some(Obj::DmaParams(p)) = self.objs.get_mut(&(c, params)) else { return Err(Fatal::new(params, 0xffff, "gone")) };
        if p.used { return Err(Fatal::new(params, e::ERROR_ALREADY_USED, "params already used")); }
        p.used = true;
        let modifier = p.modifier.unwrap_or(0);
        if p.planes[0].is_none() { return Err(Fatal::new(params, e::ERROR_INCOMPLETE, "no plane 0")); }
        let n = p.planes.iter().take_while(|x| x.is_some()).count();
        if p.planes[n..].iter().any(|x| x.is_some()) { return Err(Fatal::new(params, e::ERROR_INCOMPLETE, "gap in the plane list")); }
        if w <= 0 || h <= 0 { return Err(Fatal::new(params, e::ERROR_INVALID_DIMENSIONS, "bad size")); }
        let Some(d) = &self.dmabuf else { return Err(Fatal::new(params, e::ERROR_INVALID_FORMAT, "dmabuf is not offered")) };
        if !d.cfg.formats.contains(&(fmt, modifier)) { return Err(Fatal::new(params, e::ERROR_INVALID_FORMAT, "format/modifier not offered")); }
        let mut planes = Vec::new();
        for slot in p.planes.iter_mut().take(n) {
            let (fd, offset, stride) = slot.take().unwrap();
            // Bounds, as far as the kernel tells us (linear layouts only; tiled sizes are the driver's).
            if modifier == 0 {
                let need = offset as u64 + stride as u64 * (h as u64 - 1) + w as u64 * 4;
                if fd_len(&fd).is_some_and(|len| need > len) { return Err(Fatal::new(params, e::ERROR_OUT_OF_BOUNDS, "plane exceeds its dmabuf")); }
            }
            planes.push(DmabufPlane { fd, offset, stride });
        }
        Ok(DmabufBuffer { width: w as u32, height: h as u32, fourcc: fmt, modifier, flags, planes })
    }

    /// Host side: the host is done reading `buffer`. The fence (if any) goes on the dma-buf so a
    /// writer waits for it, then the client is told it may reuse the buffer.
    pub fn buffer_done(&mut self, core: &mut Core, client: ClientId, buffer: u32, fence: Option<OwnedFd>) {
        if let (Some(Obj::DmaBuffer(buf)), Some(f)) = (self.objs.get(&(client, buffer)), fence.as_ref()) {
            for p in &buf.planes { import_read_fence(&p.fd, f); }
        }
        // Explicit sync: the release points get the fence; no wl_buffer.release is sent for them.
        if let Some(points) = self.release_points.remove(&(client, buffer)) {
            if let Some(d) = &self.drm { for p in points { d.signal_point(p, fence.as_ref()); } }
            return;
        }
        let _ = core.send(client, buffer, wl_buffer::EVT_RELEASE, vec![]);
    }

    /// The events that are ready, in order: a dmabuf commit waits for its writer's fence.
    pub fn take_events(&mut self) -> Vec<UiEvent> {
        let mut ready = 0;
        for ev in self.events.iter_mut() {
            if let UiEvent::Commit { buffer: BufferUpdate::Dmabuf(f), .. } = ev {
                if f.acquire.is_none() {
                    if let (Some((p, since)), Some(d)) = (f.point, &self.drm) {
                        match d.acquire_fence(p) {
                            Some(fd) => { f.acquire = Some(fd); f.point = None; }
                            None if since.elapsed() > std::time::Duration::from_secs(2) => f.point = None,
                            None => break,
                        }
                    }
                }
                if let Some(fd) = &f.acquire {
                    if !fence_ready(fd) { break; }
                    f.acquire = None;
                }
            }
            ready += 1;
        }
        self.events.drain(..ready).collect()
    }
    /// True while events wait on a fence (the host should poll again soon).
    pub fn has_gated_events(&self) -> bool { !self.events.is_empty() }
}

trait IntoFd { fn into_fd(self) -> Option<Value>; }
impl IntoFd for Value { fn into_fd(self) -> Option<Value> { match self { Value::Fd(_) => Some(self), _ => None } } }
