use {
    crate::{
        action::ActionsBuf,
        action::{ActionSend, ACTION_SENDER_GLOBAL},
        area::Area,
        component::ComponentRegistries,
        cx_api::CxOsOp,
        debug::Debug,
        display_context::DisplayContext,
        draw_list::CxDrawListPool,
        draw_matrix::CxDrawMatrixPool,
        draw_pass::CxDrawPassPool,
        draw_shader::CxDrawShaders,
        event::{
            CxDragDrop, CxFingers, CxKeyboard, DrawEvent, Event, NextFrame, Trigger,
            WindowGeomChangeEvent,
        },
        file_dialogs::FileDialogState,
        geometry::CxGeometryPool,
        gpu_info::GpuInfo,
        os::CxOs,
        perf_monitor::PerfMonitor,
        performance_stats::PerformanceStats,
        script::script::CxScriptData,
        sploded::SplodedView,
        storage::StorageState,
        texture::{CxTexturePool, Texture, TextureFormat, TextureUpdated},
        thread::{SignalToUI, ToUIReceiver},
        uniform_buffer::CxUniformBufferPool,
        window::CxWindowPool,
    },
    makepad_futures::{
        executor,
        executor::{Executor, Spawner},
    },
    makepad_network::NetworkRuntime,
    makepad_script::*,
    makepad_studio_protocol::{
        RunViewFrameData, RunViewFrameRequest, ScreenshotRequest, WidgetSnapshot,
    },
    std::{
        any::{Any, TypeId},
        cell::{Cell, RefCell, UnsafeCell},
        collections::{HashMap, HashSet, VecDeque},
        rc::Rc,
        sync::Arc,
    },
};

//pub use makepad_shader_compiler::makepad_derive_live::*;
//pub use makepad_shader_compiler::makepad_math::*;

pub(crate) struct PendingCameraPlayback {
    pub permission: crate::permission::Permission,
    pub video_id: LiveId,
    pub source: crate::event::VideoSource,
    pub camera_preview_mode: crate::event::video_playback::CameraPreviewMode,
    pub external_texture_id: u32,
    pub texture_id: crate::texture::TextureId,
    pub autoplay: bool,
    pub should_loop: bool,
}

pub struct Cx {
    pub script_vm: Option<Box<ScriptVmBase>>,
    pub script_data: CxScriptData,
    pub package_root: Option<String>,
    pub(crate) font_set: crate::font_policy::FontSet,
    pub(crate) font_set_frozen: bool,

    pub debug_trace_active: bool,

    pub(crate) os_type: OsType,
    pub in_makepad_studio: bool,
    /// Game controllers forwarded by Studio. An app hosted by Studio is a
    /// child process with no window, so the OS hands controller input to
    /// Studio instead; this is where that arrives, and it is what
    /// `game_input_states` reads while `in_makepad_studio` is set.
    pub(crate) game_input_remote: Vec<crate::event::game_input::GameInputState>,
    pub demo_time_repaint: bool,
    pub(crate) gpu_info: GpuInfo,
    pub(crate) xr_capabilities: XrCapabilities,
    pub(crate) cpu_cores: usize,
    /// Process memory envelope available to subsystems with large, elastic
    /// caches. Web startup replaces the native default with the shared wasm
    /// memory limit reported by the JS bridge.
    pub(crate) memory_budget_bytes: usize,
    pub(crate) memory_budget_initialized: bool,
    pub(crate) thread_spawner: crate::thread::ThreadSpawner,
    /// The runtime's warm background executor; see `Cx::task_pool`.
    pub(crate) task_pool: std::cell::OnceCell<crate::thread::TaskPool>,
    pub null_texture: Texture,
    pub null_cube_texture: Texture,
    pub windows: CxWindowPool,
    pub passes: CxDrawPassPool,
    pub draw_lists: CxDrawListPool,
    pub draw_matrices: CxDrawMatrixPool,
    pub textures: CxTexturePool,
    pub uniform_buffers: CxUniformBufferPool,
    pub(crate) geometries: CxGeometryPool,

    pub draw_shaders: CxDrawShaders,

    pub new_draw_event: DrawEvent,

    pub redraw_id: u64,

    /// Process-wide source of uniform block generations. A generation is
    /// issued once and never reused, so pooled draw/pass/list slots cannot
    /// compare equal to a previous occupant's cached upload generation.
    pub(crate) uniform_gen: u64,

    pub(crate) repaint_id: u64,
    pub(crate) event_id: u64,
    pub(crate) timer_id: u64,
    pub(crate) next_frame_id: u64,
    pub(crate) permissions_request_id: i32,
    pub(crate) storage_state: StorageState,

