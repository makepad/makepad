//! One chunk in, the vocal out.
//!
//! The same sandwich as the four-stem model — STFT, graph, complex mask,
//! inverse STFT, with the transforms on the CPU — and the same transform,
//! packing and masking code. Two steps are this model's own, both between
//! the graph and the inverse transform: the band masks the graph returns
//! overlap and are averaged into one mask over the spectrum, and the masked
//! spectrum has its DC bin cleared, as the reference does before it inverts.

use super::config::*;
use super::graph::{build_graph_until, Stage, VocalsGraph};
use super::weights;
use crate::config::{ChunkGeometry, AUDIO_CHANNELS, DIM, FEATURES, FREQ_BINS};
use crate::demix::ChunkSeparator;
use crate::model::{
    apply_mask, as_bytes, command_buffer_ops_limit, f32_from_bytes, istft_threads, pack_features,
    StereoBuf, CB_MAX_BYTES,
};
use crate::stft::Stft;
use crate::weights::{StemsWeights, DEFAULT_GRAPH_EXTRA_BYTES};
use makepad_ai_common::backend::{
    BufferStorageMode, DeviceGraphSession, DeviceRuntime, GraphDevice,
};
use makepad_ai_common::{DiffusionError, Result};
use std::path::Path;

/// A loaded, compiled vocal separator. Not `Sync`; keep it on one worker
/// thread (the device runtime and its buffers are thread-affine).
///
/// Compiled for one [`ChunkGeometry`], like the four-stem model: no weight
/// depends on the frame count, so the chunk is a runtime value, and
/// [`separate_chunk`](Self::separate_chunk) takes exactly that many samples.
/// [`load`](Self::load) uses the chunk the checkpoint was trained at, which
/// is also the grid its cache entries are addressed in.
pub struct VocalsModel {
    geometry: ChunkGeometry,
    weights: StemsWeights,
    graph: VocalsGraph,
    session: DeviceGraphSession,
    stft: Stft,
    /// Reused per chunk so a long demix does not churn the allocator.
    features: Vec<f32>,
    spectrum: [Vec<f32>; AUDIO_CHANNELS],
    /// The averaged mask of the last chunk, `[4100, frames]`.
    mask: Vec<f32>,
    inverse_band_counts: Vec<f32>,
    istft_threads: usize,
}

impl VocalsModel {
    /// Loads the checkpoint and compiles the forward graph for the device
    /// runtime. Expensive (seconds): do it once, off any latency-sensitive
    /// thread.
    pub fn load(checkpoint: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_geometry(checkpoint, CHUNK)
    }

    /// As [`load`](Self::load), compiled for `geometry` instead of the
    /// trained chunk.
    pub fn load_with_geometry(
        checkpoint: impl AsRef<Path>,
        geometry: ChunkGeometry,
    ) -> Result<Self> {
        let runtime = DeviceRuntime::new()?;
        Self::build(checkpoint, runtime, geometry, Stage::Masks)
    }

    fn build(
        checkpoint: impl AsRef<Path>,
        runtime: DeviceRuntime,
        geometry: ChunkGeometry,
        stage: Stage,
    ) -> Result<Self> {
        // The command-buffer budget and the weight precision are decided per
        // store for the reasons the four-stem model gives; nothing about
        // either is particular to a checkpoint.
        if let DeviceRuntime::Metal(metal) = &runtime {
            if let Some(ops) = command_buffer_ops_limit() {
                metal.set_command_buffer_limits(ops, CB_MAX_BYTES);
            }
        }
        let f16 = match runtime.device() {
            GraphDevice::Metal => crate::weights::f16_weights_enabled(),
            GraphDevice::Cuda => crate::weights::f16_weights_requested(),
        };
        let mut weights = weights::load(checkpoint, DEFAULT_GRAPH_EXTRA_BYTES, f16)?;
        let graph = build_graph_until(&mut weights, geometry.frames, stage)?;
        let session = runtime.compile_graph(
            &weights.ctx,
            &graph.graph,
            &[graph.output],
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )?;
        let frames = geometry.frames;
        Ok(Self {
            geometry,
            weights,
            graph,
            session,
            stft: Stft::bs_roformer(),
            features: vec![0.0; FEATURES * frames],
            spectrum: [
                vec![0.0; FREQ_BINS * frames * 2],
                vec![0.0; FREQ_BINS * frames * 2],
            ],
            mask: vec![0.0; FEATURES * frames],
            inverse_band_counts: inverse_band_counts(),
            // One inverse transform per channel is all the CPU work there is.
            istft_threads: istft_threads().min(AUDIO_CHANNELS),
        })
    }

