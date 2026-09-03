//! The panels around the canvas (DESIGN.md §8): the flow list, the Running
//! list, the palette of prelude types, the inspector for the selected node,
//! the template picker behind New, the run's total progress bar, and the
//! App view that shows a flow as a product.

use crate::faces::{format_preset_name, param_text, snap_stepped_value, FaceHost, ModelChoice};
use makepad_flow::{
    FlowSummary, Graph, InstanceRow, Literal, Node, NodeInputValue, NodeTypeCatalog,
    TemplateSummary, ValueBytes, ValueRef,
};
use makepad_widgets::fab_controls::*;
use makepad_widgets::makepad_micro_serde::{JsonValue, SerJson};
use makepad_widgets::*;
use std::collections::HashMap;

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
                    thumbnail := View{
                        width: Fill height: 72 flow: Overlay cursor: MouseCursor.Hand visible: false
                        image := Image{width: Fill height: Fill fit: ImageFit.Smallest}
                        marker := Label{visible: false}
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

    let OutputScroll = ScrollYView{
        width: Fill
        height: Fit{max: FitBound.Abs(160)}
        scroll_bars +: {
            scroll_bar_y +: {
                bar_size: 8
                bar_side_margin: 1
                draw_bg +: {
                    color: #xffffff18
                    color_hover: #xffffff30
                    color_drag: #xffffff48
                }
            }
        }
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
                padding: Inset{left: 2 right: 2 top: 4 bottom: 8}
                spacing: theme.space_1
                top := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    align: Align{y: 0.5}
                    spacing: theme.space_1
                    kind_icon := Image{width: 16 height: 16 fit: ImageFit.Smallest}
                    node_id := TextInput{
                        width: Fill
                        height: 27
                        empty_text: "node id"
                        draw_text +: {text_style: theme.font_bold{font_size: 10.5}}
                    }
                    type_name := Label{
                        width: Fit
                        height: Fit
                        draw_text +: {
                            text_style: theme.font_bold{font_size: 9}
                            color: theme.flow_text_muted
                        }
                    }
                }
                type_doc := MetaText{}
                doc_label := RowLabel{width: Fill text: "NODE NOTE"}
                node_doc := TextInput{
                    width: Fill
                    height: 64
                    is_multiline: true
                    empty_text: "Add a short note about this node"
                }
                doc_actions := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    align: Align{x: 1.0}
                    save_doc := ButtonFlat{text: "Save note"}
                }
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
            Text := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    name := RowLabel{}
                    value := TextInput{width: Fill height: 26 empty_text: ""}
                    reset := ButtonFlatter{text: "Reset"}
                }
                help := MetaText{margin: Inset{left: 88}}
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
                    reset := ButtonFlatter{text: "Reset"}
                    apply := ButtonFlat{text: "Apply"}
                }
                value := TextInput{
                    width: Fill
                    height: 110
                    is_multiline: true
                    empty_text: ""
                    draw_text +: {text_style: theme.font_code{font_size: 9}}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Number := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    name := RowLabel{}
                    value := mod.widgets.FabValueInput{
                        width: Fill
                        height: 24
                        quantize: true
                    }
                    reset := ButtonFlatter{text: "Reset"}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Choice := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    name := RowLabel{}
                    value := DropDown{width: Fill height: 26}
                    reset := ButtonFlatter{text: "Reset"}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Bool := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    name := RowLabel{}
                    value := Toggle{text: ""}
                    View{width: Fill height: 1}
                    reset := ButtonFlatter{text: "Reset"}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Color := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    name := RowLabel{}
                    value := mod.widgets.FabColorPick{width: 60 height: 20}
                    View{width: Fill height: 1}
                    reset := ButtonFlatter{text: "Reset"}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Edge := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    type_icon := Image{width: 14 height: 14 fit: ImageFit.Smallest}
                    name := RowLabel{width: 68}
                    source := ButtonFlatter{width: Fill text: ""}
                    disconnect := ButtonFlatter{text: "×"}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Port := View{
                width: Fill height: Fit flow: Down spacing: 2
                top := Row{
                    type_icon := Image{width: 14 height: 14 fit: ImageFit.Smallest}
                    name := RowLabel{width: 68}
                    consumers := ButtonFlatter{width: Fill text: ""}
                }
                help := MetaText{margin: Inset{left: 88}}
            }
            Result := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: theme.space_1
                padding: Inset{left: 2 right: 2 top: 4 bottom: 6}
                head := View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5} spacing: theme.space_1
                    name := RowLabel{width: Fill}
                    save := ButtonFlatter{text: "Save…"}
                    copy := ButtonFlatter{text: "Copy digest"}
                }
                thumb := View{
                    width: Fill height: 136 flow: Overlay cursor: MouseCursor.Hand
                    image := Image{width: Fill height: Fill fit: ImageFit.Smallest}
                }
                text_scroll := OutputScroll{
                    text := Label{
                        width: Fill height: Fit text: ""
                        draw_text +: {
                            text_style: theme.font_code{font_size: 8.5}
                            color: theme.flow_text_code
                        }
                    }
                }
                media := MetaText{}
                meta := MetaText{}
                marker := Label{visible: false}
            }
            Advanced := View{
                width: Fill height: Fit padding: Inset{top: 8 bottom: 2}
                toggle := ButtonFlat{width: Fill text: "▸  ADVANCED"}
            }
            FaceSource := View{
                width: Fill height: Fit flow: Down spacing: theme.space_1
                head := View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    name := RowLabel{width: Fill text: "FACE SOURCE"}
                    apply := ButtonFlat{text: "Apply"}
                }
                editor := mod.widgets.CodeView{
                    editor +: {width: Fill height: 150 read_only: false show_gutter: false word_wrap: true}
                }
            }
            RawSource := View{
                width: Fill height: Fit flow: Down spacing: theme.space_1
                name := RowLabel{width: Fill text: "NODE SOURCE (READ ONLY)"}
                source := ButtonFlatter{
                    width: Fill height: Fit text: ""
                    draw_text +: {text_style: theme.font_code{font_size: 8.5}}
                }
            }
            Empty := View{
                width: Fill
                height: Fit
                hint := EmptyHint{margin: Inset{top: 2}}
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
    SetFnSrc {
        node: String,
        src: String,
    },
    SetFaceSrc {
        node: String,
        src: String,
    },
    RenameNode {
        node: String,
        new_id: String,
    },
    SetNodeDoc {
        node: String,
        doc: String,
    },
    SetNodeMeta {
        node: String,
        key: String,
        value: Literal,
    },
    SelectNode(String),
    Disconnect {
        node: String,
        port: String,
    },
    JumpSource(String),
    SaveValue {
        node: String,
        port: String,
    },
    CopyDigest(String),
    OpenValue {
        node: String,
        port: String,
    },
}

