//! The data grid stories: a grid that draws cells the host hands it, one at
//! a time, and holds none of them, and a host that says what may be written
//! into them; then the same grid shaped as a list whose picks the host keeps.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::collections::BTreeSet;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryDataGridBase = #(StoryDataGrid::register_widget(vm))

    mod.storybook.StoryDataGrid = set_type_default() do mod.storybook.StoryDataGridBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        View{
            width: Fill height: 300.
            grid := DataGrid{
                width: Fill
                height: Fill
                // Zero, not the real count: set_grid_size returns early when
                // the numbers already match, and that early return skips the
                // geometry pass the first draw needs. Every caller in this
                // repository declares zero here for the same reason.
                rows: 0
                cols: 3
                // On, so the page shows what a sortable heading looks like
                // before it is sorted as well as after.
                sortable: true
                default_col_width: 150.0
                default_row_height: 24.0
                allow_col_reorder: true

                // The stock grid is a light-mode spreadsheet: every surface
                // it paints is a literal colour, so all of this has to be
                // restated or the page turns up white in a dark theme. This
                // block is what the section about it is pointing at.
                color_bg: theme.color_surface_container_low
                color_cell: theme.color_surface
                color_cell_alt: theme.color_surface_container
                color_text: theme.color_text
                color_header: theme.color_surface_container
                color_header_active: theme.color_surface_container_high
                color_header_text: theme.color_text_meta
                color_selection: theme.color_selection
                color_selection_border: theme.color_bevel_focus
                color_drag_marker: theme.color_bevel_focus
                color_resize_guide: theme.color_bevel_focus
                // The gridline lives on the cell shader, not in the colour
                // list, and is the one that is easiest to leave behind.
                draw_cell +: {border_color: uniform(theme.color_bevel)}
                draw_text +: {color: theme.color_text}
                draw_text_bold +: {color: theme.color_text}
            }
        }
        reported := Label{text: "nothing selected"}
        edited := Label{text: "nothing edited yet"}
    }

    mod.storybook.StoryDataGridListBase = #(StoryDataGridList::register_widget(vm))

    mod.storybook.StoryDataGridList = set_type_default() do mod.storybook.StoryDataGridListBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2

        // The picks and the cursor are this page's, so their colours are
        // too: the grid is never told a row is picked.
        color_picked: theme.color_secondary_container
        color_picked_text: theme.color_on_secondary_container
        color_cursor: theme.color_primary

        View{
            width: Fill height: 260.
            grid := DataGrid{
                width: Fill
                height: Fill
                rows: 0
                cols: 4
                // The grid selects nothing: no overlay, no rubber band. It
                // reports each press with the keys held, and this page
                // does the picking.
                selection: GridSelectMode.Off
                // A press that travels past drag_threshold carries its row
                // rather than being a click.
                row_drag: true
                show_row_headers: false
                allow_row_resize: false
                zebra_stripes: true
                default_col_width: 150.0
                default_row_height: 24.0

                // Restated from theme tokens, as on the Overview page.
                color_bg: theme.color_surface_container_low
                color_cell: theme.color_surface
                color_cell_alt: theme.color_surface_container
                color_text: theme.color_text
                color_header: theme.color_surface_container
                color_header_active: theme.color_surface_container_high
                color_header_text: theme.color_text_meta
                color_selection: theme.color_selection
                color_selection_border: theme.color_bevel_focus
                color_drag_marker: theme.color_bevel_focus
                color_resize_guide: theme.color_bevel_focus
                draw_cell +: {border_color: uniform(theme.color_bevel)}
                draw_text +: {color: theme.color_text}
                draw_text_bold +: {color: theme.color_text}
            }
        }
        picked := Label{text: "nothing picked"}
        log := Label{text: "press a row and let go, or drag it"}
    }

    mod.stories.DataGridOverview = StoryPage{
        StoryNote{text: "A spreadsheet-shaped table over a very large number of rows. Eleven places in this repository use one. It holds none of your data: it works out which cells are on screen and asks for them, one at a time, while you draw them."}

        StoryHeading{text: "Three columns of people"}
        StoryNote{text: "Click a cell, shift-click another for a rectangle, click a row number for the row, a column header for the column. Arrow keys move and shift extends. Drag a column edge to resize it, and a column header to move it somewhere else."}
        StoryNote{text: "Press a heading to sort by it: once for up, again for down, a third time back to the order the rows arrived in. The grid does not do this — it reports the press and this page reorders its own rows, which is the whole arrangement."}
        demo := mod.storybook.StoryDataGrid{}

        StoryHeading{text: "Editing"}
        StoryNote{text: "The grid seats its own editor. Type over a selected cell to replace what it says, or press F2, Return or double-click to amend it. Return keeps the value and steps down a row, shift-Return steps up, Tab steps sideways, Escape puts the cell back as it was, and a click anywhere else keeps the value where it stands."}

        StoryHeading{text: "The host says what may be written"}
        StoryNote{text: "The grid holds no data, so it cannot know what a cell says or whether a value is any good. It asks this page for the text to start from and hands the finished text back. This page keeps a name that is not blank and a year of four digits, and refuses the rest: a refused value is simply not written down, the cell redraws with what it had, and the line under the grid says why."}

        StoryHeading{text: "An edit survives a scroll"}
        StoryNote{text: "Start an edit, wheel the row off the bottom and back, and the text is still there. The grid keeps the live editor out of the pool its other cells are recycled through; a host that draws its own editor into a cell loses the text the moment the wheel moves."}
    }

    mod.stories.DataGridList = StoryPage{
        StoryNote{text: "The same grid shaped as a list: no row numbers, rows picked whole, and the picks kept by the page rather than by the grid."}

        StoryHeading{text: "Picks the host keeps"}
        StoryNote{text: "Click a row to pick it alone, Ctrl-click to add a row or drop it, Shift-click to pick everything from the row last clicked. The grid selects nothing here. It reports each press with the keys that were held, and this page decides what is picked, tints those rows and outlines the row clicked last."}
        list := mod.storybook.StoryDataGridList{}

        StoryHeading{text: "A click, or a carry"}
        StoryNote{text: "The press picks, and letting go where it went down finishes the click: the line under the list says released. Move more than five points first and the grid reports that the row is being carried instead, and never reports the release, so a list that opens a row when it is let go never opens one that was dragged away."}

        StoryHeading{text: "Three ways to select"}
        StoryNote{text: "Cells is the spreadsheet on the Overview page. Rows picks a whole row with a press or an arrow key, and a heading press only sorts. Off, used here, picks nothing and draws nothing, for a host whose picks are its own."}
    }
}

