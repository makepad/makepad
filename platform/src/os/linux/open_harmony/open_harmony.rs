use {
    self::super::{
        super::gl_sys, arkts_obj_ref::ArkTsObjRef, oh_callbacks::*, oh_media::CxOpenHarmonyMedia,
        raw_file::RawFileMgr,
    },
    crate::{
        cx::{Cx, OpenHarmonyParams, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        cx_stdin::{PollTimer, PollTimers},
        egl_sys::{self, LibEgl, EGL_NONE},
        event::{Event, KeyCode, KeyEvent, TouchUpdateEvent, VirtualKeyboardEvent, WindowGeom},
        gpu_info::GpuPerformance,
        makepad_math::*,
        os::cx_native::EventFlow,
        pass::{CxPassParent, PassClearColor, PassClearDepth, PassId},
        thread::SignalToUI,
        window::CxWindowPool,
        WindowGeomChangeEvent,
    },
    napi_derive_ohos::napi,
    napi_ohos::{sys::*, Env, JsObject, NapiRaw},
    std::{ffi::CString, os::raw::c_void, ptr::null_mut, rc::Rc, sync::mpsc, time::Instant},
};

#[napi(js_name = "onCreate")]
pub fn ohos_ability_on_create(env: Env, ark_ts: JsObject) -> napi_ohos::Result<()> {
    let raw_env = env.raw();
    let raw_ark = unsafe { ark_ts.raw() };
    let mut arkts_ref = std::ptr::null_mut();

    let status = unsafe { napi_create_reference(raw_env, raw_ark, 1, &mut arkts_ref) };
    assert!(status == 0);

    let arkts_obj = ArkTsObjRef::new(raw_env, arkts_ref);
    let device_type = arkts_obj
        .get_string("deviceType")
        .unwrap_or("phone".to_string());
    let os_full_name = arkts_obj
        .get_string("osFullName")
        .unwrap_or("OpenHarmony".to_string());
    let display_density = arkts_obj.get_number("displayDensity").unwrap_or(3.25);
    let files_dir = arkts_obj.get_string("filesDir").unwrap();
    let cache_dir = arkts_obj.get_string("cacheDir").unwrap();
    let temp_dir = arkts_obj.get_string("tempDir").unwrap();
    let res_mgr = arkts_obj.get_property("resMgr").unwrap();

    let raw_file = RawFileMgr::new(raw_env, res_mgr);

    crate::log!("call onCreate, device_type = {}, os_full_name = {}, display_density = {}, files_dir = {}, cache_dir = {}, temp_dir = {}", device_type, os_full_name, display_density, files_dir,cache_dir,temp_dir);

    send_from_ohos_message(FromOhosMessage::Init {
        device_type,
        os_full_name,
        display_density,
        files_dir,
        cache_dir,
        temp_dir,
        raw_env,
        arkts_ref,
        raw_file,
    });
    Ok(())
}

impl Cx {
    fn main_loop(&mut self, from_ohos_rx: mpsc::Receiver<FromOhosMessage>) {
        crate::log!("entry main_loop");

        self.gpu_info.performance = GpuPerformance::Tier1;

        self.call_event_handler(&Event::Startup);
        self.redraw_all();

        while !self.os.quit {
            match from_ohos_rx.recv() {
                Ok(FromOhosMessage::VSync) => {
                    self.handle_all_pending_messages(&from_ohos_rx);
                    self.handle_other_events();
                    self.handle_drawing();
                }
                Ok(message) => self.handle_message(message),
                Err(e) => {
                    crate::error!("Error receiving message: {:?}", e);
                }
            }
        }
    }

    fn handle_all_pending_messages(&mut self, from_ohos_rx: &mpsc::Receiver<FromOhosMessage>) {
        // Handle the messages that arrived during the last frame
        while let Ok(msg) = from_ohos_rx.try_recv() {
            self.handle_message(msg);
        }
    }

    fn handle_other_events(&mut self) {
        // Timers
        for event in self.os.timers.get_dispatch() {
            self.call_event_handler(&event);
        }

        // Signals
        if SignalToUI::check_and_clear_ui_signal() {
            self.handle_media_signals();
            self.call_event_handler(&Event::Signal);
        }

        // Video updates
        // let to_dispatch = self.get_video_updates();
        // for video_id in to_dispatch {
        //     let e = Event::VideoTextureUpdated(
        //         VideoTextureUpdatedEvent {
        //             video_id,
        //         }
        //     );
        //     self.call_event_handler(&e);
        // }

        // Live edits
        if self.handle_live_edit() {
            self.call_event_handler(&Event::LiveEdit);
            self.redraw_all();
        }

        // Platform operations
        self.handle_platform_ops();
    }

    fn handle_drawing(&mut self) {
        if self.any_passes_dirty() || self.need_redrawing() || !self.new_next_frames.is_empty() {
            if !self.new_next_frames.is_empty() {
                self.call_next_frame_event(self.os.timers.time_now());
            }
            if self.need_redrawing() {
                self.call_draw_event();
                self.opengl_compile_shaders();
            }

            if self.os.first_after_resize {
                self.os.first_after_resize = false;
                self.redraw_all();
            }

            self.handle_repaint();
        }
    }

    fn handle_message(&mut self, msg: FromOhosMessage) {
        match msg {
            FromOhosMessage::SurfaceCreated {
                window,
                width: _,
                height: _,
            } => unsafe {
                self.os.display.as_mut().unwrap().update_surface(window);
            },
            FromOhosMessage::SurfaceDestroyed => unsafe {
                self.os.display.as_mut().unwrap().destroy_surface();
            },
            FromOhosMessage::SurfaceChanged {
                window,
                width,
                height,
            } => {
                unsafe {
                    self.os.display.as_mut().unwrap().update_surface(window);
                }
                self.os.display_size = dvec2(width as f64, height as f64);
                let window_id = CxWindowPool::id_zero();
                let window = &mut self.windows[window_id];
                let old_geom = window.window_geom.clone();

                let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                let size = self.os.display_size / dpi_factor;
                window.window_geom = WindowGeom {
                    dpi_factor,
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
                    old_geom,
                }));
                if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
                    self.redraw_pass_and_child_passes(main_pass_id);
                }
                self.redraw_all();
                self.os.first_after_resize = true;
                self.call_event_handler(&Event::ClearAtlasses);
            }
            FromOhosMessage::Touch(mut touches) => {
                let time = touches[0].time;
                let window = &mut self.windows[CxWindowPool::id_zero()];
                let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                for touch in &mut touches {
                    // When the software keyboard shifted the UI in the vertical axis,
                    //we need to make the math here to keep touch events positions synchronized.
                    //if self.os.keyboard_visible {touch.abs.y += self.os.keyboard_panning_offset as f64};
                    //crate::log!("{} {:?} {} {}", time, touch.state, touch.uid, touch.abs);
                    touch.abs /= dpi_factor;
                }
                self.fingers.process_touch_update_start(time, &touches);
                let e = Event::TouchUpdate(
                    TouchUpdateEvent {
                        time,
                        window_id: CxWindowPool::id_zero(),
                        touches,
                        modifiers: Default::default()
                    }
                );
                self.call_event_handler(&e);
                let e = if let Event::TouchUpdate(e) = e {e}else {panic!()};
                self.fingers.process_touch_update_end(&e.touches);
            }
            FromOhosMessage::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e));
            }
            FromOhosMessage::DeleteLeft(length) => {
                for _ in 0..length {
                    let time = self.os.timers.time_now();
                    let e = KeyEvent {
                        key_code: KeyCode::Backspace,
                        is_repeat: false,
                        modifiers: Default::default(),
                        time,
                    };
                    self.keyboard.process_key_down(e.clone());
                    self.call_event_handler(&Event::KeyDown(e.clone()));
                    self.keyboard.process_key_up(e.clone());
                    self.call_event_handler(&Event::KeyUp(e));
                }
            }
            FromOhosMessage::ResizeTextIME(is_open, keyboard_height) => {
                let keyboard_height = (keyboard_height as f64) / self.os.dpi_factor;
                if is_open {
                    self.call_event_handler(&Event::VirtualKeyboard(
                        VirtualKeyboardEvent::DidShow {
                            height: keyboard_height,
                            time: self.os.timers.time_now(),
                        },
                    ))
                } else {
                    self.text_ime_was_dismissed();
                    self.call_event_handler(&Event::VirtualKeyboard(
                        VirtualKeyboardEvent::DidHide {
                            time: self.os.timers.time_now(),
                        },
                    ))
                }
            }
            _ => {}
        }
    }

    fn wait_init(&mut self, from_ohos_rx: &mpsc::Receiver<FromOhosMessage>) -> bool {
        if let Ok(FromOhosMessage::Init {
            device_type,
            os_full_name,
            display_density,
            files_dir,
            cache_dir,
            temp_dir,
            raw_env,
            arkts_ref,
            raw_file,
        }) = from_ohos_rx.recv()
        {
            self.os.dpi_factor = display_density;
            self.os.raw_file = Some(raw_file);
            self.os_type = OsType::OpenHarmony(OpenHarmonyParams {
                files_dir,
                cache_dir,
                temp_dir,
                device_type,
                os_full_name,
                display_density,
            });
            self.os.arkts_obj = Some(ArkTsObjRef::new(raw_env, arkts_ref));
            return true;
        } else {
            crate::error!("Cant' recv Init from arkts");
            return false;
        }
    }

    fn wait_surface_created(
        &mut self,
        from_ohos_rx: &mpsc::Receiver<FromOhosMessage>,
    ) -> *mut c_void {
        if let Ok(FromOhosMessage::SurfaceCreated {
            window,
            width,
            height,
        }) = from_ohos_rx.recv()
        {
            self.os.display_size = dvec2(width as f64, height as f64);
            crate::log!(
                "handle surface created, width={}, height={}, display_density={}",
                width,
                height,
                self.os.dpi_factor
            );
            return window;
        } else {
            crate::error!("Can't recv SurfaceCreated from arkts");
            return null_mut();
        }
    }

    pub fn ohos_init<F>(exports: JsObject, env: Env, startup: F)
    where
        F: FnOnce() -> Box<Cx> + Send + 'static,
    {
        crate::log!("ohos init");
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(move || {
            std::panic::set_hook(Box::new(|info| {
                crate::log!("custom panic hook: {}", info);
            }));
            Cx::ohos_startup(startup);
        });

        if let Ok(xcomponent) = exports.get_named_property::<JsObject>("__NATIVE_XCOMPONENT_OBJ__")
        {
            register_xcomponent_callbacks(&env, &xcomponent);
        } else {
            crate::log!("Failed to get xcomponent in ohos_init");
        }
    }

    fn ohos_startup<F>(startup: F)
    where
        F: FnOnce() -> Box<Cx> + Send + 'static,
    {
        crate::log!("ohos startup");
        let (from_ohos_tx, from_ohos_rx) = mpsc::channel();
        let ohos_tx = from_ohos_tx.clone();
        init_globals(ohos_tx);

        std::thread::spawn(move || {
            let mut cx = startup();
            assert!(cx.wait_init(&from_ohos_rx));
            cx.ohos_load_dependencies();

            let window = cx.wait_surface_created(&from_ohos_rx);

            let mut libegl = LibEgl::try_load().expect("can't load LibEGL");
            let (egl_context, egl_config, egl_display) = unsafe {
                egl_sys::create_egl_context(&mut libegl).expect("Can't create EGL context")
            };
            unsafe {
                gl_sys::load_with(|s| {
                    let s = CString::new(s).unwrap();
                    libegl.eglGetProcAddress.unwrap()(s.as_ptr())
                })
            };

            let win_attr = vec![EGL_NONE];
            let surface = unsafe {
                (libegl.eglCreateWindowSurface.unwrap())(
                    egl_display,
                    egl_config,
                    window as _,
                    win_attr.as_ptr() as _,
                )
            };

            if surface.is_null() {
                let err_code = unsafe { (libegl.eglGetError.unwrap())() };
                crate::log!("eglCreateWindowSurface error code:{}", err_code);
            }
            assert!(!surface.is_null());

            crate::log!("eglCreateWindowSurface success");
            unsafe {
                (libegl.eglSwapBuffers.unwrap())(egl_display, surface);
            }

            if unsafe {
                (libegl.eglMakeCurrent.unwrap())(egl_display, surface, surface, egl_context)
            } == 0
            {
                panic!();
            }

            cx.os.display = Some(CxOhosDisplay {
                libegl,
                egl_display,
                egl_config,
                egl_context,
                surface,
                window,
            });

            register_vsync_callback(from_ohos_tx);
            cx.main_loop(from_ohos_rx);
            //TODO, destroy surface
        });
    }

    pub fn ohos_load_dependencies(&mut self) {
        for (path, dep) in &mut self.dependencies {
            let mut buffer = Vec::<u8>::new();
            if let Ok(_) = self
                .os
                .raw_file
                .as_mut()
                .unwrap()
                .read_to_end(path, &mut buffer)
            {
                dep.data = Some(Ok(Rc::new(buffer)));
            } else {
                dep.data = Some(Err("read_to_end failed".to_string()));
            }
        }
    }

    pub fn draw_pass_to_fullscreen(&mut self, pass_id: PassId) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();

        self.setup_render_pass(pass_id);

        // keep repainting in a loop
        //self.passes[pass_id].paint_dirty = false;

        unsafe {
            //direct_app.egl.make_current();
            gl_sys::Viewport(
                0,
                0,
                self.os.display_size.x as i32,
                self.os.display_size.y as i32,
            );
        }

        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            self.passes[pass_id].clear_color
        } else {
            match self.passes[pass_id].color_textures[0].clear_color {
                PassClearColor::InitWith(color) => color,
                PassClearColor::ClearWith(color) => color,
            }
        };
        let clear_depth = match self.passes[pass_id].clear_depth {
            PassClearDepth::InitWith(depth) => depth,
            PassClearDepth::ClearWith(depth) => depth,
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

        self.render_view(pass_id, draw_list_id, &mut zbias, zbias_step);

        unsafe { self.os.display.as_mut().unwrap().swap_buffers() };

        //unsafe {
        //direct_app.drm.swap_buffers_and_wait(&direct_app.egl);
        //}
    }

    pub(crate) fn handle_repaint(&mut self) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            self.passes[*pass_id].set_time(self.os.timers.time_now() as f32);
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    self.draw_pass_to_fullscreen(*pass_id);
                }
                CxPassParent::Pass(_) => {
                    self.draw_pass_to_magic_texture(*pass_id);
                }
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(*pass_id);
                }
            }
        }
    }

    fn handle_platform_ops(&mut self) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            //crate::log!("============ handle_platform_ops");
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let size = dvec2(
                        self.os.display_size.x / self.os.dpi_factor,
                        self.os.display_size.y / self.os.dpi_factor,
                    );
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
                }
                CxOsOp::SetCursor(_cursor) => {
                    //xlib_app.set_mouse_cursor(cursor);
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    self.os
                        .timers
                        .timers
                        .insert(timer_id, PollTimer::new(interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    self.os.timers.timers.remove(&timer_id);
                }
                CxOsOp::ShowTextIME(_area, _pos) => {
                    let _ = self.os.arkts_obj.as_mut().unwrap().call_js_function(
                        "showKeyBoard",
                        0,
                        std::ptr::null_mut(),
                    );
                }
                CxOsOp::HideTextIME => {
                    let _ = self.os.arkts_obj.as_mut().unwrap().call_js_function(
                        "hideKeyBoard",
                        0,
                        std::ptr::null_mut(),
                    );
                    //self.os.keyboard_visible = false;
                    //unsafe {android_jni::to_java_show_keyboard(false);}
                }
                _ => (),
            }
        }
        EventFlow::Poll
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.live_registry.borrow_mut().package_root = Some("makepad".to_string());
        self.live_expand();
        self.live_scan_dependencies();
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

