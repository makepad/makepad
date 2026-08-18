//! Bounded, decoder-independent video loop analysis.
//!
//! Decode/downscale happens elsewhere.  This module consumes compact signatures,
//! caps work to 384 frames and 256 values per channel, and uses only local
//! appearance recurrence plus motion summaries.  The result is advisory: low
//! confidence explicitly returns `None`, allowing the cue engine to make a
//! beat-quantized cut instead of pretending a bad seam is a loop.

pub const MAX_ANALYSIS_FRAMES: usize = 384;
pub const MAX_CHANNEL_VALUES: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionSummary {
    /// Mean horizontal block motion in normalized frame units.
    pub x: f32,
    /// Mean vertical block motion in normalized frame units.
    pub y: f32,
    /// Mean expansion/contraction; useful for zooming shots.
    pub divergence: f32,
    /// Fraction/strength of blocks carrying reliable motion.
    pub activity: f32,
}

impl MotionSummary {
    pub fn new(x: f32, y: f32, divergence: f32, activity: f32) -> Self {
        Self {
            x: finite_or_zero(x),
            y: finite_or_zero(y),
            divergence: finite_or_zero(divergence),
            activity: finite_or_zero(activity).clamp(0.0, 1.0),
        }
    }

    fn speed(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.divergence * self.divergence).sqrt()
            * self.activity
    }
}

/// A compact frame description supplied by the decoder/vision worker.
///
/// Channel values should conventionally be normalized to `[0, 1]`.  Mismatched
/// or oversized channels are handled deterministically; only the first 256
/// values of each are read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSignature {
    pub luma_blocks: Vec<f32>,
    pub chroma_blocks: Vec<[f32; 2]>,
    pub edge_blocks: Vec<f32>,
    pub motion: MotionSummary,
}

impl FrameSignature {
    pub fn new(
        luma_blocks: Vec<f32>,
        chroma_blocks: Vec<[f32; 2]>,
        edge_blocks: Vec<f32>,
        motion: MotionSummary,
    ) -> Self {
        Self {
            luma_blocks,
            chroma_blocks,
            edge_blocks,
            motion,
        }
    }

