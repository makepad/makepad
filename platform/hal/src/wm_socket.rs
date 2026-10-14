//! Finding a makepad window manager (Linux). A running WM publishes a per-user socket,
//! `$XDG_RUNTIME_DIR/makepad-wm-<n>`, and exports its path to everything it starts as
//! `MAKEPAD_WM_SOCKET`. A UI process that is not forced onto a backend asks that socket first:
//! a live WM that speaks the same hosted wire takes its windows (they become WM windows);
//! otherwise the process goes on to Wayland, then X11. The direct display (DRM/KMS) is never
//! picked by discovery: only the WM's own start asks for it (`MAKEPAD=linux_direct` or
//! `--linux-backend=direct`), and children never inherit that request
//! (`makepad_app_registry::child_env::scrub`).
//!
//! The socket speaks lines:
//! - client `makepad-wm hello <wire>`; server `makepad-wm ok` or `makepad-wm mismatch <wire>`
//!   (`<wire>`: the hosted protocol's fingerprint in hex). A probe ends here.
//! - then client `adopt <pid> <crate>`; server `admitted <hub port> <client id>` or
//!   `refused <reason>`: the WM made a window slot for the caller, which connects to the hub as
//!   that client like an app the WM launched itself.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// The variable a WM sets for its children: the path of its socket.
pub const SOCKET_ENV: &str = "MAKEPAD_WM_SOCKET";
const SOCKET_PREFIX: &str = "makepad-wm-";
const GREETING: &str = "makepad-wm";
/// How long either side waits for the other's line before giving up on it.
pub const LINE_TIMEOUT: Duration = Duration::from_secs(3);
/// A WM claims the first free index below this.
const MAX_INDEX: u32 = 16;

/// What asking the socket found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// No socket to ask (no path, nothing at it).
    Absent,
    /// Something is there but did not answer as a WM (stale file, refused, garbled, timeout).
    Dead(String),
    /// A WM answered, built from other hosted-protocol sources (its wire).
    Mismatch(u64),
    /// A WM answered with this process's wire.
    Live,
}

/// Where a WM admitted this process: its hub port and the client id to connect as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admission {
    pub port: u16,
    pub client: u64,
}

/// The Linux window backends, in the order discovery tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// A makepad window manager hosts the windows.
    Wm,
    /// DRM/KMS: only when forced.
    Direct,
    Wayland,
    X11,
}

impl Backend {
    fn from_name(name: &str) -> Option<Backend> {
        match name {
            "wm" => Some(Backend::Wm),
            "linux_direct" | "direct" => Some(Backend::Direct),
            "wayland" => Some(Backend::Wayland),
            "x11" => Some(Backend::X11),
            _ => None,
        }
    }
}

/// A backend the launch named explicitly, and where it was named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forced {
    pub backend: Backend,
    pub source: &'static str,
}

/// `--linux-backend=<name>` overrides everything (the WM's own launcher, a developer); without
/// it a backend token in `MAKEPAD` (`linux_direct`, `direct`, `wayland`, `x11`; `+`/`,`
/// separated with the other build tokens) forces it. Only these explicit names count.
pub fn forced(args: impl IntoIterator<Item = String>, makepad_env: Option<&str>) -> Option<Forced> {
    for arg in args {
        if let Some(name) = arg.strip_prefix("--linux-backend=") {
            if let Some(backend) = Backend::from_name(name) {
                return Some(Forced { backend, source: "forced by --linux-backend" });
            }
        }
    }
    makepad_env?
        .split(['+', ','])
        .find_map(|token| match token {
            "linux_direct" | "direct" | "wayland" | "x11" => Backend::from_name(token),
            _ => None,
        })
        .map(|backend| Forced { backend, source: "forced by MAKEPAD env" })
}

/// The backend for this process and why: a forced one; else a live WM (never for a window
/// manager itself); else Wayland when its environment is set; else X11. `probe` runs only when
/// discovery is asked for.
pub fn choose(forced: Option<Forced>, window_manager: bool, wayland_env: bool, probe: impl FnOnce() -> Probe) -> (Backend, String) {
    if let Some(forced) = forced {
        return (forced.backend, forced.source.to_string());
    }
    let wm = if window_manager {
        "this process is a window manager (no WM discovery)".to_string()
    } else {
        match probe() {
            Probe::Live => return (Backend::Wm, "a live makepad WM answered".to_string()),
            Probe::Absent => "no makepad WM".to_string(),
            Probe::Dead(why) => format!("no live makepad WM ({why})"),
            Probe::Mismatch(wire) => format!("the makepad WM speaks another hosted wire ({wire:016x})"),
        }
    };
    if wayland_env {
        (Backend::Wayland, format!("{wm}; Wayland environment is set"))
    } else {
        (Backend::X11, format!("{wm}; no Wayland environment"))
    }
}

