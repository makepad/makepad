//! The setup process owns blocking work. On Windows MpTerm hosts this process
//! and all of its children; Unix bootstraps use the matching terminal menu.
//!
//! One full-screen view in the terminal's own colours: SETUP, YOUR LICENSES,
//! MAKEPAD APPS and CODING AGENTS rows, one status line where questions,
//! progress and results appear, and a key-hint footer.
use crate::{
    catalog::{self, Release},
    progress,
    rustc,
    runtime::{self, Dependency, Environment, RustChoice},
};
use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

/// App states and their status/action texts: `state|status|action`.
const MENU: &str = include_str!("../menu.txt");
/// The subtitle under the header.
const ABOUT: &str = include_str!("../about.txt");

const MAKEPAD_LICENSE_URL: &str = "https://makepad.nl/commercial-license";
const BUILD_TOOLS_LICENSE_URL: &str = "https://visualstudio.microsoft.com/license-terms/vs2022-ga-diagnosticbuildtools/";
const WINDOWS_SDK_LICENSE_URL: &str = "https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license";
const RUST_LICENSE_URL: &str = "https://www.rust-lang.org/policies/licenses";
const CUDA_LICENSE_URL: &str = "https://docs.nvidia.com/cuda/eula/";

/// Returned as an error by sub-screens when the person pressed q.
const QUIT: &str = "\u{1}quit";
/// The GPU driver notice (Windows and Linux, once per start).
const GPU_NOTICE: &str = "Makepad heavily relies on your GPU to draw its UI and implement AI functionality. Old hardware and broken drivers can cause your computer to reboot unexpectedly.";

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
fn line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut value = String::new();
    if io::stdin()
        .read_line(&mut value)
        .map_err(|e| e.to_string())?
        == 0
    {
        return Err("Input closed".into());
    }
    Ok(value.trim().to_owned())
}
#[path = "tui_view.rs"]
mod view;
pub use view::with_progress;
use view::{activity, done, text, Item, Nav, Row, Screen, View, DIM, OK, PLAIN, WARN};

struct RunningApp {
    title: String,
    child: std::process::Child,
    log: PathBuf,
}
thread_local! {
    static RUNNING_APPS: std::cell::RefCell<Vec<RunningApp>> = const { std::cell::RefCell::new(Vec::new()) };
}
// Menu polling owns child bookkeeping; no blocking waiter or per-app thread.
fn reap_apps() -> bool {
    RUNNING_APPS.with(|apps| {
        let mut changed = false;
        apps.borrow_mut().retain_mut(|app| match app.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if status.success() {
                    activity(&format!("{} closed", app.title));
                } else {
                    view::warn(&format!("{} exited {status}; see {}", app.title, app.log.display()));
                }
                changed = true;
                false
            }
            Err(error) => {
                view::warn(&format!("{}: {error}", app.title));
                changed = true;
                false
            }
        });
        changed
    })
}

enum Key {
    Left,
    Right,
    Up,
    Down,
    Enter,
    PageUp,
    PageDown,
    Home,
    End,
    #[cfg(not(windows))]
    WheelUp,
    #[cfg(not(windows))]
    WheelDown,
    /// Escape: back out of a screen or question.
    Back,
    Backspace,
    /// Ctrl-C or closed input.
    Quit,
    Char(char),
    /// No key within the poll interval: redraw, reap apps.
    Other,
}

/// A GUI-subsystem executable also serves as its ConPTY setup child.
pub fn attach_parent_console() {
    #[cfg(windows)] unsafe {
        #[link(name = "kernel32")]
        unsafe extern "system" { fn AttachConsole(pid: u32) -> i32; }
        let _ = AttachConsole(u32::MAX);
    }
}

#[cfg(windows)]
mod console {
    use super::*;
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Info {
        size: Coord,
        cursor: Coord,
        attrs: u16,
        window: Rect,
        maximum: Coord,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: *mut c_void, info: *mut Info) -> i32;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, timeout: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    unsafe extern "C" {
        fn _getwch() -> u16;
        fn _kbhit() -> i32;
    }
    pub fn enable() {
        unsafe {
            let h = GetStdHandle(-11i32 as u32);
            let mut mode = 0;
            if GetConsoleMode(h, &mut mode) != 0 {
                SetConsoleMode(h, mode | 4);
            }
            SetConsoleOutputCP(65001);
        }
    }
    pub fn size() -> (usize, usize) {
        unsafe {
            let mut info = Info::default();
            if GetConsoleScreenBufferInfo(GetStdHandle(-11i32 as u32), &mut info) != 0 {
                (
                    (info.window.right - info.window.left + 1).max(1) as usize,
                    (info.window.bottom - info.window.top + 1).max(1) as usize,
                )
            } else {
                (100, 32)
            }
        }
    }
    pub fn alive(pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(0x00100000, 0, pid);
            if h.is_null() {
                return false;
            }
            let active = WaitForSingleObject(h, 0) == 258;
            CloseHandle(h);
            active
        }
    }
    pub struct Input;
    impl Input {
        pub fn enter() -> Result<Self, String> {
            Ok(Self)
        }
    }
    pub fn key() -> Result<Key, String> {
        // Returning periodically lets the caller redraw after a ConPTY resize.
        for _ in 0..10 {
            if unsafe { _kbhit() } != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if unsafe { _kbhit() } == 0 {
            return Ok(Key::Other);
        }
        let value = unsafe { _getwch() };
        Ok(match value {
            0 | 224 => match unsafe { _getwch() } {
                72 => Key::Up,
                80 => Key::Down,
                75 => Key::Left,
                77 => Key::Right,
                73 => Key::PageUp,
                81 => Key::PageDown,
                71 => Key::Home,
                79 => Key::End,
                68 => Key::Quit,
                _ => Key::Other,
            },
            13 => Key::Enter,
            9 => Key::Right,
            8 => Key::Backspace,
            27 => Key::Back,
            3 => Key::Quit,
            _ => char::from_u32(value as u32)
                .map(Key::Char)
                .unwrap_or(Key::Other),
        })
    }
}
#[cfg(not(windows))]
mod console {
    use super::*;
    pub fn enable() {}
    #[repr(C)]
    #[derive(Default)]
    struct Size {
        rows: u16,
        cols: u16,
        x: u16,
        y: u16,
    }
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    #[cfg(target_os = "macos")]
    type NFds = u32;
    #[cfg(not(target_os = "macos"))]
    type NFds = std::ffi::c_ulong;
    unsafe extern "C" {
        fn ioctl(fd: i32, request: usize, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
        fn poll(fds: *mut PollFd, count: NFds, timeout: i32) -> i32;
        fn read(fd: i32, buffer: *mut u8, count: usize) -> isize;
    }
    pub fn size() -> (usize, usize) {
        let mut size = Size::default();
        let request = if cfg!(target_os = "macos") {
            0x40087468
        } else {
            0x5413
        };
        if unsafe { ioctl(0, request, &mut size) } == 0 && size.cols > 0 && size.rows > 0 {
            (size.cols as usize, size.rows as usize)
        } else {
            (100, 32)
        }
    }
    pub fn alive(pid: u32) -> bool {
        unsafe { kill(pid as i32, 0) == 0 }
    }
    pub struct Input(String);
    impl Input {
        pub fn enter() -> Result<Self, String> {
            let saved = Command::new("stty")
                .arg("-g")
                .stdin(std::process::Stdio::inherit())
                .output()
                .map_err(|e| e.to_string())?;
            let saved = String::from_utf8_lossy(&saved.stdout).trim().to_owned();
            if !saved.is_empty()
                && Command::new("stty")
                    .args(["-icanon", "-echo", "-isig", "min", "1", "time", "0"])
                    .status()
                    .map_err(|e| e.to_string())?
                    .success()
            {
                // Report mouse input only while a menu is consuming it with
                // echo disabled, never during builds or while an app runs.
                print!("\x1b[?1000h\x1b[?1006h");
                let _ = io::stdout().flush();
                Ok(Self(saved))
            } else {
                Err("Cannot read terminal keys".into())
            }
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            print!("\x1b[?1000l\x1b[?1006l");
            let _ = io::stdout().flush();
            let _ = Command::new("stty").arg(&self.0).status();
        }
    }
    /// Input is waiting on the terminal within `ms` milliseconds.
    fn waiting(ms: i32) -> bool {
        let mut fd = PollFd { fd: 0, events: 1, revents: 0 };
        unsafe { poll(&mut fd, 1, ms) > 0 }
    }
    fn byte() -> Option<u8> {
        let mut value = 0u8;
        (unsafe { read(0, &mut value, 1) } == 1).then_some(value)
    }
    pub fn key() -> Result<Key, String> {
        // Poll, so a quiet terminal returns Other for redraws and child
        // reaping; a readable terminal that yields nothing is closed input.
        if !waiting(200) {
            return Ok(Key::Other);
        }
        let Some(first) = byte() else {
            // Closed input is never an answer: leave instead of spinning or
            // accepting a default.
            return Ok(Key::Quit);
        };
        Ok(match first {
            b'\r' | b'\n' => Key::Enter,
            9 => Key::Right,
            3 => Key::Quit,
            8 | 127 => Key::Backspace,
            27 => {
                let mut sequence = Vec::new();
                while sequence.len() < 32 && waiting(30) {
                    let Some(value) = byte() else { break };
                    sequence.push(value);
                    let introducer = sequence.len() == 1 && (value == b'[' || value == b'O');
                    if !introducer && (value.is_ascii_alphabetic() || value == b'~') {
                        break;
                    }
                }
                match sequence.as_slice() {
                    b"" => Key::Back,
                    b"[A" | b"OA" => Key::Up,
                    b"[B" | b"OB" => Key::Down,
                    b"[C" | b"OC" => Key::Right,
                    b"[D" | b"OD" => Key::Left,
                    b"[5~" => Key::PageUp,
                    b"[6~" => Key::PageDown,
                    b"[H" | b"OH" | b"[1~" | b"[7~" => Key::Home,
                    b"[F" | b"OF" | b"[4~" | b"[8~" => Key::End,
                    s if s.starts_with(b"[<64;") && s.ends_with(b"M") => Key::WheelUp,
                    s if s.starts_with(b"[<65;") && s.ends_with(b"M") => Key::WheelDown,
                    b"[21~" => Key::Quit,
                    _ => Key::Other,
                }
            }
            c if c.is_ascii() => Key::Char(c as char),
            _ => Key::Other,
        })
    }
}

/// Open a link in the default browser without a console window.
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(window: *mut std::ffi::c_void, operation: *const u16, file: *const u16, parameters: *const u16, directory: *const u16, show: i32) -> isize;
        }
        let wide = |s: &str| std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let (operation, file) = (wide("open"), wide(url));
        let result = unsafe { ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
        if result > 32 { Ok(()) } else { Err(format!("Could not open {url} (error {result})")) }
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open {url}: {e}"))
    }
}

