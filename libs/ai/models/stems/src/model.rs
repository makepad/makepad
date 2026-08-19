//! One chunk in, four stems out.
//!
//! Owns the compiled forward graph plus the CPU transform halves that sandwich
//! it: STFT -> [graph] -> complex mask -> inverse STFT.

use crate::config::*;
use crate::graph::{build_graph, StemsGraph};
use crate::stft::Stft;
use crate::weights::{StemsWeights, DEFAULT_GRAPH_EXTRA_BYTES};
use makepad_ai_common::backend::{
    compile_graph_session, new_runtime, BufferStorageMode, GraphSession, GraphTensorWrite, Runtime,
};
use makepad_ai_common::{DiffusionError, Result, TensorId};
use std::path::Path;

/// Planar stereo audio: one `Vec` per channel, equal length.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StereoBuf {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl StereoBuf {
    pub fn silence(frames: usize) -> Self {
        Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
        }
    }

    pub fn frames(&self) -> usize {
        self.left.len().min(self.right.len())
    }

    pub fn channel(&self, index: usize) -> &[f32] {
        if index == 0 {
            &self.left
        } else {
            &self.right
        }
    }

    pub fn channel_mut(&mut self, index: usize) -> &mut Vec<f32> {
        if index == 0 {
            &mut self.left
        } else {
            &mut self.right
        }
    }
}

/// Four separated sources for one span of audio, in `Stem::ALL` order.
pub type StemSet = [StereoBuf; NUM_STEMS];

pub fn empty_stem_set(frames: usize) -> StemSet {
    [
        StereoBuf::silence(frames),
        StereoBuf::silence(frames),
        StereoBuf::silence(frames),
        StereoBuf::silence(frames),
    ]
}

/// A loaded, compiled separator. Not `Sync`; keep it on one worker thread (the
/// device runtime and its buffers are thread-affine).
pub struct StemsModel {
    weights: StemsWeights,
    graph: StemsGraph,
    session: GraphSession,
    stft: Stft,
    /// Reused per chunk so a long demix does not churn the allocator.
    features: Vec<f32>,
    spectrum: [Vec<f32>; AUDIO_CHANNELS],
    masked: Vec<f32>,
}

impl StemsModel {
    /// Loads the checkpoint and compiles the forward graph for the device
    /// runtime. Expensive (seconds): do it once, off any latency-sensitive
    /// thread.
    pub fn load(checkpoint: impl AsRef<Path>) -> Result<Self> {
        let runtime = new_runtime()?;
        Self::load_with_runtime(checkpoint, runtime)
    }

    pub fn load_with_runtime(checkpoint: impl AsRef<Path>, runtime: Runtime) -> Result<Self> {
        let mut weights =
            StemsWeights::load_with_extra(checkpoint, DEFAULT_GRAPH_EXTRA_BYTES)?;
        let graph = build_graph(&mut weights)?;
        let session = compile_graph_session(
            &runtime,
            &weights.ctx,
            &graph.graph,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )?;
        Ok(Self {
            weights,
            graph,
            session,
            stft: Stft::bs_roformer(),
            features: vec![0.0; FEATURES * CHUNK_FRAMES],
            spectrum: [
                vec![0.0; FREQ_BINS * CHUNK_FRAMES * 2],
                vec![0.0; FREQ_BINS * CHUNK_FRAMES * 2],
            ],
            masked: vec![0.0; FREQ_BINS * CHUNK_FRAMES * 2],
        })
    }

    pub fn checkpoint_path(&self) -> &Path {
        &self.weights.path
    }

