use {
    std::rc::Rc,
    std::cell::{RefCell},
    std::ffi::CString,
    std::os::raw::{c_char, c_void},
    std::time::Instant,
    self::super::{
        android_media::CxAndroidMedia,
        jni_sys::jobject,
        android_jni::{AndroidToJava},
        android_util::*,
    },
    self::super::super::{
        gl_sys,
        //libc_sys,
    },
    core::cmp,
    crate::{
        cx_api::{CxOsOp, CxOsApi},
        makepad_math::*,
        thread::Signal,
        event::{
            TouchPoint,
            TouchUpdateEvent,
            WindowGeomChangeEvent,
            TimerEvent,
            TextInputEvent,
            TextCopyEvent,
            KeyEvent,
            KeyModifiers,
            WebSocket,
            WebSocketAutoReconnect,
            Event,
            WindowGeom,
        },
        window::CxWindowPool,
        pass::CxPassParent,
        cx::{Cx, OsType, AndroidInitParams},
        gpu_info::GpuPerformance,
        os::cx_native::EventFlow,
        pass::{PassClearColor, PassClearDepth, PassId},
    }
};

use num_traits::FromPrimitive;

// Defined in https://developer.android.com/reference/android/view/KeyEvent#META_CTRL_MASK
const ANDROID_META_CTRL_MASK: i32 = 28672;
// Defined in  https://developer.android.com/reference/android/view/KeyEvent#META_SHIFT_MASK
const ANDROID_META_SHIFT_MASK: i32 = 193;
// Defined in  https://developer.android.com/reference/android/view/KeyEvent#META_ALT_MASK
const ANDROID_META_ALT_MASK: i32 = 50;

#[link(name = "EGL")]
extern "C" {
    fn eglGetProcAddress(procname: *const c_char) -> *mut c_void;
}

impl Cx {
    
    /// Called when EGL is initialized.
    pub fn from_java_on_init(&mut self, params: AndroidInitParams, to_java: AndroidToJava) {
        // lets load dependencies here.
        self.android_load_dependencies(&to_java);
        
        self.os_type = OsType::Android(params.extract_android_params());
        self.os.dpi_factor = params.density;
        self.gpu_info.performance = GpuPerformance::Tier1;
        self.call_event_handler(&Event::Construct);
    } 
    
    pub fn from_java_on_pause(&mut self,  _to_java: AndroidToJava) {
        self.call_event_handler(&Event::Pause);
    }
     
