//! The studio canvas: the shared flow canvas (`makepad_flowgraph`) showing
//! the preset's graph, with each node's run state and, on the faces of the
//! nodes that finished, what they made: pictures, waves, text; for a live
//! root, the pictures it is making (`live.rs`). The canvas owns wiring,
//! sockets and hit targets.
//!
//! A preset's nodes are edited in place: every node's face is a
//! [`StudioNodeEditor`] with its settings (an Input's text, a language
//! model's instruction, a generator's MODEL list with each model's size and
//! where it fits, its sizes, steps, seed…), a Run button (this node and
//! what it needs upstream) and, under them, what the node made. The faces
//! are laid out in canvas units, so pointer events reach them through the
//! inverse camera; edits leave as [`StudioCanvasAction`]s.

use super::cards::DrawStudioPicture;
use makepad_flow::Graph;
use makepad_flowgraph::{
    FlowCanvas, FlowCanvasAction, NodeFaces, NodeFacesScope, NodeStatus, Selection,
};
use makepad_widgets::*;
use std::collections::HashMap;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    mod.widgets.StudioNodeFaceBase = #(StudioNodeFace::register_widget(vm))
    mod.widgets.StudioNodeFace = set_type_default() do mod.widgets.StudioNodeFaceBase{
        width: Fill height: 96
        draw_bg +: {color: stage.well}
        draw_bar +: {color: stage.accent}
        draw_text +: {color: stage.label text_style: theme.font_regular{font_size: 8}}
        draw_caption +: {color: stage.glass_ink text_style: theme.font_bold{font_size: 7}}
    }

    let FieldName = Label{width: 76 height: Fit padding: Inset{top: 4} text: ""
        draw_text +: {color: stage.label text_style: theme.font_bold{font_size: 7}}}
    let Field = TextInput{width: Fill height: 22 margin: 0 padding: Inset{left: 5 right: 5 top: 3 bottom: 3} empty_text: "auto"
        draw_text +: {text_style: theme.font_regular{font_size: 8}}}
    let ParamRow = View{visible: false width: Fill height: Fit flow: Right spacing: 4 align: Align{y: 0.5}
        name := FieldName{} value := Field{}}
    let ModelRow = StageButton{visible: false width: Fill height: 20 margin: 0 padding: Inset{left: 6 right: 6}
        align: Align{x: 0.0 y: 0.5}
        draw_text +: {text_style: theme.font_regular{font_size: 7}}}

    mod.widgets.StudioNodeEditorBase = #(StudioNodeEditor::register_widget(vm))
    mod.widgets.StudioNodeEditor = set_type_default() do mod.widgets.StudioNodeEditorBase{
        width: Fill height: Fit flow: Down spacing: 4 padding: Inset{top: 2 bottom: 4}
        text_box := View{visible: false width: Fill height: Fit flow: Down spacing: 2
            text_name := FieldName{width: Fill}
            node_text := TextInput{width: Fill height: Fit margin: 0 is_multiline: true
                padding: Inset{left: 5 right: 5 top: 4 bottom: 4} empty_text: "type here"
                draw_text +: {text_style: theme.font_regular{font_size: 8}}}
        }
        model_box := View{visible: false width: Fill height: Fit flow: Down spacing: 2
            FieldName{width: Fill text: "MODEL"}
            m0 := ModelRow{} m1 := ModelRow{} m2 := ModelRow{} m3 := ModelRow{}
            m4 := ModelRow{} m5 := ModelRow{} m6 := ModelRow{} m7 := ModelRow{}
        }
        p0 := ParamRow{} p1 := ParamRow{} p2 := ParamRow{} p3 := ParamRow{} p4 := ParamRow{} p5 := ParamRow{}
        run_row := View{visible: false width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
            node_run := StageButton{width: Fit height: 22 margin: 0 padding: Inset{left: 10 right: 10} text: "Run"}
            node_note := Label{width: Fill height: Fit text: ""
                draw_text +: {color: stage.secondary text_style: theme.font_regular{font_size: 7}}}
        }
        face := mod.widgets.StudioNodeFace{height: 0}
    }

    mod.widgets.StudioCanvasBase = #(StudioCanvas::register_widget(vm))
    mod.widgets.StudioCanvas = set_type_default() do mod.widgets.StudioCanvasBase{
        width: Fill height: Fill flow: Down
        canvas := FlowCanvas{
            width: Fill height: Fill
            draw_card +: {shadow_color: stage.shadow}
            draw_title +: {color: stage.text}
            draw_meta +: {color: stage.label}
            draw_port +: {color: stage.label}
            draw_chip +: {color: stage.text}
            draw_error +: {color: stage.danger}
            card_color: stage.panel
            card_color_hover: stage.raised
            card_edge_color: stage.border_hi
            accent_color: stage.accent
            highlight_color: stage.accent
            color_input: stage.live
            color_output: stage.live
            color_chat: stage.info
            color_gen: stage.amber
            color_fn: stage.accent
            color_http: stage.info
            color_ask: stage.amber
            color_flow: stage.secondary
            color_port_text: stage.text
            color_port_image: stage.amber
            color_port_audio: stage.info
            color_port_video: stage.accent
            color_port_mesh: stage.live
            color_port_json: stage.amber
            color_port_list: stage.accent
            color_port_bytes: stage.secondary
            color_state_running: stage.info
            color_state_done: stage.live
            color_state_failed: stage.danger
            color_state_waiting: stage.amber
            color_state_inactive: stage.dim
            color_state_idle: stage.dim
            color_port_label_connected: stage.text
            color_port_label_open: stage.label
            icon_check +: {color: stage.live}
            icon_alert +: {color: stage.danger}
            icon_clock +: {color: stage.amber}
            draw_bg +: {color_a: stage.bg color_b: stage.menu}
        }
    }
}

