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
use crate::model::StemSet;
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

/// Identity of what produced the cached audio. A mismatch on any field means
/// the cached spans are not the ones this build would compute, so the entry is
/// rebuilt rather than trusted.
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
}

impl CacheHeader {
    pub fn for_track(frames: u64) -> Self {
        Self {
            model_id: crate::MODEL_ID.to_string(),
            checkpoint: crate::MODEL_CHECKPOINT.to_string(),
            checkpoint_sha256: crate::MODEL_SHA256.to_string(),
            license: crate::MODEL_LICENSE.to_string(),
            source: crate::MODEL_SOURCE.to_string(),
            sample_rate: SAMPLE_RATE,
            frames,
            span_samples: CHUNK_STEP as u64,
            // TRACK spans, not model chunks: with the reference's leading
            // reflect pad the first model chunk finalizes only padding, so the
            // padded chunk index and the track span index differ by one. The
            // cache is addressed in track coordinates (`start / CHUNK_STEP`).
            span_count: (frames as usize).div_ceil(CHUNK_STEP) as u64,
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
        })
    }
}

/// Whether `digest` is separated end to end under `root`, WITHOUT touching a
/// byte of it.
///
/// [`StemCache::open`] is the separator's door: it creates the entry, sizes
/// four sparse stem files to the whole track and REPLACES one whose header
/// disagrees. That is right for a caller about to write spans and wrong for
/// one that only wants to know — a deck served its stems from the store never
/// separates locally, and opening the entry to ask about it would leave an
/// empty one behind on every load and put it in front of the budget.
///
/// Same two checks `open` makes before it trusts an entry, in the same order:
/// the header must be this exact track under this exact model, and every span
/// byte must be set. Anything unreadable, short or stale reads as "not
/// complete" — the question is only ever asked to decide whether work can be
/// SKIPPED, so uncertainty has to answer no.
pub fn is_complete_on_disk(root: impl AsRef<Path>, digest: &str, header: &CacheHeader) -> bool {
    if digest.is_empty() || digest.contains(['/', '\\', '.']) {
        return false;
    }
    let dir = root.as_ref().join(digest);
    let Ok(text) = std::fs::read_to_string(dir.join("header")) else {
        return false;
    };
    if CacheHeader::decode(&text).ok().as_ref() != Some(header) {
        return false;
    }
    let Ok(spans) = std::fs::read(dir.join("spans")) else {
        return false;
    };
    spans.len() as u64 == header.span_count
        && header.span_count > 0
        && spans.iter().all(|present| *present != 0)
}

/// A per-track cache directory, opened for read+write.
pub struct StemCache {
    dir: PathBuf,
    header: CacheHeader,
    present: Vec<bool>,
    /// Per-span, per-stem normalization peak, `span * NUM_STEMS + stem`.
    gains: Vec<f32>,
    files: Vec<File>,
    spans_file: File,
    gains_file: File,
}

impl StemCache {
    /// Opens (or creates) the cache entry for `digest`. An existing entry whose
    /// header disagrees with `header` is REPLACED, not silently reused.
    pub fn open(root: impl AsRef<Path>, digest: &str, header: CacheHeader) -> Result<StemCache> {
        if digest.is_empty() || digest.contains(['/', '\\', '.']) {
            return Err(CacheError::Mismatch(format!(
                "digest {digest:?} is not a bare content hash"
            )));
        }
        let dir = root.as_ref().join(digest);
        let header_path = dir.join("header");
        if header_path.is_file() {
            let mut text = String::new();
            File::open(&header_path)?.read_to_string(&mut text)?;
            let stale = match CacheHeader::decode(&text) {
                Ok(existing) => existing != header,
                Err(_) => true,
            };
            if stale {
                std::fs::remove_dir_all(&dir)?;
            }
        }
        std::fs::create_dir_all(&dir)?;
        if !header_path.is_file() {
            let mut file = File::create(&header_path)?;
            file.write_all(header.encode().as_bytes())?;
            file.sync_all()?;
        }

        let span_count = header.span_count as usize;
        let mut spans_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("spans"))?;
        spans_file.set_len(span_count as u64)?;
        let mut present_bytes = vec![0u8; span_count];
        spans_file.seek(SeekFrom::Start(0))?;
        spans_file.read_exact(&mut present_bytes)?;

