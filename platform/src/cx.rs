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
            CancelScope, CancelScopeKind, CxCancelScopes, CxDragDrop, CxFingers, CxKeyboard, DrawEvent, Event,
            NextFrame, Trigger, WindowGeomChangeEvent,
        },
        file_dialogs::FileDialogState,
        geometry::CxGeometryPool,
        gpu_info::GpuInfo,
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
    /// `crate_name` → source directory for a compile-on-device installation,
    /// from the map beside the executable (`os::cx_native::load_package_paths`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) package_paths: std::collections::HashMap<String, std::path::PathBuf>,
    /// Native user-input bookkeeping for the `--remote` interface.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) remote_activity: crate::remote::RemoteActivity,
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
    /// Keeps the frame loop running. Which passes repaint is up to each pass's `repaint_next`.
    pub demo_time_repaint: bool,
    /// `draw_pass.time` counts from here, which `Cx::pass_time` moves while no pass owes a time frame.
    pub(crate) pass_time_origin: f64,
    pub(crate) pass_time_rebase: bool,
    pub(crate) mouse_cursor: crate::cursor::MouseCursor,
    pub(crate) gpu_info: GpuInfo,
    pub(crate) gpu_admission: crate::gpu_admission::GpuLedger,
    pub(crate) clipboard: crate::clipboard_read::ClipboardRequests,
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
    /// Shared-instance publications (cleanup DL-1 freeze): the registry that
    /// mints ids, charges bytes and hands out receipts over the frame serials.
    pub publications: crate::shared_instances::Publications,
    pub uniform_buffers: CxUniformBufferPool,
    pub geometries: CxGeometryPool,

    pub draw_shaders: CxDrawShaders,

    pub new_draw_event: DrawEvent,
    /// Run on every Event::Signal before the app sees it: subsystems above the platform
    /// (the text system's glyph workers) react to their workers' signals.
    pub signal_hooks: Vec<fn(&mut Cx)>,

    pub redraw_id: u64,
    /// The drawing being recorded (`Cx::drawing_id`): one per draw event, and
    /// one per draw context made outside a draw event.
    pub(crate) drawing_id: u64,
    /// `redraw_id` when that drawing began: the lists its draw contexts
    /// recorded carry a higher one.
    pub(crate) drawing_start: u64,
    /// What a frame drawn again before it painted keeps of its earlier
    /// drawings (`Cx::frame_drawn_again`).
    pub(crate) redraw_keep: crate::os::cx_shared::RedrawKeep,

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
    pub(crate) cancel_scopes: CxCancelScopes,
    pub fingers: CxFingers,
    pub(crate) ime_area: Area,
    pub keyboard_shift: f64,
    pub(crate) drag_drop: CxDragDrop,
    pub(crate) file_dialogs: FileDialogState,

    pub(crate) platform_ops: VecDeque<CxOsOp>,
    pub(crate) pending_camera_playbacks: Vec<PendingCameraPlayback>,

    pub(crate) new_next_frames: HashSet<NextFrame>,
    /// Frame requests made so far (`Cx::frame_requests`).
    pub(crate) frame_requests: u64,
    /// A child its host says nothing shows of (`HostedVisible`: a hosted process over its
    /// socket, a HAL domain as an app message from its parent): next-frame callbacks and draws
    /// wait until it is shown again (`hosted_frames_held`).
    pub(crate) hosted_hidden: bool,

    pub new_actions: ActionsBuf,

    pub(crate) dependencies: HashMap<String, CxDependency>,

    pub(crate) triggers: HashMap<Area, Vec<Trigger>>,
    /*
    pub (crate) live_file_change_receiver: crate::makepad_network::mpsc::Receiver<Vec<LiveFileChange>>,
    pub (crate) live_file_change_sender: crate::makepad_network::mpsc::Sender<Vec<LiveFileChange >>,
    */
    pub(crate) action_receiver: crate::makepad_network::mpsc::Receiver<ActionSend>,

    #[cfg(not(target_arch = "wasm32"))]
    pub hal: crate::os::hal::HalClient,
    #[cfg(target_arch = "wasm32")]
    pub os: crate::os::web::CxOs,
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
    /// which re-runs `script_mod` and re-applies with `Apply::Rebake`. Use
    /// this when a primitive heap value (e.g. `mod.widgets.SAFE_INSET_PAD_TOP`)
    /// has changed and needs to be re-baked into widget definitions that
    /// reference it via expressions like `top: (mod.widgets.SAFE_INSET_PAD_TOP)`
    /// — those expressions are only re-evaluated when `script_mod` re-runs.
    /// The re-run is still a full-tree walk, so prefer
    /// `pending_script_reapply` whenever the change can be modeled as a
    /// shared-heap-object mutation instead.
    pub pending_live_edit_request: bool,
    /// Re-evaluate Splash definitions while preserving imperative widget state.
    pub pending_style_reload: bool,
    /// The script errors the last `Event::LiveEdit` re-run raised, kept for
    /// whoever queued the change (the design overlay rolls its edit back on
    /// them); they are logged as well.
    pub(crate) live_edit_errors: Vec<String>,

    /// Which `Apply` variant the pending `Event::LiveEdit` should re-apply
    /// the freshly re-run `script_mod` value with. A file-change hot reload
    /// means the DSL actually changed, so the new template wins
    /// (`Apply::Reload`). A `request_live_edit()` re-bake did not change the
    /// DSL, so imperative runtime state must survive (`Apply::Rebake`) —
    /// otherwise every safe-area inset change wipes each `set_text`,
    /// `set_visible` and animator state in the tree.
    pub(crate) live_edit_apply: Apply,

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
    pub(crate) pick: crate::pick::CxPick,
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
    /// The collect run's driver (`MAKEPAD_RUN=collect-web`, see
    /// [`crate::collect`]); `None` in a normal run.
    pub(crate) collect: Option<Box<crate::collect::Collector>>,
    #[allow(unused)]
    pub(crate) screenshot_requests: Vec<ScreenshotRequest>,
    /// Frames to copy for transitions (`window_snapshot.rs`).
    pub(crate) window_snapshots: Vec<crate::window_snapshot::WindowSnapshotRequest>,
    /// Draws skipped because their pipeline was still compiling: in all,
    /// and the repaint that last skipped one (`window_snapshot.rs`).
    pub(crate) pipeline_skips: u64,
    /// The same by reason: instances not yet presentable, no compiled
    /// shader, pipeline still compiling.
    pub(crate) skip_reasons: [u64; 3],
    /// Until then (seconds since start), a window frame with a draw skipped
    /// for a compiling pipeline is not presented: the last whole frame stays.
    pub(crate) whole_frames_until: f64,
    /// When the current whole-frames hold began (for the `switch` trace).
    pub(crate) whole_hold_began: Option<f64>,
    /// The app knows the frame being drawn is not the finished one (it lays
    /// out again next frame): hold it back, within a whole-frames hold.
    pub(crate) hold_next_paint: bool,
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

    /// The app's timers while `--virtual-clock` runs (see `remote::app_clock`).
    pub app_clock: crate::remote::app_clock::AppClock,
    /// The widget snapshot with each widget's dotted id path from its window
    /// root (named widgets only), registered by the widgets layer; the remote
    /// bridge's `/snap` path addressing reads it.
    pub widget_paths_callback: Option<fn(&Cx) -> Vec<(WidgetSnapshot, String)>>,
    /// The widget path and type of each given uid, registered by the widgets layer (frame capture
    /// attribution: `draw_owners`).
    pub widget_uid_paths_callback: Option<fn(&Cx, &[u64]) -> Vec<(u64, String, String)>>,
    /// Per-instance widget attribution, only while a capture asks for it (`draw_owners`).
    pub draw_owners: Option<Box<crate::draw_owners::DrawOwners>>,
    /// Subsystems' counts of asynchronous loads in flight, by kind (see
    /// `Cx::register_async_load_probe`).
    pub(crate) async_load_probes: Vec<(&'static str, crate::cx_api::AsyncLoadProbe)>,
    pub widget_tree_ptr: *mut (),
    pub widget_tree_dump_callback: Option<fn(&Cx) -> String>,
    pub widget_query_callback: Option<fn(&Cx, &str) -> Vec<String>>,
    pub widget_snapshot_callback: Option<fn(&Cx) -> Vec<WidgetSnapshot>>,
    /// Selects a cancel scope from the currently active widget hierarchy. The lookup
    /// returns the newest matching scope for a widget UID. Called only for a new
    /// Escape/Back press; widgets install this without a platform dependency on them.
    pub cancel_scope_resolver: Option<fn(&Cx, &dyn Fn(u64) -> Option<u64>) -> Option<u64>>,
    /// The tweaker overlay's remote dispatcher (widgets/families/tweaker/src/tweaker.rs).
    /// Registered by the widgets crate at startup, exactly like the widget
    /// tree callbacks above; the /tweak routes in remote.rs delegate here so
    /// platform never depends on widgets. `(op, query/body params) -> JSON`.
    pub tweak_callback: Option<fn(&mut Cx, &str, &[(String, String)]) -> Result<String, String>>,
    /// A design preview hook: an app that can re-run one file's templates on
    /// its own (the storybook re-evaluates one story file) registers it, so a
    /// designer preview of that file skips the app-wide live edit. Called
    /// with the file and its edited text; `None` leaves the preview to the
    /// platform's live edit, `Some(Ok)` says the text is installed and the
    /// tree rebuilt, `Some(Err)` that the text was refused.
    pub design_preview_callback: Option<fn(&mut Cx, &str, &str) -> Option<Result<(), String>>>,
    /// The AI chat overlay's remote dispatcher (`/ai`, `/ai/transcript`):
    /// registered by the aichat crate when an app links it, the same way
    /// the widgets crate registers the tweaker's. `(op, params) -> JSON`.
    pub ai_callback: Option<fn(&mut Cx, &str, &[(String, String)]) -> Result<String, String>>,

    pub net: Arc<NetworkRuntime>,
    /// A hosted child's host messages that the network drain took off the
    /// shared queue: its event loop reads the host socket itself (from the
    /// first read on this is `Some`), and a Tick also drains `net` for HTTP
    /// and script sockets. A host batch landing in that drain went through
    /// `dispatch_studio_msg`, which drops the loop's own messages (geometry,
    /// swapchain, Tick): the child then kept drawing at the old size. The
    /// drain parks them here and the loop's next read takes them first.
    pub(crate) studio_backlog: Option<std::collections::VecDeque<crate::makepad_network::NetworkResponse>>,
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
    /// The system is in dark (night) mode. Set before Startup and kept
    /// current: a switch while the app runs updates it and re-runs `script_mod`.
    #[live]
    pub night_mode: bool,
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
    #[live(LinuxWindowParams::default())]
    LinuxWindow(LinuxWindowParams),
    LinuxDirect,
    #[live(WebParams::default())]
    Web(WebParams),
}

