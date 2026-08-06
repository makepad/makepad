//! The sample bank: decode on demand, resample once, cache with a budget.
//!
//! Sounds are addressed by the asset index's stable ids (`kenney/impact/
//! wood-heavy-01`), so nothing here knows about file paths — the host hands
//! over bytes and the bank owns what happens after.

use crate::{decode, AudioError, Pcm};
use std::collections::HashMap;

/// Handle to a loaded sample.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SampleId(pub u32);

/// Default cache ceiling. Kenney's whole impact pack decodes to roughly 20 MB
/// of f32 PCM at 44.1k stereo; a Quest should not hold every pack at once.
pub const DEFAULT_BUDGET_BYTES: usize = 24 * 1024 * 1024;

struct Entry {
    name: String,
    pcm: Pcm,
    /// Monotonic counter for LRU eviction.
    last_used: u64,
    /// Never evict something a voice is still reading.
    pinned: u32,
}

pub struct SampleBank {
    entries: Vec<Option<Entry>>,
    by_name: HashMap<String, SampleId>,
    free: Vec<u32>,
    budget: usize,
    used: usize,
    clock: u64,
    device_rate: u32,
    /// Names that failed to decode, so a broken file is not retried forever.
    failed: HashMap<String, AudioError>,
}

impl SampleBank {
    pub fn new(device_rate: u32) -> Self {
        Self::with_budget(device_rate, DEFAULT_BUDGET_BYTES)
    }

    pub fn with_budget(device_rate: u32, budget: usize) -> Self {
        Self {
            entries: Vec::new(),
            by_name: HashMap::new(),
            free: Vec::new(),
            budget,
            used: 0,
            clock: 0,
            device_rate: device_rate.max(1),
            failed: HashMap::new(),
        }
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn get(&self, id: SampleId) -> Option<&Pcm> {
        self.entries
            .get(id.0 as usize)
            .and_then(|e| e.as_ref())
            .map(|e| &e.pcm)
    }

    pub fn id_of(&self, name: &str) -> Option<SampleId> {
        self.by_name.get(name).copied()
    }

    /// Whether this name already failed to decode.
    pub fn is_failed(&self, name: &str) -> bool {
        self.failed.contains_key(name)
    }

    /// Decode `bytes` under `name` and cache it. Repeat calls with a known
    /// name skip the decode entirely.
    pub fn insert(&mut self, name: &str, bytes: &[u8]) -> Result<SampleId, AudioError> {
        if let Some(id) = self.by_name.get(name) {
            return Ok(*id);
        }
        if let Some(err) = self.failed.get(name) {
            return Err(err.clone());
        }
        let pcm = match decode(bytes) {
            Ok(p) => p,
            Err(e) => {
                self.failed.insert(name.to_string(), e.clone());
                return Err(e);
            }
        };
        let pcm = resample(pcm, self.device_rate);
        let bytes_used = pcm.bytes();
        self.make_room(bytes_used);
        self.clock += 1;
        let entry = Entry {
            name: name.to_string(),
            pcm,
            last_used: self.clock,
            pinned: 0,
        };
        let id = match self.free.pop() {
            Some(i) => {
                self.entries[i as usize] = Some(entry);
                SampleId(i)
            }
            None => {
                self.entries.push(Some(entry));
                SampleId(self.entries.len() as u32 - 1)
            }
        };
        self.by_name.insert(name.to_string(), id);
        self.used += bytes_used;
        Ok(id)
    }

    /// Mark a sample as in use so eviction cannot pull it out from under a
    /// playing voice.
    pub fn pin(&mut self, id: SampleId) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(Some(e)) = self.entries.get_mut(id.0 as usize) {
            e.pinned += 1;
            e.last_used = clock;
        }
    }

    pub fn unpin(&mut self, id: SampleId) {
        if let Some(Some(e)) = self.entries.get_mut(id.0 as usize) {
            e.pinned = e.pinned.saturating_sub(1);
        }
    }

    /// Evict least-recently-used unpinned samples until `need` bytes fit.
    fn make_room(&mut self, need: usize) {
        if need > self.budget {
            // A single sample larger than the whole budget still loads; the
            // alternative is a sound that can never play at all.
            return;
        }
        while self.used + need > self.budget {
            let victim = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.as_ref().map(|e| (i, e)))
                .filter(|(_, e)| e.pinned == 0)
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i);
            let Some(i) = victim else { break };
            if let Some(e) = self.entries[i].take() {
                self.used = self.used.saturating_sub(e.pcm.bytes());
                self.by_name.remove(&e.name);
                self.free.push(i as u32);
            }
        }
    }
}

