//! "Split audio layers": the stems + lyrics analysis bake, and the fetch of
//! what an already-analysed track carries.
//!
//! One audio asset in the store, four separated Ogg stems and (optionally) a
//! word-aligned lyrics document back onto it as typed side-channel files.
//! The heavy half runs ONCE per track, wherever it happens first, and every
//! other client — a VJ deck, this app's own preview — then gets the fancy
//! behaviour by FETCHING instead of by spending half a minute of GPU.
//!
//! Two lanes, two threads, both of which exit when their request channel
//! disconnects (the app dropping [`AnalysisQueue`] is the shutdown):
//!
//! * **the bake lane** (`asset-ui-stem-bake`) — strictly serial, because
//!   separation owns the GPU: resolve → fetch → decode → separate → encode →
//!   transcribe → publish, per track, with the model and the whisper
//!   checkpoint loaded ONCE and kept resident for the whole session (loading
//!   either costs seconds, and a queue of 200 tracks must pay that once).
//! * **the fetch lane** (`asset-ui-stem-fetch`) — light I/O: which
//!   side-channels does this asset's head revision carry, and their bytes,
//!   decoded into what the mixer and the lyrics reader consume.
//!
//! Nothing here is automatic. The bake is only ever started by an explicit
//! user action (the Load card's "split audio layers" checkbox, the Library
//! rail's Analyse button, or the bulk "Analyse N shown"), because it is
//! minutes of GPU time per track.
//!
//! ## Digest parity with the VJ
//!
//! The lyrics document is keyed by [`makepad_audio_lyrics::track_digest`]
//! over the DECODED track as stereo i16 at its own rate — the same key the
//! VJ's stem cache and lyrics cache use. That parity only holds if the
//! decode and the i16 conversion match, so [`decode_track`] mirrors
//! `apps/vj/src/media.rs::decode_audio_clip` exactly: PCM16 WAV keeps its
//! raw samples, float WAV/MP3/Ogg go through `(v.clamp(-1,1) * 32767.0) as
//! i16`, and the frame is `[first channel, last channel]` at the file's own
//! sample rate.

use crate::import::ServerSession;
use makepad_ai_stems::{Demixer, StemSet, StemsModel, StereoBuf, SAMPLE_RATE as STEMS_RATE};
use makepad_asset_client::{AssetClient, ClientConfig};
use makepad_asset_data::{AssetAlias, AssetFile, AssetId, AssetManifest, FileRole};
use makepad_asset_widgets::lyric_reader::{lyric_stamp, LyricRow};
use makepad_audio_lyrics::bake::LyricsBaker;
use makepad_audio_lyrics::TrackLyrics;
use makepad_audio_sidechannels::{encode_stem_oggs, publish_side_channels};
use makepad_widgets::log;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// The whisper checkpoint the lyrics half wants. MIT weights, no gate.
const WHISPER_MODEL_FILE: &str = "ggml-large-v3-turbo.bin";

/// Language the transcript is baked in. The VJ's bake is English-only too;
/// a per-track language pick is a later knob, not a silent guess.
const BAKE_LANGUAGE: &str = "en";

// ---------------------------------------------------------------------------
// where the models live
// ---------------------------------------------------------------------------

/// The separation checkpoint. `ASSET_UI_STEMS_CKPT` wins, then the VJ's own
/// `VJ_STEMS_CKPT` (one machine, one copy of a 527 MB checkpoint), then the
/// checkout's reference copy.
pub fn stems_checkpoint_path() -> PathBuf {
    for key in ["ASSET_UI_STEMS_CKPT", "VJ_STEMS_CKPT"] {
        if let Ok(path) = std::env::var(key) {
            if !path.trim().is_empty() {
                return PathBuf::from(path);
            }
        }
    }
    checkout_root()
        .join("local/stems_ref/ckpt")
        .join(makepad_ai_stems::MODEL_CHECKPOINT)
}

/// The whisper checkpoint, if this machine has one — the SAME search the VJ
/// does (`apps/vj/src/lyrics.rs::whisper_model_path`), so both apps find the
/// one file.
pub fn whisper_model_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MAKEPAD_VOICE_MODEL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let root = checkout_root();
    [
        PathBuf::from(WHISPER_MODEL_FILE),
        root.join(WHISPER_MODEL_FILE),
        root.join("local").join(WHISPER_MODEL_FILE),
        root.join("local/models").join(WHISPER_MODEL_FILE),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn checkout_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Provenance for anything that surfaces where a separation came from.
pub fn model_provenance() -> String {
    format!(
        "{} · {} · {}",
        makepad_ai_stems::MODEL_ID,
        makepad_ai_stems::MODEL_CHECKPOINT,
        makepad_ai_stems::MODEL_LICENSE
    )
}

// ---------------------------------------------------------------------------
// decoded track audio: the one shape the digest is defined over
// ---------------------------------------------------------------------------

/// A whole decoded track as stereo i16 at its OWN rate. This exact shape is
/// what [`makepad_audio_lyrics::track_digest`] hashes, so it is also the
/// identity a VJ deck computes for the same file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackPcm {
    pub frames: Vec<[i16; 2]>,
    pub sample_rate: u32,
}

impl TrackPcm {
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames.len() as f64 / self.sample_rate as f64
    }

    pub fn digest(&self) -> String {
        makepad_audio_lyrics::track_digest(self.sample_rate, &self.frames)
    }
}

