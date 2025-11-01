use {
    crate::{
        cx::{Cx, OsType}, cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace}, cx_stdin::PollTimers, event::{
            Event, MouseButton, MouseUpEvent, NetworkResponseChannel, WindowGeom
        }, makepad_live_id::*, makepad_math::*, os::{
            apple::{
                apple_classes::init_apple_classes_global, apple_sys::*, macos::{
                    macos_app::{
                        init_macos_app_global, with_macos_app, MacosApp
                    }, macos_event::MacosEvent, macos_window::MacosWindow
                }, url_session::AppleHttpRequests
            }, apple_media::CxAppleMedia, cx_native::EventFlow, metal::{DrawPassMode, MetalCx}, metal_xpc::start_xpc_service
        }, pass::CxPassParent, permission::{Permission}, thread::SignalToUI, window::{CxWindowPool, WindowId}
    }, makepad_objc_sys::{
        msg_send, objc_block, sel, sel_impl
    }, std::{
        cell::RefCell, rc::Rc, time::Instant
    }
};


#[derive(Clone)]
pub struct MetalWindow {
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    cal_size: DVec2,
    pub ca_layer: ObjcId,
    pub cocoa_window: Box<MacosWindow>,
    pub is_resizing: bool
}

impl MetalWindow {
    pub (crate) fn new(
        window_id: WindowId,
        metal_cx: &MetalCx,
        inner_size: DVec2,
        position: Option<DVec2>,
        title: &str,
        is_fullscreen: bool,
    ) -> MetalWindow {
        
        let ca_layer: ObjcId = unsafe {msg_send![class!(CAMetalLayer), new]};
        
        let mut cocoa_window = Box::new(MacosWindow::new(window_id));
        
        cocoa_window.init(title, inner_size, position, is_fullscreen);
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            let () = msg_send![ca_layer, setDisplaySyncEnabled: YES];
            let () = msg_send![ca_layer, setNeedsDisplayOnBoundsChange: YES];
            let () = msg_send![ca_layer, setAutoresizingMask: (1 << 4) | (1 << 1)];
            let () = msg_send![ca_layer, setAllowsNextDrawableTimeout: NO];
            let () = msg_send![ca_layer, setDelegate: cocoa_window.view];
            let () = msg_send![ca_layer, setBackgroundColor: CGColorCreateGenericRGB(0.0, 0.0, 0.0, 1.0)];
            
            let view = cocoa_window.view;
            let () = msg_send![view, setWantsBestResolutionOpenGLSurface: YES];
            let () = msg_send![view, setWantsLayer: YES];
            let () = msg_send![view, setLayerContentsPlacement: 11];
            let () = msg_send![view, setLayer: ca_layer];
        }
        
        MetalWindow {
            is_resizing: false,
            window_id,
            cal_size: DVec2::default(),
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window
        }
    }
    
    pub (crate) fn start_resize(&mut self) {
        self.is_resizing = true;
        let () = unsafe {msg_send![self.ca_layer, setPresentsWithTransaction: YES]};
    }
    
    pub (crate) fn stop_resize(&mut self) {
        self.is_resizing = false;
        let () = unsafe {msg_send![self.ca_layer, setPresentsWithTransaction: NO]};
    }
    
    pub (crate) fn resize_core_animation_layer(&mut self, _metal_cx: &MetalCx) -> bool {
        let cal_size = DVec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            unsafe {
                let () = msg_send![self.ca_layer, setDrawableSize: CGSize {width: cal_size.x, height: cal_size.y}];
                let () = msg_send![self.ca_layer, setContentsScale: self.window_geom.dpi_factor];
            }
            true
        }
        else {
            false
        }
    }
    
}
 

const KEEP_ALIVE_COUNT: usize = 5;

impl Cx {
    
