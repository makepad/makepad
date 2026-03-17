use crate::{ffi, BootstrapResult, Error, Frame, Result, TEXT_INPUT_MODE_NONE};
use libloading::Library;
use makepad_objc_sys::declare::{ClassDecl, MethodImplementation};
use makepad_objc_sys::runtime::{
    self, Class, ObjcId, Object, Protocol, Sel, BOOL, NO, YES,
};
use makepad_objc_sys::{class, msg_send, sel, sel_impl};
use makepad_objc_sys::{Encode, EncodeArguments, Encoding};
use std::env;
use std::ffi::{c_char, c_void, CString};
use std::os::raw::c_int;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::symlink;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

extern "C" {
    static NSRunLoopCommonModes: ObjcId;
}

const CEF_API_VERSION: c_int = parse_api_version(env!("MAKEPAD_CEF_API_VERSION"));
const HELPER_APP_SUFFIXES: [(&str, &str); 5] = [
    ("", ""),
    (" (Alerts)", ".alerts"),
    (" (GPU)", ".gpu"),
    (" (Plugin)", ".plugin"),
    (" (Renderer)", ".renderer"),
];
const PKGINFO_CONTENTS: &str = "APPL????";
const RTLD_FIRST: c_int = 0x100;
const EXTERNAL_PUMP_TIMER_PLACEHOLDER: i64 = i32::MAX as i64;
const EXTERNAL_PUMP_MAX_DELAY_MS: i64 = 1000 / 30;

const fn parse_api_version(value: &str) -> c_int {
    let bytes = value.as_bytes();
    let mut out = 0_i32;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte >= b'0' && byte <= b'9' {
            out = out * 10 + (byte - b'0') as i32;
        }
        index += 1;
    }
    out
}

struct CefApi {
    cef_api_hash: unsafe extern "C" fn(version: c_int, entry: c_int) -> *const c_char,
    cef_execute_process: unsafe extern "C" fn(
        args: *const ffi::cef_main_args_t,
        application: *mut ffi::cef_app_t,
        windows_sandbox_info: *mut c_void,
    ) -> c_int,
    cef_initialize: unsafe extern "C" fn(
        args: *const ffi::cef_main_args_t,
        settings: *const ffi::cef_settings_t,
        application: *mut ffi::cef_app_t,
        windows_sandbox_info: *mut c_void,
    ) -> c_int,
    cef_get_exit_code: unsafe extern "C" fn() -> c_int,
    cef_shutdown: unsafe extern "C" fn(),
    cef_do_message_loop_work: unsafe extern "C" fn(),
    cef_browser_host_create_browser_sync: unsafe extern "C" fn(
        window_info: *const ffi::cef_window_info_t,
        client: *mut ffi::cef_client_t,
        url: *const ffi::cef_string_t,
        settings: *const ffi::cef_browser_settings_t,
        extra_info: *mut ffi::cef_dictionary_value_t,
        request_context: *mut ffi::cef_request_context_t,
    ) -> *mut ffi::cef_browser_t,
    cef_string_utf8_to_utf16: unsafe extern "C" fn(
        src: *const c_char,
        src_len: usize,
        output: *mut ffi::cef_string_t,
    ) -> c_int,
    cef_string_utf16_clear: unsafe extern "C" fn(str_: *mut ffi::cef_string_t),
}

struct RuntimePaths {
    framework_bin: PathBuf,
    framework_dir: PathBuf,
    resources_dir: PathBuf,
}

struct Runtime {
    _library: Library,
    api: CefApi,
    paths: RuntimePaths,
    state: Mutex<RuntimeState>,
}

#[derive(Default)]
struct RuntimeState {
    initialized: bool,
    shutting_down: bool,
    app: usize,
}

struct SyntheticAppBundle {
    bundle_dir: PathBuf,
    bundle_executable: PathBuf,
    framework_dir: PathBuf,
    resources_dir: PathBuf,
    helper_executable: PathBuf,
    log_file: PathBuf,
}

struct MainArgsStorage {
    _args: Vec<CString>,
    _ptrs: Vec<*mut c_char>,
    main_args: ffi::cef_main_args_t,
}

struct CefString {
    value: ffi::cef_string_t,
    clear: unsafe extern "C" fn(*mut ffi::cef_string_t),
}

#[derive(Default)]
struct SharedBrowserState {
    view: Mutex<ViewState>,
    frame: Mutex<Option<Frame>>,
    closing: AtomicBool,
    editable_focus: AtomicBool,
}

#[derive(Clone, Copy)]
struct ViewState {
    width: usize,
    height: usize,
    scale_factor: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            width: 1,
            height: 1,
            scale_factor: 1.0,
        }
    }
}

#[repr(C)]
struct RenderHandler {
    cef_render_handler: ffi::cef_render_handler_t,
    ref_count: AtomicUsize,
    state: Arc<SharedBrowserState>,
}

#[repr(C)]
struct ClientHandler {
    cef_client: ffi::cef_client_t,
    ref_count: AtomicUsize,
    render_handler: *mut RenderHandler,
}

#[repr(C)]
struct BrowserProcessHandler {
    cef_browser_process_handler: ffi::cef_browser_process_handler_t,
    ref_count: AtomicUsize,
}

#[repr(C)]
struct AppHandler {
    cef_app: ffi::cef_app_t,
    ref_count: AtomicUsize,
    browser_process_handler: *mut BrowserProcessHandler,
}

struct ExternalPump {
    owner_thread: usize,
    handler: usize,
    timer: Mutex<usize>,
    is_active: AtomicBool,
    reentrancy_detected: AtomicBool,
}

pub struct Browser {
    browser: *mut ffi::cef_browser_t,
    state: Arc<SharedBrowserState>,
    width: usize,
    height: usize,
    scale_factor: f32,
}

unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T> {
    let symbol = library.get::<T>(name).map_err(|err| {
        Error::new(format!(
            "failed to load {}: {err}",
            String::from_utf8_lossy(name)
        ))
    })?;
    Ok(*symbol)
}

fn framework_dir_for_bundle_executable(executable: &Path) -> Option<PathBuf> {
    let macos_dir = executable.parent()?;
    let contents_dir = macos_dir.parent()?;
    let bundle_dir = contents_dir.parent()?;
    if macos_dir.file_name().and_then(|value| value.to_str()) != Some("MacOS")
        || contents_dir.file_name().and_then(|value| value.to_str()) != Some("Contents")
        || bundle_dir.extension().and_then(|value| value.to_str()) != Some("app")
    {
        return None;
    }

    let bundle_parent = bundle_dir.parent()?;
    if bundle_parent.file_name().and_then(|value| value.to_str()) == Some("Frameworks") {
        Some(bundle_parent.join("Chromium Embedded Framework.framework"))
    } else {
        Some(
            contents_dir
                .join("Frameworks")
                .join("Chromium Embedded Framework.framework"),
        )
    }
}

