// makepad is win10 only because of dx12 + terminal API
use {
    std::{
        rc::Rc,
        cell::{RefCell},
    },
    crate::{
        makepad_live_id::*,
        cx::*,
        event::*,
        os::{
            mswindows::win32_event::*,
            mswindows::d3d11::{D3d11Window, D3d11Cx},
            mswindows::win32_app::*,
            cx_desktop::EventFlow,
        },
        cx_api::{CxOsApi, CxOsOp},
    }
};

impl Cx {

    pub fn event_loop(mut self){
        
        self.platform_type = OsType::Windows;
        let d3d11_cx = Rc::new(RefCell::new(D3d11Cx::new()));
        let cx = Rc::new(RefCell::new(self));
                
        let mut win32_app = Win32App::new();
        
        win32_app.init();
        
        let mut d3d11_windows =  Rc::new(RefCell::new(Vec::new()));
        
        init_win32_app_global(Box::new({
            let cx = cx.clone();
            move | cocoa_app,
            events | {
                let mut cx = cx.borrow_mut();
                let mut d3d11_cx = d3d11_cx.borrow_mut();
                let mut d3d11_windows = d3d11_windows.borrow_mut();
                cx.win32_event_callback(cocoa_app, events, &mut d3d11_cx, &mut d3d11_windows)
            }
        }));
        
        get_win32_app_global().event_loop();
    }
    
    fn win32_event_callback(
        &mut self,
        win32_app: &mut Win32App,
        events: Vec<Win32Event>,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>
    ) -> EventFlow {
         self.handle_platform_ops(d3d11_windows, d3d11_cx, win32_app);
        
        let mut paint_dirty = false;
        for event in events {
            
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

                    self.call_event_handler(&Event::WindowClosed(wc));
                    // lets remove the window from the set
                    self.windows[wc.window_id].is_created = false;
                    if let Some(index) = d3d11_windows.iter().position( | w | w.window_id == wc.window_id) {
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
                    
                    self.handle_repaint(d3d11_cx, d3d11_cx);
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
                    self.fingers.move_captures();
                }
                Win32Event::MouseUp(e) => {
                    let button = e.button;
                    self.call_event_handler(&Event::MouseUp(e.into()));
                    self.fingers.mouse_up(button);
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
                Win32Event::Timer(e) => {
                    self.call_event_handler(&Event::Timer(e))
                }
                Win32Event::Signal(se) => {
                    //println!("SIGNAL!");
                    //self.handle_core_midi_signals(&se);
                    self.call_event_handler(&Event::Signal(se));
                }
                Win32Event::MenuCommand(e) => {
                    self.call_event_handler(&Event::MenuCommand(e))
                }
            }
        }
        
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
        
    }


