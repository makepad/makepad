//! Media decode: video slot players, audio track/pad decoding, waveforms.
//!
//! The video path adapts the proven sandbox/ai-content pattern
//! (`apps/asset-ui/src/video_player.rs`): one decode thread per slot pulls
//! frames + audio from the platform video-file seam (hardware codecs on
//! macOS), keeps a small BGRA ring paced by pts against a pause-aware wall
//! clock, and pushes PCM into this slot's mixer bus (never a process
//! global — VJ crossfades two slots with independent gains). Extensions over
//! the copied pattern: pre-roll signalling, pause, loop-by-reopen,
//! seek-by-reopen (the platform decoder has no native seek), and position
//! reporting. Teardown is a stop flag + detached thread — never a join on
//! the UI thread.
//!
//! Audio tracks (music decks) and SFX pads decode fully to memory on a small
//! worker pool: WAV through a bounded RIFF parser, MP4/M4A audio through the
//! platform decoder. Everything is budgeted.

use crate::cue::SlotId;
use crate::decks::DeckId;
use crate::mixer::{Mixer, TrackPcm, MAX_VIDEO_PLAYBACK_RATE, MIN_VIDEO_PLAYBACK_RATE};
use crate::pads::PadKey;
use makepad_asset_data::{AssetRevisionId, MediaType, ThumbnailCells};
use makepad_audio_decode::{decode_audio_limited, AudioFormat, Limits as AudioLimits};
use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder, VideoFileInfo};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const RING_FRAMES: usize = 3;

/// A backward pts jump bigger than this is a stream restart (loop wrap /
/// seek) to the pacer. Frame-scale — half a 120fps frame — because
/// decoded presentation pts are monotonic within a pass, and a margin
/// any wider blinds the pacer to short loops (the 200fps-flicker bug).
const WRAP_MARGIN_100NS: i64 = 40_000;
const AUDIO_AHEAD_SECS: f64 = 1.0;
/// Enough decoded audio to survive UI/display-link jitter after a beat start.
const PREROLL_AUDIO_LEAD_SECS: f64 = 0.25;
/// Some platform decoders advertise audio before producing it. Do not leave a
/// cue armed forever: expose that degraded readiness honestly after this bound.
const PREROLL_AUDIO_TIMEOUT: Duration = Duration::from_millis(1_500);
static DECODER_ALIAS_ID: AtomicU64 = AtomicU64::new(0);
/// Longest fully decoded track (music deck), frames: 60 min at 48 kHz.
/// YouTube DJ mixes land here; two hours would be ~1.3 GiB PCM per deck.
pub const MAX_TRACK_FRAMES: usize = 48_000 * 60 * 60;
/// Longest fully decoded SFX sample, frames: 30 s at 48 kHz.
pub const MAX_PAD_FRAMES: usize = 48_000 * 30;
/// Waveform resolution (min/max columns) computed at decode time.
pub const WAVE_COLS: usize = 2048;

// ---------------------------------------------------------------------------
// video slot player
// ---------------------------------------------------------------------------

/// Platform media frameworks use a path's extension as a container type
/// hint. Asset-cache objects intentionally have digest-only names, so an
/// otherwise valid MP4 is reported as having no video track by AVURLAsset.
///
/// Give the decoder an extension-bearing hard link beside (but outside) the
/// cache's content-addressed `objects/` tree. A hard link does not duplicate
/// a potentially large clip. The decode thread owns this lease and removes
/// the link only after every reopen/loop has stopped, which keeps detached
/// slot teardown safe.
struct DecoderInput {
    path: String,
    alias: Option<PathBuf>,
}

impl DecoderInput {
    fn prepare(source: &Path, media: MediaType) -> Result<Self, String> {
        let extension = match media {
            MediaType::Mp4 => "mp4",
            other => return Err(format!("unsupported video media {other:?}")),
        };
        if !source.is_file() {
            return Err(format!("media file not found: {}", source.display()));
        }
        if source
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            return Ok(Self {
                path: source
                    .to_str()
                    .ok_or_else(|| format!("non-utf8 media path: {}", source.display()))?
                    .to_string(),
                alias: None,
            });
        }

        // Cache paths are `<root>/objects/<prefix>/<digest>`. Keep aliases
        // under `<root>/decoder-input/`; for a non-cache source, use a
        // sibling private directory. Either choice is on the source volume,
        // so hard-linking never needs a byte-copy fallback.
        let parent = source.parent().ok_or("media path has no parent")?;
        let objects = parent.parent();
        let alias_dir = match objects {
            Some(objects) if objects.file_name().is_some_and(|name| name == "objects") => objects
                .parent()
                .unwrap_or(parent)
                .join("decoder-input"),
            _ => parent.join(".makepad-decoder-input"),
        };
        std::fs::create_dir_all(&alias_dir)
            .map_err(|e| format!("create decoder input directory: {e}"))?;
        let ticket = DECODER_ALIAS_ID.fetch_add(1, Ordering::Relaxed);
        let alias = alias_dir.join(format!(
            "decoder-{}-{ticket}.{extension}",
            std::process::id()
        ));
        std::fs::hard_link(source, &alias)
            .map_err(|e| format!("create typed decoder link: {e}"))?;
        let path = match alias.to_str() {
            Some(path) => path.to_string(),
            None => {
                let _ = std::fs::remove_file(&alias);
                return Err(format!("non-utf8 decoder path: {}", alias.display()));
            }
        };
        Ok(Self { path, alias: Some(alias) })
    }
}

impl Drop for DecoderInput {
    fn drop(&mut self) {
        if let Some(alias) = self.alias.take() {
            let _ = std::fs::remove_file(alias);
        }
    }
}

struct Frame {
    /// Pacing timestamp — monotonic for the clock (synthetic in bounce).
    pts_100ns: i64,
    /// TRUE clip position of this picture, for the position readout: in
    /// bounce the pacing stamps climb forever while the picture runs
    /// backward — the scrub bar follows this, never the pacing stamp.
    clip_100ns: i64,
    bgra: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PrerollStatus {
    WaitingVideo = 0,
    WaitingAudio = 1,
    Ready = 2,
    /// The source advertised audio, but its complete (short) audio track is
    /// smaller than the normal lead target.
    ReadyAudioExhausted = 3,
    /// The source advertised audio but did not produce a bounded lead in
    /// time. Playback may start, while the UI can surface degraded A/V sync.
    ReadyAudioTimeout = 4,
}

impl PrerollStatus {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::WaitingAudio,
            2 => Self::Ready,
            3 => Self::ReadyAudioExhausted,
            4 => Self::ReadyAudioTimeout,
            _ => Self::WaitingVideo,
        }
    }

    pub fn is_ready(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::ReadyAudioExhausted | Self::ReadyAudioTimeout
        )
    }
}

fn preroll_status(
    video_ready: bool,
    has_audio: bool,
    audio_buffered_secs: f64,
    audio_eos: bool,
    timed_out: bool,
) -> PrerollStatus {
    if !video_ready {
        return PrerollStatus::WaitingVideo;
    }
    if !has_audio || audio_buffered_secs >= PREROLL_AUDIO_LEAD_SECS {
        return PrerollStatus::Ready;
    }
    if audio_eos {
        return PrerollStatus::ReadyAudioExhausted;
    }
    if timed_out {
        return PrerollStatus::ReadyAudioTimeout;
    }
    PrerollStatus::WaitingAudio
}

/// How a video slot behaves at end of clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    Once = 0,
    Loop = 1,
    /// Forward-backward alternation from a decoded-frame cache. Falls back
    /// to Loop while the cache is not yet complete (first pass) or when the
    /// clip exceeds the cache budget. Ping-pong plays no audio — reversed
    /// sound is not a thing a pad wants.
    PingPong = 2,
}

impl PlayMode {
    fn from_u8(v: u8) -> PlayMode {
        match v {
            1 => PlayMode::Loop,
            2 => PlayMode::PingPong,
            _ => PlayMode::Once,
        }
    }
}

/// Decoded-frame cache ceiling for ping-pong (BGRA bytes). Sized so the
/// enhance service's 1280×704 outputs bounce from memory: 281 frames at
/// 3.6 MB is ~1.01 GB — but an enhanced clip normally carries a flow map
/// and bounces through the WARP path from its 508 MB endpoint cache
/// instead, so this ceiling serves plain clips: a 5.9 s 640×352 loop is
/// 127 MB, a 1280×704 clip fits up to ~7 s (177 frames at 24 fps). Bigger
/// clips fall through to the seek-bounce tier below.
const MAX_PINGPONG_CACHE_BYTES: usize = 640 * 1024 * 1024;

/// Seek-bounce (tier 3): how far one reverse hop reaches back. Two seconds
/// is a typical GOP, so most of what the in-seek discard walk decodes is
/// the window itself.
const REVERSE_WINDOW_100NS: i64 = 20_000_000;

/// Byte cap on one collected reverse window. When 2 s of frames exceed it
/// (large formats), the window keeps its NEWEST frames and the next hop
/// re-decodes the trimmed head — reverse stays correct, just costs more
/// decode. 96 MB holds a full 2 s window up to ~1 MB/frame (e.g. 640×352
/// and 720p), and ~26 frames of 1280×704.
const REVERSE_WINDOW_MAX_BYTES: usize = 96 * 1024 * 1024;

struct SlotShared {
    stop: AtomicBool,
    paused: AtomicBool,
    mode: AtomicU8,
    muted: AtomicBool,
    /// The operator is holding the scrub bar: seeks land silently (no
    /// per-tick audio blips); the release seek re-primes audio if unmuted.
    scrub: AtomicBool,
    /// Pending seek target in 100ns units; negative = none.
    seek_100ns: AtomicI64,
    /// IN/OUT trim bounds (the scrub bar's range handles), 100ns units.
    /// `i64::MAX` out = clip end. EVERY tier confines playback to the
    /// range: streaming wraps to IN at OUT, the cache tiers loop/bounce
    /// inside it, ONCE holds at OUT.
    trim_in_100ns: AtomicI64,
    trim_out_100ns: AtomicI64,
    /// BEAT TRANSPORT (the videoloop sync law: "one beat, one play
    /// direction"): while on, the cache repeat runs at CONSTANT rate and
    /// each `beat_pulse` tick — the app's beat boundary — wraps a Loop to
    /// IN / flips a PingPong's direction. A range edge reached before the
    /// beat HOLDS there; the next pulse launches the next sweep.
    beat_transport: AtomicBool,
    beat_pulse: AtomicU64,
    /// BEATS PER SWEEP — the rate chip's value, literally (8/4/2/1: 8 =
    /// one sweep stretched across eight beats, 1 = a sweep per beat).
    beats_per_sweep: AtomicU8,
    /// The beat period in presentation pts, HINTED by the app at cue and
    /// on tempo changes so the law governs from the very first frame —
    /// the pulse-learned period refines it but nothing waits for it.
    beat_hint_100ns: AtomicI64,
    /// SCRATCH override: while the operator shuttles by hand the beat
    /// sweep disengages; sign and magnitude are natural-rate units.
    scratch_active: AtomicBool,
    scratch_rate_bits: AtomicU64,
    /// The last PACING pts pushed to the ring, whatever stamped it — the
    /// law-paced first pass and the sweep tiers hand the presentation
    /// clock across seamlessly by continuing from here.
    pace_tail_100ns: AtomicI64,
    /// Bumped by every trim change: a decoded-frame cache built under the
    /// old bounds only covers the old range, so the tiers hand control
    /// back and the next pass rebuilds.
    trim_epoch: AtomicU64,
    /// Presentation position (pts of the last frame handed to the UI).
    position_100ns: AtomicI64,
    video_ready: AtomicBool,
    preroll_status: AtomicU8,
    playback_rate_bits: AtomicU64,
    end_of_stream: AtomicBool,
    frames: Mutex<VecDeque<Frame>>,
    failure: Mutex<Option<String>>,
}

/// One playback slot: decode thread + frame ring + pts pacing.
pub struct SlotPlayer {
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
    shared: Arc<SlotShared>,
    /// Pause-aware presentation clock: media time at `clock_base` was
    /// `base_media_100ns`.
    clock_base: Option<Instant>,
    base_media_100ns: i64,
    last_pts: i64,
    slot: SlotId,
    mixer: Mixer,
}

impl SlotPlayer {
    /// Probe + spawn the decode thread. Fails fast on unopenable files; the
    /// pre-roll becomes ready only after the first frame and a bounded audio
    /// lead (when the source has audio). The mixer bus stays paused, so this
    /// preparation never consumes a sample before the device-clock start.
    pub fn open(
        slot: SlotId,
        path: &str,
        media: MediaType,
        mixer: Mixer,
        loop_on: bool,
        start_paused: bool,
    ) -> Result<SlotPlayer, String> {
        let input = DecoderInput::prepare(Path::new(path), media)?;
        let info = VideoFileDecoder::open(&input.path)
            .map_err(|e| e.to_string())?
            .info()
            .clone();
        if info.width == 0 || info.height == 0 {
            return Err(format!("video reports zero size: {}x{}", info.width, info.height));
        }
        let shared = Arc::new(SlotShared {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(start_paused),
            mode: AtomicU8::new(if loop_on { PlayMode::Loop } else { PlayMode::Once } as u8),
            muted: AtomicBool::new(false),
            scrub: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
            trim_in_100ns: AtomicI64::new(0),
            trim_out_100ns: AtomicI64::new(i64::MAX),
            beat_transport: AtomicBool::new(false),
            beat_pulse: AtomicU64::new(0),
            beats_per_sweep: AtomicU8::new(4),
            beat_hint_100ns: AtomicI64::new(0),
            scratch_active: AtomicBool::new(false),
            scratch_rate_bits: AtomicU64::new(0f64.to_bits()),
            pace_tail_100ns: AtomicI64::new(0),
            trim_epoch: AtomicU64::new(0),
            position_100ns: AtomicI64::new(0),
            video_ready: AtomicBool::new(false),
            preroll_status: AtomicU8::new(PrerollStatus::WaitingVideo as u8),
            playback_rate_bits: AtomicU64::new(1.0f64.to_bits()),
            end_of_stream: AtomicBool::new(false),
            frames: Mutex::new(VecDeque::new()),
            failure: Mutex::new(None),
        });
        mixer.set_slot_playback_rate(slot, 1.0);
        mixer.set_slot_paused(slot, start_paused);
        let thread_shared = shared.clone();
        let thread_mixer = mixer.clone();
        std::thread::Builder::new()
            .name(format!("vj-slot-{:?}", slot))
            .spawn(move || decode_loop(slot, input, thread_mixer, thread_shared))
            .map_err(|e| e.to_string())?;
        Ok(SlotPlayer {
            width: info.width,
            height: info.height,
            duration_secs: info.duration_100ns.max(0) as f64 / 10_000_000.0,
            shared,
            clock_base: None,
            base_media_100ns: 0,
            last_pts: 0,
            slot,
            mixer,
        })
    }

    pub fn preroll_ready(&self) -> bool {
        self.preroll_status().is_ready()
    }

    pub fn preroll_status(&self) -> PrerollStatus {
        PrerollStatus::from_u8(self.shared.preroll_status.load(Ordering::Acquire))
    }

    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().unwrap().clone()
    }

    pub fn set_paused(&mut self, paused: bool) {
        let was = self.shared.paused.load(Ordering::Acquire);
        if was == paused {
            return;
        }
        if paused {
            // Freeze: remember where the clock stood.
            self.base_media_100ns = self.media_now_100ns();
            self.clock_base = None;
        }
        self.shared.paused.store(paused, Ordering::Release);
        self.mixer.set_slot_paused(self.slot, paused);
        // Unpause re-bases lazily on the next take_due_frame.
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    /// Whether frame-paced pumping is still useful. A settled paused slot
    /// and an exhausted slot with an empty ring need no display-link loop.
    pub fn needs_frame_pump(&self) -> bool {
        !self.shared.paused.load(Ordering::Acquire)
            && (!self.shared.end_of_stream.load(Ordering::Acquire)
                || !self.shared.frames.lock().unwrap().is_empty())
    }

    pub fn set_loop(&mut self, loop_on: bool) {
        self.set_mode(if loop_on { PlayMode::Loop } else { PlayMode::Once });
    }

    pub fn set_mode(&mut self, mode: PlayMode) {
        self.shared.mode.store(mode as u8, Ordering::Release);
    }

    /// Drop this slot's audio at the source: no samples reach the mixer
    /// while muted (the already-buffered lead drains first).
    /// Scrub-in-progress: audio stays silent across the drag's seeks.
    pub fn set_scrub(&mut self, scrub: bool) {
        self.shared.scrub.store(scrub, Ordering::Release);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Release);
    }

    /// Set coherent picture/audio pacing for this video slot. No reverse or
    /// wide time-stretch is pretended: the rate is clamped to the musically
    /// safe loop-fit range shared with the mixer.
    pub fn set_playback_rate(&mut self, rate: f64) -> f64 {
        let now = self.media_now_100ns();
        let rate = rate.clamp(MIN_VIDEO_PLAYBACK_RATE, MAX_VIDEO_PLAYBACK_RATE);
        self.base_media_100ns = now;
        if !self.is_paused() && self.clock_base.is_some() {
            self.clock_base = Some(Instant::now());
        }
        self.shared.playback_rate_bits.store(rate.to_bits(), Ordering::Release);
        self.mixer.set_slot_playback_rate(self.slot, rate)
    }

    pub fn playback_rate(&self) -> f64 {
        f64::from_bits(self.shared.playback_rate_bits.load(Ordering::Acquire))
            .clamp(MIN_VIDEO_PLAYBACK_RATE, MAX_VIDEO_PLAYBACK_RATE)
    }

    /// Position of the frame currently on screen, seconds.
    pub fn position_secs(&self) -> f64 {
        self.shared.position_100ns.load(Ordering::Acquire).max(0) as f64 / 10_000_000.0
    }

    /// Constrain playback to the [start, end] fraction range — the scrub
    /// bar's IN/OUT trim handles. Loop wraps jump to IN, bounce reflects
    /// at both handles, ONCE holds at OUT. `end >= 1` clears the out
    /// bound. (A wrap seek lands on the prior keyframe, so an UNMUTED
    /// trimmed loop may whisper a few pre-IN audio frames per wrap — VJ
    /// loops are muted by default, and the picture is exact.)
    pub fn set_trim(&mut self, start: f64, end: f64) {
        let d = (self.duration_secs * 10_000_000.0).max(0.0) as i64;
        // Normalize: order the pair and keep at least 100ms of clip
        // between the handles — a collapsed window (a hasty drag, or a
        // stale sticky profile restored onto a different edit) otherwise
        // parks the sweep on a near-single frame and reads as a hang.
        let (mut start, mut end) = if start <= end { (start, end) } else { (end, start) };
        let min_span = (0.1 / self.duration_secs.max(0.001)).min(1.0);
        if end - start < min_span {
            end = (start + min_span).min(1.0);
            start = (end - min_span).max(0.0);
        }
        let a = (start.clamp(0.0, 1.0) * d as f64) as i64;
        let b = (end.clamp(0.0, 1.0) * d as f64) as i64;
        let (lo, hi) = (a.min(b), a.max(b));
        self.shared.trim_in_100ns.store(lo, Ordering::Release);
        self.shared
            .trim_out_100ns
            .store(if end >= 1.0 { i64::MAX } else { hi }, Ordering::Release);
        self.shared.trim_epoch.fetch_add(1, Ordering::AcqRel);
        // A once-mode clip parked past OUT stays put until the operator
        // seeks; a LOOPING clip past OUT wraps on the next decode step.
        self.shared.end_of_stream.store(false, Ordering::Release);
    }

    /// Beat-driven transport on/off (see `SlotShared::beat_transport`).
    pub fn set_beat_transport(&mut self, on: bool) {
        self.shared.beat_transport.store(on, Ordering::Release);
    }

    /// The rate chip, literally: BEATS PER SWEEP (8/4/2/1). Values are
    /// clamped into that ladder; anything stale (an old 0.5x profile)
    /// falls to the default 4.
    pub fn set_beats_per_sweep(&mut self, beats: u8) {
        let beats = if [8u8, 4, 2, 1].contains(&beats) { beats } else { 4 };
        self.shared.beats_per_sweep.store(beats, Ordering::Release);
    }

    /// Beat-period hint in presentation pts (wall period × playback
    /// rate): lets the sweep run law-paced from the FIRST frame of a cue
    /// instead of free-wheeling one natural pass while it learns the
    /// grid from pulses.
    pub fn set_beat_hint(&mut self, period_100ns: i64) {
        self.shared.beat_hint_100ns.store(period_100ns.max(0), Ordering::Release);
    }

    /// Manual SCRATCH shuttle: engage with a signed natural-rate factor
    /// (+1 = natural forward, -2 = double-speed reverse). While active
    /// the beat sweep disengages and the picture follows the hand within
    /// the trim window, clamping at its edges. `clear_scratch` releases
    /// — the sweep re-engages FROM THE CURRENT POSITION per the law (no
    /// jump), re-locking to the grid over the next beats.
    pub fn set_scratch(&mut self, rate: f64) {
        self.shared.scratch_rate_bits.store(rate.to_bits(), Ordering::Release);
        self.shared.scratch_active.store(true, Ordering::Release);
    }

    pub fn clear_scratch(&mut self) {
        self.shared.scratch_active.store(false, Ordering::Release);
    }

    /// One beat boundary: the transport turns/wraps NOW.
    pub fn beat_pulse(&mut self) {
        self.shared.beat_pulse.fetch_add(1, Ordering::AcqRel);
    }

    /// The current trim bounds as fractions (0..=1).
    pub fn trim_fractions(&self) -> (f64, f64) {
        let d = (self.duration_secs * 10_000_000.0).max(1.0);
        let lo = self.shared.trim_in_100ns.load(Ordering::Acquire).max(0) as f64 / d;
        let hi = self.shared.trim_out_100ns.load(Ordering::Acquire);
        let hi = if hi == i64::MAX { 1.0 } else { (hi as f64 / d).min(1.0) };
        (lo.min(1.0), hi)
    }

    /// Seek by reopening the stream and discarding up to the target. The
    /// decode thread does the work; playback resumes from the target.
    pub fn seek_fraction(&mut self, fraction: f64) {
        let target =
            (fraction.clamp(0.0, 1.0) * self.duration_secs * 10_000_000.0) as i64;
        self.shared.seek_100ns.store(target.max(0), Ordering::Release);
        self.shared.end_of_stream.store(false, Ordering::Release);
        // The position IS the target from this instant: a PAUSED scrub
        // presents no frame to refresh the atomic, and leaving it stale
        // let the playhead snap back to the pre-seek spot until the next
        // presented frame.
        self.shared.position_100ns.store(target.max(0), Ordering::Release);
        // Re-base the presentation clock at the target.
        self.base_media_100ns = target;
        self.clock_base = None;
        self.last_pts = target;
    }

    /// Wall-clock media time, honoring pause.
    fn media_now_100ns(&self) -> i64 {
        match (self.shared.paused.load(Ordering::Acquire), self.clock_base) {
            (true, _) | (false, None) => self.base_media_100ns,
            (false, Some(base)) => {
                self.base_media_100ns
                    + ((base.elapsed().as_nanos() as f64 / 100.0) * self.playback_rate()) as i64
            }
        }
    }

    /// The newest due frame (call once per UI frame); `None` keeps the
    /// current texture. Rebases the clock on stream restarts (loop/seek).
    pub fn take_due_frame(&mut self) -> Option<Vec<u32>> {
        if self.shared.paused.load(Ordering::Acquire) {
            return None;
        }
        let mut frames = self.shared.frames.lock().unwrap();
        let first_pts = frames.front()?.pts_100ns;
        // A large backward pts jump means the stream restarted (loop or
        // seek): rebase the clock there.
        // Any frame-scale backward jump is a restart. The margin must be
        // FRAME-scale, not loop-scale: it once sat at 500ms, so a loop
        // (or a live trim) shorter than half a second never re-based the
        // clock — every wrapped pass was instantly "due" and the ring
        // drained at poll speed, a 200fps flicker instead of a loop.
        if first_pts + WRAP_MARGIN_100NS < self.last_pts {
            self.base_media_100ns = first_pts;
            self.clock_base = Some(Instant::now());
        }
        // ...and a FORWARD jump is a restart too: a transport handoff
        // that lands on a farther pts must present NOW, not stall until
        // the clock walks the gap (the stall scales with 1/rate — at a
        // slow chip it read as playback stopping dead).
        if first_pts > self.last_pts + 5_000_000 {
            self.base_media_100ns = first_pts;
            self.clock_base = Some(Instant::now());
        }
        if self.clock_base.is_none() {
            self.base_media_100ns = self.base_media_100ns.max(first_pts.min(self.base_media_100ns + 10_000_000));
            self.clock_base = Some(Instant::now());
        }
        let media = self.media_now_100ns();
        let mut due: Option<Frame> = None;
        while let Some(front) = frames.front() {
            if front.pts_100ns > media {
                break;
            }
            // STOP at a wrap boundary: a looping cache queues the restart
            // behind the tail, and draining past it in one call plays the
            // whole buffered next pass in fast-forward (seen live as "the
            // clip replays fast, twice" — once per buffered pass until the
            // wrap happened to sit at the front when the rebase looks).
            // Leaving the wrapped frame queued lets the NEXT call's rebase
            // land on it and restart the clock at normal speed.
            if let Some(d) = &due {
                if front.pts_100ns + WRAP_MARGIN_100NS < d.pts_100ns {
                    break;
                }
            }
            due = frames.pop_front();
        }
        if let Some(frame) = &due {
            self.last_pts = frame.pts_100ns;
            self.shared.position_100ns.store(frame.clip_100ns, Ordering::Release);
        }
        due.map(|f| f.bgra)
    }
}

