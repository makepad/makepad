use crate::makepad_draw::{
    audio::{AudioBuffer, AudioDeviceId, AudioDevicesEvent},
    permission::{Permission, PermissionResult, PermissionStatus},
    thread::SignalToUI,
    Cx, CxMediaApi,
};
use makepad_voice::{Segment, VoiceTranscribeParams, VoiceTranscriber};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VOICE_TARGET_SAMPLE_RATE: f64 = 16_000.0;
const VOICE_AUDIO_PACKET_SAMPLES: usize = 320; // 20ms @16k
const VOICE_TRANSCRIBE_MIN_SAMPLES: usize = 16_000 / 2; // ~0.50s
const VOICE_TRANSCRIBE_MAX_SAMPLES: usize = 16_000 * 2; // 2.0s
const VOICE_TRANSCRIBE_PREROLL_SAMPLES: usize = 16_000 / 2; // 500ms
const VOICE_TRANSCRIBE_OVERLAP_SAMPLES: usize = 16_000 / 4; // 250ms carry between chunks
const VOICE_TRIM_TAIL_PAD_SAMPLES: usize = 16_000 / 8; // 125ms safety pad
const VOICE_MAX_PENDING_SAMPLES: usize = 16_000 * 4; // 4.0s backlog cap
const VOICE_SILENCE_RMS_THRESHOLD: f32 = 0.0026;
const VOICE_PAUSE_RMS_THRESHOLD: f32 = 0.0024;
const VOICE_SPEECH_RMS_THRESHOLD: f32 = 0.0030;
const VOICE_PAUSE_PACKETS_TO_FLUSH: usize = 24; // ~480ms
const VOICE_IDLE_TIMEOUT_TICKS_TO_FLUSH: usize = 40; // ~400ms at 10ms poll
const VOICE_MIN_VOICED_SAMPLES_FOR_EARLY_FLUSH: usize = 16_000 / 2; // ~0.50s
const VOICE_NORM_TARGET_RMS: f32 = 0.10; // ~ -20 dBFS RMS
const VOICE_NORM_MAX_GAIN: f32 = 10.0;
const VOICE_NORM_MIN_GAIN: f32 = 0.35;
const VOICE_NORM_PEAK_LIMIT: f32 = 0.98;
const VOICE_NORM_MIN_RMS_FOR_BOOST: f32 = 0.004;

enum VoiceControlMessage {
    Reset,
    Preload,
    Shutdown,
}

pub enum VoiceWaveEvent {
    Append(Vec<f32>),
    Submitted(Vec<f32>),
}

pub struct WindowVoiceInput {
    desired_enabled: bool,
    callback_installed: bool,
    callback_index: Option<usize>,
    default_input: Option<AudioDeviceId>,
    pending_permission_request: Option<i32>,
    capture_enabled: Arc<AtomicBool>,
    callback_state: Arc<Mutex<CaptureCallbackState>>,
    control_tx: mpsc::Sender<VoiceControlMessage>,
    text_rx: Receiver<String>,
    wave_rx: Receiver<VoiceWaveEvent>,
    text_signal: SignalToUI,
}

impl Default for WindowVoiceInput {
    fn default() -> Self {
        let (audio_tx, audio_rx) = mpsc::sync_channel(24);
        let (wave_tx, wave_rx) = mpsc::sync_channel(128);
        let (control_tx, control_rx) = mpsc::channel();
        let (text_tx, text_rx) = mpsc::channel();
        let text_signal = SignalToUI::new();
        let callback_state = Arc::new(Mutex::new(CaptureCallbackState::new(
            audio_tx,
            wave_tx.clone(),
            text_signal.clone(),
        )));
        let capture_enabled = Arc::new(AtomicBool::new(false));
        spawn_voice_worker(
            audio_rx,
            control_rx,
            text_tx,
            wave_tx,
            text_signal.clone(),
        );
        // Keep the backend warm once per app lifetime: worker/model/threadpools stay alive
        // and are not restarted on mic toggles.
        let _ = control_tx.send(VoiceControlMessage::Preload);

        Self {
            desired_enabled: false,
            callback_installed: false,
            callback_index: None,
            default_input: None,
            pending_permission_request: None,
            capture_enabled,
            callback_state,
            control_tx,
            text_rx,
            wave_rx,
            text_signal,
        }
    }
}

