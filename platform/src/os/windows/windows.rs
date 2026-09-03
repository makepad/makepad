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
        texture::{Texture, TextureFormat},
        //permission::{PermissionResult, PermissionStatus},
        thread::SignalToUI,
        window::{CxWindowPool, WindowId},
        windows::Win32::Graphics::Direct3D11::ID3D11Device,
    },
    std::{cell::RefCell, collections::HashMap, rc::Rc, time::{Duration, Instant}},
};

impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Windows;

        let d3d11_cx = Rc::new(RefCell::new(D3d11Cx::new()));

        // hack: store ID3D11Device in CxOs, so texture-related operations become possible on the makepad/studio side, yet don't completely destroy the code there
        cx.borrow_mut().os.d3d11_device = Some(d3d11_cx.borrow().device.clone());
        cx.borrow_mut().publish_d3d11_device_for_media();

        cx.borrow_mut().set_physical_keyboard_state(true);
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
        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();
        // The 8 ms signal-poll heartbeat. This used to be armed TWICE — once as
        // `start_timer(0, 0.008, true)` (which maps id 0 onto a SignalPoll timer)
        // and once here — so every idle tick ran the whole signal/action/network
        // drain twice and posted two WM_TIMERs.
        with_win32_app(|app| app.start_signal_poll());
        Win32App::event_loop();
    }

    fn win32_event_callback(
        &mut self,
        event: Win32Event,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>,
    ) -> EventFlow {
        // Before anything touches the GPU. This is the one place holding both `&mut D3d11Cx`
        // and `&mut Vec<D3d11Window>` exclusively while nothing is mid-render — the wndproc
        // queues re-entrant events, `handle_platform_ops` only borrows the Cx immutably, and
        // `present` runs with the passes and the window list already borrowed.
        self.inject_test_device_loss(d3d11_cx);
        if d3d11_cx.device_lost.get() {
            self.recover_lost_d3d11_device(d3d11_cx, d3d11_windows);
        }
        if let EventFlow::Exit = self.handle_platform_ops(d3d11_windows, d3d11_cx) {
            self.call_event_handler(&Event::Shutdown);
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
                    {
                        let cx_window = &mut self.windows[re.window_id];
                        cx_window.os_dpi_factor = Some(re.new_geom.dpi_factor);
                        re.new_geom = cx_window.native_window_geom_to_layout(re.new_geom);
                    }

                    window.window_geom = re.new_geom.clone();
                    self.windows[re.window_id].window_geom = re.new_geom.clone();
                }
                // Redraw just this window's pass tree (size or DPI — a DPI-only
                // change still needs a pass rebuild at the new physical scale).
                // This used to be followed by an unconditional `redraw_all()`,
                // which rebuilt every OTHER window's whole widget tree on every
                // WM_SIZE/WM_MOVE of one of them — during a drag-resize that is a
                // full re-layout of the entire app per mouse sample.
                if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                    self.redraw_pass_and_child_passes(main_pass_id);
                }
                self.call_event_handler(&Event::WindowGeomChange(re));
            }
            Win32Event::WindowClosed(wc) => {
                // This WM_DESTROY-generated event reaches this arm exactly once on every
                // close path; no other code may synthesize a WindowClosed, or the app
                // would see it twice.
                let window_id = wc.window_id;
                // Cascade-close popups parented to this window. Their own WindowClosed
                // arrives via the queued authentic event; removing the D3d11Window now
                // makes the app's `WindowHandle::close()` response a no-op.
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
                    if let Some(index) = d3d11_windows.iter().position(|w| w.window_id == popup_id)
                    {
                        self.windows[popup_id].is_created = false;
                        d3d11_windows[index].win32_window.close_window();
                        d3d11_windows.remove(index);
                    }
                }
                // `close_window` (behind `CxOsOp::CloseWindow`) clears
                // `is_created` *before* calling `DestroyWindow`, so a window
                // still marked created at this point was torn down by
                // something other than the app — combined with the WM_CLOSE
                // accept above, that is the human dismissing it. Say so on
                // stdout so an agent tailing the log does not read a
                // deliberate close as an unexplained death (mirrors
                // macos.rs's `MacosEvent::WindowClosed` handling).
                let user_closed = crate::remote::take_window_close_requested(window_id.id())
                    || self.windows[window_id].is_created;
                let title = self.windows[window_id].create_title.clone();
                self.call_event_handler(&Event::WindowClosed(wc));
                // Remove the window; tolerate CxOsOp::CloseWindow having removed it already.
                self.windows[window_id].is_created = false;
                if user_closed {
                    crate::remote::note_user_closed_window(window_id.id(), &title);
                }
                if let Some(index) = d3d11_windows.iter().position(|w| w.window_id == window_id) {
                    d3d11_windows.remove(index);
                }
                // The main pass can no longer be painted; clear its dirty flag so
                // `any_passes_dirty()` cannot keep the loop in Poll forever.
                if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
                    self.passes[main_pass_id].paint_dirty = false;
                }
                // Exit once the last window is gone, but not while another WindowClosed
                // is still queued (the app must see every WindowClosed before Shutdown)
                // or a CreateWindow op is pending (the app is not actually windowless).
                if d3d11_windows.is_empty()
                    && !with_win32_app(|app| {
                        app.pending_events
                            .iter()
                            .any(|e| matches!(e, Win32Event::WindowClosed(_)))
                    })
                    && !self.platform_ops.iter().any(|op| {
                        matches!(
                            op,
                            CxOsOp::CreateWindow(_) | CxOsOp::CreatePopupWindow { .. }
                        )
                    })
                {
                    if user_closed {
                        crate::remote::note_user_closed_last_window();
                    }
                    self.call_event_handler(&Event::Shutdown);
                    return EventFlow::Exit;
                }
            }
            Win32Event::Beat {
                window_id,
                time,
                primary,
            } => {
                // One window's frame-latency waitable fired: the compositor retired
                // a present and is ready for that window's next frame. Aim the tick
                // at the flip it will actually be shown on, not at "now" — that is
                // what makes animation advance in even steps instead of by however
                // long this particular tick happened to take.
                let flip_time = d3d11_windows
                    .iter_mut()
                    .find(|w| w.window_id == window_id)
                    .map(|w| w.target_present_time(time))
                    .unwrap_or(time);
                // The primary window owns the app clock. If it can no longer beat
                // (minimized, occluded, device lost) its tick would simply never
                // run, and every animation would freeze while a secondary window
                // keeps flipping — so hand the full tick to whoever is still
                // beating.
                let primary_window =
                    with_win32_app(|app| app.beat_handles.first().map(|b| b.window_id));
                let primary_alive = primary_window.is_some_and(|pid| {
                    pid == window_id
                        || d3d11_windows.iter().any(|w| {
                            w.window_id == pid
                                && !w.device_lost
                                && w.occluded_since.is_none()
                                && !w.win32_window.is_iconic()
                        })
                });
                let primary = primary || !primary_alive;
                with_win32_app(|app| app.frame_trace.flip_lead(time, flip_time));
                self.os.link_scope = Some(window_id);
                self.os.link_flip_time = Some(flip_time);
                // The primary window's beat drives the WHOLE tick (video, next
                // frames, draw, paint) — the same work as the unscoped Paint, just
                // clocked by the flip. A secondary window's beat paints only its own
                // pass tree: advancing the animation clock once per window per
                // refresh would double-step every animation in a multi-window app.
                self.paint_tick(flip_time, primary, d3d11_cx, d3d11_windows);
                self.os.link_scope = None;
                self.os.link_flip_time = None;
                // If nothing was painted for this window (its pass was clean, or it
                // is occluded) the credit taken by the wait stays held: the beat
                // simply drops out of the wait list until a frame is presented,
                // since the compositor is already waiting for one. See
                // `BeatSource::credit_held` — the credit cannot be handed back.
            }
            Win32Event::Paint => {
                // The unscoped tick: no window flip is driving it (a resize/drag
                // heartbeat, a geometry echo, or the beat's wait timing out because
                // nothing is being composited). Paint every dirty pass and stamp the
                // frame with wall-now.
                let time_now = with_win32_app(|app| app.time_now());
                self.paint_tick(time_now, true, d3d11_cx, d3d11_windows);
            }
            Win32Event::MouseDown(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.fingers.process_tap_count(e.abs, e.time);
                self.fingers.mouse_down(e.button, e.window_id);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            Win32Event::MouseMove(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            Win32Event::MouseUp(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            Win32Event::MouseLeave(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::MouseLeave(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            Win32Event::Scroll(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::Scroll(e.into()))
            }
            Win32Event::WindowDragQuery(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            Win32Event::WindowCloseRequested(e) => {
                // WM_CLOSE only ever reaches here for a native close (the
                // close button, Alt-F4, or the system menu) — `close_window`
                // (behind `CxOsOp::CloseWindow`) calls `DestroyWindow`
                // directly and never sends WM_CLOSE. So an accepted request
                // here is the human dismissing the window; remember it, and
                // report it when the close actually lands (mirrors
                // macos.rs's `windowShouldClose:` handling — see
                // `note_window_close_requested`).
                let window_id = e.window_id;
                let accept_close = e.accept_close.clone();
                self.call_event_handler(&Event::WindowCloseRequested(e));
                if accept_close.get() {
                    crate::remote::note_window_close_requested(window_id.id());
                }
            }
            Win32Event::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
            Win32Event::Drag(window_id, mut e) => {
                self.dpi_override_scale(&mut e.abs, window_id);
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            }
            Win32Event::Drop(window_id, mut e) => {
                self.dpi_override_scale(&mut e.abs, window_id);
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
                    self.handle_termination_signal();
                    self.handle_media_signals();
                    self.handle_script_signals();
                    self.call_event_handler(&Event::Signal);
                }
                if SignalToUI::check_and_clear_action_signal() {
                    self.handle_action_receiver();
                }
                self.poll_control_channel();
                // A `--remote` grab arrives here (the control channel is polled on
                // this tick) and can only be answered by a pass that renders. Dirty
                // the window passes so the next tick paints one — the same thing the
                // macOS timer handler does for `screenshot_requests`.
                if !self.screenshot_requests.is_empty() {
                    self.repaint_windows();
                }

                self.run_live_edit_if_needed("windows");
                self.handle_networking_events();

                // Drain platform_ops queued by the signal handlers above (e.g. a
                // `CxOsOp::Quit` pushed by `handle_termination_signal`) so Ctrl+C /
                // SIGTERM still terminate the process. Unlike the old code this must
                // NOT unconditionally repaint: an idle 8ms signal-poll tick with
                // nothing dirty should do zero GPU work (the old recursive `Paint`
                // here was the ~125 Hz idle repaint that dominated CPU). This is the
                // same drain that runs at the top of every callback and returns
                // `Exit` on `Quit`.
                if let EventFlow::Exit = self.handle_platform_ops(d3d11_windows, d3d11_cx) {
                    self.call_event_handler(&Event::Shutdown);
                    return EventFlow::Exit;
                }

                // Poll connected game controllers on the signal-poll tick so gamepad input is
                // serviced even while the app is otherwise idle: a controller produces no Win32
                // message, so nothing else would call this and a button press could not wake the
                // loop. Any resulting redraw/animation is picked up by the resume check below.
                self.handle_game_input_events();

                // If a signal handler dirtied the UI (redraw / animation / dirty pass),
                // or video is playing, resume the vsync-paced Poll loop so it paints
                // promptly; otherwise go back to sleep in `GetMessageW`.
                // Video must keep Poll: Wait skips Paint on signal-poll ticks.
                // A lost device makes every one of those conditions unsatisfiable: nothing can
                // paint, so `Poll` would spin at the loop's full rate for the whole outage —
                // which can be hours with a lid shut. Sleep instead and let the signal-poll
                // heartbeat deliver the retries.
                if d3d11_cx.device_lost.get() {
                    return EventFlow::Wait;
                }
                if self.any_passes_dirty()
                    || self.need_redrawing()
                    || self.new_next_frames.len() != 0
                    || !self.screenshot_requests.is_empty()
                    || self.os.video_players.values().any(|p| p.keep_polling())
                {
                    return EventFlow::Poll;
                }
                return EventFlow::Wait;
            }
        }

        self.handle_game_input_events();

        // Pace painting like macOS/Linux: spin (Poll) only while there is visible work
        // pending — a pass is dirty, a redraw was requested, an animation NextFrame
        // is queued, or a video player is preparing/playing. Otherwise block (Wait) so
        // the loop sleeps in `GetMessageW` at ~0% CPU until the next input / timer / signal.
        // While Poll-ing, the vsync-blocking D3D11 `Present` (`handle_repaint` ->
        // `draw_pass_to_window` -> `Present(1,..)`) is what actually paces frames to
        // the display; this replaces the old hard-forced Poll that repainted
        // unconditionally at the 8 ms signal-timer rate (~125 Hz).
        // A lost device makes all of those unsatisfiable — nothing can paint until it is
        // rebuilt — so `Poll` would spin at the loop's full rate for the whole outage.
        if d3d11_cx.device_lost.get() {
            return EventFlow::Wait;
        }
        if self.any_passes_dirty()
            || self.need_redrawing()
            || self.new_next_frames.len() != 0
            // A pending screenshot must never be left asleep in `GetMessageW`:
            // nothing else would wake the loop to render the frame it needs.
            || !self.screenshot_requests.is_empty()
            || self.os.video_players.values().any(|p| p.keep_polling())
        {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
    }

    /// One paint tick: advance the frame, redraw what is dirty, and present.
    ///
    /// `time_now` is the timestamp the WHOLE frame is stamped with — the beat
    /// passes the flip this frame is aimed at, the unscoped fallbacks pass
    /// wall-now. `full` distinguishes the tick that owns the app clock (video
    /// polling and the NextFrame advance) from a secondary window's beat, which
    /// only redraws and presents its own pass tree.
    fn paint_tick(
        &mut self,
        time_now: f64,
        full: bool,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>,
    ) {
        // Poll video players for new frames
        if full && !self.os.video_players.is_empty() {
            let mut players = std::mem::take(&mut self.os.video_players);
            let mut video_events = Vec::new();
            for (_id, player) in players.iter_mut() {
                player.sync_worker();
                match player.check_prepared() {
                    Some(Ok(crate::media_plugin::PlaybackPrepared {
                        width,
                        height,
                        duration_ms: duration,
                        is_seekable,
                        video_tracks,
                        audio_tracks,
                    })) => {
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
                                enabled: player.uses_yuv(),
                                matrix: player.yuv_matrix(),
                                biplanar: player.yuv_biplanar(),
                                full_range: player.yuv_full_range(),
                                rotation_steps: 0.0,
                                external: false,
                                array: player.yuv_array(),
                            },
                        rgba_gl_2d: false,
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
            let needs_repaint = players.values().any(|p| p.keep_polling());
            self.os.video_players = players;
            for event in video_events {
                self.call_event_handler(&event);
            }
            // Keep the paint loop alive while preparing or playing.
            // Arm *before* next-frame dispatch so widgets can observe it, then
            // re-arm *after* — `call_next_frame_event` consumes the set, and without
            // a re-arm a 30fps stream on a 60Hz display drops into `EventFlow::Wait`
            // on the empty half of the ticks. Wait mode skips Paint on the 8ms
            // signal-poll timer, so the video freezes until the next mouse/input
            // message wakes GetMessageW.
            if needs_repaint {
                self.new_next_frame();
            }
        }
        // Only the tick that owns the app clock advances animations: a secondary
        // window's beat fires once per ITS refresh, and stepping NextFrame there
        // too would run every animation at N× speed in a multi-window app.
        if full {
            if self.new_next_frames.len() != 0 {
                with_win32_app(|app| app.frame_trace.next_frame(time_now));
                self.call_next_frame_event(time_now);
            }
            if self.os.video_players.values().any(|p| p.keep_polling()) {
                self.new_next_frame();
            }
        }
        if self.need_redrawing() {
            self.call_draw_event(time_now);
            self.hlsl_compile_shaders(&d3d11_cx);
        }
        // ok here we send out to all our childprocesses

        let presented = self.handle_repaint(d3d11_windows, d3d11_cx);
        // A presenting pass blocks in the frame-latency wait or Present, pacing
        // the Poll loop to the display. A pass that presents nothing has no
        // blocking call at all, so a NextFrame listener that re-arms without
        // dirtying a pass (e.g. a video player polling between decoded frames)
        // would spin the loop at full speed; sleep briefly to cap that.
        // `any_passes_dirty` also paces a popup's waitless dropped-present retry.
        // While video is preparing/playing we keep re-arming NextFrame so Poll
        // does not drop into Wait; pace that like the 8 ms signal-poll timer.
        // A window that cannot reach glass at all (minimized, hidden, or in a
        // session the compositor has abandoned — a disconnected RDP desktop
        // reports every present as DXGI_STATUS_OCCLUDED) presents nothing
        // either, and its beat never signals; the 1 ms retry then spun this
        // loop at ~600 Hz, stepping every NextFrame animation every 1.6 ms for
        // nothing (measured: `MAKEPAD_TRACE=frames`, ticks/2s: drain=1265,
        // next_frame gap 0-4 ms). Pace that like video and the idle beat
        // timeout — 8 ms, the same cadence a hidden window keeps on macOS —
        // and leave the 1 ms retry to a window that is on screen and merely
        // dropped a frame.
        if !presented {
            let video_pacing = self.os.video_players.values().any(|p| p.keep_polling());
            let nothing_can_present = !d3d11_windows.is_empty()
                && d3d11_windows.iter().all(|w| {
                    w.device_lost || w.occluded_since.is_some() || w.win32_window.is_iconic()
                });
            if !self.new_next_frames.is_empty() || self.any_passes_dirty() || video_pacing {
                let ms = if video_pacing || nothing_can_present { 8 } else { 1 };
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
        // Tell the beat how long it may block. The frame-latency waitable is a
        // credit semaphore refilled by retired presents, so a stretch of ticks
        // that present nothing would leave it unsignaled and the beat would sit
        // out its full timeout; drop to the 8 ms heartbeat rate for those, which
        // is exactly the cadence this work had before the beat existed.
        with_win32_app(|app| {
            app.beat_timeout_ms = if presented {
                BEAT_TIMEOUT_PRESENTED_MS
            } else {
                BEAT_TIMEOUT_IDLE_MS
            };
            let now = app.time_now();
            app.frame_trace.maybe_print(now);
        });

        // Run script-VM garbage collection at a safe point after paint, matching
        // the macOS backend, so the script object heap doesn't grow without bound:
        // every `eval` / `script_apply_eval!` allocates script objects that are
        // only reclaimed by `gc()`. `needs_gc()` gates the actual sweep.
        if full {
            self.with_vm(|vm| {
                if vm.heap().needs_gc() {
                    vm.gc();
                }
            });
        }
    }

    /// Repaints all dirty passes. Returns whether any window pass actually presented a
    /// Fault injection for the recovery path: `MAKEPAD_D3D11_TEST_DEVICE_LOSS=<seconds>` trips
    /// the loss latch every that many seconds and forces a full device recreation.
    ///
    /// A real device removal needs a driver reset, which cannot be provoked from inside the
    /// process, so this stands in for it. It is a stronger test than merely setting the latch:
    /// the device really is replaced, so every GPU object the sweep fails to rebuild still
    /// belongs to the old device and will not render against the new one. What it cannot cover
    /// is the detection itself, which only a genuine `DXGI_ERROR_DEVICE_REMOVED` exercises.
    fn inject_test_device_loss(&mut self, d3d11_cx: &D3d11Cx) {
        static PERIOD: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
        let Some(period) = PERIOD.get_or_init(|| {
            std::env::var("MAKEPAD_D3D11_TEST_DEVICE_LOSS")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|secs| *secs > 0.0)
                .map(Duration::from_secs_f64)
        }) else {
            return;
        };
        let now = Instant::now();
        let due = *self.os.d3d11_test_loss_next.get_or_insert(now + *period);
        if now < due {
            return;
        }
        self.os.d3d11_test_loss_next = Some(now + *period);
        self.os.d3d11_force_recreate = true;
        d3d11_cx.device_lost.set(true);
        crate::log!("MAKEPAD_D3D11_TEST_DEVICE_LOSS: forcing a device loss now");
    }

    /// Rebuilds the D3D11 device and everything created from it after the device was removed
    /// or reset — a GPU driver restart, a TDR, a driver update, or the hybrid-GPU transition a
    /// laptop makes across suspend/resume.
    ///
    /// Retries are driven by whatever event next reaches the loop rather than by a timer, and
    /// spaced by a backoff, because the GPU can stay absent for a long time: a lid can be shut
    /// for hours. It never gives up, since a failed `D3D11CreateDevice` on an absent adapter
    /// returns in milliseconds and costs nothing to repeat.
    fn recover_lost_d3d11_device(
        &mut self,
        d3d11_cx: &mut D3d11Cx,
        d3d11_windows: &mut Vec<D3d11Window>,
    ) {
        let now = Instant::now();
        if self.os.d3d11_next_recovery_attempt.is_some_and(|at| now < at) {
            return;
        }
        // 250ms doubling to 4s. The first attempt is immediate; this only spaces the retries.
        let backoff = (250u64 << self.os.d3d11_recovery_attempts.min(4)).min(4000);
        self.os.d3d11_next_recovery_attempt =
            Some(now + Duration::from_millis(backoff));
        self.os.d3d11_recovery_attempts = self.os.d3d11_recovery_attempts.saturating_add(1);

        // Every window drops its swap chain, back buffer, view and beat registration first:
        // DXGI allows one flip-model swap chain per HWND at a time, so the dead one has to be
        // gone before a replacement can be made against the same window.
        for window in d3d11_windows.iter_mut() {
            window.release_gpu_resources();
        }
        // Only now are the old chains really gone: the context held the last references to
        // their back-buffer views, and a chain that still exists keeps its claim on the HWND,
        // which would make every rebuild below fail with E_ACCESSDENIED.
        d3d11_cx.clear_and_flush_context();
        // A pending studio grab can never be answered from a dead device, and leaving it
        // pending would both block its requester and hold the event loop in `Poll`.
        // A pending studio or `/g` grab can never be answered from a dead device. Answering
        // with the empty-PNG convention releases the requester and, just as importantly, empties
        // `screenshot_requests` — which is one of the conditions that would otherwise hold the
        // event loop in `Poll` for the whole outage.
        let pending: Vec<u64> = self
            .screenshot_requests
            .drain(..)
            .map(|r| r.request_id)
            .collect();
        Self::send_studio_screenshot_response(pending, 0, 0, Vec::new());

        if self.os.d3d11_force_recreate || !d3d11_cx.device_is_alive() {
            self.os.d3d11_force_recreate = false;
            self.os.d3d11_device = None;
            self.unpublish_d3d11_device_for_media();
            if !d3d11_cx.recreate_device() {
                return;
            }
            self.os.d3d11_device = Some(d3d11_cx.device.clone());
            self.publish_d3d11_device_for_media();
        }

        // The device is live again, so throw away every handle made from the old one. This
        // must happen before any window presents, or the first paint binds dead objects.
        self.d3d11_forget_gpu_resources();

        for window in d3d11_windows.iter_mut() {
            if !window.create_swap_chain(d3d11_cx) {
                // Leave `device_lost` set and try the whole sequence again on a later event.
                return;
            }
            window.device_lost = false;
            window.present_error_logged = false;
            window.resize_error_logged = false;
        }

        d3d11_cx.device_lost.set(false);
        self.os.d3d11_recovery_attempts = 0;
        self.os.d3d11_next_recovery_attempt = None;
        crate::log!("D3D11 device recovered; redrawing every window.");
        // Nothing on the GPU survived, so every pass has to be re-rendered, not just the
        // window passes a repaint would reach.
        for pass_id in self.passes.id_iter() {
            // Only passes that have actually been set up: a slot with no main draw list is one
            // nothing has drawn into, and painting it would be an immediate `unwrap` on `None`
            // in `draw_pass_to_texture`. `redraw_all` plus `repaint_windows` below reach the
            // window passes; this is what also reaches the offscreen ones.
            if self.passes[pass_id].main_draw_list_id.is_some() {
                self.passes[pass_id].paint_dirty = true;
            }
        }
        self.redraw_all();
        self.repaint_windows();
    }

    /// frame, so the Paint handler can tell a paced (vsync-blocking) pass from a no-op
    /// or dropped one; a dropped present does not count and re-marks its pass dirty.
    pub(crate) fn handle_repaint(
        &mut self,
        d3d11_windows: &mut Vec<D3d11Window>,
        d3d11_cx: &mut D3d11Cx,
    ) -> bool {
        let mut presented = false;
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        // ONE timestamp for the whole frame: the flip this beat is aimed at, or
        // wall-now for an unscoped tick. It used to be sampled per pass, so an
        // offscreen pass and the window pass that consumed it were stamped
        // milliseconds apart and any animation split across them sheared.
        let time_now = self
            .os
            .link_flip_time
            .unwrap_or_else(|| with_win32_app(|app| app.time_now())) as f32;
        let scope = self.os.link_scope;
        // Which windows have a beat of their own. Only those are held back during
        // someone else's beat — a popup (no frame-latency waitable) or a window in
        // a live resize has no beat coming, so holding its pass back would freeze
        // it for as long as another window keeps flipping.
        let paced: Vec<WindowId> = if scope.is_some() {
            with_win32_app(|app| app.beat_handles.iter().map(|b| b.window_id).collect())
        } else {
            Vec::new()
        };
        for draw_pass_id in &passes_todo {
            // Per-window pacing: during a beat only the flipping window's pass
            // tree paints; everything else stays dirty for its OWN beat.
            if let Some(scope) = scope {
                if let Some(window_id) = self.pass_root_window(*draw_pass_id) {
                    // ...unless a capture is waiting on that window. Its own beat
                    // may never come (an occluded window's presents are not
                    // retired), and holding the pass back while another window
                    // keeps flipping would leave the grab unanswered forever.
                    if window_id != scope
                        && paced.contains(&window_id)
                        && !self.has_pending_window_screenshot(window_id)
                    {
                        self.repaint_pass(*draw_pass_id);
                        continue;
                    }
                }
            }
            let uniforms_gen = self.next_uniform_gen();
            self.passes[*draw_pass_id].set_time(time_now, uniforms_gen);
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        // The device is gone; presenting can never succeed again, so
                        // do not re-dirty the pass — that is what would otherwise keep
                        // the loop spinning on a dead swap chain forever.
                        if window.device_lost {
                            continue;
                        }
                        // A minimized window gets no compositor vsync, and a window
                        // that reported DXGI_STATUS_OCCLUDED is not reaching glass:
                        // painting either is pure waste and its frame-latency waitable
                        // will not signal. Skip and keep the pass dirty — but only for
                        // so long, since both flags can stick on "hidden" while the
                        // window is really on screen (same probe the macOS backend runs
                        // against `occlusionState`).
                        // A pending `/g` grab overrides that skip: a capture is only
                        // ever produced by a pass that renders, and an app is just as
                        // grabbable behind another window as in front of it.
                        let capture_pending = self.has_pending_window_screenshot(window_id);
                        if window.win32_window.is_iconic() || window.occluded_since.is_some() {
                            let now = Instant::now();
                            let since = *window.occluded_since.get_or_insert(now);
                            if now.duration_since(since) < D3d11Window::OCCLUSION_PROBE_INTERVAL {
                                if !capture_pending {
                                    self.repaint_pass(*draw_pass_id);
                                    continue;
                                }
                                // Rendered for the grab, not as a probe: leave the
                                // probe clock alone so it still fires on schedule.
                            } else {
                                // Fall through and paint one probe frame: if the flag is
                                // stale we recover, if it is honest we spent one frame.
                                window.occluded_since = Some(now);
                            }
                        }
                        //let dpi_factor = window.window_geom.dpi_factor;
                        if window.is_in_resize {
                            window.sync_background_color(self.passes[*draw_pass_id].clear_color);
                        }
                        window.resize_buffers(&d3d11_cx);
                        // Present paced to the display refresh (vsync); see `windows_window_vsync()`
                        // for why this defaults to ON.
                        if self.draw_pass_to_window(
                            *draw_pass_id,
                            windows_window_vsync(),
                            window,
                            d3d11_cx,
                        ) {
                            presented = true;
                        } else {
                            // The frame was dropped: re-mark the pass dirty so the next loop
                            // pass re-presents, or the loop settles into Wait on stale
                            // content. The frame-latency wait paces the retry.
                            self.repaint_pass(*draw_pass_id);
                        }
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
        presented
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
        while let Some(op) = self.platform_ops.pop_front() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let (create_position, create_inner_size) = window.create_geom();
                    let d3d11_window = D3d11Window::new(
                        window_id,
                        &d3d11_cx,
                        create_inner_size,
                        create_position,
                        &window.create_title,
                        window.is_fullscreen,
                    );
                    let visuals = window.window_visuals();
                    let mut d3d11_window = d3d11_window;
                    d3d11_window.win32_window.apply_window_visuals(visuals);

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
                    let mut d3d11_window = d3d11_window;
                    d3d11_window
                        .win32_window
                        .apply_window_visuals(window.window_visuals());
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
                    // The authentic WindowClosed event this triggers delivers
                    // Event::WindowClosed and the exit check; firing it here would double it.
                    // Remove the D3d11Window now so later ops cannot touch the destroyed hwnd.
                    if let Some(index) = d3d11_windows.iter().position(|w| w.window_id == window_id)
                    {
                        self.windows[window_id].is_created = false;
                        d3d11_windows[index].win32_window.close_window();
                        d3d11_windows.remove(index);
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
                        // Apps rely on an unconditional WindowGeomChange echo, but ShowWindow
                        // sends no WM_SIZE when already in the target state; detect that
                        // no-op via the geometry-event generation and send it ourselves.
                        let gen = window.win32_window.geom_event_gen.get();
                        window.win32_window.maximize();
                        if window.win32_window.geom_event_gen.get() == gen {
                            window.win32_window.send_change_event();
                        }
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
                        // See MaximizeWindow: echo a WindowGeomChange on a ShowWindow no-op.
                        let gen = window.win32_window.geom_event_gen.get();
                        window.win32_window.restore();
                        if window.win32_window.geom_event_gen.get() == gen {
                            window.win32_window.send_change_event();
                        }
                    }
                }
                CxOsOp::Quit => ret = EventFlow::Exit,
                CxOsOp::SetTopmost(window_id, is_topmost) => {
                    if d3d11_windows.len() == 0 {
                        if self.defer_platform_op(CxOsOp::SetTopmost(window_id, is_topmost)) {
                            continue;
                        }
                        break;
                    }
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.set_topmost(is_topmost);
                    }
                }
                CxOsOp::SetChromelessWhenMaximized(window_id, chromeless) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.set_chromeless_when_maximized(chromeless);
                    }
                }
                CxOsOp::SetWindowTitle(window_id, title) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.set_title(&title);
                    }
                }
                CxOsOp::SetWindowVisuals(window_id, visuals) => {
                    if let Some(window) =
                        d3d11_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        window.win32_window.apply_window_visuals(visuals);
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
                CxOsOp::SelectFolderDialog(settings) => {
                    // Runs on its own STA thread; the answer arrives as a
                    // FileDialogAction, same contract as macOS.
                    super::file_dialog::open_select_folder_dialog(settings);
                }
                CxOsOp::SaveFolderDialog(settings) => {
                    super::file_dialog::open_save_folder_dialog(settings);
                }
                CxOsOp::SelectFileDialog(settings) => {
                    super::file_dialog::open_select_file_dialog(settings);
                }
                CxOsOp::SaveFileDialog(settings) => {
                    super::file_dialog::open_save_file_dialog(settings);
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
                CxOsOp::StartExternalDragging { .. } => {
                    // The existing OLE path advertises MOVE and has internal
                    // drag completion semantics. Do not expose managed files
                    // through it until the external COPY-only contract has a
                    // dedicated Windows source implementation.
                    crate::error!("external file dragging is not implemented on Windows");
                    self.call_event_handler(&Event::DragEnd);
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::ShowTextIME(area, cursor_rect, _config) => {
                    // Convert both corners of the caret line rect so its height is
                    // carried into native points along with the position.
                    let area_pos = area.clipped_rect(self).pos;
                    let window_id = self.get_window_id_of(&area).unwrap_or(CxWindowPool::id_zero());
                    let top_left = self.windows[window_id]
                        .layout_vec2d_to_native_points(area_pos + cursor_rect.pos);
                    let bottom_right = self.windows[window_id]
                        .layout_vec2d_to_native_points(area_pos + cursor_rect.pos + cursor_rect.size);
                    let ime_rect = Rect {
                        pos: top_left,
                        size: bottom_right - top_left,
                    };
                    d3d11_windows.iter_mut().for_each(|w| {
                        w.win32_window.set_ime_active(true);
                        w.win32_window.set_ime_rect(ime_rect);
                    });
                }
                CxOsOp::HideTextIME => {
                    d3d11_windows.iter_mut().for_each(|w| {
                        w.win32_window.set_ime_active(false);
                        w.win32_window.set_ime_rect(Rect::default());
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
                    if let Some(device) = self.os.d3d11_device.clone() {
                        // Allocate YUV textures internally for software decode path
                        let tex_y = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_u = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_v = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_y_id = tex_y.texture_id();
                        let tex_u_id = tex_u.texture_id();
                        let tex_v_id = tex_v.texture_id();
                        let player = WindowsUnifiedVideoPlayer::new(
                            &device,
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
                            VideoYuvTexturesReady::planes(video_id, tex_y, tex_u, tex_v),
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
                // Track selection is currently implemented on Linux GStreamer only.
                CxOsOp::SelectVideoTrack(_, _) | CxOsOp::SelectAudioTrack(_, _) => {}
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

/// Whether to present the window paced to the display's refresh rate (vsync).
///
/// Defaults to ON, matching the Linux/EGL backend (`swap_interval = 1`, see
/// `os/linux/opengl_cx.rs`). Previously the Windows backend always presented uncapped
/// (`Present(0, ...)`) from inside the free-spinning `EventFlow::Poll` loop, so during any
/// scroll/animation it rendered far more frames than the monitor could display (e.g. ~127 fps
/// on a 99 Hz panel). The surplus frames are discarded unevenly by the DWM compositor, and
/// because makepad's scroll/fling animations advance by a fixed step *per rendered frame*, the
/// uneven display cadence makes scrolling visibly judder — perceived as "laggy scrolling" even
/// though the raw frame rate is high. Pacing to vblank renders exactly one frame per refresh,
/// so each displayed frame advances the scroll by a constant step (smooth), and it also stops
/// the loop from burning CPU/GPU rendering invisible frames.
///
/// Set the `MAKEPAD_NO_VSYNC` env var to opt out (e.g. for benchmarking), mirroring the Linux
/// backend's env var of the same name.
fn windows_window_vsync() -> bool {
    use std::sync::OnceLock;
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var_os("MAKEPAD_NO_VSYNC").is_none())
}

impl CxGameInputApi for Cx {
    fn game_input_state(&mut self, index: usize) -> Option<&GameInputState> {
        if self.in_makepad_studio {
            return self.game_input_remote.get(index);
        }
        if let Some(game_input) = &self.os.windows_game_input {
            if index < game_input.states.len() {
                return Some(&game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states(&mut self) -> &[GameInputState] {
        // Hosted by Studio: this process has no window, so the OS never gave
        // it the controllers. Studio forwards them instead.
        if self.in_makepad_studio {
            return &self.game_input_remote;
        }
        if let Some(game_input) = &self.os.windows_game_input {
            return &game_input.states;
        }
        &[]
    }

    fn game_input_state_mut(&mut self, index: usize) -> Option<&mut GameInputState> {
        if self.in_makepad_studio {
            return self.game_input_remote.get_mut(index);
        }
        if let Some(game_input) = &mut self.os.windows_game_input {
            if index < game_input.states.len() {
                return Some(&mut game_input.states[index]);
            }
        }
        None
    }

    fn game_input_states_mut(&mut self) -> &mut [GameInputState] {
        if self.in_makepad_studio {
            return &mut self.game_input_remote;
        }
        if let Some(game_input) = &mut self.os.windows_game_input {
            return &mut game_input.states;
        }
        &mut []
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR") {
            self.package_root = Some(item.to_string());
        }

        self.native_load_dependencies();

        self.os.windows_game_input = Some(WindowsGameInput::init());
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
    /// While a beat runs: paint ONLY passes rooted in this window, and stamp them
    /// with `link_flip_time` — the app time of the flip the frame is aimed at.
    /// None = an unscoped tick (heartbeat / resize / geometry echo): paint
    /// everything, stamp wall-now. Twin of the macOS backend's link_scope.
    pub(crate) link_scope: Option<WindowId>,
    pub(crate) link_flip_time: Option<f64>,
    pub(crate) start_time: Option<Instant>,
    pub(crate) media: CxWindowsMedia,
    pub(crate) d3d11_device: Option<ID3D11Device>,
    pub(crate) game_input_events: GameInputEventChannel,
    pub(crate) windows_game_input: Option<WindowsGameInput>,
    pub(crate) video_players: HashMap<LiveId, WindowsUnifiedVideoPlayer>,
    pub(crate) async_hlsl_compile: crate::os::windows::d3d11::AsyncHlslCompile,
    pub(crate) stdin_timers: crate::os::shared_framebuf::PollTimers,
    /// Earliest time the device-loss recovery may try again, and how many tries this outage
    /// has taken. Recovery is driven by whatever event next reaches the loop rather than by a
    /// timer of its own, so this is what spaces the attempts.
    pub(crate) d3d11_next_recovery_attempt: Option<Instant>,
    pub(crate) d3d11_recovery_attempts: u32,
    /// Next scheduled fault injection; see `MAKEPAD_D3D11_TEST_DEVICE_LOSS`.
    pub(crate) d3d11_test_loss_next: Option<Instant>,
    /// Recreate the device even though it reports itself alive. Set only by fault injection.
    pub(crate) d3d11_force_recreate: bool,
}