    /// Separates exactly one `CHUNK_SAMPLES`-long stereo chunk.
    ///
    /// `chunk` must already be padded to `CHUNK_SAMPLES` frames — the caller
    /// owns the reference's reflect/constant padding rules (see `demix.rs`).
    pub fn separate_chunk(&mut self, chunk: &StereoBuf) -> Result<StemSet> {
        if chunk.left.len() != CHUNK_SAMPLES || chunk.right.len() != CHUNK_SAMPLES {
            return Err(DiffusionError::model(format!(
                "stems: chunk must be {CHUNK_SAMPLES} frames per channel, got {}/{}",
                chunk.left.len(),
                chunk.right.len()
            )));
        }

        let timing = std::env::var_os("MAKEPAD_STEMS_TIMING").is_some();
        let t0 = std::time::Instant::now();

        // -- STFT both channels into the graph's feature layout --
        for ch in 0..AUDIO_CHANNELS {
            let (spec, frames) = self.stft.forward(chunk.channel(ch));
            if frames != CHUNK_FRAMES {
                return Err(DiffusionError::model(format!(
                    "stems: stft produced {frames} frames, expected {CHUNK_FRAMES}"
                )));
            }
            self.spectrum[ch].copy_from_slice(&spec);
        }
        pack_features(&self.spectrum, &mut self.features);
        let t_stft = t0.elapsed();

        // -- forward --
        let outputs: Vec<TensorId> = self.graph.masks.to_vec();
        let execution = self
            .session
            .execute(
                &self.weights.ctx,
                &[GraphTensorWrite {
                    tensor_id: self.graph.features,
                    bytes: as_bytes(&self.features),
                }],
                &outputs,
            )
            .map_err(DiffusionError::model)?;
        let t_forward = t0.elapsed();

        // -- complex mask + inverse STFT --
        let mut stems = empty_stem_set(0);
        for (stem, mask_id) in self.graph.masks.iter().enumerate() {
            let bytes = execution.outputs.get(mask_id).ok_or_else(|| {
                DiffusionError::model(format!("stems: graph returned no mask for stem {stem}"))
            })?;
            let mask = f32_from_bytes(bytes)?;
            if mask.len() != FEATURES * CHUNK_FRAMES {
                return Err(DiffusionError::model(format!(
                    "stems: mask {stem} has {} floats, expected {}",
                    mask.len(),
                    FEATURES * CHUNK_FRAMES
                )));
            }
            for ch in 0..AUDIO_CHANNELS {
                apply_mask(&self.spectrum[ch], mask, ch, &mut self.masked);
                let samples = self
                    .stft
                    .inverse(&self.masked, CHUNK_FRAMES, CHUNK_SAMPLES);
                *stems[stem].channel_mut(ch) = samples;
            }
        }
        if timing {
            let total = t0.elapsed();
            eprintln!(
                "stems chunk: stft {:.0}ms  forward {:.0}ms  mask+istft {:.0}ms  total {:.0}ms",
                t_stft.as_secs_f64() * 1e3,
                (t_forward - t_stft).as_secs_f64() * 1e3,
                (total - t_forward).as_secs_f64() * 1e3,
                total.as_secs_f64() * 1e3,
            );
        }
        Ok(stems)
    }
}

/// `rearrange('b s f t c -> b t ((f s) c)')`: feature index of
/// `(bin, channel, re_im)` is `(bin * channels + channel) * 2 + re_im`.
pub fn feature_index(bin: usize, channel: usize, re_im: usize) -> usize {
    (bin * AUDIO_CHANNELS + channel) * 2 + re_im
}

/// Interleaves the two channels' spectra into the graph's `[4100, 1101]` input.
fn pack_features(spectrum: &[Vec<f32>; AUDIO_CHANNELS], out: &mut [f32]) {
    for bin in 0..FREQ_BINS {
        for ch in 0..AUDIO_CHANNELS {
            let src = &spectrum[ch];
            let base = bin * CHUNK_FRAMES * 2;
            let re_at = feature_index(bin, ch, 0);
            let im_at = feature_index(bin, ch, 1);
            for frame in 0..CHUNK_FRAMES {
                let s = base + frame * 2;
                let d = frame * FEATURES;
                out[d + re_at] = src[s];
                out[d + im_at] = src[s + 1];
            }
        }
    }
}

