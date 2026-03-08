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
use crate::makepad_math::dvec2;
use crate::opengl_cx::OpenglCx;
use crate::os::linux::gstreamer_sys::LibGStreamer;
use crate::os::linux::linux_video_playback::GStreamerVideoPlayer;
use crate::os::linux::linux_video_player::{LinuxVideoPlayer, YuvTextureSet};
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
            VideoBufferedRangesEvent, VideoDecodingErrorEvent, VideoPlaybackPreparedEvent,
            VideoPlaybackResourcesReleasedEvent, VideoSeekableRangesEvent, VideoSource,
            VideoTextureUpdatedEvent, VideoYuvTexturesReady,
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

pub fn wayland_event_loop(cx: Rc<RefCell<Cx>>) {
    WaylandCx::event_loop_impl(cx);
}

pub(crate) struct WaylandCx {
    cx: Rc<RefCell<Cx>>,
    qhandle: Option<wayland_client::QueueHandle<WaylandState>>,
}

impl WaylandCx {
    pub fn event_loop_impl(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::LinuxWindow(LinuxWindowParams {
            custom_window_chrome: true,
        });
        cx.borrow_mut().gpu_info.performance = GpuPerformance::Tier1;

        let wayland_cx = Rc::new(RefCell::new(WaylandCx {
            cx: cx.clone(),
            qhandle: None,
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
                //paint_dirty = true;
                cx.call_event_handler(&Event::WindowGotFocus(window_id));
            }
            XlibEvent::WindowLostFocus(window_id) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowLostFocus(window_id));
            }
            XlibEvent::WindowGeomChange(mut re) => {
                // do this here because mac
                let mut cx = self.cx.borrow_mut();
                if let Some(window) = state
                    .windows
                    .iter_mut()
                    .find(|w| w.window_id == re.window_id)
                {
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
                } else if let Some(window) = state
                    .popups
                    .iter_mut()
                    .find(|w| w.window_id == re.window_id)
                {
                    window.window_geom = re.new_geom.clone();
                    cx.windows[re.window_id].window_geom = re.new_geom.clone();
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
                let window_id = wc.window_id;
                self.close_popup_children(state, window_id);

                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                cx.windows[window_id].is_created = false;
                if state.pointer_window == Some(window_id) {
                    state.pointer_window = None;
                }
                if state.keyboard_window == Some(window_id) {
                    state.keyboard_window = None;
                }
                if let Some(index) = state.windows.iter().position(|w| w.window_id == window_id) {
                    state.windows.remove(index);
                    if state.windows.len() == 0 {
                        cx.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                } else if let Some(index) =
                    state.popups.iter().position(|w| w.window_id == window_id)
                {
                    state.popups.remove(index);
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
            }
            XlibEvent::MouseMove(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::MouseMove(e.into()));
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
                cx.fingers.switch_captures();
            }
            XlibEvent::MouseDown(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.fingers.process_tap_count(e.abs, e.time);
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
                cx.call_event_handler(&Event::Drag(e));
                cx.drag_drop.cycle_drag();
            }
            XlibEvent::Drop(e) => {
                let mut cx = self.cx.borrow_mut();
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
                        cx.handle_media_signals();
                        cx.handle_script_signals();
                        cx.call_event_handler(&Event::Signal);
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        cx.handle_action_receiver();
                    }
                    cx.poll_control_channel();
                    cx.handle_networking_events();

                    // Poll video players on the timer tick (every ~8ms).
                    if !cx.os.video_players.is_empty() {
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        let gl: *const crate::os::linux::gl_sys::LibGl =
                            &cx.os.opengl_cx.as_ref().unwrap().libgl;
                        let mut players = std::mem::take(&mut cx.os.video_players);
                        let mut video_events = Vec::new();
                        for (_video_id, player) in players.iter_mut() {
                            match player.check_prepared() {
                                Some(Ok((
                                    width,
                                    height,
                                    duration,
                                    is_seekable,
                                    video_tracks,
                                    audio_tracks,
                                ))) => {
                                    video_events.push(Event::VideoPlaybackPrepared(
                                        VideoPlaybackPreparedEvent {
                                            video_id: player.video_id(),
                                            video_width: width,
                                            video_height: height,
                                            duration,
                                            is_seekable,
                                            video_tracks,
                                            audio_tracks,
                                        },
                                    ));
                                    let seekable = player.seekable_ranges();
                                    if !seekable.is_empty() {
                                        video_events.push(Event::VideoSeekableRanges(
                                            VideoSeekableRangesEvent {
                                                video_id: player.video_id(),
                                                ranges: seekable,
                                            },
                                        ));
                                    }
                                    let buffered = player.buffered_ranges();
                                    if !buffered.is_empty() {
                                        video_events.push(Event::VideoBufferedRanges(
                                            VideoBufferedRangesEvent {
                                                video_id: player.video_id(),
                                                ranges: buffered,
                                            },
                                        ));
                                    }
                                }
                                Some(Err(err)) => {
                                    video_events.push(Event::VideoDecodingError(
                                        VideoDecodingErrorEvent {
                                            video_id: player.video_id(),
                                            error: err,
                                        },
                                    ));
                                }
                                None => {}
                            }
                            if player.poll_frame(unsafe { &*gl }, &mut cx.textures) {
                                video_events.push(Event::VideoTextureUpdated(
                                    VideoTextureUpdatedEvent {
                                        video_id: player.video_id(),
                                        current_position_ms: player.current_position_ms(),
                                        yuv: crate::event::video_playback::VideoYuvMetadata {
                                            enabled: player.is_yuv_mode(),
                                            matrix: player.yuv_matrix(),
                                            biplanar: false,
                                            rotation_steps: 0.0,
                                        },
                                    },
                                ));
                            }
                            if player.check_eos() {
                                video_events.push(Event::VideoPlaybackCompleted(
                                    crate::event::video_playback::VideoPlaybackCompletedEvent {
                                        video_id: player.video_id(),
                                    },
                                ));
                            }
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

                if cx.handle_live_edit() {
                    cx.call_event_handler(&Event::LiveEdit);
                    cx.redraw_all();
                }
                return EventFlow::Wait;
            }
        }
        return EventFlow::Poll;
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
        if let Some(index) = state.popups.iter().position(|w| w.window_id == window_id) {
            state.popups.remove(index);
        }
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
        while let Some(op) = cx.platform_ops.pop() {
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
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        app_id,
                        window.is_fullscreen,
                    );
                    state.windows.push(window);
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
                        state.popups.push(popup);
                    }
                }
                CxOsOp::CloseWindow(window_id) => {
                    self.close_popup_children(state, window_id);
                    if state.popups.iter().any(|w| w.window_id == window_id) {
                        drop(cx);
                        self.close_popup_window(state, window_id, None);
                        cx = self.cx.borrow_mut();
                        if state.windows.is_empty() {
                            ret = EventFlow::Exit;
                        }
                        continue;
                    }

                    cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                    let windows = &mut state.windows;
                    if let Some(index) = windows.iter().position(|w| w.window_id == window_id) {
                        cx.windows[window_id].is_created = false;
                        windows.remove(index);
                        if windows.len() == 0 {
                            ret = EventFlow::Exit
                        }
                    }
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
                CxOsOp::ResizeWindow(window_id, size) => {}
                CxOsOp::RepositionWindow(window_id, size) => {}
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
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = cx.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = cx.net.http_cancel(request_id);
                }
                CxOsOp::ShowTextIME(area, pos, _config) => {
                    if let Some(_window) = state.keyboard_window.or(state.pointer_window) {
                        if let Some(text_input) = state.text_input.as_ref() {
                            text_input.enable();

                            // todo: follow the cursor while input
                            text_input.set_cursor_rectangle(
                                state.last_mouse_pos.x as i32,
                                state.last_mouse_pos.y as i32,
                                0,
                                0,
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
                    // Skip if an active player already exists for this video_id
                    if cx
                        .os
                        .video_players
                        .get(&video_id)
                        .map_or(false, |p| p.is_active())
                    {
                        continue;
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
                            video_id, tex_y_id, tex_u_id, tex_v_id, input_id, format_id, camera_access,
                        );
                        cx.os.video_players.insert(video_id, LinuxVideoPlayer::Camera(player));
                        cx.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady { video_id, tex_y, tex_u, tex_v },
                        ));
                        continue;
                    }
                    // Try GStreamer first, fall back to software rav1d
                    let mut use_software = std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some();
                    if use_software {
                        crate::log!(
                            "VIDEO: MAKEPAD_FORCE_SOFTWARE_VIDEO set, using software video decoder"
                        );
                    }
                    if cx.os.gstreamer.is_none() {
                        match LibGStreamer::try_load() {
                            Some(gst) => {
                                gst.init();
                                cx.os.gstreamer = Some(gst);
                            }
                            None => {
                                crate::log!(
                                    "VIDEO: GStreamer not available, using software video decoder"
                                );
                                use_software = true;
                            }
                        }
                    }
                    if !use_software {
                        if cx.os.gstreamer.is_some() {
                            let yuv = YuvTextureSet::new(
                                cx.textures.alloc(TextureFormat::VideoYuvPlane),
                                cx.textures.alloc(TextureFormat::VideoYuvPlane),
                                cx.textures.alloc(TextureFormat::VideoYuvPlane),
                            );
                            let gst = cx.os.gstreamer.as_ref().unwrap();

                            let player = GStreamerVideoPlayer::new(
                                gst,
                                video_id,
                                texture_id,
                                Some(yuv.ids),
                                source.clone(),
                                autoplay,
                                should_loop,
                            );
                            if player.is_active() {
                                cx.os.video_players.insert(
                                    video_id,
                                    LinuxVideoPlayer::GStreamer {
                                        player,
                                        yuv: Some(yuv.clone()),
                                    },
                                );
                                cx.call_event_handler(&Event::VideoYuvTexturesReady(
                                    VideoYuvTexturesReady {
                                        video_id,
                                        tex_y: yuv.tex_y,
                                        tex_u: yuv.tex_u,
                                        tex_v: yuv.tex_v,
                                    },
                                ));
                                continue;
                            }
                            crate::log!("VIDEO: GStreamer pipeline failed, falling back to software video decoder");
                            use_software = true;
                        }
                    }
                    if use_software {
                        // Allocate YUV textures internally for software decode
                        let yuv = YuvTextureSet::new(
                            cx.textures.alloc(TextureFormat::VideoYuvPlane),
                            cx.textures.alloc(TextureFormat::VideoYuvPlane),
                            cx.textures.alloc(TextureFormat::VideoYuvPlane),
                        );
                        let player = crate::video_decode::software_video::SoftwareVideoPlayer::new(
                            video_id,
                            texture_id,
                            source,
                            autoplay,
                            should_loop,
                        );
                        cx.os.video_players.insert(
                            video_id,
                            LinuxVideoPlayer::Software {
                                player,
                                yuv: yuv.clone(),
                                yuv_matrix: 0.0,
                            },
                        );
                        // Notify widget so it can bind textures to shader slots
                        cx.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady {
                                video_id,
                                tex_y: yuv.tex_y,
                                tex_u: yuv.tex_u,
                                tex_v: yuv.tex_v,
                            },
                        ));
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
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.mute();
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get(&video_id) {
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
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.set_volume(volume);
                    }
                }
                CxOsOp::SetVideoPlaybackRate(video_id, rate) => {
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.set_playback_rate(rate);
                    }
                }
                CxOsOp::AttachCameraNativePreview { .. }
                | CxOsOp::UpdateCameraNativePreview { .. }
                | CxOsOp::DetachCameraNativePreview { .. } => {
                    // Native camera preview is emulated via composited texture path on Linux.
                }
                CxOsOp::PrepareAudioPlayback(video_id, source, autoplay, should_loop) => {
                    if cx
                        .os
                        .video_players
                        .get(&video_id)
                        .map_or(false, |p| p.is_active())
                    {
                        continue;
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
                } | CxOsOp::RequestPermission {
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
                            viewport.set_destination(
                                window.window_geom.inner_size.x as i32,
                                window.window_geom.inner_size.y as i32,
                            );
                        }
                        let pix_width =
                            window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height =
                            window.window_geom.inner_size.y * window.window_geom.dpi_factor;

                        cx.draw_pass_to_window(
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
                            viewport.set_destination(
                                window.window_geom.inner_size.x as i32,
                                window.window_geom.inner_size.y as i32,
                            );
                        }
                        let pix_width =
                            window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height =
                            window.window_geom.inner_size.y * window.window_geom.dpi_factor;
                        cx.draw_pass_to_window(
                            *draw_pass_id,
                            window.egl_surface,
                            pix_width,
                            pix_height,
                        );
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
