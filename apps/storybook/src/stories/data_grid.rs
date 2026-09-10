//! The data grid story: a grid that draws cells the host hands it, one at a
//! time, and holds none of them.
use crate::makepad_widgets::*;
use crate::registry::Story;

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
    }

    mod.stories.DataGridOverview = StoryPage{
        StoryNote{text: "A spreadsheet-shaped table over a very large number of rows. Eleven places in this repository use one. It holds none of your data: it works out which cells are on screen and asks for them, one at a time, while you draw them."}

        StoryHeading{text: "Three columns of people"}
        StoryNote{text: "Click a cell, shift-click another for a rectangle, click a row number for the row, a column header for the column. Arrow keys move and shift extends. Drag a column edge to resize it, and a column header to move it somewhere else."}
        StoryNote{text: "Press a heading to sort by it: once for up, again for down, a third time back to the order the rows arrived in. The grid does not do this — it reports the press and this page reorders its own rows, which is the whole arrangement."}
        demo := mod.storybook.StoryDataGrid{}
    }
}

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
];

/// The rows in the order a sort put them, as indices into `ROWS`.
/// Empty means the order they were written in.
///
/// Kept here rather than in the grid because the grid holds no data:
/// it reports the heading press and the host does the sorting, which
/// is what this page is demonstrating.
fn sorted_order(col: usize, ascending: Option<bool>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ROWS.len()).collect();
    let Some(ascending) = ascending else {
        return order;
    };
    // Stable, so the rows that tie stay in the order they came in,
    // and a second column press does not shuffle them.
    order.sort_by(|a, b| {
        let (x, y) = (ROWS[*a][col], ROWS[*b][col]);
        if ascending { x.cmp(y) } else { y.cmp(x) }
    });
    order
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryDataGrid {
    #[deref]
    view: View,
    /// Indices into `ROWS`. Empty until a heading is pressed.
    #[rust]
    order: Vec<usize>,
}

impl StoryDataGrid {
    /// A heading was pressed. Returns what to say about it.
    fn sort(&mut self, col: usize, ascending: Option<bool>) -> String {
        self.order = match ascending {
            Some(_) => sorted_order(col, ascending),
            None => Vec::new(),
        };
        const NAMES: [&str; 3] = ["First", "Last", "Born"];
        let name = NAMES.get(col).copied().unwrap_or("?");
        match ascending {
            Some(true) => format!("sorted by {name}, up"),
            Some(false) => format!("sorted by {name}, down"),
            None => "back to the order the rows arrived in".to_string(),
        }
    }

    /// Which row of `ROWS` is drawn at this line of the grid.
    fn row_at(&self, line: usize) -> usize {
        self.order.get(line).copied().unwrap_or(line)
    }
}

impl Widget for StoryDataGrid {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else {
                continue;
            };
            grid.set_col_labels(
                ["First", "Last", "Born"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
            grid.set_grid_size(ROWS.len(), 3);
            // The loop is the whole contract. A cell nobody draws gets no
            // quad and no gridline, so a grid with no host behind it shows
            // its headers over a flat field and looks half finished rather
            // than empty.
            while let Some(cell) = grid.next_cell(cx) {
                // cell.col, never cell.display_col: the data index survives
                // a column being dragged somewhere else.
                //
                // cell.row is a line of the grid, not a row of the data:
                // sorting moves the data under the lines and leaves the
                // lines where they are.
                let row = self.row_at(cell.row);
                grid.cell_text(cx, &cell, ROWS[row][cell.col]);
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
    // The grid sorts nothing. It says which heading was pressed and
    // which way it now points, and the rows move here.
    if let Some((col, ascending)) = grid.sort_changed(actions) {
        let host = root.widget(cx, ids!(demo));
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

pub const STORIES: &[Story] = &[Story {
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

Yours, despite the name: **it does not sort the rows** — with `sortable: true` a heading press cycles unsorted, up, down and back, draws the mark and raises `SortChanged { col, ascending }`; the rows themselves are yours to reorder, because the grid never held them. This page keeps a list of indices and draws through it, which is all it takes. **It does not copy** — Cmd+C does nothing until you install a `set_copy_provider`. **It does not edit** — it raises `EditCell` and you host the input. And row headers cannot be labelled: that strip always prints the row number, so names down the left belong in column zero with `show_row_headers: false`.

## Two things that will bite

**Declare `rows: 0`.** `set_grid_size` returns early when the counts already match, and the early return skips the geometry pass. Declare the real count in the DSL and your first `set_grid_size` does nothing, so a width set in the same frame lands a frame late with nothing scheduling that frame. Every caller here declares zero.

**Index with `cell.col`, not `cell.display_col`.** The data index survives a column being dragged elsewhere; the display index is where it currently sits. Widths and selection coordinates are in display space, while `HeaderClicked` and the sort indicator are in data space — mix them and you get the wrong column silently, one drag later.

## It ignores the theme

Every surface it paints is a literal light-mode colour — `color_bg: #fafafa`, `color_cell: #ffffff`, `color_text: #202020`, the gridline, and both scrollbar handles in translucent *black*. All six application callers restate the lot; one of them says in a comment that the register turns up white otherwise. This page restates them from theme tokens, which is the shortest honest demonstration of what the widget actually costs to use.",
    subject: "demo",
    feature: None,
    controls: &[],
    on_actions: Some(data_grid_actions),
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_and_down_are_reverses_of_each_other() {
        let up = sorted_order(1, Some(true));
        let mut down = sorted_order(1, Some(false));
        down.reverse();
        assert_eq!(up, down);
    }

    #[test]
    fn unsorted_is_the_order_the_rows_arrived_in() {
        assert_eq!(sorted_order(0, None), (0..ROWS.len()).collect::<Vec<_>>());
    }

    #[test]
    fn sorting_by_a_column_orders_that_column() {
        let order = sorted_order(1, Some(true));
        let names: Vec<&str> = order.iter().map(|i| ROWS[*i][1]).collect();
        let mut want = names.clone();
        want.sort();
        assert_eq!(names, want);
        assert_eq!(names[0], "Allen");
    }

    #[test]
    fn every_row_is_still_there_afterwards() {
        let mut order = sorted_order(2, Some(false));
        order.sort();
        assert_eq!(order, (0..ROWS.len()).collect::<Vec<_>>());
    }
}
