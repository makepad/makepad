//! A working spreadsheet on top of DataGrid: formulas, editing, a formula
//! bar, formatting toolbar, clipboard copy, resizable rows/columns.

use crate::sheet_engine::{ref_name, CellValue, Sheet};
use makepad_widgets::*;

const ROWS: usize = 500;
const COLS: usize = 52;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ToolBtn = Button{
        width: 30 height: 26
        margin: 0 padding: 0
        align: Align{x: 0.5 y: 0.5}
    }

    mod.widgets.SheetsTabBase = #(SheetsTab::register_widget(vm))
    mod.widgets.SheetsTab = set_type_default() do mod.widgets.SheetsTabBase{
        width: Fill height: Fill
        flow: Down

        toolbar := View{
            width: Fill height: 38
            flow: Right spacing: 4
            padding: Inset{left: 8, right: 8, top: 6, bottom: 6}
            align: Align{y: 0.5}

            bold_btn := ToolBtn{text: "B" draw_text +: {text_style: theme.font_bold{font_size: 10.0}}}
            align_l := ToolBtn{text: "L"}
            align_c := ToolBtn{text: "C"}
            align_r := ToolBtn{text: "R"}
            View{width: 10 height: Fill}
            sw_none := ToolBtn{text: "×"}
            sw_yellow := ToolBtn{text: "" draw_bg +: {color: #xffefad color_hover: #xf5db70}}
            sw_green := ToolBtn{text: "" draw_bg +: {color: #xcdf0cd color_hover: #x9fd89f}}
            sw_blue := ToolBtn{text: "" draw_bg +: {color: #xcfdff7 color_hover: #xa9c7f0}}
            sw_pink := ToolBtn{text: "" draw_bg +: {color: #xf7d5df color_hover: #xf0aabf}}
            View{width: 16 height: Fill}
            hint := Label{
                text: "Type in cells · =SUM(B4:B9) · drag header edges to resize · click headers to select · ⌘C copies TSV"
                draw_text +: {color: #x888888 text_style +: {font_size: 8.5}}
            }
        }

        formula_bar := View{
            width: Fill height: 32
            flow: Right spacing: 6
            padding: Inset{left: 8, right: 8, top: 3, bottom: 3}
            align: Align{y: 0.5}

            cell_name := Label{
                width: 56
                text: "A1"
                draw_text +: {color: #xcccccc text_style: theme.font_bold{font_size: 9.0}}
            }
            Label{
                text: "fx"
                draw_text +: {color: #x999999 text_style +: {font_size: 9.0}}
            }
            formula_input := TextInput{
                width: Fill height: 26
                empty_text: "Enter a value or =formula"
            }
        }

        grid := DataGrid{
            width: Fill height: Fill
            rows: 500
            cols: 52
            default_col_width: 96.0
            default_row_height: 26.0

            Editor := TextInput{
                width: Fill height: Fill
                margin: 0
                padding: Inset{left: 4, right: 4, top: 5, bottom: 3}
                draw_bg +: {
                    border_radius: 0.
                    border_size: 2.0
                    border_color: #x1a73e8
                    border_color_hover: #x1a73e8
                    border_color_focus: #x1a73e8
                    color: #ffffff
                    color_hover: #ffffff
                    color_focus: #ffffff
                }
                draw_text +: {
                    text_style +: {font_size: 9.0}
                    color: #x202020
                    color_hover: #x202020
                    color_focus: #x202020
                    color_down: #x202020
                }
            }
        }
    }
}

const PALETTE: [Vec4f; 5] = [
    Vec4f {
        x: 1.0,
        y: 1.0,
        z: 1.0,
        w: 0.0,
    },
    Vec4f {
        x: 1.0,
        y: 0.937,
        z: 0.678,
        w: 1.0,
    },
    Vec4f {
        x: 0.804,
        y: 0.941,
        z: 0.804,
        w: 1.0,
    },
    Vec4f {
        x: 0.812,
        y: 0.875,
        z: 0.969,
        w: 1.0,
    },
    Vec4f {
        x: 0.969,
        y: 0.835,
        z: 0.875,
        w: 1.0,
    },
];

fn demo_sheet() -> Sheet {
    let mut s = Sheet::default();
    let mut set = |cell: &str, v: &str| {
        let (r, c) = crate::sheet_engine::parse_ref(cell).unwrap();
        s.set_input(r, c, v);
    };
    set("A1", "Quarterly Budget");
    set("A3", "Month");
    set("B3", "Revenue");
    set("C3", "Costs");
    set("D3", "Profit");
    set("E3", "Margin");
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"];
    let revenue = [120500, 132800, 128400, 145200, 158900, 171300];
    let costs = [88400, 91200, 91850, 91400, 94800, 98100];
    for i in 0..6 {
        let row = 4 + i;
        set(&format!("A{row}"), months[i]);
        set(&format!("B{row}"), &revenue[i].to_string());
        set(&format!("C{row}"), &costs[i].to_string());
        set(&format!("D{row}"), &format!("=B{row}-C{row}"));
        set(&format!("E{row}"), &format!("=ROUND(D{row}/B{row}*100,1)&\"%\""));
    }
    set("A10", "TOTAL");
    set("B10", "=SUM(B4:B9)");
    set("C10", "=SUM(C4:C9)");
    set("D10", "=B10-C10");
    set("E10", "=ROUND(D10/B10*100,1)&\"%\"");
    set("A12", "Best month");
    set("B12", "=MAX(D4:D9)");
    set("A13", "Average profit");
    set("B13", "=ROUND(AVG(D4:D9))");
    set("G3", "Notes");
    set("G4", "Edit any cell: type, or press Enter / F2.");
    set("G5", "Formulas start with = and update live.");
    set("G6", "Try =SUM(B4:C9) or =B10^0.5");

    // formats
    let bold_cells = ["A1", "A3", "B3", "C3", "D3", "E3", "A10", "B10", "C10", "D10", "E10", "G3"];
    for cell in bold_cells {
        let (r, c) = crate::sheet_engine::parse_ref(cell).unwrap();
        s.format_mut(r, c).bold = true;
    }
    for c in 0..5 {
        s.format_mut(2, c).bg = 3;
        s.format_mut(9, c).bg = 1;
    }
    s
}

#[derive(Script, ScriptHook, Widget)]
pub struct SheetsTab {
    #[deref]
    view: View,
    #[rust(demo_sheet())]
    sheet: Sheet,
    #[rust]
    editing: Option<(usize, usize)>,
    /// Some(text): the editor needs seeding + focus on next draw.
    #[rust]
    edit_seed: Option<String>,
}

impl SheetsTab {
    fn grid(&self, cx: &Cx) -> DataGridRef {
        self.view.data_grid(cx, ids!(grid))
    }

    fn start_edit(&mut self, cx: &mut Cx, row: usize, col: usize, replace: Option<String>) {
        // commit any previous edit first
        if self.editing.is_some() {
            self.commit_current(cx);
        }
        self.editing = Some((row, col));
        self.edit_seed = Some(match replace {
            Some(text) => text,
            None => self.sheet.input(row, col).to_string(),
        });
        let grid = self.grid(cx);
        grid.set_selection(
            cx,
            Some(GridSelection::single(row, col)),
        );
        grid.scroll_cell_into_view(cx, row, col);
        grid.redraw(cx);
    }

    fn commit_current(&mut self, cx: &mut Cx) {
        // Commit whatever the live editor currently holds (click-away commit).
        if let Some((row, col)) = self.editing {
            if let Some((_, widget)) = self.grid(cx).get_item(row, col) {
                let text = widget.as_text_input().text();
                self.sheet.set_input(row, col, &text);
            }
        }
        self.editing = None;
        self.edit_seed = None;
        self.sync_formula_bar(cx);
        self.grid(cx).redraw(cx);
    }

    fn commit_edit(&mut self, cx: &mut Cx, row: usize, col: usize, text: &str, move_down: bool) {
        self.sheet.set_input(row, col, text);
        self.editing = None;
        self.edit_seed = None;
        let grid = self.grid(cx);
        if move_down {
            let next = (row + 1).min(ROWS - 1);
            grid.set_selection(cx, Some(GridSelection::single(next, col)));
            grid.scroll_cell_into_view(cx, next, col);
        }
        self.sync_formula_bar(cx);
        self.refresh_copy_provider(cx);
        grid.redraw(cx);
    }

    fn cancel_edit(&mut self, cx: &mut Cx) {
        self.editing = None;
        self.edit_seed = None;
        self.grid(cx).redraw(cx);
    }

    fn sync_formula_bar(&mut self, cx: &mut Cx) {
        let grid = self.grid(cx);
        if let Some((row, col)) = grid.active_cell() {
            self.view
                .label(cx, ids!(cell_name))
                .set_text(cx, &ref_name(row, col));
            self.view
                .text_input(cx, ids!(formula_input))
                .set_text(cx, self.sheet.input(row, col));
        }
    }

    fn selection_cells(&self, cx: &Cx) -> Vec<(usize, usize)> {
        let Some(sel) = self.grid(cx).selection() else {
            return Vec::new();
        };
        let (mut r0, mut r1) = sel.row_range();
        let (mut c0, mut c1) = sel.col_range();
        match sel.kind {
            GridSelectKind::All => {
                r0 = 0;
                r1 = ROWS - 1;
                c0 = 0;
                c1 = COLS - 1;
            }
            GridSelectKind::Rows => {
                c0 = 0;
                c1 = COLS - 1;
            }
            GridSelectKind::Cols => {
                r0 = 0;
                r1 = ROWS - 1;
            }
            GridSelectKind::Cells => (),
        }
        let mut out = Vec::new();
        for r in r0..=r1.min(r0 + 199) {
            for c in c0..=c1.min(c0 + 199) {
                out.push((r, c));
            }
        }
        out
    }

    fn apply_format(&mut self, cx: &mut Cx, f: impl Fn(&mut crate::sheet_engine::CellFormat)) {
        for (r, c) in self.selection_cells(cx) {
            f(self.sheet.format_mut(r, c));
        }
        self.grid(cx).redraw(cx);
    }

    fn clear_selection_cells(&mut self, cx: &mut Cx) {
        for (r, c) in self.selection_cells(cx) {
            self.sheet.set_input(r, c, "");
        }
        self.sync_formula_bar(cx);
        self.grid(cx).redraw(cx);
    }

    fn refresh_copy_provider(&mut self, cx: &mut Cx) {
        // Materialize the current selection as TSV so clipboard-copy can be
        // answered synchronously by the grid.
        let cells = self.selection_cells(cx);
        if cells.is_empty() {
            return;
        }
        let (r0, r1) = cells.iter().fold((usize::MAX, 0), |a, (r, _)| {
            (a.0.min(*r), a.1.max(*r))
        });
        let (c0, c1) = cells.iter().fold((usize::MAX, 0), |a, (_, c)| {
            (a.0.min(*c), a.1.max(*c))
        });
        let mut tsv = String::new();
        for r in r0..=r1 {
            for c in c0..=c1 {
                if c > c0 {
                    tsv.push('\t');
                }
                tsv.push_str(&self.sheet.value(r, c).display());
            }
            tsv.push('\n');
        }
        self.grid(cx)
            .set_copy_provider(Box::new(move |_sel| tsv.clone()));
    }
}

impl Widget for SheetsTab {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                grid.set_grid_size(ROWS, COLS);
                while let Some(cell) = grid.next_cell(cx) {
                    if Some((cell.row, cell.col)) == self.editing {
                        if let Some(item) = grid.item(cx, cell.row, cell.col, id!(Editor)) {
                            let seed = self.edit_seed.take();
                            if let Some(seed) = &seed {
                                item.as_text_input().set_text(cx, seed);
                            }
                            grid.draw_item(cx, &cell, &item, None);
                            // focus only once the editor has a drawn area
                            if seed.is_some() {
                                item.as_text_input().take_key_focus(cx);
                            }
                        }
                        continue;
                    }
                    let value = self.sheet.value(cell.row, cell.col);
                    if matches!(value, CellValue::Empty) {
                        let fmt = self.sheet.format(cell.row, cell.col);
                        let bg = if fmt.bg > 0 { Some(PALETTE[fmt.bg]) } else { None };
                        grid.cell_text_styled(
                            cx,
                            &cell,
                            "",
                            CellStyle {
                                bg,
                                ..CellStyle::default()
                            },
                        );
                        continue;
                    }
                    let fmt = self.sheet.format(cell.row, cell.col);
                    let align = fmt.align.unwrap_or(match &value {
                        CellValue::Num(_) => 1.0,
                        CellValue::Err(_) => 0.5,
                        _ => 0.0,
                    });
                    let color = match &value {
                        CellValue::Err(_) => Some(vec4(0.85, 0.2, 0.2, 1.0)),
                        _ => None,
                    };
                    let bg = if fmt.bg > 0 { Some(PALETTE[fmt.bg]) } else { None };
                    grid.cell_text_styled(
                        cx,
                        &cell,
                        &value.display(),
                        CellStyle {
                            bg,
                            color,
                            align,
                            bold: fmt.bold,
                            font_scale: 1.0,
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
        let grid = self.grid(cx);

        for action in grid.actions(actions) {
            match action {
                DataGridAction::EditCell { row, col, replace } => {
                    self.start_edit(cx, row, col, replace);
                }
                DataGridAction::CellDoubleClicked { row, col } => {
                    self.start_edit(cx, row, col, None);
                }
                DataGridAction::CellClicked { .. } => {
                    if self.editing.is_some() {
                        self.commit_current(cx);
                    }
                }
                DataGridAction::SelectionChanged { .. } => {
                    self.sync_formula_bar(cx);
                    self.refresh_copy_provider(cx);
                }
                DataGridAction::ClearCells => {
                    self.clear_selection_cells(cx);
                }
                _ => (),
            }
        }

        // in-cell editor commit / cancel
        for (row, col, widget) in grid.cell_widgets_with_actions(actions) {
            let ti = widget.as_text_input();
            if let Some((text, _mods)) = ti.returned(actions) {
                self.commit_edit(cx, row, col, &text, true);
            } else if ti.escaped(actions) {
                self.cancel_edit(cx);
            }
        }

        // formula bar commit
        if let Some((text, _mods)) = self
            .view
            .text_input(cx, ids!(formula_input))
            .returned(actions)
        {
            if let Some((row, col)) = grid.active_cell() {
                self.commit_edit(cx, row, col, &text, false);
            }
        }

        // toolbar
        if self.view.button(cx, ids!(bold_btn)).clicked(actions) {
            let target = self
                .grid(cx)
                .active_cell()
                .map(|(r, c)| !self.sheet.format(r, c).bold)
                .unwrap_or(true);
            self.apply_format(cx, move |f| f.bold = target);
        }
        if self.view.button(cx, ids!(align_l)).clicked(actions) {
            self.apply_format(cx, |f| f.align = Some(0.0));
        }
        if self.view.button(cx, ids!(align_c)).clicked(actions) {
            self.apply_format(cx, |f| f.align = Some(0.5));
        }
        if self.view.button(cx, ids!(align_r)).clicked(actions) {
            self.apply_format(cx, |f| f.align = Some(1.0));
        }
        let swatches = [
            (ids!(sw_none), 0),
            (ids!(sw_yellow), 1),
            (ids!(sw_green), 2),
            (ids!(sw_blue), 3),
            (ids!(sw_pink), 4),
        ];
        for (id, idx) in swatches {
            if self.view.button(cx, id).clicked(actions) {
                self.apply_format(cx, move |f| f.bg = idx);
            }
        }
    }
}
