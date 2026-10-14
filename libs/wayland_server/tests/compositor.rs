// The compositor is Linux-only (the crate's modules are cfg(target_os = "linux")).
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wayland_server::compositor::*;
use wayland_server::proto::*;
use wayland_server::schema::{ArgType, Interface};
use wayland_server::server::Server;
use wayland_server::wire::*;

struct Ev { id: u32, iface: &'static str, name: &'static str, args: Vec<Value> }

struct Client {
    s: UnixStream,
    buf: Vec<u8>,
    fds: std::collections::VecDeque<OwnedFd>,
    ifaces: HashMap<u32, &'static Interface>,
    next: u32,
}
impl Client {
    fn new(s: UnixStream) -> Self {
        s.set_nonblocking(true).unwrap();
        let mut ifaces = HashMap::new();
        ifaces.insert(1, &wl_display::INTERFACE);
        Self { s, buf: Vec::new(), fds: Default::default(), ifaces, next: 2 }
    }
    fn alloc(&mut self, i: &'static Interface) -> u32 { let id = self.next; self.next += 1; self.ifaces.insert(id, i); id }
    fn req(&mut self, id: u32, op: u16, args: Vec<Value>) {
        let iface = self.ifaces[&id];
        let mut fds = Vec::new();
        let bytes = encode(id, op, iface.requests[op as usize].args, args, &mut fds).unwrap();
        if fds.is_empty() { self.s.write_all(&bytes).unwrap(); return; }
        use wayland_server::sys::*;
        let raw: Vec<i32> = fds.iter().map(|f| f.as_raw_fd()).collect();
        let mut ctl = [0u64; 16];
        let base = ctl.as_mut_ptr() as *mut u8;
        unsafe {
            std::ptr::write(base as *mut cmsghdr, cmsghdr { len: cmsg_len(raw.len() * 4), level: SOL_SOCKET, kind: SCM_RIGHTS });
            std::ptr::copy_nonoverlapping(raw.as_ptr() as *const u8, base.add(cmsg_len(0)), raw.len() * 4);
        }
        let mut iov = iovec { base: bytes.as_ptr() as *mut _, len: bytes.len() };
        let msg = msghdr { name: std::ptr::null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: ctl.as_mut_ptr() as *mut _, controllen: cmsg_space(raw.len() * 4), flags: 0 };
        assert!(unsafe { sendmsg(self.s.as_raw_fd(), &msg, 0) } > 0);
    }
    /// Creates an object of `i` through request `op` of `parent` with the new id first.
    fn create(&mut self, parent: u32, op: u16, i: &'static Interface, rest: Vec<Value>) -> u32 {
        let id = self.alloc(i);
        let mut a = vec![Value::NewId(id)];
        a.extend(rest);
        self.req(parent, op, a);
        id
    }
    /// Reads whatever the server has sent, receiving fds with recvmsg.
    fn events(&mut self) -> Vec<Ev> {
        use wayland_server::sys::*;
        loop {
            let mut tmp = [0u8; 8192];
            let mut ctl = [0u64; 16];
            let mut iov = iovec { base: tmp.as_mut_ptr() as *mut _, len: tmp.len() };
            let mut msg = msghdr { name: std::ptr::null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: ctl.as_mut_ptr() as *mut _, controllen: 128, flags: 0 };
            let n = unsafe { recvmsg(self.s.as_raw_fd(), &mut msg, MSG_DONTWAIT | MSG_CMSG_CLOEXEC) };
            if n <= 0 { break; }
            let h = unsafe { std::ptr::read_unaligned(ctl.as_ptr() as *const cmsghdr) };
            if msg.controllen >= 16 && h.level == SOL_SOCKET && h.kind == SCM_RIGHTS {
                for i in 0..(h.len - cmsg_len(0)) / 4 {
                    let fd = unsafe { std::ptr::read_unaligned((ctl.as_ptr() as *const u8).add(cmsg_len(0) + i * 4) as *const i32) };
                    use std::os::fd::FromRawFd;
                    self.fds.push_back(unsafe { OwnedFd::from_raw_fd(fd) });
                }
            }
            self.buf.extend_from_slice(&tmp[..n as usize]);
        }
        let mut out = Vec::new();
        while let Some((id, op, size)) = peek_header(&self.buf) {
            if self.buf.len() < size { break; }
            let iface = self.ifaces[&id];
            let m = &iface.events[op as usize];
            let args = decode_args(&self.buf[8..size], m.args, &mut self.fds).unwrap();
            for (t, v) in m.args.iter().zip(&args) {
                if let (ArgType::NewId(i), Value::NewId(n)) = (t, v) { self.ifaces.insert(*n, find_iface(i)); }
            }
            out.push(Ev { id, iface: iface.name, name: m.name, args });
            self.buf.drain(..size);
        }
        out
    }
}
fn find_iface(n: &str) -> &'static Interface { wayland_server::schema::find(n).unwrap() }

struct World { srv: Server, comp: Compositor, c: Client }
impl World {
    fn new() -> World {
        let mut srv = Server::new().unwrap();
        let mut comp = Compositor::new(&mut srv.core, OutputInfo::default(), &xkb_keymap::US).unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        srv.add_client(&mut comp, a).unwrap();
        World { srv, comp, c: Client::new(b) }
    }
    fn pump(&mut self) -> Vec<Ev> {
        for _ in 0..3 { self.srv.run_once(&mut self.comp, 20).unwrap(); }
        self.srv.settle(&mut self.comp);
        self.c.events()
    }
    /// Binds a global by interface name.
    fn bind(&mut self, reg: u32, evs: &[Ev], name: &str, version: u32) -> u32 {
        let g = evs.iter().find(|e| e.name == "global" && e.args[1].as_str() == name).unwrap_or_else(|| panic!("no global {name}"));
        let gname = g.args[0].as_uint();
        let i = find_iface(name);
        let id = self.c.alloc(i);
        self.c.req(reg, wl_registry::REQ_BIND, vec![Value::Uint(gname), Value::NewIdDyn { interface: name.into(), version, id }]);
        id
    }
    fn setup(&mut self) -> (u32, Vec<Ev>) {
        let reg = self.c.create(1, wl_display::REQ_GET_REGISTRY, &wl_registry::INTERFACE, vec![]);
        let evs = self.pump();
        (reg, evs)
    }
}
fn memfile(tag: &str, data: &[u8]) -> (OwnedFd, std::fs::File, std::path::PathBuf) {
    let p = std::env::temp_dir().join(format!("wsrv-comp-{}-{tag}", std::process::id()));
    let mut f = std::fs::File::options().read(true).write(true).create(true).truncate(true).open(&p).unwrap();
    f.write_all(data).unwrap();
    (OwnedFd::from(f.try_clone().unwrap()), f, p)
}
fn named<'a>(evs: &'a [Ev], n: &str) -> Vec<&'a Ev> { evs.iter().filter(|e| e.name == n).collect() }