    pub fn checkpoint_path(&self) -> &Path {
        &self.weights.path
    }

    /// The chunk this model was compiled for.
    pub fn geometry(&self) -> ChunkGeometry {
        self.geometry
    }

    /// The mask the last [`separate_chunk`](Self::separate_chunk) applied:
    /// `[4100, frames]` in the spectrum's own feature order, the band masks
    /// averaged over the bands that cover each bin. All zero before the
    /// first chunk.
    pub fn last_mask(&self) -> &[f32] {
        &self.mask
    }

    /// STFT of both channels into the graph's input, then the graph. Returns
    /// whatever tensor the graph was built to return.
    fn forward(&mut self, chunk: &StereoBuf) -> Result<Vec<u8>> {
        let samples = self.geometry.samples;
        let frames = self.geometry.frames;
        if chunk.left.len() != samples || chunk.right.len() != samples {
            return Err(DiffusionError::model(format!(
                "vocals: chunk must be {samples} frames per channel, got {}/{}",
                chunk.left.len(),
                chunk.right.len()
            )));
        }
        for ch in 0..AUDIO_CHANNELS {
            let (spec, got) = self.stft.forward(chunk.channel(ch));
            if got != frames {
                return Err(DiffusionError::model(format!(
                    "vocals: stft produced {got} frames, expected {frames}"
                )));
            }
            self.spectrum[ch].copy_from_slice(&spec);
        }
        pack_features(&self.spectrum, &mut self.features, frames);
        let mut execution = self.session.execute(
            &self.weights.ctx,
            &[(self.graph.features, as_bytes(&self.features))],
            &[self.graph.output],
        )?;
        execution
            .outputs
            .remove(&self.graph.output)
            .ok_or_else(|| DiffusionError::model("vocals: graph returned no output"))
    }

    /// Separates exactly one chunk of [`geometry`](Self::geometry) samples.
    ///
    /// `chunk` must already be padded to that length — the caller owns the
    /// reference's reflect/constant padding rules (see `demix.rs`).
    pub fn separate_chunk(&mut self, chunk: &StereoBuf) -> Result<StereoBuf> {
        let samples = self.geometry.samples;
        let frames = self.geometry.frames;
        let bytes = self.forward(chunk)?;
        let band_masks = f32_from_bytes(&bytes)?;
        if band_masks.len() != BAND_FEATURES * frames {
            return Err(DiffusionError::model(format!(
                "vocals: band masks have {} floats, expected {}",
                band_masks.len(),
                BAND_FEATURES * frames
            )));
        }
        average_band_masks(band_masks, &self.inverse_band_counts, &mut self.mask, frames);

        // -- complex mask, DC out, inverse STFT: one task per channel --
        let spectrum = &self.spectrum;
        let mask = &self.mask;
        let stft = &self.stft;
        let channel = move |ch: usize| {
            let mut masked = vec![0.0f32; FREQ_BINS * frames * 2];
            apply_mask(&spectrum[ch], mask, ch, &mut masked, frames);
            zero_dc(&mut masked, frames);
            stft.inverse(&masked, frames, samples)
        };
        if self.istft_threads < AUDIO_CHANNELS {
            return Ok(StereoBuf {
                left: channel(0),
                right: channel(1),
            });
        }
        std::thread::scope(|scope| -> Result<StereoBuf> {
            let right = scope.spawn(move || channel(1));
            let left = channel(0);
            // A panic in the worker must surface as an error, not as a
            // silent channel.
            let right = right
                .join()
                .map_err(|_| DiffusionError::model("vocals: inverse-STFT worker panicked"))?;
            Ok(StereoBuf { left, right })
        })
    }
}