    pub fn event_loop(cx: Rc<RefCell<Cx >>) {
        
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Macos;
        let metal_cx: Rc<RefCell<MetalCx >> = Rc::new(RefCell::new(MetalCx::new()));
        
        // store device object ID for double buffering
        cx.borrow_mut().os.metal_device = Some(metal_cx.borrow().device);
        
        //let cx = Rc::new(RefCell::new(self));
        if std::env::args().find( | v | v == "--stdin-loop").is_some() {
            let mut cx = cx.borrow_mut();
            cx.in_makepad_studio = true;
            let mut metal_cx = metal_cx.borrow_mut();
            return cx.stdin_event_loop(&mut metal_cx);
        }
        
        let metal_windows = Rc::new(RefCell::new(Vec::new()));
        init_macos_app_global(Box::new({
            let cx = cx.clone();
            move | event | {
                let mut cx_ref = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let mut metal_windows = metal_windows.borrow_mut();
                let event_flow = cx_ref.cocoa_event_callback(event, &mut metal_cx, &mut metal_windows);
                let executor = cx_ref.executor.take().unwrap();
                drop(cx_ref);
                executor.run_until_stalled();
                let mut cx_ref = cx.borrow_mut();
                cx_ref.executor = Some(executor);
                event_flow
            }
        }));

        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();
        // Start timer if there's initial work after startup
        if cx.borrow().need_redrawing() {
            cx.borrow_mut().ensure_timer0_started();
        }
        MacosApp::event_loop();
    }
    
