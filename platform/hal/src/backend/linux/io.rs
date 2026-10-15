//! Nonblocking desktop fd pump. The wake socket is the equivalent of SelectTimers' self-pipe;
//! neither stdin nor a continuously ticking idle timer participates.
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Instant;

#[repr(C)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}
pub const READ: i16 = 1;
pub const WRITE: i16 = 4;

pub(super) mod memory {
    use std::ffi::{c_char, c_void};
    pub const MFD_CLOEXEC: u32 = 1;
    pub const PROT_READ: i32 = 1;
    pub const PROT_WRITE: i32 = 2;
    pub const MAP_SHARED: i32 = 1;
    pub const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
    extern "C" {
        pub fn memfd_create(name: *const c_char, flags: u32) -> i32;
        pub fn ftruncate(fd: i32, size: i64) -> i32;
        pub fn mmap(
            address: *mut c_void,
            length: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        pub fn munmap(address: *mut c_void, length: usize) -> i32;
    }
}
extern "C" {
    pub(crate) fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn pipe2(fds: *mut i32, flags: i32) -> i32;
}

pub fn nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { fcntl(fd, 3) };
    if flags < 0 || unsafe { fcntl(fd, 4, flags | 0o4000) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { pipe2(fds.as_mut_ptr(), 0o2000000) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // Only our read end is nonblocking. The sender owns the other end's write policy.
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    nonblocking(read.as_raw_fd())?;
    Ok((read, write))
}
pub fn wait(wake: i32, fds: &mut [PollFd], deadline: Option<Instant>) -> Result<(), String> {
    let mut all = Vec::with_capacity(fds.len() + 1);
    all.push(PollFd {
        fd: wake,
        events: READ,
        revents: 0,
    });
    all.extend(fds.iter().map(|p| PollFd {
        fd: p.fd,
        events: p.events,
        revents: 0,
    }));
    let timeout = deadline.map_or(-1, |d| {
        d.saturating_duration_since(Instant::now())
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32
    });
    if unsafe { poll(all.as_mut_ptr(), all.len(), timeout) } < 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e.to_string());
        }
    }
    for (out, got) in fds.iter_mut().zip(&all[1..]) {
        out.revents = got.revents;
    }
    Ok(())
}

pub struct ReadPipe<T> {
    pub file: File,
    pub bytes: Vec<u8>,
    pub target: T,
}
impl<T> ReadPipe<T> {
    pub fn read(&mut self) -> io::Result<bool> {
        let mut buf = [0; 8192];
        loop {
            match self.file.read(&mut buf) {
                Ok(0) => return Ok(true),
                Ok(n) => self.bytes.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }
}
pub struct WritePipe {
    pub file: File,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub offset: usize,
}
impl WritePipe {
    pub fn write(&mut self) -> io::Result<bool> {
        while self.offset < self.bytes.len() {
            match self.file.write(&self.bytes[self.offset..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.offset += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}