impl ChunkSeparator for VocalsModel {
    fn geometry(&self) -> ChunkGeometry {
        VocalsModel::geometry(self)
    }

    fn targets(&self) -> usize {
        NUM_TARGETS
    }

    fn separate(&mut self, chunk: &StereoBuf) -> Result<Vec<StereoBuf>> {
        Ok(vec![self.separate_chunk(chunk)?])
    }
}

/// The reference's scatter-add and divide: each band's mask is added into the
/// spectrum features the band covers, and every feature is then divided by
/// the number of bands that reached it.
///
/// `band_masks` is `[7916, frames]` and `out` is `[4100, frames]`, both
/// frame-major. A band's mask and the band's run of the spectrum share one
/// order, so a band is a slice added onto a slice.
fn average_band_masks(band_masks: &[f32], inverse_counts: &[f32], out: &mut [f32], frames: usize) {
    out.fill(0.0);
    for frame in 0..frames {
        let src = &band_masks[frame * BAND_FEATURES..(frame + 1) * BAND_FEATURES];
        let dst = &mut out[frame * FEATURES..(frame + 1) * FEATURES];
        for band in 0..NUM_BANDS {
            let width = band_width(band);
            let from = band_mask_offset(band);
            let to = band_feature_offset(band);
            for (sum, value) in dst[to..to + width].iter_mut().zip(&src[from..from + width]) {
                *sum += value;
            }
        }
        for (sum, inverse) in dst.iter_mut().zip(inverse_counts) {
            *sum *= inverse;
        }
    }
}

/// Clears frequency 0 of one channel's spectrum in every frame, which is what
/// the reference's `zero_dc` does to the masked spectrum before inverting it.
fn zero_dc(spectrum: &mut [f32], frames: usize) {
    spectrum[..frames * 2].fill(0.0);
}

/// The accompaniment of a one-target separation: whatever of the mix is not
/// the vocal, sample by sample. This is the reference's own definition of the
/// second instrument of a single-target model, and it makes the two lanes sum
/// back to the mix to within the rounding of one subtraction and one
/// addition.
pub fn instrumental(mix: &StereoBuf, vocals: &StereoBuf) -> StereoBuf {
    let lane = |mix: &[f32], vocals: &[f32]| -> Vec<f32> {
        mix.iter().zip(vocals).map(|(m, v)| m - v).collect()
    };
    StereoBuf {
        left: lane(&mix.left, &vocals.left),
        right: lane(&mix.right, &vocals.right),
    }
}

/// A forward built only as far as one [`Stage`], for holding that stage
/// against a reference. It is a whole separate load and compile, so it costs
/// the separator nothing; it is a fault-finding tool and no part of a
/// separation.
pub struct StageProbe {
    model: VocalsModel,
}

impl StageProbe {
    pub fn load(
        checkpoint: impl AsRef<Path>,
        geometry: ChunkGeometry,
        stage: Stage,
    ) -> Result<Self> {
        let runtime = DeviceRuntime::new()?;
        Ok(Self {
            model: VocalsModel::build(checkpoint, runtime, geometry, stage)?,
        })
    }

