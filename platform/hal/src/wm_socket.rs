//! Finding a makepad window manager (Linux). A running WM publishes a per-user socket,
//! `$XDG_RUNTIME_DIR/makepad-wm-<n>`, and exports its path to everything it starts as
//! `MAKEPAD_WM_SOCKET`. A UI process that is not forced onto a backend asks that socket first:
//! a live WM that speaks a common hosted-protocol version takes its windows (they become WM
//! windows); an incompatible WM stops discovery. With no live WM the process goes on to
//! Wayland, then X11. The direct display (DRM/KMS) is never
//! picked by discovery: only the WM's own start asks for it (`MAKEPAD=linux_direct` or
//! `--linux-backend=direct`), and children never inherit that request
//! (`makepad_app_registry::child_env::scrub`).
//!
//! The socket speaks lines:
//! - client `makepad-wm hello2 <min>-<max>`; server `makepad-wm ok <v>` or
//!   `makepad-wm incompatible <min>-<max>` (the server's range). [`Versions::THIS`] is this
//!   build's `pub const` range. A probe ends here.
//! - legacy client `makepad-wm hello <wire>`; server `makepad-wm ok` or
//!   `makepad-wm mismatch <wire>` (`<wire>`: the hosted protocol's fingerprint in hex).
//!   A client retries this on a fresh connection if the WM cannot answer `hello2`.
//! - then client `adopt <pid> <crate>[ lane=<name>]`; server `admitted <hub port> <client id>`
//!   or `refused <reason>`: the WM made a window slot for the caller, which connects to the hub
//!   as that client like an app the WM launched itself. `lane=` marks a lane's test instance
//!   (`MAKEPAD_TEST_LANE`, [`TEST_LANE_ENV`]): the WM badges it and never takes it for the
//!   dock's own instance of the app.
//! - or, instead of the hello, client `makepad-wm launch <app id>`; server `makepad-wm launched`
//!   or `makepad-wm refused <reason>`: the WM opens its dock app (launch-or-focus), what devhub's
//!   `show` and a shell ask for (`makepad_app_registry::machine::request_launch`).
//! - or client `makepad-wm notify <title>\t<body>[\t<action>]`; server `makepad-wm notified` or
//!   `makepad-wm refused <reason>`: a toast in the WM's notification stack (devhub's "app
//!   updated" after a sync, `makepad_app_registry::machine::request_notify`); `<action>` is what
//!   clicking it does (`relaunch=<pid>`).
//! - or client `makepad-wm activity`; server `makepad-wm activity {"input_ms":..,"seq":..,
//!   "origin":..,"what":..}`: how long ago the WM's device readers took a person's input
//!   ([`crate::user_input`]; `input_ms` null: none since the WM started). devhub's node hub
//!   asks it to tell a person at the box from work running there
//!   (`makepad_app_registry::machine::request_activity`).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// The variable a WM sets for its children: the path of its socket.
pub const SOCKET_ENV: &str = "MAKEPAD_WM_SOCKET";
/// The lane a test instance runs for (devhub's lease environment sets it for every lane run).
pub const TEST_LANE_ENV: &str = "MAKEPAD_TEST_LANE";

/// This process's test lane: `MAKEPAD_TEST_LANE`, as a name (letters, digits, `-_.`, at most 48).
pub fn test_lane() -> Option<String> {
    let lane = std::env::var(TEST_LANE_ENV).ok()?;
    let lane: String = lane.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).take(48).collect();
    (!lane.is_empty()).then_some(lane)
}
const SOCKET_PREFIX: &str = "makepad-wm-";
const GREETING: &str = "makepad-wm";
/// How long either side waits for the other's line before giving up on it.
pub const LINE_TIMEOUT: Duration = Duration::from_secs(3);
/// A WM claims the first free index below this.
const MAX_INDEX: u32 = 16;

/// The hosted-protocol versions a build speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Versions {
    /// The oldest hosted-protocol version this build still speaks.
    pub min: u32,
    /// The newest hosted-protocol version this build speaks.
    pub max: u32,
}

impl Versions {
    /// Bump `max` on any change to the hosted wire (the types in `platform/studio`);
    /// raise `min` only when dropping support for an older version.
    pub const THIS: Versions = Versions { min: 1, max: 1 };

