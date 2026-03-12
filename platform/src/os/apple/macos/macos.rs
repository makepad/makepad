use {
    crate::{
        cx::{Cx, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        event::{
            video_playback::{
                CameraPreviewMode, VideoBufferedRangesEvent, VideoDecodingErrorEvent,
                VideoPlaybackPreparedEvent, VideoPlaybackResourcesReleasedEvent,
                VideoSeekableRangesEvent, VideoTextureUpdatedEvent, VideoYuvTexturesReady,
            },
            Event, GameInputEventChannel, MouseButton, MouseUpEvent, VideoSource, WindowGeom,
        },
        makepad_live_id::*,
        makepad_math::*,
        os::{
            apple::{
                apple_classes::init_apple_classes_global,
                apple_game_input::AppleGameInput,
                apple_sys::*,
                apple_util::str_to_nsstring,
                apple_video_player::AppleUnifiedVideoPlayer,
                macos::{
                    macos_app::{init_macos_app_global, with_macos_app, MacosApp},
                    macos_event::MacosEvent,
                    macos_window::MacosWindow,
                },
            },
            apple_media::CxAppleMedia,
            cx_native::EventFlow,
            metal::{DrawPassMode, MetalCx},
        },
        permission::Permission,
        shared_framebuf::PollTimers,
        texture::{Texture, TextureFormat},
        thread::SignalToUI,
        window::{CxWindowPool, WindowId},
    },
    makepad_objc_sys::{msg_send, objc_block, sel, sel_impl},
    std::{
        cell::RefCell,
        collections::HashMap,
        rc::Rc,
        sync::{Arc, Mutex},
        time::Instant,
    },
};

#[derive(Clone)]
pub struct MetalWindow {
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    cal_size: Vec2d,
    pub ca_layer: ObjcId,
    pub cocoa_window: Box<MacosWindow>,
    pub is_resizing: bool,
}

impl MetalWindow {
    pub(crate) fn new(
        window_id: WindowId,
        metal_cx: &MetalCx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
        is_fullscreen: bool,
    ) -> MetalWindow {
        let ca_layer: ObjcId = unsafe { msg_send![class!(CAMetalLayer), new] };

        let mut cocoa_window = Box::new(MacosWindow::new(window_id));

        cocoa_window.init(title, inner_size, position, is_fullscreen);
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            let () = msg_send![ca_layer, setDisplaySyncEnabled: YES];
            let () = msg_send![ca_layer, setNeedsDisplayOnBoundsChange: YES];
            let () = msg_send![ca_layer, setAutoresizingMask: (1 << 4) | (1 << 1)];
            let () = msg_send![ca_layer, setAllowsNextDrawableTimeout: NO];
            let () = msg_send![ca_layer, setDelegate: cocoa_window.view];
            let () = msg_send![ca_layer, setBackgroundColor: CGColorCreateGenericRGB(0.0, 0.0, 0.0, 1.0)];

            let view = cocoa_window.view;
            let () = msg_send![view, setWantsBestResolutionOpenGLSurface: YES];
            let () = msg_send![view, setWantsLayer: YES];
            let () = msg_send![view, setLayerContentsPlacement: 11];
            let () = msg_send![view, setLayer: ca_layer];
        }

        MetalWindow {
            is_resizing: false,
            window_id,
            cal_size: Vec2d::default(),
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window,
        }
    }

    pub(crate) fn new_popup(
        window_id: WindowId,
        metal_cx: &MetalCx,
        size: Vec2d,
        position: Vec2d,
        parent_window: ObjcId,
    ) -> MetalWindow {
        let ca_layer: ObjcId = unsafe { msg_send![class!(CAMetalLayer), new] };

        let mut cocoa_window = Box::new(MacosWindow::new(window_id));

        cocoa_window.init_popup(size, position, parent_window);
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            let () = msg_send![ca_layer, setDisplaySyncEnabled: YES];
            let () = msg_send![ca_layer, setNeedsDisplayOnBoundsChange: YES];
            let () = msg_send![ca_layer, setAutoresizingMask: (1 << 4) | (1 << 1)];
            let () = msg_send![ca_layer, setAllowsNextDrawableTimeout: NO];
            let () = msg_send![ca_layer, setDelegate: cocoa_window.view];
            let () = msg_send![ca_layer, setBackgroundColor: CGColorCreateGenericRGB(0.0, 0.0, 0.0, 1.0)];

            let view = cocoa_window.view;
            let () = msg_send![view, setWantsBestResolutionOpenGLSurface: YES];
            let () = msg_send![view, setWantsLayer: YES];
            let () = msg_send![view, setLayerContentsPlacement: 11];
            let () = msg_send![view, setLayer: ca_layer];
        }

        MetalWindow {
            is_resizing: false,
            window_id,
            cal_size: Vec2d::default(),
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window,
        }
    }

    pub(crate) fn start_resize(&mut self) {
        self.is_resizing = true;
        let () = unsafe { msg_send![self.ca_layer, setPresentsWithTransaction: YES] };
    }

    pub(crate) fn stop_resize(&mut self) {
        self.is_resizing = false;
        let () = unsafe { msg_send![self.ca_layer, setPresentsWithTransaction: NO] };
    }

    pub(crate) fn resize_core_animation_layer(&mut self, _metal_cx: &MetalCx) -> bool {
        let cal_size = Vec2d {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor,
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            unsafe {
                let () = msg_send![self.ca_layer, setDrawableSize: CGSize {width: cal_size.x, height: cal_size.y}];
                let () = msg_send![self.ca_layer, setContentsScale: self.window_geom.dpi_factor];
            }
            true
        } else {
            false
        }
    }
}

