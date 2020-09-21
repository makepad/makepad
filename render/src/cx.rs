use std::collections::HashMap;
use std::fmt::Write;

pub use makepad_live_compiler::livetypes::*;
pub use makepad_live_compiler::livestyles::*;
pub use makepad_live_compiler::math::*;
pub use makepad_live_compiler::colors::*;
pub use makepad_live_compiler::ty::Ty;

pub use crate::fonts::*;
pub use crate::turtle::*;
pub use crate::cursor::*;
pub use crate::window::*;
pub use crate::view::*;
pub use crate::pass::*;
pub use crate::geometry::*;
pub use crate::texture::*;
pub use crate::text::*;
pub use crate::live::*;
pub use crate::events::*;
pub use crate::animator::*;
pub use crate::area::*;
pub use crate::menu::*;
pub use crate::shader::*;
pub use crate::live::*;
pub use crate::geometrygen::*;
pub use crate::uid;

#[cfg(all(not(feature = "ipc"), target_os = "linux"))]
pub use crate::cx_linux::*;
#[cfg(all(not(feature = "ipc"), target_os = "linux"))]
pub use crate::cx_opengl::*;

#[cfg(all(not(feature = "ipc"), target_os = "macos"))]
pub use crate::cx_macos::*;
#[cfg(all(not(feature = "ipc"), target_os = "macos"))]
pub use crate::cx_metal::*;

#[cfg(all(not(feature = "ipc"), target_os = "windows"))]
pub use crate::cx_windows::*;
#[cfg(all(not(feature = "ipc"), target_os = "windows"))]
pub use crate::cx_dx11::*;

#[cfg(all(not(feature = "ipc"), target_arch = "wasm32"))]
pub use crate::cx_webgl::*;

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
    pub counter: usize,
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
    
    pub geometries: Vec<CxGeometry>,

    pub shaders: Vec<CxShader>,
    //pub shader_recompiles: Vec<Shader>,

    pub live_macros_on_self: bool,

    pub is_in_redraw_cycle: bool,
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
    pub live_update_id: u64,
    
    pub prev_key_focus: Area,
    pub next_key_focus: Area,
    pub key_focus: Area,
    pub keys_down: Vec<KeyEvent>,
    
    pub debug_area: Area,
    
    pub down_mouse_cursor: Option<MouseCursor>,
    pub hover_mouse_cursor: Option<MouseCursor>,
    pub fingers: Vec<CxPerFinger>,
    
    pub playing_anim_areas: Vec<AnimArea>,
    pub ended_anim_areas: Vec<AnimArea>,
    
    pub frame_callbacks: Vec<Area>,
    pub _frame_callbacks: Vec<Area>,
    
    pub signals: HashMap<Signal, Vec<StatusId >>,
    
    pub live_styles: LiveStyles,
    
    pub command_settings: HashMap<CommandId, CxCommandSetting>,
    
    pub panic_now: bool,
    pub panic_redraw: bool,

    pub platform: CxPlatform,
}

#[derive(Clone, Copy, Default)]
pub struct CxCommandSetting {
    pub shift: bool,
    pub key_code: KeyCode,
    pub enabled: bool
}

#[derive(Default, Clone)]
pub struct CxPerFinger {
    pub captured: Area,
    pub tap_count: (Vec2, f64, u32),
    pub down_abs_start: Vec2,
    pub down_rel_start: Vec2,
    pub over_last: Area,
    pub _over_last: Area
}

pub const NUM_FINGERS: usize = 10;

impl Default for Cx {
    fn default() -> Self {
        let mut fingers = Vec::new();
        fingers.resize(NUM_FINGERS, CxPerFinger::default());
        
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
            counter: 0,
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
            //shader_recompiles: Vec::new(),

            geometries: Vec::new(),
            
            live_macros_on_self: true,
            
            default_dpi_factor: 1.0,
            current_dpi_factor: 1.0,
            is_in_redraw_cycle: false,
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
            live_update_id: 1,
            
            next_key_focus: Area::Empty,
            prev_key_focus: Area::Empty,
            key_focus: Area::Empty,
            keys_down: Vec::new(),
            
            debug_area: Area::Empty,
            
            down_mouse_cursor: None,
            hover_mouse_cursor: None,
            fingers: fingers,
            
            live_styles: LiveStyles::new(),
            
            command_settings: HashMap::new(),
            
            playing_anim_areas: Vec::new(),
            ended_anim_areas: Vec::new(),
            
            frame_callbacks: Vec::new(),
            _frame_callbacks: Vec::new(),
            
            signals: HashMap::new(),
            
            panic_now: false,
            panic_redraw: false,
            
            platform: CxPlatform {..Default::default()},

        }
    }
}


