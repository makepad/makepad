// The compositor is Linux-only (the crate's modules are cfg(target_os = "linux")).
#![cfg(target_os = "linux")]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wayland_server::compositor::*;
use wayland_server::host::*;
use wayland_server::proto::*;
use wayland_server::wire::*;

fn send(s: &mut UnixStream, id: u32, op: u16, iface: &wayland_server::schema::Interface, args: Vec<Value>) {
    let mut fds = Vec::new();
    s.write_all(&encode(id, op, iface.requests[op as usize].args, args, &mut fds).unwrap()).unwrap();
}

#[test]
fn host_thread_roundtrip_and_shutdown() {
    let dir = std::env::temp_dir().join(format!("wsrv-host-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let woke = Arc::new(AtomicUsize::new(0));
    let w2 = woke.clone();
    let host = Host::spawn(&dir, "wayland-hosttest", OutputInfo::default(), &xkb_keymap::US, None, move || { w2.fetch_add(1, Ordering::SeqCst); }).unwrap();
    let path = dir.join(&host.socket);
    assert!(path.exists());
    let mut s = UnixStream::connect(&path).unwrap();
    // registry -> bind compositor (global 1) -> create_surface -> destroy it
    send(&mut s, 1, wl_display::REQ_GET_REGISTRY, &wl_display::INTERFACE, vec![Value::NewId(2)]);
    send(&mut s, 2, wl_registry::REQ_BIND, &wl_registry::INTERFACE, vec![Value::Uint(1), Value::NewIdDyn { interface: "wl_compositor".into(), version: 6, id: 3 }]);
    send(&mut s, 3, wl_compositor::REQ_CREATE_SURFACE, &wl_compositor::INTERFACE, vec![Value::NewId(4)]);
    send(&mut s, 4, wl_surface::REQ_DESTROY, &wl_surface::INTERFACE, vec![]);
    let mut got_gone = false;
    for _ in 0..50 {
        if let Ok(e) = host.events.recv_timeout(Duration::from_millis(100)) {
            if matches!(e, UiEvent::SurfaceGone { surface: (_, 4) }) { got_gone = true; break; }
        }
    }
    assert!(got_gone);
    assert!(woke.load(Ordering::SeqCst) > 0);
    // a command reaches the server thread without error even for unknown surfaces
    host.send(Command::KeyboardFocus(None));
    host.send(Command::FrameDone { surface: (1, 99), time_ms: 1 });
    drop(s);
    drop(host); // joins the thread
    assert!(!path.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
