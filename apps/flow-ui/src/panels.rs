//! The panels around the canvas (DESIGN.md §8): the flow list, the Running
//! list, the palette of prelude types, the inspector for the selected node,
//! the template picker behind New, the run's total progress bar, and the
//! App view that shows a flow as a product.

use crate::faces::{
    format_options_for_node, format_preset_name, node_dimensions, param_text, FaceHost,
    FormatOptions, ModelChoice, CUSTOM_FORMAT, HUB_PICKS,
};
use makepad_flow::{
    FlowSummary, Graph, InstanceRow, Literal, Node, NodeInputValue, NodeTypeCatalog,
    TemplateSummary, ValueBytes, ValueRef,
};
use makepad_widgets::fab_controls::*;
use makepad_widgets::makepad_micro_serde::SerJson;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let RowLabel = Label{
        width: 84
        height: Fit
        draw_text +: {
            color: theme.flow_text_muted
            text_style: theme.font_regular{font_size: 9}
        }
    }

    let Card = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        padding: Inset{left: 10 right: 10 top: 8 bottom: 8}
        spacing: theme.space_1
        show_bg: true
        draw_bg +: {
            color: theme.flow_surface
            border_radius: 10.0
            border_size: 1.0
            border_color: theme.flow_surface_raised
        }
    }

    let Dot = RoundedView{
        width: 8
        height: 8
        draw_bg +: {
            border_radius: 4.0
            color: theme.flow_success
        }
    }

    let TitleButton = ButtonFlatter{
        width: Fill
        height: Fit
        padding: Inset{left: 0 right: 0 top: 2 bottom: 2}
        draw_text +: {
            text_style: theme.font_bold{font_size: 9.5}
            color: theme.flow_text
        }
    }

    let MetaText = Label{
        width: Fill
        height: Fit
        text: ""
        draw_text +: {
            color: theme.flow_text_muted
            text_style: theme.font_regular{font_size: 8.5}
        }
    }

    let EmptyHint = Label{
        width: Fill
        height: Fit
        margin: Inset{top: 8}
        text: ""
        draw_text +: {
            color: theme.flow_text_hint
            text_style: theme.font_regular{font_size: 9}
        }
    }

    // -- flows ------------------------------------------------------------------

    mod.widgets.FlowListBase = #(FlowList::register_widget(vm))
    mod.widgets.FlowList = set_type_default() do mod.widgets.FlowListBase{
        width: Fill
        height: Fill
        flow: Down
        hint := EmptyHint{text: "No flows yet — New starts one from a template."}
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Item := View{
                width: Fill
                height: Fit
                padding: Inset{bottom: 4}
                card := Card{
                    flow: Right
                    align: Align{y: 0.5}
                    spacing: theme.space_2
                    dot := Dot{}
                    select := TitleButton{}
                    count := Label{
                        width: Fit
                        height: Fit
                        text: ""
                        draw_text +: {
                            color: theme.flow_text_muted
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                }
            }
        }
    }

    // -- running ----------------------------------------------------------------

    mod.widgets.RunningListBase = #(RunningList::register_widget(vm))
    mod.widgets.RunningList = set_type_default() do mod.widgets.RunningListBase{
        width: Fill
        height: Fill
        flow: Down
        hint := EmptyHint{text: "Nothing is running. Run starts an instance of the open flow."}
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Item := View{
                width: Fill
                height: Fit
                padding: Inset{bottom: 4}
                card := Card{
                    head := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        align: Align{y: 0.5}
                        spacing: theme.space_2
                        dot := Dot{}
                        attach := TitleButton{}
                    }
                    detail := MetaText{}
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
    }

    // -- palette ----------------------------------------------------------------

    let Badge = RoundedView{
        width: 30
        height: 30
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            border_radius: 8.0
            color: theme.flow_surface_raised
        }
        icon := Icon{
            icon_walk: Walk{width: 16 height: Fit}
            draw_icon +: {
                color: theme.flow_text_white
            }
        }
    }

    let PaletteCard = View{
        width: Fill
        height: Fit
        padding: Inset{bottom: 6}
        card := RoundedView{
            width: Fill
            height: Fit
            flow: Right
            align: Align{y: 0.5}
            spacing: theme.space_2
            padding: Inset{left: 8 right: 6 top: 6 bottom: 6}
            cursor: MouseCursor.Hand
            show_bg: true
            draw_bg +: {
                color: theme.flow_surface
                border_radius: 10.0
                border_size: 1.0
                border_color: theme.flow_surface_raised
            }
            badge := Badge{}
            View{
                width: Fill
                height: Fit
                flow: Down
                spacing: 1
                name := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {
                        text_style: theme.font_bold{font_size: 9.5}
                        color: theme.flow_text
                    }
                }
                doc := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {
                        color: theme.flow_text_muted
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
            }
            grip := Icon{
                icon_walk: Walk{width: 12 height: Fit}
                draw_icon +: {
                    color: theme.flow_text_grip
                    svg: crate_resource("self:resources/icons/grip.svg")
                }
            }
        }
    }

    mod.widgets.PaletteBase = #(Palette::register_widget(vm))
    mod.widgets.Palette = set_type_default() do mod.widgets.PaletteBase{
        width: Fill
        height: Fill
        flow: Down
        hint := EmptyHint{text: "Waiting for the server's node catalog…"}
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
                padding: Inset{left: 2 top: 8 bottom: 4}
                title := Label{
                    text: ""
                    draw_text +: {
                        color: theme.flow_text_subtle
                        text_style: theme.font_bold{font_size: 8.5}
                    }
                }
            }
            CardInput := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_input} icon +: { draw_icon +: { color: theme.flow_input svg: crate_resource("self:resources/icons/input.svg") } } } }
            }
            CardOutput := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_output} icon +: { draw_icon +: { color: theme.flow_success svg: crate_resource("self:resources/icons/output.svg") } } } }
            }
            CardChat := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_chat} icon +: { draw_icon +: { color: theme.flow_chat svg: crate_resource("self:resources/icons/chat.svg") } } } }
            }
            CardGen := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_generation} icon +: { draw_icon +: { color: theme.flow_generation svg: crate_resource("self:resources/icons/gen.svg") } } } }
            }
            CardFn := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_waiting} icon +: { draw_icon +: { color: theme.flow_function svg: crate_resource("self:resources/icons/fn.svg") } } } }
            }
            CardHttp := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_http} icon +: { draw_icon +: { color: theme.flow_http svg: crate_resource("self:resources/icons/http.svg") } } } }
            }
            CardAsk := PaletteCard{
                card +: { badge +: { draw_bg +: {color: theme.flow_badge_waiting} icon +: { draw_icon +: { color: theme.flow_waiting svg: crate_resource("self:resources/icons/ask.svg") } } } }
            }
        }
    }

    // -- inspector ----------------------------------------------------------------

    let Row = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        padding: Inset{left: 2 right: 2 top: 3 bottom: 3}
        align: Align{y: 0.5}
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
                padding: Inset{left: 2 right: 2 top: 4 bottom: 6}
                spacing: 2
                title := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {
                        text_style: theme.font_bold{font_size: 11}
                        color: theme.flow_text
                    }
                }
                doc := MetaText{}
            }
            Section := View{
                width: Fill
                height: Fit
                flow: Down
                padding: Inset{left: 2 right: 2 top: 10 bottom: 2}
                spacing: 4
                title := Label{
                    text: ""
                    draw_text +: {
                        color: theme.flow_text_subtle
                        text_style: theme.font_bold{font_size: 8.5}
                    }
                }
                Hr{}
            }
            Text := Row{
                name := RowLabel{}
                value := TextInput{
                    width: Fill
                    height: 26
                    empty_text: ""
                }
            }
            Multiline := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_1
                padding: Inset{left: 2 right: 2 top: 3 bottom: 3}
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
                    empty_text: ""
                    draw_text +: {text_style: theme.font_code{font_size: 9}}
                }
            }
            Number := Row{
                name := RowLabel{}
                value := mod.widgets.FabValueInput{
                    width: Fill
                    height: 24
                    quantize: true
                }
            }
            Dimensions := Row{
                spacing: theme.space_1
                w_field := mod.widgets.FabValueInput{
                    width: 54
                    height: 24
                    label: "w"
                    precision: 0
                    quantize: true
                }
                h_field := mod.widgets.FabValueInput{
                    width: 54
                    height: 24
                    label: "h"
                    precision: 0
                    quantize: true
                }
                format := DropDown{
                    width: Fill
                    height: 26
                    labels: ["Custom"]
                }
                swap := ButtonFlatter{
                    width: 26
                    height: 26
                    text: "⇄"
                }
            }
            Choice := Row{
                name := RowLabel{}
                value := DropDown{
                    width: Fill
                    height: 26
                }
            }
            Model := Row{
                name := RowLabel{}
                value := DropDown{
                    width: Fill
                    height: 26
                    labels: ["hub picks"]
                }
            }
            Bool := Row{
                name := RowLabel{}
                value := Toggle{
                    text: ""
                }
            }
            Color := Row{
                name := RowLabel{}
                value := mod.widgets.FabColorPick{
                    width: 60
                    height: 20
                }
            }
            Wired := Row{
                name := RowLabel{}
                value := MetaText{}
            }
            Output := Row{
                name := RowLabel{}
                value := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {
                        text_style: theme.font_code{font_size: 8.5}
                        color: theme.flow_text_code
                    }
                }
                open := ButtonFlat{text: "Open"}
            }
            Preview := View{
                width: Fill
                height: Fit
                flow: Down
                padding: Inset{left: 2 right: 2 top: 4 bottom: 4}
                text := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {text_style: theme.font_code{font_size: 8.5}}
                }
                image := Image{
                    width: Fill
                    height: Fit
                    fit: ImageFit.Horizontal
                }
            }
            Empty := View{
                width: Fill
                height: Fit
                flow: Down
                padding: Inset{left: 2 right: 2 top: 4 bottom: 4}
                spacing: 4
                title := Label{
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text +: {
                        text_style: theme.font_bold{font_size: 10}
                        color: theme.flow_text_body
                    }
                }
                doc := EmptyHint{margin: Inset{top: 0}}
            }
        }
    }

    // -- templates ----------------------------------------------------------------

    mod.widgets.TemplatePickerBase = #(TemplatePicker::register_widget(vm))
    mod.widgets.TemplatePicker = set_type_default() do mod.widgets.TemplatePickerBase{
        width: Fit
        height: Fit
        panel := RoundedView{
            width: 420
            height: 520
            flow: Down
            spacing: theme.space_2
            padding: Inset{left: 14 right: 14 top: 12 bottom: 12}
            show_bg: true
            draw_bg +: {
                color: theme.flow_surface
                border_radius: 14.0
                border_size: 1.0
                border_color: theme.flow_edge_soft
            }
            head := View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{y: 0.5}
                title := Label{
                    width: Fill
                    height: Fit
                    text: "New flow from a template"
                    draw_text +: {
                        text_style: theme.font_bold{font_size: 11}
                        color: theme.flow_text
                    }
                }
                close := ButtonFlat{text: "Close"}
            }
            MetaText{text: "The pipelines the fleet runs today. Pick one; it opens on the canvas with an instance bound."}
            hint := EmptyHint{text: "Fetching the template list…"}
            list := PortalList{
                width: Fill
                height: Fill
                scroll_bar: ScrollBar{}
                Item := View{
                    width: Fill
                    height: Fit
                    padding: Inset{bottom: 6}
                    card := Card{
                        flow: Right
                        align: Align{y: 0.5}
                        spacing: theme.space_2
                        View{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 2
                            label := Label{
                                width: Fill
                                height: Fit
                                text: ""
                                draw_text +: {
                                    text_style: theme.font_bold{font_size: 10}
                                    color: theme.flow_text
                                }
                            }
                            brief := MetaText{}
                            io := MetaText{}
                        }
                        create := Button{text: "Create"}
                    }
                }
            }
        }
    }

    // -- the run's total progress -----------------------------------------------------

    mod.widgets.RunBarBase = #(RunBar::register_widget(vm))
    mod.widgets.RunBar = set_type_default() do mod.widgets.RunBarBase{
        width: 200
        height: 6
    }

    // -- app view -----------------------------------------------------------------------

    mod.widgets.AppViewBase = #(AppView::register_widget(vm))
    mod.widgets.AppView = set_type_default() do mod.widgets.AppViewBase{
        width: Fill
        height: Fill
        flow: Down
        padding: theme.mspace_3
        spacing: theme.space_2
        draw_bg +: {color: theme.flow_grid_a}
        draw_frame +: {color: theme.flow_surface}
        draw_text +: {
            text_style: theme.font_bold{font_size: 10}
            color: theme.flow_text
        }
    }
}