impl Drop for SlotPlayer {
    fn drop(&mut self) {
        // Detached teardown: flag it and walk away; the thread exits on its
        // own (never a UI-thread join).
        self.shared.stop.store(true, Ordering::Release);
    }
}

fn decode_loop(slot: SlotId, input: DecoderInput, mixer: Mixer, shared: Arc<SlotShared>) {
    let path = &input.path;
    let mut decoder = match VideoFileDecoder::open(path) {
        Ok(d) => d,
        Err(e) => {
            *shared.failure.lock().unwrap() = Some(e.to_string());
            return;
        }
    };
    let info = decoder.info().clone();
    let mut audio_eos = !info.has_audio;
    let mut rgb_scratch = Vec::new();
    let preroll_deadline = Instant::now() + PREROLL_AUDIO_TIMEOUT;
    // Ping-pong frame cache: filled during a full forward pass that runs
    // with the mode set, complete only when the WHOLE clip fit the budget.
    // Any seek or reopen restarts it (a partial cache must never bounce).
    let mut pingpong_cache: Vec<Frame> = Vec::new();
    let mut pingpong_cache_bytes: usize = 0;
    let mut pingpong_cache_complete = false;
    let mut pingpong_over_budget = false;
    // Frames decoded this pass: a cache that started mid-pass (the mode
    // flipped on partway through) covers only the TAIL and must never be
    // declared complete — it bounces again from the next full pass.
    let mut pass_frames: u64 = 0;
    let mut pingpong_cache_partial = false;
    // Latched when this decoder's seek fails: never retry a broken seam.
    let mut seek_bounce_broken = false;
    let mut trim_epoch_seen = shared.trim_epoch.load(Ordering::Acquire);
    // Law-paced first pass state: the last REAL video pts seen and the
    // last sane real inter-frame delta (used across wrap seams, where
    // the real pts jump backward).
    let mut last_real_pts: Option<i64> = None;
    let mut last_real_delta: i64 = 416_667;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        // A trim change invalidates the frame cache (it only covers the
        // old range); the rest of THIS pass can never complete one either.
        let trim_epoch = shared.trim_epoch.load(Ordering::Acquire);
        if trim_epoch != trim_epoch_seen {
            trim_epoch_seen = trim_epoch;
            pingpong_cache.clear();
            pingpong_cache_bytes = 0;
            pingpong_cache_complete = false;
            pingpong_over_budget = false;
            pingpong_cache_partial = pass_frames > 0;
        }
        // Seek: reopen and discard up to the target.
        let seek = shared.seek_100ns.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            pingpong_cache.clear();
            pingpong_cache_bytes = 0;
            pingpong_cache_complete = false;
            // A post-SEEK pass covers target→OUT, not the window: it may
            // never declare a complete cache (a scrub used to mint a
            // tail-only cache the sweep then mapped the WHOLE trim onto —
            // the "loops only the last third" lie). The wrap after this
            // pass rebuilds from live IN and THAT pass completes.
            pingpong_cache_partial = true;
            pass_frames = 0;
            match VideoFileDecoder::open(&path) {
                Ok(d) => {
                    decoder = d;
                    audio_eos = !info.has_audio;
                    shared.frames.lock().unwrap().clear();
                    mixer.flush_slot_audio(slot);
                    // Discard video frames strictly before the target.
                    loop {
                        if shared.stop.load(Ordering::Acquire) {
                            return;
                        }
                        match decoder.next_frame() {
                            Ok(Some(frame)) if frame.pts_100ns + 400_000 < seek => continue,
                            Ok(Some(frame)) => {
                                push_frame(&shared, frame, &mut rgb_scratch);
                                break;
                            }
                            Ok(None) => break,
                            Err(e) => {
                                *shared.failure.lock().unwrap() = Some(e.to_string());
                                return;
                            }
                        }
                    }
                    // Discard audio strictly before the target.
                    while !audio_eos {
                        if shared.stop.load(Ordering::Acquire) {
                            return;
                        }
                        match decoder.next_audio() {
                            Ok(Some(chunk)) if chunk.pts_100ns < seek => continue,
                            Ok(Some(chunk)) => {
                                // MUTED stays silent, and a scrub NEVER
                                // sounds — only the release seek of an
                                // unmuted clip re-primes the bus here.
                                if !shared.muted.load(Ordering::Acquire)
                                    && !shared.scrub.load(Ordering::Acquire)
                                {
                                    mixer.push_slot_audio(
                                        slot,
                                        &chunk.samples,
                                        chunk.channels,
                                        chunk.sample_rate,
                                    );
                                }
                                break;
                            }
                            Ok(None) => {
                                audio_eos = true;
                            }
                            Err(_) => {
                                audio_eos = true;
                            }
                        }
                    }
                }
                Err(e) => {
                    *shared.failure.lock().unwrap() = Some(e.to_string());
                    return;
                }
            }
        }
        if shared.paused.load(Ordering::Acquire) {
            // True preroll: hold one video frame plus a bounded audio lead.
            // The bus is paused, so decoding fills the queue without moving
            // its source cursor.
            if shared.frames.lock().unwrap().is_empty() {
                match decoder.next_frame() {
                    Ok(Some(frame)) => {
                        push_frame(&shared, frame, &mut rgb_scratch);
                    }
                    Ok(None) => {
                        shared.end_of_stream.store(true, Ordering::Release);
                        *shared.failure.lock().unwrap() =
                            Some("video ended before producing a preroll frame".into());
                        return;
                    }
                    Err(e) => {
                        *shared.failure.lock().unwrap() = Some(e.to_string());
                        return;
                    }
                }
            }
            let mut buffered = mixer.slot_buffered_secs(slot);
            if info.has_audio
                && !audio_eos
                && buffered < PREROLL_AUDIO_LEAD_SECS
                && Instant::now() < preroll_deadline
            {
                match decoder.next_audio() {
                    Ok(Some(chunk)) => {
                        if !mixer.push_slot_audio(
                            slot,
                            &chunk.samples,
                            chunk.channels,
                            chunk.sample_rate,
                        ) {
                            return;
                        }
                        buffered = mixer.slot_buffered_secs(slot);
                    }
                    Ok(None) | Err(_) => audio_eos = true,
                }
            }
            let status = preroll_status(
                shared.video_ready.load(Ordering::Acquire),
                info.has_audio,
                buffered,
                audio_eos,
                Instant::now() >= preroll_deadline,
            );
            shared.preroll_status.store(status as u8, Ordering::Release);
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }
        // Keep this slot's mixer bus fed ~1s ahead. While muted, the
        // chunks are decoded and DROPPED — the decoder must still advance
        // in step with the picture, the room just hears nothing.
        if !audio_eos {
            while mixer.slot_buffered_secs(slot) < AUDIO_AHEAD_SECS {
                if shared.muted.load(Ordering::Acquire) {
                    match decoder.next_audio() {
                        Ok(Some(_)) => continue,
                        Ok(None) | Err(_) => {
                            audio_eos = true;
                        }
                    }
                    break;
                }
                match decoder.next_audio() {
                    Ok(Some(chunk)) => {
                        // The OUT handle gates audio too: nothing past it
                        // is fed ahead — a trimmed loop's sound stops at
                        // OUT and the wrap re-feeds from IN.
                        if chunk.pts_100ns
                            >= shared.trim_out_100ns.load(Ordering::Acquire)
                        {
                            break;
                        }
                        mixer.push_slot_audio(
                            slot,
                            &chunk.samples,
                            chunk.channels,
                            chunk.sample_rate,
                        );
                    }
                    Ok(None) | Err(_) => {
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
        let trim_in = shared.trim_in_100ns.load(Ordering::Acquire).max(0);
        let trim_out = shared.trim_out_100ns.load(Ordering::Acquire);
        match decoder.next_frame() {
            // Pre-IN frames (a wrap seek lands on the prior keyframe, and
            // the reopen fallback starts at zero) are decoded and dropped.
            Ok(Some(frame)) if frame.pts_100ns + 400_000 < trim_in => continue,
            // A frame past OUT falls into the next arm: the OUT handle IS
            // end-of-stream for every mode.
            Ok(Some(frame)) if frame.pts_100ns < trim_out => {
                // THE LAW FROM THE FIRST FRAME: when the sweep transport
                // owns this deck and the window is cache-able, the very
                // first (cache-building) pass is already PACED like the
                // sweep — real clip pts scaled so the pass spans the
                // chip's beats — instead of one natural-rate loop that
                // jarringly snaps to law speed when the cache completes.
                // The compression is bounded (decode must keep up), and
                // an over-budget window streams natural as before.
                let real_pts = frame.pts_100ns;
                let real_delta = match last_real_pts {
                    Some(prev) if real_pts > prev && real_pts - prev < 10_000_000 => {
                        real_pts - prev
                    }
                    _ => last_real_delta,
                };
                last_real_delta = real_delta.max(1);
                let pace = {
                    let transport =
                        shared.beat_transport.load(Ordering::Acquire)
                            && !shared.scratch_active.load(Ordering::Acquire);
                    let hint = shared.beat_hint_100ns.load(Ordering::Acquire);
                    let window = (trim_out.min(info.duration_100ns.max(1)) - trim_in)
                        .max(1) as f64;
                    let frame_bytes = (frame.width as usize)
                        .saturating_mul(frame.height as usize)
                        .saturating_mul(4);
                    let cacheable = (window / last_real_delta as f64)
                        * frame_bytes as f64
                        <= MAX_PINGPONG_CACHE_BYTES as f64;
                    if transport && hint > 0 && cacheable {
                        let beats = shared
                            .beats_per_sweep
                            .load(Ordering::Acquire)
                            .max(1) as f64;
                        let scale =
                            ((beats * hint as f64) / window).clamp(0.33, 32.0);
                        let tail =
                            shared.pace_tail_100ns.load(Ordering::Acquire);
                        let base = match last_real_pts {
                            Some(_) if tail > 0 => tail,
                            _ => real_pts,
                        };
                        Some(base + ((real_delta as f64 * scale) as i64).max(1))
                    } else {
                        None
                    }
                };
                last_real_pts = Some(real_pts);
                let cached =
                    push_frame_paced(&shared, frame, &mut rgb_scratch, pace);
                pass_frames += 1;
                if PlayMode::from_u8(shared.mode.load(Ordering::Acquire)) != PlayMode::Once
                    && !pingpong_over_budget
                    && !pingpong_cache_complete
                {
                    if pingpong_cache.is_empty() && pass_frames > 1 {
                        pingpong_cache_partial = true;
                    }
                    pingpong_cache_bytes += cached.bgra.len() * 4;
                    if pingpong_cache_bytes > MAX_PINGPONG_CACHE_BYTES {
                        pingpong_cache.clear();
                        pingpong_cache_bytes = 0;
                        pingpong_over_budget = true;
                        eprintln!("vj-slot {slot:?}: clip exceeds the ping-pong cache budget; bouncing falls back to loop");
                    } else {
                        pingpong_cache.push(cached);
                    }
                }
                if shared.preroll_status.load(Ordering::Acquire)
                    != PrerollStatus::Ready as u8
                {
                    shared
                        .preroll_status
                        .store(PrerollStatus::Ready as u8, Ordering::Release);
                }
            }
            Ok(Some(_)) | Ok(None) => {
                if !pingpong_over_budget
                    && !pingpong_cache.is_empty()
                    && !pingpong_cache_partial
                {
                    pingpong_cache_complete = true;
                }
                // A tail-only cache is thrown away; the reopen below decodes
                // the next pass from frame 0 with the mode already set, so
                // THAT cache completes.
                if pingpong_cache_partial {
                    pingpong_cache.clear();
                    pingpong_cache_bytes = 0;
                    pingpong_cache_partial = false;
                }
                pass_frames = 0;
                let mode = PlayMode::from_u8(shared.mode.load(Ordering::Acquire));
                let silent = repeat_is_silent(
                    info.has_audio,
                    shared.muted.load(Ordering::Acquire),
                    mode,
                );
               if mode != PlayMode::Once && pingpong_cache_complete && silent {
                    // The whole clip is in memory and nothing needs the
                    // audio decoder: repeat it straight from the cache —
                    // end to start with no decoder reopen, which is what
                    // used to hiccup every wrap. Loop plays forward and
                    // wraps; ping-pong bounces. Returns to the decoder
                    // path when a seek lands, the mode changes, or the
                    // trim range grows past what this cache holds — and
                    // then FALLS THROUGH to the streaming wrap below.
                    // (It used to `continue`: an instant cache exit then
                    // re-entered this branch forever — EOS → exit → EOS —
                    // a silent busy-spin that froze the picture with the
                    // loop lit.)
                    cache_playback(&shared, &pingpong_cache);
                }
                if mode == PlayMode::PingPong
                    && pingpong_over_budget
                    && silent
                    && !seek_bounce_broken
                    && info.duration_100ns > 0
                {
                    // TIER 3: too big for the frame cache, but a bounce was
                    // asked for — GOP-batch reverse via decoder seeks. Falls
                    // through to the reopen below when it hands back control
                    // (mode change / seek / a decoder that cannot seek).
                    match seek_bounce_playback(&mut decoder, &shared, &info) {
                        Ok(true) => {}
                        Ok(false) => {
                            seek_bounce_broken = true;
                            eprintln!(
                                "vj-slot {slot:?}: decoder cannot seek; over-budget bounce falls back to loop"
                            );
                        }
                        Err(e) => {
                            *shared.failure.lock().unwrap() = Some(e);
                            return;
                        }
                    }
                    if shared.stop.load(Ordering::Acquire) {
                        return;
                    }
                }
                let mode = PlayMode::from_u8(shared.mode.load(Ordering::Acquire));
                if mode != PlayMode::Once {
                    // A LOOP wrap is an in-place seek to the IN point first
                    // — ~10 ms against the reopen's full teardown, which is
                    // what made every wrap of a streaming loop hiccup.
                    // Decoders that cannot seek fall to the reopen below
                    // unchanged (the pre-IN discard arm walks them up).
                    // LIVE bounds, read NOW: the local was captured before
                    // a possibly minutes-long cache repeat, and wrapping
                    // to a stale IN rebuilt the old window forever (grow
                    // the trim, nothing changes — the disconnect).
                    let trim_in =
                        shared.trim_in_100ns.load(Ordering::Acquire).max(0);
                    if decoder.seek(trim_in).is_ok() {
                        audio_eos = !info.has_audio;
                        continue;
                    }
                    match VideoFileDecoder::open(&path) {
                        Ok(d) => {
                            decoder = d;
                            audio_eos = !info.has_audio;
                            continue;
                        }
                        Err(e) => {
                            *shared.failure.lock().unwrap() = Some(e.to_string());
                            return;
                        }
                    }
                }
                // Play once: drain the audio tail (bounded by the mixer's
                // queue cap) and hold the last frame.
                while !audio_eos {
                    if shared.stop.load(Ordering::Acquire) {
                        return;
                    }
                    if mixer.slot_buffered_secs(slot) > AUDIO_AHEAD_SECS {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    match decoder.next_audio() {
                        Ok(Some(chunk)) if chunk.pts_100ns >= trim_out => {
                            audio_eos = true;
                        }
                        Ok(Some(chunk)) => {
                            if !shared.muted.load(Ordering::Acquire) {
                                mixer.push_slot_audio(
                                    slot,
                                    &chunk.samples,
                                    chunk.channels,
                                    chunk.sample_rate,
                                );
                            }
                        }
                        Ok(None) | Err(_) => audio_eos = true,
                    }
                }
                shared.end_of_stream.store(true, Ordering::Release);
                // Wait for loop/seek/stop instead of exiting: a later loop
                // toggle or seek revives the slot.
                loop {
                    if shared.stop.load(Ordering::Acquire) {
                        return;
                    }
                    if PlayMode::from_u8(shared.mode.load(Ordering::Acquire)) != PlayMode::Once {
                        shared.end_of_stream.store(false, Ordering::Release);
                        break;
                    }
                    if shared.seek_100ns.load(Ordering::Acquire) >= 0 {
                        shared.end_of_stream.store(false, Ordering::Release);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                match VideoFileDecoder::open(&path) {
                    Ok(d) => {
                        decoder = d;
                        audio_eos = !info.has_audio;
                    }
                    Err(e) => {
                        *shared.failure.lock().unwrap() = Some(e.to_string());
                        return;
                    }
                }
            }
            Err(e) => {
                *shared.failure.lock().unwrap() = Some(e.to_string());
                return;
            }
        }
    }
}

/// Decoded NV12 → the ring's BGRA frame (no queueing).
fn convert_frame(
    frame: makepad_widgets::makepad_platform::video_file::DecodedVideoFrame,
    rgb_scratch: &mut Vec<u8>,
) -> Frame {
    nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, rgb_scratch);
    let mut bgra = Vec::with_capacity((frame.width * frame.height) as usize);
    for px in rgb_scratch.chunks_exact(3) {
        bgra.push(
            0xff00_0000 | ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32,
        );
    }
    Frame { pts_100ns: frame.pts_100ns, clip_100ns: frame.pts_100ns, bgra }
}

fn push_frame(
    shared: &Arc<SlotShared>,
    frame: makepad_widgets::makepad_platform::video_file::DecodedVideoFrame,
    rgb_scratch: &mut Vec<u8>,
) -> Frame {
    push_frame_paced(shared, frame, rgb_scratch, None)
}

/// Like [`push_frame`], but the ring copy may carry a synthetic PACING
/// pts (the law-paced first pass) while the returned frame — what the
/// repeat cache stores — always keeps the REAL clip pts.
fn push_frame_paced(
    shared: &Arc<SlotShared>,
    frame: makepad_widgets::makepad_platform::video_file::DecodedVideoFrame,
    rgb_scratch: &mut Vec<u8>,
    pace_pts: Option<i64>,
) -> Frame {
    let converted = convert_frame(frame, rgb_scratch);
    let out = Frame {
        pts_100ns: converted.pts_100ns,
        clip_100ns: converted.clip_100ns,
        bgra: converted.bgra.clone(),
    };
    let ring = Frame {
        pts_100ns: pace_pts.unwrap_or(converted.pts_100ns),
        clip_100ns: converted.clip_100ns,
        bgra: converted.bgra,
    };
    shared.pace_tail_100ns.store(ring.pts_100ns, Ordering::Release);
    shared.frames.lock().unwrap().push_back(ring);
    shared.video_ready.store(true, Ordering::Release);
    out
}

/// Repeat a fully cached clip without ever touching the decoder again:
/// Loop wraps end to start, PingPong bounces (endpoints not repeated), and
/// switching between the two mid-play is seamless because the direction is
/// read from the live mode every frame. pts are synthesized monotonically
/// so the pacing clock never sees time run backwards. Exits when the mode
/// goes to Once, a seek lands, or the slot stops — the decoder path takes
/// over again.
/// Whether repeat playback may leave the audio decoder behind and serve
/// from the frame cache / seek-bounce tiers. A PING-PONG is inherently a
/// VISUAL (there is no reversed audio): it counts as silent even with the
/// clip unmuted — this is what made the bounce button "do nothing" on any
/// clip with a soundtrack.
fn repeat_is_silent(has_audio: bool, muted: bool, mode: PlayMode) -> bool {
    !has_audio || muted || mode == PlayMode::PingPong
}

/// Whether the LIVE trim range asks for anything outside the range the
/// cache was decoded under. Pure bounds-vs-bounds — frame pts never enter
/// it, so container start offsets cannot fake an uncovered range.
fn cache_range_outgrown(built: (i64, i64), live: (i64, i64)) -> bool {
    live.0 < built.0 || live.1 > built.1
}

/// THE VIDEOLOOP SYNC LAW, in its operator-ratified final form: ONE
/// DIRECTION SWEEP = ONE BEAT STEP. The playback rate is DERIVED FROM
/// THE RANGE — a sweep of the user's trim (or the whole clip) spans
/// exactly one beat divided by the rate chip, REGARDLESS of how wide the
/// range is. A small range plays slow motion, a wide range rushes, and a
/// turn coincides with a beat boundary BY CONSTRUCTION — there is never
/// a pause (the edge-hold refinement froze the picture between edge
/// arrival and the next pulse, and was rejected: "it literally pauses"),
/// and never an off-beat turn. The chip is cadence: 2x sweeps in half a
/// beat, 0.5x stretches one sweep across two.
///
/// The transport therefore runs on PHASE, not on frame indices: `phase`
/// walks 0→1 once per sweep, and [`sweep_index`] maps it into the live
/// `[lo, hi)` window each tick. That mapping is what makes a LIVE TRIM
/// rescale instead of teleport: the phase is untouched, so the position
/// remaps proportionally into the new range and the sweep keeps landing
/// its turns on the grid. (The original sin — "dialing speeds makes up a
/// range" — was a WRONG RANGE, never the rate derivation: the range is
/// the user's trim, exactly, and this mapping cannot invent another.)
///
/// A bounce alternates direction each beat step (the mirrored map); a
/// wrap restarts each step from IN.
fn sweep_index(phase: f64, forward: bool, lo: usize, hi: usize, mode: PlayMode) -> usize {
    let span = hi.max(lo + 1) - lo;
    let u = if mode == PlayMode::PingPong && !forward {
        1.0 - phase
    } else {
        phase
    };
    lo + ((u.clamp(0.0, 1.0)) * (span as f64 - 1.0)).round() as usize
}

/// One tick of the sweep clock: advance the phase by `step` (the tick's
/// share of one beat step), wrapping at 1.0 with the OVERSHOOT CARRIED —
/// the wrap costs zero time, so the long-run cadence is exact and the
/// motion never hitches at the turn. The wrap flips a bounce and
/// restarts a wrap-mode loop (the `forward` flag; a loop is always
/// forward).
fn advance_sweep(
    phase: f64,
    forward: bool,
    step: f64,
    mode: PlayMode,
) -> (f64, bool) {
    let next = phase + step.max(0.0);
    if next >= 1.0 {
        let carried = (next - 1.0).min(1.0 - f64::EPSILON);
        (carried, if mode == PlayMode::PingPong { !forward } else { true })
    } else {
        (next, forward)
    }
}

/// The beat lock's only corrective authority: a bounded phase NUDGE.
/// At each observed pulse the sweep phase should sit on a grid multiple
/// of `m` = 1/beats-per-sweep — a 4-beat sweep passes a beat at every
/// quarter of its phase, and each must land on the pulse.
/// The returned signed nudge walks the phase toward the nearest
/// multiple, clamped to ±2% of a sweep per pulse: drift from rounding or
/// a rate flip converges over a few beats, and the correction is far too
/// small to ever read as a skip. NEVER a snap — a snap is a teleport.
fn beat_phase_nudge(phase_at_beat: f64, beats_per_sweep: f64) -> f64 {
    let m = (1.0 / beats_per_sweep.max(1.0)).clamp(0.05, 1.0);
    let err = (phase_at_beat / m).round() * m - phase_at_beat;
    err.clamp(-0.02, 0.02)
}

fn cache_playback(shared: &Arc<SlotShared>, cache: &[Frame]) {
    if cache.len() < 2 {
        return;
    }
    // Constant-fps clips: the median delta IS the frame duration.
    let mut deltas: Vec<i64> = cache.windows(2).map(|w| w[1].pts_100ns - w[0].pts_100ns).collect();
    deltas.sort_unstable();
    let delta = deltas[deltas.len() / 2].max(1);
    // Continue the PACING clock from wherever the ring left it — the
    // law-paced first pass runs a compressed/stretched pts domain, and
    // starting from the cache's REAL tail would jump the pacer forward
    // (a stall exactly as long as the compression saved).
    let mut synth_pts = {
        let tail = shared.pace_tail_100ns.load(Ordering::Acquire);
        let real = cache.last().map(|f| f.pts_100ns).unwrap_or(0);
        if tail > 0 { tail } else { real }
    };
    let n = cache.len();
    let mut idx = n - 1;
    let mut forward = false;
    let mut last_pulse = shared.beat_pulse.load(Ordering::Acquire);
    // Sweep-law transport state: the 0→1 phase of the current beat step,
    // the pulse-learned beat period in presentation pts, and whether the
    // transport was on last tick (to derive the phase from the current
    // position at engage instead of teleporting to IN).
    let mut sweep_phase: f64 = 0.0;
    let mut beat_anchor_pts: Option<i64> = None;
    let mut beat_media: i64 = 0;
    let mut transport_was_on = false;
    let mut scratch_was_on = false;
    let mut scratch_pos = 0f64;
    // Trim changes do NOT bounce control back here: the IN/OUT bounds are
    // read LIVE each frame below, so a shrinking range just tightens the
    // space the repeat moves in — playback never resets. Control only goes
    // back when the range GROWS past the bounds this cache was BUILT
    // under (the decoder must fetch the uncovered part). The build bounds
    // are simply the bounds at entry: any earlier trim change cleared the
    // cache via the epoch watch, so a complete cache is always a product
    // of the current bounds. NEVER compare against frame pts — real MP4s
    // start at a nonzero pts, and measuring trim 0 against that offset
    // made every untrimmed clip look "uncovered" (the frozen-loop bug).
    let built_in = shared.trim_in_100ns.load(Ordering::Acquire);
    let built_out = shared.trim_out_100ns.load(Ordering::Acquire);
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        if shared.seek_100ns.load(Ordering::Acquire) >= 0 {
            return;
        }
        {
            let t_in = shared.trim_in_100ns.load(Ordering::Acquire);
            let t_out = shared.trim_out_100ns.load(Ordering::Acquire);
            if cache_range_outgrown((built_in, built_out), (t_in, t_out)) {
                return;
            }
        }
        let mode = PlayMode::from_u8(shared.mode.load(Ordering::Acquire));
        if mode == PlayMode::Once {
            return;
        }
        if shared.paused.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }
        if shared.frames.lock().unwrap().len() >= RING_FRAMES {
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        // Live IN/OUT bounds → the index range [lo, hi) the repeat may
        // touch (at least one frame wide). Loop wraps to lo, bounce
        // reflects at both.
        let t_in = shared.trim_in_100ns.load(Ordering::Acquire);
        let t_out = shared.trim_out_100ns.load(Ordering::Acquire);
        let lo = cache.partition_point(|f| f.pts_100ns < t_in).min(n - 1);
        let hi = cache.partition_point(|f| f.pts_100ns < t_out).clamp(lo + 1, n);
        if shared.scratch_active.load(Ordering::Acquire) {
            // SCRATCH: the hand owns the transport. Follow it within the
            // trim window, clamp at the edges, and mark the sweep
            // disengaged so release re-engages FROM THIS POSITION.
            if !scratch_was_on {
                scratch_was_on = true;
                scratch_pos = idx.clamp(lo, hi - 1) as f64;
            }
            let srate = f64::from_bits(
                shared.scratch_rate_bits.load(Ordering::Acquire),
            )
            .clamp(-8.0, 8.0);
            scratch_pos =
                (scratch_pos + srate).clamp(lo as f64, (hi - 1) as f64);
            idx = scratch_pos.round() as usize;
            forward = srate >= 0.0;
            transport_was_on = false;
            synth_pts += delta;
            shared.pace_tail_100ns.store(synth_pts, Ordering::Release);
            shared.frames.lock().unwrap().push_back(Frame {
                pts_100ns: synth_pts,
                clip_100ns: cache[idx].pts_100ns,
                bgra: cache[idx].bgra.clone(),
            });
            continue;
        }
        scratch_was_on = false;
        if shared.beat_transport.load(Ordering::Acquire) {
            let span = hi - lo;
            if !transport_was_on {
                transport_was_on = true;
                // Engage from WHERE THE PICTURE IS: derive the phase from
                // the current index so switching sync on (or entering the
                // cache at the end of the decode pass) continues the
                // motion instead of teleporting to IN.
                let u = if span > 1 {
                    (idx.clamp(lo, hi - 1) - lo) as f64 / (span - 1) as f64
                } else {
                    0.0
                };
                sweep_phase = if mode == PlayMode::PingPong && !forward {
                    1.0 - u
                } else {
                    u
                };
                if mode == PlayMode::Loop {
                    forward = true;
                }
            }
            // THE CHIP IS BEATS: one sweep spans exactly `beats` beat
            // periods (8/4/2/1). The grid is the pulse-learned period,
            // seeded by the app's HINT so the law paces from the very
            // first frame of a cue; only with neither (no clock at all —
            // impossible in the app) does the sweep fall to natural.
            let beats =
                shared.beats_per_sweep.load(Ordering::Acquire).max(1) as f64;
            let grid = if beat_media > 0 {
                beat_media as f64
            } else {
                shared.beat_hint_100ns.load(Ordering::Acquire) as f64
            };
            let sweep_pts = if grid > 0.0 {
                grid * beats
            } else {
                (span.max(2) as f64) * delta as f64
            };
            let step = (delta as f64 / sweep_pts).clamp(1e-6, 1.0);
            // Learn the beat grid from the pulses: pulse-to-pulse spacing
            // of the push clock IS the period in presentation pts (the
            // queue lead is the same on both sides and cancels). The
            // NUDGE, by contrast, wants the phase as PRESENTED at the
            // pulse — the pacer trails the newest push by the queue
            // depth, so that lead is backed out before comparing to the
            // grid. Correction is a bounded walk, never a snap.
            let pulse = shared.beat_pulse.load(Ordering::Acquire);
            if pulse != last_pulse {
                last_pulse = pulse;
                if let Some(prev) = beat_anchor_pts {
                    let period = synth_pts - prev;
                    // A beat spans 0.2s..2s (300..30 BPM); anything else
                    // is a missed pulse or a stall — keep the estimate.
                    // And a period ≈ 2x the current one IS a missed pulse
                    // (coalesced counter), not a tempo halving: adopting
                    // it would halve the sweep cadence for a beat.
                    let doubled = beat_media > 0
                        && (period as f64 / beat_media as f64) > 1.7;
                    if (2_000_000..20_000_000).contains(&period) && !doubled {
                        beat_media = period;
                    }
                }
                beat_anchor_pts = Some(synth_pts);
                if beat_media > 0 {
                    let qlen = shared.frames.lock().unwrap().len() as f64;
                    let presented = (sweep_phase - qlen * step).rem_euclid(1.0);
                    sweep_phase = (sweep_phase
                        + beat_phase_nudge(presented, beats))
                    .rem_euclid(1.0);
                }
            }
            let (p, dir) = advance_sweep(sweep_phase, forward, step, mode);
            sweep_phase = p;
            forward = dir;
            idx = sweep_index(sweep_phase, forward, lo, hi, mode);
            synth_pts += delta;
            shared.pace_tail_100ns.store(synth_pts, Ordering::Release);
            shared.frames.lock().unwrap().push_back(Frame {
                pts_100ns: synth_pts,
                clip_100ns: cache[idx].pts_100ns,
                bgra: cache[idx].bgra.clone(),
            });
            continue;
        }
        transport_was_on = false;
        if mode == PlayMode::Loop {
            forward = true;
            idx = if idx + 1 >= hi || idx + 1 <= lo { lo } else { idx + 1 };
            // ONE monotonic presentation timeline for every cache mode:
            // the free loop used to queue REAL clip pts (rebase-on-wrap),
            // and re-engaging the beat transport then resumed the synth
            // domain far AHEAD of the pacer's clock — a forward jump the
            // pacer only knew to WAIT out (at a slow chip that wait
            // doubled: the "0.5 stops dead" report). Source position
            // lives solely in clip_100ns.
            synth_pts += delta;
            shared.pace_tail_100ns.store(synth_pts, Ordering::Release);
            shared.frames.lock().unwrap().push_back(Frame {
                pts_100ns: synth_pts,
                clip_100ns: cache[idx].pts_100ns,
                bgra: cache[idx].bgra.clone(),
            });
            continue;
        } else if forward {
            if idx + 1 >= hi {
                forward = false;
                continue;
            }
            idx += 1;
        } else {
            if idx <= lo {
                forward = true;
                continue;
            }
            idx -= 1;
        }
        idx = idx.clamp(lo, hi - 1);
        synth_pts += delta;
        shared.pace_tail_100ns.store(synth_pts, Ordering::Release);
        shared.frames.lock().unwrap().push_back(Frame {
            pts_100ns: synth_pts,
            clip_100ns: cache[idx].pts_100ns,
            bgra: cache[idx].bgra.clone(),
        });
    }
}

/// TIER-3 bounce: a clip too big for the decoded-frame cache still plays
/// forward-backward, by GOP-BATCH REVERSE. A reverse leg walks windows from
/// the end of the clip: seek [`REVERSE_WINDOW_100NS`] back, forward-decode
/// that window ONCE into a bounded buffer, serve it newest-first — one seek
/// (plus the GOP walk hidden inside it) amortizes over the whole window's
/// backwards frames. Forward legs just decode. pts are synthesized
/// monotonically so the pacer never sees time reverse (the cache_playback
/// rule). Windows over [`REVERSE_WINDOW_MAX_BYTES`] keep their NEWEST
/// frames; the trimmed head is re-decoded by the next hop, so reverse stays
/// frame-exact on any format at the price of extra decode.
///
/// Runs while the mode stays PingPong and the slot stays silent (the bounce
/// law: reversed audio is not a thing). Returns Ok(true) when control goes
/// back to the normal loop (mode change / seek request / unmute / stop),
/// Ok(false) when the decoder's seek seam failed (the caller latches that
/// and falls back to loop), Err only for a real decode failure.
fn seek_bounce_playback(
    decoder: &mut VideoFileDecoder,
    shared: &Arc<SlotShared>,
    info: &VideoFileInfo,
) -> Result<bool, String> {
    /// Anything that hands control back to the normal decode loop —
    /// including a trim change (the bounce bounds moved under us).
    fn must_exit(shared: &SlotShared, has_audio: bool, epoch0: u64) -> bool {
        shared.stop.load(Ordering::Acquire)
            || shared.seek_100ns.load(Ordering::Acquire) >= 0
            || shared.trim_epoch.load(Ordering::Acquire) != epoch0
            || PlayMode::from_u8(shared.mode.load(Ordering::Acquire)) != PlayMode::PingPong
            || (has_audio && !shared.muted.load(Ordering::Acquire))
    }
    /// Pause-aware ring backpressure; true = exit requested.
    fn wait_ring(shared: &SlotShared, has_audio: bool, epoch0: u64) -> bool {
        loop {
            if must_exit(shared, has_audio, epoch0) {
                return true;
            }
            if shared.paused.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            if shared.frames.lock().unwrap().len() >= RING_FRAMES {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            return false;
        }
    }
    let has_audio = info.has_audio;
    let epoch0 = shared.trim_epoch.load(Ordering::Acquire);
    let duration = info.duration_100ns.max(0);
    // The bounce confines itself to the trim range like every other tier.
    let t_in = shared.trim_in_100ns.load(Ordering::Acquire).clamp(0, duration);
    let t_out = shared.trim_out_100ns.load(Ordering::Acquire).clamp(t_in, duration);
    let delta = if info.fps_num > 0 {
        ((10_000_000 * info.fps_den.max(1) as i64) / info.fps_num as i64).max(1)
    } else {
        416_667 // assume 24 fps when the container is silent about it
    };
    let mut rgb_scratch = Vec::new();
    // Continue the presentation clock from wherever the forward pass ended.
    let mut synth_pts = shared
        .frames
        .lock()
        .unwrap()
        .back()
        .map(|f| f.pts_100ns)
        .unwrap_or(0)
        .max(shared.position_100ns.load(Ordering::Acquire));
    let mut serve = |shared: &SlotShared, bgra: Vec<u32>, clip_100ns: i64, synth_pts: &mut i64| {
        *synth_pts += delta;
        shared
            .frames
            .lock()
            .unwrap()
            .push_back(Frame { pts_100ns: *synth_pts, clip_100ns, bgra });
        shared.video_ready.store(true, Ordering::Release);
    };
    loop {
        // ---- reverse leg: OUT → IN, in seek-batched windows.
        let mut hi = t_out;
        while hi > t_in {
            if wait_ring(shared, has_audio, epoch0) {
                return Ok(true);
            }
            let lo = (hi - REVERSE_WINDOW_100NS).max(t_in);
            if decoder.seek(lo).is_err() {
                return Ok(false);
            }
            let mut window: VecDeque<Frame> = VecDeque::new();
            let mut bytes = 0usize;
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    return Ok(true);
                }
                match decoder.next_frame() {
                    // A window seek lands on the prior keyframe: frames
                    // before IN never enter the window.
                    Ok(Some(f)) if f.pts_100ns + 400_000 < t_in => continue,
                    Ok(Some(f)) if f.pts_100ns < hi => {
                        let frame = convert_frame(f, &mut rgb_scratch);
                        bytes += frame.bgra.len() * 4;
                        window.push_back(frame);
                        while bytes > REVERSE_WINDOW_MAX_BYTES && window.len() > 1 {
                            let dropped = window.pop_front().unwrap();
                            bytes -= dropped.bgra.len() * 4;
                        }
                    }
                    Ok(Some(_)) | Ok(None) => break,
                    Err(e) => return Err(e.to_string()),
                }
            }
            let Some(first_kept) = window.front().map(|f| f.pts_100ns) else {
                // Dead air (no frames in the window): keep walking down.
                hi = lo;
                continue;
            };
            while let Some(frame) = window.pop_back() {
                if wait_ring(shared, has_audio, epoch0) {
                    return Ok(true);
                }
                serve(shared, frame.bgra, frame.clip_100ns, &mut synth_pts);
            }
            // Strictly decreasing: every kept frame had pts < hi.
            hi = first_kept;
        }
        // ---- forward leg: IN → OUT, a plain decode pass.
        if decoder.seek(t_in).is_err() {
            return Ok(false);
        }
        loop {
            if wait_ring(shared, has_audio, epoch0) {
                return Ok(true);
            }
            match decoder.next_frame() {
                Ok(Some(f)) if f.pts_100ns + 400_000 < t_in => continue,
                Ok(Some(f)) if f.pts_100ns < t_out => {
                    let frame = convert_frame(f, &mut rgb_scratch);
                    serve(shared, frame.bgra, frame.clip_100ns, &mut synth_pts);
                }
                Ok(Some(_)) | Ok(None) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// audio decode (music tracks + sfx pads)
// ---------------------------------------------------------------------------

/// Minimal bounded RIFF/WAVE parse (PCM16 + float32), same shape as the
/// ai-content player's parser, emitting interleaved stereo i16.
fn parse_wav(bytes: &[u8], max_frames: usize) -> Result<TrackPcm, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body_end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().unwrap());
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = body_end + (size & 1);
    }
    let data = data.ok_or("wav: no data chunk")?;
    if channels == 0 || sample_rate == 0 {
        return Err("wav: no fmt chunk".into());
    }
    let ch = channels as usize;
    let mut frames: Vec<[i16; 2]> = Vec::new();
    let push = |frames: &mut Vec<[i16; 2]>, l: i16, r: i16| -> Result<(), String> {
        if frames.len() >= max_frames {
            return Err("audio clip exceeds the decode budget".into());
        }
        frames.push([l, r]);
        Ok(())
    };
    match (format, bits) {
        (1, 16) => {
            for frame in data.chunks_exact(2 * ch) {
                let sample = |i: usize| {
                    i16::from_le_bytes(frame[i * 2..i * 2 + 2].try_into().unwrap())
                };
                push(&mut frames, sample(0), sample(ch - 1))?;
            }
        }
        (3, 32) => {
            for frame in data.chunks_exact(4 * ch) {
                let sample = |i: usize| {
                    let v = f32::from_le_bytes(frame[i * 4..i * 4 + 4].try_into().unwrap());
                    (v.clamp(-1.0, 1.0) * 32767.0) as i16
                };
                push(&mut frames, sample(0), sample(ch - 1))?;
            }
        }
        other => return Err(format!("wav: unsupported format {other:?}")),
    }
    if frames.is_empty() {
        return Err("wav: empty data".into());
    }
    Ok(TrackPcm { frames, sample_rate })
}

/// Decode an audio clip fully to memory. WAV parses directly, MP3 and Ogg
/// Vorbis go through this repo's own decoders, and MP4/M4A pulls the platform
/// decoder's audio track.
pub fn decode_audio_clip(
    path: &PathBuf,
    media: MediaType,
    max_frames: usize,
) -> Result<TrackPcm, String> {
    match media {
        MediaType::Wav => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            parse_wav(&bytes, max_frames)
        }
        MediaType::Mp4 => {
            // Cache objects are digest-only names; AVURLAsset keys off the
            // extension. Lease a typed hard link the same way video slots do.
            let input = DecoderInput::prepare(path, MediaType::Mp4)?;
            let mut decoder = VideoFileDecoder::open(&input.path).map_err(|e| e.to_string())?;
            if !decoder.info().has_audio {
                return Err("mp4 has no audio track".into());
            }
            let mut frames: Vec<[i16; 2]> = Vec::new();
            let mut sample_rate = decoder.info().audio_sample_rate.max(1);
            loop {
                match decoder.next_audio().map_err(|e| e.to_string())? {
                    None => break,
                    Some(chunk) => {
                        sample_rate = chunk.sample_rate.max(1);
                        let ch = chunk.channels.max(1) as usize;
                        for frame in chunk.samples.chunks_exact(ch) {
                            if frames.len() >= max_frames {
                                return Err("audio clip exceeds the decode budget".into());
                            }
                            frames.push([frame[0], frame[ch - 1]]);
                        }
                    }
                }
            }
            if frames.is_empty() {
                return Err("mp4 audio decoded to zero frames".into());
            }
            Ok(TrackPcm { frames, sample_rate })
        }
        // MP3 and Ogg Vorbis go through the repo's own decoders, the same way
        // WAV does: whole file in, interleaved PCM out, no platform codec.
        MediaType::Mp3 | MediaType::Ogg => {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            let format = if matches!(media, MediaType::Mp3) {
                AudioFormat::Mp3
            } else {
                AudioFormat::OggVorbis
            };
            let audio = decode_audio_limited(
                &bytes,
                format,
                AudioLimits::with_max_frames(max_frames),
            )
            .map_err(|e| e.to_string())?;
            let channels = audio.channels.max(1) as usize;
            let mut frames: Vec<[i16; 2]> = Vec::with_capacity(audio.frames());
            for frame in audio.pcm_interleaved_f32.chunks_exact(channels) {
                let sample = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                frames.push([sample(frame[0]), sample(frame[channels - 1])]);
            }
            if frames.is_empty() {
                return Err(format!("{format:?} decoded to zero frames"));
            }
            Ok(TrackPcm { frames, sample_rate: audio.rate.max(1) })
        }
        other => Err(format!("unsupported audio media {other:?}")),
    }
}

/// Min/max waveform columns over the whole clip.
pub fn wave_peaks(pcm: &TrackPcm, cols: usize) -> Vec<(f32, f32)> {
    let cols = cols.max(1);
    let mut out = Vec::with_capacity(cols);
    if pcm.frames.is_empty() {
        return vec![(0.0, 0.0); cols];
    }
    let per_col = pcm.frames.len() as f64 / cols as f64;
    for col in 0..cols {
        let start = ((col as f64 * per_col) as usize).min(pcm.frames.len() - 1);
        let end = (((col + 1) as f64 * per_col) as usize)
            .clamp(start + 1, pcm.frames.len());
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for frame in &pcm.frames[start..end] {
            let mono = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
            lo = lo.min(mono);
            hi = hi.max(mono);
        }
        out.push((lo, hi));
    }
    out
}

/// Render a waveform strip with played/unplayed regions and a playhead
/// column, as BGRA pixels for a texture.
pub fn waveform_bgra(
    peaks: &[(f32, f32)],
    width: usize,
    height: usize,
    played_fraction: f64,
) -> Vec<u32> {
    const BG: u32 = 0xff14_181c;
    const UNPLAYED: u32 = 0xff2f_6e5e;
    const PLAYED: u32 = 0xff58_c4a0;
    const MID: u32 = 0xff2a_3238;
    const HEAD: u32 = 0xffe8_e8e8;
    let mut out = vec![BG; width * height];
    if width == 0 || height == 0 || peaks.is_empty() {
        return out;
    }
    let mid_y = height / 2;
    for x in 0..width {
        out[mid_y * width + x] = MID;
    }
    let head_x = ((played_fraction.clamp(0.0, 1.0) * width as f64) as usize).min(width - 1);
    for x in 0..width {
        let peak = peaks[(x * peaks.len()) / width.max(1)];
        let color = if x <= head_x { PLAYED } else { UNPLAYED };
        let half = (height / 2) as f32;
        let y0 = (mid_y as f32 - peak.1.clamp(-1.0, 1.0) * (half - 1.0)) as usize;
        let y1 = (mid_y as f32 - peak.0.clamp(-1.0, 1.0) * (half - 1.0)) as usize;
        for y in y0.min(height - 1)..=y1.min(height - 1) {
            out[y * width + x] = color;
        }
    }
    for y in 0..height {
        out[y * width + head_x] = HEAD;
    }
    out
}

// ---------------------------------------------------------------------------
// UI-thread load budget
// ---------------------------------------------------------------------------

/// Anything the UI thread does for longer than this in one go is a dropped
/// frame the operator sees as a hitch — half a 60Hz frame, so the rest of
/// the frame still has room to draw.
pub const UI_STEP_BUDGET_MS: f32 = 8.0;

/// One UI-thread step of a content load, timed.
///
/// Everything expensive about loading is supposed to happen on the decode
/// pool; what is left on this thread is GPU work (buffer/texture creation)
/// that cannot happen anywhere else. This says whether that is still true:
/// the cost is folded into the F3 perf graph's own `load` channel, and a
/// step over [`UI_STEP_BUDGET_MS`] names itself in the log, so a hitch is
/// attributable from `/log` without the graph being open.
/// `VJ_TRACE_LOAD=1` also logs the steps that stayed INSIDE the budget —
/// how a before/after is measured once the hitches are gone.
fn trace_load() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("VJ_TRACE_LOAD").is_some())
}

