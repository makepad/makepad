//! High-level playback session surface.
//!
//! `makepad-media` supports three playback paths:
//! - native delegated playback for ordinary URL/file sources
//! - direct source-backed playback over a random-access byte source
//! - custom MSE-backed playback for append-buffer ingestion
//!
//! The shared abstraction boundary is the playback session control surface.
//! Decoder, demuxer, and playout internals remain path-specific.

use crate::{
    audio_playout::{AudioPlayoutConfig, MakepadAudioOutputAdapter, PcmAudioPlayoutBuffer},
    direct_media::{DirectByteSourceReader, DirectMediaMachine, DirectMediaPlaybackConfig},
    MediaPlaybackSession, MseDecodedAudioFrame, MseDecodedFrame, MseEngineOutput,
    MseInitMetadata, MsePlaybackEngine, PlaybackPrepared, YuvPlaneData,
};
use makepad_platform::{
    AudioBuffer, AudioInfo, MediaPlaybackSessionId, register_media_playback_session,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

pub use crate::mse_player::SoftwareMsePlaybackEngine;
pub use crate::session_player::VideoFrameSessionPlayer;

pub enum PlaybackKind {
    Native,
    Mse,
    Auto,
}

pub struct SharedMseAppendOutcome {
    pub prepared: Option<PlaybackPrepared>,
    pub input_prepared: Option<PlaybackPrepared>,
    pub input_buffered_ranges: Vec<(f64, f64)>,
    pub buffered_ranges: Vec<(f64, f64)>,
    pub has_video_frames: bool,
}


#[derive(Clone, Debug)]
pub struct SharedMsePlaybackStatus {
    pub prepared: bool,
    pub playing: bool,
    pub active: bool,
    pub current_position_ms: u128,
    pub buffered_ranges: Vec<(f64, f64)>,
    pub reached_eos: bool,
}

#[derive(Clone)]
pub struct SharedMsePlaybackHandle {
    inner: Arc<Mutex<MsePlaybackSession>>,
    prepared_reported: bool,
}

struct SharedMsePlaybackSession {
    inner: Arc<Mutex<MsePlaybackSession>>,
}

pub fn register_direct_media_playback_session(
    reader: DirectByteSourceReader,
    config: DirectMediaPlaybackConfig,
    mime: &str,
) -> Result<MediaPlaybackSessionId, String> {
    let machine = DirectMediaMachine::open(reader, config, mime)?;
    Ok(register_media_playback_session(Box::new(DirectMediaPlaybackSession::new(machine))))
}

impl SharedMsePlaybackHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MsePlaybackSession::new())),
            prepared_reported: false,
        }
    }

    pub fn register_session(&self) -> MediaPlaybackSessionId {
        register_media_playback_session(Box::new(SharedMsePlaybackSession {
            inner: self.inner.clone(),
        }))
    }

    pub fn add_input(&mut self, input_id: u64, engine: Box<dyn MsePlaybackEngine>) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "MSE session lock poisoned".to_string())?;
        inner.add_input(input_id, engine)
    }

    pub fn remove_input(&mut self, input_id: u64) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "MSE session lock poisoned".to_string())?;
        inner.remove_input(input_id)
    }

    pub fn append_data(&mut self, input_id: u64, data: &[u8]) -> Result<SharedMseAppendOutcome, String> {
        let (prepared, input_prepared, input_buffered_ranges, buffered_ranges, has_video_frames) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "MSE session lock poisoned".to_string())?;
            let (input_prepared, input_buffered_ranges) = inner.append_data(input_id, data)?;
            (
                inner.queues.prepared.clone(),
                input_prepared,
                input_buffered_ranges,
                inner.queues.buffered_ranges.clone(),
                !inner.queues.video_queue.is_empty(),
            )
        };
        Ok(self.snapshot(
            prepared,
            input_prepared,
            input_buffered_ranges,
            buffered_ranges,
            has_video_frames,
        ))
    }

    pub fn end_of_stream(&mut self) -> Result<SharedMseAppendOutcome, String> {
        let (prepared, buffered_ranges, has_video_frames) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "MSE session lock poisoned".to_string())?;
            inner.end_of_stream()?;
            (
                inner.queues.prepared.clone(),
                inner.queues.buffered_ranges.clone(),
                !inner.queues.video_queue.is_empty(),
            )
        };
        Ok(self.snapshot(prepared, None, Vec::new(), buffered_ranges, has_video_frames))
    }

    pub fn remove(&mut self, input_id: u64, start: f64, end: f64) -> Result<SharedMseAppendOutcome, String> {
        let (prepared, input_buffered_ranges, buffered_ranges, has_video_frames) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "MSE session lock poisoned".to_string())?;
            let input_buffered_ranges = inner.remove(input_id, start, end)?;
            (
                inner.queues.prepared.clone(),
                input_buffered_ranges,
                inner.queues.buffered_ranges.clone(),
                !inner.queues.video_queue.is_empty(),
            )
        };
        Ok(self.snapshot(
            prepared,
            None,
            input_buffered_ranges,
            buffered_ranges,
            has_video_frames,
        ))
    }

    pub fn set_audio_track(&mut self, index: usize, enabled: bool) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "MSE session lock poisoned".to_string())?;
        inner.set_audio_track(index, enabled);
        Ok(())
    }

    pub fn set_video_track(&mut self, index: usize, selected: bool) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "MSE session lock poisoned".to_string())?;
        inner.set_video_track(index, selected);
        Ok(())
    }

    pub fn status(&self) -> Result<SharedMsePlaybackStatus, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "MSE session lock poisoned".to_string())?;
        Ok(SharedMsePlaybackStatus {
            prepared: inner.queues.prepared.is_some(),
            playing: inner.queues.playing,
            active: inner.queues.active,
            current_position_ms: inner.current_position_ms(),
            buffered_ranges: inner.queues.buffered_ranges.clone(),
            reached_eos: inner.queues.reached_eos,
        })
    }

    fn snapshot(
        &mut self,
        prepared: Option<PlaybackPrepared>,
        input_prepared: Option<PlaybackPrepared>,
        input_buffered_ranges: Vec<(f64, f64)>,
        buffered_ranges: Vec<(f64, f64)>,
        has_video_frames: bool,
    ) -> SharedMseAppendOutcome {
        let prepared = if self.prepared_reported {
            None
        } else {
            if prepared.is_some() {
                self.prepared_reported = true;
            }
            prepared
        };
        SharedMseAppendOutcome {
            prepared,
            input_prepared,
            input_buffered_ranges,
            buffered_ranges,
            has_video_frames,
        }
    }
}

