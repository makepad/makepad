//! The AI music page's song list: the same table as a playlist's
//! (`TrackTable`, widgets.rs): the same grid, palette, row height, hover
//! tint, selection and playing-row highlight. Its columns are the AI
//! songs' own: # / Title / Style / Model / Created / Time. The rows are the
//! AI list in play order (oldest first), then the songs being made: their
//! status (step, percentage, elapsed time) and a progress line stand where
//! a made song has its style. Double-click or
//! Return plays the list from that row; a press picks the row (the side
//! panel shows its lyrics). Registered in widgets.rs, beside `TrackTable`.

use crate::widgets::{hover_tint, round, style_grid, text_fit, text_right, DrawMusicRound, ICON_PAUSE, ICON_SPEAKER};
use crate::model::Model;
use crate::with_alpha;
use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AiColumn {
    Number,
    Title,
    /// The style tags or prompt the song was made with.
    Style,
    Model,
    Created,
    Time,
}

const ALL: [AiColumn; 6] = [AiColumn::Number, AiColumn::Title, AiColumn::Style, AiColumn::Model, AiColumn::Created, AiColumn::Time];

impl GridColumn for AiColumn {
    fn code(self) -> &'static str {
        match self {
            AiColumn::Number => "number",
            AiColumn::Title => "title",
            AiColumn::Style => "style",
            AiColumn::Model => "model",
            AiColumn::Created => "created",
            AiColumn::Time => "time",
        }
    }

    fn label(self) -> &'static str {
        match self {
            AiColumn::Number => "#",
            AiColumn::Title => "Title",
            AiColumn::Style => "Style",
            AiColumn::Model => "Model",
            AiColumn::Created => "Created",
            AiColumn::Time => "Time",
        }
    }

    /// As the playlist's: a fixed number and figures, the text columns
    /// sharing the rest. A narrow table drops Time, Created, Model first.
    fn width(self) -> ColumnWidth {
        match self {
            AiColumn::Number => ColumnWidth::Fixed(56.0),
            AiColumn::Title => ColumnWidth::Flex { share: 0.5, min: 140.0, max: f64::MAX },
            AiColumn::Style => ColumnWidth::Flex { share: 0.5, min: 140.0, max: f64::MAX },
            AiColumn::Model => ColumnWidth::Fixed(84.0),
            AiColumn::Created => ColumnWidth::Fixed(112.0),
            AiColumn::Time => ColumnWidth::Fixed(64.0),
        }
    }

    fn hideable(self) -> bool {
        self != AiColumn::Title
    }

    fn sortable(self) -> bool {
        false
    }

    fn movable(self) -> bool {
        false
    }
}

/// One row as shown.
#[derive(Clone, Debug, PartialEq)]
pub struct AiTableRow {
    pub num: String,
    pub title: String,
    pub style: String,
    pub model: String,
    pub created: Option<String>,
    pub time: Option<String>,
    /// A song being made: its step, percentage and elapsed time, shown in
    /// the Style column over its progress line.
    pub status: String,
    /// Its progress, 0..1 (a song being made).
    pub progress: Option<f64>,
    /// The song playing (or paused).
    pub current: bool,
    /// A song being made: it reads faint and cannot play yet.
    pub running: bool,
}

