use {
    std::ffi::CString,
    std::os::raw::{c_char, c_void},
    std::time::Instant,
    self::super::{
        android_media::CxAndroidMedia,
        android_jni::{AndroidCallback, TouchAction, TouchPointer},
    },
    self::super::super::{
        gl_sys,
        //libc_sys,
    },
    crate::{
        cx_api::{CxOsOp, CxOsApi},
        makepad_error_log::*,
        makepad_math::*,
        thread::Signal,
        event::{
            WindowGeomChangeEvent,
            TimerEvent,
            WebSocket,
            WebSocketAutoReconnect,
            Event,
            WindowGeom,
        },
        window::CxWindowPool,
        pass::CxPassParent,
        cx::{Cx, OsType,},
        gpu_info::GpuPerformance,
        os::cx_native::EventFlow,
        pass::{PassClearColor, PassClearDepth, PassId},
    }
};

#[link(name = "EGL")]
extern "C" {
    fn eglGetProcAddress(procname: *const c_char) -> *mut c_void;
}

impl Cx {
    
    /// Called when EGL is initialized.
    pub fn android_init(&mut self, callback: AndroidCallback<'_>) {
        self.platform_type = OsType::Android;
        
        self.gpu_info.performance = GpuPerformance::Tier1;
        self.call_event_handler(&Event::Construct);
        self.redraw_all();
         
        unsafe {gl_sys::load_with( | s | {
            let s = CString::new(s).unwrap();
            eglGetProcAddress(s.as_ptr())
        })}; 
        
        callback.schedule_timeout(0, 0);
        callback.schedule_redraw();
        self.after_every_event(&callback);
    }
    
    /// Called when the MakepadSurface is resized.
    pub fn android_resize(&mut self, width: i32, height: i32, callback: AndroidCallback<'_>) {
        self.os.display_size = dvec2(width as f64, height as f64);
        let window_id = CxWindowPool::id_zero();
        let window = &mut self.windows[window_id];
        let old_geom = window.window_geom.clone();
        let size = self.os.display_size / self.os.dpi_factor;
        window.window_geom = WindowGeom {
            dpi_factor: self.os.dpi_factor,
            can_fullscreen: false,
            xr_is_presenting: false,
            is_fullscreen: true,
            is_topmost: true,
            position: dvec2(0.0, 0.0),
            inner_size: size,
            outer_size: size,
        }; 
        let new_geom = window.window_geom.clone();
        self.call_event_handler(&Event::WindowGeomChange(WindowGeomChangeEvent {
            window_id,
            new_geom,
            old_geom
        }));
        if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
            self.redraw_pass_and_child_passes(main_pass_id);
        }
        self.after_every_event(&callback);
    }
    
    /// Called when the MakepadSurface needs to be redrawn.
    pub fn android_draw(&mut self, callback: AndroidCallback<'_>) {
        
        if self.new_next_frames.len() != 0 {
            self.call_next_frame_event(self.os.time_now());
        }
        if self.need_redrawing() {
            self.call_draw_event();
            //android_app.egl.make_current();
            self.opengl_compile_shaders();
        }
        
        self.handle_repaint(&callback); 
        self.after_every_event(&callback);
    }
    
    /// Called when a touch event happened on the MakepadSurface.
    pub fn android_touch(&mut self, _action: TouchAction, _pointers: &[TouchPointer], callback: AndroidCallback<'_>) {
        /*nsafe {
            gl_sys::ClearColor(pointers[0].x / 1000.0, pointers[0].y / 2000.0, 0.0, 1.0);
        }*/
        //callback.schedule_redraw();
        self.after_every_event(&callback);
    }
      
    /// Called when a timeout expired.
    pub fn android_timeout(&mut self, timer_id: i64, callback: AndroidCallback<'_>) {
        if timer_id == 0 {
            if Signal::check_and_clear_ui_signal() {
                self.handle_media_signals();
                self.call_event_handler(&Event::Signal);
            }
            callback.schedule_timeout(0, 16);
        }
        else {
            self.call_event_handler(&Event::Timer(TimerEvent {timer_id: timer_id as u64}))
        }
        self.after_every_event(&callback); 
    }
    
    fn after_every_event(&mut self, callback: &AndroidCallback<'_>) {
        self.handle_platform_ops(&callback);
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 {
            callback.schedule_redraw();
        }
    }
    
    pub fn draw_pass_to_fullscreen(
        &mut self,
        pass_id: PassId,
        callback: &AndroidCallback<'_>,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_render_pass(pass_id, self.os.dpi_factor);
        
        // keep repainting in a loop
        self.passes[pass_id].paint_dirty = false;
        
        unsafe {
            gl_sys::Viewport(0, 0, self.os.display_size.x as i32, self.os.display_size.y as i32);
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
                //gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, 0);
                //gl_sys::ClearDepth(clear_depth as f64);
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
        
        callback.swap_buffers();
        //unsafe {
        //direct_app.drm.swap_buffers_and_wait(&direct_app.egl);
        //}
    }
    
    pub (crate) fn handle_repaint(&mut self, callback: &AndroidCallback<'_>) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    self.draw_pass_to_fullscreen(*pass_id, callback);
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
    
    fn handle_platform_ops(&mut self, callback: &AndroidCallback<'_>) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let size = self.os.display_size / self.os.dpi_factor;
                    window.window_geom = WindowGeom {
                        dpi_factor: self.os.dpi_factor,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: true,
                        is_topmost: true,
                        position: dvec2(0.0, 0.0),
                        inner_size: size,
                        outer_size: size,
                    };
                    window.is_created = true;
                },
                CxOsOp::SetCursor(_cursor) => {
                    //xlib_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats: _} => {
                    //android_app.start_timer(timer_id, interval, repeats);
                    callback.schedule_timeout(timer_id as i64, (interval / 1000.0) as i64);
                },
                CxOsOp::StopTimer(timer_id) => {
                    callback.cancel_timeout(timer_id as i64);
                    //android_app.stop_timer(timer_id);
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
        self.native_load_dependencies();
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

impl Default for CxOs {
    fn default() -> Self {
        Self {
            display_size: dvec2(100., 100.),
            dpi_factor: 1.5,
            time_start: Instant::now(),
            _media: CxAndroidMedia::default()
        }
    }
}

pub struct CxOs {
    pub display_size: DVec2,
    pub dpi_factor: f64,
    pub time_start: Instant,
    pub (crate) _media: CxAndroidMedia,
}

impl CxOs {
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
}
