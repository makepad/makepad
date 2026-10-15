//! VideoPlayer — a `Video` with the glass `PlayerControls` over it.
//!
//! The one player widget for an app that wants to show a clip and let
//! someone drive it: the picture fits whole (letterboxed or pillarboxed,
//! the `Video` widget's `VideoFit.Contain`), and the controls are wired to
//! the `Video` API here, once, instead of in every app. An app hands it a
//! source with [`VideoPlayerRef::open`] (a path, a `file://` / `content://`
//! URI or an `http(s)` URL) or [`VideoPlayerRef::play_source`], and the
//! person does the rest.
//!
//! The controls are hit-tested before the picture (`EventOrder.Up`), so a
//! tap on the picture is theirs (show/hide, double-tap skip) and the
//! `Video` underneath only ever draws.
use makepad_widgets_core::*;
use crate::{
    player_controls::{PlayerControlsAction, PlayerControlsWidgetExt},
    video::{VideoAction, VideoDataSource, VideoRef, VideoWidgetExt},
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.VideoPlayerBase = #(VideoPlayer::register_widget(vm))

    /** A video with glass transport controls: play/pause, scrub, skip, volume; tap to show or hide them. */
    mod.widgets.VideoPlayer = set_type_default() do mod.widgets.VideoPlayerBase{
        width: Fill
        height: Fill
        flow: Overlay
        show_bg: true
        draw_bg.color: #x000000
        event_order: EventOrder.Up
        video := Video{
            width: Fill
            height: Fill
        }
        controls := PlayerControls{}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VideoPlayer {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    /// What the person asked for: playing or not. The backend's own state
    /// says "not playing" while a stream buffers, which must not flip the
    /// button under the finger.
    #[rust]
    wants_play: bool,
    #[rust(1.0)]
    volume: f64,
    #[rust]
    muted: bool,
    /// A source waiting for the previous clip to finish letting go.
    #[rust]
    pending: Option<VideoDataSource>,
}

impl Widget for VideoPlayer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let mut changed = false;
        if self.pending.is_some() && self.video_ref(cx).is_unprepared() {
            let source = self.pending.take().unwrap();
            self.start(cx, source);
            changed = true;
        }
        if !actions.is_empty() {
            changed |= self.handle_actions(cx, &actions);
            cx.extend_actions(actions);
        }
        match event {
            Event::VideoTextureUpdated(_) | Event::VideoPlaybackPrepared(_) => changed = true,
            _ => {}
        }
        if changed {
            self.sync(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl VideoPlayer {
    fn video_ref(&self, cx: &mut Cx) -> VideoRef {
        self.view.video(cx, ids!(video))
    }

    /// Acts on the controls and the video's own reports; true when the
    /// controls need the new state.
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        let video = self.video_ref(cx);
        let mut changed = false;
        for action in actions.filter_widget_actions_cast::<VideoAction>(video.widget_uid()) {
            match action {
                VideoAction::PlaybackBegan => {
                    self.wants_play = true;
                    changed = true;
                }
                VideoAction::PlaybackCompleted | VideoAction::PlayerReset => {
                    self.wants_play = false;
                    changed = true;
                }
                VideoAction::PlaybackPrepared => changed = true,
                _ => {}
            }
        }
        let controls = self.view.player_controls(cx, ids!(controls));
        for action in controls.actions(actions) {
            changed = true;
            match action {
                PlayerControlsAction::TogglePlay => self.toggle_play(cx),
                PlayerControlsAction::Seek(seconds) => {
                    video.seek_to(cx, (seconds.max(0.0) * 1000.0) as u64);
                }
                PlayerControlsAction::SkipBy(delta) => {
                    let total = video.total_duration_ms() as f64 / 1000.0;
                    let here = video.current_position_ms() as f64 / 1000.0;
                    let mut target = (here + delta).max(0.0);
                    if total > 0.0 {
                        target = target.min(total);
                    }
                    video.seek_to(cx, (target * 1000.0) as u64);
                    controls.set_position(cx, target);
                }
                PlayerControlsAction::Volume(volume) => {
                    self.volume = volume;
                    self.muted = volume <= 0.0;
                    video.set_volume(cx, volume);
                }
                PlayerControlsAction::ToggleMute => {
                    self.muted = !self.muted;
                    if !self.muted && self.volume <= 0.0 {
                        self.volume = 1.0;
                    }
                    video.set_volume(cx, if self.muted { 0.0 } else { self.volume });
                }
                PlayerControlsAction::None => {}
            }
        }
        changed
    }

    fn toggle_play(&mut self, cx: &mut Cx) {
        let video = self.video_ref(cx);
        if self.wants_play {
            video.pause_playback(cx);
            self.wants_play = false;
        } else {
            if video.has_completed() {
                video.seek_to(cx, 0);
            }
            if video.is_paused() || video.has_completed() {
                video.resume_playback(cx);
            } else {
                video.begin_playback(cx);
            }
            self.wants_play = true;
        }
    }

    /// Hands the controls where playback is.
    fn sync(&mut self, cx: &mut Cx) {
        let video = self.video_ref(cx);
        let controls = self.view.player_controls(cx, ids!(controls));
        controls.set_duration(cx, video.total_duration_ms() as f64 / 1000.0);
        controls.set_position(cx, video.current_position_ms() as f64 / 1000.0);
        controls.set_playing(cx, self.wants_play);
        controls.set_volume(cx, if self.muted { 0.0 } else { self.volume }, self.muted);
    }

    /// Plays `source` from the start. A player that already has a clip is
    /// stopped first; the new one starts once the old one has let go.
    pub fn play_source(&mut self, cx: &mut Cx, source: VideoDataSource) {
        let video = self.video_ref(cx);
        if !video.is_unprepared() {
            self.pending = Some(source);
            video.stop_and_cleanup_resources(cx);
            return;
        }
        self.start(cx, source);
    }

    fn start(&mut self, cx: &mut Cx, source: VideoDataSource) {
        let video = self.video_ref(cx);
        video.set_source(source);
        video.begin_playback(cx);
        self.wants_play = true;
        self.sync(cx);
    }

    /// Plays a path, a `file://` or `content://` URI, or an `http(s)` URL.
    pub fn open(&mut self, cx: &mut Cx, src: &str) {
        if let Some(source) = parse_media_source(src) {
            self.play_source(cx, source);
        }
    }

    /// Starts muted (a hidden or test run), or not.
    pub fn set_muted(&mut self, cx: &mut Cx, muted: bool) {
        self.muted = muted;
        let video = self.video_ref(cx);
        video.set_volume(cx, if muted { 0.0 } else { self.volume });
        self.sync(cx);
    }

    pub fn is_playing(&self) -> bool {
        self.wants_play
    }

    /// Plays (or resumes) what is loaded, as the play button does.
    pub fn play(&mut self, cx: &mut Cx) {
        if !self.wants_play {
            self.toggle_play(cx);
        }
        self.sync(cx);
    }

    /// Pauses, as the pause button does.
    pub fn pause(&mut self, cx: &mut Cx) {
        if self.wants_play {
            self.toggle_play(cx);
        }
        self.sync(cx);
    }

    /// Goes to `seconds` in (clamped to the clip once its length is known).
    pub fn seek(&mut self, cx: &mut Cx, seconds: f64) {
        let video = self.video_ref(cx);
        let total = video.total_duration_ms() as f64 / 1000.0;
        let mut target = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        if total > 0.0 {
            target = target.min(total);
        }
        video.seek_to(cx, (target * 1000.0).round() as u64);
        self.view.player_controls(cx, ids!(controls)).set_position(cx, target);
    }

    /// The volume, 0..1, as the slider shows it.
    pub fn volume(&self) -> f64 {
        self.volume
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Sets the volume (0..1); reaching for the volume unmutes.
    pub fn set_volume(&mut self, cx: &mut Cx, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
        self.muted = self.volume <= 0.0;
        let video = self.video_ref(cx);
        video.set_volume(cx, self.volume);
        self.sync(cx);
    }

    /// Shows or hides the controls altogether (a host's "picture only").
    pub fn set_controls_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        self.view.widget(cx, ids!(controls)).set_visible(cx, enabled);
    }

    /// Stops and lets go of the clip; the controls go back to paused.
    pub fn stop(&mut self, cx: &mut Cx) {
        self.pending = None;
        self.wants_play = false;
        self.video_ref(cx).stop_and_cleanup_resources(cx);
        self.sync(cx);
    }
}

impl VideoPlayerRef {
    pub fn open(&self, cx: &mut Cx, src: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx, src);
        }
    }

    pub fn play_source(&self, cx: &mut Cx, source: VideoDataSource) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.play_source(cx, source);
        }
    }

    pub fn set_muted(&self, cx: &mut Cx, muted: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_muted(cx, muted);
        }
    }

    pub fn is_playing(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_playing())
    }

    pub fn play(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.play(cx);
        }
    }

    pub fn pause(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.pause(cx);
        }
    }

    pub fn seek(&self, cx: &mut Cx, seconds: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.seek(cx, seconds);
        }
    }

    pub fn stop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop(cx);
        }
    }

    pub fn volume(&self) -> f64 {
        self.borrow().map_or(1.0, |inner| inner.volume())
    }

    pub fn is_muted(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_muted())
    }

    pub fn set_volume(&self, cx: &mut Cx, volume: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_volume(cx, volume);
        }
    }

    pub fn set_controls_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_controls_enabled(cx, enabled);
        }
    }

    /// The `Video` inside, for what the player does not wrap (tracks, rate).
    pub fn video(&self, cx: &mut Cx) -> VideoRef {
        self.borrow()
            .map(|inner| inner.view.video(cx, ids!(video)))
            .unwrap_or_default()
    }
}

