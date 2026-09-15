//! The setup process owns blocking work. On Windows MpTerm hosts this process
//! and all of its children; Unix bootstraps use the matching terminal menu.
use crate::{
    catalog::{self, Release},
    progress,
    runtime::{self, Dependency, Environment},
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

enum Key {
    Left,
    Right,
    Up,
    Down,
    Enter,
    PageUp,
    PageDown,
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
                    .args(["-icanon", "-echo", "min", "0", "time", "2"])
                    .status()
                    .map_err(|e| e.to_string())?
                    .success()
            {
                Ok(Self(saved))
            } else {
                Err("Cannot read terminal keys".into())
            }
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            let _ = Command::new("stty").arg(&self.0).status();
        }
    }
    pub fn key() -> Result<Key, String> {
        let mut byte = [0];
        if io::stdin().read(&mut byte).map_err(|e| e.to_string())? == 0 {
            return Ok(Key::Other);
        }
        Ok(match byte[0] {
            b'\r' | b'\n' => Key::Enter,
            9 => Key::Right,
            3 => Key::Quit,
            27 => {
                let mut sequence = Vec::new();
                for _ in 0..6 {
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
                    b"[21~" | b"" => Key::Quit,
                    _ => Key::Other,
                }
            }
            c => Key::Char(c as char),
        })
    }
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
}
impl Setup {
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
            progress::stage("Checking sources", "Reading the latest available release", 0.0);
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
        let mut page = apps.iter().position(|a| a.0 == self.app).unwrap_or(0) / 8;
        loop {
            let start = page * 8;
            let mut options: Vec<_> = apps.iter().skip(start).take(8).enumerate().map(|(i, a)| ((i + 1).to_string(), a.1.clone(), a.2.clone())).collect();
            if start + 8 < apps.len() { options.push(("9".into(), "Next page".into(), String::new())); }
            if page > 0 { options.push(("0".into(), "Previous page".into(), String::new())); }
            options.push(("q".into(), "Back".into(), String::new()));
            match Screen::enter().choose("Other Apps", &[format!("Page {} of {} · shared compiler, source and build cache", page + 1, apps.len().div_ceil(8))], &options)?.as_str() {
                "q" => return Ok(()),
                "9" => page += 1,
                "0" => page = page.saturating_sub(1),
                choice => {
                    let index = choice.parse::<usize>().map_err(|_| "Invalid app choice")?;
                    let app = apps.get(start + index - 1).ok_or("Invalid app choice")?;
                    self.app_menu(&app.0)?;
                }
            }
        }
    }
    fn app_menu(&mut self, app: &str) -> Result<(), String> {
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
        let result = show_menu(&mut selected, false);
        fs::write(self.root.join("selected-app"), &self.app).map_err(|e| e.to_string())?;
        result
    }
    fn ready(&self) -> [bool; 3] {
        let tools = if cfg!(windows) {
            crate::msvc::ready(&self.root.join("toolchain/msvc"))
        } else {
            runtime::system_tools_ready().is_ok()
        };
        let rust = self.release.as_ref().is_some_and(|r| {
            let path = runtime::rust_dir(&self.root, &r.rust);
            fs::read_to_string(path.join(".toolchain-version"))
                .is_ok_and(|s| s == format!("{} {}", r.rust, catalog::platform()))
                && path.join("bin").join(runtime::exe("cargo")).is_file()
                && path.join("bin").join(runtime::exe("rustc")).is_file()
        });
        [
            tools,
            rust,
            self.release
                .as_ref()
                .is_some_and(|r| r.installed(&self.root)),
        ]
    }
    fn install_compiler(&self, release: &Release) -> Result<(), String> {
        let ready = self.ready();
        if ready[0] && ready[1] {
            activity("Compiler is already installed.");
            return Ok(());
        }
        let mut info = vec![format!("Compiler tools and Rust {}", release.rust)];
        if cfg!(windows) {
            info.extend([
                "Accept the Microsoft Build Tools and Windows SDK terms:".into(),
                "https://visualstudio.microsoft.com/license-terms/vs2022-ga-diagnosticbuildtools/".into(),
                "https://learn.microsoft.com/legal/windows-sdk/windows-sdk-license".into(),
            ]);
        } else if cfg!(target_os = "macos") {
            info.push("Apple developer tools must be installed and licensed.".into());
        } else {
            info.push("System development packages use your distro package manager.".into());
        }
        info.extend([
            "Rust: MIT and Apache 2.0 license notices".into(),
            "https://www.rust-lang.org/policies/licenses".into(),
            "Install the compiler privately in this folder?".into(),
        ]);
        if !Screen::enter().confirm("Compiler license terms", &info)? {
            activity("Compiler installation cancelled.");
            return Ok(());
        }
        if !cfg!(windows) && !ready[0] {
            let _pause = Screen::pause();
            runtime::setup_system_tools()?;
        }
        with_progress(|| {
            if cfg!(windows) && !ready[0] {
                runtime::dependency(&self.root, release, if cfg!(windows) { Dependency::Msvc } else { Dependency::System })?;
            }
            if !ready[1] {
                runtime::dependency(&self.root, release, Dependency::Rust)?;
            }
            Ok(())
        })
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
        if !self.ready()[1] {
            return Err(format!("The latest source requires Rust {}. Choose Download compiler first.", release.rust));
        }
        with_progress(|| {
            catalog::checkout(&self.service, &self.email, &self.root, &release)?;
            Environment::prepare(&self.root, &release, self.cuda)?.build(&release)
        })?;
        fs::create_dir_all(self.root.join("installed")).map_err(|e| e.to_string())?;
        release.save(&self.root.join("installed").join(format!("{}.json", release.id)))?;
        release.save(&self.root.join("installed-release.json"))?;
        fs::write(
            self.root.join("installed-cuda"),
            if self.cuda { "1" } else { "0" },
        )
        .map_err(|e| e.to_string())?;
        activity("Build complete. Opening the app.");
        self.open()
    }
    fn environment(&self, command: &mut Command) -> Result<(), String> {
        if let Some(release) = &self.release {
            let environment = Environment::prepare(&self.root, &release, self.cuda)?;
            let context = crate::agent::write_context(&release, &environment)?;
            command.envs(environment.vars).env("MAKEPAD_AGENT_CONTEXT", context);
        }
        command
            .env_remove("MAKEPAD_LOADER_EMAIL")
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .current_dir(&self.project);
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
        let mut app = Command::new(environment.app_binary(&release));
        runtime::hide_console(&mut app);
        let log_path = environment.build.join("builder-app.log");
        let log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
        activity(&format!("Running {}", release.title));
        let status = app
            .args(["--cwd", &project.to_string_lossy()])
            .current_dir(&project)
            .envs(environment.vars)
            .env_remove("MAKEPAD_LOADER_EMAIL")
            .env_remove("RUSTUP_TOOLCHAIN")
            .stdin(Stdio::null())
            .stderr(log.try_clone().map_err(|e| e.to_string())?)
            .stdout(log)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("App exited {status}; see {}", log_path.display()))
        }
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
    let root = crate::default_root();
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
            .unwrap_or(env::current_dir().map_err(|e| e.to_string())?)
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
        root,
    };
    setup.release = load_release(&setup.root.join("available/scope.json"))
        .or_else(|| load_release(&setup.root.join("latest.json")).filter(|r| r.id == "scope"))
        .or_else(|| load_release(&setup.root.join("installed/scope.json")));
    show_menu(&mut setup, true)?;
    if io::stdout().is_terminal() { print!("\x1b[0m"); }
    println!("Makepad Builder closed.");
    Ok(())
}

