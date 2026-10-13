//! wp_linux_drm_syncobj_v1: explicit synchronization with DRM timeline syncobjs. The client's
//! acquire point becomes the sync_file the commit waits for; the host's read fence is transferred
//! to the release point instead of sending wl_buffer.release.
use super::*;
use crate::sys;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// A DRM device (the render node) on which syncobj handles live.
pub(crate) struct Drm { pub fd: OwnedFd }

/// (timeline handle, point)
pub(crate) type Point = (u32, u64);

impl Drm {
    fn create(&self) -> Option<u32> {
        let mut a = sys::drm_syncobj_create { handle: 0, flags: 0 };
        (unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_CREATE, &mut a as *mut _) } == 0).then_some(a.handle)
    }
    pub fn destroy(&self, handle: u32) {
        let mut a = sys::drm_syncobj_destroy { handle, pad: 0 };
        unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_DESTROY, &mut a as *mut _) };
    }
    pub fn import_timeline(&self, fd: &OwnedFd) -> Option<u32> {
        let mut a = sys::drm_syncobj_handle { handle: 0, flags: 0, fd: fd.as_raw_fd(), pad: 0 };
        (unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE, &mut a as *mut _) } == 0).then_some(a.handle)
    }
    fn transfer(&self, src: u32, src_point: u64, dst: u32, dst_point: u64, flags: u32) -> bool {
        let mut a = sys::drm_syncobj_transfer { src_handle: src, dst_handle: dst, src_point, dst_point, flags, pad: 0 };
        unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_TRANSFER, &mut a as *mut _) == 0 }
    }
    /// A sync_file that signals when `point` of the timeline does; `None` while the point has not
    /// been submitted yet (the caller retries).
    pub fn acquire_fence(&self, (timeline, point): Point) -> Option<OwnedFd> {
        let tmp = self.create()?;
        // Not WAIT_FOR_SUBMIT: that can block the server thread for as long as the client takes to submit.
        let ok = self.transfer(timeline, point, tmp, 0, 0);
        let fd = if ok {
            let mut a = sys::drm_syncobj_handle { handle: tmp, flags: sys::DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE, fd: -1, pad: 0 };
            (unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &mut a as *mut _) } == 0 && a.fd >= 0)
                .then(|| unsafe { OwnedFd::from_raw_fd(a.fd) })
        } else { None };
        self.destroy(tmp);
        fd
    }
    /// Signals `point` when `fence` does (or right away without a fence).
    pub fn signal_point(&self, (timeline, point): Point, fence: Option<&OwnedFd>) {
        if let Some(f) = fence {
            let mut a = sys::drm_syncobj_handle { handle: 0, flags: sys::DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE, fd: f.as_raw_fd(), pad: 0 };
            if unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE, &mut a as *mut _) } == 0 {
                self.transfer(a.handle, 0, timeline, point, 0);
                self.destroy(a.handle);
                return;
            }
        }
        let (handles, points) = ([timeline], [point]);
        let mut a = sys::drm_syncobj_timeline_array { handles: handles.as_ptr() as u64, points: points.as_ptr() as u64, count_handles: 1, flags: 0 };
        unsafe { sys::ioctl(self.fd.as_raw_fd(), sys::DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL, &mut a as *mut _) };
    }
}

/// The explicit-sync state of one surface (pending until commit).
#[derive(Default, Clone, Copy)]
pub(crate) struct SurfaceSync { pub acquire: Option<Point>, pub release: Option<Point> }

impl Compositor {
    pub(crate) fn syncobj_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &mut Vec<Value>) -> Result<(), Fatal> {
        let _ = core;
        match o.iface.name {
            "wp_linux_drm_syncobj_manager_v1" => match op {
                wp_linux_drm_syncobj_manager_v1::REQ_GET_SURFACE => {
                    let (id, surf) = (a[0].as_uint(), a[1].as_uint());
                    let Some(s) = self.surface((c, surf)) else { return Ok(()) };
                    if s.sync_obj.is_some() { return Err(Fatal::new(o.id, wp_linux_drm_syncobj_manager_v1::ERROR_SURFACE_EXISTS, "surface already has a syncobj")); }
                    s.sync_obj = Some(id);
                    self.objs.insert((c, id), Obj::SyncSurface { surface: surf });
                }
                wp_linux_drm_syncobj_manager_v1::REQ_IMPORT_TIMELINE => {
                    let id = a[0].as_uint();
                    let Some(Value::Fd(fd)) = a.pop() else { return Ok(()) };
                    let handle = self.drm.as_ref().and_then(|d| d.import_timeline(&fd));
                    let Some(handle) = handle else { return Err(Fatal::new(o.id, wp_linux_drm_syncobj_manager_v1::ERROR_INVALID_TIMELINE, "cannot import the timeline")) };
                    self.objs.insert((c, id), Obj::Timeline { handle });
                }
                _ => {}
            },
            "wp_linux_drm_syncobj_surface_v1" => {
                let Some(Obj::SyncSurface { surface }) = self.objs.get(&(c, o.id)) else { return Ok(()) };
                let surf = *surface;
                if self.surface((c, surf)).is_none() {
                    if op == wp_linux_drm_syncobj_surface_v1::REQ_DESTROY { return Ok(()); }
                    return Err(Fatal::new(o.id, wp_linux_drm_syncobj_surface_v1::ERROR_NO_SURFACE, "the surface is gone"));
                }
                if matches!(op, wp_linux_drm_syncobj_surface_v1::REQ_SET_ACQUIRE_POINT | wp_linux_drm_syncobj_surface_v1::REQ_SET_RELEASE_POINT) {
                    let t = a[0].as_uint();
                    let point = ((a[1].as_uint() as u64) << 32) | a[2].as_uint() as u64;
                    let Some(Obj::Timeline { handle }) = self.objs.get(&(c, t)) else { return Ok(()) };
                    let p = (*handle, point);
                    let Some(s) = self.surface((c, surf)) else { return Ok(()) };
                    if op == wp_linux_drm_syncobj_surface_v1::REQ_SET_ACQUIRE_POINT { s.pending_sync.acquire = Some(p) } else { s.pending_sync.release = Some(p) }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn syncobj_gone(&mut self, c: ClientId, o: ObjRef) {
        match self.objs.remove(&(c, o.id)) {
            // The handle stays: points of this timeline may still be waiting for their release.
            Some(Obj::Timeline { handle }) => self.timelines.push((c, handle)),
            Some(Obj::SyncSurface { surface }) => { if let Some(s) = self.surface((c, surface)) { s.sync_obj = None; s.pending_sync = Default::default(); } }
            _ => {}
        }
    }
}