pub(crate) struct MacosNativeCameraPreview {
    input_id: crate::video::VideoInputId,
    format_id: crate::video::VideoFormatId,
    width: u32,
    height: u32,
    prepare_notified: bool,
    camera_access: Option<Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>>,
    attached_window: Option<WindowId>,
    host_view: ObjcId,
    preview_layer: ObjcId,
}

impl MacosNativeCameraPreview {
    fn new(
        input_id: crate::video::VideoInputId,
        format_id: crate::video::VideoFormatId,
        camera_access: Arc<Mutex<crate::os::apple::av_capture::AvCaptureAccess>>,
    ) -> Self {
        {
            let mut cam = camera_access.lock().unwrap();
            *cam.camera_frame_input_cb[0].lock().unwrap() = None;
            *cam.video_input_cb[0].lock().unwrap() = None;
            cam.use_video_input(&[(input_id, format_id)]);
        }
        let (width, height) = {
            let cam = camera_access.lock().unwrap();
            cam.format_size(input_id, format_id).unwrap_or((0, 0))
        };

        Self {
            input_id,
            format_id,
            width,
            height,
            prepare_notified: false,
            camera_access: Some(camera_access),
            attached_window: None,
            host_view: nil,
            preview_layer: nil,
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

    fn ensure_attached(&mut self, window_id: WindowId, parent_view: ObjcId, rect: Rect) {
        unsafe {
            if self.attached_window != Some(window_id) || self.host_view == nil {
                self.detach_preview();

                let host_view: ObjcId = msg_send![class!(NSView), alloc];
                let host_view: ObjcId = msg_send![host_view, initWithFrame: NSRect {
                    origin: NSPoint { x: rect.pos.x, y: rect.pos.y },
                    size: NSSize { width: rect.size.x.max(0.0), height: rect.size.y.max(0.0) }
                }];
                let () = msg_send![host_view, setWantsLayer: YES];
                let () = msg_send![parent_view, addSubview: host_view];

                if let Some(session) = self.session() {
                    let preview_layer: ObjcId =
                        msg_send![class!(AVCaptureVideoPreviewLayer), layerWithSession: session];
                    if preview_layer != nil {
                        let gravity = str_to_nsstring("AVLayerVideoGravityResizeAspectFill");
                        let () = msg_send![preview_layer, setVideoGravity: gravity];
                        let layer: ObjcId = msg_send![host_view, layer];
                        if layer != nil {
                            let () = msg_send![layer, addSublayer: preview_layer];
                            self.preview_layer = preview_layer;
                        }
                    }
                }

                self.host_view = host_view;
                self.attached_window = Some(window_id);
            }
        }
    }

    fn update_preview(
        &mut self,
        window_id: WindowId,
        parent_view: ObjcId,
        rect: Rect,
        visible: bool,
    ) {
        self.ensure_attached(window_id, parent_view, rect);
        unsafe {
            if self.host_view != nil {
                let frame = NSRect {
                    origin: NSPoint {
                        x: rect.pos.x,
                        y: rect.pos.y,
                    },
                    size: NSSize {
                        width: rect.size.x.max(0.0),
                        height: rect.size.y.max(0.0),
                    },
                };
                let () = msg_send![self.host_view, setFrame: frame];
                let () = msg_send![self.host_view, setHidden: if visible { NO } else { YES }];
                if self.preview_layer != nil {
                    let () = msg_send![self.preview_layer, setFrame: NSRect {
                        origin: NSPoint { x: 0.0, y: 0.0 },
                        size: NSSize { width: rect.size.x.max(0.0), height: rect.size.y.max(0.0) },
                    }];
                }
            }
        }
    }

    fn detach_preview(&mut self) {
        unsafe {
            if self.preview_layer != nil {
                let () = msg_send![self.preview_layer, removeFromSuperlayer];
                self.preview_layer = nil;
            }
            if self.host_view != nil {
                let () = msg_send![self.host_view, removeFromSuperview];
                self.host_view = nil;
            }
        }
        self.attached_window = None;
    }

    fn cleanup(&mut self) {
        self.detach_preview();
        if let Some(cam) = self.camera_access.take() {
            let mut cam = cam.lock().unwrap();
            cam.use_video_input(&[]);
            *cam.camera_frame_input_cb[0].lock().unwrap() = None;
            *cam.video_input_cb[0].lock().unwrap() = None;
        }
    }
}

impl Drop for MacosNativeCameraPreview {
    fn drop(&mut self) {
        self.cleanup();
    }
}

const KEEP_ALIVE_COUNT: usize = 5;
const TIMER0_DOWNSHIFT_IDLE_SECS: f64 = 0.2;

impl Cx {
    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Macos;
        let metal_cx: Rc<RefCell<MetalCx>> = Rc::new(RefCell::new(MetalCx::new()));

        // store device object ID for double buffering
        cx.borrow_mut().os.metal_device = Some(metal_cx.borrow().device);

        //let cx = Rc::new(RefCell::new(self));
        if crate::app_main::should_run_stdin_loop_from_env() {
            let mut cx = cx.borrow_mut();
            cx.in_makepad_studio = true;
            let mut metal_cx = metal_cx.borrow_mut();
            return cx.stdin_event_loop(&mut metal_cx);
        }

        let metal_windows = Rc::new(RefCell::new(Vec::new()));
        init_macos_app_global(Box::new({
            let cx = cx.clone();
            move |event| {
                let mut cx_ref = cx.borrow_mut();
                let mut metal_cx = metal_cx.borrow_mut();
                let mut metal_windows = metal_windows.borrow_mut();
                let event_flow =
                    cx_ref.cocoa_event_callback(event, &mut metal_cx, &mut metal_windows);
                let executor = cx_ref.executor.take().unwrap();
                drop(cx_ref);
                executor.run_until_stalled();
                let mut cx_ref = cx.borrow_mut();
                cx_ref.executor = Some(executor);
                event_flow
            }
        }));

        cx.borrow_mut().call_event_handler(&Event::Startup);
        cx.borrow_mut().redraw_all();
        // Start timer if there's initial work after startup
        if cx.borrow().need_redrawing() {
            cx.borrow_mut().ensure_timer0_started();
        }
        MacosApp::event_loop();
    }

    pub(crate) fn handle_repaint(
        &mut self,
        metal_windows: &mut Vec<MetalWindow>,
        metal_cx: &mut MetalCx,
    ) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        let time_now = with_macos_app(|app| app.time_now() as f32);
        for draw_pass_id in &passes_todo {
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        //let dpi_factor = metal_window.window_geom.dpi_factor;
                        metal_window.resize_core_animation_layer(&metal_cx);
                        let drawable: ObjcId =
                            unsafe { msg_send![metal_window.ca_layer, nextDrawable] };
                        if drawable == nil {
                            return;
                        }
                        self.passes[*draw_pass_id].set_time(time_now);
                        if metal_window.is_resizing {
                            self.draw_pass(
                                *draw_pass_id,
                                metal_cx,
                                DrawPassMode::Resizing(drawable),
                            );
                        } else {
                            self.draw_pass(
                                *draw_pass_id,
                                metal_cx,
                                DrawPassMode::Drawable(drawable),
                            );
                        }
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.passes[*draw_pass_id]
                        .set_time(with_macos_app(|app| app.time_now() as f32));
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
                CxDrawPassParent::None => {
                    self.passes[*draw_pass_id]
                        .set_time(with_macos_app(|app| app.time_now() as f32));
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
            }
        }
    }

