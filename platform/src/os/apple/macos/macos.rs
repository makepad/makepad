use {
    crate::{
        cx::{Cx, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        event::{
            drag_drop::{DragEvent, DragItem, DragResponse, DropEvent},
            video_playback::{
                CameraPreviewMode, VideoBufferedRangesEvent, VideoDecodingErrorEvent,
                VideoPlaybackPreparedEvent, VideoPlaybackResourcesReleasedEvent,
                VideoSeekableRangesEvent, VideoTextureUpdatedEvent, VideoYuvTexturesReady,
            },
            Event, GameInputEventChannel, MouseButton, MouseUpEvent, QuitReason, VideoSource,
            WindowGeom,
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
                apple_webview::MacosSystemBrowser,
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
        window::{CxWindowPool, MacosWindowConfig, WindowId},
        PlaybackPrepared,
    },
    makepad_objc_sys::{msg_send, objc_block, sel, sel_impl},
    std::{
        cell::RefCell,
        collections::HashMap,
        rc::Rc,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    },
};

/// NSWindowOcclusionStateVisible: some part of the window is on screen.
const NS_WINDOW_OCCLUSION_STATE_VISIBLE: usize = 1 << 1;

/// Presented-handlers normally land within a few vsyncs; a gate closed this
/// long means they were lost (occlusion, display sleep) and won't come.
const PRESENT_GATE_STUCK_TIMEOUT: Duration = Duration::from_millis(250);

/// In-flight presents at which the gate closes and the beat is skipped: the
/// drawable pool holds three, so acquiring with fewer outstanding can't block.
const PRESENT_GATE_IN_FLIGHT: u32 = 3;

/// How long we trust `occlusionState` before presenting anyway. The flag can
/// stick on "hidden" while the window is really on screen, which used to skip
/// every beat forever.
const OCCLUSION_PROBE_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct MetalWindow {
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    cal_size: Vec2d,
    pub ca_layer: ObjcId,
    pub cocoa_window: Box<MacosWindow>,
    pub is_resizing: bool,
    /// Frames acquired but not yet on glass. Present-gated pacing skips a
    /// paint beat instead of letting `nextDrawable` block the main thread
    /// when the compositor consumes frames unevenly (mirrored/scaled
    /// displays throttle in 10-25ms phases).
    /// Packed: low 32 bits are the count, high 32 bits are a reset generation,
    /// so a handler armed before a watchdog reset can't decrement a newer count.
    pub in_flight_presents: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Paced by CAMetalDisplayLink, which owns the layer's drawables: a paint
    /// must use the drawable its update hands over; `nextDrawable` meanwhile
    /// raises CAMetalLayerInvalidOperation (took visible windows down at launch).
    pub link_is_metal: bool,
    /// When the present gate started skipping beats, so a gate whose
    /// handlers were lost can be forced back open instead of wedging.
    gate_closed_since: Option<Instant>,
    /// When we started skipping beats because the window reported itself
    /// hidden, so a stale `occlusionState` can't skip forever.
    occluded_since: Option<Instant>,
}

impl MetalWindow {
    pub(crate) fn new(
        window_id: WindowId,
        metal_cx: &MetalCx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
        is_fullscreen: bool,
        macos_config: MacosWindowConfig,
    ) -> MetalWindow {
        let ca_layer: ObjcId = unsafe { msg_send![class!(CAMetalLayer), new] };

        let mut cocoa_window = Box::new(MacosWindow::new(window_id, macos_config));

        cocoa_window.init(title, inner_size, position, is_fullscreen, macos_config);
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            // MAKEPAD_NO_VSYNC=1: A/B switch — with display sync off,
            // nextDrawable never throttles to compositor consumption (may
            // tear). Distinguishes "our frames are slow" from "the
            // compositor returns drawables slowly/unevenly".
            let () = msg_send![ca_layer, setDisplaySyncEnabled:
                if std::env::var_os("MAKEPAD_NO_VSYNC").is_some() { NO } else { YES }];
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
            link_is_metal: false,
            window_id,
            cal_size: Vec2d::default(),
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window,
            in_flight_presents: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            gate_closed_since: None,
            occluded_since: None,
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

        let mut cocoa_window = Box::new(MacosWindow::new_popup(window_id));

        cocoa_window.init_popup(size, position, parent_window);
        unsafe {
            let () = msg_send![ca_layer, setDevice: metal_cx.device];
            let () = msg_send![ca_layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let () = msg_send![ca_layer, setPresentsWithTransaction: NO];
            let () = msg_send![ca_layer, setMaximumDrawableCount: 3];
            // MAKEPAD_NO_VSYNC=1: A/B switch — with display sync off,
            // nextDrawable never throttles to compositor consumption (may
            // tear). Distinguishes "our frames are slow" from "the
            // compositor returns drawables slowly/unevenly".
            let () = msg_send![ca_layer, setDisplaySyncEnabled:
                if std::env::var_os("MAKEPAD_NO_VSYNC").is_some() { NO } else { YES }];
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
            link_is_metal: false,
            window_id,
            cal_size: Vec2d::default(),
            ca_layer,
            window_geom: cocoa_window.get_window_geom(),
            cocoa_window,
            in_flight_presents: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            gate_closed_since: None,
            occluded_since: None,
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

    /// Drops and recreates the layer's drawable pool the way a real window
    /// resize does, reclaiming drawables whose presented-handlers never fired.
    /// Each size goes in its own committed transaction, since two writes in one
    /// transaction coalesce to no net change and the pool survives untouched.
    pub(crate) fn rebuild_drawable_pool(&mut self) {
        let s = self.cal_size;
        for height in [s.y + 1.0, s.y] {
            unsafe {
                let () = msg_send![class!(CATransaction), begin];
                let () = msg_send![class!(CATransaction), setDisableActions: YES];
                let () = msg_send![self.ca_layer, setDrawableSize: CGSize {width: s.x, height: height}];
                let () = msg_send![class!(CATransaction), commit];
                let () = msg_send![class!(CATransaction), flush];
            }
        }
        // Bump the generation as we zero the count, so the overdue handlers
        // (late, not lost) can't steal decrements from newer frames.
        let _ = self.in_flight_presents.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |w| Some(((w >> 32).wrapping_add(1)) << 32),
        );
        self.gate_closed_since = None;
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

    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        if self.prepare_notified {
            return None;
        }
        self.prepare_notified = true;
        Some(Ok(PlaybackPrepared::new(
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
    /// Bring this app's windows to the front, as if the user clicked its Dock icon.
    /// Useful for test automation driving an unfocused (or occluded) instance.
    /// `orderFrontRegardless` raises the windows even when macOS's cooperative
    /// activation rules deny the app focus.
    pub fn macos_activate_app(&mut self) {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![ns_app, activateIgnoringOtherApps: YES];
            with_macos_app(|app| {
                for (window, _view) in &app.cocoa_windows {
                    if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() {
                        continue;
                    }
                    let () = msg_send![*window, orderFrontRegardless];
                }
            });
        }
    }

    pub fn event_loop(cx: Rc<RefCell<Cx>>) {
        cx.borrow_mut().self_ref = Some(cx.clone());
        cx.borrow_mut().os_type = OsType::Macos;
        let metal_cx: Rc<RefCell<MetalCx>> = Rc::new(RefCell::new(MetalCx::new()));

        // store device object ID for double buffering
        cx.borrow_mut().os.metal_device = Some(metal_cx.borrow().device);
        cx.borrow_mut().publish_metal_device_for_media();

        //let cx = Rc::new(RefCell::new(self));
        cx.borrow_mut().set_physical_keyboard_state(true);
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

    // `pass_root_window` now lives in os/cx_shared.rs — the Windows frame-latency
    // beat needs the exact same lookup, so it is shared rather than duplicated.

    pub(crate) fn handle_repaint(
        &mut self,
        metal_windows: &mut Vec<MetalWindow>,
        metal_cx: &mut MetalCx,
    ) {
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        // Safety flush: if a previous repaint batched offscreen passes but
        // no window pass followed (texture-only frame), commit that work
        // now so it is never stranded.
        if let Some(shared) = metal_cx.frame_command_buffer.take() {
            let () = unsafe { msg_send![shared, commit] };
            let () = unsafe { msg_send![shared, release] };
        }
        let time_now = self
            .os
            .link_flip_time
            .map(|t| t as f32)
            .unwrap_or_else(|| with_macos_app(|app| app.time_now() as f32));
        let scope = self.os.link_scope;
        // Some(drawable), including Some(nil), means this beat came from a
        // CAMetalDisplayLinkUpdate. None keeps the legacy CADisplayLink /
        // NSTimer path on CAMetalLayer.nextDrawable.
        let link_drawable = self.os.link_drawable;
        let link_target_presentation_time = self.os.link_target_presentation_time;
        for draw_pass_id in &passes_todo {
            // Per-window pacing: during a LinkFire beat only the firing
            // window's pass tree paints; everything else stays dirty for
            // its OWN flip.
            if let Some(scope) = scope {
                if let Some(window_id) = self.pass_root_window(*draw_pass_id) {
                    let matches = metal_windows.iter().any(|mw| {
                        mw.window_id == window_id
                            && mw.cocoa_window.window as usize == scope
                    });
                    if !matches {
                        self.repaint_pass(*draw_pass_id);
                        continue;
                    }
                }
            }
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        //let dpi_factor = metal_window.window_geom.dpi_factor;
                        metal_window.resize_core_animation_layer(&metal_cx);
                        use std::sync::atomic::Ordering;
                        let in_flight = (metal_window.in_flight_presents.load(Ordering::Acquire)
                            & 0xffff_ffff) as u32;
                        // An occluded window gets no compositor vsync: presents never reach
                        // glass and an exhausted pool would block nextDrawable forever.
                        // Skip and keep the pass dirty, but only for so long, since this
                        // flag can stick on "hidden" while the window is really on screen.
                        let occlusion: usize = if link_drawable.is_none() {
                            unsafe {
                                msg_send![metal_window.cocoa_window.window, occlusionState]
                            }
                        } else {
                            NS_WINDOW_OCCLUSION_STATE_VISIBLE
                        };
                        if occlusion & NS_WINDOW_OCCLUSION_STATE_VISIBLE == 0 {
                            if in_flight >= PRESENT_GATE_IN_FLIGHT {
                                metal_window.gate_closed_since.get_or_insert_with(Instant::now);
                            }
                            let now = Instant::now();
                            let since = *metal_window.occluded_since.get_or_insert(now);
                            if now.duration_since(since) < OCCLUSION_PROBE_INTERVAL {
                                self.repaint_pass(*draw_pass_id);
                                continue;
                            }
                            // Fall through and present anyway: if the flag is stale we
                            // recover, and if it's honest we spent one frame to find out.
                            // The gate below still rebuilds the pool first if it's full.
                            metal_window.occluded_since = Some(now);
                        } else {
                            metal_window.occluded_since = None;
                        }
                        // Present-gated pacing: with display sync on, a full
                        // drawable pool makes nextDrawable BLOCK the main
                        // thread until the compositor consumes a frame
                        // (10-25ms phases on mirrored/scaled displays). Skip
                        // this beat and keep the pass dirty; the next timer
                        // beat retries with the pool drained and event
                        // handling never stalls behind vsync.
                        if link_drawable.is_none() && in_flight >= PRESENT_GATE_IN_FLIGHT {
                            let now = Instant::now();
                            let since =
                                *metal_window.gate_closed_since.get_or_insert(now);
                            if now.duration_since(since) < PRESENT_GATE_STUCK_TIMEOUT {
                                self.repaint_pass(*draw_pass_id);
                                continue;
                            }
                            // Handlers this overdue are lost, so reclaim their
                            // drawables before the present below can block on
                            // an exhausted pool.
                            // Quiet while hidden: the probe above trips this every time.
                            if metal_window.occluded_since.is_none() {
                                crate::error!(
                                    "present gate stuck for {:?} with {} in flight, rebuilding drawable pool",
                                    now.duration_since(since), in_flight,
                                );
                            }
                            metal_window.rebuild_drawable_pool();
                        }
                        metal_window.gate_closed_since = None;
                        // PerfMonitor: a presented window frame starts here;
                        // nextDrawable is where vsync/pool pressure blocks
                        // the main thread, so it gets its own channel.
                        self.perf_monitor
                            .frame_boundary(with_macos_app(|app| app.time_now()));
                        if link_drawable.is_none() && metal_window.link_is_metal {
                            // The layer's display link owns the drawables and this beat did
                            // not come from it: leave the pass dirty, the next update paints it.
                            self.repaint_pass(*draw_pass_id);
                            return;
                        }
                        let drawable = if let Some(drawable) = link_drawable {
                            drawable
                        } else {
                            let wait_t0 = std::time::Instant::now();
                            let drawable: ObjcId =
                                unsafe { msg_send![metal_window.ca_layer, nextDrawable] };
                            self.perf_monitor.add(
                                crate::perf_monitor::PERF_CHANNEL_DRAWABLE_WAIT,
                                wait_t0.elapsed().as_micros() as u64,
                            );
                            drawable
                        };
                        if drawable == nil {
                            self.repaint_pass(*draw_pass_id);
                            return;
                        }
                        let generation = link_drawable.is_none().then(|| {
                            let prev = metal_window
                                .in_flight_presents
                                .fetch_add(1, Ordering::AcqRel);
                            (prev >> 32) as u32
                        });
                        let in_flight = metal_window.in_flight_presents.clone();
                        let frame_target = link_target_presentation_time;
                        let () = unsafe {
                            msg_send![
                                drawable,
                                addPresentedHandler: &objc_block!(move | drawable_: ObjcId | {
                                    // RIG (MAKEPAD_PRESENT_TRACE=1): actual
                                    // GLASS times — the CPU trace's blind
                                    // spot where dropped/slipped frames live.
                                    if std::env::var_os("MAKEPAD_PRESENT_TRACE").is_some() {
                                        let t: f64 = unsafe { msg_send![drawable_, presentedTime] };
                                        static LAST: std::sync::atomic::AtomicU64 =
                                            std::sync::atomic::AtomicU64::new(0);
                                        let prev = f64::from_bits(LAST.swap(
                                            t.to_bits(),
                                            std::sync::atomic::Ordering::AcqRel,
                                        ));
                                        if prev > 0.0 && t > prev {
                                            eprintln!("presenttrace {:.2}ms", (t - prev) * 1000.0);
                                        }
                                    }
                                    if std::env::var_os("MAKEPAD_FRAME_TRACE").is_some()
                                        && frame_target > 0.0
                                    {
                                        let actual: f64 = unsafe { msg_send![drawable_, presentedTime] };
                                        eprintln!(
                                            "[frame-trace] target={:.9} actual={:.9} delta_ms={:+.3}",
                                            frame_target,
                                            actual,
                                            (actual - frame_target) * 1000.0,
                                        );
                                    }
                                    // No-op if a watchdog reset happened since this present.
                                    if let Some(generation) = generation {
                                        let _ = in_flight.fetch_update(
                                            std::sync::atomic::Ordering::AcqRel,
                                            std::sync::atomic::Ordering::Acquire,
                                            |w| ((w >> 32) as u32 == generation && w & 0xffff_ffff != 0)
                                                .then(|| w - 1),
                                        );
                                    }
                                })
                            ]
                        };
                        self.passes[*draw_pass_id].set_time(time_now);
                        let presented = if link_drawable.is_some() {
                            // This drawable came from a CAMetalDisplayLink update,
                            // which already schedules it for the update's target
                            // presentation time; `presentDrawable:atTime:` on it
                            // raises CAMetalDrawableInvalidOperation and took
                            // every visible window down at launch. Present it
                            // plainly — the link does the pacing. The target time
                            // still feeds the frame trace above.
                            self.draw_pass(
                                *draw_pass_id,
                                metal_cx,
                                DrawPassMode::Drawable(drawable, None),
                            )
                        } else if metal_window.is_resizing {
                            self.draw_pass(
                                *draw_pass_id,
                                metal_cx,
                                DrawPassMode::Resizing(drawable),
                            )
                        } else {
                            self.draw_pass(
                                *draw_pass_id,
                                metal_cx,
                                DrawPassMode::Drawable(drawable, None),
                            )
                        };
                        // The pass bailed before presenting, so its handler never
                        // fires. Give the count back or the gate closes for good.
                        if !presented && generation.is_some() {
                            let generation = generation.unwrap();
                            let _ = metal_window.in_flight_presents.fetch_update(
                                Ordering::AcqRel,
                                Ordering::Acquire,
                                |w| ((w >> 32) as u32 == generation && w & 0xffff_ffff != 0)
                                    .then(|| w - 1),
                            );
                        }
                    }
                }
                // Offscreen passes get the SAME stamp as the window pass
                // that consumes them (it was wall-now per pass: a child
                // pass and its consumer could disagree by the encode time
                // between them, and neither matched NextFrame).
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.passes[*draw_pass_id].set_time(time_now);
                    self.draw_pass(*draw_pass_id, metal_cx, DrawPassMode::Texture);
                }
                CxDrawPassParent::None => {
                    self.passes[*draw_pass_id].set_time(time_now);
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
        // FRAME-FLIP pacing: the display link IS the refresh — one beat per
        // actual flip, phase-locked, tracking the window's own panel. The
        // NSTimer stays as the fallback (no window yet, pre-macOS-14) and
        // as the idle heartbeat. MAKEPAD_DISPLAY_LINK=0 forces timer pacing.
        static WANT_LINK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let want_link = *WANT_LINK.get_or_init(|| {
            // NSView.displayLink never fires for a window that is not on
            // screen — hidden eval/test runs (MAKEPAD_HIDE_WINDOWS) must
            // pace on the timer or they freeze.
            std::env::var("MAKEPAD_DISPLAY_LINK").map(|v| v != "0").unwrap_or(true)
                && std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_none()
        });
        // Self-heal: a window close invalidated the link while the beat
        // thought itself armed — re-anchor on a surviving window.
        if self.os.timer0_armed
            && want_link
            && with_macos_app(|app| app.display_link_needs_rearm())
        {
            self.os.timer0_armed = false;
        }
        if !self.os.timer0_armed {
            with_macos_app(|app| app.stop_timer(0));
            if want_link && with_macos_app(|app| app.ensure_display_link()) {
                self.os.timer0_armed = true;
                self.os.timer0_idle_since = None;
                return;
            }
            // Pace the paint clock to the fastest attached display. The old
            // fixed 8ms beat against an 8.33ms (120Hz) refresh: presents
            // outran vsync, the drawable pool drifted full and nextDrawable
            // blocked the main thread in a ~25-frame sawtooth (rough/smooth
            // phases as the beat drifted through vblank alignment). Matching
            // the refresh period (+0.2% so NSTimer lateness drains the queue
            // instead of accumulating) keeps acquisition non-blocking.
            let interval = unsafe {
                let screens: ObjcId = msg_send![class!(NSScreen), screens];
                let count: usize = msg_send![screens, count];
                let mut max_fps: i64 = 60;
                for i in 0..count {
                    let screen: ObjcId = msg_send![screens, objectAtIndex: i];
                    let fps: i64 = msg_send![screen, maximumFramesPerSecond];
                    max_fps = max_fps.max(fps);
                }
                1.002 / max_fps.max(1) as f64
            };
            with_macos_app(|app| app.start_timer(0, interval, true));
            self.os.timer0_armed = true;
            self.os.timer0_idle_since = None;
        }
    }

    fn ensure_timer0_stopped(&mut self) {
        if self.os.timer0_armed {
            with_macos_app(|app| {
                app.pause_display_link();
                app.stop_timer(0);
                app.start_timer(0, 0.2, true);
            });
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
                    // MAKEPAD_TIMER_TRACE=1: catch paint-clock stalls in the
                    // act — was the gap a LATE FIRE (runloop starved / OS
                    // deferred the NSTimer) or a SLOW CALLBACK (our work)?
                    let trace_t0 = if std::env::var_os("MAKEPAD_TIMER_TRACE").is_some() {
                        thread_local! {
                            static LAST_FIRE: std::cell::Cell<Option<std::time::Instant>> =
                                const { std::cell::Cell::new(None) };
                        }
                        let now = std::time::Instant::now();
                        LAST_FIRE.with(|last| {
                            if let Some(prev) = last.replace(Some(now)) {
                                let gap_ms = prev.elapsed().as_secs_f64() * 1000.0;
                                if gap_ms > 20.0 {
                                    eprintln!("[timer-trace] fire-to-fire gap {:.1}ms", gap_ms);
                                }
                            }
                        });
                        Some(now)
                    } else {
                        None
                    };
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
                        self.handle_termination_signal();
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
                    // A `--remote` request in flight (a queued command, a grab
                    // waiting on the GPU, a `wait=1` caller) keeps the paint
                    // clock at full rate so the answer lands in one frame
                    // instead of one idle poll. Idle cost when nothing is
                    // pending: none.
                    if crate::remote::needs_ticks() {
                        needs_timer = true;
                    }
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
                    let step_t = trace_t0.map(|_| std::time::Instant::now());
                    self.run_live_edit_if_needed("macos");
                    let live_edit_ms = step_t.map(|t| t.elapsed().as_secs_f64() * 1000.0);
                    let step_t = trace_t0.map(|_| std::time::Instant::now());
                    self.handle_networking_events();
                    let net_ms = step_t.map(|t| t.elapsed().as_secs_f64() * 1000.0);
                    let step_t = trace_t0.map(|_| std::time::Instant::now());
                    self.handle_gamepad_events();
                    let pad_ms = step_t.map(|t| t.elapsed().as_secs_f64() * 1000.0);
                    let paint_t = trace_t0.map(|_| std::time::Instant::now());
                    // Propagate Exit from the inner Paint dispatch. The
                    // signal handling above (Ctrl+C / SIGTERM) calls
                    // `request_quit`, which queues a `CxOsOp::Quit`; that op
                    // is drained by `handle_platform_ops` at the top of this
                    // recursive call and surfaces as `EventFlow::Exit`
                    // (after `Event::Shutdown` is dispatched). If we ignore
                    // the return value here and fall through to
                    // `EventFlow::Wait`, `do_callback` overwrites the just-
                    // set Exit and the main loop blocks indefinitely on the
                    // next NSEvent — the symptom being a Ctrl+C that runs
                    // the user's `QuitRequested` / `Shutdown` handlers but
                    // never actually exits.
                    if let EventFlow::Exit = self.cocoa_event_callback(MacosEvent::Paint, metal_cx, metal_windows) {
                        return EventFlow::Exit;
                    }
                    let paint_ms = paint_t.map(|t| t.elapsed().as_secs_f64() * 1000.0);

                    // Run garbage collection if needed - safe moment after paint, before waiting
                    let gc_t0 = std::time::Instant::now();
                    let mut did_gc = false;
                    self.with_vm(|vm| {
                        if vm.heap().needs_gc() {
                            vm.gc();
                            did_gc = true;
                        }
                    });
                    if did_gc {
                        self.perf_monitor.add(
                            crate::perf_monitor::PERF_CHANNEL_GC,
                            gc_t0.elapsed().as_micros() as u64,
                        );
                    }

                    if let Some(t0) = trace_t0 {
                        let took_ms = t0.elapsed().as_secs_f64() * 1000.0;
                        if took_ms > 10.0 {
                            eprintln!(
                                "[timer-trace] slow callback {:.1}ms (live_edit {:.1} net {:.1} pad {:.1} paint {:.1} gc {:.1})",
                                took_ms,
                                live_edit_ms.unwrap_or(0.0),
                                net_ms.unwrap_or(0.0),
                                pad_ms.unwrap_or(0.0),
                                paint_ms.unwrap_or(0.0),
                                if did_gc { gc_t0.elapsed().as_secs_f64() * 1000.0 } else { 0.0 },
                            );
                        }
                    }

                    // block till the next timer
                    return EventFlow::Wait;
                }
            }
            _ => (),
        }
        //self.process_desktop_pre_event(&mut event);
        match event {
            MacosEvent::AppQuitRequested => {
                self.request_quit(QuitReason::App);
                if let EventFlow::Exit = self.handle_platform_ops(metal_windows, metal_cx) {
                    self.call_event_handler(&Event::Shutdown);
                    return EventFlow::Exit;
                }
            }
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
                    {
                        let cx_window = &mut self.windows[re.window_id];
                        cx_window.os_dpi_factor = Some(re.new_geom.dpi_factor);
                        re.new_geom = cx_window.native_window_geom_to_layout(re.new_geom);
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
                // `CxOsOp::CloseWindow` clears `is_created` *before* asking Cocoa
                // to close, so a window still marked created at this point was
                // dismissed by the human (close button / Cmd-W) rather than by
                // the app. Say which, on stdout, so an agent watching the log
                // does not read a deliberate dismissal as a crash.
                let user_closed = crate::remote::take_window_close_requested(window_id.id())
                    || self.windows[window_id].is_created;
                let title = self.windows[window_id].create_title.clone();
                self.call_event_handler(&Event::WindowClosed(wc));

                self.windows[window_id].is_created = false;
                if user_closed {
                    crate::remote::note_user_closed_window(window_id.id(), &title);
                }
                if let Some(index) = metal_windows.iter().position(|w| w.window_id == window_id) {
                    let metal_window = metal_windows.remove(index);
                    with_macos_app(|app| app.retire_cocoa_window(metal_window.cocoa_window));
                    if metal_windows.len() == 0 {
                        if user_closed {
                            crate::remote::note_user_closed_last_window();
                        }
                        self.call_event_handler(&Event::Shutdown);
                        return EventFlow::Exit;
                    }
                }
            }
            MacosEvent::LinkFire {
                window,
                time,
                primary,
                drawable,
                target_presentation_time,
            } => {
                self.os.link_scope = Some(window as usize);
                self.os.link_flip_time = Some(time);
                self.os.link_drawable = drawable;
                if drawable.is_some() {
                    for mw in metal_windows.iter_mut() {
                        if mw.cocoa_window.window as usize == window as usize {
                            mw.link_is_metal = true;
                        }
                    }
                }
                self.os.link_target_presentation_time = target_presentation_time;
                let flow = if primary {
                    // The primary link drives the WHOLE beat — identical to
                    // the timer-0 path (signals, actions, next-frames, then
                    // paint), just clocked by the flip.
                    self.cocoa_event_callback(
                        MacosEvent::Timer(crate::event::TimerEvent { time: Some(time), timer_id: 0 }),
                        metal_cx,
                        metal_windows,
                    )
                } else {
                    // A secondary window's flip: paint that window only.
                    self.cocoa_event_callback(MacosEvent::Paint, metal_cx, metal_windows)
                };
                self.os.link_scope = None;
                self.os.link_flip_time = None;
                self.os.link_drawable = None;
                self.os.link_target_presentation_time = 0.0;
                if let EventFlow::Exit = flow {
                    return EventFlow::Exit;
                }
            }
            MacosEvent::Paint => {
                // Poll video players for new frames and preparation status
                let has_video_players = !self.os.video_players.is_empty();
                if has_video_players {
                    let mut video_events = Vec::new();
                    for (_video_id, player) in self.os.video_players.iter_mut() {
                        match player.check_prepared() {
                            Some(Ok(PlaybackPrepared {
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
                                        enabled: player.yuv_shader_enabled(),
                                        matrix: player.yuv_matrix(),
                                        biplanar: player.yuv_biplanar() > 0.5,
                                        full_range: player.yuv_full_range(),
                                        rotation_steps: 0.0,
                                    external: false,
                                    array: false,
                                    },
                                rgba_gl_2d: false,
                                },
                            ));
                        }
                    }
                    for event in video_events {
                        self.call_event_handler(&event);
                    }
                }

                let has_next_frames = self.new_next_frames.len() != 0;
                // ONE `now` per beat for everything a redraw consumes: on a
                // display-link beat it is the flip's TARGET timestamp
                // (`LinkFire.time`), which until now only reached the pass
                // uniforms while NextFrame and Draw were stamped wall-now —
                // so a transport stepping on NextFrame and a shader reading
                // `draw_pass.time` disagreed by the callback's latency, and
                // NextFrame deltas jittered with the run loop instead of
                // ticking at the frame period. Unscoped beats (NSTimer,
                // hidden windows) keep wall-now. Windows already does this
                // (`paint_tick(flip_time)`), transport design-v2 §3 / §8 step 0.
                let time_now = self
                    .os
                    .link_flip_time
                    .unwrap_or_else(|| with_macos_app(|app| app.time_now()));
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
                if !self.windows.is_valid(e.window_id)
                    || !self.windows[e.window_id].is_created
                {
                    return EventFlow::Wait;
                }
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.fingers.process_tap_count(e.abs, e.time);
                self.fingers.mouse_down(e.button, e.window_id);
                self.call_event_handler(&Event::MouseDown(e.into()));
            }
            MacosEvent::MouseMove(mut e) => {
                if !self.windows.is_valid(e.window_id)
                    || !self.windows[e.window_id].is_created
                {
                    return EventFlow::Wait;
                }
                self.dpi_override_scale(&mut e.abs, e.window_id);
                let abs = e.abs;
                let modifiers = e.modifiers;
                self.call_event_handler(&Event::MouseMove(e.into()));
                // AppKit requires beginDraggingSession to receive the live
                // NSEvent that initiated the pointer drag. The ordinary
                // platform-op drain happens at the start of the *next*
                // callback, when currentEvent may already be a timer, paint,
                // or mouse-up. Pull out only this explicit cross-app op now;
                // every other platform operation keeps its established
                // deferred semantics.
                self.handle_pending_external_drag(metal_windows);
                if let Some(items) = self.os.internal_drag_items.as_ref() {
                    self.call_event_handler(&Event::Drag(DragEvent {
                        modifiers,
                        handled: Arc::new(Mutex::new(false)),
                        abs,
                        items: items.clone(),
                        response: Arc::new(Mutex::new(DragResponse::None)),
                    }));
                    self.drag_drop.cycle_drag();
                }
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            MacosEvent::MouseUp(mut e) => {
                if !self.windows.is_valid(e.window_id)
                    || !self.windows[e.window_id].is_created
                {
                    return EventFlow::Wait;
                }
                self.dpi_override_scale(&mut e.abs, e.window_id);
                let button = e.button;
                let abs = e.abs;
                let modifiers = e.modifiers;
                self.call_event_handler(&Event::MouseUp(e.into()));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                if button == MouseButton::PRIMARY {
                    if let Some(items) = self.os.internal_drag_items.take() {
                        self.call_event_handler(&Event::Drop(DropEvent {
                            modifiers,
                            handled: Arc::new(Mutex::new(false)),
                            abs,
                            items,
                        }));
                        self.drag_drop.cycle_drag();
                        self.call_event_handler(&Event::DragEnd);
                        self.drag_drop.cycle_drag();
                    }
                }
            }
            MacosEvent::Scroll(mut e) => {
                if !self.windows.is_valid(e.window_id)
                    || !self.windows[e.window_id].is_created
                {
                    return EventFlow::Wait;
                }
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::Scroll(e.into()));
            }
            MacosEvent::WindowDragQuery(mut e) => {
                if !self.windows.is_valid(e.window_id)
                    || !self.windows[e.window_id].is_created
                {
                    return EventFlow::Wait;
                }
                self.dpi_override_scale(&mut e.abs, e.window_id);
                self.call_event_handler(&Event::WindowDragQuery(e))
            }
            MacosEvent::WindowCloseRequested(e) => {
                // Only the native close button / Cmd-W reach `windowShouldClose:`;
                // an app closing its own window does not. So an accepted request
                // here means the human dismissed the window — remember it, and
                // report it when the close actually lands.
                let window_id = e.window_id;
                let accept_close = e.accept_close.clone();
                self.call_event_handler(&Event::WindowCloseRequested(e));
                if accept_close.get() {
                    crate::remote::note_window_close_requested(window_id.id());
                }
            }
            MacosEvent::TextInput(e) => self.call_event_handler(&Event::TextInput(e)),
            MacosEvent::Drag(window_id, mut e) => {
                // External drags arrive in native-logical coordinates; remap into
                // layout space when a dpi_override is set on the window.
                self.dpi_override_scale(&mut e.abs, window_id);
                self.call_event_handler(&Event::Drag(e));
                self.drag_drop.cycle_drag();
            }
            MacosEvent::Drop(window_id, mut e) => {
                self.dpi_override_scale(&mut e.abs, window_id);
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

    fn start_external_drag_now(
        &mut self,
        metal_windows: &mut [MetalWindow],
        window_id: WindowId,
        items: Vec<DragItem>,
    ) {
        let started = metal_windows
            .iter_mut()
            .find(|window| window.window_id == window_id)
            .is_some_and(|window| window.cocoa_window.start_external_dragging(items));
        if !started {
            crate::error!("could not start external file drag");
            // A native session normally emits DragEnd from its AppKit source
            // callback. A rejected start needs the same completion signal so
            // application gesture guards can recover immediately.
            self.call_event_handler(&Event::DragEnd);
        }
    }

    fn handle_pending_external_drag(&mut self, metal_windows: &mut [MetalWindow]) {
        let Some(index) = self
            .platform_ops
            .iter()
            .rposition(|op| matches!(op, CxOsOp::StartExternalDragging { .. }))
        else {
            return;
        };
        let Some(CxOsOp::StartExternalDragging { window_id, items }) =
            self.platform_ops.remove(index)
        else {
            unreachable!();
        };
        self.start_external_drag_now(metal_windows, window_id, items);
    }

    fn handle_platform_ops(
        &mut self,
        metal_windows: &mut Vec<MetalWindow>,
        metal_cx: &MetalCx,
    ) -> EventFlow {
        while let Some(op) = self.platform_ops.pop_front() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let mut metal_window = MetalWindow::new(
                        window_id,
                        &metal_cx,
                        window.create_inner_size.unwrap_or(dvec2(800., 600.)),
                        window.create_position,
                        &window.create_title,
                        window.is_fullscreen,
                        window.macos,
                    );
                    let visuals = window.window_visuals();
                    metal_window.cocoa_window.set_window_visuals(visuals);
                    let layer_opaque = if visuals.transparent { NO } else { YES };
                    let layer_alpha = if visuals.transparent { 0.0 } else { 1.0 };
                    let () = unsafe { msg_send![metal_window.ca_layer, setOpaque: layer_opaque] };
                    let () = unsafe {
                        msg_send![metal_window.ca_layer, setBackgroundColor: CGColorCreateGenericRGB(0.0, 0.0, 0.0, layer_alpha)]
                    };
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
                    let mut metal_window = MetalWindow::new_popup(
                        window_id,
                        &metal_cx,
                        size,
                        position,
                        parent_ns_window,
                    );
                    metal_window
                        .cocoa_window
                        .set_window_visuals(window.window_visuals());
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
                CxOsOp::SetTopmost(window_id, is_topmost) => {
                    if metal_windows.is_empty() {
                        if self.defer_platform_op(CxOsOp::SetTopmost(window_id, is_topmost)) {
                            continue;
                        }
                        break;
                    }
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.set_topmost(is_topmost);
                        self.windows[window_id].window_geom =
                            metal_window.cocoa_window.get_window_geom();
                    }
                }
                CxOsOp::SetWindowVisuals(window_id, visuals) => {
                    if let Some(metal_window) =
                        metal_windows.iter_mut().find(|w| w.window_id == window_id)
                    {
                        metal_window.cocoa_window.set_window_visuals(visuals);
                        let layer_opaque = if visuals.transparent { NO } else { YES };
                        let layer_alpha = if visuals.transparent { 0.0 } else { 1.0 };
                        let () =
                            unsafe { msg_send![metal_window.ca_layer, setOpaque: layer_opaque] };
                        let () = unsafe {
                            msg_send![metal_window.ca_layer, setBackgroundColor: CGColorCreateGenericRGB(0.0, 0.0, 0.0, layer_alpha)]
                        };
                    }
                }
                CxOsOp::ShowTextIME(area, cursor_rect, _config) => {
                    // Convert both corners of the caret line rect (area-relative,
                    // logical px) into window content-view points so the height is
                    // scaled correctly along with the position.
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
                    metal_windows.iter_mut().for_each(|w| {
                        w.cocoa_window.set_ime_active(true);
                        w.cocoa_window.set_ime_rect(ime_rect);
                    });
                }
                CxOsOp::HideTextIME => {
                    metal_windows.iter_mut().for_each(|w| {
                        w.cocoa_window.set_ime_active(false);
                        w.cocoa_window.set_ime_rect(Rect::default());
                    });
                }
                CxOsOp::SetCursor(cursor) => {
                    with_macos_app(|app| app.set_mouse_cursor(cursor));
                }
                CxOsOp::LockMousePointer(lock) => {
                    with_macos_app(|app| {
                        app.mouse_pointer_lock = lock;
                        app.apply_pointer_lock_effects(lock);
                    });
                }
                CxOsOp::RepinMousePointer => {
                    with_macos_app(|app| app.repin_pointer());
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
                    // Use internal drag-and-drop (synthesizing Drag/Drop events
                    // from mouse move/up) instead of OS-level drag, which delays
                    // DragEnd by ~1 second due to the macOS fly-back animation.
                    self.os.internal_drag_items = Some(Arc::new(items));
                }
                CxOsOp::StartExternalDragging { window_id, items } => {
                    self.start_external_drag_now(metal_windows, window_id, items);
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
                CxOsOp::SpawnSystemBrowser { browser_id, url } => {
                    self.os
                        .system_browsers
                        .entry(browser_id)
                        .or_insert_with(|| MacosSystemBrowser::new(&url))
                        .set_url(&url, false);
                }
                CxOsOp::UpdateSystemBrowser {
                    browser_id,
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

                    let mut unclipped_rect = area.rect(self);
                    let mut clipped_rect = area.clipped_rect(self);
                    let win_h = self.windows[window_id].window_geom.inner_size.y;
                    unclipped_rect.pos.y = win_h - unclipped_rect.pos.y - unclipped_rect.size.y;
                    clipped_rect.pos.y = win_h - clipped_rect.pos.y - clipped_rect.size.y;
                    let parent_view = metal_window.cocoa_window.view;

                    if let Some(browser) = self.os.system_browsers.get_mut(&browser_id) {
                        browser.update(
                            window_id,
                            parent_view,
                            unclipped_rect,
                            clipped_rect,
                            visible,
                        );
                    }
                }
                CxOsOp::DetachSystemBrowser { browser_id } => {
                    if let Some(browser) = self.os.system_browsers.get_mut(&browser_id) {
                        browser.detach();
                    }
                }
                CxOsOp::SetSystemBrowserUrl {
                    browser_id,
                    url,
                    replace,
                } => {
                    if let Some(browser) = self.os.system_browsers.get_mut(&browser_id) {
                        browser.set_url(&url, replace);
                    }
                }
                CxOsOp::SystemBrowserHistoryGo { browser_id, delta } => {
                    if let Some(browser) = self.os.system_browsers.get_mut(&browser_id) {
                        browser.history_go(delta);
                    }
                }
                CxOsOp::CloseSystemBrowser { browser_id } => {
                    if let Some(mut browser) = self.os.system_browsers.remove(&browser_id) {
                        browser.cleanup();
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
                CxOsOp::StartLocationUpdates => {
                    self.apple_start_location_updates();
                }
                CxOsOp::StopLocationUpdates => {
                    self.apple_stop_location_updates();
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
                        if let Some(Ok(PlaybackPrepared {
                            width,
                            height,
                            duration_ms: duration,
                            is_seekable,
                            video_tracks,
                            audio_tracks,
                        })) = preview.check_prepared()
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
                    self.call_event_handler(&Event::VideoYuvTexturesReady(VideoYuvTexturesReady::planes(video_id, tex_y, tex_u, tex_v)));
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
                // Track selection is currently implemented on Linux GStreamer only.
                CxOsOp::SelectVideoTrack(_, _) | CxOsOp::SelectAudioTrack(_, _) => {}
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
            Permission::HeadsetCamera => crate::permission::PermissionStatus::DeniedPermanent,
            Permission::SceneAccess => crate::permission::PermissionStatus::DeniedPermanent,
            Permission::Location => Self::apple_location_permission_status(),
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
            Permission::HeadsetCamera => crate::permission::PermissionStatus::DeniedPermanent,
            Permission::SceneAccess => crate::permission::PermissionStatus::DeniedPermanent,
            Permission::Location => Self::apple_location_permission_status(),
        };
        match status {
            crate::permission::PermissionStatus::NotDetermined => match permission {
                Permission::AudioInput => {
                    self.macos_request_audio_permission(permission, request_id)
                }
                Permission::Camera => self.macos_request_camera_permission(permission, request_id),
                Permission::HeadsetCamera => {}
                Permission::SceneAccess => {}
                Permission::Location => self.apple_request_location_permission(request_id),
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
    /// While a LinkFire beat runs: paint ONLY passes rooted in this cocoa
    /// window (as usize), and stamp them with `link_flip_time` — the flip's
    /// target timestamp in app time. None = the NSTimer/idle beat: paint
    /// everything, stamp wall-now.
    pub(crate) link_scope: Option<usize>,
    pub(crate) link_flip_time: Option<f64>,
    /// Some(drawable) is supplied only by CAMetalDisplayLinkUpdate. Some(nil)
    /// deliberately means "this Metal update has no drawable"; None selects
    /// the existing CAMetalLayer.nextDrawable fallback.
    pub(crate) link_drawable: Option<ObjcId>,
    /// Core Animation / Metal media-time domain; used both for targeted
    /// presentation and target-vs-actual frame tracing.
    pub(crate) link_target_presentation_time: f64,
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
    pub(crate) system_browsers: HashMap<LiveId, MacosSystemBrowser>,
    pub(crate) internal_drag_items: Option<Arc<Vec<DragItem>>>,
}
