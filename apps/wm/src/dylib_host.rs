//! On-demand `dylib` apps for the Android super-app: `cargo rustc --crate-type
//! dylib` against the shipped checkout, then `dlopen` and the same
//! [`ModuleHost`] path as a statically linked module.
//!
//! The engine (`apps/wm-dyn/engine`, one Rust dylib that re-exports
//! `makepad-widgets` and friends) must already be in the process
//! (`-C prefer-dynamic`) so the app `.so` binds the host's `Cx`, not a second
//! copy. `--stdin-loop` stays the desktop process model.
//!
//! Android: the APK carries the engine `.so`s, and its assets carry the
//! rustc toolchain, the checkout and the cross-built `target/` tree. The
//! first tile open provisions those into the app's files dir
//! ([`android::provision`]); every cargo/rustc child then runs under the
//! environment that makes cargo's fingerprints match the shipped tree (see
//! local/wm-dyn/STATUS.md, "Identity").

use crate::clients::{self, AppDef, ClientLine};
use crate::hub::ClientId;
use libloading::Library;
use makepad_widgets::log;
use makepad_app_module::AppModule;
use makepad_widgets::makepad_platform::thread::{SignalToUI, ThreadOptions, ThreadSpawner};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

const ENTRY: &[u8] = b"makepad_app_module\0";

pub struct LoadedDylib {
    /// Kept so the module vtable stays valid.
    _lib: Library,
    pub module: &'static dyn AppModule,
}

pub struct CompileDone {
    pub client: ClientId,
    pub app_id: String,
    pub home_tile: bool,
    pub result: Result<PathBuf, String>,
}

pub struct DylibHost {
    loaded: HashMap<String, LoadedDylib>,
    tx: Sender<CompileDone>,
    rx: Receiver<CompileDone>,
    lines: Sender<ClientLine>,
    /// Tiles compile concurrently; provisioning and the engine identity
    /// check run under this lock, once.
    provision: Arc<Mutex<()>>,
}

impl DylibHost {
    pub fn new(lines: Sender<ClientLine>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { loaded: HashMap::new(), tx, rx, lines, provision: Arc::new(Mutex::new(())) }
    }

    pub fn get(&self, app_id: &str) -> Option<&'static dyn AppModule> {
        self.loaded.get(app_id).map(|l| l.module)
    }

    pub fn drain_done(&mut self) -> Vec<CompileDone> {
        let mut out = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// Load a built `.so`/`.dylib`. Cached per app id for the process.
    pub fn load_path(&mut self, app_id: &str, path: &Path) -> Result<&'static dyn AppModule, String> {
        if let Some(loaded) = self.loaded.get(app_id) {
            return Ok(loaded.module);
        }
        let lib = open_library(path)?;
        let module = unsafe {
            let f: libloading::Symbol<fn() -> &'static dyn AppModule> =
                lib.get(ENTRY).map_err(|e| format!("makepad_app_module: {e}"))?;
            f()
        };
        self.loaded.insert(app_id.to_string(), LoadedDylib { _lib: lib, module });
        Ok(module)
    }

    /// Run `cargo rustc --crate-type dylib` on a worker. Lines go to the
    /// tile; [`drain_done`] yields the path or error.
    pub fn compile(
        &self,
        spawner: &ThreadSpawner,
        app: AppDef,
        client: ClientId,
        home_tile: bool,
        data_dir: Option<String>,
    ) {
        let tx = self.tx.clone();
        let lines = self.lines.clone();
        let app_id = app.id.clone();
        let provision = self.provision.clone();
        let submitted = spawner.spawn_worker(
            ThreadOptions { name: Some(format!("wm-dylib-{client}").into()), ..Default::default() },
            move || {
                let result = compile_app(&app, client, &lines, data_dir.as_deref(), &provision);
                let _ = tx.send(CompileDone { client, app_id: app.id, home_tile, result });
                SignalToUI::set_ui_signal();
            },
        );
        if let Err(e) = submitted {
            let _ = self.tx.send(CompileDone {
                client,
                app_id,
                home_tile,
                result: Err(format!("could not queue compile: {e}")),
            });
        }
    }
}

fn say(lines: &Sender<ClientLine>, client: ClientId, text: impl Into<String>) {
    let text = text.into();
    log!("wm: {text}");
    let _ = lines.send(ClientLine { client, text });
    SignalToUI::set_ui_signal();
}

