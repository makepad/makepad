//! One million rows × one thousand columns — a billion virtual cells.
//! Values are procedural (hashed from row/col), so nothing is stored; the
//! grid only ever touches what is on screen. Click a header to sort the
//! full million rows, drag headers to reorder, drag edges to resize.

use makepad_widgets::*;

const ROWS: usize = 1_000_000;
const COLS: usize = 1_000;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.BigDataTabBase = #(BigDataTab::register_widget(vm))
    mod.widgets.BigDataTab = set_type_default() do mod.widgets.BigDataTabBase{
        width: Fill height: Fill
        flow: Down

        grid := DataGrid{
            width: Fill height: Fill
            rows: 100
            cols: 100
            zebra_stripes: true
            allow_col_reorder: true
            default_col_width: 110.0
            default_row_height: 24.0
            row_header_width: 76.0
        }

        status := View{
            width: Fill height: 26
            flow: Right spacing: 12
            padding: Inset{left: 10, right: 10, top: 5, bottom: 5}
            align: Align{y: 0.5}

            status_label := Label{
                text: "1,000,000 rows × 1,000 columns = 1,000,000,000 virtual cells"
                draw_text +: {color: #xbbbbbb text_style +: {font_size: 8.5}}
            }
            sort_label := Label{
                text: "click a column header to sort all 1M rows · drag headers to reorder · drag edges to resize"
                draw_text +: {color: #x888899 text_style +: {font_size: 8.5}}
            }
        }
    }
}

fn hash2(row: u64, col: u64) -> u64 {
    let mut x = row
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(col.wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(0x94d0_49bb_1331_11eb);
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

const FIRST: [&str; 12] = [
    "Ada", "Linus", "Grace", "Alan", "Edsger", "Barbara", "Donald", "Margaret", "Ken", "Dennis",
    "Radia", "Vint",
];
const LAST: [&str; 12] = [
    "Hopper", "Kay", "Lovelace", "Turing", "Dijkstra", "Liskov", "Knuth", "Hamilton", "Thompson",
    "Ritchie", "Perlman", "Cerf",
];
const CITIES: [&str; 10] = [
    "Amsterdam",
    "Tokyo",
    "Berlin",
    "Lisbon",
    "Oslo",
    "Seoul",
    "Toronto",
    "Austin",
    "Zurich",
    "Kyoto",
];

/// Numeric sort key for a cell; the rendered text is derived from the same
/// hash, so sorting by this key sorts what the user sees.
fn value_num(row: usize, col: usize) -> f64 {
    let h = hash2(row as u64, col as u64);
    match col % 6 {
        0 => row as f64,
        1 => (h % (FIRST.len() * LAST.len()) as u64) as f64,
        2 => (h % CITIES.len() as u64) as f64,
        3 => ((h % 2_000_000) as f64 / 100.0) - 10000.0,
        4 => (h % 1000) as f64 / 10.0,
        _ => (h % 2) as f64,
    }
}

fn value_text(row: usize, col: usize) -> String {
    let h = hash2(row as u64, col as u64);
    match col % 6 {
        0 => format!("{}", row),
        1 => {
            let i = (h % (FIRST.len() * LAST.len()) as u64) as usize;
            format!("{} {}", FIRST[i % FIRST.len()], LAST[i / FIRST.len()])
        }
        2 => CITIES[(h % CITIES.len() as u64) as usize].to_string(),
        3 => {
            let v = ((h % 2_000_000) as f64 / 100.0) - 10000.0;
            format!("{:.2}", v)
        }
        4 => format!("{:.1}%", (h % 1000) as f64 / 10.0),
        _ => {
            if h % 2 == 0 {
                "yes".to_string()
            } else {
                "no".to_string()
            }
        }
    }
}

fn col_labels() -> Vec<String> {
    let base = ["#", "Name", "City", "Balance", "Score", "Active"];
    (0..COLS)
        .map(|c| {
            if c < 6 {
                base[c].to_string()
            } else {
                format!("{}·{}", base[c % 6], c / 6)
            }
        })
        .collect()
}

#[derive(Script, ScriptHook, Widget)]
pub struct BigDataTab {
    #[deref]
    view: View,
    #[rust]
    initialized: bool,
    /// (data col, ascending)
    #[rust]
    sort: Option<(usize, bool)>,
    /// view row -> data row (only when sorted)
    #[rust]
    perm: Option<Vec<u32>>,
}

impl BigDataTab {
    fn data_row(&self, view_row: usize) -> usize {
        match &self.perm {
            Some(perm) => perm[view_row] as usize,
            None => view_row,
        }
    }

    fn resort(&mut self, cx: &mut Cx) {
        let grid = self.view.data_grid(cx, ids!(grid));
        match self.sort {
            None => {
                self.perm = None;
                grid.set_sort_indicator(None);
                self.view
                    .label(cx, ids!(sort_label))
                    .set_text(cx, "click a column header to sort all 1M rows");
            }
            Some((col, asc)) => {
                let t0 = std::time::Instant::now();
                let mut perm: Vec<u32> = (0..ROWS as u32).collect();
                perm.sort_unstable_by(|a, b| {
                    let ka = value_num(*a as usize, col);
                    let kb = value_num(*b as usize, col);
                    let ord = ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal);
                    if asc {
                        ord
                    } else {
                        ord.reverse()
                    }
                });
                let ms = t0.elapsed().as_millis();
                self.perm = Some(perm);
                grid.set_sort_indicator(Some((col, asc)));
                self.view.label(cx, ids!(sort_label)).set_text(
                    cx,
                    &format!(
                        "sorted 1,000,000 rows by \"{}\" {} in {} ms",
                        col_labels()[col],
                        if asc { "ascending" } else { "descending" },
                        ms
                    ),
                );
            }
        }
        grid.redraw(cx);
    }
}

impl Widget for BigDataTab {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                if !self.initialized {
                    self.initialized = true;
                    grid.set_col_labels(col_labels());
                    grid.set_col_width(0, 70.0);
                }
                grid.set_grid_size(ROWS, COLS);
                while let Some(cell) = grid.next_cell(cx) {
                    let row = self.data_row(cell.row);
                    let text = value_text(row, cell.col);
                    let kind = cell.col % 6;
                    let align = match kind {
                        0 | 3 | 4 => 1.0,
                        _ => 0.0,
                    };
                    let color = match kind {
                        3 => {
                            if text.starts_with('-') {
                                Some(vec4(0.8, 0.2, 0.2, 1.0))
                            } else {
                                Some(vec4(0.1, 0.5, 0.25, 1.0))
                            }
                        }
                        0 => Some(vec4(0.55, 0.55, 0.55, 1.0)),
                        _ => None,
                    };
                    grid.cell_text_styled(
                        cx,
                        &cell,
                        &text,
                        CellStyle {
                            align,
                            color,
                            ..CellStyle::default()
                        },
                    );
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        let Event::Actions(actions) = event else {
            return;
        };
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            match action {
                DataGridAction::HeaderClicked { col, .. } => {
                    self.sort = match self.sort {
                        Some((c, true)) if c == col => Some((col, false)),
                        Some((c, false)) if c == col => None,
                        _ => Some((col, true)),
                    };
                    self.resort(cx);
                }
                DataGridAction::Scrolled | DataGridAction::SelectionChanged { .. } => {
                    let (vr, vc) = grid.visible_counts();
                    let extra = match grid.active_cell() {
                        Some((row, col)) => {
                            format!(" · active {}R×{}C", self.data_row(row), col)
                        }
                        None => String::new(),
                    };
                    self.view.label(cx, ids!(status_label)).set_text(
                        cx,
                        &format!(
                            "1,000,000 rows × 1,000 columns = 1,000,000,000 virtual cells · drawing {}×{} = {} cells{}",
                            vr,
                            vc,
                            vr * vc,
                            extra
                        ),
                    );
                }
                _ => (),
            }
        }
    }
}
