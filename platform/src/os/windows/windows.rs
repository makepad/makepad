use {
    std::{
        time::Instant,
        rc::Rc,
        cell::RefCell,
    },
    crate::{
        makepad_live_id::*,
        cx::*,
        event::*,
        thread::SignalToUI,
        os::{
            windows::{
                windows_media::CxWindowsMedia,
                win32_event::*,
                d3d11::{D3d11Window, D3d11Cx},
                win32_app::*,
                win32_window::Win32Window,
            },
            cx_native::EventFlow,
        },
        makepad_math::*,
        pass::CxPassParent,
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        window::CxWindowPool,
        windows::Win32::Graphics::Direct3D11::ID3D11Device,
    }
};

impl Cx {
    
    pub fn event_loop(cx: Rc<RefCell<Cx >>) {
        
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Windows;
        
        let d3d11_cx = Rc::new(RefCell::new(D3d11Cx::new()));

        // hack: store ID3D11Device in CxOs, so texture-related operations become possible on the makepad/studio side, yet don't completely destroy the code there
        cx.borrow_mut().os.d3d11_device = Some(d3d11_cx.borrow().device.clone());

        for arg in std::env::args() {
            if arg == "--stdin-loop" {
                let mut cx = cx.borrow_mut();
                cx.in_makepad_studio = true;
                let mut d3d11_cx = d3d11_cx.borrow_mut();
                return cx.stdin_event_loop(&mut d3d11_cx);
            }
        }
        
        let d3d11_windows = Rc::new(RefCell::new(Vec::new()));
        
        init_win32_app_global(Box::new({
            let cx = cx.clone();
            move | event | {
                let mut cx = cx.borrow_mut();
                let mut d3d11_cx = d3d11_cx.borrow_mut();
                let mut d3d11_windows = d3d11_windows.borrow_mut();
                cx.win32_event_callback(event, &mut d3d11_cx, &mut d3d11_windows)
            }
        }));
        // the signal poll timer
        with_win32_app(|app| app.start_timer(0, 0.008, true));
        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();
        with_win32_app(|app| app.start_signal_poll());
        Win32App::event_loop();
    }
    
