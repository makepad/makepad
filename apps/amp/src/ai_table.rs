//! The AI music page's song list: the same table as a playlist's
//! (`TrackTable`, widgets.rs): the same grid, palette, row height, hover
//! tint, selection and playing-row highlight. Its columns are the AI
//! songs' own: # / Title / Style / Model / Created / Time. The host hands
//! the rows in play order (# is that place), and the songs being made after
//! them: their status (step, percentage, elapsed time) and a progress line
//! stand where a made song has its style.
//!
//! The table shows them sorted: newest first (Created falling) until a
//! heading is pressed; each press on a heading turns its sort around. The
//! songs being made read as the newest. The sort is the table's own (the
//! host saves [`AiTableRef::layout_text`]); play order never follows it.
//! Every action names rows in the host's order. Double-click or Return
//! plays the list from that row; a press picks the row (the side panel
//! shows its lyrics); a press that travels drags the song out of the window
//! (drag_out.rs). Registered in widgets.rs, beside `TrackTable`.

use crate::columns::{compare, Key};
use makepad_search::fold_words as fold;
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

    /// Figures read biggest (newest, longest) first.
    fn descending_first(self) -> bool {
        matches!(self, AiColumn::Created | AiColumn::Time)
    }

    fn movable(self) -> bool {
        false
    }
}

/// One row as shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AiTableRow {
    pub num: String,
    pub title: String,
    pub style: String,
    pub model: String,
    pub created: Option<String>,
    /// When it was made (Unix seconds): what Created sorts by.
    pub created_secs: Option<i64>,
    /// Its length in seconds (0: not known): what Time sorts by.
    pub length: f64,
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
    /// A press on a row travelled: drag these rows (the picked run it began
    /// on, else that row) out of this window.
    DragOut(Vec<usize>, WindowId),
    /// The sort or a column's width changed: save [`AiTableRef::layout_text`].
    LayoutChanged,
}

/// The newest first until a heading is pressed.
pub fn default_layout() -> GridColumns<AiColumn> {
    GridColumns::new(&ALL, &[], ALL.to_vec(), Some(GridSort { column: AiColumn::Created, descending: true }), SortCycle::Flip)
}

/// The order rows show in (indices into `rows`): the made songs by `sort`,
/// ties in play order; the songs being made first when the sort puts the
/// newest first (Created or # falling), else last.
pub fn shown_order(rows: &[AiTableRow], sort: Option<GridSort<AiColumn>>) -> Vec<usize> {
    let (mut made, running): (Vec<usize>, Vec<usize>) = (0..rows.len()).partition(|&at| !rows[at].running);
    let Some(sort) = sort else { return made.into_iter().chain(running).collect() };
    let text = |at: usize| -> String {
        let row = &rows[at];
        fold(match sort.column {
            AiColumn::Title => &row.title,
            AiColumn::Style => &row.style,
            _ => &row.model,
        })
    };
    let figure = |at: usize| -> Key<'static> {
        let row = &rows[at];
        match sort.column {
            AiColumn::Created => Key::Known(row.created_secs.unwrap_or(0).max(0) as u64),
            AiColumn::Time => Key::Known((row.length.max(0.0) * 1000.0) as u64),
            _ => Key::Known(at as u64 + 1),
        }
    };
    match sort.column {
        AiColumn::Title | AiColumn::Style | AiColumn::Model => {
            let keys: Vec<String> = (0..rows.len()).map(|at| if rows[at].running { String::new() } else { text(at) }).collect();
            made.sort_by(|&a, &b| compare(&Key::Text(&keys[a]), &Key::Text(&keys[b]), sort.descending, a.cmp(&b)));
        }
        _ => made.sort_by(|&a, &b| compare(&figure(a), &figure(b), sort.descending, a.cmp(&b))),
    }
    let newest_first = sort.descending && matches!(sort.column, AiColumn::Created | AiColumn::Number);
    if newest_first {
        running.into_iter().rev().chain(made).collect()
    } else {
        made.into_iter().chain(running).collect()
    }
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
    /// In the host's order.
    #[rust]
    rows: Vec<AiTableRow>,
    /// The rows as shown: `shown[display row]` is an index into `rows`.
    #[rust]
    shown: Vec<usize>,
    #[rust]
    theme_applied: Option<(Vec4f, u64)>,
    #[rust]
    fit: GridColumnFit<AiColumn>,
    #[rust(default_layout())]
    layout: GridColumns<AiColumn>,
    /// The picked display row.
    #[rust]
    picked: Option<usize>,
    /// The picked run of display rows (Shift picks several).
    #[rust]
    picked_rows: Option<std::ops::Range<usize>>,
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
        let sort = self.fit.sort_indicator(&self.layout);
        let playing = model.playback.is_playing();
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            if reconfigure {
                self.fit.configure(cx, &mut grid, self.rows.len(), width, &self.layout);
            }
            grid.set_grid_size(self.rows.len(), columns.len());
            grid.set_sort_indicator(sort);
            if self.picked.is_some_and(|row| row >= self.rows.len()) {
                self.picked = None;
                grid.set_selection(cx, None);
            }
            while let Some(cell) = grid.next_cell(cx) {
                let Some(row) = self.shown.get(cell.row).and_then(|&at| self.rows.get(at)) else { continue };
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
                    self.picked_rows = selection.map(|selection| {
                        let (first, last) = selection.row_range();
                        first..last + 1
                    });
                    for action in row_actions(&[DataGridAction::SelectionChanged { selection }], &self.shown) {
                        cx.widget_action(uid, action);
                    }
                }
                DataGridAction::EditCell { row, col, replace: None } => {
                    for action in row_actions(&[DataGridAction::EditCell { row, col, replace: None }], &self.shown) {
                        cx.widget_action(uid, action);
                    }
                }
                DataGridAction::RowDragStarted { row, .. } => {
                    let display: Vec<usize> = match &self.picked_rows {
                        Some(range) if range.contains(&row) => range.clone().collect(),
                        _ => vec![row],
                    };
                    let rows: Vec<usize> = display.iter().filter_map(|d| self.shown.get(*d).copied()).collect();
                    if let (false, Some(window)) = (rows.is_empty(), cx.get_window_id_of(&grid.area())) {
                        cx.widget_action(uid, AiTableAction::DragOut(rows, window));
                    }
                }
                DataGridAction::HeaderClicked { col, .. } => {
                    if let Some(column) = self.fit.column(col) {
                        if self.layout.press_heading(column) {
                            self.reorder(cx);
                            cx.widget_action(uid, AiTableAction::LayoutChanged);
                        }
                    }
                }
                DataGridAction::ColumnResized { col, width, .. } => {
                    if self.layout.set_width(&self.fit, col, width) {
                        cx.widget_action(uid, AiTableAction::LayoutChanged);
                    }
                }
                _ => {}
            }
        }
    }
}

