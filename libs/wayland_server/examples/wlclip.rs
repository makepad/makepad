//! A minimal Wayland clipboard client for testing a compositor's selections (no libwayland):
//!
//!     wlclip copy <text> [--primary]   sets the selection and serves it until it is replaced
//!     wlclip paste [--primary] [--watch]
//!         opens a small window; prints `selection: <text>` for the selection it is offered
//!         (once it has the keyboard), and exits after the first unless --watch
//!
//! Uses $WAYLAND_DISPLAY in $XDG_RUNTIME_DIR. Prints `ready` once the selection is set / the
//! window is mapped. `--log=<file>` also appends every printed line to the file (for a client
//! started by a compositor with its output closed).
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wayland_server::proto::*;
use wayland_server::schema::{ArgType, Interface};
use wayland_server::sys::*;
use wayland_server::wire::*;

struct Ev { id: u32, name: &'static str, args: Vec<Value> }

struct Conn { s: UnixStream, buf: Vec<u8>, fds: VecDeque<OwnedFd>, ifaces: HashMap<u32, &'static Interface>, next: u32 }

impl Conn {
    fn connect() -> Conn {
        let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let name = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let s = UnixStream::connect(std::path::Path::new(&dir).join(name)).expect("connect to the compositor");
        let mut ifaces = HashMap::new();
        ifaces.insert(1, &wl_display::INTERFACE);
        Conn { s, buf: Vec::new(), fds: VecDeque::new(), ifaces, next: 2 }
    }
    fn req(&mut self, id: u32, op: u16, args: Vec<Value>) {
        let iface = self.ifaces[&id];
        let mut fds = Vec::new();
        let bytes = encode(id, op, iface.requests[op as usize].args, args, &mut fds).unwrap();
        let raw: Vec<i32> = fds.iter().map(|f| f.as_raw_fd()).collect();
        let mut ctl = [0u64; 16];
        let base = ctl.as_mut_ptr() as *mut u8;
        if !raw.is_empty() {
            unsafe {
                std::ptr::write(base as *mut cmsghdr, cmsghdr { len: cmsg_len(raw.len() * 4), level: SOL_SOCKET, kind: SCM_RIGHTS });
                std::ptr::copy_nonoverlapping(raw.as_ptr() as *const u8, base.add(cmsg_len(0)), raw.len() * 4);
            }
        }
        let mut iov = iovec { base: bytes.as_ptr() as *mut _, len: bytes.len() };
        let msg = msghdr { name: std::ptr::null_mut(), namelen: 0, iov: &mut iov, iovlen: 1,
            control: if raw.is_empty() { std::ptr::null_mut() } else { ctl.as_mut_ptr() as *mut _ },
            controllen: if raw.is_empty() { 0 } else { cmsg_space(raw.len() * 4) }, flags: 0 };
        assert!(unsafe { sendmsg(self.s.as_raw_fd(), &msg, MSG_NOSIGNAL) } > 0, "send to the compositor");
    }
    fn create(&mut self, parent: u32, op: u16, i: &'static Interface, rest: Vec<Value>) -> u32 {
        let id = self.next;
        self.next += 1;
        self.ifaces.insert(id, i);
        let mut a = vec![Value::NewId(id)];
        a.extend(rest);
        self.req(parent, op, a);
        id
    }
    fn bind(&mut self, reg: u32, globals: &[(u32, String, u32)], name: &str, version: u32) -> Option<u32> {
        let g = globals.iter().find(|g| g.1 == name)?;
        let id = self.next;
        self.next += 1;
        let i = wayland_server::schema::find(name).unwrap();
        self.ifaces.insert(id, i);
        self.req(reg, wl_registry::REQ_BIND, vec![Value::Uint(g.0), Value::NewIdDyn { interface: name.into(), version: version.min(g.2), id }]);
        Some(id)
    }
    /// Blocks until the server sent something; returns the decoded events.
    fn events(&mut self) -> Vec<Ev> {
        let mut pfd = pollfd { fd: self.s.as_raw_fd(), events: POLLIN, revents: 0 };
        unsafe { poll(&mut pfd, 1, -1) };
        let mut tmp = [0u8; 16384];
        let mut ctl = [0u64; 32];
        let mut iov = iovec { base: tmp.as_mut_ptr() as *mut _, len: tmp.len() };
        let mut msg = msghdr { name: std::ptr::null_mut(), namelen: 0, iov: &mut iov, iovlen: 1, control: ctl.as_mut_ptr() as *mut _, controllen: 256, flags: 0 };
        let n = unsafe { recvmsg(self.s.as_raw_fd(), &mut msg, MSG_CMSG_CLOEXEC) };
        if n <= 0 { eprintln!("wlclip: the compositor hung up"); std::process::exit(2); }
        if msg.controllen >= std::mem::size_of::<cmsghdr>() {
            let h = unsafe { std::ptr::read_unaligned(ctl.as_ptr() as *const cmsghdr) };
            if h.level == SOL_SOCKET && h.kind == SCM_RIGHTS {
                for i in 0..(h.len - cmsg_len(0)) / 4 {
                    let fd = unsafe { std::ptr::read_unaligned((ctl.as_ptr() as *const u8).add(cmsg_len(0) + i * 4) as *const i32) };
                    self.fds.push_back(unsafe { OwnedFd::from_raw_fd(fd) });
                }
            }
        }
        self.buf.extend_from_slice(&tmp[..n as usize]);
        let mut out = Vec::new();
        while let Some((id, op, size)) = peek_header(&self.buf) {
            if self.buf.len() < size { break; }
            let iface = self.ifaces[&id];
            let m = &iface.events[op as usize];
            let args = decode_args(&self.buf[8..size], m.args, &mut self.fds).unwrap();
            for (t, v) in m.args.iter().zip(&args) {
                if let (ArgType::NewId(i), Value::NewId(n)) = (t, v) { self.ifaces.insert(*n, wayland_server::schema::find(i).unwrap()); }
            }
            if iface.name == "wl_display" && m.name == "error" { eprintln!("wlclip: protocol error {args:?}"); std::process::exit(2); }
            out.push(Ev { id, name: m.name, args });
            self.buf.drain(..size);
        }
        out
    }
    /// The globals, after a roundtrip.
    fn globals(&mut self) -> (u32, Vec<(u32, String, u32)>) {
        let reg = self.create(1, wl_display::REQ_GET_REGISTRY, &wl_registry::INTERFACE, vec![]);
        let cb = self.create(1, wl_display::REQ_SYNC, &wl_callback::INTERFACE, vec![]);
        let mut globals = Vec::new();
        loop {
            for e in self.events() {
                if e.id == reg && e.name == "global" { globals.push((e.args[0].as_uint(), e.args[1].as_str().to_string(), e.args[2].as_uint())); }
                if e.id == cb { return (reg, globals); }
            }
        }
    }
}

static LOG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
fn say(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
    if let Some(path) = LOG.get() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) { let _ = writeln!(f, "{line}"); }
    }
}

