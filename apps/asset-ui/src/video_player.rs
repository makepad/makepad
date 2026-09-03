//! Video artifact playback on one app-lifetime decoder worker.
//!
//! Opens, seeks and packet decoding are commands to the worker. Frames cross
//! to the UI through a bounded, nonblocking hand-off; playback state is
//! published through atomics. Decoded audio crosses a single-producer /
//! single-consumer atomic ring into an engine owned by the device callback.
//! Neither the UI nor the realtime callback takes a mutex.

use makepad_widgets::log;
use makepad_widgets::makepad_platform::audio::AudioBuffer;
use makepad_widgets::makepad_platform::thread::{ThreadOptions, ThreadSpawner};
use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder};
use std::collections::VecDeque;
use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RING_FRAMES: usize = 3;
const AUDIO_AHEAD_SECS: f64 = 1.0;
const AUDIO_RING_FRAMES: usize = 1 << 17;

#[derive(Debug)]
struct Frame {
    generation: u64,
    pts_100ns: i64,
    width: u32,
    height: u32,
    bgra: Vec<u32>,
}

/// The count covers both the channel and frames already transferred into the
/// UI-owned presentation queue. The producer therefore never gets more than
/// `RING_FRAMES` ahead, while the consumer only ever calls `try_recv`.
struct FrameProducer {
    tx: Sender<Frame>,
    outstanding: Arc<AtomicUsize>,
}

struct FrameConsumer {
    rx: Receiver<Frame>,
    outstanding: Arc<AtomicUsize>,
}

fn frame_handoff() -> (FrameProducer, FrameConsumer) {
    let (tx, rx) = channel();
    let outstanding = Arc::new(AtomicUsize::new(0));
    (
        FrameProducer {
            tx,
            outstanding: outstanding.clone(),
        },
        FrameConsumer { rx, outstanding },
    )
}

impl FrameProducer {
    fn is_full(&self) -> bool {
        self.outstanding.load(Ordering::Acquire) >= RING_FRAMES
    }

    fn publish(&self, frame: Frame) -> Result<(), Frame> {
        if self
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < RING_FRAMES).then_some(count + 1)
            })
            .is_err()
        {
            return Err(frame);
        }
        match self.tx.send(frame) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                Err(error.0)
            }
        }
    }
}

impl FrameConsumer {
    /// Never waits for the producer and never locks application state.
    fn try_take(&self) -> Result<Frame, TryRecvError> {
        self.rx.try_recv()
    }

    fn release(&self) {
        self.outstanding.fetch_sub(1, Ordering::AcqRel);
    }

    fn outstanding(&self) -> usize {
        self.outstanding.load(Ordering::Acquire)
    }
}

#[derive(Default)]
struct PlaybackState {
    done: AtomicBool,
    eos: AtomicBool,
    seek_pending: AtomicBool,
    published_pts: AtomicI64,
    duration_100ns: AtomicI64,
}

/// Soundtrack samples shared only as a lock-free SPSC ring. The decoder is
/// the producer; `VideoAudioEngine` is the sole consumer.
struct VideoAudio {
    samples: Box<[AtomicU64]>,
    write_pos: AtomicU64,
    read_pos: AtomicU64,
    flush_at: AtomicU64,
    flush_generation: AtomicU32,
    owner: AtomicU64,
    muted: AtomicBool,
    source_rate_bits: AtomicU64,
}

impl VideoAudio {
    fn new() -> Self {
        Self {
            samples: (0..AUDIO_RING_FRAMES).map(|_| AtomicU64::new(0)).collect(),
            write_pos: AtomicU64::new(0),
            read_pos: AtomicU64::new(0),
            flush_at: AtomicU64::new(0),
            flush_generation: AtomicU32::new(0),
            owner: AtomicU64::new(0),
            muted: AtomicBool::new(true),
            source_rate_bits: AtomicU64::new(0.0f64.to_bits()),
        }
    }

    fn claim(&self, epoch: u64) {
        self.muted.store(true, Ordering::Release);
        self.owner.store(epoch, Ordering::Release);
        self.flush();
        self.source_rate_bits.store(0.0f64.to_bits(), Ordering::Release);
        self.muted.store(false, Ordering::Release);
    }