        let mut gains_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("gains"))?;
        let gain_bytes = (span_count * NUM_STEMS * 4) as u64;
        gains_file.set_len(gain_bytes)?;
        let mut gain_raw = vec![0u8; gain_bytes as usize];
        gains_file.seek(SeekFrom::Start(0))?;
        gains_file.read_exact(&mut gain_raw)?;
        let gains: Vec<f32> = gain_raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let bytes = header.frames * FRAME_BYTES;
        let mut files = Vec::with_capacity(NUM_STEMS);
        for stem in Stem::ALL {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(format!("{}.pcm", stem.name())))?;
            file.set_len(bytes)?;
            files.push(file);
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

    /// Writes one finished span. `start` is in track frames and must land on a
    /// span boundary.
    pub fn write_span(&mut self, start: usize, stems: &StemSet) -> Result<()> {
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
        let mut buf = vec![0u8; frames * FRAME_BYTES as usize];
        for (index, stem) in stems.iter().enumerate() {
            let n = frames.min(stem.frames());
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
            for byte in buf[n * 4..].iter_mut() {
                *byte = 0;
            }
            let file = &mut self.files[index];
            file.seek(SeekFrom::Start(from * FRAME_BYTES))?;
            file.write_all(&buf)?;
            let slot = span * NUM_STEMS + index;
            self.gains[slot] = if peak > 0.0 { peak } else { 1.0 };
            self.gains_file.seek(SeekFrom::Start(slot as u64 * 4))?;
            self.gains_file.write_all(&self.gains[slot].to_le_bytes())?;
        }
        // The presence flag is written LAST, so a crash mid-write leaves the
        // span marked absent and it is simply recomputed.
        for file in self.files.iter_mut() {
            file.flush()?;
        }
        self.gains_file.flush()?;
        self.spans_file.seek(SeekFrom::Start(span as u64))?;
        self.spans_file.write_all(&[1u8])?;
        self.spans_file.flush()?;
        self.present[span] = true;
        Ok(())
    }

    /// Reads one span back, or `None` if it has not been written.
    pub fn read_span(&mut self, span: usize) -> Result<Option<StemSet>> {
        if !self.has_span(span) {
            return Ok(None);
        }
        let (from, to) = self.span_range(span);
        let frames = (to - from) as usize;
        let mut out = crate::model::empty_stem_set(frames);
        let mut buf = vec![0u8; frames * FRAME_BYTES as usize];
        for (index, stem) in out.iter_mut().enumerate() {
            let file = &mut self.files[index];
            file.seek(SeekFrom::Start(from * FRAME_BYTES))?;
            file.read_exact(&mut buf)?;
            let gain = self.gains[span * NUM_STEMS + index];
            for frame in 0..frames {
                let at = frame * 4;
                stem.left[frame] = dequantize(i16::from_le_bytes([buf[at], buf[at + 1]])) * gain;
                stem.right[frame] =
                    dequantize(i16::from_le_bytes([buf[at + 2], buf[at + 3]])) * gain;
            }
        }
        Ok(Some(out))
    }

    /// Reads a whole track back, for the "already separated, just play it"
    /// path. Errors if any span is missing.
    pub fn read_all(&mut self) -> Result<StemSet> {
        let frames = self.header.frames as usize;
        let mut out = crate::model::empty_stem_set(frames);
        for span in 0..self.span_count() {
            let (from, _) = self.span_range(span);
            let Some(part) = self.read_span(span)? else {
                return Err(CacheError::Mismatch(format!("span {span} is missing")));
            };
            for (stem, src) in part.iter().enumerate() {
                let at = from as usize;
                let n = src.frames().min(frames - at);
                out[stem].left[at..at + n].copy_from_slice(&src.left[..n]);
                out[stem].right[at..at + n].copy_from_slice(&src.right[..n]);
            }
        }
        Ok(out)
    }
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
        for bad in ["../escape", "a/b", "with.dot", ""] {
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
}