impl AiTable {
    /// Sort the rows again; the picked song stays picked where it went.
    fn reorder(&mut self, cx: &mut Cx) {
        let picked = self.picked.and_then(|row| self.shown.get(row).copied());
        self.shown = shown_order(&self.rows, self.layout.sort());
        let grid = self.view.data_grid(cx, ids!(grid));
        if let Some(at) = picked {
            self.picked = self.shown.iter().position(|&s| s == at);
            if let Some(mut grid) = grid.borrow_mut() {
                let selection = self.picked.map(|row| GridSelection::single(row, 0));
                grid.set_selection(cx, selection);
            }
        }
        self.view.redraw(cx);
    }
}

/// What the grid's picks and requests mean for the host, in its own row
/// order (`shown[display row]`): a pick picks that song, Return or a
/// double-click (both ask to "edit" a cell; in a read-only list that is the
/// row being chosen) plays it.
pub fn row_actions(grid: &[DataGridAction], shown: &[usize]) -> Vec<AiTableAction> {
    grid.iter()
        .filter_map(|action| match action {
            DataGridAction::SelectionChanged { selection: Some(selection) } => shown.get(selection.head.0).map(|&at| AiTableAction::Picked(at)),
            DataGridAction::EditCell { row, replace: None, .. } => shown.get(*row).map(|&at| AiTableAction::Activated(at)),
            _ => None,
        })
        .collect()
}

/// The row a batch of the table's actions asks to play, wherever it stands
/// among them.
pub fn activated_of(actions: impl IntoIterator<Item = AiTableAction>) -> Option<usize> {
    actions.into_iter().find_map(|a| if let AiTableAction::Activated(row) = a { Some(row) } else { None })
}

/// The first action of `kind` this table raised in a batch: one event can
/// raise several (the second press of a double-click picks the row again
/// before it asks to play it).
fn find<T>(actions: &Actions, uid: WidgetUid, kind: impl Fn(AiTableAction) -> Option<T>) -> Option<T> {
    actions.filter_widget_actions_cast::<AiTableAction>(uid).find_map(kind)
}

