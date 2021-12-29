use {
    std::{
        collections::{
            HashMap,
            HashSet,
            BTreeSet,
        },
        time::Instant,
        rc::Rc,
        rc::Weak,
        cell::RefCell,
    },
    makepad_shader_compiler::makepad_live_compiler::{
        //LiveType,
        //LiveId,
        LiveEditEvent,
        LiveRegistry
    },
    makepad_shader_compiler::{
        DrawShaderPtr,
        ShaderRegistry
    },
    crate::{
        cx_registries::{
            CxRegistries
        },
        platform::{
            CxPlatform,
            CxPlatformDrawShader,
            CxPlatformTexture,
        },
        event::{
            CxPerFinger,
            NUM_FINGERS,
            Event,
            Signal,
            KeyEvent,
            NextFrame,
        },
        menu::{
            CxCommandSetting,
            CommandId
        },
        cursor::{
            MouseCursor
        },
        area::{
            Area,
        },
        gpu_info::GpuInfo,
        window::{
            CxWindow,
        },
        pass::{
            CxPass,
        },
        view::CxView,
        font::{
            CxFont,
            CxFontsAtlas,
            CxDrawFontAtlas
        },
        texture::{
            CxTexture,
            TextureDesc,
            TextureFormat
        },
        geometry::{
            Geometry,
            CxGeometry,
            GeometryFingerprint
        },
        draw_vars::{
            CxDrawShader,
            DrawShaderFingerprint,
        },
        turtle::Turtle,
    }
};

pub use makepad_shader_compiler::makepad_derive_live::*;
pub use makepad_shader_compiler::makepad_math::*;

pub struct Cx {
    pub platform_type: PlatformType,
    pub gpu_info: GpuInfo,
    
    pub windows: Vec<CxWindow>,
    pub windows_free: Rc<RefCell<Vec<usize >> >,
    
    pub passes: Vec<CxPass>,
    pub passes_free: Rc<RefCell<Vec<usize >> >,
    
    pub views: Vec<CxView>,
    pub views_free: Rc<RefCell<Vec<usize >> >,
    
    pub textures: Vec<CxTexture>,
    pub textures_free: Rc<RefCell<Vec<usize >> >,
    
    pub geometries: Vec<CxGeometry>,
    pub geometries_free: Rc<RefCell<Vec<usize >> >,
    pub geometries_refs: HashMap<GeometryFingerprint, Weak<Geometry >>,
    
    pub platform_draw_shaders: Vec<CxPlatformDrawShader>,
    pub draw_shader_generation: u64, 
    pub draw_shaders: Vec<CxDrawShader>,
    pub draw_shader_ptr_to_id: HashMap<DrawShaderPtr, usize>,
    pub draw_shader_compile_set: BTreeSet<DrawShaderPtr>,
    pub draw_shader_fingerprints: Vec<DrawShaderFingerprint>,
    pub draw_shader_error_set: HashSet<DrawShaderPtr>,
    
    pub fonts: Vec<Option<CxFont >>,
    pub fonts_atlas: CxFontsAtlas,
    pub path_to_font_id: HashMap<String, usize>,
    pub draw_font_atlas: Option<Box<CxDrawFontAtlas >>,
    
    pub in_redraw_cycle: bool, 
    pub default_dpi_factor: f32,
    pub current_dpi_factor: f32,
    pub window_stack: Vec<usize>,
    pub pass_stack: Vec<usize>,
    pub view_stack: Vec<usize>,
    pub turtles: Vec<Turtle>,
    pub align_list: Vec<Area>,
    
    //pub live_factories: Rc<RefCell<HashMap<LiveType, Box<dyn LiveFactory >> >>,
    
    pub registries: CxRegistries,
    
    pub new_redraw_views: Vec<usize>,
    pub new_redraw_views_and_children: Vec<usize>,
    pub new_redraw_all_views: bool,
    pub redraw_views: Vec<usize>,
    pub redraw_views_and_children: Vec<usize>,
    pub redraw_all_views: bool,
    
    pub redraw_id: u64,
    pub repaint_id: u64,
    pub event_id: u64,
    pub timer_id: u64,
    pub next_frame_id: u64,
    pub signal_id: usize,
    
    pub prev_key_focus: Area,
    pub next_key_focus: Area,
    pub key_focus: Area,
    pub keys_down: Vec<KeyEvent>,
    
    pub down_mouse_cursor: Option<MouseCursor>,
    pub hover_mouse_cursor: Option<MouseCursor>,
    pub fingers: Vec<CxPerFinger>,
    
    pub drag_area: Area,
    pub new_drag_area: Area,
    
