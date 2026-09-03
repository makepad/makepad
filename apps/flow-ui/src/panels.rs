//! The panels around the canvas (DESIGN.md §8): the inspector for the
//! selected node, the Running list of instances, the palette of prelude
//! types, and the App view that shows a flow as a product.

use crate::faces::{param_text, FaceHost};
use makepad_flow::{
    Graph, InstanceRow, Literal, Node, NodeInputValue, NodeTypeCatalog, ValueBytes, ValueRef,
};
use makepad_widgets::makepad_micro_serde::SerJson;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let RowLabel = Label{
        width: 90
        height: Fit
        draw_text +: {color: theme.color_text_meta}
    }

    mod.widgets.InspectorBase = #(Inspector::register_widget(vm))
    mod.widgets.Inspector = set_type_default() do mod.widgets.InspectorBase{
        width: Fill
        height: Fill
        flow: Down
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Head := View{
                width: Fill
                height: Fit
                flow: Down
                padding: theme.mspace_2
                title := H3{text: ""}
                doc := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {color: theme.color_text_meta}
                }
            }
            Section := View{
                width: Fill
                height: Fit
                padding: theme.mspace_2
                title := H3{text: ""}
            }
            Text := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_1
                align: Align{y: 0.5}
                name := RowLabel{}
                value := TextInput{
                    width: Fill
                    height: 28
                }
            }
            Multiline := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_1
                padding: theme.mspace_1
                head := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    align: Align{y: 0.5}
                    name := RowLabel{width: Fill}
                    apply := ButtonFlat{text: "Apply"}
                }
                value := TextInput{
                    width: Fill
                    height: 110
                    is_multiline: true
                    draw_text +: {text_style: theme.font_code{}}
                }
            }
            Range := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_1
                align: Align{y: 0.5}
                name := RowLabel{}
                value := Slider{
                    width: Fill
                    height: Fit
                    text: ""
                }
            }
            Choice := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_1
                align: Align{y: 0.5}
                name := RowLabel{}
                value := DropDown{
                    width: Fill
                    height: Fit
                }
            }
            Wired := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_1
                align: Align{y: 0.5}
                name := RowLabel{}
                value := Label{
                    width: Fill
                    height: Fit
                    text: ""
                }
            }
            Output := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_1
                align: Align{y: 0.5}
                name := RowLabel{}
                value := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {text_style: theme.font_code{}}
                }
                open := ButtonFlat{text: "Open"}
            }
            Preview := View{
                width: Fill
                height: Fit
                flow: Down
                padding: theme.mspace_1
                text := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {text_style: theme.font_code{}}
                }
                image := Image{
                    width: Fill
                    height: 260
                    fit: ImageFit.Smallest
                }
            }
        }
    }

    mod.widgets.RunningListBase = #(RunningList::register_widget(vm))
    mod.widgets.RunningList = set_type_default() do mod.widgets.RunningListBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Item := View{
                width: Fill
                height: Fit
                flow: Down
                padding: theme.mspace_1
                attach := ButtonFlatter{
                    width: Fill
                    text: ""
                }
                detail := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {color: theme.color_text_meta}
                }
                actions := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: theme.space_1
                    stop := ButtonFlatter{text: "Stop"}
                    dup := ButtonFlatter{text: "Duplicate"}
                    copy := ButtonFlatter{text: "Copy id"}
                }
            }
        }
    }

    mod.widgets.PaletteBase = #(Palette::register_widget(vm))
    mod.widgets.Palette = set_type_default() do mod.widgets.PaletteBase{
        width: Fill
        height: Fill
        flow: Down
        list := PortalList{
            width: Fill
            height: Fill
            // A press-and-drag here carries a type to the canvas; it must
            // not scroll the list out from under the next pick.
            drag_scrolling: false
            scroll_bar: ScrollBar{}
            Kind := View{
                width: Fill
                height: Fit
                padding: theme.mspace_1
                title := Label{
                    text: ""
                    draw_text +: {color: theme.color_text_meta}
                }
            }
            Type := View{
                width: Fill
                height: 30
                flow: Right
                align: Align{y: 0.5}
                name := ButtonFlatter{
                    width: Fill
                    text: ""
                }
            }
        }
    }

    mod.widgets.AppViewBase = #(AppView::register_widget(vm))
    mod.widgets.AppView = set_type_default() do mod.widgets.AppViewBase{
        width: Fill
        height: Fill
        flow: Down
        padding: theme.mspace_3
        spacing: theme.space_2
        draw_bg +: {color: theme.color_bg_app}
        draw_frame +: {color: theme.color_bg_container}
        draw_text +: {
            text_style: theme.font_bold{font_size: 10}
            color: theme.color_label_inner
        }
    }
}