/// Linear resampling to the device rate.
///
/// Linear interpolation images above roughly half the source rate; for short
/// impact SFX at 44.1k played on a 48k device that is inaudible, and it costs
/// one multiply per sample. A polyphase kernel is the upgrade if music ever
/// needs it.
pub fn resample(pcm: Pcm, target_rate: u32) -> Pcm {
    if pcm.sample_rate == target_rate || pcm.sample_rate == 0 || pcm.frames() == 0 {
        return pcm;
    }
    let ch = pcm.channels.max(1);
    let src_frames = pcm.frames();
    let ratio = target_rate as f64 / pcm.sample_rate as f64;
    let dst_frames = ((src_frames as f64) * ratio).round().max(1.0) as usize;
    let mut samples = Vec::with_capacity(dst_frames * ch);
    for i in 0..dst_frames {
        let src_pos = i as f64 / ratio;
        let i0 = (src_pos.floor() as usize).min(src_frames - 1);
        let frac = (src_pos - src_pos.floor()) as f32;
        let i1 = (i0 + 1).min(src_frames - 1);
        for c in 0..ch {
            let a = pcm.samples[i0 * ch + c];
            let b = pcm.samples[i1 * ch + c];
            samples.push(a + (b - a) * frac);
        }
    }
    Pcm {
        channels: ch,
        sample_rate: target_rate,
        samples,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny valid 16-bit mono WAV of `frames` samples.
    pub(crate) fn wav(frames: usize, rate: u32) -> Vec<u8> {
        let data: Vec<u8> = (0..frames)
            .flat_map(|i| ((i as i16).wrapping_mul(300)).to_le_bytes())
            .collect();
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&data);
        v
    }

    #[test]
    fn inserting_twice_reuses_the_decode() {
        let mut b = SampleBank::new(44100);
        let a = b.insert("hit", &wav(100, 44100)).unwrap();
        let a2 = b.insert("hit", &wav(100, 44100)).unwrap();
        assert_eq!(a, a2);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn a_broken_file_is_remembered_not_retried() {
        let mut b = SampleBank::new(44100);
        assert!(b.insert("bad", b"not audio").is_err());
        assert!(b.is_failed("bad"));
        assert!(b.insert("bad", b"not audio").is_err());
    }

    #[test]
    fn lru_eviction_respects_the_budget() {
        // Room for about two 100-frame mono samples (400 bytes each).
        let mut b = SampleBank::with_budget(44100, 900);
        b.insert("a", &wav(100, 44100)).unwrap();
        b.insert("b", &wav(100, 44100)).unwrap();
        assert_eq!(b.len(), 2);
        b.insert("c", &wav(100, 44100)).unwrap();
        assert!(b.used_bytes() <= 900, "used {}", b.used_bytes());
        assert!(b.id_of("c").is_some());
        assert!(b.len() <= 2);
    }

    #[test]
    fn pinned_samples_survive_pressure() {
        let mut b = SampleBank::with_budget(44100, 900);
        let keep = b.insert("keep", &wav(100, 44100)).unwrap();
        b.pin(keep);
        for i in 0..8 {
            let _ = b.insert(&format!("x{i}"), &wav(100, 44100));
        }
        assert!(b.id_of("keep").is_some(), "a playing voice was evicted");
    }

    #[test]
    fn resampling_changes_length_by_the_rate_ratio() {
        let p = Pcm {
            channels: 1,
            sample_rate: 22050,
            samples: vec![0.0; 1000],
        };
        let up = resample(p, 44100);
        assert_eq!(up.sample_rate, 44100);
        assert!((up.frames() as i64 - 2000).abs() <= 1, "{}", up.frames());
    }

    #[test]
    fn resampling_preserves_a_constant_signal() {
        let p = Pcm {
            channels: 1,
            sample_rate: 8000,
            samples: vec![0.5; 200],
        };
        let out = resample(p, 44100);
        assert!(out.samples.iter().all(|s| (s - 0.5).abs() < 1e-5));
        assert!(out.samples.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn resampling_a_matching_rate_is_a_no_op() {
        let p = Pcm {
            channels: 2,
            sample_rate: 48000,
            samples: vec![0.25; 64],
        };
        let out = resample(p.clone(), 48000);
        assert_eq!(out, p);
    }
}