impl MediaPlaybackSession for SharedMsePlaybackSession {
    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        self.inner.lock().ok().and_then(|mut inner| inner.check_prepared())
    }

    fn poll_frame(&mut self) -> bool {
        self.inner.lock().ok().map(|mut inner| inner.poll_frame()).unwrap_or(false)
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.inner.lock().ok().and_then(|mut inner| inner.take_yuv_frame())
    }

    fn check_eos(&mut self) -> bool {
        self.inner.lock().ok().map(|mut inner| inner.check_eos()).unwrap_or(false)
    }

    fn play(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.play();
        }
    }

    fn pause(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pause();
        }
    }

    fn resume(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.resume();
        }
    }

    fn is_playing(&self) -> bool {
        self.inner.lock().ok().map(|inner| inner.is_playing()).unwrap_or(false)
    }

    fn seek_to(&mut self, position_ms: u64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.seek_to(position_ms);
        }
    }

    fn set_volume(&self, volume: f64) {
        if let Ok(inner) = self.inner.lock() {
            inner.set_volume(volume);
        }
    }

    fn current_position_ms(&self) -> u128 {
        self.inner.lock().ok().map(|inner| inner.current_position_ms()).unwrap_or(0)
    }

    fn mute(&self) {
        if let Ok(inner) = self.inner.lock() {
            inner.mute();
        }
    }

    fn unmute(&self) {
        if let Ok(inner) = self.inner.lock() {
            inner.unmute();
        }
    }

    fn set_playback_rate(&self, rate: f64) {
        if let Ok(inner) = self.inner.lock() {
            inner.set_playback_rate(rate);
        }
    }

    fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        self.inner.lock().ok().map(|inner| inner.seekable_ranges()).unwrap_or_default()
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.inner.lock().ok().map(|inner| inner.buffered_ranges()).unwrap_or_default()
    }

    fn fill_audio_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.fill_audio_output(info, output);
        }
    }

    fn is_active(&self) -> bool {
        self.inner.lock().ok().map(|inner| inner.is_active()).unwrap_or(false)
    }

    fn cleanup(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.cleanup();
        }
    }
}

struct PlaybackQueues {
    prepared: Option<PlaybackPrepared>,
    prepared_notified: bool,
    playing: bool,
    active: bool,
    reached_eos: bool,
    eos_notified: bool,
    pending_yuv: Option<YuvPlaneData>,
    video_queue: VecDeque<MseDecodedFrame>,
    audio_queue: Option<PcmAudioPlayoutBuffer>,
    audio_output: Option<MakepadAudioOutputAdapter>,
    audio_output_key: Option<(u32, u8)>,
    buffered_ranges: Vec<(f64, f64)>,
    clock: MediaClock,
}

impl PlaybackQueues {
    fn new() -> Self {
        Self {
            prepared: None,
            prepared_notified: false,
            playing: false,
            active: true,
            reached_eos: false,
            eos_notified: false,
            pending_yuv: None,
            video_queue: VecDeque::new(),
            audio_queue: None,
            audio_output: None,
            audio_output_key: None,
            buffered_ranges: Vec::new(),
            clock: MediaClock::default(),
        }
    }

    fn update_prepared(&mut self, prepared: PlaybackPrepared) -> Result<(), String> {
        self.prepared = Some(prepared);
        Ok(())
    }

    fn push_audio_frames(&mut self, frames: Vec<MseDecodedAudioFrame>) {
        if let Some(audio_queue) = &self.audio_queue {
            for frame in frames {
                let pts_samples = frame.pts_ms.saturating_mul(frame.sample_rate as u64) / 1000;
                let duration_samples = (frame.samples.len() / frame.channels as usize) as u32;
                let _ = audio_queue.push_frame(crate::PcmAudioFrame {
                    pts_samples,
                    duration_samples,
                    sample_rate: frame.sample_rate,
                    channels: frame.channels as u8,
                    samples: frame.samples,
                });
            }
        }
    }

    fn push_video_frames(&mut self, frames: Vec<MseDecodedFrame>) {
        for frame in frames {
            self.video_queue.push_back(frame);
        }
    }

