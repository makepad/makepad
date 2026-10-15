use crate::{
    image_cache::ImageCacheImpl,
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_platform::event::video_playback::*,
    makepad_platform::video::{VideoFormatId, VideoInputId},
    makepad_platform::{
        register_video_frame_session, unregister_video_frame_session, VideoFrameSession,
        VideoFrameSessionId,
    },
    widget::*,
};
use std::rc::Rc;
script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.VideoDataSource = #(VideoDataSource::script_api(vm))
    mod.widgets.VideoFit = #(VideoFit::script_api(vm))
    mod.widgets.VideoBase = #(Video::register_widget(vm))

    mod.widgets.Video = set_type_default() do mod.widgets.VideoBase{
        width: 100
        height: 100

        draw_bg +: {
            video_texture: texture_video()
            thumbnail_texture: texture_2d(float)
            tex_y: texture_2d(float)
            tex_u: texture_2d(float)
            tex_v: texture_2d(float)
            // Linux DMA-Buf NV12: planes bound as TEXTURE_EXTERNAL_OES (NVIDIA rejects
            // plane→TEXTURE_2D). Selected when yuv_sample_mode ≈ 1.
            tex_y_oes: texture_video()
            tex_u_oes: texture_video()
            // Linux GLMemory RGBA is TEXTURE_2D (glupload texture-target=2D).
            // Keep at the end so existing texture slot indices stay stable.
            video_texture_2d: texture_2d(float)
            // Windows D3D11VA true zero-copy: NV12 plane SRVs on Texture2DArray.
            // Slots after video_texture_2d so 0..7 stay stable.
            // Selected when yuv_sample_mode ≈ 2.
            tex_y_arr: texture_2d_array(float)
            tex_u_arr: texture_2d_array(float)
            show_thumbnail: uniform(0.0)
            yuv_type: uniform(0.0)
            yuv_enabled: uniform(0.0)
            yuv_biplanar: uniform(0.0)
            // 0 = texture_2d planes, 1 = OES/external, 2 = texture_2d_array (Windows ZC)
            yuv_sample_mode: uniform(0.0)
            video_rgba_2d: uniform(0.0)
            // 0 = limited/video range (TV), 1 = full range (JPEG/PC)
            yuv_full_range: uniform(0.0)
            yuv_rotation_steps: uniform(0.0)
            // Android OES zero-copy: SurfaceTexture.getTransformMatrix() as 4 column
            // vec4s (column-major). Required by the Android contract before sampling
            // samplerExternalOES — handles Y-flip, crop, and buffer alignment. Left as
            // identity on non-Android / non-OES paths (no-op). Prefer keeping these over
            // a hard-coded v=1-v; only drop if UV is baked in an OES->2D blit instead.
            oes_st_c0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
            oes_st_c1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
            oes_st_c2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
            oes_st_c3: uniform(vec4(0.0, 0.0, 0.0, 1.0))

            opacity: instance(1.0)
            image_scale: instance(vec2(1.0, 1.0))
            image_pan: instance(vec2(0.5, 0.5))

            source_size: uniform(vec2(0.0, 0.0))
            fit_cover: uniform(0.0)

            // Apply SurfaceTexture ST matrix, then sample OES (see oes_st_c* comment).
            sample_oes: fn(coord: vec2) -> vec4 {
                let uv = vec4(coord.x, coord.y, 0.0, 1.0)
                let t = self.oes_st_c0 * uv.x + self.oes_st_c1 * uv.y + self.oes_st_c2 * uv.z + self.oes_st_c3 * uv.w
                return self.video_texture.sample_video(t.xy)
            }

            sample_yuv: fn(coord: vec2) -> vec4 {
                let coord_90 = vec2(1.0 - coord.y, coord.x)
                let coord_180 = vec2(1.0 - coord.x, 1.0 - coord.y)
                let coord_270 = vec2(coord.y, 1.0 - coord.x)

                let is_90 = step(0.5, self.yuv_rotation_steps) * step(self.yuv_rotation_steps, 1.5)
                let is_180 = step(1.5, self.yuv_rotation_steps) * step(self.yuv_rotation_steps, 2.5)
                let is_270 = step(2.5, self.yuv_rotation_steps)
                let is_0 = 1.0 - is_90 - is_180 - is_270
                let sample_coord = coord * is_0 + coord_90 * is_90 + coord_180 * is_180 + coord_270 * is_270

                // Never mix-sample sampler2D and samplerExternalOES in one path
                // (GLES UB / NVIDIA garbage).
                let mut y_val = 0.0
                let mut u_val = 0.0
                let mut v_val = 0.0
                if self.yuv_sample_mode > 1.5 {
                    // D3D11VA: sample one array slice via float3(uv, layer).
                    let arr_coord = vec3(sample_coord.x, sample_coord.y, 0.0)
                    y_val = self.tex_y_arr.sample(arr_coord).x
                    let uv_sample = self.tex_u_arr.sample(arr_coord)
                    u_val = uv_sample.x
                    v_val = uv_sample.y
                } else if self.yuv_sample_mode > 0.5 {
                    y_val = self.tex_y_oes.sample_video(sample_coord).x
                    let uv_sample = self.tex_u_oes.sample_video(sample_coord)
                    u_val = uv_sample.x
                    v_val = uv_sample.y
                } else {
                    y_val = self.tex_y.sample(sample_coord).x
                    let uv_sample = self.tex_u.sample(sample_coord)
                    u_val = uv_sample.x
                    // Biplanar NV12: U in tex_u.r, V in tex_u.g
                    // Triplanar I420: U in tex_u.r, V in tex_v.r
                    if self.yuv_biplanar > 0.5 {
                        v_val = uv_sample.y
                    } else {
                        v_val = self.tex_v.sample(sample_coord).x
                    }
                }

                // Limited/video range: Y [16..235] -> [0..1], UV [16..240] -> [-0.5..0.5]
                let y_lim = (y_val * 255.0 - 16.0) / 219.0
                let u_lim = (u_val * 255.0 - 128.0) / 224.0
                let v_lim = (v_val * 255.0 - 128.0) / 224.0
                // Full/JPEG range: Y [0..255] -> [0..1], UV [0..255] -> [-0.5..0.5]
                let y_full = y_val
                let u_full = u_val - 0.5
                let v_full = v_val - 0.5
                let use_full = step(0.5, self.yuv_full_range)
                let y = mix(y_lim, y_full, use_full)
                let u = mix(u_lim, u_full, use_full)
                let v = mix(v_lim, v_full, use_full)

                // BT.709 (yuv_type == 0.0)
                let r709 = y + 1.5748 * v
                let g709 = y - 0.1873 * u - 0.4681 * v
                let b709 = y + 1.8556 * u

                // BT.601 (yuv_type == 1.0)
                let r601 = y + 1.402 * v
                let g601 = y - 0.3441 * u - 0.7141 * v
                let b601 = y + 1.772 * u

                // BT.2020 (yuv_type == 2.0)
                let r2020 = y + 1.4746 * v
                let g2020 = y - 0.1646 * u - 0.5714 * v
                let b2020 = y + 1.8814 * u

                // Select matrix by yuv_type uniform
                let is_601 = step(0.5, self.yuv_type) * step(self.yuv_type, 1.5)
                let is_2020 = step(1.5, self.yuv_type)
                let is_709 = 1.0 - is_601 - is_2020

                let r = is_709 * r709 + is_601 * r601 + is_2020 * r2020
                let g = is_709 * g709 + is_601 * g601 + is_2020 * g2020
                let b = is_709 * b709 + is_601 * b601 + is_2020 * b2020

                return vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0)
            }

            // The picture at `uv` (0..1 over the picture): the thumbnail until
            // playback starts, then the video in whichever form it arrives.
            sample_picture: fn(uv: vec2) -> vec4 {
                if self.show_thumbnail > 0.5 {
                    return self.thumbnail_texture.sample(uv).xyzw
                } else if self.yuv_enabled > 0.5 {
                    return self.sample_yuv(uv)
                } else if self.video_rgba_2d > 0.5 {
                    return self.video_texture_2d.sample(uv)
                } else {
                    return self.sample_oes(uv)
                }
            }

            // source_size is the picture's display size (pixel aspect and
            // rotation applied). Contain (fit_cover 0): the whole picture,
            // centred, with transparent bars on the two free sides. Cover
            // (fit_cover 1): the picture fills the widget, cropped on one
            // axis, image_pan picking which part shows.
            get_color: fn() -> vec4 {
                let src = self.source_size
                let dst = self.rect_size
                if src.x <= 0.0 || src.y <= 0.0 || dst.x <= 0.0 || dst.y <= 0.0 {
                    return self.sample_picture(self.pos)
                }
                let src_aspect = src.x / src.y
                let dst_aspect = dst.x / dst.y
                if self.fit_cover > 0.5 {
                    let mut scale = self.image_scale
                    if src_aspect > dst_aspect {
                        scale = vec2(dst_aspect / src_aspect, 1.0)
                    } else if src_aspect < dst_aspect {
                        scale = vec2(1.0, src_aspect / dst_aspect)
                    }
                    let pan_range = vec2(max(0.0, 1.0 - scale.x), max(0.0, 1.0 - scale.y))
                    return self.sample_picture(self.pos * scale + pan_range * self.image_pan)
                }
                let mut size = vec2(1.0, 1.0)
                if src_aspect > dst_aspect {
                    size = vec2(1.0, dst_aspect / src_aspect)
                } else {
                    size = vec2(src_aspect / dst_aspect, 1.0)
                }
                let uv = (self.pos - (vec2(1.0, 1.0) - size) * 0.5) / size
                if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                return self.sample_picture(uv)
            }

            pixel: fn() {
                let color = self.get_color()
                return Pal.premul(vec4(color.xyz, color.w * self.opacity))
            }
        }

        draw_error_bg +: {
            pixel: fn() {
                return Pal.premul(vec4(0.0, 0.0, 0.0, 0.75))
            }
        }

        draw_error_text +: {
            color: #fff
            text_style: theme.font_regular{
                font_size: 10.0
            }
        }

    }
}

