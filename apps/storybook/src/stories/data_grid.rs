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
        color_carried: theme.color_tertiary

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
                // A row carried to the top or bottom of the list scrolls
                // it: the rows are reordered inside the list.
                row_drag_scroll: true
                show_row_headers: false
                allow_row_resize: false
                zebra_stripes: true
                default_col_width: 150.0
                default_row_height: 24.0
                // A heading dragged along the headings moves its column;
                // this page keeps the order.
                allow_col_reorder: true
                // A heading press sorts; this page does the sorting.
                sortable: true
                // Headings on the left, in a face of their own, the sorted
                // one brighter than the rest.
                header_align: 0.0
                color_header_sorted: theme.color_text
                draw_text_header +: {text_style: theme.font_bold{font_size: 8.5}}
                // A row is something to press.
                cell_cursor: MouseCursor.Hand

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
                // The bar a dragged heading drops at, in the accent so it
                // stands out from the gridlines.
                color_drag_marker: theme.color_primary
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
        StoryNote{text: "Turn on Drag to scroll and a drag across the cells moves the table with the pointer instead of selecting a rectangle, the way a finger moves a list on a phone. A press that stays where it went down still selects its cell."}

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

        StoryHeading{text: "Carrying a row moves it"}
        StoryNote{text: "Drag a row up or down the list and the rows move out of its way under the pointer; let go and it stays there. Hold it at the top or bottom of the list and the list scrolls, even while the pointer holds still. The grid moves no rows. It reports each move of the carried row, says which gap between the rows the pointer is over and scrolls at the edges, and this page reorders its own lines."}

        StoryHeading{text: "A menu on the right button"}
        StoryNote{text: "Right-click a row for a menu that moves it to the top or the bottom, or a heading for one that picks every row or none or puts the columns back. The right button asks for a menu and does nothing else: the picks stay as they were until a menu row is chosen. Ctrl with the left button is still a click that adds or drops a pick."}

        StoryHeading{text: "Headings and the pointer"}
        StoryNote{text: "The headings sit on the left in a face of their own, and the one the list is sorted by is brighter than the rest. Press a heading to sort by it, again to turn it round, and a third time for the order the seeds were written in; carrying a row, or moving one from its menu, ends the sort. Over a row the pointer is a hand."}

        StoryHeading{text: "Tips on the headings"}
        StoryNote{text: "Rest the pointer on a heading and a tip says what its column holds. The grid reports the heading and its text the way a Tip wrapper reports a control, to the window's one TipLayer, which this page declares at its end."}

        StoryHeading{text: "Moving columns"}
        StoryNote{text: "Drag a heading along the others and a bar shows where the column will land; let go and it moves there, taking its width with it. Seed stays first, because the page tells the grid it is unmovable, and nothing can be dropped in front of it. The grid reports the new order and this page keeps it with the widths; Put the columns back, on the heading menu, hands the grid their own order again."}

        StoryHeading{text: "The first column takes what is left"}
        StoryNote{text: "The page sets every column's width at once from its draw loop, and the Seed column fills whatever the others leave, so the list always ends at its right edge however wide the page is. Drag a column edge and the page stops fitting: the widths stay where they were dragged for as long as the page is open."}

        StoryHeading{text: "Three ways to select"}
        StoryNote{text: "Cells is the spreadsheet on the Overview page. Rows picks a whole row with a press or an arrow key, and a heading press only sorts. Off, used here, picks nothing and draws nothing, for a host whose picks are its own."}

        // Declared last, so the menus and the tips they raise draw over
        // the page.
        menus := MenuLayer{}
        tips := TipLayer{}
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

/// What each list column holds, shown as a tip on its heading.
const SEED_TIPS: [&str; 4] = [
    "The name of the variety",
    "What it grows into",
    "The month to sow it outdoors",
    "Days from sowing to the first harvest",
];

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
    "November", "December",
];

