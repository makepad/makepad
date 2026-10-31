//! Spotify on Linux, through the client's own page.
//!
//! The Linux client ships no `spotify_cli`, and the public Web API takes no
//! new apps. The client is a Chromium app, though: started with
//! `--remote-debugging-port`, it opens the Chrome DevTools Protocol on
//! 127.0.0.1, and a small bridge (`spotify_cdp.js`) evaluated in its page
//! calls the client's own internal APIs — the playlist tree, playlists,
//! saved albums and artists, search, lookups, what plays and what is queued,
//! the Connect devices and the transport. Its answers have the CLI's JSON
//! shapes, so they are read by the CLI source's parsers (`spotify.rs`).
//!
//! Chromium opens the port only when started with the flag. When Spotify
//! is not running, Amp starts it with the flag itself. When it runs without
//! it, Amp says so and offers (as an entry in the device list, the one
//! choice this source can put in front of the person) to set it up: the
//! flag goes into `spotify-launcher.conf` when the client is started by
//! spotify-launcher, so every later start has it, and Spotify is restarted
//! once — asked to quit over MPRIS, never killed. Until then, and whenever
//! the bridge cannot find what it needs (a Spotify update), the source falls
//! back to MPRIS (`spotify_linux.rs`): what plays and the transport, with the
//! reason in every answer MPRIS cannot give.
//!
//! The port Amp chose is kept in its data folder (`spotify-debug-port`).
//! Nothing but 127.0.0.1 is dialled or bound. Anything running as this user
//! can use a DevTools port while it is open; that is the trade.

use super::cdp::{self, Cdp};
use super::spotify::{parse_devices, parse_library, parse_lookup, parse_now_playing, parse_playlist, parse_search, radio_station};
use super::spotify_linux::{SpotifyMpris, LOCAL_DEVICE};
use super::*;
use makepad_micro_serde::*;
use makepad_widgets::log;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The bridge evaluated in the client's page.
const BRIDGE_JS: &str = include_str!("spotify_cdp.js");
/// The client's page among the DevTools targets.
const PAGE: &str = "https://xpui.app.spotify.com/";
/// The entry in the device list that sets Spotify up for the bridge.
const ENABLE_DEVICE: &str = "amp:enable-control";
/// While the bridge is not attached, whether it can be is asked this often.
const RECHECK: Duration = Duration::from_secs(3);
/// After the bridge found the client changed, it is tried again this late.
const RECHECK_BROKEN: Duration = Duration::from_secs(60);
/// A started Spotify gets this long to open its page before the next start.
const START_GRACE: Duration = Duration::from_secs(45);
/// A quitting Spotify gets this long to go.
const QUIT_WAIT: Duration = Duration::from_secs(15);
/// Most bridge calls answer in milliseconds; a playlist of thousands of
/// tracks is read in pages inside one call.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const LONG_CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// When Amp last started Spotify: the library and the control workers each
/// have a source, and only one of them starts it.
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
/// Spotify is started unasked once per run of Amp: when it was quit later,
/// the person quit it.
static STARTED_UNASKED: AtomicBool = AtomicBool::new(false);

/// The Linux Spotify source: the bridge when it can be reached, MPRIS when
/// not. `None` when Spotify is not installed.
pub fn find() -> Option<SpotifyLinux> {
    super::spotify_linux::find().map(|mpris| SpotifyLinux { bridge: None, mpris, why: String::new(), next_attach: Instant::now(), offer: None })
}

pub struct SpotifyLinux {
    bridge: Option<Bridge>,
    mpris: SpotifyMpris,
    /// Why the bridge is not used, in words; empty while it is.
    why: String,
    next_attach: Instant,
    /// What the enable entry in the device list offers, while it is shown.
    offer: Option<String>,
}

// ---- the bridge ----

enum CallError {
    /// The connection broke: the bridge is dropped and MPRIS answers.
    Link(String),
    /// The bridge answered with an error.
    Answer(String),
}

impl CallError {
    fn text(self) -> String {
        match self {
            CallError::Link(error) | CallError::Answer(error) => error,
        }
    }
}

#[derive(DeJson, Debug)]
struct ProbeJson {
    bridge: Option<u32>,
    client: Option<String>,
    missing: Option<Vec<String>>,
    logged_in: Option<bool>,
}