/// The ONE float→i16 conversion, shared by every path here. Identical to the
/// VJ's (`(v.clamp(-1,1) * 32767.0) as i16`) — a different rounding rule
/// would silently produce a different track digest for the same song.
pub fn i16_sample(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Interleaved float frames → the digest's stereo i16 frames. Mono is
/// duplicated (first channel, last channel), which is what the VJ does.
pub fn i16_frames(interleaved: &[f32], channels: usize) -> Vec<[i16; 2]> {
    let channels = channels.max(1);
    interleaved
        .chunks_exact(channels)
        .map(|frame| [i16_sample(frame[0]), i16_sample(frame[channels - 1])])
        .collect()
}

/// Decode a track's bytes into the digest's shape.
///
/// RIFF is parsed here so PCM16 keeps its RAW samples (a float round-trip
/// would move the digest off the VJ's); MP3 and Ogg go through the repo's
/// own decoder, the same one the preview draws with.
pub fn decode_track(bytes: &[u8]) -> Result<TrackPcm, String> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        if let Some(pcm) = wav_i16_frames(bytes) {
            return Ok(pcm);
        }
        // 8-bit and other exotic WAVs: the app's own parser, then the shared
        // conversion. No VJ counterpart exists to be parity with — that path
        // refuses these files outright.
        let wav = crate::audio::parse_wav(bytes)?;
        let frames = wav
            .frames
            .iter()
            .map(|(l, r)| [i16_sample(*l), i16_sample(*r)])
            .collect();
        return Ok(TrackPcm {
            frames,
            sample_rate: wav.sample_rate.max(1),
        });
    }
    let decoded = makepad_audio_decode::decode_any(bytes).map_err(|e| format!("{e:?}"))?;
    let frames = i16_frames(&decoded.pcm_interleaved_f32, decoded.channels as usize);
    if frames.is_empty() {
        return Err("decoded to zero frames".into());
    }
    Ok(TrackPcm {
        frames,
        sample_rate: decoded.rate.max(1),
    })
}

/// RIFF/WAVE straight to i16 frames, bit-identical to the VJ's parser for
/// the two formats a music library actually holds: PCM16 (raw) and float32
/// (clamped and scaled). `None` for anything else, so the caller can fall
/// back rather than guess.
fn wav_i16_frames(bytes: &[u8]) -> Option<TrackPcm> {
    let mut format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body_end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..body_end];
        match id {
            b"fmt " if body.len() >= 16 => {
                format = u16::from_le_bytes(body[0..2].try_into().ok()?);
                channels = u16::from_le_bytes(body[2..4].try_into().ok()?);
                sample_rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
                bits = u16::from_le_bytes(body[14..16].try_into().ok()?);
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = body_end + (size & 1);
    }
    let data = data?;
    if channels == 0 || sample_rate == 0 {
        return None;
    }
    let ch = channels as usize;
    let frames: Vec<[i16; 2]> = match (format, bits) {
        (1, 16) => data
            .chunks_exact(2 * ch)
            .map(|frame| {
                let sample = |i: usize| {
                    i16::from_le_bytes([frame[i * 2], frame[i * 2 + 1]])
                };
                [sample(0), sample(ch - 1)]
            })
            .collect(),
        (3, 32) => data
            .chunks_exact(4 * ch)
            .map(|frame| {
                let sample = |i: usize| {
                    i16_sample(f32::from_le_bytes([
                        frame[i * 4],
                        frame[i * 4 + 1],
                        frame[i * 4 + 2],
                        frame[i * 4 + 3],
                    ]))
                };
                [sample(0), sample(ch - 1)]
            })
            .collect(),
        _ => return None,
    };
    if frames.is_empty() {
        return None;
    }
    Some(TrackPcm { frames, sample_rate })
}

/// The model's input: stereo float at 44.1 kHz. Tracks at another rate are
/// resampled with the shared zero-group-delay polyphase kernel (the same one
/// the lyrics aligner mixes down with) — a cheap linear resample here would
/// put its own artifacts under the separation.
pub fn to_stereo_44k(pcm: &TrackPcm) -> StereoBuf {
    let mut left = Vec::with_capacity(pcm.frames.len());
    let mut right = Vec::with_capacity(pcm.frames.len());
    for frame in &pcm.frames {
        left.push(frame[0] as f32 / 32768.0);
        right.push(frame[1] as f32 / 32768.0);
    }
    let rate = pcm.sample_rate.max(1) as f64;
    let target = STEMS_RATE as f64;
    if (rate - target).abs() < 0.5 {
        return StereoBuf { left, right };
    }
    StereoBuf {
        left: makepad_audio_lyrics::align::resample(&left, rate, target),
        right: makepad_audio_lyrics::align::resample(&right, rate, target),
    }
}

// ---------------------------------------------------------------------------
// what a revision already carries
// ---------------------------------------------------------------------------

/// Which side-channels the head revision holds. Stems are all-four-or-none
/// by contract, so one flag is honest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideChannels {
    pub stems: bool,
    pub lyrics: bool,
}

impl SideChannels {
    pub fn of(files: &[AssetFile]) -> SideChannels {
        SideChannels {
            stems: FileRole::STEMS
                .iter()
                .all(|role| files.iter().any(|file| file.role == *role)),
            lyrics: files.iter().any(|file| file.role == FileRole::Lyrics),
        }
    }

    pub fn any(&self) -> bool {
        self.stems || self.lyrics
    }
}

/// What one bake still has to produce. Mirrors the publish's own idempotency
/// (`publish_side_channels` reports `AlreadyPresent` for roles the head
/// already carries) so the queue can skip a track without touching the GPU.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeNeed {
    pub stems: bool,
    pub lyrics: bool,
}

impl BakeNeed {
    /// `present` is what the head revision has; `want_lyrics` is what the
    /// user asked for. Stems are always wanted — they are the analysis.
    pub fn decide(present: SideChannels, want_lyrics: bool) -> BakeNeed {
        BakeNeed {
            stems: !present.stems,
            lyrics: want_lyrics && !present.lyrics,
        }
    }

    /// Nothing to do — and the ONE gate the bake takes: anything else
    /// separates, because the lyrics half reads the vocals stem, so a track
    /// that only lacks lyrics still has to be separated (its stems simply
    /// are not republished).
    pub fn nothing(&self) -> bool {
        !self.stems && !self.lyrics
    }
}

// ---------------------------------------------------------------------------
// the bake lane
// ---------------------------------------------------------------------------

/// Which asset a queued bake is for. The Load card knows its tracks by
/// ALIAS (the music importer reports aliases), the Library knows them by id;
/// the worker resolves either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BakeTarget {
    Asset(AssetId),
    Alias(String),
}

impl BakeTarget {
    /// Stable dedupe key, so the same track cannot sit in the queue twice.
    pub fn key(&self) -> String {
        match self {
            BakeTarget::Asset(id) => id.to_string(),
            BakeTarget::Alias(alias) => format!("alias:{alias}"),
        }
    }
}

