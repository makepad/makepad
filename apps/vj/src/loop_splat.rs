//! Beat-quantized loop-splat slicing.
//!
//! The slicer first enumerates only complete 4/4 bars from the analysed
//! downbeat grid. Arrangement changes are snapped to those bars, nudged to
//! nearby four-bar phrase boundaries, and reduced to seven cuts by greedy
//! farthest-point selection. Missing cuts are filled by repeatedly splitting
//! the longest eligible section at a four-bar boundary. Each stem then picks
//! the densest steady power-of-two loop in its section; the mix uses the
//! section head. All results stay in source seconds so callers can precompute
//! the exact frame-domain representation for their own source rate.

use crate::decks::LoopSpan;
use crate::music_dsp::{StemKind, STEM_COUNT};
use crate::wave_analysis::{TrackAnalysis, ZOOM_COLS_PER_SEC};

pub const SPLAT_COLS: usize = 8;
pub const SPLAT_ROWS: usize = 5;
/// `TrackGrid::confidence` measures how many rulings sit on a SHARP onset,
/// not whether the grid is right: a piano-and-strings record with a
/// 0.996-F grid against the reference scores 0.28 because its onsets are
/// soft. So this only keeps out the no-rhythm case (silence and noise score
/// near zero); the per-cell energy gate decides what is actually loopable.
const MIN_GRID_CONFIDENCE: f32 = 0.12;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SplatPart {
    pub num: u8,
    pub den: u8,
}

impl SplatPart {
    pub const WHOLE: Self = Self { num: 0, den: 1 };

    pub const fn is_valid(self) -> bool {
        matches!(self.den, 1 | 2 | 4) && self.num < self.den
    }
}