    pub keyboard: CxKeyboard,
    pub fingers: CxFingers,
    pub(crate) ime_area: Area,
    pub keyboard_shift: f64,
    pub(crate) drag_drop: CxDragDrop,
    pub(crate) file_dialogs: FileDialogState,

    pub(crate) platform_ops: VecDeque<CxOsOp>,
    pub(crate) pending_camera_playbacks: Vec<PendingCameraPlayback>,

    pub(crate) new_next_frames: HashSet<NextFrame>,

    pub new_actions: ActionsBuf,

    pub(crate) dependencies: HashMap<String, CxDependency>,

    pub(crate) triggers: HashMap<Area, Vec<Trigger>>,
    /*
    pub (crate) live_file_change_receiver: std::sync::mpsc::Receiver<Vec<LiveFileChange>>,
    pub (crate) live_file_change_sender: std::sync::mpsc::Sender<Vec<LiveFileChange >>,
    */
    pub(crate) action_receiver: std::sync::mpsc::Receiver<ActionSend>,

    pub os: CxOs,
    // (cratethis cuts the compiletime of an end-user application in half
    pub(crate) event_handler: Rc<UnsafeCell<Box<dyn FnMut(&mut Cx, &Event)>>>,
    pub(crate) event_handler_dispatch_active: Rc<Cell<bool>>,

    pub(crate) globals: Vec<(TypeId, Box<dyn Any>)>,

    pub components: ComponentRegistries,

    pub(crate) self_ref: Option<Rc<RefCell<Cx>>>,
    pub(crate) in_draw_event: bool,

    /// Display context for the main window, used by AdaptiveView
    pub display_context: DisplayContext,

    /// When true, the next event-loop iteration will fire `Event::ScriptReapply`,
    /// which re-applies the captured app value with `Apply::ScriptReapply`
    /// *without* re-running `script_mod`. Use this when a runtime mutation
    /// has updated a shared heap object (e.g. `script_eval!` overriding a
    /// preference); widgets that hold a reference to that object will pick
    /// up the new value on re-apply, and `script_eval!` overrides are
    /// preserved because the source-defined defaults aren't re-asserted.
    pub pending_script_reapply: bool,

    /// When true, the next event-loop iteration will fire `Event::LiveEdit`,
    /// which re-runs `script_mod` and re-applies with `Apply::Reload`. Use
    /// this when a primitive heap value (e.g. `mod.widgets.SAFE_INSET_PAD_TOP`)
    /// has changed and needs to be re-baked into widget definitions that
    /// reference it via expressions like `top: (mod.widgets.SAFE_INSET_PAD_TOP)`
    /// — those expressions are only re-evaluated when `script_mod` re-runs.
    /// `Apply::Reload` clobbers runtime widget state (animator values, etc.),
    /// so prefer `pending_script_reapply` whenever the change can be modeled
    /// as a shared-heap-object mutation instead.
    pub pending_live_edit_request: bool,

    /// `WindowGeomChange` events queued up during an event dispatch.
    pub(crate) pending_window_geom_changes: Vec<WindowGeomChangeEvent>,
    pub(crate) clear_hover_queued: bool,

    pub debug: Debug,

    #[allow(dead_code)]
    pub(crate) executor: Option<Executor>,
    pub(crate) spawner: Spawner,

    pub(crate) studio_http: String,

    pub performance_stats: PerformanceStats,
    /// Frame monitor behind the PerfGraph widget; off until the widget enables it.
    pub perf_monitor: PerfMonitor,
    /// The exploded z-layer inspection view. Inert while off.
    pub sploded: SplodedView,
    /// How many `WidgetRef` draw scopes deep the current draw is — the turtle
    /// nesting AS COMPONENTS SEE IT. Maintained by `WidgetRef::draw_walk` and
    /// its siblings, stamped onto every draw call at creation, and used as the
    /// z axis of the exploded view: one plane per selectable node.
    pub nesting_depth: usize,
    /// Deepest `nesting_depth` reached during the last draw. The exploded
    /// view sizes its fan from this instead of a draw-call count.
    pub nesting_depth_max: usize,
    /// Runs after every draw event, before the paint: for tools that edit
    /// the draw buffers in place (the tweaker's theme pulse) and must
    /// re-apply after widgets rewrite them.
    pub post_draw_hook: Option<Box<dyn FnMut(&mut Cx)>>,
    #[allow(unused)]
    pub(crate) screenshot_requests: Vec<ScreenshotRequest>,
    #[allow(dead_code)]
    pub(crate) run_view_frame_requests: Vec<RunViewFrameRequest>,
    #[allow(dead_code)]
    pub(crate) run_view_frame_results: ToUIReceiver<Result<RunViewFrameData, String>>,
    #[allow(dead_code)]
    pub(crate) run_view_frame_encode_in_flight: bool,
    pub(crate) widget_tree_dump_requests: Vec<u64>,
    pub(crate) widget_snapshot_requests: Vec<u64>,
    /// Event ID that triggered a widget query cache invalidation.
    /// When Some(event_id), indicates that widgets should clear their query caches
    /// on the next event loop cycle. This ensures all views process the cache clear
    /// before it's reset to None.
    ///
    /// This is primarily used when adaptive views change their active variant,
    /// as the widget hierarchy changes require parent views to rebuild their widget queries.
    pub widget_query_invalidation_event: Option<u64>,