    fn parse(range: &str) -> Option<Versions> {
        let (min, max) = range.trim().split_once('-')?;
        let versions = Versions { min: min.parse().ok()?, max: max.parse().ok()? };
        (versions.min <= versions.max).then_some(versions)
    }
}

/// What asking the socket found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// No socket to ask (no path, nothing at it).
    Absent,
    /// Something is there but did not answer as a WM (stale file, refused, garbled, timeout).
    Dead(String),
    /// A WM answered, built from other hosted-protocol sources (its wire).
    Mismatch(u64),
    /// A WM answered but shares no hosted-protocol version with this build.
    Incompatible { theirs: Versions },
    /// A WM answered with a common hosted-protocol version (or the same legacy wire).
    Live,
}

/// Where a WM admitted this process: its hub port and the client id to connect as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admission {
    pub port: u16,
    pub client: u64,
    /// The agreed hosted-protocol version.
    pub version: u32,
}

/// The Linux window backends, in the order discovery tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// A makepad window manager hosts the windows.
    Wm,
    /// A live WM answered but speaks no version this build speaks; nothing is tried after it.
    Incompatible,
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
/// discovery is asked for. A live but incompatible WM stops discovery.
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
            Probe::Mismatch(wire) => return (Backend::Incompatible, format!("the makepad WM speaks another hosted wire ({wire:016x})")),
            Probe::Incompatible { theirs } => return (Backend::Incompatible, format!("the makepad WM speaks hosted protocol {}-{}, this build {}-{}", theirs.min, theirs.max, Versions::THIS.min, Versions::THIS.max)),
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

/// Every socket a detached process may find its next WM at: the one its old WM exported first
/// (a restarted WM often claims another index, and the exported path then names a stale
/// file), then each `makepad-wm-<n>` that exists in the runtime directory.
pub fn reattach_paths() -> Vec<PathBuf> {
    let exported = std::env::var_os(SOCKET_ENV).filter(|p| !p.is_empty()).map(PathBuf::from);
    reattach_paths_in(exported, runtime_dir())
}

