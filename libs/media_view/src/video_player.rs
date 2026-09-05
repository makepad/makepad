//! Byte-backed video widget with Makepad's native controls and playback path.

use crate::{media_kind, MediaFit, MediaKind, MediaViewAction};
use makepad_widgets::*;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.VideoPlayerBase = #(VideoPlayer::register_widget(vm))
    mod.widgets.VideoPlayer = set_type_default() do mod.widgets.VideoPlayerBase{
        width: Fill
        height: Fill
        video := Video{
            width: Fill
            height: Fill
            autoplay: false
            is_looping: false
            show_controls: true
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VideoPlayer {
    #[deref]
    view: View,
    #[rust]
    pending_source: Option<Rc<Vec<u8>>>,
    #[rust]
    fit: MediaFit,
    #[rust]
    loaded: bool,
}

impl VideoPlayer {
    fn video(&self, cx: &Cx) -> VideoRef {
        self.view.video(cx, ids!(video))
    }

    fn install_source(&mut self, cx: &mut Cx, source: Rc<Vec<u8>>) {
        let video = self.video(cx);
        video.set_source_in_memory(source);
        video.prepare_playback(cx);
    }

    /// Replace the clip with host-supplied bytes. Decoding stays on the
    /// platform media worker; this call never writes a temporary file.
    pub fn load_bytes(
        &mut self,
        cx: &mut Cx,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<(), String> {
        if media_kind(content_type, bytes) != MediaKind::Video {
            let error = format!("unsupported video content type: {content_type}");
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            return Err(error);
        }
        if bytes.is_empty() {
            let error = "empty video payload".to_string();
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            return Err(error);
        }
        let source = Rc::new(bytes.to_vec());
        let video = self.video(cx);
        self.loaded = false;
        if video.is_unprepared() {
            self.install_source(cx, source);
        } else {
            self.pending_source = Some(source);
            video.stop_and_cleanup_resources(cx);
        }
        self.view.redraw(cx);
        Ok(())
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.pending_source = None;
        self.loaded = false;
        self.video(cx).stop_and_cleanup_resources(cx);
        self.view.redraw(cx);
    }

    pub fn play(&mut self, cx: &mut Cx) {
        let video = self.video(cx);
        if video.is_paused() {
            video.resume_playback(cx);
        } else {
            video.begin_playback(cx);
        }
    }

    pub fn pause(&mut self, cx: &mut Cx) {
        self.video(cx).pause_playback(cx);
    }

    pub fn seek(&mut self, cx: &mut Cx, seconds: f64) {
        let video = self.video(cx);
        let duration = video.total_duration_ms() as f64 / 1000.0;
        let seconds = clamp_seek(seconds, duration);
        video.seek_to(cx, (seconds * 1000.0).round() as u64);
    }

    pub fn set_fit(&mut self, cx: &mut Cx, fit: MediaFit) {
        self.fit = fit;
        // The core Video widget contains by default. Cover/Stretch are kept
        // as typed host intent until its shader exposes those two policies.
        self.view.redraw(cx);
    }

    pub fn set_size(&mut self, cx: &mut Cx, width: Size, height: Size) {
        self.view.walk.width = width;
        self.view.walk.height = height;
        self.view.redraw(cx);
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }
}

pub(crate) fn clamp_seek(seconds: f64, duration: f64) -> f64 {
    if !seconds.is_finite() || duration <= 0.0 || !duration.is_finite() {
        0.0
    } else {
        seconds.clamp(0.0, duration)
    }
}

impl Widget for VideoPlayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let Some(action) = actions.find_widget_action(self.video(cx).widget_uid()) else {
            return;
        };
        match action.cast::<makepad_widgets::VideoAction>() {
            makepad_widgets::VideoAction::PlaybackPrepared => {
                self.loaded = true;
                cx.widget_action(self.widget_uid(), MediaViewAction::Loaded(MediaKind::Video));
            }
            makepad_widgets::VideoAction::PlaybackCompleted => {
                cx.widget_action(self.widget_uid(), MediaViewAction::Ended);
            }
            makepad_widgets::VideoAction::PlayerReset => {
                if let Some(source) = self.pending_source.take() {
                    self.install_source(cx, source);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_is_finite_and_clamped_to_the_clip() {
        assert_eq!(clamp_seek(-4.0, 12.0), 0.0);
        assert_eq!(clamp_seek(3.25, 12.0), 3.25);
        assert_eq!(clamp_seek(99.0, 12.0), 12.0);
        assert_eq!(clamp_seek(f64::NAN, 12.0), 0.0);
        assert_eq!(clamp_seek(2.0, 0.0), 0.0);
    }
}