    pub(crate) fn handle_networking_events(&mut self) {
        self.dispatch_network_runtime_events();
    }

    pub(crate) fn handle_gamepad_events(&mut self) {
        while let Ok(event) = self.os.game_input_events.receiver.try_recv() {
            if let Some(game_input) = &mut self.os.apple_game_input {
                match &event {
                    crate::event::game_input::GameInputConnectedEvent::Connected(info) => {
                        game_input.on_connected(info)
                    }
                    crate::event::game_input::GameInputConnectedEvent::Disconnected(info) => {
                        game_input.on_disconnected(info)
                    }
                }
            }
            self.call_event_handler(&Event::GameInputConnected(event));
        }

        if let Some(game_input) = &mut self.os.apple_game_input {
            game_input.poll();
        }
    }

    fn ensure_timer0_started(&mut self) {
        if !self.os.timer0_armed {
            with_macos_app(|app| app.stop_timer(0));
            with_macos_app(|app| app.start_timer(0, 0.008, true));
            self.os.timer0_armed = true;
            self.os.timer0_idle_since = None;
        }
    }

    fn ensure_timer0_stopped(&mut self) {
        if self.os.timer0_armed {
            with_macos_app(|app| app.stop_timer(0));
            with_macos_app(|app| app.start_timer(0, 0.2, true));
            self.os.timer0_armed = false;
        }
    }