// ---------------------------------------------------------------------------
// Inspector
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum InspectorAction {
    #[default]
    None,
    SetParam {
        node: String,
        key: String,
        value: Literal,
    },
    SetFnSrc {
        node: String,
        src: String,
    },
    SetFaceSrc {
        node: String,
        src: String,
    },
    OpenValue {
        node: String,
        port: String,
        value: ValueRef,
    },
}

#[derive(Clone, Debug)]
enum Row {
    Head {
        title: String,
        doc: String,
    },
    Section(String),
    Text {
        key: String,
        value: String,
        id: bool,
        number: bool,
    },
    Multiline {
        key: String,
        value: String,
    },
    Range {
        key: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
    },
    Choice {
        key: String,
        value: String,
        options: Vec<String>,
    },
    Wired {
        key: String,
        value: String,
    },
    Output {
        port: String,
        value: ValueRef,
        chip: String,
    },
    Preview {
        text: String,
        image: Option<ValueBytes>,
    },
}

impl Row {
    fn template(&self) -> LiveId {
        match self {
            Row::Head { .. } => live_id!(Head),
            Row::Section(_) => live_id!(Section),
            Row::Text { .. } => live_id!(Text),
            Row::Multiline { .. } => live_id!(Multiline),
            Row::Range { .. } => live_id!(Range),
            Row::Choice { .. } => live_id!(Choice),
            Row::Wired { .. } => live_id!(Wired),
            Row::Output { .. } => live_id!(Output),
            Row::Preview { .. } => live_id!(Preview),
        }
    }
}

fn literal_text(value: &Literal) -> String {
    match value {
        Literal::Null => String::new(),
        Literal::Bool(value) => value.to_string(),
        Literal::Num(value) => {
            if value.fract() == 0.0 {
                (*value as i64).to_string()
            } else {
                value.to_string()
            }
        }
        Literal::Str(value) | Literal::Id(value) => value.clone(),
        Literal::Arr(values) => values
            .iter()
            .map(literal_text)
            .collect::<Vec<_>>()
            .join(", "),
        Literal::Obj(_) => Literal::serialize_json(value),
    }
}

