use {
    std::cell::RefCell,
    std::time::Instant,
    std::rc::Rc,
    self::super::opengl_x11::{
        OpenglWindow,
        OpenglCx
    },
    self::super::super::{
        egl_sys,
        x11::xlib_event::*,
        x11::xlib_app::*,
        linux_media::CxLinuxMedia
    },
    crate::{
        cx_api::{CxOsOp, CxOsApi}, 
        makepad_math::dvec2,
        makepad_live_id::*,
        thread::SignalToUI,
        event::Event,
        pass::CxPassParent,
        cx::{Cx, OsType,LinuxWindowParams}, 
        os::cx_stdin::{PollTimers},
        gpu_info::GpuPerformance,
        os::cx_native::EventFlow,
    }
};

impl Cx {
    pub fn event_loop(cx:Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::LinuxWindow(LinuxWindowParams{
            custom_window_chrome: false
        });
        cx.borrow_mut().gpu_info.performance = GpuPerformance::Tier1;

        let opengl_windows = Rc::new(RefCell::new(Vec::new()));
        let is_stdin_loop = std::env::args().find(|v| v=="--stdin-loop").is_some();
        init_xlib_app_global(Box::new({
            let cx = cx.clone();
            move | xlib_app,
            events | {
                if is_stdin_loop{
                    return EventFlow::Wait
                }
                let mut cx = cx.borrow_mut();
                let mut opengl_windows = opengl_windows.borrow_mut();
                cx.xlib_event_callback(xlib_app, events, &mut *opengl_windows)
            }
        }));
        
        cx.borrow_mut().os.opengl_cx = Some(unsafe {
            OpenglCx::from_egl_platform_display(
                egl_sys::EGL_PLATFORM_X11_EXT,
                get_xlib_app_global().display,
            )
        });
        
        if is_stdin_loop {
            return cx.borrow_mut().stdin_event_loop();
        }
        
        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();
        get_xlib_app_global().start_timer(0,0.008,true);
        get_xlib_app_global().event_loop();
    }
    