#[derive(DeJson, Debug)]
struct PlaybackJson {
    now: String,
    queue: String,
}

#[derive(DeJson, Debug)]
struct SavedJson {
    items: Vec<SavedItemJson>,
}

#[derive(DeJson, Debug)]
struct SavedItemJson {
    uri: Option<String>,
    name: Option<String>,
    artists: Option<Vec<String>>,
}

struct Bridge {
    cdp: Cdp,
    /// A hash of [`BRIDGE_JS`]: a page holding another bridge gets this one.
    tag: String,
}

fn bridge_tag() -> String {
    // FNV-1a.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in BRIDGE_JS.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Bridge {
    /// Connect to the client's page on `port` and ask the bridge what it
    /// can reach.
    fn attach(port: u16) -> Result<(Bridge, ProbeJson), String> {
        let targets = cdp::targets(port)?;
        let page = targets
            .iter()
            .find(|target| target.kind.as_deref() == Some("page") && target.url.as_deref().is_some_and(|url| url.starts_with(PAGE)))
            .and_then(|target| target.web_socket.clone())
            .ok_or("Spotify's window is not open yet")?;
        let mut bridge = Bridge { cdp: Cdp::connect(port, &page)?, tag: bridge_tag() };
        bridge.inject()?;
        let probe = bridge.call("probe", "{}", CALL_TIMEOUT).map_err(CallError::text)?;
        let probe = ProbeJson::deserialize_json_lenient(&probe).map_err(|error| format!("probe: {error:?}"))?;
        Ok((bridge, probe))
    }

    fn inject(&mut self) -> Result<(), String> {
        let expression = format!("({BRIDGE_JS})({})", self.tag.serialize_json());
        self.cdp.evaluate(&expression, CALL_TIMEOUT).map(|_| ())
    }

    /// Call the bridge; `args` is a JSON object. A page that was reloaded
    /// (or holds an older bridge) gets the bridge again first.
    fn call(&mut self, name: &str, args: &str, timeout: Duration) -> Result<String, CallError> {
        for attempt in 0..2 {
            let expression = format!(
                "(async () => {{ const b = window.__makepadAmp; if (!b || b.tag !== {}) return 'nobridge:'; return await b.call({}, {}); }})()",
                self.tag.serialize_json(),
                name.to_string().serialize_json(),
                args.to_string().serialize_json()
            );
            let answer = self.cdp.evaluate(&expression, timeout).map_err(CallError::Link)?;
            if let Some(json) = answer.strip_prefix("ok:") {
                return Ok(json.to_string());
            }
            if let Some(error) = answer.strip_prefix("err:") {
                return Err(CallError::Answer(format!("Spotify: {error}")));
            }
            if attempt == 0 && answer == "nobridge:" {
                self.inject().map_err(CallError::Link)?;
                continue;
            }
            break;
        }
        Err(CallError::Answer(format!("the Spotify bridge gave no answer to {name}")))
    }
}

// ---- the client process, its port and how it is started ----

/// The Spotify client running as this user, as it was started.
struct Running {
    pid: u32,
    exe: PathBuf,
    args: Vec<String>,
    port: Option<u16>,
}

const PORT_FLAG: &str = "--remote-debugging-port=";
const ADDRESS_FLAG: &str = "--remote-debugging-address=";

fn debugging_flags(port: u16) -> [String; 2] {
    [format!("{PORT_FLAG}{port}"), format!("{ADDRESS_FLAG}127.0.0.1")]
}

fn is_debugging_flag(arg: &str) -> bool {
    arg.starts_with(PORT_FLAG) || arg.starts_with(ADDRESS_FLAG)
}

