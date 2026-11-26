//! The audio callbacks' halves of a call. [`VoiceIn`] lives in the app's audio input callback: it
//! frames, gates and encodes the microphone and queues the frames for the network worker.
//! [`VoiceOut`] lives in the audio output callback: it plays every peer's jitter buffer and adds
//! the mix into the device block. Neither allocates, frees, blocks or takes a lock anyone waits
//! on. The hand-offs are a ring of atomic slots (voice frames out) and a mailbox both sides only
//! ever `try_lock` (a new set of peers in, the old set back).

use crate::wire::voice_flags;
use makepad_teamtalk::codec::{adpcm_decode, adpcm_encode_into, AdpcmState};
use makepad_teamtalk::jitter::{JitterRing, Playout};
use makepad_teamtalk::resample::Resampler;
use makepad_teamtalk::{Framer, FramerConfig, PlayoutConfig, INTERNAL_RATE, MAX_FRAME};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
        VoiceQueue { slots: (0..QUEUE).map(|_| slot()).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0), dropped: AtomicU64::new(0) }
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

/// The network worker's end of the input ring.
pub struct VoiceFrames {
    queue: Arc<VoiceQueue>,
}

impl VoiceFrames {
    /// The next frame, its encoded payload in `payload` (empty for a silent frame).
    pub fn pop(&self, payload: &mut Vec<u8>) -> Option<VoiceFrame> {
        self.queue.pop(payload)
    }

    /// Frames the input callback dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.queue.dropped.load(Ordering::Relaxed)
    }
}

/// The input callback's half: `Send`, not `Clone`. Move it into the audio input closure.
pub struct VoiceIn {
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
    let queue = Arc::new(VoiceQueue::new());
    let framer = Framer::new(FramerConfig { out_rate: VOICE_RATE, frame_samples: VOICE_FRAME, gate_threshold_rms: 0.01, gate_hangover_ms: 200 })
        .expect("the voice frame size is valid");
    let input = VoiceIn { framer, adpcm: AdpcmState::default(), payload: Vec::with_capacity(WORDS * 8), queue: queue.clone(), marks, muted, clock };
    (input, VoiceFrames { queue })
}

impl VoiceIn {
    /// The clock at the first sample of a block of `frames` that just arrived.
    fn block_us(&self, rate: f64, frames: usize) -> u64 {
        let now = self.clock.elapsed().as_micros() as u64;
        now.saturating_sub((frames as f64 * 1e6 / rate.max(1.0)) as u64)
    }

    fn begin(&mut self) {
        self.framer.set_muted(self.muted.load(Ordering::Relaxed));
    }

    pub fn push_mono(&mut self, rate: f64, samples: &[f32]) {
        self.begin();
        let block_us = self.block_us(rate, samples.len());
        let (adpcm, payload, queue, marks) = (&mut self.adpcm, &mut self.payload, &*self.queue, &*self.marks);
        self.framer.push_mono(rate, samples, block_us, &mut |f| emit(adpcm, payload, queue, marks, f));
    }

    /// A channel-major block (`AudioBuffer`'s layout), folded to mono.
    pub fn push_planar(&mut self, rate: f64, frames: usize, channels: usize, data: &[f32]) {
        self.begin();
        let block_us = self.block_us(rate, frames);
        let (adpcm, payload, queue, marks) = (&mut self.adpcm, &mut self.payload, &*self.queue, &*self.marks);
        self.framer.push_planar(rate, frames, channels, data, block_us, &mut |f| emit(adpcm, payload, queue, marks, f));
    }

    pub fn push_interleaved(&mut self, rate: f64, channels: usize, data: &[f32]) {
        self.begin();
        let block_us = self.block_us(rate, data.len() / channels.max(1));
        let (adpcm, payload, queue, marks) = (&mut self.adpcm, &mut self.payload, &*self.queue, &*self.marks);
        self.framer.push_interleaved(rate, channels, data, block_us, &mut |f| emit(adpcm, payload, queue, marks, f));
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
    let mailbox = Arc::new(Mailbox::default());
    // A platform's mutex may set itself up on first use: do that here, off the audio thread.
    drop(mailbox.incoming.lock());
    drop(mailbox.returned.lock());
    let out = VoiceOut { mix: Box::new(Mix { peers: Vec::new() }), mailbox: mailbox.clone(), give_back: None, scratch: [0.0; CHUNK], sum: [0.0; CHUNK] };
    (out, MixControl { mailbox, pending: None })
}

impl MixControl {
    /// The peers to play from now on. Each one's playout is fresh here; a peer the callback already
    /// plays keeps its own (the callback moves it across), so its voice carries on.
    pub fn set(&mut self, peers: impl IntoIterator<Item = (PeerId, Arc<JitterRing>, Arc<VoiceMarks>)>) {
        let peers = peers.into_iter().map(|(peer, ring, marks)| MixPeer { peer, ring, playout: Playout::new(playout_config()), marks }).collect();
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
        for p in &mut new.peers {
            if let Some(old) = self.mix.peers.iter_mut().find(|o| o.peer == p.peer) {
                std::mem::swap(&mut p.playout, &mut old.playout);
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
            for (o, x) in self.sum[..n].iter_mut().zip(s.iter()) {
                *o += x;
            }
            p.marks.set(rms(s), p.playout.is_talking());
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

    /// Voice from an input callback, through the ring and a decoder, into a peer's jitter buffer,
    /// and out of the output callback: the tone arrives, mute silences it, and a new set of peers
    /// mid-stream keeps the staying peer's playout (no gap in its tone).
    #[test]
    fn voice_flows_from_input_to_output_and_a_new_mix_keeps_the_playout() {
        let marks_me = Arc::new(VoiceMarks::default());
        let muted = Arc::new(AtomicBool::new(false));
        let (mut input, frames) = voice_in(Instant::now(), muted.clone(), marks_me.clone());
        let (mut output, mut control) = voice_out();
        let ring = Arc::new(JitterRing::new());
        let peer_marks = Arc::new(VoiceMarks::default());
        control.set([(1, ring.clone(), peer_marks.clone())]);
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
                control.set([(1, ring.clone(), peer_marks.clone()), (2, Arc::new(JitterRing::new()), Arc::new(VoiceMarks::default()))]);
            }
            tone(&mut phase, 440.0, 48_000.0, &mut block);
            input.push_mono(48_000.0, &block);
            while let Some(f) = frames.pop(&mut payload) {
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
    }
}