/// DSL Usage
///
/// `source` - determines the source for the video playback, can be either:
///  - `Network { url: "https://www.someurl.com/video.mkv" }`. On Android it supports: HLS, DASH, RTMP, RTSP, and progressive HTTP downloads
///  - `Filesystem { path: "/storage/.../DCIM/Camera/video.mp4" }`. On Android it requires read permissions that must be granted at runtime.
///  - `Dependency { path: dep("crate://self/resources/video.mp4") }`. For in-memory videos loaded through LiveDependencies
///
/// `thumbnail_source` - determines the source for the thumbnail image, currently only supports LiveDependencies.
///
/// `is_looping` - determines if the video should be played in a loop. defaults to false.
///
/// `hold_to_pause` - determines if the video should be paused when the user hold the pause button. defaults to false.
///
/// `autoplay` - determines if the video should start playback when the widget is created. defaults to false.
///
/// `show_idle_thumbnail` - when true and a `thumbnail_source` is provided, shows the thumbnail while the video is idle. defaults to false.

/// Transport controls are not this widget's: `VideoPlayer` puts the glass
/// `PlayerControls` over a `Video`.

/// Not yet supported:
/// Widget API
///  - Seek to arbitrary timestamp
///  - Hotswap video source, `set_source(VideoDataSource)` only works if video is in Unprepared state.

#[derive(Script, Widget)]
pub struct Video {
    #[uid]
    uid: WidgetUid,
    #[source]
    source_ref: ScriptObjectRef,

    // Drawing
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[walk]
    walk: Walk,
    #[live]
    layout: Layout,
    #[live]
    scale: f64,

    #[live]
    draw_error_bg: DrawColor,
    #[live]
    draw_error_text: DrawText,

    // Textures
    #[live]
    source: VideoDataSource,
    #[rust]
    in_memory_source: Option<Rc<Vec<u8>>>,
    #[rust]
    session_source_id: Option<VideoFrameSessionId>,
    #[rust]
    has_session_source: bool,
    #[rust]
    video_texture: Option<Texture>,
    #[rust]
    video_texture_handle: Option<u32>,
    /// Requires [`show_idle_thumbnail`] to be `true`.
    #[live]
    thumbnail_source: Option<ScriptHandleRef>,
    #[rust]
    thumbnail_texture: Option<Texture>,
    // YUV plane textures (software / SystemI420 path)
    #[rust]
    tex_y: Option<Texture>,
    #[rust]
    tex_u: Option<Texture>,
    #[rust]
    tex_v: Option<Texture>,
    // Linux DMA-Buf NV12 EXTERNAL_OES plane textures (true zero-copy)
    #[rust]
    tex_y_oes: Option<Texture>,
    #[rust]
    tex_u_oes: Option<Texture>,
    /// Dummy EXTERNAL_OES texture kept bound to `video_texture` while presenting
    /// GLMemory RGBA via `video_texture_2d` (shader still declares samplerExternalOES).
    #[rust]
    oes_dummy: Option<Texture>,
    /// Linux GLMemory TEXTURE_2D path — rebind slot/uniforms every draw.
    #[rust]
    rgba_gl_2d: bool,
    #[rust]
    source_mode: VideoSourceMode,
    #[live(VideoCameraPreviewMode::Auto)]
    camera_preview_mode: VideoCameraPreviewMode,
    #[rust]
    native_preview_attached: bool,

