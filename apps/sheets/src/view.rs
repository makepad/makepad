//! The spreadsheet surface: the grid, the formula bar, the toolbar, the sheet
//! tabs and the status bar, plus every gesture that ties them together.
//!
//! The stock `DataGrid` widget already owns cell drawing, scrolling, selection,
//! arrow/Tab/Enter navigation, resizing and clipboard *copy*. What lives here
//! is everything above that: which text a cell shows, the in-cell editor, the
//! fill handle, paste, undo/redo, formatting and the chrome.

use crate::docs;
use crate::formula::{Value, FUNCTIONS};
use crate::sheet::{self, HAlign, NumFormat, Pos, Workbook};
use crate::theme;
use makepad_widgets::*;

const ROWS: usize = 1000;
const COLS: usize = 64;
/// Fixed sheet-tab slots in the splash; sheets beyond this are not shown.
const MAX_TABS: usize = 8;
/// A bulk operation (select-all, then bold) is capped so the UI cannot stall.
const MAX_BULK: usize = 20_000;
/// Must match `default_col_width` in the splash below.
const DEFAULT_COL_WIDTH: f64 = 96.0;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // SolidView, not View: a plain View's draw_bg is a bare DrawQuad with no
    // colour, so `show_bg` alone paints nothing.
    let Sep = SolidView{
        width: 1 height: 16
        margin: Inset{left: 3 right: 3}
        draw_bg +: {color: mod.sheets.muted}
    }

    // Flat, square, no bevel: every `*_2*` slot is the "no gradient" sentinel
    // and every state paints one solid colour.
    let TBtn = Button{
        height: 24 width: Fit
        margin: 0
        padding: Inset{left: 7 right: 7 top: 3 bottom: 3}
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            border_radius: uniform(0.0)
            border_size: uniform(1.0)
            color_dither: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            color: uniform(mod.sheets.bg_light)
            color_hover: uniform(mod.sheets.muted)
            color_down: uniform(mod.sheets.accent)
            color_focus: uniform(mod.sheets.bg_light)
            color_disabled: uniform(mod.sheets.bg)

            color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))

            border_color: uniform(mod.sheets.muted)
            border_color_hover: uniform(mod.sheets.accent)
            border_color_down: uniform(mod.sheets.accent)
            border_color_focus: uniform(mod.sheets.muted)
            border_color_disabled: uniform(mod.sheets.muted)

            border_color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
        }
        draw_text +: {
            color: mod.sheets.fg
            color_hover: mod.sheets.fg_bright
            text_style: theme.font_regular{font_size: 8.5}
        }
    }

    let SheetTabBtn = TBtn{
        height: 20
        padding: Inset{left: 10 right: 10 top: 2 bottom: 2}
    }

    let FieldInput = TextInput{
        height: 22
        margin: 0
        padding: Inset{left: 6 right: 5 top: 3 bottom: 2}
        draw_bg +: {
            border_radius: uniform(0.0)
            border_size: uniform(1.0)
            color_dither: uniform(0.0)
            gradient_border_horizontal: uniform(0.0)
            gradient_fill_horizontal: uniform(0.0)

            // One flat fill in every state — no inset gradient, no gloss.
            // `color` is an instance field on TextInput, not a uniform.
            color: mod.sheets.bg_dark
            color_hover: uniform(mod.sheets.bg_dark)
            color_focus: uniform(mod.sheets.bg_dark)
            color_down: uniform(mod.sheets.bg_dark)
            color_empty: uniform(mod.sheets.bg_dark)
            color_disabled: uniform(mod.sheets.bg_dark)

            color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_empty: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))

            border_color: uniform(mod.sheets.muted)
            border_color_hover: uniform(mod.sheets.fg_dark)
            border_color_focus: uniform(mod.sheets.accent)
            border_color_down: uniform(mod.sheets.accent)
            border_color_empty: uniform(mod.sheets.muted)
            border_color_disabled: uniform(mod.sheets.muted)

            border_color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_empty: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
            border_color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
        }
        draw_text +: {
            text_style: TextStyle{
                font_family: FontFamily{
                    latin := FontMember{
                        res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                        asc: 0.0 desc: 0.0 weight: 400.0
                    }
                }
                font_size: 9.0
                line_spacing: 1.2
            }
            color: mod.sheets.fg_bright
            color_hover: mod.sheets.fg_bright
            color_focus: mod.sheets.fg_bright
            color_down: mod.sheets.fg_bright
            color_empty: mod.sheets.fg_dark
        }
    }

    mod.widgets.MpSheetsBase = #(MpSheets::register_widget(vm))
    mod.widgets.MpSheets = set_type_default() do mod.widgets.MpSheetsBase{
        width: Fill height: Fill
        flow: Down

        toolbar := SolidView{
            width: Fill height: 32
            flow: Right spacing: 3
            padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
            align: Align{y: 0.5}
            draw_bg +: {color: mod.sheets.bg_dark}

            new_btn := TBtn{text: "New"}
            disk_controls := View{
                width: Fit height: Fit
                flow: Right spacing: 3
                open_btn := TBtn{text: "Open"}
                save_btn := TBtn{text: "Save"}
                path_input := FieldInput{
                    width: 190
                    empty_text: "sheet.csv"
                }
            }
            demo_pick := View{
                width: Fit height: Fit
                visible: false
                demo_picker := DropDown{
                    width: 180 height: 24
                    labels: ["Open demo..."]
                }
            }
            Sep{}
            undo_btn := TBtn{text: "Undo"}
            redo_btn := TBtn{text: "Redo"}
            Sep{}
            bold_btn := TBtn{
                text: "B"
                draw_text +: {text_style: theme.font_bold{font_size: 9.5}}
            }
            italic_btn := TBtn{
                text: "I"
                draw_text +: {text_style: theme.font_italic{font_size: 9.5}}
            }
            Sep{}
            align_l := TBtn{text: "L"}
            align_c := TBtn{text: "C"}
            align_r := TBtn{text: "R"}
            Sep{}
            fmt_gen := TBtn{text: "Gen"}
            fmt_dec := TBtn{text: "0.00"}
            fmt_thou := TBtn{text: "1,000"}
            fmt_pct := TBtn{text: "%"}
            Sep{}
            fx_menu := DropDown{
                width: 108 height: 24
                labels: ["fx"]
                draw_bg +: {
                    border_radius: uniform(0.0)
                    border_size: uniform(1.0)
                    color_dither: uniform(0.0)
                    gradient_border_horizontal: uniform(0.0)
                    gradient_fill_horizontal: uniform(0.0)

                    color: uniform(mod.sheets.bg_light)
                    color_hover: uniform(mod.sheets.muted)
                    color_down: uniform(mod.sheets.muted)
                    color_focus: uniform(mod.sheets.bg_light)
                    color_disabled: uniform(mod.sheets.bg)

                    color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))

                    border_color: uniform(mod.sheets.muted)
                    border_color_hover: uniform(mod.sheets.accent)
                    border_color_down: uniform(mod.sheets.accent)
                    border_color_focus: uniform(mod.sheets.muted)
                    border_color_disabled: uniform(mod.sheets.muted)

                    border_color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                }
                draw_text +: {
                    text_style: theme.font_regular{font_size: 8.5}
                    color: mod.sheets.fg
                }
            }
        }

        formula_bar := SolidView{
            width: Fill height: 28
            flow: Right spacing: 6
            padding: Inset{left: 6 right: 6 top: 3 bottom: 3}
            align: Align{y: 0.5}
            draw_bg +: {color: mod.sheets.bg}

            name_box := FieldInput{width: 76}
            fx_label := Label{
                text: "fx"
                draw_text +: {
                    color: mod.sheets.fg_dark
                    text_style: theme.font_italic{font_size: 9.5}
                }
            }
            formula_input := FieldInput{
                width: Fill
                empty_text: "Enter a value, or = to start a formula"
            }
        }

        grid := DataGrid{
            width: Fill height: Fill
            rows: 1000
            cols: 64
            default_col_width: 96.0
            default_row_height: 22.0
            col_header_height: 22.0
            row_header_width: 48.0
            cell_pad_x: 6.0
            zebra_stripes: false

            color_bg: mod.sheets.bg
            color_cell: mod.sheets.bg
            color_cell_alt: mod.sheets.bg
            color_text: mod.sheets.fg
            color_header: mod.sheets.bg_light
            color_header_active: mod.sheets.muted
            color_header_text: mod.sheets.fg_bright
            color_selection: mod.sheets.sel_fill
            color_selection_border: mod.sheets.accent
            color_drag_marker: mod.sheets.accent
            color_resize_guide: mod.sheets.accent_ghost

            draw_cell +: {
                border_color: uniform(mod.sheets.muted)
                border_size: uniform(1.0)
            }
            draw_text +: {
                text_style: TextStyle{
                    font_family: FontFamily{
                        latin := FontMember{
                            res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                            asc: 0.0 desc: 0.0 weight: 400.0
                        }
                    }
                    font_size: 9.0
                    line_spacing: 1.2
                }
                color: mod.sheets.fg
            }
            draw_text_bold +: {
                text_style: TextStyle{
                    font_family: FontFamily{
                        latin := FontMember{
                            res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                            asc: 0.0 desc: 0.0 weight: 700.0
                        }
                    }
                    font_size: 9.0
                    line_spacing: 1.2
                }
                color: mod.sheets.fg_bright
            }

            Editor := TextInput{
                width: Fill height: Fill
                margin: 0
                padding: Inset{left: 5 right: 4 top: 3 bottom: 2}
                draw_bg +: {
                    border_radius: uniform(0.0)
                    border_size: uniform(2.0)
                    color_dither: uniform(0.0)
                    gradient_border_horizontal: uniform(0.0)
                    gradient_fill_horizontal: uniform(0.0)

                    color: mod.sheets.bg_dark
                    color_hover: uniform(mod.sheets.bg_dark)
                    color_focus: uniform(mod.sheets.bg_dark)
                    color_down: uniform(mod.sheets.bg_dark)
                    color_empty: uniform(mod.sheets.bg_dark)
                    color_disabled: uniform(mod.sheets.bg_dark)

                    color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    color_2_empty: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))

                    border_color: uniform(mod.sheets.accent)
                    border_color_hover: uniform(mod.sheets.accent)
                    border_color_focus: uniform(mod.sheets.accent)
                    border_color_down: uniform(mod.sheets.accent)
                    border_color_empty: uniform(mod.sheets.accent)
                    border_color_disabled: uniform(mod.sheets.accent)

                    border_color_2: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_hover: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_focus: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_down: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_empty: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                    border_color_2_disabled: uniform(vec4(-1.0 -1.0 -1.0 -1.0))
                }
                draw_text +: {
                    text_style: TextStyle{
                        font_family: FontFamily{
                            latin := FontMember{
                                res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                                asc: 0.0 desc: 0.0 weight: 400.0
                            }
                        }
                        font_size: 9.0
                        line_spacing: 1.2
                    }
                    color: mod.sheets.fg_bright
                    color_hover: mod.sheets.fg_bright
                    color_focus: mod.sheets.fg_bright
                    color_down: mod.sheets.fg_bright
                }
            }

            // Italic cells cannot go through the grid's two-font fast path,
            // so they are drawn as hosted labels instead.
            ItalicCell := Label{
                width: Fill height: Fill
                padding: Inset{left: 6 right: 6 top: 4 bottom: 2}
                draw_text +: {
                    text_style: theme.font_italic{font_size: 9.0}
                    color: mod.sheets.fg
                }
            }
            BoldItalicCell := Label{
                width: Fill height: Fill
                padding: Inset{left: 6 right: 6 top: 4 bottom: 2}
                draw_text +: {
                    text_style: theme.font_bold_italic{font_size: 9.0}
                    color: mod.sheets.fg_bright
                }
            }
        }

        tabbar := SolidView{
            width: Fill height: 26
            flow: Right spacing: 3
            padding: Inset{left: 6 right: 6 top: 3 bottom: 3}
            align: Align{y: 0.5}
            draw_bg +: {color: mod.sheets.bg_dark}

            tab0 := SheetTabBtn{text: "Sheet1"}
            tab1 := SheetTabBtn{text: "" visible: false}
            tab2 := SheetTabBtn{text: "" visible: false}
            tab3 := SheetTabBtn{text: "" visible: false}
            tab4 := SheetTabBtn{text: "" visible: false}
            tab5 := SheetTabBtn{text: "" visible: false}
            tab6 := SheetTabBtn{text: "" visible: false}
            tab7 := SheetTabBtn{text: "" visible: false}
            add_tab := SheetTabBtn{text: "+"}
            Sep{}
            rename_input := FieldInput{width: 118 height: 20 empty_text: "rename sheet"}
            del_tab := SheetTabBtn{text: "Delete"}
        }

        statusbar := SolidView{
            width: Fill height: 22
            flow: Right spacing: 12
            padding: Inset{left: 8 right: 8}
            align: Align{y: 0.5}
            draw_bg +: {color: mod.sheets.bg_light}

            status_msg := Label{
                width: Fill
                draw_text +: {
                    color: mod.sheets.fg_dark
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
            status_stats := Label{
                width: Fit
                draw_text +: {
                    color: mod.sheets.fg
                    text_style: TextStyle{
                        font_family: FontFamily{
                            latin := FontMember{
                                res: crate_resource("self:../../widgets/resources/jetbrains_mono_variable.ttf")
                                asc: 0.0 desc: 0.0 weight: 400.0
                            }
                        }
                        font_size: 8.5
                        line_spacing: 1.2
                    }
                }
            }
        }
    }
}

