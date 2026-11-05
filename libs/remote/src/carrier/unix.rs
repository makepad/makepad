//! The local carrier: a Unix socket `makepad-remote-<pid>.sock` in the user's socket dir
//! ([`socket_dir`]; 0600, same-uid peers only); its discovery file `makepad-remote-<pid>.json` is
//! in the user's registry dir ([`crate::discovery::registry_dir`]). Both are the same for every
//! process whatever its TMPDIR. On Windows the same discovery file points at the loopback carrier
//! (`carrier::loopback`: TCP on 127.0.0.1 behind a token).

use std::path::PathBuf;

/// The socket and discovery file paths of process `pid`.
pub fn paths(pid: u32) -> (PathBuf, PathBuf) {
    (
        socket_dir().join(format!("makepad-remote-{pid}.sock")),
        crate::discovery::registry_dir().join(format!("makepad-remote-{pid}.json")),
    )
}

/// Where this user's endpoint sockets are, whatever the process's TMPDIR: a socket's path holds
/// about a hundred bytes, and a run's TMPDIR can be deep (a test's own home under a lease's
/// scratch dir made one 121 bytes long, and the JIT in it could not be reached: a capture held a
/// jitlegs leg to twice its time, 11-05). A desktop Unix: `/tmp/makepad-<uid>`, this user's own
/// (0700, made by the first endpoint that listens). A phone's app writes only in its own temp
/// dir, and that one is short.
pub fn socket_dir() -> PathBuf {
    #[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
    {
        extern "C" {
            fn getuid() -> u32;
        }
        PathBuf::from(format!("/tmp/makepad-{}", unsafe { getuid() }))
    }
    #[cfg(not(all(unix, not(any(target_os = "android", target_os = "ios")))))]
    std::env::temp_dir()
}

/// [`socket_dir`], made this user's own first (0700; one that is not this user's own directory is
/// refused): for files of this user's processes no other user may read, replace or link (a restart's
/// state for the next run).
#[cfg(unix)]
pub fn own_socket_dir() -> std::io::Result<PathBuf> {
    let dir = socket_dir();
    imp::make_own(&dir)?;
    Ok(dir)
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
    use crate::endpoint::Endpoint;
    use std::io;
    use crate::carrier::Carrier;
    use crate::frame::FrameReader;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    /// Makes [`socket_dir`] this user's own directory (0700) before a socket goes in it.
    fn own_dir(sock: &Path) -> io::Result<()> {
        make_own(sock.parent().unwrap_or(Path::new("/")))
    }

    /// Makes `dir` this user's own directory (0700). In /tmp anyone may make one: a directory (or
    /// link) that is not this user's is refused, never used.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) fn make_own(dir: &Path) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        extern "C" {
            fn getuid() -> u32;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(io::Error::new(e.kind(), format!("its socket dir {} could not be made: {e}", dir.display()))),
            _ => {}
        }
        let m = std::fs::symlink_metadata(dir)?;
        if !m.is_dir() || m.uid() != unsafe { getuid() } {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("its socket dir {} is not this user's own directory", dir.display())));
        }
        if m.mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// A phone's app temp dir is its own already.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub(super) fn make_own(_: &Path) -> io::Result<()> {
        Ok(())
    }

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

    /// The longest socket path a `sockaddr_un` holds, its terminating zero not counted.
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
    pub(super) const SOCKET_PATH_MAX: usize = 103;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")))]
    pub(super) const SOCKET_PATH_MAX: usize = 107;

    /// Why the socket at `path` cannot be bound: its length said plainly, else the bind's error.
    fn unbindable(path: &Path, e: &io::Error) -> String {
        let n = path.as_os_str().len();
        if n > SOCKET_PATH_MAX {
            format!("its socket path {} is {} bytes, over the {} a Unix socket's path holds", path.display(), n, SOCKET_PATH_MAX)
        } else {
            format!("its socket {} could not be bound: {}", path.display(), e)
        }
    }

    /// Listens on this process's socket and writes the discovery file: `{"pid","app","socket",
    /// "started_ms","caps"}` plus `extra` (already-serialised `,"key":value` fields). Register the
    /// services first: the caps are written now. Returns the socket path.
    /// A socket that cannot be bound is an error, and the discovery file says why
    /// (`"socket_error"`, no caps): a client names the reason instead of finding no app.
    pub fn listen(ep: &Endpoint, extra: &str) -> io::Result<PathBuf> {
        let _ = crate::cmd::now_ms();
        let (sock, disc) = paths(std::process::id());
        let served = serve_announced(ep, extra, &sock, &disc, own_dir);
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(cleanup) };
        served.map(|()| sock)
    }

    /// [`listen`] on the socket `sock`, announced in the discovery file `disc`; `ready` makes
    /// the socket's directory ready first ([`own_dir`]).
    pub(super) fn serve_announced(ep: &Endpoint, extra: &str, sock: &Path, disc: &Path, ready: fn(&Path) -> io::Result<()>) -> io::Result<()> {
        let bound = ready(sock).and_then(|()| {
            let _ = std::fs::remove_file(sock);
            bind(sock).map_err(|e| io::Error::new(e.kind(), unbindable(sock, &e)))
        });
        let caps: Vec<String> = if bound.is_ok() { ep.caps().iter().map(|c| crate::cmd::json_str(c)).collect() } else { Vec::new() };
        let info = format!(
            "{{\"pid\":{},\"app\":{},\"socket\":{},\"started_ms\":{},\"caps\":[{}]{}{}}}\n",
            std::process::id(),
            crate::cmd::json_str(ep.app()),
            crate::cmd::json_str(&sock.display().to_string()),
            crate::cmd::wall_ms(),
            caps.join(","),
            bound.as_ref().err().map_or(String::new(), |e| format!(",\"socket_error\":{}", crate::cmd::json_str(&e.to_string()))),
            format!("{extra}{}", crate::discovery::lease_field())
        );
        let _ = std::fs::write(disc, info);
        let listener = bound?;
        ep.set_discovery(disc.to_path_buf());
        accept_loop(ep, listener);
        Ok(())
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
    /// serves the path; a stale socket file is replaced. Returns the listener's descriptor, which
    /// a process that execs a new image of itself can keep open for it ([`serve_listener`]).
    pub fn listen_at(ep: &Endpoint, path: &Path) -> io::Result<std::os::unix::io::RawFd> {
        use std::os::unix::io::AsRawFd;
        let _ = std::fs::remove_file(path);
        let listener = bind(path)?;
        let fd = listener.as_raw_fd();
        accept_loop(ep, listener);
        Ok(fd)
    }

    /// Serves `ep` on a listener this process holds already: one its previous image bound with
    /// [`listen_at`] and kept open over an exec. The socket file stays, and the dials that queued
    /// on it while the new image started are served now.
    pub fn serve_listener(ep: &Endpoint, listener: UnixListener) {
        accept_loop(ep, listener);
    }
}