    fn release(&self, epoch: u64) {
        if self
            .owner
            .compare_exchange(epoch, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.muted.store(true, Ordering::Release);
            self.flush();
        }
    }

    fn stop(&self) {
        self.muted.store(true, Ordering::Release);
        self.owner.store(0, Ordering::Release);
        self.flush();
    }

    fn set_muted(&self, epoch: u64, muted: bool) {
        if self.owner.load(Ordering::Acquire) == epoch {
            self.muted.store(muted, Ordering::Release);
        }
    }

    fn is_muted(&self, epoch: u64) -> bool {
        self.owner.load(Ordering::Acquire) != epoch || self.muted.load(Ordering::Acquire)
    }

    fn clear_for(&self, epoch: u64) {
        if self.owner.load(Ordering::Acquire) == epoch {
            self.flush();
        }
    }

    fn flush(&self) {
        self.flush_at
            .store(self.write_pos.load(Ordering::Acquire), Ordering::Release);
        self.flush_generation.fetch_add(1, Ordering::AcqRel);
    }

    fn buffered_frames(&self) -> u64 {
        self.write_pos
            .load(Ordering::Acquire)
            .saturating_sub(self.read_pos.load(Ordering::Acquire))
    }

    fn buffered_secs(&self) -> f64 {
        let rate = f64::from_bits(self.source_rate_bits.load(Ordering::Acquire));
        if rate <= 0.0 {
            0.0
        } else {
            self.buffered_frames() as f64 / rate
        }
    }

    fn push_i16(&self, epoch: u64, samples: &[i16], channels: u16, rate: u32) {
        if self.is_muted(epoch) {
            return;
        }
        let channels = channels.max(1) as usize;
        let write = self.write_pos.load(Ordering::Relaxed);
        let free = AUDIO_RING_FRAMES.saturating_sub(self.buffered_frames() as usize);
        let count = samples.chunks_exact(channels).len().min(free);
        if count == 0 {
            return;
        }
        self.source_rate_bits
            .store((rate as f64).to_bits(), Ordering::Release);
        const GAIN: f32 = 0.9;
        for (index, frame) in samples.chunks_exact(channels).take(count).enumerate() {
            let left = frame[0] as f32 / 32768.0 * GAIN;
            let right = frame[channels - 1] as f32 / 32768.0 * GAIN;
            let packed = left.to_bits() as u64 | ((right.to_bits() as u64) << 32);
            self.samples[((write as usize) + index) & (AUDIO_RING_FRAMES - 1)]
                .store(packed, Ordering::Relaxed);
        }
        self.write_pos.store(write + count as u64, Ordering::Release);
    }

    fn sample(&self, position: u64) -> (f32, f32) {
        let packed = self.samples[(position as usize) & (AUDIO_RING_FRAMES - 1)]
            .load(Ordering::Relaxed);
        (
            f32::from_bits(packed as u32),
            f32::from_bits((packed >> 32) as u32),
        )
    }
}

/// State owned outright by the realtime callback. Shared storage contains
/// only atomics; a UI load, pause, seek or stop can never block this engine.
pub struct VideoAudioEngine {
    shared: Arc<VideoAudio>,
    owner_seen: u64,
    flush_seen: u32,
    cursor: f64,
}

