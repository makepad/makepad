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
use makepad_widgets::makepad_platform::thread::{lock_from_ui, ThreadSpawner};
use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder, VideoFileInfo};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use crate::clock::Instant;
use std::time::Duration;

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

/// The picture a video slot hands the presenter, and the small CPU
/// helpers that go with it — including `tl_on`: `VJ_TL=1` turns on the
/// TIMELINE TRACE (stderr), where every transport decision on every
/// playback path (the flow warp clock, the cache sweep, the seek-bounce
/// reverse legs) logs its position/rate/mode as it happens, so a jank
/// report reads straight off the log instead of needing a repro under a
/// debugger. Off, it costs one relaxed bool load per frame. Both moved into `makepad-frametween` when the
/// tweener became a library — the tween worker needs them, and one
/// definition beats two. These are the VJ's names for them, unchanged.
pub use makepad_frametween::frame::{
    nv12_cut_score, tl_on, Frame, Pixels,
};

/// Small BGRA proxy of an NV12 frame for the POINT-SAMPLING consumers
/// (light zones, loop signatures): nearest-sampled, BT.709 limited — a
/// few thousand texels on the CPU, never the full frame.
pub fn nv12_proxy_bgra(data: &[u8], w: usize, h: usize, pw: usize, ph: usize) -> Vec<u32> {
    let mut out = vec![0u32; pw * ph];
    if w == 0 || h == 0 || data.len() < w * h * 3 / 2 {
        return out;
    }
    let (y_plane, uv_plane) = data.split_at(w * h);
    for py in 0..ph {
        let sy = (py * h + h / 2) / ph.max(1);
        let sy = sy.min(h - 1);
        let uv_row = &uv_plane[(sy / 2) * w..];
        for px_i in 0..pw {
            let sx = ((px_i * w + w / 2) / pw.max(1)).min(w - 1);
            let c = y_plane[sy * w + sx] as i32 - 16;
            let d = uv_row[(sx / 2) * 2] as i32 - 128;
            let e = uv_row[(sx / 2) * 2 + 1] as i32 - 128;
            let r = ((298 * c + 459 * e + 128) >> 8).clamp(0, 255) as u32;
            let g = ((298 * c - 55 * d - 136 * e + 128) >> 8).clamp(0, 255) as u32;
            let b = ((298 * c + 541 * d + 128) >> 8).clamp(0, 255) as u32;
            out[py * pw + px_i] = 0xff00_0000 | (r << 16) | (g << 8) | b;
        }
    }
    out
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
    /// Backwards looping, riding exactly the tiers ping-pong rides: the
    /// decoded-frame cache when the clip fits, the GOP-batch seek-hop
    /// reverse when it does not. Like ping-pong it is silent, and like
    /// ping-pong the very first decode pass plays forward while the cache
    /// fills — the honest price of reverse on a forward codec.
    Reverse = 3,
}

impl PlayMode {
    fn from_u8(v: u8) -> PlayMode {
        match v {
            1 => PlayMode::Loop,
            2 => PlayMode::PingPong,
            3 => PlayMode::Reverse,
            _ => PlayMode::Once,
        }
    }
}

/// Decoded-frame cache ceiling for ping-pong/reverse (BGRA bytes). A clip
/// under this ceiling gets frame-exact bidirectional playback from memory;
/// over it, reverse degrades to the seek-bounce tier (correct but decode-
/// heavy). Sized on the operator's ruling that RAM is the cheap resource
/// here — at most TWO videos ever play at once, so two of these ceilings
/// is the true worst case. 16 GB holds ~9 s of a Retina screen capture
/// (3034×1882, 22.8 MB/frame), ~30 s of 1920×1080, minutes of 1280×704.
/// Bigger clips fall through to the seek-bounce tier below.
const MAX_PINGPONG_CACHE_BYTES: usize = if usize::BITS >= 64 {
    17_179_869_184_u64
} else {
    (usize::MAX - 1) as u64
} as usize;

/// Seek-bounce (tier 3): how far one reverse hop reaches back. Two seconds
/// is a typical GOP, so most of what the in-seek discard walk decodes is
/// the window itself.
const REVERSE_WINDOW_100NS: i64 = 20_000_000;

/// Byte cap on one collected reverse window. When the window's frames
/// exceed it (giant formats), it keeps its NEWEST frames and the next hop
/// re-decodes the trimmed head — reverse stays correct, just costs more
/// decode. Sized for the performance machine, because an undersized cap is
/// catastrophic, not degraded: a Retina screen capture (3034×1882, 22.8 MB
/// a frame, ~57-frame GOP ≈ 1.3 GB decoded) used to hit the old 96 MB cap
/// after FOUR frames, so every 1.6 s GOP decode served 4 frames and reverse
/// ran at 1/25 speed. 4 GB holds several such GOPs — and ~2 s of 4K60.
const REVERSE_WINDOW_MAX_BYTES: usize = if usize::BITS >= 64 {
    4_294_967_296_u64
} else {
    (usize::MAX - 1) as u64
} as usize;

