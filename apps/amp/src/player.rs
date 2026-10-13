//! Local files play here, through the same equalizer and output level as
//! the routed player, then our volume (`audio.rs`).
//!
//! * The **decoder** worker reads the file, decodes it progressively
//!   (`makepad-audio-decode`) and writes stereo frames at the file's own
//!   rate into a lock-free ring of atomics shared with the audio thread. A
//!   full ring makes it wait; a new command interrupts it.
//! * The **audio callback** ([`Output`]) takes frames from the ring,
//!   resamples them to the device rate (linear interpolation, enough for
//!   the 44.1/48 kHz step), runs the chain and writes every output channel
//!   (stereo onto each pair). It allocates nothing, takes no lock and never
//!   waits: an empty ring plays silence.
//!
//! A new track or a seek does not wait for the audio thread either: the
//! decoder marks everything written so far as skipped (`skip_to`) and the
//! callback jumps over it on its next block. The position is counted from
//! where the current track's frames start in the ring (`base_frame`, at
//! `base_ms` into the track), set as soon as a play is taken, before the
//! file is read, and tagged with the play's `seq` so the UI never shows the
//! previous track's position for the new one. When the last frame has been
//! played the decoder says so ([`Message::LocalEnded`]), which moves our
//! queue on.
//!
//! Commands waiting for the decoder collapse to the newest (a dragged seek
//! bar sends many), and the bytes of the file last played are kept, so a
//! seek decodes again from memory instead of reading the file again.

use crate::worker::{send, Message};
use makepad_audio_decode::flac::FlacDecoder;
use makepad_audio_decode::mp3::Mp3Decoder;
use makepad_audio_decode::vorbis::VorbisDecoder;
use makepad_audio_decode::{sniff, AudioError, AudioFormat};
use makepad_audio_route::{FrameInfo, Processor};
use makepad_widgets::AudioBuffer;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

/// Stereo frames the ring holds: about three seconds at 44.1 kHz.
const RING_FRAMES: u64 = 1 << 17;
/// Frames the callback processes at a time (its scratch is this big).
const BLOCK: usize = 4096;
/// The decoder looks at the ring (and for commands) this often while it
/// waits for room or for the track to play out.
const WAIT: Duration = Duration::from_millis(5);
/// A file bigger than this is not read.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// What the decoder worker is asked.
pub enum PlayerCommand {
    /// Play `path` from `start_ms`; `generation` comes back with its end.
    /// `seq` numbers the command, for [`Shared::position_for`].
    Play { path: String, start_ms: u32, generation: u64, seq: u64 },
    Stop,
}

