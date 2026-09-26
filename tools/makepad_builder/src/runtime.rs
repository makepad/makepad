//! Private toolchain paths shared by the compiler and the embedded terminal.
use crate::{
    catalog::{self, Release},
    progress,
};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{atomic::{AtomicU64, Ordering}, Arc},
};

#[derive(Clone, Copy, Debug)]
pub enum Dependency {
    Rust,
    Msvc,
    Cuda,
    System,
}

pub fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    }
}
pub fn rust_dir(root: &Path, version: &str) -> PathBuf {
    root.join("toolchain/rust")
        .join(format!("{version}-{}", rust_triple(root)))
}

/// The GNU toolchain Rust publishes for Windows: rustc, cargo, the standard
/// library and rust-mingw (MinGW's runtime and import libraries), linked with
/// Rust's own LLD. What makepad-builder.bat downloads to compile the Builder
/// itself, and one of the two ways apps are compiled here.
pub const GNU_TRIPLE: &str = "x86_64-pc-windows-gnu";

/// Windows: how apps are compiled, `<root>/selected-compiler`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowsChain {
    /// Not asked yet.
    Undecided,
    /// Rust's GNU toolchain: nothing else to install. No CUDA.
    Gnu,
    /// Microsoft's C++ Build Tools and the Windows SDK with the MSVC Rust,
    /// which CUDA's compiler needs.
    Msvc,
}

pub fn windows_chain(root: &Path) -> WindowsChain {
    if !cfg!(windows) {
        return WindowsChain::Undecided;
    }
    match fs::read_to_string(root.join("selected-compiler")).unwrap_or_default().trim() {
        "gnu" => WindowsChain::Gnu,
        "msvc" => WindowsChain::Msvc,
        // Installations from before the choice have the Microsoft tools.
        _ if crate::msvc::ready(&root.join("toolchain/msvc")) => WindowsChain::Msvc,
        // Nobody is asked at the start: Rust's GNU toolchain (already there,
        // makepad-builder.bat downloaded it) until the person picks
        // Microsoft's tools for local AI when building an app with CUDA.
        _ => WindowsChain::Gnu,
    }
}

pub fn record_windows_chain(root: &Path, chain: WindowsChain) -> Result<(), String> {
    let text = match chain {
        WindowsChain::Gnu => "gnu",
        WindowsChain::Msvc => "msvc",
        WindowsChain::Undecided => return Err("An undecided compiler is not recorded".into()),
    };
    let next = root.join("selected-compiler.new");
    fs::write(&next, format!("{text}\n")).map_err(|e| format!("Record the compiler: {e}"))?;
    fs::rename(&next, root.join("selected-compiler")).map_err(|e| format!("Record the compiler: {e}"))
}

/// The Rust this installation compiles with: the GNU one when that is the
/// Windows choice, else this platform's (MSVC on Windows).
pub fn rust_triple(root: &Path) -> &'static str {
    if cfg!(windows) && windows_chain(root) == WindowsChain::Gnu {
        GNU_TRIPLE
    } else {
        catalog::platform()
    }
}

/// The macOS/Linux compiler choice recorded in `<root>/selected-rust` by the
/// bootstrap or Builder. Windows always uses the private pinned Rust and
/// never reads this file.
#[derive(Clone, Debug, PartialEq)]
pub enum RustChoice {
    /// Not asked yet: use the private toolchain; an installed Rust may be offered.
    Undecided,
    /// The user declined the installed Rust; never offer it again.
    Private,
    /// Absolute sysroot of the installed Rust the user chose explicitly.
    External(String),
}

