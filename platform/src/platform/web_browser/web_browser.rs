
use {
    std::rc::Rc,
    std::cell::RefCell,
    crate::{
        makepad_live_id::*,
        makepad_math::Vec2,
        makepad_error_log::*,
        makepad_wasm_bridge::{WasmDataU8, FromWasmMsg, ToWasmMsg, FromWasm, ToWasm},
        platform::{
            web_browser::{
                from_wasm::*,
                to_wasm::*,
            },
            web_audio::*,
        },
        audio::{
            AudioTime,
            AudioOutputBuffer
        },
        window::{
            CxWindowPool
        },
        event::{
            WebSocket,
            WebSocketErrorEvent,
            WebSocketMessageEvent,
            WebSocketAutoReconnect,
            Signal,
            Event,
            XRInput,
            TextCopyEvent,
            TimerEvent,
            WindowGeom,
            WindowGeomChangeEvent
        },
        cx_api::{CxPlatformApi, CxPlatformOp},
        cx::{Cx},
    }
};

impl Cx {
    
    pub fn get_default_window_size(&self) -> Vec2 {
        return self.platform.window_geom.inner_size;
    }
    
    pub fn process_to_wasm(&mut self, msg_ptr: u32) -> u32
    //where F: FnMut(&mut Cx, &Event),
    {
        //self.event_handler = Some(&mut event_handler as *const dyn FnMut(&mut Cx, &Event) as *mut dyn FnMut(&mut Cx, &Event));
        let ret = self.event_loop_core(ToWasmMsg::take_ownership(msg_ptr));
        //self.event_handler = None;
        ret
    }
    
    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn event_loop_core(&mut self, mut to_wasm: ToWasmMsg) -> u32 {
        
        self.platform.from_wasm = Some(FromWasmMsg::new());
        
        let mut is_animation_frame = false;
        while !to_wasm.was_last_block() {
            let block_id = LiveId(to_wasm.read_u64());
            let skip = to_wasm.read_block_skip();
            match block_id {
                id!(ToWasmGetDeps) => { // fetch_deps
                    let tw = ToWasmGetDeps::read_to_wasm(&mut to_wasm);
                    self.cpu_cores = tw.cpu_cores as usize;
                    self.gpu_info.init_from_info(
                        tw.gpu_info.min_uniform_vectors,
                        tw.gpu_info.vendor,
                        tw.gpu_info.renderer
                    );
                    self.platform_type = tw.browser_info.into();
                    
                    let mut deps = Vec::<String>::new();
                    for (path, _) in &self.dependencies {
                        deps.push(path.to_string());
                    }
                    
                    self.platform.from_wasm(
                        FromWasmLoadDeps {deps}
                    );
                },
                
                id!(ToWasmInit) => {
                    let tw = ToWasmInit::read_to_wasm(&mut to_wasm);
                    
                    for dep_in in tw.deps {
                        if let Some(dep) = self.dependencies.get_mut(&dep_in.path) {
                            
                            dep.data = Some(Ok(dep_in.data.into_vec_u8()))
                        }
                    }
                    self.platform.window_geom = tw.window_info.into();
                    
                    self.call_event_handler(&Event::Construct);
                    //self.platform.from_wasm(FromWasmCreateThread{thread_id:1});
                },
                
                id!(ToWasmResizeWindow) => {
                    let tw = ToWasmResizeWindow::read_to_wasm(&mut to_wasm);
                    let old_geom = self.platform.window_geom.clone();
                    let new_geom = tw.window_info.into();
                    if old_geom != new_geom {
                        self.platform.window_geom = new_geom.clone();
                        let id_zero = CxWindowPool::id_zero();
                        self.windows[id_zero].window_geom = new_geom.clone();
                        self.call_event_handler(&Event::WindowGeomChange(WindowGeomChangeEvent {
                            window_id: id_zero,
                            old_geom: old_geom,
                            new_geom: new_geom
                        }));
                        self.redraw_all();
                    }
                }
                
                id!(ToWasmAnimationFrame) => {
                    let tw = ToWasmAnimationFrame::read_to_wasm(&mut to_wasm);
                    is_animation_frame = true;
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(tw.time);
                    }
                }
                
                id!(ToWasmTouchStart) => {
                    let tw = ToWasmTouchStart::read_to_wasm(&mut to_wasm);
                    
                    // lets get a unique digit
                    let digit_id = id_num!(touch, tw.touch.uid as u64).into();
                    self.fingers.alloc_digit(digit_id);
                    
                    self.fingers.process_tap_count(
                        digit_id,
                        Vec2 {x: tw.touch.x, y: tw.touch.y},
                        tw.touch.time
                    );
                    self.call_event_handler(&Event::FingerDown(
                        tw.into_finger_down_event(&self.fingers, digit_id,)
                    ));
                }
                
                id!(ToWasmTouchMove) => {
                    let tw = ToWasmTouchMove::read_to_wasm(&mut to_wasm);
                    let digit_id = id_num!(touch, tw.touch.uid as u64).into();
                    // lets grab the captured area
                    self.call_event_handler(&Event::FingerMove(
                        tw.into_finger_move_event(&self.fingers, digit_id)
                    ));
                    
                }
                
                id!(ToWasmTouchEnd) => {
                    let tw = ToWasmTouchEnd::read_to_wasm(&mut to_wasm);
                    let digit_id = id_num!(touch, tw.touch.uid as u64).into();
                    self.call_event_handler(&Event::FingerUp(
                        tw.into_finger_up_event(&self.fingers, digit_id)
                    ));
                    
                    self.fingers.free_digit(digit_id);
                }
                
                id!(ToWasmMouseDown) => {
                    let tw = ToWasmMouseDown::read_to_wasm(&mut to_wasm);
                    
                    if self.platform.last_mouse_button == None ||
                    self.platform.last_mouse_button == Some(tw.mouse.button) {
                        self.platform.last_mouse_button = Some(tw.mouse.button);
                        // lets get a unique digit
                        let digit_id = id!(mouse).into();
                        self.fingers.alloc_digit(digit_id);
                        
                        self.fingers.process_tap_count(
                            digit_id,
                            Vec2 {x: tw.mouse.x, y: tw.mouse.y},
                            tw.mouse.time
                        );
                        
                        self.call_event_handler(&Event::FingerDown(
                            tw.into_finger_down_event(&self.fingers, digit_id)
                        ));
                    }
                }
                
                id!(ToWasmMouseMove) => {
                    let digit_id = id!(mouse).into();
                    let tw = ToWasmMouseMove::read_to_wasm(&mut to_wasm);
                    // ok so. what do we do without a captured area
                    // if our digit is NOT down we send hovers.
                    if !self.fingers.is_digit_allocated(digit_id) {
                        let hover_last = self.fingers.get_hover_area(digit_id);
                        self.call_event_handler(&Event::FingerHover(
                            tw.into_finger_hover_event(
                                digit_id,
                                hover_last,
                                self.platform.last_mouse_button.unwrap_or(0) as usize
                            )
                        ));
                        self.fingers.cycle_hover_area(digit_id);
                    }
                    else {
                        self.call_event_handler(&Event::FingerMove(
                            tw.into_finger_move_event(
                                &self.fingers,
                                digit_id,
                                self.platform.last_mouse_button.unwrap_or(0) as usize
                            )
                        ));
                    }
                }
                
                id!(ToWasmMouseUp) => {
                    let tw = ToWasmMouseUp::read_to_wasm(&mut to_wasm);
                    
                    if self.platform.last_mouse_button == Some(tw.mouse.button) {
                        self.platform.last_mouse_button = None;
                        let digit_id = id!(mouse).into();
                        let captured = self.fingers.get_captured_area(digit_id);
                        let digit_index = self.fingers.get_digit_index(digit_id);
                        let digit_count = self.fingers.get_digit_count();
                        self.call_event_handler(&Event::FingerUp(
                            tw.into_finger_up_event(
                                digit_id,
                                digit_index,
                                digit_count,
                                captured
                            )
                        ));
                        self.fingers.free_digit(digit_id);
                    }
                }
                
                id!(ToWasmScroll) => {
                    let tw = ToWasmScroll::read_to_wasm(&mut to_wasm);
                    let digit_id = id!(mouse).into();
                    self.call_event_handler(&Event::FingerScroll(
                        tw.into_finger_scroll_event(digit_id)
                    ));
                }
                
                id!(ToWasmKeyDown) => {
                    let tw = ToWasmKeyDown::read_to_wasm(&mut to_wasm);
                    self.keyboard.process_key_down(tw.key.clone().into());
                    self.call_event_handler(&Event::KeyDown(tw.key.into()));
                }
                
                id!(ToWasmKeyUp) => {
                    let tw = ToWasmKeyUp::read_to_wasm(&mut to_wasm);
                    self.keyboard.process_key_up(tw.key.clone().into());
                    self.call_event_handler(&Event::KeyUp(tw.key.into()));
                }
                
                id!(ToWasmTextInput) => {
                    let tw = ToWasmTextInput::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::TextInput(tw.into()));
                }
                
                id!(ToWasmTextCopy) => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCopy(TextCopyEvent {
                        response: response.clone()
                    }));
                    let response = response.borrow_mut().take();
                    if let Some(response) = response {
                        self.platform.from_wasm(FromWasmTextCopyResponse {response});
                    }
                }
                
                id!(ToWasmTimerFired) => {
                    let tw = ToWasmTimerFired::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::Timer(TimerEvent {
                        timer_id: tw.timer_id as u64
                    }));
                }
                
                id!(ToWasmAppGotFocus) => {
                    self.call_event_handler(&Event::AppGotFocus);
                }
                
                id!(ToWasmAppLostFocus) => {
                    self.call_event_handler(&Event::AppLostFocus);
                }
                
                id!(ToWasmXRUpdate) => {
                    let tw = ToWasmXRUpdate::read_to_wasm(&mut to_wasm);
                    let event = Event::XRUpdate(
                        tw.into_xrupdate_event(self.platform.xr_last_inputs.take())
                    );
                    self.call_event_handler(&event);
                    if let Event::XRUpdate(event) = event {
                        self.platform.xr_last_inputs = Some(event.inputs);
                    }
                }
                
                id!(ToWasmRedrawAll) => {
                    self.redraw_all();
                }
                
                id!(ToWasmPaintDirty) => {
                    let main_pass_id = self.windows[CxWindowPool::id_zero()].main_pass_id.unwrap();
                    self.passes[main_pass_id].paint_dirty = true;
                }
                
                id!(ToWasmSignal) => {
                    let tw = ToWasmSignal::read_to_wasm(&mut to_wasm);
                    for sig in tw.signals {
                        let signal_id = ((sig.signal_hi as u64) << 32) | (sig.signal_lo as u64);
                        self.send_signal(Signal(LiveId(signal_id)));
                    }
                }
                
                id!(ToWasmWebSocketClose) => {
                    let tw = ToWasmWebSocketClose::read_to_wasm(&mut to_wasm);
                    let web_socket = WebSocket(tw.web_socket_id as u64);
                    self.call_event_handler(&Event::WebSocketClose(web_socket));
                }
                
                id!(ToWasmWebSocketOpen) => {
                    let tw = ToWasmWebSocketOpen::read_to_wasm(&mut to_wasm);
                    let web_socket = WebSocket(tw.web_socket_id as u64);
                    self.call_event_handler(&Event::WebSocketOpen(web_socket));
                }
                
                id!(ToWasmWebSocketError) => {
                    let tw = ToWasmWebSocketError::read_to_wasm(&mut to_wasm);
                    let web_socket = WebSocket(tw.web_socket_id as u64);
                    self.call_event_handler(&Event::WebSocketError(WebSocketErrorEvent {
                        web_socket,
                        error: tw.error,
                    }));
                }
                
                id!(ToWasmWebSocketMessage) => {
                    let tw = ToWasmWebSocketMessage::read_to_wasm(&mut to_wasm);
                    let web_socket = WebSocket(tw.web_socket_id as u64);
                    self.call_event_handler(&Event::WebSocketMessage(WebSocketMessageEvent {
                        web_socket,
                        data: tw.data.into_vec_u8()
                    }));
                }
                
                id!(ToWasmMidiInputData) => {
                    let tw = ToWasmMidiInputData::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::Midi1InputData(vec![tw.into()]));
                }
                
                id!(ToWasmMidiInputList) => {
                    let tw = ToWasmMidiInputList::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::MidiInputList(tw.into()));
                }
                
                _ => {
                    log!("Message not handled in wasm {}", block_id);
                    
                    //panic!("Message unknown")
                }
            };
            to_wasm.block_skip(skip);
        };
        
        if is_animation_frame {
            if self.need_redrawing() {
                self.call_draw_event();
                self.webgl_compile_shaders();
            }
            self.handle_repaint();
        }
        
        self.handle_platform_ops();
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 {
            self.platform.from_wasm(FromWasmRequestAnimationFrame {});
        }
        
        //return wasm pointer to caller
        self.platform.from_wasm.take().unwrap().release_ownership()
    }
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event) {
    }
    
    fn handle_platform_ops(&mut self) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxPlatformOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    self.platform.from_wasm(FromWasmSetDocumentTitle {
                        title: window.create_title.clone()
                    });
                    window.window_geom = self.platform.window_geom.clone();
                    window.is_created = true;
                },
                CxPlatformOp::CloseWindow(_window_id) => {
                },
                CxPlatformOp::MinimizeWindow(_window_id) => {
                },
                CxPlatformOp::MaximizeWindow(_window_id) => {
                },
                CxPlatformOp::RestoreWindow(_window_id) => {
                },
                CxPlatformOp::FullscreenWindow(_window_id) => {
                    self.platform.from_wasm(FromWasmFullScreen {});
                },
                CxPlatformOp::NormalizeWindow(_window_id) => {
                    self.platform.from_wasm(FromWasmNormalScreen {});
                }
                CxPlatformOp::SetTopmost(_window_id, _is_topmost) => {
                    todo!()
                }
                CxPlatformOp::XrStartPresenting(_) => {
                    self.platform.from_wasm(FromWasmXrStartPresenting {});
                },
                CxPlatformOp::XrStopPresenting(_) => {
                    self.platform.from_wasm(FromWasmXrStopPresenting {});
                },
                CxPlatformOp::ShowTextIME(pos) => {
                    self.platform.from_wasm(FromWasmShowTextIME {x: pos.x, y: pos.y});
                },
                CxPlatformOp::HideTextIME => {
                    self.platform.from_wasm(FromWasmHideTextIME {});
                },
                
                CxPlatformOp::SetCursor(cursor) => {
                    self.platform.from_wasm(FromWasmSetMouseCursor::new(cursor));
                },
                CxPlatformOp::StartTimer {timer_id, interval, repeats} => {
                    self.platform.from_wasm(FromWasmStartTimer {
                        repeats,
                        interval,
                        timer_id: timer_id as f64,
                    });
                },
                CxPlatformOp::StopTimer(timer_id) => {
                    self.platform.from_wasm(FromWasmStopTimer {
                        id: timer_id as f64,
                    });
                },
                CxPlatformOp::StartDragging(_dragged_item) => {
                }
                CxPlatformOp::UpdateMenu(_menu) => {
                }
            }
        }
    }
}


