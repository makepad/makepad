//! The audio callbacks' halves of a call. [`VoiceIn`] lives in the app's audio input callback: it
//! frames, gates and encodes the microphone and queues the frames for the network worker.
//! [`VoiceOut`] lives in the audio output callback: it plays every peer's jitter buffer and adds
//! the mix into the device block. Neither allocates, frees, blocks or takes a lock anyone waits
//! on. The hand-offs are a ring of atomic slots (voice frames out) and a mailbox both sides only
//! ever `try_lock` (a new set of peers in, the old set back).

use crate::aec::{AecProcessor, RenderRefRing, AEC_BLOCK, AEC_SAMPLE_RATE, RENDER_RING_CAP};
use crate::wire::{stream, voice_flags};
use makepad_teamtalk::codec::{adpcm_decode, adpcm_encode_into, AdpcmState};
use makepad_teamtalk::jitter::{JitterRing, Playout};
use makepad_teamtalk::resample::Resampler;
use makepad_teamtalk::{Framer, FramerConfig, PlayoutConfig, INTERNAL_RATE, MAX_FRAME};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use super::PeerId;

/// The voice's rate on the wire.
pub const VOICE_RATE: f64 = 16_000.0;
/// Samples per voice frame: 20 ms.
pub const VOICE_FRAME: usize = 320;
/// Frames the input ring holds: 1.28 s, far beyond any wake of the network worker. A frame that
/// finds it full is dropped and counted (voice that late is useless to play).
const QUEUE: usize = 64;
/// A slot's payload room in 64-bit words. A 4-bit frame of 320 samples is 168 bytes.
const WORDS: usize = 32;
/// The longest run the output callback renders at once; longer blocks go in pieces.
const CHUNK: usize = 1024;
/// A backlog beyond twice the 64 ms burst slack indicates a gap in capture.
const CAPTURE_GAP_SAMPLES: usize = 128 * AEC_SAMPLE_RATE / 1000;

/// A participant's voice marks, written by whichever callback hears them, read by the UI.
#[derive(Default)]
pub struct VoiceMarks {
    level: AtomicU32,
    talking: AtomicBool,
}

impl VoiceMarks {
    /// The voice's level, 0 to 1 (the RMS of the last frame or block heard).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn talking(&self) -> bool {
        self.talking.load(Ordering::Relaxed)
    }

    fn set(&self, level: f32, talking: bool) {
        self.level.store(level.to_bits(), Ordering::Relaxed);
        self.talking.store(talking, Ordering::Relaxed);
    }
}

/// Playback controls for one audio stream from one participant.
pub struct SourceLevel {
    gain: AtomicU32,
    muted: AtomicBool,
}

impl Default for SourceLevel {
    fn default() -> Self {
        Self { gain: AtomicU32::new(1.0f32.to_bits()), muted: AtomicBool::new(false) }
    }
}

impl SourceLevel {
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    fn volume(&self) -> f32 {
        let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
        if self.muted.load(Ordering::Relaxed) { 0.0 } else { gain }
    }
}

/// Net publishes the control when the first frame arrives. Polling the UI is unnecessary.
#[derive(Default)]
pub(super) struct AudioLevels {
    voice: OnceLock<Arc<SourceLevel>>,
    app: OnceLock<Arc<SourceLevel>>,
}

impl AudioLevels {
    pub(super) fn slot(&self, stream: u8) -> Option<&OnceLock<Arc<SourceLevel>>> {
        match stream {
            stream::VOICE => Some(&self.voice),
            stream::APP_AUDIO => Some(&self.app),
            _ => None,
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt().min(1.0)
}

struct Slot {
    /// seq (32 bits) | flags (8) | payload length (16)
    meta: AtomicU64,
    capture_us: AtomicU64,
    words: [AtomicU64; WORDS],
}

/// The ring between the input callback (the only writer) and the network worker (the only
/// reader). Its slots are atomics, so it needs no `unsafe`: the head's release store publishes a
/// slot's contents, the tail's release store gives the slot back.
struct VoiceQueue {
    tap_on: AtomicBool,
    tap: OnceLock<TapRing>,
    slots: Box<[Slot]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    dropped: AtomicU64,
}

/// One voice frame as the network worker takes it from the ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceFrame {
    pub seq: u32,
    pub flags: u8,
    pub capture_us: u64,
}

impl VoiceQueue {
    fn new() -> Self {
        let slot = || Slot { meta: AtomicU64::new(0), capture_us: AtomicU64::new(0), words: std::array::from_fn(|_| AtomicU64::new(0)) };
        VoiceQueue { tap_on: AtomicBool::new(false), tap: OnceLock::new(), slots: (0..QUEUE).map(|_| slot()).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0), dropped: AtomicU64::new(0) }
    }

    fn push(&self, frame: VoiceFrame, payload: &[u8]) {
        let head = self.head.load(Ordering::Relaxed);
        if payload.len() > WORDS * 8 || head.wrapping_sub(self.tail.load(Ordering::Acquire)) >= QUEUE {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let slot = &self.slots[head % QUEUE];
        for (word, bytes) in slot.words.iter().zip(payload.chunks(8)) {
            let mut b = [0u8; 8];
            b[..bytes.len()].copy_from_slice(bytes);
            word.store(u64::from_le_bytes(b), Ordering::Relaxed);
        }
        let meta = u64::from(frame.seq) << 32 | u64::from(frame.flags) << 16 | payload.len() as u64;
        slot.meta.store(meta, Ordering::Relaxed);
        slot.capture_us.store(frame.capture_us, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }

    fn pop(&self, payload: &mut Vec<u8>) -> Option<VoiceFrame> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let slot = &self.slots[tail % QUEUE];
        let meta = slot.meta.load(Ordering::Relaxed);
        let len = (meta & 0xffff) as usize;
        payload.clear();
        for word in &slot.words[..len.div_ceil(8)] {
            payload.extend_from_slice(&word.load(Ordering::Relaxed).to_le_bytes());
        }
        payload.truncate(len);
        let frame = VoiceFrame { seq: (meta >> 32) as u32, flags: (meta >> 16) as u8, capture_us: slot.capture_us.load(Ordering::Relaxed) };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(frame)
    }
}

/// Flags and capture time of one frame after gating and mute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapFrame {
    pub silent: bool,
    pub talk_start: bool,
    pub capture_us: u64,
}