impl VideoAudioEngine {
    pub fn mix_into(&mut self, output: &mut AudioBuffer, device_rate: f64) {
        let owner = self.shared.owner.load(Ordering::Acquire);
        let flush = self.shared.flush_generation.load(Ordering::Acquire);
        if owner != self.owner_seen || flush != self.flush_seen {
            self.owner_seen = owner;
            self.flush_seen = flush;
            let at = self.shared.flush_at.load(Ordering::Acquire);
            self.shared.read_pos.store(at, Ordering::Release);
            self.cursor = at as f64;
        }
        if owner == 0 || self.shared.muted.load(Ordering::Acquire) || device_rate <= 0.0 {
            return;
        }
        let source_rate = f64::from_bits(self.shared.source_rate_bits.load(Ordering::Acquire));
        if source_rate <= 0.0 {
            return;
        }
        let write = self.shared.write_pos.load(Ordering::Acquire);
        let step = source_rate / device_rate;
        let channels = output.channel_count();
        for frame in 0..output.frame_count() {
            let index = self.cursor.floor() as u64;
            if index + 1 >= write {
                break;
            }
            let fraction = (self.cursor - index as f64) as f32;
            let (al, ar) = self.shared.sample(index);
            let (bl, br) = self.shared.sample(index + 1);
            let left = al + (bl - al) * fraction;
            let right = ar + (br - ar) * fraction;
            for channel in 0..channels {
                output.channel_mut(channel)[frame] += if channel == 0 { left } else { right };
            }
            self.cursor += step;
        }
        self.shared
            .read_pos
            .store(self.cursor.floor() as u64, Ordering::Release);
    }
}

enum DecoderCommand {
    Open {
        epoch: u64,
        path: String,
        frames: FrameProducer,
        state: Arc<PlaybackState>,
    },
    Seek {
        epoch: u64,
        generation: u64,
        target_100ns: i64,
    },
    Wake {
        epoch: u64,
    },
    Stop {
        epoch: u64,
    },
    Shutdown,
}

pub struct VideoDecoder {
    tx: Sender<DecoderCommand>,
    audio: Arc<VideoAudio>,
}

impl VideoDecoder {
    pub fn start(spawner: ThreadSpawner) -> Result<(Self, VideoAudioEngine), String> {
        let (tx, rx) = channel();
        let audio = Arc::new(VideoAudio::new());
        let worker_audio = audio.clone();
        spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("asset-ui-video-decode".into()),
                    ..Default::default()
                },
                move || decoder_worker(rx, worker_audio),
            )
            .map_err(|error| error.to_string())?
            .detach();
        Ok((
            Self {
                tx,
                audio: audio.clone(),
            },
            VideoAudioEngine {
                shared: audio,
                owner_seen: 0,
                flush_seen: 0,
                cursor: 0.0,
            },
        ))
    }

    pub fn stop_audio(&self) {
        self.audio.stop();
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.audio.stop();
        let _ = self.tx.send(DecoderCommand::Shutdown);
    }
}

static NEXT_CLIP_EPOCH: AtomicU64 = AtomicU64::new(1);
const FRAME_EPS_100NS: i64 = 83_000;

pub struct VideoPlayer {
    pub width: u32,
    pub height: u32,
    pub duration_100ns: i64,
    tx: Sender<DecoderCommand>,
    audio: Arc<VideoAudio>,
    incoming: FrameConsumer,
    frames: VecDeque<Frame>,
    state: Arc<PlaybackState>,
    started: Option<Instant>,
    last_pts: i64,
    paused_at: Option<Instant>,
    epoch: u64,
    generation: u64,
}

