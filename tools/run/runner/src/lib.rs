//! The runner (the `makepad` binary, tools/run, is this plus its [`Host`]; the `relax` binary,
//! bin/relax.rs, is this with none; the Android host library, tools/run/android_host, is the
//! `makepad` one as a shared object): Relax (the Rust JIT; makepad's proc macros JIT-compiled by
//! its host, macros.rs sets the lexer's `script_mod!` extension). Same commands and flags as `makepad` (`run <package> [cargo flags]
//! [-- args]` in a workspace), plus `run <git url>[#ref] [--mount <dir>=<git url>[#ref]]...
//! [--cached] <package> ...` and `fetch <spec>` (generic git fetching, fetch.rs). Which
//! repositories to run is always given on the command line; the makepad entry scripts name the
//! runner's (tools/run/entry/).

// macros.rs names Relax's lexer by this path
// Diagnostics never panic: std's print macros panic when a write fails (ENOSPC on the session
// log in a full tmpfs /tmp aborted a fleet Sandbox inside a lazy compile, 2026-10-23), so in this
// crate every print is a write whose error is dropped. Test builds keep std's captured macros.
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprintln {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprint {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!($($t)+)); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! println {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! print {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!($($t)+)); }};
}

mod lexer {
    pub use relax_compiler::lexer::*;
}
mod cargo;
pub mod fetch;
mod help;
pub mod launch;
mod macros;
pub mod remote;

/// What a runner binary adds to the bare runner. The `makepad` binary (tools/run) provides crates
/// built into it and hosts HAL programs on the main thread; the `relax` binary adds nothing (no
/// host), so every crate of a program, the HAL and Splash included, is JIT-compiled from source.
pub struct Host {
    /// what this binary was built from (its build.rs; the linked compiler's own stamp is
    /// relax_compiler::PROVENANCE)
    pub provenance: &'static str,
    /// once, before anything else (the compute crate's kernel store)
    pub init: fn(),
    /// before a session is made: registers the provided crates and the main-thread run host
    pub provide: fn(),
    /// the HAL's host and the main-thread run host (the window, the run's end) without the
    /// provided crates, before the program's source is had: an app's start opens its window
    /// first (`provide` registers them too)
    pub run_host: fn(),
    /// how much the JIT compiles ahead of the program's calls when neither `--ahead=` nor
    /// MAKEPAD_RUN_AHEAD says (jit::AheadPolicy): the entry point's default
    pub ahead: relax_compiler::options::AheadLevel,
    /// which proc-macro crates may use `unsafe` and std capabilities (pmhost::Policy): `relax`
    /// runs crates.io macros as cargo does, `makepad` sandboxes third-party ones
    pub proc_macros: relax_compiler::pmhost::Policy,
    /// what `makepad` with no arguments runs when no launch record says (launch.rs): None prints
    /// the usage
    pub default_launch: Option<fn() -> Option<launch::Launch>>,
    /// the cargo package of this binary and its manifest: a change the running process cannot
    /// take (provided crates, std, the binary's own sources) rebuilds it with cargo and restarts
    /// into it (reload::host)
    pub package: &'static str,
    pub manifest: &'static str,
}

/// The bare runner (bin `relax`): provides nothing; tests, checks and agents' boots compile only
/// what runs.
pub const BARE: Host = Host { provenance: env!("RELAX_RUNNER_PROVENANCE"), init: || {}, provide: || {}, run_host: || {}, ahead: relax_compiler::options::AheadLevel::Minimal, proc_macros: relax_compiler::pmhost::Policy::Open, default_launch: None, package: env!("CARGO_PKG_NAME"), manifest: concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml") };

static HOST: std::sync::OnceLock<&'static Host> = std::sync::OnceLock::new();

fn host() -> &'static Host {
    HOST.get().copied().unwrap_or(&BARE)
}

/// The runner's host, once, before `main`/`init_embedded`/`run_checkout`.
pub fn set_host(host: &'static Host) {
    if HOST.set(host).is_ok() {
        relax_compiler::pmhost::set_policy(host.proc_macros);
        relax_compiler::reload::set_loader_package(host.package, host.manifest);
        // the entry point's ahead level, unless the environment names one; MAKEPAD_RUN_AHEAD
        // carries it to the session (options.rs) and to every process this one starts
        if std::env::var_os("MAKEPAD_RUN_AHEAD").is_none() {
            std::env::set_var("MAKEPAD_RUN_AHEAD", host.ahead.name());
        }
        (host.init)();
    }
}

