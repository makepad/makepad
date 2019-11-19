use std::collections::HashMap;
use std::cell::RefCell;

pub use crate::shadergen::*;
pub use crate::fonts::*;
pub use crate::turtle::*;
pub use crate::cursor::*;
pub use crate::window::*;
pub use crate::view::*;
pub use crate::pass::*;
pub use crate::texture::*;
pub use crate::text::*;
pub use crate::shader::*;

pub use crate::math::*;
pub use crate::events::*;
pub use crate::colors::*;
pub use crate::elements::*;
pub use crate::animator::*;
pub use crate::area::*;
pub use crate::menu::*;
pub use crate::theming::*;

#[cfg(all(not(feature = "ipc"), target_os = "linux"))]
pub use crate::cx_linux::*;
#[cfg(all(not(feature = "ipc"), target_os = "linux"))]
pub use crate::cx_opengl::*;

#[cfg(all(not(feature = "ipc"), target_os = "macos"))]
pub use crate::cx_macos::*;
#[cfg(all(not(feature = "ipc"), target_os = "macos"))]
pub use crate::cx_metal::*;
#[cfg(all(not(feature = "ipc"), target_os = "macos"))]
pub use crate::cx_metalsl::*;

#[cfg(all(not(feature="ipc"),target_os = "windows"))]
pub use crate::cx_windows::*;
#[cfg(all(not(feature="ipc"),target_os = "windows"))]
pub use crate::cx_dx11::*;
#[cfg(all(not(feature = "ipc"), target_os = "windows"))]
pub use crate::cx_hlsl::*;

#[cfg(all(not(feature = "ipc"), target_arch = "wasm32"))]
pub use crate::cx_webgl::*;

#[cfg(all(not(feature = "ipc"), any(target_arch = "wasm32", target_os = "linux")))]
pub use crate::cx_glsl::*;

#[cfg(all(not(feature = "ipc"), any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use crate::cx_desktop::*;

#[cfg(all(not(feature = "ipc"), target_arch = "wasm32"))]
pub use crate::cx_wasm32::*;

#[cfg(feature = "ipc")]
pub use crate::cx_ipc_child::*;

#[cfg(all(feature = "ipc", target_arch = "wasm32"))]
pub use crate::cx_ipc_wasm32::*;

#[cfg(all(feature = "ipc", any(target_os = "linux", target_os = "macos")))]
pub use crate::cx_ipc_posix::*;

#[cfg(all(feature = "ipc", target_os = "windows"))]
pub use crate::cx_ipc_win32::*;

pub enum PlatformType {
    Windows,
    OSX,
    Linux,
    WASM
}

impl PlatformType {
    pub fn is_desktop(&self) -> bool {
        match self {
            PlatformType::Windows => true,
            PlatformType::OSX => true,
            PlatformType::Linux => true,
            PlatformType::WASM => false
        }
    }
}

pub struct Cx {
    pub running: bool,
    pub platform_type: PlatformType,

    pub windows: Vec<CxWindow>,
    pub windows_free: Vec<usize>,
    pub passes: Vec<CxPass>,
    pub passes_free: Vec<usize>,
    pub views: Vec<CxView>,
    pub views_free: Vec<usize>,

    pub fonts: Vec<CxFont>,
    pub fonts_atlas: CxFontsAtlas,
    pub textures: Vec<CxTexture>,
    pub textures_free: Vec<usize>,
    pub shaders: Vec<CxShader>,
    pub shader_map: HashMap<ShaderGen, usize>,
    pub shader_instance_id: usize,

    pub str_to_id: RefCell<HashMap<String, usize>>,
    pub id_to_str: RefCell<HashMap<usize, String>>,

    pub is_in_redraw_cycle: bool,
    pub vr_can_present: bool,
    pub default_dpi_factor: f32,
    pub current_dpi_factor: f32,
    pub window_stack: Vec<usize>,
    pub pass_stack: Vec<usize>,
    pub view_stack: Vec<usize>,
    pub turtles: Vec<Turtle>,
    pub align_list: Vec<Area>,

