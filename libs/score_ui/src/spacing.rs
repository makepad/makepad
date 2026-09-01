//! Horizontal spacing and page planning.
//!
//! This is the seam between the semantic score and
//! [`makepad_score_layout`]'s constrained spring-and-rod solver. Nothing here
//! decides *how* to space music — the kernel does that — but everything here
//! decides *what the music actually is*, in numbers the kernel understands:
//!
//! * a **column** is one onset, a moment where something starts, merged
//!   across every voice and both staves of the grand staff so the two hands
//!   line up vertically;
//! * its **spring** (`natural`) is its duration run through the kernel's
//!   duration curve, so a sixteenth asks for less room than a half note but
//!   not sixteen times less;
//! * its **rod** (`minimum`) is measured ink: the real Bravura advance widths
//!   of the noteheads at that onset, the second-interval head shift, the
//!   augmentation dots, the accidentals hanging off the *next* column, and
//!   the barline where the measure ends.
//!
//! The kernel then solves each system's chain to the system width and breaks
//! the measure list into systems and the systems into pages; this module
//! turns the solved widths back into page coordinates the engraver draws at.
//!
//! # Why these flexibilities
//!
//! A column's width under force `F` is `rod + I*q(d)*(1 + F)`: both the
//! stretch and the shrink flexibility are set to the column's *duration
//! space* `I*q(d)`, with `headroom` set to the rod. Three properties follow,
//! and they are exactly the classical engraving behaviour:
//!
//! 1. Ink never scales. Justifying a system moves whitespace around; it never
//!    inflates or squeezes a notehead's own advance.
//! 2. Whitespace scales in proportion to duration space, so a system that has
//!    to stretch keeps the *ratios* between a sixteenth's gap and a half
//!    note's gap. That proportional invariance is what reads as "engraved".
//! 3. `F = -1` is exactly the point where all whitespace is gone and every
//!    rod is touching. Since [`makepad_score_layout::BreakStyle::min_ratio`]
//!    is `-1`, the line breaker's feasibility test and the spacing model's
//!    collision limit become the same statement, with no fudge factor.

use crate::document::PAGE_WIDTH_SP;
use crate::engrave::{
    measure_staff_columns, staff_frames, Column, MARGIN_LEFT, MARGIN_RIGHT, STAFF_SPAN,
};
use crate::font::{music_font, MusicFont};
use makepad_score::model::{KeySignature, Measure, MeasureId, Meter, Rational, Score, ScoreTime};
use makepad_score_layout::{
    break_pages, BreakRule, DistanceStyle, IncrementalLayout, LayoutStyle, LineWidths,
    MeasureSource, PageSpec, RelayoutStats, Sp, SpacingColumn, SystemLayout, SystemVertical,
    TurnRule,
};
use std::{collections::BTreeMap, ops::Range};

/// Page y of the top of the first system's block (its ink, not its staff).
const PAGE_MUSIC_TOP: f64 = 28.0;
/// Page y below which nothing but the folio may be printed.
const PAGE_MUSIC_BOTTOM: f64 = 222.0;

/// One placed onset column: where its noteheads' left edges go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnPlacement {
    /// Absolute score time of the onset.
    pub onset: ScoreTime,
    /// Page x of the unshifted notehead's left edge.
    pub x: f64,
}

/// One placed measure.
#[derive(Clone, Debug)]
pub struct MeasurePlacement {
    pub measure: MeasureId,
    /// Index in score order.
    pub index: usize,
    /// Page x of the measure's left boundary (the opening barline).
    pub left: f64,
    /// Page x of the measure's closing barline.
    pub right: f64,
    /// Onset columns in time order.
    pub columns: Vec<ColumnPlacement>,
}

impl MeasurePlacement {
    /// Page x for an onset, or the measure's left boundary when the onset is
    /// not a column of this measure (which the engraver never asks for).
    pub fn x_of(&self, onset: ScoreTime) -> f64 {
        self.columns
            .iter()
            .find(|column| column.onset == onset)
            .map(|column| column.x)
            .unwrap_or(self.left)
    }