struct TapSlot {
    samples: [AtomicU32; VOICE_FRAME],
    flags: AtomicU8,
    capture_us: AtomicU64,
}

// Over two seconds of microphone frames. A slow transcriber cannot stall the call.
const TAP_FRAMES: usize = 128;

struct TapRing {
    slots: Box<[TapSlot]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    dropped: AtomicU64,
}

impl TapRing {
    fn new() -> Self {
        let slots = (0..TAP_FRAMES).map(|_| TapSlot {
            samples: std::array::from_fn(|_| AtomicU32::new(0)),
            flags: 0.into(),
            capture_us: 0.into(),
        }).collect();
        Self { slots, head: 0.into(), tail: 0.into(), dropped: 0.into() }
    }

    fn push(&self, f: &makepad_teamtalk::Frame<'_>, flags: u8) {
        let head = self.head.load(Ordering::Relaxed);
        if head.wrapping_sub(self.tail.load(Ordering::Acquire)) >= TAP_FRAMES {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let slot = &self.slots[head % TAP_FRAMES];
        for (word, sample) in slot.samples.iter().zip(f.samples) {
            word.store(sample.to_bits(), Ordering::Relaxed);
        }
        slot.flags.store(flags, Ordering::Relaxed);
        slot.capture_us.store(f.capture_us, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }
}

/// The transcriber's end of the local microphone, after the call's gate and mute.
/// Each frame spans 20 ms at 16 kHz. The control thread can switch it off while the reader works.
pub struct TranscriptTap {
    queue: Arc<VoiceQueue>,
    // Only pop takes this lock. Sharing the tap to toggle it never locks the callback or the UI.
    next: Mutex<usize>,
}

impl TranscriptTap {
    /// Enables or disables writing new microphone frames. Initially off.
    pub fn set_on(&self, on: bool) {
        self.queue.tap_on.store(on, Ordering::Release);
    }

    /// Clears `samples`, then fills it with VOICE_FRAME samples for speech or none for silence.
    pub fn pop(&self, samples: &mut Vec<f32>) -> Option<TapFrame> {
        samples.clear();
        let ring = self.queue.tap.get()?;
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        if *next == ring.head.load(Ordering::Acquire) {
            return None;
        }
        let slot = &ring.slots[*next % TAP_FRAMES];
        let flags = slot.flags.load(Ordering::Relaxed);
        let frame = TapFrame {
            silent: flags & voice_flags::SILENT != 0,
            talk_start: flags & voice_flags::TALK_START != 0,
            capture_us: slot.capture_us.load(Ordering::Relaxed),
        };
        if !frame.silent {
            samples.extend(slot.samples.iter().map(|word| f32::from_bits(word.load(Ordering::Relaxed))));
        }
        *next = next.wrapping_add(1);
        ring.tail.store(*next, Ordering::Release);
        Some(frame)
    }

    /// Frames the callback dropped because the tap was full.
    pub fn dropped(&self) -> u64 {
        self.queue.tap.get().unwrap().dropped.load(Ordering::Relaxed)
    }
}

impl Drop for TranscriptTap {
    fn drop(&mut self) {
        self.set_on(false);
    }
}

/// The network worker's end of the input ring.
pub struct VoiceFrames {
    queue: Arc<VoiceQueue>,
}

impl VoiceFrames {
    /// Prepares a transcription reader off the audio thread, once for this source.
    pub fn transcript_tap(&mut self) -> Option<TranscriptTap> {
        if self.queue.tap.get().is_some() {
            return None;
        }
        self.queue.tap.set(TapRing::new()).ok()?;
        Some(TranscriptTap { queue: self.queue.clone(), next: Mutex::new(0) })
    }

    /// The next frame, its encoded payload in `payload` (empty for a silent frame).
    pub fn pop(&self, payload: &mut Vec<u8>) -> Option<VoiceFrame> {
        self.queue.pop(payload)
    }

    /// Frames the input callback dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.queue.dropped.load(Ordering::Relaxed)
    }
}

/// The capture side of the render hand-off, including any partial 4 ms block.
struct RenderDrain {
    ring: Arc<RenderRefRing>,
    last_overflows: u64,
    last_paused_overflows: u64,
    last_reset_epoch: u64,
    scratch: [f32; 512],
    buf: [f32; AEC_BLOCK],
    fill: usize,
}

impl RenderDrain {
    fn new(ring: Arc<RenderRefRing>) -> Self {
        Self {
            ring, last_overflows: 0, last_paused_overflows: 0, last_reset_epoch: 0,
            scratch: [0.0; 512], buf: [0.0; AEC_BLOCK], fill: 0,
        }
    }

    fn has_gap(&self) -> bool {
        self.ring.overflows() != self.last_overflows
            || self.ring.paused_overflows() != self.last_paused_overflows
            || self.ring.reset_epoch() != self.last_reset_epoch
            || self.ring.available() > CAPTURE_GAP_SAMPLES
    }

    fn reset(&mut self) {
        // Leave concurrent reset requests pending and absorb losses on the discarded backlog.
        self.last_reset_epoch = self.ring.reset_epoch();
        self.ring.discard();
        self.last_overflows = self.ring.overflows();
        self.last_paused_overflows = self.ring.paused_overflows();
        self.fill = 0;
    }

    fn feed(&mut self, processor: &mut AecProcessor) {
        if self.has_gap() {
            self.reset();
            processor.reset();
        }
        for _ in 0..RENDER_RING_CAP / self.scratch.len() {
            let n = self.ring.pop_into(&mut self.scratch);
            let mut pos = 0;
            while pos < n {
                let take = (AEC_BLOCK - self.fill).min(n - pos);
                self.buf[self.fill..self.fill + take].copy_from_slice(&self.scratch[pos..pos + take]);
                pos += take;
                self.fill += take;
                if self.fill == AEC_BLOCK {
                    processor.feed_render(&self.buf);
                    self.fill = 0;
                }
            }
            if n < self.scratch.len() { break; }
        }
    }
}

struct AecCapture {
    near_resampler: Resampler,
    render: RenderDrain,
    processor: AecProcessor,
    near_buf: [f32; AEC_BLOCK],
    near_fill: usize,
    near_block_us: u64,
    last_rate: f64,
}

impl AecCapture {
    fn new(ring: Arc<RenderRefRing>) -> Self {
        Self {
            near_resampler: Resampler::new(),
            render: RenderDrain::new(ring),
            processor: AecProcessor::new(), near_buf: [0.0; AEC_BLOCK], near_fill: 0, near_block_us: 0,
            last_rate: 0.0,
        }
    }

