use {
    std::{
        rc::Rc,
        cell::{RefCell},
    },
    makepad_objc_sys::{
        
        msg_send,
        sel,
        sel_impl,
    },
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        os::{
            apple::frameworks::*,
            cocoa_event::{CocoaEvent},
            metal_xpc::{
                start_xpc_service,
            },
            cocoa_app::{
                CocoaApp,
                get_cocoa_app_global,
                init_cocoa_globals
            },
            metal::{MetalCx, MetalWindow, DrawPassMode},
        },
        pass::{CxPassParent},
        event::{
            WebSocket,
            WebSocketAutoReconnect,
            Signal,
            Event,
        },
        cx_api::{CxOsApi, CxOsOp},
        cx::{Cx, OsType},
    }
};

const KEEP_ALIVE_COUNT: usize = 5;

impl Cx {
    
    pub fn event_loop(mut self) {
        for arg in std::env::args() {
            if arg == "--metal-xpc" {
                return start_xpc_service();
            }
        }
        
        self.platform_type = OsType::OSX;
        let metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        let cx = Rc::new(RefCell::new(self));
        
        for arg in std::env::args() {
            if arg == "--stdin-loop" {
                let mut cx = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                return cx.stdin_event_loop(&mut metal_cx);
            }
        }
        
        let metal_windows = Rc::new(RefCell::new(Vec::new()));
        init_cocoa_globals(Box::new({
            let cx = cx.clone();
            move | cocoa_app,
            events | {
                let mut cx = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let mut metal_windows = metal_windows.borrow_mut();
                cx.cocoa_event_callback(cocoa_app, events, &mut metal_cx, &mut metal_windows)
            }
        }));
        
        // final bit of initflow
        //get_cocoa_app_global().start_timer(0, 0.2, true);
        cx.borrow_mut().call_event_handler(&Event::Construct);
        cx.borrow_mut().redraw_all();
        get_cocoa_app_global().event_loop();
    }
    