    pub widget_tree_ptr: *mut (),
    pub widget_tree_dump_callback: Option<fn(&Cx) -> String>,
    pub widget_query_callback: Option<fn(&Cx, &str) -> Vec<String>>,
    pub widget_snapshot_callback: Option<fn(&Cx) -> Vec<WidgetSnapshot>>,
    /// The tweaker overlay's remote dispatcher (widgets/src/tweaker.rs).
    /// Registered by the widgets crate at startup, exactly like the widget
    /// tree callbacks above; the /tweak routes in remote.rs delegate here so
    /// platform never depends on widgets. `(op, query/body params) -> JSON`.
    pub tweak_callback: Option<fn(&mut Cx, &str, &[(String, String)]) -> Result<String, String>>,
    /// The AI chat overlay's remote dispatcher (`/ai`, `/ai/transcript`):
    /// registered by the aichat crate when an app links it, the same way
    /// the widgets crate registers the tweaker's. `(op, params) -> JSON`.
    pub ai_callback: Option<fn(&mut Cx, &str, &[(String, String)]) -> Result<String, String>>,

    pub net: Arc<NetworkRuntime>,
}

#[derive(Clone)]
pub struct CxRef(pub Rc<RefCell<Cx>>);

pub struct CxDependency {
    pub data: Option<Result<Rc<Vec<u8>>, String>>,
}
#[derive(Clone, Debug, Default, Script, ScriptHook)]
pub struct AndroidParams {
    #[live]
    pub cache_path: String,
    #[live]
    pub data_path: String,
    #[live]
    pub density: f64,
    #[live]
    pub is_emulator: bool,
    #[live]
    pub has_xr_mode: bool,
    #[live]
    pub android_version: String,
    #[live]
    pub build_number: String,
    #[live]
    pub kernel_version: String,
}

#[derive(Clone, Debug, Default, Script, ScriptHook)]
pub struct IosParams {
    #[live]
    pub data_path: String,
    #[live]
    pub device_model: String,
    #[live]
    pub system_version: String,
}

#[derive(Clone, Debug, Default, Script, ScriptHook)]
pub struct OpenHarmonyParams {
    #[live]
    pub files_dir: String,
    #[live]
    pub cache_dir: String,
    #[live]
    pub temp_dir: String,
    #[live]
    pub device_type: String,
    #[live]
    pub os_full_name: String,
    #[live]
    pub display_density: f64,
}

#[derive(Clone, Debug, Default, Script, ScriptHook)]
pub struct WebParams {
    #[live]
    pub protocol: String,
    #[live]
    pub host: String,
    #[live]
    pub hostname: String,
    #[live]
    pub pathname: String,
    #[live]
    pub search: String,
    #[live]
    pub hash: String,
    /// Phone-class browser (see `WasmBridge.is_phone`): the memory budget is
    /// `PHONE_WEB_MEMORY_BUDGET_BYTES` and the wasm heap maximum is 512 MiB.
    #[live]
    pub is_phone: bool,
}

#[derive(Clone, Debug, Default, Script, ScriptHook)]
pub struct LinuxWindowParams {
    #[live]
    pub custom_window_chrome: bool,
}

#[derive(Clone, Debug, Script, ScriptHook)]
pub enum OsType {
    #[pick]
    Unknown,
    Windows,
    Macos,
    #[live(IosParams::default())]
    Ios(IosParams),
    #[live(AndroidParams::default())]
    Android(AndroidParams),
    #[live(OpenHarmonyParams::default())]
    OpenHarmony(OpenHarmonyParams),
    #[live(LinuxWindowParams::default())]
    LinuxWindow(LinuxWindowParams),
    LinuxDirect,
    #[live(WebParams::default())]
    Web(WebParams),
}

#[derive(Default)]
pub struct XrCapabilities {
    pub ar_supported: bool,
    pub vr_supported: bool,
}

impl OsType {
    /// The platform has ONE window. A second `Window` is not created there
    /// (the web reports it once and never paints its pass; the canvas is
    /// window zero's), so an app that wants a second surface — a projector
    /// output, say — hosts it in-page when this is true.
    pub fn is_single_window(&self) -> bool {
        match self {
            OsType::Web(_) => true,
            OsType::Ios(_) => true,
            OsType::Android(_) => true,
            OsType::LinuxDirect => true,
            _ => false,
        }
    }
    pub fn is_web(&self) -> bool {
        match self {
            OsType::Web(_) => true,
            _ => false,
        }
    }