fn provided() {
    (host().provide)()
}

/// The loader binary: current_exe is canonicalised so a macOS bundle's in-bundle symlink
/// resolves to the real loader. Everything found beside the loader (the runner's checkout,
/// std and makepad-launcher) is therefore found beside that shared binary.
pub(crate) fn loader_exe() -> Option<std::path::PathBuf> {
    static EXE: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    EXE.get_or_init(|| {
        let _kept = relax_compiler::zone_alloc::kept();
        Some(plain(std::env::current_exe().and_then(|e| e.canonicalize()).ok()?))
    })
    .clone()
}

/// Windows: canonicalize answers a verbatim path (`\\?\C:\...`), in which `/` is no separator, so
/// everything joined to it later (`libs/relax/compiler/std`, `/core/lib.rs`) would not
/// open; a plain drive path is the same file (as the dunce crate does). Elsewhere unchanged.
fn plain(p: std::path::PathBuf) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
                return std::path::PathBuf::from(rest.to_string());
            }
        }
    }
    p
}

/// The runner on this process's command line, with `host` added.
pub fn main(host: &'static Host) {
    set_host(host);
    relax_compiler::wasm_export::startup();
    run(relax_compiler::os::proc::args_bytes())
}

/// What the loader sets up before it runs a package, for an embedder whose process is not the
/// loader's executable (the Android host library in an Activity's process): `exe` is Relax's own
/// executable, which build scripts and `MAKEPAD_RUN_LOADER` programs run as.
pub fn init_embedded(exe: &std::path::Path, host: &'static Host) {
    set_host(host);
    relax_compiler::unitgraph::set_self_exe(exe.to_path_buf());
    // the program's argv[0] / current_exe: the embedder has no command line of its own
    relax_compiler::os::proc::set_args(vec![exe.as_os_str().as_encoded_bytes().to_vec()]);
    std::env::set_var("MAKEPAD_RUN_LOADER", exe);
    macros::register();
}

/// The loader's command line as an embedder gives it. A host library loaded into another process
/// (the Android APK's executable dlopens it for build scripts) has no process arguments of its
/// own; this names them for everything that reads them (relax_compiler::os::proc::args_bytes).
pub fn run_with_args(args: Vec<Vec<u8>>) {
    relax_compiler::os::proc::set_args(args.clone());
    run(args)
}