pub fn rust_choice(root: &Path) -> Result<RustChoice, String> {
    if cfg!(windows) { return Ok(RustChoice::Private); }
    match fs::read_to_string(root.join("selected-rust")) {
        Ok(text) => {
            let text = text.trim();
            Ok(if text == "private" {
                RustChoice::Private
            } else if text.starts_with('/') {
                RustChoice::External(text.to_owned())
            } else {
                RustChoice::Undecided
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RustChoice::Undecided),
        Err(error) => Err(format!("Read selected Rust: {error}")),
    }
}

pub fn record_rust_choice(root: &Path, choice: &RustChoice) -> Result<(), String> {
    if cfg!(windows) { return Err("Windows always uses the private Rust toolchain".into()); }
    let file = root.join("selected-rust");
    let text = match choice {
        // "Not asked yet" is the absence of a choice; nothing records it.
        RustChoice::Undecided => return Err("An undecided Rust choice is not recorded".into()),
        RustChoice::Private => "private".to_owned(),
        RustChoice::External(sysroot) => sysroot.clone(),
    };
    let next = root.join("selected-rust.new");
    fs::write(&next, format!("{text}\n")).map_err(|e| format!("Record selected Rust: {e}"))?;
    fs::rename(&next, &file).map_err(|e| format!("Record selected Rust: {e}"))
}

// The shell helpers macOS and Linux run (Windows never does, so its
// source ships without them).
#[cfg(not(windows))]
const RUST_TOOLS: &str = include_str!("../rust-tools.sh");
#[cfg(windows)]
const RUST_TOOLS: &str = "";
#[cfg(not(windows))]
const CHECK_TOOLS: &str = include_str!("../check-tools.sh");
#[cfg(windows)]
const CHECK_TOOLS: &str = "";

/// Run the shared read-only helper. Ok carries the canonical sysroot; Err the
/// one-line reason. Only the toolchain's own binaries are executed.
fn rust_tools(arguments: &[&str]) -> Result<PathBuf, String> {
    let output = Command::new("sh")
        .args(["-c", RUST_TOOLS, "makepad-rust"])
        .args(arguments)
        .output()
        .map_err(|e| format!("Check installed Rust: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && text.starts_with('/') {
        Ok(PathBuf::from(text))
    } else if text.is_empty() {
        Err("No installed Rust was found on PATH.".into())
    } else {
        Err(text)
    }
}

/// Find the user's default installed Rust if it satisfies `version` on this host.
pub fn probe_rust(version: &str) -> Result<PathBuf, String> {
    if cfg!(windows) { return Err("Windows always uses the private Rust toolchain".into()); }
    rust_tools(&["--probe", version, catalog::platform()])
}

/// Re-check a recorded sysroot without modifying it.
pub fn validate_rust(version: &str, sysroot: &str) -> Result<PathBuf, String> {
    if cfg!(windows) { return Err("Windows always uses the private Rust toolchain".into()); }
    rust_tools(&["--validate", version, catalog::platform(), sysroot])
}

/// Resolve the compiler for this installation: `(sysroot, external)`. An
/// external choice is validated every time; a stale one is an error so callers
/// can offer remediation instead of silently falling back.
pub fn selected_rust(root: &Path, version: &str) -> Result<(PathBuf, bool), String> {
    match rust_choice(root)? {
        RustChoice::External(recorded) => validate_rust(version, &recorded)
            .map(|sysroot| (sysroot, true))
            .map_err(|reason| format!("The selected Rust cannot be used: {reason} Choose Download compiler to pick a compiler.")),
        RustChoice::Private | RustChoice::Undecided => Ok((rust_dir(root, version), false)),
    }
}

pub fn rust_ready(root: &Path, version: &str) -> bool {
    let Ok((path, external)) = selected_rust(root, version) else { return false; };
    (external || fs::read_to_string(path.join(".toolchain-version"))
        .is_ok_and(|s| s == format!("{version} {}", rust_triple(root))))
        && path.join("bin").join(exe("cargo")).is_file()
        && path.join("bin").join(exe("rustc")).is_file()
}

/// Same checks as the auditable Unix bootstrap; never install during a probe.
pub fn system_tools_ready() -> Result<(), String> {
    if cfg!(windows) { return Ok(()); }
    let output = Command::new("sh")
        .args(["-c", CHECK_TOOLS, "makepad-tools", "--check"])
        .output().map_err(|e| format!("Check system tools: {e}"))?;
    if !output.status.success() {
        return Err("System developer tools are not ready. Choose Download compiler to finish platform setup.".into());
    }
    Ok(())
}

pub fn setup_system_tools() -> Result<(), String> {
    if cfg!(windows) { return Ok(()); }
    let status = Command::new("sh")
        .args(["-c", CHECK_TOOLS, "makepad-tools"])
        .status().map_err(|e| format!("Set up system tools: {e}"))?;
    if !status.success() { return Err("System tool setup did not complete".into()); }
    system_tools_ready()
}

pub fn dependency(root: &Path, release: &Release, kind: Dependency) -> Result<String, String> {
    dependencies(root, release, &[kind])?;
    Ok(format!("{kind:?} ready"))
}

/// Install the missing ones of `kinds` side by side (see `jobs::run`):
/// Msvc is two components, Build tools and Windows SDK, sharing one catalog.
/// Their progress arrives as one row per component.
pub fn dependencies(root: &Path, release: &Release, kinds: &[Dependency]) -> Result<(), String> {
    let root = crate::validate_install_root(root)?;
    let cache = root.join("cache");
    let tools = root.join("toolchain");
    let (msvc, cuda) = (tools.join("msvc"), tools.join("cuda"));
    let catalog = crate::msvc::Catalog::default();
    let mut jobs = Vec::new();
    for kind in kinds {
        match kind {
            Dependency::Rust => {
                let (rust, external) = selected_rust(&root, &release.rust)?;
                if !external {
                    let (cache, version) = (&cache, &release.rust);
                    let triple = rust_triple(&root);
                    jobs.push(crate::jobs::Job::new("Rust", move || crate::rustc::install_version_for(cache, &rust, version, triple)));
                }
            }
            Dependency::Msvc => {
                if !cfg!(all(windows, target_arch = "x86_64")) {
                    return Err("MSVC setup is only available on x64 Windows".into());
                }
                jobs.extend(crate::msvc::install_jobs(&cache, &msvc, &catalog, false));
            }
            Dependency::Cuda => {
                if !crate::cuda::supported() {
                    return Err("The optional CUDA toolkit is available on x64 Windows".into());
                }
                let (cache, cuda) = (&cache, &cuda);
                jobs.push(crate::jobs::Job::new("CUDA", move || crate::cuda::install(cache, cuda)));
            }
            Dependency::System => system_tools_ready()?,
        }
    }
    crate::jobs::run(jobs)
}

#[derive(Clone, Debug)]
pub struct Environment {
    pub root: PathBuf,
    pub vars: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub script: PathBuf,
    pub build: PathBuf,
}

impl Environment {
    pub fn prepare(root: &Path, release: &Release, cuda: bool) -> Result<Self, String> {
        let absolute_root = crate::validate_install_root(root)?;
        system_tools_ready()?;
        let root = absolute_root.as_path();
        let (rust, external) = selected_rust(root, &release.rust)?;
        if !rust.join("bin").join(exe("cargo")).is_file() {
            return Err("Install the pinned Rust toolchain first".into());
        }
        if !external {
            crate::rustc::prepare_host_tools(&rust)?;
        }
        let gnu = cfg!(windows) && windows_chain(root) == WindowsChain::Gnu;
        if gnu && cuda {
            return Err("CUDA needs Microsoft's C++ tools: choose Build tools to switch".into());
        }
        let mut vars = BTreeMap::new();
        // One target directory per source snapshot, shared by all its apps.
        let build = release.prepare_target_dir(root)?;
        let tmp = root.join("tmp");
        let cargo = root.join("cargo-home");
        for d in [&build, &tmp, &cargo] {
            fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut path = vec![rust.join("bin"), cargo.join("bin")];
        for (k, v) in [
            ("CARGO_HOME", cargo),
            ("CARGO_TARGET_DIR", build.clone()),
            ("RUSTC", rust.join("bin").join(exe("rustc"))),
            ("CARGO", rust.join("bin").join(exe("cargo"))),
            ("TEMP", compiler_temp(&tmp)),
            ("TMP", compiler_temp(&tmp)),
            ("TMPDIR", compiler_temp(&tmp)),
        ] {
            vars.insert(k.into(), v.to_string_lossy().into_owned());
        }
        // rustdoc is not on the build path; point at it only when the selected
        // toolchain ships it, so an inherited RUSTDOC never leaks in.
        let rustdoc = rust.join("bin").join(exe("rustdoc"));
        vars.insert("RUSTDOC".into(), if rustdoc.is_file() { rustdoc.to_string_lossy().into_owned() } else { String::new() });
        vars.insert("MAKEPAD_LOADER_EMAIL".into(), String::new());
        vars.insert("RUSTUP_TOOLCHAIN".into(), String::new());
        // Any rustup proxy reached later on PATH fails closed instead of
        // downloading into, or switching, the user's global rustup.
        vars.insert("RUSTUP_AUTO_INSTALL".into(), "0".into());
        vars.insert(
            "RUSTUP_HOME".into(),
            root.join("rustup-home").to_string_lossy().into_owned(),
        );
        vars.insert("CARGO_TERM_PROGRESS_WHEN".into(), "always".into());
        vars.insert("CARGO_TERM_PROGRESS_WIDTH".into(), "100".into());
        vars.insert("CARGO_TERM_COLOR".into(), "always".into());
        vars.insert("MAKEPAD_PACKAGE_DIR".into(), ".".into());
        vars.insert("MAKEPAD_BUILDER_APP".into(), release.id.clone());
        vars.insert("CUDA_PATH".into(), String::new());
        vars.insert("CUDA_HOME".into(), String::new());
        vars.insert("CUDACXX".into(), String::new());
        vars.insert(
            "MAKEPAD_GGML_NO_CUDA".into(),
            if cuda { "0" } else { "1" }.into(),
        );
        vars.insert(
            "MAKEPAD_GGML_REQUIRE_CUDA".into(),
            if cuda { "1" } else { "0" }.into(),
        );
        if gnu {
            // Rust's LLD links with Rust's MinGW runtime; the import libraries
            // of the system DLLs come from the Builder answering as dlltool
            // (implib.rs), first on the build's PATH.
            let tools = root.join("toolchain/gnu-tools");
            let dlltool = tools.join("dlltool.exe");
            let me = env::current_exe().map_err(|e| e.to_string())?;
            let same = fs::metadata(&dlltool).ok().map(|m| m.len()) == fs::metadata(&me).ok().map(|m| m.len());
            if !same {
                fs::create_dir_all(&tools).map_err(|e| e.to_string())?;
                let next = tools.join("dlltool.exe.new");
                fs::copy(&me, &next).map_err(|e| format!("Prepare dlltool: {e}"))?;
                // A dlltool that a build is running stays; the next start retries.
                let _ = fs::rename(&next, &dlltool);
            }
            path.insert(0, tools);
            // Plain `#[link(name = "dwmapi")]` DLLs MinGW's runtime has no
            // import library for.
            let libraries = rust.join("lib/rustlib").join(GNU_TRIPLE).join("lib/self-contained");
            // The whole source snapshot: an app's path dependencies reach
            // outside its own workspace.
            let source = release.source(root);
            let sources = root.join("sources");
            let snapshot = source
                .strip_prefix(&sources)
                .ok()
                .and_then(|rest| rest.components().next())
                .map(|first| sources.join(first))
                .unwrap_or(source);
            crate::implib::prepare(&libraries, &snapshot)?;
            let system = env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            path.push(system.join("System32"));
            path.push(system.clone());
            vars.insert("SystemRoot".into(), system.to_string_lossy().into_owned());
            vars.insert("MAKEPAD_BUILDER_LINK".into(), "gnu".into());
        } else if cfg!(windows) {
            let linker = rust.join("lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe");
            if !linker.is_file() {
                return Err("The private Rust toolchain is missing its bundled LLD linker".into());
            }
            vars.insert("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER".into(), linker.to_string_lossy().into_owned());
            let msvc = root.join("toolchain/msvc");
            let bin = crate::msvc::msvc_bin_dir(&msvc)
                .ok_or("Install Microsoft Build Tools + Windows SDK first")?;
            vars.insert("NVCC_CCBIN".into(), bin.to_string_lossy().into_owned());
            path.push(bin);
            let mut includes = Vec::new();
            let mut libs = Vec::new();
            let tools =
                crate::msvc::msvc_root_tools(&msvc).ok_or("Incomplete MSVC installation")?;
            includes.push(tools.join("include"));
            libs.push(tools.join("lib/x64"));
            let kits = msvc.join("Windows Kits/10");
            let latest = |p: &Path| -> Result<PathBuf, String> {
                let mut entries: Vec<_> = fs::read_dir(p)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect();
                entries.sort();
                entries.pop().ok_or_else(|| "Missing Windows SDK".into())
            };
            let inc = latest(&kits.join("Include"))?;
            let lib = latest(&kits.join("Lib"))?;
            for p in ["ucrt", "shared", "um", "winrt"] {
                includes.push(inc.join(p));
            }
            for p in ["ucrt/x64", "um/x64"] {
                libs.push(lib.join(p));
            }
            vars.insert("INCLUDE".into(), join_paths(&includes)?);
            vars.insert("LIB".into(), join_paths(&libs)?);
            let system = env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            path.push(system.join("System32"));
            path.push(system.clone());
            vars.insert("SystemRoot".into(), system.to_string_lossy().into_owned());
        } else {
            path.extend([
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/sbin"),
                PathBuf::from("/sbin"),
            ]);
        }
        // Pinned: the shell's RUSTFLAGS would otherwise change every
        // fingerprint in the shared target.
        compiler_flags(&mut vars, frontend_threads(root));
        if cuda {
            let cuda = root.join("toolchain/cuda");
            if !crate::cuda::supported() || !cuda.join("bin").join(exe("nvcc")).is_file() {
                return Err("Install optional CUDA first, or turn it off".into());
            }
            path.insert(1, cuda.join("bin"));
            // CUDA 13 on Windows keeps its runtime DLLs (cudart, cuBLAS,
            // cuBLASLt) in bin\x64, not bin: without it on PATH a launched
            // app stops with "cublas64_13.dll was not found".
            if cuda.join("bin").join("x64").is_dir() {
                path.insert(2, cuda.join("bin").join("x64"));
            }
            vars.insert("CUDA_PATH".into(), cuda.to_string_lossy().into_owned());
            // The only toolkit a build may link: this installation's own.
            vars.insert("MAKEPAD_CUDA_ROOT".into(), cuda.to_string_lossy().into_owned());
        }
        // The selected compiler takes precedence; installed shells and agents
        // remain reachable without changing the parent or global PATH.
        if let Some(inherited) = env::var_os("PATH") {
            path.extend(env::split_paths(&inherited));
        }
        vars.insert("PATH".into(), join_paths(&path)?);
        // The installation root is canonical, so on Windows every path in
        // here carries the verbatim `\\?\` prefix. Rust and Cargo take it;
        // cl.exe does not: with it in INCLUDE, nvcc's host compiler cannot
        // open crtdefs.h and every CUDA kernel fails to build (measured on
        // the Windows box; a space or parentheses in the folder are fine).
        if cfg!(windows) {
            for value in vars.values_mut() {
                *value = crate::plain_paths(value);
            }
        }
        let cwd = release.source(root);
        let script = release.directory(root).join(if cfg!(windows) {
            "environment.bat"
        } else {
            "environment.sh"
        });
        let out = Self {
            root: root.to_path_buf(),
            vars,
            cwd,
            script,
            build,
        };
        out.write()?;
        Ok(out)
    }
    fn write(&self) -> Result<(), String> {
        let mut s = if cfg!(windows) {
            "@echo off\r\n".to_string()
        } else {
            "# Makepad Builder compiler environment; source in this shell only.\n".to_string()
        };
        for (key, value) in &self.vars {
            if cfg!(windows) {
                s += &format!("set \"{key}={}\"\r\n", batch(value)?);
            } else {
                s += &format!("export {key}={}\n", shell(value));
            }
        }
        if cfg!(windows) {
            s += &format!("cd /d \"{}\"\r\n", batch(&self.cwd.to_string_lossy())?);
        } else {
            s += &format!("cd {}\n", shell(&self.cwd.to_string_lossy()));
        }
        fs::write(&self.script, s).map_err(|e| e.to_string())
    }
    pub fn terminal_command(&self) -> String {
        if cfg!(windows) {
            format!("cmd /d /v:off /k call \"{}\"", self.script.display())
        } else {
            format!(
                "/bin/bash --noprofile --rcfile {} -i",
                shell(&self.script.to_string_lossy())
            )
        }
    }
    pub fn build_command(&self, release: &Release, remote: bool) -> String {
        let binary = self.app_binary(release);
        let command = format!(
            "cargo build --release -p {} --bin {} --no-default-features{}",
            release.package, release.binary, if release.features.is_empty() { String::new() } else { format!(" --features {}", release.features.join(",")) }
        );
        if cfg!(windows) {
            format!(
                "cd /d \"{}\" && {command} && \"{}\"{}",
                self.cwd.display(),
                binary.display(),
                if remote { " --remote" } else { "" }
            )
        } else {
            format!(
                "cd {} && {command} && {}{}",
                shell(&self.cwd.to_string_lossy()),
                shell(&binary.to_string_lossy()),
                if remote { " --remote" } else { "" }
            )
        }
    }
    pub fn build(&self, release: &Release) -> Result<(), String> {
        let cargo = self.vars.get("CARGO").ok_or("Missing Cargo path")?;
        // Windows executables carry the app's icon and name as resources.
        // `cargo rustc` passes the linker input to the final link only, so
        // dependencies in the shared target keep their fingerprints.
        #[cfg(windows)]
        let resources = Some(crate::app_icon::windows_resources(&self.root, &self.build, release)?);
        #[cfg(not(windows))]
        let resources: Option<PathBuf> = None;
        let command = |vars: &BTreeMap<String, String>| {
            let mut cmd = Command::new(cargo);
            cmd.current_dir(&self.cwd).envs(vars);
            isolate(&mut cmd);
            cmd.env("MAKEPAD_PACKAGE_DIR", ".")
                .args([
                    if resources.is_some() { "rustc" } else { "build" },
                    "--release",
                    "--message-format=json-render-diagnostics",
                    "-p",
                    &release.package,
                    "--bin",
                    &release.binary,
                ]);
            // The pinned repository commits are the lock: Cargo may bring the
            // lockfile in line with the pinned path crates (--locked failed when an
            // app's own lock went stale). No --offline: the graph names crates for
            // other targets (Android, OpenHarmony) that are never built here, and
            // resolving them needs their registry entries, which a new private
            // CARGO_HOME does not have yet. Once cached, Cargo stays off the network.
            // A release is the app without its development defaults (the design
            // overlay, `tweaker`); the catalog names every feature it ships with.
            cmd.arg("--no-default-features");
            if !release.features.is_empty() { cmd.args(["--features", &release.features.join(",")]); }
            if let Some(resources) = &resources {
                cmd.args(["--", "-C"]).arg(format!("link-arg={}", resources.display()));
                // A windowed program: started from Explorer it opens no console
                // window. stdin/stdout still work through pipes (an MCP client
                // starting `--mcp`), and from a terminal the app joins that
                // terminal's console (platform attach_parent_console).
                if vars.get("MAKEPAD_BUILDER_LINK").is_some_and(|l| l == "gnu") {
                    cmd.args(["-C", "link-arg=--subsystem", "-C", "link-arg=windows"]);
                } else {
                    cmd.args(["-C", "link-arg=/SUBSYSTEM:WINDOWS", "-C", "link-arg=/ENTRY:mainCRTStartup"]);
                }
            }
            cmd
        };
        let mut features = vec!["--no-default-features".to_owned()];
        if !release.features.is_empty() { features.extend(["--features".to_owned(), release.features.join(",")]); }
        let record = self.build.join(format!("{}.crates", release.binary));
        // The build starts at once; without a count from an earlier build
        // the total is worked out beside it (cargo tree and metadata can take
        // a while) and the bar gets its end when that is known.
        let expected = Arc::new(AtomicU64::new(recorded_crates(&record).unwrap_or(0)));
        if expected.load(Ordering::Relaxed) == 0 {
            let (cargo, cwd, vars, package, triple, total) = (cargo.clone(), self.cwd.clone(), self.vars.clone(), release.package.clone(), self.triple(), expected.clone());
            let features = features.clone();
            std::thread::spawn(move || {
                let features: Vec<&str> = features.iter().map(String::as_str).collect();
                total.store(expected_crates(&cargo, &cwd, &vars, &package, &features, triple), Ordering::Relaxed);
            });
        }
        let log = self.build.join("builder-build.log");
        let (mut status, mut crates) = self.cargo_logged(&log, &expected, &command)?;
        // CUDA kernels that do not compile here must not cost the person the
        // app: build it once more without them (only the AI crates rebuild),
        // and when that works keep this installation CPU-only from now on,
        // so later builds pass the same flags and reuse the target.
        if !status.success() && self.vars.get("MAKEPAD_GGML_REQUIRE_CUDA").is_some_and(|v| v == "1") {
            if let Some(reason) = cuda_kernels_failure(&log) {
                let _ = fs::copy(&log, log.with_extension("cuda.log"));
                note(&format!("The CUDA kernels did not compile ({reason}); building again without CUDA."));
                let mut cpu = self.vars.clone();
                cpu.insert("MAKEPAD_GGML_NO_CUDA".into(), "1".into());
                cpu.insert("MAKEPAD_GGML_REQUIRE_CUDA".into(), "0".into());
                (status, crates) = self.cargo_logged(&log, &expected, |_| command(&cpu))?;
                if status.success() {
                    let _ = fs::write(
                        self.root.join(crate::cuda::KERNELS_FAILED_FILE),
                        format!("{reason}\nThe CUDA kernels failed to compile here (see {}), so apps build without CUDA and AI features run on the CPU. Delete this file to try CUDA again.\n",
                            crate::shown(&log.with_extension("cuda.log"))),
                    );
                    let _ = Environment { vars: cpu, ..self.clone() }.write();
                    note("CUDA kernels failed to build; AI features run on the CPU (see cuda-kernels-failed).");
                }
            }
        }
        if status.success() {
            let _ = fs::write(&record, crates.to_string());
        }
        if !status.success() {
            return Err(format!("Build failed: {status}"));
        }
        self.publish_app(release)?;
        Ok(())
    }

    /// The crates `cargo build -p package` produces on this platform, from
    /// `cargo metadata`: every package reachable through normal and build
    /// dependencies, plus its build script, and the package's own binary.
    /// Artifacts Cargo reports as fresh count too. 0 when unknown.
    /// The triple this environment's Rust builds for (its host): on Windows
    /// the GNU or MSVC one the installation chose.
    fn triple(&self) -> &'static str {
        if cfg!(windows) { rust_triple(&self.root) } else { crate::catalog::platform() }
    }


    /// Run the Cargo build `command` makes from a set of variables, logged to
    /// `log`. rustc's parallel frontend is still unstable upstream: when it
    /// crashes, runs out of memory or stops making progress, the same build
    /// runs once more with the serial frontend (the flags this installation
    /// had before), and when that succeeds `rustc-threads` records it, so
    /// every later build here stays serial and the target stays fresh. A
    /// build that fails serially too is an ordinary failure and changes
    /// nothing. The activity log says what happened.
    fn cargo_logged(&self, log: &Path, expected: &Arc<AtomicU64>, command: impl Fn(&BTreeMap<String, String>) -> Command) -> Result<(std::process::ExitStatus, u64), String> {
        let threads = frontend_threads_in(&self.vars);
        if threads <= 1 {
            let run = run_build_logged(&mut command(&self.vars), log, expected, None)?;
            return Ok((run.status, run.crates));
        }
        // CUDA kernels (nvcc in a build script) can compile quietly for a
        // long time; nothing else a build runs comes close to 10 minutes.
        let quiet = std::time::Duration::from_secs(if self.vars.get("MAKEPAD_GGML_REQUIRE_CUDA").is_some_and(|v| v == "1") { 3600 } else { 600 });
        let run = run_build_logged(&mut command(&self.vars), log, expected, Some(quiet))?;
        if run.status.success() {
            return Ok((run.status, run.crates));
        }
        let reason = if run.stalled {
            format!("made no progress for {} minutes", quiet.as_secs() / 60)
        } else if let Some(reason) = frontend_failure(log) {
            reason
        } else {
            return Ok((run.status, run.crates));
        };
        let _ = fs::copy(log, log.with_extension("parallel.log"));
        note(&format!("The parallel Rust compiler ({threads} threads) {reason}; compiling again with one thread."));
        let mut serial = self.vars.clone();
        compiler_flags(&mut serial, 1);
        let retry = run_build_logged(&mut command(&serial), log, expected, None)?;
        if retry.status.success() {
            let _ = fs::write(
                self.root.join(FRONTEND_THREADS_FILE),
                format!("1\nSerial since the parallel Rust compiler ({threads} threads) {reason} and the serial one then compiled the same build. Delete this file to try the parallel compiler again.\n"),
            );
            // Shells and agents started from here on build like this too.
            let _ = Environment { vars: serial, ..self.clone() }.write();
            note("Compiled with one thread. Builds here stay on one thread from now on (see rustc-threads).");
        }
        Ok((retry.status, retry.crates))
    }

    /// Where a built app is published: on Windows the executable itself in
    /// the installation folder people see (it is what they open); on macOS
    /// and Linux `<binary>.bin` in the Builder's folder, started by the
    /// `<binary>` command (and the macOS bundle) in the installation folder.
    pub fn app_binary(&self, release: &Release) -> PathBuf {
        if cfg!(windows) {
            crate::home_of(&self.root).join(exe(&release.binary))
        } else {
            self.root.join(format!("{}.bin", release.binary))
        }
    }

    /// A published app built with CUDA links cudart and cuBLAS dynamically,
    /// and Windows looks for DLLs beside the executable (and on PATH), not
    /// in this installation's toolkit. So the toolkit's runtime DLLs are put
    /// beside the executable: hard links (no second copy of the 435 MB
    /// cuBLASLt), copies where a link is refused. Started from Explorer or a
    /// shortcut, the app then finds them as well as from the Builder.
    #[cfg(windows)]
    fn publish_cuda_runtime(&self, binary: &Path) -> Result<(), String> {
        if self.vars.get("CUDA_PATH").is_none_or(|p| p.is_empty()) {
            return Ok(());
        }
        let runtime = self.root.join("toolchain/cuda/bin/x64");
        let Ok(entries) = fs::read_dir(&runtime) else { return Ok(()) };
        let beside = binary.parent().ok_or("Missing installation folder")?;
        for entry in entries.flatten() {
            let from = entry.path();
            if from.extension().is_none_or(|e| !e.eq_ignore_ascii_case("dll")) { continue; }
            let to = beside.join(entry.file_name());
            let same = fs::metadata(&to).ok().zip(entry.metadata().ok()).is_some_and(|(a, b)| a.len() == b.len());
            if same { continue; }
            let _ = fs::remove_file(&to);
            if fs::hard_link(&from, &to).is_err() {
                fs::copy(&from, &to).map_err(|e| format!("CUDA runtime {}: {e}", entry.file_name().to_string_lossy()))?;
            }
        }
        Ok(())
    }
    fn publish_app(&self, release: &Release) -> Result<(), String> {
        use makepad_strict_json::{self as json, Value};
        progress::stage("Finishing", "Linking resources to the downloaded source", 0.0);
        use std::io::BufRead;
        let root = self.root.canonicalize().map_err(|e| e.to_string())?;
        let sources = release.directory(&root);
        // Map rows are relative to the executable's folder.
        let binary = self.app_binary(release);
        let beside = binary.parent().ok_or("Missing installation folder")?.canonicalize().map_err(|e| e.to_string())?;
        let log = fs::File::open(self.build.join("builder-build.log")).map_err(|e| e.to_string())?;
        let mut paths = BTreeMap::new();
        for line in std::io::BufReader::new(log).lines() {
            let line = line.map_err(|e| e.to_string())?;
            let Ok(artifact) = json::parse(line.as_bytes()) else { continue };
            if artifact.get("reason").and_then(Value::as_str) != Some("compiler-artifact") { continue; }
            let target = artifact.get("target").ok_or("Missing Cargo artifact target")?;
            if target.get("kind").and_then(Value::as_arr).is_none_or(|kinds| kinds.iter().all(|k| k.as_str() == Some("custom-build"))) { continue; }
            let name = target.get("name").and_then(Value::as_str).ok_or("Missing Cargo crate name")?.replace('-', "_");
            let manifest = artifact.get("manifest_path").and_then(Value::as_str).ok_or("Missing Cargo artifact manifest")?;
            let directory = Path::new(manifest).parent().ok_or("Missing crate directory")?.canonicalize().map_err(|e| e.to_string())?;
            if !directory.starts_with(&sources) && !directory.join("resources").is_dir() { continue; }
            if !directory.starts_with(&root) { return Err(format!("Resources for {name} are outside the portable installation")); }
            let relative = directory.strip_prefix(&beside).map_err(|_| format!("Resources for {name} are outside the portable installation"))?;
            let relative = relative.to_str().ok_or("Non-UTF8 resource path")?.replace('\\', "/");
            if relative.contains(['\t', '\r', '\n']) { return Err("Unsupported character in resource path".into()); }
            if let Some(previous) = paths.insert(name.clone(), relative.clone()) {
                if previous != relative { return Err(format!("Conflicting resource crates: {name}")); }
            }
        }
        if paths.is_empty() { return Err("Cargo produced no application resource paths".into()); }
        let text: String = paths.iter().map(|(name, path)| format!("{name}\t{path}\n")).collect();
        let map_name = format!("{}.makepad-package-paths", binary.file_name().ok_or("Missing binary name")?.to_string_lossy());
        // A Windows app whose runtime also looks in the Builder's folder
        // for its map keeps the installation folder to executables only.
        if cfg!(windows) && reads_map_from_state_dir(release, &root) {
            fs::write(root.join(&map_name), &text).map_err(|e| e.to_string())?;
            let _ = fs::remove_file(beside.join(&map_name));
        } else if cfg!(windows) && reads_own_map(release, &root) {
            // Its runtime reads only the map beside the executable: there,
            // hidden, so the folder shows just the apps.
            let path = beside.join(&map_name);
            set_hidden(&path, false);
            fs::write(&path, &text).map_err(|e| e.to_string())?;
            set_hidden(&path, true);
            // The shared map older runtimes read is not needed by this one.
            set_hidden(&beside.join("makepad-package-paths"), true);
        } else {
            fs::write(beside.join(&map_name), &text).map_err(|e| e.to_string())?;
            // Legacy runtimes read the shared map. Keep other apps' entries when
            // publishing one app; current runtimes prefer their own map above.
            let map = beside.join("makepad-package-paths");
            if let Ok(previous) = fs::read_to_string(&map) {
                for line in previous.lines() {
                    if let Some((name, path)) = line.split_once('\t') { paths.entry(name.to_owned()).or_insert_with(|| path.to_owned()); }
                }
            }
            let text: String = paths.into_iter().map(|(name, path)| format!("{name}\t{path}\n")).collect();
            let next = map.with_extension("next");
            fs::write(&next, text).map_err(|e| e.to_string())?;
            replace_file(&next, &map)?;
        }
        let next = binary.with_extension("next");
        fs::copy(self.build.join("release").join(exe(&release.binary)), &next).map_err(|e| e.to_string())?;
        replace_file(&next, &binary)?;
        #[cfg(windows)]
        self.publish_cuda_runtime(&binary)?;
        // The shared target now holds this snapshot's artifacts: later
        // releases at the same commits build from here.
        release.mark_built(&root)?;
        record_toolchain(&root, &release.id)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            let command = crate::home_of(&self.root).join(&release.binary);
            fs::write(&command, include_str!("../launcher.sh").replace("@APP_BINARY@", &release.binary)).map_err(|e| e.to_string())?;
            fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        {
            let project = release.repositories.iter().find(|repo| repo.name == "makepad")
                .map(|repo| release.directory(&self.root).join(&repo.path))
                .unwrap_or_else(|| release.source(&self.root));
            crate::desktop::prepare(&self.root, release, &project)?;
        }
        progress::stage("Ready", &format!("{} is ready in the installation folder", exe(&release.binary)), 1.0);
        Ok(())
    }
}

/// Whether the release's Makepad runtime also reads an app's resource map
/// from the Builder's folder (`builder/<exe>.makepad-package-paths`), which
/// runtimes from before this layout do not.
fn reads_map_from_state_dir(release: &Release, root: &Path) -> bool {
    let Some(makepad) = release.repositories.iter().find(|repo| repo.name == "makepad") else { return false };
    let file = release.directory(root).join(&makepad.path).join("platform/src/os/cx_native.rs");
    fs::read_to_string(file).is_ok_and(|text| text.contains("join(\"builder\").join(format!(\"{}.makepad-package-paths\""))
}

/// The compiler an app is built with: the Rust triple and whether CUDA is
/// on. `installed/<app>.toolchain` records it for each app built here.
fn toolchain_stamp(root: &Path) -> String {
    format!("{} {}", rust_triple(root), if crate::cuda::build_with(root) { "cuda" } else { "cpu" })
}
fn record_toolchain(root: &Path, app: &str) -> Result<(), String> {
    let dir = root.join("installed");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{app}.toolchain")), toolchain_stamp(root)).map_err(|e| format!("Record the compiler of {app}: {e}"))
}
/// False when the app was built with another compiler than the one chosen
/// now (the other Windows toolchain, or CUDA on or off since), so it
/// compiles again. Windows apps built before this record count as other;
/// elsewhere there is only one compiler.
pub fn built_with_current_toolchain(root: &Path, app: &str) -> bool {
    match fs::read_to_string(root.join("installed").join(format!("{app}.toolchain"))) {
        Ok(stamp) => stamp.trim() == toolchain_stamp(root),
        Err(_) => !cfg!(windows),
    }
}

/// Whether the release's runtime reads `<exe>.makepad-package-paths` (all
/// but the oldest; those read only the shared `makepad-package-paths`).
fn reads_own_map(release: &Release, root: &Path) -> bool {
    let Some(makepad) = release.repositories.iter().find(|repo| repo.name == "makepad") else { return false };
    let file = release.directory(root).join(&makepad.path).join("platform/src/os/cx_native.rs");
    fs::read_to_string(file).is_ok_and(|text| text.contains("{}.makepad-package-paths"))
}

/// Set or clear the hidden attribute of a file (Windows; elsewhere nothing).
fn set_hidden(path: &Path, hidden: bool) {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileAttributesW(name: *const u16) -> u32;
            fn SetFileAttributesW(name: *const u16, attributes: u32) -> i32;
        }
        use std::os::windows::ffi::OsStrExt;
        const HIDDEN: u32 = 0x2;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        unsafe {
            let attributes = GetFileAttributesW(wide.as_ptr());
            if attributes != u32::MAX {
                SetFileAttributesW(wide.as_ptr(), if hidden { attributes | HIDDEN } else { attributes & !HIDDEN });
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (path, hidden);
}

/// Where an installation keeps its rustc frontend thread count, shared with
/// the POSIX bootstrap (which compiles the Builder into the same target).
pub const FRONTEND_THREADS_FILE: &str = "rustc-threads";

/// Threads for rustc's parallel frontend (`-Zthreads`) in every build of this
/// installation: the first number in `rustc-threads`, written once as the
/// machine's parallelism capped at 8 and then kept, so builds on this
/// machine always pass identical flags and never invalidate the shared
/// target. More than 8 threads gained nothing measurable on 16-thread
/// machines. 1 is the serial frontend: a single-thread machine, or the
/// parallel one failed here (see `Environment::cargo_logged`).
pub fn frontend_threads(root: &Path) -> usize {
    let file = root.join(FRONTEND_THREADS_FILE);
    let recorded = fs::read_to_string(&file).ok().and_then(|text| text.split_whitespace().next()?.parse::<usize>().ok());
    if let Some(threads) = recorded.filter(|n| *n >= 1) {
        return threads;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let _ = fs::write(&file, format!("{threads}\n"));
    threads
}

/// The pinned compiler flags: `RUSTFLAGS` (on Windows the bundled LLD and
/// the static CRT) plus `-Zthreads=N` when the frontend runs in parallel.
/// `-Z` flags need `RUSTC_BOOTSTRAP=1` on the pinned stable compiler; the
/// serial frontend sets it empty, which rustc and Cargo read as unset.
fn compiler_flags(vars: &mut BTreeMap<String, String>, threads: usize) {
    let gnu = vars.get("MAKEPAD_BUILDER_LINK").is_some_and(|l| l == "gnu");
    let mut flags = if gnu {
        // Rust's own LLD in MinGW mode, with the runtime Rust ships: GNU ld
        // cannot link the import libraries implib.rs writes.
        "-C linker=rust-lld -C linker-flavor=ld.lld -C link-self-contained=yes".to_owned()
    } else if cfg!(windows) {
        "-C linker-flavor=lld-link -C target-feature=+crt-static".to_owned()
    } else {
        String::new()
    };
    if threads > 1 {
        if !flags.is_empty() { flags.push(' '); }
        flags += &format!("-Zthreads={threads}");
    }
    vars.insert("RUSTFLAGS".into(), flags);
    vars.insert("RUSTC_BOOTSTRAP".into(), if threads > 1 { "1" } else { "" }.into());
}

fn frontend_threads_in(vars: &BTreeMap<String, String>) -> usize {
    vars.get("RUSTFLAGS")
        .and_then(|flags| flags.split_whitespace().find_map(|flag| flag.strip_prefix("-Zthreads=")?.parse().ok()))
        .unwrap_or(1)
}

/// Why a failed build looks like the parallel frontend's fault: rustc
/// panicked (an internal compiler error, its deadlock detector included),
/// ran out of memory, or died of a signal or Windows exception. Build
/// scripts that crash are not rustc and do not count.
fn frontend_failure(log: &Path) -> Option<String> {
    let text = fs::read(log).ok()?;
    let text = String::from_utf8_lossy(&text);
    for line in text.lines() {
        let reason = if line.contains("internal compiler error") || line.contains("the compiler unexpectedly panicked") || line.contains("deadlock detected") {
            "crashed (internal compiler error)"
        } else if line.contains("memory allocation of") && line.contains("failed") {
            "ran out of memory"
        } else if line.contains("rustc interrupted by") {
            "crashed"
        } else if line.contains("process didn't exit successfully") && line.contains("--crate-name") && (line.contains("(signal: ") || line.contains("exit code: 0xc0")) {
            if line.contains("SIGKILL") { "was stopped (out of memory?)" } else { "crashed" }
        } else {
            continue;
        };
        return Some(reason.to_owned());
    }
    None
}

/// Why a build failed in the CUDA kernels' build script, when that is why:
/// its own panic line, and the first nvcc diagnostic it forwarded.
fn cuda_kernels_failure(log: &Path) -> Option<String> {
    let text = fs::read(log).ok()?;
    let text = String::from_utf8_lossy(&text);
    text.lines().find(|line| line.contains("MAKEPAD_GGML_REQUIRE_CUDA=1, but the CUDA backend build failed"))?;
    let diagnostic = text
        .lines()
        .find(|line| line.contains("nvcc:") && (line.contains("error") || line.contains("fatal")))
        .map(|line| line.split("nvcc:").nth(1).unwrap_or(line).trim().to_owned());
    Some(crate::plain_paths(&diagnostic.unwrap_or_else(|| "nvcc failed".into())).chars().take(200).collect())
}

/// A line for the activity log (the plain output of the CLI).
fn note(text: &str) {
    if progress::active() {
        progress::stage("Compiling Rust", text, 0.0);
    } else {
        eprintln!("{text}");
    }
}

/// A Windows job object holding a build and everything it starts (rustc,
/// build scripts, nvcc), so the whole tree can be stopped at once without
/// running another program.
#[cfg(windows)]
struct Job(*mut std::ffi::c_void);
#[cfg(windows)]
impl Job {
    /// A job containing `child`, or none when it cannot be made.
    fn holding(child: &std::process::Child) -> Option<Job> {
        use std::os::windows::io::AsRawHandle;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateJobObjectW(attributes: *mut std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
            fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
        }
        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if job.is_null() {
            return None;
        }
        let job = Job(job);
        (unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } != 0).then_some(job)
    }
    fn terminate(&self) {
        #[link(name = "kernel32")]
        unsafe extern "system" { fn TerminateJobObject(job: *mut std::ffi::c_void, code: u32) -> i32; }
        unsafe { TerminateJobObject(self.0, 1) };
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        #[link(name = "kernel32")]
        unsafe extern "system" { fn CloseHandle(handle: *mut std::ffi::c_void) -> i32; }
        unsafe { CloseHandle(self.0) };
    }
}
#[cfg(not(windows))]
struct Job;
#[cfg(not(windows))]
impl Job {
    fn holding(_child: &std::process::Child) -> Option<Job> {
        None
    }
    fn terminate(&self) {}
}

/// Stop a process and everything it started (rustc, build scripts), for a
/// build that stopped making progress: its compilers would otherwise stay
/// behind, holding their files. On Windows the build's job object ends the
/// whole tree.
fn kill_tree(child: &mut std::process::Child, job: Option<&Job>) {
    if let Some(job) = job {
        job.terminate();
    }
    #[cfg(not(windows))]
    {
        let pid = child.id();
        // Every descendant, from the process table: `ps -A -o pid= -o ppid=`
        // is POSIX on macOS and Linux alike.
        if let Ok(out) = Command::new("ps").args(["-A", "-o", "pid=", "-o", "ppid="]).stdin(std::process::Stdio::null()).output() {
            let table: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|line| {
                let mut words = line.split_whitespace();
                Some((words.next()?.parse().ok()?, words.next()?.parse().ok()?))
            }).collect();
            let mut tree = vec![pid];
            let mut i = 0;
            while i < tree.len() {
                let parent = tree[i];
                tree.extend(table.iter().filter(|(_, ppid)| *ppid == parent).map(|(pid, _)| *pid));
                i += 1;
            }
            let _ = Command::new("kill").arg("-KILL").args(tree[1..].iter().map(u32::to_string)).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Strip inherited settings that would move Cargo's outputs or change its
/// flags from one session to the next. The installation's own environment
/// decides them, so every app and agent shares one stable target directory
/// with identical fingerprints, and no credential or rustup override leaks in.
pub fn isolate(command: &mut Command) {
    for key in [
        "MAKEPAD_LOADER_EMAIL",
        "RUSTUP_TOOLCHAIN",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_INCREMENTAL",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_BUILD_RUSTDOCFLAGS",
        "CARGO_BUILD_DEP_INFO_BASEDIR",
        "CARGO_TARGET_APPLIES_TO_HOST",
        "CARGO_UNSTABLE_BUILD_STD",
    ] {
        command.env_remove(key);
    }
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("CARGO_PROFILE_") {
            command.env_remove(key);
        }
    }
}

/// The POSIX bootstrap compiles this Builder from the pinned Makepad tree and
/// records that tree in `.builder-version` as "<commit> <rust> <triple>".
/// The catalog moves on while an installation lives, so once the release
/// checked out for Scope pins another Makepad commit the Builder is compiled
/// again from that tree, exactly as the bootstrap did, and the bootstrap's
/// own pins move along: its fast path then accepts the new binary and its
/// full path, should it ever run again, starts from these sources. Windows
/// ships a prebuilt Builder and has no such file, so nothing happens there.
/// The compiled binary serves the next start; this process keeps running.
/// Returns a line for the activity pane when the Builder changed.
pub fn update_builder(environment: &Environment, release: &Release) -> Result<Option<String>, String> {
    // Windows compiles the Builder from the source beside makepad-builder.bat.
    if cfg!(windows) {
        return Ok(None);
    }
    let root = environment.root.as_path();
    let recorded = match fs::read_to_string(root.join(".builder-version")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Read .builder-version: {error}")),
    };
    let mut fields = recorded.split_whitespace();
    let (Some(built), Some(_), Some(triple)) = (fields.next(), fields.next(), fields.next()) else {
        return Err("Unreadable .builder-version".into());
    };
    let Some(makepad) = release.repositories.iter().find(|repo| repo.name == "makepad") else {
        return Ok(None);
    };
    if built == makepad.commit {
        return Ok(None);
    }
    let directory = release.directory(root);
    let cargo = environment.vars.get("CARGO").ok_or("Missing Cargo path")?;
    let command = |vars: &BTreeMap<String, String>| {
        let mut command = Command::new(cargo);
        command.current_dir(directory.join(&makepad.path)).envs(vars);
        isolate(&mut command);
        command.args(["build", "--release", "-p", "makepad-loader", "--bin", "makepad-builder"]);
        command
    };
    progress::stage("Compiling Rust", "Makepad Builder", 0.0);
    let record = environment.build.join("makepad-builder.crates");
    let expected = Arc::new(AtomicU64::new(recorded_crates(&record)
        .unwrap_or_else(|| expected_crates(cargo, &directory.join(&makepad.path), &environment.vars, "makepad-loader", &[], environment.triple()))));
    let (status, crates) = environment.cargo_logged(&environment.build.join("builder-update.log"), &expected, command)?;
    if status.success() {
        let _ = fs::write(&record, crates.to_string());
    }
    if !status.success() {
        return Err(format!("Builder build failed: {status}"));
    }
    let next = root.join("makepad-builder.next");
    fs::copy(environment.build.join("release").join(exe("makepad-builder")), &next).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&next, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    replace_file(&next, &root.join("makepad-builder"))?;
    fs::write(root.join(".builder-version"), format!("{} {} {triple}", makepad.commit, release.rust)).map_err(|e| e.to_string())?;
    let receipt = fs::read_to_string(directory.join(".builder-repositories").join(&makepad.name)).map_err(|e| e.to_string())?;
    let label = directory.file_name().and_then(|name| name.to_str()).ok_or("Source snapshot has no label")?;
    let pinned = pin_bootstrap(
        &root.join("bootstrap-builder.sh"),
        &[("rust", &release.rust), ("commit", &makepad.commit), ("release", label), ("receipt", receipt.trim())],
    )?;
    Ok(Some(format!(
        "Builder compiled from Makepad {}; the next start uses it{}",
        &makepad.commit[..12],
        if pinned { "." } else { ", and bootstrap-builder.sh keeps its original pins." }
    )))
}

/// Move the `builder_<key>='…'` assignments of the installed bootstrap to
/// new values, all of them or none: a script without the expected lines is
/// left as it is and false comes back. The values are commits, versions,
/// labels and receipts, so none of them needs quoting.
fn pin_bootstrap(script: &Path, pins: &[(&str, &str)]) -> Result<bool, String> {
    let text = match fs::read_to_string(script) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("Read {}: {error}", script.display())),
    };
    let mut found = 0;
    let mut lines = Vec::new();
    for line in text.lines() {
        match pins.iter().find(|(key, _)| line.starts_with(&format!("builder_{key}='"))) {
            Some((key, value)) => {
                found += 1;
                lines.push(format!("builder_{key}='{value}'"));
            }
            None => lines.push(line.to_owned()),
        }
    }
    if found != pins.len() {
        return Ok(false);
    }
    let next = script.with_extension("sh.next");
    fs::write(&next, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    let mode = fs::metadata(script).map_err(|e| e.to_string())?.permissions();
    fs::set_permissions(&next, mode).map_err(|e| e.to_string())?;
    replace_file(&next, script)?;
    Ok(true)
}

fn replace_file(next: &Path, destination: &Path) -> Result<(), String> {
    // Windows cannot replace an existing file with std::fs::rename.
    // A running executable can be renamed but not overwritten, so the old
    // file moves aside first (replacing any older .previous) and the new one
    // takes its name.
    let backup = destination.with_extension("previous");
    let existed = destination.is_file();
    if existed {
        fs::rename(destination, &backup).map_err(|e| e.to_string())?;
    }
    if let Err(error) = fs::rename(next, destination) {
        if existed { let _ = fs::rename(&backup, destination); }
        return Err(error.to_string());
    }
    // Still running programs keep their .previous file until the next update.
    if let (true, Some(folder)) = (existed, destination.parent()) {
        let _ = crate::remove_inside(folder, &backup);
    }
    Ok(())
}
/// The temporary folder builds are given. nvcc cannot create its
/// intermediate files under a TMP with a space in it (an installation in
/// `Downloads\makepad-builder (2)`) or in the verbatim `\\?\` form of a
/// canonical Windows path: "Could not open output file", and every CUDA
/// crate fails to build. On Windows TMP is the folder's plain short (8.3)
/// path, which has neither.
fn compiler_temp(tmp: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        #[link(name = "kernel32")]
        unsafe extern "system" { fn GetShortPathNameW(long: *const u16, short: *mut u16, length: u32) -> u32; }
        let plain = match tmp.to_str().and_then(|p| p.strip_prefix(r"\\?\")) {
            Some(plain) if plain.as_bytes().get(1) == Some(&b':') => PathBuf::from(plain),
            _ => tmp.to_path_buf(),
        };
        let shorten = |path: &Path| -> PathBuf {
            let long: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            let mut short = vec![0u16; 1024];
            let length = unsafe { GetShortPathNameW(long.as_ptr(), short.as_mut_ptr(), short.len() as u32) } as usize;
            // No short name (8.3 names off on the volume): the long path.
            if length > 0 && length < short.len() { PathBuf::from(OsString::from_wide(&short[..length])) } else { path.to_path_buf() }
        };
        let temp = shorten(&plain);
        if !temp.to_string_lossy().contains(' ') {
            return temp;
        }
        // Still a space (8.3 names are off on this volume): nvcc fails
        // silently there, so the builds get a folder under the system's
        // temporary directory, which usually has none.
        let system = shorten(&env::temp_dir()).join("makepad-builder");
        if !system.to_string_lossy().contains(' ') && fs::create_dir_all(&system).is_ok() {
            return system;
        }
        temp
    }
    #[cfg(not(windows))]
    tmp.to_path_buf()
}
/// Console applications launched for setup/build must not show a second
/// Windows console. When this process is already attached to a console (the
/// setup TUI runs inside MpTerm's pseudo console) the child inherits that
/// console, so cargo and everything it starts share the invisible session.
/// Only a console-less parent passes CREATE_NO_WINDOW, which per Microsoft's
/// process-creation flags leaves the child without a console handle; how the
/// child's own children behave in that state is not established here.
/// Interactive agents still run through the inherited PTY.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        #[link(name = "kernel32")]
        unsafe extern "system" { fn GetConsoleCP() -> u32; }
        if unsafe { GetConsoleCP() } == 0 {
            command.creation_flags(0x08000000);
        }
    }
    #[cfg(not(windows))] let _ = command;
}
/// How many crates this binary's last successful build had, fresh ones
/// included: exact, where cargo metadata overcounts in a large workspace
/// (features any member asks for are resolved for all of them).
fn recorded_crates(record: &Path) -> Option<u64> {
    fs::read_to_string(record).ok()?.trim().parse().ok().filter(|n| *n > 0)
}

/// How a logged Cargo build ended: its status, the crates it reported
/// (fresh ones included), and whether it was stopped for making no progress.
struct BuildRun {
    status: std::process::ExitStatus,
    crates: u64,
    stalled: bool,
}

/// `expected` is the number of crates the build produces (0 when unknown);
/// with it the crate count becomes a bar. With `quiet`, a build that writes
/// nothing for that long is stopped, all its compilers with it.
fn run_build_logged(command: &mut Command, log: &Path, expected: &AtomicU64, quiet: Option<std::time::Duration>) -> Result<BuildRun, String> {
    use std::{io::Read, process::Stdio, time::Duration};
    hide_console(command);
    command.env("CARGO_TERM_COLOR", "never").env("CARGO_TERM_PROGRESS_WHEN", "never");
    // A long build script (the CUDA kernels) reports its progress through
    // this one-line file instead of writing over the Builder's screen.
    let status_file = log.with_extension("status");
    let _ = fs::remove_file(&status_file);
    command.env("MAKEPAD_BUILD_STATUS_FILE", &status_file);
    let mut status_line = String::new();
    let output = fs::File::create(log).map_err(|e| e.to_string())?;
    let mut input = fs::File::open(log).map_err(|e| e.to_string())?;
    let child = command.stdin(Stdio::null()).stdout(output.try_clone().map_err(|e| e.to_string())?).stderr(output).spawn().map_err(|e| e.to_string())?;
    let job = Job::holding(&child);
    struct Running(Option<std::process::Child>);
    impl Drop for Running { fn drop(&mut self) { if let Some(child) = &mut self.0 { let _ = child.kill(); let _ = child.wait(); } } }
    let mut child = Running(Some(child));
    let mut pending = Vec::new();
    let mut last = "Starting Cargo release build".to_string();
    // Every crate is one compiler-artifact message. A crate that is already
    // up to date ("fresh") is no work: it leaves the total instead of adding
    // to the count, so an incremental build measures what really compiles.
    // The bar stays short of the end until Cargo is done, also when the
    // estimate is low.
    let (mut crates, mut fresh) = (0u64, 0u64);
    let of = |crates: u64, fresh: u64| match expected.load(Ordering::Relaxed) {
        0 => 0,
        expected => expected.saturating_sub(fresh).max(crates + 1),
    };
    let mut heard = std::time::Instant::now();
    // A caller that reads Cargo's own progress bar (the WM's tile, which
    // hands its builds here) asks for it as lines: "Building [ ] 73/88".
    let bar_lines = env::var_os("MAKEPAD_BUILD_PROGRESS_LINES").is_some();
    let mut bar_said = (u64::MAX, u64::MAX);
    loop {
        let status = child.0.as_mut().unwrap().try_wait().map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 16384];
        loop {
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            heard = std::time::Instant::now();
            pending.extend_from_slice(&buffer[..n]);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line = String::from_utf8_lossy(&pending[..end]).trim_end().to_owned();
                pending.drain(..=end);
                // Cargo artifacts are machine data for resource publication.
                // Diagnostics and normal Cargo status remain visible.
                if let Ok(message) = makepad_strict_json::parse(line.as_bytes()) {
                    if message.get("reason").and_then(makepad_strict_json::Value::as_str) == Some("compiler-artifact") {
                        if message.get("fresh").and_then(makepad_strict_json::Value::as_bool) == Some(true) {
                            fresh += 1;
                        } else {
                            crates += 1;
                        }
                        progress::measured("Compiling Rust", &last, crates, of(crates, fresh), progress::Unit::Crates);
                        if bar_lines && (crates, of(crates, fresh)) != bar_said {
                            bar_said = (crates, of(crates, fresh));
                            eprintln!("Building [ ] {}/{}", bar_said.0, if bar_said.1 == 0 { crates + 1 } else { bar_said.1 });
                        }
                    }
                    if message.get("reason").is_some() { continue; }
                }
                last = line;
                if !progress::active() { eprintln!("{last}"); }
                progress::measured("Compiling Rust", &last, crates, of(crates, fresh), progress::Unit::Crates);
            }
            if pending.len() > 16384 { pending.drain(..pending.len() - 16384); }
        }
        if let Some(status) = status {
            if !pending.is_empty() { progress::stage("Compiling Rust", &String::from_utf8_lossy(&pending), 0.0); }
            child.0.take();
            if status.success() { progress::stage("Ready", "Release build complete", 1.0); }
            return Ok(BuildRun { status, crates: crates + fresh, stalled: false });
        }
        if quiet.is_some_and(|quiet| heard.elapsed() > quiet) {
            let mut stopped = child.0.take().unwrap();
            kill_tree(&mut stopped, job.as_ref());
            let status = stopped.wait().map_err(|e| e.to_string())?;
            return Ok(BuildRun { status, crates: crates + fresh, stalled: true });
        }
        if let Some(line) = fs::read_to_string(&status_file).ok().and_then(|s| s.lines().next().map(|l| l.trim().to_owned())).filter(|l| !l.is_empty()) {
            if line != status_line {
                heard = std::time::Instant::now();
                last = line.clone();
                status_line = line;
            }
        }
        progress::measured("Compiling Rust", &last, crates, of(crates, fresh), progress::Unit::Crates);
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn expected_crates(cargo: &str, cwd: &Path, vars: &BTreeMap<String, String>, package: &str, features: &[&str], triple: &str) -> u64 {
    use makepad_strict_json::{self as json, Value};
    let cargo_output = |args: &[&str]| -> Option<Vec<u8>> {
        let mut command = Command::new(cargo);
        command.current_dir(cwd).envs(vars);
        isolate(&mut command);
        hide_console(&mut command);
        command.args(args).args(features).stdin(std::process::Stdio::null());
        let output = command.output().ok()?;
        output.status.success().then_some(output.stdout)
    };
    // The packages this build compiles, as `cargo tree` resolves them for
    // this package alone. `cargo metadata` resolves the features every member
    // of a large workspace asks for and counted twice as many. The triple is
    // the one the build compiles for: with the GNU toolchain's link flags in
    // RUSTFLAGS, rustc refuses to describe the MSVC target, and the count
    // (and with it the bar) was lost.
    let Some(tree) = cargo_output(&["tree", "-q", "-p", package, "-e", "normal,build", "--prefix", "none", "--format", "{p}", "--target", triple]) else { return 0 };
    let tree = String::from_utf8_lossy(&tree);
    let built: std::collections::BTreeSet<&str> = tree.lines()
        .map(|line| line.trim_end_matches(" (*)").trim())
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((words.next()?, words.next()?))
        })
        .map(|(name, _version)| name)
        .collect();
    // Which of them have a build script: each is one more artifact.
    let Some(metadata) = cargo_output(&["metadata", "--format-version", "1", "--filter-platform", triple]) else { return 0 };
    let Ok(metadata) = json::parse(&metadata) else { return 0 };
    let Some(packages) = metadata.get("packages").and_then(Value::as_arr) else { return 0 };
    let scripts = packages.iter()
        .filter(|p| p.get("name").and_then(Value::as_str).is_some_and(|name| built.contains(name)))
        .filter(|p| p.get("targets").and_then(Value::as_arr).is_some_and(|targets| {
            targets.iter().any(|t| t.get("kind").and_then(Value::as_arr).is_some_and(|k| k.iter().any(|k| k.as_str() == Some("custom-build"))))
        }))
        .count() as u64;
    // One artifact per package, one per build script, and the app's binary.
    built.len() as u64 + scripts + 1
}