    fn handle_platform_ops(&mut self, metal_windows: &mut Vec<D3d11Window>, metal_cx: &D3d11Cx, win32_app: &mut CocoaApp) {
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
                CxOsOp::StartDragging(dragged_item) => {
                    cocoa_app.start_dragging(dragged_item);
                }
                CxOsOp::UpdateMenu(menu) => {
                    cocoa_app.update_app_menu(&menu, &self.command_settings)
                }
            }
        }
    }
    
    fn win32_event_callback(
        &mut self,
        win32_app: &mut Win32App,
        events: Vec<Win32Event>,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>
    ) -> bool {
        
        let d3d11_cx = D3d11Cx::new();
        
        self.platform.d3d11_cx = Some(&d3d11_cx);
        
        self.hlsl_compile_all_shaders(&d3d11_cx);
         
        self.load_all_fonts();
        
        self.call_event_handler(&mut Event::Construct);
        
        self.redraw_child_area(Area::All);
        let mut passes_todo = Vec::new();
        
        win32_app.event_loop( | win32_app, events | { //if let Ok(d3d11_cx) = d3d11_cx.lock(){
            // acquire d3d11_cx exclusive
            for mut event in events {
                
                self.process_desktop_pre_event(&mut event);
                match &event {
                    Event::WindowSetHoverCursor(mc) => {
                        self.set_hover_mouse_cursor(mc.clone());
                    },
                    Event::WindowResizeLoop(wr) => {
                        for d3d11_window in &mut d3d11_windows {
                            if d3d11_window.window_id == wr.window_id {
                                if wr.was_started {
                                    d3d11_window.start_resize();
                                }
                                else {
                                    d3d11_window.stop_resize();
                                }
                            }
                        }
                    },
                    Event::WindowGeomChange(re) => { // do this here because mac
                        for d3d11_window in &mut d3d11_windows {
                            if d3d11_window.window_id == re.window_id {
                                d3d11_window.window_geom = re.new_geom.clone();
                                self.windows[re.window_id].window_geom = re.new_geom.clone();
                                // redraw just this windows root draw list
                                //if re.old_geom.inner_size != re.new_geom.inner_size{
                                    if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                                        self.redraw_pass_and_sub_passes(main_pass_id);
                                    }
                                //}
                                break;
                            }
                        }
                        // ok lets not redraw all, just this window
                        self.call_event_handler(&mut event);
                    },
                    Event::WindowClosed(wc) => { // do this here because mac
                        // lets remove the window from the set
                        self.windows[wc.window_id].window_state = CxWindowState::Closed;
                        self.windows_free.push(wc.window_id);
                        // remove the d3d11/win32 window
                        
                        for index in 0..d3d11_windows.len() {
                            if d3d11_windows[index].window_id == wc.window_id {
                                d3d11_windows.remove(index);
                                if d3d11_windows.len() == 0 {
                                    win32_app.terminate_event_loop();
                                }
                                for d3d11_window in &mut d3d11_windows {
                                    d3d11_window.win32_window.update_ptrs();
                                }
                            }
                        }
                        self.call_event_handler(&mut event);
                    },
                    Event::Paint => {
                        self.repaint_id += 1;
                        let vsync = self.process_desktop_paint_callbacks(win32_app.time_now());
                        
                        // construct or destruct windows
                        for (index, window) in self.windows.iter_mut().enumerate() {
                            
                            window.window_state = match &window.window_state {
                                CxWindowState::Create {inner_size, position, title} => {
                                    // lets create a platformwindow
                                    let d3d11_window = D3d11Window::new(index, &d3d11_cx, win32_app, *inner_size, *position, &title);
                                    window.window_geom = d3d11_window.window_geom.clone();
                                    d3d11_windows.push(d3d11_window);
                                    for d3d11_window in &mut d3d11_windows {
                                        d3d11_window.win32_window.update_ptrs();
                                    }
                                    CxWindowState::Created
                                },
                                CxWindowState::Close => {
                                    // ok we close the window
                                    // lets send it a WM_CLOSE event
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.close_window();
                                        if win32_app.event_loop_running == false{
                                            return false;
                                        }
                                        break;
                                    }}
                                    CxWindowState::Closed
                                },
                                CxWindowState::Created => CxWindowState::Created,
                                CxWindowState::Closed => CxWindowState::Closed
                            };
                            
                            if let Some(set_position) = window.window_set_position {
                                for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                    d3d11_window.win32_window.set_position(set_position);
                                }}
                            }
                            
                            window.window_command = match &window.window_command {
                                CxWindowCmd::Restore => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.restore();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Maximize => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.maximize();
                                    }}
                                    CxWindowCmd::None
                                },
                                CxWindowCmd::Minimize => {
                                    for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                        d3d11_window.win32_window.minimize();
                                    }}
                                    CxWindowCmd::None
                                },
                                _ => CxWindowCmd::None,
                            };
                            
                            
                            window.window_set_position = None;
                            
                            if let Some(topmost) = window.window_topmost {
                                for d3d11_window in &mut d3d11_windows {if d3d11_window.window_id == index {
                                    d3d11_window.win32_window.set_topmost(topmost);
                                }}
                            }
                        }
                        
                        // set a cursor
                        if !self.down_mouse_cursor.is_none() {
                            win32_app.set_mouse_cursor(self.down_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else if !self.hover_mouse_cursor.is_none() {
                            win32_app.set_mouse_cursor(self.hover_mouse_cursor.as_ref().unwrap().clone())
                        }
                        else {
                            win32_app.set_mouse_cursor(MouseCursor::Default)
                        }
                        
                        if let Some(set_ime_position) = self.platform.set_ime_position {
                            self.platform.set_ime_position = None;
                            for d3d11_window in &mut d3d11_windows {
                                d3d11_window.win32_window.set_ime_spot(set_ime_position);
                            }
                        }
                        
                        while self.platform.start_timer.len() > 0 {
                            let (timer_id, interval, repeats) = self.platform.start_timer.pop().unwrap();
                            win32_app.start_timer(timer_id, interval, repeats);
                        }
                        
                        while self.platform.stop_timer.len() > 0 {
                            let timer_id = self.platform.stop_timer.pop().unwrap();
                            win32_app.stop_timer(timer_id);
                        }
                        
                        // build a list of renderpasses to repaint
                        let mut windows_need_repaint = 0;
                        self.compute_passes_to_repaint(&mut passes_todo, &mut windows_need_repaint);
                        
                        if passes_todo.len() > 0 {
                            for pass_id in &passes_todo {
                                match self.passes[*pass_id].dep_of.clone() {
                                    CxPassDepOf::Window(window_id) => {
                                        // find the accompanying render window
                                        if let Some(d3d11_window) = d3d11_windows.iter_mut().find( | w | w.window_id == window_id) {
                                            windows_need_repaint -= 1;
                                            
                                            let dpi_factor = d3d11_window.window_geom.dpi_factor;
                                            self.passes[*pass_id].set_dpi_factor(dpi_factor);
                                            
                                            d3d11_window.resize_buffers(&d3d11_cx);
                                            
                                            self.draw_pass_to_window(
                                                *pass_id,
                                                vsync,
                                                dpi_factor,
                                                d3d11_window,
                                                &d3d11_cx,
                                            );
                                            // call redraw if we guessed the dpi wrong on startup
                                            if d3d11_window.first_draw{
                                                d3d11_window.first_draw = false;
                                                if dpi_factor != self.default_dpi_factor{
                                                    self.redraw_pass_and_sub_passes(*pass_id);
                                                }
                                            }
                                        }
                                    }
                                    CxPassDepOf::Pass(parent_pass_id) => {
                                        let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            dpi_factor,
                                            &d3d11_cx,
                                        );
                                    },
                                    CxPassDepOf::None => {
                                        self.draw_pass_to_texture(
                                            *pass_id,
                                            1.0,
                                            &d3d11_cx,
                                        );
                                    }
                                }
                            }
                        }
                    },
                    Event::None => {
                    },
                    Event::Signal{..}=>{
                        self.call_event_handler(&mut event);
                        self.call_signals_and_triggers();
                    },
                    _ => {
                        self.call_event_handler(&mut event);
                    }
                }
                self.process_desktop_post_event(event);
            }

            if self.live_styles.changed_live_bodies.len()>0 || self.live_styles.changed_deps.len()>0{
                let changed_live_bodies = self.live_styles.changed_live_bodies.clone();
                let mut errors = self.process_live_styles_changes();
                self.hlsl_update_all_shaders(&d3d11_cx, &mut errors);
                self.call_live_recompile_event(changed_live_bodies, errors);
            }
            
            self.process_live_style_errors();

            
            if self.playing_animator_ids.len() != 0
                || self.redraw_parent_areas.len() != 0
                || self.redraw_child_areas.len() != 0
                || self.next_frames.len() != 0 {
                false
            } else {
                true
            }
        })
    }
    
    pub fn show_text_ime(&mut self, x: f32, y: f32) { 
        self.platform.set_ime_position = Some(Vec2 {x: x, y: y});
    }
    
    pub fn hide_text_ime(&mut self) {
    }
    
    pub fn start_timer(&mut self, interval: f64, repeats: bool) -> Timer {
        self.timer_id += 1;
        self.platform.start_timer.push((self.timer_id, interval, repeats));
        Timer {timer_id: self.timer_id}
    }
    
    pub fn stop_timer(&mut self, timer: &mut Timer) {
        if timer.timer_id != 0 {
            self.platform.stop_timer.push(timer.timer_id);
            timer.timer_id = 0;
        }
    }

    pub fn post_signal(signal: Signal, status: StatusId) {
        Win32App::post_signal(signal, status);
    }

    pub fn update_menu(&mut self, _menu:&Menu){
    }
}

#[derive(Default)]
pub struct CxOs {
}
  