    /// Page x for a point in time inside the measure, interpolated between
    /// the columns that bracket it. Used by the playback cursor.
    pub fn x_at(&self, whole: f64, measure_start: f64, measure_end: f64) -> f64 {
        let time = whole.clamp(measure_start, measure_end);
        let mut previous = (measure_start, self.left);
        for column in &self.columns {
            let at = rational_f64(column.onset.0);
            if at > time + 1e-12 {
                let span = (at - previous.0).max(1e-9);
                let t = ((time - previous.0) / span).clamp(0.0, 1.0);
                return previous.1 + (column.x - previous.1) * t;
            }
            previous = (at, column.x);
        }
        let span = (measure_end - previous.0).max(1e-9);
        let t = ((time - previous.0) / span).clamp(0.0, 1.0);
        previous.1 + (self.right - previous.1) * t
    }
}

/// One placed system.
#[derive(Clone, Debug)]
pub struct SystemPlacement {
    /// Page y of the top staff line of the upper staff.
    pub top: f64,
    /// Page y of the bottom staff line of the lower staff.
    pub bottom: f64,
    /// Page x where this system's music starts.
    pub music_left: f64,
    /// Page x of the system's right edge.
    pub right: f64,
    /// True for the system carrying the score's first measure.
    pub show_meter: bool,
    pub measures: Vec<MeasurePlacement>,
}

/// A moment in the score, resolved to the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorLocation {
    pub page: usize,
    pub x_sp: f64,
    pub top_sp: f64,
    pub bottom_sp: f64,
}

/// How far above and below the staves the playback cursor reaches, so it
/// clears ledger lines and stems without touching the neighbouring system.
const CURSOR_SYSTEM_PAD: f64 = 3.0;

/// One placed page.
#[derive(Clone, Debug, Default)]
pub struct PagePlacement {
    pub systems: Vec<SystemPlacement>,
}

/// Which pages a relayout invalidated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagesDirty {
    /// The break structure moved: everything must be repainted.
    All,
    /// Only these pages changed.
    Only(Vec<usize>),
}

/// Cached per-measure spacing input, kept beside the kernel's own cache so a
/// one-measure edit re-measures one measure's ink rather than the score's.
#[derive(Clone, Debug, Default)]
struct MeasureCache {
    /// The onsets, in time order, matching `source.columns` one for one.
    onsets: Vec<ScoreTime>,
    /// Distance from this measure's closing barline back to the next
    /// measure's first column.
    lead_after: f64,
    /// Ink reach above the upper staff's top line, in staff spaces.
    above: f64,
    /// Ink reach below the lower staff's bottom line.
    below: f64,
}

/// The document's horizontal spacing and page plan.
pub struct ScoreSpacing {
    style: LayoutStyle,
    incremental: IncrementalLayout,
    sources: Vec<MeasureSource>,
    cache: Vec<MeasureCache>,
    measure_ids: Vec<MeasureId>,
    /// Page x where music starts on a system that shows no time signature.
    music_left: f64,
    /// Page x where music starts on the score's very first system.
    music_left_first: f64,
    widths: LineWidths,
    systems: Vec<SystemLayout>,
    pages: Vec<PagePlacement>,
    /// Which systems each page carries, with its vertical adjustment ratio.
    page_fills: Vec<(Range<usize>, f64)>,
    /// Page index of every measure, by score order.
    measure_page: Vec<usize>,
    stats: RelayoutStats,
}

impl Default for ScoreSpacing {
    fn default() -> Self {
        Self::new()
    }
}

impl ScoreSpacing {
    pub fn new() -> Self {
        Self {
            style: LayoutStyle::default(),
            incremental: IncrementalLayout::new(),
            sources: Vec::new(),
            cache: Vec::new(),
            measure_ids: Vec::new(),
            music_left: MARGIN_LEFT + 8.0,
            music_left_first: MARGIN_LEFT + 8.0,
            widths: LineWidths::uniform(Sp(1.0)),
            systems: Vec::new(),
            pages: Vec::new(),
            page_fills: Vec::new(),
            measure_page: Vec::new(),
            stats: RelayoutStats::default(),
        }
    }

    pub fn style(&self) -> &LayoutStyle {
        &self.style
    }

