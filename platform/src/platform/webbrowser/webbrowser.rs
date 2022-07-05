
use {
    crate::{
        makepad_live_id::*,
        makepad_math::Vec2,
        makepad_wasm_bridge::{FromWasmMsg, ToWasmMsg, FromWasm, ToWasm},
        platform::{
            webbrowser::{
                from_wasm::*,
                to_wasm::*,
            }
        },
        event::{
            Timer,
            Signal,
            Event,
            XRInput,
            DraggedItem,
            WindowGeom
        },
        menu::Menu,
        cursor::MouseCursor,
        cx_api::{CxPlatformApi},
        cx::{Cx},
        window::{CxWindowState, CxWindowCmd},
        pass::CxPassParent,
    }
};

impl Cx {
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return self.platform.window_geom.inner_size;
    }
    
    pub fn process_to_wasm<F>(&mut self, msg_ptr: u32, mut event_handler: F) -> u32
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.event_handler = Some(&mut event_handler as *const dyn FnMut(&mut Cx, &mut Event) as *mut dyn FnMut(&mut Cx, &mut Event));
        let ret = self.event_loop_core(ToWasmMsg::take_ownership(msg_ptr));
        self.event_handler = None;
        ret
    }
    
    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn event_loop_core(&mut self, mut to_wasm: ToWasmMsg) -> u32
    {
        // store our view root somewhere
        if self.platform.is_initialized == false {
            self.platform.is_initialized = true;
            for _i in 0..10 {
                self.platform.fingers_down.push(false);
            }
        }
        
        self.platform.from_wasm = Some(FromWasmMsg::new());
        
        let is_animation_frame = false;
        while !to_wasm.was_last_block() {
            let block_id = LiveId(to_wasm.read_u64());
            let skip = to_wasm.read_block_skip();
            match block_id {
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
                    let mut deps = Vec::<String>::new();
                    for (path, _) in &self.dependencies {
                        deps.push(path.to_string());
                    }
                    // other textures, things
                    self.platform.from_wasm(
                        FromWasmLoadDeps {deps}
                    );
                    
                    self.webgl_compile_shaders();
                },
                /*
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
                }*/
                _ => {
                    panic!("Message unknown")
                }
            };
            to_wasm.block_skip(skip);
        };
        
        self.call_signals_and_triggers();
        
        if self.need_redrawing() || self.new_next_frames.len() != 0 {
            self.call_draw_event();
        }
        
        self.call_signals_and_triggers();
        
        for window in &mut self.windows {
            
            window.window_state = match &window.window_state {
                CxWindowState::Create {title, ..} => {
                    self.platform.from_wasm(FromWasmSetDocumentTitle {
                        title: title.to_string()
                    });
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
                    self.platform.from_wasm(FromWasmXrStartPresenting {});
                    CxWindowCmd::None
                },
                CxWindowCmd::XrStopPresenting => {
                    self.platform.from_wasm(FromWasmXrStopPresenting {});
                    CxWindowCmd::None
                },
                CxWindowCmd::FullScreen => {
                    self.platform.from_wasm(FromWasmFullScreen {});
                    CxWindowCmd::None
                },
                CxWindowCmd::NormalScreen => {
                    self.platform.from_wasm(FromWasmNormalScreen {});
                    CxWindowCmd::None
                },
                _ => CxWindowCmd::None,
            };
        }
        
        self.webgl_compile_shaders();
        
        // check if we need to send a cursor
        if !self.down_mouse_cursor.is_none() {
            self.platform.from_wasm(
                FromWasmSetMouseCursor::new(self.down_mouse_cursor.as_ref().unwrap().clone())
            )
        }
        else if !self.hover_mouse_cursor.is_none() {
            self.platform.from_wasm(
                FromWasmSetMouseCursor::new(self.hover_mouse_cursor.as_ref().unwrap().clone())
            )
        }
        else {
            self.platform.from_wasm(
                FromWasmSetMouseCursor::new(MouseCursor::Default)
            )
        }
        
        let mut passes_todo = Vec::new();
        let mut windows_need_repaint = 0;
        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
        
        if is_animation_frame {
            if passes_todo.len() > 0 {
                for pass_id in &passes_todo {
                    match self.passes[*pass_id].parent.clone() {
                        CxPassParent::Window(_) => {
                            // find the accompanying render window
                            // its a render window
                            windows_need_repaint -= 1;
                            let dpi_factor = self.platform.window_geom.dpi_factor;
                            self.draw_pass_to_canvas(*pass_id, dpi_factor);
                        }
                        CxPassParent::Pass(parent_pass_id) => {
                            let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                            self.draw_pass_to_texture(*pass_id, dpi_factor);
                        },
                        CxPassParent::None => {
                            self.draw_pass_to_texture(*pass_id, 1.0);
                        }
                    }
                }
            }
        }
        
        // request animation frame if still need to redraw, or repaint
        // we use request animation frame for that.
        if self.need_redrawing() || self.new_next_frames.len() != 0 {
            self.platform.from_wasm(FromWasmRequestAnimationFrame{});
        }
        
        //return wasm pointer to caller
        self.platform.from_wasm.take().unwrap().release_ownership()
    }
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event),
    {
    }
}


impl CxPlatformApi for Cx{

    fn show_text_ime(&mut self, x: f32, y: f32) {
        self.platform.from_wasm(FromWasmShowTextIME{x,y});
    }
    
    fn hide_text_ime(&mut self) {
       self.platform.from_wasm(FromWasmHideTextIME{});
    }
    