/// NVIDIA's shader disk cache (GL and Vulkan) for every program this loader runs. The driver keys
/// its files by the executable's path, and every JIT build is a new executable: each binary's first
/// runs compiled every pipeline cold (Stage held its first frame 9 s for 109 pipelines), so they
/// all take one name. And a folder of their own, `$XDG_CACHE_HOME/makepad/nvidia`: the driver's
/// shared folder takes no entry once it holds its 1 GB, and every other program's per-path keys
/// fill it (a pool box at 1025 MB: every cold Fab compiled its pipelines again, the first frame
/// held 20.5 s for 177 pipelines; in a folder of its own 0.3 s). The driver trims neither: past
/// 3/4 of its cap this one is emptied, and the next runs fill it again. It is renamed away before
/// it is removed, so a start beside this one finds the full folder or a new empty one, never one
/// half removed, and a running program's open files go with the old one (its writes are lost, not
/// mixed into the new folder).
#[cfg(target_os = "linux")]
fn nvidia_shader_cache() {
    if std::env::var_os("__GL_SHADER_DISK_CACHE_APP_NAME").is_none() {
        std::env::set_var("__GL_SHADER_DISK_CACHE_APP_NAME", "makepad");
    }
    if std::env::var_os("__GL_SHADER_DISK_CACHE_PATH").is_some() {
        return;
    }
    let abs = |v: Option<std::ffi::OsString>| v.map(std::path::PathBuf::from).filter(|p| p.is_absolute());
    let Some(base) = abs(std::env::var_os("XDG_CACHE_HOME")).or_else(|| abs(std::env::var_os("HOME")).map(|h| h.join(".cache"))) else {
        return;
    };
    let dir = base.join("makepad").join("nvidia");
    fn bytes(dir: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
        entries.flatten().map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => bytes(&e.path()),
            _ => e.metadata().map_or(0, |m| m.len()),
        }).sum()
    }
    if bytes(&dir) > 768 << 20 {
        // (one name: what a killed removal left goes with the next one)
        let old = dir.with_extension("old");
        let _ = std::fs::remove_dir_all(&old);
        if std::fs::rename(&dir, &old).is_ok() {
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    if std::fs::create_dir_all(&dir).is_ok() {
        std::env::set_var("__GL_SHADER_DISK_CACHE_PATH", &dir);
    }
}

fn run(raw_args: Vec<Vec<u8>>) {
    {
        let _k = relax_compiler::zone_alloc::kept();
        // A connected zero-capacity receive enters std's persistent mpmc context before
        // checking its deadline; try_recv does not. Keep that TLS Arc out of the arena.
        let (_tx, rx) = std::sync::mpsc::sync_channel::<()>(0);
        let _ = rx.recv_timeout(std::time::Duration::ZERO);
    }
    // the boot thread compiles: its allocations go to the compiler arena (JIT calls switch back)
    let _phase = relax_compiler::zone_alloc::compiler();
    relax_compiler::clock::mark_process_start();
    // A build script's CARGO / RUSTC calls are answered before normal loader commands.
    if cargo::dispatch(&raw_args) || relax_compiler::relax_as_rustc() {
        return;
    }
    // cargo's RUSTC_WRAPPER of an --allow-c native build, or a stubbed build script of it
    if let Some(code) = relax_compiler::native::wrapper() {
        std::process::exit(code);
    }
    // `makepad app <id>` and `makepad` alone: the launch record's command line (launch.rs), with
    // its folder and environment applied before anything reads them
    let raw_args = match launch::resolve(&raw_args, host().default_launch) {
        None => raw_args,
        Some(Ok(words)) => {
            relax_compiler::os::proc::set_args(words.clone());
            words
        }
        Some(Err(e)) => {
            eprintln!("makepad: {e}");
            std::process::exit(2);
        }
    };
    // Windows has no exec: a run that can restart becomes the child of this process, which waits
    #[cfg(windows)]
    relax_compiler::restart_parent::enter(&raw_args);
    // a child started in a virtual folder of a git repository run gets the mount table
    if let Err(e) = relax_compiler::vfs::mount_from_env() {
        eprintln!("makepad: MAKEPAD_RUN_VFS: {e}");
        std::process::exit(2);
    }
    let mut raw_args = raw_args;
    if take_privileged(&mut raw_args) {
        PRIVILEGED.store(true, std::sync::atomic::Ordering::Relaxed);
        relax_compiler::os::proc::set_args(raw_args.clone());
    }
    let args: Vec<String> = raw_args.iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("makepad {} (compiler: {})", host().provenance, relax_compiler::PROVENANCE);
        return;
    }
    // every program this loader runs (and every program those start) finds it here: a program
    // runs under the loader when its own executable is this file (std::env::current_exe), and
    // starts other apps with `$MAKEPAD_RUN_LOADER run ...` (std process.rs: a spawn of the loader
    // with `run` arguments is a launch, never a re-run of the program)
    if let Some(e) = loader_exe() {
        std::env::set_var("MAKEPAD_RUN_LOADER", e);
    }
    // the programs it runs (the runner app) keep their launch records in the same home
    std::env::set_var("MAKEPAD_RUN_HOME", fetch::home());
    #[cfg(target_os = "linux")]
    nvidia_shader_cache();
    macros::register();
    match args.get(1).map(String::as_str) {
        None => {
            help::print_main(help::Stream::Err);
            std::process::exit(2);
        }
        Some("-h") | Some("--help") => {
            help::print_main(help::Stream::Out);
            std::process::exit(0);
        }
        Some("help") => std::process::exit(help::help(&args[2..])),
        // `inspect <pid|app|all|list> <command>`: ask a running app (no compiler started here)
        Some("inspect") => std::process::exit(relax_compiler::inspect::client::main(&args[2..])),
        // the remote hub: `hub [--listen <addr>]`, `remote-relay [--dial ..]`, `remote add|pair|list`
        Some("hub") => std::process::exit(remote::hub(&args[2..])),
        // `devhub` (or `observe`) `stream|ls|inspect|reply|inbox|hubs`: the local devhub (every JIT
        // on the LAN from the master), its own binary
        Some("devhub" | "observe") => {
            if args.get(2).map(String::as_str) == Some("ci") {
                eprintln!("use `makepad ci`");
                std::process::exit(2);
            }
            std::process::exit(devhub(&args[2..]));
        }
        Some("ci") => std::process::exit(devhub(&args[1..])),
        Some("remote-relay") => std::process::exit(remote::relay(&args[2..])),
        Some("remote") => std::process::exit(remote::remote(&args[2..])),
        // `pack-src [--target <triple>] <out.store> <package>...`: the source archive `run --src` runs from
        Some("pack-src") => match relax_compiler::packsrc::parse_args(&args[2..]).and_then(relax_compiler::packsrc::run) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("makepad pack-src: {e}");
                std::process::exit(1);
            }
        },
        // `run --src <store>[@<offset>+<len>] <package> ...`: from a packed source archive, no checkout
        Some("run") if args[2..].iter().any(|a| a == "--src") => relax_compiler::os::proc::exit(run_src(&args[2..])),
        // `check --src <store>[@<offset>+<len>] <package> ...`: type/borrow-check what `run --src` would
        // compile, reading only the archive (an archive that misses a file the app needs fails here,
        // not on the phone); never through the check server
        Some("check") if args[2..].iter().any(|a| a == "--src") => {
            let mut words = vec!["--check".to_string()];
            words.extend(args[2..].iter().filter(|a| !matches!(a.as_str(), "--serve" | "--no-serve")).cloned());
            relax_compiler::os::proc::exit(run_src(&words))
        }
        // `run <git url>[#ref] [--mount <dir>=<git url>[#ref]]... [--cached] <package> ...`
        Some("run") => match fetch::split_args(&args[2..]) {
            None => {}
            Some(Err(e)) => {
                eprintln!("makepad: {e}");
                std::process::exit(2);
            }
            Some(Ok((opts, spec, cached, rest))) => relax_compiler::os::proc::exit(run_spec(opts, &spec, cached, rest)),
        },
        // `fetch <git url>[#ref] [--mount ..]`: the snapshot's folder on stdout
        Some("fetch") => match fetch::split_args(&args[2..]) {
            Some(Ok((_, spec, _, rest))) if rest.is_empty() => match fetch::fetch(&fetch::home(), &spec) {
                Ok(snap) => {
                    for (dir, store, commit) in &snap.parts {
                        println!("{}\t{}\t{}", if dir.is_empty() { "." } else { dir }, store.display(), relax_compiler::gitstore::hex(commit));
                    }
                    std::process::exit(0)
                }
                Err(e) => {
                    eprintln!("makepad: {e}");
                    std::process::exit(1)
                }
            },
            Some(Err(e)) => {
                eprintln!("makepad: {e}");
                std::process::exit(2);
            }
            _ => {
                eprintln!("usage: makepad fetch <git url>[#ref] [--mount <dir>=<git url>[#ref]]...");
                std::process::exit(2);
            }
        },
        Some("export") if args.get(2).is_some_and(|a| a == "--mounted") => {
            let [out] = &args[3..] else {
                eprintln!("usage: makepad export --mounted <folder>");
                std::process::exit(2);
            };
            let table = std::env::var("MAKEPAD_RUN_VFS").unwrap_or_default();
            let workspace = std::env::var("MAKEPAD_RUN_WORKSPACE").unwrap_or_default();
            export_to(fetch::mounted(&table, &workspace), out)
        }
        Some("export") => match fetch::split_args(&args[2..]) {
            Some(Ok((_, spec, cached, rest))) if rest.len() == 1 => {
                let home = fetch::home();
                export_to(if cached { fetch::cached(&home, &spec) } else { fetch::fetch(&home, &spec) }, &rest[0])
            }
            Some(Err(e)) => {
                eprintln!("makepad: {e}");
                std::process::exit(2);
            }
            _ => {
                eprintln!("usage: makepad export <git url>[#ref] [--mount <dir>=<git url>[#ref]]... [--cached] <folder>");
                std::process::exit(2);
            }
        },
        Some(w) if !w.starts_with('-') && !help::COMMANDS.contains(&w) => std::process::exit(help::unknown(w)),
        _ => {}
    }
    // `run <package>` in a checkout: the package's run-env first (it decides what is provided)
    if args.get(1).map(String::as_str) == Some("run") {
        if let (Some(pkg), Ok(cwd)) = (run_package(&args[2..]), std::env::current_dir()) {
            apply_run_env(&cwd.display().to_string(), &pkg);
        }
    }
    provided();
    let sess = relax_compiler::session_from_env(&args);
    relax_compiler::cli_main(&sess);
}

