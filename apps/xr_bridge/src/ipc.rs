//! One game connection, one outstanding UI transaction, one long-lived worker.
use makepad_widgets::{Cx, SignalToUI};
use makepad_widgets::makepad_platform::thread::ThreadOptions;
use makepad_xr_runtime::protocol::{self, ipc::Channel, Message};
use std::{io, net::Shutdown, os::{fd::{AsRawFd, OwnedFd}, unix::{fs::{FileTypeExt, MetadataExt, PermissionsExt}, net::{UnixListener, UnixStream}}}, sync::{Arc, Mutex, mpsc::{sync_channel, Receiver, SyncSender, TrySendError}}};

pub enum Request { Listening, Packet(Message, Option<OwnedFd>), Disconnected, Failed(String) }
pub struct Reply { pub message: Option<Message>, pub fence: Option<OwnedFd> }
impl Reply { pub fn message(message: Option<Message>) -> Self { Self { message, fence: None } } }

pub struct Worker {
    pub requests: Receiver<Request>,
    replies: Option<SyncSender<Reply>>,
    pending: Option<Reply>,
    stop: UnixStream,
    active: Arc<Mutex<Option<UnixStream>>>,
}

#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }
extern "C" { fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32; fn geteuid() -> u32; }
fn wait(fd: i32, stop: &UnixStream) -> io::Result<()> {
    let mut fds = [PollFd { fd, events: 1, revents: 0 }, PollFd { fd: stop.as_raw_fd(), events: 1, revents: 0 }];
    loop {
        let result = unsafe { poll(fds.as_mut_ptr(), fds.len(), -1) };
        if result < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted { continue; }
            return Err(e);
        }
        if fds[1].revents != 0 { return Err(io::Error::new(io::ErrorKind::Interrupted, "XR bridge stopped")); }
        if fds[0].revents & (8 | 32) != 0 { return Err(io::Error::other("XR descriptor failed")); }
        if fds[0].revents & (1 | 16) != 0 { return Ok(()); }
    }
}

impl Worker {
    pub fn start(cx: &mut Cx) -> io::Result<Self> {
        let (stop, stopped) = UnixStream::pair()?;
        // A protocol transaction owns the single slot until answered. A game
        // cannot flood the UI; a slow UI backpressures this worker, never itself.
        let (requests, rx) = sync_channel(1);
        let (tx, replies) = sync_channel::<Reply>(1);
        let active = Arc::new(Mutex::new(None));
        let worker_active = active.clone();
        cx.thread_spawner().spawn_worker(ThreadOptions { name: Some("xr-game".into()), ..Default::default() }, move || {
            let send = |request| {
                requests.send(request).map_err(|_| io::Error::other("XR bridge closed"))?;
                SignalToUI::set_ui_signal();
                Ok::<(), io::Error>(())
            };
            let run = || -> io::Result<()> {
                let path = protocol::socket_path();
                // Only remove our user's stale socket, never an arbitrary file
                // or a listener belonging to an already running bridge.
                if let Ok(meta) = std::fs::symlink_metadata(&path) {
                    if !meta.file_type().is_socket() || meta.uid() != unsafe { geteuid() } {
                        return Err(io::Error::other("XR socket path is occupied"));
                    }
                    match UnixStream::connect(&path) {
                        Ok(_) => return Err(io::Error::other("XR Bridge is already running")),
                        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => std::fs::remove_file(&path)?,
                        Err(e) => return Err(e),
                    }
                }
                let listener = UnixListener::bind(&path)?;
                struct Socket(std::path::PathBuf);
                impl Drop for Socket { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
                let _socket = Socket(path.clone());
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
                send(Request::Listening)?;
                loop {
                    wait(listener.as_raw_fd(), &stopped)?;
                    let (stream, _) = listener.accept()?;
                    let mut channel = match Channel::new(stream) { Ok(c) => c, Err(e) => { eprintln!("xr bridge: rejected peer: {e}"); continue; } };
                    *worker_active.lock().unwrap() = Some(channel.cancel_handle()?);
                    while let Ok((message, fd)) = channel.recv() {
                        let stop = matches!(message, Message::Stop);
                        send(Request::Packet(message, fd))?;
                        let reply = replies.recv().map_err(|_| io::Error::other("XR bridge closed"))?;
                        if let Some(fence) = reply.fence {
                            // GpuCompletion publishes a sync_file before it
                            // signals. Await the actual GPU read before reuse.
                            wait(fence.as_raw_fd(), &stopped)?;
                        }
                        if let Some(message) = reply.message {
                            let fatal = matches!(message, Message::Error(_));
                            if channel.send(&message, None).is_err() || fatal { break; }
                        }
                        if stop { break; }
                    }
                    *worker_active.lock().unwrap() = None;
                    send(Request::Disconnected)?;
                    // Let the UI release this connection's imported textures
                    // before admitting a replacement game.
                    replies.recv().map_err(|_| io::Error::other("XR bridge closed"))?;
                }
            };
            if let Err(e) = run() { let _ = send(Request::Failed(e.to_string())); }
        }).map_err(|e| io::Error::other(format!("XR worker: {e:?}")))?.detach();
        Ok(Self { requests: rx, replies: Some(tx), pending: None, stop, active })
    }

    pub fn reply(&mut self, reply: Reply) {
        debug_assert!(self.pending.is_none());
        self.pending = Some(reply);
        self.flush();
    }
    /// The game can close while WaitFrame is held for a sleeping headset. A
    /// nonblocking descriptor check lets the UI cancel that pending transaction
    /// on its existing status tick, so another game can connect.
    pub fn peer_closed(&self) -> bool {
        let active = self.active.lock().unwrap();
        let Some(stream) = active.as_ref() else { return false; };
        let mut fd = PollFd { fd: stream.as_raw_fd(), events: 0x2000, revents: 0 }; // POLLRDHUP
        unsafe { poll(&mut fd, 1, 0) > 0 && fd.revents & (0x2000 | 8 | 16 | 32) != 0 }
    }
    pub fn flush(&mut self) {
        let Some(reply) = self.pending.take() else { return; };
        let Some(tx) = &self.replies else { return; };
        match tx.try_send(reply) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => (),
            Err(TrySendError::Full(reply)) => {
                eprintln!("xr bridge: reply queued until worker is ready");
                self.pending = Some(reply);
                SignalToUI::set_ui_signal();
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop.shutdown(Shutdown::Both);
        if let Some(stream) = self.active.lock().unwrap().take() { let _ = stream.shutdown(Shutdown::Both); }
        self.replies.take();
    }
}