/// The client's main process (its helpers carry `--type=`), when one runs
/// as this user.
fn running_client() -> Option<Running> {
    let me = std::fs::metadata("/proc/self").ok()?.uid();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        if entry.metadata().map(|m| m.uid()).ok() != Some(me) {
            continue;
        }
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let mut parts: Vec<String> = cmdline.split(|b| *b == 0).filter(|part| !part.is_empty()).map(|part| String::from_utf8_lossy(part).into_owned()).collect();
        let Some(program) = parts.first().cloned() else { continue };
        let exe = std::fs::read_link(entry.path().join("exe")).unwrap_or_else(|_| PathBuf::from(&program));
        let is_spotify = |path: &Path| path.file_name().and_then(|name| name.to_str()) == Some("spotify");
        // Chromium rewrites its command line into one space-separated string.
        if parts.len() == 1 {
            let rest = exe.to_str().and_then(|exe| program.strip_prefix(exe)).map(str::to_string);
            let mut words: Vec<String> = rest.as_deref().unwrap_or(&program).split_whitespace().map(str::to_string).collect();
            if rest.is_none() && !words.is_empty() {
                words.remove(0);
            }
            parts = std::iter::once(exe.display().to_string()).chain(words).collect();
        }
        if !is_spotify(&exe) && !is_spotify(Path::new(&parts[0])) {
            continue;
        }
        let args: Vec<String> = parts.split_off(1);
        if args.iter().any(|arg| arg.starts_with("--type=")) {
            continue;
        }
        let port = args.iter().find_map(|arg| arg.strip_prefix(PORT_FLAG)).and_then(|port| port.parse().ok()).filter(|port| *port != 0);
        return Some(Running { pid, exe, args, port });
    }
    None
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// The port Amp asks Spotify to open: the one it chose before (kept in its
/// data folder), else a free one now.
fn chosen_port() -> u16 {
    let file = crate::worker::data_dir().map(|dir| dir.join("spotify-debug-port"));
    if let Some(port) = file.as_ref().and_then(|file| std::fs::read_to_string(file).ok()).and_then(|text| text.trim().parse::<u16>().ok()).filter(|port| *port >= 1024) {
        return port;
    }
    let port = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).and_then(|listener| listener.local_addr()).map(|addr| addr.port()).unwrap_or(9222);
    if let Some(file) = &file {
        let _ = file.parent().map(std::fs::create_dir_all);
        if let Err(error) = std::fs::write(file, format!("{port}\n")) {
            log!("amp: cannot keep the Spotify port in {}: {error}", file.display());
        }
    }
    port
}

/// The client's executable when it is installed natively.
fn installed_client() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = super::spotify_linux::client_dirs().into_iter().map(|dir| dir.join("spotify")).collect();
    places.push(PathBuf::from("/usr/bin/spotify"));
    places.into_iter().find(|path| path.is_file())
}

const FLATPAK_APP: &str = "com.spotify.Client";

fn flatpak_installed() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    Path::new("/var/lib/flatpak/app").join(FLATPAK_APP).exists() || home.join(".local/share/flatpak/app").join(FLATPAK_APP).exists()
}

fn launcher() -> Option<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).map(|dir| dir.join("spotify-launcher")).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain([PathBuf::from("/usr/bin/spotify-launcher")])
        .find(|path| path.is_file())
}

/// Start a program detached from Amp: a shell starts it in the background
/// and exits, so it belongs to init, never becomes Amp's zombie and outlives
/// Amp; its own process group keeps Amp's signals away from it.
fn start_detached(program: &Path, args: &[String]) -> Result<(), String> {
    let mut command = Command::new("/bin/sh");
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let status = command
        .arg("-c")
        .arg("\"$0\" \"$@\" </dev/null >/dev/null 2>&1 &")
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("cannot start {}: {error}", program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cannot start {}", program.display()))
    }
}

/// Start Spotify with the port open. `previous` is how the Spotify that was
/// just quit had been started: its own arguments are kept.
fn start_client(port: u16, previous: Option<&Running>) -> Result<String, String> {
    {
        let mut started = STARTED.lock().unwrap_or_else(|e| e.into_inner());
        if started.is_some_and(|at| at.elapsed() < START_GRACE) {
            return Ok("Spotify is starting".into());
        }
        *started = Some(Instant::now());
    }
    // spotify-launcher reads the flags from its configuration by itself.
    if let (Some(launcher), Some(set)) = (launcher(), launcher_config_port()) {
        if set == port {
            start_detached(&launcher, &[])?;
            return Ok(format!("started Spotify through {}", launcher.display()));
        }
    }
    let mut args: Vec<String> = match previous {
        Some(previous) => previous.args.clone(),
        None => launcher_extra_arguments(),
    };
    args.retain(|arg| !is_debugging_flag(arg));
    args.extend(debugging_flags(port));
    let native = previous.map(|previous| previous.exe.clone()).filter(|exe| exe.is_file() && !exe.starts_with("/app")).or_else(installed_client);
    if let Some(exe) = native {
        start_detached(&exe, &args)?;
        return Ok(format!("started {}", exe.display()));
    }
    if flatpak_installed() {
        let mut flatpak_args = vec!["run".to_string(), FLATPAK_APP.to_string()];
        flatpak_args.extend(args);
        start_detached(Path::new("flatpak"), &flatpak_args)?;
        return Ok("started the Spotify Flatpak".into());
    }
    if Path::new("/snap/bin/spotify").exists() {
        start_detached(Path::new("/snap/bin/spotify"), &args)?;
        return Ok("started the Spotify Snap".into());
    }
    Err("cannot find the Spotify client to start".into())
}

