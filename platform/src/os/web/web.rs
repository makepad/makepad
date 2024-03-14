
use {
    std::panic,
    std::rc::Rc,
    std::cell::RefCell,
    self::super::{
        web_media::CxWebMedia,
        from_wasm::*,
        to_wasm::*,
    },
    crate::{
        makepad_live_compiler::LiveFileChange,
        makepad_live_id::*,
        makepad_wasm_bridge::{WasmDataU8, FromWasmMsg, ToWasmMsg, FromWasm, ToWasm},
        thread::SignalToUI,
        window::{
            CxWindowPool
        },
        event::{
            ToWasmMsgEvent,
            NetworkResponseItem,
            HttpResponse,
            NetworkResponse,
            Event,
            XRInput,
            TextClipboardEvent,
            TimerEvent,
            MouseDownEvent,
            MouseMoveEvent,
            MouseUpEvent,
            TouchUpdateEvent,
            ScrollEvent,
            WindowGeom,
            WindowGeomChangeEvent
        },
        pass::CxPassParent,
        cx_api::{CxOsApi, CxOsOp},
        cx::{Cx},
    }
};

impl Cx {
    
    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn process_to_wasm(&mut self, msg_ptr: u32) -> u32 {
        
        let mut to_wasm_msg = ToWasmMsg::take_ownership(msg_ptr);
        let mut network_responses = Vec::new();
        self.os.from_wasm = Some(FromWasmMsg::new());
        let mut to_wasm = to_wasm_msg.as_ref();
        let mut is_animation_frame = None;
        while !to_wasm.was_last_block() {
            let block_id = LiveId(to_wasm.read_u64());
            let skip = to_wasm.read_block_skip();
            match block_id {
                live_id!(ToWasmGetDeps) => { // fetch_deps
                    let tw = ToWasmGetDeps::read_to_wasm(&mut to_wasm);
                    self.cpu_cores = tw.cpu_cores as usize;
                    self.gpu_info.init_from_info(
                        tw.gpu_info.min_uniform_vectors,
                        tw.gpu_info.vendor,
                        tw.gpu_info.renderer
                    );
                    self.os_type = tw.browser_info.into();
                    self.xr_capabilities = tw.xr_capabilities.into();
                    
                    let mut deps = Vec::<String>::new();
                    for (path, _) in &self.dependencies {
                        deps.push(path.to_string());
                    }
                    
                    self.os.from_wasm(
                        FromWasmLoadDeps {deps}
                    );
                },
                
                live_id!(ToWasmInit) => {
                    let tw = ToWasmInit::read_to_wasm(&mut to_wasm);
                    
                    for dep_in in tw.deps {
                        if let Some(dep) = self.dependencies.get_mut(&dep_in.path) {
                            
                            dep.data = Some(Ok(Rc::new(dep_in.data.into_vec_u8())))
                        }
                    }
                    self.os.window_geom = tw.window_info.into();
                    //self.default_inner_window_size = self.os.window_geom.inner_size;
                    
                    self.call_event_handler(&Event::Startup);
                    //self.platform.from_wasm(FromWasmCreateThread{thread_id:1});
                },
                
                live_id!(ToWasmResizeWindow) => {
                    let tw = ToWasmResizeWindow::read_to_wasm(&mut to_wasm);
                    let old_geom = self.os.window_geom.clone();
                    let new_geom = tw.window_info.into();
                    if old_geom != new_geom {
                        self.os.window_geom = new_geom.clone();
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
                
                live_id!(ToWasmAnimationFrame) => {
                    let tw = ToWasmAnimationFrame::read_to_wasm(&mut to_wasm);
                    is_animation_frame = Some(tw.time);
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(tw.time);
                    }
                }
                
                live_id!(ToWasmTouchUpdate) => {
                    let e: TouchUpdateEvent = ToWasmTouchUpdate::read_to_wasm(&mut to_wasm).into();
                    self.fingers.process_touch_update_start(e.time, &e.touches);
                    let e = Event::TouchUpdate(e);
                    self.call_event_handler(&e);
                    let e = if let Event::TouchUpdate(e) = e{e}else{panic!()};
                    self.fingers.process_touch_update_end(&e.touches);
                }
                
                live_id!(ToWasmMouseDown) => {
                    let e: MouseDownEvent = ToWasmMouseDown::read_to_wasm(&mut to_wasm).into();
                    self.fingers.process_tap_count(e.abs, e.time);
                    self.fingers.mouse_down(e.button);
                    self.call_event_handler(&Event::MouseDown(e))
                }
                
                live_id!(ToWasmMouseMove) => {
                    let e: MouseMoveEvent = ToWasmMouseMove::read_to_wasm(&mut to_wasm).into();
                    self.call_event_handler(&Event::MouseMove(e.into()));
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                    self.fingers.switch_captures();
                }
                
                live_id!(ToWasmMouseUp) => {
                    let e: MouseUpEvent = ToWasmMouseUp::read_to_wasm(&mut to_wasm).into();
                    let button = e.button;
                    self.call_event_handler(&Event::MouseUp(e.into()));
                    self.fingers.mouse_up(button);
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                }
                
                live_id!(ToWasmScroll) => {
                    let e: ScrollEvent = ToWasmScroll::read_to_wasm(&mut to_wasm).into();
                    self.call_event_handler(&Event::Scroll(e.into()));
                }
                
                live_id!(ToWasmKeyDown) => {
                    let tw = ToWasmKeyDown::read_to_wasm(&mut to_wasm);
                    self.keyboard.process_key_down(tw.key.clone().into());
                    self.call_event_handler(&Event::KeyDown(tw.key.into()));
                }
                
                live_id!(ToWasmKeyUp) => {
                    let tw = ToWasmKeyUp::read_to_wasm(&mut to_wasm);
                    self.keyboard.process_key_up(tw.key.clone().into());
                    self.call_event_handler(&Event::KeyUp(tw.key.into()));
                }
                
                live_id!(ToWasmTextInput) => {
                    let tw = ToWasmTextInput::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::TextInput(tw.into()));
                }
                
                live_id!(ToWasmTextCopy) => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCopy(TextClipboardEvent {
                        response: response.clone()
                    }));
                    let response = response.borrow_mut().take();
                    if let Some(response) = response {
                        self.os.from_wasm(FromWasmTextCopyResponse {response});
                    }
                }
                
                live_id!(ToWasmSignal) =>{
                    self.handle_media_signals();
                    self.call_event_handler(&Event::Signal);
                }
                
                live_id!(ToWasmTimerFired) => {
                    let tw = ToWasmTimerFired::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::Timer(TimerEvent {
                        timer_id: tw.timer_id as u64,
                        time: None
                    }));
                }
                
