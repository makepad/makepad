//! Server-stored stems and lyrics: fetch what someone already computed, and
//! give back what this machine computes.
//!
//! Separation and word alignment are the two expensive things a deck does,
//! and they produce exactly the same answer every time for the same audio.
//! So the answer belongs on the asset, not in one laptop's cache: four Ogg
//! Vorbis stems and one lyrics JSON published as side-channel files on the
//! audio asset's own revision (see `makepad_audio_sidechannels` and the
//! `FileRole::STEMS` / `FileRole::Lyrics` contract).
//!
//! Two directions, both here:
//!
//! * **Fetch** ([`SideChannelPool`]) — a track whose manifest carries the
//!   stem roles skips `stems.rs` entirely. The four oggs download beside the
//!   audio, decode in parallel in well under a second, and publish through
//!   the SAME [`StemsMsg`] chunk plumbing the separator uses, so the mixer,
//!   the knobs and the waveform colour cannot tell the difference. A lyrics
//!   file is written verbatim into the VJ's own lyrics cache — the two
//!   formats are one format ([`makepad_audio_lyrics::schema`]) — and the
//!   normal cache-probe path then installs it.
//! * **Write back** ([`WriteBackPool`]) — when the LOCAL separator finishes a
//!   store track that had no stems, the finished span cache is read back,
//!   encoded once and offered to the store. It is the lowest-priority work
//!   in the app: it never starts until separation is completely done, it
//!   stands aside before it begins, and one refusal is one log line.
//!
//! Nothing here writes the stem cache: fetched stems go straight to the
//! deck, and the digest-keyed span cache stays what it has always been —
//! what THIS machine separated.

use crate::decks::DeckId;
use crate::mixer::TrackPcm;
use crate::stems::{
    chunk_count, chunk_frames, model_frames, resample, track_digest, StemChunk, StemsMsg,
};
use makepad_ai_stems::{CacheHeader, StemCache, SAMPLE_RATE as STEMS_RATE};
use makepad_asset_data::AssetId;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

/// Where each deck lane's audio sits in a fetched `FileRole::STEMS` set.
///
/// Three orders meet on this line and none of them agree: the deck's lanes
/// are `STEM_ORDER` (vocals, drums, bass, other), the separation model's
/// `StemSet` is (drums, bass, other, vocals), and the published contract's
/// `FileRole::STEMS` is (drums, bass, vocals, other). Getting it wrong is
/// silent — the track still plays, with the bass knob killing the singer —
/// so the mapping is derived from the crates' own constants in the test
/// below rather than trusted here.
pub const LANE_FROM_ROLE: [usize; 4] = [2, 0, 1, 3];

// ---------------------------------------------------------------------------
// fetching
// ---------------------------------------------------------------------------

/// One deck's fetched side-channels, ready to become chunks.
pub struct FetchedJob {
    pub deck: DeckId,
    pub gen: u64,
    /// The decoded track: it decides the chunk geometry, the rate the stems
    /// are resampled to, and the digest the lyrics are keyed by.
    pub pcm: Arc<TrackPcm>,
    /// The four downloaded stem oggs, in `FileRole::STEMS` order.
    pub stem_files: [PathBuf; 4],
    /// The downloaded lyrics JSON, when the revision carried one.
    pub lyrics_file: Option<PathBuf>,
}

pub enum SideChannelMsg {
    /// Chunks and status, in the separator's own vocabulary: everything
    /// downstream of the deck treats the two sources identically.
    Stems(StemsMsg),
    /// The side-channel was unusable (a stem would not decode). The deck has
    /// to separate locally after all; nothing else can rescue it.
    Fallback { deck: DeckId, gen: u64, reason: String },
}

/// One worker for fetched side-channels. Separate from the separation pool
/// on purpose: this path is a deck-load latency path (four small decodes)
/// and must never queue behind a model forward.
pub struct SideChannelPool {
    tx: Sender<FetchedJob>,
    rx: Receiver<SideChannelMsg>,
}

