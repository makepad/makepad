//! wl_surface double-buffered state, shm pools and buffers, subsurfaces.
use super::*;
use crate::shm::{copy_rows, ShmError};

#[derive(Default, Clone)]
pub(crate) struct Pending {
    /// `Some(None)` detaches, `Some(Some(id))` attaches the wl_buffer.
    attach: Option<Option<u32>>,
    damage: Vec<Rect>,
    buffer_damage: Vec<Rect>,
    frames: Vec<u32>,
    input: Option<Option<u32>>,
    scale: Option<i32>,
    transform: Option<i32>,
    viewport: Option<VpState>,
    sync: syncobj::SurfaceSync,
}
/// wp_viewport state: crop of the buffer (buffer pixels) and the size to show it at (logical px).
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub struct VpState { pub src: Option<(f64, f64, f64, f64)>, pub dst: Option<(i32, i32)> }
impl Pending {
    fn merge(&mut self, n: Pending) {
        if n.attach.is_some() { self.attach = n.attach; }
        self.damage.extend(n.damage);
        self.buffer_damage.extend(n.buffer_damage);
        self.frames.extend(n.frames);
        if n.input.is_some() { self.input = n.input; }
        if n.scale.is_some() { self.scale = n.scale; }
        if n.transform.is_some() { self.transform = n.transform; }
        if n.viewport.is_some() { self.viewport = n.viewport; }
        if n.sync.acquire.is_some() || n.sync.release.is_some() { self.sync = n.sync; }
    }
}

#[derive(Default)]
pub(crate) struct Sub { pub parent: u32, pub sync: bool, pub pos: (i32, i32), pub pending_pos: (i32, i32) }

pub(crate) struct Surface {
    pending: Pending,
    cached: Option<Pending>,
    pub scale: i32,
    pub transform: i32,
    pub has_buffer: bool,
    pub frames: Vec<u32>,
    pub sub: Option<Sub>,
    /// Stacking of children; `0` is the surface itself.
    pub stack: Vec<u32>,
    pub pending_stack: Vec<u32>,
    pub xdg: Option<u32>,
    pub buffer_size: (i32, i32),
    pub vp: VpState,
    pub viewport_obj: Option<u32>,
    pub sync_obj: Option<u32>,
    pub pending_sync: syncobj::SurfaceSync,
}
impl Surface {
    pub fn new() -> Self {
        Self { pending: Pending::default(), cached: None, scale: 1, transform: 0, has_buffer: false, frames: Vec::new(), sub: None,
               stack: vec![0], pending_stack: vec![0], xdg: None, buffer_size: (0, 0), vp: VpState::default(), viewport_obj: None, sync_obj: None, pending_sync: Default::default() }
    }
}

fn rect_args(a: &[Value]) -> Rect { Rect::new(a[0].as_int(), a[1].as_int(), a[2].as_int(), a[3].as_int()) }

impl Compositor {
    pub(crate) fn create_pool(&mut self, c: ClientId, shm_id: u32, id: u32, fd: std::os::fd::OwnedFd, size: i32) -> Result<(), Fatal> {
        if size <= 0 { return Err(Fatal::new(shm_id, wl_shm::ERROR_INVALID_STRIDE, "invalid pool size")); }
        match Pool::new(fd, size as usize) {
            Ok(p) => { self.objs.insert((c, id), Obj::Pool(Arc::new(Mutex::new(p)))); Ok(()) }
            Err(_) => Err(Fatal::new(shm_id, wl_shm::ERROR_INVALID_FD, "cannot map pool")),
        }
    }