    pub fn pages(&self) -> &[PagePlacement] {
        &self.pages
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn stats(&self) -> RelayoutStats {
        self.stats
    }

    pub fn page_of_measure(&self, index: usize) -> Option<usize> {
        self.measure_page.get(index).copied()
    }

    /// Where a moment in the score is on the page, for the playback cursor:
    /// page index, page x, and the vertical extent of the system it lands in.
    /// The system span is what keeps the cursor from ruling the whole sheet.
    pub fn locate(&self, score: &Score, whole: f64) -> Option<CursorLocation> {
        let index = self.measure_ids.iter().position(|id| {
            score.measures.get(id).is_some_and(|measure| {
                let start = rational_f64(measure.start.0);
                let end = start + rational_f64(measure.extent.0);
                whole >= start && whole < end
            })
        })?;
        let measure = score.measures.get(&self.measure_ids[index])?;
        let start = rational_f64(measure.start.0);
        let end = start + rational_f64(measure.extent.0);
        let page = *self.measure_page.get(index)?;
        let system = self
            .pages
            .get(page)?
            .systems
            .iter()
            .find(|system| system.measures.iter().any(|placement| placement.index == index))?;
        let placement = system
            .measures
            .iter()
            .find(|placement| placement.index == index)?;
        Some(CursorLocation {
            page,
            x_sp: placement.x_at(whole, start, end),
            top_sp: system.top - CURSOR_SYSTEM_PAD,
            bottom_sp: system.bottom + CURSOR_SYSTEM_PAD,
        })
    }

    /// Re-measure every measure and lay the whole score out. Used on load and
    /// after any edit that can move more than one measure (undo, redo).
    pub fn rebuild(&mut self, score: &Score) {
        let font = music_font();
        let measures = ordered_measures(score);
        self.measure_ids = measures.iter().map(|measure| measure.id).collect();
        // Measure every bar's ink once; a bar's trailing rod needs the next
        // bar's leading ink, which is the neighbour in this same list.
        let inks: Vec<MeasureInk> = measures
            .iter()
            .map(|measure| measure_ink(score, font, measure, &self.style.distance))
            .collect();
        let previous: Vec<u64> = self.sources.iter().map(|source| source.revision).collect();
        self.sources.clear();
        self.cache.clear();
        for (index, measure) in measures.iter().enumerate() {
            let revision = previous.get(index).copied().unwrap_or(0).wrapping_add(1);
            let next_left = leading_ink(inks.get(index + 1));
            let (source, cache) = self.springs(measure, &inks[index], next_left, revision);
            self.sources.push(source);
            self.cache.push(cache);
        }
        self.plan_prefix(score, font);
        self.relayout();
        self.repaginate();
    }

    /// One measure changed. Re-measures that measure (and its predecessor,
    /// whose trailing rod reaches across the barline into it), then asks the
    /// kernel for the cheapest relayout it can get away with.
    pub fn touch_measure(&mut self, score: &Score, index: usize) -> PagesDirty {
        if index >= self.sources.len() {
            self.rebuild(score);
            return PagesDirty::All;
        }
        let font = music_font();
        let measures = ordered_measures(score);
        let mut touched = vec![index];
        if index > 0 {
            touched.insert(0, index - 1);
        }
        for &at in &touched {
            let revision = self.sources[at].revision.wrapping_add(1);
            let Some(measure) = measures.get(at) else { continue };
            let ink = measure_ink(score, font, measure, &self.style.distance);
            let next_left = leading_ink(
                measures
                    .get(at + 1)
                    .map(|next| measure_ink(score, font, next, &self.style.distance))
                    .as_ref(),
            );
            let (source, cache) = self.springs(measure, &ink, next_left, revision);
            // The predecessor is only really dirty when the edit changed the
            // ink its trailing rod reaches into, so compare before bumping.
            if at != index
                && source.columns == self.sources[at].columns
                && source.break_right == self.sources[at].break_right
            {
                continue;
            }
            self.sources[at] = source;
            self.cache[at] = cache;
        }
        let before: Vec<Range<usize>> = self.systems.iter().map(|s| s.measures.clone()).collect();
        self.relayout();
        let after: Vec<Range<usize>> = self.systems.iter().map(|s| s.measures.clone()).collect();
        if before != after {
            self.repaginate();
            return PagesDirty::All;
        }
        // Same breaks: only the pages carrying a re-solved system move.
        let mut dirty: Vec<usize> = touched
            .iter()
            .filter_map(|&at| self.measure_page.get(at).copied())
            .collect();
        dirty.sort_unstable();
        dirty.dedup();
        for &page in &dirty {
            self.place_page(page);
        }
        PagesDirty::Only(dirty)
    }

    /// The kernel's incremental line layout over the cached measure sources.
    fn relayout(&mut self) {
        self.systems = self
            .incremental
            .layout(&self.sources, self.widths, &self.style)
            .to_vec();
        self.stats = self.incremental.stats();
    }

    /// Where music starts, and therefore how wide a system is.
    ///
    /// The clef and key signature are drawn at every system start, so their
    /// width comes off every system; the time signature is drawn once, so it
    /// comes off the first system only. The key allowance is the *widest*
    /// key the score uses, which keeps the music left edge aligned down the
    /// page even across a key change.
    fn plan_prefix(&mut self, score: &Score, font: &'static MusicFont) {
        let mut fifths = 0_i8;
        for measure in ordered_measures(score) {
            if let Some(key) = score.maps.key_at(measure.start, None, None) {
                if key.fifths.unsigned_abs() > fifths.unsigned_abs() {
                    fifths = key.fifths;
                }
            }
        }
        let meter = score
            .maps
            .meter_at(ScoreTime::ZERO, None, None)
            .cloned()
            .unwrap_or(Meter::Measured {
                groups: vec![4],
                unit: 4,
            });
        let key = KeySignature { fifths, custom: Vec::new() };
        let lead = self.style.distance.barline_to_note.0;
        self.music_left =
            crate::engrave::prefix_width(font, &key, None, &self.style) + MARGIN_LEFT + lead;
        self.music_left_first =
            crate::engrave::prefix_width(font, &key, Some(&meter), &self.style) + MARGIN_LEFT + lead;
        let right = PAGE_WIDTH_SP - MARGIN_RIGHT;
        // The chain runs from the first column to one `barline_to_note` past
        // the closing barline, so solving to this target lands that barline
        // exactly on the right edge.
        self.widths = LineWidths {
            first: Sp(right - self.music_left_first + lead),
            rest: Sp(right - self.music_left + lead),
        };
    }

    /// Turn one measure's measured ink into springs and rods.
    ///
    /// `next_left` is the ink the *following* measure's first column hangs
    /// left of its notehead — an accidental, say — which this measure's
    /// trailing rod has to clear along with the barline.
    fn springs(
        &self,
        measure: &Measure,
        ink: &MeasureInk,
        next_left: f64,
        revision: u64,
    ) -> (MeasureSource, MeasureCache) {
        let extent = rational_f64(measure.extent.0).max(1e-9);
        let start = rational_f64(measure.start.0);
        let distance = &self.style.distance;
        // The rod has to clear the barline the engraver actually draws, so
        // take its thickness from the font's own engraving defaults.
        let barline = music_font()
            .engraving()
            .thin_barline_thickness
            .max(self.style.stroke.barline_thin.0);
        let lead_after = if next_left > 0.0 {
            (distance.barline_to_accidental.0 + next_left).max(distance.barline_to_note.0)
        } else {
            distance.barline_to_note.0
        };

        let mut columns = Vec::with_capacity(ink.columns.len().max(1));
        for (at, column) in ink.columns.iter().enumerate() {
            let onset = rational_f64(column.onset.0) - start;
            let next = ink
                .columns
                .get(at + 1)
                .map(|next| rational_f64(next.onset.0) - start)
                .unwrap_or(extent);
            // The spring's duration is the interval to the next onset: what
            // this column has to *say*, as opposed to what it has to clear.
            let duration = (next - onset).max(1.0 / 512.0);
            let gap = if at + 1 < ink.columns.len() {
                distance.note_to_note_min.0 + ink.columns[at + 1].left
            } else {
                distance.note_to_barline.0 + barline + lead_after
            };
            columns.push(spring(column.right + gap, duration, &self.style));
        }
        if columns.is_empty() {
            // An empty measure still has to be wide enough to read as one.
            columns.push(spring(distance.min_measure_width.0, extent, &self.style));
        }
        let cache = MeasureCache {
            onsets: ink.columns.iter().map(|column| column.onset).collect(),
            lead_after,
            above: ink.above,
            below: ink.below,
        };
        (
            MeasureSource {
                revision,
                columns,
                break_right: BreakRule::Allowed,
                spanner_penalty: 0.0,
            },
            cache,
        )
    }

    /// Stack the systems onto pages and place every column on every page.
    fn repaginate(&mut self) {
        let verticals: Vec<SystemVertical> = self
            .systems
            .iter()
            .map(|system| {
                let (above, below) = self.system_reach(&system.measures);
                SystemVertical {
                    height: Sp(STAFF_SPAN + above + below),
                    gap_natural: self.style.vertical.system_distance_min,
                    gap_min: self.style.vertical.system_distance_min * 0.7,
                    gap_stretch: self.style.vertical.system_distance_max
                        - self.style.vertical.system_distance_min,
                    turn_after: TurnRule::Allowed,
                }
            })
            .collect();
        let plan = break_pages(
            &verticals,
            PageSpec {
                usable_height: Sp(PAGE_MUSIC_BOTTOM - PAGE_MUSIC_TOP),
            },
            &self.style.breaking,
        );
        self.pages.clear();
        self.measure_page = vec![0; self.sources.len()];
        // A score with nothing on it still gets one page to put its title on.
        let fills: Vec<Range<usize>> = if plan.pages.is_empty() {
            vec![0..self.systems.len()]
        } else {
            plan.pages.iter().map(|page| page.systems.clone()).collect()
        };
        for (page_index, fill) in fills.iter().enumerate() {
            self.pages.push(PagePlacement::default());
            for system in fill.clone() {
                for measure in self.systems[system].measures.clone() {
                    if let Some(slot) = self.measure_page.get_mut(measure) {
                        *slot = page_index;
                    }
                }
            }
        }
        let adjustments: Vec<(Range<usize>, f64)> = fills
            .iter()
            .cloned()
            .zip(
                plan.pages
                    .iter()
                    .map(|page| if page.justified { page.adjustment } else { 0.0 })
                    .chain(std::iter::repeat(0.0)),
            )
            .collect();
        self.page_fills = adjustments;
        for page in 0..self.pages.len() {
            self.place_page(page);
        }
    }

    fn system_reach(&self, measures: &Range<usize>) -> (f64, f64) {
        let mut above = 3.0_f64;
        let mut below = 3.0_f64;
        for index in measures.clone() {
            if let Some(cache) = self.cache.get(index) {
                above = above.max(cache.above);
                below = below.max(cache.below);
            }
        }
        (above.min(14.0), below.min(14.0))
    }

    /// Turn one page's solved column widths into page coordinates.
    fn place_page(&mut self, page_index: usize) {
        let Some((fill, adjustment)) = self.page_fills.get(page_index).cloned() else {
            return;
        };
        let mut placed = PagePlacement::default();
        let mut y = PAGE_MUSIC_TOP;
        for system_index in fill.clone() {
            let system = &self.systems[system_index];
            let (above, below) = self.system_reach(&system.measures);
            if system_index != fill.start {
                let gap = self.style.vertical.system_distance_min.0
                    + adjustment
                        * (self.style.vertical.system_distance_max.0
                            - self.style.vertical.system_distance_min.0);
                y += gap;
            }
            let top = y + above;
            y += STAFF_SPAN + above + below;
            let show_meter = system.measures.start == 0;
            let music_left = if show_meter {
                self.music_left_first
            } else {
                self.music_left
            };
            let mut x = music_left;
            let mut widths = system.solution.widths.iter().map(|w| w.0);
            let mut measures = Vec::with_capacity(system.measures.len());
            // Measure boundaries tile the system: each starts where the
            // previous one's barline stands.
            let mut left = music_left - self.style.distance.barline_to_note.0;
            for index in system.measures.clone() {
                let source = &self.sources[index];
                let cache = &self.cache[index];
                let mut columns = Vec::with_capacity(cache.onsets.len());
                for (at, _) in source.columns.iter().enumerate() {
                    if let Some(&onset) = cache.onsets.get(at) {
                        columns.push(ColumnPlacement { onset, x });
                    }
                    x += widths.next().unwrap_or(0.0);
                }
                // The closing barline stands back from the next measure's
                // first column by the same lead its trailing rod reserved;
                // the system's last barline lands on the right edge.
                let last = index + 1 == system.measures.end;
                let lead = if last {
                    self.style.distance.barline_to_note.0
                } else {
                    cache.lead_after
                };
                let right = x - lead;
                measures.push(MeasurePlacement {
                    measure: self.measure_ids[index],
                    index,
                    left,
                    right,
                    columns,
                });
                left = right;
            }
            placed.systems.push(SystemPlacement {
                top,
                bottom: top + STAFF_SPAN,
                music_left,
                right: PAGE_WIDTH_SP - MARGIN_RIGHT,
                show_meter,
                measures,
            });
        }
        if let Some(slot) = self.pages.get_mut(page_index) {
            *slot = placed;
        }
    }
}

/// Build one column's spring from its rod and its duration.
///
/// `headroom` is the rod, so the regularizer's notion of "whitespace"
/// (`width - headroom`) is exactly the duration space, and both flexibilities
/// are that same duration space — see the module docs for why.
fn spring(rod: f64, duration: f64, style: &LayoutStyle) -> SpacingColumn {
    let space = style.spacing.spacing_increment.0
        * makepad_score_layout::duration_quanta(duration, &style.spacing);
    SpacingColumn {
        natural: Sp(rod + space),
        minimum: Sp(rod),
        stretch_flex: space.max(style.spacing.min_stretch_flex),
        shrink_flex: space.max(style.spacing.min_shrink_flex),
        headroom: Sp(rod),
        duration_class: Some((duration * 1024.0).round().clamp(0.0, 4096.0) as u32),
    }
}

/// One onset's measured ink, merged over every voice and both staves.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColumnInk {
    pub onset: ScoreTime,
    /// Ink reaching left of the notehead origin: accidentals, and the heads
    /// a down-stem chord pushes to the far side of its stem.
    pub left: f64,
    /// Ink reaching right of it: the notehead advance, second-interval head
    /// shifts, augmentation dots.
    pub right: f64,
}