fn distribution_runtime_paths() -> RuntimePaths {
    let framework_bin = env::var_os("MAKEPAD_CEF_FRAMEWORK_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("MAKEPAD_CEF_FRAMEWORK_BIN")));
    let framework_dir = env::var_os("MAKEPAD_CEF_FRAMEWORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("MAKEPAD_CEF_FRAMEWORK_DIR")));
    let resources_dir = env::var_os("MAKEPAD_CEF_RESOURCES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("MAKEPAD_CEF_RESOURCES_DIR")));
    RuntimePaths {
        framework_bin,
        framework_dir,
        resources_dir,
    }
}

fn runtime_paths() -> RuntimePaths {
    if let Ok(current_executable) = env::current_exe() {
        if let Some(framework_dir) = framework_dir_for_bundle_executable(&current_executable) {
            let framework_bin = framework_dir.join("Chromium Embedded Framework");
            let resources_dir = framework_dir.join("Resources");
            if framework_bin.exists() && resources_dir.exists() {
                return RuntimePaths {
                    framework_bin,
                    framework_dir,
                    resources_dir,
                };
            }
        }
    }

    distribution_runtime_paths()
}

fn helper_binary_source() -> PathBuf {
    env::var_os("MAKEPAD_CEF_HELPER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("MAKEPAD_CEF_HELPER_BIN")))
}

fn runtime() -> Result<&'static Runtime> {
    static RUNTIME: OnceLock<Result<Runtime>> = OnceLock::new();
    let runtime_result = RUNTIME.get_or_init(|| {
        let paths = runtime_paths();
        let library = unsafe {
            libloading::os::unix::Library::open(
                Some(&paths.framework_bin),
                libloading::os::unix::RTLD_LAZY
                    | libloading::os::unix::RTLD_LOCAL
                    | RTLD_FIRST,
            )
            .map(Library::from)
        }
        .map_err(|err| {
            Error::new(format!(
                "failed to load {}: {err}",
                paths.framework_bin.display()
            ))
        })?;
        let api = unsafe {
            CefApi {
                cef_api_hash: load_symbol(&library, b"cef_api_hash\0")?,
                cef_execute_process: load_symbol(&library, b"cef_execute_process\0")?,
                cef_initialize: load_symbol(&library, b"cef_initialize\0")?,
                cef_get_exit_code: load_symbol(&library, b"cef_get_exit_code\0")?,
                cef_shutdown: load_symbol(&library, b"cef_shutdown\0")?,
                cef_do_message_loop_work: load_symbol(&library, b"cef_do_message_loop_work\0")?,
                cef_browser_host_create_browser_sync: load_symbol(
                    &library,
                    b"cef_browser_host_create_browser_sync\0",
                )?,
                cef_string_utf8_to_utf16: load_symbol(&library, b"cef_string_utf8_to_utf16\0")?,
                cef_string_utf16_clear: load_symbol(&library, b"cef_string_utf16_clear\0")?,
            }
        };

        let hash = unsafe { (api.cef_api_hash)(CEF_API_VERSION, 0) };
        if hash.is_null() {
            return Err(Error::new("cef_api_hash returned a null pointer"));
        }

        Ok(Runtime {
            _library: library,
            api,
            paths,
            state: Mutex::new(RuntimeState::default()),
        })
    });
    runtime_result
        .as_ref()
        .map_err(|err| Error::new(err.to_string()))
}

fn objc_method_type_encoding(ret: &Encoding, args: &[Encoding]) -> CString {
    let mut types = ret.as_str().to_owned();
    types.push_str(<*mut Object>::encode().as_str());
    types.push_str(Sel::encode().as_str());
    types.extend(args.iter().map(|encoding| encoding.as_str()));
    CString::new(types).unwrap()
}

unsafe fn objc_add_instance_method<F>(class: *mut Class, selector: Sel, func: F) -> Result<()>
where
    F: MethodImplementation<Callee = Object>,
{
    let arg_encodings = F::Args::encodings();
    let arg_encodings = arg_encodings.as_ref();
    let expected_args = selector.name().chars().filter(|&ch| ch == ':').count();
    if expected_args != arg_encodings.len() {
        return Err(Error::new(format!(
            "Objective-C selector {} expects {} arguments but function encodes {}",
            selector.name(),
            expected_args,
            arg_encodings.len()
        )));
    }
    let types = objc_method_type_encoding(&F::Ret::encode(), arg_encodings);
    if runtime::class_addMethod(class, selector, func.imp(), types.as_ptr()) == NO {
        return Err(Error::new(format!(
            "failed to add Objective-C method {} to {}",
            selector.name(),
            (&*class).name()
        )));
    }
    Ok(())
}

fn objc_bool(value: bool) -> BOOL {
    if value {
        YES
    } else {
        NO
    }
}

extern "C" fn cef_is_handling_send_event(_this: &Object, _cmd: Sel) -> BOOL {
    objc_bool(CEF_HANDLING_SEND_EVENT.load(Ordering::Acquire))
}

extern "C" fn cef_set_handling_send_event(_this: &Object, _cmd: Sel, handling: BOOL) {
    CEF_HANDLING_SEND_EVENT.store(handling != NO, Ordering::Release);
}

extern "C" fn cef_application_send_event(this: &Object, _cmd: Sel, event: ObjcId) {
    let previous = CEF_HANDLING_SEND_EVENT.swap(true, Ordering::AcqRel);
    unsafe {
        let () = msg_send![super(this, class!(NSApplication)), sendEvent: event];
    };
    CEF_HANDLING_SEND_EVENT.store(previous, Ordering::Release);
}

fn ensure_cef_application_class() -> Result<&'static Class> {
    static APP_CLASS: OnceLock<Result<&'static Class>> = OnceLock::new();
    APP_CLASS
        .get_or_init(|| unsafe {
            if let Some(existing) = Class::get("MakepadCefApplication") {
                return Ok(existing);
            }

            let mut decl = ClassDecl::new("MakepadCefApplication", class!(NSApplication))
                .ok_or_else(|| Error::new("failed to allocate MakepadCefApplication"))?;
            if let Some(protocol) = Protocol::get("CefAppProtocol") {
                decl.add_protocol(protocol);
            }
            decl.add_method(
                sel!(isHandlingSendEvent),
                cef_is_handling_send_event as extern "C" fn(&Object, Sel) -> BOOL,
            );
            decl.add_method(
                sel!(setHandlingSendEvent:),
                cef_set_handling_send_event as extern "C" fn(&Object, Sel, BOOL),
            );
            decl.add_method(
                sel!(sendEvent:),
                cef_application_send_event as extern "C" fn(&Object, Sel, ObjcId),
            );
            Ok(decl.register())
        })
        .as_ref()
        .map(|class| *class)
        .map_err(|err| Error::new(err.to_string()))
}

fn ensure_cef_application_patch() -> Result<()> {
    static PATCH_RESULT: OnceLock<Result<()>> = OnceLock::new();
    PATCH_RESULT
        .get_or_init(|| unsafe {
            let app_class = ensure_cef_application_class()?;
            let ns_app: ObjcId = msg_send![app_class, sharedApplication];
            if ns_app.is_null() {
                return Err(Error::new("MakepadCefApplication sharedApplication returned null"));
            }

            let actual_class = runtime::object_getClass(ns_app as *const Object) as *mut Class;
            if actual_class.is_null() {
                return Err(Error::new("object_getClass(NSApp) returned null"));
            }

            let actual_class = &*actual_class;
            if actual_class.name() != app_class.name() {
                return Err(Error::new(format!(
                    "CEF requires NSApp to be {}, found {}",
                    app_class.name(),
                    actual_class.name()
                )));
            }

            Ok(())
        })
        .as_ref()
        .map(|_| ())
        .map_err(|err| Error::new(err.to_string()))
}

static CEF_HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

extern "C" fn external_pump_schedule_work(_this: &Object, _cmd: Sel, delay_ms: ObjcId) {
    let delay_ms = if delay_ms.is_null() {
        0
    } else {
        unsafe { msg_send![delay_ms, longLongValue] }
    };
    if let Ok(pump) = external_pump() {
        pump.handle_schedule_work(delay_ms);
    }
}

extern "C" fn external_pump_timer_fired(_this: &Object, _cmd: Sel, _timer: ObjcId) {
    if let Ok(pump) = external_pump() {
        pump.handle_timer_timeout();
    }
}

fn ensure_external_pump_class() -> Result<&'static Class> {
    static PUMP_CLASS: OnceLock<Result<&'static Class>> = OnceLock::new();
    PUMP_CLASS
        .get_or_init(|| unsafe {
            if let Some(existing) = Class::get("MakepadCefMessagePumpTarget") {
                return Ok(existing);
            }

            let mut decl = ClassDecl::new("MakepadCefMessagePumpTarget", class!(NSObject))
                .ok_or_else(|| Error::new("failed to allocate MakepadCefMessagePumpTarget"))?;
            decl.add_method(
                sel!(scheduleWork:),
                external_pump_schedule_work as extern "C" fn(&Object, Sel, ObjcId),
            );
            decl.add_method(
                sel!(timerFired:),
                external_pump_timer_fired as extern "C" fn(&Object, Sel, ObjcId),
            );
            Ok(decl.register())
        })
        .as_ref()
        .map(|class| *class)
        .map_err(|err| Error::new(err.to_string()))
}

fn external_pump() -> Result<&'static ExternalPump> {
    static EXTERNAL_PUMP: OnceLock<Result<ExternalPump>> = OnceLock::new();
    let result = EXTERNAL_PUMP.get_or_init(|| unsafe {
        let pump_class = ensure_external_pump_class()?;
        let handler: ObjcId = msg_send![pump_class, new];
        if handler.is_null() {
            return Err(Error::new(
                "MakepadCefMessagePumpTarget new returned null",
            ));
        }
        let owner_thread: ObjcId = msg_send![class!(NSThread), mainThread];
        if owner_thread.is_null() {
            return Err(Error::new("NSThread mainThread returned null"));
        }
        Ok(ExternalPump {
            owner_thread: owner_thread as usize,
            handler: handler as usize,
            timer: Mutex::new(0),
            is_active: AtomicBool::new(false),
            reentrancy_detected: AtomicBool::new(false),
        })
    });
    result
        .as_ref()
        .map_err(|err| Error::new(err.to_string()))
}

