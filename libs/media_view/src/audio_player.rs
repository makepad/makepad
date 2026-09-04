//! Audio waveform and transport over Makepad's existing media playback path.

use crate::{media_kind, MediaKind, MediaViewAction};
use makepad_audio_decode::decode_any;
use makepad_widgets::*;
use std::rc::Rc;

const WAVE_BINS: usize = 256;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AudioPlayerBase = #(AudioPlayer::register_widget(vm))
    mod.widgets.AudioPlayer = set_type_default() do mod.widgets.AudioPlayerBase{
        width: Fill
        height: 150
        flow: Overlay
        engine := Video{
            abs_pos: vec2(-1000.0, -1000.0)
            width: 1
            height: 1
            show_controls: false
            autoplay: false
            is_looping: false
        }
        surface := View{
            width: Fill
            height: Fill
            flow: Down
            spacing: 4
            wave := RoundedView{
                width: Fill
                height: Fill
                cursor: MouseCursor.Hand
                show_bg: true
                draw_bg +: {
                    color: #x11151b
                    border_color: #xffffff12
                    border_size: 1.0
                    border_radius: 5.0
                }
            }
            controls := View{
                width: Fill
                height: 26
                flow: Right
                spacing: 6
                align: Align{y: 0.5}
                play := ButtonFlat{
                    width: 34
                    height: 22
                    text: "▶"
                }
                position := Label{
                    width: Fit
                    text: "00:00 / 00:00"
                    draw_text +: {
                        color: #x8a939d
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
            }
        }
        draw_wave +: {color: #x72a7d8}
        draw_playhead +: {color: #xff5a4d}
    }
}

#[derive(Debug)]
struct Pcm {
    samples: Vec<f32>,
    channels: usize,
    rate: u32,
}

impl Pcm {
    fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    fn duration(&self) -> f64 {
        if self.rate == 0 {
            0.0
        } else {
            self.frames() as f64 / self.rate as f64
        }
    }
}

/// Reduce interleaved PCM to min/max amplitude pairs. Every source frame is
/// covered exactly once, and empty bins are represented by silence.
pub fn downsample_waveform(
    samples: &[f32],
    channels: usize,
    bins: usize,
) -> Vec<(f32, f32)> {
    if channels == 0 || bins == 0 {
        return Vec::new();
    }
    let frames = samples.len() / channels;
    if frames == 0 {
        return vec![(0.0, 0.0); bins];
    }
    (0..bins)
        .map(|bin| {
            let start = bin * frames / bins;
            let end = ((bin + 1) * frames / bins).max(start + 1).min(frames);
            if start >= frames {
                return (0.0, 0.0);
            }
            let mut low = 0.0f32;
            let mut high = 0.0f32;
            for frame in start..end {
                for channel in 0..channels {
                    let sample = samples[frame * channels + channel].clamp(-1.0, 1.0);
                    low = low.min(sample);
                    high = high.max(sample);
                }
            }
            (low, high)
        })
        .collect()
}

fn decode_pcm(bytes: &[u8]) -> Result<Pcm, String> {
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        return decode_wav(bytes);
    }
    let decoded = decode_any(bytes).map_err(|error| error.to_string())?;
    if decoded.rate == 0 || decoded.channels == 0 || decoded.pcm_interleaved_f32.is_empty() {
        return Err("decoded audio is empty".into());
    }
    Ok(Pcm {
        samples: decoded.pcm_interleaved_f32,
        channels: decoded.channels as usize,
        rate: decoded.rate,
    })
}

fn decode_wav(bytes: &[u8]) -> Result<Pcm, String> {
    if bytes.len() < 12 || !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF/WAVE file".into());
    }
    let (mut encoding, mut channels, mut rate, mut bits) = (0u16, 0usize, 0u32, 0u16);
    let mut payload = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body_start = at + 8;
        let body_end = body_start
            .checked_add(size)
            .ok_or_else(|| "WAV chunk length overflow".to_string())?
            .min(bytes.len());
        let body = &bytes[body_start..body_end];
        match &bytes[at..at + 4] {
            b"fmt " if body.len() >= 16 => {
                encoding = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap()) as usize;
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => payload = Some(body),
            _ => {}
        }
        at = body_end.saturating_add(size & 1);
    }
    if channels == 0 || rate == 0 {
        return Err("WAV has no usable format chunk".into());
    }
    let payload = payload.ok_or_else(|| "WAV has no data chunk".to_string())?;
    let bytes_per_sample = (bits as usize).div_ceil(8);
    let frame_bytes = bytes_per_sample.saturating_mul(channels);
    if frame_bytes == 0 {
        return Err("WAV has an invalid sample width".into());
    }
    let mut samples = Vec::with_capacity(payload.len() / bytes_per_sample.max(1));
    match (encoding, bits) {
        (1, 8) => samples.extend(payload.iter().map(|sample| (*sample as f32 - 128.0) / 128.0)),
        (1, 16) => samples.extend(
            payload
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0),
        ),
        (1, 24) => samples.extend(payload.chunks_exact(3).map(|sample| {
            let raw = i32::from_le_bytes([
                sample[0],
                sample[1],
                sample[2],
                if sample[2] & 0x80 == 0 { 0 } else { 0xff },
            ]);
            raw as f32 / 8_388_608.0
        })),
        (1, 32) => samples.extend(
            payload
                .chunks_exact(4)
                .map(|sample| i32::from_le_bytes(sample.try_into().unwrap()) as f32 / 2_147_483_648.0),
        ),
        (3, 32) => samples.extend(
            payload
                .chunks_exact(4)
                .map(|sample| f32::from_le_bytes(sample.try_into().unwrap()).clamp(-1.0, 1.0)),
        ),
        _ => return Err(format!("unsupported WAV encoding {encoding}/{bits}-bit")),
    }
    samples.truncate(samples.len() / channels * channels);
    if samples.is_empty() {
        return Err("WAV data chunk is empty".into());
    }
    Ok(Pcm {
        samples,
        channels,
        rate,
    })
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn clamp_seek(seconds: f64, duration: f64) -> f64 {
    if !seconds.is_finite() || !duration.is_finite() || duration <= 0.0 {
        0.0
    } else {
        seconds.clamp(0.0, duration)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AudioPlayer {
    #[deref]
    view: View,
    #[live]
    draw_wave: DrawColor,
    #[live]
    draw_playhead: DrawColor,
    #[rust]
    waveform: Vec<(f32, f32)>,
    #[rust]
    pending_source: Option<Rc<Vec<u8>>>,
    #[rust]
    duration: f64,
    #[rust]
    elapsed: f64,
    #[rust]
    playing: bool,
    #[rust]
    scrubbing: bool,
    #[rust]
    next_frame: NextFrame,
}

impl AudioPlayer {
    fn engine(&self, cx: &Cx) -> VideoRef {
        self.view.video(cx, ids!(engine))
    }

    fn install_source(&mut self, cx: &mut Cx, source: Rc<Vec<u8>>) {
        let engine = self.engine(cx);
        engine.set_source_in_memory(source);
        engine.prepare_playback(cx);
    }

    pub fn load_bytes(
        &mut self,
        cx: &mut Cx,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<(), String> {
        if media_kind(content_type, bytes) != MediaKind::Audio {
            let error = format!("unsupported audio content type: {content_type}");
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            return Err(error);
        }
        let pcm = decode_pcm(bytes).map_err(|error| {
            cx.widget_action(self.widget_uid(), MediaViewAction::Failed(error.clone()));
            error
        })?;
        self.duration = pcm.duration();
        self.elapsed = 0.0;
        self.playing = false;
        self.waveform = downsample_waveform(&pcm.samples, pcm.channels, WAVE_BINS);
        let source = Rc::new(bytes.to_vec());
        let engine = self.engine(cx);
        if engine.is_unprepared() {
            self.install_source(cx, source);
        } else {
            self.pending_source = Some(source);
            engine.stop_and_cleanup_resources(cx);
        }
        self.sync_transport(cx);
        cx.widget_action(self.widget_uid(), MediaViewAction::Loaded(MediaKind::Audio));
        self.view.redraw(cx);
        Ok(())
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.pending_source = None;
        self.waveform.clear();
        self.duration = 0.0;
        self.elapsed = 0.0;
        self.playing = false;
        self.engine(cx).stop_and_cleanup_resources(cx);
        self.sync_transport(cx);
        self.view.redraw(cx);
    }

    pub fn play(&mut self, cx: &mut Cx) {
        let engine = self.engine(cx);
        if engine.has_completed() {
            engine.seek_to(cx, 0);
        }
        if engine.is_paused() {
            engine.resume_playback(cx);
        } else {
            engine.begin_playback(cx);
        }
        self.playing = true;
        self.sync_transport(cx);
        self.next_frame = cx.new_next_frame();
    }

    pub fn pause(&mut self, cx: &mut Cx) {
        self.engine(cx).pause_playback(cx);
        self.playing = false;
        self.sync_transport(cx);
    }

    pub fn seek(&mut self, cx: &mut Cx, seconds: f64) {
        self.elapsed = clamp_seek(seconds, self.duration);
        self.engine(cx)
            .seek_to(cx, (self.elapsed * 1000.0).round() as u64);
        self.sync_transport(cx);
        self.view.redraw(cx);
    }

    pub fn set_size(&mut self, cx: &mut Cx, width: Size, height: Size) {
        self.view.walk.width = width;
        self.view.walk.height = height;
        self.view.redraw(cx);
    }

    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn is_loaded(&self) -> bool {
        !self.waveform.is_empty()
    }

    fn sync_transport(&mut self, cx: &mut Cx) {
        self.view
            .button(cx, ids!(play))
            .set_text(cx, if self.playing { "■" } else { "▶" });
        self.view.label(cx, ids!(position)).set_text(
            cx,
            &format!("{} / {}", format_time(self.elapsed), format_time(self.duration)),
        );
    }

    fn seek_at(&mut self, cx: &mut Cx, x: f64) {
        let rect = self.view.view(cx, ids!(wave)).area().rect(cx);
        if rect.size.x > 1.0 {
            self.seek(cx, ((x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0) * self.duration);
        }
    }
}

impl Widget for AudioPlayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.view.draw_walk(cx, scope, walk).is_step() {}
        let rect = self.view.view(cx, ids!(wave)).area().rect(cx);
        if rect.size.x > 1.0 && rect.size.y > 1.0 && !self.waveform.is_empty() {
            let center = rect.pos.y + rect.size.y * 0.5;
            let width = rect.size.x / self.waveform.len() as f64;
            for (index, (low, high)) in self.waveform.iter().copied().enumerate() {
                let top = center - high.max(0.0) as f64 * rect.size.y * 0.46;
                let bottom = center - low.min(0.0) as f64 * rect.size.y * 0.46;
                self.draw_wave.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(rect.pos.x + index as f64 * width, top),
                        size: dvec2(width.max(1.0), (bottom - top).max(1.0)),
                    },
                );
            }
            let fraction = if self.duration > 0.0 {
                (self.elapsed / self.duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.draw_playhead.draw_abs(
                cx,
                Rect {
                    pos: dvec2(rect.pos.x + rect.size.x * fraction, rect.pos.y),
                    size: dvec2(2.0, rect.size.y),
                },
            );
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(play)).clicked(actions) {
                if self.playing {
                    self.pause(cx);
                } else {
                    self.play(cx);
                }
            }
            if let Some(action) = actions.find_widget_action(self.engine(cx).widget_uid()) {
                match action.cast::<makepad_widgets::VideoAction>() {
                    makepad_widgets::VideoAction::PlaybackCompleted => {
                        self.playing = false;
                        self.elapsed = self.duration;
                        self.sync_transport(cx);
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
        if self.next_frame.is_event(event).is_some() && self.playing {
            self.elapsed = (self.engine(cx).current_position_ms() as f64 / 1000.0)
                .clamp(0.0, self.duration);
            self.sync_transport(cx);
            self.view.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
        let wave = self.view.view(cx, ids!(wave)).area();
        match event.hits(cx, wave) {
            Hit::FingerDown(event) => {
                self.scrubbing = true;
                self.seek_at(cx, event.abs.x);
            }
            Hit::FingerMove(event) if self.scrubbing => self.seek_at(cx, event.abs.x),
            Hit::FingerUp(event) => {
                if self.scrubbing {
                    self.seek_at(cx, event.abs.x);
                }
                self.scrubbing = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_downsampling_keeps_peaks_and_shape() {
        let samples = [-1.0, 0.25, 0.5, 1.0, -0.75, 0.4, 0.2, -0.1];
        assert_eq!(
            downsample_waveform(&samples, 2, 2),
            vec![(-1.0, 1.0), (-0.75, 0.4)]
        );
        assert_eq!(downsample_waveform(&[], 2, 3), vec![(0.0, 0.0); 3]);
        assert!(downsample_waveform(&samples, 0, 3).is_empty());
    }

    #[test]
    fn audio_seek_clamps_and_rejects_non_finite_values() {
        assert_eq!(clamp_seek(-1.0, 10.0), 0.0);
        assert_eq!(clamp_seek(4.0, 10.0), 4.0);
        assert_eq!(clamp_seek(11.0, 10.0), 10.0);
        assert_eq!(clamp_seek(f64::INFINITY, 10.0), 0.0);
    }
}