/// A measure's merged onset columns plus its vertical reach.
pub(crate) struct MeasureInk {
    pub columns: Vec<ColumnInk>,
    pub above: f64,
    pub below: f64,
}

/// Measure one measure: what ink sits at each onset, and how far the ink
/// reaches above and below the grand staff.
pub(crate) fn measure_ink(
    score: &Score,
    font: &'static MusicFont,
    measure: &Measure,
    distance: &DistanceStyle,
) -> MeasureInk {
    let key = score
        .maps
        .key_at(measure.start, None, None)
        .cloned()
        .unwrap_or(KeySignature::C_MAJOR);
    let frames = staff_frames(0.0);
    let staves = measure_staff_columns(font, score, measure, &key, &frames);
    let mut merged: BTreeMap<ScoreTime, (f64, f64)> = BTreeMap::new();
    let mut top = 0.0_f64;
    let mut bottom = STAFF_SPAN;
    for voices in &staves {
        for columns in voices {
            for column in columns {
                let (left, right) = column_extents(font, column, distance);
                let entry = merged.entry(column.time).or_insert((0.0, 0.0));
                entry.0 = entry.0.max(left);
                entry.1 = entry.1.max(right);
                top = top.min(column.top_y());
                bottom = bottom.max(column.bottom_y());
            }
        }
    }
    MeasureInk {
        columns: merged
            .into_iter()
            .map(|(onset, (left, right))| ColumnInk { onset, left, right })
            .collect(),
        // A notehead is one staff space tall; stems, beams and flags reach
        // roughly a stem length past the outermost head.
        above: (-top + 1.0 + 2.0).max(3.0),
        below: (bottom - STAFF_SPAN + 1.0 + 2.0).max(3.0),
    }
}

