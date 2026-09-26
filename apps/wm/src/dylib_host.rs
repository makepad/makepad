//! On-demand `dylib` apps for the Android super-app: `cargo rustc --crate-type
//! dylib` against the shipped checkout, then `dlopen` and the same
//! [`ModuleHost`] path as a statically linked module.
//!
//! The engine (`apps/wm-dyn/engine`, one Rust dylib that re-exports
//! `makepad-widgets` and friends) is already in the process
//! (`-C prefer-dynamic`); the app `.so` must bind it and never carry a
//! second widgets: Rust's ABI is per build, so a widgets compiled here could
//! not bind the loaded engine (and on the phone its build script cannot even
//! run: W^X). No app manifest says any of this — the host does, at the two
//! levels of the build it runs:
//!  - cargo: the packaged workspace resolves features across ALL of its
//!    members (`.cargo/config.toml` `feature-unification = "workspace"`,
//!    written by `cargo makepad android dyn-pack`; `--no-default-features` on both the
//!    engine cross-build and here), so `-p <app>` sees exactly the widgets
//!    unit the engine was built from — Fresh, never compiled;
//!  - rustc: the app's own invocation gets `--extern force:makepad_wm_engine=
//!    <engine .so>` (cargo's `--` passthrough), which loads the engine crate
//!    without a source-level `extern crate`; rustc then links every rlib the
//!    engine contains from it (`IncludedFromDylib`) instead of embedding it.
//! Both are nightly-gated on this toolchain, so every cargo child runs with
//! `RUSTC_BOOTSTRAP=1` — the Mac cross-build too: the flag is in every
//! crate's SVH. `--stdin-loop` stays the desktop process model.
//!
//! Android: the APK carries the engine `.so`s, and its assets carry the
//! rustc toolchain, the checkout and the cross-built `target/` tree. The
//! first tile open provisions those into the app's files dir
//! ([`android::provision`]); every cargo/rustc child then runs under the
//! environment that makes cargo's fingerprints match the shipped tree (the
//! identity rules: tools/cargo_makepad/src/android/dyn_pack).

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

/// The client id provisioning reports under: no tile, the desk shows its
/// lines (lib.rs `drain_client_lines`, `WmState::provision`).
pub const PROVISION_CLIENT: ClientId = 0;

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
    // refuses for a dylib). `--no-default-features` applies to every member
    // of the packaged workspace (feature unification is workspace-wide
    // there) and keeps the apps' `standalone` graphs out — the engine was
    // cross-built with the same flag, so the units match.
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
    // The engine joins the app's crate graph here, not in its manifest:
    // `--extern force:` loads it without an `extern crate` in the app, and
    // rustc links widgets and friends from the dylib that is already in
    // this process. Passthrough args reach only the app's own rustc.
    let engine = engine_dylib(&target_dir);
    if !engine.is_file() {
        return Err(format!("engine dylib missing: {}", engine.display()));
    }
    cmd.arg("--")
        .arg("-Zunstable-options")
        .arg("--extern")
        .arg(format!("force:makepad_wm_engine={}", engine.display()));
    // Android: everything lives under the app's files dir, with the exact
    // strings the Mac cross-build used (cargo hashes them). Desktop: the
    // process environment (CARGO_TARGET_DIR of the host build) is the
    // target dir, so the app links the engine dylib the host loaded.
    #[cfg(target_os = "android")]
    dyn_env.env(&mut cmd);
    #[cfg(not(target_os = "android"))]
    {
        // `-Zunstable-options` and the workspace feature unification are
        // nightly-gated; Android has this in the packed env (env.txt).
        cmd.env("RUSTFLAGS", rustflags());
        cmd.env("RUSTC_BOOTSTRAP", "1");
    }
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

fn dylib_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// The release output dir inside a target dir: the cross-compiled triple's
/// on Android, the host's elsewhere.
fn release_dir(target_dir: &Path) -> PathBuf {
    if cfg!(target_os = "android") {
        target_dir.join("aarch64-linux-android/release")
    } else {
        target_dir.join("release")
    }
}

