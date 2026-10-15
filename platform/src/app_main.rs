use crate::cx::Cx;
use crate::event::Event;
use crate::font_policy::FontSet;
use crate::ui_runner::UiRunner;
use makepad_script::{ScriptValue, ScriptVm};

/// Windows desktop apps are linked as windowed programs (no console of
/// their own when started from Explorer). Started from a terminal, such a
/// program has no stdout: join the terminal's console so its output shows
/// there. Pipes and console builds already have a stdout and are left alone.
pub fn attach_parent_console() {
    #[cfg(target_os = "windows")]
    unsafe {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(which: u32) -> isize;
            fn AttachConsole(process: u32) -> i32;
        }
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out == 0 || out == -1 {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// `app_main!`'s JNI_OnLoad for a standalone Android app: ART loads libmakepad.so (the app
/// crate as a cdylib). Chosen here so the app crate never sees the `relax` cfg.
#[cfg(all(target_os = "android", not(relax)))]
#[macro_export]
#[doc(hidden)]
macro_rules! android_jni_entry {
    ($app:path) => {
        #[no_mangle]
        pub unsafe extern "system" fn JNI_OnLoad(vm: *mut std::ffi::c_void, _reserved: *mut std::ffi::c_void) -> i32 {
            $crate::android_on_load(vm, $app)
        }
    };
}

#[cfg(not(all(target_os = "android", not(relax))))]
#[macro_export]
#[doc(hidden)]
macro_rules! android_jni_entry {
    ($app:path) => {};
}

/// The process entry of a native app. Under the HAL on macOS/iOS/tvOS/Linux/Windows the HAL owns the
/// main thread and runs the OS shell there (on iOS / tvOS `UIApplicationMain`, which never
/// returns); the app, with its `Cx`, runs on its own thread. Elsewhere the app runs right here.
/// JNI_OnLoad of a standalone Android app (its libmakepad.so is the app, natively compiled):
/// the HAL shell registers its natives and runs `app` on its own thread once the Activity exists.
/// `app_main!` exports it; a packed app under makepad run never gets here (the host owns JNI).
/// # Safety
/// Called by ART with its JavaVM, once per process.
#[cfg(all(target_os = "android", not(relax)))]
#[doc(hidden)]
pub unsafe fn android_on_load(vm: *mut std::ffi::c_void, app: fn()) -> i32 {
    makepad_android_state::jni_on_load(vm);
    unsafe fn activity_ready(activity: makepad_jni_sys::jobject) {
        if let Some(set) = makepad_android_state::get_activity_setter_fn() {
            set(activity);
        }
    }
    makepad_hal::backend::android::standalone::on_load(vm, app, activity_ready)
}

/// `app` is the whole of the app's main: it creates its `Cx` itself.
/// Under Relax the host has done the same around the program's `main` (it gave the process's main
/// thread to the HAL and called `main` on a thread of its own), so the app just runs.
pub fn run_app(app: fn()) {
    #[cfg(all(any(target_vendor = "apple", target_os = "linux", target_os = "windows"), not(relax)))]
    {
        /// Ends the main loop when the app thread is done, however it is done.
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                {
                    if std::env::var_os("MAKEPAD_HAL_LOG").is_some() {
                        eprintln!("[hal] app thread ending (panicking={}); closing remaining HAL sessions before stopping main loop", std::thread::panicking());
                    }
                    // A standalone app can return or unwind without submitting Quit. The
                    // process-global backend would then retain its swapchains forever,
                    // while the shell waited for their native window leases to retire.
                    let hal = makepad_hal::Hal::global();
                    for session in hal.with_world(|world| world.session_ids()) {
                        hal.close(session);
                    }
                }
                makepad_hal::stop_main_loop();
            }
        }
        makepad_hal::expect_main_loop();
        let thread = std::thread::Builder::new()
            .name("makepad-app".into())
            .stack_size(8 << 20)
            .spawn(move || {
                let _done = Done;
                app();
            })
            .expect("the app thread");
        makepad_hal::run_main_loop();
        #[cfg(target_os = "linux")]
        if std::env::var_os("MAKEPAD_HAL_LOG").is_some() {
            eprintln!("[hal-linux] main loop returned; joining app thread");
        }
        if thread.join().is_err() {
            std::process::exit(101);
        }
        #[cfg(target_os = "linux")]
        if std::env::var_os("MAKEPAD_HAL_LOG").is_some() {
            eprintln!("[hal-linux] app thread joined; run_app returning");
        }
    }
    #[cfg(not(all(any(target_vendor = "apple", target_os = "linux", target_os = "windows"), not(relax))))]
    app();
}