/// `makepad run --privileged ..`: the booted app is granted the HAL's domain control (hal_host.rs).
/// The host's own flag: taken out of the command line here, before anything else reads it.
static PRIVILEGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the run was granted `--privileged` (by the command line or the embedder).
pub fn privileged() -> bool {
    PRIVILEGED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The embedder's `--privileged` (the Android host: `privileged=1` in the APK's configuration):
/// the booted app is the window manager and may start, stop and embed other apps as domains.
pub fn set_privileged() {
    PRIVILEGED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Removes `--privileged` from among the options of a `run` (before its package); whether it was there.
fn take_privileged(raw: &mut Vec<Vec<u8>>) -> bool {
    if raw.get(1).map(Vec::as_slice) != Some(b"run".as_slice()) {
        return false;
    }
    let mut i = 2;
    while i < raw.len() {
        match raw[i].as_slice() {
            b"--privileged" => {
                raw.remove(i);
                return true;
            }
            b"--units" | b"--report-to" | b"--entry" | b"--boundary" | b"--run-dir" | b"--workspace-dir" | b"--cfg" => i += 2,
            w if w.starts_with(b"--") => i += 1,
            _ => return false,
        }
    }
    false
}

/// The package a `run` names: the first word after its options (and their values).
fn run_package(words: &[String]) -> Option<String> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        match w {
            "--units" | "--report-to" | "--entry" | "--boundary" | "--run-dir" | "--workspace-dir" | "--cfg" => i += 2,
            _ if w.starts_with("--") => i += 1,
            _ => return Some(w.to_string()),
        }
    }
    None
}