    fn xlib_event_callback(
        &mut self,
        xlib_app: &mut XlibApp, 
        event: XlibEvent,
        opengl_windows: &mut Vec<OpenglWindow>
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(opengl_windows, xlib_app) {
            return EventFlow::Exit
        }
        
        let mut paint_dirty = false;
        
        //self.process_desktop_pre_event(&mut event);
        match event {
            XlibEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in opengl_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                paint_dirty = true;
                self.call_event_handler(&Event::AppGotFocus);
            }
            XlibEvent::AppLostFocus => { 
                self.call_event_handler(&Event::AppLostFocus);
            }
            XlibEvent::WindowGeomChange(mut re) => { // do this here because mac
                if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == re.window_id) {
                    if let Some(dpi_override) = self.windows[re.window_id].dpi_override {
                        re.new_geom.inner_size *= re.new_geom.dpi_factor / dpi_override;
                        re.new_geom.dpi_factor = dpi_override;
                    }
                    
                    window.window_geom = re.new_geom.clone();
                    self.windows[re.window_id].window_geom = re.new_geom.clone();
                    // redraw just this windows root draw list
                    if re.old_geom.inner_size != re.new_geom.inner_size {
                        if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                            self.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                }
                // ok lets not redraw all, just this window
                self.call_event_handler(&Event::WindowGeomChange(re));
            }
            XlibEvent::WindowClosed(wc) => {
                let window_id = wc.window_id;
                self.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                self.windows[window_id].is_created = false;
                if let Some(index) = opengl_windows.iter().position( | w | w.window_id == window_id) {
                    opengl_windows.remove(index);
                    if opengl_windows.len() == 0 {
                        xlib_app.terminate_event_loop();
                        self.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit
                    }
                }
            }
            XlibEvent::Paint => {
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(xlib_app.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    self.os.opengl_cx.as_ref().unwrap().make_current();
                    self.opengl_compile_shaders();
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint(opengl_windows);
            }
            XlibEvent::MouseDown(e) => {
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            XlibEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            XlibEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            XlibEvent::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            XlibEvent::WindowDragQuery(e) => {
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            XlibEvent::WindowCloseRequested(e) => {
                self.call_event_handler(&Event::WindowCloseRequested(e))
            }
            XlibEvent::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e))
            }
            XlibEvent::Drag(e) => {
                self.call_event_handler(&Event::Drag(e))
            }
            XlibEvent::Drop(e) => {
                self.call_event_handler(&Event::Drop(e))
            }
            XlibEvent::DragEnd => {
                self.call_event_handler(&Event::DragEnd)
            }
            XlibEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            XlibEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            XlibEvent::TextCopy(e) => {
                self.call_event_handler(&Event::TextCopy(e))
            }
            XlibEvent::TextCut(e) => {
                self.call_event_handler(&Event::TextCut(e))
            }
            XlibEvent::Timer(e) => {
                //println!("TIMER! {:?}", std::time::Instant::now());
                if e.timer_id == 0{
                    if SignalToUI::check_and_clear_ui_signal(){
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                }
                else{
                    self.call_event_handler(&Event::Timer(e))
                }
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
        
    }

    pub(crate) fn handle_networking_events(&mut self) {
    }
    
    pub (crate) fn handle_repaint(&mut self, opengl_windows: &mut Vec<OpenglWindow>) {
        self.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            self.passes[*pass_id].set_time(get_xlib_app_global().time_now() as f32);
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(window_id) => {
                    if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers();
                        self.draw_pass_to_window(*pass_id, window);
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_magic_texture(*pass_id);
                },
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(*pass_id);
                }
            }
        }
    }
    
    fn handle_platform_ops(&mut self, opengl_windows: &mut Vec<OpenglWindow>, xlib_app: &mut XlibApp) -> EventFlow {
        let mut ret = EventFlow::Poll;
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let opengl_window = OpenglWindow::new(
                        window_id,
                        self.os.opengl_cx.as_ref().unwrap(),
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                    );
                    window.window_geom = opengl_window.window_geom.clone();
                    opengl_windows.push(opengl_window);
                    window.is_created = true;
                },
                CxOsOp::CloseWindow(window_id) => {
                    if let Some(index) = opengl_windows.iter().position( | w | w.window_id == window_id) {
                        self.windows[window_id].is_created = false;
                        opengl_windows[index].xlib_window.close_window();
                        opengl_windows.remove(index);
                        if opengl_windows.len() == 0 {
                            ret = EventFlow::Exit
                        }
                    }
                },
                CxOsOp::Quit=>{
                    ret = EventFlow::Exit
                }
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.xlib_window.minimize();
                    }
                },
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.xlib_window.maximize();
                    }
                },
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.xlib_window.restore();
                    }
                },
                CxOsOp::ShowClipboardActions(_) =>{
                }
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
                    //todo!()
                }
                CxOsOp::HideTextIME => {
                    //todo!()
                },
                CxOsOp::SetCursor(cursor) => {
                    xlib_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    xlib_app.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    xlib_app.stop_timer(timer_id);
                },
                CxOsOp::StartDragging(_dragged_item) => {
                },
                CxOsOp::UpdateMacosMenu(_menu) => {
                },
                CxOsOp::HttpRequest{request_id:_, request:_} => {
                    todo!()
                },
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
        ret
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        self.live_expand();
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time.unwrap()).as_secs_f64()
    }
}

#[derive(Default)]
pub struct CxOs {
    pub(crate) media: CxLinuxMedia,
    pub (crate) stdin_timers: PollTimers,
    pub (crate) start_time: Option<Instant>,
    // HACK(eddyb) generalize this to EGL, properly.
    pub(super) opengl_cx: Option<OpenglCx>,
}