    fn post_signal(_signal: Signal, _value: u64) {
        // todo
    }
    /*
    fn file_read(&mut self, path: &str) -> FileRead {
        let id = self.platform.file_read_id;
        self.platform.from_wasm.read_file(id as u32, path);
        self.platform.file_read_id += 1;
        FileRead {read_id: id, path: path.to_string()}
    }
    
    fn file_write(&mut self, _path: &str, _data: &[u8]) -> u64 {
        return 0
    }
    */
    fn set_window_outer_size(&mut self, _size: Vec2) {
    }
    
    fn set_window_position(&mut self, _pos: Vec2) {
    }
    
    fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer {
        self.timer_id += 1;
        self.platform.from_wasm(FromWasmStartTimer{
            repeats,
            interval,
            id: self.timer_id as f64,
        });
        Timer {timer_id: self.timer_id}
    }
    
    fn stop_timer(&mut self, timer: Timer) {
        if timer.timer_id != 0 {
            self.platform.from_wasm(FromWasmStopTimer{
                id: timer.timer_id as f64,
            });
        }
    }
    /*
    fn http_send(&mut self, verb: &str, path: &str, proto: &str, domain: &str, port: u16, content_type: &str, body: &[u8], signal: Signal) {
        self.platform.from_wasm.http_send(verb, path, proto, domain, port, content_type, body, signal);
    }
    
    fn websocket_send(&mut self, url: &str, data: &[u8]) {
        self.platform.from_wasm.websocket_send(url, data);
    }*/
    fn start_dragging(&mut self, _dragged_item: DraggedItem) {
    }
        
    fn update_menu(&mut self, _menu: &Menu) {
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
    pub fn from_wasm(&mut self, from_wasm: impl FromWasm) {
        self.from_wasm.as_mut().unwrap().from_wasm(from_wasm);
    }
}

#[export_name = "get_wasm_js_msg_class"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn get_wasm_js_msg_class() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
    
    out.push_str("return {\n");
    out.push_str("ToWasmMsg:class extends ToWasmMsg{\n");
    
    ToWasmConstructAndGetDeps::to_wasm_js_method(&mut out);
    ToWasmDepsLoaded::to_wasm_js_method(&mut out);
    ToWasmInit::to_wasm_js_method(&mut out);
    ToWasmResizeWindow::to_wasm_js_method(&mut out);
    ToWasmAnimationFrame::to_wasm_js_method(&mut out);
    ToWasmFingerDown::to_wasm_js_method(&mut out);
    ToWasmFingerUp::to_wasm_js_method(&mut out);
    ToWasmFingerMove::to_wasm_js_method(&mut out);
    ToWasmFingerHover::to_wasm_js_method(&mut out);
    ToWasmFingerScroll::to_wasm_js_method(&mut out);
    ToWasmKeyDown::to_wasm_js_method(&mut out);
    ToWasmKeyUp::to_wasm_js_method(&mut out);
    ToWasmTextInput::to_wasm_js_method(&mut out);
    ToWasmTextCopy::to_wasm_js_method(&mut out);
    ToWasmTimerFired::to_wasm_js_method(&mut out);
    ToWasmPaintDirty::to_wasm_js_method(&mut out);
    ToWasmWindowFocusChange::to_wasm_js_method(&mut out);
    ToWasmXRUpdate::to_wasm_js_method(&mut out);
    
    out.push_str("},\n");
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    
    FromWasmCompileWebGLShader::from_wasm_js_method(&mut out);
    FromWasmAllocArrayBuffer::from_wasm_js_method(&mut out);
    FromWasmAllocIndexBuffer::from_wasm_js_method(&mut out);
    FromWasmAllocVao::from_wasm_js_method(&mut out);
    FromWasmDrawCall::from_wasm_js_method(&mut out);
    FromWasmClear::from_wasm_js_method(&mut out);
    FromWasmLoadDeps::from_wasm_js_method(&mut out);
    FromWasmUpdateTextureImage2D::from_wasm_js_method(&mut out);
    FromWasmRequestAnimationFrame::from_wasm_js_method(&mut out);
    FromWasmSetDocumentTitle::from_wasm_js_method(&mut out);
    FromWasmSetMouseCursor::from_wasm_js_method(&mut out);
    FromWasmReadFile::from_wasm_js_method(&mut out);
    FromWasmShowTextIME::from_wasm_js_method(&mut out);
    FromWasmHideTextIME::from_wasm_js_method(&mut out);
    FromWasmTextCopyResponse::from_wasm_js_method(&mut out);
    FromWasmStartTimer::from_wasm_js_method(&mut out);
    FromWasmStopTimer::from_wasm_js_method(&mut out);
    FromWasmXrStartPresenting::from_wasm_js_method(&mut out);
    FromWasmXrStopPresenting::from_wasm_js_method(&mut out);
    FromWasmBeginRenderTargets::from_wasm_js_method(&mut out);
    FromWasmAddColorTarget::from_wasm_js_method(&mut out);
    FromWasmSetDepthTarget::from_wasm_js_method(&mut out);
    FromWasmEndRenderTargets::from_wasm_js_method(&mut out);
    FromWasmBeginMainCanvas::from_wasm_js_method(&mut out);
    FromWasmSetDefaultDepthAndBlendMode::from_wasm_js_method(&mut out);
    FromWasmFullScreen::from_wasm_js_method(&mut out);
    FromWasmNormalScreen::from_wasm_js_method(&mut out);
    
    out.push_str("}\n");
    out.push_str("}");
    
    msg.push_str(&out);
    msg.release_ownership()
}
