use makepad_platform::{
    makepad_live_id::{LiveId, live_id},
    event::Event,
    cx_api::CxOsOp,
    audio::AudioSource,
    live_hook::{LiveHook, LiveApply, ApplyFrom, LiveNode},
    derive_live::Live,
    cx::Cx,
    area::Area,
    layout::{Walk, Layout, Align, Padding, Flow},
    draw_2d::{Cx2d, DrawShape, DrawText, Text},
    widget::{Widget, WidgetDraw, WidgetActionItem, WidgetRef, WidgetSet, WidgetActions, ButtonAction, ButtonWidgetRefExt},
    // scope_form_action_cast, // Not standard, remove if not defined elsewhere
    scope::Scope,
    Signal,
    event::{
        AudioPlaybackPreparedEvent,
        AudioPlaybackStartedEvent,
        AudioPlaybackPausedEvent,
        AudioPlaybackStoppedEvent,
        AudioPlaybackCompletedEvent,
        AudioPlaybackErrorEvent,
        AudioPlaybackReleasedEvent,
        // AudioPlaybackTimeUpdateEvent, // Assuming this will be added to platform/src/event/event.rs if used
        finger::{FingerDownEvent, FingerUpEvent, FingerHoverEvent, HoverState},
    },
    vec2,
    Color,
};
use crate::button::Button; // Import the Button widget

#[derive(Live, LiveHook, LiveApply, Widget)]
pub struct AudioPlayer {
    #[live] walk: Walk,
    #[live] layout: Layout,
    #[live] area: Area,

    #[live] player_id: LiveId,

    #[live(AudioSource::default())] source: AudioSource,
    #[live(false)] auto_play: bool,
    #[live(false)] loop_playback: bool,
    #[live(1.0)] initial_volume: f64,
    #[live(false)] initial_mute_state: bool,

    #[rust] is_prepared: bool,
    #[rust] is_playing: bool,
    #[rust] duration_ms: Option<u64>,
    #[rust] current_time_ms: Option<u64>,
    #[rust] current_volume: f64,
    #[rust] current_mute_state: bool,

    // UI Elements
    #[live] play_pause_button: Button,
    #[live] time_label_style: Text,
    #[live] progress_bar_color: Color,
    #[live] progress_bar_bg_color: Color,
    #[live(10.0)] progress_bar_height: f64,
    
    #[rust] hover_state: HoverState,
}

impl LiveHook for AudioPlayer {
    fn before_live_design(cx: &mut Cx) {
        register_widget_factory!(cx, AudioPlayer);
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        if self.player_id.is_empty() {
            self.player_id = LiveId::unique();
        }
        self.current_volume = self.initial_volume.max(0.0).min(1.0);
        self.current_mute_state = self.initial_mute_state;

        if self.source != AudioSource::None && self.auto_play {
            self.request_prepare_playback(cx);
        }
    }
}

impl LiveApply for AudioPlayer {
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut should_re_prepare = false;
        let mut volume_changed = false;
        let mut mute_changed = false;

        if let Some(index) = nodes.child_by_name(index, live_id!(source)) {
            let old_source = self.source.clone();
            self.source.apply(cx, apply_from, index, nodes);
            if self.source != old_source && self.source != AudioSource::None {
                should_re_prepare = true;
            }
        }
        // Other property applications... (auto_play, loop_playback, initial_volume, initial_mute_state)
        // For brevity, assuming these are handled by derive_live or similar logic as before
        // and focusing on the source change logic.

        if should_re_prepare {
            if self.is_prepared || self.is_playing {
                cx.platform_ops.push(CxOsOp::CleanupAudioPlaybackResources(self.player_id));
                self.reset_playback_state();
            }
            if self.source != AudioSource::None { // Only prepare if new source is valid
                self.request_prepare_playback(cx);
            }
        } else {
            // Handle volume/mute changes if they occurred without source change
            // (Assuming initial_volume and initial_mute_state changes are caught here)
            let new_volume = self.initial_volume.max(0.0).min(1.0);
            if self.current_volume != new_volume {
                self.current_volume = new_volume;
                volume_changed = true;
            }
             if self.current_mute_state != self.initial_mute_state {
                self.current_mute_state = self.initial_mute_state;
                mute_changed = true;
            }

            if volume_changed {
                 cx.platform_ops.push(CxOsOp::SetAudioVolume(self.player_id, self.current_volume));
            }
            if mute_changed {
                if self.current_mute_state {
                    cx.platform_ops.push(CxOsOp::MuteAudioPlayback(self.player_id));
                } else {
                    cx.platform_ops.push(CxOsOp::UnmuteAudioPlayback(self.player_id));
                }
            }
        }
        
