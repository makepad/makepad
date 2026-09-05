//! The decode thread, the clock and the soundtrack.
//!
//! Lifted from `apps/asset-ui/src/video_player.rs` — the proven pattern: a
//! detached decode thread pulls frames + audio from the platform's
//! hardware video-file seam (`makepad_platform::video_file`), a small ring
//! buffer hands frames to the render thread paced by pts against a wall
//! clock, and the audio track mixes into this app's `cx.audio_output`
//! closure. The clock model is the source's, unchanged: the picture rebases
//! its wall-clock origin on the first frame's pts and the soundtrack
//! free-runs from a queue the decoder keeps ~1 s ahead, so the two stay
//! together without a second timebase to drift against.
//!
//! Two deliberate differences from the source:
//!
//! * Frames stay NV12. asset-ui converts every frame to BGRA in a software
//!   loop on the decode thread; here the ring carries the decoder's own
//!   NV12 and the two planes go straight to GPU textures (the VJ's
//!   `nv12_view.rs` recipe, see `widget.rs`). A 4K frame costs two memcpys
//!   instead of ~8M arithmetic ops.
//! * The soundtrack queue has a volume knob and a user mute, because this
//!   app has volume keys and asset-ui does not.
//!
//! Playback is PLAY-ONCE: at end of stream the picture holds on the last
//! frame and the decode thread PARKS (it does not exit), so a seek back
//! into the clip — or Space to replay — is a ~10 ms in-place decoder seek,
//! never a reopen.

use makepad_widgets::log;
use makepad_widgets::makepad_platform::audio::AudioBuffer;
use makepad_widgets::makepad_platform::thread::{CancellationToken, Lane, TaskHandle, TaskPool};
use makepad_widgets::makepad_platform::video_file::VideoFileDecoder;
use makepad_widgets::Cx;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};

/// How many decoded frames the ring holds before the decode thread idles.
const RING_FRAMES: usize = 3;
/// How far ahead of the speaker the decode thread keeps the soundtrack.
const AUDIO_AHEAD_SECS: f64 = 1.0;
/// Half a frame at 60 fps: a seek target inside a frame's span lands on
/// that frame instead of the next.
const FRAME_EPS_100NS: i64 = 83_000;

/// One decoded picture, still in the decoder's own NV12 layout (tightly
/// packed, stride == width: a `w*h` Y plane followed by a `w/2*h/2`
/// interleaved UV plane).
pub struct Frame {
    pub pts_100ns: i64,
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
}