    pub fn has_xr_mode(&self) -> bool {
        match self {
            OsType::Android(o) => o.has_xr_mode,
            _ => false,
        }
    }

    pub fn get_cache_dir(&self) -> Option<String> {
        match self {
            OsType::Android(params) => Some(params.cache_path.clone()),
            OsType::OpenHarmony(params) => Some(params.cache_dir.clone()),
            // Desktop Linux (windowed or DRM/direct): persist the GL program-binary cache
            // under the XDG cache directory so compiled shaders survive across launches.
            // Computed once and memoized (env lookup + directory creation).
            //
            // Note: the Windows backend is D3D11 and caches its compiled DXBC separately
            // via `shader_cache_dir()` in `os/windows/d3d11.rs`, so it does not rely on
            // this. macOS/iOS use Metal libraries and likewise do not use this path.
            OsType::LinuxWindow(_) | OsType::LinuxDirect => {
                use std::sync::OnceLock;
                static DIR: OnceLock<Option<String>> = OnceLock::new();
                DIR.get_or_init(|| {
                    // Resolve the XDG cache base the same way the XDG Base Directory spec
                    // (and the `robius-directories` crate) do: honor $XDG_CACHE_HOME only
                    // when it is an *absolute* path, otherwise fall back to $HOME/.cache.
                    // A relative or empty value is ignored per spec.
                    let base = std::env::var_os("XDG_CACHE_HOME")
                        .map(std::path::PathBuf::from)
                        .filter(|p| p.is_absolute())
                        .or_else(|| {
                            std::env::var_os("HOME")
                                .map(std::path::PathBuf::from)
                                .filter(|p| p.is_absolute())
                                .map(|home| home.join(".cache"))
                        })?;
                    let dir = base.join("makepad");
                    std::fs::create_dir_all(&dir).ok()?;
                    Some(dir.to_string_lossy().into_owned())
                })
                .clone()
            }
            _ => None,
        }
    }

    pub fn get_data_dir(&self) -> Option<String> {
        if let OsType::Android(params) = self {
            Some(params.data_path.clone())
        } else if let OsType::Ios(params) = self {
            Some(params.data_path.clone())
        } else if let OsType::OpenHarmony(params) = self {
            Some(params.files_dir.clone())
        } else {
            None
        }
    }
}

const DEFAULT_MEMORY_BUDGET_BYTES: usize = 1536 * 1024 * 1024;
/// Working budget for a phone-class browser tab: the wasm heap maximum there
/// is 512 MiB and the tab dies around 1 GiB total, so elastic caches must
/// stop well below the heap ceiling.
pub const PHONE_WEB_MEMORY_BUDGET_BYTES: usize = 320 * 1024 * 1024;
#[allow(dead_code)]
const LOW_MEMORY_DEVICE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
#[allow(dead_code)]
const MIN_MOBILE_MEMORY_BUDGET_BYTES: u64 = 384 * 1024 * 1024;
#[allow(dead_code)]
const MAX_MOBILE_MEMORY_BUDGET_BYTES: u64 = 1536 * 1024 * 1024;

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum MemoryBudgetPolicy {
    Desktop,
    Mobile,
}