/// The columns, in the order the grid shows them.
const COLUMNS: [&str; 3] = ["First", "Last", "Born"];

/// The rows as they arrive. More than fit in the grid's height, so an edit
/// can be scrolled out of sight and back.
const ROWS: &[[&str; 3]] = &[
    ["Ada", "Lovelace", "1815"],
    ["Alan", "Turing", "1912"],
    ["Grace", "Hopper", "1906"],
    ["Katherine", "Johnson", "1918"],
    ["Edsger", "Dijkstra", "1930"],
    ["Barbara", "Liskov", "1939"],
    ["Frances", "Allen", "1932"],
    ["Donald", "Knuth", "1938"],
    ["Margaret", "Hamilton", "1936"],
    ["Tony", "Hoare", "1934"],
    ["Leslie", "Lamport", "1941"],
    ["Karen", "Sparck Jones", "1935"],
    ["John", "Backus", "1924"],
    ["Jean", "Bartik", "1924"],
    ["Kathleen", "Booth", "1922"],
    ["Adele", "Goldberg", "1945"],
    ["Radia", "Perlman", "1951"],
    ["Dennis", "Ritchie", "1941"],
    ["Ken", "Thompson", "1943"],
    ["Niklaus", "Wirth", "1934"],
];

/// The rows in the order a sort put them, as indices into `rows`.
/// Empty means the order they were written in.
///
/// Kept here rather than in the grid because the grid holds no data:
/// it reports the heading press and the host does the sorting, which
/// is what this page is demonstrating.
fn sorted_order<S: AsRef<str>>(rows: &[[S; 3]], col: usize, ascending: Option<bool>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    let Some(ascending) = ascending else {
        return order;
    };
    // Stable, so the rows that tie stay in the order they came in,
    // and a second column press does not shuffle them.
    order.sort_by(|a, b| {
        let (x, y) = (rows[*a][col].as_ref(), rows[*b][col].as_ref());
        if ascending { x.cmp(y) } else { y.cmp(x) }
    });
    order
}

/// The list page's columns.
const SEED_COLUMNS: [&str; 4] = ["Seed", "Kind", "Sow", "Days"];

