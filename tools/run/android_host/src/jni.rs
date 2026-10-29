//! JNI belongs to the native HAL before Java can deliver its first Activity callback.

use crate::boot::{self, Boot, Source};
use crate::logcat;
use makepad_jni_sys as sys;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;

#[link(name = "android")]
extern "C" {
    fn AAssetManager_fromJava(env: *mut sys::JNIEnv, asset_manager: sys::jobject) -> *mut c_void;
    fn AAssetManager_open(manager: *mut c_void, name: *const c_char, mode: c_int) -> *mut c_void;
    fn AAsset_openFileDescriptor64(asset: *mut c_void, start: *mut i64, length: *mut i64) -> c_int;
    fn AAsset_close(asset: *mut c_void);
    fn AAsset_getLength(asset: *mut c_void) -> i64;
    fn AAsset_read(asset: *mut c_void, buf: *mut c_void, count: usize) -> c_int;
}

extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn close(fd: c_int) -> c_int;
}

const RTLD_NOW: c_int = 2;
const RTLD_NOLOAD: c_int = 4;
/// AASSET_MODE_UNKNOWN
const ASSET_MODE_UNKNOWN: c_int = 0;

/// The packed source archive's name in the APK's assets.
const ARCHIVE_ASSET: &str = "makepad-run-src.pack";
/// The package the APK runs and the compile's flags (cargo_makepad `android run-host` writes them
/// into assets/makepad-run-host.conf: one `key=value` per line).
const CONF_ASSET: &str = "makepad-run-host.conf";
/// A compile cache recorded on a device of this build, shipped in the APK (cargo_makepad `build-host
/// --cache-seed=<dir>`): the index lists one path per line relative to the relax home
/// (`target/<run dir>/cache/<store>`), each stored as the asset `makepad-run-seed/<path>`.
const SEED_INDEX_ASSET: &str = "makepad-run-seed/index.txt";
const SEED_ASSET_DIR: &str = "makepad-run-seed";

#[no_mangle]
pub unsafe extern "system" fn JNI_OnLoad(vm: *mut sys::JavaVM, _: *mut c_void) -> sys::jint {
    let mut env: *mut sys::JNIEnv = std::ptr::null_mut();
    if ((**vm).GetEnv.unwrap())(vm, &mut env as *mut _ as _, sys::JNI_VERSION_1_6) != 0 {
        return sys::JNI_ERR;
    }
    let class = ((**env).FindClass.unwrap())(env, c"dev/makepad/android/MakepadNative".as_ptr());
    if class.is_null() { return sys::JNI_ERR; }
    let result = makepad_hal::backend::android::register_natives(env, class);
    ((**env).DeleteLocalRef.unwrap())(env, class);
    match result {
        Ok(pending) => {
            crate::android_fleet::register();
            logcat::log(&format!("HAL shell registered; pending services: {}", pending.join(", ")))
        }
        Err(error) => {
            logcat::log(&format!("HAL registration failed: {error}"));
            return sys::JNI_ERR;
        }
    }
    sys::JNI_VERSION_1_6
}

/// Relax finds the host's exported functions through the library's own handle (see
/// os::proc::set_host_library); System.loadLibrary keeps the library out of the process-wide scope.
pub fn name_host_library() {
    let h = unsafe { dlopen(c"libmakepad.so".as_ptr(), RTLD_NOW | RTLD_NOLOAD) };
    relax_compiler::os::proc::set_host_library(h as *mut u8);
    crate::trampolines::register();
}

unsafe fn jstring_to_string(env: *mut sys::JNIEnv, s: sys::jstring) -> String {
    if s.is_null() {
        return String::new();
    }
    let get = (**env).GetStringUTFChars.unwrap();
    let release = (**env).ReleaseStringUTFChars.unwrap();
    let chars = get(env, s, std::ptr::null_mut());
    let out = CStr::from_ptr(chars).to_string_lossy().into_owned();
    release(env, s, chars);
    out
}

unsafe fn new_jstring(env: *mut sys::JNIEnv, s: &str) -> sys::jstring {
    let c = CString::new(s.replace('\0', " ")).unwrap();
    ((**env).NewStringUTF.unwrap())(env, c.as_ptr())
}

