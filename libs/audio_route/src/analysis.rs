//! What a visualizer reads from a route: the samples that were played, as a
//! mono stream in a lock-free ring (a spectrum of them is the host's to
//! take; `makepad-audio-picture` has the transform).
//!
//! [`Analyzer`] wraps the processor a route runs. After the inner processor
//! has written the block, it averages each frame's channels and stores the
//! result in a fixed ring of atomics: no allocation, no lock, and nothing the
//! reader does can make the audio thread wait. [`AnalyzerHandle`] reads the
//! newest samples from any other thread. A reader that copies more than the
//! ring holds in the time the writer takes to lap it would see a torn window;
//! the ring is sized many blocks larger than any window read from it.

use crate::{FrameInfo, Processor};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

struct Ring {
    samples: Box<[AtomicU32]>,
    /// The side, (L − R) / 2, beside each mono sample (0 for one channel):
    /// with the mid it gives back left and right (`latest_stereo`).
    side: Box<[AtomicU32]>,
    /// Mono samples written since the analyzer was made.
    written: AtomicU64,
    sample_rate: AtomicU32,
}

/// Runs `inner`, then records what it left in the block.
pub struct Analyzer {
    inner: Box<dyn Processor>,
    ring: Arc<Ring>,
}

/// The reading side of an [`Analyzer`], cloned before the analyzer is moved
/// into a route.
#[derive(Clone)]
pub struct AnalyzerHandle {
    ring: Arc<Ring>,
}

impl Analyzer {
    /// `capacity` mono samples are kept (rounded up to a power of two, at
    /// least 1024); every allocation happens here.
    pub fn new(inner: Box<dyn Processor>, capacity: usize) -> (Self, AnalyzerHandle) {
        let capacity = capacity.max(1024).next_power_of_two();
        let samples: Box<[AtomicU32]> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
        let side: Box<[AtomicU32]> = (0..capacity).map(|_| AtomicU32::new(0)).collect();
        let ring = Arc::new(Ring { samples, side, written: AtomicU64::new(0), sample_rate: AtomicU32::new(0) });
        (Analyzer { inner, ring: Arc::clone(&ring) }, AnalyzerHandle { ring })
    }

    /// Records into the ring of an existing handle: a route opened again
    /// keeps feeding the reader it had.
    pub fn with_handle(inner: Box<dyn Processor>, handle: &AnalyzerHandle) -> Self {
        Analyzer { inner, ring: Arc::clone(&handle.ring) }
    }
}

impl Processor for Analyzer {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        self.inner.process(frames, info);
        let channels = info.channels as usize;
        if channels == 0 {
            return;
        }
        let ring = &*self.ring;
        let mask = ring.samples.len() as u64 - 1;
        let start = ring.written.load(Ordering::Relaxed);
        let scale = 1.0 / channels as f32;
        let mut at = start;
        for frame in frames.chunks_exact(channels) {
            let mono = frame.iter().sum::<f32>() * scale;
            let side = if channels >= 2 { (frame[0] - frame[1]) * 0.5 } else { 0.0 };
            ring.samples[(at & mask) as usize].store(mono.to_bits(), Ordering::Relaxed);
            ring.side[(at & mask) as usize].store(side.to_bits(), Ordering::Relaxed);
            at += 1;
        }
        ring.sample_rate.store((info.sample_rate as f32).to_bits(), Ordering::Relaxed);
        // Publishes the samples stored above to a reader that acquires it.
        ring.written.store(at, Ordering::Release);
    }
}

impl AnalyzerHandle {
    /// Mono samples written so far. A reader that sees it unchanged has
    /// nothing new to show.
    pub fn written(&self) -> u64 {
        self.ring.written.load(Ordering::Acquire)
    }

