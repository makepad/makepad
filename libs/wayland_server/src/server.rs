//! Listening socket, epoll loop and wake-up eventfd around `Core`.
use crate::core::{ClientId, Core, Handler};
use crate::sys::*;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

const TOKEN_LISTENER: u64 = 0;
const TOKEN_WAKE: u64 = 1;
const TOKEN_CLIENT: u64 = 2;

pub struct Server {
    pub core: Core,
    epoll: OwnedFd,
    wake: OwnedFd,
    listener: Option<(UnixListener, File, PathBuf)>,
    /// Clients with EPOLLOUT registered.
    writing: HashMap<ClientId, bool>,
}

/// Cloneable handle that wakes the server loop from another thread.
pub struct Waker(OwnedFd);
impl Waker {
    pub fn wake(&self) {
        let one = 1u64;
        unsafe { write(self.0.as_raw_fd(), &one as *const u64 as *const _, 8) };
    }
    pub fn try_clone(&self) -> io::Result<Waker> { Ok(Waker(self.0.try_clone()?)) }
}

fn ctl(ep: &OwnedFd, op: i32, fd: i32, events: u32, token: u64) -> io::Result<()> {
    let mut ev = epoll_event { events, data: token };
    if unsafe { epoll_ctl(ep.as_raw_fd(), op, fd, &mut ev) } < 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}