// ---- spotify-launcher.conf ----

/// `$XDG_CONFIG_HOME/spotify-launcher.conf`: spotify-launcher reads it in
/// place of `/etc/spotify-launcher.conf` when it is there.
fn launcher_config() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("spotify-launcher.conf"))
}

const SYSTEM_LAUNCHER_CONFIG: &str = "/etc/spotify-launcher.conf";

/// The configuration spotify-launcher uses now, as text.
fn launcher_config_text() -> String {
    launcher_config()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .or_else(|| std::fs::read_to_string(SYSTEM_LAUNCHER_CONFIG).ok())
        .unwrap_or_default()
}

/// Where `extra_arguments = [...]` sits in the `[spotify]` section: the
/// byte range of the whole assignment and its strings. `None` when absent.
fn find_extra_arguments(text: &str) -> Option<(std::ops::Range<usize>, Vec<String>)> {
    let mut section = String::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed = line.trim();
        if trimmed.starts_with('[') && !trimmed.starts_with("[[") && trimmed.ends_with(']') {
            section = trimmed.trim_matches(['[', ']']).trim().to_string();
            continue;
        }
        if section != "spotify" {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("extra_arguments") else { continue };
        if !rest.trim_start().starts_with('=') {
            continue;
        }
        // The array may run over several lines: up to its closing bracket,
        // outside strings.
        let open = start + line.find('[')?;
        let mut strings = Vec::new();
        let mut current: Option<String> = None;
        let mut escaped = false;
        for (offset, c) in text[open + 1..].char_indices() {
            match &mut current {
                Some(string) => {
                    if escaped {
                        string.push(c);
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        strings.push(current.take().unwrap_or_default());
                    } else {
                        string.push(c);
                    }
                }
                None if c == '"' => current = Some(String::new()),
                None if c == ']' => {
                    let mut end = open + 1 + offset + 1;
                    // The rest of that line (a comment) goes with it.
                    end += text[end..].find('\n').map_or(text.len() - end, |n| n + 1);
                    return Some((start..end, strings));
                }
                None => {}
            }
        }
        return None;
    }
    None
}

/// The extra arguments spotify-launcher passes to the client now.
fn launcher_extra_arguments() -> Vec<String> {
    find_extra_arguments(&launcher_config_text()).map(|(_, args)| args).unwrap_or_default()
}

/// The port spotify-launcher's configuration opens, when it has one.
fn launcher_config_port() -> Option<u16> {
    launcher_extra_arguments().iter().find_map(|arg| arg.strip_prefix(PORT_FLAG)).and_then(|port| port.parse().ok())
}

/// `text` with the debugging flags for `port` in its extra arguments,
/// everything else as it was.
fn with_debugging_flags(text: &str, port: u16) -> String {
    let quote = |arg: &str| format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""));
    let assignment = |mut args: Vec<String>| {
        args.retain(|arg| !is_debugging_flag(arg));
        args.extend(debugging_flags(port));
        format!("extra_arguments = [{}]\n", args.iter().map(|arg| quote(arg)).collect::<Vec<_>>().join(", "))
    };
    if let Some((range, args)) = find_extra_arguments(text) {
        return format!("{}{}{}", &text[..range.start], assignment(args), &text[range.end..]);
    }
    // No assignment: one goes right under the [spotify] header, or in a
    // new section at the end.
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        at += line.len();
        if line.trim() == "[spotify]" {
            let newline = if line.ends_with('\n') { "" } else { "\n" };
            return format!("{}{newline}{}{}", &text[..at], assignment(Vec::new()), &text[at..]);
        }
    }
    let gap = if text.is_empty() || text.ends_with("\n\n") { "" } else if text.ends_with('\n') { "\n" } else { "\n\n" };
    format!("{text}{gap}[spotify]\n{}", assignment(Vec::new()))
}

