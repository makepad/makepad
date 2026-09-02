use crate::makepad_draw::{
    audio::{AudioBuffer, AudioDeviceId, AudioDevicesEvent, AudioInputOptions},
    permission::{Permission, PermissionResult, PermissionStatus},
    thread::SignalToUI,
    Cx, CxMediaApi, Event, NextFrame,
};
use makepad_ai_hub::speech::{Segment, SttConfig, SttEvent, SttSession};
use makepad_ai_speech::vad::{SileroVad, VadStream};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VOICE_TARGET_SAMPLE_RATE: f64 = 16_000.0;
const VOICE_AUDIO_PACKET_SAMPLES: usize = 320; // 20ms @16k
const VOICE_TRANSCRIBE_MIN_SAMPLES: usize = 16_000 / 2; // ~0.50s
const VOICE_TRANSCRIBE_PREROLL_SAMPLES: usize = 16_000 / 2; // 500ms
const VOICE_TRIM_TAIL_PAD_SAMPLES: usize = 16_000 / 8; // 125ms safety pad
const VOICE_MAX_PENDING_SAMPLES: usize = 16_000 * 12; // 12.0s backlog cap
const VOICE_SILENCE_RMS_THRESHOLD: f32 = 0.0026;
const VOICE_PAUSE_RMS_THRESHOLD: f32 = 0.0024;
const VOICE_SPEECH_RMS_THRESHOLD: f32 = 0.0030;
// Silero VAD gate (preferred over the RMS thresholds when the model file is
// present): silero's own defaults — enter speech at 0.5, leave below 0.35.
const VOICE_VAD_SPEECH_PROB: f32 = 0.5;
const VOICE_VAD_PAUSE_PROB: f32 = 0.35;
const VOICE_PAUSE_PACKETS_TO_FLUSH: usize = 24; // ~480ms
const VOICE_IDLE_TIMEOUT_TICKS_TO_FLUSH: usize = 40; // ~400ms at 10ms poll
const VOICE_MIN_VOICED_SAMPLES_FOR_EARLY_FLUSH: usize = 16_000 / 2; // ~0.50s
const VOICE_NORM_TARGET_RMS: f32 = 0.10; // ~ -20 dBFS RMS
const VOICE_NORM_MAX_GAIN: f32 = 10.0;
const VOICE_NORM_MIN_GAIN: f32 = 0.35;
const VOICE_NORM_PEAK_LIMIT: f32 = 0.98;
const VOICE_NORM_MIN_RMS_FOR_BOOST: f32 = 0.004;
const VOICE_WAVE_STEP_SAMPLES: usize = 320;
const VOICE_WAVE_MAX_PENDING_SAMPLES: usize = VOICE_WAVE_STEP_SAMPLES * 64;
const VOICE_ENTER_DELAY_SECS: f64 = 0.075;

enum VoiceControlMessage {
    Reset,
    /// Capture (re)started: an engine that owns the microphone starts listening.
    Start,
    /// Capture stopped: a mic-owning engine stops listening.
    Stop,
    Shutdown,
}

pub enum VoiceWaveEvent {
    Append(Vec<f32>),
    Submitted(Vec<f32>),
    /// Input level 0..1 from an engine that owns the microphone (no samples
    /// reach us in that mode, so this drives the activity glow instead).
    Level(f32),
}

#[derive(Clone, Debug)]
pub enum VoiceInjectEvent {
    Text(String),
    Enter,
}