pub struct BakeRequest {
    pub id: u64,
    /// Which batch this belongs to; a Stop bumps the counter and every
    /// request older than it is dropped unopened.
    pub batch: u64,
    pub target: BakeTarget,
    pub title: String,
    pub lyrics: bool,
    pub session: ServerSession,
}

/// Where one track is in the bake. `Separating` is the only long stage and
/// the only one with a fraction worth drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakeStage {
    Waiting,
    Resolving,
    Fetching,
    Decoding,
    Loading,
    Separating { done: usize, total: usize },
    Encoding,
    Transcribing,
    Publishing,
}

impl BakeStage {
    pub fn label(&self) -> String {
        match self {
            BakeStage::Waiting => "waiting".into(),
            BakeStage::Resolving => "resolving".into(),
            BakeStage::Fetching => "fetching".into(),
            BakeStage::Decoding => "decoding".into(),
            BakeStage::Loading => "loading the model".into(),
            BakeStage::Separating { done, total } => {
                if *total == 0 {
                    "separating".into()
                } else {
                    format!("separating {}%", (100 * done / total).min(100))
                }
            }
            BakeStage::Encoding => "encoding stems".into(),
            BakeStage::Transcribing => "transcribing".into(),
            BakeStage::Publishing => "publishing".into(),
        }
    }

    /// Monotonic 0..1 across the whole bake, in honest bands: separation
    /// owns the middle 70% because it owns the minutes.
    pub fn fraction(&self) -> f32 {
        match self {
            BakeStage::Waiting => 0.0,
            BakeStage::Resolving => 0.02,
            BakeStage::Fetching => 0.05,
            BakeStage::Decoding => 0.08,
            BakeStage::Loading => 0.10,
            BakeStage::Separating { done, total } => {
                if *total == 0 {
                    0.10
                } else {
                    0.10 + 0.70 * (*done as f32 / *total as f32).min(1.0)
                }
            }
            BakeStage::Encoding => 0.85,
            BakeStage::Transcribing => 0.90,
            BakeStage::Publishing => 0.97,
        }
    }
}

/// What one finished bake did.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeDone {
    pub asset: AssetId,
    pub stems: bool,
    pub lyrics: bool,
    /// Nothing was written: the head already carried everything asked for.
    pub already: bool,
    /// Lyrics were asked for and the vocals stem held nothing sung. A real
    /// answer, not a failure — and NOT the same thing as "already analysed",
    /// which is what a bare `already` would have read as.
    pub no_vocals: bool,
    pub secs: f64,
    pub lines: usize,
}

impl BakeDone {
    pub fn summary(&self) -> String {
        if self.already && !self.no_vocals {
            return "already analysed".into();
        }
        let mut parts = Vec::new();
        if self.stems {
            parts.push("4 stems".to_string());
        }
        if self.lyrics {
            parts.push(format!("{} lyric lines", self.lines));
        }
        if self.no_vocals {
            parts.push("nothing sung in the vocals stem".to_string());
        }
        if parts.is_empty() {
            parts.push("nothing to publish".to_string());
        }
        format!("{} · {:.0}s", parts.join(" + "), self.secs)
    }
}

pub enum BakeMsg {
    Stage { id: u64, stage: BakeStage },
    Done { id: u64, result: Result<BakeDone, String> },
}

// ---------------------------------------------------------------------------
// the fetch lane
// ---------------------------------------------------------------------------

pub struct FetchRequest {
    pub asset: AssetId,
    /// Latest-selection-wins: a result for an older generation is dropped
    /// rather than shown over the track the user is now looking at.
    pub generation: u64,
    pub session: ServerSession,
    /// Fetch the payload bytes too, not just the roles. False for the cheap
    /// "does this asset have stems" probe the Analyse button needs.
    pub want_content: bool,
}