    pub fn from_java_on_resume(&mut self, _to_java: AndroidToJava) {
        self.call_event_handler(&Event::Resume);
        let window_id = CxWindowPool::id_zero();
        if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
            self.redraw_pass_and_child_passes(main_pass_id);
        }
        self.redraw_all();
        self.reinitialise_media();
    }

    pub fn from_java_on_new_gl(&mut self,  to_java: AndroidToJava) {
        // init GL
        self.redraw_all(); 
        unsafe {gl_sys::load_with( | s | {   
            let s = CString::new(s).unwrap();
            eglGetProcAddress(s.as_ptr()) 
        })};
        
        to_java.schedule_timeout(0, 0); 
        to_java.schedule_redraw(); 
        self.after_every_event(&to_java);
    }    
 
    pub fn from_java_on_free_gl(&mut self,  _to_java: AndroidToJava) {
        // lets destroy all of our gl resources
        for texture in &mut self.textures.0.pool{
            texture.os.free_resources();
        }
        // delete all geometry buffers
        for geometry in &mut self.geometries.0.pool{
            geometry.os.free_resources();
        }

        for pass in &mut self.passes.0.pool{
            pass.os.free_resources();
        }
        // ok now we walk the views and remove all vaos and indexbuffers
        for draw_list in &mut self.draw_lists.0.pool{
            for item in &mut draw_list.draw_items.buffer{
                item.os.free_resources();
            }
        }
        
        for shader in &mut self.draw_shaders.os_shaders{
            shader.free_resources();
        }
    }
    
    pub fn android_load_dependencies(&mut self, to_java: &AndroidToJava) {
        for (path, dep) in &mut self.dependencies {
            if let Some(data) = to_java.read_asset(path) {
                dep.data = Some(Ok(data))
            }
            else {
                let message = format!("cannot load dependency {}", path);
                crate::makepad_error_log::error!("Android asset failed: {}", message);
                dep.data = Some(Err(message));
            }
        }
    }
    
    /// Called when the MakepadSurface is resized.
    pub fn from_java_on_resize(&mut self, width: i32, height: i32, to_java: AndroidToJava) {
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
        self.redraw_all();
        self.os.first_after_resize = true;
        self.after_every_event(&to_java);
    }
    
    /// Called when the MakepadSurface needs to be redrawn.
    pub fn from_java_on_draw(&mut self, to_java: AndroidToJava) {
        if self.new_next_frames.len() != 0 {
            self.call_next_frame_event(self.os.time_now());
        }
        if self.need_redrawing() {
            self.call_draw_event();
            
            //android_app.egl.make_current();
            self.opengl_compile_shaders();
        }
        
        if self.os.first_after_resize {
            self.os.first_after_resize = false;
            self.redraw_all();
        }
        
        self.handle_repaint(&to_java);
        self.after_every_event(&to_java);
    }
    
    /// Called when a touch event happened on the MakepadSurface.
    pub fn from_java_on_touch(&mut self, mut touches: Vec<TouchPoint>, to_java: AndroidToJava) {
        let time = self.os.time_now();
        for touch in &mut touches {
            // When the software keyboard shifted the UI in the vertical axis,
            //we need to make the math here to keep touch events positions synchronized.
            if self.os.keyboard_visible { touch.abs.y += self.os.keyboard_panning_offset as f64 };

            touch.abs /= self.os.dpi_factor;
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
        self.after_every_event(&to_java);
    }

    /// Called when a touch event happened on the software keyword
    pub fn from_java_on_key_down(&mut self, key_code_val: i32, characters: Option<String>, meta_state: i32, to_java: AndroidToJava) {
        let shift = meta_state & ANDROID_META_SHIFT_MASK != 0;
        let e: Event;

        match characters {
            Some(input) => {
                e = Event::TextInput(
                    TextInputEvent {
                        input: input,
                        replace_last: false,
                        was_paste: false,
                    }
                );
                self.call_event_handler(&e);
                self.after_every_event(&to_java);
            }
            None =>
                if let Some(native_keycode) = AndroidKeyCode::from_i32(key_code_val) {
                    let control = meta_state & ANDROID_META_CTRL_MASK != 0;
                    let alt = meta_state & ANDROID_META_ALT_MASK != 0;

                    let is_shortcut = control || alt;
                    let input_str = native_keycode.to_string(shift);

                    if input_str.is_some() && !is_shortcut {
                        let input = input_str.unwrap().to_string();
                        e = Event::TextInput(
                            TextInputEvent {
                                input: input,
                                replace_last: false,
                                was_paste: false,
                            }
                        )
                    } else {
                        e = Event::KeyDown(
                            KeyEvent {
                                key_code: AndroidKeyCode::to_makepad_key_code(key_code_val),
                                is_repeat: false,
                                modifiers: KeyModifiers {shift, control, alt, ..Default::default()},
                                time: self.os.time_now()
                            }
                        )
                    }
                    self.call_event_handler(&e);
                    self.after_every_event(&to_java);
                }
        }
    }
    
    /// Called when a timeout expired.
    pub fn from_java_on_timeout(&mut self, timer_id: i64, to_java: AndroidToJava) {
        if timer_id == 0 {
            if Signal::check_and_clear_ui_signal() {
                self.handle_media_signals(&to_java);
                self.call_event_handler(&Event::Signal);
            }
            to_java.schedule_timeout(0, 16);
        }
        else {
            self.call_event_handler(&Event::Timer(TimerEvent {timer_id: timer_id as u64}))
        }
        self.after_every_event(&to_java);
    }
    
    fn after_every_event(&mut self, to_java: &AndroidToJava) {
        self.handle_platform_ops(&to_java);
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 {
            to_java.schedule_redraw();
        }
    }
    
    pub fn from_java_on_midi_device_opened(&mut self, name: String, midi_device: jobject, to_java: AndroidToJava) {
        self.os.media.android_midi().lock().unwrap().midi_device_opened(name, midi_device, &to_java);
    }

    pub fn from_java_on_hide_text_ime(&mut self, to_java: AndroidToJava) {
        self.text_ime_was_dismissed();
        self.redraw_all();
        self.after_every_event(&to_java);
    }

    pub fn from_java_on_resize_text_ime(&mut self, ime_height: i32, to_java: AndroidToJava) {
        self.os.keyboard_visible = true;
        self.panning_adjust_for_text_ime(ime_height);
        self.redraw_all();
        self.after_every_event(&to_java);
    }

    pub fn from_java_copy_to_clipboard(&mut self, to_java: AndroidToJava) {
        let response = Rc::new(RefCell::new(None));
        let e = Event::TextCopy(TextCopyEvent {
            response: response.clone()
        });
        self.call_event_handler(&e);
        self.after_every_event(&to_java);
    }

    pub fn from_java_paste_from_clipboard(&mut self, content: String, to_java: AndroidToJava) {
        let e = Event::TextInput(
            TextInputEvent {
                input: content,
                replace_last: false,
                was_paste: true,
            }
        );
        self.call_event_handler(&e);
        self.after_every_event(&to_java);
    }

    pub fn from_java_cut_to_clipboard(&mut self, to_java: AndroidToJava) {
        let e = Event::TextCut;
        self.call_event_handler(&e);
        self.after_every_event(&to_java);
    }

    pub fn draw_pass_to_fullscreen(
        &mut self,
        pass_id: PassId,
        to_java: &AndroidToJava,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();

        self.setup_render_pass(pass_id, self.os.dpi_factor);
        
        // keep repainting in a loop 
        self.passes[pass_id].paint_dirty = false;
        let panning_offset = if self.os.keyboard_visible { self.os.keyboard_panning_offset } else { 0 };
        
        unsafe {
            gl_sys::Viewport(0, panning_offset, self.os.display_size.x as i32, self.os.display_size.y as i32);
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
        
        to_java.swap_buffers();
        //unsafe {
        //direct_app.drm.swap_buffers_and_wait(&direct_app.egl);
        //}
    }
    
    pub (crate) fn handle_repaint(&mut self, to_java: &AndroidToJava) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(_window_id) => {
                    self.draw_pass_to_fullscreen(*pass_id, to_java);
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

    fn panning_adjust_for_text_ime(&mut self, android_ime_height: i32) {
        self.os.keyboard_visible = true;

        let screen_height = (self.os.display_size.y / self.os.dpi_factor) as i32;
        let vertical_offset = self.os.keyboard_trigger_position.y as i32;
        let ime_height = (android_ime_height as f64 / self.os.dpi_factor) as i32;

        // Make sure there is some room between the software keyword and the text input or widget that triggered
        // the TextIME event
        let vertical_space = ime_height / 3;

        let should_be_panned = vertical_offset > screen_height - ime_height;
        if should_be_panned {
            let panning_offset = vertical_offset - (screen_height - ime_height) + vertical_space;
            self.os.keyboard_panning_offset = (panning_offset as f64 * self.os.dpi_factor) as i32;
        } else {
            self.os.keyboard_panning_offset = 0;
        }
    }
    
    fn handle_platform_ops(&mut self, to_java: &AndroidToJava) -> EventFlow {
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
                    to_java.schedule_timeout(timer_id as i64, (interval / 1000.0) as i64);
                },
                CxOsOp::StopTimer(timer_id) => {
                    to_java.cancel_timeout(timer_id as i64);
                    //android_app.stop_timer(timer_id);
                },
                CxOsOp::ShowTextIME(area, _pos) => {
                    self.os.keyboard_trigger_position = area.get_clipped_rect(self).pos;
                    to_java.show_text_ime();
                },
                CxOsOp::HideTextIME => {
                    self.os.keyboard_visible = false;
                    to_java.hide_text_ime();
                },
                CxOsOp::ShowClipboardActions(selected) => {
                    to_java.show_clipboard_actions(selected.as_str());
                },
                _ => ()
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
            first_after_resize: true,
            display_size: dvec2(100., 100.),
            dpi_factor: 1.5,
            time_start: Instant::now(),
            keyboard_visible: false,
            keyboard_trigger_position: DVec2::default(),
            keyboard_panning_offset: 0,
            media: CxAndroidMedia::default()
        }
    }
}

pub struct CxOs {
    pub first_after_resize: bool,
    pub display_size: DVec2,
    pub dpi_factor: f64,
    pub time_start: Instant,

    pub keyboard_visible: bool,
    pub keyboard_trigger_position: DVec2,
    pub keyboard_panning_offset: i32,

    pub (crate) media: CxAndroidMedia,
}

impl CxOs {
    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_micros() as f64 / 1_000_000.0
    }
}
