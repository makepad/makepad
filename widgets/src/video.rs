use crate::{
    makepad_derive_widget::*, makepad_draw::*, makepad_platform::event::video_playback::*,
    widget::*,
};

// Currently only supported on Android

// DSL Usage
// source - determines the source for the video playback, can be either:
//  - Network { url: "https://www.someurl.com/video.mkv" }. On Android it supports: HLS, DASH, RTMP, RTSP, and progressive HTTP downloads
//  - Filesystem { path: "/storage/.../DCIM/Camera/video.mp4" }. On Android it requires read permissions that must be granted at runtime.
//  - Dependency { path: dep("crate://self/resources/video.mp4") }. For in-memory videos loaded through LiveDependencies
// is_looping - determines if the video should be played in a loop. defaults to false.
// hold_to_pause - determines if the video should be paused when the user hold the pause button. defaults to false.
// autoplay - determines if the video should start playback when the widget is created. defaults to false.

// Not yet implemented:
// UI
//  - Playback controls
//  - Progress/seek-to bar

// API
//  - Option to restart playback manually when not looping.
//  - Mute/Unmute
//  - Hotswap video source

live_design! {
    VideoBase = {{Video}} {}
}

#[derive(Live, WidgetRef, WidgetSet)]
pub struct Video {
    // Drawing
    #[live]
    draw_bg: DrawColor,
    #[walk]
    walk: Walk,
    #[live]
    layout: Layout,
    #[live]
    scale: f64,

    // Texture
    #[live]
    source: VideoDataSource,
    #[rust]
    texture: Option<Texture>,
    #[rust]
    texture_handle: Option<u32>,

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
    pub fn prepare_playback(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.prepare_playback(cx);
        }
    }

    pub fn begin_playback(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.begin_playback(cx);
        }
    }

    pub fn pause_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pause_playback(cx);
        }
    }

    pub fn resume_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.resume_playback(cx);
        }
    }

    pub fn mute_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.mute_playback(cx);
        }
    }

    pub fn unmute_playback(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.unmute_playback(cx);
        }
    }

    // It will finish playback and cleanup all resources related to playback
    // including data source, decoding threads, object references, etc.
    pub fn stop_and_cleanup_resources(&mut self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop_and_cleanup_resources(cx);
        }
    }

    pub fn set_source(&mut self, source: VideoDataSource) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_source(source);
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
    // Completed is only used when not looping, means playback reached end of stream
    Completed,
    // CleaningUp happens when the platform is called to stop playback and release all resources
    // including data source, object references, decoding threads, etc.
    CleaningUp,
}

#[derive(Default, PartialEq, Debug)]
enum AudioState {
    #[default]
    Playing,
    Muted,
}

impl LiveRegister for Video{
    #[allow(unused)]
    fn live_register(cx: &mut Cx) {
        #[cfg(target_os = "android")]
        {
            register_widget!(cx, Video);
        }
    }
}

impl LiveHook for Video {
    #[allow(unused)]
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.id = LiveId::unique();

        #[cfg(target_os = "android")]
        if self.texture.is_none() {
            let new_texture = Texture::new(cx);
            new_texture.set_format(cx, TextureFormat::VideoRGB);
            self.texture = Some(new_texture);
        }

        let texture = self.texture.as_mut().unwrap();
        self.draw_bg.draw_vars.set_texture(0, &texture);
        self.should_prepare_playback = self.autoplay;
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum VideoAction {
    None,
    PlaybackPrepared,
    PlaybackBegan,
    TextureUpdated,
    PlaybackCompleted,
    PlayerReset
}

impl Widget for Video {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope){
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
                }
                cx.widget_action(uid, &scope.path, VideoAction::TextureUpdated);
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
                if event.texture_id == self.texture.clone().unwrap().texture_id() {
                    self.texture_handle = Some(event.handle);
                    self.maybe_prepare_playback(cx);
                }
            }
            _=>()
        }
        
        self.handle_gestures(cx, event);
        self.handle_activity_events(cx, event);
        self.handle_errors(event);
    }
}

impl Video {

    fn maybe_prepare_playback(&mut self, cx: &mut Cx) {
        if self.playback_state == PlaybackState::Unprepared && self.should_prepare_playback {
            if self.texture_handle.is_none() {
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
                self.texture_handle.unwrap(),
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
            .set_uniform(cx, id!(video_height), &[self.video_height as f32]);
        self.draw_bg
            .set_uniform(cx, id!(video_width), &[self.video_width as f32]);

        if self.mute && self.audio_state != AudioState::Muted {
            cx.mute_video_playback(self.id);
        }

        // Debug
        // log!(
        //     "Video id {} - decoding initialized: \n {}x{}px |",
        //     self.id.0,
        //     self.video_width,
        //     self.video_height,
        // );
    }

    fn handle_gestures(&mut self, cx: &mut Cx, event: &Event) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(_fe) => {
                if self.hold_to_pause {
                    self.pause_playback(cx);
                }
            }
            Hit::FingerUp(_fe) => {
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
        }
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
        if self.playback_state != PlaybackState::Unprepared {
            cx.cleanup_video_playback_resources(self.id);
        }
        self.playback_state = PlaybackState::CleaningUp;
    }

    fn set_source(&mut self, source: VideoDataSource) {
        if self.playback_state == PlaybackState::Unprepared {
            self.source = source;
        } else {
            error!(
                "Attempted to set source while player state is: {:?}",
                self.playback_state
            );
        }
    }
}

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