/// The GPU API this process renders with: the active HAL backend on native
/// targets (Metal on Apple, Direct3D 11 on Windows, Vulkan or GLES on Linux),
/// WebGL on the web. `Cx::gpu_backend` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuBackend {
    Metal,
    Direct3d11,
    Vulkan,
    OpenGl,
    WebGl,
    Headless,
}

impl GpuBackend {
    /// Whether draw calls' scissor rectangles are applied
    /// (`CxDrawShaderOptions::scissor`); elsewhere they are ignored.
    pub fn honors_scissor(self) -> bool {
        matches!(self, GpuBackend::Metal | GpuBackend::Vulkan | GpuBackend::OpenGl)
    }

    pub fn name(self) -> &'static str {
        match self {
            GpuBackend::Metal => "Metal",
            GpuBackend::Direct3d11 => "Direct3D 11",
            GpuBackend::Vulkan => "Vulkan",
            GpuBackend::OpenGl => "OpenGL",
            GpuBackend::WebGl => "WebGL",
            GpuBackend::Headless => "HAL headless",
        }
    }
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
            // Desktop Linux (windowed or DRM/direct) uses the XDG cache directory.
            // Computed once and memoized (env lookup + directory creation).
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
        } else {
            None
        }
    }
}

const DEFAULT_MEMORY_BUDGET_BYTES: usize = 1536 * 1024 * 1024;
/// Working budget for a phone-class browser tab: the wasm heap maximum there
/// is 512 MiB and the tab dies around 1 GiB total, so elastic caches must
/// stop well below the heap ceiling.
#[allow(dead_code)]
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
        MemoryBudgetPolicy::Desktop => physical_memory_bytes / 2,
        MemoryBudgetPolicy::Mobile => (physical_memory_bytes / 4).clamp(
            MIN_MOBILE_MEMORY_BUDGET_BYTES,
            MAX_MOBILE_MEMORY_BUDGET_BYTES,
        ),
    };
    budget.min(usize::MAX as u64) as usize
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn apple_physical_memory_bytes() -> Option<u64> {
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

#[cfg(any(target_os = "android", target_os = "linux"))]
pub(crate) fn linux_physical_memory_bytes() -> Option<u64> {
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
pub(crate) fn windows_physical_memory_bytes() -> Option<u64> {
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

    #[link(name = "kernel32", kind = "raw-dylib")] extern "system" { fn GlobalMemoryStatusEx(
        status: *mut MemoryStatusEx
    ) -> windows::core::BOOL; }

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
    linux_physical_memory_bytes()
        .map(|bytes| {
            (
                memory_budget_from_physical_memory(bytes, MemoryBudgetPolicy::Mobile),
                "/proc/meminfo MemTotal",
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

#[cfg(target_os = "linux")]
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

#[cfg(not(any(target_arch = "wasm32", target_os = "macos", target_os = "ios", target_os = "android", target_os = "windows", target_os = "linux")))]
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

    /// Process-wide memory envelope for cache/batch budgets.
    /// Desktop native is half of physical RAM with no upper clamp; web reports
    /// the shared wasm browser memory envelope through `ToWasmInit` before
    /// `Event::Startup`.
    pub fn memory_budget(&self) -> usize {
        self.memory_budget_bytes
    }

    pub fn memory_budget_bytes(&self) -> usize {
        self.memory_budget()
    }

    pub(crate) fn initialize_memory_budget(&mut self) {
        if self.memory_budget_initialized {
            return;
        }
        self.memory_budget_initialized = true;
        let (budget, source) = platform_memory_budget(self.memory_budget_bytes);
        self.memory_budget_bytes = budget;
        crate::trace!(
            "memory",
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

    /// Begins a global scope for Escape and Back. Widget operations should use
    /// [`Self::begin_widget_cancel_scope()`] so visibility and ancestry are handled
    /// automatically. Global scopes are ordered by activation and have no widget
    /// visibility check; end or drop one when its operation stops being active.
    pub fn begin_cancel_scope(&mut self) -> CancelScope {
        self.cancel_scopes.begin()
    }

    /// Begins a global scope for only the specified gestures. For widget operations,
    /// use [`Self::begin_widget_cancel_scope()`] with the desired gesture kind.
    pub fn begin_cancel_scope_for(&mut self, kind: CancelScopeKind) -> CancelScope {
        self.cancel_scopes.begin_for(kind)
    }

    /// Begins a cancel scope owned by a widget. The widgets framework automatically
    /// excludes hidden or detached owners, and gives descendants priority over their
    /// ancestors. Keep the scope while the operation remains active, including while
    /// its page is hidden. `owner` is the widget's nonzero UID, not its reusable named
    /// ID. Zero is reserved for global scopes. A widgets resolver must be installed
    /// for a bound scope to receive input.
    pub fn begin_widget_cancel_scope(&mut self, owner: u64, kind: CancelScopeKind) -> CancelScope {
        self.cancel_scopes.begin_widget(owner, kind)
    }

    /// Gives up a scope from [`Self::begin_cancel_scope()`]. Dropping it does the same.
    pub fn end_cancel_scope(&mut self, scope: CancelScope) {
        self.cancel_scopes.end(scope)
    }

    /// Whether the cancel gesture being delivered belongs to `scope`, which is the only
    /// case in which that scope's owner should act on it.
    pub fn owns_cancel(&self, scope: &CancelScope) -> bool {
        self.cancel_scopes.owns_press(scope)
    }

    /// Whether this physical cancel press was assigned to any scope. Remains true
    /// after that scope is dropped, so a focused fallback cannot reuse its release.
    pub fn has_cancel_owner(&self) -> bool {
        self.cancel_scopes.has_press_owner()
    }

    /// Whether a back swipe by a touch that starts in the event being delivered would belong
    /// to `scope`, since Back would go to it right now. Only meaningful during that event.
    pub fn owns_back_swipe(&self, scope: &CancelScope) -> bool {
        self.cancel_scopes.owns_back_swipe(scope)
    }

    pub fn new(event_handler: Box<dyn FnMut(&mut Cx, &Event)>) -> Self {
        crate::thread::ui_hang::initialize();
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        if !crate::restart::hosted() {
            crate::os::termination_signal::install();
        }

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

        // a task woken from another thread wakes the event loop that runs it (an idle loop sleeps)
        let (executor, spawner) = executor::new_executor_and_spawner(std::sync::Arc::new(crate::thread::wake_ui_event_loop));
        //let (live_file_change_sender, live_file_change_receiver) = crate::makepad_network::mpsc::channel();
        let (action_sender, action_receiver) = crate::makepad_network::mpsc::channel();
        if let Ok(mut sender) = ACTION_SENDER_GLOBAL.lock() {
            *sender = Some(action_sender);
        }
        // On platforms that use a shim backend (wasm, android), install it
        // before creating the NetworkRuntime so it picks up the real backend.
        #[cfg(target_arch = "wasm32")]
        crate::os::web_network::install_network_backend_shim();

        #[cfg(not(target_arch = "wasm32"))]
        {
            // `--remote` / MAKEPAD_REMOTE=1: the process's makepad-remote endpoint, for every
            // process that has a `Cx`
            crate::remote::start_if_requested();
        }

        let net = Arc::new(NetworkRuntime::new(Default::default()));
        net.set_wake_fn(Some(Arc::new(|| {
            SignalToUI::set_internal_signal();
        })));

        let script_std = makepad_script_std::ScriptStd::with_network_runtime(net.clone());
        let script_vm = Box::new(ScriptVmBase::new());
        let crate_manifests = script_vm.shared_crate_manifests();
        let script_mod_overrides = script_vm.shared_mod_overrides();

        let publications = crate::shared_instances::Publications::new(textures.1.serials.clone());
        let mut cx = Self {
            package_root: None,
            #[cfg(not(target_arch = "wasm32"))]
            package_paths: crate::os::cx_native::load_package_paths(),
            #[cfg(not(target_arch = "wasm32"))]
            remote_activity: Default::default(),
            font_set: crate::font_policy::FontSet::target_default(),
            font_set_frozen: false,
            demo_time_repaint: false,
            pass_time_origin: 0.0,
            pass_time_rebase: true,
            mouse_cursor: Default::default(),
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
            gpu_admission: Default::default(),
            clipboard: Default::default(),
            xr_capabilities: Default::default(),

            windows: Default::default(),
            passes: Default::default(),
            draw_lists: Default::default(),
            draw_matrices: Default::default(),
            geometries: Default::default(),
            textures,
            publications,
            uniform_buffers: Default::default(),

            draw_shaders: Default::default(),

            new_draw_event: Default::default(),
            signal_hooks: Vec::new(),
            new_actions: Default::default(),

            redraw_id: 1,
            drawing_id: 0,
            drawing_start: 0,
            redraw_keep: Default::default(),
            uniform_gen: 1,
            event_id: 1,
            repaint_id: 1,
            timer_id: 1,
            next_frame_id: 1,
            permissions_request_id: 0,
            storage_state: StorageState::default(),

            keyboard: Default::default(),
            cancel_scopes: Default::default(),
            fingers: Default::default(),
            drag_drop: Default::default(),
            file_dialogs: Default::default(),
            ime_area: Default::default(),
            keyboard_shift: 0.0,
            platform_ops: Default::default(),
            pending_camera_playbacks: Vec::new(),
            studio_http: "".to_string(),
            new_next_frames: Default::default(),
            frame_requests: 0,
            hosted_hidden: false,

            post_draw_hook: None,
            collect: None,
            screenshot_requests: Default::default(),
            window_snapshots: Vec::new(),
            pipeline_skips: 0,
            skip_reasons: [0; 3],
            whole_frames_until: 0.0,
            whole_hold_began: None,
            hold_next_paint: false,
            run_view_frame_requests: Default::default(),
            run_view_frame_results: Default::default(),
            run_view_frame_encode_in_flight: false,

            dependencies: Default::default(),

            triggers: Default::default(),

            action_receiver,

            #[cfg(not(target_arch = "wasm32"))]
            hal: crate::os::hal::HalClient::default(),
            #[cfg(target_arch = "wasm32")]
            os: crate::os::web::CxOs::default(),

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
            pick: Default::default(),
            nesting_depth: 0,
            nesting_depth_max: 0,

            display_context: Default::default(),
            pending_script_reapply: false,
            pending_style_reload: false,
            pending_live_edit_request: false,
            live_edit_errors: Vec::new(),
            live_edit_apply: Apply::Reload,
            pending_window_geom_changes: Default::default(),
            clear_hover_queued: false,

            widget_tree_dump_requests: Default::default(),
            widget_snapshot_requests: Default::default(),
            widget_query_invalidation_event: None,
            app_clock: Default::default(),
            widget_paths_callback: None,
            widget_uid_paths_callback: None,
            draw_owners: None,
            async_load_probes: Vec::new(),
            widget_tree_ptr: std::ptr::null_mut(),
            widget_tree_dump_callback: None,
            widget_query_callback: None,
            widget_snapshot_callback: None,
            cancel_scope_resolver: None,
            tweak_callback: None,
            design_preview_callback: None,
            ai_callback: None,
            net,
            studio_backlog: None,

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
        std::env::var("MAKEPAD_RUN_STARTUP_T0")
            .or_else(|_| std::env::var("MAKEPAD_STARTUP_T0"))
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
    fn desktop_uses_half_of_physical_ram_with_no_upper_clamp() {
        assert_eq!(
            memory_budget_from_physical_memory(8 * GIB, MemoryBudgetPolicy::Desktop),
            (4 * GIB) as usize
        );
        assert_eq!(
            memory_budget_from_physical_memory(16 * GIB, MemoryBudgetPolicy::Desktop),
            (8 * GIB) as usize
        );
        assert_eq!(
            memory_budget_from_physical_memory(128 * GIB, MemoryBudgetPolicy::Desktop),
            (64 * GIB) as usize
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

    /// Of the draw shaders [`Cx::draw_shaders_pending`] waits for, how many
    /// are compiling (the browser's parallel compile), and how many are
    /// built and wait for their first draw (first draws spread,
    /// [`Cx::spread_first_draws`]): a host drawing ahead of showing gives
    /// the first time to draw again, the second a draw that uses them.
    pub fn draw_shaders_compiling_and_waiting(&self) -> (usize, usize) {
        #[cfg(target_arch = "wasm32")]
        {
            let waiting = self.os.webgl_shaders_waiting.min(self.os.webgl_shaders_pending);
            (self.os.webgl_shaders_pending - waiting, waiting)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            (0, 0)
        }
    }

    /// WebGL: draw each program for the first time in a frame of its own. A
    /// program's first draw is where the driver builds it for that draw; on
    /// Direct3D (ANGLE) that compile takes up to seconds of the browser's GPU
    /// process for a big shader, so a frame drawing several new programs held
    /// it for their sum (a plate with six scene programs: 9 s). Spread, a
    /// frame makes at most one first draw and leaves out the draws of the
    /// other new programs, which count as pending
    /// ([`Cx::draw_shaders_pending`]) until theirs; the next frames draw them.
    /// For a host that draws ahead of showing (a player's load), not for UI
    /// that must show at once. Native backends ignore it.
    pub fn spread_first_draws(&mut self, on: bool) {
        #[cfg(target_arch = "wasm32")]
        {
            self.os.from_wasm(crate::os::web::from_wasm::FromWasmSpreadFirstDraws { on });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = on;
        }
    }
}

static FIRST_PRESENT_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a first frame reached the display.
pub fn first_present_seen() -> bool {
    FIRST_PRESENT_SEEN.load(std::sync::atomic::Ordering::Acquire)
}

/// Launcher-started windows stay invisible until their first frame.
pub fn launched_hidden_until_first_present() -> bool {
    static WANTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WANTED.get_or_init(|| {
        std::env::var_os("MAKEPAD_RUN_REPORT_TO").is_some()
            && std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_none()
    })
}

/// Also tells a launcher (MAKEPAD_RUN_REPORT_TO, see `report_first_present`).
pub fn startup_trace_first_present() {
    static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    if FIRST.swap(false, std::sync::atomic::Ordering::Relaxed) {
        let presented_at = Cx::time_now();
        FIRST_PRESENT_SEEN.store(true, std::sync::atomic::Ordering::Release);
        // The launcher is told first: the loader's hook below starts the post-present work, and the
        // owner thread serving it (a cache batch) made the first call of this report wait for it
        // (a lazily compiled fn: ~55 ms on a quiet machine) before the launcher saw the frame.
        report_first_present();
        relax_first_present();
        print_first_present(presented_at);
        startup_trace("FIRST PRESENT ON GLASS");
        startup_trace_flush("cumulative at first present");
        crate::restart::first_present();
    }
}

/// Relax runs the app in its own process and publishes this metadata before calling main.
/// Cargo/rustc launches have no package marker and keep their existing stderr behavior.
fn print_first_present(presented_at: f64) {
    let Ok(package) = std::env::var("MAKEPAD_RUN_PACKAGE") else { return };
    // Relax's boot progress line already printed its "ready" summary instead of this line
    if std::env::var_os("MAKEPAD_RUN_PROGRESS").is_some() {
        return;
    }
    // Capture the time in the hook, before report I/O or lazy compilation of the formatter.
    let present_ms = (presented_at - startup_t0()).max(0.0) * 1000.0;
    let main_ms = std::env::var("MAKEPAD_RUN_MAIN_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let cache = std::env::var("MAKEPAD_RUN_CACHE_STATE").unwrap_or_else(|_| "off".to_string());
    let ahead = std::env::var("MAKEPAD_RUN_AHEAD").unwrap_or_else(|_| "balanced".to_string());
    eprintln!("{}", first_present_line(&package, present_ms, main_ms, &cache, &ahead));
}

fn first_present_line(package: &str, present_ms: f64, main_ms: f64, cache: &str, ahead: &str) -> String {
    format!(
        "makepad: {} first frame at {:.2} s (main at {:.2} s, cache {}, ahead {})",
        package, present_ms / 1000.0, main_ms / 1000.0, cache, ahead
    )
}

/// A launcher that started this app with MAKEPAD_RUN_REPORT_TO=<path> (the runner app, through
/// `makepad run --report-to`) learns that the first frame is on glass: the line `present`
/// appended to <path>, in the loader's progress format (the launcher times it on receipt).
/// The launcher brings the window forward then. Any thread (presented handlers run off the
/// main thread); a single O_APPEND write.
fn report_first_present() {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = std::env::var_os("MAKEPAD_RUN_REPORT_TO") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
            let _ = f.write_all(b"present\n");
        }
    }
}

/// A symbol the `makepad` loader provides to the programs it runs (null in a rustc build, which
/// has no loader): dlsym's default scope on unix; on Windows GetProcAddress on the process's own
/// module, which makepad run answers from its loader symbols (a PE exe exports none).
#[cfg(any(unix, windows))]
pub(crate) fn relax_host_symbol(name: &std::ffi::CStr) -> *mut std::ffi::c_void {
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn dlsym(
                handle: *mut std::ffi::c_void,
                name: *const std::ffi::c_char,
            ) -> *mut std::ffi::c_void;
        }
        #[cfg(target_os = "macos")]
        let default = -2isize as *mut std::ffi::c_void;
        #[cfg(not(target_os = "macos"))]
        let default = std::ptr::null_mut();
        dlsym(default, name.as_ptr())
    }
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
            fn GetProcAddress(
                module: *mut std::ffi::c_void,
                name: *const std::ffi::c_char,
            ) -> *mut std::ffi::c_void;
        }
        GetProcAddress(GetModuleHandleW(std::ptr::null()), name.as_ptr())
    }
}

/// Optional loader notification, like alloc_epoch's allocator-swap hook.
/// A rustc build has no such symbol.
#[cfg(any(unix, windows))]
pub(crate) fn relax_first_present() {
    let hook = relax_host_symbol(c"relax_first_present");
    if !hook.is_null() {
        let hook = unsafe { std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn()>(hook) };
        hook();
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn relax_first_present() {}

/// The app's shutdown starts (it quits, its last window closed, a remote `/quit`): under Relax the
/// loader marks the quit (its quit-compile accounting, no settle swap from now on) and writes the
/// disk cache while the app shuts down -- what a termination signal does, without reporting an
/// abnormal end. Once per process; a rustc build has no such symbol.
#[cfg(any(unix, windows))]
pub(crate) fn relax_quit_started() {
    static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ONCE.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let hook = relax_host_symbol(c"relax_quit_started");
    if !hook.is_null() {
        let hook = unsafe { std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn()>(hook) };
        hook();
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn relax_quit_started() {}

/// Each frame the HAL finished, under Relax: the loader logs the frame gaps after the first
/// present (its post-present responsiveness report). The symbol is looked up once.
#[cfg(any(unix, windows))]
pub(crate) fn relax_frame_done() {
    static HOOK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let hook = *HOOK.get_or_init(|| relax_host_symbol(c"relax_frame_done") as usize);
    if hook != 0 {
        let hook = unsafe { std::mem::transmute::<usize, extern "C" fn()>(hook) };
        hook();
    }
}

#[cfg(not(any(unix, windows, target_arch = "wasm32")))]
pub(crate) fn relax_frame_done() {}