                live_id!(ToWasmAppGotFocus) => {
                    self.call_event_handler(&Event::AppGotFocus);
                }
                
                live_id!(ToWasmAppLostFocus) => {
                    self.call_event_handler(&Event::AppLostFocus);
                }
                
                live_id!(ToWasmXRUpdate) => {
                    let tw = ToWasmXRUpdate::read_to_wasm(&mut to_wasm);
                    let event = Event::XRUpdate(
                        tw.into_xrupdate_event(self.os.xr_last_inputs.take())
                    );
                    self.call_event_handler(&event);
                    if let Event::XRUpdate(event) = event {
                        self.os.xr_last_inputs = Some(event.inputs);
                    }
                }
                
                live_id!(ToWasmRedrawAll) => {
                    self.redraw_all();
                }
                
                live_id!(ToWasmPaintDirty) => {
                    let main_pass_id = self.windows[CxWindowPool::id_zero()].main_pass_id.unwrap();
                    self.passes[main_pass_id].paint_dirty = true;
                }

                live_id!(ToWasmHTTPResponse) => {
                    let tw = ToWasmHTTPResponse::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseItem{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::HttpResponse(HttpResponse::new(
                            LiveId::from_lo_hi(tw.metadata_id_lo, tw.metadata_id_hi),
                            tw.status as u16,
                            tw.headers,
                            Some(tw.body.into_vec_u8())
                        ))
                    });
                }

                live_id!(ToWasmHttpRequestError) => {
                    let tw = ToWasmHttpRequestError::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseItem{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::HttpRequestError(tw.error)
                    });
                }

                live_id!(ToWasmHttpResponseProgress) => {
                    let tw = ToWasmHttpResponseProgress::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseItem{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::HttpProgress{loaded:tw.loaded as u64, total:tw.total as u64}
                    });
                }

                live_id!(ToWasmHttpUploadProgress) => {
                    let tw = ToWasmHttpUploadProgress::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseItem{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::HttpProgress{loaded:tw.loaded as u64, total:tw.total as u64}
                    });
                }
                /*
                live_id!(ToWasmWebSocketClose) => {
                    let tw = ToWasmWebSocketClose::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketClose
                    });
                }
                
                live_id!(ToWasmWebSocketOpen) => {
                    let tw = ToWasmWebSocketOpen::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketOpen
                    });
                }
                
                live_id!(ToWasmWebSocketError) => {
                    let tw = ToWasmWebSocketError::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketError(tw.error)
                    });
                }
                live_id!(ToWasmWebSocketString) => {
                    let tw = ToWasmWebSocketString::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketString(tw.data)
                    });
                }
                live_id!(ToWasmWebSocketBinary) => {
                    let tw = ToWasmWebSocketBinary::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketBinary(tw.data.into_vec_u8())
                    });
                }*/
                live_id!(ToWasmLiveFileChange)=>{
                    let tw = ToWasmLiveFileChange::read_to_wasm(&mut to_wasm);
                    // live file change. lets do it.
                    if tw.body.len()>0 {
                        let mut parts = tw.body.split("$$$makepad_live_change$$$");
                        if let Some(file_name) = parts.next() {
                            let content = parts.next().unwrap().to_string();
                            let _ = self.live_file_change_sender.send(vec![LiveFileChange{
                                file_name:file_name.to_string(),
                                content
                            }]);
                        }
                    }
                }
                live_id!(ToWasmAudioDeviceList)=>{
                    let tw = ToWasmAudioDeviceList::read_to_wasm(&mut to_wasm);
                    self.os.web_audio().lock().unwrap().to_wasm_audio_device_list(tw);
                }
                live_id!(ToWasmMidiPortList)=>{
                    let tw = ToWasmMidiPortList::read_to_wasm(&mut to_wasm);
                    self.os.web_midi().lock().unwrap().to_wasm_midi_port_list(tw);
                }
                live_id!(ToWasmMidiInputData)=>{
                    let tw = ToWasmMidiInputData::read_to_wasm(&mut to_wasm);
                    self.os.web_midi().lock().unwrap().to_wasm_midi_input_data(tw);
                }
                msg_id => {
                    // swap the message into an event to avoid a copy
                    let offset = to_wasm.u32_offset;
                    drop(to_wasm);
                    let event = Event::ToWasmMsg(ToWasmMsgEvent {id: msg_id, msg: to_wasm_msg, offset});
                    self.call_event_handler(&event);
                    // and swap it back
                    if let Event::ToWasmMsg(ToWasmMsgEvent {msg, ..}) = event {to_wasm_msg = msg}else {panic!()};
                    to_wasm = to_wasm_msg.as_ref();
                }
            };
            to_wasm.block_skip(skip);
        };
        
        if self.handle_live_edit(){
            self.call_event_handler(&Event::LiveEdit);
            self.redraw_all();
        }

        if let Some(time) = is_animation_frame {
            if self.need_redrawing() {
                self.call_draw_event();
                self.webgl_compile_shaders();
            }
            self.handle_repaint(time);
        }

        if network_responses.len() != 0 {
            self.call_event_handler(&Event::NetworkResponses(network_responses));
        }

        self.handle_platform_ops();
        self.handle_media_signals();
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 {
            self.os.from_wasm(FromWasmRequestAnimationFrame {});
        }
        
        //return wasm pointer to caller
        self.os.from_wasm.take().unwrap().release_ownership()
    }
    
         
    pub fn handle_repaint(&mut self, time: f64){
        let mut passes_todo = Vec::new();
         
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            self.passes[*pass_id].set_time(time as f32);
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_) => {
                    //et dpi_factor = self.os.window_geom.dpi_factor;
                    self.draw_pass_to_canvas(*pass_id);
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(*pass_id);
                },
                CxPassParent::None => {
                    self.draw_pass_to_texture(*pass_id);
                }
            }
        }    
    }
    
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event) {
    }
    
    fn handle_platform_ops(&mut self) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    self.os.from_wasm(FromWasmSetDocumentTitle {
                        title: window.create_title.clone()
                    });
                    window.window_geom = self.os.window_geom.clone();
                    window.is_created = true;
                },
                CxOsOp::Quit=>{}
                CxOsOp::CloseWindow(_window_id) => {
                },
                CxOsOp::MinimizeWindow(_window_id) => {
                },
                CxOsOp::MaximizeWindow(_window_id) => {
                },
                CxOsOp::RestoreWindow(_window_id) => {
                },
                CxOsOp::FullscreenWindow(_window_id) => {
                    self.os.from_wasm(FromWasmFullScreen {});
                },
                CxOsOp::NormalizeWindow(_window_id) => {
                    self.os.from_wasm(FromWasmNormalScreen {});
                }
                CxOsOp::SetTopmost(_window_id, _is_topmost) => {
                    todo!()
                }
                CxOsOp::XrStartPresenting => {
                    self.os.from_wasm(FromWasmXrStartPresenting {});
                },
                CxOsOp::XrStopPresenting => {
                    self.os.from_wasm(FromWasmXrStopPresenting {});
                },
                CxOsOp::ShowTextIME(area, pos) => {
                    let pos = area.clipped_rect(self).pos + pos;
                    self.os.from_wasm(FromWasmShowTextIME {x: pos.x, y: pos.y});
                },
                CxOsOp::HideTextIME => {
                    self.os.from_wasm(FromWasmHideTextIME {});
                },
                CxOsOp::ShowClipboardActions(_) =>{
                }
                CxOsOp::SetCursor(cursor) => {
                    self.os.from_wasm(FromWasmSetMouseCursor::new(cursor));
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    self.os.from_wasm(FromWasmStartTimer {
                        repeats,
                        interval,
                        timer_id: timer_id as f64,
                    });
                },
                CxOsOp::StopTimer(timer_id) => {
                    self.os.from_wasm(FromWasmStopTimer {
                        id: timer_id as f64,
                    });
                },
                CxOsOp::StartDragging(_dragged_item) => {
                }
                CxOsOp::UpdateMacosMenu(_menu) => {
                },
                CxOsOp::HttpRequest{request_id, request} => {
                    let headers = request.get_headers_string();
                    self.os.from_wasm(FromWasmHTTPRequest {
                        request_id_lo: request_id.lo(),
                        request_id_hi: request_id.hi(),
                        metadata_id_lo: request.metadata_id.lo(),
                        metadata_id_hi: request.metadata_id.hi(),
                        url: request.url,
                        method: request.method.to_string().into(),
                        headers: headers,
                        body: WasmDataU8::from_vec_u8(request.body.unwrap_or(Vec::new())),
                    });
                },
                /*
                CxOsOp::WebSocketOpen{request_id, request}=>{
                    let headers = request.get_headers_string();
                    self.os.from_wasm(FromWasmWebSocketOpen {
                        url: request.url,
                        method: request.method.to_string().into(),
                        headers: headers,
                        body: WasmDataU8::from_vec_u8(request.body.unwrap_or(Vec::new())),
                        request_id_lo: request_id.lo(),
                        request_id_hi: request_id.hi(),
                    });
                }
                CxOsOp::WebSocketSendBinary{request_id, data}=>{
                    self.os.from_wasm(FromWasmWebSocketSendBinary {
                        request_id_lo: request_id.lo(),
                        request_id_hi: request_id.hi(),
                        data: WasmDataU8::from_vec_u8(data)
                    });
                }
                CxOsOp::WebSocketSendString{request_id, data}=>{
                    self.os.from_wasm(FromWasmWebSocketSendString {
                        request_id_lo: request_id.lo(),
                        request_id_hi: request_id.hi(),
                        data
                    });
                },*/
                CxOsOp::PrepareVideoPlayback(_, _, _, _, _) => todo!(),
                CxOsOp::BeginVideoPlayback(_) => todo!(),
                CxOsOp::PauseVideoPlayback(_) => todo!(),
                CxOsOp::ResumeVideoPlayback(_) => todo!(),
                CxOsOp::MuteVideoPlayback(_) => todo!(),
                CxOsOp::UnmuteVideoPlayback(_) => todo!(),
                CxOsOp::CleanupVideoPlaybackResources(_) => todo!(),
                CxOsOp::UpdateVideoSurfaceTexture(_) => todo!(),
                CxOsOp::SaveFileDialog(_) => todo!(),
                CxOsOp::SelectFileDialog(_) => todo!(),
                CxOsOp::SaveFolderDialog(_) => todo!(),
                CxOsOp::SelectFolderDialog(_) => todo!(),    
            }
        }
    }
}


impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.live_expand();
        self.live_scan_dependencies();
        
        self.os.append_to_wasm_js(&[
            ToWasmGetDeps::to_js_code(),
            ToWasmInit::to_js_code(),
            ToWasmResizeWindow::to_js_code(),
            ToWasmAnimationFrame::to_js_code(),
            
            ToWasmTouchUpdate::to_js_code(),
            ToWasmMouseDown::to_js_code(),
            ToWasmMouseMove::to_js_code(),
            ToWasmMouseUp::to_js_code(),
            ToWasmScroll::to_js_code(),
            
            ToWasmKeyDown::to_js_code(),
            ToWasmKeyUp::to_js_code(),
            ToWasmTextInput::to_js_code(),
            ToWasmTextCopy::to_js_code(),
            ToWasmTimerFired::to_js_code(),
            ToWasmPaintDirty::to_js_code(),
            ToWasmRedrawAll::to_js_code(),
            ToWasmXRUpdate::to_js_code(),
            ToWasmAppGotFocus::to_js_code(),
            ToWasmAppLostFocus::to_js_code(),
            ToWasmHTTPResponse::to_js_code(),
            ToWasmHttpRequestError::to_js_code(),
            ToWasmHttpResponseProgress::to_js_code(),
            ToWasmHttpUploadProgress::to_js_code(),
            /*ToWasmWebSocketOpen::to_js_code(),
            ToWasmWebSocketClose::to_js_code(),
            ToWasmWebSocketError::to_js_code(),
            ToWasmWebSocketString::to_js_code(),
            ToWasmWebSocketBinary::to_js_code(),*/
            ToWasmSignal::to_js_code(),
            ToWasmMidiInputData::to_js_code(),
            ToWasmMidiPortList::to_js_code(),
            ToWasmAudioDeviceList::to_js_code(),
            ToWasmLiveFileChange::to_js_code()
        ]);
        
        self.os.append_from_wasm_js(&[
            FromWasmLoadDeps::to_js_code(),
            FromWasmStartTimer::to_js_code(),
            FromWasmStopTimer::to_js_code(),
            FromWasmFullScreen::to_js_code(),
            FromWasmNormalScreen::to_js_code(),
            FromWasmRequestAnimationFrame::to_js_code(),
            FromWasmSetDocumentTitle::to_js_code(),
            FromWasmSetMouseCursor::to_js_code(),
            FromWasmTextCopyResponse::to_js_code(),
            FromWasmShowTextIME::to_js_code(),
            FromWasmHideTextIME::to_js_code(),
            FromWasmCreateThread::to_js_code(),
            FromWasmHTTPRequest::to_js_code(),
            /*FromWasmWebSocketOpen::to_js_code(),
            FromWasmWebSocketSendString::to_js_code(),
            FromWasmWebSocketSendBinary::to_js_code(),*/
            FromWasmXrStartPresenting::to_js_code(),
            FromWasmXrStopPresenting::to_js_code(),
            
            FromWasmCompileWebGLShader::to_js_code(),
            FromWasmAllocArrayBuffer::to_js_code(),
            FromWasmAllocIndexBuffer::to_js_code(),
            FromWasmAllocVao::to_js_code(),
            FromWasmAllocTextureImage2D::to_js_code(),
            FromWasmBeginRenderTexture::to_js_code(),
            FromWasmBeginRenderCanvas::to_js_code(),
            FromWasmSetDefaultDepthAndBlendMode::to_js_code(),
            FromWasmDrawCall::to_js_code(),
            
            FromWasmUseMidiInputs::to_js_code(),
            FromWasmSendMidiOutput::to_js_code(),
            FromWasmQueryAudioDevices::to_js_code(),
            FromWasmStartAudioOutput::to_js_code(),
            FromWasmStopAudioOutput::to_js_code(),
            FromWasmQueryMidiPorts::to_js_code()
        ]);
    }
    
    fn seconds_since_app_start(&self)->f64{
        0.0
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        let closure_box: Box<dyn FnOnce() + Send + 'static> = Box::new(f);
        let context_ptr = Box::into_raw(Box::new(closure_box));
        self.os.from_wasm(FromWasmCreateThread {context_ptr: context_ptr as u32});
    }
    
    /*
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
    }*/
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
pub struct CxOs {
    pub (crate) window_geom: WindowGeom,
    
