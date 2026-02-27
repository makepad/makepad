#![allow(unused_imports, unused_variables)]
//! Main Wayland backend implementation
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::opengl_wayland::WaylandWindow;
use super::wayland_state::WaylandState;
use crate::cx_native::EventFlow;
use crate::egl_sys::NativeDisplayType;
use crate::gl_sys::TEXTURE0;
use crate::makepad_live_id::*;
use crate::makepad_math::dvec2;
use crate::opengl_cx::OpenglCx;
use crate::os::linux::gstreamer_sys::LibGStreamer;
use crate::os::linux::linux_video_playback::GStreamerVideoPlayer;
use crate::wayland::wayland_app::WaylandApp;
use crate::wayland::xkb_sys;
use crate::x11::xlib_event::XlibEvent;
use crate::WindowId;
use crate::{
    cx::{LinuxWindowParams, OsType},
    egl_sys,
    event::video_playback::{
        VideoDecodingErrorEvent, VideoPlaybackPreparedEvent,
        VideoPlaybackResourcesReleasedEvent, VideoTextureUpdatedEvent,
    },
    gpu_info::GpuPerformance,
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
                if let Some(index) = state.windows.iter().position(|w| w.window_id == window_id) {
                    state.windows.remove(index);
                    if state.windows.len() == 0 {
                        cx.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                }
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
                            if let Some((width, height, duration)) = player.check_prepared() {
                                video_events.push(Event::VideoPlaybackPrepared(
                                    VideoPlaybackPreparedEvent {
                                        video_id: player.video_id,
                                        video_width: width,
                                        video_height: height,
                                        duration,
                                    },
                                ));
                            }
                            if player.poll_frame(unsafe { &*gl }, &mut cx.textures) {
                                video_events.push(Event::VideoTextureUpdated(
                                    VideoTextureUpdatedEvent {
                                        video_id: player.video_id,
                                        current_position_ms: player.current_position_ms(),
                                    },
                                ));
                            }
                            if player.check_eos() {
                                video_events.push(Event::VideoPlaybackCompleted(
                                    crate::event::video_playback::VideoPlaybackCompletedEvent {
                                        video_id: player.video_id,
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
                CxOsOp::CloseWindow(window_id) => {
                    cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                    let windows = &mut state.windows;
                    if let Some(index) = windows.iter().position(|w| w.window_id == window_id) {
                        cx.windows[window_id].is_created = false;
                        windows[index].close_window();
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
                        state.clipboard_text = content;
                    }
                }
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
                    if let Some(window) = state.current_window {
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
                    _external_texture_id,
                    texture_id,
                    autoplay,
                    should_loop,
                ) => {
                    // Skip if an active player already exists for this video_id
                    // (prevents accidental replacement which would reset the pipeline)
                    if cx.os.video_players.get(&video_id).map_or(false, |p| p.is_active()) {
                        continue;
                    }
                    // Lazy-load GStreamer
                    if cx.os.gstreamer.is_none() {
                        match LibGStreamer::try_load() {
                            Some(gst) => {
                                gst.init();
                                cx.os.gstreamer = Some(gst);
                            }
                            None => {
                                let error_msg = "GStreamer not available — install gstreamer1.0-plugins-base and gstreamer1.0-plugins-good.".to_string();
                                crate::error!("VIDEO: {}", error_msg);
                                cx.call_event_handler(&Event::VideoDecodingError(
                                    VideoDecodingErrorEvent {
                                        video_id,
                                        error: error_msg,
                                    },
                                ));
                                continue;
                            }
                        }
                    }
                    if let Some(ref gst) = cx.os.gstreamer {
                        let player = GStreamerVideoPlayer::new(
                            gst, video_id, texture_id, source, autoplay, should_loop,
                        );
                        if player.is_active() {
                            cx.os.video_players.insert(video_id, player);
                        } else {
                            cx.call_event_handler(&Event::VideoDecodingError(
                                VideoDecodingErrorEvent {
                                    video_id,
                                    error: "Failed to initialize Linux GStreamer playback pipeline".to_string(),
                                },
                            ));
                        }
                    }
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.play();
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.pause();
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if let Some(player) = cx.os.video_players.get(&video_id) {
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
                    if let Some(player) = cx.os.video_players.get(&video_id) {
                        player.seek_to(position_ms);
                    }
                }
                CxOsOp::UpdateVideoSurfaceTexture(_) => {
                    // Not needed on Linux desktop (Android-only)
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
            let windows = &mut state.windows;
            cx.passes[*draw_pass_id].set_time(now as f32);
            let parent = cx.passes[*draw_pass_id].parent.clone();
            match parent {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(window) = windows.iter_mut().find(|w| w.window_id == window_id) {
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
