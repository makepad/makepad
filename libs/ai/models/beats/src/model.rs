//! Model ownership, reference chunk stitching, and minimal postprocessing.

use crate::config::*;
use crate::graph::{build_graph, BeatsGraph};
use crate::mel::LogMelSpect;
use crate::weights::{BeatsWeights, DEFAULT_GRAPH_EXTRA_BYTES};
use makepad_ai_common::backend::{
    BufferStorageMode, DeviceGraphSession, DeviceRuntime, GraphDevice,
};
use makepad_ai_common::{DiffusionError, Result};
use std::cmp::Ordering;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeatAnalysis {
    pub beats_secs: Vec<f64>,
    pub downbeats_secs: Vec<f64>,
    pub bpm: f64,
    pub confidence: f32,
    pub frame_rate: f64,
    pub beat_prob: Vec<f32>,
    pub downbeat_prob: Vec<f32>,
}

pub struct BeatsModel {
    weights: BeatsWeights,
    graph: BeatsGraph,
    session: DeviceGraphSession,
    mel_frontend: LogMelSpect,
    chunk_mel: Vec<f32>,
}

impl BeatsModel {
    pub fn load(checkpoint: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_runtime(checkpoint, DeviceRuntime::new()?)
    }

    pub fn load_with_runtime(
        checkpoint: impl AsRef<Path>,
        runtime: DeviceRuntime,
    ) -> Result<Self> {
        let f16 = match runtime.device() {
            GraphDevice::Metal => crate::weights::f16_weights_enabled(),
            GraphDevice::Cuda => crate::weights::f16_weights_requested(),
        };
        let mut weights =
            BeatsWeights::load_with_options(checkpoint, DEFAULT_GRAPH_EXTRA_BYTES, f16)?;
        let graph = build_graph(&mut weights)?;
        let session = runtime.compile_graph(
            &weights.ctx,
            &graph.graph,
            &[graph.logits],
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )?;
        Ok(Self {
            weights,
            graph,
            session,
            mel_frontend: LogMelSpect::new(),
            chunk_mel: vec![0.0; CHUNK_FRAMES * MEL_BINS],
        })
    }

    pub fn checkpoint_path(&self) -> &Path {
        &self.weights.path
    }

    pub fn analyze(&mut self, mono_22k: &[f32]) -> Result<BeatAnalysis> {
        self.analyze_with_progress(mono_22k, &mut |_, _| Ok(()))
    }

    /// As [`Self::analyze`], with one cancellation/progress boundary before
    /// inference and after every chunk.
    pub fn analyze_with_progress(
        &mut self,
        mono_22k: &[f32],
        progress: &mut dyn FnMut(usize, usize) -> Result<()>,
    ) -> Result<BeatAnalysis> {
        if mono_22k.is_empty() {
            return Ok(BeatAnalysis {
                frame_rate: FRAME_RATE,
                ..BeatAnalysis::default()
            });
        }
        let (mel, frames) = self.mel_frontend.compute(mono_22k);
        let starts = chunk_starts(frames);
        let total_chunks = starts.len();
        progress(0, total_chunks)?;
        let mut beat_logits = vec![-1000.0f32; frames];
        let mut downbeat_logits = vec![-1000.0f32; frames];

        for (chunk_index, &start) in starts.iter().enumerate() {
            self.chunk_mel.fill(0.0);
            for local in 0..CHUNK_FRAMES {
                let global = start + local as isize;
                if global >= 0 && (global as usize) < frames {
                    let source = global as usize * MEL_BINS;
                    let destination = local * MEL_BINS;
                    self.chunk_mel[destination..destination + MEL_BINS]
                        .copy_from_slice(&mel[source..source + MEL_BINS]);
                }
            }
            let execution = self.session.execute(
                &self.weights.ctx,
                &[(self.graph.mel, as_bytes(&self.chunk_mel))],
                &[self.graph.logits],
            )?;
            let bytes = execution.outputs.get(&self.graph.logits).ok_or_else(|| {
                DiffusionError::model("beats graph returned no logits tensor")
            })?;
            if bytes.len() != CHUNK_FRAMES * 2 * 4 {
                return Err(DiffusionError::model(format!(
                    "beats graph returned {} logit bytes, expected {}",
                    bytes.len(),
                    CHUNK_FRAMES * 2 * 4
                )));
            }

            // Forward order + write-if-empty is reference `keep_first`.
            for local in BORDER_FRAMES..CHUNK_FRAMES - BORDER_FRAMES {
                let global = start + local as isize;
                if global < 0 || global as usize >= frames {
                    continue;
                }
                let global = global as usize;
                if beat_logits[global] != -1000.0 {
                    continue;
                }
                beat_logits[global] = read_f32(bytes, local * 2)?;
                downbeat_logits[global] = read_f32(bytes, local * 2 + 1)?;
            }
            progress(chunk_index + 1, total_chunks)?;
        }

        let beat_prob: Vec<f32> = beat_logits.iter().copied().map(sigmoid).collect();
        let downbeat_prob: Vec<f32> = downbeat_logits.iter().copied().map(sigmoid).collect();
        Ok(postprocess(beat_prob, downbeat_prob))
    }
}