    pub(crate) fn pool_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &[Value]) -> Result<(), Fatal> {
        let Some(Obj::Pool(pool)) = self.objs.get(&(c, o.id)) else { return Ok(()) };
        let pool = pool.clone();
        match op {
            wl_shm_pool::REQ_CREATE_BUFFER => {
                let len = pool.lock().unwrap().len();
                let spec = BufferSpec::new(len, a[1].as_int(), a[2].as_int(), a[3].as_int(), a[4].as_int(), a[5].as_uint());
                match spec {
                    Ok(spec) => { self.objs.insert((c, a[0].as_uint()), Obj::Buffer { spec, pool }); }
                    Err(_) => {
                        let code = if shm::format_supported(a[5].as_uint()) { wl_shm::ERROR_INVALID_STRIDE } else { wl_shm::ERROR_INVALID_FORMAT };
                        return Err(Fatal::new(o.id, code, "invalid buffer parameters"));
                    }
                }
            }
            wl_shm_pool::REQ_RESIZE => {
                let _ = core;
                if pool.lock().unwrap().resize(a[0].as_int().max(0) as usize).is_err() {
                    return Err(Fatal::new(o.id, wl_shm::ERROR_INVALID_FD, "cannot grow pool"));
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn surface_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &[Value]) -> Result<(), Fatal> {
        let k = (c, o.id);
        match op {
            wl_surface::REQ_ATTACH => {
                let (x, y) = (a[1].as_int(), a[2].as_int());
                if o.version >= 5 && (x != 0 || y != 0) {
                    return Err(Fatal::new(o.id, wl_surface::ERROR_INVALID_OFFSET, "attach offsets are replaced by wl_surface.offset"));
                }
                let b = a[0].as_uint();
                if let Some(s) = self.surface(k) { s.pending.attach = Some((b != 0).then_some(b)); }
            }
            wl_surface::REQ_DAMAGE => { let r = rect_args(&a[0..4]); if let Some(s) = self.surface(k) { s.pending.damage.push(r); } }
            wl_surface::REQ_DAMAGE_BUFFER => { let r = rect_args(&a[0..4]); if let Some(s) = self.surface(k) { s.pending.buffer_damage.push(r); } }
            wl_surface::REQ_FRAME => { let cb = a[0].as_uint(); if let Some(s) = self.surface(k) { s.pending.frames.push(cb); } }
            wl_surface::REQ_SET_INPUT_REGION => { let r = a[0].as_uint(); if let Some(s) = self.surface(k) { s.pending.input = Some((r != 0).then_some(r)); } }
            wl_surface::REQ_SET_BUFFER_SCALE => {
                let v = a[0].as_int();
                if v < 1 { return Err(Fatal::new(o.id, wl_surface::ERROR_INVALID_SCALE, "scale must be positive")); }
                if let Some(s) = self.surface(k) { s.pending.scale = Some(v); }
            }
            wl_surface::REQ_SET_BUFFER_TRANSFORM => {
                let v = a[0].as_int();
                if !(0..=7).contains(&v) { return Err(Fatal::new(o.id, wl_surface::ERROR_INVALID_TRANSFORM, "bad transform")); }
                if let Some(s) = self.surface(k) { s.pending.transform = Some(v); }
            }
            wl_surface::REQ_COMMIT => self.commit(core, k)?,
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self, core: &mut Core, k: SurfKey) -> Result<(), Fatal> {
        self.xdg_before_commit(core, k)?;
        let sync = self.effectively_sync(k);
        let Some(s) = self.surface(k) else { return Ok(()) };
        let mut p = std::mem::take(&mut s.pending);
        p.sync = std::mem::take(&mut s.pending_sync);
        if sync {
            match &mut s.cached { Some(cached) => cached.merge(p), None => s.cached = Some(p) }
            return Ok(());
        }
        self.apply(core, k, p)
    }

    fn effectively_sync(&mut self, k: SurfKey) -> bool {
        let mut cur = k;
        for _ in 0..64 {
            let Some(s) = self.surface(cur) else { return false };
            let Some(sub) = &s.sub else { return false };
            if sub.sync { return true; }
            cur = (k.0, sub.parent);
        }
        false
    }

    /// Makes `p` the surface's current state.
    fn apply(&mut self, core: &mut Core, k: SurfKey, p: Pending) -> Result<(), Fatal> {
        let mut update = BufferUpdate::Keep;
        let mut released = None;
        let mut buffer_err = None;
        if let Some(att) = p.attach {
            match att {
                None => update = BufferUpdate::Detach,
                Some(b) if matches!(self.objs.get(&(k.0, b)), Some(Obj::DmaBuffer(_))) => {
                    let Some(Obj::DmaBuffer(data)) = self.objs.get(&(k.0, b)) else { unreachable!() };
                    let data = data.clone();
                    if let Some(s) = self.surface(k) { s.buffer_size = (data.width as i32, data.height as i32); }
                    // The writer's fence rides with the event; the buffer is released by the host when its reads are done.
                    let (acquire, point);
                    let explicit = self.surface(k).is_some_and(|s| s.sync_obj.is_some());
                    if explicit {
                        let sync_id = self.surface(k).and_then(|s| s.sync_obj).unwrap();
                        match (p.sync.acquire, p.sync.release) {
                            (None, _) => return Err(Fatal::new(sync_id, wp_linux_drm_syncobj_surface_v1::ERROR_NO_ACQUIRE_POINT, "no acquire point")),
                            (_, None) => return Err(Fatal::new(sync_id, wp_linux_drm_syncobj_surface_v1::ERROR_NO_RELEASE_POINT, "no release point")),
                            (Some(a), Some(r)) => {
                                if a.0 == r.0 && a.1 >= r.1 { return Err(Fatal::new(sync_id, wp_linux_drm_syncobj_surface_v1::ERROR_CONFLICTING_POINTS, "acquire point must precede the release point")); }
                                let fence = self.drm.as_ref().and_then(|d| d.acquire_fence(a));
                                point = fence.is_none().then(|| (a, std::time::Instant::now()));
                                acquire = fence;
                                self.release_points.entry((k.0, b)).or_default().push(r);
                            }
                        }
                    } else {
                        acquire = data.planes.first().and_then(|pl| dmabuf::export_fence(&pl.fd));
                        point = None;
                    }
                    update = BufferUpdate::Dmabuf(DmabufFrame { buffer: b, data, acquire, point });
                }
                Some(b) => match self.objs.get(&(k.0, b)).and_then(|o| match o { Obj::Buffer { spec, pool } => Some((*spec, pool.clone())), _ => None }) {
                    Some((spec, pool)) => {
                        let (w, h) = (spec.width as i32, spec.height as i32);
                        let (y0, y1) = self.damage_rows(&p, &spec, k);
                        let pool = pool.lock().unwrap();
                        let mut data = Vec::new();
                        match copy_rows(&pool, &spec, y0, y1, &mut data) {
                            Ok(()) => {
                                update = BufferUpdate::Attach(Frame { width: spec.width as u32, height: spec.height as u32, format: spec.format, y0: y0 as u32, y1: y1.min(spec.height) as u32, data });
                                if let Some(s) = self.surface(k) { s.buffer_size = (w, h); }
                            }
                            Err(ShmError::Truncated) => buffer_err = Some(Fatal::new(b, wl_shm::ERROR_INVALID_FD, "pool memory was truncated")),
                            Err(_) => buffer_err = Some(Fatal::new(b, wl_shm::ERROR_INVALID_STRIDE, "buffer out of pool")),
                        }
                        released = Some(b);
                    }
                    None => buffer_err = Some(Fatal::new(k.1, wl_surface::ERROR_INVALID_SIZE, "attached object is not a buffer")),
                },
            }
        }
        if (p.sync.acquire.is_some() || p.sync.release.is_some()) && !matches!(update, BufferUpdate::Dmabuf(_)) {
            let sync_id = self.surface(k).and_then(|s| s.sync_obj).unwrap_or(k.1);
            let code = if matches!(update, BufferUpdate::Keep | BufferUpdate::Detach) { wp_linux_drm_syncobj_surface_v1::ERROR_NO_BUFFER } else { wp_linux_drm_syncobj_surface_v1::ERROR_UNSUPPORTED_BUFFER };
            return Err(Fatal::new(sync_id, code, "sync points need a dmabuf buffer"));
        }
        if let Some(b) = released { let _ = core.send(k.0, b, wl_buffer::EVT_RELEASE, vec![]); }
        if let Some(f) = buffer_err { return Err(f); }
        let input_region = match p.input {
            Some(Some(r)) => match self.objs.get(&(k.0, r)) { Some(Obj::Region(v)) => Some(v.clone()), _ => None },
            _ => None,
        };
        let Some(s) = self.surface(k) else { return Ok(()) };
        if let Some(sc) = p.scale { s.scale = sc; }
        if let Some(t) = p.transform { s.transform = t; }
        if let Some(v) = p.viewport { s.vp = v; }
        match &update { BufferUpdate::Attach(_) | BufferUpdate::Dmabuf(_) => s.has_buffer = true, BufferUpdate::Detach => s.has_buffer = false, BufferUpdate::Keep => {} }
        s.frames.extend(p.frames);
        let (scale, transform) = (s.scale, s.transform);
        let (src, size) = (s.vp.src, match (s.vp.dst, s.vp.src) {
            _ if !s.has_buffer => (0, 0),
            (Some(d), _) => d,
            (None, Some(r)) => (r.2.round() as i32, r.3.round() as i32),
            _ => (s.buffer_size.0 / scale.max(1), s.buffer_size.1 / scale.max(1)),
        });
        let is_parent = s.stack.len() > 1 || s.pending_stack.len() > 1;
        self.events.push(UiEvent::Commit { surface: k, buffer: update, scale, transform, input_region, size, src });
        self.xdg_after_apply(core, k);
        if is_parent { self.apply_children(core, k)?; }
        Ok(())
    }

    /// Bounding rows to copy: the union of damage (scaled to buffer pixels), or everything.
    fn damage_rows(&mut self, p: &Pending, spec: &BufferSpec, k: SurfKey) -> (usize, usize) {
        let full = (0, spec.height);
        let (prev_size, scale) = match self.surface(k) { Some(s) => (s.buffer_size, s.scale), None => return full };
        if prev_size != (spec.width as i32, spec.height as i32) { return full; }
        let sc = p.scale.unwrap_or(scale).max(1);
        let mut acc: Option<(i32, i32)> = None;
        let mut add = |y0: i32, y1: i32| { acc = Some(match acc { Some((a, b)) => (a.min(y0), b.max(y1)), None => (y0, y1) }); };
        for r in &p.damage { add(r.y * sc, (r.y + r.h) * sc); }
        for r in &p.buffer_damage { add(r.y, r.y + r.h); }
        match acc {
            Some((a, b)) if b > a => (a.max(0) as usize, (b.max(0) as usize).min(spec.height)),
            _ => full,
        }
    }

    fn apply_children(&mut self, core: &mut Core, k: SurfKey) -> Result<(), Fatal> {
        let Some(s) = self.surface(k) else { return Ok(()) };
        s.stack = s.pending_stack.clone();
        let kids: Vec<u32> = s.stack.iter().copied().filter(|&i| i != 0).collect();
        for kid in &kids {
            let ck = (k.0, *kid);
            let cached = if let Some(cs) = self.surface(ck) {
                if let Some(sub) = &mut cs.sub { sub.pos = sub.pending_pos; }
                cs.cached.take()
            } else { None };
            if let Some(p) = cached { self.apply(core, ck, p)?; }
        }
        self.emit_tree(k);
        Ok(())
    }

    pub(crate) fn emit_tree(&mut self, k: SurfKey) {
        let Some(s) = self.surface(k) else { return };
        let ids = s.stack.clone();
        let stack = ids.iter().map(|&i| if i == 0 { None } else {
            let pos = self.surface((k.0, i)).and_then(|s| s.sub.as_ref().map(|x| x.pos)).unwrap_or((0, 0));
            Some(((k.0, i), pos))
        }).collect();
        self.events.push(UiEvent::SubsurfaceTree { parent: k, stack });
    }

    pub(crate) fn get_subsurface(&mut self, _core: &mut Core, c: ClientId, comp: u32, a: &[Value]) -> Result<(), Fatal> {
        let (id, surf, parent) = (a[0].as_uint(), a[1].as_uint(), a[2].as_uint());
        if surf == parent { return Err(Fatal::new(comp, wl_subcompositor::ERROR_BAD_SURFACE, "a surface cannot be its own parent")); }
        // The parent chain must not contain the child.
        let mut cur = parent;
        for _ in 0..64 {
            if cur == surf { return Err(Fatal::new(comp, wl_subcompositor::ERROR_BAD_PARENT, "parent cycle")); }
            match self.surface((c, cur)).and_then(|s| s.sub.as_ref().map(|x| x.parent)) { Some(p) => cur = p, None => break }
        }
        match self.surface((c, surf)) {
            Some(s) if s.sub.is_none() && s.xdg.is_none() => s.sub = Some(Sub { parent, sync: true, ..Default::default() }),
            _ => return Err(Fatal::new(comp, wl_subcompositor::ERROR_BAD_SURFACE, "surface already has a role")),
        }
        let Some(p) = self.surface((c, parent)) else { return Err(Fatal::new(comp, wl_subcompositor::ERROR_BAD_PARENT, "no such parent")) };
        p.pending_stack.push(surf);
        self.objs.insert((c, id), Obj::Subsurface { surface: surf });
        Ok(())
    }

    pub(crate) fn subsurface_request(&mut self, _core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &[Value]) -> Result<(), Fatal> {
        let Some(Obj::Subsurface { surface }) = self.objs.get(&(c, o.id)) else { return Ok(()) };
        let surf = *surface;
        let Some(parent) = self.surface((c, surf)).and_then(|s| s.sub.as_ref().map(|x| x.parent)) else { return Ok(()) };
        match op {
            wl_subsurface::REQ_SET_POSITION => { if let Some(sub) = self.surface((c, surf)).and_then(|s| s.sub.as_mut()) { sub.pending_pos = (a[0].as_int(), a[1].as_int()); } }
            wl_subsurface::REQ_SET_SYNC => { if let Some(sub) = self.surface((c, surf)).and_then(|s| s.sub.as_mut()) { sub.sync = true; } }
            wl_subsurface::REQ_SET_DESYNC => {
                if let Some(sub) = self.surface((c, surf)).and_then(|s| s.sub.as_mut()) { sub.sync = false; }
                // Leaving sync mode applies what was cached, unless an ancestor is still sync.
                if !self.effectively_sync((c, surf)) {
                    let cached = self.surface((c, surf)).and_then(|s| s.cached.take());
                    if let Some(p) = cached { return self.apply(_core, (c, surf), p); }
                }
            }
            wl_subsurface::REQ_PLACE_ABOVE | wl_subsurface::REQ_PLACE_BELOW => {
                let sib = a[0].as_uint();
                let above = op == wl_subsurface::REQ_PLACE_ABOVE;
                let Some(p) = self.surface((c, parent)) else { return Ok(()) };
                let target = if sib == parent { 0 } else { sib };
                let Some(ti) = p.pending_stack.iter().position(|&i| i == target) else {
                    return Err(Fatal::new(o.id, wl_subsurface::ERROR_BAD_SURFACE, "sibling is not a sibling or the parent"));
                };
                p.pending_stack.retain(|&i| i != surf);
                let ti = p.pending_stack.iter().position(|&i| i == target).unwrap_or(ti);
                p.pending_stack.insert(if above { ti + 1 } else { ti }, surf);
            }
            _ => {}
        }
        Ok(())
    }

    /// Sends wl_callback.done for the surface's waiting frame callbacks.
    pub fn frame_done(&mut self, core: &mut Core, k: SurfKey, time_ms: u32) {
        let Some(s) = self.surface(k) else { return };
        let cbs = std::mem::take(&mut s.frames);
        for cb in cbs {
            let _ = core.send(k.0, cb, wl_callback::EVT_DONE, vec![Value::Uint(time_ms)]);
            // The callback is released to the client right away (delete_id).
            core.destroy_object(self, k.0, cb);
        }
    }

    pub(crate) fn send_output(&mut self, core: &mut Core, c: ClientId, id: u32, version: u32) {
        let o = self.output.clone();
        let _ = core.send(c, id, wl_output::EVT_GEOMETRY, vec![Value::Int(o.x), Value::Int(o.y), Value::Int(o.phys_mm.0), Value::Int(o.phys_mm.1),
            Value::Int(0), Value::Str(Some("WM".into())), Value::Str(Some(o.name.clone())), Value::Int(0)]);
        let _ = core.send(c, id, wl_output::EVT_MODE, vec![Value::Uint(3), Value::Int(o.width * o.scale), Value::Int(o.height * o.scale), Value::Int(o.refresh_mhz)]);
        let _ = core.send(c, id, wl_output::EVT_SCALE, vec![Value::Int(o.scale)]);
        let _ = core.send(c, id, wl_output::EVT_NAME, vec![Value::Str(Some(o.name))]);
        if version >= 2 { let _ = core.send(c, id, wl_output::EVT_DONE, vec![]); }
    }

    pub(crate) fn object_destroyed(&mut self, core: &mut Core, c: ClientId, o: ObjRef) {
        let k = (c, o.id);
        match o.iface.name {
            "wl_surface" => {
                if let Some(Obj::Surface(s)) = self.objs.get(&k) {
                    // Pending frame callbacks are dropped with the surface.
                    let cbs: Vec<u32> = s.frames.iter().chain(s.pending.frames.iter()).copied().collect();
                    let parent = s.sub.as_ref().map(|x| x.parent);
                    let was_xdg = s.xdg.is_some();
                    let _ = (cbs, was_xdg);
                    if let Some(p) = parent {
                        if let Some(ps) = self.surface((c, p)) {
                            ps.stack.retain(|&i| i != o.id);
                            ps.pending_stack.retain(|&i| i != o.id);
                            self.emit_tree((c, p));
                        }
                    }
                }
                self.input.surface_gone(k);
                self.objs.remove(&k);
                self.events.push(UiEvent::SurfaceGone { surface: k });
            }
            "wl_subsurface" => {
                if let Some(Obj::Subsurface { surface }) = self.objs.remove(&k) {
                    let parent = self.surface((c, surface)).and_then(|s| { let p = s.sub.take().map(|x| x.parent); s.cached = None; p });
                    if let Some(p) = parent {
                        if let Some(ps) = self.surface((c, p)) {
                            ps.stack.retain(|&i| i != surface);
                            ps.pending_stack.retain(|&i| i != surface);
                            self.emit_tree((c, p));
                        }
                    }
                }
            }
            "xdg_toplevel" | "xdg_popup" | "xdg_surface" => self.xdg_destroyed(core, c, o),
            "wl_pointer" => { self.input.pointer_gone(c, o.id); self.objs.remove(&k); }
            "wl_keyboard" => { self.input.keyboard_gone(c, o.id); self.objs.remove(&k); }
            "wl_data_source" | "wl_data_device" | "wl_data_offer" | "zwp_primary_selection_source_v1" | "zwp_primary_selection_device_v1" | "zwp_primary_selection_offer_v1" => self.data_object_gone(core, c, o.id),
            "wp_viewport" | "wp_fractional_scale_v1" | "wp_cursor_shape_device_v1" | "zxdg_toplevel_decoration_v1" => self.ext_gone(c, o),
            "wp_linux_drm_syncobj_timeline_v1" | "wp_linux_drm_syncobj_surface_v1" => self.syncobj_gone(c, o),
            "wl_buffer" => {
                if matches!(self.objs.remove(&k), Some(Obj::DmaBuffer(_))) {
                    self.events.push(UiEvent::BufferGone { client: c, buffer: o.id });
                    // Release points still owed must not leave the client waiting.
                    if let (Some(points), Some(d)) = (self.release_points.remove(&k), &self.drm) { for p in points { d.signal_point(p, None); } }
                }
            }
            _ => { self.objs.remove(&k); }
        }
    }
}

impl Surface {
    pub(crate) fn pending_viewport(&mut self) -> &mut VpState { let cur = self.vp; self.pending.viewport.get_or_insert(cur) }
    pub(crate) fn pending_attaches(&self) -> bool { matches!(self.pending.attach, Some(Some(_))) }
}
