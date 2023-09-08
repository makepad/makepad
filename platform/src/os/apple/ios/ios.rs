use {
    std::{
        rc::Rc,
        cell::{RefCell},
    },

    crate::{
        makepad_live_id::*,
        os::{
            cx_native::EventFlow,
            apple::{
                ios_event::IosEvent,
                ns_url_session::{make_http_request, web_socket_open},
            },
            apple_media::CxAppleMedia,
            metal::{MetalCx, DrawPassMode},
        },
        pass::{CxPassParent},
        thread::Signal,
        event::{
            Event,
            NetworkResponseChannel
        },
        cx_api::{CxOsApi, CxOsOp},
        cx::{Cx, OsType},
    }
};

const KEEP_ALIVE_COUNT: usize = 5;

impl Cx {
    
    pub fn event_loop(cx:Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Ios;
        let _metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        //let cx = Rc::new(RefCell::new(self));
        crate::log!("Hello world ! We booted up!");
        //let metal_windows = Rc::new(RefCell::new(Vec::new()));
        /*
        init_macos_app_global(Box::new({
            let cx = cx.clone();
            move | cocoa_app,
            event | {
                let mut cx_ref = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let mut metal_windows = metal_windows.borrow_mut();
                let event_flow = cx_ref.cocoa_event_callback(cocoa_app, event, &mut metal_cx, &mut metal_windows);
                let executor = cx_ref.executor.take().unwrap();
                drop(cx_ref);
                executor.run_until_stalled();
                let mut cx_ref = cx.borrow_mut();
                cx_ref.executor = Some(executor);
                event_flow
            }
        }));
        // lets set our signal poll timer
        
        // final bit of initflow
        get_macos_app_global().start_timer(0, 0.008, true);
        cx.borrow_mut().call_event_handler(&Event::Construct);
        cx.borrow_mut().redraw_all();
        get_macos_app_global().event_loop();*/
    }
    
    pub (crate) fn handle_repaint(&mut self, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    /*if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = metal_window.window_geom.dpi_factor;
                        metal_window.resize_core_animation_layer(&metal_cx);
                        let drawable: ObjcId = unsafe {msg_send![metal_window.ca_layer, nextDrawable]};
                        if drawable == nil {
                            return
                        }
                        if metal_window.is_resizing {
                            self.draw_pass(*pass_id, metal_cx, DrawPassMode::Resizing(drawable));
                        }
                        else {
                            self.draw_pass(*pass_id, metal_cx, DrawPassMode::Drawable(drawable));
                        }
                    }*/
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::Texture);
                },
                CxPassParent::None => {
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }

    pub(crate) fn handle_networking_events(&mut self) {
        let mut out = Vec::new();
        while let Ok(event) = self.os.network_response.receiver.try_recv(){
            out.push(event);
        }
        if out.len()>0{
            self.call_event_handler(&Event::NetworkResponses(out))
        }
    }
    
    fn ios_event_callback(
        &mut self,
        event: IosEvent,
        metal_cx: &mut MetalCx,
    ) -> EventFlow {
        
        self.handle_platform_ops(metal_cx);
         
        // send a mouse up when dragging starts
        
        let mut paint_dirty = false;
        match &event {
            IosEvent::MouseDown(_) |
            IosEvent::MouseMove(_) |
            IosEvent::MouseUp(_) |
            IosEvent::Scroll(_) |
            IosEvent::KeyDown(_) |
            IosEvent::KeyUp(_) |
            IosEvent::TextInput(_) => {
                self.os.keep_alive_counter = KEEP_ALIVE_COUNT;
            }
            IosEvent::Timer(te) => {
                if te.timer_id == 0 {
                    if self.os.keep_alive_counter>0 {
                        self.os.keep_alive_counter -= 1;
                        self.repaint_windows();
                    }

                    // check signals
                    if Signal::check_and_clear_ui_signal(){
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if self.was_live_edit(){
                        // self.draw_shaders.ptr_to_item.clear();
                        // self.draw_shaders.fingerprints.clear();
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();

                    return EventFlow::Poll;
                }
            }
            _ => ()
        }
        
        //self.process_desktop_pre_event(&mut event);
        match event {
            IosEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                
                paint_dirty = true;
                self.call_event_handler(&Event::AppGotFocus);
            }
            IosEvent::AppLostFocus => {
                self.call_event_handler(&Event::AppLostFocus);
            }
            

            IosEvent::WindowGeomChange(re) => { // do this here because mac
                
                self.call_event_handler(&Event::WindowGeomChange(re));
            }
           
            IosEvent::Paint => {
                /*if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(cocoa_app.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    self.mtl_compile_shaders(&metal_cx);
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint(metal_windows, metal_cx);*/
                
            }
            IosEvent::MouseDown(e) => {
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            IosEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            IosEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            IosEvent::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            IosEvent::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e))
            }
           
            IosEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            IosEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            IosEvent::TextCopy(e) => {
                self.call_event_handler(&Event::TextCopy(e))
            }
            IosEvent::TextCut(e) => {
                self.call_event_handler(&Event::TextCut(e))
            }
            IosEvent::Timer(e) => {
                self.call_event_handler(&Event::Timer(e))
            }
            IosEvent::MenuCommand(e) => {
                self.call_event_handler(&Event::MenuCommand(e))
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
    }
    
    fn handle_platform_ops(&mut self,  _metal_cx: &MetalCx){
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(_window_id) => {
                    /*let window = &mut self.windows[window_id];
                    let metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        cocoa_app,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;*/
                },
                CxOsOp::CloseWindow(_window_id) => {
                },
                CxOsOp::MinimizeWindow(_window_id) => {
                },
                CxOsOp::MaximizeWindow(_window_id) => {
                },
                CxOsOp::RestoreWindow(_window_id) => {
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
                CxOsOp::XrStartPresenting => {
                    //todo!()
                },
                CxOsOp::XrStopPresenting => {
                    //todo!()
                },
                CxOsOp::ShowTextIME(_area, _pos) => {
                },
                CxOsOp::HideTextIME => {
                },
                CxOsOp::SetCursor(_cursor) => { 
                },
                CxOsOp::StartTimer {timer_id:_, interval:_, repeats:_} => {
                },
                CxOsOp::StopTimer(_timer_id) => {
                },
                CxOsOp::StartDragging(_) => {
                }
                CxOsOp::UpdateMenu(_menu) => {
                },
                CxOsOp::HttpRequest{request_id, request} => {
                    make_http_request(request_id, request, self.os.network_response.sender.clone());
                },
                CxOsOp::ShowClipboardActions(_request) => {
                    crate::log!("Show clipboard actions not supported yet");
                }
                CxOsOp::WebSocketOpen{request_id, request}=>{
                    web_socket_open(request_id, request, self.os.network_response.sender.clone());
                }
                CxOsOp::WebSocketSendBinary{request_id:_, data:_}=>{
                    todo!()
                }
                CxOsOp::WebSocketSendString{request_id:_, data:_}=>{
                    todo!()
                }
            }
        }
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.live_expand();
        self.start_live_file_watcher();
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    /*
    fn web_socket_open(&mut self, _url: String, _rec: WebSocketAutoReconnect) -> WebSocket {
        todo!()
    }
    
    fn web_socket_send(&mut self, _websocket: WebSocket, _data: Vec<u8>) {
        todo!()
    }*/
}


#[derive(Default)]
pub struct CxOs {
    pub (crate) keep_alive_counter: usize,
    pub (crate) media: CxAppleMedia,
    pub (crate) bytes_written: usize,
    pub (crate) draw_calls_done: usize,
    pub (crate) network_response: NetworkResponseChannel,
}