impl ExternalPump {
    fn owner_thread(&self) -> ObjcId {
        self.owner_thread as ObjcId
    }

    fn handler(&self) -> ObjcId {
        self.handler as ObjcId
    }

    fn schedule(&self, delay_ms: i64) {
        unsafe {
            let delay_number: ObjcId = msg_send![class!(NSNumber), numberWithLongLong: delay_ms];
            let _: () = msg_send![
                self.handler(),
                performSelector: sel!(scheduleWork:)
                onThread: self.owner_thread()
                withObject: delay_number
                waitUntilDone: NO
            ];
        }
    }

    fn is_timer_pending(&self) -> bool {
        *self.timer.lock().unwrap() != 0
    }

    fn kill_timer(&self) {
        let timer = {
            let mut timer_slot = self.timer.lock().unwrap();
            let timer = *timer_slot as ObjcId;
            *timer_slot = 0;
            timer
        };
        if !timer.is_null() {
            unsafe {
                let _: () = msg_send![timer, invalidate];
            }
        }
    }

    fn set_timer(&self, delay_ms: i64) {
        debug_assert!(delay_ms > 0);
        unsafe {
            let timer: ObjcId = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: (delay_ms as f64 / 1000.0)
                target: self.handler()
                selector: sel!(timerFired:)
                userInfo: ptr::null_mut::<Object>()
                repeats: NO
            ];
            let run_loop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let _: () = msg_send![run_loop, addTimer: timer forMode: NSRunLoopCommonModes];
            *self.timer.lock().unwrap() = timer as usize;
        }
    }

    fn handle_schedule_work(&self, mut delay_ms: i64) {
        if delay_ms == EXTERNAL_PUMP_TIMER_PLACEHOLDER && self.is_timer_pending() {
            return;
        }

        self.kill_timer();

        if delay_ms <= 0 {
            self.do_work();
            return;
        }

        if delay_ms > EXTERNAL_PUMP_MAX_DELAY_MS {
            delay_ms = EXTERNAL_PUMP_MAX_DELAY_MS;
        }
        self.set_timer(delay_ms);
    }

    fn handle_timer_timeout(&self) {
        self.kill_timer();
        self.do_work();
    }

    fn do_work(&self) {
        let was_reentrant = self.perform_message_loop_work();
        if was_reentrant {
            self.schedule(0);
        } else if !self.is_timer_pending() {
            self.schedule(EXTERNAL_PUMP_TIMER_PLACEHOLDER);
        }
    }

    fn perform_message_loop_work(&self) -> bool {
        if self.is_active.swap(true, Ordering::AcqRel) {
            self.reentrancy_detected.store(true, Ordering::Release);
            return false;
        }

        self.reentrancy_detected.store(false, Ordering::Release);
        do_message_loop_work_impl();
        self.is_active.store(false, Ordering::Release);
        self.reentrancy_detected.load(Ordering::Acquire)
    }
}

impl MainArgsStorage {
    fn current_process() -> Result<Self> {
        let args_os = env::args_os().collect::<Vec<_>>();
        let mut filtered_args = Vec::with_capacity(args_os.len());
        if let Some(program) = args_os.first() {
            filtered_args.push(program.clone());
        }

        let mut skip_next = false;
        for arg in args_os.into_iter().skip(1) {
            if skip_next {
                skip_next = false;
                continue;
            }

            let arg_str = arg.to_string_lossy();
            if arg_str == "--stdin-loop" {
                continue;
            }
            if arg_str == "--message-format" {
                skip_next = true;
                continue;
            }
            if arg_str.starts_with("--message-format=") {
                continue;
            }

            filtered_args.push(arg);
        }

        let mut args = Vec::new();
        for arg in filtered_args {
            let bytes = arg.into_vec();
            args.push(
                CString::new(bytes)
                    .map_err(|_| Error::new("failed to convert process arguments for CEF"))?,
            );
        }
        let mut ptrs = args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect::<Vec<_>>();
        let main_args = ffi::cef_main_args_t {
            argc: ptrs.len() as c_int,
            argv: ptrs.as_mut_ptr(),
        };
        Ok(Self {
            _args: args,
            _ptrs: ptrs,
            main_args,
        })
    }
}

impl CefString {
    fn new(api: &CefApi, value: impl AsRef<str>) -> Result<Self> {
        let string = value.as_ref();
        let c_string = CString::new(string)
            .map_err(|_| Error::new(format!("string contains interior null bytes: {string:?}")))?;
        let mut out = ffi::cef_string_t::default();
        let ok = unsafe {
            (api.cef_string_utf8_to_utf16)(c_string.as_ptr(), c_string.as_bytes().len(), &mut out)
        };
        if ok == 0 {
            return Err(Error::new(format!(
                "CEF failed to convert string: {string}"
            )));
        }
        Ok(Self {
            value: out,
            clear: api.cef_string_utf16_clear,
        })
    }

    fn raw(&self) -> ffi::cef_string_t {
        self.value
    }
}

impl Drop for CefString {
    fn drop(&mut self) {
        unsafe {
            (self.clear)(&mut self.value);
        }
    }
}