/// The engine dylib this process loaded, as the shared target dir holds
/// it (a path package's dylib: no `-<hash>` in the name). Every app dylib
/// is linked against exactly this file (`--extern force:`), and cargo must
/// leave it alone (`Dyn::engine_stamp`).
fn engine_dylib(target_dir: &Path) -> PathBuf {
    release_dir(target_dir).join("deps").join(format!("libmakepad_wm_engine.{}", dylib_ext()))
}

fn dylib_path(root: &Path, data_dir: Option<&str>, package: &str) -> Result<PathBuf, String> {
    let stem = lib_stem(package);
    let release = release_dir(&shared_target_dir(root, data_dir));
    let name = format!("{stem}.{}", dylib_ext());
    for dir in [release.clone(), release.join("deps")] {
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

/// The Android super-app's files-dir layout and first-run provisioning: the
/// assets `cargo makepad android dyn-pack` packed, unpacked by
/// makepad-ondevice-build (libs/ondevice_build: the stamps, the streamed LZ4
/// parts, the proc-macro bootstrap and SVH rewrite, the mtime alignment).
#[cfg(target_os = "android")]
mod android {
    use super::*;
    use makepad_ondevice_build::{Assets, Toolchain};
    use makepad_widgets::makepad_platform::os::linux::android::android_jni::{load_asset, open_asset};

    /// The APK's assets through the AssetManager: this is the WM's own
    /// process, the one with a JVM.
    struct ApkAssets;

    impl Assets for ApkAssets {
        fn load(&mut self, name: &str) -> Option<Vec<u8>> {
            load_asset(name)
        }
        fn open(&mut self, name: &str) -> Option<Box<dyn std::io::Read>> {
            open_asset(name).map(|r| Box::new(r) as Box<dyn std::io::Read>)
        }
    }

    pub struct Dyn {
        tc: Toolchain,
    }

    pub fn detect(data_dir: Option<&str>) -> Result<Dyn, String> {
        let data = PathBuf::from(data_dir.ok_or("no data dir")?);
        let maps = std::fs::read_to_string("/proc/self/maps").map_err(|e| format!("maps: {e}"))?;
        let nlib = maps
            .lines()
            .filter_map(|l| l.rsplit(' ').next())
            .find(|p| p.ends_with("/libmakepad.so"))
            .and_then(|p| Path::new(p).parent().map(Path::to_path_buf))
            .ok_or("libmakepad.so is not a file on disk (extractNativeLibs?)")?;
        Ok(Dyn { tc: Toolchain::new(data, nlib, &mut ApkAssets)? })
    }

    impl Dyn {
        /// `cargo` = the musl loader running the toolchain's cargo.
        pub fn cargo(&self) -> Command {
            let mut cmd = Command::new(self.tc.nlib.join("libld-musl.so"));
            cmd.arg(self.tc.data.join("tc/bin/cargo"));
            cmd
        }

        pub fn env(&self, cmd: &mut Command) {
            self.tc.env(cmd);
        }

        /// (size, mtime) of the shipped engine `.so` in target/: cargo must
        /// leave it alone.
        pub fn engine_stamp(&self) -> Option<(u64, std::time::SystemTime)> {
            let m = std::fs::metadata(engine_dylib(&self.tc.target())).ok()?;
            Some((m.len(), m.modified().ok()?))
        }
    }

    /// First tile open: provisioning (makepad-ondevice-build). The desk
    /// shows the lines; a tap on a tile retries a failure.
    pub fn provision(d: &Dyn, lines: &Sender<ClientLine>, client: ClientId) -> Result<(), String> {
        makepad_ondevice_build::provision(&d.tc, &mut ApkAssets, &mut |text| say(lines, client, text))?;
        std::env::set_var("MAKEPAD_WM_ROOT", d.tc.src());
        Ok(())
    }
}