const MIMES: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let primary = args.iter().any(|a| a == "--primary");
    let watch = args.iter().any(|a| a == "--watch");
    if let Some(path) = args.iter().find_map(|a| a.strip_prefix("--log=")) { let _ = LOG.set(path.to_string()); }
    let plain: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let mut c = Conn::connect();
    let (reg, globals) = c.globals();
    let seat = c.bind(reg, &globals, "wl_seat", 5).expect("wl_seat");
    let (mgr_name, dev_iface, src_iface) = if primary {
        ("zwp_primary_selection_device_manager_v1", &zwp_primary_selection_device_v1::INTERFACE, &zwp_primary_selection_source_v1::INTERFACE)
    } else {
        ("wl_data_device_manager", &wl_data_device::INTERFACE, &wl_data_source::INTERFACE)
    };
    let mgr = c.bind(reg, &globals, mgr_name, 3).unwrap_or_else(|| { eprintln!("wlclip: no {mgr_name}"); std::process::exit(2) });
    let dev = c.create(mgr, if primary { zwp_primary_selection_device_manager_v1::REQ_GET_DEVICE } else { wl_data_device_manager::REQ_GET_DATA_DEVICE }, dev_iface, vec![Value::Object(seat)]);
    match plain.first().map(|s| s.as_str()) {
        Some("copy") => {
            let text = plain.get(1).map(|s| s.as_str()).unwrap_or("").to_string();
            let src = c.create(mgr, if primary { zwp_primary_selection_device_manager_v1::REQ_CREATE_SOURCE } else { wl_data_device_manager::REQ_CREATE_DATA_SOURCE }, src_iface, vec![]);
            for m in MIMES { c.req(src, 0, vec![Value::Str(Some(m.into()))]); }
            c.req(dev, if primary { zwp_primary_selection_device_v1::REQ_SET_SELECTION } else { wl_data_device::REQ_SET_SELECTION }, vec![Value::Object(src), Value::Uint(0)]);
            say("ready");
            loop {
                for e in c.events() {
                    if e.id != src { continue; }
                    match e.name {
                        "send" => {
                            let mut args = e.args;
                            if let Some(Value::Fd(fd)) = args.pop() {
                                let _ = std::fs::File::from(fd).write_all(text.as_bytes());
                                say(&format!("served {}", args[0].as_str()));
                            }
                        }
                        "cancelled" => { say("cancelled"); return; }
                        _ => {}
                    }
                }
            }
        }
        Some("paste") => {
            let comp = c.bind(reg, &globals, "wl_compositor", 4).expect("wl_compositor");
            let shm = c.bind(reg, &globals, "wl_shm", 1).expect("wl_shm");
            let wm = c.bind(reg, &globals, "xdg_wm_base", 2).expect("xdg_wm_base");
            let _kb = c.create(seat, wl_seat::REQ_GET_KEYBOARD, &wl_keyboard::INTERFACE, vec![]);
            let surf = c.create(comp, wl_compositor::REQ_CREATE_SURFACE, &wl_surface::INTERFACE, vec![]);
            let xdg = c.create(wm, xdg_wm_base::REQ_GET_XDG_SURFACE, &xdg_surface::INTERFACE, vec![Value::Object(surf)]);
            let top = c.create(xdg, xdg_surface::REQ_GET_TOPLEVEL, &xdg_toplevel::INTERFACE, vec![]);
            c.req(top, xdg_toplevel::REQ_SET_TITLE, vec![Value::Str(Some("wlclip".into()))]);
            c.req(top, xdg_toplevel::REQ_SET_APP_ID, vec![Value::Str(Some("wlclip".into()))]);
            c.req(surf, wl_surface::REQ_COMMIT, vec![]);
            let (w, h) = (160i32, 120i32);
            let fd = unsafe { memfd_create(c"wlclip".as_ptr(), MFD_CLOEXEC) };
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            file.write_all(&vec![0xffu8; (w * h * 4) as usize]).unwrap();
            let pool = c.create(shm, wl_shm::REQ_CREATE_POOL, &wl_shm_pool::INTERFACE, vec![Value::Fd(OwnedFd::from(file.try_clone().unwrap())), Value::Int(w * h * 4)]);
            let buffer = c.create(pool, wl_shm_pool::REQ_CREATE_BUFFER, &wl_buffer::INTERFACE, vec![Value::Int(0), Value::Int(w), Value::Int(h), Value::Int(w * 4), Value::Uint(1)]);
            let mut offers: HashMap<u32, Vec<String>> = HashMap::new();
            loop {
                for e in c.events() {
                    if e.id == wm && e.name == "ping" { c.req(wm, xdg_wm_base::REQ_PONG, vec![Value::Uint(e.args[0].as_uint())]); }
                    if e.id == xdg && e.name == "configure" {
                        c.req(xdg, xdg_surface::REQ_ACK_CONFIGURE, vec![Value::Uint(e.args[0].as_uint())]);
                        c.req(surf, wl_surface::REQ_ATTACH, vec![Value::Object(buffer), Value::Int(0), Value::Int(0)]);
                        c.req(surf, wl_surface::REQ_DAMAGE, vec![Value::Int(0), Value::Int(0), Value::Int(w), Value::Int(h)]);
                        c.req(surf, wl_surface::REQ_COMMIT, vec![]);
                        say("ready");
                    }
                    if e.name == "offer" && offers.contains_key(&e.id) { offers.get_mut(&e.id).unwrap().push(e.args[0].as_str().to_string()); }
                    if e.id == dev && e.name == "data_offer" { if let Value::NewId(o) = e.args[0] { offers.insert(o, Vec::new()); } }
                    if e.id == dev && e.name == "selection" {
                        let offer = e.args[0].as_uint();
                        if offer == 0 { say("selection: (none)"); continue; }
                        let mimes = offers.get(&offer).cloned().unwrap_or_default();
                        let Some(mime) = MIMES.iter().find(|m| mimes.iter().any(|x| x == *m)) else { say(&format!("selection: (no text: {mimes:?})")); continue };
                        let mut fds = [0i32; 2];
                        assert_eq!(unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) }, 0);
                        let (rd, wr) = unsafe { (std::fs::File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
                        c.req(offer, if primary { zwp_primary_selection_offer_v1::REQ_RECEIVE } else { wl_data_offer::REQ_RECEIVE }, vec![Value::Str(Some(mime.to_string())), Value::Fd(wr)]);
                        let mut text = String::new();
                        let _ = { rd }.read_to_string(&mut text);
                        say(&format!("selection: {text}"));
                        if !watch { return; }
                    }
                }
            }
        }
        _ => { eprintln!("usage: wlclip copy <text> [--primary] | wlclip paste [--primary] [--watch]"); std::process::exit(1); }
    }
}
