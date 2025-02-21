use {
    makepad_futures::{executor, executor::{Executor, Spawner}},
    std::{
        collections::{
            HashMap,
            HashSet,
        },
        any::{Any, TypeId},
        rc::Rc,
        rc::Weak,
        cell::RefCell,
    },
    crate::{
        action::{ActionSendSync,ACTION_SENDER_GLOBAL},
        makepad_live_compiler::{
            LiveRegistry,
            LiveFileChange
        },
        makepad_shader_compiler::ShaderRegistry,
        draw_shader::CxDrawShaders,
        draw_matrix::CxDrawMatrixPool,
        os::CxOs,
        debug::Debug,
        display_context::DisplayContext,
        performance_stats::PerformanceStats,
        event::{
            DrawEvent,
            CxFingers,
            CxDragDrop,
            Event,
            Trigger,
            CxKeyboard,
            NextFrame,
        },
        action::ActionsBuf,
        cx_api::CxOsOp,
        area::Area,
        gpu_info::GpuInfo,
        window::CxWindowPool,
        draw_list::CxDrawListPool,
        web_socket::WebSocket,
        pass::CxPassPool,
        texture::{CxTexturePool,TextureFormat,Texture,TextureUpdated},
        geometry::{
            Geometry,
            CxGeometryPool,
            GeometryFingerprint
        },
    }
};

//pub use makepad_shader_compiler::makepad_derive_live::*;
//pub use makepad_shader_compiler::makepad_math::*;
 
pub struct Cx {
    pub (crate) os_type: OsType,
    pub in_makepad_studio: bool,
    pub demo_time_repaint: bool,
    pub (crate) gpu_info: GpuInfo,
    pub (crate) xr_capabilities: XrCapabilities,
    pub (crate) cpu_cores: usize,
    pub null_texture: Texture,
    pub windows: CxWindowPool,
    pub passes: CxPassPool,
    pub draw_lists: CxDrawListPool,
    pub draw_matrices: CxDrawMatrixPool,
    pub textures: CxTexturePool,
    pub (crate) geometries: CxGeometryPool,
    pub (crate) geometries_refs: HashMap<GeometryFingerprint, Weak<Geometry >>, 
    
    pub draw_shaders: CxDrawShaders,
    
    pub new_draw_event: DrawEvent,
    
    pub redraw_id: u64,
    
    pub (crate) repaint_id: u64,
    pub (crate) event_id: u64,
    pub (crate) timer_id: u64,
    pub (crate) next_frame_id: u64,
    
    pub keyboard: CxKeyboard,
    pub fingers: CxFingers,
    pub (crate) ime_area: Area,
    pub (crate) drag_drop: CxDragDrop,
    
    pub (crate) platform_ops: Vec<CxOsOp>,
    
    pub (crate) new_next_frames: HashSet<NextFrame>,
    
    pub new_actions: ActionsBuf,
    
    pub (crate) dependencies: HashMap<String, CxDependency>,
    
    pub (crate) triggers: HashMap<Area, Vec<Trigger >>,
    
    pub live_registry: Rc<RefCell<LiveRegistry >>,

    pub (crate) live_file_change_receiver: std::sync::mpsc::Receiver<Vec<LiveFileChange>>,
    pub (crate) live_file_change_sender: std::sync::mpsc::Sender<Vec<LiveFileChange >>,
    
    pub (crate) action_receiver: std::sync::mpsc::Receiver<ActionSendSync>,
    
    pub shader_registry: ShaderRegistry,
    
    pub os: CxOs,
    // (cratethis cuts the compiletime of an end-user application in half
    pub (crate) event_handler: Option<Box<dyn FnMut(&mut Cx, &Event) >>,
    
    pub (crate) globals: Vec<(TypeId, Box<dyn Any>)>,

    pub (crate) self_ref: Option<Rc<RefCell<Cx>>>,
    pub (crate) in_draw_event: bool,

    pub display_context: DisplayContext,
    
    pub debug: Debug,

    #[allow(dead_code)]
    pub(crate) executor: Option<Executor>,
    pub(crate) spawner: Spawner,
    
    pub(crate) studio_web_socket: Option<WebSocket>,
    pub(crate) studio_http: String,
    
