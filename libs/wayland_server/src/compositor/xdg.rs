//! xdg_shell: windows, popups, positioners and the configure/ack handshake.
use super::*;

pub const STATE_MAXIMIZED: u32 = 1;
pub const STATE_FULLSCREEN: u32 = 2;
pub const STATE_RESIZING: u32 = 3;
pub const STATE_ACTIVATED: u32 = 4;
pub const STATE_TILED_LEFT: u32 = 5;
pub const STATE_TILED_RIGHT: u32 = 6;
pub const STATE_TILED_TOP: u32 = 7;
pub const STATE_TILED_BOTTOM: u32 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role { None, Toplevel(u32), Popup(u32) }

pub(crate) struct XdgSurface {
    pub surface: u32,
    pub role: Role,
    pub sent: Vec<u32>,
    pub configured: bool,
    pub initial_sent: bool,
    pub mapped: bool,
    pub pending_geometry: Option<Rect>,
    pub pending_limits: Option<((i32, i32), (i32, i32))>,
    pub limits: ((i32, i32), (i32, i32)),
}

fn array_u32(v: &[u32]) -> Value { Value::Array(v.iter().flat_map(|x| x.to_ne_bytes()).collect()) }
fn fatal(o: u32, code: u32, m: &str) -> Fatal { Fatal::new(o, code, m) }

impl Compositor {
    fn xdg(&mut self, c: ClientId, id: u32) -> Option<&mut XdgSurface> {
        match self.objs.get_mut(&(c, id)) { Some(Obj::XdgSurface(x)) => Some(x), _ => None }
    }
    fn xdg_of_surface(&mut self, k: SurfKey) -> Option<u32> { self.surface(k).and_then(|s| s.xdg) }