pub struct CxOhosDisplay {
    pub libegl: LibEgl,
    pub egl_display: egl_sys::EGLDisplay,
    pub egl_config: egl_sys::EGLConfig,
    pub egl_context: egl_sys::EGLContext,
    pub surface: egl_sys::EGLSurface,
    pub window: *mut c_void, //event_handler: Box<dyn EventHandler>,
}

pub struct CxOs {
    pub first_after_resize: bool,
    pub display_size: DVec2,
    pub dpi_factor: f64,
    pub media: CxOpenHarmonyMedia,
    pub quit: bool,
    pub timers: PollTimers,
    pub raw_file: Option<RawFileMgr>,
    pub arkts_obj: Option<ArkTsObjRef>,
    pub(crate) start_time: Instant,
    pub(crate) display: Option<CxOhosDisplay>,
}

impl Default for CxOs {
    fn default() -> Self {
        Self {
            first_after_resize: true,
            display_size: dvec2(1260 as f64, 2503 as f64),
            dpi_factor: 3.25,
            media: Default::default(),
            quit: false,
            timers: Default::default(),
            raw_file: None,
            arkts_obj: None,
            start_time: Instant::now(),
            display: None,
        }
    }
}

impl CxOhosDisplay {
    unsafe fn destroy_surface(&mut self) {
        (self.libegl.eglMakeCurrent.unwrap())(
            self.egl_display,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        (self.libegl.eglDestroySurface.unwrap())(self.egl_display, self.surface);
        self.surface = std::ptr::null_mut();
    }

    unsafe fn update_surface(&mut self, window: *mut c_void) {
        if !self.window.is_null() {
            //todo release window
        }
        self.window = window;
        if self.surface.is_null() == false {
            self.destroy_surface();
        }

        let win_attr = vec![EGL_NONE];
        self.surface = (self.libegl.eglCreateWindowSurface.unwrap())(
            self.egl_display,
            self.egl_config,
            self.window as _,
            win_attr.as_ptr() as _,
        );

        if self.surface.is_null() {
            let err_code = unsafe { (self.libegl.eglGetError.unwrap())() };
            crate::log!("eglCreateWindowSurface error code:{}", err_code);
        }

        assert!(!self.surface.is_null());

        self.make_current();
    }

    unsafe fn swap_buffers(&mut self) {
        (self.libegl.eglSwapBuffers.unwrap())(self.egl_display, self.surface);
    }

    unsafe fn make_current(&mut self) {
        if (self.libegl.eglMakeCurrent.unwrap())(
            self.egl_display,
            self.surface,
            self.surface,
            self.egl_context,
        ) == 0
        {
            panic!();
        }
    }
}
