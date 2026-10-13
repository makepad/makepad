//! Serves the compositor on a socket and prints what clients do; for trying real clients without a window manager.
//! Usage: serve <runtime dir> [seconds]. Configure requests are answered with a 800x600 window.
use std::time::{Duration, Instant};
use wayland_server::compositor::*;
use wayland_server::host::{Command, Host};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let host = Host::spawn(&dir, "wayland-serve", OutputInfo::default(), &xkb_keymap::US, None, || {}).unwrap();
    println!("socket {}", host.socket);
    let end = Instant::now() + Duration::from_secs(secs);
    let mut mapped = Vec::new();
    while Instant::now() < end {
        let Ok(ev) = host.events.recv_timeout(Duration::from_millis(100)) else { continue };
        match &ev {
            UiEvent::Commit { surface, buffer: BufferUpdate::Attach(f), .. } => {
                let lit = f.data.chunks_exact(4).filter(|p| p[0] | p[1] | p[2] != 0).count();
                println!("commit {surface:?} {}x{} rows {}..{} fmt {} lit {lit}", f.width, f.height, f.y0, f.y1, f.format);
            }
            UiEvent::ToplevelNew { surface } => {
                println!("toplevel {surface:?}");
                host.send(Command::ConfigureToplevel { surface: *surface, width: 800, height: 600, states: vec![xdg::STATE_ACTIVATED] });
            }
            UiEvent::Mapped { surface } => { println!("mapped {surface:?}"); mapped.push(*surface); }
            other => println!("{other:?}").to_owned(),
        }
        for s in &mapped { host.send(Command::FrameDone { surface: *s, time_ms: 0 }); }
    }
}