/// Whether this process runs hosted: its host launched it as a `--stdin-loop` child, or a
/// makepad window manager admitted it ([`discover_window_manager`]).
pub fn should_run_stdin_loop_from_env() -> bool {
    launched_by_host() || admitted_by_window_manager().is_some()
}

/// The host started this process as its child (`--stdin-loop` / `MAKEPAD_STDIN_LOOP`): the
/// child follows its parent's lifetime and the host serves its tools.
pub fn launched_by_host() -> bool {
    std::env::args().any(|v| v == "--stdin-loop")
        || std::env::var("MAKEPAD_STDIN_LOOP").is_ok_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

static WINDOW_MANAGER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `app_main!` records [`AppMain::window_manager`] here before anything opens.
pub fn set_window_manager(window_manager: bool) {
    WINDOW_MANAGER.store(window_manager, std::sync::atomic::Ordering::Release);
}

/// This process is a window manager: it never becomes a client of another one.
pub fn is_window_manager() -> bool {
    WINDOW_MANAGER.load(std::sync::atomic::Ordering::Acquire)
}

/// Where a running makepad window manager admitted this process (Linux, native builds).
#[derive(Clone, Copy, Debug)]
pub struct WmAdmission {
    pub port: u16,
    pub client: u64,
    pub crate_name: &'static str,
}

#[cfg(all(target_os = "linux", not(relax)))]
static ADMITTED: std::sync::OnceLock<Option<WmAdmission>> = std::sync::OnceLock::new();

pub fn admitted_by_window_manager() -> Option<WmAdmission> {
    #[cfg(all(target_os = "linux", not(relax)))]
    return ADMITTED.get().copied().flatten();
    #[cfg(not(all(target_os = "linux", not(relax))))]
    None
}

/// Linux: a UI process looks for a makepad window manager before any display server. Unless
/// the launch forced a backend (`--linux-backend=`, a backend token in `MAKEPAD`), its host
/// launched it, it runs hidden or headless, or it is a window manager itself, a live WM that
/// speaks this build's hosted wire adopts it: its windows become WM windows over the same
/// hosted path as the apps the WM launches. Otherwise the HAL picks Wayland, then X11.
/// Under Relax the HAL shell does this for the whole `makepad run` process.
pub fn discover_window_manager(crate_name: &'static str) {
    #[cfg(all(target_os = "linux", not(relax)))]
    ADMITTED.get_or_init(|| {
        use makepad_hal::wm_socket::{self, Backend};
        if launched_by_host() || is_window_manager() || resolve_studio_host_from_env().is_some()
            || std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some()
            || std::env::var("MAKEPAD_HAL_BACKEND").is_ok_and(|b| !b.is_empty())
        {
            return None;
        }
        let forced = wm_socket::forced(std::env::args(), std::env::var("MAKEPAD").ok().as_deref());
        let wire = makepad_studio_protocol::WIRE;
        let (backend, reason) = wm_socket::choose(forced, false, false, || wm_socket::discovered(wire).clone());
        if backend != Backend::Wm {
            return None;
        }
        let path = wm_socket::discovery_path()?;
        match wm_socket::adopt(&path, wire, std::process::id(), crate_name) {
            Ok(admission) => {
                crate::log!("makepad WM at {} hosts this app as client {} ({reason})", path.display(), admission.client);
                Some(WmAdmission { port: admission.port, client: admission.client, crate_name })
            }
            Err(probe) => {
                crate::error!("the makepad WM at {} did not adopt this app: {probe:?}", path.display());
                None
            }
        }
    });
    #[cfg(not(all(target_os = "linux", not(relax))))]
    let _ = crate_name;
}

fn normalize_studio_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        return String::new();
    }

    let (scheme, rest) = if let Some((scheme, rest)) = host.split_once("://") {
        (scheme, rest)
    } else {
        ("http", host)
    };
    let host_port = rest
        .split_once(['/', '?', '#'])
        .map(|(host_port, _)| host_port)
        .unwrap_or(rest)
        .trim();
    if host_port.is_empty() {
        return String::new();
    }
    format!("{scheme}://{host_port}")
}

fn studio_query_value(studio: &str, key: &str) -> Option<String> {
    let query = studio.split_once('?')?.1;
    for pair in query.split('&') {
        let (pair_key, pair_value) = pair.split_once('=').unwrap_or((pair, ""));
        if pair_key == key {
            let pair_value = pair_value.trim();
            if !pair_value.is_empty() {
                return Some(pair_value.to_string());
            }
        }
    }
    None
}

