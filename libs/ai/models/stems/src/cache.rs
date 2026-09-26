//! On-disk cache of separated spans, keyed by the source audio's content
//! digest.
//!
//! Separation costs about a third of a track's duration, so a track must pay
//! it once — not once per play, and not again after a seek back into a region
//! already separated. The cache is therefore SPAN-granular, not track-granular:
//! `Demixer` finalizes one `CHUNK_STEP` span per forward, each span is written
//! as soon as it exists, and a later session can start playing from whatever
//! spans are already there while the worker fills the gaps.
//!
//! Layout under `<root>/<digest>/`:
//!   `header`      — `key=value` lines: model provenance + geometry.
//!   `spans`       — one byte per span, 1 = written. The completeness bitmap.
//!   `gains`       — one f32 per (span, stem): the peak that span's samples
//!                   were normalized by before quantization.
//!   `<stem>.pcm`  — interleaved stereo i16 at the track's sample rate,
//!                   pre-sized to the full track, written span by span.
//!
//! Which stems an entry holds is DATA, not a constant of the crate: the header
//! names its lanes in the separator's order, and there is one `<lane>.pcm` and
//! one gain per span for each. The four-stem model's header names none, which
//! means `drums,bass,other,vocals`; its text is compared byte for byte by
//! builds still in use, so it goes on being written exactly as it always was.
//! A separator with other lanes, or another span grid, keeps its entries
//! under a root of its own: one `<digest>` directory holds one separation.
//! Opening it for another rendering by the same model replaces it; opening
//! it for another model, or for other lanes, is refused and changes nothing.
//!
//! i16 is deliberate: it is what a mixer consumes and it is 4x smaller than f32
//! (a 4-minute track is 169 MB across four stems rather than 677 MB). It is
//! NOT safe to clamp at full scale, though: BS-RoFormer's masks are complex
//! ratios, not a partition of unity, so a stem legitimately peaks above 1.0 —
//! the measured vocals stem of the test fixture hits 1.12. Each span is
//! therefore normalized by its own peak and the peak stored beside it, which
//! also buys back precision on quiet spans.
//!
//! That trade costs about 700 KB per second of track — a quarter of a gigabyte
//! for a six-minute one — so the root is BOUNDED: [`prune`] drops whole track
//! entries, least recently USED first, until the root fits a byte budget. An
//! entry is "used" when it is opened, which is when a track lands on a deck,
//! and the caller pins whatever is on a deck right now so a set in progress is
//! never evicted out from under itself.

use crate::config::*;
use crate::model::{StemSet, StereoBuf};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bytes one frame occupies in a stem file (stereo i16).
const FRAME_BYTES: u64 = 4;

/// Last-used stamp, seconds since the epoch, little-endian. Its own file so a
/// read of the cache never rewrites anything the audio depends on.
const USED_FILE: &str = "used";

/// An entry being deleted is RENAMED under this prefix first, so a prune that
/// is interrupted can never leave a half-deleted entry that still claims to
/// hold spans. The prefix starts with a dot, which [`StemCache::open`] refuses
/// as a digest, so a leftover can never be mistaken for a track.
const EVICTING_PREFIX: &str = ".evicting-";

#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    /// The cache directory exists but describes different audio or a different
    /// model — the caller should discard it.
    Mismatch(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::Io(e) => write!(f, "stems cache io: {e}"),
            CacheError::Mismatch(what) => write!(f, "stems cache mismatch: {what}"),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<std::io::Error> for CacheError {
    fn from(e: std::io::Error) -> Self {
        CacheError::Io(e)
    }
}

type Result<T> = std::result::Result<T, CacheError>;

/// What a separator is, as a cache header records it: the model, the
/// checkpoint file and its hash, and the two provenance lines a person reads.
/// Each separator brings its own, so the cache itself knows about none of
/// them; [`BS_ROFORMER_4STEM`](Self::BS_ROFORMER_4STEM) is the one every
/// entry was made by before there was a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub model_id: &'static str,
    pub checkpoint: &'static str,
    pub checkpoint_sha256: &'static str,
    /// The licence line as the HEADER carries it. For the four-stem model
    /// that is the frozen [`crate::CACHE_HEADER_LICENSE`], not the statement
    /// of record.
    pub license: &'static str,
    pub source: &'static str,
}

impl ModelIdentity {
    /// The four-stem BS-RoFormer, exactly as [`CacheHeader::for_track`] has
    /// always written it.
    pub const BS_ROFORMER_4STEM: ModelIdentity = ModelIdentity {
        model_id: crate::MODEL_ID,
        checkpoint: crate::MODEL_CHECKPOINT,
        checkpoint_sha256: crate::MODEL_SHA256,
        license: crate::CACHE_HEADER_LICENSE,
        source: crate::CACHE_HEADER_SOURCE,
    };
}

/// Identity of what produced the cached audio. A mismatch on any field means
/// the cached spans are not the ones this build would compute, so the entry is
/// never trusted: rebuilt where the same model made it, refused where another
/// did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheHeader {
    pub model_id: String,
    pub checkpoint: String,
    pub checkpoint_sha256: String,
    pub license: String,
    pub source: String,
    pub sample_rate: u32,
    pub frames: u64,
    pub span_samples: u64,
    pub span_count: u64,
    /// The lanes the entry holds, in the separator's order: one `<lane>.pcm`
    /// and one gain per span for each. The four-stem model's
    /// `drums,bass,other,vocals` is what a header that names none means.
    pub stems: Vec<String>,
}

/// Whether `stems` is the four-stem model's lane list, in its order — the
/// one list a header leaves unwritten, and the only one a [`StemSet`] can
/// stand for.
fn is_four_stem_lanes(stems: &[String]) -> bool {
    stems.iter().map(String::as_str).eq(STEM_NAMES)
}

/// A lane name becomes a file name and one item of a comma-separated header
/// line, so it is held to letters, digits, `-` and `_`: nothing that could
/// leave the entry's directory, split the list or end the line.
fn check_lanes(stems: &[String]) -> Result<()> {
    if stems.is_empty() {
        return Err(CacheError::Mismatch("header names no lanes".into()));
    }
    for (index, lane) in stems.iter().enumerate() {
        let plain = !lane.is_empty()
            && lane
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !plain {
            return Err(CacheError::Mismatch(format!(
                "lane name {lane:?} is not a bare word"
            )));
        }
        if stems[..index].contains(lane) {
            return Err(CacheError::Mismatch(format!("lane {lane:?} is named twice")));
        }
    }
    Ok(())
}

impl CacheHeader {
    /// Whether `other` describes the same separation of the same track: the
    /// same model and checkpoint over the same samples on the same span
    /// grid. The licence and source lines are provenance -- what somebody
    /// reads, not what the bytes depend on -- so a model described in new
    /// words is still the model that made these stems, and hours of
    /// separation are not thrown away over a sentence.
    ///
    /// The lanes ARE what the bytes depend on: they size the gains file and
    /// name the audio files, so two headers that differ in them are never
    /// the same separation, whatever else agrees.
    pub fn same_separation(&self, other: &CacheHeader) -> bool {
        self.model_id == other.model_id
            && self.checkpoint == other.checkpoint
            && self.checkpoint_sha256 == other.checkpoint_sha256
            && self.sample_rate == other.sample_rate
            && self.frames == other.frames
            && self.span_samples == other.span_samples
            && self.span_count == other.span_count
            && self.stems == other.stems
    }

    /// The four-stem model's header for a track of `frames`, on the full
    /// geometry's grid.
    pub fn for_track(frames: u64) -> Self {
        Self::for_model(
            &ModelIdentity::BS_ROFORMER_4STEM,
            CHUNK_STEP as u64,
            frames,
            &STEM_NAMES,
        )
    }

    /// The header of a track of `frames` separated by `identity` into
    /// `stems`, cached in spans of `span_samples` — the step of the chunk
    /// geometry that separator's full-quality stream runs at, since one
    /// forward finishes one span and a span is written whole.
    pub fn for_model(
        identity: &ModelIdentity,
        span_samples: u64,
        frames: u64,
        stems: &[&str],
    ) -> Self {
        Self {
            model_id: identity.model_id.to_string(),
            checkpoint: identity.checkpoint.to_string(),
            checkpoint_sha256: identity.checkpoint_sha256.to_string(),
            license: identity.license.to_string(),
            source: identity.source.to_string(),
            sample_rate: SAMPLE_RATE,
            frames,
            span_samples,
            // TRACK spans, not model chunks: with the reference's leading
            // reflect pad the first model chunk finalizes only padding, so the
            // padded chunk index and the track span index differ by one. The
            // cache is addressed in track coordinates (`start / span_samples`).
            // A grid of zero has no spans; `open` makes such an entry and
            // `write_span` refuses it, rather than this dividing by it.
            span_count: if span_samples == 0 {
                0
            } else {
                frames.div_ceil(span_samples)
            },
            stems: stems.iter().map(|lane| lane.to_string()).collect(),
        }
    }

    fn encode(&self) -> String {
        let mut out = String::new();
        for (key, value) in [
            ("model_id", self.model_id.clone()),
            ("checkpoint", self.checkpoint.clone()),
            ("checkpoint_sha256", self.checkpoint_sha256.clone()),
            ("license", self.license.clone()),
            ("source", self.source.clone()),
            ("sample_rate", self.sample_rate.to_string()),
            ("frames", self.frames.to_string()),
            ("span_samples", self.span_samples.to_string()),
            ("span_count", self.span_count.to_string()),
        ] {
            // Values are model constants and integers, never newlines; assert
            // rather than silently write an unparseable header.
            debug_assert!(!value.contains('\n'));
            out.push_str(key);
            out.push('=');
            out.push_str(&value);
            out.push('\n');
        }
        // The four-stem lanes are never written. Builds that know nothing of
        // lanes compare this text byte for byte and delete an entry that
        // differs, so that model's header must stay the nine lines above and
        // nothing more; any other list is a line of its own.
        if !is_four_stem_lanes(&self.stems) {
            debug_assert!(check_lanes(&self.stems).is_ok());
            out.push_str("stems=");
            out.push_str(&self.stems.join(","));
            out.push('\n');
        }
        out
    }