/// `256..2048 step 64` and `one of: a, b, c` hints from a param doc.
fn parse_range(doc: &str) -> Option<(f64, f64, f64)> {
    let doc = doc.trim();
    let (range, rest) = match doc.split_once(" step ") {
        Some((range, rest)) => (range, Some(rest)),
        None => (doc, None),
    };
    let (min, max) = range.split_once("..")?;
    let min: f64 = min.trim().parse().ok()?;
    let max: f64 = max
        .trim()
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .next()?
        .parse()
        .ok()?;
    let step = rest
        .and_then(|rest| {
            rest.trim()
                .split(|c: char| !(c.is_ascii_digit() || c == '.'))
                .next()?
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(1.0);
    Some((min, max, step))
}

fn parse_choices(doc: &str) -> Option<Vec<String>> {
    let (_, list) = doc.split_once("one of:")?;
    let list = list.split('.').next().unwrap_or(list);
    let options: Vec<String> = list
        .split(',')
        .map(|item| item.trim().trim_end_matches('.').to_string())
        .filter(|item| !item.is_empty())
        .collect();
    (!options.is_empty()).then_some(options)
}

#[derive(Script, ScriptHook, Widget)]
pub struct Inspector {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    node: Option<String>,
    #[rust]
    preview: Option<(String, ValueBytes)>,
}

impl Inspector {
    /// Rebuild the rows for a node from the graph, the catalog and the run's
    /// last outputs for it.
    pub fn show_node(
        &mut self,
        cx: &mut Cx,
        graph: Option<&Graph>,
        catalog: &[NodeTypeCatalog],
        node_id: Option<&str>,
        outputs: &[(String, ValueRef)],
    ) {
        self.rows.clear();
        self.node = node_id.map(str::to_string);
        let node = graph.and_then(|graph| graph.nodes.iter().find(|node| Some(node.id.as_str()) == node_id));
        let Some(node) = node else {
            self.rows.push(Row::Head {
                title: "Nothing selected".into(),
                doc: "Click a node on the canvas to edit its params.".into(),
            });
            self.redraw(cx);
            return;
        };
        let entry = catalog.iter().find(|entry| entry.type_name == node.type_name);
        self.rows.push(Row::Head {
            title: format!("{} · {}", node.id, node.type_name),
            doc: node
                .doc
                .clone()
                .or_else(|| entry.map(|entry| entry.doc.clone()))
                .unwrap_or_default(),
        });
        self.rows.push(Row::Section("Params".into()));
        for (key, value) in &node.params {
            if matches!(key.as_str(), "ui" | "at" | "ports" | "domain" | "out" if node.type_name == "Fn" || key != "out")
            {
                if key == "out" && node.type_name == "Fn" {
                    // Output names are edited through the code, not here.
                    self.rows.push(Row::Wired {
                        key: key.clone(),
                        value: literal_text(value),
                    });
                }
                continue;
            }
            let doc = entry
                .and_then(|entry| entry.params.iter().find(|param| &param.name == key))
                .map(|param| param.doc.clone())
                .unwrap_or_default();
            let range = entry
                .and_then(|entry| entry.params.iter().find(|param| &param.name == key))
                .and_then(|param| param.range.as_ref())
                .map(|range| (range.min, range.max, range.step.unwrap_or(1.0)))
                .or_else(|| parse_range(&doc));
            if let (Some((min, max, step)), Literal::Num(number)) = (range, value) {
                self.rows.push(Row::Range {
                    key: key.clone(),
                    value: *number,
                    min,
                    max,
                    step,
                });
                continue;
            }
            if let Some(options) = parse_choices(&doc) {
                self.rows.push(Row::Choice {
                    key: key.clone(),
                    value: literal_text(value),
                    options,
                });
                continue;
            }
            if key == "type" || key == "method" || key == "out" || key == "on_fail" {
                let options: Vec<String> = match key.as_str() {
                    "type" | "out" => ["text", "image", "audio", "video", "mesh", "json", "list", "bytes"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    "method" => ["get", "post", "put", "delete"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    _ => ["fail", "skip"].iter().map(|s| s.to_string()).collect(),
                };
                self.rows.push(Row::Choice {
                    key: key.clone(),
                    value: literal_text(value),
                    options,
                });
                continue;
            }
            match value {
                Literal::Str(text)
                    if matches!(
                        key.as_str(),
                        "system" | "prompt" | "question" | "negative" | "default" | "brief"
                    ) || text.contains('\n') =>
                {
                    self.rows.push(Row::Multiline {
                        key: key.clone(),
                        value: text.clone(),
                    });
                }
                Literal::Num(number) => self.rows.push(Row::Text {
                    key: key.clone(),
                    value: literal_text(&Literal::Num(*number)),
                    id: false,
                    number: true,
                }),
                Literal::Id(text) => self.rows.push(Row::Text {
                    key: key.clone(),
                    value: text.clone(),
                    id: true,
                    number: false,
                }),
                other => self.rows.push(Row::Text {
                    key: key.clone(),
                    value: literal_text(other),
                    id: false,
                    number: false,
                }),
            }
        }
        if !node.inputs.is_empty() {
            self.rows.push(Row::Section("Inputs".into()));
        }
        for input in &node.inputs {
            if node.params.iter().any(|(key, _)| key == &input.port) {
                continue;
            }
            match &input.value {
                NodeInputValue::Edge(edge) => self.rows.push(Row::Wired {
                    key: input.port.clone(),
                    value: format!("← {}.{} ({})", edge.from_node, edge.from_port, input.ty.as_str()),
                }),
                NodeInputValue::Literal(Literal::Null) => self.rows.push(Row::Wired {
                    key: input.port.clone(),
                    value: format!("(unwired {})", input.ty.as_str()),
                }),
                NodeInputValue::Literal(Literal::Str(text)) if input.port == "prompt" => {
                    self.rows.push(Row::Multiline {
                        key: input.port.clone(),
                        value: text.clone(),
                    })
                }
                NodeInputValue::Literal(value) => self.rows.push(Row::Text {
                    key: input.port.clone(),
                    value: literal_text(value),
                    id: matches!(value, Literal::Id(_)),
                    number: matches!(value, Literal::Num(_)),
                }),
            }
        }
        if node.type_name == "Fn" {
            self.rows.push(Row::Section("Code".into()));
            self.rows.push(Row::Multiline {
                key: "run".into(),
                value: node.fn_src.clone().unwrap_or_default(),
            });
        }
        self.rows.push(Row::Section("Face".into()));
        self.rows.push(Row::Multiline {
            key: "ui".into(),
            value: node.face_src.clone().unwrap_or_default(),
        });
        if !outputs.is_empty() {
            self.rows.push(Row::Section("Last outputs".into()));
            for (port, value) in outputs {
                let chip = crate::faces::preview_text(value)
                    .unwrap_or_else(|| format!("{} · {} bytes", value.content_type, value.bytes));
                let chip: String = chip.chars().take(80).collect();
                self.rows.push(Row::Output {
                    port: port.clone(),
                    value: value.clone(),
                    chip,
                });
            }
        }
        if let Some((text, image)) = self.preview.clone() {
            self.rows.push(Row::Section("Value".into()));
            let is_image = image.content_type.starts_with("image/");
            self.rows.push(Row::Preview {
                text: if is_image { String::new() } else { text },
                image: is_image.then_some(image),
            });
        }
        self.redraw(cx);
    }

    pub fn set_preview(&mut self, preview: Option<(String, ValueBytes)>) {
        self.preview = preview;
    }

    /// Edits made in the rows, as actions for the app.
    pub fn changes(&self, cx: &mut Cx, actions: &Actions) -> Vec<InspectorAction> {
        let mut out = Vec::new();
        let Some(node) = self.node.clone() else {
            return out;
        };
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(row) = self.rows.get(index) else {
                continue;
            };
            match row {
                Row::Text {
                    key, id, number, ..
                } => {
                    if let Some((text, _)) = item.text_input(cx, ids!(value)).returned(actions) {
                        let value = if *number {
                            text.trim()
                                .parse::<f64>()
                                .map(Literal::Num)
                                .unwrap_or(Literal::Str(text))
                        } else if *id {
                            Literal::Id(text.trim().trim_start_matches('@').to_string())
                        } else {
                            Literal::Str(text)
                        };
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: key.clone(),
                            value,
                        });
                    }
                }
                Row::Multiline { key, .. } => {
                    if item.button(cx, ids!(apply)).clicked(actions) {
                        let text = item.text_input(cx, ids!(value)).text();
                        out.push(match key.as_str() {
                            "run" => InspectorAction::SetFnSrc {
                                node: node.clone(),
                                src: text,
                            },
                            "ui" => InspectorAction::SetFaceSrc {
                                node: node.clone(),
                                src: text,
                            },
                            _ => InspectorAction::SetParam {
                                node: node.clone(),
                                key: key.clone(),
                                value: Literal::Str(text),
                            },
                        });
                    }
                }
                Row::Range { key, .. } => {
                    if let Some(value) = item.slider(cx, ids!(value)).end_slide(actions) {
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: key.clone(),
                            value: Literal::Num(value),
                        });
                    }
                }
                Row::Choice { key, .. } => {
                    if let Some(label) = item.drop_down(cx, ids!(value)).changed_label(actions) {
                        let value = if matches!(key.as_str(), "type" | "method" | "out" | "on_fail") {
                            Literal::Id(label)
                        } else if let Ok(number) = label.parse::<f64>() {
                            Literal::Num(number)
                        } else {
                            Literal::Str(label)
                        };
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: key.clone(),
                            value,
                        });
                    }
                }
                Row::Output { port, value, .. } => {
                    if item.button(cx, ids!(open)).clicked(actions) {
                        out.push(InspectorAction::OpenValue {
                            node: node.clone(),
                            port: port.clone(),
                            value: value.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        out
    }
}

impl Widget for Inspector {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(row) = self.rows.get(index) else {
                    continue;
                };
                let (item, existed) = list.item_with_existed(cx, index, row.template());
                match row {
                    Row::Head { title, doc } => {
                        item.label(cx, ids!(title)).set_text(cx, title);
                        item.label(cx, ids!(doc)).set_text(cx, doc);
                    }
                    Row::Section(title) => item.label(cx, ids!(title)).set_text(cx, title),
                    Row::Text { key, value, .. } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        if !existed {
                            item.text_input(cx, ids!(value)).set_text(cx, value);
                        }
                    }
                    Row::Multiline { key, value } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        if !existed {
                            item.text_input(cx, ids!(value)).set_text(cx, value);
                        }
                    }
                    Row::Range {
                        key,
                        value,
                        min,
                        max,
                        step,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        let mut slider = item.slider(cx, ids!(value));
                        if !existed {
                            script_apply_eval!(cx, slider, {
                                min: #(*min)
                                max: #(*max)
                                step: #(*step)
                                precision: #(if step.fract() == 0.0 { 0usize } else { 2usize })
                            });
                            slider.set_value(cx, *value);
                        }
                    }
                    Row::Choice {
                        key,
                        value,
                        options,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        let drop_down = item.drop_down(cx, ids!(value));
                        if !existed {
                            drop_down.set_labels(cx, options.clone());
                            drop_down.set_selected_by_label(value, cx);
                        }
                    }
                    Row::Wired { key, value } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(value)).set_text(cx, value);
                    }
                    Row::Output { port, chip, .. } => {
                        item.label(cx, ids!(name)).set_text(cx, port);
                        item.label(cx, ids!(value)).set_text(cx, chip);
                    }
                    Row::Preview { text, image } => {
                        item.label(cx, ids!(text)).set_text(cx, text);
                        let image_ref = item.image(cx, ids!(image));
                        match image {
                            Some(bytes) if !existed => {
                                let loaded = if bytes.content_type.contains("jpeg") {
                                    image_ref.load_jpg_from_data(cx, &bytes.bytes)
                                } else {
                                    image_ref.load_png_from_data(cx, &bytes.bytes)
                                };
                                image_ref.set_visible(cx, loaded.is_ok());
                            }
                            Some(_) => {}
                            None => image_ref.set_visible(cx, false),
                        }
                    }
                }
                item.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum RunningAction {
    #[default]
    None,
    Attach(String),
    Stop(String),
    Duplicate(String),
    CopyId(String),
}

