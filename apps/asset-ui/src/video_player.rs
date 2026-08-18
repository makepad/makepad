//! Video artifact playback — the sandbox's proven decode pattern
//! (apps/sandbox/src/video_player.rs): a decode thread pulls frames + audio
//! from the platform video-file seam (`makepad_platform::video_file`,
//! hardware codecs), a small ring buffer hands BGRA frames to the render
//! thread paced by pts against a wall clock, and the audio track mixes into
//! this app's `cx.audio_output` closure.
//!
//! Playback is PLAY-ONCE: at end-of-stream the remaining audio drains and
//! the clip stops (the sandbox pattern's loop-forever reopen was what users
//! heard as "the soundtrack never ends"). Loading a new artifact drops the
//! previous player (its `Drop` silences the queue); `stop_audio()` silences
//! immediately and stays muted until the next clip starts.
//!
//! Copied rather than imported: the sandbox is an app crate under active
//! concurrent development, not a library — and this pattern is ~250 lines.

use makepad_widgets::log;
use makepad_widgets::makepad_platform::audio::AudioBuffer;
use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RING_FRAMES: usize = 3;
const AUDIO_AHEAD_SECS: f64 = 1.0;

struct Frame {
    pts_100ns: i64,
    bgra: Vec<u32>,
}

struct Shared {
    frames: Mutex<VecDeque<Frame>>,
    stop: AtomicBool,
    /// The decode thread exited — end of stream, decode error, or a
    /// honored stop request. Together with an empty ring this is the
    /// player's real EOS state: the pump must stop re-arming on it.
    done: AtomicBool,
}

/// Soundtrack-queue ownership ticket. Every player claims a fresh epoch; a
/// DETACHED decode thread of a dropped player still holds its old epoch and
/// its pushes bounce off the queue (see [`VideoPlayer::drop`]).
static NEXT_CLIP_EPOCH: AtomicU64 = AtomicU64::new(1);

pub struct VideoPlayer {
    pub width: u32,
    pub height: u32,
    shared: Arc<Shared>,
    started: Option<Instant>,
    epoch: u64,
}