    fn decode(text: &str) -> Result<CacheHeader> {
        let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                fields.insert(key, value);
            }
        }
        let get = |key: &str| -> Result<String> {
            fields
                .get(key)
                .map(|v| v.to_string())
                .ok_or_else(|| CacheError::Mismatch(format!("header has no '{key}'")))
        };
        let num = |key: &str| -> Result<u64> {
            get(key)?
                .parse()
                .map_err(|_| CacheError::Mismatch(format!("header '{key}' is not a number")))
        };
        Ok(CacheHeader {
            model_id: get("model_id")?,
            checkpoint: get("checkpoint")?,
            checkpoint_sha256: get("checkpoint_sha256")?,
            license: get("license")?,
            source: get("source")?,
            sample_rate: num("sample_rate")? as u32,
            frames: num("frames")?,
            span_samples: num("span_samples")?,
            span_count: num("span_count")?,
            // Every header written before lanes were data has no such line,
            // and every one of them is a four-stem entry.
            stems: match fields.get("stems") {
                Some(list) => list.split(',').map(|lane| lane.to_string()).collect(),
                None => STEM_NAMES.iter().map(|lane| lane.to_string()).collect(),
            },
        })
    }
}

/// A digest is one directory name and nothing else: letters and digits. Not a
/// separator, not a dot, and not the colon that makes `D:` a drive when it
/// is joined to a root.
fn is_bare_digest(digest: &str) -> bool {
    !digest.is_empty() && digest.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Builds that are still in use compare the four-stem header byte for byte,
/// ignore any lane line and DELETE what differs. So under that model's id a
/// header is the one [`CacheHeader::for_track`] writes -- its frozen licence
/// and source lines over the four lanes in their order -- or it is refused
/// before it can reach a disk those builds share.
fn check_four_stem_header(header: &CacheHeader) -> Result<()> {
    if header.model_id != crate::MODEL_ID {
        return Ok(());
    }
    if header.license != crate::CACHE_HEADER_LICENSE
        || header.source != crate::CACHE_HEADER_SOURCE
        || !is_four_stem_lanes(&header.stems)
    {
        return Err(CacheError::Mismatch(format!(
            "a {} header is built by CacheHeader::for_track and no other way",
            crate::MODEL_ID
        )));
    }
    Ok(())
}

/// The header, written beside and renamed into place: no reader, and no
/// crash, ever finds a header that is there and empty.
fn write_header(dir: &Path, header: &CacheHeader) -> std::io::Result<()> {
    let fresh = dir.join("header.tmp");
    let mut file = File::create(&fresh)?;
    file.write_all(header.encode().as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&fresh, dir.join("header"))
}

/// Whether the entry's span record claims anything at all.
fn records_a_span(dir: &Path) -> bool {
    std::fs::read(dir.join("spans")).is_ok_and(|spans| spans.iter().any(|present| *present != 0))
}

/// An entry this model made of another rendering of the track goes the way
/// an evicted one does: renamed first, so that whatever interrupts the
/// delete, nothing is left under the digest that could be opened as audio.
/// Only the rename has to succeed; the next prune finishes a delete that did
/// not.
fn replace_stale(root: &Path, digest: &str) -> std::io::Result<()> {
    let to = root.join(format!("{EVICTING_PREFIX}{digest}"));
    let _ = std::fs::remove_dir_all(&to);
    std::fs::rename(root.join(digest), &to)?;
    let _ = std::fs::remove_dir_all(&to);
    Ok(())
}

/// Whether `digest` is separated end to end under `root`, WITHOUT touching a
/// byte of it.
///
/// [`StemCache::open`] is the separator's door: it creates the entry, sizes
/// a sparse file per lane to the whole track and REPLACES one the same model
/// made of another rendering. That is right for a caller about to write spans and wrong for
/// one that only wants to know — a deck served its stems from the store never
/// separates locally, and opening the entry to ask about it would leave an
/// empty one behind on every load and put it in front of the budget.
///
/// The same checks `open` makes before it trusts an entry, in the same order:
/// the header must be this exact track under this exact model, every span
/// byte must be set, and the gains and every lane must be the size that
/// header makes them -- a span record beside missing audio is what an
/// interrupted delete leaves. Anything unreadable, short or stale reads as "not
/// complete" — the question is only ever asked to decide whether work can be
/// SKIPPED, so uncertainty has to answer no.
pub fn is_complete_on_disk(root: impl AsRef<Path>, digest: &str, header: &CacheHeader) -> bool {
    if !is_bare_digest(digest)
        || check_lanes(&header.stems).is_err()
        || check_four_stem_header(header).is_err()
    {
        return false;
    }
    let dir = root.as_ref().join(digest);
    let Ok(text) = std::fs::read_to_string(dir.join("header")) else {
        return false;
    };
    if !CacheHeader::decode(&text).is_ok_and(|existing| existing.same_separation(header)) {
        return false;
    }
    let Ok(spans) = std::fs::read(dir.join("spans")) else {
        return false;
    };
    let sized = |name: &str, bytes: u64| {
        std::fs::metadata(dir.join(name)).is_ok_and(|meta| meta.is_file() && meta.len() == bytes)
    };
    spans.len() as u64 == header.span_count
        && header.span_count > 0
        && spans.iter().all(|present| *present != 0)
        && sized("gains", header.span_count * header.stems.len() as u64 * 4)
        && header
            .stems
            .iter()
            .all(|lane| sized(&format!("{lane}.pcm"), header.frames * FRAME_BYTES))
}

/// A per-track cache directory, opened for read+write.
pub struct StemCache {
    dir: PathBuf,
    header: CacheHeader,
    present: Vec<bool>,
    /// Per-span, per-lane normalization peak, `span * lanes + lane`.
    gains: Vec<f32>,
    /// One per lane, in the header's order.
    files: Vec<File>,
    spans_file: File,
    gains_file: File,
}

impl StemCache {
    /// Opens (or creates) the cache entry for `digest`.
    ///
    /// An entry this same model made of another rendering of the track --
    /// another checkpoint, length or span grid -- is REPLACED, not silently
    /// reused. An entry ANOTHER model made, or one with other lanes, is
    /// refused and left exactly as it is: no caller means to put two
    /// separators' work in one directory, so finding one there is a wrong
    /// root or a wrong header, and the answer to a mistake is never to
    /// delete hours of separation.
    ///
    /// An entry is trusted for what it can show. One with no header, or
    /// whose span record, gains or lanes are not the size this header makes
    /// them, is what an interrupted delete or move leaves behind: it is
    /// adopted with every span forgotten, never read back as audio.
    pub fn open(root: impl AsRef<Path>, digest: &str, header: CacheHeader) -> Result<StemCache> {
        if !is_bare_digest(digest) {
            return Err(CacheError::Mismatch(format!(
                "digest {digest:?} is not a bare content hash"
            )));
        }
        // Before anything is created or replaced: a lane name is about to
        // become a file name, and a four-stem header is about to be read by
        // builds that delete what they do not recognise.
        check_lanes(&header.stems)?;
        check_four_stem_header(&header)?;
        let root = root.as_ref();
        let dir = root.join(digest);
        let header_path = dir.join("header");
        if header_path.is_file() {
            let mut text = String::new();
            File::open(&header_path)?.read_to_string(&mut text)?;
            match CacheHeader::decode(&text) {
                Ok(existing) if existing.same_separation(&header) => {
                    if existing != header {
                        // Same stems, newer words about them: the record is
                        // brought up to date beside the audio, which stays
                        // exactly where it is.
                        write_header(&dir, &header)?;
                    }
                }
                Ok(existing)
                    if existing.model_id != header.model_id || existing.stems != header.stems =>
                {
                    return Err(CacheError::Mismatch(format!(
                        "{} holds {} ({}), not {} ({}); it is left as it is",
                        dir.display(),
                        existing.model_id,
                        existing.stems.join(","),
                        header.model_id,
                        header.stems.join(",")
                    )));
                }
                Ok(_) => replace_stale(root, digest)?,
                Err(_) => {
                    // A header nothing can read says nothing about the audio
                    // beside it. With spans recorded that audio may be
                    // somebody's hours; with none there is nothing to lose.
                    if records_a_span(&dir) {
                        return Err(CacheError::Mismatch(format!(
                            "{} has a header that cannot be read and spans on record; it is \
                             left as it is",
                            dir.display()
                        )));
                    }
                    replace_stale(root, digest)?;
                }
            }
        }
        std::fs::create_dir_all(&dir)?;
        let adopted = !header_path.is_file();

        let span_count = header.span_count as usize;
        let gain_bytes = (span_count * header.stems.len() * 4) as u64;
        let lane_bytes = header.frames * FRAME_BYTES;
        let open_in_place = |name: &str| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(name))
        };
        let mut spans_file = open_in_place("spans")?;
        let mut gains_file = open_in_place("gains")?;
        let mut files = Vec::with_capacity(header.stems.len());
        for lane in &header.stems {
            files.push(open_in_place(&format!("{lane}.pcm"))?);
        }
        let sized = |file: &File, bytes: u64| file.metadata().is_ok_and(|meta| meta.len() == bytes);
        let whole = !adopted
            && sized(&spans_file, span_count as u64)
            && sized(&gains_file, gain_bytes)
            && files.iter().all(|file| sized(file, lane_bytes));
        if !whole {
            // Forgotten BEFORE the header is written or a file is resized, so
            // that no crash from here on can leave a record of spans beside
            // files that do not hold them.
            spans_file.set_len(0)?;
            spans_file.sync_data()?;
        }
        if adopted {
            write_header(&dir, &header)?;
        }

        spans_file.set_len(span_count as u64)?;
        let mut present_bytes = vec![0u8; span_count];
        spans_file.seek(SeekFrom::Start(0))?;
        spans_file.read_exact(&mut present_bytes)?;

        gains_file.set_len(gain_bytes)?;
        let mut gain_raw = vec![0u8; gain_bytes as usize];
        gains_file.seek(SeekFrom::Start(0))?;
        gains_file.read_exact(&mut gain_raw)?;
        let gains: Vec<f32> = gain_raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        for file in &files {
            file.set_len(lane_bytes)?;
        }

        // Opening IS using: the LRU order the budget prunes by is the order
        // tracks were last put on a deck, not the order they were separated.
        touch_used(&dir);

        Ok(StemCache {
            dir,
            header,
            present: present_bytes.into_iter().map(|b| b != 0).collect(),
            gains,
            files,
            spans_file,
            gains_file,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn header(&self) -> &CacheHeader {
        &self.header
    }

    /// The lanes this entry holds, in the order every lane-generic call
    /// takes and returns them.
    pub fn lanes(&self) -> &[String] {
        &self.header.stems
    }

    pub fn lane_count(&self) -> usize {
        self.files.len()
    }

    /// Where `name` sits among [`lanes`](Self::lanes), if the entry has it.
    pub fn lane_index(&self, name: &str) -> Option<usize> {
        self.header.stems.iter().position(|lane| lane == name)
    }

    pub fn span_count(&self) -> usize {
        self.present.len()
    }

    pub fn has_span(&self, span: usize) -> bool {
        self.present.get(span).copied().unwrap_or(false)
    }

    /// True once every span has been written.
    pub fn is_complete(&self) -> bool {
        !self.present.is_empty() && self.present.iter().all(|p| *p)
    }

    pub fn missing_spans(&self) -> impl Iterator<Item = usize> + '_ {
        self.present
            .iter()
            .enumerate()
            .filter_map(|(i, present)| (!*present).then_some(i))
    }

    fn span_range(&self, span: usize) -> (u64, u64) {
        let start = span as u64 * self.header.span_samples;
        let end = (start + self.header.span_samples).min(self.header.frames);
        (start, end.max(start))
    }

    /// A [`StemSet`] is four lanes in `Stem::ALL` order and says so nowhere
    /// but in its type, so the calls that speak it serve an entry with
    /// exactly those lanes and refuse any other: handing the third of three
    /// lanes back as "other" would be a wrong answer with nothing to show
    /// for it.
    fn four_stems_only(&self, call: &str) -> Result<()> {
        if is_four_stem_lanes(&self.header.stems) {
            return Ok(());
        }
        Err(CacheError::Mismatch(format!(
            "{call} speaks the four-stem set, this entry holds {}",
            self.header.stems.join(",")
        )))
    }

    /// Writes one finished span. `start` is in track frames and must land on a
    /// span boundary.
    pub fn write_span(&mut self, start: usize, stems: &StemSet) -> Result<()> {
        self.four_stems_only("write_span")?;
        self.write_span_lanes(start, stems)
    }

    /// Writes one finished span of an entry with any lanes: one buffer per
    /// lane, in the entry's order. `start` is in track frames and must land
    /// on a span boundary.
    pub fn write_span_lanes(&mut self, start: usize, lanes: &[StereoBuf]) -> Result<()> {
        if lanes.len() != self.files.len() {
            return Err(CacheError::Mismatch(format!(
                "a span of {} lanes was handed to an entry of {}",
                lanes.len(),
                self.files.len()
            )));
        }
        if self.header.span_samples == 0 {
            return Err(CacheError::Mismatch("span_samples is zero".into()));
        }
        if start as u64 % self.header.span_samples != 0 {
            return Err(CacheError::Mismatch(format!(
                "span start {start} is not a multiple of {}",
                self.header.span_samples
            )));
        }
        let span = start / self.header.span_samples as usize;
        if span >= self.present.len() {
            return Err(CacheError::Mismatch(format!(
                "span {span} beyond span_count {}",
                self.present.len()
            )));
        }
        let (from, to) = self.span_range(span);
        let frames = (to - from) as usize;
        // A span is written whole. The slot is exactly one step of the full
        // geometry (the last slot, what is left of the track), and audio of
        // any other length — a shorter chunk geometry's span, a run cut off
        // mid-span — would be zero-filled to the slot and read back on the
        // next load as separated silence.
        for (index, stem) in lanes.iter().enumerate() {
            if stem.left.len() != frames || stem.right.len() != frames {
                return Err(CacheError::Mismatch(format!(
                    "span {span} stem {index} holds {}/{} frames, the slot holds {frames}",
                    stem.left.len(),
                    stem.right.len()
                )));
            }
        }
        let mut buf = vec![0u8; frames * FRAME_BYTES as usize];
        for (index, stem) in lanes.iter().enumerate() {
            let n = frames;
            let peak = stem.left[..n]
                .iter()
                .chain(&stem.right[..n])
                .fold(0.0f32, |a, v| a.max(v.abs()));
            let scale = if peak > 0.0 { 1.0 / peak } else { 1.0 };
            for frame in 0..n {
                let l = quantize(stem.left[frame] * scale);
                let r = quantize(stem.right[frame] * scale);
                let at = frame * 4;
                buf[at..at + 2].copy_from_slice(&l.to_le_bytes());
                buf[at + 2..at + 4].copy_from_slice(&r.to_le_bytes());
            }
            let file = &mut self.files[index];
            file.seek(SeekFrom::Start(from * FRAME_BYTES))?;
            file.write_all(&buf)?;
            let slot = span * lanes.len() + index;
            self.gains[slot] = if peak > 0.0 { peak } else { 1.0 };
            self.gains_file.seek(SeekFrom::Start(slot as u64 * 4))?;
            self.gains_file.write_all(&self.gains[slot].to_le_bytes())?;
        }
        // The presence flag is written LAST, and only once the audio it
        // vouches for has reached the disk, so a crash or a power cut
        // mid-write leaves the span marked absent and it is simply
        // recomputed.
        for file in self.files.iter_mut() {
            file.sync_data()?;
        }
        self.gains_file.sync_data()?;
        self.spans_file.seek(SeekFrom::Start(span as u64))?;
        self.spans_file.write_all(&[1u8])?;
        self.spans_file.flush()?;
        self.present[span] = true;
        Ok(())
    }

    /// Decodes one lane of one span into `left` and `right`, each exactly the
    /// span long. `buf` is the caller's scratch, the span's size in bytes.
    fn decode_span_lane(
        &mut self,
        span: usize,
        lane: usize,
        buf: &mut [u8],
        left: &mut [f32],
        right: &mut [f32],
    ) -> Result<()> {
        let (from, _) = self.span_range(span);
        let gain = self.gains[span * self.files.len() + lane];
        let file = &mut self.files[lane];
        file.seek(SeekFrom::Start(from * FRAME_BYTES))?;
        file.read_exact(buf)?;
        for frame in 0..left.len() {
            let at = frame * 4;
            left[frame] = dequantize(i16::from_le_bytes([buf[at], buf[at + 1]])) * gain;
            right[frame] = dequantize(i16::from_le_bytes([buf[at + 2], buf[at + 3]])) * gain;
        }
        Ok(())
    }

    /// Decodes the given lanes across the whole track, one buffer per lane
    /// asked for, reading no file but theirs. Errors if any span is missing.
    fn decode_track(&mut self, lanes: &[usize]) -> Result<Vec<StereoBuf>> {
        if let Some(span) = self.missing_spans().next() {
            return Err(CacheError::Mismatch(format!("span {span} is missing")));
        }
        let frames = self.header.frames as usize;
        let mut out: Vec<StereoBuf> = lanes.iter().map(|_| StereoBuf::silence(frames)).collect();
        let mut buf = Vec::new();
        for span in 0..self.span_count() {
            let (from, to) = self.span_range(span);
            let (from, to) = (from as usize, to as usize);
            buf.resize((to - from) * FRAME_BYTES as usize, 0u8);
            for (dst, lane) in out.iter_mut().zip(lanes) {
                let StereoBuf { left, right } = dst;
                self.decode_span_lane(
                    span,
                    *lane,
                    &mut buf,
                    &mut left[from..to],
                    &mut right[from..to],
                )?;
            }
        }
        Ok(out)
    }

    /// Reads one span back, or `None` if it has not been written.
    pub fn read_span(&mut self, span: usize) -> Result<Option<StemSet>> {
        self.four_stems_only("read_span")?;
        match self.read_span_lanes(span)? {
            Some(lanes) => Ok(Some(into_stem_set(lanes)?)),
            None => Ok(None),
        }
    }

    /// Reads one span of an entry with any lanes back, one buffer per lane in
    /// the entry's order, or `None` if it has not been written.
    pub fn read_span_lanes(&mut self, span: usize) -> Result<Option<Vec<StereoBuf>>> {
        if !self.has_span(span) {
            return Ok(None);
        }
        let (from, to) = self.span_range(span);
        let frames = (to - from) as usize;
        let mut out: Vec<StereoBuf> = (0..self.files.len())
            .map(|_| StereoBuf::silence(frames))
            .collect();
        let mut buf = vec![0u8; frames * FRAME_BYTES as usize];
        for (index, lane) in out.iter_mut().enumerate() {
            let StereoBuf { left, right } = lane;
            self.decode_span_lane(span, index, &mut buf, left, right)?;
        }
        Ok(Some(out))
    }

    /// Reads a whole track back, for the "already separated, just play it"
    /// path. Errors if any span is missing.
    pub fn read_all(&mut self) -> Result<StemSet> {
        self.four_stems_only("read_all")?;
        into_stem_set(self.read_all_lanes()?)
    }

    /// Reads a whole track of an entry with any lanes back, one buffer per
    /// lane in the entry's order. Errors if any span is missing.
    pub fn read_all_lanes(&mut self) -> Result<Vec<StereoBuf>> {
        let every: Vec<usize> = (0..self.files.len()).collect();
        self.decode_track(&every)
    }

    /// Reads ONE lane of the whole track back, by name, without opening a
    /// byte of the others: a player that wants the vocals of a four-lane
    /// entry pays for a quarter of it, in time and in memory. Errors if the
    /// entry has no such lane or any span is missing.
    pub fn read_lane(&mut self, name: &str) -> Result<StereoBuf> {
        let Some(lane) = self.lane_index(name) else {
            return Err(CacheError::Mismatch(format!(
                "entry has no lane {name:?}, it holds {}",
                self.header.stems.join(",")
            )));
        };
        let mut only = self.decode_track(&[lane])?;
        Ok(only.remove(0))
    }
}

/// Four lanes as the four-stem set. The buffers are moved, not copied.
fn into_stem_set(lanes: Vec<StereoBuf>) -> Result<StemSet> {
    lanes.try_into().map_err(|lanes: Vec<StereoBuf>| {
        CacheError::Mismatch(format!("{} lanes do not make a four-stem set", lanes.len()))
    })
}

// ---------------------------------------------------------------------------
// keeping the root inside a budget
// ---------------------------------------------------------------------------

/// Default ceiling for the whole cache root.
///
/// A track costs about 700 KB of separated audio per second, so this is
/// roughly six hours of separated music: a working DJ set several times over,
/// and small enough that a laptop does not quietly lose a tenth of its disk to
/// a directory nobody looks at. The number is the caller's to choose —
/// [`prune`] takes it as an argument — but this is the one the client uses.
pub const DEFAULT_BUDGET_BYTES: u64 = 6 * 1024 * 1024 * 1024;

/// What one [`prune`] did. `before`/`after` are the whole root's footprint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub before: u64,
    pub after: u64,
    /// `(digest, bytes)` per evicted entry, in the order they went.
    pub removed: Vec<(String, u64)>,
}

