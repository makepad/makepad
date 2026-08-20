//! One chunk in, four stems out.
//!
//! Owns the compiled forward graph plus the CPU transform halves that sandwich
//! it: STFT -> [graph] -> complex mask -> inverse STFT.

use crate::config::*;
use crate::graph::{build_graph, StemsGraph};
use crate::stft::Stft;
use crate::weights::{StemsWeights, DEFAULT_GRAPH_EXTRA_BYTES};
use makepad_ai_common::backend::{
    BufferStorageMode, DeviceGraphSession, DeviceRuntime, GraphDevice,
};
use makepad_ai_common::{DiffusionError, Result, TensorId};
use std::path::Path;

/// Command-buffer budget this model applies to the runtime it drives.
///
/// A chunk is ~700 GPU dispatches. Left at the runtime's throughput defaults
/// those land in THREE command buffers of roughly half a second each, and an
/// Apple GPU only preempts between command buffers — so a co-tenant trying to
/// present a frame can wait that long. Rolling every `CB_MAX_OPS` dispatches
/// cuts the worst-case wait to tens of milliseconds for a host-side cost of a
/// few extra commits per chunk. Set `MAKEPAD_STEMS_CB_OPS=0` to leave the
/// runtime's defaults alone (batch use with no interactive co-tenant).
pub const CB_MAX_OPS: usize = 32;
/// Shared-buffer byte budget paired with `CB_MAX_OPS`. Our one main buffer is
/// multi-gigabyte and counted once per command buffer, so this alone would
/// roll on every op; the op count is what actually governs. Kept generous so
/// it never becomes the binding constraint by accident.
pub const CB_MAX_BYTES: usize = 8 << 30;

fn command_buffer_ops_limit() -> Option<usize> {
    parse_cb_ops(std::env::var("MAKEPAD_STEMS_CB_OPS").ok().as_deref())
}

/// `None` = leave the runtime's own defaults in place.
fn parse_cb_ops(value: Option<&str>) -> Option<usize> {
    match value {
        None => Some(CB_MAX_OPS),
        Some(value) => match value.trim().parse::<usize>() {
            Ok(0) => None,
            Ok(ops) => Some(ops),
            // Unparseable input must not silently disable the co-tenancy
            // budget; fall back to the default rather than to "off".
            Err(_) => Some(CB_MAX_OPS),
        },
    }
}

/// Worker threads for the per-(stem, channel) inverse STFT. Eight independent
/// transforms exist per chunk; the default deliberately leaves cores for a
/// host app's UI and audio threads rather than taking every core for ~100 ms.
fn istft_threads() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    parse_istft_threads(
        std::env::var("MAKEPAD_STEMS_ISTFT_THREADS").ok().as_deref(),
        cores,
    )
}

fn parse_istft_threads(value: Option<&str>, cores: usize) -> usize {
    let tasks = NUM_STEMS * AUDIO_CHANNELS;
    if let Some(value) = value {
        if let Ok(threads) = value.trim().parse::<usize>() {
            return threads.clamp(1, tasks);
        }
    }
    (cores / 2).clamp(1, tasks)
}

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
    session: DeviceGraphSession,
    stft: Stft,
    /// Reused per chunk so a long demix does not churn the allocator.
    features: Vec<f32>,
    spectrum: [Vec<f32>; AUDIO_CHANNELS],
    istft_threads: usize,
}

impl StemsModel {
    /// Loads the checkpoint and compiles the forward graph for the device
    /// runtime. Expensive (seconds): do it once, off any latency-sensitive
    /// thread.
    pub fn load(checkpoint: impl AsRef<Path>) -> Result<Self> {
        let runtime = DeviceRuntime::new()?;
        Self::load_with_runtime(checkpoint, runtime)
    }

