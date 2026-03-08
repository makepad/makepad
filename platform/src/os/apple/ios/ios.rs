use {
    crate::{
        cx::{Cx, IosParams, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        event::{
            video_playback::{
                CameraPreviewMode, VideoBufferedRangesEvent, VideoDecodingErrorEvent,
                VideoPlaybackPreparedEvent, VideoPlaybackResourcesReleasedEvent,
                VideoSeekableRangesEvent, VideoSource, VideoTextureUpdatedEvent,
                VideoYuvTexturesReady,
            },
            Event, KeyEvent, TextInputEvent, TextRangeReplaceEvent,
        },
        makepad_live_id::*,
        makepad_objc_sys::objc_block,
        media_api::CxMediaApi,
        os::{
            apple::{
                apple_sys::*,
                apple_video_player::AppleUnifiedVideoPlayer,
                apple_yuv_metal::AppleYuvMetal,
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
        texture::{CxTexturePool, Texture, TextureFormat, TextureId},
        thread::SignalToUI,
        video::{
            CameraFrameInputFn, CameraFrameLatest, CameraFrameLayout, CameraFrameRef,
            VideoFormatId, VideoInputId, MAX_VIDEO_DEVICE_INDEX,
        },
        window::CxWindowPool,
        DVec2, Rect,
    },
    std::{
        cell::RefCell,
        collections::HashMap,
        rc::Rc,
        sync::{
            mpsc::{channel, Receiver, Sender},
            Arc, Mutex,
        },
        time::Instant,
    },
};

pub(crate) struct IosCameraPlayer {
    video_id: LiveId,
    tex_y_id: TextureId,
    tex_u_id: TextureId,
    tex_v_id: TextureId,
    width: u32,
    height: u32,
    prepared: bool,
    prepare_notified: bool,
    yuv_matrix: f32,
    yuv_biplanar: bool,
    yuv_metal: AppleYuvMetal,
    latest_nv12: Arc<Mutex<Option<crate::os::apple::av_capture::AvCapturePixelBuffer>>>,
    i420_frames: CameraFrameLatest,
    camera_access: Option<Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>>,
}

fn register_ios_camera_subscription(
    camera_access: &Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>,
    video_id: LiveId,
    input_id: VideoInputId,
    format_id: VideoFormatId,
    frame_cb: Option<CameraFrameInputFn>,
    pixel_buffer_cb: Option<crate::os::apple::av_capture::CameraPixelBufferInputFn>,
) {
    camera_access.lock().unwrap().register_preview(
        video_id,
        input_id,
        format_id,
        frame_cb,
        pixel_buffer_cb,
    );
}

fn unregister_ios_camera_subscription(
    camera_access: Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>,
    video_id: LiveId,
) {
    camera_access.lock().unwrap().unregister_preview(video_id);
}

impl IosCameraPlayer {
    fn new(
        video_id: LiveId,
        tex_y_id: TextureId,
        tex_u_id: TextureId,
        tex_v_id: TextureId,
        metal_device: ObjcId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        camera_access: Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>,
    ) -> Self {
        let i420_frames = CameraFrameLatest::new(4);
        let i420_ring = i420_frames.ring();
        let latest_nv12 = Arc::new(Mutex::new(None));
        let latest_nv12_clone = latest_nv12.clone();

        let cb: CameraFrameInputFn = Box::new(move |frame_ref: CameraFrameRef<'_>| {
            if frame_ref.layout == CameraFrameLayout::NV12 {
                return;
            }
            let _ = i420_ring.publish_i420_converted(frame_ref);
        });

        let pixel_buffer_cb: crate::os::apple::av_capture::CameraPixelBufferInputFn =
            Box::new(move |frame| {
                let mut latest = latest_nv12_clone.lock().unwrap();
                if let Some(old) = latest.replace(frame) {
                    unsafe {
                        CVPixelBufferRelease(old.pixel_buffer);
                    }
                }
            });

        register_ios_camera_subscription(
            &camera_access,
            video_id,
            input_id,
            format_id,
            Some(cb),
            Some(pixel_buffer_cb),
        );

        let yuv_metal = AppleYuvMetal::new(metal_device, "iOS camera");

        Self {
            video_id,
            tex_y_id,
            tex_u_id,
            tex_v_id,
            width: 0,
            height: 0,
            prepared: false,
            prepare_notified: false,
            yuv_matrix: 0.0,
            yuv_biplanar: false,
            yuv_metal,
            latest_nv12,
            i420_frames,
            camera_access: Some(camera_access),
        }
    }

    fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        if self.prepare_notified {
            return None;
        }

        if let Some(frame) = self.latest_nv12.lock().unwrap().as_ref() {
            self.width = frame.width as u32;
            self.height = frame.height as u32;
            self.prepared = true;
            self.prepare_notified = true;
            return Some(Ok((
                self.width,
                self.height,
                0,
                false,
                vec!["camera".to_string()],
                vec![],
            )));
        }

        if !self.i420_frames.prime_pending_from_latest() {
            return None;
        }

        let (width, height) = {
            let frame = self.i420_frames.pending_frame()?;
            (frame.width as u32, frame.height as u32)
        };
        self.width = width;
        self.height = height;
        self.prepared = true;
        self.prepare_notified = true;

        Some(Ok((
            self.width,
            self.height,
            0,
            false,
            vec!["camera".to_string()],
            vec![],
        )))
    }

    fn poll_frame(&mut self, textures: &mut CxTexturePool) -> bool {
        if !self.prepared {
            return false;
        }

        if let Some(frame) = self.latest_nv12.lock().unwrap().take() {
            self.width = frame.width as u32;
            self.height = frame.height as u32;
            self.yuv_matrix = frame.matrix.as_yuv_uniform();
            let wrapped = self.yuv_metal.wrap_nv12_cv_pixel_buffer(
                textures,
                self.tex_y_id,
                self.tex_u_id,
                self.tex_v_id,
                frame.pixel_buffer,
                frame.width as u32,
                frame.height as u32,
            );
            unsafe {
                CVPixelBufferRelease(frame.pixel_buffer);
            }
            if wrapped {
                self.yuv_biplanar = true;
                return true;
            }
        }

        let Some(frame) = self.i420_frames.take_pending_or_latest() else {
            return false;
        };

        if frame.width == 0 || frame.height == 0 || frame.plane_count < 3 {
            return false;
        }

        let width = frame.width as u32;
        let height = frame.height as u32;
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);

        self.yuv_metal.upload_r8_plane(
            textures,
            self.tex_y_id,
            &frame.planes[0].bytes,
            width,
            height,
        );
        self.yuv_metal
            .upload_r8_plane(textures, self.tex_u_id, &frame.planes[1].bytes, cw, ch);
        self.yuv_metal
            .upload_r8_plane(textures, self.tex_v_id, &frame.planes[2].bytes, cw, ch);

        self.yuv_biplanar = false;
        self.yuv_matrix = frame.matrix.as_yuv_uniform();

        true
    }

    fn yuv_biplanar(&self) -> f32 {
        if self.yuv_biplanar {
            1.0
        } else {
            0.0
        }
    }

    fn cleanup(&mut self) {
        if let Some(frame) = self.latest_nv12.lock().unwrap().take() {
            unsafe {
                CVPixelBufferRelease(frame.pixel_buffer);
            }
        }

        self.yuv_metal.cleanup();

        if let Some(cam) = self.camera_access.take() {
            unregister_ios_camera_subscription(cam, self.video_id);
        }
    }
}

