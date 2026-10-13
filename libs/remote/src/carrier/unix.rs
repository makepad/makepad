//! The local carrier: a Unix socket `$TMPDIR/makepad-remote-<pid>.sock` (0600, same-uid peers
//! only) next to the discovery file `makepad-remote-<pid>.json`. On Windows the same
//! discovery file points at the loopback carrier (`carrier::loopback`: TCP on 127.0.0.1 behind a token).

use crate::endpoint::Endpoint;
use std::io;
use std::path::PathBuf;

/// The socket and discovery file paths of process `pid`.
pub fn paths(pid: u32) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir();
    (dir.join(format!("makepad-remote-{pid}.sock")), dir.join(format!("makepad-remote-{pid}.json")))
}

/// Removes the socket and discovery file of this process (at exit).
extern "C" fn cleanup() {
    let (sock, disc) = paths(std::process::id());
    let _ = std::fs::remove_file(sock);
    let _ = std::fs::remove_file(disc);
}

#[cfg(unix)]
mod imp {
    use super::*;
    use crate::carrier::Carrier;
    use crate::frame::FrameReader;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    fn bind(path: &Path) -> io::Result<UnixListener> {
        use std::os::unix::fs::PermissionsExt;
        let l = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(l)
    }

    /// Only this user's processes may connect (the socket is 0600 too).
    fn same_user(c: &UnixStream) -> bool {
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn getuid() -> u32;
        }
        #[cfg(target_os = "macos")]
        {
            extern "C" {
                fn getpeereid(fd: i32, uid: *mut u32, gid: *mut u32) -> i32;
            }
            let (mut uid, mut gid) = (u32::MAX, 0u32);
            unsafe { getpeereid(c.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == getuid() }
        }
        #[cfg(not(target_os = "macos"))]
        {
            extern "C" {
                fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, len: *mut u32) -> i32;
            }
            // SOL_SOCKET 1, SO_PEERCRED 17: struct ucred { pid, uid, gid }
            let mut cred = [0u32; 3];
            let mut len = 12u32;
            unsafe { getsockopt(c.as_raw_fd(), 1, 17, cred.as_mut_ptr() as *mut u8, &mut len) == 0 && cred[1] == getuid() }
        }
    }

    fn carrier(s: UnixStream) -> io::Result<Carrier> {
        let r = s.try_clone()?;
        let w = s.try_clone()?;
        Ok(Carrier {
            reader: FrameReader::new(Box::new(r)),
            writer: Box::new(w),
            shutdown: Box::new(move || {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }),
        })
    }

    /// Connects to the endpoint socket at `path`. The read timeout is for the handshake: call
    /// [`settle`] on the returned stream handle afterwards.
    pub fn dial(path: &Path) -> io::Result<(Carrier, UnixStream)> {
        let s = UnixStream::connect(path)?;
        s.set_read_timeout(Some(std::time::Duration::from_secs(1)))?;
        let keep = s.try_clone()?;
        Ok((carrier(s)?, keep))
    }

    /// Ends the handshake read timeout.
    pub fn settle(s: &UnixStream) {
        let _ = s.set_read_timeout(None);
    }

    /// Listens on this process's socket and writes the discovery file: `{"pid","app","socket",
    /// "started_ms","caps"}` plus `extra` (already-serialised `,"key":value` fields). Register the
    /// services first: the caps are written now. Returns the socket path.
    pub fn listen(ep: &Endpoint, extra: &str) -> io::Result<PathBuf> {
        let _ = crate::cmd::now_ms();
        let pid = std::process::id();
        let (sock, disc) = paths(pid);
        let _ = std::fs::remove_file(&sock);
        let listener = bind(&sock)?;
        let caps: Vec<String> = ep.caps().iter().map(|c| crate::cmd::json_str(c)).collect();
        let info = format!(
            "{{\"pid\":{},\"app\":{},\"socket\":{},\"started_ms\":{},\"caps\":[{}]{}}}\n",
            pid,
            crate::cmd::json_str(ep.app()),
            crate::cmd::json_str(&sock.display().to_string()),
            crate::cmd::wall_ms(),
            caps.join(","),
            extra
        );
        let _ = std::fs::write(&disc, info);
        ep.set_discovery(disc.clone());
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(cleanup) };
        accept_loop(ep, listener);
        Ok(sock)
    }

    fn accept_loop(ep: &Endpoint, listener: UnixListener) {
        let ep = ep.clone();
        let hooks = ep.hooks();
        let _ = std::thread::Builder::new().name("makepad-remote-accept".into()).spawn(move || {
            (hooks.enter)();
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                if !same_user(&conn) {
                    continue;
                }
                let ep = ep.clone();
                let _ = std::thread::Builder::new().name("makepad-remote-hello".into()).spawn(move || {
                    let (hooks, keep) = (ep.hooks(), conn.try_clone());
                    (hooks.enter)();
                    let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                    if let Ok(c) = carrier(conn) {
                        if let Err(e) = ep.serve(c.reader, c.writer, c.shutdown) {
                            eprintln!("makepad-remote: a client was refused: {e}");
                        } else if let Ok(k) = keep {
                            settle(&k);
                        }
                    }
                    (hooks.exit)();
                });
            }
        });
    }

    /// Listens on the socket at `path` (0600, same-uid peers) without a discovery file: for a
    /// service with a fixed address such as the hub. The caller has made sure no live process
    /// serves the path; a stale socket file is replaced.
    pub fn listen_at(ep: &Endpoint, path: &Path) -> io::Result<()> {
        let _ = std::fs::remove_file(path);
        accept_loop(ep, bind(path)?);
        Ok(())
    }
}

#[cfg(unix)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::*;
    /// No Unix sockets here: the endpoint listens on loopback TCP (ephemeral port) behind a random
    /// token, both written to the discovery file `{"pid","app","socket":"tcp:127.0.0.1:PORT",
    /// "token","started_ms","caps"}` (+ `extra`). Returns `tcp:127.0.0.1:PORT`.
    pub fn listen(ep: &Endpoint, extra: &str) -> io::Result<PathBuf> {
        let _ = crate::cmd::now_ms();
        let token = crate::carrier::loopback::new_token()?;
        let addr = crate::carrier::loopback::serve(ep, &token)?;
        let socket = format!("tcp:{addr}");
        let pid = std::process::id();
        let caps: Vec<String> = ep.caps().iter().map(|c| crate::cmd::json_str(c)).collect();
        let info = format!(
            "{{\"pid\":{},\"app\":{},\"socket\":{},\"token\":{},\"started_ms\":{},\"caps\":[{}]{}}}\n",
            pid,
            crate::cmd::json_str(ep.app()),
            crate::cmd::json_str(&socket),
            crate::cmd::json_str(&token),
            crate::cmd::wall_ms(),
            caps.join(","),
            extra
        );
        std::fs::write(paths(pid).1, info)?;
        ep.set_discovery(paths(pid).1);
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(cleanup) };
        Ok(PathBuf::from(socket))
    }
    pub fn listen_at(_ep: &Endpoint, _path: &std::path::Path) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "makepad-remote is not available on Windows yet"))
    }
}

#[cfg(windows)]
pub use imp::*;