pub struct WindowVoiceInput {
    desired_enabled: bool,
    callback_installed: bool,
    /// Worker spawn is LAZY (first ensure_audio_callback): every VoiceWave
    /// in the tree (incl. the Window caption one) constructs this struct,
    /// and eager spawns meant duplicate whisper workers/models.
    worker_inputs: Option<(Receiver<Vec<f32>>, Receiver<VoiceControlMessage>, mpsc::Sender<String>, SyncSender<VoiceWaveEvent>)>,
    callback_index: Option<usize>,
    default_input: Option<AudioDeviceId>,
    pending_permission_request: Option<i32>,
    capture_enabled: Arc<AtomicBool>,
    echo_cancellation: bool,
    /// Set by the worker once the recognizer is known to own the microphone
    /// itself (Android / Windows system engines): our own capture must then
    /// stay off — two recorders on one mic is a conflict, not a feature.
    engine_mic: Arc<AtomicBool>,
    callback_state: Arc<Mutex<CaptureCallbackState>>,
    control_tx: mpsc::Sender<VoiceControlMessage>,
    text_rx: Receiver<String>,
    wave_rx: Receiver<VoiceWaveEvent>,
    text_signal: SignalToUI,
    voice_active_until: f64,
    submit_flash_until: f64,
    voice_visual_next_frame: NextFrame,
    voice_wave_pending: VecDeque<f32>,
    pending_inject: VecDeque<VoiceInjectEvent>,
    next_enter_at: f64,
    enter_after_next_text: bool,
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
        let engine_mic = Arc::new(AtomicBool::new(false));

        Self {
            desired_enabled: false,
            callback_installed: false,
            worker_inputs: Some((audio_rx, control_rx, text_tx, wave_tx)),
            callback_index: None,
            default_input: None,
            pending_permission_request: None,
            capture_enabled,
            engine_mic,
            // Constant AEC while capturing: swapping the unit mid-stream to
            // dodge ducking glitches the assistant's own audio start, and
            // standard+Min ducking is gentle enough to live with.
            echo_cancellation: true,
            callback_state,
            control_tx,
            text_rx,
            wave_rx,
            text_signal,
            voice_active_until: 0.0,
            submit_flash_until: 0.0,
            voice_visual_next_frame: NextFrame::default(),
            voice_wave_pending: VecDeque::new(),
            pending_inject: VecDeque::new(),
            next_enter_at: 0.0,
            enter_after_next_text: false,
        }
    }
}

impl WindowVoiceInput {
    fn now_secs() -> f64 {
        Cx::monotonic_now()
    }