#[derive(Script, ScriptHook, Widget)]
pub struct RunningList {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<InstanceRow>,
    #[rust]
    attached: Option<String>,
    #[rust]
    now_ms: u64,
}

impl RunningList {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<InstanceRow>, attached: Option<String>) {
        self.rows = rows;
        self.attached = attached;
        self.now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.redraw(cx);
    }

    pub fn actions(&self, cx: &mut Cx, actions: &Actions) -> Vec<RunningAction> {
        let mut out = Vec::new();
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(row) = self.rows.get(index) else {
                continue;
            };
            if item.button(cx, ids!(attach)).clicked(actions) {
                out.push(RunningAction::Attach(row.instance.clone()));
            }
            if item.button(cx, ids!(stop)).clicked(actions) {
                out.push(RunningAction::Stop(row.instance.clone()));
            }
            if item.button(cx, ids!(dup)).clicked(actions) {
                out.push(RunningAction::Duplicate(row.instance.clone()));
            }
            if item.button(cx, ids!(copy)).clicked(actions) {
                out.push(RunningAction::CopyId(row.instance.clone()));
            }
        }
        out
    }
}

impl Widget for RunningList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(row) = self.rows.get(index) else {
                    continue;
                };
                let item = list.item(cx, index, id!(Item));
                let attached = self.attached.as_deref() == Some(row.instance.as_str());
                let title = format!(
                    "{}{} · {}",
                    if attached { "▸ " } else { "" },
                    row.flow,
                    row.label
                        .clone()
                        .unwrap_or_else(|| row.instance.chars().take(13).collect())
                );
                item.button(cx, ids!(attach)).set_text(cx, &title);
                let mut detail = format!(
                    "{} · {}",
                    row.owner,
                    row.state,
                );
                if let Some(run) = &row.run {
                    detail.push_str(&format!(" · {}", run.chars().take(13).collect::<String>()));
                }
                if row.state == "running" && self.now_ms > row.last_activity_ms {
                    detail.push_str(&format!(
                        " · {:.1} s",
                        (self.now_ms - row.last_activity_ms) as f64 / 1000.0
                    ));
                }
                if row.subscribers > 0 {
                    detail.push_str(&format!(" · {} sub", row.subscribers));
                }
                if let Some((name, value)) = row.outputs.iter().next() {
                    if let Some(text) = crate::faces::preview_text(value) {
                        let text: String = text.chars().take(32).collect();
                        detail.push_str(&format!("\n{name}: {text}"));
                    }
                }
                item.label(cx, ids!(detail)).set_text(cx, &detail);
                item.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub enum PaletteAction {
    #[default]
    None,
    /// A type was pressed: the canvas places it where the mouse is released.
    Armed(String),
    /// A type was clicked while the palette was filtered from a wire drop:
    /// place it at the drop point and connect.
    Picked(String),
}

