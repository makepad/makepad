use {
    self::super::super::{gl_sys, linux_media::CxLinuxMedia, select_timer::SelectTimers},
    self::super::{
        direct_event::*,
        egl_drm::{Drm, Egl},
        raw_input::RawInput,
    },
    crate::{
        cx::{Cx, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
        event::{Event, TimerEvent, WindowGeom},
        gpu_info::GpuPerformance,
        makepad_live_id::*,
        makepad_math::*,
        os::cx_native::EventFlow,
        thread::SignalToUI,
        window::CxWindowPool,
    },
    std::cell::RefCell,
    std::rc::Rc,
    std::time::Instant,
};

pub struct DirectApp {
    timers: SelectTimers,
    drm: Drm,
    egl: Egl,
    raw_input: RawInput,
    dpi_factor: f64,
}

impl DirectApp {
    fn new() -> Self {
        let mut mode = "1280x720-60".to_string();
        let mut dpi_factor = 1.0;
        for arg in std::env::args() {
            if arg.starts_with("-mode=") {
                mode = arg.trim_start_matches("-mode=").to_string();
            }
            if arg.starts_with("-scale=") {
                dpi_factor = arg.trim_start_matches("-scale=").parse().unwrap();
            }
        }

        // ok so. lets do some drm devices things
        let mut drm = unsafe { Drm::new(&mode) }.unwrap();
        let egl = unsafe { Egl::new(&drm) }.unwrap();
        egl.swap_buffers();
        unsafe { drm.first_mode() };
        Self {
            dpi_factor,
            egl,
            raw_input: RawInput::new(
                drm.width as f64 / dpi_factor,
                drm.height as f64 / dpi_factor,
                dpi_factor,
            ),
            drm,
            timers: SelectTimers::new(),
        }
    }
}

impl Cx {
    pub(crate) fn handle_networking_events(&mut self) {
        self.dispatch_network_runtime_events();
    }

    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        let mut cx = cx.borrow_mut();

        cx.os_type = OsType::LinuxDirect;
        cx.gpu_info.performance = GpuPerformance::Tier1;

        cx.call_event_handler(&Event::Startup);
        cx.redraw_all();

        let mut direct_app = DirectApp::new();
        direct_app.timers.start_timer(0, 0.008, true);
        // lets run the kms eventloop
        let mut event_flow = EventFlow::Poll;
        let mut timer_ids = Vec::new();

        while event_flow != EventFlow::Exit {
            if event_flow == EventFlow::Wait {
                //    kms_app.timers.select(signal_fds[0]);
            }
            direct_app.timers.update_timers(&mut timer_ids);
            let time = direct_app.timers.time_now();
            for timer_id in &timer_ids {
                cx.direct_event_callback(
                    &mut direct_app,
                    DirectEvent::Timer(TimerEvent {
                        timer_id: *timer_id,
                        time: Some(time),
                    }),
                );
            }
            let input_events = direct_app
                .raw_input
                .poll_raw_input(direct_app.timers.time_now(), CxWindowPool::id_zero());
            for event in input_events {
                cx.direct_event_callback(&mut direct_app, event);
            }
            event_flow = cx.direct_event_callback(&mut direct_app, DirectEvent::Paint);
        }
    }

    fn direct_event_callback(
        &mut self,
        direct_app: &mut DirectApp,
        event: DirectEvent,
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(direct_app) {
            return EventFlow::Exit;
        }

        //self.process_desktop_pre_event(&mut event);
        match event {
            DirectEvent::Paint => {
                //let p = profile_start();
                let time_now = direct_app.timers.time_now();
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(time_now);
                }
                if self.need_redrawing() {
                    self.call_draw_event(time_now);
                    direct_app.egl.make_current();
                    self.opengl_compile_shaders();
                }
                // ok here we send out to all our childprocesses
                //profile_end("paint event handling", p);
                //let p = profile_start();
                self.handle_repaint(direct_app);
                //profile_end("paint openGL", p);
            }
            DirectEvent::MouseDown(e) => {
                self.fingers.process_tap_count(e.abs, e.time);
                self.fingers.mouse_down(e.button, CxWindowPool::id_zero());
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            DirectEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            DirectEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            DirectEvent::Scroll(e) => self.call_event_handler(&Event::Scroll(e.into())),
            DirectEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            DirectEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            DirectEvent::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
            DirectEvent::Timer(e) => {
                if e.timer_id == 0 {
                    if SignalToUI::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.handle_script_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        self.handle_action_receiver();
                    }
                    self.handle_networking_events();
                } else {
                    self.handle_script_timer(&e);
                    self.call_event_handler(&Event::Timer(e))
                }

                if self.handle_live_edit() {
                    self.call_event_handler(&Event::LiveEdit);
                    self.redraw_all();
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
        draw_pass_id: DrawPassId,
        direct_app: &mut DirectApp,
    ) {
        let draw_list_id = self.passes[draw_pass_id].main_draw_list_id.unwrap();

        self.setup_render_pass(draw_pass_id);

        // keep repainting in a loop
        //self.passes[draw_pass_id].paint_dirty = false;

        unsafe {
            direct_app.egl.make_current();
            (gl.glViewport)(
                0,
                0,
                direct_app.drm.width as i32,
                direct_app.drm.height as i32,
            );
        }

        let clear_color = if self.passes[draw_pass_id].color_textures.len() == 0 {
            self.passes[draw_pass_id].clear_color
        } else {
            match self.passes[draw_pass_id].color_textures[0].clear_color {
                DrawPassClearColor::InitWith(color) => color,
                DrawPassClearColor::ClearWith(color) => color,
            }
        };
        let clear_depth = match self.passes[draw_pass_id].clear_depth {
            DrawPassClearDepth::InitWith(depth) => depth,
            DrawPassClearDepth::ClearWith(depth) => depth,
        };

        if !self.passes[draw_pass_id].dont_clear {
            unsafe {
                (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
                (gl.glClearDepthf)(clear_depth as f32);
                (gl.glClearColor)(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                (gl.glClear)(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
            }
        }
        Self::set_default_depth_and_blend_mode();

        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;

        self.render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step);

        unsafe {
            direct_app.drm.swap_buffers_and_wait(&direct_app.egl);
        }
    }

    pub(crate) fn handle_repaint(&mut self, direct_app: &mut DirectApp) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for draw_pass_id in &passes_todo {
            self.passes[*draw_pass_id].set_time(direct_app.timers.time_now() as f32);
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(_window_id) => {
                    self.draw_pass_to_fullscreen(*draw_pass_id, direct_app);
                }
                CxDrawPassParent::DrawPass(_) => {
                    self.draw_pass_to_magic_texture(*draw_pass_id);
                }
                CxDrawPassParent::None => {
                    self.draw_pass_to_magic_texture(*draw_pass_id);
                }
            }
        }
    }

    fn handle_platform_ops(&mut self, direct_app: &mut DirectApp) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let size = dvec2(
                        direct_app.drm.width as f64 / direct_app.dpi_factor,
                        direct_app.drm.height as f64 / direct_app.dpi_factor,
                    );
                    window.window_geom = WindowGeom {
                        dpi_factor: direct_app.dpi_factor,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: true,
                        is_topmost: true,
                        position: dvec2(0.0, 0.0),
                        inner_size: size,
                        outer_size: size,
                    };
                    window.is_created = true;
                }
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let window = &mut self.windows[window_id];
                    window.window_geom = WindowGeom {
                        dpi_factor: direct_app.dpi_factor,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: false,
                        is_topmost: true,
                        position,
                        inner_size: size,
                        outer_size: size,
                    };
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;
                    window.is_created = true;
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    direct_app.timers.start_timer(timer_id, interval, repeats);
                }
                CxOsOp::StopTimer(timer_id) => {
                    direct_app.timers.stop_timer(timer_id);
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                CxOsOp::Quit => {
                    return EventFlow::Exit;
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    let window = &mut self.windows[window_id];
                    window.window_geom.inner_size = size;
                }
                CxOsOp::RepositionWindow(window_id, size) => {
                    let window = &mut self.windows[window_id];
                    window.window_geom.position = size;
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                } => {
                    self.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status: crate::permission::PermissionStatus::Granted,
                        },
                    ));
                }
                CxOsOp::RequestPermission {
                    permission,
                    request_id,
                } => {
                    self.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status: crate::permission::PermissionStatus::Granted,
                        },
                    ));
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        EventFlow::Poll
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.live_expand();
        if !Self::has_studio_web_socket() {
            self.start_disk_live_file_watcher(100);
        }
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(f);
    }

    fn open_url(&mut self, _url: &str, _in_place: OpenUrlInPlace) {
        crate::error!("open_url not implemented on this platform");
    }

    fn seconds_since_app_start(&self) -> f64 {
        Instant::now()
            .duration_since(self.os.start_time)
            .as_secs_f64()
    }
}

pub struct CxOs {
    pub(crate) media: CxLinuxMedia,
    pub(crate) start_time: Instant,
}

impl Default for CxOs {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            media: Default::default(),
        }
    }
}