/// Turns the platform's one physical-memory measurement into the process
/// envelope used by elastic subsystems. Device discovery stays platform
/// specific; all policy lives here.
#[allow(dead_code)]
fn memory_budget_from_physical_memory(
    physical_memory_bytes: u64,
    policy: MemoryBudgetPolicy,
) -> usize {
    let budget = match policy {
        MemoryBudgetPolicy::Desktop if physical_memory_bytes < LOW_MEMORY_DEVICE_BYTES => {
            physical_memory_bytes / 4
        }
        MemoryBudgetPolicy::Desktop => DEFAULT_MEMORY_BUDGET_BYTES as u64,
        MemoryBudgetPolicy::Mobile => (physical_memory_bytes / 4).clamp(
            MIN_MOBILE_MEMORY_BUDGET_BYTES,
            MAX_MOBILE_MEMORY_BUDGET_BYTES,
        ),
    };
    budget.min(usize::MAX as u64) as usize
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn apple_physical_memory_bytes() -> Option<u64> {
    use makepad_objc_sys::{class, msg_send, runtime::Object, sel, sel_impl};

    unsafe {
        let process_info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        if process_info.is_null() {
            None
        } else {
            let bytes: u64 = msg_send![process_info, physicalMemory];
            (bytes != 0).then_some(bytes)
        }
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn linux_physical_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|line| {
        line.split_once(':')
            .is_some_and(|(name, _)| name.trim() == "MemTotal")
    })?;
    let mut fields = line.split_once(':')?.1.split_whitespace();
    let kib = fields.next()?.parse::<u64>().ok()?;
    (fields.next() == Some("kB"))
        .then(|| kib.checked_mul(1024))
        .flatten()
}

#[cfg(target_os = "windows")]
fn windows_physical_memory_bytes() -> Option<u64> {
    #[allow(non_snake_case)]
    #[repr(C)]
    struct MemoryStatusEx {
        dwLength: u32,
        dwMemoryLoad: u32,
        ullTotalPhys: u64,
        ullAvailPhys: u64,
        ullTotalPageFile: u64,
        ullAvailPageFile: u64,
        ullTotalVirtual: u64,
        ullAvailVirtual: u64,
        ullAvailExtendedVirtual: u64,
    }

    windows_core::link!("kernel32.dll" "system" fn GlobalMemoryStatusEx(
        status: *mut MemoryStatusEx
    ) -> windows_core::BOOL);

    let mut status = MemoryStatusEx {
        dwLength: std::mem::size_of::<MemoryStatusEx>() as u32,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };
    unsafe {
        (GlobalMemoryStatusEx(&mut status).0 != 0 && status.ullTotalPhys != 0)
            .then_some(status.ullTotalPhys)
    }
}

#[cfg(target_arch = "wasm32")]
fn platform_memory_budget(web_memory_bytes: usize) -> (usize, &'static str) {
    if web_memory_bytes == PHONE_WEB_MEMORY_BUDGET_BYTES {
        (web_memory_bytes, "phone web policy")
    } else {
        (web_memory_bytes, "wasm memory maximum")
    }
}

#[cfg(target_os = "macos")]
fn platform_memory_budget(default: usize) -> (usize, &'static str) {
    apple_physical_memory_bytes()
        .map(|bytes| {
            (
                memory_budget_from_physical_memory(bytes, MemoryBudgetPolicy::Desktop),
                "ProcessInfo.processInfo.physicalMemory",
            )
        })
        .unwrap_or((default, "default"))
}

#[cfg(target_os = "ios")]
fn platform_memory_budget(default: usize) -> (usize, &'static str) {
    apple_physical_memory_bytes()
        .map(|bytes| {
            (
                memory_budget_from_physical_memory(bytes, MemoryBudgetPolicy::Mobile),
                "ProcessInfo.processInfo.physicalMemory",
            )
        })
        .unwrap_or((default, "default"))
}

#[cfg(target_os = "android")]
fn platform_memory_budget(default: usize) -> (usize, &'static str) {
    crate::os::linux::android::android_jni::physical_memory_bytes()
        .map(|bytes| {
            (
                memory_budget_from_physical_memory(bytes, MemoryBudgetPolicy::Mobile),
                "ActivityManager.MemoryInfo.totalMem",
            )
        })
        .unwrap_or((default, "default"))
}

#[cfg(target_os = "windows")]
fn platform_memory_budget(default: usize) -> (usize, &'static str) {
    windows_physical_memory_bytes()
        .map(|bytes| {
            (
                memory_budget_from_physical_memory(bytes, MemoryBudgetPolicy::Desktop),
                "GlobalMemoryStatusEx",
            )
        })
        .unwrap_or((default, "default"))
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn platform_memory_budget(default: usize) -> (usize, &'static str) {
    linux_physical_memory_bytes()
        .map(|bytes| {
            (
                memory_budget_from_physical_memory(bytes, MemoryBudgetPolicy::Desktop),
                "/proc/meminfo MemTotal",
            )
        })
        .unwrap_or((default, "default"))
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos")),
)))]
fn platform_memory_budget(default: usize) -> (usize, &'static str) {
    (default, "default")
}

/// Owner breakdown of the process memory the platform itself holds: CPU-side
/// geometry staging, draw-list instance buffers, texture data, script
/// resources (font files) and, on wasm, what the allocator holds from the
/// linear memory. Walks the pools once; read it from a slow timer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CxMemoryReport {
    /// Linear memory size on wasm; 0 elsewhere.
    pub wasm_memory_bytes: usize,
    /// Live direct (above the size classes) allocations on wasm; 0 elsewhere.
    pub alloc_large_bytes: usize,
    pub alloc_large_count: usize,
    /// Live 64 KiB size-class chunks on wasm; 0 elsewhere.
    pub alloc_chunk_bytes: usize,
    /// CPU vertex/index staging still held by geometry slots (free slots included).
    pub geometry_cpu_bytes: usize,
    pub geometry_slots: usize,
    /// CPU instance buffers held by draw items across every draw-list slot.
    pub instance_cpu_bytes: usize,
    pub draw_list_slots: usize,
    /// CPU pixel data held by texture slots.
    pub texture_cpu_bytes: usize,
    /// Loaded script resource bytes (font files and other crate resources).
    pub resource_bytes: usize,
}