    pub (crate) fn handle_repaint(&mut self, metal_windows: &mut Vec<MetalWindow>, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        let dpi_factor = metal_window.window_geom.dpi_factor;
                        metal_window.resize_core_animation_layer(&metal_cx);
                        let drawable: ObjcId = unsafe {msg_send![metal_window.ca_layer, nextDrawable]};
                        if drawable == nil {
                            return
                        }
                        if metal_window.is_resizing{
                            self.draw_pass(*pass_id, dpi_factor, metal_cx, DrawPassMode::Resizing(drawable));
                        }
                        else{
                            self.draw_pass(*pass_id, dpi_factor, metal_cx, DrawPassMode::Drawable(drawable));
                        }
                    }
                }
                CxPassParent::Pass(parent_pass_id) => {
                    let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass(*pass_id, dpi_factor, metal_cx, DrawPassMode::Texture);
                },
                CxPassParent::None => {
                    self.draw_pass(*pass_id, 1.0, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }
    
    fn cocoa_event_callback(
        &mut self,
        cocoa_app: &mut CocoaApp,
        events: Vec<CocoaEvent>,
        metal_cx: &mut MetalCx,
        metal_windows: &mut Vec<MetalWindow>
    ) -> bool {
        
        self.handle_platform_ops(metal_windows, metal_cx, cocoa_app);
        
        let mut paint_dirty = false;
        for event in events {
            // keepalive check
            match &event {
                CocoaEvent::MouseDown(_) |
                CocoaEvent::MouseMove(_) |
                CocoaEvent::MouseUp(_) |
                CocoaEvent::Scroll(_) |
                CocoaEvent::KeyDown(_) |
                CocoaEvent::KeyUp(_) |
                CocoaEvent::TextInput(_) => {
                    self.os.keep_alive_counter = KEEP_ALIVE_COUNT;
                }
                CocoaEvent::Timer(te) => {
                    if te.timer_id == 0 {
                        if self.os.keep_alive_counter>0 {
                            self.os.keep_alive_counter -= 1;
                            self.repaint_windows();
                            paint_dirty = true;
                        }
                        continue;
                    }
                }
                _ => ()
            }
            
            //self.process_desktop_pre_event(&mut event);
            match event {
                CocoaEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                    for mw in metal_windows.iter_mut() {
                        if let Some(main_pass_id) = self.windows[mw.window_id].main_pass_id {
                            self.repaint_pass(main_pass_id);
                        }
                    }
                    paint_dirty = true;
                    self.call_event_handler(&Event::AppGotFocus);
                }
                CocoaEvent::AppLostFocus => {
                    self.call_event_handler(&Event::AppLostFocus);
                }
                CocoaEvent::WindowResizeLoopStart(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.start_resize();
                    }
                }
                CocoaEvent::WindowResizeLoopStop(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.stop_resize();
                    }
                }
                CocoaEvent::WindowGeomChange(re) => { // do this here because mac
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == re.window_id) {
                        metal_window.window_geom = re.new_geom.clone();
                        self.windows[re.window_id].window_geom = re.new_geom.clone();
                        // redraw just this windows root draw list
                        if re.old_geom.inner_size != re.new_geom.inner_size {
                            if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                self.redraw_pass_and_child_passes(main_pass_id);
                            }
                        }
                    }
                    // ok lets not redraw all, just this window
                    self.call_event_handler(&Event::WindowGeomChange(re));
                }
                CocoaEvent::WindowClosed(wc) => {
                    // lets remove the window from the set
                    self.windows[wc.window_id].is_created = false;
                    if let Some(index) = metal_windows.iter().position( | w | w.window_id == wc.window_id) {
                        metal_windows.remove(index);
                        if metal_windows.len() == 0 {
                            cocoa_app.terminate_event_loop();
                        }
                    }
                    self.call_event_handler(&Event::WindowClosed(wc));
                }
                CocoaEvent::Paint => {
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(cocoa_app.time_now());
                    }
                    if self.need_redrawing() {
                        self.call_draw_event();
                        self.mtl_compile_shaders(&metal_cx);
                    }
                    // ok here we send out to all our childprocesses
                    
                    self.handle_repaint(metal_windows, metal_cx);
                }
                CocoaEvent::MouseDown(md) => {
                    if self.os.last_mouse_button == None ||
                    self.os.last_mouse_button == Some(md.button) {
                        self.os.last_mouse_button = Some(md.button);
                        let digit_id = live_id!(mouse).into();
                        self.fingers.alloc_digit(digit_id);
                        self.fingers.process_tap_count(
                            digit_id,
                            md.abs,
                            md.time
                        );
                        self.call_event_handler(&Event::FingerDown(
                            md.into_finger_down_event(&self.fingers, digit_id)
                        ));
                    }
                }
                CocoaEvent::MouseMove(mm) => {
                    let digit_id = live_id!(mouse).into();
                    
                    if !self.fingers.is_digit_allocated(digit_id) {
                        let area = self.fingers.get_hover_area(digit_id);
                        self.call_event_handler(&Event::FingerHover(
                            mm.into_finger_hover_event(
                                digit_id,
                                area,
                                self.os.last_mouse_button.unwrap_or(0)
                            )
                        ));
                    }
                    else {
                        self.call_event_handler(&mut Event::FingerMove(
                            mm.into_finger_move_event(
                                &self.fingers,
                                digit_id,
                                self.os.last_mouse_button.unwrap_or(0)
                            )
                        ));
                    }
                    self.fingers.cycle_hover_area(digit_id);
                }
                CocoaEvent::MouseUp(md) => {
                    if self.os.last_mouse_button == Some(md.button) {
                        self.os.last_mouse_button = None;
                        let digit_id = live_id!(mouse).into();
                        self.call_event_handler(&Event::FingerUp(
                            md.into_finger_up_event(
                                &self.fingers,
                                digit_id,
                            )
                        ));
                        self.fingers.free_digit(digit_id);
                    }
                }
                CocoaEvent::Scroll(e) => {
                    self.call_event_handler(&Event::FingerScroll(
                        e.into_finger_scroll_event(live_id!(mouse).into())
                    ))
                }
                CocoaEvent::WindowDragQuery(e) => {
                    self.call_event_handler(&Event::WindowDragQuery(e))
                }
                CocoaEvent::WindowCloseRequested(e) => {
                    self.call_event_handler(&Event::WindowCloseRequested(e))
                }
                CocoaEvent::TextInput(e) => {
                    self.call_event_handler(&Event::TextInput(e))
                }
                CocoaEvent::Drag(e) => {
                    self.call_event_handler(&Event::Drag(e))
                }
                CocoaEvent::Drop(e) => {
                    self.call_event_handler(&Event::Drop(e))
                }
                CocoaEvent::DragEnd => {
                    self.call_event_handler(&Event::DragEnd)
                }
                CocoaEvent::KeyDown(e) => {
                    self.keyboard.process_key_down(e.clone());
                    self.call_event_handler(&Event::KeyDown(e))
                }
                CocoaEvent::KeyUp(e) => {
                    self.keyboard.process_key_up(e.clone());
                    self.call_event_handler(&Event::KeyUp(e))
                }
                CocoaEvent::TextCopy(e) => {
                    self.call_event_handler(&Event::TextCopy(e))
                }
                CocoaEvent::Timer(e) => {
                    self.call_event_handler(&Event::Timer(e))
                }
                CocoaEvent::Signal(se) => {
                    //println!("SIGNAL!");
                    //self.handle_core_midi_signals(&se);
                    self.call_event_handler(&Event::Signal(se));
                }
                CocoaEvent::MenuCommand(e) => {
                    self.call_event_handler(&Event::MenuCommand(e))
                }
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            false
        } else {
            true
        }
    }
    
    fn handle_platform_ops(&mut self, metal_windows: &mut Vec<MetalWindow>, metal_cx: &MetalCx, cocoa_app: &mut CocoaApp) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        cocoa_app,
                        window.create_inner_size.unwrap_or(dvec2(800.,600.)),
                        window.create_position,
                        &window.create_title
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;
                },
                CxOsOp::CloseWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        self.windows[window_id].is_created = false;
                        metal_window.cocoa_window.close_window();
                        break;
                    }
                },
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.minimize();
                    }
                },
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.maximize();
                    }
                },
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.restore();
                    }
                },
                CxOsOp::FullscreenWindow(_window_id) => {
                    todo!()
                },
                CxOsOp::NormalizeWindow(_window_id) => {
                    todo!()
                }
                CxOsOp::SetTopmost(_window_id, _is_topmost) => {
                    todo!()
                }
                CxOsOp::XrStartPresenting(_) => {
                    todo!()
                },
                CxOsOp::XrStopPresenting(_) => {
                    todo!()
                },
                CxOsOp::ShowTextIME(area, pos) => {
                    let pos = area.get_clipped_rect(self).pos + pos;
                    metal_windows.iter_mut().for_each( | w | {
                        w.cocoa_window.set_ime_spot(pos);
                    });
                },
                CxOsOp::HideTextIME => {
                    //todo!()
                },
                CxOsOp::SetCursor(cursor) => {
                    cocoa_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    cocoa_app.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    cocoa_app.stop_timer(timer_id);
                },
                CxOsOp::StartDragging(dragged_item) => {
                    cocoa_app.start_dragging(dragged_item);
                }
                CxOsOp::UpdateMenu(menu) => {
                    cocoa_app.update_app_menu(&menu, &self.command_settings)
                }
            }
        }
    }
    /*
    fn handle_core_midi_signals(&mut self, se: &SignalEvent) {
        
        if self.platform.midi_access.is_some() {
            if se.signals.contains(&live_id!(CoreMidiInputData).into()) {
                let out_data = if let Ok(data) = self.platform.midi_input_data.lock() {
                    let mut data = data.borrow_mut();
                    let out_data = data.clone();
                    data.clear();
                    out_data
                }
                else {
                    panic!();
                };
                self.call_event_handler(&mut Event::Midi1InputData(out_data));
            }
            else if se.signals.contains(&live_id!(CoreMidiInputsChanged).into()) {
                let inputs = self.platform.midi_access.as_ref().unwrap().connect_all_inputs();
                self.call_event_handler(&mut Event::MidiInputList(MidiInputListEvent {inputs}));
            }
        }
    }*/
    
}