pub(crate) fn extract_studio_build_id(studio: &str) -> Option<String> {
    studio_query_value(studio, "build").or_else(|| {
        let studio = studio.trim().trim_end_matches('/');
        if studio.is_empty() {
            return None;
        }

        let without_scheme = studio
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(studio);
        let path = without_scheme
            .split_once('/')
            .map(|(_, path)| path)
            .unwrap_or("");

        if let Some(rest) = path.strip_prefix("app/") {
            let build_id = rest.split('/').next()?;
            if !build_id.is_empty() {
                return Some(build_id.to_string());
            }
        }

        None
    })
}

pub(crate) fn extract_studio_crate_name(studio: &str) -> Option<String> {
    studio_query_value(studio, "crate")
}

fn resolve_studio_host_from_env() -> Option<String> {
    std::env::var("STUDIO_HOST")
        .ok()
        .or_else(|| std::env::var("STUDIO").ok())
        .map(|studio| normalize_studio_host(&studio))
        .filter(|studio_host| !studio_host.is_empty())
}

pub(crate) fn resolve_studio_host() -> String {
    if let Some(wm) = admitted_by_window_manager() {
        return normalize_studio_host(&format!("127.0.0.1:{}", wm.port));
    }
    resolve_studio_host_from_env().unwrap_or_default()
}

pub(crate) fn resolve_studio_build() -> Option<String> {
    if let Some(wm) = admitted_by_window_manager() {
        return Some(wm.client.to_string());
    }
    std::env::var("STUDIO_BUILD")
        .ok()
        .and_then(|build| {
            let build = build.trim();
            (!build.is_empty()).then(|| build.to_string())
        })
        .or_else(|| {
            std::env::var("STUDIO")
                .ok()
                .and_then(|studio| extract_studio_build_id(&studio))
        })
}

pub(crate) fn resolve_studio_crate() -> Option<String> {
    if let Some(wm) = admitted_by_window_manager() {
        return Some(wm.crate_name.to_string());
    }
    std::env::var("STUDIO_CRATE")
        .ok()
        .and_then(|crate_name| {
            let crate_name = crate_name.trim();
            (!crate_name.is_empty()).then(|| crate_name.to_string())
        })
        .or_else(|| {
            std::env::var("STUDIO")
                .ok()
                .and_then(|studio| extract_studio_crate_name(&studio))
        })
}

fn build_studio_http(
    studio_host: &str,
    studio_build: Option<&str>,
    studio_crate: Option<&str>,
) -> String {
    let studio_host = normalize_studio_host(studio_host);
    if studio_host.is_empty() {
        return String::new();
    }

    let studio_build = studio_build
        .map(str::trim)
        .filter(|build| !build.is_empty());
    let studio_crate = studio_crate
        .map(str::trim)
        .filter(|crate_name| !crate_name.is_empty());

    if studio_build.is_none() && studio_crate.is_none() {
        return String::new();
    }

    let mut query = Vec::new();
    if let Some(build) = studio_build {
        query.push(format!("build={build}"));
    }
    if let Some(crate_name) = studio_crate {
        query.push(format!("crate={crate_name}"));
    }
    // The wire this child speaks: its host compares it with its own.
    query.push(format!("wire={:016x}", makepad_studio_protocol::WIRE));
    format!("{studio_host}/app?{}", query.join("&"))
}