/// What the decoder and the audio thread share.
pub struct Shared {
    /// Interleaved stereo, `f32` bits.
    ring: Box<[AtomicU32]>,
    /// Frames written and read since the start; `write - read` are waiting.
    write: AtomicU64,
    read: AtomicU64,
    /// Frames before this are stale (a stopped track): the reader jumps.
    skip_to: AtomicU64,
    paused: AtomicBool,
    /// The file's own rate, its channels and its size in bytes (the small
    /// window's kHz / stereo / kbps readouts).
    rate: AtomicU32,
    channels: AtomicU32,
    file_bytes: AtomicU64,
    /// Where the current track's frames start in the ring, and how far into
    /// the track that is.
    base_frame: AtomicU64,
    base_ms: AtomicU64,
    /// The `seq` of the play the base belongs to.
    base_seq: AtomicU64,
    /// Sound is processed (the visualizer sees it) but not played: test
    /// runs set `MUSIC_MUTE` so they never sound on the machine's speakers.
    muted: bool,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ring: (0..RING_FRAMES * 2).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            skip_to: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            rate: AtomicU32::new(44_100),
            channels: AtomicU32::new(0),
            file_bytes: AtomicU64::new(0),
            base_frame: AtomicU64::new(0),
            base_ms: AtomicU64::new(0),
            base_seq: AtomicU64::new(0),
            muted: std::env::var_os("MUSIC_MUTE").is_some(),
        })
    }

    /// The playing file: its rate (Hz), channels and size (bytes); 0
    /// channels until its first block is decoded.
    pub fn file_info(&self) -> (u32, u32, u64) {
        (self.rate.load(Ordering::Relaxed), self.channels.load(Ordering::Relaxed), self.file_bytes.load(Ordering::Relaxed))
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Where the playing track is, in ms.
    pub fn position_ms(&self) -> u64 {
        let rate = self.rate.load(Ordering::Relaxed).max(1) as u64;
        let played = self.read.load(Ordering::Relaxed).saturating_sub(self.base_frame.load(Ordering::Relaxed));
        self.base_ms.load(Ordering::Relaxed) + played * 1000 / rate
    }

    /// Where play `seq` is, in ms; `None` until the decoder has taken it
    /// (until then the count is still the previous play's).
    pub fn position_for(&self, seq: u64) -> Option<u64> {
        (self.base_seq.load(Ordering::Acquire) == seq).then(|| self.position_ms())
    }

    /// A play starts at `start_ms`: what was written is stale and the
    /// position counts from here.
    fn begin(&self, start_ms: u32, seq: u64) {
        let base = self.write.load(Ordering::Relaxed);
        self.skip_to.store(base, Ordering::Release);
        self.base_frame.store(base, Ordering::Relaxed);
        self.base_ms.store(start_ms as u64, Ordering::Relaxed);
        self.base_seq.store(seq, Ordering::Release);
    }

    /// Everything written so far is stale: the reader skips it.
    fn flush(&self) {
        self.skip_to.store(self.write.load(Ordering::Relaxed), Ordering::Release);
    }

    /// Frames the ring has room for now. What lies before `skip_to` is
    /// room already, whether or not the reader got there.
    fn room(&self) -> u64 {
        let read = self.read.load(Ordering::Acquire).max(self.skip_to.load(Ordering::Acquire));
        RING_FRAMES - (self.write.load(Ordering::Relaxed) - read).min(RING_FRAMES)
    }

    fn put(&self, at: u64, frame: [f32; 2]) {
        let slot = ((at % RING_FRAMES) * 2) as usize;
        self.ring[slot].store(frame[0].to_bits(), Ordering::Relaxed);
        self.ring[slot + 1].store(frame[1].to_bits(), Ordering::Relaxed);
    }

    fn get(&self, at: u64) -> [f32; 2] {
        let slot = ((at % RING_FRAMES) * 2) as usize;
        [f32::from_bits(self.ring[slot].load(Ordering::Relaxed)), f32::from_bits(self.ring[slot + 1].load(Ordering::Relaxed))]
    }
}

/// The audio thread's side: the ring's reader, the resampler and the chain.
pub struct Output {
    shared: Arc<Shared>,
    chain: Box<dyn Processor>,
    scratch: Box<[f32]>,
    reader: Reader,
}

impl Output {
    pub fn new(shared: Arc<Shared>, chain: Box<dyn Processor>) -> Self {
        Self { shared, chain, scratch: vec![0.0; BLOCK * 2].into_boxed_slice(), reader: Reader::default() }
    }

    /// Fill one device buffer. Runs on the audio thread.
    pub fn fill(&mut self, device_rate: f64, buffer: &mut AudioBuffer) {
        let frames = buffer.frame_count();
        let channels = buffer.channel_count();
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(BLOCK);
            let block = &mut self.scratch[..count * 2];
            self.reader.pull(&self.shared, device_rate, block);
            self.chain.process(block, &FrameInfo { sample_rate: device_rate, channels: 2, frames: count, host_time: 0 });
            for channel in 0..channels {
                let out = &mut buffer.channel_mut(channel)[done..done + count];
                if self.shared.muted {
                    out.fill(0.0);
                } else {
                    for (frame, sample) in out.iter_mut().enumerate() {
                        *sample = device_sample(block, frame, channel, channels);
                    }
                }
            }
            done += count;
        }
    }
}

/// What output `channel` of `channels` plays of stereo `frame`: a mono
/// device gets both sides mixed, more than two channels get the pair again
/// on every pair.
fn device_sample(block: &[f32], frame: usize, channel: usize, channels: usize) -> f32 {
    if channels == 1 {
        (block[frame * 2] + block[frame * 2 + 1]) * 0.5
    } else {
        block[frame * 2 + channel % 2]
    }
}

