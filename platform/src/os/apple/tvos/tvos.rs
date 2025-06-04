use {
    std::{
        time::Instant,
        rc::Rc,
        cell::{RefCell},
    }, 
 
    crate::{ 
        //makepad_live_id::*,
        os::{
            apple::apple_sys::*,
            apple::apple_util::{str_to_nsstring},
            cx_native::EventFlow,
            apple::{
                tvos::{
                    tvos_event::TvosEvent,
                    tvos_app::{TvosApp, init_tvos_app_global,get_tvos_app_global}
                },
                url_session::{AppleHttpRequests},
            },
            apple_classes::init_apple_classes_global,
            apple_media::CxAppleMedia,
            metal::{MetalCx, DrawPassMode},
        },
        pass::{CxPassParent},
        thread::SignalToUI,
        window::CxWindowPool,
        event::{
            Event,
            NetworkResponseChannel
        },
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        cx::{Cx, OsType},
    }
};

impl Cx {
    
    pub fn trace(val:&str){
        unsafe{NSLog(str_to_nsstring(val))};
    }
    
    pub fn event_loop(cx:Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Ios;
        let metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        //let cx = Rc::new(RefCell::new(self));
        crate::log!("Makepad tvOS application started.");
        //let metal_windows = Rc::new(RefCell::new(Vec::new()));
        let device = metal_cx.borrow().device;
        init_apple_classes_global();
        init_tvos_app_global(device, Box::new({
            let cx = cx.clone();
            move | event | {
                let mut cx_ref = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let event_flow = cx_ref.tvos_event_callback(event, &mut metal_cx);
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
        
        TvosApp::event_loop();
    }
    
    pub (crate) fn handle_repaint(&mut self, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Xr => {}
                CxPassParent::Window(_window_id) => {
                    let mtk_view = get_tvos_app_global().mtk_view.unwrap();
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::MTKView(mtk_view));
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
        while let Ok(item) = self.os.network_response.receiver.try_recv(){
            self.os.http_requests.handle_response_item(&item);
            out.push(item);
        }
        if out.len()>0{
            self.call_event_handler(&Event::NetworkResponses(out))
        }
    }
    
    fn tvos_event_callback(
        &mut self,
        event:TvosEvent,
        metal_cx: &mut MetalCx,
    ) -> EventFlow {
        
        self.handle_platform_ops(metal_cx);
         
        // send a mouse up when dragging starts
        
        let mut paint_dirty = false;
        match &event {
           TvosEvent::Timer(te) => {
                if te.timer_id == 0 {
                   if SignalToUI::check_and_clear_ui_signal(){
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        self.handle_action_receiver();
                    }

                    if self.handle_live_edit(){
                        // self.draw_shaders.ptr_to_item.clear();
                        // self.draw_shaders.fingerprints.clear();
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();
                }
            }
            _ => ()
        }
        
        //self.process_desktop_pre_event(&mut event);
        match event {
           TvosEvent::Init=>{
                get_tvos_app_global().start_timer(0, 0.008, true);
                self.start_studio_websocket_delayed();
                self.call_event_handler(&Event::Startup);
                self.redraw_all();
            }
            TvosEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                paint_dirty = true;
                self.call_event_handler(&Event::AppGotFocus);
            }
            TvosEvent::AppLostFocus => {
                self.call_event_handler(&Event::AppLostFocus);
            } 
            TvosEvent::WindowGeomChange(re) => { // do this here because mac
                let window_id = CxWindowPool::id_zero();
                let window = &mut self.windows[window_id];
                window.window_geom = re.new_geom.clone();
                self.call_event_handler(&Event::WindowGeomChange(re));
                self.redraw_all();
            }
            TvosEvent::Paint => { 
                if self.new_next_frames.len() != 0 {
                    let time_now = get_tvos_app_global().time_now();
                    self.call_next_frame_event(time_now);
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    self.mtl_compile_shaders(&metal_cx);
                }
                // ok here we send out to all our childprocesses
                self.handle_repaint(metal_cx);
            }
            TvosEvent::Timer(e) => if e.timer_id != 0 {
                self.call_event_handler(&Event::Timer(e))
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
    }
    
    fn handle_platform_ops(&mut self, _metal_cx: &MetalCx){
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    window.window_geom = get_tvos_app_global().last_window_geom.clone();
                    window.is_created = true;
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    get_tvos_app_global().start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    get_tvos_app_global().stop_timer(timer_id);
                },
                CxOsOp::HttpRequest {request_id, request} => {
                    self.os.http_requests.make_http_request(request_id, request, self.os.network_response.sender.clone());
                },
                CxOsOp::CancelHttpRequest {request_id} => {
                    self.os.http_requests.cancel_http_request(request_id);
                },
                CxOsOp::ShowClipboardActions(_request) => {
                    crate::log!("Show clipboard actions not supported yet");
                }
                CxOsOp::CopyToClipboard(_request) => {
                    crate::error!("Clipboard actions not yet implemented for tvOS");
                }
                e=>{
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
    }

    
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) { 
        
        self.os.start_time = Some(Instant::now());
        #[cfg(not(apple_sim))]{
            self.live_registry.borrow_mut().package_root = Some("makepad".to_string());
        }
        
        self.live_expand();

        #[cfg(apple_sim)]
        self.start_disk_live_file_watcher(50);
        
        self.live_scan_dependencies();

        #[cfg(not(apple_sim))]
        self.apple_bundle_load_dependencies();
        #[cfg(apple_sim)]
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time.unwrap()).as_secs_f64()
    }    
    
    fn open_url(&mut self, _url:&str, _in_place:OpenUrlInPlace){
        crate::error!("open_url not implemented on this platform");
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
    pub (crate) start_time: Option<Instant>,
    pub (crate) media: CxAppleMedia,
    pub (crate) bytes_written: usize,
    pub (crate) draw_calls_done: usize,
    pub (crate) network_response: NetworkResponseChannel,
    pub (crate) http_requests: AppleHttpRequests,
}