fn ensure_initialized() -> Result<()> {
    let runtime = runtime()?;
    let mut state = runtime.state.lock().unwrap();
    if state.initialized {
        return Ok(());
    }
    if state.shutting_down {
        return Err(Error::new("CEF is shutting down"));
    }

    ensure_cef_application_patch()?;
    external_pump()?;
    env::set_var("MallocNanoZone", "0");

    let args = MainArgsStorage::current_process()?;
    let current_exe = env::current_exe()
        .map_err(|err| Error::new(format!("failed to resolve current executable: {err}")))?;
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);
    let synthetic_bundle = ensure_synthetic_app_bundle(&distribution_runtime_paths(), &current_exe)?;
    let helper_executable = synthetic_bundle.helper_executable.to_string_lossy().to_string();
    let log_file = synthetic_bundle.log_file.to_string_lossy().to_string();
    let root_cache_path = temp_root_cache_path()?;

    let browser_subprocess_path = CefString::new(&runtime.api, helper_executable)?;
    let log_file = CefString::new(&runtime.api, log_file)?;
    let root_cache_path = CefString::new(&runtime.api, root_cache_path.to_string_lossy())?;

    let mut settings = ffi::cef_settings_t {
        size: std::mem::size_of::<ffi::cef_settings_t>(),
        no_sandbox: 1,
        browser_subprocess_path: browser_subprocess_path.raw(),
        framework_dir_path: ffi::cef_string_t::default(),
        main_bundle_path: ffi::cef_string_t::default(),
        multi_threaded_message_loop: 0,
        external_message_pump: 1,
        windowless_rendering_enabled: 1,
        command_line_args_disabled: 0,
        cache_path: ffi::cef_string_t::default(),
        root_cache_path: root_cache_path.raw(),
        persist_session_cookies: 0,
        user_agent: ffi::cef_string_t::default(),
        user_agent_product: ffi::cef_string_t::default(),
        locale: ffi::cef_string_t::default(),
        log_file: log_file.raw(),
        log_severity: ffi::LOGSEVERITY_INFO,
        log_items: ffi::LOG_ITEMS_DEFAULT,
        javascript_flags: ffi::cef_string_t::default(),
        resources_dir_path: ffi::cef_string_t::default(),
        locales_dir_path: ffi::cef_string_t::default(),
        ..Default::default()
    };

    let app = AppHandler::allocate();

    let initialized = unsafe {
        (runtime.api.cef_initialize)(
            &args.main_args,
            &mut settings,
            &mut (*app).cef_app,
            ptr::null_mut(),
        )
    };
    if initialized == 0 {
        let exit_code = unsafe { (runtime.api.cef_get_exit_code)() };
        unsafe {
            release_ref_counted(&mut (*app).cef_app.base as *mut _);
        }
        return Err(Error::new(format!(
            "cef_initialize returned false with exit code {exit_code}"
        )));
    }

    state.app = app as usize;
    state.initialized = true;
    if let Ok(pump) = external_pump() {
        pump.schedule(0);
    }
    Ok(())
}

fn temp_root_cache_path() -> Result<PathBuf> {
    let exe = env::current_exe()
        .map_err(|err| Error::new(format!("failed to resolve current executable: {err}")))?;
    let stem = exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("makepad-cef");
    let path = env::temp_dir()
        .join("makepad-cef")
        .join(stem)
        .join(format!("pid-{}", std::process::id()));
    std::fs::create_dir_all(&path)
        .map_err(|err| Error::new(format!("failed to create {}: {err}", path.display())))?;
    Ok(path)
}

fn ensure_symlink(source: &PathBuf, destination: &PathBuf) -> Result<()> {
    if source == destination {
        return Ok(());
    }
    if let (Ok(source_real), Ok(destination_real)) =
        (source.canonicalize(), destination.canonicalize())
    {
        if source_real == destination_real {
            return Ok(());
        }
    }

    if let Ok(existing) = std::fs::read_link(destination) {
        if existing == *source {
            return Ok(());
        }
    }

    match std::fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                std::fs::remove_dir_all(destination).map_err(|err| {
                    Error::new(format!(
                        "failed to remove stale directory {}: {err}",
                        destination.display()
                    ))
                })?;
            } else {
                std::fs::remove_file(destination).map_err(|err| {
                    Error::new(format!(
                        "failed to remove stale path {}: {err}",
                        destination.display()
                    ))
                })?;
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(Error::new(format!(
                "failed to inspect {}: {err}",
                destination.display()
            )));
        }
    }

    symlink(source, destination).map_err(|err| {
        Error::new(format!(
            "failed to create symlink {} -> {}: {err}",
            destination.display(),
            source.display()
        ))
    })
}

fn write_if_changed(path: &PathBuf, content: &str) -> Result<()> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == content => return Ok(()),
        Ok(_) | Err(_) => {}
    }
    std::fs::write(path, content)
        .map_err(|err| Error::new(format!("failed to write {}: {err}", path.display())))
}

fn copy_executable(source: &PathBuf, destination: &PathBuf) -> Result<()> {
    if source == destination {
        return Ok(());
    }
    if let (Ok(source_real), Ok(destination_real)) =
        (source.canonicalize(), destination.canonicalize())
    {
        if source_real == destination_real {
            return Ok(());
        }
    }

    match std::fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                std::fs::remove_dir_all(destination).map_err(|err| {
                    Error::new(format!(
                        "failed to remove stale directory {}: {err}",
                        destination.display()
                    ))
                })?;
            } else {
                std::fs::remove_file(destination).map_err(|err| {
                    Error::new(format!(
                        "failed to remove stale executable {}: {err}",
                        destination.display()
                    ))
                })?;
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(Error::new(format!(
                "failed to inspect {}: {err}",
                destination.display()
            )));
        }
    }

    std::fs::copy(source, destination).map_err(|err| {
        Error::new(format!(
            "failed to copy {} to {}: {err}",
            source.display(),
            destination.display()
        ))
    })?;

    let permissions = std::fs::metadata(source)
        .map_err(|err| Error::new(format!("failed to stat {}: {err}", source.display())))?
        .permissions();
    std::fs::set_permissions(destination, permissions).map_err(|err| {
        Error::new(format!(
            "failed to set permissions on {}: {err}",
            destination.display()
        ))
    })
}

fn bundle_id_component(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn main_bundle_info_plist(executable_name: &str, bundle_name: &str, bundle_id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>{executable_name}</string>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{bundle_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSEnvironment</key>
    <dict>
        <key>MallocNanoZone</key>
        <string>0</string>
    </dict>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
"#
    )
}

fn helper_bundle_info_plist(
    executable_name: &str,
    bundle_name: &str,
    bundle_id: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>{executable_name}</string>
    <key>CFBundleExecutable</key>
    <string>{executable_name}</string>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{bundle_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSEnvironment</key>
    <dict>
        <key>MallocNanoZone</key>
        <string>0</string>
    </dict>
    <key>LSFileQuarantineEnabled</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSUIElement</key>
    <string>1</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
"#
    )
}

fn ensure_framework_bundle_layout(source_framework_dir: &PathBuf, framework_dir: &PathBuf) -> Result<()> {
    match std::fs::symlink_metadata(framework_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            std::fs::remove_file(framework_dir).map_err(|err| {
                Error::new(format!(
                    "failed to remove stale framework symlink {}: {err}",
                    framework_dir.display()
                ))
            })?;
        }
        Ok(metadata) if !metadata.file_type().is_dir() => {
            std::fs::remove_file(framework_dir).map_err(|err| {
                Error::new(format!(
                    "failed to remove stale framework path {}: {err}",
                    framework_dir.display()
                ))
            })?;
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(Error::new(format!(
                "failed to inspect {}: {err}",
                framework_dir.display()
            )));
        }
    }

    std::fs::create_dir_all(framework_dir).map_err(|err| {
        Error::new(format!(
            "failed to create {}: {err}",
            framework_dir.display()
        ))
    })?;
    let versions_dir = framework_dir.join("Versions");
    std::fs::create_dir_all(&versions_dir).map_err(|err| {
        Error::new(format!(
            "failed to create {}: {err}",
            versions_dir.display()
        ))
    })?;

    ensure_symlink(source_framework_dir, &versions_dir.join("A"))?;
    ensure_symlink(&PathBuf::from("A"), &versions_dir.join("Current"))?;
    ensure_symlink(
        &PathBuf::from("Versions/Current/Chromium Embedded Framework"),
        &framework_dir.join("Chromium Embedded Framework"),
    )?;
    ensure_symlink(
        &PathBuf::from("Versions/Current/Libraries"),
        &framework_dir.join("Libraries"),
    )?;
    ensure_symlink(
        &PathBuf::from("Versions/Current/Resources"),
        &framework_dir.join("Resources"),
    )?;
    Ok(())
}