impl AiTableRef {
    /// The rows, in play order, then the songs being made. Redrawn only
    /// when they changed.
    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<AiTableRow>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.rows == rows {
            return;
        }
        inner.rows = rows;
        inner.reorder(cx);
    }

    /// The columns and the sort, for the settings.
    pub fn layout_text(&self) -> String {
        self.borrow().map(|inner| inner.layout.serialize()).unwrap_or_default()
    }

    /// Read back what [`Self::layout_text`] gave.
    pub fn restore_layout(&self, cx: &mut Cx, text: &str) {
        let Some(mut inner) = self.borrow_mut() else { return };
        if inner.layout.parse(text) {
            inner.fit = GridColumnFit::default();
            inner.reorder(cx);
        }
    }

    pub fn activated(&self, actions: &Actions) -> Option<usize> {
        activated_of(actions.filter_widget_actions_cast::<AiTableAction>(self.widget_uid()))
    }

    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        find(actions, self.widget_uid(), |a| if let AiTableAction::Picked(row) = a { Some(row) } else { None })
    }

    pub fn drag_out(&self, actions: &Actions) -> Option<(Vec<usize>, WindowId)> {
        find(actions, self.widget_uid(), |a| if let AiTableAction::DragOut(rows, window) = a { Some((rows, window)) } else { None })
    }

    pub fn layout_changed(&self, actions: &Actions) -> bool {
        find(actions, self.widget_uid(), |a| matches!(a, AiTableAction::LayoutChanged).then_some(())).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::QueueItem;

    fn row(title: &str, created: i64, length: f64) -> AiTableRow {
        AiTableRow { title: title.into(), created_secs: Some(created), length, ..AiTableRow::default() }
    }

    fn making() -> AiTableRow {
        AiTableRow { title: "New song".into(), running: true, ..AiTableRow::default() }
    }

    /// Newest first until a heading is pressed; each press turns it around;
    /// Created sorts by the time, not by its text ("9 Oct" after "26 Oct");
    /// the songs being made read as the newest.
    #[test]
    fn the_newest_song_is_on_top_and_headings_turn_the_sort() {
        let rows = vec![row("b", 1_760_000_000, 90.0), row("a", 1_761_000_000, 30.0), row("c", 1_759_000_000, 60.0), making()];
        let mut layout = default_layout();
        assert_eq!(shown_order(&rows, layout.sort()), [3, 1, 0, 2]);
        layout.press_heading(AiColumn::Created);
        assert_eq!(shown_order(&rows, layout.sort()), [2, 0, 1, 3]);
        layout.press_heading(AiColumn::Created);
        assert_eq!(shown_order(&rows, layout.sort()), [3, 1, 0, 2]);
        layout.press_heading(AiColumn::Title);
        assert_eq!(shown_order(&rows, layout.sort()), [1, 0, 2, 3]);
        layout.press_heading(AiColumn::Title);
        assert_eq!(shown_order(&rows, layout.sort()), [2, 0, 1, 3]);
        layout.press_heading(AiColumn::Time);
        assert_eq!(shown_order(&rows, layout.sort()), [0, 2, 1, 3]);
        // Kept with the layout and read back.
        let mut back = default_layout();
        assert!(back.parse(&layout.serialize()));
        assert_eq!(back.sort(), layout.sort());
    }

    /// A double-click's second press makes the grid pick the row again and
    /// then ask to edit it, in one batch: the table says Picked, then
    /// Activated, and the Activated row is the song under the pointer in
    /// play order, whatever the sort. Playing it starts that song.
    #[test]
    fn a_double_click_plays_the_song_under_the_pointer_after_a_sort() {
        let rows = vec![row("first", 1_760_000_000, 60.0), row("second", 1_761_000_000, 60.0), row("third", 1_762_000_000, 60.0)];
        // Newest first: display row 0 is "third" (play order 2).
        let shown = shown_order(&rows, default_layout().sort());
        assert_eq!(shown, [2, 1, 0]);
        let press = |display: usize| {
            vec![
                DataGridAction::CellClicked { row: display, col: 1, modifiers: KeyModifiers::default() },
                DataGridAction::SelectionChanged { selection: Some(GridSelection::single(display, 1)) },
            ]
        };
        let mut second = press(2);
        second.push(DataGridAction::CellDoubleClicked { row: 2, col: 1 });
        second.push(DataGridAction::EditCell { row: 2, col: 1, replace: None });
        assert_eq!(activated_of(row_actions(&press(2), &shown)), None, "one press only picks");
        let batch = row_actions(&second, &shown);
        assert!(matches!(batch[0], AiTableAction::Picked(0)), "the pick comes first in the batch");
        let at = activated_of(batch).expect("the double-click plays");
        assert_eq!(at, 0, "display row 2 is the oldest song");
        // The AI list plays from that song, in play order: the player is
        // asked to play its file.
        use crate::model::tests::{item, mixed_model, player_plays};
        // (The test library has the files mine.ogg and kept.ogg.)
        let files = ["mine", "kept", "kept"];
        let items: Vec<QueueItem> = rows.iter().zip(files).map(|(r, file)| item(&format!("file:///m/{file}.ogg"), &r.title)).collect();
        let mut model = mixed_model();
        model.ai_play(items, at);
        assert_eq!(player_plays(&mut model), ["/m/mine.ogg"], "the oldest song, \"first\", plays");
        let next: Vec<&str> = model.queue.upcoming().map(|(_, i)| i.name.as_str()).collect();
        assert_eq!(next, ["second", "third"]);
    }
}