/// An executable of this name is on PATH (with a PATHEXT extension on Windows).
fn on_path(name: &str) -> bool {
    let Some(path) = env::var_os("PATH") else { return false };
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').filter(|e| !e.is_empty()).map(str::to_lowercase).collect()
    } else {
        vec![String::new()]
    };
    env::split_paths(&path).any(|directory| extensions.iter().any(|extension| directory.join(format!("{name}{extension}")).is_file()))
}

/// Local (year, month, day, hour, minute).
fn local_time() -> (i32, u32, u32, u32, u32) {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" { fn GetLocalTime(time: *mut [u16; 8]); }
        let mut t = [0u16; 8];
        unsafe { GetLocalTime(&mut t) };
        (t[0] as i32, t[1] as u32, t[3] as u32, t[4] as u32, t[5] as u32)
    }
    #[cfg(not(windows))]
    {
        unsafe extern "C" {
            fn time(out: *mut i64) -> i64;
            fn localtime_r(time: *const i64, out: *mut [i64; 8]) -> *mut [i64; 8];
        }
        // struct tm starts with int sec, min, hour, mday, mon, year.
        let mut tm = [0i64; 8];
        let now = unsafe { time(std::ptr::null_mut()) };
        if unsafe { localtime_r(&now, &mut tm) }.is_null() {
            return (1970, 1, 1, 0, 0);
        }
        let f = unsafe { std::slice::from_raw_parts(tm.as_ptr() as *const i32, 6) };
        (f[5] + 1900, f[4] as u32 + 1, f[3] as u32, f[2] as u32, f[1] as u32)
    }
}

fn tree_size(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::read_dir(path).map(|entries| entries.flatten().map(|e| tree_size(&e.path())).sum()).unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}
/// (everything this folder uses, the build data "clean build" deletes: target/ only).
fn measure(root: &Path) -> (u64, u64) {
    (tree_size(root), tree_size(&root.join("target")))
}
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1073741824.)
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    New,
    Partial,
    Compile,
    Update,
    Ready,
    Merge,
}
impl State {
    fn key(self) -> &'static str {
        match self {
            State::New => "new",
            State::Partial => "partial",
            State::Compile => "compile",
            State::Update => "update",
            State::Ready => "ready",
            State::Merge => "merge",
        }
    }
    /// Status and action texts from menu.txt.
    /// The status is the action for every state but ready and merge; those
    /// rows get only a trailing ⏎ when selected (action "⏎"). `bytes` is
    /// the download size when known.
    fn texts(self, bytes: Option<u64>) -> (view::Text, String) {
        let (status, action) = MENU
            .lines()
            .filter_map(|line| line.split_once('|'))
            .find(|(key, _)| *key == self.key())
            .and_then(|(_, rest)| rest.split_once('|'))
            .map(|(s, a)| (s.to_owned(), a.to_owned()))
            .unwrap_or_else(|| (self.key().to_owned(), "run".to_owned()));
        let status = match bytes {
            Some(bytes) => status.replace("{mb}", &bytes.div_ceil(1048576).to_string()),
            None => status.replace(" {mb} MB", ""),
        };
        let action = if action.is_empty() { "⏎".to_owned() } else { action };
        let status = match self {
            State::New => text(status, DIM),
            State::Ready => done(status),
            State::Compile => text(status, PLAIN),
            _ => text(status, WARN),
        };
        (status, action)
    }
}

fn item(id: impl Into<String>, name: impl Into<String>, license: &'static str, status: view::Text, action: impl Into<String>) -> Row {
    Row::Item(Item { id: id.into(), name: name.into(), license, status, action: action.into() })
}
fn short_path(path: &Path) -> String {
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    match home.and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rest) if !cfg!(windows) => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}
/// The agreements involved on this platform: (id, name, URL).
fn agreements() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut list = vec![("makepad", "Makepad commercial license", MAKEPAD_LICENSE_URL)];
    if cfg!(windows) {
        list.push(("vs", "Microsoft Visual Studio Build Tools", BUILD_TOOLS_LICENSE_URL));
        list.push(("sdk", "Microsoft Windows SDK", WINDOWS_SDK_LICENSE_URL));
    }
    list.push(("rust", "Rust", RUST_LICENSE_URL));
    if crate::cuda::supported() {
        list.push(("cuda", "NVIDIA CUDA Toolkit", CUDA_LICENSE_URL));
    }
    list
}
/// Agreement rows: full name and host; Return opens the link.
fn agreement_rows(ids: Option<&[&str]>) -> Vec<Row> {
    agreements()
        .into_iter()
        .filter(|(id, _, _)| ids.is_none_or(|ids| ids.contains(id)))
        .map(|(id, name, url)| item(format!("url:{id}"), format!("{name:<36}"), "", text(host(url), DIM), "open in browser"))
        .collect()
}
/// Return on an agreement row: open it and confirm on the status line.
fn open_agreement(id: &str) {
    let Some((_, _, url)) = agreements().into_iter().find(|(key, _, _)| Some(*key) == id.strip_prefix("url:")) else { return };
    match open_url(url) {
        Ok(()) => view::message(done(format!("Opened {} in your browser.", url.trim_start_matches("https://")))),
        Err(error) => view::warn(&error),
    }
}
fn host(url: &str) -> &str {
    url.split_once("://").map_or(url, |(_, rest)| rest.split('/').next().unwrap_or(rest))
}
/// Makepad WM, then every registry app marked `menu: other`.
fn free_apps() -> Result<Vec<(String, String)>, String> {
    let mut list = Vec::new();
    let mut wm = None;
    for app in catalog::apps()? {
        let field = |name| app.get(name).and_then(makepad_strict_json::Value::as_str).map(str::to_owned);
        let (Some(id), Some(title)) = (field("id"), field("title")) else { return Err("Invalid app registry".into()) };
        match field("menu").as_deref() {
            Some("other") => list.push((id, title)),
            _ if id == "wm" => wm = Some((id, title)),
            _ => {}
        }
    }
    list.splice(0..0, wm);
    Ok(list)
}
fn is_public(app: &str) -> bool {
    catalog::apps().is_ok_and(|apps| apps.iter().any(|a| a.get("id").and_then(makepad_strict_json::Value::as_str) == Some(app)))
}