    fn chunk_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let mut sum = 0.0f32;
        for sample in samples {
            sum += sample * sample;
        }
        (sum / samples.len() as f32).sqrt()
    }

    pub fn ensure_audio_callback(&mut self, cx: &mut Cx, callback_index: usize) {
        if self.callback_installed {
            return;
        }
        if let Some((audio_rx, control_rx, text_tx, wave_tx)) = self.worker_inputs.take() {
            spawn_voice_worker(
                audio_rx,
                control_rx,
                text_tx,
                wave_tx,
                self.text_signal.clone(),
                self.engine_mic.clone(),
            );
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

    pub fn arm_enter_after_next_transcript(&mut self) {
        self.enter_after_next_text = true;
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
                self.stop_capture_and_reset(cx);
            }
        }
        old_enabled != self.desired_enabled
    }

    pub fn shutdown(&mut self, cx: &mut Cx) {
        self.stop_capture_graceful(cx);
        self.pending_inject.clear();
        self.voice_wave_pending.clear();
        self.next_enter_at = 0.0;
        self.enter_after_next_text = false;
        let _ = self.control_tx.send(VoiceControlMessage::Shutdown);
    }

    fn request_enable(&mut self, cx: &mut Cx) {
        self.desired_enabled = true;
        self.pending_permission_request = Some(cx.request_permission(Permission::AudioInput));
        self.enter_after_next_text = false;
        self.start_capture(cx);
    }

    fn disable(&mut self, cx: &mut Cx) {
        self.desired_enabled = false;
        self.pending_permission_request = None;
        self.stop_capture_graceful(cx);
    }

    fn reset_pipeline(&mut self) {
        if let Ok(mut callback_state) = self.callback_state.lock() {
            callback_state.reset();
        }
        let _ = self.control_tx.send(VoiceControlMessage::Reset);
    }

    fn start_capture(&mut self, cx: &mut Cx) {
        self.reset_pipeline();
        let _ = self.control_tx.send(VoiceControlMessage::Start);
        self.rearm_capture(cx);
    }

    /// (Re)apply device + options for the current capture state. Called on
    /// start and whenever the echo-cancellation need flips.
    fn rearm_capture(&mut self, cx: &mut Cx) {
        if self.engine_mic.load(Ordering::Relaxed) {
            self.capture_enabled.store(false, Ordering::Relaxed);
            cx.use_audio_inputs(&[]);
            return;
        }
        if let Some(device_id) = self.default_input {
            self.capture_enabled.store(true, Ordering::Relaxed);
            cx.use_audio_inputs_with_options(
                &[device_id],
                AudioInputOptions {
                    echo_cancellation: self.echo_cancellation,
                },
            );
        } else {
            self.capture_enabled.store(false, Ordering::Relaxed);
            cx.use_audio_inputs(&[]);
        }
    }

    /// Half-duplex echo control: the app arms the OS voice-processing path
    /// (which DUCKS all other audio) only while its own voice output plays —
    /// idle listening stays on plain capture with music untouched.
    pub fn set_echo_cancellation(&mut self, cx: &mut Cx, on: bool) {
        if self.echo_cancellation == on {
            return;
        }
        self.echo_cancellation = on;
        if self.desired_enabled {
            self.rearm_capture(cx);
        }
    }

    fn stop_capture_graceful(&mut self, cx: &mut Cx) {
        let _ = self.control_tx.send(VoiceControlMessage::Stop);
        self.capture_enabled.store(false, Ordering::Relaxed);
        cx.use_audio_inputs(&[]);
        if let Ok(mut callback_state) = self.callback_state.lock() {
            callback_state.flush_partial_packet();
        }
    }

    fn stop_capture_and_reset(&mut self, cx: &mut Cx) {
        self.stop_capture_graceful(cx);
        self.reset_pipeline();
    }

    fn queue_transcript_parts(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        for part in parse_voice_inject_parts(&text) {
            self.pending_inject.push_back(part);
        }
    }

    fn enqueue_voice_wave_samples(&mut self, samples: &[f32]) {
        self.voice_wave_pending.extend(samples.iter().copied());
        if self.voice_wave_pending.len() > VOICE_WAVE_MAX_PENDING_SAMPLES {
            let drop_count = self.voice_wave_pending.len() - VOICE_WAVE_MAX_PENDING_SAMPLES;
            for _ in 0..drop_count {
                let _ = self.voice_wave_pending.pop_front();
            }
        }
    }

    pub fn drain_ready_inject_events(&mut self, cx: &mut Cx) -> Vec<VoiceInjectEvent> {
        let mut out = Vec::new();

        loop {
            let Some(next) = self.pending_inject.front() else {
                break;
            };
            let now = Self::now_secs();
            match next {
                VoiceInjectEvent::Text(_) => {
                    if let Some(VoiceInjectEvent::Text(text)) = self.pending_inject.pop_front() {
                        out.push(VoiceInjectEvent::Text(text));
                        self.next_enter_at = now + VOICE_ENTER_DELAY_SECS;
                    }
                }
                VoiceInjectEvent::Enter => {
                    if now + 1e-6 < self.next_enter_at {
                        break;
                    }
                    let _ = self.pending_inject.pop_front();
                    out.push(VoiceInjectEvent::Enter);
                    self.next_enter_at = now + VOICE_ENTER_DELAY_SECS;
                }
            }
        }

        if !self.pending_inject.is_empty() {
            self.voice_visual_next_frame = cx.new_next_frame();
        }
        out
    }

    /// Clear pending wave samples and visual timers (used when mic is disabled).
    pub fn clear_pending(&mut self) {
        self.voice_wave_pending.clear();
        self.voice_active_until = 0.0;
        self.submit_flash_until = 0.0;
    }

    /// Take one chunk of pending wave samples for the waveform display.
    /// Returns `None` when no complete chunk is available.
    pub fn take_wave_chunk(&mut self) -> Option<Vec<f32>> {
        if !self.desired_enabled || self.voice_wave_pending.len() < VOICE_WAVE_STEP_SAMPLES {
            return None;
        }
        let mut chunk = Vec::with_capacity(VOICE_WAVE_STEP_SAMPLES);
        for _ in 0..VOICE_WAVE_STEP_SAMPLES {
            if let Some(sample) = self.voice_wave_pending.pop_front() {
                chunk.push(sample);
            }
        }
        if !chunk.is_empty() {
            if WindowVoiceInput::chunk_rms(&chunk) > 0.008 {
                self.voice_active_until = Self::now_secs() + 0.22;
            }
            Some(chunk)
        } else {
            None
        }
    }

    /// Get current visual state without touching the wave widget.
    pub fn visual_state(&self) -> VoiceVisualState {
        let now = Self::now_secs();
        let submit_flash = now < self.submit_flash_until;
        let voice_active = now < self.voice_active_until && !submit_flash;
        VoiceVisualState {
            voice_active,
            submit_flash,
            pending_wave: self.voice_wave_pending.len() >= VOICE_WAVE_STEP_SAMPLES,
            pending_inject: !self.pending_inject.is_empty(),
        }
    }

    /// Schedule a next-frame callback for animation.
    pub fn request_next_frame(&mut self, cx: &mut Cx) {
        self.voice_visual_next_frame = cx.new_next_frame();
    }

    /// Process signal events without touching the wave widget.
    /// Caller should call `take_wave_chunk` / `visual_state` separately.
    pub fn process_signal_no_wave(&mut self, _cx: &mut Cx) -> Vec<VoiceInjectEvent> {
        if !self.text_signal.check_and_clear() {
            return Vec::new();
        }

        if self.engine_mic.load(Ordering::Relaxed) && self.capture_enabled.load(Ordering::Relaxed) {
            self.rearm_capture(_cx);
        }

        let mut text_count = 0usize;
        while let Ok(text) = self.text_rx.try_recv() {
            self.queue_transcript_parts(text);
            text_count += 1;
        }
        if self.enter_after_next_text && text_count > 0 {
            self.pending_inject.push_back(VoiceInjectEvent::Enter);
            self.enter_after_next_text = false;
        }

        while let Ok(event) = self.wave_rx.try_recv() {
            match event {
                VoiceWaveEvent::Append(samples) => {
                    self.enqueue_voice_wave_samples(&samples);
                }
                VoiceWaveEvent::Submitted(_chunk) => {
                    self.submit_flash_until = Self::now_secs() + 0.16;
                    self.voice_active_until = 0.0;
                }
                VoiceWaveEvent::Level(level) => {
                    if level > 0.3 {
                        self.voice_active_until = Self::now_secs() + 0.22;
                    }
                }
            }
        }

        self.drain_ready_inject_events(_cx)
    }

    /// Check if the given event is the timer event we're waiting for.
    pub fn is_timer_event(&self, event: &Event) -> bool {
        self.voice_visual_next_frame.is_event(event).is_some()
    }
}