impl Drop for IosCameraPlayer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

pub(crate) struct IosNativeCameraPreview {
    video_id: LiveId,
    input_id: VideoInputId,
    format_id: VideoFormatId,
    width: u32,
    height: u32,
    prepare_notified: bool,
    camera_access: Option<Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>>,
}

impl IosNativeCameraPreview {
    fn new(
        video_id: LiveId,
        input_id: VideoInputId,
        format_id: VideoFormatId,
        camera_access: Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>,
    ) -> Self {
        register_ios_camera_subscription(
            &camera_access,
            video_id,
            input_id,
            format_id,
            None,
            None,
        );

        let (width, height) = {
            let cam = camera_access.lock().unwrap();
            cam.format_size(input_id, format_id).unwrap_or((0, 0))
        };

        Self {
            video_id,
            input_id,
            format_id,
            width,
            height,
            prepare_notified: false,
            camera_access: Some(camera_access),
        }
    }

    fn check_prepared(
        &mut self,
    ) -> Option<Result<(u32, u32, u128, bool, Vec<String>, Vec<String>), String>> {
        if self.prepare_notified {
            return None;
        }
        self.prepare_notified = true;
        Some(Ok((
            self.width,
            self.height,
            0,
            false,
            vec!["camera".to_string()],
            vec![],
        )))
    }

