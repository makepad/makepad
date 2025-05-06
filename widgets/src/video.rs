use crate::{
    makepad_derive_widget::*, makepad_draw::*, makepad_platform::event::video_playback::*,
    widget::*, image_cache::ImageCacheImpl,
};

live_design! {
    link widgets;
    use link::shaders::*;
    
    pub VideoBase = {{Video}} {}
    pub Video = <VideoBase> {
        width: 100, height: 100
        
        draw_bg: {
            shape: Solid,
            fill: Image
            texture video_texture: textureOES
            texture thumbnail_texture: texture2d
            uniform show_thumbnail: 0.0
            
            instance opacity: 1.0
            instance image_scale: vec2(1.0, 1.0)
            instance image_pan: vec2(0.5, 0.5)
            
            uniform source_size: vec2(1.0, 1.0)
            uniform target_size: vec2(-1.0, -1.0)
            
            fn get_color_scale_pan(self) -> vec4 {
                // Early return for default scaling and panning,
                // used when walk size is not specified or non-fixed.
                if self.target_size.x <= 0.0 && self.target_size.y <= 0.0 {
                    if self.show_thumbnail > 0.0 {
                        return sample2d(self.thumbnail_texture, self.pos).xyzw;
                    } else {
                        return sample2dOES(self.video_texture, self.pos);
                    }  
                }
                
                let scale = self.image_scale;
                let pan = self.image_pan;
                let source_aspect_ratio = self.source_size.x / self.source_size.y;
                let target_aspect_ratio = self.target_size.x / self.target_size.y;
                
                // Adjust scale based on aspect ratio difference
                if (source_aspect_ratio != target_aspect_ratio) {
                    if (source_aspect_ratio > target_aspect_ratio) {
                        scale.x = target_aspect_ratio / source_aspect_ratio;
                        scale.y = 1.0;
                    } else {
                        scale.x = 1.0;
                        scale.y = source_aspect_ratio / target_aspect_ratio;
                    }
                }
                
                // Calculate the range for panning
                let pan_range_x = max(0.0, (1.0 - scale.x));
                let pan_range_y = max(0.0, (1.0 - scale.y));
                
                // Adjust the user pan values to be within the pan range
                let adjusted_pan_x = pan_range_x * pan.x;
                let adjusted_pan_y = pan_range_y * pan.y;
                let adjusted_pan = vec2(adjusted_pan_x, adjusted_pan_y);
                let adjusted_pos = (self.pos * scale) + adjusted_pan;
                
                if self.show_thumbnail > 0.5 {
                    return sample2d(self.thumbnail_texture, adjusted_pos).xyzw;
                } else {
                    return sample2dOES(self.video_texture, adjusted_pos);
                }      
            }
            
            fn pixel(self) -> vec4 {
                let color = self.get_color_scale_pan();
                return Pal::premul(vec4(color.xyz, color.w * self.opacity));
            }
        }
    }
}

/// Currently only supported on Android

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

/// Not yet supported:
/// UI
///  - Playback controls
///  - Progress/seek-to bar

/// Widget API
///  - Seek to timestamp
///  - Option to restart playback manually when not looping.
///  - Hotswap video source, `set_source(VideoDataSource)` only works if video is in Unprepared state.

#[derive(Live, Widget)]
pub struct Video {
    // Drawing
    #[redraw] #[live]
    draw_bg: DrawColor,
    #[walk]
    walk: Walk,
    #[live]
    layout: Layout,
    #[live]
    scale: f64,

    // Textures
    #[live]
    source: VideoDataSource,
    #[rust]
    video_texture: Option<Texture>,
    #[rust]
    video_texture_handle: Option<u32>,
    /// Requires [`show_thumbnail_before_playback`] to be `true`.
    #[live]
    thumbnail_source: Option<LiveDependency>,
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
    show_thumbnail_before_playback: bool,

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

