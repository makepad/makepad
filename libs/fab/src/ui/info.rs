//! Lane D. The Info editor — Fab's log area, ours reads the model.
//!
//! Everything on it is live: the file that is open, what the loader is doing
//! right now (with a progress bar while it works), the counts the renderer
//! reports each frame, and the `metadata` key/value pairs the parser found.
//! Nothing here is decorative; if there is no model the panel says so.

use crate::api::*;
use crate::ui::widgets::fold_panel_clicked;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let InfoRow = mod.widgets.FabPropRow{
        name +: { width: 120 }
    }

    mod.widgets.FabInfoPanelBase = #(FabInfoPanel::register_widget(vm))
    mod.widgets.FabInfoPanel = set_type_default() do mod.widgets.FabInfoPanelBase{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: {
            color: fab.color_editor
        }
        header := mod.widgets.FabAreaHeader{
            FabTip{ text: "Choose editor"
                editor_type := mod.widgets.FabDropdownButton{ label +: { text: "Info" } }
            }
            Filler{}
            state := mod.widgets.FabLabelSmall{ text: "" }
        }
        body := mod.widgets.FabScroll{
            padding: Inset{left: 4 right: 4 top: 4 bottom: 8}
            spacing: 2
            status_panel := mod.widgets.FabPanel{
                header +: { hdr +: { title +: { text: "Status" } } }
                body +: {
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: Inset{left: 0 right: 0 top: 2 bottom: 6}
                    row_file := InfoRow{ name +: { text: "File" } }
                    row_state := InfoRow{ name +: { text: "State" } }
                    bar_row := View{
                        width: Fill
                        height: fab.row_height
                        flow: Right
                        align: Align{x: 0.0 y: 0.5}
                        padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
                        spacing: 6
                        mod.widgets.FabLabelDim{ width: 120 text: "Progress" }
                        bar := mod.widgets.FabProgress{ width: Fill }
                    }
                }
            }
            counts_panel := mod.widgets.FabPanel{
                header +: { hdr +: { title +: { text: "Counts" } } }
                body +: {
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: Inset{left: 0 right: 0 top: 2 bottom: 6}
                    row_elements := InfoRow{ name +: { text: "Elements" } }
                    row_stories := InfoRow{ name +: { text: "Stories" } }
                    row_layers := InfoRow{ name +: { text: "Layers" } }
                    row_materials := InfoRow{ name +: { text: "Materials" } }
                    row_sheets := InfoRow{ name +: { text: "Sheets" } }
                    row_tris := InfoRow{ name +: { text: "Triangles" } }
                    row_drawn := InfoRow{ name +: { text: "Drawn" } }
                    row_fps := InfoRow{ name +: { text: "Frame" } }
                    row_gpu := InfoRow{ name +: { text: "GPU memory" } }
                }
            }
            meta_panel := mod.widgets.FabPanel{
                header +: { hdr +: { title +: { text: "Metadata" } } }
                body +: {
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: Inset{left: 0 right: 0 top: 2 bottom: 6}
                    meta_0 := InfoRow{ visible: false }
                    meta_1 := InfoRow{ visible: false }
                    meta_2 := InfoRow{ visible: false }
                    meta_3 := InfoRow{ visible: false }
                    meta_4 := InfoRow{ visible: false }
                    meta_5 := InfoRow{ visible: false }
                    meta_6 := InfoRow{ visible: false }
                    meta_7 := InfoRow{ visible: false }
                    meta_none := mod.widgets.FabLabelMuted{
                        margin: Inset{left: 8 top: 2}
                        text: "No metadata in this model"
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabInfoPanel {
    #[deref]
    view: View,
}

const META_ROWS: usize = 8;

impl FabInfoPanel {
    fn set(&self, cx: &mut Cx, path: &[LiveId], value: &str) {
        self.view.view(cx, path).label(cx, ids!(value)).set_text(cx, value);
    }
}

impl Widget for FabInfoPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            for panel in [
                ids!(body.status_panel),
                ids!(body.counts_panel),
                ids!(body.meta_panel),
            ] {
                fold_panel_clicked(&self.view, cx, actions, panel);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get_mut::<AppState>() {
            let (file, st, progress) = match &state.load {
                LoadStatus::Idle => ("—".to_string(), "Idle".to_string(), 0.0f64),
                LoadStatus::Loading { path, progress } => (
                    path.display().to_string(),
                    format!("{progress:?}"),
                    match progress {
                        crate::model::LoadProgress::Opening => 0.05,
                        crate::model::LoadProgress::Parsing(f) => 0.05 + *f as f64 * 0.45,
                        crate::model::LoadProgress::Meshing { done, total } => {
                            0.5 + (*done as f64 / (*total).max(1) as f64) * 0.3
                        }
                        crate::model::LoadProgress::Building { fraction, .. } => {
                            0.8 + *fraction as f64 * 0.2
                        }
                        crate::model::LoadProgress::Done => 1.0,
                    },
                ),
                LoadStatus::Failed { path, error } => {
                    (path.display().to_string(), format!("Failed: {error}"), 0.0)
                }
                LoadStatus::Loaded { path } => (
                    path.as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "demo house".into()),
                    "Loaded".to_string(),
                    1.0,
                ),
            };
            self.set(cx, ids!(body.status_panel.row_file), &file);
            self.set(cx, ids!(body.status_panel.row_state), &st);
            self.view.label(cx, ids!(header.state)).set_text(cx, &st);
            let mut bar = self.view.view(cx, ids!(body.status_panel.bar_row.bar));
            let p = progress as f32;
            script_apply_eval!(cx, bar, {
                draw_bg +: { progress: #(p) }
            });

            let sc = &state.scene;
            self.set(cx, ids!(body.counts_panel.row_elements), &sc.stats.elements.to_string());
            self.set(cx, ids!(body.counts_panel.row_stories), &sc.stories.len().to_string());
            self.set(cx, ids!(body.counts_panel.row_layers), &sc.layers.len().to_string());
            self.set(cx, ids!(body.counts_panel.row_materials), &sc.materials.len().to_string());
            self.set(cx, ids!(body.counts_panel.row_sheets), &sc.sheets.len().to_string());
            self.set(cx, ids!(body.counts_panel.row_tris), &sc.stats.triangles.to_string());
            let s = state.stats;
            self.set(cx, ids!(body.counts_panel.row_drawn), &s.triangles_drawn.to_string());
            self.set(
                cx,
                ids!(body.counts_panel.row_fps),
                &format!("{:.1} ms · {:.0} fps", s.frame_ms, s.fps),
            );
            self.set(
                cx,
                ids!(body.counts_panel.row_gpu),
                &format!("{:.1} MB", s.gpu_bytes as f64 / 1e6),
            );

            let meta: Vec<(String, String)> = sc.metadata.clone();
            let ids_list = [
                ids!(body.meta_panel.meta_0),
                ids!(body.meta_panel.meta_1),
                ids!(body.meta_panel.meta_2),
                ids!(body.meta_panel.meta_3),
                ids!(body.meta_panel.meta_4),
                ids!(body.meta_panel.meta_5),
                ids!(body.meta_panel.meta_6),
                ids!(body.meta_panel.meta_7),
            ];
            for i in 0..META_ROWS {
                let row = self.view.view(cx, ids_list[i]);
                match meta.get(i) {
                    Some((k, v)) => {
                        row.set_visible(cx, true);
                        row.label(cx, ids!(name)).set_text(cx, k);
                        row.label(cx, ids!(value)).set_text(cx, v);
                    }
                    None => row.set_visible(cx, false),
                }
            }
            self.view
                .widget(cx, ids!(body.meta_panel.meta_none))
                .set_visible(cx, meta.is_empty());
        }
        self.view.draw_walk(cx, scope, walk)
    }
}