    fn session(&self) -> Option<ObjcId> {
        let cam = self.camera_access.as_ref()?.lock().unwrap();
        cam.session_for(self.input_id, self.format_id)
    }

    fn cleanup(&mut self) {
        if let Some(cam) = self.camera_access.take() {
            unregister_ios_camera_subscription(cam, self.video_id);
        }
    }
}

impl Drop for IosNativeCameraPreview {
    fn drop(&mut self) {
        self.cleanup();
    }
}

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
                CxDrawPassParent::Window(window_id) => {
                    // Skip popup window passes — drawn as overlays after parent.
                    if self.windows[window_id].is_popup {
                        continue;
                    }
                    let mtk_view = with_ios_app(|app| app.mtk_view.unwrap());
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::MTKView(mtk_view));

                    // Draw popup window passes as overlays on the same MTKView
                    for popup_pass_id in &passes_todo.clone() {
                        if let CxDrawPassParent::Window(pw_id) = self.passes[*popup_pass_id].parent
                        {
                            let pw = &self.windows[pw_id];
                            if pw.is_popup && pw.popup_parent == Some(window_id) {
                                let mtk_view = with_ios_app(|app| app.mtk_view.unwrap());
                                self.draw_pass(
                                    *popup_pass_id,
                                    metal_cx,
                                    DrawPassMode::MTKView(mtk_view),
                                );
                            }
                        }
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
                CxDrawPassParent::None => {
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }

        let timestamp_ns = self
            .os
            .start_time
            .map(|start| Instant::now().duration_since(start).as_nanos() as u64)
            .unwrap_or(0);
        for index in 0..MAX_VIDEO_DEVICE_INDEX {
            if let Err(err) = self.video_encoder_capture_texture_frame(index, timestamp_ns) {
                if err != crate::video::VideoEncodeError::UnsupportedSource
                    && err != crate::video::VideoEncodeError::EncoderNotStarted
                {
                    crate::error!(
                        "ios video texture capture failed on slot {}: {:?}",
                        index,
                        err
                    );
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
                                let seekable = player.seekable_ranges();
                                if !seekable.is_empty() {
                                    video_events.push(Event::VideoSeekableRanges(
                                        VideoSeekableRangesEvent {
                                            video_id: player.video_id,
                                            ranges: seekable,
                                        },
                                    ));
                                }
                                let buffered = player.buffered_ranges();
                                if !buffered.is_empty() {
                                    video_events.push(Event::VideoBufferedRanges(
                                        VideoBufferedRangesEvent {
                                            video_id: player.video_id,
                                            ranges: buffered,
                                        },
                                    ));
                                }
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
                                        biplanar: player.yuv_biplanar() > 0.5,
                                        rotation_steps: 0.0,
                                    },
                                },
                            ));
                        }
                    }
                    for event in video_events {
                        self.call_event_handler(&event);
                    }
                }

                if !self.os.camera_players.is_empty() {
                    let mut camera_events = Vec::new();
                    for (_video_id, player) in self.os.camera_players.iter_mut() {
                        match player.check_prepared() {
                            Some(Ok((
                                width,
                                height,
                                duration,
                                is_seekable,
                                video_tracks,
                                audio_tracks,
                            ))) => {
                                camera_events.push(Event::VideoPlaybackPrepared(
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
                                camera_events.push(Event::VideoDecodingError(
                                    VideoDecodingErrorEvent {
                                        video_id: player.video_id,
                                        error: err,
                                    },
                                ));
                            }
                            None => {}
                        }

                        if player.poll_frame(&mut self.textures) {
                            camera_events.push(Event::VideoTextureUpdated(
                                VideoTextureUpdatedEvent {
                                    video_id: player.video_id,
                                    current_position_ms: 0,
                                    yuv: crate::event::video_playback::VideoYuvMetadata {
                                        enabled: true,
                                        matrix: player.yuv_matrix,
                                        biplanar: player.yuv_biplanar() > 0.5,
                                        rotation_steps: 0.0,
                                    },
                                },
                            ));
                        }
                    }
                    for event in camera_events {
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
                // Check for outside-click popup dismiss on touch start
                if e.touches
                    .iter()
                    .any(|t| t.state == crate::event::TouchState::Start)
                {
                    if let Some(popup_window_id) = self.find_popup_to_dismiss_on_touch(&e.touches) {
                        self.dismiss_popup_window(
                            popup_window_id,
                            crate::event::PopupDismissReason::OutsideClick,
                        );
                    }
                }
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
                // Check for outside-click popup dismiss
                if let Some(popup_window_id) = self.find_popup_to_dismiss_on_mouse(e.abs) {
                    self.dismiss_popup_window(
                        popup_window_id,
                        crate::event::PopupDismissReason::OutsideClick,
                    );
                }
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
            IosEvent::SelectionHandleDrag(e) => {
                self.call_event_handler(&Event::SelectionHandleDrag(e))
            }

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
            || !self.os.camera_players.is_empty()
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
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let mut geom = with_ios_app(|app| app.last_window_geom.clone());
                    geom.position = position;
                    geom.inner_size = size;
                    geom.outer_size = size;
                    let window = &mut self.windows[window_id];
                    window.window_geom = geom;
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;
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
                CxOsOp::SetPrimarySelection(_) => {}
                CxOsOp::ShowSelectionHandles { start, end } => {
                    IosApp::show_selection_handles(start, end);
                }
                CxOsOp::UpdateSelectionHandles { start, end } => {
                    IosApp::update_selection_handles(start, end);
                }
                CxOsOp::HideSelectionHandles => {
                    IosApp::hide_selection_handles();
                }
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::FullscreenWindow(_window_id) => {
                    with_ios_app(|app| app.set_fullscreen(true));
                }
                CxOsOp::NormalizeWindow(_window_id) => {
                    with_ios_app(|app| app.set_fullscreen(false));
                }
                CxOsOp::SetCursor(_) => {
                    // no need
                }
                CxOsOp::AttachCameraNativePreview { video_id, area } => {
                    if let Some(preview) = self.os.native_camera_previews.get(&video_id) {
                        if let Some(session) = preview.session() {
                            IosApp::attach_camera_preview(video_id.0, session);
                            let rect = area.clipped_rect(self);
                            IosApp::update_camera_preview(video_id.0, rect, true);
                        }
                    }
                }
                CxOsOp::UpdateCameraNativePreview {
                    video_id,
                    area,
                    visible,
                } => {
                    if let Some(preview) = self.os.native_camera_previews.get(&video_id) {
                        if let Some(session) = preview.session() {
                            IosApp::attach_camera_preview(video_id.0, session);
                        }
                    }
                    let rect = area.clipped_rect(self);
                    IosApp::update_camera_preview(video_id.0, rect, visible);
                }
                CxOsOp::DetachCameraNativePreview { video_id } => {
                    IosApp::detach_camera_preview(video_id.0);
                }
                CxOsOp::PrepareVideoPlayback(
                    video_id,
                    source,
                    camera_preview_mode,
                    _gl_handle,
                    texture_id,
                    autoplay,
                    should_loop,
                ) => {
                    if let Some(mut player) = self.os.video_players.remove(&video_id) {
                        player.cleanup();
                    }
                    if let Some(mut player) = self.os.camera_players.remove(&video_id) {
                        player.cleanup();
                    }
                    if let Some(mut preview) = self.os.native_camera_previews.remove(&video_id) {
                        preview.cleanup();
                        IosApp::detach_camera_preview(video_id.0);
                    }

                    if let VideoSource::Camera(input_id, format_id) = source {
                        let wants_native = matches!(camera_preview_mode, CameraPreviewMode::Native);
                        let camera_access = self.os.media.av_capture();

                        if wants_native {
                            let mut preview = IosNativeCameraPreview::new(
                                video_id,
                                input_id,
                                format_id,
                                camera_access,
                            );
                            if let Some(Ok((
                                width,
                                height,
                                duration,
                                is_seekable,
                                video_tracks,
                                audio_tracks,
                            ))) = preview.check_prepared()
                            {
                                self.call_event_handler(&Event::VideoPlaybackPrepared(
                                    VideoPlaybackPreparedEvent {
                                        video_id,
                                        video_width: width,
                                        video_height: height,
                                        duration,
                                        is_seekable,
                                        video_tracks,
                                        audio_tracks,
                                    },
                                ));
                            }
                            self.os.native_camera_previews.insert(video_id, preview);
                            continue;
                        }

                        // Allocate YUV textures for composited camera path.
                        // Each plane needs a distinct texture — Texture::new() returns the
                        // shared null texture singleton, which would cause all three planes
                        // to write to the same pool entry and corrupt the null texture used
                        // by the rest of the UI.
                        let tex_y = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_u = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_v = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                        let tex_y_id = tex_y.texture_id();
                        let tex_u_id = tex_u.texture_id();
                        let tex_v_id = tex_v.texture_id();

                        let player = IosCameraPlayer::new(
                            video_id,
                            tex_y_id,
                            tex_u_id,
                            tex_v_id,
                            metal_cx.device,
                            input_id,
                            format_id,
                            camera_access,
                        );
                        self.os.camera_players.insert(video_id, player);
                        self.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady {
                                video_id,
                                tex_y,
                                tex_u,
                                tex_v,
                            },
                        ));
                        continue;
                    }

                    // Allocate YUV textures for software decode paths.
                    let tex_y = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                    let tex_u = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                    let tex_v = Texture::new_with_format(self, TextureFormat::VideoYuvPlane);
                    let tex_y_id = tex_y.texture_id();
                    let tex_u_id = tex_u.texture_id();
                    let tex_v_id = tex_v.texture_id();

                    let player = AppleUnifiedVideoPlayer::new(
                        metal_cx.device,
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
                    self.call_event_handler(&Event::VideoYuvTexturesReady(VideoYuvTexturesReady {
                        video_id,
                        tex_y,
                        tex_u,
                        tex_v,
                    }));
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.play();
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.pause();
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.resume();
                    }
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.mute();
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.unmute();
                    }
                }
                CxOsOp::CleanupVideoPlaybackResources(video_id) => {
                    if let Some(mut preview) = self.os.native_camera_previews.remove(&video_id) {
                        preview.cleanup();
                        IosApp::detach_camera_preview(video_id.0);
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                        continue;
                    }
                    if let Some(mut player) = self.os.camera_players.remove(&video_id) {
                        player.cleanup();
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                        continue;
                    }
                    if let Some(mut player) = self.os.video_players.remove(&video_id) {
                        player.cleanup();
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                    }
                }
                CxOsOp::SeekVideoPlayback(video_id, position_ms) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.seek_to(position_ms);
                    }
                }
                CxOsOp::SetVideoVolume(video_id, volume) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.set_volume(volume);
                    }
                }
                CxOsOp::SetVideoPlaybackRate(video_id, rate) => {
                    if self.os.camera_players.contains_key(&video_id)
                        || self.os.native_camera_previews.contains_key(&video_id)
                    {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.set_playback_rate(rate);
                    }
                }
                CxOsOp::PrepareAudioPlayback(video_id, source, autoplay, should_loop) => {
                    use crate::texture::TextureId;
                    let player = AppleUnifiedVideoPlayer::new(
                        metal_cx.device,
                        video_id,
                        TextureId::default(),
                        TextureId::default(),
                        TextureId::default(),
                        TextureId::default(),
                        source,
                        autoplay,
                        should_loop,
                    );
                    self.os.video_players.insert(video_id, player);
                }
                CxOsOp::CloseWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    if window.is_popup {
                        window.is_created = false;
                        window.is_popup = false;
                        window.popup_parent = None;
                        window.popup_position = None;
                        window.popup_size = None;
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

    fn check_camera_permission_status(&self) -> crate::permission::PermissionStatus {
        unsafe {
            let permission_status: i32 = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeVideo];
            match permission_status {
                3 => crate::permission::PermissionStatus::Granted,
                2 => crate::permission::PermissionStatus::DeniedPermanent,
                1 => crate::permission::PermissionStatus::DeniedPermanent,
                _ => crate::permission::PermissionStatus::NotDetermined,
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
            crate::permission::Permission::Camera => self.check_camera_permission_status(),
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
        let status = match permission {
            crate::permission::Permission::AudioInput => self.check_audio_permission_status(),
            crate::permission::Permission::Camera => self.check_camera_permission_status(),
        };
        match status {
            crate::permission::PermissionStatus::NotDetermined => match permission {
                crate::permission::Permission::AudioInput => {
                    self.ios_request_audio_permission(permission, request_id);
                }
                crate::permission::Permission::Camera => {
                    self.ios_request_camera_permission(permission, request_id);
                }
            },
            _ => {
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

    fn ios_request_camera_permission(
        &mut self,
        permission: crate::permission::Permission,
        request_id: i32,
    ) {
        let sender = self.os.permission_response.sender.clone();
        unsafe {
            let completion_handler = objc_block!(move |granted: BOOL| {
                let permission_result = crate::permission::PermissionResult {
                    permission,
                    request_id,
                    status: if granted == YES {
                        crate::permission::PermissionStatus::Granted
                    } else {
                        crate::permission::PermissionStatus::DeniedPermanent
                    },
                };
                let _ = sender.send(permission_result);
            });
            let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: AVMediaTypeVideo completionHandler: &completion_handler];
        }
    }
}

impl Cx {
    fn find_popup_to_dismiss_on_touch(
        &self,
        touches: &[crate::event::TouchPoint],
    ) -> Option<crate::window::WindowId> {
        use crate::window::CxWindowPool;
        for i in (0..self.windows.len()).rev() {
            let window_id = CxWindowPool::from_usize(i);
            let window = &self.windows[window_id];
            if !window.is_created || !window.is_popup {
                continue;
            }
            if let (Some(pos), Some(size)) = (window.popup_position, window.popup_size) {
                let rect = Rect { pos, size };
                for touch in touches {
                    if touch.state == crate::event::TouchState::Start && !rect.contains(touch.abs) {
                        return Some(window_id);
                    }
                }
            }
        }
        None
    }

    fn find_popup_to_dismiss_on_mouse(&self, abs: DVec2) -> Option<crate::window::WindowId> {
        use crate::window::CxWindowPool;
        for i in (0..self.windows.len()).rev() {
            let window_id = CxWindowPool::from_usize(i);
            let window = &self.windows[window_id];
            if !window.is_created || !window.is_popup {
                continue;
            }
            if let (Some(pos), Some(size)) = (window.popup_position, window.popup_size) {
                let rect = Rect { pos, size };
                if !rect.contains(abs) {
                    return Some(window_id);
                }
            }
        }
        None
    }

    fn dismiss_popup_window(
        &mut self,
        window_id: crate::window::WindowId,
        reason: crate::event::PopupDismissReason,
    ) {
        use crate::window::CxWindowPool;
        // First dismiss any child popups
        let children: Vec<crate::window::WindowId> = (0..self.windows.len())
            .filter_map(|i| {
                let child_id = CxWindowPool::from_usize(i);
                let w = &self.windows[child_id];
                if w.is_created && w.is_popup && w.popup_parent == Some(window_id) {
                    Some(child_id)
                } else {
                    None
                }
            })
            .collect();
        for child_id in children {
            self.dismiss_popup_window(child_id, crate::event::PopupDismissReason::ParentClosed);
        }
        self.call_event_handler(&Event::PopupDismissed(crate::event::PopupDismissedEvent {
            window_id,
            reason,
        }));
        self.call_event_handler(&Event::WindowClosed(crate::event::WindowClosedEvent {
            window_id,
        }));
        self.windows[window_id].is_created = false;
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
    pub(crate) video_players: HashMap<LiveId, AppleUnifiedVideoPlayer>,
    pub(crate) camera_players: HashMap<LiveId, IosCameraPlayer>,
    pub(crate) native_camera_previews: HashMap<LiveId, IosNativeCameraPreview>,
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