struct Win { surf: u32, xdg: u32, top: u32 }
fn open_window(w: &mut World, reg: u32, evs: &[Ev]) -> (Win, u32, u32) {
    let comp = w.bind(reg, evs, "wl_compositor", 6);
    let shm = w.bind(reg, evs, "wl_shm", 1);
    let wm = w.bind(reg, evs, "xdg_wm_base", 6);
    let surf = w.c.create(comp, wl_compositor::REQ_CREATE_SURFACE, &wl_surface::INTERFACE, vec![]);
    let xdg = w.c.create(wm, xdg_wm_base::REQ_GET_XDG_SURFACE, &xdg_surface::INTERFACE, vec![Value::Object(surf)]);
    let top = w.c.create(xdg, xdg_surface::REQ_GET_TOPLEVEL, &xdg_toplevel::INTERFACE, vec![]);
    w.c.req(top, xdg_toplevel::REQ_SET_TITLE, vec![Value::Str(Some("hello".into()))]);
    w.c.req(top, xdg_toplevel::REQ_SET_APP_ID, vec![Value::Str(Some("app.test".into()))]);
    w.c.req(surf, wl_surface::REQ_COMMIT, vec![]);
    (Win { surf, xdg, top }, shm, wm)
}

#[test]
fn window_configure_ack_buffer_map_release() {
    let mut w = World::new();
    let (reg, evs) = w.setup();
    assert_eq!(named(&evs, "global").len(), 12);
    let (win, shm, _wm) = open_window(&mut w, reg, &evs);
    let evs = w.pump();
    let conf = named(&evs, "configure");
    assert_eq!(conf.len(), 2); // toplevel + xdg_surface
    let serial = conf.iter().find(|e| e.iface == "xdg_surface").unwrap().args[0].as_uint();
    let ui = w.comp.take_events();
    assert!(ui.iter().any(|e| matches!(e, UiEvent::ToplevelNew { .. })));
    assert!(ui.iter().any(|e| matches!(e, UiEvent::ToplevelTitle { title, .. } if title == "hello")));
    w.c.req(win.xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(serial)]);
    // a 4x2 XRGB buffer
    let px: Vec<u8> = (0..32u8).collect();
    let (fd, _f, path) = memfile("buf", &px);
    let pool = w.c.create(shm, wl_shm::REQ_CREATE_POOL, &wl_shm_pool::INTERFACE, vec![Value::Fd(fd), Value::Int(32)]);
    let buf = w.c.create(pool, wl_shm_pool::REQ_CREATE_BUFFER, &wl_buffer::INTERFACE, vec![Value::Int(0), Value::Int(4), Value::Int(2), Value::Int(16), Value::Uint(1)]);
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_DAMAGE_BUFFER, vec![Value::Int(0), Value::Int(0), Value::Int(4), Value::Int(2)]);
    let cb = w.c.create(win.surf, wl_surface::REQ_FRAME, &wl_callback::INTERFACE, vec![]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    let evs = w.pump();
    assert_eq!(named(&evs, "release").len(), 1);
    let ui = w.comp.take_events();
    assert!(ui.iter().any(|e| matches!(e, UiEvent::Mapped { .. })));
    let frame = ui.iter().find_map(|e| match e { UiEvent::Commit { buffer: BufferUpdate::Attach(f), .. } => Some(f), _ => None }).expect("frame");
    assert_eq!((frame.width, frame.height, frame.y0, frame.y1), (4, 2, 0, 2));
    assert_eq!(frame.data, px);
    // frame callback fires on frame_done
    w.comp.frame_done(&mut w.srv.core, (1, win.surf), 1234);
    let evs = w.pump();
    let done = evs.iter().find(|e| e.id == cb && e.name == "done").unwrap();
    assert_eq!(done.args[0].as_uint(), 1234);
    // host resizes the window
    w.comp.configure_toplevel(&mut w.srv.core, (1, win.surf), 800, 600, &[STATE_ACTIVATED_FOR_TEST]);
    let evs = w.pump();
    let c = named(&evs, "configure");
    assert_eq!(c[0].id, win.top);
    assert_eq!((c[0].args[0].as_int(), c[0].args[1].as_int()), (800, 600));
    // partial damage copies only the damaged rows
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_DAMAGE_BUFFER, vec![Value::Int(0), Value::Int(1), Value::Int(4), Value::Int(1)]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    w.pump();
    let ui = w.comp.take_events();
    let frame = ui.iter().find_map(|e| match e { UiEvent::Commit { buffer: BufferUpdate::Attach(f), .. } => Some(f), _ => None }).unwrap();
    assert_eq!((frame.y0, frame.y1, frame.data.len()), (1, 2, 16));
    std::fs::remove_file(path).unwrap();
}
const STATE_ACTIVATED_FOR_TEST: u32 = xdg::STATE_ACTIVATED;