pub fn resolve_studio_http() -> String {
    let studio_host = resolve_studio_host();
    // No STUDIO_HOST/STUDIO: no studio to connect to, nothing else to resolve.
    if studio_host.is_empty() {
        return String::new();
    }
    build_studio_http(
        &studio_host,
        resolve_studio_build().as_deref(),
        resolve_studio_crate().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_studio_build_id_handles_app_paths() {
        assert_eq!(
            extract_studio_build_id("127.0.0.1:8001/app/42"),
            Some("42".to_string())
        );
        assert_eq!(
            extract_studio_build_id("http://127.0.0.1:8001/app/77"),
            Some("77".to_string())
        );
    }

    #[test]
    fn extract_studio_build_and_crate_from_query_url() {
        let studio = "http://127.0.0.1:8001/app?build=77&crate=makepad-example-xr";
        assert_eq!(extract_studio_build_id(studio), Some("77".to_string()));
        assert_eq!(
            extract_studio_crate_name(studio),
            Some("makepad-example-xr".to_string())
        );
    }

    #[test]
    fn build_studio_http_uses_query_identity() {
        let wire = format!("wire={:016x}", makepad_studio_protocol::WIRE);
        assert_eq!(
            build_studio_http("127.0.0.1:8001", Some("77"), Some("makepad-example-xr"),),
            format!("http://127.0.0.1:8001/app?build=77&crate=makepad-example-xr&{wire}")
        );
        assert_eq!(
            build_studio_http("127.0.0.1:8001", None, Some("makepad-example-xr")),
            format!("http://127.0.0.1:8001/app?crate=makepad-example-xr&{wire}")
        );
    }
}

pub trait AppMain {
    fn script_mod(_vm: &mut ScriptVm) -> ScriptValue
    where
        Self: Sized,
    {
        panic!("AppMain::script_mod not implemented for this app")
    }

    fn after_new_from_script(_vm: &mut ScriptVm, _app: &mut Self)
    where
        Self: Sized,
    {
    }

    /// Whether the app runs as one instance per user and build. A later
    /// launch hands its command line to the running instance, which gets a
    /// `SecondInstance` [`Event::AppOpen`] and comes to the front, and
    /// exits before anything of the app exists. Asked once, before the
    /// `Cx`, so it can read the command line but not `Cx`.
    ///
    /// Launches for agents, tests and hosts (`--remote`,
    /// `MAKEPAD_HIDE_WINDOWS`, `--stdin-loop`) and `MAKEPAD_NEW_INSTANCE=1`
    /// always run on their own. Mobile OSes keep one instance themselves.
    fn single_instance() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// Whether the app is a window manager: it hosts other apps' windows, so it never looks for
    /// a makepad window manager to host its own (Linux backend discovery).
    fn window_manager() -> bool
    where
        Self: Sized,
    {
        false
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event);
    /// Live sessions (Relax): an edit needs a restart. `Err(why)` postpones it, e.g. while the
    /// user has unsaved work; it is asked again on later events (`crate::restart`).
    fn can_restart(&mut self, _cx: &mut Cx) -> Result<(), String> {
        Ok(())
    }

    /// Live sessions: the app's own state to carry across a restart (what the user is working
    /// on and looking at), as text. Window frames are kept by the platform.
    fn restart_state(&mut self, _cx: &mut Cx) -> Option<String> {
        None
    }

    /// Live sessions: the state `restart_state` returned in the previous run, after `Startup`.
    fn restore_state(&mut self, _cx: &mut Cx, _state: &str) {}

    fn ui_runner(&self) -> UiRunner<Self>
    where
        Self: Sized + 'static,
    {
        // This assumes there is only one `AppMain`, and that `0` is reserved for it.
        UiRunner::new(0)
    }
}

/// Internal helper for [`app_main!`]. Emits the boxed event-handler
/// closure that every platform entry point hands to `Cx::new`. Captures
/// its own `app` and `app_value` slots, so the three
/// platform entry points (desktop, Android activity, wasm32) all
/// share a single source of truth for `Event::Startup` / `LiveEdit` /
/// `ScriptReapply` / `handle_event` dispatch.
///
/// `cx.start_hot_reload_file_observer_if_requested()` is called
/// unconditionally — its body is `#[cfg]`-gated to desktop OSes, so on
/// android / wasm32 it's a compile-time no-op.
///
/// Not part of the public API. Use [`app_main!`] instead.
#[doc(hidden)]
#[macro_export]
macro_rules! _app_main_event_closure {
    ($app:ident) => {
        $crate::_app_main_event_closure!($app, |_cx: &mut Cx| {})
    };
    ($app:ident, $configure:expr) => {{
        // This app's build inputs (package dir, icons), read in the app
        // crate so they never rebuild platform; before `init_cx_os`.
        $crate::app_meta::set_app_build_meta($crate::_app_build_meta!());
        // Event dispatch already excludes synchronous re-entry. Plain captured slots
        // also survive a caught wasm panic=abort trap; a RefCell borrow flag would not
        // be released because wasm does not unwind the trapped Rust stack.
        let mut app: Option<$app> = None;
        let mut app_value: Option<$crate::ScriptObjectRef> = None;
        let mut configure = $crate::app_main_configure($configure);
        Box::new(move |cx: &mut Cx, event: &Event| {
            if let Event::Startup = event {
                // The binary's say before anything of the app runs: what
                // this build is, as globals on `Cx` the app's `script_mod`
                // and startup read (`app_main!`'s `configure:` clause).
                configure(cx);
                // Font resource registration happens inside AppMain::script_mod,
                // so freeze the app_main! choice before that code can run.
                cx.freeze_font_set();
                $crate::startup_trace("Startup: script_mod begin");
                app = Some(cx.with_vm(|vm| {
                    let value = <$app as AppMain>::script_mod(vm);
                    $crate::startup_trace("Startup: script_mod eval done");
                    if let Some(obj) = value.as_object() {
                        app_value = Some(vm.heap_mut().new_object_ref(obj));
                    }
                    let mut app = <$app as $crate::ScriptNew>::script_from_value(vm, value);
                    $crate::startup_trace("Startup: app from_value done");
                    <$app as AppMain>::after_new_from_script(vm, &mut app);
                    app
                }));
                $crate::startup_trace("Startup: handler done");
                cx.start_hot_reload_file_observer_if_requested();
            }
            if let Event::LiveEdit = event {
                let style_reload = std::mem::take(&mut cx.pending_style_reload);
                if let Some(app) = app.as_mut() {
                    // Style reapply keeps ScriptReapply. Otherwise Reload when
                    // the DSL changed on disk, Rebake when `script_mod` only
                    // re-ran to pick up new heap primitives.
                    let live_edit_apply = if style_reload {
                        $crate::Apply::ScriptReapply
                    } else {
                        cx.live_edit_apply()
                    };
                    cx.live_edit_capture_begin();
                    cx.with_vm(|vm| {
                        let value = vm.with_reload(|vm| <$app as AppMain>::script_mod(vm));
                        if let Some(obj) = value.as_object() {
                            app_value = Some(vm.heap_mut().new_object_ref(obj));
                        }
                        <$app as $crate::ScriptApply>::script_apply(
                            app,
                            vm,
                            &live_edit_apply,
                            &mut $crate::Scope::empty(),
                            value,
                        );
                        // A module run leaves the whole previous module tree
                        // behind it, and the collector is paced by growth, so
                        // one rebuild is never enough to make it run: the
                        // garbage sat until the heap had doubled. A rebuild is
                        // the one allocation spike whose size is known in
                        // advance; collect on the way out, now that the app
                        // value is held and the tree has been applied.
                        vm.gc();
                    });
                    cx.live_edit_capture_end();
                }
            }
            if let Event::ScriptReapply = event {
                if let Some(app) = app.as_mut() {
                    let value = app_value
                        .as_ref()
                        .map(|r| $crate::ScriptValue::from(r.as_object()));
                    if let Some(value) = value {
                        cx.with_vm(|vm| {
                            <$app as $crate::ScriptApply>::script_apply(
                                app,
                                vm,
                                &$crate::Apply::ScriptReapply,
                                &mut $crate::Scope::empty(),
                                value,
                            );
                        });
                    }
                }
            }
            if let Some(app) = app.as_mut() {
                <dyn AppMain>::handle_event(app, cx, event);
                $crate::restart::after_event(app, cx, event);
            }
        }) as Box<dyn FnMut(&mut Cx, &Event)>
    }};
}

/// Construct the application context with its manifest-backed font choice
/// already immutable. Every platform entry point generated by `app_main!`
/// goes through this function before it can dispatch `Event::Startup` and
/// therefore before `AppMain::script_mod` can run.
#[doc(hidden)]
///
/// `web_font_set` is the set the app's web build selects: a collect run
/// (`MAKEPAD_RUN=collect-web`, see [`crate::collect`]) lays text out in
/// it, and records every shader from here on, before any script module.
pub fn new_cx_with_font_set(
    event_handler: Box<dyn FnMut(&mut Cx, &Event)>,
    font_set: FontSet,
    web_font_set: FontSet,
) -> Cx {
    let mut cx = Cx::new(event_handler);
    cx.init_collect_from_env();
    #[cfg(not(target_arch = "wasm32"))]
    cx.init_splash_blank_from_env();
    let font_set = if cx.is_collecting() { web_font_set } else { font_set };
    assert!(cx.set_font_set(font_set));
    cx.freeze_font_set();
    cx
}

/// The shader pack a web build was linked with: `--cfg makepad_shader_pack`
/// and `MAKEPAD_SHADER_PACK=<file>` (what `cargo makepad wasm --pack`
/// sets) embed the file; otherwise there is none. Expanded in the app's
/// crate, so a new pack rebuilds only the app.
#[cfg(makepad_shader_pack)]
#[doc(hidden)]
#[macro_export]
macro_rules! embedded_shader_pack {
    () => {
        Some(&include_bytes!(env!("MAKEPAD_SHADER_PACK"))[..])
    };
}

#[cfg(not(makepad_shader_pack))]
#[doc(hidden)]
#[macro_export]
macro_rules! embedded_shader_pack {
    () => {
        None::<&'static [u8]>
    };
}

/// Installs the embedded shader pack (see [`embedded_shader_pack`]) before
/// the first shader is applied.
#[doc(hidden)]
pub fn install_embedded_shader_pack(cx: &mut Cx, pack: Option<&'static [u8]>) {
    if let Some(pack) = pack {
        match cx.install_shader_pack(pack) {
            Ok(n) => crate::log!("shader pack: {n} shaders"),
            Err(e) => crate::error!("shader pack refused: {e}"),
        }
    }
}

/// Pins the type of an `app_main!` `configure:` closure so a bare
/// `|cx| {...}` infers `&mut Cx` without an annotation.
#[doc(hidden)]
pub fn app_main_configure<F: FnMut(&mut Cx)>(f: F) -> F {
    f
}

/// The application entry point, for every platform at once.
///
/// `configure: |cx| {...}` is the binary's say before anything of the app
/// runs — it is called once, at `Event::Startup`, before
/// `AppMain::script_mod`. A binary that links an app LIBRARY uses it to
/// hand the app its build (`cx.set_global(...)`), which the library's
/// `script_mod` reads through `vm.cx_mut()` and its startup through `cx`:
/// one mechanism on desktop, Android and the web, and no statics.
#[macro_export]
macro_rules! app_main {
    ( $app:ident ) => {
        $crate::app_main!($app, configure: |_cx: &mut Cx| {});
    };
    ( $app:ident, configure: $configure:expr ) => {
        #[cfg(target_arch = "wasm32")]
        $crate::app_main!(@impl $app, $crate::FontSet::Latin, $crate::FontSet::Latin, $crate::LATIN_FONT_ASSET_PACKAGE_MANIFEST, [], $configure);
        #[cfg(not(target_arch = "wasm32"))]
        $crate::app_main!(@impl $app, $crate::FontSet::International, $crate::FontSet::Latin, $crate::INTERNATIONAL_FONT_ASSET_MANIFEST, [], $configure);
    };
    ( $app:ident, font_assets: [$($asset:expr),* $(,)?] ) => {
        $crate::app_main!($app, font_assets: [$($asset),*], configure: |_cx: &mut Cx| {});
    };
    ( $app:ident, font_assets: [$($asset:expr),* $(,)?], configure: $configure:expr ) => {
        #[cfg(target_arch = "wasm32")]
        $crate::app_main!(@impl $app, $crate::FontSet::Latin, $crate::FontSet::Latin, $crate::LATIN_FONT_ASSET_PACKAGE_MANIFEST, [$($asset),*], $configure);
        #[cfg(not(target_arch = "wasm32"))]
        $crate::app_main!(@impl $app, $crate::FontSet::International, $crate::FontSet::Latin, $crate::INTERNATIONAL_FONT_ASSET_MANIFEST, [$($asset),*], $configure);
    };
    ( $app:ident, font_set: Latin ) => {
        $crate::app_main!($app, font_set: Latin, configure: |_cx: &mut Cx| {});
    };
    ( $app:ident, font_set: Latin, configure: $configure:expr ) => {
        $crate::app_main!(@impl $app, $crate::FontSet::Latin, $crate::FontSet::Latin, $crate::LATIN_FONT_ASSET_PACKAGE_MANIFEST, [], $configure);
    };
    ( $app:ident, font_set: International ) => {
        $crate::app_main!($app, font_set: International, configure: |_cx: &mut Cx| {});
    };
    ( $app:ident, font_set: International, configure: $configure:expr ) => {
        $crate::app_main!(@impl $app, $crate::FontSet::International, $crate::FontSet::International, $crate::INTERNATIONAL_FONT_ASSET_MANIFEST, [], $configure);
    };
    ( $app:ident, font_set: Latin, font_assets: [$($asset:expr),* $(,)?] ) => {
        $crate::app_main!($app, font_set: Latin, font_assets: [$($asset),*], configure: |_cx: &mut Cx| {});
    };
    ( $app:ident, font_set: Latin, font_assets: [$($asset:expr),* $(,)?], configure: $configure:expr ) => {
        $crate::app_main!(@impl $app, $crate::FontSet::Latin, $crate::FontSet::Latin, $crate::LATIN_FONT_ASSET_PACKAGE_MANIFEST, [$($asset),*], $configure);
    };
    ( $app:ident, font_set: International, font_assets: [$($asset:expr),* $(,)?] ) => {
        $crate::app_main!($app, font_set: International, font_assets: [$($asset),*], configure: |_cx: &mut Cx| {});
    };
    ( $app:ident, font_set: International, font_assets: [$($asset:expr),* $(,)?], configure: $configure:expr ) => {
        $crate::app_main!(@impl $app, $crate::FontSet::International, $crate::FontSet::International, $crate::INTERNATIONAL_FONT_ASSET_MANIFEST, [$($asset),*], $configure);
    };
    (@impl $app:ident, $font_set:expr, $web_font_set:expr, $manifest:expr, [$($asset:expr),*], $configure:expr) => {
        // The payload is line-oriented UTF-8. Lane B reads this section from
        // freshly linked wasm before any optional custom-section stripping.
        const MAKEPAD_EXTRA_FONT_ASSETS: &[&str] = &[$($asset),*];
        #[used]
        #[cfg_attr(target_arch = "wasm32", link_section = "makepad.font-assets.v1")]
        #[cfg_attr(
            all(not(target_arch = "wasm32"), target_vendor = "apple"),
            link_section = "__DATA,__mp_font_v1"
        )]
        #[cfg_attr(
            all(not(target_arch = "wasm32"), not(target_vendor = "apple")),
            link_section = ".makepad.font-assets.v1"
        )]
        static MAKEPAD_FONT_ASSETS_V1: [u8; $crate::font_asset_manifest_len(
            $manifest,
            MAKEPAD_EXTRA_FONT_ASSETS,
        )] = $crate::extend_font_asset_manifest($manifest, MAKEPAD_EXTRA_FONT_ASSETS);

        // The executable's own Info.plist (see app_meta.rs): the application
        // menu's title of an unbundled launch. Read here, in the app crate,
        // a bundle name rebuilds only the app.
        #[cfg(target_os = "macos")]
        const MAKEPAD_BUNDLE_NAME: &str = match option_env!("MAKEPAD_BUNDLE_NAME") {
            Some(name) => name,
            None => $crate::app_meta::DEFAULT_BUNDLE_NAME,
        };
        #[cfg(target_os = "macos")]
        const MAKEPAD_BUNDLE_IDENTIFIER: Option<&str> = option_env!("MAKEPAD_BUNDLE_IDENTIFIER");
        #[cfg(target_os = "macos")]
        #[used]
        #[link_section = "__TEXT,__info_plist"]
        static MAKEPAD_INFO_PLIST: [u8; $crate::app_meta::info_plist_len(
            MAKEPAD_BUNDLE_NAME,
            MAKEPAD_BUNDLE_IDENTIFIER,
        )] = $crate::app_meta::info_plist(MAKEPAD_BUNDLE_NAME, MAKEPAD_BUNDLE_IDENTIFIER);

        fn main() {
            $crate::run_app(app_main);
        }

        // A standalone Android app's entry (decided in this crate, which knows the `relax` cfg).
        $crate::android_jni_entry!(app_main);

        #[cfg(not(target_arch = "wasm32"))]
        pub fn app_main() {
            $crate::attach_parent_console();
            Cx::init_log();
            $crate::startup_trace("main-entered (dyld done)");
            // A later launch of a single-instance app hands its command line
            // to the running instance and ends here, before the VM or a window.
            let single_instance = if <$app as AppMain>::single_instance() {
                match $crate::app_open::claim_single_instance(env!("CARGO_PKG_NAME")) {
                    $crate::app_open::SingleInstance::Forwarded => return,
                    $crate::app_open::SingleInstance::Primary(server) => Some(server),
                    $crate::app_open::SingleInstance::Standalone => None,
                }
            } else {
                None
            };
            if Cx::pre_start() {
                return;
            }
            $crate::set_window_manager(<$app as AppMain>::window_manager());
            $crate::discover_window_manager(env!("CARGO_PKG_NAME"));
            $crate::startup_trace("pre_start (objc classes)");

            // The event-handler closure (which captures `app` and
            // `app_value` Rcs internally) is shared across all four
            // platform entry points via `_app_main_event_closure!`.
            let cx = std::rc::Rc::new(std::cell::RefCell::new(
                $crate::new_cx_with_font_set(
                $crate::_app_main_event_closure!($app, $configure),
                $font_set,
                $web_font_set,
            )));
            $crate::startup_trace("Cx::new (vm + std script)");
            let studio_http = $crate::resolve_studio_http();
            cx.borrow_mut().init_websockets(&studio_http);
            if $crate::should_run_stdin_loop_from_env() {
                cx.borrow_mut().in_makepad_studio = true;
                // A hosted child must never outlive its host, whatever its
                // event loop happens to be busy with.
                $crate::memory_watchdog::start_stdin_orphan_watchdog();
            }
            //cx.borrow_mut().init_websockets("");
            cx.borrow_mut().init_cx_os();
            $crate::startup_trace("init_cx_os (deps loaded)");
            // The command line's files and URLs, then later launches': each
            // an `Event::AppOpen` after `Event::Startup`.
            $crate::app_open::post_launch_args();
            if let Some(server) = single_instance {
                server.serve();
            }
            Cx::event_loop(cx);
        }

        #[cfg(target_arch = "wasm32")]
        pub fn app_main() {}

        #[export_name = "wasm_create_app"]
        #[cfg(target_arch = "wasm32")]
        pub extern "C" fn create_wasm_app() -> u32 {
            Cx::init_log();
            let mut cx = Box::new($crate::new_cx_with_font_set(
                $crate::_app_main_event_closure!($app, $configure),
                $font_set,
                $web_font_set,
            ));
            $crate::install_embedded_shader_pack(&mut cx, $crate::embedded_shader_pack!());
            let studio_http = $crate::resolve_studio_http();
            cx.init_websockets(&studio_http);
            cx.init_cx_os();
            Box::into_raw(cx) as u32
        }

        #[export_name = "wasm_process_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_process_msg(msg_ptr: u32, cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx);
            cx.process_to_wasm(msg_ptr)
        }

        #[export_name = "wasm_return_first_msg"]
        #[cfg(target_arch = "wasm32")]
        pub unsafe extern "C" fn wasm_return_first_msg(cx_ptr: u32) -> u32 {
            let cx = &mut *(cx_ptr as *mut Cx);
            cx.os.from_wasm.take().unwrap().release_ownership()
        }
    };
}