    pub (crate) from_wasm: Option<FromWasmMsg>,
    
    pub (crate) vertex_buffers: usize,
    pub (crate) index_buffers: usize,
    pub (crate) vaos: usize,
    
    pub (crate) xr_last_inputs: Option<Vec<XRInput >>,
    
    pub (crate) to_wasm_js: Vec<String>,
    pub (crate) from_wasm_js: Vec<String>,
    
    pub (crate) media: CxWebMedia,
}

impl CxOs {
    
    pub fn append_to_wasm_js(&mut self, strs: &[String]) {
        self.to_wasm_js.extend_from_slice(strs);
    }
    
    pub fn append_from_wasm_js(&mut self, strs: &[String]) {
        self.from_wasm_js.extend_from_slice(strs);
    }
    
    pub fn from_wasm(&mut self, from_wasm: impl FromWasm) {
        self.from_wasm.as_mut().unwrap().from_wasm(from_wasm);
    }
}

#[export_name = "wasm_get_js_message_bridge"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_get_js_message_bridge(cx_ptr: u32) -> u32 {
    let cx = &mut *(cx_ptr as *mut Cx);
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
    
    out.push_str("return {\n");
    out.push_str("ToWasmMsg:class extends ToWasmMsg{\n");
    for to_wasm in &cx.os.to_wasm_js {
        out.push_str(to_wasm);
    }
    out.push_str("},\n");
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    for from_wasm in &cx.os.from_wasm_js {
        out.push_str(from_wasm);
    }
    out.push_str("}\n");
    out.push_str("}");
    msg.push_str(&out);
    msg.release_ownership()
}

#[export_name = "wasm_check_signal"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_check_signal() -> u32 {
    if SignalToUI::check_and_clear_ui_signal(){
        1
    }
    else{
        0
    }
}

#[export_name = "wasm_init_panic_hook"]
pub unsafe extern "C" fn init_panic_hook() {
    pub fn panic_hook(info: &panic::PanicInfo) {
        crate::error!("{}", info)
    }
    panic::set_hook(Box::new(panic_hook));
}

#[no_mangle]
pub static mut BASE_ADDR: usize = 10;