    #[rust]
    id: LiveId,
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
            return inner.playback_state == PlaybackState::Unprepared
        }
        false
    }

    pub fn is_preparing(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Preparing
        }
        false
    }

    pub fn is_prepared(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Prepared
        }
        false
    }
    
    pub fn is_playing(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Playing
        }
        false
    }

    pub fn is_paused(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Paused
        }
        false
    }

    pub fn has_completed(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::Completed
        }
        false
    }

    pub fn is_cleaning_up(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.playback_state == PlaybackState::CleaningUp
        }
        false
    }

    pub fn is_muted(&self) -> bool {
        if let Some(inner) = self.borrow() {
            return inner.audio_state == AudioState::Muted
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

impl LiveHook for Video {
    #[allow(unused)]
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.id = LiveId::unique();

        #[cfg(target_os = "android")]
        {
            if self.video_texture.is_none() {
                let new_texture = Texture::new_with_format(cx, TextureFormat::VideoRGB);
                self.video_texture = Some(new_texture);
            }
            let texture = self.video_texture.as_mut().unwrap();
            self.draw_bg.draw_vars.set_texture(0, &texture);
        }

        #[cfg(not(target_os = "android"))]
        error!("Video Widget is currently only supported on Android.");

        match cx.os_type() {
            OsType::Android(params) => {
                if params.is_emulator {
                    panic!("Video Widget is currently only supported on real devices. (unreliable support for external textures on some emulators hosts)");
                }
            },
            _ => {}
        }
        
        self.should_prepare_playback = self.autoplay;
    }

    fn after_apply(&mut self, cx: &mut Cx, _apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        self.lazy_create_image_cache(cx);
        self.thumbnail_texture = Some(Texture::new(cx));

        let target_w = self.walk.width.fixed_or_zero();
        let target_h = self.walk.height.fixed_or_zero();
        self.draw_bg
            .set_uniform(cx, id!(target_size), &[target_w as f32, target_h as f32]);

        if self.show_thumbnail_before_playback {
            self.load_thumbnail_image(cx);
            self.draw_bg
            .set_uniform(cx, id!(show_thumbnail), &[1.0]);
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum VideoAction {
    None,
    PlaybackPrepared,
    PlaybackBegan,
    TextureUpdated,
    PlaybackCompleted,
    PlayerReset,
    // The video view was secondary clicked (right-clicked) or long-pressed.
    SecondaryClicked {
        abs: DVec2,
        modifiers: KeyModifiers,
    }
}

impl Widget for Video {

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        if let Some(texture) = &self.thumbnail_texture {
            self.draw_bg.draw_vars.set_texture(1, texture);
        }

        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let uid = self.widget_uid();
        match event{
            Event::VideoPlaybackPrepared(event)=> if event.video_id == self.id {
                self.handle_playback_prepared(cx, event);
                cx.widget_action(uid, &scope.path, VideoAction::PlaybackPrepared);
            }
            Event::VideoTextureUpdated(event)=>if event.video_id == self.id {
                self.redraw(cx);
                if self.playback_state == PlaybackState::Prepared {
                    self.playback_state = PlaybackState::Playing;
                    cx.widget_action(uid, &scope.path, VideoAction::PlaybackBegan);
                    self.draw_bg
                    .set_uniform(cx, id!(show_thumbnail), &[0.0]);
                }
                if self.should_dispatch_texture_updates {
                    cx.widget_action(uid, &scope.path, VideoAction::TextureUpdated);
                }
            }
            Event::VideoPlaybackCompleted(event) =>  if event.video_id == self.id {
                if !self.is_looping {
                    self.playback_state = PlaybackState::Completed;
                    cx.widget_action(uid, &scope.path, VideoAction::PlaybackCompleted);
                }
            }
            Event::VideoPlaybackResourcesReleased(event) => if event.video_id == self.id {
                self.playback_state = PlaybackState::Unprepared;
                cx.widget_action(uid, &scope.path, VideoAction::PlayerReset);
            }
            Event::TextureHandleReady(event) => {
                if event.texture_id == self.video_texture.clone().unwrap().texture_id() {
                    self.video_texture_handle = Some(event.handle);
                    self.maybe_prepare_playback(cx);
                }
            }
            _=>()
        }
        
        self.handle_gestures(cx, event, scope);
        self.handle_activity_events(cx, event);
        self.handle_errors(event);
    }
}

impl ImageCacheImpl for Video {
    fn get_texture(&self, _id:usize) -> &Option<Texture> {
        &self.thumbnail_texture
    }

    fn set_texture(&mut self, texture: Option<Texture>, _id:usize) {
        self.thumbnail_texture = texture;
    }
}

impl Video {
    fn maybe_prepare_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Unprepared && self.should_prepare_playback {
            if self.video_texture_handle.is_none() {
                // texture is not yet ready, this method will be called again on TextureHandleReady
                return;
            }

            let source = match &self.source {
                VideoDataSource::Dependency { path } => match cx.get_dependency(path.as_str()) {
                    Ok(data) => VideoSource::InMemory(data),
                    Err(e) => {
                        error!(
                            "Attempted to prepare playback: resource not found {} {}",
                            path.as_str(),
                            e
                        );
                        return;
                    }
                },
                VideoDataSource::Network { url } => VideoSource::Network(url.to_string()),
                VideoDataSource::Filesystem { path } => VideoSource::Filesystem(path.to_string()),
            };

            cx.prepare_video_playback(
                self.id,
                source,
                self.video_texture_handle.unwrap(),
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

        self.draw_bg
            .set_uniform(cx, id!(source_size), &[self.video_width as f32, self.video_height as f32]);

        if self.mute && self.audio_state != AudioState::Muted {
            cx.mute_video_playback(self.id);
        }
    }

    fn handle_gestures(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if self.hold_to_pause {
                    self.pause_playback(cx);
                }
            }
            Hit::FingerDown(fe) if fe.mouse_button().is_some_and(|mb| mb.is_secondary()) => {
                self.handle_secondary_click(cx, scope, fe.abs, fe.modifiers);
            }
            Hit::FingerLongPress(lp) => {
                // TODO: here we could offer some customization, e.g., setting playback speed to 2x.
                // For now, we treat a long press just like a secondary click.
                self.handle_secondary_click(cx, scope, lp.abs, Default::default());
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if self.hold_to_pause {
                    self.resume_playback(cx);
                }
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
        scope: &mut Scope,
        abs: DVec2,
        modifiers: KeyModifiers,
    ) {
        cx.widget_action(
            self.widget_uid(),
            &scope.path,
            VideoAction::SecondaryClicked {
                abs,
                modifiers,
            }
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
        if self.playback_state == PlaybackState::Playing || self.playback_state == PlaybackState::Paused || self.playback_state == PlaybackState::Prepared {
            cx.mute_video_playback(self.id);
            self.audio_state = AudioState::Muted;
        }
    }

    fn unmute_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Playing || self.playback_state == PlaybackState::Paused || self.playback_state == PlaybackState::Prepared
        && self.audio_state == AudioState::Muted {
            cx.unmute_video_playback(self.id);
            self.audio_state = AudioState::Playing;
        }
    }

    fn stop_and_cleanup_resources(&mut self, cx: &mut Cx) {
        if self.playback_state != PlaybackState::Unprepared 
            && self.playback_state != PlaybackState::Preparing
            && self.playback_state != PlaybackState::CleaningUp {
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
                self.id.0,
                self.playback_state
            );
        }
    }

    fn load_thumbnail_image(&mut self, cx: &mut Cx) {
        if let Some(path) = self.thumbnail_source.clone() {
            let path_str = path.as_str();

            if path_str.len() > 0 {
                let _ = self.load_image_dep_by_path(cx, path_str, 0);
            }
        }
    }
}

/// The source of the video data.
/// 
/// [`Dependency`]: The path to a LiveDependency (an asset loaded with `dep("crate://..)`).
/// 
/// [`Network`]: The URL of a video file, it can be any regular HTTP download or HLS, DASH, RTMP, RTSP.
/// 
/// [`Filesystem`]: The path to a video file on the local filesystem. This requires runtime-approved permissions for reading storage.
#[derive(Clone, Debug, Live, LiveHook)]
#[live_ignore]
pub enum VideoDataSource {
    #[live {path: LiveDependency::default()}]
    Dependency { path: LiveDependency },
    #[pick {url: "".to_string()}]
    Network { url: String },
    #[live {path: "".to_string()}]
    Filesystem { path: String },
}