impl Default for SideChannelPool {
    fn default() -> Self {
        SideChannelPool::new()
    }
}

impl SideChannelPool {
    pub fn new() -> SideChannelPool {
        let (tx, jobs) = channel::<FetchedJob>();
        let (out, rx) = channel::<SideChannelMsg>();
        let _ = std::thread::Builder::new()
            .name("vj-sidechannel".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    run_fetched(job, &out);
                }
            });
        SideChannelPool { tx, rx }
    }

    pub fn submit(&self, job: FetchedJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<SideChannelMsg> {
        drain(&self.rx)
    }
}

fn drain<T>(rx: &Receiver<T>) -> Vec<T> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(message) => out.push(message),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        }
    }
    out
}

/// Decode one fetched stem into the track's own frames: 44.1 kHz stereo out
/// of the encoder, resampled only when the track itself is at another rate.
fn decode_stem(path: &Path, track_rate: u32) -> Result<Vec<[i16; 2]>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoded = makepad_audio_decode::decode_any(&bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let channels = decoded.channels.max(1) as usize;
    let frames = decoded.pcm_interleaved_f32.len() / channels;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for frame in decoded.pcm_interleaved_f32.chunks_exact(channels) {
        left.push(frame[0]);
        // A mono stem (never produced by our encoder, but a store is not
        // ours to assume about) plays down the middle rather than silently
        // losing a channel.
        right.push(if channels > 1 { frame[1] } else { frame[0] });
    }
    let from = decoded.rate.max(1) as f64;
    let to = track_rate.max(1) as f64;
    if (from - to).abs() >= 0.5 {
        left = resample(&left, from, to);
        right = resample(&right, from, to);
    }
    Ok(left
        .iter()
        .zip(right.iter())
        .map(|(l, r)| {
            [
                (l.clamp(-1.0, 1.0) * 32767.0) as i16,
                (r.clamp(-1.0, 1.0) * 32767.0) as i16,
            ]
        })
        .collect())
}

/// Cut whole-track lanes (in the deck's lane order, at the track's rate)
/// into the deck's chunks.
///
/// WHOLE chunks only, exactly like the separator's `ChunkWriter`: the mixer
/// falls back to the mixed file for a chunk that is absent, but a chunk that
/// is PRESENT and short would play its tail as silence. The last part-second
/// of a track is left to the file.
fn cut_chunks(
    lanes: [Vec<[i16; 2]>; 4],
    rate: u32,
    track_frames: usize,
) -> Vec<[Arc<Vec<[i16; 2]>>; 4]> {
    let size = chunk_frames(rate);
    let whole = track_frames / size;
    let mut out = Vec::with_capacity(whole);
    for index in 0..whole {
        let start = index * size;
        let end = start + size;
        let blocks: Vec<Arc<Vec<[i16; 2]>>> = lanes
            .iter()
            .map(|lane| {
                // A stem may come back a few frames short of the track (codec
                // granularity); those frames are silence in that lane, not a
                // reason to drop the chunk.
                Arc::new(
                    (start..end)
                        .map(|frame| lane.get(frame).copied().unwrap_or([0, 0]))
                        .collect::<Vec<[i16; 2]>>(),
                )
            })
            .collect();
        out.push([
            blocks[0].clone(),
            blocks[1].clone(),
            blocks[2].clone(),
            blocks[3].clone(),
        ]);
    }
    out
}