/// The package's `[package.metadata.makepad.run-env]` (makepad resolve::run_env), for every way it
/// is run (a checkout, the stores, the runner's tiles): set unless the environment already has it.
fn apply_run_env(workspace: &str, package: &str) {
    // Manifest discovery does not evaluate target predicates. Freeze compilation inputs
    // after run-env has been applied.
    let discovery = std::sync::Arc::new(relax_compiler::Session::default());
    let Ok(dir) = relax_compiler::resolve::package_dir(&discovery, workspace, package) else { return };
    let Ok(text) = relax_compiler::vfs::read_to_string(format!("{dir}/Cargo.toml")) else { return };
    for (k, v) in relax_compiler::resolve::run_env(&text) {
        if std::env::var_os(&k).is_none() {
            std::env::set_var(&k, &v);
        }
    }
}

/// Runs `spec`'s package from its repositories' stores (fetched, or the cached commits): mounted
/// on Relax's VFS as a virtual workspace, then the normal run.
fn run_spec(opts: Vec<String>, spec: &fetch::Spec, cached: bool, rest: Vec<String>) -> i32 {
    if rest.first().map_or(true, |p| p.starts_with('-')) {
        eprintln!("makepad run <git url> needs a package after the URL");
        return 2;
    }
    let home = fetch::home();
    // the launcher's report from the start: what the fetch has to say (a remote it could not
    // reach, the stored commit it runs instead) reaches the runner's tile
    if let Some(i) = opts.iter().position(|o| o == "--report-to") {
        if let Some(p) = opts.get(i + 1) {
            let _ = relax_compiler::run::report_open(p);
        }
    }
    // an app's start from its launch record (`launch::resolve` named the app): its window opens
    // first and shows the source being had, or why it could not be
    if std::env::var_os("MAKEPAD_APP_ID").is_some() {
        // its window keeps its geometry in the run dir, which the mount would otherwise decide
        let workspace = fetch::workspace(&home, spec);
        relax_compiler::dirs::preset_virtual_workspace(&workspace);
        // what the run says of itself is set before its window's threads run (the environment is
        // the process's: a write while another thread reads it is a race); what only its source
        // says follows the mount (`mount_vars`)
        std::env::set_var("MAKEPAD_RUN_SPEC", spec.args().join("\u{1f}"));
        std::env::set_var("MAKEPAD_RUN_WORKSPACE", &workspace);
        let words: Vec<String> = opts.iter().chain(&rest).cloned().collect();
        let package = rest[0].clone();
        let (home, spec, opts, rest) = (home.clone(), spec.clone(), opts.clone(), rest.clone());
        let prepare = move || -> Result<std::sync::Arc<relax_compiler::Session>, String> {
            // the stores, their mounts and the run's environment live for the process, as the
            // main thread makes them for any other run: the program's heap, not the compile
            // thread's compiler arena (the settle swap destroys it)
            let workspace = relax_compiler::zone_alloc::kept_with(|| -> Result<String, String> {
                let snap = if cached { fetch::cached(&home, &spec) } else { fetch::fetch(&home, &spec) }?;
                let workspace = fetch::mount(&home, &snap)?;
                mount_vars(&workspace, &rest[0]);
                std::mem::forget(snap);
                Ok(workspace)
            })?;
            let (sess, words, invocation) = session(opts.clone(), rest.clone());
            // compiler-arena blocks (the compile thread's allocations go there): see boot_package
            std::mem::forget((workspace, words, invocation));
            Ok(sess)
        };
        (host().run_host)();
        if let Some(code) = relax_compiler::run::cmd_run_app(&package, words, Box::new(prepare)) {
            return code;
        }
    }
    let snap = match if cached { fetch::cached(&home, spec) } else { fetch::fetch(&home, spec) } {
        Ok(s) => s,
        Err(e) => {
            eprintln!("makepad: {e}");
            relax_compiler::run::report_failed(&e, 1);
            return 1;
        }
    };
    let workspace = match fetch::mount(&home, &snap) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("makepad: {e}");
            relax_compiler::run::report_failed(&e, 1);
            return 1;
        }
    };
    // and the spec itself (the runner names its own repositories for apps that share them)
    std::env::set_var("MAKEPAD_RUN_SPEC", spec.args().join("\u{1f}"));
    let code = run_mounted(&workspace, opts, rest);
    // compiler-arena blocks (the boot thread compiles): see boot_package
    std::mem::forget((home, snap, workspace));
    code
}