    // Playback
    #[live(false)]
    is_looping: bool,
    #[live(false)]
    hold_to_pause: bool,
    /// This press paused playback for as long as it is held
    /// (`hold_to_pause`): its end, released or taken away, resumes it.
    #[rust]
    held_paused: bool,
    #[live(false)]
    autoplay: bool,
    /// How the picture sits in the widget: whole and centred (Contain), or
    /// filling it, cropped on one axis (Cover, `image_pan` picks the part).
    #[live]
    fit: VideoFit,
    #[live(false)]
    mute: bool,
    #[rust]
    playback_state: PlaybackState,
    #[rust]
    should_prepare_playback: bool,
    #[rust]
    audio_state: AudioState,
    /// Whether to show the provided thumbnail when the video has not yet started playing.
    #[live(false)]
    show_idle_thumbnail: bool,

    // Actions
    #[rust(false)]
    should_dispatch_texture_updates: bool,

    // Original video metadata
    #[rust]
    video_width: usize,
    #[rust]
    video_height: usize,
    /// The planes arrive a quarter turn off (yuv rotation steps 1 or 3).
    #[rust]
    quarter_turned: bool,
    #[rust]
    total_duration: u128,

    /// Labels from the last prepare / tracks-changed event (Linux GStreamer playbin3).
    #[rust]
    video_tracks: Vec<String>,
    #[rust]
    audio_tracks: Vec<String>,

    // Playback position tracking
    #[rust]
    current_position_ms: u128,

    #[rust]
    id: LiveId,
    #[rust]
    last_error: Option<String>,
}

impl ScriptHook for Video {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.init_video_texture(cx);
        });
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // Skip widget-level side effects for animator-driven applies (hover/seek
        // indicator transitions fire one apply per frame). Otherwise
        // apply_thumbnail_settings would re-decode the PNG/JPG thumbnail and
        // allocate a fresh GPU texture on every animator frame.
        if apply.is_animate() {
            return;
        }
        vm.with_cx_mut(|cx| {
            self.ensure_primary_texture(cx);
            self.apply_thumbnail_settings(cx);
            // Prepare the video when autoplay is enabled (so it starts immediately) or when
            // show_idle_thumbnail is true (so the first frame is available as a thumbnail).
            // On macOS/iOS there's no TextureHandleReady event (Metal doesn't use GL external textures),
            // so trigger playback preparation here after all DSL properties have been applied.
            self.should_prepare_playback = self.autoplay || self.show_idle_thumbnail;
            self.maybe_prepare_playback(cx);
        });
    }
}

