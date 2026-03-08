use makepad_widgets::*;
use makepad_widgets::makepad_platform::video::{VideoInputsEvent, VideoPixelFormat};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                body +: {
                    main_view := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16

                        Label{
                            text: "Camera Home"
                            draw_text.text_style.font_size: 18
                        }

                        mode_row := View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8

                            no_camera_btn := Button{ width: Fill text: "no-camera" }
                            texture_btn := Button{ width: Fill text: "texture" }
                            native_btn := Button{ width: Fill text: "nativepreview" }
                        }

                        mode_label := Label{
                            text: "Mode: no-camera"
                            draw_text.text_style.font_size: 10
                        }

                        rotation_label := Label{
                            text: "YUV rotation: 0 (0°)"
                            draw_text.text_style.font_size: 10
                            draw_text.color: #888
                        }

                        status_label := Label{
                            text: "Camera is off"
                            draw_text.text_style.font_size: 10
                            draw_text.color: #888
                        }

                        camera_placeholder := View{
                            width: Fill
                            height: Fill
                            align: Center
                            no_camera_hint := Label{
                                text: "Camera preview is off"
                                draw_text.color: #888
                            }
                        }

                        camera_native_host := View{
                            width: Fill
                            height: Fill
                            visible: false
                            camera_video_native := Video{
                                width: Fill
                                height: Fill
                                autoplay: false
                                show_controls: false
                            }
                        }

                        camera_texture_host := View{
                            width: Fill
                            height: Fill
                            visible: false
                            camera_video_texture := Video{
                                width: Fill
                                height: Fill
                                autoplay: false
                                show_controls: false
                            }
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn set_preview_mode_visible(&self, cx: &mut Cx, mode: Option<CameraHomeMode>) {
        self.ui.view(cx, ids!(camera_native_host)).set_visible(
            cx,
            mode == Some(CameraHomeMode::NativePreview),
        );
        self.ui.view(cx, ids!(camera_texture_host)).set_visible(
            cx,
            mode == Some(CameraHomeMode::Texture),
        );
        self.ui
            .view(cx, ids!(camera_placeholder))
            .set_visible(cx, mode.is_none());
    }

    fn update_mode_label(&self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(mode_label))
            .set_text(cx, &format!("Mode: {}", self.desired_mode.label()));
    }

    fn update_rotation_label(&self, cx: &mut Cx) {
        let steps = self.last_yuv_rotation_steps.round().clamp(0.0, 3.0) as i32;
        let degrees = steps * 90;
        self.ui
            .label(cx, ids!(rotation_label))
            .set_text(cx, &format!("YUV rotation: {} ({}°)", steps, degrees));
    }

    fn pick_camera_choice(ev: &VideoInputsEvent) -> Option<CameraChoice> {
        let desc = ev.descs.first()?;

        fn pixel_rank(pixel_format: VideoPixelFormat) -> usize {
            match pixel_format {
                VideoPixelFormat::NV12 => 3,
                VideoPixelFormat::YUY2 => 2,
                VideoPixelFormat::YUV420 => 1,
                _ => 0,
            }
        }

        fn is_supported(pixel_format: VideoPixelFormat) -> bool {
            matches!(pixel_format, VideoPixelFormat::NV12 | VideoPixelFormat::YUY2 | VideoPixelFormat::YUV420)
        }

        fn better(a: &makepad_widgets::makepad_platform::video::VideoFormat, b: &makepad_widgets::makepad_platform::video::VideoFormat) -> bool {
            let a_rank = pixel_rank(a.pixel_format);
            let b_rank = pixel_rank(b.pixel_format);
            if a_rank != b_rank {
                return a_rank > b_rank;
            }
            let a_pixels = a.width * a.height;
            let b_pixels = b.width * b.height;
            if a_pixels != b_pixels {
                return a_pixels > b_pixels;
            }
            let a_fps = a.frame_rate.unwrap_or(0.0);
            let b_fps = b.frame_rate.unwrap_or(0.0);
            a_fps > b_fps
        }

        let mut best: Option<makepad_widgets::makepad_platform::video::VideoFormat> = None;

        // Pass 1: NV12 at <= 1080p (preferred for iOS texture path stability)
        for fmt in &desc.formats {
            if fmt.pixel_format != VideoPixelFormat::NV12 {
                continue;
            }
            if fmt.width > 1920 || fmt.height > 1080 {
                continue;
            }
            if best.as_ref().map_or(true, |b| better(fmt, b)) {
                best = Some(*fmt);
            }
        }

        // Pass 2: any NV12
        if best.is_none() {
            for fmt in &desc.formats {
                if fmt.pixel_format != VideoPixelFormat::NV12 {
                    continue;
                }
                if best.as_ref().map_or(true, |b| better(fmt, b)) {
                    best = Some(*fmt);
                }
            }
        }

        // Pass 3: other supported YUV formats
        if best.is_none() {
            for fmt in &desc.formats {
                if !is_supported(fmt.pixel_format) {
                    continue;
                }
                if best.as_ref().map_or(true, |b| better(fmt, b)) {
                    best = Some(*fmt);
                }
            }
        }

        let format = best?;
        Some(CameraChoice {
            input_id: desc.input_id,
            format_id: format.format_id,
            name: desc.name.clone(),
            width: format.width,
            height: format.height,
            pixel_format: format.pixel_format,
            frame_rate: format.frame_rate,
        })
    }

    fn choose_mode(&mut self, cx: &mut Cx, mode: CameraHomeMode) {
        if self.desired_mode != mode {
            self.desired_mode = mode;
            self.pending_mode_switch = true;
        }
        self.update_mode_label(cx);
        self.drive_mode(cx);
    }

    fn drive_mode(&mut self, cx: &mut Cx) {
        if !self.pending_mode_switch {
            return;
        }

        let native_video = self.ui.video(cx, &[live_id!(camera_video_native)]);
        let texture_video = self.ui.video(cx, &[live_id!(camera_video_texture)]);

        match self.desired_mode {
            CameraHomeMode::NoCamera => {
                self.set_preview_mode_visible(cx, None);

                let native_idle = native_video.is_unprepared();
                let texture_idle = texture_video.is_unprepared();
                if native_idle && texture_idle {
                    self.pending_mode_switch = false;
                    self.set_status(cx, "Camera is off");
                    return;
                }

                if !native_idle && !native_video.is_cleaning_up() {
                    native_video.stop_and_cleanup_resources(cx);
                }
                if !texture_idle && !texture_video.is_cleaning_up() {
                    texture_video.stop_and_cleanup_resources(cx);
                }

                self.set_status(cx, "Stopping camera...");
            }
            CameraHomeMode::Texture | CameraHomeMode::NativePreview => {
                self.set_preview_mode_visible(cx, None);

                if !matches!(
                    self.camera_permission,
                    Some(makepad_widgets::makepad_platform::permission::PermissionStatus::Granted)
                ) {
                    self.set_status(cx, "Waiting for camera permission...");
                    return;
                }

                let Some(choice) = self.camera_choice.clone() else {
                    self.set_status(cx, "Waiting for camera device...");
                    return;
                };

                let (target_video, other_video) = match self.desired_mode {
                    CameraHomeMode::Texture => (texture_video, native_video),
                    CameraHomeMode::NativePreview => (native_video, texture_video),
                    CameraHomeMode::NoCamera => return,
                };

                if !other_video.is_unprepared() {
                    if !other_video.is_cleaning_up() {
                        self.set_status(cx, "Cleaning up previous mode...");
                        other_video.stop_and_cleanup_resources(cx);
                    }
                    return;
                }

                if !target_video.is_unprepared() {
                    if !target_video.is_cleaning_up() {
                        self.pending_mode_switch = false;
                        if self.desired_mode == CameraHomeMode::NativePreview {
                            self.set_preview_mode_visible(cx, Some(CameraHomeMode::NativePreview));
                        }
                    }
                    return;
                }

                target_video.set_camera_preview_mode(cx, self.desired_mode.to_preview_mode());
                target_video.set_source_camera(cx, choice.input_id, choice.format_id);
                target_video.begin_playback(cx);
                if self.desired_mode == CameraHomeMode::NativePreview {
                    self.set_preview_mode_visible(cx, Some(CameraHomeMode::NativePreview));
                }
                self.pending_mode_switch = false;
                self.set_status(
                    cx,
                    &format!(
                        "Starting {} mode on {} ({}x{} {:?} fps={:?})",
                        self.desired_mode.label(),
                        choice.name,
                        choice.width,
                        choice.height,
                        choice.pixel_format,
                        choice.frame_rate
                    ),
                );
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    desired_mode: CameraHomeMode,
    #[rust]
    pending_mode_switch: bool,
    #[rust]
    camera_permission: Option<makepad_widgets::makepad_platform::permission::PermissionStatus>,
    #[rust]
    camera_choice: Option<CameraChoice>,
    #[rust]
    last_yuv_rotation_steps: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum CameraHomeMode {
    #[default]
    NoCamera,
    Texture,
    NativePreview,
}

impl CameraHomeMode {
    fn label(self) -> &'static str {
        match self {
            Self::NoCamera => "no-camera",
            Self::Texture => "texture",
            Self::NativePreview => "nativepreview",
        }
    }

    fn to_preview_mode(self) -> VideoCameraPreviewMode {
        match self {
            Self::Texture => VideoCameraPreviewMode::Texture,
            Self::NativePreview => VideoCameraPreviewMode::Native,
            Self::NoCamera => VideoCameraPreviewMode::Texture,
        }
    }
}

#[derive(Clone)]
struct CameraChoice {
    input_id: makepad_widgets::makepad_platform::video::VideoInputId,
    format_id: makepad_widgets::makepad_platform::video::VideoFormatId,
    name: String,
    width: usize,
    height: usize,
    pixel_format: makepad_widgets::makepad_platform::video::VideoPixelFormat,
    frame_rate: Option<f64>,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(no_camera_btn)).clicked(actions) {
            self.choose_mode(cx, CameraHomeMode::NoCamera);
        }
        if self.ui.button(cx, ids!(texture_btn)).clicked(actions) {
            self.choose_mode(cx, CameraHomeMode::Texture);
        }
        if self.ui.button(cx, ids!(native_btn)).clicked(actions) {
            self.choose_mode(cx, CameraHomeMode::NativePreview);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::Startup => {
                cx.request_permission(makepad_widgets::makepad_platform::permission::Permission::Camera);
                cx.video_input(0, |_buf| {});
                self.update_mode_label(cx);
                self.update_rotation_label(cx);
                self.set_preview_mode_visible(cx, None);
                self.set_status(cx, "Camera is off");
            }
            Event::VideoInputs(ev) => {
                let Some(_first) = ev.descs.first() else {
                    self.camera_choice = None;
                    self.set_preview_mode_visible(cx, None);
                    if self.desired_mode != CameraHomeMode::NoCamera {
                        self.set_status(cx, "No camera found");
                    }
                    return;
                };

                self.camera_choice = Self::pick_camera_choice(ev);
                if self.camera_choice.is_none() {
                    self.set_status(cx, "No suitable YUV camera format found");
                }

                self.drive_mode(cx);
            }
            Event::PermissionResult(result) => {
                use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};

                if result.permission == Permission::Camera {
                    self.camera_permission = Some(result.status);
                    match result.status {
                        PermissionStatus::Granted => {
                            if self.desired_mode == CameraHomeMode::NoCamera {
                                self.set_status(cx, "Camera is off");
                            } else {
                                self.set_status(cx, "Camera permission granted");
                            }
                            self.drive_mode(cx);
                        }
                        PermissionStatus::DeniedPermanent => {
                            self.set_preview_mode_visible(cx, None);
                            self.set_status(cx, "Camera permission denied");
                        }
                        _ => {
                            self.set_status(
                                cx,
                                &format!("Camera permission: {:?}", result.status),
                            );
                        }
                    }
                }
            }
            Event::VideoPlaybackPrepared(ev) => {
                if self.desired_mode == CameraHomeMode::NativePreview {
                    self.set_preview_mode_visible(cx, Some(CameraHomeMode::NativePreview));
                }
                self.set_status(
                    cx,
                    &format!(
                        "Running {} mode at {}x{}",
                        self.desired_mode.label(),
                        ev.video_width,
                        ev.video_height
                    ),
                );
                self.drive_mode(cx);
            }
            Event::VideoTextureUpdated(ev) => {
                self.last_yuv_rotation_steps = ev.yuv.rotation_steps;
                self.update_rotation_label(cx);
                if self.desired_mode == CameraHomeMode::Texture {
                    self.set_preview_mode_visible(cx, Some(CameraHomeMode::Texture));
                }
            }
            Event::VideoPlaybackResourcesReleased(_) => {
                self.drive_mode(cx);
            }
            Event::VideoDecodingError(ev) => {
                self.set_preview_mode_visible(cx, None);
                self.set_status(cx, &format!("Camera error: {}", ev.error));
            }
            _ => {}
        }
    }
}