#[cfg(test)]
mod font_set_macro_compile_test {
    #![allow(dead_code, unused_mut)]

    use crate::*;

    #[derive(Script, ScriptHook)]
    struct ExplicitFontApp {}

    impl AppMain for ExplicitFontApp {
        fn script_mod(_vm: &mut ScriptVm) -> ScriptValue {
            NIL
        }

        fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) {}
    }

    app_main!(
        ExplicitFontApp,
        font_set: International,
        font_assets: [
            "example/resources/Custom.ttf",
            "makepad_widgets/resources/NotoColorEmoji.ttf",
        ]
    );

    #[test]
    fn explicit_font_set_macro_form_compiles_with_international_manifest() {
        const EXTRAS: &[&str] = &[
            "example/resources/Custom.ttf",
            "makepad_widgets/resources/NotoColorEmoji.ttf",
        ];
        const N: usize = font_asset_manifest_len(INTERNATIONAL_FONT_ASSET_MANIFEST, EXTRAS);
        const EXPECTED: [u8; N] =
            extend_font_asset_manifest(INTERNATIONAL_FONT_ASSET_MANIFEST, EXTRAS);
        assert_eq!(MAKEPAD_FONT_ASSETS_V1, EXPECTED);
        let manifest = std::str::from_utf8(&MAKEPAD_FONT_ASSETS_V1).unwrap();
        assert_eq!(manifest.matches("NotoColorEmoji.ttf").count(), 1);
        assert!(manifest.contains("example/resources/Custom.ttf"));
        // Optional widget fonts only ship when declared.
        assert!(!manifest.contains("NewCMMath-Regular.otf"));
        assert!(!manifest.contains("Inter.ttf"));
        assert!(!manifest.contains("RobotoFlex.ttf"));
    }

    #[test]
    fn every_entry_point_uses_a_pre_frozen_font_selection() {
        for set in [FontSet::Latin, FontSet::International] {
            let cx = new_cx_with_font_set(Box::new(|_, _| {}), set, set);
            assert_eq!(cx.font_set(), set);
            assert!(cx.is_font_set_frozen());
        }

        // Every Cx an app_main! entry makes goes through new_cx_with_font_set: the macro makes
        // one, and never through a bare Cx::new (whose font set is not frozen yet).
        let source = include_str!("app_main.rs");
        let start = source.find(["macro_rules! ", "app_main"].concat().as_str()).unwrap();
        let end = start + source[start..].find(["mod font_set_", "macro_compile_test"].concat().as_str()).unwrap();
        let entries = &source[start..end];
        assert!(entries.contains(["$crate::new_cx", "_with_font_set("].concat().as_str()));
        assert!(!entries.contains(["Cx::", "new("].concat().as_str()));
    }
}
