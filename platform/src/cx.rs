use {
    std::{
        collections::{
            HashMap,
            HashSet,
        },
        time::Instant,
        rc::Rc,
        rc::Weak,
        cell::RefCell,
    },
    crate::{
        makepad_live_compiler::{
            id,
            LiveId,
            LiveEditEvent,
            LiveRegistry
        },
        makepad_shader_compiler::{
            ShaderRegistry
        },  
        /*cx_registries::{
            CxRegistries
        },*/
        cx_draw_shaders::{
            CxDrawShaders
        },
        platform::{
            CxPlatform,
            CxPlatformTexture,
        },
        event::{
            DrawEvent,
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
        draw_list::DrawList,
        pass::{
            CxPass,
        },
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
    
    pub draw_lists: Vec<DrawList>,
    pub draw_lists_free: Rc<RefCell<Vec<usize >> >,
    
    pub textures: Vec<CxTexture>,
    pub textures_free: Rc<RefCell<Vec<usize >> >,
    
    pub geometries: Vec<CxGeometry>,
    pub geometries_free: Rc<RefCell<Vec<usize >> >,
    pub geometries_refs: HashMap<GeometryFingerprint, Weak<Geometry >>,
    
    pub draw_shaders: CxDrawShaders,
    
    pub fonts: Vec<Option<CxFont >>,
    pub fonts_atlas: CxFontsAtlas,
    pub path_to_font_id: HashMap<String, usize>,
    pub draw_font_atlas: Option<Box<CxDrawFontAtlas >>,
    
    //pub registries: CxRegistries,
    
    pub new_draw_event: DrawEvent,
    
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
        
        let mut live_registry = LiveRegistry::default();
        live_registry.add_ignore_no_dsl(&[
            id!(Margin),
            id!(Walk),
            id!(Align),
            id!(Axis),
            id!(Layout),
            id!(Padding),
            id!(f32),
            id!(usize),
            id!(f64),
            id!(bool),
            id!(DrawVars),
            id!(Vec2),
            id!(Vec3),
            id!(Vec4),
            id!(LivePtr),
            id!(String),
            id!(View),
            id!(Pass),
            id!(Texture),
            id!(Window),
            id!(TextStyle),
            id!(Wrapping),
        ]);
        
        Self {
            platform_type: PlatformType::Unknown,
            gpu_info: GpuInfo::default(),
            
            windows: Vec::new(),
            windows_free: Rc::new(RefCell::new(Vec::new())),
            
            passes: Vec::new(),
            passes_free: Rc::new(RefCell::new(Vec::new())),
            
            draw_lists: Vec::new(),
            draw_lists_free: Rc::new(RefCell::new(Vec::new())),
            
            textures: textures,
            textures_free: Rc::new(RefCell::new(Vec::new())),
            
            geometries: Vec::new(),
            geometries_free: Rc::new(RefCell::new(Vec::new())),
            geometries_refs: HashMap::new(),
            
            draw_shaders: CxDrawShaders::default(),
            
            fonts: Vec::new(),
            fonts_atlas: CxFontsAtlas::new(),
            path_to_font_id: HashMap::new(),
            draw_font_atlas: None,
            
            //registries: CxRegistries::new(),
            
            new_draw_event: DrawEvent::default(),
            
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
            
            signals: HashMap::new(),
            triggers: HashMap::new(),
            
            profiles: HashMap::new(),
            
            live_registry: Rc::new(RefCell::new(live_registry)),
            shader_registry: ShaderRegistry::new(),
            
            command_settings: HashMap::new(),
            
            platform: CxPlatform {..Default::default()},
            
            live_edit_event: None,
            
            event_handler: None
        }
    }
}