impl Cx {
    
    pub fn process_tap_count(&mut self, digit: usize, pos: Vec2, time: f64) -> u32 {
        if digit >= self.fingers.len() {
            return 0
        };
        let (last_pos, last_time, count) = self.fingers[digit].tap_count;
        
        if (time - last_time) < 0.5 && pos.distance(&last_pos) < 10. {
            self.fingers[digit].tap_count = (pos, time, count + 1);
            count + 1
        }
        else {
            self.fingers[digit].tap_count = (pos, time, 1);
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
    
    pub fn update_area_refs(&mut self, old_area: Area, new_area: Area) -> Area {
        if old_area == Area::Empty || old_area == Area::All {
            return new_area
        }
        
        if let Some(anim_anim) = self.playing_anim_areas.iter_mut().find( | v | v.area == old_area) {
            anim_anim.area = new_area.clone()
        }
        
        for finger in &mut self.fingers {
            if finger.captured == old_area {
                finger.captured = new_area.clone();
            }
            if finger._over_last == old_area {
                finger._over_last = new_area.clone();
            }
        }
        // update capture keyboard
        if self.key_focus == old_area {
            self.key_focus = new_area.clone()
        }
        
        // update capture keyboard
        if self.prev_key_focus == old_area {
            self.prev_key_focus = new_area.clone()
        }
        if self.next_key_focus == old_area {
            self.next_key_focus = new_area.clone()
        }
        
        //
        if let Some(next_frame) = self.frame_callbacks.iter_mut().find( | v | **v == old_area) {
            *next_frame = new_area.clone()
        }
        new_area
    }
    
    pub fn set_key_focus(&mut self, focus_area: Area) {
        self.next_key_focus = focus_area;
    }
    
    pub fn revert_key_focus(&mut self) {
        self.next_key_focus = self.prev_key_focus;
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
        
        if self.next_key_focus != self.key_focus {
            self.prev_key_focus = self.key_focus;
            self.key_focus = self.next_key_focus;
            event_handler(self, &mut Event::KeyFocus(KeyFocusEvent {
                prev: self.prev_key_focus,
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
        self.counter = 0;
        std::mem::swap(&mut self._redraw_child_areas, &mut self.redraw_child_areas);
        std::mem::swap(&mut self._redraw_parent_areas, &mut self.redraw_parent_areas);
        self.align_list.truncate(0);
        self.redraw_child_areas.truncate(0);
        self.redraw_parent_areas.truncate(0);
        self.call_event_handler(&mut event_handler, &mut Event::Draw);
        self.is_in_redraw_cycle = false;
        if self.live_styles.style_stack.len()>0 {
            panic!("Style stack disaligned, forgot a cx.end_style()");
        }
        if self.view_stack.len()>0 {
            panic!("View stack disaligned, forgot an end_view(cx)");
        }
        if self.pass_stack.len()>0 {
            panic!("Pass stack disaligned, forgot an end_pass(cx)");
        }
        if self.window_stack.len()>0 {
            panic!("Window stack disaligned, forgot an end_window(cx)");
        }
        if self.turtles.len()>0 {
            panic!("Turtle stack disaligned, forgot an end_turtle()");
        }
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
    
    pub fn send_signal(&mut self, signal: Signal, status: StatusId) {
        if signal.signal_id == 0 {
            return
        }
        if let Some(statusses) = self.signals.get_mut(&signal) {
            if statusses.iter().find( | s | **s == status).is_none() {
                statusses.push(status);
            }
        }
        else {
            self.signals.insert(signal, vec![status]);
        }
    }
    
    pub fn call_signals<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        let mut counter = 0;
        while self.signals.len() != 0 {
            counter += 1;
            let mut signals = HashMap::new();
            std::mem::swap(&mut self.signals, &mut signals);
            
            self.call_event_handler(&mut event_handler, &mut Event::Signal(SignalEvent {
                signals: signals,
            }));
            
            if counter > 100 {
                println!("Signal feedback loop detected");
                break
            }
        }
    }
    
    pub fn call_shader_recompile_event<F>(&mut self, results: Vec<ShaderCompileResult>, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event)
    {
        if results.len()>0 {
            self.call_event_handler(&mut event_handler, &mut Event::ShaderRecompile(ShaderRecompileEvent {
                results: results
            }));
        }
    }
    
    pub fn status_http_send_ok() -> StatusId {uid!()}
    pub fn status_http_send_fail() -> StatusId {uid!()}

    
    pub fn debug_draw_tree_recur(&mut self, dump_instances: bool, s: &mut String, view_id: usize, depth: usize) {
        if view_id >= self.views.len() {
            writeln!(s, "---------- Drawlist still empty ---------").unwrap();
            return
        }
        let mut indent = String::new();
        for _i in 0..depth {
            indent.push_str("  ");
        }
        let draw_calls_len = self.views[view_id].draw_calls_len;
        if view_id == 0 {
            writeln!(s, "---------- Begin Debug draw tree for redraw_id: {} ---------", self.redraw_id).unwrap();
        }
        writeln!(s, "{}view {}: len:{} rect:{:?} scroll:{:?}", indent, view_id, draw_calls_len, self.views[view_id].rect, self.views[view_id].get_local_scroll()).unwrap();
        indent.push_str("  ");
        for draw_call_id in 0..draw_calls_len {
            let sub_view_id = self.views[view_id].draw_calls[draw_call_id].sub_view_id;
            if sub_view_id != 0 {
                self.debug_draw_tree_recur(dump_instances, s, sub_view_id, depth + 1);
            }
            else {
                let cxview = &mut self.views[view_id];
                let draw_call = &mut cxview.draw_calls[draw_call_id];
                let sh = &self.shaders[draw_call.shader_id];
                let slots = sh.mapping.instance_props.total_slots;
                let instances = draw_call.instance.len() / slots;
                writeln!(s, "{}call {}: {}({}) *:{} scroll:{}", indent, draw_call_id, sh.name, draw_call.shader_id, instances, draw_call.get_local_scroll()).unwrap();
                // lets dump the instance geometry
                if dump_instances {
                    for inst in 0..instances.min(1) {
                        let mut out = String::new();
                        let mut off = 0;
                        for prop in &sh.mapping.instance_props.props {
                            match prop.slots {
                                1 => out.push_str(&format!("{}:{} ", prop.name, draw_call.instance[inst * slots + off])),
                                2 => out.push_str(&format!("{}:v2({},{}) ", prop.name, draw_call.instance[inst * slots + off], draw_call.instance[inst * slots + 1 + off])),
                                3 => out.push_str(&format!("{}:v3({},{},{}) ", prop.name, draw_call.instance[inst * slots + off], draw_call.instance[inst * slots + 1 + off], draw_call.instance[inst * slots + 1 + off])),
                                4 => out.push_str(&format!("{}:v4({},{},{},{}) ", prop.name, draw_call.instance[inst * slots + off], draw_call.instance[inst * slots + 1 + off], draw_call.instance[inst * slots + 2 + off], draw_call.instance[inst * slots + 3 + off])),
                                _ => {}
                            }
                            off += prop.slots;
                        }
                        writeln!(s, "  {}instance {}: {}", indent, inst, out).unwrap();
                    }
                }
            }
        }
        if view_id == 0 {
            writeln!(s, "---------- End Debug draw tree for redraw_id: {} ---------", self.redraw_id).unwrap();
        }
    }
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
        let mut cx = Cx::default();
        cx.style();
        $app::style(&mut cx);
        let mut app = $ app::new(&mut cx);
        let mut cxafterdraw = CxAfterDraw::new(&mut cx);
        cx.event_loop( | cx, mut event | {
            if let Event::Draw = event {
                app.draw_app(cx);
                cxafterdraw.after_draw(cx);
                return
            }
            app.handle_app(cx, &mut event);
        });
    }
}

#[macro_export]
macro_rules!wasm_app {
    ( $ app: ident) => {
        #[export_name = "create_wasm_app"]
        pub extern "C" fn create_wasm_app() -> u32 {
            let mut cx = Box::new(Cx::default());
            cx.style();
            $app::style(&mut cx);
            let app = Box::new( $ app::new(&mut cx));
            let cxafterdraw = Box::new(CxAfterDraw::new(&mut cx));
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
