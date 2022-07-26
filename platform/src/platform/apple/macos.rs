use {
    std::cell::RefCell,
    std::sync::{Arc, Mutex},
    crate::{
        makepad_live_id::*,
        platform::{
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

impl Cx {
    
    pub fn event_loop<F>(&mut self, mut event_handler: F)
    where F: FnMut(&mut Cx, &mut Event),
    {
        self.event_handler = Some(&mut event_handler as *const dyn FnMut(&mut Cx, &mut Event) as *mut dyn FnMut(&mut Cx, &mut Event));
        self.event_loop_core();
        self.event_handler = None;
    }
    
    pub fn event_loop_core(&mut self) {
        self.platform_type = PlatformType::OSX;
        
        init_cocoa_globals();
        
        get_cocoa_app_global().init();
        
        let mut metal_cx = MetalCx::new();
        let mut metal_windows: Vec<MetalWindow> = Vec::new();
        
        self.call_event_handler(&mut Event::Construct);
        self.redraw_all();
        
        const KEEP_ALIVE_COUNT: usize = 5;
        let mut keep_alive_counter = 0;
        
        // keep alive timer
        get_cocoa_app_global().start_timer(0, 0.2, true);
        
        get_cocoa_app_global().event_loop( | cocoa_app, events | {
            
            self.handle_platform_ops(&mut metal_windows, &mut metal_cx, cocoa_app);
            
            let mut paint_dirty = false;
            for mut event in events {
                match &event {
                    Event::FingerDown(_) |
                    Event::FingerMove(_) |
                    Event::FingerHover(_) |
                    Event::FingerUp(_) |
                    Event::FingerScroll(_) |
                    Event::KeyDown(_) |
                    Event::KeyUp(_) |
                    Event::TextInput(_) => {
                        keep_alive_counter = KEEP_ALIVE_COUNT;
                    }
                    Event::Timer(te) => {
                        if te.timer_id == 0 {
                            if keep_alive_counter>0 {
                                keep_alive_counter -= 1;
                                self.repaint_windows();
                                paint_dirty = true;
                            }
                            continue;
                        }
                    }
                    _ => ()
                }
                self.process_desktop_pre_event(&mut event);
                match &event {
                    Event::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                        for mw in metal_windows.iter_mut() {
                            if let Some(main_pass_id) = self.windows[mw.window_id].main_pass_id {
                                self.repaint_pass(main_pass_id);
                            }
                        }
                        paint_dirty = true;
                        self.call_event_handler(&mut event);
                    }
                    Event::WindowResizeLoop(wr) => {
                        if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == wr.window_id) {
                            if wr.was_started {
                                metal_window.start_resize();
                            }
                            else {
                                metal_window.stop_resize();
                            }
                        }
                    },
                    Event::WindowGeomChange(re) => { // do this here because mac
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
                        self.call_event_handler(&mut event);
                    },
                    Event::WindowClosed(wc) => {
                        // lets remove the window from the set
                        self.windows[wc.window_id].is_created = false;
                        if let Some(index) = metal_windows.iter().position( | w | w.window_id == wc.window_id) {
                            metal_windows.remove(index);
                            if metal_windows.len() == 0 {
                                cocoa_app.terminate_event_loop();
                            }
                        }
                        self.call_event_handler(&mut event);
                    },
                    Event::Paint => {
                        if self.new_next_frames.len() != 0 {
                            self.call_next_frame_event(cocoa_app.time_now());
                        }
                        if self.need_redrawing() {
                            self.call_draw_event();
                            self.mtl_compile_shaders(&metal_cx);
                        }
                        self.handle_repaint(&mut metal_windows, &mut metal_cx);
                    },
                    Event::Signal(se) => {
                        self.handle_core_midi_signals(se);
                        self.call_event_handler(&mut event);
                    },
                    _ => {
                        self.call_event_handler(&mut event);
                    }
                }
                if self.process_desktop_post_event(event) {
                    cocoa_app.terminate_event_loop();
                }
            }
            
            if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
                false
            } else {
                true
            }
        })
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
                    todo!()
                },
                CxPlatformOp::SetHoverCursor(cursor) => {
                    cocoa_app.set_mouse_cursor(cursor);
                },
                CxPlatformOp::SetDownCursor(cursor) => {
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
        
        //if !set_cursor {
        //    cocoa_app.set_mouse_cursor(MouseCursor::Default)
        //}
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
                    Err(err) => println!("Error {:?}", err)
                }
            });
        });
    }
}

#[derive(Default)]
pub struct CxPlatform {
    pub midi_access: Option<CoreMidiAccess>,
    pub midi_input_data: Arc<Mutex<RefCell<Vec<Midi1InputData >> >>,
    pub bytes_written: usize,
    pub draw_calls_done: usize,
    pub text_clipboard_response: Option<String>,
}