    fn ready_to_present(&self) -> bool {
        let audio_ready = self
            .audio_queue
            .as_ref()
            .map(|queue| queue.has_adequate_buffer())
            .unwrap_or(true);
        let video_ready = !self.video_queue.is_empty() || self.prepared.as_ref().map(|prepared| prepared.video_tracks.is_empty()).unwrap_or(false);
        audio_ready && video_ready
    }

    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        if self.prepared_notified {
            return None;
        }
        let prepared = self.prepared.clone()?;
        self.prepared_notified = true;
        Some(Ok(prepared))
    }

    fn poll_frame(&mut self) -> bool {
        if !self.playing || !self.ready_to_present() {
            return false;
        }
        let now_ms = self.clock.position_ms();
        while let Some(frame) = self.video_queue.front() {
            if frame.pts_ms + 100 < now_ms {
                self.video_queue.pop_front();
                continue;
            }
            if frame.pts_ms <= now_ms + 5 {
                let frame = self.video_queue.pop_front().unwrap();
                self.pending_yuv = Some(frame.yuv);
                return true;
            }
            break;
        }
        false
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.pending_yuv.take()
    }

    fn check_eos(&mut self) -> bool {
        if self.eos_notified {
            return false;
        }
        let audio_drained = self.audio_queue.as_ref().map(|queue| queue.is_drained()).unwrap_or(true);
        if self.reached_eos && self.video_queue.is_empty() && audio_drained {
            self.eos_notified = true;
            return true;
        }
        false
    }

    fn seek_reset(&mut self, position_ms: u64) {
        self.video_queue.retain(|frame| frame.pts_ms >= position_ms);
        if let Some(audio_queue) = &self.audio_queue {
            audio_queue.flush();
        }
        self.pending_yuv = None;
        self.reached_eos = false;
        self.eos_notified = false;
        self.clock.seek(position_ms);
    }

    fn current_position_ms(&self) -> u128 {
        if let Some(audio_queue) = &self.audio_queue {
            if let Some(head) = audio_queue.playout_head_pts_samples() {
                let sample_rate = audio_queue.sample_rate().max(1) as u64;
                return (head.saturating_mul(1000) / sample_rate) as u128;
            }
        }
        self.clock.position_ms() as u128
    }

    fn fill_audio_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        if !self.playing {
            return;
        }
        if let Some(audio_output) = &mut self.audio_output {
            audio_output.fill_output(info, output);
        }
    }

    fn cleanup(&mut self) {
        self.pending_yuv = None;
        self.video_queue.clear();
        if let Some(audio_queue) = &self.audio_queue {
            audio_queue.flush();
        }
        self.audio_output = None;
        self.audio_output_key = None;
        self.active = false;
    }
}

pub struct DirectMediaPlaybackSession {
    machine: DirectMediaMachine,
    queues: PlaybackQueues,
    pending_error: Option<String>,
    prepared_error_reported: bool,
}

impl DirectMediaPlaybackSession {
    fn new(machine: DirectMediaMachine) -> Self {
        let mut queues = PlaybackQueues::new();
        queues.buffered_ranges = machine.buffered_ranges();
        if let Some((sample_rate, channels)) = machine.audio_output_info() {
            let config = AudioPlayoutConfig {
                sample_rate,
                channels,
                frame_samples: 1024,
                ..AudioPlayoutConfig::default()
            };
            if let Ok((queue, adapter)) = PcmAudioPlayoutBuffer::new(config) {
                queues.audio_queue = Some(queue);
                queues.audio_output = Some(adapter);
            }
        }
        Self {
            machine,
            queues,
            pending_error: None,
            prepared_error_reported: false,
        }
    }

    fn pump_machine(&mut self) {
        if self.pending_error.is_some() || !self.queues.active {
            return;
        }
        match self.machine.pump(self.queues.current_position_ms() as u64) {
            Ok(output) => {
                if let Some(prepared) = output.prepared {
                    if let Err(error) = self.queues.update_prepared(prepared) {
                        self.pending_error = Some(error);
                        return;
                    }
                }
                self.queues.push_audio_frames(output.audio_frames);
                self.queues.push_video_frames(output.video_frames);
                self.queues.buffered_ranges = output.buffered_ranges;
                if output.reached_eos {
                    self.queues.reached_eos = true;
                    if let Some(audio_queue) = &self.queues.audio_queue {
                        audio_queue.set_end_of_stream();
                    }
                }
            }
            Err(error) => {
                self.pending_error = Some(error);
            }
        }
    }
}

impl MediaPlaybackSession for DirectMediaPlaybackSession {
    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        self.pump_machine();
        if let Some(error) = self.pending_error.clone() {
            if !self.prepared_error_reported {
                self.prepared_error_reported = true;
                return Some(Err(error));
            }
        }
        self.queues.check_prepared()
    }

    fn poll_frame(&mut self) -> bool {
        self.pump_machine();
        self.queues.poll_frame()
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.queues.take_yuv_frame()
    }

    fn check_eos(&mut self) -> bool {
        self.pump_machine();
        self.queues.check_eos()
    }

    fn play(&mut self) {
        self.queues.playing = true;
        self.queues.clock.play();
    }

    fn pause(&mut self) {
        self.queues.playing = false;
        self.queues.clock.pause();
    }

    fn resume(&mut self) {
        self.queues.playing = true;
        self.queues.clock.play();
    }

    fn is_playing(&self) -> bool {
        self.queues.playing
    }

    fn seek_to(&mut self, position_ms: u64) {
        self.queues.seek_reset(position_ms);
        if let Err(error) = self.machine.seek_to(position_ms) {
            self.pending_error = Some(error);
        }
        self.pump_machine();
    }

    fn set_volume(&self, _volume: f64) {}

    fn current_position_ms(&self) -> u128 {
        self.queues.current_position_ms()
    }

    fn mute(&self) {}
    fn unmute(&self) {}
    fn set_playback_rate(&self, _rate: f64) {}

    fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        self.queues.buffered_ranges.clone()
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.queues.buffered_ranges.clone()
    }

    fn fill_audio_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        self.pump_machine();
        self.queues.fill_audio_output(info, output);
    }

    fn is_active(&self) -> bool {
        self.queues.active
    }

    fn cleanup(&mut self) {
        self.queues.cleanup();
    }
}

pub struct NativePlaybackSession {
    inner: Box<dyn MediaPlaybackSession>,
}

impl NativePlaybackSession {
    pub fn new(inner: Box<dyn MediaPlaybackSession>) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> Box<dyn MediaPlaybackSession> {
        self.inner
    }
}