#[test]
fn buffer_before_ack_is_an_error() {
    let mut w = World::new();
    let (reg, evs) = w.setup();
    let (win, shm, _) = open_window(&mut w, reg, &evs);
    w.pump();
    let (fd, _f, path) = memfile("early", &[0u8; 16]);
    let pool = w.c.create(shm, wl_shm::REQ_CREATE_POOL, &wl_shm_pool::INTERFACE, vec![Value::Fd(fd), Value::Int(16)]);
    let buf = w.c.create(pool, wl_shm_pool::REQ_CREATE_BUFFER, &wl_buffer::INTERFACE, vec![Value::Int(0), Value::Int(2), Value::Int(2), Value::Int(8), Value::Uint(0)]);
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    let evs = w.pump();
    let e = named(&evs, "error");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].args[1].as_uint(), xdg_surface::ERROR_UNCONFIGURED_BUFFER);
    assert!(w.srv.core.client_ids().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn truncated_pool_kills_the_client_not_the_server() {
    let mut w = World::new();
    let (reg, evs) = w.setup();
    let (win, shm, _) = open_window(&mut w, reg, &evs);
    let evs = w.pump();
    let serial = evs.iter().find(|e| e.iface == "xdg_surface" && e.name == "configure").unwrap().args[0].as_uint();
    w.c.req(win.xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(serial)]);
    let (fd, f, path) = memfile("trunc", &vec![7u8; 16384]);
    let pool = w.c.create(shm, wl_shm::REQ_CREATE_POOL, &wl_shm_pool::INTERFACE, vec![Value::Fd(fd), Value::Int(16384)]);
    let buf = w.c.create(pool, wl_shm_pool::REQ_CREATE_BUFFER, &wl_buffer::INTERFACE, vec![Value::Int(0), Value::Int(64), Value::Int(64), Value::Int(256), Value::Uint(0)]);
    w.pump();
    f.set_len(0).unwrap();
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    let evs = w.pump();
    assert_eq!(named(&evs, "error").len(), 1);
    assert!(w.srv.core.client_ids().is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn popup_is_positioned_and_grab_dismissed() {
    let mut w = World::new();
    let (reg, evs) = w.setup();
    let (win, _shm, wm) = open_window(&mut w, reg, &evs);
    let comp_id = w.c.ifaces.iter().find(|(_, i)| i.name == "wl_compositor").map(|(id, _)| *id).unwrap();
    let seat = w.bind(reg, &evs, "wl_seat", 8);
    w.pump();
    let pos = w.c.create(wm, xdg_wm_base::REQ_CREATE_POSITIONER, &xdg_positioner::INTERFACE, vec![]);
    w.c.req(pos, xdg_positioner::REQ_SET_SIZE, vec![Value::Int(100), Value::Int(50)]);
    w.c.req(pos, xdg_positioner::REQ_SET_ANCHOR_RECT, vec![Value::Int(10), Value::Int(10), Value::Int(40), Value::Int(20)]);
    w.c.req(pos, xdg_positioner::REQ_SET_ANCHOR, vec![Value::Uint(xdg_positioner::ANCHOR_BOTTOM_LEFT)]);
    w.c.req(pos, xdg_positioner::REQ_SET_GRAVITY, vec![Value::Uint(xdg_positioner::GRAVITY_BOTTOM_RIGHT)]);
    let psurf = w.c.create(comp_id, wl_compositor::REQ_CREATE_SURFACE, &wl_surface::INTERFACE, vec![]);
    let pxdg = w.c.create(wm, xdg_wm_base::REQ_GET_XDG_SURFACE, &xdg_surface::INTERFACE, vec![Value::Object(psurf)]);
    let popup = w.c.create(pxdg, xdg_surface::REQ_GET_POPUP, &xdg_popup::INTERFACE, vec![Value::Object(win.xdg), Value::Object(pos)]);
    w.c.req(popup, xdg_popup::REQ_GRAB, vec![Value::Object(seat), Value::Uint(1)]);
    w.c.req(psurf, wl_surface::REQ_COMMIT, vec![]);
    let evs = w.pump();
    let c = evs.iter().find(|e| e.iface == "xdg_popup" && e.name == "configure").unwrap();
    assert_eq!((c.args[0].as_int(), c.args[1].as_int(), c.args[2].as_int(), c.args[3].as_int()), (10, 30, 100, 50));
    w.comp.dismiss_popup(&mut w.srv.core, (1, psurf));
    let evs = w.pump();
    assert_eq!(named(&evs, "popup_done").len(), 1);
}

#[test]
fn seat_keyboard_and_pointer_events() {
    let mut w = World::new();
    let (reg, evs) = w.setup();
    let (win, _shm, _) = open_window(&mut w, reg, &evs);
    let seat = w.bind(reg, &evs, "wl_seat", 8);
    let kb = w.c.create(seat, wl_seat::REQ_GET_KEYBOARD, &wl_keyboard::INTERFACE, vec![]);
    let ptr = w.c.create(seat, wl_seat::REQ_GET_POINTER, &wl_pointer::INTERFACE, vec![]);
    let evs = w.pump();
    let keymap = evs.iter().find(|e| e.name == "keymap").unwrap();
    assert_eq!(keymap.id, kb);
    let size = keymap.args[2].as_uint() as usize;
    let Value::Fd(fd) = &keymap.args[1] else { panic!() };
    let mut text = String::new();
    use std::io::Seek;
    let mut f = std::fs::File::from(fd.try_clone().unwrap());
    f.seek(std::io::SeekFrom::Start(0)).unwrap();
    f.read_to_string(&mut text).unwrap();
    assert_eq!(text.len(), size);
    assert!(text.starts_with("xkb_keymap {") && text.ends_with("\0"));
    assert_eq!(named(&evs, "repeat_info").len(), 1);
    // focus + typing shift+a
    w.comp.keyboard_focus(&mut w.srv.core, Some((1, win.surf)));
    w.comp.key(&mut w.srv.core, 10, 42, true);
    w.comp.key(&mut w.srv.core, 11, 30, true);
    w.comp.key(&mut w.srv.core, 12, 30, false);
    let evs = w.pump();
    let names: Vec<&str> = evs.iter().map(|e| e.name).collect();
    assert_eq!(names, ["enter", "modifiers", "key", "modifiers", "key", "key"]);
    assert_eq!(evs[3].args[1].as_uint(), 1); // shift depressed
    // pointer
    w.comp.pointer_enter(&mut w.srv.core, (1, win.surf), 5.5, 6.0);
    w.comp.pointer_motion(&mut w.srv.core, 20, 7.0, 8.0);
    w.comp.pointer_button(&mut w.srv.core, 21, 0x110, true);
    w.comp.pointer_axis(&mut w.srv.core, 22, 0, 10.0, 120);
    let evs = w.pump();
    let p: Vec<&str> = evs.iter().filter(|e| e.id == ptr).map(|e| e.name).collect();
    assert_eq!(p, ["enter", "frame", "motion", "frame", "button", "frame", "axis_source", "axis", "axis_value120", "frame"]);
    assert_eq!(evs[0].args[2].as_f64(), 5.5);
}

#[test]
fn sync_subsurface_commit_waits_for_parent() {
    let mut w = World::new();
    let (reg, evs0) = w.setup();
    let (win, shm, _) = open_window(&mut w, reg, &evs0);
    let evs = w.pump();
    let serial = evs.iter().find(|e| e.iface == "xdg_surface" && e.name == "configure").unwrap().args[0].as_uint();
    w.c.req(win.xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(serial)]);
    let sub = w.bind(reg, &evs0, "wl_subcompositor", 1);
    let comp_id = w.c.ifaces.iter().find(|(_, i)| i.name == "wl_compositor").map(|(id, _)| *id).unwrap();
    let child = w.c.create(comp_id, wl_compositor::REQ_CREATE_SURFACE, &wl_surface::INTERFACE, vec![]);
    let ss = w.c.create(sub, wl_subcompositor::REQ_GET_SUBSURFACE, &wl_subsurface::INTERFACE, vec![Value::Object(child), Value::Object(win.surf)]);
    w.c.req(ss, wl_subsurface::REQ_SET_POSITION, vec![Value::Int(30), Value::Int(40)]);
    let (fd, _f, path) = memfile("sub", &[1u8; 16]);
    let pool = w.c.create(shm, wl_shm::REQ_CREATE_POOL, &wl_shm_pool::INTERFACE, vec![Value::Fd(fd), Value::Int(16)]);
    let buf = w.c.create(pool, wl_shm_pool::REQ_CREATE_BUFFER, &wl_buffer::INTERFACE, vec![Value::Int(0), Value::Int(2), Value::Int(2), Value::Int(8), Value::Uint(0)]);
    w.c.req(child, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(child, wl_surface::REQ_COMMIT, vec![]);
    w.pump();
    assert!(!w.comp.take_events().iter().any(|e| matches!(e, UiEvent::Commit { surface, .. } if *surface == (1, child))));
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    w.pump();
    let ui = w.comp.take_events();
    assert!(ui.iter().any(|e| matches!(e, UiEvent::Commit { surface, buffer: BufferUpdate::Attach(_), .. } if *surface == (1, child))));
    let tree = ui.iter().find_map(|e| match e { UiEvent::SubsurfaceTree { stack, .. } => Some(stack), _ => None }).unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[1].unwrap(), ((1, child), (30, 40)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn clipboard_selection_between_clients() {
    let mut w = World::new();
    // second client
    let (a, b) = UnixStream::pair().unwrap();
    w.srv.add_client(&mut w.comp, a).unwrap();
    let mut c2 = Client::new(b);
    // client 1: window, data device, a source
    let (reg, evs) = w.setup();
    let (win1, _shm, _) = open_window(&mut w, reg, &evs);
    let seat1 = w.bind(reg, &evs, "wl_seat", 8);
    let mgr1 = w.bind(reg, &evs, "wl_data_device_manager", 3);
    let dev1 = w.c.create(mgr1, wl_data_device_manager::REQ_GET_DATA_DEVICE, &wl_data_device::INTERFACE, vec![Value::Object(seat1)]);
    let src = w.c.create(mgr1, wl_data_device_manager::REQ_CREATE_DATA_SOURCE, &wl_data_source::INTERFACE, vec![]);
    w.c.req(src, wl_data_source::REQ_OFFER, vec![Value::Str(Some("text/plain;charset=utf-8".into()))]);
    w.c.req(dev1, wl_data_device::REQ_SET_SELECTION, vec![Value::Object(src), Value::Uint(1)]);
    w.pump();
    // client 2: window with keyboard focus and a data device
    let reg2 = c2.create(1, wl_display::REQ_GET_REGISTRY, &wl_registry::INTERFACE, vec![]);
    for _ in 0..3 { w.srv.run_once(&mut w.comp, 20).unwrap(); }
    let evs2 = c2.events();
    let bind2 = |c2: &mut Client, name: &str, v: u32| {
        let g = evs2.iter().find(|e| e.name == "global" && e.args[1].as_str() == name).unwrap();
        let id = c2.alloc(find_iface(name));
        c2.req(reg2, wl_registry::REQ_BIND, vec![Value::Uint(g.args[0].as_uint()), Value::NewIdDyn { interface: name.into(), version: v, id }]);
        id
    };
    let comp2 = bind2(&mut c2, "wl_compositor", 6);
    let wm2 = bind2(&mut c2, "xdg_wm_base", 6);
    let seat2 = bind2(&mut c2, "wl_seat", 8);
    let mgr2 = bind2(&mut c2, "wl_data_device_manager", 3);
    let surf2 = c2.create(comp2, wl_compositor::REQ_CREATE_SURFACE, &wl_surface::INTERFACE, vec![]);
    let _ = wm2;
    let dev2 = c2.create(mgr2, wl_data_device_manager::REQ_GET_DATA_DEVICE, &wl_data_device::INTERFACE, vec![Value::Object(seat2)]);
    for _ in 0..3 { w.srv.run_once(&mut w.comp, 20).unwrap(); }
    c2.events();
    w.comp.keyboard_focus(&mut w.srv.core, Some((2, surf2)));
    for _ in 0..3 { w.srv.run_once(&mut w.comp, 20).unwrap(); }
    w.srv.settle(&mut w.comp);
    let evs = c2.events();
    let names: Vec<&str> = evs.iter().filter(|e| e.id == dev2 || e.iface == "wl_data_offer").map(|e| e.name).collect();
    assert_eq!(names, ["data_offer", "offer", "selection"]);
    let offer = evs.iter().find(|e| e.name == "selection").unwrap().args[0].as_uint();
    assert!(offer >= 0xff00_0000);
    // receive: client 1's source gets send with a usable fd
    let (rd, wr) = {
        use std::os::fd::FromRawFd;
        let mut fds = [0i32; 2];
        extern "C" { fn pipe(fds: *mut i32) -> i32; }
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        unsafe { (std::fs::File::from_raw_fd(fds[0]), std::fs::File::from_raw_fd(fds[1])) }
    };
    c2.req(offer, wl_data_offer::REQ_RECEIVE, vec![Value::Str(Some("text/plain;charset=utf-8".into())), Value::Fd(OwnedFd::from(wr))]);
    for _ in 0..3 { w.srv.run_once(&mut w.comp, 20).unwrap(); }
    w.srv.settle(&mut w.comp);
    let evs = w.c.events();
    let send = evs.iter().find(|e| e.id == src && e.name == "send").expect("send event");
    assert_eq!(send.args[0].as_str(), "text/plain;charset=utf-8");
    let Value::Fd(fd) = &send.args[1] else { panic!() };
    std::fs::File::from(fd.try_clone().unwrap()).write_all(b"pasted").unwrap();
    drop(evs);
    let mut text = String::new();
    let mut rd = rd;
    rd.read_to_string(&mut text).unwrap();
    assert_eq!(text, "pasted");
    let _ = win1;
}

fn pipe() -> (std::fs::File, OwnedFd) {
    use std::os::fd::FromRawFd;
    let mut fds = [0i32; 2];
    extern "C" { fn pipe(fds: *mut i32) -> i32; }
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
    unsafe { (std::fs::File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}
fn turn(w: &mut World) { for _ in 0..3 { w.srv.run_once(&mut w.comp, 20).unwrap(); } w.srv.settle(&mut w.comp); }

/// A second client with its own registry; `bind` binds a global by name.
struct Other { c: Client, reg: u32, globals: Vec<Ev> }
impl Other {
    fn join(w: &mut World) -> Other {
        let (a, b) = UnixStream::pair().unwrap();
        w.srv.add_client(&mut w.comp, a).unwrap();
        let mut c = Client::new(b);
        let reg = c.create(1, wl_display::REQ_GET_REGISTRY, &wl_registry::INTERFACE, vec![]);
        turn(w);
        let globals = c.events();
        Other { c, reg, globals }
    }
    fn bind(&mut self, name: &str, v: u32) -> u32 {
        let g = self.globals.iter().find(|e| e.name == "global" && e.args[1].as_str() == name).unwrap();
        let gname = g.args[0].as_uint();
        let id = self.c.alloc(find_iface(name));
        self.c.req(self.reg, wl_registry::REQ_BIND, vec![Value::Uint(gname), Value::NewIdDyn { interface: name.into(), version: v, id }]);
        id
    }
    /// A surface, a clipboard device and a primary-selection device.
    fn devices(&mut self) -> (u32, u32, u32) {
        let comp = self.bind("wl_compositor", 6);
        let seat = self.bind("wl_seat", 8);
        let mgr = self.bind("wl_data_device_manager", 3);
        let pmgr = self.bind("zwp_primary_selection_device_manager_v1", 1);
        let surf = self.c.create(comp, wl_compositor::REQ_CREATE_SURFACE, &wl_surface::INTERFACE, vec![]);
        let _keyboard = self.c.create(seat, wl_seat::REQ_GET_KEYBOARD, &wl_keyboard::INTERFACE, vec![]);
        let dev = self.c.create(mgr, wl_data_device_manager::REQ_GET_DATA_DEVICE, &wl_data_device::INTERFACE, vec![Value::Object(seat)]);
        let pdev = self.c.create(pmgr, zwp_primary_selection_device_manager_v1::REQ_GET_DEVICE, &zwp_primary_selection_device_v1::INTERFACE, vec![Value::Object(seat)]);
        (surf, dev, pdev)
    }
    /// The offer of the last `selection` event of `dev`.
    fn selection(evs: &[Ev], dev: u32) -> u32 { evs.iter().filter(|e| e.id == dev && e.name == "selection").last().expect("selection event").args[0].as_uint() }
    fn paste(&mut self, w: &mut World, offer: u32, primary: bool, mime: &str) -> String {
        let (mut rd, wr) = pipe();
        let op = if primary { zwp_primary_selection_offer_v1::REQ_RECEIVE } else { wl_data_offer::REQ_RECEIVE };
        self.c.req(offer, op, vec![Value::Str(Some(mime.into())), Value::Fd(wr)]);
        turn(w);
        let mut text = String::new();
        rd.read_to_string(&mut text).unwrap();
        text
    }
}

/// Text a host app copied is every Wayland client's selection, offered as the usual text types,
/// for the clipboard and the primary selection; it comes before the keyboard enter.
#[test]
fn host_text_is_the_clients_selection() {
    let mut w = World::new();
    let _ = w.setup();
    let mut o = Other::join(&mut w);
    let (surf, dev, pdev) = o.devices();
    turn(&mut w);
    o.c.events();
    w.comp.set_host_selection(&mut w.srv.core, Board::Clipboard, "from a makepad app".into());
    w.comp.set_host_selection(&mut w.srv.core, Board::Primary, "selected in makepad".into());
    turn(&mut w);
    assert!(o.c.events().is_empty(), "no selection before the keyboard");
    w.comp.keyboard_focus(&mut w.srv.core, Some((2, surf)));
    turn(&mut w);
    let evs = o.c.events();
    let order: Vec<&str> = evs.iter().filter(|e| e.name == "selection" || e.name == "enter").map(|e| e.name).collect();
    assert_eq!(order, ["selection", "selection", "enter"], "{order:?}");
    let offer = Other::selection(&evs, dev);
    let mimes: Vec<&str> = evs.iter().filter(|e| e.id == offer && e.name == "offer").map(|e| e.args[0].as_str()).collect();
    assert_eq!(mimes, TEXT_MIMES);
    for mime in ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"] {
        assert_eq!(o.paste(&mut w, offer, false, mime), "from a makepad app");
    }
    let poffer = Other::selection(&evs, pdev);
    assert_eq!(o.paste(&mut w, poffer, true, "text/plain;charset=utf-8"), "selected in makepad");
    // a newer host copy: the focused client hears it at once, the old offer is stale (closed unanswered)
    w.comp.set_host_selection(&mut w.srv.core, Board::Clipboard, "second".into());
    turn(&mut w);
    let evs = o.c.events();
    let newer = Other::selection(&evs, dev);
    assert_ne!(newer, offer);
    assert_eq!(o.paste(&mut w, offer, false, "text/plain"), "");
    assert_eq!(o.paste(&mut w, newer, false, "text/plain"), "second");
}

/// A client's copy is read once for the host (UiEvent::Selection), other clients paste it from the
/// source, and when the source goes (its app quit) its text stays the selection.
#[test]
fn client_selection_reaches_the_host_and_outlives_its_source() {
    let mut w = World::new();
    let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let r2 = reads.clone();
    w.comp.set_selection_reader(std::sync::Arc::new(move |b, g, t| r2.lock().unwrap().push((b, g, t))));
    let (reg, evs) = w.setup();
    let seat1 = w.bind(reg, &evs, "wl_seat", 8);
    let mgr1 = w.bind(reg, &evs, "wl_data_device_manager", 3);
    let dev1 = w.c.create(mgr1, wl_data_device_manager::REQ_GET_DATA_DEVICE, &wl_data_device::INTERFACE, vec![Value::Object(seat1)]);
    let mut o = Other::join(&mut w);
    let (surf2, dev2, _) = o.devices();
    turn(&mut w);
    w.comp.keyboard_focus(&mut w.srv.core, Some((2, surf2)));
    turn(&mut w);
    o.c.events();
    // client 1 copies: offers html and text; the server asks for the text
    let src = w.c.create(mgr1, wl_data_device_manager::REQ_CREATE_DATA_SOURCE, &wl_data_source::INTERFACE, vec![]);
    for m in ["text/html", "UTF8_STRING", "text/plain;charset=utf-8"] { w.c.req(src, wl_data_source::REQ_OFFER, vec![Value::Str(Some(m.into()))]); }
    w.c.req(dev1, wl_data_device::REQ_SET_SELECTION, vec![Value::Object(src), Value::Uint(1)]);
    turn(&mut w);
    let evs = w.c.events();
    let send = evs.iter().find(|e| e.id == src && e.name == "send").expect("the server reads the copy");
    assert_eq!(send.args[0].as_str(), "text/plain;charset=utf-8");
    let Value::Fd(fd) = &send.args[1] else { panic!() };
    std::fs::File::from(fd.try_clone().unwrap()).write_all("copied in a client ✓".as_bytes()).unwrap();
    drop(evs);
    let read = loop {
        if let Some(r) = reads.lock().unwrap().pop() { break r; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(read.0, Board::Clipboard);
    w.comp.selection_read(&mut w.srv.core, read.0, read.1, read.2);
    let got: Vec<String> = w.comp.take_events().into_iter().filter_map(|e| match e { UiEvent::Selection { board: Board::Clipboard, text } => Some(text), _ => None }).collect();
    assert_eq!(got, ["copied in a client ✓"]);
    // client 2 (focused) pastes html straight from the source
    let evs = o.c.events();
    let offer = Other::selection(&evs, dev2);
    let mimes: Vec<&str> = evs.iter().filter(|e| e.id == offer && e.name == "offer").map(|e| e.args[0].as_str()).collect();
    assert_eq!(mimes, ["text/html", "UTF8_STRING", "text/plain;charset=utf-8"]);
    let (mut rd, wr) = pipe();
    o.c.req(offer, wl_data_offer::REQ_RECEIVE, vec![Value::Str(Some("text/html".into())), Value::Fd(wr)]);
    turn(&mut w);
    let evs = w.c.events();
    let send = evs.iter().find(|e| e.id == src && e.name == "send").expect("send to the source");
    assert_eq!(send.args[0].as_str(), "text/html");
    let Value::Fd(fd) = &send.args[1] else { panic!() };
    std::fs::File::from(fd.try_clone().unwrap()).write_all(b"<b>x</b>").unwrap();
    drop(evs);
    let mut html = String::new();
    rd.read_to_string(&mut html).unwrap();
    assert_eq!(html, "<b>x</b>");
    // client 1 quits: client 2 is offered the text the compositor kept, and pastes it
    drop(std::mem::replace(&mut w.c, Client::new(UnixStream::pair().unwrap().0)));
    turn(&mut w);
    let evs = o.c.events();
    let kept = Other::selection(&evs, dev2);
    assert_ne!(kept, offer);
    assert_eq!(o.paste(&mut w, kept, false, "text/plain;charset=utf-8"), "copied in a client ✓");
    assert!(w.comp.take_events().iter().all(|e| !matches!(e, UiEvent::Selection { .. })), "the kept text is not news to the host");
}

/// A source destroyed before its text was read: the text, when it arrives, is the selection.
/// A clear (set_selection null) and a new copy make an old read stale.
#[test]
fn selection_read_after_the_source_went() {
    let mut w = World::new();
    let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let r2 = reads.clone();
    w.comp.set_selection_reader(std::sync::Arc::new(move |b, g, t| r2.lock().unwrap().push((b, g, t))));
    let (reg, evs) = w.setup();
    let seat1 = w.bind(reg, &evs, "wl_seat", 8);
    let pmgr = w.bind(reg, &evs, "zwp_primary_selection_device_manager_v1", 1);
    let pdev = w.c.create(pmgr, zwp_primary_selection_device_manager_v1::REQ_GET_DEVICE, &zwp_primary_selection_device_v1::INTERFACE, vec![Value::Object(seat1)]);
    let src = w.c.create(pmgr, zwp_primary_selection_device_manager_v1::REQ_CREATE_SOURCE, &zwp_primary_selection_source_v1::INTERFACE, vec![]);
    w.c.req(src, zwp_primary_selection_source_v1::REQ_OFFER, vec![Value::Str(Some("text/plain".into()))]);
    w.c.req(pdev, zwp_primary_selection_device_v1::REQ_SET_SELECTION, vec![Value::Object(src), Value::Uint(1)]);
    turn(&mut w);
    let evs = w.c.events();
    let send = evs.iter().find(|e| e.id == src && e.name == "send").expect("the server reads the selection");
    let Value::Fd(fd) = &send.args[1] else { panic!() };
    let fd = fd.try_clone().unwrap();
    drop(evs);
    w.c.req(src, zwp_primary_selection_source_v1::REQ_DESTROY, vec![]);
    turn(&mut w);
    std::fs::File::from(fd).write_all(b"late").unwrap();
    let read = loop {
        if let Some(r) = reads.lock().unwrap().pop() { break r; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(read.0, Board::Primary);
    let generation = read.1;
    w.comp.selection_read(&mut w.srv.core, read.0, read.1, read.2);
    assert!(w.comp.take_events().iter().any(|e| matches!(e, UiEvent::Selection { board: Board::Primary, text } if text == "late")));
    let mut o = Other::join(&mut w);
    let (surf, _, opdev) = o.devices();
    turn(&mut w);
    w.comp.keyboard_focus(&mut w.srv.core, Some((2, surf)));
    turn(&mut w);
    let evs = o.c.events();
    let offer = Other::selection(&evs, opdev);
    assert_eq!(o.paste(&mut w, offer, true, "text/plain"), "late");
    // the same read again (stale now): nothing
    w.comp.selection_read(&mut w.srv.core, Board::Primary, generation, "again".into());
    assert!(w.comp.take_events().is_empty());
}

#[test]
fn viewport_fractional_scale_cursor_shape_decoration() {
    let mut w = World::new();
    let (reg, evs) = w.setup();
    let (win, shm, _) = open_window(&mut w, reg, &evs);
    let seat = w.bind(reg, &evs, "wl_seat", 8);
    let vpr = w.bind(reg, &evs, "wp_viewporter", 1);
    let frac = w.bind(reg, &evs, "wp_fractional_scale_manager_v1", 1);
    let shape = w.bind(reg, &evs, "wp_cursor_shape_manager_v1", 1);
    let deco = w.bind(reg, &evs, "zxdg_decoration_manager_v1", 1);
    let ptr = w.c.create(seat, wl_seat::REQ_GET_POINTER, &wl_pointer::INTERFACE, vec![]);
    let dev = w.c.create(shape, wp_cursor_shape_manager_v1::REQ_GET_POINTER, &wp_cursor_shape_device_v1::INTERFACE, vec![Value::Object(ptr)]);
    let td = w.c.create(deco, zxdg_decoration_manager_v1::REQ_GET_TOPLEVEL_DECORATION, &zxdg_toplevel_decoration_v1::INTERFACE, vec![Value::Object(win.top)]);
    let fs = w.c.create(frac, wp_fractional_scale_manager_v1::REQ_GET_FRACTIONAL_SCALE, &wp_fractional_scale_v1::INTERFACE, vec![Value::Object(win.surf)]);
    let vp = w.c.create(vpr, wp_viewporter::REQ_GET_VIEWPORT, &wp_viewport::INTERFACE, vec![Value::Object(win.surf)]);
    let evs = w.pump();
    let ser = evs.iter().find(|e| e.iface == "xdg_surface" && e.name == "configure").unwrap().args[0].as_uint();
    // decoration answers server side, fractional scale starts at 1.0
    assert_eq!(evs.iter().find(|e| e.id == td).unwrap().args[0].as_uint(), 2);
    assert_eq!(evs.iter().find(|e| e.id == fs && e.name == "preferred_scale").unwrap().args[0].as_uint(), 120);
    w.comp.set_scale(&mut w.srv.core, 1.3);
    let evs = w.pump();
    assert_eq!(evs.iter().find(|e| e.id == fs).unwrap().args[0].as_uint(), 156);
    // the configure and a 8x4 buffer shown at 4x2 logical through the viewport
    let (fd, _f, path) = memfile("vp", &vec![9u8; 128]);
    let pool = w.c.create(shm, wl_shm::REQ_CREATE_POOL, &wl_shm_pool::INTERFACE, vec![Value::Fd(fd), Value::Int(128)]);
    let buf = w.c.create(pool, wl_shm_pool::REQ_CREATE_BUFFER, &wl_buffer::INTERFACE, vec![Value::Int(0), Value::Int(8), Value::Int(4), Value::Int(32), Value::Uint(1)]);
    // ack the initial configure first (already sent by open_window's commit)
    w.c.req(win.xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(ser)]);
    w.c.req(vp, wp_viewport::REQ_SET_DESTINATION, vec![Value::Int(4), Value::Int(2)]);
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    w.pump();
    let ui = w.comp.take_events();
    let size = ui.iter().rev().find_map(|e| match e { UiEvent::Commit { size, .. } => Some(*size), _ => None }).unwrap();
    assert_eq!(size, (4, 2));
    // bad value is a protocol error
    w.c.req(vp, wp_viewport::REQ_SET_DESTINATION, vec![Value::Int(0), Value::Int(5)]);
    let evs = w.pump();
    assert_eq!(named(&evs, "error").len(), 1);
    let _ = (dev, ptr);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn dmabuf_buffer_validation_commit_and_release() {
    let ar24 = u32::from_le_bytes(*b"AR24");
    let mut w = World::new();
    w.comp.enable_dmabuf(&mut w.srv.core, DmabufConfig { main_device: 0xe280, formats: vec![(ar24, 0), (ar24, 0x0300_0000_0000_0001)], render_node: None }).unwrap();
    let (reg, evs) = w.setup();
    let (win, _shm, _) = open_window(&mut w, reg, &evs);
    let dm = w.bind(reg, &evs, "zwp_linux_dmabuf_v1", 4);
    let fb = w.c.create(dm, zwp_linux_dmabuf_v1::REQ_GET_DEFAULT_FEEDBACK, &zwp_linux_dmabuf_feedback_v1::INTERFACE, vec![]);
    let evs = w.pump();
    let ser = evs.iter().find(|e| e.iface == "xdg_surface" && e.name == "configure").unwrap().args[0].as_uint();
    let names: Vec<&str> = evs.iter().filter(|e| e.id == fb).map(|e| e.name).collect();
    assert_eq!(names, ["format_table", "main_device", "tranche_target_device", "tranche_flags", "tranche_formats", "tranche_done", "done"]);
    let table = evs.iter().find(|e| e.name == "format_table").unwrap();
    assert_eq!(table.args[1].as_uint(), 32);
    let Value::Fd(tfd) = &table.args[0] else { panic!() };
    let mut tbytes = Vec::new();
    use std::io::Seek;
    let mut tf = std::fs::File::from(tfd.try_clone().unwrap());
    tf.seek(std::io::SeekFrom::Start(0)).unwrap();
    tf.read_to_end(&mut tbytes).unwrap();
    assert_eq!(u32::from_ne_bytes(tbytes[0..4].try_into().unwrap()), ar24);
    assert_eq!(u64::from_ne_bytes(tbytes[24..32].try_into().unwrap()), 0x0300_0000_0000_0001);
    assert_eq!(u64::from_ne_bytes(evs.iter().find(|e| e.name == "main_device").unwrap().args[0].as_bytes().try_into().unwrap()), 0xe280);
    w.c.req(win.xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(ser)]);
    // a linear 4x4 AR24 buffer in a memfd-like file
    let (fd, _f, path) = memfile("dma", &vec![5u8; 4 * 4 * 4]);
    let params = w.c.create(dm, zwp_linux_dmabuf_v1::REQ_CREATE_PARAMS, &zwp_linux_buffer_params_v1::INTERFACE, vec![]);
    w.c.req(params, zwp_linux_buffer_params_v1::REQ_ADD, vec![Value::Fd(fd), Value::Uint(0), Value::Uint(0), Value::Uint(16), Value::Uint(0), Value::Uint(0)]);
    let buf = w.c.create(params, zwp_linux_buffer_params_v1::REQ_CREATE_IMMED, &wl_buffer::INTERFACE, vec![Value::Int(4), Value::Int(4), Value::Uint(ar24), Value::Uint(0)]);
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    let evs = w.pump();
    assert!(named(&evs, "release").is_empty()); // not released until the host is done
    let ui = w.comp.take_events();
    let frame = ui.iter().find_map(|e| match e { UiEvent::Commit { buffer: BufferUpdate::Dmabuf(f), size, .. } => Some((f, *size)), _ => None }).expect("dmabuf commit");
    assert_eq!((frame.0.buffer, frame.1), (buf, (4, 4)));
    assert_eq!((frame.0.data.fourcc, frame.0.data.planes.len(), frame.0.data.planes[0].stride), (ar24, 1, 16));
    w.comp.buffer_done(&mut w.srv.core, 1, buf, None);
    let evs = w.pump();
    assert_eq!(named(&evs, "release").len(), 1);
    // a format that was not offered is a protocol error
    let params = w.c.create(dm, zwp_linux_dmabuf_v1::REQ_CREATE_PARAMS, &zwp_linux_buffer_params_v1::INTERFACE, vec![]);
    let (fd2, _f2, path2) = memfile("dma2", &vec![0u8; 64]);
    w.c.req(params, zwp_linux_buffer_params_v1::REQ_ADD, vec![Value::Fd(fd2), Value::Uint(0), Value::Uint(0), Value::Uint(16), Value::Uint(0), Value::Uint(0)]);
    w.c.create(params, zwp_linux_buffer_params_v1::REQ_CREATE_IMMED, &wl_buffer::INTERFACE, vec![Value::Int(4), Value::Int(4), Value::Uint(u32::from_le_bytes(*b"NV12")), Value::Uint(0)]);
    let evs = w.pump();
    assert_eq!(named(&evs, "error").len(), 1);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(path2).unwrap();
}

#[test]
fn explicit_sync_points_gate_the_commit_and_release_through_the_timeline() {
    use wayland_server::sys::*;
    let Some(node) = (128..140).map(|n| std::path::PathBuf::from(format!("/dev/dri/renderD{n}"))).find(|p| p.exists()) else { return };
    let drm = match std::fs::OpenOptions::new().read(true).write(true).open(&node) { Ok(f) => f, Err(_) => return };
    let dfd = drm.as_raw_fd();
    let mut create = drm_syncobj_create { handle: 0, flags: 0 };
    if unsafe { ioctl(dfd, DRM_IOCTL_SYNCOBJ_CREATE, &mut create as *mut _) } != 0 { return; }
    eprintln!("syncobj test running on {}", node.display());
    let mut export = drm_syncobj_handle { handle: create.handle, flags: 0, fd: -1, pad: 0 };
    assert_eq!(unsafe { ioctl(dfd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &mut export as *mut _) }, 0);
    let timeline_fd = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(export.fd) };
    let signal = |point: u64| {
        let (h, p) = ([create.handle], [point]);
        let mut a = drm_syncobj_timeline_array { handles: h.as_ptr() as u64, points: p.as_ptr() as u64, count_handles: 1, flags: 0 };
        assert_eq!(unsafe { ioctl(dfd, DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL, &mut a as *mut _) }, 0);
    };
    let ar24 = u32::from_le_bytes(*b"AR24");
    let mut w = World::new();
    w.comp.enable_dmabuf(&mut w.srv.core, DmabufConfig { main_device: 1, formats: vec![(ar24, 0)], render_node: Some(node) }).unwrap();
    let (reg, evs) = w.setup();
    let (win, _shm, _) = open_window(&mut w, reg, &evs);
    let dm = w.bind(reg, &evs, "zwp_linux_dmabuf_v1", 4);
    let mgr = w.bind(reg, &evs, "wp_linux_drm_syncobj_manager_v1", 1);
    let tl = w.c.create(mgr, wp_linux_drm_syncobj_manager_v1::REQ_IMPORT_TIMELINE, &wp_linux_drm_syncobj_timeline_v1::INTERFACE, vec![Value::Fd(timeline_fd)]);
    let ss = w.c.create(mgr, wp_linux_drm_syncobj_manager_v1::REQ_GET_SURFACE, &wp_linux_drm_syncobj_surface_v1::INTERFACE, vec![Value::Object(win.surf)]);
    let evs = w.pump();
    let ser = evs.iter().find(|e| e.iface == "xdg_surface" && e.name == "configure").unwrap().args[0].as_uint();
    w.c.req(win.xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(ser)]);
    let (fd, _f, path) = memfile("sync", &vec![5u8; 64]);
    let params = w.c.create(dm, zwp_linux_dmabuf_v1::REQ_CREATE_PARAMS, &zwp_linux_buffer_params_v1::INTERFACE, vec![]);
    w.c.req(params, zwp_linux_buffer_params_v1::REQ_ADD, vec![Value::Fd(fd), Value::Uint(0), Value::Uint(0), Value::Uint(16), Value::Uint(0), Value::Uint(0)]);
    let buf = w.c.create(params, zwp_linux_buffer_params_v1::REQ_CREATE_IMMED, &wl_buffer::INTERFACE, vec![Value::Int(4), Value::Int(4), Value::Uint(ar24), Value::Uint(0)]);
    // acquire point 1 is signalled before the commit; release point 2 is for the server to signal
    signal(1);
    w.c.req(ss, wp_linux_drm_syncobj_surface_v1::REQ_SET_ACQUIRE_POINT, vec![Value::Object(tl), Value::Uint(0), Value::Uint(1)]);
    w.c.req(ss, wp_linux_drm_syncobj_surface_v1::REQ_SET_RELEASE_POINT, vec![Value::Object(tl), Value::Uint(0), Value::Uint(2)]);
    w.c.req(win.surf, wl_surface::REQ_ATTACH, vec![Value::Object(buf), Value::Int(0), Value::Int(0)]);
    w.c.req(win.surf, wl_surface::REQ_COMMIT, vec![]);
    w.pump();
    let mut ui = w.comp.take_events();
    if !ui.iter().any(|e| matches!(e, UiEvent::Commit { buffer: BufferUpdate::Dmabuf(_), .. })) {
        std::thread::sleep(std::time::Duration::from_millis(20));
        ui = w.comp.take_events();
    }
    assert!(ui.iter().any(|e| matches!(e, UiEvent::Commit { buffer: BufferUpdate::Dmabuf(_), .. })), "commit delivered once the acquire point signalled");
    let waits = |point: u64| {
        let (h, p) = ([create.handle], [point]);
        let mut a = [0u64; 6];
        a[0] = h.as_ptr() as u64; a[1] = p.as_ptr() as u64; a[2] = 0; // timeout 0
        a[3] = 1; // count_handles (u32) | flags 0 in the high half
        unsafe { ioctl(dfd, 0xC028_64CA, a.as_mut_ptr()) == 0 }
    };
    assert!(!waits(2), "release point is not signalled before the host is done");
    w.comp.buffer_done(&mut w.srv.core, 1, buf, None);
    let evs = w.pump();
    assert!(named(&evs, "release").is_empty(), "explicit sync does not use wl_buffer.release");
    assert!(waits(2), "release point signalled");
    std::fs::remove_file(path).unwrap();
}