    pub(crate) fn xdg_request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, a: &[Value]) -> Result<(), Fatal> {
        match o.iface.name {
            "xdg_wm_base" => match op {
                xdg_wm_base::REQ_CREATE_POSITIONER => { self.objs.insert((c, a[0].as_uint()), Obj::Positioner(Positioner::default())); }
                xdg_wm_base::REQ_GET_XDG_SURFACE => {
                    let (id, surf) = (a[0].as_uint(), a[1].as_uint());
                    let Some(s) = self.surface((c, surf)) else { return Ok(()) };
                    if s.xdg.is_some() || s.sub.is_some() { return Err(fatal(o.id, xdg_wm_base::ERROR_ROLE, "surface already has a role")); }
                    if s.has_buffer || s.pending_attaches() { return Err(fatal(o.id, xdg_wm_base::ERROR_INVALID_SURFACE_STATE, "surface already has a buffer")); }
                    s.xdg = Some(id);
                    self.objs.insert((c, id), Obj::XdgSurface(XdgSurface {
                        surface: surf, role: Role::None, sent: Vec::new(), configured: false, initial_sent: false, mapped: false,
                        pending_geometry: None, pending_limits: None, limits: ((0, 0), (0, 0)),
                    }));
                }
                _ => {}
            },
            "xdg_positioner" => {
                let Some(Obj::Positioner(p)) = self.objs.get_mut(&(c, o.id)) else { return Ok(()) };
                match op {
                    xdg_positioner::REQ_SET_SIZE => {
                        if a[0].as_int() <= 0 || a[1].as_int() <= 0 { return Err(fatal(o.id, xdg_positioner::ERROR_INVALID_INPUT, "size must be positive")); }
                        p.size = (a[0].as_int(), a[1].as_int());
                    }
                    xdg_positioner::REQ_SET_ANCHOR_RECT => {
                        if a[2].as_int() < 0 || a[3].as_int() < 0 { return Err(fatal(o.id, xdg_positioner::ERROR_INVALID_INPUT, "negative anchor rect")); }
                        p.anchor_rect = Rect::new(a[0].as_int(), a[1].as_int(), a[2].as_int(), a[3].as_int());
                    }
                    xdg_positioner::REQ_SET_ANCHOR => p.anchor = a[0].as_uint(),
                    xdg_positioner::REQ_SET_GRAVITY => p.gravity = a[0].as_uint(),
                    xdg_positioner::REQ_SET_CONSTRAINT_ADJUSTMENT => p.adjust = a[0].as_uint(),
                    xdg_positioner::REQ_SET_OFFSET => p.offset = (a[0].as_int(), a[1].as_int()),
                    xdg_positioner::REQ_SET_REACTIVE => p.reactive = true,
                    xdg_positioner::REQ_SET_PARENT_SIZE => p.parent_size = Some((a[0].as_int(), a[1].as_int())),
                    _ => {}
                }
            }
            "xdg_surface" => match op {
                xdg_surface::REQ_GET_TOPLEVEL => {
                    let id = a[0].as_uint();
                    let Some(x) = self.xdg(c, o.id) else { return Ok(()) };
                    if x.role != Role::None { return Err(fatal(o.id, xdg_surface::ERROR_ALREADY_CONSTRUCTED, "xdg_surface already has a role")); }
                    x.role = Role::Toplevel(id);
                    let surf = x.surface;
                    self.objs.insert((c, id), Obj::Toplevel { xdg: o.id });
                    self.events.push(UiEvent::ToplevelNew { surface: (c, surf) });
                }
                xdg_surface::REQ_GET_POPUP => {
                    let (id, parent, pos) = (a[0].as_uint(), a[1].as_uint(), a[2].as_uint());
                    let Some(Obj::XdgSurface(px)) = self.objs.get(&(c, parent)) else {
                        return Err(fatal(o.id, xdg_wm_base::ERROR_INVALID_POPUP_PARENT, "popups need an xdg_surface parent"));
                    };
                    let parent_surf = (c, px.surface);
                    let Some(Obj::Positioner(p)) = self.objs.get(&(c, pos)) else { return Ok(()) };
                    let p = p.clone();
                    if p.size.0 <= 0 || p.size.1 <= 0 { return Err(fatal(o.id, xdg_wm_base::ERROR_INVALID_POSITIONER, "positioner is incomplete")); }
                    let Some(x) = self.xdg(c, o.id) else { return Ok(()) };
                    if x.role != Role::None { return Err(fatal(o.id, xdg_surface::ERROR_ALREADY_CONSTRUCTED, "xdg_surface already has a role")); }
                    x.role = Role::Popup(id);
                    self.objs.insert((c, id), Obj::Popup { xdg: o.id, parent: parent_surf, positioner: p, grab: false });
                }
                xdg_surface::REQ_SET_WINDOW_GEOMETRY => {
                    let (w, h) = (a[2].as_int(), a[3].as_int());
                    if w <= 0 || h <= 0 { return Err(fatal(o.id, xdg_surface::ERROR_INVALID_SIZE, "window geometry must be positive")); }
                    if let Some(x) = self.xdg(c, o.id) { x.pending_geometry = Some(Rect::new(a[0].as_int(), a[1].as_int(), w, h)); }
                }
                xdg_surface::REQ_ACK_CONFIGURE => {
                    let serial = a[0].as_uint();
                    let Some(x) = self.xdg(c, o.id) else { return Ok(()) };
                    let Some(i) = x.sent.iter().position(|&s| s == serial) else {
                        return Err(fatal(o.id, xdg_surface::ERROR_INVALID_SERIAL, "ack of a serial that was not sent"));
                    };
                    x.sent.drain(..=i);
                    x.configured = true;
                }
                _ => {}
            },
            "xdg_toplevel" => {
                let Some(Obj::Toplevel { xdg }) = self.objs.get(&(c, o.id)) else { return Ok(()) };
                let xid = *xdg;
                let Some(x) = self.xdg(c, xid) else { return Ok(()) };
                let surface = (c, x.surface);
                let ev = |what| UiEvent::ToplevelRequest { surface, what };
                match op {
                    xdg_toplevel::REQ_SET_TITLE => self.events.push(UiEvent::ToplevelTitle { surface, title: a[0].as_str().to_string() }),
                    xdg_toplevel::REQ_SET_APP_ID => self.events.push(UiEvent::ToplevelAppId { surface, app_id: a[0].as_str().to_string() }),
                    xdg_toplevel::REQ_SET_MAX_SIZE | xdg_toplevel::REQ_SET_MIN_SIZE => {
                        let v = (a[0].as_int(), a[1].as_int());
                        if v.0 < 0 || v.1 < 0 { return Err(fatal(o.id, xdg_toplevel::ERROR_INVALID_SIZE, "negative size limit")); }
                        let mut lim = x.pending_limits.unwrap_or(x.limits);
                        if op == xdg_toplevel::REQ_SET_MIN_SIZE { lim.0 = v } else { lim.1 = v }
                        x.pending_limits = Some(lim);
                    }
                    xdg_toplevel::REQ_MOVE => self.events.push(ev(ToplevelRequest::Move)),
                    xdg_toplevel::REQ_RESIZE => self.events.push(ev(ToplevelRequest::Resize(a[2].as_uint()))),
                    xdg_toplevel::REQ_SHOW_WINDOW_MENU => self.events.push(ev(ToplevelRequest::WindowMenu(a[2].as_int(), a[3].as_int()))),
                    xdg_toplevel::REQ_SET_MAXIMIZED => self.events.push(ev(ToplevelRequest::Maximize(true))),
                    xdg_toplevel::REQ_UNSET_MAXIMIZED => self.events.push(ev(ToplevelRequest::Maximize(false))),
                    xdg_toplevel::REQ_SET_FULLSCREEN => self.events.push(ev(ToplevelRequest::Fullscreen(true))),
                    xdg_toplevel::REQ_UNSET_FULLSCREEN => self.events.push(ev(ToplevelRequest::Fullscreen(false))),
                    xdg_toplevel::REQ_SET_MINIMIZED => self.events.push(ev(ToplevelRequest::Minimize)),
                    _ => {}
                }
            }
            "xdg_popup" => match op {
                xdg_popup::REQ_GRAB => { if let Some(Obj::Popup { grab, .. }) = self.objs.get_mut(&(c, o.id)) { *grab = true; } }
                xdg_popup::REQ_REPOSITION => {
                    let Some(Obj::Positioner(p)) = self.objs.get(&(c, a[0].as_uint())) else { return Ok(()) };
                    let p = p.clone();
                    let token = a[1].as_uint();
                    let Some(Obj::Popup { xdg, positioner, .. }) = self.objs.get_mut(&(c, o.id)) else { return Ok(()) };
                    *positioner = p;
                    let xid = *xdg;
                    let rect = self.popup_rect(c, o.id);
                    let _ = core.send(c, o.id, xdg_popup::EVT_REPOSITIONED, vec![Value::Uint(token)]);
                    self.send_popup_configure(core, c, o.id, xid, rect);
                    if let Some(Obj::XdgSurface(x)) = self.objs.get(&(c, xid)) {
                        self.events.push(UiEvent::PopupMoved { surface: (c, x.surface), rect });
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Ok(())
    }

    fn popup_rect(&mut self, c: ClientId, popup: u32) -> Rect {
        let bounds = self.popup_bounds;
        match self.objs.get(&(c, popup)) { Some(Obj::Popup { positioner, .. }) => positioner.solve(bounds), _ => Rect::default() }
    }
    fn send_popup_configure(&mut self, core: &mut Core, c: ClientId, popup: u32, xdg: u32, r: Rect) {
        let _ = core.send(c, popup, xdg_popup::EVT_CONFIGURE, vec![Value::Int(r.x), Value::Int(r.y), Value::Int(r.w), Value::Int(r.h)]);
        self.send_xdg_configure(core, c, xdg);
    }
    fn send_xdg_configure(&mut self, core: &mut Core, c: ClientId, xdg: u32) {
        let serial = core.next_serial();
        if let Some(x) = self.xdg(c, xdg) { x.sent.push(serial); }
        let _ = core.send(c, xdg, xdg_surface::EVT_CONFIGURE, vec![Value::Uint(serial)]);
    }

    /// Runs at the start of every wl_surface.commit: sends the initial configure, rejects buffers
    /// attached before the first acked configure.
    pub(crate) fn xdg_before_commit(&mut self, core: &mut Core, k: SurfKey) -> Result<(), Fatal> {
        let Some(xid) = self.xdg_of_surface(k) else { return Ok(()) };
        let attaches = self.surface(k).is_some_and(|s| s.pending_attaches());
        let Some(x) = self.xdg(k.0, xid) else { return Ok(()) };
        let role = x.role;
        if role == Role::None { return Err(fatal(xid, xdg_surface::ERROR_NOT_CONSTRUCTED, "commit before the xdg_surface got a role")); }
        if attaches && !x.configured { return Err(fatal(xid, xdg_surface::ERROR_UNCONFIGURED_BUFFER, "buffer attached before the first configure was acked")); }
        if x.initial_sent { return Ok(()); }
        x.initial_sent = true;
        match role {
            Role::Toplevel(t) => {
                let version = core.obj(k.0, t).map_or(1, |o| o.version);
                if version >= 5 { let _ = core.send(k.0, t, xdg_toplevel::EVT_WM_CAPABILITIES, vec![array_u32(&[1, 2, 3, 4])]); }
                let _ = core.send(k.0, t, xdg_toplevel::EVT_CONFIGURE, vec![Value::Int(0), Value::Int(0), array_u32(&[])]);
                self.send_xdg_configure(core, k.0, xid);
            }
            Role::Popup(p) => {
                let rect = self.popup_rect(k.0, p);
                self.send_popup_configure(core, k.0, p, xid, rect);
            }
            Role::None => {}
        }
        Ok(())
    }

    /// Runs after a surface's state was applied: geometry, limits, map and unmap.
    pub(crate) fn xdg_after_apply(&mut self, _core: &mut Core, k: SurfKey) {
        let Some(xid) = self.xdg_of_surface(k) else { return };
        let has_buffer = self.surface(k).is_some_and(|s| s.has_buffer);
        let Some(x) = self.xdg(k.0, xid) else { return };
        let mut out = Vec::new();
        if let Some(g) = x.pending_geometry.take() { out.push(UiEvent::WindowGeometry { surface: k, rect: g }); }
        if let Some(l) = x.pending_limits.take() {
            if l != x.limits { x.limits = l; out.push(UiEvent::ToplevelLimits { surface: k, min: l.0, max: l.1 }); }
        }
        let role = x.role;
        if has_buffer && x.configured && !x.mapped {
            x.mapped = true;
            out.push(UiEvent::Mapped { surface: k });
        } else if !has_buffer && x.mapped {
            x.mapped = false;
            x.configured = false;
            x.initial_sent = false;
            out.push(UiEvent::Unmapped { surface: k });
        }
        self.events.extend(out);
        // A popup announces itself to the host when it first maps.
        if let Role::Popup(p) = role {
            let mapped_now = matches!(self.events.last(), Some(UiEvent::Mapped { surface }) if *surface == k);
            if mapped_now {
                self.events.pop();
                let rect = self.popup_rect(k.0, p);
                if let Some(Obj::Popup { parent, grab, .. }) = self.objs.get(&(k.0, p)) {
                    let (parent, grab) = (*parent, *grab);
                    self.events.push(UiEvent::PopupNew { surface: k, parent, rect, grab });
                }
            }
        }
    }

    pub(crate) fn xdg_destroyed(&mut self, _core: &mut Core, c: ClientId, o: ObjRef) {
        let k = (c, o.id);
        match o.iface.name {
            "xdg_toplevel" | "xdg_popup" => {
                let xid = match self.objs.remove(&k) { Some(Obj::Toplevel { xdg }) | Some(Obj::Popup { xdg, .. }) => xdg, _ => return };
                if let Some(x) = self.xdg(c, xid) {
                    x.role = Role::None;
                    let (surf, was) = (x.surface, x.mapped);
                    x.mapped = false; x.configured = false; x.initial_sent = false;
                    if was { self.events.push(UiEvent::Unmapped { surface: (c, surf) }); }
                }
            }
            _ => {
                if let Some(Obj::XdgSurface(x)) = self.objs.remove(&k) {
                    if let Some(s) = self.surface((c, x.surface)) { s.xdg = None; }
                }
            }
        }
    }

    /// Host side: tell a window its new size and states. Sends xdg_toplevel.configure then xdg_surface.configure.
    pub fn configure_toplevel(&mut self, core: &mut Core, k: SurfKey, w: i32, h: i32, states: &[u32]) {
        let Some(xid) = self.xdg_of_surface(k) else { return };
        let Some(Role::Toplevel(t)) = self.xdg(k.0, xid).map(|x| x.role) else { return };
        let _ = core.send(k.0, t, xdg_toplevel::EVT_CONFIGURE, vec![Value::Int(w), Value::Int(h), array_u32(states)]);
        self.send_xdg_configure(core, k.0, xid);
    }
    pub fn close_toplevel(&mut self, core: &mut Core, k: SurfKey) {
        let Some(xid) = self.xdg_of_surface(k) else { return };
        if let Some(Role::Toplevel(t)) = self.xdg(k.0, xid).map(|x| x.role) { let _ = core.send(k.0, t, xdg_toplevel::EVT_CLOSE, vec![]); }
    }
    /// Host side: dismiss a popup (focus loss or a click outside its grab chain).
    pub fn dismiss_popup(&mut self, core: &mut Core, k: SurfKey) {
        let Some(xid) = self.xdg_of_surface(k) else { return };
        if let Some(Role::Popup(p)) = self.xdg(k.0, xid).map(|x| x.role) { let _ = core.send(k.0, p, xdg_popup::EVT_POPUP_DONE, vec![]); }
    }
}