impl VideoRef {
    /// Prepares the video for playback. Does not start playback or update the video texture.
    ///
    /// Once playback is prepared, [`begin_playback`] can be called to start the actual playback.
    ///
    /// Alternatively, [`begin_playback`] (which uses [`prepare_playback`]) can be called if you want to start playback as soon as it's prepared.
    pub fn prepare_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.prepare_playback(cx);
        }
    }

    /// Starts the video playback. Calls `prepare_playback(cx)` if the video not already prepared.
    pub fn begin_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.begin_playback(cx);
        }
    }

    /// Pauses the video playback. Ignores if the video is not currently playing.
    pub fn pause_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pause_playback(cx);
        }
    }

    /// Pauses the video playback. Ignores if the video is already playing.
    pub fn resume_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.resume_playback(cx);
        }
    }

    /// Mutes the video playback. Ignores if the video is not currently playing or already muted.
    pub fn mute_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.mute_playback(cx);
        }
    }

    /// Unmutes the video playback. Ignores if the video is not currently muted or not playing.
    pub fn unmute_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.unmute_playback(cx);
        }
    }

    /// Stops playback and performs cleanup of all resources related to playback,
    /// including data source, decoding threads, object references, etc.
    ///
    /// In order to play the video again you must either call [`prepare_playback`] or [`begin_playback`].
    pub fn stop_and_cleanup_resources(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop_and_cleanup_resources(cx);
        }
    }

    /// Returns the most recent known playback position in milliseconds.
    pub fn current_position_ms(&self) -> u128 {
        if let Some(inner) = self.borrow() {
            inner.current_position_ms
        } else {
            0
        }
    }

    /// Seeks playback to the given position in milliseconds. Has no effect if the
    /// underlying player has not yet been prepared.
    pub fn seek_to(&self, cx: &mut Cx, position_ms: u64) {
        if let Some(inner) = self.borrow() {
            cx.seek_video_playback(inner.id, position_ms);
        }
    }

    /// Returns the total duration of the video in milliseconds, or 0 if not yet known
    /// (the duration is reported by the platform once playback has been prepared).
    pub fn total_duration_ms(&self) -> u128 {
        if let Some(inner) = self.borrow() {
            inner.total_duration
        } else {
            0
        }
    }

    /// Sets the playback volume, where `volume` is clamped to the `0.0..=1.0` range.
    /// A volume of `0.0` mutes the audio; any value above `0.0` unmutes it. The audio
    /// state is kept in sync so [`is_muted`] reflects the change.
    pub fn set_volume(&self, cx: &mut Cx, volume: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            let volume = volume.clamp(0.0, 1.0);
            cx.set_video_volume(inner.id, volume);
            inner.audio_state = if volume <= 0.0 {
                AudioState::Muted
            } else {
                AudioState::Playing
            };
        }
    }

    /// Sets the playback rate (speed), where `1.0` is normal speed. Common values are
    /// `0.5`, `0.75`, `1.0`, `1.25`, `1.5`, `1.75`, and `2.0`. The value is forwarded to
    /// the platform player; unsupported rates may be clamped by the backend.
    pub fn set_playback_rate(&self, cx: &mut Cx, rate: f64) {
        if let Some(inner) = self.borrow() {
            cx.set_video_playback_rate(inner.id, rate);
        }
    }

    /// Select a video track by index from [`VideoRef::video_tracks`] / prepared labels.
    pub fn select_video_track(&self, cx: &mut Cx, index: usize) {
        if let Some(inner) = self.borrow() {
            cx.select_video_track(inner.id, index);
        }
    }

    /// Select an audio track by index from [`VideoRef::audio_tracks`] / prepared labels.
    pub fn select_audio_track(&self, cx: &mut Cx, index: usize) {
        if let Some(inner) = self.borrow() {
            cx.select_audio_track(inner.id, index);
        }
    }

    /// Video track labels from prepare / mid-stream collection updates.
    pub fn video_tracks(&self) -> Vec<String> {
        if let Some(inner) = self.borrow() {
            inner.video_tracks.clone()
        } else {
            Vec::new()
        }
    }

    /// Audio track labels from prepare / mid-stream collection updates.
    pub fn audio_tracks(&self) -> Vec<String> {
        if let Some(inner) = self.borrow() {
            inner.audio_tracks.clone()
        } else {
            Vec::new()
        }
    }

    /// Updates the source of the video data. Currently it only proceeds if the video is in Unprepared state.
    pub fn set_source(&self, source: VideoDataSource) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source(source);
        }
    }

    pub fn set_camera_preview_mode(&self, cx: &mut Cx, mode: VideoCameraPreviewMode) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_camera_preview_mode(cx, mode);
        }
    }

    /// Sets a camera source for the video.
    pub fn set_source_camera(
        &self,
        _cx: &mut Cx,
        input_id: VideoInputId,
        format_id: VideoFormatId,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.playback_state == PlaybackState::Unprepared {
                inner.clear_session_source_registration();
                inner.source = VideoDataSource::Camera {
                    input_id: input_id.0,
                    format_id: format_id.0,
                };
                inner.source_mode = VideoSourceMode::YuvPlanes;
            }
        }
    }

    /// Sets an in-memory source for the video data.
    ///
    /// This bypasses filesystem and network paths and is useful for embedded media assets.
    pub fn set_source_in_memory(&self, data: Rc<Vec<u8>>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source_in_memory(data);
        }
    }

    /// Sets an app-owned decoded-frame session as the source for this video.
    ///
    /// The session is consumed by the next playback preparation and is rendered
    /// through the normal widget YUV path.
    pub fn set_source_session<S>(&self, session: S)
    where
        S: VideoFrameSession + 'static,
    {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source_session(Box::new(session));
        }
    }

    /// Sets an app-owned decoded-frame session as the source for this video.
    pub fn set_boxed_source_session(&self, session: Box<dyn VideoFrameSession>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source_session(session);
        }
    }

    /// Determines if this video instance should dispatch [`VideoAction::TextureUpdated`] actions on each texture update.
    /// This is disbaled by default because it can be quite nosiy when debugging actions.
    pub fn should_dispatch_texture_updates(&self, should_dispatch: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.should_dispatch_texture_updates = should_dispatch;
        }
    }

    pub fn set_thumbnail_texture(&self, cx: &mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.thumbnail_texture = texture;
            inner.load_thumbnail_image(cx);
        }
    }

    pub fn is_unprepared(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Unprepared;
        }
        false
    }

    pub fn is_preparing(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Preparing;
        }
        false
    }

    pub fn is_prepared(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Prepared;
        }
        false
    }

    pub fn is_playing(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Playing;
        }
        false
    }

    pub fn is_paused(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Paused;
        }
        false
    }

    pub fn has_completed(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Completed;
        }
        false
    }

    pub fn is_cleaning_up(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::CleaningUp;
        }
        false
    }

    pub fn is_muted(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.audio_state == AudioState::Muted;
        }
        false
    }

    /// The picture's size as the platform reported it (0x0 until prepared).
    pub fn video_size(&self) -> (usize, usize) {
        self.borrow().map_or((0, 0), |inner| (inner.video_width, inner.video_height))
    }

    /// The last decoding error the platform reported, if any.
    pub fn last_error(&self) -> Option<String> {
        self.borrow().and_then(|inner| inner.last_error.clone())
    }
}

#[derive(Clone, Debug, Script, ScriptHook, PartialEq)]
pub enum VideoCameraPreviewMode {
    #[pick]
    Auto,
    Texture,
    Native,
}

#[derive(Default, PartialEq, Debug)]
enum VideoSourceMode {
    #[default]
    ExternalTexture,
    YuvPlanes,
}

#[derive(Default, PartialEq, Debug)]
enum PlaybackState {
    #[default]
    Unprepared,
    Preparing,
    Prepared,
    Playing,
    Paused,
    /// When playback reached end of stream, only observable when not looping.
    Completed,
    /// When the platform is called to stop playback and release all resources
    /// including data source, object references, decoding threads, etc.
    ///
    /// Once cleanup has completed, the video will go into `Unprepared` state.
    CleaningUp,
}

#[derive(Default, PartialEq, Debug)]
enum AudioState {
    #[default]
    Playing,
    Muted,
}

#[derive(Clone, Debug, Default)]
pub enum VideoAction {
    #[default]
    None,
    PlaybackPrepared,
    PlaybackBegan,
    TextureUpdated,
    PlaybackCompleted,
    /// Stream collection labels changed after prepare (e.g. HLS variants).
    TracksChanged,
    PlayerReset,
    // The video view was secondary clicked (right-clicked) or long-pressed.
    SecondaryClicked {
        abs: Vec2d,
        modifiers: KeyModifiers,
    },
}

