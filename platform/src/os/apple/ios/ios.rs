use {
    crate::{
        cx::{Cx, IosParams, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        event::{
            video_playback::{
                VideoPlaybackPreparedEvent, VideoPlaybackResourcesReleasedEvent,
                VideoTextureUpdatedEvent,
            },
            Event, KeyEvent, TextInputEvent, TextRangeReplaceEvent,
        },
        makepad_live_id::*,
        makepad_objc_sys::objc_block,
        os::{
            apple::{
                apple_sys::*,
                apple_util::*,
                apple_video_playback::AppleVideoPlayer,
                ios::{
                    ios_app::{self, init_ios_app_global, with_ios_app, IosApp},
                    ios_event::IosEvent,
                },
            },
            apple_classes::init_apple_classes_global,
            apple_media::CxAppleMedia,
            cx_native::EventFlow,
            metal::{DrawPassMode, MetalCx},
        },
        permission::PermissionResult,
        thread::SignalToUI,
        window::CxWindowPool,
    },
    std::{
        cell::RefCell,
        collections::HashMap,
        rc::Rc,
        sync::mpsc::{channel, Receiver, Sender},
        time::Instant,
    },
};

impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        let data_path = IosApp::get_ios_directory_paths();

        // Get device info
        let device_model = unsafe {
            let device: ObjcId = msg_send![class!(UIDevice), currentDevice];
            let model: ObjcId = msg_send![device, model];
            nsstring_to_string(model)
        };

        let system_version = unsafe {
            let device: ObjcId = msg_send![class!(UIDevice), currentDevice];
            let version: ObjcId = msg_send![device, systemVersion];
            nsstring_to_string(version)
        };

        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Ios(IosParams {
            data_path,
            device_model,
            system_version,
        });

        let metal_cx: Rc<RefCell<MetalCx>> = Rc::new(RefCell::new(MetalCx::new()));
        //let cx = Rc::new(RefCell::new(self));
        //crate::log!("Makepad iOS application started.");
        //let metal_windows = Rc::new(RefCell::new(Vec::new()));
        let device = metal_cx.borrow().device;
        init_apple_classes_global();
        init_ios_app_global(
            device,
            Box::new({
                let cx = cx.clone();
                move |event| {
                    let mut cx_ref = cx.borrow_mut();
                    let mut metal_cx = metal_cx.borrow_mut();
                    let event_flow = cx_ref.ios_event_callback(event, &mut metal_cx);
                    let executor = cx_ref.executor.take().unwrap();
                    drop(cx_ref);
                    executor.run_until_stalled();
                    let mut cx_ref = cx.borrow_mut();
                    cx_ref.executor = Some(executor);
                    event_flow
                }
            }),
        );
        // lets set our signal poll timer

        // final bit of initflow

        IosApp::event_loop();
    }

    pub(crate) fn handle_repaint(&mut self, metal_cx: &mut MetalCx) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for draw_pass_id in &passes_todo {
            self.passes[*draw_pass_id].set_time(with_ios_app(|app| app.time_now() as f32));
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(_window_id) => {
                    let mtk_view = with_ios_app(|app| app.mtk_view.unwrap());
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::MTKView(mtk_view));
                }
                CxDrawPassParent::DrawPass(_) => {
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
                CxDrawPassParent::None => {
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }

    pub(crate) fn handle_networking_events(&mut self) {
        self.dispatch_network_runtime_events();
    }

    pub(crate) fn handle_permission_events(&mut self) {
        while let Ok(result) = self.os.permission_response.receiver.try_recv() {
            self.call_event_handler(&Event::PermissionResult(result));
        }
    }

    fn ios_event_callback(&mut self, event: IosEvent, metal_cx: &mut MetalCx) -> EventFlow {
        self.handle_platform_ops(metal_cx);

        // send a mouse up when dragging starts

        let mut paint_dirty = false;
        match &event {
            IosEvent::KeyDown(_) | IosEvent::KeyUp(_) | IosEvent::TextInput(_) => {}
            IosEvent::Timer(te) => {
                if te.timer_id == 0 {
                    let vk = with_ios_app(|app| app.virtual_keyboard_event.take());
                    if let Some(vk) = vk {
                        self.call_event_handler(&Event::VirtualKeyboard(vk));
                    }
                    // Drain iOS text events as one batch to avoid re-entrancy from UITextInput callbacks.
                    let queued_events =
                        with_ios_app(|app| std::mem::take(&mut app.queued_text_events));
                    let time = with_ios_app(|app| app.time_now());
                    for queued_event in queued_events {
                        match queued_event {
                            ios_app::IosTextInputEvent::TextInput(input, replace_last) => {
                                self.call_event_handler(&Event::TextInput(TextInputEvent {
                                    input,
                                    replace_last,
                                    was_paste: false,
                                    ..Default::default()
                                }));
                            }
                            ios_app::IosTextInputEvent::RangeReplace(start, end, text) => {
                                self.call_event_handler(&Event::TextRangeReplace(
                                    TextRangeReplaceEvent { start, end, text },
                                ));
                            }
                            ios_app::IosTextInputEvent::KeyEvent(key_code) => {
                                self.call_event_handler(&Event::KeyDown(KeyEvent {
                                    key_code,
                                    is_repeat: false,
                                    modifiers: Default::default(),
                                    time,
                                }));
                                self.call_event_handler(&Event::KeyUp(KeyEvent {
                                    key_code,
                                    is_repeat: false,
                                    modifiers: Default::default(),
                                    time,
                                }));
                            }
                        }
                    }
                    // check signals
                    if SignalToUI::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.handle_script_signals();
                        self.call_event_handler(&Event::Signal);
                    }
                    if SignalToUI::check_and_clear_action_signal() {
                        self.handle_action_receiver();
                    }

                    if self.handle_live_edit() {
                        // self.draw_shaders.ptr_to_item.clear();
                        // self.draw_shaders.fingerprints.clear();
                        self.call_event_handler(&Event::LiveEdit);
                        self.redraw_all();
                    }
                    self.handle_networking_events();
                    self.handle_permission_events();
                }
            }
            _ => (),
        }

        //self.process_desktop_pre_event(&mut event);
        match event {
            IosEvent::VirtualKeyboard(vk) => {
                self.call_event_handler(&Event::VirtualKeyboard(vk));
            }
            IosEvent::Init => {
                with_ios_app(|app| app.start_timer(0, 0.008, true));
                self.start_studio_websocket_delayed();
                self.call_event_handler(&Event::Startup);
                self.redraw_all();
            }
            IosEvent::WindowGotFocus(window_id) => {
                // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                paint_dirty = true;
                self.call_event_handler(&Event::WindowGotFocus(window_id));
            }
            IosEvent::WindowLostFocus(window_id) => {
                self.call_event_handler(&Event::WindowLostFocus(window_id));
            }
            IosEvent::WindowGeomChange(re) => {
                // do this here because mac
                let window_id = CxWindowPool::id_zero();
                let window = &mut self.windows[window_id];
                window.window_geom = re.new_geom.clone();
                self.call_event_handler(&Event::WindowGeomChange(re));
                self.redraw_all();
            }
            IosEvent::Paint => {
                // Poll video players for new frames and preparation status
                if !self.os.video_players.is_empty() {
                    let mut video_events = Vec::new();
                    for (_video_id, player) in self.os.video_players.iter_mut() {
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
                        if player.poll_frame(&mut self.textures) {
                            video_events.push(Event::VideoTextureUpdated(
                                VideoTextureUpdatedEvent {
                                    video_id: player.video_id,
                                    current_position_ms: player.current_position_ms(),
                                },
                            ));
                        }
                    }
                    for event in video_events {
                        self.call_event_handler(&event);
                    }
                }

                let time_now = with_ios_app(|app| app.time_now());
                if self.new_next_frames.len() != 0 {
                    self.call_next_frame_event(time_now);
                }
                if self.need_redrawing() {
                    self.call_draw_event(time_now);
                    self.mtl_compile_shaders(&metal_cx);
                }
                // ok here we send out to all our childprocesses
                self.handle_repaint(metal_cx);
            }
            IosEvent::TouchUpdate(e) => {
                self.fingers.process_touch_update_start(e.time, &e.touches);
                let e = Event::TouchUpdate(e);
                self.call_event_handler(&e);
                let e = if let Event::TouchUpdate(e) = e {
                    e
                } else {
                    panic!()
                };
                self.fingers.process_touch_update_end(&e.touches);
            }
            IosEvent::LongPress(e) => {
                self.call_event_handler(&Event::LongPress(e.into()));
            }
            IosEvent::MouseDown(e) => {
                self.fingers.process_tap_count(e.abs, e.time);
                self.fingers.mouse_down(e.button, e.window_id);
                self.call_event_handler(&Event::MouseDown(e.into()))
            }
            IosEvent::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            IosEvent::MouseUp(e) => {
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            IosEvent::Scroll(e) => self.call_event_handler(&Event::Scroll(e.into())),
            IosEvent::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
            IosEvent::TextRangeReplace(e) => self.call_event_handler(&Event::TextRangeReplace(e)),

            IosEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            IosEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            IosEvent::TextCopy(e) => self.call_event_handler(&Event::TextCopy(e)),
            IosEvent::TextCut(e) => self.call_event_handler(&Event::TextCut(e)),
            IosEvent::Timer(e) => {
                if e.timer_id != 0 {
                    self.handle_script_timer(&e);
                    self.call_event_handler(&Event::Timer(e))
                }
            }
            IosEvent::PermissionResult(result) => {
                self.call_event_handler(&Event::PermissionResult(result))
            }
        }

        if self.any_passes_dirty()
            || self.need_redrawing()
            || self.new_next_frames.len() != 0
            || paint_dirty
            || self.demo_time_repaint
            || !self.os.video_players.is_empty()
        {
            EventFlow::Poll
        } else {
            EventFlow::Wait
        }
    }

    fn handle_platform_ops(&mut self, metal_cx: &MetalCx) {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    window.window_geom = with_ios_app(|app| app.last_window_geom.clone());
                    window.is_created = true;
                }
                CxOsOp::ShowTextIME(_area, pos, config) => {
                    IosApp::set_ime_position(pos);
                    IosApp::configure_keyboard(&config);
                    IosApp::show_keyboard();
                }
                CxOsOp::HideTextIME => {
                    IosApp::hide_keyboard();
                }
                CxOsOp::SyncImeState {
                    text,
                    selection,
                    composition: _,
                } => {
                    IosApp::set_ime_text(text, selection.end.0);
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    with_ios_app(|app| app.start_timer(timer_id, interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    with_ios_app(|app| app.stop_timer(timer_id));
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                } => {
                    self.handle_permission_check(permission, request_id);
                }
                CxOsOp::RequestPermission {
                    permission,
                    request_id,
                } => {
                    self.handle_permission_request(permission, request_id);
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                CxOsOp::ShowClipboardActions {
                    has_selection,
                    rect,
                    keyboard_shift,
                } => {
                    IosApp::show_clipboard_actions(has_selection, rect, keyboard_shift);
                }
                CxOsOp::HideClipboardActions => {
                    IosApp::hide_clipboard_actions();
                }
                CxOsOp::CopyToClipboard(content) => {
                    with_ios_app(|app| app.copy_to_clipboard(&content));
                }
                CxOsOp::SetCursor(_) => {
                    // no need
                }
                CxOsOp::PrepareVideoPlayback(
                    video_id,
                    source,
                    _gl_handle,
                    texture_id,
                    autoplay,
                    should_loop,
                ) => {
                    let player = AppleVideoPlayer::new(
                        metal_cx.device,
                        video_id,
                        texture_id,
                        source,
                        autoplay,
                        should_loop,
                    );
                    self.os.video_players.insert(video_id, player);
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.play();
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.pause();
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.resume();
                    }
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.mute();
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if let Some(player) = self.os.video_players.get(&video_id) {
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
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.seek_to(position_ms);
                    }
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
    }

    /*
    let _ = self.live_file_change_sender.send(vec![LiveFileChange{
        file_name:file_name.to_string(),
        content
    }]);*/

    fn check_audio_permission_status(&self) -> crate::permission::PermissionStatus {
        unsafe {
            let av_audio_session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];
            let permission_status: i32 = msg_send![av_audio_session, recordPermission];

            match permission_status {
                2 => crate::permission::PermissionStatus::Granted, // AVAudioSessionRecordPermissionGranted
                1 => crate::permission::PermissionStatus::DeniedPermanent, // AVAudioSessionRecordPermissionDenied - iOS doesn't re-prompt
                _ => crate::permission::PermissionStatus::NotDetermined, // AVAudioSessionRecordPermissionUndetermined (0) or unknown
            }
        }
    }

    fn handle_permission_check(
        &mut self,
        permission: crate::permission::Permission,
        request_id: i32,
    ) {
        let status = match permission {
            crate::permission::Permission::AudioInput => self.check_audio_permission_status(),
        };

        self.call_event_handler(&crate::event::Event::PermissionResult(
            crate::permission::PermissionResult {
                permission,
                request_id,
                status,
            },
        ));
    }

    fn handle_permission_request(
        &mut self,
        permission: crate::permission::Permission,
        request_id: i32,
    ) {
        match permission {
            crate::permission::Permission::AudioInput => {
                let status = self.check_audio_permission_status();
                match status {
                    crate::permission::PermissionStatus::Granted => {
                        // Already granted, don't re-ask
                        self.call_event_handler(&crate::event::Event::PermissionResult(
                            crate::permission::PermissionResult {
                                permission,
                                request_id,
                                status,
                            },
                        ));
                    }
                    crate::permission::PermissionStatus::DeniedPermanent => {
                        // Previously denied, send denied event
                        self.call_event_handler(&crate::event::Event::PermissionResult(
                            crate::permission::PermissionResult {
                                permission,
                                request_id,
                                status,
                            },
                        ));
                    }
                    crate::permission::PermissionStatus::NotDetermined => {
                        // Need to request permission
                        self.ios_request_audio_permission(permission, request_id);
                    }
                    _ => {
                        // For other statuses, send the result directly
                        self.call_event_handler(&crate::event::Event::PermissionResult(
                            crate::permission::PermissionResult {
                                permission,
                                request_id,
                                status,
                            },
                        ));
                    }
                }
            }
        }
    }

    fn ios_request_audio_permission(
        &mut self,
        permission: crate::permission::Permission,
        request_id: i32,
    ) {
        let sender = self.os.permission_response.sender.clone();
        unsafe {
            let av_audio_session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];

            let completion_handler = objc_block!(move |granted: BOOL| {
                let permission_result = crate::permission::PermissionResult {
                    permission,
                    request_id,
                    status: if granted == YES {
                        crate::permission::PermissionStatus::Granted
                    } else {
                        crate::permission::PermissionStatus::DeniedPermanent // iOS doesn't re-prompt
                    },
                };

                let _ = sender.send(permission_result);
            });

            let () = msg_send![av_audio_session, requestRecordPermission: &completion_handler];
        }
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        #[cfg(not(apple_sim))]
        {
            self.package_root = Some("makepad".to_string());
        }

        #[cfg(apple_sim)]
        self.native_load_dependencies();
        #[cfg(not(apple_sim))]
        self.apple_bundle_load_dependencies();
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

    fn max_texture_width() -> usize {
        16384
    }

    /*
    fn web_socket_open(&mut self, _url: String, _rec: WebSocketAutoReconnect) -> WebSocket {
        todo!()
    }

    fn web_socket_send(&mut self, _websocket: WebSocket, _data: Vec<u8>) {
        todo!()
    }*/
}

#[derive(Default)]
pub struct CxOs {
    pub(crate) start_time: Option<Instant>,
    pub(crate) media: CxAppleMedia,
    pub(crate) bytes_written: usize,
    pub(crate) draw_calls_done: usize,
    pub(crate) instances_done: u64,
    pub(crate) vertices_done: u64,
    pub(crate) instance_bytes_uploaded: u64,
    pub(crate) uniform_bytes_uploaded: u64,
    pub(crate) vertex_buffer_bytes_uploaded: u64,
    pub(crate) texture_bytes_uploaded: u64,
    pub(crate) permission_response: PermissionResultChannel,
    pub(crate) apple_game_input: Option<crate::os::apple::apple_game_input::AppleGameInput>,
    pub(crate) video_players: HashMap<LiveId, AppleVideoPlayer>,
}

pub struct PermissionResultChannel {
    pub receiver: Receiver<PermissionResult>,
    pub sender: Sender<PermissionResult>,
}

impl Default for PermissionResultChannel {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self { sender, receiver }
    }
}