impl VideoPlayer {
    pub fn new(path: &str, decoder: &VideoDecoder) -> Result<Self, String> {
        if !std::path::Path::new(path).is_file() {
            return Err(format!("video file not found: {path}"));
        }
        let epoch = NEXT_CLIP_EPOCH.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(PlaybackState::default());
        let (producer, incoming) = frame_handoff();
        decoder.audio.claim(epoch);
        decoder
            .tx
            .send(DecoderCommand::Open {
                epoch,
                path: path.to_string(),
                frames: producer,
                state: state.clone(),
            })
            .map_err(|_| "video decoder worker is not running".to_string())?;
        Ok(Self {
            // The worker publishes the real dimensions with its first frame.
            // Until then callers get a stable 16:9 loading shape rather than
            // blocking the UI on a container open.
            width: 16,
            height: 9,
            duration_100ns: 0,
            tx: decoder.tx.clone(),
            audio: decoder.audio.clone(),
            incoming,
            frames: VecDeque::new(),
            state,
            started: None,
            last_pts: 0,
            paused_at: None,
            epoch,
            generation: 0,
        })
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
            self.audio.set_muted(self.epoch, true);
        }
    }

    pub fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            if let Some(started) = &mut self.started {
                *started += paused_at.elapsed();
            }
            self.audio.set_muted(self.epoch, false);
            self.wake_decoder();
        }
    }

    fn wake_decoder(&self) {
        let _ = self.tx.send(DecoderCommand::Wake { epoch: self.epoch });
    }

    fn release_frame(&self) {
        self.incoming.release();
        self.wake_decoder();
    }

    fn receive_frames(&mut self) {
        while self.frames.len() < RING_FRAMES {
            match self.incoming.try_take() {
                Ok(frame) if frame.generation == self.generation => {
                    self.width = frame.width;
                    self.height = frame.height;
                    self.duration_100ns = self.state.duration_100ns.load(Ordering::Acquire);
                    self.frames.push_back(frame);
                }
                Ok(_) => self.release_frame(),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    pub fn take_due_frame(&mut self) -> Option<Vec<u32>> {
        self.receive_frames();
        if self.paused_at.is_some() {
            if self.started.is_some() {
                return None;
            }
            let frame = self.frames.pop_front()?;
            self.last_pts = frame.pts_100ns;
            self.release_frame();
            return Some(frame.bgra);
        }
        let first_pts = self.frames.front()?.pts_100ns;
        let started = *self.started.get_or_insert_with(|| {
            Instant::now() - Duration::from_nanos(first_pts.max(0) as u64 * 100)
        });
        let media_100ns = (started.elapsed().as_nanos() / 100) as i64;
        let mut due = None;
        while self.frames.front().is_some_and(|frame| frame.pts_100ns <= media_100ns) {
            due = self.frames.pop_front();
            self.release_frame();
            self.receive_frames();
        }
        if let Some(frame) = &due {
            self.last_pts = frame.pts_100ns;
        }
        due.map(|frame| frame.bgra)
    }

    pub fn position_secs(&self) -> f64 {
        self.last_pts as f64 / 10_000_000.0
    }

    pub fn duration_secs(&self) -> f64 {
        self.state.duration_100ns.load(Ordering::Acquire) as f64 / 10_000_000.0
    }

    pub fn seek(&mut self, secs: f64) {
        let target = (secs.max(0.0) * 10_000_000.0) as i64;
        while self.frames.pop_front().is_some() {
            self.release_frame();
        }
        while self.incoming.try_take().is_ok() {
            self.release_frame();
        }
        self.generation = self.generation.wrapping_add(1);
        self.started = None;
        self.last_pts = target;
        self.state.seek_pending.store(true, Ordering::Release);
        self.audio.clear_for(self.epoch);
        let _ = self.tx.send(DecoderCommand::Seek {
            epoch: self.epoch,
            generation: self.generation,
            target_100ns: target,
        });
    }

    pub fn at_end(&self) -> bool {
        (self.state.eos.load(Ordering::Acquire) || self.state.done.load(Ordering::Acquire))
            && self.incoming.outstanding() == 0
    }

    pub fn seek_pending(&self) -> bool {
        self.state.seek_pending.load(Ordering::Acquire)
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        while self.frames.pop_front().is_some() {
            self.incoming.release();
        }
        while self.incoming.try_take().is_ok() {
            self.incoming.release();
        }
        let _ = self.tx.send(DecoderCommand::Stop { epoch: self.epoch });
        self.audio.release(self.epoch);
    }
}

struct DecodeSession {
    epoch: u64,
    generation: u64,
    decoder: VideoFileDecoder,
    frames: FrameProducer,
    state: Arc<PlaybackState>,
    audio_eos: bool,
    seek_target: Option<i64>,
    rgb_scratch: Vec<u8>,
}

enum Step {
    Continue,
    Park,
    End,
}

fn decoder_worker(rx: Receiver<DecoderCommand>, audio: Arc<VideoAudio>) {
    let mut session: Option<DecodeSession> = None;
    loop {
        if session.is_none()
            || session.as_ref().is_some_and(|item| {
                item.frames.is_full() || item.state.eos.load(Ordering::Acquire)
            })
        {
            let Ok(command) = rx.recv() else { return };
            if apply_command(command, &mut session, &audio) {
                return;
            }
            continue;
        }
        while let Ok(command) = rx.try_recv() {
            if apply_command(command, &mut session, &audio) {
                return;
            }
        }
        let Some(active) = &mut session else { continue };
        match decode_step(active, &audio) {
            Step::Continue | Step::Park => {}
            Step::End => {
                active.state.done.store(true, Ordering::Release);
                session = None;
            }
        }
    }
}

fn apply_command(
    command: DecoderCommand,
    session: &mut Option<DecodeSession>,
    audio: &VideoAudio,
) -> bool {
    match command {
        DecoderCommand::Open { epoch, path, frames, state } => {
            if let Some(old) = session.take() {
                old.state.done.store(true, Ordering::Release);
            }
            match VideoFileDecoder::open(&path) {
                Ok(decoder) => {
                    let info = decoder.info();
                    if info.width == 0 || info.height == 0 {
                        log!("video: {path} reports zero size: {}x{}", info.width, info.height);
                        state.done.store(true, Ordering::Release);
                        return false;
                    }
                    log!(
                        "video: {} {}x{} {}/{} fps codec {:?} audio {} ({} Hz, {} ch)",
                        path,
                        info.width,
                        info.height,
                        info.fps_num,
                        info.fps_den,
                        info.video_codec,
                        info.has_audio,
                        info.audio_sample_rate,
                        info.audio_channels
                    );
                    state
                        .duration_100ns
                        .store(info.duration_100ns, Ordering::Release);
                    let audio_eos = !info.has_audio;
                    *session = Some(DecodeSession {
                        epoch,
                        generation: 0,
                        decoder,
                        frames,
                        state,
                        audio_eos,
                        seek_target: None,
                        rgb_scratch: Vec::new(),
                    });
                }
                Err(error) => {
                    log!("video: decoder worker open failed: {error}");
                    state.done.store(true, Ordering::Release);
                }
            }
        }
        DecoderCommand::Seek { epoch, generation, target_100ns } => {
            let Some(active) = session.as_mut().filter(|item| item.epoch == epoch) else {
                return false;
            };
            active.state.eos.store(false, Ordering::Release);
            audio.clear_for(epoch);
            match active.decoder.seek(target_100ns) {
                Ok(()) => {
                    active.generation = generation;
                    active.seek_target = Some(target_100ns);
                    active.audio_eos = !active.decoder.info().has_audio;
                }
                Err(error) => log!("video: decoder seek failed: {error}"),
            }
            active.state.seek_pending.store(false, Ordering::Release);
        }
        DecoderCommand::Wake { epoch } => {
            let _ = session.as_ref().is_some_and(|item| item.epoch == epoch);
        }
        DecoderCommand::Stop { epoch } => {
            if session.as_ref().is_some_and(|item| item.epoch == epoch) {
                if let Some(old) = session.take() {
                    old.state.done.store(true, Ordering::Release);
                }
            }
            audio.release(epoch);
        }
        DecoderCommand::Shutdown => {
            if let Some(old) = session.take() {
                old.state.done.store(true, Ordering::Release);
            }
            return true;
        }
    }
    false
}

fn decode_step(active: &mut DecodeSession, audio: &VideoAudio) -> Step {
    let info = active.decoder.info().clone();
    if info.has_audio && !active.audio_eos && !audio.is_muted(active.epoch) {
        while audio.buffered_secs() < AUDIO_AHEAD_SECS {
            match active.decoder.next_audio() {
                Ok(Some(chunk)) => audio.push_i16(
                    active.epoch,
                    &chunk.samples,
                    chunk.channels,
                    chunk.sample_rate,
                ),
                Ok(None) => {
                    active.audio_eos = true;
                    break;
                }
                Err(error) => {
                    log!("video: audio decode error: {error}");
                    active.audio_eos = true;
                    break;
                }
            }
        }
    }
    if active.frames.is_full() {
        return Step::Park;
    }
    match active.decoder.next_frame() {
        Ok(Some(frame)) => {
            if active
                .seek_target
                .is_some_and(|target| frame.pts_100ns + FRAME_EPS_100NS < target)
            {
                return Step::Continue;
            }
            active.seek_target = None;
            nv12::nv12_to_rgb8(
                &frame.nv12,
                frame.width,
                frame.height,
                &mut active.rgb_scratch,
            );
            let mut bgra = Vec::with_capacity((frame.width * frame.height) as usize);
            for pixel in active.rgb_scratch.chunks_exact(3) {
                bgra.push(
                    0xff00_0000
                        | ((pixel[0] as u32) << 16)
                        | ((pixel[1] as u32) << 8)
                        | pixel[2] as u32,
                );
            }
            let pts = frame.pts_100ns;
            if active
                .frames
                .publish(Frame {
                    generation: active.generation,
                    pts_100ns: pts,
                    width: frame.width,
                    height: frame.height,
                    bgra,
                })
                .is_err()
            {
                return Step::End;
            }
            active.state.published_pts.store(pts, Ordering::Release);
            Step::Continue
        }
        Ok(None) => {
            if info.has_audio && !active.audio_eos && !audio.is_muted(active.epoch) {
                loop {
                    match active.decoder.next_audio() {
                        Ok(Some(chunk)) => audio.push_i16(
                            active.epoch,
                            &chunk.samples,
                            chunk.channels,
                            chunk.sample_rate,
                        ),
                        Ok(None) => break,
                        Err(error) => {
                            log!("video: audio tail decode error: {error}");
                            break;
                        }
                    }
                }
                active.audio_eos = true;
            }
            active.state.eos.store(true, Ordering::Release);
            Step::Park
        }
        Err(error) => {
            log!("video: decode error: {error}");
            Step::End
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame(pts_100ns: i64) -> Frame {
        Frame {
            generation: 0,
            pts_100ns,
            width: 2,
            height: 2,
            bgra: vec![0xff00_0000; 4],
        }
    }

    #[test]
    fn video_frame_handover_never_blocks_the_consumer() {
        let (producer, consumer) = frame_handoff();
        for pts in 0..RING_FRAMES as i64 {
            producer.publish(test_frame(pts)).unwrap();
        }
        assert!(producer.publish(test_frame(99)).is_err(), "producer is bounded");
        for pts in 0..RING_FRAMES as i64 {
            assert_eq!(consumer.try_take().unwrap().pts_100ns, pts);
            consumer.release();
        }
        assert!(matches!(consumer.try_take(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn at_end_needs_decode_exit_and_a_drained_handover() {
        let (tx, _rx) = channel();
        let audio = Arc::new(VideoAudio::new());
        let state = Arc::new(PlaybackState::default());
        let (producer, incoming) = frame_handoff();
        producer.publish(test_frame(0)).unwrap();
        let mut player = VideoPlayer {
            width: 2,
            height: 2,
            duration_100ns: 0,
            tx,
            audio,
            incoming,
            frames: VecDeque::new(),
            state: state.clone(),
            started: None,
            last_pts: 0,
            paused_at: None,
            epoch: u64::MAX,
            generation: 0,
        };
        assert!(!player.at_end());
        state.done.store(true, Ordering::Release);
        assert!(!player.at_end());
        assert!(player.take_due_frame().is_some());
        assert!(player.at_end());
    }

    #[test]
    fn soundtrack_ring_ignores_stale_epochs_and_revoked_ownership() {
        let audio = VideoAudio::new();
        audio.claim(7);
        audio.push_i16(6, &[1000, 1000], 2, 48_000);
        assert_eq!(audio.buffered_frames(), 0);
        audio.push_i16(7, &[1000, 1000, 2000, 2000], 2, 48_000);
        assert_eq!(audio.buffered_frames(), 2);
        audio.stop();
        audio.push_i16(7, &[1000, 1000], 2, 48_000);
        assert_eq!(audio.owner.load(Ordering::Acquire), 0);
        assert!(audio.muted.load(Ordering::Acquire));
    }
}
