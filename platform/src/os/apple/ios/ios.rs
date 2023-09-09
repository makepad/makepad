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
                ios::{
                    ios_event::IosEvent,
                    ios_app::{IosApp, init_ios_app_global,get_ios_app_global}
                },
                ns_url_session::{make_http_request, web_socket_open},
            },
            apple_classes::init_apple_classes_global,
            apple_media::CxAppleMedia,
            apple_decoding::CxAppleDecoding,
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
        let metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        //let cx = Rc::new(RefCell::new(self));
        crate::log!("Hello world ! We booted up!");
        //let metal_windows = Rc::new(RefCell::new(Vec::new()));
        let device = metal_cx.borrow().device;
        init_apple_classes_global();
        init_ios_app_global(device, Box::new({
            let cx = cx.clone();
            move | ios_app,
            event | {
                let mut cx_ref = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let event_flow = cx_ref.ios_event_callback(ios_app, event, &mut metal_cx);
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
        
        get_ios_app_global().event_loop();
    }
    
    pub (crate) fn handle_repaint(&mut self, ios_app:&mut IosApp, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::MTKView(ios_app.mtk_view.unwrap()));
                }
                CxPassParent::Pass(_) => {
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
        ios_app: &mut IosApp,
        event: IosEvent,
        metal_cx: &mut MetalCx,
    ) -> EventFlow {
        
        self.handle_platform_ops(ios_app, metal_cx);
         
        // send a mouse up when dragging starts
        
        let mut paint_dirty = false;
        match &event {
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
            IosEvent::Init=>{
                get_ios_app_global().start_timer(0, 0.008, true);
                self.call_event_handler(&Event::Construct);
                self.redraw_all();
            }
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
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(ios_app.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    self.mtl_compile_shaders(&metal_cx);
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint(ios_app, metal_cx);
                
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
    
    fn handle_platform_ops(&mut self, ios_app:&mut IosApp, _metal_cx: &MetalCx){
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    window.window_geom = ios_app.last_window_geom.clone();
                    window.is_created = true;
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
                },
                CxOsOp::InitializeVideoDecoding(_, _, _) => todo!(),
                CxOsOp::DecodeNextVideoChunk(_, _) => todo!(),
                CxOsOp::FetchNextVideoFrames(_, _) => todo!(),
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
    pub (crate) media: CxAppleDecoding,
}