#[derive(Clone, Debug)]
enum PaletteRow {
    Kind(String),
    Type(String),
}

#[derive(Script, ScriptHook, Widget)]
pub struct Palette {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<PaletteRow>,
    #[rust]
    filtered: bool,
}

impl Palette {
    pub fn set_types(&mut self, cx: &mut Cx, catalog: &[NodeTypeCatalog], filtered: bool) {
        self.rows.clear();
        self.filtered = filtered;
        // The flow's edge first, then the executors in the order a flow
        // usually reads, then everything the recipes add.
        const ORDER: [&str; 7] = ["input", "output", "chat", "fn", "http", "ask", "gen"];
        let mut kinds: Vec<&str> = catalog.iter().map(|entry| entry.kind.as_str()).collect();
        kinds.sort_by_key(|kind| {
            (
                ORDER.iter().position(|known| known == kind).unwrap_or(ORDER.len()),
                kind.to_string(),
            )
        });
        kinds.dedup();
        for kind in kinds {
            self.rows.push(PaletteRow::Kind(if filtered {
                format!("{kind} · compatible")
            } else {
                kind.to_string()
            }));
            for entry in catalog.iter().filter(|entry| entry.kind == kind) {
                self.rows.push(PaletteRow::Type(entry.type_name.clone()));
            }
        }
        self.redraw(cx);
    }

