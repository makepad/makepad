use {
    self::super::super::{
        egl_sys,
        gstreamer_sys::LibGStreamer,
        linux_video_playback::GStreamerVideoPlayer,
        linux_video_player::{LinuxVideoPlayer, YuvTextureSet},
        opengl_cx::OpenglCx,
        v4l2_camera_player::V4l2CameraPlayer,
        x11::x11_sys,
        x11::xlib_app::*,
        x11::xlib_event::*,
    },
    self::super::opengl_x11::OpenglWindow,
    crate::{
        cx::{Cx, LinuxWindowParams, OsType},
        cx_api::CxOsOp,
        draw_pass::CxDrawPassParent,
        event::{
            video_playback::{
                VideoBufferedRangesEvent, VideoDecodingErrorEvent, VideoPlaybackPreparedEvent,
                VideoPlaybackResourcesReleasedEvent, VideoSeekableRangesEvent,
                VideoTextureUpdatedEvent, VideoYuvTexturesReady,
            },
            *,
        },
        gpu_info::GpuPerformance,
        makepad_live_id::*,
        makepad_math::dvec2,
        os::cx_native::EventFlow,
        texture::TextureFormat,
        thread::SignalToUI,
        CxWindowPool,
    },
    std::cell::RefCell,
    std::rc::Rc,
    std::sync::{Arc, Mutex},
};

pub fn x11_event_loop(cx: Rc<RefCell<Cx>>) {
    X11Cx::event_loop_impl(cx)
}

pub struct X11Cx {
    pub cx: Rc<RefCell<Cx>>,
    internal_drag_items: Option<Arc<Vec<DragItem>>>,
}

impl X11Cx {
    pub fn event_loop_impl(cx: Rc<RefCell<Cx>>) {
        let mut x11_cx = X11Cx {
            cx: cx.clone(),
            internal_drag_items: None,
        };
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::LinuxWindow(LinuxWindowParams {
            custom_window_chrome: false,
        });
        cx.borrow_mut().gpu_info.performance = GpuPerformance::Tier1;

        let opengl_windows = Rc::new(RefCell::new(Vec::new()));
        let is_stdin_loop = crate::app_main::should_run_stdin_loop_from_env();
        if is_stdin_loop {
            cx.borrow_mut().in_makepad_studio = true;
        }
        init_xlib_app_global(Box::new({
            move |xlib_app, events| {
                if is_stdin_loop {
                    return EventFlow::Wait;
                }
                let mut opengl_windows = opengl_windows.borrow_mut();
                x11_cx.xlib_event_callback(xlib_app, events, &mut *opengl_windows)
            }
        }));

        // Stdin-loop runs Linux child rendering through the X11 backend.
        // Keep EGL platform selection consistent to avoid mixed X11/Wayland
        // context behavior in WSL/Xwayland setups.
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
        get_xlib_app_global().start_timer(0, 0.008, true);
        get_xlib_app_global().event_loop();
    }

