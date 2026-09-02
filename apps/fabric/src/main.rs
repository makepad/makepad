mod body_view;
mod camera;
mod install;
mod pattern_view;
mod pipeline;

use body_view::FabricBodyView;
use camera::{install_camera, pick_camera, CameraMailbox};
use install::{body_model_row, body_model_status, BODY_MODEL_ID, BODY_MODEL_ROLE};
use makepad_ai_hub::local::{InstallState, LocalModels};
use makepad_ai_hub_ui::{ModelInstallPanel, ModelRowInstallState};
use makepad_fabric_draft::{
    designs, nest, to_pdf, to_svg, Design, OptionSpec, Options, PageSize, Pattern,
};
use makepad_fabric_measure::{Measurements, MEASUREMENT_KEYS};
use makepad_widgets::*;
use pattern_view::FabricPatternView;
use pipeline::{Pipeline, PipelineMessage};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let SectionTitle = Label {
        width: Fill
        height: 20
        draw_text +: {
            color: #x8da0b5
            text_style: theme.font_bold{font_size: 9.0}
        }
    }

    let Hint = Label {
        width: Fill
        height: Fit
        draw_text +: {
            color: #x667789
            text_style: theme.font_regular{font_size: 9.0}
        }
    }

    let Status = Label {
        width: Fill
        height: Fit
        draw_text +: {
            color: #xa9b6c4
            text_style: theme.font_regular{font_size: 9.0}
        }
    }

    let ToolButton = Button {
        height: 30
        draw_bg +: {
            color: #x27313c
            color_hover: #x334252
            color_down: #x1d252e
            border_color: #x52677c
            border_size: 1.0
            border_radius: 3.0
        }
        draw_text +: {
            color: #xe7edf3
            color_hover: #xffffffff
            text_style: theme.font_bold{font_size: 9.5}
        }
    }

    let CameraImage = Image {
        draw_bg +: {
            bbox: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            mirror: uniform(1.0)
            pixel: fn() {
                let scale = self.fit_scale * self.image_scale
                let pan = self.fit_pan * self.image_scale + self.image_pan
                let sample_scale = vec2(scale.x * mix(1.0, -1.0, self.mirror), scale.y)
                let sample_pan = vec2(mix(pan.x, 1.0 - pan.x, self.mirror), pan.y)
                let color = self.get_color_scale_pan(sample_scale, sample_pan)
                if self.bbox.x >= 0.0 {
                    let line = vec2(1.5, 1.5) / self.rect_size
                    let on_x = (abs(self.pos.x - self.bbox.x) < line.x || abs(self.pos.x - self.bbox.z) < line.x)
                        && self.pos.y >= self.bbox.y && self.pos.y <= self.bbox.w
                    let on_y = (abs(self.pos.y - self.bbox.y) < line.y || abs(self.pos.y - self.bbox.w) < line.y)
                        && self.pos.x >= self.bbox.x && self.pos.x <= self.bbox.z
                    if on_x || on_y {
                        return Pal.premul(#x54d59a)
                    }
                }
                return Pal.premul(color)
            }
        }
    }

    let TabButton = Button {
        height: 24
        padding: Inset{left: 12 right: 12 top: 4 bottom: 4}
        draw_bg +: {
            color: #x1b232c
            color_hover: #x2a3542
            color_down: #x151b22
            border_color: #x1b232c
            border_size: 1.0
            border_radius: 3.0
        }
        draw_text +: {
            color: #x9aa8b7
            color_hover: #xffffffff
            text_style: theme.font_bold{font_size: 9.0}
        }
    }
    mod.widgets.FabricMeasurementGridBase = #(FabricMeasurementGrid::register_widget(vm))
    mod.widgets.FabricMeasurementGrid = set_type_default() do mod.widgets.FabricMeasurementGridBase {
        width: Fill
        height: Fill
        grid := DataGrid {
            width: Fill
            height: Fill
            rows: 25
            cols: 2
            default_col_width: 110.0
            default_row_height: 24.0
            row_header_width: 30.0
            color_bg: #x161d25
            color_cell: #x1a222c
            color_cell_alt: #x1d2631
            color_text: #xdbe4ee
            color_header: #x222c38
            color_header_active: #x2f3d4d
            color_header_text: #x9aa8b7
            color_selection: #x4fa3ff26
            color_selection_border: #x4fa3ff
            draw_text +: {color: #xdbe4ee}
            draw_text_bold +: {color: #xdbe4ee}
            Editor := TextInput {
                width: Fill
                height: Fill
                margin: 0
                padding: Inset{left: 4 right: 4 top: 4 bottom: 3}
                draw_bg +: {
                    border_radius: 0.0
                    border_size: 2.0
                    border_color: #x4fa3ff
                    border_color_hover: #x4fa3ff
                    border_color_focus: #x4fa3ff
                    color: #x0f151c
                    color_hover: #x0f151c
                    color_focus: #x0f151c
                }
                draw_text +: {
                    text_style: theme.font_code{font_size: 9.0}
                    color: #xffffffff
                }
            }
        }
    }
    mod.widgets.FabricOptionsListBase = #(FabricOptionsList::register_widget(vm))
    mod.widgets.FabricOptionsList = set_type_default() do mod.widgets.FabricOptionsListBase {
        width: Fill
        height: Fill
        empty := Hint{text: "this design has no options"}
        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: true
            Row := View {
                width: Fill
                height: 46
                flow: Down
                spacing: 2
                option_caption := View {
                    width: Fill
                    height: Fit
                    flow: Right
                    option_name := Label {
                        width: Fill
                        height: Fit
                        draw_text +: {
                            color: #xa9b6c4
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    option_unit := Label {
                        width: Fit
                        height: Fit
                        draw_text +: {
                            color: #x667789
                            text_style: theme.font_regular{font_size: 8.0}
                        }
                    }
                }
                option_slider := Slider {
                    width: Fill
                    height: 24
                    min: 0.0
                    max: 100.0
                    default: 0.0
                    precision: 1
                    text: ""
                }
            }
        }
    }

    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.title: "Fabric"
                window.inner_size: vec2(1400, 900)
                pass +: {clear_color: #x0b1016}
                body +: {
                    width: Fill
                    height: Fill
                    flow: Right
                    spacing: 1
                    padding: 0
                    // Events go to children in draw order here, first come first
                    // served: the licence modal lives inside the LEFT column and
                    // must get a click before the body view under it does.
                    event_order: EventOrder.Down

                    left_panel := SolidView {
                        width: 320
                        height: Fill
                        flow: Down
                        spacing: 8
                        padding: 14
                        draw_bg +: {color: #x161d25}

                        Label {
                            width: Fill
                            height: 28
                            text: "FABRIC"
                            draw_text +: {
                                color: #xf0f4f8
                                text_style: theme.font_bold{font_size: 15.0}
                            }
                        }
                        SectionTitle{text: "BODY MODEL"}
                        model_install := mod.widgets.ModelInstallPanel {
                            width: Fill
                            height: 142
                        }
                        model_status := Status{text: "checking model…"}

                        SectionTitle{text: "PHOTO"}
                        drop_zone := RoundedView {
                            width: Fill
                            height: 225
                            flow: Down
                            spacing: 7
                            padding: 10
                            align: Align{x: 0.5 y: 0.5}
                            show_bg: true
                            draw_bg +: {
                                color: #x111820
                                border_color: #x405164
                                border_size: 1.0
                                border_radius: 4.0
                                pixel: fn() {
                                    let p = self.pos * self.rect_size
                                    let edge_x = min(p.x, self.rect_size.x - p.x)
                                    let edge_y = min(p.y, self.rect_size.y - p.y)
                                    let edge = min(edge_x, edge_y)
                                    let along = if edge_x < edge_y p.y else p.x
                                    if edge < self.border_size && modf(along, 10.0) < 6.0 {
                                        return Pal.premul(self.border_color)
                                    }
                                    return Pal.premul(self.color)
                                }
                            }
                            photo_image := Image {
                                width: Fill
                                height: 163
                                fit: ImageFit.Smallest
                                visible: false
                            }
                            live_image := CameraImage {
                                width: Fill
                                height: 163
                                fit: ImageFit.Smallest
                                visible: false
                            }
                            drop_title := Label {
                                width: Fill
                                height: Fit
                                text: "drop a photo"
                                draw_text +: {
                                    color: #xd1dae4
                                    text_style: theme.font_bold{font_size: 11.0}
                                }
                            }
                            drop_help := Hint {
                                text: "front view · tight clothes · whole body in frame"
                            }
                            photo_name := Hint{text: "JPG or PNG"}
                        }

                        View {
                            width: Fill
                            height: 29
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0 y: 0.5}
                            Label {
                                width: 58
                                height: Fit
                                text: "HEIGHT"
                                draw_text +: {
                                    color: #x8da0b5
                                    text_style: theme.font_bold{font_size: 9.0}
                                }
                            }
                            height_input := TextInput {
                                width: Fill
                                height: 25
                                empty_text: "optional"
                            }
                            Label {
                                width: 22
                                height: Fit
                                text: "cm"
                                draw_text +: {color: #x667789}
                            }
                        }
                        View {
                            width: Fill
                            height: 30
                            flow: Right
                            spacing: 8
                            measure_button := ToolButton {
                                width: Fill
                                text: "MEASURE"
                            }
                            live_button := ToolButton {
                                width: 78
                                text: "LIVE"
                            }
                            mirror_toggle := Toggle {
                                width: 88
                                height: 30
                                text: "MIRROR"
                                active: true
                            }
                        }
                        progress_status := Status{text: "drop a photo to begin"}
                    }

                    centre_panel := SolidView {
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: #x11161d}
                        split := Splitter {
                            width: Fill
                            height: Fill
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.Weighted(0.42)
                            size: 6.0
                            draw_bg +: {
                                color_bg: #x11161d
                                color: #x1f2731
                                color_hover: #x2f3d4d
                                color_drag: #x4fa3ff
                            }
                            a: View {
                                width: Fill
                                height: Fill
                                flow: Down
                                View {
                                    width: Fill
                                    height: 38
                                    flow: Right
                                    spacing: 10
                                    padding: Inset{left: 14 right: 14}
                                    align: Align{x: 0.0 y: 0.5}
                                    SectionTitle{width: Fit text: "BODY"}
                                    Hint{text: "drag orbit · shift-drag pan · wheel zoom · double-click reset"}
                                }
                                body_preview := mod.widgets.FabricBodyView {}
                            }
                            b: View {
                                width: Fill
                                height: Fill
                                flow: Right
                                View {
                                    width: Fill
                                    height: Fill
                                    flow: Down
                                    View {
                                        width: Fill
                                        height: 38
                                        flow: Right
                                        spacing: 10
                                        padding: Inset{left: 14 right: 14}
                                        align: Align{x: 0.0 y: 0.5}
                                        SectionTitle{width: Fit text: "PATTERN"}
                                        settling_tag := RoundedView {
                                            width: Fit
                                            height: 18
                                            padding: Inset{left: 7 right: 7 top: 2 bottom: 2}
                                            visible: false
                                            show_bg: true
                                            draw_bg +: {color: #x493d24 border_radius: 9.0}
                                            Label {
                                                width: Fit
                                                height: Fit
                                                text: "settling…"
                                                draw_text +: {
                                                    color: #xe8c36d
                                                    text_style: theme.font_regular{font_size: 8.0}
                                                }
                                            }
                                        }
                                        Hint{text: "drag pan · wheel zoom · cut line solid, seam line dim"}
                                    }
                                    pattern_preview := mod.widgets.FabricPatternView {}
                                }
                                design_column := SolidView {
                                    width: 232
                                    height: Fill
                                    flow: Down
                                    spacing: 8
                                    padding: 12
                                    draw_bg +: {color: #x141a22}
                                    SectionTitle{text: "DESIGN"}
                                    design_select := DropDown {
                                        width: Fill
                                        height: 28
                                        labels: ["No designs available"]
                                    }
                                    design_options := mod.widgets.FabricOptionsList {
                                        width: Fill
                                        height: Fill
                                    }
                                    export_svg := ToolButton{width: Fill text: "EXPORT SVG"}
                                    export_pdf := ToolButton{width: Fill text: "EXPORT PDF (A4)"}
                                    app_status := Status{text: "sample measurements ready"}
                                }
                            }
                        }
                    }
                    right_panel := SolidView {
                        width: 380
                        height: Fill
                        flow: Down
                        spacing: 8
                        padding: 14
                        draw_bg +: {color: #x161d25}
                        View {
                            width: Fill
                            height: 20
                            flow: Right
                            spacing: 8
                            align: Align{x: 0.0 y: 0.5}
                            SectionTitle{width: Fill text: "MEASUREMENTS"}
                            sample_tag := RoundedView {
                                width: Fit
                                height: 18
                                padding: Inset{left: 7 right: 7 top: 2 bottom: 2}
                                show_bg: true
                                draw_bg +: {color: #x293440 border_radius: 9.0}
                                sample_tag_label := Label {
                                    width: Fit
                                    height: Fit
                                    text: "sample body"
                                    draw_text +: {
                                        color: #x8998a8
                                        text_style: theme.font_regular{font_size: 8.0}
                                    }
                                }
                            }
                            copy_all := TabButton{
                                height: 20
                                padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
                                text: "COPY ALL"
                            }
                        }
                        measurement_grid := mod.widgets.FabricMeasurementGrid {
                            width: Fill
                            height: Fill
                        }
                        Hint{text: "click and drag to select · ⌘C copies as tab-separated text · double-click a value to correct it"}
                    }
                }
            }
        }
    }
}
#[derive(Clone, Debug, Default)]
enum MeasurementListAction {
    Changed { key: &'static str, value: f32 },
    #[default]
    None,
}

/// The measurement table as a spreadsheet grid: keys down, centimetres in
/// the second column, so a selection copies as tab-separated text straight
/// into a spreadsheet. Double-click (or type on) a value to correct it.
#[derive(Script, ScriptHook, Widget)]
struct FabricMeasurementGrid {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    values: Measurements,
    #[rust(true)]
    sample: bool,
    #[rust]
    initialized: bool,
    /// The row whose value is being edited in place.
    #[rust]
    editing: Option<usize>,
    /// The text the editor starts with; taken on the draw that seeds it.
    #[rust]
    edit_seed: Option<String>,
}

impl FabricMeasurementGrid {
    fn set_measurements(&mut self, cx: &mut Cx, values: Measurements, sample: bool) {
        self.values = values;
        self.sample = sample;
        self.refresh_copy_provider(cx);
        self.view.redraw(cx);
    }

    /// Tab-separated rows for a selection; every row when nothing is selected.
    fn tsv(&self, selection: Option<GridSelection>) -> String {
        let entries = self.values.entries();
        let last = entries.len() - 1;
        let (r0, r1, c0, c1) = match selection {
            Some(sel) => {
                let (r0, r1) = sel.row_range();
                let (c0, c1) = sel.col_range();
                (r0.min(last), r1.min(last), c0.min(1), c1.min(1))
            }
            None => (0, last, 0, 1),
        };
        let mut out = String::new();
        for (key, value) in &entries[r0..=r1] {
            let mut cols: Vec<String> = Vec::new();
            if c0 == 0 {
                cols.push(humanise_key(key));
            }
            if c1 == 1 {
                cols.push(format!("{value:.1}"));
            }
            out.push_str(&cols.join("\t"));
            out.push('\n');
        }
        out
    }

    fn refresh_copy_provider(&self, cx: &mut Cx) {
        let grid = self.view.data_grid(cx, ids!(grid));
        let text = self.tsv(grid.selection());
        grid.set_copy_provider(Box::new(move |_sel| text.clone()));
    }

    fn start_edit(&mut self, cx: &mut Cx, row: usize, replace: Option<String>) {
        if row >= MEASUREMENT_KEYS.len() {
            return;
        }
        self.editing = Some(row);
        self.edit_seed = Some(
            replace.unwrap_or_else(|| format!("{:.1}", self.values.entries()[row].1)),
        );
        let grid = self.view.data_grid(cx, ids!(grid));
        grid.set_selection(cx, Some(GridSelection::single(row, 1)));
        grid.redraw(cx);
    }

    fn commit_edit(&mut self, cx: &mut Cx, row: usize, text: &str) {
        self.editing = None;
        self.edit_seed = None;
        if let Some(&key) = MEASUREMENT_KEYS.get(row) {
            if let Ok(value) = text.trim().replace(',', ".").parse::<f32>() {
                if value.is_finite() && value > 0.0 && self.values.set(key, value) {
                    cx.widget_action(
                        self.widget_uid(),
                        MeasurementListAction::Changed { key, value },
                    );
                }
            }
        }
        self.refresh_copy_provider(cx);
        self.view.data_grid(cx, ids!(grid)).redraw(cx);
    }

    fn commit_current(&mut self, cx: &mut Cx) {
        let Some(row) = self.editing else { return };
        let text = self
            .view
            .data_grid(cx, ids!(grid))
            .get_item(row, 1)
            .map(|(_, widget)| widget.as_text_input().text());
        match text {
            Some(text) => self.commit_edit(cx, row, &text),
            None => self.cancel_edit(cx),
        }
    }

    fn cancel_edit(&mut self, cx: &mut Cx) {
        self.editing = None;
        self.edit_seed = None;
        self.view.data_grid(cx, ids!(grid)).redraw(cx);
    }
}

impl Widget for FabricMeasurementGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            match action {
                DataGridAction::EditCell { row, col, replace } if col == 1 => {
                    self.start_edit(cx, row, replace);
                }
                DataGridAction::CellDoubleClicked { row, col } if col == 1 => {
                    self.start_edit(cx, row, None);
                }
                DataGridAction::CellClicked { .. } => {
                    if self.editing.is_some() {
                        self.commit_current(cx);
                    }
                }
                DataGridAction::SelectionChanged { .. } => self.refresh_copy_provider(cx),
                _ => {}
            }
        }
        for (row, _col, widget) in grid.cell_widgets_with_actions(actions) {
            let input = widget.as_text_input();
            if let Some((text, _modifiers)) = input.returned(actions) {
                self.commit_edit(cx, row, &text);
            } else if input.escaped(actions) {
                self.cancel_edit(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else {
                continue;
            };
            if !self.initialized {
                self.initialized = true;
                grid.set_col_labels(vec!["measurement".to_string(), "cm".to_string()]);
                grid.set_col_width(0, 190.0);
                grid.set_col_width(1, 84.0);
            }
            grid.set_grid_size(MEASUREMENT_KEYS.len(), 2);
            let entries = self.values.entries();
            let value_color = if self.sample {
                vec4(0.55, 0.61, 0.67, 1.0)
            } else {
                vec4(0.92, 0.95, 0.98, 1.0)
            };
            let key_color = vec4(0.66, 0.72, 0.78, 1.0);
            while let Some(cell) = grid.next_cell(cx) {
                let Some((key, value)) = entries.get(cell.row).copied() else {
                    continue;
                };
                if cell.col == 1 && self.editing == Some(cell.row) {
                    if let Some(item) = grid.item(cx, cell.row, cell.col, id!(Editor)) {
                        let seed = self.edit_seed.take();
                        if let Some(seed) = &seed {
                            item.as_text_input().set_text(cx, seed);
                        }
                        grid.draw_item(cx, &cell, &item, None);
                        // Focus only once the editor has a drawn area.
                        if seed.is_some() {
                            item.as_text_input().take_key_focus(cx);
                        }
                    }
                    continue;
                }
                let (text, align, color) = if cell.col == 0 {
                    (humanise_key(key), 0.0, key_color)
                } else {
                    (format!("{value:.1}"), 1.0, value_color)
                };
                grid.cell_text_styled(
                    cx,
                    &cell,
                    &text,
                    CellStyle {
                        align,
                        color: Some(color),
                        ..CellStyle::default()
                    },
                );
            }
        }
        DrawStep::done()
    }
}

#[derive(Clone, Debug, Default)]
enum OptionsListAction {
    Changed { key: String, value: f64 },
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
struct FabricOptionsList {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    specs: Vec<OptionSpec>,
    #[rust]
    values: Vec<f64>,
}

impl FabricOptionsList {
    fn set_specs(&mut self, cx: &mut Cx, specs: Vec<OptionSpec>, options: &Options) {
        self.values = specs.iter().map(|spec| options.get(spec)).collect();
        self.specs = specs;
        self.view.label(cx, ids!(empty)).set_visible(cx, self.specs.is_empty());
        self.view
            .portal_list(cx, ids!(list))
            .set_visible(cx, !self.specs.is_empty());
        self.view.redraw(cx);
    }
}

impl Widget for FabricOptionsList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(spec) = self.specs.get(index) else {
                continue;
            };
            if let Some(value) = item.slider(cx, ids!(option_slider)).slided(actions) {
                if let Some(slot) = self.values.get_mut(index) {
                    *slot = value;
                }
                cx.widget_action(
                    self.widget_uid(),
                    OptionsListAction::Changed {
                        key: spec.key.to_string(),
                        value,
                    },
                );
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else {
                continue;
            };
            list.set_item_range(cx, 0, self.specs.len());
            while let Some(index) = list.next_visible_item(cx) {
                let row = list.item(cx, index, id!(Row));
                if let (Some(spec), Some(value)) = (self.specs.get(index), self.values.get(index)) {
                    row.label(cx, ids!(option_name)).set_text(cx, spec.label);
                    row.label(cx, ids!(option_unit)).set_text(cx, spec.unit);
                    let mut slider = row.slider(cx, ids!(option_slider));
                    let min = spec.min;
                    let max = spec.max;
                    let default = spec.default;
                    let step = ((max - min).abs() / 100.0).max(0.01);
                    script_apply_eval!(cx, slider, {
                        min: #(min)
                        max: #(max)
                        default: #(default)
                        step: #(step)
                    });
                    slider.set_value(cx, *value);
                }
                row.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }
}

const SETTLE_AFTER: Duration = Duration::from_millis(1_500);
const SETTLE_TOLERANCE_CM: f32 = 0.5;

#[derive(Default)]
struct MeasurementSettler {
    history: VecDeque<(Duration, Measurements)>,
    settled: bool,
}

impl MeasurementSettler {
    /// Returns true only on the transition into a settled state, so a stable
    /// live stream drafts once instead of rebuilding the pattern every frame.
    fn push(&mut self, now: Duration, measurements: Measurements) -> bool {
        self.history.push_back((now, measurements));
        let Some(cutoff) = now.checked_sub(SETTLE_AFTER) else {
            self.settled = false;
            return false;
        };
        while self.history.len() > 1
            && self
                .history
                .get(1)
                .is_some_and(|(time, _)| *time <= cutoff)
        {
            self.history.pop_front();
        }
        let stable = self
            .history
            .front()
            .filter(|(time, _)| *time <= cutoff)
            .is_some_and(|(_, previous)| {
                measurements.entries().iter().all(|(key, value)| {
                    previous
                        .get(key)
                        .is_some_and(|old| (value - old).abs() <= SETTLE_TOLERANCE_CM)
                })
            });
        let became_settled = stable && !self.settled;
        self.settled = stable;
        became_settled
    }

    fn reset(&mut self) {
        self.history.clear();
        self.settled = false;
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    models: Option<LocalModels>,
    #[rust]
    pipeline: Option<Pipeline>,
    #[rust]
    refresh_timer: Timer,
    #[rust]
    photo: Option<PathBuf>,
    #[rust]
    pipeline_busy: bool,
    #[rust]
    pipeline_started: Option<Instant>,
    #[rust]
    camera: CameraMailbox,
    #[rust]
    camera_installed: bool,
    #[rust]
    live: bool,
    #[rust(true)]
    mirrored: bool,
    #[rust]
    live_started: Option<Instant>,
    #[rust]
    preview_texture: Option<Texture>,
    #[rust]
    preview_serial: u64,
    #[rust]
    live_bbox: Option<[f32; 4]>,
    #[rust]
    live_frame_size: Option<(u32, u32)>,
    #[rust]
    settler: MeasurementSettler,
    #[rust]
    measurements: Measurements,
    #[rust]
    has_measured_body: bool,
    #[rust]
    designs: Vec<Box<dyn Design>>,
    #[rust]
    design_index: usize,
    #[rust]
    options: Options,
    #[rust]
    pattern: Option<Pattern>,
    #[rust]
    model_ready: bool,
    #[rust]
    drag_over: bool,
}

impl App {
    fn startup(&mut self, cx: &mut Cx) {
        self.pipeline = Some(Pipeline::new());
        self.refresh_timer = cx.start_interval(0.1);
        self.measurements = Measurements::sample();
        self.sync_measurements(cx);
        self.designs = designs();
        let labels = if self.designs.is_empty() {
            vec!["No designs available".to_string()]
        } else {
            self.designs
                .iter()
                .map(|design| design.name().to_string())
                .collect()
        };
        self.ui
            .drop_down(cx, ids!(design_select))
            .set_labels(cx, labels);
        self.configure_options(cx);
        self.redraft(cx);

        match LocalModels::open() {
            Ok(models) => {
                let row = body_model_row(&models);
                let panel = self.ui.widget(cx, ids!(model_install));
                if let Some(mut panel) = panel.borrow_mut::<ModelInstallPanel>() {
                    panel.set_rows(cx, vec![row]);
                }
                self.models = Some(models);
                self.refresh_model_ui(cx);
            }
            Err(error) => {
                self.set_progress(cx, format!("could not open local models: {error}"));
                self.ui
                    .label(cx, ids!(model_status))
                    .set_text(cx, "model registry unavailable");
                self.ui
                    .button(cx, ids!(measure_button))
                    .set_disabled(cx, true);
                self.ui
                    .button(cx, ids!(live_button))
                    .set_disabled(cx, true);
            }
        }
    }

    fn sync_measurements(&self, cx: &mut Cx) {
        let widget = self.ui.widget(cx, ids!(measurement_grid));
        if let Some(mut grid) = widget.borrow_mut::<FabricMeasurementGrid>() {
            grid.set_measurements(cx, self.measurements, !self.has_measured_body);
        }
        self.ui
            .view(cx, ids!(sample_tag))
            .set_visible(cx, self.live || !self.has_measured_body);
        self.ui
            .label(cx, ids!(sample_tag_label))
            .set_text(cx, if self.live { "live body" } else { "sample body" });
    }

    fn configure_options(&mut self, cx: &mut Cx) {
        self.options = Options::default();
        let specs = self
            .designs
            .get(self.design_index)
            .map(|design| design.options())
            .unwrap_or_default();
        for spec in &specs {
            self.options.0.insert(spec.key.to_string(), spec.default);
        }
        let widget = self.ui.widget(cx, ids!(design_options));
        if let Some(mut list) = widget.borrow_mut::<FabricOptionsList>() {
            list.set_specs(cx, specs, &self.options);
        };
    }

    fn redraft(&mut self, cx: &mut Cx) {
        let Some(design) = self.designs.get(self.design_index) else {
            self.pattern = None;
            self.set_pattern_error(cx, "no designs are available from the draft library");
            self.set_app_status(cx, "sample body ready · waiting for a draft design");
            return;
        };
        match design.draft(&self.measurements, &self.options) {
            Ok(pattern) => {
                self.pattern = Some(pattern.clone());
                let widget = self.ui.widget(cx, ids!(pattern_preview));
                if let Some(mut view) = widget.borrow_mut::<FabricPatternView>() {
                    view.set_pattern(cx, pattern);
                }
                self.set_app_status(cx, format!("{} pattern ready", design.name()));
            }
            Err(error) => {
                self.pattern = None;
                self.set_pattern_error(cx, error.to_string());
                self.set_app_status(cx, error.to_string());
            }
        }
    }

    fn set_pattern_error(&self, cx: &mut Cx, error: impl Into<String>) {
        let widget = self.ui.widget(cx, ids!(pattern_preview));
        if let Some(mut view) = widget.borrow_mut::<FabricPatternView>() {
            view.set_error(cx, error);
        };
    }

    fn set_progress(&self, cx: &mut Cx, status: impl AsRef<str>) {
        self.ui
            .label(cx, ids!(progress_status))
            .set_text(cx, status.as_ref());
    }

    fn set_app_status(&self, cx: &mut Cx, status: impl AsRef<str>) {
        self.ui
            .label(cx, ids!(app_status))
            .set_text(cx, status.as_ref());
    }

    fn panel_is_downloading(&self, cx: &mut Cx) -> bool {
        let panel = self.ui.widget(cx, ids!(model_install));
        panel
            .borrow::<ModelInstallPanel>()
            .map(|panel| {
                panel.rows().iter().any(|row| {
                    row.model_id == BODY_MODEL_ID
                        && matches!(row.state, ModelRowInstallState::Downloading)
                })
            })
            .unwrap_or(false)
    }

    fn refresh_model_ui(&mut self, cx: &mut Cx) {
        let downloading = self.panel_is_downloading(cx);
        let ready = self.models.as_ref().is_some_and(|models| {
            models.license_acknowledged(BODY_MODEL_ID)
                && matches!(models.install_state(BODY_MODEL_ID), InstallState::Installed)
                && models
                    .installed_path(BODY_MODEL_ID, BODY_MODEL_ROLE)
                    .is_some()
        });
        if let Some(models) = self.models.as_ref() {
            self.ui
                .label(cx, ids!(model_status))
                .set_text(cx, &body_model_status(models, downloading));
        }
        self.ui
            .button(cx, ids!(measure_button))
            .set_disabled(cx, !ready || self.pipeline_busy || self.photo.is_none());
        self.ui
            .button(cx, ids!(live_button))
            .set_disabled(cx, !self.live && (!ready || self.pipeline_busy));
        let became_ready = ready && !self.model_ready;
        self.model_ready = ready;
        if became_ready && self.photo.is_some() && !self.pipeline_busy {
            self.start_measurement(cx);
        }
    }

    fn start_measurement(&mut self, cx: &mut Cx) {
        if self.pipeline_busy {
            return;
        }
        let Some(photo) = self.photo.clone() else {
            self.set_progress(cx, "drop a photo first");
            return;
        };
        let (weights, height_cm) = match self.pipeline_inputs(cx) {
            Ok(inputs) => inputs,
            Err(error) => {
                self.set_progress(cx, error);
                return;
            }
        };
        let Some(pipeline) = self.pipeline.as_ref() else {
            self.set_progress(cx, "the body model worker is unavailable");
            return;
        };
        match pipeline.run(photo, weights, height_cm) {
            Ok(()) => {
                self.pipeline_busy = true;
                self.pipeline_started = Some(Instant::now());
                self.set_progress(cx, "queued…");
                self.refresh_model_ui(cx);
            }
            Err(error) => self.set_progress(cx, error),
        }
    }

    fn pipeline_inputs(&self, cx: &mut Cx) -> Result<(PathBuf, Option<f32>), String> {
        let models = self
            .models
            .as_ref()
            .ok_or_else(|| "install the body model first".to_string())?;
        if !models.license_acknowledged(BODY_MODEL_ID) {
            return Err("accept the body model licence first".to_string());
        }
        let weights = models
            .installed_path(BODY_MODEL_ID, BODY_MODEL_ROLE)
            .ok_or_else(|| "install the body model first".to_string())?;
        let height_text = self.ui.text_input(cx, ids!(height_input)).text();
        let height_cm = if height_text.trim().is_empty() {
            None
        } else {
            match height_text.trim().parse::<f32>() {
                Ok(value) if value.is_finite() && (80.0..=260.0).contains(&value) => Some(value),
                _ => return Err("height must be 80–260 cm, or left empty".to_string()),
            }
        };
        Ok((weights, height_cm))
    }

    fn start_live(&mut self, cx: &mut Cx) {
        if self.live || self.pipeline_busy {
            return;
        }
        let (weights, height_cm) = match self.pipeline_inputs(cx) {
            Ok(inputs) => inputs,
            Err(error) => {
                self.set_progress(cx, error);
                return;
            }
        };
        use makepad_widgets::makepad_platform::permission::Permission;
        cx.request_permission(Permission::Camera);
        if !self.camera_installed {
            install_camera(cx, self.camera.clone());
            self.camera_installed = true;
        }
        let Some(pipeline) = self.pipeline.as_ref() else {
            self.set_progress(cx, "the body model worker is unavailable");
            return;
        };
        if let Err(error) = pipeline.start_live(weights, height_cm, self.camera.clone()) {
            self.set_progress(cx, error);
            return;
        }

        self.live = true;
        self.live_started = Some(Instant::now());
        self.live_bbox = None;
        self.live_frame_size = None;
        self.settler.reset();
        self.set_live_ui(cx);
        self.set_progress(cx, "live · looking for a camera…");
        self.refresh_model_ui(cx);
    }

    fn stop_live(&mut self, cx: &mut Cx) {
        if !self.live {
            return;
        }
        cx.use_video_input(&[]);
        if let Some(pipeline) = self.pipeline.as_ref() {
            pipeline.stop_live();
        }
        self.live = false;
        self.live_started = None;
        self.live_bbox = None;
        self.live_frame_size = None;
        self.settler.reset();
        self.set_live_ui(cx);
        self.set_progress(
            cx,
            if self.pipeline_busy {
                "live stopped · queued photo next"
            } else {
                "live stopped"
            },
        );
        self.refresh_model_ui(cx);
    }

    fn set_live_ui(&self, cx: &mut Cx) {
        self.ui
            .image(cx, ids!(live_image))
            .set_visible(cx, self.live);
        self.ui
            .image(cx, ids!(photo_image))
            .set_visible(cx, !self.live && self.photo.is_some());
        self.ui
            .label(cx, ids!(drop_title))
            .set_text(cx, if self.live { "live camera" } else if self.photo.is_some() { "photo loaded" } else { "drop a photo" });
        let photo_name = if self.live {
            "camera · body model runs continuously".to_string()
        } else {
            self.photo
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "JPG or PNG".to_string())
        };
        self.ui
            .label(cx, ids!(photo_name))
            .set_text(cx, &photo_name);
        self.ui
            .view(cx, ids!(settling_tag))
            .set_visible(cx, self.live && !self.settler.settled);

        let color = if self.live {
            Vec4f {
                x: 0.08,
                y: 0.38,
                z: 0.24,
                w: 1.0,
            }
        } else {
            Vec4f {
                x: 0.153,
                y: 0.192,
                z: 0.235,
                w: 1.0,
            }
        };
        let border = if self.live {
            Vec4f {
                x: 0.33,
                y: 0.84,
                z: 0.60,
                w: 1.0,
            }
        } else {
            Vec4f {
                x: 0.322,
                y: 0.404,
                z: 0.486,
                w: 1.0,
            }
        };
        let mut button = self.ui.button(cx, ids!(live_button));
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(color)
                border_color: #(border)
            }
        });
        self.update_live_bbox(cx);
        self.sync_measurements(cx);
    }

    fn set_mirrored(&mut self, cx: &mut Cx, mirrored: bool) {
        self.mirrored = mirrored;
        self.ui
            .image(cx, ids!(live_image))
            .set_uniform(
                cx,
                live_id!(mirror),
                &[if mirrored { 1.0 } else { 0.0 }],
            );
        let widget = self.ui.widget(cx, ids!(body_preview));
        if let Some(mut view) = widget.borrow_mut::<FabricBodyView>() {
            view.set_mirrored(cx, mirrored);
        }
        self.update_live_bbox(cx);
    }

    fn pump_camera_preview(&mut self, cx: &mut Cx) {
        if !self.live {
            return;
        }
        let Some(frame) = self.camera.peek_preview() else {
            return;
        };
        if frame.serial <= self.preview_serial {
            return;
        }
        self.preview_serial = frame.serial;
        let pixels: Vec<u32> = frame
            .rgb
            .chunks_exact(3)
            .map(|rgb| {
                0xff00_0000 | (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2])
            })
            .collect();
        if let Some(texture) = self.preview_texture.as_ref() {
            texture.set_data_u32(cx, frame.width as usize, frame.height as usize, pixels);
        } else {
            self.preview_texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: frame.width as usize,
                    height: frame.height as usize,
                    data: Some(pixels),
                    updated: TextureUpdated::Full,
                },
            ));
        }
        self.ui
            .image(cx, ids!(live_image))
            .set_texture(cx, self.preview_texture.clone());
        self.ui.widget(cx, ids!(live_image)).redraw(cx);
    }

    fn update_live_bbox(&self, cx: &mut Cx) {
        let bbox = self
            .live_bbox
            .zip(self.live_frame_size)
            .map(|(bbox, (width, height))| {
                [
                    (bbox[0] / width as f32).clamp(0.0, 1.0),
                    (bbox[1] / height as f32).clamp(0.0, 1.0),
                    (bbox[2] / width as f32).clamp(0.0, 1.0),
                    (bbox[3] / height as f32).clamp(0.0, 1.0),
                ]
            })
            .unwrap_or([-1.0; 4]);
        let bbox = mirror_normalized_bbox(bbox, self.mirrored);
        self.ui
            .image(cx, ids!(live_image))
            .set_uniform(cx, live_id!(bbox), &bbox);
        self.ui.widget(cx, ids!(live_image)).redraw(cx);
    }

    fn drain_pipeline(&mut self, cx: &mut Cx) {
        let messages = self
            .pipeline
            .as_ref()
            .map(Pipeline::poll)
            .unwrap_or_default();
        for message in messages {
            match message {
                PipelineMessage::Stage(stage) => {
                    if self.live {
                        self.set_progress(cx, format!("live · {stage}"));
                    } else {
                        self.set_progress(cx, stage);
                    }
                }
                PipelineMessage::LiveFrame {
                    fps,
                    model_ms,
                    pose_ms,
                    person,
                    bbox,
                } => {
                    if !self.live {
                        continue;
                    }
                    self.live_bbox = bbox;
                    self.live_frame_size = self.camera.model_size();
                    self.update_live_bbox(cx);
                    if person {
                        self.set_progress(
                            cx,
                            format!(
                                "live · {fps:.1} fps · model {model_ms:.0} ms · pose {pose_ms:.1} ms · person"
                            ),
                        );
                    } else {
                        self.set_progress(cx, "live · no person in frame");
                    }
                }
                PipelineMessage::Done {
                    measured,
                    mesh,
                    posed,
                    pose_mapping,
                    reset_pose,
                } => {
                    self.measurements = measured.values;
                    self.has_measured_body = true;
                    self.sync_measurements(cx);
                    let widget = self.ui.widget(cx, ids!(body_preview));
                    if let Some(mut view) = widget.borrow_mut::<FabricBodyView>() {
                        if reset_pose {
                            view.set_pose(cx, None);
                        }
                        view.set_body(cx, mesh, &measured, pose_mapping);
                        view.set_pose(cx, posed);
                    }
                    if self.live {
                        let now = self
                            .live_started
                            .map(|start| start.elapsed())
                            .unwrap_or_default();
                        let redraft = self.settler.push(now, self.measurements);
                        self.ui
                            .view(cx, ids!(settling_tag))
                            .set_visible(cx, !self.settler.settled);
                        if redraft {
                            self.redraft(cx);
                        }
                    } else if self.pipeline_busy {
                        self.pipeline_busy = false;
                        let seconds = self
                            .pipeline_started
                            .take()
                            .map(|start| start.elapsed().as_secs_f32())
                            .unwrap_or(0.0);
                        self.set_progress(cx, format!("done in {seconds:.1} s"));
                        self.redraft(cx);
                    }
                }
                PipelineMessage::Failed(error) => {
                    if self.live {
                        self.stop_live(cx);
                    } else {
                        self.pipeline_busy = false;
                        self.pipeline_started = None;
                    }
                    self.set_progress(cx, error);
                }
            }
        }
        self.refresh_model_ui(cx);
    }

    fn accept_photo(&mut self, cx: &mut Cx, path: PathBuf) {
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            self.set_progress(cx, "the dropped photo has no file name");
            return;
        };
        self.photo = Some(path.clone());
        if !self.live {
            self.ui
                .label(cx, ids!(photo_name))
                .set_text(cx, &name);
            self.ui
                .label(cx, ids!(drop_title))
                .set_text(cx, "photo loaded");
        }
        self.ui
            .image(cx, ids!(photo_image))
            .set_visible(cx, !self.live);
        if let Err(error) = self
            .ui
            .image(cx, ids!(photo_image))
            .load_image_file_by_path_async(cx, &path)
        {
            self.set_progress(cx, format!("could not decode {name}: {error}"));
            return;
        }
        if self.model_ready {
            self.start_measurement(cx);
        } else {
            self.set_progress(cx, "install the body model first");
        }
        self.refresh_model_ui(cx);
    }

    fn set_drop_highlight(&mut self, cx: &mut Cx, active: bool) {
        if self.drag_over == active {
            return;
        }
        self.drag_over = active;
        let color = if active {
            Vec4f {
                x: 0.10,
                y: 0.18,
                z: 0.24,
                w: 1.0,
            }
        } else {
            Vec4f {
                x: 0.067,
                y: 0.094,
                z: 0.125,
                w: 1.0,
            }
        };
        let border = if active {
            Vec4f {
                x: 0.31,
                y: 0.78,
                z: 1.0,
                w: 1.0,
            }
        } else {
            Vec4f {
                x: 0.25,
                y: 0.32,
                z: 0.39,
                w: 1.0,
            }
        };
        let mut zone = self.ui.view(cx, ids!(drop_zone));
        script_apply_eval!(cx, zone, {
            draw_bg +: {
                color: #(color)
                border_color: #(border)
            }
        });
    }

    fn handle_file_drop(&mut self, cx: &mut Cx, event: &Event) {
        if !matches!(event, Event::Drag(_) | Event::Drop(_) | Event::DragEnd) {
            return;
        }
        let area = self.ui.widget(cx, ids!(drop_zone)).area();
        match event.drag_hits(cx, area) {
            DragHit::Drag(drag) => {
                let accepts = drag.items.iter().any(accepted_photo_item);
                *drag.response.lock().unwrap() = if accepts {
                    DragResponse::Copy
                } else {
                    DragResponse::None
                };
                self.set_drop_highlight(cx, accepts && drag.state != DragState::Out);
            }
            DragHit::Drop(drop) => {
                self.set_drop_highlight(cx, false);
                if let Some(path) = drop.items.iter().find_map(photo_item_path) {
                    self.accept_photo(cx, path);
                }
            }
            DragHit::DragEnd | DragHit::NoHit => self.set_drop_highlight(cx, false),
        }
    }

    fn export(&mut self, cx: &mut Cx, extension: &str) {
        let Some(pattern) = self.pattern.as_ref() else {
            self.set_app_status(cx, "there is no drafted pattern to export");
            return;
        };
        let layout = nest(pattern, 1500.0);
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let directory = makepad_ai_hub::home::makepad_home().join("fabric/exports");
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.set_app_status(cx, format!("could not create {}: {error}", directory.display()));
            return;
        }
        let path = directory.join(export_file_name(&pattern.design_id, extension, seconds));
        let result = match extension {
            "svg" => std::fs::write(&path, to_svg(pattern, &layout).as_bytes()),
            "pdf" => std::fs::write(&path, to_pdf(pattern, &layout, PageSize::A4)),
            _ => return,
        };
        match result {
            Ok(()) => self.set_app_status(cx, format!("exported {}", path.display())),
            Err(error) => self.set_app_status(
                cx,
                format!("could not write {}: {error}", path.display()),
            ),
        }
    }

    fn pump_install_panel(&mut self, cx: &mut Cx) {
        let panel = self.ui.widget(cx, ids!(model_install));
        if let (Some(models), Some(mut panel)) =
            (self.models.as_mut(), panel.borrow_mut::<ModelInstallPanel>())
        {
            panel.pump(cx, models);
        };
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(measure_button)).clicked(actions) {
            self.start_measurement(cx);
        }
        if self.ui.button(cx, ids!(live_button)).clicked(actions) {
            if self.live {
                self.stop_live(cx);
            } else {
                self.start_live(cx);
            }
        }
        if let Some(mirrored) = self
            .ui
            .check_box(cx, ids!(mirror_toggle))
            .changed(actions)
        {
            self.set_mirrored(cx, mirrored);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(design_select)).changed(actions) {
            if index < self.designs.len() {
                self.design_index = index;
                self.configure_options(cx);
                self.redraft(cx);
            }
        }
        if self.ui.button(cx, ids!(copy_all)).clicked(actions) {
            let widget = self.ui.widget(cx, ids!(measurement_grid));
            let text = widget
                .borrow::<FabricMeasurementGrid>()
                .map(|grid| grid.tsv(None))
                .unwrap_or_default();
            cx.copy_to_clipboard(&text);
            self.set_app_status(cx, "measurements copied · paste into a spreadsheet");
        }
        let measurement_uid = self.ui.widget(cx, ids!(measurement_grid)).widget_uid();
        match actions.find_widget_action_cast::<MeasurementListAction>(measurement_uid) {
            MeasurementListAction::Changed { key, value } => {
                if self.measurements.set(key, value) {
                    self.sync_measurements(cx);
                    self.redraft(cx);
                }
            }
            MeasurementListAction::None => {}
        }
        let options_uid = self.ui.widget(cx, ids!(design_options)).widget_uid();
        match actions.find_widget_action_cast::<OptionsListAction>(options_uid) {
            OptionsListAction::Changed { key, value } => {
                self.options.0.insert(key, value);
                self.redraft(cx);
            }
            OptionsListAction::None => {}
        }
        if self.ui.button(cx, ids!(export_svg)).clicked(actions) {
            self.export(cx, "svg");
        }
        if self.ui.button(cx, ids!(export_pdf)).clicked(actions) {
            self.export(cx, "pdf");
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_ai_hub_ui::script_mod(vm);
        crate::body_view::script_mod(vm);
        crate::pattern_view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.startup(cx);
            // The bar shows our title when the window manager hosts us.
            makepad_wm_api::set_title(cx, "Fabric");
        }
        // The window manager asked politely (SUPER+W): go now.
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.handle_file_drop(cx, event);
        self.pump_install_panel(cx);
        match event {
            Event::VideoInputs(inputs) if self.live => {
                if let Some(input) = pick_camera(inputs) {
                    cx.use_video_input(&[input]);
                    self.set_progress(cx, "live · camera ready…");
                } else {
                    self.set_progress(cx, "live · no NV12/YUY2 camera at 640×360");
                }
            }
            Event::PermissionResult(result)
                if self.live
                    && result.permission
                        == makepad_widgets::makepad_platform::permission::Permission::Camera =>
            {
                use makepad_widgets::makepad_platform::permission::PermissionStatus;
                if result.status != PermissionStatus::Granted {
                    let status = format!("camera permission: {:?}", result.status);
                    self.stop_live(cx);
                    self.set_progress(cx, status);
                }
            }
            _ => {}
        }
        if let Event::Signal = event {
            self.drain_pipeline(cx);
        }
        if self.refresh_timer.is_event(event).is_some() {
            self.pump_camera_preview(cx);
            self.refresh_model_ui(cx);
        }
        self.refresh_model_ui(cx);
    }
}