#[derive(Clone, Debug)]
enum Row {
    Head {
        kind: String,
        id: String,
        type_name: String,
        type_doc: String,
        node_doc: String,
    },
    Empty(String),
    Section(String),
    Text {
        key: String,
        value: String,
        id: bool,
        number: bool,
        help: String,
        default: Option<Literal>,
    },
    Multiline {
        key: String,
        value: String,
        help: String,
        default: Option<Literal>,
    },
    Number {
        key: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        help: String,
        default: Option<Literal>,
    },
    Choice {
        key: String,
        value: String,
        options: Vec<String>,
        help: String,
        default: Option<Literal>,
    },
    Bool {
        key: String,
        value: bool,
        help: String,
        default: Option<Literal>,
    },
    Color {
        key: String,
        rgba: [f32; 4],
        help: String,
        default: Option<Literal>,
    },
    Edge {
        port: String,
        ty: makepad_flow::PortType,
        from_node: String,
        from_port: String,
    },
    Port {
        port: String,
        ty: makepad_flow::PortType,
        consumers: Vec<(String, String)>,
    },
    Result {
        port: String,
        value: ValueRef,
        bytes: Option<ValueBytes>,
    },
    Advanced,
    FaceSource(String),
    RawSource(String),
}

impl Row {
    fn template(&self) -> LiveId {
        match self {
            Row::Head { .. } => live_id!(Head),
            Row::Empty(_) => live_id!(Empty),
            Row::Section(_) => live_id!(Section),
            Row::Text { .. } => live_id!(Text),
            Row::Multiline { .. } => live_id!(Multiline),
            Row::Number { .. } => live_id!(Number),
            Row::Choice { .. } => live_id!(Choice),
            Row::Bool { .. } => live_id!(Bool),
            Row::Color { .. } => live_id!(Color),
            Row::Edge { .. } => live_id!(Edge),
            Row::Port { .. } => live_id!(Port),
            Row::Result { .. } => live_id!(Result),
            Row::Advanced => live_id!(Advanced),
            Row::FaceSource(_) => live_id!(FaceSource),
            Row::RawSource(_) => live_id!(RawSource),
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

fn inspector_commit_number(value: f64, range: (f64, f64, f64)) -> f64 {
    snap_stepped_value(value, range)
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

/// Parameters already represented by an editable control on the built-in
/// node card. The inspector is the complement of this list.
fn card_control_param(type_name: &str, key: &str) -> bool {
    // Keep the inspector and the card format picker on the same "Custom"
    // convention even though the picker itself owns width/height editing.
    let _custom = format_preset_name(&[], 0, 0);
    match type_name {
        "Image" => matches!(key, "width" | "height" | "steps" | "seed" | "model"),
        "Gen" => matches!(key, "width" | "height" | "model"),
        "Llm" | "Upscale" => key == "model",
        _ => false,
    }
}

fn inspector_setting_names<'a>(
    type_name: &str,
    params: &'a [(String, Literal)],
) -> Vec<&'a str> {
    params
        .iter()
        .filter(|(key, _)| {
            !matches!(key.as_str(), "ui" | "at" | "ports" | "domain" | "on_fail" | "label")
                && !(type_name == "Fn" && key == "out")
                && !card_control_param(type_name, key)
        })
        .map(|(key, _)| key.as_str())
        .collect()
}

fn literal_from_json(value: &JsonValue) -> Literal {
    match value {
        JsonValue::Null | JsonValue::Undefined => Literal::Null,
        JsonValue::Bool(value) => Literal::Bool(*value),
        JsonValue::U64(value) => Literal::Num(*value as f64),
        JsonValue::U128(value) => Literal::Num(*value as f64),
        JsonValue::I64(value) => Literal::Num(*value as f64),
        JsonValue::I128(value) => Literal::Num(*value as f64),
        JsonValue::F64(value) => Literal::Num(*value),
        JsonValue::String(value) => Literal::Str(value.clone()),
        JsonValue::BareIdent(value) => Literal::Id(value.clone()),
        JsonValue::Char(value) => Literal::Str(value.to_string()),
        JsonValue::Array(values) => {
            Literal::Arr(values.iter().map(literal_from_json).collect())
        }
        JsonValue::Object(values) => Literal::Obj(
            values
                .iter()
                .map(|(key, value)| (key.clone(), literal_from_json(value)))
                .collect(),
        ),
    }
}

fn setting_row(
    key: &str,
    value: &Literal,
    help: String,
    default: Option<Literal>,
    range: Option<(f64, f64, f64)>,
) -> Row {
    if let Literal::Num(number) = value {
        let (min, max, step) = range.unwrap_or_else(|| {
            let magnitude = number.abs().max(1.0);
            (
                0.0,
                (magnitude * 4.0).max(10.0),
                if number.fract() == 0.0 { 1.0 } else { 0.01 },
            )
        });
        return Row::Number {
            key: key.to_string(),
            value: *number,
            min,
            max,
            step,
            help,
            default,
        };
    }
    if let Literal::Bool(value) = value {
        return Row::Bool {
            key: key.to_string(),
            value: *value,
            help,
            default,
        };
    }
    if let Some(options) = parse_choices(&help).or_else(|| match key {
        "type" | "out" => Some(
            ["text", "image", "audio", "video", "mesh", "json", "list", "bytes"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        ),
        "method" => Some(
            ["get", "post", "put", "delete"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        ),
        "on_fail" => Some(["fail", "skip"].into_iter().map(str::to_string).collect()),
        _ => None,
    }) {
        return Row::Choice {
            key: key.to_string(),
            value: literal_text(value),
            options,
            help,
            default,
        };
    }
    if let Literal::Str(text) = value {
        if let Some(rgba) = parse_hex_color(text) {
            if key.contains("color") || key.contains("colour") || key.contains("tint") {
                return Row::Color {
                    key: key.to_string(),
                    rgba,
                    help,
                    default,
                };
            }
        }
        if matches!(key, "system" | "prompt" | "question" | "negative" | "default" | "brief")
            || text.contains('\n')
        {
            return Row::Multiline {
                key: key.to_string(),
                value: text.clone(),
                help,
                default,
            };
        }
    }
    Row::Text {
        key: key.to_string(),
        value: literal_text(value),
        id: matches!(value, Literal::Id(_)),
        number: false,
        help,
        default,
    }
}

fn port_icon_svg(ty: makepad_flow::PortType) -> &'static str {
    match ty {
        makepad_flow::PortType::Text => include_str!("../resources/icons/text.svg"),
        makepad_flow::PortType::Image => include_str!("../resources/icons/image.svg"),
        makepad_flow::PortType::Audio => include_str!("../resources/icons/audio.svg"),
        makepad_flow::PortType::Video => include_str!("../resources/icons/video.svg"),
        makepad_flow::PortType::Mesh => include_str!("../resources/icons/mesh.svg"),
        makepad_flow::PortType::Json | makepad_flow::PortType::List => {
            include_str!("../resources/icons/json.svg")
        }
        makepad_flow::PortType::Bytes => include_str!("../resources/icons/bytes.svg"),
    }
}

fn kind_icon_svg(kind: &str) -> &'static str {
    match kind {
        "input" => include_str!("../resources/icons/input.svg"),
        "output" => include_str!("../resources/icons/output.svg"),
        "chat" => include_str!("../resources/icons/chat.svg"),
        "gen" => include_str!("../resources/icons/gen.svg"),
        "fn" => include_str!("../resources/icons/fn.svg"),
        "http" => include_str!("../resources/icons/http.svg"),
        "ask" => include_str!("../resources/icons/ask.svg"),
        _ => include_str!("../resources/icons/flow.svg"),
    }
}

fn setting_action(node: &str, key: &str, value: Literal) -> InspectorAction {
    if matches!(key, "on_fail" | "label") {
        InspectorAction::SetNodeMeta {
            node: node.to_string(),
            key: key.to_string(),
            value,
        }
    } else {
        InspectorAction::SetParam {
            node: node.to_string(),
            key: key.to_string(),
            value,
        }
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
    advanced_rows: Vec<Row>,
    #[rust]
    advanced_open: bool,
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
        loaded: &[(String, ValueBytes)],
        source: Option<&str>,
    ) {
        let changed_node = self.node.as_deref() != node_id;
        self.rows.clear();
        self.advanced_rows.clear();
        if changed_node {
            self.advanced_open = false;
        }
        self.node = node_id.map(str::to_string);
        let node = graph.and_then(|graph| {
            graph
                .nodes
                .iter()
                .find(|node| Some(node.id.as_str()) == node_id)
        });
        let Some(node) = node else {
            self.rows.push(Row::Empty(
                "Select a node to inspect what its card does not already show.".into(),
            ));
            self.redraw(cx);
            return;
        };
        let entry = catalog.iter().find(|entry| entry.type_name == node.type_name);
        self.rows.push(Row::Head {
            kind: node.kind.clone(),
            id: node.id.clone(),
            type_name: node.type_name.clone(),
            type_doc: entry.map(|entry| entry.doc.clone()).unwrap_or_default(),
            node_doc: node.doc.clone().unwrap_or_default(),
        });
        self.rows.push(Row::Section("SETTINGS".into()));
        for key in inspector_setting_names(&node.type_name, &node.params) {
            let Some((_, value)) = node.params.iter().find(|(name, _)| name == key) else {
                continue;
            };
            let spec = entry.and_then(|entry| entry.params.iter().find(|param| param.name == key));
            let help = spec.map(|param| param.doc.clone()).unwrap_or_default();
            let default = spec.map(|param| literal_from_json(&param.default));
            let range = spec
                .and_then(|param| param.range.as_ref())
                .map(|range| (range.min, range.max, range.step.unwrap_or(1.0)))
                .or_else(|| parse_range(&help));
            self.rows
                .push(setting_row(key, value, help, default, range));
        }
        self.rows.push(setting_row(
            "on_fail",
            &Literal::Id(node.on_fail.clone()),
            "What to do when this node fails.".into(),
            Some(Literal::Id("fail".into())),
            None,
        ));
        self.rows.push(setting_row(
            "label",
            &Literal::Str(node.label.clone().unwrap_or_default()),
            "Optional display label for this node.".into(),
            Some(Literal::Str(String::new())),
            None,
        ));

        self.rows.push(Row::Section("CONNECTIONS".into()));
        for input in &node.inputs {
            match &input.value {
                NodeInputValue::Edge(edge) => self.rows.push(Row::Edge {
                    port: input.port.clone(),
                    ty: input.ty,
                    from_node: edge.from_node.clone(),
                    from_port: edge.from_port.clone(),
                }),
                NodeInputValue::Literal(value) => self.rows.push(setting_row(
                    &input.port,
                    value,
                    format!("{} input · literal value", input.ty.as_str()),
                    None,
                    None,
                )),
            }
        }
        if let Some(graph) = graph {
            for output in &node.outputs {
                let consumers = graph
                    .edges
                    .iter()
                    .filter(|edge| edge.from_node == node.id && edge.from_port == output.name)
                    .map(|edge| (edge.to_node.clone(), edge.to_port.clone()))
                    .collect();
                self.rows.push(Row::Port {
                    port: output.name.clone(),
                    ty: output.ty,
                    consumers,
                });
            }
        }

        if !outputs.is_empty() {
            self.rows.push(Row::Section("RESULT".into()));
            for (port, value) in outputs {
                self.rows.push(Row::Result {
                    port: port.clone(),
                    value: value.clone(),
                    bytes: loaded
                        .iter()
                        .find(|(name, _)| name == port)
                        .map(|(_, bytes)| bytes.clone()),
                });
            }
        }

        self.rows.push(Row::Advanced);
        self.advanced_rows.push(Row::FaceSource(
            node.face_src.clone().unwrap_or_default(),
        ));
        let raw = source
            .map(|source| {
                let start = source
                    .split_inclusive('\n')
                    .take(node.loc.line.saturating_sub(1) as usize)
                    .map(str::len)
                    .sum::<usize>();
                let end_line = graph
                    .into_iter()
                    .flat_map(|graph| graph.nodes.iter())
                    .filter(|other| other.loc.line > node.loc.line)
                    .map(|other| other.loc.line)
                    .min();
                let end = end_line
                    .map(|line| {
                        source
                            .split_inclusive('\n')
                            .take(line.saturating_sub(1) as usize)
                            .map(str::len)
                            .sum()
                    })
                    .unwrap_or(source.len());
                source.get(start..end).unwrap_or_default().trim().to_string()
            })
            .unwrap_or_default();
        self.advanced_rows.push(Row::RawSource(raw));
        if self.advanced_open {
            self.rows.extend(self.advanced_rows.clone());
        }
        self.redraw(cx);
    }

    /// Kept as the app's compatibility seam; results now read directly from
    /// the digest cache passed to `show_node`.
    pub fn set_preview(&mut self, _preview: Option<(String, ValueBytes)>) {}

    /// Card model pickers own their models. The focused inspector deliberately
    /// does not duplicate those controls.
    pub fn set_models(&mut self, _cx: &mut Cx, _models: Vec<ModelChoice>) {}

    /// Edits made in the rows, as actions for the app.
    pub fn changes(&mut self, cx: &mut Cx, actions: &Actions) -> Vec<InspectorAction> {
        let mut out = Vec::new();
        let mut toggle_advanced = false;
        let Some(node) = self.node.clone() else {
            return out;
        };
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(row) = self.rows.get(index) else {
                continue;
            };
            match row {
                Row::Head { id, node_doc, .. } => {
                    if let Some((new_id, _)) = item.text_input(cx, ids!(node_id)).returned(actions) {
                        let new_id = new_id.trim().to_string();
                        if !new_id.is_empty() && new_id != *id {
                            out.push(InspectorAction::RenameNode {
                                node: node.clone(),
                                new_id,
                            });
                        }
                    }
                    if item.button(cx, ids!(save_doc)).clicked(actions) {
                        let doc = item.text_input(cx, ids!(node_doc)).text();
                        if doc != *node_doc {
                            out.push(InspectorAction::SetNodeDoc {
                                node: node.clone(),
                                doc,
                            });
                        }
                    }
                }
                Row::Text {
                    key, id, number, default, ..
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
                        out.push(setting_action(&node, key, value));
                    }
                    if item.button(cx, ids!(reset)).clicked(actions) {
                        if let Some(default) = default.clone() {
                            out.push(setting_action(&node, key, default));
                        }
                    }
                }
                Row::Multiline { key, default, .. } => {
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
                    if item.button(cx, ids!(reset)).clicked(actions) {
                        if let Some(default) = default.clone() {
                            out.push(setting_action(&node, key, default));
                        }
                    }
                }
                Row::Number {
                    key,
                    min,
                    max,
                    step,
                    default,
                    ..
                } => {
                    if let Some(value) = item.fab_value_input(cx, ids!(value)).ended(actions) {
                        let value = inspector_commit_number(value, (*min, *max, *step));
                        item.fab_value_input(cx, ids!(value)).set_value(cx, value);
                        out.push(setting_action(&node, key, Literal::Num(value)));
                    }
                    if item.button(cx, ids!(reset)).clicked(actions) {
                        if let Some(default) = default.clone() {
                            out.push(setting_action(&node, key, default));
                        }
                    }
                }
                Row::Choice { key, default, .. } => {
                    if let Some(label) = item.drop_down(cx, ids!(value)).changed_label(actions) {
                        let value = if matches!(key.as_str(), "type" | "method" | "out" | "on_fail") {
                            Literal::Id(label)
                        } else if let Ok(number) = label.parse::<f64>() {
                            Literal::Num(number)
                        } else {
                            Literal::Str(label)
                        };
                        out.push(setting_action(&node, key, value));
                    }
                    if item.button(cx, ids!(reset)).clicked(actions) {
                        if let Some(default) = default.clone() {
                            out.push(setting_action(&node, key, default));
                        }
                    }
                }
                Row::Bool { key, default, .. } => {
                    if let Some(flag) = item.check_box(cx, ids!(value)).changed(actions) {
                        out.push(setting_action(&node, key, Literal::Bool(flag)));
                    }
                    if item.button(cx, ids!(reset)).clicked(actions) {
                        if let Some(default) = default.clone() {
                            out.push(setting_action(&node, key, default));
                        }
                    }
                }
                Row::Color { key, default, .. } => {
                    if let Some(rgba) = item.fab_color_pick(cx, ids!(value)).changed(actions) {
                        out.push(setting_action(&node, key, Literal::Str(hex_color(rgba))));
                    }
                    if item.button(cx, ids!(reset)).clicked(actions) {
                        if let Some(default) = default.clone() {
                            out.push(setting_action(&node, key, default));
                        }
                    }
                }
                Row::Edge { port, from_node, .. } => {
                    if item.button(cx, ids!(source)).clicked(actions) {
                        out.push(InspectorAction::SelectNode(from_node.clone()));
                    }
                    if item.button(cx, ids!(disconnect)).clicked(actions) {
                        out.push(InspectorAction::Disconnect {
                            node: node.clone(),
                            port: port.clone(),
                        });
                    }
                }
                Row::Port { consumers, .. } => {
                    if item.button(cx, ids!(consumers)).clicked(actions) {
                        if let Some((consumer, _)) = consumers.first() {
                            out.push(InspectorAction::SelectNode(consumer.clone()));
                        }
                    }
                }
                Row::Result { port, value, .. } => {
                    if item
                        .view(cx, ids!(thumb))
                        .finger_up(actions)
                        .is_some_and(|up| up.is_over)
                    {
                        out.push(InspectorAction::OpenValue {
                            node: node.clone(),
                            port: port.clone(),
                        });
                    }
                    if item.button(cx, ids!(save)).clicked(actions) {
                        out.push(InspectorAction::SaveValue {
                            node: node.clone(),
                            port: port.clone(),
                        });
                    }
                    if item.button(cx, ids!(copy)).clicked(actions) {
                        out.push(InspectorAction::CopyDigest(value.digest.clone()));
                    }
                }
                Row::Advanced => {
                    if item.button(cx, ids!(toggle)).clicked(actions) {
                        toggle_advanced = true;
                    }
                }
                Row::FaceSource(_) => {
                    if item.button(cx, ids!(apply)).clicked(actions) {
                        out.push(InspectorAction::SetFaceSrc {
                            node: node.clone(),
                            src: item.widget(cx, ids!(editor)).text(),
                        });
                    }
                }
                Row::RawSource(_) => {
                    if item.button(cx, ids!(source)).clicked(actions) {
                        out.push(InspectorAction::JumpSource(node.clone()));
                    }
                }
                _ => {}
            }
        }
        if toggle_advanced {
            self.advanced_open = !self.advanced_open;
            if self.advanced_open {
                self.rows.extend(self.advanced_rows.clone());
            } else if let Some(index) = self.rows.iter().position(|row| matches!(row, Row::Advanced)) {
                self.rows.truncate(index + 1);
            }
            self.redraw(cx);
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
                    Row::Head {
                        kind,
                        id,
                        type_name,
                        type_doc,
                        node_doc,
                    } => {
                        let id_input = item.text_input(cx, ids!(node_id));
                        let changed = !existed || id_input.text() != *id;
                        if changed {
                            id_input.set_text(cx, id);
                            let _ = item
                                .image(cx, ids!(kind_icon))
                                .load_svg_from_data(cx, kind_icon_svg(kind).as_bytes());
                            item.text_input(cx, ids!(node_doc)).set_text(cx, node_doc);
                        }
                        item.label(cx, ids!(type_name)).set_text(cx, type_name);
                        item.label(cx, ids!(type_doc)).set_text(cx, type_doc);
                    }
                    Row::Empty(hint) => item.label(cx, ids!(hint)).set_text(cx, hint),
                    Row::Section(title) => item.label(cx, ids!(title)).set_text(cx, title),
                    Row::Text {
                        key,
                        value,
                        id,
                        help,
                        default,
                        ..
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(help)).set_text(cx, help);
                        if !existed {
                            item.text_input(cx, ids!(value)).set_text(cx, value);
                        }
                        let current = if *id {
                            Literal::Id(value.clone())
                        } else {
                            Literal::Str(value.clone())
                        };
                        let reset = item.button(cx, ids!(reset));
                        reset.set_visible(cx, default.is_some());
                        reset.set_enabled(
                            cx,
                            default.as_ref().is_some_and(|value| value != &current),
                        );
                    }
                    Row::Multiline {
                        key,
                        value,
                        help,
                        default,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(help)).set_text(cx, help);
                        if !existed {
                            item.text_input(cx, ids!(value)).set_text(cx, value);
                        }
                        let reset = item.button(cx, ids!(reset));
                        reset.set_visible(cx, default.is_some());
                        reset.set_enabled(
                            cx,
                            default
                                .as_ref()
                                .is_some_and(|default| default != &Literal::Str(value.clone())),
                        );
                    }
                    Row::Number {
                        key,
                        value,
                        min,
                        max,
                        step,
                        help,
                        default,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(help)).set_text(cx, help);
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
                        let reset = item.button(cx, ids!(reset));
                        reset.set_visible(cx, default.is_some());
                        reset.set_enabled(
                            cx,
                            default
                                .as_ref()
                                .is_some_and(|default| default != &Literal::Num(*value)),
                        );
                    }
                    Row::Choice {
                        key,
                        value,
                        options,
                        help,
                        default,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(help)).set_text(cx, help);
                        let drop_down = item.drop_down(cx, ids!(value));
                        if !existed {
                            drop_down.set_labels(cx, options.clone());
                            drop_down.set_selected_by_label(value, cx);
                        }
                        let reset = item.button(cx, ids!(reset));
                        reset.set_visible(cx, default.is_some());
                        reset.set_enabled(
                            cx,
                            default.as_ref().is_some_and(|default| literal_text(default) != *value),
                        );
                    }
                    Row::Bool {
                        key,
                        value,
                        help,
                        default,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(help)).set_text(cx, help);
                        if !existed {
                            item.check_box(cx, ids!(value)).set_active(cx, *value, Animate::No);
                        }
                        let reset = item.button(cx, ids!(reset));
                        reset.set_visible(cx, default.is_some());
                        reset.set_enabled(
                            cx,
                            default
                                .as_ref()
                                .is_some_and(|default| default != &Literal::Bool(*value)),
                        );
                    }
                    Row::Color {
                        key,
                        rgba,
                        help,
                        default,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, key);
                        item.label(cx, ids!(help)).set_text(cx, help);
                        if !existed {
                            item.fab_color_pick(cx, ids!(value)).set_rgba(cx, *rgba);
                        }
                        let reset = item.button(cx, ids!(reset));
                        reset.set_visible(cx, default.is_some());
                        reset.set_enabled(
                            cx,
                            default.as_ref().is_some_and(|default| {
                                literal_text(default)
                                    != hex_color(Vec4f { x: rgba[0], y: rgba[1], z: rgba[2], w: rgba[3] })
                            }),
                        );
                    }
                    Row::Edge {
                        port,
                        ty,
                        from_node,
                        from_port,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, port);
                        item.button(cx, ids!(source))
                            .set_text(cx, &format!("← {from_node}.{from_port}"));
                        item.label(cx, ids!(help))
                            .set_text(cx, &format!("{} input", ty.as_str()));
                        if !existed {
                            let _ = item
                                .image(cx, ids!(type_icon))
                                .load_svg_from_data(cx, port_icon_svg(*ty).as_bytes());
                        }
                    }
                    Row::Port {
                        port,
                        ty,
                        consumers,
                    } => {
                        item.label(cx, ids!(name)).set_text(cx, port);
                        let label = if consumers.is_empty() {
                            "No consumers".to_string()
                        } else {
                            format!(
                                "{} → {}",
                                consumers.len(),
                                consumers
                                    .iter()
                                    .map(|(node, port)| format!("{node}.{port}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        item.button(cx, ids!(consumers)).set_text(cx, &label);
                        item.button(cx, ids!(consumers))
                            .set_enabled(cx, !consumers.is_empty());
                        item.label(cx, ids!(help))
                            .set_text(cx, &format!("{} output", ty.as_str()));
                        if !existed {
                            let _ = item
                                .image(cx, ids!(type_icon))
                                .load_svg_from_data(cx, port_icon_svg(*ty).as_bytes());
                        }
                    }
                    Row::Result { port, value, bytes } => {
                        item.label(cx, ids!(name)).set_text(cx, port);
                        item.label(cx, ids!(meta)).set_text(
                            cx,
                            &format!(
                                "{} · {} · {}",
                                value.digest,
                                crate::faces::size_text(value.bytes),
                                value.content_type
                            ),
                        );
                        let is_image = value.content_type.starts_with("image/");
                        let is_text = value.content_type.starts_with("text/")
                            || value.content_type == "application/json";
                        item.view(cx, ids!(thumb)).set_visible(cx, is_image);
                        item.view(cx, ids!(text_scroll)).set_visible(cx, is_text);
                        item.label(cx, ids!(media))
                            .set_visible(cx, !is_image && !is_text);
                        item.label(cx, ids!(media)).set_text(
                            cx,
                            &format!("{} value", value.ty.as_str()),
                        );
                        let text = bytes
                            .as_ref()
                            .filter(|_| is_text)
                            .map(|bytes| String::from_utf8_lossy(&bytes.bytes).into_owned())
                            .or_else(|| crate::faces::preview_text(value))
                            .unwrap_or_default();
                        item.label(cx, ids!(text)).set_text(cx, &text);
                        if is_image {
                            let marker = item.label(cx, ids!(marker));
                            let image = item.image(cx, ids!(image));
                            let needs_load = marker.text() != value.digest || !image.has_content();
                            marker.set_text(cx, &value.digest);
                            if needs_load {
                                let loaded = bytes
                                    .as_ref()
                                    .is_some_and(|bytes| image.load_image_from_data(cx, &bytes.bytes).is_ok());
                                image.set_visible(cx, loaded);
                            }
                        }
                    }
                    Row::Advanced => item.button(cx, ids!(toggle)).set_text(
                        cx,
                        if self.advanced_open { "▾  ADVANCED" } else { "▸  ADVANCED" },
                    ),
                    Row::FaceSource(source) => {
                        if !existed {
                            item.widget(cx, ids!(editor)).set_text(cx, source);
                        }
                    }
                    Row::RawSource(source) => {
                        item.button(cx, ids!(source)).set_text(cx, source);
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
    OpenImage {
        instance: String,
        label: String,
        digest: String,
    },
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
    #[rust]
    thumbnails: HashMap<String, ValueBytes>,
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

    pub fn set_thumbnail(&mut self, cx: &mut Cx, bytes: ValueBytes) {
        self.thumbnails.insert(bytes.digest.clone(), bytes);
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
            if item
                .view(cx, ids!(thumbnail))
                .finger_up(actions)
                .is_some_and(|up| up.is_over)
            {
                if let Some((label, value)) = row
                    .outputs
                    .iter()
                    .find(|(_, value)| value.content_type.starts_with("image/"))
                {
                    out.push(RunningAction::OpenImage {
                        instance: row.instance.clone(),
                        label: label.clone(),
                        digest: value.digest.clone(),
                    });
                }
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
                let picture = row
                    .outputs
                    .iter()
                    .find(|(_, value)| value.content_type.starts_with("image/"));
                let thumb = item.view(cx, ids!(thumbnail));
                thumb.set_visible(cx, picture.is_some());
                if let Some((_, value)) = picture {
                    let marker = item.label(cx, ids!(marker));
                    let image = item.image(cx, ids!(image));
                    let changed = marker.text() != value.digest || !image.has_content();
                    marker.set_text(cx, &value.digest);
                    if changed {
                        let loaded = self
                            .thumbnails
                            .get(&value.digest)
                            .is_some_and(|bytes| {
                                image.load_image_from_data(cx, &bytes.bytes).is_ok()
                            });
                        image.set_visible(cx, loaded);
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

#[cfg(test)]
mod tests {
    use super::{inspector_commit_number, inspector_setting_names};
    use makepad_flow::Literal;

    #[test]
    fn inspector_number_commit_snaps_and_clamps() {
        let range = (256.0, 2048.0, 16.0);
        assert_eq!(inspector_commit_number(1064.0, range), 1072.0);
        assert_eq!(inspector_commit_number(2057.0, range), 2048.0);
    }

    #[test]
    fn image_settings_are_the_complement_of_card_controls() {
        let params = [
            "width", "height", "steps", "seed", "model", "negative", "guidance", "loras",
        ]
        .into_iter()
        .map(|name| (name.to_string(), Literal::Null))
        .collect::<Vec<_>>();
        assert_eq!(
            inspector_setting_names("Image", &params),
            vec!["negative", "guidance", "loras"]
        );
    }

    #[test]
    fn card_setting_split_is_specific_to_each_builtin_type() {
        let params = ["width", "height", "model", "system", "temperature"]
            .into_iter()
            .map(|name| (name.to_string(), Literal::Null))
            .collect::<Vec<_>>();
        assert_eq!(
            inspector_setting_names("Llm", &params),
            vec!["width", "height", "system", "temperature"]
        );
        assert_eq!(
            inspector_setting_names("Gen", &params),
            vec!["system", "temperature"]
        );
        assert_eq!(
            inspector_setting_names("Http", &params),
            vec!["width", "height", "model", "system", "temperature"]
        );
    }
}