/// The ring's reader on the audio thread: the two source frames the output
/// falls between and where, and the filter that keeps a higher source rate
/// from aliasing.
#[derive(Default)]
struct Reader {
    now: [f32; 2],
    next: [f32; 2],
    phase: f64,
    primed: bool,
    alias: AntiAlias,
}

impl Reader {
    /// Take `out.len() / 2` stereo frames from the ring at the device rate.
    fn pull(&mut self, shared: &Shared, device_rate: f64, out: &mut [f32]) {
        let write = shared.write.load(Ordering::Acquire);
        let mut read = shared.read.load(Ordering::Relaxed);
        let skip = shared.skip_to.load(Ordering::Acquire);
        if skip > read {
            read = skip.min(write);
            self.primed = false;
        }
        if shared.paused.load(Ordering::Relaxed) {
            out.fill(0.0);
            shared.read.store(read, Ordering::Release);
            return;
        }
        let source_rate = shared.rate.load(Ordering::Relaxed);
        self.alias.tune(source_rate, device_rate);
        let step = source_rate as f64 / device_rate.max(1.0);
        for frame in out.chunks_exact_mut(2) {
            if !self.primed {
                // One frame is enough to start: waiting for two would
                // leave a track's very last frame unplayed for ever, and
                // its end never reported.
                if read < write {
                    self.now = self.alias.apply(shared.get(read));
                    read += 1;
                    self.next = if read < write {
                        read += 1;
                        self.alias.apply(shared.get(read - 1))
                    } else {
                        self.now
                    };
                    self.phase = 0.0;
                    self.primed = true;
                } else {
                    frame.fill(0.0);
                    continue;
                }
            }
            let t = self.phase as f32;
            frame[0] = self.now[0] + (self.next[0] - self.now[0]) * t;
            frame[1] = self.now[1] + (self.next[1] - self.now[1]) * t;
            self.phase += step;
            while self.phase >= 1.0 {
                self.phase -= 1.0;
                self.now = self.next;
                if read < write {
                    self.next = self.alias.apply(shared.get(read));
                    read += 1;
                } else {
                    // Ran dry (the end, or the decoder behind): silence
                    // until frames come again.
                    self.primed = false;
                    break;
                }
            }
        }
        shared.read.store(read, Ordering::Release);
    }
}

/// A two-pole low-pass (Butterworth, RBJ) at the source rate, cutting just
/// under the device's Nyquist: on only when the source rate is higher
/// (96 or 192 kHz files on a 48 kHz device), where interpolating alone
/// would fold what the device cannot carry back down as noise. Tuned on
/// the audio thread when a rate changes; no allocation.
#[derive(Default)]
struct AntiAlias {
    rates: (u32, u64),
    on: bool,
    b: [f32; 3],
    a: [f32; 2],
    z: [[f32; 2]; 2],
}

impl AntiAlias {
    fn tune(&mut self, source_rate: u32, device_rate: f64) {
        let rates = (source_rate, device_rate.to_bits());
        if rates == self.rates {
            return;
        }
        self.rates = rates;
        self.z = [[0.0; 2]; 2];
        self.on = source_rate as f64 > device_rate * 1.02;
        if !self.on {
            return;
        }
        let w0 = std::f64::consts::TAU * (0.45 * device_rate) / source_rate as f64;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        self.b = [(b1 * 0.5) as f32, b1 as f32, (b1 * 0.5) as f32];
        self.a = [(-2.0 * cos / a0) as f32, ((1.0 - alpha) / a0) as f32];
    }

    fn apply(&mut self, frame: [f32; 2]) -> [f32; 2] {
        if !self.on {
            return frame;
        }
        let mut out = [0.0; 2];
        for (side, input) in frame.into_iter().enumerate() {
            let z = &mut self.z[side];
            let output = self.b[0] * input + z[0];
            z[0] = self.b[1] * input - self.a[0] * output + z[1];
            z[1] = self.b[2] * input - self.a[1] * output;
            out[side] = output;
        }
        out
    }
}