impl CxPlatformApi for Cx {
    
    fn post_signal(signal: Signal,) {
        unsafe {js_post_signal((signal.0.0 >> 32) as u32, signal.0.0 as u32)};
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        let closure_box: Box<dyn FnOnce() + Send + 'static> = Box::new(f);
        let closure_ptr = Box::into_raw(Box::new(closure_box));
        self.platform.from_wasm(FromWasmCreateThread {closure_ptr: closure_ptr as u32});
    }
    
    fn web_socket_open(&mut self, url: String, rec: WebSocketAutoReconnect) -> WebSocket {
        let web_socket_id = self.web_socket_id;
        self.web_socket_id += 1;
        
        self.platform.from_wasm(FromWasmWebSocketOpen {
            url,
            web_socket_id: web_socket_id as usize,
            auto_reconnect: if let WebSocketAutoReconnect::Yes = rec {true} else {false},
            
        });
        WebSocket(web_socket_id)
    }
    
    fn web_socket_send(&mut self, websocket: WebSocket, data: Vec<u8>) {
        self.platform.from_wasm(FromWasmWebSocketSend {
            web_socket_id: websocket.0 as usize,
            data: WasmDataU8::from_vec_u8(data)
        });
    }
    
    fn start_midi_input(&mut self) {
        self.platform.from_wasm(FromWasmStartMidiInput {
        });
    }
    