fn compile_app(
    app: &AppDef,
    client: ClientId,
    lines: &Sender<ClientLine>,
    data_dir: Option<&str>,
    provision: &Mutex<()>,
) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    let dyn_env = {
        let d = android::detect(data_dir)?;
        {
            let _guard = provision.lock().map_err(|_| "provisioning lock poisoned".to_string())?;
            android::provision(&d, lines, client)?;
        }
        d
    };
    #[cfg(not(target_os = "android"))]
    let _ = provision;

    let root = clients::repo_root().ok_or_else(|| {
        "MAKEPAD_WM_ROOT is not a makepad checkout (packaged tree missing)".to_string()
    })?;
    // Always the workspace root + one target dir. A per-crate manifest
    // would isolate the unit graph and rebuild widgets for every app.
    let target_dir = shared_target_dir(&root, data_dir);
    say(
        lines,
        client,
        format!("target dir {}", target_dir.display()),
    );
    #[cfg(target_os = "android")]
    let mut cmd = dyn_env.cargo();
    #[cfg(not(target_os = "android"))]
    let mut cmd = Command::new(cargo_bin());
    cmd.current_dir(&root);
    // `--crate-type dylib` is cargo's own flag, not a `--` passthrough: only
    // then does cargo hand rustc the deps as `.rlib`s (a passthrough leaves
    // the unit an rlib in cargo's eyes and it passes `.rmeta`s, which rustc
    // refuses for a dylib). `--no-default-features` keeps the app's
    // `standalone` graph out, so the engine's feature set is the host's.
    cmd.arg("rustc").arg("--release").arg("--lib").arg("--crate-type").arg("dylib");
    if cfg!(target_os = "android") {
        cmd.arg("--offline").arg("--frozen");
        cmd.arg("--target").arg("aarch64-linux-android");
    }
    cmd.arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target_dir)
        .arg("-p")
        .arg(&app.package)
        .arg("--no-default-features")
        .arg("--features")
        .arg("dynamic-module");
    // Android: everything lives under the app's files dir, with the exact
    // strings the Mac cross-build used (cargo hashes them). Desktop: the
    // process environment (CARGO_TARGET_DIR of the host build) is the
    // target dir, so the app links the engine dylib the host loaded.
    #[cfg(target_os = "android")]
    dyn_env.env(&mut cmd);
    #[cfg(not(target_os = "android"))]
    cmd.env("RUSTFLAGS", rustflags());
    #[cfg(target_os = "android")]
    let engine_before = dyn_env.engine_stamp();

    say(lines, client, format!("cargo rustc -p {} --crate-type dylib", app.package));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cargo: {e}"))?;
    let mut err = String::new();
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let text = line.trim().to_string();
            if text.is_empty() {
                continue;
            }
            say(lines, client, text.clone());
            if err.len() < 1200 {
                err.push_str(&text);
                err.push('\n');
            }
        }
    }
    let status = child.wait().map_err(|e| format!("cargo wait: {e}"))?;
    if !status.success() {
        return Err(format!("cargo rustc failed for {} ({status}): {err}", app.package));
    }
    #[cfg(target_os = "android")]
    {
        // Identity guard: if cargo touched the engine, the app `.so` binds a
        // different engine than the one this process loaded — never load it.
        if dyn_env.engine_stamp() != engine_before {
            return Err(
                "engine rebuilt on device: the packaged target/ did not stay fresh (identity mismatch)"
                    .to_string(),
            );
        }
    }
    let path = dylib_path(&root, data_dir, &app.package)?;
    say(lines, client, format!("built {}", path.display()));
    Ok(path)
}

/// One cargo cache for every on-demand app: Android files/target, else
/// `CARGO_TARGET_DIR`, else `<checkout>/target`.
fn shared_target_dir(root: &Path, data_dir: Option<&str>) -> PathBuf {
    if let Some(dir) = data_dir {
        return PathBuf::from(dir).join("target");
    }
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    root.join("target")
}

fn rustflags() -> String {
    let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    if !flags.split_whitespace().any(|t| t.contains("prefer-dynamic")) {
        if !flags.is_empty() {
            flags.push(' ');
        }
        flags.push_str("-C prefer-dynamic");
    }
    flags
}

#[cfg(not(target_os = "android"))]
fn cargo_bin() -> PathBuf {
    if let Ok(cargo) = std::env::var("CARGO") {
        return PathBuf::from(cargo);
    }
    PathBuf::from("cargo")
}

fn lib_stem(package: &str) -> String {
    format!("lib{}", package.replace('-', "_"))
}