impl Widget for Video {
    /// Video plays through the web runtime's media element.
    fn scan(&mut self, _cx: &mut Cx, scan: &ScanEvent, _scope: &mut Scope) {
        scan.need_js_feature("video-playback");
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(texture) = &self.thumbnail_texture {
            self.draw_bg.draw_vars.set_texture(1, texture);
        }
        // GLMemory TEXTURE_2D: keep sampler2D slot + uniforms live every frame.
        if self.rgba_gl_2d {
            self.draw_bg
                .set_uniform(cx, id!(video_rgba_2d), &[1.0]);
            self.draw_bg.set_uniform(cx, id!(yuv_enabled), &[0.0]);
            if let Some(tex) = self.video_texture.as_ref() {
                self.draw_bg.draw_vars.set_texture(7, tex);
            }
            if self.oes_dummy.is_none() {
                self.oes_dummy =
                    Some(Texture::new_with_format(cx, TextureFormat::VideoExternal));
            }
            if let Some(tex) = self.oes_dummy.as_ref() {
                self.draw_bg.draw_vars.set_texture(0, tex);
            }
        }

        // set on every draw: a re-apply of the style (rotation, theme) puts
        // the uniforms back to their defaults
        let cover = if self.fit == VideoFit::Cover { 1.0 } else { 0.0 };
        self.draw_bg.set_uniform(cx, id!(fit_cover), &[cover]);
        let size = self.picture_size(cx);
        self.draw_bg.set_uniform(cx, id!(source_size), &size);
        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_bg.end(cx);

        self.sync_native_camera_preview(cx);

        self.draw_error_overlay(cx);

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        match event {
            Event::VideoYuvTexturesReady(event) => {
                if event.video_id == self.id {
                    self.tex_y = Some(event.tex_y.clone());
                    self.tex_u = Some(event.tex_u.clone());
                    self.tex_v = Some(event.tex_v.clone());
                    self.draw_bg.draw_vars.set_texture(2, &event.tex_y);
                    self.draw_bg.draw_vars.set_texture(3, &event.tex_u);
                    self.draw_bg.draw_vars.set_texture(4, &event.tex_v);
                    // Linux DMA-Buf EXTERNAL_OES planes (optional extension).
                    if let Some(ext) = event.external.as_ref() {
                        self.tex_y_oes = Some(ext.tex_y.clone());
                        self.tex_u_oes = Some(ext.tex_u.clone());
                        self.draw_bg.draw_vars.set_texture(5, &ext.tex_y);
                        self.draw_bg.draw_vars.set_texture(6, &ext.tex_u);
                    } else {
                        self.tex_y_oes = None;
                        self.tex_u_oes = None;
                    }
                }
            }
            Event::VideoPlaybackPrepared(event) => {
                if event.video_id == self.id {
                    self.handle_playback_prepared(cx, event);
                    cx.widget_action(uid, VideoAction::PlaybackPrepared);
                }
            }
            Event::VideoTracksChanged(event) => {
                if event.video_id == self.id {
                    self.video_tracks = event.video_tracks.clone();
                    self.audio_tracks = event.audio_tracks.clone();
                    cx.widget_action(uid, VideoAction::TracksChanged);
                }
            }
            Event::VideoTextureUpdated(event) => {
                if event.video_id == self.id {
                    // Don't overwrite local position while dragging or right after a seek —
                    // the platform may still report stale positions before the seek takes effect.
                    // Also skip 0 when we have a meaningful position — handles platforms that
                    // don't track position (Android always reports 0).
                    // Skip 0 when we have a meaningful position (a
                    // platform that does not track position reports 0).
                    if event.current_position_ms > 0 {
                        self.current_position_ms = event.current_position_ms;
                    }
                    // Update YUV shader uniforms from platform event
                    self.draw_bg
                        .set_uniform(cx, id!(yuv_enabled), &[event.yuv.shader_enabled()]);
                    self.draw_bg
                        .set_uniform(cx, id!(yuv_type), &[event.yuv.matrix]);
                    self.draw_bg
                        .set_uniform(cx, id!(yuv_biplanar), &[event.yuv.shader_biplanar()]);
                    self.draw_bg
                        .set_uniform(cx, id!(yuv_sample_mode), &[event.yuv.shader_sample_mode()]);
                    self.draw_bg.set_uniform(
                        cx,
                        id!(video_rgba_2d),
                        &[if event.rgba_gl_2d { 1.0 } else { 0.0 }],
                    );
                    self.rgba_gl_2d = event.rgba_gl_2d;
                    self.draw_bg
                        .set_uniform(cx, id!(yuv_full_range), &[event.yuv.shader_full_range()]);
                    self.draw_bg.set_uniform(
                        cx,
                        id!(yuv_rotation_steps),
                        &[event.yuv.rotation_steps],
                    );
                    self.quarter_turned = (event.yuv.rotation_steps.round() as i32) % 2 != 0;
                    // Keep every samplerExternalOES slot bound to a real OES texture.
                    // An empty video_texture + live tex_*_oes can 花屏 on NVIDIA.
                    if event.yuv.external {
                        if let Some(tex) = self.tex_y_oes.as_ref() {
                            self.draw_bg.draw_vars.set_texture(0, tex);
                            self.draw_bg.draw_vars.set_texture(5, tex);
                        }
                        if let Some(tex) = self.tex_u_oes.as_ref() {
                            self.draw_bg.draw_vars.set_texture(6, tex);
                        }
                    } else if event.yuv.array {
                        // Slots 8/9 = tex_y_arr / tex_u_arr (after video_texture_2d).
                        // Array SRVs must not stay bound to texture_2d slots 2/3.
                        if let Some(tex) = self.tex_y.as_ref() {
                            self.draw_bg.draw_vars.set_texture(8, tex);
                        }
                        if let Some(tex) = self.tex_u.as_ref() {
                            self.draw_bg.draw_vars.set_texture(9, tex);
                        }
                        self.draw_bg.draw_vars.empty_texture(2);
                        self.draw_bg.draw_vars.empty_texture(3);
                    } else {
                        // Blit / CPU YUV: Texture2D plane SRVs on slots 2/3/4.
                        if let Some(tex) = self.tex_y.as_ref() {
                            self.draw_bg.draw_vars.set_texture(2, tex);
                        }
                        if let Some(tex) = self.tex_u.as_ref() {
                            self.draw_bg.draw_vars.set_texture(3, tex);
                        }
                        if let Some(tex) = self.tex_v.as_ref() {
                            self.draw_bg.draw_vars.set_texture(4, tex);
                        }
                        self.draw_bg.draw_vars.empty_texture(8);
                        self.draw_bg.draw_vars.empty_texture(9);
                        if event.rgba_gl_2d {
                            // Slot 7 = video_texture_2d (declared after OES planes).
                            if let Some(tex) = self.video_texture.as_ref() {
                                self.draw_bg.draw_vars.set_texture(7, tex);
                            }
                            // OES slot 0 still needs a live EXTERNAL_OES texture when the
                            // shader declares samplerExternalOES (even if unused this frame).
                            if self.oes_dummy.is_none() {
                                self.oes_dummy = Some(Texture::new_with_format(
                                    cx,
                                    TextureFormat::VideoExternal,
                                ));
                            }
                            if let Some(tex) = self.oes_dummy.as_ref() {
                                self.draw_bg.draw_vars.set_texture(0, tex);
                            }
                        }
                    }
                    self.redraw(cx);
                    if self.playback_state == PlaybackState::Prepared && self.autoplay {
                        self.playback_state = PlaybackState::Playing;
                        cx.widget_action(uid, VideoAction::PlaybackBegan);
                        self.draw_bg.set_uniform(cx, id!(show_thumbnail), &[0.0]);
                    }
                    if self.should_dispatch_texture_updates {
                        cx.widget_action(uid, VideoAction::TextureUpdated);
                    }
                }
            }
            Event::VideoPlaybackCompleted(event) => {
                if event.video_id == self.id {
                    if !self.is_looping {
                        self.playback_state = PlaybackState::Completed;
                        cx.widget_action(uid, VideoAction::PlaybackCompleted);
                    }
                }
            }
            Event::VideoPlaybackResourcesReleased(event) => {
                if event.video_id == self.id {
                    self.playback_state = PlaybackState::Unprepared;
                    self.video_tracks.clear();
                    self.audio_tracks.clear();
                    if self.native_preview_attached {
                        cx.detach_camera_native_preview(self.id);
                        self.native_preview_attached = false;
                    }
                    cx.widget_action(uid, VideoAction::PlayerReset);
                }
            }
            Event::TextureHandleReady(event) => {
                if let Some(video_texture) = &self.video_texture {
                    if event.texture_id == video_texture.texture_id() {
                        self.video_texture_handle = Some(event.handle);
                        self.maybe_prepare_playback(cx);
                    }
                }
            }
            _ => (),
        }

        self.handle_gestures(cx, event, scope);
        self.handle_activity_events(cx, event);
        self.handle_errors(cx, event);
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.clear_session_source_registration();
    }
}

