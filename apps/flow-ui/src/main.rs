// integration notes
// F3 supplied the canvas, faces, inspector, source/App views and running list;
// F4 supplied `services` (the aichat bridge) and the port/event/worker wiring
// kept here; F3b rewrote the canvas (transform camera, cards, progress bars),
// the faces and the panels, and added the menu bar, the toolbar with the
// run's total bar, the template picker behind New, the hub model lists and the
// dev-only `FLOW_GEN_BASE_URL` seam. The client keeps typed instance/run/value
// methods plus `_json` render projections; `FlowClient::models` is additive.

mod services;

use crate::services::{BridgeContext, FlowServices, FlowUiAction};
pub use makepad_widgets;

mod faces;
mod graph_edit;
mod graph_view;
mod panels;
mod testpattern;
mod theme;
mod values;
mod viewer;

use faces::{model_choices, BridgeCall, FaceBridgeCall, FaceHost, ModelChoice};
use makepad_flow::client::{
    ClientError, FlowClient, FlowSubscriber, FlowSubscriberConfig, SessionConfig,
    SessionConnector, SessionStatus, SubscriptionEvent,
};
use makepad_flow::embed::{default_root, resolve, EmbedPolicy, Resolved};
use makepad_flow::engine::{FixedGen, HubChat, HubHttp, Seams};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{
    CreateInstanceRequest, CreateInstanceResponse, CreateRunResponse, Event as FlowEvent,
    FlowDefinition, FlowSummary, Graph, InstanceRow, Literal, ModelsResponse, NodeTypeCatalog,
    NodeState, NodesResponse, PortType, PutFlowResponse, RunRowDto, RunState, TemplateSummary,
    ValueRef,
};
use makepad_widgets::makepad_draw::text::selection::Cursor;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;
use makepad_flowgraph::{
    CanvasEdit, FlowCanvas, FlowCanvasAction, NodeFacesScope, NodeStatus, Selection, FIRST_AT,
};
use makepad_flowgraph::wire_route::WireMode;
use panels::{
    AppView, FlowList, Inspector, InspectorAction, Palette, PaletteAction, RunBar, RunningAction,
    RunningList, TemplatePicker,
};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use values::ValueCache;
use viewer::{ImageViewer, ImageViewerAction, ImageViewerItem};

app_main!(App);

/// The template the app starts from when the root is empty.
const FIRST_TEMPLATE: &str = "prompt-to-image";
/// How often the hub's model lists are refreshed while a flow with generators is open.
const MODELS_REFRESH_SECS: f64 = 10.0;

fn set_wire_menu_checks(menus: &mut [MenuDef], mode: WireMode) {
    for entry in menus.iter_mut().flat_map(|menu| &mut menu.items) {
        if entry.id == live_id!(wires_routed) {
            entry.label = if mode == WireMode::Routed {
                "  ✓ Routed"
            } else {
                "    Routed"
            }
            .to_string();
        } else if entry.id == live_id!(wires_bezier) {
            entry.label = if mode == WireMode::Bezier {
                "  ✓ Bezier"
            } else {
                "    Bezier"
            }
            .to_string();
        }
    }
}

fn is_focus_routed_input(event: &Event) -> bool {
    matches!(
        event,
        Event::KeyDown(_)
            | Event::KeyUp(_)
            | Event::TextInput(_)
            | Event::TextRangeReplace(_)
            | Event::TextCopy(_)
            | Event::TextCut(_)
            | Event::ImeAction(_)
            | Event::SelectionHandleDrag(_)
    )
}

fn is_viewer_modal_input(event: &Event) -> bool {
    is_focus_routed_input(event)
        || matches!(
            event,
            Event::MouseDown(_)
                | Event::MouseMove(_)
                | Event::MouseUp(_)
                | Event::MouseLeave(_)
                | Event::TouchUpdate(_)
                | Event::LongPress(_)
                | Event::Scroll(_)
                | Event::PhysicalKeyboard(_)
                | Event::Drag(_)
                | Event::Drop(_)
                | Event::DragEnd
                | Event::MacosMenuCommand(_)
        )
}

const HELP_SHORTCUTS: &str = "⌘N  new flow from a template
⌘O  open the next flow
⌘S  save the source pane
⌘Z / ⇧⌘Z  undo / redo a graph edit
⌘⌫  delete the selected node
⌘D  duplicate the selected node
⌘R  run · ⌘.  cancel · ⇧⌘K  clear outputs · ⌥⌘R  run to the selected node
⌘1 / ⌘2 / ⌘3  canvas · app view · source
F  flip the selected card's input/output facing
⌘= / ⌘-  zoom in / out · ⌘0  100 % · Home  fit
drag empty canvas to pan · wheel to zoom at the cursor
drag from a port to wire it · drag a palette card onto the canvas";