    fn win32_event_callback(
        &mut self,
        event: Win32Event,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(d3d11_windows, d3d11_cx) {
            return EventFlow::Exit
        }
        
        //let mut paint_dirty = false;
        /*match &event{
            Win32Event::Timer(time) =>{
                
            }
            _=>{}
        }*/

        //self.process_desktop_pre_event(&mut event);
        match event {
            Win32Event::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in d3d11_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                //paint_dirty = true;
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
            Win32Event::WindowGeomChange(mut re) => { // do this here because mac
               
                if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == re.window_id) {
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
                self.redraw_all();
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
                        self.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit
                    }
                }
            }
            Win32Event::Paint => {
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(with_win32_app(|app| app.time_now()));
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
                self.fingers.mouse_down(e.button, e.window_id);
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
            Win32Event::MouseLeave(e) => {
                self.call_event_handler(&Event::MouseLeave(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
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
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            },
            Win32Event::Drop(e) => {
                self.call_event_handler(&Event::Drop(e));
                self.drag_drop.cycle_drag();
            },
            Win32Event::DragEnd => {
                // send MouseUp
                self.call_event_handler(&Event::MouseUp(MouseUpEvent {
                    abs: dvec2(-100000.0, -100000.0),
                    button: MouseButton::PRIMARY,
                    window_id: CxWindowPool::id_zero(),
                    modifiers: Default::default(),
                    time: 0.0
                }));
                self.fingers.mouse_up(MouseButton::PRIMARY);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
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
                if SignalToUI::check_and_clear_ui_signal() {
                    self.handle_media_signals();
                    self.call_event_handler(&Event::Signal);
                }
                if SignalToUI::check_and_clear_action_signal() {
                    self.handle_action_receiver();
                }

                if self.handle_live_edit() {
                    self.call_event_handler(&Event::LiveEdit);
                    self.redraw_all();
                }
                self.handle_networking_events();
                
                self.win32_event_callback(Win32Event::Paint, d3d11_cx, d3d11_windows);
                
                return EventFlow::Wait;
            }
        }
        
        return EventFlow::Poll;
        /*
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }*/
        
    }
    
    pub (crate) fn handle_repaint(&mut self, d3d11_windows: &mut Vec<D3d11Window>, d3d11_cx: &mut D3d11Cx) {
        
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            self.passes[*pass_id].set_time(with_win32_app(|app| app.time_now() as f32));
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Xr => {}
                CxPassParent::Window(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = window.window_geom.dpi_factor;                        
                        window.resize_buffers(&d3d11_cx);
                        self.draw_pass_to_window(*pass_id, false, window, d3d11_cx);
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(*pass_id, d3d11_cx, None);
                },
                CxPassParent::None => {
                    self.draw_pass_to_texture(*pass_id, d3d11_cx, None);
                }
            }
        }
    }
    
    pub(crate) fn handle_networking_events(&mut self) {
        let mut out = Vec::new();
        while let Ok(event) = self.os.network_response.receiver.try_recv(){
            out.push(event);
        }
        if out.len()>0{
            self.call_event_handler(& Event::NetworkResponses(out))
        }
    }
    
    fn handle_platform_ops(&mut self, d3d11_windows: &mut Vec<D3d11Window>, d3d11_cx: &D3d11Cx) -> EventFlow {
        let mut ret = EventFlow::Poll;
        let mut geom_changes = Vec::new();
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
                    geom_changes.push(WindowGeomChangeEvent{
                        window_id,
                        old_geom: window.window_geom.clone(),
                        new_geom: window.window_geom.clone()
                    });
                },
                CxOsOp::CloseWindow(window_id) => {
                    self.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
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
                CxOsOp::Deminiaturize(_window_id) => todo!(),
                CxOsOp::HideWindow(_window_id) => todo!(),
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.maximize();
                    }
                },
                CxOsOp::ResizeWindow(window_id, size) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.set_inner_size(size);
                    }
                }
                CxOsOp::RepositionWindow(window_id, pos) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.set_position(pos);
                    }
                }
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.restore();
                    }
                },
                CxOsOp::Quit=>{
                    ret = EventFlow::Exit
                }
                CxOsOp::SetTopmost(window_id, is_topmost) => {
                    if d3d11_windows.len() == 0 {
                        self.platform_ops.insert(0, CxOsOp::SetTopmost(window_id, is_topmost));
                        continue;
                    }
                    if let Some(window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.win32_window.set_topmost(is_topmost);
                    }
                }
                CxOsOp::CopyToClipboard(content) => {
                    unsafe {
                        Win32Window::copy_to_clipboard(&content);
                    }
                },
                CxOsOp::SetCursor(cursor) => {
                    with_win32_app(|app| app.set_mouse_cursor(cursor));
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    with_win32_app(|app| app.start_timer(timer_id, interval, repeats));
                },
                CxOsOp::StopTimer(timer_id) => {
                    with_win32_app(|app| app.stop_timer(timer_id));
                },
                CxOsOp::StartDragging(dragged_item) => {
                    with_win32_app(|app| app.start_dragging(dragged_item));
                },
                CxOsOp::HttpRequest {request_id, request} => {
                    use crate::os::windows::http::WindowsHttpSocket;
                    WindowsHttpSocket::open(request_id, request, self.os.network_response.sender.clone());

                    //todo!("HttpRequest not implemented yet on windows, we'll get there");
                },
                e=>{
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        if geom_changes.len()>0{
            self.redraw_all();
            for geom_change in geom_changes{
                self.call_event_handler(&Event::WindowGeomChange(geom_change));
            }
        }
        ret
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR"){
            self.live_registry.borrow_mut().package_root = Some(item.to_string());
        }
        
        self.live_expand();
        if std::env::args().find( | v | v == "--stdin-loop").is_none() {
            self.start_disk_live_file_watcher(100);
        }
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time.unwrap()).as_secs_f64()
    }
    
    fn open_url(&mut self, _url:&str, _in_place:OpenUrlInPlace){
        crate::error!("open_url not implemented on this platform");
    }
}

#[derive(Default)]
pub struct CxOs {
    pub (crate) start_time: Option<Instant>,
    pub (crate) media: CxWindowsMedia,
    pub (crate) d3d11_device: Option<ID3D11Device>,
    pub (crate) network_response: NetworkResponseChannel,
    //pub (crate) new_frame_being_rendered: Option<crate::cx_stdin::PresentableDraw>,
}
