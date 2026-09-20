//! The setup process owns blocking work. On Windows MpTerm hosts this process
//! and all of its children; Unix bootstraps use the matching terminal menu.
use crate::{
    catalog::{self, Release},
    progress,
    rustc,
    runtime::{self, Dependency, Environment, RustChoice},
};
#[cfg(not(windows))]
use std::io::Read;
use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const MENU: &str = include_str!("../menu.txt");
const ABOUT: &str = include_str!("../about.txt");
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
use view::{activity, Screen};

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
                    activity(&format!("{} exited {status}; see {}", app.title, app.log.display()));
                }
                changed = true;
                false
            }
            Err(error) => {
                activity(&format!("{}: {error}", app.title));
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
    Quit,
    Char(char),
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
            27 | 3 => Key::Quit,
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
    unsafe extern "C" {
        fn ioctl(fd: i32, request: usize, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
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
                    .args(["-icanon", "-echo", "-isig", "min", "0", "time", "2"])
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
    pub fn key() -> Result<Key, String> {
        let mut byte = [0];
        if io::stdin().read(&mut byte).map_err(|e| e.to_string())? == 0 {
            // Closed input is never an answer: leave the menu instead of
            // spinning or accepting a default.
            return Ok(Key::Quit);
        }
        Ok(match byte[0] {
            b'\r' | b'\n' => Key::Enter,
            9 => Key::Right,
            3 => Key::Quit,
            27 => {
                let mut sequence = Vec::new();
                for _ in 0..32 {
                    if io::stdin().read(&mut byte).map_err(|e| e.to_string())? == 0 {
                        break;
                    }
                    sequence.push(byte[0]);
                    if byte[0].is_ascii_alphabetic() || byte[0] == b'~' {
                        break;
                    }
                }
                match sequence.as_slice() {
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
                    b"[21~" | b"" => Key::Quit,
                    _ => Key::Other,
                }
            }
            c => Key::Char(c as char),
        })
    }
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
    /// change of the recorded choice is an explicit answer; cancelling keeps
    /// the previous record. Returns true when the record changed. Windows
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
        let mut info = Vec::new();
        if let Some((recorded, reason)) = &stale {
            info.push(format!("The selected Rust at {recorded} cannot be used:"));
            info.push(reason.clone());
        }
        match runtime::probe_rust(version) {
            Ok(candidate) => {
                let sysroot = candidate.to_string_lossy().into_owned();
                info.extend([
                    format!("Found an installed Rust ({version} or newer) at:"),
                    sysroot.clone(),
                    "It is not modified. Cargo caches and build output stay in this folder.".into(),
                    "Yes: use this installed Rust. No: download a private Rust into this folder instead.".into(),
                ]);
                let choice = if Screen::enter().confirm("Installed Rust", &info)? {
                    activity("Using the installed Rust.");
                    RustChoice::External(sysroot)
                } else {
                    activity("Using a private Rust in this folder.");
                    RustChoice::Private
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
                info.extend([
                    format!("No other compatible installed Rust was found: {reason}"),
                    "Yes: switch to a private Rust in this folder (its license confirmation follows).".into(),
                    "No: keep the selected Rust and stop here.".into(),
                ]);
                if Screen::enter().confirm("Selected Rust unavailable", &info)? {
                    runtime::record_rust_choice(&self.root, &RustChoice::Private)?;
                    activity("Switching to a private Rust in this folder.");
                    Ok(true)
                } else {
                    Err(format!("Kept the selected Rust at {recorded}. Make it available again, or choose Download compiler to switch."))
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
    fn refresh(&mut self) -> Result<Release, String> {
        if self.app == "scope" && self.email.is_empty() {
            let _pause = Screen::pause();
            self.email = catalog::email(&line("Scope access email: ")?)?;
        }
        let release = with_progress(|| {
            progress::stage(
                "Checking sources",
                "The source service may refresh its Git cache while we wait",
                0.0,
            );
            if self.app == "scope" {
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
    fn apps(&mut self) -> Result<(), String> {
        let mut apps: Vec<(String, String, String)> = Vec::new();
        for app in catalog::apps()? {
            if app.get("menu").and_then(makepad_strict_json::Value::as_str) != Some("other") { continue; }
            let text = |name| app.get(name).and_then(makepad_strict_json::Value::as_str).map(str::to_owned).ok_or("Invalid app registry");
            apps.push((text("id")?, text("title")?, "Public Makepad app".into()));
        }
        let mut selected = apps.iter().position(|a| a.0 == self.app).unwrap_or(0);
        let mut options: Vec<_> = apps.iter().enumerate().map(|(i, a)| ((i + 1).to_string(), a.1.clone(), a.2.clone())).collect();
        options.push(("q".into(), "Back".into(), String::new()));
        let info = ["Choosing an app sets up any missing compiler, downloads its sources, then compiles and opens it".into()];
        loop {
            match Screen::enter().choose_from("Makepad Apps", &info, &options, selected)?.as_str() {
                "q" => return Ok(()),
                choice => {
                    selected = choice.parse::<usize>().ok().and_then(|i| i.checked_sub(1)).ok_or("Invalid app choice")?;
                    let app = apps.get(selected).ok_or("Invalid app choice")?;
                    // The app returns here when it has been launched (or could
                    // not be), with the row still selected.
                    if let Err(error) = self.run_app(&app.0) {
                        let text = clean(&if self.email.is_empty() { error } else { error.replace(&self.email, "[email]") });
                        activity(&text);
                        Screen::enter().message("Step could not complete", &text)?;
                    }
                }
            }
        }
    }
    fn app_setup(&self, app: &str) -> Result<Self, String> {
        let definition = catalog::apps()?.into_iter().find(|entry| entry.get("id").and_then(makepad_strict_json::Value::as_str) == Some(app)).ok_or("Unknown public app")?;
        let mut selected = self.clone();
        selected.app = app.into();
        let available = self.root.join("available");
        let cached = available.join(format!("{}.json", selected.app));
        selected.release = load_release(&cached)
            .or_else(|| load_release(&self.root.join("installed").join(format!("{}.json", selected.app))));
        if selected.release.is_none() {
            selected.release = self.release.as_ref().map(|release| release.for_app(&definition)).transpose()?;
        }
        if let Some(release) = &selected.release {
            fs::create_dir_all(&available).map_err(|e| e.to_string())?;
            release.save(&cached)?;
        }
        Ok(selected)
    }
    fn run_app(&mut self, app: &str) -> Result<(), String> {
        let mut selected = self.app_setup(app)?;
        fs::write(self.root.join("selected-app"), app).map_err(|e| e.to_string())?;
        let result = selected.install_and_run();
        fs::write(self.root.join("selected-app"), &self.app).map_err(|e| e.to_string())?;
        result
    }
    fn ready(&self) -> [bool; 3] {
        let tools = if cfg!(windows) {
            crate::msvc::ready(&self.root.join("toolchain/msvc"))
        } else {
            runtime::system_tools_ready().is_ok()
        };
        let rust = self.release.as_ref()
            .is_some_and(|r| self.rust_ready(&r.rust));
        [
            tools,
            rust,
            self.release
                .as_ref()
                .is_some_and(|r| r.installed(&self.root)),
        ]
    }
    fn install_compiler(&mut self, release: &Release) -> Result<bool, String> {
        if self.compiler_retry {
            activity("Retrying the staged Rust compiler check; no download is needed.");
            return match rustc::retry_staged(&runtime::rust_dir(&self.root, &release.rust), &release.rust) {
                Ok(()) => {
                    self.compiler_retry = false;
                    activity("Compiler is ready.");
                    Ok(true)
                }
                Err(error) if rustc::needs_compiler_retry(&error) => Err(
                    "Windows security software is still scanning the staged Rust compiler. Choose Retry compiler check to try again.".into()
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
        // The license screen: why the compiler is needed, what this step
        // downloads and from where, the terms of exactly those downloads, and
        // one question. Missing pieces only: Rust when it is not installed,
        // the Microsoft tools when they are not. Plain terminals append
        // "[Y/n]" to the last line; Enter accepts.
        let mut info = vec![format!(
            "{} is compiled from source on this computer, so you can tweak it with your own coding agent.",
            release.title
        )];
        let mut sources = Vec::new();
        if !ready[1] {
            sources.push(format!("Rust {} from rust-lang.org", release.rust));
        }
        if cfg!(windows) && !ready[0] {
            sources.push("the Microsoft Build Tools and Windows SDK from Microsoft".to_owned());
        }
        if !sources.is_empty() {
            info.push(format!("Downloaded from official sources: {}.", sources.join(", and ")));
        }
        if !cfg!(windows) && !ready[0] {
            info.push(if cfg!(target_os = "macos") {
                "Apple's Command Line Tools are installed through Apple (xcode-select) under Apple's terms.".to_owned()
            } else {
                "System development packages are installed with your distribution's package manager.".to_owned()
            });
        }
        if !ready[1] {
            info.push("Rust terms (MIT and Apache 2.0): https://www.rust-lang.org/policies/licenses".into());
        }
        if cfg!(windows) && !ready[0] {
            info.extend([
                "Microsoft Build Tools terms: https://visualstudio.microsoft.com/license-terms/vs2022-ga-diagnosticbuildtools/".into(),
                "Windows SDK terms: https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license".into(),
            ]);
        }
        info.push("Do you accept?".into());
        if !Screen::enter().confirm("Compiler license terms", &info)? {
            activity("Compiler installation cancelled.");
            return Ok(false);
        }
        if !cfg!(windows) && !ready[0] {
            let _pause = Screen::pause();
            runtime::setup_system_tools()?;
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
                    "Windows security software is still scanning the staged Rust compiler. Choose Retry compiler check to try again.".into()
                );
            }
        }
        result
    }
    fn install_and_run(&mut self) -> Result<(), String> {
        // Resolve the release once so compiler setup and the following build
        // cannot disagree if a newer release appears while installing tools.
        let release = if self.compiler_retry {
            self.release.clone().ok_or("The compiler retry has no release metadata; choose Download compiler to start again")?
        } else {
            self.refresh()?
        };
        if !self.install_compiler(&release)? {
            return Ok(());
        }
        activity("Compiler ready. Continuing to download, compile and open the app.");
        self.build_release(release)
    }
    fn install_cuda(&self, release: &Release) -> Result<(), String> {
        let info = [
            "Optional CUDA compiler and libraries".into(),
            "By continuing, you accept NVIDIA's CUDA EULA:".into(),
            "https://docs.nvidia.com/cuda/eula/".into(),
        ];
        if !Screen::enter().confirm("NVIDIA license terms", &info)? {
            activity("CUDA installation cancelled.");
            return Ok(());
        }
        with_progress(|| runtime::dependency(&self.root, release, Dependency::Cuda)).map(|_| ())
    }
    fn build(&mut self) -> Result<(), String> {
        if !self.ready()[..2].iter().all(|v| *v) {
            return Err("Download the compiler first".into());
        }
        let release = self.refresh()?;
        self.build_release(release)
    }
    fn build_release(&mut self, release: Release) -> Result<(), String> {
        if !self.ready()[..2].iter().all(|ready| *ready) {
            return Err(format!("The latest source requires Rust {}. Choose Download compiler first.", release.rust));
        }
        // Say which snapshot builds, before anything downloads: shared
        // sources mean shared artifacts; a differing Makepad commit means
        // the dependencies compile again, and that is expected.
        activity(&release.describe_sources(&self.root));
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
        self.open()?;
        activity(&format!("{} is running; Builder remains open.", release.title));
        let launch_hint = if cfg!(target_os = "macos") {
            format!("Keep {} in your Dock to launch it again later.", release.title)
        } else if cfg!(windows) {
            format!("Keep {} pinned to your taskbar to launch it again later.", release.title)
        } else {
            format!("You can launch {} again from Builder or its installed command.", release.title)
        };
        Screen::enter().message(
            "Build complete",
            &format!(
                "{} is built and running.\n{}\nContinue to return to the Builder menu.",
                release.title, launch_hint
            ),
        )
    }
    fn environment(&self, command: &mut Command) -> Result<(), String> {
        if let Some(release) = &self.release {
            let environment = Environment::prepare(&self.root, &release, self.cuda)?;
            let context = crate::agent::write_context(&release, &environment)?;
            command.envs(environment.vars).env("MAKEPAD_AGENT_CONTEXT", context);
        }
        runtime::isolate(command);
        command.current_dir(&self.project);
        Ok(())
    }
    fn ai_terminal(&self) -> Result<(), String> {
        let options = [
            ("1", "Claude", "Edit and rebuild this app with the installed claude command"),
            ("2", "Codex", "Edit and rebuild this app with the installed codex command"),
            ("3", "Shell", "Use exit to return to Makepad Builder"),
            (
                "4",
                "Other installed tool",
                "Enter an executable name or path",
            ),
            ("q", "Back", ""),
        ]
        .map(|(k, l, d)| (k.into(), l.into(), d.into()));
        let key = Screen::enter().choose(
            "AI Terminal",
            &[format!("Project: {}", self.project.display())],
            &options,
        )?;
        if matches!(key.as_str(), "1" | "2") {
            return self.launch_agent(if key == "1" { "claude" } else { "codex" });
        }
        let executable = match key.as_str() {
            "3" => None,
            "4" => Some(line("Executable (no arguments): ")?),
            _ => return Ok(()),
        };
        let mut command = if cfg!(windows) {
            let mut c = Command::new(env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
            c.args(["/d", "/v:off"]);
            if let Some(executable) = &executable {
                // CMD is needed for npm's .cmd shims. Validate before composing
                // its command line; user input is never treated as shell code.
                if executable.is_empty()
                    || executable.contains(['"', '%', '!', '&', '|', '<', '>', '^', '\r', '\n'])
                {
                    return Err(
                        "Use an executable name or path without shell metacharacters".into(),
                    );
                }
                c.args(["/s", "/c", &format!("\"{executable}\"")]);
            }
            c
        } else {
            Command::new(
                executable
                    .as_deref()
                    .unwrap_or(&env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())),
            )
        };
        self.environment(&mut command)?;
        let _pause = Screen::pause();
        println!(
            "Project: {}\nExit the tool to return to Makepad.\n",
            self.project.display()
        );
        let status = command
            .status()
            .map_err(|e| format!("Could not launch installed tool: {e}"))?;
        if !status.success() {
            return Err(format!("Tool exited {status}; no software was installed"));
        }
        Ok(())
    }
    fn launch_agent(&self, name: &str) -> Result<(), String> {
        if !self.ready().into_iter().all(|v| v) { return Err("Complete compiler and source setup before opening an app agent".into()); }
        let release = self.release.clone().ok_or("Download the app sources first")?;
        let environment = Environment::prepare(&self.root, &release, self.cuda)?;
        let mut command = crate::agent::command(name, &release, &environment)?;
        let _pause = Screen::pause();
        let status = command.status().map_err(|e| format!("Could not start installed {name}: {e}"))?;
        if !status.success() { return Err(format!("{name} exited {status}. It must already be installed and signed in.")); }
        Ok(())
    }
    fn open(&self) -> Result<(), String> {
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
        RUNNING_APPS.with(|apps| apps.borrow_mut().push(RunningApp {
            title: release.title, child, log: log_path,
        }));
        Ok(())
    }
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
        root,
    };
    setup.release = load_release(&setup.root.join("available/scope.json"))
        .or_else(|| load_release(&setup.root.join("latest.json")).filter(|r| r.id == "scope"))
        .or_else(|| load_release(&setup.root.join("installed/scope.json")));
    if !gpu_warning_acknowledged()? {
        println!("Setup cancelled. Nothing was downloaded or installed.");
        return Ok(());
    }
    show_menu(&mut setup)?;
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
fn gpu_warning_acknowledged() -> Result<bool, String> {
    let inherited = env::var_os("MAKEPAD_GPU_ACK").is_some_and(|value| value == "1");
    env::remove_var("MAKEPAD_GPU_ACK");
    if cfg!(target_os = "macos") || (cfg!(target_os = "linux") && inherited) {
        return Ok(true);
    }
    let info = [
        "Makepad Scope heavily uses the GPU to render its UI.".to_owned(),
        "If you have old or broken video drivers, this can result in a system crash.".to_owned(),
    ];
    Screen::enter().acknowledge("GPU driver notice", &info)
}

fn show_menu(setup: &mut Setup) -> Result<(), String> {
    fs::write(setup.root.join("selected-app"), &setup.app).map_err(|e| e.to_string())?;
    let _screen = Screen::enter();
    // Start setup once when Builder opens. Success, cancellation and failure
    // all return to the normal menu; never restart the sequence on redraw.
    let mut startup = true;
    loop {
        let ready = setup.ready();
        let registered_title = catalog::apps()?.iter().find(|app| app.get("id").and_then(makepad_strict_json::Value::as_str) == Some(&setup.app))
            .and_then(|app| app.get("title").and_then(makepad_strict_json::Value::as_str)).map(str::to_owned);
        let title = setup.release.as_ref().map(|r| r.title.clone()).or(registered_title).unwrap_or_else(|| {
            let mut chars = setup.app.chars();
            chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        });
        let mut options: Vec<_> = MENU
            .lines()
            .filter_map(|entry| {
                let fields: Vec<_> = entry.split('|').collect();
                (fields.len() == 3).then(|| {
                    let retry_compiler = setup.compiler_retry && fields[0] == "1";
                    (
                        fields[0].into(),
                        if retry_compiler { "Retry compiler check".into() } else { fields[1].replace("{app}", &title) },
                        if retry_compiler {
                            "Recheck the staged Rust compiler after Windows security scanning".into()
                        } else {
                            fields[2].replace("{app}", &title).replace(
                                "{tools}",
                                if cfg!(windows) {
                                    "Microsoft C++ tools and Windows SDK"
                                } else if cfg!(target_os = "macos") {
                                    "Apple developer tools, SDK and Git"
                                } else {
                                    "Distro development packages"
                                },
                            )
                        },
                    )
                })
            })
            .collect();
        if crate::cuda::supported() {
            options.push(("6".into(), if setup.cuda { "Disable CUDA".into() } else { "Enable CUDA (required for AI app features)".into() }, "NVIDIA compiler and libraries for AI app features".into()));
        }
        let command_key = if crate::cuda::supported() { "7" } else { "6" };
        if setup.root.join(if cfg!(windows) { "scope.exe" } else { "scope.bin" }).is_file() {
            options.push((command_key.into(), "Set up scope command".into(), "Open any project from your terminal".into()));
        }
        let info = [ABOUT.trim().replace("{app}", &title)];
        let heading = "Makepad Builder".to_owned();
        let compiler_ready = ready[0] && ready[1];
        let built = setup.release.as_ref().is_some_and(|release| {
            load_release(&setup.root.join("installed").join(format!("{}.json", setup.app)))
                .is_some_and(|installed| installed.release == release.release)
                && setup.root.join(if cfg!(windows) { runtime::exe(&release.binary) } else { format!("{}.bin", release.binary) }).is_file()
        });
        let mut disabled = Vec::new();
        if !compiler_ready { disabled.push("2"); }
        if !ready.into_iter().all(|v| v) { disabled.push("5"); }
        let choice = if std::mem::take(&mut startup) {
            "1".to_owned()
        } else {
            Screen::enter().choose_state(
                &heading,
                &info,
                &options,
                Some([compiler_ready, built]),
                &disabled,
            )?
        };
        if choice == "q" {
            break;
        }
        let result = match choice.as_str() {
            "1" => setup.install_and_run(),
            "2" => setup.build(),
            "3" => setup.run_app("wm"),
            "4" => setup.apps(),
            "5" => setup.ai_terminal(),
            key if key == command_key => (|| {
                let info = [
                    "Add the Scope command to your user PATH?".into(),
                    if cfg!(windows) { "Use the user environment settings.".into() } else { "Create ~/.local/bin/scope; add ~/.local/bin to your shell profile if needed.".into() },
                    "Only the app command is added; compiler paths stay private.".into(),
                    "This replaces any previous Makepad scope command.".into(),
                    "Reopen Builder after moving this installation to update the command.".into(),
                ];
                if Screen::enter().confirm("Scope from any directory", &info)? {
                    crate::command::install(&setup.root)?;
                    Screen::enter().message("Scope command ready", "In a new terminal, run scope to open the current directory, or scope PATH to open a project.")?;
                }
                Ok(())
            })(),
            "6" => setup.release().and_then(|r| {
                if !crate::cuda::supported() {
                    return Err("The optional CUDA toolkit is available on x64 Windows".into());
                }
                if setup.cuda {
                    setup.cuda = false;
                    activity("CUDA disabled for the next build.");
                    Ok(())
                } else {
                    setup.install_cuda(&r)?;
                    setup.cuda = setup.root.join("toolchain/cuda/bin/nvcc.exe").is_file();
                    Ok(())
                }
            }),
            _ => Ok(()),
        };
        if let Err(error) = result {
            let text = clean(&if setup.email.is_empty() { error } else { error.replace(&setup.email, "[email]") });
            activity(&text);
            Screen::enter().message("Step could not complete", &text)?;
        }
    }
    Ok(())
}