/// A named asset of the APK as (file offset, length) of the APK file: it must be stored, not
/// compressed (AAsset gives no descriptor for a deflated one).
unsafe fn asset_range(manager: *mut c_void, name: &str) -> Result<(u64, u64), String> {
    let cname = CString::new(name).unwrap();
    let asset = AAssetManager_open(manager, cname.as_ptr(), ASSET_MODE_UNKNOWN);
    if asset.is_null() {
        return Err(format!("the APK has no asset {name}"));
    }
    let (mut start, mut len) = (0i64, 0i64);
    let fd = AAsset_openFileDescriptor64(asset, &mut start, &mut len);
    AAsset_close(asset);
    if fd < 0 {
        return Err(format!("asset {name} is compressed in the APK (it must be stored)"));
    }
    close(fd);
    Ok((start as u64, len as u64))
}

unsafe fn asset_text(manager: *mut c_void, name: &str) -> Result<String, String> {
    let cname = CString::new(name).unwrap();
    let asset = AAssetManager_open(manager, cname.as_ptr(), ASSET_MODE_UNKNOWN);
    if asset.is_null() {
        return Err(format!("the APK has no asset {name}"));
    }
    let len = AAsset_getLength(asset) as usize;
    let mut buf = vec![0u8; len];
    let n = AAsset_read(asset, buf.as_mut_ptr() as *mut c_void, len);
    AAsset_close(asset);
    if n < 0 || n as usize != len {
        return Err(format!("cannot read asset {name}"));
    }
    String::from_utf8(buf).map_err(|_| format!("asset {name} is not UTF-8"))
}

/// Copies asset `name` to `dst` through a temporary sibling; bytes copied.
unsafe fn asset_copy(manager: *mut c_void, name: &str, dst: &std::path::Path) -> Result<u64, String> {
    let cname = CString::new(name).map_err(|e| e.to_string())?;
    let asset = AAssetManager_open(manager, cname.as_ptr(), ASSET_MODE_UNKNOWN);
    if asset.is_null() {
        return Err(format!("the APK has no asset {name}"));
    }
    let r = (|| -> Result<u64, String> {
        let dir = dst.parent().ok_or("no parent")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dst.with_file_name(format!(".seed-{}", dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?);
        let mut buf = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            let n = AAsset_read(asset, buf.as_mut_ptr() as *mut c_void, buf.len());
            if n < 0 {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("cannot read asset {name}"));
            }
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n as usize]).map_err(|e| format!("{}: {e}", tmp.display()))?;
            total += n as u64;
        }
        std::io::Write::flush(&mut out).map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&tmp, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        Ok(total)
    })();
    AAsset_close(asset);
    r
}

/// Seeds the compile cache from the APK: once, when no run dir under the relax home holds a store
/// yet (a fresh install, or a cleared cache), after freshen_cache (an older build's cache is gone
/// by then). A seed of another build or CPU class is keyed away by its store names (codegen
/// identity, arch, atomics) and costs only disk until the next prune.
unsafe fn seed_cache(manager: *mut c_void, home: &std::path::Path) {
    let Ok(index) = asset_text(manager, SEED_INDEX_ASSET) else { return };
    let has_stores = std::fs::read_dir(home.join("target")).into_iter().flatten().flatten()
        .any(|d| std::fs::read_dir(d.path().join("cache")).into_iter().flatten().flatten().any(|f| f.file_name().to_string_lossy().contains(".rcs")));
    if has_stores {
        return;
    }
    let t = std::time::Instant::now();
    let (mut n, mut bytes, mut failed) = (0usize, 0u64, 0usize);
    for rel in index.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
            continue;
        }
        match asset_copy(manager, &format!("{SEED_ASSET_DIR}/{rel}"), &home.join(rel)) {
            Ok(b) => {
                n += 1;
                bytes += b;
            }
            Err(e) => {
                failed += 1;
                logcat::log(&format!("cache seed: {rel}: {e}"));
            }
        }
    }
    logcat::log(&format!("compile cache seeded from the APK: {n} files, {} MB in {:.1} s{}", bytes >> 20, t.elapsed().as_secs_f64(), if failed > 0 { format!(", {failed} failed") } else { String::new() }));
    boot::set_note("warm start \u{b7} seeded cache");
}

struct App {
    package: String,
    cargo_flags: Vec<String>,
}