/// The seeds in the order a heading press asks for, as indices into
/// `SEEDS`: by that column, with months in calendar order and days as
/// numbers, or as written when the sort is off. Stable, so seeds that tie
/// keep the order they were written in.
fn sorted_seeds(col: usize, ascending: Option<bool>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..SEEDS.len()).collect();
    let Some(ascending) = ascending else {
        return order;
    };
    let key = |item: usize| {
        let text = SEEDS[item][col];
        let rank = match col {
            2 => MONTHS.iter().position(|m| *m == text).unwrap_or(12) as u32,
            3 => text.parse().unwrap_or(0),
            _ => 0,
        };
        (rank, text)
    };
    order.sort_by(|a, b| {
        let ord = key(*a).cmp(&key(*b));
        if ascending { ord } else { ord.reverse() }
    });
    order
}

/// The widths of every list column but the first, which takes what these
/// leave, down to `SEED_FIRST_MIN`.
const SEED_WIDTHS: [f64; 3] = [120.0, 100.0, 70.0];
const SEED_FIRST_MIN: f64 = 140.0;

/// The list's widths fitted to `data_width`: the first column fills
/// whatever the others leave, and past its minimum the list scrolls.
fn fit_seed_widths(data_width: f64) -> Vec<f64> {
    let rest: f64 = SEED_WIDTHS.iter().sum();
    let mut widths = vec![(data_width - rest).max(SEED_FIRST_MIN)];
    widths.extend(SEED_WIDTHS);
    widths
}

/// The list's columns as they have been arranged, all kept here: the
/// order they are drawn in and their widths. The widths are fitted to the
/// list until someone drags a column edge, and from then on they are what
/// the edges were dragged to. Both last as long as the page is open;
/// keeping them past that is writing `order` and `by_hand` out and reading
/// them back in.
#[derive(Debug)]
struct ListColumns {
    /// The column drawn at each position, from the left.
    order: Vec<usize>,
    /// The width each column was dragged to, by column, once one has been.
    by_hand: Option<Vec<f64>>,
    /// The widths last handed to the grid, from the left.
    pushed: Vec<f64>,
}

impl Default for ListColumns {
    fn default() -> Self {
        ListColumns {
            order: (0..SEED_COLUMNS.len()).collect(),
            by_hand: None,
            pushed: Vec::new(),
        }
    }
}

impl ListColumns {
    /// The widths to hand the grid for a list `data_width` wide, from the
    /// left in the order the columns are drawn, or nothing when it already
    /// has them. Asked on every draw, it hands over nothing while an edge
    /// is dragged: the width fitted to has not changed, so the drag is not
    /// undone under the pointer.
    fn widths_to_push(&mut self, data_width: f64) -> Option<Vec<f64>> {
        let by_column = match &self.by_hand {
            Some(by_hand) => by_hand.clone(),
            None => fit_seed_widths(data_width),
        };
        let widths: Vec<f64> = self.order.iter().map(|&col| by_column[col]).collect();
        if widths == self.pushed {
            return None;
        }
        self.pushed = widths.clone();
        Some(widths)
    }

    /// An edge was dragged, and `widths`, from the left, are the grid's now.
    fn dragged(&mut self, widths: Vec<f64>) {
        let mut by_column = vec![0.0; self.order.len()];
        for (&col, &width) in self.order.iter().zip(&widths) {
            by_column[col] = width;
        }
        self.pushed = widths;
        self.by_hand = Some(by_column);
    }

    /// A heading was dropped somewhere else, and `order` is the grid's now.
    fn moved(&mut self, order: Vec<usize>) {
        if order.len() == self.order.len() {
            self.order = order;
        }
    }

    /// Every column back in its own place; the grid is handed the order on
    /// the next draw.
    fn put_back(&mut self) {
        self.order = (0..self.order.len()).collect();
    }

    /// The columns named in the order they are drawn, for the log line.
    fn said(&self) -> String {
        let names: Vec<&str> = self.order.iter().map(|&col| SEED_COLUMNS[col]).collect();
        format!("columns now {}", names.join(", "))
    }
}

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

/// Move `item` to the first line, or to the last.
fn move_to_end(order: &mut Vec<usize>, item: usize, to_top: bool) {
    let Some(from) = order.iter().position(|&i| i == item) else {
        return;
    };
    order.remove(from);
    if to_top {
        order.insert(0, item);
    } else {
        order.push(item);
    }
}

