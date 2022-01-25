use std::mem;
use std::ptr;
use std::alloc;
use crate::cx::*;
use std::collections::BTreeSet;

impl Cx {
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return self.platform.window_geom.inner_size;
    }
    
    pub fn process_to_wasm<F>(&mut self, msg: u32, mut event_handler: F) -> u32
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.event_handler = Some(&mut event_handler as *const dyn FnMut(&mut Cx, &mut Event) as *mut dyn FnMut(&mut Cx, &mut Event));
        let ret = self.event_loop_core(msg);
        self.event_handler = None;
        ret
    }
    
    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn event_loop_core(&mut self, msg: u32) -> u32
    {
        // store our view root somewhere
        if self.platform.is_initialized == false {
            self.platform.is_initialized = true;
            for _i in 0..10 {
                self.platform.fingers_down.push(false);
            }
        }
        
        //let root_view = unsafe {&mut *(self.platform.root_view_ptr as *mut View<NoScrollBar>)};
        let mut to_wasm = ToWasm::from(msg);
        if self.platform.from_wasm.offset == 0 {
            self.platform.from_wasm = FromWasm::new();
        }
        let mut is_animation_frame = false;
        loop {
            let msg_type = to_wasm.mu32();
            match msg_type {
                0 => { // end
                    break;
                },
                1 => { // fetch_deps
                    self.call_event_handler(&mut Event::Construct);
                    
                    self.gpu_info.init_from_info(
                        to_wasm.mu32(), // min_uniforms
                        to_wasm.parse_string(), // min_uniforms
                        to_wasm.parse_string() // min_uniforms
                    );
                    self.platform_type = PlatformType::Web {
                        port: to_wasm.mu32() as u16,
                        protocol: to_wasm.parse_string(),
                        hostname: to_wasm.parse_string(),
                        pathname: to_wasm.parse_string(),
                        search: to_wasm.parse_string(),
                        hash: to_wasm.parse_string(),
                    };
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
        // free the received message
        to_wasm.dealloc();
        
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
        // mark the end of the message
        self.platform.from_wasm.end();
        
        //return wasm pointer to caller
        self.platform.from_wasm.take_wasm_ptr()
    }
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event),
    {
    }
    
    pub fn write_log(&mut self, data: &str) {
        self.platform.from_wasm.log(data);
        //let _=io::stdout().write(data.as_bytes());
        //let _=io::stdout().flush();
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


fn unpack_key_modifier(modifiers: u32) -> KeyModifiers {
    KeyModifiers {
        shift: (modifiers & 1) != 0,
        control: (modifiers & 2) != 0,
        alt: (modifiers & 4) != 0,
        logo: (modifiers & 8) != 0
    }
}

fn web_to_key_code(key_code: u32) -> KeyCode {
    match key_code {
        27 => KeyCode::Escape,
        192 => KeyCode::Backtick,
        48 => KeyCode::Key0,
        49 => KeyCode::Key1,
        50 => KeyCode::Key2,
        51 => KeyCode::Key3,
        52 => KeyCode::Key4,
        53 => KeyCode::Key5,
        54 => KeyCode::Key6,
        55 => KeyCode::Key7,
        56 => KeyCode::Key8,
        57 => KeyCode::Key9,
        173 => KeyCode::Minus,
        189 => KeyCode::Minus,
        61 => KeyCode::Equals,
        187 => KeyCode::Equals,
        
        8 => KeyCode::Backspace,
        9 => KeyCode::Tab,
        
        81 => KeyCode::KeyQ,
        87 => KeyCode::KeyW,
        69 => KeyCode::KeyE,
        82 => KeyCode::KeyR,
        84 => KeyCode::KeyT,
        89 => KeyCode::KeyY,
        85 => KeyCode::KeyU,
        73 => KeyCode::KeyI,
        79 => KeyCode::KeyO,
        80 => KeyCode::KeyP,
        219 => KeyCode::LBracket,
        221 => KeyCode::RBracket,
        13 => KeyCode::Return,
        
        65 => KeyCode::KeyA,
        83 => KeyCode::KeyS,
        68 => KeyCode::KeyD,
        70 => KeyCode::KeyF,
        71 => KeyCode::KeyG,
        72 => KeyCode::KeyH,
        74 => KeyCode::KeyJ,
        75 => KeyCode::KeyK,
        76 => KeyCode::KeyL,
        
        59 => KeyCode::Semicolon,
        186 => KeyCode::Semicolon,
        222 => KeyCode::Quote,
        220 => KeyCode::Backslash,
        
        90 => KeyCode::KeyZ,
        88 => KeyCode::KeyX,
        67 => KeyCode::KeyC,
        86 => KeyCode::KeyV,
        66 => KeyCode::KeyB,
        78 => KeyCode::KeyN,
        77 => KeyCode::KeyM,
        188 => KeyCode::Comma,
        190 => KeyCode::Period,
        191 => KeyCode::Slash,
        
        17 => KeyCode::Control,
        18 => KeyCode::Alt,
        16 => KeyCode::Shift,
        224 => KeyCode::Logo,
        91 => KeyCode::Logo,
        
        //RightControl,
        //RightShift,
        //RightAlt,
        93 => KeyCode::Logo,
        
        32 => KeyCode::Space,
        20 => KeyCode::Capslock,
        112 => KeyCode::F1,
        113 => KeyCode::F2,
        114 => KeyCode::F3,
        115 => KeyCode::F4,
        116 => KeyCode::F5,
        117 => KeyCode::F6,
        118 => KeyCode::F7,
        119 => KeyCode::F8,
        120 => KeyCode::F9,
        121 => KeyCode::F10,
        122 => KeyCode::F11,
        123 => KeyCode::F12,
        
        44 => KeyCode::PrintScreen,
        124 => KeyCode::PrintScreen,
        //Scrolllock,
        //Pause,
        
        45 => KeyCode::Insert,
        46 => KeyCode::Delete,
        36 => KeyCode::Home,
        35 => KeyCode::End,
        33 => KeyCode::PageUp,
        34 => KeyCode::PageDown,
        
        96 => KeyCode::Numpad0,
        97 => KeyCode::Numpad1,
        98 => KeyCode::Numpad2,
        99 => KeyCode::Numpad3,
        100 => KeyCode::Numpad4,
        101 => KeyCode::Numpad5,
        102 => KeyCode::Numpad6,
        103 => KeyCode::Numpad7,
        104 => KeyCode::Numpad8,
        105 => KeyCode::Numpad9,
        
        //NumpadEquals,
        109 => KeyCode::NumpadSubtract,
        107 => KeyCode::NumpadAdd,
        110 => KeyCode::NumpadDecimal,
        106 => KeyCode::NumpadMultiply,
        111 => KeyCode::NumpadDivide,
        12 => KeyCode::Numlock,
        //NumpadEnter,
        
        38 => KeyCode::ArrowUp,
        40 => KeyCode::ArrowDown,
        37 => KeyCode::ArrowLeft,
        39 => KeyCode::ArrowRight,
        _ => KeyCode::Unknown
    }
}


// storage buffers for graphics API related platform
#[derive(Clone)]
pub struct CxPlatform {
    pub is_initialized: bool,
    pub window_geom: WindowGeom,
    pub from_wasm: FromWasm,
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
            from_wasm: FromWasm::new(),
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




//  Wasm API




#[derive(Clone)]
pub struct FromWasm {
    mu32: *mut u32,
    mf32: *mut f32,
    mf64: *mut f64,
    slots: usize,
    used: isize,
    offset: isize
}

impl FromWasm {
    pub fn zero() -> FromWasm {
        FromWasm {
            mu32: 0 as *mut u32,
            mf32: 0 as *mut f32,
            mf64: 0 as *mut f64,
            slots: 0,
            used: 0,
            offset: 0
        }
    }
    pub fn new() -> FromWasm {
        unsafe {
            let start_bytes = 4096;
            
            let buf = alloc::alloc(alloc::Layout::from_size_align(start_bytes as usize, mem::align_of::<u32>()).unwrap()) as *mut u32;
            (buf as *mut u64).write(start_bytes as u64);
            FromWasm {
                mu32: buf as *mut u32,
                mf32: buf as *mut f32,
                mf64: buf as *mut f64,
                slots: start_bytes >> 2,
                used: 2,
                offset: 0
            }
        }
    }
    
    // fit enough size for RPC structure with exponential alloc strategy
    // returns position to write to
    fn fit(&mut self, slots: usize) {
        unsafe {
            if self.used as usize + slots> self.slots {
                let mut new_slots = usize::max(self.used as usize + slots, self.slots * 2);
                if new_slots & 1 != 0 { // f64 align
                    new_slots += 1;
                }
                let new_bytes = new_slots << 2;
                let old_bytes = self.slots << 2;
                let new_buf = alloc::alloc(alloc::Layout::from_size_align(new_bytes as usize, mem::align_of::<u64>()).unwrap()) as *mut u32;
                ptr::copy_nonoverlapping(self.mu32, new_buf, self.slots);
                alloc::dealloc(
                    self.mu32 as *mut u8,
                    alloc::Layout::from_size_align(
                        old_bytes as usize,
                        mem::align_of::<u64>()
                    ).unwrap()
                );
                self.slots = new_slots;
                (new_buf as *mut u64).write(new_bytes as u64);
                self.mu32 = new_buf;
                self.mf32 = new_buf as *mut f32;
                self.mf64 = new_buf as *mut f64;
            }
            self.offset = self.used;
            self.used += slots as isize;
        }
    }
    
    fn check(&mut self) {
        if self.offset != self.used {
            panic!("Unequal allocation and writes")
        }
    }
    
    fn mu32(&mut self, v: u32) {
        unsafe {
            self.mu32.offset(self.offset).write(v);
            self.offset += 1;
        }
    }
    
    fn mf32(&mut self, v: f32) {
        unsafe {
            self.mf32.offset(self.offset).write(v);
            self.offset += 1;
        }
    }
    
    fn add_f64(&mut self, v: f64) {
        unsafe {
            if self.offset & 1 != 0 {
                self.fit(1);
                self.offset += 1;
            }
            self.fit(2);
            self.mf64.offset(self.offset >> 1).write(v);
            self.offset += 2;
        }
    }
    
    
    pub fn take_wasm_ptr(&mut self) -> u32 {
        let mu32 = self.mu32;
        self.mu32 = 0 as *mut u32;
        self.mf32 = 0 as *mut f32;
        self.mf64 = 0 as *mut f64;
        self.slots = 0;
        self.used = 0;
        self.offset = 0;
        mu32 as u32
    }

    // end the block and return ownership of the pointer
    pub fn end(&mut self) {
        self.fit(1);
        self.mu32(0);
    }
    
    fn add_propdefvec(&mut self, prop_defs: &Vec<PropDef>) {
        self.fit(1);
        self.mu32(prop_defs.len() as u32);
        for prop_def in prop_defs {
            self.add_string(match prop_def.ty {
                Ty::Vec4 => "vec4",
                Ty::Vec3 => "vec3",
                Ty::Vec2 => "vec2",
                Ty::Float => "float",
                Ty::Mat4 => "mat4",
                Ty::Texture2D => "sampler2D",
                _ => panic!("unexpected type in add_propdefvec")
            });
            self.add_string(&prop_def.name);
        }
    }
    
    // log a string
    pub fn log(&mut self, msg: &str) {
        self.fit(1);
        self.mu32(1);
        self.add_string(msg);
    }
    
    pub fn compile_webgl_shader(
        &mut self,
        shader_id: usize,
        vertex: &str,
        fragment: &str,
        mapping: &CxShaderMapping
    ) {
        self.fit(2);
        self.mu32(2);
        self.mu32(shader_id as u32);
        self.add_string(fragment);
        self.add_string(vertex);
        self.fit(2);
        self.mu32(mapping.geometry_props.total_slots as u32);
        self.mu32(mapping.instance_props.total_slots as u32);
        self.add_propdefvec(&mapping.pass_uniforms);
        self.add_propdefvec(&mapping.view_uniforms);
        self.add_propdefvec(&mapping.draw_uniforms);
        self.add_propdefvec(&mapping.user_uniforms);
        self.add_propdefvec(&mapping.live_uniforms);
        self.add_propdefvec(&mapping.textures);
    }
    
    pub fn alloc_array_buffer(&mut self, buffer_id: usize, len: usize, data: *const f32) {
        self.fit(4);
        self.mu32(3);
        self.mu32(buffer_id as u32);
        self.mu32(len as u32);
        self.mu32(data as u32);
    }
    
    pub fn alloc_index_buffer(&mut self, buffer_id: usize, len: usize, data: *const u32) {
        self.fit(4);
        self.mu32(4);
        self.mu32(buffer_id as u32);
        self.mu32(len as u32);
        self.mu32(data as u32);
    }
    
    pub fn alloc_vao(&mut self, vao_id: usize, shader_id: usize, geom_ib_id: usize, geom_vb_id: usize, inst_vb_id: usize) {
        self.fit(6);
        self.mu32(5);
        self.mu32(vao_id as u32);
        self.mu32(shader_id as u32);
        self.mu32(geom_ib_id as u32);
        self.mu32(geom_vb_id as u32);
        self.mu32(inst_vb_id as u32);
    }
    
    pub fn draw_call(
        &mut self,
        shader_id: usize,
        vao_id: usize,
        uniforms_pass: &[f32],
        uniforms_view: &[f32],
        uniforms_draw: &[f32],
        uniforms_user: &[f32],
        uniforms_live: &[f32],
        textures: &Vec<u32>,
        const_table: &Option<Vec<f32 >>
    ) {
        self.fit(11);
        self.mu32(6);
        self.mu32(shader_id as u32);
        self.mu32(vao_id as u32);
        self.mu32(uniforms_pass.as_ptr() as u32);
        self.mu32(uniforms_view.as_ptr() as u32);
        self.mu32(uniforms_draw.as_ptr() as u32);
        self.mu32(uniforms_user.as_ptr() as u32);
        self.mu32(uniforms_live.as_ptr() as u32);
        self.mu32(textures.as_ptr() as u32);
        if let Some(const_table) = const_table {
            self.mu32(const_table.as_ptr() as u32);
            self.mu32(const_table.len() as u32);
        }
        else {
            self.mu32(0);
            self.mu32(0);
        }
    }
    
    pub fn clear(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.fit(5);
        self.mu32(7);
        self.mf32(r);
        self.mf32(g);
        self.mf32(b);
        self.mf32(a);
    }
    
    pub fn load_deps(&mut self, deps: Vec<String>) {
        self.fit(1);
        self.mu32(8);
        self.fit(1);
        self.mu32(deps.len() as u32);
        for dep in deps {
            self.add_string(&dep);
        }
    }
    
    pub fn update_texture_image2d(&mut self, texture_id: usize, texture: &mut CxTexture) {
        //usize, width: usize, height: usize, data: &Vec<u32>
        self.fit(5);
        self.mu32(9);
        self.mu32(texture_id as u32);
        self.mu32(texture.desc.width.unwrap() as u32);
        self.mu32(texture.desc.height.unwrap() as u32);
        self.mu32(texture.image_u32.as_ptr() as u32)
    }
    
    pub fn request_animation_frame(&mut self) {
        self.fit(1);
        self.mu32(10);
    }
    
    pub fn set_document_title(&mut self, title: &str) {
        self.fit(1);
        self.mu32(11);
        self.add_string(title);
    }
    
    pub fn set_mouse_cursor(&mut self, mouse_cursor: MouseCursor) {
        self.fit(2);
        self.mu32(12);
        let cursor_id = match mouse_cursor {
            MouseCursor::Hidden => 0,
            MouseCursor::Default => 1,
            MouseCursor::Crosshair => 2,
            MouseCursor::Hand => 3,
            MouseCursor::Arrow => 4,
            MouseCursor::Move => 5,
            MouseCursor::Text => 6,
            MouseCursor::Wait => 7,
            MouseCursor::Help => 8,
            MouseCursor::NotAllowed => 9,
            MouseCursor::NResize => 10,
            MouseCursor::NeResize => 11,
            MouseCursor::EResize => 12,
            MouseCursor::SeResize => 13,
            MouseCursor::SResize => 14,
            MouseCursor::SwResize => 15,
            MouseCursor::WResize => 16,
            MouseCursor::NwResize => 17,
            
            MouseCursor::NsResize => 18,
            MouseCursor::NeswResize => 19,
            MouseCursor::EwResize => 20,
            MouseCursor::NwseResize => 21,
            MouseCursor::ColResize => 22,
            MouseCursor::RowResize => 23,
            
        };
        self.mu32(cursor_id);
    }
    
    pub fn read_file(&mut self, id: u32, path: &str) {
        self.fit(2);
        self.mu32(13);
        self.mu32(id);
        self.add_string(path);
    }
    
    pub fn show_text_ime(&mut self, x: f32, y: f32) {
        self.fit(3);
        self.mu32(14);
        self.mf32(x);
        self.mf32(y);
    }
    
    pub fn hide_text_ime(&mut self) {
        self.fit(1);
        self.mu32(15);
    }
    
    pub fn text_copy_response(&mut self, response: &str) {
        self.fit(1);
        self.mu32(16);
        self.add_string(response);
    }
    
    pub fn start_timer(&mut self, id: u64, interval: f64, repeats: bool) {
        self.fit(2);
        self.mu32(17);
        self.mu32(if repeats {1}else {0});
        self.add_f64(id as f64);
        self.add_f64(interval);
    }
    
    pub fn stop_timer(&mut self, id: u64) {
        self.fit(1);
        self.mu32(18);
        self.add_f64(id as f64);
    }
    
    pub fn xr_start_presenting(&mut self) {
        self.fit(1);
        self.mu32(19);
    }
    
    pub fn xr_stop_presenting(&mut self) {
        self.fit(1);
        self.mu32(20);
    }
    
    pub fn begin_render_targets(&mut self, pass_id: usize, width: usize, height: usize) {
        self.fit(4);
        self.mu32(21);
        self.mu32(pass_id as u32);
        self.mu32(width as u32);
        self.mu32(height as u32);
    }
    
    pub fn add_color_target(&mut self, texture_id: usize, init_only: bool, color: Vec4) {
        self.fit(7);
        self.mu32(22);
        self.mu32(texture_id as u32);
        self.mu32(if init_only {1} else {0});
        self.mf32(color.x);
        self.mf32(color.y);
        self.mf32(color.z);
        self.mf32(color.w);
    }
    
    pub fn set_depth_target(&mut self, texture_id: usize, init_only: bool, depth: f32) {
        self.fit(4);
        self.mu32(23);
        self.mu32(texture_id as u32);
        self.mu32(if init_only {1} else {0});
        self.mf32(depth);
    }
    
    pub fn end_render_targets(&mut self) {
        self.fit(1);
        self.mu32(24);
    }
    
    pub fn set_default_depth_and_blend_mode(&mut self) {
        self.fit(1);
        self.mu32(25);
    }
    
    pub fn begin_main_canvas(&mut self, color: Vec4, depth: f32) {
        self.fit(6);
        self.mu32(26);
        self.mf32(color.x);
        self.mf32(color.y);
        self.mf32(color.z);
        self.mf32(color.w);
        self.mf32(depth);
    }
    
    
    fn add_string(&mut self, msg: &str) {
        let len = msg.chars().count();
        self.fit(len + 1);
        self.mu32(len as u32);
        for c in msg.chars() {
            self.mu32(c as u32);
        }
        self.check();
    }
    
    fn add_u8slice(&mut self, msg: &[u8]) {
        let u8_len = msg.len();
        let len = u8_len >> 2;
        let spare = u8_len & 3;
        self.fit(len + if spare > 0 {1}else {0} + 1);
        self.mu32(u8_len as u32);
        // this is terrible. im sure this can be done so much nicer
        for i in 0..len {
            self.mu32(((msg[(i << 2) + 0] as u32)) | ((msg[(i << 2) + 1] as u32) << 8) | ((msg[(i << 2) + 2] as u32) << 16) | ((msg[(i << 2) + 3] as u32) << 24));
        }
        match spare {
            1 => self.mu32(msg[(len << 2) + 0] as u32),
            2 => self.mu32((msg[(len << 2) + 0] as u32) | ((msg[(len << 2) + 1] as u32) << 8)),
            3 => self.mu32((msg[(len << 2) + 0] as u32) | ((msg[(len << 2) + 1] as u32) << 8) | ((msg[(len << 2) + 2] as u32) << 16)),
            _ => ()
        }
        self.check();
    }
    
    pub fn http_send(&mut self, verb: &str, path: &str, proto: &str, domain: &str, port: u16, content_type: &str, body: &[u8], signal: Signal) {
        self.fit(3);
        self.mu32(27);
        self.mu32(port as u32);
        self.mu32(signal.signal_id as u32);
        self.add_string(verb);
        self.add_string(path);
        self.add_string(proto);
        self.add_string(domain);
        self.add_string(content_type);
        self.add_u8slice(body);
    }
    
    pub fn websocket_send(&mut self, url: &str, data: &[u8]) {
        self.fit(1);
        self.mu32(30);
        self.add_string(url);
        self.add_u8slice(data);
    }
    
    pub fn fullscreen(&mut self) {
        self.fit(1);
        self.mu32(28);
    }
    
    pub fn normalscreen(&mut self) {
        self.fit(1);
        self.mu32(29);
    }
}

#[derive(Clone)]
struct ToWasm {
    mu32: *mut u32,
    mf32: *mut f32,
    mf64: *mut f64,
    slots: usize,
    offset: isize
}

impl ToWasm {
    
    pub fn dealloc(&mut self) {
        unsafe {
            alloc::dealloc(self.mu32 as *mut u8, alloc::Layout::from_size_align((self.slots * mem::size_of::<u64>()) as usize, mem::align_of::<u32>()).unwrap());
            self.mu32 = 0 as *mut u32;
            self.mf32 = 0 as *mut f32;
            self.mf64 = 0 as *mut f64;
        }
    }
    
    pub fn from(buf: u32) -> ToWasm {
        unsafe {
            let bytes = (buf as *mut u64).read() as usize;
            ToWasm {
                mu32: buf as *mut u32,
                mf32: buf as *mut f32,
                mf64: buf as *mut f64,
                offset: 2,
                slots: bytes >> 2
            }
        }
    }
    
    fn mu32(&mut self) -> u32 {
        unsafe {
            let ret = self.mu32.offset(self.offset).read();
            self.offset += 1;
            ret
        }
    }
    
    fn mf32(&mut self) -> f32 {
        unsafe {
            let ret = self.mf32.offset(self.offset).read();
            self.offset += 1;
            ret
        }
    }
    
    fn mf64(&mut self) -> f64 {
        unsafe {
            if self.offset & 1 != 0 {
                self.offset += 1;
            }
            let ret = self.mf64.offset(self.offset >> 1).read();
            self.offset += 2;
            ret
        }
    }
    
    fn parse_string(&mut self) -> String {
        let len = self.mu32();
        let mut out = String::new();
        for _i in 0..len {
            if let Some(c) = std::char::from_u32(self.mu32()) {
                out.push(c);
            }
        }
        out
    }
    
    fn parse_transform(&mut self) -> Transform {
        Transform {
            orientation: Quat {
                a: self.mf32(),
                b: self.mf32(),
                c: self.mf32(),
                d: self.mf32(),
            },
            position: Vec3 {
                x: self.mf32(),
                y: self.mf32(),
                z: self.mf32(),
            }
        }
    }
}

// for use with sending wasm vec data
#[export_name = "alloc_wasm_vec"]
pub unsafe extern "C" fn alloc_wasm_vec(bytes: u32) -> u32 {
    let mut vec = Vec::<u8>::with_capacity(bytes as usize);
    vec.resize(bytes as usize, 0);
    let ptr = vec.as_mut_ptr();
    mem::forget(vec);
    return ptr as u32
}

// for use with message passing
#[export_name = "alloc_wasm_message"]
pub unsafe extern "C" fn alloc_wasm_message(bytes: u32) -> u32 {
    let buf = std::alloc::alloc(std::alloc::Layout::from_size_align(bytes as usize, mem::align_of::<u64>()).unwrap()) as u32;
    (buf as *mut u64).write(bytes as u64);
    buf as u32
}

// for use with message passing
#[export_name = "realloc_wasm_message"]
pub unsafe extern "C" fn realloc_wasm_message(in_buf: u32, new_bytes: u32) -> u32 {
    let old_buf = in_buf as *mut u8;
    let old_bytes = (old_buf as *mut u64).read() as usize;
    let new_buf = alloc::alloc(alloc::Layout::from_size_align(new_bytes as usize, mem::align_of::<u64>()).unwrap()) as *mut u8;
    ptr::copy_nonoverlapping(old_buf, new_buf, old_bytes);
    alloc::dealloc(old_buf as *mut u8, alloc::Layout::from_size_align(old_bytes as usize, mem::align_of::<u64>()).unwrap());
    (new_buf as *mut u64).write(new_bytes as u64);
    new_buf as u32
}

#[export_name = "dealloc_wasm_message"]
pub unsafe extern "C" fn dealloc_wasm_message(in_buf: u32) {
    let buf = in_buf as *mut u8;
    let bytes = buf.read() as usize;
    std::alloc::dealloc(buf as *mut u8, std::alloc::Layout::from_size_align(bytes as usize, mem::align_of::<u64>()).unwrap());
}

extern "C" {
    pub fn _console_log(chars: u32, len: u32);
}

pub fn console_log(val: &str) {
    unsafe {
        let chars = val.chars().collect::<Vec<char >> ();
        _console_log(chars.as_ptr() as u32, chars.len() as u32);
    }
}

#[macro_export]
macro_rules!log {
    ( $ ( $ t: tt) *) => {
        console_log(&format!("{}:{} - {}", file!(), line!(), format!( $ ( $ t) *)))
    }
}