    fn cocoa_event_callback(
        &mut self,
        event: MacosEvent,
        metal_cx: &mut MetalCx,
        metal_windows: &mut Vec<MetalWindow>,
    ) -> EventFlow {
        if let EventFlow::Exit = self.handle_platform_ops(metal_windows, metal_cx) {
            self.call_event_handler(&Event::Shutdown);
            return EventFlow::Exit;
        }
        // send a mouse up when dragging starts
        match &event {
            MacosEvent::MouseDown(_)
            | MacosEvent::MouseMove(_)
            | MacosEvent::MouseUp(_)
            | MacosEvent::Scroll(_)
            | MacosEvent::KeyDown(_)
            | MacosEvent::KeyUp(_)
            | MacosEvent::TextInput(_) => {
                self.os.keep_alive_counter = KEEP_ALIVE_COUNT;
                self.os.timer0_idle_since = None;
                self.ensure_timer0_started();
            }
            MacosEvent::Timer(te) => {
                if te.timer_id == 0 {
                    let mut needs_timer = false;

                    if self.screenshot_requests.len() > 0 {
                        self.repaint_windows();
                        needs_timer = true;
                    }
                    if self.os.keep_alive_counter > 0 {
                        self.os.keep_alive_counter -= 1;
                        needs_timer = true;
                    }

                    // check signals
                    if SignalToUI::check_and_clear_ui_signal() {
                        self.handle_media_signals();
                        self.handle_script_signals();
                        self.call_event_handler(&Event::Signal);
                        needs_timer = true;
                    }

                    if SignalToUI::check_and_clear_action_signal() {
                        self.handle_action_receiver();
                        needs_timer = true;
                    }
                    self.poll_control_channel();
                    self.handle_actions();

                    if self.any_passes_dirty()
                        || self.need_redrawing()
                        || !self.new_next_frames.is_empty()
                        || self.demo_time_repaint
                        || !self.os.video_players.is_empty()
                    {
                        needs_timer = true;
                    }

                    if needs_timer {
                        self.os.timer0_idle_since = None;
                        self.ensure_timer0_started();
                    } else {
                        let now = with_macos_app(|app| app.time_now());
                        if let Some(idle_since) = self.os.timer0_idle_since {
                            if now - idle_since >= TIMER0_DOWNSHIFT_IDLE_SECS {
                                self.ensure_timer0_stopped();
                            }
                        } else {
                            self.os.timer0_idle_since = Some(now);
                        }
                    }
                    self.run_live_edit_if_needed("macos");
                    self.handle_networking_events();
                    self.handle_gamepad_events();
                    self.cocoa_event_callback(MacosEvent::Paint, metal_cx, metal_windows);

                    // Run garbage collection if needed - safe moment after paint, before waiting
                    self.with_vm(|vm| {
                        if vm.heap().needs_gc() {
                            vm.gc();
                        }
                    });

                    // block till the next timer
                    return EventFlow::Wait;
                }
            }
            _ => (),
        }
        //self.process_desktop_pre_event(&mut event);
        match event {
            MacosEvent::WindowGotFocus(window_id) => {
                // repaint all window passes. Metal sometimes doesnt flip buffers when hidden/no focus
                for window in metal_windows.iter_mut() {
                    if let Some(main_pass_id) = self.windows[window.window_id].main_pass_id {
                        self.repaint_pass(main_pass_id);
                    }
                }
                self.call_event_handler(&Event::WindowGotFocus(window_id));
            }
            MacosEvent::WindowLostFocus(window_id) => {
                self.call_event_handler(&Event::WindowLostFocus(window_id));
            }
            MacosEvent::PopupDismissed(event) => {
                self.call_event_handler(&Event::PopupDismissed(event));
            }
            MacosEvent::WindowResizeLoopStart(window_id) => {
                if let Some(window) = metal_windows.iter_mut().find(|w| w.window_id == window_id) {
                    window.start_resize();
                }
            }
            MacosEvent::WindowResizeLoopStop(window_id) => {
                if let Some(window) = metal_windows.iter_mut().find(|w| w.window_id == window_id) {
                    window.stop_resize();
                }
            }
            MacosEvent::WindowGeomChange(mut re) => {
                // do this here because mac
                if let Some(window) = metal_windows
                    .iter_mut()
                    .find(|w| w.window_id == re.window_id)
                {
                    self.windows[re.window_id].os_dpi_factor = Some(re.new_geom.dpi_factor);
                    if let Some(dpi_override) = self.windows[re.window_id].dpi_override {
                        re.new_geom.inner_size *= re.new_geom.dpi_factor / dpi_override;
                        re.new_geom.dpi_factor = dpi_override;
                    }
                    window.window_geom = re.new_geom.clone();
                    self.windows[re.window_id].window_geom = re.new_geom.clone();

                    // redraw just this windows root draw list
                    if re.old_geom.dpi_factor != re.new_geom.dpi_factor
                        || re.old_geom.inner_size != re.new_geom.inner_size
                    {
                        if let Some(main_pass_id) = self.windows[re.window_id].main_pass_id {
                            self.redraw_pass_and_child_passes(main_pass_id);
                        }
                    }
                }
                // ok lets not redraw all, just this window
                self.call_event_handler(&Event::WindowGeomChange(re));
            }
            MacosEvent::WindowClosed(wc) => {
                // lets remove the window from the set
                let window_id = wc.window_id;
                self.call_event_handler(&Event::WindowClosed(wc));

                self.windows[window_id].is_created = false;
                if let Some(index) = metal_windows.iter().position(|w| w.window_id == window_id) {
                    metal_windows.remove(index);
                    if metal_windows.len() == 0 {
                        self.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                }
            }
            MacosEvent::Paint => {
                // Poll video players for new frames and preparation status
                let has_video_players = !self.os.video_players.is_empty();
                if has_video_players {
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

                let has_next_frames = self.new_next_frames.len() != 0;
                let time_now = with_macos_app(|app| app.time_now());
                if has_next_frames {
                    self.call_next_frame_event(time_now);
                }
                let needs_redrawing = self.need_redrawing();
                if needs_redrawing {
                    self.call_draw_event(time_now);
                    self.mtl_compile_shaders(&metal_cx);
                }
                let has_dirty_passes = self.any_passes_dirty();
                // Start timer if we have work
                if has_next_frames
                    || needs_redrawing
                    || has_dirty_passes
                    || self.screenshot_requests.len() > 0
                    || self.os.keep_alive_counter > 0
                    || self.demo_time_repaint
                    || has_video_players
                {
                    self.os.timer0_idle_since = None;
                    self.ensure_timer0_started();
                }

                // ok here we send out to all our childprocesses
                self.handle_repaint(metal_windows, metal_cx);
            }
            MacosEvent::MouseDown(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.fingers.process_tap_count(e.abs, e.time);
                self.fingers.mouse_down(e.button, e.window_id);
                self.call_event_handler(&Event::MouseDown(e.into()));
            }
            MacosEvent::MouseMove(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::MouseMove(e.into()));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            MacosEvent::MouseUp(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                let button = e.button;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
            }
            MacosEvent::Scroll(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::Scroll(e.into()));
            }
            MacosEvent::WindowDragQuery(mut e) => {
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            MacosEvent::WindowCloseRequested(e) => {
                self.call_event_handler(&Event::WindowCloseRequested(e))
            }
            MacosEvent::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
            MacosEvent::Drag(e) => {
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            }
            MacosEvent::Drop(e) => {
                self.call_event_handler(&Event::Drop(e));
                self.drag_drop.cycle_drag();
            }
            MacosEvent::DragEnd => {
                // lets send mousebutton ups to fix missing it.
                // TODO! make this more resilient
                self.call_event_handler(&Event::MouseUp(MouseUpEvent {
                    abs: dvec2(-100000.0, -100000.0),
                    button: MouseButton::PRIMARY,
                    window_id: CxWindowPool::id_zero(),
                    modifiers: Default::default(),
                    time: 0.0,
                }));
                self.fingers.mouse_up(MouseButton::PRIMARY);
                self.fingers.cycle_hover_area(live_id!(mouse).into());

                self.call_event_handler(&Event::DragEnd);
                self.drag_drop.cycle_drag();
            }
            MacosEvent::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e))
            }
            MacosEvent::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e))
            }
            MacosEvent::TextCopy(e) => self.call_event_handler(&Event::TextCopy(e)),
            MacosEvent::TextCut(e) => self.call_event_handler(&Event::TextCut(e)),
            MacosEvent::Timer(e) => {
                self.handle_script_timer(&e);
                self.call_event_handler(&Event::Timer(e));
                return EventFlow::Wait;
            }
            MacosEvent::MacosMenuCommand(e) => self.call_event_handler(&Event::MacosMenuCommand(e)),
            MacosEvent::PermissionResult(result) => {
                self.call_event_handler(&Event::PermissionResult(result))
            }
            MacosEvent::GameInputConnected(e) => {
                self.call_event_handler(&Event::GameInputConnected(e))
            }
        }

        // Determine the event flow based on whether we have work to do
        if self.any_passes_dirty()
            || self.need_redrawing()
            || self.new_next_frames.len() != 0
            || self.os.keep_alive_counter > 0
            || self.screenshot_requests.len() > 0
            || self.demo_time_repaint
            || self.os.timer0_armed
        {
            // We have work to do or timer is running
            EventFlow::Poll
        } else {
            // No work pending and timer is stopped - we can wait
            EventFlow::Wait
        }
    }

    fn dpi_override_scale(&self, pos: &mut Vec2d, window_id: WindowId) {
        *pos = self.windows[window_id].remap_dpi_override(*pos)
    }

    fn handle_platform_ops(
        &mut self,
        metal_windows: &mut Vec<MetalWindow>,
        metal_cx: &MetalCx,
    ) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        window.is_fullscreen,
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;
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
                    // Find the parent NSWindow handle for coordinate conversion
                    let parent_ns_window = metal_windows
                        .iter()
                        .find(|w| w.window_id == parent_window_id)
                        .map(|w| w.cocoa_window.window)
                        .unwrap_or(nil);
                    let metal_window = MetalWindow::new_popup(
                        window_id,
                        &metal_cx,
                        size,
                        position,
                        parent_ns_window,
                    );
                    window.window_geom = metal_window.window_geom.clone();
                    metal_windows.push(metal_window);
                    window.is_created = true;
                }
                CxOsOp::ResizeWindow(window_id, size) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.set_outer_size(size);
                    }
                }
                CxOsOp::RepositionWindow(window_id, pos) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.set_position(pos);
                    }
                }
                CxOsOp::CloseWindow(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        self.windows[window_id].is_created = false;
                        metal_window.cocoa_window.close_window();
                        break;
                    }
                }
                CxOsOp::Quit => {
                    return EventFlow::Exit;
                }
                CxOsOp::MinimizeWindow(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.minimize();
                    }
                }
                CxOsOp::Deminiaturize(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.deminiaturize();
                    }
                }
                CxOsOp::MaximizeWindow(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.maximize();
                    }
                }
                CxOsOp::RestoreWindow(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.restore();
                    }
                }
                CxOsOp::HideWindow(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.hide();
                    }
                }
                CxOsOp::HideWindowButtons(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.set_window_buttons_visible(false);
                    }
                }
                CxOsOp::ShowWindowButtons(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.set_window_buttons_visible(true);
                    }
                }
                CxOsOp::ShowTextIME(area, pos, _config) => {
                    let pos = area.clipped_rect(self).pos + pos;
                    metal_windows.iter_mut().for_each(|w| {
                        w.cocoa_window.set_ime_active(true);
                        w.cocoa_window.set_ime_spot(pos);
                    });
                }
                CxOsOp::HideTextIME => {
                    metal_windows.iter_mut().for_each(|w| {
                        w.cocoa_window.set_ime_active(false);
                        w.cocoa_window.set_ime_spot(dvec2(0.0, 0.0));
                    });
                }
                CxOsOp::SetCursor(cursor) => {
                    with_macos_app(|app| app.set_mouse_cursor(cursor));
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    with_macos_app(|app| app.start_timer(timer_id, interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    with_macos_app(|app| app.stop_timer(timer_id));
                }
                CxOsOp::StartDragging(items) => {
                    //  lets start dragging on the right window
                    if let Some(metal_window) = metal_windows.iter_mut().next() {
                        metal_window.cocoa_window.start_dragging(items);
                        break;
                    }
                }
                CxOsOp::UpdateMacosMenu(menu) => with_macos_app(|app| app.update_macos_menu(&menu)),
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let _ = self.net.http_start(request_id, request);
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    let _ = self.net.http_cancel(request_id);
                }
                // These ops are mobile-only (soft keyboard, clipboard UI); no-op on macOS
                CxOsOp::SyncImeState { .. } => {}
                CxOsOp::ShowClipboardActions { .. } => {}
                CxOsOp::HideClipboardActions => {}
                CxOsOp::CopyToClipboard(content) => {
                    with_macos_app(|app| app.copy_to_clipboard(&content));
                }
                CxOsOp::SetPrimarySelection(_) => {}
                CxOsOp::ShowSelectionHandles { .. } => {}
                CxOsOp::UpdateSelectionHandles { .. } => {}
                CxOsOp::HideSelectionHandles => {}
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::AttachCameraNativePreview { video_id, area } => {
                    let Some(draw_list_id) = area.draw_list_id() else {
                        continue;
                    };
                    let Some(draw_pass_id) = self.draw_lists[draw_list_id].draw_pass_id else {
                        continue;
                    };
                    let Some(window_id) = self.get_pass_window_id(draw_pass_id) else {
                        continue;
                    };
                    let Some(metal_window) =
                        metal_windows.iter().find(|w| w.window_id == window_id)
                    else {
                        continue;
                    };

                    let mut rect = area.clipped_rect(self);
                    let win_h = self.windows[window_id].window_geom.inner_size.y;
                    rect.pos.y = (win_h - rect.pos.y - rect.size.y).max(0.0);
                    let parent_view = metal_window.cocoa_window.view;

                    if let Some(preview) = self.os.native_camera_previews.get_mut(&video_id) {
                        preview.update_preview(window_id, parent_view, rect, true);
                    }
                }
                CxOsOp::UpdateCameraNativePreview {
                    video_id,
                    area,
                    visible,
                } => {
                    let Some(draw_list_id) = area.draw_list_id() else {
                        continue;
                    };
                    let Some(draw_pass_id) = self.draw_lists[draw_list_id].draw_pass_id else {
                        continue;
                    };
                    let Some(window_id) = self.get_pass_window_id(draw_pass_id) else {
                        continue;
                    };
                    let Some(metal_window) =
                        metal_windows.iter().find(|w| w.window_id == window_id)
                    else {
                        continue;
                    };

                    let mut rect = area.clipped_rect(self);
                    let win_h = self.windows[window_id].window_geom.inner_size.y;
                    rect.pos.y = (win_h - rect.pos.y - rect.size.y).max(0.0);
                    let parent_view = metal_window.cocoa_window.view;

                    if let Some(preview) = self.os.native_camera_previews.get_mut(&video_id) {
                        preview.update_preview(window_id, parent_view, rect, visible);
                    }
                }
                CxOsOp::DetachCameraNativePreview { video_id } => {
                    if let Some(preview) = self.os.native_camera_previews.get_mut(&video_id) {
                        preview.detach_preview();
                    }
                }
                CxOsOp::SaveFileDialog(settings) => {
                    with_macos_app(|app| app.open_save_file_dialog(settings));
                }

                CxOsOp::SelectFileDialog(settings) => {
                    with_macos_app(|app| app.open_select_file_dialog(settings));
                }

                CxOsOp::SaveFolderDialog(settings) => {
                    with_macos_app(|app| app.open_save_folder_dialog(settings));
                }

                CxOsOp::SelectFolderDialog(settings) => {
                    with_macos_app(|app| app.open_select_folder_dialog(settings));
                }
                CxOsOp::ShowInDock(show) => {
                    with_macos_app(|app| app.show_in_dock(show));
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
                    if let Some(mut preview) = self.os.native_camera_previews.remove(&video_id) {
                        preview.cleanup();
                    }

                    if let VideoSource::Camera(input_id, format_id) = source {
                        if matches!(camera_preview_mode, CameraPreviewMode::Texture) {
                            crate::log!(
                                "VIDEO: macOS camera texture mode is not available, using native preview"
                            );
                        }
                        let camera_access = self.os.media.av_capture();
                        let mut preview =
                            MacosNativeCameraPreview::new(input_id, format_id, camera_access);
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

                    // Allocate YUV textures internally for software/NV12 decode path
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
                    // Notify widget so it can bind textures to shader slots
                    self.call_event_handler(&Event::VideoYuvTexturesReady(VideoYuvTexturesReady {
                        video_id,
                        tex_y,
                        tex_u,
                        tex_v,
                    }));
                    // Keep timer alive so we can poll for video frames
                    self.ensure_timer0_started();
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.play();
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.pause();
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.resume();
                    }
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.mute();
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.unmute();
                    }
                }
                CxOsOp::CleanupVideoPlaybackResources(video_id) => {
                    if let Some(mut preview) = self.os.native_camera_previews.remove(&video_id) {
                        preview.cleanup();
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
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get_mut(&video_id) {
                        player.seek_to(position_ms);
                    }
                }
                CxOsOp::SetVideoVolume(video_id, volume) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(player) = self.os.video_players.get(&video_id) {
                        player.set_volume(volume);
                    }
                }
                CxOsOp::SetVideoPlaybackRate(video_id, rate) => {
                    if self.os.native_camera_previews.contains_key(&video_id) {
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
                    self.ensure_timer0_started();
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        EventFlow::Poll
    }

    fn check_audio_permission_status(&self) -> crate::permission::PermissionStatus {
        unsafe {
            let permission_status: i32 = msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio];
            match permission_status {
                3 => crate::permission::PermissionStatus::Granted, // AVAuthorizationStatusAuthorized
                2 => crate::permission::PermissionStatus::DeniedPermanent, // AVAuthorizationStatusDenied - macOS doesn't re-prompt
                1 => crate::permission::PermissionStatus::DeniedPermanent, // AVAuthorizationStatusRestricted
                _ => crate::permission::PermissionStatus::NotDetermined, // AVAuthorizationStatusNotDetermined (0) or unknown
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

    fn handle_permission_check(&mut self, permission: Permission, request_id: i32) {
        let status = match permission {
            Permission::AudioInput => self.check_audio_permission_status(),
            Permission::Camera => self.check_camera_permission_status(),
        };

        self.call_event_handler(&crate::event::Event::PermissionResult(
            crate::permission::PermissionResult {
                permission,
                request_id,
                status,
            },
        ));
    }

    fn handle_permission_request(&mut self, permission: Permission, request_id: i32) {
        let status = match permission {
            Permission::AudioInput => self.check_audio_permission_status(),
            Permission::Camera => self.check_camera_permission_status(),
        };
        match status {
            crate::permission::PermissionStatus::NotDetermined => match permission {
                Permission::AudioInput => {
                    self.macos_request_audio_permission(permission, request_id)
                }
                Permission::Camera => self.macos_request_camera_permission(permission, request_id),
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

    fn macos_request_audio_permission(&mut self, permission: Permission, request_id: i32) {
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

                // Dispatch callback to main thread
                // AVCaptureDevice completion handlers run on arbitrary background threads
                Self::dispatch_permission_result_to_main_thread(permission_result);
            });

            let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType:AVMediaTypeAudio completionHandler:&completion_handler];
        }
    }

    fn macos_request_camera_permission(&mut self, permission: Permission, request_id: i32) {
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

                Self::dispatch_permission_result_to_main_thread(permission_result);
            });

            let () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType:AVMediaTypeVideo completionHandler:&completion_handler];
        }
    }

    fn dispatch_permission_result_to_main_thread(
        permission_result: crate::permission::PermissionResult,
    ) {
        unsafe {
            let result_clone = permission_result.clone();

            // Create a block that will be executed on the main thread
            let main_thread_block = objc_block!(move || {
                MacosApp::do_callback(MacosEvent::PermissionResult(result_clone.clone()));
            });

            // Use NSOperationQueue.mainQueue to dispatch to main thread
            let main_queue: ObjcId = msg_send![class!(NSOperationQueue), mainQueue];
            let block_operation: ObjcId =
                msg_send![class!(NSBlockOperation), blockOperationWithBlock: &main_thread_block];
            let () = msg_send![main_queue, addOperation: block_operation];
        }
    }
}