/// What a face shows.
#[derive(Clone, Debug)]
pub enum FaceContent {
    Picture { texture: Texture, caption: String },
    Wave { peaks: Arc<Vec<f32>>, caption: String },
    Text(String),
}

#[derive(Script, ScriptHook, Widget)]
pub struct StudioNodeFace {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_bar: DrawColor,
    #[live]
    draw_picture: DrawStudioPicture,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_caption: DrawText,
    #[rust]
    content: Option<FaceContent>,
}

impl StudioNodeFace {
    pub fn set(&mut self, cx: &mut Cx, content: FaceContent) {
        let height = match &content {
            FaceContent::Picture { texture, .. } => {
                let (w, h) = texture.get_format(cx).vec_width_height().unwrap_or((4, 3));
                // The card is the canvas's node width; keep the picture's
                // aspect within a sensible band.
                (200.0 * h.max(1) as f64 / w.max(1) as f64).clamp(64.0, 260.0)
            }
            FaceContent::Wave { .. } => 48.0,
            FaceContent::Text(text) => (text.lines().count().max(text.len() / 38 + 1).min(8) as f64 * 12.0 + 12.0).max(28.0),
        };
        self.walk.height = Size::Fixed(height);
        self.content = Some(content);
        self.redraw(cx);
    }
}

impl StudioNodeFace {
    /// Nothing to show: no room taken.
    pub fn clear(&mut self, cx: &mut Cx) {
        self.walk.height = Size::Fixed(0.0);
        self.content = None;
        self.redraw(cx);
    }
}