fn ensure_synthetic_app_bundle(
    runtime_paths: &RuntimePaths,
    main_executable: &PathBuf,
) -> Result<SyntheticAppBundle> {
    let app_name = main_executable
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("makepad-browser");
    let bundle_id = format!("dev.makepad.{}", bundle_id_component(app_name));

    let root_dir = env::temp_dir()
        .join("makepad-cef")
        .join("bundle")
        .join(app_name);
    let bundle_dir = root_dir.join(format!("{app_name}.app"));
    let contents_dir = bundle_dir.join("Contents");
    let macos_dir = contents_dir.join("MacOS");
    let resources_dir = contents_dir.join("Resources");
    let frameworks_dir = contents_dir.join("Frameworks");

    std::fs::create_dir_all(&macos_dir)
        .map_err(|err| Error::new(format!("failed to create {}: {err}", macos_dir.display())))?;
    std::fs::create_dir_all(&resources_dir).map_err(|err| {
        Error::new(format!(
            "failed to create {}: {err}",
            resources_dir.display()
        ))
    })?;
    std::fs::create_dir_all(&frameworks_dir).map_err(|err| {
        Error::new(format!(
            "failed to create {}: {err}",
            frameworks_dir.display()
        ))
    })?;

    let framework_dir = frameworks_dir.join("Chromium Embedded Framework.framework");
    let main_bundle_executable = macos_dir.join(app_name);
    let main_info_plist = contents_dir.join("Info.plist");
    let main_pkginfo = contents_dir.join("PkgInfo");
    let log_file = root_dir.join("cef.log");
    let helper_source = helper_binary_source();

    copy_executable(main_executable, &main_bundle_executable)?;
    ensure_framework_bundle_layout(&runtime_paths.framework_dir, &framework_dir)?;

    write_if_changed(
        &main_info_plist,
        &main_bundle_info_plist(app_name, app_name, &bundle_id),
    )?;
    write_if_changed(&main_pkginfo, PKGINFO_CONTENTS)?;

    let mut helper_executable = None;
    for (name_suffix, bundle_id_suffix) in HELPER_APP_SUFFIXES {
        let helper_name = format!("{app_name} Helper{name_suffix}");
        let helper_bundle_id = format!("{bundle_id}.helper{bundle_id_suffix}");
        let helper_bundle_dir = frameworks_dir.join(format!("{helper_name}.app"));
        let helper_contents_dir = helper_bundle_dir.join("Contents");
        let helper_macos_dir = helper_contents_dir.join("MacOS");
        let helper_resources_dir = helper_contents_dir.join("Resources");
        let helper_info_plist = helper_contents_dir.join("Info.plist");
        let helper_pkginfo = helper_contents_dir.join("PkgInfo");
        let helper_binary = helper_macos_dir.join(&helper_name);

        std::fs::create_dir_all(&helper_macos_dir).map_err(|err| {
            Error::new(format!(
                "failed to create {}: {err}",
                helper_macos_dir.display()
            ))
        })?;
        std::fs::create_dir_all(&helper_resources_dir).map_err(|err| {
            Error::new(format!(
                "failed to create {}: {err}",
                helper_resources_dir.display()
            ))
        })?;

        copy_executable(&helper_source, &helper_binary)?;
        write_if_changed(
            &helper_info_plist,
            &helper_bundle_info_plist(&helper_name, &helper_name, &helper_bundle_id),
        )?;
        write_if_changed(&helper_pkginfo, PKGINFO_CONTENTS)?;

        if helper_executable.is_none() {
            helper_executable = Some(helper_binary);
        }
    }
    let helper_executable =
        helper_executable.ok_or_else(|| Error::new("failed to create CEF helper bundle"))?;

    Ok(SyntheticAppBundle {
        bundle_dir,
        bundle_executable: main_bundle_executable,
        framework_dir: framework_dir.clone(),
        resources_dir: framework_dir.join("Resources"),
        helper_executable,
        log_file,
    })
}

fn is_running_inside_app_bundle(executable: &PathBuf) -> bool {
    let Some(macos_dir) = executable.parent() else {
        return false;
    };
    let Some(contents_dir) = macos_dir.parent() else {
        return false;
    };
    let Some(bundle_dir) = contents_dir.parent() else {
        return false;
    };
    macos_dir.file_name().and_then(|value| value.to_str()) == Some("MacOS")
        && contents_dir.file_name().and_then(|value| value.to_str()) == Some("Contents")
        && bundle_dir.extension().and_then(|value| value.to_str()) == Some("app")
}

pub fn reexec_into_app_bundle_if_needed() -> Result<()> {
    let current_executable = env::current_exe()
        .map_err(|err| Error::new(format!("failed to resolve current executable: {err}")))?;
    if is_running_inside_app_bundle(&current_executable) {
        return Ok(());
    }
    if env::var_os("MAKEPAD_CEF_APP_BUNDLE_EXEC").is_some() {
        return Err(Error::new(format!(
            "refusing to re-exec {} into an app bundle twice",
            current_executable.display()
        )));
    }

    let synthetic_bundle =
        ensure_synthetic_app_bundle(&distribution_runtime_paths(), &current_executable)?;
    let mut command = std::process::Command::new(&synthetic_bundle.bundle_executable);
    command.args(env::args_os().skip(1));
    command.env("MAKEPAD_CEF_APP_BUNDLE_EXEC", "1");
    command.env("MallocNanoZone", "0");
    let err = command.exec();
    Err(Error::new(format!(
        "failed to exec {}: {err}",
        synthetic_bundle.bundle_executable.display()
    )))
}

pub fn initialize() -> Result<()> {
    ensure_initialized()
}

impl SharedBrowserState {
    fn new(width: usize, height: usize, scale_factor: f32) -> Self {
        Self {
            view: Mutex::new(ViewState {
                width: width.max(1),
                height: height.max(1),
                scale_factor: scale_factor.max(0.1),
            }),
            frame: Mutex::new(None),
            closing: AtomicBool::new(false),
            editable_focus: AtomicBool::new(false),
        }
    }

    fn view(&self) -> ViewState {
        *self.view.lock().unwrap()
    }

    fn update_view(&self, width: usize, height: usize, scale_factor: f32) {
        *self.view.lock().unwrap() = ViewState {
            width: width.max(1),
            height: height.max(1),
            scale_factor: scale_factor.max(0.1),
        };
    }

    fn set_frame(&self, frame: Frame) {
        *self.frame.lock().unwrap() = Some(frame);
    }

    fn take_frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap().take()
    }

    fn editable_focus(&self) -> bool {
        self.editable_focus.load(Ordering::Acquire)
    }

    fn set_editable_focus(&self, editable_focus: bool) {
        self.editable_focus
            .store(editable_focus, Ordering::Release);
    }
}

unsafe fn release_ref_counted(base: *mut ffi::cef_base_ref_counted_t) {
    if let Some(release) = (*base).release {
        release(base);
    }
}