/// The line a carried row on line `from` belongs on when the pointer is
/// over `gap`, the gap between lines the grid's `row_gap_at` names: that
/// gap, or one line less when it is past `from`, since the gap counts the
/// carried row itself.
fn carried_to(from: usize, gap: usize) -> usize {
    if gap > from { gap - 1 } else { gap }
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
    #[live]
    color_carried: Vec4f,
    /// Which item each line draws. Filled the first time it is asked for.
    #[rust]
    order: Vec<usize>,
    #[rust]
    picks: Picks,
    /// The item being carried, from the grid's word that a carry began
    /// until the button comes up.
    #[rust]
    carrying: Option<usize>,
    /// The item a row menu was opened on, until one of its rows is chosen.
    #[rust]
    menu_item: Option<usize>,
    #[rust]
    columns: ListColumns,
}

impl StoryDataGridList {
    fn order(&mut self) -> &[usize] {
        if self.order.len() != SEEDS.len() {
            self.order = (0..SEEDS.len()).collect();
        }
        &self.order
    }

    /// The grid says the row on `line` is being carried.
    fn carry(&mut self, line: usize) {
        self.carrying = self.order().get(line).copied();
    }

    /// The carried row's pointer is at `abs`, or the rows scrolled under
    /// it: move the row into the gap the grid says the pointer is over.
    /// Returns whether anything moved.
    fn follow(&mut self, grid: &DataGridRef, abs: DVec2) -> bool {
        let Some(item) = self.carrying else {
            return false;
        };
        let (Some(gap), Some(from)) = (grid.row_gap_at(abs), self.order.iter().position(|&i| i == item)) else {
            return false;
        };
        let to = carried_to(from, gap);
        if to == from {
            return false;
        }
        let item = self.order.remove(from);
        self.order.insert(to, item);
        true
    }

    /// The carried row was let go. Returns the line it is on, where it
    /// already is: there is nothing left to move.
    fn let_go(&mut self) -> Option<usize> {
        let item = self.carrying.take()?;
        self.order.iter().position(|&i| i == item)
    }

    /// A row was clicked. Returns what to say about the picks now.
    fn press(&mut self, line: usize, held: KeyModifiers) -> String {
        self.order();
        self.picks.press(&self.order, line, held);
        self.picks_said()
    }

    /// A heading was pressed and the sort moved on. Returns what to say
    /// about it.
    fn sort(&mut self, col: usize, ascending: Option<bool>) -> String {
        self.order = sorted_seeds(col, ascending);
        let name = SEED_COLUMNS.get(col).copied().unwrap_or("?");
        match ascending {
            Some(true) => format!("sorted by {name}, up"),
            Some(false) => format!("sorted by {name}, down"),
            None => "back to the order the seeds were written in".to_string(),
        }
    }