/// `run [options] [--root <folder>] --src <store>[@<offset>+<len>] <package> ...`: the app from a packed source
/// archive (packsrc.rs): the store, or the range of a bigger file it sits in (an APK asset), is the
/// whole workspace; nothing is read from a checkout.
fn run_src(words: &[String]) -> i32 {
    let mut opts = Vec::new();
    let mut src = None;
    let mut root = None;
    let mut i = 0;
    while i < words.len() && words[i].starts_with("--") {
        let n = match words[i].as_str() {
            "--src" => {
                let Some(s) = words.get(i + 1) else {
                    eprintln!("makepad: --src wants <store>[@<offset>+<len>]");
                    return 2;
                };
                src = Some(s.clone());
                i += 2;
                continue;
            }
            "--root" => {
                let Some(r) = words.get(i + 1) else {
                    eprintln!("makepad: --root wants a folder");
                    return 2;
                };
                root = Some(r.clone());
                i += 2;
                continue;
            }
            "--units" | "--report-to" | "--entry" | "--boundary" | "--run-dir" | "--workspace-dir" | "--cfg" => 2,
            _ => 1,
        };
        opts.extend(words[i..(i + n).min(words.len())].iter().cloned());
        i += n;
    }
    let rest = words[i..].to_vec();
    let Some(src) = src else {
        eprintln!("makepad: --src goes before the package");
        return 2;
    };
    if rest.first().map_or(true, |p| p.starts_with('-')) {
        eprintln!("makepad run --src <store> needs a package after the store");
        return 2;
    }
    let (path, range) = match src.rsplit_once('@').and_then(|(p, r)| relax_compiler::vfs::parse_range(r).map(|r| (p, Some(r)))) {
        Some((p, r)) => (p.to_string(), r),
        None => (src.clone(), None),
    };
    let (path, range) = (std::path::Path::new(&path), range);
    let mounted = match range {
        Some((offset, len)) => mount_archive(path, offset, len, root.as_deref()),
        None => std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display())).and_then(|m| mount_archive(path, 0, m.len(), root.as_deref())),
    };
    match mounted {
        Ok(workspace) => run_mounted(&workspace, opts, rest),
        Err(e) => {
            eprintln!("makepad: {e}");
            1
        }
    }
}