fn set_dot(cx: &mut Cx, item: &WidgetRef, color: Vec4f) {
    let mut dot = item.view(cx, ids!(dot));
    script_apply_eval!(cx, dot, {draw_bg +: {color: #(color)}});
}

// ---------------------------------------------------------------------------
// Flows
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook, Widget)]
pub struct FlowList {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<FlowSummary>,
    #[rust]
    selected: Option<String>,
}

impl FlowList {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<FlowSummary>, selected: Option<String>) {
        self.rows = rows;
        self.selected = selected;
        self.view
            .label(cx, ids!(hint))
            .set_visible(cx, self.rows.is_empty());
        self.redraw(cx);
    }

    pub fn selected(&self, cx: &mut Cx, actions: &Actions) -> Option<usize> {
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(select)).clicked(actions) {
                return Some(index);
            }
        }
        None
    }
}

impl Widget for FlowList {
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
                let open = self.selected.as_deref() == Some(row.name.as_str());
                let title = if open {
                    format!("› {}", row.name)
                } else {
                    row.name.clone()
                };
                item.button(cx, ids!(select)).set_text(cx, &title);
                let count = if row.instances > 0 {
                    format!("{} live", row.instances)
                } else {
                    String::new()
                };
                item.label(cx, ids!(count)).set_text(cx, &count);
                set_dot(cx, &item, crate::theme::state_color(&row.state));
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
    SetParams {
        node: String,
        values: Vec<(String, Literal)>,
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
    },
}