    /// The right button went down on `line`: the menu about to open is for
    /// the item drawn there. Returns that item's name.
    fn menu_on(&mut self, line: usize) -> Option<&'static str> {
        self.menu_item = self.order().get(line).copied();
        self.menu_item.map(|item| SEEDS[item][0])
    }

    /// A row of one of this page's menus was chosen. Returns what to say
    /// about it, for the log line.
    fn menu_chosen(&mut self, id: LiveId) -> Option<String> {
        self.order();
        if id == live_id!(to_top) || id == live_id!(to_bottom) {
            let item = self.menu_item.take()?;
            let to_top = id == live_id!(to_top);
            move_to_end(&mut self.order, item, to_top);
            let end = if to_top { "top" } else { "bottom" };
            Some(format!("moved {} to the {end}", SEEDS[item][0]))
        } else if id == live_id!(pick_all) {
            self.picks.items = self.order.iter().copied().collect();
            Some("picked every row".to_string())
        } else if id == live_id!(pick_none) {
            self.picks.items.clear();
            Some("picked no rows".to_string())
        } else if id == live_id!(columns_back) {
            self.columns.put_back();
            Some(self.columns.said())
        } else {
            None
        }
    }

    /// What to say about the picks, in the order the lines are drawn.
    fn picks_said(&self) -> String {
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
            grid.set_header_tips(SEED_TIPS.iter().map(|s| s.to_string()).collect());
            let lines = self.order().len();
            grid.set_grid_size(lines, SEED_COLUMNS.len());
            // The seed names stay first. The order is this page's and is
            // handed over on every draw; the one the grid already has
            // changes nothing.
            grid.set_unmovable_cols(vec![0]);
            grid.set_col_order(cx, &self.columns.order);
            // Measured for this frame already, and set before the first
            // cell, so a list that changes width is fitted in the same
            // frame.
            if let Some(widths) = self.columns.widths_to_push(grid.data_width()) {
                grid.set_col_widths(cx, &widths);
            }
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
            // one frame. The carried row wears its own colour.
            let (marked, color) = match self.carrying {
                Some(item) => (Some(item), self.color_carried),
                None => (self.picks.cursor, self.color_cursor),
            };
            if let Some(line) = marked.and_then(|m| self.order.iter().position(|&i| i == m)) {
                grid.outline_row(line, color);
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
                if let Some(mut inner) = host.borrow_mut::<StoryDataGridList>() {
                    inner.carry(row);
                }
                // The order is being made by hand now, so no heading is lit.
                grid.set_sort_indicator(None);
                host.redraw(cx);
                root.label(cx, ids!(list.log)).set_text(cx, &format!("carrying row {row}"));
            }
            // The carried row's pointer moved, wherever it went, or the
            // list scrolled under it at an edge: the row goes into the gap
            // the pointer is over, and the other rows make way.
            DataGridAction::RowDragMoved { abs } => {
                let moved = host
                    .borrow_mut::<StoryDataGridList>()
                    .is_some_and(|mut inner| inner.follow(&grid, abs));
                if moved {
                    host.redraw(cx);
                }
            }
            // Let go: the row is already where it lands.
            DataGridAction::RowDragEnded { .. } => {
                let line = host
                    .borrow_mut::<StoryDataGridList>()
                    .and_then(|mut inner| inner.let_go());
                host.redraw(cx);
                if let Some(line) = line {
                    root.label(cx, ids!(list.log)).set_text(cx, &format!("dropped on row {line}"));
                }
            }
            // An edge was dragged: the widths are the person's from now on,
            // and the page stops fitting them.
            DataGridAction::ColumnResized { .. } => {
                let widths = grid.col_widths();
                if let Some(mut inner) = host.borrow_mut::<StoryDataGridList>() {
                    inner.columns.dragged(widths);
                }
                root.label(cx, ids!(list.log)).set_text(cx, "column widths set by hand");
            }
            // A heading was dropped somewhere else: the grid has moved the
            // column and its width, and the order is kept here from now on.
            DataGridAction::ColumnOrderChanged { order } => {
                let said = host.borrow_mut::<StoryDataGridList>().map(|mut inner| {
                    inner.columns.moved(order);
                    inner.columns.said()
                });
                if let Some(said) = said {
                    root.label(cx, ids!(list.log)).set_text(cx, &said);
                }
            }
            // The right button: a menu at the pointer, and nothing else.
            // The picks are left alone until a row of the menu is chosen.
            DataGridAction::CellContextMenu { row, abs, .. } => {
                let name = host
                    .borrow_mut::<StoryDataGridList>()
                    .and_then(|mut inner| inner.menu_on(row));
                if let Some(name) = name {
                    let rows = vec![
                        MenuRow::section(name),
                        MenuRow::new(live_id!(to_top), "Move to the top"),
                        MenuRow::new(live_id!(to_bottom), "Move to the bottom"),
                    ];
                    let at = Rect { pos: abs, size: dvec2(0.0, 0.0) };
                    root.menu_layer(cx, ids!(menus)).open(cx, live_id!(seed_row), rows, at, MenuPlace::At);
                    root.label(cx, ids!(list.log)).set_text(cx, &format!("menu on row {row}"));
                }
            }
            DataGridAction::HeaderContextMenu { col, abs } => {
                let name = SEED_COLUMNS.get(col).copied().unwrap_or("?");
                let rows = vec![
                    MenuRow::section(name),
                    MenuRow::new(live_id!(pick_all), "Pick every row"),
                    MenuRow::new(live_id!(pick_none), "Pick no rows"),
                    MenuRow::new(live_id!(columns_back), "Put the columns back"),
                ];
                let at = Rect { pos: abs, size: dvec2(0.0, 0.0) };
                root.menu_layer(cx, ids!(menus)).open(cx, live_id!(seed_heading), rows, at, MenuPlace::At);
                root.label(cx, ids!(list.log)).set_text(cx, &format!("menu on the {name} heading"));
            }
            _ => {}
        }
    }
    for action in menu_actions(actions) {
        let MenuAction::Picked { owner, id } = action else {
            continue;
        };
        if *owner != live_id!(seed_row) && *owner != live_id!(seed_heading) {
            continue;
        }
        let said = host
            .borrow_mut::<StoryDataGridList>()
            .and_then(|mut inner| inner.menu_chosen(*id).map(|said| (said, inner.picks_said())));
        if let Some((said, picked)) = said {
            if *owner == live_id!(seed_row) {
                grid.set_sort_indicator(None);
            }
            host.redraw(cx);
            root.label(cx, ids!(list.log)).set_text(cx, &said);
            root.label(cx, ids!(list.picked)).set_text(cx, &picked);
        }
    }
    // The grid lights the heading and says which way; the seeds are put
    // in that order here.
    if let Some((col, ascending)) = grid.sort_changed(actions) {
        let said = host
            .borrow_mut::<StoryDataGridList>()
            .map(|mut inner| inner.sort(col, ascending));
        if let Some(said) = said {
            host.redraw(cx);
            root.label(cx, ids!(list.log)).set_text(cx, &said);
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
    Control { label: "Drag to scroll", target: "demo.grid", kind: ControlKind::Bool { prop: "drag_scrolling", default: false } },
    Control { label: "Header align", target: "demo.grid", kind: ControlKind::Number { prop: "header_align", min: 0., max: 1., step: 0.25, default: 0.5 } },
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

Its own: column and row headers, resizing a column or a row by dragging its edge, reordering columns by dragging a header (`allow_col_reorder`, off by default while both resize flags are on; the List page keeps the order it reports), the whole selection model — single cell, rectangle, row, column, everything — and keyboard navigation with arrows, the page keys, Home, End, and shift to extend.

What a press selects is `selection:`. `GridSelectMode.Cells`, the default, is the spreadsheet described here. `GridSelectMode.Rows` picks whole rows with a press or an arrow key, and a heading press only sorts. `GridSelectMode.Off` selects nothing at all and still reports every press with its modifiers, for a list that keeps its own picks; the List page beside this one is that.

`drag_scrolling: true` makes a press on a cell that travels past `drag_threshold` scroll the grid with the pointer, both ways, as a finger scrolls a list on a phone, instead of dragging out a selection or carrying a row. The point pressed stays under the pointer, the scroll stops at the ends, and the press is no click, so it raises no `CellReleased`. It is off by default; the Drag to scroll control turns it on here, and `set_drag_scrolling` turns it on or off from code, for a list that scrolls under a finger in one layout and not in another.

How the headings look is declared too: `header_align` places the labels, from the left at 0 to the right at 1 (the Header align control), `color_header_sorted` lights the sorted heading, `draw_text_header` gives the headings a face of their own, and `cell_cursor` is the pointer over a cell. The List page uses all four.

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

**Declare `rows: 0`.** `set_grid_size` returns early when the counts already match, and the early return skips the geometry pass. Declare the real count in the DSL and your first `set_grid_size` does nothing, so a width set in the same frame lands a frame late with nothing scheduling that frame. Every caller here declares zero. `set_col_widths`, which sets all of them at once, measures the frame again itself and lands in the frame it is called from.

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

With `row_drag: true`, a press that travels past the threshold raises `RowDragStarted { row, col, abs, modifiers }` once, with `abs` where the pointer is as the carry begins. From then on the grid leaves that press alone: it selects nothing, does not scroll at the edge unless `row_drag_scroll` asks it to, and raises no `CellReleased` when the press comes up. Where the row goes is the host's to follow. Without `row_drag` a drag across cells drags out a selection, as it always has, and is not a click either.

## Moving a carried row

The grid never reorders anything: the rows are the host's. This page keeps a list of lines, each naming an item, and moves one entry of it while a row is carried, so the other rows make way under the pointer and letting go leaves the row where it already is. The grid reports the whole carry, wherever the pointer goes: `RowDragStarted` as it begins, `RowDragMoved { abs }` for every move after that, and `RowDragEnded { abs, modifiers }` when the row is let go. On each move the page asks one thing:

- `row_gap_at(abs)` — the gap the row would drop into, from 0 in front of the first row to `rows` behind the last: in front of the row level with the pointer while the pointer is above that row's middle, behind it once it is below. The row moves from its line to that gap, or to one line less when the gap is past it, since the gap counts the carried row itself. Halfway points keep rows of different heights from trading places under a pointer that holds still. Only the height counts: under the last row is the gap behind it, and a pointer that is not over the rows, over the headings or above, below or beside the grid, has no gap, so the row stays where it is and a carry that strays over something else moves nothing.

With `row_drag_scroll: true`, a carry within a row's height inside the top or bottom edge of the rows scrolls the list that way, faster the nearer the edge, and keeps scrolling while the pointer holds still. Taken off the rows, the carry stops the scroll. Each step raises `RowDragMoved` with the pointer where it is, since another row is under it now, so the one handler that follows the pointer follows the scroll too. It is off by default: a list whose rows are carried out of it to somewhere else must not scroll.

`row_at(abs)`, the row level with the pointer, and `row_rect(row)`, where a row is drawn, answer the same question in parts for a host with a rule of its own. All three are measured against the last drawn frame and the scroll as it is now, and all three are on `DataGridRef`, as are `set_scroll` and `scroll_pos`, and `set_row_height` and `clear_row_heights` for a host whose rows are not all one height. The picks and the outline follow the item, not the line, so they move with it. The carried row wears `outline_row` in a colour of its own, which is all it takes to show which row is in the hand.

## A menu on the right button

A secondary press on a cell raises `CellContextMenu { row, col, abs }`, and on a column heading `HeaderContextMenu { col, abs }`, with `abs` where it went down and `col` the data column. Only the secondary button asks: Ctrl with the primary button is a primary press, because that is how a list toggles its picks. The press selects nothing, sorts nothing and takes no keyboard focus, so a menu opened on a row finds the picks as they were; a secondary press while a row is being carried or a column dragged belongs to that gesture and asks for nothing. This page opens a `MenuLayer` menu at `abs`: on a row, moving it to either end; on a heading, picking every row or none, or putting the columns back in their own order.

## Headings and the pointer

Four declarations change how the headings and the pointer look, and each leaves the stock look alone until it is declared:

- `header_align` places a heading's label from `0.0`, the left, to `1.0`, the right; centred unless declared. Past the middle the label keeps clear of the sort marks, and at `1.0` it ends before them. Row numbers stay centred.
- `color_header_sorted` is the colour of the sorted column's label and marks. Transparent, the default, draws that heading in `color_header_text` like the rest.
- `draw_text_header` is the headings' text: labels, marks and row numbers. With a font size of 0, the default, the headings are drawn with `draw_text`, so a grid that restyles its cells restyles its headings too. Give it a size, `draw_text_header +: {text_style: theme.font_bold{font_size: 8.5}}`, and the headings get that face and size. The colour still comes from the two heading colours.
- `cell_cursor` is the pointer over a cell, `MouseCursor.Default` unless declared. A column edge that can be dragged, and a heading that can be carried, keep their own pointers.

This list declares all four: left-aligned headings in a bold face, the sorted one in the text colour against the quieter rest, and a hand over the rows. It is also `sortable`, and sorts its own seeds when `SortChanged` arrives, as the Overview page does. Carrying a row, or moving one from its menu, makes the order a hand-made one, so the page clears the heading with `set_sort_indicator(None)`.

## Tips on the headings

`set_header_tips(Vec<String>)` gives each column heading a tip, by data column as `set_col_labels` does, so a tip stays with its column when the column is dragged elsewhere. An empty string, or a column past the end of the list, has no tip. When the pointer comes to rest on a heading that has one, the grid raises `TipAction::HoverIn` with the text and the heading's rectangle, cut to the part that shows; when the pointer leaves that heading, for a heading without a tip, a cell or outside the grid, it raises `TipAction::HoverOut`. Those are the reports a `Tip` wrapper makes, and the window's one `TipLayer` does the dwell, the placement and the drawing, so a page with no `TipLayer` shows nothing. A grid given no tips raises neither. This page declares its `TipLayer` last.

## Moving columns

With `allow_col_reorder: true` a heading dragged past `drag_threshold` follows the pointer along the headings, and a bar across the headings and rows shows the gap it will drop into: in front of the heading under the pointer left of that heading's middle, behind it right of it. Dropped, the column moves there and takes its width with it. The grid raises `ColumnMoved { from_display, to_display }` and then `ColumnOrderChanged { order }`, where `order` is the data column drawn at each position from the left. A heading that travels less than the threshold is a press, and sorts as one.

`set_unmovable_cols(Vec<usize>)` names data columns that keep their place: their headings do not drag, and no other column can be dropped on the far side of one, so a column of badges stays first and a column of controls stays last. This page keeps Seed first.

The order is the host's to keep. `col_order()` reads it and `set_col_order(cx, &order)` puts it back: it must name every column once, or it is refused and changes nothing; widths go with their columns and a selection is cleared, as for a drop; the order the grid already has changes nothing, so it can be pushed on every draw; and like `set_col_widths` it lands in the frame it is called from. Changing the number of columns puts them back in their own order. This page keeps the order it is told with its widths, hands it back on every draw, and puts the columns back from the heading menu.

## Column widths set by the host

`set_col_widths(cx, &[f64])` sets every column's width at once, in display order. Columns past the end of the list go back to `default_col_width`, and a width below `min_col_width` is raised to it. The grid measures the frame again straight away, so widths set from the draw loop before the first `next_cell` land in the frame being drawn, where one `set_col_width` lands in the next. `data_width()` is the width the columns share in that frame, and `col_widths()` reads every width back.

This page keeps its widths in one place, with the column order, and by column rather than by position, so a width stays with its column wherever the column is dragged. Until an edge is dragged it fits them to `data_width()`, with the Seed column taking what the others leave. When `ColumnResized` arrives it keeps `col_widths()` and stops fitting. It hands the grid widths only when they differ from the last ones it handed over: pushed on every draw, the fit would put a dragged edge back under the pointer. The widths and the order last as long as the page is open, and saving them would be writing that one value out and reading it back.

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
        let addressed: [(&[LiveId], &[&[LiveId]], &[&[LiveId]]); 2] = [
            (ids!(demo.grid), &[ids!(demo.reported), ids!(demo.edited)], &[]),
            (ids!(list.grid), &[ids!(list.picked), ids!(list.log)], &[ids!(menus), ids!(tips)]),
        ];
        for (story, (path, labels, layers)) in STORIES.iter().zip(addressed) {
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
            for layer in layers {
                assert!(!page.widget(&cx, layer).is_empty(), "{}: no layer {layer:?}", story.key);
            }
            let subject = page.widget(&cx, &[LiveId::from_str(story.subject)]);
            assert!(!subject.is_empty(), "{}: no {}", story.key, story.subject);
        }
    }

    /// The row menu moves its item to either end and leaves every other
    /// line in the order it had.
    #[test]
    fn the_row_menu_moves_its_item_to_either_end() {
        let mut order = vec![4, 2, 7, 1];
        move_to_end(&mut order, 7, true);
        assert_eq!(order, vec![7, 4, 2, 1]);
        move_to_end(&mut order, 7, false);
        assert_eq!(order, vec![4, 2, 1, 7]);
        move_to_end(&mut order, 9, true);
        assert_eq!(order, vec![4, 2, 1, 7], "an item that is not there moves nothing");
    }

    /// A heading press on the list orders the seeds by that column, months
    /// by the calendar and days by number, turns round on the second press
    /// and goes back to the order they were written in on the third.
    #[test]
    fn the_list_sorts_months_by_the_calendar_and_days_by_number() {
        let days = |order: Vec<usize>| -> Vec<u32> { order.iter().map(|i| SEEDS[*i][3].parse().unwrap()).collect() };
        let up = days(sorted_seeds(3, Some(true)));
        assert!(up.windows(2).all(|w| w[0] <= w[1]), "{up:?}");
        let mut down = days(sorted_seeds(3, Some(false)));
        down.reverse();
        assert_eq!(up, down);
        let months: Vec<usize> = sorted_seeds(2, Some(true))
            .iter()
            .map(|i| MONTHS.iter().position(|m| *m == SEEDS[*i][2]).expect("a month the list does not know"))
            .collect();
        assert!(months.windows(2).all(|w| w[0] <= w[1]), "{months:?}");
        assert_eq!(sorted_seeds(0, None), (0..SEEDS.len()).collect::<Vec<_>>());
    }

    /// The first column fills what the others leave, and stops shrinking
    /// at its minimum.
    #[test]
    fn the_first_column_takes_what_the_others_leave() {
        let rest: f64 = SEED_WIDTHS.iter().sum();
        let wide = fit_seed_widths(rest + 400.0);
        assert_eq!(wide, vec![400.0, SEED_WIDTHS[0], SEED_WIDTHS[1], SEED_WIDTHS[2]]);
        assert_eq!(wide.iter().sum::<f64>(), rest + 400.0, "the list ends at its edge");
        assert_eq!(fit_seed_widths(rest + 10.0)[0], SEED_FIRST_MIN);
    }

    /// The fit is handed over when the width changes and not otherwise,
    /// and once an edge is dragged the dragged widths stay, whatever width
    /// the list is.
    #[test]
    fn a_dragged_edge_ends_the_fit() {
        let mut columns = ListColumns::default();
        assert_eq!(columns.widths_to_push(700.0), Some(fit_seed_widths(700.0)));
        assert_eq!(columns.widths_to_push(700.0), None, "the same fit twice, or a drag undone");
        assert_eq!(columns.widths_to_push(800.0), Some(fit_seed_widths(800.0)));
        let dragged = vec![300.0, 80.0, 100.0, 70.0];
        columns.dragged(dragged.clone());
        assert_eq!(columns.widths_to_push(800.0), None, "the grid already has the dragged widths");
        assert_eq!(columns.widths_to_push(500.0), None, "a narrower list fitted over a drag");
        assert_eq!(columns.by_hand, Some(dragged));
    }

    /// Widths belong to their columns. Moved, the columns are handed over
    /// in their new places with their own widths; an edge dragged in that
    /// order is kept for its column; and put back, every column is where
    /// it started with the width it has now.
    #[test]
    fn widths_go_with_their_columns_wherever_they_are_drawn() {
        let mut columns = ListColumns::default();
        let fitted = fit_seed_widths(700.0);
        columns.widths_to_push(700.0);
        columns.moved(vec![0, 3, 1, 2]);
        assert_eq!(columns.said(), "columns now Seed, Days, Kind, Sow");
        assert_eq!(
            columns.widths_to_push(700.0),
            Some(vec![fitted[0], fitted[3], fitted[1], fitted[2]])
        );
        // Days, drawn second, dragged to 90.
        columns.dragged(vec![fitted[0], 90.0, fitted[1], fitted[2]]);
        assert_eq!(columns.by_hand, Some(vec![fitted[0], fitted[1], fitted[2], 90.0]));
        columns.put_back();
        assert_eq!(columns.order, vec![0, 1, 2, 3]);
        assert_eq!(columns.widths_to_push(700.0), Some(vec![fitted[0], fitted[1], fitted[2], 90.0]));
        columns.moved(vec![1, 0]);
        assert_eq!(columns.order, vec![0, 1, 2, 3], "an order for other columns was kept");
    }

    /// A gap in front of the carried row, or right behind it, leaves the
    /// row where it is; a gap further down takes it to the line before
    /// that gap, since the gap counts the row itself; one further up takes
    /// it to that gap; and the gap behind the last line takes it to the
    /// last line.
    #[test]
    fn a_carried_row_goes_into_the_gap_it_is_over() {
        assert_eq!(carried_to(2, 2), 2);
        assert_eq!(carried_to(2, 3), 2, "the gap right behind it");
        assert_eq!(carried_to(2, 4), 3);
        assert_eq!(carried_to(2, 0), 0);
        assert_eq!(carried_to(2, 1), 1);
        let lines = 24;
        assert_eq!(carried_to(5, lines), lines - 1);
    }

    /// Carried down a list, one gap at a time and then in one jump, the
    /// row moves and every other line keeps its order.
    #[test]
    fn a_carry_moves_one_line_and_keeps_the_rest_in_order() {
        let mut order: Vec<usize> = (0..6).collect();
        let mut from = 1;
        for gap in [2, 3, 4, 6] {
            let to = carried_to(from, gap);
            let item = order.remove(from);
            order.insert(to, item);
            from = to;
        }
        assert_eq!(order, vec![0, 2, 3, 4, 5, 1]);
        assert_eq!(from, 5);
    }
}