/// Mounts the packed source archive (`pack-src`) that sits at `offset`+`len` of `file` (a whole
/// store file, or an APK asset) as the whole workspace and returns the workspace's root: `root`, or
/// one fixed root per home (the compile cache keys a crate by its root path, so a run is warm
/// against the previous one; `--root` puts it where a checkout of the same sources sits).
pub fn mount_archive(file: &std::path::Path, offset: u64, len: u64, root: Option<&str>) -> Result<String, String> {
    // the mounted store (index, file tables) is process state: kept heap, never the compiler zone
    let _k = relax_compiler::zone_alloc::kept();
    let store = relax_compiler::gitstore::Store::open_range(file, offset, len)
        .map_err(|e| format!("{}: {e}", file.display()))?
        .ok_or_else(|| format!("{}: not a makepad run git store", file.display()))?;
    let [(commit, _)] = store.index.tables.as_slice() else {
        return Err(format!("{}: a source archive holds exactly one commit", file.display()));
    };
    let commit = *commit;
    let workspace = match root {
        Some(r) => r.trim_end_matches('/').to_string(),
        None => fetch::home().join("vfs").join("src").display().to_string(),
    };
    relax_compiler::vfs::mount(&workspace, std::sync::Arc::new(store), &commit)?;
    Ok(workspace)
}

/// `makepad export`: writes a snapshot out with `$MAKEPAD_RUN_EDITS` over it, and ends the process.
fn export_to(snap: Result<fetch::Snapshot, String>, out: &str) -> ! {
    let edits = std::env::var_os("MAKEPAD_RUN_EDITS").filter(|d| !d.is_empty()).map(std::path::PathBuf::from);
    match snap.and_then(|snap| fetch::export(&snap, edits.as_deref(), std::path::Path::new(out))) {
        Ok((files, wrote)) => {
            println!("{files} files, {wrote} written");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("makepad: {e}");
            std::process::exit(1)
        }
    }
}

/// Runs `rest` (the package and its flags) with `opts` (`cmd_run`'s options before the package)
/// in `workspace`, a virtual workspace whose stores are mounted on Relax's VFS. Also the
/// embedder's entry (the Android host mounts the APK's source store, then runs `--exports`).
pub fn run_mounted(workspace: &str, opts: Vec<String>, rest: Vec<String>) -> i32 {
    mount_env(workspace, &rest[0]);
    boot_package(opts, rest)
}

/// What a run of the mounted workspace and the programs it starts see: the workspace and its
/// mounts, its std, the package's run-env.
fn mount_env(workspace: &str, package: &str) {
    std::env::set_var("MAKEPAD_RUN_WORKSPACE", workspace);
    mount_vars(workspace, package);
}

/// What only the mounted source says: its mounts, its std, the package's run-env.
fn mount_vars(workspace: &str, package: &str) {
    // programs it starts that run on Relax (the runner's apps) mount the same repositories
    std::env::set_var("MAKEPAD_RUN_VFS", relax_compiler::vfs::mounts_text());
    // Verify the mounted std against the compiler build before any std disk probes.
    let own = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../libs/relax/compiler/std");
    relax_compiler::std_source::select(workspace, &own);
    // the run's own files (unit graph, build script outputs, code cache): no workspace on disk,
    // so its relax dir is $MAKEPAD_RUN_HOME/target/<hash of the virtual workspace> (relax dirs.rs), one
    // per spec shape (the same across updates)
    apply_run_env(workspace, package);
    // compiler-arena blocks (the boot thread compiles): see boot_package
    std::mem::forget(own);
}