    pub redraw_child_areas: Vec<Area>,
    pub redraw_parent_areas: Vec<Area>,
    pub _redraw_child_areas: Vec<Area>,
    pub _redraw_parent_areas: Vec<Area>,

    pub redraw_id: u64,
    pub repaint_id: u64,
    pub event_id: u64,
    pub timer_id: u64,
    pub signal_id: usize,
    pub theme_update_id: usize,
    
    pub last_key_focus: Area,
    pub key_focus: Area,
    pub keys_down: Vec<KeyEvent>,

    pub debug_area: Area,

    pub down_mouse_cursor: Option<MouseCursor>,
    pub hover_mouse_cursor: Option<MouseCursor>,
    pub captured_fingers: Vec<Area>,
    pub finger_tap_count: Vec<(Vec2, f64, u32)>,
    pub finger_down_abs_start: Vec<Vec2>,
    pub finger_down_rel_start: Vec<Vec2>,
    pub finger_over_last_area: Area,
    pub _finger_over_last_area: Area,

    pub playing_anim_areas: Vec<AnimArea>,
    pub ended_anim_areas: Vec<AnimArea>,

    pub frame_callbacks: Vec<Area>,
    pub _frame_callbacks: Vec<Area>,

    pub signals: Vec<(Signal, usize)>,

    pub theme_colors: HashMap<(ColorId,ClassId), Color>,
    pub theme_text_styles: HashMap<(TextStyleId,ClassId), TextStyle>,
    pub theme_layouts: HashMap<(LayoutId,ClassId), Layout>,
    pub theme_walks: HashMap<(WalkId,ClassId), Walk>, 
    pub theme_anims: HashMap<(AnimId,ClassId), Anim>, 
    pub theme_shaders: HashMap<(ShaderId,ClassId), Shader>,
    pub panic_now: bool,
    pub panic_redraw: bool,
    
    pub platform: CxPlatform,
}

pub const NUM_FINGERS: usize = 10;

impl Default for Cx {
    fn default() -> Self {
        let mut captured_fingers = Vec::new();
        let mut finger_tap_count = Vec::new();
        let mut finger_down_abs_start = Vec::new();
        let mut finger_down_rel_start = Vec::new();

        captured_fingers.resize(NUM_FINGERS, Area::Empty);
        finger_tap_count.resize(NUM_FINGERS, (Vec2::zero(), 0.0, 0));
        finger_down_abs_start.resize(NUM_FINGERS, Vec2::zero());
        finger_down_rel_start.resize(NUM_FINGERS, Vec2::zero());

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
            platform_type: PlatformType::Windows,
            running: true,

            windows: Vec::new(),
            windows_free: Vec::new(),
            passes: Vec::new(),
            passes_free: Vec::new(),
            views: vec![CxView {..Default::default()}],
            views_free: Vec::new(),

            fonts: Vec::new(),
            fonts_atlas: CxFontsAtlas::default(),
            textures: textures,
            textures_free: Vec::new(),
            shaders: Vec::new(),
            shader_map: HashMap::new(),
            id_to_str: RefCell::new(HashMap::new()),
            str_to_id: RefCell::new(HashMap::new()),

            default_dpi_factor: 1.0,
            current_dpi_factor: 1.0,
            is_in_redraw_cycle: false,
            vr_can_present: false,
            window_stack: Vec::new(),
            pass_stack: Vec::new(),
            view_stack: Vec::new(),
            turtles: Vec::new(),
            align_list: Vec::new(),

            redraw_parent_areas: Vec::new(),
            _redraw_parent_areas: Vec::new(),
            redraw_child_areas: Vec::new(),
            _redraw_child_areas: Vec::new(),

            redraw_id: 1,
            event_id: 1,
            repaint_id: 1,
            timer_id: 1,
            signal_id: 1,
            shader_instance_id: 1,
            theme_update_id: 1, 
            
            last_key_focus: Area::Empty,
            key_focus: Area::Empty,
            keys_down: Vec::new(),

            debug_area: Area::Empty,

            down_mouse_cursor: None,
            hover_mouse_cursor: None,
            captured_fingers: captured_fingers,
            finger_tap_count: finger_tap_count,
            finger_down_abs_start: finger_down_abs_start,
            finger_down_rel_start: finger_down_rel_start,
            finger_over_last_area: Area::Empty,
            _finger_over_last_area: Area::Empty,

            theme_colors: HashMap::new(),
            theme_text_styles: HashMap::new(),
            theme_layouts: HashMap::new(),
            theme_walks: HashMap::new(),
            theme_anims: HashMap::new(),
            theme_shaders: HashMap::new(),

            playing_anim_areas: Vec::new(),
            ended_anim_areas: Vec::new(),

            frame_callbacks: Vec::new(),
            _frame_callbacks: Vec::new(),

            //custom_before_draw:Vec::new(),
            signals: Vec::new(),

            panic_now: false,
            panic_redraw: false,

            platform: CxPlatform {..Default::default()},
        }
    }
}