impl Widget for StudioNodeFace {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        let inner = Rect { pos: dvec2(rect.pos.x + 3.0, rect.pos.y + 3.0), size: dvec2(rect.size.x - 6.0, rect.size.y - 6.0) };
        match self.content.clone() {
            Some(FaceContent::Picture { texture, caption }) => {
                let (tw, th) = texture.get_format(cx).vec_width_height().unwrap_or((4, 3));
                let aspect = tw.max(1) as f64 / th.max(1) as f64;
                let (w, h) = if aspect > inner.size.x / inner.size.y { (inner.size.x, inner.size.x / aspect) } else { (inner.size.y * aspect, inner.size.y) };
                self.draw_picture.draw_vars.set_texture(0, &texture);
                self.draw_picture.opacity = 1.0;
                self.draw_picture.draw_abs(cx, Rect { pos: dvec2(inner.pos.x + (inner.size.x - w) * 0.5, inner.pos.y + (inner.size.y - h) * 0.5), size: dvec2(w, h) });
                self.draw_caption.draw_abs(cx, dvec2(inner.pos.x + 4.0, inner.pos.y + inner.size.y - 12.0), &caption);
            }
            Some(FaceContent::Wave { peaks, caption }) => {
                let n = peaks.len().max(1);
                let bar = inner.size.x / n as f64;
                let mid = inner.pos.y + inner.size.y * 0.5;
                for (index, peak) in peaks.iter().enumerate() {
                    let h = (*peak as f64 * inner.size.y).max(1.0);
                    self.draw_bar.draw_abs(cx, Rect { pos: dvec2(inner.pos.x + index as f64 * bar, mid - h * 0.5), size: dvec2((bar - 1.0).max(0.5), h) });
                }
                self.draw_caption.draw_abs(cx, dvec2(inner.pos.x + 4.0, inner.pos.y + 2.0), &caption);
            }
            Some(FaceContent::Text(text)) => {
                let mut y = inner.pos.y + 2.0;
                for line in wrap(&text, ((inner.size.x - 8.0) / 5.0).max(8.0) as usize).into_iter().take(8) {
                    self.draw_text.draw_abs(cx, dvec2(inner.pos.x + 4.0, y), &line);
                    y += 12.0;
                }
            }
            None => {}
        }
        DrawStep::done()
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// Model rows a generator's MODEL list shows at most.
pub const MODEL_ROWS: usize = 8;
/// Setting rows a node shows at most.
pub const PARAM_ROWS: usize = 6;
const MODEL_IDS: [LiveId; MODEL_ROWS] = [live_id!(m0), live_id!(m1), live_id!(m2), live_id!(m3), live_id!(m4), live_id!(m5), live_id!(m6), live_id!(m7)];
const PARAM_IDS: [LiveId; PARAM_ROWS] = [live_id!(p0), live_id!(p1), live_id!(p2), live_id!(p3), live_id!(p4), live_id!(p5)];

/// The person is typing in the field at `area` (an undrawn field has an
/// empty area, which equals "no focus" and is not being typed in).
fn typing(cx: &Cx, area: Area) -> bool {
    !area.is_empty() && cx.has_key_focus(area)
}

/// One entry of a generator's MODEL list.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelChoice {
    /// The model id the node is set to ("" = the hub picks).
    pub id: String,
    /// What the row reads: name, size, where it fits.
    pub label: String,
}

/// What a node's editor offers. Values are text as typed; empty means
/// the node's own default (the hub's choice for a model, a fresh seed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorSpec {
    pub node: String,
    /// The text box: `(caption, text)` (an Input's value, an Llm's
    /// instruction).
    pub text: Option<(String, String)>,
    /// A generator's models; empty: no MODEL list.
    pub models: Vec<ModelChoice>,
    /// The chosen model's id.
    pub model: String,
    /// `(key, value)` setting rows.
    pub params: Vec<(String, String)>,
    /// The node can be run (it is not an output or a library node).
    pub run: bool,
    /// The Run button reads Cancel: a run of this node is under way.
    pub running: bool,
    /// The line beside Run: what it last did.
    pub note: String,
}

/// A node's face on the FLOW canvas: its settings, Run, and its result.
#[derive(Script, ScriptHook, Widget)]
pub struct StudioNodeEditor {
    #[deref]
    view: View,
    #[rust]
    spec: EditorSpec,
}