impl VideoPlayer {
    pub fn new(path: &str) -> Result<Self, String> {
        let info = VideoFileDecoder::open(path)
            .map_err(|e| e.to_string())?
            .info()
            .clone();
        if info.width == 0 || info.height == 0 {
            return Err(format!(
                "video reports zero size: {}x{}",
                info.width, info.height
            ));
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
        // Fresh clip: take ownership of the soundtrack queue. Claiming a new
        // epoch drops any previous tail, lifts the sticky stop_audio() mute,
        // and locks every straggler push from an older detached decode
        // thread out of the queue.
        let epoch = NEXT_CLIP_EPOCH.fetch_add(1, Ordering::Relaxed);
        {
            let mut audio = video_audio().lock().unwrap();
            audio.clear();
            audio.muted = false;
            audio.owner = epoch;
        }
        let shared = Arc::new(Shared {
            frames: Mutex::new(VecDeque::new()),
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let thread_path = path.to_string();
        // The JoinHandle is deliberately dropped: teardown must never join a
        // possibly wedged hardware decoder on the UI thread. The thread is
        // detached; `stop` + the audio epoch make that safe.
        std::thread::Builder::new()
            .name("asset-ui-video-decode".into())
            .spawn(move || {
                match VideoFileDecoder::open(&thread_path) {
                    Ok(decoder) => decode_loop(thread_path, decoder, &thread_shared, epoch),
                    Err(e) => log!("video: decode thread open failed: {}", e),
                }
                thread_shared.done.store(true, Ordering::Release);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            width: info.width,
            height: info.height,
            shared,
            started: None,
            epoch,
        })
    }

    /// The newest frame whose pts has been reached; `None` keeps whatever is
    /// on the texture. Call once per render frame.
    pub fn take_due_frame(&mut self) -> Option<Vec<u32>> {
        let mut frames = self.shared.frames.lock().unwrap();
        let first_pts = frames.front()?.pts_100ns;
        let started = *self.started.get_or_insert_with(|| {
            Instant::now() - Duration::from_nanos(first_pts.max(0) as u64 * 100)
        });
        let media_100ns = (started.elapsed().as_nanos() / 100) as i64;
        let mut due = None;
        while frames.front().is_some_and(|f| f.pts_100ns <= media_100ns) {
            due = frames.pop_front();
        }
        due.map(|f| f.bgra)
    }

    /// True end-of-playback for the frame pump: the decode thread has exited
    /// (end of stream, decode error, or stop) and every buffered frame has
    /// been taken. The soundtrack tail may still be draining in the audio
    /// callback — that needs no frame pump.
    pub fn at_end(&self) -> bool {
        self.shared.done.load(Ordering::Acquire) && self.shared.frames.lock().unwrap().is_empty()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        // NEVER join the decode thread here: a wedged hardware decoder call
        // would hang the UI thread. The detached thread observes `stop`
        // between packets and exits on its own; until then the epoch guard
        // keeps its audio pushes out of the queue, and its frame ring dies
        // with the last Arc.
        let mut audio = video_audio().lock().unwrap();
        if audio.owner == self.epoch {
            audio.clear();
            audio.owner = 0;
        }
    }
}

fn decode_loop(path: String, mut decoder: VideoFileDecoder, shared: &Shared, epoch: u64) {
    let info = decoder.info().clone();
    let mut audio_eos = false;
    let mut rgb_scratch = Vec::new();
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        if info.has_audio && !audio_eos {
            while video_audio().lock().unwrap().buffered_secs() < AUDIO_AHEAD_SECS {
                match decoder.next_audio() {
                    Ok(Some(chunk)) => video_audio().lock().unwrap().push_i16(
                        epoch,
                        &chunk.samples,
                        chunk.channels,
                        chunk.sample_rate,
                    ),
                    Ok(None) => {
                        audio_eos = true;
                        break;
                    }
                    Err(e) => {
                        log!("video: audio decode error: {}", e);
                        audio_eos = true;
                        break;
                    }
                }
            }
        }
        if shared.frames.lock().unwrap().len() >= RING_FRAMES {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb_scratch);
                let mut bgra = Vec::with_capacity((frame.width * frame.height) as usize);
                for px in rgb_scratch.chunks_exact(3) {
                    bgra.push(
                        0xff00_0000
                            | ((px[0] as u32) << 16)
                            | ((px[1] as u32) << 8)
                            | px[2] as u32,
                    );
                }
                shared.frames.lock().unwrap().push_back(Frame {
                    pts_100ns: frame.pts_100ns,
                    bgra,
                });
            }
            Ok(None) => {
                // End of stream: play ONCE and stop. The audio pump above
                // only reads ~1s ahead, so push the rest of the soundtrack
                // now; the queue then drains to silence and the thread ends.
                if info.has_audio && !audio_eos {
                    loop {
                        if shared.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        match decoder.next_audio() {
                            Ok(Some(chunk)) => video_audio().lock().unwrap().push_i16(
                                epoch,
                                &chunk.samples,
                                chunk.channels,
                                chunk.sample_rate,
                            ),
                            Ok(None) => break,
                            Err(e) => {
                                log!("video: audio tail decode error: {}", e);
                                break;
                            }
                        }
                    }
                }
                log!("video: end of stream: {}", path);
                return;
            }
            Err(e) => {
                log!("video: decode error: {}", e);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Soundtrack queue (identical mixer shape to the wav player in audio.rs)
// ---------------------------------------------------------------------------

pub struct VideoAudio {
    frames: VecDeque<(f32, f32)>,
    cursor: f64,
    source_rate: f64,
    /// Sticky mute raised by [`stop_audio`]: the decode thread may still be
    /// refilling the queue, so a plain clear would go audible again ~a
    /// second later. Cleared when the next clip starts.
    muted: bool,
    /// Epoch of the ONE player allowed to push (0 = none). Detached decode
    /// threads of dropped players carry stale epochs and are locked out.
    owner: u64,
}

impl VideoAudio {
    const fn new() -> Self {
        Self {
            frames: VecDeque::new(),
            cursor: 0.0,
            source_rate: 0.0,
            muted: false,
            owner: 0,
        }
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.cursor = 0.0;
    }

    fn buffered_secs(&self) -> f64 {
        if self.source_rate <= 0.0 {
            return 0.0;
        }
        (self.frames.len() as f64 - self.cursor).max(0.0) / self.source_rate
    }

    fn push_i16(&mut self, epoch: u64, samples: &[i16], channels: u16, rate: u32) {
        if self.muted || self.owner != epoch {
            return;
        }
        self.source_rate = rate as f64;
        let ch = channels.max(1) as usize;
        const GAIN: f32 = 0.9;
        for frame in samples.chunks_exact(ch) {
            let l = frame[0] as f32 / 32768.0 * GAIN;
            let r = frame[ch - 1] as f32 / 32768.0 * GAIN;
            self.frames.push_back((l, r));
        }
    }

    fn mix(&mut self, output: &mut AudioBuffer, device_rate: f64) {
        if self.frames.is_empty() || self.source_rate <= 0.0 || device_rate <= 0.0 {
            return;
        }
        let step = self.source_rate / device_rate;
        let channels = output.channel_count();
        for frame in 0..output.frame_count() {
            let index = self.cursor as usize;
            if index + 1 >= self.frames.len() {
                break;
            }
            let fraction = (self.cursor - index as f64) as f32;
            let (al, ar) = self.frames[index];
            let (bl, br) = self.frames[index + 1];
            let l = al + (bl - al) * fraction;
            let r = ar + (br - ar) * fraction;
            for channel in 0..channels {
                let s = if channel == 0 { l } else { r };
                output.channel_mut(channel)[frame] += s;
            }
            self.cursor += step;
        }
        let consumed = self.cursor as usize;
        if consumed > 0 {
            self.frames.drain(..consumed.min(self.frames.len()));
            self.cursor -= consumed as f64;
        }
        // A lone trailing frame can never be interpolated: once the queue is
        // down to it the clip is over (or hard-underrun) — release it so the
        // soundtrack ends instead of pinning the final sample forever.
        if self.frames.len() <= 1 {
            self.clear();
        }
    }
}

static VIDEO_AUDIO: Mutex<VideoAudio> = Mutex::new(VideoAudio::new());

fn video_audio() -> &'static Mutex<VideoAudio> {
    &VIDEO_AUDIO
}

/// Mix queued video audio into the device buffer (one line in the app's
/// `cx.audio_output` closure).
pub fn mix_into(output: &mut AudioBuffer, device_rate: f64) {
    if let Ok(mut audio) = video_audio().lock() {
        audio.mix(output, device_rate);
    }
}

/// Silences the video soundtrack immediately and keeps it silent (the
/// decode thread may still be refilling) until the next clip starts —
/// stop-button / tab-switch hook. Revoking ownership locks every live
/// decode thread out of the queue, not just muting it.
pub fn stop_audio() {
    if let Ok(mut audio) = video_audio().lock() {
        audio.clear();
        audio.muted = true;
        audio.owner = 0;
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tests touch the process-global soundtrack queue.
    static VIDEO_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn at_end_needs_decode_exit_and_a_drained_ring() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        let shared = Arc::new(Shared {
            frames: Mutex::new(VecDeque::new()),
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
        });
        shared.frames.lock().unwrap().push_back(Frame {
            pts_100ns: 0,
            bgra: vec![0xff00_0000; 4],
        });
        let mut player = VideoPlayer {
            width: 2,
            height: 2,
            shared: shared.clone(),
            started: None,
            epoch: u64::MAX,
        };
        // Still decoding: never EOS, with or without buffered frames.
        assert!(!player.at_end());
        // Decode exited, but a due frame is still buffered: pump keeps going.
        shared.done.store(true, Ordering::Release);
        assert!(!player.at_end());
        // The pts-0 frame is immediately due; taking it drains the ring.
        assert!(player.take_due_frame().is_some());
        assert!(player.at_end(), "decode done + drained ring is EOS");
        // A stopped-but-undrained ring is also not EOS until taken.
        assert!(player.take_due_frame().is_none());
    }

    #[test]
    fn soundtrack_queue_ignores_stale_epochs_and_revoked_ownership() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        {
            let mut audio = video_audio().lock().unwrap();
            audio.clear();
            audio.muted = false;
            audio.owner = 7;
            audio.source_rate = 0.0;
        }
        // A dropped player's detached thread (stale epoch) cannot push.
        video_audio()
            .lock()
            .unwrap()
            .push_i16(6, &[1000, 1000], 2, 48_000);
        assert_eq!(video_audio().lock().unwrap().frames.len(), 0);
        // The owning epoch pushes fine.
        video_audio()
            .lock()
            .unwrap()
            .push_i16(7, &[1000, 1000, 2000, 2000], 2, 48_000);
        assert_eq!(video_audio().lock().unwrap().frames.len(), 2);
        // stop_audio revokes ownership: even the former owner is locked out.
        stop_audio();
        video_audio()
            .lock()
            .unwrap()
            .push_i16(7, &[1000, 1000], 2, 48_000);
        let audio = video_audio().lock().unwrap();
        assert_eq!(audio.frames.len(), 0);
        assert_eq!(audio.owner, 0);
        assert!(audio.muted);
    }
}