pub fn chunk_starts(frames: usize) -> Vec<isize> {
    if frames == 0 {
        return Vec::new();
    }
    let mut starts = Vec::new();
    let mut start = -(BORDER_FRAMES as isize);
    let stop = frames as isize - BORDER_FRAMES as isize;
    while start < stop {
        starts.push(start);
        start += CHUNK_STRIDE as isize;
    }
    if frames > CHUNK_STRIDE {
        *starts.last_mut().unwrap() = frames as isize - (CHUNK_FRAMES - BORDER_FRAMES) as isize;
    }
    starts
}

pub fn postprocess(beat_prob: Vec<f32>, downbeat_prob: Vec<f32>) -> BeatAnalysis {
    let beat_peaks = local_peaks(&beat_prob);
    let downbeat_peaks = local_peaks(&downbeat_prob);
    let beat_frames = deduplicate_adjacent(&beat_peaks);
    let mut downbeat_frames = deduplicate_adjacent(&downbeat_peaks);

    if !beat_frames.is_empty() {
        for frame in &mut downbeat_frames {
            *frame = beat_frames
                .iter()
                .copied()
                .min_by(|a, b| {
                    (a - *frame)
                        .abs()
                        .partial_cmp(&(b - *frame).abs())
                        .unwrap_or(Ordering::Equal)
                })
                .unwrap();
        }
        downbeat_frames.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        downbeat_frames.dedup_by(|a, b| *a == *b);
    }

    let beats_secs: Vec<f64> = beat_frames.iter().map(|frame| frame / FRAME_RATE).collect();
    let downbeats_secs: Vec<f64> = downbeat_frames
        .iter()
        .map(|frame| frame / FRAME_RATE)
        .collect();
    let bpm = estimate_bpm(&beats_secs);
    let confidence = if beat_frames.is_empty() {
        0.0
    } else {
        beat_frames
            .iter()
            .map(|frame| {
                let index = frame.round().clamp(0.0, beat_prob.len().saturating_sub(1) as f64);
                beat_prob[index as usize]
            })
            .sum::<f32>()
            / beat_frames.len() as f32
    };
    BeatAnalysis {
        beats_secs,
        downbeats_secs,
        bpm,
        confidence,
        frame_rate: FRAME_RATE,
        beat_prob,
        downbeat_prob,
    }
}

fn local_peaks(probability: &[f32]) -> Vec<usize> {
    let mut peaks = Vec::new();
    for (frame, &value) in probability.iter().enumerate() {
        if value <= 0.5 {
            continue;
        }
        let from = frame.saturating_sub(3);
        let to = (frame + 3).min(probability.len().saturating_sub(1));
        if probability[from..=to]
            .iter()
            .all(|&candidate| value >= candidate)
        {
            peaks.push(frame);
        }
    }
    peaks
}

fn deduplicate_adjacent(peaks: &[usize]) -> Vec<f64> {
    let Some((&first, rest)) = peaks.split_first() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut mean = first as f64;
    let mut previous = first;
    let mut count = 1usize;
    for &peak in rest {
        if peak - previous <= 1 {
            count += 1;
            mean += (peak as f64 - mean) / count as f64;
        } else {
            result.push(mean);
            mean = peak as f64;
            count = 1;
        }
        previous = peak;
    }
    result.push(mean);
    result
}

fn estimate_bpm(beats: &[f64]) -> f64 {
    if beats.len() < 2 {
        return 0.0;
    }
    let mut intervals: Vec<f64> = beats.windows(2).map(|pair| pair[1] - pair[0]).collect();
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let middle = intervals.len() / 2;
    let median = if intervals.len() % 2 == 0 {
        (intervals[middle - 1] + intervals[middle]) * 0.5
    } else {
        intervals[middle]
    };
    if median > 0.0 {
        60.0 / median
    } else {
        0.0
    }
}

fn sigmoid(value: f32) -> f32 {
    if value >= 0.0 {
        1.0 / (1.0 + (-value).exp())
    } else {
        let exponential = value.exp();
        exponential / (1.0 + exponential)
    }
}