#[must_use = "a step that is never `done` is never measured"]
pub struct UiStep {
    t0: Instant,
    what: &'static str,
}

impl UiStep {
    pub fn new(what: &'static str) -> Self {
        Self { t0: Instant::now(), what }
    }

    /// Close the step; returns its cost in milliseconds.
    pub fn done(self, cx: &mut makepad_widgets::Cx) -> f32 {
        let us = self.t0.elapsed().as_micros() as u64;
        let channel = cx.perf_monitor.channel("load", 0xff_b4_54);
        cx.perf_monitor.add(channel, us);
        let ms = us as f32 / 1000.0;
        if ms > UI_STEP_BUDGET_MS {
            makepad_widgets::log!(
                "ui-hitch: {} took {ms:.1}ms on the UI thread (budget {UI_STEP_BUDGET_MS:.1}ms)",
                self.what
            );
        } else if trace_load() {
            makepad_widgets::log!("ui-step: {} {ms:.2}ms", self.what);
        }
        ms
    }
}

// ---------------------------------------------------------------------------
// decode worker pool
// ---------------------------------------------------------------------------

pub enum DecodeJob {
    Deck { deck: DeckId, gen: u64, path: PathBuf, media: MediaType },
    Pad { pad: PadKey, gen: u64, revision: AssetRevisionId, path: PathBuf, media: MediaType },
    /// Read + parse + fully prepare a GLB for the 3D program slot: the UI
    /// thread only uploads the finished result.
    MeshPrep { gen: u64, path: PathBuf },
    /// Same prep, destined for a program slot (A/B overlay).
    /// Same prep for a program slot; `world` marks a walkable level, which
    /// the slot presents at authored scale instead of on a turntable.
    SlotMesh {
        gen: u64,
        slot: usize,
        path: PathBuf,
        world: bool,
        /// The body that will walk it, when it is a world — the SAME one the
        /// nav grid is built with here, so the graph and the legs agree.
        cfg: Option<makepad_render::level::WalkerConfig>,
    },
    /// Decode a still (PNG/JPEG) for a program slot.
    Still { gen: u64, slot: usize, path: PathBuf },
    /// Probe a freshly cued video for an embedded `mkfl` motion payload and,
    /// when present and within budget, decode the whole clip into the
    /// flow-warp endpoint cache (see `crate::flow_warp::prepare_flow_clip`).
    FlowClip { gen: u64, slot: usize, path: PathBuf },
    /// Local `.billboard` manifest with one PNG per frame beside it.
    Billboard { gen: u64, slot: usize, path: PathBuf },
    /// Catalog sprite actor: ONE packed sheet plus the `stateful-billboard`
    /// manifest text that says how to cut it (grouped Billboard assets).
    BillboardSheet { gen: u64, slot: usize, sheet: PathBuf, manifest: PathBuf },
    /// Read + decode a tile thumbnail into BGRA pixels (bounded); the UI
    /// thread only creates the texture.
    ///
    /// `sheet` is what the MANIFEST declared about the picture: the cell
    /// layout of a packed animation sheet and the rate its producer wrote
    /// down, or `None` for a still — and for the pre-contract revisions
    /// whose thumbnails say nothing, where `legacy_may_be_sheet` decides
    /// instead. `epoch` is the host's current visible-range generation
    /// (bumped whenever the grid's visible range changes); once the thumb
    /// lane has seen a newer epoch, any job still waiting from an older one
    /// is skipped without decoding when its turn comes — see `DecodePool`'s
    /// doc comment.
    Thumb {
        revision: AssetRevisionId,
        path: PathBuf,
        sheet: Option<(ThumbnailCells, f32)>,
        legacy_may_be_sheet: bool,
        epoch: u64,
    },
}