#[cfg(unix)]
pub use imp::*;

#[cfg(all(test, unix))]
mod tests {
    use super::imp::*;

    /// A socket path longer than a Unix socket holds cannot be bound (one in a deep TMPDIR could
    /// not, before the sockets had a dir of their own: `makepad inspect` found no app and
    /// waited). The run's discovery file says why, and a client names it.
    #[test]
    fn a_socket_path_too_long_to_bind_is_said_in_the_discovery_file() {
        let sock = std::env::temp_dir().join("d".repeat(SOCKET_PATH_MAX)).join("makepad-remote-1.sock");
        let disc = std::env::temp_dir().join(format!("makepad-remote-long-test-{}.json", std::process::id()));
        let ep = crate::endpoint::Endpoint::new("makepad-remote-long");
        ep.register_cmd(|_, _| {});
        let err = serve_announced(&ep, "", &sock, &disc, |_| Ok(())).unwrap_err().to_string();
        assert!(err.contains(&format!("bytes, over the {SOCKET_PATH_MAX}")), "{err}");
        let app = crate::discovery::parse(&std::fs::read_to_string(&disc).unwrap()).unwrap();
        std::fs::remove_file(&disc).unwrap();
        assert!(app.caps.is_empty(), "nothing is served: {:?}", app.caps);
        let why = app.unreachable().expect("the discovery file says why");
        assert!(why.contains("runs but cannot be reached") && why.contains(&err), "{why}");
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::endpoint::Endpoint;
    use std::io;
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
            format!("{extra}{}", crate::discovery::lease_field())
        );
        std::fs::write(paths(pid).1, info)?;
        ep.set_discovery(paths(pid).1);
        extern "C" {
            fn atexit(f: extern "C" fn()) -> i32;
        }
        unsafe { atexit(cleanup) };
        Ok(PathBuf::from(socket))
    }
    /// Serves `ep` at a fixed address such as the hub's: loopback TCP behind a fresh token, both
    /// written to the file at `path` (`{"pid","socket":"tcp:127.0.0.1:PORT","token"}`, in the
    /// user's own temp folder, which only that user can read). [`resolve`] is the dial side.
    pub fn listen_at(ep: &Endpoint, path: &std::path::Path) -> io::Result<()> {
        let token = crate::carrier::loopback::new_token()?;
        let addr = crate::carrier::loopback::serve(ep, &token)?;
        let info = format!(
            "{{\"pid\":{},\"app\":{},\"socket\":{},\"token\":{},\"started_ms\":{},\"caps\":[]}}\n",
            std::process::id(),
            crate::cmd::json_str(ep.app()),
            crate::cmd::json_str(&format!("tcp:{addr}")),
            crate::cmd::json_str(&token),
            crate::cmd::wall_ms()
        );
        let _ = std::fs::remove_file(path);
        makepad_network::tls::write_private(path, info.as_bytes())
    }

    /// The loopback address and token a [`listen_at`] file at `path` names.
    pub fn resolve(path: &std::path::Path) -> io::Result<(String, String)> {
        let text = std::fs::read_to_string(path)?;
        let app = crate::discovery::parse(&text).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("{}: not an endpoint file", path.display())))?;
        let addr = app.socket.strip_prefix("tcp:").ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("{}: no tcp: socket", path.display())))?;
        Ok((addr.to_string(), app.text("token").unwrap_or_default()))
    }
}

#[cfg(windows)]
pub use imp::*;