/// Complex product of one channel's spectrum with the stem's ratio mask.
fn apply_mask(spectrum: &[f32], mask: &[f32], channel: usize, out: &mut [f32]) {
    for bin in 0..FREQ_BINS {
        let re_at = feature_index(bin, channel, 0);
        let im_at = feature_index(bin, channel, 1);
        let base = bin * CHUNK_FRAMES * 2;
        for frame in 0..CHUNK_FRAMES {
            let s = base + frame * 2;
            let m = frame * FEATURES;
            let (ar, ai) = (spectrum[s], spectrum[s + 1]);
            let (br, bi) = (mask[m + re_at], mask[m + im_at]);
            out[s] = ar * br - ai * bi;
            out[s + 1] = ar * bi + ai * br;
        }
    }
}

fn as_bytes(values: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 4) }
}

fn f32_from_bytes(bytes: &[u8]) -> Result<&[f32]> {
    if bytes.len() % 4 != 0 {
        return Err(DiffusionError::model(format!(
            "stems: graph output has {} bytes, not a multiple of 4",
            bytes.len()
        )));
    }
    if bytes.as_ptr() as usize % std::mem::align_of::<f32>() != 0 {
        return Err(DiffusionError::model(
            "stems: graph output is not f32-aligned",
        ));
    }
    Ok(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_index_tiles_the_vector_once() {
        let mut seen = vec![false; FEATURES];
        for bin in 0..FREQ_BINS {
            for ch in 0..AUDIO_CHANNELS {
                for c in 0..2 {
                    let at = feature_index(bin, ch, c);
                    assert!(at < FEATURES);
                    assert!(!seen[at], "feature {at} written twice");
                    seen[at] = true;
                }
            }
        }
        assert!(seen.into_iter().all(|s| s));
    }

    #[test]
    fn pack_then_mask_with_unit_mask_is_identity() {
        // A unit complex mask (1 + 0i) must return the spectrum untouched.
        let mut spectrum = [
            vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2],
            vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2],
        ];
        let mut seed = 12345u32;
        for ch in 0..AUDIO_CHANNELS {
            for v in spectrum[ch].iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *v = (seed >> 8) as f32 / 8388608.0 - 1.0;
            }
        }
        let mut mask = vec![0.0f32; FEATURES * CHUNK_FRAMES];
        for frame in 0..CHUNK_FRAMES {
            for bin in 0..FREQ_BINS {
                for ch in 0..AUDIO_CHANNELS {
                    mask[frame * FEATURES + feature_index(bin, ch, 0)] = 1.0;
                }
            }
        }
        let mut out = vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2];
        for ch in 0..AUDIO_CHANNELS {
            apply_mask(&spectrum[ch], &mask, ch, &mut out);
            assert_eq!(out, spectrum[ch], "channel {ch}");
        }
    }

    #[test]
    fn mask_multiplication_is_complex() {
        // (1+2i) * (3+4i) = -5 + 10i
        let mut spectrum = vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2];
        spectrum[0] = 1.0;
        spectrum[1] = 2.0;
        let mut mask = vec![0.0f32; FEATURES * CHUNK_FRAMES];
        mask[feature_index(0, 0, 0)] = 3.0;
        mask[feature_index(0, 0, 1)] = 4.0;
        let mut out = vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2];
        apply_mask(&spectrum, &mask, 0, &mut out);
        assert_eq!(out[0], -5.0);
        assert_eq!(out[1], 10.0);
    }

    #[test]
    fn pack_features_places_both_channels() {
        let mut spectrum = [
            vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2],
            vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2],
        ];
        // bin 3, frame 5.
        spectrum[0][3 * CHUNK_FRAMES * 2 + 5 * 2] = 7.0;
        spectrum[1][3 * CHUNK_FRAMES * 2 + 5 * 2 + 1] = -9.0;
        let mut features = vec![0.0f32; FEATURES * CHUNK_FRAMES];
        pack_features(&spectrum, &mut features);
        assert_eq!(features[5 * FEATURES + feature_index(3, 0, 0)], 7.0);
        assert_eq!(features[5 * FEATURES + feature_index(3, 1, 1)], -9.0);
        assert_eq!(
            features.iter().filter(|v| **v != 0.0).count(),
            2,
            "nothing else may be written"
        );
    }
}
