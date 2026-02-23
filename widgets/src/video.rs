use std::time::Instant;
use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl},
    image_cache::ImageCacheImpl, makepad_derive_widget::*, makepad_draw::*,
    makepad_platform::event::video_playback::*, widget::*,
};


script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.VideoDataSource = #(VideoDataSource::script_api(vm))
    mod.widgets.VideoBase = #(Video::register_widget(vm))

    mod.widgets.Video = set_type_default() do mod.widgets.VideoBase{
        width: 100
        height: 100

        draw_bg +: {
            video_texture: texture_video()
            thumbnail_texture: texture_2d(float)
            show_thumbnail: uniform(0.0)

            opacity: instance(1.0)
            image_scale: instance(vec2(1.0, 1.0))
            image_pan: instance(vec2(0.5, 0.5))

            source_size: uniform(vec2(1.0, 1.0))
            target_size: uniform(vec2(-1.0, -1.0))

            get_color_scale_pan: fn() {
                // Early return for default scaling and panning,
                // used when walk size is not specified or non-fixed.
                if self.target_size.x <= 0.0 || self.target_size.y <= 0.0 {
                    if self.show_thumbnail > 0.0 {
                        return self.thumbnail_texture.sample_as_bgra(self.pos).xyzw
                    } else {
                        return self.video_texture.sample_video(self.pos)
                    }
                }

                let mut scale = self.image_scale
                let pan = self.image_pan
                let source_aspect_ratio = self.source_size.x / self.source_size.y
                let target_aspect_ratio = self.target_size.x / self.target_size.y

                // Adjust scale based on aspect ratio difference
                if source_aspect_ratio != target_aspect_ratio {
                    if source_aspect_ratio > target_aspect_ratio {
                        scale.x = target_aspect_ratio / source_aspect_ratio
                        scale.y = 1.0
                    } else {
                        scale.x = 1.0
                        scale.y = source_aspect_ratio / target_aspect_ratio
                    }
                }

                // Calculate the range for panning
                let pan_range_x = max(0.0, 1.0 - scale.x)
                let pan_range_y = max(0.0, 1.0 - scale.y)

                // Adjust the user pan values to be within the pan range
                let adjusted_pan_x = pan_range_x * pan.x
                let adjusted_pan_y = pan_range_y * pan.y
                let adjusted_pan = vec2(adjusted_pan_x, adjusted_pan_y)
                let adjusted_pos = self.pos * scale + adjusted_pan

                if self.show_thumbnail > 0.5 {
                    return self.thumbnail_texture.sample_as_bgra(adjusted_pos).xyzw
                } else {
                    return self.video_texture.sample_video(adjusted_pos)
                }
            }

            pixel: fn() {
                let color = self.get_color_scale_pan()
                return Pal.premul(vec4(color.xyz, color.w * self.opacity))
            }
        }

        draw_center_play_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let r = min(c.x, c.y)
                sdf.circle(c.x, c.y, r)
                sdf.fill(Pal.premul(vec4(0.0, 0.0, 0.0, 0.5)))
                return sdf.result
            }
        }

        draw_center_play_icon +: {
            color: #fff
            text_style: theme.font_icons{
                font_size: 20.0
            }
        }

        draw_controls_bg +: {
            controls_opacity: instance(0.0)
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.6 * self.controls_opacity)
            }
        }

        draw_play_icon +: {
            color: #0000
            text_style: theme.font_icons{
                font_size: 11.0
            }
        }

        draw_restart_icon +: {
            color: #0000
            text_style: theme.font_icons{
                font_size: 10.0
            }
        }

        draw_volume_icon +: {
            color: #0000
            text_style: theme.font_icons{
                font_size: 10.0
            }
        }

        draw_time_text +: {
            color: #0000
            text_style: theme.font_regular{
                font_size: 8.0
            }
        }

        draw_progress_bg +: {
            controls_opacity: instance(0.0)
            pixel: fn() {
                return Pal.premul(vec4(1.0, 1.0, 1.0, 0.3 * self.controls_opacity))
            }
        }

        draw_progress_fill +: {
            controls_opacity: instance(0.0)
            pixel: fn() {
                return Pal.premul(vec4(1.0, 1.0, 1.0, 0.9 * self.controls_opacity))
            }
        }

        draw_progress_thumb +: {
            controls_opacity: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let r = min(c.x, c.y)
                sdf.circle(c.x, c.y, r)
                sdf.fill(#fff)
                return sdf.result * self.controls_opacity
            }
        }

        draw_seek_indicator +: {
            indicator_opacity: instance(0.0)
            direction: uniform(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let sz = min(self.rect_size.x, self.rect_size.y) * 0.3
                if self.direction > 0.5 {
                    // Right arrow (fast-forward) - CCW winding
                    sdf.move_to(c.x - sz, c.y - sz)
                    sdf.line_to(c.x + sz, c.y)
                    sdf.line_to(c.x - sz, c.y + sz)
                    sdf.close_path()
                    sdf.fill(#fff)
                } else {
                    // Left arrow (rewind) - CCW winding
                    sdf.move_to(c.x + sz, c.y + sz)
                    sdf.line_to(c.x - sz, c.y)
                    sdf.line_to(c.x + sz, c.y - sz)
                    sdf.close_path()
                    sdf.fill(#fff)
                }
                return sdf.result * self.indicator_opacity
            }
        }

        controls_height: 32.0
        show_controls: true

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.3}}
                    apply: {
                        draw_controls_bg: {controls_opacity: 0.0}
                        draw_play_icon: {color: #0000}
                        draw_restart_icon: {color: #0000}
                        draw_volume_icon: {color: #0000}
                        draw_progress_bg: {controls_opacity: 0.0}
                        draw_progress_fill: {controls_opacity: 0.0}
                        draw_progress_thumb: {controls_opacity: 0.0}
                        draw_time_text: {color: #0000}
                    }
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    apply: {
                        draw_controls_bg: {controls_opacity: 1.0}
                        draw_play_icon: {color: #fff}
                        draw_restart_icon: {color: #fff}
                        draw_volume_icon: {color: #fff}
                        draw_progress_bg: {controls_opacity: 1.0}
                        draw_progress_fill: {controls_opacity: 1.0}
                        draw_progress_thumb: {controls_opacity: 1.0}
                        draw_time_text: {color: #fff}
                    }
                }
            }
            seek_indicator: {
                default: @hidden
                hidden: AnimatorState{
                    from: {all: Forward {duration: 0.4}}
                    apply: {
                        draw_seek_indicator: {indicator_opacity: 0.0}
                    }
                }
                show: AnimatorState{
                    from: {all: Snap}
                    apply: {
                        draw_seek_indicator: {indicator_opacity: 0.8}
                    }
                }
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

/// `show_controls` - determines if overlay controls (play/pause, restart, progress bar) appear on hover. defaults to true.
///
/// `controls_height` - height of the controls bar in logical pixels. defaults to 32.0.

/// Not yet supported:
/// Widget API
///  - Seek to arbitrary timestamp
///  - Hotswap video source, `set_source(VideoDataSource)` only works if video is in Unprepared state.

#[derive(Script, Widget, Animator)]
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

    // Center play button overlay
    #[live]
    draw_center_play_bg: DrawQuad,
    #[live]
    draw_center_play_icon: DrawText,

    // Controls overlay drawing
    #[live]
    draw_controls_bg: DrawColor,
    #[live]
    draw_play_icon: DrawText,
    #[live]
    draw_restart_icon: DrawText,
    #[live]
    draw_volume_icon: DrawText,
    #[live]
    draw_progress_bg: DrawColor,
    #[live]
    draw_progress_fill: DrawColor,
    #[live]
    draw_progress_thumb: DrawQuad,
    #[live]
    draw_time_text: DrawText,
    #[live]
    draw_seek_indicator: DrawQuad,

    // Controls config
    #[live(true)]
    show_controls: bool,
    #[live(32.0)]
    controls_height: f64,
    // Animator for hover fade
    #[apply_default]
    animator: Animator,

    // Textures
    #[live]
    source: VideoDataSource,
    #[rust]
    video_texture: Option<Texture>,
    #[rust]
    video_texture_handle: Option<u32>,
    /// Requires [`show_idle_thumbnail`] to be `true`.
    #[live]
    thumbnail_source: Option<ScriptHandleRef>,
    #[rust]
    thumbnail_texture: Option<Texture>,

    // Playback
    #[live(false)]
    is_looping: bool,
    #[live(false)]
    hold_to_pause: bool,
    #[live(false)]
    autoplay: bool,
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
    #[rust]
    total_duration: u128,

    // Playback position tracking
    #[rust]
    current_position_ms: u128,

    // Interaction state
    #[rust]
    is_dragging_progress: bool,
    #[rust]
    seek_indicator_direction: f64,
    /// Whether controls are pinned visible (for touch, or click-to-pin on desktop).
    #[rust]
    controls_visible: bool,
    /// Whether the mouse is currently hovering over the video (desktop only).
    #[rust]
    controls_hover: bool,
    /// Number of position updates to skip after a seek (prevents stale platform positions from reverting the progress bar).
    #[rust]
    seek_cooldown: u32,
    /// Manual double-tap tracking (fallback for unreliable platform tap_count on touch).
    #[rust]
    last_tap_time: Option<Instant>,
    #[rust]
    last_tap_abs: DVec2,

    #[rust]
    id: LiveId,
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
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        vm.with_cx_mut(|cx| {
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

    /// Updates the source of the video data. Currently it only proceeds if the video is in Unprepared state.
    pub fn set_source(&self, source: VideoDataSource) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source(source);
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
    PlayerReset,
    // The video view was secondary clicked (right-clicked) or long-pressed.
    SecondaryClicked {
        abs: Vec2d,
        modifiers: KeyModifiers,
    },
}

impl Widget for Video {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(texture) = &self.thumbnail_texture {
            self.draw_bg.draw_vars.set_texture(1, texture);
        }

        self.draw_bg.begin(cx, walk, self.layout);
        self.draw_bg.end(cx);

        if self.show_controls {
            self.draw_center_play_button(cx);
            self.draw_controls(cx);
        }

        // Draw seek indicator overlay (centered on left or right half of video)
        self.draw_seek_indicator(cx);

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }

        let uid = self.widget_uid();
        match event {
            Event::VideoPlaybackPrepared(event) => {
                if event.video_id == self.id {
                    self.handle_playback_prepared(cx, event);
                    cx.widget_action(uid, VideoAction::PlaybackPrepared);
                }
            }
            Event::VideoTextureUpdated(event) => {
                if event.video_id == self.id {
                    // Don't overwrite local position while dragging or right after a seek —
                    // the platform may still report stale positions before the seek takes effect.
                    // Also skip 0 when we have a meaningful position — handles platforms that
                    // don't track position (Android always reports 0).
                    if self.is_dragging_progress {
                        // Keep our drag-set position
                    } else if self.seek_cooldown > 0 {
                        self.seek_cooldown -= 1;
                    } else if event.current_position_ms > 0 {
                        self.current_position_ms = event.current_position_ms;
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
        self.handle_errors(event);
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
    fn init_video_texture(&mut self, cx: &mut Cx) {
        self.id = LiveId::unique();

        if self.video_texture.is_none() {
            let new_texture = Texture::new_with_format(cx, TextureFormat::VideoRGB);
            self.video_texture = Some(new_texture);
        }
        let texture = self.video_texture.as_mut().unwrap();
        self.draw_bg.draw_vars.set_texture(0, &texture);

        #[cfg(target_os = "android")]
        match cx.os_type() {
            OsType::Android(params) if params.is_emulator => {
                panic!("Video Widget is currently only supported on real devices. (unreliable support for external textures on some emulators hosts)");
            }
            _ => {}
        }

        self.should_prepare_playback = self.autoplay;
        self.maybe_prepare_playback(cx);
    }

    fn apply_thumbnail_settings(&mut self, cx: &mut Cx) {
        self.lazy_create_image_cache(cx);
        self.thumbnail_texture = Some(Texture::new(cx));

        let target_w = self.walk.width.to_fixed().unwrap_or(0.0);
        let target_h = self.walk.height.to_fixed().unwrap_or(0.0);
        self.draw_bg
            .set_uniform(cx, id!(target_size), &[target_w as f32, target_h as f32]);

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
        if self.playback_state == PlaybackState::Unprepared && self.should_prepare_playback {
            // On Android, wait for GL texture handle before preparing
            #[cfg(target_os = "android")]
            if self.video_texture_handle.is_none() {
                // texture is not yet ready, this method will be called again on TextureHandleReady
                return;
            }

            let source = match &self.source {
                VideoDataSource::Dependency { res } => {
                    if let Some(handle_ref) = res {
                        let handle = handle_ref.as_handle();
                        match cx.get_resource(handle) {
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
                VideoDataSource::Filesystem { path } => VideoSource::Filesystem(path.to_string()),
            };

            let Some(texture) = self.video_texture.as_ref() else {
                return;
            };
            cx.prepare_video_playback(
                self.id,
                source,
                self.video_texture_handle.unwrap_or(0),
                texture.texture_id(),
                self.autoplay,
                self.is_looping,
            );

            self.playback_state = PlaybackState::Preparing;
            self.should_prepare_playback = false;
        }
    }

    fn handle_playback_prepared(&mut self, cx: &mut Cx, event: &VideoPlaybackPreparedEvent) {
        self.playback_state = PlaybackState::Prepared;
        self.video_width = event.video_width as usize;
        self.video_height = event.video_height as usize;
        self.total_duration = event.duration;

        self.draw_bg.set_uniform(
            cx,
            id!(source_size),
            &[self.video_width as f32, self.video_height as f32],
        );

        if self.mute && self.audio_state != AudioState::Muted {
            cx.mute_video_playback(self.id);
        }
    }

    fn controls_interactable(&self) -> bool {
        self.show_controls && (self.controls_visible || self.controls_hover)
    }

    /// Manual double-tap detection as a fallback for unreliable platform tap_count on touch.
    /// Returns true if a double-tap is detected, and clears the tracking state.
    /// Otherwise records this tap for future detection.
    fn check_double_tap(&mut self, abs: DVec2) -> bool {
        if let Some(last_time) = self.last_tap_time {
            let elapsed = last_time.elapsed().as_secs_f64();
            let dx = abs.x - self.last_tap_abs.x;
            let dy = abs.y - self.last_tap_abs.y;
            let dist = (dx * dx + dy * dy).sqrt();
            if elapsed < 0.4 && dist < 80.0 {
                self.last_tap_time = None;
                return true;
            }
        }
        self.last_tap_time = Some(Instant::now());
        self.last_tap_abs = abs;
        false
    }

    fn handle_gestures(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fhe) => {
                if self.show_controls {
                    self.controls_hover = true;
                    self.animator_play(cx, ids!(hover.on));
                }
                self.update_cursor(cx, fhe.abs);
            }
            Hit::FingerHoverOver(fhe) => {
                self.update_cursor(cx, fhe.abs);
            }
            Hit::FingerHoverOut(_) => {
                cx.set_cursor(MouseCursor::Arrow);
                if self.show_controls {
                    self.controls_hover = false;
                    // Don't hide controls if they're pinned visible or during drag
                    if !self.is_dragging_progress && !self.controls_visible {
                        self.animator_play(cx, ids!(hover.off));
                    }
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());

                // Double-tap detection: use platform tap_count OR manual fallback
                let is_double_tap = if fe.tap_count >= 2 {
                    self.last_tap_time = None;
                    true
                } else {
                    self.check_double_tap(fe.abs)
                };

                // Double-tap on video area: seek forward/backward
                // Only when actively playing/paused (not when center play button is showing)
                if self.show_controls && is_double_tap
                    && !self.should_show_center_play()
                    && !(self.controls_interactable() && self.hit_test_controls(cx, fe.abs))
                {
                    let video_rect = self.draw_bg.area().rect(cx);
                    let mid_x = video_rect.pos.x + video_rect.size.x * 0.5;
                    if fe.abs.x < mid_x {
                        self.seek_backward(cx);
                    } else {
                        self.seek_forward(cx);
                    }
                } else if self.controls_interactable() && self.hit_test_progress_bar(cx, fe.abs) {
                    // Start dragging progress bar
                    self.is_dragging_progress = true;
                    self.seek_to_position_from_x(cx, fe.abs.x);
                } else if self.controls_interactable() && self.hit_test_controls(cx, fe.abs) {
                    // Will be handled on FingerUp
                } else if !self.show_controls && self.hold_to_pause {
                    self.pause_playback(cx);
                }
            }
            Hit::FingerMove(fe) => {
                if self.is_dragging_progress {
                    self.seek_to_position_from_x(cx, fe.abs.x);
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if self.is_dragging_progress {
                    self.is_dragging_progress = false;
                    self.seek_to_position_from_x(cx, fe.abs.x);
                } else if self.should_show_center_play() && fe.tap_count < 2 {
                    // Center play button is showing — tap anywhere to start playback
                    self.toggle_play_pause(cx);
                } else if self.controls_interactable() && self.hit_test_controls_click(cx, fe.abs) {
                    // Control click handled
                } else if self.show_controls && fe.tap_count < 2 && !self.hit_test_controls(cx, fe.abs) {
                    // Single tap outside controls bar: toggle pinned visibility
                    self.controls_visible = !self.controls_visible;
                    if self.controls_visible {
                        self.animator_play(cx, ids!(hover.on));
                    } else if !self.controls_hover {
                        self.animator_play(cx, ids!(hover.off));
                    }
                } else if !self.show_controls && self.hold_to_pause {
                    self.resume_playback(cx);
                }
            }
            Hit::FingerDown(fe) if fe.mouse_button().is_some_and(|mb| mb.is_secondary()) => {
                self.handle_secondary_click(cx, scope, fe.abs, fe.modifiers);
            }
            Hit::FingerLongPress(lp) => {
                self.handle_secondary_click(cx, scope, lp.abs, Default::default());
            }
            Hit::KeyDown(ke) => {
                self.handle_key_seek(cx, &ke);
            }
            _ => (),
        }
    }

    fn seek_backward(&mut self, cx: &mut Cx) {
        let seek_step_ms: u128 = 5000;
        let new_pos = self.current_position_ms.saturating_sub(seek_step_ms);
        cx.seek_video_playback(self.id, new_pos as u64);
        self.current_position_ms = new_pos;
        self.seek_cooldown = 5;
        self.seek_indicator_direction = 0.0;
        self.animator_play(cx, ids!(seek_indicator.show));
        self.animator_play(cx, ids!(seek_indicator.hidden));
        self.redraw(cx);
    }

    fn seek_forward(&mut self, cx: &mut Cx) {
        let seek_step_ms: u128 = 5000;
        let new_pos = (self.current_position_ms + seek_step_ms).min(self.total_duration);
        cx.seek_video_playback(self.id, new_pos as u64);
        self.current_position_ms = new_pos;
        self.seek_cooldown = 5;
        self.seek_indicator_direction = 1.0;
        self.animator_play(cx, ids!(seek_indicator.show));
        self.animator_play(cx, ids!(seek_indicator.hidden));
        self.redraw(cx);
    }

    fn handle_key_seek(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        match ke.key_code {
            KeyCode::ArrowLeft => self.seek_backward(cx),
            KeyCode::ArrowRight => self.seek_forward(cx),
            KeyCode::Space => self.toggle_play_pause(cx),
            KeyCode::KeyM => self.toggle_mute(cx),
            _ => {}
        }
    }

    fn handle_activity_events(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Pause => self.pause_playback(cx),
            Event::Resume => self.resume_playback(cx),
            _ => (),
        }
    }

    fn handle_errors(&mut self, event: &Event) {
        if let Event::VideoDecodingError(event) = event {
            if event.video_id == self.id {
                error!(
                    "Error decoding video with id {} : {}",
                    self.id.0, event.error
                );
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
            && self.playback_state != PlaybackState::Preparing
            && self.playback_state != PlaybackState::CleaningUp
        {
            cx.cleanup_video_playback_resources(self.id);

            self.playback_state = PlaybackState::CleaningUp;
            self.autoplay = false;
            self.should_prepare_playback = false;
        }
    }

    fn set_source(&mut self, source: VideoDataSource) {
        if self.playback_state == PlaybackState::Unprepared {
            self.source = source;
        } else {
            error!(
                "Attempted to set source while player {} state is: {:?}",
                self.id.0, self.playback_state
            );
        }
    }

    fn should_show_center_play(&self) -> bool {
        matches!(
            self.playback_state,
            PlaybackState::Unprepared | PlaybackState::Preparing | PlaybackState::Prepared | PlaybackState::Completed
        )
    }

    fn draw_center_play_button(&mut self, cx: &mut Cx2d) {
        if !self.should_show_center_play() {
            return;
        }

        let video_rect = self.draw_bg.area().rect(cx);
        if video_rect.size.x <= 0.0 || video_rect.size.y <= 0.0 {
            return;
        }

        let btn_size = 48.0;
        let btn_x = video_rect.pos.x + (video_rect.size.x - btn_size) * 0.5;
        let btn_y = video_rect.pos.y + (video_rect.size.y - btn_size) * 0.5;

        // Draw semi-transparent circle background
        self.draw_center_play_bg.draw_abs(cx, Rect {
            pos: dvec2(btn_x, btn_y),
            size: dvec2(btn_size, btn_size),
        });

        // Measure the glyph to center it precisely within the circle
        let glyph = "\u{f04b}"; // fa-play
        let laid_out = self.draw_center_play_icon.layout(cx, 0.0, 0.0, None, false, Align::default(), glyph);
        let glyph_w = laid_out.size_in_lpxs.width as f64;
        let glyph_h = laid_out.size_in_lpxs.height as f64;
        // Nudge right by 2px — play triangles are geometrically centered but
        // perceptually look left-shifted due to the pointed right edge.
        let icon_x = btn_x + (btn_size - glyph_w) * 0.5 + 2.0;
        let icon_y = btn_y + (btn_size - glyph_h) * 0.5;
        self.draw_center_play_icon.draw_abs(cx, dvec2(icon_x, icon_y), glyph);
    }

    fn draw_controls(&mut self, cx: &mut Cx2d) {
        let video_rect = self.draw_bg.area().rect(cx);
        if video_rect.size.x <= 0.0 || video_rect.size.y <= 0.0 {
            return;
        }

        let ch = self.controls_height;
        let bar_y = video_rect.pos.y + video_rect.size.y - ch;

        // Begin the controls bar as a layout at absolute position (bottom of video)
        self.draw_controls_bg.begin(cx, Walk {
            abs_pos: Some(dvec2(video_rect.pos.x, bar_y)),
            width: Size::Fixed(video_rect.size.x),
            height: Size::Fixed(ch),
            ..Walk::default()
        }, Layout {
            flow: Flow::right(),
            align: Align { x: 0.0, y: 0.5 },
            spacing: 8.0,
            padding: Inset { left: 8.0, right: 12.0, top: 0.0, bottom: 0.0 },
            clip_x: true,
            clip_y: true,
            ..Layout::default()
        });

        let icon_walk = Walk::fit();

        // Play/pause icon (FontAwesome)
        let play_glyph = if matches!(self.playback_state, PlaybackState::Playing) {
            "\u{f04c}" // fa-pause
        } else {
            "\u{f04b}" // fa-play
        };
        self.draw_play_icon.draw_walk(cx, Walk {
            width: Size::Fixed(14.0),
            ..icon_walk
        }, Align::default(), play_glyph);

        // Restart icon (FontAwesome)
        self.draw_restart_icon.draw_walk(cx, icon_walk, Align::default(), "\u{f048}"); // fa-backward-step

        // Volume icon (FontAwesome)
        let volume_glyph = if self.audio_state == AudioState::Muted {
            "\u{f6a9}" // fa-volume-mute
        } else {
            "\u{f028}" // fa-volume-up
        };
        self.draw_volume_icon.draw_walk(cx, Walk {
            width: Size::Fixed(14.0),
            ..icon_walk
        }, Align::default(), volume_glyph);

        // Progress bar track (Fill width to take remaining space)
        self.draw_progress_bg.draw_walk(cx, Walk {
            width: Size::fill(),
            height: Size::Fixed(4.0),
            margin: Inset { left: 0.0, right: 4.0, top: 0.0, bottom: 0.0 },
            ..Walk::default()
        });

        // Time text (fixed width so fill doesn't consume its space)
        let time_text = format!(
            "{} / {}",
            Self::format_time_ms(self.current_position_ms),
            Self::format_time_ms(self.total_duration),
        );
        self.draw_time_text.draw_walk(cx, Walk {
            width: Size::Fixed(80.0),
            height: Size::fit(),
            ..Walk::default()
        }, Align::default(), &time_text);

        self.draw_controls_bg.end(cx);

        // Draw progress fill and thumb AFTER end() so the layout alignment is finalized
        let progress_rect = self.draw_progress_bg.area().rect(cx);
        if progress_rect.size.x > 0.0 {
            let progress = if self.total_duration > 0 {
                (self.current_position_ms as f64) / (self.total_duration as f64)
            } else {
                0.0
            };
            let fill_w = progress_rect.size.x * progress.min(1.0);
            self.draw_progress_fill.draw_abs(cx, Rect {
                pos: progress_rect.pos,
                size: dvec2(fill_w, progress_rect.size.y),
            });

            let thumb_size = 10.0;
            self.draw_progress_thumb.draw_abs(cx, Rect {
                pos: dvec2(
                    progress_rect.pos.x + fill_w - thumb_size * 0.5,
                    progress_rect.pos.y + (progress_rect.size.y - thumb_size) * 0.5,
                ),
                size: dvec2(thumb_size, thumb_size),
            });
        }
    }

    fn format_time_ms(ms: u128) -> String {
        let total_seconds = (ms / 1000) as u64;
        let minutes = total_seconds / 60;
        let seconds = total_seconds % 60;
        format!("{:02}:{:02}", minutes, seconds)
    }

    fn draw_seek_indicator(&mut self, cx: &mut Cx2d) {
        let video_rect = self.draw_bg.area().rect(cx);
        if video_rect.size.x <= 0.0 || video_rect.size.y <= 0.0 {
            return;
        }

        let indicator_size = 48.0;
        let center_y = video_rect.pos.y + (video_rect.size.y - indicator_size) * 0.5;

        // Position on left or right half depending on direction
        let center_x = if self.seek_indicator_direction > 0.5 {
            video_rect.pos.x + video_rect.size.x * 0.75 - indicator_size * 0.5
        } else {
            video_rect.pos.x + video_rect.size.x * 0.25 - indicator_size * 0.5
        };

        self.draw_seek_indicator.set_uniform(cx, id!(direction), &[self.seek_indicator_direction as f32]);
        self.draw_seek_indicator.draw_abs(cx, Rect {
            pos: dvec2(center_x, center_y),
            size: dvec2(indicator_size, indicator_size),
        });
    }

    fn seek_to_position_from_x(&mut self, cx: &mut Cx, abs_x: f64) {
        let progress_rect = self.draw_progress_bg.area().rect(cx);
        if progress_rect.size.x <= 0.0 || self.total_duration == 0 {
            return;
        }
        let fraction = ((abs_x - progress_rect.pos.x) / progress_rect.size.x).clamp(0.0, 1.0);
        let position_ms = (fraction * self.total_duration as f64) as u64;
        cx.seek_video_playback(self.id, position_ms);
        self.current_position_ms = position_ms as u128;
        self.seek_cooldown = 5;
        self.redraw(cx);
    }

    fn update_cursor(&self, cx: &mut Cx, abs: Vec2d) {
        if self.should_show_center_play() {
            // Center play button visible — whole area is clickable
            cx.set_cursor(MouseCursor::Hand);
        } else if self.controls_interactable() && self.hit_test_controls(cx, abs) {
            cx.set_cursor(MouseCursor::Hand);
        } else {
            cx.set_cursor(MouseCursor::Arrow);
        }
    }

    fn hit_test_controls(&self, cx: &Cx, abs: Vec2d) -> bool {
        let bar_rect = self.draw_controls_bg.area().rect(cx);
        abs.x >= bar_rect.pos.x && abs.x <= bar_rect.pos.x + bar_rect.size.x
            && abs.y >= bar_rect.pos.y && abs.y <= bar_rect.pos.y + bar_rect.size.y
    }

    fn hit_test_progress_bar(&self, cx: &Cx, abs: Vec2d) -> bool {
        let bar_rect = self.draw_controls_bg.area().rect(cx);
        let progress_rect = self.draw_progress_bg.area().rect(cx);
        // Use full bar height for easier click target, but progress bar x range
        abs.y >= bar_rect.pos.y && abs.y <= bar_rect.pos.y + bar_rect.size.y
            && abs.x >= progress_rect.pos.x && abs.x <= progress_rect.pos.x + progress_rect.size.x
    }

    fn hit_test_controls_click(&mut self, cx: &mut Cx, abs: Vec2d) -> bool {
        if !self.hit_test_controls(cx, abs) {
            return false;
        }

        // Play/pause button — use the draw area from layout
        let play_rect = self.draw_play_icon.area().rect(cx);
        if abs.x >= play_rect.pos.x && abs.x <= play_rect.pos.x + play_rect.size.x {
            self.toggle_play_pause(cx);
            return true;
        }

        // Restart button
        let restart_rect = self.draw_restart_icon.area().rect(cx);
        if abs.x >= restart_rect.pos.x && abs.x <= restart_rect.pos.x + restart_rect.size.x {
            cx.seek_video_playback(self.id, 0);
            self.current_position_ms = 0;
            self.seek_cooldown = 5;
            if self.playback_state == PlaybackState::Completed {
                self.resume_playback(cx);
            }
            self.redraw(cx);
            return true;
        }

        // Volume button
        let volume_rect = self.draw_volume_icon.area().rect(cx);
        if abs.x >= volume_rect.pos.x && abs.x <= volume_rect.pos.x + volume_rect.size.x {
            self.toggle_mute(cx);
            return true;
        }

        // Progress bar region - click to seek
        if self.hit_test_progress_bar(cx, abs) {
            self.seek_to_position_from_x(cx, abs.x);
            return true;
        }

        true // Click was in bar, consume it
    }

    fn toggle_play_pause(&mut self, cx: &mut Cx) {
        match self.playback_state {
            PlaybackState::Playing => self.pause_playback(cx),
            PlaybackState::Paused => self.resume_playback(cx),
            PlaybackState::Unprepared => {
                self.begin_playback(cx);
            }
            PlaybackState::Prepared => {
                cx.begin_video_playback(self.id);
                self.playback_state = PlaybackState::Playing;
                if self.show_idle_thumbnail {
                    self.draw_bg.set_uniform(cx, id!(show_thumbnail), &[0.0]);
                }
            }
            PlaybackState::Completed => {
                cx.seek_video_playback(self.id, 0);
                self.current_position_ms = 0;
                self.seek_cooldown = 5;
                cx.resume_video_playback(self.id);
                self.playback_state = PlaybackState::Playing;
            }
            _ => {}
        }
        self.redraw(cx);
    }

    fn toggle_mute(&mut self, cx: &mut Cx) {
        if self.audio_state == AudioState::Muted {
            self.unmute_playback(cx);
        } else {
            self.mute_playback(cx);
        }
        self.redraw(cx);
    }

    fn load_thumbnail_image(&mut self, cx: &mut Cx) -> bool {
        if let Some(ref handle_ref) = self.thumbnail_source {
            let handle = handle_ref.as_handle();
            if let Some(data) = cx.get_resource(handle) {
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

/// The source of the video data.
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
}
