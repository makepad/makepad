use {
    std::{
        rc::Rc,
        cell::{RefCell},
    },
    makepad_objc_sys::{
        msg_send,
        sel,
        sel_impl,
    },
    crate::{
        makepad_live_id::*,
        makepad_math::*,
        os::{
            cx_native::EventFlow,
            apple::apple_sys::*,
            cocoa_event::{CocoaEvent},
            metal_xpc::{
                start_xpc_service,
            },
            cocoa_app::{
                CocoaApp,
                get_cocoa_app_global,
                init_cocoa_app_global
            },
            apple_media::CxAppleMedia,
            metal::{MetalCx, MetalWindow, DrawPassMode},
        },
        pass::{CxPassParent},
        thread::Signal,
        event::{
            MouseUpEvent,
            Event,
            NetworkResponseChannel
        },
        window::CxWindowPool,
        cx_api::{CxOsApi, CxOsOp},
        cx::{Cx, OsType},
    }
};

const KEEP_ALIVE_COUNT: usize = 5;

impl Cx {
    
    pub fn event_loop(cx:Rc<RefCell<Cx>>) {
        for arg in std::env::args() {
            if arg == "--metal-xpc" {
                return start_xpc_service();
            }
        }
        
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Macos;
        let metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        //let cx = Rc::new(RefCell::new(self));
        
        for arg in std::env::args() {
            if arg == "--stdin-loop" {
                let mut cx = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                return cx.stdin_event_loop(&mut metal_cx);
            }
        }
        
        let metal_windows = Rc::new(RefCell::new(Vec::new()));
        
        init_cocoa_app_global(Box::new({
            let cx = cx.clone();
            move | cocoa_app,
            event | {
                let mut cx_ref = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let mut metal_windows = metal_windows.borrow_mut();
                let event_flow = cx_ref.cocoa_event_callback(cocoa_app, event, &mut metal_cx, &mut metal_windows);
                let executor = cx_ref.executor.take().unwrap();
                drop(cx_ref);
                executor.run_until_stalled();
                let mut cx_ref = cx.borrow_mut();
                cx_ref.executor = Some(executor);
                event_flow
            }
        }));
        // lets set our signal poll timer
        
        // final bit of initflow
        get_cocoa_app_global().start_timer(0, 0.008, true);
        cx.borrow_mut().call_event_handler(&Event::Construct);
        cx.borrow_mut().redraw_all();
        get_cocoa_app_global().event_loop();
    }
    