#[derive(Clone, Debug)]
enum Row {
    Head {
        title: String,
        doc: String,
    },
    Empty {
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
    Number {
        key: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
    },
    Dimensions {
        width: u32,
        height: u32,
        options: FormatOptions,
    },
    Choice {
        key: String,
        value: String,
        options: Vec<String>,
    },
    Model {
        value: String,
    },
    Bool {
        key: String,
        value: bool,
    },
    Color {
        key: String,
        rgba: [f32; 4],
    },
    Wired {
        key: String,
        value: String,
    },
    Output {
        port: String,
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
            Row::Empty { .. } => live_id!(Empty),
            Row::Section(_) => live_id!(Section),
            Row::Text { .. } => live_id!(Text),
            Row::Multiline { .. } => live_id!(Multiline),
            Row::Number { .. } => live_id!(Number),
            Row::Dimensions { .. } => live_id!(Dimensions),
            Row::Choice { .. } => live_id!(Choice),
            Row::Model { .. } => live_id!(Model),
            Row::Bool { .. } => live_id!(Bool),
            Row::Color { .. } => live_id!(Color),
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

/// `#rrggbb` / `#rrggbbaa` → rgba 0..1.
fn parse_hex_color(text: &str) -> Option<[f32; 4]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 && hex.len() != 8 {
        return None;
    }
    let channel = |index: usize| -> Option<f32> {
        let byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
        Some(byte as f32 / 255.0)
    };
    Some([
        channel(0)?,
        channel(1)?,
        channel(2)?,
        if hex.len() == 8 { channel(3)? } else { 1.0 },
    ])
}

fn hex_color(rgba: Vec4f) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if rgba.w >= 0.999 {
        format!("#{:02x}{:02x}{:02x}", byte(rgba.x), byte(rgba.y), byte(rgba.z))
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            byte(rgba.x),
            byte(rgba.y),
            byte(rgba.z),
            byte(rgba.w)
        )
    }
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
    /// The hub's models for the shown node's domain (`Model` rows).
    #[rust]
    models: Vec<ModelChoice>,
    #[rust]
    dimensions_dirty: bool,
    #[rust]
    dimensions_signature: Option<(String, u32, u32, FormatOptions)>,
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
            self.dimensions_dirty = false;
            self.dimensions_signature = None;
            self.rows.push(Row::Empty {
                title: "Nothing selected".into(),
                doc: "Click a node on the canvas to edit its params; drag a palette card onto the canvas to add one.".into(),
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
        let dimensions = format_options_for_node(node, catalog)
            .and_then(|options| node_dimensions(node).map(|size| (size, options)));
        let dimensions_signature = dimensions.as_ref().map(|((width, height), options)| {
            (node.id.clone(), *width, *height, options.clone())
        });
        self.dimensions_dirty = self.dimensions_signature.as_ref() != dimensions_signature.as_ref();
        self.dimensions_signature = dimensions_signature;
        let mut dimensions_added = false;
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
            if matches!(key.as_str(), "width" | "height") {
                if let Some(((width, height), options)) = dimensions.as_ref() {
                    if !dimensions_added {
                        self.rows.push(Row::Dimensions {
                            width: *width,
                            height: *height,
                            options: options.clone(),
                        });
                        dimensions_added = true;
                    }
                    continue;
                }
            }
            let doc = entry
                .and_then(|entry| entry.params.iter().find(|param| &param.name == key))
                .map(|param| param.doc.clone())
                .unwrap_or_default();
            if key == "model" && node.domain.is_some() {
                self.rows.push(Row::Model {
                    value: literal_text(value),
                });
                continue;
            }
            let range = entry
                .and_then(|entry| entry.params.iter().find(|param| &param.name == key))
                .and_then(|param| param.range.as_ref())
                .map(|range| (range.min, range.max, range.step.unwrap_or(1.0)))
                .or_else(|| parse_range(&doc));
            if let Literal::Num(number) = value {
                let (min, max, step) = range.unwrap_or_else(|| {
                    let magnitude = number.abs().max(1.0);
                    (0.0, (magnitude * 4.0).max(10.0), if number.fract() == 0.0 { 1.0 } else { 0.01 })
                });
                self.rows.push(Row::Number {
                    key: key.clone(),
                    value: *number,
                    min,
                    max,
                    step,
                });
                continue;
            }
            if let Literal::Bool(flag) = value {
                self.rows.push(Row::Bool {
                    key: key.clone(),
                    value: *flag,
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
            if let Literal::Str(text) = value {
                if let Some(rgba) = parse_hex_color(text) {
                    if key.contains("color") || key.contains("colour") || key.contains("tint") {
                        self.rows.push(Row::Color {
                            key: key.clone(),
                            rgba,
                        });
                        continue;
                    }
                }
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
                NodeInputValue::Literal(Literal::Num(number)) => self.rows.push(Row::Number {
                    key: input.port.clone(),
                    value: *number,
                    min: 0.0,
                    max: (number.abs() * 4.0).max(10.0),
                    step: if number.fract() == 0.0 { 1.0 } else { 0.01 },
                }),
                NodeInputValue::Literal(value) => self.rows.push(Row::Text {
                    key: input.port.clone(),
                    value: literal_text(value),
                    id: matches!(value, Literal::Id(_)),
                    number: false,
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
                    .unwrap_or_else(|| format!("{} · {}", value.content_type, crate::faces::size_text(value.bytes)));
                let chip: String = chip.chars().take(80).collect();
                self.rows.push(Row::Output {
                    port: port.clone(),
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

    /// The hub's models for the shown node's domain.
    pub fn set_models(&mut self, cx: &mut Cx, models: Vec<ModelChoice>) {
        if self.models != models {
            self.models = models;
            self.redraw(cx);
        }
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
                Row::Number { key, .. } => {
                    if let Some(value) = item.fab_value_input(cx, ids!(value)).ended(actions) {
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: key.clone(),
                            value: Literal::Num(value),
                        });
                    }
                }
                Row::Dimensions { options, .. } => {
                    let width_field = item.fab_value_input(cx, ids!(w_field));
                    let height_field = item.fab_value_input(cx, ids!(h_field));
                    let picker = item.drop_down(cx, ids!(format));
                    if let Some(index) = picker.changed(actions) {
                        if let Some(preset) = index
                            .checked_sub(1)
                            .and_then(|index| options.presets.get(index))
                        {
                            width_field.set_value(cx, preset.width as f64);
                            height_field.set_value(cx, preset.height as f64);
                            out.push(InspectorAction::SetParams {
                                node: node.clone(),
                                values: vec![
                                    ("width".into(), Literal::Num(preset.width as f64)),
                                    ("height".into(), Literal::Num(preset.height as f64)),
                                ],
                            });
                        }
                        continue;
                    }
                    if item.button(cx, ids!(swap)).clicked(actions) {
                        let width = width_field.value().round().max(0.0) as u32;
                        let height = height_field.value().round().max(0.0) as u32;
                        width_field.set_value(cx, height as f64);
                        height_field.set_value(cx, width as f64);
                        picker.set_selected_by_label(
                            format_preset_name(&options.presets, height, width),
                            cx,
                        );
                        out.push(InspectorAction::SetParams {
                            node: node.clone(),
                            values: vec![
                                ("width".into(), Literal::Num(height as f64)),
                                ("height".into(), Literal::Num(width as f64)),
                            ],
                        });
                        continue;
                    }
                    let width_ended = width_field.ended(actions);
                    let height_ended = height_field.ended(actions);
                    if width_ended.is_some() || height_ended.is_some() {
                        let width = width_field.value().round().max(0.0) as u32;
                        let height = height_field.value().round().max(0.0) as u32;
                        picker.set_selected_by_label(
                            format_preset_name(&options.presets, width, height),
                            cx,
                        );
                        match (width_ended, height_ended) {
                            (Some(width), Some(height)) => {
                                out.push(InspectorAction::SetParams {
                                    node: node.clone(),
                                    values: vec![
                                        ("width".into(), Literal::Num(width)),
                                        ("height".into(), Literal::Num(height)),
                                    ],
                                });
                            }
                            (Some(value), None) => out.push(InspectorAction::SetParam {
                                node: node.clone(),
                                key: "width".into(),
                                value: Literal::Num(value),
                            }),
                            (None, Some(value)) => out.push(InspectorAction::SetParam {
                                node: node.clone(),
                                key: "height".into(),
                                value: Literal::Num(value),
                            }),
                            (None, None) => {}
                        }
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
                Row::Model { value: current } => {
                    if let Some(index) = item.drop_down(cx, ids!(value)).changed(actions) {
                        let value = index
                            .checked_sub(1)
                            .and_then(|index| self.models.get(index))
                            .map(|model| model.id.clone())
                            .unwrap_or_else(|| {
                                if index == 0 {
                                    String::new()
                                } else {
                                    current.clone()
                                }
                            });
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: "model".into(),
                            value: Literal::Str(value),
                        });
                    }
                }
                Row::Bool { key, .. } => {
                    if let Some(flag) = item.check_box(cx, ids!(value)).changed(actions) {
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: key.clone(),
                            value: Literal::Bool(flag),
                        });
                    }
                }
                Row::Color { key, .. } => {
                    if let Some(rgba) = item.fab_color_pick(cx, ids!(value)).changed(actions) {
                        out.push(InspectorAction::SetParam {
                            node: node.clone(),
                            key: key.clone(),
                            value: Literal::Str(hex_color(rgba)),
                        });
                    }
                }
                Row::Output { port, .. } => {
                    if item.button(cx, ids!(open)).clicked(actions) {
                        out.push(InspectorAction::OpenValue {
                            node: node.clone(),
                            port: port.clone(),
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
                    Row::Head { title, doc } | Row::Empty { title, doc } => {
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
                    Row::Number {
                        key,
                        value,
                        min,
                        max,
                        step,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        let mut field = item.fab_value_input(cx, ids!(value));
                        if !existed {
                            let integral = step.fract() == 0.0;
                            let scrub = (*step * 0.25).max(if integral { 0.25 } else { *step });
                            script_apply_eval!(cx, field, {
                                min: #(*min)
                                max: #(*max)
                                step: #(scrub)
                                snap: #(*step)
                                precision: #(if integral { 0usize } else { 2usize })
                            });
                            field.set_value(cx, *value);
                        }
                    }
                    Row::Dimensions {
                        width,
                        height,
                        options,
                    } => {
                        let width_field = item.fab_value_input(cx, ids!(w_field));
                        let height_field = item.fab_value_input(cx, ids!(h_field));
                        let picker = item.drop_down(cx, ids!(format));
                        if !existed || self.dimensions_dirty {
                            let (width_min, width_max, width_step) = options.width_range;
                            let (height_min, height_max, height_step) = options.height_range;
                            let mut width_field_inner = width_field.clone();
                            script_apply_eval!(cx, width_field_inner, {
                                min: #(width_min)
                                max: #(width_max)
                                step: #((width_step * 0.125).max(1.0))
                                snap: #(width_step)
                            });
                            let mut height_field_inner = height_field.clone();
                            script_apply_eval!(cx, height_field_inner, {
                                min: #(height_min)
                                max: #(height_max)
                                step: #((height_step * 0.125).max(1.0))
                                snap: #(height_step)
                            });
                            width_field.set_value(cx, *width as f64);
                            height_field.set_value(cx, *height as f64);
                            let mut labels = vec![CUSTOM_FORMAT.to_string()];
                            labels.extend(
                                options.presets.iter().map(|preset| preset.name.clone()),
                            );
                            picker.set_labels(cx, labels);
                            picker.set_selected_by_label(
                                format_preset_name(&options.presets, *width, *height),
                                cx,
                            );
                            self.dimensions_dirty = false;
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
                    Row::Model { value } => {
                        item.label(cx, ids!(name)).set_text(cx, "model");
                        let drop_down = item.drop_down(cx, ids!(value));
                        let mut labels = vec![HUB_PICKS.to_string()];
                        labels.extend(self.models.iter().map(|model| model.label.clone()));
                        let selected = self
                            .models
                            .iter()
                            .find(|model| model.id == *value)
                            .map(|model| model.label.clone())
                            .unwrap_or_else(|| value.clone());
                        if !selected.is_empty() && !labels.iter().any(|label| label == &selected) {
                            labels.push(value.clone());
                        }
                        drop_down.set_labels(cx, labels);
                        let mut dimmed = vec![false];
                        dimmed.extend(self.models.iter().map(|model| model.dimmed));
                        drop_down.set_dimmed_items(cx, dimmed);
                        drop_down.set_selected_by_label(
                            if value.is_empty() { HUB_PICKS } else { &selected },
                            cx,
                        );
                    }
                    Row::Bool { key, value } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        if !existed {
                            item.check_box(cx, ids!(value)).set_active(cx, *value, Animate::No);
                        }
                    }
                    Row::Color { key, rgba } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        if !existed {
                            item.fab_color_pick(cx, ids!(value)).set_rgba(cx, *rgba);
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
        self.view
            .label(cx, ids!(hint))
            .set_visible(cx, self.rows.is_empty());
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
                    if attached { "› " } else { "" },
                    row.flow,
                    row.label
                        .clone()
                        .unwrap_or_else(|| row.instance.chars().take(8).collect())
                );
                item.button(cx, ids!(attach)).set_text(cx, &title);
                set_dot(cx, &item, crate::theme::state_color(&row.state));
                let mut detail = format!("{} · {}", row.owner, row.state);
                if row.state == "running" && self.now_ms > row.last_activity_ms {
                    detail.push_str(&format!(
                        " · {:.0} s ago",
                        (self.now_ms - row.last_activity_ms) as f64 / 1000.0
                    ));
                }
                if row.subscribers > 0 {
                    detail.push_str(&format!(" · {} watching", row.subscribers));
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
    /// A card was pressed: the canvas places the type where the mouse is released.
    Armed(String),
    /// A card was clicked while the palette was filtered from a wire drop:
    /// place it at the drop point and connect.
    Picked(String),
}

#[derive(Clone, Debug)]
enum PaletteRow {
    Kind(String),
    Type {
        name: String,
        kind: String,
        doc: String,
    },
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

fn kind_template(kind: &str) -> LiveId {
    match kind {
        "input" => live_id!(CardInput),
        "output" => live_id!(CardOutput),
        "chat" => live_id!(CardChat),
        "fn" => live_id!(CardFn),
        "http" => live_id!(CardHttp),
        "ask" => live_id!(CardAsk),
        _ => live_id!(CardGen),
    }
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
            let title = match kind {
                "input" => "INPUTS",
                "output" => "OUTPUTS",
                "chat" => "LANGUAGE MODELS",
                "fn" => "FUNCTIONS",
                "http" => "HTTP",
                "ask" => "ASK THE USER",
                "gen" => "GENERATORS",
                other => other,
            };
            self.rows.push(PaletteRow::Kind(if filtered {
                format!("{title} · COMPATIBLE")
            } else {
                title.to_string()
            }));
            for entry in catalog.iter().filter(|entry| entry.kind == kind) {
                let doc = entry
                    .doc
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let doc = if doc.is_empty() {
                    entry
                        .domain
                        .as_ref()
                        .map(|domain| format!("{domain} domain"))
                        .unwrap_or_default()
                } else {
                    doc
                };
                self.rows.push(PaletteRow::Type {
                    name: entry.type_name.clone(),
                    kind: entry.kind.clone(),
                    doc,
                });
            }
        }
        self.view
            .label(cx, ids!(hint))
            .set_visible(cx, self.rows.is_empty());
        self.redraw(cx);
    }

    pub fn actions(&self, cx: &mut Cx, actions: &Actions) -> Vec<PaletteAction> {
        let mut out = Vec::new();
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(PaletteRow::Type { name, .. }) = self.rows.get(index) else {
                continue;
            };
            let card = item.view(cx, ids!(card));
            if self.filtered {
                if card.finger_up(actions).is_some_and(|up| up.is_over) {
                    out.push(PaletteAction::Picked(name.clone()));
                }
            } else if card.finger_down(actions).is_some() {
                out.push(PaletteAction::Armed(name.clone()));
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
                    PaletteRow::Type { name, kind, doc } => {
                        let item = list.item(cx, index, kind_template(kind));
                        item.label(cx, ids!(name)).set_text(cx, name);
                        item.label(cx, ids!(doc)).set_text(cx, doc);
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
// Templates
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook, Widget)]
pub struct TemplatePicker {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<TemplateSummary>,
}

impl TemplatePicker {
    pub fn set_templates(&mut self, cx: &mut Cx, rows: Vec<TemplateSummary>) {
        self.rows = rows;
        let hint = self.view.label(cx, ids!(hint));
        if self.rows.is_empty() {
            hint.set_text(cx, "The server lists no templates.");
        }
        hint.set_visible(cx, self.rows.is_empty());
        self.redraw(cx);
    }

    /// The template whose Create was clicked.
    pub fn picked(&self, cx: &mut Cx, actions: &Actions) -> Option<String> {
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(create)).clicked(actions) {
                return self.rows.get(index).map(|row| row.name.clone());
            }
        }
        None
    }

    pub fn closed(&self, cx: &mut Cx, actions: &Actions) -> bool {
        self.view.button(cx, ids!(close)).clicked(actions)
    }
}

impl Widget for TemplatePicker {
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
                item.label(cx, ids!(label)).set_text(cx, &row.label);
                item.label(cx, ids!(brief)).set_text(cx, &row.brief);
                let inputs: Vec<String> = row
                    .inputs
                    .iter()
                    .map(|(name, ty)| format!("{name}: {ty}"))
                    .collect();
                let outputs: Vec<String> = row
                    .outputs
                    .iter()
                    .map(|(name, ty)| format!("{name}: {ty}"))
                    .collect();
                item.label(cx, ids!(io)).set_text(
                    cx,
                    &format!(
                        "{} nodes · in {} · out {}",
                        row.node_count,
                        inputs.join(", "),
                        outputs.join(", ")
                    ),
                );
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
// The run's total progress
// ---------------------------------------------------------------------------

/// A thin luminous strip: the run's total progress, eased, in the state's colour.
#[derive(Script, ScriptHook, Widget)]
pub struct RunBar {
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
    draw_bar: DrawVector,
    #[rust]
    area: Area,
    #[rust]
    fraction: f64,
    #[rust]
    shown: f64,
    #[rust]
    state: String,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    last_time: f64,
}

impl RunBar {
    pub fn set_progress(&mut self, cx: &mut Cx, fraction: f64, state: &str) {
        self.fraction = fraction.clamp(0.0, 1.0);
        if self.state != state {
            self.state = state.to_string();
            if state.is_empty() {
                self.shown = 0.0;
            }
        }
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }
}

impl Widget for RunBar {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, rect);
        let (x, y, w, h) = (rect.pos.x as f32, rect.pos.y as f32, rect.size.x as f32, rect.size.y as f32);
        self.draw_bar.begin();
        self.draw_bar.set_color(1.0, 1.0, 1.0, 0.08);
        self.draw_bar.rounded_rect(x, y, w, h, h * 0.5);
        self.draw_bar.fill();
        let color = match self.state.as_str() {
            "" => None,
            "done" => Some((0.30, 0.77, 0.42)),
            "failed" => Some((0.95, 0.43, 0.43)),
            "cancelled" => Some((0.55, 0.55, 0.58)),
            _ => Some((0.35, 0.62, 1.0)),
        };
        if let Some((r, g, b)) = color {
            let indeterminate = matches!(self.state.as_str(), "running" | "queued") && self.fraction <= 0.0;
            if indeterminate {
                let seg = w * 0.3;
                let t = (self.time * 0.8).fract() as f32;
                let sx = x + (w + seg) * t - seg;
                let x0 = sx.max(x);
                let x1 = (sx + seg).min(x + w);
                if x1 > x0 {
                    self.draw_bar.set_color(r, g, b, 0.95);
                    self.draw_bar.rounded_rect(x0, y, x1 - x0, h, h * 0.5);
                    self.draw_bar.fill();
                }
            } else if self.shown > 0.0 {
                let fw = (w * self.shown as f32).max(h);
                // A soft glow under the strip.
                self.draw_bar.set_color(r, g, b, 0.35);
                self.draw_bar
                    .shadow(x, y, fw, h, h * 0.5, 4.0, 0.0, 0.0);
                self.draw_bar.set_color(r, g, b, 1.0);
                self.draw_bar.rounded_rect(x, y, fw, h, h * 0.5);
                self.draw_bar.fill();
            }
        }
        self.draw_bar.end(cx);
        if !self.state.is_empty() {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(nf) = self.next_frame.is_event(event) {
            let dt = (nf.time - self.last_time).clamp(0.0, 0.1);
            self.last_time = nf.time;
            self.time = nf.time;
            let k = 1.0 - (-dt * 10.0).exp();
            self.shown += (self.fraction - self.shown) * k;
            if (self.shown - self.fraction).abs() < 1e-3 {
                self.shown = self.fraction;
            }
            self.area.redraw(cx);
        }
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
                    left: 12.0,
                    right: 12.0,
                    top: 6.0,
                    bottom: 12.0,
                },
                ..Layout::default()
            },
        );
        let header = cx.walk_turtle(Walk::fixed(width - 24.0, 22.0));
        let title = node
            .doc
            .clone()
            .or_else(|| node.label.clone())
            .unwrap_or_else(|| format!("{} · {}", node.id, node.type_name));
        self.draw_text.draw_abs(cx, header.pos + dvec2(0.0, 4.0), &title);
        if let Some(faces) = scope.data.get_mut::<FaceHost>() {
            faces.draw_face(cx, &node.id, Walk::fill_fit(), false);
        } else {
            let text = param_text(node, "default");
            let rect = cx.walk_turtle(Walk::fixed(width - 24.0, 20.0));
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