/// How far one chord's ink reaches either side of its notehead origin.
fn column_extents(font: &'static MusicFont, column: &Column, distance: &DistanceStyle) -> (f64, f64) {
    let head = column
        .heads
        .iter()
        .map(|head| font.advance(&head.glyph).max(font.bbox(&head.glyph).width()))
        .fold(0.0_f64, f64::max)
        .max(0.6);
    let shifted = column.heads.iter().any(|head| head.shifted);
    // A second-interval head sits on the far side of the stem: to the right
    // for an up stem, to the left for a down stem.
    let (mut left, mut right) = match (shifted, column.stem_up) {
        (false, _) => (0.0, head),
        (true, true) => (0.0, head * 2.0),
        (true, false) => (head, head),
    };
    let accidental = column
        .heads
        .iter()
        .filter_map(|head| head.accidental.as_deref())
        .map(|name| font.advance(name).max(font.bbox(name).width()))
        .fold(0.0_f64, f64::max);
    if accidental > 0.0 {
        left += accidental + distance.accidental_to_note.0;
    }
    if column.value.dots > 0 {
        let dot = font.advance("augmentationDot").max(0.3);
        let dots = f64::from(column.value.dots);
        right += distance.note_to_dot.0 + dots * dot + (dots - 1.0) * distance.dot_to_dot.0;
    }
    (left, right)
}

