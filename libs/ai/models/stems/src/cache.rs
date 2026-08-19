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

use crate::config::*;
use crate::model::StemSet;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Bytes one frame occupies in a stem file (stereo i16).
const FRAME_BYTES: u64 = 4;

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

    #[test]
    fn refuses_a_span_start_off_the_grid() {
        let root = temp_root("grid");
        let mut cache =
            StemCache::open(&root, "0f0f", CacheHeader::for_track(2 * CHUNK_STEP as u64)).unwrap();
        assert!(cache.write_span(7, &ramp_stems(16, 0.0)).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