#[derive(Default, PartialEq, Copy, Clone, Debug)]
pub struct CxId(usize);

impl Cx {
    
        
    pub fn add_shader(&mut self, sg: ShaderGen, name: &str) -> Shader {
        let inst_id = self.shader_instance_id;
        self.shader_instance_id += 1;
        if let Some(stored_id) = self.shader_map.get(&sg) {
            return Shader {shader_id: Some((*stored_id, inst_id))}
        }

        let new_id = self.shaders.len();
        self.shader_map.insert(sg.clone(), new_id);
        self.shaders.push(CxShader {
            name: name.to_string(),
            shader_gen: sg,
            platform: None,
            mapping: CxShaderMapping::default()
        });
        Shader {shader_id: Some((new_id, inst_id))}
    }

    pub fn process_tap_count(&mut self, digit: usize, pos: Vec2, time: f64) -> u32 {
        if digit >= self.finger_tap_count.len() {
            return 0
        };
        let (last_pos, last_time, count) = self.finger_tap_count[digit];

        if (time - last_time) < 0.5 && pos.distance(&last_pos) < 10. {
            self.finger_tap_count[digit] = (pos, time, count + 1);
            count + 1
        }
        else {
            self.finger_tap_count[digit] = (pos, time, 1);
            1
        }
    }
    
    pub fn get_dpi_factor_of(&mut self, area: &Area) -> f32 {
        match area {
            Area::Instance(ia) => {
                let pass_id = self.views[ia.view_id].pass_id;
                return self.get_delegated_dpi_factor(pass_id)
            },
            Area::View(va) => {
                let pass_id = self.views[va.view_id].pass_id;
                return self.get_delegated_dpi_factor(pass_id)
            },
            _ => ()
        }
        return 1.0;
    }

    pub fn get_delegated_dpi_factor(&mut self, pass_id: usize) -> f32 {
        let mut dpi_factor = 1.0;
        let mut pass_id_walk = pass_id;
        for _ in 0..25 {
            match self.passes[pass_id_walk].dep_of {
                CxPassDepOf::Window(window_id) => {
                    dpi_factor = match self.windows[window_id].window_state {
                        CxWindowState::Create {..} => {
                            self.default_dpi_factor
                        },
                        CxWindowState::Created => {
                            self.windows[window_id].window_geom.dpi_factor
                        },
                        _ => 1.0
                    };
                    break;
                },
                CxPassDepOf::Pass(next_pass_id) => {
                    pass_id_walk = next_pass_id;
                },
                _ => {break;}
            }
        }
        dpi_factor
    }