impl WindowVoiceInput {
    pub fn ensure_audio_callback(&mut self, cx: &mut Cx, callback_index: usize) {
        if self.callback_installed {
            return;
        }
        let callback_state = self.callback_state.clone();
        let capture_enabled = self.capture_enabled.clone();
        cx.audio_input(callback_index, move |info, input_buffer| {
            if !capture_enabled.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(mut callback_state) = callback_state.try_lock() {
                callback_state.handle_input(info.sample_rate, input_buffer);
            }
        });
        self.callback_installed = true;
        self.callback_index = Some(callback_index);
    }

    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        if enabled {
            self.request_enable(cx);
        } else {
            self.disable(cx);
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.desired_enabled
    }

    pub fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        self.default_input = devices.default_input().into_iter().next();
        if self.desired_enabled {
            self.start_capture(cx);
        }
    }

    pub fn handle_permission_result(&mut self, cx: &mut Cx, result: &PermissionResult) -> bool {
        if result.permission != Permission::AudioInput {
            return false;
        }

        let old_enabled = self.desired_enabled;
        if let Some(request_id) = self.pending_permission_request {
            if request_id != result.request_id {
                return false;
            }
            self.pending_permission_request = None;
        } else {
            return false;
        }

        match result.status {
            PermissionStatus::Granted => {
                if self.desired_enabled {
                    self.start_capture(cx);
                }
            }
            PermissionStatus::DeniedCanRetry
            | PermissionStatus::DeniedPermanent
            | PermissionStatus::NotDetermined => {
                self.desired_enabled = false;
                self.stop_capture(cx);
            }
        }
        old_enabled != self.desired_enabled
    }

    pub fn take_pending_output(&mut self) -> (Vec<String>, Vec<VoiceWaveEvent>) {
        if !self.text_signal.check_and_clear() {
            return (Vec::new(), Vec::new());
        }
        let mut texts = Vec::new();
        while let Ok(text) = self.text_rx.try_recv() {
            texts.push(text);
        }
        let mut waves = Vec::new();
        while let Ok(event) = self.wave_rx.try_recv() {
            waves.push(event);
        }
        (texts, waves)
    }

    pub fn shutdown(&mut self, cx: &mut Cx) {
        self.stop_capture(cx);
        let _ = self.control_tx.send(VoiceControlMessage::Shutdown);
    }

    fn request_enable(&mut self, cx: &mut Cx) {
        self.desired_enabled = true;
        self.pending_permission_request = Some(cx.request_permission(Permission::AudioInput));
        self.start_capture(cx);
    }

    fn disable(&mut self, cx: &mut Cx) {
        self.desired_enabled = false;
        self.pending_permission_request = None;
        self.stop_capture(cx);
    }

    fn reset_pipeline(&mut self) {
        if let Ok(mut callback_state) = self.callback_state.lock() {
            callback_state.reset();
        }
        let _ = self.control_tx.send(VoiceControlMessage::Reset);
    }

    fn start_capture(&mut self, cx: &mut Cx) {
        self.reset_pipeline();
        let _ = self.control_tx.send(VoiceControlMessage::Preload);
        if let Some(device_id) = self.default_input {
            self.capture_enabled.store(true, Ordering::Relaxed);
            cx.use_audio_inputs(&[device_id]);
        } else {
            self.capture_enabled.store(false, Ordering::Relaxed);
            cx.use_audio_inputs(&[]);
        }
    }

    fn stop_capture(&mut self, cx: &mut Cx) {
        self.capture_enabled.store(false, Ordering::Relaxed);
        cx.use_audio_inputs(&[]);
        self.reset_pipeline();
    }
}

impl Drop for WindowVoiceInput {
    fn drop(&mut self) {
        let _ = self.control_tx.send(VoiceControlMessage::Shutdown);
    }
}

struct CaptureCallbackState {
    downsampler: StreamingDownsampler,
    mono_scratch: Vec<f32>,
    resampled_scratch: Vec<f32>,
    pending_16k: VecDeque<f32>,
    audio_tx: SyncSender<Vec<f32>>,
    wave_tx: SyncSender<VoiceWaveEvent>,
    text_signal: SignalToUI,
}

impl CaptureCallbackState {
    fn new(
        audio_tx: SyncSender<Vec<f32>>,
        wave_tx: SyncSender<VoiceWaveEvent>,
        text_signal: SignalToUI,
    ) -> Self {
        Self {
            downsampler: StreamingDownsampler::default(),
            mono_scratch: Vec::new(),
            resampled_scratch: Vec::new(),
            pending_16k: VecDeque::new(),
            audio_tx,
            wave_tx,
            text_signal,
        }
    }