pub enum FetchMsg {
    /// What the head revision carries — the first thing back, always.
    Roles {
        asset: AssetId,
        generation: u64,
        roles: SideChannels,
    },
    Stems {
        asset: AssetId,
        generation: u64,
        lanes: Box<[crate::audio::StemPcm; 4]>,
    },
    Lyrics {
        asset: AssetId,
        generation: u64,
        rows: Vec<LyricRow>,
    },
    Failed {
        asset: AssetId,
        generation: u64,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// the UI-side handle
// ---------------------------------------------------------------------------

/// One track the user asked for, as the UI sees it.
pub struct AnalysisRow {
    pub id: u64,
    pub key: String,
    pub title: String,
    pub lyrics: bool,
    pub stage: BakeStage,
}

/// The app's handle on both lanes: enqueue, drain, and one honest line
/// describing what the queue is doing.
pub struct AnalysisQueue {
    bake_tx: Sender<BakeRequest>,
    bake_rx: Receiver<BakeMsg>,
    fetch_tx: Sender<FetchRequest>,
    fetch_rx: Receiver<FetchMsg>,
    /// Bumped by [`AnalysisQueue::stop`] and by Drop; the worker abandons
    /// anything older.
    batch: Arc<AtomicU64>,
    next_id: u64,
    /// Queued + running, oldest first. The head is the one on the GPU.
    pub rows: Vec<AnalysisRow>,
    pub queued_keys: HashSet<String>,
    /// Finished in this batch, and how many were asked for.
    pub done: usize,
    pub total: usize,
    pub failed: usize,
    /// The last verdict, kept after the queue empties.
    pub summary: String,
    fetch_generation: u64,
}

impl Default for AnalysisQueue {
    fn default() -> Self {
        AnalysisQueue::start()
    }
}

impl AnalysisQueue {
    pub fn start() -> AnalysisQueue {
        let (bake_tx, bake_requests) = channel::<BakeRequest>();
        let (bake_done, bake_rx) = channel::<BakeMsg>();
        let (fetch_tx, fetch_requests) = channel::<FetchRequest>();
        let (fetch_done, fetch_rx) = channel::<FetchMsg>();
        let batch = Arc::new(AtomicU64::new(1));
        let worker_batch = Arc::clone(&batch);
        // A failed spawn is not fatal: the queue simply never runs, and
        // every enqueue reports it instead of pretending to work.
        let _ = std::thread::Builder::new()
            .name("asset-ui-stem-bake".into())
            .spawn(move || bake_loop(bake_requests, bake_done, worker_batch));
        let _ = std::thread::Builder::new()
            .name("asset-ui-stem-fetch".into())
            .spawn(move || fetch_loop(fetch_requests, fetch_done));
        AnalysisQueue {
            bake_tx,
            bake_rx,
            fetch_tx,
            fetch_rx,
            batch,
            next_id: 0,
            rows: Vec::new(),
            queued_keys: HashSet::new(),
            done: 0,
            total: 0,
            failed: 0,
            summary: String::new(),
            fetch_generation: 0,
        }
    }

    pub fn busy(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Enqueue one track. Returns false when it is already in the queue.
    pub fn enqueue(
        &mut self,
        target: BakeTarget,
        title: impl Into<String>,
        lyrics: bool,
        session: ServerSession,
    ) -> bool {
        let key = target.key();
        if !self.queued_keys.insert(key.clone()) {
            return false;
        }
        // A new batch after the last one drained: counters start over so the
        // "2/7" the user reads is about the run they just started.
        if self.rows.is_empty() {
            self.done = 0;
            self.total = 0;
            self.failed = 0;
        }
        self.next_id += 1;
        let id = self.next_id;
        let title = title.into();
        self.rows.push(AnalysisRow {
            id,
            key,
            title: title.clone(),
            lyrics,
            stage: BakeStage::Waiting,
        });
        self.total += 1;
        let batch = self.batch.load(Ordering::Acquire);
        if self
            .bake_tx
            .send(BakeRequest {
                id,
                batch,
                target,
                title,
                lyrics,
                session,
            })
            .is_err()
        {
            self.summary = "the analysis worker is not running".into();
            if let Some(at) = self.rows.iter().position(|row| row.id == id) {
                let row = self.rows.remove(at);
                self.queued_keys.remove(&row.key);
            }
            self.total = self.total.saturating_sub(1);
            return false;
        }
        true
    }

    /// Abandon everything queued and the job in flight (between spans).
    pub fn stop(&mut self) {
        self.batch.fetch_add(1, Ordering::AcqRel);
        let dropped = self.rows.len();
        self.rows.clear();
        self.queued_keys.clear();
        if dropped > 0 {
            self.summary = format!("stopped · {} left unanalysed", dropped);
        }
    }

    /// Ask the fetch lane what an asset carries (and, with `want_content`,
    /// for the bytes). Every call bumps the generation, so only the newest
    /// answer is accepted.
    pub fn request_fetch(&mut self, asset: AssetId, session: ServerSession, want_content: bool) -> u64 {
        self.fetch_generation += 1;
        let generation = self.fetch_generation;
        let _ = self.fetch_tx.send(FetchRequest {
            asset,
            generation,
            session,
            want_content,
        });
        generation
    }

    pub fn fetch_generation(&self) -> u64 {
        self.fetch_generation
    }

    /// Everything the fetch lane finished since the last drain.
    pub fn drain_fetch(&mut self) -> Vec<FetchMsg> {
        self.fetch_rx.try_iter().collect()
    }

    /// Fold the bake lane's news into the rows. Returns the finished jobs
    /// (for the log) and whether anything changed.
    pub fn drain_bake(&mut self) -> (bool, Vec<(String, Result<BakeDone, String>)>) {
        let mut changed = false;
        let mut finished = Vec::new();
        for msg in self.bake_rx.try_iter().collect::<Vec<_>>() {
            changed = true;
            match msg {
                BakeMsg::Stage { id, stage } => {
                    if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
                        row.stage = stage;
                    }
                }
                BakeMsg::Done { id, result } => {
                    let Some(at) = self.rows.iter().position(|row| row.id == id) else {
                        continue;
                    };
                    let row = self.rows.remove(at);
                    self.queued_keys.remove(&row.key);
                    self.done += 1;
                    if result.is_err() {
                        self.failed += 1;
                    }
                    self.summary = match &result {
                        Ok(done) => format!("{} · {}", row.title, done.summary()),
                        Err(error) => format!("{} · failed: {error}", row.title),
                    };
                    finished.push((row.title, result));
                }
            }
        }
        if changed && self.rows.is_empty() && self.total > 0 {
            self.summary = format!(
                "analysed {}/{}{}",
                self.total - self.failed,
                self.total,
                if self.failed > 0 {
                    format!(" · {} failed", self.failed)
                } else {
                    String::new()
                }
            );
        }
        (changed, finished)
    }

    /// The one line every surface shows: what is happening, on which track,
    /// and how far into the batch.
    pub fn status_line(&self) -> String {
        let Some(row) = self.rows.first() else {
            return self.summary.clone();
        };
        format!(
            "analysing {}/{} · {} · {}",
            (self.done + 1).min(self.total.max(1)),
            self.total,
            row.stage.label(),
            row.title
        )
    }

    /// 0..1 across the whole batch, so one bar can stand for the run.
    pub fn progress_fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let inside = self.rows.first().map_or(0.0, |row| row.stage.fraction());
        ((self.done as f32 + inside) / self.total as f32).clamp(0.0, 1.0)
    }
}

impl Drop for AnalysisQueue {
    fn drop(&mut self) {
        // Both loops end when their channel disconnects; the batch bump
        // stops the job already on the GPU at the next span boundary.
        self.batch.fetch_add(1, Ordering::AcqRel);
    }
}

// ---------------------------------------------------------------------------
// the workers
// ---------------------------------------------------------------------------

/// One connected client per session, reused across jobs: against the local
/// server a fresh connect costs more than the request.
struct ClientLane {
    session: Option<ServerSession>,
    client: Option<AssetClient>,
    cache: PathBuf,
}

impl ClientLane {
    fn new(cache: &str) -> ClientLane {
        ClientLane {
            session: None,
            client: None,
            cache: crate::asset_store_state::asset_ui_home().join(cache),
        }
    }