    pub fn compute_passes_to_repaint(&mut self, passes_todo: &mut Vec<usize>, windows_need_repaint: &mut usize) {
        passes_todo.truncate(0);

        for (pass_id, cxpass) in self.passes.iter().enumerate() {
            if cxpass.paint_dirty {
                let mut inserted = false;
                match cxpass.dep_of {
                    CxPassDepOf::Window(_) => {
                        *windows_need_repaint += 1
                    },
                    CxPassDepOf::Pass(dep_of_pass_id) => {
                        for insert_before in 0..passes_todo.len() {
                            if passes_todo[insert_before] == dep_of_pass_id {
                                passes_todo.insert(insert_before, pass_id);
                                inserted = true;
                                break;
                            }
                        }
                    },
                    CxPassDepOf::None => { // we need to be first
                        passes_todo.insert(0, pass_id);
                        inserted = true;
                    },
                }
                if !inserted {
                    passes_todo.push(pass_id);
                }
            }
        }
    }

    pub fn redraw_pass_of(&mut self, area: Area) {
        // we walk up the stack of area
        match area {
            Area::All => {
                for window_id in 0..self.windows.len() {
                    let redraw = match self.windows[window_id].window_state {
                        CxWindowState::Create {..} | CxWindowState::Created => {
                            true
                        },
                        _ => false
                    };
                    if redraw {
                        if let Some(pass_id) = self.windows[window_id].main_pass_id {
                            self.redraw_pass_and_dep_of_passes(pass_id);
                        }
                    }
                }
            },
            Area::Empty => (),
            Area::Instance(instance) => {
                self.redraw_pass_and_dep_of_passes(self.views[instance.view_id].pass_id);
            },
            Area::View(viewarea) => {
                self.redraw_pass_and_dep_of_passes(self.views[viewarea.view_id].pass_id);
            }
        }
    }

    pub fn redraw_pass_and_dep_of_passes(&mut self, pass_id: usize) {
        let mut walk_pass_id = pass_id;
        loop {
            if let Some(main_view_id) = self.passes[walk_pass_id].main_view_id {
                self.redraw_parent_area(Area::View(ViewArea {redraw_id: 0, view_id: main_view_id}));
            }
            match self.passes[walk_pass_id].dep_of.clone() {
                CxPassDepOf::Pass(next_pass_id) => {
                    walk_pass_id = next_pass_id;
                },
                _ => {
                    break;
                }
            }
        }
    }

    pub fn redraw_pass_and_sub_passes(&mut self, pass_id: usize) {
        let cxpass = &self.passes[pass_id];
        if let Some(main_view_id) = cxpass.main_view_id {
            self.redraw_parent_area(Area::View(ViewArea {redraw_id: 0, view_id: main_view_id}));
        }
        // lets redraw all subpasses as well
        for sub_pass_id in 0..self.passes.len() {
            if let CxPassDepOf::Pass(dep_pass_id) = self.passes[sub_pass_id].dep_of.clone() {
                if dep_pass_id == pass_id {
                    self.redraw_pass_and_sub_passes(sub_pass_id);
                }
            }
        }
    }

    pub fn redraw_child_area(&mut self, area: Area) {
        if self.panic_redraw {
            #[cfg(debug_assertions)]
            panic!("Panic Redraw triggered")
        }

        // if we are redrawing all, clear the rest
        if area == Area::All {
            self.redraw_child_areas.truncate(0);
        }
        // check if we are already redrawing all
        else if self.redraw_child_areas.len() == 1 && self.redraw_child_areas[0] == Area::All {
            return;
        };
        // only add it if we dont have it already
        if let Some(_) = self.redraw_child_areas.iter().position( | a | *a == area) {
            return;
        }
        self.redraw_child_areas.push(area);
    }

    pub fn redraw_parent_area(&mut self, area: Area) {
        if self.panic_redraw {
            #[cfg(debug_assertions)]
            panic!("Panic Redraw triggered")
        }

        // if we are redrawing all, clear the rest
        if area == Area::All {
            self.redraw_parent_areas.truncate(0);
        }
        // check if we are already redrawing all
        else if self.redraw_parent_areas.len() == 1 && self.redraw_parent_areas[0] == Area::All {
            return;
        };
        // only add it if we dont have it already
        if let Some(_) = self.redraw_parent_areas.iter().position( | a | *a == area) {
            return;
        }
        self.redraw_parent_areas.push(area);
    }