pub struct VoiceVisualState {
    pub voice_active: bool,
    pub submit_flash: bool,
    pub pending_wave: bool,
    pub pending_inject: bool,
}

impl Drop for WindowVoiceInput {
    fn drop(&mut self) {
        let _ = self.control_tx.send(VoiceControlMessage::Shutdown);
    }
}

fn parse_voice_inject_parts(text: &str) -> Vec<VoiceInjectEvent> {
    let mut out = Vec::new();
    let mut current_text = String::new();
    for raw in text.split_whitespace() {
        let token = raw.trim_matches(|c: char| !c.is_alphanumeric());
        if token.eq_ignore_ascii_case("enter") {
            while current_text
                .chars()
                .last()
                .is_some_and(|ch| ch.is_whitespace() || ch == ',')
            {
                current_text.pop();
            }
            if !current_text.is_empty() {
                out.push(VoiceInjectEvent::Text(std::mem::take(&mut current_text)));
            }
            out.push(VoiceInjectEvent::Enter);
        } else {
            current_text.push_str(raw);
            current_text.push(' ');
        }
    }
    if !current_text.trim().is_empty() {
        out.push(VoiceInjectEvent::Text(current_text));
    }
    out
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

    fn flush_partial_packet(&mut self) {
        if self.pending_16k.is_empty() {
            return;
        }
        let mut chunk = Vec::with_capacity(self.pending_16k.len());
        while let Some(sample) = self.pending_16k.pop_front() {
            chunk.push(sample);
        }
        let _ = self.wave_tx.try_send(VoiceWaveEvent::Append(chunk.clone()));
        self.text_signal.set();
        let _ = self.audio_tx.try_send(chunk);
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
            self.downsampler.push(
                source_sample_rate,
                &self.mono_scratch,
                &mut self.resampled_scratch,
            );
        }

        if self.resampled_scratch.is_empty() {
            return;
        }

        self.pending_16k
            .extend(self.resampled_scratch.iter().copied());
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
    engine_mic: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        // The hub picks the recognizer: Whisper in this process (weights here,
        // machine election), on the machine node, on a LAN node, else the OS
        // engine. Loading happens on the session's own thread and reports
        // through `poll`, so this worker keeps eating audio meanwhile.
        let session = SttSession::start(SttConfig::live_dictation());
        // PCM mode (Whisper, Apple): we gate with VAD and hand over utterances.
        // Engine-mic mode (Android / Windows system recognizers): the engine
        // owns the microphone; we only relay its results.
        let mut engine_owns_mic = false;
        let mut listening_wanted = false;
        let mut engine_ready = false;

        // Learned gate when the Silero weights are present, RMS energy gate
        // otherwise. The VAD stream carries its own 512-sample chunking, so it
        // just eats the 320-sample packets as they come.
        let mut vad = match SileroVad::from_makepad_env() {
            Ok(vad) => {
                crate::log!("voice: gate silero vad");
                Some(VadStream::new(vad))
            }
            Err(err) => {
                crate::log!("voice: gate rms (no silero model: {err:?})");
                None
            }
        };

        let mut pending_samples = VecDeque::<f32>::new();
        let mut chunk = Vec::with_capacity(VOICE_MAX_PENDING_SAMPLES);
        let mut silence_packet_run = 0usize;
        let mut saw_speech_since_flush = false;
        let mut voiced_samples_since_flush = 0usize;
        let mut idle_timeout_ticks = 0usize;

        'worker: loop {
            for event in session.poll() {
                match event {
                    SttEvent::Loading { phase, fraction } => {
                        if fraction == 0.0 || fraction >= 1.0 {
                            crate::log!("voice: {phase}");
                        }
                    }
                    SttEvent::Ready(info) => {
                        crate::log!(
                            "voice: backend {} via {}{} caps={:?}",
                            info.engine,
                            info.pipe,
                            match &info.remote {
                                Some(node) => format!(" on {node}"),
                                None => String::new(),
                            },
                            info.capabilities
                        );
                        engine_ready = true;
                        engine_owns_mic = !info.capabilities.pcm_input && info.capabilities.engine_mic;
                        if engine_owns_mic {
                            engine_mic.store(true, Ordering::Relaxed);
                            // Tell the UI side to release its own capture.
                            text_signal.set();
                            if listening_wanted {
                                session.listen();
                            }
                        }
                    }
                    SttEvent::Failed(why) => {
                        crate::log!("voice: no speech recognizer available: {why}");
                    }
                    SttEvent::Level(level) => {
                        let _ = wave_tx.try_send(VoiceWaveEvent::Level(level));
                        text_signal.set();
                    }
                    SttEvent::Partial(_) => {}
                    SttEvent::Final { transcript, secs, .. } => {
                        let text = normalize_transcript(&transcript.segments);
                        if !text.is_empty() {
                            crate::log!("voice: transcript ({secs:.2}s) {text}");
                            let _ = text_tx.send(text);
                            text_signal.set();
                        }
                    }
                    SttEvent::Error { message, .. } => crate::log!("voice: {message}"),
                    SttEvent::ListenEnded => {
                        // Continuous dictation: the engine ends a session per
                        // utterance; start the next one while the mic is on.
                        if listening_wanted && engine_owns_mic {
                            session.listen();
                        }
                    }
                }
            }

            while let Ok(control) = control_rx.try_recv() {
                match control {
                    VoiceControlMessage::Reset => {
                        pending_samples.clear();
                        silence_packet_run = 0;
                        saw_speech_since_flush = false;
                        voiced_samples_since_flush = 0;
                        idle_timeout_ticks = 0;
                        if let Some(vad) = vad.as_mut() {
                            vad.reset();
                        }
                        session.cancel();
                    }
                    VoiceControlMessage::Start => {
                        listening_wanted = true;
                        if engine_owns_mic && engine_ready {
                            session.listen();
                        }
                    }
                    VoiceControlMessage::Stop => {
                        listening_wanted = false;
                        if engine_owns_mic {
                            session.stop_listening();
                        }
                    }
                    VoiceControlMessage::Shutdown => break 'worker,
                }
            }

            match audio_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(audio_chunk) => {
                    if engine_owns_mic {
                        // Our capture is being released; nothing to gate.
                        continue;
                    }
                    idle_timeout_ticks = 0;
                    // Classify the packet as speech / undecided / pause. Silero
                    // updates its probability every 512 samples, so between
                    // chunk boundaries the newest probability carries over.
                    let (is_speech, is_pause) = match vad.as_mut() {
                        Some(vad) => {
                            vad.push(&audio_chunk);
                            let prob = vad.prob();
                            (prob >= VOICE_VAD_SPEECH_PROB, prob < VOICE_VAD_PAUSE_PROB)
                        }
                        None => {
                            let packet_rms = rms(&audio_chunk);
                            (
                                packet_rms >= VOICE_SPEECH_RMS_THRESHOLD,
                                packet_rms < VOICE_PAUSE_RMS_THRESHOLD,
                            )
                        }
                    };
                    if is_speech {
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
                    } else if is_pause {
                        if saw_speech_since_flush {
                            silence_packet_run += 1;
                        }
                    } else {
                        // Undecided packet: keep the phrase active if speech
                        // already started.
                        if saw_speech_since_flush {
                            silence_packet_run = 0;
                        }
                    }
                    pending_samples.extend(audio_chunk);
                    if !saw_speech_since_flush {
                        trim_pending_to_recent(
                            &mut pending_samples,
                            VOICE_TRANSCRIBE_PREROLL_SAMPLES,
                        );
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
                let flush_on_pause = saw_speech_since_flush
                    && silence_packet_run >= VOICE_PAUSE_PACKETS_TO_FLUSH
                    && voiced_samples_since_flush >= VOICE_MIN_VOICED_SAMPLES_FOR_EARLY_FLUSH
                    && pending_samples.len() >= VOICE_TRANSCRIBE_MIN_SAMPLES;
                let flush_on_idle = !flush_on_pause
                    && saw_speech_since_flush
                    && voiced_samples_since_flush >= VOICE_MIN_VOICED_SAMPLES_FOR_EARLY_FLUSH
                    && idle_timeout_ticks >= VOICE_IDLE_TIMEOUT_TICKS_TO_FLUSH
                    && pending_samples.len() >= VOICE_TRANSCRIBE_MIN_SAMPLES;
                if !flush_on_pause && !flush_on_idle {
                    break;
                }
                let flush_reason = if flush_on_pause { "pause" } else { "idle" };

                let flush_len = pending_samples.len();

                chunk.clear();
                for _ in 0..flush_len {
                    if let Some(sample) = pending_samples.pop_front() {
                        chunk.push(sample);
                    }
                }

                trim_trailing_silence(&mut chunk);
                silence_packet_run = 0;
                saw_speech_since_flush = false;
                voiced_samples_since_flush = 0;
                idle_timeout_ticks = 0;

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
                // Non-blocking: the session recognizes on its own thread and
                // the result comes back through `poll` above, so audio keeps
                // flowing into the gate while Whisper works.
                session.transcribe(normalized_chunk);
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
    merged = strip_noise_markers(merged);
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

fn strip_noise_markers(mut text: String) -> String {
    for marker in [
        "(static)",
        "[static]",
        "<static>",
        "(noise)",
        "[noise]",
        "<noise>",
        "(background noise)",
        "[background noise]",
        "<background noise>",
        "(white noise)",
        "[white noise]",
        "(hiss)",
        "[hiss]",
        "(music)",
        "[music]",
        "<music>",
        "(laughter)",
        "[laughter]",
        "(applause)",
        "[applause]",
        "(inaudible)",
        "[inaudible]",
        "(silence)",
        "[silence]",
        "(crosstalk)",
        "[crosstalk]",
    ] {
        text = replace_case_insensitive_all(text, marker, " ");
    }

    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '♪' | '♫' | '♬' | '♩') {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
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