impl MediaPlaybackSession for NativePlaybackSession {
    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        self.inner.check_prepared()
    }

    fn poll_frame(&mut self) -> bool {
        self.inner.poll_frame()
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.inner.take_yuv_frame()
    }

    fn check_eos(&mut self) -> bool {
        self.inner.check_eos()
    }

    fn play(&mut self) { self.inner.play(); }
    fn pause(&mut self) { self.inner.pause(); }
    fn resume(&mut self) { self.inner.resume(); }
    fn is_playing(&self) -> bool { self.inner.is_playing() }
    fn seek_to(&mut self, position_ms: u64) { self.inner.seek_to(position_ms); }
    fn set_volume(&self, volume: f64) { self.inner.set_volume(volume); }
    fn current_position_ms(&self) -> u128 { self.inner.current_position_ms() }
    fn mute(&self) { self.inner.mute(); }
    fn unmute(&self) { self.inner.unmute(); }
    fn set_playback_rate(&self, rate: f64) { self.inner.set_playback_rate(rate); }
    fn seekable_ranges(&self) -> Vec<(f64, f64)> { self.inner.seekable_ranges() }
    fn buffered_ranges(&self) -> Vec<(f64, f64)> { self.inner.buffered_ranges() }
    fn fill_audio_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        self.inner.fill_audio_output(info, output);
    }
    fn is_active(&self) -> bool { self.inner.is_active() }
    fn cleanup(&mut self) { self.inner.cleanup(); }
}

struct MseInputState {
    engine: Box<dyn MsePlaybackEngine>,
    init: Option<MseInitMetadata>,
    buffered_ranges: Vec<(f64, f64)>,
    reached_eos: bool,
}

#[derive(Clone)]
struct RegisteredVideoTrack {
    input_id: u64,
    track_id: u32,
    codec: String,
    width: u32,
    height: u32,
}

#[derive(Clone)]
struct RegisteredAudioTrack {
    input_id: u64,
    track_id: u32,
    codec: String,
    sample_rate: u32,
    channels: u8,
}

impl MseInputState {
    fn new(engine: Box<dyn MsePlaybackEngine>) -> Self {
        Self {
            engine,
            init: None,
            buffered_ranges: Vec::new(),
            reached_eos: false,
        }
    }
}