impl PruneReport {
    pub fn freed(&self) -> u64 {
        self.before.saturating_sub(self.after)
    }
}

/// Bring the cache root inside `budget_bytes` by dropping whole track entries,
/// least recently used first.
///
/// `keep` names digests that must survive whatever the budget says — the
/// tracks on the decks right now. Their bytes still COUNT toward the total, so
/// a budget smaller than what is pinned simply evicts everything else and
/// stops; the alternative (evicting the track under the needle) is not an
/// improvement anyone would thank us for.
///
/// Whole entries, never spans: a track is either cached or it is not, and half
/// an entry would mean re-separating the gaps at exactly the moment the
/// operator is playing them. Eviction renames before it deletes, so an
/// interrupted prune leaves nothing that could be read back as audio; the
/// leftover is finished off by the next prune.
pub fn prune(root: impl AsRef<Path>, budget_bytes: u64, keep: &[&str]) -> Result<PruneReport> {
    let root = root.as_ref();
    let mut report = PruneReport::default();
    let Ok(listing) = std::fs::read_dir(root) else {
        return Ok(report);
    };
    // `(used, digest, bytes)`, entries that MAY go.
    let mut candidates: Vec<(u64, String, u64)> = Vec::new();
    let mut total = 0u64;
    for entry in listing.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(EVICTING_PREFIX) {
            // A prune that died halfway. Finish it and do not count it.
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        if name.starts_with('.') {
            continue;
        }
        // Only what is an entry is ever a candidate: a directory somebody
        // else put under the root is not this cache's to count or delete.
        if !path.join("header").is_file() {
            continue;
        }
        let bytes = entry_bytes(&path);
        total += bytes;
        if keep.contains(&name) {
            continue;
        }
        candidates.push((used_stamp(&path), name.to_string(), bytes));
    }
    report.before = total;
    report.after = total;
    if total <= budget_bytes {
        return Ok(report);
    }
    // Oldest use first; the digest breaks ties so a prune is deterministic.
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for (_, digest, bytes) in candidates {
        if report.after <= budget_bytes {
            break;
        }
        if evict(root, &digest).is_err() {
            continue;
        }
        report.after = report.after.saturating_sub(bytes);
        report.removed.push((digest, bytes));
    }
    Ok(report)
}