impl Cx {
    /// Issue the next nonzero, process-wide uniform generation.
    #[inline]
    pub fn next_uniform_gen(&mut self) -> u64 {
        Self::next_uniform_gen_from(&mut self.uniform_gen)
    }

    #[inline]
    pub(crate) fn next_uniform_gen_from(uniform_gen: &mut u64) -> u64 {
        let next = *uniform_gen;
        *uniform_gen = uniform_gen
            .checked_add(1)
            .expect("uniform generation counter exhausted");
        next
    }

    /// A conservative process-wide memory envelope for cache/batch budgets.
    /// Native keeps a generous fixed ceiling; web reports the shared wasm
    /// browser memory envelope through `ToWasmInit` before `Event::Startup`.
    pub fn memory_budget_bytes(&self) -> usize {
        self.memory_budget_bytes
    }

    pub(crate) fn initialize_memory_budget(&mut self) {
        if self.memory_budget_initialized {
            return;
        }
        self.memory_budget_initialized = true;
        let (budget, source) = platform_memory_budget(self.memory_budget_bytes);
        self.memory_budget_bytes = budget;
        crate::log!(
            "memory budget: {} MiB ({})",
            budget / (1024 * 1024),
            source
        );
    }

    pub fn memory_report(&self) -> CxMemoryReport {
        let mut report = CxMemoryReport::default();
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            let stats = crate::web_alloc::stats();
            report.wasm_memory_bytes = crate::web_alloc::wasm_memory_bytes();
            report.alloc_large_bytes = stats.large_bytes;
            report.alloc_large_count = stats.large_count;
            report.alloc_chunk_bytes = stats.chunk_bytes;
        }
        for slot in &self.geometries.0.pool {
            report.geometry_slots += 1;
            report.geometry_cpu_bytes = report
                .geometry_cpu_bytes
                .saturating_add(slot.item.vertices.capacity_bytes())
                .saturating_add(slot.item.indices.capacity_bytes());
        }
        for slot in &self.draw_lists.0.pool {
            report.draw_list_slots += 1;
            for item in &slot.item.draw_items.buffer {
                if let Some(instances) = &item.instances {
                    report.instance_cpu_bytes = report
                        .instance_cpu_bytes
                        .saturating_add(instances.capacity().saturating_mul(4));
                }
            }
        }
        for slot in &self.textures.0.pool {
            report.texture_cpu_bytes = report
                .texture_cpu_bytes
                .saturating_add(slot.item.format.cpu_data_bytes());
        }
        report.resource_bytes = self
            .script_data
            .resources
            .resources
            .borrow()
            .iter()
            .map(|resource| resource.loaded_len())
            .fold(0usize, usize::saturating_add);
        report
    }

    /// Direct allocations of 4 MiB or more since the previous call, oldest
    /// first, as `(bytes, linear memory bytes at that moment)`. Empty outside
    /// wasm. Names the requests that grew the heap between two reports.
    pub fn take_big_allocation_events(&self) -> Vec<(usize, usize)> {
        #[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
        {
            crate::web_alloc::take_big_events()
        }
        #[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
        {
            Vec::new()
        }
    }

    /// Select the application's font policy before script/theme registration.
    /// Once startup begins the choice is immutable because compiled package
    /// metadata and registered resource handles must continue to agree.
    pub fn set_font_set(&mut self, font_set: crate::font_policy::FontSet) -> bool {
        if self.font_set_frozen {
            crate::error!(
                "font set is immutable after script registration (kept {}, rejected {})",
                self.font_set.as_str(),
                font_set.as_str()
            );
            return false;
        }
        self.font_set = font_set;
        true
    }

    pub fn font_set(&self) -> crate::font_policy::FontSet {
        self.font_set
    }

    #[doc(hidden)]
    pub fn freeze_font_set(&mut self) {
        self.font_set_frozen = true;
    }

    pub fn is_font_set_frozen(&self) -> bool {
        self.font_set_frozen
    }

    pub fn new(event_handler: Box<dyn FnMut(&mut Cx, &Event)>) -> Self {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        crate::os::termination_signal::install();

        makepad_network::install_ui_waker(Some(makepad_network::UiWaker::new(|| {
            crate::thread::wake_ui_event_loop();
        })));

        //#[cfg(any(target_arch = "wasm32", target_os = "android"))]
        //crate::makepad_error_log::set_panic_hook();
        // the null texture
        let mut textures = CxTexturePool::default();
        let null_texture = textures.alloc(TextureFormat::VecBGRAu8_32 {
            width: 4,
            height: 4,
            data: Some(vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            updated: TextureUpdated::Full,
        });
        let null_cube_texture = textures.alloc(TextureFormat::VecCubeBGRAu8_32 {
            width: 4,
            height: 4,
            data: Some(vec![0; 4 * 4 * 6]),
            updated: TextureUpdated::Full,
        });

        let (executor, spawner) = executor::new_executor_and_spawner();
        //let (live_file_change_sender, live_file_change_receiver) = std::sync::mpsc::channel();
        let (action_sender, action_receiver) = std::sync::mpsc::channel();
        if let Ok(mut sender) = ACTION_SENDER_GLOBAL.lock() {
            *sender = Some(action_sender);
        }
        // On platforms that use a shim backend (wasm, android), install it
        // before creating the NetworkRuntime so it picks up the real backend.
        #[cfg(target_arch = "wasm32")]
        crate::os::web_network::install_network_backend_shim();
        #[cfg(target_os = "android")]
        crate::os::linux::android::android_network::install_network_backend_shim();

        let net = Arc::new(NetworkRuntime::new(Default::default()));
        net.set_wake_fn(Some(Arc::new(|| {
            SignalToUI::set_ui_signal();
        })));

        let script_std = makepad_script_std::ScriptStd::with_network_runtime(net.clone());
        let script_vm = Box::new(ScriptVmBase::new());
        let crate_manifests = script_vm.code.crate_manifests.clone();
        let script_mod_overrides = script_vm.code.script_mod_overrides.clone();

        let mut cx = Self {
            package_root: None,
            font_set: crate::font_policy::FontSet::target_default(),
            font_set_frozen: false,
            demo_time_repaint: false,
            null_texture,
            null_cube_texture,
            cpu_cores: crate::thread::available_parallelism().get(),
            memory_budget_bytes: DEFAULT_MEMORY_BUDGET_BYTES,
            memory_budget_initialized: false,
            thread_spawner: crate::thread::ThreadSpawner::for_current_thread(
                crate::thread::available_parallelism().get(),
            ),
            task_pool: std::cell::OnceCell::new(),
            in_makepad_studio: false,
            game_input_remote: Vec::new(),
            in_draw_event: false,
            os_type: OsType::Unknown,
            gpu_info: Default::default(),
            xr_capabilities: Default::default(),

            windows: Default::default(),
            passes: Default::default(),
            draw_lists: Default::default(),
            draw_matrices: Default::default(),
            geometries: Default::default(),
            textures,
            uniform_buffers: Default::default(),

            draw_shaders: Default::default(),

            new_draw_event: Default::default(),
            new_actions: Default::default(),

            redraw_id: 1,
            uniform_gen: 1,
            event_id: 1,
            repaint_id: 1,
            timer_id: 1,
            next_frame_id: 1,
            permissions_request_id: 0,
            storage_state: StorageState::default(),

            keyboard: Default::default(),
            fingers: Default::default(),
            drag_drop: Default::default(),
            file_dialogs: Default::default(),
            ime_area: Default::default(),
            keyboard_shift: 0.0,
            platform_ops: Default::default(),
            pending_camera_playbacks: Vec::new(),
            studio_http: "".to_string(),
            new_next_frames: Default::default(),

            post_draw_hook: None,
            screenshot_requests: Default::default(),
            run_view_frame_requests: Default::default(),
            run_view_frame_results: Default::default(),
            run_view_frame_encode_in_flight: false,

            dependencies: Default::default(),

            triggers: Default::default(),

            action_receiver,

            os: CxOs::default(),

            event_handler: Rc::new(UnsafeCell::new(event_handler)),
            event_handler_dispatch_active: Rc::new(Cell::new(false)),

            debug: Default::default(),

            debug_trace_active: false,

            globals: Default::default(),

            components: ComponentRegistries::new(),

            executor: Some(executor),
            spawner,

            self_ref: None,
            performance_stats: Default::default(),
            perf_monitor: Default::default(),
            sploded: Default::default(),
            nesting_depth: 0,
            nesting_depth_max: 0,

            display_context: Default::default(),
            pending_script_reapply: false,
            pending_live_edit_request: false,
            pending_window_geom_changes: Default::default(),
            clear_hover_queued: false,

            widget_tree_dump_requests: Default::default(),
            widget_snapshot_requests: Default::default(),
            widget_query_invalidation_event: None,
            widget_tree_ptr: std::ptr::null_mut(),
            widget_tree_dump_callback: None,
            widget_query_callback: None,
            widget_snapshot_callback: None,
            tweak_callback: None,
            ai_callback: None,
            net,

            script_data: CxScriptData {
                std: script_std,
                crate_manifests,
                live_reload: crate::live_reload::CxLiveReloadState {
                    script_mod_overrides,
                    ..Default::default()
                },
                ..Default::default()
            },
            script_vm: Some(script_vm),
        };

        //todo!();
        cx.with_vm(crate::script::script_mod);
        cx
    }
}