struct SlotShared {
    stop: AtomicBool,
    paused: AtomicBool,
    mode: AtomicU8,
    /// Frames of tail to cross-fade onto the head when the repeat window is
    /// built, closing a generated clip into a seamless loop (see
    /// `loop_close`). 0 = off, which is every clip the operator did not ask
    /// this of: a clip that already loops must not be shortened and
    /// dissolved behind their back.
    seam_wrap: AtomicUsize,
    /// What the closer actually did, for the run row's words.
    seam_applied: AtomicUsize,
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
    /// BEAT TRANSPORT: while on, the law-paced FIRST PASS (the streaming
    /// decode while the cache fills) runs at the sweep rate the beats chip
    /// and the beat hint derive. The resident tier's beat lock lives in
    /// the presenter's platter (transport.rs), not here.
    beat_transport: AtomicBool,
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
    /// The direction the STREAMING tiers are serving (the tier-3 seek
    /// bounce publishes its leg) so the REV button can track it; a
    /// resident clip's button reads the platter's map instead.
    travel_forward: AtomicBool,
    /// THE EAGER REPEAT CACHE: a dedicated worker decodes the trim window
    /// at full hardware speed the moment a loop-capable mode is on — the
    /// frame-exact transports unlock in a fraction of one play-through
    /// (the old shape filled a cache at PLAYBACK pace as a side effect of
    /// the first pass: "the clip must play once before it bounces").
    repeat_cache: Mutex<RepeatCacheSlot>,
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
    /// The source carries a soundtrack (residency is "under budget AND
    /// silent": an unmuted loop with audio streams).
    has_audio: bool,
    /// Minted at open from a process counter — see [`Self::generation`].
    generation: u64,
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
            seam_wrap: AtomicUsize::new(0),
            seam_applied: AtomicUsize::new(0),
            muted: AtomicBool::new(false),
            scrub: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
            trim_in_100ns: AtomicI64::new(0),
            trim_out_100ns: AtomicI64::new(i64::MAX),
            beat_transport: AtomicBool::new(false),
            beats_per_sweep: AtomicU8::new(4),
            beat_hint_100ns: AtomicI64::new(0),
            scratch_active: AtomicBool::new(false),
            scratch_rate_bits: AtomicU64::new(0f64.to_bits()),
            travel_forward: AtomicBool::new(true),
            repeat_cache: Mutex::new(RepeatCacheSlot::default()),
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
            has_audio: info.has_audio,
            generation: {
                static NEXT: AtomicU64 = AtomicU64::new(1);
                NEXT.fetch_add(1, Ordering::Relaxed)
            },
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
        self.shared.failure.try_lock().ok()?.clone()
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
                || self
                    .shared
                    .frames
                    .try_lock()
                    .map_or(true, |frames| !frames.is_empty()))
    }

    /// Ask for this clip's loop to be CLOSED: when the repeat window is
    /// built, cross-fade `wrap` frames of tail onto the head so the pad's
    /// wrap is not a jump cut. Set before the window fills (i.e. at cue
    /// time); 0 turns it off.
    pub fn set_seam_close(&mut self, wrap: usize) {
        self.shared.seam_wrap.store(wrap, Ordering::Release);
    }

    /// Frames the closer actually spent, once the window exists; 0 = the
    /// clip plays as it came.
    pub fn seam_closed(&self) -> usize {
        self.shared.seam_applied.load(Ordering::Acquire)
    }

    pub fn set_loop(&mut self, loop_on: bool) {
        self.set_mode(if loop_on { PlayMode::Loop } else { PlayMode::Once });
    }

    pub fn set_mode(&mut self, mode: PlayMode) {
        self.shared.mode.store(mode as u8, Ordering::Release);
    }

    /// The direction the STREAMING transport is serving (true = forward).
    pub fn travel_forward(&self) -> bool {
        self.shared.travel_forward.load(Ordering::Acquire)
    }

    /// The eager repeat cache, once complete — the tweener reads frame
    /// pairs straight out of it.
    pub fn cache_frames(&self) -> Option<Arc<Vec<Frame>>> {
        lock_from_ui(&self.shared.repeat_cache).frames.clone()
    }

    /// A stable identity for this player (this cue of this clip): the
    /// presenter's platter tells a rebuilt cache of the same clip (phase
    /// preserved) from a new cue (anchored afresh) by it.
    pub fn identity(&self) -> usize {
        Arc::as_ptr(&self.shared) as usize
    }

    /// THE CLIP GENERATION: a process-monotonic number minted at open.
    /// Every per-pair product (a RIFE ladder, a cut verdict) is keyed by
    /// it, so nothing made for a previous cue can ever be adopted by this
    /// one — an address can be reused, a generation cannot.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// THE RESIDENT TIER'S FRAMES: the complete cache, but only while the
    /// residency law holds — the window fit the budget AND the repeat is
    /// silent (no soundtrack, muted, or a bounce/reverse, which never
    /// play audio). An unmuted loop with sound streams instead, so its
    /// picture stays on the audio's clock.
    pub fn resident_frames(&self) -> Option<Arc<Vec<Frame>>> {
        let mode = PlayMode::from_u8(self.shared.mode.load(Ordering::Acquire));
        let muted = self.shared.muted.load(Ordering::Acquire);
        if !repeat_is_silent(self.has_audio, muted, mode) {
            return None;
        }
        self.cache_frames()
    }

    /// The presenter says which source position is on screen (the resident
    /// tier presents from the cache; nothing else would refresh the
    /// readout the scrub bar and the loop analyzer follow).
    pub fn publish_position_secs(&self, secs: f64) {
        let pts = (secs.max(0.0) * 10_000_000.0) as i64;
        self.shared.position_100ns.store(pts, Ordering::Release);
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
        // The chip's real stops. 16 was missing after the chip grew its
        // sixteenth row — the fallback silently played 16 as 4, which is
        // why "16" ran FASTER than 8.
        let beats = if [16u8, 8, 4, 2, 1].contains(&beats) { beats } else { 4 };
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
    pub fn take_due_frame(&mut self) -> Option<Pixels> {
        if self.shared.paused.load(Ordering::Acquire) {
            return None;
        }
        let mut frames = self.shared.frames.try_lock().ok()?;
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
        due.map(|f| f.px)
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
    let preroll_deadline = Instant::now() + PREROLL_AUDIO_TIMEOUT;
    // The repeat cache is EAGER now: `ensure_repeat_fill` (called every
    // loop iteration below) keeps a dedicated worker decoding the trim
    // window at full hardware speed into `shared.repeat_cache`; this pass
    // only PLAYS. The whole per-pass collect/partial/complete dance died
    // with the old shape.
    // Latched when this decoder's seek fails: never retry a broken seam.
    let mut seek_bounce_broken = false;
    // Law-paced first pass state: the last REAL video pts seen and the
    // last sane real inter-frame delta (used across wrap seams, where
    // the real pts jump backward).
    let mut last_real_pts: Option<i64> = None;
    let mut last_real_delta: i64 = 416_667;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        // The eager fill worker: spawned/refreshed here (a no-op lock
        // when settled). Trim-epoch invalidation lives inside it.
        ensure_repeat_fill(&shared, path, &info);
        // Seek: reopen and discard up to the target.
        let seek = shared.seek_100ns.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            match VideoFileDecoder::open(&path) {
                Ok(d) => {
                    decoder = d;
                    audio_eos = !info.has_audio;
                    shared.frames.lock().unwrap().clear();
                    mixer.flush_slot_audio_from_worker(slot);
                    // Discard video frames strictly before the target.
                    loop {
                        if shared.stop.load(Ordering::Acquire) {
                            return;
                        }
                        match decoder.next_frame() {
                            Ok(Some(frame)) if frame.pts_100ns + 400_000 < seek => continue,
                            Ok(Some(frame)) => {
                                push_frame(&shared, frame);
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
                        push_frame(&shared, frame);
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
                        .saturating_mul(3)
                        / 2;
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
                push_frame_paced(&shared, frame, pace);
                if shared.preroll_status.load(Ordering::Acquire)
                    != PrerollStatus::Ready as u8
                {
                    shared
                        .preroll_status
                        .store(PrerollStatus::Ready as u8, Ordering::Release);
                }
                // LIVE handover mid-pass — the eager fill's whole point:
                // the moment the cache is ready (or the seek tier is the
                // verdict) the frame-exact transport takes over, instead
                // of the old wait for this pass's EOS ("the clip must
                // play once first").
                let live_mode = PlayMode::from_u8(shared.mode.load(Ordering::Acquire));
                let scratching = shared.scratch_active.load(Ordering::Acquire);
                let silent_now = repeat_is_silent(
                    info.has_audio,
                    shared.muted.load(Ordering::Acquire),
                    live_mode,
                );
                let (cache, over_budget) = {
                    let rc = shared.repeat_cache.lock().unwrap();
                    (rc.frames.clone(), rc.over_budget)
                };
                if silent_now
                    && (live_mode != PlayMode::Once || scratching)
                    && cache.is_some()
                {
                    park_while_resident(&shared, info.has_audio);
                    if shared.stop.load(Ordering::Acquire) {
                        return;
                    }
                    let trim_in = shared.trim_in_100ns.load(Ordering::Acquire).max(0);
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
                let wants_tier3 = matches!(
                    live_mode,
                    PlayMode::PingPong | PlayMode::Reverse
                ) || scratching;
                if over_budget
                    && wants_tier3
                    && !seek_bounce_broken
                    && info.duration_100ns > 0
                    && silent_now
                {
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
                    // Hand back like an EOS wrap; a pending seek (the
                    // scratch release path leaves one) is serviced by the
                    // normal loop machinery right after.
                    let trim_in = shared.trim_in_100ns.load(Ordering::Acquire).max(0);
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
            }
            Ok(Some(_)) | Ok(None) => {
                let mode = PlayMode::from_u8(shared.mode.load(Ordering::Acquire));
                let silent = repeat_is_silent(
                    info.has_audio,
                    shared.muted.load(Ordering::Acquire),
                    mode,
                );
                let (cache, cache_over_budget) = {
                    let rc = shared.repeat_cache.lock().unwrap();
                    (rc.frames.clone(), rc.over_budget)
                };
                if mode != PlayMode::Once && silent && cache.is_some() {
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
                    park_while_resident(&shared, info.has_audio);
                }
                if (matches!(mode, PlayMode::PingPong | PlayMode::Reverse)
                    || shared.scratch_active.load(Ordering::Acquire))
                    && cache_over_budget
                    && silent
                    && !seek_bounce_broken
                    && info.duration_100ns > 0
                {
                    // TIER 3: too big for the frame cache, but a bounce (or
                    // a reverse) was asked for — GOP-batch reverse via decoder seeks. Falls
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
/// The eager repeat cache's shared slot (see `ensure_repeat_fill`).
#[derive(Default)]
struct RepeatCacheSlot {
    /// A COMPLETE decoded window — published whole, never partial.
    frames: Option<Arc<Vec<Frame>>>,
    /// Trim bounds the frames were built under.
    built: (i64, i64),
    /// Trim epoch the verdicts belong to; a trim change resets everything
    /// (a smaller window may fit where the old one did not).
    epoch: u64,
    /// The window cannot fit the budget — the seek tier serves instead.
    over_budget: bool,
    /// A fill worker is running (dedupes spawns).
    filling: bool,
}

/// Keep the eager fill honest and running: called once per decode-loop
/// iteration. Cheap when settled (one lock). The budget verdict is
/// ARITHMETIC — window length × NV12 frame bytes — so an over-budget clip
/// is known the instant it is cued, not minutes into a decode.
fn ensure_repeat_fill(shared: &Arc<SlotShared>, path: &str, info: &VideoFileInfo) {
    if PlayMode::from_u8(shared.mode.load(Ordering::Acquire)) == PlayMode::Once {
        return;
    }
    let epoch = shared.trim_epoch.load(Ordering::Acquire);
    let duration = info.duration_100ns.max(1);
    let t_in = shared.trim_in_100ns.load(Ordering::Acquire).clamp(0, duration);
    let t_out = shared.trim_out_100ns.load(Ordering::Acquire).clamp(t_in, duration);
    // No OUT of the operator's own: the window is the whole clip, and the
    // fill must not stop at a duration the container rounded down.
    let open_ended = t_out >= duration;
    {
        let mut rc = shared.repeat_cache.lock().unwrap();
        if rc.epoch != epoch {
            rc.frames = None;
            rc.over_budget = false;
            rc.epoch = epoch;
        }
        if rc.filling || rc.over_budget {
            return;
        }
        if rc.frames.is_some() && !cache_range_outgrown(rc.built, (t_in, t_out)) {
            return;
        }
        let frame_bytes = (info.width as usize)
            .saturating_mul(info.height as usize)
            .saturating_mul(3)
            / 2;
        let delta = if info.fps_num > 0 {
            ((10_000_000 * info.fps_den.max(1) as i64) / info.fps_num as i64).max(1)
        } else {
            416_667
        };
        let est = ((t_out - t_in).max(0) as f64 / delta as f64).ceil() * frame_bytes as f64;
        if est > MAX_PINGPONG_CACHE_BYTES as f64 {
            rc.over_budget = true;
            eprintln!(
                "vj repeat cache: window needs ~{:.0} MB (budget {} MB); the seek tier serves reverse/bounce",
                est / 1e6,
                MAX_PINGPONG_CACHE_BYTES >> 20
            );
            return;
        }
        rc.frames = None;
        rc.filling = true;
    }
    let shared = shared.clone();
    let path = path.to_string();
    let _ = std::thread::Builder::new().name("vj-cache-fill".into()).spawn(move || {
        let t0 = Instant::now();
        let ok = repeat_fill_worker(&shared, &path, epoch, t_in, t_out, open_ended);
        shared.repeat_cache.lock().unwrap().filling = false;
        if tl_on() {
            eprintln!("tl fill done ok={ok} in {}ms", t0.elapsed().as_millis());
        }
    });
}

/// The fill itself: a PRIVATE decoder, video track only, running at
/// whatever speed the hardware gives. Aborts quietly on stop or a trim
/// change; publishes only a whole window under its own epoch.
fn repeat_fill_worker(
    shared: &Arc<SlotShared>,
    path: &str,
    epoch: u64,
    t_in: i64,
    t_out: i64,
    open_ended: bool,
) -> bool {
    let Ok(mut decoder) = VideoFileDecoder::open(path) else { return false };
    if t_in > 0 {
        // Seek failure is fine: decode from zero, the discard arm below
        // walks up to IN.
        let _ = decoder.seek(t_in);
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut bytes = 0usize;
    loop {
        if shared.stop.load(Ordering::Acquire)
            || shared.trim_epoch.load(Ordering::Acquire) != epoch
        {
            return false;
        }
        match decoder.next_frame() {
            Ok(Some(f)) if f.pts_100ns + 400_000 < t_in => {}
            // An UNTRIMMED window runs to end-of-file, never to the
            // container's idea of the duration. Those two disagree: the
            // decoder hands back each frame's presentation time one frame
            // later than the encoder wrote it, so the LAST frame's pts
            // lands just past a duration that was rounded down — and a
            // strict `< t_out` quietly lopped that frame off every
            // resident cache. A loop then skipped its own last frame,
            // forever. A real trim still gates on OUT, where the operator
            // put it.
            Ok(Some(f)) if open_ended || f.pts_100ns < t_out => {
                let frame = decoded_frame(f);
                bytes += frame.px.byte_len();
                if bytes > MAX_PINGPONG_CACHE_BYTES {
                    // The arithmetic verdict missed (VFR denser than the
                    // container's fps claim): same outcome, later.
                    let mut rc = shared.repeat_cache.lock().unwrap();
                    if rc.epoch == epoch {
                        rc.over_budget = true;
                    }
                    eprintln!("vj repeat cache: fill blew the budget; the seek tier serves reverse/bounce");
                    return false;
                }
                frames.push(frame);
            }
            Ok(Some(_)) | Ok(None) => break,
            Err(_) => return false,
        }
    }
    if frames.len() < 2 {
        return false;
    }
    // The one moment a generated clip can be closed into a loop: the whole
    // window is decoded and in hand, and nothing has presented it yet.
    let wrap = shared.seam_wrap.load(Ordering::Acquire);
    let applied = match wrap {
        0 => 0,
        wrap => match crate::loop_close::close_loop(&mut frames, wrap) {
            crate::loop_close::LoopClosure::Crossfade { wrap } => wrap,
            crate::loop_close::LoopClosure::None => 0,
        },
    };
    shared.seam_applied.store(applied, Ordering::Release);
    let mut rc = shared.repeat_cache.lock().unwrap();
    if rc.epoch != epoch {
        return false;
    }
    rc.built = (t_in, t_out);
    rc.frames = Some(Arc::new(frames));
    true
}

/// Wrap a decoded frame for residency: the NV12 planes MOVE straight in.
fn decoded_frame(
    frame: makepad_widgets::makepad_platform::video_file::DecodedVideoFrame,
) -> Frame {
    Frame {
        pts_100ns: frame.pts_100ns,
        clip_100ns: frame.pts_100ns,
        px: Pixels::Nv12 { data: frame.nv12, width: frame.width, height: frame.height },
    }
}

fn push_frame(
    shared: &Arc<SlotShared>,
    frame: makepad_widgets::makepad_platform::video_file::DecodedVideoFrame,
) -> Frame {
    push_frame_paced(shared, frame, None)
}

/// Like [`push_frame`], but the ring copy may carry a synthetic PACING
/// pts (the law-paced first pass) while the returned frame — what the
/// repeat cache stores — always keeps the REAL clip pts.
fn push_frame_paced(
    shared: &Arc<SlotShared>,
    frame: makepad_widgets::makepad_platform::video_file::DecodedVideoFrame,
    pace_pts: Option<i64>,
) -> Frame {
    let converted = decoded_frame(frame);
    let out = Frame {
        pts_100ns: converted.pts_100ns,
        clip_100ns: converted.clip_100ns,
        px: converted.px.clone(),
    };
    let ring = Frame {
        pts_100ns: pace_pts.unwrap_or(converted.pts_100ns),
        clip_100ns: converted.clip_100ns,
        px: converted.px,
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
    !has_audio || muted || matches!(mode, PlayMode::PingPong | PlayMode::Reverse)
}

/// Whether the LIVE trim range asks for anything outside the range the
/// cache was decoded under. Pure bounds-vs-bounds — frame pts never enter
/// it, so container start offsets cannot fake an uncovered range.
fn cache_range_outgrown(built: (i64, i64), live: (i64, i64)) -> bool {
    live.0 < built.0 || live.1 > built.1
}

/// THE RESIDENT PARK. Once a silent, loop-capable clip's window is
/// resident (the complete cache the presenter's platter serves from —
/// transport.rs / platter.rs own position from here on), the decode
/// thread has nothing to add: it idles here, pushing nothing, until the
/// decoder is needed again — a seek (the presenter cued the stream: the
/// ring restarts from the target), Once mode (no repeat to serve), the
/// live trim growing past the window the cache was built under (the
/// eager fill must fetch the uncovered part), the repeat going audible
/// (an unmuted loop with a soundtrack streams so its picture stays on the
/// audio's clock), the cache being dropped (a trim epoch), or stop. The
/// ring is cleared on entry so nothing decoded under the old regime is
/// presented after the handover; the presenter anchors at the frame it
/// last showed, never at a queue tail.
///
/// This replaced `cache_playback`, the media thread's own sweep clock
/// (phase + beat nudge + published pos/rate atomics): two clocks and a
/// position writer the platter has no room for.
fn park_while_resident(shared: &Arc<SlotShared>, has_audio: bool) {
    shared.frames.lock().unwrap().clear();
    let built = shared.repeat_cache.lock().unwrap().built;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        if shared.seek_100ns.load(Ordering::Acquire) >= 0 {
            return;
        }
        let mode = PlayMode::from_u8(shared.mode.load(Ordering::Acquire));
        if mode == PlayMode::Once {
            return;
        }
        if !repeat_is_silent(has_audio, shared.muted.load(Ordering::Acquire), mode) {
            return;
        }
        let live = (
            shared.trim_in_100ns.load(Ordering::Acquire),
            shared.trim_out_100ns.load(Ordering::Acquire),
        );
        if cache_range_outgrown(built, live) {
            return;
        }
        if shared.repeat_cache.lock().unwrap().frames.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(8));
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
    /// including a trim change (the bounce bounds moved under us). A live
    /// SCRATCH keeps the machine alive whatever the mode: the hand entered
    /// through this tier and must keep being served by it.
    fn must_exit(shared: &SlotShared, has_audio: bool, epoch0: u64) -> bool {
        shared.stop.load(Ordering::Acquire)
            || shared.seek_100ns.load(Ordering::Acquire) >= 0
            || shared.trim_epoch.load(Ordering::Acquire) != epoch0
            || (!matches!(
                PlayMode::from_u8(shared.mode.load(Ordering::Acquire)),
                PlayMode::PingPong | PlayMode::Reverse
            ) && !shared.scratch_active.load(Ordering::Acquire))
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
    // Continue the presentation clock from wherever the forward pass ended.
    let mut synth_pts = shared
        .frames
        .lock()
        .unwrap()
        .back()
        .map(|f| f.pts_100ns)
        .unwrap_or(0)
        .max(shared.position_100ns.load(Ordering::Acquire));
    let serve = |shared: &SlotShared, px: Pixels, clip_100ns: i64, synth_pts: &mut i64| {
        *synth_pts += delta;
        if tl_on() {
            eprintln!(
                "tl hop frame clip={:.3}s synth={:.3}s",
                clip_100ns as f64 / 1e7,
                *synth_pts as f64 / 1e7
            );
        }
        shared
            .frames
            .lock()
            .unwrap()
            .push_back(Frame { pts_100ns: *synth_pts, clip_100ns, px });
        shared.video_ready.store(true, Ordering::Release);
    };
    // A reverse window mid-collection: frames decoded so far (oldest
    // first), their byte total, and the span it covers.
    struct Collecting {
        frames: VecDeque<Frame>,
        bytes: usize,
        lo: i64,
        hi: i64,
        done: bool,
        t0: Instant,
        /// Microseconds inside decoder.next_frame() / convert_frame().
        dec_us: u64,
        cvt_us: u64,
    }
    loop {
        // ---- reverse leg: OUT → IN, PIPELINED windows, plus the SCRATCH
        // SERVICE. The old shape (collect a window, then serve it, then
        // collect the next) stalled for a whole GOP decode between windows
        // — on a big-GOP clip that read as a hiccup every second. This is
        // a one-thread interleave instead: every iteration serves ONE due
        // frame from the resident window when the ring has room, otherwise
        // decodes ONE frame of the window below it — the ring's
        // backpressure idle time IS the decode-ahead time, and reverse
        // runs continuously whenever the decoder is at least realtime.
        //
        // The resident window is served BY INDEX, walking down; a served
        // frame's buffer moves out (`ceiling` marks the highest intact
        // index). That indexed residency is what makes SCRATCH possible on
        // a streaming clip at all: while the hand is down, the index walks
        // by the hand's rate instead — both directions, clamped to the
        // intact span — and release resumes reverse from wherever the
        // hand left the picture. A scratch that began in a non-bounce
        // mode hands back through a seek to the hand's position, so the
        // forward stream resumes there, not at IN.
        shared.travel_forward.store(false, Ordering::Release);
        let mut hi = t_out;
        let mut current: VecDeque<Frame> = VecDeque::new();
        // serve_idx: the NEXT index to serve (fractional while the hand
        // owns it). ceiling: highest index whose buffer is still intact.
        let mut serve_idx: f64 = -1.0;
        let mut ceiling: f64 = -1.0;
        let mut collecting: Option<Collecting> = None;
        let mut last_clip = t_out;
        let mut scratching = false;
        // When the resident window drained with the collector still busy:
        // the visible stall the pipeline exists to prevent.
        let mut drained_at: Option<Instant> = None;
        loop {
            if must_exit(shared, has_audio, epoch0) {
                return Ok(true);
            }
            let scratch_on = shared.scratch_active.load(Ordering::Acquire);
            if scratching && !scratch_on {
                // Release. In a bounce/reverse the leg resumes from the
                // hand's position by itself; from any other mode the
                // machine exits, seeking the stream to the hand.
                scratching = false;
                _ = scratching;
                shared.travel_forward.store(false, Ordering::Release);
                if !matches!(
                    PlayMode::from_u8(shared.mode.load(Ordering::Acquire)),
                    PlayMode::PingPong | PlayMode::Reverse
                ) {
                    shared.seek_100ns.store(last_clip.max(0), Ordering::Release);
                    return Ok(true);
                }
            }
            scratching = scratch_on;
            if shared.paused.load(Ordering::Acquire) && !scratching {
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            // 1) A due frame out of the resident window.
            let ring_full = shared.frames.lock().unwrap().len() >= RING_FRAMES;
            if !ring_full && serve_idx >= 0.0 && !current.is_empty() {
                if scratching {
                    // THE HAND: walk the index by the shuttle rate, both
                    // directions, inside the intact span. Buffers are
                    // CLONED (not moved) so the hand can revisit.
                    let srate = f64::from_bits(
                        shared.scratch_rate_bits.load(Ordering::Acquire),
                    )
                    .clamp(-8.0, 8.0);
                    serve_idx = (serve_idx + srate).clamp(0.0, ceiling.max(0.0));
                    let idx = (serve_idx.round() as usize).min(current.len() - 1);
                    let frame = &current[idx];
                    last_clip = frame.clip_100ns;
                    shared.travel_forward.store(srate >= 0.0, Ordering::Release);
                    if tl_on() {
                        eprintln!(
                            "tl hop scratch idx={idx} srate={srate:+.3} clip={:.3}s",
                            last_clip as f64 / 1e7
                        );
                    }
                    serve(shared, frame.px.clone(), frame.clip_100ns, &mut synth_pts);
                    continue;
                }
                let idx = (serve_idx.floor() as usize).min(current.len() - 1);
                let px = std::mem::take(&mut current[idx].px);
                last_clip = current[idx].clip_100ns;
                serve_idx = idx as f64 - 1.0;
                ceiling = serve_idx;
                if serve_idx < 0.0 {
                    drained_at = Some(Instant::now());
                }
                serve(shared, px, last_clip, &mut synth_pts);
                continue;
            }
            // 2) Arm the collector for the window below `hi`.
            if collecting.is_none() {
                if hi > t_in {
                    let lo = (hi - REVERSE_WINDOW_100NS).max(t_in);
                    if decoder.seek(lo).is_err() {
                        return Ok(false);
                    }
                    collecting = Some(Collecting {
                        frames: VecDeque::new(),
                        bytes: 0,
                        lo,
                        hi,
                        done: false,
                        t0: Instant::now(),
                        dec_us: 0,
                        cvt_us: 0,
                    });
                } else if PlayMode::from_u8(shared.mode.load(Ordering::Acquire))
                    == PlayMode::Reverse
                    && !scratching
                {
                    // REVERSE wraps IN → OUT. Re-arming here — while the
                    // bottom window is still serving — is what kills the
                    // old freeze at the loop point: the top window decodes
                    // in the serve slack instead of after it.
                    hi = t_out;
                    continue;
                } else if serve_idx < 0.0 || current.is_empty() {
                    // Ping-pong: the leg ends when the last resident
                    // frame has been served; a pinned scratch just holds.
                    if scratching {
                        std::thread::sleep(Duration::from_millis(4));
                        continue;
                    }
                    break;
                } else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
            }
            let Some(c) = collecting.as_mut() else { continue };
            // 3) One decode step of the pending window.
            if !c.done {
                let dec_t0 = Instant::now();
                let step = decoder.next_frame();
                c.dec_us += dec_t0.elapsed().as_micros() as u64;
                match step {
                    // A window seek lands on the prior keyframe: frames
                    // before IN never enter the window.
                    Ok(Some(f)) if f.pts_100ns + 400_000 < t_in => {}
                    Ok(Some(f)) if f.pts_100ns < c.hi => {
                        let cvt_t0 = Instant::now();
                        let frame = decoded_frame(f);
                        c.cvt_us += cvt_t0.elapsed().as_micros() as u64;
                        c.bytes += frame.px.byte_len();
                        c.frames.push_back(frame);
                        while c.bytes > REVERSE_WINDOW_MAX_BYTES && c.frames.len() > 1 {
                            let dropped = c.frames.pop_front().unwrap();
                            c.bytes -= dropped.px.byte_len();
                        }
                    }
                    Ok(Some(_)) | Ok(None) => {
                        c.done = true;
                        if tl_on() {
                            eprintln!(
                                "tl hop window {:.3}s..{:.3}s kept={} bytes={}MB collect={}ms decode={}ms convert={}ms",
                                c.lo as f64 / 1e7,
                                c.hi as f64 / 1e7,
                                c.frames.len(),
                                c.bytes >> 20,
                                c.t0.elapsed().as_millis(),
                                c.dec_us / 1000,
                                c.cvt_us / 1000
                            );
                        }
                    }
                    Err(e) => return Err(e.to_string()),
                }
                continue;
            }
            // 4) Collected and waiting: promote once the resident window
            // has drained (never earlier — serve order is strictly
            // newest-first across windows). A held scratch only promotes
            // into the window ADJACENT below it — never across a reverse
            // wrap, which would teleport the hand to the clip's end.
            let drained = serve_idx < 0.0 || current.is_empty();
            let hand_at_floor = scratching && serve_idx <= 0.0;
            let adjacent = current
                .front()
                .map(|f| c.hi <= f.pts_100ns)
                .unwrap_or(true);
            if (drained && !scratching) || (hand_at_floor && adjacent) {
                let c = collecting.take().unwrap();
                if tl_on() {
                    let stall = drained_at
                        .take()
                        .map(|at| at.elapsed().as_millis())
                        .unwrap_or(0);
                    eprintln!(
                        "tl hop promote kept={} stall={}ms (window was ready {})",
                        c.frames.len(),
                        stall,
                        if stall == 0 { "in time" } else { "LATE" }
                    );
                }
                match c.frames.front().map(|f| f.pts_100ns) {
                    // Strictly decreasing: every kept frame had pts < hi.
                    Some(first_kept) => {
                        hi = first_kept;
                        serve_idx = c.frames.len() as f64 - 1.0;
                        ceiling = serve_idx;
                        current = c.frames;
                    }
                    // Dead air (no frames in the window): keep walking.
                    None => hi = c.lo,
                }
            } else {
                // Ring full, window decoded, resident still serving.
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        // REVERSE mode loops the reverse leg only — the forward pass
        // belongs to ping-pong's bounce. Read LIVE so a picker switch
        // between the two mid-play changes the very next leg.
        if PlayMode::from_u8(shared.mode.load(Ordering::Acquire)) == PlayMode::Reverse {
            continue;
        }
        // ---- forward leg: IN → OUT, a plain decode pass.
        shared.travel_forward.store(true, Ordering::Release);
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
                    let frame = decoded_frame(f);
                    serve(shared, frame.px, frame.clip_100ns, &mut synth_pts);
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
    decode_audio_source(&DecodeSource::Path(path.clone()), media, max_frames)
}

fn decode_audio_source(
    source: &DecodeSource,
    media: MediaType,
    max_frames: usize,
) -> Result<TrackPcm, String> {
    match media {
        MediaType::Wav => {
            let bytes = source.read_bytes()?;
            parse_wav(&bytes, max_frames)
        }
        MediaType::Mp4 => {
            let DecodeSource::Path(path) = source else {
                return Err("hardware MP4 audio decode is unavailable for in-memory web blobs".into());
            };
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
            let bytes = source.read_bytes()?;
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

impl DecodeSource {
    fn read_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Path(path) => std::fs::read(path).map_err(|error| error.to_string()),
            Self::Bytes(bytes) => Ok(bytes.to_vec()),
        }
    }
}

/// Min/max waveform columns over the whole clip.
/// Columns in the pre-listen player's seek strip — one bin per bar.
pub const PREVIEW_WAVE_COLS: usize = 240;

/// The pre-listen strip's shape: ENERGY (RMS) per bin, divided by the
/// loudest bin so the busiest moment of the track fills the strip.
///
/// Deliberately NOT peak-per-bin. Over a bin this wide almost every one of
/// a loud master's bins contains a full-scale sample, so a peak strip ties
/// dozens of bars at exactly the ceiling and draws a hard flat line along
/// the top and bottom — which reads as a waveform with its head and feet
/// cut off, however much room is left around it. Energy varies smoothly,
/// ties nowhere, and is what the ear follows anyway: the intro, the drop
/// and the outro are all visible in it.
pub fn preview_wave_bins(pcm: &TrackPcm, cols: usize) -> Vec<f32> {
    let cols = cols.max(1);
    if pcm.frames.is_empty() {
        return vec![0.0; cols];
    }
    let per_col = pcm.frames.len() as f64 / cols as f64;
    let mut bins = Vec::with_capacity(cols);
    let mut loudest = 0.0f32;
    for col in 0..cols {
        let start = ((col as f64 * per_col) as usize).min(pcm.frames.len() - 1);
        let end =
            (((col + 1) as f64 * per_col) as usize).clamp(start + 1, pcm.frames.len());
        let mut sum = 0.0f64;
        for frame in &pcm.frames[start..end] {
            let mono = (frame[0] as f32 + frame[1] as f32) * 0.5 / 32768.0;
            sum += (mono as f64) * (mono as f64);
        }
        let rms = (sum / (end - start).max(1) as f64).sqrt() as f32;
        loudest = loudest.max(rms);
        bins.push(rms);
    }
    // One divisor, no curve: the loudest bin is full height and every
    // other bar stands in true proportion to it.
    let scale = if loudest > 1e-6 { 1.0 / loudest } else { 0.0 };
    bins.into_iter().map(|rms| (rms * scale).clamp(0.0, 1.0)).collect()
}

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
/// step is recorded in the F3 performance graph without emitting a
/// per-item console line.

#[must_use = "a step that is never `done` is never measured"]
pub struct UiStep {
    t0: Instant,
}

impl UiStep {
    pub fn new(_what: &'static str) -> Self {
        Self { t0: Instant::now() }
    }

    /// Close the step; returns its cost in milliseconds.
    pub fn done(self, cx: &mut makepad_widgets::Cx) -> f32 {
        let us = self.t0.elapsed().as_micros() as u64;
        let channel = cx.perf_monitor.channel("load", 0xff_b4_54);
        cx.perf_monitor.add(channel, us);
        us as f32 / 1000.0
    }
}

// ---------------------------------------------------------------------------
// decode worker pool
// ---------------------------------------------------------------------------

pub enum DecodeJob {
    Deck { deck: DeckId, gen: u64, source: DecodeSource, media: MediaType },
    /// The headphone pre-listen: the same full decode as a deck, plus the
    /// overview strip for the mini player's seek bar.
    Preview { gen: u64, source: DecodeSource, media: MediaType },
    Pad { pad: PadKey, gen: u64, revision: AssetRevisionId, source: DecodeSource, media: MediaType },
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
        source: DecodeSource,
        sheet: Option<(ThumbnailCells, f32)>,
        legacy_may_be_sheet: bool,
        epoch: u64,
    },
}

/// Verified encoded media can be backed by the native cache filesystem or
/// by the portable static store's in-memory object cache. Audio/image
/// decoders consume this one seam, so web callers never invent fake paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

impl From<PathBuf> for DecodeSource {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<makepad_asset_client::BlobContent> for DecodeSource {
    fn from(content: makepad_asset_client::BlobContent) -> Self {
        match content {
            makepad_asset_client::BlobContent::Bytes(bytes) => Self::Bytes(bytes),
            #[cfg(not(target_arch = "wasm32"))]
            makepad_asset_client::BlobContent::VerifiedPath(path) => Self::Path(path),
        }
    }
}

impl DecodeSource {
    pub fn into_path(self) -> Option<PathBuf> {
        match self {
            Self::Path(path) => Some(path),
            Self::Bytes(_) => None,
        }
    }
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
    Preview {
        gen: u64,
        result: Result<(Arc<TrackPcm>, Vec<f32>), String>,
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
    /// A queued thumbnail the lane threw away UNSTARTED — its view scrolled
    /// off (stale epoch) or the pending stack overflowed. Not a failure and
    /// not a picture: the caller must clear its in-flight mark so the tile
    /// asks again next time it is wanted. Without this the drop was silent
    /// and the tile stayed blank for the rest of the session.
    ThumbDropped {
        revision: AssetRevisionId,
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
    let started = crate::clock::Instant::now();
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
    decode_thumb_source(
        &DecodeSource::Path(path.clone()),
        sheet,
        legacy_may_be_sheet,
    )
}

fn decode_thumb_source(
    source: &DecodeSource,
    sheet: Option<(ThumbnailCells, f32)>,
    legacy_may_be_sheet: bool,
) -> Result<ThumbPixels, String> {
    let mut pixels = match source {
        DecodeSource::Path(path) => decode_thumb_full(path, sheet, legacy_may_be_sheet)?,
        DecodeSource::Bytes(bytes) => decode_thumb_image_bytes(bytes, sheet)?,
    };
    fit_thumb_for_tiles(&mut pixels);
    Ok(pixels)
}

fn decode_thumb_image_bytes(
    bytes: &[u8],
    sheet: Option<(ThumbnailCells, f32)>,
) -> Result<ThumbPixels, String> {
    if bytes.len() as u64 > MAX_THUMB_BYTES {
        return Err(format!("thumbnail over byte budget: {}", bytes.len()));
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Err("hardware video thumbnails are unavailable for in-memory web blobs".into());
    }
    let image = if bytes.starts_with(&[0xff, 0xd8]) {
        makepad_widgets::ImageBuffer::from_jpg(bytes)
    } else {
        makepad_widgets::ImageBuffer::from_png(bytes)
    }
    .map_err(|error| format!("thumbnail decode failed: {error:?}"))?;
    let (width, height) = (image.width, image.height);
    if width == 0 || height == 0 || width > MAX_THUMB_DIM || height > MAX_THUMB_DIM {
        return Err(format!("thumbnail dimensions out of bounds: {width}x{height}"));
    }
    let pixels = ThumbPixels {
        bgra: image.data,
        width,
        height,
        frames: Vec::new(),
        fps: sheet.map_or(0.0, |(_, fps)| fps),
    };
    Ok(match sheet {
        Some((cells, fps)) => declared_thumb(
            pixels.width,
            pixels.height,
            &pixels.bgra,
            cells,
            fps,
        )
        .unwrap_or(pixels),
        None => pixels,
    })
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
    // VJ-effect sheets cache as tiny mp4s (fx_thumbs.rs): the file is
    // hardware-decoded ONCE here — on the same decode lane every thumbnail
    // rides — into the frames-in-a-sheet pixels the atlas path already
    // animates by frame index, and the decoder session is torn down.
    // Storage format only: zero codec work ever happens at draw time.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return decode_thumb_video(path, sheet);
    }
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

/// A video-cached animated thumbnail (the VJ-effect sheets): every frame
/// of the stream is one cell, decoded through the platform's hardware
/// path (makepad-video: VideoToolbox / Media Foundation / GStreamer) into
/// exactly the `ThumbPixels::frames` sequence a cut PNG sheet produced.
/// The declared layout caps the frame count and carries the playback fps;
/// a stream with no declaration plays at its own fps.
fn decode_thumb_video(
    path: &PathBuf,
    sheet: Option<(ThumbnailCells, f32)>,
) -> Result<ThumbPixels, String> {
    use makepad_widgets::makepad_platform::video_file::VideoFileDecoder;
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("non-utf8 thumbnail path: {}", path.display()))?;
    let mut decoder =
        VideoFileDecoder::open(path_str).map_err(|e| format!("thumb video open: {e}"))?;
    let info = decoder.info().clone();
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 || w > MAX_THUMB_DIM || h > MAX_THUMB_DIM {
        return Err(format!("thumb video dimensions out of bounds: {w}x{h}"));
    }
    let (cap, fps) = match sheet {
        Some((cells, fps)) => (cells.count.clamp(1, 512) as usize, fps),
        None => (
            512,
            if info.fps_den > 0 { info.fps_num as f32 / info.fps_den as f32 } else { 0.0 },
        ),
    };
    let declared = sheet.is_some();
    let mut frames: Vec<(i64, Vec<u32>)> = Vec::new();
    // One NV12→RGB scratch reused across every frame of the pull: this
    // decode sits on the SCROLL HOT PATH (the atlas LRU re-decodes evicted
    // sheets), so the conversion must not allocate 30 fresh buffers per
    // sheet.
    let mut rgb_scratch: Vec<u8> = Vec::new();
    while frames.len() < cap {
        let frame = match decoder.next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(e) => return Err(format!("thumb video decode: {e}")),
        };
        let (fw, fh) = (frame.width as usize, frame.height as usize);
        if fw != w || fh != h {
            return Err(format!("thumb video frame size drifted: {fw}x{fh} vs {w}x{h}"));
        }
        nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb_scratch);
        if rgb_scratch.len() < w * h * 3 {
            return Err("thumb video frame underrun".to_string());
        }
        let mut px = vec![0u32; w * h];
        for (i, c) in rgb_scratch.chunks_exact(3).take(w * h).enumerate() {
            px[i] = 0xff00_0000 | ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
        }
        frames.push((frame.pts_100ns, px));
    }
    // THE CACHE-FORMAT LAW (any mp4 whose stamped layout DECLARES its frame
    // count — the fx_thumbs sheets): the declared count is a contract, and
    // the stream must end cleanly right behind it. Probe once past the last
    // accepted frame while the session is still open: a frame there means
    // the stream carries more than it declared.
    if declared && frames.len() >= cap {
        match decoder.next_frame() {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(format!(
                    "thumb video: stream continues past the {cap} declared frames"
                ));
            }
            Err(e) => return Err(format!("thumb video decode at declared EOF: {e}")),
        }
    }
    // The decoder session ends here — the frames live on as sheet pixels.
    drop(decoder);
    // A FRAME LANDS WHERE ITS TIMESTAMP SAYS — never where the decoder's
    // emission happened to put it. Hardware decoders may emit in decode
    // order (B-frame reordering differs per platform and per session
    // mode), so the cell index derives from the frame's OWN pts:
    // `round((pts - first_pts) * fps / 1e7)`, bounds-checked — never a
    // running counter. (The bake writes all-intra streams, which cannot
    // reorder — this is the decode-side half of the same law.)
    //
    // DECLARED sheets are strict: exactly `cap` frames, one per unique pts
    // slot, or the whole decode is an ERROR — which is what fires the
    // app's cache-removal/rebake path. The old gap-hold leniency let a
    // partial sheet (one rendered cell, twenty-nine holds) pass the
    // whole-sheet-black gate and cache forever.
    if declared {
        let placed = place_declared_video_frames(frames, cap, fps)?;
        let mut frames: Vec<(Vec<u32>, usize, usize)> =
            placed.into_iter().map(|px| (px, w, h)).collect();
        return match frames.len() {
            0 => Err("thumb video decoded no frames".to_string()),
            1 => {
                let (bgra, w, h) = frames.remove(0);
                Ok(ThumbPixels { bgra, width: w, height: h, frames: Vec::new(), fps: 0.0 })
            }
            _ => Ok(ThumbPixels {
                bgra: frames[0].0.clone(),
                width: w,
                height: h,
                frames,
                fps,
            }),
        };
    }
    // UNDECLARED streams (no stamped layout) keep the forgiving path: a
    // gap holds the previous picture rather than shifting every later
    // cell; frames past the cap drop.
    let count = frames.len();
    let base_pts = frames.iter().map(|(pts, _)| *pts).min().unwrap_or(0);
    let fps_f = if fps > 1.0 { fps as f64 } else { 30.0 };
    let slot_count = cap.max(count).max(1);
    let idx_of = |pts: i64| -> Option<usize> {
        let idx = (((pts - base_pts) as f64) * fps_f / 10_000_000.0).round() as i64;
        if idx >= 0 && (idx as usize) < slot_count {
            Some(idx as usize)
        } else {
            None
        }
    };
    let mut seen = vec![false; slot_count];
    let mut placed = 0usize;
    for (pts, _) in &frames {
        if let Some(i) = idx_of(*pts) {
            if !seen[i] {
                seen[i] = true;
                placed += 1;
            }
        }
    }
    let mut frames: Vec<(Vec<u32>, usize, usize)> = if placed * 2 > count {
        let mut slots: Vec<Option<Vec<u32>>> = Vec::new();
        slots.resize_with(slot_count, || None);
        for (pts, px) in frames {
            if let Some(i) = idx_of(pts) {
                // A pts collision keeps the later emission (the decoder's
                // own correction wins).
                slots[i] = Some(px);
            }
        }
        // Fill any gap by holding the previous picture (a leading gap
        // takes the first real one), and trim trailing emptiness.
        let last = slots.iter().rposition(|s| s.is_some()).unwrap_or(0);
        slots.truncate(last + 1);
        let mut held = slots.iter().flatten().next().cloned().unwrap_or_default();
        slots
            .into_iter()
            .map(|s| {
                if let Some(px) = s {
                    held = px.clone();
                    (px, w, h)
                } else {
                    (held.clone(), w, h)
                }
            })
            .collect()
    } else {
        // Degenerate timestamps (all-equal or junk): pts-sorted emission
        // order is the best remaining truth.
        frames.sort_by_key(|(pts, _)| *pts);
        frames.into_iter().map(|(_, px)| (px, w, h)).collect()
    };
    match frames.len() {
        0 => Err("thumb video decoded no frames".to_string()),
        1 => {
            let (bgra, w, h) = frames.remove(0);
            Ok(ThumbPixels { bgra, width: w, height: h, frames: Vec::new(), fps: 0.0 })
        }
        _ => Ok(ThumbPixels {
            bgra: frames[0].0.clone(),
            width: w,
            height: h,
            frames,
            fps,
        }),
    }
}

/// THE STRICT PLACEMENT for a DECLARED-count video sheet, pure so the
/// tests can pin it: exactly `count` frames, each landing on its own
/// unique pts slot (`round((pts - first_pts) * fps / 1e7)`), no slot
/// empty, no slot doubled — anything else is an error. `count` frames
/// placed uniquely in `count` slots fills every slot by pigeonhole, so a
/// success IS a complete sheet in slot order.
fn place_declared_video_frames(
    frames: Vec<(i64, Vec<u32>)>,
    count: usize,
    fps: f32,
) -> Result<Vec<Vec<u32>>, String> {
    if frames.len() != count {
        return Err(format!(
            "thumb video: {} frames decoded, {count} declared",
            frames.len()
        ));
    }
    let base_pts = frames.iter().map(|(pts, _)| *pts).min().unwrap_or(0);
    let fps_f = if fps > 1.0 { fps as f64 } else { 30.0 };
    let mut slots: Vec<Option<Vec<u32>>> = Vec::new();
    slots.resize_with(count, || None);
    for (pts, px) in frames {
        let idx = (((pts - base_pts) as f64) * fps_f / 10_000_000.0).round() as i64;
        if idx < 0 || idx as usize >= count {
            return Err(format!(
                "thumb video: frame pts {pts} lands outside the {count} declared slots"
            ));
        }
        let slot = &mut slots[idx as usize];
        if slot.is_some() {
            return Err(format!(
                "thumb video: two frames land in pts slot {idx} of {count}"
            ));
        }
        *slot = Some(px);
    }
    let mut out = Vec::with_capacity(count);
    for slot in slots {
        match slot {
            Some(px) => out.push(px),
            // Unreachable by pigeonhole; stated as an error rather than a
            // panic (this runs on the decode workers).
            None => return Err("thumb video: pts slot accounting hole".to_string()),
        }
    }
    Ok(out)
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
/// Thumb lane: one worker per core, capped at [`MAX_THUMB_WORKERS`]. A
/// thumbnail is not "a few milliseconds" any more — a 30-cell effect sheet
/// is a 768x400 PNG that decodes, keys and cuts into 31 buffers, measured
/// at 30-50ms apiece — and filling a library is hundreds of them at once.
/// Four workers made the decode stage a queue; the machine has the cores,
/// so the lane takes them WHILE NOBODY IS ON STAGE. What keeps that from
/// stealing the picture during a set is not a small pool but
/// [`DecodePool::set_thumb_width`], which narrows how many of these workers
/// may decode AT ONCE the moment the program window opens or a deck plays.
///
/// Peak resident pixels stay bounded the same way (see the memory note on
/// `DecodePool`): the live cap is on active decodes, not on threads.
fn lane_sizes(cpus: usize) -> (usize, usize) {
    let heavy = cpus.clamp(2, 8);
    let thumb = cpus.clamp(2, MAX_THUMB_WORKERS);
    (heavy, thumb)
}

/// Ceiling on thumb-lane worker threads. Past this the decodes contend for
/// memory bandwidth instead of shortening the queue.
pub const MAX_THUMB_WORKERS: usize = 12;

/// Active thumb decodes allowed while a set is RUNNING — the program window
/// is up or a deck is playing. The grid keeps filling; it just stops taking
/// the cores the picture needs.
pub const THUMB_WIDTH_PERFORMING: usize = 2;

/// Bound on the thumb lane's pending stack: past this many queued-but-not-
/// started thumbnails, the OLDEST pending job (the one furthest from the
/// current view — it was requested longest ago) is dropped to make room.
/// Keeps a fast scroll from growing the backlog without limit.
const MAX_PENDING_THUMBS: usize = 64;

struct PendingThumb {
    revision: AssetRevisionId,
    source: DecodeSource,
    sheet: Option<(ThumbnailCells, f32)>,
    legacy_may_be_sheet: bool,
    epoch: u64,
}

struct ThumbQueueState {
    /// Push at the back, pop from the back: a stack, not a FIFO queue.
    stack: VecDeque<PendingThumb>,
    newest_epoch: u64,
    closed: bool,
    /// Decodes running right now, and how many may. The threads exist
    /// whatever the width is; narrowing just parks the surplus on the
    /// condvar, so widening again costs nothing.
    active: usize,
    width: usize,
    /// Jobs thrown away UNSTARTED, waiting to be reported so the caller can
    /// clear their in-flight marks. A silent drop is a blank tile forever.
    dropped: Vec<AssetRevisionId>,
}

/// LIFO job source shared by the thumb lane's workers. See `DecodePool`'s
/// doc comment for the full ordering/epoch/cap contract.
struct ThumbQueue {
    state: Mutex<ThumbQueueState>,
    cv: Condvar,
}

impl ThumbQueue {
    fn new(width: usize) -> ThumbQueue {
        ThumbQueue {
            state: Mutex::new(ThumbQueueState {
                stack: VecDeque::new(),
                newest_epoch: 0,
                closed: false,
                active: 0,
                width,
                dropped: Vec::new(),
            }),
            cv: Condvar::new(),
        }
    }

    /// How many workers may decode at once. Raising it wakes the parked
    /// ones; lowering it lets the ones already decoding finish.
    fn set_width(&self, width: usize) {
        let mut state = lock_from_ui(&self.state);
        let width = width.max(1);
        if state.width == width {
            return;
        }
        state.width = width;
        drop(state);
        self.cv.notify_all();
    }

    fn take_dropped(&self) -> Vec<AssetRevisionId> {
        let Ok(mut state) = self.state.try_lock() else {
            return Vec::new();
        };
        std::mem::take(&mut state.dropped)
    }

    /// One decode finished: free its slot and wake whoever is waiting.
    fn finished(&self) {
        let mut state = self.state.lock().unwrap();
        state.active = state.active.saturating_sub(1);
        drop(state);
        self.cv.notify_one();
    }

    fn push(&self, job: PendingThumb) {
        let mut state = lock_from_ui(&self.state);
        if job.epoch > state.newest_epoch {
            state.newest_epoch = job.epoch;
            // A new visible range makes every pending job for the old one
            // dead weight. Dropping them HERE rather than at pop keeps the
            // backlog honest: a fast scroll leaves no queue behind it.
            let newest = state.newest_epoch;
            let stale: Vec<AssetRevisionId> = state
                .stack
                .iter()
                .filter(|j| j.epoch < newest)
                .map(|j| j.revision)
                .collect();
            state.stack.retain(|j| j.epoch >= newest);
            state.dropped.extend(stale);
        }
        state.stack.push_back(job);
        while state.stack.len() > MAX_PENDING_THUMBS {
            // Drop the oldest pending job — and SAY SO.
            if let Some(job) = state.stack.pop_front() {
                state.dropped.push(job.revision);
            }
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
            while state.active < state.width {
                let Some(job) = state.stack.pop_back() else { break };
                if job.epoch >= state.newest_epoch {
                    state.active += 1;
                    return Some(job);
                }
                state.dropped.push(job.revision);
            }
            if state.closed {
                return None;
            }
            state = self.cv.wait(state).unwrap();
        }
    }

    fn close(&self) {
        let mut state = lock_from_ui(&self.state);
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
        DecodeJob::Deck { deck, gen, source, media } => {
            let result = decode_audio_source(&source, media, MAX_TRACK_FRAMES).map(|pcm| {
                let peaks = wave_peaks(&pcm, WAVE_COLS);
                (Arc::new(pcm), peaks)
            });
            DecodeDone::Deck { deck, gen, result }
        }
        DecodeJob::Pad { pad, gen, revision, source, media } => {
            let result = decode_audio_source(&source, media, MAX_PAD_FRAMES).map(Arc::new);
            DecodeDone::Pad { pad, gen, revision, result }
        }
        DecodeJob::Preview { gen, source, media } => {
            let result = decode_audio_source(&source, media, MAX_TRACK_FRAMES).map(|pcm| {
                let peaks = preview_wave_bins(&pcm, PREVIEW_WAVE_COLS);
                (Arc::new(pcm), peaks)
            });
            DecodeDone::Preview { gen, result }
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
    heavy_rx: Option<Receiver<DecodeJob>>,
    thumb_queue: Arc<ThumbQueue>,
    done_tx: Sender<DecodeDone>,
    rx: Receiver<DecodeDone>,
}

impl Default for DecodePool {
    fn default() -> Self {
        Self::new()
    }
}

impl DecodePool {
    pub fn new() -> DecodePool {
        let (heavy_tx, job_rx) = channel::<DecodeJob>();
        let (done_tx, rx) = channel::<DecodeDone>();
        DecodePool {
            heavy_tx,
            heavy_rx: Some(job_rx),
            thumb_queue: Arc::new(ThumbQueue::new(1)),
            done_tx,
            rx,
        }
    }

    /// Start the CPU lanes through Makepad's native/web-worker executor.
    /// Construction starts no OS primitive, so the web build never falls
    /// through `std::thread::spawn` and silently loses its decoder.
    pub fn start(&mut self, spawner: ThreadSpawner) {
        let Some(job_rx) = self.heavy_rx.take() else { return };
        let (heavy_workers, thumb_workers) = lane_sizes(spawner.available_parallelism().get());
        self.thumb_queue.set_width(thumb_workers);
        let job_rx = Arc::new(Mutex::new(job_rx));
        for i in 0..heavy_workers {
            let jobs = job_rx.clone();
            let done = self.done_tx.clone();
            match spawner.spawn(move || loop {
                    let job = {
                        let guard = jobs.lock().unwrap();
                        guard.recv()
                    };
                    let Ok(job) = job else { return };
                    let out = run_heavy_job(job);
                    if done.send(out).is_err() {
                        return;
                    }
                }) {
                Ok(handle) => handle.detach(),
                Err(error) => makepad_widgets::log!("vj decode worker {i} unavailable: {error}"),
            }
        }

        for i in 0..thumb_workers {
            let queue = self.thumb_queue.clone();
            let done = self.done_tx.clone();
            match spawner.spawn(move || loop {
                    let Some(job) = queue.pop() else { return };
                    let result = decode_thumb_source(&job.source, job.sheet, job.legacy_may_be_sheet);
                    queue.finished();
                    let out = DecodeDone::Thumb { revision: job.revision, result };
                    if done.send(out).is_err() {
                        return;
                    }
                }) {
                Ok(handle) => handle.detach(),
                Err(error) => makepad_widgets::log!("vj thumbnail worker {i} unavailable: {error}"),
            }
        }
    }

    pub fn submit(&self, job: DecodeJob) {
        match job {
            DecodeJob::Thumb { revision, source, sheet, legacy_may_be_sheet, epoch } => {
                self.thumb_queue
                    .push(PendingThumb { revision, source, sheet, legacy_may_be_sheet, epoch });
            }
            other => {
                let _ = self.heavy_tx.send(other);
            }
        }
    }

    /// Narrow or widen the thumb lane's ACTIVE decodes (never its threads).
    /// Politeness lever: the full lane while browsing, a sliver during a
    /// set. See [`THUMB_WIDTH_PERFORMING`].
    pub fn set_thumb_width(&self, width: usize) {
        self.thumb_queue.set_width(width);
    }

    pub fn poll(&self) -> Vec<DecodeDone> {
        let mut out: Vec<DecodeDone> = self
            .thumb_queue
            .take_dropped()
            .into_iter()
            .map(|revision| DecodeDone::ThumbDropped { revision })
            .collect();
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
            seam_wrap: AtomicUsize::new(0),
            seam_applied: AtomicUsize::new(0),
            muted: AtomicBool::new(true),
            scrub: AtomicBool::new(false),
            seek_100ns: AtomicI64::new(-1),
            trim_in_100ns: AtomicI64::new(0),
            trim_out_100ns: AtomicI64::new(i64::MAX),
            beat_transport: AtomicBool::new(false),
            beats_per_sweep: AtomicU8::new(4),
            beat_hint_100ns: AtomicI64::new(0),
            scratch_active: AtomicBool::new(false),
            scratch_rate_bits: AtomicU64::new(0f64.to_bits()),
            travel_forward: AtomicBool::new(true),
            repeat_cache: Mutex::new(RepeatCacheSlot::default()),
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
                Some(frame) => seen.push((frame.pts_100ns, identity_of(&frame.px.to_bgra()))),
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
                px: Pixels::Bgra(vec![0xff00_0000]),
            });
        }
        let mixer = Mixer::new();
        mixer.open_slot(SlotId::A);
        SlotPlayer {
            width: 1,
            height: 1,
            duration_secs: 1.0,
            has_audio: false,
            generation: 0,
            shared: Arc::new(SlotShared {
                stop: AtomicBool::new(false),
                paused: AtomicBool::new(paused),
                mode: AtomicU8::new(PlayMode::Once as u8),
                seam_wrap: AtomicUsize::new(0),
                seam_applied: AtomicUsize::new(0),
                muted: AtomicBool::new(false),
                scrub: AtomicBool::new(false),
                seek_100ns: AtomicI64::new(-1),
            trim_in_100ns: AtomicI64::new(0),
            trim_out_100ns: AtomicI64::new(i64::MAX),
            beat_transport: AtomicBool::new(false),
            beats_per_sweep: AtomicU8::new(4),
            beat_hint_100ns: AtomicI64::new(0),
            scratch_active: AtomicBool::new(false),
            scratch_rate_bits: AtomicU64::new(0f64.to_bits()),
            travel_forward: AtomicBool::new(true),
            repeat_cache: Mutex::new(RepeatCacheSlot::default()),
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
            px: Pixels::Bgra(vec![0xff00_0000]),
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

    /// THE SCROLL-HOT-PATH COST INSTRUMENT: one vjfx cache artifact
    /// (384x240, 30 all-intra H.264 frames, stamped layout — exactly what
    /// `fx_thumbs::encode_and_write` produces) decoded through the exact
    /// production entry (`decode_thumb`, the thumb-lane worker's call),
    /// timed. The atlas LRU re-decodes evicted sheets on scroll, so this
    /// number sizes the budget; run with `--nocapture` to read it. The
    /// assert is a sanity ceiling, not a benchmark gate.
    #[cfg(target_os = "macos")]
    #[test]
    fn vjfx_sheet_decode_wall_time_is_sane_and_reported() {
        use makepad_asset_importer::anim_icon;
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        let dir = test_dir("vjfx-decode-timing");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sheet.mp4");
        let (w, h, n) = (384usize, 240usize, 30usize);
        let mut enc = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: w as u32,
                height: h as u32,
                fps_num: 30,
                fps_den: 1,
                video_bitrate_bps: 2_000_000,
                audio: None,
                keyframe_only: true,
            },
        )
        .expect("hardware encoder");
        let mut rgba = vec![0u8; w * h * 4];
        for k in 0..n {
            for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
                let (x, y) = (i % w, i / w);
                px[0] = (x * 255 / w) as u8;
                px[1] = (y * 255 / h) as u8;
                px[2] = (k * 8) as u8;
                px[3] = 255;
            }
            enc.push_frame_rgba8(&rgba, None).unwrap();
        }
        enc.finish().unwrap();
        let cells = ThumbnailCells {
            cols: 6,
            cell_w: w as u32,
            cell_h: h as u32,
            first: 0,
            count: n as u32,
        };
        let bytes = std::fs::read(&path).unwrap();
        let bytes = anim_icon::stamp_layout_mp4(&bytes, cells, 30.0);
        std::fs::write(&path, &bytes).unwrap();

        // Warm once (first decoder session pays codec setup differently),
        // then time the steady-state cost a scroll re-decode pays.
        let warm = decode_thumb(&path, Some((cells, 30.0)), false).expect("decodes");
        assert_eq!(warm.frames.len(), n, "the full declared sheet");
        let runs = 5;
        let t0 = crate::clock::Instant::now();
        for _ in 0..runs {
            let p = decode_thumb(&path, Some((cells, 30.0)), false).unwrap();
            assert_eq!(p.frames.len(), n);
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!("vjfx sheet decode (384x240x30 all-intra, hw pull, thumb-lane path): {ms:.1} ms/sheet");
        // The split, so the LRU budget can be sized against the part that
        // scales: session open (codec setup, fixed) vs frame pull (hw
        // decode) vs conversion+placement (CPU, ours).
        let t_open = crate::clock::Instant::now();
        for _ in 0..runs {
            let d = VideoFileDecoder::open(path.to_str().unwrap()).unwrap();
            drop(d);
        }
        let open_ms = t_open.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        let t_pull = crate::clock::Instant::now();
        for _ in 0..runs {
            let mut d = VideoFileDecoder::open(path.to_str().unwrap()).unwrap();
            let mut got = 0;
            while let Ok(Some(_frame)) = d.next_frame() {
                got += 1;
            }
            assert_eq!(got, n);
        }
        let pull_ms = t_pull.elapsed().as_secs_f64() * 1000.0 / runs as f64 - open_ms;
        println!(
            "vjfx sheet decode split: open {open_ms:.1} ms, pull {pull_ms:.1} ms, convert+place {:.1} ms",
            (ms - open_ms - pull_ms).max(0.0)
        );
        assert!(ms < 2000.0, "pathological sheet decode time: {ms:.1}ms");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE DECLARED-COUNT LAW for video sheets (the fx_thumbs cache
    /// format): exactly `count` frames on `count` unique pts slots, or the
    /// decode is an ERROR — which is what fires the app's cache-removal /
    /// rebake path. A sheet with one rendered cell and twenty-nine holds
    /// used to pass the whole-sheet-black gate and cache forever.
    #[test]
    fn declared_video_sheets_demand_every_frame_on_its_own_pts_slot() {
        // 100ns pts steps at 30 fps.
        let step = 10_000_000i64 / 30;
        let frame = |i: i64| (i * step, vec![i as u32; 4]);
        // The complete sheet places, in slot order, even when the decoder
        // emitted it shuffled.
        let mut frames: Vec<(i64, Vec<u32>)> = (0..30).map(frame).collect();
        frames.swap(3, 27);
        frames.swap(0, 15);
        let placed = place_declared_video_frames(frames, 30, 30.0).expect("complete sheet");
        assert_eq!(placed.len(), 30);
        for (i, px) in placed.iter().enumerate() {
            assert_eq!(px[0], i as u32, "cell {i} must hold the frame its pts declares");
        }
        // Fewer frames than declared: ERROR, never a gap-hold.
        let partial: Vec<_> = (0..29).map(frame).collect();
        let err = place_declared_video_frames(partial, 30, 30.0).unwrap_err();
        assert!(err.contains("29 frames decoded, 30 declared"), "{err}");
        // One frame, twenty-nine missing — the exact half-black corruption
        // shape — is an error too.
        let one: Vec<_> = vec![frame(0)];
        assert!(place_declared_video_frames(one, 30, 30.0).is_err());
        // Two frames colliding on one slot: ERROR (a silent overwrite hid
        // a missing cell before).
        let mut collide: Vec<_> = (0..30).map(frame).collect();
        collide[5].0 = collide[4].0;
        let err = place_declared_video_frames(collide, 30, 30.0).unwrap_err();
        assert!(err.contains("two frames land in pts slot"), "{err}");
        // A frame whose pts lands past the declared range: ERROR.
        let mut outside: Vec<_> = (0..30).map(frame).collect();
        outside[29].0 = 40 * step;
        let err = place_declared_video_frames(outside, 30, 30.0).unwrap_err();
        assert!(err.contains("outside the 30 declared slots"), "{err}");
        // A one-cell declaration still works (single-frame still).
        let placed = place_declared_video_frames(vec![frame(0)], 1, 30.0).unwrap();
        assert_eq!(placed.len(), 1);
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
        pool.submit(DecodeJob::Deck {
            deck: DeckId::A,
            gen: 7,
            source: good.into(),
            media: MediaType::Wav,
        });
        pool.submit(DecodeJob::Pad {
            pad: PadKey::from_bytes([2; 16]),
            gen: 9,
            revision: AssetRevisionId::from_bytes([3; 32]),
            source: bad.into(),
            media: MediaType::Wav,
        });
        let deadline = crate::clock::Instant::now() + Duration::from_secs(10);
        let mut results = Vec::new();
        while results.len() < 2 && crate::clock::Instant::now() < deadline {
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
                DecodeDone::Preview { .. }
                | DecodeDone::MeshPrep { .. }
                | DecodeDone::SlotMesh { .. }
                | DecodeDone::Still { .. }
                | DecodeDone::Billboard { .. }
                | DecodeDone::FlowClip { .. }
                | DecodeDone::Thumb { .. }
                | DecodeDone::ThumbDropped { .. } => {
                    panic!("no mesh/flow/thumb job submitted")
                }
            }
        }
    }

    #[test]
    fn lane_sizes_scale_heavy_and_cap_thumb() {
        // 1 cpu: both lanes floor at their minimum (2 workers each).
        assert_eq!(lane_sizes(1), (2, 2));
        // 4 cpus: both lanes track the core count.
        assert_eq!(lane_sizes(4), (4, 4));
        // 32 cpus: heavy caps at 8; the thumb lane takes MAX_THUMB_WORKERS
        // threads — what keeps it from stealing a live set is the ACTIVE
        // width (`set_thumb_width`), not a small pool.
        assert_eq!(lane_sizes(32), (8, MAX_THUMB_WORKERS));
    }

    /// The thumb lane's politeness valve: narrowing parks the surplus
    /// workers, widening wakes them, and neither loses a job.
    #[test]
    fn thumb_width_bounds_active_decodes() {
        let queue = ThumbQueue::new(1);
        for i in 0..3u32 {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                source: PathBuf::from(format!("w{i}.png")).into(),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 0,
            });
        }
        // Width 1: one job out, and the next only after it finishes.
        let first = queue.pop().expect("first job");
        {
            let state = queue.state.lock().unwrap();
            assert_eq!(state.active, 1);
            assert_eq!(state.stack.len(), 2);
        }
        queue.finished();
        let second = queue.pop().expect("second job");
        assert_ne!(first.revision, second.revision);
        // Widening lets a third start while the second is still running.
        queue.set_width(4);
        let third = queue.pop().expect("third job");
        assert_ne!(second.revision, third.revision);
        let state = queue.state.lock().unwrap();
        assert_eq!(state.active, 2, "the second and third; the first reported finished");
        assert!(state.stack.is_empty());
    }

    /// A job the lane throws away UNSTARTED must be reported, or the tile
    /// that asked for it stays blank for the rest of the session.
    #[test]
    fn dropped_thumbs_are_reported() {
        let queue = ThumbQueue::new(4);
        let stale = AssetRevisionId::from_bytes([1u8; 32]);
        queue.push(PendingThumb {
            revision: stale,
            source: PathBuf::from("stale.png").into(),
            sheet: None,
            legacy_may_be_sheet: false,
            epoch: 1,
        });
        // A newer epoch retires the pending job for the old one.
        queue.push(PendingThumb {
            revision: AssetRevisionId::from_bytes([2u8; 32]),
            source: PathBuf::from("live.png").into(),
            sheet: None,
            legacy_may_be_sheet: false,
            epoch: 2,
        });
        assert_eq!(queue.take_dropped(), vec![stale]);
        assert!(queue.take_dropped().is_empty(), "reported once, not forever");

        // Overflow drops the OLDEST pending job — and says which.
        let queue = ThumbQueue::new(4);
        for i in 0..(MAX_PENDING_THUMBS + 2) {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                source: PathBuf::from(format!("o{i}.png")).into(),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 5,
            });
        }
        let dropped = queue.take_dropped();
        assert_eq!(dropped.len(), 2, "two over the cap, two reported");
        assert_eq!(dropped[0], AssetRevisionId::from_bytes([0u8; 32]));
    }

    #[test]
    fn thumb_queue_is_lifo_and_prunes_stale_and_bounds_pending() {
        // LIFO: with every job at the same epoch (none stale), the queue
        // must hand back the most recently pushed job first. Width is not
        // what is under test here, so it is wide enough to never bind (a
        // worker that never reports `finished` would otherwise fill it).
        let queue = ThumbQueue::new(64);
        for i in 0..10u32 {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                source: PathBuf::from(format!("t{i}.png")).into(),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 0,
            });
        }
        for expect in (0..10u32).rev() {
            let job = queue.pop().expect("job available");
            assert_eq!(
                job.source,
                DecodeSource::Path(PathBuf::from(format!("t{expect}.png"))),
                "must be newest-first"
            );
        }

        // Staleness: jobs stamped with an epoch older than the newest one
        // this queue has seen are skipped (dropped, not decoded).
        let queue = ThumbQueue::new(64);
        for i in 0..5u32 {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([i as u8; 32]),
                source: PathBuf::from(format!("old{i}.png")).into(),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 1,
            });
        }
        queue.push(PendingThumb {
            revision: AssetRevisionId::from_bytes([9; 32]),
            source: PathBuf::from("fresh.png").into(),
            sheet: None,
            legacy_may_be_sheet: false,
            epoch: 2,
        });
        let job = queue.pop().expect("the fresh-epoch job survives");
        assert_eq!(job.source, DecodeSource::Path(PathBuf::from("fresh.png")));
        assert!(
            queue.state.lock().unwrap().stack.is_empty(),
            "stale jobs must be dropped when popped, not left behind"
        );

        // Cap: pushing past MAX_PENDING_THUMBS drops the OLDEST pending job.
        let queue = ThumbQueue::new(64);
        for i in 0..(MAX_PENDING_THUMBS + 3) {
            queue.push(PendingThumb {
                revision: AssetRevisionId::from_bytes([0; 32]),
                source: PathBuf::from(format!("p{i}.png")).into(),
                sheet: None,
                legacy_may_be_sheet: false,
                epoch: 0,
            });
        }
        let remaining = queue.state.lock().unwrap();
        assert_eq!(remaining.stack.len(), MAX_PENDING_THUMBS);
        assert_eq!(
            remaining.stack.front().unwrap().source,
            DecodeSource::Path(PathBuf::from("p3.png")),
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
                source: path.into(),
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
                source: path.into(),
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

    /// A 32-frame keyframe-only clip whose frames carry their index as a
    /// 5-bit bar pattern (robust through the codec), plus the reader.
    fn encode_identity_clip(dir: &PathBuf) -> PathBuf {
        use makepad_widgets::makepad_platform::video_file::{
            VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
        };
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("clip.mp4");
        let mut encoder = VideoFileEncoder::new(
            path.to_str().unwrap(),
            VideoFileEncoderOptions {
                codec: VideoFileCodec::H264,
                width: ID_W,
                height: ID_H,
                fps_num: ID_FPS,
                fps_den: 1,
                video_bitrate_bps: 4_000_000,
                audio: None,
                keyframe_only: true,
            },
        )
        .expect("encoder");
        for index in 0..ID_FRAMES {
            encoder.push_frame_rgb8(&identity_frame_rgb8(index), None).expect("push");
        }
        encoder.finish().expect("finish");
        path
    }

    const ID_W: u32 = 128;
    const ID_H: u32 = 64;
    const ID_FPS: u32 = 24;
    const ID_FRAMES: usize = 32;
    const ID_BITS: usize = 5;
    const ID_BLOCK_W: usize = ID_W as usize / ID_BITS;

    fn identity_frame_rgb8(index: usize) -> Vec<u8> {
        let mut out = vec![40u8; ID_W as usize * ID_H as usize * 3];
        for y in 0..ID_H as usize / 2 {
            for x in 0..ID_W as usize {
                let bit = (x / ID_BLOCK_W).min(ID_BITS - 1);
                let luma = if index >> bit & 1 == 1 { 235 } else { 16 };
                let at = (y * ID_W as usize + x) * 3;
                out[at] = luma;
                out[at + 1] = luma;
                out[at + 2] = luma;
            }
        }
        out
    }

    fn identity_of(bgra: &[u32]) -> usize {
        let mut index = 0;
        for bit in 0..ID_BITS {
            let x0 = bit * ID_BLOCK_W + ID_BLOCK_W / 4;
            let x1 = bit * ID_BLOCK_W + ID_BLOCK_W * 3 / 4;
            let mut sum = 0u32;
            let mut count = 0u32;
            for y in 4..ID_H as usize / 4 {
                for x in x0..x1 {
                    sum += (bgra[y * ID_W as usize + x] >> 16) & 0xff;
                    count += 1;
                }
            }
            if sum / count.max(1) > 128 {
                index |= 1 << bit;
            }
        }
        index
    }

    /// Poll the ring for `dur`, returning every presented identity.
    fn presented_for(player: &mut SlotPlayer, dur: Duration) -> Vec<usize> {
        let mut ids = Vec::new();
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            if let Some(px) = player.take_due_frame() {
                ids.push(identity_of(&px.to_bgra()));
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        ids
    }

    /// Wait until the window is resident (or give up after `dur`).
    fn wait_resident(player: &mut SlotPlayer, dur: Duration) -> Option<Arc<Vec<Frame>>> {
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            if let Some(frames) = player.resident_frames() {
                return Some(frames);
            }
            let _ = player.take_due_frame();
            std::thread::sleep(Duration::from_millis(3));
        }
        None
    }

    /// THE RESIDENT LAWS, end to end through the real SlotPlayer. A muted
    /// looping clip becomes RESIDENT (its whole window cached, served by
    /// the presenter's platter — transport.rs) and the decode thread
    /// PARKS: the ring goes quiet and stays quiet across mode flips and a
    /// seek. A TRIM is a new window: the cache is dropped, the decoder
    /// streams the new window — every frame inside it — until it is
    /// resident again. Unmuting a clip without a soundtrack changes
    /// nothing; ONCE has no repeat, so the decoder un-parks and plays out.
    #[test]
    fn a_muted_loop_goes_resident_and_parks_the_decoder() {
        let dir = test_dir("resident-park");
        let path = encode_identity_clip(&dir);
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

        // Resident: the whole window, in order.
        let frames = wait_resident(&mut player, Duration::from_secs(4)).expect("went resident");
        assert_eq!(frames.len(), ID_FRAMES, "the cache is the whole window");
        assert!(frames.windows(2).all(|w| w[1].pts_100ns > w[0].pts_100ns));
        // Parked: whatever the first pass queued drains, then nothing.
        let _ = presented_for(&mut player, Duration::from_millis(600));
        let quiet = presented_for(&mut player, Duration::from_millis(800));
        assert!(quiet.is_empty(), "the ring kept flowing while resident: {quiet:?}");
        assert!(player.needs_frame_pump(), "a parked resident clip still wants the display pump");

        // Mode flips: still resident, still parked (the map is the platter's).
        for mode in [PlayMode::PingPong, PlayMode::Reverse, PlayMode::Loop] {
            player.set_mode(mode);
            std::thread::sleep(Duration::from_millis(250));
            assert!(player.resident_frames().is_some(), "{mode:?} dropped the cache");
            let leak = presented_for(&mut player, Duration::from_millis(400));
            assert!(leak.is_empty(), "{mode:?} un-parked the ring: {leak:?}");
        }

        // A seek: the readout moves, the cache stays whole (never a
        // tail-only cache built from the scrub target), the ring settles.
        player.seek_fraction(0.6);
        assert!((player.position_secs() - 0.6 * player.duration_secs).abs() < 0.05);
        std::thread::sleep(Duration::from_millis(600));
        let _ = presented_for(&mut player, Duration::from_millis(400));
        let after = player.resident_frames().expect("a seek does not drop the cache");
        assert_eq!(after.len(), ID_FRAMES);
        // The head is the WINDOW's head, not the scrub target's: a cache
        // rebuilt from a seek at 0.6 would start near 0.8s, two orders of
        // magnitude away from this bound. The slack is one frame because
        // Media Foundation's own encode/decode round trip does not hand
        // back a zero-based origin — it puts the first sample one frame
        // duration in, and makepad reports its timestamps as given.
        let head_slack = 2 * 10_000_000 / ID_FPS as i64;
        assert!(after[0].pts_100ns < head_slack, "cache lost its head");
        let leak = presented_for(&mut player, Duration::from_millis(400));
        assert!(leak.is_empty(), "the ring kept flowing after a seek: {leak:?}");

        // Unmute without a soundtrack: silent by law, nothing changes.
        player.set_muted(false);
        std::thread::sleep(Duration::from_millis(250));
        assert!(player.resident_frames().is_some());
        player.set_muted(true);

        // TRIM to [8, 24): a new window. The old cache goes, the decoder
        // streams the window until it is resident again.
        player.set_trim(0.25, 0.75);
        let mut streamed: Vec<usize> = Vec::new();
        let mut rebuilt = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(px) = player.take_due_frame() {
                streamed.push(identity_of(&px.to_bgra()));
            }
            if let Some(frames) = player.resident_frames() {
                if frames.len() < ID_FRAMES {
                    rebuilt = Some(frames);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        let rebuilt = rebuilt.expect("the trimmed window never went resident");
        assert!(
            (15..=17).contains(&rebuilt.len()),
            "trimmed cache holds {} frames, expected ~16",
            rebuilt.len()
        );
        let lo = ID_FRAMES / 4;
        let hi = ID_FRAMES * 3 / 4;
        for id in &streamed {
            assert!(
                *id + 1 >= lo && *id < hi + 1,
                "a streamed frame escaped the trim window: {id} not in [{lo}, {hi}) — {streamed:?}"
            );
        }

        // ONCE: no repeat to serve; the decoder un-parks and plays out.
        player.set_mode(PlayMode::Once);
        player.seek_fraction(0.3);
        let played = presented_for(&mut player, Duration::from_secs(2));
        assert!(!played.is_empty(), "Once never streamed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
