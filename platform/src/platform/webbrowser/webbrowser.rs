
use {
    crate::{
        makepad_live_id::*,
        makepad_math::Vec2,
        makepad_wasm_bridge::{FromWasmMsg, ToWasmMsg, FromWasm, ToWasm},
        platform::{
            webbrowser::{
                from_wasm::*,
                to_wasm::*,
                webgl_platform::*,
            }
        },
        event::{
            Timer,
            Signal,
            Event,
            XRInput,
            WindowGeom
        },
        menu::Menu,
        cursor::MouseCursor,
        cx_api::{CxPlatformApi},
        cx::{Cx, PlatformType},
        window::{CxWindowState, CxWindowCmd},
        pass::CxPassParent,
    }
};

impl Cx {
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return self.platform.window_geom.inner_size;
    }
    
    pub fn process_to_wasm<F>(&mut self, msg: u32, mut event_handler: F) -> u32
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.event_handler = Some(&mut event_handler as *const dyn FnMut(&mut Cx, &mut Event) as *mut dyn FnMut(&mut Cx, &mut Event));
        let ret = self.event_loop_core(ToWasmMsg::take_ptr_ownership(msg));
        self.event_handler = None;
        ret
    }
    
    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn event_loop_core(&mut self, to_wasm: ToWasmMsg) -> u32
    {
        // store our view root somewhere
        if self.platform.is_initialized == false {
            self.platform.is_initialized = true;
            for _i in 0..10 {
                self.platform.fingers_down.push(false);
            }
        }
        
        self.platform.from_wasm = Some(FromWasmMsg::new());
        
        let mut is_animation_frame = false;
        while !to_wasm.was_last_cmd(){
            let cmd_id = LiveId(to_wasm.read_u64());
            let cmd_skip = to_wasm.read_cmd_skip();
            match cmd_id{
                id!(ToWasmConstructAndGetDeps) => { // fetch_deps
                    let msg = ToWasmConstructAndGetDeps::read_to_wasm(&mut to_wasm);
                    
                    self.call_event_handler(&mut Event::Construct);
                    
                    self.gpu_info.init_from_info(
                        msg.gpu_info.min_uniform_vectors,
                        msg.gpu_info.vendor,
                        msg.gpu_info.renderer
                    );
                    
                    self.platform_type = msg.browser_info.into();
                    // send the UI our deps, overlap with shadercompiler
                    let mut load_deps = Vec::<String>::new();
                    
                    for (file, _) in &self.live_styles.font_index { 
                        load_deps.push(file.to_string());
                    }
                    // other textures, things
                    self.platform.from_wasm.load_deps(load_deps);
                    
                    self.webgl_compile_all_shaders();
                },
                2 => { // deps_loaded
                    let len = to_wasm.mu32();
                    self.fonts.resize(self.live_styles.font_index.len(), CxFont::default());
                    for _ in 0..len {
                        let dep_path = to_wasm.parse_string();
                        let vec_ptr = to_wasm.mu32() as *mut u8;
                        let vec_len = to_wasm.mu32() as usize;
                        let vec_rec = unsafe {Vec::<u8>::from_raw_parts(vec_ptr, vec_len, vec_len)};
                        // check if its a font
                        for (file, font) in &self.live_styles.font_index {
                            let file_str = file.to_string();
                            if file_str.to_string() == dep_path {
                                let mut cxfont = &mut self.fonts[font.font_id];
                                // load it
                                if cxfont.load_from_ttf_bytes(&vec_rec).is_err() {
                                    println!("Error loading font {} ", dep_path);
                                }
                                else {
                                    cxfont.file = file_str;
                                }
                                break;
                            }
                        }
                    }
                },
                3 => { // init
                    self.platform.window_geom = WindowGeom {
                        is_fullscreen: false,
                        is_topmost: false,
                        inner_size: Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()},
                        dpi_factor: to_wasm.mf32(),
                        outer_size: Vec2 {x: 0., y: 0.},
                        position: Vec2 {x: 0., y: 0.},
                        xr_is_presenting: false,
                        xr_can_present: to_wasm.mu32() > 0,
                        can_fullscreen: to_wasm.mu32() > 0
                    };
                    self.default_dpi_factor = self.platform.window_geom.dpi_factor;
                    
                    if self.windows.len() > 0 {
                        self.windows[0].window_geom = self.platform.window_geom.clone();
                    }
                    
                    self.redraw_child_area(Area::All);
                },
                4 => { // resize
                    let old_geom = self.platform.window_geom.clone();
                    self.platform.window_geom = WindowGeom {
                        is_topmost: false,
                        inner_size: Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()},
                        dpi_factor: to_wasm.mf32(),
                        outer_size: Vec2 {x: 0., y: 0.},
                        position: Vec2 {x: 0., y: 0.},
                        xr_is_presenting: to_wasm.mu32() > 0,
                        xr_can_present: to_wasm.mu32() > 0,
                        is_fullscreen: to_wasm.mu32() > 0,
                        can_fullscreen: to_wasm.mu32() > 0,
                    };
                    let new_geom = self.platform.window_geom.clone();
                    
                    if self.windows.len()>0 {
                        self.windows[0].window_geom = self.platform.window_geom.clone();
                    }
                    if old_geom != new_geom {
                        self.call_event_handler(&mut Event::WindowGeomChange(WindowGeomChangeEvent {
                            window_id: 0,
                            old_geom: old_geom,
                            new_geom: new_geom
                        }));
                    }
                    
                    // do our initial redraw and repaint
                    self.redraw_child_area(Area::All);
                },
                5 => { // animation_frame
                    is_animation_frame = true;
                    let time = to_wasm.mf64();
                    self.anim_time = time;
                    //log!(self, "{} o clock",time);
                    if self.playing_animator_ids.len() != 0 {
                        self.call_animate_event(time);
                    }
                    if self.next_frames.len() != 0 {
                        self.call_next_frame_event(time);
                    }
                },
                6 => { // finger down
                    let abs = Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()};
                    let digit = to_wasm.mu32() as usize;
                    self.platform.fingers_down[digit] = true;
                    let is_touch = to_wasm.mu32() > 0;
                    let modifiers = unpack_key_modifier(to_wasm.mu32());
                    let time = to_wasm.mf64();
                    let tap_count = self.process_tap_count(digit, abs, time);
                    self.call_event_handler(&mut Event::FingerDown(FingerDownEvent {
                        window_id: 0,
                        abs: abs,
                        rel: abs,
                        rect: Rect::default(),
                        handled: false,
                        digit: digit,
                        input_type: if is_touch {FingerInputType::Touch} else {FingerInputType::Mouse},
                        modifiers: modifiers,
                        time: time,
                        tap_count: tap_count
                    }));
                },
                7 => { // finger up
                    let abs = Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()};
                    let digit = to_wasm.mu32() as usize;
                    self.platform.fingers_down[digit] = false;
                    if !self.platform.fingers_down.iter().any( | down | *down) {
                        self.down_mouse_cursor = None;
                    }
                    let is_touch = to_wasm.mu32()>0;
                    let modifiers = unpack_key_modifier(to_wasm.mu32());
                    let time = to_wasm.mf64();
                    self.call_event_handler(&mut Event::FingerUp(FingerUpEvent {
                        window_id: 0,
                        abs: abs,
                        rel: abs,
                        rect: Rect::default(),
                        abs_start: Vec2::default(),
                        rel_start: Vec2::default(),
                        digit: digit,
                        is_over: false,
                        input_type: if is_touch {FingerInputType::Touch} else {FingerInputType::Mouse},
                        modifiers: modifiers,
                        time: time
                    }));
                    self.fingers[digit].captured = Area::Empty;
                    self.down_mouse_cursor = None;
                },
                8 => { // finger move
                    let abs = Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()};
                    let digit = to_wasm.mu32() as usize;
                    let is_touch = to_wasm.mu32()>0;
                    let modifiers = unpack_key_modifier(to_wasm.mu32());
                    let time = to_wasm.mf64();
                    self.call_event_handler(&mut Event::FingerMove(FingerMoveEvent {
                        window_id: 0,
                        abs: abs,
                        rel: abs,
                        rect: Rect::default(),
                        abs_start: Vec2::default(),
                        rel_start: Vec2::default(),
                        is_over: false,
                        digit: digit,
                        input_type: if is_touch {FingerInputType::Touch} else {FingerInputType::Mouse},
                        modifiers: modifiers,
                        time: time
                    }));
                },
                9 => { // finger hover
                    self.fingers[0].over_last = Area::Empty;
                    let abs = Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()};
                    self.hover_mouse_cursor = None;
                    let modifiers = unpack_key_modifier(to_wasm.mu32());
                    let time = to_wasm.mf64();
                    self.call_event_handler(&mut Event::FingerHover(FingerHoverEvent {
                        any_down: false,
                        digit: 0,
                        window_id: 0,
                        abs: abs,
                        rel: abs,
                        rect: Rect::default(),
                        handled: false,
                        hover_state: HoverState::Over,
                        modifiers: modifiers,
                        time: time
                    }));
                    self.fingers[0]._over_last = self.fingers[0].over_last;
                    //if fe.hover_state == HoverState::Out {
                    //    self.hover_mouse_cursor = None;
                    // }
                },
                10 => { // finger scroll
                    let abs = Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()};
                    let scroll = Vec2 {
                        x: to_wasm.mf32(),
                        y: to_wasm.mf32()
                    };
                    let is_wheel = to_wasm.mu32() != 0;
                    let modifiers = unpack_key_modifier(to_wasm.mu32());
                    let time = to_wasm.mf64();
                    self.call_event_handler(&mut Event::FingerScroll(FingerScrollEvent {
                        window_id: 0,
                        digit: 0,
                        abs: abs,
                        rel: abs,
                        rect: Rect::default(),
                        handled_x: false,
                        handled_y: false,
                        scroll: scroll,
                        input_type: if is_wheel {FingerInputType::Mouse} else {FingerInputType::Touch},
                        modifiers: modifiers,
                        time: time
                    }));
                },
                11 => { // finger out
                    let abs = Vec2 {x: to_wasm.mf32(), y: to_wasm.mf32()};
                    let modifiers = unpack_key_modifier(to_wasm.mu32());
                    let time = to_wasm.mf64();
                    self.call_event_handler(&mut Event::FingerHover(FingerHoverEvent {
                        window_id: 0,
                        digit: 0,
                        any_down: false,
                        abs: abs,
                        rel: abs,
                        rect: Rect::default(),
                        handled: false,
                        hover_state: HoverState::Out,
                        modifiers: modifiers,
                        time: time
                    }));
                },
                12 => { // key_down
                    let key_event = KeyEvent {
                        key_code: web_to_key_code(to_wasm.mu32()),
                        //key_char: if let Some(c) = std::char::from_u32(to_wasm.mu32()) {c}else {'?'},
                        is_repeat: to_wasm.mu32() > 0,
                        modifiers: unpack_key_modifier(to_wasm.mu32()),
                        time: to_wasm.mf64()
                    };
                    self.process_key_down(key_event.clone());
                    self.call_event_handler(&mut Event::KeyDown(key_event));
                },
                13 => { // key up
                    let key_event = KeyEvent {
                        key_code: web_to_key_code(to_wasm.mu32()),
                        //key_char: if let Some(c) = std::char::from_u32(to_wasm.mu32()) {c}else {'?'},
                        is_repeat: to_wasm.mu32() > 0,
                        modifiers: unpack_key_modifier(to_wasm.mu32()),
                        time: to_wasm.mf64()
                    };
                    self.process_key_up(&key_event);
                    self.call_event_handler(&mut Event::KeyUp(key_event));
                },
                14 => { // text input
                    self.call_event_handler(&mut Event::TextInput(TextInputEvent {
                        was_paste: to_wasm.mu32()>0,
                        replace_last: to_wasm.mu32()>0,
                        input: to_wasm.parse_string(),
                    }));
                },
                15 => { // file read data
                    let read_id = to_wasm.mu32();
                    let buf_ptr = to_wasm.mu32() as *mut u8;
                    let buf_len = to_wasm.mu32() as usize;
                    let vec_buf = unsafe {Vec::<u8>::from_raw_parts(buf_ptr, buf_len, buf_len)};
                    
                    self.call_event_handler(&mut Event::FileRead(FileReadEvent {
                        read_id: read_id as u64,
                        data: Ok(vec_buf)
                    }));
                },
                16 => { // file error
                    let read_id = to_wasm.mu32();
                    
                    self.call_event_handler(&mut Event::FileRead(FileReadEvent {
                        read_id: read_id as u64,
                        data: Err("Cannot load".to_string())
                    }));
                },
                17 => { // text copy
                    let mut event = Event::TextCopy(TextCopyEvent {
                        response: None
                    });
                    self.call_event_handler(&mut event);
                    match &event {
                        Event::TextCopy(req) => if let Some(response) = &req.response {
                            self.platform.from_wasm.text_copy_response(&response);
                        }
                        _ => ()
                    };
                },
                18 => { // timer fired
                    let timer_id = to_wasm.mf64() as u64;
                    self.call_event_handler(&mut Event::Timer(TimerEvent {
                        timer_id: timer_id
                    }));
                },
                19 => { // window focus lost
                    let focus = to_wasm.mu32();
                    if focus == 0 {
                        self.call_all_keys_up();
                        self.call_event_handler(&mut Event::AppFocusLost);
                    }
                    else {
                        self.call_event_handler(&mut Event::AppFocus);
                    }
                },
                20 => { // xr_update, TODO add all the matrices / tracked hands / position IO'ed here
                    //is_animation_frame = true;
                    let inputs_len = to_wasm.mu32();
                    let time = to_wasm.mf64();
                    let head_transform = to_wasm.parse_transform();
                    let mut left_input = XRInput::default();
                    let mut right_input = XRInput::default();
                    let mut other_inputs = Vec::new();
                    for _ in 0..inputs_len {
                        let skip = to_wasm.mu32();
                        if skip == 0 {
                            continue;
                        }
                        let mut input = XRInput::default();
                        input.active = true;
                        input.grip = to_wasm.parse_transform();
                        input.ray = to_wasm.parse_transform();
                        
                        let hand = to_wasm.mu32();
                        let num_buttons = to_wasm.mu32() as usize;
                        input.num_buttons = num_buttons;
                        for i in 0..num_buttons {
                            input.buttons[i].pressed = to_wasm.mu32() > 0;
                            input.buttons[i].value = to_wasm.mf32();
                        }
                        
                        let num_axes = to_wasm.mu32() as usize;
                        input.num_axes = num_axes;
                        for i in 0..num_axes {
                            input.axes[i] = to_wasm.mf32();
                        }
                        
                        if hand == 1 {
                            left_input = input;
                        }
                        else if hand == 2 {
                            right_input = input;
                        }
                        else {
                            other_inputs.push(input);
                        }
                    }
                    // call the VRUpdate event
                    self.call_event_handler(&mut Event::XRUpdate(XRUpdateEvent {
                        time,
                        head_transform,
                        last_left_input: self.platform.xr_last_left_input.clone(),
                        last_right_input: self.platform.xr_last_right_input.clone(),
                        left_input: left_input.clone(),
                        right_input: right_input.clone(),
                        other_inputs,
                    }));
                    
                    self.platform.xr_last_left_input = left_input;
                    self.platform.xr_last_right_input = right_input;
                    
                },
                21 => { // paint_dirty, only set the passes of the main window to dirty
                    self.passes[self.windows[0].main_pass_id.unwrap()].paint_dirty = true;
                },
                22 => { //http_send_response
                    let signal_id = to_wasm.mu32();
                    let success = to_wasm.mu32();
                    let mut new_set = BTreeSet::new();
                    new_set.insert(match success {
                        1 => Cx::status_http_send_ok(),
                        _ => Cx::status_http_send_fail()
                    });
                    self.signals.insert(Signal {signal_id: signal_id as usize}, new_set);
                },
                23 => { // websocket message
                    let vec_ptr = to_wasm.mu32() as *mut u8;
                    let vec_len = to_wasm.mu32() as usize;
                    let url = to_wasm.parse_string();
                    let data = unsafe {Vec::<u8>::from_raw_parts(vec_ptr, vec_len, vec_len)};
                    self.call_event_handler(&mut Event::WebSocketMessage(
                        WebSocketMessageEvent {url, result: Ok(data)}
                    ));
                }
                24 => { // websocket error
                    let url = to_wasm.parse_string();
                    let err = to_wasm.parse_string();
                    self.call_event_handler(&mut Event::WebSocketMessage(
                        WebSocketMessageEvent {url, result: Err(err)}
                    ));
                }
                _ => {
                    panic!("Message unknown")
                }
            };
            to_wasm.cmd_skip(cmd_skip);
        };
        
        self.call_signals_and_triggers();
        
        if is_animation_frame && (self.redraw_child_areas.len()>0 || self.redraw_parent_areas.len()>0) {
            self.call_draw_event();
        }
        self.call_signals_and_triggers();
        
        for window in &mut self.windows {
            
            window.window_state = match &window.window_state {
                CxWindowState::Create {title, ..} => {
                    self.platform.from_wasm.set_document_title(&title);
                    window.window_geom = self.platform.window_geom.clone();
                    
                    CxWindowState::Created
                },
                CxWindowState::Close => {
                    CxWindowState::Closed
                },
                CxWindowState::Created => CxWindowState::Created,
                CxWindowState::Closed => CxWindowState::Closed
            };
            
            window.window_command = match &window.window_command {
                CxWindowCmd::XrStartPresenting => {
                    self.platform.from_wasm.xr_start_presenting();
                    CxWindowCmd::None
                },
                CxWindowCmd::XrStopPresenting => {
                    self.platform.from_wasm.xr_stop_presenting();
                    CxWindowCmd::None
                },
                CxWindowCmd::FullScreen => {
                    self.platform.from_wasm.fullscreen();
                    CxWindowCmd::None
                },
                CxWindowCmd::NormalScreen => {
                    self.platform.from_wasm.normalscreen();
                    CxWindowCmd::None
                },
                _ => CxWindowCmd::None,
            };
        }
        
        // check if we need to send a cursor
        if !self.down_mouse_cursor.is_none() {
            self.platform.from_wasm.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
        }
        else if !self.hover_mouse_cursor.is_none() {
            self.platform.from_wasm.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
        }
        else {
            self.platform.from_wasm.set_mouse_cursor(MouseCursor::Default);
        }
        
        let mut passes_todo = Vec::new();
        let mut windows_need_repaint = 0;
        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
        
        if is_animation_frame {
            if passes_todo.len() > 0 {
                for pass_id in &passes_todo {
                    match self.passes[*pass_id].dep_of.clone() {
                        CxPassDepOf::Window(_) => {
                            // find the accompanying render window
                            // its a render window
                            windows_need_repaint -= 1;
                            let dpi_factor = self.platform.window_geom.dpi_factor;
                            self.draw_pass_to_canvas(*pass_id, dpi_factor);
                        }
                        CxPassDepOf::Pass(parent_pass_id) => {
                            let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                            self.draw_pass_to_texture(*pass_id, dpi_factor);
                        },
                        CxPassDepOf::None => {
                            self.draw_pass_to_texture(*pass_id, 1.0);
                        }
                    }
                }
            }
        }
        
        // request animation frame if still need to redraw, or repaint
        // we use request animation frame for that.
        if passes_todo.len() != 0
            || self.playing_animator_ids.len() != 0
            || self.redraw_parent_areas.len() != 0
            || self.redraw_child_areas.len() != 0
            || self.next_frames.len() != 0
        {
            self.platform.from_wasm.request_animation_frame();
        }
        
        // lets check our recompile queue
        if !is_animation_frame {
            if self.live_styles.changed_live_bodies.len()>0 || self.live_styles.changed_deps.len()>0 {
                let changed_live_bodies = self.live_styles.changed_live_bodies.clone();
                let mut errors = self.process_live_styles_changes();
                self.webgl_update_all_shaders(&mut errors);
                self.call_live_recompile_event(changed_live_bodies, errors);
            }
            /*
            let mut shader_results = Vec::new();
            for shader_id in &self.shader_recompiles {
                shader_results.push(Self::webgl_compile_shader(*shader_id, !self.platform.gpu_spec_is_low_on_uniforms, true, &mut self.shaders[*shader_id], &mut self.platform, &mut self.shader_inherit_cache));
            }
            self.shader_recompiles.truncate(0);
            self.call_shader_recompile_event(shader_results, &mut event_handler);*/
        }
        self.process_live_style_errors();

        //return wasm pointer to caller
        self.platform.from_wasm.take().into_wasm_ptr()
    }
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event),
    {
    }
    
    pub fn post_signal(_signal: Signal, _value: StatusId) {
        // todo
    }
    
    pub fn file_read(&mut self, path: &str) -> FileRead {
        let id = self.platform.file_read_id;
        self.platform.from_wasm.read_file(id as u32, path);
        self.platform.file_read_id += 1;
        FileRead {read_id: id, path: path.to_string()}
    }
    
    pub fn file_write(&mut self, _path: &str, _data: &[u8]) -> u64 {
        return 0
    }
    
    pub fn set_window_outer_size(&mut self, _size: Vec2) {
    }
    
    pub fn set_window_position(&mut self, _pos: Vec2) {
    }
    
    pub fn show_text_ime(&mut self, x: f32, y: f32) {
        self.platform.from_wasm.show_text_ime(x, y);
    }
    
    pub fn hide_text_ime(&mut self) {
        self.platform.from_wasm.hide_text_ime();
    }
    
    pub fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer {
        self.timer_id += 1;
        self.platform.from_wasm.start_timer(self.timer_id, interval, repeats);
        Timer {timer_id: self.timer_id}
    }
    
    pub fn stop_timer(&mut self, timer: &mut Timer) {
        if timer.timer_id != 0 {
            self.platform.from_wasm.stop_timer(timer.timer_id);
            timer.timer_id = 0;
        }
    }
    
    pub fn http_send(&mut self, verb: &str, path: &str, proto: &str, domain: &str, port: u16, content_type: &str, body: &[u8], signal: Signal) {
        self.platform.from_wasm.http_send(verb, path, proto, domain, port, content_type, body, signal);
    }
    
    pub fn websocket_send(&mut self, url: &str, data: &[u8]) {
        self.platform.from_wasm.websocket_send(url, data);
    }
    
    pub fn update_menu(&mut self, _menu: &Menu) {
    }
}

// storage buffers for graphics API related platform
pub struct CxPlatform {
    pub is_initialized: bool,
    pub window_geom: WindowGeom,
    pub from_wasm: Option<FromWasmMsg>,
    pub vertex_buffers: usize,
    pub index_buffers: usize,
    pub vaos: usize,
    pub fingers_down: Vec<bool>,
    pub xr_last_left_input: XRInput,
    pub xr_last_right_input: XRInput,
    pub file_read_id: u64,
}

impl Default for CxPlatform {
    fn default() -> CxPlatform {
        CxPlatform {
            is_initialized: false,
            window_geom: WindowGeom::default(),
            from_wasm: None,
            vertex_buffers: 0,
            index_buffers: 0,
            vaos: 0,
            file_read_id: 1,
            fingers_down: Vec::new(),
            xr_last_left_input: XRInput::default(),
            xr_last_right_input: XRInput::default(),
        }
    }
}

impl CxPlatform {
}