    fn reset(&mut self) {
        self.downsampler.reset();
        self.mono_scratch.clear();
        self.resampled_scratch.clear();
        self.pending_16k.clear();
    }

    fn handle_input(&mut self, source_sample_rate: f64, input_buffer: &AudioBuffer) {
        if input_buffer.frame_count() == 0 || input_buffer.channel_count() == 0 {
            return;
        }

        self.resampled_scratch.clear();
        if input_buffer.channel_count() == 1 {
            self.downsampler.push(
                source_sample_rate,
                input_buffer.channel(0),
                &mut self.resampled_scratch,
            );
        } else {
            self.mono_scratch.resize(input_buffer.frame_count(), 0.0);
            self.mono_scratch.fill(0.0);
            let channel_count = input_buffer.channel_count() as f32;
            for channel in 0..input_buffer.channel_count() {
                let input = input_buffer.channel(channel);
                for (i, sample) in input.iter().enumerate() {
                    self.mono_scratch[i] += *sample;
                }
            }
            for sample in &mut self.mono_scratch {
                *sample /= channel_count;
            }
            self.downsampler
                .push(source_sample_rate, &self.mono_scratch, &mut self.resampled_scratch);
        }

        if self.resampled_scratch.is_empty() {
            return;
        }

        self.pending_16k.extend(self.resampled_scratch.iter().copied());
        while self.pending_16k.len() >= VOICE_AUDIO_PACKET_SAMPLES {
            let mut chunk = Vec::with_capacity(VOICE_AUDIO_PACKET_SAMPLES);
            for _ in 0..VOICE_AUDIO_PACKET_SAMPLES {
                if let Some(sample) = self.pending_16k.pop_front() {
                    chunk.push(sample);
                }
            }
            let _ = self.wave_tx.try_send(VoiceWaveEvent::Append(chunk.clone()));
            self.text_signal.set();
            let _ = self.audio_tx.try_send(chunk);
        }
    }
}

#[derive(Default)]
struct StreamingDownsampler {
    source_sample_rate: f64,
    phase: f64,
    accum_sum: f32,
    accum_count: usize,
}

impl StreamingDownsampler {
    fn reset(&mut self) {
        self.source_sample_rate = 0.0;
        self.phase = 0.0;
        self.accum_sum = 0.0;
        self.accum_count = 0;
    }

    fn push(&mut self, source_sample_rate: f64, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() || source_sample_rate <= 1.0 {
            return;
        }
        if (self.source_sample_rate - source_sample_rate).abs() > 0.5 {
            self.source_sample_rate = source_sample_rate;
            self.phase = 0.0;
            self.accum_sum = 0.0;
            self.accum_count = 0;
        }

        for sample in input {
            self.accum_sum += *sample;
            self.accum_count += 1;
            self.phase += VOICE_TARGET_SAMPLE_RATE;
            while self.phase >= self.source_sample_rate {
                let out_sample = if self.accum_count > 0 {
                    self.accum_sum / self.accum_count as f32
                } else {
                    *sample
                };
                out.push(out_sample);
                self.phase -= self.source_sample_rate;
                self.accum_sum = 0.0;
                self.accum_count = 0;
            }
        }
    }
}

