//! The server on its own thread. The host (UI) thread sends `Command`s and receives `UiEvent`s,
//! so a stalling or flooding client can never block the host's frame loop.
use crate::compositor::{Board, Compositor, OutputInfo, SurfKey, UiEvent};
use crate::server::{Server, Waker};
use std::io;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;

pub enum Command {
    ConfigureToplevel { surface: SurfKey, width: i32, height: i32, states: Vec<u32> },
    CloseToplevel(SurfKey),
    DismissPopup(SurfKey),
    /// Frame callbacks for the surface and the buffers it composites (`time_ms` is the present time).
    FrameDone { surface: SurfKey, time_ms: u32 },
    PointerEnter { surface: SurfKey, x: f64, y: f64 },
    PointerLeave,
    PointerMotion { time: u32, x: f64, y: f64 },
    PointerButton { time: u32, button: u32, pressed: bool },
    PointerAxis { time: u32, axis: u32, value: f64, v120: i32 },
    KeyboardFocus(Option<SurfKey>),
    Key { time: u32, evdev: u16, pressed: bool },
    PopupBounds(crate::compositor::Rect),
    SetScale(f64),
    /// The host finished reading a dmabuf buffer; the fence covers its reads.
    BufferDone { client: u32, buffer: u32, fence: Option<std::os::fd::OwnedFd> },
    /// A host app copied (or selected) text: it becomes the Wayland clients' selection.
    SetSelection { board: Board, text: String },
    /// A client selection's text was read (the server's own reading thread sends this).
    SelectionRead { board: Board, generation: u64, text: String },
    Shutdown,
}

pub struct Host {
    /// Socket name inside the runtime dir, for `WAYLAND_DISPLAY`.
    pub socket: String,
    pub events: Receiver<UiEvent>,
    tx: Sender<Command>,
    waker: Waker,
    join: Option<JoinHandle<()>>,
}

impl Host {
    /// Starts listening on `<dir>/<prefix>-N`. `notify` is called (from the server thread) whenever
    /// new `UiEvent`s were queued, so the host can wake its own loop.
    pub fn spawn(dir: &Path, prefix: &str, output: OutputInfo, layout: &'static xkb_keymap::Layout, dmabuf: Option<crate::compositor::DmabufConfig>, notify: impl Fn() + Send + 'static) -> io::Result<Host> {
        let mut srv = Server::new()?;
        let socket = srv.listen_auto(dir, prefix)?;
        let mut comp = Compositor::new(&mut srv.core, output, layout)?;
        if let Some(cfg) = dmabuf { comp.enable_dmabuf(&mut srv.core, cfg)?; }
        let waker = srv.waker()?;
        let (tx, rx) = channel::<Command>();
        // client selections are read off the server thread; the text comes back as a command
        let (read_tx, read_waker) = (tx.clone(), srv.waker()?);
        comp.set_selection_reader(std::sync::Arc::new(move |board, generation, text| {
            if read_tx.send(Command::SelectionRead { board, generation, text }).is_ok() { read_waker.wake(); }
        }));
        let (etx, erx) = channel::<UiEvent>();
        let join = std::thread::Builder::new().name("wayland-server".into()).spawn(move || {
            'run: loop {
                let timeout = if comp.has_gated_events() { 1 } else { -1 };
                let woken = match srv.run_once(&mut comp, timeout) { Ok(w) => w, Err(_) => break };
                if woken {
                    while let Ok(cmd) = rx.try_recv() {
                        if !apply(&mut srv, &mut comp, cmd) { break 'run; }
                    }
                    srv.settle(&mut comp);
                }
                let mut events = comp.take_events();
                events.extend(std::mem::take(&mut srv.core.killed).into_iter().map(|(client, why)| UiEvent::ClientKilled { client, why }));
                if !events.is_empty() {
                    for e in events { if etx.send(e).is_err() { break 'run; } }
                    notify();
                }
            }
        })?;
        Ok(Host { socket, events: erx, tx, waker, join: Some(join) })
    }
    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_ok() { self.waker.wake(); }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.send(Command::Shutdown);
        if let Some(j) = self.join.take() { let _ = j.join(); }
    }
}

fn apply(srv: &mut Server, comp: &mut Compositor, cmd: Command) -> bool {
    let core = &mut srv.core;
    match cmd {
        Command::ConfigureToplevel { surface, width, height, states } => comp.configure_toplevel(core, surface, width, height, &states),
        Command::CloseToplevel(k) => comp.close_toplevel(core, k),
        Command::DismissPopup(k) => comp.dismiss_popup(core, k),
        Command::FrameDone { surface, time_ms } => comp.frame_done(core, surface, time_ms),
        Command::PointerEnter { surface, x, y } => comp.pointer_enter(core, surface, x, y),
        Command::PointerLeave => comp.pointer_leave(core),
        Command::PointerMotion { time, x, y } => comp.pointer_motion(core, time, x, y),
        Command::PointerButton { time, button, pressed } => { comp.pointer_button(core, time, button, pressed); }
        Command::PointerAxis { time, axis, value, v120 } => comp.pointer_axis(core, time, axis, value, v120),
        Command::KeyboardFocus(k) => comp.keyboard_focus(core, k),
        Command::Key { time, evdev, pressed } => comp.key(core, time, evdev, pressed),
        Command::PopupBounds(r) => comp.popup_bounds = r,
        Command::SetScale(s) => comp.set_scale(core, s),
        Command::BufferDone { client, buffer, fence } => comp.buffer_done(core, client, buffer, fence),
        Command::SetSelection { board, text } => comp.set_host_selection(core, board, text),
        Command::SelectionRead { board, generation, text } => comp.selection_read(core, board, generation, text),
        Command::Shutdown => return false,
    }
    true
}