fn dylib_path(root: &Path, data_dir: Option<&str>, package: &str) -> Result<PathBuf, String> {
    let stem = lib_stem(package);
    let ext = if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let target_root = shared_target_dir(root, data_dir);
    let triple = if cfg!(target_os = "android") {
        "aarch64-linux-android/release"
    } else {
        "release"
    };
    let name = format!("{stem}.{ext}");
    for dir in [
        target_root.join(triple),
        target_root.join(triple).join("deps"),
    ] {
        let p = dir.join(&name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(format!("built library not found: {name}"))
}

fn open_library(path: &Path) -> Result<Library, String> {
    #[cfg(target_os = "android")]
    {
        // dlopen from the app's files dir is allowed (mmap-exec of
        // app_data_file); the memfd copy is the fallback for a namespace
        // that refuses the path.
        match unsafe { Library::new(path) } {
            Ok(lib) => Ok(lib),
            Err(e) => {
                log!("wm: dlopen {}: {e}; retrying through memfd", path.display());
                load_android_memfd(path)
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        unsafe { Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(target_os = "android")]
fn load_android_memfd(path: &Path) -> Result<Library, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let cname = std::ffi::CString::new("makepad_app").unwrap();
    let fd = unsafe { memfd_create(cname.as_ptr(), 1) };
    if fd < 0 {
        return Err("memfd_create failed".into());
    }
    let mut off = 0;
    while off < bytes.len() {
        let n = unsafe { write(fd, bytes.as_ptr().add(off), bytes.len() - off) };
        if n <= 0 {
            unsafe { close(fd) };
            return Err("memfd write failed".into());
        }
        off += n as usize;
    }
    let proc = format!("/proc/self/fd/{fd}");
    unsafe { Library::new(&proc) }.map_err(|e| {
        unsafe { close(fd) };
        format!("dlopen (memfd): {e}")
    })
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    fn memfd_create(name: *const core::ffi::c_char, flags: u32) -> i32;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
}

/// The Android super-app's files-dir layout and first-run provisioning.
///
/// Assets (`assets/wmdyn/…`, packed by local/wm-dyn/pack.py):
/// `stamp` (pack id), `env.txt` (KEY=VALUE: the linker string and RUSTFLAGS
/// of the Mac cross-build), `proc-macros.txt` (host crates to bootstrap),
/// `proc-macro-svh.txt` (the SVH each rebuilt proc-macro must carry),
/// `manifest.txt` (`name parts bytes` per tarball) and `<name>.NNN` parts
/// of `src.tar.gz`, `target.tar.gz`, `tc.tar.gz`.
///
/// nativeLibraryDir (`apk_data_file`, executable) carries the musl loader
/// `libld-musl.so`, `libbusybox.so`, and the `#!/system/bin/sh` drivers
/// `librustc-wrap.so` / `libmk-ld.so` / `libmk-host-ld.so`. Everything under
/// files/ is only ever mmap-exec'd (allowed), never execve'd (denied).
#[cfg(target_os = "android")]
mod android {
    use super::*;
    use makepad_widgets::makepad_platform::os::linux::android::android_jni::load_asset;
    use std::io::Write;

    pub struct Dyn {
        pub data: PathBuf,
        pub nlib: PathBuf,
        env: Vec<(String, String)>,
    }

    const STAMP: &str = ".wmdyn-stamp";

    pub fn detect(data_dir: Option<&str>) -> Result<Dyn, String> {
        let data = PathBuf::from(data_dir.ok_or("no data dir")?);
        let maps = std::fs::read_to_string("/proc/self/maps").map_err(|e| format!("maps: {e}"))?;
        let nlib = maps
            .lines()
            .filter_map(|l| l.rsplit(' ').next())
            .find(|p| p.ends_with("/libmakepad.so"))
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
            .ok_or("libmakepad.so is not a file on disk (extractNativeLibs?)")?;
        let env = load_asset("wmdyn/env.txt")
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or("asset wmdyn/env.txt missing")?
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        Ok(Dyn { data, nlib, env })
    }

    impl Dyn {
        /// `cargo` = the musl loader running the toolchain's cargo.
        pub fn cargo(&self) -> Command {
            let mut cmd = Command::new(self.nlib.join("libld-musl.so"));
            cmd.arg(self.data.join("tc/bin/cargo"));
            cmd
        }

        pub fn env(&self, cmd: &mut Command) {
            cmd.env_remove("MAKEPAD");
            cmd.env("RUSTC", self.nlib.join("librustc-wrap.so"));
            cmd.env("MAKEPAD_DYN_DIR", &self.data);
            cmd.env("MAKEPAD_DYN_NLIB", &self.nlib);
            cmd.env("CARGO_HOME", self.data.join("cargo"));
            cmd.env("CARGO_TARGET_DIR", self.data.join("target"));
            cmd.env("HOME", self.data.join("home"));
            cmd.env("TMPDIR", self.data.join("tmp"));
            cmd.env("PATH", "/system/bin");
            cmd.env("CARGO_TERM_COLOR", "never");
            cmd.env("CARGO_BUILD_JOBS", "4");
            for (k, v) in &self.env {
                cmd.env(k, v);
            }
        }

        /// (size, mtime) of the shipped engine `.so` in target/: cargo must
        /// leave it alone.
        pub fn engine_stamp(&self) -> Option<(u64, std::time::SystemTime)> {
            let p = self.data.join("target/aarch64-linux-android/release/deps/libmakepad_wm_engine.so");
            let m = std::fs::metadata(p).ok()?;
            Some((m.len(), m.modified().ok()?))
        }
    }

    pub fn provision(d: &Dyn, lines: &Sender<ClientLine>, client: ClientId) -> Result<(), String> {
        let stamp = load_asset("wmdyn/stamp")
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or("asset wmdyn/stamp missing: APK packed without local/wm-dyn/pack.py")?;
        if std::fs::read_to_string(d.data.join(STAMP)).ok().as_deref() == Some(stamp.as_str()) {
            std::env::set_var("MAKEPAD_WM_ROOT", d.data.join("src"));
            return Ok(());
        }
        say(lines, client, format!("provisioning {} into {}", stamp.trim(), d.data.display()));
        for dir in ["src", "target", "tc", "cargo", "home", "tmp"] {
            let _ = std::fs::remove_dir_all(d.data.join(dir));
        }
        let _ = std::fs::remove_file(d.data.join(STAMP));
        for dir in ["cargo", "home", "tmp"] {
            std::fs::create_dir_all(d.data.join(dir)).map_err(|e| format!("mkdir {dir}: {e}"))?;
        }
        let manifest = load_asset("wmdyn/manifest.txt")
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or("asset wmdyn/manifest.txt missing")?;
        for line in manifest.lines() {
            let mut it = line.split_whitespace();
            let (Some(name), Some(parts), Some(bytes)) = (it.next(), it.next(), it.next()) else { continue };
            let parts: usize = parts.parse().map_err(|_| format!("manifest: {line}"))?;
            say(lines, client, format!("extracting {name} ({bytes} bytes, {parts} parts)"));
            extract(d, name, parts, lines, client)?;
        }
        std::env::set_var("MAKEPAD_WM_ROOT", d.data.join("src"));
        bootstrap(d, lines, client)?;
        patch_proc_macro_svh(d, lines, client)?;
        say(lines, client, "aligning target/ mtimes");
        bump_mtimes(&d.data.join("target"))?;
        std::fs::write(d.data.join(STAMP), &stamp).map_err(|e| format!("stamp: {e}"))?;
        say(lines, client, "provisioned");
        Ok(())
    }

    /// Concatenate asset parts to a file, then `busybox tar -xzf` that file.
    /// Piping stdin died immediately (Broken pipe) on this Pixel.
    fn extract(d: &Dyn, name: &str, parts: usize, lines: &Sender<ClientLine>, client: ClientId) -> Result<(), String> {
        let tar_path = d.data.join(name);
        {
            let mut out = std::fs::File::create(&tar_path).map_err(|e| format!("create {name}: {e}"))?;
            for i in 0..parts {
                let part = format!("wmdyn/{name}.{i:03}");
                let bytes = load_asset(&part).ok_or_else(|| format!("asset {part} missing"))?;
                out.write_all(&bytes).map_err(|e| format!("write {name}: {e}"))?;
                let pct = ((i + 1) * 100) / parts;
                say(lines, client, format!("extracting {name}: {}/{parts} ({pct}%)", i + 1));
            }
            out.flush().map_err(|e| format!("flush {name}: {e}"))?;
        }
        say(lines, client, format!("unpacking {name}"));
        let report = makepad_tar::unpack_file(&tar_path, &d.data).map_err(|e| format!("unpack {name}: {e}"))?;
        say(lines, client, format!("unpacked {name}: {} files, {} links", report.files, report.symlinks + report.hardlinks));
        let _ = std::fs::remove_file(&tar_path);
        Ok(())
    }

    /// The host-kind units the Mac could not ship (Mach-O): proc-macros and
    /// their host rlibs, rebuilt here by the musl rustc as dependencies of
    /// the generated `wmdyn-bootstrap` crate (build-override profile, the
    /// resolved features: the same unit hashes the engine's fingerprints
    /// name). The engine units stay Fresh once the mtimes are aligned.
    fn bootstrap(d: &Dyn, lines: &Sender<ClientLine>, client: ClientId) -> Result<(), String> {
        let mut cmd = d.cargo();
        cmd.current_dir(d.data.join("src"));
        cmd.args(["build", "--release", "--offline", "--frozen", "--target", "aarch64-linux-android"]);
        cmd.args(["-p", "wmdyn-bootstrap"]);
        d.env(&mut cmd);
        say(lines, client, "bootstrapping proc-macros with the on-device rustc");
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("cargo (bootstrap): {e}"))?;
        let mut err = String::new();
        if let Some(stderr) = child.stderr.take() {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let text = line.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                say(lines, client, text.clone());
                if err.len() < 800 {
                    err.push_str(&text);
                    err.push('\n');
                }
            }
        }
        let status = child.wait().map_err(|e| format!("cargo wait: {e}"))?;
        if !status.success() {
            return Err(format!("proc-macro bootstrap failed ({status}): {err}"));
        }
        Ok(())
    }

    /// The SVH law (crate::rmeta): the engine's metadata names every
    /// proc-macro by the SVH of the Mac's Mach-O build, and a proc-macro the
    /// musl rustc rebuilt carries another one (host triple, host std are in
    /// the hash) — rustc then fails the first app build with E0463 "can't
    /// find crate for makepad_micro_serde_derive which makepad_wm_engine
    /// depends on". So each rebuilt `.so` gets the recorded SVH written into
    /// its header (assets/wmdyn/proc-macro-svh.txt: `lib<crate>-<hash>.so
    /// <svh>`, from pack.py), before the mtime bump. A listed file the
    /// bootstrap did not produce means the unit hashes differ from the
    /// Mac's: the engine would not be Fresh either — stop here, say which.
    fn patch_proc_macro_svh(d: &Dyn, lines: &Sender<ClientLine>, client: ClientId) -> Result<(), String> {
        let list = load_asset("wmdyn/proc-macro-svh.txt")
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or("asset wmdyn/proc-macro-svh.txt missing: APK packed by an older local/wm-dyn/pack.py")?;
        let deps = d.data.join("target/release/deps");
        for line in list.lines() {
            let mut it = line.split_whitespace();
            let (Some(file), Some(hex)) = (it.next(), it.next()) else { continue };
            let want = crate::rmeta::parse_svh(hex).ok_or_else(|| format!("proc-macro-svh.txt: bad svh {hex}"))?;
            let path = deps.join(file);
            let mut data = std::fs::read(&path)
                .map_err(|e| format!("bootstrap produced no {file} (unit hash differs from the Mac's?): {e}"))?;
            let h = crate::rmeta::read_header(&data).map_err(|e| format!("{file}: {e}"))?;
            if h.svh == want {
                say(lines, client, format!("{file}: svh already {hex}"));
                continue;
            }
            crate::rmeta::set_svh(&mut data, &h, want);
            std::fs::write(&path, &data).map_err(|e| format!("write {file}: {e}"))?;
            say(
                lines,
                client,
                format!("{file}: svh {} -> {hex} ({} {})", crate::rmeta::svh_hex(&h.svh), h.name, h.triple),
            );
        }
        Ok(())
    }

    /// Every file under target/ gets the same mtime, newer than any source:
    /// cargo's "dependency output newer than mine" staleness rule cannot
    /// fire between the Mac-built engine and the device-built proc-macros.
    fn bump_mtimes(dir: &Path) -> Result<(), String> {
        // One whole-second instant for every file: cargo compares dependency
        // output mtimes with strict "newer than", nanoseconds included.
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let rd = std::fs::read_dir(&d).map_err(|e| format!("read_dir {}: {e}", d.display()))?;
            for entry in rd.flatten() {
                let p = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(p);
                } else if ft.is_file() {
                    if let Ok(f) = std::fs::File::options().write(true).open(&p) {
                        let _ = f.set_modified(now);
                    }
                }
            }
        }
        Ok(())
    }
}