impl ImageCacheImpl for Video {
    fn get_texture(&self, _id: usize) -> &Option<Texture> {
        &self.thumbnail_texture
    }

    fn set_texture(&mut self, texture: Option<Texture>, _id: usize) {
        self.thumbnail_texture = texture;
    }
}

impl Video {
    fn resolve_camera_preview_mode(
        &self,
    ) -> makepad_platform::event::video_playback::CameraPreviewMode {
        use makepad_platform::event::video_playback::CameraPreviewMode as PlatformMode;
        match self.camera_preview_mode {
            VideoCameraPreviewMode::Texture => PlatformMode::Texture,
            VideoCameraPreviewMode::Native => PlatformMode::Native,
            VideoCameraPreviewMode::Auto => {
                #[cfg(any(target_os = "ios", target_os = "android", target_os = "macos"))]
                {
                    PlatformMode::Native
                }
                #[cfg(not(any(target_os = "ios", target_os = "android", target_os = "macos")))]
                {
                    PlatformMode::Texture
                }
            }
        }
    }

    fn should_use_native_camera_preview(&self) -> bool {
        matches!(self.source, VideoDataSource::Camera { .. })
            && matches!(
                self.resolve_camera_preview_mode(),
                makepad_platform::event::video_playback::CameraPreviewMode::Native
            )
    }

    fn sync_native_camera_preview(&mut self, cx: &mut Cx) {
        if !self.should_use_native_camera_preview() {
            if self.native_preview_attached {
                cx.detach_camera_native_preview(self.id);
                self.native_preview_attached = false;
            }
            return;
        }

        let area = self.draw_bg.area();
        let visible = self.playback_state != PlaybackState::CleaningUp
            && self.playback_state != PlaybackState::Unprepared;

        if !visible {
            if self.native_preview_attached {
                cx.detach_camera_native_preview(self.id);
                self.native_preview_attached = false;
            }
            return;
        }

        if !self.native_preview_attached {
            cx.attach_camera_native_preview(self.id, area);
            self.native_preview_attached = true;
        }
        cx.update_camera_native_preview(self.id, area, visible);
    }

    fn clear_session_source_registration(&mut self) {
        if let Some(session_id) = self.session_source_id.take() {
            let _ = unregister_video_frame_session(session_id);
        }
        self.has_session_source = false;
    }

    fn infer_source_mode(&self) -> VideoSourceMode {
        if self.has_session_source {
            return VideoSourceMode::YuvPlanes;
        }
        if self.in_memory_source.is_some() {
            return VideoSourceMode::ExternalTexture;
        }
        match &self.source {
            VideoDataSource::Camera { .. } => VideoSourceMode::YuvPlanes,
            _ => VideoSourceMode::ExternalTexture,
        }
    }

    fn ensure_primary_texture(&mut self, cx: &mut Cx) {
        let desired = self.infer_source_mode();
        let needs_recreate = self.video_texture.is_none() || self.source_mode != desired;

        if needs_recreate {
            self.video_texture = Some(match desired {
                VideoSourceMode::ExternalTexture => {
                    Texture::new_with_format(cx, TextureFormat::VideoExternal)
                }
                // In YUV mode the primary texture is unused (shader samples
                // tex_y/tex_u/tex_v), but it must still be a valid video texture
                // for the pipeline plumbing.
                VideoSourceMode::YuvPlanes => {
                    Texture::new_with_format(cx, TextureFormat::VideoExternal)
                }
            });
        }

        self.source_mode = desired;
        if let Some(texture) = self.video_texture.as_ref() {
            self.draw_bg.draw_vars.set_texture(0, texture);
        }
    }

    fn init_video_texture(&mut self, cx: &mut Cx) {
        self.id = LiveId::unique();
        self.ensure_primary_texture(cx);

        self.should_prepare_playback = self.autoplay;
        self.maybe_prepare_playback(cx);
    }

    fn apply_thumbnail_settings(&mut self, cx: &mut Cx) {
        self.lazy_create_image_cache(cx);
        // Don't pre-allocate an empty thumbnail texture here: load_thumbnail_image
        // (via load_png_from_data / load_jpg_from_data) already calls set_texture
        // with a fresh GPU texture containing the decoded image. Allocating a
        // throwaway Texture::new(cx) first was just churning GPU resources.

        if self.show_idle_thumbnail {
            let loaded = self.load_thumbnail_image(cx);
            // Only enable the thumbnail shader path if a thumbnail was actually loaded;
            // otherwise the shader would sample from an empty texture.
            if loaded {
                self.draw_bg.set_uniform(cx, id!(show_thumbnail), &[1.0]);
            }
        }
    }