impl CxOsApi for Cx {
    fn pre_start() -> bool {
        init_apple_classes_global();
        false
    }

    fn init_cx_os(&mut self) {
        self.os.start_time = Some(Instant::now());
        if let Some(item) = std::option_env!("MAKEPAD_PACKAGE_DIR") {
            self.package_root = Some(item.to_string());
        }
        //self.live_expand();
        #[cfg(debug_assertions)]
        if !Self::has_studio_web_socket() {
            //self.start_disk_live_file_watcher(100);
        }
        //self.live_scan_dependencies();

        #[cfg(apple_bundle)]
        self.apple_bundle_load_dependencies();
        #[cfg(not(apple_bundle))]
        self.native_load_dependencies();

        let sender = self.os.game_input_events.sender.clone();
        self.os.apple_game_input = Some(AppleGameInput::init(move |event| {
            let _ = sender.send(event);
            SignalToUI::set_ui_signal();
        }));
    }

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(f);
    }

    fn start_stdin_service(&mut self) {
        // macOS studio mode routes control and frame messages over websocket.
        // No separate stdin-side texture sharing service is required.
    }

    fn seconds_since_app_start(&self) -> f64 {
        Instant::now()
            .duration_since(self.os.start_time.unwrap())
            .as_secs_f64()
    }

    fn open_url(&mut self, url: &str, _in_place: OpenUrlInPlace) {
        // Use the macOS `open` command to open URLs
        let _ = std::process::Command::new("open").arg(url).spawn();
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
    /// For how long to keep the timer alive when the app is idle
    pub(crate) keep_alive_counter: usize,
    /// Indicates wether the main timer is armed
    pub(crate) timer0_armed: bool,
    /// Start time of the current idle stretch while timer0 is armed.
    pub(crate) timer0_idle_since: Option<f64>,
    pub(crate) media: CxAppleMedia,
    pub(crate) bytes_written: usize,
    pub(crate) draw_calls_done: usize,
    pub(crate) instances_done: u64,
    pub(crate) vertices_done: u64,
    pub(crate) instance_bytes_uploaded: u64,
    pub(crate) uniform_bytes_uploaded: u64,
    pub(crate) vertex_buffer_bytes_uploaded: u64,
    pub(crate) texture_bytes_uploaded: u64,
    pub(crate) stdin_timers: PollTimers,
    pub(crate) start_time: Option<Instant>,
    pub metal_device: Option<ObjcId>,
    pub(crate) game_input_events: GameInputEventChannel,
    pub(crate) apple_game_input: Option<AppleGameInput>,
    pub(crate) video_players: HashMap<LiveId, AppleUnifiedVideoPlayer>,
    pub(crate) native_camera_previews: HashMap<LiveId, MacosNativeCameraPreview>,
}
