// The compositor is Linux-only (the crate's modules are cfg(target_os = "linux")).
#![cfg(target_os = "linux")]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wayland_server::core::*;
use wayland_server::proto::*;
use wayland_server::schema::Interface;
use wayland_server::wire::*;
use wayland_server::server::Server;

#[derive(Default)]
struct H { requests: Vec<(u32, String, Vec<String>)>, destroyed: Vec<u32>, binds: Vec<(String, u32, u32)>, fds: Vec<OwnedFd> }
impl Handler for H {
    fn bind(&mut self, _: &mut Core, _: ClientId, _: u32, i: &'static Interface, v: u32, id: u32) -> Result<(), Fatal> {
        self.binds.push((i.name.into(), v, id));
        Ok(())
    }
    fn request(&mut self, core: &mut Core, c: ClientId, o: ObjRef, op: u16, args: Vec<Value>) -> Result<(), Fatal> {
        let name = o.iface.requests[op as usize].name.to_string();
        let mut shown = Vec::new();
        for a in args {
            match a {
                Value::Fd(f) => { shown.push("fd".into()); self.fds.push(f); }
                other => shown.push(format!("{other:?}")),
            }
        }
        self.requests.push((o.id, name.clone(), shown));
        if o.iface.name == "wl_shm" && name == "create_pool" {
            return Err(Fatal::new(o.id, 0, "test error"));
        }
        let _ = (core, c);
        Ok(())
    }
    fn destroyed(&mut self, _: &mut Core, _: ClientId, o: ObjRef) { self.destroyed.push(o.id); }
}

struct Client { s: UnixStream, buf: Vec<u8>, fds: VecDeque<OwnedFd> }
impl Client {
    fn send(&mut self, id: u32, opcode: u16, iface: &Interface, args: Vec<Value>) {
        let mut fds = Vec::new();
        let sig = iface.requests[opcode as usize].args;
        let bytes = encode(id, opcode, sig, args, &mut fds).unwrap();
        if fds.is_empty() { self.s.write_all(&bytes).unwrap(); } else { send_with_fds(&self.s, &bytes, &fds); }
    }
    /// Reads events until `n` messages are available or the peer closes.
    fn events(&mut self, n: usize, tracker: &mut Vec<(u32, u16, Vec<Value>)>, ifaces: &mut std::collections::HashMap<u32, &'static Interface>) {
        self.s.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        while tracker.len() < n {
            // parse what we have
            while let Some((id, op, size)) = peek_header(&self.buf) {
                if self.buf.len() < size { break; }
                let iface = ifaces[&id];
                let args = decode_args(&self.buf[8..size], iface.events[op as usize].args, &mut self.fds).unwrap();
                for (t, v) in iface.events[op as usize].args.iter().zip(&args) {
                    if let (wayland_server::schema::ArgType::NewId(i), Value::NewId(nid)) = (t, v) { ifaces.insert(*nid, wayland_server::schema::find(i).unwrap()); }
                }
                tracker.push((id, op, args));
                self.buf.drain(..size);
            }
            if tracker.len() >= n { break; }
            let mut tmp = [0u8; 4096];
            match self.s.read(&mut tmp) { Ok(0) => break, Ok(k) => self.buf.extend_from_slice(&tmp[..k]), Err(_) => break }
        }
    }
}
fn send_with_fds(s: &UnixStream, bytes: &[u8], fds: &[OwnedFd]) {
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
    assert!(unsafe { sendmsg(s.as_raw_fd(), &msg, 0) } > 0);
}

fn setup() -> (Server, H, Client) {
    let mut srv = Server::new().unwrap();
    let mut h = H::default();
    let (a, b) = UnixStream::pair().unwrap();
    srv.add_client(&mut h, a).unwrap();
    srv.core.add_global(&wl_shm::INTERFACE, 1);
    srv.core.add_global(&wl_compositor::INTERFACE, 6);
    (srv, h, Client { s: b, buf: Vec::new(), fds: VecDeque::new() })
}
fn pump(srv: &mut Server, h: &mut H) { for _ in 0..3 { srv.run_once(h, 50).unwrap(); } }