fn spawn_voice_worker(
    audio_rx: Receiver<Vec<f32>>,
    control_rx: Receiver<VoiceControlMessage>,
    text_tx: mpsc::Sender<String>,
    wave_tx: SyncSender<VoiceWaveEvent>,
    text_signal: SignalToUI,
) {
    std::thread::spawn(move || {
        let mut transcriber = VoiceTranscriber::from_makepad_env();
        let params = VoiceTranscribeParams::for_live_dictation();
        crate::log!("voice: backend {:?}", transcriber.kind());

        let mut pending_samples = VecDeque::<f32>::new();
        let mut chunk = Vec::with_capacity(VOICE_TRANSCRIBE_MAX_SAMPLES);
        let mut silence_packet_run = 0usize;
        let mut saw_speech_since_flush = false;
        let mut voiced_samples_since_flush = 0usize;
        let mut idle_timeout_ticks = 0usize;

        'worker: loop {
            while let Ok(control) = control_rx.try_recv() {
                match control {
                    VoiceControlMessage::Reset => {
                        pending_samples.clear();
                        silence_packet_run = 0;
                        saw_speech_since_flush = false;
                        voiced_samples_since_flush = 0;
                        idle_timeout_ticks = 0;
                    }
                    VoiceControlMessage::Preload => {
                        let _ = transcriber.preload(&params);
                    }
                    VoiceControlMessage::Shutdown => break 'worker,
                }
            }

            match audio_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(audio_chunk) => {
                    idle_timeout_ticks = 0;
                    let packet_rms = rms(&audio_chunk);
                    if packet_rms >= VOICE_SPEECH_RMS_THRESHOLD {
                        if !saw_speech_since_flush {
                            trim_pending_to_recent(
                                &mut pending_samples,
                                VOICE_TRANSCRIBE_PREROLL_SAMPLES,
                            );
                        }
                        silence_packet_run = 0;
                        saw_speech_since_flush = true;
                        voiced_samples_since_flush =
                            voiced_samples_since_flush.saturating_add(audio_chunk.len());
                    } else if packet_rms < VOICE_PAUSE_RMS_THRESHOLD {
                        if saw_speech_since_flush {
                            silence_packet_run += 1;
                        }
                    } else {
                        // Mid-band packet: keep phrase active if speech already started.
                        if saw_speech_since_flush {
                            silence_packet_run = 0;
                        }
                    }
                    pending_samples.extend(audio_chunk);
                    if !saw_speech_since_flush {
                        trim_pending_to_recent(&mut pending_samples, VOICE_TRANSCRIBE_PREROLL_SAMPLES);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    idle_timeout_ticks += 1;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if pending_samples.len() > VOICE_MAX_PENDING_SAMPLES {
                let drop_count = pending_samples.len() - VOICE_MAX_PENDING_SAMPLES;
                for _ in 0..drop_count {
                    let _ = pending_samples.pop_front();
                }
            }

            loop {
                let flush_on_hard_limit = saw_speech_since_flush
                    && pending_samples.len() >= VOICE_TRANSCRIBE_MAX_SAMPLES;
                let flush_on_pause = !flush_on_hard_limit
                    && saw_speech_since_flush
                    && silence_packet_run >= VOICE_PAUSE_PACKETS_TO_FLUSH
                    && voiced_samples_since_flush >= VOICE_MIN_VOICED_SAMPLES_FOR_EARLY_FLUSH
                    && pending_samples.len() >= VOICE_TRANSCRIBE_MIN_SAMPLES;
                let flush_on_idle = !flush_on_hard_limit
                    && !flush_on_pause
                    && saw_speech_since_flush
                    && voiced_samples_since_flush >= VOICE_MIN_VOICED_SAMPLES_FOR_EARLY_FLUSH
                    && idle_timeout_ticks >= VOICE_IDLE_TIMEOUT_TICKS_TO_FLUSH
                    && pending_samples.len() >= VOICE_TRANSCRIBE_MIN_SAMPLES;
                if !flush_on_hard_limit && !flush_on_pause && !flush_on_idle {
                    break;
                }
                let flush_reason = if flush_on_hard_limit {
                    "hard_limit"
                } else if flush_on_pause {
                    "pause"
                } else {
                    "idle"
                };

                let flush_len = if flush_on_hard_limit {
                    VOICE_TRANSCRIBE_MAX_SAMPLES
                } else {
                    pending_samples.len()
                };

                chunk.clear();
                for _ in 0..flush_len {
                    if let Some(sample) = pending_samples.pop_front() {
                        chunk.push(sample);
                    }
                }
                if flush_on_hard_limit {
                    prepend_overlap_tail(
                        &mut pending_samples,
                        &chunk,
                        VOICE_TRANSCRIBE_OVERLAP_SAMPLES,
                    );
                }

                if flush_on_pause || flush_on_idle {
                    trim_trailing_silence(&mut chunk);
                    silence_packet_run = 0;
                    saw_speech_since_flush = false;
                    voiced_samples_since_flush = 0;
                    idle_timeout_ticks = 0;
                }

                if chunk.len() < VOICE_TRANSCRIBE_MIN_SAMPLES {
                    continue;
                }
                let chunk_rms = rms(&chunk);
                if chunk_rms < VOICE_SILENCE_RMS_THRESHOLD {
                    continue;
                }

                crate::log!(
                    "voice: submit chunk reason={} len={} rms={:.5} pending_after={}",
                    flush_reason,
                    chunk.len(),
                    chunk_rms,
                    pending_samples.len()
                );
                let normalized_chunk = normalize_for_whisper(&chunk);
                let _ = wave_tx.try_send(VoiceWaveEvent::Submitted(normalized_chunk.clone()));
                text_signal.set();

                let segments = match transcriber.transcribe(&normalized_chunk, &params) {
                    Ok(segments) => segments,
                    Err(_) => Vec::new(),
                };
                let text = normalize_transcript(&segments);
                if !text.is_empty() {
                    crate::log!("voice: transcript {}", text);
                    let _ = text_tx.send(text);
                    text_signal.set();
                }

                // After any submission, require fresh voiced audio before next early flush.
                silence_packet_run = 0;
                saw_speech_since_flush = flush_on_hard_limit;
                voiced_samples_since_flush = if flush_on_hard_limit {
                    VOICE_TRANSCRIBE_OVERLAP_SAMPLES
                } else {
                    0
                };
                idle_timeout_ticks = 0;
            }
        }
    });
}

fn trim_pending_to_recent(samples: &mut VecDeque<f32>, keep: usize) {
    if samples.len() <= keep {
        return;
    }
    let drop_count = samples.len() - keep;
    for _ in 0..drop_count {
        let _ = samples.pop_front();
    }
}

fn prepend_overlap_tail(pending: &mut VecDeque<f32>, chunk: &[f32], overlap: usize) {
    let keep = overlap.min(chunk.len());
    if keep == 0 {
        return;
    }
    let start = chunk.len() - keep;
    for &sample in chunk[start..].iter().rev() {
        pending.push_front(sample);
    }
}

fn trim_trailing_silence(samples: &mut Vec<f32>) {
    let mut keep = samples.len();
    while keep >= VOICE_AUDIO_PACKET_SAMPLES {
        let start = keep - VOICE_AUDIO_PACKET_SAMPLES;
        if rms(&samples[start..keep]) >= VOICE_PAUSE_RMS_THRESHOLD {
            break;
        }
        keep = start;
    }
    keep = (keep + VOICE_TRIM_TAIL_PAD_SAMPLES).min(samples.len());
    let keep = keep.max(VOICE_TRANSCRIBE_MIN_SAMPLES).min(samples.len());
    samples.truncate(keep);
}

fn normalize_transcript(segments: &[Segment]) -> String {
    let mut merged = String::new();
    for segment in segments {
        merged.push_str(&segment.text);
    }
    merged = strip_blank_audio_markers(merged);
    let mut normalized = String::new();
    let mut last_was_space = true;
    for ch in merged.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(ch);
            last_was_space = false;
        }
    }
    let mut trimmed = normalized.trim().to_string();
    if !trimmed.is_empty() {
        trimmed.push(' ');
    }
    trimmed
}

