//! The desktop single-instance handoff behind `AppMain::single_instance`.
//!
//! The first launch CLAIMS the app's name and listens on it; a later launch
//! finds the claim taken, connects, sends its command line and exits once
//! the running instance has queued it (`Event::AppOpen`).
//!
//! - macOS / Linux: an exclusive `flock` on `<dir>/<key>.lock` is the claim
//!   and `<dir>/<key>.sock` (a Unix socket) the door. The lock dies with its
//!   process, so a crashed instance leaves no claim behind: the next launch
//!   takes the lock and replaces the stale socket file. Two launches at once
//!   race for the lock, never for the socket. `<dir>` is the user's own
//!   runtime directory (`$TMPDIR` on macOS, `$XDG_RUNTIME_DIR` on Linux,
//!   else a 0700 `/tmp/makepad-<uid>`), so users never meet.
//! - Windows: a named pipe `\\.\pipe\makepad-<user>-<key>`. Creating its
//!   first instance (`FILE_FLAG_FIRST_PIPE_INSTANCE`) is the claim; the pipe
//!   disappears with its process. The pipe's default DACL lets only its own
//!   user (and administrators) write to it.
//!
//! The wire format: `makepad-app-open/1\0`, each item and a NUL, a final
//! NUL; the answer is `ok\n` once the items are queued.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const MAGIC: &[u8] = b"makepad-app-open/1\0";
const REPLY: &[u8] = b"ok\n";
const MAX_REQUEST: usize = 1 << 20;
/// A running instance claims before its `Cx` exists and accepts once it
/// does, so a launch in that gap waits: this long at most, then it runs on
/// its own.
pub(crate) const HANDOFF_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(20);
/// A connected peer that stops talking does not hold the door for long.
#[cfg(unix)]
const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) enum Claim<S> {
    Primary(S),
    Forwarded,
    Standalone,
}

pub(crate) type Sink = Box<dyn FnMut(Vec<String>) + Send>;

/// The name one app's instances share: its package name, and a hash of the
/// package name and the executable's path, so another build of the same app
/// (a worktree, a debug build, an installed copy) is another instance.
pub(crate) fn instance_key(app_name: &str, exe: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in app_name
        .as_bytes()
        .iter()
        .chain(&[0])
        .chain(exe.to_string_lossy().as_bytes())
    {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let name: String = app_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(24)
        .collect();
    format!("{name}-{hash:016x}")
}

fn write_request(w: &mut impl Write, items: &[String]) -> io::Result<()> {
    let mut buf = MAGIC.to_vec();
    for item in items {
        // Neither can travel (NUL ends an item, an empty one the request),
        // and neither names anything.
        if item.is_empty() || item.contains('\0') {
            continue;
        }
        buf.extend_from_slice(item.as_bytes());
        buf.push(0);
    }
    buf.push(0);
    w.write_all(&buf)?;
    w.flush()
}

fn read_request(r: &mut impl BufRead) -> io::Result<Vec<String>> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_string());
    let mut magic = [0u8; MAGIC.len()];
    r.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(invalid("not an app-open request"));
    }
    let mut items = Vec::new();
    let mut total = 0;
    loop {
        let mut item = Vec::new();
        let n = r
            .by_ref()
            .take((MAX_REQUEST - total) as u64)
            .read_until(0, &mut item)?;
        total += n;
        if item.pop() != Some(0) {
            return Err(invalid("app-open request cut short or too long"));
        }
        if item.is_empty() {
            return Ok(items);
        }
        items.push(String::from_utf8(item).map_err(|_| invalid("app-open item is not UTF-8"))?);
    }
}

/// The later launch's side: send the items and wait for the running
/// instance to take them.
fn forward(stream: &mut (impl Read + Write), items: &[String]) -> io::Result<()> {
    write_request(stream, items)?;
    let mut reply = [0u8; REPLY.len()];
    stream.read_exact(&mut reply)?;
    if reply != REPLY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected app-open reply"));
    }
    Ok(())
}

/// The running instance's side of one connection.
fn answer(stream: &mut (impl Read + Write), sink: &mut Sink) -> io::Result<()> {
    let items = read_request(&mut BufReader::new(&mut *stream))?;
    sink(items);
    stream.write_all(REPLY)?;
    stream.flush()
}