fn run_fetched(job: FetchedJob, out: &Sender<SideChannelMsg>) {
    let status = |text: &str, working: bool| {
        let _ = out.send(SideChannelMsg::Stems(StemsMsg::Status {
            deck: job.deck,
            gen: job.gen,
            text: text.to_string(),
            working,
        }));
    };
    status("stems: side-channel", true);

    let track_rate = job.pcm.sample_rate.max(1);
    let mut decoded: Vec<Result<Vec<[i16; 2]>, String>> = Vec::with_capacity(4);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(4);
        for path in job.stem_files.iter() {
            handles.push(scope.spawn(move || decode_stem(path, track_rate)));
        }
        for handle in handles {
            decoded.push(handle.join().unwrap_or_else(|_| Err("decode panicked".into())));
        }
    });
    let mut lanes: [Vec<[i16; 2]>; 4] = Default::default();
    for (lane, source) in LANE_FROM_ROLE.into_iter().enumerate() {
        match std::mem::replace(&mut decoded[source], Ok(Vec::new())) {
            Ok(frames) => lanes[lane] = frames,
            Err(error) => {
                let _ = out.send(SideChannelMsg::Fallback {
                    deck: job.deck,
                    gen: job.gen,
                    reason: error,
                });
                return;
            }
        }
    }
    let bytes: u64 = job
        .stem_files
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum();

    let frames = job.pcm.frames.len();
    let count = chunk_count(frames, track_rate);
    let size = chunk_frames(track_rate);
    for (index, blocks) in cut_chunks(lanes, track_rate, frames).into_iter().enumerate() {
        if out
            .send(SideChannelMsg::Stems(StemsMsg::Chunk(Box::new(StemChunk {
                deck: job.deck,
                gen: job.gen,
                index,
                chunk_frames: size,
                chunk_count: count,
                lanes: blocks,
            }))))
            .is_err()
        {
            return;
        }
    }
    let _ = out.send(SideChannelMsg::Stems(StemsMsg::Done {
        deck: job.deck,
        gen: job.gen,
    }));
    makepad_widgets::log!(
        "deck {:?}: stems from side-channel ({bytes} bytes), separation skipped",
        job.deck
    );

    // The digest is what both caches are keyed by, and hashing decoded PCM is
    // not UI-thread work — so it happens here, once, like the separator does.
    let digest = track_digest(&job.pcm);
    if let Some(path) = job.lyrics_file.as_ref() {
        install_lyrics(path, &digest);
    }
    // Arm the lyrics READ probe (never a bake: the whisper pass reads the
    // vocals stem out of the span cache, and a fetched track never writes
    // one). A store lyrics file has just landed in the cache, and a track
    // this machine transcribed in an earlier session is in there too; both
    // reach the deck through the one existing path.
    let _ = out.send(SideChannelMsg::Stems(StemsMsg::Coverage {
        deck: job.deck,
        gen: job.gen,
        digest,
        model_frames: model_frames(&job.pcm) as u64,
        complete: false,
    }));
}

/// Drop a fetched lyrics document into the VJ's lyrics cache, verbatim.
///
/// The side-channel payload and the cache file are the same format keyed by
/// the same digest, so this is a copy and not a re-serialization — but it is
/// a VERIFIED copy: `from_json` refuses a document whose digest is not this
/// track's, which is exactly what a re-encode of the audio would produce.
/// Wrong words on the wrong timeline are worse than no words.
fn install_lyrics(path: &Path, digest: &str) {
    let Ok(bytes) = std::fs::read(path) else { return };
    if makepad_audio_lyrics::TrackLyrics::from_json(&bytes, digest).is_none() {
        makepad_widgets::log!(
            "side-channel lyrics ignored: not this audio (digest {})",
            &digest[..8.min(digest.len())]
        );
        return;
    }
    let dir = crate::lyrics::cache_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let target = crate::lyrics::cache_path(&dir, digest);
    // Beside and rename, like `lyrics::store_cached`: a crash mid-write must
    // not leave a half file that parses as a shorter track.
    let temp = target.with_extension("json.tmp");
    if std::fs::write(&temp, &bytes).is_ok() {
        let _ = std::fs::rename(&temp, &target);
    }
}

// ---------------------------------------------------------------------------
// writing back
// ---------------------------------------------------------------------------

/// How long the write-back stands aside before it starts. Separation has
/// just finished, the deck is probably playing, and nothing about this work
/// is urgent — it is for the NEXT machine to load this track.
const WRITE_BACK_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// Read a completed span cache back and encode it, so the store can have it.
pub struct WriteBackJob {
    pub asset: AssetId,
    /// The span cache's key and header: exactly what `StemsMsg::Coverage`
    /// reported, so the entry re-opens as the run that wrote it left it.
    pub digest: String,
    pub model_frames: u64,
}