/// A media reference as a [`VideoDataSource`]: `http(s)://` is the network
/// (an `.m3u8`/`.mpd` manifest plays as a stream), `file://` drops to its
/// path (also a Windows `file:///C:/...`), and anything else — a path or a
/// `content://` URI — is the filesystem.
pub fn parse_media_source(src: &str) -> Option<VideoDataSource> {
    let src = src.trim();
    if src.is_empty() {
        return None;
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return Some(VideoDataSource::Network { url: src.to_string() });
    }
    if let Some(rest) = src.strip_prefix("file://") {
        let rest = rest
            .strip_prefix('/')
            .filter(|r| r.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(rest);
        return Some(VideoDataSource::Filesystem { path: rest.to_string() });
    }
    Some(VideoDataSource::Filesystem { path: src.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_of(source: Option<VideoDataSource>) -> (String, bool) {
        match source {
            Some(VideoDataSource::Network { url }) => (url, true),
            Some(VideoDataSource::Filesystem { path }) => (path, false),
            _ => panic!("unexpected source"),
        }
    }

    #[test]
    fn media_references_pick_their_source() {
        assert_eq!(
            path_of(parse_media_source("content://media/external/video/media/42")),
            ("content://media/external/video/media/42".into(), false)
        );
        assert_eq!(path_of(parse_media_source("file:///C:/Videos/clip.mp4")), ("C:/Videos/clip.mp4".into(), false));
        assert_eq!(path_of(parse_media_source("file:///home/a/clip.mp4")), ("/home/a/clip.mp4".into(), false));
        assert_eq!(
            path_of(parse_media_source(" https://example.com/x.m3u8 ")),
            ("https://example.com/x.m3u8".into(), true)
        );
        assert!(parse_media_source("   ").is_none());
    }
}
