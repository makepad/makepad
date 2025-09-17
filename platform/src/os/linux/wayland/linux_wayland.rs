//! Main Wayland backend implementation
use std::{rc::Rc, cell::Cell};
use std::cell::RefCell;

use crate::cx_native::EventFlow;
use crate::egl_sys::{EGLSurface, NativeDisplayType};
use crate::opengl_cx::OpenglCx;
use crate::wayland::wayland_app::WaylandApp;
use super::opengl_wayland::WaylandWindow;
use crate::{egl_sys, gl_sys, live_id, makepad_live_id::*, Cx, CxOsOp, CxPassParent, Event, OsType, PassClearColor, PassClearDepth, PassId, SignalToUI, WindowCloseRequestedEvent, WindowClosedEvent, WindowGeomChangeEvent};
use wayland_client::protocol::wl_keyboard;
use wayland_client::{Connection, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::xdg_toplevel;
use super::wayland_state::WaylandState;
use crate::makepad_math::dvec2;
use super::wayland_event::WaylandEvent;

pub fn wayland_event_loop(cx: Rc<RefCell<Cx>>) {
    WaylandCx::event_loop_impl(cx);
}

thread_local! {
    static WAYLAND_APP: RefCell<Option<WaylandApp>> = RefCell::new(None);
}

pub(crate) struct WaylandCx{cx: Rc<RefCell<Cx>>}

impl WaylandCx {
    pub fn event_loop_impl(cx: Rc<RefCell<Cx>>) {
        let wayland_cx = Rc::new(RefCell::new(WaylandCx{cx: cx.clone()}));
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

        let wayland_cx_clone = wayland_cx.clone();
        let mut state = WaylandState::new(Box::new(move |wayland_state, event| {
            wayland_cx_clone.borrow_mut().state_event_callback(wayland_state, event);
        }));
        while !state.available() {
            event_queue.roundtrip(&mut state).unwrap();
        }
        WAYLAND_APP.with(move |cell| {
            let mut app = cell.borrow_mut();
            *app = Some(WaylandApp::new(conn, event_queue, state,
                Box::new(
                    move |wayland_app, event| {
                        wayland_cx.borrow_mut().app_event_callback(wayland_app, event)
                    })));
        });

        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();

        WAYLAND_APP.with(|cell| {
            let mut app = cell.borrow_mut();
            let app =  app.as_mut().unwrap();
            app.start_timer(0, 0.008, true);
            app.event_loop();
        });
    }

    fn state_event_callback(&self, state: &mut WaylandState, event: WaylandEvent) {
        match event {
            WaylandEvent::Toplevel(xdg_toplevel::Event::Close, window_id) => {
                WAYLAND_APP.with(|cell| {
                    let app = cell.as_ptr();
                    unsafe{
                        if let Some(ref mut app) = *app {
                            app.terminate_event_loop();
                        }
                    }
                })
            },
            WaylandEvent::Toplevel(xdg_toplevel::Event::Configure { width, height, states, }, window_id) => {
                // ignore the illegal width and height
                if width <= 0 || height <= 0 {
                    return;
                }
                let mut cx = self.cx.borrow_mut();
                if let Some(window) = state.windows.iter_mut().find( | w | w.window_id == window_id) {
                    let old_geom = window.window_geom.clone();
                    window.window_geom.inner_size.x = width as f64;
                    window.window_geom.inner_size.y = height as f64;
                    window.window_geom.outer_size = window.window_geom.inner_size;
                    if let Some(dpi_override) = cx.windows[window_id].dpi_override {
                        window.window_geom.inner_size *= window.window_geom.dpi_factor / dpi_override;
                        window.window_geom.dpi_factor = dpi_override;
                    }
                    cx.windows[window_id].window_geom = window.window_geom.clone();

                    // redraw just this windows root draw list
                    if window.window_geom.inner_size != old_geom.inner_size {
                        if let Some(main_pass_id) = cx.windows[window.window_id].main_pass_id {
                            cx.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                    let re = Event::WindowGeomChange(WindowGeomChangeEvent{window_id, new_geom: window.window_geom.clone(), old_geom});
                    println!("re: {:?}", re);
                    // ok lets not redraw all, just this window
                    cx.call_event_handler(&re);
                }
            },
            WaylandEvent::Keyboard(wl_keyboard::Event::Enter { serial, surface, keys }) => {
                let mut cx = self.cx.borrow_mut();
                for window in state.windows.iter_mut() {
                    if let Some(main_pass_id) = cx.windows[window.window_id].main_pass_id {
                        cx.repaint_pass(main_pass_id);
                    }
                }
                cx.call_event_handler(&Event::AppGotFocus);
            },
            WaylandEvent::Keyboard(wl_keyboard::Event::Leave { serial, surface }) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::AppLostFocus);
            }
            _ => {}
        }
    }

    fn app_event_callback(&self, wayland_app: &mut WaylandApp, event: WaylandEvent) -> EventFlow{
        // let qhandle = &wayland_app.event_queue.handle();

        if let EventFlow::Exit = self.handle_platform_ops(wayland_app) {
            return EventFlow::Exit
        }

        match event {
            WaylandEvent::Paint => {
                {
                    let mut cx = self.cx.borrow_mut();
                    if cx.new_next_frames.len() != 0 {
                        cx.call_next_frame_event(wayland_app.time_now());
                    }
                    if cx.need_redrawing() {
                        cx.call_draw_event();
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        cx.opengl_compile_shaders();
                    }
                }
                // ok here we send out to all our childprocesses

                self.handle_repaint(&mut wayland_app.state.windows);
            }
            WaylandEvent::Timer(e) => {
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
            _ => {}
        }

        EventFlow::Poll
    }

    fn handle_platform_ops(&self, app: &mut WaylandApp) -> EventFlow {
        let mut ret = EventFlow::Poll;
        let qhandle = &app.event_queue.handle();
        let mut cx = self.cx.borrow_mut();
        if cx.platform_ops.is_empty() {
            return EventFlow::Poll;
        }
        while let Some(op) = cx.platform_ops.pop() {
            println!("handle op: {:?}", op);
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let gl_cx = cx.os.opengl_cx.as_ref().unwrap();
                    let compositor = app.state.compositor.as_ref().unwrap();
                    let wm_base = app.state.wm_base.as_ref().unwrap();
                    let decoration_manager = app.state.decoration_manager.as_ref().unwrap();
                    let window = &cx.windows[window_id];
                    let window = WaylandWindow::new(
                        window_id,
                        compositor,
                        wm_base,
                        decoration_manager,
                        qhandle,
                        gl_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        window.is_fullscreen
                    );
                    app.state.windows.push(window);
                },
                CxOsOp::CloseWindow(window_id) => {
                    cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                    let windows = &mut app.state.windows;
                    if let Some(index) = windows.iter().position( | w | w.window_id == window_id) {
                        cx.windows[window_id].is_created = false;
                        windows[index].close_window();
                        windows.remove(index);
                        if windows.len() == 0 {
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
                },
                CxOsOp::StartTimer {timer_id, interval, repeats} => {
                    app.start_timer(timer_id, interval, repeats);
                },
                CxOsOp::StopTimer(timer_id) => {
                    app.stop_timer(timer_id);
                },
                CxOsOp::ShowTextIME(area, pos) => {
                },
                CxOsOp::HideTextIME => {
                },
                e=>{
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        ret
    }

    pub (crate) fn handle_repaint(&self, windows: &mut Vec<WaylandWindow>) {
        let mut cx = self.cx.borrow_mut();
        cx.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        cx.compute_pass_repaint_order(&mut passes_todo);
        cx.repaint_id += 1;
        for pass_id in &passes_todo {
             let now = WAYLAND_APP.with(|app| {
                let app = app.as_ptr();
                unsafe {
                    (*app).as_ref().unwrap().time_now()
                }
            });
            cx.passes[*pass_id].set_time(now as f32);
            let parent = cx.passes[*pass_id].parent.clone();
            match parent {
                CxPassParent::Xr => {}
                CxPassParent::Window(window_id) => {
                    if let Some(window) = windows.iter_mut().find( | w | w.window_id == window_id) {
                        if window.resize_buffers() {
                            let pix_width = window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                            let pix_height = window.window_geom.inner_size.y * window.window_geom.dpi_factor;

                            cx.draw_pass_to_window(*pass_id, window.egl_surface, pix_width, pix_height);
                            window.wl_egl_surface.resize(window.cal_size.x as i32, window.cal_size.y as i32, 0, 0);
                        }
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