struct Shared {
    stop: AtomicBool,
    /// The decode thread exited for good — an honored stop or a fatal
    /// decode error. End of stream does NOT end the thread: it parks (see
    /// `eos`) so seeks stay instant.
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

pub struct VideoPlayer {
    pub width: u32,
    pub height: u32,
    /// Container-reported duration; 0 when the container does not say.
    pub duration_100ns: i64,
    pub fps: f64,
    pub has_audio: bool,
    shared: Arc<Shared>,
    frames: VecDeque<Frame>,
    frame_rx: Receiver<Frame>,
    decode_task: Option<TaskHandle<()>>,
    started: Option<f64>,
    last_pts: i64,
    /// While paused: when the pause began. The clock rebases by the paused
    /// span on resume, so playback continues where it stopped instead of
    /// skipping the frames "missed" on the wall clock.
    paused_at: Option<f64>,
    /// A paused seek hands over exactly ONE frame — its target. Without
    /// this latch a host that keeps pumping while paused (this one does,
    /// to fade its transport bar out) walks the ring forward frame by
    /// frame and the "paused" clip races to the end.
    paused_frame_taken: bool,
    epoch: u64,
}

impl VideoPlayer {
    pub fn new(path: &str, pool: TaskPool) -> Result<Self, String> {
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
        // epoch drops any previous tail, lifts the pause mute, and locks
        // every straggler push from an older detached decode thread out.
        let epoch = NEXT_CLIP_EPOCH.fetch_add(1, Ordering::Relaxed);
        {
            let mut audio = video_audio().lock().unwrap();
            audio.clear();
            audio.muted = false;
            audio.owner = epoch;
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            eos: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
        });
        let (frame_tx, frame_rx) = sync_channel(RING_FRAMES);
        let worker_shared = shared.clone();
        let worker_path = path.to_string();
        let wait = CancellationToken::new();
        let worker_wait = wait.clone();
        let decode_task = pool
            .submit(Lane::Heavy, move || {
                match VideoFileDecoder::open(&worker_path) {
                    Ok(decoder) => decode_loop(decoder, &worker_shared, epoch, frame_tx, worker_wait),
                    Err(e) => log!("video: decode task open failed: {}", e),
                }
                worker_shared.done.store(true, Ordering::Release);
            })
            .map_err(|e| format!("could not queue video decode: {e}"))?;
        let fps = if info.fps_den > 0 {
            info.fps_num as f64 / info.fps_den as f64
        } else {
            0.0
        };
        Ok(Self {
            width: info.width,
            height: info.height,
            duration_100ns: info.duration_100ns,
            fps,
            has_audio: info.has_audio,
            shared,
            frames: VecDeque::new(),
            frame_rx,
            decode_task: Some(decode_task),
            started: None,
            last_pts: 0,
            paused_at: None,
            paused_frame_taken: false,
            epoch,
        })
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Freeze the picture; the soundtrack mutes with it. Idempotent.
    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Cx::monotonic_now());
            self.paused_frame_taken = true;
            video_audio().lock().unwrap().muted = true;
        }
    }

    /// Continue from the paused position by pushing the clock base forward
    /// by the paused span. Idempotent.
    pub fn resume(&mut self) {
        if let Some(paused_at) = self.paused_at.take() {
            if let Some(started) = &mut self.started {
                *started += Cx::monotonic_now() - paused_at;
            }
            self.paused_frame_taken = false;
            video_audio().lock().unwrap().muted = false;
        }
    }

    /// The newest frame whose pts has been reached; `None` keeps whatever is
    /// on the texture. Call once per render frame.
    pub fn take_due_frame(&mut self) -> Option<Frame> {
        self.drain_frames();
        if self.paused_at.is_some() {
            // Paused normally: hold the picture. Freshly seeked while
            // paused (clock unset, nothing handed over yet): show the seek
            // target frame ONCE.
            if self.started.is_some() || self.paused_frame_taken {
                return None;
            }
            let frame = self.frames.pop_front()?;
            self.last_pts = frame.pts_100ns;
            self.paused_frame_taken = true;
            // Keep the clock unset: resume rebases at the next frame.
            return Some(frame);
        }
        let first_pts = self.frames.front()?.pts_100ns;
        let started = *self.started.get_or_insert_with(|| {
            Cx::monotonic_now() - first_pts.max(0) as f64 / 10_000_000.0
        });
        let media_100ns = ((Cx::monotonic_now() - started).max(0.0) * 10_000_000.0) as i64;
        let mut due = None;
        while self.frames.front().is_some_and(|f| f.pts_100ns <= media_100ns) {
            due = self.frames.pop_front();
        }
        if let Some(frame) = &due {
            self.last_pts = frame.pts_100ns;
        }
        due
    }

    pub fn position_secs(&self) -> f64 {
        self.last_pts as f64 / 10_000_000.0
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_100ns as f64 / 10_000_000.0
    }

    /// Jump playback to `secs`. The decode thread seeks in place and the
    /// picture clock rebases on the first frame that arrives, so play
    /// continues from there (paused stays paused, showing the seeked frame).
    pub fn seek(&mut self, secs: f64) {
        let duration = self.duration_secs();
        let secs = if duration > 0.0 {
            secs.clamp(0.0, (duration - 0.05).max(0.0))
        } else {
            secs.max(0.0)
        };
        let target = (secs * 10_000_000.0) as i64;
        self.shared.seek_100ns.store(target, Ordering::Release);
        // Drop the end-of-stream flag here rather than waiting for the
        // decode thread to notice the request: otherwise the host sees
        // `at_end()` for the ~10 ms of the seek and calls the clip over
        // again the instant it was told to replay.
        self.shared.eos.store(false, Ordering::Release);
        self.frames.clear();
        while self.frame_rx.try_recv().is_ok() {}
        self.started = None;
        self.paused_frame_taken = false;
        self.last_pts = target;
        let mut audio = video_audio().lock().unwrap();
        if audio.owner == self.epoch {
            audio.clear();
        }
    }

    /// True end-of-playback for the frame pump: the decode thread has parked
    /// at end of stream (or exited) and every buffered frame has been taken.
    pub fn at_end(&mut self) -> bool {
        self.drain_frames();
        (self.shared.eos.load(Ordering::Acquire) || self.shared.done.load(Ordering::Acquire))
            && self.frames.is_empty()
    }

    /// True while a seek request is still unconsumed by the decode thread —
    /// the host coalesces scrub drags on this instead of flooding.
    pub fn seek_pending(&self) -> bool {
        self.shared.seek_100ns.load(Ordering::Acquire) >= 0
    }

    fn drain_frames(&mut self) {
        loop {
            match self.frame_rx.try_recv() {
                Ok(frame) => self.frames.push_back(frame),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if self.decode_task.as_ref().is_some_and(TaskHandle::is_finished) {
            let mut task = self.decode_task.take().unwrap();
            if let Some(Err(error)) = task.try_take() {
                log!("video: decode task failed: {error}");
            }
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.decode_task.take() {
            task.cancel();
        }
        // NEVER join the decode task here: a wedged hardware decoder call
        // would hang the UI thread. The detached task observes `stop`
        // between packets and exits on its own; until then the epoch guard
        // keeps its audio pushes out of the queue and the frame receiver is
        // already disconnected.
        let mut audio = video_audio().lock().unwrap();
        if audio.owner == self.epoch {
            audio.clear();
            audio.owner = 0;
        }
    }
}

fn decode_loop(
    mut decoder: VideoFileDecoder,
    shared: &Shared,
    epoch: u64,
    frame_tx: SyncSender<Frame>,
    wait: CancellationToken,
) {
    let info = decoder.info().clone();
    let mut audio_eos = false;
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        let seek = shared.seek_100ns.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            shared.eos.store(false, Ordering::Release);
            // In-place decoder seek (~10 ms in the platform layer) — never a
            // full reopen, which is what makes SCRUBBING realtime.
            match decoder.seek(seek) {
                Ok(()) => {
                    audio_eos = !info.has_audio;
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
                                if !send_frame(
                                    &frame_tx,
                                    Frame {
                                    pts_100ns: frame.pts_100ns,
                                    width: frame.width,
                                    height: frame.height,
                                    nv12: frame.nv12,
                                    },
                                    shared,
                                    &wait,
                                ) {
                                    break;
                                }
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
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                let _ = send_frame(
                    &frame_tx,
                    Frame {
                        pts_100ns: frame.pts_100ns,
                        width: frame.width,
                        height: frame.height,
                        nv12: frame.nv12,
                    },
                    shared,
                    &wait,
                );
            }
            Ok(None) => {
                // End of stream: drain the soundtrack tail, then PARK. The
                // thread stays alive serving seeks — a replay or a scrub
                // back into the clip is a ~10 ms decoder seek, not a
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
                    let _ = wait.wait_until(Cx::monotonic_now() + 0.010);
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

fn send_frame(
    tx: &SyncSender<Frame>,
    mut frame: Frame,
    shared: &Shared,
    wait: &CancellationToken,
) -> bool {
    loop {
        if shared.stop.load(Ordering::Relaxed)
            || shared.seek_100ns.load(Ordering::Acquire) >= 0
        {
            return false;
        }
        match tx.try_send(frame) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                frame = returned;
                let _ = wait.wait_until(Cx::monotonic_now() + 0.004);
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

// ---------------------------------------------------------------------------
// Soundtrack queue
// ---------------------------------------------------------------------------

/// A process-global resampling stereo queue, mixed additively from the
/// app's single `cx.audio_output` callback.
pub struct VideoAudio {
    frames: VecDeque<(f32, f32)>,
    cursor: f64,
    source_rate: f64,
    /// Transport mute raised by pause / [`stop_audio`]: the decode thread
    /// may still be refilling the queue, so a plain clear would go audible
    /// again ~a second later.
    muted: bool,
    /// The listener's own mute (the M key), independent of the transport.
    user_muted: bool,
    /// The listener's volume, 0.0..=1.0.
    volume: f32,
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
            user_muted: false,
            volume: 1.0,
            owner: 0,
        }
    }

    fn clear(&mut self) {
        self.frames.clear();
        self.cursor = 0.0;
    }

    /// Epoch-guarded queue drop for a seek: only the owning player's decode
    /// thread may flush what it queued.
    fn clear_for(&mut self, epoch: u64) {
        if self.owner == epoch {
            self.clear();
        }
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
        for frame in samples.chunks_exact(ch) {
            let l = frame[0] as f32 / 32768.0;
            let r = frame[ch - 1] as f32 / 32768.0;
            self.frames.push_back((l, r));
        }
    }

    fn mix(&mut self, output: &mut AudioBuffer, device_rate: f64) {
        if self.frames.is_empty() || self.source_rate <= 0.0 || device_rate <= 0.0 {
            return;
        }
        // Volume is applied at the speaker, not at the queue: a change
        // takes effect on the next buffer instead of a second later, and
        // muting never costs the ~1 s of already-decoded sound.
        let gain = if self.user_muted { 0.0 } else { self.volume };
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
            let l = (al + (bl - al) * fraction) * gain;
            let r = (ar + (br - ar) * fraction) * gain;
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

/// Silences the soundtrack immediately and keeps it silent (the decode
/// thread may still be refilling) until the next clip starts. Revoking
/// ownership locks every live decode thread out of the queue.
pub fn stop_audio() {
    if let Ok(mut audio) = video_audio().lock() {
        audio.clear();
        audio.muted = true;
        audio.owner = 0;
    }
}

/// The listener's volume, clamped to 0.0..=1.0.
pub fn set_volume(volume: f32) {
    if let Ok(mut audio) = video_audio().lock() {
        audio.volume = volume.clamp(0.0, 1.0);
    }
}

/// The listener's own mute (the M key), independent of pause.
pub fn set_user_muted(muted: bool) {
    if let Ok(mut audio) = video_audio().lock() {
        audio.user_muted = muted;
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test here touches the process-global soundtrack queue.
    static VIDEO_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_player(shared: Arc<Shared>) -> VideoPlayer {
        let (_frame_tx, frame_rx) = sync_channel(RING_FRAMES);
        VideoPlayer {
            width: 2,
            height: 2,
            duration_100ns: 20_000_000,
            fps: 30.0,
            has_audio: false,
            shared,
            frames: VecDeque::new(),
            frame_rx,
            decode_task: None,
            started: None,
            last_pts: 0,
            paused_at: None,
            paused_frame_taken: false,
            epoch: u64::MAX,
        }
    }

    fn empty_shared() -> Arc<Shared> {
        Arc::new(Shared {
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            eos: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
        })
    }

    fn push_frame(player: &mut VideoPlayer, pts_100ns: i64) {
        player.frames.push_back(Frame {
            pts_100ns,
            width: 2,
            height: 2,
            nv12: vec![0x80; 2 * 2 * 3 / 2],
        });
    }

    #[test]
    fn at_end_needs_decode_exit_and_a_drained_ring() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        let shared = empty_shared();
        let mut player = test_player(shared.clone());
        push_frame(&mut player, 0);
        // Still decoding: never EOS, with or without buffered frames.
        assert!(!player.at_end());
        // Decode parked at EOS, but a due frame is still buffered.
        shared.eos.store(true, Ordering::Release);
        assert!(!player.at_end());
        // The pts-0 frame is immediately due; taking it drains the ring.
        assert!(player.take_due_frame().is_some());
        assert!(player.at_end(), "eos + drained ring is end of playback");
        assert!(player.take_due_frame().is_none());
    }

    #[test]
    fn seek_clamps_into_the_clip_and_requests_the_target() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        let shared = empty_shared();
        let mut player = test_player(shared.clone());
        assert_eq!(player.duration_secs(), 2.0);
        player.seek(-5.0);
        assert_eq!(shared.seek_100ns.load(Ordering::Acquire), 0);
        assert!(player.seek_pending());
        // Past the end clamps just inside the clip, never past duration.
        player.seek(99.0);
        let target = shared.seek_100ns.load(Ordering::Acquire);
        assert!(target > 0 && target < player.duration_100ns, "{target}");
        // The reported position follows the request immediately, so the
        // scrub knob does not snap back while the decoder catches up.
        assert!((player.position_secs() - 1.95).abs() < 0.01);
    }

    #[test]
    fn pause_holds_the_picture_and_resume_rebases_the_clock() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        let shared = empty_shared();
        let mut player = test_player(shared.clone());
        push_frame(&mut player, 0);
        assert!(player.take_due_frame().is_some(), "playing: frame is due");
        assert!(!player.is_paused());
        player.pause();
        assert!(player.is_paused());
        // Paused with a live clock: the picture holds even when a frame is
        // waiting in the ring.
        push_frame(&mut player, 10_000_000);
        assert!(player.take_due_frame().is_none());
        player.paused_at = Some(Cx::monotonic_now() - 0.020);
        player.resume();
        assert!(!player.is_paused());
        // The paused span was added to the clock base, so the frame that was
        // 1 s out is still 1 s out rather than "missed".
        assert!(player.take_due_frame().is_none());
    }

    /// The bug the first live run caught: a host that keeps pumping while
    /// paused (this one does, to fade the transport bar out) used to walk
    /// the ring forward frame by frame, so a paused scrub raced to the end
    /// of the clip on its own.
    #[test]
    fn a_paused_seek_hands_over_exactly_one_frame() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        let shared = empty_shared();
        let mut player = test_player(shared.clone());
        player.pause();
        player.seek(0.5);
        for pts in [5_000_000, 5_400_000, 5_800_000] {
            push_frame(&mut player, pts);
        }
        assert!(player.take_due_frame().is_some(), "the seek target shows");
        assert!((player.position_secs() - 0.5).abs() < 0.001);
        // Every further pump holds the picture, however many are queued.
        for _ in 0..10 {
            assert!(player.take_due_frame().is_none(), "paused holds");
        }
        assert_eq!(player.frames.len(), 2);
        // Resuming releases the ring again.
        player.resume();
        assert!(player.take_due_frame().is_some());
    }

    /// Replay after end of stream: the host must not be told "ended" again
    /// during the ~10 ms the decode thread takes to notice the seek.
    #[test]
    fn seeking_clears_end_of_stream_immediately() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        let shared = empty_shared();
        let mut player = test_player(shared.clone());
        shared.eos.store(true, Ordering::Release);
        assert!(player.at_end());
        player.seek(0.0);
        assert!(!player.at_end(), "a replay request is not the end");
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

    #[test]
    fn volume_and_mute_scale_the_speaker_not_the_queue() {
        let _serial = VIDEO_TEST_LOCK.lock().unwrap();
        stop_audio();
        {
            let mut audio = video_audio().lock().unwrap();
            audio.muted = false;
            audio.owner = 11;
        }
        // A flat ramp of full-scale frames at the device rate (step 1.0).
        let samples: Vec<i16> = (0..8).flat_map(|_| [16384, 16384]).collect();
        video_audio()
            .lock()
            .unwrap()
            .push_i16(11, &samples, 2, 10);
        let queued = video_audio().lock().unwrap().frames.len();
        assert_eq!(queued, 8);

        set_volume(0.5);
        set_user_muted(false);
        let mut half = AudioBuffer::new_with_size(2, 2);
        half.zero();
        mix_into(&mut half, 10.0);
        let loud = half.channel(0)[0];
        assert!((loud - 0.25).abs() < 0.01, "half volume of 0.5 fs: {loud}");
        // The queue itself was untouched by the gain: only 2 frames left.
        assert_eq!(video_audio().lock().unwrap().frames.len(), queued - 2);

        set_user_muted(true);
        let mut silent = AudioBuffer::new_with_size(2, 2);
        silent.zero();
        mix_into(&mut silent, 10.0);
        assert_eq!(silent.channel(0)[0], 0.0, "M mutes the speaker");
        // ... and muting still consumes the queue, so unmuting resumes in
        // sync with the picture instead of replaying the muted span.
        assert_eq!(video_audio().lock().unwrap().frames.len(), queued - 4);
        set_user_muted(false);
        set_volume(1.0);
        stop_audio();
    }
}
