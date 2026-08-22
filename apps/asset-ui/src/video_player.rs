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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
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
    /// The decode thread exited for good — a honored stop or a fatal
    /// decode error. End-of-stream no longer ends the thread: it PARKS
    /// (see `eos`) so seeks — loop restarts, scrubs — stay instant.
    done: AtomicBool,
    /// The stream is fully decoded and the thread is parked waiting for a
    /// seek or a stop. With an empty ring this is the player's EOS state.
    eos: AtomicBool,
    /// Requested playback position in 100ns units; -1 = none. The decode
    /// thread consumes it (an in-place decoder seek — no reopen).
    seek_100ns: AtomicI64,
}

/// Soundtrack-queue ownership ticket. Every player claims a fresh epoch; a
/// DETACHED decode thread of a dropped player still holds its old epoch and
/// its pushes bounce off the queue (see [`VideoPlayer::drop`]).
static NEXT_CLIP_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Half a frame at 60 fps: a seek target inside a frame's span lands on
/// that frame instead of the next.
const FRAME_EPS_100NS: i64 = 83_000;

pub struct VideoPlayer {
    pub width: u32,
    pub height: u32,
    /// Container-reported duration; 0 when the container does not say.
    pub duration_100ns: i64,
    shared: Arc<Shared>,
    started: Option<Instant>,
    last_pts: i64,
    /// While paused: when the pause began. The clock rebases by the paused
    /// span on resume, so playback continues where it stopped instead of
    /// skipping the frames "missed" on the wall clock.
    paused_at: Option<Instant>,
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
            eos: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
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
            duration_100ns: info.duration_100ns,
            shared,
            started: None,
            last_pts: 0,
            paused_at: None,
            epoch,
        })
    }

    /// The newest frame whose pts has been reached; `None` keeps whatever is
    /// on the texture. Call once per render frame.
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Freeze the picture; the soundtrack mutes with it. Idempotent.
    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
            video_audio().lock().unwrap().muted = true;
        }
    }

    /// Continue from the paused position by pushing the clock base forward
    /// by the paused span. Idempotent.
    pub fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            if let Some(started) = &mut self.started {
                *started += paused_at.elapsed();
            }
            video_audio().lock().unwrap().muted = false;
        }
    }

    pub fn take_due_frame(&mut self) -> Option<Vec<u32>> {
        if self.paused_at.is_some() {
            // Paused normally: hold the picture. Freshly seeked while
            // paused (clock unset): show the seek target frame once.
            if self.started.is_some() {
                return None;
            }
            let mut frames = self.shared.frames.lock().unwrap();
            let frame = frames.pop_front()?;
            self.last_pts = frame.pts_100ns;
            // Keep the clock unset: resume rebases at the next frame.
            return Some(frame.bgra);
        }
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
        if let Some(frame) = &due {
            self.last_pts = frame.pts_100ns;
        }
        due.map(|f| f.bgra)
    }

    pub fn position_secs(&self) -> f64 {
        self.last_pts as f64 / 10_000_000.0
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_100ns as f64 / 10_000_000.0
    }

    /// Jump playback to `secs`. The decode thread reopens the file and
    /// discards up to the target; the picture clock rebases on the first
    /// frame that arrives, so play continues from there (paused stays
    /// paused, showing the seeked frame).
    pub fn seek(&mut self, secs: f64) {
        let target = (secs.max(0.0) * 10_000_000.0) as i64;
        self.shared.seek_100ns.store(target, Ordering::Release);
        self.shared.frames.lock().unwrap().clear();
        self.started = None;
        self.last_pts = target;
        let mut audio = video_audio().lock().unwrap();
        if audio.owner == self.epoch {
            audio.clear();
        }
    }

    /// True end-of-playback for the frame pump: the decode thread has exited
    /// (end of stream, decode error, or stop) and every buffered frame has
    /// been taken. The soundtrack tail may still be draining in the audio
    /// callback — that needs no frame pump.
    pub fn at_end(&self) -> bool {
        (self.shared.eos.load(Ordering::Acquire) || self.shared.done.load(Ordering::Acquire))
            && self.shared.frames.lock().unwrap().is_empty()
    }

    /// True while a seek request is still unconsumed by the decode thread —
    /// the host coalesces scrub drags on this instead of flooding.
    pub fn seek_pending(&self) -> bool {
        self.shared.seek_100ns.load(Ordering::Acquire) >= 0
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
        let seek = shared.seek_100ns.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            shared.eos.store(false, Ordering::Release);
            // In-place decoder seek (SetCurrentPosition / reader rebuild in
            // the platform layer, ~10 ms) — never a full reopen, which is
            // what makes SCRUBBING realtime and a loop restart seamless.
            match decoder.seek(seek) {
                Ok(()) => {
                    audio_eos = !info.has_audio;
                    shared.frames.lock().unwrap().clear();
                    loop {
                        if shared.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if shared.seek_100ns.load(Ordering::Acquire) >= 0 {
                            break; // newer scrub target supersedes this one
                        }
                        match decoder.next_frame() {
                            Ok(Some(frame)) if frame.pts_100ns + FRAME_EPS_100NS < seek => {}
                            Ok(Some(frame)) => {
                                nv12::nv12_to_rgb8(
                                    &frame.nv12,
                                    frame.width,
                                    frame.height,
                                    &mut rgb_scratch,
                                );
                                let mut bgra =
                                    Vec::with_capacity((frame.width * frame.height) as usize);
                                for px in rgb_scratch.chunks_exact(3) {
                                    bgra.push(
                                        0xff00_0000
                                            | ((px[0] as u32) << 16)
                                            | ((px[1] as u32) << 8)
                                            | px[2] as u32,
                                    );
                                }
                                shared
                                    .frames
                                    .lock()
                                    .unwrap()
                                    .push_back(Frame { pts_100ns: frame.pts_100ns, bgra });
                                break;
                            }
                            Ok(None) => break,
                            Err(e) => {
                                log!("video: seek decode error: {}", e);
                                break;
                            }
                        }
                    }
                    // Audio follows the picture: drop queued samples and
                    // skip the soundtrack forward to the target.
                    if info.has_audio {
                        video_audio().lock().unwrap().clear_for(epoch);
                        loop {
                            if shared.stop.load(Ordering::Relaxed) {
                                return;
                            }
                            match decoder.next_audio() {
                                Ok(Some(chunk)) if chunk.pts_100ns < seek => {}
                                Ok(Some(chunk)) => {
                                    video_audio().lock().unwrap().push_i16(
                                        epoch,
                                        &chunk.samples,
                                        chunk.channels,
                                        chunk.sample_rate,
                                    );
                                    break;
                                }
                                Ok(None) => {
                                    audio_eos = true;
                                    break;
                                }
                                Err(_) => {
                                    audio_eos = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => log!("video: decoder seek failed: {}", e),
            }
            continue;
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
                // End of stream: drain the soundtrack tail, then PARK. The
                // thread stays alive serving seeks — a loop restart or a
                // scrub back into the clip is a ~10 ms decoder seek, not a
                // teardown and reopen.
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
                    audio_eos = true;
                }
                shared.eos.store(true, Ordering::Release);
                while shared.eos.load(Ordering::Acquire) {
                    if shared.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if shared.seek_100ns.load(Ordering::Acquire) >= 0 {
                        break; // the top of the loop consumes it
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                continue;
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

impl VideoAudio {
    /// Epoch-guarded queue drop for a seek: only the owning player's decode
    /// thread may flush what it queued.
    fn clear_for(&mut self, epoch: u64) {
        if self.owner == epoch {
            self.frames.clear();
            self.cursor = 0.0;
        }
    }
}

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
            eos: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
        });
        shared.frames.lock().unwrap().push_back(Frame {
            pts_100ns: 0,
            bgra: vec![0xff00_0000; 4],
        });
        let mut player = VideoPlayer {
            width: 2,
            height: 2,
            duration_100ns: 0,
            shared: shared.clone(),
            started: None,
            last_pts: 0,
            paused_at: None,
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