#[test]
fn sync_registry_bind_and_requests() {
    let (mut srv, mut h, mut c) = setup();
    let mut ifaces = std::collections::HashMap::new();
    ifaces.insert(1, &wl_display::INTERFACE);
    c.send(1, wl_display::REQ_GET_REGISTRY, &wl_display::INTERFACE, vec![Value::NewId(2)]);
    c.send(1, wl_display::REQ_SYNC, &wl_display::INTERFACE, vec![Value::NewId(3)]);
    ifaces.insert(2, &wl_registry::INTERFACE);
    ifaces.insert(3, &wl_callback::INTERFACE);
    pump(&mut srv, &mut h);
    let mut ev = Vec::new();
    c.events(4, &mut ev, &mut ifaces); // 2 globals, done, delete_id
    assert_eq!(ev[0].0, 2);
    assert_eq!(ev[0].2[1].as_str(), "wl_shm");
    assert_eq!(ev[1].2[1].as_str(), "wl_compositor");
    assert_eq!(ev[2].0, 3); // callback done
    assert_eq!((ev[3].0, ev[3].1), (1, wl_display::EVT_DELETE_ID)); // id 3 released
    // bind compositor v6 (asks for 7 -> error; first v6)
    c.send(2, wl_registry::REQ_BIND, &wl_registry::INTERFACE, vec![Value::Uint(2), Value::NewIdDyn { interface: "wl_compositor".into(), version: 6, id: 4 }]);
    pump(&mut srv, &mut h);
    assert_eq!(h.binds, vec![("wl_compositor".to_string(), 6, 4)]);
    // create_surface (new_id typed) then destroy it
    c.send(4, wl_compositor::REQ_CREATE_SURFACE, &wl_compositor::INTERFACE, vec![Value::NewId(5)]);
    c.send(5, wl_surface::REQ_DESTROY, &wl_surface::INTERFACE, vec![]);
    pump(&mut srv, &mut h);
    assert_eq!(h.destroyed, vec![3, 5]);
    assert_eq!(h.requests.len(), 2);
    // id 5 is gone: using it again is a protocol error
    c.send(4, wl_compositor::REQ_CREATE_SURFACE, &wl_compositor::INTERFACE, vec![Value::NewId(5)]);
    pump(&mut srv, &mut h);
}

#[test]
fn bad_object_gets_error_and_disconnect() {
    let (mut srv, mut h, mut c) = setup();
    let mut ifaces = std::collections::HashMap::new();
    ifaces.insert(1, &wl_display::INTERFACE);
    c.s.write_all(&[99, 0, 0, 0, 0, 0, 8, 0]).unwrap();
    pump(&mut srv, &mut h);
    let mut ev = Vec::new();
    c.events(1, &mut ev, &mut ifaces);
    assert_eq!((ev[0].0, ev[0].1), (1, wl_display::EVT_ERROR));
    assert_eq!(ev[0].2[1].as_uint(), ERR_INVALID_OBJECT);
    assert!(srv.core.client_ids().is_empty());
}

