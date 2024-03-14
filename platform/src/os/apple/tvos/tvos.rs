use {
    std::{
        time::Instant,
        rc::Rc,
        cell::{RefCell},
        io::prelude::*,
        fs::File,
    }, 
 
    crate::{ 
        //makepad_live_id::*,
        os::{
            apple::apple_sys::*,
            apple::apple_util::nsstring_to_string,
            cx_native::EventFlow,
            apple::{
                tvos::{
                    tvos_event::TvosEvent,
                    tvos_app::{TvosApp, init_tvos_app_global,get_tvos_app_global}
                },
                url_session::{make_http_request},
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
        cx_api::{CxOsApi, CxOsOp},
        cx::{Cx, OsType},
    }
};
#[cfg(not(apple_sim))]
use crate::makepad_live_compiler::LiveFileChange;
#[cfg(not(apple_sim))]
use crate::event::{NetworkResponse, HttpRequest, HttpMethod};
#[cfg(not(apple_sim))]
use crate::makepad_live_id::*;

impl Cx {
    
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
        while let Ok(event) = self.os.network_response.receiver.try_recv(){
            out.push(event);
        }
        if out.len()>0{
            let mut e = Event::NetworkResponses(out);
            if self.studio_http_connection(&mut e){
                self.call_event_handler(&e)
            }
        }
    }
    
    #[allow(dead_code)]
    pub (crate) fn tvos_load_dependencies(&mut self){
        
        let bundle_path = unsafe{
            let main:ObjcId = msg_send![class!(NSBundle), mainBundle];
            let path:ObjcId = msg_send![main, resourcePath];
            nsstring_to_string(path)
        };
        
        for (path,dep) in &mut self.dependencies{
            if let Ok(mut file_handle) = File::open(format!("{}/{}",bundle_path,path)) {
                let mut buffer = Vec::<u8>::new();
                if file_handle.read_to_end(&mut buffer).is_ok() {
                    dep.data = Some(Ok(Rc::new(buffer)));
                }
                else{
                    dep.data = Some(Err("read_to_end failed".to_string()));
                }
            }
            else{
                dep.data = Some(Err("File open failed".to_string()));
            }
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
                    //IosApp::show_keyboard();
                },
                CxOsOp::HideTextIME => {
                    //IosApp::hide_keyboard();
                },
                CxOsOp::SetCursor(_cursor) => { 
                },
                CxOsOp::StartTimer {timer_id:_, interval:_, repeats:_} => {
                },
                CxOsOp::StopTimer(_timer_id) => {
                },
                CxOsOp::StartDragging(_) => {
                }
                CxOsOp::UpdateMacosMenu(_menu) => {
                },
                CxOsOp::HttpRequest{request_id, request} => {
                    make_http_request(request_id, request, self.os.network_response.sender.clone());
                },
                CxOsOp::ShowClipboardActions(_request) => {
                    crate::log!("Show clipboard actions not supported yet");
                }
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

    #[cfg(apple_sim)]
    pub fn studio_http_connection(&mut self, _event: &mut Event) -> bool {
        true
    }
    
    #[cfg(not(apple_sim))]
    pub fn studio_http_connection(&mut self, event: &mut Event) -> bool {
        if let Event::NetworkResponses(res) = event {
            res.retain( | res | {
                if res.request_id == live_id!(live_reload) {
                    // alright lets see if we need to live reload from the body
                    if let NetworkResponse::HttpResponse(res) = &res.response {
                        // lets check our response
                        if let Some(body) = res.get_string_body() {
                            if body.len()>0 {
                                let mut parts = body.split("$$$makepad_live_change$$$");
                                if let Some(file_name) = parts.next() {
                                    let content = parts.next().unwrap().to_string();
                                    let _ = self.live_file_change_sender.send(vec![LiveFileChange{
                                        file_name:file_name.to_string(),
                                        content
                                    }]);
                                }
                            }
                        }
                        self.poll_studio_http();
                    }
                    false
                }
                else {
                    true
                }
            });
            if res.len()>0 {
                return true
            }
        }
        false
    }
    #[cfg(not(apple_sim))]
    fn poll_studio_http(&self) {
        let studio_http: Option<&'static str> = std::option_env!("MAKEPAD_STUDIO_HTTP");
        if studio_http.is_none() {
            return
        }
        let url = format!("http://{}/$live_file_change", studio_http.unwrap());
        let request = HttpRequest::new(url, HttpMethod::GET);
        make_http_request(live_id!(live_reload), request, self.os.network_response.sender.clone());
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
        
        #[cfg(not(apple_sim))]
        self.poll_studio_http();
        
        self.live_scan_dependencies();
        //#[cfg(target_feature="sim")]
        #[cfg(apple_sim)]
        self.native_load_dependencies();
        
        #[cfg(not(apple_sim))]
        self.tvos_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time.unwrap()).as_secs_f64()
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
}

