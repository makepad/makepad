use {
    self::super::{super::{
        egl_sys,
        x11::{x11_sys, xlib_app::*, xlib_event::*},
    }, opengl_x11::OpenglWindow}, crate::{
        cx::{Cx, LinuxWindowParams, OsType}, cx_api::CxOsOp, event::*, gpu_info::GpuPerformance, makepad_live_id::*, makepad_math::dvec2, opengl_cx::OpenglCx, os::cx_native::EventFlow, pass::CxPassParent, thread::SignalToUI
    }, std::{cell::RefCell, rc::Rc}
};

pub fn x11_event_loop(cx:Rc<RefCell<Cx>>) {
    X11Cx::event_loop_impl(cx)
}

pub struct X11Cx {
    pub cx: Rc<RefCell<Cx>>,
}

impl X11Cx {
    pub fn event_loop_impl(cx:Rc<RefCell<Cx>>) {
        let mut x11_cx = X11Cx { cx: cx.clone() };
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::LinuxWindow(LinuxWindowParams{
            custom_window_chrome: false
        });
        cx.borrow_mut().gpu_info.performance = GpuPerformance::Tier1;

        let opengl_windows = Rc::new(RefCell::new(Vec::new()));
        let is_stdin_loop = std::env::args().find(|v| v=="--stdin-loop").is_some();
        if is_stdin_loop {
            cx.borrow_mut().in_makepad_studio = true;
        }
        init_xlib_app_global(Box::new({
            move | xlib_app,
            events | {
                if is_stdin_loop{
                    return EventFlow::Wait
                }
                let mut opengl_windows = opengl_windows.borrow_mut();
                x11_cx.xlib_event_callback(xlib_app, events, &mut *opengl_windows)
            }
        }));

        cx.borrow_mut().os.opengl_cx = Some(unsafe {
            OpenglCx::from_egl_platform_display(
                egl_sys::EGL_PLATFORM_X11_EXT,
                get_xlib_app_global().display,
            )
        });

        if is_stdin_loop {
            cx.borrow_mut().in_makepad_studio = true;
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

        //let mut paint_dirty = false;

        //self.process_desktop_pre_event(&mut event);
        match event {
            XlibEvent::Paint | XlibEvent::Timer(_)=> {
            }
            XlibEvent::MouseMove(_) | XlibEvent::WindowDragQuery(_)  => {
                return EventFlow::Poll;
            }
            _ => {
                println!("event: {:?}", event);
            }
        }
        match event {
            XlibEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                let mut cx = self.cx.borrow_mut();
                for window in opengl_windows.iter_mut() {
                    if let Some(main_pass_id) = cx.windows[window.window_id].main_pass_id {
                        cx.repaint_pass(main_pass_id);
                    }
                }
                //paint_dirty = true;
                cx.call_event_handler(&Event::AppGotFocus);
            }
            XlibEvent::AppLostFocus => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::AppLostFocus);
            }
            XlibEvent::WindowGeomChange(mut re) => { // do this here because mac
                let mut cx = self.cx.borrow_mut();
                if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == re.window_id) {
                    if let Some(dpi_override) = cx.windows[re.window_id].dpi_override {
                        re.new_geom.inner_size *= re.new_geom.dpi_factor / dpi_override;
                        re.new_geom.dpi_factor = dpi_override;
                    }

                    window.window_geom = re.new_geom.clone();
                    cx.windows[re.window_id].window_geom = re.new_geom.clone();
                    // redraw just this windows root draw list
                    if re.old_geom.inner_size != re.new_geom.inner_size {
                        if let Some(main_pass_id) = cx.windows[re.window_id].main_pass_id {
                            cx.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                }
                println!("re: {:?}", re);
                // ok lets not redraw all, just this window
                cx.call_event_handler(&Event::WindowGeomChange(re));
            }
            XlibEvent::WindowClosed(wc) => {
                let mut cx = self.cx.borrow_mut();
                let window_id = wc.window_id;
                cx.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                cx.windows[window_id].is_created = false;
                if let Some(index) = opengl_windows.iter().position( | w | w.window_id == window_id) {
                    opengl_windows.remove(index);
                    if opengl_windows.len() == 0 {
                        xlib_app.terminate_event_loop();
                        cx.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit
                    }
                }
            }
            XlibEvent::Paint => {
                {
                    let mut cx = self.cx.borrow_mut();
                    if cx.new_next_frames.len() != 0 {
                        cx.call_next_frame_event(xlib_app.time_now());
                    }
                    if cx.need_redrawing() {
                        cx.call_draw_event();
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        cx.opengl_compile_shaders();
                    }
                }
                // ok here we send out to all our childprocesses

                self.handle_repaint(opengl_windows);
            }
            XlibEvent::MouseDown(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.fingers.process_tap_count(
                    e.abs,
                    e.time
                );
                cx.fingers.mouse_down(e.button, e.window_id);
                cx.call_event_handler(&Event::MouseDown(e.into()))
            }
            XlibEvent::MouseMove(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::MouseMove(e.into()));
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
                cx.fingers.switch_captures();
            }
            XlibEvent::MouseUp(e) => {
                let mut cx = self.cx.borrow_mut();
                let button = e.button;
                cx.call_event_handler(&Event::MouseUp(e.into()));
                cx.fingers.mouse_up(button);
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            XlibEvent::Scroll(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::Scroll(e.into()))
            }
            XlibEvent::WindowDragQuery(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowDragQuery(e))
            }
            XlibEvent::WindowCloseRequested(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowCloseRequested(e))
            }
            XlibEvent::TextInput(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::TextInput(e))
            }
            XlibEvent::Drag(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::Drag(e))
            }
            XlibEvent::Drop(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::Drop(e))
            }
            XlibEvent::DragEnd => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::DragEnd)
            }
            XlibEvent::KeyDown(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.keyboard.process_key_down(e.clone());
                cx.call_event_handler(&Event::KeyDown(e))
            }
            XlibEvent::KeyUp(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.keyboard.process_key_up(e.clone());
                cx.call_event_handler(&Event::KeyUp(e))
            }
            XlibEvent::TextCopy(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::TextCopy(e))
            }
            XlibEvent::TextCut(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::TextCut(e))
            }
            XlibEvent::Timer(e) => {
                let mut cx = self.cx.borrow_mut();
                //println!("TIMER! {:?}", std::time::Instant::now());
                if e.timer_id == 0{
                    if SignalToUI::check_and_clear_ui_signal(){
                        cx.handle_media_signals();
                        cx.call_event_handler(&Event::Signal);
                    }
                    cx.handle_action_receiver();
                }
                else{
                    cx.call_event_handler(&Event::Timer(e))
                }

                if cx.handle_live_edit() {
                    cx.call_event_handler(&Event::LiveEdit);
                    cx.redraw_all();
                }
                return EventFlow::Wait;
            }
        }

        //if self.any_passes_dirty() || self.need_redrawing() || paint_dirty {
            EventFlow::Poll
        //} else {
        //    EventFlow::Wait
        // }

    }

    pub (crate) fn handle_repaint(&mut self, opengl_windows: &mut Vec<OpenglWindow>) {
        let mut passes_todo = Vec::new();
        {
            let mut cx = self.cx.borrow_mut();
            cx.os.opengl_cx.as_ref().unwrap().make_current();
            cx.compute_pass_repaint_order(&mut passes_todo);
            cx.repaint_id += 1;
        }
        for pass_id in &passes_todo {
            let parent = {
                let mut cx = self.cx.borrow_mut();
                cx.passes[*pass_id].set_time(get_xlib_app_global().time_now() as f32);
                cx.passes[*pass_id].parent.clone()
            };
            match parent {
                CxPassParent::Xr => {}
                CxPassParent::Window(window_id) => {
                    if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                        //let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers();

                        let egl_surface = window.egl_surface;

                        let pix_width = window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height = window.window_geom.inner_size.y * window.window_geom.dpi_factor;
                        let mut cx = self.cx.borrow_mut();
                        cx.draw_pass_to_window(*pass_id, egl_surface, pix_width, pix_height);
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    let mut cx = self.cx.borrow_mut();
                    cx.draw_pass_to_texture(*pass_id, None);
                },
                CxPassParent::None => {
                    let mut cx = self.cx.borrow_mut();
                    cx.draw_pass_to_texture(*pass_id, None);
                }
            }
        }
    }

    fn handle_platform_ops(&mut self, opengl_windows: &mut Vec<OpenglWindow>, xlib_app: &mut XlibApp) -> EventFlow {
            let mut ret = EventFlow::Poll;
            let mut cx = self.cx.borrow_mut();
            while let Some(op) = cx.platform_ops.pop() {
                println!("handle op: {:?}", op);
                match op {
                    CxOsOp::CreateWindow(window_id) => {
                        let gl_cx = cx.os.opengl_cx.as_ref().unwrap();
                        let window = &cx.windows[window_id];
                        let opengl_window = OpenglWindow::new(
                            window_id,
                            gl_cx,
                            window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                            window.create_position,
                            &window.create_title,
                            window.is_fullscreen
                        );
                        let window = &mut cx.windows[window_id];
                        window.window_geom = opengl_window.window_geom.clone();
                        opengl_windows.push(opengl_window);
                        window.is_created = true;
                    },
                    CxOsOp::CloseWindow(window_id) => {
                        cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                        if let Some(index) = opengl_windows.iter().position( | w | w.window_id == window_id) {
                            cx.windows[window_id].is_created = false;
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
                    CxOsOp::Deminiaturize(_window_id) => todo!(),
                    CxOsOp::HideWindow(_window_id) => todo!(),
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
                    CxOsOp::ResizeWindow(window_id, size) => {
                        if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                            window.xlib_window.set_inner_size(size);
                        }
                    },
                    CxOsOp::RepositionWindow(window_id, size) => {
                        if let Some(window) = opengl_windows.iter_mut().find( | w | w.window_id == window_id) {
                            window.xlib_window.set_position(size);
                        }
                    },
                    CxOsOp::ShowClipboardActions(_) =>{
                    },
                    CxOsOp::CopyToClipboard(content) => {
                        if let Some(window) = opengl_windows.get(0) {
                            unsafe {
                                xlib_app.copy_to_clipboard(&content, window.xlib_window.window.unwrap(), x11_sys::CurrentTime as u64)
                            }
                        }
                    }
                    CxOsOp::SetCursor(cursor) => {
                        xlib_app.set_mouse_cursor(cursor);
                    },
                    CxOsOp::StartTimer {timer_id, interval, repeats} => {
                        xlib_app.start_timer(timer_id, interval, repeats);
                    },
                    CxOsOp::StopTimer(timer_id) => {
                        xlib_app.stop_timer(timer_id);
                    },
                    CxOsOp::ShowTextIME(area, pos) => {
                        let pos = area.clipped_rect(&self.cx.borrow()).pos + pos;
                        opengl_windows.iter_mut().for_each(|w| {
                            w.xlib_window.set_ime_spot(pos);
                        });
                    },
                    CxOsOp::HideTextIME => {
                        opengl_windows.iter_mut().for_each(|w| {
                            w.xlib_window.set_ime_spot(dvec2(0.0,0.0));
                        });
                    },
                    e=>{
                        crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                    }
                }
            }
            ret
        }
}