    fn reset(&mut self) {
        self.near_resampler.reset();
        self.near_fill = 0;
        self.render.reset();
        self.processor.reset();
    }

    fn push(&mut self, rate: f64, samples: impl Iterator<Item = f32>, block_us: u64, muted: bool,
            mut framed: impl FnMut(&[f32; AEC_BLOCK], u64)) {
        let rate = rate.max(1000.0);
        let mut samples = samples.peekable();
        if samples.peek().is_none() { return; }
        let rate_changed = self.last_rate != 0.0 && (rate - self.last_rate).abs() > 1.0;
        if rate_changed || self.render.has_gap() {
            self.reset();
        }
        self.last_rate = rate;
        if !muted { self.render.ring.note_capture_active(); }
        self.near_resampler.set_ratio(rate, VOICE_RATE, 0.0);
        for (i, x) in samples.enumerate() {
            self.near_resampler.push(if muted { 0.0 } else { x }, |y| {
                if self.near_fill == 0 {
                    self.near_block_us = block_us.saturating_add((i as f64 * 1e6 / rate).round() as u64);
                }
                self.near_buf[self.near_fill] = y;
                self.near_fill += 1;
                if self.near_fill == AEC_BLOCK {
                    self.render.feed(&mut self.processor);
                    if muted {
                        self.processor.process_muted();
                        self.near_buf.fill(0.0);
                    } else {
                        self.processor.process_capture(&mut self.near_buf);
                    }
                    framed(&self.near_buf, self.near_block_us);
                    self.near_fill = 0;
                }
            });
        }
    }
}

/// The input callback's half: `Send`, not `Clone`. Move it into the audio input closure.
pub struct VoiceIn {
    aec: Option<Box<AecCapture>>,
    framer: Framer,
    adpcm: AdpcmState,
    payload: Vec<u8>,
    queue: Arc<VoiceQueue>,
    marks: Arc<VoiceMarks>,
    muted: Arc<AtomicBool>,
    clock: Instant,
}

/// The input callback's half and the network worker's end of its ring. `clock` is the call's
/// media clock (capture times are µs since it); `muted` is read at every block.
pub fn voice_in(clock: Instant, muted: Arc<AtomicBool>, marks: Arc<VoiceMarks>) -> (VoiceIn, VoiceFrames) {
    voice_in_with_aec(clock, muted, marks, None)
}

pub fn voice_in_with_aec(clock: Instant, muted: Arc<AtomicBool>, marks: Arc<VoiceMarks>, render_ref: Option<Arc<RenderRefRing>>) -> (VoiceIn, VoiceFrames) {
    let aec = render_ref.map(|ring| Box::new(AecCapture::new(ring)));
    let queue = Arc::new(VoiceQueue::new());
    let framer = Framer::new(FramerConfig { out_rate: VOICE_RATE, frame_samples: VOICE_FRAME, gate_threshold_rms: 0.01, gate_hangover_ms: 200 })
        .expect("the voice frame size is valid");
    let input = VoiceIn { aec, framer, adpcm: AdpcmState::default(), payload: Vec::with_capacity(WORDS * 8), queue: queue.clone(), marks, muted, clock };
    (input, VoiceFrames { queue })
}

impl VoiceIn {
    /// The clock at the first sample of a block of `frames` that just arrived.
    fn block_us(&self, rate: f64, frames: usize) -> u64 {
        let now = super::elapsed_us(self.clock);
        now.saturating_sub((frames as f64 * 1e6 / rate.max(1.0)) as u64)
    }

    fn begin(&mut self) -> bool {
        let muted = self.muted.load(Ordering::Acquire);
        self.framer.set_muted(muted);
        muted
    }

    pub fn push_mono(&mut self, rate: f64, samples: &[f32]) {
        let muted = self.begin();
        let block_us = self.block_us(rate, samples.len());
        let (adpcm, payload, queue, marks) = (&mut self.adpcm, &mut self.payload, &*self.queue, &*self.marks);
        let mut send = |f: &makepad_teamtalk::Frame<'_>| emit(adpcm, payload, queue, marks, f);
        if let Some(aec) = &mut self.aec {
            aec.push(rate, samples.iter().copied(), block_us, muted, |block, us| self.framer.push_mono(VOICE_RATE, block, us, &mut send));
        } else {
            self.framer.push_mono(rate, samples, block_us, &mut send);
        }
    }

    /// A channel-major block (`AudioBuffer`'s layout), folded to mono.
    pub fn push_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &[f32]) {
        let muted = self.begin();
        let block_us = self.block_us(rate, frames);
        let (adpcm, payload, queue, marks) = (&mut self.adpcm, &mut self.payload, &*self.queue, &*self.marks);
        let mut send = |f: &makepad_teamtalk::Frame<'_>| emit(adpcm, payload, queue, marks, f);
        if let Some(aec) = &mut self.aec {
            let channels = channels.max(1);
            if frames > data.len() / channels { return; }
            let scale = 1.0 / channels as f32;
            let mono = (0..frames).map(|i| (0..channels).map(|c| data[c * frames + i]).sum::<f32>() * scale);
            aec.push(rate, mono, block_us, muted, |block, us| self.framer.push_mono(VOICE_RATE, block, us, &mut send));
        } else {
            self.framer.push_planar(rate, frames, channels, data, block_us, &mut send);
        }
    }

