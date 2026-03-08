use {
    crate::{
        cx::*,
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        event::{
            game_input::*,
            video_playback::{
                VideoDecodingErrorEvent, VideoPlaybackCompletedEvent, VideoPlaybackPreparedEvent,
                VideoPlaybackResourcesReleasedEvent, VideoTextureUpdatedEvent,
                VideoYuvTexturesReady,
            },
            *,
        },
        game_input::*,
        makepad_live_id::*,
        texture::{Texture, TextureFormat},
        makepad_math::*,
        os::{
            cx_native::EventFlow,
            windows::{
                d3d11::{D3d11Cx, D3d11Window},
                win32_app::*,
                win32_event::*,
                win32_window::Win32Window,
                windows_game_input::WindowsGameInput,
                windows_media::CxWindowsMedia,
                windows_video_player::WindowsUnifiedVideoPlayer,
            },
        },
        //permission::{PermissionResult, PermissionStatus},
        thread::SignalToUI,
        window::CxWindowPool,
        windows::Win32::Graphics::Direct3D11::ID3D11Device,
    },
    std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant},
};

impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Windows;

        let d3d11_cx = Rc::new(RefCell::new(D3d11Cx::new()));

        // hack: store ID3D11Device in CxOs, so texture-related operations become possible on the makepad/studio side, yet don't completely destroy the code there
        cx.borrow_mut().os.d3d11_device = Some(d3d11_cx.borrow().device.clone());

        if crate::app_main::should_run_stdin_loop_from_env() {
            let mut cx = cx.borrow_mut();
            cx.in_makepad_studio = true;
            let mut d3d11_cx = d3d11_cx.borrow_mut();
            return cx.stdin_event_loop(&mut d3d11_cx);
        }

        let d3d11_windows = Rc::new(RefCell::new(Vec::new()));

        init_win32_app_global(Box::new({
            let cx = cx.clone();
            move |event| {
                let mut cx = cx.borrow_mut();
                let mut d3d11_cx = d3d11_cx.borrow_mut();
                let mut d3d11_windows = d3d11_windows.borrow_mut();
                cx.win32_event_callback(event, &mut d3d11_cx, &mut d3d11_windows)
            }
        }));
        // the signal poll timer
        with_win32_app(|app| app.start_timer(0, 0.008, true));
        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();
        with_win32_app(|app| app.start_signal_poll());
        Win32App::event_loop();
    }

    fn win32_event_callback(
        &mut self,
        event: Win32Event,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>,
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(d3d11_windows, d3d11_cx) {
            return EventFlow::Exit;
        }

        //let mut paint_dirty = false;
        /*match &event{
            Win32Event::Timer(time) =>{

            }
            _=>{}
        }*/

        //self.process_desktop_pre_event(&mut event);
        match event {
            Win32Event::WindowGotFocus(window_id) => {
                // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in d3d11_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                //paint_dirty = true;
                self.call_event_handler(&Event::WindowGotFocus(window_id));
            }
            Win32Event::WindowLostFocus(window_id) => {
                self.call_event_handler(&Event::WindowLostFocus(window_id));
            }
            Win32Event::PopupDismissed(event) => {
                self.call_event_handler(&Event::PopupDismissed(event));
            }
            Win32Event::WindowResizeLoopStart(window_id) => {
                if let Some(window) = d3d11_windows.iter_mut().find(|w| w.window_id == window_id) {
                    window.start_resize();
                }
            }
            Win32Event::WindowResizeLoopStop(window_id) => {
                if let Some(window) = d3d11_windows.iter_mut().find(|w| w.window_id == window_id) {
                    window.stop_resize();
                }
            }
            Win32Event::WindowGeomChange(mut re) => {
                // do this here because mac

                if let Some(window) = d3d11_windows
                    .iter_mut()
                    .find(|w| w.window_id == re.window_id)
                {
                    if let Some(dpi_override) = self.windows[re.window_id].dpi_override {
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
                self.redraw_all();
                self.call_event_handler(&Event::WindowGeomChange(re));
            }
            Win32Event::WindowClosed(wc) => {
                let window_id = wc.window_id;
                // Cascade-close any popup windows parented to this window
                let popup_ids: Vec<WindowId> = d3d11_windows
                    .iter()
                    .filter(|w| self.windows[w.window_id].popup_parent == Some(window_id))
                    .map(|w| w.window_id)
                    .collect();
                for popup_id in popup_ids {
                    self.call_event_handler(&Event::PopupDismissed(
                        crate::event::PopupDismissedEvent {
                            window_id: popup_id,
                            reason: crate::event::PopupDismissReason::ParentClosed,
                        },
                    ));
                    self.call_event_handler(&Event::WindowClosed(WindowClosedEvent {
                        window_id: popup_id,
                    }));
                    self.windows[popup_id].is_created = false;
                    if let Some(idx) = d3d11_windows.iter().position(|w| w.window_id == popup_id) {
                        d3d11_windows[idx].win32_window.close_window();
                        d3d11_windows.remove(idx);
                    }
                }
                self.call_event_handler(&Event::WindowClosed(wc));
                // lets remove the window from the set
                self.windows[window_id].is_created = false;
                if let Some(index) = d3d11_windows.iter().position(|w| w.window_id == window_id) {
                    d3d11_windows.remove(index);
                    if d3d11_windows.len() == 0 {
                        self.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                }
            }
            Win32Event::Paint => {
                // Poll video players for new frames
                if !self.os.video_players.is_empty() {
                    let mut players = std::mem::take(&mut self.os.video_players);
                    let mut video_events = Vec::new();
                    for (_id, player) in players.iter_mut() {
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
                                        video_id: player.video_id,
                                        video_width: width,
                                        video_height: height,
                                        duration,
                                        is_seekable,
                                        video_tracks,
                                        audio_tracks,
                                    },
                                ));
                            }
                            Some(Err(err)) => {
                                video_events.push(Event::VideoDecodingError(
                                    VideoDecodingErrorEvent {
                                        video_id: player.video_id,
                                        error: err,
                                    },
                                ));
                            }
                            None => {}
                        }
                        if player.poll_frame(&mut self.textures) {
                            video_events.push(Event::VideoTextureUpdated(
                                VideoTextureUpdatedEvent {
                                    video_id: player.video_id,
                                    current_position_ms: player.current_position_ms(),
                                    yuv: crate::event::video_playback::VideoYuvMetadata {
                                        enabled: player.is_software_mode(),
                                        matrix: player.yuv_matrix(),
                                        biplanar: false,
                                        rotation_steps: 0.0,
                                    },
                                },
                            ));
                        }
                        if player.check_eos() {
                            video_events.push(Event::VideoPlaybackCompleted(
                                VideoPlaybackCompletedEvent {
                                    video_id: player.video_id,
                                },
                            ));
                        }
                    }
                    let needs_repaint = players.values().any(|p| p.is_playing());
                    self.os.video_players = players;
                    for event in video_events {
                        self.call_event_handler(&event);
                    }
                    // Keep paint loop alive while any player is actively playing
                    if needs_repaint {
                        self.new_next_frame();
                    }
                }

                let time_now = with_win32_app(|app| app.time_now());
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(time_now);
                }
                if self.need_redrawing() {
                    self.call_draw_event(time_now);
                    self.hlsl_compile_shaders(&d3d11_cx);
                }
                // ok here we send out to all our childprocesses

                self.handle_repaint(d3d11_windows, d3d11_cx);
            }
            Win32Event::MouseDown(e) => {
                self.fingers.process_tap_count(e.abs, e.time);
                self.fingers.mouse_down(e.button, e.window_id);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            Win32Event::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            Win32Event::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            Win32Event::MouseLeave(e) => {
                self.call_event_handler(&Event::MouseLeave(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            Win32Event::Scroll(e) => self.call_event_handler(&Event::Scroll(e.into())),
            Win32Event::WindowDragQuery(e) => self.call_event_handler(&Event::WindowDragQuery(e)),
            Win32Event::WindowCloseRequested(e) => {
                self.call_event_handler(&Event::WindowCloseRequested(e))
            }
            Win32Event::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
            Win32Event::Drag(e) => {
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            }
            Win32Event::Drop(e) => {
                self.call_event_handler(&Event::Drop(e));
                self.drag_drop.cycle_drag();
            }
            Win32Event::DragEnd => {
                // send MouseUp
                self.call_event_handler(&Event::MouseUp(MouseUpEvent {
                    abs: dvec2(-100000.0, -100000.0),
                    button: MouseButton::PRIMARY,
                    window_id: CxWindowPool::id_zero(),
                    modifiers: Default::default(),
                    time: 0.0,
                }));
                self.fingers.mouse_up(MouseButton::PRIMARY);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            Win32Event::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            Win32Event::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            Win32Event::TextCopy(e) => self.call_event_handler(&Event::TextCopy(e)),
            Win32Event::TextCut(e) => self.call_event_handler(&Event::TextCut(e)),
            Win32Event::Timer(e) => {
                self.handle_script_timer(&e);
                self.call_event_handler(&Event::Timer(e))
            }
            Win32Event::Signal => {
                if SignalToUI::check_and_clear_ui_signal() {
                    self.handle_media_signals();
                    self.handle_script_signals();
                    self.call_event_handler(&Event::Signal);
                }
                if SignalToUI::check_and_clear_action_signal() {
                    self.handle_action_receiver();
                }

                self.run_live_edit_if_needed("windows");
                self.handle_networking_events();

                self.win32_event_callback(Win32Event::Paint, d3d11_cx, d3d11_windows);

                return EventFlow::Wait;
            }
        }

        self.handle_game_input_events();

        return EventFlow::Poll;
        /*
        if self.any_passes_dirty() || self.need_redrawing() || self.new_next_frames.len() != 0 || paint_dirty {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }*/
    }

    pub(crate) fn handle_repaint(
        &mut self,
        d3d11_windows: &mut Vec<D3d11Window>,
        d3d11_cx: &mut D3d11Cx,
    ) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for draw_pass_id in &passes_todo {
            self.passes[*draw_pass_id].set_time(with_win32_app(|app| app.time_now() as f32));
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        //let dpi_factor = window.window_geom.dpi_factor;
                        window.resize_buffers(&d3d11_cx);
                        self.draw_pass_to_window(*draw_pass_id, false, window, d3d11_cx);
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(*draw_pass_id, d3d11_cx, None);
                }
                CxDrawPassParent::None => {
                    self.draw_pass_to_texture(*draw_pass_id, d3d11_cx, None);
                }
            }
        }
    }

    pub(crate) fn handle_networking_events(&mut self) {
        self.dispatch_network_runtime_events();
    }

    pub(crate) fn handle_game_input_events(&mut self) {
        while let Ok(event) = self.os.game_input_events.receiver.try_recv() {
            self.call_event_handler(&Event::GameInputConnected(event));
        }

        // Poll for new events and state updates
        let mut events = Vec::new();
        if let Some(game_input) = &mut self.os.windows_game_input {
            game_input.poll(|event| {
                events.push(event);
            });
        }

        for event in events {
            self.os.game_input_events.sender.send(event).unwrap();
        }
        // Force a repaint if any gamepad buttons are pressed?
        // Or just let the signal loop handle it.
        // For now, we rely on the standard event loop polling.
    }

    fn handle_platform_ops(
        &mut self,
        d3d11_windows: &mut Vec<D3d11Window>,
        d3d11_cx: &D3d11Cx,
    ) -> EventFlow {
        let mut ret = EventFlow::Poll;
        let mut geom_changes = Vec::new();
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let d3d11_window = D3d11Window::new(
                        window_id,
                        &d3d11_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        window.is_fullscreen,
                    );

                    window.window_geom = d3d11_window.window_geom.clone();
                    d3d11_windows.push(d3d11_window);
                    window.is_created = true;
                    geom_changes.push(WindowGeomChangeEvent {
                        window_id,
                        old_geom: window.window_geom.clone(),
                        new_geom: window.window_geom.clone(),
                    });
                }
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let window = &mut self.windows[window_id];
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;

                    // Convert parent-relative position to screen coordinates
                    let screen_position = if let Some(parent_d3d11) = d3d11_windows
                        .iter()
                        .find(|w| w.window_id == parent_window_id)
                    {
                        let parent_pos = parent_d3d11.win32_window.get_position();
                        let parent_dpi = parent_d3d11.win32_window.get_dpi_factor();
                        // parent_pos is already in screen pixels / dpi, position is in logical coords
                        dvec2(
                            parent_pos.x + position.x * parent_dpi,
                            parent_pos.y + position.y * parent_dpi,
                        )
                    } else {
                        position
                    };

                    let d3d11_window =
                        D3d11Window::new_popup(window_id, &d3d11_cx, size, screen_position);
                    window.window_geom = d3d11_window.window_geom.clone();
                    d3d11_windows.push(d3d11_window);
                    window.is_created = true;
                    geom_changes.push(WindowGeomChangeEvent {
                        window_id,
                        old_geom: window.window_geom.clone(),
                        new_geom: window.window_geom.clone(),
                    });
                }
                CxOsOp::CloseWindow(window_id) => {
                    self.call_event_handler(&Event::WindowClosed(WindowClosedEvent { window_id }));
                    if let Some(index) = d3d11_windows.iter().position(|w| w.window_id == window_id)
                    {
                        self.windows[window_id].is_created = false;
                        d3d11_windows[index].win32_window.close_window();
                        d3d11_windows.remove(index);
                        if d3d11_windows.len() == 0 {
                            self.call_event_handler(&Event::Shutdown);
                            ret = EventFlow::Exit
                        }
                    }
                }
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.minimize();
                    }
                }
                CxOsOp::Deminiaturize(_window_id) => todo!(),
                CxOsOp::HideWindow(_window_id) => todo!(),
                CxOsOp::HideWindowButtons(_) => {}
                CxOsOp::ShowWindowButtons(_) => {}
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.maximize();
                    }
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.set_inner_size(size);
                    }
                }
                CxOsOp::RepositionWindow(window_id, pos) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.set_position(pos);
                    }
                }
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.restore();
                    }
                }
                CxOsOp::Quit => ret = EventFlow::Exit,
                CxOsOp::SetTopmost(window_id, is_topmost) => {
                    if d3d11_windows.len() == 0 {
                        self.platform_ops
                            .insert(0, CxOsOp::SetTopmost(window_id, is_topmost));
                        continue;
                    }
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.set_topmost(is_topmost);
                    }
                }
                CxOsOp::CopyToClipboard(content) => unsafe {
                    Win32Window::copy_to_clipboard(&content);
                },
                CxOsOp::SetPrimarySelection(_) => {}
                CxOsOp::ShowSelectionHandles { .. } => {}
                CxOsOp::UpdateSelectionHandles { .. } => {}
                CxOsOp::HideSelectionHandles => {}
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::SetCursor(cursor) => {
                    with_win32_app(|app| app.set_mouse_cursor(cursor));
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    with_win32_app(|app| app.start_timer(timer_id, interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    with_win32_app(|app| app.stop_timer(timer_id));
                }
                CxOsOp::StartDragging(dragged_item) => {
                    with_win32_app(|app| app.start_dragging(dragged_item));
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::ShowTextIME(area, pos, _config) => {
                    let pos = area.clipped_rect(self).pos + pos;
                    d3d11_windows.iter_mut().for_each(|w| {
                        w.win32_window.set_ime_active(true);
                        w.win32_window.set_ime_spot(pos);
                    });
                }
                CxOsOp::HideTextIME => {
                    d3d11_windows.iter_mut().for_each(|w| {
                        w.win32_window.set_ime_active(false);
                        w.win32_window.set_ime_spot(Vec2d::default());
                    });
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                } => {
                    // Windows desktop apps have all permissions granted by default
                    self.call_event_handler(&Event::PermissionResult(
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
                    // Windows desktop apps have all permissions granted by default
                    self.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission,
                            request_id,
                            status: crate::permission::PermissionStatus::Granted,
                        },
                    ));
                }
                // Mobile-only ops (soft keyboard, clipboard UI); no-op on desktop
                CxOsOp::SyncImeState { .. } => {}
                CxOsOp::ShowClipboardActions { .. } => {}
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
                    if self.os.video_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(ref device) = self.os.d3d11_device {
                        // Allocate YUV textures internally for software decode path
                        let tex_y = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_u = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_v = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_y_id = tex_y.texture_id();
                        let tex_u_id = tex_u.texture_id();
                        let tex_v_id = tex_v.texture_id();
                        let player = WindowsUnifiedVideoPlayer::new(
                            device,
                            video_id,
                            texture_id,
                            tex_y_id,
                            tex_u_id,
                            tex_v_id,
                            source,
                            autoplay,
                            should_loop,
                        );
                        self.os.video_players.insert(video_id, player);
                        // Notify widget so it can bind textures to shader slots
                        self.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady {
                                video_id,
                                tex_y,
                                tex_u,
                                tex_v,
                            },
                        ));
                    } else {
                        self.call_event_handler(&Event::VideoDecodingError(
                            VideoDecodingErrorEvent {
                                video_id,
                                error: "D3D11 device unavailable for Windows video playback"
                                    .to_string(),
                            },
                        ));
                        crate::error!(
                            "VIDEO: PrepareVideoPlayback skipped for {:?}: missing D3D11 device",
                            video_id
                        );
                    }
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.play();
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.pause();
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.resume();
                    }
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.mute();
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.unmute();
                    }
                }
                CxOsOp::CleanupVideoPlaybackResources(video_id) => {
                    if let Some(mut player) = self.os.video_players.remove(&video_id) {
                        player.cleanup();
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                    }
                }
                CxOsOp::SeekVideoPlayback(video_id, position_ms) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.seek_to(position_ms);
                    }
                }
                CxOsOp::SetVideoVolume(video_id, volume) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.set_volume(volume);
                    }
                }
                CxOsOp::SetVideoPlaybackRate(video_id, rate) => {
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.set_playback_rate(rate);
                    }
                }
                CxOsOp::AttachCameraNativePreview { .. }
                | CxOsOp::UpdateCameraNativePreview { .. }
                | CxOsOp::DetachCameraNativePreview { .. } => {
                    // Native camera preview is emulated via composited texture path on Windows.
                }
                CxOsOp::PrepareAudioPlayback(_, _, _, _) => {
                    // TODO: implement Windows audio-only playback
                }
                CxOsOp::UpdateVideoSurfaceTexture(_) => {
                    // Android-only, no-op on Windows
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        if geom_changes.len() > 0 {
            self.redraw_all();
            for geom_change in geom_changes {
                self.call_event_handler(&Event::WindowGeomChange(geom_change));
            }
        }
        ret
    }
}

impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState> {
        if let Some(game_input) = &self.os.windows_game_input {
            if index < game_input.states.len() {
                return Some(&game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        if let Some(game_input) = &self.os.windows_game_input {
            return &game_input.states;
        }
        &[]
    }

    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState> {
        if let Some(game_input) = &mut self.os.windows_game_input {
            if index < game_input.states.len() {
                return Some(&mut game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        if let Some(game_input) = &mut self.os.windows_game_input {
            return &mut game_input.states;
        }
        &mut []
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(_item) = std::option_env!("MAKEPAD_PACKAGE_DIR") {
            //    self.live_registry.borrow_mut().package_root = Some(item.to_string());
        }

        //self.live_expand();
        //if std::env::args().find( | v | v == "--stdin-loop").is_none() {
        //    self.start_disk_live_file_watcher(100);
        //}
        //self.live_scan_dependencies();
        self.native_load_dependencies();

        self.os.windows_game_input = Some(WindowsGameInput::init());
    }

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(f);
    }

    fn seconds_since_app_start(&self) -> f64 {
        Instant::now()
            .duration_since(self.os.start_time.unwrap())
            .as_secs_f64()
    }

    fn open_url(&mut self, _url: &str, _in_place: OpenUrlInPlace) {
        crate::error!("open_url not implemented on this platform");
    }
}

#[derive(Default)]
pub struct CxOs {
    pub(crate) start_time: Option<Instant>,
    pub(crate) media: CxWindowsMedia,
    pub(crate) d3d11_device: Option<ID3D11Device>,
    pub(crate) game_input_events: GameInputEventChannel,
    pub(crate) windows_game_input: Option<WindowsGameInput>,
    pub(crate) video_players: HashMap<LiveId, WindowsUnifiedVideoPlayer>,
}