struct Conf {
    /// the apps the APK carries (`app=<package>`, then its `flag=` lines); the first is the default
    apps: Vec<App>,
    /// environment of the process (`env=KEY=VALUE`: MAKEPAD_TRACE, SANDBOX_MUTE, ...)
    env: Vec<(String, String)>,
    /// `privileged=1`: the app that boots is the window manager (domain control)
    privileged: bool,
    /// the embedded devhub node (`devhub_name=`, `rev=<repo> <sha>`, `prefix=<repo> <path>`)
    store: crate::devhub::StoreConf,
    /// the embedded Desk Link endpoint (`desk_link=on|off`, `desk_link_fps=`)
    desk: crate::deskhost::DeskConf,
}

fn parse_conf(text: &str) -> Result<Conf, String> {
    let mut apps: Vec<App> = Vec::new();
    let mut env = Vec::new();
    let mut privileged = false;
    let mut store = crate::devhub::StoreConf::default();
    let mut desk = crate::deskhost::DeskConf::default();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (k, v) = line.split_once('=').ok_or_else(|| format!("{CONF_ASSET}: bad line {line:?}"))?;
        match k {
            "app" => apps.push(App { package: v.to_string(), cargo_flags: Vec::new() }),
            "flag" => apps.last_mut().ok_or_else(|| format!("{CONF_ASSET}: flag before app"))?.cargo_flags.push(v.to_string()),
            "env" => {
                let (k, val) = v.split_once('=').ok_or_else(|| format!("{CONF_ASSET}: env wants KEY=VALUE, got {v:?}"))?;
                env.push((k.to_string(), val.to_string()));
            }
            "privileged" => privileged = v == "1",
            "devhub_name" => store.name = v.to_string(),
            "desk_link" => desk.on = v == "on",
            "desk_link_fps" => desk.fps = Some(v.parse().map_err(|_| format!("{CONF_ASSET}: desk_link_fps wants a number, got {v:?}"))?),
            "rev" | "prefix" => {
                let (repo, val) = v.split_once(' ').ok_or_else(|| format!("{CONF_ASSET}: {k} wants `<repo> <value>`, got {v:?}"))?;
                let list = if k == "rev" { &mut store.revs } else { &mut store.prefixes };
                list.push((repo.to_string(), val.to_string()));
            }
            _ => return Err(format!("{CONF_ASSET}: unknown key {k:?}")),
        }
    }
    if apps.is_empty() {
        return Err(format!("{CONF_ASSET}: no app"));
    }
    Ok(Conf { apps, env, privileged, store, desk })
}

/// Starts the process app on its own thread; Activity callbacks never wait for compilation.
#[no_mangle]
pub unsafe extern "system" fn Java_dev_makepad_run_RunActivity_boot(
    env: *mut sys::JNIEnv,
    run_activity: sys::jclass,
    apk_path: sys::jstring,
    files_dir: sys::jstring,
    cache_dir: sys::jstring,
    lib_dir: sys::jstring,
    assets: sys::jobject,
    app: sys::jstring,
    remote: sys::jstring,
) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    let mut first = false;
    STARTED.call_once(|| first = true);
    if !first {
        return;
    }
    logcat::redirect_stdio();
    // a panic of the host's own code (the compiler) is written to logcat before it can abort the process
    std::panic::set_hook(Box::new(|info| {
        logcat::log(&format!("host panic: {info}"));
        logcat::log(&format!("{}", std::backtrace::Backtrace::force_capture()));
    }));
    let new_global = (**env).NewGlobalRef.unwrap();
    let assets = new_global(env, assets);
    {
        // RunActivity.restartProcess(), for makepad_run_restart_process (any thread, later)
        let mut vm: *mut sys::JavaVM = std::ptr::null_mut();
        ((**env).GetJavaVM.unwrap())(env, &mut vm);
        let class = new_global(env, run_activity);
        RESTART.store(Box::into_raw(Box::new((vm as usize, class as usize))) as usize, std::sync::atomic::Ordering::Release);
    }
    let manager = AAssetManager_fromJava(env, assets) as usize;
    // The global AssetManager reference stays alive with the packed store and HAL.
    makepad_hal::backend::android::assets::set_manager(manager as *mut c_void);
    let apk = PathBuf::from(jstring_to_string(env, apk_path));
    let files = PathBuf::from(jstring_to_string(env, files_dir));
    let cache = jstring_to_string(env, cache_dir);
    let lib_dir = PathBuf::from(jstring_to_string(env, lib_dir));
    let app = jstring_to_string(env, app);
    let remote = jstring_to_string(env, remote);
    boot::set_state("starting", String::new());
    let spawned = std::thread::Builder::new().name("run-boot".into()).stack_size(64 << 20).spawn(move || {
        let result = boot_thread(manager as *mut c_void, &apk, &files, &cache, &lib_dir, &app, &remote);
        match result {
            Ok(()) => boot::set_state("exited", String::new()),
            Err(e) => {
                logcat::log(&format!("boot failed: {e}"));
                boot::set_state("failed", e);
            }
        }
        // the thread that booted the program stays (Relax names it the main thread)
        loop {
            std::thread::park();
        }
    });
    if let Err(e) = spawned {
        boot::set_state("failed", format!("cannot start the boot thread: {e}"));
    }
}