fn merge_time_ranges(mut ranges: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    ranges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

pub struct MsePlaybackSession {
    inputs: HashMap<u64, MseInputState>,
    video_tracks: Vec<RegisteredVideoTrack>,
    audio_tracks: Vec<RegisteredAudioTrack>,
    selected_video_track_id: Option<u32>,
    enabled_audio_track_ids: HashSet<u32>,
    queues: PlaybackQueues,
}

impl MsePlaybackSession {
    pub fn new() -> Self {
        Self {
            inputs: HashMap::new(),
            video_tracks: Vec::new(),
            audio_tracks: Vec::new(),
            selected_video_track_id: None,
            enabled_audio_track_ids: HashSet::new(),
            queues: PlaybackQueues::new(),
        }
    }

    pub fn add_input(&mut self, input_id: u64, engine: Box<dyn MsePlaybackEngine>) -> Result<(), String> {
        if self.inputs.contains_key(&input_id) {
            return Ok(());
        }
        self.inputs.insert(input_id, MseInputState::new(engine));
        self.refresh_session_state()?;
        Ok(())
    }

    pub fn remove_input(&mut self, input_id: u64) -> Result<(), String> {
        let Some(mut input) = self.inputs.remove(&input_id) else {
            return Ok(());
        };
        input.engine.cleanup();
        self.refresh_session_state()?;
        Ok(())
    }

    pub fn append_data(
        &mut self,
        input_id: u64,
        data: &[u8],
    ) -> Result<(Option<PlaybackPrepared>, Vec<(f64, f64)>), String> {
        let output = self
            .inputs
            .get_mut(&input_id)
            .ok_or_else(|| format!("missing MSE input {input_id}"))?
            .engine
            .append_data(data)?;
        self.consume_engine_output(input_id, output)
    }

    pub fn end_of_stream(&mut self) -> Result<(), String> {
        let input_ids: Vec<u64> = self.inputs.keys().copied().collect();
        for input_id in input_ids {
            let output = self
                .inputs
                .get_mut(&input_id)
                .ok_or_else(|| format!("missing MSE input {input_id}"))?
                .engine
                .end_of_stream()?;
            let _ = self.consume_engine_output(input_id, output)?;
        }
        self.queues.reached_eos = self.inputs.values().all(|input| input.reached_eos);
        if self.queues.reached_eos {
            if let Some(audio_queue) = &self.queues.audio_queue {
                audio_queue.set_end_of_stream();
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, input_id: u64, start: f64, end: f64) -> Result<Vec<(f64, f64)>, String> {
        let input_buffered_ranges = {
            let input = self
                .inputs
                .get_mut(&input_id)
                .ok_or_else(|| format!("missing MSE input {input_id}"))?;
            input.engine.remove(start, end);
            input.buffered_ranges = input.engine.buffered_ranges();
            input.reached_eos = false;
            input.buffered_ranges.clone()
        };
        self.drop_removed_range_from_queues(input_id, start, end);
        self.refresh_session_state()?;
        Ok(input_buffered_ranges)
    }

    pub fn set_audio_track(&mut self, index: usize, enabled: bool) {
        let Some(track) = self.audio_tracks.get(index) else {
            return;
        };
        if enabled {
            self.enabled_audio_track_ids.insert(track.track_id);
        } else {
            self.enabled_audio_track_ids.remove(&track.track_id);
        }
        self.apply_track_selection_to_queues(true);
    }

    pub fn set_video_track(&mut self, index: usize, selected: bool) {
        let Some(track) = self.video_tracks.get(index) else {
            return;
        };
        if selected {
            self.selected_video_track_id = Some(track.track_id);
        } else if self.selected_video_track_id == Some(track.track_id) {
            self.selected_video_track_id = None;
        }
        self.apply_track_selection_to_queues(true);
    }

    fn consume_engine_output(
        &mut self,
        input_id: u64,
        output: MseEngineOutput,
    ) -> Result<(Option<PlaybackPrepared>, Vec<(f64, f64)>), String> {
        let input_prepared = output.init.as_ref().map(|init| {
            PlaybackPrepared::new(
                init.video_tracks.first().map(|track| track.width).unwrap_or(0),
                init.video_tracks.first().map(|track| track.height).unwrap_or(0),
                init.duration_ms,
                init.duration_ms > 0,
                init.video_tracks.iter().map(|track| track.codec.clone()).collect(),
                init.audio_tracks.iter().map(|track| track.codec.clone()).collect(),
            )
        });
        let input_buffered_ranges = {
            let input = self
                .inputs
                .get_mut(&input_id)
                .ok_or_else(|| format!("missing MSE input {input_id}"))?;
            if let Some(init) = output.init {
                input.init = Some(init);
            }
            input.buffered_ranges = output.buffered_ranges;
            input.reached_eos = output.reached_eos;
            input.buffered_ranges.clone()
        };

        self.refresh_session_state()?;
        let video_frames = output
            .video_frames
            .into_iter()
            .filter(|frame| self.video_frame_is_selected(frame.track_id))
            .collect();
        let audio_frames = output
            .audio_frames
            .into_iter()
            .filter(|frame| self.audio_frame_is_enabled(frame.track_id))
            .collect();
        self.queues.push_video_frames(video_frames);
        self.queues.push_audio_frames(audio_frames);
        Ok((input_prepared, input_buffered_ranges))
    }

    fn refresh_session_state(&mut self) -> Result<(), String> {
        let merged_ranges = merge_time_ranges(
            self.inputs
                .values()
                .flat_map(|input| input.buffered_ranges.iter().copied())
                .collect(),
        );
        self.queues.buffered_ranges = merged_ranges;
        self.queues.reached_eos = !self.inputs.is_empty() && self.inputs.values().all(|input| input.reached_eos);

        let mut video_tracks = Vec::new();
        let mut audio_tracks = Vec::new();
        let mut inits: Vec<MseInitMetadata> = Vec::new();
        let mut input_ids: Vec<u64> = self.inputs.keys().copied().collect();
        input_ids.sort_unstable();
        for input_id in input_ids {
            let Some(input) = self.inputs.get(&input_id) else {
                continue;
            };
            if let Some(init) = input.init.as_ref() {
                inits.push(init.clone());
                video_tracks.extend(init.video_tracks.iter().map(|track| RegisteredVideoTrack {
                    input_id,
                    track_id: track.track_id,
                    codec: track.codec.clone(),
                    width: track.width,
                    height: track.height,
                }));
                audio_tracks.extend(init.audio_tracks.iter().map(|track| RegisteredAudioTrack {
                    input_id,
                    track_id: track.track_id,
                    codec: track.codec.clone(),
                    sample_rate: track.sample_rate,
                    channels: track.channels as u8,
                }));
            }
        }
        self.video_tracks = video_tracks;
        self.audio_tracks = audio_tracks;
        self.reconcile_track_selection();

        if inits.is_empty() {
            self.queues.prepared = None;
            self.queues.prepared_notified = false;
            self.queues.audio_queue = None;
            self.queues.audio_output = None;
            self.queues.audio_output_key = None;
            return Ok(());
        }

        let width = self
            .video_tracks
            .iter()
            .map(|track| track.width)
            .next()
            .unwrap_or(0);
        let height = self
            .video_tracks
            .iter()
            .map(|track| track.height)
            .next()
            .unwrap_or(0);
        let duration_ms = inits.iter().map(|init| init.duration_ms).max().unwrap_or(0);
        let video_track_names = self.video_tracks.iter().map(|track| track.codec.clone()).collect();
        let audio_track_names: Vec<String> = self.audio_tracks.iter().map(|track| track.codec.clone()).collect();
        self.queues.update_prepared(PlaybackPrepared::new(
            width,
            height,
            duration_ms,
            duration_ms > 0,
            video_track_names,
            audio_track_names,
        ))?;

        if let Some(track) = self.enabled_audio_track() {
            let audio_output_key = (track.sample_rate, track.channels);
            if self.queues.audio_output_key != Some(audio_output_key) {
                let config = AudioPlayoutConfig {
                    sample_rate: track.sample_rate,
                    channels: track.channels,
                    frame_samples: 1024,
                    ..AudioPlayoutConfig::default()
                };
                let (queue, adapter) = PcmAudioPlayoutBuffer::new(config).map_err(|err| err.to_string())?;
                self.queues.audio_queue = Some(queue);
                self.queues.audio_output = Some(adapter);
                self.queues.audio_output_key = Some(audio_output_key);
            }
        } else {
            self.queues.audio_queue = None;
            self.queues.audio_output = None;
            self.queues.audio_output_key = None;
        }

        self.apply_track_selection_to_queues(false);

        if self.queues.reached_eos {
            if let Some(audio_queue) = &self.queues.audio_queue {
                audio_queue.set_end_of_stream();
            }
        }

        Ok(())
    }

    fn reconcile_track_selection(&mut self) {
        if !self
            .video_tracks
            .iter()
            .any(|track| Some(track.track_id) == self.selected_video_track_id)
        {
            self.selected_video_track_id = self.video_tracks.first().map(|track| track.track_id);
        }

        let valid_audio_track_ids: HashSet<u32> = self.audio_tracks.iter().map(|track| track.track_id).collect();
        self.enabled_audio_track_ids
            .retain(|track_id| valid_audio_track_ids.contains(track_id));
        if self.enabled_audio_track_ids.is_empty() {
            if let Some(track) = self.audio_tracks.first() {
                self.enabled_audio_track_ids.insert(track.track_id);
            }
        }
    }

    fn enabled_audio_track(&self) -> Option<&RegisteredAudioTrack> {
        self.audio_tracks
            .iter()
            .find(|track| self.enabled_audio_track_ids.contains(&track.track_id))
    }

    fn video_frame_is_selected(&self, track_id: u32) -> bool {
        self.selected_video_track_id.is_none_or(|selected| selected == track_id)
    }

    fn audio_frame_is_enabled(&self, track_id: u32) -> bool {
        self.enabled_audio_track_ids.is_empty() || self.enabled_audio_track_ids.contains(&track_id)
    }

    fn apply_track_selection_to_queues(&mut self, reset_playback_state: bool) {
        let selected_video_track_id = self.selected_video_track_id;
        self.queues.video_queue.retain(|frame| {
            selected_video_track_id.is_none_or(|selected| selected == frame.track_id)
        });
        self.queues.pending_yuv = None;
        if let Some(audio_queue) = &self.queues.audio_queue {
            audio_queue.flush();
        }
        if reset_playback_state {
            self.queues.reached_eos = false;
            self.queues.eos_notified = false;
        }
    }

    fn drop_removed_range_from_queues(&mut self, input_id: u64, start: f64, end: f64) {
        let start_ms = (start * 1000.0).max(0.0) as u64;
        let end_ms = (end * 1000.0).max(start * 1000.0) as u64;
        let removed_video_track_ids: HashSet<u32> = self
            .video_tracks
            .iter()
            .filter(|track| track.input_id == input_id)
            .map(|track| track.track_id)
            .collect();
        self.queues.video_queue.retain(|frame| {
            !(removed_video_track_ids.contains(&frame.track_id) && frame.pts_ms >= start_ms && frame.pts_ms < end_ms)
        });
        let removed_audio = self.audio_tracks.iter().any(|track| track.input_id == input_id);
        if removed_audio {
            if let Some(audio_queue) = &self.queues.audio_queue {
                audio_queue.flush();
            }
        }
        self.queues.pending_yuv = None;
        self.queues.reached_eos = false;
        self.queues.eos_notified = false;
    }
}

impl MediaPlaybackSession for MsePlaybackSession {
    fn check_prepared(&mut self) -> Option<Result<PlaybackPrepared, String>> {
        self.queues.check_prepared()
    }

    fn poll_frame(&mut self) -> bool {
        self.queues.poll_frame()
    }

    fn take_yuv_frame(&mut self) -> Option<YuvPlaneData> {
        self.queues.take_yuv_frame()
    }

    fn check_eos(&mut self) -> bool {
        self.queues.check_eos()
    }

    fn play(&mut self) {
        self.queues.playing = true;
        self.queues.clock.play();
    }

    fn pause(&mut self) {
        self.queues.playing = false;
        self.queues.clock.pause();
    }

    fn resume(&mut self) {
        self.queues.playing = true;
        self.queues.clock.play();
    }

    fn is_playing(&self) -> bool {
        self.queues.playing
    }

    fn seek_to(&mut self, position_ms: u64) {
        self.queues.seek_reset(position_ms);
        for input in self.inputs.values_mut() {
            input.engine.flush();
        }
    }

    fn set_volume(&self, _volume: f64) {}

    fn current_position_ms(&self) -> u128 {
        self.queues.current_position_ms()
    }

    fn mute(&self) {}
    fn unmute(&self) {}
    fn set_playback_rate(&self, _rate: f64) {}

    fn seekable_ranges(&self) -> Vec<(f64, f64)> {
        self.queues.buffered_ranges.clone()
    }

    fn buffered_ranges(&self) -> Vec<(f64, f64)> {
        self.queues.buffered_ranges.clone()
    }

    fn fill_audio_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        self.queues.fill_audio_output(info, output);
    }

    fn is_active(&self) -> bool {
        self.queues.active
    }

    fn cleanup(&mut self) {
        self.queues.cleanup();
        for input in self.inputs.values_mut() {
            input.engine.cleanup();
        }
    }
}

/// Session-owned playback clock.
///
/// This is currently wall-clock driven. It is not yet backed by a device
/// playout head. The audio queue is used for buffering policy, not for sink
/// timing authority.
#[derive(Clone, Debug)]
struct MediaClock {
    anchor_ms: u64,
    started_at: Option<Instant>,
}

impl Default for MediaClock {
    fn default() -> Self {
        Self {
            anchor_ms: 0,
            started_at: None,
        }
    }
}

impl MediaClock {
    fn play(&mut self) {
        if self.started_at.is_none() {
            self.started_at = Some(Instant::now());
        }
    }

    fn pause(&mut self) {
        if let Some(started_at) = self.started_at.take() {
            self.anchor_ms = self.anchor_ms.saturating_add(started_at.elapsed().as_millis() as u64);
        }
    }

    fn seek(&mut self, position_ms: u64) {
        self.anchor_ms = position_ms;
        if self.started_at.is_some() {
            self.started_at = Some(Instant::now());
        }
    }

    fn position_ms(&self) -> u64 {
        self.position_ms_from_anchor(self.anchor_ms)
    }

    fn position_ms_from_anchor(&self, anchor_ms: u64) -> u64 {
        match self.started_at {
            Some(started_at) => anchor_ms.saturating_add(started_at.elapsed().as_millis() as u64),
            None => anchor_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_platform::{
        AudioBuffer, AudioInfo, MseAudioTrackInfo, MseDecodedAudioFrame, MseInitMetadata,
        MseVideoTrackInfo, take_registered_media_playback_session,
    };

    struct StubEngine {
        outputs: VecDeque<MseEngineOutput>,
        flushed: bool,
    }

    impl MsePlaybackEngine for StubEngine {
        fn append_data(&mut self, _data: &[u8]) -> Result<MseEngineOutput, String> {
            Ok(self.outputs.pop_front().unwrap_or_default())
        }
        fn end_of_stream(&mut self) -> Result<MseEngineOutput, String> {
            Ok(self.outputs.pop_front().unwrap_or(MseEngineOutput { reached_eos: true, ..Default::default() }))
        }
        fn remove(&mut self, _start: f64, _end: f64) {}
        fn buffered_ranges(&self) -> Vec<(f64, f64)> { Vec::new() }
        fn flush(&mut self) { self.flushed = true; }
        fn cleanup(&mut self) {}
    }

    fn yuv_frame(pts_ms: u64) -> MseDecodedFrame {
        MseDecodedFrame {
            track_id: 1,
            pts_ms,
            yuv: YuvPlaneData {
                y: vec![16; 4],
                u: vec![128; 1],
                v: vec![128; 1],
                width: 2,
                height: 2,
                layout: crate::YuvLayout::I420,
                matrix: crate::YuvColorMatrix::BT709,
            },
        }
    }

    #[test]
    fn session_prepares_and_buffers_track_aware_batches() {
        let init = MseInitMetadata {
            duration_ms: 1_000,
            video_tracks: vec![MseVideoTrackInfo { track_id: 1, codec: "avc1".into(), width: 2, height: 2, config: vec![] }],
            audio_tracks: vec![MseAudioTrackInfo { track_id: 2, codec: "mp4a.40.2".into(), sample_rate: 48_000, channels: 2, config: vec![0x11, 0x90] }],
        };
        let mut session = MsePlaybackSession::new();
        session.add_input(1, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(init),
                audio_frames: vec![MseDecodedAudioFrame { track_id: 2, pts_ms: 0, sample_rate: 48_000, channels: 2, samples: vec![0.0; 960 * 2] }],
                video_frames: vec![yuv_frame(0)],
                buffered_ranges: vec![(0.0, 1.0)],
                reached_eos: false,
            }]),
            flushed: false,
        })).unwrap();

        session.append_data(1, &[1]).unwrap();
        let prepared = session.check_prepared().unwrap().unwrap();
        assert_eq!(prepared.width, 2);
        assert_eq!(prepared.audio_tracks, vec!["mp4a.40.2".to_string()]);
        assert_eq!(session.buffered_ranges(), vec![(0.0, 1.0)]);
    }

    #[test]
    fn session_seek_flushes_engine_and_queues() {
        let mut session = MsePlaybackSession::new();
        session.add_input(1, Box::new(StubEngine { outputs: VecDeque::new(), flushed: false })).unwrap();
        session.seek_to(250);
        assert_eq!(session.current_position_ms(), 250);
    }

    #[test]
    fn session_audio_output_advances_clock_from_playout_head() {
        let init = MseInitMetadata {
            duration_ms: 1_000,
            video_tracks: vec![],
            audio_tracks: vec![MseAudioTrackInfo {
                track_id: 2,
                codec: "mp4a.40.2".into(),
                sample_rate: 48_000,
                channels: 2,
                config: vec![0x11, 0x90],
            }],
        };
        let mut session = MsePlaybackSession::new();
        session.add_input(2, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(init),
                audio_frames: vec![MseDecodedAudioFrame {
                    track_id: 2,
                    pts_ms: 0,
                    sample_rate: 48_000,
                    channels: 2,
                    samples: vec![0.25; 4096 * 2],
                }],
                video_frames: vec![],
                buffered_ranges: vec![(0.0, 0.1)],
                reached_eos: false,
            }]),
            flushed: false,
        })).unwrap();

        session.append_data(2, &[1]).unwrap();
        session.play();
        let mut output = AudioBuffer::new_with_size(480, 2);
        session.fill_audio_output(
            AudioInfo {
                device_id: Default::default(),
                time: None,
                sample_rate: 48_000.0,
            },
            &mut output,
        );
        assert!(output.data.iter().any(|sample| sample.abs() > 0.0));
        assert!(session.current_position_ms() >= 10);
    }

    #[test]
    fn session_aggregates_multiple_inputs_structurally() {
        let mut session = MsePlaybackSession::new();
        session.add_input(10, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(MseInitMetadata {
                    duration_ms: 1_000,
                    video_tracks: vec![MseVideoTrackInfo {
                        track_id: 1,
                        codec: "avc1".into(),
                        width: 2,
                        height: 2,
                        config: vec![],
                    }],
                    audio_tracks: vec![],
                }),
                audio_frames: vec![],
                video_frames: vec![yuv_frame(0)],
                buffered_ranges: vec![(0.0, 0.5)],
                reached_eos: false,
            }]),
            flushed: false,
        })).unwrap();
        session.add_input(20, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(MseInitMetadata {
                    duration_ms: 1_000,
                    video_tracks: vec![],
                    audio_tracks: vec![MseAudioTrackInfo {
                        track_id: 2,
                        codec: "mp4a.40.2".into(),
                        sample_rate: 48_000,
                        channels: 2,
                        config: vec![0x11, 0x90],
                    }],
                }),
                audio_frames: vec![],
                video_frames: vec![],
                buffered_ranges: vec![(0.25, 1.0)],
                reached_eos: false,
            }]),
            flushed: false,
        })).unwrap();

        assert_eq!(session.append_data(10, &[1]).unwrap().1, vec![(0.0, 0.5)]);
        assert_eq!(session.append_data(20, &[2]).unwrap().1, vec![(0.25, 1.0)]);
        let prepared = session.check_prepared().unwrap().unwrap();
        assert_eq!(prepared.video_tracks, vec!["avc1".to_string()]);
        assert_eq!(prepared.audio_tracks, vec!["mp4a.40.2".to_string()]);
        assert_eq!(session.buffered_ranges(), vec![(0.0, 1.0)]);
    }

    #[test]
    fn session_video_track_selection_filters_frames() {
        let mut session = MsePlaybackSession::new();
        session.add_input(1, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(MseInitMetadata {
                    duration_ms: 1_000,
                    video_tracks: vec![MseVideoTrackInfo {
                        track_id: 11,
                        codec: "avc1-a".into(),
                        width: 2,
                        height: 2,
                        config: vec![],
                    }],
                    audio_tracks: vec![],
                }),
                audio_frames: vec![],
                video_frames: vec![MseDecodedFrame { track_id: 11, ..yuv_frame(0) }],
                buffered_ranges: vec![(0.0, 1.0)],
                reached_eos: false,
            }]),
            flushed: false,
        })).unwrap();
        session.add_input(2, Box::new(StubEngine {
            outputs: VecDeque::from([
                MseEngineOutput {
                    init: Some(MseInitMetadata {
                        duration_ms: 1_000,
                        video_tracks: vec![MseVideoTrackInfo {
                            track_id: 22,
                            codec: "avc1-b".into(),
                            width: 2,
                            height: 2,
                            config: vec![],
                        }],
                        audio_tracks: vec![],
                    }),
                    audio_frames: vec![],
                    video_frames: vec![MseDecodedFrame { track_id: 22, ..yuv_frame(0) }],
                    buffered_ranges: vec![(0.0, 1.0)],
                    reached_eos: false,
                },
                MseEngineOutput {
                    init: None,
                    audio_frames: vec![],
                    video_frames: vec![MseDecodedFrame { track_id: 22, ..yuv_frame(0) }],
                    buffered_ranges: vec![(0.0, 1.0)],
                    reached_eos: false,
                },
            ]),
            flushed: false,
        })).unwrap();

        session.append_data(1, &[1]).unwrap();
        session.append_data(2, &[2]).unwrap();
        assert_eq!(session.video_tracks.len(), 2);
        assert_eq!(session.selected_video_track_id, Some(11));
        assert_eq!(session.queues.video_queue.len(), 1);
        assert_eq!(session.queues.video_queue.front().map(|frame| frame.track_id), Some(11));

        session.set_video_track(1, true);
        assert_eq!(session.selected_video_track_id, Some(22));
        assert!(session.queues.video_queue.is_empty());
        session.append_data(2, &[3]).unwrap();
        assert_eq!(session.queues.video_queue.len(), 1);
        assert_eq!(session.queues.video_queue.front().map(|frame| frame.track_id), Some(22));
    }

    #[test]
    fn session_remove_clears_queued_frames_and_resets_eos() {
        let mut session = MsePlaybackSession::new();
        session.add_input(1, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(MseInitMetadata {
                    duration_ms: 1_000,
                    video_tracks: vec![MseVideoTrackInfo {
                        track_id: 11,
                        codec: "avc1".into(),
                        width: 2,
                        height: 2,
                        config: vec![],
                    }],
                    audio_tracks: vec![],
                }),
                audio_frames: vec![],
                video_frames: vec![
                    MseDecodedFrame { track_id: 11, ..yuv_frame(0) },
                    MseDecodedFrame { track_id: 11, ..yuv_frame(300) },
                ],
                buffered_ranges: vec![(0.0, 1.0)],
                reached_eos: true,
            }]),
            flushed: false,
        })).unwrap();

        session.append_data(1, &[1]).unwrap();
        assert_eq!(session.queues.video_queue.len(), 2);
        assert!(session.queues.reached_eos);
        session.remove(1, 0.0, 0.2).unwrap();
        assert_eq!(session.queues.video_queue.len(), 1);
        assert_eq!(session.queues.video_queue.front().map(|frame| frame.pts_ms), Some(300));
        assert!(!session.queues.reached_eos);
        assert!(!session.queues.eos_notified);
    }

    #[test]
    fn shared_handle_feeds_registered_session() {
        let init = MseInitMetadata {
            duration_ms: 1_000,
            video_tracks: vec![MseVideoTrackInfo {
                track_id: 1,
                codec: "avc1".into(),
                width: 2,
                height: 2,
                config: vec![],
            }],
            audio_tracks: vec![],
        };
        let mut handle = SharedMsePlaybackHandle::new();
        handle.add_input(1, Box::new(StubEngine {
            outputs: VecDeque::from([MseEngineOutput {
                init: Some(init),
                audio_frames: vec![],
                video_frames: vec![yuv_frame(0)],
                buffered_ranges: vec![(0.0, 1.0)],
                reached_eos: false,
            }]),
            flushed: false,
        })).unwrap();
        let session_id = handle.register_session();

        let outcome = handle.append_data(1, &[1]).unwrap();
        assert_eq!(outcome.input_buffered_ranges, vec![(0.0, 1.0)]);
        assert_eq!(outcome.buffered_ranges, vec![(0.0, 1.0)]);
        assert!(outcome.prepared.is_some());
        assert!(outcome.has_video_frames);

        let status = handle.status().unwrap();
        assert!(status.prepared);
        assert!(status.active);
        assert_eq!(status.buffered_ranges, vec![(0.0, 1.0)]);

        let mut session = take_registered_media_playback_session(session_id)
            .expect("registered MSE session missing");
        let prepared = session.check_prepared().unwrap().unwrap();
        assert_eq!(prepared.width, 2);
        assert_eq!(prepared.duration_ms, 1_000);
    }

}