#[derive(Clone, Debug, Default)]
pub enum AiTableAction {
    #[default]
    None,
    /// A row was double-clicked or Return was pressed on it.
    Activated(usize),
    /// A row was picked (pressed, or moved to by the keys).
    Picked(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiTable {
    #[deref]
    view: View,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_icon: DrawText,
    #[live]
    draw_round: DrawMusicRound,
    #[rust]
    rows: Vec<AiTableRow>,
    #[rust]
    theme_applied: Option<(Vec4f, u64)>,
    #[rust]
    fit: GridColumnFit<AiColumn>,
    #[rust(GridColumns::new(&ALL, &[], ALL.to_vec(), None, SortCycle::FlipThenNone))]
    layout: GridColumns<AiColumn>,
    #[rust]
    picked: Option<usize>,
    #[rust]
    hover_any: Option<usize>,
}

impl Widget for AiTable {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(model) = scope.data_mut().get::<Model>() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        if self.theme_applied != Some((model.theme.background, model.style_gen)) {
            self.theme_applied = Some((model.theme.background, model.style_gen));
            let mut grid = self.view.widget(cx, ids!(grid));
            style_grid(cx, &mut grid, &model.theme, model.theme.background);
        }
        let width = cx.peek_walk_turtle(walk).size.x;
        let reconfigure = self.fit.fit(width, &self.layout);
        let columns = self.fit.columns().to_vec();
        let playing = model.playback.is_playing();
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            if reconfigure {
                self.fit.configure(cx, &mut grid, self.rows.len(), width, &self.layout);
            }
            grid.set_grid_size(self.rows.len(), columns.len());
            if self.picked.is_some_and(|row| row >= self.rows.len()) {
                self.picked = None;
                grid.set_selection(cx, None);
            }
            while let Some(cell) = grid.next_cell(cx) {
                let Some(row) = self.rows.get(cell.row) else { continue };
                let Some(&column) = columns.get(cell.col) else { continue };
                let theme = if model.theme.row_invert && self.picked == Some(cell.row) {
                    grid.cell_bg(cx, &cell, model.theme.foreground);
                    model.theme.inverted()
                } else {
                    model.theme
                };
                if self.hover_any == Some(cell.row) {
                    grid.cell_bg(cx, &cell, hover_tint(&theme));
                }
                let faint = |color: Vec4f| if row.running { with_alpha(color, 0.5) } else { color };
                let mid = cell.rect.pos.y + cell.rect.size.y * 0.5;
                let x = cell.rect.pos.x + 10.0;
                let room = cell.rect.size.x - 16.0;
                let right = cell.rect.pos.x + cell.rect.size.x - 14.0;
                // A figure nobody knows yet reads as a quiet dash.
                let figure = |draw: &mut DrawText, cx: &mut Cx2d, text: &Option<String>| match text {
                    Some(text) => {
                        draw.color = faint(theme.muted);
                        text_right(draw, cx, right, mid, text);
                    }
                    None => {
                        draw.color = with_alpha(theme.muted, theme.muted.w * 0.4);
                        text_right(draw, cx, right, mid, "–");
                    }
                };
                match column {
                    AiColumn::Number => {
                        if row.current {
                            self.draw_icon.color = theme.accent;
                            text_right(&mut self.draw_icon, cx, cell.rect.pos.x + cell.rect.size.x - 12.0, mid, if playing { ICON_SPEAKER } else { ICON_PAUSE });
                        } else {
                            self.draw_small.color = faint(theme.muted);
                            text_right(&mut self.draw_small, cx, cell.rect.pos.x + cell.rect.size.x - 12.0, mid, &row.num);
                        }
                    }
                    AiColumn::Title => {
                        self.draw_title.color = if row.current { theme.accent } else { faint(theme.foreground) };
                        text_fit(&mut self.draw_title, cx, x, mid, room, &row.title);
                    }
                    // A song being made: what it is at, and a progress line under it.
                    AiColumn::Style => match row.progress {
                        Some(progress) => {
                            self.draw_small.color = theme.foreground;
                            text_fit(&mut self.draw_small, cx, x, mid - 4.0, room, &row.status);
                            let track = Rect { pos: dvec2(x, mid + 8.0), size: dvec2(room.max(0.0), 2.0) };
                            round(&mut self.draw_round, cx, track, with_alpha(theme.muted, 0.25), 1.0);
                            let done = Rect { pos: track.pos, size: dvec2((room * progress.clamp(0.0, 1.0)).max(2.0), 2.0) };
                            round(&mut self.draw_round, cx, done, theme.accent, 1.0);
                        }
                        None => {
                            self.draw_small.color = faint(theme.muted);
                            text_fit(&mut self.draw_small, cx, x, mid, room, &row.style);
                        }
                    },
                    AiColumn::Model => {
                        self.draw_small.color = faint(theme.muted);
                        text_fit(&mut self.draw_small, cx, x, mid, room, &row.model);
                    }
                    AiColumn::Created => figure(&mut self.draw_small, cx, &row.created),
                    AiColumn::Time => figure(&mut self.draw_small, cx, &row.time),
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.view.handle_event(cx, event, &mut Scope::empty());
        let uid = self.widget_uid();
        let grid = self.view.data_grid(cx, ids!(grid));
        crate::widgets::track_hover(cx, &grid, event, &mut self.hover_any);
        let Event::Actions(actions) = event else { return };
        for action in grid.actions(actions) {
            match action {
                DataGridAction::SelectionChanged { selection } => {
                    self.picked = selection.map(|selection| selection.head.0);
                    if let Some(row) = self.picked {
                        cx.widget_action(uid, AiTableAction::Picked(row));
                    }
                }
                // Return and a double-click both ask to "edit" a cell; in a
                // read-only list that is the row being chosen.
                DataGridAction::EditCell { row, replace: None, .. } => {
                    cx.widget_action(uid, AiTableAction::Activated(row));
                }
                DataGridAction::ColumnResized { col, width, .. } => {
                    self.layout.set_width(&self.fit, col, width);
                }
                _ => {}
            }
        }
    }
}

impl AiTableRef {
    /// The rows, in play order. Redrawn only when they changed.
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<AiTableRow>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.rows == rows {
            return;
        }
        inner.rows = rows;
        inner.view.redraw(cx);
    }

    pub fn activated(&self, actions: &Actions) -> Option<usize> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            AiTableAction::Activated(row) => Some(row),
            _ => None,
        }
    }

    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            AiTableAction::Picked(row) => Some(row),
            _ => None,
        }
    }
}
