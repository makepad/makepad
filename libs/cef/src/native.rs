use crate::{ffi, BootstrapResult, Error, Frame, Result, TEXT_INPUT_MODE_NONE};
use libloading::Library;
#[cfg(target_os = "macos")]
use makepad_objc_sys::declare::ClassDecl;
#[cfg(target_os = "macos")]
use makepad_objc_sys::runtime::{self, Class, ObjcId, Object, Protocol, Sel, BOOL, NO, YES};
#[cfg(target_os = "macos")]
use makepad_objc_sys::{class, msg_send, sel, sel_impl};
use std::env;
use std::ffi::{c_char, c_void, CString};
use std::os::raw::c_int;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(target_os = "macos")]
use std::os::unix::fs::symlink;
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_os = "macos")]
extern "C" {
    static NSRunLoopCommonModes: ObjcId;
}

#[cfg(target_os = "macos")]
#[link(name = "Metal", kind = "framework")]
extern "C" {
    fn MTLCreateSystemDefaultDevice() -> ObjcId;
}

#[cfg(target_os = "macos")]
#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
    fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

// Metal constants (MTLPixelFormat / MTLTextureType / MTLStorageMode / MTLTextureUsage).
#[cfg(target_os = "macos")]
const MTL_PIXEL_FORMAT_BGRA8_UNORM: u64 = 80;
#[cfg(target_os = "macos")]
const MTL_TEXTURE_TYPE_2D: u64 = 2;
#[cfg(target_os = "macos")]
const MTL_STORAGE_MODE_SHARED: u64 = 0;
#[cfg(target_os = "macos")]
const MTL_TEXTURE_USAGE_SHADER_READ: u64 = 1;
#[cfg(target_os = "macos")]
const MTL_TEXTURE_USAGE_RENDER_TARGET: u64 = 4;

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MTLOrigin {
    x: u64,
    y: u64,
    z: u64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct MTLSize {
    width: u64,
    height: u64,
    depth: u64,
}

/// How a browser delivers its frames. `Accelerated` = CEF paints on the GPU
/// into a pooled IOSurface which we blit into a client-owned IOSurface on
/// every `on_accelerated_paint`; `Software` = the classic CPU BGRA buffer of
/// `on_paint`. `None` = nothing arrived yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    None,
    Software,
    Accelerated,
}

/// Our own Metal device + command queue for the IOSurface -> IOSurface blit.
/// This is the system default device, i.e. the same GPU Makepad renders with;
/// both sides talk to the same IOSurface memory, so no cross-device transfer
/// is involved.
#[cfg(target_os = "macos")]
struct MetalBlit {
    device: usize,
    queue: usize,
}

#[cfg(target_os = "macos")]
unsafe impl Send for MetalBlit {}
#[cfg(target_os = "macos")]
unsafe impl Sync for MetalBlit {}

#[cfg(target_os = "macos")]
impl MetalBlit {
    fn device(&self) -> ObjcId {
        self.device as ObjcId
    }

    fn queue(&self) -> ObjcId {
        self.queue as ObjcId
    }
}

#[cfg(target_os = "macos")]
fn metal_blit() -> Option<&'static MetalBlit> {
    static METAL: OnceLock<Option<MetalBlit>> = OnceLock::new();
    METAL
        .get_or_init(|| unsafe {
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                return None;
            }
            let queue: ObjcId = msg_send![device, newCommandQueue];
            if queue.is_null() {
                return None;
            }
            Some(MetalBlit {
                device: device as usize,
                queue: queue as usize,
            })
        })
        .as_ref()
}

/// Wrap an IOSurface in a Metal texture on our blit device. Returns a +1
/// retained MTLTexture (caller releases).
#[cfg(target_os = "macos")]
unsafe fn metal_texture_for_iosurface(
    device: ObjcId,
    iosurface: *mut c_void,
    width: usize,
    height: usize,
    pixel_format: u64,
) -> ObjcId {
    let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
    if descriptor.is_null() {
        return ptr::null_mut();
    }
    let () = msg_send![descriptor, setTextureType: MTL_TEXTURE_TYPE_2D];
    let () = msg_send![descriptor, setPixelFormat: pixel_format];
    let () = msg_send![descriptor, setWidth: width as u64];
    let () = msg_send![descriptor, setHeight: height as u64];
    let () = msg_send![descriptor, setDepth: 1u64];
    let () = msg_send![descriptor, setMipmapLevelCount: 1u64];
    let () = msg_send![descriptor, setStorageMode: MTL_STORAGE_MODE_SHARED];
    let () = msg_send![
        descriptor,
        setUsage: (MTL_TEXTURE_USAGE_SHADER_READ | MTL_TEXTURE_USAGE_RENDER_TARGET)
    ];
    let texture: ObjcId = msg_send![
        device,
        newTextureWithDescriptor: descriptor
        iosurface: iosurface
        plane: 0u64
    ];
    let () = msg_send![descriptor, release];
    texture
}

/// The client-owned copy destination for accelerated paints: an IOSurface
/// created by the embedder (Makepad's `create_iosurface_render_texture`) that
/// we keep retained, plus a Metal view of it on our blit device.
#[cfg(target_os = "macos")]
struct AccelTarget {
    iosurface: usize,
    texture: usize,
    width: usize,
    height: usize,
}

#[cfg(target_os = "macos")]
unsafe impl Send for AccelTarget {}