pub enum WriteBackMsg {
    /// Four Ogg Vorbis stems in `FileRole::STEMS` order, ready to publish.
    Encoded { asset: AssetId, oggs: Box<[Vec<u8>; 4]> },
    /// There was nothing to give back after all (the entry was evicted by the
    /// cache budget between the coverage report and this read, say).
    Skipped { asset: AssetId, reason: String },
}

/// One worker, so two tracks separated back to back never encode at once.
pub struct WriteBackPool {
    tx: Sender<WriteBackJob>,
    rx: Receiver<WriteBackMsg>,
}

impl Default for WriteBackPool {
    fn default() -> Self {
        WriteBackPool::new()
    }
}

impl WriteBackPool {
    pub fn new() -> WriteBackPool {
        WriteBackPool::with_root(crate::stems::cache_dir())
    }

    pub fn with_root(root: PathBuf) -> WriteBackPool {
        let (tx, jobs) = channel::<WriteBackJob>();
        let (out, rx) = channel::<WriteBackMsg>();
        let _ = std::thread::Builder::new()
            .name("vj-sidechannel-writeback".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    std::thread::sleep(WRITE_BACK_DELAY);
                    let message = match encode_from_cache(&root, &job) {
                        Ok(oggs) => WriteBackMsg::Encoded { asset: job.asset, oggs },
                        Err(reason) => WriteBackMsg::Skipped { asset: job.asset, reason },
                    };
                    if out.send(message).is_err() {
                        return;
                    }
                }
            });
        WriteBackPool { tx, rx }
    }

    pub fn submit(&self, job: WriteBackJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<WriteBackMsg> {
        drain(&self.rx)
    }
}