    pub performance_stats: PerformanceStats,

    /// Event ID that triggered a widget query cache invalidation.
    /// When Some(event_id), indicates that widgets should clear their query caches
    /// on the next event loop cycle. This ensures all views process the cache clear
    /// before it's reset to None.
    /// 
    /// This is primarily used when adaptive views change their active variant,
    /// as the widget hierarchy changes require parent views to rebuild their widget queries.
    pub widget_query_invalidation_event: Option<u64>,
}

#[derive(Clone)]
pub struct CxRef(pub Rc<RefCell<Cx>>);

pub struct CxDependency {
    pub data: Option<Result<Rc<Vec<u8>>, String >>
}
#[derive(Clone, Debug)]
pub struct AndroidParams {
    pub cache_path: String,
    pub density: f64,
    pub is_emulator: bool,
    pub android_version: String,
    pub build_number: String,
    pub kernel_version: String
}

#[derive(Clone, Debug)]
pub struct OpenHarmonyParams {
    pub files_dir:String,
    pub cache_dir:String,
    pub temp_dir:String,
    pub device_type: String,
    pub os_full_name: String,
    pub display_density: f64,
}

#[derive(Clone, Debug)]
pub struct WebParams {
    pub protocol: String,
    pub host: String,
    pub hostname: String,
    pub pathname: String,
    pub search: String,
    pub hash: String
}

#[derive(Clone, Debug)]
pub struct LinuxWindowParams {
    pub custom_window_chrome: bool,
}

#[derive(Clone, Debug)]
pub enum OsType {
    Unknown,
    Windows,
    Macos,
    Ios,
    Android(AndroidParams),
    OpenHarmony(OpenHarmonyParams),
    LinuxWindow (LinuxWindowParams),
    LinuxDirect,
    Web(WebParams)
}

#[derive(Default)]
pub struct XrCapabilities {
    pub ar_supported: bool,
    pub vr_supported: bool,
}

impl OsType {
    pub fn is_single_window(&self)->bool{
        match self{
            OsType::Web(_) => true,
            OsType::Ios=>true,
            OsType::Android(_) => true,
            OsType::LinuxDirect=> true,
            _=> false
        }
    }
    pub fn is_web(&self) -> bool {
        match self {
            OsType::Web(_) => true,
            _ => false
        }
    }
    
    
    pub fn get_cache_dir(&self)->Option<String>{
        if let OsType::Android(params) = self {
            Some(params.cache_path.clone())
        }
        else if let OsType::OpenHarmony(params) = self {
            Some(params.cache_dir.clone())
        }
        else {
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
        
        let (executor, spawner) = executor::new_executor_and_spawner();
        let (live_file_change_sender, live_file_change_receiver) = std::sync::mpsc::channel();
        let (action_sender, action_receiver) = std::sync::mpsc::channel();
        if let Ok(mut sender) = ACTION_SENDER_GLOBAL.lock(){
            *sender = Some(action_sender);
        }
        
        Self {
            demo_time_repaint: false,
            null_texture,
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
            geometries_refs: Default::default(),
            
            draw_shaders: Default::default(),
            
            new_draw_event: Default::default(),
            new_actions: Default::default(),
            
            redraw_id: 1,
            event_id: 1,
            repaint_id: 1,
            timer_id: 1,
            next_frame_id: 1,
            
            keyboard: Default::default(),
            fingers: Default::default(),
            drag_drop: Default::default(),
            ime_area: Default::default(),
            platform_ops: Default::default(),
            studio_web_socket: None,
            studio_http: "".to_string(),
            new_next_frames: Default::default(),
            
            dependencies: Default::default(),
            
            triggers: Default::default(),
            
            live_registry: Rc::new(RefCell::new(LiveRegistry::default())),
            
            live_file_change_receiver,
            live_file_change_sender,
            action_receiver,
            
            shader_registry: ShaderRegistry::new(true),
            
            os: CxOs::default(),
            
            event_handler: Some(event_handler),
            
            debug: Default::default(),
            
            globals: Default::default(),

            executor: Some(executor),
            spawner,

            self_ref: None,
            performance_stats: Default::default(),

            display_context: Default::default(),

            widget_query_invalidation_event: None,
        }
    }
}