impl Server {
    pub fn new() -> io::Result<Self> {
        let ep = unsafe { epoll_create1(EPOLL_CLOEXEC) };
        if ep < 0 { return Err(io::Error::last_os_error()); }
        let epoll = unsafe { OwnedFd::from_raw_fd(ep) };
        let w = unsafe { eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK) };
        if w < 0 { return Err(io::Error::last_os_error()); }
        let wake = unsafe { OwnedFd::from_raw_fd(w) };
        ctl(&epoll, EPOLL_CTL_ADD, wake.as_raw_fd(), EPOLLIN, TOKEN_WAKE)?;
        Ok(Self { core: Core::new(), epoll, wake, listener: None, writing: HashMap::new() })
    }
    pub fn waker(&self) -> io::Result<Waker> { Ok(Waker(self.wake.try_clone()?)) }

    /// Binds `<dir>/<name>` guarded by `<name>.lock`. Fails if another server holds the lock.
    pub fn listen(&mut self, dir: &Path, name: &str) -> io::Result<()> {
        let lock_path = dir.join(format!("{name}.lock"));
        let lock = File::options().create(true).truncate(false).write(true).open(&lock_path)?;
        if unsafe { flock(lock.as_raw_fd(), LOCK_EX | LOCK_NB) } < 0 { return Err(io::Error::last_os_error()); }
        let path = dir.join(name);
        // We hold the lock, so any socket file left behind is stale.
        match std::fs::remove_file(&path) { Ok(()) => {}, Err(e) if e.kind() == io::ErrorKind::NotFound => {}, Err(e) => return Err(e) }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        ctl(&self.epoll, EPOLL_CTL_ADD, listener.as_raw_fd(), EPOLLIN, TOKEN_LISTENER)?;
        self.listener = Some((listener, lock, path));
        Ok(())
    }
    /// Picks the first free `<prefix>-N` socket name in `dir`.
    pub fn listen_auto(&mut self, dir: &Path, prefix: &str) -> io::Result<String> {
        for n in 0..32 {
            let name = format!("{prefix}-{n}");
            match self.listen(dir, &name) {
                Ok(()) => return Ok(name),
                Err(e) if e.raw_os_error() == Some(EAGAIN) || e.kind() == io::ErrorKind::AddrInUse => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AddrInUse, "no free wayland socket name"))
    }

    pub fn add_client(&mut self, h: &mut dyn Handler, stream: std::os::unix::net::UnixStream) -> io::Result<ClientId> {
        let id = self.core.add_client(stream)?;
        let fd = self.core.raw_fd(id).unwrap();
        ctl(&self.epoll, EPOLL_CTL_ADD, fd, EPOLLIN | EPOLLRDHUP, TOKEN_CLIENT + id as u64)?;
        h.client_added(&mut self.core, id);
        Ok(id)
    }

    /// Waits up to `timeout_ms` (-1 = forever) and handles what is ready. Returns true when the
    /// wake-up eventfd fired, so the caller can drain its command queue.
    pub fn run_once(&mut self, h: &mut dyn Handler, timeout_ms: i32) -> io::Result<bool> {
        let mut events = [epoll_event { events: 0, data: 0 }; 64];
        let n = unsafe { epoll_wait(self.epoll.as_raw_fd(), events.as_mut_ptr(), events.len() as i32, timeout_ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(EINTR) { Ok(false) } else { Err(e) };
        }
        let mut woken = false;
        for ev in &events[..n as usize] {
            let (flags, token) = (ev.events, ev.data);
            match token {
                TOKEN_LISTENER => self.accept_all(h),
                TOKEN_WAKE => {
                    let mut buf = 0u64;
                    unsafe { read(self.wake.as_raw_fd(), &mut buf as *mut u64 as *mut _, 8) };
                    woken = true;
                }
                t => {
                    let c = (t - TOKEN_CLIENT) as ClientId;
                    if flags & EPOLLIN != 0 { self.core.readable(h, c); }
                    if flags & (EPOLLERR | EPOLLHUP) != 0 && flags & EPOLLIN == 0 { self.core.kill(c); }
                    if flags & EPOLLRDHUP != 0 && flags & EPOLLIN == 0 { self.core.kill(c); }
                    if flags & EPOLLOUT != 0 { self.core.dirty.push(c); }
                }
            }
        }
        self.settle(h);
        Ok(woken)
    }

    fn accept_all(&mut self, h: &mut dyn Handler) {
        loop {
            let accepted = match &self.listener { Some((l, _, _)) => l.accept(), None => return };
            match accepted {
                Ok((stream, _)) => { let _ = self.add_client(h, stream); }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    }

    /// Flushes queued output, arms/disarms EPOLLOUT, and drops clients that are dying.
    /// Call after handing events to `core` from outside `run_once`.
    pub fn settle(&mut self, h: &mut dyn Handler) {
        let dirty = std::mem::take(&mut self.core.dirty);
        for c in dirty {
            if !self.core.has_client(c) { continue; }
            let done = self.core.flush(c);
            let armed = self.writing.get(&c).copied().unwrap_or(false);
            if !done && !armed {
                if let Some(fd) = self.core.raw_fd(c) {
                    let _ = ctl_raw(&self.epoll, EPOLL_CTL_MOD, fd, EPOLLIN | EPOLLRDHUP | EPOLLOUT, TOKEN_CLIENT + c as u64);
                    self.writing.insert(c, true);
                }
            } else if done && armed {
                if let Some(fd) = self.core.raw_fd(c) {
                    let _ = ctl_raw(&self.epoll, EPOLL_CTL_MOD, fd, EPOLLIN | EPOLLRDHUP, TOKEN_CLIENT + c as u64);
                    self.writing.insert(c, false);
                }
            }
        }
        for c in self.core.client_ids() {
            if self.core.is_dying(c) {
                self.core.flush(c);
                if let Some(fd) = self.core.raw_fd(c) { let _ = ctl_raw(&self.epoll, EPOLL_CTL_DEL, fd, 0, 0); }
                self.writing.remove(&c);
                self.core.disconnect(h, c);
            }
        }
        // Handlers may have queued more output while clients were disconnected.
        if !self.core.dirty.is_empty() { self.settle(h); }
    }
}
fn ctl_raw(ep: &OwnedFd, op: i32, fd: i32, events: u32, token: u64) -> io::Result<()> { ctl(ep, op, fd, events, token) }

impl Drop for Server {
    fn drop(&mut self) {
        if let Some((_, _, path)) = &self.listener { let _ = std::fs::remove_file(path); }
    }
}