    fn get(&mut self, session: &ServerSession) -> Result<&mut AssetClient, String> {
        let same = self
            .session
            .as_ref()
            .is_some_and(|held| held.endpoints == session.endpoints && held.token == session.token);
        if !same || self.client.is_none() {
            let mut config = ClientConfig::new(self.cache.clone());
            config.token = Some(session.token.clone());
            let client = AssetClient::connect(config, session.endpoints, Some(session.server_id))
                .map_err(|error| format!("asset client: {error}"))?;
            self.client = Some(client);
            self.session = Some(session.clone());
        }
        Ok(self.client.as_mut().expect("connected client"))
    }
}

fn head_manifest(
    client: &mut AssetClient,
    asset: &AssetId,
) -> Result<AssetManifest, String> {
    let detail = client
        .asset_detail(asset)
        .map_err(|e| format!("asset detail: {e}"))?;
    let head = detail
        .latest_published()
        .ok_or("asset has no published revision")?;
    client
        .fetch_asset_manifest(&head.revision)
        .map_err(|e| format!("revision manifest: {e}"))
}

fn bake_loop(rx: Receiver<BakeRequest>, tx: Sender<BakeMsg>, batch: Arc<AtomicU64>) {
    let mut lane = ClientLane::new("analysis-cache");
    // Loaded once, kept for the session: both are seconds of load time and
    // hundreds of megabytes, and a queue of tracks must pay that once.
    let mut model: Option<StemsModel> = None;
    let mut baker: Option<LyricsBaker> = None;
    while let Ok(request) = rx.recv() {
        let id = request.id;
        if request.batch < batch.load(Ordering::Acquire) {
            // Stopped before it started: report it so the row cannot hang.
            let _ = tx.send(BakeMsg::Done {
                id,
                result: Err("stopped".into()),
            });
            SignalToUI::set_ui_signal();
            continue;
        }
        let result = run_bake(&request, &mut lane, &mut model, &mut baker, &tx, &batch);
        if tx.send(BakeMsg::Done { id, result }).is_err() {
            return;
        }
        SignalToUI::set_ui_signal();
    }
}

fn report(tx: &Sender<BakeMsg>, id: u64, stage: BakeStage) {
    let _ = tx.send(BakeMsg::Stage { id, stage });
    SignalToUI::set_ui_signal();
}

fn run_bake(
    request: &BakeRequest,
    lane: &mut ClientLane,
    model: &mut Option<StemsModel>,
    baker: &mut Option<LyricsBaker>,
    tx: &Sender<BakeMsg>,
    batch: &Arc<AtomicU64>,
) -> Result<BakeDone, String> {
    let started = std::time::Instant::now();
    let id = request.id;
    let stopped = || request.batch < batch.load(Ordering::Acquire);

    report(tx, id, BakeStage::Resolving);
    let client = lane.get(&request.session)?;
    let asset = match &request.target {
        BakeTarget::Asset(id) => *id,
        BakeTarget::Alias(alias) => {
            let alias = AssetAlias::from_str(alias.trim())
                .map_err(|_| format!("not an alias: {alias}"))?;
            client
                .resolve_alias(&alias)
                .map_err(|e| format!("resolve {alias}: {e}"))?
                .asset_id
        }
    };

    // 1. What the head already carries. Skipping here is the same decision
    //    the publish would make, taken before the GPU is touched.
    let manifest = head_manifest(client, &asset)?;
    let present = SideChannels::of(&manifest.files);
    let need = BakeNeed::decide(present, request.lyrics);
    if need.nothing() {
        return Ok(BakeDone {
            asset,
            stems: false,
            lyrics: false,
            already: true,
            no_vocals: false,
            secs: started.elapsed().as_secs_f64(),
            lines: 0,
        });
    }

    // 2. The audio itself, by digest.
    report(tx, id, BakeStage::Fetching);
    let file = manifest
        .files
        .iter()
        .find(|file| file.role == FileRole::Audio)
        .ok_or("asset carries no Audio file")?;
    let bytes = crate::store_content::fetch_blob(client, &file.blob, file.byte_len)?;

    report(tx, id, BakeStage::Decoding);
    let pcm = decode_track(&bytes)?;
    if pcm.frames.is_empty() {
        return Err("decoded to zero frames".into());
    }
    let digest = pcm.digest();
    let duration = pcm.duration_secs();
    let track = to_stereo_44k(&pcm);
    drop(bytes);
    drop(pcm);
    if stopped() {
        return Err("stopped".into());
    }

    // 3. Separation, one span per model forward so progress is truthful.
    report(tx, id, BakeStage::Loading);
    if model.is_none() {
        let path = stems_checkpoint_path();
        if !path.is_file() {
            return Err(format!(
                "separation checkpoint not found at {} (set ASSET_UI_STEMS_CKPT)",
                path.display()
            ));
        }
        log!("analysis: loading separation model · {}", model_provenance());
        *model = Some(
            StemsModel::load(&path).map_err(|error| format!("separation model: {error}"))?,
        );
    }
    let model = model.as_mut().expect("loaded model");
    let stems = separate(model, &track, tx, id, &stopped)?;
    drop(track);

    // 4. Encode, and 5. transcribe from the vocals stem we just made.
    let oggs = if need.stems {
        report(tx, id, BakeStage::Encoding);
        Some(encode_stem_oggs(&stems))
    } else {
        None
    };
    let mut lines = 0usize;
    let lyrics_json = if need.lyrics {
        report(tx, id, BakeStage::Transcribing);
        match transcribe(baker, &stems, duration, &digest) {
            Ok(Some((json, count))) => {
                lines = count;
                Some(json)
            }
            // Nothing sung is a real answer, not a failure: the stems still
            // publish and the track simply carries no lyrics.
            Ok(None) => None,
            Err(error) if oggs.is_some() => {
                // The expensive half succeeded; publish it and say why the
                // other half did not.
                let outcome = publish(lane, &request.session, &asset, oggs, None, tx, id)?;
                return Err(format!(
                    "stems published ({outcome}), lyrics failed: {error}"
                ));
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    drop(stems);

    // 6. Publish. Idempotent: a concurrent winner reports AlreadyPresent.
    let published_stems = oggs.is_some();
    let published_lyrics = lyrics_json.is_some();
    let no_vocals = need.lyrics && !published_lyrics;
    let outcome = publish(lane, &request.session, &asset, oggs, lyrics_json, tx, id)?;
    Ok(BakeDone {
        asset,
        stems: published_stems && !outcome.already,
        lyrics: published_lyrics && !outcome.already,
        already: outcome.already,
        no_vocals,
        secs: started.elapsed().as_secs_f64(),
        lines,
    })
}

struct PublishOutcome {
    already: bool,
}

impl std::fmt::Display for PublishOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.already { "already present" } else { "published" })
    }
}

fn publish(
    lane: &mut ClientLane,
    session: &ServerSession,
    asset: &AssetId,
    oggs: Option<[Vec<u8>; 4]>,
    lyrics_json: Option<String>,
    tx: &Sender<BakeMsg>,
    id: u64,
) -> Result<PublishOutcome, String> {
    if oggs.is_none() && lyrics_json.is_none() {
        return Ok(PublishOutcome { already: true });
    }
    report(tx, id, BakeStage::Publishing);
    let client = lane.get(session)?;
    let outcome = publish_side_channels(client, asset, oggs, lyrics_json)
        .map_err(|error| format!("publish side-channels: {error}"))?;
    Ok(PublishOutcome {
        already: matches!(
            outcome,
            makepad_asset_client::side_channels::SideChannelOutcome::AlreadyPresent { .. }
        ),
    })
}

/// Run the demixer to the end, reporting one span at a time. Byte-identical
/// to `demix_all`; the loop is open only so progress can be reported and a
/// Stop can land between spans.
fn separate(
    model: &mut StemsModel,
    track: &StereoBuf,
    tx: &Sender<BakeMsg>,
    id: u64,
    stopped: &impl Fn() -> bool,
) -> Result<StemSet, String> {
    let frames = track.frames();
    let mut out = makepad_ai_stems::model::empty_stem_set(frames);
    let mut demixer =
        Demixer::new(model, track).map_err(|error| format!("separation: {error}"))?;
    let total = demixer.span_count();
    let mut done = 0usize;
    report(tx, id, BakeStage::Separating { done, total });
    loop {
        if stopped() {
            return Err("stopped".into());
        }
        let Some(span) = demixer
            .next_span()
            .map_err(|error| format!("separation: {error}"))?
        else {
            break;
        };
        for stem in 0..makepad_ai_stems::NUM_STEMS {
            for channel in 0..makepad_ai_stems::AUDIO_CHANNELS {
                let src = span.stems[stem].channel(channel);
                let dst = out[stem].channel_mut(channel);
                let end = (span.start + src.len()).min(frames);
                if span.start < end {
                    dst[span.start..end].copy_from_slice(&src[..end - span.start]);
                }
            }
        }
        done += 1;
        report(tx, id, BakeStage::Separating { done, total });
    }
    Ok(out)
}

/// Lyrics from the separated VOCALS stem (`StemSet` index 3), as the
/// side-channel JSON keyed by the decoded track's digest.
fn transcribe(
    baker: &mut Option<LyricsBaker>,
    stems: &StemSet,
    duration: f64,
    digest: &str,
) -> Result<Option<(String, usize)>, String> {
    if baker.is_none() {
        let path = whisper_model_path().ok_or_else(|| {
            format!("no whisper checkpoint found ({WHISPER_MODEL_FILE}); set MAKEPAD_VOICE_MODEL")
        })?;
        *baker = Some(LyricsBaker::open(&path)?);
    }
    let baker = baker.as_mut().expect("loaded baker");
    let vocals = &stems[3];
    let mono: Vec<f32> = vocals
        .left
        .iter()
        .zip(vocals.right.iter())
        .map(|(l, r)| (l + r) * 0.5)
        .collect();
    let Some(lyrics) = baker.bake(&mono, STEMS_RATE as f64, duration, BAKE_LANGUAGE) else {
        return Ok(None);
    };
    let lines = lyrics.lines.len();
    Ok(Some((lyrics.to_json(digest), lines)))
}

fn fetch_loop(rx: Receiver<FetchRequest>, tx: Sender<FetchMsg>) {
    let mut lane = ClientLane::new("analysis-fetch-cache");
    while let Ok(request) = rx.recv() {
        let asset = request.asset;
        let generation = request.generation;
        if let Err(message) = run_fetch(&request, &mut lane, &tx) {
            let _ = tx.send(FetchMsg::Failed {
                asset,
                generation,
                message,
            });
        }
        SignalToUI::set_ui_signal();
    }
}

fn run_fetch(
    request: &FetchRequest,
    lane: &mut ClientLane,
    tx: &Sender<FetchMsg>,
) -> Result<(), String> {
    let asset = request.asset;
    let generation = request.generation;
    let client = lane.get(&request.session)?;
    let manifest = head_manifest(client, &asset)?;
    let roles = SideChannels::of(&manifest.files);
    let _ = tx.send(FetchMsg::Roles {
        asset,
        generation,
        roles,
    });
    SignalToUI::set_ui_signal();
    if !request.want_content || !roles.any() {
        return Ok(());
    }

    if roles.stems {
        let mut lanes: Vec<crate::audio::StemPcm> = Vec::with_capacity(4);
        for role in FileRole::STEMS {
            let file = manifest
                .files
                .iter()
                .find(|file| file.role == role)
                .ok_or("stem role vanished between checks")?;
            let bytes = crate::store_content::fetch_blob(client, &file.blob, file.byte_len)?;
            let decoded =
                makepad_audio_decode::decode_any(&bytes).map_err(|e| format!("{role:?}: {e:?}"))?;
            lanes.push(crate::audio::StemPcm {
                frames: i16_frames(&decoded.pcm_interleaved_f32, decoded.channels as usize),
                sample_rate: decoded.rate.max(1),
            });
        }
        let mut lanes = lanes.into_iter();
        let set: [crate::audio::StemPcm; 4] = [
            lanes.next().expect("drums"),
            lanes.next().expect("bass"),
            lanes.next().expect("vocals"),
            lanes.next().expect("other"),
        ];
        let _ = tx.send(FetchMsg::Stems {
            asset,
            generation,
            lanes: Box::new(set),
        });
        SignalToUI::set_ui_signal();
    }

    if roles.lyrics {
        let file = manifest
            .files
            .iter()
            .find(|file| file.role == FileRole::Lyrics)
            .ok_or("lyrics role vanished between checks")?;
        let bytes = crate::store_content::fetch_blob(client, &file.blob, file.byte_len)?;
        let rows = lyric_rows(&bytes).ok_or("lyrics document did not parse")?;
        let _ = tx.send(FetchMsg::Lyrics {
            asset,
            generation,
            rows,
        });
    }
    Ok(())
}

/// A stored lyrics document as reader rows.
///
/// The document is trusted for its OWN digest: the store handed these exact
/// bytes back under the blob hash the manifest names, so re-deriving the
/// track digest here to check it against itself would prove nothing the
/// content addressing has not already proved.
pub fn lyric_rows(bytes: &[u8]) -> Option<Vec<LyricRow>> {
    let digest = TrackLyrics::digest_of(bytes)?;
    let lyrics = TrackLyrics::from_json(bytes, &digest)?;
    Some(
        lyrics
            .lines
            .into_iter()
            .map(|line| LyricRow {
                stamp: lyric_stamp(line.start_secs),
                start_secs: line.start_secs,
                end_secs: line.end_secs,
                text: line.text,
                words: line.words,
                confident: line.confident,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_data::{AssetFile, BlobId, DeviceTier, MediaType};

    fn manifest_with(roles: &[FileRole]) -> Vec<AssetFile> {
        let mut files = Vec::new();
        for role in roles {
            files.push(AssetFile {
                role: *role,
                tier: DeviceTier::Any,
                lod: 0,
                media: if *role == FileRole::Lyrics {
                    MediaType::Json
                } else {
                    MediaType::Ogg
                },
                blob: BlobId::hash_of(format!("{role:?}").as_bytes()),
                byte_len: 4,
                dims: None,
            });
        }
        files
    }

    #[test]
    fn a_head_with_every_role_is_skipped_before_the_gpu_is_touched() {
        let bare = manifest_with(&[FileRole::Audio]);
        assert_eq!(
            SideChannels::of(&bare),
            SideChannels { stems: false, lyrics: false }
        );
        // Stems only.
        let mut roles = FileRole::STEMS.to_vec();
        roles.push(FileRole::Audio);
        let stemmed = manifest_with(&roles);
        assert_eq!(
            SideChannels::of(&stemmed),
            SideChannels { stems: true, lyrics: false }
        );
        // A stems-only ask over a stemmed head does nothing at all.
        assert!(BakeNeed::decide(SideChannels::of(&stemmed), false).nothing());
        // The same head with lyrics asked for still SEPARATES (the bake
        // reads the vocals stem) but only publishes the lyrics.
        let need = BakeNeed::decide(SideChannels::of(&stemmed), true);
        assert_eq!(need, BakeNeed { stems: false, lyrics: true });
        assert!(
            !need.nothing(),
            "lyrics alone still separate: the transcript is baked FROM the vocals stem"
        );
        // A bare head needs both.
        assert_eq!(
            BakeNeed::decide(SideChannels::of(&bare), true),
            BakeNeed { stems: true, lyrics: true }
        );
        // Three of four stems is NOT a stem set: all-four-or-none.
        let partial = manifest_with(&FileRole::STEMS[..3]);
        assert!(!SideChannels::of(&partial).stems);
        assert!(BakeNeed::decide(SideChannels::of(&partial), false).stems);
    }

    #[test]
    fn lyrics_only_heads_still_need_the_stems() {
        let lyric_only = manifest_with(&[FileRole::Audio, FileRole::Lyrics]);
        let roles = SideChannels::of(&lyric_only);
        assert_eq!(roles, SideChannels { stems: false, lyrics: true });
        assert_eq!(
            BakeNeed::decide(roles, true),
            BakeNeed { stems: true, lyrics: false }
        );
    }

    /// The i16 conversion IS the digest: it has to be the VJ's, sample for
    /// sample. (`apps/vj` is a separate binary crate, so the parity is
    /// pinned here as the literal formula both sides implement.)
    #[test]
    fn i16_conversion_matches_the_vj_formula() {
        for value in [-1.5f32, -1.0, -0.5, -1.0 / 32768.0, 0.0, 0.25, 0.5, 1.0, 2.0] {
            assert_eq!(
                i16_sample(value),
                (value.clamp(-1.0, 1.0) * 32767.0) as i16,
                "{value}"
            );
        }
        assert_eq!(i16_sample(1.0), 32767);
        assert_eq!(i16_sample(-1.0), -32767);
        assert_eq!(i16_sample(f32::NAN), 0, "a NaN sample must not wrap");
        // Mono duplicates into both lanes; stereo keeps first/last.
        assert_eq!(i16_frames(&[1.0, -1.0], 2), vec![[32767, -32767]]);
        assert_eq!(i16_frames(&[1.0], 1), vec![[32767, 32767]]);
        // Five channels: first and LAST, exactly as the VJ's decode does.
        assert_eq!(i16_frames(&[1.0, 0.0, 0.0, 0.0, -1.0], 5), vec![[32767, -32767]]);
    }

    /// PCM16 WAV must keep its RAW samples: a float round-trip moves values
    /// by one bit and takes the whole digest with it.
    #[test]
    fn wav_pcm16_is_decoded_without_a_float_round_trip() {
        let samples: [i16; 6] = [0, 32767, -32768, 1234, -1234, 500];
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&0u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&((samples.len() * 2) as u32).to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        let size = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&size.to_le_bytes());

        let pcm = decode_track(&wav).expect("wav decodes");
        assert_eq!(pcm.sample_rate, 44_100);
        assert_eq!(
            pcm.frames,
            vec![[0, 32767], [-32768, 1234], [-1234, 500]],
            "PCM16 keeps -32768, which a float round-trip cannot represent"
        );
        assert!((pcm.duration_secs() - 3.0 / 44_100.0).abs() < 1e-9);
    }

    #[test]
    fn the_digest_is_the_lyrics_crates_and_moves_with_the_audio() {
        let a = TrackPcm {
            frames: vec![[0, 0]; 100],
            sample_rate: 44_100,
        };
        assert_eq!(a.digest(), makepad_audio_lyrics::track_digest(44_100, &a.frames));
        let longer = TrackPcm {
            frames: vec![[0, 0]; 101],
            sample_rate: 44_100,
        };
        assert_ne!(a.digest(), longer.digest());
        let other_rate = TrackPcm {
            frames: vec![[0, 0]; 100],
            sample_rate: 48_000,
        };
        assert_ne!(a.digest(), other_rate.digest());
        let mut altered = a.clone();
        altered.frames[50] = [1, 0];
        assert_ne!(a.digest(), altered.digest());
        // Filename-safe: it keys caches and side-channel documents.
        assert!(!a.digest().contains(['/', '\\', '.']));
        assert_eq!(a.digest().len(), 64);
    }

    #[test]
    fn model_input_is_stereo_at_the_models_own_rate() {
        let track = TrackPcm {
            frames: (0..2_000).map(|i| [(i % 100) as i16 * 100, 0]).collect(),
            sample_rate: 22_050,
        };
        let buf = to_stereo_44k(&track);
        assert_eq!(buf.left.len(), buf.right.len());
        // Half rate in, double the frames out (within the resampler's own
        // rounding of the output length).
        assert!(
            buf.frames().abs_diff(4_000) <= 2,
            "{} frames from 2000 at 22.05k",
            buf.frames()
        );
        // A 44.1k track is passed through untouched.
        let native = TrackPcm {
            frames: vec![[16_384, -16_384]; 64],
            sample_rate: 44_100,
        };
        let buf = to_stereo_44k(&native);
        assert_eq!(buf.frames(), 64);
        assert!((buf.left[0] - 0.5).abs() < 1e-6);
        assert!((buf.right[0] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn stage_labels_and_bands_are_monotonic() {
        let stages = [
            BakeStage::Waiting,
            BakeStage::Resolving,
            BakeStage::Fetching,
            BakeStage::Decoding,
            BakeStage::Loading,
            BakeStage::Separating { done: 0, total: 10 },
            BakeStage::Separating { done: 5, total: 10 },
            BakeStage::Separating { done: 10, total: 10 },
            BakeStage::Encoding,
            BakeStage::Transcribing,
            BakeStage::Publishing,
        ];
        for pair in stages.windows(2) {
            assert!(
                pair[0].fraction() <= pair[1].fraction(),
                "{:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(
            BakeStage::Separating { done: 4, total: 10 }.label(),
            "separating 40%"
        );
        // A zero-span track must not divide by zero.
        assert_eq!(BakeStage::Separating { done: 0, total: 0 }.label(), "separating");
    }

    #[test]
    fn a_lyrics_document_round_trips_into_reader_rows() {
        let lyrics = TrackLyrics {
            backend: "whisper".into(),
            model: "ggml-large-v3-turbo.bin".into(),
            language: "en".into(),
            duration_secs: 61.5,
            onset: Default::default(),
            lines: vec![
                makepad_audio_lyrics::LyricLine {
                    start_secs: 61.25,
                    end_secs: 63.0,
                    text: "hello there".into(),
                    words: vec![61.25, 62.0],
                    confident: true,
                },
                makepad_audio_lyrics::LyricLine::new(70.0, 71.0, "and again"),
            ],
        };
        let json = lyrics.to_json("f00d");
        let rows = lyric_rows(json.as_bytes()).expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "hello there");
        assert_eq!(rows[0].stamp, lyric_stamp(61.25));
        assert!(rows[0].confident);
        assert_eq!(rows[0].words.len(), 2);
        assert!(!rows[1].confident);
        // Garbage is refused rather than shown as an empty transcript.
        assert!(lyric_rows(b"{}").is_none());
        assert!(lyric_rows(b"not json at all").is_none());
    }

    #[test]
    fn a_verdict_names_what_was_published_and_never_lies_about_lyrics() {
        let asset = AssetId::from_bytes([7u8; 16]);
        let done = |stems, lyrics, already, no_vocals, lines| BakeDone {
            asset,
            stems,
            lyrics,
            already,
            no_vocals,
            secs: 42.0,
            lines,
        };
        assert_eq!(
            done(true, true, false, false, 31).summary(),
            "4 stems + 31 lyric lines · 42s"
        );
        assert_eq!(done(true, false, false, false, 0).summary(), "4 stems · 42s");
        assert_eq!(done(false, false, true, false, 0).summary(), "already analysed");
        // An instrumental asked for lyrics: the stems still landed and the
        // verdict says what happened instead of "already analysed".
        assert_eq!(
            done(true, false, false, true, 0).summary(),
            "4 stems + nothing sung in the vocals stem · 42s"
        );
        assert_eq!(
            done(false, false, true, true, 0).summary(),
            "nothing sung in the vocals stem · 42s"
        );
    }

    #[test]
    fn targets_dedupe_by_a_stable_key() {
        let alias = BakeTarget::Alias("music/artist/track".into());
        assert_eq!(alias.key(), "alias:music/artist/track");
        assert_ne!(
            BakeTarget::Alias("a".into()).key(),
            BakeTarget::Alias("b".into()).key()
        );
    }

    #[test]
    fn the_queue_counts_a_batch_and_keeps_its_verdict() {
        let mut queue = AnalysisQueue::start();
        assert!(!queue.busy());
        assert_eq!(queue.status_line(), "");
        assert_eq!(queue.progress_fraction(), 0.0);

        // No server session in a unit test: enqueue with a fabricated one is
        // fine, the worker simply fails to connect. What is under test is
        // the bookkeeping.
        let session = ServerSession {
            endpoints: makepad_asset_client::ApiEndpoints {
                control: "127.0.0.1:1".parse().unwrap(),
                data: "127.0.0.1:2".parse().unwrap(),
            },
            token: "t".into(),
            server_id: [0u8; 16],
        };
        let target = BakeTarget::Alias("music/a".into());
        assert!(queue.enqueue(target.clone(), "A", false, session.clone()));
        assert!(
            !queue.enqueue(target, "A", false, session.clone()),
            "the same track cannot sit in the queue twice"
        );
        assert!(queue.enqueue(BakeTarget::Alias("music/b".into()), "B", true, session));
        assert_eq!(queue.total, 2);
        assert!(queue.busy());
        assert!(queue.status_line().starts_with("analysing 1/2 · waiting · A"));

        // Stopping empties the queue and says what it dropped.
        queue.stop();
        assert!(!queue.busy());
        assert!(queue.summary.contains("2 left unanalysed"), "{}", queue.summary);
    }
}