const HELP_ABOUT: &str = "Flow — makepad's canvas for AI pipelines.
Every node runs on the hub fleet; the file is the truth and this window is a view of it.
The flow server runs embedded in this window unless another one already serves the root.";

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let FlowCanvas = mod.widgets.FlowCanvas{
        draw_bg +: {
            color_a: theme.flow_grid_a
            color_b: theme.flow_grid_b
        }
        draw_card +: {shadow_color: theme.flow_shadow}
        draw_title +: {color: theme.flow_text}
        draw_meta +: {color: theme.flow_text_muted}
        draw_port +: {color: theme.flow_text_port}
        draw_chip +: {color: theme.flow_text_chip}
        draw_error +: {color: theme.flow_error}
        card_color: theme.flow_surface
        card_color_hover: theme.flow_surface_hover
        card_edge_color: theme.flow_edge
        accent_color: theme.flow_accent
        highlight_color: theme.flow_highlight
        color_input: theme.flow_input
        color_output: theme.flow_success
        color_chat: theme.flow_chat
        color_gen: theme.flow_generation
        color_fn: theme.flow_function
        color_http: theme.flow_http
        color_ask: theme.flow_waiting
        color_flow: theme.flow_text_port
        color_port_text: theme.flow_port_text
        color_port_image: theme.flow_port_image
        color_port_audio: theme.flow_port_audio
        color_port_video: theme.flow_port_video
        color_port_mesh: theme.flow_port_mesh
        color_port_json: theme.flow_port_json
        color_port_list: theme.flow_port_list
        color_port_bytes: theme.flow_port_bytes
        color_state_running: theme.flow_state_running
        color_state_done: theme.flow_success
        color_state_failed: theme.flow_error
        color_state_waiting: theme.flow_waiting
        color_state_inactive: theme.flow_text_muted
        color_state_idle: theme.flow_state_idle
        color_port_label_connected: theme.flow_text_port_connected
        color_port_label_open: theme.flow_text_port_open

        styles +: {
            FlowNodeStyle{kind: "input" color: theme.flow_input icon: crate_resource("self:resources/icons/input.svg")}
            FlowNodeStyle{kind: "output" color: theme.flow_success icon: crate_resource("self:resources/icons/output.svg")}
            FlowNodeStyle{kind: "chat" color: theme.flow_chat icon: crate_resource("self:resources/icons/chat.svg")}
            FlowNodeStyle{kind: "gen" color: theme.flow_generation icon: crate_resource("self:resources/icons/gen.svg")}
            FlowNodeStyle{kind: "fn" color: theme.flow_function icon: crate_resource("self:resources/icons/fn.svg")}
            FlowNodeStyle{kind: "http" color: theme.flow_http icon: crate_resource("self:resources/icons/http.svg")}
            FlowNodeStyle{kind: "ask" color: theme.flow_waiting icon: crate_resource("self:resources/icons/ask.svg")}
            FlowNodeStyle{kind: "flow" color: theme.flow_text_port icon: crate_resource("self:resources/icons/flow.svg")}

            FlowPortStyle{kind: "text" color: theme.flow_port_text icon: crate_resource("self:resources/icons/text.svg")}
            FlowPortStyle{kind: "image" color: theme.flow_port_image icon: crate_resource("self:resources/icons/image.svg")}
            FlowPortStyle{kind: "audio" color: theme.flow_port_audio icon: crate_resource("self:resources/icons/audio.svg")}
            FlowPortStyle{kind: "video" color: theme.flow_port_video icon: crate_resource("self:resources/icons/video.svg")}
            FlowPortStyle{kind: "mesh" color: theme.flow_port_mesh icon: crate_resource("self:resources/icons/mesh.svg")}
            FlowPortStyle{kind: "json" color: theme.flow_port_json icon: crate_resource("self:resources/icons/json.svg")}
            FlowPortStyle{kind: "list" color: theme.flow_port_list icon: crate_resource("self:resources/icons/json.svg")}
            FlowPortStyle{kind: "bytes" color: theme.flow_port_bytes icon: crate_resource("self:resources/icons/bytes.svg")}
        }
    }

    let SectionTitle = Label{
        width: Fill
        height: Fit
        margin: Inset{top: 6 bottom: 2}
        text: ""
        draw_text +: {
            color: theme.flow_text_subtle
            text_style: theme.font_bold{font_size: 8.5}
        }
    }

    let Panel = RoundedShadowView{
        width: Fill
        height: Fill
        flow: Down
        cursor: MouseCursor.Default
        grab_key_focus: false
        padding: Inset{left: 10 right: 10 top: 8 bottom: 10}
        spacing: theme.space_1
        show_bg: true
        draw_bg +: {
            color: theme.flow_surface_translucent
            border_radius: 12.0
            border_size: 1.0
            border_color: theme.flow_surface_hover
            shadow_color: theme.flow_shadow
            shadow_radius: 12.0
            shadow_offset: vec2(0.0, 0.0)
        }
    }

    let PanelSplitter = Splitter{
        width: Fill
        height: Fill
        size: 8.0
        draw_bg +: {
            color_bg: theme.flow_clear
            color: theme.flow_divider
            color_hover: theme.flow_accent_hover
            color_drag: theme.flow_accent
            splitter_pad: 2.0
            bar_size: 72.0
            border_radius: 2.0
        }
    }

    let ToolText = Label{
        width: Fit
        height: Fit
        text: ""
        draw_text +: {
            color: theme.flow_text_muted
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    let HelpPanel = RoundedView{
        width: 560
        height: Fit
        flow: Down
        spacing: theme.space_2
        padding: Inset{left: 16 right: 16 top: 12 bottom: 14}
        show_bg: true
        draw_bg +: {
            color: theme.flow_surface
            border_radius: 14.0
            border_size: 1.0
            border_color: theme.flow_edge_soft
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Flow"
                window.inner_size: vec2(1800, 1000)
                pass.clear_color: theme.flow_window
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down

                    menu_bar := MenuBar{
                        menus: [
                            {label: "File" items: [
                                {id: @new_from_template label: "New from template…" shortcut: "Cmd+N"}
                                {id: @open_flow label: "Open next flow" shortcut: "Cmd+O"}
                                {id: @save label: "Save source" shortcut: "Cmd+S"}
                                {id: @revert label: "Revert last edit"}
                                {sep: true}
                                {id: @delete_flow label: "Delete flow"}
                                {sep: true}
                                {id: @quit label: "Quit" shortcut: "Cmd+Q"}
                            ]}
                            {label: "Edit" items: [
                                {id: @undo label: "Undo" shortcut: "Cmd+Z"}
                                {id: @redo label: "Redo" shortcut: "Shift+Cmd+Z"}
                                {sep: true}
                                {id: @delete_selection label: "Delete" shortcut: "Cmd+Backspace"}
                                {id: @select_all label: "Select all" shortcut: "Cmd+A"}
                                {id: @duplicate label: "Duplicate node" shortcut: "Cmd+D"}
                            ]}
                            {label: "View" items: [
                                {id: @view_canvas label: "Canvas" shortcut: "Cmd+1"}
                                {id: @view_app label: "App view" shortcut: "Cmd+2"}
                                {id: @view_source label: "Source" shortcut: "Cmd+3"}
                                {id: @view_inspector label: "Inspector" shortcut: "Cmd+I"}
                                {id: @flip_card label: "Flip card" shortcut: "F"}
                                {sep: true}
                                {id: @toggle_left label: "Flows, Running and Palette" shortcut: "Cmd+L"}
                                {sep: true}
                                {id: @zoom_in label: "Zoom in" shortcut: "Cmd+Plus"}
                                {id: @zoom_out label: "Zoom out" shortcut: "Cmd+Minus"}
                                {id: @zoom_fit label: "Fit" shortcut: "Home"}
                                {id: @zoom_100 label: "100 %" shortcut: "Cmd+0"}
                                {sep: true}
                                {id: @wires_heading label: "Wires" enabled: false}
                                {id: @wires_routed label: "  ✓ Routed"}
                                {id: @wires_bezier label: "    Bezier"}
                            ]}
                            {label: "Run" items: [
                                {id: @run label: "Run" shortcut: "Cmd+R"}
                                {id: @cancel label: "Cancel" shortcut: "Cmd+Period"}
                                {id: @clear_outputs label: "Clear outputs" shortcut: "Shift+Cmd+K"}
                                {id: @run_to_node label: "Run to selected node" shortcut: "Alt+Cmd+R"}
                            ]}
                            {label: "Help" items: [
                                {id: @help_shortcuts label: "Keyboard shortcuts" shortcut: "F1"}
                                {id: @help_language label: "Flow language cheat sheet"}
                                {sep: true}
                                {id: @help_about label: "About Flow"}
                            ]}
                        ]
                    }

                    workspace := View{
                        width: Fill
                        height: Fill
                        flow: Overlay

                        canvas_view := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            canvas := FlowCanvas{}
                        }
                        app_view_view := View{
                            width: Fill
                            height: Fill
                            visible: false
                            app_view := AppView{}
                        }

                        chrome := View{
                            width: Fill
                            height: Fill
                            flow: Down

                            toolbar := RoundedShadowView{
                                width: Fill
                                height: 46
                                flow: Right
                                cursor: MouseCursor.Default
                                grab_key_focus: false
                                spacing: theme.space_2
                                padding: Inset{left: 12 right: 12 top: 6 bottom: 6}
                                margin: Inset{left: 8 right: 8 top: 4 bottom: 4}
                                align: Align{y: 0.5}
                                show_bg: true
                                draw_bg +: {
                                    color: theme.flow_surface_translucent
                                    border_radius: 12.0
                                    border_size: 1.0
                                    border_color: theme.flow_surface_hover
                                    shadow_color: theme.flow_shadow
                                    shadow_radius: 12.0
                                    shadow_offset: vec2(0.0, 0.0)
                                }
                                status_dot := RoundedView{
                                    width: 8
                                    height: 8
                                    draw_bg +: {
                                        border_radius: 4.0
                                        color: theme.flow_text_muted
                                    }
                                }
                                status_chip := ToolText{text: "Discovering"}
                                flow_name := Label{
                                    width: Fit
                                    height: Fit
                                    margin: Inset{left: 10}
                                    text: "No flow open"
                                    draw_text +: {
                                        color: theme.flow_text
                                        text_style: theme.font_bold{font_size: 11}
                                    }
                                }
                                instance_chip := ToolText{}
                                View{width: 10 height: 1}
                                run_btn := Button{text: "▶ Run"}
                                cancel_btn := ButtonFlat{text: "Cancel"}
                                clear_btn := ButtonFlat{text: "Clear"}
                                run_bar := RunBar{width: 220 height: 6 margin: Inset{left: 6 right: 6}}
                                run_state := ToolText{width: Fill}
                                zoom_label := ToolText{text: "100 %"}
                                fit_btn := ButtonFlat{text: "Fit"}
                                view_btn := ButtonFlat{text: "App view"}
                                side_btn := ButtonFlat{text: "Source"}
                                new_btn := Button{text: "New"}
                            }

                            column_split := PanelSplitter{
                                width: Fill
                                height: Fill
                                axis: SplitterAxis.Horizontal
                                align: SplitterAlign.FromA(244.0)
                                min_vertical: 180.0
                                max_vertical: 620.0
                                a: View{
                                    width: Fill
                                    height: Fill
                                    left_panel := View{
                                        width: Fill
                                        height: Fill
                                        clip_x: false
                                        clip_y: false
                                        padding: Inset{left: 8 right: 4 bottom: 8}
                                        left_flow_split := PanelSplitter{
                                            axis: SplitterAxis.Vertical
                                            align: SplitterAlign.FromA(176.0)
                                            min_horizontal: 72.0
                                            max_horizontal: 220.0
                                            a: Panel{
                                                SectionTitle{text: "FLOWS"}
                                                flow_list := FlowList{}
                                            }
                                            b: View{
                                                width: Fill
                                                height: Fill
                                                left_running_split := PanelSplitter{
                                                    axis: SplitterAxis.Vertical
                                                    align: SplitterAlign.FromA(194.0)
                                                    min_horizontal: 100.0
                                                    max_horizontal: 104.0
                                                    a: Panel{
                                                        SectionTitle{text: "RUNNING"}
                                                        running := RunningList{}
                                                    }
                                                    b: Panel{
                                                        SectionTitle{text: "PALETTE"}
                                                        palette_note := Label{
                                                            width: Fill
                                                            height: Fit
                                                            text: "Drag a card onto the canvas to add a node."
                                                            draw_text +: {
                                                                color: theme.flow_text_hint
                                                                text_style: theme.font_regular{font_size: 8.5}
                                                            }
                                                        }
                                                        palette := Palette{}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                b: View{
                                    width: Fill
                                    height: Fill
                                    canvas_right_split := PanelSplitter{
                                        axis: SplitterAxis.Horizontal
                                        align: SplitterAlign.FromB(338.0)
                                        min_vertical: 320.0
                                        max_vertical: 260.0
                                        a: View{width: Fill height: Fill}
                                        b: View{
                                            width: Fill
                                            height: Fill
                                            right_panel := View{
                                                width: Fill
                                                height: Fill
                                                clip_x: false
                                                clip_y: false
                                                padding: Inset{left: 4 right: 8 bottom: 8}
                                                right_source_split := PanelSplitter{
                                                    axis: SplitterAxis.Vertical
                                                    align: SplitterAlign.FromB(330.0)
                                                    min_horizontal: 84.0
                                                    max_horizontal: 84.0
                                                    a: Panel{
                                                        spacing: theme.space_1
                                                        SectionTitle{text: "INSPECTOR"}
                                                        inspector := Inspector{}
                                                    }
                                                    b: View{
                                                        width: Fill
                                                        height: Fill
                                                        source_view := Panel{
                                                            visible: false
                                                            spacing: theme.space_2
                                                            View{
                                                                width: Fill
                                                                height: Fit
                                                                flow: Right
                                                                align: Align{y: 0.5}
                                                                SectionTitle{width: Fill text: "SOURCE"}
                                                                save_btn := Button{text: "Save"}
                                                            }
                                                            source := TextInput{
                                                                width: Fill
                                                                height: Fill
                                                                is_multiline: true
                                                                empty_text: "Open a flow to edit its source"
                                                                draw_text +: {text_style: theme.font_code{font_size: 9}}
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        error_line := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            align: Align{y: 1.0}
                            padding: Inset{left: 12 right: 12 bottom: 10}
                            error_label := Label{
                                width: Fill
                                height: Fit
                                text: ""
                                draw_text +: {
                                    color: theme.flow_error
                                    text_style: theme.font_regular{font_size: 9}
                                }
                            }
                        }
                        template_view := View{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            visible: false
                            template_picker := TemplatePicker{}
                        }
                        help_view := View{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            visible: false
                            help := HelpPanel{
                                head := View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    align: Align{y: 0.5}
                                    help_title := Label{
                                        width: Fill
                                        height: Fit
                                        text: ""
                                        draw_text +: {
                                            text_style: theme.font_bold{font_size: 11}
                                            color: theme.flow_text
                                        }
                                    }
                                    help_close := ButtonFlat{text: "Close"}
                                }
                                help_body := ScrollYView{
                                    width: Fill
                                    height: Fit{max: FitBound.Abs(520)}
                                    help_text := Label{
                                        width: Fill
                                        height: Fit
                                        text: ""
                                        draw_text +: {
                                            color: theme.flow_text_body
                                            text_style: theme.font_regular{font_size: 9.5}
                                        }
                                    }
                                }
                            }
                        }
                        value_preview_view := View{
                            width: Fill
                            height: Fill
                            flow: Overlay
                            visible: false
                            preview_scrim := RoundedView{
                                width: Fill
                                height: Fill
                                show_bg: true
                                draw_bg +: {color: theme.flow_scrim}
                            }
                            preview_panel := RoundedShadowView{
                                width: Fill
                                height: Fill
                                margin: Inset{left: 36 right: 36 top: 36 bottom: 36}
                                padding: Inset{left: 14 right: 14 top: 12 bottom: 14}
                                spacing: theme.space_2
                                flow: Down
                                show_bg: true
                                draw_bg +: {
                                    color: theme.flow_surface
                                    border_color: theme.flow_edge_soft
                                    border_size: 1.0
                                    border_radius: 14.0
                                    shadow_color: theme.flow_shadow
                                    shadow_radius: 18.0
                                }
                                preview_head := View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    align: Align{y: 0.5}
                                    preview_title := Label{
                                        width: Fill
                                        height: Fit
                                        text: "Value"
                                        draw_text +: {
                                            color: theme.flow_text
                                            text_style: theme.font_bold{font_size: 11}
                                        }
                                    }
                                    preview_save := Button{text: "Save…"}
                                    preview_close := ButtonFlat{text: "Close"}
                                }
                                preview_value := mod.flow.ui.ValueView{
                                    width: Fill
                                    height: Fill
                                }
                            }
                        }
                        clear_confirm := Modal{
                            content +: {
                                width: 420
                                height: Fit
                                confirm_panel := HelpPanel{
                                    Label{
                                        width: Fill
                                        height: Fit
                                        text: "Cancel this run and clear generated output?"
                                        draw_text +: {
                                            color: theme.flow_text
                                            text_style: theme.font_bold{font_size: 11}
                                        }
                                    }
                                    Label{
                                        width: Fill
                                        height: Fit
                                        text: "Node results, streamed text and progress will be removed. Your inputs and flow file stay unchanged."
                                        draw_text +: {
                                            color: theme.flow_text_body
                                            text_style: theme.font_regular{font_size: 9.5}
                                        }
                                    }
                                    View{
                                        width: Fill
                                        height: Fit
                                        flow: Right
                                        spacing: theme.space_2
                                        align: Align{x: 1.0 y: 0.5}
                                        clear_cancel := ButtonFlat{text: "Keep running"}
                                        clear_confirm_btn := Button{text: "Cancel and clear"}
                                    }
                                }
                            }
                        }
                        image_viewer := ImageViewer{}
                    }
                }
            }
        }
    }
}

enum IoResult {
    Flows(Result<Vec<FlowSummary>, ClientError>),
    Flow {
        name: String,
        generation: u64,
        result: Result<FlowDefinition, ClientError>,
    },
    Saved {
        name: String,
        source: String,
        result: Result<PutFlowResponse, ClientError>,
    },
    Created {
        name: String,
        result: Result<PutFlowResponse, ClientError>,
    },
    GraphPut {
        name: String,
        result: Result<PutFlowResponse, ClientError>,
    },
    Catalog(Result<NodesResponse, ClientError>),
    Templates(Result<Vec<TemplateSummary>, ClientError>),
    Models {
        domain: String,
        result: Result<ModelsResponse, ClientError>,
    },
    InstanceCreated {
        flow: String,
        generation: u64,
        result: Result<CreateInstanceResponse, ClientError>,
    },
    InstanceRunStarted {
        flow: String,
        generation: u64,
        request_generation: u64,
        planned_nodes: Vec<String>,
        journal: InputJournal,
        result: Result<(String, CreateRunResponse), ClientError>,
    },
    Instances(Result<Vec<InstanceRow>, ClientError>),
    Instance {
        instance: String,
        generation: u64,
        result: Result<InstanceRow, ClientError>,
    },
    RunStarted {
        instance: String,
        generation: u64,
        request_generation: u64,
        planned_nodes: Vec<String>,
        journal: InputJournal,
        result: Result<CreateRunResponse, ClientError>,
    },
    InputsPut {
        instance: String,
        generation: u64,
        journal: InputJournal,
        result: Result<(), ClientError>,
    },
    RunSnapshot {
        instance: String,
        generation: u64,
        run_id: String,
        result: Result<RunRowDto, ClientError>,
    },
    InstanceCleared {
        instance: String,
        generation: u64,
        result: Result<(), ClientError>,
    },
    Done(Result<(), ClientError>),
}

type InputKey = (String, String);
type InputJournal = HashMap<InputKey, String>;

#[derive(Clone)]
struct RunRequest {
    outputs: Option<Vec<String>>,
}

#[derive(Default)]
struct IoMailbox {
    sender: Option<Sender<IoResult>>,
    receiver: Option<Receiver<IoResult>>,
    fetching_flows: bool,
    fetching_instances: bool,
    fetching_models: HashSet<String>,
}

impl IoMailbox {
    fn start(&mut self) {
        let (sender, receiver) = channel();
        self.sender = Some(sender);
        self.receiver = Some(receiver);
    }
}

#[derive(Clone, Debug)]
struct RunInfo {
    run_id: String,
    state: String,
    started: f64,
    finished_secs: Option<f64>,
    revision: u64,
    planned_nodes: Vec<String>,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    host: Option<FlowServer>,
    #[rust]
    testpattern_service: Option<testpattern::TestpatternService>,
    #[rust]
    session: Option<SessionConnector>,
    #[rust]
    subscriber: Option<FlowSubscriber>,
    #[rust]
    poll_timer: Timer,
    #[rust]
    io: IoMailbox,
    #[rust]
    flows: Vec<FlowSummary>,
    #[rust]
    templates: Vec<TemplateSummary>,
    #[rust]
    catalog: Vec<NodeTypeCatalog>,
    /// The hub's model ids by domain, from `GET /v1/models?domain=`.
    #[rust]
    models: HashMap<String, Vec<ModelChoice>>,
    #[rust]
    models_fetched_at: f64,
    #[rust]
    selected: Option<String>,
    #[rust]
    definition: Option<FlowDefinition>,
    #[rust]
    revisions: Vec<u64>,
    #[rust]
    redo: Vec<u64>,
    #[rust]
    instance: Option<String>,
    #[rust]
    attachment_generation: u64,
    #[rust]
    run_request_generation: u64,
    #[rust]
    run_start_in_flight: bool,
    #[rust]
    instance_row: Option<InstanceRow>,
    #[rust]
    instances: Vec<InstanceRow>,
    #[rust]
    faces: Option<FaceHost>,
    #[rust]
    values: ValueCache,
    #[rust]
    run: Option<RunInfo>,
    #[rust]
    outputs: HashMap<String, Vec<(String, ValueRef)>>,
    #[rust]
    cleared_instances: HashSet<String>,
    #[rust]
    unsaved: bool,
    #[rust]
    connected_server: Option<[u8; 16]>,
    #[rust]
    embedded: bool,
    #[rust]
    auto_opened: bool,
    /// An instance create for the open flow is in flight.
    #[rust]
    binding: bool,
    #[rust]
    warned_custom: HashSet<String>,
    #[rust]
    palette_drop: Option<((f64, f64), String, String, PortType)>,
    #[rust]
    pending_inputs: HashMap<(String, String), String>,
    #[rust]
    input_flush_in_flight: bool,
    #[rust]
    deferred_run: Option<RunRequest>,
    #[rust]
    deferred_flow_reload: bool,
    #[rust]
    pending_params: HashMap<(String, String), Literal>,
    #[rust]
    pending_graph: bool,
    #[rust]
    pending_auto_flips: HashMap<String, bool>,
    #[rust]
    app_mode: bool,
    #[rust]
    source_mode: bool,
    #[rust]
    left_hidden: bool,
    #[rust]
    wire_mode: WireMode,
    #[rust]
    preview_digest: Option<(String, String)>,
    #[rust]
    preview_bytes: Option<makepad_flow::ValueBytes>,
    #[rust]
    save_dialog_bytes: Option<makepad_flow::ValueBytes>,
    #[rust]
    save_when_ready: Option<String>,
    #[rust]
    save_task: Option<makepad_widgets::makepad_platform::thread::TaskHandle<Result<std::path::PathBuf, String>>>,
    #[rust]
    time: f64,
    #[rust]
    services: FlowServices,
    #[rust]
    current_node: Option<String>,
    #[rust]
    selected_node: Option<String>,
    #[rust]
    last_error: Option<String>,
    #[rust]
    menu_state: (bool, bool, bool, bool, bool, bool),
    #[rust]
    menu_state_initialized: bool,
    /// Splitter positions are widget state; these two slots only remember a
    /// pane's live size while that pane is collapsed by a View-menu toggle.
    #[rust]
    left_panel_align: Option<SplitterAlign>,
    #[rust]
    source_panel_align: Option<SplitterAlign>,
}

impl App {
    fn startup(&mut self, cx: &mut Cx) {
        self.io.start();
        self.poll_timer = cx.start_interval(0.25);
        let root = default_root();
        let policy = EmbedPolicy::from_env();
        let (hint, token) = match resolve(policy, &root, None) {
            Resolved::Host => {
                let mut config = FlowServerConfig::new(root.clone());
                config.control_addr = "127.0.0.1:0".to_string();
                config.data_addr = "127.0.0.1:0".to_string();
                // Dev-only: `FLOW_GEN_BASE_URL=<url>` points every gen node at
                // one hub box instead of the fleet; `=testpattern` serves the
                // hub's testpattern model in-process and streams a stand-in
                // paragraph for the Llm nodes (see `testpattern.rs`).
                if let Some(value) = std::env::var_os("FLOW_GEN_BASE_URL")
                    .and_then(|value| value.into_string().ok())
                    .filter(|value| !value.is_empty())
                {
                    let seams = if value == "testpattern" {
                        match testpattern::start_service() {
                            Ok(service) => {
                                let url = service.url.clone();
                                self.testpattern_service = Some(service);
                                log!("flow-ui: testpattern hub service at {url} — gen and chat are stand-ins");
                                Some(Seams {
                                    chat: Arc::new(testpattern::TestpatternChat),
                                    gen: Arc::new(FixedGen(url)),
                                    http: Arc::new(HubHttp),
                                })
                            }
                            Err(error) => {
                                self.set_error(cx, &format!("testpattern service: {error}"));
                                None
                            }
                        }
                    } else {
                        log!("flow-ui: FLOW_GEN_BASE_URL={value} — gen nodes use that box");
                        Some(Seams {
                            chat: Arc::new(HubChat::from_env()),
                            gen: Arc::new(FixedGen(value)),
                            http: Arc::new(HubHttp),
                        })
                    };
                    if let Some(seams) = seams {
                        config = config.with_seams(seams);
                    }
                }
                match FlowServer::start(config) {
                    Ok(server) => {
                        let served = server.endpoints();
                        let hint = Some(makepad_flow::client::Endpoints {
                            control: served.control,
                            data: served.data,
                        });
                        let token = Some(served.token.clone());
                        self.host = Some(server);
                        self.embedded = true;
                        (hint, token)
                    }
                    Err(error) => {
                        self.set_error(cx, &format!("Could not host flow server: {error}"));
                        (None, None)
                    }
                }
            }
            Resolved::Attach(hint, token, _) => (hint, token),
        };
        self.session = Some(SessionConnector::start(SessionConfig {
            hint,
            root: Some(root),
            token,
            ..SessionConfig::default()
        }));
        self.update_connection(cx);
        self.set_wire_mode(cx, self.wire_mode);
        self.set_modes(cx);
    }

    fn set_wire_mode(&mut self, cx: &mut Cx, mode: WireMode) {
        self.wire_mode = mode;
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_wire_mode(cx, mode);
        }

        let menu = self.ui.menu_bar(cx, ids!(menu_bar));
        let Some(inner) = menu.borrow() else {
            return;
        };
        let mut defs = inner.menus().to_vec();
        drop(inner);
        set_wire_menu_checks(&mut defs, mode);
        menu.set_menus(cx, defs);
    }

    fn client(&self) -> Option<Arc<Mutex<FlowClient>>> {
        self.session.as_ref().and_then(|session| session.client())
    }

    fn run_is_active(&self) -> bool {
        self.run.as_ref().is_some_and(|run| {
            matches!(run.state.as_str(), "queued" | "running" | "waiting")
        })
    }

    fn display_run(&mut self, cx: &mut Cx, run: RunInfo) {
        if let Some(instance) = self.instance.as_ref() {
            self.cleared_instances.remove(instance);
        }
        let changed = self.run.as_ref().map(|old| old.run_id.as_str())
            != Some(run.run_id.as_str());
        self.run = Some(run);
        if changed {
            self.current_node = None;
            self.outputs.clear();
            if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                canvas.clear_run(cx);
            }
            if let Some(faces) = self.faces.as_mut() {
                faces.reset_run();
            }
        }
        self.update_run_bar(cx);
    }

    fn detach_faces(&mut self, cx: &mut Cx) {
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_roots(cx, Vec::new());
        }
        if let Some(faces) = self.faces.take() {
            faces.free(cx);
        }
    }

    fn clear_attachment(&mut self, cx: &mut Cx) {
        self.detach_faces(cx);
        self.close_preview(cx);
        self.attachment_generation = self.attachment_generation.wrapping_add(1);
        self.run_request_generation = self.run_request_generation.wrapping_add(1);
        self.run_start_in_flight = false;
        self.instance = None;
        self.instance_row = None;
        self.run = None;
        self.outputs.clear();
        self.current_node = None;
        self.pending_inputs.clear();
        self.input_flush_in_flight = false;
        self.deferred_run = None;
    }

    /// The sole instance ownership transition. Old isolate roots are removed
    /// before its VM is freed; only a known matching definition may remount.
    fn attach_instance(&mut self, cx: &mut Cx, instance: Option<String>, mount: bool) {
        if self.instance == instance {
            if mount && self.faces.is_none() {
                self.remount_faces(cx);
            }
            return;
        }
        self.clear_attachment(cx);
        self.instance = instance;
        if mount && self.instance.is_some() && self.definition.is_some() {
            self.remount_faces(cx);
        }
    }

    fn focus_instance(&mut self, cx: &mut Cx, instance: String, flow: Option<String>) {
        if flow.as_deref() == self.selected.as_deref() && self.definition.is_some() {
            self.attach_instance(cx, Some(instance), true);
        } else {
            self.clear_attachment(cx);
            self.instance = Some(instance);
            self.definition = None;
            if let Some(flow) = flow {
                self.selected = Some(flow.clone());
                self.unsaved = false;
                self.revisions.clear();
                self.redo.clear();
                self.ui.label(cx, ids!(flow_name)).set_text(cx, &flow);
                self.show_flow_list(cx);
                self.load_flow(flow);
            }
        }
        self.fetch_instance();
        self.refresh_instances();
    }

    /// Run one client call on a worker thread; the result lands in the
    /// mailbox and wakes the UI.
    fn io<F>(&self, f: F)
    where
        F: FnOnce(&FlowClient) -> IoResult + Send + 'static,
    {
        let (Some(client), Some(sender)) = (self.client(), self.io.sender.clone()) else {
            return;
        };
        std::thread::spawn(move || {
            let result = match client.lock() {
                Ok(client) => f(&client),
                Err(_) => IoResult::Done(Err(ClientError::Protocol(
                    "flow client lock poisoned".into(),
                ))),
            };
            let _ = sender.send(result);
            SignalToUI::set_ui_signal();
        });
    }

    fn update_connection(&mut self, cx: &mut Cx) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let status = session.status();
        let (text, color) = match &status {
            SessionStatus::Discovering => ("Discovering".to_string(), theme::state_color("cancelled")),
            SessionStatus::Connecting { .. } => ("Connecting…".to_string(), theme::state_color("waiting")),
            SessionStatus::Retrying { in_secs, .. } => {
                (format!("Retrying in {in_secs} s"), theme::state_color("failed"))
            }
            SessionStatus::Connected { .. } => (
                format!(
                    "{} · r{}",
                    if self.embedded { "embedded" } else { "attached" },
                    self.definition
                        .as_ref()
                        .map(|definition| definition.revision)
                        .unwrap_or(0)
                ),
                theme::state_color("done"),
            ),
        };
        self.ui.label(cx, ids!(status_chip)).set_text(cx, &text);
        let mut dot = self.ui.view(cx, ids!(status_dot));
        script_apply_eval!(cx, dot, {draw_bg +: {color: #(color)}});
        if let SessionStatus::Connected { server_id, .. } = status {
            if self.connected_server != Some(server_id) {
                self.connected_server = Some(server_id);
                if let Some(client) = session.client() {
                    self.services.connect(cx, client.clone());
                    self.subscriber =
                        FlowSubscriber::start(client, FlowSubscriberConfig::default()).ok();
                    self.refresh_flows();
                    self.io(|client| IoResult::Catalog(client.nodes_catalog()));
                    self.io(|client| IoResult::Templates(client.templates()));
                    self.refresh_instances();
                }
            }
        } else if self.connected_server.take().is_some() {
            self.services.disconnect();
            self.subscriber = None;
        }
    }

    fn refresh_flows(&mut self) {
        if self.io.fetching_flows || self.client().is_none() {
            return;
        }
        self.io.fetching_flows = true;
        self.io(|client| IoResult::Flows(client.flows()));
    }

    fn refresh_instances(&mut self) {
        if self.io.fetching_instances || self.client().is_none() {
            return;
        }
        self.io.fetching_instances = true;
        self.io(|client| IoResult::Instances(client.instances(None, false)));
    }

    /// The hub's models for every domain the open graph uses (the pickers
    /// in the faces and the inspector fill from them).
    fn refresh_models(&mut self, force: bool) {
        let Some(graph) = self.current_graph() else {
            return;
        };
        if !force && self.time - self.models_fetched_at < MODELS_REFRESH_SECS {
            return;
        }
        let mut domains: Vec<String> = graph
            .nodes
            .iter()
            .filter_map(|node| node.domain.clone())
            .collect();
        domains.sort();
        domains.dedup();
        if domains.is_empty() {
            return;
        }
        self.models_fetched_at = self.time;
        for domain in domains {
            if !self.io.fetching_models.insert(domain.clone()) {
                continue;
            }
            self.io(move |client| {
                let result = client.models(Some(&domain));
                IoResult::Models { domain, result }
            });
        }
    }

    fn fetch_instance(&mut self) {
        let Some(id) = self.instance.clone() else {
            return;
        };
        let generation = self.attachment_generation;
        self.io(move |client| IoResult::Instance {
            instance: id.clone(),
            generation,
            result: client.instance(&id),
        });
    }

    fn load_flow(&mut self, name: String) {
        let generation = self.attachment_generation;
        self.io(move |client| {
            let result = client.flow(&name);
            IoResult::Flow {
                name,
                generation,
                result,
            }
        });
    }

    fn save_source(&mut self, name: String, source: String) {
        self.io(move |client| {
            let result = client.put_source(&name, &source);
            IoResult::Saved {
                name,
                source,
                result,
            }
        });
    }

    fn create_from_template(&mut self, template: &str) {
        let name = self.fresh_flow_name(template);
        let template = template.to_string();
        self.io(move |client| {
            let result = client.create_from_template(&name, &template);
            IoResult::Created { name, result }
        });
    }

    fn open_flow(&mut self, cx: &mut Cx, name: String) {
        self.clear_attachment(cx);
        self.selected = Some(name.clone());
        self.definition = None;
        self.unsaved = false;
        self.deferred_flow_reload = false;
        self.pending_auto_flips.clear();
        self.revisions.clear();
        self.redo.clear();
        self.ui.label(cx, ids!(flow_name)).set_text(cx, &name);
        self.ui.label(cx, ids!(instance_chip)).set_text(cx, "");
        self.set_error(cx, "");
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
            canvas.reset_view(cx);
        }
        self.update_run_bar(cx);
        self.show_flow_list(cx);
        self.load_flow(name);
    }

    /// Bind the open flow to an instance: the newest live one of its own,
    /// else a fresh one — so the faces fill and Run is one click.
    fn bind_instance(&mut self, cx: &mut Cx) {
        if self.instance.is_some() || self.binding {
            return;
        }
        let Some(flow) = self.selected.clone() else {
            return;
        };
        let existing = self
            .instances
            .iter()
            .filter(|row| row.flow == flow && row.live && row.owner == "tab")
            .max_by_key(|row| row.last_activity_ms)
            .map(|row| row.instance.clone());
        if let Some(id) = existing {
            self.attach_instance(cx, Some(id), true);
            self.fetch_instance();
            return;
        }
        self.binding = true;
        let generation = self.attachment_generation;
        self.io(move |client| {
            let result = client.create_instance(&flow, &CreateInstanceRequest::default());
            IoResult::InstanceCreated {
                flow,
                generation,
                result,
            }
        });
    }

    /// Nothing open yet: the newest live instance's flow, else the
    /// prompt-to-image flow, else the first flow; an empty root gets the
    /// prompt-to-image template.
    fn maybe_auto_open(&mut self, cx: &mut Cx) {
        if self.auto_opened || self.selected.is_some() || self.io.fetching_flows || self.io.fetching_instances {
            return;
        }
        self.auto_opened = true;
        if self.flows.is_empty() {
            self.create_from_template(FIRST_TEMPLATE);
            return;
        }
        let from_instance = self
            .instances
            .iter()
            .filter(|row| row.live && self.flows.iter().any(|flow| flow.name == row.flow))
            .max_by_key(|row| row.last_activity_ms)
            .map(|row| row.flow.clone());
        let name = from_instance
            .or_else(|| {
                self.flows
                    .iter()
                    .find(|flow| flow.name.starts_with(FIRST_TEMPLATE))
                    .map(|flow| flow.name.clone())
            })
            .or_else(|| self.flows.first().map(|flow| flow.name.clone()));
        if let Some(name) = name {
            self.open_flow(cx, name);
        }
    }

    /// Every canvas / inspector / face edit ends here: the new graph goes to
    /// the server, the file is rewritten, and `flow.changed` redraws.
    fn put_graph(&mut self, cx: &mut Cx, graph: Graph) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        self.pending_auto_flips.clear();
        let canonical = self
            .flows
            .iter()
            .find(|flow| flow.name == name)
            .map(|flow| flow.canonical)
            .unwrap_or(true);
        if !canonical && self.warned_custom.insert(name.clone()) {
            self.set_error(
                cx,
                "this flow has logic outside its nodes; canvas edits rewrite the file from the graph",
            );
        }
        let remount = self
            .definition
            .as_ref()
            .and_then(|definition| definition.graph.as_ref())
            .is_none_or(|old| graph_edit::needs_face_remount(old, &graph));
        if let Some(definition) = self.definition.as_mut() {
            // The canvas draws the edit at once; the server's re-evaluation
            // confirms or corrects it through flow.changed.
            definition.graph = Some(graph.clone());
        }
        self.show_graph(cx);
        if remount {
            self.remount_faces(cx);
        }
        self.pending_graph = true;
        self.redo.clear();
        self.io(move |client| {
            let result = client.put_graph(&name, &graph);
            IoResult::GraphPut { name, result }
        });
    }

    fn current_graph(&self) -> Option<Graph> {
        self.definition
            .as_ref()
            .and_then(|definition| definition.graph.clone())
    }

    fn apply_edit(&mut self, cx: &mut Cx, edit: CanvasEdit) {
        let Some(graph) = self.current_graph() else {
            return;
        };
        let next = match edit {
            CanvasEdit::Move { node, at } => graph_edit::move_node(&graph, &node, at),
            CanvasEdit::Resize { node, size } => graph_edit::resize_node(&graph, &node, size),
            CanvasEdit::Flip { node } => {
                let Some(flip) = graph
                    .nodes
                    .iter()
                    .find(|candidate| candidate.id == node)
                    .map(|candidate| !candidate.flip)
                else {
                    return;
                };
                self.with_canvas(cx, |_, canvas| canvas.lock_flip(&node));
                graph_edit::flip_node(&graph, &node, flip)
            }
            CanvasEdit::Connect {
                from_node,
                from_port,
                to_node,
                to_port,
            } => match graph_edit::connect(&graph, &from_node, &from_port, &to_node, &to_port) {
                Ok(next) => next,
                Err(error) => {
                    self.set_error(cx, &error);
                    return;
                }
            },
            CanvasEdit::Disconnect { to_node, to_port } => {
                graph_edit::disconnect(&graph, &to_node, &to_port)
            }
            CanvasEdit::Delete { node } => graph_edit::delete_node(&graph, &node),
            CanvasEdit::AddType { type_name, at } => {
                let Some(entry) = self.catalog.iter().find(|entry| entry.type_name == type_name)
                else {
                    return;
                };
                graph_edit::add_node(&graph, entry, at).0
            }
        };
        self.put_graph(cx, next);
    }

    fn duplicate_selected(&mut self, cx: &mut Cx) {
        let (Some(graph), Some(selected)) = (self.current_graph(), self.selected_node.clone()) else {
            return;
        };
        let Some(node) = graph.nodes.iter().find(|node| node.id == selected).cloned() else {
            return;
        };
        let Some(entry) = self.catalog.iter().find(|entry| entry.type_name == node.type_name) else {
            return;
        };
        let at = node.at.unwrap_or(FIRST_AT);
        let (mut next, id) = graph_edit::add_node(&graph, entry, (at.0 + 40.0, at.1 + 60.0));
        if let Some(copy) = next.nodes.iter_mut().find(|n| n.id == id) {
            copy.params = node.params.clone();
            copy.fn_src = node.fn_src.clone();
            copy.face_src = node.face_src.clone();
            copy.size = node.size;
            copy.flip = node.flip;
        }
        self.put_graph(cx, next);
    }

    fn drain_io(&mut self, cx: &mut Cx) {
        loop {
            let next = match self.io.receiver.as_ref().map(Receiver::try_recv) {
                Some(Ok(result)) => result,
                Some(Err(TryRecvError::Empty | TryRecvError::Disconnected)) | None => break,
            };
            match next {
                IoResult::Flows(result) => {
                    self.io.fetching_flows = false;
                    match result {
                        Ok(flows) => {
                            self.flows = flows;
                            self.show_flow_list(cx);
                            self.update_connection(cx);
                            self.maybe_auto_open(cx);
                        }
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::Flow {
                    name,
                    generation,
                    result,
                } => {
                    if self.selected.as_deref() != Some(&name)
                        || generation != self.attachment_generation
                    {
                        continue;
                    }
                    match result {
                        Ok(definition) => {
                            self.show_flow(cx, definition);
                            self.bind_instance(cx);
                            self.refresh_models(true);
                        }
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::Saved {
                    name,
                    source,
                    result,
                } => match result {
                    Ok(_) => {
                        if self.selected.as_deref() == Some(&name)
                            && self.ui.text_input(cx, ids!(source)).text() == source
                        {
                            self.unsaved = false;
                        }
                        if self.selected.as_deref() != Some(&name) {
                            self.open_flow(cx, name.clone());
                        }
                        self.set_error(cx, "");
                        self.refresh_flows();
                        self.services.refresh_definitions();
                        self.load_flow(name);
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::Created { name, result } => match result {
                    Ok(_) => {
                        self.refresh_flows();
                        self.services.refresh_definitions();
                        self.open_flow(cx, name);
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::GraphPut { name, result } => {
                    self.pending_graph = false;
                    match result {
                        Ok(mut response) => {
                            for (id, flip) in &self.pending_auto_flips {
                                if let Some(node) =
                                    response.graph.nodes.iter_mut().find(|node| node.id == *id)
                                {
                                    node.flip = *flip;
                                }
                            }
                            if self.selected.as_deref() == Some(&name) {
                                let remount = self
                                    .definition
                                    .as_ref()
                                    .and_then(|definition| definition.graph.as_ref())
                                    .is_none_or(|old| {
                                        graph_edit::needs_face_remount(old, &response.graph)
                                    });
                                if let Some(definition) = self.definition.as_mut() {
                                    definition.revision = response.revision;
                                    definition.graph = Some(response.graph);
                                    definition.error = None;
                                }
                                self.push_revision(response.revision);
                                self.show_graph(cx);
                                if remount {
                                    self.remount_faces(cx);
                                }
                            }
                            self.refresh_flows();
                            self.services.refresh_definitions();
                        }
                        Err(error) => {
                            self.show_error(cx, &error);
                            if let Some(name) = self.selected.clone() {
                                self.load_flow(name);
                            }
                        }
                    }
                }
                IoResult::Catalog(result) => match result {
                    Ok(catalog) => {
                        self.catalog = catalog.types;
                        if let Some(mut palette) =
                            self.ui.widget(cx, ids!(palette)).borrow_mut::<Palette>()
                        {
                            palette.set_types(cx, &self.catalog, false);
                        }
                        if self.definition.is_some() {
                            self.remount_faces(cx);
                        }
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::Templates(result) => match result {
                    Ok(templates) => {
                        self.templates = templates;
                        if let Some(mut picker) = self
                            .ui
                            .widget(cx, ids!(template_picker))
                            .borrow_mut::<TemplatePicker>()
                        {
                            picker.set_templates(cx, self.templates.clone());
                        }
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::Models { domain, result } => {
                    self.io.fetching_models.remove(&domain);
                    match result {
                        Ok(response) => {
                            self.models.insert(domain, model_choices(&response));
                            self.push_models(cx);
                        }
                        Err(error) => {
                            // Preserve the last good choices; a first failure leaves
                            // the picker with its built-in "hub picks" entry.
                            log!("flow-ui: models for {domain}: {error}");
                        }
                    }
                }
                IoResult::InstanceCreated {
                    flow,
                    generation,
                    result,
                } => {
                    self.binding = false;
                    if self.selected.as_deref() != Some(&flow)
                        || generation != self.attachment_generation
                    {
                        continue;
                    }
                    match result {
                        Ok(response) => {
                            self.attach_instance(cx, Some(response.instance), true);
                            self.fetch_instance();
                            self.refresh_instances();
                        }
                        Err(error) => {
                            self.set_error(cx, &format!("no instance: {error}"));
                        }
                    }
                }
                IoResult::InstanceRunStarted {
                    flow,
                    generation,
                    request_generation,
                    planned_nodes,
                    journal,
                    result,
                } => {
                    if self.selected.as_deref() != Some(&flow)
                        || generation != self.attachment_generation
                    {
                        continue;
                    }
                    if !request_generation_matches(
                        self.run_request_generation,
                        request_generation,
                    ) {
                        if result.is_err() {
                            merge_failed_input_journal(&mut self.pending_inputs, journal);
                        }
                        continue;
                    }
                    self.run_start_in_flight = false;
                    let deferred = self.deferred_run.take();
                    match result {
                        Ok((instance, response)) => {
                            // Edits made while instance creation was in flight
                            // belong to the new instance, not to the empty
                            // attachment that is about to be cleared.
                            let newer_inputs = std::mem::take(&mut self.pending_inputs);
                            self.attach_instance(cx, Some(instance), true);
                            self.pending_inputs = newer_inputs;
                            self.display_run(cx, RunInfo {
                                run_id: response.run_id,
                                state: if response.queued == 0 {
                                    "running".into()
                                } else {
                                    "queued".into()
                                },
                                started: self.time,
                                finished_secs: None,
                                revision: self
                                    .definition
                                    .as_ref()
                                    .map_or(0, |definition| definition.revision),
                                planned_nodes,
                            });
                            self.fetch_instance();
                            self.fetch_run_snapshot();
                            self.refresh_instances();
                        }
                        Err(error) => {
                            merge_failed_input_journal(&mut self.pending_inputs, journal);
                            self.show_error(cx, &error);
                        }
                    }
                    if let Some(request) = deferred {
                        self.start_run(request.outputs);
                    }
                }
                IoResult::Instances(result) => {
                    self.io.fetching_instances = false;
                    if let Ok(mut rows) = result {
                        for row in &mut rows {
                            if self.cleared_instances.contains(&row.instance) && row.state == "idle" {
                                row.state = "idle · cleared".to_string();
                            } else if row.state != "idle" && row.state != "idle · cleared" {
                                self.cleared_instances.remove(&row.instance);
                            }
                        }
                        self.instances = rows.clone();
                        if let Some(mut list) =
                            self.ui.widget(cx, ids!(running)).borrow_mut::<RunningList>()
                        {
                            list.set_rows(cx, rows, self.instance.clone());
                        }
                        self.refresh_running_thumbnails(cx);
                        self.maybe_auto_open(cx);
                    }
                }
                IoResult::Instance {
                    instance,
                    generation,
                    result,
                } => {
                    if !attachment_matches(
                        self.instance.as_deref(),
                        self.attachment_generation,
                        &instance,
                        generation,
                    ) {
                        continue;
                    }
                    match result {
                    Ok(row) => {
                        if row.instance == instance {
                            if self.selected.as_deref() != Some(row.flow.as_str()) {
                                let instance = row.instance.clone();
                                self.focus_instance(cx, instance, Some(row.flow.clone()));
                            }
                            let chip = format!(
                                "{} · {}{}",
                                row.label
                                    .clone()
                                    .unwrap_or_else(|| row.instance.chars().take(8).collect()),
                                row.state,
                                if self.cleared_instances.contains(&row.instance) {
                                    " · cleared"
                                } else {
                                    ""
                                }
                            );
                            self.ui.label(cx, ids!(instance_chip)).set_text(cx, &chip);
                            let recover_run = !self.run_is_active()
                                && row.run.as_ref().is_some_and(|run_id| {
                                    self.run.as_ref().map(|run| run.run_id.as_str())
                                        != Some(run_id.as_str())
                                });
                            if recover_run {
                                self.run = Some(RunInfo {
                                    run_id: row.run.clone().unwrap(),
                                    state: row.state.clone(),
                                    started: self.time,
                                    finished_secs: None,
                                    revision: row.revision,
                                    planned_nodes: Vec::new(),
                                });
                            }
                            if let Some(faces) = self.faces.as_mut() {
                                faces.fill_inputs(cx, &row);
                            }
                            let completed_outputs: Vec<(String, ValueRef)> = row
                                .outputs
                                .iter()
                                .map(|(node, value)| (node.clone(), value.clone()))
                                .collect();
                            for (node, value) in completed_outputs {
                                let port = self.output_face_port(&node);
                                self.record_value(cx, &node, &port, value);
                            }
                            self.request_wanted_values(cx);
                            if let Some(waiting) = row
                                .waiting
                                .as_ref()
                                .filter(|_| row.state == "waiting")
                                .map(|waiting| waiting.node.clone())
                            {
                                if let Some(mut app_view) =
                                    self.ui.widget(cx, ids!(app_view)).borrow_mut::<AppView>()
                                {
                                    app_view.waiting = Some(waiting);
                                }
                            }
                            self.instance_row = Some(row);
                            if recover_run {
                                self.fetch_run_snapshot();
                            }
                            self.update_run_bar(cx);
                        }
                    }
                    Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::RunStarted {
                    instance,
                    generation,
                    request_generation,
                    planned_nodes,
                    journal,
                    result,
                } => {
                    if !attachment_matches(
                        self.instance.as_deref(),
                        self.attachment_generation,
                        &instance,
                        generation,
                    ) {
                        continue;
                    }
                    if !request_generation_matches(
                        self.run_request_generation,
                        request_generation,
                    ) {
                        if result.is_err() {
                            merge_failed_input_journal(&mut self.pending_inputs, journal);
                        }
                        continue;
                    }
                    self.run_start_in_flight = false;
                    let deferred = self.deferred_run.take();
                    match result {
                        Ok(response) => {
                            self.display_run(cx, RunInfo {
                                run_id: response.run_id,
                                state: if response.queued == 0 {
                                    "running".into()
                                } else {
                                    "queued".into()
                                },
                                started: self.time,
                                finished_secs: None,
                                revision: self
                                    .definition
                                    .as_ref()
                                    .map_or(0, |definition| definition.revision),
                                planned_nodes,
                            });
                            self.fetch_run_snapshot();
                        }
                        Err(error) => {
                            merge_failed_input_journal(&mut self.pending_inputs, journal);
                            self.show_error(cx, &error);
                        }
                    }
                    if let Some(request) = deferred {
                        self.start_run(request.outputs);
                    }
                }
                IoResult::InputsPut {
                    instance,
                    generation,
                    journal,
                    result,
                } => {
                    if !attachment_matches(
                        self.instance.as_deref(),
                        self.attachment_generation,
                        &instance,
                        generation,
                    )
                    {
                        continue;
                    }
                    self.input_flush_in_flight = false;
                    match result {
                        Ok(()) => {
                            if let Some(request) = self.deferred_run.take() {
                                self.start_run(request.outputs);
                            }
                        }
                        Err(error) => {
                            merge_failed_input_journal(&mut self.pending_inputs, journal);
                            self.show_error(cx, &error);
                        }
                    }
                }
                IoResult::RunSnapshot {
                    instance,
                    generation,
                    run_id,
                    result,
                } => {
                    if !attachment_matches(
                        self.instance.as_deref(),
                        self.attachment_generation,
                        &instance,
                        generation,
                    )
                        || self.run.as_ref().map(|run| run.run_id.as_str())
                            != Some(run_id.as_str())
                    {
                        continue;
                    }
                    match result {
                        Ok(row) => self.apply_run_row(cx, row),
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::InstanceCleared {
                    instance,
                    generation,
                    result,
                } => {
                    if !attachment_matches(
                        self.instance.as_deref(),
                        self.attachment_generation,
                        &instance,
                        generation,
                    ) {
                        continue;
                    }
                    match result {
                        Ok(()) => self.apply_instance_cleared(cx, &instance),
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::Done(result) => {
                    if let Err(error) = result {
                        self.show_error(cx, &error);
                    }
                }
            }
        }
    }

    fn push_revision(&mut self, revision: u64) {
        if self.revisions.last() != Some(&revision) {
            self.revisions.push(revision);
            if self.revisions.len() > 32 {
                self.revisions.remove(0);
            }
        }
    }

    fn show_flow_list(&mut self, cx: &mut Cx) {
        if let Some(mut list) = self.ui.widget(cx, ids!(flow_list)).borrow_mut::<FlowList>() {
            list.set_rows(cx, self.flows.clone(), self.selected.clone());
        }
    }

    fn show_flow(&mut self, cx: &mut Cx, mut definition: FlowDefinition) {
        if let Some(graph) = definition.graph.as_mut() {
            for (id, flip) in &self.pending_auto_flips {
                if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == *id) {
                    node.flip = *flip;
                }
            }
        }
        if !self.unsaved {
            self.ui
                .text_input(cx, ids!(source))
                .set_text(cx, &definition.source);
        }
        self.set_error(
            cx,
            &definition
                .error
                .as_ref()
                .map(format_eval_error)
                .unwrap_or_default(),
        );
        self.last_error = definition.error.as_ref().map(format_eval_error);
        self.push_revision(definition.revision);
        let remount = self
            .definition
            .as_ref()
            .and_then(|old| old.graph.as_ref())
            .zip(definition.graph.as_ref())
            .is_none_or(|(old, new)| graph_edit::needs_face_remount(old, new));
        self.definition = Some(definition);
        self.show_graph(cx);
        if remount || self.faces.is_none() {
            self.remount_faces(cx);
        }
        self.update_connection(cx);
    }

    /// Push the current graph into the canvas, the app view and the inspector.
    fn show_graph(&mut self, cx: &mut Cx) {
        let graph = self.current_graph();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_compatible_ports(
                graph
                    .as_ref()
                    .map(graph_view::compatibility_of)
                    .unwrap_or_default(),
            );
            canvas.set_graph(cx, graph.as_ref().map(graph_view::view_of));
        }
        if let Some(mut app_view) = self.ui.widget(cx, ids!(app_view)).borrow_mut::<AppView>() {
            app_view.set_graph(cx, graph.clone());
        }
        if let Some(faces) = self.faces.as_mut() {
            if let Some(graph) = graph.as_ref() {
                faces.refresh_params(cx, graph);
            }
        }
        self.refresh_inspector(cx);
    }

    fn refresh_inspector(&mut self, cx: &mut Cx) {
        let selection = self
            .ui
            .widget(cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .and_then(|canvas| canvas.selection().cloned());
        let selected = selection
            .as_ref()
            .and_then(Selection::node)
            .map(str::to_string);
        let outputs = selected
            .as_ref()
            .and_then(|node| self.outputs.get(node).cloned())
            .unwrap_or_default();
        let graph = self.current_graph();
        let domain = selected
            .as_ref()
            .and_then(|id| graph.as_ref()?.nodes.iter().find(|node| &node.id == id)?.domain.clone());
        let models = domain
            .as_ref()
            .and_then(|domain| self.models.get(domain).cloned())
            .unwrap_or_default();
        let loaded: Vec<(String, makepad_flow::ValueBytes)> = outputs
            .iter()
            .filter_map(|(port, value)| {
                self.values
                    .get(&value.digest)
                    .map(|bytes| (port.clone(), bytes))
            })
            .collect();
        let source = self
            .definition
            .as_ref()
            .map(|definition| definition.source.clone());
        if let Some(mut inspector) = self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>() {
            inspector.set_models(cx, models);
            match selection.as_ref() {
                Some(Selection::Edge {
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                }) => inspector.show_edge(
                    cx,
                    graph.as_ref(),
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                ),
                _ => inspector.show_node(
                    cx,
                    graph.as_ref(),
                    &self.catalog,
                    selected.as_deref(),
                    &outputs,
                    &loaded,
                    source.as_deref(),
                ),
            }
        }
        if let Some(client) = self.client() {
            for (_, value) in &outputs {
                if !self.values.contains(&value.digest) {
                    self.values.request(&value.digest, client.clone());
                }
            }
        }
        if domain.is_some() {
            self.refresh_models(false);
        }
    }

    /// The hub's lists reach every picker: the faces' and the inspector's.
    fn push_models(&mut self, cx: &mut Cx) {
        let Some(graph) = self.current_graph() else {
            return;
        };
        if let Some(faces) = self.faces.as_mut() {
            for node in &graph.nodes {
                let Some(domain) = node.domain.as_ref() else {
                    continue;
                };
                if let Some(models) = self.models.get(domain) {
                    faces.set_models(cx, &node.id, models);
                }
            }
        }
        self.refresh_inspector(cx);
    }

    /// Free the instance's isolate and evaluate the file again into a new one.
    fn remount_faces(&mut self, cx: &mut Cx) {
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_roots(cx, Vec::new());
        }
        if let Some(faces) = self.faces.take() {
            faces.free(cx);
        }
        let (Some(definition), Some(name)) = (self.definition.as_ref(), self.selected.as_ref())
        else {
            return;
        };
        let Some(graph) = definition.graph.as_ref() else {
            return;
        };
        if self.catalog.is_empty() {
            return;
        }
        let parent = self
            .ui
            .widget(cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .map(|canvas| canvas.widget_uid())
            .unwrap_or(WidgetUid(0));
        let instance = self.instance.clone().unwrap_or_else(|| "unbound".to_string());
        let mut graph = graph.clone();
        graph_edit::auto_place(&mut graph);
        let mut faces = FaceHost::mount(
            cx,
            parent,
            &instance,
            &format!("{name}.splash"),
            &definition.source,
            &graph,
            &self.catalog,
        );
        // A face error shows once in the strip and in the owning card only.
        if let Some(error) = faces.error.as_ref() {
            self.set_error(cx, &format!("faces: {error}"));
        }
        let face_errors = faces.face_errors(&graph);
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_errors(cx, face_errors);
        }
        if let Some(row) = self.instance_row.as_ref() {
            faces.fill_inputs(cx, row);
        }
        // Outputs the run already produced land in the fresh faces too.
        let outputs = self.outputs.clone();
        for (node, ports) in &outputs {
            for (port, value) in ports {
                let bytes = self.values.get(&value.digest);
                faces.push_value(cx, node, port, value, bytes.as_ref());
            }
        }
        let roots: Vec<(LiveId, WidgetRef)> = faces
            .faces
            .iter()
            .map(|(id, face)| (LiveId::from_str(id), face.root.clone()))
            .chain(
                faces
                    .flow_face
                    .iter()
                    .map(|face| (live_id!(flow_face), face.root.clone())),
            )
            .filter(|(_, root)| !root.is_empty())
            .collect();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_roots(cx, roots);
        }
        self.faces = Some(faces);
        self.push_models(cx);
        self.refresh_models(true);
        self.request_wanted_values(cx);
        self.ui.redraw(cx);
    }

    /// Fetch the bytes the faces asked for (image previews); a digest that
    /// is already cached is delivered on the spot.
    fn request_wanted_values(&mut self, cx: &mut Cx) {
        let Some(client) = self.client() else {
            return;
        };
        let Some(faces) = self.faces.as_mut() else {
            return;
        };
        let wanted: Vec<String> = faces.wanted.clone();
        for digest in wanted {
            if self.values.contains(&digest) {
                faces.deliver_bytes(cx, &mut self.values, &digest);
                continue;
            }
            self.values.request(&digest, client.clone());
        }
    }

    fn set_error(&mut self, cx: &mut Cx, text: &str) {
        self.last_error = (!text.is_empty()).then(|| text.to_string());
        self.ui.label(cx, ids!(error_label)).set_text(cx, text);
    }

    fn show_error(&mut self, cx: &mut Cx, error: &ClientError) {
        let text = match error {
            ClientError::Eval(error) => format_eval_error(error),
            other => other.to_string(),
        };
        self.set_error(cx, &text);
    }

    /// Exact §3 progress: `(done + running_fraction) / planned_nodes`.
    /// Terminal color is selected independently by `RunBar` from `state`.
    fn update_run_bar(&mut self, cx: &mut Cx) {
        let (fraction, done, total) = {
            let canvas = self.ui.widget(cx, ids!(canvas));
            let canvas = canvas.borrow::<FlowCanvas>();
            self.run
                .as_ref()
                .zip(canvas.as_ref())
                .map_or((0.0, 0, 0), |(run, canvas)| {
                    total_progress(&run.planned_nodes, &canvas.statuses)
                })
        };
        let (state, text) = match (&self.run, &self.instance_row) {
            (Some(run), _) => {
                let secs = run
                    .finished_secs
                    .unwrap_or_else(|| (self.time - run.started).max(0.0));
                (
                    run.state.clone(),
                    format!("{} · {:.1} s · {done}/{total} nodes", run.state, secs),
                )
            }
            (None, Some(row)) => (String::new(), row.state.clone()),
            (None, None) => (String::new(), String::new()),
        };
        self.ui.label(cx, ids!(run_state)).set_text(cx, &text);
        if let Some(mut bar) = self.ui.widget(cx, ids!(run_bar)).borrow_mut::<RunBar>() {
            bar.set_progress(cx, fraction, &state);
        }
        self.update_menu_state(cx);
    }

    fn update_menu_state(&mut self, cx: &mut Cx) {
        let running = self.run_is_active();
        let has_selected_node = self.selected_node.is_some();
        let has_selection = self
            .ui
            .widget(cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .is_some_and(|canvas| canvas.selection().is_some());
        let can_undo = self.revisions.len() >= 2;
        let can_redo = !self.redo.is_empty();
        let has_instance = self.instance.is_some();
        let next = (
            running,
            has_selected_node,
            has_selection,
            can_undo,
            can_redo,
            has_instance,
        );
        if self.menu_state_initialized && next == self.menu_state {
            return;
        }
        self.menu_state_initialized = true;
        self.menu_state = next;
        let menu = self.ui.menu_bar(cx, ids!(menu_bar));
        menu.set_enabled(cx, live_id!(cancel), running);
        menu.set_enabled(cx, live_id!(clear_outputs), has_instance);
        menu.set_enabled(cx, live_id!(delete_selection), has_selection);
        menu.set_enabled(cx, live_id!(duplicate), has_selected_node);
        menu.set_enabled(cx, live_id!(flip_card), has_selected_node);
        menu.set_enabled(cx, live_id!(run_to_node), has_selected_node);
        menu.set_enabled(cx, live_id!(undo), can_undo);
        menu.set_enabled(cx, live_id!(redo), can_redo);
        menu.set_enabled(cx, live_id!(revert), can_undo);
        self.ui.button(cx, ids!(cancel_btn)).set_visible(cx, running);
        self.ui.button(cx, ids!(clear_btn)).set_enabled(cx, has_instance);
    }

    // -- events from the server ------------------------------------------------

    fn poll_subscription(&mut self, cx: &mut Cx) {
        let Some(subscriber) = self.subscriber.as_ref() else {
            return;
        };
        let events = subscriber.poll();
        for event in events {
            match event {
                SubscriptionEvent::Ready => {
                    self.refresh_flows();
                    self.refresh_instances();
                    self.fetch_instance();
                }
                SubscriptionEvent::ResyncRequired => {
                    self.refresh_flows();
                    self.refresh_instances();
                    self.fetch_instance();
                    self.fetch_run_snapshot();
                }
                SubscriptionEvent::Events(events) => {
                    for event in events {
                        self.services.handle_flow_event(&event);
                        self.handle_flow_event(cx, event);
                    }
                }
                SubscriptionEvent::Retry { .. } => {}
            }
        }
    }

    fn handle_flow_event(&mut self, cx: &mut Cx, event: FlowEvent) {
        match event.kind.as_str() {
            "flow.changed" => {
                self.refresh_flows();
                self.services.refresh_definitions();
                if !self.unsaved && event.name.as_deref() == self.selected.as_deref() {
                    if self.run_is_active() {
                        self.deferred_flow_reload |= self
                            .run
                            .as_ref()
                            .is_some_and(|run| event.revision != Some(run.revision));
                    } else if let Some(name) = event.name {
                        self.load_flow(name);
                    }
                }
                return;
            }
            "flow.removed" | "flow.error" => {
                self.refresh_flows();
                self.services.refresh_definitions();
                if event.kind == "flow.error" && event.name.as_deref() == self.selected.as_deref() {
                    if let Some(error) = event.error_text() {
                        self.set_error(cx, &error);
                    }
                }
                return;
            }
            _ => {}
        }
        if event.kind == "instance.cleared"
            && event.instance.as_deref() == self.instance.as_deref()
        {
            if let Some(instance) = event.instance.as_deref() {
                self.apply_instance_cleared(cx, instance);
            }
            return;
        }
        if event.topic == "instance" || event.kind.starts_with("instance.") {
            self.refresh_instances();
            if event.instance.as_deref() == self.instance.as_deref() {
                self.fetch_instance();
            }
            return;
        }
        if event.instance.as_deref() != self.instance.as_deref() {
            if event.kind.starts_with("run.") {
                self.refresh_instances();
            }
            return;
        }
        let event_run_id = event.run_id.as_deref().unwrap_or_default();
        if !run_event_belongs(self.run.as_ref(), &event.kind, event_run_id) {
            if event.kind == "run.started" && !self.run_is_active() {
                self.fetch_instance();
            }
            return;
        }
        let node = event.node.clone().unwrap_or_default();
        match event.kind.as_str() {
            "run.started" => {
                self.display_run(cx, RunInfo {
                    run_id: event.run_id.clone().unwrap_or_default(),
                    state: "running".into(),
                    started: self.time,
                    finished_secs: None,
                    revision: event.revision.unwrap_or(0),
                    planned_nodes: event.planned_nodes.clone().unwrap_or_default(),
                });
                self.refresh_instances();
            }
            "node.started" => self.set_node_status(cx, &node, "running", 0, false, "", None),
            "node.progress" => {
                let permille = event.permille.unwrap_or(0).min(1000) as u16;
                let stage = event.stage.clone().unwrap_or_default();
                self.set_node_status(cx, &node, "running", permille, true, &stage, None);
            }
            "node.delta" => {
                let port = event.port.clone().unwrap_or_else(|| "text".into());
                let text = event.text.clone().unwrap_or_default();
                if let Some(faces) = self.faces.as_mut() {
                    faces.push_delta(cx, &node, &port, &text);
                }
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    canvas.pulse(cx, &node, false);
                    canvas.set_streaming(cx, &node, true);
                }
            }
            "node.waiting" => {
                self.current_node = Some(node.clone());
                self.set_node_status(cx, &node, "waiting", 0, false, "", None);
                if let Some(mut app_view) = self.ui.widget(cx, ids!(app_view)).borrow_mut::<AppView>() {
                    app_view.waiting = Some(node.clone());
                }
            }
            "node.answered" => self.set_node_status(cx, &node, "running", 0, false, "", None),
            "node.done" => {
                self.set_node_status(cx, &node, "done", 1000, true, "", None);
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    canvas.pulse(cx, &node, true);
                }
                let outputs = event.output_values();
                for (port, value) in outputs {
                    self.record_value(cx, &node, &port, value);
                }
                self.request_wanted_values(cx);
                if self
                    .ui
                    .widget(cx, ids!(canvas))
                    .borrow::<FlowCanvas>()
                    .is_some_and(|canvas| canvas.selected() == Some(node.as_str()))
                {
                    self.refresh_inspector(cx);
                }
            }
            "node.failed" => {
                let error = event.error_text();
                if self.last_error.is_none() {
                    if let Some(error) = error.as_deref() {
                        self.set_error(cx, error);
                    }
                }
                self.set_node_status(cx, &node, "failed", 0, false, "", error);
            }
            "node.skipped" => {
                self.set_node_status(cx, &node, "skipped", 0, false, "", event.reason.clone())
            }
            "run.finished" => {
                let state = event.state_text().unwrap_or_else(|| "done".into());
                // Output nodes get no node.* events of their own: the run's
                // outputs are their values, so they finish with the run.
                for (output_node, value) in event.output_values() {
                    let port = self.output_face_port(&output_node);
                    self.record_value(cx, &output_node, &port, value);
                    self.set_node_status(cx, &output_node, "done", 1000, true, "", None);
                    if let Some(mut canvas) =
                        self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                    {
                        canvas.pulse(cx, &output_node, true);
                    }
                }
                self.request_wanted_values(cx);
                if let Some(run) = self.run.as_mut() {
                    run.state = state.clone();
                    run.finished_secs = event.secs;
                }
                let unresolved = self
                    .run
                    .as_ref()
                    .map(|run| run.planned_nodes.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|node| {
                        self.ui
                            .widget(cx, ids!(canvas))
                            .borrow::<FlowCanvas>()
                            .and_then(|canvas| canvas.statuses.get(node).cloned())
                            .is_none_or(|status| {
                                matches!(status.state.as_str(), "pending" | "ready" | "running" | "waiting")
                            })
                    })
                    .collect::<Vec<_>>();
                for node in unresolved {
                    self.set_node_status(cx, &node, "cancelled", 0, false, "", None);
                }
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    let streaming: Vec<String> = canvas.streaming.iter().cloned().collect();
                    for node in streaming {
                        canvas.set_streaming(cx, &node, false);
                    }
                }
                if let Some(faces) = self.faces.as_mut() {
                    faces.push_state(cx, "run", &state);
                }
                self.current_node = None;
                self.refresh_instances();
                self.fetch_instance();
                self.update_run_bar(cx);
                if self.deferred_flow_reload {
                    self.deferred_flow_reload = false;
                    if let Some(name) = self.selected.clone() {
                        self.load_flow(name);
                    }
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn set_node_status(
        &mut self,
        cx: &mut Cx,
        node: &str,
        state: &str,
        permille: u16,
        has_progress: bool,
        stage: &str,
        error: Option<String>,
    ) {
        if matches!(state, "running" | "waiting") {
            self.current_node = Some(node.to_string());
        }
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            let stage = if state == "running" {
                stage.to_string()
            } else {
                String::new()
            };
            canvas.set_status(
                cx,
                node,
                NodeStatus::new(state, permille, has_progress, &stage, error),
            );
        }
        if let Some(faces) = self.faces.as_mut() {
            faces.push_state(cx, node, state);
        }
        self.update_run_bar(cx);
    }

    fn output_face_port(&self, node_id: &str) -> String {
        self.definition
            .as_ref()
            .and_then(|definition| definition.graph.as_ref())
            .and_then(|graph| graph.nodes.iter().find(|node| node.id == node_id))
            .and_then(|node| {
                if node.kind == "output" {
                    node.inputs.first().map(|input| input.port.clone())
                } else {
                    node.outputs.first().map(|output| output.name.clone())
                }
            })
            .unwrap_or_else(|| "value".to_string())
    }

    fn record_value(&mut self, cx: &mut Cx, node: &str, port: &str, value: ValueRef) {
        let ports = self.outputs.entry(node.to_string()).or_default();
        if let Some((_, old)) = ports.iter_mut().find(|(name, _)| name == port) {
            *old = value.clone();
        } else {
            ports.push((port.to_string(), value.clone()));
        }
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_streaming(cx, node, false);
        }
        let bytes = self.values.get(&value.digest);
        if let Some(faces) = self.faces.as_mut() {
            faces.push_value(cx, node, port, &value, bytes.as_ref());
        }
    }

    // -- pending writes ----------------------------------------------------------

    /// Text inputs and auto-facing changes are coalesced on the 250 ms poll
    /// tick; explicit controls are written on the tick they change.
    fn flush_pending(&mut self, cx: &mut Cx) {
        if !self.pending_inputs.is_empty()
            && !self.input_flush_in_flight
            && self.instance.is_some()
            && self.client().is_some()
        {
            let instance = self.instance.clone().unwrap();
            let generation = self.attachment_generation;
            let journal = std::mem::take(&mut self.pending_inputs);
            let body = input_journal_body(self.current_graph().as_ref(), &journal);
            self.input_flush_in_flight = true;
            self.io(move |client| {
                let result = client.put_inputs(&instance, "tab", &body).map(|_| ());
                IoResult::InputsPut {
                    instance,
                    generation,
                    journal,
                    result,
                }
            });
        }
        if (!self.pending_params.is_empty() || !self.pending_auto_flips.is_empty())
            && !self.pending_graph
        {
            if let Some(mut graph) = self.current_graph() {
                let pending = std::mem::take(&mut self.pending_params);
                for ((node, key), value) in pending {
                    graph = graph_edit::set_param(&graph, &node, &key, value);
                }
                for (id, flip) in &self.pending_auto_flips {
                    if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == *id) {
                        node.flip = *flip;
                    }
                }
                self.put_graph(cx, graph);
            } else {
                self.pending_params.clear();
                self.pending_auto_flips.clear();
            }
        }
    }

    fn handle_bridge_call(&mut self, call: FaceBridgeCall) {
        let mine = self.instance.as_deref() == Some(call.instance.as_str())
            || (self.instance.is_none() && call.instance == "unbound");
        if !mine {
            return;
        }
        match call.call {
            BridgeCall::Input {
                node,
                port,
                value_json,
            } => {
                let text = makepad_strict_json::parse(value_json.as_bytes())
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or(value_json);
                self.pending_inputs.insert((node, port), text);
            }
            BridgeCall::Run { outputs } => self.start_run(outputs),
            BridgeCall::Cancel => self.cancel_run(),
            BridgeCall::Param {
                node,
                key,
                value_json,
            } => {
                let literal = match makepad_strict_json::parse(value_json.as_bytes()) {
                    Ok(makepad_strict_json::Value::Str(text)) => Literal::Str(text),
                    Ok(makepad_strict_json::Value::Int(number)) => Literal::Num(number as f64),
                    Ok(makepad_strict_json::Value::F64(number)) => Literal::Num(number),
                    Ok(makepad_strict_json::Value::Bool(value)) => Literal::Bool(value),
                    _ => Literal::Str(value_json),
                };
                self.pending_params.insert((node, key), literal);
            }
        }
    }

    fn start_run(&mut self, outputs: Option<Vec<String>>) {
        if self.input_flush_in_flight || self.run_start_in_flight {
            self.deferred_run = Some(RunRequest { outputs });
            return;
        }
        if self.client().is_none() {
            return;
        }
        let journal = std::mem::take(&mut self.pending_inputs);
        let input_body = (!journal.is_empty())
            .then(|| input_journal_body(self.current_graph().as_ref(), &journal));
        let planned_nodes = planned_nodes_for(self.current_graph().as_ref(), outputs.as_deref());
        let generation = self.attachment_generation;
        self.run_request_generation = self.run_request_generation.wrapping_add(1);
        let request_generation = self.run_request_generation;
        self.run_start_in_flight = true;
        if let Some(instance) = self.instance.clone() {
            self.io(move |client| {
                let result = (|| {
                    if let Some(inputs) = input_body.as_ref() {
                        client.put_inputs(&instance, "tab", inputs)?;
                    }
                    client.start_run(&instance, outputs.as_deref())
                })();
                IoResult::RunStarted {
                    instance,
                    generation,
                    request_generation,
                    planned_nodes,
                    journal,
                    result,
                }
            });
            return;
        }
        let Some(flow) = self.selected.clone() else {
            return;
        };
        self.io(move |client| {
            let result = (|| {
                let created = client.create_instance(&flow, &CreateInstanceRequest::default())?;
                if let Some(inputs) = input_body.as_ref() {
                    client.put_inputs(&created.instance, "tab", inputs)?;
                }
                let started = client.start_run(&created.instance, outputs.as_deref())?;
                Ok((created.instance, started))
            })();
            IoResult::InstanceRunStarted {
                flow,
                generation,
                request_generation,
                planned_nodes,
                journal,
                result,
            }
        });
    }

    fn fetch_run_snapshot(&self) {
        let (Some(instance), Some(run)) = (self.instance.clone(), self.run.as_ref()) else {
            return;
        };
        let generation = self.attachment_generation;
        let run_id = run.run_id.clone();
        self.io(move |client| IoResult::RunSnapshot {
            instance,
            generation,
            result: client.run(&run_id),
            run_id,
        });
    }

    fn apply_run_row(&mut self, cx: &mut Cx, row: RunRowDto) {
        let state = run_state_name(row.state).to_string();
        let terminal = matches!(row.state, RunState::Done | RunState::Failed | RunState::Cancelled);
        let finished_secs = row
            .finished_ms
            .map(|finished| finished.saturating_sub(row.started_ms) as f64 / 1000.0);
        let started = self.run.as_ref().map_or(self.time, |run| run.started);
        self.run = Some(RunInfo {
            run_id: row.run_id.clone(),
            state: state.clone(),
            started,
            finished_secs,
            revision: row.revision,
            planned_nodes: row.planned_nodes.clone(),
        });
        self.outputs.clear();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
        }
        if let Some(faces) = self.faces.as_mut() {
            faces.reset_run();
        }

        let mut first_error = None;
        for node in &row.planned_nodes {
            let source = row.nodes.get(node);
            let mut node_state = source.map_or(NodeState::Pending, |node| node.state);
            node_state = reconciled_node_state(row.state, node_state);
            let state_name = node_state_name(node_state);
            let progress = source.and_then(|node| node.progress);
            let stage = source.and_then(|node| node.stage.as_deref()).unwrap_or_default();
            let error = source.and_then(|node| node.error.clone());
            if first_error.is_none() {
                first_error = error.clone();
            }
            self.set_node_status(
                cx,
                node,
                state_name,
                progress.unwrap_or(0),
                progress.is_some(),
                stage,
                error,
            );
            if let Some(text) = source.and_then(|node| node.text.as_deref()) {
                if let Some(faces) = self.faces.as_mut() {
                    faces.push_delta(cx, node, "text", text);
                }
            }
            if let Some(source) = source {
                for output in &source.outputs {
                    self.record_value(cx, node, &output.port, output.value.clone());
                }
            }
        }
        for (node, value) in row.outputs {
            let port = self.output_face_port(&node);
            self.record_value(cx, &node, &port, value);
        }
        if let Some(error) = first_error {
            self.set_error(cx, &error);
        }
        if let Some(faces) = self.faces.as_mut() {
            faces.push_state(cx, "run", &state);
        }
        self.request_wanted_values(cx);
        self.update_run_bar(cx);
        if terminal && self.deferred_flow_reload {
            self.deferred_flow_reload = false;
            if let Some(name) = self.selected.clone() {
                self.load_flow(name);
            }
        }
    }

    fn cancel_run(&mut self) {
        let Some(run) = self.run.clone() else {
            return;
        };
        self.io(move |client| IoResult::Done(client.cancel_run(&run.run_id)));
    }

    fn clear_instance(&mut self, cx: &mut Cx) {
        if self.instance.is_none() {
            return;
        }
        if self.run_is_active() {
            self.ui.modal(cx, ids!(clear_confirm)).open(cx);
        } else {
            self.request_clear_instance(false);
        }
    }

    fn request_clear_instance(&self, cancel_first: bool) {
        let (Some(instance), Some(_client)) = (self.instance.clone(), self.client()) else {
            return;
        };
        let generation = self.attachment_generation;
        self.io(move |client| {
            let result = (|| {
                if cancel_first {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    loop {
                        for row in client.runs(Some(&instance))? {
                            if matches!(
                                row.state,
                                RunState::Queued | RunState::Running | RunState::Waiting
                            ) {
                                client.cancel_run(&row.run_id)?;
                            }
                        }
                        match client.clear_instance(&instance) {
                            Ok(()) => break,
                            Err(ClientError::Http { status: 409, .. })
                                if std::time::Instant::now() < deadline =>
                            {
                                std::thread::sleep(std::time::Duration::from_millis(20));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    Ok(())
                } else {
                    client.clear_instance(&instance)
                }
            })();
            IoResult::InstanceCleared {
                instance,
                generation,
                result,
            }
        });
    }

    fn apply_instance_cleared(&mut self, cx: &mut Cx, instance: &str) {
        if self.cleared_instances.contains(instance) && self.run.is_none() && self.outputs.is_empty()
        {
            return;
        }
        self.cleared_instances.insert(instance.to_string());
        self.run = None;
        self.outputs.clear();
        self.current_node = None;
        self.values = ValueCache::default();
        self.close_preview(cx);
        if let Some(row) = self.instance_row.as_mut() {
            row.outputs.clear();
            row.run = None;
            row.waiting = None;
            row.state = "idle".to_string();
        }
        if let Some(row) = self.instances.iter_mut().find(|row| row.instance == instance) {
            row.outputs.clear();
            row.run = None;
            row.waiting = None;
            row.state = "idle · cleared".to_string();
        }
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
        }
        if let Some(mut app_view) = self.ui.widget(cx, ids!(app_view)).borrow_mut::<AppView>() {
            app_view.waiting = None;
        }
        // Re-evaluating the faces is the narrowest way to restore every
        // generated widget to its declared empty state; fill_inputs below
        // immediately restores the preserved instance inputs.
        self.remount_faces(cx);
        if let Some(mut list) = self.ui.widget(cx, ids!(running)).borrow_mut::<RunningList>() {
            list.set_rows(cx, self.instances.clone(), self.instance.clone());
        }
        self.update_run_bar(cx);
        self.ui
            .label(cx, ids!(run_state))
            .set_text(cx, "idle · cleared");
        self.fetch_instance();
        self.refresh_instances();
    }

    fn undo(&mut self) {
        if self.revisions.len() < 2 {
            return;
        }
        let current = self.revisions.pop().unwrap();
        self.redo.push(current);
        let revision = *self.revisions.last().unwrap();
        self.revert_to(revision);
    }

    fn redo(&mut self) {
        let Some(revision) = self.redo.pop() else {
            return;
        };
        self.revisions.push(revision);
        self.revert_to(revision);
    }

    fn revert_to(&mut self, revision: u64) {
        if let Some(name) = self.selected.clone() {
            self.io(move |client| {
                let result = client.revert(&name, revision);
                IoResult::GraphPut { name, result }
            });
        }
    }

    // -- source pane ↔ canvas ----------------------------------------------------

    fn jump_to_node(&mut self, cx: &mut Cx, node: &str) {
        let Some(definition) = self.definition.as_ref() else {
            return;
        };
        let Some(line) = definition
            .graph
            .as_ref()
            .and_then(|graph| graph.nodes.iter().find(|n| n.id == node))
            .map(|node| node.loc.line as usize)
        else {
            return;
        };
        let index = definition
            .source
            .split_inclusive('\n')
            .take(line.saturating_sub(1))
            .map(str::len)
            .sum();
        self.ui.text_input(cx, ids!(source)).set_cursor(
            cx,
            Cursor {
                index,
                prefer_next_row: false,
            },
            false,
        );
    }

    fn highlight_caret_node(&mut self, cx: &mut Cx) {
        let Some(definition) = self.definition.as_ref() else {
            return;
        };
        let source = self.ui.text_input(cx, ids!(source));
        let index = source.cursor().index.min(definition.source.len());
        let line = definition.source[..index].matches('\n').count() + 1;
        let node = definition.graph.as_ref().and_then(|graph| {
            graph
                .nodes
                .iter()
                .filter(|node| node.loc.line as usize <= line)
                .max_by_key(|node| node.loc.line)
                .map(|node| node.id.clone())
        });
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_highlight(cx, node);
        }
    }

    fn set_modes(&mut self, cx: &mut Cx) {
        self.ui
            .view(cx, ids!(canvas_view))
            .set_visible(cx, !self.app_mode);
        self.ui
            .view(cx, ids!(app_view_view))
            .set_visible(cx, self.app_mode);
        self.ui
            .view(cx, ids!(source_view))
            .set_visible(cx, self.source_mode);
        let left_split = self.ui.widget(cx, ids!(column_split));
        if self.left_hidden {
            if self.left_panel_align.is_none() {
                self.left_panel_align = left_split.borrow::<Splitter>().map(|splitter| splitter.align());
            }
            left_split.as_splitter().set_align(cx, SplitterAlign::FromA(0.0));
        } else if let Some(align) = self.left_panel_align.take() {
            left_split.as_splitter().set_align(cx, align);
        }
        self.ui.view(cx, ids!(left_panel)).set_visible(cx, !self.left_hidden);

        let source_split = self.ui.widget(cx, ids!(right_source_split));
        if self.source_mode {
            if let Some(align) = self.source_panel_align.take() {
                source_split.as_splitter().set_align(cx, align);
            }
        } else {
            if self.source_panel_align.is_none() {
                self.source_panel_align = source_split
                    .borrow::<Splitter>()
                    .map(|splitter| splitter.align());
            }
            source_split
                .as_splitter()
                .set_align(cx, SplitterAlign::FromB(0.0));
        }
        self.ui.button(cx, ids!(view_btn)).set_text(
            cx,
            if self.app_mode { "Canvas" } else { "App view" },
        );
        self.ui.button(cx, ids!(side_btn)).set_text(
            cx,
            if self.source_mode { "Inspector" } else { "Source" },
        );
        self.update_canvas_fit_insets(cx);
        if self.selected_node.is_some() {
            self.refresh_models(true);
        }
        self.ui.redraw(cx);
    }

    fn point_over_canvas_chrome(&self, cx: &mut Cx, point: DVec2) -> bool {
        self.canvas_chrome_rects(cx)
            .iter()
            .any(|rect| rect.contains(point))
    }

    fn canvas_chrome_rects(&self, cx: &mut Cx) -> Vec<Rect> {
        let mut rects = vec![
            self.ui.widget(cx, ids!(toolbar)).area().rect(cx),
            self.ui.widget(cx, ids!(right_panel)).area().rect(cx),
            self.ui.widget(cx, ids!(column_split)).area().rect(cx),
            self.ui.widget(cx, ids!(canvas_right_split)).area().rect(cx),
        ];
        if !self.left_hidden {
            rects.push(self.ui.widget(cx, ids!(left_panel)).area().rect(cx));
        }
        let overlay_open = self.ui.view(cx, ids!(template_view)).visible()
            || self.ui.view(cx, ids!(help_view)).visible()
            || self.ui.view(cx, ids!(value_preview_view)).visible()
            || self.ui.modal(cx, ids!(clear_confirm)).is_open()
            || self
                .ui
                .widget(cx, ids!(image_viewer))
                .borrow::<ImageViewer>()
                .is_some_and(|viewer| viewer.is_open());
        if overlay_open {
            // App-owned overlays cover the graph. Popup widgets outside this
            // tree use the framework sweep lock or the scroll handled flags.
            rects.push(self.ui.widget(cx, ids!(canvas)).area().rect(cx));
        }
        rects
    }

    fn route_viewer_modal_input(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if !is_viewer_modal_input(event) {
            return false;
        }
        let viewer = self.ui.widget(cx, ids!(image_viewer));
        let Some(mut viewer) = viewer.borrow_mut::<ImageViewer>() else {
            return false;
        };
        if !viewer.is_open() {
            return false;
        }
        viewer.handle_event(cx, event, &mut Scope::empty());
        true
    }

    fn update_canvas_fit_insets(&mut self, cx: &mut Cx) {
        let canvas_rect = self.ui.widget(cx, ids!(canvas)).area().rect(cx);
        let left_rect = self.ui.widget(cx, ids!(left_panel)).area().rect(cx);
        let right_rect = self.ui.widget(cx, ids!(right_panel)).area().rect(cx);
        let left = if self.left_hidden {
            8.0
        } else if canvas_rect.size.x <= 0.0 || left_rect.size.x <= 0.0 {
            252.0
        } else {
            (left_rect.pos.x + left_rect.size.x - canvas_rect.pos.x + 8.0).max(8.0)
        };
        let right = if canvas_rect.size.x <= 0.0 || right_rect.size.x <= 0.0 {
            346.0
        } else {
            (canvas_rect.pos.x + canvas_rect.size.x - right_rect.pos.x + 8.0).max(8.0)
        };
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_fit_insets(Inset {
                left,
                top: 58.0,
                right,
                bottom: 8.0,
            });
        }
    }

    fn show_templates(&mut self, cx: &mut Cx, visible: bool) {
        if visible {
            self.io(|client| IoResult::Templates(client.templates()));
            self.ui.view(cx, ids!(help_view)).set_visible(cx, false);
        }
        self.ui.view(cx, ids!(template_view)).set_visible(cx, visible);
        self.ui.redraw(cx);
    }

    fn show_help(&mut self, cx: &mut Cx, title: &str, body: &str) {
        self.ui.view(cx, ids!(template_view)).set_visible(cx, false);
        self.ui.label(cx, ids!(help_title)).set_text(cx, title);
        self.ui.label(cx, ids!(help_text)).set_text(cx, body);
        self.ui.view(cx, ids!(help_view)).set_visible(cx, true);
        self.ui.redraw(cx);
    }

    fn fresh_flow_name(&self, base: &str) -> String {
        if !self.flows.iter().any(|flow| flow.name == base) {
            return base.to_string();
        }
        let mut number = 2;
        loop {
            let name = format!("{base}-{number}");
            if !self.flows.iter().any(|flow| flow.name == name) {
                return name;
            }
            number += 1;
        }
    }

    fn open_next_flow(&mut self, cx: &mut Cx) {
        if self.flows.is_empty() {
            return;
        }
        let index = self
            .selected
            .as_ref()
            .and_then(|name| self.flows.iter().position(|flow| &flow.name == name))
            .map(|index| (index + 1) % self.flows.len())
            .unwrap_or(0);
        let name = self.flows[index].name.clone();
        self.open_flow(cx, name);
    }

    fn delete_flow(&mut self, cx: &mut Cx) {
        let Some(name) = self.selected.take() else {
            return;
        };
        self.clear_attachment(cx);
        self.definition = None;
        self.ui.label(cx, ids!(flow_name)).set_text(cx, "No flow open");
        self.ui.label(cx, ids!(instance_chip)).set_text(cx, "");
        self.show_graph(cx);
        self.io(move |client| IoResult::Done(client.delete(&name)));
        self.auto_opened = false;
    }

    fn handle_menu(&mut self, cx: &mut Cx, id: LiveId) {
        match id {
            id if id == live_id!(new_from_template) => self.show_templates(cx, true),
            id if id == live_id!(open_flow) => self.open_next_flow(cx),
            id if id == live_id!(save) => {
                if let Some(name) = self.selected.clone() {
                    let source = self.ui.text_input(cx, ids!(source)).text();
                    if self.source_mode && !source.is_empty() {
                        self.save_source(name, source);
                    }
                }
            }
            id if id == live_id!(revert) || id == live_id!(undo) => self.undo(),
            id if id == live_id!(redo) => self.redo(),
            id if id == live_id!(delete_flow) => self.delete_flow(cx),
            id if id == live_id!(quit) => cx.quit(),
            id if id == live_id!(delete_selection) => {
                let selection = self
                    .ui
                    .widget(cx, ids!(canvas))
                    .borrow::<FlowCanvas>()
                    .and_then(|canvas| canvas.selection().cloned());
                let edit = match selection {
                    Some(Selection::Node(node)) => Some(CanvasEdit::Delete { node }),
                    Some(Selection::Edge {
                        to_node, to_port, ..
                    }) => Some(CanvasEdit::Disconnect { to_node, to_port }),
                    None => None,
                };
                if let Some(edit) = edit {
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        canvas.select(cx, None);
                    }
                    self.selected_node = None;
                    self.apply_edit(cx, edit);
                    self.refresh_inspector(cx);
                    self.update_menu_state(cx);
                }
            }
            id if id == live_id!(select_all) => {
                // One selection at a time: the first node stands for all.
                let first = self
                    .current_graph()
                    .and_then(|graph| graph.nodes.first().map(|node| node.id.clone()));
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    canvas.select(cx, first.clone());
                }
                self.selected_node = first;
                self.refresh_inspector(cx);
            }
            id if id == live_id!(duplicate) => self.duplicate_selected(cx),
            id if id == live_id!(wires_routed) => self.set_wire_mode(cx, WireMode::Routed),
            id if id == live_id!(wires_bezier) => self.set_wire_mode(cx, WireMode::Bezier),
            id if id == live_id!(view_canvas) => {
                self.app_mode = false;
                self.set_modes(cx);
            }
            id if id == live_id!(view_app) => {
                self.app_mode = true;
                self.set_modes(cx);
            }
            id if id == live_id!(view_source) => {
                self.source_mode = true;
                self.set_modes(cx);
            }
            id if id == live_id!(view_inspector) => {
                self.source_mode = false;
                self.set_modes(cx);
            }
            id if id == live_id!(flip_card) => {
                if let Some(node) = self.selected_node.clone() {
                    self.apply_edit(cx, CanvasEdit::Flip { node });
                }
            }
            id if id == live_id!(toggle_left) => {
                self.left_hidden = !self.left_hidden;
                self.set_modes(cx);
            }
            id if id == live_id!(zoom_in) => self.with_canvas(cx, |cx, canvas| canvas.zoom_by(cx, 1.25)),
            id if id == live_id!(zoom_out) => self.with_canvas(cx, |cx, canvas| canvas.zoom_by(cx, 0.8)),
            id if id == live_id!(zoom_fit) => self.with_canvas(cx, |cx, canvas| canvas.fit(cx)),
            id if id == live_id!(zoom_100) => self.with_canvas(cx, |cx, canvas| canvas.zoom_reset(cx)),
            id if id == live_id!(run) => self.start_run(None),
            id if id == live_id!(cancel) => self.cancel_run(),
            id if id == live_id!(clear_outputs) => self.clear_instance(cx),
            id if id == live_id!(run_to_node) => {
                if let Some(node) = self.selected_node.clone() {
                    self.start_run(Some(vec![node]));
                }
            }
            id if id == live_id!(help_shortcuts) => self.show_help(cx, "Keyboard shortcuts", HELP_SHORTCUTS),
            id if id == live_id!(help_language) => {
                self.show_help(cx, "The flow language", makepad_flow::AUTHORING_BRIEF)
            }
            id if id == live_id!(help_about) => self.show_help(cx, "About Flow", HELP_ABOUT),
            _ => {}
        }
    }

    fn with_canvas(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut FlowCanvas)) {
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            f(cx, &mut canvas);
        }
    }

    fn shutdown(&mut self, cx: &mut Cx) {
        self.services.shutdown();
        self.detach_faces(cx);
        if let Some(subscriber) = self.subscriber.take() {
            subscriber.request_stop();
        }
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        if let Some(host) = self.host.take() {
            host.shutdown();
        }
        self.testpattern_service.take();
    }

    fn refresh_ai_context(&mut self) {
        let selected = self
            .selected
            .as_deref()
            .and_then(|name| self.flows.iter().find(|flow| flow.name == name));
        let open_view = if self.app_mode {
            "app"
        } else if self.source_mode {
            "source"
        } else {
            "canvas"
        };
        self.services.set_context(BridgeContext {
            flow: self.selected.clone(),
            revision: selected.map(|flow| flow.revision),
            canonical: selected.map(|flow| flow.canonical),
            instance: self.instance.clone(),
            instance_state: self.instance_row.as_ref().map(|row| row.state.clone()),
            current_node: self.current_node.clone(),
            selected_node: self.selected_node.clone(),
            open_view: open_view.to_string(),
            last_error: self.last_error.clone(),
        });
    }

    fn open_value(&mut self, cx: &mut Cx, node: &str, port: &str) {
        let value = self
            .outputs
            .get(node)
            .and_then(|ports| ports.iter().find(|(name, _)| name == port))
            .map(|(_, value)| value.clone());
        let Some(value) = value else {
            return;
        };
        self.preview_bytes = None;
        self.preview_digest = Some((value.digest.clone(), format!("{node}.{port}")));
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.select(cx, Some(node.to_string()));
        }
        self.selected_node = Some(node.to_string());
        self.source_mode = false;
        self.set_modes(cx);
        match self.values.get(&value.digest) {
            Some(bytes) if value.content_type.starts_with("image/") => {
                self.show_image_viewer(cx, node, port, &bytes)
            }
            Some(bytes) => self.show_preview(cx, &bytes),
            None => {
                if let Some(client) = self.client() {
                    self.values.request(&value.digest, client);
                }
                self.refresh_inspector(cx);
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(dialog) = action.downcast_ref::<
                makepad_widgets::makepad_platform::file_dialogs::FileDialogAction,
            >() {
                self.handle_preview_save_dialog(cx, dialog);
            }
            if let Some(call) = action.downcast_ref::<FaceBridgeCall>() {
                self.handle_bridge_call(call.clone());
            }
            if let Some(action) = action.downcast_ref::<FlowUiAction>() {
                match action {
                    FlowUiAction::Focus { instance } => {
                        let flow = self
                            .instances
                            .iter()
                            .find(|row| row.instance == *instance)
                            .map(|row| row.flow.clone());
                        self.focus_instance(cx, instance.clone(), flow);
                    }
                    FlowUiAction::Select { node } => {
                        self.selected_node = Some(node.clone());
                        if let Some(mut canvas) =
                            self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                        {
                            canvas.select(cx, Some(node.clone()));
                        }
                        self.refresh_inspector(cx);
                        self.refresh_models(true);
                    }
                }
            }
        }
        if let Some(id) = self.ui.menu_bar(cx, ids!(menu_bar)).selected(actions) {
            self.handle_menu(cx, id);
        }
        let selected_index = self
            .ui
            .widget(cx, ids!(flow_list))
            .borrow::<FlowList>()
            .and_then(|list| list.selected(cx, actions));
        if let Some(index) = selected_index {
            if let Some(name) = self.flows.get(index).map(|flow| flow.name.clone()) {
                self.open_flow(cx, name);
            }
        }
        if self.ui.button(cx, ids!(new_btn)).clicked(actions) {
            let visible = self.ui.view(cx, ids!(template_view)).visible();
            self.show_templates(cx, !visible);
        }
        let template_pick = self
            .ui
            .widget(cx, ids!(template_picker))
            .borrow::<TemplatePicker>()
            .and_then(|picker| picker.picked(cx, actions));
        if let Some(template) = template_pick {
            self.show_templates(cx, false);
            self.create_from_template(&template);
        }
        if self
            .ui
            .widget(cx, ids!(template_picker))
            .borrow::<TemplatePicker>()
            .is_some_and(|picker| picker.closed(cx, actions))
        {
            self.show_templates(cx, false);
        }
        if self.ui.button(cx, ids!(help_close)).clicked(actions) {
            self.ui.view(cx, ids!(help_view)).set_visible(cx, false);
            self.ui.redraw(cx);
        }
        if self.ui.button(cx, ids!(preview_close)).clicked(actions) {
            self.close_preview(cx);
        }
        if self.ui.button(cx, ids!(preview_save)).clicked(actions) {
            self.save_preview(cx);
        }
        let viewer_actions = self
            .ui
            .widget(cx, ids!(image_viewer))
            .borrow_mut::<ImageViewer>()
            .map(|mut viewer| viewer.actions(cx, actions))
            .unwrap_or_default();
        for action in viewer_actions {
            match action {
                ImageViewerAction::None => {}
                ImageViewerAction::Close => self.close_preview(cx),
                ImageViewerAction::Save => self.save_preview(cx),
                ImageViewerAction::CopyDigest(digest) => cx.copy_to_clipboard(&digest),
                ImageViewerAction::Step(direction) => self.step_image_viewer(cx, direction),
            }
        }
        if self.ui.button(cx, ids!(save_btn)).clicked(actions) {
            if let Some(name) = self.selected.clone() {
                let source = self.ui.text_input(cx, ids!(source)).text();
                self.save_source(name, source);
            }
        }
        if self.ui.text_input(cx, ids!(source)).changed(actions).is_some() {
            self.unsaved = true;
        }
        if self.ui.button(cx, ids!(run_btn)).clicked(actions) {
            self.start_run(None);
        }
        if self.ui.button(cx, ids!(cancel_btn)).clicked(actions) {
            self.cancel_run();
        }
        if self.ui.button(cx, ids!(clear_btn)).clicked(actions) {
            self.clear_instance(cx);
        }
        if self.ui.button(cx, ids!(clear_cancel)).clicked(actions)
            || self.ui.modal(cx, ids!(clear_confirm)).dismissed(actions)
        {
            self.ui.modal(cx, ids!(clear_confirm)).close(cx);
        }
        if self.ui.button(cx, ids!(clear_confirm_btn)).clicked(actions) {
            self.ui.modal(cx, ids!(clear_confirm)).close(cx);
            self.request_clear_instance(true);
        }
        if self.ui.button(cx, ids!(fit_btn)).clicked(actions) {
            self.with_canvas(cx, |cx, canvas| canvas.fit(cx));
        }
        if self.ui.button(cx, ids!(view_btn)).clicked(actions) {
            self.app_mode = !self.app_mode;
            self.set_modes(cx);
        }
        if self.ui.button(cx, ids!(side_btn)).clicked(actions) {
            self.source_mode = !self.source_mode;
            self.set_modes(cx);
        }
        let column_changed = self
            .ui
            .widget(cx, ids!(column_split))
            .borrow::<Splitter>()
            .is_some_and(|splitter| splitter.changed(actions).is_some());
        let right_changed = self
            .ui
            .widget(cx, ids!(canvas_right_split))
            .borrow::<Splitter>()
            .is_some_and(|splitter| splitter.changed(actions).is_some());
        if column_changed && self.left_hidden {
            self.ui
                .widget(cx, ids!(column_split))
                .as_splitter()
                .set_align(cx, SplitterAlign::FromA(0.0));
        }
        let source_changed = self
            .ui
            .widget(cx, ids!(right_source_split))
            .borrow::<Splitter>()
            .is_some_and(|splitter| splitter.changed(actions).is_some());
        if source_changed && !self.source_mode {
            self.ui
                .widget(cx, ids!(right_source_split))
                .as_splitter()
                .set_align(cx, SplitterAlign::FromB(0.0));
        }
        if column_changed || right_changed {
            self.update_canvas_fit_insets(cx);
        }

        // Canvas.
        let canvas_uid = self.ui.widget(cx, ids!(canvas)).widget_uid();
        let canvas_actions: Vec<FlowCanvasAction> = actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == canvas_uid)
            .map(|action| action.cast::<FlowCanvasAction>())
            .collect();
        for action in canvas_actions {
            match action {
                FlowCanvasAction::None => {}
                FlowCanvasAction::Select(selection) => {
                    self.selected_node = selection
                        .as_ref()
                        .and_then(Selection::node)
                        .map(str::to_string);
                    if let Some(node) = self.selected_node.clone() {
                        if self.source_mode {
                            self.jump_to_node(cx, &node);
                        }
                    }
                    self.refresh_inspector(cx);
                    if self.selected_node.is_some() {
                        self.refresh_models(true);
                    }
                    self.update_menu_state(cx);
                }
                FlowCanvasAction::Edit(edit) => self.apply_edit(cx, edit),
                FlowCanvasAction::OpenPalette {
                    at,
                    from_node,
                    from_port,
                    ty,
                } => {
                    let Some(ty) = graph_edit::port_type_from_name(&ty) else {
                        continue;
                    };
                    let compatible: Vec<NodeTypeCatalog> =
                        graph_edit::types_with_compatible_input(&self.catalog, ty)
                            .into_iter()
                            .cloned()
                            .collect();
                    if let Some(mut palette) = self.ui.widget(cx, ids!(palette)).borrow_mut::<Palette>() {
                        palette.set_types(cx, &compatible, true);
                    }
                    self.ui
                        .label(cx, ids!(palette_note))
                        .set_text(cx, "Click a card to add it at the drop point, wired.");
                    self.palette_drop = Some((at, from_node, from_port, ty));
                }
                FlowCanvasAction::Camera { scale } => {
                    self.ui
                        .label(cx, ids!(zoom_label))
                        .set_text(cx, &format!("{:.0} %", scale * 100.0));
                }
                FlowCanvasAction::AutoFlip(changes) => {
                    let Some(mut graph) = self.current_graph() else {
                        continue;
                    };
                    let mut changed = false;
                    let mut staged = Vec::new();
                    for (id, flip) in changes {
                        if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
                            if node.flip != flip {
                                changed = true;
                                staged.push((id, flip));
                            }
                            node.flip = flip;
                        }
                    }
                    if changed {
                        if let Some(definition) = self.definition.as_mut() {
                            definition.graph = Some(graph);
                        }
                        self.show_graph(cx);
                        self.pending_auto_flips.extend(staged);
                    }
                }
            }
        }

        // Palette.
        let palette_actions = self
            .ui
            .widget(cx, ids!(palette))
            .borrow::<Palette>()
            .map(|palette| palette.actions(cx, actions))
            .unwrap_or_default();
        for action in palette_actions {
            match action {
                PaletteAction::None => {}
                PaletteAction::Armed(type_name) => {
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        canvas.armed_type = Some(type_name);
                    }
                }
                PaletteAction::Picked(type_name) => {
                    if let Some((at, from_node, from_port, ty)) = self.palette_drop.take() {
                        if let (Some(graph), Some(entry)) = (
                            self.current_graph(),
                            self.catalog.iter().find(|entry| entry.type_name == type_name),
                        ) {
                            let (next, id) = graph_edit::add_node(&graph, entry, at);
                            let target = next
                                .nodes
                                .iter()
                                .find(|node| node.id == id)
                                .and_then(|node| {
                                    node.inputs
                                        .iter()
                                        .find(|input| input.ty == ty)
                                        .or_else(|| node.inputs.first())
                                        .map(|input| input.port.clone())
                                });
                            let next = match target {
                                Some(port) => graph_edit::connect(&next, &from_node, &from_port, &id, &port)
                                    .unwrap_or(next),
                                None => next,
                            };
                            self.put_graph(cx, next);
                        }
                    }
                    if let Some(mut palette) = self.ui.widget(cx, ids!(palette)).borrow_mut::<Palette>() {
                        palette.set_types(cx, &self.catalog, false);
                    }
                    self.ui
                        .label(cx, ids!(palette_note))
                        .set_text(cx, "Drag a card onto the canvas to add a node.");
                }
            }
        }

        // Inspector.
        let inspector_actions = self
            .ui
            .widget(cx, ids!(inspector))
            .borrow_mut::<Inspector>()
            .map(|mut inspector| inspector.changes(cx, actions))
            .unwrap_or_default();
        for action in inspector_actions {
            match action {
                InspectorAction::None => {}
                InspectorAction::SetParam { node, key, value } => {
                    if let Some(graph) = self.current_graph() {
                        let next = graph_edit::set_param(&graph, &node, &key, value);
                        self.put_graph(cx, next);
                    }
                }
                InspectorAction::SetFnSrc { node, src } => {
                    if let Some(graph) = self.current_graph() {
                        let next = graph_edit::set_fn_src(&graph, &node, &src);
                        self.put_graph(cx, next);
                    }
                }
                InspectorAction::SetFaceSrc { node, src } => {
                    if let Some(graph) = self.current_graph() {
                        let next = graph_edit::set_face_src(&graph, &node, &src);
                        self.put_graph(cx, next);
                    }
                }
                InspectorAction::FlipCard { node } => {
                    self.apply_edit(cx, CanvasEdit::Flip { node });
                }
                InspectorAction::RenameNode { node, new_id } => {
                    if !valid_node_id(&new_id) {
                        self.set_error(cx, "node ids use letters, digits and underscores and cannot start with a digit");
                        continue;
                    }
                    let Some(mut graph) = self.current_graph() else {
                        continue;
                    };
                    if graph.nodes.iter().any(|item| item.id == new_id) {
                        self.set_error(cx, &format!("a node named {new_id} already exists"));
                        continue;
                    }
                    rename_graph_node(&mut graph, &node, &new_id);
                    if let Some(values) = self.outputs.remove(&node) {
                        self.outputs.insert(new_id.clone(), values);
                    }
                    self.selected_node = Some(new_id.clone());
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        canvas.select(cx, Some(new_id));
                    }
                    self.put_graph(cx, graph);
                }
                InspectorAction::SetNodeDoc { node, doc } => {
                    if let Some(mut graph) = self.current_graph() {
                        if let Some(item) = graph.nodes.iter_mut().find(|item| item.id == node) {
                            let doc = doc.trim();
                            item.doc = (!doc.is_empty()).then(|| doc.to_string());
                        }
                        self.put_graph(cx, graph);
                    }
                }
                InspectorAction::SetNodeMeta { node, key, value } => {
                    if let Some(mut graph) = self.current_graph() {
                        if let Some(item) = graph.nodes.iter_mut().find(|item| item.id == node) {
                            match (key.as_str(), value) {
                                ("on_fail", Literal::Id(value) | Literal::Str(value)) => {
                                    item.on_fail = value;
                                }
                                ("label", Literal::Str(value) | Literal::Id(value)) => {
                                    item.label = (!value.is_empty()).then_some(value);
                                }
                                _ => {}
                            }
                        }
                        self.put_graph(cx, graph);
                    }
                }
                InspectorAction::SelectNode(node) => {
                    self.selected_node = Some(node.clone());
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        canvas.select(cx, Some(node));
                    }
                    self.refresh_inspector(cx);
                    self.refresh_models(true);
                }
                InspectorAction::Disconnect { node, port } => {
                    if let Some(graph) = self.current_graph() {
                        self.put_graph(cx, graph_edit::disconnect(&graph, &node, &port));
                    }
                }
                InspectorAction::JumpSource(node) => {
                    self.source_mode = true;
                    self.set_modes(cx);
                    self.jump_to_node(cx, &node);
                }
                InspectorAction::SaveValue { node, port } => {
                    let digest = self
                        .outputs
                        .get(&node)
                        .and_then(|ports| ports.iter().find(|(name, _)| name == &port))
                        .map(|(_, value)| value.digest.clone());
                    self.save_when_ready = digest;
                    self.open_value(cx, &node, &port);
                    if self.preview_bytes.is_some() {
                        self.save_when_ready = None;
                        self.save_preview(cx);
                    }
                }
                InspectorAction::CopyDigest(digest) => cx.copy_to_clipboard(&digest),
                InspectorAction::OpenValue { node, port } => self.open_value(cx, &node, &port),
            }
        }

        // Running.
        let running_actions = self
            .ui
            .widget(cx, ids!(running))
            .borrow::<RunningList>()
            .map(|list| list.actions(cx, actions))
            .unwrap_or_default();
        for action in running_actions {
            match action {
                RunningAction::None => {}
                RunningAction::Attach(id) => {
                    let flow = self
                        .instances
                        .iter()
                        .find(|row| row.instance == id)
                        .map(|row| row.flow.clone());
                    self.focus_instance(cx, id, flow);
                }
                RunningAction::Stop(id) => {
                    let target = id.clone();
                    self.io(move |client| IoResult::Done(client.delete_instance(&target)));
                    if self.instance.as_deref() == Some(id.as_str()) {
                        self.attach_instance(cx, None, false);
                    }
                }
                RunningAction::Duplicate(id) => {
                    let flow = self
                        .instances
                        .iter()
                        .find(|row| row.instance == id)
                        .map(|row| row.flow.clone());
                    if let Some(flow) = flow {
                        let generation = self.attachment_generation;
                        self.io(move |client| {
                            let request = CreateInstanceRequest {
                                label: Some("copy".to_string()),
                                ..CreateInstanceRequest::default()
                            };
                            let result = client.create_instance(&flow, &request);
                            IoResult::InstanceCreated {
                                flow,
                                generation,
                                result,
                            }
                        });
                    }
                }
                RunningAction::CopyId(id) => cx.copy_to_clipboard(&id),
                RunningAction::OpenImage {
                    instance,
                    label,
                    digest,
                } => {
                    self.preview_digest = Some((digest.clone(), format!("{instance}.{label}")));
                    self.preview_bytes = None;
                    if let Some(bytes) = self.values.get(&digest) {
                        self.show_image_viewer(cx, &instance, &label, &bytes);
                    } else if let Some(client) = self.client() {
                        self.values.request(&digest, client);
                    }
                }
            }
        }

        // Faces: bound widgets → instance inputs / graph params; pictures → open.
        let mut opens = Vec::new();
        if let Some(faces) = self.faces.as_mut() {
            for (node, port, text) in faces.bind_changes(cx, actions) {
                self.pending_inputs.insert((node, port), text);
            }
            let mut params = faces.param_changes(cx, actions);
            params.extend(faces.model_changes(cx, actions));
            for (node, key, value) in params {
                self.pending_params.insert((node, key), value);
            }
            opens = faces.open_requests(actions);
        }
        for (node, port) in opens {
            self.open_value(cx, &node, &port);
        }
        self.refresh_ai_context();
    }
}

impl App {
    fn close_preview(&mut self, cx: &mut Cx) {
        self.preview_digest = None;
        self.preview_bytes = None;
        self.save_when_ready = None;
        if let Some(mut viewer) = self
            .ui
            .widget(cx, ids!(image_viewer))
            .borrow_mut::<ImageViewer>()
        {
            viewer.close(cx);
        }
        self.ui
            .view(cx, ids!(value_preview_view))
            .set_visible(cx, false);
        self.ui.redraw(cx);
    }

    fn save_preview(&mut self, cx: &mut Cx) {
        let Some(bytes) = self.preview_bytes.clone() else {
            return;
        };
        let extension = value_file_extension(&bytes.content_type);
        let stem = self
            .preview_digest
            .as_ref()
            .map(|(_, label)| label.as_str())
            .unwrap_or("value");
        self.save_dialog_bytes = Some(bytes);
        cx.open_save_file_dialog(
            makepad_widgets::makepad_platform::file_dialogs::FileDialog::new()
                .set_id(live_id!(save_flow_value))
                .set_title("Save flow output".into())
                .set_filename(format!("{stem}.{extension}"))
                .add_filter(format!("{extension} file"), vec![extension.into()]),
        );
    }

    fn handle_preview_save_dialog(
        &mut self,
        cx: &mut Cx,
        action: &makepad_widgets::makepad_platform::file_dialogs::FileDialogAction,
    ) {
        use makepad_widgets::makepad_platform::file_dialogs::FileDialogAction;
        match action {
            FileDialogAction::SaveFileSelected { id, path }
                if *id == live_id!(save_flow_value) =>
            {
                let Some(value) = self.save_dialog_bytes.take() else {
                    return;
                };
                let path = path.clone();
                let bytes = value.bytes;
                match cx.task_pool().submit(
                    makepad_widgets::makepad_platform::thread::Lane::Light,
                    move || {
                        std::fs::write(&path, bytes.as_ref())
                            .map(|_| path)
                            .map_err(|error| error.to_string())
                    },
                ) {
                    Ok(task) => self.save_task = Some(task),
                    Err(error) => self.set_error(cx, &format!("could not queue save: {error}")),
                }
            }
            FileDialogAction::SaveFileCancelled { id } if *id == live_id!(save_flow_value) => {
                self.save_dialog_bytes = None;
            }
            _ => {}
        }
    }

    fn drain_save_task(&mut self, cx: &mut Cx) {
        let Some(result) = self.save_task.as_mut().and_then(|task| task.try_take()) else {
            return;
        };
        self.save_task = None;
        match result {
            Ok(Ok(path)) => self.set_error(cx, &format!("saved {}", path.display())),
            Ok(Err(error)) => self.set_error(cx, &format!("save failed: {error}")),
            Err(error) => self.set_error(cx, &format!("save worker failed: {error}")),
        }
    }

    fn show_preview(&mut self, cx: &mut Cx, bytes: &makepad_flow::ValueBytes) {
        let text = if bytes.content_type.starts_with("image/") {
            String::new()
        } else {
            String::from_utf8_lossy(&bytes.bytes).chars().take(4000).collect()
        };
        if let Some(mut inspector) = self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>() {
            inspector.set_preview(Some((text, bytes.clone())));
        }
        self.preview_bytes = Some(bytes.clone());
        let title = self
            .preview_digest
            .as_ref()
            .map(|(_, label)| label.as_str())
            .unwrap_or("Value");
        self.ui
            .label(cx, ids!(preview_title))
            .set_text(cx, title);
        if let Some(mut value) = self
            .ui
            .widget(cx, ids!(preview_value))
            .borrow_mut::<faces::ValueView>()
        {
            value.set_card_sized(cx, true);
            if bytes.content_type.starts_with("image/") {
                value.set_image(cx, bytes);
            } else {
                let text = String::from_utf8_lossy(&bytes.bytes);
                let text = if text.is_empty() {
                    format!("{} · {}", bytes.content_type, faces::size_text(bytes.bytes.len()))
                } else {
                    text.chars().take(16 * 1024).collect()
                };
                value.set_text(cx, &text);
            }
        }
        self.ui
            .view(cx, ids!(value_preview_view))
            .set_visible(cx, true);
        self.ui.redraw(cx);
        self.refresh_inspector(cx);
    }

    fn show_image_viewer(
        &mut self,
        cx: &mut Cx,
        node: &str,
        port: &str,
        bytes: &makepad_flow::ValueBytes,
    ) {
        self.preview_bytes = Some(bytes.clone());
        self.ui
            .view(cx, ids!(value_preview_view))
            .set_visible(cx, false);
        let result = self
            .ui
            .widget(cx, ids!(image_viewer))
            .borrow_mut::<ImageViewer>()
            .map(|mut viewer| {
                viewer.show(
                    cx,
                    ImageViewerItem {
                        node: node.to_string(),
                        port: port.to_string(),
                        bytes: bytes.clone(),
                    },
                )
            });
        if let Some(Err(error)) = result {
            self.set_error(cx, &error);
        }
        self.refresh_inspector(cx);
    }

    fn refresh_running_thumbnails(&mut self, cx: &mut Cx) {
        let pictures: Vec<String> = self
            .instances
            .iter()
            .flat_map(|row| row.outputs.values())
            .filter(|value| value.content_type.starts_with("image/"))
            .map(|value| value.digest.clone())
            .collect();
        let client = self.client();
        for digest in pictures {
            if let Some(bytes) = self.values.get(&digest) {
                if let Some(mut list) = self
                    .ui
                    .widget(cx, ids!(running))
                    .borrow_mut::<RunningList>()
                {
                    list.set_thumbnail(cx, bytes);
                }
            } else if let Some(client) = client.as_ref() {
                self.values.request(&digest, client.clone());
            }
        }
    }

    fn step_image_viewer(&mut self, cx: &mut Cx, direction: i32) {
        let current_digest = self.preview_digest.as_ref().map(|(digest, _)| digest.as_str());
        let running_pictures = current_digest.and_then(|digest| {
            self.instances
                .iter()
                .find(|row| row.outputs.values().any(|value| value.digest == digest))
                .map(|row| {
                    row.outputs
                        .iter()
                        .filter(|(_, value)| value.content_type.starts_with("image/"))
                        .map(|(label, value)| {
                            (row.instance.clone(), label.clone(), value.digest.clone())
                        })
                        .collect::<Vec<_>>()
                })
        });
        let running_gallery = running_pictures.is_some();
        let mut pictures: Vec<(String, String, String)> = running_pictures.unwrap_or_else(|| {
            self.outputs
                .iter()
                .flat_map(|(node, ports)| {
                    ports.iter().filter_map(move |(port, value)| {
                        value.content_type.starts_with("image/").then(|| {
                            (node.clone(), port.clone(), value.digest.clone())
                        })
                    })
                })
                .collect()
        });
        pictures.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        if pictures.is_empty() {
            return;
        }
        let current = current_digest
            .and_then(|digest| pictures.iter().position(|(_, _, item)| item == digest))
            .unwrap_or(0);
        let next = (current as i32 + direction).rem_euclid(pictures.len() as i32) as usize;
        let (node, port, _) = pictures[next].clone();
        if running_gallery {
            let digest = pictures[next].2.clone();
            self.preview_digest = Some((digest.clone(), format!("{node}.{port}")));
            self.preview_bytes = None;
            if let Some(bytes) = self.values.get(&digest) {
                self.show_image_viewer(cx, &node, &port, &bytes);
            } else if let Some(client) = self.client() {
                self.values.request(&digest, client);
            }
        } else {
            self.open_value(cx, &node, &port);
        }
    }

    fn drain_values(&mut self, cx: &mut Cx) {
        for arrival in self.values.drain() {
            match arrival {
                Ok(digest) => {
                    if let Some(faces) = self.faces.as_mut() {
                        faces.deliver_bytes(cx, &mut self.values, &digest);
                    }
                    if let Some(bytes) = self.values.get(&digest) {
                        if bytes.content_type.starts_with("image/") {
                            if let Some(mut list) = self
                                .ui
                                .widget(cx, ids!(running))
                                .borrow_mut::<RunningList>()
                            {
                                list.set_thumbnail(cx, bytes);
                            }
                        }
                    }
                    if self
                        .preview_digest
                        .as_ref()
                        .is_some_and(|(wanted, _)| *wanted == digest)
                    {
                        if let Some(bytes) = self.values.get(&digest) {
                            let label = self
                                .preview_digest
                                .as_ref()
                                .map(|(_, label)| label.clone())
                                .unwrap_or_default();
                            if bytes.content_type.starts_with("image/") {
                                let (node, port) = label
                                    .split_once('.')
                                    .unwrap_or((label.as_str(), "image"));
                                self.show_image_viewer(cx, node, port, &bytes);
                            } else {
                                self.show_preview(cx, &bytes);
                            }
                            if self.save_when_ready.as_deref() == Some(digest.as_str()) {
                                self.save_when_ready = None;
                                self.save_preview(cx);
                            }
                        }
                    }
                }
                Err((digest, error)) => {
                    self.set_error(cx, &format!("value {}…: {error}", &digest[..8.min(digest.len())]));
                }
            }
        }
        self.drain_save_task(cx);
    }
}

fn value_file_extension(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or(content_type).trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        "video/mp4" => "mp4",
        "application/json" => "json",
        "text/plain" | "text/markdown" => "txt",
        _ => "bin",
    }
}

fn valid_node_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// Rename every graph reference in one atomic graph PUT. This includes the
/// input edge mirrors and the names projected into `Flow{ tools: ... }`.
fn rename_graph_node(graph: &mut Graph, old: &str, new: &str) {
    if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == old) {
        node.id = new.to_string();
    }
    for node in &mut graph.nodes {
        for input in &mut node.inputs {
            if let makepad_flow::NodeInputValue::Edge(edge) = &mut input.value {
                if edge.from_node == old {
                    edge.from_node = new.to_string();
                }
            }
        }
    }
    for edge in &mut graph.edges {
        if edge.from_node == old {
            edge.from_node = new.to_string();
        }
        if edge.to_node == old {
            edge.to_node = new.to_string();
        }
    }
    for tool in &mut graph.tools {
        for name in tool
            .inputs
            .iter_mut()
            .chain(tool.outputs.iter_mut())
            .chain(tool.nodes.iter_mut())
        {
            if name == old {
                *name = new.to_string();
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        makepad_aichat::script_mod(vm);
        makepad_flowgraph::script_mod(vm);
        theme::script_mod(vm);
        faces::register_host_widgets(vm);
        panels::script_mod(vm);
        viewer::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.startup(cx);
        }
        if let Event::NextFrame(nf) = event {
            self.time = nf.time;
        }
        // The full-window viewer is modal. Route user input directly to it
        // before selection, faces, the ordinary widget tree, and shortcuts.
        if self.route_viewer_modal_input(cx, event) {
            return;
        }
        if matches!(event, Event::KeyDown(e) if e.key_code == KeyCode::Escape)
            && self.ui.view(cx, ids!(value_preview_view)).visible()
        {
            self.close_preview(cx);
        }
        self.services.handle_event(cx, event);
        self.match_event(cx, event);
        self.update_canvas_fit_insets(cx);
        let chrome_rects = self.canvas_chrome_rects(cx);
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_chrome_rects(chrome_rects);
        }
        // Selection belongs to the card, even when the press is about to be
        // handled by an interactive child in its mounted face.
        if !self.app_mode {
            if let Event::MouseDown(e) = event {
                if !self.point_over_canvas_chrome(cx, e.abs) {
                    if let Some(mut canvas) =
                        self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                    {
                        canvas.select_at(cx, e.abs);
                    }
                }
            }
        }
        // The faces first: a click inside a face is the face's, and the
        // canvas then sees it as handled. Their positions go through the
        // canvas camera.
        if !matches!(event, Event::Draw(_)) {
            let canvas_mode = !self.app_mode;
            let (camera, resize_press) = self
                .ui
                .widget(cx, ids!(canvas))
                .borrow::<FlowCanvas>()
                .map(|canvas| {
                    (
                        canvas.camera(),
                        canvas_mode
                            && matches!(event, Event::MouseDown(e) if canvas.is_resize_handle_at(e.abs)),
                    )
                })
                .map(|(camera, resize)| (Some(camera), resize))
                .unwrap_or((None, false));
            let mapped = canvas_mode;
            let over_chrome = match event {
                Event::MouseDown(e) => self.point_over_canvas_chrome(cx, e.abs),
                Event::MouseMove(e) => self.point_over_canvas_chrome(cx, e.abs),
                Event::MouseUp(e) => self.point_over_canvas_chrome(cx, e.abs),
                Event::Scroll(e) => self.point_over_canvas_chrome(cx, e.abs),
                Event::LongPress(e) => self.point_over_canvas_chrome(cx, e.abs),
                _ => false,
            };
            if !over_chrome && !resize_press {
                if let Some(faces) = self.faces.as_mut() {
                    faces.handle_event(
                        cx,
                        event,
                        &mut Scope::empty(),
                        if mapped { camera.as_ref() } else { None },
                    );
                }
            }
        }
        let face_owns_key_focus = self
            .faces
            .as_ref()
            .is_some_and(|faces| faces.owns_key_focus(cx));
        if !(face_owns_key_focus && is_focus_routed_input(event)) {
            match self.faces.as_mut() {
                Some(faces) => {
                    let mut faces = NodeFacesScope::new(faces);
                    let mut scope = Scope::with_data(&mut faces);
                    self.ui.handle_event(cx, event, &mut scope);
                }
                None => self.ui.handle_event(cx, event, &mut Scope::empty()),
            }
        }
        let text_editing = widget_tree_has_text_focus(&self.ui, cx);
        self.ui
            .menu_bar(cx, ids!(menu_bar))
            .handle_shortcut(cx, event, text_editing);
        if self.poll_timer.is_event(event).is_some() || matches!(event, Event::Signal) {
            self.drain_io(cx);
            self.drain_values(cx);
            self.update_connection(cx);
            self.poll_subscription(cx);
            self.flush_pending(cx);
            if self.source_mode {
                self.highlight_caret_node(cx);
            }
            if self.run_is_active() {
                self.update_run_bar(cx);
            }
            self.refresh_models(false);
        }
        self.refresh_ai_context();
        if let Event::Shutdown = event {
            self.shutdown(cx);
        }
    }
}

fn widget_tree_has_text_focus(root: &WidgetRef, cx: &Cx) -> bool {
    if root.borrow::<TextInput>().is_some() && cx.has_key_focus(root.area()) {
        return true;
    }
    let mut children = Vec::new();
    root.children(&mut |_, child| children.push(child));
    children
        .iter()
        .any(|child| widget_tree_has_text_focus(child, cx))
}

fn merge_failed_input_journal(pending: &mut InputJournal, journal: InputJournal) {
    for (key, value) in journal {
        pending.entry(key).or_insert(value);
    }
}

fn attachment_matches(
    current_instance: Option<&str>,
    current_generation: u64,
    response_instance: &str,
    response_generation: u64,
) -> bool {
    current_generation == response_generation && current_instance == Some(response_instance)
}

fn request_generation_matches(current: u64, response: u64) -> bool {
    current == response
}

fn run_event_belongs(current: Option<&RunInfo>, kind: &str, event_run_id: &str) -> bool {
    if kind == "run.started" {
        return current.map(|run| run.run_id.as_str()) == Some(event_run_id);
    }
    if kind.starts_with("node.") || kind == "run.finished" {
        return current.map(|run| run.run_id.as_str()) == Some(event_run_id);
    }
    true
}

fn reconciled_node_state(run: RunState, node: NodeState) -> NodeState {
    if matches!(run, RunState::Done | RunState::Failed | RunState::Cancelled)
        && matches!(
            node,
            NodeState::Pending | NodeState::Ready | NodeState::Running | NodeState::Waiting
        )
    {
        NodeState::Cancelled
    } else {
        node
    }
}

fn input_journal_body(graph: Option<&Graph>, journal: &InputJournal) -> makepad_strict_json::Value {
    let mut by_node: HashMap<String, Vec<(String, makepad_strict_json::Value)>> = HashMap::new();
    for ((node, port), text) in journal {
        let ty = graph
            .and_then(|graph| instance_input_type(graph, node, port))
            .unwrap_or(PortType::Text);
        by_node
            .entry(node.clone())
            .or_default()
            .push((port.clone(), input_value_json(ty, text.clone())));
    }
    makepad_strict_json::Value::Obj(
        by_node
            .into_iter()
            .map(|(node, ports)| (node, makepad_strict_json::Value::Obj(ports)))
            .collect(),
    )
}

fn planned_nodes_for(graph: Option<&Graph>, outputs: Option<&[String]>) -> Vec<String> {
    let Some(graph) = graph else {
        return Vec::new();
    };
    let requested: Vec<String> = outputs.map_or_else(
        || {
            graph
                .nodes
                .iter()
                .filter(|node| node.kind == "output")
                .map(|node| node.id.clone())
                .collect()
        },
        |outputs| outputs.to_vec(),
    );
    let index = graph_edit::graph_index(&graph);
    let mut planned = HashSet::new();
    for node in requested {
        planned.insert(node.clone());
        planned.extend(
            index
                .ancestor_indices(&node)
                .into_iter()
                .map(|index| graph.nodes[index].id.clone()),
        );
    }
    let mut planned: Vec<String> = planned.into_iter().collect();
    planned.sort();
    planned
}

fn total_progress(
    planned_nodes: &[String],
    statuses: &HashMap<String, NodeStatus>,
) -> (f64, usize, usize) {
    let mut contribution = 0.0;
    let mut done = 0;
    for node in planned_nodes {
        match statuses.get(node) {
            Some(status) if status.state == "done" => {
                done += 1;
                contribution += 1.0;
            }
            Some(status) if status.state == "running" && status.has_progress => {
                contribution += status.permille as f64 / 1000.0;
            }
            _ => {}
        }
    }
    let total = planned_nodes.len();
    (if total == 0 { 0.0 } else { contribution / total as f64 }, done, total)
}

fn run_state_name(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::Waiting => "waiting",
        RunState::Done => "done",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
    }
}

fn node_state_name(state: NodeState) -> &'static str {
    match state {
        NodeState::Pending => "pending",
        NodeState::Ready => "ready",
        NodeState::Running => "running",
        NodeState::Waiting => "waiting",
        NodeState::Done => "done",
        NodeState::Failed => "failed",
        NodeState::Skipped => "skipped",
        NodeState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use makepad_flow::NodeRowDto;
    use makepad_widgets::makepad_platform::event::{ScrollEvent, ScrollPhase};
    use std::cell::Cell;

    fn menu_label(app: &App, cx: &Cx, id: LiveId) -> String {
        let menu = app.ui.menu_bar(cx, ids!(menu_bar));
        let inner = menu.borrow().unwrap();
        inner
            .menus()
            .iter()
            .flat_map(|menu| &menu.items)
            .find(|entry| entry.id == id)
            .unwrap()
            .label
            .clone()
    }

    fn draw_canvas_and_viewer(
        cx: &mut Cx,
        app: &mut App,
        pass: &DrawPass,
        draw_list: &mut DrawList2d,
        size: DVec2,
    ) {
        let event = DrawEvent {
            redraw_all: true,
            ..Default::default()
        };
        let canvas = app.ui.widget(cx, ids!(canvas));
        let viewer = app.ui.widget(cx, ids!(image_viewer));
        let mut cx_draw = CxDraw::new(cx, &event);
        let cx = &mut Cx2d::new(&mut cx_draw);
        cx.begin_pass(pass, Some(1.0));
        draw_list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        canvas.draw_walk_all(cx, &mut Scope::empty(), Walk::fill());
        viewer.draw_walk_all(cx, &mut Scope::empty(), Walk::fill());
        cx.end_pass_sized_turtle();
        draw_list.end(cx);
        cx.end_pass(pass);
    }

    fn scroll_event(window_id: WindowId, abs: DVec2) -> Event {
        Event::Scroll(ScrollEvent {
            window_id,
            scroll: dvec2(0.0, -60.0),
            abs,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: 0.0,
            phase: ScrollPhase::None,
        })
    }

    #[test]
    fn splitter_panel_layout_mounts_headlessly() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        for path in [
            ids!(column_split),
            ids!(left_flow_split),
            ids!(left_running_split),
            ids!(canvas_right_split),
            ids!(right_source_split),
        ] {
            assert!(app.ui.widget(&cx, path).borrow::<Splitter>().is_some());
        }
        assert!(app.ui.widget(&cx, ids!(flow_list)).borrow::<FlowList>().is_some());
        assert!(app
            .ui
            .widget(&cx, ids!(running))
            .borrow::<RunningList>()
            .is_some());
        assert!(app.ui.widget(&cx, ids!(palette)).borrow::<Palette>().is_some());
        assert!(app
            .ui
            .widget(&cx, ids!(inspector))
            .borrow::<Inspector>()
            .is_some());
        assert!(app
            .ui
            .widget(&cx, ids!(source))
            .borrow::<TextInput>()
            .is_some());
        assert!(app
            .ui
            .widget(&cx, ids!(preview_value))
            .borrow::<faces::ValueView>()
            .is_some());
    }

    #[test]
    fn wire_menu_toggles_canvas_mode_and_checked_item() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));

        assert_eq!(app.wire_mode, WireMode::Routed);
        assert!(menu_label(&app, &cx, live_id!(wires_routed)).contains('✓'));
        app.handle_menu(&mut cx, live_id!(wires_bezier));
        assert_eq!(app.wire_mode, WireMode::Bezier);
        assert!(menu_label(&app, &cx, live_id!(wires_bezier)).contains('✓'));
        assert!(!menu_label(&app, &cx, live_id!(wires_routed)).contains('✓'));
        assert_eq!(
            app.ui
                .widget(&cx, ids!(canvas))
                .borrow::<FlowCanvas>()
                .unwrap()
                .wire_mode(),
            WireMode::Bezier
        );

        app.handle_menu(&mut cx, live_id!(wires_routed));
        assert_eq!(app.wire_mode, WireMode::Routed);
        assert!(menu_label(&app, &cx, live_id!(wires_routed)).contains('✓'));
    }

    #[test]
    fn image_viewer_owns_wheel_until_it_closes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        app.ui
            .widget(&cx, ids!(image_viewer))
            .borrow_mut::<ImageViewer>()
            .unwrap()
            .show(
                &mut cx,
                ImageViewerItem {
                    node: "image".into(),
                    port: "value".into(),
                    bytes: makepad_flow::ValueBytes {
                        digest: "test".into(),
                        content_type: "image/svg+xml".into(),
                        bytes: br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"/>"#
                            .as_slice()
                            .into(),
                    },
                },
            )
            .unwrap();

        let size = dvec2(1000.0, 700.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        draw_canvas_and_viewer(&mut cx, &mut app, &pass, &mut draw_list, size);
        draw_canvas_and_viewer(&mut cx, &mut app, &pass, &mut draw_list, size);
        let viewer_area = app.ui.widget(&cx, ids!(image_viewer)).area();
        let viewer_rect = viewer_area.rect(&cx);
        let centre = viewer_rect.pos + viewer_rect.size * 0.5;
        assert!(viewer_rect.size.x > 0.0 && viewer_rect.size.y > 0.0);
        let window_id = app.ui.window(&cx, ids!(main_window)).window_id().unwrap();

        let canvas_zoom = app
            .ui
            .widget(&cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .unwrap()
            .zoom();
        let viewer_zoom = app
            .ui
            .widget(&cx, ids!(image_viewer))
            .borrow::<ImageViewer>()
            .unwrap()
            .zoom();
        app.handle_event(&mut cx, &scroll_event(window_id, centre));
        assert_eq!(
            app.ui
                .widget(&cx, ids!(canvas))
                .borrow::<FlowCanvas>()
                .unwrap()
                .zoom(),
            canvas_zoom
        );
        assert_ne!(
            app.ui
                .widget(&cx, ids!(image_viewer))
                .borrow::<ImageViewer>()
                .unwrap()
                .zoom(),
            viewer_zoom
        );

        app.ui
            .widget(&cx, ids!(image_viewer))
            .borrow_mut::<ImageViewer>()
            .unwrap()
            .close(&mut cx);
        app.handle_event(&mut cx, &scroll_event(window_id, centre));
        assert_ne!(
            app.ui
                .widget(&cx, ids!(canvas))
                .borrow::<FlowCanvas>()
                .unwrap()
                .zoom(),
            canvas_zoom
        );
    }

    #[test]
    fn failed_input_put_restores_only_edits_that_were_not_replaced() {
        let old = ("input".to_string(), "text".to_string());
        let untouched = ("other".to_string(), "text".to_string());
        let mut pending = HashMap::from([(old.clone(), "newer".to_string())]);
        merge_failed_input_journal(
            &mut pending,
            HashMap::from([
                (old.clone(), "in-flight".to_string()),
                (untouched.clone(), "restore-me".to_string()),
            ]),
        );
        assert_eq!(pending.get(&old).map(String::as_str), Some("newer"));
        assert_eq!(pending.get(&untouched).map(String::as_str), Some("restore-me"));
    }

    #[test]
    fn output_save_extensions_follow_the_value_content_type() {
        assert_eq!(value_file_extension("image/png"), "png");
        assert_eq!(value_file_extension("image/jpeg; charset=binary"), "jpg");
        assert_eq!(value_file_extension("application/json"), "json");
        assert_eq!(value_file_extension("application/octet-stream"), "bin");
    }

    #[test]
    fn node_rename_rewrites_every_graph_reference() {
        let source = "use mod.flow.*\nlet source = Text{default: \"hi\"}\nlet result = Output{value: source.text()}\nFlow{source result}\n";
        let mut graph = makepad_flow::graph::evaluate(source, "<rename>").unwrap();
        rename_graph_node(&mut graph, "source", "prompt");

        assert!(graph.nodes.iter().any(|node| node.id == "prompt"));
        assert!(!graph.nodes.iter().any(|node| node.id == "source"));
        assert!(graph.edges.iter().all(|edge| {
            edge.from_node != "source" && edge.to_node != "source"
        }));
        assert!(graph.nodes.iter().flat_map(|node| &node.inputs).all(|input| {
            !matches!(&input.value, makepad_flow::NodeInputValue::Edge(edge) if edge.from_node == "source")
        }));
        assert!(graph.tools.iter().flat_map(|tool| {
            tool.inputs.iter().chain(&tool.outputs).chain(&tool.nodes)
        }).all(|node| node != "source"));
        assert!(graph.tools.iter().any(|tool| {
            tool.inputs.iter().chain(&tool.outputs).chain(&tool.nodes).any(|node| node == "prompt")
        }));
    }

    #[test]
    fn interleaved_run_ids_and_stale_start_responses_are_rejected() {
        let run = RunInfo {
            run_id: "run-a".into(),
            state: "running".into(),
            started: 0.0,
            finished_secs: None,
            revision: 7,
            planned_nodes: vec!["a".into()],
        };
        assert!(run_event_belongs(Some(&run), "node.progress", "run-a"));
        assert!(!run_event_belongs(Some(&run), "node.done", "run-b"));
        assert!(!run_event_belongs(Some(&run), "run.started", "run-b"));
        assert!(!run_event_belongs(None, "run.started", "run-b"));
        assert!(attachment_matches(Some("instance-a"), 4, "instance-a", 4));
        assert!(!attachment_matches(Some("instance-b"), 5, "instance-a", 4));
        assert!(request_generation_matches(7, 7));
        assert!(!request_generation_matches(8, 7));
    }

    #[test]
    fn pruned_progress_uses_only_planned_nodes_and_terminal_state_does_not_complete_it() {
        let planned = vec!["done".into(), "running".into(), "cancelled".into()];
        let statuses = HashMap::from([
            ("done".into(), NodeStatus::new("done", 1000, true, "", None)),
            (
                "running".into(),
                NodeStatus::new("running", 250, true, "", None),
            ),
            (
                "cancelled".into(),
                NodeStatus::new("cancelled", 0, false, "", None),
            ),
            (
                "not-planned".into(),
                NodeStatus::new("done", 1000, true, "", None),
            ),
        ]);
        assert_eq!(total_progress(&planned, &statuses), (1.25 / 3.0, 1, 3));
        assert_eq!(
            reconciled_node_state(RunState::Cancelled, NodeState::Running),
            NodeState::Cancelled
        );
        assert_eq!(
            reconciled_node_state(RunState::Failed, NodeState::Failed),
            NodeState::Failed
        );
    }

    #[test]
    fn pruned_plan_tracks_only_the_requested_output_ancestors() {
        let source = "use mod.flow.*\nlet a = Text{default: \"a\"}\nlet b = Text{default: \"b\"}\nlet left = Output{value: a.text()}\nlet right = Output{value: b.text()}\nFlow{a b left right}\n";
        let graph = makepad_flow::graph::evaluate(source, "<pruned-progress>").unwrap();
        assert_eq!(
            planned_nodes_for(Some(&graph), Some(&["left".to_string()])),
            vec!["a".to_string(), "left".to_string()]
        );
    }

    #[test]
    fn flow_change_waits_for_the_captured_run_revision() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        app.selected = Some("demo".into());
        app.run = Some(RunInfo {
            run_id: "run-a".into(),
            state: "running".into(),
            started: 0.0,
            finished_secs: None,
            revision: 7,
            planned_nodes: vec!["node".into()],
        });
        app.handle_flow_event(
            &mut cx,
            FlowEvent {
                topic: "flow".into(),
                kind: "flow.changed".into(),
                name: Some("demo".into()),
                revision: Some(8),
                ..Default::default()
            },
        );
        assert!(app.deferred_flow_reload);
    }

    #[test]
    fn gap_run_row_rebuild_reconciles_nonterminal_nodes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        app.run = Some(RunInfo {
            run_id: "run-a".into(),
            state: "running".into(),
            started: 0.0,
            finished_secs: None,
            revision: 7,
            planned_nodes: vec!["running".into(), "failed".into()],
        });
        app.apply_run_row(
            &mut cx,
            RunRowDto {
                run_id: "run-a".into(),
                instance: "instance-a".into(),
                flow: "demo".into(),
                revision: 7,
                state: RunState::Failed,
                planned_nodes: vec!["running".into(), "failed".into()],
                nodes: HashMap::from([
                    (
                        "running".into(),
                        NodeRowDto {
                            state: NodeState::Running,
                            progress: Some(500),
                            stage: Some("work".into()),
                            outputs: Vec::new(),
                            error: None,
                            text: Some("partial".into()),
                        },
                    ),
                    (
                        "failed".into(),
                        NodeRowDto {
                            state: NodeState::Failed,
                            progress: None,
                            stage: None,
                            outputs: Vec::new(),
                            error: Some("boom".into()),
                            text: None,
                        },
                    ),
                ]),
                outputs: HashMap::new(),
                http_log: Vec::new(),
                started_ms: 1_000,
                finished_ms: Some(2_000),
            },
        );
        let canvas = app.ui.widget(&cx, ids!(canvas));
        let canvas = canvas.borrow::<FlowCanvas>().unwrap();
        assert_eq!(canvas.statuses["running"].state, "cancelled");
        assert_eq!(canvas.statuses["failed"].state, "failed");
        assert_eq!(app.last_error.as_deref(), Some("boom"));
    }
}

fn format_eval_error(error: &makepad_flow::EvalError) -> String {
    format!("{}:{} {}", error.line, error.col, error.message)
}

fn instance_input_type(graph: &Graph, node_id: &str, port_name: &str) -> Option<PortType> {
    let node = graph.nodes.iter().find(|node| node.id == node_id)?;
    if node.kind == "input" || node.kind == "ask" {
        node.outputs
            .iter()
            .find_map(|port| (port.name == port_name).then_some(port.ty))
    } else {
        node.inputs
            .iter()
            .find_map(|port| (port.port == port_name).then_some(port.ty))
    }
}

fn input_value_json(ty: PortType, text: String) -> makepad_strict_json::Value {
    use makepad_strict_json::Value as Json;

    let payload = match ty {
        PortType::Text => ("text".to_string(), Json::Str(text)),
        PortType::Json | PortType::List => {
            let value = makepad_strict_json::parse_depth(text.as_bytes(), 32)
                .unwrap_or_else(|_| Json::Str(text));
            ("json".to_string(), value)
        }
        PortType::Image
        | PortType::Audio
        | PortType::Video
        | PortType::Mesh
        | PortType::Bytes => {
            let digest = if text.starts_with("sha256:") {
                text
            } else {
                format!("sha256:{text}")
            };
            ("digest".to_string(), Json::Str(digest))
        }
    };
    Json::Obj(vec![
        ("type".to_string(), Json::Str(ty.as_str().to_string())),
        payload,
    ])
}