/// The per-user directory WM sockets live in.
pub fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(PathBuf::from)
}

pub fn socket_path(dir: &Path, index: u32) -> PathBuf {
    dir.join(format!("{SOCKET_PREFIX}{index}"))
}

/// The socket a client asks: the one its WM exported, else the first of this user's.
pub fn discovery_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SOCKET_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    Some(socket_path(&runtime_dir()?, 0))
}

fn connect(path: &Path) -> Result<UnixStream, Probe> {
    if !path.exists() {
        return Err(Probe::Absent);
    }
    let stream = UnixStream::connect(path).map_err(|e| Probe::Dead(format!("{}: {e}", path.display())))?;
    stream.set_read_timeout(Some(LINE_TIMEOUT)).and_then(|_| stream.set_write_timeout(Some(LINE_TIMEOUT)))
        .map_err(|e| Probe::Dead(e.to_string()))?;
    Ok(stream)
}

fn read_line(reader: &mut BufReader<&UnixStream>) -> io::Result<String> {
    let mut line = String::new();
    // The peer is bounded by the timeout; a line longer than this is not ours.
    reader.by_ref().take(512).read_line(&mut line)?;
    if !line.ends_with('\n') {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no complete line"));
    }
    Ok(line.trim_end().to_string())
}

fn hello(stream: &UnixStream, reader: &mut BufReader<&UnixStream>, wire: u64) -> Probe {
    if let Err(e) = (&*stream).write_all(format!("{GREETING} hello {wire:016x}\n").as_bytes()) {
        return Probe::Dead(e.to_string());
    }
    match read_line(reader) {
        Ok(line) => match line.strip_prefix(GREETING).map(str::trim) {
            Some("ok") => Probe::Live,
            Some(rest) => match rest.strip_prefix("mismatch ").and_then(|w| u64::from_str_radix(w.trim(), 16).ok()) {
                Some(theirs) => Probe::Mismatch(theirs),
                None => Probe::Dead(format!("unexpected answer {line:?}")),
            },
            None => Probe::Dead(format!("not a makepad WM: {line:?}")),
        },
        Err(e) => Probe::Dead(e.to_string()),
    }
}

/// Ask the WM at `path` whether it is live and speaks `wire`. Nothing is created on its side.
pub fn probe(path: &Path, wire: u64) -> Probe {
    let stream = match connect(path) {
        Ok(stream) => stream,
        Err(probe) => return probe,
    };
    let mut reader = BufReader::new(&stream);
    hello(&stream, &mut reader, wire)
}

/// This process's answer for [`discovery_path`], asked once per process.
pub fn discovered(wire: u64) -> &'static Probe {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    PROBE.get_or_init(|| match discovery_path() {
        Some(path) => probe(&path, wire),
        None => Probe::Absent,
    })
}

/// Ask the WM at `path` to host this process: on `Ok` it has a window slot for client
/// `client` on its hub at `port`.
pub fn adopt(path: &Path, wire: u64, pid: u32, crate_name: &str) -> Result<Admission, Probe> {
    let stream = connect(path)?;
    let mut reader = BufReader::new(&stream);
    match hello(&stream, &mut reader, wire) {
        Probe::Live => {}
        other => return Err(other),
    }
    let name: String = crate_name.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    (&stream).write_all(format!("adopt {pid} {name}\n").as_bytes()).map_err(|e| Probe::Dead(e.to_string()))?;
    let line = read_line(&mut reader).map_err(|e| Probe::Dead(e.to_string()))?;
    let mut parts = line.split(' ');
    match (parts.next(), parts.next().and_then(|p| p.parse().ok()), parts.next().and_then(|c| c.parse().ok())) {
        (Some("admitted"), Some(port), Some(client)) => Ok(Admission { port, client }),
        _ => Err(Probe::Dead(line.strip_prefix("refused ").map_or_else(|| format!("unexpected answer {line:?}"), str::to_string))),
    }
}

