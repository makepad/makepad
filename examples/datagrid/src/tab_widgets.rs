//! Cells hosting real widgets: checkboxes, sliders, dropdowns, buttons and
//! Markdown rich text — instantiated only while visible and recycled from a
//! per-template pool as you scroll. Columns can still be reordered/resized.

use makepad_widgets::*;

const ROWS: usize = 500;
const COLS: usize = 6;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.WidgetCellsTabBase = #(WidgetCellsTab::register_widget(vm))
    mod.widgets.WidgetCellsTab = set_type_default() do mod.widgets.WidgetCellsTabBase{
        width: Fill height: Fill
        flow: Down

        grid := DataGrid{
            width: Fill height: Fill
            rows: 500
            cols: 6
            zebra_stripes: true
            allow_col_reorder: true
            default_row_height: 44.0
            row_header_width: 56.0

            CellCheck := View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                check := CheckBox{text: ""}
            }
            CellSlider := View{
                width: Fill height: Fill
                align: Align{y: 0.5}
                padding: Inset{left: 8, right: 8}
                slider := Slider{
                    width: Fill
                    text: ""
                    min: 0.0 max: 100.0
                }
            }
            CellDrop := View{
                width: Fill height: Fill
                align: Align{y: 0.5}
                padding: Inset{left: 6, right: 6}
                drop := DropDown{
                    width: Fill
                    labels: ["Low", "Medium", "High", "Urgent"]
                    draw_text +: {
                        color: #x202020
                        color_hover: #x101010
                        color_focus: #x101010
                        color_down: #x101010
                    }
                }
            }
            CellNotes := View{
                width: Fill height: Fill
                padding: Inset{left: 6, right: 6, top: 4}
                notes := Markdown{
                    width: Fill height: Fit
                    font_color: #x24292f
                    body: ""
                }
            }
            CellBoost := View{
                width: Fill height: Fill
                align: Align{x: 0.5 y: 0.5}
                boost := Button{
                    text: "+10"
                    padding: Inset{left: 10, right: 10, top: 4, bottom: 4}
                    draw_text +: {
                        color: #x202020
                        color_hover: #x101010
                        color_focus: #x101010
                        color_down: #x101010
                    }
                    draw_bg +: {
                        color: #xf2f4f7
                        color_hover: #xe4e9ef
                        color_down: #xd5dce5
                        color_focus: #xe4e9ef
                        border_color: #xc5ccd6
                        border_color_hover: #xa9b3c0
                        border_color_focus: #xa9b3c0
                        border_color_down: #x8c99a9
                    }
                }
            }
        }

        status := View{
            width: Fill height: 26
            flow: Right
            padding: Inset{left: 10, right: 10, top: 5, bottom: 5}
            align: Align{y: 0.5}
            status_label := Label{
                text: "500 rows of live widgets — only visible cells are instantiated, scrolled-out widgets are recycled"
                draw_text +: {color: #xbbbbbb text_style +: {font_size: 8.5}}
            }
        }
    }
}

struct Task {
    done: bool,
    title: String,
    progress: f64,
    priority: usize,
    notes: String,
}

fn demo_tasks() -> Vec<Task> {
    let verbs = [
        "Design", "Implement", "Refactor", "Benchmark", "Document", "Review", "Ship", "Profile",
    ];
    let things = [
        "virtual viewport",
        "cell recycling",
        "column reorder",
        "formula engine",
        "sparkline cells",
        "clip batching",
        "scroll physics",
        "header drag",
    ];
    let notes = [
        "Uses **two draw calls** for all text cells — see `data_grid.rs`",
        "Markdown in a cell: *italic*, **bold** and `code` all work",
        "Try dragging this column's header somewhere else",
        "Resize the row headers' rows too — drag the row edge",
        "The widget pool keeps scrolling this list allocation-free",
        "A `DataGrid` cell can host *any* widget template",
    ];
    (0..ROWS)
        .map(|i| Task {
            done: i % 7 == 3,
            title: format!("{} {} #{}", verbs[i % 8], things[(i / 3) % 8], i + 1),
            progress: ((i * 37) % 101) as f64,
            priority: (i * 13 + i / 9) % 4,
            notes: notes[i % 6].to_string(),
        })
        .collect()
}

