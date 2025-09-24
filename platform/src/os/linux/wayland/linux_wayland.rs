//! Main Wayland backend implementation
use std::rc::Rc;
use std::cell::{Cell, RefCell};

use crate::gl_sys::TEXTURE0;
use crate::wayland::xkb_sys;
use crate::x11::xlib_event::XlibEvent;
use crate::WindowId;
use crate::cx_native::EventFlow;
use crate::egl_sys::NativeDisplayType;
use crate::opengl_cx::OpenglCx;
use crate::wayland::wayland_app::WaylandApp;
use super::opengl_wayland::WaylandWindow;
use crate::{egl_sys, Area, Cx, CxOsOp, CxPassParent, Event, KeyModifiers, MouseMoveEvent, SignalToUI, WindowClosedEvent, WindowGeomChangeEvent};
use crate::makepad_live_id::*;
use wayland_client::protocol::{wl_keyboard, wl_pointer};
use wayland_client::{Connection, Proxy};
use wayland_protocols::xdg::shell::client::xdg_toplevel;
use super::wayland_state::WaylandState;
use crate::makepad_math::dvec2;

pub fn wayland_event_loop(cx: Rc<RefCell<Cx>>) {
    WaylandCx::event_loop_impl(cx);
}

pub(crate) struct WaylandCx{
    cx: Rc<RefCell<Cx>>,
    qhandle: Option<wayland_client::QueueHandle<WaylandState>>,
}

impl WaylandCx {
    pub fn event_loop_impl(cx: Rc<RefCell<Cx>>) {
        let wayland_cx = Rc::new(RefCell::new(WaylandCx{
            cx: cx.clone(), qhandle: None,
        }));
        let conn = Connection::connect_to_env().unwrap();
        let display = conn.display();

        let display_ptr = conn.backend().display_ptr();
        cx.borrow_mut().os.opengl_cx = Some(unsafe {
            OpenglCx::from_egl_platform_display(
                egl_sys::EGL_PLATFORM_WAYLAND_KHR,
                display_ptr as NativeDisplayType
            )
        });

        let mut event_queue = conn.new_event_queue();
        let qhandle = event_queue.handle();
        display.get_registry(&qhandle, ());
        wayland_cx.borrow_mut().qhandle = Some(qhandle);

        let wayland_cx_clone = wayland_cx.clone();
        let mut state = WaylandState::new(Box::new(move |wayland_state, event| {
            if let EventFlow::Exit = wayland_cx_clone.borrow_mut().state_event_callback(wayland_state, event){
                wayland_state.event_loop_running = false;
            }
        }));
        while !state.available() {
            event_queue.roundtrip(&mut state).unwrap();
        }
        let mut app = WaylandApp::new(conn, event_queue, state,
            Box::new(
                move |wayland_app, event| {
                    wayland_cx.borrow_mut().app_event_callback(wayland_app, event)
                }));

        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();

        app.start_timer(0, 0.008, true);
        app.event_loop();
    }

    fn state_event_callback(&mut self, state: &mut WaylandState, event: XlibEvent) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(state) {
            state.event_loop_running = false;
            return EventFlow::Exit
        }