/// The list page's rows: made up, and more than fit, so a pick can be
/// scrolled away from.
const SEEDS: &[[&str; 4]] = &[
    ["Amber Runner", "bean", "May", "70"],
    ["Blue Lake Pole", "bean", "May", "65"],
    ["Early Round", "beet", "March", "55"],
    ["Winter Keeper", "beet", "June", "80"],
    ["Purple Sprouting", "broccoli", "April", "120"],
    ["Little Gem", "lettuce", "March", "50"],
    ["Red Oak Leaf", "lettuce", "April", "45"],
    ["Paris Market", "carrot", "March", "60"],
    ["Long Autumn", "carrot", "May", "90"],
    ["Green Globe", "artichoke", "February", "150"],
    ["Snow Crown", "cauliflower", "April", "85"],
    ["Golden Ball", "turnip", "July", "55"],
    ["White Lisbon", "onion", "March", "60"],
    ["Stuttgart Giant", "onion", "March", "110"],
    ["Sugar Snap", "pea", "March", "65"],
    ["Early Onward", "pea", "April", "70"],
    ["Cherry Belle", "radish", "April", "25"],
    ["Black Spanish", "radish", "July", "55"],
    ["Bright Lights", "chard", "April", "60"],
    ["Perpetual", "spinach", "April", "50"],
    ["Crown Prince", "squash", "May", "100"],
    ["Golden Acorn", "squash", "May", "85"],
    ["Long Green Ridge", "cucumber", "May", "65"],
    ["Garden Pearl", "tomato", "March", "75"],
];

/// The picks a list's host keeps. They are held by item, not by line, so
/// a pick stays with its row wherever the row is drawn.
#[derive(Default, Debug)]
struct Picks {
    items: BTreeSet<usize>,
    /// The item a Shift-click measures from: the last one clicked without
    /// Shift.
    anchor: Option<usize>,
    /// The item the cursor outline sits on: the last one clicked.
    cursor: Option<usize>,
}

impl Picks {
    /// A click on `line` with `held` down, where `order` says which item
    /// each line draws. A plain click picks that row alone, Ctrl (or the
    /// logo key) adds it or drops it, and Shift picks every line from the
    /// anchor to it, on top of the picks when Ctrl is held as well.
    fn press(&mut self, order: &[usize], line: usize, held: KeyModifiers) {
        let Some(&item) = order.get(line) else {
            return;
        };
        let toggle = held.control || held.logo;
        let anchor_line = self.anchor.and_then(|a| order.iter().position(|&i| i == a));
        match anchor_line {
            Some(from) if held.shift => {
                if !toggle {
                    self.items.clear();
                }
                let (lo, hi) = (from.min(line), from.max(line));
                self.items.extend(order[lo..=hi].iter().copied());
            }
            _ if toggle => {
                if !self.items.remove(&item) {
                    self.items.insert(item);
                }
                self.anchor = Some(item);
            }
            _ => {
                self.items.clear();
                self.items.insert(item);
                self.anchor = Some(item);
            }
        }
        self.cursor = Some(item);
    }
}