static CLIENT_NAME: OnceLock<String> = OnceLock::new();

/// The name a process gives the WM when it asks to be hosted (the WM finds the app's label
/// and icon by it): its package. A host running someone else's code (`makepad run`) names
/// the app it runs before opening the HAL; otherwise the executable's name.
pub fn set_client_name(name: &str) {
    let _ = CLIENT_NAME.set(name.to_string());
}

pub fn client_name() -> String {
    CLIENT_NAME.get().cloned().unwrap_or_else(|| {
        std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "app".into())
    })
}

/// What a client asked once it passed the wire check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptRequest {
    pub pid: u32,
    pub crate_name: String,
}

/// A WM's published socket: removed when dropped.
pub struct WmSocket {
    pub path: PathBuf,
    pub listener: UnixListener,
}

impl WmSocket {
    /// Claim the first free `makepad-wm-<n>` in `dir` from `first` on: a path whose WM still
    /// answers is taken, a file nobody listens on is a crashed WM's and is replaced. Index 0 is
    /// the one apps find without being told ([`discovery_path`]): a WM nobody sees (a hidden test
    /// instance) starts at 1, so only its own children (`MAKEPAD_WM_SOCKET`) come to it.
    pub fn bind(dir: &Path, first: u32) -> io::Result<WmSocket> {
        for index in first..MAX_INDEX {
            let path = socket_path(dir, index);
            if path.exists() {
                match UnixStream::connect(&path) {
                    Ok(_) => continue,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => std::fs::remove_file(&path)?,
                    Err(_) => continue,
                }
            }
            match UnixListener::bind(&path) {
                Ok(listener) => return Ok(WmSocket { path, listener }),
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AddrInUse, format!("no free {SOCKET_PREFIX}<n> below {MAX_INDEX} in {}", dir.display())))
    }

    /// Serve one connection: the wire check, then (when the client asks) `admit`, which makes
    /// the window slot and answers the hub port and client id.
    pub fn serve(stream: UnixStream, wire: u64, admit: impl FnOnce(&AdoptRequest) -> Result<Admission, String>) -> io::Result<Option<AdoptRequest>> {
        stream.set_read_timeout(Some(LINE_TIMEOUT))?;
        stream.set_write_timeout(Some(LINE_TIMEOUT))?;
        let mut reader = BufReader::new(&stream);
        // A peer that hangs up unasked (another WM checking whether this path is taken).
        let line = match read_line(&mut reader) {
            Ok(line) => line,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        };
        let theirs = line.strip_prefix(GREETING).and_then(|r| r.trim().strip_prefix("hello "))
            .and_then(|w| u64::from_str_radix(w.trim(), 16).ok());
        let Some(theirs) = theirs else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("not a makepad WM client: {line:?}")));
        };
        if theirs != wire {
            (&stream).write_all(format!("{GREETING} mismatch {wire:016x}\n").as_bytes())?;
            return Ok(None);
        }
        (&stream).write_all(format!("{GREETING} ok\n").as_bytes())?;
        // A probe hangs up here.
        let line = match read_line(&mut reader) {
            Ok(line) => line,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut parts = line.splitn(3, ' ');
        let request = match (parts.next(), parts.next().and_then(|p| p.parse().ok()), parts.next()) {
            (Some("adopt"), Some(pid), Some(name)) if !name.is_empty() => AdoptRequest { pid, crate_name: name.to_string() },
            _ => {
                (&stream).write_all(b"refused malformed request\n")?;
                return Ok(None);
            }
        };
        match admit(&request) {
            Ok(a) => (&stream).write_all(format!("admitted {} {}\n", a.port, a.client).as_bytes())?,
            Err(why) => {
                let why: String = why.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                (&stream).write_all(format!("refused {why}\n").as_bytes())?;
                return Ok(None);
            }
        }
        Ok(Some(request))
    }
}

