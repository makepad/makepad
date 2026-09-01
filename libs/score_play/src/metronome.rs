//! Sample-clock metronome patterns, count-in, tap tempo, and beat-light state.

use crate::clock::AudioClockSnapshot;
use crate::event::{ClickLevel, EventSource, ScheduledEvent, SynthEvent, SynthEventKind};
use crate::scheduler::PerformancePlan;
use crate::tempo::TempoMap;
use std::fmt;

const MAX_GROUPS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeterError {
    InvalidSignature,
    TooManyGroups,
    GroupsDoNotFillBar,
}

impl fmt::Display for MeterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid metronome meter: {self:?}")
    }
}

impl std::error::Error for MeterError {}

/// A time signature plus explicit additive beat groups in denominator units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    numerator: u8,
    denominator: u8,
    groups: [u8; MAX_GROUPS],
    group_count: u8,
}

impl Meter {
    /// Empty `groups` chooses groups of three for compound metres above 3, and
    /// one-unit beats otherwise. Irregular metre callers should pass an explicit spelling.
    pub fn new(numerator: u8, denominator: u8, groups: &[u8]) -> Result<Self, MeterError> {
        if numerator == 0 || denominator == 0 || !denominator.is_power_of_two() {
            return Err(MeterError::InvalidSignature);
        }
        let mut result = Self {
            numerator,
            denominator,
            groups: [0; MAX_GROUPS],
            group_count: 0,
        };
        if groups.is_empty() {
            if numerator > 3 && numerator % 3 == 0 {
                let count = numerator / 3;
                for index in 0..usize::from(count) {
                    result.groups[index] = 3;
                }
                result.group_count = count;
            } else {
                if usize::from(numerator) > MAX_GROUPS {
                    return Err(MeterError::TooManyGroups);
                }
                for index in 0..usize::from(numerator) {
                    result.groups[index] = 1;
                }
                result.group_count = numerator;
            }
        } else {
            if groups.len() > MAX_GROUPS {
                return Err(MeterError::TooManyGroups);
            }
            if groups.iter().any(|group| *group == 0)
                || groups.iter().map(|group| u16::from(*group)).sum::<u16>()
                    != u16::from(numerator)
            {
                return Err(MeterError::GroupsDoNotFillBar);
            }
            result.groups[..groups.len()].copy_from_slice(groups);
            result.group_count = groups.len() as u8;
        }
        Ok(result)
    }

    pub fn numerator(self) -> u8 {
        self.numerator
    }

    pub fn denominator(self) -> u8 {
        self.denominator
    }

    pub fn group_slice(&self) -> &[u8] {
        &self.groups[..usize::from(self.group_count)]
    }

    pub fn bar_quarters(self) -> f64 {
        f64::from(self.numerator) * 4.0 / f64::from(self.denominator)
    }