impl StudioNodeEditor {
    /// Show `spec`. A field the person is typing in keeps its text.
    pub fn apply(&mut self, cx: &mut Cx, spec: &EditorSpec) {
        let text_box = self.view.view(cx, ids!(text_box));
        text_box.set_visible(cx, spec.text.is_some());
        if let Some((caption, text)) = &spec.text {
            self.view.label(cx, ids!(text_name)).set_text(cx, caption);
            let input = self.view.text_input(cx, ids!(node_text));
            if !typing(cx, input.area()) && input.text() != *text {
                input.set_text(cx, text);
            }
        }
        self.view.view(cx, ids!(model_box)).set_visible(cx, !spec.models.is_empty());
        let palette = makepad_look::palette::get(cx);
        for (index, id) in MODEL_IDS.iter().enumerate() {
            let mut row = self.view.button(cx, &[*id]);
            match spec.models.get(index) {
                Some(choice) => {
                    row.set_visible(cx, true);
                    row.set_text(cx, &choice.label);
                    let selected = choice.id == spec.model;
                    let face = Vec4f::from_u32(if selected { palette.accent } else { palette.raised });
                    let hover = Vec4f::from_u32(if selected { palette.accent_hover } else { palette.control_top });
                    let ink = Vec4f::from_u32(if selected { palette.on_accent } else { palette.label });
                    script_apply_eval!(cx, row, {
                        draw_bg +: {color: #(face) color_focus: #(face) color_hover: #(hover) color_down: #(hover)}
                        draw_text +: {color: #(ink) color_focus: #(ink) color_hover: #(ink) color_down: #(ink)}
                    });
                }
                None => row.set_visible(cx, false),
            }
        }
        for (index, id) in PARAM_IDS.iter().enumerate() {
            let row = self.view.view(cx, &[*id]);
            match spec.params.get(index) {
                Some((key, value)) => {
                    row.set_visible(cx, true);
                    row.label(cx, ids!(name)).set_text(cx, &key.to_uppercase());
                    let input = row.text_input(cx, ids!(value));
                    if !typing(cx, input.area()) && input.text() != *value {
                        input.set_text(cx, value);
                    }
                }
                None => row.set_visible(cx, false),
            }
        }
        self.view.view(cx, ids!(run_row)).set_visible(cx, spec.run);
        self.view.button(cx, ids!(node_run)).set_text(cx, if spec.running { "Cancel" } else { "Run" });
        self.view.label(cx, ids!(node_note)).set_text(cx, &spec.note);
        self.spec = spec.clone();
        self.view.redraw(cx);
    }

    /// What the node made (or nothing).
    pub fn set_content(&mut self, cx: &mut Cx, content: Option<FaceContent>) {
        let face = self.view.widget(cx, ids!(face));
        if let Some(mut face) = face.borrow_mut::<StudioNodeFace>() {
            match content {
                Some(content) => face.set(cx, content),
                None => face.clear(cx),
            }
        }
        self.view.redraw(cx);
    }

    /// The edits in `actions`, as the canvas reports them.
    fn edits(&self, cx: &mut Cx, actions: &Actions) -> Vec<StudioCanvasAction> {
        let node = self.spec.node.clone();
        let mut out = Vec::new();
        if let Some(text) = self.view.text_input(cx, ids!(node_text)).changed(actions) {
            out.push(StudioCanvasAction::Text { node: node.clone(), text });
        }
        for (index, id) in MODEL_IDS.iter().enumerate() {
            if self.view.button(cx, &[*id]).clicked(actions) {
                if let Some(choice) = self.spec.models.get(index) {
                    out.push(StudioCanvasAction::Model { node: node.clone(), model: choice.id.clone() });
                }
            }
        }
        for (index, id) in PARAM_IDS.iter().enumerate() {
            let row = self.view.view(cx, &[*id]);
            if let (Some(value), Some((key, _))) = (row.text_input(cx, ids!(value)).changed(actions), self.spec.params.get(index)) {
                out.push(StudioCanvasAction::Param { node: node.clone(), key: key.clone(), value });
            }
        }
        if self.view.button(cx, ids!(node_run)).clicked(actions) {
            out.push(StudioCanvasAction::Run(node));
        }
        out
    }
}

impl Widget for StudioNodeEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

#[derive(Default)]
struct StudioFaces {
    /// Faces put on directly (a live root's pictures).
    widgets: HashMap<String, WidgetRef>,
    /// The preset's node editors.
    editors: HashMap<String, WidgetRef>,
}

impl StudioFaces {
    fn face(&self, node: &str) -> Option<&WidgetRef> {
        self.widgets.get(node).or_else(|| self.editors.get(node))
    }
}

impl NodeFaces for StudioFaces {
    fn draw_face(&mut self, cx: &mut Cx2d, node: &str, mut walk: Walk, card_sized: bool) {
        let Some(widget) = self.face(node) else { return };
        if !card_sized && self.widgets.contains_key(node) {
            walk.height = match widget.walk(cx).height { Size::Fixed(height) => Size::Fixed(height), _ => Size::Fixed(96.0) };
        }
        widget.draw_walk_all(cx, &mut Scope::empty(), walk);
    }

    fn set_z_order(&mut self, _order: &[String]) {}

    fn set_popup_anchor_transform(&mut self, _cx: &mut Cx, _transform: Option<PopupAnchorTransform>) {}
}

#[derive(Clone, Debug, Default)]
pub enum StudioCanvasAction {
    #[default]
    None,
    Select(Option<String>),
    /// A node's text box changed.
    Text { node: String, text: String },
    /// A node's setting changed (as typed).
    Param { node: String, key: String, value: String },
    /// A generator's model was picked ("" = the hub picks).
    Model { node: String, model: String },
    /// Run this node (and what it needs upstream), or cancel its run.
    Run(String),
}

#[derive(Script, ScriptHook, Widget)]
pub struct StudioCanvas {
    #[deref]
    view: View,
    #[rust]
    faces: StudioFaces,
    /// What each node's editor shows, by node.
    #[rust]
    specs: Vec<EditorSpec>,
    /// What each node made, by node, for its editor.
    #[rust]
    contents: HashMap<String, FaceContent>,
    #[rust]
    fit_next: NextFrame,
    #[rust]
    fit_frames: u8,
}

impl StudioCanvas {
    fn with_canvas<R>(&mut self, cx: &mut Cx, f: impl FnOnce(&mut FlowCanvas, &mut Cx) -> R) -> Option<R> {
        let widget = self.view.widget(cx, ids!(canvas));
        let mut canvas = widget.borrow_mut::<FlowCanvas>()?;
        Some(f(&mut canvas, cx))
    }

    /// Show `graph` (or nothing). A new graph is fitted once it has been
    /// measured.
    pub fn set_graph(&mut self, cx: &mut Cx, graph: Option<&Graph>, fit: bool) {
        let view = graph.map(makepad_flow_ui::graph_view::view_of);
        let compatible = graph.map(makepad_flow_ui::graph_view::compatibility_of);
        self.with_canvas(cx, |canvas, cx| {
            canvas.set_graph(cx, view);
            if let Some(compatible) = compatible {
                canvas.set_compatible_ports(compatible);
            }
        });
        if fit {
            // Faces and cards measure on their first draw; fit after it.
            self.fit_frames = 2;
            self.fit_next = cx.new_next_frame();
        }
    }

    pub fn fit(&mut self, cx: &mut Cx) {
        self.with_canvas(cx, |canvas, cx| canvas.fit(cx));
    }

    pub fn set_status(&mut self, cx: &mut Cx, node: &str, status: NodeStatus) {
        self.with_canvas(cx, |canvas, cx| canvas.set_status(cx, node, status));
    }

    pub fn clear_run(&mut self, cx: &mut Cx) {
        self.with_canvas(cx, |canvas, cx| canvas.clear_run(cx));
    }

    /// The node editors: one per spec, kept across calls (a field being
    /// typed in keeps its text and focus); nodes no longer listed lose
    /// theirs.
    pub fn set_editors(&mut self, cx: &mut Cx, specs: Vec<EditorSpec>) {
        if specs == self.specs {
            return;
        }
        let before = self.faces.editors.len();
        self.faces.editors.retain(|node, _| specs.iter().any(|spec| &spec.node == node));
        let mut added = before != self.faces.editors.len();
        for spec in &specs {
            let widget = match self.faces.editors.get(&spec.node) {
                Some(widget) => widget.clone(),
                None => {
                    let widget = cx.with_vm(|vm| {
                        let value = script_eval!(vm, {
                            use mod.prelude.widgets.*
                            use mod.widgets.*
                            StudioNodeEditor{}
                        });
                        WidgetRef::script_from_value(vm, value)
                    });
                    self.faces.editors.insert(spec.node.clone(), widget.clone());
                    added = true;
                    widget
                }
            };
            if let Some(mut editor) = widget.borrow_mut::<StudioNodeEditor>() {
                editor.apply(cx, spec);
                if added {
                    editor.set_content(cx, self.contents.get(&spec.node).cloned());
                }
            };
        }
        self.specs = specs;
        if added {
            self.sync_roots(cx);
        }
        self.view.redraw(cx);
    }

    /// Put `content` on `node`'s face: in its editor, else a face of its
    /// own.
    pub fn set_face(&mut self, cx: &mut Cx, node: &str, content: FaceContent) {
        self.contents.insert(node.to_string(), content.clone());
        if let Some(widget) = self.faces.editors.get(node) {
            if let Some(mut editor) = widget.borrow_mut::<StudioNodeEditor>() {
                editor.set_content(cx, Some(content));
            }
            self.view.redraw(cx);
            return;
        }
        let widget = match self.faces.widgets.get(node) {
            Some(widget) => widget.clone(),
            None => {
                let widget = cx.with_vm(|vm| {
                    let value = script_eval!(vm, {
                        use mod.prelude.widgets.*
                        use mod.widgets.*
                        StudioNodeFace{}
                    });
                    WidgetRef::script_from_value(vm, value)
                });
                self.faces.widgets.insert(node.to_string(), widget.clone());
                self.sync_roots(cx);
                widget
            }
        };
        if let Some(mut face) = widget.borrow_mut::<StudioNodeFace>() {
            face.set(cx, content);
        }
        self.view.redraw(cx);
    }

    /// Put a ready-made face widget on `node` (a live root's picture).
    pub fn set_face_widget(&mut self, cx: &mut Cx, node: &str, widget: WidgetRef) {
        self.faces.widgets.insert(node.to_string(), widget);
        self.sync_roots(cx);
        self.view.redraw(cx);
    }

    /// Take every result off the faces (the editors stay).
    pub fn clear_faces(&mut self, cx: &mut Cx) {
        self.contents.clear();
        for widget in self.faces.editors.values() {
            if let Some(mut editor) = widget.borrow_mut::<StudioNodeEditor>() {
                editor.set_content(cx, None);
            }
        }
        if !self.faces.widgets.is_empty() {
            self.faces.widgets.clear();
            self.sync_roots(cx);
        }
        self.view.redraw(cx);
    }

    fn sync_roots(&mut self, cx: &mut Cx) {
        let roots = self.faces.widgets.iter().chain(self.faces.editors.iter().filter(|(node, _)| !self.faces.widgets.contains_key(*node)))
            .map(|(node, widget)| (LiveId::from_str(node), widget.clone())).collect();
        self.with_canvas(cx, |canvas, cx| canvas.set_face_roots(cx, roots));
    }

    /// Deliver `event` to the node editors in canvas units (through the
    /// inverse camera); a hit they take is not the canvas's.
    fn handle_editors(&mut self, cx: &mut Cx, event: &Event) {
        if self.faces.editors.is_empty() {
            return;
        }
        let Some(camera) = self.with_canvas(cx, |canvas, _| canvas.camera()) else { return };
        let remapped = makepad_flow_ui::camera_events::remap_event(event, &camera);
        let delivered = remapped.as_ref().unwrap_or(event);
        let editors: Vec<WidgetRef> = self.faces.editors.iter()
            .filter(|(node, _)| !self.faces.widgets.contains_key(*node))
            .map(|(_, widget)| widget.clone()).collect();
        let mut edits = Vec::new();
        for widget in editors {
            let actions = cx.capture_actions(|cx| widget.handle_event(cx, delivered, &mut Scope::empty()));
            if actions.is_empty() {
                continue;
            }
            if let Some(editor) = widget.borrow::<StudioNodeEditor>() {
                edits.extend(editor.edits(cx, &actions));
            }
            cx.extend_actions(actions);
        }
        if let Some(remapped) = remapped.as_ref() {
            makepad_flow_ui::camera_events::sync_handled(event, remapped, &camera);
        }
        for edit in edits {
            cx.widget_action(self.widget_uid(), edit);
        }
    }
}

impl Widget for StudioCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.fit_next.is_event(event).is_some() {
            self.fit(cx);
            self.fit_frames = self.fit_frames.saturating_sub(1);
            if self.fit_frames > 0 {
                self.fit_next = cx.new_next_frame();
            }
        }
        self.handle_editors(cx, event);
        let mut faces = NodeFacesScope::new(&mut self.faces);
        self.view.handle_event(cx, event, &mut Scope::with_data(&mut faces));
        if let Event::Actions(actions) = event {
            let uid = self.view.widget(cx, ids!(canvas)).widget_uid();
            for action in actions.filter_widget_actions_cast::<FlowCanvasAction>(uid) {
                match action {
                    FlowCanvasAction::Select(selection) => {
                        let node = selection.as_ref().and_then(Selection::node).map(str::to_string);
                        cx.widget_action(self.widget_uid(), StudioCanvasAction::Select(node));
                    }
                    _ => {}
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut faces = NodeFacesScope::new(&mut self.faces);
        self.view.draw_walk(cx, &mut Scope::with_data(&mut faces), walk)
    }
}

impl StudioCanvasRef {
    pub fn set_graph(&self, cx: &mut Cx, graph: Option<&Graph>, fit: bool) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_graph(cx, graph, fit); }
    }
    pub fn fit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.fit(cx); }
    }
    pub fn set_status(&self, cx: &mut Cx, node: &str, status: NodeStatus) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_status(cx, node, status); }
    }
    pub fn clear_run(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.clear_run(cx); }
    }
    pub fn set_face(&self, cx: &mut Cx, node: &str, content: FaceContent) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_face(cx, node, content); }
    }
    pub fn set_face_widget(&self, cx: &mut Cx, node: &str, widget: WidgetRef) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_face_widget(cx, node, widget); }
    }
    pub fn clear_faces(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() { inner.clear_faces(cx); }
    }
    pub fn set_editors(&self, cx: &mut Cx, specs: Vec<EditorSpec>) {
        if let Some(mut inner) = self.borrow_mut() { inner.set_editors(cx, specs); }
    }
    pub fn actions(&self, actions: &Actions) -> Vec<StudioCanvasAction> {
        actions.filter_widget_actions_cast::<StudioCanvasAction>(self.widget_uid()).collect()
    }
}