/// Put the debugging flags into the person's spotify-launcher.conf (made
/// from the system one when they have none, since theirs replaces it). A
/// file that was there is kept beside it as `.before-makepad-amp`.
fn write_launcher_config(port: u16) -> Result<PathBuf, String> {
    let path = launcher_config().ok_or("no configuration folder")?;
    let existing = std::fs::read_to_string(&path).ok();
    let base = existing.clone().or_else(|| std::fs::read_to_string(SYSTEM_LAUNCHER_CONFIG).ok()).unwrap_or_default();
    let text = with_debugging_flags(&base, port);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    if let Some(existing) = &existing {
        let backup = path.with_extension("conf.before-makepad-amp");
        if !backup.exists() {
            std::fs::write(&backup, existing).map_err(|error| format!("{}: {error}", backup.display()))?;
        }
    }
    let temporary = path.with_extension("conf.makepad-amp-new");
    std::fs::write(&temporary, text).map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

// ---- the source ----

const NEEDS_PORT: &str = "Spotify is running without the control port Amp reads your library through";

impl SpotifyLinux {
    /// The bridge, attaching it when the client can be reached and it is
    /// time to try. `None` means MPRIS answers, and `why` says why.
    fn link(&mut self) -> Option<&mut Bridge> {
        if self.bridge.is_some() {
            return self.bridge.as_mut();
        }
        if Instant::now() < self.next_attach {
            return None;
        }
        self.next_attach = Instant::now() + RECHECK;
        let why = match running_client() {
            Some(Running { port: Some(port), .. }) => match Bridge::attach(port) {
                Ok((bridge, probe)) => {
                    let missing = probe.missing.unwrap_or_default();
                    let client = probe.client.unwrap_or_default();
                    // Without the player the bridge is no use; other gaps
                    // fail only the calls that need them.
                    if missing.iter().any(|m| m == "Platform" || m.starts_with("PlayerAPI")) {
                        self.next_attach = Instant::now() + RECHECK_BROKEN;
                        format!("Spotify {client} has changed inside and Amp's bridge (v{}) cannot find {}; only playback control works (MPRIS)", probe.bridge.unwrap_or(0), missing.join(", "))
                    } else {
                        if !missing.is_empty() {
                            log!("amp: Spotify {client}: the bridge cannot find {}", missing.join(", "));
                        }
                        log!("amp: Spotify {client} attached on 127.0.0.1:{port} (bridge v{}, signed in: {})", probe.bridge.unwrap_or(0), probe.logged_in.unwrap_or(false));
                        self.why.clear();
                        self.offer = None;
                        self.bridge = Some(bridge);
                        return self.bridge.as_mut();
                    }
                }
                Err(error) => format!("Spotify's control port {port} does not answer yet ({error})"),
            },
            Some(Running { port: None, .. }) => {
                self.offer = Some(if launcher().is_some() {
                    "Enable full control (restarts Spotify, adds the port to spotify-launcher.conf)".into()
                } else {
                    "Enable full control (restarts Spotify with Amp's port)".into()
                });
                format!("{NEEDS_PORT}; choose \"Enable full control\" under devices")
            }
            None => {
                if !STARTED_UNASKED.swap(true, Ordering::Relaxed) {
                    match start_client(chosen_port(), None) {
                        Ok(how) => {
                            log!("amp: {how}");
                            "Spotify is starting".into()
                        }
                        Err(error) => format!("Spotify is not running ({error})"),
                    }
                } else {
                    "Spotify is not running".into()
                }
            }
        };
        if why != self.why {
            log!("amp: Spotify over MPRIS: {why}");
            self.why = why;
        }
        None
    }

    /// Run a bridge call; `None` when the bridge is not there (or the
    /// connection broke, which drops it), so MPRIS answers instead.
    fn bridged(&mut self, name: &str, args: &str, timeout: Duration) -> Option<Result<String, String>> {
        let bridge = self.link()?;
        match bridge.call(name, args, timeout) {
            Ok(answer) => Some(Ok(answer)),
            Err(CallError::Answer(error)) => Some(Err(error)),
            Err(CallError::Link(error)) => {
                log!("amp: lost Spotify's page: {error}");
                self.bridge = None;
                self.why = format!("lost Spotify's page ({error})");
                self.next_attach = Instant::now();
                None
            }
        }
    }

    fn unavailable(&self, what: &str) -> String {
        let why = if self.why.is_empty() { "Spotify cannot be reached" } else { &self.why };
        format!("{what} are not available: {why}")
    }

    /// Set Spotify up for the bridge, as the person asked: the flags into
    /// spotify-launcher's configuration when it starts the client, then one
    /// restart (a quit over MPRIS, never a kill).
    fn enable_control(&mut self) -> Result<(), String> {
        let port = launcher_config_port().unwrap_or_else(chosen_port);
        if launcher().is_some() {
            let path = write_launcher_config(port)?;
            log!("amp: added {PORT_FLAG}{port} to {}", path.display());
        }
        let previous = running_client();
        if let Some(previous) = &previous {
            if previous.port.is_none() {
                self.mpris.quit()?;
                let deadline = Instant::now() + QUIT_WAIT;
                while alive(previous.pid) {
                    if Instant::now() >= deadline {
                        return Err("Spotify did not quit; quit it yourself and Amp starts it with the control port".into());
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        // The person asked for it now: an earlier start does not hold it back.
        *STARTED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let how = start_client(port, previous.as_ref())?;
        log!("amp: {how} with the control port {port}");
        self.offer = None;
        self.why = "Spotify is restarting with the control port".into();
        self.next_attach = Instant::now() + Duration::from_secs(2);
        Ok(())
    }
}

fn saved(text: &str) -> Result<Vec<SavedItem>, String> {
    let json = SavedJson::deserialize_json_lenient(text).map_err(|error| format!("saved items: {error:?}"))?;
    Ok(json
        .items
        .into_iter()
        .filter_map(|item| Some(SavedItem { uri: item.uri?, name: item.name.unwrap_or_default(), artists: item.artists.unwrap_or_default().join(", ") }))
        .collect())
}

fn json_args(pairs: &[(&str, String)]) -> String {
    let fields: Vec<String> = pairs.iter().map(|(key, value)| format!("{}:{}", key.to_string().serialize_json(), value)).collect();
    format!("{{{}}}", fields.join(","))
}

fn json_str(text: &str) -> String {
    text.to_string().serialize_json()
}

impl MusicSource for SpotifyLinux {
    fn id(&self) -> SourceId {
        SourceId::Spotify
    }

    fn player_bundle_id(&self) -> Option<&'static str> {
        Some("com.spotify.client")
    }

    fn ready(&mut self) -> Result<bool, String> {
        match self.bridged("probe", "{}", CALL_TIMEOUT) {
            Some(Ok(probe)) => Ok(ProbeJson::deserialize_json_lenient(&probe).ok().and_then(|probe| probe.logged_in).unwrap_or(false)),
            Some(Err(error)) => Err(error),
            None => self.mpris.ready(),
        }
    }

    fn library(&mut self) -> Result<Vec<LibraryEntry>, String> {
        match self.bridged("library", "{}", CALL_TIMEOUT) {
            Some(answer) => parse_library(&answer?),
            None => Err(self.unavailable("Playlists")),
        }
    }

    fn saved_albums(&mut self) -> Result<Vec<SavedItem>, String> {
        match self.bridged("saved", &json_args(&[("kind", json_str("album"))]), LONG_CALL_TIMEOUT) {
            Some(answer) => saved(&answer?),
            None => Err(self.unavailable("Saved albums")),
        }
    }

    fn saved_artists(&mut self) -> Result<Vec<SavedItem>, String> {
        match self.bridged("saved", &json_args(&[("kind", json_str("artist"))]), LONG_CALL_TIMEOUT) {
            Some(answer) => saved(&answer?),
            None => Err(self.unavailable("Followed artists")),
        }
    }

    fn playlist(&mut self, uri: &str) -> Result<Playlist, String> {
        match self.bridged("playlist", &json_args(&[("uri", json_str(uri))]), LONG_CALL_TIMEOUT) {
            Some(answer) => parse_playlist(&answer?).map(|(playlist, _)| playlist),
            None => Err(self.unavailable("Playlists")),
        }
    }

    fn lookup(&mut self, uris: &[String]) -> Result<Vec<TrackDetail>, String> {
        let list: Vec<String> = uris.iter().take(LOOKUP_BATCH).cloned().collect();
        match self.bridged("lookup", &json_args(&[("uris", list.serialize_json())]), LONG_CALL_TIMEOUT) {
            Some(answer) => parse_lookup(&answer?),
            None => self.mpris.lookup(uris),
        }
    }

    fn search(&mut self, query: &str) -> Result<CatalogResults, String> {
        match self.bridged("search", &json_args(&[("query", json_str(query))]), CALL_TIMEOUT) {
            Some(answer) => parse_search(&answer?),
            None => Err(self.unavailable("Spotify search results")),
        }
    }

    fn playback(&mut self) -> Result<(Option<NowPlaying>, Vec<QueueEntry>), String> {
        match self.bridged("playback", "{}", CALL_TIMEOUT) {
            Some(answer) => {
                let answer = PlaybackJson::deserialize_json_lenient(&answer?).map_err(|error| format!("playback: {error:?}"))?;
                parse_now_playing(&answer.now, &answer.queue)
            }
            None => self.mpris.playback(),
        }
    }

    fn devices(&mut self) -> Result<Vec<Device>, String> {
        match self.bridged("devices", "{}", CALL_TIMEOUT) {
            Some(answer) => parse_devices(&answer?),
            None => {
                let mut devices = self.mpris.devices()?;
                if let Some(offer) = &self.offer {
                    devices.push(Device { id: ENABLE_DEVICE.into(), name: offer.clone(), kind: "Setup".into(), volume: 0.0, active: false, this_machine: false });
                }
                Ok(devices)
            }
        }
    }

    fn transport(&mut self, command: &Transport) -> Result<(), String> {
        if matches!(command, Transport::Transfer(device) if device == ENABLE_DEVICE) {
            return self.enable_control();
        }
        let (name, value) = match command {
            Transport::Play(uri) => ("play", json_str(uri)),
            Transport::Resume => ("resume", "null".into()),
            Transport::Pause => ("pause", "null".into()),
            Transport::Next => ("next", "null".into()),
            Transport::Previous => ("previous", "null".into()),
            Transport::Seek(ms) => ("seek", ms.to_string()),
            Transport::Shuffle(on) => ("shuffle", on.to_string()),
            Transport::Repeat(mode) => (
                "repeat",
                match mode {
                    Repeat::Off => "0",
                    Repeat::Context => "1",
                    Repeat::Track => "2",
                }
                .into(),
            ),
            Transport::Volume(level) => ("volume", format!("{:.4}", level.clamp(0.0, 1.0))),
            Transport::Transfer(device) => ("transfer", json_str(device)),
            Transport::Enqueue(uri) => ("enqueue", json_str(uri)),
            Transport::Dequeue(position) => ("dequeue", position.to_string()),
        };
        match self.bridged("transport", &json_args(&[("command", json_str(name)), ("value", value)]), CALL_TIMEOUT) {
            Some(answer) => answer.map(|_| ()),
            None => match command {
                Transport::Enqueue(_) | Transport::Dequeue(_) => Err(self.unavailable("The play queue's controls")),
                Transport::Transfer(device) if device != LOCAL_DEVICE => Err(self.unavailable("Other devices")),
                _ => self.mpris.transport(command),
            },
        }
    }

    /// A track's or artist's station, read like a playlist; nothing plays.
    fn radio(&mut self, seed: &str) -> Option<Result<Vec<PlaylistTrack>, String>> {
        let station = radio_station(seed)?;
        Some(match self.bridged("playlist", &json_args(&[("uri", json_str(&station))]), LONG_CALL_TIMEOUT) {
            Some(answer) => answer.and_then(|text| parse_playlist(&text)).map(|(playlist, _)| playlist.tracks),
            None => Err(self.unavailable("Radios")),
        })
    }
}
