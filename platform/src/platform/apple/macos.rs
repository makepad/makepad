use {
    std::rc::Rc,
    std::cell::{RefCell},
    std::sync::{Arc, Mutex},
    crate::{
        makepad_error_log::*,
        makepad_live_id::*,
        platform::{
            cocoa_event::{CocoaEvent},
            core_midi::CoreMidiAccess,
            cocoa_app::{CocoaApp, get_cocoa_app_global, init_cocoa_globals},
            metal::{MetalCx, MetalWindow},
            audio_unit::*,
        },
        audio::{
            AudioTime,
            AudioOutputBuffer
        },
        midi::{
            Midi1InputData
        },
        event::{
            WebSocket,
            WebSocketAutoReconnect,
            Signal,
            SignalEvent,
            Event,
            MidiInputListEvent,
        },
        cx_api::{CxPlatformApi, CxPlatformOp},
        cx::{Cx, PlatformType},
    }
};

const KEEP_ALIVE_COUNT: usize = 5;

impl Cx {
    
    pub fn event_loop(mut self) {
        self.platform_type = PlatformType::OSX;
        
        let metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        let metal_windows = Rc::new(RefCell::new(Vec::new()));
        let cx = Rc::new(RefCell::new(self));
        
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
        get_cocoa_app_global().start_timer(0, 0.2, true);
        cx.borrow_mut().call_event_handler(&Event::Construct);
        cx.borrow_mut().redraw_all();
        get_cocoa_app_global().event_loop();
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
                    self.platform.keep_alive_counter = KEEP_ALIVE_COUNT;
                }
                CocoaEvent::Timer(te) => {
                    if te.timer_id == 0 {
                        if self.platform.keep_alive_counter>0 {
                            self.platform.keep_alive_counter -= 1;
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
                    self.handle_repaint(metal_windows, metal_cx);
                }
                CocoaEvent::MouseDown(md) => {
                    if self.platform.last_mouse_button == None ||
                    self.platform.last_mouse_button == Some(md.button) {
                        self.platform.last_mouse_button = Some(md.button);
                        let digit_id = id!(mouse).into();
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
                    let digit_id = id!(mouse).into();
                    
                    if !self.fingers.is_digit_allocated(digit_id) {
                        let area = self.fingers.get_hover_area(digit_id);
                        self.call_event_handler(&Event::FingerHover(
                            mm.into_finger_hover_event(
                                digit_id,
                                area,
                                self.platform.last_mouse_button.unwrap_or(0)
                            )
                        ));
                        self.fingers.cycle_hover_area(digit_id);
                    }
                    else {
                        self.call_event_handler(&mut Event::FingerMove(
                            mm.into_finger_move_event(
                                &self.fingers,
                                digit_id,
                                self.platform.last_mouse_button.unwrap_or(0)
                            )
                        ));
                    }
                }
                CocoaEvent::MouseUp(md) => {
                    if self.platform.last_mouse_button == Some(md.button) {
                        self.platform.last_mouse_button = None;
                        let digit_id = id!(mouse).into();
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
                        e.into_finger_scroll_event(id!(mouse).into())
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
                    self.handle_core_midi_signals(&se);
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
                CxPlatformOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        cocoa_app,
                        window.create_inner_size,
                        window.create_position,
                        &window.create_title
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;
                },
                CxPlatformOp::CloseWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        self.windows[window_id].is_created = false;
                        metal_window.cocoa_window.close_window();
                        break;
                    }
                },
                CxPlatformOp::MinimizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.minimize();
                    }
                },
                CxPlatformOp::MaximizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.maximize();
                    }
                },
                CxPlatformOp::RestoreWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.restore();
                    }
                },
                CxPlatformOp::FullscreenWindow(_window_id) => {
                    todo!()
                },
                CxPlatformOp::NormalizeWindow(_window_id) => {
                    todo!()
                }
                CxPlatformOp::SetTopmost(_window_id, _is_topmost) => {
                    todo!()
                }
                CxPlatformOp::XrStartPresenting(_) => {
                    todo!()
                },
                CxPlatformOp::XrStopPresenting(_) => {
                    todo!()
                },
                CxPlatformOp::ShowTextIME(pos) => {
                    metal_windows.iter_mut().for_each( | w | {
                        w.cocoa_window.set_ime_spot(pos);
                    });
                },
                CxPlatformOp::HideTextIME => {
                    //todo!()
                },
                CxPlatformOp::SetCursor(cursor) => {
                    cocoa_app.set_mouse_cursor(cursor);
                },
                CxPlatformOp::StartTimer {timer_id, interval, repeats} => {
                    cocoa_app.start_timer(timer_id, interval, repeats);
                },
                CxPlatformOp::StopTimer(timer_id) => {
                    cocoa_app.stop_timer(timer_id);
                },
                CxPlatformOp::StartDragging(dragged_item) => {
                    cocoa_app.start_dragging(dragged_item);
                }
                CxPlatformOp::UpdateMenu(menu) => {
                    cocoa_app.update_app_menu(&menu, &self.command_settings)
                }
            }
        }
    }
    
    fn handle_core_midi_signals(&mut self, se: &SignalEvent) {
        
        if self.platform.midi_access.is_some() {
            if se.signals.contains(&id!(CoreMidiInputData).into()) {
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
            else if se.signals.contains(&id!(CoreMidiInputsChanged).into()) {
                let inputs = self.platform.midi_access.as_ref().unwrap().connect_all_inputs();
                self.call_event_handler(&mut Event::MidiInputList(MidiInputListEvent {inputs}));
            }
        }
    }
    
}

impl CxPlatformApi for Cx {
    
    fn post_signal(signal: Signal) {
        CocoaApp::post_signal(signal.0.0);
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
    
    fn start_midi_input(&mut self) {
        let midi_input_data = self.platform.midi_input_data.clone();
        
        if self.platform.midi_access.is_none() {
            if let Ok(ma) = CoreMidiAccess::new_midi_1_input(
                move | datas | {
                    if let Ok(midi_input_data) = midi_input_data.lock() {
                        let mut midi_input_data = midi_input_data.borrow_mut();
                        midi_input_data.extend_from_slice(&datas);
                        Cx::post_signal(id!(CoreMidiInputData).into());
                    }
                },
                move || {
                    Cx::post_signal(id!(CoreMidiInputsChanged).into());
                }
            ) {
                self.platform.midi_access = Some(ma);
            }
        }
        Cx::post_signal(id!(CoreMidiInputsChanged).into());
    }
    
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
    }
}

#[derive(Default)]
pub (crate) struct CxPlatform {
    pub (crate) keep_alive_counter: usize,
    pub (crate)midi_access: Option<CoreMidiAccess>,
    pub (crate)midi_input_data: Arc<Mutex<RefCell<Vec<Midi1InputData >> >>,
    pub (crate)last_mouse_button: Option<usize>,
    pub (crate) bytes_written: usize,
    pub (crate) draw_calls_done: usize,
}