    pub fn actions(&self, cx: &mut Cx, actions: &Actions) -> Vec<PaletteAction> {
        let mut out = Vec::new();
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(PaletteRow::Type(type_name)) = self.rows.get(index) else {
                continue;
            };
            let button = item.button(cx, ids!(name));
            if self.filtered {
                if button.clicked(actions) {
                    out.push(PaletteAction::Picked(type_name.clone()));
                }
            } else if button.pressed(actions) {
                out.push(PaletteAction::Armed(type_name.clone()));
            }
        }
        out
    }
}

impl Widget for Palette {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, self.rows.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(row) = self.rows.get(index) else {
                    continue;
                };
                match row {
                    PaletteRow::Kind(kind) => {
                        let item = list.item(cx, index, id!(Kind));
                        item.label(cx, ids!(title)).set_text(cx, kind);
                        item.draw_all_unscoped(cx);
                    }
                    PaletteRow::Type(type_name) => {
                        let item = list.item(cx, index, id!(Type));
                        item.button(cx, ids!(name)).set_text(cx, type_name);
                        item.draw_all_unscoped(cx);
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ---------------------------------------------------------------------------
// App view
// ---------------------------------------------------------------------------

/// The flow as a product: its own face full-size, or the input faces above
/// the output faces (a waiting Ask on top). Same instance, same faces.
#[derive(Script, ScriptHook, Widget)]
pub struct AppView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_frame: DrawColor,
    #[live]
    draw_text: DrawText,
    #[rust]
    area: Area,
    #[rust]
    graph: Option<Graph>,
    #[rust]
    pub waiting: Option<String>,
}

impl AppView {
    pub fn set_graph(&mut self, cx: &mut Cx, graph: Option<Graph>) {
        self.graph = graph;
        self.redraw(cx);
    }

    fn draw_node_face(&mut self, cx: &mut Cx2d, scope: &mut Scope, node: &Node) {
        let width = cx.turtle().rect().size.x - 2.0 * self.layout.padding.left;
        self.draw_frame.begin(
            cx,
            Walk {
                abs_pos: None,
                margin: Inset::default(),
                width: Size::Fixed(width.max(200.0)),
                height: Size::fit(),
                metrics: Metrics::default(),
            },
            Layout {
                flow: Flow::Down,
                padding: Inset {
                    left: 8.0,
                    right: 8.0,
                    top: 4.0,
                    bottom: 8.0,
                },
                ..Layout::default()
            },
        );
        let header = cx.walk_turtle(Walk::fixed(width - 16.0, 22.0));
        let title = node
            .doc
            .clone()
            .or_else(|| node.label.clone())
            .unwrap_or_else(|| format!("{} · {}", node.id, node.type_name));
        self.draw_text.draw_abs(cx, header.pos + dvec2(0.0, 4.0), &title);
        if let Some(faces) = scope.data.get_mut::<FaceHost>() {
            faces.draw_face(cx, &node.id, Walk::fill_fit());
        } else {
            let text = param_text(node, "default");
            let rect = cx.walk_turtle(Walk::fixed(width - 16.0, 20.0));
            self.draw_text.draw_abs(cx, rect.pos, &text);
        }
        self.draw_frame.end(cx);
    }
}

impl Widget for AppView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let Some(graph) = self.graph.clone() else {
            self.draw_bg.end(cx);
            return DrawStep::done();
        };
        let has_flow_face = scope
            .data
            .get::<FaceHost>()
            .is_some_and(|faces| faces.flow_face.as_ref().is_some_and(|face| !face.root.is_empty()));
        if has_flow_face {
            if let Some(faces) = scope.data.get_mut::<FaceHost>() {
                faces.draw_flow_face(cx, Walk::fill());
            }
        } else {
            if let Some(waiting) = self.waiting.clone() {
                if let Some(node) = graph.nodes.iter().find(|node| node.id == waiting).cloned() {
                    self.draw_node_face(cx, scope, &node);
                }
            }
            for node in graph.nodes.iter().filter(|node| node.kind == "input").cloned() {
                self.draw_node_face(cx, scope, &node);
            }
            for node in graph.nodes.iter().filter(|node| node.kind == "output").cloned() {
                self.draw_node_face(cx, scope, &node);
            }
        }
        self.draw_bg.end(cx);
        self.area = self.draw_bg.draw_vars.area;
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}