    /// NOTE: on Metal this configures `runtime`'s command-buffer budget for
    /// interactive co-tenancy (see [`CB_MAX_OPS`]). Each Metal `Runtime` owns
    /// its own command queue, so only this separator's submissions are
    /// affected. CUDA has no equivalent knob — its dispatches are already
    /// individually preemptible — so the budget is simply not applied there.
    pub fn load_with_runtime(checkpoint: impl AsRef<Path>, runtime: DeviceRuntime) -> Result<Self> {
        if let DeviceRuntime::Metal(metal) = &runtime {
            if let Some(ops) = command_buffer_ops_limit() {
                metal.set_command_buffer_limits(ops, CB_MAX_BYTES);
            }
        }
        // Half-precision matmul weights are a Metal-side win (faster AND
        // slightly more accurate there, and half the resident bytes). On CUDA
        // they are a loss: cuBLAS has no f16-weight x f32-activation GEMM, so
        // an f16 weight would fall off the GEMM path onto the generic batched
        // kernel that re-reads the activations once per output row. f32 is
        // also the arithmetic the oracle-parity gate is measured in.
        let f16 = match runtime.device() {
            GraphDevice::Metal => crate::weights::f16_weights_enabled(),
            GraphDevice::Cuda => crate::weights::f16_weights_requested(),
        };
        let mut weights =
            StemsWeights::load_with_options(checkpoint, DEFAULT_GRAPH_EXTRA_BYTES, f16)?;
        let graph = build_graph(&mut weights)?;
        let session = runtime.compile_graph(
            &weights.ctx,
            &graph.graph,
            &graph.masks,
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
            istft_threads: istft_threads(),
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
        let execution = self.session.execute(
            &self.weights.ctx,
            &[(self.graph.features, as_bytes(&self.features))],
            &outputs,
        )?;
        let t_forward = t0.elapsed();

        // -- complex mask + inverse STFT --
        // The eight (stem, channel) reconstructions are fully independent, and
        // together they are the whole CPU cost of a chunk (the profile puts
        // ~85% of on-CPU time in `Stft::inverse` and its FFT). Run them on a
        // small pool: same arithmetic, a shorter window of CPU interference
        // for whatever else the host is doing.
        let mut masks: Vec<&[f32]> = Vec::with_capacity(NUM_STEMS);
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
            masks.push(mask);
        }

        let tasks: Vec<(usize, usize)> = (0..NUM_STEMS)
            .flat_map(|stem| (0..AUDIO_CHANNELS).map(move |ch| (stem, ch)))
            .collect();
        let spectrum = &self.spectrum;
        let stft = &self.stft;
        let masks = &masks;
        let threads = self.istft_threads.min(tasks.len()).max(1);
        let per_thread = tasks.len().div_ceil(threads);
        let mut done: Vec<(usize, usize, Vec<f32>)> = Vec::with_capacity(tasks.len());
        std::thread::scope(|scope| -> Result<()> {
            let mut handles = Vec::with_capacity(threads);
            for group in tasks.chunks(per_thread) {
                handles.push(scope.spawn(move || {
                    let mut masked = vec![0.0f32; FREQ_BINS * CHUNK_FRAMES * 2];
                    group
                        .iter()
                        .map(|&(stem, ch)| {
                            apply_mask(&spectrum[ch], masks[stem], ch, &mut masked);
                            (stem, ch, stft.inverse(&masked, CHUNK_FRAMES, CHUNK_SAMPLES))
                        })
                        .collect::<Vec<_>>()
                }));
            }
            for handle in handles {
                // A panic in a worker must surface as an error, not as a
                // silently short stem set.
                let part = handle.join().map_err(|_| {
                    DiffusionError::model("stems: inverse-STFT worker panicked")
                })?;
                done.extend(part);
            }
            Ok(())
        })?;

        let mut stems = empty_stem_set(0);
        for (stem, ch, samples) in done {
            *stems[stem].channel_mut(ch) = samples;
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
    fn command_buffer_budget_parses() {
        assert_eq!(parse_cb_ops(None), Some(CB_MAX_OPS));
        assert_eq!(parse_cb_ops(Some("64")), Some(64));
        // Explicit zero is the documented opt-out.
        assert_eq!(parse_cb_ops(Some("0")), None);
        // Garbage must not read as "opt out".
        assert_eq!(parse_cb_ops(Some("banana")), Some(CB_MAX_OPS));
        assert_eq!(parse_cb_ops(Some("")), Some(CB_MAX_OPS));
        // 32 ops over ~713 dispatches is ~24 command buffers, measured as the
        // knee: finer is free, coarser costs throughput.
        assert!(CB_MAX_OPS > 0 && CB_MAX_OPS <= 64);
    }

    #[test]
    fn istft_thread_count_is_clamped_to_the_work() {
        let tasks = NUM_STEMS * AUDIO_CHANNELS;
        // Never more threads than there are independent transforms.
        assert_eq!(parse_istft_threads(Some("64"), 16), tasks);
        assert_eq!(parse_istft_threads(Some("1"), 16), 1);
        // Never zero.
        assert_eq!(parse_istft_threads(Some("0"), 16), 1);
        assert_eq!(parse_istft_threads(Some("nope"), 16), 8);
        // Default leaves half the cores for the host's UI and audio threads.
        assert_eq!(parse_istft_threads(None, 16), tasks);
        assert_eq!(parse_istft_threads(None, 8), 4);
        assert_eq!(parse_istft_threads(None, 2), 1);
        assert_eq!(parse_istft_threads(None, 1), 1);
    }

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