    fn xlib_event_callback(
        &mut self,
        xlib_app: &mut XlibApp,
        event: XlibEvent,
        opengl_windows: &mut Vec<OpenglWindow>,
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(opengl_windows, xlib_app) {
            return EventFlow::Exit;
        }

        //let mut paint_dirty = false;

        //self.process_desktop_pre_event(&mut event);
        match event {
            XlibEvent::WindowGotFocus(window_id) => {
                // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                let mut cx = self.cx.borrow_mut();
                for window in opengl_windows.iter_mut() {
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
                if let Some(window) = opengl_windows
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
                let window_id = wc.window_id;
                self.close_popup_children(opengl_windows, xlib_app, window_id);

                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                cx.windows[window_id].is_created = false;
                if let Some(index) = opengl_windows.iter().position(|w| w.window_id == window_id) {
                    if let Some(xid) = opengl_windows[index].xlib_window.window {
                        unsafe {
                            xlib_app.release_popup_grab(xid);
                        }
                    }
                    opengl_windows.remove(index);
                    if opengl_windows.len() == 0 {
                        xlib_app.terminate_event_loop();
                        cx.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                }
            }
            XlibEvent::PopupDismissed(event) => {
                let mut cx = self.cx.borrow_mut();
                cx.call_event_handler(&Event::PopupDismissed(event));
            }
            XlibEvent::Paint => {
                {
                    let mut cx = self.cx.borrow_mut();
                    let time_now = xlib_app.time_now();
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

                self.handle_repaint(opengl_windows);
            }
            XlibEvent::MouseDown(e) => {
                let mut cx = self.cx.borrow_mut();
                cx.fingers.process_tap_count(e.abs, e.time);
                cx.fingers.mouse_down(e.button, e.window_id);
                cx.call_event_handler(&Event::MouseDown(e.into()))
            }
            XlibEvent::MouseMove(e) => {
                let mut cx = self.cx.borrow_mut();
                let abs = e.abs;
                let modifiers = e.modifiers;
                cx.call_event_handler(&Event::MouseMove(e.into()));
                if let Some(items) = self.internal_drag_items.as_ref() {
                    cx.call_event_handler(&Event::Drag(DragEvent {
                        modifiers,
                        handled: Arc::new(Mutex::new(false)),
                        abs,
                        items: items.clone(),
                        response: Arc::new(Mutex::new(DragResponse::None)),
                    }));
                    cx.drag_drop.cycle_drag();
                }
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
                cx.fingers.switch_captures();
            }
            XlibEvent::MouseUp(e) => {
                let mut cx = self.cx.borrow_mut();
                let button = e.button;
                let abs = e.abs;
                let modifiers = e.modifiers;
                cx.call_event_handler(&Event::MouseUp(e.into()));
                cx.fingers.mouse_up(button);
                cx.fingers.cycle_hover_area(live_id!(mouse).into());
                if button == MouseButton::PRIMARY {
                    if let Some(items) = self.internal_drag_items.take() {
                        cx.call_event_handler(&Event::Drop(DropEvent {
                            modifiers,
                            handled: Arc::new(Mutex::new(false)),
                            abs,
                            items,
                        }));
                        cx.drag_drop.cycle_drag();
                        cx.call_event_handler(&Event::DragEnd);
                        cx.drag_drop.cycle_drag();
                    }
                }
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
                    cx.handle_actions();
                    cx.handle_networking_events();

                    // Poll video players on the timer tick (every ~8ms).
                    if !cx.os.video_players.is_empty() {
                        cx.os.opengl_cx.as_ref().unwrap().make_current();
                        let gl: *const super::super::super::gl_sys::LibGl =
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

                cx.run_live_edit_if_needed("linux-x11");
                return EventFlow::Wait;
            }
        }

        //if self.any_passes_dirty() || self.need_redrawing() || paint_dirty {
        EventFlow::Poll
        //} else {
        //    EventFlow::Wait
        // }
    }

    pub(crate) fn handle_repaint(&mut self, opengl_windows: &mut Vec<OpenglWindow>) {
        let mut passes_todo = Vec::new();
        {
            let mut cx = self.cx.borrow_mut();
            cx.os.opengl_cx.as_ref().unwrap().make_current();
            cx.compute_pass_repaint_order(&mut passes_todo);
            cx.repaint_id += 1;
        }
        for draw_pass_id in &passes_todo {
            let parent = {
                let mut cx = self.cx.borrow_mut();
                cx.passes[*draw_pass_id].set_time(get_xlib_app_global().time_now() as f32);
                cx.passes[*draw_pass_id].parent.clone()
            };
            match parent {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(window) =
                        opengl_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        //let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers();

                        let egl_surface = window.egl_surface;

                        let pix_width =
                            window.window_geom.inner_size.x * window.window_geom.dpi_factor;
                        let pix_height =
                            window.window_geom.inner_size.y * window.window_geom.dpi_factor;
                        let mut cx = self.cx.borrow_mut();
                        cx.draw_pass_to_window(*draw_pass_id, egl_surface, pix_width, pix_height);
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    let mut cx = self.cx.borrow_mut();
                    cx.draw_pass_to_texture(*draw_pass_id, None);
                }
                CxDrawPassParent::None => {
                    let mut cx = self.cx.borrow_mut();
                    cx.draw_pass_to_texture(*draw_pass_id, None);
                }
            }
        }
    }

    fn close_popup_window(
        &mut self,
        opengl_windows: &mut Vec<OpenglWindow>,
        xlib_app: &mut XlibApp,
        window_id: crate::window::WindowId,
        reason: Option<PopupDismissReason>,
    ) {
        if let Some(index) = opengl_windows.iter().position(|w| w.window_id == window_id) {
            let mut cx = self.cx.borrow_mut();
            if let Some(reason) = reason {
                cx.call_event_handler(&Event::PopupDismissed(PopupDismissedEvent {
                    window_id,
                    reason,
                }));
            }
            cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
            cx.windows[window_id].is_created = false;
            if let Some(xid) = opengl_windows[index].xlib_window.window {
                unsafe {
                    xlib_app.release_popup_grab(xid);
                }
            }
            opengl_windows[index].xlib_window.close_window();
            opengl_windows.remove(index);
        }
    }

    fn close_popup_children(
        &mut self,
        opengl_windows: &mut Vec<OpenglWindow>,
        xlib_app: &mut XlibApp,
        parent_window_id: crate::window::WindowId,
    ) {
        loop {
            let child = opengl_windows
                .iter()
                .find(|w| w.xlib_window.popup_parent == Some(parent_window_id))
                .map(|w| w.window_id);
            if let Some(child_window_id) = child {
                self.close_popup_children(opengl_windows, xlib_app, child_window_id);
                self.close_popup_window(
                    opengl_windows,
                    xlib_app,
                    child_window_id,
                    Some(PopupDismissReason::ParentClosed),
                );
            } else {
                break;
            }
        }
    }

    fn handle_platform_ops(
        &mut self,
        opengl_windows: &mut Vec<OpenglWindow>,
        xlib_app: &mut XlibApp,
    ) -> EventFlow {
        let mut ret = EventFlow::Poll;
        let mut cx = self.cx.borrow_mut();
        while let Some(op) = cx.platform_ops.pop() {
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
                        window.is_fullscreen,
                    );
                    let window = &mut cx.windows[window_id];
                    window.window_geom = opengl_window.window_geom.clone();
                    opengl_windows.push(opengl_window);
                    window.is_created = true;
                }
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let gl_cx = cx.os.opengl_cx.as_ref().unwrap();
                    let opengl_window =
                        OpenglWindow::new_popup(window_id, parent_window_id, gl_cx, size, position);
                    let window = &mut cx.windows[window_id];
                    window.window_geom = opengl_window.window_geom.clone();
                    window.is_created = true;
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;
                    if let Some(xid) = opengl_window.xlib_window.window {
                        unsafe {
                            xlib_app.activate_popup_grab(xid, grab_keyboard);
                        }
                    }
                    opengl_windows.push(opengl_window);
                }
                CxOsOp::CloseWindow(window_id) => {
                    drop(cx);
                    self.close_popup_children(opengl_windows, xlib_app, window_id);
                    cx = self.cx.borrow_mut();

                    if let Some(index) =
                        opengl_windows.iter().position(|w| w.window_id == window_id)
                    {
                        if opengl_windows[index].xlib_window.is_popup {
                            drop(cx);
                            self.close_popup_window(opengl_windows, xlib_app, window_id, None);
                            cx = self.cx.borrow_mut();
                        } else {
                            cx.call_event_handler(&Event::WindowClosed(WindowClosedEvent {
                                window_id,
                            }));
                            cx.windows[window_id].is_created = false;
                            if let Some(xid) = opengl_windows[index].xlib_window.window {
                                unsafe {
                                    xlib_app.release_popup_grab(xid);
                                }
                            }
                            opengl_windows[index].xlib_window.close_window();
                            opengl_windows.remove(index);
                        }
                        if opengl_windows.len() == 0 {
                            ret = EventFlow::Exit
                        }
                    }
                }
                CxOsOp::Quit => ret = EventFlow::Exit,
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(window) =
                        opengl_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.xlib_window.minimize();
                    }
                }
                CxOsOp::Deminiaturize(_window_id) => todo!(),
                CxOsOp::HideWindow(_window_id) => todo!(),
                CxOsOp::HideWindowButtons(_) => {}
                CxOsOp::ShowWindowButtons(_) => {}
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(window) =
                        opengl_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.xlib_window.maximize();
                    }
                }
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(window) =
                        opengl_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.xlib_window.restore();
                    }
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    if let Some(window) =
                        opengl_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.xlib_window.set_inner_size(size);
                    }
                }
                CxOsOp::RepositionWindow(window_id, size) => {
                    if let Some(window) =
                        opengl_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.xlib_window.set_position(size);
                    }
                }
                CxOsOp::ShowClipboardActions { .. } => {}
                CxOsOp::CopyToClipboard(content) => {
                    if let Some(window) = opengl_windows.get(0) {
                        unsafe {
                            xlib_app.copy_to_clipboard(
                                &content,
                                window.xlib_window.window.unwrap(),
                                x11_sys::CurrentTime as u64,
                            )
                        }
                    }
                }
                CxOsOp::SetPrimarySelection(content) => {
                    if let Some(window) = opengl_windows.get(0) {
                        unsafe {
                            xlib_app.set_primary_selection(
                                &content,
                                window.xlib_window.window.unwrap(),
                                x11_sys::CurrentTime as u64,
                            )
                        }
                    }
                }
                CxOsOp::ShowSelectionHandles { .. } => {}
                CxOsOp::UpdateSelectionHandles { .. } => {}
                CxOsOp::HideSelectionHandles => {}
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::StartDragging(items) => {
                    self.internal_drag_items = Some(Arc::new(items));
                }
                CxOsOp::SetCursor(cursor) => {
                    xlib_app.set_mouse_cursor(cursor);
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    xlib_app.start_timer(timer_id, interval, repeats);
                }
                CxOsOp::StopTimer(timer_id) => {
                    xlib_app.stop_timer(timer_id);
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
                    let pos = area.clipped_rect(&cx).pos + pos;
                    opengl_windows.iter_mut().for_each(|w| {
                        w.xlib_window.set_ime_spot(pos);
                        w.xlib_window.set_ime_active(true);
                    });
                }
                CxOsOp::HideTextIME => {
                    opengl_windows.iter_mut().for_each(|w| {
                        w.xlib_window.set_ime_active(false);
                        w.xlib_window.set_ime_spot(dvec2(0.0, 0.0));
                    });
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                } => {
                    // Linux desktop apps have all permissions granted by default (handled at system level)
                    // TODO: Handle sandbox cases like flatpak
                    cx.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status: crate::permission::PermissionStatus::Granted,
                        },
                    ));
                }
                CxOsOp::RequestPermission {
                    permission,
                    request_id,
                } => {
                    // Linux desktop apps have all permissions granted by default (handled at system level)
                    // TODO: Handle sandbox cases like flatpak
                    cx.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status: crate::permission::PermissionStatus::Granted,
                        },
                    ));
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
                            VideoYuvTexturesReady {
                                video_id,
                                tex_y,
                                tex_u,
                                tex_v,
                            },
                        ));
                        continue;
                    }
                    // Try GStreamer first, fall back to software rav1d
                    let mut use_software =
                        std::env::var_os("MAKEPAD_FORCE_SOFTWARE_VIDEO").is_some();
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
                        match super::super::gstreamer_sys::LibGStreamer::try_load() {
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
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        ret
    }
}