fn encode_from_cache(root: &Path, job: &WriteBackJob) -> Result<Box<[Vec<u8>; 4]>, String> {
    if job.model_frames == 0 {
        return Err("empty track".to_string());
    }
    let header = CacheHeader::for_track(job.model_frames);
    let mut cache =
        StemCache::open(root, &job.digest, header).map_err(|e| format!("cache: {e}"))?;
    if !cache.is_complete() {
        return Err("span cache is no longer complete".to_string());
    }
    // The cache is already at the model's rate, which is the rate the stem
    // encoder publishes at, so nothing is resampled on this path.
    let stems = cache.read_all().map_err(|e| format!("cache read: {e}"))?;
    debug_assert_eq!(STEMS_RATE, makepad_ai_stems::SAMPLE_RATE);
    Ok(Box::new(makepad_audio_sidechannels::encode_stem_oggs(&stems)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stems::STEM_ORDER;
    use makepad_asset_data::FileRole;

    /// The one mapping that cannot be checked by ear: derive it from the two
    /// crates' own constants instead of trusting the literal.
    #[test]
    fn every_deck_lane_takes_the_role_that_holds_its_stem() {
        for (lane, stem) in STEM_ORDER.into_iter().enumerate() {
            // `STEM_ROLE_TO_SET` is the contract's role order paired with the
            // model's `StemSet` index, which is what `Stem as usize` is.
            let want = makepad_audio_sidechannels::STEM_ROLE_TO_SET
                .iter()
                .position(|(_, set_index)| *set_index == stem as usize)
                .expect("every model stem has a role");
            assert_eq!(
                LANE_FROM_ROLE[lane], want,
                "deck lane {lane} ({stem:?}) must read role {:?}",
                FileRole::STEMS[want]
            );
        }
        // …and the role order the mapping indexes into is the published one.
        let roles: Vec<FileRole> = makepad_audio_sidechannels::STEM_ROLE_TO_SET
            .iter()
            .map(|(role, _)| *role)
            .collect();
        assert_eq!(roles, FileRole::STEMS.to_vec());
        // No lane reads the same file as another.
        let mut seen = LANE_FROM_ROLE.to_vec();
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    #[test]
    fn chunks_are_whole_seconds_of_track_in_lane_order() {
        // Two and a half seconds at 100 Hz, each lane carrying its own value
        // so a swapped lane is visible.
        let rate = 100u32;
        let frames = 250usize;
        let lanes: [Vec<[i16; 2]>; 4] =
            std::array::from_fn(|lane| vec![[lane as i16 + 1, lane as i16 + 1]; frames]);
        let chunks = cut_chunks(lanes, rate, frames);
        // The part-second at the end is left to the mixed file, exactly as
        // the separator's writer leaves it.
        assert_eq!(chunks.len(), 2);
        for chunk in &chunks {
            for (lane, block) in chunk.iter().enumerate() {
                assert_eq!(block.len(), chunk_frames(rate));
                assert_eq!(block[0], [lane as i16 + 1; 2]);
            }
        }
    }

    #[test]
    fn a_short_stem_pads_rather_than_losing_the_chunk() {
        let rate = 100u32;
        let frames = 200usize;
        // Lane 3 stops four frames early — codec granularity, not a fault.
        let lanes: [Vec<[i16; 2]>; 4] = std::array::from_fn(|lane| {
            let len = if lane == 3 { frames - 4 } else { frames };
            vec![[7, 7]; len]
        });
        let chunks = cut_chunks(lanes, rate, frames);
        assert_eq!(chunks.len(), 2);
        let tail = &chunks[1][3];
        assert_eq!(tail.len(), 100);
        assert_eq!(tail[95], [7, 7]);
        assert_eq!(tail[96], [0, 0], "the missing frames are silence in that lane");
    }

    #[test]
    fn a_track_shorter_than_a_chunk_publishes_nothing() {
        let lanes: [Vec<[i16; 2]>; 4] = std::array::from_fn(|_| vec![[1, 1]; 50]);
        assert!(cut_chunks(lanes, 100, 50).is_empty());
    }

    /// The round trip that actually matters: encode four distinguishable
    /// tones as the store would, decode them back through the fetch path,
    /// and check each DECK lane got the stem its knob is labelled with.
    #[test]
    fn fetched_stems_land_on_the_lane_their_role_names() {
        use makepad_ai_stems::{StemSet, StereoBuf};
        let rate = STEMS_RATE;
        let hz = [110.0f32, 220.0, 440.0, 660.0];
        // `StemSet` order is the model's: drums, bass, other, vocals.
        let set: StemSet = std::array::from_fn(|stem| {
            let n = rate as usize * 2;
            let wave: Vec<f32> = (0..n)
                .map(|t| {
                    (2.0 * std::f32::consts::PI * hz[stem] * t as f32 / rate as f32).sin() * 0.4
                })
                .collect();
            StereoBuf { left: wave.clone(), right: wave }
        });
        let oggs = makepad_audio_sidechannels::encode_stem_oggs(&set);
        let dir = std::env::temp_dir().join(format!(
            "makepad-vj-sidechannel-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut paths = Vec::new();
        for (index, ogg) in oggs.iter().enumerate() {
            let path = dir.join(format!("{index}.ogg"));
            std::fs::write(&path, ogg).expect("write stem");
            paths.push(path);
        }
        let mut lanes: [Vec<[i16; 2]>; 4] = Default::default();
        for (lane, source) in LANE_FROM_ROLE.into_iter().enumerate() {
            lanes[lane] = decode_stem(&paths[source], rate).expect("decode");
        }
        for (lane, stem) in STEM_ORDER.into_iter().enumerate() {
            let frames = &lanes[lane];
            let mid = &frames[frames.len() / 4..frames.len() * 3 / 4];
            let crossings = mid
                .windows(2)
                .filter(|w| w[0][0] < 0 && w[1][0] >= 0)
                .count();
            let measured = crossings as f32 * rate as f32 / mid.len() as f32;
            let want = hz[stem as usize];
            assert!(
                (measured - want).abs() < want * 0.05,
                "deck lane {lane} ({stem:?}) came back at {measured} Hz, want {want}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
