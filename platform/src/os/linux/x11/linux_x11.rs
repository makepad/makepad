use {
    std::cell::RefCell,
    std::rc::Rc,
    self::super::{
        opengl_x11::{OpenglWindow,OpenglCx},
    },
    self::super::super::{
        x11::xlib_event::*,
        x11::xlib_app::*,
        linux_media::CxLinuxMedia
    },
    crate::{
        cx_api::{CxOsOp, CxOsApi}, 
        makepad_math::{dvec2},
        makepad_live_id::*,
        thread::Signal,
        event::{
            WebSocket,
            WebSocketAutoReconnect, 
            Event,
        },
        pass::CxPassParent,
        cx::{Cx, OsType,}, 
        gpu_info::GpuPerformance,
        os::cx_native::EventFlow,
        
    }
};

impl Cx {
    pub fn event_loop(mut self) {
        self.platform_type = OsType::LinuxWindow {custom_window_chrome: false};
        self.gpu_info.performance = GpuPerformance::Tier1;
        
        let opengl_cx = Rc::new(RefCell::new(None));
        let opengl_windows = Rc::new(RefCell::new(Vec::new()));
        let cx = Rc::new(RefCell::new(self));
        
        init_xlib_app_global(Box::new({
            let cx = cx.clone();
            let opengl_cx = opengl_cx.clone();
            move | xlib_app,
            events | {
                let mut cx = cx.borrow_mut();
                let mut opengl_cx = opengl_cx.borrow_mut();
                let mut opengl_windows = opengl_windows.borrow_mut();
                cx.xlib_event_callback(xlib_app, events, opengl_cx.as_mut().unwrap(), &mut *opengl_windows)
            }
        }));
        
        *opengl_cx.borrow_mut() = Some(OpenglCx::new(get_xlib_app_global().display));
        
        cx.borrow_mut().call_event_handler(&Event::Construct);
        cx.borrow_mut().redraw_all();
        get_xlib_app_global().start_timer(0,0.008,true);
        get_xlib_app_global().event_loop();
    }
    
    fn xlib_event_callback(
        &mut self,
        xlib_app: &mut XlibApp, 
        event: XlibEvent,
        opengl_cx: &mut OpenglCx,
        opengl_windows: &mut Vec<OpenglWindow>
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(opengl_windows, opengl_cx, xlib_app) {
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
            XlibEvent::WindowGeomChange(re) => { // do this here because mac
                if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == re.window_id) {
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
                    opengl_cx.make_current();
                    self.opengl_compile_shaders();
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint(opengl_windows, opengl_cx);
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
            XlibEvent::Timer(e) => {
                //println!("TIMER! {:?}", std::time::Instant::now());
                if e.timer_id == 0{
                    if Signal::check_and_clear_ui_signal(){
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
    
    pub (crate) fn handle_repaint(&mut self, opengl_windows: &mut Vec<OpenglWindow>, opengl_cx: &mut OpenglCx) {
        opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(window_id) => {
                    if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers(&opengl_cx);
                        self.draw_pass_to_window(*pass_id, dpi_factor, window, opengl_cx);
                    }
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
    
    fn handle_platform_ops(&mut self, opengl_windows: &mut Vec<OpenglWindow>, opengl_cx: &OpenglCx, xlib_app: &mut XlibApp) -> EventFlow {
        let mut ret = EventFlow::Poll;
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let opengl_window = OpenglWindow::new(
                        window_id,
                        &opengl_cx,
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
                }
                CxOsOp::UpdateMenu(_menu) => {
                }
            }
        }
        ret
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

#[derive(Default)]
pub struct CxOs {
    pub (crate) media: CxLinuxMedia,
}