    fn maybe_prepare_playback(&mut self, cx: &mut Cx) {
        self.ensure_primary_texture(cx);
        if self.playback_state == PlaybackState::Unprepared && self.should_prepare_playback {
            let source = if self.has_session_source {
                match self.session_source_id.take() {
                    Some(session_id) => VideoSource::Session(session_id),
                    None => {
                        error!("Attempted to prepare playback: session source already consumed");
                        return;
                    }
                }
            } else if let Some(data) = self.in_memory_source.clone() {
                VideoSource::InMemory(data)
            } else {
                match &self.source {
                    VideoDataSource::Dependency { res } => {
                        if let Some(handle_ref) = res {
                            let handle = handle_ref.as_handle();
                            match cx.get_resource(handle_ref.heap_key(), handle) {
                                Some(data) => VideoSource::InMemory(data),
                                None => {
                                    error!("Attempted to prepare playback: resource not found");
                                    return;
                                }
                            }
                        } else {
                            error!("Attempted to prepare playback: no resource handle provided");
                            return;
                        }
                    }
                    VideoDataSource::Network { url } => VideoSource::Network(url.to_string()),
                    VideoDataSource::Filesystem { path } => {
                        VideoSource::Filesystem(path.to_string())
                    }
                    VideoDataSource::Camera {
                        input_id,
                        format_id,
                    } => VideoSource::Camera(VideoInputId(*input_id), VideoFormatId(*format_id)),
                }
            };

            let Some(texture) = self.video_texture.as_ref() else {
                return;
            };
            let camera_preview_mode = if matches!(source, VideoSource::Camera(..)) {
                self.resolve_camera_preview_mode()
            } else {
                makepad_platform::event::video_playback::CameraPreviewMode::Texture
            };

            cx.prepare_video_playback(
                self.id,
                source,
                camera_preview_mode,
                self.video_texture_handle.unwrap_or(0),
                texture.texture_id(),
                self.autoplay,
                self.is_looping,
            );

            self.playback_state = PlaybackState::Preparing;
            self.last_error = None;
            self.should_prepare_playback = false;
        }
    }

    fn handle_playback_prepared(&mut self, cx: &mut Cx, event: &VideoPlaybackPreparedEvent) {
        self.playback_state = PlaybackState::Prepared;
        self.last_error = None;
        self.video_width = event.video_width as usize;
        self.video_height = event.video_height as usize;
        self.total_duration = event.duration;
        self.video_tracks = event.video_tracks.clone();
        self.audio_tracks = event.audio_tracks.clone();


        if self.mute && self.audio_state != AudioState::Muted {
            cx.mute_video_playback(self.id);
        }

        if self.autoplay {
            cx.begin_video_playback(self.id);
            self.playback_state = PlaybackState::Playing;
            self.draw_bg.set_uniform(cx, id!(show_thumbnail), &[0.0]);
        }
    }

    /// The picture's display size for the shader's fit: the decoder's display
    /// size (turned when the shader turns the planes a quarter), the
    /// thumbnail's until the video reports one.
    fn picture_size(&self, cx: &mut Cx) -> [f32; 2] {
        if self.video_width > 0 && self.video_height > 0 {
            let (w, h) = (self.video_width as f32, self.video_height as f32);
            return if self.quarter_turned { [h, w] } else { [w, h] };
        }
        match self.thumbnail_texture.as_ref().and_then(|t| t.get_format(cx).vec_width_height()) {
            Some((w, h)) => [w as f32, h as f32],
            None => [0.0, 0.0],
        }
    }

