#![allow(unused_imports, unused_variables)]
//! Main Wayland backend implementation
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::opengl_wayland::{WaylandPopupWindow, WaylandWindow};
use super::wayland_state::WaylandState;
use crate::cx_native::EventFlow;
use crate::egl_sys::NativeDisplayType;
use crate::gl_sys::TEXTURE0;
use crate::makepad_live_id::*;
use crate::makepad_math::{dvec2, Rect, Vec2d};
use crate::opengl_cx::OpenglCx;
use crate::os::linux::gstreamer_sys::LibGStreamer;
use crate::os::linux::linux_video_playback::{
    poll_pending_gstreamer_teardowns, GStreamerVideoPlayer,
};
use crate::os::linux::linux_video_player::{
    collect_linux_video_player_events, prepare_desktop_linux_video, LinuxPrepareResult,
    LinuxVideoPlayer,
};
use crate::os::linux::v4l2_camera_player::V4l2CameraPlayer;
use crate::wayland::wayland_app::WaylandApp;
use crate::wayland::xkb_sys;
use crate::x11::xlib_event::XlibEvent;
use crate::WindowId;
use crate::{
    cx::{LinuxWindowParams, OsType},
    egl_sys,
    event::{
        video_playback::{
            VideoDecodingErrorEvent, VideoPlaybackResourcesReleasedEvent, VideoSource,
            VideoYuvTexturesReady,
        },
        PopupDismissReason, PopupDismissedEvent,
    },
    gpu_info::GpuPerformance,
    texture::TextureFormat,
    Area, Cx, CxDrawPassParent, CxOsOp, CxWindowPool, Event, KeyModifiers, MouseButton,
    MouseMoveEvent, MouseUpEvent, SignalToUI, WindowClosedEvent, WindowGeomChangeEvent,
};
use wayland_client::protocol::{wl_keyboard, wl_pointer};
use wayland_client::{Connection, Proxy};
use wayland_protocols::xdg::shell::client::xdg_toplevel;

fn log_linux_backdrop_unsupported_once() {
    static LOG_ONCE: std::sync::Once = std::sync::Once::new();
    LOG_ONCE.call_once(|| {
        crate::log!("Window backdrop requested on Linux/Wayland; compositor backdrop blur is not supported in M1 (no-op).");
    });
}

pub fn wayland_event_loop(cx: Rc<RefCell<Cx>>) {
    WaylandCx::event_loop_impl(cx);
}

pub(crate) struct WaylandCx {
    cx: Rc<RefCell<Cx>>,
    qhandle: Option<wayland_client::QueueHandle<WaylandState>>,
    /// Pace presents with `wl_surface::frame` callbacks: a window is only presented
    /// once the callback for its previous frame has fired. The EGL swap interval is 0
    /// on Wayland (see `OpenglCx::swap_interval`), so this is what caps the render
    /// loop at the display rate, and it keeps redraws of occluded windows (whose
    /// callbacks the compositor withholds) from wedging the event loop. Disabled by
    /// `MAKEPAD_NO_VSYNC` for uncapped benchmarking.
    frame_pacing: bool,
}

impl WaylandCx {
    pub fn event_loop_impl(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::LinuxWindow(LinuxWindowParams {
            custom_window_chrome: true,
        });
        cx.borrow_mut().gpu_info.performance = GpuPerformance::Tier1;
        cx.borrow_mut().set_physical_keyboard_state(true);

        let wayland_cx = Rc::new(RefCell::new(WaylandCx {
            cx: cx.clone(),
            qhandle: None,
            frame_pacing: std::env::var_os("MAKEPAD_NO_VSYNC").is_none(),
        }));
        let conn = Connection::connect_to_env().unwrap();
        let display = conn.display();

        let display_ptr = conn.backend().display_ptr();
        cx.borrow_mut().os.opengl_cx = Some(unsafe {
            OpenglCx::from_egl_platform_display(
                egl_sys::EGL_PLATFORM_WAYLAND_KHR,
                display_ptr as NativeDisplayType,
            )
        });

        if crate::app_main::should_run_stdin_loop_from_env() {
            cx.borrow_mut().in_makepad_studio = true;
            return cx.borrow_mut().stdin_event_loop();
        }

        let mut event_queue = conn.new_event_queue();
        let qhandle = event_queue.handle();
        display.get_registry(&qhandle, ());
        wayland_cx.borrow_mut().qhandle = Some(qhandle);

        let wayland_cx_clone = wayland_cx.clone();
        let mut state = WaylandState::new(Box::new(move |wayland_state, event| {
            if let EventFlow::Exit = wayland_cx_clone
                .borrow_mut()
                .state_event_callback(wayland_state, event)
            {
                wayland_state.event_loop_running = false;
            }
        }));
        while !state.available() {
            event_queue.roundtrip(&mut state).unwrap();
        }
        let mut app = WaylandApp::new(
            conn,
            event_queue,
            state,
            Box::new(move |wayland_app, event| {
                wayland_cx
                    .borrow_mut()
                    .app_event_callback(wayland_app, event)
            }),
        );

        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();

        app.start_timer(0, 0.008, true);
        app.event_loop();
    }