    /// The rate of the last block, 0 before the first.
    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.ring.sample_rate.load(Ordering::Relaxed))
    }

    /// Fills `out` with the newest `out.len()` samples, oldest first, and
    /// returns [`Self::written`] as of the copy. Samples not written yet
    /// read as silence; `out` longer than the ring is clamped to it.
    pub fn latest(&self, out: &mut [f32]) -> u64 {
        let ring = &*self.ring;
        let written = ring.written.load(Ordering::Acquire);
        let capacity = ring.samples.len();
        let mask = capacity as u64 - 1;
        let count = out.len().min(capacity);
        let (silent, out) = out.split_at_mut(out.len() - count);
        silent.fill(0.0);
        for (offset, slot) in out.iter_mut().enumerate() {
            let back = (count - offset) as u64;
            *slot = if back <= written { f32::from_bits(ring.samples[((written - back) & mask) as usize].load(Ordering::Relaxed)) } else { 0.0 };
        }
        written
    }

    /// Fills `out` with the newest `out.len() / 2` frames as interleaved
    /// left, right (from the mid and the side; a mono source gives L = R),
    /// oldest first, and returns [`Self::written`] as of the copy.
    pub fn latest_stereo(&self, out: &mut [f32]) -> u64 {
        let ring = &*self.ring;
        let written = ring.written.load(Ordering::Acquire);
        let capacity = ring.samples.len();
        let mask = capacity as u64 - 1;
        let frames = out.len() / 2;
        let count = frames.min(capacity);
        let silent = (frames - count) * 2;
        out[..silent].fill(0.0);
        for offset in 0..count {
            let back = (count - offset) as u64;
            let (mid, side) = if back <= written {
                let at = ((written - back) & mask) as usize;
                (f32::from_bits(ring.samples[at].load(Ordering::Relaxed)), f32::from_bits(ring.side[at].load(Ordering::Relaxed)))
            } else {
                (0.0, 0.0)
            };
            out[silent + offset * 2] = mid + side;
            out[silent + offset * 2 + 1] = mid - side;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Passthrough;

    fn info(channels: u16) -> FrameInfo {
        FrameInfo { sample_rate: 48_000.0, channels, frames: 0, host_time: 0 }
    }

    #[test]
    fn the_ring_keeps_the_newest_mono_samples_in_order() {
        let (mut analyzer, handle) = Analyzer::new(Box::new(Passthrough), 1024);
        let mut stereo: Vec<f32> = (0..600).flat_map(|i| [i as f32, i as f32 + 2.0]).collect();
        analyzer.process(&mut stereo, &info(2));
        assert_eq!(handle.written(), 600);
        assert_eq!(handle.sample_rate(), 48_000.0);
        let mut out = [0.0f32; 4];
        handle.latest(&mut out);
        assert_eq!(out, [597.0, 598.0, 599.0, 600.0], "mean of the channels, oldest first");
        // Wrap past the capacity: the newest samples are still in order.
        let mut more: Vec<f32> = (600..1700).flat_map(|i| [i as f32, i as f32]).collect();
        analyzer.process(&mut more, &info(2));
        let mut out = [0.0f32; 3];
        assert_eq!(handle.latest(&mut out), 1700);
        assert_eq!(out, [1697.0, 1698.0, 1699.0]);
        // Asking for more than was written pads the front with silence.
        let (_, fresh) = Analyzer::new(Box::new(Passthrough), 1024);
        let mut out = [1.0f32; 2];
        fresh.latest(&mut out);
        assert_eq!(out, [0.0, 0.0]);
    }

    #[test]
    fn the_analyzer_records_what_the_inner_processor_left() {
        let (gain, _) = crate::Gain::new(0.5);
        let (mut analyzer, handle) = Analyzer::new(Box::new(gain), 1024);
        let mut mono = vec![1.0f32; 8];
        analyzer.process(&mut mono, &info(1));
        assert_eq!(mono[0], 0.5, "the block itself is processed");
        let mut out = [0.0f32; 1];
        handle.latest(&mut out);
        assert_eq!(out[0], 0.5);
    }

    #[test]
    fn stereo_comes_back_from_the_mid_and_side() {
        let (mut analyzer, handle) = Analyzer::new(Box::new(Passthrough), 1024);
        let mut stereo: Vec<f32> = (0..8).flat_map(|i| [i as f32, -(i as f32)]).collect();
        analyzer.process(&mut stereo, &info(2));
        let mut out = vec![0.0; 16];
        handle.latest_stereo(&mut out);
        for i in 0..8 {
            assert!((out[i * 2] - i as f32).abs() < 1e-6 && (out[i * 2 + 1] + i as f32).abs() < 1e-6);
        }
    }

}