unsafe extern "system" fn client_add_ref(self_: *mut ffi::cef_base_ref_counted_t) {
    let client = self_ as *mut ClientHandler;
    (*client).ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "system" fn client_release(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
    let client = self_ as *mut ClientHandler;
    if (*client).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        if !(*client).render_handler.is_null() {
            release_ref_counted(&mut (*(*client).render_handler).cef_render_handler.base as *mut _);
        }
        drop(Box::from_raw(client));
        1
    } else {
        0
    }
}

unsafe extern "system" fn client_has_one_ref(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
    let client = self_ as *mut ClientHandler;
    ((*client).ref_count.load(Ordering::Acquire) == 1) as c_int
}

unsafe extern "system" fn client_has_at_least_one_ref(
    self_: *mut ffi::cef_base_ref_counted_t,
) -> c_int {
    let client = self_ as *mut ClientHandler;
    ((*client).ref_count.load(Ordering::Acquire) >= 1) as c_int
}

unsafe extern "system" fn render_add_ref(self_: *mut ffi::cef_base_ref_counted_t) {
    let render = self_ as *mut RenderHandler;
    (*render).ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "system" fn render_release(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
    let render = self_ as *mut RenderHandler;
    if (*render).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        drop(Box::from_raw(render));
        1
    } else {
        0
    }
}

unsafe extern "system" fn render_has_one_ref(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
    let render = self_ as *mut RenderHandler;
    ((*render).ref_count.load(Ordering::Acquire) == 1) as c_int
}

unsafe extern "system" fn render_has_at_least_one_ref(
    self_: *mut ffi::cef_base_ref_counted_t,
) -> c_int {
    let render = self_ as *mut RenderHandler;
    ((*render).ref_count.load(Ordering::Acquire) >= 1) as c_int
}

unsafe extern "system" fn browser_process_add_ref(self_: *mut ffi::cef_base_ref_counted_t) {
    let handler = self_ as *mut BrowserProcessHandler;
    (*handler).ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "system" fn browser_process_release(
    self_: *mut ffi::cef_base_ref_counted_t,
) -> c_int {
    let handler = self_ as *mut BrowserProcessHandler;
    if (*handler).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        drop(Box::from_raw(handler));
        1
    } else {
        0
    }
}

unsafe extern "system" fn browser_process_has_one_ref(
    self_: *mut ffi::cef_base_ref_counted_t,
) -> c_int {
    let handler = self_ as *mut BrowserProcessHandler;
    ((*handler).ref_count.load(Ordering::Acquire) == 1) as c_int
}

unsafe extern "system" fn browser_process_has_at_least_one_ref(
    self_: *mut ffi::cef_base_ref_counted_t,
) -> c_int {
    let handler = self_ as *mut BrowserProcessHandler;
    ((*handler).ref_count.load(Ordering::Acquire) >= 1) as c_int
}

unsafe extern "system" fn browser_process_on_schedule_message_pump_work(
    _self: *mut ffi::cef_browser_process_handler_t,
    delay_ms: i64,
) {
    if let Ok(pump) = external_pump() {
        pump.schedule(delay_ms);
    }
}

unsafe extern "system" fn app_add_ref(self_: *mut ffi::cef_base_ref_counted_t) {
    let app = self_ as *mut AppHandler;
    (*app).ref_count.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "system" fn app_release(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
    let app = self_ as *mut AppHandler;
    if (*app).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
        if !(*app).browser_process_handler.is_null() {
            release_ref_counted(
                &mut (*(*app).browser_process_handler).cef_browser_process_handler.base as *mut _,
            );
        }
        drop(Box::from_raw(app));
        1
    } else {
        0
    }
}

unsafe extern "system" fn app_has_one_ref(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
    let app = self_ as *mut AppHandler;
    ((*app).ref_count.load(Ordering::Acquire) == 1) as c_int
}

unsafe extern "system" fn app_has_at_least_one_ref(
    self_: *mut ffi::cef_base_ref_counted_t,
) -> c_int {
    let app = self_ as *mut AppHandler;
    ((*app).ref_count.load(Ordering::Acquire) >= 1) as c_int
}

unsafe extern "system" fn app_get_browser_process_handler(
    self_: *mut ffi::cef_app_t,
) -> *mut ffi::cef_browser_process_handler_t {
    let app = self_ as *mut AppHandler;
    let handler = (*app).browser_process_handler;
    if handler.is_null() {
        return ptr::null_mut();
    }
    let base = &mut (*handler).cef_browser_process_handler.base as *mut ffi::cef_base_ref_counted_t;
    if let Some(add_ref) = (*base).add_ref {
        add_ref(base);
    }
    &mut (*handler).cef_browser_process_handler
}

unsafe extern "system" fn client_null_handler(_self: *mut ffi::cef_client_t) -> *mut c_void {
    ptr::null_mut()
}

unsafe extern "system" fn client_get_render_handler(
    self_: *mut ffi::cef_client_t,
) -> *mut ffi::cef_render_handler_t {
    let client = self_ as *mut ClientHandler;
    let render = (*client).render_handler;
    if !render.is_null() {
        let base = &mut (*render).cef_render_handler.base as *mut ffi::cef_base_ref_counted_t;
        if let Some(add_ref) = (*base).add_ref {
            add_ref(base);
        }
        return &mut (*render).cef_render_handler;
    }
    ptr::null_mut()
}

unsafe extern "system" fn render_get_root_screen_rect(
    self_: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    rect: *mut ffi::cef_rect_t,
) -> c_int {
    if rect.is_null() {
        return 0;
    }
    let render = self_ as *mut RenderHandler;
    let view = (*render).state.view();
    *rect = ffi::cef_rect_t {
        x: 0,
        y: 0,
        width: view.width as c_int,
        height: view.height as c_int,
    };
    1
}

unsafe extern "system" fn render_get_view_rect(
    self_: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    rect: *mut ffi::cef_rect_t,
) {
    if rect.is_null() {
        return;
    }
    let render = self_ as *mut RenderHandler;
    let view = (*render).state.view();
    *rect = ffi::cef_rect_t {
        x: 0,
        y: 0,
        width: view.width as c_int,
        height: view.height as c_int,
    };
}

unsafe extern "system" fn render_get_screen_point(
    _self: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    view_x: c_int,
    view_y: c_int,
    screen_x: *mut c_int,
    screen_y: *mut c_int,
) -> c_int {
    if !screen_x.is_null() {
        *screen_x = view_x;
    }
    if !screen_y.is_null() {
        *screen_y = view_y;
    }
    1
}

unsafe extern "system" fn render_get_screen_info(
    self_: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    screen_info: *mut ffi::cef_screen_info_t,
) -> c_int {
    if screen_info.is_null() {
        return 0;
    }
    let render = self_ as *mut RenderHandler;
    let view = (*render).state.view();
    *screen_info = ffi::cef_screen_info_t {
        size: std::mem::size_of::<ffi::cef_screen_info_t>(),
        device_scale_factor: view.scale_factor,
        depth: 32,
        depth_per_component: 8,
        is_monochrome: 0,
        rect: ffi::cef_rect_t {
            x: 0,
            y: 0,
            width: view.width as c_int,
            height: view.height as c_int,
        },
        available_rect: ffi::cef_rect_t {
            x: 0,
            y: 0,
            width: view.width as c_int,
            height: view.height as c_int,
        },
    };
    1
}

unsafe extern "system" fn render_on_popup_show(
    _self: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    _show: c_int,
) {
}

unsafe extern "system" fn render_on_popup_size(
    _self: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    _rect: *const ffi::cef_rect_t,
) {
}