// ---------------------------------------------------------------------------
// Startup trace — working-tree instrumentation, gated on the `startup` topic.
//
// Prints `[startup] <phase> +<ms>` where <ms> is measured from process exec
// when a launcher exported MAKEPAD_STARTUP_T0 (epoch seconds, f64) just
// before exec — that is the only way to see the pre-`main` dyld / Gatekeeper
// window. Without it the clock starts at the first call.
//
// MAKEPAD_STARTUP_T0 remains launcher-supplied and is read only when enabled.
// ---------------------------------------------------------------------------

static STARTUP_T0: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
static STARTUP_ACC: std::sync::Mutex<Vec<(&'static str, f64, u32)>> =
    std::sync::Mutex::new(Vec::new());

#[inline]
pub fn startup_trace_enabled() -> bool {
    crate::makepad_error_log::trace_enabled("startup")
}

fn startup_t0() -> f64 {
    *STARTUP_T0.get_or_init(|| {
        std::env::var("MAKEPAD_STARTUP_T0")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or_else(Cx::time_now)
    })
}

/// Milliseconds since exec (or since the first trace call).
pub fn startup_since_exec_ms() -> f64 {
    if !startup_trace_enabled() {
        return 0.0;
    }
    startup_since_exec_ms_enabled()
}

fn startup_since_exec_ms_enabled() -> f64 {
    (Cx::time_now() - startup_t0()).max(0.0) * 1000.0
}

/// Mark a startup phase.
pub fn startup_trace(phase: &str) {
    crate::trace!(
        "startup",
        "{:<28} +{:9.2} ms",
        phase,
        startup_since_exec_ms_enabled()
    );
}

/// Accumulate a repeated sub-cost (shader compiles, font loads, …) under a
/// bucket name; `startup_trace_flush` prints the totals.
pub fn startup_acc(bucket: &'static str, ms: f64) {
    if !startup_trace_enabled() {
        return;
    }
    let mut acc = STARTUP_ACC.lock().unwrap();
    if let Some(row) = acc.iter_mut().find(|r| r.0 == bucket) {
        row.1 += ms;
        row.2 += 1;
    } else {
        acc.push((bucket, ms, 1));
    }
}

/// Print accumulated buckets and reset them.
pub fn startup_trace_flush(phase: &str) {
    if !startup_trace_enabled() {
        return;
    }
    let rows = std::mem::take(&mut *STARTUP_ACC.lock().unwrap());
    for (bucket, ms, n) in rows {
        crate::trace!(
            "startup",
            "{:<28}  {:8.2} ms total over {} ({})",
            bucket, ms, n, phase
        );
    }
}

#[cfg(test)]
mod memory_budget_tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    #[test]
    fn uniform_generation_counter_starts_at_one_and_is_monotonic() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert_eq!(cx.next_uniform_gen(), 1);
        assert_eq!(cx.next_uniform_gen(), 2);
        assert_eq!(cx.next_uniform_gen(), 3);
    }

    #[test]
    fn low_memory_desktop_uses_one_quarter_of_physical_ram() {
        assert_eq!(
            memory_budget_from_physical_memory(4 * GIB, MemoryBudgetPolicy::Desktop),
            (1 * GIB) as usize
        );
    }

    #[test]
    fn desktop_at_threshold_keeps_the_default_budget() {
        assert_eq!(
            memory_budget_from_physical_memory(8 * GIB, MemoryBudgetPolicy::Desktop),
            DEFAULT_MEMORY_BUDGET_BYTES
        );
    }

    #[test]
    fn small_mobile_is_clamped_to_the_minimum() {
        assert_eq!(
            memory_budget_from_physical_memory(1 * GIB, MemoryBudgetPolicy::Mobile),
            (384 * MIB) as usize
        );
    }

    #[test]
    fn mid_sized_mobile_uses_one_quarter_of_physical_ram() {
        assert_eq!(
            memory_budget_from_physical_memory(4 * GIB, MemoryBudgetPolicy::Mobile),
            (1 * GIB) as usize
        );
    }

    #[test]
    fn large_mobile_is_clamped_to_the_maximum() {
        assert_eq!(
            memory_budget_from_physical_memory(8 * GIB, MemoryBudgetPolicy::Mobile),
            DEFAULT_MEMORY_BUDGET_BYTES
        );
    }
}

impl Cx {
    /// True while the platform is still compiling draw shaders it was handed
    /// and is therefore dropping (WebGL) their draw calls. Native backends
    /// build pipelines inside the paint that first uses them and never
    /// answer true. An offscreen bake that captures pixels polls this before
    /// trusting a frame: a capture drawn while its program links is black.
    pub fn draw_shaders_pending(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.os.webgl_shaders_pending > 0
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    }
}