#[derive(Script, ScriptHook, Widget)]
pub struct WidgetCellsTab {
    #[deref]
    view: View,
    #[rust(demo_tasks())]
    tasks: Vec<Task>,
    #[rust]
    initialized: bool,
}

const PRIORITY_TINT: [Vec4f; 4] = [
    Vec4f { x: 0.93, y: 0.96, z: 0.93, w: 1.0 },
    Vec4f { x: 0.90, y: 0.93, z: 0.98, w: 1.0 },
    Vec4f { x: 1.00, y: 0.95, z: 0.85, w: 1.0 },
    Vec4f { x: 0.99, y: 0.88, z: 0.87, w: 1.0 },
];

impl Widget for WidgetCellsTab {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                if !self.initialized {
                    self.initialized = true;
                    grid.set_col_labels(
                        ["Done", "Task", "Progress", "Priority", "Notes", "Boost"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    );
                    grid.set_col_width(0, 60.0);
                    grid.set_col_width(1, 250.0);
                    grid.set_col_width(2, 190.0);
                    grid.set_col_width(3, 130.0);
                    grid.set_col_width(4, 430.0);
                    grid.set_col_width(5, 90.0);
                }
                grid.set_grid_size(ROWS, COLS);
                while let Some(cell) = grid.next_cell(cx) {
                    let task = &self.tasks[cell.row];
                    match cell.col {
                        0 => {
                            if let Some(item) = grid.item(cx, cell.row, cell.col, id!(CellCheck)) {
                                item.check_box(cx, ids!(check))
                                    .set_active(cx, task.done, Animate::No);
                                grid.draw_item(cx, &cell, &item, None);
                            }
                        }
                        1 => {
                            let (text, color) = if task.done {
                                (
                                    format!("✓ {}", task.title),
                                    Some(vec4(0.55, 0.55, 0.55, 1.0)),
                                )
                            } else {
                                (task.title.clone(), None)
                            };
                            grid.cell_text_styled(
                                cx,
                                &cell,
                                &text,
                                CellStyle {
                                    color,
                                    ..CellStyle::default()
                                },
                            );
                        }
                        2 => {
                            if let Some(item) = grid.item(cx, cell.row, cell.col, id!(CellSlider)) {
                                item.slider(cx, ids!(slider)).set_value(cx, task.progress);
                                grid.draw_item(cx, &cell, &item, None);
                            }
                        }
                        3 => {
                            if let Some(item) = grid.item(cx, cell.row, cell.col, id!(CellDrop)) {
                                item.drop_down(cx, ids!(drop))
                                    .set_selected_item(cx, task.priority);
                                grid.draw_item(
                                    cx,
                                    &cell,
                                    &item,
                                    Some(PRIORITY_TINT[task.priority]),
                                );
                            }
                        }
                        4 => {
                            if let Some(item) = grid.item(cx, cell.row, cell.col, id!(CellNotes)) {
                                item.markdown(cx, ids!(notes)).set_text(cx, &task.notes);
                                grid.draw_item(cx, &cell, &item, None);
                            }
                        }
                        _ => {
                            if let Some(item) = grid.item(cx, cell.row, cell.col, id!(CellBoost)) {
                                grid.draw_item(cx, &cell, &item, None);
                            }
                        }
                    }
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
        let mut dirty = false;
        for (row, col, widget) in grid.cell_widgets_with_actions(actions) {
            if row >= self.tasks.len() {
                continue;
            }
            match col {
                0 => {
                    if let Some(active) = widget.check_box(cx, ids!(check)).changed(actions) {
                        self.tasks[row].done = active;
                        dirty = true;
                    }
                }
                2 => {
                    if let Some(v) = widget.slider(cx, ids!(slider)).slided(actions) {
                        self.tasks[row].progress = v;
                    }
                }
                3 => {
                    if let Some(i) = widget.drop_down(cx, ids!(drop)).changed(actions) {
                        self.tasks[row].priority = i;
                        dirty = true;
                    }
                }
                5 => {
                    if widget.button(cx, ids!(boost)).clicked(actions) {
                        self.tasks[row].progress = (self.tasks[row].progress + 10.0).min(100.0);
                        dirty = true;
                    }
                }
                _ => (),
            }
        }
        if dirty {
            grid.redraw(cx);
        }
    }
}