/// One visible cell's absolute rectangle, recorded while drawing so that the
/// fill handle can be hit-tested against the same geometry the user sees.
#[derive(Clone, Copy)]
pub struct CellRect {
    row: usize,
    display_col: usize,
    rect: Rect,
}

/// An in-progress fill-handle drag.
#[derive(Clone, Copy)]
pub struct FillDrag {
    s0: Pos,
    s1: Pos,
    cur: Pos,
}

fn initial_workbook() -> Workbook {
    docs::docs().initial()
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpSheets {
    #[deref]
    view: View,
    #[live]
    draw_fill: DrawColor,

    #[rust(initial_workbook())]
    wb: Workbook,
    #[rust]
    editing: Option<Pos>,
    /// Some(text): the in-cell editor needs seeding and focus on the next draw.
    #[rust]
    edit_seed: Option<String>,
    #[rust]
    fill: Option<FillDrag>,
    #[rust]
    cell_rects: Vec<CellRect>,
    #[rust]
    handle_rect: Option<Rect>,
    #[rust]
    status: String,
    #[rust]
    chrome_synced: bool,
    #[rust]
    widths_applied: bool,
}

impl MpSheets {
    fn grid(&self, cx: &Cx) -> DataGridRef {
        self.view.data_grid(cx, ids!(grid))
    }

    fn grid_area(&self, cx: &mut Cx) -> Area {
        self.view.widget(cx, ids!(grid)).area()
    }

    fn active(&self, cx: &Cx) -> Pos {
        self.grid(cx).active_cell().unwrap_or((0, 0))
    }

    // -- editing -----------------------------------------------------------

    fn start_edit(&mut self, cx: &mut Cx, pos: Pos, replace: Option<String>) {
        if self.editing.is_some() {
            self.commit_live_editor(cx);
        }
        self.editing = Some(pos);
        self.edit_seed = Some(match replace {
            Some(text) => text,
            None => self.wb.sheet().input(pos).to_string(),
        });
        let grid = self.grid(cx);
        grid.set_selection(cx, Some(GridSelection::single(pos.0, pos.1)));
        grid.scroll_cell_into_view(cx, pos.0, pos.1);
        grid.redraw(cx);
    }

    /// Commit whatever the live editor holds right now (a click elsewhere).
    fn commit_live_editor(&mut self, cx: &mut Cx) {
        if let Some(pos) = self.editing {
            if let Some((_, widget)) = self.grid(cx).get_item(pos.0, pos.1) {
                let text = widget.as_text_input().text();
                self.wb.set_input(pos, &text);
            }
        }
        self.editing = None;
        self.edit_seed = None;
        self.sync_chrome(cx);
        self.grid(cx).redraw(cx);
    }

    fn commit_edit(&mut self, cx: &mut Cx, pos: Pos, text: &str, step: (isize, isize)) {
        self.wb.set_input(pos, text);
        self.editing = None;
        self.edit_seed = None;
        if step != (0, 0) {
            let row = (pos.0 as isize + step.0).clamp(0, ROWS as isize - 1) as usize;
            let col = (pos.1 as isize + step.1).clamp(0, COLS as isize - 1) as usize;
            let grid = self.grid(cx);
            grid.set_selection(cx, Some(GridSelection::single(row, col)));
            grid.scroll_cell_into_view(cx, row, col);
        }
        self.status = format!("{} = {}", sheet::pos_name(pos), self.wb.sheet().display(pos));
        self.sync_chrome(cx);
        self.grid(cx).redraw(cx);
    }

    fn cancel_edit(&mut self, cx: &mut Cx) {
        self.editing = None;
        self.edit_seed = None;
        self.grid(cx).redraw(cx);
    }

    // -- selection ---------------------------------------------------------

    /// Every cell in the current selection, clamped so a select-all cannot
    /// turn a formatting click into a million writes.
    fn selection_cells(&self, cx: &Cx) -> Vec<Pos> {
        let Some(sel) = self.grid(cx).selection() else {
            return Vec::new();
        };
        let (mut r0, mut r1) = sel.row_range();
        let (mut c0, mut c1) = sel.col_range();
        match sel.kind {
            GridSelectKind::All => {
                r0 = 0;
                c0 = 0;
                r1 = ROWS - 1;
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
        // For whole-row/column/sheet selections, only the used range can
        // meaningfully be touched.
        if !matches!(sel.kind, GridSelectKind::Cells) {
            if let Some(((ur0, uc0), (ur1, uc1))) = self.wb.sheet().used_range() {
                r0 = r0.max(ur0.min(r1));
                c0 = c0.max(uc0.min(c1));
                r1 = r1.min(ur1.max(r0));
                c1 = c1.min(uc1.max(c0));
            } else {
                return Vec::new();
            }
        }
        let mut out = Vec::new();
        'outer: for r in r0..=r1 {
            for c in c0..=c1 {
                out.push((r, c));
                if out.len() >= MAX_BULK {
                    break 'outer;
                }
            }
        }
        out
    }

    /// What the assistant is told about the sheet on screen (`sheets.summary`):
    /// its name, its used range, the header row and the selection.
    pub fn ai_summary(&self, cx: &Cx) -> String {
        let sheet = self.wb.sheet();
        let mut out = format!(
            "Sheet \"{}\" ({} of {})",
            sheet.name,
            self.wb.active + 1,
            self.wb.sheets.len()
        );
        match sheet.used_range() {
            Some(((r0, c0), (r1, c1))) => {
                out.push_str(&format!(
                    ", used {}:{} ({} rows × {} columns).",
                    sheet::pos_name((r0, c0)),
                    sheet::pos_name((r1, c1)),
                    r1 - r0 + 1,
                    c1 - c0 + 1
                ));
                let header: Vec<String> = (c0..=c1).take(26).map(|c| sheet.display((r0, c))).collect();
                out.push_str(&format!(" Header row: {}.", header.join(" | ")));
            }
            None => out.push_str(", empty."),
        }
        let ((sr0, sc0), (sr1, sc1)) = self.selection_rect(cx);
        out.push_str(&format!(
            " Selection: {}:{}.",
            sheet::pos_name((sr0, sc0)),
            sheet::pos_name((sr1, sc1))
        ));
        out
    }

    fn selection_rect(&self, cx: &Cx) -> (Pos, Pos) {
        match self.grid(cx).selection() {
            Some(sel) => {
                let (r0, r1) = sel.row_range();
                let (c0, c1) = sel.col_range();
                ((r0, c0), (r1, c1))
            }
            None => ((0, 0), (0, 0)),
        }
    }

    // -- chrome ------------------------------------------------------------

    fn sync_chrome(&mut self, cx: &mut Cx) {
        let pos = self.active(cx);
        self.view
            .text_input(cx, ids!(name_box))
            .set_text(cx, &sheet::pos_name(pos));
        self.view
            .text_input(cx, ids!(formula_input))
            .set_text(cx, self.wb.sheet().input(pos));

        // status: selection statistics
        let cells = self.selection_cells(cx);
        let stats = self.wb.sheet().stats(cells.iter().copied());
        let mut line = String::new();
        if stats.numeric > 0 {
            line.push_str(&format!(
                "Sum {}   Avg {}   ",
                crate::formula::format_general(stats.sum),
                crate::formula::format_general(stats.average().unwrap_or(0.0))
            ));
        }
        line.push_str(&format!("Count {}", stats.count));
        self.view
            .label(cx, ids!(status_stats))
            .set_text(cx, &line);

        let msg = if self.status.is_empty() {
            format!(
                "{} | {} cell{} selected | Cmd/Ctrl+C copy | Cmd/Ctrl+V paste | Cmd/Ctrl+Z undo | drag the corner handle to fill",
                self.wb.sheet().name,
                cells.len(),
                if cells.len() == 1 { "" } else { "s" }
            )
        } else {
            self.status.clone()
        };
        self.view.label(cx, ids!(status_msg)).set_text(cx, &msg);

        // Undo/Redo read as available only when they are.
        let c = theme::colors();
        for (id, live) in [
            (ids!(undo_btn), self.wb.can_undo()),
            (ids!(redo_btn), self.wb.can_redo()),
        ] {
            let col = if live { c.fg } else { c.fg_dark };
            let mut w = self.view.widget(cx, id);
            script_apply_eval!(cx, w, {
                draw_text +: {color: #(col)}
            });
        }

        self.sync_tabs(cx);
        self.refresh_copy_provider(cx);
    }

    fn tab_id(&self, cx: &Cx, i: usize) -> WidgetRef {
        match i {
            0 => self.view.widget(cx, ids!(tab0)),
            1 => self.view.widget(cx, ids!(tab1)),
            2 => self.view.widget(cx, ids!(tab2)),
            3 => self.view.widget(cx, ids!(tab3)),
            4 => self.view.widget(cx, ids!(tab4)),
            5 => self.view.widget(cx, ids!(tab5)),
            6 => self.view.widget(cx, ids!(tab6)),
            _ => self.view.widget(cx, ids!(tab7)),
        }
    }

    fn tab_button(&self, cx: &Cx, i: usize) -> ButtonRef {
        match i {
            0 => self.view.button(cx, ids!(tab0)),
            1 => self.view.button(cx, ids!(tab1)),
            2 => self.view.button(cx, ids!(tab2)),
            3 => self.view.button(cx, ids!(tab3)),
            4 => self.view.button(cx, ids!(tab4)),
            5 => self.view.button(cx, ids!(tab5)),
            6 => self.view.button(cx, ids!(tab6)),
            _ => self.view.button(cx, ids!(tab7)),
        }
    }

    fn sync_tabs(&mut self, cx: &mut Cx) {
        let names: Vec<String> = self.wb.sheets.iter().map(|s| s.name.clone()).collect();
        let active = self.wb.active;
        let c = theme::colors();
        for i in 0..MAX_TABS {
            let w = self.tab_id(cx, i);
            match names.get(i) {
                Some(name) => {
                    w.set_visible(cx, true);
                    self.tab_button(cx, i).set_text(cx, name);
                    let (bg, fg) = if i == active {
                        (c.accent, c.bg)
                    } else {
                        (c.bg_light, c.fg)
                    };
                    let mut item = self.tab_id(cx, i);
                    script_apply_eval!(cx, item, {
                        draw_bg +: {color: #(bg)}
                        draw_text +: {color: #(fg)}
                    });
                }
                None => w.set_visible(cx, false),
            }
        }
    }

    fn refresh_copy_provider(&mut self, cx: &mut Cx) {
        let ((r0, c0), (r1, c1)) = self.selection_rect(cx);
        let r1 = r1.min(r0 + 500);
        let c1 = c1.min(c0 + 200);
        let tsv = sheet::to_tsv(self.wb.sheet(), (r0, c0), (r1, c1));
        self.grid(cx)
            .set_copy_provider(Box::new(move |_sel| tsv.clone()));
    }

    // -- commands ----------------------------------------------------------

    fn apply_format(&mut self, cx: &mut Cx, f: impl Fn(&mut sheet::CellFormat) + Copy) {
        let cells = self.selection_cells(cx);
        self.wb.set_format(&cells, f);
        self.sync_chrome(cx);
        self.grid(cx).redraw(cx);
    }

    fn undo(&mut self, cx: &mut Cx) {
        self.status = if self.wb.undo() {
            "Undo".into()
        } else {
            "Nothing to undo".into()
        };
        self.after_model_change(cx);
    }

    fn redo(&mut self, cx: &mut Cx) {
        self.status = if self.wb.redo() {
            "Redo".into()
        } else {
            "Nothing to redo".into()
        };
        self.after_model_change(cx);
    }

    fn after_model_change(&mut self, cx: &mut Cx) {
        self.editing = None;
        self.edit_seed = None;
        self.sync_chrome(cx);
        self.grid(cx).redraw(cx);
        self.view.redraw(cx);
    }

    fn reset_loaded_sheet_view(&mut self, cx: &mut Cx) {
        self.widths_applied = false;
        let grid = self.grid(cx);
        grid.set_selection(cx, Some(GridSelection::single(0, 0)));
        grid.scroll_cell_into_view(cx, 0, 0);
    }

    fn paste_clipboard(&mut self, cx: &mut Cx, text: &str) {
        let rows = sheet::parse_tsv(text);
        if rows.is_empty() {
            return;
        }
        let ((r0, c0), _) = self.selection_rect(cx);
        self.wb.paste_block((r0, c0), &rows);
        let h = rows.len().saturating_sub(1);
        let w = rows.iter().map(|r| r.len()).max().unwrap_or(1).saturating_sub(1);
        self.grid(cx).set_selection(
            cx,
            Some(GridSelection {
                kind: GridSelectKind::Cells,
                anchor: (r0, c0),
                head: ((r0 + h).min(ROWS - 1), (c0 + w).min(COLS - 1)),
            }),
        );
        self.status = format!("Pasted {} x {}", rows.len(), w + 1);
        self.after_model_change(cx);
    }

    fn copy_selection(&mut self, cx: &mut Cx) {
        let ((r0, c0), (r1, c1)) = self.selection_rect(cx);
        let tsv = sheet::to_tsv(
            self.wb.sheet(),
            (r0, c0),
            (r1.min(r0 + 500), c1.min(c0 + 200)),
        );
        docs::copy_text(cx, &tsv);
        self.status = format!("Copied {}:{}", sheet::pos_name((r0, c0)), sheet::pos_name((r1, c1)));
        self.sync_chrome(cx);
    }

    fn csv_path(&self, cx: &mut Cx) -> String {
        let p = self.view.text_input(cx, ids!(path_input)).text();
        let p = p.trim().to_string();
        if p.is_empty() {
            "sheet.csv".to_string()
        } else {
            p
        }
    }

    fn open_csv(&mut self, cx: &mut Cx) {
        let path = self.csv_path(cx);
        match docs::docs().load(&path) {
            Ok(sheet) => {
                self.wb.open_loaded_sheet(sheet);
                self.reset_loaded_sheet_view(cx);
                self.status = format!("Opened {path}");
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
        self.after_model_change(cx);
    }

    fn save_csv(&mut self, cx: &mut Cx) {
        let path = self.csv_path(cx);
        self.status = match docs::docs().save(&path, self.wb.sheet()) {
            Ok(()) => format!("Saved {path}"),
            Err(e) => format!("Save failed: {e}"),
        };
        self.sync_chrome(cx);
    }

    fn open_demo(&mut self, cx: &mut Cx, index: usize) {
        if self.wb.sheets.len() >= MAX_TABS {
            self.status = format!("At most {MAX_TABS} sheets");
            self.sync_chrome(cx);
            return;
        }
        let source = docs::docs();
        let Some(demo) = source.demos().get(index).copied() else {
            return;
        };
        match source.load(demo.id) {
            Ok(sheet) => {
                self.wb.open_loaded_sheet(sheet);
                self.reset_loaded_sheet_view(cx);
                self.status = format!("Loaded {} demo", demo.title);
            }
            Err(e) => self.status = format!("Demo unavailable: {e}"),
        }
        self.after_model_change(cx);
    }

    fn new_sheet_doc(&mut self, cx: &mut Cx) {
        self.wb = Workbook::default();
        self.widths_applied = false;
        self.status = "New workbook".into();
        self.grid(cx).set_selection(cx, Some(GridSelection::single(0, 0)));
        self.after_model_change(cx);
    }

    // -- fill handle -------------------------------------------------------

    fn cell_at(&self, abs: Vec2d) -> Option<Pos> {
        self.cell_rects
            .iter()
            .find(|c| c.rect.contains(abs))
            .map(|c| (c.row, c.display_col))
    }

    /// The fill gesture runs *before* the grid sees the mouse, so dragging the
    /// handle does not start a selection drag instead.
    fn handle_fill_gesture(&mut self, cx: &mut Cx, event: &Event) -> bool {
        match event {
            Event::MouseDown(e) => {
                if self.fill.is_some() || self.editing.is_some() {
                    return false;
                }
                let Some(hr) = self.handle_rect else {
                    return false;
                };
                if !hr.contains(e.abs) {
                    return false;
                }
                let ((r0, c0), (r1, c1)) = self.selection_rect(cx);
                self.fill = Some(FillDrag {
                    s0: (r0, c0),
                    s1: (r1, c1),
                    cur: (r1, c1),
                });
                true
            }
            Event::MouseMove(e) if self.fill.is_some() => {
                if let Some(pos) = self.cell_at(e.abs) {
                    if let Some(f) = &mut self.fill {
                        if f.cur != pos {
                            f.cur = pos;
                            self.view.redraw(cx);
                        }
                    }
                }
                true
            }
            Event::MouseUp(_) if self.fill.is_some() => {
                if let Some(f) = self.fill.take() {
                    self.commit_fill(cx, f);
                }
                true
            }
            _ => false,
        }
    }

    /// The destination a drag to `cur` means: Excel extends along whichever
    /// axis the pointer moved furthest.
    fn fill_dest(f: &FillDrag) -> (Pos, Pos) {
        // How far the pointer reaches beyond each edge of the source block.
        let down = f.cur.0 as isize - f.s1.0 as isize;
        let up = f.s0.0 as isize - f.cur.0 as isize;
        let right = f.cur.1 as isize - f.s1.1 as isize;
        let left = f.s0.1 as isize - f.cur.1 as isize;
        let vertical = down.max(up).max(0);
        let horizontal = right.max(left).max(0);
        if vertical == 0 && horizontal == 0 {
            return (f.s0, f.s1);
        }
        if vertical >= horizontal {
            if down > 0 {
                (f.s0, (f.cur.0, f.s1.1))
            } else {
                ((f.cur.0, f.s0.1), f.s1)
            }
        } else if right > 0 {
            (f.s0, (f.s1.0, f.cur.1))
        } else {
            ((f.s0.0, f.cur.1), f.s1)
        }
    }

    fn commit_fill(&mut self, cx: &mut Cx, f: FillDrag) {
        let dest = Self::fill_dest(&f);
        if dest == (f.s0, f.s1) {
            self.view.redraw(cx);
            return;
        }
        self.wb.fill((f.s0, f.s1), dest);
        self.grid(cx).set_selection(
            cx,
            Some(GridSelection {
                kind: GridSelectKind::Cells,
                anchor: dest.0,
                head: dest.1,
            }),
        );
        self.status = format!(
            "Filled {}:{}",
            sheet::pos_name(dest.0),
            sheet::pos_name(dest.1)
        );
        self.after_model_change(cx);
    }

    // -- keyboard ----------------------------------------------------------

    fn handle_global_keys(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::TextInput(te) if te.was_paste => {
                if self.editing.is_some() {
                    return;
                }
                let area = self.grid_area(cx);
                if cx.has_key_focus(area) {
                    let text = te.input.clone();
                    self.paste_clipboard(cx, &text);
                }
            }
            Event::KeyDown(ke) => {
                let cmd = ke.modifiers.logo || ke.modifiers.control;
                if !cmd {
                    return;
                }
                match ke.key_code {
                    KeyCode::KeyZ if ke.modifiers.shift => self.redo(cx),
                    KeyCode::KeyZ => self.undo(cx),
                    KeyCode::KeyY => self.redo(cx),
                    KeyCode::KeyB => {
                        let pos = self.active(cx);
                        let on = !self.wb.sheet().format(pos).bold;
                        self.apply_format(cx, move |f| f.bold = on);
                    }
                    KeyCode::KeyI => {
                        let pos = self.active(cx);
                        let on = !self.wb.sheet().format(pos).italic;
                        self.apply_format(cx, move |f| f.italic = on);
                    }
                    KeyCode::KeyC => {
                        // The grid answers the platform copy request through
                        // its provider; this is the belt-and-braces path for
                        // platforms that do not raise Hit::TextCopy.
                        if self.editing.is_none() {
                            self.copy_selection(cx);
                        }
                    }
                    _ => (),
                }
            }
            _ => (),
        }
    }
}

impl Widget for MpSheets {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let c = theme::colors();
        self.cell_rects.clear();
        let mut handle = None;
        let editing = self.editing;
        let fill = self.fill;

        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else {
                continue;
            };
            grid.set_grid_size(ROWS, COLS);
            if !self.widths_applied {
                self.widths_applied = true;
                // Every column is written, not just the overridden ones: the
                // grid keeps its own width table, so switching sheets has to
                // clear the previous sheet's columns as well as set this one's.
                let widths: Vec<f64> = (0..COLS)
                    .map(|c| {
                        self.wb
                            .sheet()
                            .col_widths
                            .get(&c)
                            .copied()
                            .unwrap_or(DEFAULT_COL_WIDTH)
                    })
                    .collect();
                for (col, w) in widths.into_iter().enumerate() {
                    grid.set_col_width(col, w);
                }
            }
            let sel = grid.selection();

            while let Some(cell) = grid.next_cell(cx) {
                let pos = (cell.row, cell.col);
                self.cell_rects.push(CellRect {
                    row: cell.row,
                    display_col: cell.display_col,
                    rect: cell.rect,
                });

                if Some(pos) == editing {
                    if let Some(item) = grid.item(cx, cell.row, cell.col, id!(Editor)) {
                        let seed = self.edit_seed.take();
                        if let Some(seed) = &seed {
                            item.as_text_input().set_text(cx, seed);
                        }
                        grid.draw_item(cx, &cell, &item, None);
                        if seed.is_some() {
                            item.as_text_input().take_key_focus(cx);
                        }
                    }
                    continue;
                }

                let value = self.wb.sheet().value(pos);
                let fmt = self.wb.sheet().format(pos);
                if matches!(value, Value::Empty) {
                    grid.cell_text_styled(cx, &cell, "", CellStyle::default());
                    continue;
                }
                let text = self.wb.sheet().display(pos);
                let align = fmt.align.factor(value.is_num());
                let color = match &value {
                    Value::Err(_) => Some(c.red),
                    Value::Bool(_) => Some(c.green),
                    _ => None,
                };

                if fmt.italic {
                    // Hosted-label path: the grid's fast path has no italic.
                    let template = if fmt.bold {
                        id!(BoldItalicCell)
                    } else {
                        id!(ItalicCell)
                    };
                    if let Some(mut item) = grid.item(cx, cell.row, cell.col, template) {
                        let label = item.as_label();
                        label.set_text(cx, &text);
                        let col = color.unwrap_or(if fmt.bold { c.fg } else { c.fg });
                        let ax = align;
                        // Dotted paths only: `Align` is not in scope inside a
                        // script_apply_eval.
                        script_apply_eval!(cx, item, {
                            align.x: #(ax)
                            draw_text +: {color: #(col)}
                        });
                        grid.draw_item(cx, &cell, &item, None);
                        continue;
                    }
                }

                grid.cell_text_styled(
                    cx,
                    &cell,
                    &text,
                    CellStyle {
                        bg: None,
                        color,
                        align,
                        bold: fmt.bold,
                        font_scale: 1.0,
                    },
                );
            }

            // The fill handle sits on the selection's bottom-right corner.
            if let Some(sel) = sel {
                if sel.kind == GridSelectKind::Cells && editing.is_none() {
                    let (_, r1) = sel.row_range();
                    let (_, c1) = sel.col_range();
                    if let Some(cr) = self
                        .cell_rects
                        .iter()
                        .find(|cr| cr.row == r1 && cr.display_col == c1)
                    {
                        let s = 8.0;
                        handle = Some(Rect {
                            pos: dvec2(
                                cr.rect.pos.x + cr.rect.size.x - s * 0.5 - 1.0,
                                cr.rect.pos.y + cr.rect.size.y - s * 0.5 - 1.0,
                            ),
                            size: dvec2(s, s),
                        });
                    }
                }
            }
        }

        // Painted after the grid so the selection overlay cannot cover them.
        if let Some(f) = fill {
            let dest = Self::fill_dest(&f);
            let a = self
                .cell_rects
                .iter()
                .find(|cr| cr.row == dest.0 .0 && cr.display_col == dest.0 .1)
                .map(|cr| cr.rect);
            let b = self
                .cell_rects
                .iter()
                .find(|cr| cr.row == dest.1 .0 && cr.display_col == dest.1 .1)
                .map(|cr| cr.rect);
            if let (Some(a), Some(b)) = (a, b) {
                let r = Rect {
                    pos: a.pos,
                    size: dvec2(
                        b.pos.x + b.size.x - a.pos.x,
                        b.pos.y + b.size.y - a.pos.y,
                    ),
                };
                self.draw_fill.color = c.accent;
                for edge in [
                    Rect { pos: r.pos, size: dvec2(r.size.x, 1.0) },
                    Rect {
                        pos: dvec2(r.pos.x, r.pos.y + r.size.y - 1.0),
                        size: dvec2(r.size.x, 1.0),
                    },
                    Rect { pos: r.pos, size: dvec2(1.0, r.size.y) },
                    Rect {
                        pos: dvec2(r.pos.x + r.size.x - 1.0, r.pos.y),
                        size: dvec2(1.0, r.size.y),
                    },
                ] {
                    self.draw_fill.draw_abs(cx, edge);
                }
            }
        }
        if let Some(r) = handle {
            self.draw_fill.color = c.accent;
            self.draw_fill.draw_abs(cx, r);
        }
        self.handle_rect = handle;

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.handle_fill_gesture(cx, event) {
            return;
        }
        self.handle_global_keys(cx, event);
        self.view.handle_event(cx, event, scope);

        let Event::Actions(actions) = event else {
            return;
        };
        if !self.chrome_synced {
            self.chrome_synced = true;
            let grid = self.grid(cx);
            if grid.selection().is_none() {
                grid.set_selection(cx, Some(GridSelection::single(0, 0)));
            }
            // Entry 0 is the menu's own title; the rest are the functions, in
            // the same order `FUNCTIONS` lists them.
            let mut labels = Vec::with_capacity(FUNCTIONS.len() + 1);
            labels.push("fx".to_string());
            labels.extend(FUNCTIONS.iter().map(|(name, _)| name.to_string()));
            self.view.drop_down(cx, ids!(fx_menu)).set_labels(cx, labels);
            let source = docs::docs();
            let can_save = source.can_save();
            let has_demos = !source.demos().is_empty();
            self.view
                .widget(cx, ids!(disk_controls))
                .set_visible(cx, can_save);
            self.view.widget(cx, ids!(demo_pick)).set_visible(cx, has_demos);
            let mut demo_labels = Vec::with_capacity(source.demos().len() + 1);
            demo_labels.push("Open demo...".to_string());
            demo_labels.extend(source.demos().iter().map(|demo| demo.title.to_string()));
            self.view.drop_down(cx, ids!(demo_picker)).set_labels(cx, demo_labels);
            self.sync_chrome(cx);
        }
        let grid = self.grid(cx);

        for action in grid.actions(actions) {
            match action {
                DataGridAction::EditCell { row, col, replace } => {
                    self.start_edit(cx, (row, col), replace);
                }
                DataGridAction::CellDoubleClicked { row, col } => {
                    self.start_edit(cx, (row, col), None);
                }
                DataGridAction::CellClicked { .. } => {
                    if self.editing.is_some() {
                        self.commit_live_editor(cx);
                    }
                }
                DataGridAction::SelectionChanged { .. } => {
                    self.status.clear();
                    self.sync_chrome(cx);
                }
                DataGridAction::ClearCells => {
                    let cells = self.selection_cells(cx);
                    self.wb.clear_cells(&cells);
                    self.status = format!("Cleared {} cells", cells.len());
                    self.after_model_change(cx);
                }
                DataGridAction::ColumnResized {
                    display_col, width, ..
                } => {
                    self.wb.sheet_mut().col_widths.insert(display_col, width);
                }
                _ => (),
            }
        }

        // in-cell editor: Enter commits and steps down, Escape cancels
        for (row, col, widget) in grid.cell_widgets_with_actions(actions) {
            let ti = widget.as_text_input();
            if let Some((text, mods)) = ti.returned(actions) {
                let step = if mods.shift { (-1, 0) } else { (1, 0) };
                self.commit_edit(cx, (row, col), &text, step);
                let area = self.grid_area(cx);
                cx.set_key_focus(area);
            } else if ti.escaped(actions) {
                self.cancel_edit(cx);
                let area = self.grid_area(cx);
                cx.set_key_focus(area);
            }
        }

        // formula bar
        if let Some((text, _)) = self
            .view
            .text_input(cx, ids!(formula_input))
            .returned(actions)
        {
            let pos = self.active(cx);
            self.commit_edit(cx, pos, &text, (1, 0));
            let area = self.grid_area(cx);
            cx.set_key_focus(area);
        }

        // name box: jump to a cell
        if let Some((text, _)) = self.view.text_input(cx, ids!(name_box)).returned(actions) {
            if let Some(pos) = sheet::name_pos(&text) {
                let pos = (pos.0.min(ROWS - 1), pos.1.min(COLS - 1));
                let grid = self.grid(cx);
                grid.set_selection(cx, Some(GridSelection::single(pos.0, pos.1)));
                grid.scroll_cell_into_view(cx, pos.0, pos.1);
                self.sync_chrome(cx);
                let area = self.grid_area(cx);
                cx.set_key_focus(area);
            }
        }

        // file
        if self.view.button(cx, ids!(new_btn)).clicked(actions) {
            self.new_sheet_doc(cx);
        }
        if self.view.button(cx, ids!(open_btn)).clicked(actions) {
            self.open_csv(cx);
        }
        if self.view.button(cx, ids!(save_btn)).clicked(actions) {
            self.save_csv(cx);
        }
        if let Some(i) = self.view.drop_down(cx, ids!(demo_picker)).selected(actions) {
            if i > 0 {
                self.open_demo(cx, i - 1);
            }
            self.view.drop_down(cx, ids!(demo_picker)).set_selected_item(cx, 0);
        }
        if self.view.button(cx, ids!(undo_btn)).clicked(actions) {
            self.undo(cx);
        }
        if self.view.button(cx, ids!(redo_btn)).clicked(actions) {
            self.redo(cx);
        }

        // formatting
        if self.view.button(cx, ids!(bold_btn)).clicked(actions) {
            let pos = self.active(cx);
            let on = !self.wb.sheet().format(pos).bold;
            self.apply_format(cx, move |f| f.bold = on);
        }
        if self.view.button(cx, ids!(italic_btn)).clicked(actions) {
            let pos = self.active(cx);
            let on = !self.wb.sheet().format(pos).italic;
            self.apply_format(cx, move |f| f.italic = on);
        }
        for (id, a) in [
            (ids!(align_l), HAlign::Left),
            (ids!(align_c), HAlign::Center),
            (ids!(align_r), HAlign::Right),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                self.apply_format(cx, move |f| f.align = a);
            }
        }
        for (id, n) in [
            (ids!(fmt_gen), NumFormat::General),
            (ids!(fmt_dec), NumFormat::Fixed2),
            (ids!(fmt_thou), NumFormat::Thousands),
            (ids!(fmt_pct), NumFormat::Percent),
        ] {
            if self.view.button(cx, id).clicked(actions) {
                self.apply_format(cx, move |f| f.num = n);
            }
        }

        // fx menu: drop the chosen function into the formula bar
        if let Some(i) = self.view.drop_down(cx, ids!(fx_menu)).selected(actions) {
            if i > 0 {
                if let Some((name, help)) = FUNCTIONS.get(i - 1) {
                    let pos = self.active(cx);
                    let existing = self.wb.sheet().input(pos).to_string();
                    let text = if existing.starts_with('=') {
                        format!("{existing}{name}(")
                    } else {
                        format!("={name}(")
                    };
                    self.status = help.to_string();
                    // sync_chrome rewrites the formula bar from the active
                    // cell, so seed the field after it, not before.
                    self.sync_chrome(cx);
                    let fi = self.view.text_input(cx, ids!(formula_input));
                    fi.set_text(cx, &text);
                    fi.take_key_focus(cx);
                }
                self.view.drop_down(cx, ids!(fx_menu)).set_selected_item(cx, 0);
            }
        }

        // sheet tabs
        for i in 0..MAX_TABS {
            if i < self.wb.sheets.len() && self.tab_button(cx, i).clicked(actions) {
                self.wb.active = i;
                self.widths_applied = false;
                self.status = format!("Sheet {}", self.wb.sheets[i].name);
                self.after_model_change(cx);
            }
        }
        if self.view.button(cx, ids!(add_tab)).clicked(actions) {
            if self.wb.sheets.len() < MAX_TABS {
                self.wb.add_sheet();
                self.widths_applied = false;
                self.status = "New sheet".into();
                self.after_model_change(cx);
            } else {
                self.status = format!("At most {MAX_TABS} sheets");
                self.sync_chrome(cx);
            }
        }
        if self.view.button(cx, ids!(del_tab)).clicked(actions) {
            let i = self.wb.active;
            self.wb.remove_sheet(i);
            self.widths_applied = false;
            self.status = "Sheet removed".into();
            self.after_model_change(cx);
        }
        if let Some((text, _)) = self
            .view
            .text_input(cx, ids!(rename_input))
            .returned(actions)
        {
            let i = self.wb.active;
            self.wb.rename_sheet(i, &text);
            self.view.text_input(cx, ids!(rename_input)).set_text(cx, "");
            self.status = format!("Renamed to {text}");
            self.after_model_change(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(s0: Pos, s1: Pos, cur: Pos) -> (Pos, Pos) {
        MpSheets::fill_dest(&FillDrag { s0, s1, cur })
    }

    #[test]
    fn fill_down_extends_the_rows() {
        assert_eq!(drag((0, 0), (0, 0), (4, 0)), ((0, 0), (4, 0)));
        assert_eq!(drag((0, 1), (2, 1), (6, 1)), ((0, 1), (6, 1)));
    }

    #[test]
    fn fill_right_extends_the_columns() {
        assert_eq!(drag((0, 0), (0, 0), (0, 3)), ((0, 0), (0, 3)));
        assert_eq!(drag((1, 0), (1, 2), (1, 5)), ((1, 0), (1, 5)));
    }

    #[test]
    fn fill_picks_the_dominant_axis() {
        // five rows down, one column across: vertical wins
        assert_eq!(drag((0, 0), (0, 0), (5, 1)), ((0, 0), (5, 0)));
        // one row down, five columns across: horizontal wins
        assert_eq!(drag((0, 0), (0, 0), (1, 5)), ((0, 0), (0, 5)));
    }

    #[test]
    fn dragging_back_onto_the_source_is_a_no_op() {
        assert_eq!(drag((2, 2), (2, 2), (2, 2)), ((2, 2), (2, 2)));
    }

    #[test]
    fn fill_upwards_extends_above_the_source() {
        assert_eq!(drag((5, 0), (5, 0), (2, 0)), ((2, 0), (5, 0)));
    }
}
