use {
    std::rc::Rc,
    std::time::Instant,
    std::cell::RefCell,
    self::super::{
        oh_event::*,
        oh_media::CxOpenHarmonyMedia
    },
    self::super::super::{
        gl_sys,
        select_timer::SelectTimers,
    },
    crate::{
        window::CxWindowPool,
        cx_api::{CxOsOp, CxOsApi, OpenUrlInPlace},
        makepad_live_id::*,
        makepad_math::*,
        thread::SignalToUI,
        event::{
            TimerEvent,
            Event,
            WindowGeom,
        },
        //window::CxWindowPool,
        pass::CxPassParent,
        cx::{Cx, OsType, OpenHarmonyParams},
        gpu_info::GpuPerformance,
        os::cx_native::EventFlow,
        pass::{PassClearColor, PassClearDepth, PassId},
    }
};

pub struct OpenHarmonyApp {
    timers: SelectTimers,
    dpi_factor: f64,
    width: f64,
    height: f64,
    //add egl here etc
}

impl OpenHarmonyApp {
    fn new() -> Self {

        Self {
            dpi_factor: 2.0,
            width: 1000.0,
            height: 3000.0,
            timers: SelectTimers::new()
        }
    }
}

impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx >>) {
        
        let mut cx = cx.borrow_mut();
        
        cx.os_type = OsType::OpenHarmony(OpenHarmonyParams{
        });
        cx.gpu_info.performance = GpuPerformance::Tier1;
        
        cx.call_event_handler(&Event::Startup);
        cx.redraw_all();
        
        let mut app = OpenHarmonyApp::new();
        app.timers.start_timer(0, 0.008, true);
        // lets run the kms eventloop
        let mut event_flow = EventFlow::Poll;
        let mut timer_ids = Vec::new();
        
        while event_flow != EventFlow::Exit {
            if event_flow == EventFlow::Wait {
                //    kms_app.timers.select(signal_fds[0]);
            }
            app.timers.update_timers(&mut timer_ids);
            let time = app.timers.time_now();
            for timer_id in &timer_ids {
                cx.oh_event_callback(
                    &mut app,
                    OpenHarmonyEvent::Timer(TimerEvent {
                        timer_id: *timer_id,
                        time:Some(time)
                    })
                );
            }
            /*let input_events = direct_app.raw_input.poll_raw_input(
                direct_app.timers.time_now(),
                CxWindowPool::id_zero()
            );
            for event in input_events {
                cx.direct_event_callback(
                    &mut direct_app,
                    event
                );
            }*/
            
            event_flow = cx.oh_event_callback(&mut app, OpenHarmonyEvent::Paint);
        }
    }
    
    fn oh_event_callback(
        &mut self,
        app: &mut OpenHarmonyApp,
        event: OpenHarmonyEvent,
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(app) {
            return EventFlow::Exit
        }
        
        //self.process_desktop_pre_event(&mut event);
        match event {
            OpenHarmonyEvent::Paint => {
                //let p = profile_start();
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(app.timers.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    //direct_app.egl.make_current();
                    self.opengl_compile_shaders();
                }
                // ok here we send out to all our childprocesses
                //profile_end("paint event handling", p);
                //let p = profile_start();
                self.handle_repaint(app);
                //profile_end("paint openGL", p);
            }
            OpenHarmonyEvent::MouseDown(e) => {
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button, CxWindowPool::id_zero());
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            OpenHarmonyEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            OpenHarmonyEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            OpenHarmonyEvent::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            OpenHarmonyEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            OpenHarmonyEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            OpenHarmonyEvent::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e))
            }
            OpenHarmonyEvent::Timer(e) => {
                if e.timer_id == 0 {
                    if SignalToUI::check_and_clear_ui_signal() {
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
        app: &mut OpenHarmonyApp,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_render_pass(pass_id);
        
        // keep repainting in a loop
        //self.passes[pass_id].paint_dirty = false;
        
        unsafe {
            //direct_app.egl.make_current();
            gl_sys::Viewport(0, 0, app.width as i32, app.height as i32);
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
                gl_sys::ClearDepthf(clear_depth as f32);
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
        
        //unsafe {
            //direct_app.drm.swap_buffers_and_wait(&direct_app.egl);
        //}
    }
    
    pub (crate) fn handle_repaint(&mut self, app: &mut OpenHarmonyApp) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            self.passes[*pass_id].set_time(app.timers.time_now() as f32);
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    self.draw_pass_to_fullscreen(*pass_id, app);
                }
                CxPassParent::Pass(_) => {
                    self.draw_pass_to_magic_texture(*pass_id);
                },
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(*pass_id);
                }
            }
        }
    }
    
    fn handle_platform_ops(&mut self, app: &mut OpenHarmonyApp) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let size = dvec2(app.width as f64 / app.dpi_factor, app.height as f64 / app.dpi_factor);
                    window.window_geom = WindowGeom {
                        dpi_factor: app.dpi_factor,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: true,
                        is_topmost: true,
                        position: dvec2(0.0, 0.0),
                        inner_size: size,
                        outer_size: size
                    };
                    window.is_created = true;
                },
                CxOsOp::SetCursor(_cursor) => {
                    //xlib_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    app.timers.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    app.timers.stop_timer(timer_id);
                },
                _ => ()
            }
        }
        EventFlow::Poll
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.live_expand();
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn open_url(&mut self, _url:&str, _in_place:OpenUrlInPlace){
        crate::error!("open_url not implemented on this platform");
    }
    
    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time).as_secs_f64()
    }
}

pub struct CxOs {
    pub media: CxOpenHarmonyMedia,
    pub (crate) start_time: Instant,
}

impl Default for CxOs {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            media: Default::default()
        }
    }
}