        self.play_pause_button.apply(cx, apply_from, index, nodes);
        // Apply for other UI elements if they are Live components

        nodes.skip_node(index)
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum AudioPlayerAction {
    None,
    Prepare,
    Play,
    Pause,
    Resume,
    Stop,
    SeekTo(u64), 
    SetVolume(f64),
    SetMute(bool),
    Cleanup,
}

#[derive(Clone, Debug, WidgetAction)]
pub enum AudioPlayerEvent {
    Prepared { duration_ms: u64, can_seek: bool, can_pause: bool, can_set_volume: bool },
    PlaybackStarted,
    PlaybackPaused,
    PlaybackResumed,
    PlaybackStopped,
    PlaybackCompleted,
    TimeUpdate { current_time_ms: u64 },
    Error { message: String },
    Released,
}


impl AudioPlayer {
    fn reset_playback_state(&mut self) {
        self.is_prepared = false;
        self.is_playing = false;
        self.duration_ms = None;
        self.current_time_ms = None;
    }

    fn request_prepare_playback(&mut self, cx: &mut Cx) {
        if self.source == AudioSource::None {
            self.dispatch_event(cx, AudioPlayerEvent::Error {
                message: "Cannot prepare playback: source is None".to_string()
            });
            return;
        }
        
        self.reset_playback_state();
        cx.platform_ops.push(CxOsOp::PrepareAudioPlayback(
            self.player_id,
            self.source.clone(),
            self.auto_play,
            self.loop_playback,
        ));
    }

    fn dispatch_event(&mut self, cx: &mut Cx, event: AudioPlayerEvent) {
        cx.widget_action(self.widget_uid(), &Scope::empty(), event);
    }

    fn format_time(time_ms: Option<u64>) -> String {
        if let Some(ms) = time_ms {
            let total_seconds = ms / 1000;
            let seconds = total_seconds % 60;
            let minutes = total_seconds / 60;
            format!("{:02}:{:02}", minutes, seconds)
        } else {
            "--:--".to_string()
        }
    }
}

impl Widget for AudioPlayer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Handle UI interactions first
        if self.play_pause_button.handle_event(cx, event, scope).has_clicked() {
            if self.is_playing {
                self.dispatch_action(cx, AudioPlayerAction::Pause);
            } else {
                self.dispatch_action(cx, AudioPlayerAction::Play);
            }
        }
        