unsafe extern "system" fn render_on_paint(
    self_: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    type_: ffi::cef_paint_element_type_t,
    _dirty_rects_count: usize,
    _dirty_rects: *const ffi::cef_rect_t,
    buffer: *const c_void,
    width: c_int,
    height: c_int,
) {
    if type_ != ffi::PET_VIEW || buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let render = self_ as *mut RenderHandler;
    let pixels =
        slice::from_raw_parts(buffer as *const u32, width as usize * height as usize).to_vec();
    (*render).state.set_frame(Frame {
        width: width as usize,
        height: height as usize,
        pixels,
    });
}

unsafe extern "system" fn render_on_virtual_keyboard_requested(
    self_: *mut ffi::cef_render_handler_t,
    _browser: *mut ffi::cef_browser_t,
    input_mode: ffi::cef_text_input_mode_t,
) {
    let render = self_ as *mut RenderHandler;
    (*render)
        .state
        .set_editable_focus(input_mode != TEXT_INPUT_MODE_NONE);
}

impl RenderHandler {
    fn allocate(state: Arc<SharedBrowserState>) -> *mut RenderHandler {
        Box::into_raw(Box::new(Self {
            cef_render_handler: ffi::cef_render_handler_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_render_handler_t>(),
                    add_ref: Some(render_add_ref),
                    release: Some(render_release),
                    has_one_ref: Some(render_has_one_ref),
                    has_at_least_one_ref: Some(render_has_at_least_one_ref),
                },
                get_accessibility_handler: None,
                get_root_screen_rect: Some(render_get_root_screen_rect),
                get_view_rect: Some(render_get_view_rect),
                get_screen_point: Some(render_get_screen_point),
                get_screen_info: Some(render_get_screen_info),
                on_popup_show: Some(render_on_popup_show),
                on_popup_size: Some(render_on_popup_size),
                on_paint: Some(render_on_paint),
                on_accelerated_paint: None,
                get_touch_handle_size: None,
                on_touch_handle_state_changed: None,
                start_dragging: None,
                update_drag_cursor: None,
                on_scroll_offset_changed: None,
                on_ime_composition_range_changed: None,
                on_text_selection_changed: None,
                on_virtual_keyboard_requested: Some(render_on_virtual_keyboard_requested),
            },
            ref_count: AtomicUsize::new(1),
            state,
        }))
    }
}

impl ClientHandler {
    fn allocate(render_handler: *mut RenderHandler) -> *mut ClientHandler {
        Box::into_raw(Box::new(Self {
            cef_client: ffi::cef_client_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_client_t>(),
                    add_ref: Some(client_add_ref),
                    release: Some(client_release),
                    has_one_ref: Some(client_has_one_ref),
                    has_at_least_one_ref: Some(client_has_at_least_one_ref),
                },
                get_audio_handler: Some(client_null_handler),
                get_command_handler: Some(client_null_handler),
                get_context_menu_handler: Some(client_null_handler),
                get_dialog_handler: Some(client_null_handler),
                get_display_handler: Some(client_null_handler),
                get_download_handler: Some(client_null_handler),
                get_drag_handler: Some(client_null_handler),
                get_find_handler: Some(client_null_handler),
                get_focus_handler: Some(client_null_handler),
                get_frame_handler: Some(client_null_handler),
                get_permission_handler: Some(client_null_handler),
                get_jsdialog_handler: Some(client_null_handler),
                get_keyboard_handler: Some(client_null_handler),
                get_life_span_handler: Some(client_null_handler),
                get_load_handler: Some(client_null_handler),
                get_print_handler: Some(client_null_handler),
                get_render_handler: Some(client_get_render_handler),
                get_request_handler: Some(client_null_handler),
                on_process_message_received: None,
            },
            ref_count: AtomicUsize::new(1),
            render_handler,
        }))
    }
}

impl BrowserProcessHandler {
    fn allocate() -> *mut BrowserProcessHandler {
        Box::into_raw(Box::new(Self {
            cef_browser_process_handler: ffi::cef_browser_process_handler_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_browser_process_handler_t>(),
                    add_ref: Some(browser_process_add_ref),
                    release: Some(browser_process_release),
                    has_one_ref: Some(browser_process_has_one_ref),
                    has_at_least_one_ref: Some(browser_process_has_at_least_one_ref),
                },
                on_register_custom_preferences: None,
                on_context_initialized: None,
                on_before_child_process_launch: None,
                on_already_running_app_relaunch: None,
                on_schedule_message_pump_work: Some(
                    browser_process_on_schedule_message_pump_work,
                ),
                get_default_client: None,
                get_default_request_context_handler: None,
            },
            ref_count: AtomicUsize::new(1),
        }))
    }
}

impl AppHandler {
    fn allocate() -> *mut AppHandler {
        let browser_process_handler = BrowserProcessHandler::allocate();
        Box::into_raw(Box::new(Self {
            cef_app: ffi::cef_app_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_app_t>(),
                    add_ref: Some(app_add_ref),
                    release: Some(app_release),
                    has_one_ref: Some(app_has_one_ref),
                    has_at_least_one_ref: Some(app_has_at_least_one_ref),
                },
                on_before_command_line_processing: None,
                on_register_custom_schemes: None,
                get_resource_bundle_handler: None,
                get_browser_process_handler: Some(app_get_browser_process_handler),
                get_render_process_handler: None,
            },
            ref_count: AtomicUsize::new(1),
            browser_process_handler,
        }))
    }
}

impl Browser {
    fn with_host<T>(
        &self,
        f: impl FnOnce(*mut ffi::cef_browser_host_t) -> Result<T>,
    ) -> Result<T> {
        unsafe {
            let host = (*self.browser)
                .get_host
                .ok_or_else(|| Error::new("cef_browser_t::get_host missing"))?(self.browser);
            if host.is_null() {
                return Err(Error::new("cef_browser_t::get_host returned null"));
            }
            let result = f(host);
            release_ref_counted(&mut (*host).base as *mut _);
            result
        }
    }

    fn with_main_frame<T>(
        &self,
        f: impl FnOnce(*mut ffi::cef_frame_t) -> Result<T>,
    ) -> Result<T> {
        unsafe {
            let frame = (*self.browser)
                .get_main_frame
                .ok_or_else(|| Error::new("cef_browser_t::get_main_frame missing"))?(self.browser);
            if frame.is_null() {
                return Err(Error::new("cef_browser_t::get_main_frame returned null"));
            }
            let result = f(frame);
            release_ref_counted(&mut (*frame).base as *mut _);
            result
        }
    }