fn strip_blank_audio_markers(mut text: String) -> String {
    for marker in [
        "[BLANK AUDIO]",
        "[BLANK_AUDIO]",
        "[BLANK-AUDIO]",
        "BLANK_AUDIO",
        "BLANK-AUDIO",
    ] {
        text = replace_case_insensitive_all(text, marker, " ");
    }
    text
}

fn replace_case_insensitive_all(mut text: String, pattern: &str, replacement: &str) -> String {
    let pattern_upper = pattern.to_ascii_uppercase();
    loop {
        let upper = text.to_ascii_uppercase();
        let Some(start) = upper.find(&pattern_upper) else {
            break;
        };
        let end = start + pattern.len();
        text.replace_range(start..end, replacement);
    }
    text
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0f32;
    for sample in samples {
        sum += sample * sample;
    }
    (sum / samples.len() as f32).sqrt()
}

fn normalize_for_whisper(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut peak = 0.0f32;
    let mut sum = 0.0f32;
    for &sample in samples {
        let a = sample.abs();
        if a > peak {
            peak = a;
        }
        sum += sample * sample;
    }
    let rms = (sum / samples.len() as f32).sqrt();
    if peak <= 1e-6 {
        return samples.to_vec();
    }

    let gain_peak = VOICE_NORM_PEAK_LIMIT / peak.max(1e-6);
    let gain_rms = if rms >= VOICE_NORM_MIN_RMS_FOR_BOOST {
        VOICE_NORM_TARGET_RMS / rms
    } else {
        1.0
    };
    let gain = gain_rms
        .min(gain_peak)
        .clamp(VOICE_NORM_MIN_GAIN, VOICE_NORM_MAX_GAIN);
    if (gain - 1.0).abs() < 0.01 {
        return samples.to_vec();
    }

    let mut out = Vec::with_capacity(samples.len());
    for &sample in samples {
        out.push((sample * gain).clamp(-1.0, 1.0));
    }
    out
}