/// Largest GLB the mesh lane will lift into memory.
pub const MAX_MESH_BYTES: u64 = 256 * 1024 * 1024;
/// Mesh admission gates before anything reaches the UI thread.
pub const MAX_MESH_JOINTS: usize = 256;
pub const MAX_MESH_VERTICES: usize = 2_000_000;
pub const MAX_MESH_CLIPS: usize = 64;
/// Largest encoded thumbnail the grid will download/decode, and the pixel
/// dimension cap (catalog publications should stay well under both).
pub const MAX_THUMB_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_THUMB_DIM: usize = 2048;
/// Stills on A/B can be larger than grid thumbs.
pub const MAX_STILL_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_STILL_DIM: usize = 4096;
/// A grouped sprite actor packs every frame of every state into one sheet,
/// so it is allowed to be bigger than a still — still bounded.
pub const MAX_SHEET_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_SHEET_DIM: usize = 8192;
/// Largest `stateful-billboard` manifest text the slot lane will read.
pub const MAX_BILLBOARD_TEXT_BYTES: u64 = 4 * 1024 * 1024;

/// Everything the 3D slot needs, prepared off-thread. The UI's remaining
/// work is GPU-only: upload the rest bundle / decode-free texture create.
pub enum PreparedMesh {
    Skinned {
        model: Box<makepad_render::skin::SkinnedModel>,
        rest: makepad_render::skin::SkinRestGpu,
        clip: usize,
        scale: f32,
        lift: f32,
        /// Embedded base-color image bytes (PNG/JPEG), when present.
        base_color: Option<Vec<u8>>,
    },
    /// Unskinned fallback: a static prop or a walkable level. PARSED here —
    /// the GLB parse is 30ms of a 35ms Doom-level load and needs no `Cx`, so
    /// the UI thread is left with the GPU upload alone
    /// (`Renderer::load_model_parsed`). The level's triangle collision and
    /// nav grid are built here for the same reason.
    Statue {
        model: Box<makepad_render::StaticModel>,
        base_color: Option<Vec<u8>>,
        level: Option<Box<makepad_render::level::LevelCollision>>,
        /// The config the nav grid below was built with. The walker MUST be
        /// given this one: a graph probed with one body and walked by
        /// another offers steps the legs refuse.
        nav_cfg: Option<makepad_render::level::WalkerConfig>,
        /// Walkable-cell graph over the whole map, so the tour plans routes
        /// instead of scoring the twelve headings in front of its nose.
        /// Tens of thousands of probes: worker work, never a frame's.
        nav: Option<Box<makepad_render::level::NavGrid>>,
        /// Interior spawn found while the collision was being built — the
        /// grid scan is thousands of ray casts and must not run on the UI
        /// thread when a map is cued.
        start: Option<makepad_widgets::Vec3f>,
    },
}

pub enum DecodeDone {
    Deck {
        deck: DeckId,
        gen: u64,
        result: Result<(Arc<TrackPcm>, Vec<(f32, f32)>), String>,
    },
    Pad {
        pad: PadKey,
        gen: u64,
        revision: AssetRevisionId,
        result: Result<Arc<TrackPcm>, String>,
    },
    MeshPrep {
        gen: u64,
        result: Result<Box<PreparedMesh>, String>,
    },
    SlotMesh {
        gen: u64,
        slot: usize,
        world: bool,
        result: Result<Box<PreparedMesh>, String>,
    },
    Still {
        gen: u64,
        slot: usize,
        result: Result<(Vec<u32>, usize, usize), String>,
    },
    /// `Ok(None)` is the honest no-flow outcome (no mkfl / over budget /
    /// unmappable geometry): the slot keeps playing exactly as today.
    FlowClip {
        gen: u64,
        slot: usize,
        result: Result<Option<Box<crate::flow_warp::FlowClipData>>, String>,
    },
    Billboard {
        gen: u64,
        slot: usize,
        result: Result<Box<crate::billboard::PreparedBillboard>, String>,
    },
    Thumb {
        revision: AssetRevisionId,
        result: Result<ThumbPixels, String>,
    },
}

/// Admission gate for parsed skinned meshes — checked on the worker BEFORE
/// any CPU-heavy preparation or UI upload.
pub fn mesh_gate(bytes: u64, joints: usize, vertices: usize, clips: usize) -> Result<(), String> {
    if bytes > MAX_MESH_BYTES {
        return Err(format!("glb over budget: {bytes} bytes"));
    }
    if joints > MAX_MESH_JOINTS {
        return Err(format!("too many joints: {joints}"));
    }
    if vertices > MAX_MESH_VERTICES {
        return Err(format!("too many vertices: {vertices}"));
    }
    if clips > MAX_MESH_CLIPS {
        return Err(format!("too many clips: {clips}"));
    }
    Ok(())
}

/// Clip preference for the dance lane: dance first, then common loops.
pub const CLIP_PREFERENCE: [&str; 4] = ["dance", "idle", "walk", "run"];

/// Triangle collision for a walkable level, built off the UI thread. The
/// renderer's prop collider is a box decomposition (a few dozen boxes for a
/// whole map): a walker standing on those stands in mid-air, so the level's
/// own triangles are indexed instead.
fn build_level(
    model: &makepad_render::StaticModel,
    glb: &[u8],
    cfg: &makepad_render::level::WalkerConfig,
) -> (
    Option<Box<makepad_render::level::LevelCollision>>,
    Option<Box<makepad_render::level::NavGrid>>,
    Option<makepad_widgets::Vec3f>,
) {
    use makepad_render::level::{
        surface_kinds_from_glb, LevelCollision, NavGrid, SurfaceKind, UpAxis,
    };
    use makepad_render::model::MODEL_VERTEX_FLOATS;
    // Every classic pack publishes Y-up (the importer converts).
    let Some(level) =
        LevelCollision::from_packed(&model.vertices, MODEL_VERTEX_FLOATS, &model.indices, UpAxis::Y)
    else {
        return (None, None, None);
    };
    // Which floors hurt: the importer's `hazard_N` nodes, or the source
    // engine's flat names on older publications. Without this every floor is
    // plain and the tour happily paddles through the nukage.
    let level = match surface_kinds_from_glb(glb, model.triangle_count()) {
        Some(kinds) => {
            let hazard = kinds.iter().filter(|k| **k == SurfaceKind::Hazard).count();
            let liquid = kinds.iter().filter(|k| **k == SurfaceKind::Liquid).count();
            makepad_widgets::log!(
                "vj level: {hazard} hazard + {liquid} liquid of {} triangles classified",
                kinds.len()
            );
            level.with_kinds(kinds)
        }
        None => {
            makepad_widgets::log!(
                "vj level: no per-triangle surface kinds (no hazard_N nodes, no flat names) \
                 — every floor is plain until the map is re-imported"
            );
            level
        }
    };
    // The nav grid is the expensive part (a capsule probe per cell, a wall
    // probe per edge) and the reason this whole function is off-thread.
    let started = std::time::Instant::now();
    let nav = NavGrid::build(&level, &cfg);
    let (nx, nz) = nav.dims();
    use makepad_widgets::log;
    // The spawn comes from the graph: the middle of the biggest piece of
    // the map that is actually one piece. Only a level with no graph at all
    // falls back to the old open-space scan.
    let start = nav.best_start().and_then(|c| nav.cell(c).map(|c| c.pos));
    let r = nav.refusals();
    makepad_widgets::log!(
        "vj level: refused edges — {} too tall (smallest refused rise {:.4}, step limit {:.3}), \
         {} too deep, {} walled; {} escape links added; components {:?}",
        r.too_tall,
        r.smallest_refused_rise,
        cfg.step_up,
        r.too_deep,
        r.walled,
        r.escapes,
        &nav.component_sizes()[..nav.component_sizes().len().min(6)]
    );
    log!(
        "vj level: {} triangles, nav {}×{} columns @ {:.2}, {} cells, {} edges in {:?}",
        level.triangles(),
        nx,
        nz,
        nav.cell_size(),
        nav.len(),
        nav.edge_count(),
        started.elapsed()
    );
    let start = start.or_else(|| level.interior_start(&cfg));
    let nav = (!nav.is_empty()).then(|| Box::new(nav));
    (Some(Box::new(level)), nav, start)
}

fn prepare_mesh(
    path: &PathBuf,
    world: Option<makepad_render::level::WalkerConfig>,
) -> Result<Box<PreparedMesh>, String> {
    use makepad_render::skin::{SkinnedModel, SKIN_VERTEX_FLOATS};
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_MESH_BYTES {
        return Err(format!("glb over budget: {} bytes", meta.len()));
    }
    let glb = std::fs::read(path).map_err(|e| e.to_string())?;
    let base_color = extract_base_color(&glb);
    // Skinned first; an unskinned model (generated props, painted TRELLIS
    // meshes) does not parse as one — it becomes a statue if it is a valid
    // static GLB. Only a GLB neither parser accepts is a failure.
    let model = match SkinnedModel::parse_glb(&glb) {
        Ok(model) => model,
        Err(skin_error) => {
            let static_model = makepad_render::StaticModel::parse_glb(&glb)
                .map_err(|e| format!("mesh parse failed: {e} (skinned: {skin_error})"))?;
            let (level, nav, start) = match world.as_ref() {
                Some(cfg) => build_level(&static_model, &glb, cfg),
                None => (None, None, None),
            };
            // The SAME parse the renderer would have done on the UI thread:
            // it travels instead of the bytes.
            return Ok(Box::new(PreparedMesh::Statue {
                model: Box::new(static_model),
                base_color,
                level,
                nav_cfg: world,
                nav,
                start,
            }));
        }
    };
    let playable =
        model.joint_count() > 0 && model.joint_count() <= MAX_MESH_JOINTS && !model.clips.is_empty();
    if !playable {
        let static_model = makepad_render::StaticModel::parse_glb(&glb)
            .map_err(|e| format!("mesh parse failed: {e}"))?;
        return Ok(Box::new(PreparedMesh::Statue {
            model: Box::new(static_model),
            base_color,
            level: None,
            nav: None,
            nav_cfg: None,
            start: None,
        }));
    }
    mesh_gate(
        meta.len(),
        model.joint_count(),
        model.vertex_count(),
        model.clips.len(),
    )?;
    let clip = model.clip_index_any(&CLIP_PREFERENCE).unwrap_or(0);
    // Measure the rest pose (CPU-skin once) for human height + ground lift.
    let (scale, lift) = {
        let rest = model.rest_pose();
        let mut palette = Vec::new();
        model.palette(&rest, &mut palette);
        let mut packed = Vec::new();
        model.skin_to_packed(&palette, &mut packed);
        let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
        for v in packed.chunks_exact(SKIN_VERTEX_FLOATS) {
            min_y = min_y.min(v[1]);
            max_y = max_y.max(v[1]);
        }
        let height = (max_y - min_y).max(0.01);
        let scale = 1.75 / height;
        (scale, -min_y * scale)
    };
    // Flat-AO rest bundle built off-thread; the UI only uploads it.
    let rest = model.rest_gpu_flat();
    Ok(Box::new(PreparedMesh::Skinned {
        model: Box::new(model),
        rest,
        clip,
        scale,
        lift,
        base_color,
    }))
}

