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
        event::{CxDragDrop, CxFingers, CxKeyboard, DrawEvent, Event, NextFrame, Trigger},
        geometry::CxGeometryPool,
        gpu_info::GpuInfo,
        os::CxOs,
        performance_stats::PerformanceStats,
        script::script::CxScriptData,
        studio::ScreenshotRequest,
        thread::SignalToUI,
        texture::{CxTexturePool, Texture, TextureFormat, TextureUpdated},
        window::CxWindowPool,
    },
    makepad_futures::{
        executor,
        executor::{Executor, Spawner},
    },
    makepad_network::NetworkRuntime,
    makepad_script::*,
    std::{
        any::{Any, TypeId},
        cell::RefCell,
        collections::{HashMap, HashSet},
        rc::Rc,
        sync::Arc,
    },
};

//pub use makepad_shader_compiler::makepad_derive_live::*;
//pub use makepad_shader_compiler::makepad_math::*;

pub struct Cx {
    pub script_vm: Option<Box<ScriptVmBase>>,
    pub script_data: CxScriptData,
    pub package_root: Option<String>,

    pub debug_trace_active: bool,

    pub(crate) os_type: OsType,
    pub in_makepad_studio: bool,
    pub demo_time_repaint: bool,
    pub(crate) gpu_info: GpuInfo,
    pub(crate) xr_capabilities: XrCapabilities,
    pub(crate) cpu_cores: usize,
    pub null_texture: Texture,
    pub null_cube_texture: Texture,
    pub windows: CxWindowPool,
    pub passes: CxDrawPassPool,
    pub draw_lists: CxDrawListPool,
    pub draw_matrices: CxDrawMatrixPool,
    pub textures: CxTexturePool,
    pub(crate) geometries: CxGeometryPool,

    pub draw_shaders: CxDrawShaders,

    pub new_draw_event: DrawEvent,

    pub redraw_id: u64,

    pub(crate) repaint_id: u64,
    pub(crate) event_id: u64,
    pub(crate) timer_id: u64,
    pub(crate) next_frame_id: u64,
    pub(crate) permissions_request_id: i32,

    pub keyboard: CxKeyboard,
    pub fingers: CxFingers,
    pub(crate) ime_area: Area,
    pub keyboard_shift: f64,
    pub(crate) drag_drop: CxDragDrop,

    pub(crate) platform_ops: Vec<CxOsOp>,

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
    pub(crate) event_handler: Option<Box<dyn FnMut(&mut Cx, &Event)>>,

    pub(crate) globals: Vec<(TypeId, Box<dyn Any>)>,

    pub components: ComponentRegistries,

    pub(crate) self_ref: Option<Rc<RefCell<Cx>>>,
    pub(crate) in_draw_event: bool,

    /// Display context for the main window, used by AdaptiveView
    pub display_context: DisplayContext,

    pub debug: Debug,

    #[allow(dead_code)]
    pub(crate) executor: Option<Executor>,
    pub(crate) spawner: Spawner,

    pub(crate) studio_http: String,

    pub performance_stats: PerformanceStats,
    #[allow(unused)]
    pub(crate) screenshot_requests: Vec<ScreenshotRequest>,
    pub(crate) widget_tree_dump_requests: Vec<u64>,
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
        if let OsType::Android(params) = self {
            Some(params.cache_path.clone())
        } else if let OsType::OpenHarmony(params) = self {
            Some(params.cache_dir.clone())
        } else {
            None
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

impl Cx {
    pub fn new(event_handler: Box<dyn FnMut(&mut Cx, &Event)>) -> Self {
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
        let net = Arc::new(NetworkRuntime::new(Default::default()));
        net.set_wake_fn(Some(Arc::new(|| {
            SignalToUI::set_ui_signal();
        })));

        let mut vm = ScriptVm {
            host: &mut 0,
            bx: Box::new(ScriptVmBase::new()),
        };

        //todo!();
        crate::script::script_mod(&mut vm);

        Self {
            package_root: None,
            demo_time_repaint: false,
            null_texture,
            null_cube_texture,
            cpu_cores: 8,
            in_makepad_studio: false,
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

            draw_shaders: Default::default(),

            new_draw_event: Default::default(),
            new_actions: Default::default(),

            redraw_id: 1,
            event_id: 1,
            repaint_id: 1,
            timer_id: 1,
            next_frame_id: 1,
            permissions_request_id: 0,

            keyboard: Default::default(),
            fingers: Default::default(),
            drag_drop: Default::default(),
            ime_area: Default::default(),
            keyboard_shift: 0.0,
            platform_ops: Default::default(),
            studio_http: "".to_string(),
            new_next_frames: Default::default(),

            screenshot_requests: Default::default(),
            widget_tree_dump_requests: Default::default(),

            dependencies: Default::default(),

            triggers: Default::default(),

            action_receiver,

            os: CxOs::default(),

            event_handler: Some(event_handler),

            debug: Default::default(),

            debug_trace_active: false,

            globals: Default::default(),

            components: ComponentRegistries::new(),

            executor: Some(executor),
            spawner,

            self_ref: None,
            performance_stats: Default::default(),

            display_context: Default::default(),

            widget_query_invalidation_event: None,
            widget_tree_ptr: std::ptr::null_mut(),
            widget_tree_dump_callback: None,
            net,

            script_vm: Some(vm.bx),
            script_data: Default::default(),
        }
    }
}

impl Cx {
    pub fn handle_live_edit(&mut self) -> bool {
        false
    }
}