fn accepted_photo_item(item: &DragItem) -> bool {
    photo_item_path(item).is_some()
}

fn mirror_normalized_bbox(bbox: [f32; 4], mirrored: bool) -> [f32; 4] {
    if mirrored && bbox[0] >= 0.0 {
        [1.0 - bbox[2], bbox[1], 1.0 - bbox[0], bbox[3]]
    } else {
        bbox
    }
}

fn photo_item_path(item: &DragItem) -> Option<PathBuf> {
    let DragItem::FilePath {
        path,
        internal_id: None,
    } = item
    else {
        return None;
    };
    let path = Path::new(path);
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(extension.as_str(), "jpg" | "jpeg" | "png").then(|| path.to_path_buf())
}

pub(crate) fn humanise_key(key: &str) -> String {
    let words = key.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub(crate) fn export_file_name(design_id: &str, extension: &str, unix_seconds: u64) -> String {
    let stem: String = design_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "pattern" } else { stem };
    format!("{stem}-{}.{}", utc_timestamp(unix_seconds), extension)
}

fn utc_timestamp(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let seconds = unix_seconds % 86_400;
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u64, u64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096)
            / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u64, day as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanises_measurement_keys() {
        assert_eq!(humanise_key("shoulder_to_bust"), "Shoulder to bust");
        assert_eq!(humanise_key("height"), "Height");
    }

    #[test]
    fn mirror_transform_flips_preview_bbox_horizontally() {
        assert_eq!(
            mirror_normalized_bbox([0.1, 0.2, 0.4, 0.8], true),
            [0.6, 0.2, 0.9, 0.8]
        );
        assert_eq!(
            mirror_normalized_bbox([0.1, 0.2, 0.4, 0.8], false),
            [0.1, 0.2, 0.4, 0.8]
        );
        assert_eq!(mirror_normalized_bbox([-1.0; 4], true), [-1.0; 4]);
    }

    #[test]
    fn export_names_are_safe_and_timestamped() {
        assert_eq!(
            export_file_name("Classic Shirt", "svg", 0),
            "classic-shirt-19700101-000000.svg"
        );
        assert_eq!(
            export_file_name("dress/v2", "pdf", 1_700_000_000),
            "dress-v2-20231114-221320.pdf"
        );
    }

    #[test]
    fn stable_measurements_settle_after_one_and_a_half_seconds() {
        let mut settler = MeasurementSettler::default();
        let measurements = Measurements::sample();
        assert!(!settler.push(Duration::ZERO, measurements));
        assert!(!settler.push(Duration::from_millis(750), measurements));
        assert!(settler.push(Duration::from_millis(1_500), measurements));
        assert!(settler.settled);
        assert!(!settler.push(Duration::from_millis(1_750), measurements));
    }

    #[test]
    fn jittering_measurements_never_settle() {
        let mut settler = MeasurementSettler::default();
        for index in 0..16 {
            let mut measurements = Measurements::sample();
            measurements.bust += index as f32;
            assert!(!settler.push(Duration::from_millis(index * 250), measurements));
            assert!(!settler.settled);
        }
    }
}