/// Base-color image bytes out of a GLB, when it embeds one (material 0's
/// baseColorTexture source, else image 0).
pub fn extract_base_color(glb: &[u8]) -> Option<Vec<u8>> {
    let loaded = makepad_gltf::load_gltf_from_bytes(glb, None).ok()?;
    let doc = &loaded.document;
    let image_index = doc
        .materials_slice()
        .first()
        .and_then(|m| m.pbr_metallic_roughness.as_ref())
        .and_then(|pbr| pbr.base_color_texture.as_ref())
        .and_then(|info| doc.textures_slice().get(info.index))
        .and_then(|tex| tex.source)
        .or(if doc.images_slice().is_empty() { None } else { Some(0) })?;
    makepad_gltf::load_image_bytes(&loaded, image_index).ok()
}

#[derive(Debug)]
pub struct ThumbPixels {
    pub bgra: Vec<u32>,
    pub width: usize,
    pub height: usize,
    /// Native-size frames when this is a 128² anim sheet or a `.billboard`.
    /// Each tuple is `(bgra, width, height)` so later frames can differ.
    pub frames: Vec<(Vec<u32>, usize, usize)>,
    pub fps: f32,
}

/// Largest thumbnail a tile ever needs. A grid card is 164x104 layout
/// points, so 512 still has pixels to spare on a 2x display.
///
/// Measured: uploading 1024² stills whole cost 4-6ms of UI thread EACH, and
/// a grid filling with 105 of them spent 254ms hitching (12 batches over the
/// 8ms budget) for detail no tile can show. A quarter of the pixels is a
/// quarter of the upload.
pub const MAX_TILE_TEX_DIM: usize = 512;

/// Read + decode a thumbnail into BGRA, refusing oversized or malformed
/// images before any pixel reaches the UI thread, and shrinking anything
/// bigger than a tile can draw ([`MAX_TILE_TEX_DIM`]) while still off it.
///
/// A picture that DECLARED its cell layout is cut at that layout, exactly:
/// the frames the producer wrote, at the rate it wrote them, with the clear
/// padding it added for the thumbnail height floor simply not among them —
/// down to a ONE-cell declaration, which is a still of that cell (see
/// [`declared_thumb`]). A picture that declared nothing is a still — unless
/// it is old enough to predate the declaration, which is what
/// `legacy_may_be_sheet` is for.
fn decode_thumb(
    path: &PathBuf,
    sheet: Option<(ThumbnailCells, f32)>,
    legacy_may_be_sheet: bool,
) -> Result<ThumbPixels, String> {
    let mut pixels = decode_thumb_full(path, sheet, legacy_may_be_sheet)?;
    fit_thumb_for_tiles(&mut pixels);
    Ok(pixels)
}

/// Whole-integer box shrink so `w`x`h` fits [`MAX_TILE_TEX_DIM`]; 1 = leave
/// it alone. Integer factors keep the filter exact (every destination pixel
/// averages the same number of sources) and every sprite cell — 128² — at
/// its authored size.
fn shrink_factor(w: usize, h: usize) -> usize {
    let long = w.max(h);
    if long <= MAX_TILE_TEX_DIM {
        return 1;
    }
    long.div_ceil(MAX_TILE_TEX_DIM)
}

/// Average `factor`x`factor` blocks of BGRA into one pixel, over PREMULTIPLIED
/// colour so a keyed sprite's transparent pixels do not darken its edges.
/// Trailing pixels that do not fill a whole block are dropped rather than
/// weighted differently — at most `factor - 1` of them.
fn box_shrink(src: &[u32], w: usize, h: usize, factor: usize) -> (Vec<u32>, usize, usize) {
    let (dw, dh) = (w / factor, h / factor);
    if dw == 0 || dh == 0 || src.len() < w * h {
        return (src.to_vec(), w, h);
    }
    let mut out = vec![0u32; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let (mut a, mut r, mut g, mut b) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..factor {
                let row = (y * factor + sy) * w + x * factor;
                for sx in 0..factor {
                    let px = src[row + sx];
                    let pa = (px >> 24) & 0xff;
                    a += pa;
                    r += ((px >> 16) & 0xff) * pa;
                    g += ((px >> 8) & 0xff) * pa;
                    b += (px & 0xff) * pa;
                }
            }
            out[y * dw + x] = if a == 0 {
                0
            } else {
                let n = (factor * factor) as u32;
                ((a / n) << 24) | ((r / a) << 16) | ((g / a) << 8) | (b / a)
            };
        }
    }
    (out, dw, dh)
}

/// Shrink a decoded thumbnail — still frame and animation frames alike — to
/// what a tile can draw. A no-op for everything already small (every sprite
/// cell, every 128² strip tile), which is why it can be unconditional.
fn fit_thumb_for_tiles(pixels: &mut ThumbPixels) {
    let factor = shrink_factor(pixels.width, pixels.height);
    if factor > 1 {
        let (data, w, h) = box_shrink(&pixels.bgra, pixels.width, pixels.height, factor);
        pixels.bgra = data;
        pixels.width = w;
        pixels.height = h;
    }
    for (data, w, h) in pixels.frames.iter_mut() {
        let factor = shrink_factor(*w, *h);
        if factor > 1 {
            let (shrunk, sw, sh) = box_shrink(data, *w, *h, factor);
            *data = shrunk;
            *w = sw;
            *h = sh;
        }
    }
}