    pub fn redraw_previous_areas(&mut self) {
        for area in self._redraw_child_areas.clone() {
            self.redraw_child_area(area);
        }
        for area in self._redraw_parent_areas.clone() {
            self.redraw_parent_area(area);
        }
    }

    pub fn view_will_redraw(&self, view_id: usize) -> bool {

        // figure out if areas are in some way a child of draw_list_id, then we need to redraw
        for area in &self._redraw_child_areas {
            match area {
                Area::All => {
                    return true;
                },
                Area::Empty => (),
                Area::Instance(instance) => {
                    let mut vw = instance.view_id;
                    if vw == view_id {
                        return true
                    }
                    while vw != 0 {
                        vw = self.views[vw].nesting_view_id;
                        if vw == view_id {
                            return true
                        }
                    }
                },
                Area::View(viewarea) => {
                    let mut vw = viewarea.view_id;
                    if vw == view_id {
                        return true
                    }
                    while vw != 0 {
                        vw = self.views[vw].nesting_view_id;
                        if vw == view_id {
                            return true
                        }
                    }

                }
            }
        }
        // figure out if areas are in some way a parent of draw_list_id, then redraw
        for area in &self._redraw_parent_areas {
            match area {
                Area::All => {
                    return true;
                },
                Area::Empty => (),
                Area::Instance(instance) => {
                    let mut vw = view_id;
                    if vw == instance.view_id {
                        return true
                    }
                    while vw != 0 {
                        vw = self.views[vw].nesting_view_id;
                        if vw == instance.view_id {
                            return true
                        }
                    }
                },
                Area::View(viewarea) => {
                    let mut vw = view_id;
                    if vw == viewarea.view_id {
                        return true
                    }
                    while vw != 0 {
                        vw = self.views[vw].nesting_view_id;
                        if vw == viewarea.view_id {
                            return true
                        }
                    }

                }
            }
        }

        false
    }



    pub fn check_ended_anim_areas(&mut self, time: f64) {
        let mut i = 0;
        self.ended_anim_areas.truncate(0);
        loop {
            if i >= self.playing_anim_areas.len() {
                break
            }
            let anim_start_time = self.playing_anim_areas[i].start_time;
            let anim_total_time = self.playing_anim_areas[i].total_time;
            if anim_start_time.is_nan() || time - anim_start_time >= anim_total_time {
                self.ended_anim_areas.push(self.playing_anim_areas.remove(i));
            }
            else {
                i = i + 1;
            }
        }
    }