        match event {
            XlibEvent::Paint | XlibEvent::Timer(_) |
            XlibEvent::MouseMove(_) | XlibEvent::WindowDragQuery(_) |
            XlibEvent::WindowGeomChange(_) |
            XlibEvent::MouseDown(_) | XlibEvent::MouseUp(_) |
            XlibEvent::KeyDown(_) | XlibEvent::KeyUp(_) => {
            }
            _ => {
                // println!("event: {:?}", event);
            }
        }
        match event {
            XlibEvent::AppGotFocus => { // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                let mut cx = self.cx.borrow_mut();
                for window in state.windows.iter_mut() {
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
                if let Some(window) = state.windows.iter_mut().find( | w | w.window_id == re.window_id) {
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
                // ok lets not redraw all, just this window
                cx.call_event_handler(&Event::WindowGeomChange(re));
            }
            XlibEvent::WindowClosed(wc) => {
                let mut cx = self.cx.borrow_mut();
                let window_id = wc.window_id;
                cx.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                cx.windows[window_id].is_created = false;
                if let Some(index) = state.windows.iter().position( | w | w.window_id == window_id) {
                    state.windows.remove(index);
                    if state.windows.len() == 0 {
                        cx.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit
                    }
                }
            }
            XlibEvent::Paint => {
                {
                    let mut cx = self.cx.borrow_mut();
                    if cx.new_next_frames.len() != 0 {
                        cx.call_next_frame_event(state.time_now());
                    }
                    if cx.need_redrawing() {
                        cx.call_draw_event();
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        cx.opengl_compile_shaders();
                    }
                }
                // ok here we send out to all our childprocesses

                self.handle_repaint(state);
            }
            XlibEvent::MouseMove(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::MouseMove(e.into()));
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
                cx.fingers.switch_captures();
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
                state.windows.retain_mut(|win| win.window_id != e.window_id);
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
        return EventFlow::Poll;
    }

    fn app_event_callback(&mut self, wayland_app: &mut WaylandApp, event: XlibEvent) -> EventFlow{
        let event_flow = self.state_event_callback(&mut wayland_app.state, event);
        if let EventFlow::Exit = event_flow {
            wayland_app.terminate_event_loop();
        }
        event_flow
    }

    fn handle_platform_ops(&self, state: &mut WaylandState) -> EventFlow {
        let mut ret = EventFlow::Poll;
        let mut cx = self.cx.borrow_mut();
        if cx.platform_ops.is_empty() {
            return EventFlow::Poll;
        }
        while let Some(op) = cx.platform_ops.pop() {
            match op {
                CxOsOp::SetCursor(_) | CxOsOp::StartTimer{..} | CxOsOp::StopTimer(_) => {}
                _ => {
                    //println!("handle op: {:?}", op)
                }
            }
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let gl_cx = cx.os.opengl_cx.as_ref().unwrap();
                    let compositor = state.compositor.as_ref().unwrap();
                    let wm_base = state.wm_base.as_ref().unwrap();
                    let decoration_manager = state.decoration_manager.as_ref().unwrap();
                    let scale_manager = state.scale_manager.as_ref().unwrap();
                    let viewporter = state.viewporter.as_ref().unwrap();
                    let window = &cx.windows[window_id];
                    let window = WaylandWindow::new(
                        window_id,
                        compositor,
                        wm_base,
                        decoration_manager,
                        scale_manager,
                        viewporter,
                        self.qhandle.as_ref().unwrap(),
                        gl_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        window.is_fullscreen
                    );
                    state.windows.push(window);
                },
                CxOsOp::CloseWindow(window_id) => {
                    cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                    let windows = &mut state.windows;
                    if let Some(index) = windows.iter().position( | w | w.window_id == window_id) {
                        cx.windows[window_id].is_created = false;
                        windows[index].close_window();
                        windows.remove(index);
                        if windows.len() == 0 {
                            println!("exit");
                            ret = EventFlow::Exit
                        }
                    }
                },
                CxOsOp::Quit=>{
                    ret = EventFlow::Exit
                }
                CxOsOp::MinimizeWindow(window_id) => {
                },
                CxOsOp::Deminiaturize(_window_id) => todo!(),
                CxOsOp::HideWindow(_window_id) => todo!(),
                CxOsOp::MaximizeWindow(window_id) => {
                },
                CxOsOp::RestoreWindow(window_id) => {
                },
                CxOsOp::ResizeWindow(window_id, size) => {
                },
                CxOsOp::RepositionWindow(window_id, size) => {
                },
                CxOsOp::ShowClipboardActions(_) =>{
                },
                CxOsOp::CopyToClipboard(content) => {
                }
                CxOsOp::SetCursor(cursor) => {
                    if let Some(cursor_shape) = state.cursor_shape.as_ref() {
                        if let Some(serial) = state.pointer_serial.as_ref() {
                            cursor_shape.set_shape(*serial, cursor.into());
                        }
                    }                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    state.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    state.stop_timer(timer_id);
                },
                CxOsOp::ShowTextIME(area, pos) => {
                    if let Some(window) = state.current_window {
                        if let Some(text_input) = state.text_input.as_ref() {
                            text_input.enable();

                            // todo: follow the cursor while input
                            text_input.set_cursor_rectangle(state.last_mouse_pos.x as i32, state.last_mouse_pos.y as i32, 0, 0 );
                            text_input.commit();
                        }
                    }
                },
                CxOsOp::HideTextIME => {
                    if let Some(text_input) = state.text_input.as_ref() {
                        text_input.disable();
                        text_input.commit();
                    }
                },
                e=>{
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        ret
    }

    pub (crate) fn handle_repaint(&self, state: &mut WaylandState) {
        let mut cx = self.cx.borrow_mut();
        cx.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        cx.compute_pass_repaint_order(&mut passes_todo);
        cx.repaint_id += 1;
        for pass_id in &passes_todo {
            let now = state.time_now();
            let windows = &mut state.windows;
            cx.passes[*pass_id].set_time(now as f32);
            let parent = cx.passes[*pass_id].parent.clone();
            match parent {
                CxPassParent::Xr => {}
                CxPassParent::Window(window_id) => {
                    if let Some(window) = windows.iter_mut().find( | w | w.window_id == window_id) {
                        window.resize_buffers();
                        let pix_width = window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height = window.window_geom.inner_size.y * window.window_geom.dpi_factor;

                        cx.draw_pass_to_window(*pass_id, window.egl_surface, pix_width, pix_height);
                        window.wl_egl_surface.resize(pix_width as i32, pix_height as i32, 0, 0);
                        window.viewport.set_source(-1., -1., -1., -1.);
                        window.viewport.set_destination(window.window_geom.inner_size.x as i32, window.window_geom.inner_size.y as i32);
                    }
                }
                CxPassParent::Pass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    cx.draw_pass_to_texture(*pass_id, None);
                },
                CxPassParent::None => {
                    cx.draw_pass_to_texture(*pass_id, None);
                }
            }
        }
    }
}