    pub (crate) fn handle_repaint(&mut self, metal_windows: &mut Vec<MetalWindow>, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        let time_now = with_macos_app(|app| app.time_now() as f32);
        for pass_id in &passes_todo {
            match self.passes[*pass_id].parent.clone() {
                CxPassParent::Xr => {}
                CxPassParent::Window(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = metal_window.window_geom.dpi_factor;
                        metal_window.resize_core_animation_layer(&metal_cx);
                        let drawable: ObjcId = unsafe {msg_send![metal_window.ca_layer, nextDrawable]};
                        if drawable == nil {
                            return
                        }
                        self.passes[*pass_id].set_time(time_now);
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
                    self.passes[*pass_id].set_time(with_macos_app(|app| app.time_now() as f32));
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::Texture);
                },
                CxPassParent::None => {
                    self.passes[*pass_id].set_time(with_macos_app(|app| app.time_now() as f32));
                    self.draw_pass(*pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }
    
    pub (crate) fn handle_networking_events(&mut self) {
        let mut out = Vec::new();
        while let Ok(item) = self.os.network_response.receiver.try_recv() {
            // remove the request object on error or end
            self.os.http_requests.handle_response_item(&item);
            out.push(item);
        }
        if out.len()>0 {
            self.handle_script_network_events(&out);
            self.call_event_handler(&Event::NetworkResponses(out))
        }
    }

    fn ensure_timer0_started(&mut self) {
        if !self.os.timer0_armed {
            with_macos_app(|app| app.stop_timer(0));
            with_macos_app(|app| app.start_timer(0, 0.008, true));
            self.os.timer0_armed = true;
        }
    }

    fn ensure_timer0_stopped(&mut self) {
        if self.os.timer0_armed {
            with_macos_app(|app| app.stop_timer(0));
            with_macos_app(|app| app.start_timer(0, 0.2, true));
            self.os.timer0_armed = false;
        }
    }

    fn cocoa_event_callback(
        &mut self,
        event: MacosEvent,
        metal_cx: &mut MetalCx,
        metal_windows: &mut Vec<MetalWindow>
    ) -> EventFlow {
        if let  EventFlow::Exit = self.handle_platform_ops(metal_windows, metal_cx){
            self.call_event_handler(&Event::Shutdown);
            return EventFlow::Exit
        }
        // send a mouse up when dragging starts
        match &event {
            MacosEvent::MouseDown(_) |
            MacosEvent::MouseMove(_) |
            MacosEvent::MouseUp(_) |
            MacosEvent::Scroll(_) |
            MacosEvent::KeyDown(_) |
            MacosEvent::KeyUp(_) |
            MacosEvent::TextInput(_) => {
                self.os.keep_alive_counter = KEEP_ALIVE_COUNT;
                self.ensure_timer0_started();
            }
            MacosEvent::Timer(te) => {
                if te.timer_id == 0 {
                    let mut needs_timer = false;

                    if self.screenshot_requests.len()>0{
                        self.repaint_windows();
                        needs_timer = true;
                    }
                    if self.os.keep_alive_counter>0 {
                        self.os.keep_alive_counter -= 1;
                        needs_timer = true;
                    }

                    // check signals
                    if SignalToUI::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.handle_script_signals();
                        self.call_event_handler(&Event::Signal);
                        needs_timer = true;
                    }

                    // Check if we still need the timer
                    if !needs_timer && !self.need_redrawing() && self.new_next_frames.len() == 0 && !self.demo_time_repaint {
                        self.ensure_timer0_stopped();
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        self.handle_action_receiver();
                    }
                    if self.handle_live_edit() {
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();
                    self.cocoa_event_callback(MacosEvent::Paint, metal_cx, metal_windows);

                    // block till the next timer
                    return EventFlow::Wait;
                }
            }
            _ => ()
        }
        //self.process_desktop_pre_event(&mut event);
        match event {
            MacosEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in metal_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                self.call_event_handler(&Event::AppGotFocus);
            }
            MacosEvent::AppLostFocus => {
                self.call_event_handler(&Event::AppLostFocus);
            }
            MacosEvent::WindowResizeLoopStart(window_id) => {
                if let Some(window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                    window.start_resize();
                }
            }
            MacosEvent::WindowResizeLoopStop(window_id) => {
                if let Some(window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                    window.stop_resize();
                }
            }
            MacosEvent::WindowGeomChange(mut re) => { // do this here because mac
                if let Some(window) = metal_windows.iter_mut().find( | w | w.window_id == re.window_id) {
                    self.windows[re.window_id].os_dpi_factor = Some(re.new_geom.dpi_factor);
                    if let Some(dpi_override) = self.windows[re.window_id].dpi_override {
                        re.new_geom.inner_size *= re.new_geom.dpi_factor / dpi_override;
                        re.new_geom.dpi_factor = dpi_override;
                    }
                    window.window_geom = re.new_geom.clone();
                    self.windows[re.window_id].window_geom = re.new_geom.clone();
                    
                    // redraw just this windows root draw list
                    if re.old_geom.dpi_factor != re.new_geom.dpi_factor || re.old_geom.inner_size != re.new_geom.inner_size {
                        if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                            self.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                }
                // ok lets not redraw all, just this window
                self.call_event_handler(&Event::WindowGeomChange(re));
            }
            MacosEvent::WindowClosed(wc) => {
                // lets remove the window from the set
                let window_id = wc.window_id;
                self.call_event_handler(&Event::WindowClosed(wc));
                
                self.windows[window_id].is_created = false;
                if let Some(index) = metal_windows.iter().position( | w | w.window_id == window_id) {
                    metal_windows.remove(index);
                    if metal_windows.len() == 0 {
                        self.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit
                    }
                }
            }
            MacosEvent::Paint => {
                let has_next_frames = self.new_next_frames.len() != 0;
                if has_next_frames {
                    self.call_next_frame_event(with_macos_app(|app| app.time_now()));
                }
                let needs_redrawing = self.need_redrawing();
                if needs_redrawing {
                    self.call_draw_event();
                    self.mtl_compile_shaders(&metal_cx);
                }

                // Start timer if we have work
                if has_next_frames || needs_redrawing || self.screenshot_requests.len() > 0 || self.os.keep_alive_counter > 0 || self.demo_time_repaint {
                    self.ensure_timer0_started();
                }

                // ok here we send out to all our childprocesses
                self.handle_repaint(metal_windows, metal_cx);
                
            }
            MacosEvent::MouseDown(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                self.fingers.mouse_down(e.button, e.window_id);
                self.call_event_handler(&Event::MouseDown(e.into()));
            }
            MacosEvent::MouseMove(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            MacosEvent::MouseUp(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            MacosEvent::Scroll(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::Scroll(e.into()));
            }
            MacosEvent::WindowDragQuery(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            MacosEvent::WindowCloseRequested(e) => {
                self.call_event_handler(&Event::WindowCloseRequested(e))
            }
            MacosEvent::TextInput(e) => {
                self.call_event_handler(&Event::TextInput(e))
            }
            MacosEvent::Drag(e) => {
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            }
            MacosEvent::Drop(e) => {
                self.call_event_handler(&Event::Drop(e));
                self.drag_drop.cycle_drag();
            }
            MacosEvent::DragEnd => {
                // lets send mousebutton ups to fix missing it.
                // TODO! make this more resilient
                self.call_event_handler(&Event::MouseUp(MouseUpEvent {
                    abs: dvec2(-100000.0, -100000.0),
                    button: MouseButton::PRIMARY,
                    window_id: CxWindowPool::id_zero(),
                    modifiers: Default::default(),
                    time: 0.0
                }));
                self.fingers.mouse_up(MouseButton::PRIMARY);
                self.fingers.cycle_hover_area(live_id!(mouse).into());

                self.call_event_handler(&Event::DragEnd);
                self.drag_drop.cycle_drag();
            }
            MacosEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            MacosEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            MacosEvent::TextCopy(e) => {
                self.call_event_handler(&Event::TextCopy(e))
            }
            MacosEvent::TextCut(e) => {
                self.call_event_handler(&Event::TextCut(e))
            }
            MacosEvent::Timer(e) => {
                self.handle_script_timer(&e);
                self.call_event_handler(&Event::Timer(e));
                return EventFlow::Wait;
            }
            MacosEvent::MacosMenuCommand(e) => {
                self.call_event_handler(&Event::MacosMenuCommand(e))
            }
            MacosEvent::PermissionResult(result) => {
                self.call_event_handler(&Event::PermissionResult(result))
            }
        }

        // Determine the event flow based on whether we have work to do
        if self.any_passes_dirty() ||
           self.need_redrawing() ||
           self.new_next_frames.len() != 0 ||
           self.os.keep_alive_counter > 0 ||
           self.screenshot_requests.len() > 0 ||
           self.demo_time_repaint ||
           self.os.timer0_armed {
            // We have work to do or timer is running
            EventFlow::Poll
        } else {
            // No work pending and timer is stopped - we can wait
            EventFlow::Wait
        }
    }
    
    fn dpi_override_scale(&self, pos:&mut DVec2, window_id:WindowId){
        *pos = self.windows[window_id].remap_dpi_override(*pos)
    }
    
    fn handle_platform_ops(&mut self, metal_windows: &mut Vec<MetalWindow>, metal_cx: &MetalCx)->EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            println!("{:?}", op);
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        window.is_fullscreen
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;
                },
                CxOsOp::ResizeWindow(window_id, size) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.set_outer_size(size);
                    }
                }
                CxOsOp::RepositionWindow(window_id, pos ) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.set_position(pos);
                    }
                }
                CxOsOp::CloseWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        self.windows[window_id].is_created = false;
                        metal_window.cocoa_window.close_window();
                        break;
                    }
                },
                CxOsOp::Quit => {
                    return EventFlow::Exit;
                },
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.minimize();
                    }
                },
                CxOsOp::Deminiaturize(window_id) => {
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.deminiaturize();
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
                CxOsOp::HideWindow(window_id) =>{
                    if let Some(metal_window) = metal_windows.iter_mut().find( | w | w.window_id == window_id) {
                        metal_window.cocoa_window.hide();
                    }
                }
                CxOsOp::ShowTextIME(area, pos) => {
                    let pos = area.clipped_rect(self).pos + pos;
                    metal_windows.iter_mut().for_each( | w | {
                        w.cocoa_window.set_ime_spot(pos);
                    });
                },
                CxOsOp::HideTextIME => {
                    metal_windows.iter_mut().for_each( | w | {
                        w.cocoa_window.set_ime_spot(dvec2(0.0,0.0));
                    });
                },
                CxOsOp::SetCursor(cursor) => {
                    with_macos_app(|app| app.set_mouse_cursor(cursor));
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    println!("START TIMER {} {}", timer_id, repeats);
                    with_macos_app(|app| app.start_timer(timer_id, interval, repeats));
                },
                CxOsOp::StopTimer(timer_id) => {
                    with_macos_app(|app| app.stop_timer(timer_id));
                },
                CxOsOp::StartDragging(items) => {
                    //  lets start dragging on the right window
                    if let Some(metal_window) = metal_windows.iter_mut().next() {
                        metal_window.cocoa_window.start_dragging(items);
                        break;
                    }
                }
                CxOsOp::UpdateMacosMenu(menu) => {
                    with_macos_app(|app| app.update_macos_menu(&menu))
                },
                CxOsOp::HttpRequest {request_id, request} => {
                    self.os.http_requests.make_http_request(request_id, request, self.os.network_response.sender.clone());
                },
                CxOsOp::CancelHttpRequest {request_id} => {
                    self.os.http_requests.cancel_http_request(request_id);
                },
                CxOsOp::ShowClipboardActions(_request) => {
                    crate::log!("Show clipboard actions not supported yet");
                },
                CxOsOp::CopyToClipboard(content) => {
                    with_macos_app(|app| app.copy_to_clipboard(&content));
                },
                CxOsOp::SaveFileDialog(settings) => 
                {
                    with_macos_app(|app| app.open_save_file_dialog(settings));
                }
                
                CxOsOp::SelectFileDialog(settings) => 
                {
                    with_macos_app(|app| app.open_select_file_dialog(settings));
                }
                
                CxOsOp::SaveFolderDialog(settings) => 
                {
                    with_macos_app(|app| app.open_save_folder_dialog(settings));
                }
                
                CxOsOp::SelectFolderDialog(settings) => 
                {
                    with_macos_app(|app| app.open_select_folder_dialog(settings));
                }
                CxOsOp::ShowInDock(show) => {
                    with_macos_app(|app| app.show_in_dock(show));
                },
                CxOsOp::CheckPermission {permission, request_id} => {
                    self.handle_permission_check(permission, request_id);
                },
                CxOsOp::RequestPermission {permission, request_id} => {
                    self.handle_permission_request(permission, request_id);
                },
                e=>{
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        EventFlow::Poll
    }

    fn check_audio_permission_status(&self) -> crate::permission::PermissionStatus {
        unsafe {
            let permission_status: i32 = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio];
            match permission_status {
                3 => crate::permission::PermissionStatus::Granted, // AVAuthorizationStatusAuthorized
                2 => crate::permission::PermissionStatus::DeniedPermanent,  // AVAuthorizationStatusDenied - macOS doesn't re-prompt
                1 => crate::permission::PermissionStatus::DeniedPermanent,  // AVAuthorizationStatusRestricted
                _ => crate::permission::PermissionStatus::NotDetermined, // AVAuthorizationStatusNotDetermined (0) or unknown
            }
        }
    }

    fn handle_permission_check(&mut self, permission: Permission, request_id: i32) {
        let status = match permission {
            Permission::AudioInput => self.check_audio_permission_status()
        };
        
        self.call_event_handler(&crate::event::Event::PermissionResult(crate::permission::PermissionResult {
            permission,
            request_id,
            status,
        }));
    }

    fn handle_permission_request(&mut self, permission: Permission, request_id: i32) {
        match permission {
            Permission::AudioInput => {
                let status = self.check_audio_permission_status();
                match status {
                    crate::permission::PermissionStatus::Granted => {
                        // Already granted, don't re-ask
                        self.call_event_handler(&crate::event::Event::PermissionResult(crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status,
                        }));
                    },
                    crate::permission::PermissionStatus::DeniedPermanent => {
                        // Previously denied, send denied event
                        self.call_event_handler(&crate::event::Event::PermissionResult(crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status,
                        }));
                    },
                    crate::permission::PermissionStatus::NotDetermined => {
                        // Need to request permission
                        self.macos_request_audio_permission(permission, request_id);
                    }
                    _ => {
                        // For other statuses, send the result directly
                        self.call_event_handler(&crate::event::Event::PermissionResult(crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status,
                        }));
                    }
                }
            }
        }
    }

    fn macos_request_audio_permission(&mut self, permission: Permission, request_id: i32) {
        unsafe {
            let completion_handler = objc_block!(move |granted: BOOL| {
                let permission_result = crate::permission::PermissionResult {
                    permission,
                    request_id,
                    status: if granted == YES { 
                        crate::permission::PermissionStatus::Granted 
                    } else { 
                        crate::permission::PermissionStatus::DeniedPermanent 
                    },
                };

                // Dispatch callback to main thread
                // AVCaptureDevice completion handlers run on arbitrary background threads
                Self::dispatch_permission_result_to_main_thread(permission_result);
            });
            
            let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType:AVMediaTypeAudio completionHandler:&completion_handler];
        }
    }

    fn dispatch_permission_result_to_main_thread(permission_result: crate::permission::PermissionResult) {
        unsafe {
            let result_clone = permission_result.clone();
            
            // Create a block that will be executed on the main thread
            let main_thread_block = objc_block!(move | | {
                MacosApp::do_callback(MacosEvent::PermissionResult(result_clone.clone()));
            });
            
            // Use NSOperationQueue.mainQueue to dispatch to main thread
            let main_queue: ObjcId = msg_send![class!(NSOperationQueue), mainQueue];
            let block_operation: ObjcId = msg_send![class!(NSBlockOperation), blockOperationWithBlock: &main_thread_block];
            let () = msg_send![main_queue, addOperation: block_operation];
        }
    }
}