fn read_f32(bytes: &[u8], index: usize) -> Result<f32> {
    let at = index
        .checked_mul(4)
        .ok_or_else(|| DiffusionError::model("beats logit index overflow"))?;
    let value = bytes
        .get(at..at + 4)
        .ok_or_else(|| DiffusionError::model("beats logit output is truncated"))?;
    Ok(f32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn as_bytes(values: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), values.len() * 4) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use std::path::{Path, PathBuf};

    const WEIGHTS: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../local/models/weights/beat_this/final0.ckpt"
    );

    fn checkpoint() -> PathBuf {
        Path::new(WEIGHTS).to_path_buf()
    }

    fn click_track(seconds: usize, bpm: f64) -> Vec<f32> {
        let mut audio = vec![0.0f32; seconds * SAMPLE_RATE as usize];
        let beat_samples = (60.0 / bpm * SAMPLE_RATE as f64).round() as usize;
        for (beat, at) in (0..audio.len()).step_by(beat_samples).enumerate() {
            let amplitude = if beat % 4 == 0 { 1.0 } else { 0.55 };
            for offset in 0..220usize.min(audio.len() - at) {
                let envelope = (-(offset as f64) / 45.0).exp();
                audio[at + offset] +=
                    (amplitude * envelope * (2.0 * PI * 1000.0 * offset as f64 / SAMPLE_RATE as f64).sin()) as f32;
            }
        }
        audio
    }

    #[test]
    fn chunk_starts_match_reference_shifted_tail() {
        assert_eq!(chunk_starts(1000), vec![-6]);
        assert_eq!(chunk_starts(1501), vec![-6, 7]);
        assert_eq!(chunk_starts(4501), vec![-6, 1482, 2970, 3007]);
    }

    #[test]
    fn minimal_postprocessor_snaps_downbeats_and_estimates_bpm() {
        let mut beat = vec![0.0f32; 501];
        let mut downbeat = vec![0.0f32; 501];
        for frame in (0..=500).step_by(25) {
            beat[frame] = 0.9;
        }
        for frame in (1..=500).step_by(100) {
            downbeat[frame] = 0.8;
        }
        let analysis = postprocess(beat, downbeat);
        assert!((analysis.bpm - 120.0).abs() < 1e-9);
        assert!(analysis
            .downbeats_secs
            .iter()
            .all(|time| analysis.beats_secs.contains(time)));
        assert_eq!(analysis.frame_rate, 50.0);
    }

    #[test]
    fn silence_probabilities_produce_no_events() {
        let analysis = postprocess(vec![0.5; 100], vec![0.5; 100]);
        assert!(analysis.beats_secs.is_empty());
        assert!(analysis.downbeats_secs.is_empty());
        assert_eq!(analysis.bpm, 0.0);
        assert_eq!(analysis.confidence, 0.0);
    }

    #[test]
    #[ignore = "requires seeded final0.ckpt and Metal/CUDA graph execution"]
    fn synthetic_120_bpm_click_track_tracks_beats_and_downbeats() {
        let expected: Vec<f64> = (0..40).map(|beat| beat as f64 * 0.5).collect();
        let analysis = BeatsModel::load(checkpoint())
            .unwrap()
            .analyze(&click_track(20, 120.0))
            .unwrap();
        let matched = expected
            .iter()
            .filter(|&&time| {
                analysis
                    .beats_secs
                    .iter()
                    .any(|beat| (beat - time).abs() <= 0.03)
            })
            .count();
        assert!(matched >= expected.len() * 3 / 4, "matched {matched}/{}", expected.len());
        assert!((analysis.bpm - 120.0).abs() <= 1.0);
        let downbeats: Vec<f64> = expected.iter().step_by(4).copied().collect();
        let precise = analysis
            .downbeats_secs
            .iter()
            .filter(|&&time| downbeats.iter().any(|want| (time - want).abs() <= 0.03))
            .count();
        assert!(analysis.downbeats_secs.is_empty() || precise * 4 >= analysis.downbeats_secs.len() * 3);
    }

    #[test]
    #[ignore = "requires seeded final0.ckpt and Metal/CUDA graph execution"]
    fn model_silence_has_no_beats() {
        let analysis = BeatsModel::load(checkpoint())
            .unwrap()
            .analyze(&vec![0.0; SAMPLE_RATE as usize * 20])
            .unwrap();
        assert!(analysis.beats_secs.is_empty());
        assert!(analysis.downbeats_secs.is_empty());
    }

    #[test]
    #[ignore = "requires seeded final0.ckpt and Metal/CUDA graph execution"]
    fn ninety_seconds_is_continuous_across_chunk_seams() {
        let analysis = BeatsModel::load(checkpoint())
            .unwrap()
            .analyze(&click_track(90, 120.0))
            .unwrap();
        assert!(analysis
            .beats_secs
            .windows(2)
            .all(|pair| pair[1] - pair[0] > 0.03 && pair[1] - pair[0] < 0.8));
        for seam in [1488.0 / 50.0, 2976.0 / 50.0] {
            assert!(analysis.beats_secs.iter().any(|beat| (beat - seam).abs() < 0.55));
        }
    }
}
