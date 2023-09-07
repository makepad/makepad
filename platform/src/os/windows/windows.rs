use {
    std::{
        rc::Rc,
        cell::RefCell,
    },
    crate::{
        makepad_live_id::*,
        cx::*,
        event::*,
        thread::Signal,
        os::{
            windows::{
                //windows_media::CxWindowsMedia,
                win32_event::*,
                d3d11::{D3d11Window, D3d11Cx},
                win32_app::*,
            },
            cx_native::EventFlow,
        },
        pass::CxPassParent,
        cx_api::{CxOsApi, CxOsOp},
    }
};

impl Cx {
    
    pub fn event_loop(cx:Rc<RefCell<Cx>>) {
        
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Windows;
        
        let d3d11_cx = Rc::new(RefCell::new(D3d11Cx::new()));
        
        for arg in std::env::args() {
            if arg == "--stdin-loop" {
                let mut cx = cx.borrow_mut();
                let mut d3d11_cx = d3d11_cx.borrow_mut();
                return cx.stdin_event_loop(&mut d3d11_cx);
            }
        }
        
        let d3d11_windows = Rc::new(RefCell::new(Vec::new()));
        
        init_win32_app_global(Box::new({
            let cx = cx.clone();
            move | win32_app,
            event | {
                let mut cx = cx.borrow_mut();
                let mut d3d11_cx = d3d11_cx.borrow_mut();
                let mut d3d11_windows = d3d11_windows.borrow_mut();
                cx.win32_event_callback(win32_app, event, &mut d3d11_cx, &mut d3d11_windows)
            }
        }));
        
        cx.borrow_mut().call_event_handler(&Event::Construct);
        cx.borrow_mut().redraw_all();
        get_win32_app_global().start_signal_poll();
        get_win32_app_global().event_loop();
    }
    
    fn win32_event_callback(
        &mut self,
        win32_app: &mut Win32App,
        event: Win32Event,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(d3d11_windows, d3d11_cx, win32_app){
            return EventFlow::Exit
        }
        
        let mut paint_dirty = false;
    
        //self.process_desktop_pre_event(&mut event);
        match event { 
            Win32Event::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in d3d11_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                paint_dirty = true;
                self.call_event_handler(&Event::AppGotFocus);
            }
            Win32Event::AppLostFocus => {
                self.call_event_handler(&Event::AppLostFocus);
            }
            Win32Event::WindowResizeLoopStart(window_id) => {
                if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                    window.start_resize();
                }
            }
            Win32Event::WindowResizeLoopStop(window_id) => {
                if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                    window.stop_resize();
                }
            }
            Win32Event::WindowGeomChange(re) => { // do this here because mac
                if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == re.window_id) {
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
            Win32Event::WindowClosed(wc) => {
                let window_id = wc.window_id;
                self.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                self.windows[window_id].is_created = false;
                if let Some(index) = d3d11_windows.iter().position( | w | w.window_id == window_id) {
                    d3d11_windows.remove(index);
                    if d3d11_windows.len() == 0 {
                        return EventFlow::Exit
                    }
                }
            }
            Win32Event::Paint => {
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(win32_app.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    self.hlsl_compile_shaders(&d3d11_cx);
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint(d3d11_windows, d3d11_cx);
            }
            Win32Event::MouseDown(e) => {
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            Win32Event::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            Win32Event::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            Win32Event::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            Win32Event::WindowDragQuery(e) => {
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            Win32Event::WindowCloseRequested(e) => {
                self.call_event_handler(&Event::WindowCloseRequested(e))
            }
            Win32Event::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e))
            }
            Win32Event::Drag(e) => {
                self.call_event_handler(&Event::Drag(e))
            }
            Win32Event::Drop(e) => {
                self.call_event_handler(&Event::Drop(e))
            }
            Win32Event::DragEnd => {
                self.call_event_handler(&Event::DragEnd)
            }
            Win32Event::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            Win32Event::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            Win32Event::TextCopy(e) => {
                self.call_event_handler(&Event::TextCopy(e))
            }
            Win32Event::TextCut(e) => {
                self.call_event_handler(&Event::TextCut(e))
            }
            Win32Event::Timer(e) => {
                self.call_event_handler(&Event::Timer(e))
            }
            Win32Event::Signal => {
                if Signal::check_and_clear_ui_signal(){
                    //self.handle_media_signals();
                    self.call_event_handler(&Event::Signal);
                }
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
        
    }
    
    pub (crate) fn handle_repaint(&mut self, d3d11_windows: &mut Vec<D3d11Window>, d3d11_cx: &mut D3d11Cx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers(&d3d11_cx);
                        self.draw_pass_to_window(*pass_id, true, window, d3d11_cx);
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_magic_texture(*pass_id, d3d11_cx);
                },
                CxPassParent::None => {
                    self.draw_pass_to_magic_texture(*pass_id, d3d11_cx);
                }
            }
        }
    }
    
    pub(crate) fn handle_networking_events(&mut self) {
    }
    
    fn handle_platform_ops(&mut self, d3d11_windows: &mut Vec<D3d11Window>, d3d11_cx: &D3d11Cx, win32_app: &mut Win32App)->EventFlow {
        let mut ret = EventFlow::Poll;
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let d3d11_window = D3d11Window::new(
                        window_id,
                        &d3d11_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title
                    );
                    
                    window.window_geom = d3d11_window.window_geom.clone();
                    d3d11_windows.push(d3d11_window);
                    window.is_created = true;
                },
                CxOsOp::CloseWindow(window_id) => {
                    if let Some(index) = d3d11_windows.iter().position( | w | w.window_id == window_id) {
                        self.windows[window_id].is_created = false;
                        d3d11_windows[index].win32_window.close_window();
                        d3d11_windows.remove(index);
                        if d3d11_windows.len() == 0 {
                            ret = EventFlow::Exit
                        }
                    }
                },
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.minimize();
                    }
                },
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.maximize();
                    }
                },
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.restore();
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
                CxOsOp::ShowClipboardActions(_) =>{
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
                    win32_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    win32_app.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    win32_app.stop_timer(timer_id);
                },
                CxOsOp::StartDragging(_dragged_item) => {
                },
                CxOsOp::UpdateMenu(_menu) => {
                },
                CxOsOp::HttpRequest{request_id:_, request:_} => {
                    //todo!()
                },
                CxOsOp::WebSocketOpen{request_id:_, request:_,}=>{
                    //todo!()
                }
                CxOsOp::WebSocketSendBinary{request_id:_, data:_}=>{
                    //todo!()
                }
                CxOsOp::WebSocketSendString{request_id:_, data:_}=>{
                    //todo!()
                }
            }
        }
        ret
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
}

#[derive(Default)]
pub struct CxOs {
    //pub (crate) media: CxWindowsMedia,
}