    pub (crate) fn handle_repaint(&mut self, metal_windows: &mut Vec<MetalWindow>, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Window(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = metal_window.window_geom.dpi_factor;
                        metal_window.resize_core_animation_layer(&metal_cx);
                        let drawable: ObjcId = unsafe {msg_send![metal_window.ca_layer, nextDrawable]};
                        if drawable == nil {
                            return
                        }
                        if metal_window.is_resizing {
                            self.draw_pass(*pass_id, metal_cx, DrawPassMode::Resizing(drawable));
                        }
                        else {
                            self.draw_pass(*pass_id, metal_cx, DrawPassMode::Drawable(drawable));
                        }
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::Texture);
                },
                CxPassParent::None => {
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::Texture);
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
            self.call_event_handler(&Event::NetworkResponses(out))
        }
    }
    
    fn cocoa_event_callback(
        &mut self,
        cocoa_app: &mut CocoaApp,
        event: CocoaEvent,
        metal_cx: &mut MetalCx,
        metal_windows: &mut Vec<MetalWindow>
    ) -> EventFlow {
        
        self.handle_platform_ops(metal_windows, metal_cx, cocoa_app);
         
        // send a mouse up when dragging starts
        
        let mut paint_dirty = false;
        match &event {
            CocoaEvent::MouseDown(_) |
            CocoaEvent::MouseMove(_) |
            CocoaEvent::MouseUp(_) |
            CocoaEvent::Scroll(_) |
            CocoaEvent::KeyDown(_) |
            CocoaEvent::KeyUp(_) |
            CocoaEvent::TextInput(_) => {
                self.os.keep_alive_counter = KEEP_ALIVE_COUNT;
            }
            CocoaEvent::Timer(te) => {
                if te.timer_id == 0 {
                    if self.os.keep_alive_counter>0 {
                        self.os.keep_alive_counter -= 1;
                        self.repaint_windows();
                    }

                    // check signals
                    if Signal::check_and_clear_ui_signal(){
                        self.handle_media_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if self.was_live_edit(){
                        self.draw_shaders.ptr_to_item.clear();
                        self.draw_shaders.fingerprints.clear();
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();

                    return EventFlow::Poll;
                }
            }
            _ => ()
        }
        
        //self.process_desktop_pre_event(&mut event);
        match event {
            CocoaEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in metal_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                paint_dirty = true;
                self.call_event_handler(&Event::AppGotFocus);
            }
            CocoaEvent::AppLostFocus => {
                self.call_event_handler(&Event::AppLostFocus);
            }
            CocoaEvent::WindowResizeLoopStart(window_id) => {
                if let Some(window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                    window.start_resize();
                }
            }
            CocoaEvent::WindowResizeLoopStop(window_id) => {
                if let Some(window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                    window.stop_resize();
                }
            }
            CocoaEvent::WindowGeomChange(mut re) => { // do this here because mac
                if let Some(window) = metal_windows.iter_mut().find( | w | w.window_id == re.window_id) {
                    if let Some(dpi_override) = self.windows[re.window_id].dpi_override{
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
            CocoaEvent::WindowClosed(wc) => {
                // lets remove the window from the set
                let window_id = wc.window_id;
                self.call_event_handler(&Event::WindowClosed(wc));
                
                self.windows[window_id].is_created = false;
                if let Some(index) = metal_windows.iter().position( | w | w.window_id == window_id) {
                    metal_windows.remove(index);
                    if metal_windows.len() == 0 {
                        return EventFlow::Exit
                    }
                }
            }
            CocoaEvent::Paint => {
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(cocoa_app.time_now());
                }
                if self.need_redrawing() {
                    self.call_draw_event();
                    self.mtl_compile_shaders(&metal_cx);
                }
                // ok here we send out to all our childprocesses
                
                self.handle_repaint(metal_windows, metal_cx);
                
            }
            CocoaEvent::MouseDown(e) => {
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            CocoaEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            CocoaEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            CocoaEvent::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            CocoaEvent::WindowDragQuery(e) => {
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            CocoaEvent::WindowCloseRequested(e) => {
                self.call_event_handler(&Event::WindowCloseRequested(e))
            }
            CocoaEvent::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e))
            }
            CocoaEvent::Drag(e) => {
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            }
            CocoaEvent::Drop(e) => {
                self.call_event_handler(&Event::Drop(e));
                self.drag_drop.cycle_drag();
            }
            CocoaEvent::DragEnd => {
                // lets send mousebutton ups to fix missing it.
                // TODO! make this more resilient
                self.call_event_handler(&Event::MouseUp(MouseUpEvent{
                    abs: dvec2(0.0,0.0),
                    button: 0,
                    window_id: CxWindowPool::id_zero(),
                    modifiers: Default::default(),
                    time: 0.0
                }));
                self.fingers.mouse_up(0);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                
                self.call_event_handler(&Event::DragEnd);
                self.drag_drop.cycle_drag();
            }
            CocoaEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            CocoaEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            CocoaEvent::TextCopy(e) => {
                self.call_event_handler(&Event::TextCopy(e))
            }
            CocoaEvent::TextCut(e) => {
                self.call_event_handler(&Event::TextCut(e))
            }
            CocoaEvent::Timer(e) => {
                self.call_event_handler(&Event::Timer(e))
            }
            CocoaEvent::MenuCommand(e) => {
                self.call_event_handler(&Event::MenuCommand(e))
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
    }
    
    fn handle_platform_ops(&mut self, metal_windows: &mut Vec<MetalWindow>, metal_cx: &MetalCx, cocoa_app: &mut CocoaApp){
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        cocoa_app,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;
                },
                CxOsOp::CloseWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        self.windows[window_id].is_created = false;
                        metal_window.cocoa_window.close_window();
                        break;
                    }
                },
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.minimize();
                    }
                },
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.maximize();
                    }
                },
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.restore();
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
                CxOsOp::ShowTextIME(area, pos) => {
                    let pos = area.get_clipped_rect(self).pos + pos;
                    metal_windows.iter_mut().for_each( | w | {
                        w.cocoa_window.set_ime_spot(pos);
                    });
                },
                CxOsOp::HideTextIME => {
                    //todo!()
                },
                CxOsOp::SetCursor(cursor) => {
                    cocoa_app.set_mouse_cursor(cursor);
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    cocoa_app.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    cocoa_app.stop_timer(timer_id);
                },
                CxOsOp::StartDragging(items) => {
                    cocoa_app.start_dragging(items);
                }
                CxOsOp::UpdateMenu(menu) => {
                    cocoa_app.update_app_menu(&menu, &self.command_settings)
                },
                CxOsOp::HttpRequest{request_id, request} => {
                    cocoa_app.make_http_request(request_id, request, self.os.network_response.sender.clone());
                },
                CxOsOp::ShowClipboardActions(_request) => {
                    crate::log!("Show clipboard actions not supported yet");
                }
                CxOsOp::WebSocketOpen{request_id, request}=>{
                    cocoa_app.web_socket_open(request_id, request, self.os.network_response.sender.clone());
                }
                CxOsOp::WebSocketSendBinary{request_id:_, data:_}=>{
                    todo!()
                }
                CxOsOp::WebSocketSendString{request_id:_, data:_}=>{
                    todo!()
                }
            }
        }
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.live_expand();
        self.start_live_file_watcher();
        self.live_scan_dependencies();
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    /*
    fn web_socket_open(&mut self, _url: String, _rec: WebSocketAutoReconnect) -> WebSocket {
        todo!()
    }
    
    fn web_socket_send(&mut self, _websocket: WebSocket, _data: Vec<u8>) {
        todo!()
    }*/
}


#[derive(Default)]
pub struct CxOs {
    pub (crate) keep_alive_counter: usize,
    pub (crate) media: CxAppleMedia,
    pub (crate) bytes_written: usize,
    pub (crate) draw_calls_done: usize,
    pub (crate) network_response: NetworkResponseChannel,
}