#[cfg(unix)]
pub(crate) mod unix {
    use super::*;
    use std::fs::{File, OpenOptions, TryLockError};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub(crate) struct Server {
        listener: UnixListener,
        /// Held for the life of the process: the claim.
        _lock: File,
    }

    extern "C" {
        fn getuid() -> u32;
    }

    /// The user's own directory for the claim and the socket.
    pub(crate) fn runtime_dir() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        let own = "TMPDIR";
        #[cfg(not(target_os = "macos"))]
        let own = "XDG_RUNTIME_DIR";
        if let Some(dir) = std::env::var_os(own).filter(|dir| !dir.is_empty()) {
            return Some(PathBuf::from(dir));
        }
        let uid = unsafe { getuid() };
        let dir = PathBuf::from(format!("/tmp/makepad-{uid}"));
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
        // Someone else's directory under that name is not ours to use.
        let meta = std::fs::symlink_metadata(&dir).ok()?;
        (meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0).then_some(dir)
    }

    pub(crate) fn claim(dir: &Path, key: &str, items: &[String], timeout: Duration) -> Claim<Server> {
        let lock_path = dir.join(format!("{key}.lock"));
        let socket_path = dir.join(format!("{key}.sock"));
        let deadline = Instant::now() + timeout;
        loop {
            let lock = match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
            {
                Ok(lock) => lock,
                Err(err) => {
                    crate::warning!("single instance: cannot open {}: {err}", lock_path.display());
                    return Claim::Standalone;
                }
            };
            match lock.try_lock() {
                Ok(()) => {
                    // Whatever socket file is left belongs to a dead instance.
                    let _ = std::fs::remove_file(&socket_path);
                    return match UnixListener::bind(&socket_path) {
                        Ok(listener) => Claim::Primary(Server { listener, _lock: lock }),
                        Err(err) => {
                            crate::warning!(
                                "single instance: cannot listen on {}: {err}",
                                socket_path.display()
                            );
                            Claim::Standalone
                        }
                    };
                }
                Err(TryLockError::WouldBlock) => {
                    drop(lock);
                    // The claimant binds right after locking; until then, or
                    // if it just died, try again from the top.
                    if let Ok(mut stream) = UnixStream::connect(&socket_path) {
                        let _ = stream.set_read_timeout(Some(timeout));
                        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                        return match forward(&mut stream, items) {
                            Ok(()) => Claim::Forwarded,
                            Err(err) => {
                                crate::warning!("single instance: handoff failed: {err}");
                                Claim::Standalone
                            }
                        };
                    }
                }
                Err(TryLockError::Error(err)) => {
                    crate::warning!("single instance: cannot lock {}: {err}", lock_path.display());
                    return Claim::Standalone;
                }
            }
            if Instant::now() >= deadline {
                crate::warning!("single instance: the running instance does not answer");
                return Claim::Standalone;
            }
            std::thread::sleep(RETRY);
        }
    }

    impl Server {
        pub(crate) fn serve(self, mut sink: Sink) {
            let spawned = std::thread::Builder::new()
                .name("single-instance".into())
                .spawn(move || {
                    let _lock = self._lock;
                    for stream in self.listener.incoming() {
                        let Ok(mut stream) = stream else { continue };
                        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                        if let Err(err) = answer(&mut stream, &mut sink) {
                            crate::warning!("single instance: bad request: {err}");
                        }
                    }
                });
            if let Err(err) = spawned {
                crate::warning!("single instance: cannot start its listener: {err}");
            }
        }
    }
}