    pub new_next_frames: HashSet<NextFrame>,
    pub next_frames: HashSet<NextFrame>,
    
    pub signals: HashMap<Signal, Vec<u64 >>,
    pub triggers: HashMap<Area, Vec<u64 >>,
    
    pub profiles: HashMap<u64, Instant>,
    
    pub live_registry: Rc<RefCell<LiveRegistry >>,
    pub shader_registry: ShaderRegistry,
    
    pub live_edit_event: Option<LiveEditEvent>,
    
    pub command_settings: HashMap<CommandId, CxCommandSetting>,
    
    pub platform: CxPlatform,
    // this cuts the compiletime of an end-user application in half
    pub event_handler: Option<*mut dyn FnMut(&mut Cx, &mut Event)>,
}

#[derive(Clone)]
pub enum PlatformType {
    Unknown,
    MsWindows,
    OSX,
    Linux {custom_window_chrome: bool},
    WebBrowser {protocol: String, hostname: String, port: u16, pathname: String, search: String, hash: String}
}

impl PlatformType {
    pub fn is_desktop(&self) -> bool {
        match self {
            PlatformType::Unknown => true,
            PlatformType::MsWindows => true,
            PlatformType::OSX => true,
            PlatformType::Linux {..} => true,
            PlatformType::WebBrowser {..} => false
        }
    }
}

impl Default for Cx {
    fn default() -> Self {
        let mut fingers = Vec::new();
        fingers.resize(NUM_FINGERS, CxPerFinger::default());
        
        // the null texture
        let textures = vec![CxTexture {
            desc: TextureDesc {
                format: TextureFormat::ImageBGRA,
                width: Some(4),
                height: Some(4),
                multisample: None
            },
            image_u32: vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            image_f32: Vec::new(),
            update_image: true,
            platform: CxPlatformTexture::default()
        }];
        
        Self {
            platform_type: PlatformType::Unknown,
            gpu_info: GpuInfo::default(),
            
            windows: Vec::new(),
            windows_free: Rc::new(RefCell::new(Vec::new())),
            
            passes: Vec::new(),
            passes_free: Rc::new(RefCell::new(Vec::new())),
            
            views: Vec::new(),
            views_free: Rc::new(RefCell::new(Vec::new())),
            
            textures: textures,
            textures_free: Rc::new(RefCell::new(Vec::new())),
            
            geometries: Vec::new(),
            geometries_free: Rc::new(RefCell::new(Vec::new())),
            geometries_refs: HashMap::new(),
            
            platform_draw_shaders: Vec::new(),
            
            draw_shader_generation: 0,
            draw_shaders: Vec::new(),
            draw_shader_ptr_to_id: HashMap::new(),
            draw_shader_compile_set: BTreeSet::new(),
            draw_shader_fingerprints: Vec::new(),
            draw_shader_error_set: HashSet::new(),
            
            fonts: Vec::new(),
            fonts_atlas: CxFontsAtlas::new(),
            path_to_font_id: HashMap::new(),
            draw_font_atlas: None,
            
            in_redraw_cycle: false,
            default_dpi_factor: 1.0,
            current_dpi_factor: 1.0,
            window_stack: Vec::new(),
            pass_stack: Vec::new(),
            view_stack: Vec::new(),
            turtles: Vec::new(),
            align_list: Vec::new(),
            
            //live_factories: Rc::new(RefCell::new(HashMap::new())),
            
            new_redraw_views: Vec::new(),
            new_redraw_views_and_children: Vec::new(),
            new_redraw_all_views: true,
            redraw_views: Vec::new(),
            redraw_views_and_children: Vec::new(),
            redraw_all_views: true,
            
            registries: CxRegistries::new(),
            
            redraw_id: 1,
            event_id: 1,
            repaint_id: 1,
            timer_id: 1,
            signal_id: 1,
            next_frame_id: 1,
            
            next_key_focus: Area::Empty,
            prev_key_focus: Area::Empty,
            key_focus: Area::Empty,
            keys_down: Vec::new(),
            
            down_mouse_cursor: None,
            hover_mouse_cursor: None,
            fingers: fingers,
            
            drag_area: Area::Empty,
            new_drag_area: Area::Empty,
            
            new_next_frames: HashSet::new(),
            next_frames: HashSet::new(),
            
            signals: HashMap::new(),
            triggers: HashMap::new(),
            
            profiles: HashMap::new(),
            
            live_registry: Rc::new(RefCell::new(LiveRegistry::default())),
            shader_registry: ShaderRegistry::new(),
            
            command_settings: HashMap::new(),
            
            platform: CxPlatform {..Default::default()},
            
            live_edit_event: None,
             
            event_handler: None
        }
    }
}
