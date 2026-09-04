//! Compact view-model and direct-draw widget for the loop splat.

use crate::loop_blocks::CellBlocks;
use crate::loop_splat::SplatPart;
use crate::music_view::{WavePyramid, STEM_COLORS};
use crate::wave_analysis::ZOOM_COLS_PER_SEC;
use makepad_widgets::*;
use std::sync::Arc;

pub const SPLAT_COLS: usize = 8;
pub const SPLAT_ROWS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SplatRowView {
    Drums,
    Bass,
    Vocals,
    Other,
    Mix,
}

impl SplatRowView {
    pub const ALL: [Self; SPLAT_ROWS] = [
        Self::Drums,
        Self::Bass,
        Self::Vocals,
        Self::Other,
        Self::Mix,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Drums => "DRUMS",
            Self::Bass => "BASS",
            Self::Vocals => "VOCALS",
            Self::Other => "OTHER",
            Self::Mix => "MIX",
        }
    }

    pub fn color(self) -> [f32; 4] {
        match self {
            Self::Drums => STEM_COLORS[1],
            Self::Bass => STEM_COLORS[2],
            Self::Vocals => STEM_COLORS[0],
            Self::Other => STEM_COLORS[3],
            Self::Mix => [0.80, 0.82, 0.85, 1.0],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SplatCellView {
    Empty,
    Silent,
    Ready { energy: f32 },
    Queued { energy: f32, part: SplatPart },
    Playing { energy: f32, phase: f32, part: SplatPart },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplatDeck {
    A,
    B,
}

#[derive(Clone, Debug)]
pub struct SplatViewModel {
    pub deck: SplatDeck,
    pub enabled: bool,
    pub cols: usize,
    pub col_bars: [u8; SPLAT_COLS],
    /// Each cell's source-time start and length. Missing cells stay zeroed.
    pub spans: [[(f32, f32); SPLAT_COLS]; SPLAT_ROWS],
    pub duration_secs: f32,
    pub cells: [[SplatCellView; SPLAT_COLS]; SPLAT_ROWS],
    pub blocks: [[Option<Arc<CellBlocks>>; SPLAT_COLS]; SPLAT_ROWS],
    pub bar_phase: f32,
    /// The score-popup preview cursor: row, column, and loop phase.
    pub preview: Option<(usize, usize, f32)>,
    /// What the deck is still doing before the grid is complete (text, progress 0..1;
    /// `None` progress = indeterminate). Drawn as an overlay over the grid.
    pub status: Option<(String, Option<f32>)>,
}

impl PartialEq for SplatViewModel {
    fn eq(&self, other: &Self) -> bool {
        self.deck == other.deck
            && self.enabled == other.enabled
            && self.cols == other.cols
            && self.col_bars == other.col_bars
            && self.spans == other.spans
            && self.duration_secs == other.duration_secs
            && self.cells == other.cells
            && self.bar_phase == other.bar_phase
            && self.preview == other.preview
            && self.status == other.status
            && self.blocks.iter().flatten().zip(other.blocks.iter().flatten()).all(
                |(left, right)| match (left, right) {
                    (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                    (None, None) => true,
                    _ => false,
                },
            )
    }
}

impl SplatViewModel {
    pub fn empty(deck: SplatDeck) -> Self {
        Self {
            deck,
            enabled: false,
            cols: 0,
            col_bars: [0; SPLAT_COLS],
            spans: [[(0.0, 0.0); SPLAT_COLS]; SPLAT_ROWS],
            duration_secs: 0.0,
            cells: [[SplatCellView::Empty; SPLAT_COLS]; SPLAT_ROWS],
            blocks: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            bar_phase: 0.0,
            preview: None,
            status: None,
        }
    }
}

impl Default for SplatViewModel {
    fn default() -> Self {
        Self::empty(SplatDeck::A)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum LoopSplatAction {
    /// `timed` = shift held: stop on the next bar instead of at once.
    Cell { row: SplatRowView, col: u8, timed: bool, part: SplatPart },
    StopRow { row: SplatRowView, timed: bool },
    LaunchColumn { col: u8, timed: bool },
    FocusDeck(SplatDeck),
    ToggleEnabled,
    ToggleScore,
    #[default]
    None,
}

/// One instanced material for every grid cell. The shader interprets
/// `state` as empty/silent/ready/queued/playing in that order.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplatCell {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub phase: f32,
    /// Progress through the current bar: the countdown to a queued launch.
    #[live]
    pub bar_phase: f32,
    #[live]
    pub state: f32,
    #[live]
    pub hover: f32,
    #[live]
    pub part_x0: f32,
    #[live(1.0)]
    pub part_x1: f32,
    #[live]
    pub part_y0: f32,
    #[live(1.0)]
    pub part_y1: f32,
    /// Source span in finest-level pyramid columns.
    #[live]
    pub span_start: f32,
    #[live]
    pub span_cols: f32,
    /// vocals/drums/bass/other = 0/1/2/3; mix = 4.
    #[live]
    pub channel: f32,
    #[live(1.0)]
    pub tex_w: f32,
    #[live(1.0)]
    pub tex_h: f32,
    #[live]
    pub lo_row: f32,
    #[live(1.0)]
    pub lo_cols: f32,
    #[live(1.0)]
    pub lo_scale: f32,
    #[live]
    pub hi_row: f32,
    #[live(1.0)]
    pub hi_cols: f32,
    #[live(2.0)]
    pub hi_scale: f32,
    #[live]
    pub lod_blend: f32,
    #[live]
    pub has_mix: f32,
    #[live]
    pub has_stems: f32,
    #[live]
    pub has_blocks: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSplatBlock {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub alpha: f32,
}

/// Convert the model's `(start_secs, len_secs)` span to the waveform
/// pyramid's finest-column timebase.
pub(crate) fn span_to_pyramid_columns(span: (f32, f32)) -> (f32, f32) {
    let rate = ZOOM_COLS_PER_SEC as f32;
    if !span.0.is_finite() || !span.1.is_finite() {
        return (0.0, 0.0);
    }
    (span.0.max(0.0) * rate, span.1.max(0.0) * rate)
}

const PAD: f64 = 8.0;
const COL_HEAD_H: f64 = 18.0;
const ROW_HEAD_W: f64 = 76.0;
const CELL_MIN: f64 = 44.0;
const CELL_INSET: f64 = 2.0;
const SLOT_INSET: f64 = 2.0;
const BLOCK_INSET: f64 = 4.0;
const BLOCK_GAP: f64 = 1.0;
const BLOCK_MIN_W: f64 = 2.0;
const BLOCK_MIN_H: f64 = 2.0;
const LAUNCH_H: f64 = 18.0;
const FOOT_GAP: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplatHit {
    Cell { row: usize, col: usize, part: SplatPart },
    StopRow(usize),
    LaunchColumn(usize),
}

#[derive(Clone, Copy)]
struct SplatGeometry {
    grid: Rect,
    col_head_y: f64,
    cell_h: f64,
    launch_y: f64,
    cols: usize,
}

impl SplatGeometry {
    fn new(rect: Rect, cols: usize) -> Self {
        let cols = cols.clamp(1, SPLAT_COLS);
        let inner_x = rect.pos.x + PAD;
        let inner_w = (rect.size.x - PAD * 2.0).max(1.0);
        let grid_x = inner_x + ROW_HEAD_W;
        let grid_w = (inner_w - ROW_HEAD_W).max(cols as f64);
        let col_head_y = rect.pos.y + PAD;
        let grid_y = col_head_y + COL_HEAD_H;
        let footer = FOOT_GAP + LAUNCH_H;
        let available = (rect.pos.y + rect.size.y - PAD - footer - grid_y) / SPLAT_ROWS as f64;
        let cell_w = grid_w / cols as f64;
        let cell_h = cell_w.min(available).max(CELL_MIN);
        let grid = Rect {
            pos: dvec2(grid_x, grid_y),
            size: dvec2(grid_w, cell_h * SPLAT_ROWS as f64),
        };
        let launch_y = grid.pos.y + grid.size.y + FOOT_GAP;
        Self {
            grid,
            col_head_y,
            cell_h,
            launch_y,
            cols,
        }
    }

    fn col_w(self) -> f64 {
        self.grid.size.x / self.cols as f64
    }

    fn cell_rect(self, row: usize, col: usize) -> Rect {
        Rect {
            pos: dvec2(
                self.grid.pos.x + col as f64 * self.col_w() + CELL_INSET,
                self.grid.pos.y + row as f64 * self.cell_h + CELL_INSET,
            ),
            size: dvec2(
                (self.col_w() - CELL_INSET * 2.0).max(1.0),
                (self.cell_h - CELL_INSET * 2.0).max(1.0),
            ),
        }
    }

    fn cell_bounds(self, row: usize, col: usize) -> Rect {
        Rect {
            pos: dvec2(
                self.grid.pos.x + col as f64 * self.col_w(),
                self.grid.pos.y + row as f64 * self.cell_h,
            ),
            size: dvec2(self.col_w(), self.cell_h),
        }
    }

    fn part_rect(self, row: usize, col: usize, part: SplatPart) -> Rect {
        slot_rect(self.cell_rect(row, col), part)
    }

    fn row_stop(self, row: usize) -> Rect {
        Rect {
            pos: dvec2(
                self.grid.pos.x - 18.0,
                self.grid.pos.y + row as f64 * self.cell_h + (self.cell_h - 14.0) * 0.5,
            ),
            size: dvec2(14.0, 14.0),
        }
    }

    fn launch(self, col: usize) -> Rect {
        Rect {
            pos: dvec2(self.grid.pos.x + col as f64 * self.col_w() + CELL_INSET, self.launch_y),
            size: dvec2((self.col_w() - CELL_INSET * 2.0).max(1.0), LAUNCH_H - 2.0),
        }
    }

    fn hit(self, pos: DVec2) -> Option<SplatHit> {
        if let Some((row, col)) = cell_at_cols(self.grid, pos, self.cols) {
            let part = part_at(self.cell_bounds(row, col), pos)?;
            return Some(SplatHit::Cell { row, col, part });
        }
        for row in 0..SPLAT_ROWS {
            if self.row_stop(row).contains(pos) {
                return Some(SplatHit::StopRow(row));
            }
        }
        for col in 0..self.cols {
            if self.launch(col).contains(pos) {
                return Some(SplatHit::LaunchColumn(col));
            }
        }
        None
    }
}

fn slot_area(rect: Rect) -> Rect {
    Rect {
        pos: rect.pos + dvec2(SLOT_INSET, SLOT_INSET),
        size: dvec2(
            (rect.size.x - SLOT_INSET * 2.0).max(0.0),
            (rect.size.y - SLOT_INSET * 2.0).max(0.0),
        ),
    }
}

fn slot_rect(rect: Rect, part: SplatPart) -> Rect {
    if part == SplatPart::WHOLE {
        return rect;
    }
    let area = slot_area(rect);
    let den = f64::from(part.den.max(1));
    let x0 = f64::from(part.num.min(part.den.saturating_sub(1))) / den;
    let (y0, y1) = if part.den == 2 { (0.0, 0.5) } else { (0.5, 1.0) };
    Rect {
        pos: dvec2(area.pos.x + area.size.x * x0, area.pos.y + area.size.y * y0),
        size: dvec2(area.size.x / den, area.size.y * (y1 - y0)),
    }
}

fn part_at(rect: Rect, pos: DVec2) -> Option<SplatPart> {
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(pos) {
        return None;
    }
    let x = ((pos.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0 - f64::EPSILON);
    let y = (pos.y - rect.pos.y) / rect.size.y;
    let den = if y < 0.5 { 2 } else { 4 };
    Some(SplatPart {
        num: (x * f64::from(den)).floor() as u8,
        den,
    })
}

fn cell_at_cols(rect: Rect, pos: DVec2, cols: usize) -> Option<(usize, usize)> {
    if cols == 0 || rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(pos) {
        return None;
    }
    let col = (((pos.x - rect.pos.x) / rect.size.x) * cols as f64).floor() as usize;
    let row = (((pos.y - rect.pos.y) / rect.size.y) * SPLAT_ROWS as f64).floor() as usize;
    (row < SPLAT_ROWS && col < cols).then_some((row, col))
}

pub(crate) fn cell_at(rect: Rect, pos: DVec2) -> Option<(usize, usize)> {
    cell_at_cols(rect, pos, SPLAT_COLS)
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjLoopSplat {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_cell: DrawSplatCell,
    #[live]
    draw_block: DrawSplatBlock,
    #[live]
    draw_chrome: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_small: DrawText,
    #[redraw]
    #[area]
    area: Area,
    #[rust]
    model: SplatViewModel,
    #[rust]
    mix_pyramid: Option<WavePyramid>,
    #[rust]
    stem_pyramid: Option<WavePyramid>,
    #[rust]
    hover: Option<SplatHit>,
    #[rust]
    pressed: Option<SplatHit>,
    #[rust]
    selected: Option<(SplatRowView, u8)>,
    #[rust]
    anim_frame: NextFrame,
}

impl VjLoopSplat {
    pub fn set_model(&mut self, cx: &mut Cx, mut model: SplatViewModel) {
        model.cols = model.cols.min(SPLAT_COLS);
        model.bar_phase = model.bar_phase.clamp(0.0, 1.0);
        model.preview = model.preview.and_then(|(row, col, phase)| {
            (row < SPLAT_ROWS && col < model.cols)
                .then_some((row, col, phase.clamp(0.0, 1.0)))
        });
        if self.model != model {
            self.model = model;
            self.area.redraw(cx);
        }
    }

    pub fn model(&self) -> &SplatViewModel {
        &self.model
    }

    pub fn set_waves(
        &mut self,
        cx: &mut Cx,
        mix: Option<WavePyramid>,
        stems: Option<WavePyramid>,
    ) {
        if self.mix_pyramid != mix || self.stem_pyramid != stems {
            self.mix_pyramid = mix;
            self.stem_pyramid = stems;
            self.area.redraw(cx);
        }
    }

    pub fn set_selected(&mut self, cx: &mut Cx, row: SplatRowView, col: u8) {
        let selected = ((col as usize) < SPLAT_COLS).then_some((row, col));
        if self.selected != selected {
            self.selected = selected;
            self.area.redraw(cx);
        }
    }

    pub fn selected(&self) -> Option<(SplatRowView, u8)> {
        self.selected
    }

    fn animates(&self) -> bool {
        if matches!(self.model.status, Some((_, None))) {
            return true;
        }
        self.model
            .cells
            .iter()
            .flatten()
            .any(|cell| matches!(cell, SplatCellView::Queued { .. } | SplatCellView::Playing { .. }))
    }

    fn hit_at(&self, cx: &Cx, pos: DVec2) -> Option<SplatHit> {
        if self.model.cols == 0 {
            return None;
        }
        SplatGeometry::new(self.area.rect(cx), self.model.cols).hit(pos)
    }

    fn hovered_part(&self, row: usize, col: usize) -> Option<SplatPart> {
        match self.hover {
            Some(SplatHit::Cell {
                row: hit_row,
                col: hit_col,
                part,
            }) if hit_row == row && hit_col == col => Some(part),
            _ => None,
        }
    }

    fn emit(&mut self, cx: &mut Cx, hit: SplatHit, timed: bool) {
        let action = match hit {
            SplatHit::Cell { row, col, part } => {
                let row = SplatRowView::ALL[row];
                self.set_selected(cx, row, col as u8);
                LoopSplatAction::Cell { row, col: col as u8, timed, part }
            }
            SplatHit::StopRow(row) => LoopSplatAction::StopRow { row: SplatRowView::ALL[row], timed },
            SplatHit::LaunchColumn(col) => LoopSplatAction::LaunchColumn { col: col as u8, timed },
        };
        cx.widget_action(self.widget_uid(), action);
    }

    fn draw_box(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_chrome.color = color;
        self.draw_chrome.draw_abs(cx, rect);
    }

    fn text_width(&self, cx: &mut Cx2d, text: &str, small: bool) -> f64 {
        let draw = if small { &self.draw_small } else { &self.draw_text };
        let laid = draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        laid.size_in_lpxs.width as f64 * draw.font_scale as f64
    }

    fn draw_centered(
        &mut self,
        cx: &mut Cx2d,
        rect: Rect,
        text: &str,
        color: Vec4f,
        small: bool,
    ) {
        let width = self.text_width(cx, text, small);
        let pos = dvec2(
            rect.pos.x + (rect.size.x - width) * 0.5,
            rect.pos.y + (rect.size.y - if small { 9.0 } else { 11.0 }) * 0.5,
        );
        let draw = if small {
            &mut self.draw_small
        } else {
            &mut self.draw_text
        };
        draw.color = color;
        draw.draw_abs(cx, pos, text);
    }

    fn draw_cell_at(
        &mut self,
        cx: &mut Cx2d,
        rect: Rect,
        row: SplatRowView,
        cell: SplatCellView,
        hover: bool,
        enabled: bool,
        span: (f32, f32),
        has_blocks: bool,
    ) {
        let mut c = row.color();
        if !enabled {
            c[0] *= 0.55;
            c[1] *= 0.55;
            c[2] *= 0.55;
        }
        self.draw_cell.color = vec4(c[0], c[1], c[2], c[3]);
        let (state, phase, part) = match cell {
            SplatCellView::Empty => (0.0, 0.0, SplatPart::WHOLE),
            SplatCellView::Silent => (1.0, 0.0, SplatPart::WHOLE),
            SplatCellView::Ready { .. } => (2.0, 0.0, SplatPart::WHOLE),
            SplatCellView::Queued { part, .. } => (3.0, 0.0, part),
            SplatCellView::Playing { phase, part, .. } => (4.0, phase, part),
        };
        self.draw_cell.state = state;
        self.draw_cell.bar_phase = self.model.bar_phase.clamp(0.0, 1.0);
        self.draw_cell.phase = phase.clamp(0.0, 1.0);
        self.draw_cell.hover = if hover { 1.0 } else { 0.0 };
        let part_rect = slot_rect(rect, part);
        let width = rect.size.x.max(1.0);
        let height = rect.size.y.max(1.0);
        self.draw_cell.part_x0 = ((part_rect.pos.x - rect.pos.x) / width) as f32;
        self.draw_cell.part_x1 = ((part_rect.pos.x + part_rect.size.x - rect.pos.x) / width) as f32;
        self.draw_cell.part_y0 = ((part_rect.pos.y - rect.pos.y) / height) as f32;
        self.draw_cell.part_y1 = ((part_rect.pos.y + part_rect.size.y - rect.pos.y) / height) as f32;
        self.draw_cell.has_blocks = if has_blocks { 1.0 } else { 0.0 };
        self.draw_cell.channel = match row {
            SplatRowView::Vocals => 0.0,
            SplatRowView::Drums => 1.0,
            SplatRowView::Bass => 2.0,
            SplatRowView::Other => 3.0,
            SplatRowView::Mix => 4.0,
        };
        let (span_start, mut span_cols) = span_to_pyramid_columns(span);
        let duration_cols = self.model.duration_secs.max(0.0) * ZOOM_COLS_PER_SEC as f32;
        self.draw_cell.span_start = span_start.min(duration_cols);
        span_cols = span_cols.min((duration_cols - self.draw_cell.span_start).max(0.0));
        self.draw_cell.span_cols = span_cols;
        if let Some(pyramid) = self.mix_pyramid.as_ref() {
            self.draw_cell.tex_w = pyramid.width.max(1) as f32;
            self.draw_cell.tex_h = pyramid.height.max(1) as f32;
            let inner_width = (rect.size.x - 8.0).max(1.0);
            let cols_per_px = (span_cols as f64 / inner_width).max(0.001);
            let (lo, lo_scale, hi, hi_scale, blend) = pyramid.levels_for(cols_per_px);
            self.draw_cell.lo_row = lo.base_row as f32;
            self.draw_cell.lo_cols = lo.cols.max(1) as f32;
            self.draw_cell.lo_scale = lo_scale as f32;
            self.draw_cell.hi_row = hi.base_row as f32;
            self.draw_cell.hi_cols = hi.cols.max(1) as f32;
            self.draw_cell.hi_scale = hi_scale as f32;
            self.draw_cell.lod_blend = blend as f32;
        } else {
            self.draw_cell.tex_w = 1.0;
            self.draw_cell.tex_h = 1.0;
            self.draw_cell.lo_cols = 1.0;
            self.draw_cell.hi_cols = 1.0;
            self.draw_cell.lo_scale = 1.0;
            self.draw_cell.hi_scale = 2.0;
            self.draw_cell.lod_blend = 0.0;
        }
        self.draw_cell.draw_abs(cx, rect);
    }

    fn draw_blocks_at(
        &mut self,
        cx: &mut Cx2d,
        rect: Rect,
        row: SplatRowView,
        cell: SplatCellView,
        blocks: &CellBlocks,
    ) {
        let alpha = match cell {
            SplatCellView::Ready { .. } | SplatCellView::Queued { .. } => 0.85,
            SplatCellView::Playing { .. } => 1.0,
            SplatCellView::Empty | SplatCellView::Silent => return,
        };
        let inner = Rect {
            pos: rect.pos + dvec2(BLOCK_INSET, BLOCK_INSET),
            size: dvec2(
                (rect.size.x - BLOCK_INSET * 2.0).max(1.0),
                (rect.size.y - BLOCK_INSET * 2.0).max(1.0),
            ),
        };
        let total_beats = f64::from(blocks.bars) * 4.0;
        if total_beats <= 0.0 {
            return;
        }
        let px_per_beat = inner.size.x / total_beats;
        let stem = row.color();
        for block in &blocks.blocks {
            let lanes = block.lanes.max(1) as f64;
            let lane = block.lane.min(block.lanes.saturating_sub(1)) as f64;
            let slot_h = inner.size.y / lanes;
            let height = (slot_h - BLOCK_GAP).max(BLOCK_MIN_H).min(inner.size.y);
            let y = inner.pos.y + inner.size.y - (lane + 1.0) * slot_h
                + (slot_h - height) * 0.5;
            let start = f64::from(block.start_beats).clamp(0.0, total_beats);
            let x = inner.pos.x + start * px_per_beat;
            let width = (f64::from(block.len_beats.max(0.0)) * px_per_beat - BLOCK_GAP)
                .max(BLOCK_MIN_W)
                .min((inner.pos.x + inner.size.x - x).max(0.0));
            if width <= 0.0 {
                continue;
            }
            // A quiet event is still 70% stem colour; the loudest events
            // approach white without losing the row hue entirely.
            let whiten = block.velocity.clamp(0.3, 1.0) * 0.9;
            self.draw_block.color = vec4(
                stem[0] + (1.0 - stem[0]) * whiten,
                stem[1] + (1.0 - stem[1]) * whiten,
                stem[2] + (1.0 - stem[2]) * whiten,
                1.0,
            );
            self.draw_block.alpha = alpha;
            self.draw_block.draw_abs(cx, Rect { pos: dvec2(x, y), size: dvec2(width, height) });
        }
    }

    fn draw_selection(&mut self, cx: &mut Cx2d, rect: Rect) {
        let white = Vec4f::from_u32(0xffffffff);
        for edge in [
            Rect { pos: rect.pos, size: dvec2(rect.size.x, 1.0) },
            Rect {
                pos: dvec2(rect.pos.x, rect.pos.y + rect.size.y - 1.0),
                size: dvec2(rect.size.x, 1.0),
            },
            Rect { pos: rect.pos, size: dvec2(1.0, rect.size.y) },
            Rect {
                pos: dvec2(rect.pos.x + rect.size.x - 1.0, rect.pos.y),
                size: dvec2(1.0, rect.size.y),
            },
        ] {
            self.draw_box(cx, edge, white);
        }
    }

    fn draw_slot_guides(
        &mut self,
        cx: &mut Cx2d,
        rect: Rect,
        row: SplatRowView,
        hovered: SplatPart,
    ) {
        // The slot under the pointer: a clear lift plus a frame in the row
        // colour, so the six targets read at a glance before the click.
        let slot = slot_rect(rect, hovered);
        let color = row.color();
        self.draw_box(cx, slot, vec4(1.0, 1.0, 1.0, 0.16));
        let frame = vec4(color[0], color[1], color[2], 0.9);
        for edge in [
            Rect { pos: slot.pos, size: dvec2(slot.size.x, 1.0) },
            Rect { pos: dvec2(slot.pos.x, slot.pos.y + slot.size.y - 1.0), size: dvec2(slot.size.x, 1.0) },
            Rect { pos: slot.pos, size: dvec2(1.0, slot.size.y) },
            Rect { pos: dvec2(slot.pos.x + slot.size.x - 1.0, slot.pos.y), size: dvec2(1.0, slot.size.y) },
        ] {
            self.draw_box(cx, edge, frame);
        }
        let area = slot_area(rect);
        let divider = vec4(color[0], color[1], color[2], 0.5);
        let half_y = area.pos.y + area.size.y * 0.5;
        self.draw_box(
            cx,
            Rect {
                pos: dvec2(area.pos.x, half_y - 0.5),
                size: dvec2(area.size.x, 1.0),
            },
            divider,
        );
        self.draw_box(
            cx,
            Rect {
                pos: dvec2(area.pos.x + area.size.x * 0.5 - 0.5, area.pos.y),
                size: dvec2(1.0, area.size.y * 0.5),
            },
            divider,
        );
        for quarter in 1..4 {
            self.draw_box(
                cx,
                Rect {
                    pos: dvec2(
                        area.pos.x + area.size.x * quarter as f64 * 0.25 - 0.5,
                        half_y,
                    ),
                    size: dvec2(1.0, area.size.y * 0.5),
                },
                divider,
            );
        }
    }
}

impl VjLoopSplatRef {
    pub fn splat_action(&self, actions: &Actions) -> LoopSplatAction {
        actions.find_widget_action_cast(self.widget_uid())
    }
}

impl Widget for VjLoopSplat {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.anim_frame.is_event(event).is_some() && self.animates() {
            self.area.redraw(cx);
            self.anim_frame = cx.new_next_frame();
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe)
                if fe.is_primary_hit()
                    || fe.mouse_button().is_some_and(|button| button.is_secondary()) =>
            {
                let secondary = fe.mouse_button().is_some_and(|button| button.is_secondary());
                self.pressed = self.hit_at(cx, fe.abs).and_then(|hit| match (secondary, hit) {
                    (true, hit @ SplatHit::Cell { .. }) => Some(hit),
                    (true, _) => None,
                    (false, SplatHit::Cell { row, col, .. }) => Some(SplatHit::Cell {
                        row,
                        col,
                        part: SplatPart::WHOLE,
                    }),
                    (false, hit) => Some(hit),
                });
                if self.pressed.is_some() {
                    cx.set_key_focus(self.area);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerUp(fe)
                if fe.is_primary_hit()
                    || fe.mouse_button().is_some_and(|button| button.is_secondary()) =>
            {
                let secondary = fe.mouse_button().is_some_and(|button| button.is_secondary());
                let released = self.hit_at(cx, fe.abs).and_then(|hit| match (secondary, hit) {
                    (true, hit @ SplatHit::Cell { .. }) => Some(hit),
                    (true, _) => None,
                    (false, SplatHit::Cell { row, col, .. }) => Some(SplatHit::Cell {
                        row,
                        col,
                        part: SplatPart::WHOLE,
                    }),
                    (false, hit) => Some(hit),
                });
                if fe.is_over && released == self.pressed {
                    if let Some(hit) = released {
                        // Shift makes a stop wait for the bar; a plain click is immediate.
                        self.emit(cx, hit, fe.modifiers.shift);
                    }
                }
                if self.pressed.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hover = self.hit_at(cx, fe.abs);
                if hover.is_some() {
                    cx.set_cursor(MouseCursor::Hand);
                }
                if self.hover != hover {
                    self.hover = hover;
                    self.area.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 16.0 || rect.size.y < 16.0 {
            return DrawStep::done();
        }
        let active = self.model.cols != 0;
        let cols = if active { self.model.cols } else { SPLAT_COLS };
        let geom = SplatGeometry::new(rect, cols);
        let now = cx.seconds_since_app_start() as f32;
        self.draw_cell
            .draw_vars
            .set_uniform(cx, live_id!(time), &[now]);
        match self.mix_pyramid.as_ref() {
            Some(pyramid) => {
                self.draw_cell.draw_vars.set_texture(0, &pyramid.texture);
                self.draw_cell.has_mix = 1.0;
            }
            None => {
                self.draw_cell.draw_vars.empty_texture(0);
                self.draw_cell.has_mix = 0.0;
            }
        }
        match self.stem_pyramid.as_ref() {
            Some(pyramid) => {
                self.draw_cell.draw_vars.set_texture(1, &pyramid.texture);
                self.draw_cell.has_stems = 1.0;
            }
            None => {
                self.draw_cell.draw_vars.empty_texture(1);
                self.draw_cell.has_stems = 0.0;
            }
        }
        if self.animates() {
            self.anim_frame = cx.new_next_frame();
        }

        let selected = Vec4f::from_u32(0xff5c39ff);
        let idle = Vec4f::from_u32(0x202730ff);
        let hover = Vec4f::from_u32(0x313b47ff);
        let dim_text = Vec4f::from_u32(0x8995a2ff);
        for row in 0..SPLAT_ROWS {
            let row_view = SplatRowView::ALL[row];
            let rc = row_view.color();
            let label_rect = Rect {
                pos: dvec2(rect.pos.x + PAD, geom.grid.pos.y + row as f64 * geom.cell_h),
                size: dvec2(ROW_HEAD_W - 20.0, geom.cell_h),
            };
            self.draw_centered(cx, label_rect, row_view.label(), vec4(rc[0], rc[1], rc[2], if active { 1.0 } else { 0.55 }), true);
            let stop = geom.row_stop(row);
            let stop_hot = self.hover == Some(SplatHit::StopRow(row));
            // The row's stop button lights while that row has a loop running.
            let row_live = active
                && self.model.cells[row].iter().any(|cell| {
                    matches!(cell, SplatCellView::Playing { .. })
                });
            self.draw_box(cx, stop, if row_live { selected } else if stop_hot { hover } else { idle });
            self.draw_centered(cx, stop, "■", vec4(rc[0], rc[1], rc[2], if active { 1.0 } else { 0.35 }), true);
        }

        // Every pad is one instance of the same textured material. Level
        // selection and source span vary per instance; the textures do not.
        self.draw_cell.begin_many_instances(cx);
        for row in 0..SPLAT_ROWS {
            let row_view = SplatRowView::ALL[row];
            for col in 0..cols {
                let cell = if active {
                    self.model.cells[row][col]
                } else {
                    SplatCellView::Empty
                };
                self.draw_cell_at(
                    cx,
                    geom.cell_rect(row, col),
                    row_view,
                    cell,
                    self.hovered_part(row, col).is_some(),
                    self.model.enabled || !active,
                    if active { self.model.spans[row][col] } else { (0.0, 0.0) },
                    active && self.model.blocks[row][col].is_some(),
                );
            }
        }
        self.draw_cell.end_many_instances(cx);
        self.draw_block.begin_many_instances(cx);
        if active {
            for row in 0..SPLAT_ROWS {
                let row_view = SplatRowView::ALL[row];
                for col in 0..cols {
                    if let Some(blocks) = self.model.blocks[row][col].clone() {
                        self.draw_blocks_at(
                            cx,
                            geom.cell_rect(row, col),
                            row_view,
                            self.model.cells[row][col],
                            &blocks,
                        );
                    }
                }
            }
        }
        self.draw_block.end_many_instances(cx);
        if active {
            for row in 0..SPLAT_ROWS {
                for col in 0..cols {
                    let Some(part) = self.hovered_part(row, col) else { continue };
                    if matches!(
                        self.model.cells[row][col],
                        SplatCellView::Ready { .. }
                            | SplatCellView::Queued { .. }
                            | SplatCellView::Playing { .. }
                    ) {
                        self.draw_slot_guides(
                            cx,
                            geom.cell_rect(row, col),
                            SplatRowView::ALL[row],
                            part,
                        );
                    }
                }
            }
        }
        // State tags: what a cell is about to do, in words, at its top-left.
        if active {
            for row in 0..SPLAT_ROWS {
                for col in 0..cols {
                    let SplatCellView::Queued { part, .. } = self.model.cells[row][col] else { continue };
                    let (tag, color) = ("NEXT", Vec4f::from_u32(0xffffffff));
                    let cell = geom.part_rect(row, col, part);
                    let tag_rect = Rect {
                        pos: dvec2(cell.pos.x + 2.0, cell.pos.y + 2.0),
                        size: dvec2((cell.size.x - 4.0).clamp(1.0, 30.0), 10.0),
                    };
                    self.draw_box(cx, tag_rect, Vec4f::from_u32(0x000000aa));
                    self.draw_centered(cx, tag_rect, tag, color, true);
                }
            }
        }
        if let Some((row, col, phase)) = self.model.preview {
            if active && row < SPLAT_ROWS && col < cols {
                let rect = geom.cell_rect(row, col);
                let inset = 2.0;
                let width = (rect.size.x - inset * 2.0).max(1.0);
                let x = rect.pos.x + inset + (width - 1.0) * phase.clamp(0.0, 1.0) as f64;
                self.draw_box(
                    cx,
                    Rect {
                        pos: dvec2(x, rect.pos.y + inset),
                        size: dvec2(1.0, (rect.size.y - inset * 2.0).max(1.0)),
                    },
                    Vec4f::from_u32(0xfffffff2),
                );
            }
        }
        if let Some((row, col)) = self.selected {
            if active && (col as usize) < cols {
                let row = SplatRowView::ALL.iter().position(|item| *item == row).unwrap_or(0);
                let rect = match self.model.cells[row][col as usize] {
                    SplatCellView::Queued { part, .. }
                    | SplatCellView::Playing { part, .. } => {
                        geom.part_rect(row, col as usize, part)
                    }
                    _ => geom.cell_rect(row, col as usize),
                };
                self.draw_selection(cx, rect);
            }
        }

        if active {
            for col in 0..cols {
                let head = Rect {
                    pos: dvec2(geom.grid.pos.x + col as f64 * geom.col_w() + CELL_INSET, geom.col_head_y),
                    size: dvec2((geom.col_w() - CELL_INSET * 2.0).max(1.0), COL_HEAD_H - 2.0),
                };
                let title = format!("{} · {}", col + 1, self.model.col_bars[col]);
                self.draw_centered(cx, head, &title, dim_text, true);

                let launch = geom.launch(col);
                let launch_hot = self.hover == Some(SplatHit::LaunchColumn(col));
                // Lit while any stem row of this section is running or queued:
                // the same button then stops the section.
                let col_live = (0..SPLAT_ROWS - 1).any(|row| {
                    matches!(
                        self.model.cells[row][col],
                        SplatCellView::Playing { .. } | SplatCellView::Queued { .. }
                    )
                });
                self.draw_box(cx, launch, if col_live { selected } else if launch_hot { hover } else { idle });
                self.draw_centered(cx, launch, if col_live { "■" } else { "▶" }, Vec4f::from_u32(0xdce3eaff), true);
            }
        } else if self.model.status.is_none() {
            let message = "load a track on deck A or B";
            let message_rect = Rect {
                pos: geom.grid.pos,
                size: geom.grid.size,
            };
            self.draw_centered(cx, message_rect, message, Vec4f::from_u32(0xaab4beff), false);
        }
        if let Some((text, progress)) = self.model.status.clone() {
            // The loading overlay: what the deck is still doing, over the grid.
            let box_w = 360.0f64.min(geom.grid.size.x - 16.0).max(120.0);
            let box_h = 44.0;
            let panel = Rect {
                pos: dvec2(
                    geom.grid.pos.x + (geom.grid.size.x - box_w) * 0.5,
                    geom.grid.pos.y + (geom.grid.size.y - box_h) * 0.5,
                ),
                size: dvec2(box_w, box_h),
            };
            self.draw_box(cx, panel, Vec4f::from_u32(0x0f1318e6));
            let text_rect = Rect { pos: panel.pos + dvec2(0.0, 6.0), size: dvec2(box_w, 14.0) };
            self.draw_centered(cx, text_rect, &text, Vec4f::from_u32(0xdce3eaff), true);
            let track = Rect {
                pos: dvec2(panel.pos.x + 20.0, panel.pos.y + box_h - 14.0),
                size: dvec2(box_w - 40.0, 6.0),
            };
            self.draw_box(cx, track, Vec4f::from_u32(0x2a323cff));
            match progress {
                Some(p) => {
                    let fill = Rect { pos: track.pos, size: dvec2(track.size.x * p.clamp(0.0, 1.0) as f64, track.size.y) };
                    self.draw_box(cx, fill, selected);
                }
                None => {
                    // Indeterminate: a block sweeping back and forth.
                    let t = (now as f64 * 0.8).rem_euclid(2.0);
                    let u = if t < 1.0 { t } else { 2.0 - t };
                    let block_w = 60.0f64.min(track.size.x);
                    let fill = Rect {
                        pos: dvec2(track.pos.x + (track.size.x - block_w) * u, track.pos.y),
                        size: dvec2(block_w, track.size.y),
                    };
                    self.draw_box(cx, fill, selected);
                }
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn row_colours_are_distinct() {
        let colors: HashSet<[u32; 4]> = SplatRowView::ALL
            .iter()
            .map(|row| row.color().map(f32::to_bits))
            .collect();
        assert_eq!(colors.len(), SPLAT_ROWS);
    }

    #[test]
    fn empty_model_has_no_synthetic_content() {
        let model = SplatViewModel::empty(SplatDeck::B);
        assert_eq!(model.deck, SplatDeck::B);
        assert!(!model.enabled);
        assert_eq!(model.cols, 0);
        assert_eq!(model.col_bars, [0; SPLAT_COLS]);
        assert_eq!(model.spans, [[(0.0, 0.0); SPLAT_COLS]; SPLAT_ROWS]);
        assert_eq!(model.duration_secs, 0.0);
        assert!(model.blocks.iter().flatten().all(Option::is_none));
        assert_eq!(model.bar_phase, 0.0);
        assert_eq!(model.preview, None);
        assert!(model.cells.iter().flatten().all(|cell| *cell == SplatCellView::Empty));
    }

    #[test]
    fn model_block_equality_uses_arc_identity() {
        let mut left = SplatViewModel::empty(SplatDeck::A);
        let shared = Arc::new(CellBlocks { bars: 1, blocks: Vec::new() });
        left.blocks[0][0] = Some(shared.clone());
        let mut right = left.clone();
        assert_eq!(left, right);

        right.blocks[0][0] = Some(Arc::new((*shared).clone()));
        assert_ne!(left, right);
    }

    #[test]
    fn span_seconds_convert_to_finest_pyramid_columns() {
        assert_eq!(span_to_pyramid_columns((1.25, 2.5)), (125.0, 250.0));
        assert_eq!(span_to_pyramid_columns((3.0, 0.0)), (300.0, 0.0));
        assert_eq!(span_to_pyramid_columns((f32::NAN, 1.0)), (0.0, 0.0));
    }

    #[test]
    fn cell_hit_maps_edges_and_rejects_outside_points() {
        let rect = Rect { pos: dvec2(10.0, 20.0), size: dvec2(800.0, 250.0) };
        assert_eq!(cell_at(rect, dvec2(10.1, 20.1)), Some((0, 0)));
        assert_eq!(cell_at(rect, dvec2(409.0, 145.0)), Some((2, 3)));
        assert_eq!(cell_at(rect, dvec2(809.9, 269.9)), Some((4, 7)));
        assert_eq!(cell_at(rect, dvec2(9.9, 20.0)), None);
        assert_eq!(cell_at(rect, dvec2(810.0, 270.0)), None);
    }

    #[test]
    fn subloop_hit_maps_all_six_slots_and_cell_edges() {
        let rect = Rect { pos: dvec2(10.0, 20.0), size: dvec2(120.0, 80.0) };
        let parts = [
            SplatPart { num: 0, den: 2 },
            SplatPart { num: 1, den: 2 },
            SplatPart { num: 0, den: 4 },
            SplatPart { num: 1, den: 4 },
            SplatPart { num: 2, den: 4 },
            SplatPart { num: 3, den: 4 },
        ];
        for part in parts {
            let slot = slot_rect(rect, part);
            assert_eq!(
                part_at(slot_area(rect), slot.pos + slot.size * 0.5),
                Some(part)
            );
        }
        assert_eq!(
            part_at(rect, rect.pos + dvec2(0.001, 0.001)),
            Some(SplatPart { num: 0, den: 2 })
        );
        assert_eq!(
            part_at(rect, rect.pos + rect.size - dvec2(0.001, 0.001)),
            Some(SplatPart { num: 3, den: 4 })
        );
        assert_eq!(
            part_at(rect, dvec2(rect.pos.x + rect.size.x * 0.5, rect.pos.y + 0.001)),
            Some(SplatPart { num: 1, den: 2 })
        );
        assert_eq!(
            part_at(rect, dvec2(rect.pos.x + 0.001, rect.pos.y + rect.size.y * 0.5)),
            Some(SplatPart { num: 0, den: 4 })
        );
    }

    #[test]
    fn subloop_slot_rectangles_tile_without_overlap() {
        let rect = Rect { pos: dvec2(4.0, 7.0), size: dvec2(100.0, 60.0) };
        let parts = [
            SplatPart { num: 0, den: 2 },
            SplatPart { num: 1, den: 2 },
            SplatPart { num: 0, den: 4 },
            SplatPart { num: 1, den: 4 },
            SplatPart { num: 2, den: 4 },
            SplatPart { num: 3, den: 4 },
        ];
        let slots = parts.map(|part| slot_rect(rect, part));
        for (index, left) in slots.iter().enumerate() {
            for right in &slots[index + 1..] {
                let overlap_w = (left.pos.x + left.size.x).min(right.pos.x + right.size.x)
                    - left.pos.x.max(right.pos.x);
                let overlap_h = (left.pos.y + left.size.y).min(right.pos.y + right.size.y)
                    - left.pos.y.max(right.pos.y);
                assert!(overlap_w <= 0.0 || overlap_h <= 0.0, "{left:?} overlaps {right:?}");
            }
        }
        let tiled_area: f64 = slots.iter().map(|slot| slot.size.x * slot.size.y).sum();
        let area = slot_area(rect);
        assert!((tiled_area - area.size.x * area.size.y).abs() < 1e-9);
    }
}