    pub fn unit_quarters(self) -> f64 {
        4.0 / f64::from(self.denominator)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetronomeConfig {
    /// Zero emits only primary group pulses. One adds denominator units; larger values
    /// divide every denominator unit evenly.
    pub subdivisions_per_unit: u8,
}

impl Default for MetronomeConfig {
    fn default() -> Self {
        Self {
            subdivisions_per_unit: 0,
        }
    }
}

pub struct Metronome;

impl Metronome {
    /// Schedules a score-time bar span against the same tempo transform as playback.
    pub fn schedule(
        tempo: &TempoMap,
        score_origin_sample: u64,
        start_quarter: f64,
        bars: u32,
        meter: Meter,
        config: MetronomeConfig,
        first_sequence: u32,
    ) -> Vec<ScheduledEvent> {
        let mut result = Vec::new();
        let mut sequence = first_sequence;
        for bar in 0..bars {
            let bar_start = start_quarter + f64::from(bar) * meter.bar_quarters();
            schedule_bar_quarters(
                &mut result,
                &mut sequence,
                bar_start,
                meter,
                config.subdivisions_per_unit,
                |quarter| score_origin_sample.saturating_add(tempo.quarter_to_sample(quarter)),
            );
        }
        result
    }

    /// Builds count-in clicks before score sample zero and returns its timeline duration.
    pub(crate) fn count_in(
        sample_rate: u32,
        bpm: f64,
        meter: Meter,
        bars: u8,
        subdivisions_per_unit: u8,
    ) -> Option<(u64, Vec<ScheduledEvent>)> {
        if sample_rate == 0 || !bpm.is_finite() || bpm <= 0.0 {
            return None;
        }
        let total_quarters = meter.bar_quarters() * f64::from(bars);
        let duration_f64 = total_quarters * 60.0 / bpm * f64::from(sample_rate);
        if !duration_f64.is_finite() || duration_f64 < 0.0 || duration_f64 > u64::MAX as f64 {
            return None;
        }
        let duration = duration_f64.round() as u64;
        let mut events = Vec::new();
        let mut sequence = 0;
        for bar in 0..bars {
            let bar_start = f64::from(bar) * meter.bar_quarters();
            schedule_bar_quarters(
                &mut events,
                &mut sequence,
                bar_start,
                meter,
                subdivisions_per_unit,
                |quarter| {
                    (quarter * 60.0 / bpm * f64::from(sample_rate)).round() as u64
                },
            );
        }
        Some((duration, events))
    }
}

fn schedule_bar_quarters<F: Fn(f64) -> u64>(
    result: &mut Vec<ScheduledEvent>,
    sequence: &mut u32,
    bar_start: f64,
    meter: Meter,
    subdivisions_per_unit: u8,
    to_sample: F,
) {
    let unit = meter.unit_quarters();
    let fine_division = subdivisions_per_unit.max(1);
    let mut group_starts = [false; 256];
    let mut unit_cursor = 0usize;
    group_starts[0] = true;
    for group in meter.group_slice() {
        unit_cursor += usize::from(*group);
        if unit_cursor < group_starts.len() {
            group_starts[unit_cursor] = true;
        }
    }
    let total_ticks = usize::from(meter.numerator) * usize::from(fine_division);
    for tick in 0..total_ticks {
        let unit_index = tick / usize::from(fine_division);
        let on_unit = tick % usize::from(fine_division) == 0;
        let primary = on_unit && group_starts[unit_index];
        if !primary && subdivisions_per_unit == 0 {
            continue;
        }
        let quarter = bar_start + tick as f64 * unit / f64::from(fine_division);
        let level = if tick == 0 {
            ClickLevel::Bar
        } else if primary {
            ClickLevel::Beat
        } else {
            ClickLevel::Subdivision
        };
        result.push(ScheduledEvent {
            at: to_sample(quarter),
            sequence: *sequence,
            event: SynthEvent {
                source: EventSource::Metronome,
                kind: SynthEventKind::Click { level },
            },
        });
        *sequence = sequence.wrapping_add(1);
    }
}

/// Visual classification at the position estimated from an audio-clock snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BeatKind {
    Bar,
    Group,
    Subdivision,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatIndicator {
    pub bar_index: i64,
    pub group_index: u8,
    pub phase: f64,
    pub kind: BeatKind,
}

impl BeatIndicator {
    /// Reads the same snapshot estimator intended for the cursor and falling notes.
    pub fn from_clock(
        snapshot: AudioClockSnapshot,
        plan: &PerformancePlan,
        host_time_ns: u64,
        meter: Meter,
    ) -> Self {
        let display = snapshot.estimate(plan, host_time_ns);
        Self::from_quarter(display.score_quarter, meter)
    }

    pub fn from_quarter(quarter: f64, meter: Meter) -> Self {
        let bar_length = meter.bar_quarters();
        let bar_index = (quarter / bar_length).floor() as i64;
        let within = quarter - bar_index as f64 * bar_length;
        let unit_position = within / meter.unit_quarters();
        let unit_index = unit_position.floor().max(0.0) as usize;
        let mut cursor = 0usize;
        let mut group_index = 0u8;
        let mut group_start = 0usize;
        let mut group_len = 1usize;
        for (index, group) in meter.group_slice().iter().enumerate() {
            let end = cursor + usize::from(*group);
            if unit_index < end {
                group_index = index as u8;
                group_start = cursor;
                group_len = usize::from(*group);
                break;
            }
            cursor = end;
        }
        let group_quarters = group_len as f64 * meter.unit_quarters();
        let group_local = within - group_start as f64 * meter.unit_quarters();
        let phase = (group_local / group_quarters).clamp(0.0, 1.0);
        let kind = if within.abs() < 1.0e-9 {
            BeatKind::Bar
        } else if group_local.abs() < 1.0e-9 {
            BeatKind::Group
        } else {
            BeatKind::Subdivision
        };
        Self {
            bar_index,
            group_index,
            phase,
            kind,
        }
    }
}

/// Median-based tap tempo with bounded storage, outlier rejection, and pause reset.
pub struct TapTempo {
    taps: [u64; 9],
    len: usize,
    smoothed_bpm: Option<f64>,
}

impl Default for TapTempo {
    fn default() -> Self {
        Self::new()
    }
}

impl TapTempo {
    pub const fn new() -> Self {
        Self {
            taps: [0; 9],
            len: 0,
            smoothed_bpm: None,
        }
    }

    pub fn tap(&mut self, host_time_ns: u64) -> Option<f64> {
        if self.len > 0 {
            let last = self.taps[self.len - 1];
            if host_time_ns <= last || host_time_ns - last > 2_500_000_000 {
                self.len = 0;
                self.smoothed_bpm = None;
            }
        }
        if self.len == self.taps.len() {
            self.taps.copy_within(1.., 0);
            self.len -= 1;
        }
        self.taps[self.len] = host_time_ns;
        self.len += 1;
        let raw = self.median_bpm()?;
        let smoothed = self.smoothed_bpm.map_or(raw, |previous| previous * 0.65 + raw * 0.35);
        self.smoothed_bpm = Some(smoothed);
        Some(smoothed)
    }

    pub fn bpm(&self) -> Option<f64> {
        self.smoothed_bpm
    }

    fn median_bpm(&self) -> Option<f64> {
        if self.len < 2 {
            return None;
        }
        let mut intervals = [0u64; 8];
        let mut count = 0;
        for index in 1..self.len {
            let interval = self.taps[index] - self.taps[index - 1];
            if (200_000_000..=2_000_000_000).contains(&interval) {
                intervals[count] = interval;
                count += 1;
            }
        }
        if count == 0 {
            return None;
        }
        intervals[..count].sort_unstable();
        let median = intervals[(count - 1) / 2];
        let mut deviations = [0u64; 8];
        for index in 0..count {
            deviations[index] = intervals[index].abs_diff(median);
        }
        deviations[..count].sort_unstable();
        let mad = deviations[(count - 1) / 2];
        let threshold = mad.saturating_mul(3).max(25_000_000);
        let mut filtered = [0u64; 8];
        let mut filtered_count = 0;
        for interval in intervals[..count].iter().copied() {
            if interval.abs_diff(median) <= threshold {
                filtered[filtered_count] = interval;
                filtered_count += 1;
            }
        }
        filtered[..filtered_count].sort_unstable();
        let robust = filtered[(filtered_count - 1) / 2];
        Some(60_000_000_000.0 / robust as f64)
    }
}
