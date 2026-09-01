//! Exact integration of constant and score-linear tempo segments.

use std::fmt;

/// A tempo change at a quarter-note position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoPoint {
    pub quarter: f64,
    pub bpm: f64,
    /// If true, BPM changes linearly in score time until the next point.
    pub ramp_to_next: bool,
}

impl TempoPoint {
    pub const fn constant(quarter: f64, bpm: f64) -> Self {
        Self {
            quarter,
            bpm,
            ramp_to_next: false,
        }
    }

    pub const fn ramp(quarter: f64, bpm: f64) -> Self {
        Self {
            quarter,
            bpm,
            ramp_to_next: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TempoError {
    ZeroSampleRate,
    Empty,
    FirstPointNotZero,
    InvalidPoint,
    NotStrictlyIncreasing,
}

impl fmt::Display for TempoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid tempo map: {self:?}")
    }
}

impl std::error::Error for TempoError {}

/// Tempo transform between score quarter notes and nominal timeline samples.
///
/// A ramp integrates `60 / BPM(q)` analytically. Conversion rounds only the final
/// sample coordinate, so callback size cannot affect event placement.
#[derive(Clone, Debug, PartialEq)]
pub struct TempoMap {
    sample_rate: u32,
    points: Vec<TempoPoint>,
    start_seconds: Vec<f64>,
}

impl TempoMap {
    pub fn new(sample_rate: u32, points: Vec<TempoPoint>) -> Result<Self, TempoError> {
        if sample_rate == 0 {
            return Err(TempoError::ZeroSampleRate);
        }
        if points.is_empty() {
            return Err(TempoError::Empty);
        }
        if points[0].quarter.abs() > 1.0e-12 {
            return Err(TempoError::FirstPointNotZero);
        }
        for (index, point) in points.iter().enumerate() {
            if !point.quarter.is_finite() || !point.bpm.is_finite() || point.bpm <= 0.0 {
                return Err(TempoError::InvalidPoint);
            }
            if index > 0 && point.quarter <= points[index - 1].quarter {
                return Err(TempoError::NotStrictlyIncreasing);
            }
        }

        let mut start_seconds = Vec::with_capacity(points.len());
        start_seconds.push(0.0);
        for index in 0..points.len().saturating_sub(1) {
            let dq = points[index + 1].quarter - points[index].quarter;
            let seconds = segment_seconds(points[index], points[index + 1].bpm, dq, dq);
            start_seconds.push(start_seconds[index] + seconds);
        }
        Ok(Self {
            sample_rate,
            points,
            start_seconds,
        })
    }

    pub fn constant(sample_rate: u32, bpm: f64) -> Result<Self, TempoError> {
        Self::new(sample_rate, vec![TempoPoint::constant(0.0, bpm)])
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }

    pub fn initial_bpm(&self) -> f64 {
        self.points[0].bpm
    }

    pub fn bpm_at_quarter(&self, quarter: f64) -> f64 {
        if quarter <= 0.0 {
            return self.points[0].bpm;
        }
        let index = self.segment_for_quarter(quarter);
        let point = self.points[index];
        if point.ramp_to_next && index + 1 < self.points.len() {
            let next = self.points[index + 1];
            let fraction = (quarter - point.quarter) / (next.quarter - point.quarter);
            point.bpm + (next.bpm - point.bpm) * fraction.clamp(0.0, 1.0)
        } else {
            point.bpm
        }
    }

    pub fn quarter_to_seconds(&self, quarter: f64) -> f64 {
        if quarter <= 0.0 {
            return quarter * 60.0 / self.points[0].bpm;
        }
        let index = self.segment_for_quarter(quarter);
        let point = self.points[index];
        let dq = quarter - point.quarter;
        let (end_bpm, span) = self.points.get(index + 1).map_or(
            (point.bpm, dq),
            |next| (next.bpm, next.quarter - point.quarter),
        );
        self.start_seconds[index] + segment_seconds(point, end_bpm, span, dq)
    }

    pub fn seconds_to_quarter(&self, seconds: f64) -> f64 {
        if seconds <= 0.0 {
            return seconds * self.points[0].bpm / 60.0;
        }
        let mut index = self.start_seconds.len() - 1;
        for candidate in 0..self.start_seconds.len().saturating_sub(1) {
            if seconds < self.start_seconds[candidate + 1] {
                index = candidate;
                break;
            }
        }
        let point = self.points[index];
        let elapsed = seconds - self.start_seconds[index];
        if point.ramp_to_next && index + 1 < self.points.len() {
            let next = self.points[index + 1];
            let span = next.quarter - point.quarter;
            let slope = (next.bpm - point.bpm) / span;
            if slope.abs() > 1.0e-12 {
                let bpm = point.bpm * (slope * elapsed / 60.0).exp();
                return point.quarter + (bpm - point.bpm) / slope;
            }
        }
        point.quarter + elapsed * point.bpm / 60.0
    }

    pub fn quarter_to_sample(&self, quarter: f64) -> u64 {
        let frames = self.quarter_to_seconds(quarter) * f64::from(self.sample_rate);
        if !frames.is_finite() || frames <= 0.0 {
            0
        } else if frames >= u64::MAX as f64 {
            u64::MAX
        } else {
            frames.round() as u64
        }
    }

    pub fn sample_to_quarter(&self, sample: u64) -> f64 {
        self.seconds_to_quarter(sample as f64 / f64::from(self.sample_rate))
    }

    fn segment_for_quarter(&self, quarter: f64) -> usize {
        let mut index = self.points.len() - 1;
        for candidate in 0..self.points.len().saturating_sub(1) {
            if quarter < self.points[candidate + 1].quarter {
                index = candidate;
                break;
            }
        }
        index
    }
}

fn segment_seconds(
    start: TempoPoint,
    end_bpm: f64,
    segment_quarters: f64,
    delta_quarter: f64,
) -> f64 {
    if !start.ramp_to_next || (end_bpm - start.bpm).abs() < 1.0e-12 {
        return 60.0 * delta_quarter / start.bpm;
    }
    let slope = (end_bpm - start.bpm) / segment_quarters;
    let bpm = start.bpm + slope * delta_quarter;
    60.0 * (bpm / start.bpm).ln() / slope
}
