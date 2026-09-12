//! The data grid story: a grid that draws cells the host hands it, one at a
//! time, and holds none of them — and, now that the editor lives on the
//! grid, a host that says what may be written into them.
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};

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
                color_cell_alt: theme.color_surface_container_lowest
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

    mod.stories.DataGridOverview = StoryPage{
        StoryNote{text: "A spreadsheet-shaped table over a very large number of rows. Eleven places in this repository use one. It holds none of your data: it works out which cells are on screen and asks for them, one at a time, while you draw them."}

        StoryHeading{text: "Three columns of people"}
        StoryNote{text: "Click a cell, shift-click another for a rectangle, click a row number for the row, a column header for the column. Arrow keys move and shift extends. Drag a column edge to resize it, and a column header to move it somewhere else."}
        StoryNote{text: "Press a heading to sort by it: once for up, again for down, a third time back to the order the rows arrived in. The grid does not do this — it reports the press and this page reorders its own rows, which is the whole arrangement."}
        StoryNote{text: "Every cell can be edited: type over it, or press F2, Return or double-click to amend it. The Editing page is about that."}
        demo := mod.storybook.StoryDataGrid{}
    }

    mod.stories.DataGridEditing = StoryPage{
        StoryNote{text: "The grid seats its own editor. Type over a selected cell to replace what it says, or press F2, Return or double-click to amend it. Return keeps the value and steps down a row, shift-Return steps up, Tab steps sideways, Escape puts the cell back as it was, and a click anywhere else keeps the value where it stands."}

        StoryHeading{text: "The host says what may be written"}
        StoryNote{text: "The grid holds no data, so it cannot know what a cell says or whether a value is any good. It asks this page for the text to start from and hands the finished text back. This page keeps a name that is not blank and a year of four digits, and refuses the rest: a refused value is simply not written down, the cell redraws with what it had, and the line under the grid says why."}

        StoryHeading{text: "An edit survives a scroll"}
        StoryNote{text: "Start an edit, wheel the row off the bottom and back, and the text is still there. The grid keeps the live editor out of the pool its other cells are recycled through; a host that drew its own editor into a cell lost the text the moment the wheel moved."}
        demo := mod.storybook.StoryDataGrid{}
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

/// Both pages show the same grid, so both get the same knobs. The defaults
/// are what the story's own declaration says, and each sits on a step of
/// its slider.
const CONTROLS: &[Control] = &[
    Control { label: "Row height", target: "demo.grid", kind: ControlKind::Number { prop: "default_row_height", min: 18., max: 40., step: 2., default: 24. } },
    Control { label: "Column width", target: "demo.grid", kind: ControlKind::Number { prop: "default_col_width", min: 90., max: 250., step: 10., default: 150. } },
    Control { label: "Row numbers", target: "demo.grid", kind: ControlKind::Bool { prop: "show_row_headers", default: true } },
    Control { label: "Zebra stripes", target: "demo.grid", kind: ControlKind::Bool { prop: "zebra_stripes", default: false } },
    Control { label: "Sortable headings", target: "demo.grid", kind: ControlKind::Bool { prop: "sortable", default: true } },
    Control { label: "Drag columns", target: "demo.grid", kind: ControlKind::Bool { prop: "allow_col_reorder", default: true } },
];

pub const STORIES: &[Story] = &[
    Story {
        key: "data-display/datagrid/overview",
        category: "Data display",
        component: "DataGrid",
        also: &[],
        name: "Overview",
        dsl: "DataGridOverview",
        added: "2025-05-06",
        tags: &[],
        doc: "# DataGrid

A spreadsheet-shaped table over a very large number of rows. Eleven places in this repository use one.

**It holds none of your data.** It knows a row count, a column count and a size for each axis, works out which cells are on screen, and asks you for them one at a time in a draw loop — `while let Some(cell) = grid.next_cell(cx)` — the way the portal list asks for rows. Both axes are virtualised over a sparse size table, so a million rows costs nothing until somebody resizes one.

The consequence is the trap: **a `DataGrid` with no host behind it is not empty, it is half drawn.** The background and the headers are painted by the grid; the cell quads and the gridlines are painted by your call. Leave the loop out and you get lettered columns and numbered rows over a flat field.

## What it does, and what is yours

Its own: column and row headers, resizing a column or a row by dragging its edge, reordering columns by dragging a header (`allow_col_reorder`, off by default while both resize flags are on), the whole selection model — single cell, rectangle, row, column, everything — and keyboard navigation with arrows, the page keys, Home, End, and shift to extend.

Yours, despite the name: **it does not sort the rows** — with `sortable: true` a heading press cycles unsorted, up, down and back, draws the mark and raises `SortChanged { col, ascending }`; the rows themselves are yours to reorder, because the grid never held them. This page keeps a list of indices and draws through it, which is all it takes. **It does not copy** — Cmd+C does nothing until you install a `set_copy_provider`. **Editing is shared** — the grid seats the editor and reads its keys, you answer `EditCell` with the text to start from and `CellEdited` with a yes or a no; the Editing page is about that. And row headers cannot be labelled: that strip always prints the row number, so names down the left belong in column zero with `show_row_headers: false`.

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
        key: "data-display/datagrid/editing",
        category: "Data display",
        component: "DataGrid",
        also: &[],
        name: "Editing",
        dsl: "DataGridEditing",
        added: "2026-09-11",
        tags: &["editing"],
        doc: "# DataGrid editing

The grid seats its own editor. **Type over** a selected cell and the editor opens holding what you typed; **F2**, **Return** or a **double-click** opens it holding what the cell says, with the caret after the last character. **Return** keeps the value and steps down a row, **shift-Return** up, **Tab** sideways and shift-Tab back; **Escape** puts the cell back as it was; a click anywhere else keeps the value where it stands, and asking to edit another cell keeps this one first.

## The host still owns the data

The grid holds no data, so it cannot start an edit by itself. It raises `EditCell { row, col, replace }` — `replace` carries what was typed, or nothing for an amendment — and the host answers with `edit_cell(cx, row, col, text)`, passing the typed text or the cell's own. An unanswered request is a read-only grid, which is what an unanswered request ought to be.

When the editor closes the grid raises `CellEdited { row, col, text }`, with the editor already put away and, for a key, the selection already moved on. Whether the text is written down is the host's call, and **refusing is not writing it down**: the cell redraws with what the host still has. This page keeps a name that is not blank and a year of four digits, and says under the grid what it refused and why. A host that wants the person to try again reopens the editor with `edit_cell` and the refused text. Escape raises `EditCancelled` and nothing else; there is nothing to write.

Both pages here answer with the same handful of lines, which is the whole of hosting an editor now.

## Why it lives on the grid

Three hosts in this repository, two applications and an example, each drew their own text input into the edited cell, and each lost the text the moment the wheel moved. The grid recycles the widgets of cells that leave the screen, and a host-side editor is one of them: it went into the pool with the text still in it, and the next cell to reuse it wrote the template back over the top. Only the grid can keep the live editor out of that sweep — it does, whether or not the cell is on screen — and only the grid can turn the editor losing the keyboard into a commit.

Two details every copy had to find for itself come with it. The seed is written exactly once, when the editor is seated, and never during a draw, so a redraw cannot write over what has been typed since. And the keyboard is handed to the editor after the first draw that gives it an area; focus set before then lands nowhere.

## The editor

The stock editor fills the cell in the grid's type size from `theme.font_regular`. A grid that wants another look declares `Editor := TextInput{...}` inside its own declaration and that one takes its place. Whatever it looks like it has to be a `TextInput`: Return, Escape and the loss of the keyboard are read from it.",
        subject: "demo",
        feature: None,
        controls: CONTROLS,
        on_actions: Some(data_grid_actions),
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
}