/// The compile cache persists: a fresh process boots warm from it, and an APK update keeps it
/// (Relax keys its stored builds by its codegen identity, relax_compiler::codegen_id, and
/// recompiles only sources that changed). It is deleted only for a host library with another code
/// generator (another codegen identity: none of its code could be used) or when the window manager's settings ("Clear JIT cache") leave a
/// `clear-cache` file in the home folder. The old cache is renamed away at once and deleted on a
/// thread of its own, so the boot does not wait for the delete.
/// Answers whether the packed sources changed (another archive than the last process's).
fn freshen_cache(home: &std::path::Path, offset: u64, len: u64) -> bool {
    let stamp_file = home.join("build.stamp");
    let stamp = format!("{:032x}", relax_compiler::codegen_id::id());
    let pack_file = home.join("pack.stamp");
    let pack = format!("{offset} {len}");
    let other_pack = std::fs::read_to_string(&pack_file).map_or(true, |s| s != pack);
    if other_pack { let _ = std::fs::write(&pack_file, &pack); }
    let other_build = std::fs::read_to_string(&stamp_file).map_or(true, |s| s != stamp);
    let cleared = home.join("clear-cache");
    let asked = cleared.exists();
    let target = home.join("target");
    if (other_build || asked) && target.exists() {
        let trash = home.join(format!("target.old-{}", std::process::id()));
        if std::fs::rename(&target, &trash).is_err() {
            let _ = std::fs::remove_dir_all(&target);
        }
        logcat::log(&format!("compile cache cleared at process start ({})", if asked { "cleared in settings" } else { "another code generator" }));
        boot::set_note("cold start \u{b7} cache cleared");
    }
    let _ = std::fs::remove_file(&cleared);
    let tmp = home.join("build.stamp.new");
    if std::fs::write(&tmp, &stamp).is_ok() { let _ = std::fs::rename(&tmp, &stamp_file); }
    // this and earlier processes' renamed caches (a delete cut short by a kill stays until here)
    let home = home.to_path_buf();
    let other = other_pack;
    let _ = std::thread::Builder::new().name("cache-delete".into()).spawn(move || {
        let Ok(entries) = std::fs::read_dir(&home) else { return };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("target.old-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    });
    other
}

fn boot_thread(
    manager: *mut c_void,
    apk: &std::path::Path,
    files: &std::path::Path,
    cache: &str,
    lib_dir: &std::path::Path,
    app: &str,
    remote: &str,
) -> Result<(), String> {
    name_host_library();
    let relax_home = files.join("makepad-run");
    std::fs::create_dir_all(&relax_home).map_err(|e| format!("{}: {e}", relax_home.display()))?;
    // an Android app has no HOME and /tmp is not its own
    std::env::set_var("HOME", files);
    std::env::set_var("TMPDIR", cache);
    std::env::set_var("MAKEPAD_RUN_HOME", &relax_home);
    if let Some(mb) = boot::memory_budget_mb() {
        std::env::set_var("MAKEPAD_RUN_MEM_BUDGET_MB", mb.to_string());
    }
    crate::socket::register();
    // makepad-remote: dial the dev machine's hub so the phone is reachable like a desktop (no run-as, no shell)
    let remote_file = files.join("remote.spec");
    let spec = if remote.is_empty() { std::fs::read_to_string(&remote_file).unwrap_or_default() } else { let _ = std::fs::write(&remote_file, remote); remote.to_string() };
    if !spec.trim().is_empty() {
        match relax_runner::remote::dial_in_background(spec.trim(), files) {
            Ok(()) => logcat::log("makepad-remote: dialling the hub (from the intent / remote.spec)"),
            Err(e) => logcat::log(&format!("makepad-remote: not dialling: {e}")),
        }
    }
    let conf = parse_conf(&unsafe { asset_text(manager, CONF_ASSET)? })?;
    let (offset, len) = unsafe { asset_range(manager, ARCHIVE_ASSET)? };
    let other_pack = freshen_cache(&relax_home, offset, len);
    seed_cache(manager, &relax_home);
    // devhub pushes live in the run's edits folder over the APK's source store; other packed
    // sources start from their own revs (the conf's)
    if other_pack {
        let _ = std::fs::remove_dir_all(files.join("devhub"));
    }
    let edits = crate::devhub::edits_dir(files);
    std::fs::create_dir_all(&edits).map_err(|e| format!("{}: {e}", edits.display()))?;
    std::env::set_var("MAKEPAD_RUN_EDITS", &edits);
    std::env::set_var("MAKEPAD_RUN_STORE_LIVE", "1");
    relax_compiler::vfs::set_edits(Some(edits));
    for (k, v) in &conf.env {
        std::env::set_var(k, v);
    }
    let chosen = if app.is_empty() {
        conf.apps.first().map(|a| App { package: a.package.clone(), cargo_flags: a.cargo_flags.clone() })
    } else {
        conf.apps.iter().find(|a| a.package == app).map(|a| App { package: a.package.clone(), cargo_flags: a.cargo_flags.clone() })
    }
    .ok_or_else(|| format!("the APK does not carry the app {app:?}"))?;
    let b = Boot {
        package: chosen.package,
        cargo_flags: chosen.cargo_flags,
        source: Source::Archive { file: apk.to_path_buf(), offset, len },
        exe: lib_dir.join("libmakepad_exe.so"),
        // the window manager is the app the APK boots first; a launch-intent app is an ordinary one
        privileged: conf.privileged && app.is_empty(),
        on_mounted: {
            let (files, store, desk) = (files.to_path_buf(), conf.store.clone(), conf.desk.clone());
            Some(Box::new(move |workspace: &str| {
                let name = if store.name.is_empty() { makepad_devhub::proto::default_machine_name() } else { store.name.clone() };
                // after devhub's start: it sets the fleet dir the endpoint's credential is in
                crate::devhub::start(&files, workspace, store);
                crate::deskhost::start(&files, name, desk);
            }))
        },
    };
    logcat::log(&format!("booting {} from {} at {offset}+{len}", b.package, apk.display()));
    boot::boot(b)
}

/// `RunActivity.status()`: "state|phase|loaded|total|error" (error last: it may hold anything).
#[no_mangle]
pub unsafe extern "system" fn Java_dev_makepad_run_RunActivity_status(env: *mut sys::JNIEnv, _: sys::jclass) -> sys::jstring {
    let s = boot::status();
    let fns = relax_compiler::jit::COMPILED_FNS.load(std::sync::atomic::Ordering::Relaxed);
    new_jstring(env, &format!("{}|{}|{}|{}|{}|{}", s.state, s.phase, s.loaded, s.total, fns, s.error.replace('\n', " ")))
}

/// (JavaVM, global ref of RunActivity's class), set at boot.
static RESTART: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The program asks for a new process (the window manager's settings, "restart now" after a
/// cache clear): RunActivity.restartProcess() starts the task again and kills this process.
/// Called by name from JIT code (`extern "C"`), on any thread.
#[no_mangle]
pub extern "C" fn makepad_run_restart_process() {
    let p = RESTART.load(std::sync::atomic::Ordering::Acquire);
    if p == 0 { return; }
    unsafe {
        let (vm, class) = *(p as *const (usize, usize));
        let vm = vm as *mut sys::JavaVM;
        let mut env: *mut sys::JNIEnv = std::ptr::null_mut();
        if ((**vm).AttachCurrentThread.unwrap())(vm, &mut env, std::ptr::null_mut()) != 0 || env.is_null() { return; }
        let class = class as sys::jclass;
        let id = ((**env).GetStaticMethodID.unwrap())(env, class, c"restartProcess".as_ptr(), c"()V".as_ptr());
        if !id.is_null() {
            ((**env).CallStaticVoidMethodA.unwrap())(env, class, id, std::ptr::null());
        }
    }
}