    pub fn update_area_refs(&mut self, old_area: Area, new_area: Area) {
        if old_area == Area::Empty || old_area == Area::All {
            return
        }

        if let Some(anim_anim) = self.playing_anim_areas.iter_mut().find( | v | v.area == old_area) {
            anim_anim.area = new_area.clone()
        }

        if let Some(digit_area) = self.captured_fingers.iter_mut().find( | v | **v == old_area) {
            *digit_area = new_area.clone()
        }
        // update capture keyboard
        if self.key_focus == old_area {
            self.key_focus = new_area.clone()
        }

        if self._finger_over_last_area == old_area {
            self._finger_over_last_area = new_area.clone()
        }
        //
        if let Some(next_frame) = self.frame_callbacks.iter_mut().find( | v | **v == old_area) {
            *next_frame = new_area.clone()
        }
    }

    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.key_focus = focus_area;
    }

    pub fn has_key_focus(&self, focus_area: Area) -> bool {
        self.key_focus == focus_area
    }

    pub fn process_key_down(&mut self, key_event: KeyEvent) {
        if let Some(_) = self.keys_down.iter().position( | k | k.key_code == key_event.key_code) {
            return;
        }
        self.keys_down.push(key_event);
    }

    pub fn process_key_up(&mut self, key_event: &KeyEvent) {
        for i in 0..self.keys_down.len() {
            if self.keys_down[i].key_code == key_event.key_code {
                self.keys_down.remove(i);
                return
            }
        }
    }

    pub fn call_all_keys_up<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        let mut keys_down = Vec::new();
        std::mem::swap(&mut keys_down, &mut self.keys_down);
        for key_event in keys_down {
            self.call_event_handler(&mut event_handler, &mut Event::KeyUp(key_event))
        }
    }

    // event handler wrappers

    pub fn call_event_handler<F>(&mut self, mut event_handler: F, event: &mut Event)
    where F: FnMut(&mut Cx, &mut Event)
    {
        self.event_id += 1;
        event_handler(self, event);

        if self.last_key_focus != self.key_focus {
            let last_key_focus = self.last_key_focus;
            self.last_key_focus = self.key_focus;
            event_handler(self, &mut Event::KeyFocus(KeyFocusEvent {
                last: last_key_focus,
                focus: self.key_focus
            }))
        }
    }

    pub fn call_draw_event<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        // self.profile();
        self.is_in_redraw_cycle = true;
        self.redraw_id += 1;
        std::mem::swap(&mut self._redraw_child_areas, &mut self.redraw_child_areas);
        std::mem::swap(&mut self._redraw_parent_areas, &mut self.redraw_parent_areas);
        self.align_list.truncate(0);
        self.redraw_child_areas.truncate(0);
        self.redraw_parent_areas.truncate(0);
        self.call_event_handler(&mut event_handler, &mut Event::Draw);
        self.is_in_redraw_cycle = false;
        //self.profile();
    }

    pub fn call_animation_event<F>(&mut self, mut event_handler: F, time: f64)
    where F: FnMut(&mut Cx, &mut Event)
    {
        self.call_event_handler(&mut event_handler, &mut Event::Animate(AnimateEvent {time: time, frame: self.repaint_id}));
        self.check_ended_anim_areas(time);
        if self.ended_anim_areas.len() > 0 {
            self.call_event_handler(&mut event_handler, &mut Event::AnimEnded(AnimateEvent {time: time, frame: self.repaint_id}));
        }
    }

    pub fn call_frame_event<F>(&mut self, mut event_handler: F, time: f64)
    where F: FnMut(&mut Cx, &mut Event)
    {
        std::mem::swap(&mut self._frame_callbacks, &mut self.frame_callbacks);
        self.frame_callbacks.truncate(0);
        self.call_event_handler(&mut event_handler, &mut Event::Frame(FrameEvent {time: time, frame: self.repaint_id}));
    }

    pub fn next_frame(&mut self, area: Area) {
        if let Some(_) = self.frame_callbacks.iter().position( | a | *a == area) {
            return;
        }
        self.frame_callbacks.push(area);
    }

    pub fn new_signal(&mut self) -> Signal {
        self.signal_id += 1;
        return Signal {signal_id: self.signal_id}
    }

    pub fn send_signal(&mut self, signal: Signal, message: usize) {
        self.signals.push((signal, message));
    }

    pub fn call_signals<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        let mut counter = 0;
        while self.signals.len() != 0 {
            counter += 1;
            let mut signals = Vec::new();
            std::mem::swap(&mut self.signals, &mut signals);

            for (signal, value) in signals {
                self.call_event_handler(&mut event_handler, &mut Event::Signal(SignalEvent {
                    signal_id: signal.signal_id,
                    value: value
                }));
            }
            if counter > 100 {
                println!("Signal feedback loop detected");
                break
            }
        }
    }

    /*
    pub fn debug_draw_tree_recur(&mut self, draw_list_id: usize, depth:usize){
        if draw_list_id >= self.draw_lists.len(){
            println!("---------- Drawlist still empty ---------");
            return
        }
        let mut indent = String::new();
        for _i in 0..depth{
            indent.push_str("  ");
        }
        let draw_calls_len = self.draw_lists[draw_list_id].draw_calls_len;
        if draw_list_id == 0{
            println!("---------- Begin Debug draw tree for redraw_id: {} ---------", self.redraw_id)
        }
        println!("{}list {}: len:{} rect:{:?}", indent, draw_list_id, draw_calls_len, self.draw_lists[draw_list_id].rect);
        indent.push_str("  ");
        for draw_call_id in 0..draw_calls_len{
            let sub_list_id = self.draw_lists[draw_list_id].draw_calls[draw_call_id].sub_list_id;
            if sub_list_id != 0{
                self.debug_draw_tree_recur(sub_list_id, depth + 1);
            }
            else{
                let draw_list = &mut self.draw_lists[draw_list_id];
                let draw_call = &mut draw_list.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let shc = &self.compiled_shaders[draw_call.shader_id];
                let slots = shc.instance_slots;
                let instances = draw_call.instance.len() / slots;
                println!("{}call {}: {}({}) x:{}", indent, draw_call_id, sh.name, draw_call.shader_id, instances);
                // lets dump the instance geometry
                for inst in 0..instances.min(1){
                    let mut out = String::new();
                    let mut off = 0;
                    for prop in &shc.named_instance_props.props{
                        match prop.slots{
                            1=>out.push_str(&format!("{}:{} ", prop.name,
                                draw_call.instance[inst*slots + off])),
                            2=>out.push_str(&format!("{}:v2({},{}) ", prop.name,
                                draw_call.instance[inst*slots+ off],
                                draw_call.instance[inst*slots+1+ off])),
                            3=>out.push_str(&format!("{}:v3({},{},{}) ", prop.name,
                                draw_call.instance[inst*slots+ off],
                                draw_call.instance[inst*slots+1+ off],
                                draw_call.instance[inst*slots+1+ off])),
                            4=>out.push_str(&format!("{}:v4({},{},{},{}) ", prop.name,
                                draw_call.instance[inst*slots+ off],
                                draw_call.instance[inst*slots+1+ off],
                                draw_call.instance[inst*slots+2+ off],
                                draw_call.instance[inst*slots+3+ off])),
                            _=>{}
                        }
                        off += prop.slots;
                    }
                    println!("  {}instance {}: {}", indent, inst, out);
                }
            }
        }
        if draw_list_id == 0{
            println!("---------- End Debug draw tree for redraw_id: {} ---------", self.redraw_id)
        }
    }*/
}