/// How far the first column of a measure reaches left of its notehead.
fn leading_ink(ink: Option<&MeasureInk>) -> f64 {
    ink.and_then(|ink| ink.columns.first())
        .map(|column| column.left)
        .unwrap_or(0.0)
}

pub(crate) fn ordered_measures(score: &Score) -> Vec<&Measure> {
    let mut measures: Vec<&Measure> = score.measures.values().collect();
    measures.sort_by_key(|measure| (measure.ordinal, measure.start));
    measures
}

pub(crate) fn rational_f64(value: Rational) -> f64 {
    value.numerator() as f64 / value.denominator() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engrave::tests::{engrave, fixture, fixture_events, Placed};
    use makepad_score::model::Step;
    use makepad_score_layout::duration_quanta;

    const SCALE: [Step; 7] = [
        Step::C,
        Step::D,
        Step::E,
        Step::F,
        Step::G,
        Step::A,
        Step::B,
    ];

    /// The x of every notehead on the page, left to right.
    fn head_xs(score: &Score) -> Vec<f64> {
        let mut xs: Vec<f64> = engrave(score)
            .noteheads
            .into_iter()
            .map(|(x, _, _)| x)
            .collect();
        xs.sort_by(f64::total_cmp);
        xs
    }

    fn run(count: usize, denominator: u64) -> Score {
        let pitches: Vec<(Step, i8)> = (0..count).map(|i| (SCALE[i % 7], 4)).collect();
        fixture(&pitches, denominator)
    }

    /// The headline defect this module exists to fix: a run of equal notes
    /// must advance by one constant step, not bunch up at the barline.
    #[test]
    fn a_run_of_sixteenths_advances_in_equal_steps() {
        let font = music_font();
        let style = LayoutStyle::default();
        let xs = head_xs(&run(16, 16));
        assert_eq!(xs.len(), 16);
        let steps: Vec<f64> = xs.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let first = steps[0];
        for step in &steps {
            assert!(
                (step - first).abs() < 1e-9,
                "sixteenths are unevenly spaced: {steps:?}"
            );
        }
        // And every step clears the ink: notehead advance plus the house
        // minimum whitespace between notes.
        let rod = font.advance("noteheadBlack") + style.distance.note_to_note_min.0;
        assert!(first > rod, "step {first} is tighter than the rod {rod}");
    }

    /// A single bar is not smeared across the page: below the style's
    /// fill threshold the last system stays ragged at its natural width.
    #[test]
    fn a_one_bar_score_stays_ragged() {
        let drawn = engrave(&run(16, 16));
        let margin = PAGE_WIDTH_SP - MARGIN_RIGHT;
        assert!(
            drawn.system_right < margin - 10.0,
            "a one-bar score was stretched to {} of {margin}",
            drawn.system_right
        );
        // But it still starts where music starts, and the notes fill it.
        let last = drawn.noteheads.iter().map(|(x, _, _)| *x).fold(0.0, f64::max);
        assert!(last < drawn.system_right && last > drawn.system_right - 6.0);
    }

    /// Space follows the duration curve, not the clock: a half note gets the
    /// curve's ratio more whitespace than a quarter, never four times more.
    #[test]
    fn a_long_note_earns_curve_much_room_not_clock_much() {
        let font = music_font();
        let style = LayoutStyle::default();
        let xs = head_xs(&fixture_events(&[
            Placed { onset: (0, 1), duration: (1, 2), step: Step::G, octave: 4 },
            Placed { onset: (1, 2), duration: (1, 4), step: Step::A, octave: 4 },
            Placed { onset: (3, 4), duration: (1, 4), step: Step::B, octave: 4 },
        ]));
        assert_eq!(xs.len(), 3);
        let gap = style.distance.note_to_note_min.0;
        let white_half = (xs[1] - xs[0]) - (font.advance("noteheadHalf") + gap);
        let white_quarter = (xs[2] - xs[1]) - (font.advance("noteheadBlack") + gap);
        let want = duration_quanta(0.5, &style.spacing) / duration_quanta(0.25, &style.spacing);
        let got = white_half / white_quarter;
        assert!(
            (got - want).abs() < 1e-6,
            "whitespace ratio {got} should follow the duration curve {want}"
        );
        // Sanity: the clock ratio would have been 2.0.
        assert!(got < 1.5);
    }

    /// The rods are measured ink, not a guess: for a plain run they are the
    /// font's own notehead advance plus the house note-to-note minimum, and
    /// the last one also has to clear the barline.
    #[test]
    fn rods_are_measured_from_the_font() {
        let font = music_font();
        let style = LayoutStyle::default();
        let score = run(8, 8);
        let measures = ordered_measures(&score);
        let ink = measure_ink(&score, font, measures[0], &style.distance);
        assert_eq!(ink.columns.len(), 8);
        for column in &ink.columns {
            assert_eq!(column.left, 0.0);
            assert!((column.right - font.advance("noteheadBlack")).abs() < 1e-9);
        }
        let mut spacing = ScoreSpacing::new();
        spacing.rebuild(&score);
        let rods: Vec<f64> = spacing.sources[0]
            .columns
            .iter()
            .map(|column| column.minimum.0)
            .collect();
        let inner = font.advance("noteheadBlack") + style.distance.note_to_note_min.0;
        for rod in &rods[..7] {
            assert!((rod - inner).abs() < 1e-9, "inner rod {rod} != {inner}");
        }
        // The trailing rod carries the note-to-barline space, the barline
        // itself and the lead into the next measure.
        assert!(rods[7] > inner + style.distance.note_to_barline.0);
    }

    /// Every column of a system is a spring whose whitespace is its duration
    /// space scaled by one shared force. That is the invariant the engraved
    /// picture rests on, so pin it directly on the solved widths.
    #[test]
    fn one_force_scales_every_column_s_duration_space() {
        let score = run(12, 16);
        let mut spacing = ScoreSpacing::new();
        spacing.rebuild(&score);
        let system = &spacing.systems[0];
        let force = system.solution.force;
        for (column, width) in spacing.sources[0]
            .columns
            .iter()
            .zip(&system.solution.widths)
        {
            let want = column.minimum.0 + (column.natural.0 - column.minimum.0) * (1.0 + force);
            assert!(
                (width.0 - want).abs() < 1e-9,
                "column width {} is not rod + duration space * (1 + F)",
                width.0
            );
            assert!(width.0 >= column.minimum.0 - 1e-12, "a rod was violated");
        }
    }

    /// Systems no longer hold a constant four measures: the breaker decides,
    /// and a bar of sixteenths costs more room than a bar of quarters.
    #[test]
    fn measures_per_system_follows_the_music() {
        let dense = {
            let mut score = run(16, 16);
            grow(&mut score, 8);
            score
        };
        let sparse = {
            let mut score = run(4, 4);
            grow(&mut score, 8);
            score
        };
        let mut a = ScoreSpacing::new();
        a.rebuild(&dense);
        let mut b = ScoreSpacing::new();
        b.rebuild(&sparse);
        let per = |s: &ScoreSpacing| s.systems[0].measures.len();
        assert!(
            per(&a) < per(&b),
            "sixteenths {} should not fit as densely as quarters {}",
            per(&a),
            per(&b)
        );
    }

    /// Repeat a one-measure fixture `count` times, so a score has something
    /// for the line breaker to break.
    fn grow(score: &mut Score, count: u32) {
        let first = ordered_measures(score)[0].id;
        let template = score.measures[&first].clone();
        let events: Vec<_> = score
            .voices
            .values()
            .map(|voice| (voice.id, voice.events.clone()))
            .collect();
        let mut ids = makepad_score::model::IdGenerator::new(0x7e58);
        for ordinal in 1..count {
            let start = ScoreTime::new(i64::from(ordinal), 1).unwrap();
            let id = ids.next::<makepad_score::model::MeasureTag>().unwrap();
            score.measures.insert(
                id,
                Measure {
                    id,
                    ordinal,
                    label: (ordinal + 1).to_string(),
                    start,
                    extent: template.extent,
                },
            );
            score.flow.nodes.push(makepad_score::model::FlowNode {
                measure: id,
                ordinal,
            });
            for (voice, source) in &events {
                let mut copies = Vec::with_capacity(source.len());
                for event in source {
                    let mut event = event.clone();
                    event.id = ids.next::<makepad_score::model::EventTag>().unwrap();
                    event.onset = event.onset.checked_add_time(start).unwrap();
                    if let makepad_score::model::EventKind::Chord(notes) = &mut event.kind {
                        for note in notes {
                            note.id = ids.next::<makepad_score::model::NoteTag>().unwrap();
                        }
                    }
                    copies.push(event);
                }
                score.voices.get_mut(voice).unwrap().events.extend(copies);
            }
        }
    }
}