    pub fn push_interleaved(&mut self, rate: f64, channels: usize, data: &[f32]) {
        let muted = self.begin();
        let block_us = self.block_us(rate, data.len() / channels.max(1));
        let (adpcm, payload, queue, marks) = (&mut self.adpcm, &mut self.payload, &*self.queue, &*self.marks);
        let mut send = |f: &makepad_teamtalk::Frame<'_>| emit(adpcm, payload, queue, marks, f);
        if let Some(aec) = &mut self.aec {
            let channels = channels.max(1);
            let scale = 1.0 / channels as f32;
            let mono = data.chunks_exact(channels).map(|s| s.iter().sum::<f32>() * scale);
            aec.push(rate, mono, block_us, muted, |block, us| self.framer.push_mono(VOICE_RATE, block, us, &mut send));
        } else {
            self.framer.push_interleaved(rate, channels, data, block_us, &mut send);
        }
    }
}

fn emit(adpcm: &mut AdpcmState, payload: &mut Vec<u8>, queue: &VoiceQueue, marks: &VoiceMarks, f: &makepad_teamtalk::Frame<'_>) {
    payload.clear();
    if !f.silent {
        // `payload` has room for a whole frame, so this never allocates.
        adpcm_encode_into(adpcm, 4, f.samples, payload);
    }
    let mut flags = 0;
    if f.silent {
        flags |= voice_flags::SILENT;
    }
    if f.talk_start {
        flags |= voice_flags::TALK_START;
    }
    marks.set(rms(f.samples), !f.silent);
    if queue.tap_on.load(Ordering::Acquire) {
        queue.tap.get().unwrap().push(f, flags);
    }
    queue.push(VoiceFrame { seq: f.seq, flags, capture_us: f.capture_us }, payload);
}

/// The network worker's decoder of one peer's voice into that peer's jitter buffer.
pub struct VoiceDecoder {
    up: Resampler,
    pcm: Vec<i16>,
}

impl Default for VoiceDecoder {
    fn default() -> Self {
        let mut up = Resampler::new();
        up.set_ratio(VOICE_RATE, INTERNAL_RATE, 0.0);
        VoiceDecoder { up, pcm: Vec::with_capacity(MAX_FRAME) }
    }
}

impl VoiceDecoder {
    pub fn insert(&mut self, ring: &JitterRing, seq: u32, flags: u8, payload: &[u8]) {
        self.pcm.clear();
        let silent = flags & voice_flags::SILENT != 0;
        if !silent {
            if let Some(samples) = adpcm_decode(payload) {
                let pcm = &mut self.pcm;
                for s in samples {
                    self.up.push(s, |x| pcm.push((x * 32768.0).round().clamp(-32768.0, 32767.0) as i16));
                }
            }
        }
        if silent || self.pcm.is_empty() {
            self.pcm.resize(VOICE_FRAME * 3, 0);
        }
        self.pcm.truncate(MAX_FRAME);
        ring.insert(seq, &self.pcm, silent);
    }
}

fn playout_config() -> PlayoutConfig {
    PlayoutConfig { default_frame: VOICE_FRAME * 3, ..Default::default() }
}

struct MixPeer {
    peer: PeerId,
    stream: u8,
    level: Arc<SourceLevel>,
    ring: Arc<JitterRing>,
    playout: Playout,
    marks: Arc<VoiceMarks>,
}

/// The peers the output callback plays. A new one is built off the audio thread, whole.
pub struct Mix {
    peers: Vec<MixPeer>,
}

#[derive(Default)]
struct Mailbox {
    incoming: Mutex<Option<Box<Mix>>>,
    returned: Mutex<Option<Box<Mix>>>,
}

/// The output callback's half: `Send`. Move it into the audio output closure.
pub struct VoiceOut {
    render_ref: Option<Arc<RenderRefRing>>,
    ref_resampler: Resampler,
    ref_scratch: [f32; CHUNK],
    mix: Box<Mix>,
    mailbox: Arc<Mailbox>,
    /// A replaced mix the return slot had no room for yet. It goes back before a new one is taken.
    give_back: Option<Box<Mix>>,
    scratch: [f32; CHUNK],
    sum: [f32; CHUNK],
}

/// The network worker's control of the output callback's peers.
pub struct MixControl {
    mailbox: Arc<Mailbox>,
    pending: Option<Box<Mix>>,
}

pub fn voice_out() -> (VoiceOut, MixControl) {
    voice_out_with_ref(None)
}

pub fn voice_out_with_ref(render_ref: Option<Arc<RenderRefRing>>) -> (VoiceOut, MixControl) {
    let mailbox = Arc::new(Mailbox::default());
    // A platform's mutex may set itself up on first use: do that here, off the audio thread.
    drop(mailbox.incoming.lock());
    drop(mailbox.returned.lock());
    let out = VoiceOut { render_ref, ref_resampler: Resampler::new(), ref_scratch: [0.0; CHUNK],
        mix: Box::new(Mix { peers: Vec::new() }), mailbox: mailbox.clone(), give_back: None, scratch: [0.0; CHUNK], sum: [0.0; CHUNK] };
    (out, MixControl { mailbox, pending: None })
}

impl MixControl {
    /// The peers to play from now on. Each one's playout is fresh here; a peer the callback already
    /// plays keeps its own (the callback moves it across), so its voice carries on.
    pub fn set(&mut self, peers: impl IntoIterator<Item = (PeerId, u8, Arc<JitterRing>, Arc<VoiceMarks>, Arc<SourceLevel>)>) {
        let peers = peers.into_iter().map(|(peer, stream, ring, marks, level)| MixPeer { peer, stream, level, ring, playout: Playout::new(playout_config()), marks }).collect();
        self.pending = Some(Box::new(Mix { peers }));
        self.tick();
    }

    /// Call at every wake: frees what the callback gave back, and hands over a pending set.
    pub fn tick(&mut self) {
        let returned = self.mailbox.returned.try_lock().ok().and_then(|mut slot| slot.take());
        drop(returned);
        if self.pending.is_some() {
            let older = match self.mailbox.incoming.try_lock() {
                // An older set the callback never took is replaced; it is freed here, unlocked.
                Ok(mut slot) => std::mem::replace(&mut *slot, self.pending.take()),
                Err(_) => None,
            };
            drop(older);
        }
    }
}