    pub fn new(url: &str, width: usize, height: usize, scale_factor: f32) -> Result<Self> {
        ensure_initialized()?;
        let runtime = runtime()?;

        let state = Arc::new(SharedBrowserState::new(width, height, scale_factor));
        let render_handler = RenderHandler::allocate(state.clone());
        let client = ClientHandler::allocate(render_handler);

        let url = CefString::new(
            &runtime.api,
            if url.is_empty() { "about:blank" } else { url },
        )?;
        let mut window_info = ffi::cef_window_info_t {
            size: std::mem::size_of::<ffi::cef_window_info_t>(),
            window_name: ffi::cef_string_t::default(),
            bounds: ffi::cef_rect_t {
                x: 0,
                y: 0,
                width: width.max(1) as c_int,
                height: height.max(1) as c_int,
            },
            hidden: 0,
            parent_view: ptr::null_mut(),
            windowless_rendering_enabled: 1,
            shared_texture_enabled: 0,
            external_begin_frame_enabled: 0,
            view: ptr::null_mut(),
            runtime_style: 0,
        };
        let mut browser_settings = ffi::cef_browser_settings_t {
            size: std::mem::size_of::<ffi::cef_browser_settings_t>(),
            windowless_frame_rate: 60,
            ..Default::default()
        };

        let browser = unsafe {
            (runtime.api.cef_browser_host_create_browser_sync)(
                &mut window_info,
                &mut (*client).cef_client,
                &url.value,
                &mut browser_settings,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };

        if browser.is_null() {
            unsafe {
                release_ref_counted(&mut (*client).cef_client.base as *mut _);
            }
            return Err(Error::new(
                "cef_browser_host_create_browser_sync returned null",
            ));
        }

        let mut this = Self {
            browser,
            state,
            width: 0,
            height: 0,
            scale_factor: 0.0,
        };
        let _ = this.resize(width, height, scale_factor);
        if let Ok(pump) = external_pump() {
            pump.schedule(0);
        }
        Ok(this)
    }

    pub fn resize(&mut self, width: usize, height: usize, scale_factor: f32) -> Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        let scale_factor = scale_factor.max(0.1);
        let scale_changed = (self.scale_factor - scale_factor).abs() >= f32::EPSILON;
        if self.width == width
            && self.height == height
            && !scale_changed
        {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self.scale_factor = scale_factor;
        self.state.update_view(width, height, scale_factor);

        self.with_host(|host| unsafe {
            if scale_changed {
                if let Some(notify_screen_info_changed) = (*host).notify_screen_info_changed {
                    notify_screen_info_changed(host);
                }
            }
            if let Some(was_resized) = (*host).was_resized {
                was_resized(host);
            }
            if let Some(invalidate) = (*host).invalidate {
                invalidate(host, ffi::PET_VIEW);
            }
            Ok(())
        })
    }

    pub fn set_url(&mut self, url: &str) -> Result<()> {
        let runtime = runtime()?;
        let url = CefString::new(
            &runtime.api,
            if url.is_empty() { "about:blank" } else { url },
        )?;
        self.with_main_frame(|frame| unsafe {
            (*frame)
                .load_url
                .ok_or_else(|| Error::new("cef_frame_t::load_url missing"))?(frame, &url.value);
            Ok(())
        })
    }

    pub fn set_focus(&mut self, focus: bool) -> Result<()> {
        self.with_host(|host| unsafe {
            (*host)
                .set_focus
                .ok_or_else(|| Error::new("cef_browser_host_t::set_focus missing"))?(
                host,
                focus as c_int,
            );
            Ok(())
        })
    }

    pub fn send_mouse_move(
        &mut self,
        x: i32,
        y: i32,
        modifiers: u32,
        mouse_leave: bool,
    ) -> Result<()> {
        let event = ffi::cef_mouse_event_t { x, y, modifiers };
        self.with_host(|host| unsafe {
            (*host)
                .send_mouse_move_event
                .ok_or_else(|| Error::new("cef_browser_host_t::send_mouse_move_event missing"))?(
                host,
                &event,
                mouse_leave as c_int,
            );
            Ok(())
        })
    }

    pub fn send_mouse_click(
        &mut self,
        x: i32,
        y: i32,
        modifiers: u32,
        button: i32,
        mouse_up: bool,
        click_count: i32,
    ) -> Result<()> {
        let event = ffi::cef_mouse_event_t { x, y, modifiers };
        self.with_host(|host| unsafe {
            (*host)
                .send_mouse_click_event
                .ok_or_else(|| Error::new("cef_browser_host_t::send_mouse_click_event missing"))?(
                host,
                &event,
                button,
                mouse_up as c_int,
                click_count,
            );
            Ok(())
        })
    }

    pub fn send_mouse_wheel(
        &mut self,
        x: i32,
        y: i32,
        modifiers: u32,
        delta_x: i32,
        delta_y: i32,
    ) -> Result<()> {
        let event = ffi::cef_mouse_event_t { x, y, modifiers };
        self.with_host(|host| unsafe {
            (*host)
                .send_mouse_wheel_event
                .ok_or_else(|| Error::new("cef_browser_host_t::send_mouse_wheel_event missing"))?(
                host,
                &event,
                delta_x,
                delta_y,
            );
            Ok(())
        })
    }

    pub fn send_key_event(
        &mut self,
        event_type: i32,
        modifiers: u32,
        windows_key_code: i32,
        native_key_code: i32,
        character: u16,
        unmodified_character: u16,
        is_system_key: bool,
    ) -> Result<()> {
        let event = ffi::cef_key_event_t {
            size: std::mem::size_of::<ffi::cef_key_event_t>(),
            type_: event_type,
            modifiers,
            windows_key_code,
            native_key_code,
            is_system_key: is_system_key as c_int,
            character,
            unmodified_character,
            focus_on_editable_field: self.state.editable_focus() as c_int,
        };
        self.with_host(|host| unsafe {
            (*host)
                .send_key_event
                .ok_or_else(|| Error::new("cef_browser_host_t::send_key_event missing"))?(
                host,
                &event,
            );
            Ok(())
        })
    }

    pub fn ime_commit_text(&mut self, text: &str) -> Result<()> {
        let runtime = runtime()?;
        let text = CefString::new(&runtime.api, text)?;
        self.with_host(|host| unsafe {
            (*host)
                .ime_commit_text
                .ok_or_else(|| Error::new("cef_browser_host_t::ime_commit_text missing"))?(
                host,
                &text.value,
                ptr::null(),
                0,
            );
            Ok(())
        })
    }

    pub fn take_frame(&mut self) -> Option<Frame> {
        self.state.take_frame()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.state.closing.store(true, Ordering::Release);
        if let Ok(runtime) = runtime() {
            let state = runtime.state.lock().unwrap();
            if state.initialized && !state.shutting_down && !self.browser.is_null() {
                unsafe {
                    if let Some(get_host) = (*self.browser).get_host {
                        let host = get_host(self.browser);
                        if !host.is_null() {
                            if let Some(close_browser) = (*host).close_browser {
                                close_browser(host, 1);
                            }
                            release_ref_counted(&mut (*host).base as *mut _);
                        }
                    }
                }
            }
        }
        if !self.browser.is_null() {
            unsafe {
                release_ref_counted(&mut (*self.browser).base as *mut _);
            }
        }
    }
}

pub fn bootstrap() -> Result<BootstrapResult> {
    let runtime = runtime()?;
    let args = MainArgsStorage::current_process()?;
    let app = AppHandler::allocate();
    let exit_code = unsafe {
        (runtime.api.cef_execute_process)(&args.main_args, &mut (*app).cef_app, ptr::null_mut())
    };
    unsafe {
        release_ref_counted(&mut (*app).cef_app.base as *mut _);
    }
    if exit_code >= 0 {
        Ok(BootstrapResult::Exit(exit_code))
    } else {
        Ok(BootstrapResult::Continue)
    }
}

fn do_message_loop_work_impl() {
    if let Ok(runtime) = runtime() {
        let should_pump = {
            let state = runtime.state.lock().unwrap();
            state.initialized && !state.shutting_down
        };
        if should_pump {
            unsafe {
                (runtime.api.cef_do_message_loop_work)();
            }
        }
    }
}

pub fn do_message_loop_work() {
    do_message_loop_work_impl();
}

pub fn shutdown() {
    let Ok(runtime) = runtime() else {
        return;
    };
    let mut state = runtime.state.lock().unwrap();
    if !state.initialized || state.shutting_down {
        return;
    }
    state.shutting_down = true;
    unsafe {
        (runtime.api.cef_shutdown)();
    }
    let app = state.app as *mut AppHandler;
    state.app = 0;
    if !app.is_null() {
        unsafe {
            release_ref_counted(&mut (*app).cef_app.base as *mut _);
        }
    }
    state.initialized = false;
}