    /// The picture's own presses: hold-to-pause and the secondary click.
    /// Transport gestures belong to `PlayerControls` (see `VideoPlayer`).
    fn handle_gestures(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) if fe.is_primary_hit() && self.hold_to_pause => {
                // Only a pause this press caused is its to undo.
                self.held_paused = self.playback_state == PlaybackState::Playing;
                self.pause_playback(cx);
            }
            // Released or taken away: a hold that paused playback lets go.
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if std::mem::take(&mut self.held_paused) {
                    self.resume_playback(cx);
                }
            }
            Hit::FingerDown(fe) if fe.mouse_button().is_some_and(|mb| mb.is_secondary()) => {
                self.handle_secondary_click(cx, scope, fe.abs, fe.modifiers);
            }
            Hit::FingerLongPress(lp) => {
                self.handle_secondary_click(cx, scope, lp.abs, Default::default());
            }
            _ => (),
        }
    }

    fn handle_activity_events(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Pause => self.pause_playback(cx),
            Event::Resume => self.resume_playback(cx),
            _ => (),
        }
    }

    fn handle_errors(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::VideoDecodingError(event) = event {
            if event.video_id == self.id {
                error!(
                    "Error decoding video with id {} : {}",
                    self.id.0, event.error
                );
                // Recover from a failed prepare/playback attempt so callers can retry.
                self.playback_state = PlaybackState::Unprepared;
                self.should_prepare_playback = false;
                self.last_error = Some(event.error.clone());
                if self.native_preview_attached {
                    cx.detach_camera_native_preview(self.id);
                    self.native_preview_attached = false;
                }
                self.redraw(cx);
            }
        }
    }

    fn prepare_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Unprepared {
            self.should_prepare_playback = true;
            self.maybe_prepare_playback(cx);
        }
    }

    fn begin_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Unprepared {
            self.should_prepare_playback = true;
            self.autoplay = true;
            self.maybe_prepare_playback(cx);
        } else if self.playback_state == PlaybackState::Prepared {
            cx.begin_video_playback(self.id);
        }
    }

    fn handle_secondary_click(
        &mut self,
        cx: &mut Cx,
        _scope: &mut Scope,
        abs: Vec2d,
        modifiers: KeyModifiers,
    ) {
        cx.widget_action(
            self.widget_uid(),
            VideoAction::SecondaryClicked { abs, modifiers },
        );
    }

    fn pause_playback(&mut self, cx: &mut Cx) {
        if self.playback_state != PlaybackState::Paused {
            cx.pause_video_playback(self.id);
            self.playback_state = PlaybackState::Paused;
        }
    }

    fn resume_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Paused {
            cx.resume_video_playback(self.id);
            self.playback_state = PlaybackState::Playing;
        }
    }

    fn mute_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Playing
            || self.playback_state == PlaybackState::Paused
            || self.playback_state == PlaybackState::Prepared
        {
            cx.mute_video_playback(self.id);
            self.audio_state = AudioState::Muted;
        }
    }

    fn unmute_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Playing
            || self.playback_state == PlaybackState::Paused
            || self.playback_state == PlaybackState::Prepared
                && self.audio_state == AudioState::Muted
        {
            cx.unmute_video_playback(self.id);
            self.audio_state = AudioState::Playing;
        }
    }

    fn stop_and_cleanup_resources(&mut self, cx: &mut Cx) {
        if self.playback_state != PlaybackState::Unprepared
            && self.playback_state != PlaybackState::CleaningUp
        {
            cx.cleanup_video_playback_resources(self.id);
            if self.native_preview_attached {
                cx.detach_camera_native_preview(self.id);
                self.native_preview_attached = false;
            }

            self.playback_state = PlaybackState::CleaningUp;
            self.autoplay = false;
            self.should_prepare_playback = false;
        }
    }

    fn set_source(&mut self, source: VideoDataSource) {
        if self.playback_state == PlaybackState::Unprepared {
            self.clear_session_source_registration();
            self.in_memory_source = None;
            self.source_mode = match source {
                VideoDataSource::Camera { .. } => VideoSourceMode::YuvPlanes,
                _ => VideoSourceMode::ExternalTexture,
            };
            self.source = source;
        } else {
            error!(
                "Attempted to set source while player {} state is: {:?}",
                self.id.0, self.playback_state
            );
        }
    }

    fn set_camera_preview_mode(&mut self, cx: &mut Cx, mode: VideoCameraPreviewMode) {
        if self.camera_preview_mode == mode {
            return;
        }
        self.camera_preview_mode = mode;
        if self.native_preview_attached && !self.should_use_native_camera_preview() {
            cx.detach_camera_native_preview(self.id);
            self.native_preview_attached = false;
        }
        self.redraw(cx);
    }

    fn set_source_in_memory(&mut self, data: Rc<Vec<u8>>) {
        if self.playback_state == PlaybackState::Unprepared {
            self.clear_session_source_registration();
            self.source_mode = VideoSourceMode::ExternalTexture;
            self.in_memory_source = Some(data);
        } else {
            error!(
                "Attempted to set source while player {} state is: {:?}",
                self.id.0, self.playback_state
            );
        }
    }

    fn set_source_session(&mut self, session: Box<dyn VideoFrameSession>) {
        if self.playback_state == PlaybackState::Unprepared {
            self.clear_session_source_registration();
            self.in_memory_source = None;
            self.session_source_id = Some(register_video_frame_session(session));
            self.has_session_source = true;
            self.source_mode = VideoSourceMode::YuvPlanes;
        } else {
            error!(
                "Attempted to set source while player {} state is: {:?}",
                self.id.0, self.playback_state
            );
        }
    }

    fn draw_error_overlay(&mut self, cx: &mut Cx2d) {
        let Some(error_text) = self.last_error.as_ref() else {
            return;
        };

        let video_rect = self.draw_bg.area().rect(cx);
        if video_rect.size.x <= 0.0 || video_rect.size.y <= 0.0 {
            return;
        }

        let msg_owned = if error_text.chars().count() > 140 {
            let mut s = error_text.chars().take(140).collect::<String>();
            s.push_str("...");
            s
        } else {
            error_text.clone()
        };
        let msg = msg_owned.as_str();

        let pad_x = 10.0;
        let pad_y = 6.0;
        let max_width = (video_rect.size.x - 2.0 * pad_x).max(32.0);
        let laid_out = self.draw_error_text.layout(
            cx,
            max_width as f32,
            0.0,
            None,
            false,
            Align::default(),
            msg,
        );
        let text_w = laid_out.size_in_lpxs.width as f64;
        let text_h = laid_out.size_in_lpxs.height as f64;

        let bg_h = text_h + 2.0 * pad_y;
        self.draw_error_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(video_rect.pos.x, video_rect.pos.y),
                size: dvec2(video_rect.size.x, bg_h),
            },
        );

        let text_x = video_rect.pos.x + ((video_rect.size.x - text_w) * 0.5).max(pad_x);
        let text_y = video_rect.pos.y + pad_y;
        self.draw_error_text
            .draw_abs(cx, dvec2(text_x, text_y), msg);
    }

    fn load_thumbnail_image(&mut self, cx: &mut Cx) -> bool {
        if let Some(ref handle_ref) = self.thumbnail_source {
            let handle = handle_ref.as_handle();
            if let Some(data) = cx.get_resource(handle_ref.heap_key(), handle) {
                // Try to load as PNG first, then JPG
                if self.load_png_from_data(cx, &data, 0).is_ok() {
                    return true;
                }
                if self.load_jpg_from_data(cx, &data, 0).is_ok() {
                    return true;
                }
                error!("Failed to load thumbnail image: unsupported format");
            }
        }
        false
    }
}

/// How a [`Video`] places its picture (display aspect kept either way).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Script, ScriptHook)]
pub enum VideoFit {
    /// The whole picture, centred, transparent bars on the free sides.
    #[default]
    Contain,
    /// The picture fills the widget, cropped on one axis.
    Cover,
}

/// The script-facing source of the video data.
///
/// For app-owned live sessions, use [`VideoRef::set_source_session`].
///
/// [`Dependency`]: A resource handle (loaded with `crate_resource("self:path/to/video.mp4")`).
///
/// [`Network`]: The URL of a video file, it can be any regular HTTP download or HLS, DASH, RTMP, RTSP.
///
/// [`Filesystem`]: The path to a video file on the local filesystem. This requires runtime-approved permissions for reading storage.
#[derive(Clone, Debug, Script, ScriptHook)]
pub enum VideoDataSource {
    #[pick {res: None}]
    Dependency { res: Option<ScriptHandleRef> },
    #[live {url: "".to_string()}]
    Network { url: String },
    #[live {path: "".to_string()}]
    Filesystem { path: String },
    #[live {input_id: LiveId::empty(), format_id: LiveId::empty()}]
    Camera { input_id: LiveId, format_id: LiveId },
}