/// What this page will write into a cell of `col`: the text tidied, or
/// the reason it will not be. The grid asks nothing of the value; the
/// judgement is the host's, and here this is the whole of it.
fn accept(col: usize, text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    if col == 2 {
        return if text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit()) {
            Ok(text.to_string())
        } else {
            Err("not a four-digit year")
        };
    }
    if text.is_empty() {
        Err("a name cannot be blank")
    } else {
        Ok(text.to_string())
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryDataGrid {
    #[deref]
    view: View,
    /// The cells as they now read: `ROWS` with the edits this page accepted
    /// written over it. Filled from `ROWS` the first time it is asked for,
    /// since a `#[rust]` field starts empty.
    #[rust]
    cells: Vec<[String; 3]>,
    /// Indices into `cells`. Empty until a heading is pressed.
    #[rust]
    order: Vec<usize>,
}

impl StoryDataGrid {
    fn cells(&mut self) -> &[[String; 3]] {
        if self.cells.is_empty() {
            self.cells = ROWS.iter().map(|row| row.map(str::to_string)).collect();
        }
        &self.cells
    }

    /// A heading was pressed. Returns what to say about it.
    fn sort(&mut self, col: usize, ascending: Option<bool>) -> String {
        self.order = match ascending {
            Some(_) => sorted_order(self.cells(), col, ascending),
            None => Vec::new(),
        };
        let name = COLUMNS.get(col).copied().unwrap_or("?");
        match ascending {
            Some(true) => format!("sorted by {name}, up"),
            Some(false) => format!("sorted by {name}, down"),
            None => "back to the order the rows arrived in".to_string(),
        }
    }

    /// Which row of the data is drawn at this line of the grid.
    fn row_at(&self, line: usize) -> usize {
        self.order.get(line).copied().unwrap_or(line)
    }

    /// What the cell at this line and column says now: the text an
    /// amendment starts from.
    fn text_at(&mut self, line: usize, col: usize) -> String {
        let row = self.row_at(line);
        self.cells().get(row).map(|cells| cells[col].clone()).unwrap_or_default()
    }

    /// The editor on this line and column closed holding `text`. Returns
    /// what to say about it: kept, or refused and why. The grid has put
    /// the editor away either way; a refusal is nothing more than leaving
    /// the cell as it was.
    fn edited(&mut self, line: usize, col: usize, text: &str) -> String {
        let row = self.row_at(line);
        let name = COLUMNS.get(col).copied().unwrap_or("?");
        let who = self.cells().get(row).map(|cells| cells[1].clone()).unwrap_or_default();
        match accept(col, text) {
            Ok(value) => {
                if let Some(cells) = self.cells.get_mut(row) {
                    cells[col] = value.clone();
                }
                format!("{who}, {name}: now \"{value}\"")
            }
            Err(why) => format!("{who}, {name}: refused \"{text}\", {why}"),
        }
    }
}

impl Widget for StoryDataGrid {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else {
                continue;
            };
            grid.set_col_labels(COLUMNS.iter().map(|s| s.to_string()).collect());
            let rows = self.cells().len();
            grid.set_grid_size(rows, 3);
            // The loop is the whole contract. A cell nobody draws gets no
            // quad and no gridline, so a grid with no host behind it shows
            // its headers over a flat field and looks half finished rather
            // than empty. The cell being edited is never handed out: the
            // grid draws its editor there and moves on.
            while let Some(cell) = grid.next_cell(cx) {
                // cell.col, never cell.display_col: the data index survives
                // a column being dragged somewhere else.
                //
                // cell.row is a line of the grid, not a row of the data:
                // sorting moves the data under the lines and leaves the
                // lines where they are.
                let row = self.row_at(cell.row);
                grid.cell_text(cx, &cell, &self.cells[row][cell.col]);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryDataGridList {
    #[deref]
    view: View,
    #[live]
    color_picked: Vec4f,
    #[live]
    color_picked_text: Vec4f,
    #[live]
    color_cursor: Vec4f,
    /// Which item each line draws. Filled the first time it is asked for.
    #[rust]
    order: Vec<usize>,
    #[rust]
    picks: Picks,
}

impl StoryDataGridList {
    fn order(&mut self) -> &[usize] {
        if self.order.len() != SEEDS.len() {
            self.order = (0..SEEDS.len()).collect();
        }
        &self.order
    }

    /// A row was clicked. Returns what to say about the picks now.
    fn press(&mut self, line: usize, held: KeyModifiers) -> String {
        self.order();
        self.picks.press(&self.order, line, held);
        let names: Vec<&str> = self
            .order
            .iter()
            .filter(|item| self.picks.items.contains(item))
            .map(|item| SEEDS[*item][0])
            .collect();
        match names.len() {
            0 => "nothing picked".to_string(),
            1 => format!("picked {}", names[0]),
            n => format!("{n} picked: {}", names.join(", ")),
        }
    }
}

impl Widget for StoryDataGridList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else {
                continue;
            };
            grid.set_col_labels(SEED_COLUMNS.iter().map(|s| s.to_string()).collect());
            let lines = self.order().len();
            grid.set_grid_size(lines, SEED_COLUMNS.len());
            while let Some(cell) = grid.next_cell(cx) {
                let item = self.order[cell.row];
                // The grid holds no picks, so a picked row is nothing but
                // a row this page draws in other colours.
                let style = if self.picks.items.contains(&item) {
                    CellStyle {
                        bg: Some(self.color_picked),
                        color: Some(self.color_picked_text),
                        ..Default::default()
                    }
                } else {
                    CellStyle::default()
                };
                grid.cell_text_styled(cx, &cell, SEEDS[item][cell.col], style);
            }
            // Asked for on every draw, like the cells: an outline lasts
            // one frame.
            let cursor = self.picks.cursor.and_then(|c| self.order.iter().position(|&i| i == c));
            if let Some(line) = cursor {
                grid.outline_row(line, self.color_cursor);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

fn data_grid_list_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let grid = root.data_grid(cx, ids!(list.grid));
    let host = root.widget(cx, ids!(list));
    for action in grid.actions(actions) {
        match action {
            // With the selection off the grid picks nothing: the press
            // says which row and which keys, and the picking is done here.
            DataGridAction::CellClicked { row, modifiers, .. } => {
                let said = host
                    .borrow_mut::<StoryDataGridList>()
                    .map(|mut inner| inner.press(row, modifiers));
                if let Some(said) = said {
                    host.redraw(cx);
                    root.label(cx, ids!(list.picked)).set_text(cx, &said);
                }
            }
            // The press came up within the threshold: a finished click,
            // the moment to act on a row.
            DataGridAction::CellReleased { row, .. } => {
                root.label(cx, ids!(list.log)).set_text(cx, &format!("released row {row}"));
            }
            // The press travelled: the row is on its way somewhere, and
            // no release will follow.
            DataGridAction::RowDragStarted { row, .. } => {
                root.label(cx, ids!(list.log)).set_text(cx, &format!("carrying row {row}"));
            }
            _ => {}
        }
    }
}

fn data_grid_actions(cx: &mut Cx, root: &WidgetRef, actions: &Actions) {
    let grid = root.data_grid(cx, ids!(demo.grid));
    let host = root.widget(cx, ids!(demo));
    for action in grid.actions(actions) {
        match action {
            // The grid asked to edit. It holds no data, so the text to start
            // from comes from here: what was typed, to replace the cell, or
            // what the cell says, to amend it. This answer is the whole of
            // hosting the editor.
            DataGridAction::EditCell { row, col, replace } => {
                let text = match replace {
                    Some(typed) => typed,
                    None => host
                        .borrow_mut::<StoryDataGrid>()
                        .map(|mut inner| inner.text_at(row, col))
                        .unwrap_or_default(),
                };
                grid.edit_cell(cx, row, col, &text);
            }
            // The editor closed with a value. Whether it is written down is
            // decided here, and the grid redraws the cell with whatever this
            // page still has.
            DataGridAction::CellEdited { row, col, text } => {
                let said = host
                    .borrow_mut::<StoryDataGrid>()
                    .map(|mut inner| inner.edited(row, col, &text));
                if let Some(said) = said {
                    host.redraw(cx);
                    root.label(cx, ids!(demo.edited)).set_text(cx, &said);
                }
            }
            DataGridAction::EditCancelled { .. } => {
                root.label(cx, ids!(demo.edited)).set_text(cx, "put back as it was");
            }
            _ => {}
        }
    }
    // The grid sorts nothing. It says which heading was pressed and
    // which way it now points, and the rows move here.
    if let Some((col, ascending)) = grid.sort_changed(actions) {
        let said = host
            .borrow_mut::<StoryDataGrid>()
            .map(|mut inner| inner.sort(col, ascending));
        if let Some(said) = said {
            host.redraw(cx);
            root.label(cx, ids!(demo.reported)).set_text(cx, &said);
            return;
        }
    }
    // Asked each pass rather than listened for: the selection moves under
    // the keyboard as well as the mouse, and a label that only heard about
    // clicks would fall behind the arrow keys.
    let text = match grid.selection() {
        // A row selection covers every column, so it is counted in rows.
        Some(sel) if sel.kind == GridSelectKind::Rows => match sel.row_range() {
            (r0, r1) if r0 == r1 => format!("row {r0}"),
            (r0, r1) => format!("rows {r0}-{r1}"),
        },
        Some(sel) => {
            let (r0, r1) = (sel.anchor.0.min(sel.head.0), sel.anchor.0.max(sel.head.0));
            let (c0, c1) = (sel.anchor.1.min(sel.head.1), sel.anchor.1.max(sel.head.1));
            let cells = (r1 - r0 + 1) * (c1 - c0 + 1);
            if cells == 1 {
                format!("one cell: row {r0}, column {c0}")
            } else {
                format!("rows {r0}-{r1} by columns {c0}-{c1}: {cells} cells")
            }
        }
        None => "nothing selected".to_string(),
    };
    let label = root.label(cx, ids!(demo.reported));
    if label.text() != text {
        label.set_text(cx, &text);
    }
}

/// The grid's knobs. The defaults are what the story's own declaration
/// says, and each sits on a step of its slider.
const CONTROLS: &[Control] = &[
    Control { label: "Row height", target: "demo.grid", kind: ControlKind::Number { prop: "default_row_height", min: 18., max: 40., step: 2., default: 24. } },
    Control { label: "Column width", target: "demo.grid", kind: ControlKind::Number { prop: "default_col_width", min: 90., max: 250., step: 10., default: 150. } },
    Control { label: "Row numbers", target: "demo.grid", kind: ControlKind::Bool { prop: "show_row_headers", default: true } },
    Control { label: "Zebra stripes", target: "demo.grid", kind: ControlKind::Bool { prop: "zebra_stripes", default: false } },
    Control { label: "Sortable headings", target: "demo.grid", kind: ControlKind::Bool { prop: "sortable", default: true } },
    Control { label: "Drag columns", target: "demo.grid", kind: ControlKind::Bool { prop: "allow_col_reorder", default: true } },
    Control { label: "Selection", target: "demo.grid", kind: ControlKind::Choice { prop: "selection", options: &["GridSelectMode.Cells", "GridSelectMode.Rows", "GridSelectMode.Off"], default: 0 } },
];

pub const STORIES: &[Story] = &[
    Story {
        key: "collections/datagrid/overview",
        category: "Collections",
        component: "DataGrid",
        also: &[],
        name: "Overview",
        dsl: "DataGridOverview",
        added: "2025-05-06",
        tags: &["editing"],
        doc: "# DataGrid

A spreadsheet-shaped table over a very large number of rows. Eleven places in this repository use one.

**It holds none of your data.** It knows a row count, a column count and a size for each axis, works out which cells are on screen, and asks you for them one at a time in a draw loop — `while let Some(cell) = grid.next_cell(cx)` — the way the portal list asks for rows. Both axes are virtualised over a sparse size table, so a million rows costs nothing until somebody resizes one.

The consequence is the trap: **a `DataGrid` with no host behind it is not empty, it is half drawn.** The background and the headers are painted by the grid; the cell quads and the gridlines are painted by your call. Leave the loop out and you get lettered columns and numbered rows over a flat field.

## What it does, and what is yours

Its own: column and row headers, resizing a column or a row by dragging its edge, reordering columns by dragging a header (`allow_col_reorder`, off by default while both resize flags are on), the whole selection model — single cell, rectangle, row, column, everything — and keyboard navigation with arrows, the page keys, Home, End, and shift to extend.

What a press selects is `selection:`. `GridSelectMode.Cells`, the default, is the spreadsheet described here. `GridSelectMode.Rows` picks whole rows with a press or an arrow key, and a heading press only sorts. `GridSelectMode.Off` selects nothing at all and still reports every press with its modifiers, for a list that keeps its own picks; the List page beside this one is that.

Yours, despite the name: **it does not sort the rows** — with `sortable: true` a heading press cycles unsorted, up, down and back, draws the mark and raises `SortChanged { col, ascending }`; the rows themselves are yours to reorder, because the grid never held them. This page keeps a list of indices and draws through it, which is all it takes. **It does not copy** — Cmd+C does nothing until you install a `set_copy_provider`. **Editing is shared** — the grid seats the editor and reads its keys, you answer `EditCell` with the text to start from and `CellEdited` with a yes or a no; the next section is about that. And row headers cannot be labelled: that strip always prints the row number, so names down the left belong in column zero with `show_row_headers: false`.

## Editing

The grid seats its own editor. **Type over** a selected cell and the editor opens holding what you typed; **F2**, **Return** or a **double-click** opens it holding what the cell says, with the caret after the last character. **Return** keeps the value and steps down a row, **shift-Return** up, **Tab** sideways and shift-Tab back; **Escape** puts the cell back as it was; a click anywhere else keeps the value where it stands, and asking to edit another cell keeps this one first.

### The host still owns the data

The grid holds no data, so it cannot start an edit by itself. It raises `EditCell { row, col, replace }` — `replace` carries what was typed, or nothing for an amendment — and the host answers with `edit_cell(cx, row, col, text)`, passing the typed text or the cell's own. An unanswered request is a read-only grid, which is what an unanswered request ought to be.

When the editor closes the grid raises `CellEdited { row, col, text }`, with the editor already put away and, for a key, the selection already moved on. Whether the text is written down is the host's call, and **refusing is not writing it down**: the cell redraws with what the host still has. This page keeps a name that is not blank and a year of four digits, and says under the grid what it refused and why. A host that wants the person to try again reopens the editor with `edit_cell` and the refused text. Escape raises `EditCancelled` and nothing else; there is nothing to write.

This page answers with a handful of lines, which is the whole of hosting an editor.

### Why it lives on the grid

A host that draws its own text input into the edited cell loses the text the moment the wheel moves. The grid recycles the widgets of cells that leave the screen, and a host-side editor is one of them: it goes into the pool with the text still in it, and the next cell to reuse it writes the template back over the top. Only the grid can keep the live editor out of that sweep — it does, whether or not the cell is on screen — and only the grid can turn the editor losing the keyboard into a commit.

Two details come with it that a host would otherwise have to find for itself. The seed is written exactly once, when the editor is seated, and never during a draw, so a redraw cannot write over what has been typed since. And the keyboard is handed to the editor after the first draw that gives it an area; focus set before then lands nowhere.

### The editor

The stock editor fills the cell in the grid's type size from `theme.font_regular`. A grid that wants another look declares `Editor := TextInput{...}` inside its own declaration and that one takes its place. Whatever it looks like it has to be a `TextInput`: Return, Escape and the loss of the keyboard are read from it.

## Two things that will bite

**Declare `rows: 0`.** `set_grid_size` returns early when the counts already match, and the early return skips the geometry pass. Declare the real count in the DSL and your first `set_grid_size` does nothing, so a width set in the same frame lands a frame late with nothing scheduling that frame. Every caller here declares zero.

**Index with `cell.col`, not `cell.display_col`.** The data index survives a column being dragged elsewhere; the display index is where it currently sits. Widths and selection coordinates are in display space, while `HeaderClicked` and the sort indicator are in data space — mix them and you get the wrong column silently, one drag later.

## It ignores the theme

Every surface it paints is a literal light-mode colour — `color_bg: #fafafa`, `color_cell: #ffffff`, `color_text: #202020`, the gridline, and both scrollbar handles in translucent *black*. All six application callers restate the lot; one of them says in a comment that the register turns up white otherwise. This page restates them from theme tokens, which is the shortest honest demonstration of what the widget actually costs to use.",
        subject: "demo",
        feature: None,
        controls: CONTROLS,
        on_actions: Some(data_grid_actions),
    },
    Story {
        key: "collections/datagrid/list",
        category: "Collections",
        component: "DataGrid",
        also: &[],
        name: "List",
        dsl: "DataGridList",
        added: "2025-05-06",
        tags: &["list", "picks"],
        doc: "# DataGrid as a list

The grid on the Overview page is a spreadsheet. This one is a list: no row numbers, rows picked whole, and **the picks kept by the host**. A list's picks usually mean something to the rest of the app and have to survive its rows being rebuilt, sorted and moved, so the grid is told to keep out of the way.

## `selection:`

- `GridSelectMode.Cells` — the default and the spreadsheet: a press selects a cell, a drag a rectangle, a row number its row and a heading its column.
- `GridSelectMode.Rows` — a press or an arrow key selects the whole row, shift extends by rows, and a heading press sorts without selecting a column. No cell is drawn as the active one, and a row pick does not light the headings.
- `GridSelectMode.Off` — the grid never selects: no overlay, no rubber band, no keys that move a selection. A press still raises `CellClicked { row, col, modifiers }`, and that is what a host with picks of its own reads. This page is declared this way.

## The picks are the page's

This page keeps a set of picked items and an anchor. A plain click picks one row, Ctrl adds or drops a row, Shift picks from the anchor to the row clicked. The picks are held **by item, not by line**, so they stay on their rows wherever those rows are drawn. A picked row is nothing more than a row the page draws with `cell_text_styled` and another background.

## A click, or a carry

A press raises `CellClicked` as it goes down. When it comes back up without the pointer having gone further than `drag_threshold` (5 points unless declared otherwise), the grid raises `CellReleased { row, col, modifiers }`, in every selection mode. A host that acts on a finished click — opening a row, loading it — reads the release rather than the press.

With `row_drag: true`, a press that travels past the threshold raises `RowDragStarted { row, col, abs, modifiers }` once, with `abs` where the pointer is as the carry begins. From then on the grid leaves that press alone: it selects nothing, does not scroll at the edge, and raises no `CellReleased` when the press comes up. Where the row goes is the host's to follow. Without `row_drag` a drag across cells drags out a selection, as it always has, and is not a click either.

## The outline

`outline_row(row, color)` draws a 1.5 point outline round a row's visible cells: a keyboard cursor, a row being carried, anything a host marks without selecting it. Call it from the draw loop, as the cells are. It is drawn when the frame ends, over every cell, so the row's own cells may come before the call or after it; and it lasts one frame, so ask for it again on every draw. This page outlines the row clicked last.",
        subject: "list",
        feature: None,
        controls: &[],
        on_actions: Some(data_grid_list_actions),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_and_down_are_reverses_of_each_other() {
        let up = sorted_order(ROWS, 1, Some(true));
        let mut down = sorted_order(ROWS, 1, Some(false));
        down.reverse();
        assert_eq!(up, down);
    }

    #[test]
    fn unsorted_is_the_order_the_rows_arrived_in() {
        assert_eq!(sorted_order(ROWS, 0, None), (0..ROWS.len()).collect::<Vec<_>>());
    }

    #[test]
    fn sorting_by_a_column_orders_that_column() {
        let order = sorted_order(ROWS, 1, Some(true));
        let names: Vec<&str> = order.iter().map(|i| ROWS[*i][1]).collect();
        let mut want = names.clone();
        want.sort();
        assert_eq!(names, want);
        assert_eq!(names[0], "Allen");
    }

    #[test]
    fn every_row_is_still_there_afterwards() {
        let mut order = sorted_order(ROWS, 2, Some(false));
        order.sort();
        assert_eq!(order, (0..ROWS.len()).collect::<Vec<_>>());
    }

    /// The edited rows sort the same way the arriving ones do: the order
    /// is over whatever the cells say now, not over the constant.
    #[test]
    fn edited_cells_sort_as_they_now_read() {
        let mut cells: Vec<[String; 3]> = ROWS.iter().map(|row| row.map(str::to_string)).collect();
        cells[0][1] = "Zuse".to_string();
        let order = sorted_order(&cells, 1, Some(true));
        assert_eq!(*order.last().unwrap(), 0);
    }

    #[test]
    fn a_year_is_four_digits_and_nothing_else() {
        assert_eq!(accept(2, " 1815 "), Ok("1815".to_string()));
        assert_eq!(accept(2, "abc"), Err("not a four-digit year"));
        assert_eq!(accept(2, "18150"), Err("not a four-digit year"));
        assert_eq!(accept(2, ""), Err("not a four-digit year"));
    }

    #[test]
    fn a_name_is_anything_but_blank() {
        assert_eq!(accept(0, " Ada "), Ok("Ada".to_string()));
        assert_eq!(accept(1, "   "), Err("a name cannot be blank"));
    }

    fn held(shift: bool, control: bool) -> KeyModifiers {
        KeyModifiers {
            shift,
            control,
            ..Default::default()
        }
    }

    fn picked(picks: &Picks) -> Vec<usize> {
        picks.items.iter().copied().collect()
    }

    #[test]
    fn a_plain_click_picks_one_row_and_ctrl_adds_or_drops() {
        let order: Vec<usize> = (0..10).collect();
        let mut picks = Picks::default();
        picks.press(&order, 3, held(false, false));
        assert_eq!(picked(&picks), vec![3]);
        picks.press(&order, 6, held(false, true));
        assert_eq!(picked(&picks), vec![3, 6]);
        picks.press(&order, 3, held(false, true));
        assert_eq!(picked(&picks), vec![6]);
        assert_eq!(picks.cursor, Some(3), "the cursor is on the row clicked last");
        picks.press(&order, 8, held(false, false));
        assert_eq!(picked(&picks), vec![8]);
    }

    /// Shift measures from the last row clicked without it, and a second
    /// Shift-click moves the far end rather than the anchor.
    #[test]
    fn shift_picks_every_line_from_the_anchor() {
        let order: Vec<usize> = (0..10).collect();
        let mut picks = Picks::default();
        picks.press(&order, 4, held(false, false));
        picks.press(&order, 7, held(true, false));
        assert_eq!(picked(&picks), vec![4, 5, 6, 7]);
        picks.press(&order, 2, held(true, false));
        assert_eq!(picked(&picks), vec![2, 3, 4]);
        assert_eq!(picks.cursor, Some(2));
    }

    /// Picks are items: drawn in another order the same items stay
    /// picked, and a Shift range runs over the lines as they are drawn now.
    #[test]
    fn picks_follow_their_items_through_another_order() {
        let order: Vec<usize> = (0..6).collect();
        let reversed: Vec<usize> = (0..6).rev().collect();
        let mut picks = Picks::default();
        picks.press(&order, 1, held(false, false));
        // Item 1 is on line 4 now. Shift-clicking line 2, item 3, picks
        // lines 2 to 4: items 3, 2 and 1.
        picks.press(&reversed, 2, held(true, false));
        assert_eq!(picked(&picks), vec![1, 2, 3]);
    }

    /// A `Cx` with the theme, the shared page templates and this file's
    /// stories, and nothing else, with the script errors this file's own
    /// DSL raised while it was read.
    fn shell() -> (Cx, Vec<String>) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let errors = cx.with_vm(|vm| {
            crate::theme::widgets_script_mod(vm);
            crate::shell::script_mod(vm);
            vm.bx.captured_errors = Some(Vec::new());
            self::script_mod(vm);
            let _ = makepad_platform::shader_error::take();
            vm.take_errors()
        });
        (cx, errors)
    }

    /// Both pages read without a script error, and each holds the grid its
    /// action handler addresses. Nothing the compiler checks reads the DSL:
    /// a misspelt `selection:` value is one log line and a spreadsheet
    /// where a list was meant.
    #[test]
    fn both_pages_build_and_hold_the_grids_their_handlers_address() {
        let (mut cx, errors) = shell();
        assert!(errors.is_empty(), "{errors:?}");
        let addressed: [(&[LiveId], &[&[LiveId]]); 2] = [
            (ids!(demo.grid), &[ids!(demo.reported), ids!(demo.edited)]),
            (ids!(list.grid), &[ids!(list.picked), ids!(list.log)]),
        ];
        for (story, (path, labels)) in STORIES.iter().zip(addressed) {
            let page = cx.with_vm(|vm| {
                let stories = vm.module(id!(stories));
                let value = vm.bx.heap.value(stories, LiveId::from_str(story.dsl).into(), NoTrap);
                assert!(value.as_object().is_some(), "no template {}", story.dsl);
                WidgetRef::script_from_value(vm, value)
            });
            assert!(!page.is_empty(), "{} built no widget", story.key);
            assert!(page.data_grid(&cx, path).borrow().is_some(), "{}: no grid", story.key);
            for label in labels {
                assert!(page.label(&cx, label).borrow().is_some(), "{}: no label {label:?}", story.key);
            }
            let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
            assert!(!subject.is_empty(), "{}: no {}", story.key, story.subject);
        }
    }
}