impl CxOsApi for Cx {
    fn pre_start() -> bool {
        init_apple_classes_global();
        for arg in std::env::args() {
            if arg == "--metal-xpc" {
                start_xpc_service();
                return true
            }
        }
        false
    }
    
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR"){
            self.live_registry.borrow_mut().package_root = Some(item.to_string());
        }
        self.live_expand();
        #[cfg(debug_assertions)]
        if !Self::has_studio_web_socket() {
            self.start_disk_live_file_watcher(100);
        }
        self.live_scan_dependencies();

        #[cfg(apple_bundle)]
        self.apple_bundle_load_dependencies();
        #[cfg(not(apple_bundle))]
        self.native_load_dependencies();
    }
    
    fn spawn_thread<F>(&mut self, f: F) where F: FnOnce() + Send + 'static {
        std::thread::spawn(f);
    }
    
    fn start_stdin_service(&mut self) {
        self.start_xpc_service()
    }
    
    fn seconds_since_app_start(&self)->f64{
        Instant::now().duration_since(self.os.start_time.unwrap()).as_secs_f64()
    }
    
    fn open_url(&mut self, _url:&str, _in_place:OpenUrlInPlace){
        crate::error!("open_url not implemented on this platform");
    }
    
    fn max_texture_width()->usize{16384}
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
    /// For how long to keep the timer alive when the app is idle
    pub (crate) keep_alive_counter: usize,
    /// Indicates wether the main timer is armed
    pub (crate) timer0_armed: bool,
    pub (crate) media: CxAppleMedia,
    pub (crate) bytes_written: usize,
    pub (crate) draw_calls_done: usize,
    pub (crate) network_response: NetworkResponseChannel,
    pub (crate) stdin_timers: PollTimers,
    pub (crate) start_time: Option<Instant>,
    pub (crate) http_requests: AppleHttpRequests,
    pub metal_device: Option<ObjcId>,
}