    /// Convenient minimal signature for tests or an inexpensive first pass.
    pub fn from_luma(luma_blocks: Vec<f32>, motion: MotionSummary) -> Self {
        Self::new(luma_blocks, Vec::new(), Vec::new(), motion)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopKind {
    Static,
    Wrap,
    PingPong,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreativeLoopRecommendation {
    /// A still can be held for any musical duration without a visual seam.
    Hold,
    /// Use the discovered source window as a normal cyclic loop.
    CleanWrap,
    /// Rotate the source loop so the discovered cut is at `offset_frames`.
    PhaseShift { offset_frames: usize },
    /// Play forward then backward; the source already exhibits this symmetry.
    Boomerang { turnaround_frame: usize },
    /// Preserve directional flow and crossfade on a motion-compatible seam.
    MotionMatchedCrossfade,
    /// No honest long loop was found; use short, beat-quantized slices instead.
    BeatRepeat,
    #[default]
    AvoidLoop,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoopDetection {
    pub kind: LoopKind,
    /// Inclusive source-frame index.
    pub window_start: usize,
    /// Exclusive source-frame index.
    pub window_end: usize,
    pub period_frames: usize,
    /// Recommended cut phase, relative to `window_start`.
    pub phase_frame: usize,
    pub confidence: f32,
    pub support_cycles: f32,
    pub recurrence_score: f32,
    pub seam_score: f32,
    pub motion_score: f32,
    pub internal_cut_penalty: f32,
    pub recommendation: CreativeLoopRecommendation,
    pub frames_considered: usize,
    pub input_was_capped: bool,
}

impl LoopDetection {
    pub fn is_usable(&self) -> bool {
        self.kind != LoopKind::None && self.confidence > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopDetectorConfig {
    pub max_frames: usize,
    pub min_period_frames: usize,
    pub recurrence_tolerance: f32,
    pub min_confidence: f32,
}

impl Default for LoopDetectorConfig {
    fn default() -> Self {
        Self {
            max_frames: MAX_ANALYSIS_FRAMES,
            min_period_frames: 4,
            recurrence_tolerance: 0.115,
            min_confidence: 0.56,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Candidate {
    start: usize,
    end: usize,
    period: usize,
    confidence: f32,
    recurrence: f32,
    seam: f32,
    motion: f32,
    cut_penalty: f32,
    cycles: f32,
}

pub struct LoopDetector {
    config: LoopDetectorConfig,
}

impl Default for LoopDetector {
    fn default() -> Self {
        Self::new(LoopDetectorConfig::default())
    }
}

impl LoopDetector {
    pub fn new(mut config: LoopDetectorConfig) -> Self {
        config.max_frames = config.max_frames.clamp(8, MAX_ANALYSIS_FRAMES);
        config.min_period_frames = config.min_period_frames.max(2);
        config.recurrence_tolerance = config.recurrence_tolerance.clamp(0.02, 0.5);
        config.min_confidence = config.min_confidence.clamp(0.0, 1.0);
        Self { config }
    }

    pub fn analyze(&self, input: &[FrameSignature]) -> LoopDetection {
        let frame_count = input.len().min(self.config.max_frames);
        let capped = input.len() > frame_count;
        let frames = &input[..frame_count];
        if frame_count < 4 {
            return none_detection(frame_count, capped);
        }

        if let Some(static_detection) = self.detect_static(frames, capped) {
            return static_detection;
        }

        let best = self.best_recurrence_candidate(frames);
        if let Some(candidate) = best {
            let ping_pong = ping_pong_symmetry(frames, candidate.start, candidate.period);
            if ping_pong.score >= 0.68 && ping_pong.reversal >= 0.55 {
                let confidence = (candidate.confidence * 0.48 + ping_pong.score * 0.52)
                    .clamp(0.0, 1.0);
                if confidence >= self.config.min_confidence {
                    return LoopDetection {
                        kind: LoopKind::PingPong,
                        window_start: candidate.start,
                        window_end: candidate.end,
                        period_frames: candidate.period,
                        phase_frame: ping_pong.axis % candidate.period,
                        confidence,
                        support_cycles: candidate.cycles,
                        recurrence_score: candidate.recurrence,
                        seam_score: candidate.seam,
                        motion_score: ping_pong.motion_opposition,
                        internal_cut_penalty: candidate.cut_penalty,
                        recommendation: CreativeLoopRecommendation::Boomerang {
                            turnaround_frame: candidate.start + ping_pong.axis,
                        },
                        frames_considered: frame_count,
                        input_was_capped: capped,
                    };
                }
            }

            if candidate.confidence >= self.config.min_confidence {
                let phase = best_cut_phase(frames, candidate.start, candidate.period);
                let recommendation = if candidate.start > 0 || phase > 0 {
                    CreativeLoopRecommendation::PhaseShift {
                        offset_frames: phase,
                    }
                } else if candidate.motion > 0.72 {
                    CreativeLoopRecommendation::MotionMatchedCrossfade
                } else {
                    CreativeLoopRecommendation::CleanWrap
                };
                return LoopDetection {
                    kind: LoopKind::Wrap,
                    window_start: candidate.start,
                    window_end: candidate.end,
                    period_frames: candidate.period,
                    phase_frame: phase,
                    confidence: candidate.confidence,
                    support_cycles: candidate.cycles,
                    recurrence_score: candidate.recurrence,
                    seam_score: candidate.seam,
                    motion_score: candidate.motion,
                    internal_cut_penalty: candidate.cut_penalty,
                    recommendation,
                    frames_considered: frame_count,
                    input_was_capped: capped,
                };
            }
        }

        // A one-shot forward/reverse clip has only one cycle and therefore no
        // recurrence lag.  Test whole-clip temporal symmetry as a final bounded
        // pass, still requiring actual motion reversal to avoid labelling an
        // ordinary symmetric composition as a boomerang.
        let ping_pong = ping_pong_symmetry(frames, 0, frame_count);
        if ping_pong.score >= 0.74 && ping_pong.reversal >= 0.60 {
            return LoopDetection {
                kind: LoopKind::PingPong,
                window_start: 0,
                window_end: frame_count,
                period_frames: frame_count,
                phase_frame: ping_pong.axis,
                confidence: ping_pong.score,
                support_cycles: 1.0,
                recurrence_score: 0.0,
                seam_score: ping_pong.appearance,
                motion_score: ping_pong.motion_opposition,
                internal_cut_penalty: 0.0,
                recommendation: CreativeLoopRecommendation::Boomerang {
                    turnaround_frame: ping_pong.axis,
                },
                frames_considered: frame_count,
                input_was_capped: capped,
            };
        }

        let mut none = none_detection(frame_count, capped);
        // If there was weak periodic evidence, beat-repeat is a more useful
        // creative fallback than an arbitrary long wrap.
        if best.map(|candidate| candidate.confidence > 0.32).unwrap_or(false) {
            none.recommendation = CreativeLoopRecommendation::BeatRepeat;
        }
        none
    }

    fn detect_static(
        &self,
        frames: &[FrameSignature],
        capped: bool,
    ) -> Option<LoopDetection> {
        let anchor_count = frames.len().min(16);
        let mut spread = 0.0;
        let mut adjacent = 0.0;
        let mut motion = 0.0;
        for (index, frame) in frames.iter().enumerate() {
            let anchor = index * anchor_count / frames.len();
            spread += appearance_distance(frame, &frames[anchor]);
            motion += frame.motion.speed();
            if index > 0 {
                adjacent += appearance_distance(&frames[index - 1], frame);
            }
        }
        spread /= frames.len() as f32;
        adjacent /= (frames.len() - 1) as f32;
        motion /= frames.len() as f32;
        // Also compare to the first frame: the strided anchors above prevent a
        // tiny sensor shimmer from dominating, while this catches slow drift.
        let first_spread = frames
            .iter()
            .map(|frame| appearance_distance(frame, &frames[0]))
            .sum::<f32>()
            / frames.len() as f32;
        let stillness = spread.max(first_spread * 0.5) + adjacent * 0.5 + motion * 0.5;
        if stillness <= 0.024 {
            let confidence = (1.0 - stillness / 0.032).clamp(0.0, 1.0);
            Some(LoopDetection {
                kind: LoopKind::Static,
                window_start: 0,
                window_end: frames.len(),
                period_frames: 1,
                phase_frame: 0,
                confidence,
                support_cycles: frames.len() as f32,
                recurrence_score: confidence,
                seam_score: 1.0,
                motion_score: (1.0 - motion / 0.04).clamp(0.0, 1.0),
                internal_cut_penalty: 0.0,
                recommendation: CreativeLoopRecommendation::Hold,
                frames_considered: frames.len(),
                input_was_capped: capped,
            })
        } else {
            None
        }
    }

    fn best_recurrence_candidate(&self, frames: &[FrameSignature]) -> Option<Candidate> {
        let n = frames.len();
        let max_period = n / 2;
        if max_period < self.config.min_period_frames {
            return None;
        }
        let mut best: Option<Candidate> = None;
        let mut similarities = [0.0f32; MAX_ANALYSIS_FRAMES];

        for period in self.config.min_period_frames..=max_period {
            let recurrence_count = n - period;
            for i in 0..recurrence_count {
                let distance = recurrence_distance(&frames[i], &frames[i + period]);
                similarities[i] = (-distance / self.config.recurrence_tolerance).exp();
            }

            // Maximum positive recurrence run. Negative evidence at an intro,
            // outro or hard cut naturally trims the candidate to the cyclic body.
            let baseline = 0.56;
            let mut current_sum = 0.0f32;
            let mut current_start = 0usize;
            let mut run_start = 0usize;
            let mut run_end = 0usize;
            let mut run_sum = f32::NEG_INFINITY;
            for (i, &similarity) in similarities[..recurrence_count].iter().enumerate() {
                let value = similarity - baseline;
                if current_sum <= 0.0 {
                    current_sum = value;
                    current_start = i;
                } else {
                    current_sum += value;
                }
                let length = i + 1 - current_start;
                if length >= period
                    && (current_sum > run_sum
                        || ((current_sum - run_sum).abs() < 1e-6
                            && length > run_end.saturating_sub(run_start)))
                {
                    run_sum = current_sum;
                    run_start = current_start;
                    run_end = i + 1;
                }
            }
            if !run_sum.is_finite() || run_end <= run_start {
                continue;
            }

            let available_end = (run_end + period).min(n);
            let cycles = (available_end - run_start) / period;
            if cycles < 2 {
                continue;
            }
            let end = run_start + cycles * period;
            let candidate = score_candidate(frames, run_start, end, period);
            if candidate.cycles < 2.0 {
                continue;
            }

            let replace = best
                .map(|old| {
                    candidate.confidence > old.confidence + 0.025
                        || ((candidate.confidence - old.confidence).abs() <= 0.025
                            && (candidate.period < old.period
                                || (candidate.period == old.period
                                    && candidate.end - candidate.start > old.end - old.start)))
                })
                .unwrap_or(true);
            if replace {
                best = Some(candidate);
            }
        }
        best
    }
}

pub fn analyze_video_loop(frames: &[FrameSignature]) -> LoopDetection {
    LoopDetector::default().analyze(frames)
}

fn score_candidate(
    frames: &[FrameSignature],
    start: usize,
    end: usize,
    period: usize,
) -> Candidate {
    let mut recurrence_sum = 0.0;
    let mut recurrence_count = 0usize;
    for i in start..end - period {
        recurrence_sum += recurrence_distance(&frames[i], &frames[i + period]);
        recurrence_count += 1;
    }
    let recurrence_error = recurrence_sum / recurrence_count.max(1) as f32;
    let recurrence = (1.0 - recurrence_error / 0.14).clamp(0.0, 1.0);

    let mut adjacent_sum = 0.0;
    let mut adjacent_max = 0.0f32;
    for i in start + 1..end {
        let distance = appearance_distance(&frames[i - 1], &frames[i]);
        adjacent_sum += distance;
        adjacent_max = adjacent_max.max(distance);
    }
    let adjacent_mean = adjacent_sum / (end - start - 1).max(1) as f32;
    let seam_jump = appearance_distance(&frames[end - 1], &frames[start]);
    let reference_index = start + period - 1;
    let reference_next = (reference_index + 1).min(end - 1);
    let reference_jump = appearance_distance(&frames[reference_index], &frames[reference_next]);
    let seam_error = (seam_jump - reference_jump).abs();
    let seam = (-seam_error / (0.035 + adjacent_mean * 0.8)).exp();

    let cut_excess = (adjacent_max - (adjacent_mean * 3.2 + 0.10)).max(0.0);
    let cut_penalty = (cut_excess / 0.45).clamp(0.0, 1.0);

    let comparison = start + period - 1;
    let motion_error = motion_repeat_distance(frames[end - 1].motion, frames[comparison].motion);
    let motion = (1.0 - motion_error).clamp(0.0, 1.0);
    let cycles = (end - start) as f32 / period as f32;
    let support = (0.70 + 0.10 * cycles.min(3.0)).min(1.0);
    let confidence = ((0.57 * recurrence + 0.20 * seam + 0.13 * motion + 0.10)
        * (1.0 - 0.55 * cut_penalty)
        * support)
        .clamp(0.0, 1.0);
    Candidate {
        start,
        end,
        period,
        confidence,
        recurrence,
        seam,
        motion,
        cut_penalty,
        cycles,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PingPongScore {
    score: f32,
    appearance: f32,
    motion_opposition: f32,
    reversal: f32,
    axis: usize,
}

fn ping_pong_symmetry(
    frames: &[FrameSignature],
    start: usize,
    period: usize,
) -> PingPongScore {
    if period < 4 || start + period > frames.len() {
        return PingPongScore::default();
    }
    let mut mean_x = 0.0;
    let mut mean_y = 0.0;
    let mut mean_divergence = 0.0;
    let mut mean_speed = 0.0;
    for frame in &frames[start..start + period] {
        let activity = frame.motion.activity;
        mean_x += frame.motion.x * activity;
        mean_y += frame.motion.y * activity;
        mean_divergence += frame.motion.divergence * activity;
        mean_speed += frame.motion.speed();
    }
    mean_x /= period as f32;
    mean_y /= period as f32;
    mean_divergence /= period as f32;
    mean_speed /= period as f32;
    let mean_vector =
        (mean_x * mean_x + mean_y * mean_y + mean_divergence * mean_divergence).sqrt();
    let reversal = if mean_speed > 0.003 {
        (1.0 - mean_vector / mean_speed).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let mut best = PingPongScore {
        reversal,
        ..PingPongScore::default()
    };
    // Twice as many axes includes axes through frames and between frames.
    for doubled_axis in 0..period * 2 {
        let mut appearance_error = 0.0;
        let mut opposition_error = 0.0;
        for i in 0..period {
            let reflected = if doubled_axis % 2 == 0 {
                let axis = doubled_axis / 2;
                (2 * axis + period - i % period) % period
            } else {
                let axis = doubled_axis / 2;
                (2 * axis + 1 + period - i % period) % period
            };
            appearance_error += appearance_distance(&frames[start + i], &frames[start + reflected]);
            opposition_error += motion_opposite_distance(
                frames[start + i].motion,
                frames[start + reflected].motion,
            );
        }
        appearance_error /= period as f32;
        opposition_error /= period as f32;
        let appearance = (1.0 - appearance_error / 0.14).clamp(0.0, 1.0);
        let opposition = (1.0 - opposition_error).clamp(0.0, 1.0);
        let score = (0.62 * appearance + 0.38 * opposition) * reversal;
        if score > best.score {
            best = PingPongScore {
                score,
                appearance,
                motion_opposition: opposition,
                reversal,
                axis: doubled_axis / 2,
            };
        }
    }
    best
}

fn best_cut_phase(frames: &[FrameSignature], start: usize, period: usize) -> usize {
    if period <= 1 || start + period > frames.len() {
        return 0;
    }
    let mut best_phase = 0;
    let mut best_score = f32::NEG_INFINITY;
    for phase in 0..period {
        let current = start + phase;
        let previous = start + (phase + period - 1) % period;
        let transition = appearance_distance(&frames[previous], &frames[current]);
        let motion = 1.0 - motion_repeat_distance(frames[previous].motion, frames[current].motion);
        // Favor subtle cuts, but retain directional motion where it exists.
        let score = -transition + 0.04 * motion;
        if score > best_score {
            best_score = score;
            best_phase = phase;
        }
    }
    best_phase
}

fn none_detection(frames_considered: usize, input_was_capped: bool) -> LoopDetection {
    LoopDetection {
        kind: LoopKind::None,
        frames_considered,
        input_was_capped,
        recommendation: CreativeLoopRecommendation::AvoidLoop,
        ..LoopDetection::default()
    }
}

fn appearance_distance(a: &FrameSignature, b: &FrameSignature) -> f32 {
    let mut weighted = 0.0;
    let mut weights = 0.0;
    if !a.luma_blocks.is_empty() && !b.luma_blocks.is_empty() {
        weighted += 0.55 * scalar_channel_distance(&a.luma_blocks, &b.luma_blocks);
        weights += 0.55;
    }
    if !a.chroma_blocks.is_empty() && !b.chroma_blocks.is_empty() {
        weighted += 0.25 * chroma_channel_distance(&a.chroma_blocks, &b.chroma_blocks);
        weights += 0.25;
    }
    if !a.edge_blocks.is_empty() && !b.edge_blocks.is_empty() {
        weighted += 0.20 * scalar_channel_distance(&a.edge_blocks, &b.edge_blocks);
        weights += 0.20;
    }
    if weights > 0.0 {
        (weighted / weights).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn scalar_channel_distance(a: &[f32], b: &[f32]) -> f32 {
    let count = a.len().min(b.len()).min(MAX_CHANNEL_VALUES);
    if count == 0 {
        return 1.0;
    }
    let mut sum = 0.0;
    for i in 0..count {
        sum += (finite_or_zero(a[i]) - finite_or_zero(b[i])).abs().min(1.0);
    }
    let length_penalty = a.len().abs_diff(b.len()).min(MAX_CHANNEL_VALUES) as f32
        / a.len().max(b.len()).min(MAX_CHANNEL_VALUES).max(1) as f32;
    (sum / count as f32 + 0.15 * length_penalty).clamp(0.0, 1.0)
}

fn chroma_channel_distance(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    let count = a.len().min(b.len()).min(MAX_CHANNEL_VALUES);
    if count == 0 {
        return 1.0;
    }
    let mut sum = 0.0;
    for i in 0..count {
        sum += 0.5
            * ((finite_or_zero(a[i][0]) - finite_or_zero(b[i][0]))
                .abs()
                .min(1.0)
                + (finite_or_zero(a[i][1]) - finite_or_zero(b[i][1]))
                    .abs()
                    .min(1.0));
    }
    let length_penalty = a.len().abs_diff(b.len()).min(MAX_CHANNEL_VALUES) as f32
        / a.len().max(b.len()).min(MAX_CHANNEL_VALUES).max(1) as f32;
    (sum / count as f32 + 0.15 * length_penalty).clamp(0.0, 1.0)
}

fn recurrence_distance(a: &FrameSignature, b: &FrameSignature) -> f32 {
    let appearance = appearance_distance(a, b);
    let motion_activity = a.motion.activity.max(b.motion.activity);
    if motion_activity > 0.01 {
        (0.84 * appearance + 0.16 * motion_repeat_distance(a.motion, b.motion)).clamp(0.0, 1.0)
    } else {
        appearance
    }
}

fn motion_repeat_distance(a: MotionSummary, b: MotionSummary) -> f32 {
    let vector = normalized_delta(a.x, b.x) * 0.38
        + normalized_delta(a.y, b.y) * 0.38
        + normalized_delta(a.divergence, b.divergence) * 0.14;
    let activity = (a.activity - b.activity).abs() * 0.10;
    (vector + activity).clamp(0.0, 1.0)
}

fn motion_opposite_distance(a: MotionSummary, b: MotionSummary) -> f32 {
    let vector = normalized_sum(a.x, b.x) * 0.38
        + normalized_sum(a.y, b.y) * 0.38
        + normalized_sum(a.divergence, b.divergence) * 0.14;
    let activity = (a.activity - b.activity).abs() * 0.10;
    (vector + activity).clamp(0.0, 1.0)
}

fn normalized_delta(a: f32, b: f32) -> f32 {
    (finite_or_zero(a) - finite_or_zero(b)).abs() / (0.05 + a.abs().max(b.abs()))
}

fn normalized_sum(a: f32, b: f32) -> f32 {
    (finite_or_zero(a) + finite_or_zero(b)).abs() / (0.05 + a.abs().max(b.abs()))
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn signature(value: f32, motion_x: f32) -> FrameSignature {
        FrameSignature::new(
            vec![value, value * 0.73 + 0.11, 1.0 - value * 0.6],
            vec![[value * 0.4, 1.0 - value * 0.3]; 2],
            vec![(value * TAU).sin().abs(), value],
            MotionSummary::new(motion_x, 0.0, 0.0, 1.0),
        )
    }

    fn sinusoidal(period: usize, cycles: usize, phase: usize, noise: f32) -> Vec<FrameSignature> {
        let mut seed = 0x91e1_0da5u32;
        (0..period * cycles)
            .map(|index| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let jitter = (((seed >> 8) as f32 / 16_777_216.0) - 0.5) * noise;
                let angle = (index + phase) as f32 * TAU / period as f32;
                signature((0.5 + 0.42 * angle.sin() + jitter).clamp(0.0, 1.0), 0.08)
            })
            .collect()
    }

    #[test]
    fn exact_and_noisy_sinusoidal_wraps_find_fundamental() {
        for noise in [0.0, 0.012] {
            let frames = sinusoidal(24, 5, 0, noise);
            let result = analyze_video_loop(&frames);
            assert_eq!(result.kind, LoopKind::Wrap, "{result:?}");
            assert_eq!(result.period_frames, 24, "{result:?}");
            assert!(result.confidence > 0.70, "{result:?}");
            assert!(result.support_cycles >= 4.0, "{result:?}");
        }
    }

    #[test]
    fn phase_shifted_wrap_retains_period() {
        let mut frames = vec![signature(0.93, -0.05), signature(0.12, 0.04)];
        frames.extend(sinusoidal(18, 4, 7, 0.006));
        frames.push(signature(0.04, -0.09));
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::Wrap, "{result:?}");
        assert_eq!(result.period_frames, 18, "{result:?}");
        assert!(result.window_start <= 4, "{result:?}");
        assert!(matches!(
            result.recommendation,
            CreativeLoopRecommendation::PhaseShift { .. }
                | CreativeLoopRecommendation::MotionMatchedCrossfade
        ));
    }

    #[test]
    fn fundamental_wins_over_two_x_alias() {
        let frames = sinusoidal(16, 8, 0, 0.0);
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::Wrap, "{result:?}");
        assert_eq!(result.period_frames, 16, "{result:?}");
    }

    #[test]
    fn translated_motion_sequence_is_a_wrap() {
        let period = 20;
        let mut frames = Vec::new();
        for index in 0..period * 4 {
            let phase = index % period;
            let mut blocks = vec![0.08; 8];
            let hot_block = (phase / 3) % blocks.len();
            blocks[hot_block] = 0.92;
            frames.push(FrameSignature::new(
                blocks.clone(),
                vec![[phase as f32 / period as f32, 0.3]; 8],
                blocks,
                MotionSummary::new(0.12, 0.0, 0.0, 0.9),
            ));
        }
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::Wrap, "{result:?}");
        assert_eq!(result.period_frames, period, "{result:?}");
        assert!(result.motion_score > 0.8, "{result:?}");
    }

    #[test]
    fn palindrome_with_reversing_motion_is_ping_pong() {
        let arm = 12;
        let mut cycle = Vec::new();
        for index in 0..arm {
            cycle.push(signature(index as f32 / arm as f32, 0.12));
        }
        for index in (0..arm).rev() {
            cycle.push(signature(index as f32 / arm as f32, -0.12));
        }
        let mut frames = Vec::new();
        for _ in 0..3 {
            frames.extend(cycle.iter().cloned());
        }
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::PingPong, "{result:?}");
        assert_eq!(result.period_frames, arm * 2, "{result:?}");
        assert!(result.motion_score > 0.60, "{result:?}");
    }

    #[test]
    fn hard_cut_random_sequence_is_not_a_loop() {
        let mut seed = 7u32;
        let frames: Vec<_> = (0..100)
            .map(|index| {
                seed = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
                let value = ((seed >> 8) as f32 / 16_777_216.0).fract();
                signature(value, if index % 3 == 0 { 0.3 } else { -0.17 })
            })
            .collect();
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::None, "{result:?}");
    }

    #[test]
    fn static_sequence_is_detected_without_period_hunting() {
        let frames = vec![signature(0.42, 0.0); 80];
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::Static, "{result:?}");
        assert_eq!(result.recommendation, CreativeLoopRecommendation::Hold);
    }

    #[test]
    fn intro_and_outro_are_trimmed_to_periodic_body() {
        let mut frames = Vec::new();
        for i in 0..11 {
            frames.push(signature(i as f32 / 13.0, -0.2 + i as f32 * 0.01));
        }
        let body_start = frames.len();
        frames.extend(sinusoidal(22, 4, 3, 0.003));
        let body_end = frames.len();
        for i in 0..9 {
            frames.push(signature(0.97 - i as f32 / 11.0, 0.25));
        }
        let result = analyze_video_loop(&frames);
        assert_eq!(result.kind, LoopKind::Wrap, "{result:?}");
        assert_eq!(result.period_frames, 22, "{result:?}");
        assert!(result.window_start >= body_start - 2, "{result:?}");
        assert!(result.window_end <= body_end + 2, "{result:?}");
    }

    #[test]
    fn frame_and_channel_caps_are_deterministic() {
        let mut frames = sinusoidal(32, 16, 0, 0.004);
        for (index, frame) in frames.iter_mut().enumerate() {
            frame.luma_blocks.extend((0..400).map(|i| ((i + index) % 17) as f32 / 17.0));
        }
        let first = analyze_video_loop(&frames);
        let second = analyze_video_loop(&frames);
        assert_eq!(first, second);
        assert_eq!(first.frames_considered, MAX_ANALYSIS_FRAMES);
        assert!(first.input_was_capped);
    }
}
