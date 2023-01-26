use {
    self::super::{
        kms_event::*,
        egl_drm::*,
    },
    self::super::super::{
        gl_sys,
        libc_sys,
        select_timer::SelectTimers,
        linux_media::CxLinuxMedia
    },
    crate::{
        cx_api::{CxOsOp, CxOsApi},
        makepad_live_id::*,
        makepad_math::*,
        thread::Signal,
        event::{
            TimerEvent,
            WebSocket,
            WebSocketAutoReconnect,
            Event,
            WindowGeom,
        },
        pass::CxPassParent,
        cx::{Cx, OsType,},
        gpu_info::GpuPerformance,
        os::cx_desktop::EventFlow,
        pass::{PassClearColor, PassClearDepth, PassId},
    }
};


pub struct KmsApp {
    timers: SelectTimers,
    drm: Drm,
    egl: Egl,
}

impl KmsApp {
    fn new() -> Self {
        // ok so. lets do some drm devices things
        let mut drm =  unsafe {Drm::new("1920x1080-60")}.unwrap();
        let egl = unsafe {Egl::new(&drm)}.unwrap();
        egl.swap_buffers();
        unsafe{drm.first_mode()};
        Self {
            egl,
            drm,
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
        kms_app.timers.start_timer(0,0.008,true);
        // lets run the kms eventloop
        let mut event_flow = EventFlow::Poll;
        let mut timer_ids = Vec::new();
        let mut signal_fds = [0, 0];
        unsafe {libc_sys::pipe(signal_fds.as_mut_ptr());}
         
        while event_flow != EventFlow::Exit { 
            if event_flow == EventFlow::Wait {
            //    kms_app.timers.select(signal_fds[0]);
            } 
            kms_app.timers.update_timers(&mut timer_ids);
            for timer_id in &timer_ids {
                self.kms_event_callback(
                    &mut kms_app,
                    KmsEvent::Timer(TimerEvent {timer_id: *timer_id})
                );
            }
            event_flow = self.kms_event_callback(&mut kms_app, KmsEvent::Paint);
            // alright so.. how do we do things
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
                    kms_app.egl.make_current();
                    self.opengl_compile_shaders();
                }
                // ok here we send out to all our childprocesses
                self.handle_repaint(kms_app);
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
    
    pub fn draw_pass_to_fullscreen(
        &mut self,
        pass_id: PassId,
        kms_app:&mut KmsApp,
        dpi_factor: f64,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_render_pass(pass_id, dpi_factor);
        
        self.passes[pass_id].paint_dirty = false;

        unsafe {
            kms_app.egl.make_current();
            gl_sys::Viewport(0, 0, kms_app.drm.width as i32, kms_app.drm.height as i32);
        }
        
        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            self.passes[pass_id].clear_color
        }
        else { 
            match self.passes[pass_id].color_textures[0].clear_color {
                PassClearColor::InitWith(color) => color,
                PassClearColor::ClearWith(color) => color
            }
        };
        let clear_depth = match self.passes[pass_id].clear_depth {
            PassClearDepth::InitWith(depth) => depth,
            PassClearDepth::ClearWith(depth) => depth
        };
        
        if !self.passes[pass_id].dont_clear {
            unsafe {
                gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, 0);
                gl_sys::ClearDepth(clear_depth as f64);
                gl_sys::ClearColor(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                gl_sys::Clear(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
            }
        }
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
        );
        
        unsafe {
            kms_app.drm.flip_buffers_and_wait(&kms_app.egl);
        }
    }
    
    pub (crate) fn handle_repaint(&mut self, kms_app:&mut KmsApp) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    self.draw_pass_to_fullscreen(*pass_id, kms_app, 1.0);
                    // lets run a paint pass here
                    
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
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let size =  dvec2(kms_app.drm.width as f64,kms_app.drm.height as f64);
                    window.window_geom = WindowGeom{
                        dpi_factor: 1.0,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: true, 
                        is_topmost: true,
                        position: dvec2(0.0,0.0),
                        inner_size: size,
                        outer_size: size
                    };
                    window.is_created = true;
                },
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