impl CxOsApi for Cx {
    fn init(&mut self) {
        self.live_expand();
        self.live_scan_dependencies();
        self.desktop_load_dependencies();
    }
    
    fn post_signal(signal: Signal) {
        for arg in std::env::args() {
            if arg == "--stdin-loop" {
                return Self::stdin_post_signal(signal);
            }
        }
        CocoaApp::post_signal(signal);
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn web_socket_open(&mut self, _url: String, _rec: WebSocketAutoReconnect) -> WebSocket {
        todo!()
    }
    
    fn web_socket_send(&mut self, _websocket: WebSocket, _data: Vec<u8>) {
        todo!()
    }
    /*
    fn start_midi_input(&mut self) {
        let midi_input_data = self.platform.midi_input_data.clone();
        
        if self.platform.midi_access.is_none() {
            if let Ok(ma) = CoreMidiAccess::new_midi_1_input(
                move | datas | {
                    if let Ok(midi_input_data) = midi_input_data.lock() {
                        let mut midi_input_data = midi_input_data.borrow_mut();
                        midi_input_data.extend_from_slice(&datas);
                        Cx::post_signal(live_id!(CoreMidiInputData).into());
                    }
                },
                move || {
                    Cx::post_signal(live_id!(CoreMidiInputsChanged).into());
                }
            ) {
                self.platform.midi_access = Some(ma);
            }
        }
        Cx::post_signal(live_id!(CoreMidiInputsChanged).into());
    }*/
    /*
    
    fn spawn_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static {
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let out = &AudioUnitFactory::query_audio_units(AudioUnitType::DefaultOutput)[0];
            let fbox = fbox.clone();
            AudioUnitFactory::new_audio_unit(out, move | result | {
                match result {
                    Ok(audio_unit) => {
                        let fbox = fbox.clone();
                        audio_unit.set_input_callback(move | time, output | {
                            if let Ok(mut fbox) = fbox.lock() {
                                fbox(time, output);
                            }
                        });
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                    Err(err) => error!("spawn_audio_output Error {:?}", err)
                }
            });
        });
    }*/
}

#[derive(Default)]
pub struct CxOs {
    pub (crate) keep_alive_counter: usize,
    //pub (crate)midi_access: Option<CoreMidiAccess>,
    //pub (crate)midi_input_data: Arc<Mutex<RefCell<Vec<Midi1InputData >> >>,
    pub (crate)last_mouse_button: Option<usize>,
    pub (crate) bytes_written: usize,
    pub (crate) draw_calls_done: usize,
}