fn reattach_paths_in(exported: Option<PathBuf>, dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    paths.extend(exported);
    if let Some(dir) = dir {
        for index in 0..MAX_INDEX {
            let path = socket_path(&dir, index);
            if path.exists() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
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

fn hello(path: &Path, stream: &mut UnixStream, wire: u64, versions: Versions) -> (Probe, u32) {
    let answer = (&*stream).write_all(format!("{GREETING} hello2 {}-{}\n", versions.min, versions.max).as_bytes())
        .and_then(|_| read_line(&mut BufReader::new(&*stream)));
    if let Ok(line) = answer {
        if let Some(rest) = line.strip_prefix(GREETING).map(str::trim) {
            if let Some(version) = rest.strip_prefix("ok ").and_then(|v| v.parse::<u32>().ok()) {
                if version >= versions.min && version <= versions.max {
                    return (Probe::Live, version);
                }
            }
            if let Some(theirs) = rest.strip_prefix("incompatible ").and_then(Versions::parse) {
                return (Probe::Incompatible { theirs }, 0);
            }
        }
    }
    // Old WMs close an unknown hello2 without answering. Retry the wire check on a fresh socket.
    *stream = match connect(path) {
        Ok(stream) => stream,
        Err(probe) => return (probe, 0),
    };
    if let Err(e) = (&*stream).write_all(format!("{GREETING} hello {wire:016x}\n").as_bytes()) {
        return (Probe::Dead(e.to_string()), 0);
    }
    match read_line(&mut BufReader::new(&*stream)) {
        Ok(line) => match line.strip_prefix(GREETING).map(str::trim) {
            Some("ok") => (Probe::Live, Versions::THIS.max),
            Some(rest) => match rest.strip_prefix("mismatch ").and_then(|w| u64::from_str_radix(w.trim(), 16).ok()) {
                Some(theirs) => (Probe::Mismatch(theirs), 0),
                None => (Probe::Dead(format!("unexpected answer {line:?}")), 0),
            },
            None => (Probe::Dead(format!("not a makepad WM: {line:?}")), 0),
        },
        Err(e) => (Probe::Dead(e.to_string()), 0),
    }
}

/// Ask the WM at `path` whether it speaks this build's versions (or legacy `wire`).
/// Nothing is created on its side.
pub fn probe(path: &Path, wire: u64) -> Probe {
    probe_with(path, wire, Versions::THIS)
}

/// Probe with an explicit hosted-protocol range.
pub fn probe_with(path: &Path, wire: u64, versions: Versions) -> Probe {
    let mut stream = match connect(path) {
        Ok(stream) => stream,
        Err(probe) => return probe,
    };
    hello(path, &mut stream, wire, versions).0
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
    adopt_with(path, wire, Versions::THIS, pid, crate_name)
}

/// Ask to be hosted with an explicit hosted-protocol range.
pub fn adopt_with(path: &Path, wire: u64, versions: Versions, pid: u32, crate_name: &str) -> Result<Admission, Probe> {
    let mut stream = connect(path)?;
    let (probe, version) = hello(path, &mut stream, wire, versions);
    match probe {
        Probe::Live => {}
        other => return Err(other),
    }
    let mut reader = BufReader::new(&stream);
    let name: String = crate_name.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect();
    let lane = test_lane().map_or(String::new(), |lane| format!(" lane={lane}"));
    (&stream).write_all(format!("adopt {pid} {name}{lane}\n").as_bytes()).map_err(|e| Probe::Dead(e.to_string()))?;
    let line = read_line(&mut reader).map_err(|e| Probe::Dead(e.to_string()))?;
    let mut parts = line.split(' ');
    match (parts.next(), parts.next().and_then(|p| p.parse().ok()), parts.next().and_then(|c| c.parse().ok())) {
        (Some("admitted"), Some(port), Some(client)) => Ok(Admission { port, client, version }),
        _ => Err(Probe::Dead(line.strip_prefix("refused ").map_or_else(|| format!("unexpected answer {line:?}"), str::to_string))),
    }
}

static CLIENT_NAME: OnceLock<String> = OnceLock::new();

/// The name a process gives the WM when it asks to be hosted (the WM finds the app's label
/// and icon by it): its package. An app's `app_main` names its package
/// (`discover_window_manager`) and a host running someone else's code (`makepad run`) names the
/// app it runs, both before opening the HAL; only a process that does neither falls back to the
/// executable's name.
pub fn set_client_name(name: &str) {
    let _ = CLIENT_NAME.set(name.to_string());
}

pub fn client_name() -> String {
    CLIENT_NAME.get().cloned().unwrap_or_else(|| {
        std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "app".into())
    })
}

/// What a client asked once it negotiated a hosted-protocol version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdoptRequest {
    pub pid: u32,
    pub crate_name: String,
    /// The hosted-protocol version the server agreed to speak.
    pub version: u32,
    /// A lane's test instance (`lane=`): never the dock's instance of its app.
    pub test_lane: Option<String>,
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

    /// Serve one connection: the version check, then (when the client asks) `admit`, which makes
    /// the window slot and answers the hub port and client id.
    /// `launch`: a `makepad-wm launch <id>` request (the WM opens its dock app `id`).
    /// `notify`: a `makepad-wm notify <title>\t<body>[\t<action>]` request (a toast).
    pub fn serve(
        stream: UnixStream,
        wire: u64,
        admit: impl FnOnce(&AdoptRequest) -> Result<Admission, String>,
        launch: impl FnOnce(&str) -> Result<(), String>,
        notify: impl FnOnce(&str, &str, Option<&str>) -> Result<(), String>,
    ) -> io::Result<Option<AdoptRequest>> {
        Self::serve_with(stream, wire, Versions::THIS, admit, launch, notify)
    }

    /// Serve with an explicit hosted-protocol range; legacy clients still use the wire check.
    pub fn serve_with(
        stream: UnixStream,
        wire: u64,
        versions: Versions,
        admit: impl FnOnce(&AdoptRequest) -> Result<Admission, String>,
        launch: impl FnOnce(&str) -> Result<(), String>,
        notify: impl FnOnce(&str, &str, Option<&str>) -> Result<(), String>,
    ) -> io::Result<Option<AdoptRequest>> {
        stream.set_read_timeout(Some(LINE_TIMEOUT))?;
        stream.set_write_timeout(Some(LINE_TIMEOUT))?;
        let mut reader = BufReader::new(&stream);
        // A peer that hangs up unasked (another WM checking whether this path is taken).
        let line = match read_line(&mut reader) {
            Ok(line) => line,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        };
        if let Some(id) = line.strip_prefix(GREETING).and_then(|r| r.trim().strip_prefix("launch ")) {
            let id = id.trim();
            let answer = if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
                format!("{GREETING} refused not an app id\n")
            } else {
                match launch(id) {
                    Ok(()) => format!("{GREETING} launched\n"),
                    Err(why) => format!("{GREETING} refused {}\n", why.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>()),
                }
            };
            (&stream).write_all(answer.as_bytes())?;
            return Ok(None);
        }
        if line.strip_prefix(GREETING).is_some_and(|r| r.trim() == "activity") {
            (&stream).write_all(format!("{GREETING} activity {}\n", activity_json(crate::user_input::last(), crate::clock_ns())).as_bytes())?;
            return Ok(None);
        }
        if let Some(text) = line.strip_prefix(GREETING).and_then(|r| r.strip_prefix(" notify ")) {
            let mut parts = text.splitn(3, '\t');
            let (title, body, action) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().map(str::trim).filter(|a| !a.is_empty()));
            let answer = match notify(title.trim(), body.trim(), action) {
                Ok(()) => format!("{GREETING} notified\n"),
                Err(why) => format!("{GREETING} refused {}\n", why.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>()),
            };
            (&stream).write_all(answer.as_bytes())?;
            return Ok(None);
        }
        let version = if let Some(theirs) = line.strip_prefix(GREETING).and_then(|r| r.trim().strip_prefix("hello2 ")).and_then(Versions::parse) {
            let version = versions.max.min(theirs.max);
            if version < versions.min.max(theirs.min) {
                (&stream).write_all(format!("{GREETING} incompatible {}-{}\n", versions.min, versions.max).as_bytes())?;
                return Ok(None);
            }
            (&stream).write_all(format!("{GREETING} ok {version}\n").as_bytes())?;
            version
        } else {
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
            Versions::THIS.max
        };
        // A probe hangs up here.
        let line = match read_line(&mut reader) {
            Ok(line) => line,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut parts = line.split(' ');
        let request = match (parts.next(), parts.next().and_then(|p| p.parse().ok()), parts.next()) {
            (Some("adopt"), Some(pid), Some(name)) if !name.is_empty() => {
                let test_lane = parts.find_map(|w| w.strip_prefix("lane=")).filter(|l| !l.is_empty()).map(str::to_string);
                AdoptRequest { pid, crate_name: name.to_string(), version, test_lane }
            }
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

/// The `activity` answer: the last counted input, as ms before `now_ns`.
pub fn activity_json(last: Option<crate::user_input::UserInput>, now_ns: u64) -> String {
    match last {
        Some(u) => format!("{{\"input_ms\":{},\"seq\":{},\"origin\":\"{}\",\"what\":\"{}\",\"viewers\":{}}}", now_ns.saturating_sub(u.at_ns) / 1_000_000, u.seq, u.origin.as_str(), u.what.as_str(), crate::user_input::viewers()),
        None => format!("{{\"input_ms\":null,\"seq\":0,\"origin\":null,\"what\":null,\"viewers\":{}}}", crate::user_input::viewers()),
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
        assert_eq!(forced(args(&["app", "--linux-backend=incompatible"]), Some("incompatible")), None);
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
        for wayland in [false, true] {
            let (backend, why) = choose(None, false, wayland, || Probe::Mismatch(7));
            assert_eq!(backend, Backend::Incompatible);
            assert!(why.contains("another hosted wire (0000000000000007)"), "{why}");
            let (backend, why) = choose(None, false, wayland, || Probe::Incompatible { theirs: Versions { min: 3, max: 4 } });
            assert_eq!(backend, Backend::Incompatible);
            assert!(why.contains("hosted protocol 3-4") && why.contains(&format!("this build {}-{}", Versions::THIS.min, Versions::THIS.max)), "{why}");
            let fallback = if wayland { Backend::Wayland } else { Backend::X11 };
            assert_eq!(choose(None, false, wayland, || Probe::Absent).0, fallback);
            assert_eq!(choose(None, false, wayland, || Probe::Dead("refused".into())).0, fallback);
        }
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
            // the second bind's check, two probes, two legacy wire checks, an adoption
            for _ in 0..6 {
                let (stream, _) = socket.listener.accept().unwrap();
                let got = WmSocket::serve(stream, wire, |req| {
                    assert_eq!(req.crate_name, "makepad-app-amp");
                    Ok(Admission { port: 8766, client: 1 << 32, version: req.version })
                }, |_| Err("no launches here".into()), |_, _, _| Err("no toasts here".into())).unwrap();
                adopted.extend(got);
            }
            adopted
        });
        assert_eq!(probe(&path, wire), Probe::Live);
        assert_eq!(probe(&path, wire ^ 1), Probe::Live, "versioned peers do not compare wire fingerprints");
        for client_wire in [wire ^ 1, wire] {
            let stream = connect(&path).unwrap();
            let mut reader = BufReader::new(&stream);
            (&stream).write_all(format!("{GREETING} hello {client_wire:016x}\n").as_bytes()).unwrap();
            if client_wire != wire {
                assert_eq!(read_line(&mut reader).unwrap(), format!("{GREETING} mismatch {wire:016x}"));
            } else {
                assert_eq!(read_line(&mut reader).unwrap(), format!("{GREETING} ok"));
                (&stream).write_all(b"adopt 42 makepad-app-amp\n").unwrap();
                assert_eq!(read_line(&mut reader).unwrap(), format!("admitted 8766 {}", 1u64 << 32));
            }
        }
        assert_eq!(adopt(&path, wire, 42, "makepad-app-amp"), Ok(Admission { port: 8766, client: 1 << 32, version: Versions::THIS.max }));
        let adopted = server.join().unwrap();
        // `adopt` says this process's test lane (MAKEPAD_TEST_LANE: a devhub lease sets it), whatever it is
        assert_eq!(adopted, vec![
            AdoptRequest { pid: 42, crate_name: "makepad-app-amp".into(), version: Versions::THIS.max, test_lane: None },
            AdoptRequest { pid: 42, crate_name: "makepad-app-amp".into(), version: Versions::THIS.max, test_lane: test_lane() },
        ]);
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
    fn two_protocol_versions_agree_on_the_common_one() {
        let dir = temp_dir("versions");
        let one = Versions { min: 1, max: 1 };
        let two = Versions { min: 1, max: 2 };
        let newer = Versions { min: 3, max: 4 };
        for (theirs, ours, version) in [(two, two, 2), (one, two, 1), (two, one, 1), (two, newer, 0), (newer, two, 0)] {
            let socket = WmSocket::bind(&dir, 0).unwrap();
            let path = socket.path.clone();
            let server = thread::spawn(move || {
                let mut adopted = Vec::new();
                for _ in 0..2 {
                    let (stream, _) = socket.listener.accept().unwrap();
                    let got = WmSocket::serve_with(stream, 7, theirs, |request| {
                        assert_ne!(version, 0, "incompatible clients never ask for admission");
                        assert_eq!(request.version, version);
                        Ok(Admission { port: 1, client: 2, version: request.version })
                    }, |_| Err("no launches here".into()), |_, _, _| Err("no toasts here".into())).unwrap();
                    adopted.extend(got);
                }
                adopted
            });
            let expected = if version == 0 { Probe::Incompatible { theirs } } else { Probe::Live };
            assert_eq!(probe_with(&path, 8, ours), expected);
            let admission = adopt_with(&path, 8, ours, 42, "app");
            if version == 0 {
                assert_eq!(admission, Err(expected));
                assert!(server.join().unwrap().is_empty());
            } else {
                assert_eq!(admission, Ok(Admission { port: 1, client: 2, version }));
                assert_eq!(server.join().unwrap(), vec![AdoptRequest { pid: 42, crate_name: "app".into(), version, test_lane: test_lane() }]);
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn legacy_wm_without_hello2_is_still_hosted() {
        let dir = temp_dir("legacy");
        let wire = 0x1234_5678_9abc_def0;
        let socket = WmSocket::bind(&dir, 0).unwrap();
        let path = socket.path.clone();
        let server = thread::spawn(move || {
            // A fresh hello2 and legacy connection for each probe or adoption.
            for _ in 0..8 {
                let (stream, _) = socket.listener.accept().unwrap();
                stream.set_read_timeout(Some(LINE_TIMEOUT)).unwrap();
                stream.set_write_timeout(Some(LINE_TIMEOUT)).unwrap();
                let mut reader = BufReader::new(&stream);
                let line = read_line(&mut reader).unwrap();
                let Some(client_wire) = line.strip_prefix("makepad-wm hello ").and_then(|w| u64::from_str_radix(w, 16).ok()) else { continue };
                if client_wire != wire {
                    (&stream).write_all(format!("makepad-wm mismatch {wire:016x}\n").as_bytes()).unwrap();
                    continue;
                }
                (&stream).write_all(b"makepad-wm ok\n").unwrap();
                match read_line(&mut reader) {
                    Ok(line) => {
                        assert_eq!(line.split_whitespace().take(3).collect::<Vec<_>>(), vec!["adopt", "42", "app"]);
                        (&stream).write_all(b"admitted 1 2\n").unwrap();
                    }
                    Err(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
                }
            }
        });
        let versions = Versions { min: 1, max: 2 };
        assert_eq!(probe_with(&path, wire, versions), Probe::Live);
        assert_eq!(adopt_with(&path, wire, versions, 42, "app"), Ok(Admission { port: 1, client: 2, version: Versions::THIS.max }));
        assert_eq!(probe_with(&path, wire ^ 1, versions), Probe::Mismatch(wire));
        assert_eq!(adopt_with(&path, wire ^ 1, versions, 42, "app"), Err(Probe::Mismatch(wire)));
        server.join().unwrap();
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn activity_answers_the_last_device_input() {
        let _turn = crate::user_input::TEST_TURN.lock().unwrap_or_else(|e| e.into_inner());
        crate::user_input::set_viewers(0);
        assert_eq!(activity_json(None, 5), "{\"input_ms\":null,\"seq\":0,\"origin\":null,\"what\":null,\"viewers\":0}");
        let dir = temp_dir("activity");
        let socket = WmSocket::bind(&dir, 0).unwrap();
        let path = socket.path.clone();
        crate::user_input::note(crate::user_input::Origin::Device, crate::user_input::What::Key);
        let server = thread::spawn(move || {
            let (stream, _) = socket.listener.accept().unwrap();
            WmSocket::serve(stream, 7, |_| Err("no".into()), |_| Err("no".into()), |_, _, _| Err("no".into())).unwrap();
        });
        let stream = UnixStream::connect(&path).unwrap();
        (&stream).write_all(b"makepad-wm activity\n").unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        server.join().unwrap();
        let json = line.trim().strip_prefix("makepad-wm activity ").unwrap();
        assert!(json.contains("\"origin\":\"device\"") && json.contains("\"what\":\"key\""), "{json}");
        let ms: u64 = json.split("\"input_ms\":").nth(1).and_then(|r| r.split(',').next()).and_then(|n| n.parse().ok()).unwrap();
        assert!(ms < 5_000, "{json}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reattach_finds_the_live_wm_when_the_exported_path_is_stale() {
        let dir = temp_dir("reattach");
        let wire = 7;
        // the old WM sat at index 1 and left its file; the new one took index 0
        let stale = socket_path(&dir, 1);
        drop(UnixListener::bind(&stale).unwrap());
        let live = WmSocket::bind(&dir, 0).unwrap();
        assert_eq!(live.path, socket_path(&dir, 0));
        let paths = reattach_paths_in(Some(stale.clone()), Some(dir.clone()));
        assert_eq!(paths, vec![stale.clone(), socket_path(&dir, 0)]);
        let server = thread::spawn(move || {
            let (stream, _) = live.listener.accept().unwrap();
            WmSocket::serve(stream, wire, |request| Ok(Admission { port: 1, client: 2, version: request.version }), |_| Ok(()), |_, _, _| Ok(())).unwrap();
        });
        let found = paths.iter().find_map(|p| adopt(p, wire, 9, "devhub").ok());
        assert_eq!(found, Some(Admission { port: 1, client: 2, version: Versions::THIS.max }));
        server.join().unwrap();
        let _ = std::fs::remove_file(&stale);
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
