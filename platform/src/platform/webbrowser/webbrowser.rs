
use {
    crate::{
        makepad_live_id::*,
        makepad_math::Vec2,
        makepad_wasm_bridge::{console_log, FromWasmMsg, ToWasmMsg, FromWasm, ToWasm},
        platform::{
            webbrowser::{
                from_wasm::*,
                to_wasm::*,
            }
        },
        area::Area,
        event::{
            Timer,
            Signal,
            Event,
            XRInput,
            TextCopyEvent,
            TimerEvent,
            DraggedItem,
            WindowGeom,
            WindowGeomChangeEvent
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
        
        let mut is_animation_frame = false;
        while !to_wasm.was_last_block() {
            let block_id = LiveId(to_wasm.read_u64());
            let skip = to_wasm.read_block_skip();
            match block_id {
                id!(ToWasmGetDeps) => { // fetch_deps
                    let tw = ToWasmGetDeps::read_to_wasm(&mut to_wasm);
                    
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

                    self.call_event_handler(&mut Event::Construct);
                },
                
                id!(ToWasmResizeWindow) => {
                    let tw = ToWasmResizeWindow::read_to_wasm(&mut to_wasm);
                    let old_geom = self.platform.window_geom.clone();
                    let new_geom = tw.window_info.into();
                    if old_geom != new_geom {
                        self.platform.window_geom = new_geom.clone();
                        if self.windows.len()>0 {
                            self.windows[0].window_geom = new_geom.clone();
                        }
                        self.call_event_handler(&mut Event::WindowGeomChange(WindowGeomChangeEvent {
                            window_id: 0,
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
                
                id!(ToWasmFingerDown) => {
                    let tw = ToWasmFingerDown::read_to_wasm(&mut to_wasm);
                    
                    let tap_count = self.process_tap_count(
                        tw.finger.digit,
                        Vec2 {x: tw.finger.x, y: tw.finger.y},
                        tw.finger.time
                    );
                    
                    self.platform.fingers_down[tw.finger.digit] = true;
                    
                    self.call_event_handler(&mut Event::FingerDown(
                        tw.into_finger_down_event(tap_count)
                    ));
                }
                
                id!(ToWasmFingerUp) => {
                    let tw = ToWasmFingerUp::read_to_wasm(&mut to_wasm);
                    
                    let digit = tw.finger.digit;
                    self.platform.fingers_down[digit] = false;
                    
                    if !self.platform.fingers_down.iter().any( | down | *down) {
                        self.down_mouse_cursor = None;
                    }
                    
                    self.call_event_handler(&mut Event::FingerUp(tw.into()));
                    
                    self.fingers[digit].captured = Area::Empty;
                    self.down_mouse_cursor = None;
                }
                
                id!(ToWasmFingerMove) => {
                    let tw = ToWasmFingerMove::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&mut Event::FingerMove(tw.into()));
                }
                
                id!(ToWasmFingerHover) => {
                    let tw = ToWasmFingerHover::read_to_wasm(&mut to_wasm);
                    self.fingers[0].over_last = Area::Empty;
                    self.hover_mouse_cursor = None;
                    self.call_event_handler(&mut Event::FingerHover(tw.into()));
                    self.fingers[0]._over_last = self.fingers[0].over_last;
                }
                
                id!(ToWasmFingerOut) => {
                    // what was this for again?
                    let tw = ToWasmFingerOut::read_to_wasm(&mut to_wasm);
                    self.fingers[0].over_last = Area::Empty;
                    self.hover_mouse_cursor = None;
                    self.call_event_handler(&mut Event::FingerHover(tw.into()));
                    self.fingers[0]._over_last = self.fingers[0].over_last;
                }
                
                id!(ToWasmFingerScroll) => {
                    let tw = ToWasmFingerScroll::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&mut Event::FingerScroll(tw.into()));
                }
                
                id!(ToWasmKeyDown) => {
                    let tw = ToWasmKeyDown::read_to_wasm(&mut to_wasm);
                    self.process_key_down(tw.key.clone().into());
                    self.call_event_handler(&mut Event::KeyDown(tw.key.into()));
                }
                
                id!(ToWasmKeyUp) => {
                    let tw = ToWasmKeyUp::read_to_wasm(&mut to_wasm);
                    self.process_key_up(tw.key.clone().into());
                    self.call_event_handler(&mut Event::KeyUp(tw.key.into()));
                }
                
                id!(ToWasmTextInput) => {
                    let tw = ToWasmTextInput::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&mut Event::TextInput(tw.into()));
                }
                
                id!(ToWasmTextCopy) => {
                    let mut event = Event::TextCopy(TextCopyEvent {
                        response: None
                    });
                    self.call_event_handler(&mut event);
                    if let Event::TextCopy(TextCopyEvent {response: Some(response)}) = event {
                        self.platform.from_wasm(FromWasmTextCopyResponse {response});
                    }
                }
                
                id!(ToWasmTimerFired) => {
                    let tw = ToWasmTimerFired::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&mut Event::Timer(TimerEvent {
                        timer_id: tw.timer_id as u64
                    }));
                }
                
                id!(ToWasmAppGotFocus) => {
                     self.call_event_handler(&mut Event::AppGotFocus);
                }
                
                id!(ToWasmAppLostFocus) => {
                     self.call_event_handler(&mut Event::AppLostFocus);
                }
                
                id!(ToWasmXRUpdate) => {
                    let tw = ToWasmXRUpdate::read_to_wasm(&mut to_wasm);
                    let mut event = Event::XRUpdate(
                        tw.into_xrupdate_event(self.platform.xr_last_inputs.take())
                    );
                    self.call_event_handler(&mut event);
                    if let Event::XRUpdate(event) = event{
                        self.platform.xr_last_inputs = Some(event.inputs);
                    }
                }
                
                id!(ToWasmRedrawAll)=>{
                    self.redraw_all();
                }
                
                id!(ToWasmPaintDirty) => {
                    self.passes[self.windows[0].main_pass_id.unwrap()].paint_dirty = true;
                }
                
                _ => {
                    console_log!("Message unknown");
                    
                    //panic!("Message unknown")
                }
            };
            to_wasm.block_skip(skip);
        };
        
        self.call_signals_and_triggers();
        
        if self.need_redrawing() || self.new_next_frames.len() != 0 {
            self.call_draw_event();
            self.platform.from_wasm(FromWasmRequestAnimationFrame {});
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
        if let Some(cursor) = self.down_mouse_cursor {
            self.platform.from_wasm(
                FromWasmSetMouseCursor::new(cursor)
            )
        }
        else if let Some(cursor) = self.hover_mouse_cursor {
            self.platform.from_wasm(
                FromWasmSetMouseCursor::new(cursor)
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
        
        //return wasm pointer to caller
        self.platform.from_wasm.take().unwrap().release_ownership()
    }
    
    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where F: FnMut(&mut Cx, Event),
    {
    }
}


impl CxPlatformApi for Cx {
    
    fn show_text_ime(&mut self, x: f32, y: f32) {
        self.platform.from_wasm(FromWasmShowTextIME {x, y});
    }
    
    fn hide_text_ime(&mut self) {
        self.platform.from_wasm(FromWasmHideTextIME {});
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
        self.platform.from_wasm(FromWasmStartTimer {
            repeats,
            interval,
            timer_id: self.timer_id as f64,
        });
        Timer {timer_id: self.timer_id}
    }
    
    fn stop_timer(&mut self, timer: Timer) {
        if timer.timer_id != 0 {
            self.platform.from_wasm(FromWasmStopTimer {
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
    pub xr_last_inputs: Option<Vec<XRInput>>,
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
            xr_last_inputs: None,
        }
    }
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
    
    ToWasmGetDeps::to_wasm_js_method(&mut out);
    ToWasmInit::to_wasm_js_method(&mut out);
    ToWasmResizeWindow::to_wasm_js_method(&mut out);
    ToWasmAnimationFrame::to_wasm_js_method(&mut out);
    ToWasmFingerDown::to_wasm_js_method(&mut out);
    ToWasmFingerUp::to_wasm_js_method(&mut out);
    ToWasmFingerMove::to_wasm_js_method(&mut out);
    ToWasmFingerHover::to_wasm_js_method(&mut out);
    ToWasmFingerOut::to_wasm_js_method(&mut out);
    ToWasmFingerScroll::to_wasm_js_method(&mut out);
    ToWasmKeyDown::to_wasm_js_method(&mut out);
    ToWasmKeyUp::to_wasm_js_method(&mut out);
    ToWasmTextInput::to_wasm_js_method(&mut out);
    ToWasmTextCopy::to_wasm_js_method(&mut out);
    ToWasmTimerFired::to_wasm_js_method(&mut out);
    ToWasmPaintDirty::to_wasm_js_method(&mut out);
    ToWasmRedrawAll::to_wasm_js_method(&mut out);
    ToWasmAppGotFocus::to_wasm_js_method(&mut out);
    ToWasmAppLostFocus::to_wasm_js_method(&mut out);
    ToWasmXRUpdate::to_wasm_js_method(&mut out);
    
    out.push_str("},\n");
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    
    FromWasmLoadDeps::from_wasm_js_method(&mut out);
    FromWasmStartTimer::from_wasm_js_method(&mut out);
    FromWasmStopTimer::from_wasm_js_method(&mut out);
    FromWasmFullScreen::from_wasm_js_method(&mut out);
    FromWasmNormalScreen::from_wasm_js_method(&mut out);
    FromWasmRequestAnimationFrame::from_wasm_js_method(&mut out);
    FromWasmSetDocumentTitle::from_wasm_js_method(&mut out);
    FromWasmSetMouseCursor::from_wasm_js_method(&mut out);
    FromWasmTextCopyResponse::from_wasm_js_method(&mut out);
    FromWasmShowTextIME::from_wasm_js_method(&mut out);
    FromWasmHideTextIME::from_wasm_js_method(&mut out);

    FromWasmCompileWebGLShader::from_wasm_js_method(&mut out);
    FromWasmAllocArrayBuffer::from_wasm_js_method(&mut out);
    FromWasmAllocIndexBuffer::from_wasm_js_method(&mut out);
    FromWasmAllocVao::from_wasm_js_method(&mut out);
    FromWasmAllocTextureImage2D::from_wasm_js_method(&mut out);
    FromWasmBeginRenderTexture::from_wasm_js_method(&mut out);
    FromWasmBeginRenderCanvas::from_wasm_js_method(&mut out);
    FromWasmSetDefaultDepthAndBlendMode::from_wasm_js_method(&mut out);
    FromWasmDrawCall::from_wasm_js_method(&mut out);

    FromWasmXrStartPresenting::from_wasm_js_method(&mut out);
    FromWasmXrStopPresenting::from_wasm_js_method(&mut out);
   
    out.push_str("}\n");
    out.push_str("}");
    
    msg.push_str(&out);
    msg.release_ownership()
}