impl Default for SplatPart {
    fn default() -> Self {
        Self::WHOLE
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
pub enum SplatRow {
    Drums = 0,
    Bass = 1,
    Vocals = 2,
    Other = 3,
    Mix = 4,
}

impl SplatRow {
    pub const ALL: [SplatRow; SPLAT_ROWS] = [
        SplatRow::Drums,
        SplatRow::Bass,
        SplatRow::Vocals,
        SplatRow::Other,
        SplatRow::Mix,
    ];

    pub fn stem(self) -> Option<StemKind> {
        match self {
            SplatRow::Drums => Some(StemKind::Drums),
            SplatRow::Bass => Some(StemKind::Bass),
            SplatRow::Vocals => Some(StemKind::Vocals),
            SplatRow::Other => Some(StemKind::Other),
            SplatRow::Mix => None,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplatCell {
    /// Source seconds, aligned to whole bars.
    pub span: LoopSpan,
    pub bars: u8,
    pub energy: f32,
    /// A stem with negligible activity in this section is not launchable.
    pub silent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplatSection {
    pub start_secs: f64,
    pub end_secs: f64,
    pub bars: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplatGrid {
    pub bpm: f64,
    pub bar_secs: f64,
    pub first_bar_secs: f64,
    pub sections: Vec<SplatSection>,
    pub cells: [[Option<SplatCell>; SPLAT_COLS]; SPLAT_ROWS],
    pub bars_per_col: [u8; SPLAT_COLS],
}

const CACHE_MAGIC: &[u8; 8] = b"VJSPLAT\0";
const CACHE_VERSION: u32 = 1;

/// Encode the derived grid as a bounded, versioned side-channel. This is the
/// complete load-time loop-slicing result; live playback state is not part of
/// it.
pub fn encode_splat(grid: &SplatGrid) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + grid.sections.len() * 20 + SPLAT_ROWS * SPLAT_COLS * 24);
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&grid.bpm.to_le_bytes());
    out.extend_from_slice(&grid.bar_secs.to_le_bytes());
    out.extend_from_slice(&grid.first_bar_secs.to_le_bytes());
    out.push(grid.sections.len().min(SPLAT_COLS) as u8);
    for section in grid.sections.iter().take(SPLAT_COLS) {
        out.extend_from_slice(&section.start_secs.to_le_bytes());
        out.extend_from_slice(&section.end_secs.to_le_bytes());
        out.extend_from_slice(&section.bars.to_le_bytes());
    }
    out.extend_from_slice(&grid.bars_per_col);
    for row in &grid.cells {
        for cell in row {
            match cell {
                None => out.push(0),
                Some(cell) => {
                    out.push(1);
                    out.extend_from_slice(&cell.span.start_secs.to_le_bytes());
                    out.extend_from_slice(&cell.span.end_secs.to_le_bytes());
                    out.push(cell.bars);
                    out.extend_from_slice(&cell.energy.to_le_bytes());
                    out.push(u8::from(cell.silent));
                }
            }
        }
    }
    out
}

/// Decode a loop-splat side-channel. Invalid, oversized and newer payloads
/// are refused so a web deck can settle to typed `Unavailable` immediately.
pub fn decode_splat(bytes: &[u8]) -> Result<SplatGrid, String> {
    let mut at = 0usize;
    let mut take = |count: usize| -> Result<&[u8], String> {
        let end = at.checked_add(count).ok_or("loop-splat length overflow")?;
        if end > bytes.len() {
            return Err("loop-splat cache truncated".into());
        }
        let value = &bytes[at..end];
        at = end;
        Ok(value)
    };
    if take(8)? != CACHE_MAGIC {
        return Err("not a loop-splat cache file".into());
    }
    let version = u32::from_le_bytes(take(4)?.try_into().unwrap());
    if version != CACHE_VERSION {
        return Err(format!("loop-splat cache version {version}"));
    }
    let bpm = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let bar_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    let first_bar_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
    if !bpm.is_finite() || !bar_secs.is_finite() || !first_bar_secs.is_finite() || bar_secs <= 0.0 {
        return Err("loop-splat timing is invalid".into());
    }
    let section_count = take(1)?[0] as usize;
    if section_count > SPLAT_COLS {
        return Err("loop-splat section count out of range".into());
    }
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        let start_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
        let end_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
        let bars = u32::from_le_bytes(take(4)?.try_into().unwrap());
        if !start_secs.is_finite() || !end_secs.is_finite() || end_secs <= start_secs || bars == 0 {
            return Err("loop-splat section is invalid".into());
        }
        sections.push(SplatSection { start_secs, end_secs, bars });
    }
    let bars_per_col: [u8; SPLAT_COLS] = take(SPLAT_COLS)?.try_into().unwrap();
    let mut cells = [[None; SPLAT_COLS]; SPLAT_ROWS];
    for row in &mut cells {
        for cell in row {
            let present = take(1)?[0];
            if present == 0 {
                continue;
            }
            if present != 1 {
                return Err("loop-splat cell flag out of range".into());
            }
            let start_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
            let end_secs = f64::from_le_bytes(take(8)?.try_into().unwrap());
            let bars = take(1)?[0];
            let energy = f32::from_le_bytes(take(4)?.try_into().unwrap());
            let silent = match take(1)?[0] {
                0 => false,
                1 => true,
                _ => return Err("loop-splat silent flag out of range".into()),
            };
            if !start_secs.is_finite()
                || !end_secs.is_finite()
                || end_secs <= start_secs
                || bars == 0
                || !energy.is_finite()
            {
                return Err("loop-splat cell is invalid".into());
            }
            *cell = Some(SplatCell {
                span: LoopSpan { start_secs, end_secs },
                bars,
                energy,
                silent,
            });
        }
    }
    if at != bytes.len() {
        return Err("loop-splat cache has trailing bytes".into());
    }
    Ok(SplatGrid { bpm, bar_secs, first_bar_secs, sections, cells, bars_per_col })
}

/// Per-beat mean level per stem. Beat `i` begins at
/// `first_beat_secs + i * beat_secs`.
#[derive(Clone, Debug, Default)]
pub struct StemLevels {
    pub beat_secs: f64,
    pub first_beat_secs: f64,
    pub levels: [Vec<f32>; STEM_COUNT],
}

impl StemLevels {
    /// Build per-beat stereo RMS without depending on the mixer's stem table.
    /// The callback returns an unscaled floating-point frame for one lane;
    /// `None` represents a missing frame and contributes silence.
    pub fn from_stems<F>(
        beat_secs: f64,
        first_beat_secs: f64,
        sample_rate: u32,
        frame_count: usize,
        mut frame: F,
    ) -> Self
    where
        F: FnMut(StemKind, usize) -> Option<[f32; 2]>,
    {
        let rate = sample_rate.max(1) as f64;
        if !beat_secs.is_finite() || beat_secs <= 0.0 || frame_count == 0 {
            return Self { beat_secs, first_beat_secs, ..Self::default() };
        }
        let duration = frame_count as f64 / rate;
        let beat_count = ((duration - first_beat_secs).max(0.0) / beat_secs).ceil() as usize;
        let mut levels: [Vec<f32>; STEM_COUNT] = std::array::from_fn(|_| {
            Vec::with_capacity(beat_count)
        });
        for stem in StemKind::ALL {
            for beat in 0..beat_count {
                let start_secs = first_beat_secs + beat as f64 * beat_secs;
                let end_secs = (start_secs + beat_secs).min(duration);
                let start = (start_secs.max(0.0) * rate).floor() as usize;
                let end = (end_secs.max(0.0) * rate).ceil() as usize;
                let mut sum = 0.0f64;
                let count = end.saturating_sub(start);
                for index in start..end.min(frame_count) {
                    if let Some(value) = frame(stem, index) {
                        sum += (value[0] as f64 * value[0] as f64
                            + value[1] as f64 * value[1] as f64)
                            * 0.5;
                    }
                }
                levels[stem.index()].push(if count == 0 {
                    0.0
                } else {
                    (sum / count as f64).sqrt() as f32
                });
            }
        }
        Self { beat_secs, first_beat_secs, levels }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SplatSnapshot {
    pub active: bool,
    pub playing: [Option<(u8, SplatPart)>; SPLAT_ROWS],
    pub queued: [Option<(u8, SplatPart)>; SPLAT_ROWS],
    pub bar_index: i64,
    pub bar_phase: f32,
    pub row_phase: [f32; SPLAT_ROWS],
}

/// Build a deterministic section grid. A weak beat grid, invalid timing, or
/// fewer than two complete bars cannot produce a splat.
pub fn build_splat(
    analysis: &TrackAnalysis,
    stem_levels: Option<&StemLevels>,
) -> Option<SplatGrid> {
    let track_grid = analysis.grid;
    if !track_grid.has_grid()
        || track_grid.confidence < MIN_GRID_CONFIDENCE
        || !analysis.duration_secs.is_finite()
        || analysis.duration_secs <= 0.0
    {
        return None;
    }
    let bar_secs = track_grid.beat_secs * 4.0;
    if !bar_secs.is_finite() || bar_secs <= 0.0 {
        return None;
    }
    let bar_zero = track_grid.first_beat_secs
        - track_grid.downbeat_phase as f64 * track_grid.beat_secs;
    let first_index = ((-bar_zero) / bar_secs - 1e-10).ceil();
    let first_bar_secs = bar_zero + first_index * bar_secs;
    let whole_bars = ((analysis.duration_secs - first_bar_secs + 1e-10) / bar_secs)
        .floor()
        .max(0.0) as u32;
    if whole_bars < 2 {
        return None;
    }

    let mut boundaries: Vec<u32> = analysis
        .changes_secs
        .iter()
        .copied()
        .filter(|secs| secs.is_finite())
        .map(|secs| ((secs - first_bar_secs) / bar_secs).round() as i64)
        .filter_map(|bar| {
            if bar < 0 {
                return None;
            }
            let bar = bar as u32;
            let phrase = ((bar as f64 / 4.0).round() as i64 * 4).max(0) as u32;
            let bar = if bar.abs_diff(phrase) <= 1 { phrase } else { bar };
            (bar >= 2 && whole_bars.saturating_sub(bar) >= 2).then_some(bar)
        })
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    if boundaries.len() > SPLAT_COLS - 1 {
        boundaries = farthest_boundaries(&boundaries, SPLAT_COLS - 1);
    }
    while boundaries.len() < SPLAT_COLS - 1 {
        let mut edges = Vec::with_capacity(boundaries.len() + 2);
        edges.push(0);
        edges.extend(boundaries.iter().copied());
        edges.push(whole_bars);
        let Some((start, end)) = edges
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .filter(|(start, end)| end - start >= 8)
            .max_by_key(|(start, end)| (end - start, std::cmp::Reverse(*start)))
        else {
            break;
        };
        let midpoint = (start + end) as f64 * 0.5;
        let mut split = ((midpoint / 4.0).round() as u32) * 4;
        split = split.clamp(start + 4, end - 4);
        if boundaries.binary_search(&split).is_ok() {
            break;
        }
        boundaries.push(split);
        boundaries.sort_unstable();
    }

    let mut edges = Vec::with_capacity(boundaries.len() + 2);
    edges.push(0);
    edges.extend(boundaries);
    edges.push(whole_bars);
    let sections: Vec<SplatSection> = edges
        .windows(2)
        .map(|pair| SplatSection {
            start_secs: first_bar_secs + pair[0] as f64 * bar_secs,
            end_secs: first_bar_secs + pair[1] as f64 * bar_secs,
            bars: pair[1] - pair[0],
        })
        .collect();
    let mut grid = SplatGrid {
        bpm: track_grid.bpm,
        bar_secs,
        first_bar_secs,
        sections,
        cells: [[None; SPLAT_COLS]; SPLAT_ROWS],
        bars_per_col: [0; SPLAT_COLS],
    };
    for col in 0..grid.sections.len() {
        let bars = largest_power_of_two(grid.sections[col].bars.min(4) as u8);
        rebuild_column(&mut grid, analysis, stem_levels, col, bars);
    }
    Some(grid)
}

fn largest_power_of_two(value: u8) -> u8 {
    if value == 0 {
        0
    } else {
        1 << (7 - value.leading_zeros() as u8)
    }
}

fn farthest_boundaries(candidates: &[u32], keep: usize) -> Vec<u32> {
    if candidates.len() <= keep {
        return candidates.to_vec();
    }
    let mut selected = vec![candidates[0], *candidates.last().unwrap()];
    while selected.len() < keep {
        let next = candidates
            .iter()
            .copied()
            .filter(|candidate| !selected.contains(candidate))
            .max_by_key(|candidate| {
                let spacing = selected
                    .iter()
                    .map(|picked| candidate.abs_diff(*picked))
                    .min()
                    .unwrap_or(0);
                (spacing, std::cmp::Reverse(*candidate))
            });
        let Some(next) = next else { break };
        selected.push(next);
    }
    selected.sort_unstable();
    selected
}

fn rebuild_column(
    grid: &mut SplatGrid,
    analysis: &TrackAnalysis,
    stem_levels: Option<&StemLevels>,
    col: usize,
    bars: u8,
) {
    let Some(section) = grid.sections.get(col).cloned() else { return };
    grid.bars_per_col[col] = bars;
    let len_secs = bars as f64 * grid.bar_secs;
    let mix_energy = mix_energy(analysis, section.start_secs, section.start_secs + len_secs);
    grid.cells[SplatRow::Mix.index()][col] = Some(SplatCell {
        span: LoopSpan {
            start_secs: section.start_secs,
            end_secs: section.start_secs + len_secs,
        },
        bars,
        energy: mix_energy,
        silent: false,
    });

    for row in SplatRow::ALL.into_iter().filter(|row| row.stem().is_some()) {
        let (start_secs, energy, silent) = match stem_levels {
            Some(levels) => choose_stem_window(levels, row.stem().unwrap(), &section, bars, grid.bar_secs),
            None => (section.start_secs, mix_energy, false),
        };
        grid.cells[row.index()][col] = Some(SplatCell {
            span: LoopSpan { start_secs, end_secs: start_secs + len_secs },
            bars,
            energy,
            silent,
        });
    }
}

fn mix_energy(analysis: &TrackAnalysis, start_secs: f64, end_secs: f64) -> f32 {
    if analysis.tiles.zoom.is_empty() {
        return 0.5;
    }
    let start = (start_secs * ZOOM_COLS_PER_SEC).floor().max(0.0) as usize;
    let end = (end_secs * ZOOM_COLS_PER_SEC).ceil().max(start as f64 + 1.0) as usize;
    let end = end.min(analysis.tiles.zoom.len());
    if start >= end {
        return 0.0;
    }
    analysis.tiles.zoom[start..end]
        .iter()
        .map(|column| column[3] as f32 / 255.0)
        .sum::<f32>()
        / (end - start) as f32
}

fn choose_stem_window(
    levels: &StemLevels,
    stem: StemKind,
    section: &SplatSection,
    bars: u8,
    bar_secs: f64,
) -> (f64, f32, bool) {
    let lane = &levels.levels[stem.index()];
    let p95 = percentile_95(lane);
    let whole = level_stats(levels, lane, section.start_secs, section.end_secs).0;
    let silent = p95 <= f32::EPSILON || whole < p95 * 0.08;
    let windows = section.bars.saturating_sub(bars as u32) + 1;
    let mut best = (f32::NEG_INFINITY, section.start_secs, 0.0f32);
    for offset in 0..windows {
        let start = section.start_secs + offset as f64 * bar_secs;
        let (mean, std_dev) = level_stats(levels, lane, start, start + bars as f64 * bar_secs);
        let score = mean - 0.5 * std_dev;
        if score > best.0 + 1e-7 {
            best = (score, start, mean);
        }
    }
    let energy = if p95 > f32::EPSILON {
        (best.2 / p95).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (best.1, energy, silent)
}

fn level_stats(levels: &StemLevels, lane: &[f32], start_secs: f64, end_secs: f64) -> (f32, f32) {
    if lane.is_empty() || levels.beat_secs <= 0.0 {
        return (0.0, 0.0);
    }
    let start = ((start_secs - levels.first_beat_secs) / levels.beat_secs)
        .round()
        .max(0.0) as usize;
    let end = ((end_secs - levels.first_beat_secs) / levels.beat_secs)
        .round()
        .max(start as f64 + 1.0) as usize;
    let end = end.min(lane.len());
    if start >= end {
        return (0.0, 0.0);
    }
    let slice = &lane[start..end];
    let mean = slice.iter().copied().sum::<f32>() / slice.len() as f32;
    let variance = slice
        .iter()
        .map(|value| {
            let delta = *value - mean;
            delta * delta
        })
        .sum::<f32>()
        / slice.len() as f32;
    (mean, variance.sqrt())
}

fn percentile_95(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let index = ((sorted.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave_analysis::{TempoMap, TrackGrid, WaveTiles};

    fn analysis(duration_secs: f64, changes_secs: Vec<f64>, confidence: f32) -> TrackAnalysis {
        TrackAnalysis {
            duration_secs,
            sample_rate: 48_000,
            grid: TrackGrid {
                bpm: 120.0,
                beat_secs: 0.5,
                first_beat_secs: 0.25,
                downbeat_phase: 0,
                confidence,
            },
            tempo_map: TempoMap::default(),
            tiles: WaveTiles {
                zoom: vec![[0, 0, 0, 128]; (duration_secs * ZOOM_COLS_PER_SEC) as usize],
                overview: Vec::new(),
            },
            changes_secs,
        }
    }

    #[test]
    fn sections_are_complete_contiguous_bar_spans() {
        let source = analysis(200.0, vec![17.9, 34.2, 65.0, 97.0, 130.0, 163.0], 0.9);
        let grid = build_splat(&source, None).unwrap();
        assert!(grid.sections.len() <= SPLAT_COLS);
        assert_eq!(grid.sections.first().unwrap().start_secs, grid.first_bar_secs);
        for pair in grid.sections.windows(2) {
            assert!((pair[0].end_secs - pair[1].start_secs).abs() < 1e-9);
        }
        for section in &grid.sections {
            let bars = (section.start_secs - grid.first_bar_secs) / grid.bar_secs;
            assert!((bars - bars.round()).abs() < 1e-9);
            assert!((section.end_secs - section.start_secs - section.bars as f64 * grid.bar_secs).abs() < 1e-9);
        }
        for col in 0..grid.sections.len() {
            let bars = grid.bars_per_col[col];
            assert!(bars.is_power_of_two() && bars <= 8);
            assert!(bars as u32 <= grid.sections[col].bars);
        }
    }

    #[test]
    fn dense_steady_window_and_silence_are_detected() {
        let source = analysis(128.25, vec![], 0.9);
        let beat_count = 256;
        let mut levels = StemLevels {
            beat_secs: 0.5,
            first_beat_secs: 0.25,
            levels: std::array::from_fn(|_| vec![0.1; beat_count]),
        };
        // First section is split to eight bars. Its four bars starting at
        // bar two are the unique dense, steady drums window.
        for beat in 8..24 {
            levels.levels[StemKind::Drums.index()][beat] = 0.8;
        }
        levels.levels[StemKind::Vocals.index()].fill(0.01);
        for beat in 80..100 {
            levels.levels[StemKind::Vocals.index()][beat] = 1.0;
        }
        let grid = build_splat(&source, Some(&levels)).unwrap();
        let drums = grid.cells[SplatRow::Drums.index()][0].unwrap();
        assert!((drums.span.start_secs - (grid.first_bar_secs + 2.0 * grid.bar_secs)).abs() < 1e-9);
        assert!(!drums.silent);
        let vocals = grid.cells[SplatRow::Vocals.index()][0].unwrap();
        assert!(vocals.silent);
    }

    #[test]
    fn degenerate_and_many_change_inputs_are_bounded() {
        assert!(build_splat(&analysis(200.0, vec![], 0.0), None).is_none());
        assert!(build_splat(&analysis(4.24, vec![], 0.9), None).is_none());
        let three_bars = build_splat(&analysis(6.25, vec![], 0.9), None).unwrap();
        assert_eq!(three_bars.sections.iter().map(|section| section.bars).sum::<u32>(), 3);
        let empty = build_splat(&analysis(200.0, vec![], 0.9), None).unwrap();
        assert!(!empty.sections.is_empty());
        let changes = (1..=40).map(|index| index as f64 * 4.1).collect();
        let many = build_splat(&analysis(200.0, changes, 0.9), None).unwrap();
        assert!(many.sections.len() <= SPLAT_COLS);
        assert_eq!(many.sections.len(), SPLAT_COLS);
    }

    #[test]
    fn stem_levels_builder_uses_per_beat_rms() {
        let levels = StemLevels::from_stems(0.5, 0.0, 8, 8, |stem, frame| {
            let value = if stem == StemKind::Bass && frame < 4 { 0.5 } else { 0.0 };
            Some([value, value])
        });
        assert_eq!(levels.levels[StemKind::Bass.index()].len(), 2);
        assert!((levels.levels[StemKind::Bass.index()][0] - 0.5).abs() < 1e-6);
        assert_eq!(levels.levels[StemKind::Bass.index()][1], 0.0);
    }
}