    fn spawn_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static {
        let closure_ptr = Box::into_raw(Box::new(WebAudioOutputClosure {
            callback: Box::new(f),
            output_buffer: WebAudioOutputBuffer::default()
        }));
        self.platform.from_wasm(FromWasmSpawnAudioOutput {closure_ptr: closure_ptr as u32});
    }
}

extern "C" {
    pub fn js_post_signal(signal_hi: u32, signal_lo: u32);
}

#[export_name = "wasm_thread_entrypoint"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_thread_entrypoint(closure_ptr: u32) {
    let closure = Box::from_raw(closure_ptr as *mut Box<dyn FnOnce() + Send + 'static>);
    closure();
}

#[export_name = "wasm_thread_alloc_tls_and_stack"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_thread_alloc_tls_and_stack(tls_size: u32) -> u32 {
    let mut v = Vec::<u64>::new();
    v.reserve_exact(tls_size as usize);
    let mut v = std::mem::ManuallyDrop::new(v);
    v.as_mut_ptr() as u32
}

// storage buffers for graphics API related platform
#[derive(Default)]
pub struct CxPlatform {
    pub window_geom: WindowGeom,
    pub last_mouse_button: Option<u32>,
    pub from_wasm: Option<FromWasmMsg>,
    pub vertex_buffers: usize,
    pub index_buffers: usize,
    pub vaos: usize,
    pub xr_last_inputs: Option<Vec<XRInput >>,
}