impl Drop for WmSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wm-socket-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn forced_names_only_explicit_backends() {
        let argv = args(&["app", "--linux-backend=x11"]);
        assert_eq!(forced(argv, Some("linux_direct+vulkan")), Some(Forced { backend: Backend::X11, source: "forced by --linux-backend" }));
        assert_eq!(forced(args(&["app"]), Some("linux_direct+vulkan")), Some(Forced { backend: Backend::Direct, source: "forced by MAKEPAD env" }));
        assert_eq!(forced(args(&["app"]), Some("vulkan")), None);
        assert_eq!(forced(args(&["app"]), Some("wm")), None, "the env never forces the WM");
        assert_eq!(forced(args(&["app", "--linux-backend=wm"]), None).map(|f| f.backend), Some(Backend::Wm));
        assert_eq!(forced(args(&["app", "--linux-backend=bogus"]), None), None);
    }

    #[test]
    fn order_is_forced_then_wm_then_wayland_then_x11() {
        let never = || -> Probe { panic!("forced choices never probe") };
        let direct = Some(Forced { backend: Backend::Direct, source: "forced by MAKEPAD env" });
        assert_eq!(choose(direct, true, true, never).0, Backend::Direct);
        assert_eq!(choose(direct, false, true, never).0, Backend::Direct);
        assert_eq!(choose(None, false, true, || Probe::Live).0, Backend::Wm);
        assert_eq!(choose(None, false, false, || Probe::Live).0, Backend::Wm);
        let (backend, why) = choose(None, false, true, || Probe::Dead("refused".into()));
        assert_eq!(backend, Backend::Wayland);
        assert!(why.contains("refused") && why.contains("Wayland"), "{why}");
        let (backend, why) = choose(None, false, false, || Probe::Mismatch(7));
        assert_eq!(backend, Backend::X11);
        assert!(why.contains("another hosted wire"), "{why}");
        assert_eq!(choose(None, false, false, || Probe::Absent).0, Backend::X11);
        // the WM itself never discovers itself (nor another WM)
        let (backend, why) = choose(None, true, true, never);
        assert_eq!(backend, Backend::Wayland);
        assert!(why.contains("window manager"), "{why}");
        // direct is never a default
        for probe in [Probe::Absent, Probe::Dead("x".into()), Probe::Mismatch(1)] {
            assert_ne!(choose(None, false, false, || probe.clone()).0, Backend::Direct);
        }
    }

    #[test]
    fn probe_live_dead_wrong_version_and_adoption() {
        let dir = temp_dir("probe");
        let wire = 0x1234_5678_9abc_def0;
        let socket = WmSocket::bind(&dir, 0).unwrap();
        assert_eq!(socket.path, socket_path(&dir, 0));
        // a second WM takes the next index while the first answers
        let second = WmSocket::bind(&dir, 0).unwrap();
        assert_eq!(second.path, socket_path(&dir, 1));
        drop(second);
        let path = socket.path.clone();
        let server = thread::spawn(move || {
            let mut adopted = Vec::new();
            // the second bind's check, a probe, a wrong wire, an adoption
            for _ in 0..4 {
                let (stream, _) = socket.listener.accept().unwrap();
                let got = WmSocket::serve(stream, wire, |req| {
                    assert_eq!(req.crate_name, "makepad-app-amp");
                    Ok(Admission { port: 8766, client: 1 << 32 })
                }).unwrap();
                adopted.extend(got);
            }
            adopted
        });
        assert_eq!(probe(&path, wire), Probe::Live);
        assert_eq!(probe(&path, wire ^ 1), Probe::Mismatch(wire));
        assert_eq!(adopt(&path, wire, 42, "makepad-app-amp"), Ok(Admission { port: 8766, client: 1 << 32 }));
        let adopted = server.join().unwrap();
        assert_eq!(adopted, vec![AdoptRequest { pid: 42, crate_name: "makepad-app-amp".into() }]);
        // the socket file went with its WM: absent; a stale file nobody listens on: dead
        assert_eq!(probe(&path, wire), Probe::Absent);
        let stale = UnixListener::bind(&path).unwrap();
        drop(stale);
        assert!(matches!(probe(&path, wire), Probe::Dead(_)));
        // a new WM replaces the crashed one's file
        let again = WmSocket::bind(&dir, 0).unwrap();
        assert_eq!(WmSocket::bind(&dir, 1).unwrap().path, socket_path(&dir, 1), "a hidden WM never takes index 0");
        assert_eq!(again.path, path);
        drop(again);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn a_silent_peer_is_dead_not_live() {
        let dir = temp_dir("silent");
        let path = socket_path(&dir, 0);
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = (&stream).write_all(b"HTTP/1.1 400 nope\n");
        });
        assert!(matches!(probe(&path, 1), Probe::Dead(_)));
        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