fn decode_thumb_full(
    path: &PathBuf,
    sheet: Option<(ThumbnailCells, f32)>,
    legacy_may_be_sheet: bool,
) -> Result<ThumbPixels, String> {
    if path.extension().and_then(|e| e.to_str()) == Some("billboard") {
        return decode_billboard_thumb(path);
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_THUMB_BYTES {
        return Err(format!("thumbnail over byte budget: {}", meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let image = if bytes.starts_with(&[0xff, 0xd8]) {
        makepad_widgets::ImageBuffer::from_jpg(&bytes)
    } else {
        makepad_widgets::ImageBuffer::from_png(&bytes)
    }
    .map_err(|e| format!("thumbnail decode failed: {e:?}"))?;
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 || w > MAX_THUMB_DIM || h > MAX_THUMB_DIM {
        return Err(format!("thumbnail dimensions out of bounds: {w}x{h}"));
    }
    let mut data = image.data;
    if data.len() > w * h {
        data.truncate(w * h);
    }
    key_sprite_alpha(&mut data);
    if let Some((cells, fps)) = sheet {
        if let Some(cut) = declared_thumb(w, h, &data, cells, fps) {
            return Ok(cut);
        }
    } else if let Some(frames) = legacy_may_be_sheet
        .then(|| legacy_split_sheet_bgra(w, h, &data))
        .flatten()
    {
        let first = frames.first().cloned().unwrap_or_default();
        let seq = frames
            .into_iter()
            .map(|frame| (frame, SHEET_TILE, SHEET_TILE))
            .collect::<Vec<_>>();
        return Ok(ThumbPixels {
            bgra: first,
            width: SHEET_TILE,
            height: SHEET_TILE,
            frames: seq,
            fps: LEGACY_SHEET_FPS,
        });
    }
    Ok(ThumbPixels {
        width: w,
        height: h,
        frames: Vec::new(),
        fps: 0.0,
        bgra: data,
    })
}

/// What a DECLARED cell layout means for a decoded picture, once its cells
/// are cut. `None` = the declaration bought nothing (a stale stamp whose
/// range lies outside the picture), so the caller draws the whole image.
///
/// TWO cells or more cycle. ONE cell is a STILL **of that cell** — never of
/// the whole picture: a single-frame sprite actor (`bpak`, `clip`, `cand`,
/// every Doom pickup) publishes its preview as one painted 128² tile on a
/// 1024x256 strip, the rest clear padding bought to clear the 256px
/// published-thumbnail floor. Drawing the strip put that tile in the corner
/// of a picture eight times as wide as it — the tiny top-left sprites the
/// SPRITE shelf showed. The tile itself is already the frame's content,
/// aspect-fit and CENTRED by the producer (`anim_icon::fit_tile`), so the
/// cell IS the content rect and nothing here has to measure pixels.
///
/// Same precedence as the shared cutter in `makepad-asset-widgets`
/// (`thumb::plan_views`: an `Anim` view of one cell is a still of it).
fn declared_thumb(
    width: usize,
    height: usize,
    data: &[u32],
    cells: ThumbnailCells,
    fps: f32,
) -> Option<ThumbPixels> {
    let (cw, ch) = (cells.cell_w.max(1) as usize, cells.cell_h.max(1) as usize);
    let mut frames = cut_declared_cells(width, height, data, cells);
    match frames.len() {
        0 => None,
        1 => Some(ThumbPixels {
            bgra: frames.remove(0),
            width: cw,
            height: ch,
            frames: Vec::new(),
            fps: 0.0,
        }),
        _ => Some(ThumbPixels {
            bgra: frames[0].clone(),
            width: cw,
            height: ch,
            frames: frames.into_iter().map(|f| (f, cw, ch)).collect(),
            fps,
        }),
    }
}

/// Cut the cells a manifest NAMED, row-major from the declared origin. A
/// range that runs off the picture stops rather than reading whatever is
/// next in memory.
fn cut_declared_cells(
    width: usize,
    height: usize,
    data: &[u32],
    cells: ThumbnailCells,
) -> Vec<Vec<u32>> {
    let (cw, ch) = (cells.cell_w.max(1) as usize, cells.cell_h.max(1) as usize);
    let cols = cells.cols.max(1) as usize;
    let mut frames = Vec::new();
    for i in 0..cells.count as usize {
        let index = cells.first as usize + i;
        let (ox, oy) = ((index % cols) * cw, (index / cols) * ch);
        if ox + cw > width || oy + ch > height || data.len() < width * height {
            break;
        }
        let mut tile = vec![0u32; cw * ch];
        for y in 0..ch {
            let src = (oy + y) * width + ox;
            tile[y * cw..(y + 1) * cw].copy_from_slice(&data[src..src + cw]);
        }
        frames.push(tile);
    }
    frames
}

fn decode_billboard_thumb(path: &PathBuf) -> Result<ThumbPixels, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let bb = crate::billboard::Manifest::parse(&text)?;
    let root = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut frames = Vec::new();
    // A sheet-backed manifest decodes ONE image and cuts the preview
    // frames out of it; older ones have a PNG per frame.
    match crate::billboard::sheet_beside(&bb, root) {
        Some(sheet) => {
            let (pixels, w, h) = sheet?;
            let layout = bb.sheet.ok_or("sheet manifest without a layout")?;
            for frame in bb.preview_frames() {
                let Some(cell) = frame.cell else { continue };
                if let Ok(pix) = crate::billboard::cut_cell(
                    &pixels,
                    w,
                    h,
                    layout,
                    cell,
                    frame.w as usize,
                    frame.h as usize,
                ) {
                    frames.push(pix);
                }
            }
        }
        None => {
            for frame in bb.preview_frames() {
                if let Ok(pix) = crate::billboard::decode_frame(&root.join(&frame.file)) {
                    frames.push(pix);
                }
            }
        }
    }
    let (bgra, width, height) = frames.first().cloned().ok_or("billboard empty")?;
    Ok(ThumbPixels {
        bgra,
        width,
        height,
        frames,
        fps: bb.preview_fps(),
    })
}

const SHEET_TILE: usize = 128;
const SHEET_W: usize = 1024;
/// Playback rate for a sheet that did NOT declare one. Only pre-contract
/// revisions reach it; everything else runs at the rate its producer wrote.
const LEGACY_SHEET_FPS: f32 = 8.0;
/// Studio-clear padding used by packed 128² sheets (asset-ui `anim_icon`).
const SHEET_CLEAR: u32 = 0xFF1A1F29;

/// LEGACY ONLY: true for the 1024-wide packed sheet, or any single-row
/// 128-tall strip with at least two tiles. Regular square thumbs
/// (256/512/1024) stay still. This is the guess a declared layout replaced —
/// it gets a 1024-square render wrong, and always did.
fn legacy_is_anim_sheet(width: usize, height: usize) -> bool {
    if width < SHEET_TILE * 2 || height < SHEET_TILE {
        return false;
    }
    if width % SHEET_TILE != 0 || height % SHEET_TILE != 0 {
        return false;
    }
    height == SHEET_TILE || width == SHEET_W
}

fn is_sheet_clear(p: u32) -> bool {
    let a = (p >> 24) & 0xff;
    a == 0 || p == SHEET_CLEAR
}

/// LEGACY ONLY: split a decoded BGRA sheet into 128² frames by measuring it.
/// Reserved for revisions published before a thumbnail declared its layout;
/// delete with the last of them.
fn legacy_split_sheet_bgra(width: usize, height: usize, data: &[u32]) -> Option<Vec<Vec<u32>>> {
    if !legacy_is_anim_sheet(width, height) || data.len() < width * height {
        return None;
    }
    let cols = width / SHEET_TILE;
    let rows = height / SHEET_TILE;
    let mut frames = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let mut tile = vec![0u32; SHEET_TILE * SHEET_TILE];
            for y in 0..SHEET_TILE {
                let src = (row * SHEET_TILE + y) * width + col * SHEET_TILE;
                let dst = y * SHEET_TILE;
                tile[dst..dst + SHEET_TILE].copy_from_slice(&data[src..src + SHEET_TILE]);
            }
            let painted = tile.iter().filter(|&&p| !is_sheet_clear(p)).count();
            if painted >= 16 {
                frames.push(tile);
            }
        }
    }
    (frames.len() > 1).then_some(frames)
}

/// Decode a packed sprite sheet + its manifest into playable states. Both
/// files are bounded before a byte is decoded; the sheet is alpha-keyed once
/// (classic sprites are magenta-keyed) and then cut per cell.
fn prepare_billboard_sheet(
    sheet: &PathBuf,
    manifest: &PathBuf,
) -> Result<crate::billboard::PreparedBillboard, String> {
    let text_meta = std::fs::metadata(manifest).map_err(|e| e.to_string())?;
    if text_meta.len() > MAX_BILLBOARD_TEXT_BYTES {
        return Err(format!("billboard manifest over budget: {}", text_meta.len()));
    }
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let meta = std::fs::metadata(sheet).map_err(|e| e.to_string())?;
    if meta.len() > MAX_SHEET_BYTES {
        return Err(format!("sprite sheet over byte budget: {}", meta.len()));
    }
    let bytes = std::fs::read(sheet).map_err(|e| e.to_string())?;
    let image = if bytes.starts_with(&[0xff, 0xd8]) {
        makepad_widgets::ImageBuffer::from_jpg(&bytes)
    } else {
        makepad_widgets::ImageBuffer::from_png(&bytes)
    }
    .map_err(|e| format!("sprite sheet decode failed: {e:?}"))?;
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 || w > MAX_SHEET_DIM || h > MAX_SHEET_DIM {
        return Err(format!("sprite sheet dimensions out of bounds: {w}x{h}"));
    }
    let mut data = image.data;
    if data.len() > w * h {
        data.truncate(w * h);
    }
    key_sprite_alpha(&mut data);
    crate::billboard::prepare_from_sheet(&text, &data, w, h)
}

fn decode_still(path: &PathBuf) -> Result<(Vec<u32>, usize, usize), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_STILL_BYTES {
        return Err(format!("still over byte budget: {}", meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let image = if bytes.starts_with(&[0xff, 0xd8]) {
        makepad_widgets::ImageBuffer::from_jpg(&bytes)
    } else {
        makepad_widgets::ImageBuffer::from_png(&bytes)
    }
    .map_err(|e| format!("still decode failed: {e:?}"))?;
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 || w > MAX_STILL_DIM || h > MAX_STILL_DIM {
        return Err(format!("still dimensions out of bounds: {w}x{h}"));
    }
    let mut data = image.data;
    key_sprite_alpha(&mut data);
    Ok((data, w, h))
}

/// Classic Build/Doom sprites key out magenta (and keep real PNG alpha).
/// Pixels are `0xAARRGGBB`.
pub fn key_sprite_alpha(pixels: &mut [u32]) {
    for px in pixels {
        let a = (*px >> 24) & 0xff;
        let r = (*px >> 16) & 0xff;
        let g = (*px >> 8) & 0xff;
        let b = *px & 0xff;
        let magenta = r >= 200 && b >= 200 && g <= 48;
        if magenta || a < 8 {
            *px = 0;
        }
    }
}

/// Sizing policy for the two decode lanes, factored out so it is testable in
/// isolation from real threads. `cpus` is
/// `std::thread::available_parallelism()`'s count (or 1 if the platform
/// can't answer). Returns `(heavy_workers, thumb_workers)`.
///
/// Heavy lane (Deck/Pad audio, MeshPrep, SlotMesh, Still, Billboard,
/// BillboardSheet): these are the seconds-scale jobs — mesh prep is ~1s,
/// a level's nav/collision build another ~1s, a full track decode longer
/// still — so the lane scales with the machine, floored at 2 (a
/// single-core box still overlaps two decodes) and capped at 8 (past that,
/// more threads just add disk/GPU-upload contention without shortening the
/// queue).
///
/// Thumb lane: deliberately small (2..=4) and only loosely tied to core
/// count. A thumbnail decode is a few milliseconds of work bounded by
/// `MAX_THUMB_DIM`, so throughput isn't core-starved the way heavy jobs
/// are — a handful of dedicated workers is enough to keep a scrolling grid
/// fed, and a bigger lane would only buy more `MAX_THUMB_DIM²` buffers live
/// at once (see the memory note on `DecodePool`) for no real gain.
fn lane_sizes(cpus: usize) -> (usize, usize) {
    let heavy = cpus.clamp(2, 8);
    let thumb = (cpus / 2).clamp(2, 4);
    (heavy, thumb)
}

/// Bound on the thumb lane's pending stack: past this many queued-but-not-
/// started thumbnails, the OLDEST pending job (the one furthest from the
/// current view — it was requested longest ago) is dropped to make room.
/// Keeps a fast scroll from growing the backlog without limit.
const MAX_PENDING_THUMBS: usize = 64;

struct PendingThumb {
    revision: AssetRevisionId,
    path: PathBuf,
    sheet: Option<(ThumbnailCells, f32)>,
    legacy_may_be_sheet: bool,
    epoch: u64,
}

struct ThumbQueueState {
    /// Push at the back, pop from the back: a stack, not a FIFO queue.
    stack: VecDeque<PendingThumb>,
    newest_epoch: u64,
    closed: bool,
}

/// LIFO job source shared by the thumb lane's workers. See `DecodePool`'s
/// doc comment for the full ordering/epoch/cap contract.
struct ThumbQueue {
    state: Mutex<ThumbQueueState>,
    cv: Condvar,
}

impl ThumbQueue {
    fn new() -> ThumbQueue {
        ThumbQueue {
            state: Mutex::new(ThumbQueueState {
                stack: VecDeque::new(),
                newest_epoch: 0,
                closed: false,
            }),
            cv: Condvar::new(),
        }
    }

    fn push(&self, job: PendingThumb) {
        let mut state = self.state.lock().unwrap();
        if job.epoch > state.newest_epoch {
            state.newest_epoch = job.epoch;
            // A new visible range makes every pending job for the old one
            // dead weight. Dropping them HERE rather than at pop keeps the
            // backlog honest: a fast scroll leaves no queue behind it.
            let newest = state.newest_epoch;
            state.stack.retain(|j| j.epoch >= newest);
        }
        state.stack.push_back(job);
        while state.stack.len() > MAX_PENDING_THUMBS {
            state.stack.pop_front(); // drop the oldest pending job
        }
        self.cv.notify_one();
    }

    /// Blocks until a live job is available or the queue is closed. Stale
    /// jobs (epoch older than the newest one this queue has seen) are
    /// popped and dropped in place, never decoded — they've certainly
    /// scrolled out of view by the time their turn comes.
    fn pop(&self) -> Option<PendingThumb> {
        let mut state = self.state.lock().unwrap();
        loop {
            while let Some(job) = state.stack.pop_back() {
                if job.epoch >= state.newest_epoch {
                    return Some(job);
                }
            }
            if state.closed {
                return None;
            }
            state = self.cv.wait(state).unwrap();
        }
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        self.cv.notify_all();
    }
}

#[cfg(test)]
fn test_sleep_marker(path: &Path) -> Option<Duration> {
    let ms: u64 = path
        .file_stem()?
        .to_str()?
        .strip_prefix("vj_test_sleep_")?
        .parse()
        .ok()?;
    Some(Duration::from_millis(ms))
}

fn run_heavy_job(job: DecodeJob) -> DecodeDone {
    match job {
        DecodeJob::Deck { deck, gen, path, media } => {
            let result = decode_audio_clip(&path, media, MAX_TRACK_FRAMES).map(|pcm| {
                let peaks = wave_peaks(&pcm, WAVE_COLS);
                (Arc::new(pcm), peaks)
            });
            DecodeDone::Deck { deck, gen, result }
        }
        DecodeJob::Pad { pad, gen, revision, path, media } => {
            let result = decode_audio_clip(&path, media, MAX_PAD_FRAMES).map(Arc::new);
            DecodeDone::Pad { pad, gen, revision, result }
        }
        DecodeJob::MeshPrep { gen, path } => {
            #[cfg(test)]
            if let Some(delay) = test_sleep_marker(&path) {
                std::thread::sleep(delay);
            }
            let result = prepare_mesh(&path, None);
            DecodeDone::MeshPrep { gen, result }
        }
        DecodeJob::SlotMesh { gen, slot, path, world, cfg } => {
            let result = prepare_mesh(&path, cfg.filter(|_| world));
            DecodeDone::SlotMesh { gen, slot, world, result }
        }
        DecodeJob::Still { gen, slot, path } => {
            let result = decode_still(&path);
            DecodeDone::Still { gen, slot, result }
        }
        DecodeJob::FlowClip { gen, slot, path } => {
            // The platform decoder needs an extension-bearing path for
            // digest-named cache objects; the alias lease drops (and the
            // link is removed) only after the full decode finished.
            let result = DecoderInput::prepare(&path, MediaType::Mp4).and_then(|input| {
                crate::flow_warp::prepare_flow_clip(Path::new(&input.path))
            });
            DecodeDone::FlowClip { gen, slot, result }
        }
        DecodeJob::Billboard { gen, slot, path } => {
            let result = crate::billboard::prepare(&path).map(Box::new);
            DecodeDone::Billboard { gen, slot, result }
        }
        DecodeJob::BillboardSheet { gen, slot, sheet, manifest } => {
            let result = prepare_billboard_sheet(&sheet, &manifest).map(Box::new);
            DecodeDone::Billboard { gen, slot, result }
        }
        DecodeJob::Thumb { .. } => {
            unreachable!("Thumb jobs are routed to the thumb lane by DecodePool::submit")
        }
    }
}

/// Two decode lanes so a grid full of thumbnails never queues behind a
/// heavier job:
///
/// - the HEAVY lane (Deck/Pad audio, MeshPrep, SlotMesh, Still, Billboard,
///   BillboardSheet) is a plain FIFO worker pool sized by `lane_sizes` —
///   these are the jobs that take real wall-clock time, and a mesh or track
///   decode that is already wanted must never be starved by ordering games.
/// - the THUMB lane is a small, dedicated pool (also sized by `lane_sizes`)
///   that only ever decodes `DecodeJob::Thumb`. Its pending jobs live on a
///   bounded LIFO stack (`ThumbQueue`), not a queue: the tile under the
///   operator's eye right now decodes before ones they scrolled past a
///   moment ago, and a job whose `epoch` has been superseded by a newer one
///   is skipped — never decoded — instead of wasting a worker on a tile
///   that has already scrolled away. See `ThumbQueue` and
///   `MAX_PENDING_THUMBS` for the exact rules.
///
/// Memory: a thumb decodes to at most `MAX_THUMB_DIM² × 4` bytes of BGRA
/// (2048² × 4 = 16 MiB) before the UI thread turns it into a texture and
/// drops the CPU buffer, so the thumb lane's peak resident memory is
/// `thumb_workers × 16 MiB` — bounded at 64 MiB even at the lane's cap of
/// 4 workers.
pub struct DecodePool {
    heavy_tx: Sender<DecodeJob>,
    thumb_queue: Arc<ThumbQueue>,
    rx: Receiver<DecodeDone>,
}

impl Default for DecodePool {
    fn default() -> Self {
        Self::new()
    }
}

impl DecodePool {
    pub fn new() -> DecodePool {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let (heavy_workers, thumb_workers) = lane_sizes(cpus);

        let (heavy_tx, job_rx) = channel::<DecodeJob>();
        let (done_tx, rx) = channel::<DecodeDone>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for i in 0..heavy_workers {
            let jobs = job_rx.clone();
            let done = done_tx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("vj-decode-heavy-{i}"))
                .spawn(move || loop {
                    let job = {
                        let guard = jobs.lock().unwrap();
                        guard.recv()
                    };
                    let Ok(job) = job else { return };
                    let out = run_heavy_job(job);
                    if done.send(out).is_err() {
                        return;
                    }
                });
        }

        let thumb_queue = Arc::new(ThumbQueue::new());
        for i in 0..thumb_workers {
            let queue = thumb_queue.clone();
            let done = done_tx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("vj-decode-thumb-{i}"))
                .spawn(move || loop {
                    let Some(job) = queue.pop() else { return };
                    let result = decode_thumb(&job.path, job.sheet, job.legacy_may_be_sheet);
                    let out = DecodeDone::Thumb { revision: job.revision, result };
                    if done.send(out).is_err() {
                        return;
                    }
                });
        }

        DecodePool { heavy_tx, thumb_queue, rx }
    }

    pub fn submit(&self, job: DecodeJob) {
        match job {
            DecodeJob::Thumb { revision, path, sheet, legacy_may_be_sheet, epoch } => {
                self.thumb_queue
                    .push(PendingThumb { revision, path, sheet, legacy_may_be_sheet, epoch });
            }
            other => {
                let _ = self.heavy_tx.send(other);
            }
        }
    }

    pub fn poll(&self) -> Vec<DecodeDone> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(done) => out.push(done),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for DecodePool {
    fn drop(&mut self) {
        // Wake any thumb worker blocked on the condvar so it observes
        // `closed` and exits instead of leaking. The heavy lane needs no
        // equivalent nudge: dropping `heavy_tx` (a struct field, dropped
        // right after this fn returns) already unblocks a blocked
        // `Receiver::recv()` per std::sync::mpsc's own disconnect signal.
        self.thumb_queue.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        let ticket = DECODER_ALIAS_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "makepad-vj-{label}-{}-{ticket}",
            std::process::id()
        ))
    }

    /// TIER-3 seek-bounce, on a REAL encoded GOP clip: the reverse leg must
    /// hand out every frame in exact reverse order (identity read from the
    /// picture, not the pts) and the synthesized pts must never go
    /// backwards. macOS-only: the platform file codec this exercises runs
    /// here; Windows takes the same facade.
    #[cfg(target_os = "macos")]
    #[test]
    fn seek_bounce_reverses_a_gop_clip_frame_exact() {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        const W: u32 = 320;
        const H: u32 = 192;
        const FPS: u32 = 24;
        const FRAMES: usize = 60;
        const BITS: usize = 6;
        const BLOCK_W: usize = W as usize / BITS;
        const BLOCK_H: usize = 24;
        // Frame index painted as a bit strip (the file_seek.rs trick): it
        // survives the codec round trip as exact bits, not a luma level.
        fn frame_rgb8(index: usize) -> Vec<u8> {
            let bar = (index * 5) % W as usize;
            let mut out = vec![0u8; W as usize * H as usize * 3];
            for y in 0..H as usize {
                for x in 0..W as usize {
                    let luma = if y < BLOCK_H {
                        let bit = (x / BLOCK_W).min(BITS - 1);
                        if index >> bit & 1 == 1 { 235 } else { 16 }
                    } else if y >= H as usize / 2 && x.abs_diff(bar) < 20 {
                        220
                    } else {
                        90
                    };
                    let at = (y * W as usize + x) * 3;
                    out[at] = luma;
                    out[at + 1] = luma;
                    out[at + 2] = luma;
                }
            }
            out
        }
        fn identity_of(bgra: &[u32]) -> usize {
            let mut index = 0;
            for bit in 0..BITS {
                let x0 = bit * BLOCK_W + BLOCK_W / 4;
                let x1 = bit * BLOCK_W + BLOCK_W * 3 / 4;
                let mut sum = 0u32;
                let mut count = 0u32;
                for y in BLOCK_H / 4..BLOCK_H * 3 / 4 {
                    for x in x0..x1 {
                        sum += (bgra[y * W as usize + x] >> 16) & 0xff;
                        count += 1;
                    }
                }
                if sum / count.max(1) > 128 {
                    index |= 1 << bit;
                }
            }
            index
        }

        let dir = test_dir("seek-bounce");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gop.mp4");
        let path_str = path.to_str().unwrap();
        let mut encoder = VideoFileEncoder::new(
            path_str,
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: W,
                height: H,
                fps_num: FPS,
                fps_den: 1,
                video_bitrate_bps: 8_000_000,
                audio: None,
                keyframe_only: false, // a REAL GOP clip: reverse must batch
            },
        )
        .expect("encoder");
        for index in 0..FRAMES {
            encoder.push_frame_rgb8(&frame_rgb8(index), None).expect("push");
        }
        encoder.finish().expect("finish");

        // Put the decoder where decode_loop enters tier 3: at end of stream.
        let mut decoder = VideoFileDecoder::open(path_str).expect("open");
        let info = decoder.info().clone();
        assert!(info.duration_100ns > 0);
        while decoder.next_frame().expect("forward decode").is_some() {}

        let shared = Arc::new(SlotShared {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            mode: AtomicU8::new(PlayMode::PingPong as u8),
            muted: AtomicBool::new(true),
            scrub: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
            trim_in_100ns: AtomicI64::new(0),
            trim_out_100ns: AtomicI64::new(i64::MAX),
            beat_transport: AtomicBool::new(false),
            beat_pulse: AtomicU64::new(0),
            beats_per_sweep: AtomicU8::new(4),
            beat_hint_100ns: AtomicI64::new(0),
            scratch_active: AtomicBool::new(false),
            scratch_rate_bits: AtomicU64::new(0f64.to_bits()),
            pace_tail_100ns: AtomicI64::new(0),
            trim_epoch: AtomicU64::new(0),
            position_100ns: AtomicI64::new(info.duration_100ns),
            video_ready: AtomicBool::new(false),
            preroll_status: AtomicU8::new(PrerollStatus::Ready as u8),
            playback_rate_bits: AtomicU64::new(1.0f64.to_bits()),
            end_of_stream: AtomicBool::new(false),
            frames: Mutex::new(VecDeque::new()),
            failure: Mutex::new(None),
        });
        let worker_shared = shared.clone();
        let worker = std::thread::spawn(move || {
            seek_bounce_playback(&mut decoder, &worker_shared, &info)
        });

        // Consume one full reverse leg plus a taste of the forward leg.
        let want = FRAMES + 10;
        let mut seen: Vec<(i64, usize)> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        while seen.len() < want {
            assert!(Instant::now() < deadline, "seek bounce starved: {seen:?}");
            let frame = shared.frames.lock().unwrap().pop_front();
            match frame {
                Some(frame) => seen.push((frame.pts_100ns, identity_of(&frame.bgra))),
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        shared.mode.store(PlayMode::Once as u8, Ordering::Release);
        let outcome = worker.join().expect("worker join");
        assert_eq!(outcome.ok(), Some(true), "bounce must exit on the mode change");

        // pts never go backwards — the pacer's contract.
        for pair in seen.windows(2) {
            assert!(pair[1].0 > pair[0].0, "pts reversed: {pair:?}");
        }
        // The reverse leg: every frame, newest to oldest, frame-exact.
        let reverse: Vec<usize> = seen[..FRAMES].iter().map(|s| s.1).collect();
        let expect: Vec<usize> = (0..FRAMES).rev().collect();
        assert_eq!(reverse, expect, "reverse leg must be frame-exact");
        // Then the forward leg starts over from the head of the clip.
        let forward: Vec<usize> = seen[FRAMES..].iter().map(|s| s.1).collect();
        let expect: Vec<usize> = (0..forward.len()).collect();
        assert_eq!(forward, expect, "forward leg must restart in order");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typed_decoder_input_hard_links_and_cleans_up() {
        let root = test_dir("decoder-link");
        let object_dir = root.join("objects/ab");
        std::fs::create_dir_all(&object_dir).unwrap();
        let source = object_dir.join(
            "ab00000000000000000000000000000000000000000000000000000000000000",
        );
        std::fs::write(&source, b"mp4 bytes stand in").unwrap();

        let input = DecoderInput::prepare(&source, MediaType::Mp4).unwrap();
        let alias = PathBuf::from(&input.path);
        assert_eq!(alias.extension().and_then(|e| e.to_str()), Some("mp4"));
        assert_eq!(alias.parent(), Some(root.join("decoder-input").as_path()));
        assert_eq!(std::fs::read(&alias).unwrap(), std::fs::read(&source).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(
                std::fs::metadata(&alias).unwrap().ino(),
                std::fs::metadata(&source).unwrap().ino(),
                "decoder input must not copy a large media blob"
            );
        }
        drop(input);
        assert!(!alias.exists(), "the decode-thread lease removes its hard link");
        let _ = std::fs::remove_dir_all(root);
    }

    fn pump_test_player(paused: bool, end_of_stream: bool, queued_frames: usize) -> SlotPlayer {
        let mut frames = VecDeque::new();
        for index in 0..queued_frames {
            frames.push_back(Frame {
                pts_100ns: index as i64,
                clip_100ns: index as i64,
                bgra: vec![0xff00_0000],
            });
        }
        let mixer = Mixer::new();
        mixer.open_slot(SlotId::A);
        SlotPlayer {
            width: 1,
            height: 1,
            duration_secs: 1.0,
            shared: Arc::new(SlotShared {
                stop: AtomicBool::new(false),
                paused: AtomicBool::new(paused),
                mode: AtomicU8::new(PlayMode::Once as u8),
                muted: AtomicBool::new(false),
                scrub: AtomicBool::new(false),
                seek_100ns: AtomicI64::new(-1),
            trim_in_100ns: AtomicI64::new(0),
            trim_out_100ns: AtomicI64::new(i64::MAX),
            beat_transport: AtomicBool::new(false),
            beat_pulse: AtomicU64::new(0),
            beats_per_sweep: AtomicU8::new(4),
            beat_hint_100ns: AtomicI64::new(0),
            scratch_active: AtomicBool::new(false),
            scratch_rate_bits: AtomicU64::new(0f64.to_bits()),
            pace_tail_100ns: AtomicI64::new(0),
            trim_epoch: AtomicU64::new(0),
                position_100ns: AtomicI64::new(0),
                video_ready: AtomicBool::new(false),
                preroll_status: AtomicU8::new(PrerollStatus::WaitingVideo as u8),
                playback_rate_bits: AtomicU64::new(1.0f64.to_bits()),
                end_of_stream: AtomicBool::new(end_of_stream),
                frames: Mutex::new(frames),
                failure: Mutex::new(None),
            }),
            clock_base: None,
            base_media_100ns: 0,
            last_pts: 0,
            slot: SlotId::A,
            mixer,
        }
    }

    #[test]
    fn frame_pump_sleeps_when_idle_but_drains_an_eos_ring() {
        assert!(pump_test_player(false, false, 0).needs_frame_pump());
        assert!(!pump_test_player(true, false, 1).needs_frame_pump());
        assert!(!pump_test_player(false, true, 0).needs_frame_pump());
        assert!(pump_test_player(false, true, 1).needs_frame_pump());
    }

    /// A FORWARD pts jump is a restart too: a transport handoff that
    /// lands far ahead must present NOW — the pacer used to wait the gap
    /// out at clock speed, which at a slow chip read as playback dead.
    #[test]
    fn forward_pts_jump_rebases_instead_of_stalling() {
        let mut player = pump_test_player(false, false, 0);
        player.last_pts = 1_000_000;
        player.base_media_100ns = 1_000_000;
        player.clock_base = Some(Instant::now());
        player.shared.frames.lock().unwrap().push_back(Frame {
            pts_100ns: 60_000_000, // six seconds ahead of the clock
            clip_100ns: 2_000_000,
            bgra: vec![0xff00_0000],
        });
        let got = player.take_due_frame();
        assert!(got.is_some(), "forward jump stalled the pacer");
        assert_eq!(
            player.shared.position_100ns.load(Ordering::Acquire),
            2_000_000
        );
    }

    /// THE RATE-CHIP LAW: dialing .5/1/2/4 touches the PLAYBACK RATE and
    /// nothing else — the trim bounds and the play position are the
    /// user's, and a speed change may never move either (the "dialing
    /// speeds made up a range" bug lived downstream of this promise).
    #[test]
    fn rate_chip_touches_neither_bounds_nor_position() {
        let mut player = pump_test_player(false, false, 0);
        player.set_trim(0.25, 0.75);
        let t_in = player.shared.trim_in_100ns.load(Ordering::Acquire);
        let t_out = player.shared.trim_out_100ns.load(Ordering::Acquire);
        player.shared.position_100ns.store(4_200_000, Ordering::Release);
        for chip in [0.5, 1.0, 2.0, 4.0, 1.0, 0.5] {
            player.set_playback_rate(chip);
            assert_eq!(player.playback_rate(), chip);
            assert_eq!(
                player.shared.trim_in_100ns.load(Ordering::Acquire),
                t_in,
                "rate change moved trim IN"
            );
            assert_eq!(
                player.shared.trim_out_100ns.load(Ordering::Acquire),
                t_out,
                "rate change moved trim OUT"
            );
            assert_eq!(
                player.shared.position_100ns.load(Ordering::Acquire),
                4_200_000,
                "rate change moved the play position"
            );
        }
    }

    #[test]
    fn preroll_requires_video_and_bounded_audio_lead() {
        assert_eq!(
            preroll_status(false, true, PREROLL_AUDIO_LEAD_SECS, false, false),
            PrerollStatus::WaitingVideo
        );
        assert_eq!(
            preroll_status(true, true, 0.0, false, false),
            PrerollStatus::WaitingAudio
        );
        assert_eq!(
            preroll_status(true, true, PREROLL_AUDIO_LEAD_SECS, false, false),
            PrerollStatus::Ready
        );
        assert_eq!(
            preroll_status(true, false, 0.0, false, false),
            PrerollStatus::Ready
        );
        assert_eq!(
            preroll_status(true, true, 0.05, true, false),
            PrerollStatus::ReadyAudioExhausted
        );
        assert_eq!(
            preroll_status(true, true, 0.0, false, true),
            PrerollStatus::ReadyAudioTimeout
        );
    }

    #[test]
    fn slot_player_rate_is_capped_and_updates_only_its_video_bus() {
        let mut player = pump_test_player(true, false, 1);
        player.mixer.install_deck(
            DeckId::A,
            Arc::new(TrackPcm { frames: vec![[1, 1]; 100], sample_rate: 48_000 }),
        );
        let deck_before = player.mixer.deck_position(DeckId::A);
        assert_eq!(player.set_playback_rate(99.0), MAX_VIDEO_PLAYBACK_RATE);
        assert_eq!(player.playback_rate(), MAX_VIDEO_PLAYBACK_RATE);
        assert_eq!(
            player.mixer.slot_playback_rate(SlotId::A),
            MAX_VIDEO_PLAYBACK_RATE
        );
        assert_eq!(player.mixer.deck_position(DeckId::A), deck_before);
        assert_eq!(player.set_playback_rate(0.0), MIN_VIDEO_PLAYBACK_RATE);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extensionless_cached_mp4_opens_through_typed_decoder_input() {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };

        let root = test_dir("decoder-mp4");
        let object_dir = root.join("objects/cd");
        std::fs::create_dir_all(&object_dir).unwrap();
        let encoded = root.join("encoded.mp4");
        let (width, height) = (64u32, 64u32);
        let mut encoder = VideoFileEncoder::new(
            encoded.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width,
                height,
                fps_num: 24,
                fps_den: 1,
                video_bitrate_bps: 1_000_000,
                audio: None,
                ..Default::default()
            },
        )
        .expect("video encoder");
        let rgb = vec![180u8; (width * height * 3) as usize];
        for _ in 0..6 {
            encoder.push_frame_rgb8(&rgb, None).expect("encode frame");
        }
        encoder.finish().expect("finish mp4");
        let source = object_dir.join(
            "cd00000000000000000000000000000000000000000000000000000000000000",
        );
        std::fs::rename(encoded, &source).unwrap();

        let input = DecoderInput::prepare(&source, MediaType::Mp4).unwrap();
        let mut decoder = VideoFileDecoder::open(&input.path).expect("typed MP4 path opens");
        assert_eq!((decoder.info().width, decoder.info().height), (width, height));
        assert!(decoder.next_frame().unwrap().is_some());
        drop(decoder);
        drop(input);
        let _ = std::fs::remove_dir_all(root);
    }

    fn wav_pcm16(frames: &[(i16, i16)], rate: u32) -> Vec<u8> {
        let mut data = Vec::new();
        for (l, r) in frames {
            data.extend_from_slice(&l.to_le_bytes());
            data.extend_from_slice(&r.to_le_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // pcm
        out.extend_from_slice(&2u16.to_le_bytes()); // stereo
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn wav_parse_roundtrip_budget_and_refusals() {
        let frames: Vec<(i16, i16)> = (0..100).map(|i| (i * 100, -i * 100)).collect();
        let bytes = wav_pcm16(&frames, 24_000);
        let pcm = parse_wav(&bytes, 1000).unwrap();
        assert_eq!(pcm.sample_rate, 24_000);
        assert_eq!(pcm.frames.len(), 100);
        assert_eq!(pcm.frames[3], [300, -300]);
        assert!((pcm.seconds() - 100.0 / 24_000.0).abs() < 1e-9);
        // Budget refusal.
        assert!(parse_wav(&bytes, 50).is_err());
        // Garbage refusal.
        assert!(parse_wav(b"garbage", 1000).is_err());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(parse_wav(&bad, 1000).is_err());
    }

    #[test]
    fn wave_peaks_and_strip_render_are_bounded() {
        let frames: Vec<[i16; 2]> =
            (0..1000).map(|i| if i < 500 { [16384, 16384] } else { [-8192, -8192] }).collect();
        let pcm = TrackPcm { frames, sample_rate: 48_000 };
        let peaks = wave_peaks(&pcm, 10);
        assert_eq!(peaks.len(), 10);
        assert!(peaks[0].1 > 0.4 && peaks[0].0 >= 0.0);
        assert!(peaks[9].0 < -0.2 && peaks[9].1 <= 0.0);
        // Render never panics at odd sizes and marks the playhead.
        let bgra = waveform_bgra(&peaks, 63, 17, 0.5);
        assert_eq!(bgra.len(), 63 * 17);
        // A clip shorter than the strip is wide still renders.
        let tiny = TrackPcm { frames: vec![[1000, 1000]; 3], sample_rate: 8000 };
        let bgra = waveform_bgra(&wave_peaks(&tiny, 8), 32, 8, 0.0);
        assert_eq!(bgra.len(), 32 * 8);
    }

    #[test]
    fn mesh_gate_refuses_hostile_dimensions() {
        assert!(mesh_gate(1_000, 28, 400_000, 3).is_ok());
        assert!(mesh_gate(MAX_MESH_BYTES + 1, 1, 1, 1).is_err());
        assert!(mesh_gate(1_000, MAX_MESH_JOINTS + 1, 1, 1).is_err());
        assert!(mesh_gate(1_000, 1, MAX_MESH_VERTICES + 1, 1).is_err());
        assert!(mesh_gate(1_000, 1, 1, MAX_MESH_CLIPS + 1).is_err());
    }

    #[test]
    fn thumb_decode_refuses_oversized_and_malformed() {
        let dir = std::env::temp_dir().join(format!("vj_thumb_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Malformed bytes refuse with a typed error.
        let bad = dir.join("bad.png");
        std::fs::write(&bad, b"not an image at all").unwrap();
        assert!(decode_thumb(&bad, None, true).is_err());
        // Over the byte budget refuses BEFORE decode.
        let huge = dir.join("huge.png");
        std::fs::write(&huge, vec![0u8; (MAX_THUMB_BYTES + 1) as usize]).unwrap();
        let err = decode_thumb(&huge, None, true).unwrap_err();
        assert!(err.contains("byte budget"), "{err}");
        // Mesh prep on garbage refuses too (worker-side, never the UI).
        let junk = dir.join("junk.glb");
        std::fs::write(&junk, b"gLTF-not-really").unwrap();
        assert!(prepare_mesh(&junk, None).is_err());
    }

    #[test]
    fn decode_pool_decodes_wav_and_reports_errors() {
        let dir = std::env::temp_dir().join(format!("vj_media_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.wav");
        std::fs::write(&good, wav_pcm16(&[(1000, -1000); 50], 22_050)).unwrap();
        let bad = dir.join("bad.wav");
        std::fs::write(&bad, b"not a wav").unwrap();

        let pool = DecodePool::new();
        pool.submit(DecodeJob::Deck { deck: DeckId::A, gen: 7, path: good, media: MediaType::Wav });
        pool.submit(DecodeJob::Pad {
            pad: PadKey::from_bytes([2; 16]),
            gen: 9,
            revision: AssetRevisionId::from_bytes([3; 32]),
            path: bad,
            media: MediaType::Wav,
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut results = Vec::new();
        while results.len() < 2 && std::time::Instant::now() < deadline {
            results.extend(pool.poll());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(results.len(), 2);
        for done in results {
            match done {
                DecodeDone::Deck { deck, gen, result } => {
                    assert_eq!((deck, gen), (DeckId::A, 7));
                    let (pcm, peaks) = result.expect("good wav decodes");
                    assert_eq!(pcm.frames.len(), 50);
                    assert_eq!(peaks.len(), WAVE_COLS);
                }
                DecodeDone::Pad { gen, result, .. } => {
                    assert_eq!(gen, 9);
                    assert!(result.is_err(), "bad wav must fail");
                }
                DecodeDone::MeshPrep { .. }
                | DecodeDone::SlotMesh { .. }
                | DecodeDone::Still { .. }
                | DecodeDone::Billboard { .. }
                | DecodeDone::FlowClip { .. }
                | DecodeDone::Thumb { .. } => {
                    panic!("no mesh/flow/thumb job submitted")
                }
            }
        }
    }

    #[test]
    fn lane_sizes_scale_heavy_and_cap_thumb() {
        // 1 cpu: both lanes floor at their minimum (2 workers each).
        assert_eq!(lane_sizes(1), (2, 2));
        // 4 cpus: heavy tracks the core count; thumb stays at its floor.
        assert_eq!(lane_sizes(4), (4, 2));
        // 32 cpus: heavy caps at 8; thumb caps at 4.
        assert_eq!(lane_sizes(32), (8, 4));
    }

    #[test]
    fn thumb_queue_is_lifo_and_prunes_stale_and_bounds_pending() {
        // LIFO: with every job at the same epoch (none stale), the queue
        // must hand back the most recently pushed job first.
        let queue = ThumbQueue::new();
        for i in 0..10u32 {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                path: PathBuf::from(format!("t{i}.png")),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 0,
            });
        }
        for expect in (0..10u32).rev() {
            let job = queue.pop().expect("job available");
            assert_eq!(job.path, PathBuf::from(format!("t{expect}.png")), "must be newest-first");
        }

        // Staleness: jobs stamped with an epoch older than the newest one
        // this queue has seen are skipped (dropped, not decoded).
        let queue = ThumbQueue::new();
        for i in 0..5u32 {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                path: PathBuf::from(format!("old{i}.png")),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 1,
            });
        }
        queue.push(PendingThumb {
            revision: AssetRevisionId::from_bytes([9; 32]),
            path: PathBuf::from("fresh.png"),
            sheet: None,
            legacy_may_be_sheet: false,
            epoch: 2,
        });
        let job = queue.pop().expect("the fresh-epoch job survives");
        assert_eq!(job.path, PathBuf::from("fresh.png"));
        assert!(
            queue.state.lock().unwrap().stack.is_empty(),
            "stale jobs must be dropped when popped, not left behind"
        );

        // Cap: pushing past MAX_PENDING_THUMBS drops the OLDEST pending job.
        let queue = ThumbQueue::new();
        for i in 0..(MAX_PENDING_THUMBS + 3) {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([0; 32]),
                path: PathBuf::from(format!("p{i}.png")),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 0,
            });
        }
        let remaining = queue.state.lock().unwrap();
        assert_eq!(remaining.stack.len(), MAX_PENDING_THUMBS);
        assert_eq!(
            remaining.stack.front().unwrap().path,
            PathBuf::from("p3.png"),
            "the three oldest (p0..p2) must have been dropped to stay at the cap"
        );
    }

    #[test]
    fn thumb_lane_is_not_blocked_by_a_slow_heavy_job() {
        let pool = DecodePool::new();
        // A heavy job that sleeps before failing (nonexistent glb) -- long
        // enough to prove the thumb lane doesn't queue behind it.
        pool.submit(DecodeJob::MeshPrep {
            gen: 1,
            path: PathBuf::from("vj_test_sleep_600.glb"),
        });

        let dir = std::env::temp_dir().join(format!("vj_thumb_lane_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Malformed thumbs decode-fail fast, but that still proves the
        // thumb lane drained them without waiting on the mesh job.
        for i in 0..8u32 {
            let path = dir.join(format!("bad{i}.png"));
            std::fs::write(&path, b"not an image").unwrap();
            pool.submit(DecodeJob::Thumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                path,
                sheet: None,
                legacy_may_be_sheet: true,
                epoch: 0,
            });
        }

        let deadline = Instant::now() + Duration::from_millis(400);
        let mut thumbs = 0;
        let mut mesh_done = false;
        while Instant::now() < deadline {
            for done in pool.poll() {
                match done {
                    DecodeDone::Thumb { .. } => thumbs += 1,
                    DecodeDone::MeshPrep { .. } => mesh_done = true,
                    _ => {}
                }
            }
            if thumbs == 8 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(thumbs, 8, "all thumbs must finish while the mesh job is still sleeping");
        assert!(!mesh_done, "the slow mesh job (600ms) must not have finished within 400ms");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn shutdown_drains_pending_jobs_without_hang_or_panic() {
        let pool = DecodePool::new();
        let dir = std::env::temp_dir().join(format!("vj_shutdown_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..20u32 {
            let path = dir.join(format!("t{i}.png"));
            std::fs::write(&path, b"not an image").unwrap();
            pool.submit(DecodeJob::Thumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                path,
                sheet: None,
                legacy_may_be_sheet: true,
                epoch: 0,
            });
        }
        pool.submit(DecodeJob::MeshPrep {
            gen: 1,
            path: PathBuf::from("vj_test_sleep_50.glb"),
        });
        // Dropping mid-flight (workers still busy/blocked) must not panic
        // or hang: the heavy lane unblocks via mpsc's own sender-drop
        // disconnect, the thumb lane via ThumbQueue::close()'s notify_all.
        drop(pool);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    /// A DECLARED layout is cut exactly as declared: the producer's frame
    /// count, the producer's cell size, the producer's origin. None of it is
    /// measured, so a picture the legacy guess would refuse (or chop wrong)
    /// comes out right.
    #[test]
    fn a_declared_layout_is_cut_exactly() {
        // 4x2 grid of 3x3 cells, each filled with its own value.
        let (cw, ch, cols, rows) = (3usize, 3usize, 4usize, 2usize);
        let (w, h) = (cols * cw, rows * ch);
        let mut data = vec![0u32; w * h];
        for i in 0..cols * rows {
            let (ox, oy) = ((i % cols) * cw, (i / cols) * ch);
            for y in 0..ch {
                for x in 0..cw {
                    data[(oy + y) * w + ox + x] = i as u32 + 1;
                }
            }
        }
        let cells = ThumbnailCells {
            cols: cols as u32,
            cell_w: cw as u32,
            cell_h: ch as u32,
            first: 1,
            count: 3,
        };
        let frames = cut_declared_cells(w, h, &data, cells);
        assert_eq!(frames.len(), 3, "three frames, from cell one");
        assert!(frames[0].iter().all(|p| *p == 2));
        assert!(frames[2].iter().all(|p| *p == 4));
        // These dimensions are nothing like a 128-tile sheet: the legacy
        // guess refuses them, which is exactly why the declaration matters.
        assert!(legacy_split_sheet_bgra(w, h, &data).is_none());
        // A range past the edge stops instead of reading past the picture.
        let over = ThumbnailCells { first: 6, count: 8, ..cells };
        assert_eq!(cut_declared_cells(w, h, &data, over).len(), 2);
    }

    /// The published shape of a SINGLE-FRAME sprite actor (`bpak`, `clip`,
    /// `cand`, every Doom pickup): one painted 128² tile at the top-left of
    /// a 1024x256 strip, the other fifteen cells clear padding bought to
    /// clear the 256px published-thumbnail floor.
    fn one_cell_strip() -> (usize, usize, Vec<u32>) {
        let (w, h) = (1024usize, 256usize);
        let mut data = vec![SHEET_CLEAR; w * h];
        for y in 0..SHEET_TILE {
            for x in 0..SHEET_TILE {
                data[y * w + x] = 0xFF44AA66;
            }
        }
        (w, h, data)
    }

    #[test]
    /// A ONE-cell declaration is a still OF THAT CELL. This is the tiny
    /// top-left sprite bug: the strip declares `cells 8 128 128 0 1`, and
    /// drawing the whole 1024x256 picture left a 128px sprite in the corner
    /// of a tile eight times as wide as it.
    fn a_single_declared_cell_is_a_still_of_that_cell() {
        let (w, h, data) = one_cell_strip();
        let cells = ThumbnailCells { cols: 8, cell_w: 128, cell_h: 128, first: 0, count: 1 };
        let cut = declared_thumb(w, h, &data, cells, 8.0).expect("one cell is a usable cut");
        assert_eq!((cut.width, cut.height), (128, 128), "the CELL, not the strip");
        assert_eq!(cut.bgra.len(), 128 * 128);
        assert!(cut.bgra.iter().all(|p| *p == 0xFF44AA66), "the painted tile");
        assert!(cut.frames.is_empty(), "one cell is a still, not an animation");
        assert_eq!(cut.fps, 0.0);
    }

    #[test]
    /// The same decision for the other counts: two or more cells cycle at
    /// the declared rate, and a declaration that fits nothing inside the
    /// picture buys nothing (the caller draws the whole image).
    fn declared_cells_cycle_and_a_stale_declaration_declines() {
        let (w, h, data) = one_cell_strip();
        let cells = ThumbnailCells { cols: 8, cell_w: 128, cell_h: 128, first: 0, count: 3 };
        let cut = declared_thumb(w, h, &data, cells, 6.0).expect("three cells");
        assert_eq!(cut.frames.len(), 3);
        assert_eq!(cut.fps, 6.0);
        assert!(cut.frames.iter().all(|(px, fw, fh)| (*fw, *fh) == (128, 128)
            && px.len() == 128 * 128));
        // The still frame a grid without an animation clock shows is the
        // FIRST declared cell, at cell size.
        assert_eq!((cut.width, cut.height), (128, 128));
        assert!(cut.bgra.iter().all(|p| *p == 0xFF44AA66));

        // A stamp from a bigger picture than the one that arrived: every
        // cell lies outside, so nothing is cut and nothing is guessed.
        let stale = ThumbnailCells { cols: 8, cell_w: 512, cell_h: 512, first: 0, count: 4 };
        assert!(declared_thumb(w, h, &data, stale, 8.0).is_none());
    }

    #[test]
    /// A thumbnail bigger than a tile can draw is shrunk BEFORE it reaches
    /// the UI thread, by whole-integer box averaging: a 1024² still becomes
    /// 512² (a quarter of the upload), while every sprite cell and 128²
    /// strip tile is already small and passes through untouched.
    fn oversized_thumbnails_shrink_and_small_ones_are_left_alone() {
        assert_eq!(shrink_factor(128, 128), 1);
        assert_eq!(shrink_factor(512, 512), 1);
        assert_eq!(shrink_factor(1024, 1024), 2);
        assert_eq!(shrink_factor(2048, 256), 4);
        assert_eq!(shrink_factor(1024, 256), 2, "the long axis decides");

        // Two shades in a checker: every 2x2 block averages to their mean,
        // so a correct box filter lands exactly halfway.
        let (w, h) = (8usize, 4usize);
        let mut src = vec![0u32; w * h];
        for y in 0..h {
            for x in 0..w {
                src[y * w + x] = if (x + y) % 2 == 0 { 0xFF00_0000 } else { 0xFF40_4040 };
            }
        }
        let (out, ow, oh) = box_shrink(&src, w, h, 2);
        assert_eq!((ow, oh), (4, 2));
        assert!(out.iter().all(|p| *p == 0xFF20_2020), "block average: {:08x}", out[0]);

        // Fully transparent stays transparent (and never divides by zero).
        let clear = vec![0u32; w * h];
        let (out, _, _) = box_shrink(&clear, w, h, 2);
        assert!(out.iter().all(|p| *p == 0));

        // A keyed sprite's transparent pixels must not darken what is left:
        // one opaque red among three clear ones is still red, at a quarter
        // of the coverage.
        let mut keyed = vec![0u32; 2 * 2];
        keyed[0] = 0xFFFF_0000;
        let (out, _, _) = box_shrink(&keyed, 2, 2, 2);
        assert_eq!(out[0] & 0x00FF_FFFF, 0x00FF_0000, "colour survives the alpha average");
        assert_eq!((out[0] >> 24) & 0xff, 63, "coverage is a quarter");
    }

    #[test]
    /// The shrink runs over the whole `ThumbPixels`, frames included, and a
    /// declared cell layout (already tile-sized) comes through unchanged.
    fn fitting_a_thumb_shrinks_the_still_and_every_frame() {
        let mut big = ThumbPixels {
            bgra: vec![0xFF10_2030; 1024 * 1024],
            width: 1024,
            height: 1024,
            frames: vec![(vec![0xFF10_2030; 1024 * 512], 1024, 512)],
            fps: 8.0,
        };
        fit_thumb_for_tiles(&mut big);
        assert_eq!((big.width, big.height), (512, 512));
        assert_eq!(big.bgra.len(), 512 * 512);
        assert_eq!((big.frames[0].1, big.frames[0].2), (512, 256));
        assert!(big.bgra.iter().all(|p| *p == 0xFF10_2030), "a flat picture stays flat");

        let cells = ThumbPixels {
            bgra: vec![0xFF44_AA66; 128 * 128],
            width: 128,
            height: 128,
            frames: vec![(vec![0xFF44_AA66; 128 * 128], 128, 128)],
            fps: 8.0,
        };
        let mut same = ThumbPixels {
            bgra: cells.bgra.clone(),
            width: 128,
            height: 128,
            frames: cells.frames.clone(),
            fps: 8.0,
        };
        fit_thumb_for_tiles(&mut same);
        assert_eq!((same.width, same.height), (128, 128));
        assert_eq!(same.frames[0].1, 128);
    }

    #[test]
    fn only_128_packed_sheets_split_by_the_legacy_guess() {
        // A native sprite with lots of keyed/clear pixels must stay one frame.
        let mut sprite = vec![0u32; 64 * 128];
        for y in 20..100 {
            for x in 16..48 {
                sprite[y * 64 + x] = 0xFF2244AAu32;
            }
        }
        assert!(legacy_split_sheet_bgra(64, 128, &sprite).is_none());

        // A 256² photograph-like thumb is not a sheet even if it divides by 128.
        assert!(legacy_split_sheet_bgra(256, 256, &vec![0xFF808080u32; 256 * 256]).is_none());

        // 1024×128 packed sheet: two painted tiles, the rest studio-clear.
        let mut sheet = vec![SHEET_CLEAR; SHEET_W * SHEET_TILE];
        for tile in 0..2 {
            for y in 0..SHEET_TILE {
                for x in 0..SHEET_TILE {
                    sheet[y * SHEET_W + tile * SHEET_TILE + x] = 0xFF3366CCu32;
                }
            }
        }
        let frames = legacy_split_sheet_bgra(SHEET_W, SHEET_TILE, &sheet).expect("two painted tiles");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].len(), SHEET_TILE * SHEET_TILE);
    }

    fn library_billboard(name: &str) -> Option<PathBuf> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/ai_content_library")
            .join(name);
        path.exists().then_some(path)
    }

    #[test]
    fn billboard_thumb_keeps_native_sizes_and_front_facing() {
        let Some(path) = library_billboard("lib-17232.billboard") else {
            return;
        };
        let thumb = decode_billboard_thumb(&path).expect("forceripple decodes");
        assert!(thumb.frames.len() > 1, "idle cycle should play");
        let sizes: Vec<(usize, usize)> = thumb
            .frames
            .iter()
            .map(|(_, w, h)| (*w, *h))
            .collect();
        assert!(
            sizes.windows(2).any(|pair| pair[0] != pair[1]),
            "later frames must keep authored size, got {sizes:?}"
        );
        assert_eq!(sizes[0], (thumb.width, thumb.height));

        let Some(troop) = library_billboard("lib-17234.billboard") else {
            return;
        };
        let prepared = crate::billboard::prepare(&troop).expect("liztroop prepares");
        let walk = prepared
            .states
            .iter()
            .find(|s| s.name == "walk")
            .expect("walk state");
        assert!(
            walk.frames.len() < 10,
            "walk must be front-facing letters, not every rotation ({})",
            walk.frames.len()
        );
    }
}

#[cfg(test)]
mod mode_flip_tests {
    use super::*;

    /// THE FROZEN-LOOP LAW: an untrimmed clip's cache repeat must never
    /// think its range is uncovered (the old pts-based check compared trim
    /// 0 against a container's nonzero first pts and exited instantly —
    /// which busy-spun the EOS path and froze every looping video).
    #[test]
    fn untrimmed_and_shrunk_ranges_never_outgrow_the_cache() {
        let full = (0i64, i64::MAX);
        assert!(!cache_range_outgrown(full, full), "untrimmed never exits");
        // Shrinking stays in the cache (no playback reset).
        assert!(!cache_range_outgrown(full, (2_000_000, 8_000_000)));
        // Growing past the BUILD bounds hands control back for a re-pass.
        let built = (2_000_000i64, 8_000_000i64);
        assert!(cache_range_outgrown(built, (1_000_000, 8_000_000)));
        assert!(cache_range_outgrown(built, (2_000_000, 9_000_000)));
        assert!(!cache_range_outgrown(built, (3_000_000, 7_000_000)));
    }

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vj-media-{label}-{}", std::process::id()))
    }

    /// THE LOOP/BOUNCE LAW, end to end through the real SlotPlayer: a
    /// muted looping clip wraps to frame 0 at the end (a clean jump cut,
    /// never a tail oscillation), a mid-play flip to PING-PONG visibly
    /// REVERSES the frame sequence, and flipping back restores the wrap.
    #[test]
    fn loop_wraps_clean_and_a_midplay_bounce_flip_reverses() {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        const W: u32 = 128;
        const H: u32 = 64;
        const FPS: u32 = 24;
        const FRAMES: usize = 24;
        const BITS: usize = 5;
        const BLOCK_W: usize = W as usize / BITS;
        fn frame_rgb8(index: usize) -> Vec<u8> {
            let mut out = vec![40u8; W as usize * H as usize * 3];
            for y in 0..H as usize / 2 {
                for x in 0..W as usize {
                    let bit = (x / BLOCK_W).min(BITS - 1);
                    let luma = if index >> bit & 1 == 1 { 235 } else { 16 };
                    let at = (y * W as usize + x) * 3;
                    out[at] = luma;
                    out[at + 1] = luma;
                    out[at + 2] = luma;
                }
            }
            out
        }
        fn identity_of(bgra: &[u32]) -> usize {
            let mut index = 0;
            for bit in 0..BITS {
                let x0 = bit * BLOCK_W + BLOCK_W / 4;
                let x1 = bit * BLOCK_W + BLOCK_W * 3 / 4;
                let mut sum = 0u32;
                let mut count = 0u32;
                for y in 4..H as usize / 4 {
                    for x in x0..x1 {
                        sum += (bgra[y * W as usize + x] >> 16) & 0xff;
                        count += 1;
                    }
                }
                if sum / count.max(1) > 128 {
                    index |= 1 << bit;
                }
            }
            index
        }

        let dir = test_dir("mode-flip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("loop.mp4");
        let mut encoder = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: W,
                height: H,
                fps_num: FPS,
                fps_den: 1,
                video_bitrate_bps: 4_000_000,
                audio: None,
                keyframe_only: true,
            },
        )
        .expect("encoder");
        for index in 0..FRAMES {
            encoder.push_frame_rgb8(&frame_rgb8(index), None).expect("push");
        }
        encoder.finish().expect("finish");

        let mixer = Mixer::new();
        let mut player = SlotPlayer::open(
            SlotId::A,
            path.to_str().unwrap(),
            MediaType::Mp4,
            mixer,
            true,  // loop on
            false, // playing
        )
        .expect("open");
        player.set_muted(true);

        // Collect presented identities for ~3 clip lengths.
        let mut ids: Vec<usize> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && ids.len() < FRAMES * 3 {
            if let Some(bgra) = player.take_due_frame() {
                ids.push(identity_of(&bgra));
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert!(ids.len() >= FRAMES * 2, "loop starved: {} frames", ids.len());
        // Law 1: LOOP only ever steps forward or wraps to 0 — a backward
        // step that is not a wrap is the tail oscillation bug.
        let mut wraps = 0;
        for pair in ids.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b < a {
                assert_eq!(b, 0, "loop went BACKWARD {a}->{b} (tail oscillation)");
                wraps += 1;
            }
        }
        assert!(wraps >= 1, "never wrapped: {ids:?}");

        // Law 2: flip to PING-PONG mid-play — the sequence must REVERSE.
        player.set_mode(PlayMode::PingPong);
        let mut ids: Vec<usize> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && ids.len() < FRAMES * 3 {
            if let Some(bgra) = player.take_due_frame() {
                ids.push(identity_of(&bgra));
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        let mut descending_run = 0;
        let mut best_run = 0;
        for pair in ids.windows(2) {
            if pair[1] < pair[0] {
                descending_run += 1;
                best_run = best_run.max(descending_run);
            } else if pair[1] > pair[0] {
                descending_run = 0;
            }
        }
        assert!(
            best_run >= FRAMES / 3,
            "ping-pong never reversed (best descending run {best_run}): {ids:?}"
        );

        // Law 3: back to LOOP — forward-or-wrap again.
        player.set_mode(PlayMode::Loop);
        // Let the mode change propagate through a bounce leg.
        std::thread::sleep(Duration::from_millis(400));
        let mut ids: Vec<usize> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && ids.len() < FRAMES * 2 {
            if let Some(bgra) = player.take_due_frame() {
                ids.push(identity_of(&bgra));
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        // Skip the first leg (the in-flight bounce direction drains first).
        let tail = &ids[ids.len().min(FRAMES / 2)..];
        for pair in tail.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b < a && b != 0 {
                // One residual backward step right at the changeover is the
                // buffered bounce leg; a repeated pattern is the bug.
                assert!(
                    a - b <= 1,
                    "loop after flip still bouncing: {a}->{b} in {tail:?}"
                );
            }
        }

        // Law 4: IN/OUT TRIM — the scrub bar's range handles confine the
        // loop: every presented frame inside [in, out), wraps land on IN
        // (never zero).
        player.set_trim(0.25, 0.75);
        // Drain the in-flight ring (frames decoded under the old bounds).
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let _ = player.take_due_frame();
            std::thread::sleep(Duration::from_millis(4));
        }
        let mut ids: Vec<usize> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && ids.len() < FRAMES * 2 {
            if let Some(bgra) = player.take_due_frame() {
                ids.push(identity_of(&bgra));
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert!(ids.len() >= FRAMES, "trimmed loop starved: {}", ids.len());
        let lo = *ids.iter().min().unwrap();
        let hi = *ids.iter().max().unwrap();
        assert!(
            lo + 1 >= FRAMES / 4 && hi < FRAMES * 3 / 4,
            "trimmed loop escaped [{}..{}): frames {lo}..{hi} in {ids:?}",
            FRAMES / 4,
            FRAMES * 3 / 4
        );
        let mut wraps = 0;
        for pair in ids.windows(2) {
            if pair[1] < pair[0] {
                wraps += 1;
                assert!(
                    pair[1] <= FRAMES / 4 + 1,
                    "trimmed wrap landed at {} not IN: {ids:?}",
                    pair[1]
                );
            }
        }
        assert!(wraps >= 1, "trimmed loop never wrapped: {ids:?}");
    }
}

#[cfg(test)]
mod beat_transport_tests {
    use super::*;

    const D: i64 = 416_667; // one 24fps frame in 100ns units

    /// Run the sweep clock for `ticks`, returning (phase, forward, idx)
    /// after each tick.
    fn run(
        mode: PlayMode,
        window: (usize, usize),
        start: (f64, bool),
        step: f64,
        ticks: usize,
    ) -> Vec<(f64, bool, usize)> {
        let (lo, hi) = window;
        let (mut phase, mut fwd) = start;
        let mut out = Vec::with_capacity(ticks);
        for _ in 0..ticks {
            let (p, d) = advance_sweep(phase, fwd, step, mode);
            phase = p;
            fwd = d;
            out.push((phase, fwd, sweep_index(phase, fwd, lo, hi, mode)));
        }
        out
    }

    fn turn_ticks(trace: &[(f64, bool, usize)]) -> Vec<usize> {
        let mut turns = Vec::new();
        for (t, pair) in trace.windows(2).enumerate() {
            let wrapped = pair[1].0 < pair[0].0;
            if wrapped || pair[1].1 != pair[0].1 {
                turns.push(t + 1);
            }
        }
        turns
    }

    /// THE SWEEP LAW: one direction sweep = one beat step, at ANY range
    /// width — the tick count between turns depends only on the step
    /// (beat ÷ chip), never on the window.
    #[test]
    fn one_sweep_is_one_beat_step_at_any_range_width() {
        let step = D as f64 / 5_000_000.0; // 0.5s beat at 24fps -> 12 ticks
        for mode in [PlayMode::Loop, PlayMode::PingPong] {
            let narrow = turn_ticks(&run(mode, (10, 14), (0.0, true), step, 600));
            let wide = turn_ticks(&run(mode, (0, 400), (0.0, true), step, 600));
            assert_eq!(narrow.len(), wide.len(), "{mode:?}: range width changed the cadence");
            for (a, b) in narrow.iter().zip(wide.iter()) {
                assert_eq!(a, b, "{mode:?}: turns drifted between widths");
            }
            // And the cadence is the beat step: 12 ticks per sweep.
            for pair in narrow.windows(2) {
                assert_eq!(pair[1] - pair[0], 12, "{mode:?}: sweep != one beat step");
            }
        }
    }

    /// The overshoot is CARRIED at the turn — the wrap costs zero time,
    /// so the long-run cadence is exact even when the beat step is not a
    /// whole number of ticks (the accumulate-and-jump failure mode).
    #[test]
    fn fractional_beat_steps_keep_exact_long_run_cadence() {
        let step = 0.093; // 10.75 ticks per sweep — nothing divides
        let trace = run(PlayMode::PingPong, (0, 60), (0.0, true), step, 10_000);
        let turns = turn_ticks(&trace);
        let first = turns[0] as f64;
        let last = *turns.last().unwrap() as f64;
        let measured = (last - first) / (turns.len() - 1) as f64;
        let expect = 1.0 / step;
        assert!(
            (measured - expect).abs() < 0.02,
            "cadence drifted: measured {measured:.3} ticks/sweep, law says {expect:.3}"
        );
    }

    /// The chip is CADENCE: doubling the rate halves the ticks per sweep
    /// (2x sweeps in half a beat), the mapping itself untouched.
    #[test]
    fn chip_changes_cadence_only() {
        let beat = 5_000_000.0;
        for (rate, want) in [(0.5f64, 24usize), (1.0, 12), (2.0, 6), (4.0, 3)] {
            let step = D as f64 / (beat / rate);
            let turns = turn_ticks(&run(PlayMode::Loop, (0, 100), (0.0, true), step, 200));
            for pair in turns.windows(2) {
                assert_eq!(
                    pair[1] - pair[0],
                    want,
                    "chip {rate} should sweep in {want} ticks"
                );
            }
        }
    }

    /// A LIVE TRIM RESCALES, never teleports: the phase is the state, so
    /// the position remaps proportionally into the new window and the
    /// motion carries on.
    #[test]
    fn trim_rescales_the_sweep_without_teleport() {
        // Mid-sweep, phase 0.5: the position is the middle of ANY window.
        assert_eq!(sweep_index(0.5, true, 0, 101, PlayMode::Loop), 50);
        assert_eq!(sweep_index(0.5, true, 20, 41, PlayMode::Loop), 30);
        assert_eq!(sweep_index(0.5, true, 10, 12, PlayMode::Loop), 11);
        // The mirrored bounce leg remaps the same way.
        assert_eq!(sweep_index(0.25, false, 0, 101, PlayMode::PingPong), 75);
        assert_eq!(sweep_index(0.25, false, 20, 41, PlayMode::PingPong), 35);
        // Degenerate windows never escape or panic.
        assert_eq!(sweep_index(0.7, true, 5, 6, PlayMode::PingPong), 5);
        assert_eq!(sweep_index(1.0, true, 3, 9, PlayMode::Loop), 8);
    }

    /// Bounce alternates direction each beat step; wrap restarts forward
    /// — and the apex never dwells beyond the natural per-frame hold.
    #[test]
    fn bounce_alternates_and_never_pauses_at_the_apex() {
        let step = D as f64 / 5_000_000.0;
        let trace = run(PlayMode::PingPong, (0, 48), (0.0, true), step, 480);
        let dirs: Vec<bool> = turn_ticks(&trace)
            .iter()
            .map(|t| trace[*t].1)
            .collect();
        for pair in dirs.windows(2) {
            assert_ne!(pair[0], pair[1], "bounce failed to alternate");
        }
        // No pause: the index never repeats for longer than the natural
        // dwell (ticks-per-sweep / span, +1 for the apex rounding).
        let natural = (12.0f64 / 47.0).ceil() as usize + 1;
        let mut dwell = 1;
        let mut worst = 1;
        for pair in trace.windows(2) {
            if pair[1].2 == pair[0].2 {
                dwell += 1;
                worst = worst.max(dwell);
            } else {
                dwell = 1;
            }
        }
        assert!(
            worst <= natural,
            "the sweep dwelt {worst} ticks on one frame (natural dwell {natural}) — a pause"
        );
        // Loop mode: always forward.
        let wrap = run(PlayMode::Loop, (0, 48), (0.9, true), step, 480);
        assert!(wrap.iter().all(|(_, f, _)| *f), "a wrap-mode sweep ran backward");
    }

    /// The nudge is the beat lock's ONLY corrective authority: bounded to
    /// ±2% of a sweep per pulse, zero when aligned, and it converges an
    /// engage-offset onto the grid over a few beats — never a snap.
    #[test]
    fn nudge_is_bounded_zero_when_aligned_and_convergent() {
        for beats in [1.0f64, 2.0, 4.0, 8.0] {
            assert_eq!(beat_phase_nudge(0.0, beats), 0.0);
            for phase in [0.01f64, 0.13, 0.35, 0.49, 0.5, 0.77, 0.99] {
                let nudge = beat_phase_nudge(phase, beats);
                assert!(nudge.abs() <= 0.02 + 1e-12, "nudge {nudge} out of authority");
            }
        }
        // Convergence on a 1-beat sweep: an 8%-off engage walks onto the
        // grid over a few pulses.
        let mut phase = 0.08f64;
        for _ in 0..8 {
            phase += beat_phase_nudge(phase, 1.0);
        }
        assert!(phase.abs() < 1e-9, "phase failed to converge: {phase}");
        // A 4-beat sweep passes a beat at every quarter of its phase:
        // 0.25 IS the grid — no correction; 0.30 pulls back toward it.
        assert_eq!(beat_phase_nudge(0.25, 4.0), 0.0);
        assert!(beat_phase_nudge(0.30, 4.0) < 0.0);
    }

    /// THE LAW END TO END through decode → cache → pacer, INSTRUMENTED:
    /// with the transport on and pulses at beat cadence, a bounce turns
    /// continuously — no pause at the edges (max inter-presentation gap
    /// stays frame-scale, nowhere near beat-scale), the full range keeps
    /// getting swept, and direction reversals track the beat count.
    #[test]
    fn transport_sweeps_on_the_beat_without_pausing() {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        const W: u32 = 64;
        const H: u32 = 32;
        const FPS: u32 = 24;
        const FRAMES: usize = 12;
        const BEAT: Duration = Duration::from_millis(400);
        fn frame_rgb8(index: usize) -> Vec<u8> {
            vec![(index * 16 + 8) as u8; W as usize * H as usize * 3]
        }
        fn identity_of(bgra: &[u32]) -> usize {
            let mid = bgra[(H as usize / 2) * W as usize + W as usize / 2];
            (((mid >> 16) & 0xff) as usize) / 16
        }

        let dir = std::env::temp_dir()
            .join(format!("vj-media-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sweep.mp4");
        let mut encoder = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: W,
                height: H,
                fps_num: FPS,
                fps_den: 1,
                video_bitrate_bps: 2_000_000,
                audio: None,
                keyframe_only: true,
            },
        )
        .expect("encoder");
        for index in 0..FRAMES {
            encoder.push_frame_rgb8(&frame_rgb8(index), None).expect("push");
        }
        encoder.finish().expect("finish");

        let mixer = Mixer::new();
        let mut player = SlotPlayer::open(
            SlotId::A,
            path.to_str().unwrap(),
            MediaType::Mp4,
            mixer,
            true,
            false,
        )
        .expect("open");
        player.set_muted(true);
        player.set_mode(PlayMode::PingPong);
        // Chip 1 = a sweep per beat; the HINT seeds the grid so the law
        // paces from the first frame (no natural-rate first pass).
        player.set_beats_per_sweep(1);
        player.set_beat_hint((BEAT.as_secs_f64() * 1e7) as i64);
        player.set_beat_transport(true);

        // Present for ~14 beats, pulsing on the beat, recording identity
        // and wall instant of every presented frame.
        let mut seen: Vec<(Instant, usize)> = Vec::new();
        let start = Instant::now();
        let mut next_pulse = start + BEAT;
        let deadline = start + Duration::from_millis(5_600);
        while Instant::now() < deadline {
            let now = Instant::now();
            if now >= next_pulse {
                player.beat_pulse();
                next_pulse += BEAT;
            }
            if let Some(bgra) = player.take_due_frame() {
                seen.push((Instant::now(), identity_of(&bgra)));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(seen.len() > 60, "starved: {} frames", seen.len());

        // Skip the natural-speed first pass + the first learned beats;
        // judge the locked regime.
        let judge_from = start + Duration::from_millis(2_000);
        let locked: Vec<&(Instant, usize)> =
            seen.iter().filter(|(at, _)| *at >= judge_from).collect();
        assert!(locked.len() > 30, "no locked regime captured");

        // NO PAUSE: the largest gap between presented frames stays frame-
        // scale. A beat-scale gap is the rejected freeze. The printed
        // stats are the instrument (run with --nocapture).
        let mut gaps: Vec<Duration> = locked
            .windows(2)
            .map(|pair| pair[1].0.duration_since(pair[0].0))
            .collect();
        gaps.sort_unstable();
        let worst_gap = *gaps.last().unwrap();
        println!(
            "sweep-transport presentation gaps: n={} p50={:?} p95={:?} max={:?}",
            gaps.len(),
            gaps[gaps.len() / 2],
            gaps[gaps.len() * 95 / 100],
            worst_gap
        );
        assert!(
            worst_gap < Duration::from_millis(160),
            "presentation stalled {worst_gap:?} — a visible pause"
        );

        // The FULL range keeps being swept (no invented sub-range).
        let ids: Vec<usize> = locked.iter().map(|(_, id)| *id).collect();
        assert!(ids.iter().copied().min().unwrap() <= 1, "IN never reached: {ids:?}");
        assert!(
            ids.iter().copied().max().unwrap() + 2 >= FRAMES,
            "OUT never reached: {ids:?}"
        );

        // Bounce reversals happen and track the beat count (one turn per
        // beat; wide tolerance — this is a threaded pipeline, the pure
        // cadence law is pinned above).
        let mut turns = 0;
        let mut dir_up = true;
        for pair in ids.windows(2) {
            if pair[1] != pair[0] {
                let up = pair[1] > pair[0];
                if up != dir_up {
                    turns += 1;
                    dir_up = up;
                }
            }
        }
        let beats = 9; // ~3.6s of locked regime at 400ms
        assert!(
            turns >= beats / 2 && turns <= beats * 2,
            "{turns} turns over ~{beats} beats — the sweep is not on the beat grid"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE TAIL-ONLY CACHE LIE, pinned: a mid-clip SCRUB rebuilds the
    /// decoder from its target — that pass may NEVER declare a complete
    /// cache (it covers target→OUT, not the window). The wrap after it
    /// rebuilds from live IN, and the sweep must span the WHOLE range
    /// again — not loop the tail the scrub left behind.
    #[test]
    fn a_scrub_never_leaves_a_tail_only_sweep() {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        const W: u32 = 64;
        const H: u32 = 32;
        const FRAMES: usize = 12;
        fn identity_of(bgra: &[u32]) -> usize {
            let mid = bgra[(H as usize / 2) * W as usize + W as usize / 2];
            (((mid >> 16) & 0xff) as usize) / 16
        }
        let dir = std::env::temp_dir()
            .join(format!("vj-media-scrubtail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scrub.mp4");
        let mut encoder = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: W,
                height: H,
                fps_num: 24,
                fps_den: 1,
                video_bitrate_bps: 2_000_000,
                audio: None,
                keyframe_only: true,
            },
        )
        .expect("encoder");
        for index in 0..FRAMES {
            encoder
                .push_frame_rgb8(
                    &vec![(index * 16 + 8) as u8; W as usize * H as usize * 3],
                    None,
                )
                .expect("push");
        }
        encoder.finish().expect("finish");
        let mixer = Mixer::new();
        let mut player = SlotPlayer::open(
            SlotId::A,
            path.to_str().unwrap(),
            MediaType::Mp4,
            mixer,
            true,
            false,
        )
        .expect("open");
        player.set_muted(true);
        player.set_beats_per_sweep(1);
        player.set_beat_hint(4_000_000);
        player.set_beat_transport(true);
        // Let the first window cache complete and the sweep run…
        let warm = Instant::now() + Duration::from_millis(1_500);
        while Instant::now() < warm {
            let _ = player.take_due_frame();
            std::thread::sleep(Duration::from_millis(3));
        }
        // …then SCRUB to 60% and pulse the clock like the app would.
        player.seek_fraction(0.6);
        let mut ids: Vec<usize> = Vec::new();
        let mut next_pulse = Instant::now() + Duration::from_millis(400);
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            if Instant::now() >= next_pulse {
                player.beat_pulse();
                next_pulse += Duration::from_millis(400);
            }
            if let Some(bgra) = player.take_due_frame() {
                ids.push(identity_of(&bgra));
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        // The sweep must reach the head again — a tail-only cache never
        // shows anything below the scrub target (frame 7).
        assert!(
            ids.iter().copied().min().unwrap_or(99) <= 1,
            "sweep never returned to the head after a scrub: {ids:?}"
        );
        assert!(
            ids.iter().copied().max().unwrap_or(0) + 2 >= FRAMES,
            "sweep lost the tail after a scrub: {ids:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The FREE path's wrap, measured for comparison (printed with
    /// --nocapture): the classic loop pushes REAL pts and re-bases the
    /// pacer clock at the wrap, which costs a frame-scale dwell at the
    /// seam — the number that used to read as the loop "hiccup". Sanity
    /// bound only; the synced transport above is the product lane.
    #[test]
    fn free_loop_wrap_gap_measured() {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        const W: u32 = 64;
        const H: u32 = 32;
        let dir = std::env::temp_dir()
            .join(format!("vj-media-freegap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("free.mp4");
        let mut encoder = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: W,
                height: H,
                fps_num: 24,
                fps_den: 1,
                video_bitrate_bps: 2_000_000,
                audio: None,
                keyframe_only: true,
            },
        )
        .expect("encoder");
        for index in 0..12usize {
            encoder
                .push_frame_rgb8(
                    &vec![(index * 16 + 8) as u8; W as usize * H as usize * 3],
                    None,
                )
                .expect("push");
        }
        encoder.finish().expect("finish");
        let mixer = Mixer::new();
        let mut player = SlotPlayer::open(
            SlotId::A,
            path.to_str().unwrap(),
            MediaType::Mp4,
            mixer,
            true,
            false,
        )
        .expect("open");
        player.set_muted(true);
        let mut stamps: Vec<Instant> = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(4_000);
        while Instant::now() < deadline {
            if player.take_due_frame().is_some() {
                stamps.push(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(stamps.len() > 40, "starved: {}", stamps.len());
        let mut gaps: Vec<Duration> = stamps
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]))
            .collect();
        gaps.sort_unstable();
        println!(
            "free-loop presentation gaps: n={} p50={:?} p95={:?} max={:?}",
            gaps.len(),
            gaps[gaps.len() / 2],
            gaps[gaps.len() * 95 / 100],
            *gaps.last().unwrap()
        );
        // Three frame-times: the wrap must cost nothing visible. (Before
        // the frame-scale WRAP_MARGIN this clip raced at poll speed —
        // p50 five milliseconds — because a sub-500ms loop never re-based
        // the pacer.)
        assert!(*gaps.last().unwrap() < Duration::from_millis(125));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