fn join_paths(paths: &[PathBuf]) -> Result<String, String> {
    env::join_paths(paths)
        .map(|s| s.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}
pub fn shell(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
pub fn batch(s: &str) -> Result<String, String> {
    if s.contains(['"', '\r', '\n', '\0']) {
        return Err("Path cannot be represented in a Windows command script".into());
    }
    Ok(s.replace('%', "%%"))
}

pub fn prepare(
    root: &Path,
    service: &str,
    key: &str,
    release: &Release,
    cuda: bool,
) -> Result<Environment, String> {
    let root = crate::validate_install_root(root)?;
    if cuda && !crate::cuda::supported() {
        return Err("The optional CUDA toolkit is available on x64 Windows".into());
    }
    system_tools_ready()?;
    let mut kinds = vec![Dependency::Rust];
    if cfg!(windows) && windows_chain(&root) != WindowsChain::Gnu {
        kinds.push(Dependency::Msvc);
    }
    if cuda {
        kinds.push(Dependency::Cuda);
    }
    progress::stage("Toolchain", &format!("Rust {}, platform tools", release.rust), 0.05);
    dependencies(&root, release, &kinds)?;
    catalog::checkout(service, key, &root, release)?;
    progress::stage("Ready", "Private toolchain and sources ready", 1.0);
    Environment::prepare(&root, release, cuda)
}

/// Rebuild the edited, already installed sources without refreshing or downloading.
pub fn rebuild_local() -> Result<(), String> {
    let root = crate::validate_install_root(&crate::default_root())?;
    let selected = env::var("MAKEPAD_BUILDER_APP").ok().or_else(|| fs::read_to_string(root.join("selected-app")).ok()).filter(|s| catalog::identifier(s));
    let installed = selected.as_ref().map(|s| root.join("installed").join(format!("{s}.json")));
    let release_path = installed.filter(|p| p.is_file()).unwrap_or_else(|| root.join("latest.json"));
    let release = Release::parse(&makepad_strict_json::parse(&fs::read(release_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?)?;
    if !release.installed(&root) { return Err("Download the app sources first".into()); }
    let environment = Environment::prepare(&root, &release, crate::cuda::build_with(&root))?;
    environment.build(&release)?;
    release.save(&root.join("installed-release.json"))?;
    println!("Ready: {}", crate::shown(&environment.app_binary(&release)));
    Ok(())
}

/// `makepad-builder build APP`: compile an app whose sources are already in
/// this installation, offline, exactly as the Builder does (its compiler,
/// flags, features and shared target), and publish it. What the macOS/Linux
/// `makepad build APP` is; the WM hands its builds here, so an app it opens
/// builds the same way whether the WM was started from the Builder or on
/// its own.
pub fn build_local(app: &str) -> Result<(), String> {
    if !catalog::identifier(app) {
        return Err(format!("Not an app name: {app}"));
    }
    let root = crate::validate_install_root(&crate::default_root())?;
    let load = |path: PathBuf| -> Option<Release> {
        let data = fs::read(path).ok()?;
        Release::parse(&makepad_strict_json::parse(&data).ok()?).ok()
    };
    // The installed release, else the one the catalog last offered, else a
    // free app out of the public release's Makepad sources.
    let release = load(root.join("installed").join(format!("{app}.json")))
        .or_else(|| load(root.join("available").join(format!("{app}.json"))))
        .or_else(|| {
            let definition = catalog::apps().ok()?.into_iter()
                .find(|entry| entry.get("id").and_then(makepad_strict_json::Value::as_str) == Some(app))?;
            // The public release, as the TUI picks it from available/.
            let public = fs::read_dir(root.join("available")).ok()?.flatten()
                .filter_map(|entry| load(entry.path()))
                .filter(|release| release.public)
                .max_by(|a, b| a.release.cmp(&b.release))?;
            public.for_app(&definition).ok()
        })
        .ok_or_else(|| format!("{app} is not in this installation; download it in the Builder first"))?;
    if !release.installed(&root) {
        return Err(format!("The sources of {} are not downloaded yet; download them in the Builder first", release.title));
    }
    let environment = Environment::prepare(&root, &release, crate::cuda::build_with(&root))?;
    environment.build(&release)?;
    fs::create_dir_all(root.join("installed")).map_err(|e| e.to_string())?;
    release.save(&root.join("installed").join(format!("{}.json", release.id)))?;
    println!("Ready: {}", crate::shown(&environment.app_binary(&release)));
    Ok(())
}

pub fn cli_main() -> Result<(), String> {
    let bootstrap = makepad_loader_bundle::load()?.map(|(_, config)| config);
    let mut app = bootstrap.as_ref().map(|config| config.app.clone());
    let mut address = env::var("MAKEPAD_LOADER_EMAIL").ok()
        .or_else(|| bootstrap.map(|config| config.email));
    let mut root = crate::default_root();
    let mut service =
        env::var("MAKEPAD_LOADER_SERVICE").unwrap_or_else(|_| catalog::DEFAULT_SERVICE.into());
    let mut sources = false;
    let mut setup = false;
    let mut build = false;
    let mut run = false;
    let mut cuda = false;
    let mut accept_rust = false;
    let mut accept_ms = false;
    let mut accept_nv = false;
    let mut accept_apple = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--app" => app = Some(args.next().ok_or("--app needs an app ID")?),
            "--email" => address = Some(args.next().ok_or("--email needs an address")?),
            "--root" => root = crate::state_dir(Path::new(&args.next().ok_or("--root needs a folder")?)),
            "--service" => service = args.next().ok_or("--service needs a URL")?,
            "--fetch-latest" => (),
            "--sources-only" => sources = true,
            "--prepare" => setup = true,
            "--build" => build = true,
            "--run" => run = true,
            "--cuda" => cuda = true,
            "--accept-rust" => accept_rust = true,
            "--accept-microsoft" => accept_ms = true,
            "--accept-nvidia" => accept_nv = true,
            "--accept-apple" => accept_apple = true,
            _ => return Err(format!("Unknown Makepad Builder option {arg}")),
        }
    }
    root = crate::validate_install_root(&root)?;
    let app = app.ok_or("--app is required")?;
    let key = address.unwrap_or_default();
    // A public app from the registry, otherwise a licensed one (Scope,
    // Stage, ...) from the catalog this email address may use.
    let selected = if app == "makepad" { "calculator" } else { &app };
    let public = catalog::apps()?.into_iter().find(|a| a.get("id").and_then(makepad_strict_json::Value::as_str) == Some(selected));
    let release = match public {
        Some(definition) if app != "scope" => catalog::fetch_public(&service)?.for_app(&definition)?,
        _ => catalog::fetch(&service, &key)?.into_iter().find(|r| r.id == app).ok_or("App is not included in this email address")?,
    };
    println!(
        "{} release={} rust={} platform={}",
        release.title,
        release.release,
        release.rust,
        catalog::platform()
    );
    if !release.supported() {
        return Err("Release does not support this platform".into());
    }
    if setup {
        if !accept_rust
            || cfg!(windows) && windows_chain(&root) != WindowsChain::Gnu && !accept_ms
            || cfg!(target_os = "macos") && !accept_apple
            || cuda && !accept_nv
        {
            return Err("Setup requires explicit --accept-rust and the platform vendor terms; optional CUDA requires --accept-nvidia".into());
        }
        prepare(&root, &service, &key, &release, cuda)?;
    } else if sources {
        crate::timing::reset();
        let result = progress::scope(progress::printer(), || catalog::checkout(&service, &key, &root, &release));
        for line in crate::timing::report() {
            println!("{line}");
        }
        result?;
    }
    if build || run {
        let environment = Environment::prepare(&root, &release, cuda)?;
        environment.build(&release)?;
        // The same records the TUI keeps: `build APP`, the WM and the TUI's
        // app states read them.
        for dir in ["installed", "available"] {
            fs::create_dir_all(root.join(dir)).map_err(|e| e.to_string())?;
            release.save(&root.join(dir).join(format!("{}.json", release.id)))?;
        }
        if run {
            let status = Command::new(environment.app_binary(&release))
                .current_dir(&environment.cwd)
                .envs(&environment.vars)
                .env_remove("MAKEPAD_LOADER_EMAIL")
                // Send feedback's sender row (runtime only, never a build).
                .env("MAKEPAD_FEEDBACK_EMAIL", &key)
                .arg("--remote")
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("App exited {status}"));
            }
        }
    }
    Ok(())
}