/// One of the three decoders, behind one call.
enum Decoder<'a> {
    Mp3(Mp3Decoder<'a>),
    Vorbis(VorbisDecoder<'a>),
    Flac(FlacDecoder<'a>),
}

impl<'a> Decoder<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self, AudioError> {
        Ok(match sniff(bytes).ok_or(AudioError::UnknownFormat)? {
            AudioFormat::Mp3 => Decoder::Mp3(Mp3Decoder::new(bytes)?),
            AudioFormat::OggVorbis => Decoder::Vorbis(VorbisDecoder::new(bytes)?),
            AudioFormat::Flac => Decoder::Flac(FlacDecoder::new(bytes)?),
            AudioFormat::Wav => return Err(AudioError::Unsupported("WAV in the streaming player")),
        })
    }

    fn rate(&self) -> u32 {
        match self {
            Decoder::Mp3(d) => d.rate(),
            Decoder::Vorbis(d) => d.rate(),
            Decoder::Flac(d) => d.rate(),
        }
    }

    /// The next interleaved block and its channel count; `None` at the end.
    fn next(&mut self) -> Result<Option<(usize, &[f32])>, AudioError> {
        Ok(match self {
            Decoder::Mp3(d) => d.next_frame()?.map(|frame| (frame.channels as usize, frame.pcm)),
            Decoder::Flac(d) => d.next_frame()?.map(|frame| (frame.channels as usize, frame.pcm)),
            Decoder::Vorbis(d) => {
                let channels = d.channels() as usize;
                d.next_block()?.map(|pcm| (channels, pcm))
            }
        })
    }
}

/// The newest of `first` and whatever waits behind it; `None` when the UI
/// hung up. A seek bar dragged, or tracks skipped quickly, send many
/// commands; only the last means anything once the decoder gets to them.
fn newest(first: PlayerCommand, rx: &Receiver<PlayerCommand>) -> Option<PlayerCommand> {
    let mut command = first;
    loop {
        match rx.try_recv() {
            Ok(next) => command = next,
            Err(TryRecvError::Empty) => return Some(command),
            Err(TryRecvError::Disconnected) => return None,
        }
    }
}

/// The decoder worker: plays what it is asked, one track at a time.
pub fn run_decoder(shared: Arc<Shared>, rx: Receiver<PlayerCommand>, tx: SyncSender<Message>) {
    let mut pending = None;
    // The file last played, kept for seeks within it.
    let mut kept: Option<(String, Vec<u8>)> = None;
    loop {
        let first = match pending.take() {
            Some(command) => command,
            None => match rx.recv() {
                Ok(command) => command,
                Err(_) => return,
            },
        };
        let Some(command) = newest(first, &rx) else { return };
        let PlayerCommand::Play { path, start_ms, generation, seq } = command else {
            shared.flush();
            continue;
        };
        shared.begin(start_ms, seq);
        if kept.as_ref().is_none_or(|(kept_path, _)| *kept_path != path) {
            kept = None;
            let bytes = match std::fs::metadata(&path) {
                Ok(meta) if meta.len() <= MAX_FILE_BYTES => std::fs::read(&path),
                Ok(_) => Err(std::io::Error::other("the file is too big")),
                Err(error) => Err(error),
            };
            match bytes {
                Ok(bytes) => kept = Some((path.clone(), bytes)),
                Err(error) => {
                    send(&tx, Message::LocalError { generation, error: format!("cannot read {path}: {error}") });
                    continue;
                }
            }
        }
        if let Some((_, bytes)) = &kept {
            pending = play(&shared, bytes, start_ms, generation, &rx, &tx);
        }
    }
}