/// Last validation of a recorded installed Rust (macOS/Linux). Redraws only
/// confirm the binaries still exist; builds re-validate through `selected_rust`.
#[derive(Clone)]
struct RustCheck {
    recorded: String,
    version: String,
    sysroot: Option<PathBuf>,
}
#[derive(Clone)]
struct Setup {
    root: PathBuf,
    project: PathBuf,
    service: String,
    email: String,
    app: String,
    release: Option<Release>,
    cuda: bool,
    compiler_retry: bool,
    rust_check: std::cell::RefCell<Option<RustCheck>>,
    /// Licensed releases from the last catalog check; None before one succeeded.
    licenses: Option<Vec<Release>>,
    license_error: Option<String>,
    /// The newest public Makepad release known (for the free apps).
    public: Option<Release>,
    checked: Option<String>,
    disk: Arc<Mutex<(u64, Option<(u64, u64)>)>>,
    disk_seen: std::cell::Cell<u64>,
    /// Installed coding agents: (command, title).
    agents: Vec<(&'static str, &'static str)>,
    free_selected: usize,
    /// The GPU driver notice was acknowledged in this session.
    gpu_read: bool,
}
impl Setup {
    fn rust_ready(&self, version: &str) -> bool {
        // Windows never reads the choice file: private pinned Rust only.
        if cfg!(windows) {
            return runtime::rust_ready(&self.root, version);
        }
        let recorded = fs::read_to_string(self.root.join("selected-rust")).unwrap_or_default().trim().to_owned();
        if !recorded.starts_with('/') {
            self.rust_check.borrow_mut().take();
            return runtime::rust_ready(&self.root, version);
        }
        let mut cache = self.rust_check.borrow_mut();
        if let Some(check) = cache.as_ref().filter(|c| c.recorded == recorded && c.version == version) {
            return check.sysroot.as_ref().is_some_and(|s| {
                s.join("bin").join(runtime::exe("cargo")).is_file() && s.join("bin").join(runtime::exe("rustc")).is_file()
            });
        }
        let sysroot = runtime::validate_rust(version, &recorded).ok();
        let ready = sysroot.is_some();
        *cache = Some(RustCheck { recorded, version: version.to_owned(), sysroot });
        ready
    }
    /// macOS/Linux only: repair a recorded installed Rust that stopped
    /// validating, or make the same one-time offer as the bootstrap. Every
    /// change of the recorded choice is an explicit answer; backing out keeps
    /// the private default. Returns true when the record changed. Windows
    /// never reaches this.
    fn choose_rust(&self, version: &str) -> Result<bool, String> {
        let stale = match runtime::rust_choice(&self.root)? {
            RustChoice::Private => return Ok(false),
            RustChoice::External(recorded) => match runtime::validate_rust(version, &recorded) {
                Ok(_) => return Ok(false),
                Err(reason) => Some((recorded, reason)),
            },
            RustChoice::Undecided => None,
        };
        if let Some((recorded, reason)) = &stale {
            activity(&format!("The selected Rust at {recorded} cannot be used: {reason}"));
        }
        match runtime::probe_rust(version) {
            Ok(candidate) => {
                let sysroot = candidate.to_string_lossy().into_owned();
                let question = format!("Use your installed Rust, or a private Rust {version} in this folder?");
                let note = format!("installed: {} (not modified)", short_path(&candidate));
                let choice = match view::choose(&question, &note, &["private", "installed"], 0)?.as_deref() {
                    Some("installed") => {
                        activity("Using the installed Rust.");
                        RustChoice::External(sysroot)
                    }
                    Some(_) => {
                        activity("Using a private Rust in this folder.");
                        RustChoice::Private
                    }
                    None => return Ok(false),
                };
                runtime::record_rust_choice(&self.root, &choice)?;
                Ok(true)
            }
            Err(reason) => {
                let Some((recorded, _)) = stale else {
                    // Nothing to offer and nothing recorded: the private flow
                    // continues and the offer returns once a compatible Rust exists.
                    activity(&format!("Installed Rust not used: {reason}"));
                    return Ok(false);
                };
                let question = format!("The selected Rust at {recorded} cannot be used. Switch to a private Rust in this folder?");
                if view::choose(&question, "", &["switch", "keep"], 0)?.as_deref() == Some("switch") {
                    runtime::record_rust_choice(&self.root, &RustChoice::Private)?;
                    activity("Switching to a private Rust in this folder.");
                    Ok(true)
                } else {
                    Err(format!("Kept the selected Rust at {recorded}. Make it available again, or choose Rust to switch."))
                }
            }
        }
    }
    fn release(&mut self) -> Result<Release, String> {
        if let Some(release) = &self.release {
            return Ok(release.clone());
        }
        self.refresh()
    }
    fn ask_email(&mut self) -> Result<bool, String> {
        let Some(value) = view::edit("Email you bought a Makepad app with:", "", "⏎ log in   esc back")? else { return Ok(false) };
        if value.is_empty() {
            return Ok(false);
        }
        self.email = catalog::email(&value).map_err(|_| "That does not look like an email address.".to_owned())?;
        self.save_email();
        Ok(true)
    }
    /// Keep a switched email for the next start, in the folder's own bootstrap file.
    fn save_email(&self) {
        let app = makepad_loader_bundle::load_from(&self.root).ok().flatten().map_or_else(|| self.app.clone(), |b| b.app);
        match makepad_loader_bundle::Bootstrap::new(&self.email, &app) {
            Ok(bootstrap) => {
                if let Err(error) = fs::write(self.root.join(makepad_loader_bundle::BOOTSTRAP_FILE), bootstrap.encode()) {
                    activity(&format!("Email not saved for the next start: {error}"));
                }
            }
            Err(error) => activity(&format!("Email not saved for the next start: {error}")),
        }
    }
    fn refresh(&mut self) -> Result<Release, String> {
        let public = is_public(&self.app);
        if !public && self.email.is_empty() && !self.ask_email()? {
            return Err("Log in with the email that has this app's license.".into());
        }
        let release = with_progress(|| {
            progress::stage(
                "Checking sources",
                "The source service may refresh its Git cache while we wait",
                0.0,
            );
            if !public {
                catalog::fetch(&self.service, &self.email)?.into_iter().find(|r| r.id == self.app).ok_or("This app is not available for this email".into())
            } else {
                let app = catalog::apps()?.into_iter().find(|a| a.get("id").and_then(makepad_strict_json::Value::as_str) == Some(&self.app)).ok_or("Unknown public app")?;
                catalog::fetch_public(&self.service)?.for_app(&app)
            }
        })?;
        if !release.supported() {
            return Err("No release for this platform yet".into());
        }
        fs::create_dir_all(self.root.join("available")).map_err(|e| e.to_string())?;
        release.save(&self.root.join("available").join(format!("{}.json", self.app)))?;
        release.save(&self.root.join("latest.json"))?;
        self.release = Some(release.clone());
        Ok(release)
    }
    /// The licensed apps for this email, from the catalog. Their releases
    /// are cached in `available/` for launches.
    fn refresh_licenses(&mut self) -> Result<(), String> {
        if self.email.is_empty() {
            self.licenses = Some(Vec::new());
            return Ok(());
        }
        self.licenses = None;
        self.license_error = None;
        view::set_view(self.main_view());
        view::busy(&format!("Checking licenses for {}", self.email));
        let releases = match with_progress(|| catalog::fetch(&self.service, &self.email)) {
            Ok(releases) => releases,
            Err(error) => {
                self.license_error = Some(error.clone());
                return Err(error);
            }
        };
        let licensed: Vec<Release> = releases.into_iter().filter(|r| !r.public).collect();
        fs::create_dir_all(self.root.join("available")).map_err(|e| e.to_string())?;
        for release in &licensed {
            release.save(&self.root.join("available").join(format!("{}.json", release.id)))?;
        }
        self.release = licensed.iter().find(|r| r.id == self.app).cloned();
        if let Some(release) = &self.release {
            release.save(&self.root.join("latest.json"))?;
        }
        self.licenses = Some(licensed);
        Ok(())
    }
    fn licensed(&self, app: &str) -> bool {
        self.licenses.as_ref().is_some_and(|list| list.iter().any(|r| r.id == app))
    }
    /// The release an app's row refers to, without writing anything:
    /// cached, installed, or projected from the known public release.
    fn app_release(&self, app: &str) -> Option<Release> {
        if let Some(release) = load_release(&self.root.join("available").join(format!("{app}.json")))
            .or_else(|| load_release(&self.root.join("installed").join(format!("{app}.json"))))
        {
            return Some(release);
        }
        if app == self.app {
            return self.release.clone();
        }
        let definition = catalog::apps().ok()?.into_iter().find(|entry| entry.get("id").and_then(makepad_strict_json::Value::as_str) == Some(app))?;
        self.public.as_ref().or(self.release.as_ref())?.for_app(&definition).ok()
    }
    fn app_state(&self, app: &str, release: Option<&Release>) -> State {
        if self.root.join("changes").join(format!("{app}.merge")).is_file() {
            return State::Merge;
        }
        let Some(release) = release else { return State::New };
        let installed = load_release(&self.root.join("installed").join(format!("{app}.json")));
        let binary = self.root.join(if cfg!(windows) { runtime::exe(&release.binary) } else { format!("{}.bin", release.binary) });
        if installed.as_ref().is_some_and(|i| i.release == release.release) && binary.is_file() {
            State::Ready
        } else if release.installed(&self.root) {
            State::Compile
        } else if installed.is_some() {
            State::Update
        } else if fs::read_dir(release.directory(&self.root).join(".builder-repositories")).is_ok_and(|mut d| d.next().is_some()) {
            State::Partial
        } else {
            State::New
        }
    }
    fn app_setup(&self, app: &str) -> Result<Self, String> {
        let mut selected = self.clone();
        selected.app = app.into();
        selected.release = self.app_release(app);
        if let Some(release) = &selected.release {
            let available = self.root.join("available");
            fs::create_dir_all(&available).map_err(|e| e.to_string())?;
            release.save(&available.join(format!("{app}.json")))?;
        }
        Ok(selected)
    }
    /// A row's action: merge saved changes, run a built app, or download,
    /// compile and run it.
    fn open_app(&mut self, app: &str) -> Result<(), String> {
        let release = self.app_release(app);
        if self.app_state(app, release.as_ref()) == State::Merge {
            return self.merge_changes(app);
        }
        self.run_app(app)
    }
    fn run_app(&mut self, app: &str) -> Result<(), String> {
        let mut selected = self.app_setup(app)?;
        let state = self.app_state(app, selected.release.as_ref());
        fs::write(self.root.join("selected-app"), app).map_err(|e| e.to_string())?;
        view::working(&format!("app:{app}"), match state {
            State::Ready => "opening…",
            State::Compile => "compiling…",
            _ => "downloading…",
        });
        let result = if state == State::Ready {
            selected.open().map(|title| launched(&title))
        } else {
            selected.install_and_run()
        };
        fs::write(self.root.join("selected-app"), &self.app).map_err(|e| e.to_string())?;
        self.compiler_retry = selected.compiler_retry;
        self.cuda = selected.cuda;
        if selected.public.is_some() {
            self.public = selected.public.clone();
        }
        if self.email.is_empty() && !selected.email.is_empty() {
            self.email = selected.email.clone();
        }
        if app == self.app {
            self.release = selected.release.clone();
        }
        self.measure_disk();
        result
    }
    /// The release that pins the compiler: the primary app's, else the public one.
    fn compiler_release(&mut self) -> Result<Release, String> {
        if let Some(release) = self.release.clone().or_else(|| self.public.clone()) {
            return Ok(release);
        }
        // Small and silent: this can run before any menu is drawn.
        let public = catalog::fetch_public(&self.service)?;
        self.public = Some(public.clone());
        Ok(public)
    }
    fn pinned_rust(&self) -> Option<String> {
        self.release.as_ref().or(self.public.as_ref()).map(|r| r.rust.clone())
    }
    fn ready(&self) -> [bool; 3] {
        let tools = if cfg!(windows) {
            crate::msvc::ready(&self.root.join("toolchain/msvc"))
        } else {
            runtime::system_tools_ready().is_ok()
        };
        let rust = self.pinned_rust().is_some_and(|version| self.rust_ready(&version));
        [
            tools,
            rust,
            self.release
                .as_ref()
                .is_some_and(|r| r.installed(&self.root)),
        ]
    }
    /// One consent screen installs every missing compiler piece and accepts
    /// its licenses.
    fn install_compiler(&mut self, release: &Release) -> Result<bool, String> {
        if self.compiler_retry {
            activity("Retrying the staged Rust compiler check; no download is needed.");
            view::busy("Checking the staged Rust compiler");
            return match rustc::retry_staged(&runtime::rust_dir(&self.root, &release.rust), &release.rust) {
                Ok(()) => {
                    self.compiler_retry = false;
                    activity("Compiler is ready.");
                    Ok(true)
                }
                Err(error) if rustc::needs_compiler_retry(&error) => Err(
                    "Windows security software is still scanning the staged Rust compiler. Select Build tools to check again.".into()
                ),
                Err(error) => {
                    self.compiler_retry = false;
                    Err(error)
                }
            };
        }
        let mut ready = self.ready();
        if !cfg!(windows) && !ready[1] && self.choose_rust(&release.rust)? {
            ready = self.ready();
        }
        if ready[0] && ready[1] {
            activity("Compiler is ready.");
            return Ok(true);
        }
        // One consent screen covers every missing piece and its licenses.
        let mut parts = Vec::new();
        if cfg!(windows) && !ready[0] {
            parts.push("Microsoft C++ Build Tools, Windows SDK".to_owned());
        }
        if !ready[1] {
            parts.push(format!("Rust {}", release.rust));
        }
        let intro = if !cfg!(windows) && !ready[0] {
            if cfg!(target_os = "macos") {
                "Makepad compiles from source, so it needs a compiler. Apple's developer tools come from Apple's installer, which shows its own license."
            } else {
                "Makepad compiles from source, so it needs a compiler. Development packages come from your distribution's package manager."
            }
        } else {
            "Makepad compiles from source, so it needs a compiler."
        };
        let detail = if parts.is_empty() { String::new() } else { format!("Installed in this folder only: {}", parts.join(", ")) };
        let ids: &[&str] = if cfg!(windows) { &["makepad", "vs", "sdk", "rust"] } else { &["makepad", "rust"] };
        let action = if !ready[0] { "install the build tools and Rust" } else { "install Rust" };
        if !self.consent("Install build tools", intro, &detail, ids, action)? {
            activity("Compiler installation cancelled.");
            view::message(text("Nothing was installed.", DIM));
            return Ok(false);
        }
        view::set_view(self.main_view());
        view::working(if cfg!(windows) { "tools" } else { "rust" }, "installing…");
        if !cfg!(windows) && !ready[0] {
            if cfg!(target_os = "macos") {
                // Apple's tools come right after the agreements.
                if !self.xcode_screen()? {
                    view::set_view(self.main_view());
                    view::message(text("Nothing was installed.", DIM));
                    return Ok(false);
                }
                view::set_view(self.main_view());
                view::working("rust", "installing…");
            } else {
                let _pause = Screen::pause();
                runtime::setup_system_tools()?;
            }
        }
        let result: Result<bool, String> = with_progress(|| {
            if cfg!(windows) && !ready[0] {
                runtime::dependency(&self.root, release, if cfg!(windows) { Dependency::Msvc } else { Dependency::System })?;
            }
            if !ready[1] {
                runtime::dependency(&self.root, release, Dependency::Rust)?;
            }
            Ok(true)
        });
        if let Err(error) = &result {
            if rustc::needs_compiler_retry(error) {
                self.compiler_retry = true;
                return Err(
                    "Windows security software is still scanning the staged Rust compiler. Select Build tools to check again.".into()
                );
            }
        }
        result
    }
    /// The Build tools row (Windows) or Rust row (macOS/Linux).
    fn setup_tools(&mut self) -> Result<(), String> {
        let ready = self.ready();
        if ready[0] && ready[1] && !self.compiler_retry {
            if cfg!(windows) {
                view::message(done("Build tools, Windows SDK and Rust are ready."));
                return Ok(());
            }
            return self.change_rust();
        }
        let release = self.compiler_release()?;
        if self.install_compiler(&release)? {
            self.measure_disk();
            view::message(if cfg!(windows) {
                done(format!("Build tools and Rust {} installed; nothing outside this folder changed.", release.rust))
            } else {
                done(format!("Rust {} is ready. Caches and builds stay in {}.", release.rust, short_path(&self.root)))
            });
        }
        Ok(())
    }
    /// macOS/Linux: switch explicitly between the private Rust and a
    /// compatible installed one.
    fn change_rust(&mut self) -> Result<(), String> {
        let Some(version) = self.pinned_rust() else { return Ok(()) };
        let candidate = match runtime::probe_rust(&version) {
            Ok(candidate) => candidate,
            Err(reason) => {
                view::message(text(format!("No other compatible installed Rust was found: {reason}"), DIM));
                return Ok(());
            }
        };
        let current = matches!(runtime::rust_choice(&self.root)?, RustChoice::External(_));
        let question = format!("Build with a private Rust {version} in this folder, or your installed Rust?");
        let note = format!("installed: {} (not modified)", short_path(&candidate));
        let choice = match view::choose(&question, &note, &["private", "installed"], usize::from(current))?.as_deref() {
            Some("installed") => RustChoice::External(candidate.to_string_lossy().into_owned()),
            Some(_) => RustChoice::Private,
            None => return Ok(()),
        };
        runtime::record_rust_choice(&self.root, &choice)?;
        self.rust_check.borrow_mut().take();
        if self.ready()[1] {
            view::message(done("Rust changed; the next compile uses it."));
            Ok(())
        } else {
            self.setup_tools()
        }
    }
    fn setup_system_tools(&mut self) -> Result<(), String> {
        if runtime::system_tools_ready().is_ok() {
            view::message(done("Compiler, SDK, linker and git are ready."));
            return Ok(());
        }
        if cfg!(target_os = "macos") {
            if self.xcode_screen()? {
                view::message(done("Apple developer tools are ready."));
            }
            return Ok(());
        }
        let release = self.compiler_release()?;
        if self.install_compiler(&release)? {
            view::message(done("Developer tools are ready."));
        }
        Ok(())
    }
    fn install_and_run(&mut self) -> Result<(), String> {
        // Resolve the release once so compiler setup and the following build
        // cannot disagree if a newer release appears while installing tools.
        let release = if self.compiler_retry {
            self.release.clone().ok_or("The compiler retry has no release metadata; select Build tools to start again")?
        } else {
            self.release()?
        };
        if !self.install_compiler(&release)? {
            return Ok(());
        }
        activity("Compiler ready. Continuing to download, compile and open the app.");
        self.build_release(release)
    }
    fn setup_cuda(&mut self) -> Result<(), String> {
        if !crate::cuda::gpu_present() {
            view::message(text("CUDA needs an NVIDIA graphics card; none was found.", DIM));
            return Ok(());
        }
        let installed = self.root.join("toolchain/cuda/bin").join(runtime::exe("nvcc")).is_file();
        if installed {
            self.cuda = !self.cuda;
            view::message(if self.cuda {
                done("CUDA is on. AI apps use it from their next compile.")
            } else {
                text("CUDA is off for the next compile.", DIM)
            });
            return Ok(());
        }
        let detail = format!("Installed in this folder only: NVIDIA CUDA Toolkit {}", crate::cuda::CUDA_VERSION);
        if !self.consent("Install CUDA", "CUDA adds the AI features in Makepad Amp.", &detail, &["cuda"], "install CUDA")? {
            activity("CUDA installation cancelled.");
            return Ok(());
        }
        view::set_view(self.main_view());
        view::working("cuda", "installing…");
        let release = self.compiler_release()?;
        with_progress(|| runtime::dependency(&self.root, &release, Dependency::Cuda))?;
        self.cuda = self.root.join("toolchain/cuda/bin").join(runtime::exe("nvcc")).is_file();
        self.measure_disk();
        view::message(done("CUDA installed. Makepad Amp gets its AI features on its next compile."));
        Ok(())
    }
    /// An update is about to move `release.id` off the snapshot it was built
    /// from: save the edits made there first (see `catalog::save_changes`).
    fn keep_edits(&self, release: &Release) {
        let Some(previous) = load_release(&self.root.join("installed").join(format!("{}.json", release.id))) else { return };
        if previous.release == release.release
            || previous.directory(&self.root) == release.directory(&self.root)
            || self.root.join("changes").join(format!("{}.merge", release.id)).is_file()
        {
            return;
        }
        let (year, month, day, _, _) = local_time();
        match catalog::save_changes(&self.root, &release.id, &previous, &format!("{year:04}-{month:02}-{day:02}")) {
            Ok(Some(name)) => activity(&format!("Saved your changes to {} in changes/{name} before updating; the edited source stays in {}.", release.title, previous.directory(&self.root).display())),
            Ok(None) => {}
            Err(error) => activity(&format!("Could not save a diff of your changes ({error}); the edited source stays in {}.", previous.directory(&self.root).display())),
        }
    }
    fn build_release(&mut self, release: Release) -> Result<(), String> {
        if !self.ready()[..2].iter().all(|ready| *ready) {
            return Err(format!("The latest source requires Rust {}. Select Build tools first.", release.rust));
        }
        // Say which snapshot builds, before anything downloads: shared
        // sources mean shared artifacts; a differing Makepad commit means
        // the dependencies compile again, and that is expected.
        activity(&release.describe_sources(&self.root));
        self.keep_edits(&release);
        with_progress(|| {
            catalog::checkout(&self.service, &self.email, &self.root, &release)?;
            let environment = Environment::prepare(&self.root, &release, self.cuda)?;
            // Scope's release pins the Makepad tree the Builder itself is
            // compiled from. A Builder that fails to compile from the new
            // tree does not withhold the app; the next start tries again.
            if release.id == "scope" {
                match runtime::update_builder(&environment, &release) {
                    Ok(Some(note)) => activity(&note),
                    Ok(None) => {}
                    Err(error) => activity(&format!("Builder not updated: {error}")),
                }
            }
            environment.build(&release)
        })?;
        fs::create_dir_all(self.root.join("installed")).map_err(|e| e.to_string())?;
        release.save(&self.root.join("installed").join(format!("{}.json", release.id)))?;
        release.save(&self.root.join("installed-release.json"))?;
        fs::write(
            self.root.join("installed-cuda"),
            if self.cuda { "1" } else { "0" },
        )
        .map_err(|e| e.to_string())?;
        match catalog::prune_snapshots(&self.root, std::slice::from_ref(&release)) {
            Ok(removed) => {
                for label in removed {
                    activity(&format!("Removed the unused sources {label}; nothing was built from them any more and they held no edits."));
                }
            }
            Err(error) => activity(&format!("Unused sources kept: {error}")),
        }
        activity(&format!("Build complete. Opening {}.", release.title));
        let title = self.open()?;
        activity(&format!("{title} is running; Builder remains open."));
        if self.root.join("changes").join(format!("{}.merge", release.id)).is_file() {
            view::message(text(format!("{title} is updated. Your edits are in changes/; select it to merge."), WARN));
        } else {
            launched(&title);
        }
        Ok(())
    }
    /// Coding agents and the shell work in an app whose source is here.
    fn agent_release(&self, app: Option<&str>) -> Option<Release> {
        match app {
            Some(app) => self.app_release(app),
            None => [self.release.clone(), self.app_release("wm")].into_iter().flatten().find(|r| r.installed(&self.root)),
        }
    }
    fn environment(&self, command: &mut Command) -> Result<(), String> {
        if let Some(release) = self.agent_release(None) {
            let environment = Environment::prepare(&self.root, &release, self.cuda)?;
            let context = crate::agent::write_context(&release, &environment)?;
            command.envs(environment.vars).env("MAKEPAD_AGENT_CONTEXT", context);
        }
        runtime::isolate(command);
        command.current_dir(&self.project);
        Ok(())
    }
    fn shell(&self) -> Result<(), String> {
        let mut command = if cfg!(windows) {
            let mut c = Command::new(env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
            c.args(["/d", "/v:off"]);
            c
        } else {
            Command::new(env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
        };
        self.environment(&mut command)?;
        let _pause = Screen::pause();
        println!(
            "Project: {}\nThis folder's Rust is on PATH. Type exit to return to Makepad.\n",
            self.project.display()
        );
        let status = command
            .status()
            .map_err(|e| format!("Could not start the shell: {e}"))?;
        if !status.success() {
            activity(&format!("Shell exited {status}"));
        }
        Ok(())
    }
    fn launch_agent(&self, name: &str, app: Option<&str>, task: Option<&str>) -> Result<(), String> {
        let release = self.agent_release(app).filter(|r| r.installed(&self.root)).ok_or(
            "Download an app first; coding agents work in its source.",
        )?;
        let tools = if cfg!(windows) { crate::msvc::ready(&self.root.join("toolchain/msvc")) } else { runtime::system_tools_ready().is_ok() };
        if !tools || !self.rust_ready(&release.rust) {
            return Err("Set up the build tools first, so the agent can rebuild apps.".into());
        }
        let environment = Environment::prepare(&self.root, &release, self.cuda)?;
        let mut command = crate::agent::command(name, &release, &environment, task)?;
        fs::write(self.root.join("selected-app"), &release.id).map_err(|e| e.to_string())?;
        let status = {
            let _pause = Screen::pause();
            command.status().map_err(|e| format!("Could not start installed {name}: {e}"))
        };
        let _ = fs::write(self.root.join("selected-app"), &self.app);
        let status = status?;
        if !status.success() {
            return Err(format!("{name} exited {status}. It must already be installed and signed in."));
        }
        Ok(())
    }
    /// After an update saved an app's edits in changes/, a coding agent
    /// reapplies them onto the new source.
    fn merge_changes(&mut self, app: &str) -> Result<(), String> {
        let marker = self.root.join("changes").join(format!("{app}.merge"));
        let diff = fs::read_to_string(&marker).unwrap_or_default().trim().to_owned();
        let agent = match self.agents.iter().filter(|a| a.0 != "grok").collect::<Vec<_>>().as_slice() {
            [] => {
                view::message(text(format!("Install Claude Code or Codex to merge, or apply changes/{diff} yourself."), WARN));
                return Ok(());
            }
            [one] => **one,
            several => {
                let names: Vec<&str> = several.iter().map(|a| a.0).collect();
                let Some(chosen) = view::choose("Merge your changes with which agent?", "", &names, 0)? else { return Ok(()) };
                **several.iter().find(|a| a.0 == chosen).ok_or("Unknown agent")?
            }
        };
        let task = format!(
            "Reapply my changes saved in changes/{diff} in the installation root onto the new {app} source. Keep the intent of each change where the new release moved code, rebuild it until it compiles, and report any change that no longer applies."
        );
        self.launch_agent(agent.0, Some(app), Some(&task))?;
        let _ = fs::remove_file(&marker);
        view::message(done(format!("{} finished merging. Select the app to run it.", agent.1)));
        Ok(())
    }
    /// Launch the app recorded under installed/; returns its title.
    fn open(&self) -> Result<String, String> {
        let release = load_release(&self.root.join("installed").join(format!("{}.json", self.app))).ok_or("Build the app first")?;
        let cuda = fs::read_to_string(self.root.join("installed-cuda")).unwrap_or_default() == "1";
        let environment = Environment::prepare(&self.root, &release, cuda)?;
        let project = if self.project == self.root {
            release
                .repositories
                .iter()
                .find(|repo| repo.name == "makepad")
                .map(|repo| release.directory(&self.root).join(&repo.path))
                .unwrap_or_else(|| release.source(&self.root))
        } else {
            self.project.clone()
        };
        #[cfg(target_os = "macos")]
        let executable = crate::desktop::prepare(&self.root, &release, &project)?;
        #[cfg(not(target_os = "macos"))]
        let executable = environment.app_binary(&release);
        let mut app = Command::new(executable);
        #[cfg(target_os = "macos")]
        if release.id == "scope" {
            // This explicit Builder launch should open in front. Ordinary
            // Makepad windows retain their default non-activating startup.
            app.arg("--focus");
        }
        // The app is a GUI child of Builder. Give it its own process group (or
        // detached Windows process) so closing the terminal that hosts Builder
        // does not terminate an already launched app. Its output is redirected
        // below, so no child console or terminal ownership is needed.
        detach_application(&mut app);
        let log_path = environment.build.join(format!("{}-app.log", release.binary));
        let log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
        view::busy(&format!("Opening {}", release.title));
        activity(&format!("Running {}", release.title));
        let child = app
            .args(["--cwd", &project.to_string_lossy()])
            .current_dir(&project)
            .envs(environment.vars)
            .env_remove("MAKEPAD_LOADER_EMAIL")
            .env_remove("RUSTUP_TOOLCHAIN")
            .stdin(Stdio::null())
            .stderr(log.try_clone().map_err(|e| e.to_string())?)
            .stdout(log)
            .spawn()
            .map_err(|e| e.to_string())?;
        let title = release.title.clone();
        RUNNING_APPS.with(|apps| apps.borrow_mut().push(RunningApp {
            title: release.title, child, log: log_path,
        }));
        Ok(title)
    }
    fn switch_account(&mut self) -> Result<(), String> {
        let (prompt, hint) = if self.email.is_empty() {
            ("Email you bought a Makepad app with:", String::new())
        } else {
            ("Email for your licenses:", format!("⏎ keeps {}", self.email))
        };
        let Some(value) = view::edit(prompt, &hint, "⏎ log in   esc back")? else { return Ok(()) };
        if value.is_empty() || value == self.email {
            return Ok(());
        }
        self.email = catalog::email(&value).map_err(|_| "That does not look like an email address.".to_owned())?;
        self.save_email();
        self.release = None;
        self.refresh_licenses()?;
        let count = self.licenses.as_ref().map_or(0, Vec::len);
        view::message(done(format!("Switched to {} · {count} license{}.", self.email, if count == 1 { "" } else { "s" })));
        Ok(())
    }
    /// Updates: licenses first (new purchases appear, expired ones go), then
    /// every installed app is compared with its newest release. An app with
    /// a newer release gets clean new sources; edits made to its old source
    /// are saved in changes/ first and it waits for an agent to merge them.
    fn check_updates(&mut self) -> Result<(), String> {
        self.refresh_licenses()?;
        view::set_view(self.main_view());
        view::working("updates", "checking…");
        view::busy("Checking the Makepad apps");
        // TODO: send the releases this folder has (the `X-Makepad-Have`
        // header) once the catalog service defines it; today the full catalog
        // is compared here.
        let public = with_progress(|| catalog::fetch_public(&self.service))?;
        self.public = Some(public.clone());
        let available = self.root.join("available");
        fs::create_dir_all(&available).map_err(|e| e.to_string())?;
        for definition in catalog::apps()? {
            let Some(id) = definition.get("id").and_then(makepad_strict_json::Value::as_str) else { continue };
            if available.join(format!("{id}.json")).is_file() || self.root.join("installed").join(format!("{id}.json")).is_file() {
                public.for_app(&definition)?.save(&available.join(format!("{id}.json")))?;
            }
        }
        let mut updated = Vec::new();
        let mut merges = false;
        for entry in fs::read_dir(self.root.join("installed")).into_iter().flatten().flatten() {
            let Some(installed) = load_release(&entry.path()) else { continue };
            let Some(latest) = load_release(&available.join(format!("{}.json", installed.id))) else { continue };
            if latest.release == installed.release || !latest.supported() || (!latest.public && !self.licensed(&latest.id)) {
                continue;
            }
            self.keep_edits(&latest);
            merges |= self.root.join("changes").join(format!("{}.merge", latest.id)).is_file();
            activity(&latest.describe_sources(&self.root));
            with_progress(|| catalog::checkout(&self.service, &self.email, &self.root, &latest))?;
            updated.push(latest.title.clone());
        }
        if let Some(release) = self.release.as_ref().map(|r| r.id.clone()).and_then(|id| load_release(&available.join(format!("{id}.json")))) {
            self.release = Some(release);
        }
        let (_, _, _, hour, minute) = local_time();
        self.checked = Some(if updated.is_empty() {
            format!("checked {hour:02}:{minute:02}")
        } else {
            format!("checked {hour:02}:{minute:02} · updated {}", updated.len())
        });
        self.measure_disk();
        view::message(if updated.is_empty() {
            done("Licenses checked; everything is up to date.")
        } else if merges {
            done(format!("Updated {}. Your edits are saved in changes/; select to merge.", updated.join(", ")))
        } else {
            done(format!("Updated {}. Each compiles on its next run.", updated.join(", ")))
        });
        Ok(())
    }
    fn measure_disk(&self) {
        let root = self.root.clone();
        let disk = self.disk.clone();
        std::thread::spawn(move || {
            let measured = measure(&root);
            if let Ok(mut slot) = disk.lock() {
                slot.0 += 1;
                slot.1 = Some(measured);
            }
        });
    }
    fn disk_changed(&self) -> bool {
        let generation = self.disk.lock().map(|slot| slot.0).unwrap_or(0);
        generation != self.disk_seen.replace(generation)
    }
    fn clear_build(&mut self) -> Result<(), String> {
        let target = self.root.join("target");
        if !target.exists() {
            view::message(text("There is no build data to delete.", DIM));
            return Ok(());
        }
        let question = if cfg!(windows) { "Delete the build data in target\\?" } else { "Delete the build data in target/?" };
        if view::choose(question, "your apps keep working; the next compile starts from scratch", &["keep", "delete"], 0)?.as_deref() != Some("delete") {
            return Ok(());
        }
        // Cargo removes its own build directory (CARGO_TARGET_DIR = target/)
        // with this installation's compiler environment: no hand-written
        // deletes. Published executables live beside Builder and keep working.
        let Some(release) = self.agent_release(None) else {
            view::message(text("Nothing to clean yet: no app has been compiled here.", DIM));
            return Ok(());
        };
        let environment = Environment::prepare(&self.root, &release, self.cuda)?;
        let cargo = environment.vars.get("CARGO").ok_or("Missing Cargo path")?;
        let mut command = Command::new(cargo);
        command.current_dir(&environment.cwd).envs(&environment.vars).arg("clean");
        runtime::isolate(&mut command);
        view::working("disk", "cleaning…");
        view::busy("cargo clean");
        let result = command.output().map_err(|e| e.to_string()).and_then(|out| {
            if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()) }
        });
        self.measure_disk();
        match result {
            Ok(()) => view::message(done("Build data cleaned. Built apps are unchanged.")),
            Err(error) => {
                activity(&format!("cargo clean: {error}"));
                view::message(text("Build data is in use by a running app; close it and try again.", WARN));
            }
        }
        Ok(())
    }
    fn scope_command(&mut self) -> Result<(), String> {
        let question = "Add the scope command to your user PATH?";
        let note = "only the app command is added; compiler paths stay private";
        if view::choose(question, note, &["add", "cancel"], 0)?.as_deref() == Some("add") {
            crate::command::install(&self.root)?;
            view::message(done("In a new terminal: scope opens the current folder, scope PATH a project."));
        }
        Ok(())
    }
    /// The License agreements page: Return opens a row's link in the browser.
    fn agreements(&self) -> Result<(), String> {
        let mut selected = 0;
        loop {
            let view = View {
                crumb: " › License agreements".into(),
                subtitle: "Return opens an agreement in your browser.".into(),
                email: self.shown_email(),
                rows: agreement_rows(None),
                back: true,
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) => open_agreement(&id),
                Nav::Back => return Ok(()),
                Nav::Quit => return Err(QUIT.into()),
                Nav::Refresh => {}
            }
        }
    }
    /// A consent screen of its own for the licenses a step needs: what it
    /// installs and where, each agreement by name (Return opens it), then
    /// "Agree to all" (selected at the start) and Cancel. Escape cancels.
    fn consent(&self, title: &str, intro: &str, detail: &str, ids: &[&str], action: &str) -> Result<bool, String> {
        let mut selected = agreement_rows(Some(ids)).len();
        loop {
            let mut rows = vec![Row::Note(Vec::new())];
            rows.extend(wrap(intro, 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
            if !detail.is_empty() {
                rows.push(Row::Note(text(detail, DIM)));
            }
            rows.push(Row::Head("READ THE AGREEMENTS".into()));
            rows.extend(agreement_rows(Some(ids)));
            rows.push(Row::Note(Vec::new()));
            rows.push(item("agree", "Agree to all", "", Vec::new(), action));
            rows.push(item("cancel", "Cancel", "", Vec::new(), ""));
            let view = View {
                crumb: format!(" › {title}"),
                subtitle: "Please read what you are agreeing to.".into(),
                email: self.shown_email(),
                rows,
                back: true,
                footer: Some("↑↓ move   ⏎ select   esc cancel"),
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) if id == "agree" => return Ok(true),
                Nav::Select(id) if id == "cancel" => return Ok(false),
                Nav::Select(id) => open_agreement(&id),
                Nav::Back | Nav::Quit => return Ok(false),
                Nav::Refresh => {}
            }
        }
    }
    /// The Graphics row: the GPU driver notice, read again on request.
    fn graphics(&mut self) -> Result<(), String> {
        if gpu_notice(&self.shown_email())? {
            self.gpu_read = true;
        }
        Ok(())
    }
    /// Makepad apps: one scrolling list; each compiles on first run.
    fn apps(&mut self) -> Result<(), String> {
        let list = free_apps()?;
        loop {
            let rows: Vec<Row> = list
                .iter()
                .map(|(id, title)| {
                    // One shared Makepad download: rows show only readiness;
                    // the action appears on the selected row.
                    let (status, action) = match self.app_state(id, self.app_release(id).as_ref()) {
                        State::Ready => (done("ready"), "run"),
                        State::Compile => (Vec::new(), "compile and run"),
                        State::Merge => (text("updated · your changes to merge", WARN), "merge with agent"),
                        _ => (Vec::new(), "download source, compile and run"),
                    };
                    item(format!("app:{id}"), title.clone(), "", status, action)
                })
                .collect();
            let view = View {
                crumb: " › Makepad experiments".into(),
                subtitle: "From our open source repository: experiments, not finished applications.".into(),
                email: self.shown_email(),
                rows: {
                    let note = match self.app_release("wm").filter(|r| !r.installed(&self.root)) {
                        Some(release) => format!(
                            "One {} MB download covers them all; each compiles on first run.",
                            release.repositories.iter().map(|r| r.bytes).sum::<u64>().div_ceil(1048576)
                        ),
                        None => "Each one compiles the first time you run it.".into(),
                    };
                    let mut all = vec![Row::Note(Vec::new()), Row::Note(text(note, DIM))];
                    all.extend(rows);
                    all
                },
                back: true,
                ..View::default()
            };
            let mut selected = self.free_selected;
            let nav = view::menu(view, &mut selected, &|| self.disk_changed())?;
            self.free_selected = selected;
            match nav {
                Nav::Select(id) => {
                    // The app returns here when it has been launched (or could
                    // not be), with the row still selected.
                    if let Err(error) = self.open_app(id.trim_start_matches("app:")) {
                        if error == QUIT {
                            return Err(error);
                        }
                        self.report(error);
                    }
                }
                Nav::Back => return Ok(()),
                Nav::Quit => return Err(QUIT.into()),
                Nav::Refresh => {}
            }
        }
    }
    /// macOS: the "› Apple developer tools" screen when clang, the SDK or
    /// git are missing, or Xcode's license is not accepted. Installing opens
    /// Apple's installer and waits for it; the license runs Apple's own
    /// `sudo xcodebuild -license` on the real terminal. Never accepts on the
    /// person's behalf. True once the tools are ready.
    fn xcode_screen(&mut self) -> Result<bool, String> {
        let mut selected = 0;
        loop {
            if runtime::system_tools_ready().is_ok() {
                return Ok(true);
            }
            let license = xcode_license_pending();
            let (intro, note, first) = if license {
                (
                    "Xcode is installed, but its license has not been accepted yet, so Apple's compiler will not run.",
                    "Apple shows the license here in the terminal and asks for your password; type agree at the end to accept it.",
                    item("license", format!("{:<34}", "Read and accept the Xcode license"), "", Vec::new(), "sudo xcodebuild -license"),
                )
            } else {
                (
                    "Makepad compiles with Apple's command line developer tools: clang, the macOS SDK and git. They are not installed on this Mac yet.",
                    "Apple's installer opens in its own window (about 1 GB); come back here when it has finished.",
                    item("install", format!("{:<34}", "Install the developer tools"), "", Vec::new(), "opens Apple's installer"),
                )
            };
            let mut rows = vec![Row::Note(Vec::new())];
            rows.extend(wrap(intro, 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
            rows.push(Row::Note(Vec::new()));
            rows.extend(wrap(note, 74).into_iter().map(|line| Row::Note(text(line, DIM))));
            rows.push(Row::Note(Vec::new()));
            rows.push(first);
            rows.push(item("check", "Check again", "", Vec::new(), ""));
            rows.push(item("cancel", "Cancel", "", Vec::new(), ""));
            let view = View {
                crumb: " › Apple developer tools".into(),
                subtitle: "Needed to compile on macOS.".into(),
                email: self.shown_email(),
                rows,
                back: true,
                footer: Some("↑↓ move   ⏎ select   esc cancel"),
                ..View::default()
            };
            match view::menu(view, &mut selected, &|| false)? {
                Nav::Select(id) if id == "install" => {
                    let _ = Command::new("/usr/bin/xcode-select").arg("--install").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
                    // Poll until Apple's installer has finished; Escape stops waiting.
                    let _input = console::Input::enter()?;
                    let mut waited = 0;
                    loop {
                        view::busy("Waiting for Apple's installer to finish · esc stops waiting");
                        if matches!(console::key()?, Key::Back | Key::Quit) {
                            break;
                        }
                        waited += 1;
                        // Each probe runs the tools; check every few seconds.
                        if waited % 15 == 0 && runtime::system_tools_ready().is_ok() {
                            break;
                        }
                    }
                }
                Nav::Select(id) if id == "license" => {
                    let _pause = Screen::pause();
                    println!("Apple's Xcode license follows. Type agree at the end to accept it.\n");
                    let _ = Command::new("sudo").args(["/usr/bin/xcodebuild", "-license"]).status();
                }
                Nav::Select(id) if id == "check" => {
                    view::busy("Checking clang, the macOS SDK, the linker and git");
                    if runtime::system_tools_ready().is_err() {
                        view::message(text("Still not ready.", WARN));
                    }
                }
                Nav::Select(_) | Nav::Back | Nav::Quit => return Ok(false),
                Nav::Refresh => {}
            }
        }
    }
    /// First run: the Log in screen with an inline email editor. Empty
    /// continues with the open source experiments.
    fn log_in_screen(&mut self) -> Result<(), String> {
        let mut rows = vec![Row::Note(Vec::new())];
        rows.extend(wrap("Enter the email address you bought a Makepad app with, such as Scope or Amp, or that has beta access. It shows your licenses and stays in this folder only.", 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
        rows.push(Row::Note(Vec::new()));
        rows.extend(wrap("Leave it empty to continue with the open source experiments; you can log in later from the menu.", 74).into_iter().map(|line| Row::Note(text(line, DIM))));
        view::set_view(View {
            crumb: " › Log in".into(),
            subtitle: "Welcome to the Makepad Builder.".into(),
            email: self.shown_email(),
            rows,
            back: true,
            ..View::default()
        });
        let mut hint = "";
        loop {
            let Some(value) = view::edit("Email:", hint, "type your email   ⏎ continue")? else { return Ok(()) };
            if value.is_empty() {
                return Ok(());
            }
            match catalog::email(&value) {
                Ok(email) => {
                    self.email = email;
                    self.save_email();
                    return Ok(());
                }
                Err(_) => hint = "That does not look like an email address.",
            }
        }
    }
    /// The header's right side.
    fn shown_email(&self) -> String {
        if self.email.is_empty() { "not logged in".into() } else { self.email.clone() }
    }
    fn report(&self, error: String) {
        let error = if self.email.is_empty() { error } else { error.replace(&self.email, "[email]") };
        view::warn(&error);
    }
    fn app_row(&self, release: &Release) -> Row {
        let state = self.app_state(&release.id, Some(release));
        let (mut status, mut action) = state.texts(Some(release.repositories.iter().map(|r| r.bytes).sum()));
        if !release.supported() {
            status = text("not available for this platform yet", DIM);
            action = String::new();
        }
        let license = if release.license == "beta" { "beta" } else { "commercial" };
        item(format!("app:{}", release.id), release.title.clone(), license, status, action)
    }
    fn main_view(&self) -> View {
        let mut rows = vec![Row::Head("SETUP".into())];
        rows.push(item(
            "account",
            "Account",
            "",
            if self.email.is_empty() { text("not logged in", WARN) } else { text(self.email.clone(), PLAIN) },
            if self.email.is_empty() { "log in" } else { "switch email" },
        ));
        if !cfg!(target_os = "macos") {
            rows.push(if self.gpu_read {
                item("gpu", "Graphics", "", done("driver notice read"), "")
            } else {
                item("gpu", "Graphics", "", text("read the driver notice", WARN), "read")
            });
        }
        let ready = self.ready();
        // Installing the build tools accepts Makepad, Microsoft (Windows) and
        // Rust; installing CUDA accepts NVIDIA.
        let cuda_installed = self.root.join("toolchain/cuda/bin").join(runtime::exe("nvcc")).is_file();
        let mut names = vec!["Makepad"];
        if cfg!(windows) { names.push("Microsoft"); }
        names.push("Rust");
        let agreements_status = if ready[0] && ready[1] {
            if cuda_installed { names.push("NVIDIA"); }
            vec![view::Span("✓".into(), OK), view::Span(" accepted ".into(), PLAIN), view::Span(format!("· {}", names.join(", ")), DIM)]
        } else {
            if crate::cuda::supported() { names.push("NVIDIA"); }
            text(names.join(", "), DIM)
        };
        rows.push(item("terms", "Agreements", "", agreements_status, "read"));
        let version = self.pinned_rust().unwrap_or_default();
        if cfg!(windows) {
            let (status, action) = if self.compiler_retry {
                (text("Windows security is still checking Rust", WARN), "check again")
            } else if ready[0] && ready[1] {
                (done(format!("Visual Studio Build Tools, Windows SDK, Rust {version}")), "recheck")
            } else if ready[0] && !version.is_empty() {
                (text(format!("Rust {version} not installed"), WARN), "install")
            } else {
                (text("not installed", WARN), "install")
            };
            rows.push(item("tools", "Build tools", "", status, action));
        } else {
            let (status, action) = if ready[1] {
                match runtime::rust_choice(&self.root) {
                    Ok(RustChoice::External(sysroot)) => (done(format!("installed Rust {}", short_path(Path::new(&sysroot)))), "change"),
                    _ => (
                        vec![view::Span("✓".into(), OK), view::Span(format!(" private {version} "), PLAIN), view::Span(format!("in {}", short_path(&self.root)), DIM)],
                        "change",
                    ),
                }
            } else {
                (text("not set up", WARN), "set up")
            };
            rows.push(item("rust", "Rust", "", status, action));
            let name = if cfg!(target_os = "macos") { "Xcode tools" } else { "System packages" };
            let (status, action) = if ready[0] {
                (done(if cfg!(target_os = "macos") { "clang, SDK, git" } else { "compiler, linker, git" }), "recheck")
            } else if cfg!(target_os = "macos") && xcode_license_pending() {
                (text("license not accepted", WARN), "accept")
            } else if cfg!(target_os = "macos") {
                (text("not installed", WARN), "install")
            } else {
                (text("missing", WARN), "set up")
            };
            rows.push(item("system", name, "", status, action));
        }
        if crate::cuda::supported() {
            let installed = self.root.join("toolchain/cuda/bin").join(runtime::exe("nvcc")).is_file();
            let (status, action) = if !crate::cuda::gpu_present() {
                (text("needs an NVIDIA GPU", DIM), "")
            } else if installed && self.cuda {
                (vec![view::Span("✓".into(), OK), view::Span(" CUDA ".into(), PLAIN), view::Span("· AI features in Makepad Amp".into(), DIM)], "turn off")
            } else if installed {
                (text("off · AI features in Makepad Amp", DIM), "turn on")
            } else {
                (text("optional · AI features in Makepad Amp", DIM), "install")
            };
            rows.push(item("cuda", "CUDA", "", status, action));
        }
        rows.push(self.disk_row());
        rows.push(item("updates", "Update", "", text(self.checked.clone().unwrap_or_else(|| "not checked yet".into()), DIM), "check licenses & pull"));
        if self.root.join(if cfg!(windows) { "scope.exe" } else { "scope.bin" }).is_file() {
            rows.push(item("command", "Scope command", "", text("open projects from any terminal", DIM), "add to PATH"));
        }
        rows.push(Row::Head("YOUR LICENSES".into()));
        match &self.licenses {
            _ if self.email.is_empty() => rows.push(item("login", "Log in with your email address", "", Vec::new(), "to see your licenses")),
            Some(list) if list.is_empty() => rows.push(Row::Note(text("none on this email yet · buy or request beta access at makepad.nl", DIM))),
            Some(list) => rows.extend(list.iter().map(|release| self.app_row(release))),
            None => match &self.license_error {
                None => rows.push(Row::Note(text(format!("checking licenses for {}…", self.email), DIM))),
                Some(error) => {
                    // Offline: the apps cached from the last check still run.
                    let cached: Vec<Release> = fs::read_dir(self.root.join("available")).into_iter().flatten().flatten()
                        .filter_map(|e| load_release(&e.path())).filter(|r| !r.public).collect();
                    rows.push(Row::Note(text(format!("licenses not checked: {}", clean(error.lines().next().unwrap_or_default())), WARN)));
                    rows.extend(cached.iter().map(|release| self.app_row(release)));
                }
            },
        }
        let free = free_apps().unwrap_or_default();
        let built = free.iter().filter(|(id, _)| self.app_state(id, self.app_release(id).as_ref()) == State::Ready).count();
        rows.push(Row::Head("MAKEPAD EXPERIMENTS".into()));
        rows.push(item("free", "Experiments", "free", text(format!("{} · {built} ready", free.len()), PLAIN), "open list"));
        rows.push(Row::Head("CODING AGENTS · each one knows how to change and rebuild these apps".into()));
        for (command, title) in &self.agents {
            rows.push(item(format!("agent-{command}"), *title, "", Vec::new(), "open"));
        }
        rows.push(item("agent-shell", "Shell", "", text("with this folder's Rust on PATH", DIM), "open"));
        View {
            crumb: String::new(),
            subtitle: ABOUT.trim().into(),
            email: self.shown_email(),
            rows,
            back: false,
            ..View::default()
        }
    }
    fn disk_row(&self) -> Row {
        // Folder total, then what "clean build" can delete.
        match self.disk.lock().ok().and_then(|slot| slot.1) {
            None => item("disk", "Disk", "", text("measuring…", DIM), ""),
            Some((total, 0)) => item("disk", "Disk", "", text(format!("{} GB used", gb(total)), PLAIN), ""),
            Some((total, build)) => item(
                "disk",
                "Disk",
                "",
                vec![view::Span(format!("{} GB used ", gb(total)), PLAIN), view::Span(format!("· build {} GB", gb(build)), DIM)],
                "clean build",
            ),
        }
    }
    /// Index of the first licensed app row, where the menu starts.
    fn first_license_row(&self, view: &View) -> usize {
        let mut index = 0;
        let mut in_licenses = false;
        for row in &view.rows {
            match row {
                Row::Head(title) => in_licenses = title == "YOUR LICENSES",
                Row::Item(_) if in_licenses => return index,
                Row::Item(_) => index += 1,
                Row::Note(_) => {}
            }
        }
        0
    }
    fn act(&mut self, id: &str) -> Result<(), String> {
        match id {
            "account" | "login" => self.switch_account(),
            "gpu" => self.graphics(),
            "tools" | "rust" => self.setup_tools(),
            "system" => self.setup_system_tools(),
            "cuda" => self.setup_cuda(),
            "updates" => self.check_updates(),
            "disk" => self.clear_build(),
            "terms" => self.agreements(),
            "command" => self.scope_command(),
            "free" => self.apps(),
            "agent-shell" => self.shell(),
            agent if agent.starts_with("agent-") => self.launch_agent(&agent["agent-".len()..], None, None),
            app => self.open_app(app.trim_start_matches("app:")),
        }
    }
}

/// The success line after launching an app.
fn launched(title: &str) {
    let hint = if cfg!(target_os = "macos") {
        "Right-click its Dock icon › Options › Keep in Dock."
    } else if cfg!(windows) {
        "Right-click its taskbar icon › Pin to taskbar."
    } else {
        "Run it again from here or from its command."
    };
    view::message(vec![
        view::Span("✓".into(), OK),
        view::Span(format!(" {title} is running. "), PLAIN),
        view::Span(hint.into(), DIM),
    ]);
}

fn detach_application(command: &mut Command) {
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS prevents an accidental console window; the new
        // process group keeps Ctrl+C and console teardown scoped to Builder.
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        // Create a new session before exec. This removes the controlling
        // terminal as well as its process group, so the app is independent of
        // the shell hosting Builder on both macOS and Linux.
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        command.pre_exec(|| {
            if setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg(not(any(windows, unix)))] {
        let _ = command;
    }
}

fn load_release(path: &Path) -> Option<Release> {
    Release::parse(&makepad_strict_json::parse(&fs::read(path).ok()?).ok()?).ok()
}
struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("pid"));
        let _ = fs::remove_dir(&self.0);
    }
}

pub fn run() -> Result<(), String> {
    let root = crate::validate_install_root(&crate::default_root())?;
    let bootstrap = match makepad_loader_bundle::load_from(&root)? {
        Some(bootstrap) => Some(bootstrap),
        None => makepad_loader_bundle::load()?.map(|(_, b)| b),
    };
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    crate::command::repair(&root)?;
    let lock = root.join(".setup-lock");
    // A closed terminal can end this process before Drop runs. Reclaim only
    // our own lock format with a provably exited owner, never an active setup.
    if let Ok(pid) = fs::read_to_string(lock.join("pid")) {
        if pid
            .trim()
            .parse::<u32>()
            .is_ok_and(|p| p > 0 && !console::alive(p))
        {
            let _ = fs::remove_file(lock.join("pid"));
            let _ = fs::remove_dir(&lock);
        }
    }
    fs::create_dir(&lock).map_err(|_| {
        "Makepad Builder is already using this folder. Close it before starting another.".to_owned()
    })?;
    let _lock = Lock(lock);
    fs::write(_lock.0.join("pid"), std::process::id().to_string()).map_err(|e| e.to_string())?;
    let mut setup = Setup {
        project: env::var_os("MAKEPAD_LOADER_PROJECT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone())
            .canonicalize().map_err(|e| e.to_string())?,
        service: env::var("MAKEPAD_LOADER_SERVICE")
            .unwrap_or_else(|_| catalog::DEFAULT_SERVICE.into()),
        email: match env::var("MAKEPAD_LOADER_EMAIL")
            .ok()
            .or_else(|| bootstrap.as_ref().map(|b| b.email.clone()))
        {
            Some(email) if !email.is_empty() => catalog::email(&email)?,
            _ => String::new(),
        },
        app: "scope".into(),
        release: None,
        cuda: fs::read_to_string(root.join("installed-cuda")).unwrap_or_default() == "1",
        compiler_retry: false,
        rust_check: Default::default(),
        licenses: None,
        license_error: None,
        public: None,
        checked: None,
        disk: Arc::new(Mutex::new((0, None))),
        disk_seen: std::cell::Cell::new(0),
        agents: [("claude", "Claude Code"), ("codex", "Codex"), ("grok", "Grok")].into_iter().filter(|(command, _)| on_path(command)).collect(),
        free_selected: 0,
        gpu_read: false,
        root,
    };
    view::set_log(setup.root.join("builder.log"));
    setup.release = load_release(&setup.root.join("available/scope.json"))
        .or_else(|| load_release(&setup.root.join("latest.json")).filter(|r| r.id == "scope"))
        .or_else(|| load_release(&setup.root.join("installed/scope.json")));
    setup.public = fs::read_dir(setup.root.join("available")).into_iter().flatten().flatten()
        .filter_map(|entry| load_release(&entry.path()))
        .filter(|release| release.public)
        .max_by(|a, b| a.release.cmp(&b.release));
    let screen = Screen::enter();
    // The email comes first (downloads are not personalized): asked once per
    // folder, kept in its makepad-builder.json and never asked again.
    if setup.email.is_empty() && !setup.root.join(".login-asked").exists() {
        setup.log_in_screen()?;
        let _ = fs::write(setup.root.join(".login-asked"), "");
    }
    setup.gpu_read = gpu_warning_acknowledged(&setup.shown_email())?;
    if !setup.gpu_read {
        drop(screen);
        println!("Setup cancelled. Nothing was downloaded or installed.");
        return Ok(());
    }
    show_menu(&mut setup)?;
    drop(screen);
    if io::stdout().is_terminal() { print!("\x1b[0m"); }
    println!("Makepad Builder closed.");
    Ok(())
}

/// Linux and Windows show the GPU driver notice once per setup invocation,
/// before anything is probed, refreshed, downloaded or compiled. On Linux the
/// POSIX installer and bootstrap ask in the shell first and pass
/// MAKEPAD_GPU_ACK=1 down so the chain asks once; only Linux honours that
/// marker, and it is cleared here so shells, agents and apps started from
/// Builder never inherit a skip. Windows always asks itself. macOS does not
/// show the notice. Declining, Escape or closed input quits setup.
fn gpu_warning_acknowledged(email: &str) -> Result<bool, String> {
    let inherited = env::var_os("MAKEPAD_GPU_ACK").is_some_and(|value| value == "1");
    env::remove_var("MAKEPAD_GPU_ACK");
    if cfg!(target_os = "macos") || (cfg!(target_os = "linux") && inherited) {
        return Ok(true);
    }
    gpu_notice(email)
}

/// The GPU driver notice as a screen of its own: "I understand" (selected)
/// or Cancel; Escape and closed input cancel.
fn gpu_notice(email: &str) -> Result<bool, String> {
    let mut rows = vec![Row::Note(Vec::new())];
    rows.extend(wrap(GPU_NOTICE, 74).into_iter().map(|line| Row::Note(text(line, PLAIN))));
    rows.push(item("continue", "I understand", "", Vec::new(), ""));
    rows.push(item("cancel", "Cancel", "", Vec::new(), ""));
    let view = View {
        crumb: " › Graphics".into(),
        subtitle: "Before you continue.".into(),
        email: email.into(),
        rows,
        back: true,
        footer: Some("↑↓ move   ⏎ select   esc cancel"),
        ..View::default()
    };
    let mut selected = 0;
    loop {
        match view::menu(view.clone(), &mut selected, &|| false)? {
            Nav::Select(id) => return Ok(id == "continue"),
            Nav::Back | Nav::Quit => return Ok(false),
            Nav::Refresh => {}
        }
    }
}
fn wrap(value: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in value.split_whitespace() {
        let last = lines.last_mut().unwrap();
        if !last.is_empty() && last.chars().count() + 1 + word.chars().count() > width {
            lines.push(word.to_owned());
        } else {
            if !last.is_empty() { last.push(' '); }
            last.push_str(word);
        }
    }
    lines
}

fn show_menu(setup: &mut Setup) -> Result<(), String> {
    fs::write(setup.root.join("selected-app"), &setup.app).map_err(|e| e.to_string())?;
    setup.measure_disk();
    // Every start refreshes the licenses, sets up missing build tools, then
    // continues into the primary app once (Scope, when licensed). Success,
    // cancellation and failure all return to the menu; never restart the
    // sequence on redraw.
    let startup = (|| -> Result<(), String> {
        // Setup screens follow the Graphics screen directly: no main menu
        // or license check is drawn before the build tools consent; the menu
        // first appears after Agree (busy install) or Cancel.
        let ready = setup.ready();
        let mut tools_ready = ready[0] && ready[1];
        if !tools_ready {
            match setup.setup_tools() {
                Err(error) if error == QUIT => return Err(error),
                Err(error) => setup.report(error),
                Ok(()) => {}
            }
            let ready = setup.ready();
            tools_ready = ready[0] && ready[1];
        }
        if let Err(error) = setup.refresh_licenses() {
            setup.report(error);
        }
        view::set_view(setup.main_view());
        if !tools_ready {
            return Ok(());
        }
        if setup.licensed(&setup.app) {
            view::set_view(setup.main_view());
            let app = setup.app.clone();
            setup.open_app(&app)?;
        }
        Ok(())
    })();
    match startup {
        Err(error) if error == QUIT => return Ok(()),
        Err(error) => setup.report(error),
        Ok(()) => {}
    }
    let mut selected = setup.first_license_row(&setup.main_view());
    loop {
        let view = setup.main_view();
        let nav = view::menu(view, &mut selected, &|| setup.disk_changed())?;
        match nav {
            Nav::Refresh => {}
            Nav::Back | Nav::Quit => break,
            Nav::Select(id) => match setup.act(&id) {
                Err(error) if error == QUIT => break,
                Err(error) => setup.report(error),
                Ok(()) => {}
            },
        }
    }
    Ok(())
}

/// macOS: Apple's compiler refuses to run until Xcode's license is accepted,
/// and says so. Only reads; never accepts.
fn xcode_license_pending() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    Command::new("/usr/bin/xcrun")
        .args(["--sdk", "macosx", "clang", "--version"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| !output.status.success() && String::from_utf8_lossy(&output.stderr).to_lowercase().contains("license"))
}