        // Handle platform events
        match event {
            Event::AudioPlaybackPrepared(e) if e.player_id == self.player_id => {
                self.is_prepared = true;
                self.duration_ms = Some(e.duration_ms as u64);
                self.dispatch_event(cx, AudioPlayerEvent::Prepared { 
                    duration_ms: e.duration_ms as u64,
                    can_seek: e.can_seek,
                    can_pause: e.can_pause,
                    can_set_volume: e.can_set_volume,
                });
                // Auto-play is handled by native if CxOsOp had auto_play=true
                // If native doesn't handle auto_play or if we want explicit Rust control:
                // if self.auto_play && !self.is_playing {
                //     cx.platform_ops.push(CxOsOp::BeginAudioPlayback(self.player_id));
                // }
                self.area.redraw(cx);
            }
            Event::AudioPlaybackStarted(e) if e.player_id == self.player_id => {
                self.is_playing = true;
                self.current_time_ms = Some(self.current_time_ms.unwrap_or(0));
                self.dispatch_event(cx, AudioPlayerEvent::PlaybackStarted);
                self.area.redraw(cx);
            }
            Event::AudioPlaybackPaused(e) if e.player_id == self.player_id => {
                self.is_playing = false;
                self.dispatch_event(cx, AudioPlayerEvent::PlaybackPaused);
                self.area.redraw(cx);
            }
            Event::AudioPlaybackStopped(e) if e.player_id == self.player_id => {
                self.is_playing = false;
                self.current_time_ms = Some(0);
                self.dispatch_event(cx, AudioPlayerEvent::PlaybackStopped);
                self.area.redraw(cx);
            }
            Event::AudioPlaybackCompleted(e) if e.player_id == self.player_id => {
                self.is_playing = false;
                self.current_time_ms = self.duration_ms; // Or 0 if not looping
                self.dispatch_event(cx, AudioPlayerEvent::PlaybackCompleted);
                if self.loop_playback {
                    // Assuming BeginAudioPlayback restarts if completed and looping
                    cx.platform_ops.push(CxOsOp::BeginAudioPlayback(self.player_id));
                }
                self.area.redraw(cx);
            }
            Event::AudioPlaybackError(e) if e.player_id == self.player_id => {
                self.reset_playback_state();
                self.dispatch_event(cx, AudioPlayerEvent::Error { message: e.error.clone() });
                self.area.redraw(cx);
            }
            Event::AudioPlaybackReleased(e) if e.player_id == self.player_id => {
                self.reset_playback_state();
                self.dispatch_event(cx, AudioPlayerEvent::Released);
                self.area.redraw(cx);
            }
            // Handle AudioPlaybackTimeUpdate if implemented
            // Event::AudioPlaybackTimeUpdate(e) if e.player_id == self.player_id => {
            //     self.current_time_ms = Some(e.current_time_ms);
            //     self.dispatch_event(cx, AudioPlayerEvent::TimeUpdate {current_time_ms: e.current_time_ms});
            //     self.area.redraw(cx);
            // }
            _ => {}
        }