/// Decode `bytes` into the ring from `start_ms`, then wait for it to play
/// out. Returns the command that interrupted it, if one did.
fn play(shared: &Shared, bytes: &[u8], start_ms: u32, generation: u64, rx: &Receiver<PlayerCommand>, tx: &SyncSender<Message>) -> Option<PlayerCommand> {
    // A command waiting ends this play; the decoder loop takes the newest.
    let interrupted = || match rx.try_recv() {
        Ok(command) => Some(Some(command)),
        Err(TryRecvError::Disconnected) => Some(None),
        Err(TryRecvError::Empty) => None,
    };
    let mut decoder = match Decoder::open(bytes) {
        Ok(decoder) => decoder,
        Err(error) => {
            send(tx, Message::LocalError { generation, error: format!("cannot play this file: {error:?}") });
            return None;
        }
    };
    let rate = decoder.rate().max(1);
    shared.rate.store(rate, Ordering::Relaxed);
    shared.file_bytes.store(bytes.len() as u64, Ordering::Relaxed);
    shared.channels.store(0, Ordering::Relaxed);
    // A seek decodes up to the start and drops it: the decoders have no
    // index to jump by, and decoding runs far faster than playing.
    let mut to_skip = start_ms as u64 * rate as u64 / 1000;
    loop {
        if let Some(command) = interrupted() {
            return command;
        }
        let (channels, pcm) = match decoder.next() {
            Ok(Some(block)) => block,
            Ok(None) => break,
            Err(error) => {
                send(tx, Message::LocalError { generation, error: format!("the file stopped decoding: {error:?}") });
                break;
            }
        };
        let channels = channels.max(1);
        shared.channels.store(channels as u32, Ordering::Relaxed);
        let frames = pcm.len() / channels;
        let skipped = to_skip.min(frames as u64) as usize;
        to_skip -= skipped as u64;
        let mut at = skipped;
        while at < frames {
            let room = shared.room() as usize;
            if room == 0 {
                if let Some(command) = interrupted() {
                    return command;
                }
                std::thread::sleep(WAIT);
                continue;
            }
            let end = frames.min(at + room);
            let mut write = shared.write.load(Ordering::Relaxed);
            for frame in pcm[at * channels..end * channels].chunks_exact(channels) {
                let stereo = if channels == 1 { [frame[0], frame[0]] } else { [frame[0], frame[1]] };
                shared.put(write, stereo);
                write += 1;
            }
            shared.write.store(write, Ordering::Release);
            at = end;
        }
    }
    // Played out when the reader has taken the last frame.
    loop {
        if let Some(command) = interrupted() {
            return command;
        }
        if shared.read.load(Ordering::Acquire) >= shared.write.load(Ordering::Acquire) {
            send(tx, Message::LocalEnded { generation });
            return None;
        }
        std::thread::sleep(WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        let shared = Shared::new();
        shared.rate.store(48_000, Ordering::Relaxed);
        shared
    }

    fn write(shared: &Shared, frames: &[[f32; 2]]) {
        let mut at = shared.write.load(Ordering::Relaxed);
        for frame in frames {
            shared.put(at, *frame);
            at += 1;
        }
        shared.write.store(at, Ordering::Release);
    }

    fn read(shared: &Shared, frames: usize, device_rate: f64, reader: &mut Reader) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        reader.pull(shared, device_rate, &mut out);
        out
    }

    /// At the file's own rate the frames come out as written; the position
    /// counts them from where the track starts in the ring.
    #[test]
    fn frames_come_out_in_order_and_count_the_position() {
        let shared = shared();
        let frames: Vec<[f32; 2]> = (0..8).map(|i| [i as f32, -(i as f32)]).collect();
        write(&shared, &frames);
        let mut state = Reader::default();
        let out = read(&shared, 4, 48_000.0, &mut state);
        assert_eq!(out, [0.0, 0.0, 1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        shared.base_ms.store(1000, Ordering::Relaxed);
        assert_eq!(shared.position_ms(), 1000);
    }

    /// At twice the device rate every other frame is taken; at half, the
    /// frames in between are interpolated.
    #[test]
    fn a_different_device_rate_is_resampled() {
        let shared = shared();
        let frames: Vec<[f32; 2]> = (0..16).map(|i| [i as f32; 2]).collect();
        write(&shared, &frames);
        let mut state = Reader::default();
        let out = read(&shared, 4, 96_000.0, &mut state);
        assert_eq!(out, [0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.5, 1.5]);
    }

    /// A flush drops what was written before it; paused plays silence and
    /// holds the position; an empty ring plays silence.
    #[test]
    fn stale_frames_are_skipped_and_pause_holds() {
        let shared = shared();
        write(&shared, &[[9.0; 2]; 4]);
        shared.flush();
        write(&shared, &[[1.0; 2], [2.0; 2], [3.0; 2]]);
        let mut state = Reader::default();
        shared.set_paused(true);
        assert_eq!(read(&shared, 2, 48_000.0, &mut state), [0.0; 4]);
        shared.set_paused(false);
        assert_eq!(read(&shared, 2, 48_000.0, &mut state), [1.0, 1.0, 2.0, 2.0]);
        let tail = read(&shared, 3, 48_000.0, &mut state);
        assert_eq!(&tail[2..], [0.0; 4], "dry: silence");
        assert_eq!(shared.room(), RING_FRAMES, "everything was taken or skipped");
    }

    /// A track whose last frame is left alone in the ring still plays out:
    /// the reader takes it, so the decoder sees the end.
    #[test]
    fn a_lone_last_frame_is_played_out() {
        let shared = shared();
        write(&shared, &[[0.5; 2]]);
        let mut state = Reader::default();
        let out = read(&shared, 3, 48_000.0, &mut state);
        assert_eq!(&out[..2], [0.5, 0.5]);
        assert_eq!(shared.read.load(Ordering::Relaxed), shared.write.load(Ordering::Relaxed), "nothing left behind");
    }

    /// Going down from 96 to 48 kHz, a tone the device cannot carry (the
    /// source's own Nyquist) is filtered out instead of folding back.
    #[test]
    fn a_higher_source_rate_does_not_alias() {
        let shared = shared();
        shared.rate.store(96_000, Ordering::Relaxed);
        let frames: Vec<[f32; 2]> = (0..8192).map(|i| [if i % 2 == 0 { 1.0 } else { -1.0 }; 2]).collect();
        write(&shared, &frames);
        let mut state = Reader::default();
        let out = read(&shared, 4000, 48_000.0, &mut state);
        let tail = &out[2000..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms < 0.05, "aliased energy {rms}");
        // At the device's own rate nothing is filtered.
        let shared = shared_at(48_000);
        write(&shared, &[[1.0; 2], [-1.0; 2], [1.0; 2]]);
        let mut state = Reader::default();
        assert_eq!(read(&shared, 2, 48_000.0, &mut state), [1.0, 1.0, -1.0, -1.0]);
    }

    fn shared_at(rate: u32) -> Arc<Shared> {
        let shared = Shared::new();
        shared.rate.store(rate, Ordering::Relaxed);
        shared
    }

    /// A mono device hears both sides; more channels get the pair again.
    #[test]
    fn a_mono_device_gets_both_sides() {
        let block = [1.0, 0.0, 0.2, 0.4];
        assert_eq!(device_sample(&block, 0, 0, 1), 0.5);
        assert!((device_sample(&block, 1, 0, 1) - 0.3).abs() < 1e-6);
        assert_eq!(device_sample(&block, 0, 1, 2), 0.0);
        assert_eq!(device_sample(&block, 1, 2, 4), 0.2);
        assert_eq!(device_sample(&block, 1, 3, 4), 0.4);
    }

    /// A play's position counts from its start as soon as the decoder takes
    /// it, before the file is read; asked for another play, the count is
    /// not an answer yet.
    #[test]
    fn the_position_belongs_to_its_play() {
        let shared = shared();
        write(&shared, &[[1.0; 2]; 48]);
        shared.base_ms.store(90_000, Ordering::Relaxed);
        let mut state = Reader::default();
        read(&shared, 48, 48_000.0, &mut state);
        assert_eq!(shared.position_for(0), Some(90_001), "48 frames at 48 kHz: 1 ms on");
        assert_eq!(shared.position_for(7), None, "the new play is not taken yet");
        shared.begin(0, 7);
        read(&shared, 4, 48_000.0, &mut state);
        assert_eq!(shared.position_for(7), Some(0), "from the new start, not the old track's");
    }

    /// Commands waiting collapse to the newest.
    #[test]
    fn waiting_commands_collapse_to_the_newest() {
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        for ms in [1000, 2000, 3000] {
            tx.send(PlayerCommand::Play { path: "x".into(), start_ms: ms, generation: 1, seq: ms as u64 }).unwrap();
        }
        let first = rx.recv().unwrap();
        match newest(first, &rx) {
            Some(PlayerCommand::Play { start_ms, .. }) => assert_eq!(start_ms, 3000),
            _ => panic!("the last seek wins"),
        }
        drop(tx);
        assert!(newest(PlayerCommand::Stop, &rx).is_none(), "the UI hung up");
    }
}