#[test]
fn versions_oversize_and_truncated_are_rejected() {
    // bind above the offered version
    let (mut srv, mut h, mut c) = setup();
    let mut ifaces = std::collections::HashMap::new();
    ifaces.insert(1, &wl_display::INTERFACE);
    c.send(1, wl_display::REQ_GET_REGISTRY, &wl_display::INTERFACE, vec![Value::NewId(2)]);
    c.send(2, wl_registry::REQ_BIND, &wl_registry::INTERFACE, vec![Value::Uint(1), Value::NewIdDyn { interface: "wl_shm".into(), version: 9, id: 3 }]);
    pump(&mut srv, &mut h);
    assert!(h.binds.is_empty());
    assert!(srv.core.client_ids().is_empty());
    // message size smaller than the header
    let (mut srv, mut h, mut c) = setup();
    c.s.write_all(&[1, 0, 0, 0, 0, 0, 4, 0]).unwrap();
    pump(&mut srv, &mut h);
    assert!(srv.core.client_ids().is_empty());
    // wrong argument layout (trailing bytes)
    let (mut srv, mut h, mut c) = setup();
    c.s.write_all(&[1, 0, 0, 0, 0, 0, 12, 0, 2, 0, 0, 0]).unwrap(); // sync with 4 extra? size 12, opcode 0: sync takes one word -> ok
    c.s.write_all(&[1, 0, 0, 0, 0, 0, 16, 0, 3, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    pump(&mut srv, &mut h);
    assert!(srv.core.client_ids().is_empty());
}

#[test]
fn partial_reads_and_fd_passing() {
    let (mut srv, mut h, mut c) = setup();
    let mut ifaces = std::collections::HashMap::new();
    ifaces.insert(1, &wl_display::INTERFACE);
    c.send(1, wl_display::REQ_GET_REGISTRY, &wl_display::INTERFACE, vec![Value::NewId(2)]);
    c.send(2, wl_registry::REQ_BIND, &wl_registry::INTERFACE, vec![Value::Uint(1), Value::NewIdDyn { interface: "wl_shm".into(), version: 1, id: 3 }]);
    pump(&mut srv, &mut h);
    // create_pool(new_id, fd, size) with a real fd, sent in two pieces
    let f = std::fs::File::open("/dev/null").unwrap();
    let fd: OwnedFd = f.into();
    let mut fds = Vec::new();
    let bytes = encode(3, wl_shm::REQ_CREATE_POOL, wl_shm::INTERFACE.requests[0].args, vec![Value::NewId(4), Value::Fd(fd), Value::Int(4096)], &mut fds).unwrap();
    send_with_fds(&c.s, &bytes[..5], &fds);
    pump(&mut srv, &mut h);
    assert!(h.requests.is_empty());
    c.s.write_all(&bytes[5..]).unwrap();
    pump(&mut srv, &mut h);
    assert_eq!(h.requests.len(), 1);
    assert_eq!(h.fds.len(), 1);
    // the handler returned a Fatal: error event, then disconnect, destroyed callbacks fired
    assert!(srv.core.client_ids().is_empty());
    assert!(h.destroyed.contains(&4));
}

#[test]
fn server_events_carry_fds_and_listen_lock() {
    let dir = std::env::temp_dir().join(format!("wsrv-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut srv = Server::new().unwrap();
    let name = srv.listen_auto(&dir, "wayland-test").unwrap();
    let mut other = Server::new().unwrap();
    assert!(other.listen(&dir, &name).is_err()); // lock held
    let mut h = H::default();
    srv.core.add_global(&wl_shm::INTERFACE, 1);
    let s = UnixStream::connect(dir.join(&name)).unwrap();
    pump(&mut srv, &mut h);
    assert_eq!(srv.core.client_ids().len(), 1);
    let c = srv.core.client_ids()[0];
    assert!(srv.core.peer(c).unwrap().0 as u32 == std::process::id());
    // server-created object id range + fd-carrying event: wl_keyboard.keymap
    let kb = srv.core.new_object(c, &wl_keyboard::INTERFACE, 8).unwrap();
    assert!(kb >= 0xff00_0000);
    let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
    srv.core.send(c, kb, wl_keyboard::EVT_KEYMAP, vec![Value::Uint(1), Value::Fd(fd), Value::Uint(10)]).unwrap();
    srv.settle(&mut h);
    // receive with the crate's own transport semantics: use recvmsg through Transport
    let mut t = wayland_server::transport::Transport::new(s.try_clone().unwrap()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    t.fill().unwrap();
    assert_eq!(t.in_fds.len(), 1);
    let (id, op, size) = peek_header(&t.in_buf).unwrap();
    assert_eq!((id, op as u16, size), (kb, wl_keyboard::EVT_KEYMAP, 16));
    drop(srv);
    assert!(!dir.join(&name).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn many_fds_split_across_sendmsg() {
    // 40 fd-carrying events must arrive complete and in order despite the 28-fd cmsg limit.
    let (mut srv, mut h, c) = setup();
    let id = srv.core.client_ids()[0];
    let kb = srv.core.new_object(id, &wl_keyboard::INTERFACE, 8).unwrap();
    for i in 0..40u32 {
        let fd: OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        srv.core.send(id, kb, wl_keyboard::EVT_KEYMAP, vec![Value::Uint(i), Value::Fd(fd), Value::Uint(1)]).unwrap();
    }
    srv.settle(&mut h);
    let mut t = wayland_server::transport::Transport::new(c.s).unwrap();
    let mut got = 0;
    for _ in 0..50 {
        t.fill().unwrap();
        got = t.in_buf.len() / 16;
        if got == 40 { break; }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(got, 40);
    assert_eq!(t.in_fds.len(), 40);
    for i in 0..40 {
        let v = u32::from_ne_bytes(t.in_buf[i * 16 + 8..i * 16 + 12].try_into().unwrap());
        assert_eq!(v, i as u32);
    }
}