impl VoiceOut {
    pub(super) fn received_ring(&mut self, peer: PeerId, stream: u8) -> Option<&JitterRing> {
        self.take_new_mix();
        let p = self.mix.peers.iter().find(|p| p.peer == peer && p.stream == stream)?;
        p.ring.is_synced().then_some(&p.ring)
    }

    pub(super) fn received_seq(&mut self, peer: PeerId, stream: u8) -> Option<u32> {
        self.received_ring(peer, stream).map(JitterRing::newest_seq)
    }

    fn take_new_mix(&mut self) {
        if let Some(old) = self.give_back.take() {
            match self.mailbox.returned.try_lock() {
                Ok(mut slot) if slot.is_none() => *slot = Some(old),
                _ => {
                    self.give_back = Some(old);
                    return;
                }
            }
        }
        let Some(mut new) = self.mailbox.incoming.try_lock().ok().and_then(|mut slot| slot.take()) else { return };
        for old in &mut self.mix.peers {
            if let Some(p) = new.peers.iter_mut().find(|p| {
                p.peer == old.peer && p.stream == old.stream && Arc::ptr_eq(&p.ring, &old.ring)
            }) {
                std::mem::swap(&mut p.playout, &mut old.playout);
            } else if !new.peers.iter().any(|p| Arc::ptr_eq(&p.marks, &old.marks)) {
                old.marks.set(0.0, false);
            }
        }
        let old = std::mem::replace(&mut self.mix, new);
        match self.mailbox.returned.try_lock() {
            Ok(mut slot) if slot.is_none() => *slot = Some(old),
            _ => self.give_back = Some(old),
        }
    }

    /// Mixes every audible peer into `self.sum[..n]` at the device rate.
    fn render_chunk(&mut self, rate: f64, n: usize) {
        self.sum[..n].fill(0.0);
        for p in &mut self.mix.peers {
            if !p.playout.wants_render(&p.ring) {
                p.marks.set(0.0, false);
                continue;
            }
            let s = &mut self.scratch[..n];
            p.playout.render(&p.ring, rate, s);
            let volume = p.level.volume();
            for (o, x) in self.sum[..n].iter_mut().zip(s.iter()) {
                *o += x * volume;
            }
            p.marks.set(rms(s), p.playout.is_talking());
        }
        for sample in &mut self.sum[..n] {
            *sample = sample.clamp(-1.0, 1.0);
        }
        if let Some(ring) = &self.render_ref {
            // Silence advances the render clock too. Flush during upsampling as
            // well as at the end of the device block.
            self.ref_resampler.set_ratio(rate.max(1000.0), VOICE_RATE, 0.0);
            let mut fill = 0;
            for &x in &self.sum[..n] {
                self.ref_resampler.push(x, |y| {
                    self.ref_scratch[fill] = y;
                    fill += 1;
                    if fill == CHUNK {
                        ring.push_mono(&self.ref_scratch);
                        fill = 0;
                    }
                });
            }
            ring.push_mono(&self.ref_scratch[..fill]);
        }
    }

    /// Adds the mix into a mono block.
    pub fn mix_into_mono(&mut self, rate: f64, out: &mut [f32]) {
        self.take_new_mix();
        for block in out.chunks_mut(CHUNK) {
            self.render_chunk(rate, block.len());
            for (o, x) in block.iter_mut().zip(&self.sum) {
                *o += x;
            }
        }
    }