#[cfg(windows)]
pub(crate) mod windows_pipe {
    use super::*;
    use crate::windows::{
        core::{BOOL, PCWSTR},
        Win32::Foundation::HANDLE,
    };
    use std::fs::{File, OpenOptions};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};

    const PIPE_ACCESS_DUPLEX: u32 = 0x3;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8; // PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT are 0
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_PIPE_CONNECTED: i32 = 535;

    windows_core::link!("kernel32.dll" "system" fn CreateNamedPipeW(lpname: PCWSTR, dwopenmode: u32, dwpipemode: u32, nmaxinstances: u32, noutbuffersize: u32, ninbuffersize: u32, ndefaulttimeout: u32, lpsecurityattributes: *const core::ffi::c_void) -> HANDLE);
    windows_core::link!("kernel32.dll" "system" fn ConnectNamedPipe(hnamedpipe: HANDLE, lpoverlapped: *mut core::ffi::c_void) -> BOOL);
    windows_core::link!("kernel32.dll" "system" fn GetNamedPipeServerProcessId(pipe: HANDLE, serverprocessid: *mut u32) -> BOOL);
    windows_core::link!("user32.dll" "system" fn AllowSetForegroundWindow(dwprocessid: u32) -> BOOL);

    pub(crate) struct Server {
        name: Vec<u16>,
        /// The pipe instance created by the claim (a raw handle: `HANDLE`
        /// is not `Send`).
        first: isize,
    }

    /// One user's pipe for `key`: pipe names are machine-wide.
    pub(crate) fn pipe_name(key: &str) -> String {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_default()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        format!(r"\\.\pipe\makepad-{user}-{key}")
    }

    fn create(name: &[u16], first: bool) -> io::Result<isize> {
        let open_mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                open_mode,
                PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        if handle.is_invalid() {
            Err(io::Error::last_os_error())
        } else {
            Ok(handle.0 as isize)
        }
    }

    pub(crate) fn claim(name: &str, items: &[String], timeout: Duration) -> Claim<Server> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let deadline = Instant::now() + timeout;
        loop {
            match create(&wide, true) {
                Ok(first) => return Claim::Primary(Server { name: wide, first }),
                Err(err) if err.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                    match OpenOptions::new().read(true).write(true).open(name) {
                        Ok(mut pipe) => {
                            // Let the running instance take the foreground
                            // this launch was given.
                            let mut pid = 0u32;
                            unsafe {
                                let handle = HANDLE(pipe.as_raw_handle() as _);
                                if GetNamedPipeServerProcessId(handle, &mut pid).as_bool() {
                                    let _ = AllowSetForegroundWindow(pid);
                                }
                            }
                            return match forward(&mut pipe, items) {
                                Ok(()) => Claim::Forwarded,
                                Err(err) => {
                                    crate::warning!("single instance: handoff failed: {err}");
                                    Claim::Standalone
                                }
                            };
                        }
                        // Every instance busy, or the claimant between two:
                        // or it just died and the name is free again.
                        Err(_) => {}
                    }
                }
                Err(err) => {
                    crate::warning!("single instance: cannot create {name}: {err}");
                    return Claim::Standalone;
                }
            }
            if Instant::now() >= deadline {
                crate::warning!("single instance: the running instance does not answer");
                return Claim::Standalone;
            }
            std::thread::sleep(RETRY);
        }
    }

    impl Server {
        pub(crate) fn serve(self, mut sink: Sink) {
            let spawned = std::thread::Builder::new()
                .name("single-instance".into())
                .spawn(move || {
                    let mut current = self.first;
                    loop {
                        let connected = unsafe {
                            ConnectNamedPipe(HANDLE(current as _), std::ptr::null_mut()).as_bool()
                        } || io::Error::last_os_error().raw_os_error()
                            == Some(ERROR_PIPE_CONNECTED);
                        // The next instance exists before this one closes: a
                        // pipe without instances is a free name a launch
                        // would claim.
                        let next = create(&self.name, false);
                        let mut pipe = unsafe { File::from_raw_handle(current as _) };
                        if connected {
                            match answer(&mut pipe, &mut sink) {
                                // The reply must reach the client before the close.
                                Ok(()) => {
                                    let _ = pipe.sync_all();
                                }
                                Err(err) => crate::warning!("single instance: bad request: {err}"),
                            }
                        }
                        drop(pipe);
                        match next {
                            Ok(next) => current = next,
                            Err(err) => {
                                crate::warning!("single instance: listener stopped: {err}");
                                return;
                            }
                        }
                    }
                });
            if let Err(err) = spawned {
                crate::warning!("single instance: cannot start its listener: {err}");
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::unix::claim;
    use super::*;
    use std::sync::mpsc;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mp-si-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn serve(server: unix::Server) -> mpsc::Receiver<Vec<String>> {
        let (tx, rx) = mpsc::channel();
        server.serve(Box::new(move |items| {
            let _ = tx.send(items);
        }));
        rx
    }

    #[test]
    fn request_round_trips() {
        let items = vec!["/a/b c.txt".to_string(), "myapp://x?y=1".to_string()];
        let mut buf = Vec::new();
        write_request(&mut buf, &items).unwrap();
        assert_eq!(read_request(&mut &buf[..]).unwrap(), items);

        let mut empty = Vec::new();
        write_request(&mut empty, &[]).unwrap();
        assert_eq!(read_request(&mut &empty[..]).unwrap(), Vec::<String>::new());

        // Cut short, or not a request at all.
        assert!(read_request(&mut &buf[..buf.len() - 1]).is_err());
        assert!(read_request(&mut &b"GET / HTTP/1.1\r\n\r\n"[..]).is_err());
    }

    #[test]
    fn second_launch_forwards_its_items() {
        let dir = test_dir("forward");
        let Claim::Primary(server) = claim(&dir, "app", &[], HANDOFF_TIMEOUT) else {
            panic!("the first launch must claim");
        };
        let rx = serve(server);
        let items = vec!["/tmp/one.png".to_string(), "https://example.com/".to_string()];
        assert!(matches!(claim(&dir, "app", &items, HANDOFF_TIMEOUT), Claim::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), items);
        // A plain relaunch (nothing to open) still reaches the instance.
        assert!(matches!(claim(&dir, "app", &[], HANDOFF_TIMEOUT), Claim::Forwarded));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Vec::<String>::new());
        // Another app is another claim.
        assert!(matches!(claim(&dir, "other", &[], HANDOFF_TIMEOUT), Claim::Primary(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_socket_of_a_dead_instance_is_replaced() {
        let dir = test_dir("stale");
        // A crashed instance leaves its socket file (and lock file) behind,
        // but no lock.
        drop(std::os::unix::net::UnixListener::bind(dir.join("app.sock")).unwrap());
        std::fs::write(dir.join("app.lock"), b"").unwrap();
        assert!(dir.join("app.sock").exists());
        let Claim::Primary(server) = claim(&dir, "app", &[], Duration::from_millis(200)) else {
            panic!("a dead instance's leftovers must not block the claim");
        };
        let rx = serve(server);
        assert!(matches!(
            claim(&dir, "app", &["/x".to_string()], HANDOFF_TIMEOUT),
            Claim::Forwarded
        ));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), vec!["/x".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launch_waits_for_an_instance_that_is_still_starting() {
        let dir = test_dir("starting");
        let Claim::Primary(server) = claim(&dir, "app", &[], HANDOFF_TIMEOUT) else {
            panic!("the first launch must claim");
        };
        // The instance claimed but has no `Cx` yet: it starts accepting a
        // little later, and the waiting launch is taken then.
        let dir2 = dir.clone();
        let second = std::thread::spawn(move || {
            matches!(claim(&dir2, "app", &["/late".to_string()], HANDOFF_TIMEOUT), Claim::Forwarded)
        });
        std::thread::sleep(Duration::from_millis(300));
        let rx = serve(server);
        assert!(second.join().unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), vec!["/late".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn simultaneous_launches_elect_one_instance() {
        let dir = test_dir("race");
        let results: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || match claim(&dir, "app", &[format!("/f{i}")], HANDOFF_TIMEOUT) {
                    Claim::Primary(server) => {
                        let rx = serve(server);
                        let mut got = Vec::new();
                        while let Ok(items) = rx.recv_timeout(Duration::from_secs(2)) {
                            got.extend(items);
                        }
                        (1, got.len())
                    }
                    Claim::Forwarded => (0, 0),
                    Claim::Standalone => panic!("no launch may be left on its own"),
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        let primaries: usize = results.iter().map(|r| r.0).sum();
        let received: usize = results.iter().map(|r| r.1).sum();
        assert_eq!(primaries, 1);
        assert_eq!(received, 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instance_key_separates_builds() {
        let a = instance_key("makepad-app-image", Path::new("/a/target/release/image"));
        let b = instance_key("makepad-app-image", Path::new("/b/target/release/image"));
        assert_ne!(a, b);
        assert!(a.starts_with("makepad-app-image-"));
        assert_eq!(a, instance_key("makepad-app-image", Path::new("/a/target/release/image")));
    }
}