impl CxPlatform {
    pub fn from_wasm(&mut self, from_wasm: impl FromWasm) {
        self.from_wasm.as_mut().unwrap().from_wasm(from_wasm);
    }
}



#[export_name = "wasm_get_js_msg_class"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_get_js_msg_class() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
    
    out.push_str("return {\n");
    
    out.push_str("ToWasmMsg:class extends ToWasmMsg{\n");
    ToWasmGetDeps::to_wasm_js(&mut out);
    ToWasmInit::to_wasm_js(&mut out);
    ToWasmResizeWindow::to_wasm_js(&mut out);
    ToWasmAnimationFrame::to_wasm_js(&mut out);
    
    ToWasmTouchStart::to_wasm_js(&mut out);
    ToWasmTouchMove::to_wasm_js(&mut out);
    ToWasmTouchEnd::to_wasm_js(&mut out);
    ToWasmMouseDown::to_wasm_js(&mut out);
    ToWasmMouseMove::to_wasm_js(&mut out);
    ToWasmMouseUp::to_wasm_js(&mut out);
    ToWasmScroll::to_wasm_js(&mut out);
    
    ToWasmKeyDown::to_wasm_js(&mut out);
    ToWasmKeyUp::to_wasm_js(&mut out);
    ToWasmTextInput::to_wasm_js(&mut out);
    ToWasmTextCopy::to_wasm_js(&mut out);
    ToWasmTimerFired::to_wasm_js(&mut out);
    ToWasmPaintDirty::to_wasm_js(&mut out);
    ToWasmRedrawAll::to_wasm_js(&mut out);
    ToWasmXRUpdate::to_wasm_js(&mut out);
    ToWasmAppGotFocus::to_wasm_js(&mut out);
    ToWasmAppLostFocus::to_wasm_js(&mut out);
    ToWasmSignal::to_wasm_js(&mut out);
    ToWasmWebSocketOpen::to_wasm_js(&mut out);
    ToWasmWebSocketClose::to_wasm_js(&mut out);
    ToWasmWebSocketError::to_wasm_js(&mut out);
    ToWasmWebSocketMessage::to_wasm_js(&mut out);
    ToWasmMidiInputList::to_wasm_js(&mut out);
    ToWasmMidiInputData::to_wasm_js(&mut out);
    
    out.push_str("},\n");
    
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    FromWasmLoadDeps::from_wasm_js(&mut out);
    FromWasmStartTimer::from_wasm_js_reuse(&mut out);
    FromWasmStopTimer::from_wasm_js_reuse(&mut out);
    FromWasmFullScreen::from_wasm_js(&mut out);
    FromWasmNormalScreen::from_wasm_js(&mut out);
    FromWasmRequestAnimationFrame::from_wasm_js_reuse(&mut out);
    FromWasmSetDocumentTitle::from_wasm_js(&mut out);
    FromWasmSetMouseCursor::from_wasm_js(&mut out);
    FromWasmTextCopyResponse::from_wasm_js(&mut out);
    FromWasmShowTextIME::from_wasm_js(&mut out);
    FromWasmHideTextIME::from_wasm_js(&mut out);
    FromWasmCreateThread::from_wasm_js(&mut out);
    FromWasmWebSocketOpen::from_wasm_js(&mut out);
    FromWasmWebSocketSend::from_wasm_js(&mut out);
    FromWasmXrStartPresenting::from_wasm_js(&mut out);
    FromWasmXrStopPresenting::from_wasm_js(&mut out);
    FromWasmStartMidiInput::from_wasm_js(&mut out);
    FromWasmSpawnAudioOutput::from_wasm_js(&mut out);
    
    FromWasmCompileWebGLShader::from_wasm_js_reuse(&mut out);
    FromWasmAllocArrayBuffer::from_wasm_js_reuse(&mut out);
    FromWasmAllocIndexBuffer::from_wasm_js_reuse(&mut out);
    FromWasmAllocVao::from_wasm_js_reuse(&mut out);
    FromWasmAllocTextureImage2D::from_wasm_js_reuse(&mut out);
    FromWasmBeginRenderTexture::from_wasm_js_reuse(&mut out);
    FromWasmBeginRenderCanvas::from_wasm_js_reuse(&mut out);
    FromWasmSetDefaultDepthAndBlendMode::from_wasm_js_reuse(&mut out);
    FromWasmDrawCall::from_wasm_js_reuse(&mut out);
    
    out.push_str("}\n");
    
    out.push_str("}");
    
    msg.push_str(&out);
    msg.release_ownership()
}
