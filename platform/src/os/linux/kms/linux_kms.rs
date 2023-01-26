use {
    std::ffi::CStr,
    self::super::{
        kms_event::*,
        drm_sys::*,
    },
    self::super::super::{
        libc_sys,
        select_timer::SelectTimers,
        linux_media::CxLinuxMedia
    },
    crate::{
        cx_api::{CxOsOp, CxOsApi},
        makepad_live_id::*,
        thread::Signal,
        event::{
            TimerEvent,
            WebSocket,
            WebSocketAutoReconnect,
            Event,
        },
        pass::CxPassParent,
        cx::{Cx, OsType,},
        gpu_info::GpuPerformance,
        os::cx_desktop::EventFlow,
        
    }
};

pub struct KmsApp {
    pub timers: SelectTimers,
} 

impl KmsApp {
    fn new() -> Self {
        // ok so. lets do some drm devices things
        struct Drm{
            drm_fd: std::os::raw::c_int,
            mode:drmModeModeInfoPtr,
            resources:drmModeResPtr,
            connector:drmModeConnectorPtr,
            encoder:drmModeEncoderPtr,
        }
        
        impl Drm{
            unsafe fn new(mode_want:&str)->Option<Self>{
                let mut devices:[drmDevicePtr;MAX_DRM_DEVICES] = std::mem::zeroed();
                let num_devices = drmGetDevices2(0, devices.as_mut_ptr(), MAX_DRM_DEVICES as _) as usize;
                
                let mut found_connector = None;
                'outer: for i in 0..num_devices{
                    let device = *devices[i];
                    if device.available_nodes & (1<<DRM_NODE_PRIMARY) == 0{
                        continue;
                    }
                    // alright lets get the resources
                    let drm_fd = libc_sys::open(*device.nodes.offset(DRM_NODE_PRIMARY as _), libc_sys::O_RDWR);
                    let resources = drmModeGetResources(drm_fd);
                    if resources == std::ptr::null_mut(){
                        continue;
                    }
                    for j in 0..(*resources).count_connectors{
                        let connector_idx = *(*resources).connectors.offset(j as _);
                        let connector = drmModeGetConnector(drm_fd, connector_idx);
                        if connector == std::ptr::null_mut(){
                            continue;
                        }
                        if (*connector).connection == DRM_MODE_CONNECTED {
                            found_connector = Some((drm_fd,resources,connector));
                            break 'outer;
                        }
                        drmModeFreeConnector(connector);
                    }
                    drmModeFreeResources(resources);
                }
                if found_connector.is_none(){
                    return None
                }
                let (drm_fd,resources,connector) = found_connector.unwrap();
                // find a mode
                let mut found_mode = None;
                for i in 0..(*connector).count_modes{
                    let mode_info = (*connector).modes.offset(i as _);
                    let name = CStr::from_ptr((*mode_info).name.as_ptr()).to_str().unwrap();
                    let mode_name= format!("{}-{}", name, (*mode_info).vrefresh);
                    println!("{}", mode_name);
                    if mode_name == mode_want{
                        found_mode = Some(mode_info);
                    }
                }
                if found_mode.is_none(){
                    drmModeFreeConnector(connector);
                    drmModeFreeResources(resources);
                    return None
                }
                let mut found_encoder = None;
                for i in 0..(*resources).count_encoders{
                    let encoder = drmModeGetEncoder(drm_fd, *(*resources).encoders.offset(i as _));
                    if (*encoder).encoder_id == (*connector).encoder_id{
                        found_encoder = Some(encoder);
                        break;
                    }
                    drmModeFreeEncoder(encoder);
                }
                if found_encoder.is_none(){
                    drmModeFreeConnector(connector);
                    drmModeFreeResources(resources);
                    return None
                }
                Some(Drm{
                    encoder:found_encoder.unwrap(),
                    mode: found_mode.unwrap(),
                    drm_fd,
                    resources,
                    connector
                })
            }
        }
            
        if let Some(_drm) = unsafe{Drm::new("1920x1080-60")}{
            println!("HERE");
        }
        Self {
            timers: SelectTimers::new()
        }
    }
}

impl Cx {
    pub fn event_loop(mut self) {
        self.platform_type = OsType::Linux {custom_window_chrome: false};
        self.gpu_info.performance = GpuPerformance::Tier1;
        
        self.call_event_handler(&Event::Construct);
        self.redraw_all();
        let mut kms_app = KmsApp::new();
        
        // lets run the kms eventloop
        let mut event_flow = EventFlow::Poll;
        let mut timer_ids = Vec::new();
        let mut signal_fds = [0, 0];
        unsafe {libc_sys::pipe(signal_fds.as_mut_ptr());}
        
        while event_flow != EventFlow::Exit {
            if event_flow == EventFlow::Wait {
                kms_app.timers.select(signal_fds[0]);
            }
            kms_app.timers.update_timers(&mut timer_ids);
            for timer_id in &timer_ids {
                self.kms_event_callback(
                    &mut kms_app,
                    KmsEvent::Timer(TimerEvent {timer_id: *timer_id})
                );
            }
            event_flow = self.kms_event_callback(&mut kms_app, KmsEvent::Paint);
        }
    }
    
    fn kms_event_callback(
        &mut self,
        kms_app: &mut KmsApp,
        event: KmsEvent,
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(kms_app) {
            return EventFlow::Exit
        }
        
        //self.process_desktop_pre_event(&mut event);
        match event {
            KmsEvent::Paint => {
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(kms_app.timers.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    //opengl_cx.make_current();
                    self.opengl_compile_shaders();
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint();
            }
            KmsEvent::MouseDown(e) => {
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            KmsEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.move_captures();
            }
            KmsEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
            }
            KmsEvent::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            KmsEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            KmsEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            KmsEvent::Timer(e) => {
                if e.timer_id == 0 {
                    if Signal::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                }
                else {
                    self.call_event_handler(&Event::Timer(e))
                }
            }
        }
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
    }
    
    pub (crate) fn handle_repaint(&mut self) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    /*if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers(&opengl_cx);
                        self.draw_pass_to_window(*pass_id, dpi_factor, window, opengl_cx);
                    }*/
                    // DO HERE
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
    
    fn handle_platform_ops(&mut self, kms_app: &mut KmsApp) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::SetCursor(_cursor) => {
                    //xlib_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    kms_app.timers.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    kms_app.timers.stop_timer(timer_id);
                },
                _ => ()
            }
        }
        EventFlow::Poll
    }
}

impl CxOsApi for Cx {
    fn init(&mut self) {
        self.live_expand();
        self.live_scan_dependencies();
        self.desktop_load_dependencies();
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
}

#[derive(Default)]
pub struct CxOs {
    pub (crate) media: CxLinuxMedia,
}