fn show_menu(setup: &mut Setup, primary: bool) -> Result<(), String> {
    fs::write(setup.root.join("selected-app"), &setup.app).map_err(|e| e.to_string())?;
    let _screen = Screen::enter();
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
            .filter(|entry| primary || !(entry.starts_with("3|") || entry.starts_with("4|")))
            .filter_map(|entry| {
                let fields: Vec<_> = entry.split('|').collect();
                (fields.len() == 3).then(|| {
                    (
                        if !primary && fields[0] == "5" { "3".into() } else { fields[0].into() },
                        fields[1].replace("{app}", &title),
                        fields[2].replace("{app}", &title).replace(
                            "{tools}",
                            if cfg!(windows) {
                                "Microsoft C++ tools and Windows SDK"
                            } else if cfg!(target_os = "macos") {
                                "Apple developer tools, SDK and Git"
                            } else {
                                "Distro development packages"
                            },
                        ),
                    )
                })
            })
            .collect();
        if crate::cuda::supported() {
            options.push(((if primary { "6" } else { "4" }).into(), if setup.cuda { "Disable CUDA".into() } else { "Enable CUDA (required for AI app features)".into() }, "NVIDIA compiler and libraries for AI app features".into()));
        }
        let command_key = if crate::cuda::supported() { "7" } else { "6" };
        if primary && setup.root.join(if cfg!(windows) { "scope.exe" } else { "scope.bin" }).is_file() {
            options.push((command_key.into(), "Set up scope command".into(), "Open any project from your terminal".into()));
        }
        if !primary { options.push(("q".into(), if setup.app == "wm" { "Back to Builder".into() } else { "Back to Other Apps".into() }, String::new())); }
        let info = [ABOUT.trim().replace("{app}", &title)];
        let heading = if primary { "Makepad Builder".to_owned() } else { format!("Makepad Builder · {title}") };
        let compiler_ready = ready[0] && ready[1];
        let built = setup.release.as_ref().is_some_and(|release| {
            load_release(&setup.root.join("installed").join(format!("{}.json", setup.app)))
                .is_some_and(|installed| installed.release == release.release)
                && setup.root.join(if cfg!(windows) { runtime::exe(&release.binary) } else { format!("{}.bin", release.binary) }).is_file()
        });
        let mut disabled = Vec::new();
        if !compiler_ready { disabled.push("2"); }
        if !ready.into_iter().all(|v| v) { disabled.push(if primary { "5" } else { "3" }); }
        let choice = Screen::enter().choose_state(
            &heading,
            &info,
            &options,
            Some([compiler_ready, built]),
            &disabled,
        )?;
        if choice == "q" {
            break;
        }
        let result = match choice.as_str() {
            "1" => setup.release().and_then(|r| setup.install_compiler(&r)),
            "2" => setup.build(),
            "3" if primary => setup.apps(),
            "4" if primary => setup.app_menu("wm"),
            key if key == (if primary { "5" } else { "3" }) => setup.ai_terminal(),
            key if primary && key == command_key => (|| {
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
            key if key == (if primary { "6" } else { "4" }) => setup.release().and_then(|r| {
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