    /// Adds the mix to every channel of a channel-major block (`AudioBuffer`'s layout).
    pub fn mix_into_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &mut [f32]) {
        let channels = channels.max(1);
        if data.len() < frames * channels {
            return;
        }
        self.take_new_mix();
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(CHUNK);
            self.render_chunk(rate, n);
            for c in 0..channels {
                let at = c * frames + done;
                for (o, x) in data[at..at + n].iter_mut().zip(&self.sum[..n]) {
                    *o += x;
                }
            }
            done += n;
        }
    }

    /// Adds the mix to every channel of an interleaved block.
    pub fn mix_into_interleaved(&mut self, rate: f64, channels: usize, data: &mut [f32]) {
        let channels = channels.max(1);
        self.take_new_mix();
        let frames = data.len() / channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(CHUNK);
            self.render_chunk(rate, n);
            for (frame, x) in data[done * channels..(done + n) * channels].chunks_exact_mut(channels).zip(&self.sum[..n]) {
                for o in frame {
                    *o += x;
                }
            }
            done += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(phase: &mut f32, hz: f32, rate: f32, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = 0.3 * (*phase * std::f32::consts::TAU).sin();
            *phase = (*phase + hz / rate).fract();
        }
    }

    struct SpeechPeer {
        ring: Arc<JitterRing>,
        decoder: VoiceDecoder,
        encoder: AdpcmState,
        seed: u32,
        speech: f32,
        packet: Vec<u8>,
    }

    impl SpeechPeer {
        fn new(seed: u32) -> Self {
            Self { ring: Arc::new(JitterRing::new()), decoder: VoiceDecoder::default(),
                encoder: AdpcmState::default(), seed, speech: 0.0, packet: Vec::new() }
        }

        fn send(&mut self, seq: u32, level: impl Fn(f32) -> f32) {
            let pcm = std::array::from_fn::<_, VOICE_FRAME, _>(|_| {
                self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
                self.speech = 0.85 * self.speech + 0.225 * (2.0 * (self.seed >> 8) as f32 / 16777216.0 - 1.0);
                level(self.speech)
            });
            self.packet.clear();
            adpcm_encode_into(&mut self.encoder, 4, &pcm, &mut self.packet);
            self.decoder.insert(&self.ring, seq, 0, &self.packet);
        }
    }

    #[derive(Default)]
    struct EchoMeasurement {
        uplink: Vec<u8>,
        tapped: Vec<f32>,
        wire_energy: f64,
        tap_energy: f64,
        frames: usize,
        muted_frames: usize,
    }

    impl EchoMeasurement {
        fn drain(&mut self, frames: &VoiceFrames, tap: &TranscriptTap, is_muted: bool, measure: bool) {
            while let Some(frame) = frames.pop(&mut self.uplink) {
                let t = tap.pop(&mut self.tapped).expect("every uplink frame reaches the transcript tap");
                assert_eq!(t.silent, frame.flags & voice_flags::SILENT != 0, "tap and wire agree on silence");
                if is_muted {
                    assert!(t.silent && self.uplink.is_empty() && self.tapped.is_empty(), "muted frames must carry no audio on either path");
                    self.muted_frames += 1;
                }
                if measure {
                    self.frames += 1;
                    if !t.silent {
                        let decoded = adpcm_decode(&self.uplink).expect("voiced uplink is decodable");
                        self.wire_energy += decoded.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                        self.tap_energy += self.tapped.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                    }
                }
            }
        }

        fn rms(&self) -> (f64, f64) {
            let count = (self.frames * VOICE_FRAME) as f64;
            ((self.wire_energy / count).sqrt(), (self.tap_energy / count).sqrt())
        }
    }

    /// RMS on both sides of the encoder for a 50 ms acoustic echo at 48 kHz.
    /// Both ranges count 10 ms device blocks; silent frames count toward the RMS.
    fn echo_rms(software: bool, mute: std::ops::Range<usize>, measure: std::ops::Range<usize>) -> ((f64, f64), usize) {
        let (rms, frames, _) = echo_rms_with(software, 2400, 0, mute, measure);
        (rms, frames)
    }

    fn echo_rms_with(
        software: bool,
        delay_samples: usize,
        pre_render_blocks: usize,
        mute: std::ops::Range<usize>,
        measure: std::ops::Range<usize>,
    ) -> ((f64, f64), usize, u64) {
        let clock = Instant::now();
        let muted = Arc::new(AtomicBool::new(false));
        let reference = software.then(RenderRefRing::new);
        let (mut input, mut frames) = voice_in_with_aec(clock, muted.clone(), Arc::default(), reference.clone());
        let (mut output, mut control) = voice_out_with_ref(reference.clone());
        let tap = frames.transcript_tap().unwrap();
        tap.set_on(true);
        let mut peer = SpeechPeer::new(37);
        control.set([(1, stream::VOICE, peer.ring.clone(), Arc::default(), Arc::default())]);
        let mut measurement = EchoMeasurement::default();
        let mut delay = vec![0.0; delay_samples];
        for i in 0..measure.end {
            let is_muted = mute.contains(&i);
            muted.store(is_muted, Ordering::Relaxed);
            if i % 2 == 0 {
                peer.send((i / 2) as u32, |x| x);
            }
            let mut speaker = [0.0; 480];
            output.mix_into_mono(48_000.0, &mut speaker);
            let offset = (i * speaker.len()) % delay.len();
            let mic = &mut delay[offset..offset + speaker.len()];
            if i >= pre_render_blocks { input.push_mono(48_000.0, mic); }
            for (d, s) in mic.iter_mut().zip(speaker) { *d = 0.5 * s; }
            measurement.drain(&frames, &tap, is_muted, measure.contains(&i));
        }
        assert_eq!(measurement.muted_frames, mute.len() / 2, "both paths must deliver every muted frame");
        let overflows = reference.as_ref().map_or(0, |r| r.overflows());
        (measurement.rms(), measurement.frames, overflows)
    }

    #[test]
    fn voice_aec_recovers_after_closed_mic_mute_and_device_switch() {
        #[derive(Clone, Copy, Debug)]
        enum Route { LongMute, ShortMute, SameRate, NewRate }

        fn measured(route: Route, software: bool) -> (f64, f64) {
            let muted = Arc::new(AtomicBool::new(false));
            let reference = software.then(RenderRefRing::new);
            let (mut input, mut frames) = voice_in_with_aec(Instant::now(), muted.clone(), Arc::default(), reference.clone());
            let (mut output, mut control) = voice_out_with_ref(reference.clone());
            let tap = frames.transcript_tap().unwrap();
            tap.set_on(true);
            let mut peer = SpeechPeer::new(37);
            control.set([(1, stream::VOICE, peer.ring.clone(), Arc::default(), Arc::default())]);
            let (gap, reopen, new_delay, rate) = match route {
                Route::LongMute => (70, 8, 1200, 48_000.0),
                Route::ShortMute => (25, 0, 1440, 48_000.0),
                Route::SameRate => (4, 0, 960, 48_000.0),
                Route::NewRate => (0, 0, 882, 44_100.0),
            };
            let closed_mic = matches!(route, Route::LongMute | Route::ShortMute);
            let resume = 150 + gap + reopen;
            let mut delay = vec![0.0; 2400];
            let mut delay_pos = 0;
            let mut measurement = EchoMeasurement::default();
            for i in 0..resume + 200 {
                if i == 150 && closed_mic {
                    if let Some(r) = &reference { r.set_capture_muted(true); }
                    muted.store(true, Ordering::Release);
                }
                if i == 150 && matches!(route, Route::SameRate) {
                    if let Some(r) = &reference { r.request_reset(); }
                }
                if i == 150 + gap && closed_mic {
                    if let Some(r) = &reference { r.set_capture_muted(false); }
                    muted.store(false, Ordering::Release);
                }
                if i == resume {
                    if let Some(r) = &reference {
                        assert_eq!(r.overflows(), 0, "{route:?} must stay quiet before capture resumes");
                        match route {
                            Route::LongMute => assert!(r.paused_overflows() > 0, "the long mute must overflow during the pause"),
                            Route::ShortMute => {
                                assert_eq!(r.available(), 4000, "250 ms of render must be queued before the next mix");
                                assert_eq!(r.paused_overflows(), 0, "the short gap must be detected without overflow");
                            }
                            _ => {}
                        }
                    }
                    delay = vec![0.0; new_delay];
                    delay_pos = 0;
                }
                let device_rate = if i < resume { 48_000.0 } else { rate };
                if i % 2 == 0 { peer.send((i / 2) as u32, |x| x); }
                let mut block = [0.0; 480];
                let samples = &mut block[..(device_rate / 100.0) as usize];
                output.mix_into_mono(device_rate, samples);
                for s in samples.iter_mut() {
                    *s = std::mem::replace(&mut delay[delay_pos], 0.5 * *s);
                    delay_pos = (delay_pos + 1) % delay.len();
                }
                if i < 150 || i >= resume { input.push_mono(device_rate, samples); }
                measurement.drain(&frames, &tap, false, i >= resume + 150);
            }
            if let Some(r) = &reference {
                assert_eq!(r.overflows(), 0, "{route:?} must recover without active overflow");
            }
            if matches!(route, Route::NewRate) {
                assert!(measurement.frames >= 24, "the rate switch must deliver at least 24 measurement frames");
            } else {
                assert_eq!(measurement.frames, 25, "{route:?} must deliver every measurement frame");
            }
            measurement.rms()
        }

        for route in [Route::LongMute, Route::ShortMute, Route::SameRate, Route::NewRate] {
            let (wire_off, tap_off) = measured(route, false);
            let (wire_on, tap_on) = measured(route, true);
            assert!(wire_off > 0.05 && tap_off > 0.05, "{route:?} must have audible baseline echo: {wire_off}, {tap_off}");
            let wire_drop = 20.0 * (wire_off / wire_on).log10();
            let tap_drop = 20.0 * (tap_off / tap_on).log10();
            assert!(wire_drop >= 15.0 && tap_drop >= 15.0, "{route:?} recovery dropped uplink by {wire_drop} dB and transcript by {tap_drop} dB");
        }
    }

    #[test]
    fn voice_aec_converges_on_low_latency_echo_after_speaker_opens_first() {
        let ((wire_off, tap_off), frames_off, _) = echo_rms_with(false, 960, 10, 0..0, 160..210);
        let ((wire_on, tap_on), frames_on, overflows) = echo_rms_with(true, 960, 10, 0..0, 160..210);
        assert_eq!((frames_off, frames_on), (25, 25), "both runs must deliver the full measurement window");
        assert_eq!(overflows, 0, "a 100 ms speaker lead must fit in the render ring");
        assert!(wire_off > 0.05 && tap_off > 0.05, "low-latency echo must be audible with cancellation off: {wire_off}, {tap_off}");
        let wire_drop = 20.0 * (wire_off / wire_on).log10();
        let tap_drop = 20.0 * (tap_off / tap_on).log10();
        assert!(wire_drop >= 15.0 && tap_drop >= 15.0, "after speaker pre-render, uplink dropped by {wire_drop} dB and transcript by {tap_drop} dB");
    }

    #[test]
    fn voice_out_clamps_multi_peer_mix_for_dac_and_aec_reference() {
        fn measured(software: bool) -> (f64, f64) {
            let reference = software.then(RenderRefRing::new);
            let (mut input, mut frames) = voice_in_with_aec(Instant::now(), Arc::default(), Arc::default(), reference.clone());
            let (mut output, mut control) = voice_out_with_ref(reference);
            let tap = frames.transcript_tap().unwrap();
            tap.set_on(true);
            let mut peers = [37, 73, 109, 149].map(SpeechPeer::new);
            control.set(peers.iter().enumerate().map(|(i, peer)| {
                (i as PeerId + 1, stream::VOICE, peer.ring.clone(), Arc::default(), Arc::default())
            }));
            let mut delay = [0.0; 2400];
            let mut measurement = EchoMeasurement::default();
            let mut in_range = true;
            let mut saturated = false;
            let mut peak = 0.0_f32;
            for i in 0..200 {
                if i % 2 == 0 {
                    for peer in &mut peers {
                        peer.send((i / 2) as u32, |x| (2.5 * x).clamp(-0.95, 0.95));
                    }
                }
                let mut speaker = [0.0; 480];
                output.mix_into_mono(48_000.0, &mut speaker);
                let offset = i * speaker.len() % delay.len();
                let mic = &mut delay[offset..offset + speaker.len()];
                input.push_mono(48_000.0, mic);
                for (d, s) in mic.iter_mut().zip(speaker) {
                    in_range &= (-1.0..=1.0).contains(&s);
                    saturated |= s.abs() == 1.0;
                    peak = peak.max(s.abs());
                    *d = 0.5 * s.clamp(-1.0, 1.0);
                }
                measurement.drain(&frames, &tap, false, i >= 150);
            }
            assert!(in_range, "four-peer speaker mix exceeded the DAC range: peak {peak}");
            assert!(saturated, "four overlapping peers must drive the mix to full scale");
            assert_eq!(measurement.frames, 25, "the four-peer echo must deliver all 25 measurement frames");
            measurement.rms()
        }
        let (wire_off, tap_off) = measured(false);
        let (wire_on, tap_on) = measured(true);
        assert!(wire_off > 0.05 && tap_off > 0.05, "the clipped speaker echo must be audible with cancellation off: {wire_off}, {tap_off}");
        let wire_drop = 20.0 * (wire_off / wire_on).log10();
        let tap_drop = 20.0 * (tap_off / tap_on).log10();
        assert!(wire_drop >= 15.0 && tap_drop >= 15.0, "four-peer uplink dropped by {wire_drop} dB and transcript by {tap_drop} dB");
    }

    #[test]
    fn voice_worker_feeds_aec_before_transcription_tap() {
        fn measured_rms(software: bool) -> (f64, f64) {
            let (rms, frame_count) = echo_rms(software, 0..0, 150..200);
            assert_eq!(frame_count, 25, "capture emits every 20 ms frame in the measurement window");
            rms
        }
        let (wire_off, tap_off) = measured_rms(false);
        let (wire_on, tap_on) = measured_rms(true);
        assert!(wire_off > 0.05 && tap_off > 0.05, "echo must be audible with cancellation off: {wire_off}, {tap_off}");
        let wire_drop = 20.0 * (wire_off / wire_on).log10();
        let tap_drop = 20.0 * (tap_off / tap_on).log10();
        assert!(wire_drop >= 15.0, "encoded uplink echo dropped by {wire_drop} dB");
        assert!(tap_drop >= 15.0, "transcript tap echo dropped by {tap_drop} dB");
    }

    #[test]
    fn voice_aec_cancels_immediately_after_unmute() {
        let ((wire_off, tap_off), frames_off) = echo_rms(false, 150..200, 200..220);
        let ((wire_on, tap_on), frames_on) = echo_rms(true, 150..200, 200..220);
        assert_eq!((frames_off, frames_on), (10, 10), "both runs must deliver all ten post-unmute frames");
        assert!(wire_off > 0.05 && tap_off > 0.05, "unmuted echo must be audible with cancellation off: {wire_off}, {tap_off}");
        let wire_drop = 20.0 * (wire_off / wire_on).log10();
        let tap_drop = 20.0 * (tap_off / tap_on).log10();
        assert!(wire_drop >= 15.0, "uplink echo in the first 200 ms after unmute dropped by {wire_drop} dB");
        assert!(tap_drop >= 15.0, "transcript echo in the first 200 ms after unmute dropped by {tap_drop} dB");
    }

    /// Voice from an input callback, through the ring and a decoder, into a peer's jitter buffer,
    /// and out of the output callback: the tone arrives, mute silences it, and a new set of peers
    /// mid-stream keeps the staying peer's playout (no gap in its tone).
    #[test]
    fn voice_flows_from_input_to_output_and_a_new_mix_keeps_the_playout() {
        let marks_me = Arc::new(VoiceMarks::default());
        let muted = Arc::new(AtomicBool::new(false));
        let (mut input, mut frames) = voice_in(Instant::now(), muted.clone(), marks_me.clone());
        let tap = frames.transcript_tap().unwrap();
        let mut samples = vec![1.0];
        input.push_mono(VOICE_RATE, &[0.0; VOICE_FRAME]);
        assert!(tap.pop(&mut samples).is_none() && samples.is_empty(), "the disabled tap writes nothing");
        frames.pop(&mut Vec::new()).expect("the disabled tap still lets voice through");
        tap.set_on(true);
        let (mut output, mut control) = voice_out();
        let ring = Arc::new(JitterRing::new());
        let peer_marks = Arc::new(VoiceMarks::default());
        let level = Arc::new(SourceLevel::default());
        control.set([(1, stream::VOICE, ring.clone(), peer_marks.clone(), level.clone())]);
        let mut decoder = VoiceDecoder::default();
        let (mut phase, mut block, mut out) = (0f32, [0f32; 480], [0f32; 480]);
        let mut payload = Vec::new();
        let (mut sent, mut silent, mut loud, mut quiet_after_loud) = (0, 0, 0, 0);
        for n in 0..300 {
            if n == 200 {
                muted.store(true, Ordering::Relaxed);
            }
            if n == 100 {
                // the same peer and a second one: the first keeps its playout
                control.set([(1, stream::VOICE, ring.clone(), peer_marks.clone(), level.clone()), (2, stream::VOICE, Arc::new(JitterRing::new()), Arc::new(VoiceMarks::default()), Arc::default())]);
            }
            tone(&mut phase, 440.0, 48_000.0, &mut block);
            input.push_mono(48_000.0, &block);
            while let Some(f) = frames.pop(&mut payload) {
                let tapped = tap.pop(&mut samples).expect("each sent frame has a microphone frame");
                assert_eq!(tapped, TapFrame {
                    silent: f.flags & voice_flags::SILENT != 0,
                    talk_start: f.flags & voice_flags::TALK_START != 0,
                    capture_us: f.capture_us,
                }, "tap order, gate and timestamp must match voice frame {}", f.seq);
                assert_eq!(samples.len(), if tapped.silent { 0 } else { VOICE_FRAME }, "only voiced frames carry PCM");
                if n >= 202 {
                    assert!(tapped.silent, "the muted microphone must supply only silence");
                }
                sent += 1;
                silent += (f.flags & voice_flags::SILENT != 0) as u32;
                decoder.insert(&ring, f.seq, f.flags, &payload);
            }
            control.tick();
            out.fill(0.0);
            output.mix_into_mono(48_000.0, &mut out);
            let level = rms(&out);
            if (20..200).contains(&n) {
                if level > 0.1 {
                    loud += 1;
                } else if loud > 0 {
                    quiet_after_loud += 1;
                }
            }
        }
        drop(tap);
        assert!(!frames.queue.tap_on.load(Ordering::Acquire), "dropping the reader disables the callback tap");
        assert!(sent >= 145, "300 blocks of 10 ms make 150 frames of 20 ms, the ring gave {sent}");
        assert!(silent >= 40, "100 muted blocks must send silent frames, {silent} were");
        assert!(loud >= 170, "the tone must play from the second 200 ms on, it did in {loud} of 180 blocks");
        assert_eq!(quiet_after_loud, 0, "the new mix at block 100 must not interrupt the staying peer's tone");
        assert!(peer_marks.level() == 0.0 || !muted.load(Ordering::Relaxed), "marks are read without locks");
        assert_eq!(frames.dropped(), 0, "a ring drained every block drops nothing");
    }

    /// A ring the worker doesn't drain drops the newest frames, counted, and never more than it holds.
    #[test]
    fn a_full_voice_ring_drops_and_counts() {
        let (mut input, frames) = voice_in(Instant::now(), Arc::new(AtomicBool::new(false)), Arc::new(VoiceMarks::default()));
        let (mut phase, mut block) = (0f32, [0f32; 480]);
        for _ in 0..200 {
            tone(&mut phase, 300.0, 48_000.0, &mut block);
            input.push_mono(48_000.0, &block);
        }
        let mut payload = Vec::new();
        let mut seqs = Vec::new();
        while let Some(f) = frames.pop(&mut payload) {
            seqs.push(f.seq);
        }
        assert_eq!(seqs.len(), QUEUE, "the ring holds {QUEUE} frames");
        assert_eq!(seqs, (0..QUEUE as u32).collect::<Vec<_>>(), "the oldest frames stay, in order");
        assert_eq!(frames.dropped(), 100 - QUEUE as u64, "every frame past the ring's room is counted");
        let mut frames = frames;
        let tap = frames.transcript_tap().unwrap();
        tap.set_on(true);
        for _ in 0..2 * (TAP_FRAMES + 7) {
            input.push_mono(48_000.0, &block);
        }
        assert_eq!(tap.dropped(), 7, "the tap counts every overflow without waiting");
        let mut samples = Vec::new();
        let kept = std::iter::from_fn(|| tap.pop(&mut samples)).count();
        assert_eq!(kept, TAP_FRAMES, "a full tap retains exactly its capacity");
    }
}