/// Bytes one entry occupies. Stem files are pre-sized and filled span by span,
/// so on a filesystem with holes the logical length is an overstatement of
/// what is actually on the disk; count blocks where the platform reports them.
fn entry_bytes(dir: &Path) -> u64 {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in listing.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            total += meta.blocks() * 512;
        }
        #[cfg(not(unix))]
        {
            total += meta.len();
        }
    }
    total
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stamp an entry as used now. Best effort: a cache that cannot record its own
/// LRU order is still a cache, it just prunes in digest order.
fn touch_used(dir: &Path) {
    let _ = std::fs::write(dir.join(USED_FILE), now_secs().to_le_bytes());
}

/// When this entry was last opened. An entry written by an older build has no
/// stamp; its directory time is the honest fallback, and zero (evict first) is
/// the fallback for that.
fn used_stamp(dir: &Path) -> u64 {
    if let Ok(bytes) = std::fs::read(dir.join(USED_FILE)) {
        if let Ok(eight) = <[u8; 8]>::try_from(bytes.as_slice()) {
            return u64::from_le_bytes(eight);
        }
    }
    std::fs::metadata(dir)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Rename out of the way, then delete. The rename is the atomic part: after it
/// the entry cannot be opened as a track, so a crash before the delete lands
/// costs disk and nothing else.
fn evict(root: &Path, digest: &str) -> std::io::Result<()> {
    let from = root.join(digest);
    let to = root.join(format!("{EVICTING_PREFIX}{digest}"));
    let _ = std::fs::remove_dir_all(&to);
    std::fs::rename(&from, &to)?;
    std::fs::remove_dir_all(&to)
}

fn quantize(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn dequantize(sample: i16) -> f32 {
    sample as f32 / 32767.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "makepad-stems-cache-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn ramp_stems(frames: usize, seed: f32) -> StemSet {
        let mut set = crate::model::empty_stem_set(frames);
        for (index, stem) in set.iter_mut().enumerate() {
            for frame in 0..frames {
                let t = frame as f32 / frames.max(1) as f32;
                stem.left[frame] = ((t + seed + index as f32 * 0.25) % 1.0) * 2.0 - 1.0;
                stem.right[frame] = -stem.left[frame];
            }
        }
        set
    }

    /// Asking whether a track is already separated must not BUILD the entry
    /// it is asking about. `open` creates the directory, sizes four sparse
    /// stem files and replaces an entry whose header disagrees — all correct
    /// for a separator about to write, all wrong for a caller that only wants
    /// to know. A fetched track never separates locally, so a probe that
    /// created an entry would leave an empty one behind on every load.
    #[test]
    fn a_completeness_probe_creates_nothing() {
        let root = temp_root("probe-creates-nothing");
        let frames = 2 * CHUNK_STEP;
        let header = CacheHeader::for_track(frames as u64);
        let digest = "a".repeat(64);

        assert!(!is_complete_on_disk(&root, &digest, &header));
        assert!(
            !root.join(&digest).exists(),
            "the probe must not create the entry it probes"
        );
    }

    /// What the probe is FOR: a track separated in an earlier session is
    /// complete on disk, and that is true whoever is asking — the separator,
    /// or a deck being served its stems from the store.
    #[test]
    fn a_completeness_probe_reads_what_separation_left() {
        let root = temp_root("probe-truth");
        let frames = 2 * CHUNK_STEP;
        let header = CacheHeader::for_track(frames as u64);
        let digest = "b".repeat(64);
        {
            let mut cache = StemCache::open(&root, &digest, header.clone()).unwrap();
            cache.write_span(0, &ramp_stems(CHUNK_STEP, 0.0)).unwrap();
            // Half a track is not a track: the bake reads every span.
            assert!(!cache.is_complete());
        }
        assert!(!is_complete_on_disk(&root, &digest, &header));
        {
            let mut cache = StemCache::open(&root, &digest, header.clone()).unwrap();
            cache
                .write_span(CHUNK_STEP, &ramp_stems(CHUNK_STEP, 0.5))
                .unwrap();
            assert!(cache.is_complete());
        }
        assert!(is_complete_on_disk(&root, &digest, &header));

        // A DIFFERENT track under the same name is not this one. The header
        // is the check `open` makes before it trusts an entry, and the probe
        // owes callers the same one — calling a stale entry complete would
        // hand the karaoke bake another track's vocals.
        let other = CacheHeader::for_track((frames + CHUNK_STEP) as u64);
        assert!(!is_complete_on_disk(&root, &digest, &other));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The cache is on the full geometry's grid and nothing else may land in
    /// it: a bridge-length span, or a span cut short, written into a slot
    /// would be padded with silence and served as stems on the next load.
    #[test]
    fn a_span_that_does_not_fill_its_slot_is_refused() {
        let root = temp_root("wrong-length");
        let frames = 2 * CHUNK_STEP + 100;
        let header = CacheHeader::for_track(frames as u64);
        let mut cache = StemCache::open(&root, "c0ffee", header).unwrap();
        assert_eq!(cache.span_count(), 3);
        assert!(cache.write_span(0, &ramp_stems(CHUNK_STEP - 1, 0.0)).is_err());
        assert!(cache.write_span(0, &ramp_stems(CHUNK_STEP + 1, 0.0)).is_err());
        assert!(
            cache.write_span(0, &ramp_stems(ChunkGeometry::BRIDGE.step, 0.0)).is_err(),
            "a bridge span must never reach the cache"
        );
        assert!(!cache.has_span(0), "a refused span leaves the slot empty");
        cache.write_span(0, &ramp_stems(CHUNK_STEP, 0.0)).unwrap();
        cache.write_span(CHUNK_STEP, &ramp_stems(CHUNK_STEP, 0.1)).unwrap();
        // The tail slot holds what is left of the track, and only that.
        assert!(cache.write_span(2 * CHUNK_STEP, &ramp_stems(CHUNK_STEP, 0.2)).is_err());
        cache.write_span(2 * CHUNK_STEP, &ramp_stems(100, 0.2)).unwrap();
        assert!(cache.is_complete());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The licence and source lines are words ABOUT the model, not the model.
    /// A build that words them differently must find the track it separated
    /// yesterday, not delete it: separation costs a third of a track's length,
    /// and a library of them costs hours.
    #[test]
    fn a_model_described_in_new_words_keeps_the_stems_it_made() {
        let root = temp_root("reworded");
        let frames = 2 * CHUNK_STEP;
        let new = CacheHeader::for_track(frames as u64);
        let mut old = new.clone();
        old.license = "an earlier wording".to_string();
        old.source = "an earlier address".to_string();
        let digest = "b".repeat(64);
        {
            let mut cache = StemCache::open(&root, &digest, new.clone()).unwrap();
            for span in 0..cache.span_count() {
                cache
                    .write_span(span * CHUNK_STEP, &ramp_stems(CHUNK_STEP, 0.3))
                    .unwrap();
            }
            assert!(cache.is_complete());
        }
        // What a build with other words for the model left behind. No
        // caller of this one can write it, so it is put there by hand.
        std::fs::write(root.join(&digest).join("header"), old.encode()).unwrap();
        assert_ne!(old, new);
        assert!(is_complete_on_disk(&root, &digest, &new), "the probe still finds it");
        let cache = StemCache::open(&root, &digest, new.clone()).unwrap();
        assert!(cache.is_complete(), "and opening it keeps every span");
        let text = std::fs::read_to_string(cache.dir().join("header")).unwrap();
        assert_eq!(
            CacheHeader::decode(&text).unwrap(),
            new,
            "with the record brought up to date"
        );
        assert!(!cache.dir().join("header.tmp").exists(), "and nothing left beside it");
        drop(cache);

        // Another checkpoint is still another separation.
        let mut other = new.clone();
        other.checkpoint_sha256 = "0".repeat(64);
        assert!(!is_complete_on_disk(&root, &digest, &other));
        let replaced = StemCache::open(&root, &digest, other).unwrap();
        assert!(!replaced.is_complete(), "and its entry starts again from nothing");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The header's licence line is compared byte for byte by builds that
    /// are still in use, and an entry that differs is one they delete. It
    /// is pinned here so that rewording the statement of record can never
    /// reword this by accident.
    #[test]
    fn the_header_carries_the_line_older_builds_compare_byte_for_byte() {
        let header = CacheHeader::for_track(1_000_000);
        assert_eq!(
            header.license,
            "MIT (ZFTurbo/Music-Source-Separation-Training, (c) 2024 Roman Solovyev)"
        );
        assert!(header.encode().contains(
            "license=MIT (ZFTurbo/Music-Source-Separation-Training, (c) 2024 Roman Solovyev)\n"
        ));
        assert_ne!(header.license, crate::MODEL_LICENSE, "which is not the statement of record");
    }

    #[test]
    fn header_round_trips() {
        let header = CacheHeader::for_track(1_000_000);
        let decoded = CacheHeader::decode(&header.encode()).unwrap();
        assert_eq!(header, decoded);
        assert_eq!(decoded.model_id, crate::MODEL_ID);
        assert_eq!(decoded.checkpoint_sha256, crate::MODEL_SHA256);
    }

    #[test]
    fn spans_round_trip_and_track_completeness() {
        let root = temp_root("round-trip");
        let frames = 3 * CHUNK_STEP + 1234;
        let header = CacheHeader::for_track(frames as u64);
        let mut cache = StemCache::open(&root, "deadbeef", header).unwrap();
        assert!(!cache.is_complete());
        assert_eq!(cache.missing_spans().count(), cache.span_count());

        assert_eq!(cache.span_count(), frames.div_ceil(CHUNK_STEP));
        for span in 0..cache.span_count() {
            let start = span * CHUNK_STEP;
            assert!(start < frames);
            let len = (frames - start).min(CHUNK_STEP);
            let stems = ramp_stems(len, span as f32 * 0.1);
            cache.write_span(start, &stems).unwrap();
            let back = cache.read_span(span).unwrap().unwrap();
            for stem in 0..NUM_STEMS {
                assert_eq!(back[stem].frames(), len, "span {span} stem {stem}");
                for frame in 0..len {
                    let want = stems[stem].left[frame];
                    let got = back[stem].left[frame];
                    assert!(
                        (want - got).abs() < 1.0 / 16384.0,
                        "span {span} stem {stem} frame {frame}: {want} vs {got}"
                    );
                }
            }
        }
        assert!(cache.is_complete());
        assert_eq!(cache.missing_spans().count(), 0);
        let all = cache.read_all().unwrap();
        assert_eq!(all[0].frames(), frames);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn samples_above_full_scale_survive_the_round_trip() {
        // The measured vocals stem of the reference fixture peaks at 1.12, so
        // a plain i16 clamp would silently flat-top it. The per-span peak must
        // carry it back intact.
        let root = temp_root("headroom");
        let frames = CHUNK_STEP;
        let mut cache =
            StemCache::open(&root, "1234abcd", CacheHeader::for_track(frames as u64)).unwrap();
        let mut stems = crate::model::empty_stem_set(frames);
        for (index, stem) in stems.iter_mut().enumerate() {
            for frame in 0..frames {
                let phase = frame as f32 * 0.001 + index as f32;
                stem.left[frame] = 1.35 * phase.sin();
                stem.right[frame] = -1.35 * phase.cos();
            }
        }
        cache.write_span(0, &stems).unwrap();
        let back = cache.read_span(0).unwrap().unwrap();
        for stem in 0..NUM_STEMS {
            let mut max = 0.0f32;
            for frame in 0..frames {
                max = max
                    .max((stems[stem].left[frame] - back[stem].left[frame]).abs())
                    .max((stems[stem].right[frame] - back[stem].right[frame]).abs());
            }
            assert!(max < 1e-4, "stem {stem} clipped or drifted by {max:.3e}");
            let peak = back[stem].left.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            assert!(peak > 1.3, "stem {stem} peak came back as {peak}");
        }
        // A silent stem must not blow up on the zero-peak path.
        let silence = crate::model::empty_stem_set(frames);
        cache.write_span(0, &silence).unwrap();
        let back = cache.read_span(0).unwrap().unwrap();
        assert!(back[0].left.iter().all(|v| *v == 0.0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn presence_survives_reopen() {
        let root = temp_root("reopen");
        let frames = 2 * CHUNK_STEP;
        let header = CacheHeader::for_track(frames as u64);
        {
            let mut cache = StemCache::open(&root, "cafe1234", header.clone()).unwrap();
            cache
                .write_span(CHUNK_STEP, &ramp_stems(CHUNK_STEP, 0.3))
                .unwrap();
        }
        let cache = StemCache::open(&root, "cafe1234", header).unwrap();
        assert!(!cache.has_span(0));
        assert!(cache.has_span(1));
        assert_eq!(cache.missing_spans().collect::<Vec<_>>(), vec![0]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_different_track_length_invalidates_the_entry() {
        let root = temp_root("mismatch");
        let mut cache =
            StemCache::open(&root, "abc123", CacheHeader::for_track(2 * CHUNK_STEP as u64)).unwrap();
        cache
            .write_span(0, &ramp_stems(CHUNK_STEP, 0.0))
            .unwrap();
        drop(cache);
        // Same digest, different geometry: the entry must be rebuilt, not
        // reused with stale audio behind it.
        let cache =
            StemCache::open(&root, "abc123", CacheHeader::for_track(5 * CHUNK_STEP as u64)).unwrap();
        assert!(!cache.has_span(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_a_digest_that_is_a_path() {
        let root = temp_root("path");
        for bad in ["../escape", "a/b", "with.dot", "", "D:", "a:b", "two words", "abcd-1"] {
            assert!(
                StemCache::open(&root, bad, CacheHeader::for_track(CHUNK_STEP as u64)).is_err(),
                "{bad:?} should be refused"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- the budget -------------------------------------------------------

    /// One entry of `spans` spans, fully written, stamped as used at `used`.
    fn filled_entry(root: &Path, digest: &str, spans: usize, used: u64) -> u64 {
        let frames = spans * CHUNK_STEP;
        let mut cache =
            StemCache::open(root, digest, CacheHeader::for_track(frames as u64)).unwrap();
        for span in 0..spans {
            cache
                .write_span(span * CHUNK_STEP, &ramp_stems(CHUNK_STEP, span as f32 * 0.1))
                .unwrap();
        }
        drop(cache);
        std::fs::write(root.join(digest).join(USED_FILE), used.to_le_bytes()).unwrap();
        entry_bytes(&root.join(digest))
    }

    #[test]
    fn a_root_inside_its_budget_is_left_alone() {
        let root = temp_root("budget-idle");
        let bytes = filled_entry(&root, "aaaa1111", 1, 100);
        let report = prune(&root, bytes * 4, &[]).unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(report.before, report.after);
        assert!(root.join("aaaa1111").is_dir());
        // …and an unopened entry still knows when it was last used.
        assert_eq!(used_stamp(&root.join("aaaa1111")), 100);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_least_recently_used_track_goes_first() {
        let root = temp_root("budget-lru");
        // Same size, different last-used stamps.
        let one = filled_entry(&root, "aaaa1111", 1, 300);
        filled_entry(&root, "bbbb2222", 1, 100);
        filled_entry(&root, "cccc3333", 1, 200);
        assert!(one > 0, "an entry with a span in it occupies disk");

        // Room for one entry: the two oldest go, oldest first.
        let report = prune(&root, one, &[]).unwrap();
        assert_eq!(
            report.removed.iter().map(|(d, _)| d.as_str()).collect::<Vec<_>>(),
            vec!["bbbb2222", "cccc3333"],
            "{report:?}"
        );
        assert!(report.after <= one, "{report:?}");
        assert_eq!(report.freed(), report.before - report.after);
        assert!(root.join("aaaa1111").is_dir(), "the newest use survives");
        assert!(!root.join("bbbb2222").exists());
        assert!(!root.join("cccc3333").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_track_on_a_deck_is_never_evicted() {
        let root = temp_root("budget-pinned");
        // The pinned one is BOTH the oldest and the biggest: nothing but the
        // pin can save it.
        filled_entry(&root, "aaaa1111", 3, 10);
        let young = filled_entry(&root, "bbbb2222", 1, 900);
        // A budget of nothing: everything unpinned has to go, and the pinned
        // entry stays even though the root is still over.
        let report = prune(&root, 0, &["aaaa1111"]).unwrap();
        assert_eq!(
            report.removed.iter().map(|(d, _)| d.as_str()).collect::<Vec<_>>(),
            vec!["bbbb2222"],
            "{report:?}"
        );
        assert_eq!(report.freed(), young);
        assert!(root.join("aaaa1111").is_dir(), "the deck's own track survives");
        // The spans of the survivor are untouched — a pin is not a rewrite.
        let cache =
            StemCache::open(&root, "aaaa1111", CacheHeader::for_track(3 * CHUNK_STEP as u64))
                .unwrap();
        assert!(cache.is_complete());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_interrupted_eviction_leaves_nothing_readable_behind() {
        let root = temp_root("budget-crash");
        filled_entry(&root, "aaaa1111", 1, 100);
        // Exactly what a crash between the rename and the delete leaves.
        std::fs::rename(
            root.join("aaaa1111"),
            root.join(format!("{EVICTING_PREFIX}aaaa1111")),
        )
        .unwrap();
        // The half-deleted entry is not a track: it does not count toward the
        // budget, and the next prune finishes the job.
        let report = prune(&root, u64::MAX, &[]).unwrap();
        assert_eq!(report.before, 0, "{report:?}");
        assert!(!root.join(format!("{EVICTING_PREFIX}aaaa1111")).exists());
        // And the digest it came from opens as a fresh, empty entry rather
        // than one that claims spans it no longer has.
        let cache =
            StemCache::open(&root, "aaaa1111", CacheHeader::for_track(CHUNK_STEP as u64)).unwrap();
        assert!(!cache.has_span(0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn opening_an_entry_is_what_marks_it_used() {
        let root = temp_root("budget-touch");
        filled_entry(&root, "aaaa1111", 1, 100);
        let before = used_stamp(&root.join("aaaa1111"));
        assert_eq!(before, 100);
        let _ = StemCache::open(&root, "aaaa1111", CacheHeader::for_track(CHUNK_STEP as u64))
            .unwrap();
        let after = used_stamp(&root.join("aaaa1111"));
        assert!(after >= now_secs() - 5 && after > before, "{before} -> {after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_a_span_start_off_the_grid() {
        let root = temp_root("grid");
        let mut cache =
            StemCache::open(&root, "0f0f", CacheHeader::for_track(2 * CHUNK_STEP as u64)).unwrap();
        assert!(cache.write_span(7, &ramp_stems(16, 0.0)).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- lanes as data ------------------------------------------------------

    /// A second separator as the cache sees one: another identity, one lane,
    /// and the 4-second grid of an 8-second chunk. The words are made up;
    /// only the shape matters here.
    const VOCAL_MODEL: ModelIdentity = ModelIdentity {
        model_id: "vocal-model-under-test",
        checkpoint: "vocal-model-under-test.ckpt",
        checkpoint_sha256: "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
        license: "a licence line",
        source: "an address",
    };
    const VOCAL_STEP: usize = 176_400;

    fn vocal_header(frames: usize) -> CacheHeader {
        CacheHeader::for_model(&VOCAL_MODEL, VOCAL_STEP as u64, frames as u64, &["vocals"])
    }

    fn ramp_lanes(lanes: usize, frames: usize, seed: f32) -> Vec<StereoBuf> {
        (0..lanes)
            .map(|index| {
                let mut lane = StereoBuf::silence(frames);
                for frame in 0..frames {
                    let t = frame as f32 / frames.max(1) as f32;
                    lane.left[frame] = ((t + seed + index as f32 * 0.25) % 1.0) * 2.0 - 1.0;
                    lane.right[frame] = -lane.left[frame];
                }
                lane
            })
            .collect()
    }

    fn same_bits(a: &StereoBuf, b: &StereoBuf) -> bool {
        let bits = |v: &[f32]| v.iter().map(|s| s.to_bits()).collect::<Vec<u32>>();
        bits(&a.left) == bits(&b.left) && bits(&a.right) == bits(&b.right)
    }

    /// Every one-lane span of a track of `frames`, written.
    fn filled_vocal_entry(root: &Path, digest: &str, frames: usize) -> StemCache {
        let mut cache = StemCache::open(root, digest, vocal_header(frames)).unwrap();
        for span in 0..cache.span_count() {
            let start = span * VOCAL_STEP;
            let len = (frames - start).min(VOCAL_STEP);
            cache
                .write_span_lanes(start, &ramp_lanes(1, len, span as f32 * 0.1))
                .unwrap();
        }
        cache
    }

    /// Builds that know nothing of lanes share cache roots with this one,
    /// compare the header text byte for byte and DELETE an entry that
    /// differs. So the four-stem header is these nine lines and not a byte
    /// more, whatever else a header learns to say about other models.
    #[test]
    fn the_four_stem_header_is_written_byte_for_byte_as_it_always_was() {
        let text = CacheHeader::for_track(1_000_000).encode();
        assert_eq!(
            text,
            "model_id=bs-roformer-4stem\n\
             checkpoint=model_bs_roformer_ep_17_sdr_9.6568.ckpt\n\
             checkpoint_sha256=3e9daecd70aaed5b5a0d1f861cc4d77eaa45afb3fc6301b1cf32c1be0f5868fb\n\
             license=MIT (ZFTurbo/Music-Source-Separation-Training, (c) 2024 Roman Solovyev)\n\
             source=https://github.com/ZFTurbo/Music-Source-Separation-Training\n\
             sample_rate=44100\n\
             frames=1000000\n\
             span_samples=242550\n\
             span_count=5\n"
        );
        // And what reaches the disk is that text, not a rendering of it.
        let root = temp_root("pinned-header");
        let cache = StemCache::open(&root, "feed0001", CacheHeader::for_track(1_000_000)).unwrap();
        assert_eq!(std::fs::read(cache.dir().join("header")).unwrap(), text.as_bytes());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every entry made before lanes were data has no lane line, and every
    /// one of them holds the four stems. A header that does name its lanes
    /// says so on a line of its own, after the nine an older reader knows.
    #[test]
    fn a_header_that_names_no_lanes_means_the_four_stems() {
        let four = CacheHeader::for_track(1_000_000);
        assert_eq!(four.stems, STEM_NAMES);
        assert!(!four.encode().contains("stems"));
        assert_eq!(CacheHeader::decode(&four.encode()).unwrap().stems, STEM_NAMES);

        // Named outright they are the same header, and are still not written.
        let named = format!("{}stems=drums,bass,other,vocals\n", four.encode());
        let decoded = CacheHeader::decode(&named).unwrap();
        assert_eq!(decoded, four);
        assert_eq!(decoded.encode(), four.encode());

        let one = vocal_header(1_000_000);
        assert_eq!(one.stems, ["vocals"]);
        assert_eq!(one.span_samples, VOCAL_STEP as u64);
        assert_eq!(one.span_count, 6);
        assert!(one.encode().ends_with("span_count=6\nstems=vocals\n"), "{}", one.encode());
        assert_eq!(CacheHeader::decode(&one.encode()).unwrap(), one);

        let two = CacheHeader::for_model(&VOCAL_MODEL, 176_400, 1_000_000, &["vocals", "rest"]);
        assert!(two.encode().ends_with("stems=vocals,rest\n"));
        assert_eq!(CacheHeader::decode(&two.encode()).unwrap(), two);
    }

    /// The identity the four-stem header is built from is the crate's own
    /// constants, with the FROZEN licence line rather than the statement of
    /// record.
    #[test]
    fn the_four_stem_identity_is_the_one_for_track_has_always_written() {
        let identity = ModelIdentity::BS_ROFORMER_4STEM;
        assert_eq!(identity.model_id, crate::MODEL_ID);
        assert_eq!(identity.checkpoint, crate::MODEL_CHECKPOINT);
        assert_eq!(identity.checkpoint_sha256, crate::MODEL_SHA256);
        assert_eq!(identity.license, crate::CACHE_HEADER_LICENSE);
        assert_eq!(identity.source, crate::MODEL_SOURCE);
        for frames in [0u64, 1, CHUNK_STEP as u64, CHUNK_STEP as u64 + 1, 8_653_008] {
            assert_eq!(
                CacheHeader::for_model(&identity, CHUNK_STEP as u64, frames, &STEM_NAMES),
                CacheHeader::for_track(frames)
            );
            assert_eq!(
                CacheHeader::for_track(frames).span_count,
                (frames as usize).div_ceil(CHUNK_STEP) as u64
            );
        }
    }

    #[test]
    fn a_single_lane_entry_round_trips_on_its_own_grid() {
        let root = temp_root("one-lane");
        let frames = 3 * VOCAL_STEP + 1234;
        let header = vocal_header(frames);
        let digest = "0123abcd";
        let mut cache = StemCache::open(&root, digest, header.clone()).unwrap();
        assert_eq!(cache.span_count(), 4);
        assert_eq!(cache.lanes(), ["vocals"]);
        assert_eq!(cache.lane_count(), 1);
        assert_eq!(cache.lane_index("vocals"), Some(0));
        assert_eq!(cache.lane_index("drums"), None);
        assert!(!is_complete_on_disk(&root, digest, &header));

        let mut written = Vec::new();
        for span in 0..cache.span_count() {
            let start = span * VOCAL_STEP;
            let len = (frames - start).min(VOCAL_STEP);
            let lanes = ramp_lanes(1, len, span as f32 * 0.1);
            cache.write_span_lanes(start, &lanes).unwrap();
            let back = cache.read_span_lanes(span).unwrap().unwrap();
            assert_eq!(back.len(), 1);
            assert_eq!(back[0].frames(), len, "span {span}");
            for frame in 0..len {
                let (want, got) = (lanes[0].left[frame], back[0].left[frame]);
                assert!(
                    (want - got).abs() < 1.0 / 16384.0,
                    "span {span} frame {frame}: {want} vs {got}"
                );
                assert_eq!(back[0].right[frame], -got);
            }
            written.push(back);
        }
        assert!(cache.is_complete());
        assert!(is_complete_on_disk(&root, digest, &header));

        // One lane on disk, sized to the track, and one gain per span.
        let dir = cache.dir().to_path_buf();
        assert_eq!(
            std::fs::metadata(dir.join("vocals.pcm")).unwrap().len(),
            frames as u64 * FRAME_BYTES
        );
        assert_eq!(std::fs::metadata(dir.join("gains")).unwrap().len(), 4 * 4);
        for stem in ["drums", "bass", "other"] {
            assert!(!dir.join(format!("{stem}.pcm")).exists(), "{stem}");
        }

        // The whole track, as every lane and as the one lane by name.
        let all = cache.read_all_lanes().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].frames(), frames);
        for (span, part) in written.iter().enumerate() {
            let from = span * VOCAL_STEP;
            let to = from + part[0].frames();
            assert_eq!(&all[0].left[from..to], &part[0].left[..], "span {span}");
            assert_eq!(&all[0].right[from..to], &part[0].right[..], "span {span}");
        }
        assert!(same_bits(&cache.read_lane("vocals").unwrap(), &all[0]));
        assert!(cache.read_lane("drums").is_err());
        drop(cache);

        // And it is all still there for the next session.
        let mut reopened = StemCache::open(&root, digest, header).unwrap();
        assert!(reopened.is_complete());
        assert!(same_bits(&reopened.read_lane("vocals").unwrap(), &all[0]));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A slot of this grid holds one step of THIS grid, and the entry's
    /// lanes, all of them; and the calls that speak the four-stem set answer
    /// an entry that is not one with an error, not a panic or a guess.
    #[test]
    fn a_single_lane_entry_refuses_what_is_not_its_own() {
        let root = temp_root("one-lane-refusals");
        let frames = 2 * VOCAL_STEP;
        let mut cache = StemCache::open(&root, "0123abcd", vocal_header(frames)).unwrap();
        assert!(cache.write_span_lanes(0, &ramp_lanes(1, CHUNK_STEP, 0.0)).is_err());
        assert!(cache.write_span_lanes(CHUNK_STEP, &ramp_lanes(1, VOCAL_STEP, 0.0)).is_err());
        assert!(cache.write_span_lanes(0, &ramp_lanes(2, VOCAL_STEP, 0.0)).is_err());
        assert!(cache.write_span_lanes(0, &[]).is_err());
        assert!(cache.write_span(0, &ramp_stems(VOCAL_STEP, 0.0)).is_err());
        assert!(!cache.has_span(0), "a refused span leaves the slot empty");

        cache.write_span_lanes(0, &ramp_lanes(1, VOCAL_STEP, 0.0)).unwrap();
        assert!(cache.read_span(0).is_err());
        assert!(cache.read_all().is_err());
        assert!(cache.read_all_lanes().is_err(), "half a track is not a track");
        assert!(cache.read_lane("vocals").is_err());
        cache.write_span_lanes(VOCAL_STEP, &ramp_lanes(1, VOCAL_STEP, 0.5)).unwrap();
        assert!(cache.read_all().is_err(), "complete, and still not four stems");
        assert_eq!(cache.read_all_lanes().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Same track, same digest, another set of lanes: never the same
    /// separation, even with every other field forced equal. It is why the
    /// two kinds of entry are kept under different roots — the door replaces
    /// what it does not recognise.
    #[test]
    fn a_one_lane_header_is_never_the_four_lane_header_of_the_same_track() {
        let frames = 4 * CHUNK_STEP;
        let four = CacheHeader::for_track(frames as u64);
        let one = vocal_header(frames);
        assert!(!one.same_separation(&four));
        assert!(!four.same_separation(&one));

        let mut only_the_lanes_differ = four.clone();
        only_the_lanes_differ.stems = vec!["vocals".to_string()];
        assert!(!only_the_lanes_differ.same_separation(&four));
        assert!(!four.same_separation(&only_the_lanes_differ));
        assert!(four.same_separation(&four.clone()));

        let root = temp_root("lanes-differ");
        let digest = "d".repeat(64);
        filled_entry(&root, &digest, 4, 100);
        assert!(is_complete_on_disk(&root, &digest, &four));
        assert!(!is_complete_on_disk(&root, &digest, &only_the_lanes_differ));
        assert!(!is_complete_on_disk(&root, &digest, &one));
        assert!(
            root.join(&digest).join("drums.pcm").is_file(),
            "and asking changed nothing"
        );
        // The door refuses both and starts nothing again.
        assert!(StemCache::open(&root, &digest, only_the_lanes_differ).is_err());
        assert!(StemCache::open(&root, &digest, one).is_err());
        assert!(is_complete_on_disk(&root, &digest, &four));
        assert!(root.join(&digest).join("drums.pcm").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The four-stem calls are the lane calls with a fixed shape: the same
    /// bytes reach the disk and the same samples come back, whichever pair a
    /// caller uses on a four-stem entry.
    #[test]
    fn the_lane_calls_and_the_four_stem_calls_agree_on_a_four_stem_entry() {
        let root = temp_root("four-as-lanes");
        let frames = 2 * CHUNK_STEP + 4321;
        let header = CacheHeader::for_track(frames as u64);
        let mut by_set = StemCache::open(&root, "aaaa0001", header.clone()).unwrap();
        let mut by_lanes = StemCache::open(&root, "bbbb0002", header).unwrap();
        assert_eq!(by_lanes.lanes(), STEM_NAMES);
        assert_eq!(by_lanes.lane_count(), NUM_STEMS);
        assert_eq!(by_lanes.lane_index("vocals"), Some(Stem::Vocals.index()));
        for span in 0..by_set.span_count() {
            let start = span * CHUNK_STEP;
            let stems = ramp_stems((frames - start).min(CHUNK_STEP), span as f32 * 0.1);
            by_set.write_span(start, &stems).unwrap();
            by_lanes.write_span_lanes(start, &stems.to_vec()).unwrap();
        }
        let files = ["header", "spans", "gains", "drums.pcm", "bass.pcm", "other.pcm", "vocals.pcm"];
        for file in files {
            assert_eq!(
                std::fs::read(by_set.dir().join(file)).unwrap(),
                std::fs::read(by_lanes.dir().join(file)).unwrap(),
                "{file}"
            );
        }
        for span in 0..by_set.span_count() {
            let set = by_set.read_span(span).unwrap().unwrap();
            let lanes = by_set.read_span_lanes(span).unwrap().unwrap();
            assert_eq!(lanes.len(), NUM_STEMS);
            for stem in 0..NUM_STEMS {
                assert!(same_bits(&set[stem], &lanes[stem]), "span {span} stem {stem}");
            }
        }
        assert!(by_set.read_span(99).unwrap().is_none());
        assert!(by_set.read_span_lanes(99).unwrap().is_none());
        let set = by_lanes.read_all().unwrap();
        let lanes = by_lanes.read_all_lanes().unwrap();
        for stem in Stem::ALL {
            assert!(same_bits(&set[stem.index()], &lanes[stem.index()]), "{stem:?}");
            assert!(
                same_bits(&by_lanes.read_lane(stem.name()).unwrap(), &set[stem.index()]),
                "{stem:?} by name"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A lane name becomes a file name. One that could leave the entry's
    /// directory, split the header's list or name a lane twice is refused
    /// before anything is created.
    #[test]
    fn a_lane_name_that_is_not_a_bare_word_is_refused() {
        let root = temp_root("lane-names");
        let bad: [&[&str]; 8] = [
            &[],
            &[""],
            &["../vocals"],
            &["a/b"],
            &["a\\b"],
            &["with.dot"],
            &["vocals,drums"],
            &["vocals", "vocals"],
        ];
        for stems in bad {
            let header = CacheHeader::for_model(&VOCAL_MODEL, VOCAL_STEP as u64, 1000, stems);
            assert!(StemCache::open(&root, "abcd", header).is_err(), "{stems:?}");
            assert!(!root.join("abcd").exists(), "{stems:?} left an entry behind");
        }
        let fine = CacheHeader::for_model(&VOCAL_MODEL, VOCAL_STEP as u64, 1000, &["lead-vox_2"]);
        assert!(StemCache::open(&root, "abcd", fine).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second separator's entries live in a dot-named directory inside the
    /// four-stem root. The four-stem budget neither counts nor evicts them,
    /// and the same prune pointed AT that directory keeps it in a budget of
    /// its own.
    #[test]
    fn a_prune_walks_past_a_dot_directory_and_works_inside_one() {
        let root = temp_root("budget-dot-root");
        let four = filled_entry(&root, "aaaa1111", 1, 100);
        let vocal_root = root.join(".vocal-model");
        let cache = filled_vocal_entry(&vocal_root, "aaaa1111", 2 * VOCAL_STEP);
        let one = entry_bytes(cache.dir());
        drop(cache);
        assert!(one > 0 && one < four, "one lane of 8 s against four of 5.5 s: {one} vs {four}");

        let report = prune(&root, 0, &[]).unwrap();
        assert_eq!(report.before, four, "the dot directory is not counted");
        assert_eq!(report.removed, vec![("aaaa1111".to_string(), four)]);
        assert!(!root.join("aaaa1111").exists());
        assert!(is_complete_on_disk(&vocal_root, "aaaa1111", &vocal_header(2 * VOCAL_STEP)));

        let kept = prune(&vocal_root, 0, &["aaaa1111"]).unwrap();
        assert!(kept.removed.is_empty(), "a pin holds there as well: {kept:?}");
        let report = prune(&vocal_root, 0, &[]).unwrap();
        assert_eq!(report.removed, vec![("aaaa1111".to_string(), one)]);
        assert_eq!(report.after, 0);
        assert!(!vocal_root.join("aaaa1111").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two separators never share a directory. One that finds the other's
    /// entry where its own would go has been handed the wrong root, and says
    /// so; what is there stays there, whichever of the two came first.
    #[test]
    fn an_entry_of_another_model_is_refused_and_left_whole() {
        let root = temp_root("foreign-entry");
        filled_entry(&root, "aaaa1111", 2, 100);
        let four = CacheHeader::for_track(2 * CHUNK_STEP as u64);
        let before = std::fs::read(root.join("aaaa1111").join("vocals.pcm")).unwrap();
        assert!(matches!(
            StemCache::open(&root, "aaaa1111", vocal_header(2 * CHUNK_STEP)),
            Err(CacheError::Mismatch(_))
        ));
        assert!(is_complete_on_disk(&root, "aaaa1111", &four));
        assert_eq!(std::fs::read(root.join("aaaa1111").join("vocals.pcm")).unwrap(), before);
        assert!(root.join("aaaa1111").join("drums.pcm").is_file());

        let vocal_root = root.join(".vocal-model");
        drop(filled_vocal_entry(&vocal_root, "bbbb2222", 2 * VOCAL_STEP));
        assert!(matches!(
            StemCache::open(&vocal_root, "bbbb2222", CacheHeader::for_track(2 * VOCAL_STEP as u64)),
            Err(CacheError::Mismatch(_))
        ));
        assert!(is_complete_on_disk(&vocal_root, "bbbb2222", &vocal_header(2 * VOCAL_STEP)));
        assert!(!vocal_root.join("bbbb2222").join("drums.pcm").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Under the four-stem model's id a header is `for_track`'s and no
    /// other: older builds delete an entry whose licence or source line
    /// reads differently, and read silence out of one whose lanes do.
    #[test]
    fn a_four_stem_header_built_any_other_way_is_refused() {
        let root = temp_root("four-stem-guard");
        filled_entry(&root, "aaaa1111", 1, 100);
        let right = CacheHeader::for_track(CHUNK_STEP as u64);
        let text = std::fs::read(root.join("aaaa1111").join("header")).unwrap();

        let mut reworded = right.clone();
        reworded.license = crate::MODEL_LICENSE.to_string();
        let mut moved = right.clone();
        moved.source = "https://example.invalid/".to_string();
        let mut fewer = right.clone();
        fewer.stems = vec!["vocals".to_string()];
        let mut reordered = right.clone();
        reordered.stems.reverse();
        for wrong in [reworded, moved, fewer, reordered] {
            assert!(!is_complete_on_disk(&root, "aaaa1111", &wrong), "{wrong:?}");
            assert!(StemCache::open(&root, "aaaa1111", wrong.clone()).is_err(), "{wrong:?}");
            assert!(StemCache::open(&root, "cccc3333", wrong).is_err());
            assert!(!root.join("cccc3333").exists(), "nor does it start an entry");
        }
        assert_eq!(std::fs::read(root.join("aaaa1111").join("header")).unwrap(), text);
        assert!(is_complete_on_disk(&root, "aaaa1111", &right));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A span record is a claim about the files beside it. When they are
    /// gone or short -- a delete or a move that died halfway -- the claim is
    /// void: the probe says no, and the door adopts the directory with every
    /// span forgotten instead of reading zeros back as finished stems.
    #[test]
    fn an_entry_that_lost_its_audio_is_not_complete() {
        let root = temp_root("lost-audio");
        let header = CacheHeader::for_track(2 * CHUNK_STEP as u64);
        let holds_nothing = |digest: &str| {
            assert!(!is_complete_on_disk(&root, digest, &header), "{digest}");
            let cache = StemCache::open(&root, digest, header.clone()).unwrap();
            assert!(!cache.has_span(0) && !cache.has_span(1), "{digest}");
            drop(cache);
            assert!(!is_complete_on_disk(&root, digest, &header), "{digest}");
            assert_eq!(std::fs::read(root.join(digest).join("spans")).unwrap(), vec![0u8, 0]);
        };

        // The delete got as far as the header and stopped.
        filled_entry(&root, "aaaa1111", 2, 100);
        for name in ["bass.pcm", "drums.pcm", "gains", "header"] {
            std::fs::remove_file(root.join("aaaa1111").join(name)).unwrap();
        }
        holds_nothing("aaaa1111");

        // The header survived and a lane did not.
        filled_entry(&root, "bbbb2222", 2, 100);
        std::fs::remove_file(root.join("bbbb2222").join("bass.pcm")).unwrap();
        holds_nothing("bbbb2222");

        // A lane that is there and short.
        filled_entry(&root, "cccc3333", 2, 100);
        OpenOptions::new()
            .write(true)
            .open(root.join("cccc3333").join("vocals.pcm"))
            .unwrap()
            .set_len(CHUNK_STEP as u64 * FRAME_BYTES)
            .unwrap();
        holds_nothing("cccc3333");

        // And one that lost nothing is believed, as before.
        filled_entry(&root, "dddd4444", 2, 100);
        assert!(is_complete_on_disk(&root, "dddd4444", &header));
        assert!(StemCache::open(&root, "dddd4444", header.clone()).unwrap().is_complete());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A header nothing can read is no reason to delete spans that are on
    /// record beside it; with none on record there is nothing to lose and
    /// the entry starts again.
    #[test]
    fn a_header_that_cannot_be_read_does_not_cost_the_spans_beside_it() {
        let root = temp_root("unreadable-header");
        filled_entry(&root, "aaaa1111", 1, 100);
        let dir = root.join("aaaa1111");
        let header = CacheHeader::for_track(CHUNK_STEP as u64);
        std::fs::write(dir.join("header"), "").unwrap();
        assert!(StemCache::open(&root, "aaaa1111", header.clone()).is_err());
        assert!(dir.join("vocals.pcm").is_file());
        assert_eq!(std::fs::read(dir.join("spans")).unwrap(), vec![1u8]);

        std::fs::write(dir.join("spans"), [0u8]).unwrap();
        let cache = StemCache::open(&root, "aaaa1111", header.clone()).unwrap();
        assert!(!cache.has_span(0));
        let text = std::fs::read_to_string(dir.join("header")).unwrap();
        assert_eq!(CacheHeader::decode(&text).unwrap(), header);
        assert!(!dir.join("header.tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The budget is this cache's own. A directory under the root that is
    /// not an entry is neither counted nor deleted.
    #[test]
    fn a_prune_takes_only_what_is_an_entry() {
        let root = temp_root("budget-not-an-entry");
        let bytes = filled_entry(&root, "aaaa1111", 1, 100);
        let stray = root.join("notes");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("keep.txt"), vec![7u8; 4096]).unwrap();
        let report = prune(&root, 0, &[]).unwrap();
        assert_eq!(report.before, bytes, "{report:?}");
        assert_eq!(report.removed, vec![("aaaa1111".to_string(), bytes)]);
        assert!(stray.join("keep.txt").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A digest is one directory name under the root it is asked about. With
    /// a second root INSIDE the first, a digest that is a path could reach
    /// across; the probe and the door both refuse one, for either kind of
    /// header, even where the path leads to a real and complete entry.
    #[test]
    fn a_digest_that_is_a_path_cannot_reach_into_another_root() {
        let root = temp_root("digest-paths");
        let vocal_root = root.join(".vocal-model");
        let frames = 2 * VOCAL_STEP;
        let header = vocal_header(frames);
        drop(filled_vocal_entry(&vocal_root, "abcd", frames));
        assert!(is_complete_on_disk(&vocal_root, "abcd", &header));

        for digest in [".vocal-model/abcd", ".vocal-model\\abcd"] {
            assert!(!is_complete_on_disk(&root, digest, &header), "{digest:?}");
            assert!(StemCache::open(&root, digest, header.clone()).is_err(), "{digest:?}");
        }
        for digest in ["abcd/../abcd", "abcd\\..\\abcd", "./abcd", "abcd.", "abcd/", "abcd\\", ""] {
            assert!(!is_complete_on_disk(&vocal_root, digest, &header), "{digest:?}");
            assert!(
                StemCache::open(&vocal_root, digest, header.clone()).is_err(),
                "{digest:?}"
            );
            let four = CacheHeader::for_track(frames as u64);
            assert!(!is_complete_on_disk(&vocal_root, digest, &four), "{digest:?}");
            assert!(StemCache::open(&vocal_root, digest, four).is_err(), "{digest:?}");
        }
        assert!(
            is_complete_on_disk(&vocal_root, "abcd", &header),
            "and none of the asking touched the entry"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