    fn state_event_callback(&mut self, state: &mut WaylandState, event: XlibEvent) -> EventFlow {
        state.pump_pending_clipboard_read();
        if let Some(input) = state.take_pending_paste_text_input() {
            let mut cx = self.cx.borrow_mut();
            cx.call_event_handler(&Event::TextInput(crate::TextInputEvent {
                input,
                replace_last: false,
                was_paste: true,
                ..Default::default()
            }));
        }
        if let EventFlow::Exit = self.handle_platform_ops(state) {
            let mut cx = self.cx.borrow_mut();
            cx.call_event_handler(&Event::Shutdown);
            state.event_loop_running = false;
            return EventFlow::Exit;
        }

        match event {
            XlibEvent::Paint
            | XlibEvent::Timer(_)
            | XlibEvent::MouseMove(_)
            | XlibEvent::WindowDragQuery(_)
            | XlibEvent::WindowGeomChange(_)
            | XlibEvent::MouseDown(_)
            | XlibEvent::MouseUp(_)
            | XlibEvent::KeyDown(_)
            | XlibEvent::KeyUp(_) => {}
            _ => {
                // println!("event: {:?}", event);
            }
        }
        match event {
            XlibEvent::WindowGotFocus(window_id) => {
                // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                let mut cx = self.cx.borrow_mut();
                for window in state.windows.iter_mut() {
                    if let Some(main_pass_id) = cx.windows[window.window_id].main_pass_id {
                        cx.repaint_pass(main_pass_id);
                    }
                }
                for popup in state.popups.iter_mut() {
                    if let Some(main_pass_id) = cx.windows[popup.window_id].main_pass_id {
                        cx.repaint_pass(main_pass_id);
                    }
                }
                cx.call_event_handler(&Event::WindowGotFocus(window_id));
            }
            XlibEvent::WindowLostFocus(window_id) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowLostFocus(window_id));
            }
            XlibEvent::WindowGeomChange(mut re) => {
                // do this here because mac
                let mut cx = self.cx.borrow_mut();

                // When drawing our own window chrome (no server-side decorations),
                // populate the chrome buttons bounding box: three buttons right-aligned
                // at the top of the caption bar, matching the Makepad widget layout.
                if matches!(
                    cx.os_type(),
                    OsType::LinuxWindow(LinuxWindowParams {
                        custom_window_chrome: true,
                        ..
                    })
                ) {
                    const BUTTONS_W: f64 = 46.0 * 3.0;
                    const BUTTONS_H: f64 = 29.0;
                    let w = re.new_geom.inner_size.x;
                    re.new_geom.window_chrome_buttons = Rect {
                        pos: Vec2d {
                            x: w - BUTTONS_W,
                            y: 0.0,
                        },
                        size: Vec2d {
                            x: BUTTONS_W,
                            y: BUTTONS_H,
                        },
                    };
                }

                if let Some(window) = state
                    .windows
                    .iter_mut()
                    .find(|w| w.window_id == re.window_id)
                {
                    // compare in native units, before new_geom is converted below
                    let geom_changed = re.old_geom.inner_size != re.new_geom.inner_size
                        || re.old_geom.dpi_factor != re.new_geom.dpi_factor;

                    // Keep the wayland geom native (buffer/viewport size + the next resize's dpi come
                    // from it). Store the zoomed geom here and the next resize reads its dpi back as
                    // "native", so the zoom drifts/resets and the window flickers on maximize. Only
                    // the Cx window gets the zoomed geom.
                    window.window_geom = re.new_geom.clone();
                    {
                        let cx_window = &mut cx.windows[re.window_id];
                        cx_window.os_dpi_factor = Some(re.new_geom.dpi_factor);
                        re.new_geom = cx_window.native_window_geom_to_layout(re.new_geom);
                    }
                    cx.windows[re.window_id].window_geom = re.new_geom.clone();
                    // redraw when the size or scale changed
                    if geom_changed {
                        if let Some(main_pass_id) = cx.windows[re.window_id].main_pass_id {
                            cx.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                } else if let Some(window) = state
                    .popups
                    .iter_mut()
                    .find(|w| w.window_id == re.window_id)
                {
                    let geom_changed = re.old_geom.inner_size != re.new_geom.inner_size
                        || re.old_geom.dpi_factor != re.new_geom.dpi_factor;
                    // same deal — keep the wayland geom native
                    window.window_geom = re.new_geom.clone();
                    {
                        let cx_window = &mut cx.windows[re.window_id];
                        cx_window.os_dpi_factor = Some(re.new_geom.dpi_factor);
                        re.new_geom = cx_window.native_window_geom_to_layout(re.new_geom);
                    }
                    cx.windows[re.window_id].window_geom = re.new_geom.clone();
                    if geom_changed {
                        if let Some(main_pass_id) = cx.windows[re.window_id].main_pass_id {
                            cx.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                }
                // ok lets not redraw all, just this window
                cx.call_event_handler(&Event::WindowGeomChange(re));
            }
            XlibEvent::WindowClosed(wc) => {
                if let EventFlow::Exit = self.handle_window_closed(state, wc) {
                    let mut cx = self.cx.borrow_mut();
                    cx.call_event_handler(&Event::Shutdown);
                    return EventFlow::Exit;
                }
            }
            XlibEvent::PopupDismissed(event) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::PopupDismissed(event));
            }
            XlibEvent::Paint => {
                {
                    let mut cx = self.cx.borrow_mut();
                    let time_now = state.time_now();
                    if cx.new_next_frames.len() != 0 {
                        cx.call_next_frame_event(time_now);
                    }
                    if cx.need_redrawing() {
                        cx.call_draw_event(time_now);
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        cx.opengl_compile_shaders();
                    }
                }
                // ok here we send out to all our childprocesses

                self.handle_repaint(state);

                {
                    let cx = self.cx.borrow();
                    let has_platform_ops = !cx.platform_ops.is_empty();
                    drop(cx);
                    if has_platform_ops {
                        if let EventFlow::Exit = self.handle_platform_ops(state) {
                            let mut cx = self.cx.borrow_mut();
                            cx.call_event_handler(&Event::Shutdown);
                            state.event_loop_running = false;
                            return EventFlow::Exit;
                        }
                    }
                }

                // Run script-VM garbage collection at a safe point after paint, matching
                // the macOS backend. Without this the script object heap grows without
                // bound: every `eval` / `script_apply_eval!` allocates script objects
                // that are only reclaimed by `gc()`. `needs_gc()` gates the actual sweep.
                {
                    let mut cx = self.cx.borrow_mut();
                    cx.with_vm(|vm| {
                        if vm.heap().needs_gc() {
                            vm.gc();
                        }
                    });
                }

                // With the swap interval at 0 nothing in the paint path blocks, so after
                // a paint keep polling only while presenting can make progress. Once a
                // frame callback is in flight, further presents are gated on it and
                // poll-spinning would just burn CPU re-running next-frame/draw events;
                // wait instead. The callback (like any other socket event or timer)
                // wakes the select loop, which dispatches it and paints again.
                let cx = self.cx.borrow();
                let has_work = cx.any_passes_dirty()
                    || cx.need_redrawing()
                    || cx.new_next_frames.len() != 0
                    || cx.screenshot_requests.len() > 0
                    || cx.demo_time_repaint;
                return if has_work && !state.any_frame_callback_pending() {
                    EventFlow::Poll
                } else {
                    EventFlow::Wait
                };
            }
            XlibEvent::MouseMove(mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, e.window_id);
                cx.call_event_handler(&Event::MouseMove(e.into()));
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
                cx.fingers.switch_captures();
            }
            XlibEvent::MouseDown(mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, e.window_id);
                cx.fingers.process_tap_count(e.abs, e.time);
                cx.fingers.mouse_down(e.button, e.window_id);
                cx.call_event_handler(&Event::MouseDown(e.into()))
            }
            XlibEvent::MouseUp(mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, e.window_id);
                let button = e.button;
                cx.call_event_handler(&Event::MouseUp(e.into()));
                cx.fingers.mouse_up(button);
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            XlibEvent::Scroll(mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, e.window_id);
                cx.call_event_handler(&Event::Scroll(e.into()))
            }
            XlibEvent::WindowDragQuery(mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, e.window_id);
                cx.call_event_handler(&Event::WindowDragQuery(e))
            }
            XlibEvent::WindowCloseRequested(e) => {
                let window_id = e.window_id;
                let accept_close = e.accept_close.clone();
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowCloseRequested(e));
                if accept_close.get() {
                    drop(cx);
                    if let EventFlow::Exit =
                        self.handle_window_closed(state, WindowClosedEvent { window_id })
                    {
                        let mut cx = self.cx.borrow_mut();
                        cx.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                }
            }
            XlibEvent::TextInput(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::TextInput(e))
            }
            XlibEvent::Drag(window_id, mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, window_id);
                cx.call_event_handler(&Event::Drag(e));
                cx.drag_drop.cycle_drag();
            }
            XlibEvent::Drop(window_id, mut e) => {
                let mut cx = self.cx.borrow_mut();
                cx.dpi_override_scale(&mut e.abs, window_id);
                cx.call_event_handler(&Event::Drop(e));
                cx.drag_drop.cycle_drag();
            }
            XlibEvent::DragEnd => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::MouseUp(MouseUpEvent {
                    abs: dvec2(-100000.0, -100000.0),
                    button: MouseButton::PRIMARY,
                    window_id: CxWindowPool::id_zero(),
                    modifiers: Default::default(),
                    time: 0.0,
                }));
                cx.fingers.mouse_up(MouseButton::PRIMARY);
                cx.fingers.cycle_hover_area(live_id!(mouse).into());

                cx.call_event_handler(&Event::DragEnd);
                cx.drag_drop.cycle_drag();
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
                if e.timer_id == 0 {
                    if SignalToUI::check_and_clear_ui_signal() {
                        cx.handle_termination_signal();
                        cx.handle_media_signals();
                        cx.handle_script_signals();
                        cx.call_event_handler(&Event::Signal);
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        cx.handle_action_receiver();
                    }
                    cx.poll_control_channel();
                    cx.handle_actions();
                    cx.handle_networking_events();

                    // Poll video players on the timer tick (every ~8ms).
                    // Always sweep pending GStreamer teardowns so the last closed
                    // player still finishes NULL without waiting for a new prepare.
                    if cx.os.video_players.is_empty() {
                        poll_pending_gstreamer_teardowns();
                    } else {
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        let gl: *const crate::os::linux::gl_sys::LibGl =
                            &cx.os.opengl_cx.as_ref().unwrap().libgl;
                        let egl = cx
                            .os
                            .opengl_cx
                            .as_ref()
                            .map(|cx| cx as *const super::super::opengl_cx::OpenglCx);
                        let mut players = std::mem::take(&mut cx.os.video_players);
                        let mut video_events = Vec::new();
                        for (_video_id, player) in players.iter_mut() {
                            let opengl_cx = egl.map(|ptr| unsafe { &*ptr });
                            video_events.extend(collect_linux_video_player_events(
                                player,
                                unsafe { &*gl },
                                &mut cx.textures,
                                opengl_cx,
                            ));
                        }
                        cx.os.video_players = players;
                        for event in video_events {
                            cx.call_event_handler(&event);
                        }
                    }
                } else {
                    cx.handle_script_timer(&e);
                    cx.call_event_handler(&Event::Timer(e))
                }

                cx.run_live_edit_if_needed("linux-wayland");
                let has_platform_ops = !cx.platform_ops.is_empty();
                drop(cx);
                if has_platform_ops {
                    if let EventFlow::Exit = self.handle_platform_ops(state) {
                        let mut cx = self.cx.borrow_mut();
                        cx.call_event_handler(&Event::Shutdown);
                        state.event_loop_running = false;
                        return EventFlow::Exit;
                    }
                }
                return EventFlow::Wait;
            }
        }
        // Drain ops queued during this event (e.g. pause/resume from MouseDown).
        {
            let cx = self.cx.borrow();
            let has_platform_ops = !cx.platform_ops.is_empty();
            drop(cx);
            if has_platform_ops {
                if let EventFlow::Exit = self.handle_platform_ops(state) {
                    let mut cx = self.cx.borrow_mut();
                    cx.call_event_handler(&Event::Shutdown);
                    state.event_loop_running = false;
                    return EventFlow::Exit;
                }
            }
        }
        let cx = self.cx.borrow();
        if cx.any_passes_dirty()
            || cx.need_redrawing()
            || cx.new_next_frames.len() != 0
            || cx.screenshot_requests.len() > 0
            || cx.demo_time_repaint
        {
            return EventFlow::Poll;
        } else {
            return EventFlow::Wait;
        }
    }

    fn app_event_callback(&mut self, wayland_app: &mut WaylandApp, event: XlibEvent) -> EventFlow {
        let event_flow = self.state_event_callback(&mut wayland_app.state, event);
        if let EventFlow::Exit = event_flow {
            wayland_app.terminate_event_loop();
        }
        event_flow
    }

    fn close_popup_window(
        &self,
        state: &mut WaylandState,
        window_id: WindowId,
        reason: Option<PopupDismissReason>,
    ) {
        let mut cx = self.cx.borrow_mut();
        if let Some(reason) = reason {
            cx.call_event_handler(&Event::PopupDismissed(PopupDismissedEvent {
                window_id,
                reason,
            }));
        }
        cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
        cx.windows[window_id].is_created = false;
        if state.pointer_window == Some(window_id) {
            state.pointer_window = None;
        }
        if state.keyboard_window == Some(window_id) {
            state.keyboard_window = None;
        }
        // A frame callback in flight for a destroyed surface never fires.
        state.clear_frame_callback_pending(window_id);
        if let Some(index) = state.popups.iter().position(|w| w.window_id == window_id) {
            state.popups.remove(index);
        }
    }

    fn handle_window_closed(
        &self,
        state: &mut WaylandState,
        event: WindowClosedEvent,
    ) -> EventFlow {
        let window_id = event.window_id;
        if !state.windows.iter().any(|w| w.window_id == window_id)
            && !state.popups.iter().any(|w| w.window_id == window_id)
        {
            return EventFlow::Poll;
        }
        self.close_popup_children(state, window_id);

        let mut cx = self.cx.borrow_mut();
        cx.call_event_handler(&Event::WindowClosed(event));
        cx.windows[window_id].is_created = false;
        if state.pointer_window == Some(window_id) {
            state.pointer_window = None;
        }
        if state.keyboard_window == Some(window_id) {
            state.keyboard_window = None;
        }
        // A frame callback in flight for a destroyed surface never fires.
        state.clear_frame_callback_pending(window_id);
        if let Some(index) = state.windows.iter().position(|w| w.window_id == window_id) {
            state.windows.remove(index);
            if state.windows.is_empty() {
                return EventFlow::Exit;
            }
        } else if let Some(index) = state.popups.iter().position(|w| w.window_id == window_id) {
            state.popups.remove(index);
        }
        EventFlow::Poll
    }

    fn close_popup_children(&self, state: &mut WaylandState, parent_window_id: WindowId) {
        loop {
            let child = state
                .popups
                .iter()
                .find(|p| p.parent_window_id == parent_window_id)
                .map(|p| p.window_id);
            if let Some(child_window_id) = child {
                self.close_popup_children(state, child_window_id);
                self.close_popup_window(
                    state,
                    child_window_id,
                    Some(PopupDismissReason::ParentClosed),
                );
            } else {
                break;
            }
        }
    }

    fn handle_platform_ops(&self, state: &mut WaylandState) -> EventFlow {
        let mut ret = EventFlow::Poll;
        let mut cx = self.cx.borrow_mut();
        if cx.platform_ops.is_empty() {
            return EventFlow::Poll;
        }
        while let Some(op) = cx.platform_ops.pop_front() {
            match op {
                CxOsOp::SetCursor(_) | CxOsOp::StartTimer { .. } | CxOsOp::StopTimer(_) => {}
                _ => {
                    //println!("handle op: {:?}", op)
                }
            }
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let gl_cx = cx.os.opengl_cx.as_ref().unwrap();
                    let compositor = state.compositor.as_ref().unwrap();
                    let wm_base = state.wm_base.as_ref().unwrap();
                    let window = &cx.windows[window_id];
                    let (create_position, create_inner_size) = window.create_geom();
                    let app_id = if window.create_app_id.is_empty() {
                        "Makepad"
                    } else {
                        &window.create_app_id
                    };
                    let window = WaylandWindow::new(
                        window_id,
                        compositor,
                        wm_base,
                        state.decoration_manager.as_ref(),
                        state.scale_manager.as_ref(),
                        state.viewporter.as_ref(),
                        state.icon_manager.as_ref(),
                        state.shm.as_ref(),
                        self.qhandle.as_ref().unwrap(),
                        gl_cx,
                        create_inner_size,
                        create_position,
                        &window.create_title,
                        app_id,
                        window.is_fullscreen,
                    );
                    if cx.windows[window_id].backdrop != crate::window::WindowBackdrop::None {
                        log_linux_backdrop_unsupported_once();
                    }
                    // Same as every other backend (x11/windows/macos/...): the Cx window
                    // only becomes usable once its OS window exists. Without `is_created`,
                    // `Cx::dpi_override_scale()` and `get_delegated_dpi_factor()` silently
                    // no-op, so pointer `abs` keeps arriving in native surface points while
                    // widget rects live in (zoomed) layout points and every click misses.
                    // Seed the geom too: the default `dpi_factor` is 0.0, which would make
                    // `get_pass_rect()` produce NaN once the flag is on.
                    let native_geom = window.window_geom.clone();
                    state.windows.push(window);
                    let cx_window = &mut cx.windows[window_id];
                    cx_window.os_dpi_factor = Some(native_geom.dpi_factor);
                    let layout_geom = cx_window.native_window_geom_to_layout(native_geom);
                    cx_window.window_geom = layout_geom;
                    cx_window.is_created = true;
                }
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let gl_cx = cx.os.opengl_cx.as_ref().unwrap();
                    let compositor = state.compositor.as_ref().unwrap();
                    let wm_base = state.wm_base.as_ref().unwrap();
                    if let Some(parent_xdg_surface) = state.xdg_surface_for_window(parent_window_id)
                    {
                        let popup = WaylandPopupWindow::new(
                            window_id,
                            parent_window_id,
                            compositor,
                            wm_base,
                            &parent_xdg_surface,
                            state.seat.as_ref(),
                            state.pointer_serial,
                            state.keyboard_serial,
                            state.scale_manager.as_ref(),
                            state.viewporter.as_ref(),
                            self.qhandle.as_ref().unwrap(),
                            gl_cx,
                            size,
                            position,
                            grab_keyboard,
                        );
                        cx.windows[window_id].is_popup = true;
                        cx.windows[window_id].popup_parent = Some(parent_window_id);
                        cx.windows[window_id].popup_position = Some(position);
                        cx.windows[window_id].popup_size = Some(size);
                        cx.windows[window_id].popup_grab_keyboard = grab_keyboard;
                        // See CreateWindow above.
                        let native_geom = popup.window_geom.clone();
                        state.popups.push(popup);
                        let cx_window = &mut cx.windows[window_id];
                        cx_window.os_dpi_factor = Some(native_geom.dpi_factor);
                        let layout_geom = cx_window.native_window_geom_to_layout(native_geom);
                        cx_window.window_geom = layout_geom;
                        cx_window.is_created = true;
                    }
                }
                CxOsOp::CloseWindow(window_id) => {
                    drop(cx);
                    if state.popups.iter().any(|w| w.window_id == window_id) {
                        self.close_popup_children(state, window_id);
                        self.close_popup_window(state, window_id, None);
                        cx = self.cx.borrow_mut();
                        if state.windows.is_empty() {
                            ret = EventFlow::Exit;
                        }
                        continue;
                    }

                    if let EventFlow::Exit =
                        self.handle_window_closed(state, WindowClosedEvent { window_id })
                    {
                        ret = EventFlow::Exit;
                        break;
                    }
                    cx = self.cx.borrow_mut();
                }
                CxOsOp::Quit => ret = EventFlow::Exit,
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(window) = state.windows.iter().find(|w| w.window_id == window_id) {
                        window.toplevel.set_minimized();
                    }
                }
                CxOsOp::Deminiaturize(_window_id) => todo!(),
                CxOsOp::HideWindow(_window_id) => todo!(),
                CxOsOp::HideWindowButtons(_) => {}
                CxOsOp::ShowWindowButtons(_) => {}
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(window) = state.windows.iter().find(|w| w.window_id == window_id) {
                        window.toplevel.set_maximized();
                    }
                }
                CxOsOp::FullscreenWindow(window_id) => {
                    if let Some(window) = state.windows.iter().find(|w| w.window_id == window_id) {
                        window.toplevel.set_fullscreen(None);
                    }
                }
                CxOsOp::RestoreWindow(window_id) | CxOsOp::NormalizeWindow(window_id) => {
                    if let Some(window) = state.windows.iter().find(|w| w.window_id == window_id) {
                        window.toplevel.unset_maximized();
                        window.toplevel.unset_fullscreen();
                    }
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    // A Wayland client has no "set my size" request. Window geometry is by
                    // default whatever the surface commits -- `xdg_surface.set_window_geometry`:
                    // "If never set, the value is the full bounds of the surface ... This
                    // updates dynamically on every commit" -- and this backend never sets it,
                    // so a self-resize is just the next frame committed at a different extent.
                    // The paint path derives the EGL extent and the viewport destination from
                    // `window_geom.inner_size`, so writing it here is the whole operation.
                    //
                    // Only a floating toplevel may choose its own size. Under xdg_toplevel's
                    // `maximized` state the configured window geometry must be obeyed "or the
                    // xdg_wm_base.invalid_surface_state error is raised", which disconnects the
                    // client; under `fullscreen` the configured geometry is a maximum. The
                    // configure handler folds both states into `is_fullscreen`, so that one
                    // flag gates the operation. Popups take their extent from their positioner
                    // and are deliberately not matched here.
                    if let Some(window) =
                        state.windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        if window.window_geom.is_fullscreen {
                            crate::error!(
                                "ResizeWindow ignored: a maximized or fullscreen Wayland toplevel \
                                 must keep the size the compositor configured."
                            );
                        } else if let Some(size) = crate::screen::sanitize_resize(size) {
                            // Wayland surface coordinates are already logical points, so unlike
                            // X11 and Win32 -- which scale the request into device pixels -- the
                            // requested size is the surface extent as-is.
                            window.window_geom.inner_size = size;
                            window.window_geom.outer_size = size;
                            let native_geom = window.window_geom.clone();
                            let cx_window = &mut cx.windows[window_id];
                            cx_window.os_dpi_factor = Some(native_geom.dpi_factor);
                            let layout_geom =
                                cx_window.native_window_geom_to_layout(native_geom);
                            cx_window.window_geom = layout_geom;
                            if let Some(main_pass_id) = cx_window.main_pass_id {
                                cx.redraw_pass_and_child_passes(main_pass_id);
                            }
                        } else {
                            crate::error!(
                                "ResizeWindow ignored: {}x{} is not a usable surface extent.",
                                size.x,
                                size.y
                            );
                        }
                    }
                }
                // A Wayland client is not told where its windows are and cannot move them;
                // the compositor owns placement, so a window here is never left off-screen
                // by a restored position the way it can be on Windows, macOS and X11.
                // xdg_toplevel exposes no absolute-positioning request: `move` is
                // interactive and serial-gated ("This request must be used in response to
                // some sort of user action"), and `reposition` is an xdg_popup request
                // requiring xdg_wm_base v3, which `wayland_state.rs` does not bind. This arm
                // is correct as a permanent no-op.
                CxOsOp::RepositionWindow(_window_id, _size) => {}
                CxOsOp::SetWindowTitle(window_id, title) => {
                    if let Some(window) = state.windows.iter().find(|w| w.window_id == window_id) {
                        window.toplevel.set_title(title);
                    }
                }
                CxOsOp::SetWindowVisuals(_window_id, visuals) => {
                    if visuals.backdrop != crate::window::WindowBackdrop::None {
                        log_linux_backdrop_unsupported_once();
                    }
                }
                CxOsOp::ShowClipboardActions { .. } => {}
                CxOsOp::CopyToClipboard(content) => {
                    if let Some(serial) = state.keyboard_serial.or(state.pointer_serial) {
                        if let Some(qhandle) = self.qhandle.as_ref() {
                            state.set_clipboard_text(qhandle, serial, content);
                        }
                    } else {
                        state.clipboard_text = content.clone();
                        state.pending_clipboard_copy = Some(content);
                    }
                }
                CxOsOp::SetPrimarySelection(content) => {
                    if let Some(serial) = state.keyboard_serial.or(state.pointer_serial) {
                        if state.primary_selection_manager.is_some() {
                            let qh = self.qhandle.as_ref().unwrap();
                            state.set_primary_selection_text(qh, serial, content);
                        }
                    } else {
                        state.primary_selection_text = content;
                    }
                }
                CxOsOp::ShowSelectionHandles { .. } => {}
                CxOsOp::UpdateSelectionHandles { .. } => {}
                CxOsOp::HideSelectionHandles => {}
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::StartDragging(items) => {
                    state.start_internal_drag(items);
                }
                CxOsOp::StartExternalDragging { .. } => {
                    crate::error!("external file dragging is not implemented on Wayland");
                    cx.call_event_handler(&Event::DragEnd);
                }
                CxOsOp::SetCursor(cursor) => {
                    if let Some(cursor_shape) = state.cursor_shape.as_ref() {
                        if let Some(serial) = state.pointer_serial.as_ref() {
                            cursor_shape.set_shape(*serial, cursor.into());
                        }
                    }
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    state.start_timer(timer_id, interval, repeats);
                }
                CxOsOp::StopTimer(timer_id) => {
                    state.stop_timer(timer_id);
                }
                // The desktop's own dialog helper, on its own thread; the
                // answer arrives as a FileDialogAction like every OS.
                CxOsOp::SelectFileDialog(settings) => {
                    crate::os::linux::file_dialog::open_select_file_dialog(settings);
                }
                CxOsOp::SaveFileDialog(settings) => {
                    crate::os::linux::file_dialog::open_save_file_dialog(settings);
                }
                CxOsOp::SelectFolderDialog(settings) => {
                    crate::os::linux::file_dialog::open_select_folder_dialog(settings);
                }
                CxOsOp::SaveFolderDialog(settings) => {
                    crate::os::linux::file_dialog::open_save_folder_dialog(settings);
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = cx.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = cx.net.http_cancel(request_id);
                }
                CxOsOp::ShowTextIME(area, cursor_rect, _config) => {
                    if let Some(_window) = state.keyboard_window.or(state.pointer_window) {
                        if let Some(text_input) = state.text_input.as_ref() {
                            text_input.enable();

                            // Report the caret line's bounding box (surface-local
                            // logical coords) so the compositor anchors the IME
                            // candidate window directly above/below the line.
                            // Inflate it vertically by a fraction of the line height
                            // so the candidate keeps a gap from the text rather than
                            // hugging it (matches the macOS clearance).
                            let rect_pos = area.clipped_rect(&*cx).pos + cursor_rect.pos;
                            let clearance = cursor_rect.size.y * 0.6;
                            text_input.set_cursor_rectangle(
                                rect_pos.x as i32,
                                (rect_pos.y - clearance) as i32,
                                cursor_rect.size.x.max(1.0) as i32,
                                (cursor_rect.size.y + 2.0 * clearance) as i32,
                            );
                            text_input.commit();
                        }
                    }
                }
                CxOsOp::HideTextIME => {
                    if let Some(text_input) = state.text_input.as_ref() {
                        text_input.disable();
                        text_input.commit();
                    }
                }
                // Mobile-only ops (soft keyboard, clipboard UI); no-op on desktop
                CxOsOp::SyncImeState { .. } => {}
                CxOsOp::HideClipboardActions => {}
                CxOsOp::PrepareVideoPlayback(
                    video_id,
                    source,
                    _camera_preview_mode,
                    _external_texture_id,
                    texture_id,
                    autoplay,
                    should_loop,
                ) => {
                    // Replacing an existing player for the same id: tear down first so
                    // prepare is never a silent no-op (source changes, replay, etc.).
                    if let Some(mut player) = cx.os.video_players.remove(&video_id) {
                        player.cleanup();
                        cx.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                    }
                    // Camera source: use V4L2 capture player with YUV plane textures
                    if let VideoSource::Camera(input_id, format_id) = source {
                        let camera_access = cx.os.media.v4l2_camera();
                        let tex_y = cx.textures.alloc(TextureFormat::VideoYuvPlane);
                        let tex_u = cx.textures.alloc(TextureFormat::VideoYuvPlane);
                        let tex_v = cx.textures.alloc(TextureFormat::VideoYuvPlane);
                        let tex_y_id = tex_y.texture_id();
                        let tex_u_id = tex_u.texture_id();
                        let tex_v_id = tex_v.texture_id();
                        let player = V4l2CameraPlayer::new(
                            video_id,
                            tex_y_id,
                            tex_u_id,
                            tex_v_id,
                            input_id,
                            format_id,
                            camera_access,
                        );
                        cx.os
                            .video_players
                            .insert(video_id, LinuxVideoPlayer::Camera(player));
                        cx.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady::planes(video_id, tex_y, tex_u, tex_v),
                        ));
                        continue;
                    }
                    // Shared prepare for file/network/session sources.
                    let prep = {
                        let cx_ref = &mut *cx;
                        prepare_desktop_linux_video(
                            &mut cx_ref.os.gstreamer,
                            &mut cx_ref.textures,
                            video_id,
                            source,
                            texture_id,
                            autoplay,
                            should_loop,
                            cx_ref.os.opengl_cx.as_ref(),
                        )
                    };
                    match prep {
                        LinuxPrepareResult::Ready { player, yuv } => {
                            cx.os.video_players.insert(video_id, player);
                            if let Some(yuv) = yuv {
                                cx.call_event_handler(&Event::VideoYuvTexturesReady(
                                    VideoYuvTexturesReady::planes(video_id, yuv.tex_y, yuv.tex_u, yuv.tex_v)
                                        .with_external_opt(yuv.tex_y_oes, yuv.tex_u_oes),
                                ));
                            }
                        }
                        LinuxPrepareResult::Failed(error) => {
                            cx.call_event_handler(&Event::VideoDecodingError(
                                VideoDecodingErrorEvent { video_id, error },
                            ));
                        }
                    }
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.play();
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.pause();
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.resume();
                    }
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.mute();
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.unmute();
                    }
                }
                CxOsOp::CleanupVideoPlaybackResources(video_id) => {
                    if let Some(mut player) = cx.os.video_players.remove(&video_id) {
                        player.cleanup();
                        cx.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                    }
                }
                CxOsOp::SeekVideoPlayback(video_id, position_ms) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.seek_to(position_ms);
                    }
                }
                CxOsOp::SetVideoVolume(video_id, volume) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        player.set_volume(volume);
                    }
                }
                CxOsOp::SetVideoPlaybackRate(video_id, rate) => {
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.set_playback_rate(rate);
                    }
                }
                CxOsOp::SelectVideoTrack(video_id, index) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        let _ = player.select_video_track(index);
                    }
                }
                CxOsOp::SelectAudioTrack(video_id, index) => {
                    if let Some(player) = cx.os.video_players.get_mut(&video_id) {
                        let _ = player.select_audio_track(index);
                    }
                }
                CxOsOp::AttachCameraNativePreview { .. }
                | CxOsOp::UpdateCameraNativePreview { .. }
                | CxOsOp::DetachCameraNativePreview { .. } => {
                    // Native camera preview is emulated via composited texture path on Linux.
                }
                CxOsOp::PrepareAudioPlayback(video_id, source, autoplay, should_loop) => {
                    if let Some(mut player) = cx.os.video_players.remove(&video_id) {
                        player.cleanup();
                        cx.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                    }
                    if cx.os.gstreamer.is_none() {
                        match LibGStreamer::try_load() {
                            Some(gst) => {
                                gst.init();
                                cx.os.gstreamer = Some(gst);
                            }
                            None => {
                                cx.call_event_handler(&Event::VideoDecodingError(
                                    VideoDecodingErrorEvent {
                                        video_id,
                                        error: "GStreamer not available".to_string(),
                                    },
                                ));
                                continue;
                            }
                        }
                    }
                    if let Some(ref gst) = cx.os.gstreamer {
                        let player = GStreamerVideoPlayer::new_audio_only(
                            gst,
                            video_id,
                            source,
                            autoplay,
                            should_loop,
                        );
                        if player.is_active() {
                            cx.os.video_players.insert(
                                video_id,
                                LinuxVideoPlayer::GStreamer { player, yuv: None },
                            );
                        } else {
                            cx.call_event_handler(&Event::VideoDecodingError(
                                VideoDecodingErrorEvent {
                                    video_id,
                                    error: "Failed to initialize audio-only GStreamer pipeline"
                                        .to_string(),
                                },
                            ));
                        }
                    }
                }
                CxOsOp::UpdateVideoSurfaceTexture(_) => {
                    // Not needed on Linux desktop (Android-only)
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                }
                | CxOsOp::RequestPermission {
                    permission,
                    request_id,
                } => {
                    cx.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status: crate::permission::PermissionStatus::Granted,
                        },
                    ));
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        ret
    }

    pub(crate) fn handle_repaint(&self, state: &mut WaylandState) {
        let mut cx = self.cx.borrow_mut();
        // Skip the eglMakeCurrent + full pass-list scan when there is nothing to draw.
        // demo_time_repaint forces a redraw of time-animated passes (see
        // compute_pass_repaint_order), so it must keep us rendering.
        if !cx.any_passes_dirty() && !cx.demo_time_repaint {
            return;
        }
        cx.os.opengl_cx.as_ref().unwrap().make_current();
        let mut passes_todo = Vec::new();
        cx.compute_pass_repaint_order(&mut passes_todo);
        cx.repaint_id += 1;
        for draw_pass_id in &passes_todo {
            let now = state.time_now();
            cx.passes[*draw_pass_id].set_time(now as f32);
            let parent = cx.passes[*draw_pass_id].parent.clone();
            match parent {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    // Frame-callback pacing: if this surface's previous frame callback
                    // has not fired yet, the compositor is not ready for a new frame
                    // (it withholds callbacks entirely while the window is occluded or
                    // minimized). Skip the present and leave the pass dirty so the
                    // window repaints when the callback arrives.
                    if self.frame_pacing && state.is_frame_callback_pending(window_id) {
                        continue;
                    }
                    let mut presented = false;
                    if let Some(window) =
                        state.windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        if !window.configured {
                            continue;
                        }
                        window.resize_buffers();
                        if std::env::var_os("MAKEPAD_WAYLAND_TRACE").is_some() {
                            crate::log!(
                                "Wayland paint window={:?} inner=({}, {}) dpi={} pix=({}, {})",
                                window.window_id,
                                window.window_geom.inner_size.x,
                                window.window_geom.inner_size.y,
                                window.window_geom.dpi_factor,
                                window.window_geom.inner_size.x * window.window_geom.dpi_factor,
                                window.window_geom.inner_size.y * window.window_geom.dpi_factor
                            );
                        }
                        if let Some(viewport) = window.viewport.as_ref() {
                            viewport.set_source(-1., -1., -1., -1.);
                            // `wp_viewport.set_destination` raises the `bad_value` protocol
                            // error, which disconnects the client, on a zero or negative
                            // extent, and a float-to-int cast turns both a negative and a NaN
                            // into zero. Floor the destination the way `resize_buffers` floors
                            // the EGL extent.
                            viewport.set_destination(
                                window.window_geom.inner_size.x.max(1.0) as i32,
                                window.window_geom.inner_size.y.max(1.0) as i32,
                            );
                        }
                        let pix_width =
                            window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height =
                            window.window_geom.inner_size.y * window.window_geom.dpi_factor;

                        // Request the next frame callback before the swap; the swap
                        // performs the commit that carries the request.
                        if self.frame_pacing {
                            window
                                .base_surface
                                .frame(self.qhandle.as_ref().unwrap(), window_id);
                        }
                        presented = cx.draw_pass_to_window(
                            *draw_pass_id,
                            window.egl_surface,
                            pix_width,
                            pix_height,
                        );
                    } else if let Some(window) =
                        state.popups.iter_mut().find(|w| w.window_id == window_id)
                    {
                        if !window.configured {
                            continue;
                        }
                        window.resize_buffers();
                        if let Some(viewport) = window.viewport.as_ref() {
                            viewport.set_source(-1., -1., -1., -1.);
                            // `wp_viewport.set_destination` raises the `bad_value` protocol
                            // error, which disconnects the client, on a zero or negative
                            // extent, and a float-to-int cast turns both a negative and a NaN
                            // into zero. Floor the destination the way `resize_buffers` floors
                            // the EGL extent.
                            viewport.set_destination(
                                window.window_geom.inner_size.x.max(1.0) as i32,
                                window.window_geom.inner_size.y.max(1.0) as i32,
                            );
                        }
                        let pix_width =
                            window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height =
                            window.window_geom.inner_size.y * window.window_geom.dpi_factor;
                        if self.frame_pacing {
                            window
                                .base_surface
                                .frame(self.qhandle.as_ref().unwrap(), window_id);
                        }
                        presented = cx.draw_pass_to_window(
                            *draw_pass_id,
                            window.egl_surface,
                            pix_width,
                            pix_height,
                        );
                    }
                    // Only gate on the callback when a commit actually happened; a
                    // failed swap sends no commit, so its callback would never fire.
                    if self.frame_pacing && presented {
                        state.set_frame_callback_pending(window_id);
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    cx.draw_pass_to_texture(*draw_pass_id, None);
                }
                CxDrawPassParent::None => {
                    cx.draw_pass_to_texture(*draw_pass_id, None);
                }
            }
        }
    }
}