    /// Runs one chunk as far as the probe's stage.
    ///
    /// The trunk stages come back `[frame][band][384]` whichever layout the
    /// graph holds them in, so one comparison serves them all;
    /// [`Stage::Masks`] comes back `[frame][7916]`, the band masks before
    /// they are averaged.
    pub fn run(&mut self, chunk: &StereoBuf) -> Result<Vec<f32>> {
        let frames = self.model.geometry.frames;
        let bytes = self.model.forward(chunk)?;
        let values = f32_from_bytes(&bytes)?;
        let want = match self.model.graph.stage {
            Stage::Masks => BAND_FEATURES * frames,
            Stage::BandSplit | Stage::Layer(_) => DIM * NUM_BANDS * frames,
        };
        if values.len() != want {
            return Err(DiffusionError::model(format!(
                "vocals: stage {:?} returned {} floats, expected {want}",
                self.model.graph.stage,
                values.len()
            )));
        }
        match self.model.graph.stage {
            // TIME layout reads back `[band][frame][384]`.
            Stage::BandSplit => {
                let mut out = vec![0.0f32; values.len()];
                for band in 0..NUM_BANDS {
                    for frame in 0..frames {
                        let from = (band * frames + frame) * DIM;
                        let to = (frame * NUM_BANDS + band) * DIM;
                        out[to..to + DIM].copy_from_slice(&values[from..from + DIM]);
                    }
                }
                Ok(out)
            }
            Stage::Layer(_) | Stage::Masks => Ok(values.to_vec()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, mut seed: u32) -> Vec<f32> {
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / 8388608.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn overlapping_band_masks_average_and_lone_ones_pass_through() {
        let frames = 3;
        let band_masks = noise(BAND_FEATURES * frames, 7);
        let mut out = vec![f32::NAN; FEATURES * frames];
        average_band_masks(&band_masks, &inverse_band_counts(), &mut out, frames);

        // The same sum, written feature by feature instead of band by band.
        for frame in 0..frames {
            for feature in 0..FEATURES {
                let bin = feature / 4;
                let mut sum = 0.0f32;
                let mut covering = 0;
                for band in 0..NUM_BANDS {
                    let (first, end) = BAND_BINS[band];
                    if (first..end).contains(&bin) {
                        let at = band_mask_offset(band) + feature - band_feature_offset(band);
                        sum += band_masks[frame * BAND_FEATURES + at];
                        covering += 1;
                    }
                }
                let want = sum / covering as f32;
                assert_eq!(out[frame * FEATURES + feature], want, "frame {frame} feature {feature}");
            }
        }
        // Bin 0 is in band 0 alone: its mask is band 0's, untouched.
        assert_eq!(out[..4], band_masks[..4]);
        // The top bin is in band 59 alone, at the very end of both vectors.
        assert_eq!(out[FEATURES - 4..FEATURES], band_masks[BAND_FEATURES - 4..BAND_FEATURES]);
    }

    #[test]
    fn a_unit_mask_in_every_band_averages_to_a_unit_mask() {
        // If every band says "keep this bin", the average must say so too,
        // wherever one band speaks and wherever two do.
        let frames = 2;
        let mut band_masks = vec![0.0f32; BAND_FEATURES * frames];
        for slot in band_masks.chunks_exact_mut(2) {
            slot[0] = 1.0;
        }
        let mut out = vec![0.0f32; FEATURES * frames];
        average_band_masks(&band_masks, &inverse_band_counts(), &mut out, frames);
        for slot in out.chunks_exact(2) {
            assert_eq!(slot, [1.0, 0.0]);
        }
    }

    #[test]
    fn only_frequency_zero_is_cleared() {
        let frames = 5;
        let mut spectrum = vec![1.0f32; FREQ_BINS * frames * 2];
        zero_dc(&mut spectrum, frames);
        assert!(spectrum[..frames * 2].iter().all(|v| *v == 0.0));
        assert!(spectrum[frames * 2..].iter().all(|v| *v == 1.0));
    }

    #[test]
    fn the_instrumental_and_the_vocal_sum_back_to_the_mix() {
        // Mix and vocal at programme level. Each sample goes through one
        // rounded subtraction and one rounded addition, each good to half a
        // unit in the last place of a value below 2, so the round trip is
        // within one such unit of the mix.
        let mix = StereoBuf {
            left: noise(100_000, 1).iter().map(|v| v * 0.9).collect(),
            right: noise(100_000, 2).iter().map(|v| v * 0.9).collect(),
        };
        let vocals = StereoBuf {
            left: noise(100_000, 3).iter().map(|v| v * 0.5).collect(),
            right: noise(100_000, 4).iter().map(|v| v * 0.5).collect(),
        };
        let rest = instrumental(&mix, &vocals);
        assert_eq!(rest.frames(), mix.frames());
        for ch in 0..AUDIO_CHANNELS {
            for i in 0..mix.frames() {
                let back = rest.channel(ch)[i] + vocals.channel(ch)[i];
                let error = (back - mix.channel(ch)[i]).abs();
                assert!(error <= f32::EPSILON, "ch{ch}[{i}] is off by {error:e}");
            }
        }
    }
}