// palette types


#[derive(Clone)]
pub enum StyleValue {
    Color(Color),
    Font(String),
    Size(f64)
}

#[macro_export]
macro_rules!log {
    ( $ ( $ arg: tt) *) => ({
        $ crate::Cx::write_log(&format!("[{}:{}:{}] {}\n", file!(), line!(), column!(), &format!( $ ( $ arg) *)))
    })
}

#[macro_export]
macro_rules!main_app {
    ( $ app: ident) => {
        //TODO do this with a macro to generate both entrypoints for App and Cx
        pub fn main() {
            let mut cx = Cx::default();
            let mut app = $ app::proto(&mut cx);
            let mut cxafterdraw = CxAfterDraw::proto(&mut cx);
            cx.event_loop( | cx, mut event | {
                if let Event::Draw = event {
                    app.draw_app(cx);
                    cxafterdraw.after_draw(cx);
                    return
                }
                app.handle_app(cx, &mut event);
            });
        }

        #[export_name = "create_wasm_app"]
        pub extern "C" fn create_wasm_app() -> u32 {
            let mut cx = Box::new(Cx::default());
            let app = Box::new( $ app::proto(&mut cx));
            let cxafterdraw = Box::new(CxAfterDraw::proto(&mut cx));
            Box::into_raw(Box::new((Box::into_raw(app), Box::into_raw(cx), Box::into_raw(cxafterdraw)))) as u32
        }

        #[export_name = "process_to_wasm"]
        pub unsafe extern "C" fn process_to_wasm(appcx: u32, msg_bytes: u32) -> u32 {
            let appcx = &*(appcx as *mut (*mut $ app, *mut Cx, *mut CxAfterDraw));
            (*appcx.1).process_to_wasm(msg_bytes, | cx, mut event | {
                if let Event::Draw = event {
                    (*appcx.0).draw_app(cx);
                    (*appcx.2).after_draw(cx);
                    return;
                };
                (*appcx.0).handle_app(cx, &mut event);
            })
        }
    };
}