        // Handle actions dispatched to this widget
        let actions = cx.widget_actions(self.widget_uid());
        for action in actions {
            match action.as_widget_action().cast() {
                AudioPlayerAction::Prepare => self.request_prepare_playback(cx),
                AudioPlayerAction::Play => {
                    if self.is_prepared {
                        cx.platform_ops.push(CxOsOp::BeginAudioPlayback(self.player_id));
                    } else {
                        self.request_prepare_playback(cx); 
                    }
                }
                AudioPlayerAction::Pause => {
                    if self.is_playing {
                        cx.platform_ops.push(CxOsOp::PauseAudioPlayback(self.player_id));
                    }
                }
                AudioPlayerAction::Resume => {
                    if self.is_prepared { // Resume implies it was prepared and paused
                         cx.platform_ops.push(CxOsOp::ResumeAudioPlayback(self.player_id));
                    }
                }
                AudioPlayerAction::Stop => {
                    cx.platform_ops.push(CxOsOp::StopAudioPlayback(self.player_id));
                }
                AudioPlayerAction::SeekTo(time_ms) => {
                    if self.is_prepared {
                        cx.platform_ops.push(CxOsOp::SeekAudioPlayback(self.player_id, time_ms as f64));
                        self.current_time_ms = Some(time_ms);
                        self.area.redraw(cx);
                    }
                }
                AudioPlayerAction::SetVolume(volume) => {
                    let clamped_volume = volume.max(0.0).min(1.0);
                    self.current_volume = clamped_volume;
                    cx.platform_ops.push(CxOsOp::SetAudioVolume(self.player_id, clamped_volume));
                }
                AudioPlayerAction::SetMute(mute) => {
                    self.current_mute_state = mute;
                    if mute {
                        cx.platform_ops.push(CxOsOp::MuteAudioPlayback(self.player_id));
                    } else {
                        cx.platform_ops.push(CxOsOp::UnmuteAudioPlayback(self.player_id));
                    }
                }
                AudioPlayerAction::Cleanup => {
                    cx.platform_ops.push(CxOsOp::CleanupAudioPlaybackResources(self.player_id));
                }
                AudioPlayerAction::None => {}
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> WidgetDraw {
        // Apply layout and walk
        cx.begin_turtle(walk, self.layout);

        // Draw Play/Pause button
        let button_text = if self.is_playing { "Pause" } else { "Play" };
        self.play_pause_button.set_text(button_text); // Assuming Button has a set_text method
        // If Button doesn't have set_text, you might need to pass it via draw_props or similar
        self.play_pause_button.draw_walk(cx, scope, self.play_pause_button.walk(cx));
        
        // Draw Time Label
        let current_time_str = Self::format_time(self.current_time_ms);
        let duration_str = Self::format_time(self.duration_ms);
        let time_display = format!("{} / {}", current_time_str, duration_str);
        // Use self.time_label_style for drawing text
        // Example: cx.draw_text_walk(&time_display, self.time_label_style.clone(), Walk::default());
        // For now, a simple draw_text_ins call
        self.time_label_style.draw_text_walk(cx, &time_display, Walk::fit()); // Walk::fit() or define specific walk

        // Draw Progress Bar (Simple version)
        let progress_bar_walk = Walk {
            width: crate::layout::Size::Fill, // Fill remaining width
            height: crate::layout::Size::Fixed(self.progress_bar_height),
            margin: crate::layout::Margin { top: 5.0, ..Default::default() }, // Some margin
            ..Default::default()
        };
        let total_width = cx.turtle().padded_width_left(); // Get available width in current turtle layout
        
        // Background
        cx.draw_shape_walk(
            DrawShape::rect(0.0,0.0,total_width, self.progress_bar_height, 0.0),
            self.progress_bar_bg_color,
            progress_bar_walk
        );
        
        // Progress
        if let (Some(current_ms), Some(duration_ms)) = (self.current_time_ms, self.duration_ms) {
            if duration_ms > 0 {
                let progress_ratio = (current_ms as f64 / duration_ms as f64).min(1.0);
                let progress_width = progress_ratio * total_width;
                if progress_width > 0.0 {
                     // Need to draw this on top of the background, so adjust turtle or draw directly
                     // For simplicity, let's assume the turtle is already positioned for the background.
                     // We'll draw the progress bar at the same y, but with calculated width.
                     // This requires careful turtle management or absolute positioning.
                     // A simpler way for now is to use another turtle for the progress part.
                     // However, for direct drawing within the same area:
                    let progress_walk = Walk {
                        width: crate::layout::Size::Fixed(progress_width),
                        height: crate::layout::Size::Fixed(self.progress_bar_height),
                        // No margin for progress part, it sits inside the bg
                        ..Default::default()
                    };
                    // This draw call assumes the turtle is reset or positioned correctly.
                    // This part is tricky with turtle layout and might need absolute positioning or nested turtles.
                    // For a robust solution, a custom DrawQuad within the allocated space is better.
                    // Let's simulate drawing it at the start of the progress bar's area.
                    // This part is simplified and might need adjustments for precise layout.
                     cx.turtle_mut().move_y(-self.progress_bar_height); // Move back up to draw on same line
                     cx.turtle_mut().move_y(progress_bar_walk.margin.top); // Apply its own margin if any
                     cx.draw_shape_walk(
                        DrawShape::rect(0.0,0.0,progress_width, self.progress_bar_height, 0.0),
                        self.progress_bar_color,
                        progress_walk
                    );
                }
            }
        }
        
        cx.end_turtle_with_area(&mut self.area);
        WidgetDraw::done()
    }
}

live_design!{
    import makepad_widgets::audio_player::AudioPlayer;
    import makepad_widgets::button::Button;
    import makepad_draw::text::Text; // Assuming Text is the type for text_style

    AudioPlayer = {{AudioPlayer}} {
        // Default walk/layout values
        walk: {width: Fill, height: Fit}
        layout: {flow: Down, spacing: 10, padding: 10}

        // Default UI element styling
        play_pause_button: <Button> {
            walk: {width: Fit, height: Fit}
            text: "Play" // Initial text
        }
        time_label_style: <Text> {
            font_size: 10.0,
            // color: #000 // Set default color if needed
        }
        progress_bar_color: #00f // Blue
        progress_bar_bg_color: #ccc // Light gray
        progress_bar_height: 10.0
    }
}