/// Runs `rest` (the package and its flags) with `opts` in the current directory, a checkout:
/// the way `makepad run <package>` does, returning instead of exiting (an embedder's call).
pub fn run_checkout(opts: Vec<String>, rest: Vec<String>) -> i32 {
    if let Ok(cwd) = std::env::current_dir() {
        apply_run_env(&cwd.display().to_string(), &rest[0]);
    }
    boot_package(opts, rest)
}

fn boot_package(opts: Vec<String>, rest: Vec<String>) -> i32 {
    let _phase = relax_compiler::zone_alloc::compiler();
    let (sess, a, invocation) = session(opts, rest);
    let code = relax_compiler::run::cmd_run(&sess, &a);
    // made in the compiler arena (the boot thread's allocations go there), which the zone swap
    // after the first present destroyed: freeing them on the way out wrote into unmapped chunks
    // (SIGSEGV at every exit of `makepad` started without arguments, `makepad app <id>` or
    // `makepad run <git url>`). The callers on this path forget theirs the same way.
    std::mem::forget((sess, a, invocation));
    code
}

/// The run's session, its words and its invocation, made on the thread that compiles (an
/// embedder's boot thread, as the Android host's, compiles like the loader's main thread): its
/// allocations go to the compiler arena, std's mpmc context is entered in the kept heap first.
fn session(opts: Vec<String>, rest: Vec<String>) -> (std::sync::Arc<relax_compiler::Session>, Vec<String>, Vec<String>) {
    {
        let _k = relax_compiler::zone_alloc::kept();
        let (_tx, rx) = std::sync::mpsc::sync_channel::<()>(0);
        let _ = rx.recv_timeout(std::time::Duration::ZERO);
    }
    provided();
    let mut a = opts;
    a.extend(rest);
    let mut invocation = vec!["loader".to_string(), "run".to_string()];
    invocation.extend_from_slice(&a);
    let sess = relax_compiler::session_from_env(&invocation);
    (sess, a, invocation)
}


/// `makepad devhub <words>`: the `makepad-devhub` binary of the checkout this JIT came from (else
/// the one this folder is in), as built: the devhub master swaps a landing's build in when its
/// rebuild finishes, so a call never waits on a build lock. A checkout with none yet goes through
/// `cargo run`, which builds it. devhub is not linked here, so its changes never rebuild the JIT.
/// Returns only when it could not start.
fn devhub(words: &[String]) -> i32 {
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().ok();
    let is_root = |d: &&std::path::Path| d.join("libs/devhub/Cargo.toml").is_file();
    let root = exe.iter().flat_map(|e| e.ancestors()).find(is_root).or_else(|| cwd.iter().flat_map(|c| c.ancestors()).find(is_root));
    let Some(root) = root else {
        eprintln!("makepad devhub: no makepad checkout above {} or this folder (devhub builds from one)", exe.as_ref().map_or_else(String::new, |e| e.display().to_string()));
        return 2;
    };
    let built = root.join("target/host").join(format!("makepad-devhub{}", std::env::consts::EXE_SUFFIX));
    let mut command = if built.is_file() {
        std::process::Command::new(built)
    } else {
        let cargo = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| "cargo".into());
        let mut c = std::process::Command::new(cargo);
        c.args(["run", "--profile", "host", "-q", "--manifest-path"]).arg(root.join("Cargo.toml")).args(["-p", "makepad-devhub", "--bin", "makepad-devhub", "--"]);
        c
    };
    // (rustup counts its nested proxies in this one and refuses past 20: cargo -> makepad -> cargo must not count)
    command.args(words).env_remove("RUST_RECURSION_COUNT");
    #[cfg(unix)]
    let e = std::os::unix::process::CommandExt::exec(&mut command);
    #[cfg(not(unix))]
    let e = match command.status() {
        Ok(status) => return status.code().unwrap_or(1),
        Err(e) => e,
    };
    eprintln!("makepad devhub: {}: {e}", command.get_program().to_string_lossy());
    1
}