#[cfg(target_os = "macos")]
impl Drop for AccelTarget {
    fn drop(&mut self) {
        unsafe {
            if self.texture != 0 {
                let () = msg_send![self.texture as ObjcId, release];
            }
            if self.iosurface != 0 {
                CFRelease(self.iosurface as *const c_void);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AccelStats {
    frames: u64,
    last_width: usize,
    last_height: usize,
    last_format: c_int,
    last_blit_micros: u64,
    total_blit_micros: u64,
    dropped_no_target: u64,
    /// Blits into the target that is registered *right now*, reset to 0 the
    /// moment a different IOSurface is registered. The embedder needs this to
    /// know when a freshly handed-over surface holds a page frame: until it
    /// does, the surface is blank and must not be shown.
    target_frames: u64,
    /// The region of the target the last blit actually filled — `min(paint,
    /// target)`. A target bigger than the page (the embedder over-allocates so
    /// a resize does not need a new surface every step) only holds pixels in
    /// this sub-rect; everything outside it is stale or never written.
    last_copy_width: usize,
    last_copy_height: usize,
}

/// The API version handed to `cef_api_hash`: the ABI layout the structs in
/// `ffi.rs` were written for (the 138 distribution's last). A newer libcef
/// (144, 151, …) keeps serving this layout when asked for it, so one set of
/// structs works across dists; asking for a dist's own newest version would
/// misalign `cef_browser_settings_t` (144 inserts `databases`).
const CEF_API_VERSION: c_int = 13800;
#[cfg(target_os = "macos")]
const HELPER_APP_SUFFIXES: [(&str, &str); 5] = [
    ("", ""),
    (" (Alerts)", ".alerts"),
    (" (GPU)", ".gpu"),
    (" (Plugin)", ".plugin"),
    (" (Renderer)", ".renderer"),
];
#[cfg(target_os = "macos")]
const PKGINFO_CONTENTS: &str = "APPL????";
#[cfg(target_os = "macos")]
const RTLD_FIRST: c_int = 0x100;
#[cfg(target_os = "macos")]
const EXTERNAL_PUMP_TIMER_PLACEHOLDER: i64 = i32::MAX as i64;
#[cfg(target_os = "macos")]
const EXTERNAL_PUMP_MAX_DELAY_MS: i64 = 1000 / 30;
#[cfg(target_os = "macos")]
const MOCK_KEYCHAIN_SWITCH: &str = "use-mock-keychain";
/// Set by measurement (see `use_angle_backend`): `metal` is the lowest-CPU
/// backend that delivers IOSurfaces; `Some("gl")` was the pre-accelerated
/// choice; `None` = Chromium's own default.
#[cfg(target_os = "macos")]
const DEFAULT_USE_ANGLE: Option<&str> = Some("metal");
/// Chromium's own default (ANGLE over D3D11) on Windows.
#[cfg(not(target_os = "macos"))]
const DEFAULT_USE_ANGLE: Option<&str> = None;
const FAVICON_MAX_SIZE: u32 = 64;

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
    cef_string_list_size: unsafe extern "C" fn(list: ffi::cef_string_list_t) -> usize,
    cef_string_list_value: unsafe extern "C" fn(
        list: ffi::cef_string_list_t,
        index: usize,
        value: *mut ffi::cef_string_t,
    ) -> c_int,
}

#[cfg(target_os = "macos")]
struct RuntimePaths {
    framework_bin: PathBuf,
    framework_dir: PathBuf,
    _resources_dir: PathBuf,
}

/// The binary distribution as laid out on Windows: `Release/libcef.dll` with
/// its sibling DLLs (the module dir), `Resources/` with the pak files,
/// `icudtl.dat` and `locales/`.
#[cfg(windows)]
struct RuntimePaths {
    libcef: PathBuf,
    module_dir: PathBuf,
    resources_dir: PathBuf,
}

struct Runtime {
    _library: Library,
    api: CefApi,
    _paths: RuntimePaths,
    state: Mutex<RuntimeState>,
}

#[derive(Default)]
struct RuntimeState {
    initialized: bool,
    shutting_down: bool,
    app: usize,
}

#[cfg(target_os = "macos")]
struct SyntheticAppBundle {
    _bundle_dir: PathBuf,
    bundle_executable: PathBuf,
    _framework_dir: PathBuf,
    _resources_dir: PathBuf,
    helper_executable: PathBuf,
    log_file: PathBuf,
}

#[cfg(unix)]
struct MainArgsStorage {
    _args: Vec<CString>,
    _ptrs: Vec<*mut c_char>,
    main_args: ffi::cef_main_args_t,
}

/// Windows CEF takes the module handle and reads `GetCommandLineW` itself;
/// the switches unix passes in argv go in through
/// `app_on_before_command_line_processing`.
#[cfg(windows)]
struct MainArgsStorage {
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
    /// Accelerated paint: the copy destination and the frame statistics.
    #[cfg(target_os = "macos")]
    accel_target: Mutex<Option<AccelTarget>>,
    accel: Mutex<AccelStats>,
    accel_frames: AtomicU64,
    software_frames: AtomicU64,
    /// Navigation state mirrored from the display/load handlers. `nav_gen`
    /// bumps on every change so the embedder can poll cheaply.
    nav: Mutex<NavState>,
    nav_gen: AtomicU64,
    /// `window.open` / target=_blank requests, cancelled as native popups and
    /// handed to the embedder to open as tabs.
    popup_requests: Mutex<Vec<String>>,
    /// The latest decoded favicon bitmap (BGRA, premultiplied).
    favicon: Mutex<Option<Frame>>,
    favicon_url: Mutex<String>,
}

#[derive(Default, Clone, Debug)]
struct NavState {
    title: String,
    url: String,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
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
    display_handler: *mut DisplayHandler,
    load_handler: *mut LoadHandler,
    life_span_handler: *mut LifeSpanHandler,
}

#[repr(C)]
struct DisplayHandler {
    cef_display_handler: ffi::cef_display_handler_t,
    ref_count: AtomicUsize,
    state: Arc<SharedBrowserState>,
}

#[repr(C)]
struct LoadHandler {
    cef_load_handler: ffi::cef_load_handler_t,
    ref_count: AtomicUsize,
    state: Arc<SharedBrowserState>,
}

#[repr(C)]
struct LifeSpanHandler {
    cef_life_span_handler: ffi::cef_life_span_handler_t,
    ref_count: AtomicUsize,
    state: Arc<SharedBrowserState>,
}

#[repr(C)]
struct DownloadImageCallback {
    cef_callback: ffi::cef_download_image_callback_t,
    ref_count: AtomicUsize,
    state: Arc<SharedBrowserState>,
}

/// The `cef_base_ref_counted_t` vtable for a `#[repr(C)]` handler whose first
/// field is the CEF struct. `$drop_hook` runs right before the box is freed.
macro_rules! impl_ref_counted {
    ($ty:ident, $add_ref:ident, $release:ident, $has_one:ident, $has_at_least_one:ident, $drop_hook:expr) => {
        unsafe extern "system" fn $add_ref(self_: *mut ffi::cef_base_ref_counted_t) {
            let this = self_ as *mut $ty;
            (*this).ref_count.fetch_add(1, Ordering::Relaxed);
        }

        unsafe extern "system" fn $release(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
            let this = self_ as *mut $ty;
            if (*this).ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
                let hook: unsafe fn(*mut $ty) = $drop_hook;
                hook(this);
                drop(Box::from_raw(this));
                1
            } else {
                0
            }
        }

        unsafe extern "system" fn $has_one(self_: *mut ffi::cef_base_ref_counted_t) -> c_int {
            let this = self_ as *mut $ty;
            ((*this).ref_count.load(Ordering::Acquire) == 1) as c_int
        }

        unsafe extern "system" fn $has_at_least_one(
            self_: *mut ffi::cef_base_ref_counted_t,
        ) -> c_int {
            let this = self_ as *mut $ty;
            ((*this).ref_count.load(Ordering::Acquire) >= 1) as c_int
        }
    };
}

unsafe fn no_drop_hook<T>(_this: *mut T) {}

impl_ref_counted!(
    DisplayHandler,
    display_add_ref,
    display_release,
    display_has_one_ref,
    display_has_at_least_one_ref,
    no_drop_hook
);
impl_ref_counted!(
    LoadHandler,
    load_add_ref,
    load_release,
    load_has_one_ref,
    load_has_at_least_one_ref,
    no_drop_hook
);
impl_ref_counted!(
    LifeSpanHandler,
    life_span_add_ref,
    life_span_release,
    life_span_has_one_ref,
    life_span_has_at_least_one_ref,
    no_drop_hook
);
impl_ref_counted!(
    DownloadImageCallback,
    download_image_add_ref,
    download_image_release,
    download_image_has_one_ref,
    download_image_has_at_least_one_ref,
    no_drop_hook
);

/// Copy a CEF UTF-16 string into a Rust `String`.
unsafe fn cef_string_to_string(value: *const ffi::cef_string_t) -> String {
    if value.is_null() {
        return String::new();
    }
    let value = &*value;
    if value.str_.is_null() || value.length == 0 {
        return String::new();
    }
    String::from_utf16_lossy(slice::from_raw_parts(value.str_, value.length))
}

/// Hand a handler pointer back to CEF with an extra reference, the way every
/// `get_*_handler` accessor must.
unsafe fn add_ref_and_return<T>(handler: *mut T) -> *mut T {
    if handler.is_null() {
        return ptr::null_mut();
    }
    let base = handler as *mut ffi::cef_base_ref_counted_t;
    if let Some(add_ref) = (*base).add_ref {
        add_ref(base);
    }
    handler
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

#[cfg(target_os = "macos")]
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
    accelerated: bool,
    hidden: bool,
}

/// A snapshot of the accelerated-paint statistics for reporting.
#[derive(Clone, Copy, Debug, Default)]
pub struct AcceleratedStats {
    pub frames: u64,
    pub last_width: usize,
    pub last_height: usize,
    /// `1` = BGRA_8888, `0` = RGBA_8888 (cef_color_type_t).
    pub last_format: i32,
    pub last_blit_micros: u64,
    pub total_blit_micros: u64,
    pub dropped_no_target: u64,
    /// Blits into the currently registered target since it was registered.
    /// `0` means the surface handed to `set_accelerated_target` is still
    /// blank — show the previous one until this turns non-zero.
    pub target_frames: u64,
    /// The sub-rect of the target the last blit filled (`min(paint, target)`);
    /// the region outside it holds no page pixels. Sample only this part.
    pub last_copy_width: usize,
    pub last_copy_height: usize,
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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
        _resources_dir: resources_dir,
    }
}

#[cfg(target_os = "macos")]
fn runtime_paths() -> RuntimePaths {
    if let Ok(current_executable) = env::current_exe() {
        if let Some(framework_dir) = framework_dir_for_bundle_executable(&current_executable) {
            let framework_bin = framework_dir.join("Chromium Embedded Framework");
            let resources_dir = framework_dir.join("Resources");
            if framework_bin.exists() && resources_dir.exists() {
                return RuntimePaths {
                    framework_bin,
                    framework_dir,
                    _resources_dir: resources_dir,
                };
            }
        }
    }

    distribution_runtime_paths()
}

#[cfg(target_os = "macos")]
fn helper_binary_source() -> PathBuf {
    env::var_os("MAKEPAD_CEF_HELPER_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("MAKEPAD_CEF_HELPER_BIN")))
}

#[cfg(windows)]
fn runtime_paths() -> RuntimePaths {
    let dist_dir = env::var_os("MAKEPAD_CEF_DIST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("MAKEPAD_CEF_DIST_DIR")));
    let module_dir = dist_dir.join("Release");
    RuntimePaths {
        libcef: module_dir.join("libcef.dll"),
        resources_dir: dist_dir.join("Resources"),
        module_dir,
    }
}

/// The Windows distribution keeps `Resources/` (icudtl.dat, the pak files,
/// locales/) apart from `Release/` (libcef.dll), but libcef looks for them
/// next to itself — icudtl.dat with no override at all ("Invalid file
/// descriptor to ICU data received" and a breakpoint otherwise). Link the
/// resources into the DLL's directory once, so the untouched download is a
/// working layout.
#[cfg(windows)]
fn ensure_flat_layout(paths: &RuntimePaths) -> Result<()> {
    link_tree(&paths.resources_dir, &paths.module_dir)
}

/// Every file under `source` appears under `target` (same relative path):
/// a hard link when the volume allows it, a copy otherwise; files already
/// there are left alone, so a second call is a cheap walk.
#[cfg_attr(not(windows), allow(dead_code))]
fn link_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)
        .map_err(|err| Error::new(format!("failed to create {}: {err}", target.display())))?;
    let entries = std::fs::read_dir(source)
        .map_err(|err| Error::new(format!("failed to read {}: {err}", source.display())))?;
    for entry in entries {
        let entry = entry
            .map_err(|err| Error::new(format!("failed to read {}: {err}", source.display())))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            link_tree(&from, &to)?;
            continue;
        }
        if to.exists() {
            continue;
        }
        if std::fs::hard_link(&from, &to).is_err() {
            std::fs::copy(&from, &to).map_err(|err| {
                Error::new(format!(
                    "failed to place {} at {}: {err}",
                    from.display(),
                    to.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn runtime() -> Result<&'static Runtime> {
    static RUNTIME: OnceLock<Result<Runtime>> = OnceLock::new();
    let runtime_result = RUNTIME.get_or_init(|| {
        let paths = runtime_paths();
        #[cfg(windows)]
        ensure_flat_layout(&paths)?;
        #[cfg(target_os = "macos")]
        let library = unsafe {
            libloading::os::unix::Library::open(
                Some(&paths.framework_bin),
                libloading::os::unix::RTLD_LAZY | libloading::os::unix::RTLD_LOCAL | RTLD_FIRST,
            )
            .map(Library::from)
        }
        .map_err(|err| {
            Error::new(format!(
                "failed to load {}: {err}",
                paths.framework_bin.display()
            ))
        })?;
        // libcef.dll imports chrome_elf.dll and friends from its own
        // directory, which is not the executable's: search the DLL's
        // directory as well as the defaults.
        #[cfg(windows)]
        let library = unsafe {
            libloading::os::windows::Library::load_with_flags(
                &paths.libcef,
                libloading::os::windows::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                    | libloading::os::windows::LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
            .map(Library::from)
        }
        .map_err(|err| Error::new(format!("failed to load {}: {err}", paths.libcef.display())))?;
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
                cef_string_list_size: load_symbol(&library, b"cef_string_list_size\0")?,
                cef_string_list_value: load_symbol(&library, b"cef_string_list_value\0")?,
            }
        };

        let hash = unsafe { (api.cef_api_hash)(CEF_API_VERSION, 0) };
        if hash.is_null() {
            return Err(Error::new("cef_api_hash returned a null pointer"));
        }

        Ok(Runtime {
            _library: library,
            api,
            _paths: paths,
            state: Mutex::new(RuntimeState::default()),
        })
    });
    runtime_result
        .as_ref()
        .map_err(|err| Error::new(err.to_string()))
}

#[cfg(target_os = "macos")]
fn objc_bool(value: bool) -> BOOL {
    if value {
        YES
    } else {
        NO
    }
}

#[cfg(target_os = "macos")]
extern "C" fn cef_is_handling_send_event(_this: &Object, _cmd: Sel) -> BOOL {
    objc_bool(CEF_HANDLING_SEND_EVENT.load(Ordering::Acquire))
}

#[cfg(target_os = "macos")]
extern "C" fn cef_set_handling_send_event(_this: &Object, _cmd: Sel, handling: BOOL) {
    CEF_HANDLING_SEND_EVENT.store(handling != NO, Ordering::Release);
}

#[cfg(target_os = "macos")]
extern "C" fn cef_application_send_event(this: &Object, _cmd: Sel, event: ObjcId) {
    let previous = CEF_HANDLING_SEND_EVENT.swap(true, Ordering::AcqRel);
    unsafe {
        let () = msg_send![super(this, class!(NSApplication)), sendEvent: event];
    };
    CEF_HANDLING_SEND_EVENT.store(previous, Ordering::Release);
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn ensure_cef_application_patch() -> Result<()> {
    static PATCH_RESULT: OnceLock<Result<()>> = OnceLock::new();
    PATCH_RESULT
        .get_or_init(|| unsafe {
            let app_class = ensure_cef_application_class()?;
            let ns_app: ObjcId = msg_send![app_class, sharedApplication];
            if ns_app.is_null() {
                return Err(Error::new(
                    "MakepadCefApplication sharedApplication returned null",
                ));
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

#[cfg(target_os = "macos")]
static CEF_HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
extern "C" fn external_pump_timer_fired(_this: &Object, _cmd: Sel, _timer: ObjcId) {
    if let Ok(pump) = external_pump() {
        pump.handle_timer_timeout();
    }
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn external_pump() -> Result<&'static ExternalPump> {
    static EXTERNAL_PUMP: OnceLock<Result<ExternalPump>> = OnceLock::new();
    let result = EXTERNAL_PUMP.get_or_init(|| unsafe {
        let pump_class = ensure_external_pump_class()?;
        let handler: ObjcId = msg_send![pump_class, new];
        if handler.is_null() {
            return Err(Error::new("MakepadCefMessagePumpTarget new returned null"));
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
    result.as_ref().map_err(|err| Error::new(err.to_string()))
}

#[cfg(target_os = "macos")]
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

#[cfg(unix)]
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

        for switch in chromium_switches() {
            let name = switch.split('=').next().unwrap_or(&switch);
            if !filtered_args
                .iter()
                .skip(1)
                .any(|arg| arg.to_string_lossy().starts_with(name))
            {
                filtered_args.push(switch.into());
            }
        }
        for extra in extra_chromium_switches() {
            filtered_args.push(extra.into());
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

#[cfg(windows)]
impl MainArgsStorage {
    fn current_process() -> Result<Self> {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
        }
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        if instance.is_null() {
            return Err(Error::new("GetModuleHandleW(NULL) returned null"));
        }
        Ok(Self {
            main_args: ffi::cef_main_args_t { instance },
        })
    }
}

/// The Chromium switches every browser process gets unless its command line
/// already carries them: the ANGLE backend (see `use_angle_backend`) and no
/// phoning home from an embedded page renderer — Chromium's push / GCM
/// registration, sync and component updates only litter the log
/// ("Registration response error message: PHONE_REGISTRATION_ERROR"). The
/// GCM client still registers with the background-networking switch on; its
/// endpoints are overridable, so they point at an unroutable local address
/// instead of Google.
fn chromium_switches() -> Vec<String> {
    let mut switches = Vec::new();
    if let Some(backend) = use_angle_backend() {
        switches.push(format!("--use-angle={backend}"));
    }
    for switch in [
        "--disable-background-networking",
        "--disable-sync",
        "--disable-component-update",
        "--disable-features=OptimizationHints,InterestFeedContentSuggestions",
        "--gcm-checkin-url=http://127.0.0.1:9/checkin",
        "--gcm-registration-url=http://127.0.0.1:9/register",
        "--gcm-mcs-endpoint=127.0.0.1:9",
    ] {
        switches.push(switch.to_string());
    }
    switches
}

/// The cheap platform setup that must precede the embedder's windows: on
/// macOS NSApp becomes the CEF-aware subclass and the external pump timer
/// exists; Windows needs nothing — the embedder's own pump timer drives
/// `cef_do_message_loop_work`.
#[cfg(target_os = "macos")]
fn platform_prepare() -> Result<()> {
    ensure_cef_application_patch()?;
    external_pump()?;
    env::set_var("MallocNanoZone", "0");
    Ok(())
}

#[cfg(windows)]
fn platform_prepare() -> Result<()> {
    Ok(())
}

/// CEF asked for `cef_do_message_loop_work` within `delay_ms`
/// (`on_schedule_message_pump_work`, and our own nudges after init and
/// browser creation).
#[cfg(target_os = "macos")]
fn schedule_pump_work(delay_ms: i64) {
    if let Ok(pump) = external_pump() {
        pump.schedule(delay_ms);
    }
}

/// No timer of its own on Windows: the embedder pumps at a fixed rate, at
/// least as often as CEF asks for.
#[cfg(windows)]
fn schedule_pump_work(_delay_ms: i64) {}

/// Whether browsers are created with `shared_texture_enabled`: requested and
/// a Metal blit device exists. Windows paints in software (`on_paint`); the
/// D3D11 shared-texture interop is not wired up.
#[cfg(target_os = "macos")]
fn accelerated_paint_available() -> bool {
    accelerated_paint_requested() && metal_blit().is_some()
}

#[cfg(windows)]
fn accelerated_paint_available() -> bool {
    false
}

/// The user's home: `HOME`, or `USERPROFILE` on Windows.
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Whether detailed CEF tracing is enabled.
fn cef_debug() -> bool {
    makepad_error_log::trace_enabled("cef")
}

fn env_flag(name: &str) -> Option<bool> {
    let value = env::var(name).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Whether browsers are created with `shared_texture_enabled` (GPU paint into
/// IOSurfaces). Default on; `MAKEPAD_CEF_SOFTWARE=1` forces the CPU
/// `on_paint` path (kept for A/B measurement).
pub fn accelerated_paint_requested() -> bool {
    !env_flag("MAKEPAD_CEF_SOFTWARE").unwrap_or(false)
}

/// `cef_browser_settings_t::background_color` for browsers created from now
/// on. Zero (the default) makes the windowless path composite its first
/// frames on BLACK — a page opens dark and jumps bright once the site paints,
/// and area the page has not laid out yet (mid-resize, before the reflow
/// lands) is black too. An embedder sets its theme colour here once, before
/// creating browsers, and gets a themed fill instead.
static BACKGROUND_COLOR: AtomicU32 = AtomicU32::new(0);

/// Set the opaque ARGB (0xAARRGGBB, alpha 0xFF) CEF fills unpainted page area
/// with. Takes effect for browsers created afterwards.
pub fn set_background_color(argb: u32) {
    BACKGROUND_COLOR.store(argb, Ordering::Release);
}

/// The colour `set_background_color` installed (0 = CEF's own default).
pub fn background_color() -> u32 {
    BACKGROUND_COLOR.load(Ordering::Acquire)
}

/// The `--use-angle=` value handed to Chromium, or `None` for Chromium's own
/// default (ANGLE over Metal on macOS).
///
/// Measured on CEF 138 / macOS 26 (Apple silicon) with windowless rendering
/// and shared textures, WebGL aquarium at 2560x1504 for 20 s, `top` samples:
/// `metal`   browser 5.8% / gpu-process 9.1%   (accelerated, 1000+ frames)
/// `gl`      browser 6.8% / gpu-process 15.7%  (accelerated, 1000+ frames)
/// `default` (= Metal on macOS) accelerated as well.
/// The earlier note that Chromium's Metal path crashed this embedding did
/// not reproduce; `metal` is the default, `MAKEPAD_CEF_USE_ANGLE=metal|gl|
/// default` overrides.
fn use_angle_backend() -> Option<String> {
    match env::var("MAKEPAD_CEF_USE_ANGLE") {
        Ok(value) => {
            let value = value.trim().to_string();
            if value.is_empty() || value == "default" || value == "none" {
                None
            } else {
                Some(value)
            }
        }
        Err(_) => DEFAULT_USE_ANGLE.map(|value| value.to_string()),
    }
}

/// Extra Chromium switches for experiments: `MAKEPAD_CEF_SWITCHES="--a --b=c"`.
fn extra_chromium_switches() -> Vec<String> {
    env::var("MAKEPAD_CEF_SWITCHES")
        .ok()
        .map(|value| {
            value
                .split_whitespace()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
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

    platform_prepare()?;

    let args = MainArgsStorage::current_process()?;
    let current_exe = env::current_exe()
        .map_err(|err| Error::new(format!("failed to resolve current executable: {err}")))?;
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);
    // A STABLE on-disk profile by default, so cookies/logins/localStorage
    // survive restarts (the per-pid temp dir made every launch incognito:
    // the Google cookie banner on each start). MAKEPAD_CEF_EPHEMERAL=1
    // restores the throwaway profile; MAKEPAD_CEF_PROFILE_DIR overrides
    // the location. Chromium locks the profile, so a second concurrent
    // instance of the same app should run ephemeral.
    let root_cache_path = profile_root_cache_path()?;
    // macOS: Chromium's subprocesses are the helper app inside the synthetic
    // bundle. Windows: this executable is its own subprocess — `bootstrap()`
    // runs `cef_execute_process` before anything else.
    #[cfg(target_os = "macos")]
    let (helper_executable, log_file) = {
        let synthetic_bundle =
            ensure_synthetic_app_bundle(&distribution_runtime_paths(), &current_exe)?;
        (
            synthetic_bundle.helper_executable.to_string_lossy().to_string(),
            synthetic_bundle.log_file.to_string_lossy().to_string(),
        )
    };
    #[cfg(windows)]
    let (helper_executable, log_file) = (
        current_exe.to_string_lossy().to_string(),
        root_cache_path.join("cef.log").to_string_lossy().to_string(),
    );
    // The resources sit beside libcef.dll (`ensure_flat_layout`); say so.
    #[cfg(windows)]
    let paths = runtime_paths();
    #[cfg(windows)]
    let resources_dir = CefString::new(&runtime.api, paths.module_dir.to_string_lossy())?;
    #[cfg(windows)]
    let locales_dir =
        CefString::new(&runtime.api, paths.module_dir.join("locales").to_string_lossy())?;
    #[cfg(windows)]
    let (resources_dir_path, locales_dir_path) = (resources_dir.raw(), locales_dir.raw());
    #[cfg(target_os = "macos")]
    let (resources_dir_path, locales_dir_path) =
        (ffi::cef_string_t::default(), ffi::cef_string_t::default());

    let browser_subprocess_path = CefString::new(&runtime.api, helper_executable)?;
    let log_file = CefString::new(&runtime.api, log_file)?;
    let cache_path = CefString::new(&runtime.api, root_cache_path.to_string_lossy())?;
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
        // cache_path == root_cache_path: a real persistent profile (an
        // empty cache_path would be incognito regardless of the root).
        cache_path: cache_path.raw(),
        root_cache_path: root_cache_path.raw(),
        persist_session_cookies: 1,
        user_agent: ffi::cef_string_t::default(),
        user_agent_product: ffi::cef_string_t::default(),
        locale: ffi::cef_string_t::default(),
        log_file: log_file.raw(),
        log_severity: ffi::LOGSEVERITY_INFO,
        log_items: ffi::LOG_ITEMS_DEFAULT,
        javascript_flags: ffi::cef_string_t::default(),
        resources_dir_path,
        locales_dir_path,
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
    schedule_pump_work(0);
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

/// The browser profile directory: stable across launches so cookies and
/// logins persist. `MAKEPAD_CEF_PROFILE_DIR` overrides; `MAKEPAD_CEF_
/// EPHEMERAL=1` (or no resolvable home) falls back to the per-pid temp
/// profile. Chromium locks a profile per process — if the stable dir is
/// locked by a live sibling (SingletonLock points at a running pid on this
/// host), fall back to ephemeral instead of failing CEF init.
fn profile_root_cache_path() -> Result<PathBuf> {
    if env::var("MAKEPAD_CEF_EPHEMERAL").is_ok() {
        return temp_root_cache_path();
    }
    if let Ok(dir) = env::var("MAKEPAD_CEF_PROFILE_DIR") {
        let path = PathBuf::from(dir);
        std::fs::create_dir_all(&path)
            .map_err(|err| Error::new(format!("failed to create {}: {err}", path.display())))?;
        return Ok(path);
    }
    let Some(home) = home_dir() else {
        return temp_root_cache_path();
    };
    let exe = env::current_exe()
        .map_err(|err| Error::new(format!("failed to resolve current executable: {err}")))?;
    let stem = exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("makepad-cef")
        .to_string();
    let path = home.join(".makepad-cef").join(&stem);
    std::fs::create_dir_all(&path)
        .map_err(|err| Error::new(format!("failed to create {}: {err}", path.display())))?;
    // Live-lock check: Chromium's SingletonLock is a symlink "host-pid"
    // (unix; Windows: `lockfile`, below).
    #[cfg(unix)]
    let lock = path.join("SingletonLock");
    #[cfg(unix)]
    if let Ok(target) = std::fs::read_link(&lock) {
        let alive = target
            .to_string_lossy()
            .rsplit('-')
            .next()
            .and_then(|pid| pid.parse::<i32>().ok())
            .map(|pid| unsafe { libc_kill_probe(pid) })
            .unwrap_or(false);
        if alive {
            return temp_root_cache_path();
        }
        // Stale lock from a dead process: clear it.
        let _ = std::fs::remove_file(&lock);
    }
    // Windows: Chromium's ProcessSingleton keeps `lockfile` open (delete-on-
    // close, no delete sharing) for the profile's lifetime, so it exists
    // only while a live sibling owns the profile — and cannot be removed
    // while it does. A stale one removes fine.
    #[cfg(windows)]
    {
        let lock = path.join("lockfile");
        if lock.exists() && std::fs::remove_file(&lock).is_err() {
            return temp_root_cache_path();
        }
    }
    Ok(path)
}

/// True when `pid` is alive (signal 0 probe).
#[cfg(unix)]
unsafe fn libc_kill_probe(pid: i32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, 0) == 0
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn write_if_changed(path: &PathBuf, content: &str) -> Result<()> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == content => return Ok(()),
        Ok(_) | Err(_) => {}
    }
    std::fs::write(path, content)
        .map_err(|err| Error::new(format!("failed to write {}: {err}", path.display())))
}

#[cfg(target_os = "macos")]
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
    // Warm start: the bundle already holds this exact build.
    if let (Ok(src), Ok(dst)) = (std::fs::metadata(source), std::fs::metadata(destination)) {
        if dst.is_file()
            && src.len() == dst.len()
            && src.modified().ok().is_some()
            && src.modified().ok() == dst.modified().ok()
        {
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

    // A real copy, not a hard link: exec'ing a hard link to a freshly built
    // binary got the process killed by taskgated ("Code Signature Invalid")
    // on its first launch.
    std::fs::copy(source, destination).map_err(|err| {
        Error::new(format!(
            "failed to copy {} to {}: {err}",
            source.display(),
            destination.display()
        ))
    })?;

    let metadata = std::fs::metadata(source)
        .map_err(|err| Error::new(format!("failed to stat {}: {err}", source.display())))?;
    std::fs::set_permissions(destination, metadata.permissions()).map_err(|err| {
        Error::new(format!(
            "failed to set permissions on {}: {err}",
            destination.display()
        ))
    })?;
    if let Ok(modified) = metadata.modified() {
        let _ = std::fs::File::options()
            .write(true)
            .open(destination)
            .and_then(|file| file.set_modified(modified));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn helper_bundle_info_plist(executable_name: &str, bundle_name: &str, bundle_id: &str) -> String {
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

#[cfg(target_os = "macos")]
fn ensure_framework_bundle_layout(
    source_framework_dir: &PathBuf,
    framework_dir: &PathBuf,
) -> Result<()> {
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

#[cfg(target_os = "macos")]
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
        _bundle_dir: bundle_dir,
        bundle_executable: main_bundle_executable,
        _framework_dir: framework_dir.clone(),
        _resources_dir: framework_dir.join("Resources"),
        helper_executable,
        log_file,
    })
}

#[cfg(target_os = "macos")]
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

/// Startup phases measured across the bundle re-exec: `(bundle_ms,
/// exec_gap_ms)` — time spent materialising the app bundle in the first
/// process, and wall time between its `exec` and this process reaching
/// `main` (the OS's first-launch verification of a new executable lands
/// here). `None` when not re-exec'd.
pub fn startup_phases() -> Option<(u128, u128)> {
    if !makepad_error_log::trace_enabled("startup") {
        return None;
    }
    let bundle_ms = env::var("MAKEPAD_CEF_BUNDLE_MS").ok()?.parse::<u128>().ok()?;
    let exec_at = env::var("MAKEPAD_CEF_EXEC_AT_MS").ok()?.parse::<u128>().ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(exec_at);
    Some((bundle_ms, now.saturating_sub(exec_at)))
}

#[cfg(target_os = "macos")]
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

    let startup_trace = makepad_error_log::trace_enabled("startup");
    let started = startup_trace.then(std::time::Instant::now);
    let synthetic_bundle =
        ensure_synthetic_app_bundle(&distribution_runtime_paths(), &current_executable)?;
    let mut command = std::process::Command::new(&synthetic_bundle.bundle_executable);
    command.args(env::args_os().skip(1));
    command.env("MAKEPAD_CEF_APP_BUNDLE_EXEC", "1");
    if let Some(started) = started {
        // Internal self-signals let the re-exec'd process report time spent
        // materialising and launching the synthetic bundle.
        let bundle_ms = started.elapsed().as_millis();
        let launch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        command.env("MAKEPAD_CEF_BUNDLE_MS", bundle_ms.to_string());
        command.env("MAKEPAD_CEF_EXEC_AT_MS", launch_ms.to_string());
    }
    command.env("MallocNanoZone", "0");
    let err = command.exec();
    Err(Error::new(format!(
        "failed to exec {}: {err}",
        synthetic_bundle.bundle_executable.display()
    )))
}

/// Nothing to re-exec into on Windows: Chromium's subprocesses are this
/// executable.
#[cfg(windows)]
pub fn reexec_into_app_bundle_if_needed() -> Result<()> {
    Ok(())
}

pub fn initialize() -> Result<()> {
    ensure_initialized()
}

/// The cheap part of CEF setup that must happen BEFORE the embedder creates
/// its NSApplication / windows: NSApp becomes the CEF-aware subclass and the
/// external message pump exists. The heavy `cef_initialize` (subprocess
/// spawn, GPU bring-up) is left to `initialize()` / the first `Browser::new`,
/// so a window can be on screen first.
pub fn prepare() -> Result<()> {
    platform_prepare()
}

/// True once `cef_initialize` has run.
pub fn is_initialized() -> bool {
    runtime()
        .map(|runtime| runtime.state.lock().unwrap().initialized)
        .unwrap_or(false)
}

impl SharedBrowserState {
    fn new(width: usize, height: usize, scale_factor: f32) -> Self {
        let scale_factor = scale_factor.max(0.1);
        Self {
            view: Mutex::new(ViewState {
                width: ((width.max(1) as f32 / scale_factor).round() as usize).max(1),
                height: ((height.max(1) as f32 / scale_factor).round() as usize).max(1),
                scale_factor,
            }),
            frame: Mutex::new(None),
            closing: AtomicBool::new(false),
            editable_focus: AtomicBool::new(false),
            ..Default::default()
        }
    }

    fn view(&self) -> ViewState {
        *self.view.lock().unwrap()
    }

    /// `width`/`height` are the PIXEL size of the surface; CEF's view rect is
    /// in device-independent points and it multiplies by
    /// `device_scale_factor` itself, so the stored view is the logical size.
    /// (Reporting pixels here made CEF paint at scale² — 5120x3008 for a
    /// 2560-pixel-wide view on a 2x display.)
    fn update_view(&self, width: usize, height: usize, scale_factor: f32) {
        let scale_factor = scale_factor.max(0.1);
        *self.view.lock().unwrap() = ViewState {
            width: ((width.max(1) as f32 / scale_factor).round() as usize).max(1),
            height: ((height.max(1) as f32 / scale_factor).round() as usize).max(1),
            scale_factor,
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
        self.editable_focus.store(editable_focus, Ordering::Release);
    }

    fn update_nav(&self, f: impl FnOnce(&mut NavState)) {
        {
            let mut nav = self.nav.lock().unwrap();
            f(&mut nav);
        }
        self.nav_gen.fetch_add(1, Ordering::AcqRel);
    }

    fn nav(&self) -> NavState {
        self.nav.lock().unwrap().clone()
    }
}

unsafe fn release_ref_counted(base: *mut ffi::cef_base_ref_counted_t) {
    if let Some(release) = (*base).release {
        release(base);
    }
}

/// Structs CEF passes INTO a callback arrive with one reference that belongs
/// to the callee (libcef_dll's `CppToC::Wrap` adds it "to be released once
/// the structure arrives on the other side"); every handler releases what it
/// receives. Structs we pass INTO CEF transfer our reference the same way.
unsafe fn release_param<T>(param: *mut T) {
    if !param.is_null() {
        release_ref_counted(param as *mut ffi::cef_base_ref_counted_t);
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
        if !(*client).display_handler.is_null() {
            release_ref_counted(
                &mut (*(*client).display_handler).cef_display_handler.base as *mut _,
            );
        }
        if !(*client).load_handler.is_null() {
            release_ref_counted(&mut (*(*client).load_handler).cef_load_handler.base as *mut _);
        }
        if !(*client).life_span_handler.is_null() {
            release_ref_counted(
                &mut (*(*client).life_span_handler).cef_life_span_handler.base as *mut _,
            );
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
    schedule_pump_work(delay_ms);
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
                &mut (*(*app).browser_process_handler)
                    .cef_browser_process_handler
                    .base as *mut _,
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

unsafe extern "system" fn app_on_before_command_line_processing(
    _self: *mut ffi::cef_app_t,
    process_type: *const ffi::cef_string_t,
    command_line: *mut ffi::cef_command_line_t,
) {
    if command_line.is_null() {
        return;
    }
    let Ok(runtime) = runtime() else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        let _ = process_type;
        let Ok(switch_name) = CefString::new(&runtime.api, MOCK_KEYCHAIN_SWITCH) else {
            return;
        };
        let switch_name_raw = switch_name.raw();
        if let Some(append_switch) = (*command_line).append_switch {
            append_switch(command_line, &switch_name_raw);
        }
    }
    // Windows CEF parses `GetCommandLineW` itself, so the switches unix
    // hands over in argv are appended here — browser process only (an empty
    // process type); Chromium forwards what its subprocesses need.
    #[cfg(windows)]
    {
        if !process_type.is_null() && (*process_type).length > 0 {
            return;
        }
        for switch in chromium_switches() {
            append_chromium_switch(&runtime.api, command_line, &switch, false);
        }
        for switch in extra_chromium_switches() {
            append_chromium_switch(&runtime.api, command_line, &switch, true);
        }
    }
}

/// `--name=value` → `("name", Some("value"))`, `--name` → `("name", None)`;
/// the first `=` splits, so a value may hold its own.
#[cfg_attr(not(windows), allow(dead_code))]
fn split_switch(switch: &str) -> (&str, Option<&str>) {
    let body = switch.trim_start_matches('-');
    match body.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (body, None),
    }
}

/// `--name` / `--name=value` onto a CEF command line; `replace` overrides a
/// switch already present (the defaults never do — the command line wins).
#[cfg(windows)]
unsafe fn append_chromium_switch(
    api: &CefApi,
    command_line: *mut ffi::cef_command_line_t,
    switch: &str,
    replace: bool,
) {
    let (name, value) = split_switch(switch);
    let Ok(name) = CefString::new(api, name) else {
        return;
    };
    let name_raw = name.raw();
    if !replace {
        if let Some(has_switch) = (*command_line).has_switch {
            if has_switch(command_line, &name_raw) != 0 {
                return;
            }
        }
    }
    match value {
        Some(value) => {
            let Ok(value) = CefString::new(api, value) else {
                return;
            };
            let value_raw = value.raw();
            if let Some(append_switch_with_value) = (*command_line).append_switch_with_value {
                append_switch_with_value(command_line, &name_raw, &value_raw);
            }
        }
        None => {
            if let Some(append_switch) = (*command_line).append_switch {
                append_switch(command_line, &name_raw);
            }
        }
    }
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

unsafe extern "system" fn client_get_display_handler(self_: *mut ffi::cef_client_t) -> *mut c_void {
    let client = self_ as *mut ClientHandler;
    add_ref_and_return((*client).display_handler) as *mut c_void
}

unsafe extern "system" fn client_get_load_handler(self_: *mut ffi::cef_client_t) -> *mut c_void {
    let client = self_ as *mut ClientHandler;
    add_ref_and_return((*client).load_handler) as *mut c_void
}

unsafe extern "system" fn client_get_life_span_handler(
    self_: *mut ffi::cef_client_t,
) -> *mut c_void {
    let client = self_ as *mut ClientHandler;
    add_ref_and_return((*client).life_span_handler) as *mut c_void
}

unsafe extern "system" fn render_get_root_screen_rect(
    self_: *mut ffi::cef_render_handler_t,
    browser: *mut ffi::cef_browser_t,
    rect: *mut ffi::cef_rect_t,
) -> c_int {
    release_param(browser);
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
    browser: *mut ffi::cef_browser_t,
    rect: *mut ffi::cef_rect_t,
) {
    release_param(browser);
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
    browser: *mut ffi::cef_browser_t,
    view_x: c_int,
    view_y: c_int,
    screen_x: *mut c_int,
    screen_y: *mut c_int,
) -> c_int {
    release_param(browser);
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
    browser: *mut ffi::cef_browser_t,
    screen_info: *mut ffi::cef_screen_info_t,
) -> c_int {
    release_param(browser);
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
    browser: *mut ffi::cef_browser_t,
    _show: c_int,
) {
    release_param(browser);
}

unsafe extern "system" fn render_on_popup_size(
    _self: *mut ffi::cef_render_handler_t,
    browser: *mut ffi::cef_browser_t,
    _rect: *const ffi::cef_rect_t,
) {
    release_param(browser);
}

unsafe extern "system" fn render_on_paint(
    self_: *mut ffi::cef_render_handler_t,
    browser: *mut ffi::cef_browser_t,
    type_: ffi::cef_paint_element_type_t,
    _dirty_rects_count: usize,
    _dirty_rects: *const ffi::cef_rect_t,
    buffer: *const c_void,
    width: c_int,
    height: c_int,
) {
    release_param(browser);
    if type_ != ffi::PET_VIEW || buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let render = self_ as *mut RenderHandler;
    let pixels =
        slice::from_raw_parts(buffer as *const u32, width as usize * height as usize).to_vec();
    let state: &SharedBrowserState = &(*render).state;
    let n = state.software_frames.fetch_add(1, Ordering::AcqRel) + 1;
    let milestone = matches!(n, 1 | 2 | 10 | 100 | 1000 | 10000 | 100000);
    let trace = cef_debug();
    if milestone || trace {
        let message = format!(
            "software paint #{n}: CPU buffer {width}x{height} (on_paint, {} bytes copied)",
            width as usize * height as usize * 4
        );
        if trace {
            makepad_error_log::trace!("cef", "{}", message);
        } else {
            eprintln!("[makepad-cef] {message}");
        }
    }
    state.set_frame(Frame {
        width: width as usize,
        height: height as usize,
        pixels,
    });
}

/// GPU path. CEF hands us an IOSurface out of its pool that is only valid
/// until we return, so the contents are copied (GPU blit, no CPU readback)
/// into the client-owned IOSurface the embedder registered with
/// `Browser::set_accelerated_target`. The blit is waited on: the pool may
/// hand the same surface to the next frame the moment this returns.
#[cfg(target_os = "macos")]
unsafe extern "system" fn render_on_accelerated_paint(
    self_: *mut ffi::cef_render_handler_t,
    browser: *mut ffi::cef_browser_t,
    type_: ffi::cef_paint_element_type_t,
    _dirty_rects_count: usize,
    _dirty_rects: *const ffi::cef_rect_t,
    info: *const c_void,
) {
    release_param(browser);
    if type_ != ffi::PET_VIEW || info.is_null() {
        return;
    }
    let info = &*(info as *const ffi::cef_accelerated_paint_info_t);
    let render = self_ as *mut RenderHandler;
    let state = &(*render).state;
    let iosurface = info.shared_texture_io_surface;
    if iosurface.is_null() {
        return;
    }
    let width = IOSurfaceGetWidth(iosurface);
    let height = IOSurfaceGetHeight(iosurface);
    if width == 0 || height == 0 {
        return;
    }

    let started = std::time::Instant::now();
    let mut blitted = false;
    let mut copied = (0usize, 0usize);
    if let Some(metal) = metal_blit() {
        let target = state.accel_target.lock().unwrap();
        if let Some(target) = target.as_ref() {
            // Blits need identical pixel formats on both sides, so the pool
            // surface is viewed with the destination's BGRA8 format. CEF on
            // macOS reports BGRA_8888; the format is recorded in the stats so
            // a mismatch is visible instead of silent.
            let source = metal_texture_for_iosurface(
                metal.device(),
                iosurface,
                width,
                height,
                MTL_PIXEL_FORMAT_BGRA8_UNORM,
            );
            if !source.is_null() {
                let copy_width = width.min(target.width) as u64;
                let copy_height = height.min(target.height) as u64;
                let command_buffer: ObjcId = msg_send![metal.queue(), commandBuffer];
                let encoder: ObjcId = msg_send![command_buffer, blitCommandEncoder];
                let origin = MTLOrigin { x: 0, y: 0, z: 0 };
                let size = MTLSize {
                    width: copy_width,
                    height: copy_height,
                    depth: 1,
                };
                let () = msg_send![
                    encoder,
                    copyFromTexture: source
                    sourceSlice: 0u64
                    sourceLevel: 0u64
                    sourceOrigin: origin
                    sourceSize: size
                    toTexture: target.texture as ObjcId
                    destinationSlice: 0u64
                    destinationLevel: 0u64
                    destinationOrigin: origin
                ];
                let () = msg_send![encoder, endEncoding];
                let () = msg_send![command_buffer, commit];
                let () = msg_send![command_buffer, waitUntilCompleted];
                let () = msg_send![source, release];
                copied = (copy_width as usize, copy_height as usize);
                blitted = true;
            }
        }
    }
    let micros = started.elapsed().as_micros() as u64;

    {
        let mut stats = state.accel.lock().unwrap();
        stats.frames += 1;
        stats.last_width = width;
        stats.last_height = height;
        stats.last_format = info.format;
        if blitted {
            stats.last_blit_micros = micros;
            stats.total_blit_micros += micros;
            stats.target_frames += 1;
            stats.last_copy_width = copied.0;
            stats.last_copy_height = copied.1;
        } else {
            stats.dropped_no_target += 1;
        }
        // Measured evidence of the GPU path: frames 1, 2, 10, 100, 1000... are
        // logged unconditionally (every frame with the `cef` topic).
        let milestone = matches!(stats.frames, 1 | 2 | 10 | 100 | 1000 | 10000 | 100000);
        let trace = cef_debug();
        if milestone || trace {
            let message = format!(
                "accelerated paint #{}: IOSurface {}x{} format={} ({}) iosurface_pixel_format=0x{:08x} coded_size={}x{} visible_rect={:?} blit={}us (copied={}, no-target drops={})",
                stats.frames,
                width,
                height,
                info.format,
                if info.format == ffi::CEF_COLOR_TYPE_BGRA_8888 { "BGRA_8888" } else { "RGBA_8888" },
                IOSurfaceGetPixelFormat(iosurface),
                info.extra.coded_size.width,
                info.extra.coded_size.height,
                (
                    info.extra.visible_rect.x,
                    info.extra.visible_rect.y,
                    info.extra.visible_rect.width,
                    info.extra.visible_rect.height
                ),
                micros,
                blitted,
                stats.dropped_no_target
            );
            if trace {
                makepad_error_log::trace!("cef", "{}", message);
            } else {
                eprintln!("[makepad-cef] {message}");
            }
        }
    }
    if blitted {
        state.accel_frames.fetch_add(1, Ordering::AcqRel);
    }
}

unsafe extern "system" fn display_on_address_change(
    self_: *mut ffi::cef_display_handler_t,
    browser: *mut ffi::cef_browser_t,
    frame: *mut ffi::cef_frame_t,
    url: *const ffi::cef_string_t,
) {
    release_param(browser);
    let mut is_main_frame = true;
    if !frame.is_null() {
        if let Some(is_main) = (*frame).is_main {
            is_main_frame = is_main(frame) != 0;
        }
    }
    release_param(frame);
    if !is_main_frame {
        return;
    }
    let handler = self_ as *mut DisplayHandler;
    let url = cef_string_to_string(url);
    (*handler).state.update_nav(|nav| nav.url = url);
}

unsafe extern "system" fn display_on_title_change(
    self_: *mut ffi::cef_display_handler_t,
    browser: *mut ffi::cef_browser_t,
    title: *const ffi::cef_string_t,
) {
    release_param(browser);
    let handler = self_ as *mut DisplayHandler;
    let title = cef_string_to_string(title);
    (*handler).state.update_nav(|nav| nav.title = title);
}

unsafe extern "system" fn display_on_favicon_urlchange(
    self_: *mut ffi::cef_display_handler_t,
    browser: *mut ffi::cef_browser_t,
    icon_urls: ffi::cef_string_list_t,
) {
    display_on_favicon_urlchange_inner(self_, browser, icon_urls);
    release_param(browser);
}

unsafe fn display_on_favicon_urlchange_inner(
    self_: *mut ffi::cef_display_handler_t,
    browser: *mut ffi::cef_browser_t,
    icon_urls: ffi::cef_string_list_t,
) {
    let handler = self_ as *mut DisplayHandler;
    let Ok(runtime) = runtime() else {
        return;
    };
    let state = &(*handler).state;
    let mut first_url = None;
    if !icon_urls.is_null() {
        let count = (runtime.api.cef_string_list_size)(icon_urls);
        if count > 0 {
            let mut value = ffi::cef_string_t::default();
            if (runtime.api.cef_string_list_value)(icon_urls, 0, &mut value) != 0 {
                first_url = Some(cef_string_to_string(&value));
                (runtime.api.cef_string_utf16_clear)(&mut value);
            }
        }
    }
    if cef_debug() {
        makepad_error_log::trace!("cef", "favicon urls changed: {:?}", first_url);
    }
    let Some(url) = first_url else {
        *state.favicon_url.lock().unwrap() = String::new();
        *state.favicon.lock().unwrap() = None;
        state.nav_gen.fetch_add(1, Ordering::AcqRel);
        return;
    };
    if *state.favicon_url.lock().unwrap() == url {
        return;
    }
    *state.favicon_url.lock().unwrap() = url.clone();

    if browser.is_null() {
        return;
    }
    let Some(get_host) = (*browser).get_host else {
        return;
    };
    let host = get_host(browser);
    if host.is_null() {
        return;
    }
    if let Some(download_image) = (*host).download_image {
        if let Ok(url) = CefString::new(&runtime.api, &url) {
            let callback = DownloadImageCallback::allocate(state.clone());
            // Our one reference transfers to CEF with the call.
            // Plain image download: the `is_favicon` request path returns no
            // bitmaps in this embedding (no favicon service without a
            // profile), the normal fetch does.
            download_image(
                host,
                &url.value,
                0,
                FAVICON_MAX_SIZE,
                0,
                &mut (*callback).cef_callback,
            );
        }
    }
    release_ref_counted(&mut (*host).base as *mut _);
}

unsafe extern "system" fn download_image_on_finished(
    self_: *mut ffi::cef_download_image_callback_t,
    image_url: *const ffi::cef_string_t,
    http_status_code: c_int,
    image: *mut ffi::cef_image_t,
) {
    if cef_debug() {
        makepad_error_log::trace!(
            "cef",
            "favicon download {} -> http {}",
            cef_string_to_string(image_url),
            http_status_code
        );
    }
    download_image_on_finished_inner(self_, image);
    release_param(image);
}

unsafe fn download_image_on_finished_inner(
    self_: *mut ffi::cef_download_image_callback_t,
    image: *mut ffi::cef_image_t,
) {
    let callback = self_ as *mut DownloadImageCallback;
    let state = &(*callback).state;
    if cef_debug() {
        let empty = if image.is_null() {
            true
        } else {
            (*image).is_empty.map(|f| f(image) != 0).unwrap_or(true)
        };
        makepad_error_log::trace!("cef", "favicon download finished: image null={} empty={}", image.is_null(), empty);
    }
    if image.is_null() {
        return;
    }
    if let Some(is_empty) = (*image).is_empty {
        if is_empty(image) != 0 {
            return;
        }
    }
    let Some(get_as_bitmap) = (*image).get_as_bitmap else {
        return;
    };
    let mut width: c_int = 0;
    let mut height: c_int = 0;
    let bitmap = get_as_bitmap(
        image,
        1.0,
        ffi::CEF_COLOR_TYPE_BGRA_8888,
        ffi::CEF_ALPHA_TYPE_PREMULTIPLIED,
        &mut width,
        &mut height,
    );
    if bitmap.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let expected = width as usize * height as usize * 4;
    let mut bytes = vec![0u8; expected];
    let mut ok = false;
    if let (Some(get_size), Some(get_data)) = ((*bitmap).get_size, (*bitmap).get_data) {
        if get_size(bitmap) >= expected {
            let copied = get_data(bitmap, bytes.as_mut_ptr() as *mut c_void, expected, 0);
            ok = copied == expected;
        }
    }
    release_ref_counted(&mut (*bitmap).base as *mut _);
    if !ok {
        return;
    }
    let pixels = bytes
        .chunks_exact(4)
        .map(|px| u32::from_le_bytes([px[0], px[1], px[2], px[3]]))
        .collect::<Vec<u32>>();
    if cef_debug() {
        makepad_error_log::trace!("cef", "favicon bitmap {}x{}", width, height);
    }
    *state.favicon.lock().unwrap() = Some(Frame {
        width: width as usize,
        height: height as usize,
        pixels,
    });
    state.nav_gen.fetch_add(1, Ordering::AcqRel);
}

unsafe extern "system" fn load_on_loading_state_change(
    self_: *mut ffi::cef_load_handler_t,
    browser: *mut ffi::cef_browser_t,
    is_loading: c_int,
    can_go_back: c_int,
    can_go_forward: c_int,
) {
    release_param(browser);
    let handler = self_ as *mut LoadHandler;
    (*handler).state.update_nav(|nav| {
        nav.loading = is_loading != 0;
        nav.can_go_back = can_go_back != 0;
        nav.can_go_forward = can_go_forward != 0;
    });
}

/// Popups (`window.open`, target=_blank) never become native windows here:
/// the request is cancelled and its URL queued for the embedder to open as a
/// tab.
unsafe extern "system" fn life_span_on_before_popup(
    self_: *mut ffi::cef_life_span_handler_t,
    browser: *mut ffi::cef_browser_t,
    frame: *mut ffi::cef_frame_t,
    _popup_id: c_int,
    target_url: *const ffi::cef_string_t,
    _target_frame_name: *const ffi::cef_string_t,
    _target_disposition: ffi::cef_window_open_disposition_t,
    _user_gesture: c_int,
    _popup_features: *const ffi::cef_popup_features_t,
    _window_info: *mut ffi::cef_window_info_t,
    _client: *mut *mut ffi::cef_client_t,
    _settings: *mut ffi::cef_browser_settings_t,
    _extra_info: *mut *mut ffi::cef_dictionary_value_t,
    _no_javascript_access: *mut c_int,
) -> c_int {
    release_param(browser);
    release_param(frame);
    let handler = self_ as *mut LifeSpanHandler;
    let url = cef_string_to_string(target_url);
    if !url.is_empty() {
        let state: &SharedBrowserState = &(*handler).state;
        state.popup_requests.lock().unwrap().push(url);
        state.nav_gen.fetch_add(1, Ordering::AcqRel);
    }
    1
}

impl DisplayHandler {
    fn allocate(state: Arc<SharedBrowserState>) -> *mut DisplayHandler {
        Box::into_raw(Box::new(Self {
            cef_display_handler: ffi::cef_display_handler_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_display_handler_t>(),
                    add_ref: Some(display_add_ref),
                    release: Some(display_release),
                    has_one_ref: Some(display_has_one_ref),
                    has_at_least_one_ref: Some(display_has_at_least_one_ref),
                },
                on_address_change: Some(display_on_address_change),
                on_title_change: Some(display_on_title_change),
                on_favicon_urlchange: Some(display_on_favicon_urlchange),
                on_fullscreen_mode_change: None,
                on_tooltip: None,
                on_status_message: None,
                on_console_message: None,
                on_auto_resize: None,
                on_loading_progress_change: None,
                on_cursor_change: None,
                on_media_access_change: None,
                on_contents_bounds_change: None,
                get_root_window_screen_rect: None,
            },
            ref_count: AtomicUsize::new(1),
            state,
        }))
    }
}

impl LoadHandler {
    fn allocate(state: Arc<SharedBrowserState>) -> *mut LoadHandler {
        Box::into_raw(Box::new(Self {
            cef_load_handler: ffi::cef_load_handler_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_load_handler_t>(),
                    add_ref: Some(load_add_ref),
                    release: Some(load_release),
                    has_one_ref: Some(load_has_one_ref),
                    has_at_least_one_ref: Some(load_has_at_least_one_ref),
                },
                on_loading_state_change: Some(load_on_loading_state_change),
                on_load_start: None,
                on_load_end: None,
                on_load_error: None,
            },
            ref_count: AtomicUsize::new(1),
            state,
        }))
    }
}

impl LifeSpanHandler {
    fn allocate(state: Arc<SharedBrowserState>) -> *mut LifeSpanHandler {
        Box::into_raw(Box::new(Self {
            cef_life_span_handler: ffi::cef_life_span_handler_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_life_span_handler_t>(),
                    add_ref: Some(life_span_add_ref),
                    release: Some(life_span_release),
                    has_one_ref: Some(life_span_has_one_ref),
                    has_at_least_one_ref: Some(life_span_has_at_least_one_ref),
                },
                on_before_popup: Some(life_span_on_before_popup),
                on_before_popup_aborted: None,
                on_before_dev_tools_popup: None,
                on_after_created: None,
                do_close: None,
                on_before_close: None,
            },
            ref_count: AtomicUsize::new(1),
            state,
        }))
    }
}

impl DownloadImageCallback {
    fn allocate(state: Arc<SharedBrowserState>) -> *mut DownloadImageCallback {
        Box::into_raw(Box::new(Self {
            cef_callback: ffi::cef_download_image_callback_t {
                base: ffi::cef_base_ref_counted_t {
                    size: std::mem::size_of::<ffi::cef_download_image_callback_t>(),
                    add_ref: Some(download_image_add_ref),
                    release: Some(download_image_release),
                    has_one_ref: Some(download_image_has_one_ref),
                    has_at_least_one_ref: Some(download_image_has_at_least_one_ref),
                },
                on_download_image_finished: Some(download_image_on_finished),
            },
            ref_count: AtomicUsize::new(1),
            state,
        }))
    }
}

unsafe extern "system" fn render_on_virtual_keyboard_requested(
    self_: *mut ffi::cef_render_handler_t,
    browser: *mut ffi::cef_browser_t,
    input_mode: ffi::cef_text_input_mode_t,
) {
    release_param(browser);
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
                #[cfg(target_os = "macos")]
                on_accelerated_paint: Some(render_on_accelerated_paint),
                #[cfg(not(target_os = "macos"))]
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
    fn allocate(
        render_handler: *mut RenderHandler,
        display_handler: *mut DisplayHandler,
        load_handler: *mut LoadHandler,
        life_span_handler: *mut LifeSpanHandler,
    ) -> *mut ClientHandler {
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
                get_display_handler: Some(client_get_display_handler),
                get_download_handler: Some(client_null_handler),
                get_drag_handler: Some(client_null_handler),
                get_find_handler: Some(client_null_handler),
                get_focus_handler: Some(client_null_handler),
                get_frame_handler: Some(client_null_handler),
                get_permission_handler: Some(client_null_handler),
                get_jsdialog_handler: Some(client_null_handler),
                get_keyboard_handler: Some(client_null_handler),
                get_life_span_handler: Some(client_get_life_span_handler),
                get_load_handler: Some(client_get_load_handler),
                get_print_handler: Some(client_null_handler),
                get_render_handler: Some(client_get_render_handler),
                get_request_handler: Some(client_null_handler),
                on_process_message_received: None,
            },
            ref_count: AtomicUsize::new(1),
            render_handler,
            display_handler,
            load_handler,
            life_span_handler,
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
                on_schedule_message_pump_work: Some(browser_process_on_schedule_message_pump_work),
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
                on_before_command_line_processing: Some(app_on_before_command_line_processing),
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
    fn with_host<T>(&self, f: impl FnOnce(*mut ffi::cef_browser_host_t) -> Result<T>) -> Result<T> {
        unsafe {
            let host = (*self.browser)
                .get_host
                .ok_or_else(|| Error::new("cef_browser_t::get_host missing"))?(
                self.browser
            );
            if host.is_null() {
                return Err(Error::new("cef_browser_t::get_host returned null"));
            }
            let result = f(host);
            release_ref_counted(&mut (*host).base as *mut _);
            result
        }
    }

    fn with_main_frame<T>(&self, f: impl FnOnce(*mut ffi::cef_frame_t) -> Result<T>) -> Result<T> {
        unsafe {
            let frame = (*self.browser)
                .get_main_frame
                .ok_or_else(|| Error::new("cef_browser_t::get_main_frame missing"))?(
                self.browser
            );
            if frame.is_null() {
                return Err(Error::new("cef_browser_t::get_main_frame returned null"));
            }
            let result = f(frame);
            release_ref_counted(&mut (*frame).base as *mut _);
            result
        }
    }

    /// `width`/`height` are the PIXEL size of the surface the page is drawn
    /// into; `scale_factor` is the display scale. CEF works in points: the
    /// view rect it sees is `width/scale x height/scale`, and it paints
    /// `width x height` pixels. Mouse coordinates handed to `send_mouse_*`
    /// are in points as well.
    pub fn new(url: &str, width: usize, height: usize, scale_factor: f32) -> Result<Self> {
        ensure_initialized()?;
        let runtime = runtime()?;

        let state = Arc::new(SharedBrowserState::new(width, height, scale_factor));
        let logical = state.view();
        let render_handler = RenderHandler::allocate(state.clone());
        let display_handler = DisplayHandler::allocate(state.clone());
        let load_handler = LoadHandler::allocate(state.clone());
        let life_span_handler = LifeSpanHandler::allocate(state.clone());
        let client = ClientHandler::allocate(
            render_handler,
            display_handler,
            load_handler,
            life_span_handler,
        );
        let accelerated = accelerated_paint_available();

        let url = CefString::new(
            &runtime.api,
            if url.is_empty() { "about:blank" } else { url },
        )?;
        let bounds = ffi::cef_rect_t {
            x: 0,
            y: 0,
            width: logical.width as c_int,
            height: logical.height as c_int,
        };
        #[cfg(target_os = "macos")]
        let mut window_info = ffi::cef_window_info_t {
            size: std::mem::size_of::<ffi::cef_window_info_t>(),
            window_name: ffi::cef_string_t::default(),
            bounds,
            hidden: 0,
            parent_view: ptr::null_mut(),
            windowless_rendering_enabled: 1,
            shared_texture_enabled: accelerated as c_int,
            external_begin_frame_enabled: 0,
            view: ptr::null_mut(),
            runtime_style: 0,
        };
        #[cfg(windows)]
        let mut window_info = ffi::cef_window_info_t {
            size: std::mem::size_of::<ffi::cef_window_info_t>(),
            ex_style: 0,
            window_name: ffi::cef_string_t::default(),
            style: 0,
            bounds,
            parent_window: ptr::null_mut(),
            menu: ptr::null_mut(),
            windowless_rendering_enabled: 1,
            shared_texture_enabled: accelerated as c_int,
            external_begin_frame_enabled: 0,
            window: ptr::null_mut(),
            runtime_style: 0,
        };
        let mut browser_settings = ffi::cef_browser_settings_t {
            size: std::mem::size_of::<ffi::cef_browser_settings_t>(),
            windowless_frame_rate: 60,
            // Themed, not black: what the page area shows before the site
            // has composited anything, and behind area a reflow has not
            // reached yet.
            background_color: background_color(),
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
            accelerated,
            hidden: false,
        };
        let _ = this.resize(width, height, scale_factor);
        schedule_pump_work(0);
        Ok(this)
    }

    /// True when this browser was created with `shared_texture_enabled`, i.e.
    /// frames arrive through `on_accelerated_paint` as IOSurfaces.
    pub fn is_accelerated(&self) -> bool {
        self.accelerated
    }

    /// Which path has actually delivered frames so far.
    pub fn render_mode(&self) -> RenderMode {
        let accel = self.state.accel.lock().unwrap().frames;
        let software = self.state.software_frames.load(Ordering::Acquire);
        if accel > 0 {
            RenderMode::Accelerated
        } else if software > 0 {
            RenderMode::Software
        } else {
            RenderMode::None
        }
    }

    pub fn accelerated_stats(&self) -> AcceleratedStats {
        let stats = *self.state.accel.lock().unwrap();
        AcceleratedStats {
            frames: stats.frames,
            last_width: stats.last_width,
            last_height: stats.last_height,
            last_format: stats.last_format,
            last_blit_micros: stats.last_blit_micros,
            total_blit_micros: stats.total_blit_micros,
            dropped_no_target: stats.dropped_no_target,
            target_frames: stats.target_frames,
            last_copy_width: stats.last_copy_width,
            last_copy_height: stats.last_copy_height,
        }
    }

    /// Register the client-owned IOSurface that accelerated paints are copied
    /// into. `iosurface` is an `IOSurfaceRef` the embedder keeps alive (it is
    /// retained here as well); its pixel format must be BGRA8.
    ///
    /// The surface may be LARGER than the browser view — the blit fills its
    /// top-left `min(paint, target)` corner and `AcceleratedStats::
    /// last_copy_*` says how much of it holds page pixels. Over-allocating is
    /// how an embedder survives a drag-resize without a new surface (and a
    /// blank frame) at every step. A newly registered surface starts blank:
    /// `target_frames` counts blits into it, so keep showing the previous one
    /// until it is non-zero.
    #[cfg(target_os = "macos")]
    pub fn set_accelerated_target(
        &mut self,
        iosurface: *mut c_void,
        width: usize,
        height: usize,
    ) -> Result<()> {
        if iosurface.is_null() || width == 0 || height == 0 {
            *self.state.accel_target.lock().unwrap() = None;
            return Ok(());
        }
        let metal = metal_blit().ok_or_else(|| Error::new("no Metal device for CEF blits"))?;
        {
            let current = self.state.accel_target.lock().unwrap();
            if let Some(current) = current.as_ref() {
                if current.iosurface == iosurface as usize
                    && current.width == width
                    && current.height == height
                {
                    return Ok(());
                }
            }
        }
        let texture = unsafe {
            metal_texture_for_iosurface(
                metal.device(),
                iosurface,
                width,
                height,
                MTL_PIXEL_FORMAT_BGRA8_UNORM,
            )
        };
        if texture.is_null() {
            return Err(Error::new(format!(
                "failed to wrap the {width}x{height} target IOSurface in a Metal texture"
            )));
        }
        unsafe {
            CFRetain(iosurface as *const c_void);
        }
        // A brand-new surface holds nothing yet; the fill counter starts over
        // so the embedder can tell "blank" from "painted".
        {
            let mut stats = self.state.accel.lock().unwrap();
            stats.target_frames = 0;
            stats.last_copy_width = 0;
            stats.last_copy_height = 0;
        }
        *self.state.accel_target.lock().unwrap() = Some(AccelTarget {
            iosurface: iosurface as usize,
            texture: texture as usize,
            width,
            height,
        });
        // The page may be idle: ask for a repaint so the new surface fills.
        let _ = self.with_host(|host| unsafe {
            if let Some(invalidate) = (*host).invalidate {
                invalidate(host, ffi::PET_VIEW);
            }
            Ok(())
        });
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn clear_accelerated_target(&mut self) {
        *self.state.accel_target.lock().unwrap() = None;
    }

    /// Monotonic count of accelerated frames copied into the target; the
    /// embedder redraws when it changes.
    pub fn accelerated_frame_counter(&self) -> u64 {
        self.state.accel_frames.load(Ordering::Acquire)
    }

    /// Bumps whenever title / url / loading state / favicon / popup queue
    /// change.
    pub fn nav_generation(&self) -> u64 {
        self.state.nav_gen.load(Ordering::Acquire)
    }

    pub fn title(&self) -> String {
        self.state.nav().title
    }

    pub fn url(&self) -> String {
        self.state.nav().url
    }

    pub fn is_loading(&self) -> bool {
        self.state.nav().loading
    }

    pub fn can_go_back(&self) -> bool {
        self.state.nav().can_go_back
    }

    pub fn can_go_forward(&self) -> bool {
        self.state.nav().can_go_forward
    }

    pub fn go_back(&mut self) -> Result<()> {
        unsafe {
            if let Some(go_back) = (*self.browser).go_back {
                go_back(self.browser);
            }
        }
        Ok(())
    }

    pub fn go_forward(&mut self) -> Result<()> {
        unsafe {
            if let Some(go_forward) = (*self.browser).go_forward {
                go_forward(self.browser);
            }
        }
        Ok(())
    }

    pub fn reload(&mut self) -> Result<()> {
        unsafe {
            if let Some(reload) = (*self.browser).reload {
                reload(self.browser);
            }
        }
        Ok(())
    }

    pub fn stop_load(&mut self) -> Result<()> {
        unsafe {
            if let Some(stop_load) = (*self.browser).stop_load {
                stop_load(self.browser);
            }
        }
        Ok(())
    }

    /// Hidden browsers (background tabs) stop painting until shown again.
    pub fn set_hidden(&mut self, hidden: bool) -> Result<()> {
        if self.hidden == hidden {
            return Ok(());
        }
        self.hidden = hidden;
        self.with_host(|host| unsafe {
            if let Some(was_hidden) = (*host).was_hidden {
                was_hidden(host, hidden as c_int);
            }
            if !hidden {
                if let Some(invalidate) = (*host).invalidate {
                    invalidate(host, ffi::PET_VIEW);
                }
            }
            Ok(())
        })
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// URLs the page asked to open in a new window (cancelled as popups).
    pub fn take_popup_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.state.popup_requests.lock().unwrap())
    }

    /// The latest favicon bitmap (BGRA8 premultiplied), once per change.
    pub fn take_favicon(&mut self) -> Option<Frame> {
        self.state.favicon.lock().unwrap().take()
    }

    pub fn resize(&mut self, width: usize, height: usize, scale_factor: f32) -> Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        let scale_factor = scale_factor.max(0.1);
        let scale_changed = (self.scale_factor - scale_factor).abs() >= f32::EPSILON;
        if self.width == width && self.height == height && !scale_changed {
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
                .ok_or_else(|| Error::new("cef_frame_t::load_url missing"))?(
                frame, &url.value
            );
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
                host, &event, delta_x, delta_y,
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
                host, &event,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_switch_separates_name_and_value_at_the_first_equals() {
        assert_eq!(split_switch("--disable-sync"), ("disable-sync", None));
        assert_eq!(
            split_switch("--gcm-checkin-url=http://127.0.0.1:9/checkin"),
            ("gcm-checkin-url", Some("http://127.0.0.1:9/checkin"))
        );
        assert_eq!(
            split_switch("--disable-features=A,B=c"),
            ("disable-features", Some("A,B=c"))
        );
        assert_eq!(split_switch("bare"), ("bare", None));
    }

    #[test]
    fn chromium_switches_keep_the_renderer_off_the_network() {
        let switches = chromium_switches();
        for expected in [
            "--disable-background-networking",
            "--disable-sync",
            "--disable-component-update",
            "--gcm-mcs-endpoint=127.0.0.1:9",
        ] {
            assert!(switches.iter().any(|s| s == expected), "missing {expected}");
        }
        // Every entry is a well-formed `--switch[=value]`.
        for switch in &switches {
            assert!(switch.starts_with("--"), "{switch}");
            let (name, _) = split_switch(switch);
            assert!(!name.is_empty() && !name.contains('='), "{switch}");
        }
        // The ANGLE backend is a switch exactly when one is configured.
        let angle = switches.iter().filter(|s| s.starts_with("--use-angle=")).count();
        assert_eq!(angle, usize::from(use_angle_backend().is_some()));
    }

    #[test]
    fn link_tree_places_every_file_and_is_idempotent() {
        let root = env::temp_dir().join(format!("makepad-cef-link-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("Resources");
        let target = root.join("Release");
        std::fs::create_dir_all(source.join("locales")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(source.join("icudtl.dat"), b"icu").unwrap();
        std::fs::write(source.join("locales").join("en-US.pak"), b"en").unwrap();
        std::fs::write(target.join("libcef.dll"), b"dll").unwrap();

        link_tree(&source, &target).unwrap();
        assert_eq!(std::fs::read(target.join("icudtl.dat")).unwrap(), b"icu");
        assert_eq!(
            std::fs::read(target.join("locales").join("en-US.pak")).unwrap(),
            b"en"
        );
        assert_eq!(std::fs::read(target.join("libcef.dll")).unwrap(), b"dll");

        // A second walk changes nothing and touches nothing already there.
        std::fs::write(target.join("icudtl.dat"), b"kept").unwrap();
        link_tree(&source, &target).unwrap();
        assert_eq!(std::fs::read(target.join("icudtl.dat")).unwrap(), b"kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn api_version_is_pinned_to_the_ffi_layout() {
        // `ffi.rs` is the 138 layout; a newer libcef serves it when asked.
        assert_eq!(CEF_API_VERSION, 13800);
    }
}
