//! Pure mapping from the loop-splat engine state to the compact grid view.

use crate::decks::DeckId;
use crate::loop_splat::{SplatGrid, SplatRow, SplatSnapshot, SPLAT_COLS};
use crate::loop_splat_view::{SplatCellView, SplatDeck, SplatRowView, SplatViewModel};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SplatCoverage {
    pub stems_present: bool,
    /// Model-rate frames separated so far (`StemsMsg::Coverage.model_frames`).
    pub covered_frames: usize,
    pub complete: bool,
    pub model_rate: u32,
}

pub fn splat_deck(deck: DeckId) -> SplatDeck {
    match deck {
        DeckId::A => SplatDeck::A,
        DeckId::B => SplatDeck::B,
    }
}

pub fn splat_row(row: SplatRowView) -> SplatRow {
    match row {
        SplatRowView::Drums => SplatRow::Drums,
        SplatRowView::Bass => SplatRow::Bass,
        SplatRowView::Vocals => SplatRow::Vocals,
        SplatRowView::Other => SplatRow::Other,
        SplatRowView::Mix => SplatRow::Mix,
    }
}

pub fn splat_view_model(
    deck: DeckId,
    grid: &SplatGrid,
    enabled: bool,
    snapshot: Option<&SplatSnapshot>,
    coverage: &SplatCoverage,
    duration_secs: f64,
) -> SplatViewModel {
    let mut model = SplatViewModel::empty(splat_deck(deck));
    model.enabled = snapshot.map_or(enabled, |snapshot| snapshot.active);
    model.cols = grid.sections.len();
    model.col_bars = grid.bars_per_col;
    model.duration_secs = if duration_secs.is_finite() {
        duration_secs.clamp(0.0, f32::MAX as f64) as f32
    } else {
        0.0
    };
    model.bar_phase = snapshot.map_or(0.0, |snapshot| snapshot.bar_phase);

    for (row_index, row) in SplatRow::ALL.into_iter().enumerate() {
        for col in 0..SPLAT_COLS {
            let Some(cell) = grid.cells[row_index][col] else { continue };
            model.spans[row_index][col] = (
                cell.span.start_secs as f32,
                cell.span.len_secs().max(0.0) as f32,
            );
            if row.stem().is_some()
                && (!coverage.stems_present
                    || cell.span.end_secs * coverage.model_rate as f64
                        > coverage.covered_frames as f64)
            {
                continue;
            }
            if cell.silent {
                model.cells[row_index][col] = SplatCellView::Silent;
                continue;
            }
            model.cells[row_index][col] = match snapshot {
                Some(snapshot)
                    if snapshot.playing[row_index]
                        .is_some_and(|(playing_col, _)| playing_col == col as u8) =>
                {
                    let (_, part) = snapshot.playing[row_index].unwrap();
                    SplatCellView::Playing {
                        energy: cell.energy,
                        phase: snapshot.row_phase[row_index],
                        part,
                    }
                }
                Some(snapshot)
                    if snapshot.queued[row_index]
                        .is_some_and(|(queued_col, _)| queued_col == col as u8) =>
                {
                    let (_, part) = snapshot.queued[row_index].unwrap();
                    SplatCellView::Queued { energy: cell.energy, part }
                }
                _ => SplatCellView::Ready { energy: cell.energy },
            };
        }
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::LoopSpan;
    use crate::loop_splat::{SplatCell, SplatPart, SplatSection, SPLAT_ROWS};

    fn cell(start_secs: f64, end_secs: f64, energy: f32, silent: bool) -> SplatCell {
        SplatCell {
            span: LoopSpan { start_secs, end_secs },
            bars: 1,
            energy,
            silent,
        }
    }

    fn grid(cols: usize) -> SplatGrid {
        let sections = (0..cols)
            .map(|col| SplatSection {
                start_secs: col as f64 * 2.0,
                end_secs: (col + 1) as f64 * 2.0,
                bars: 1,
            })
            .collect();
        let mut bars_per_col = [0; SPLAT_COLS];
        bars_per_col[..cols].fill(1);
        SplatGrid {
            bpm: 120.0,
            bar_secs: 2.0,
            first_bar_secs: 0.0,
            sections,
            cells: [[None; SPLAT_COLS]; SPLAT_ROWS],
            bars_per_col,
        }
    }

    fn coverage(covered_frames: usize) -> SplatCoverage {
        SplatCoverage {
            stems_present: true,
            covered_frames,
            complete: false,
            model_rate: 10,
        }
    }

    #[test]
    fn coverage_gates_stems_but_never_the_mix() {
        let mut grid = grid(2);
        grid.cells[SplatRow::Drums.index()][0] = Some(cell(0.0, 2.0, 0.6, false));
        grid.cells[SplatRow::Drums.index()][1] = Some(cell(2.0, 4.0, 0.7, false));
        grid.cells[SplatRow::Mix.index()][1] = Some(cell(2.0, 4.0, 0.8, false));

        let model = splat_view_model(DeckId::A, &grid, false, None, &coverage(20), 4.0);
        assert_eq!(model.cells[SplatRow::Drums.index()][0], SplatCellView::Ready { energy: 0.6 });
        assert_eq!(model.cells[SplatRow::Drums.index()][1], SplatCellView::Empty);
        assert_eq!(model.cells[SplatRow::Mix.index()][1], SplatCellView::Ready { energy: 0.8 });

        let no_stems = SplatCoverage { stems_present: false, ..coverage(usize::MAX) };
        let model = splat_view_model(DeckId::A, &grid, false, None, &no_stems, 4.0);
        assert_eq!(model.cells[SplatRow::Drums.index()][0], SplatCellView::Empty);
        assert_eq!(model.cells[SplatRow::Mix.index()][1], SplatCellView::Ready { energy: 0.8 });
    }

    #[test]
    fn playing_precedes_queued_and_snapshot_owns_enabled_and_phase() {
        let mut grid = grid(2);
        let row = SplatRow::Bass.index();
        grid.cells[row][0] = Some(cell(0.0, 2.0, 0.4, false));
        grid.cells[row][1] = Some(cell(2.0, 4.0, 0.9, false));
        let mut snapshot = SplatSnapshot {
            active: true,
            bar_phase: 0.3,
            ..SplatSnapshot::default()
        };
        let playing_part = SplatPart { num: 1, den: 2 };
        let queued_part = SplatPart { num: 3, den: 4 };
        snapshot.playing[row] = Some((0, playing_part));
        snapshot.queued[row] = Some((0, SplatPart::WHOLE));
        snapshot.row_phase[row] = 0.75;
        let vocals = SplatRow::Vocals.index();
        grid.cells[vocals][1] = Some(cell(2.0, 4.0, 0.5, false));
        snapshot.queued[vocals] = Some((1, queued_part));

        let model = splat_view_model(
            DeckId::B,
            &grid,
            false,
            Some(&snapshot),
            &coverage(usize::MAX),
            4.0,
        );
        assert!(model.enabled);
        assert_eq!(model.bar_phase, 0.3);
        assert_eq!(
            model.cells[row][0],
            SplatCellView::Playing {
                energy: 0.4,
                phase: 0.75,
                part: playing_part,
            }
        );
        assert_eq!(
            model.cells[vocals][1],
            SplatCellView::Queued { energy: 0.5, part: queued_part }
        );
    }

    #[test]
    fn silent_cells_remain_silent() {
        let mut grid = grid(1);
        let row = SplatRow::Other.index();
        grid.cells[row][0] = Some(cell(0.0, 2.0, 0.1, true));
        let mut snapshot = SplatSnapshot::default();
        snapshot.playing[row] = Some((0, SplatPart::WHOLE));
        let model = splat_view_model(
            DeckId::A,
            &grid,
            true,
            Some(&snapshot),
            &coverage(usize::MAX),
            2.0,
        );
        assert_eq!(model.cells[row][0], SplatCellView::Silent);
    }

    #[test]
    fn columns_bar_counts_and_duration_are_copied() {
        let mut grid = grid(3);
        grid.bars_per_col[..3].copy_from_slice(&[1, 2, 4]);
        let model = splat_view_model(DeckId::B, &grid, true, None, &coverage(0), 6.25);
        assert_eq!(model.deck, SplatDeck::B);
        assert_eq!(model.cols, 3);
        assert_eq!(&model.col_bars[..3], &[1, 2, 4]);
        assert_eq!(model.duration_secs, 6.25);
    }

    #[test]
    fn cell_spans_are_copied_and_missing_cells_stay_zero() {
        let mut grid = grid(2);
        let drums = SplatRow::Drums.index();
        let mix = SplatRow::Mix.index();
        grid.cells[drums][0] = Some(cell(1.25, 2.75, 0.6, false));
        grid.cells[mix][1] = Some(cell(4.0, 5.5, 0.8, true));

        let model = splat_view_model(
            DeckId::A,
            &grid,
            false,
            None,
            &coverage(usize::MAX),
            8.0,
        );

        assert_eq!(model.spans[drums][0], (1.25, 1.5));
        assert_eq!(model.spans[mix][1], (4.0, 1.5));
        assert_eq!(model.spans[drums][1], (0.0, 0.0));
    }

    #[test]
    fn an_empty_grid_maps_to_an_empty_model() {
        let model = splat_view_model(DeckId::A, &grid(0), false, None, &coverage(0), 0.0);
        assert_eq!(model, SplatViewModel::empty(SplatDeck::A));
    }

    #[test]
    fn engine_and_view_row_orders_agree() {
        for (engine, view) in SplatRow::ALL.into_iter().zip(SplatRowView::ALL) {
            assert_eq!(engine, splat_row(view));
        }
    }
}
