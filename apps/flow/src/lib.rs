//! Flow: makepad's generation studio and graph workbench, one window.
//!
//! STUDIO (View > Generation studio) is the studio page of
//! makepad-flow-studio: every preset workflow as a card, fired up as a job on
//! the AI Hub fleet, its results stored as files in the results folder
//! (`~/.makepad/flow/results`). GRAPHS (View > Graph workbench) is the
//! package workbench: flows, their canvas, faces, inspector, source and app
//! views, the running list, over an embedded flow server. `--headless` and
//! `--check` run a workflow package without a window (`makepad_flow_studio::
//! headless`); `--package` opens one in the workbench.

mod services;
mod assets;
mod components;

use crate::services::{BridgeContext, FlowServices, FlowUiAction};
pub use makepad_widgets;

mod appearance;
mod faces;
mod hierarchy;
use makepad_flow_ui::{graph_edit, graph_view, theme, values};
mod panels;
mod queue;
mod io;
mod service;
mod surface;
mod live;
mod frames;
use live::{LiveRuntime, LiveSnapshot};
use frames::FrameStageWidgetRefExt;
use io::WorkerLane;
use service::{ServiceEvent, ServiceHandle};
use surface::SurfaceView;
use makepad_flow::document::binding::Binding;
use makepad_flow::document::closure::Closure;
use makepad_flow::document::interface::FlowInterface;
use makepad_flow::document::package::Package;
use makepad_flow_studio::testpattern;
use makepad_flow_studio::FlowStudio;
mod viewer;

use faces::{model_choices, model_choices_for_node, BridgeCall, FaceBridgeCall, FaceHost, ModelChoice};
use makepad_flow::client::{
    ClientError, FlowClient, FlowSubscriber, FlowSubscriberConfig,
    SessionStatus, SubscriptionEvent,
};
use makepad_flow::{
    AssetsResponse, CreateBatchRequest, CreateBatchResponse, CreateInstanceRequest, CreateInstanceResponse, CreateRunResponse,
    Event as FlowEvent, FlowDefinition, FlowSummary,
    Graph, InstanceRow, Literal, ModelsResponse, NodeTypeCatalog, NodeState, NodesResponse,
    ParallelismResponse, PortType, PutFlowResponse, RunRowDto, RunState, TemplateSummary, ValueRef, ValueSlotRef,
};
use makepad_code_editor::code_view::CodeViewAction;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::makepad_micro_serde::{DeJson, JsonValue as MicroJsonValue};
use makepad_widgets::*;
use makepad_flowgraph::{
    Camera, CanvasEdit, CanvasEventPolicy, CanvasPick, FlowCanvas, FlowCanvasAction, NodeFacesScope, NodeStatus, Selection, FIRST_AT,
};
use makepad_flowgraph::wire_route::WireMode;
use panels::{
    FlowList, Inspector, InspectorAction, Palette, PaletteAction, RunBar, TemplatePicker,
};
use queue::{QueueAction, QueueList};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use values::ValueCache;
use viewer::{ImageViewer, ImageViewerAction, ImageViewerItem};

// `app_main!` emits the platform entry points. On desktop that includes a
// `fn main`, which is the process entry when the macro sits in a binary
// root. Here the `makepad-app-flow` binary owns the process entry (it
// decides between this window and the headless host first), so the macro
// lives in a module and `run` enters through that same generated `main`.
mod entry {
    use super::*;
    app_main!(App, font_assets: [MATH_VIEW_FONT_ASSET]);

    #[cfg(target_arch = "wasm32")]
    pub fn run() {
        main();
    }

    /// The window's app body, on a thread `run_app` already started.
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    pub fn on_app_thread() {
        app_main();
    }
}

/// The node types the Flow app runs.
pub fn registries() -> Result<(std::sync::Arc<makepad_flow::document::Registry>, std::sync::Arc<makepad_flow::engine::providers::ExecutorRegistry>), String> {
    makepad_flow_studio::nodes::registries_with(components::register)
}

/// Whether the window opens on the studio (the desktop binary says so).
static OPEN_STUDIO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The desktop entry: `--headless`/`--check` run a workflow package without
/// a window; otherwise the window opens (the HAL owns the main thread, the
/// app runs on its own).
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if makepad_flow_studio::headless::wants_headless(&args) {
        return makepad_flow_studio::headless::main(args, registries, "makepad-app-flow");
    }
    OPEN_STUDIO.store(true, std::sync::atomic::Ordering::Relaxed);
    makepad_widgets::makepad_platform::run_app(entry::on_app_thread);
    std::process::ExitCode::SUCCESS
}

/// The web build's entry: the window.
#[cfg(target_arch = "wasm32")]
pub fn run_window() {
    entry::run();
}

/// The template the app starts from when the root is empty.
const FIRST_TEMPLATE: &str = "prompt-to-image";
/// How often the hub's model lists are refreshed while a flow with generators is open.
const MODELS_REFRESH_SECS: f64 = 10.0;
const ASSET_PREVIEW_MAX_IN_FLIGHT: usize = 4;
const ASSET_PREVIEW_RETRY_SECS: f64 = 2.0;

const TEMPLATES_MENU: &str = "Templates";

/// The menu id of a template's entry; the name is the server's, so it is
/// stable across list refreshes.
fn template_menu_id(name: &str) -> LiveId {
    LiveId::from_str(&format!("template:{name}"))
}

fn grouped_new_menu_templates(mut templates: Vec<TemplateSummary>) -> Vec<TemplateSummary> {
    templates.sort_by(|left, right| {
        makepad_flow::templates::group_rank(&left.group)
            .cmp(&makepad_flow::templates::group_rank(&right.group))
            .then_with(|| left.name.cmp(&right.name))
    });
    for template in &mut templates {
        template.label = format!("{} · {}", template.group.to_ascii_uppercase(), template.label);
    }
    templates
}

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

const HELP_ABOUT: &str = "Flow — makepad's generation studio and graph-driven workflow app. A flow defines the app.
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
        margin: Inset{top: 1 bottom: 0}
        text: ""
        draw_text +: {
            color: theme.flow_text_subtle
            text_style: theme.font_bold{font_size: 8}
        }
    }

    let Panel = RoundedShadowView{
        width: Fill
        height: Fill
        flow: Down
        cursor: MouseCursor.Default
        grab_key_focus: false
        padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
        spacing: 3
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
        max_lines: 1
        text_overflow: Ellipsis
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

                    frame_stage := FrameStage{}
                    menu_bar := MenuBar{
                        menus: [
                            {label: "File" items: [
                                {id: @new_from_template label: "New from template…" shortcut: "Cmd+N"}
                                {id: @open_flow label: "Open next flow" shortcut: "Cmd+O"}
                                {sep: true}
                                {id: @open_package label: "Open package…" shortcut: "Shift+Cmd+O"}
                                {id: @reload_package label: "Reload package"}
                                {id: @close_package label: "Close package"}
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
                                {id: @view_studio label: "Generation studio"}
                                {id: @view_graphs label: "Graph workbench"}
                                {sep: true}
                                {id: @view_canvas label: "Canvas" shortcut: "Cmd+1"}
                                {id: @view_source label: "Source" shortcut: "Cmd+3"}
                                {id: @view_inspector label: "Inspector" shortcut: "Cmd+I"}
                                {id: @flip_card label: "Flip card" shortcut: "F"}
                                {sep: true}
                                {id: @toggle_left label: "Flows, Running and Palette" shortcut: "Cmd+L"}
                                {sep: true}
                                {id: @mode_app label: "App only" shortcut: "Shift+Cmd+1"}
                                {id: @mode_split label: "App and graph" shortcut: "Shift+Cmd+2"}
                                {id: @mode_graph label: "Graph only" shortcut: "Shift+Cmd+3"}
                                {id: @leave_container label: "Leave component" shortcut: "Shift+Cmd+L"}
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

                    shell_split := PanelSplitter{
                        width: Fill
                        height: Fill
                        axis: SplitterAxis.Horizontal
                        align: SplitterAlign.FromB(560.0)
                        min_vertical: 420.0
                        max_vertical: 320.0
                        // A folded app pane's bar is a handle, not a dead
                        // strip: dragging it out brings the pane back and
                        // the app answers by switching to Split.
                        bar_reopens: true
                        a: View{
                            width: Fill
                            height: Fill
                            flow: Down
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
                                            width: Fit{max: FitBound.Rel{base: Base.Parent, factor: 0.2}}
                                            height: Fit
                                            max_lines: 1
                                            text_overflow: Ellipsis
                                            margin: Inset{left: 10}
                                            text: "No flow open"
                                            draw_text +: {
                                                color: theme.flow_text
                                                text_style: theme.font_bold{font_size: 11}
                                            }
                                        }
                                        instance_chip := ToolText{width: Fit{max: FitBound.Rel{base: Base.Parent, factor: 0.12}}}
                                        design_btn := ButtonFlat{text: "Design" visible: false}
                                        View{width: 10 height: 1}
                                        run_btn := Button{text: "▶ Run"}
                                        parallel := ComboBox{
                                            width: 86
                                            height: 28
                                            labels: ["1", "max · 1"]
                                        }
                                        cancel_btn := ButtonFlat{text: "Cancel"}
                                        clear_btn := ButtonFlat{text: "Clear"}
                                        run_bar := RunBar{width: Fill{weight: 300. min: 40. max: 220.} height: 6 margin: Inset{left: 6 right: 6}}
                                        run_state := ToolText{}
                                        View{width: Fill{weight: 100.} height: 1}
                                        zoom_label := ToolText{text: "100 %"}
                                        fit_btn := ButtonFlat{text: "Fit"}
                                        side_btn := ButtonFlat{text: "Source"}
                                        new_btn := Button{text: "New"}
                                    }

                                    column_split := PanelSplitter{
                                        width: Fill
                                        height: Fill
                                        axis: SplitterAxis.Horizontal
                                        align: SplitterAlign.FromA(244.0)
                                        min_vertical: 180.0
                                        max_vertical: 520.0
                                        // Folded, the bar at the window's edge
                                        // brings the panels back with a drag.
                                        bar_reopens: true
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
                                                                running := QueueList{}
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
                                            // The inspector sits to the graph's left,
                                            // pane A of its own splitter; pane B is the
                                            // graph's room, empty here because the canvas
                                            // lies under this whole chrome overlay. When
                                            // the inspector is hidden this host is hidden
                                            // with it, so no folded bar is left behind.
                                            // `canvas_right_split` and `right_panel` are
                                            // the ids the app and its tests have always
                                            // used for this splitter and this pane; they
                                            // name the inspector's divider and pane now.
                                            inspector_host := View{
                                                width: Fill
                                                height: Fill
                                                canvas_right_split := PanelSplitter{
                                                    axis: SplitterAxis.Horizontal
                                                    align: SplitterAlign.FromA(300.0)
                                                    min_vertical: 220.0
                                                    max_vertical: 260.0
                                                    a: View{
                                                        width: Fill
                                                        height: Fill
                                                        right_panel := View{
                                                            width: Fill
                                                            height: Fill
                                                            clip_x: false
                                                            clip_y: false
                                                            padding: Inset{left: 4 right: 4 bottom: 8}
                                                            Panel{
                                                                inspector := Inspector{}
                                                            }
                                                        }
                                                    }
                                                    b: View{width: Fill height: Fill}
                                                }
                                            }
                                        }
                                    }
                                }

                                source_overlay := View{
                                    width: Fill
                                    height: Fill
                                    flow: Overlay
                                    visible: false
                                    source_card := Panel{
                                        width: 760
                                        height: 620
                                        align: Align{x: 0.5 y: 0.5}
                                        spacing: theme.space_2
                                        padding: Inset{left: 10 right: 10 top: 8 bottom: 10}
                                        head := View{
                                            width: Fill
                                            height: 28
                                            flow: Right
                                            align: Align{y: 0.5}
                                            SectionTitle{width: Fill text: "SOURCE"}
                                            save_btn := Button{text: "Save"}
                                            discard_source := ButtonFlat{text: "Discard draft" visible: false}
                                            close_source := ButtonFlat{text: "×"}
                                        }
                                        source := CodeView{
                                            editor +: {
                                                width: Fill
                                                height: Fill
                                                read_only: false
                                                show_gutter: true
                                                word_wrap: false
                                                draw_text +: {text_style: theme.font_code{font_size: 9}}
                                            }
                                        }
                                        View{
                                            width: Fill
                                            height: 18
                                            align: Align{x: 1.0 y: 1.0}
                                            resize_grip := Label{
                                                width: 18
                                                height: 18
                                                text: "◢"
                                                draw_text +: {color: theme.flow_text_muted text_style: theme.font_regular{font_size: 12}}
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
                        b: View{
                            width: Fill
                            height: Fill
                            flow: Down
                            app_pane := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                visible: false
                                padding: Inset{left: 4 right: 8 top: 4 bottom: 8}
                                Panel{
                                    surface_title := SectionTitle{text: "APP"}
                                    surface_error := Label{
                                        width: Fill
                                        height: Fit
                                        text: ""
                                        draw_text +: {
                                            color: theme.flow_text_muted
                                            text_style: theme.font_regular{font_size: 9}
                                        }
                                    }
                                    surface_view := SurfaceView{}
                                }
                            }
                        }
                    }
                    // After the workbench: a widget path the workbench names
                    // (its `canvas`) finds the workbench's first.
                    studio_page := View{
                        width: Fill
                        height: Fill
                        visible: false
                        studio := FlowStudioPage{}
                    }
                }
            }
        }
    }
}

enum IoResult {
    Flows(Result<Vec<FlowSummary>, ClientError>),
    LiveSnapshot {
        generation: u64,
        result: Result<Arc<LiveSnapshot>, ClientError>,
    },
    /// A candidate package's entry flow was installed under `name`.
    PackagePut {
        generation: u64,
        name: String,
        result: Result<PutFlowResponse, ClientError>,
    },
    /// A candidate package's instance was created.
    PackageInstance {
        generation: u64,
        name: String,
        result: Result<CreateInstanceResponse, ClientError>,
    },
    /// A retired package's instance and definition are gone.
    PackageRetired(Result<(), ClientError>),
    /// A superseded candidate's instance and definition are gone.
    PackageCleaned(Result<(), ClientError>),
    /// A committed session's entry module was written on the persist lane.
    PackageSaved {
        session: u64,
        result: Result<Package, String>,
    },
    PackageReloadDefinition {
        session: u64,
        generation: u64,
        epoch: u64,
        result: Result<FlowDefinition, ClientError>,
    },
    Flow {
        name: String,
        generation: u64,
        result: Result<FlowDefinition, ClientError>,
    },
    Saved {
        name: String,
        generation: u64,
        source: String,
        result: Result<FlowDefinition, ClientError>,
    },
    /// A flow was asked for from a template. `automatic` is the request's
    /// own origin (the empty-root default rather than the user), captured
    /// when it was made: a reply is never classified by name or by what
    /// the window remembers, so ordering cannot confuse two creations.
    Created {
        name: String,
        automatic: bool,
        package_generation: u64,
        result: Result<PutFlowResponse, ClientError>,
    },
    GraphPut {
        name: String,
        generation: u64,
        result: Result<FlowDefinition, ClientError>,
    },
    Catalog {
        generation: u64,
        result: Result<(NodesResponse, Arc<makepad_flow::document::Registry>), ClientError>,
    },
    Templates(Result<Vec<TemplateSummary>, ClientError>),
    Models {
        domain: String,
        result: Result<ModelsResponse, ClientError>,
    },
    Parallelism {
        flow: String,
        result: Result<ParallelismResponse, ClientError>,
    },
    BatchCreated {
        flow: String,
        planned_nodes: Vec<String>,
        revision: u64,
        result: Result<CreateBatchResponse, ClientError>,
    },
    QueueRows {
        generation: u64,
        result: Result<Vec<RunRowDto>, ClientError>,
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
    Assets {
        generation: u64,
        query: String,
        namespace: Option<String>,
        next_page: bool,
        merge_existing: bool,
        preserve_cursor: bool,
        result: Result<AssetsResponse, ClientError>,
    },
    AssetContent { id: String, result: Result<makepad_flow::ValueBytes, ClientError> },
    AssetThumbnail {
        alias: String,
        result: Result<makepad_flow::ValueBytes, ClientError>,
    },
    Done(Result<(), ClientError>),
}

type InputKey = (String, String);
type InputJournal = HashMap<InputKey, String>;

#[derive(Clone)]
struct RunRequest {
    outputs: Option<Vec<String>>,
}

/// Completions the UI may fall behind by before a worker waits for it.
const IO_COMPLETIONS_MAX: usize = 1_024;

#[derive(Default)]
struct IoMailbox {
    #[cfg(test)]
    template_requests: Vec<(String, String)>,
    sender: Option<SyncSender<IoResult>>,
    receiver: Option<Receiver<IoResult>>,
    fetching_flows: bool,
    fetching_instances: bool,
    fetching_live: bool,
    fetching_models: HashSet<String>,
    fetching_parallelism: bool,
    fetching_queue: bool,
}

impl IoMailbox {
    fn start(&mut self) {
        let (sender, receiver) = sync_channel(IO_COMPLETIONS_MAX);
        self.sender = Some(sender);
        self.receiver = Some(receiver);
    }
}

/// Which panes the shell shows once a package is open. Without a package
/// the window is the graph editor alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum ShellMode {
    /// The app surface alone.
    App,
    /// The graph editor alone.
    #[default]
    Graph,
    /// App on the right, editor on the left.
    Split,
}

/// The open package: one flow installed under its namespaced name, one
/// live instance that the graph, the surface, the inspector and the run
/// bar all address.
struct PackageSession {
    /// This commit's number: the identity every save of this session
    /// carries, so an open attempt that comes and goes cannot disturb it.
    session: u64,
    package: Package,
    /// The definition name the entry flow is installed under: owned by this
    /// window for this open, see `candidate_name`.
    name: String,
    /// The live instance, once created.
    instance: Option<String>,
    /// A save of this session in flight on the persist lane, and whether
    /// the source moved on meanwhile (saved again once this one lands).
    saving: bool,
    save_dirty: bool,
    save_failed: bool,
    /// Accepted writes may overlap on the control lane. Reload reads the
    /// settled definition only after every reply has been consumed.
    writes_pending: usize,
    write_epoch: u64,
    reload: Option<PackageReload>,
    /// The surface source currently mounted, if any.
    mounted_surface: Option<String>,
}

enum PackageReload {
    Waiting,
    Loading,
    Ready { epoch: u64, source: String, modules: Vec<makepad_flow::document::closure::ModuleSource> },
    Opening { epoch: u64 },
}

/// A package being opened beside the open one: installed and instantiated
/// under its own name, committed only once its instance exists and its
/// surface evaluated. Superseded or failed, whatever it created is retired.
struct PackageCandidate {
    package: Package,
    name: String,
    /// Set once the instance exists but the commit had to wait (for the
    /// node catalog); the commit is retried when that arrives.
    instance: Option<String>,
    graph: Option<Graph>,
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
    /// The generation studio: its page shows instead of the workbench while
    /// `studio_shown`.
    #[rust]
    studio: FlowStudio,
    #[rust]
    studio_shown: bool,
    #[rust]
    studio_timer: Timer,
    /// The service thread that owns the embedded server, the connector
    /// and the store; the UI never waits on it.
    #[rust]
    service: Option<ServiceHandle>,
    /// Bounded worker lanes: control-plane calls, and value downloads.
    #[rust]
    control_lane: WorkerLane,
    #[rust]
    data_lane: WorkerLane,
    /// One thread: package writes land in submission order, so an older
    /// session's save can never replace a newer one's.
    #[rust]
    persist_lane: WorkerLane,
    /// The connector's status and client as the service last published
    /// them; retained here, read without any lock.
    #[rust]
    session_status: Option<SessionStatus>,
    #[rust]
    client_handle: Option<Arc<Mutex<FlowClient>>>,
    #[rust]
    subscriber: Option<FlowSubscriber>,
    #[rust]
    poll_timer: Timer,
    /// The live host of this process and its audio device. It follows
    /// instances, not what the window shows.
    #[rust]
    live_runtime: LiveRuntime,
    #[rust]
    live_snapshot: Option<Arc<LiveSnapshot>>,
    #[rust]
    live_refresh_at: f64,
    #[rust]
    live_snapshot_error: Option<String>,
    /// Runs only while something runs live: advances the host and feeds
    /// the surface's meters and readouts.
    #[rust]
    live_timer: Timer,
    #[rust]
    frame_next: NextFrame,
    /// The last endpoint write the live host refused, shown once.
    #[rust]
    live_refusal: Option<String>,
    #[rust]
    io: IoMailbox,
    #[rust]
    flows: Vec<FlowSummary>,
    #[rust]
    templates: Vec<TemplateSummary>,
    #[rust]
    catalog: Vec<NodeTypeCatalog>,
    #[rust]
    registry: Option<Arc<makepad_flow::document::Registry>>,
    /// The hub's model ids by domain, from `GET /v1/models?domain=`.
    #[rust]
    models: HashMap<String, Vec<ModelChoice>>,
    #[rust]
    models_fetched_at: f64,
    #[rust]
    parallel_max: u64,
    #[rust]
    parallel_use_max: bool,
    #[rust]
    queue_generation: u64,
    #[rust]
    queue_refresh_after_stale: bool,
    #[rust]
    assets_request_generation: u64,
    #[rust]
    asset_preview_inflight: HashSet<String>,
    #[rust]
    asset_preview_retry_at: HashMap<String, f64>,
    #[rust]
    selected: Option<String>,
    #[rust]
    definition: Option<FlowDefinition>,
    /// Last authoritative graph, kept separate from optimistic canvas edits.
    #[rust]
    accepted_graph: Option<Graph>,
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
    run_ordinals: HashMap<String, u64>,
    #[rust]
    faces: Option<FaceHost>,
    #[rust]
    values: ValueCache,
    #[rust]
    run: Option<RunInfo>,
    #[rust]
    /// Every port value a run published, by qualified node, in the port's
    /// shape: an ordered port is one grouped entry, never its last element.
    outputs: HashMap<String, Vec<(String, ValueSlotRef)>>,
    #[rust]
    cleared_instances: HashSet<String>,
    #[rust]
    unsaved: bool,
    /// What `unsaved` last told the platform (`restart::set_unsaved`): a session restart waits.
    #[rust]
    unsaved_published: bool,
    #[rust]
    pending_source_edit: Option<(String, String, std::time::Instant)>,
    #[rust]
    source_save_pending: bool,
    /// A draft made during a graph write needs an explicit overwrite decision.
    #[rust]
    source_layout_conflict: bool,
    #[rust]
    connected_server: Option<[u8; 16]>,
    #[rust]
    connected_client: Option<usize>,
    #[rust]
    catalog_generation: u64,
    #[rust]
    embedded: bool,
    #[rust]
    auto_opened: bool,
    /// After a state-preserving restart (makepad run live session): the flow that was open, opened
    /// again in place of the automatic choice once the flow list is in.
    #[rust]
    restart_flow: Option<String>,
    #[rust]
    #[rust]
    palette_drop: Option<((f64, f64), String, String, PortType)>,
    #[rust]
    pending_inputs: HashMap<(String, String), String>,
    #[rust]
    input_flush_in_flight: bool,
    #[rust]
    deferred_run: Option<RunRequest>,
    #[rust]
    deferred_batch: bool,
    #[rust]
    deferred_flow_reload: bool,
    #[rust]
    pending_params: HashMap<(String, String), Literal>,
    #[rust]
    pending_graph: bool,
    #[rust]
    graph_write_layout_only: bool,
    #[rust]
    graph_write_generation: u64,
    /// Editor text before the write, retained until PUT and authoritative GET finish.
    #[rust]
    graph_write_source: Option<String>,
    /// Router-chosen facing per graph level. Never part of an authored write.
    #[rust]
    display_flips: HashMap<Vec<String>, HashMap<String, bool>>,
    #[rust]
    source_mode: bool,
    #[rust]
    source_geometry_ready: bool,
    #[rust]
    source_applied_geometry: Option<Rect>,
    #[rust]
    source_origin: DVec2,
    #[rust]
    source_size: DVec2,
    #[rust]
    source_drag: Option<(DVec2, bool)>,
    #[rust]
    left_hidden: bool,
    #[rust]
    inspector_hidden: bool,
    #[rust]
    wire_mode: WireMode,
    #[rust]
    preview_digest: Option<(String, String)>,
    #[rust]
    preview_ref: Option<ValueRef>,
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
    menu_state: (bool, bool, bool, bool, bool, bool, bool),
    #[rust]
    menu_state_initialized: bool,
    /// Splitter position remembered while the left pane is collapsed.
    #[rust]
    left_panel_align: Option<SplitterAlign>,
    #[rust]
    package: Option<PackageSession>,
    /// Whether the open package's app pane is showing; browsing another
    /// flow hides it, reselecting the package's flow shows it again.
    #[rust]
    package_shown: bool,
    #[rust]
    candidate: Option<PackageCandidate>,
    /// Commits so far; the next session takes the next number.
    #[rust]
    package_sessions: u64,
    /// The attached instance's label, for the chip.
    #[rust]
    instance_label: Option<String>,
    /// The definition revision each recorded output was produced under.
    #[rust]
    output_revisions: HashMap<String, u64>,
    /// The container path the graph pane shows (root = empty).
    #[rust]
    path: Vec<String>,
    /// Inspection and pointer capture inside an embedded graph do not navigate.
    #[rust]
    nested_selection: Option<(Vec<String>, Selection)>,
    #[rust]
    nested_capture: Option<Vec<String>>,
    /// One camera per path, saved when a level is left.
    #[rust]
    cameras: HashMap<Vec<String>, Camera>,
    /// Every node status by qualified id (`a/greet`), so the level shown can
    /// be replayed and a container's state derived from its inner nodes.
    #[rust]
    statuses: HashMap<String, (String, u16, bool, String, Option<String>)>,
    /// One stage clock per running node: the hub's raw stage becomes plain
    /// words with the share done and the time left (see `progress_eta`).
    #[rust]
    node_clocks: HashMap<String, makepad_ai_services::progress_eta::ProgressTracker>,
    /// A graph write the lane refused, kept for the next tick with the
    /// revision it was made against; a newer accepted write or another
    /// flow drops it.
    #[rust]
    graph_retry: Option<(String, u64, Graph, bool)>,
    /// The selected node per container path, restored on return.
    #[rust]
    selections: HashMap<Vec<String>, Option<Selection>>,
    /// Retirements (definition, instance) the lane refused, kept for the
    /// next tick.
    #[rust]
    cleanup_retry: Vec<(String, Option<String>)>,
    /// Graph writes skipped because the graph was already the server's.
    #[rust]
    redundant_graph_commits: u64,
    /// A package directory being opened on a worker; ignored once
    /// `package_generation` moved on.
    #[rust]
    package_task: Option<(u64, makepad_widgets::makepad_platform::thread::TaskHandle<Result<Package, String>>)>,
    #[rust]
    package_generation: u64,
    /// A package named on the command line, opened once the server is up.
    #[rust]
    package_open_pending: Option<std::path::PathBuf>,
    #[rust]
    package_dialog_open: bool,
    #[rust]
    shell_mode: ShellMode,
}

/// The smallest and largest side `--window-size` accepts, in logical pixels.
const WINDOW_SIDE: std::ops::RangeInclusive<f64> = 200.0..=4096.0;

/// A window's logical size from `<width>x<height>`, for example `900x600`.
pub(crate) fn parse_window_size(text: &str) -> Result<Vec2d, String> {
    let (width, height) = text
        .split_once(|c| c == 'x' || c == 'X')
        .ok_or_else(|| format!("`{text}` is not <width>x<height>"))?;
    let side = |name: &str, value: &str| -> Result<f64, String> {
        let number: f64 = value.trim().parse().map_err(|_| format!("the {name} `{value}` is not a number"))?;
        if !number.is_finite() || !WINDOW_SIDE.contains(&number) {
            return Err(format!("the {name} `{value}` is outside {}..{}", WINDOW_SIDE.start(), WINDOW_SIDE.end()));
        }
        Ok(number)
    };
    Ok(dvec2(side("width", width)?, side("height", height)?))
}

impl App {
    fn startup(&mut self, cx: &mut Cx) {
        self.io.start();
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            if arg == "--package" {
                self.package_open_pending = args.next().map(std::path::PathBuf::from);
            } else if arg == "--window-size" || arg.starts_with("--window-size=") {
                // The size is its own argument or follows `=`; another
                // option is never taken for it.
                let text = match arg.strip_prefix("--window-size=") {
                    Some(text) => Some(text.to_string()),
                    None => args.next_if(|next| !next.starts_with("--")),
                };
                match text.as_deref().map(parse_window_size) {
                    Some(Ok(size)) => self.ui.window(cx, ids!(main_window)).resize(cx, size),
                    Some(Err(error)) => eprintln!("flow: --window-size: {error}; the window keeps its default size"),
                    None => eprintln!("flow: --window-size needs <width>x<height>; the window keeps its default size"),
                }
            }
        }
        // The desktop binary opens on the studio unless a package or the
        // workbench was asked for; the web build and tests on the workbench.
        let workbench = std::env::args().skip(1)
            .any(|arg| arg == "--package" || arg.starts_with("--package=") || arg == "--flow-workbench");
        self.show_studio(cx, OPEN_STUDIO.load(std::sync::atomic::Ordering::Relaxed) && !workbench);
        self.studio_timer = cx.start_interval(0.05);
        self.poll_timer = cx.start_interval(0.25);
        self.control_lane.start("flow-io", 4, 256);
        self.data_lane.start("flow-data", 2, 64);
        self.persist_lane.start("flow-persist", 1, 64);
        // The media admission budget scales with the machine: a quarter of
        // the process memory envelope, never under 256 MiB.
        let media_max_bytes = (cx.memory_budget_bytes() / 4).max(256 * 1024 * 1024);
        self.service = Some(service::start(media_max_bytes));
        self.drain_service(cx);
        self.update_connection(cx);
        self.set_wire_mode(cx, self.wire_mode);
        self.set_modes(cx);
    }

    fn drain_service(&mut self, cx: &mut Cx) {
        while let Some(event) = self.service.as_mut().and_then(ServiceHandle::poll) {
            match event {
                ServiceEvent::Started { embedded } => self.embedded = embedded,
                ServiceEvent::Session { status, client } => {
                    self.session_status = Some(status);
                    self.client_handle = client;
                    self.update_connection(cx);
                }
                ServiceEvent::Failed(error) => self.set_error(cx, &error),
            }
        }
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
        self.client_handle.clone()
    }

    /// The open package's live instance is the attached one: the graph is
    /// editable and every view writes to that instance.
    fn live_package(&self) -> bool {
        self.package
            .as_ref()
            .and_then(|session| session.instance.as_deref())
            .is_some_and(|instance| self.instance.as_deref() == Some(instance))
    }

    fn live_package_instance(&self) -> Option<String> {
        self.live_package().then(|| self.instance.clone()).flatten()
    }

    /// An attached run is a locked snapshot; a package's live instance is
    /// not.
    fn editing_locked(&self) -> bool {
        (self.instance.is_some() && !self.live_package()) || !self.path.is_empty()
    }

    fn with_surface<R>(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut SurfaceView) -> R) -> Option<R> {
        let widget = self.ui.widget(cx, ids!(surface_view));
        let mut surface = widget.borrow_mut::<SurfaceView>()?;
        Some(f(cx, &mut surface))
    }

    // -- containers: the level shown --------------------------------------

    /// `a/shout/` for the path `[a, shout]`; empty at the root.
    fn level_prefix(&self) -> String {
        if self.path.is_empty() {
            String::new()
        } else {
            format!("{}/", self.path.join("/"))
        }
    }

    /// The id of a qualified node as the level shown draws it, if the node
    /// belongs to that level.
    fn level_node(&self, node: &str) -> Option<String> {
        let prefix = self.level_prefix();
        let rest = node.strip_prefix(prefix.as_str())?;
        (!rest.contains('/')).then(|| rest.to_string())
    }

    /// The container of the level shown that a qualified node lies under.
    fn level_container(&self, node: &str) -> Option<String> {
        let prefix = self.level_prefix();
        let rest = node.strip_prefix(prefix.as_str())?;
        rest.split_once('/').map(|(head, _)| head.to_string())
    }

    fn root_graph(&self) -> Option<&Graph> {
        self.definition.as_ref()?.graph.as_ref()
    }

    /// The component the path ends in; `None` at the root or when the
    /// path no longer resolves in the current definition.
    fn level_alias(&self) -> Option<String> { self.alias_at(&self.path) }

    fn alias_at(&self, path: &[String]) -> Option<String> {
        let root = self.root_graph()?;
        let mut graph = root;
        let mut alias = None;
        for id in path {
            let node = graph.nodes.iter().find(|node| node.id == *id && node.kind == "component")?;
            let def = root.components.iter().find(|def| def.alias == node.type_name)?;
            alias = Some(def.alias.clone());
            graph = &def.graph;
        }
        alias
    }

    /// The graph the pane shows: the root, or the component graph at the
    /// path, with the root's component list so its own containers resolve.
    fn view_graph(&self) -> Option<Graph> {
        let root = self.root_graph()?;
        let mut graph = if self.path.is_empty() {
            root.clone()
        } else {
            let alias = self.level_alias()?;
            let def = root.components.iter().find(|def| def.alias == alias)?;
            let mut graph = def.graph.clone();
            graph.components = root.components.clone();
            graph
        };
        if let Some(flips) = self.display_flips.get(&self.path) {
            for node in &mut graph.nodes {
                if let Some(flip) = flips.get(&node.id) {
                    node.flip = *flip;
                }
            }
        }
        Some(graph)
    }

    /// The closure the open definition evaluates with.
    fn closure(&self) -> Closure {
        Closure { modules: self.definition.as_ref().map(|definition| definition.modules.clone()).unwrap_or_default() }
    }

    fn imported_module_notice(&self) -> String {
        let module = self
            .level_alias()
            .and_then(|alias| self.closure().module(&alias).map(|module| module.path.clone()))
            .unwrap_or_else(|| "its module".to_string());
        format!("this graph is imported from {module}; edit that file and repin the package")
    }

    /// Open a container's inner graph, keeping this level's camera.
    fn enter_container(&mut self, cx: &mut Cx, id: String) {
        let Some(graph) = self.view_graph() else {
            return;
        };
        if !graph.nodes.iter().any(|node| node.id == id && node.kind == "component") {
            return;
        }
        self.remember_level(cx);
        self.path.push(id);
        self.show_level(cx);
    }

    fn leave_container(&mut self, cx: &mut Cx) {
        if self.path.is_empty() {
            return;
        }
        self.remember_level(cx);
        self.path.pop();
        self.show_level(cx);
    }

    /// The level shown, as it is now: its camera and its selection, kept
    /// for the return.
    fn remember_level(&mut self, cx: &mut Cx) {
        if let Some(canvas) = self.ui.widget(cx, ids!(canvas)).borrow::<FlowCanvas>() {
            self.cameras.insert(self.path.clone(), canvas.camera());
            self.selections.insert(self.path.clone(), canvas.selection().cloned());
        }
    }

    /// Show the level the path names: its graph, its faces, every status
    /// and value it already has, and its camera (a first visit fits).
    fn show_level(&mut self, cx: &mut Cx) {
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
        }
        self.show_graph(cx);
        self.remount_faces(cx);
        self.replay_level(cx);
        let camera = self.cameras.get(&self.path).copied();
        let selection = self.selections.get(&self.path).cloned().flatten();
        // What the canvas selects, the app selects too (menus, source
        // jumps), as when the user selected it.
        self.selected_node = selection.as_ref().and_then(Selection::node).map(str::to_string);
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            match camera {
                Some(camera) => canvas.set_camera(cx, camera),
                None => canvas.reset_view(cx),
            }
            FlowCanvas::set_selection(&mut canvas, cx, selection);
        }
        self.refresh_breadcrumb(cx);
        self.update_attachment_lock(cx);
        self.refresh_inspector(cx);
        self.update_run_bar(cx);
    }

    /// Every status and value the level shown already has, into the canvas
    /// and the faces just mounted.
    fn replay_level(&mut self, cx: &mut Cx) {
        let statuses: Vec<(String, (String, u16, bool, String, Option<String>))> =
            self.statuses.iter().map(|(node, status)| (node.clone(), status.clone())).collect();
        let mut containers = HashSet::new();
        for (node, (state, permille, has_progress, stage, error)) in statuses {
            if let Some(local) = self.level_node(&node) {
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    canvas.set_status(cx, &local, NodeStatus::new(&state, permille, has_progress, &stage, error));
                }
                if let Some(faces) = self.faces.as_mut() {
                    faces.push_state(cx, &local, &state);
                }
            } else if let Some(container) = self.level_container(&node) {
                containers.insert(container);
            }
        }
        for container in containers {
            self.refresh_container_status(cx, &container);
        }
        let outputs: Vec<(String, Vec<(String, ValueSlotRef)>)> =
            self.outputs.iter().map(|(node, ports)| (node.clone(), ports.clone())).collect();
        for (node, ports) in outputs {
            let Some(local) = self.level_node(&node) else {
                continue;
            };
            for (port, slot) in ports {
                match slot {
                    ValueSlotRef::One(value) => {
                        let bytes = self.values.get(&value.digest);
                        if let Some(faces) = self.faces.as_mut() {
                            faces.push_value(cx, &local, &port, &value, bytes.as_ref());
                        }
                        self.fill_boundary_face(cx, &local, &port, &value);
                    }
                    ValueSlotRef::Seq(sequence) => {
                        if let Some(faces) = self.faces.as_mut() {
                            faces.push_sequence(cx, &local, &port, &sequence);
                        }
                    }
                }
            }
        }
        // Bytes the faces just mounted ask for (previews) are fetched or
        // delivered now, not only when the next value arrives.
        self.request_wanted_values(cx);
    }

    /// A boundary input of the level shown (an `Input` node inside a
    /// container) shows the value that crossed into it as its live value;
    /// its authored default stays what the definition says.
    fn fill_boundary_face(&mut self, cx: &mut Cx, local: &str, port: &str, value: &ValueRef) {
        if self.path.is_empty() || value.ty != PortType::Text {
            return;
        }
        let is_input = self
            .view_graph()
            .is_some_and(|graph| graph.nodes.iter().any(|node| node.id == local && node.kind == "input"));
        if !is_input {
            return;
        }
        // The preview may be cut short; the value's bytes (fetched under the
        // cache's bound, delivered by `drain_values`) replace it on arrival.
        let text = match self.values.get(&value.digest) {
            Some(bytes) => String::from_utf8_lossy(&bytes.bytes).into_owned(),
            None => {
                if let Some(client) = self.client() {
                    self.values.request(&value.digest, client, &self.data_lane);
                }
                match value.preview.as_ref() {
                    Some(Literal::Str(text)) => text.clone(),
                    _ => return,
                }
            }
        };
        if let Some(faces) = self.faces.as_mut() {
            faces.fill_input_value(cx, local, port, &text);
        }
    }

    fn inspector_shows_edge(&self, cx: &mut Cx) -> bool {
        matches!(self.inspection(cx).1, Some(Selection::Edge { .. }))
    }

    /// The qualified output the source of a wire at the level shown
    /// publishes: `(node, port)` as the run names it. A container's port is
    /// its output boundary node, whose one port is `value`.
    fn wire_source_key_at(&self, path: &[String], graph: Option<&Graph>, from_node: &str, from_port: &str) -> (String, String) {
        let prefix = hierarchy::prefix(path);
        let container = graph
            .is_some_and(|graph| graph.nodes.iter().any(|node| node.id == from_node && node.kind == "component"));
        if container {
            (format!("{prefix}{from_node}/{from_port}"), "value".to_string())
        } else {
            (format!("{prefix}{from_node}"), from_port.to_string())
        }
    }

    /// Whether the inspector shows `node`'s (qualified) values: as the
    /// selected node under the level shown, or as the source a selected
    /// wire probes.
    fn inspector_depends_on(&self, cx: &mut Cx, node: &str) -> bool {
        let (path, selection, graph) = self.inspection(cx);
        match selection {
            Some(Selection::Node(local)) => format!("{}{local}", hierarchy::prefix(&path)) == node,
            Some(Selection::Edge { from_node, from_port, .. }) =>
                self.wire_source_key_at(&path, graph.as_ref(), &from_node, &from_port).0 == node,
            None => false,
        }
    }

    /// Boundary input faces whose crossing value is `digest` show its text
    /// now that the bytes are here.
    fn refill_boundary_faces(&mut self, cx: &mut Cx, digest: &str) {
        if self.path.is_empty() {
            return;
        }
        let matching: Vec<(String, String, ValueRef)> = self
            .outputs
            .iter()
            .filter_map(|(node, ports)| self.level_node(node).map(|local| (local, ports)))
            .flat_map(|(local, ports)| {
                ports
                    .iter()
                    .filter_map(|(port, slot)| slot.one().map(|value| (port, value)))
                    .filter(|(_, value)| value.digest == digest)
                    .map(move |(port, value)| (local.clone(), port.clone(), value.clone()))
            })
            .collect();
        for (local, port, value) in matching {
            self.fill_boundary_face(cx, &local, &port, &value);
        }
    }

    /// A container's shown state is derived from its inner nodes: failed
    /// if any failed, running if any runs or waits, done once all that
    /// have a status are done, cancelled if any was, pending otherwise.
    fn refresh_container_status(&mut self, cx: &mut Cx, container: &str) {
        let prefix = format!("{}{container}/", self.level_prefix());
        let mut failed = None;
        let mut running = false;
        let mut pending = false;
        let mut cancelled = false;
        let mut done = 0usize;
        let mut total = 0usize;
        for (node, (state, _, _, _, error)) in &self.statuses {
            if !node.starts_with(&prefix) {
                continue;
            }
            total += 1;
            match state.as_str() {
                "failed" => failed = Some(error.clone().unwrap_or_else(|| format!("{node} failed"))),
                "running" | "waiting" | "ready" => running = true,
                "done" => done += 1,
                "pending" => pending = true,
                "cancelled" => cancelled = true,
                _ => {}
            }
        }
        let (state, error) = if let Some(error) = failed {
            ("failed", Some(error))
        } else if running {
            ("running", None)
        } else if total > 0 && done == total {
            ("done", None)
        } else if cancelled {
            ("cancelled", None)
        } else if pending || total > 0 {
            ("pending", None)
        } else {
            return;
        };
        let permille = if total > 0 { (done * 1000 / total) as u16 } else { 0 };
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_status(cx, container, NodeStatus::new(state, permille, state == "running", "", error));
        }
    }

    fn refresh_breadcrumb(&mut self, cx: &mut Cx) {
        let Some(title) = self.flow_title() else {
            return;
        };
        let text = breadcrumb_text(&title, &self.path);
        self.ui.label(cx, ids!(flow_name)).set_text(cx, &text);
    }

    /// What the toolbar calls the open flow: the package's display name
    /// when the open flow is the package session's (its definition name is
    /// this window's `app-<nonce>`, nothing a person recognises), else the
    /// definition name.
    fn flow_title(&self) -> Option<String> {
        let name = self.selected.as_deref()?;
        let package_title = self
            .package
            .as_ref()
            .filter(|session| session.name == name)
            .map(|session| session.package.manifest.name.trim())
            .filter(|title| !title.is_empty());
        Some(package_title.unwrap_or(name).to_string())
    }

    // -- packages ------------------------------------------------------------

    /// A definition name this server does not have, owned by this window
    /// for one open. It is never derived from the package id: opening a
    /// package can neither overwrite another flow nor be overwritten by an
    /// unrelated open of the same id, and two opens of one package are two
    /// definitions until the older is retired.
    fn candidate_name(&self) -> String {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        loop {
            let nonce = RandomState::new().build_hasher().finish() as u32;
            let name = format!("app-{nonce:08x}");
            let taken = self.flows.iter().any(|flow| flow.name == name)
                || self.package.as_ref().is_some_and(|session| session.name == name)
                || self.candidate.as_ref().is_some_and(|candidate| candidate.name == name);
            if !taken {
                return name;
            }
        }
    }

    /// Open a package directory. It is read and validated on a worker (the
    /// same reading, confinement and role checks the windowless host
    /// applies), installed and instantiated under its own name beside the
    /// open package, and replaces it only once its instance exists and its
    /// surface evaluated. Until then, and after any failure, the open
    /// package stays exactly as it is. A newer open supersedes an older
    /// candidate; what the older one created is retired as its replies
    /// come in.
    fn open_package(&mut self, cx: &mut Cx, dir: std::path::PathBuf) {
        self.package_generation = self.package_generation.wrapping_add(1);
        self.abandon_candidate();
        if let Some(open) = self.package.as_mut() {
            open.reload = None;
            if open.package.root == dir {
                open.save_failed = false;
                open.reload = Some(PackageReload::Waiting);
                self.continue_package_reload(cx);
                return;
            }
        }
        self.read_package(cx, dir);
    }

    fn read_package(&mut self, cx: &mut Cx, dir: std::path::PathBuf) {
        let generation = self.package_generation;
        match cx.task_pool().submit(
            makepad_widgets::makepad_platform::thread::Lane::Heavy,
            move || open_package_dir(&dir),
        ) {
            Ok(task) => self.package_task = Some((generation, task)),
            Err(error) => self.package_open_failed(cx, &format!("cannot open the package: {error}")),
        }
    }

    fn drain_package_task(&mut self, cx: &mut Cx) {
        let Some((generation, task)) = self.package_task.as_mut() else {
            return;
        };
        let generation = *generation;
        let Some(result) = task.try_take() else {
            return;
        };
        self.package_task = None;
        if generation != self.package_generation {
            return;
        }
        let package = match result {
            Ok(Ok(package)) => package,
            Ok(Err(error)) => return self.package_open_failed(cx, &error),
            Err(error) => return self.package_open_failed(cx, &format!("package open failed: {error}")),
        };
        if self.client().is_none() {
            return self.package_open_failed(cx, "not connected to a flow server yet");
        }
        let name = self.candidate_name();
        let source = package.entry_source.clone();
        // The closure is resolved here (order, budgets, unknown aliases,
        // cycles) and refused with its file and line; nothing is installed
        // for a package whose modules do not resolve.
        let closure = match package.closure() {
            Ok(closure) => closure,
            Err(error) => {
                return self.package_open_failed(
                    cx,
                    &format!("{}:{}:{}: {}", error.file, error.line, error.col, error.message),
                )
            }
        };
        self.candidate = Some(PackageCandidate { package, name: name.clone(), instance: None, graph: None });
        let taken = self.io(move |client| {
            let result = client.put_source_with(&name, &source, &closure.modules, true);
            IoResult::PackagePut { generation, name, result }
        });
        if !taken {
            self.candidate = None;
            self.package_open_failed(cx, "the request lane is full; open the package again in a moment");
        }
    }

    /// Keep an explicit package failure visible. A stale flow listing must
    /// not auto-open the failed candidate after its definition is retired
    /// and replace the useful error with "flow not found".
    fn package_open_failed(&mut self, cx: &mut Cx, message: &str) {
        self.auto_opened = true;
        if let Some(open) = self.package.as_mut() {
            open.reload = None;
        }
        self.set_error(cx, message);
    }

    fn package_writes_waiting(&self) -> bool {
        let Some(open) = self.package.as_ref() else { return false };
        open.writes_pending != 0
            || self.graph_retry.as_ref().is_some_and(|(name, _, _, _)| *name == open.name)
            || self.pending_source_edit.as_ref().is_some_and(|(name, _, _)| *name == open.name)
            || (self.selected.as_deref() == Some(open.name.as_str()) && !self.pending_params.is_empty())
    }

    /// One session-owned reload, with one read or save in flight. Lane
    /// refusals wait for room; actual I/O failures cancel the intent.
    fn continue_package_reload(&mut self, cx: &mut Cx) {
        if self.package_writes_waiting() { return }
        let Some(open) = self.package.as_ref() else { return };
        match open.reload.as_ref() {
            Some(PackageReload::Waiting) => {
                if !self.control_lane.can_accept() { return }
                let (session, epoch, name) = (open.session, open.write_epoch, open.name.clone());
                let generation = self.package_generation;
                if self.io(move |client| IoResult::PackageReloadDefinition {
                    session, generation, epoch, result: client.flow(&name),
                }) {
                    self.package.as_mut().unwrap().reload = Some(PackageReload::Loading);
                }
            }
            Some(PackageReload::Ready { epoch, source, modules }) => {
                if *epoch != open.write_epoch {
                    self.package.as_mut().unwrap().reload = Some(PackageReload::Waiting);
                } else if !open.saving {
                    if open.package.matches_definition(source, modules) {
                        let (epoch, dir) = (*epoch, open.package.root.clone());
                        self.package.as_mut().unwrap().reload = Some(PackageReload::Opening { epoch });
                        self.read_package(cx, dir);
                    } else if !open.save_failed && self.persist_lane.can_accept() {
                        let (source, modules) = (source.clone(), modules.clone());
                        self.save_package_definition(source, modules);
                    }
                }
            }
            _ => {}
        }
    }

    fn package_write_started(&mut self, name: &str) {
        if let Some(open) = self.package.as_mut().filter(|open| open.name == name) {
            open.writes_pending += 1;
            open.write_epoch = open.write_epoch.wrapping_add(1);
            open.save_failed = false;
        }
    }

    fn package_write_finished(&mut self, name: &str, succeeded: bool) {
        let mut cancel_reload = false;
        if let Some(open) = self.package.as_mut().filter(|open| open.name == name) {
            open.writes_pending = open.writes_pending.saturating_sub(1);
            if !succeeded {
                cancel_reload = open.reload.take().is_some();
            }
        }
        if cancel_reload {
            self.package_generation = self.package_generation.wrapping_add(1);
            self.abandon_candidate();
        }
    }

    /// The candidate's definition is installed and evaluated: create its
    /// instance. Its first inputs are what the open package's instance
    /// holds now, where the candidate has the same typed port (a reload
    /// keeps what was typed), then the manifest defaults for the rest.
    fn package_put_done(&mut self, name: String, response: PutFlowResponse) {
        let generation = self.package_generation;
        if !self.candidate.as_ref().is_some_and(|candidate| candidate.name == name) {
            return self.discard_candidate(name, None);
        }
        let interface = FlowInterface::of(&response.graph);
        let (same_package, defaults) = {
            let candidate = self.candidate.as_ref().unwrap();
            (
                self.package
                    .as_ref()
                    .is_some_and(|session| session.package.root == candidate.package.root),
                candidate.package.manifest.defaults.clone(),
            )
        };
        let mut inputs: HashMap<String, HashMap<String, makepad_flow::InputValueDto>> = HashMap::new();
        if same_package {
            if let Some(row) = self.instance_row.as_ref() {
                // Carry only an unambiguous value that fits the new port's
                // complete shape, including a reduced sequence bound.
                for input in &interface.inputs {
                    let scalar = row.inputs.get(&input.node).and_then(|ports| ports.get(&input.port));
                    let sequence = row.ordered_inputs.as_ref()
                        .and_then(|nodes| nodes.get(&input.node))
                        .and_then(|ports| ports.get(&input.port));
                    let value = match (input.many, scalar, sequence) {
                        (None, Some(value), None) if input.ty == value.ty => carried_input(value),
                        (Some(bound), None, Some(sequence)) if input.ty == sequence.ty
                            && sequence.values.len() <= bound as usize
                            && sequence.validate().is_ok() => makepad_flow::InputValueDto {
                                ty: sequence.ty.clone(),
                                text: None,
                                json: None,
                                digest: None,
                                values: Some(sequence.values.iter().map(carried_input).collect()),
                            },
                        _ => continue,
                    };
                    inputs.entry(input.node.clone()).or_default().insert(input.port.clone(), value);
                }
            }
        }
        for (node, literal) in &defaults {
            let Some(input) = interface.inputs.iter().find(|input| &input.node == node) else {
                continue;
            };
            let ports = inputs.entry(node.clone()).or_default();
            if ports.contains_key(&input.port) {
                continue;
            }
            // The manifest's scalar defaults cannot override ordered ports.
            if input.many.is_some() {
                continue;
            }
            let text = match literal {
                Literal::Str(text) | Literal::Id(text) => text.clone(),
                Literal::Num(number) => number.to_string(),
                Literal::Bool(value) => value.to_string(),
                _ => continue,
            };
            ports.insert(
                input.port.clone(),
                makepad_flow::InputValueDto { ty: input.ty.clone(), text: Some(text), json: None, digest: None, values: None },
            );
        }
        if let Some(candidate) = self.candidate.as_mut() {
            candidate.graph = Some(response.graph);
        }
        let request = CreateInstanceRequest {
            label: Some("app".to_string()),
            inputs: (!inputs.is_empty()).then_some(inputs),
            pin: Some(false),
        };
        self.io(move |client| {
            let result = client.create_instance(&name, &request);
            IoResult::PackageInstance { generation, name: name.clone(), result }
        });
    }

    /// The candidate's instance exists: evaluate its surface, and only if
    /// that succeeds retire the open package and make the candidate the
    /// open one. A surface that fails refuses the candidate; the open
    /// package keeps its own surface, values and instance.
    fn package_instance_done(&mut self, cx: &mut Cx, name: String, instance: String) {
        if !self.candidate.as_ref().is_some_and(|candidate| candidate.name == name) {
            return self.discard_candidate(name, Some(instance));
        }
        // A user can keep editing while the replacement loads. Such an
        // edit must settle and reach disk before that candidate can commit.
        if self.package.as_ref().is_some_and(|open| matches!(open.reload,
            Some(PackageReload::Opening { epoch }) if epoch != open.write_epoch
                || self.package_writes_waiting()))
        {
            self.candidate = None;
            self.discard_candidate(name, Some(instance));
            self.package.as_mut().unwrap().reload = Some(PackageReload::Waiting);
            return;
        }
        if self.catalog.is_empty() || self.registry.is_none() {
            // The face widgets need the node registry; the commit waits for
            // it and is retried when it arrives.
            if let Some(candidate) = self.candidate.as_mut() {
                candidate.instance = Some(instance);
            }
            return;
        }
        let candidate = self.candidate.take().unwrap();
        let mut mounted = None;
        if let (Some(source), Some(graph)) = (candidate.package.surface_source.clone(), candidate.graph.as_ref()) {
            let file = candidate.package.surface_file.clone().unwrap_or_else(|| "ui.splash".to_string());
            let catalog = self.catalog.clone();
            let registry = self.registry.clone().unwrap();
            let graph = graph.clone();
            let outcome = self.with_surface(cx, |cx, view| view.mount(cx, &instance, &file, &source, &graph, &catalog, &registry));
            match outcome {
                Some(Ok(())) => mounted = Some(source),
                Some(Err(error)) => {
                    self.discard_candidate(name, Some(instance));
                    return self.package_open_failed(cx, &format!("package surface `{file}`: {error}"));
                }
                None => {
                    self.discard_candidate(name, Some(instance));
                    return self.package_open_failed(cx, "the app pane is missing");
                }
            }
        }
        // Commit: only now does the open package's work go.
        self.retire_package(cx);
        if candidate.package.surface_source.is_none() {
            self.with_surface(cx, |cx, view| view.unmount(cx));
        }
        self.package_sessions = self.package_sessions.wrapping_add(1);
        self.package = Some(PackageSession {
            session: self.package_sessions,
            package: candidate.package,
            name: name.clone(),
            instance: Some(instance.clone()),
            saving: false,
            save_dirty: false,
            save_failed: false,
            writes_pending: 0,
            write_epoch: 0,
            reload: None,
            mounted_surface: mounted,
        });
        self.package_shown = true;
        if self.shell_mode == ShellMode::Graph {
            self.shell_mode = ShellMode::Split;
            // The split app view starts with room for its graph. The two
            // editor panels remain available from the View menu.
            self.left_hidden = true;
            self.inspector_hidden = true;
        }
        self.open_package_flow(cx, name, instance);
        self.ui.label(cx, ids!(surface_error)).set_text(cx, "");
        self.set_modes(cx);
    }

    /// Give up the current candidate. What it already materialized (its
    /// definition once the PUT answered, its instance once created, even one
    /// waiting for the catalog) is retired now; a reply still on its way is
    /// retired by the stale-reply path when it lands.
    fn abandon_candidate(&mut self) {
        let Some(candidate) = self.candidate.take() else {
            return;
        };
        if candidate.graph.is_some() || candidate.instance.is_some() {
            self.discard_candidate(candidate.name, candidate.instance);
        }
    }

    /// A candidate that was superseded or refused: its instance (if it got
    /// one) and its definition are deleted, off the UI thread.
    fn discard_candidate(&mut self, name: String, instance: Option<String>) {
        let (kept_name, kept_instance) = (name.clone(), instance.clone());
        if let Some(instance) = instance.as_deref() {
            self.live_runtime.stop(instance, self.connected_server);
        }
        let taken = self.io(move |client| {
            if let Some(instance) = instance {
                let _ = client.delete_instance(&instance);
            }
            IoResult::PackageCleaned(client.delete(&name))
        });
        if !taken {
            self.cleanup_retry.push((kept_name, kept_instance));
        }
    }

    /// Work the lanes refused earlier, tried again while there is room: a
    /// package save, a graph write, a retirement.
    fn retry_deferred_work(&mut self, cx: &mut Cx) {
        if self.package.as_ref().is_some_and(|open| open.save_dirty && !open.saving) {
            self.write_back_package(cx);
        }
        if self.graph_retry.is_some() && !self.pending_graph && self.control_lane.can_accept() {
            if let Some((name, revision, graph, layout_only)) = self.graph_retry.take() {
                // Only against the revision it was made for, on the same flow.
                if self.selected.as_deref() == Some(name.as_str()) && self.definition_revision() == Some(revision) {
                    self.submit_graph_write(cx, name, graph, layout_only);
                }
            }
        }
        // One bounded pass over what was held: each item is offered once;
        // the first refusal (a full lane, no client) ends the pass and the
        // rest waits for the next tick, deduplicated by definition.
        let held = std::mem::take(&mut self.cleanup_retry);
        let mut rest = Vec::with_capacity(held.len());
        let mut offered = HashSet::new();
        let mut refused = false;
        for (name, instance) in held {
            if !offered.insert(name.clone()) {
                continue;
            }
            if refused || !self.control_lane.can_accept() {
                rest.push((name, instance));
                continue;
            }
            let (kept_name, kept_instance) = (name.clone(), instance.clone());
            let taken = self.io(move |client| {
                if let Some(instance) = instance {
                    let _ = client.delete_instance(&instance);
                }
                IoResult::PackageCleaned(client.delete(&name))
            });
            if !taken {
                refused = true;
                rest.push((kept_name, kept_instance));
            }
        }
        rest.extend(std::mem::take(&mut self.cleanup_retry));
        self.cleanup_retry = rest;
    }

    /// Open the package's flow with its live instance attached from the
    /// start, so the definition load and the instance fetch carry the same
    /// attachment generation.
    fn open_package_flow(&mut self, cx: &mut Cx, name: String, instance: String) {
        self.clear_attachment(cx);
        self.selected = Some(name.clone());
        self.definition = None;
        self.unsaved = false;
        self.deferred_flow_reload = false;
        self.revisions.clear();
        self.redo.clear();
        self.path.clear();
        self.cameras.clear();
        self.display_flips.clear();
        self.selections.clear();
        self.graph_retry = None;
        self.instance = Some(instance);
        self.refresh_breadcrumb(cx);
        self.set_error(cx, "");
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
            canvas.reset_view(cx);
        }
        self.update_attachment_lock(cx);
        self.update_run_bar(cx);
        self.show_flow_list(cx);
        self.refresh_flows();
        self.load_flow(name);
        self.fetch_instance();
    }

    /// Retire the open package's work: its instance (and the runs it
    /// holds) and its definition are deleted on the server, off the UI
    /// thread. Deterministic: the session is gone here, whatever the
    /// deletes answer later. The surface is the caller's to replace.
    fn retire_package(&mut self, cx: &mut Cx) {
        let Some(session) = self.package.take() else {
            return;
        };
        self.package_shown = false;
        if let Some(instance) = session.instance.as_deref() {
            if self.instance.as_deref() == Some(instance) {
                self.clear_attachment(cx);
            }
        }
        let name = session.name;
        let instance = session.instance;
        self.retire_on_server(name, instance);
    }

    /// Delete a definition and its instance on the lane; a refused
    /// submission keeps the pair for the next tick, never dropped.
    fn retire_on_server(&mut self, name: String, instance: Option<String>) {
        let (kept_name, kept_instance) = (name.clone(), instance.clone());
        if let Some(instance) = instance.as_deref() {
            self.live_runtime.stop(instance, self.connected_server);
        }
        let taken = self.io(move |client| {
            let retired = match instance {
                Some(instance) => client.delete_instance(&instance),
                None => Ok(()),
            };
            IoResult::PackageRetired(retired.and_then(|()| client.delete(&name)))
        });
        if !taken {
            self.cleanup_retry.push((kept_name, kept_instance));
        }
    }

    fn close_package(&mut self, cx: &mut Cx) {
        self.package_generation = self.package_generation.wrapping_add(1);
        self.abandon_candidate();
        self.retire_package(cx);
        self.with_surface(cx, |cx, view| view.unmount(cx));
        self.shell_mode = ShellMode::Graph;
        self.ui.label(cx, ids!(surface_error)).set_text(cx, "");
        self.set_modes(cx);
    }

    /// Bind the package's surface to the definition as it is now. The
    /// surface was mounted at commit; a commit that had to wait for the
    /// catalog mounts here, keeping the last good one on failure. On an
    /// accepted revision change every readout starts empty and only values
    /// of the new revision are fed.
    fn refresh_surface(&mut self, cx: &mut Cx, revision_changed: bool) {
        if !self.live_package() {
            return;
        }
        if revision_changed {
        }
        let (Some(definition), Some(session)) = (self.definition.as_ref(), self.package.as_ref()) else {
            return;
        };
        let Some(graph) = definition.graph.as_ref() else {
            return;
        };
        let Some(registry) = self.registry.clone() else {
            return;
        };
        if self.catalog.is_empty() {
            return;
        }
        let revision = definition.revision;
        let instance = session.instance.clone().unwrap_or_default();
        let surface = session.package.surface_source.clone();
        let surface_file = session.package.surface_file.clone().unwrap_or_else(|| "ui.splash".to_string());
        let mounted = session.mounted_surface.clone();
        let interface = FlowInterface::of(graph);
        // What the instance's running live plan admits; empty until one is
        // installed, and this runs again when that happens.
        let endpoints = self.live_runtime.declared(&instance);
        let live_status = self.live_runtime.status_line(&instance);
        let frames = self.frame_sources();
        let graph = graph.clone();
        let catalog = self.catalog.clone();
        let mut error = None;
        let mut newly_mounted = None;
        let bindings: Vec<Binding> = self
            .with_surface(cx, |cx, view| {
                if let Some(source) = surface.as_ref() {
                    if mounted.as_deref() != Some(source.as_str()) || !view.is_mounted() {
                        match view.mount(cx, &instance, &surface_file, source, &graph, &catalog, &registry) {
                            Ok(()) => newly_mounted = Some(source.clone()),
                            Err(message) => error = Some(message),
                        }
                    } else if revision_changed {
                        view.clear_values(cx);
                    }
                }
                let bindings = view.apply_interface(cx, &interface, &endpoints).to_vec();
                // A frame may still be preparing; its view shows that state
                // locally and is rebound when the owner installs the plan.
                view.configure_live_widgets(cx, &frames);
                bindings
            })
            .unwrap_or_default();
        if let (Some(source), Some(session)) = (newly_mounted, self.package.as_mut()) {
            session.mounted_surface = Some(source);
        }
        let unbound: Vec<String> = bindings
            .iter()
            .filter(|binding| !binding.bound())
            .map(|binding| match &binding.reason {
                Some(reason) => format!("{}.{} ({reason})", binding.node, binding.port),
                None => format!("{}.{}", binding.node, binding.port),
            })
            .collect();
        let text = match (error, unbound.is_empty()) {
            (Some(error), _) => format!("surface: {error}"),
            (None, false) => format!("unbound: {}", unbound.join(", ")),
            (None, true) => String::new(),
        };
        let text = [text, live_status].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n");
        self.ui.label(cx, ids!(surface_error)).set_text(cx, &text);
        if let Some(row) = self.instance_row.clone() {
            self.with_surface(cx, |cx, view| view.fill_inputs(cx, &row));
        }
        let outputs = self.outputs.clone();
        for (node, ports) in &outputs {
            if self.output_revisions.get(node) != Some(&revision) {
                continue;
            }
            for (port, slot) in ports {
                match slot {
                    ValueSlotRef::One(value) => {
                        let bytes = self.values.get(&value.digest);
                        self.with_surface(cx, |cx, view| view.push_value(cx, node, port, value, bytes.as_ref()));
                    }
                    ValueSlotRef::Seq(sequence) => {
                        self.with_surface(cx, |cx, view| view.push_sequence(cx, node, port, sequence));
                    }
                }
            }
        }
        self.request_wanted_values(cx);
    }

    /// The package directory is the shareable artefact: an accepted graph
    /// edit that changed the entry source is written back there on the
    /// persist lane, atomically, and counted as saved only once that
    /// landed. The save belongs to the committed session, not to any open
    /// attempt: a candidate that comes and goes cannot strand it. Saves
    /// coalesce per session (one in flight, the newest source after it) and
    /// the lane's one thread lands every write in submission order. A
    /// source the server could not evaluate is not written; the package
    /// keeps its last good module.
    fn write_back_package(&mut self, cx: &mut Cx) {
        if self.pending_graph || self.graph_retry.is_some() { return; }
        let Some(open) = self.package.as_ref() else {
            return;
        };
        let Some(definition) = self.definition.as_ref() else {
            return;
        };
        if self.selected.as_deref() != Some(open.name.as_str())
            || open.reload.is_some()
            || open.save_failed
            || definition.error.is_some()
            || definition.graph.is_none()
            || open.package.matches_definition(&definition.source, &definition.modules)
        {
            return;
        }
        if open.saving {
            if let Some(open) = self.package.as_mut() {
                open.save_dirty = true;
            }
            return;
        }
        let (source, modules) = (definition.source.clone(), definition.modules.clone());
        self.save_package_definition(source, modules);
        let _ = cx;
    }

    fn save_package_definition(&mut self, source: String, modules: Vec<makepad_flow::document::closure::ModuleSource>) {
        let Some(open) = self.package.as_ref() else { return };
        let session = open.session;
        let package = open.package.clone();
        let taken = self.persist(move || {
            let result = package.save_definition(&source, &modules).map_err(|error| error.to_string());
            IoResult::PackageSaved { session, result }
        });
        if let Some(open) = self.package.as_mut() {
            // A refused write is tried again at the next accepted edit or
            // save completion; nothing is counted as saved meanwhile.
            open.saving = taken;
            open.save_dirty = !taken;
        }
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
            self.statuses.clear();
            if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                canvas.clear_run(cx);
            }
            if let Some(faces) = self.faces.as_mut() {
                faces.reset_run();
            }
            self.with_surface(cx, |_, view| view.reset_run());
            self.refresh_inspector(cx);
        }
        self.update_run_bar(cx);
    }

    fn detach_faces(&mut self, cx: &mut Cx) {
        self.nested_selection = None;
        self.nested_capture = None;
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_roots(cx, Vec::new());
            canvas.set_embedded_roots(cx, Vec::new());
        }
        if let Some(faces) = self.faces.take() {
            faces.free(cx);
        }
    }

    fn clear_attachment(&mut self, cx: &mut Cx) {
        self.detach_faces(cx);
        self.accepted_graph = None;
        self.close_preview(cx);
        self.attachment_generation = self.attachment_generation.wrapping_add(1);
        self.graph_write_generation = self.graph_write_generation.wrapping_add(1);
        self.pending_graph = false;
        self.graph_write_source = None;
        self.graph_retry = None;
        self.pending_source_edit = None;
        self.source_save_pending = false;
        self.source_layout_conflict = false;
        self.ui.button(cx, ids!(discard_source)).set_visible(cx, false);
        self.run_request_generation = self.run_request_generation.wrapping_add(1);
        self.run_start_in_flight = false;
        self.instance = None;
        self.instance_label = None;
        self.instance_row = None;
        self.run = None;
        self.outputs.clear();
        self.output_revisions.clear();
        self.statuses.clear();
        self.current_node = None;
        self.pending_inputs.clear();
        self.input_flush_in_flight = false;
        self.deferred_run = None;
        self.update_attachment_lock(cx);
    }

    /// The sole instance ownership transition. Old isolate roots are removed
    /// before its VM is freed; only a known matching definition may remount.
    fn attach_instance(&mut self, cx: &mut Cx, instance: Option<String>, mount: bool) {
        if self.instance == instance {
            if mount && self.faces.is_none() {
                self.remount_faces(cx);
            }
            self.update_attachment_lock(cx);
            return;
        }
        self.clear_attachment(cx);
        self.instance = instance;
        if mount && self.definition.is_some() {
            self.remount_faces(cx);
        }
        self.update_attachment_lock(cx);
    }

    fn update_attachment_lock(&mut self, cx: &mut Cx) {
        let locked = self.editing_locked();
        self.ui.button(cx, ids!(design_btn)).set_visible(cx, locked);
        if let Some(faces) = self.faces.as_mut() {
            faces.set_locked(cx, locked);
        }
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_faces_locked(cx, locked);
        }
        if let Some(mut inspector) = self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>() {
            inspector.set_locked(cx, locked || self.nested_selection.is_some());
        }
        self.refresh_instance_chip(cx);
        self.ui.redraw(cx);
    }

    /// The one place the chip's text comes from, so no later label update
    /// can contradict the editing state.
    fn refresh_instance_chip(&mut self, cx: &mut Cx) {
        let text = if !self.path.is_empty() {
            "component · imported".to_string()
        } else if self.editing_locked() {
            format!("viewing {} · locked", self.instance_label.as_deref().unwrap_or("a run"))
        } else if self.live_package() {
            "package · live".to_string()
        } else {
            "design".to_string()
        };
        self.ui.label(cx, ids!(instance_chip)).set_text(cx, &text);
    }

    fn focus_instance(&mut self, cx: &mut Cx, instance: String, flow: Option<String>) {
        if flow.as_deref() == self.selected.as_deref() && self.definition.is_some() {
            self.attach_instance(cx, Some(instance), true);
        } else {
            self.clear_attachment(cx);
            self.instance = Some(instance);
            self.update_attachment_lock(cx);
            self.definition = None;
            if let Some(flow) = flow {
                self.selected = Some(flow.clone());
                self.unsaved = false;
                self.revisions.clear();
                self.redo.clear();
                self.refresh_breadcrumb(cx);
                self.show_flow_list(cx);
                self.load_flow(flow);
            }
        }
        self.fetch_instance();
        self.refresh_instances();
    }

    /// Run one client call on the control lane; the result lands in the
    /// mailbox and wakes the UI. A full lane holds the call for the next
    /// tick; nothing is spawned per call. Returns whether the call was
    /// taken: a caller that marks work in flight marks it only then.
    fn io<F>(&mut self, f: F) -> bool
    where
        F: FnOnce(&FlowClient) -> IoResult + Send + 'static,
    {
        let Some(client) = self.client() else {
            return false;
        };
        self.task(move || match client.lock() {
            Ok(client) => f(&client),
            Err(_) => IoResult::Done(Err(ClientError::Protocol("flow client lock poisoned".into()))),
        })
    }

    /// Run one job without a client on the control lane. Returns whether it
    /// was taken.
    fn task<F>(&mut self, f: F) -> bool
    where
        F: FnOnce() -> IoResult + Send + 'static,
    {
        let Some(sender) = self.io.sender.clone() else {
            return false;
        };
        let accepted = self.control_lane.submit(Box::new(move || {
            let result = f();
            let _ = sender.send(result);
            SignalToUI::set_ui_signal();
        }));
        if !accepted {
            log!("flow: the control lane refused a job ({} refused so far)", self.control_lane.refused());
        }
        accepted
    }

    /// Run one package write on the persist lane: ordered, one at a time.
    /// Returns whether it was taken.
    fn persist<F>(&mut self, f: F) -> bool
    where
        F: FnOnce() -> IoResult + Send + 'static,
    {
        let Some(sender) = self.io.sender.clone() else {
            return false;
        };
        let accepted = self.persist_lane.submit(Box::new(move || {
            let result = f();
            let _ = sender.send(result);
            SignalToUI::set_ui_signal();
        }));
        if !accepted {
            log!("flow: the persist lane refused a write ({} refused so far)", self.persist_lane.refused());
        }
        accepted
    }

    fn update_connection(&mut self, cx: &mut Cx) {
        let Some(status) = self.session_status.clone() else {
            return;
        };
        let (text, color) = cx.with_vm(|vm| match &status {
            SessionStatus::Discovering => ("Discovering".to_string(), theme::state_color(vm, "cancelled")),
            SessionStatus::Connecting { .. } => ("Connecting…".to_string(), theme::state_color(vm, "waiting")),
            SessionStatus::Retrying { in_secs, .. } => {
                (format!("Retrying in {in_secs} s"), theme::state_color(vm, "failed"))
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
                theme::state_color(vm, "done"),
            ),
        });
        self.ui.label(cx, ids!(status_chip)).set_text(cx, &text);
        let mut dot = self.ui.view(cx, ids!(status_dot));
        script_apply_eval!(cx, dot, {draw_bg +: {color: #(color)}});
        if let SessionStatus::Connected { server_id, .. } = status {
            let client_key = self.client_handle.as_ref().map(|client| Arc::as_ptr(client) as usize);
            if self.connected_server != Some(server_id) || self.connected_client != client_key {
                self.live_snapshot = None;
                self.live_refresh_at = 0.0;
                self.connected_server = Some(server_id);
                self.connected_client = client_key;
                self.catalog_generation = self.catalog_generation.wrapping_add(1);
                self.registry = None;
                self.catalog.clear();
                if let Some(client) = self.client() {
                    self.services.connect(cx, client.clone());
                    self.subscriber =
                        FlowSubscriber::start(client, FlowSubscriberConfig::default()).ok();
                    self.refresh_flows();
                    let generation = self.catalog_generation;
                    self.io(move |client| IoResult::Catalog {
                        generation,
                        // Admission runs on the worker; the UI adopts the same
                        // immutable snapshot that gates this client's uploads.
                        result: client.nodes_catalog().and_then(|catalog| {
                            Ok((catalog, client.registry()?))
                        }),
                    });
                    self.io(|client| IoResult::Templates(client.templates()));
                    self.refresh_instances();
                    self.refresh_queue();
                }
                if let Some(dir) = self.package_open_pending.take() {
                    self.open_package(cx, dir);
                }
            }
        } else if self.connected_server.take().is_some() {
            self.connected_client = None;
            self.catalog_generation = self.catalog_generation.wrapping_add(1);
            self.services.disconnect();
            self.subscriber = None;
            self.registry = None;
            self.catalog.clear();
        }
    }

    fn refresh_flows(&mut self) {
        if self.io.fetching_flows || self.client().is_none() {
            return;
        }
        // Marked in flight only once the lane took it: a refused call
        // leaves nothing waiting for an answer that will not come.
        self.io.fetching_flows = self.io(|client| IoResult::Flows(client.flows()));
    }

    fn refresh_instances(&mut self) {
        if self.io.fetching_instances || self.client().is_none() {
            return;
        }
        self.io.fetching_instances = self.io(|client| IoResult::Instances(client.instances(None, false)));
    }

    fn refresh_live(&mut self) {
        let now = Cx::monotonic_now();
        if self.io.fetching_live || self.registry.is_none() || now < self.live_refresh_at {
            return;
        }
        let Some(client) = self.client() else { return };
        let previous = self.live_snapshot.clone();
        let generation = self.catalog_generation;
        // Dedicated completion even if the client is poisoned: the in-flight
        // marker must always be cleared. At most one snapshot is queued.
        self.io.fetching_live = self.task(move || {
            let result = match client.lock() {
                Ok(client) => LiveSnapshot::capture(&client, previous.as_deref()).map(Arc::new),
                Err(_) => Err(ClientError::Protocol("flow client lock poisoned".into())),
            };
            IoResult::LiveSnapshot { generation, result }
        });
        if self.io.fetching_live {
            self.live_refresh_at = now + 0.5;
        }
    }

    fn refresh_assets(
        &mut self,
        cx: &mut Cx,
        next_page: bool,
        merge_existing: bool,
        preserve_cursor: bool,
    ) {
        if self.client().is_none() {
            return;
        }
        let request = self
            .ui
            .widget(cx, ids!(inspector))
            .borrow_mut::<Inspector>()
            .map(|mut inspector| inspector.asset_request(cx, self.time, next_page, preserve_cursor));
        let Some((query, namespace, limit, cursor)) = request else { return };
        self.assets_request_generation = self.assets_request_generation.wrapping_add(1);
        let generation = self.assets_request_generation;
        self.io(move |client| {
            IoResult::Assets {
                generation,
                query: query.clone(),
                namespace: namespace.clone(),
                next_page,
                merge_existing,
                preserve_cursor,
                result: client.assets_page(&query, namespace.as_deref(), limit, cursor.as_deref()),
            }
        });
    }

    fn fetch_asset_thumbnail(&mut self, cx: &mut Cx, alias: String) {
        let text_preview = self
            .ui
            .widget(cx, ids!(inspector))
            .borrow::<Inspector>()
            .is_some_and(|inspector| inspector.asset_needs_text_preview(&alias));
        self.io(move |client| IoResult::AssetThumbnail {
            result: if text_preview {
                client.asset_preview(&alias)
            } else {
                client.asset_thumbnail(&alias)
            },
            alias,
        });
    }

    fn refresh_visible_asset_previews(&mut self, cx: &mut Cx) {
        let visible = self
            .ui
            .widget(cx, ids!(inspector))
            .borrow::<Inspector>()
            .map(|inspector| inspector.visible_asset_preview_ids(cx))
            .unwrap_or_default();
        let available = ASSET_PREVIEW_MAX_IN_FLIGHT
            .saturating_sub(self.asset_preview_inflight.len());
        for alias in visible.into_iter().take(available) {
            if self.asset_preview_inflight.contains(&alias)
                || self
                    .asset_preview_retry_at
                    .get(&alias)
                    .is_some_and(|retry_at| *retry_at > self.time)
            {
                continue;
            }
            self.asset_preview_inflight.insert(alias.clone());
            self.fetch_asset_thumbnail(cx, alias);
        }
    }

    fn open_asset(&mut self, cx: &mut Cx, asset: makepad_flow::FlowAsset) {
        self.preview_ref = None;
        self.preview_digest = Some((asset.id.clone(), asset.title.clone()));
        self.preview_bytes = None;
        if let Some(mut viewer) = self.ui.widget(cx, ids!(image_viewer)).borrow_mut::<ImageViewer>() {
            viewer.show_status(cx, &asset.title, "Loading original content…");
        }
        self.ui.view(cx, ids!(value_preview_view)).set_visible(cx, false);
        self.io(move |client| IoResult::AssetContent { result: client.asset_content(&asset.id), id: asset.id });
    }

    fn refresh_queue(&mut self) {
        if self.io.fetching_queue || self.client().is_none() {
            return;
        }
        let generation = self.queue_generation;
        self.io.fetching_queue = self.io(move |client| IoResult::QueueRows {
            generation,
            result: client.runs(None),
        });
    }

    fn refresh_parallelism(&mut self) {
        if self.io.fetching_parallelism || self.client().is_none() {
            return;
        }
        let Some(flow) = self.selected.clone() else {
            return;
        };
        self.io.fetching_parallelism = self.io(move |client| IoResult::Parallelism {
            result: client.parallelism(&flow),
            flow,
        });
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
        self.models_fetched_at = self.time;
        self.refresh_parallelism();
        for domain in domains {
            if self.io.fetching_models.contains(&domain) {
                continue;
            }
            let key = domain.clone();
            let taken = self.io(move |client| {
                let result = client.models(Some(&domain));
                IoResult::Models { domain, result }
            });
            if taken {
                self.io.fetching_models.insert(key);
            }
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

    fn save_source(&mut self, cx: &mut Cx, name: String, source: String, explicit: bool) {
        if self.editing_locked() {
            return;
        }
        if self.pending_graph || self.graph_retry.is_some() {
            self.set_error(cx, "the graph layout is still saving; source text is kept here, but wait for it to finish before saving source");
            return;
        }
        if self.source_layout_conflict {
            if !explicit { return; }
            self.source_layout_conflict = false;
            self.ui.button(cx, ids!(discard_source)).set_visible(cx, false);
        }
        if self.source_save_pending {
            self.pending_source_edit = Some((name, source, std::time::Instant::now()));
            return;
        }
        self.source_save_pending = true;
        if self.pending_source_edit.as_ref().is_some_and(|(pending_name, pending_source, _)| {
            *pending_name == name && *pending_source == source
        }) {
            self.pending_source_edit = None;
        }
        let (kept_name, kept_source) = (name.clone(), source.clone());
        let generation = self.attachment_generation;
        let taken = self.io(move |client| {
            let result = client.put_source(&name, &source).and_then(|_| client.flow(&name));
            IoResult::Saved {
                name,
                generation,
                source,
                result,
            }
        });
        if !taken {
            // Kept as the pending edit; the settle tick sends it again.
            self.source_save_pending = false;
            self.pending_source_edit = Some((kept_name, kept_source, std::time::Instant::now()));
        } else {
            self.package_write_started(&kept_name);
        }
    }

    /// The Templates menu lists every template the server serves, one entry
    /// each, alphabetical; choosing one creates a flow from it and opens it.
    /// Rebuilt whenever the template list arrives.
    fn refresh_templates_menu(&mut self, cx: &mut Cx) {
        let menu_bar = self.ui.menu_bar(cx, ids!(menu_bar));
        let mut menus: Vec<MenuDef> = menu_bar
            .borrow()
            .map(|bar| bar.menus().to_vec())
            .unwrap_or_default();
        if menus.is_empty() {
            return;
        }
        menus.retain(|menu| menu.label != TEMPLATES_MENU);
        let mut templates = self.templates.clone();
        templates.sort_by(|left, right| {
            makepad_flow::templates::group_rank(&left.group)
                .cmp(&makepad_flow::templates::group_rank(&right.group))
                .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
        });
        // One greyed heading per group, its templates indented beneath it.
        let mut items: Vec<makepad_widgets::menu_bar::MenuEntry> = Vec::new();
        let mut previous_group: Option<&str> = None;
        for template in &templates {
            if previous_group != Some(template.group.as_str()) {
                if previous_group.is_some() {
                    items.push(makepad_widgets::menu_bar::MenuEntry::separator());
                }
                let mut heading = makepad_widgets::menu_bar::MenuEntry::item(
                    LiveId::from_str(&format!("template-group:{}", template.group)),
                    template.group.as_str(),
                    None,
                );
                heading.enabled = false;
                items.push(heading);
                previous_group = Some(template.group.as_str());
            }
            items.push(makepad_widgets::menu_bar::MenuEntry::item(
                template_menu_id(&template.name),
                format!("  {}", template.label),
                None,
            ));
        }
        if items.is_empty() {
            menu_bar.set_menus(cx, menus);
            return;
        }
        // Second on the bar, right after File: the place a new flow starts.
        let at = menus.len().min(1);
        menus.insert(at, MenuDef::new(TEMPLATES_MENU, items));
        menu_bar.set_menus(cx, menus);
    }

    /// Ask for a flow from a template; its `Created` reply opens it.
    /// `automatic` marks the empty-root default (not a user request) and
    /// travels with the request, so its reply carries its own origin.
    fn create_from_template(&mut self, template: &str, automatic: bool) {
        let name = self.fresh_flow_name(template);
        let template = template.to_string();
        let package_generation = self.package_generation;
        #[cfg(test)]
        self.io.template_requests.push((name.clone(), template.clone()));
        self.io(move |client| {
            let result = client.create_from_template(&name, &template);
            IoResult::Created { name, automatic, package_generation, result }
        });
    }

    /// An explicit package open is pending (the `--package` argument before
    /// the server is up), reading, or installing: the window's intent.
    fn package_intent(&self) -> bool {
        self.package_open_pending.is_some() || self.package_task.is_some() || self.candidate.is_some()
            || self.package.as_ref().is_some_and(|open| open.reload.is_some())
    }

    fn open_flow(&mut self, cx: &mut Cx, name: String) {
        // The open package's flow is its live session; any other flow is
        // browsed with the app pane away, so no live-looking app stands
        // beside an unrelated graph.
        if let Some(session) = self.package.as_ref() {
            if session.name == name {
                if let Some(instance) = session.instance.clone() {
                    self.package_shown = true;
                    self.open_package_flow(cx, name, instance);
                    self.set_modes(cx);
                    return;
                }
            } else if self.package_shown {
                self.package_shown = false;
                self.set_modes(cx);
            }
        }
        self.clear_attachment(cx);
        self.selected = Some(name.clone());
        self.definition = None;
        self.unsaved = false;
        self.deferred_flow_reload = false;
        self.revisions.clear();
        self.redo.clear();
        self.path.clear();
        self.cameras.clear();
        self.display_flips.clear();
        self.selections.clear();
        self.graph_retry = None;
        self.ui.label(cx, ids!(flow_name)).set_text(cx, &name);
        self.set_error(cx, "");
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
            canvas.reset_view(cx);
        }
        self.update_run_bar(cx);
        self.show_flow_list(cx);
        self.load_flow(name);
    }

    /// Nothing open yet: the newest live instance's flow, else the
    /// prompt-to-image flow, else the first flow; an empty root gets the
    /// prompt-to-image template.
    fn maybe_auto_open(&mut self, cx: &mut Cx) {
        if self.auto_opened || self.selected.is_some() || self.io.fetching_flows || self.io.fetching_instances {
            return;
        }
        // An explicit package open is the window's intent: nothing is
        // created or opened for it automatically while that is under way
        // (a failed open comes back here), and an open package is the
        // selection.
        if self.package_intent() || self.package.is_some() {
            return;
        }
        self.auto_opened = true;
        if let Some(name) = self.restart_flow.take().filter(|n| self.flows.iter().any(|f| &f.name == n)) {
            self.open_flow(cx, name);
            return;
        }
        if self.flows.is_empty() {
            self.create_from_template(FIRST_TEMPLATE, true);
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
        self.put_graph_with(cx, graph, false);
    }

    /// A run view is a locked snapshot of the design's meaning, but the
    /// cards' layout stays the reader's: moving, sizing and flipping a card
    /// (`layout_only`) writes the file there too, as it does in design.
    fn put_graph_with(&mut self, cx: &mut Cx, graph: Graph, layout_only: bool) {
        if self.editing_locked() && !layout_only {
            return;
        }
        let Some(name) = self.selected.clone() else {
            return;
        };
        if !self.path.is_empty() && !layout_only {
            self.set_error(cx, &self.imported_module_notice());
            return;
        }
        // Layout edits are patched into the original source by the worker
        // and server; they never require rewriting the person's script.
        let source_owned = self.definition.as_ref().is_some_and(|definition| !definition.canonical);
        if self.unsaved || self.source_save_pending || self.pending_source_edit.is_some() || self.source_layout_conflict {
            self.show_graph(cx);
            self.set_error(cx, "source has unsaved or pending changes; save or discard that draft before editing the graph layout");
            return;
        }
        if source_owned && !layout_only {
            self.set_error(
                cx,
                "this flow contains authored code; nodes can be moved and resized, but change its wiring in the source",
            );
            return;
        }
        if self.definition.as_ref().and_then(|definition| definition.graph.as_ref()) == Some(&graph) {
            // Nothing to write: the server already has this graph.
            self.redundant_graph_commits += 1;
            return;
        }
        let Some(registry) = self.registry.as_ref() else {
            self.set_error(cx, "the host's node registry is not available yet");
            return;
        };
        let source = if source_owned {
            None
        } else {
            match makepad_flow::graph::write_with_registry(&graph, registry) {
                Ok(source) => Some(source),
                Err(error) => {
                    self.set_error(cx, &format!("cannot write this graph: {error}"));
                    return;
                }
            }
        };
        let remount = self
            .definition
            .as_ref()
            .and_then(|definition| definition.graph.as_ref())
            .is_none_or(|old| graph_edit::needs_face_remount(old, &graph));
        // One write at a time. New gestures compose on the newest optimistic
        // graph and replace only the held request, never an in-flight one.
        if self.pending_graph {
            let revision = self.definition_revision().unwrap_or(0);
            self.graph_retry = Some((name, revision, graph.clone(), layout_only));
        } else {
            self.submit_graph_write(cx, name, graph.clone(), layout_only);
        }
        if let Some(definition) = self.definition.as_mut() {
            // The canvas draws the edit at once; the server's re-evaluation
            // confirms or corrects it through flow.changed. The file text
            // follows the graph the way the server rewrites it, so a remount
            // mounts against a `Flow{}` that already lists a dropped node.
            definition.graph = Some(graph.clone());
            if let Some(source) = source {
                definition.source = source;
            }
        }
        self.show_graph(cx);
        if remount {
            self.remount_faces(cx);
        } else if layout_only {
            self.update_component_layouts(cx);
        }
        self.redo.clear();
    }

    fn submit_graph_write(&mut self, cx: &mut Cx, name: String, graph: Graph, layout_only: bool) {
        let (put_name, put_graph) = (name.clone(), graph.clone());
        let generation = self.graph_write_generation.wrapping_add(1);
        let taken = self.io(move |client| {
            // Do not release the source barrier on the PUT response: an
            // authored layout patch's exact text belongs to the server.
            let result = client.put_graph(&put_name, &put_graph)
                .and_then(|written| {
                    let definition = client.flow(&put_name)?;
                    if definition.revision != written.revision {
                        return Err(ClientError::Protocol("the flow changed during the save; queued edits were stopped so they cannot overwrite that newer revision".into()));
                    }
                    Ok(definition)
                });
            IoResult::GraphPut { name: put_name, generation, result }
        });
        if !taken {
            let revision = self.definition_revision().unwrap_or(0);
            self.graph_retry = Some((name, revision, graph, layout_only));
            self.set_error(cx, "the request lane is full; the edit is kept and sent on the next tick");
            return;
        }
        self.package_write_started(&name);
        self.graph_retry = None;
        self.graph_write_generation = generation;
        self.graph_write_source = Some(self.ui.widget(cx, ids!(source)).text());
        self.graph_write_layout_only = layout_only;
        self.pending_graph = true;
    }

    fn show_source_layout_conflict(&mut self, cx: &mut Cx) {
        self.source_layout_conflict = true;
        self.pending_source_edit = None;
        self.ui.button(cx, ids!(discard_source)).set_visible(cx, true);
        self.set_error(cx, "source draft kept; autosave paused because the graph changed. Save explicitly replaces the fetched version, including node positions. Discard draft reloads the saved source");
    }

    fn current_graph(&self) -> Option<Graph> {
        self.definition
            .as_ref()
            .and_then(|definition| definition.graph.clone())
    }

    fn apply_edit(&mut self, cx: &mut Cx, edit: CanvasEdit) {
        let layout_only = matches!(
            edit,
            CanvasEdit::Move { .. } | CanvasEdit::Resize { .. } | CanvasEdit::Flip { .. }
        );
        if !self.path.is_empty() {
            self.apply_component_layout(cx, self.path.clone(), edit);
            return;
        }
        if self.editing_locked() && !layout_only {
            return;
        }
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
                    .map(|candidate| !self.display_flips.get(&self.path)
                        .and_then(|flips| flips.get(&node))
                        .copied().unwrap_or(candidate.flip))
                else {
                    return;
                };
                self.with_canvas(cx, |_, canvas| canvas.lock_flip(&node));
                self.display_flips.entry(self.path.clone()).or_default().insert(node.clone(), flip);
                // A manual flip can restore the authored facing after an
                // automatic display flip, needing only a canvas refresh.
                self.show_graph(cx);
                self.refresh_inspector(cx);
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
            CanvasEdit::Reconnect { from_node, from_port, old_to_node, old_to_port, old_key, to_node, to_port } => {
                match graph_edit::reconnect(&graph, &from_node, &from_port, &old_to_node, &old_to_port, old_key, &to_node, &to_port) {
                    Ok(next) => next,
                    Err(error) => {
                        self.set_error(cx, &error);
                        return;
                    }
                }
            }
            CanvasEdit::Disconnect { to_node, to_port, key } => {
                graph_edit::disconnect(&graph, &to_node, &to_port, key)
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
        self.put_graph_with(cx, next, layout_only);
    }

    /// Layout belongs to the reusable module, so every occurrence of that
    /// module follows the edit. The server patches and repins its source.
    fn apply_component_layout(&mut self, cx: &mut Cx, path: Vec<String>, edit: CanvasEdit) {
        if self.unsaved || self.source_save_pending || self.pending_source_edit.is_some() || self.source_layout_conflict {
            self.restore_component_layout(cx, &path);
            self.set_error(cx, "save or discard the source draft before editing component layout");
            return;
        }
        let Some(alias) = self.alias_at(&path) else { return };
        let Some(mut root) = self.current_graph() else { return };
        let Some(component) = root.components.iter_mut().find(|component| component.alias == alias) else { return };
        component.graph = match edit {
            CanvasEdit::Move { node, at } => graph_edit::move_node(&component.graph, &node, at),
            CanvasEdit::Resize { node, size } => graph_edit::resize_node(&component.graph, &node, size),
            CanvasEdit::Flip { node } => {
                let Some(current) = component.graph.nodes.iter().find(|item| item.id == node) else { return };
                let flip = !self.display_flips.get(&path).and_then(|flips| flips.get(&node)).copied().unwrap_or(current.flip);
                self.display_flips.entry(path.clone()).or_default().insert(node.clone(), flip);
                graph_edit::flip_node(&component.graph, &node, flip)
            }
            _ => {
                self.restore_component_layout(cx, &path);
                self.set_error(cx, "component layout is editable; change its code or wiring in the module source");
                return;
            }
        };
        self.put_graph_with(cx, root, true);
    }

    fn restore_component_layout(&mut self, cx: &mut Cx, path: &[String]) {
        if path == self.path {
            self.show_graph(cx);
        } else if let Some(hierarchy) = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref()) {
            hierarchy.restore_readonly(cx, path);
        }
    }

    fn duplicate_selected(&mut self, cx: &mut Cx) {
        if self.editing_locked() {
            return;
        }
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
                IoResult::LiveSnapshot { generation, result } => {
                    self.io.fetching_live = false;
                    if generation != self.catalog_generation { continue; }
                    match result {
                        Ok(snapshot) => {
                            self.live_snapshot_error = None;
                            self.live_snapshot = Some(snapshot);
                        }
                        Err(error) => {
                            let message = format!("live synchronization: {error}");
                            if self.live_snapshot_error.as_ref() != Some(&message) {
                                log!("{message}");
                                self.set_error(cx, &message);
                            }
                            self.live_snapshot_error = Some(message);
                        }
                    }
                }
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
                IoResult::PackagePut { generation, name, result } => {
                    if generation != self.package_generation {
                        // Superseded: whatever it installed is retired.
                        self.discard_candidate(name, None);
                        continue;
                    }
                    match result {
                        Ok(response) => self.package_put_done(name, response),
                        Err(error) => {
                            self.candidate = None;
                            self.discard_candidate(name, None);
                            self.package_open_failed(cx, &error.to_string());
                        }
                    }
                }
                IoResult::PackageInstance { generation, name, result } => {
                    if generation != self.package_generation {
                        self.discard_candidate(name, result.ok().map(|created| created.instance));
                        continue;
                    }
                    match result {
                        Ok(created) => self.package_instance_done(cx, name, created.instance),
                        Err(error) => {
                            self.candidate = None;
                            self.discard_candidate(name, None);
                            self.package_open_failed(cx, &error.to_string());
                        }
                    }
                }
                IoResult::PackageRetired(result) => {
                    if let Err(error) = result {
                        if !is_absent_instance_error(&error) {
                            self.show_error(cx, &error);
                        }
                    }
                    self.refresh_flows();
                    self.refresh_instances();
                }
                IoResult::PackageCleaned(result) => {
                    if let Err(error) = result {
                        log!("flow: a superseded package candidate was not fully retired: {error}");
                    }
                    self.refresh_flows();
                    self.refresh_instances();
                }
                IoResult::PackageSaved { session, result } => {
                    // The save belongs to the session that started it. A
                    // retired session's save has already landed in order
                    // on the persist lane; nothing here to keep for it.
                    let Some(open) = self.package.as_mut().filter(|open| open.session == session) else {
                        continue;
                    };
                    open.saving = false;
                    let dirty = std::mem::take(&mut open.save_dirty);
                    match result {
                        Ok(package) => {
                            open.package = package;
                            open.save_failed = false;
                            if dirty { self.write_back_package(cx); }
                        }
                        Err(error) => {
                            open.save_failed = true;
                            let action = if open.reload.take().is_some() {
                                "package not saved; reload cancelled"
                            } else {
                                "package not saved"
                            };
                            self.set_error(cx, &format!("{action}: {error}"));
                        }
                    }
                }
                IoResult::PackageReloadDefinition { session, generation, epoch, result } => {
                    if generation != self.package_generation { continue }
                    let Some(open) = self.package.as_mut().filter(|open| open.session == session
                        && matches!(open.reload, Some(PackageReload::Loading))) else { continue };
                    if epoch != open.write_epoch {
                        open.reload = Some(PackageReload::Waiting);
                        continue;
                    }
                    match result {
                        Ok(definition) if definition.error.is_none() && definition.graph.is_some() => {
                            open.reload = Some(PackageReload::Ready { epoch, source: definition.source, modules: definition.modules });
                        }
                        Ok(_) => {
                            open.reload = None;
                            self.set_error(cx, "package reload cancelled: the latest source is not valid");
                        }
                        Err(error) => {
                            open.reload = None;
                            self.show_error(cx, &error);
                        }
                    }
                }
                IoResult::Flow {
                    name,
                    generation,
                    result,
                } => {
                    if self.selected.as_deref() != Some(&name)
                        || generation != self.attachment_generation
                        || self.pending_graph || self.graph_retry.is_some()
                    {
                        continue;
                    }
                    match result {
                        Ok(definition) => {
                            if self.definition_revision().is_some_and(|revision| definition.revision < revision) {
                                continue;
                            }
                            self.show_flow(cx, definition);
                            self.refresh_models(true);
                        }
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::Saved {
                    name,
                    generation,
                    source,
                    result,
                } => {
                    self.package_write_finished(&name, result.is_ok());
                    if generation != self.attachment_generation || self.selected.as_deref() != Some(&name) {
                        continue;
                    }
                    self.source_save_pending = false;
                    // An older save must never switch the selected flow or
                    // replace newer editor text/diagnostics as it finishes.
                    let current = !self.editing_locked()
                        && self.selected.as_deref() == Some(&name)
                        && self.ui.widget(cx, ids!(source)).text() == source;
                    match result {
                    Ok(definition) => {
                        if current {
                            self.unsaved = false;
                            self.set_error(cx, "");
                        }
                        self.show_flow(cx, definition);
                        self.refresh_flows();
                        self.services.refresh_definitions();
                    }
                    Err(error) if current || self.package.as_ref().is_some_and(|open| open.name == name) => self.show_error(cx, &error),
                    Err(_) => {}
                    }
                },
                IoResult::Created { name, automatic, package_generation, result } => {
                    // The automatic default is stale once anything was chosen
                    // meanwhile: a package open or a flow selected since is
                    // the newer intent and keeps the selection (a created flow
                    // is still listed), and its failure is not shown over what
                    // is open. A flow the user asked for opens, or reports.
                    let stale = automatic
                        && (package_generation != self.package_generation
                            || self.selected.is_some() || self.package_intent() || self.package.is_some());
                    match result {
                        Ok(_) => {
                            self.refresh_flows();
                            self.services.refresh_definitions();
                            if !stale {
                                self.open_flow(cx, name);
                            }
                        }
                        Err(error) => {
                            if !stale {
                                self.show_error(cx, &error);
                            }
                        }
                    }
                }
                IoResult::GraphPut { name, generation, result } => {
                    self.package_write_finished(&name, result.is_ok());
                    if generation != self.graph_write_generation || self.selected.as_deref() != Some(&name) {
                        continue;
                    }
                    self.pending_graph = false;
                    let draft_changed = self.graph_write_source.take()
                        .is_some_and(|source| self.ui.widget(cx, ids!(source)).text() != source);
                    if draft_changed { self.unsaved = true; }
                    let preserve_draft = draft_changed || self.unsaved
                        || self.pending_source_edit.is_some() || self.source_layout_conflict;
                    let mut graph_saved = false;
                    match result {
                        Ok(definition) => {
                            graph_saved = true;
                            let queued = self.graph_retry.as_mut().map(|(_, revision, graph, _)| {
                                *revision = definition.revision;
                                graph.revision = definition.revision;
                                if let Some(accepted) = definition.graph.as_ref() {
                                    // A previous component move repinned its source.
                                    // Keep the newest gesture, with those acknowledged
                                    // pins and locations for the next serialized write.
                                    for component in &mut graph.components {
                                        if let Some(current) = accepted.components.iter().find(|item| item.alias == component.alias) {
                                            component.hash.clone_from(&current.hash);
                                            component.graph.revision = current.graph.revision;
                                            for node in &mut component.graph.nodes {
                                                if let Some(current) = current.graph.nodes.iter().find(|item| item.id == node.id) {
                                                    node.loc.clone_from(&current.loc);
                                                }
                                            }
                                        }
                                    }
                                }
                                graph.clone()
                            });
                            let queued_source = queued.as_ref().and_then(|_| self.definition.as_ref())
                                .filter(|definition| definition.canonical).map(|definition| definition.source.clone());
                            // Persist the server's exact text before restoring the
                            // latest optimistic projection for any held gesture.
                            self.show_flow_with_layout(cx, definition, self.graph_write_layout_only);
                            if let Some(graph) = queued {
                                let remount = self.current_graph().as_ref()
                                    .is_none_or(|old| graph_edit::needs_face_remount(old, &graph));
                                if let Some(definition) = self.definition.as_mut() {
                                    definition.graph = Some(graph);
                                    if let Some(source) = queued_source { definition.source = source; }
                                }
                                self.show_graph(cx);
                                if remount { self.remount_faces(cx); }
                                else { self.update_component_layouts(cx); }
                            }
                            if preserve_draft { self.show_source_layout_conflict(cx); }
                            self.refresh_flows();
                            self.services.refresh_definitions();
                            if self.graph_retry.is_none() && std::mem::take(&mut self.deferred_batch) {
                                self.queue_run(cx);
                            }
                        }
                        Err(error) => {
                            self.graph_retry = None;
                            self.deferred_batch = false;
                            self.show_error(cx, &error);
                            if preserve_draft { self.show_source_layout_conflict(cx); }
                            if let Some(name) = self.selected.clone() {
                                self.load_flow(name);
                            }
                        }
                    }
                    if graph_saved && self.graph_retry.is_none() {
                        if let Some(request) = self.deferred_run.take() {
                            self.start_run(request.outputs);
                        }
                    }
                }
                IoResult::Catalog { generation, result } => {
                    if generation != self.catalog_generation {
                        continue;
                    }
                    match result {
                    Ok((catalog, registry)) => {
                        self.registry = Some(registry);
                        self.catalog = catalog.types;
                        if let Some(mut palette) =
                            self.ui.widget(cx, ids!(palette)).borrow_mut::<Palette>()
                        {
                            palette.set_types(cx, &self.catalog, false);
                        }
                        if self.definition.is_some() {
                            self.remount_faces(cx);
                        }
                        let waiting = self
                            .candidate
                            .as_ref()
                            .and_then(|candidate| candidate.instance.clone().map(|instance| (candidate.name.clone(), instance)));
                        if let Some((name, instance)) = waiting {
                            self.package_instance_done(cx, name, instance);
                        }
                    }
                    Err(error) => self.show_error(cx, &error),
                    }
                },
                IoResult::Templates(result) => match result {
                    Ok(mut templates) => {
                        // The shared list keeps the server's labels; the New
                        // picker shows them prefixed by group, the Templates
                        // menu under group headings.
                        templates.sort_by(|left, right| {
                            makepad_flow::templates::group_rank(&left.group)
                                .cmp(&makepad_flow::templates::group_rank(&right.group))
                                .then_with(|| left.label.to_lowercase().cmp(&right.label.to_lowercase()))
                        });
                        self.templates = templates;
                        if let Some(mut picker) = self
                            .ui
                            .widget(cx, ids!(template_picker))
                            .borrow_mut::<TemplatePicker>()
                        {
                            picker.set_templates(cx, grouped_new_menu_templates(self.templates.clone()));
                        }
                        self.refresh_templates_menu(cx);
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
                IoResult::Parallelism { flow, result } => {
                    self.io.fetching_parallelism = false;
                    if self.selected.as_deref() != Some(flow.as_str()) {
                        self.refresh_parallelism();
                        continue;
                    }
                    match result {
                        Ok(response) => {
                            self.parallel_max = response.max.max(1);
                            let combo = self.ui.combo_box(cx, ids!(parallel));
                            combo.set_labels(
                                cx,
                                vec!["1".to_string(), format!("max · {}", self.parallel_max)],
                            );
                            combo.set_selected_item(cx, usize::from(self.parallel_use_max));
                        }
                        Err(error) => log!("flow-ui: parallelism for {flow}: {error}"),
                    }
                }
                IoResult::BatchCreated {
                    flow,
                    planned_nodes,
                    revision,
                    result,
                } => match result {
                    Ok(response) => {
                        self.queue_generation = self.queue_generation.wrapping_add(1);
                        self.queue_refresh_after_stale = true;
                        let first = response.runs.first().cloned();
                        if let Some(mut queue) =
                            self.ui.widget(cx, ids!(running)).borrow_mut::<QueueList>()
                        {
                            queue.add_batch(
                                cx,
                                &flow,
                                response,
                                &planned_nodes,
                                revision,
                                wall_clock_ms(),
                            );
                        }
                        if self.selected.as_deref() == Some(flow.as_str()) {
                            if let Some(first) = first {
                                self.focus_instance(cx, first.instance, Some(flow));
                                self.display_run(
                                    cx,
                                    RunInfo {
                                        run_id: first.run_id.clone(),
                                        state: "queued".into(),
                                        started: self.time,
                                        finished_secs: None,
                                        revision: self
                                            .definition
                                            .as_ref()
                                            .map_or(0, |definition| definition.revision),
                                        planned_nodes,
                                    },
                                );
                                if let Some(mut queue) =
                                    self.ui.widget(cx, ids!(running)).borrow_mut::<QueueList>()
                                {
                                    // Named in full: the `Select` widget's `select(&self, ..)`
                                    // extension would otherwise shadow this `&mut self` method.
                                    QueueList::select(&mut queue, cx, &first.run_id);
                                }
                                self.fetch_run_snapshot();
                            }
                        }
                        self.refresh_instances();
                        self.refresh_queue();
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::QueueRows { generation, result } => {
                    self.io.fetching_queue = false;
                    if generation != self.queue_generation {
                        if std::mem::take(&mut self.queue_refresh_after_stale) {
                            self.refresh_queue();
                        }
                        continue;
                    }
                    self.queue_refresh_after_stale = false;
                    match result {
                        Ok(rows) => {
                            if let Some(mut queue) =
                                self.ui.widget(cx, ids!(running)).borrow_mut::<QueueList>()
                            {
                                queue.set_rows(cx, rows);
                            }
                        }
                        Err(error) => log!("flow-ui: queue snapshot: {error}"),
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
                            self.refresh_queue();
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
                        if let Some(mut queue) =
                            self.ui.widget(cx, ids!(running)).borrow_mut::<QueueList>()
                        {
                            queue.set_labels(
                                cx,
                                rows.iter()
                                    .filter_map(|row| {
                                        row.label.clone().map(|label| (row.instance.clone(), label))
                                    })
                                    .collect(),
                            );
                        }
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
                            let completed = match row.output_slots() {
                                Ok(slots) => slots,
                                Err(error) => {
                                    self.set_error(cx, &format!("Malformed instance outputs: {error}"));
                                    continue;
                                }
                            };
                            if self.selected.as_deref() != Some(row.flow.as_str()) {
                                let instance = row.instance.clone();
                                self.focus_instance(cx, instance, Some(row.flow.clone()));
                            }
                            let label = row
                                .label
                                .clone()
                                .unwrap_or_else(|| row.instance.chars().take(8).collect());
                            self.instance_label = Some(label);
                            self.refresh_instance_chip(cx);
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
                            if self.live_package() {
                                self.with_surface(cx, |cx, view| view.fill_inputs(cx, &row));
                            }
                            for (node, slot) in completed {
                                let port = self.output_face_port(&node);
                                self.record_slot(cx, &node, &port, slot, row.revision);
                            }
                            self.request_wanted_values(cx);
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
                IoResult::Assets {
                    generation,
                    query,
                    namespace,
                    next_page,
                    merge_existing,
                    preserve_cursor,
                    result,
                } => {
                    if generation != self.assets_request_generation {
                        continue;
                    }
                    let inspector_widget = self.ui.widget(cx, ids!(inspector));
                    let Some(inspector_ref) = inspector_widget.borrow::<Inspector>() else {
                        continue;
                    };
                    let (current_query, current_namespace) = inspector_ref.asset_query();
                    if !assets_response_is_current(
                        generation,
                        self.assets_request_generation,
                        &query,
                        namespace.as_deref(),
                        &current_query,
                        current_namespace.as_deref(),
                    ) {
                        continue;
                    }
                    drop(inspector_ref);
                    match result {
                        Ok(response) => {
                            let _missing = self
                                .ui
                                .widget(cx, ids!(inspector))
                                .borrow_mut::<Inspector>()
                                .map(|mut inspector| {
                                    inspector.set_assets_page(
                                        cx,
                                        response,
                                        merge_existing,
                                        preserve_cursor,
                                    )
                                })
                                .unwrap_or_default();
                            // Preview requests are issued from the current
                            // viewport on the next poll. Do not fetch every
                            // thumbnail in a paged response.
                        }
                        Err(error) => {
                            let message = error.to_string();
                            if next_page {
                                if let Some(mut inspector) = self
                                    .ui
                                    .widget(cx, ids!(inspector))
                                    .borrow_mut::<Inspector>()
                                {
                                    inspector.reset_asset_paging(cx);
                                }
                                self.refresh_assets(cx, false, true, false);
                            } else if let Some(mut inspector) = self
                                .ui
                                .widget(cx, ids!(inspector))
                                .borrow_mut::<Inspector>()
                            {
                                inspector.set_assets_error(cx, &message);
                            }
                        }
                    }
                },
                IoResult::AssetContent { id, result } => {
                    if self.preview_digest.as_ref().is_some_and(|(current, _)| current == &id) {
                        match result {
                            Ok(bytes) => {
                                let title = self.preview_digest.as_ref().map(|(_, title)| title.clone()).unwrap_or_default();
                                self.show_image_viewer(cx, &title, "", &bytes);
                            }
                            Err(error) => {
                                if let Some(mut viewer) = self.ui.widget(cx, ids!(image_viewer)).borrow_mut::<ImageViewer>() {
                                    viewer.show_status(cx, "Could not open asset", &error.to_string());
                                }
                                self.show_error(cx, &error);
                            },
                        }
                    }
                }
                IoResult::AssetThumbnail { alias, result } => match result {
                    Ok(bytes) => {
                        self.asset_preview_inflight.remove(&alias);
                        self.asset_preview_retry_at.remove(&alias);
                        if let Some(mut inspector) = self
                            .ui
                            .widget(cx, ids!(inspector))
                            .borrow_mut::<Inspector>()
                        {
                            inspector.set_asset_thumbnail(cx, alias, bytes);
                        }
                    }
                    Err(error) => {
                        self.asset_preview_inflight.remove(&alias);
                        self.asset_preview_retry_at
                            .insert(alias, self.time + ASSET_PREVIEW_RETRY_SECS);
                        log!("flow-ui: asset thumbnail: {error}");
                    }
                },
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

    fn show_flow(&mut self, cx: &mut Cx, definition: FlowDefinition) {
        self.show_flow_with_layout(cx, definition, false);
    }

    fn show_flow_with_layout(&mut self, cx: &mut Cx, definition: FlowDefinition, layout_only: bool) {
        if !self.unsaved {
            self.ui.widget(cx, ids!(source)).set_text(cx, &definition.source);
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
        // Only an acknowledged layout transaction may reuse mounted module
        // isolates across changed pins. An external source edit remounts.
        let same_modules = self.definition.as_ref().is_some_and(|old| old.modules == definition.modules);
        let retained = (layout_only || same_modules) && definition.graph.as_ref().is_some_and(|graph| {
            self.faces.as_mut().and_then(|faces| faces.hierarchy.as_mut())
                .is_some_and(|hierarchy| hierarchy.update_layouts(cx, graph, &Closure { modules: definition.modules.clone() }))
        });
        let remount = self
            .definition
            .as_ref()
            .and_then(|old| old.graph.as_ref())
            .zip(definition.graph.as_ref())
            .is_none_or(|(old, new)| graph_edit::needs_face_remount(old, new))
            || (!retained && self.definition.as_ref().is_some_and(|old| old.modules != definition.modules));
        let revision_changed = self
            .definition
            .as_ref()
            .is_some_and(|old| old.revision != definition.revision);
        let execution_changed = revision_changed && self.accepted_graph.as_ref()
            .zip(definition.graph.as_ref())
            .is_none_or(|(old, new)| !makepad_flow::live::same_root_layout_execution(old, new));
        self.accepted_graph = definition.graph.clone();
        self.definition = Some(definition);
        if !self.path.is_empty() && self.level_alias().is_none() {
            // The container the pane showed is gone from this revision:
            // the cameras and selections belonged to that definition.
            self.path.clear();
            self.cameras.clear();
            self.display_flips.clear();
            self.selections.clear();
        }
        self.refresh_breadcrumb(cx);
        self.show_graph(cx);
        if remount || self.faces.is_none() {
            self.remount_faces(cx);
        }
        if revision_changed {
            self.validate_pending_inputs();
        }
        self.refresh_surface(cx, execution_changed);
        self.write_back_package(cx);
        self.update_connection(cx);
        if self.source_layout_conflict { self.show_source_layout_conflict(cx); }
    }

    fn update_component_layouts(&mut self, cx: &mut Cx) {
        let Some(root) = self.current_graph() else { return };
        let closure = self.closure();
        if let Some(hierarchy) = self.faces.as_mut().and_then(|faces| faces.hierarchy.as_mut()) {
            hierarchy.update_layouts(cx, &root, &closure);
        }
    }

    /// A control change queued for a port the accepted graph no longer has
    /// goes nowhere.
    fn validate_pending_inputs(&mut self) {
        let Some(graph) = self.current_graph() else {
            return;
        };
        let interface = FlowInterface::of(&graph);
        self.pending_inputs.retain(|(node, port), _| {
            interface.inputs.iter().any(|input| &input.node == node && &input.port == port)
        });
    }

    /// Push the graph of the level shown into the canvas and inspector.
    fn show_graph(&mut self, cx: &mut Cx) {
        let graph = self.view_graph();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_compatible_ports(
                graph
                    .as_ref()
                    .map(graph_view::compatibility_of)
                    .unwrap_or_default(),
            );
            let view = graph.as_ref().map(|graph| hierarchy::view_of(graph, &self.catalog));
            if !view.as_ref().is_some_and(|view| canvas.update_graph_layout(cx, view)) {
                canvas.set_graph(cx, view);
            }
        }
        if let Some(faces) = self.faces.as_mut() {
            if let Some(graph) = graph.as_ref() {
                faces.refresh_params(cx, graph);
            }
        }
        self.refresh_inspector(cx);
    }

    fn inspection(&self, cx: &Cx) -> (Vec<String>, Option<Selection>, Option<Graph>) {
        if let Some((path, selection)) = &self.nested_selection {
            if let Some(graph) = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref())
                .and_then(|hierarchy| hierarchy.graph(path)) {
                return (path.clone(), Some(selection.clone()), Some(graph));
            }
        }
        let selection = self.ui.widget(cx, ids!(canvas)).borrow::<FlowCanvas>()
            .and_then(|canvas| canvas.selection().cloned());
        (self.path.clone(), selection, self.view_graph())
    }

    fn inspection_canvas(&self, cx: &Cx) -> WidgetRef {
        self.nested_selection.as_ref().and_then(|(path, _)| self.faces.as_ref()
            .and_then(|faces| faces.hierarchy.as_ref()).and_then(|hierarchy| hierarchy.canvas(path)))
            .unwrap_or_else(|| self.ui.widget(cx, ids!(canvas)))
    }

    fn select_inspection(&mut self, cx: &mut Cx, selection: Selection) {
        let widget = self.inspection_canvas(cx);
        if let Some(mut canvas) = widget.borrow_mut::<FlowCanvas>() {
            canvas.set_selection(cx, Some(selection.clone()));
        }
        let path = self.nested_selection.as_ref().map(|(path, _)| path.clone());
        self.selected_node = selection.node().map(|node| format!("{}{node}",
            hierarchy::prefix(path.as_deref().unwrap_or(&self.path))));
        if let Some(path) = path { self.nested_selection = Some((path, selection)); }
        self.refresh_inspector(cx);
        self.update_menu_state(cx);
    }

    fn enter_inspected_container(&mut self, cx: &mut Cx, node: String) {
        if let Some((path, _)) = self.nested_selection.clone() {
            let is_component = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref())
                .and_then(|hierarchy| hierarchy.graph(&path))
                .is_some_and(|graph| graph.nodes.iter().any(|item| item.id == node
                    && item.kind == "component"));
            if !is_component { return; }
            self.remember_level(cx);
            self.path = path;
            self.path.push(node);
            self.show_level(cx);
            return;
        }
        self.enter_container(cx, node);
    }

    fn refresh_inspector(&mut self, cx: &mut Cx) {
        let (inspection_path, selection, graph) = self.inspection(cx);
        let selected = selection
            .as_ref()
            .and_then(Selection::node)
            .map(str::to_string);
        let qualified = selected.as_ref().map(|node| format!("{}{node}", hierarchy::prefix(&inspection_path)));
        let outputs = qualified
            .as_ref()
            .and_then(|node| self.outputs.get(node).cloned())
            .unwrap_or_default();
        let domain = selected
            .as_ref()
            .and_then(|id| graph.as_ref()?.nodes.iter().find(|node| &node.id == id)?.domain.clone());
        let models = domain
            .as_ref()
            .and_then(|domain| self.models.get(domain).cloned())
            .unwrap_or_default();
        let models = selected
            .as_ref()
            .and_then(|id| graph.as_ref()?.nodes.iter().find(|node| &node.id == id))
            .map(|node| model_choices_for_node(&models, &node.kind))
            .unwrap_or(models);
        // Bytes per element: `None` for a one port, the element's index for
        // an ordered one.
        let loaded: Vec<(String, Option<usize>, makepad_flow::ValueBytes)> = outputs
            .iter()
            .flat_map(|(port, slot)| {
                let ordered = slot.is_ordered();
                slot.values()
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| {
                        self.values
                            .get(&value.digest)
                            .map(|bytes| (port.clone(), ordered.then_some(index), bytes))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let source = self.alias_at(&inspection_path)
            .and_then(|alias| self.closure().module(&alias).map(|module| module.source.clone()))
            .or_else(|| self.definition.as_ref().map(|definition| definition.source.clone()));
        // A selected wire probes the value that crossed it: its source's
        // output under the qualified id the run published. Bytes not held
        // yet are fetched on the data lane; the rows refresh on arrival.
        let probe = match selection.as_ref() {
            Some(Selection::Edge { from_node, from_port, .. }) => {
                let (key, port) = self.wire_source_key_at(&inspection_path, graph.as_ref(), from_node, from_port);
                self.outputs
                    .get(&key)
                    .and_then(|ports| ports.iter().find(|(name, _)| *name == port))
                    .map(|(_, slot)| slot.clone())
                    .map(|slot| {
                        let bytes: Vec<Option<makepad_flow::ValueBytes>> = slot
                            .values()
                            .iter()
                            .map(|value| {
                                let held = self.values.get(&value.digest);
                                if held.is_none() {
                                    if let Some(client) = self.client() {
                                        self.values.request(&value.digest, client, &self.data_lane);
                                    }
                                }
                                held
                            })
                            .collect();
                        panels::EdgeProbe { node: key, port, slot, bytes }
                    })
            }
            _ => None,
        };
        if let Some(mut inspector) = self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>() {
            inspector.set_locked(cx, self.editing_locked() || !inspection_path.is_empty());
            inspector.set_models(cx, models);
            match selection.as_ref() {
                Some(Selection::Edge {
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                    key,
                }) => inspector.show_edge(
                    cx,
                    graph.as_ref(),
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                    *key,
                    probe,
                ),
                _ => inspector.show_node(
                    cx,
                    graph.as_ref(),
                    &self.catalog,
                    selected.as_deref(),
                    qualified.as_deref(),
                    &outputs,
                    &loaded,
                    source.as_deref(),
                ),
            }
        }
        if let Some(client) = self.client() {
            for value in outputs.iter().flat_map(|(_, slot)| slot.values().iter()) {
                if !self.values.contains(&value.digest) {
                    self.values.request(&value.digest, client.clone(), &self.data_lane);
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
                    let models = model_choices_for_node(models, &node.kind);
                    faces.set_models(cx, &node.id, &models);
                }
            }
        }
        self.refresh_inspector(cx);
    }

    /// Free the instance's isolate and evaluate the file again into a new one.
    fn remount_faces(&mut self, cx: &mut Cx) {
        self.nested_selection = None;
        self.nested_capture = None;
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_roots(cx, Vec::new());
            canvas.set_embedded_roots(cx, Vec::new());
        }
        if let Some(faces) = self.faces.take() {
            faces.free(cx);
        }
        let (Some(definition), Some(name)) = (self.definition.as_ref(), self.selected.as_ref())
        else {
            return;
        };
        let Some(graph) = self.view_graph() else {
            return;
        };
        let graph = &graph;
        let Some(registry) = self.registry.as_ref() else {
            return;
        };
        if self.catalog.is_empty() {
            return;
        }
        let closure = Closure { modules: definition.modules.clone() };
        let level = self.level_alias();
        let (file_name, source) = match level.as_deref().and_then(|alias| closure.module(alias)) {
            Some(module) => (module.path.clone(), module.source.clone()),
            None => (format!("{name}.splash"), definition.source.clone()),
        };
        let parent = self
            .ui
            .widget(cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .map(|canvas| canvas.widget_uid())
            .unwrap_or(WidgetUid(0));
        let instance = self.instance.clone().unwrap_or_else(|| "unbound".to_string());
        let mut graph = graph.clone();
        hierarchy::auto_place(&mut graph, &self.catalog);
        let mut faces = FaceHost::mount_scoped_with_registry(
            cx,
            parent,
            &instance,
            &file_name,
            &source,
            &graph,
            &self.catalog,
            &closure,
            level.as_deref(),
            registry,
            &self.path,
        );
        let canvas_widget = self.ui.widget(cx, ids!(canvas));
        if let Some(root) = self.root_graph() {
            faces.hierarchy = Some(hierarchy::Hierarchy::new(cx, &canvas_widget, &graph, root,
                &self.path, &self.catalog, registry.clone(), closure.clone(), instance.clone()));
        }
        if let Some(mut canvas) = canvas_widget.borrow_mut::<FlowCanvas>() {
            let _ = canvas.set_zoom_range(cx, 0.05, 4096.0);
        }
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
        faces.set_locked(cx, self.editing_locked());
        // Outputs the run already produced land in the fresh faces too.
        let outputs = self.outputs.clone();
        for (node, ports) in &outputs {
            for (port, slot) in ports {
                match slot {
                    ValueSlotRef::One(value) => {
                        let bytes = self.values.get(&value.digest);
                        faces.push_value(cx, node, port, value, bytes.as_ref());
                    }
                    ValueSlotRef::Seq(sequence) => faces.push_sequence(cx, node, port, sequence),
                }
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
        self.bind_graph_frames(cx);
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
        if let Some(faces) = self.faces.as_mut() {
            let wanted = faces.wanted_values();
            for digest in wanted {
                if self.values.contains(&digest) {
                    faces.deliver_bytes(cx, &mut self.values, &digest);
                    continue;
                }
                self.values.request(&digest, client.clone(), &self.data_lane);
            }
        }
        let wanted = self.with_surface(cx, |_, view| view.wanted()).unwrap_or_default();
        for digest in wanted {
            if self.values.contains(&digest) {
                let mut values = std::mem::take(&mut self.values);
                self.with_surface(cx, |cx, view| view.deliver_bytes(cx, &mut values, &digest));
                self.values = values;
                continue;
            }
            self.values.request(&digest, client.clone(), &self.data_lane);
        }
    }

    fn set_error(&mut self, cx: &mut Cx, text: &str) {
        if !text.is_empty() && self.last_error.as_deref() != Some(text) {
            log!("flow: {text}");
        }
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
    /// The shown run and everything it painted go: node chips and bars on
    /// the cards, streamed text, the toolbar bar.
    fn clear_run_view(&mut self, cx: &mut Cx) {
        self.run = None;
        self.current_node = None;
        self.outputs.clear();
        self.statuses.clear();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
        }
        if let Some(faces) = self.faces.as_mut() {
            faces.reset_run();
        }
        self.refresh_inspector(cx);
        self.update_run_bar(cx);
    }

    fn update_run_bar(&mut self, cx: &mut Cx) {
        let (fraction, done, total) = match self.run.as_ref() {
            Some(run) => {
                // Planned nodes are qualified; their statuses live here,
                // whatever level the canvas shows.
                let statuses: HashMap<String, NodeStatus> = run
                    .planned_nodes
                    .iter()
                    .filter_map(|node| {
                        self.statuses.get(node).map(|(state, permille, has_progress, stage, error)| {
                            (node.clone(), NodeStatus::new(state, *permille, *has_progress, stage, error.clone()))
                        })
                    })
                    .collect();
                total_progress(&run.planned_nodes, &statuses)
            }
            None => (0.0, 0, 0),
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
        let has_selected_node = self.selected_node.is_some() && self.nested_selection.is_none();
        let has_selection = self
            .ui
            .widget(cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .is_some_and(|canvas| canvas.selection().is_some()) && self.nested_selection.is_none();
        let can_undo = self.revisions.len() >= 2;
        let can_redo = !self.redo.is_empty();
        let has_instance = self.instance.is_some();
        let can_flip = self.selected_node.is_some();
        let next = (
            running,
            has_selected_node,
            has_selection,
            can_undo,
            can_redo,
            has_instance,
            can_flip,
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
        menu.set_enabled(cx, live_id!(flip_card), can_flip);
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
                    self.refresh_queue();
                    self.fetch_instance();
                }
                SubscriptionEvent::ResyncRequired => {
                    self.refresh_flows();
                    self.refresh_instances();
                    self.refresh_queue();
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
        if event.run_id.is_some() {
            // A run the queue has not seen (a Play run, or one another client
            // started) is fetched with its row rather than guessed at.
            let known = self
                .ui
                .widget(cx, ids!(running))
                .borrow_mut::<QueueList>()
                .map(|mut queue| queue.apply_event(cx, &event, wall_clock_ms()))
                .unwrap_or(true);
            if !known {
                self.refresh_queue();
            }
        }
        match event.kind.as_str() {
            "flow.changed" => {
                self.refresh_flows();
                self.services.refresh_definitions();
                if !self.unsaved && !self.pending_graph && self.graph_retry.is_none()
                    && event.name.as_deref() == self.selected.as_deref() {
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
        let slots = if matches!(event.kind.as_str(), "node.done" | "run.finished") {
            match event.output_slots() {
                Ok(slots) => slots,
                Err(error) => {
                    self.set_error(cx, &format!("Malformed {} outputs: {error}", event.kind));
                    self.fetch_run_snapshot();
                    return;
                }
            }
        } else {
            Vec::new()
        };
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
                // `permille` is the whole node; without it the node is in an
                // open-ended stage and the bar stays indeterminate.
                let overall = event.permille.map(|permille| permille.min(1000) as u16);
                let raw_stage = event.stage.as_deref().unwrap_or_default();
                let share = event.stage_permille.filter(|p| *p > 0).map_or(-1.0, |p| p.min(1000) as f64 / 1000.0);
                let stage = self
                    .node_clocks
                    .entry(node.clone())
                    .or_default()
                    .observe(raw_stage, share, std::time::Instant::now())
                    .line();
                self.set_node_status(cx, &node, "running", overall.unwrap_or(0), overall.is_some(), &stage, None);
            }
            "node.delta" => {
                let port = event.port.clone().unwrap_or_else(|| "text".into());
                let text = event.text.clone().unwrap_or_default();
                if let Some(local) = self.level_node(&node) {
                    if let Some(faces) = self.faces.as_mut() {
                        faces.push_delta(cx, &local, &port, &text);
                    }
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        canvas.pulse(cx, &local, false);
                        canvas.set_streaming(cx, &local, true);
                    }
                }
            }
            "node.waiting" => {
                self.current_node = Some(node.clone());
                self.set_node_status(cx, &node, "waiting", 0, false, "", None);
            }
            "node.answered" => self.set_node_status(cx, &node, "running", 0, false, "", None),
            "node.done" => {
                self.node_clocks.remove(&node);
                self.set_node_status(cx, &node, "done", 1000, true, "", None);
                if let Some(local) = self.level_node(&node) {
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        canvas.pulse(cx, &local, true);
                    }
                }
                let revision = self.run_revision();
                for (port, slot) in slots {
                    self.record_slot(cx, &node, &port, slot, revision);
                }
                self.request_wanted_values(cx);
                if self.current_graph().is_some_and(|graph| {
                    graph.nodes.iter().any(|candidate| {
                        candidate.id == node && matches!(candidate.kind.as_str(), "publish" | "output" | "gen")
                    })
                }) {
                    self.refresh_assets(cx, false, false, false);
                }
                if self.inspector_depends_on(cx, &node) {
                    self.refresh_inspector(cx);
                }
            }
            "node.failed" => {
                self.node_clocks.remove(&node);
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
                self.node_clocks.clear();
                let state = event.state_text().unwrap_or_else(|| "done".into());
                if state == "cancelled" {
                    // A cancelled run leaves nothing on the canvas: cards,
                    // bars and the toolbar clear as if Clear had been pressed.
                    self.clear_run_view(cx);
                    if let Some(faces) = self.faces.as_mut() {
                        faces.push_state(cx, "run", &state);
                    }
                    self.refresh_instances();
                    self.fetch_instance();
                    if self.deferred_flow_reload {
                        self.deferred_flow_reload = false;
                        if let Some(name) = self.selected.clone() {
                            self.load_flow(name);
                        }
                    }
                    return;
                }
                // Output nodes get no node.* events of their own: the run's
                // outputs are their values, so they finish with the run.
                let revision = self.run_revision();
                for (output_node, slot) in slots {
                    let port = self.output_face_port(&output_node);
                    self.record_slot(cx, &output_node, &port, slot, revision);
                    self.set_node_status(cx, &output_node, "done", 1000, true, "", None);
                    if let Some(local) = self.level_node(&output_node) {
                        if let Some(mut canvas) =
                            self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                        {
                            canvas.pulse(cx, &local, true);
                        }
                    }
                }
                self.request_wanted_values(cx);
                self.refresh_inspector(cx);
                if let Some(run) = self.run.as_mut() {
                    run.state = state.clone();
                    run.finished_secs = event.secs;
                }
                // Planned nodes are qualified and lie at every level; the
                // status store holds all of them, the canvas only the level
                // shown. A node the run left unresolved is cancelled.
                let unresolved = self
                    .run
                    .as_ref()
                    .map(|run| run.planned_nodes.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|node| {
                        self.statuses.get(node).is_none_or(|(state, ..)| {
                            matches!(state.as_str(), "pending" | "ready" | "running" | "waiting")
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
        let stage = if state == "running" { stage.to_string() } else { String::new() };
        // Every status is kept by qualified id; the canvas and faces show
        // the level's own nodes, a container shows what its inner nodes add
        // up to, and the surface addresses qualified ids as the run does.
        self.statuses
            .insert(node.to_string(), (state.to_string(), permille, has_progress, stage.clone(), error.clone()));
        if let Some(local) = self.level_node(node) {
            if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                canvas.set_status(cx, &local, NodeStatus::new(state, permille, has_progress, &stage, error));
            }
            if let Some(faces) = self.faces.as_mut() {
                faces.push_state(cx, &local, state);
            }
        } else if let Some(container) = self.level_container(node) {
            self.refresh_container_status(cx, &container);
        }
        if self.live_package() {
            self.with_surface(cx, |cx, view| view.push_state(cx, node, state));
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

    /// Keep a node's value; `revision` is the definition revision the run
    /// that produced it ran under. Node faces show it as before; the app
    /// surface only takes values of the accepted revision, so a late value
    /// of an earlier graph never reaches a binding of the current one.
    /// A port's published value in its shape. The one store; the faces and
    /// the surface are scalar widgets, so a sequence reaches them as a
    /// count readout while the inspector lists its elements.
    fn record_slot(&mut self, cx: &mut Cx, node: &str, port: &str, slot: ValueSlotRef, revision: u64) {
        self.output_revisions.insert(node.to_string(), revision);
        let ports = self.outputs.entry(node.to_string()).or_default();
        if let Some((_, old)) = ports.iter_mut().find(|(name, _)| name == port) {
            *old = slot.clone();
        } else {
            ports.push((port.to_string(), slot.clone()));
        }
        let local = self.level_node(node);
        if let Some(local) = local.as_deref() {
            if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                canvas.set_streaming(cx, local, false);
            }
        }
        match &slot {
            ValueSlotRef::One(value) => {
                let bytes = self.values.get(&value.digest);
                if let Some(local) = local.as_deref() {
                    if let Some(faces) = self.faces.as_mut() {
                        faces.push_value(cx, local, port, value, bytes.as_ref());
                    }
                    self.fill_boundary_face(cx, local, port, value);
                }
                if self.live_package() && self.definition_revision() == Some(revision) {
                    let wants = self
                        .with_surface(cx, |cx, view| view.push_value(cx, node, port, value, bytes.as_ref()))
                        .unwrap_or(false);
                    if wants {
                        self.request_wanted_values(cx);
                    }
                }
            }
            ValueSlotRef::Seq(sequence) => {
                if let Some(local) = local.as_deref() {
                    if let Some(faces) = self.faces.as_mut() {
                        faces.push_sequence(cx, local, port, sequence);
                    }
                }
                if self.live_package() && self.definition_revision() == Some(revision) {
                    self.with_surface(cx, |cx, view| view.push_sequence(cx, node, port, sequence));
                }
            }
        }
    }

    fn definition_revision(&self) -> Option<u64> {
        self.definition.as_ref().map(|definition| definition.revision)
    }

    fn run_revision(&self) -> u64 {
        self.run.as_ref().map_or(0, |run| run.revision)
    }

    // -- pending writes ----------------------------------------------------------

    /// Text inputs and parameter changes are coalesced on the 250 ms poll
    /// tick; explicit controls are written on the tick they change.
    fn flush_pending(&mut self, cx: &mut Cx) {
        if !self.source_save_pending && !self.editing_locked() && self.client().is_some()
            && !self.pending_graph && self.graph_retry.is_none() && !self.source_layout_conflict
            && self.pending_source_edit.as_ref().is_some_and(|(_, _, due)| {
                std::time::Instant::now() >= *due
            })
        {
            let (name, source, _) = self.pending_source_edit.take().unwrap();
            self.save_source(cx, name, source, false);
        }
        if !self.pending_inputs.is_empty()
            && !self.input_flush_in_flight
            && self.instance.is_some()
            && self.client().is_some()
        {
            self.validate_pending_inputs();
            let instance = self.instance.clone().unwrap();
            let generation = self.attachment_generation;
            // The journal is handed to the lane only when the lane can take
            // it; a refusal after that (the lane is gone) gives it back.
            if !self.control_lane.can_accept() {
                return;
            }
            let journal = std::mem::take(&mut self.pending_inputs);
            let kept = journal.clone();
            let body = input_journal_body(self.current_graph().as_ref(), &journal);
            let taken = self.io(move |client| {
                let result = client.put_inputs(&instance, "tab", &body).map(|_| ());
                IoResult::InputsPut {
                    instance,
                    generation,
                    journal,
                    result,
                }
            });
            self.input_flush_in_flight = taken;
            if !taken {
                merge_failed_input_journal(&mut self.pending_inputs, kept);
            }
        }
        if !self.pending_params.is_empty() && !self.pending_graph && self.graph_retry.is_none() && self.path.is_empty() {
            if let Some(mut graph) = self.current_graph() {
                let pending = std::mem::take(&mut self.pending_params);
                for ((node, key), value) in pending {
                    graph = graph_edit::set_param(&graph, &node, &key, value);
                }
                self.put_graph(cx, graph);
            } else {
                self.pending_params.clear();
            }
        }
    }

    /// The single design-vs-run decision for face commits. In design, bound
    /// values become graph literals (`Input.value` for input nodes); an
    /// attached run is a locked snapshot and refuses every face write.
    fn route_face_commits(
        &mut self,
        binds: Vec<(String, String, String)>,
        params: Vec<(String, String, Literal)>,
    ) {
        if self.live_package() {
            // A package's instance is live: bound values are its inputs, the
            // same ones the app surface writes, and the graph keeps its own
            // literals. Params are graph edits as in design.
            for (node, port, text) in binds {
                self.pending_inputs.insert((node, port), text);
            }
            self.pending_params.extend(params.into_iter().filter(|(node, _, _)| !node.contains('/')).map(|(node, key, value)| ((node, key), value)));
            return;
        }
        if self.instance.is_some() {
            return;
        }
        let graph = self.current_graph();
        for (node_id, port, text) in binds {
            let Some(node) = graph
                .as_ref()
                .and_then(|graph| graph.nodes.iter().find(|node| node.id == node_id))
            else {
                continue;
            };
            let key = if node.kind == "input" {
                "value"
            } else if node.inputs.iter().any(|input| input.port == port) {
                port.as_str()
            } else {
                continue;
            };
            let ty = instance_input_type(graph.as_ref().unwrap(), &node_id, &port)
                .unwrap_or(PortType::Text);
            self.pending_params.insert(
                (node_id, key.to_string()),
                face_text_literal(ty, text),
            );
        }
        self.pending_params.extend(
            params
                .into_iter()
                .filter(|(node, _, _)| !node.contains('/'))
                .map(|(node, key, value)| ((node, key), value)),
        );
    }

    fn handle_bridge_call(&mut self, call: FaceBridgeCall) {
        let mine = self.instance.as_deref() == Some(call.instance.as_str())
            || (self.instance.is_none() && call.instance == "unbound");
        if !mine {
            return;
        }
        if call.surface && !self.live_package() {
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
                self.route_face_commits(vec![(node, port, text)], Vec::new());
            }
            BridgeCall::Run { outputs } => self.start_run(outputs),
            BridgeCall::Cancel => self.cancel_run(),
            BridgeCall::Control { node, name, value } => {
                // A refusal is said, never swallowed: the control stays
                // where the user put it and the status row says why.
                if let Err(refusal) = self.live_runtime.write(&call.instance, &node, &name, value) {
                    self.live_refusal = Some(format!("live: `{node}.{name}` was not written: {refusal}"));
                }
            }
            BridgeCall::Pulse { node, name } => {
                if let Err(refusal) = self.live_runtime.pulse(&call.instance, &node, &name) {
                    self.live_refusal = Some(format!("live: `{node}.{name}` was not triggered: {refusal}"));
                }
            }
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
                self.route_face_commits(Vec::new(), vec![(node, key, literal)]);
            }
        }
    }

    /// Run at the toolbar's parallelism: `1` is one design run (a numbered,
    /// locked instance, as Play always was); `max · N` queues a batch of N
    /// slices and the canvas follows the first.
    fn queue_run(&mut self, cx: &mut Cx) {
        if self.client().is_none() {
            return;
        }
        let parallel = if self.parallel_use_max {
            self.parallel_max.max(1)
        } else {
            1
        };
        if parallel == 1 {
            self.start_run(None);
            return;
        }
        if self.pending_graph || self.graph_retry.is_some() {
            self.deferred_batch = true;
            return;
        }
        if !self.pending_params.is_empty() {
            let Some(mut graph) = self.current_graph() else {
                return;
            };
            for ((node, key), value) in std::mem::take(&mut self.pending_params) {
                graph = graph_edit::set_param(&graph, &node, &key, value);
            }
            self.deferred_batch = true;
            self.put_graph(cx, graph);
            return;
        }
        let Some(flow) = self.selected.clone() else {
            return;
        };
        // Every slice reads the design values from the file; nothing is
        // overridden per batch.
        let request = CreateBatchRequest {
            parallel,
            inputs: None,
        };
        let planned_nodes = planned_nodes_for(self.current_graph().as_ref(), None);
        let revision = self
            .definition
            .as_ref()
            .map_or(0, |definition| definition.revision);
        self.io(move |client| IoResult::BatchCreated {
            result: client.create_batch(&flow, &request),
            flow,
            planned_nodes,
            revision,
        });
    }

    fn start_run(&mut self, outputs: Option<Vec<String>>) {
        // A package run waits for the values typed into its surface or its
        // faces to reach the instance first.
        let inputs_pending = self.live_package() && !self.pending_inputs.is_empty();
        if !self.pending_params.is_empty()
            || self.pending_graph
            || self.graph_retry.is_some()
            || self.input_flush_in_flight
            || self.run_start_in_flight
            || inputs_pending
        {
            self.deferred_run = Some(RunRequest { outputs });
            return;
        }
        if self.client().is_none() {
            return;
        }
        let planned_nodes = planned_nodes_for(self.current_graph().as_ref(), outputs.as_deref());
        let generation = self.attachment_generation;
        self.run_request_generation = self.run_request_generation.wrapping_add(1);
        let request_generation = self.run_request_generation;
        self.run_start_in_flight = true;
        let Some(flow) = self.selected.clone() else {
            self.run_start_in_flight = false;
            return;
        };
        if let Some(instance) = self.live_package_instance() {
            // The package's one instance runs; nothing new is created.
            self.run_start_in_flight = self.io(move |client| {
                let result = client
                    .start_run(&instance, outputs.as_deref())
                    .map(|started| (instance, started));
                IoResult::InstanceRunStarted {
                    flow,
                    generation,
                    request_generation,
                    planned_nodes,
                    journal: HashMap::new(),
                    result,
                }
            });
            return;
        }
        let label = self.next_run_label(&flow);
        self.run_start_in_flight = self.io(move |client| {
            let result = (|| {
                let request = fresh_run_instance_request(label);
                let created = client.create_instance(&flow, &request)?;
                let started = client.start_run(&created.instance, outputs.as_deref())?;
                Ok((created.instance, started))
            })();
            IoResult::InstanceRunStarted {
                flow,
                generation,
                request_generation,
                planned_nodes,
                journal: HashMap::new(),
                result,
            }
        });
    }

    fn next_run_label(&mut self, flow: &str) -> String {
        let listed = self
            .instances
            .iter()
            .filter(|row| row.flow == flow)
            .filter_map(|row| row.label.as_deref().and_then(run_label_ordinal))
            .max()
            .unwrap_or(0);
        let ordinal = self.run_ordinals.entry(flow.to_string()).or_insert(listed);
        *ordinal = (*ordinal).max(listed) + 1;
        format!("run #{}", *ordinal)
    }

    fn fetch_run_snapshot(&mut self) {
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
        // Validate and group the entire snapshot before replacing any visible
        // state. Conflicting or malformed ports must not become partial results.
        let grouped = (|| {
            let outputs = row.output_slots()?;
            let nodes = row.nodes.iter().map(|(node, source)| {
                source.output_slots()
                    .map(|slots| (node.clone(), slots))
                    .map_err(|error| format!("node `{node}`: {error}"))
            }).collect::<Result<HashMap<_, _>, String>>()?;
            Ok::<_, String>((outputs, nodes))
        })();
        let (outputs, mut node_outputs) = match grouped {
            Ok(grouped) => grouped,
            Err(error) => {
                self.set_error(cx, &format!("Malformed run snapshot: {error}"));
                return;
            }
        };
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
        self.statuses.clear();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.clear_run(cx);
        }
        if let Some(faces) = self.faces.as_mut() {
            faces.reset_run();
        }
        self.refresh_inspector(cx);

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
            if let Some(slots) = node_outputs.remove(node) {
                for (port, slot) in slots {
                    self.record_slot(cx, node, &port, slot, row.revision);
                }
            }
        }
        let revision = row.revision;
        for (node, slot) in outputs {
            let port = self.output_face_port(&node);
            self.record_slot(cx, &node, &port, slot, revision);
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
            self.clear_run_view(cx);
            return;
        }
        if self.live_package() {
            // The package's instance stays; only what its runs produced goes.
            self.request_clear_instance(self.run_is_active());
            return;
        }
        if self.run_is_active() {
            self.ui.modal(cx, ids!(clear_confirm)).open(cx);
        } else {
            self.leave_run_view(cx);
        }
    }

    /// Clear on a shown run: the canvas comes clean and returns to the
    /// design, editable; the run keeps its queue row, just no longer chosen.
    fn leave_run_view(&mut self, cx: &mut Cx) {
        self.clear_run_view(cx);
        if let Some(mut queue) = self.ui.widget(cx, ids!(running)).borrow_mut::<QueueList>() {
            queue.deselect(cx);
        }
        self.attach_instance(cx, None, true);
    }

    fn request_clear_instance(&mut self, cancel_first: bool) {
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
        self.statuses.clear();
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
        // Re-evaluating the faces is the narrowest way to restore every
        // generated widget to its declared empty state; fill_inputs below
        // immediately restores the preserved instance inputs.
        self.remount_faces(cx);
        self.refresh_inspector(cx);
        self.update_run_bar(cx);
        self.ui
            .label(cx, ids!(run_state))
            .set_text(cx, "idle · cleared");
        self.fetch_instance();
        self.refresh_instances();
    }

    fn undo(&mut self, cx: &mut Cx) {
        if self.instance.is_some() && !self.live_package() {
            return;
        }
        if self.revisions.len() < 2 {
            return;
        }
        let revision = self.revisions[self.revisions.len() - 2];
        if self.revert_to(cx, revision) {
            self.redo.push(self.revisions.pop().unwrap());
        }
    }

    fn redo(&mut self, cx: &mut Cx) {
        if self.instance.is_some() && !self.live_package() {
            return;
        }
        let Some(revision) = self.redo.last().copied() else {
            return;
        };
        if self.revert_to(cx, revision) {
            self.redo.pop();
            self.revisions.push(revision);
        }
    }

    fn revert_to(&mut self, cx: &mut Cx, revision: u64) -> bool {
        if self.instance.is_some() && !self.live_package() {
            return false;
        }
        if self.pending_graph || self.graph_retry.is_some() || self.unsaved
            || self.source_save_pending || self.pending_source_edit.is_some() {
            self.set_error(cx, "finish the pending graph or source edit before undoing or redoing");
            return false;
        }
        if let Some(name) = self.selected.clone() {
            let kept_name = name.clone();
            let generation = self.graph_write_generation.wrapping_add(1);
            if self.io(move |client| {
                let result = client.revert(&name, revision).and_then(|_| client.flow(&name));
                IoResult::GraphPut { name, generation, result }
            }) {
                self.package_write_started(&kept_name);
                self.graph_write_generation = generation;
                self.graph_write_source = Some(self.ui.widget(cx, ids!(source)).text());
                self.graph_write_layout_only = false;
                self.pending_graph = true;
                return true;
            }
        }
        self.set_error(cx, "the request lane is full; try undo or redo again in a moment");
        false
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
        if let Some(mut source) = self.ui.widget(cx, ids!(source))
            .borrow_mut::<makepad_code_editor::code_view::CodeView>()
        {
            let makepad_code_editor::code_view::CodeView { editor, session, .. } = &mut *source;
            if let Some(session) = session {
                editor.set_cursor_and_scroll(cx, makepad_code_editor::text::Position {
                    line_index: line.saturating_sub(1),
                    byte_index: 0,
                }, session);
            }
        }
    }

    fn highlight_caret_node(&mut self, cx: &mut Cx) {
        let source = self.ui.widget(cx, ids!(source));
        let Some(source) = source.borrow::<makepad_code_editor::code_view::CodeView>() else { return };
        let Some(session) = &source.session else { return };
        let selections = session.selections();
        let Some(selection) = selections.first() else { return };
        let line = selection.cursor.position.line_index + 1;
        let node = self.definition.as_ref().and_then(|definition| definition.graph.as_ref())
            .and_then(|graph| graph.nodes.iter()
                .filter(|node| node.loc.line as usize <= line)
                .max_by_key(|node| node.loc.line)
                .map(|node| node.id.clone()));
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_highlight(cx, node);
        }
    }

    fn set_modes(&mut self, cx: &mut Cx) {
        if self.shell_mode == ShellMode::Graph || !self.package_shown {
        }
        if self.source_mode {
            let bounds = self.ui.view(cx, ids!(workspace)).area().rect(cx);
            if !self.source_geometry_ready {
                self.source_size = dvec2(760.0, 620.0);
                self.source_origin = dvec2(
                    bounds.pos.x + (bounds.size.x - self.source_size.x).max(0.0) * 0.5,
                    bounds.pos.y + (bounds.size.y - self.source_size.y).max(0.0) * 0.5,
                );
                self.source_geometry_ready = true;
            }
            self.apply_source_geometry(cx, bounds);
        }
        self.ui
            .view(cx, ids!(source_overlay))
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
        left_split.as_splitter().set_collapse(
            cx, if self.left_hidden { SplitterCollapse::A } else { SplitterCollapse::None },
        );
        self.ui.view(cx, ids!(left_panel)).set_visible(cx, !self.left_hidden);
        // The inspector's whole splitter goes with the inspector: a folded
        // splitter keeps its bar, and a bar with nothing to open is a dead
        // strip on the graph. The canvas under the chrome shows through.
        self.ui.widget(cx, ids!(canvas_right_split)).as_splitter().set_collapse(cx, SplitterCollapse::None);
        self.ui.view(cx, ids!(inspector_host)).set_visible(cx, !self.inspector_hidden);
        self.ui.view(cx, ids!(right_panel)).set_visible(cx, !self.inspector_hidden);

        // The shell: without a package the editor is the window; with one,
        // the mode says which of the two panes (or both) show. Collapsing a
        // pane folds it away; nothing is unmounted or stopped.
        let has_package = self.package.is_some() && self.package_shown;
        let collapse = match (has_package, self.shell_mode) {
            (false, _) | (true, ShellMode::Graph) => SplitterCollapse::B,
            (true, ShellMode::App) => SplitterCollapse::A,
            (true, ShellMode::Split) => SplitterCollapse::None,
        };
        if let Some(mut split) = self.ui.widget(cx, ids!(shell_split)).borrow_mut::<Splitter>() {
            split.set_collapse(cx, collapse);
        }
        self.ui.view(cx, ids!(app_pane)).set_visible(cx, has_package);
        self.refresh_surface(cx, false);

        self.ui.button(cx, ids!(side_btn)).set_text(
            cx,
            "Source",
        );
        self.update_canvas_fit_insets(cx);
        if self.selected_node.is_some() {
            self.refresh_models(true);
        }
        self.ui.redraw(cx);
    }

    fn apply_source_geometry(&mut self, cx: &mut Cx, bounds: Rect) {
        let min_size = dvec2(420.0, 280.0);
        let size = dvec2(
            self.source_size.x.clamp(min_size.x, bounds.size.x.max(min_size.x)),
            self.source_size.y.clamp(min_size.y, bounds.size.y.max(min_size.y)),
        );
        let origin = dvec2(
            self.source_origin.x.clamp(bounds.pos.x, (bounds.pos.x + bounds.size.x - size.x).max(bounds.pos.x)),
            self.source_origin.y.clamp(bounds.pos.y, (bounds.pos.y + bounds.size.y - size.y).max(bounds.pos.y)),
        );
        self.source_size = size;
        self.source_origin = origin;
        let geometry = Rect { pos: origin, size };
        if self.source_applied_geometry == Some(geometry) {
            return;
        }
        self.source_applied_geometry = Some(geometry);
        self.ui.view(cx, ids!(source_card)).set_walk(
            cx,
            Walk::fixed(size.x, size.y).with_abs_pos(origin),
        );
    }

    fn handle_source_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let bounds = self.ui.view(cx, ids!(workspace)).area().rect(cx);
        if let Some((start, resize)) = self.source_drag {
            match event {
                Event::MouseMove(mouse) => {
                    let delta = mouse.abs - start;
                    if resize {
                        self.source_size += delta;
                    } else {
                        self.source_origin += delta;
                    }
                    self.source_drag = Some((mouse.abs, resize));
                    self.apply_source_geometry(cx, bounds);
                    return true;
                }
                Event::MouseUp(_) => {
                    self.source_drag = None;
                    return true;
                }
                // Drawing, timers, and worker arrivals must keep flowing
                // while the pointer is held, so dragging remains live.
                _ => return false,
            }
        }
        if !self.source_mode {
            return false;
        }
        let card = self.ui.widget(cx, ids!(source_card)).area().rect(cx);
        let header = self.ui.widget(cx, ids!(head)).area().rect(cx);
        let save = self.ui.widget(cx, ids!(save_btn)).area().rect(cx);
        let discard = self.ui.widget(cx, ids!(discard_source)).area().rect(cx);
        let close = self.ui.widget(cx, ids!(close_source)).area().rect(cx);
        match event {
            Event::MouseDown(mouse)
                if header.contains(mouse.abs)
                    && !save.contains(mouse.abs)
                    && !(self.source_layout_conflict && discard.contains(mouse.abs))
                    && !close.contains(mouse.abs) =>
            {
                self.source_drag = Some((mouse.abs, false));
                true
            }
            Event::MouseDown(mouse)
                if card.contains(mouse.abs)
                    && mouse.abs.x >= card.pos.x + card.size.x - 24.0
                    && mouse.abs.y >= card.pos.y + card.size.y - 24.0 =>
            {
                self.source_drag = Some((mouse.abs, true));
                true
            }
            _ => false,
        }
    }

    fn point_over_canvas_chrome(&self, cx: &mut Cx, point: DVec2) -> bool {
        self.canvas_chrome_rects(cx)
            .iter()
            .any(|rect| rect.contains(point))
    }

    fn canvas_chrome_rects(&self, cx: &mut Cx) -> Vec<Rect> {
        let mut rects = vec![
            self.ui.widget(cx, ids!(menu_bar)).area().rect(cx),
            self.ui.widget(cx, ids!(toolbar)).area().rect(cx),
        ];
        if !self.inspector_hidden {
            rects.push(self.ui.widget(cx, ids!(right_panel)).area().rect(cx));
            rects.push(self.ui.widget(cx, ids!(canvas_right_split)).area().rect(cx));
        }
        if !self.left_hidden {
            rects.push(self.ui.widget(cx, ids!(column_split)).area().rect(cx));
            rects.push(self.ui.widget(cx, ids!(left_panel)).area().rect(cx));
        }
        if self.source_mode {
            rects.push(self.ui.widget(cx, ids!(source_card)).area().rect(cx));
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

    /// Whether a window point is over the app pane while it shows.
    fn point_over_app_pane(&self, cx: &mut Cx, point: DVec2) -> bool {
        self.package.is_some()
            && self.package_shown
            && self.ui.widget(cx, ids!(app_pane)).area().rect(cx).contains(point)
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
        // Both panels stand to the graph's left, the inspector after the
        // flow column; the inset is the right edge of the outermost one that
        // shows. Before a layout the defaults stand in (244 + 8 and 300 + 8).
        let edge = |rect: Rect, fallback: f64| {
            if canvas_rect.size.x <= 0.0 || rect.size.x <= 0.0 {
                fallback
            } else {
                (rect.pos.x + rect.size.x - canvas_rect.pos.x + 8.0).max(8.0)
            }
        };
        let left = match (self.left_hidden, self.inspector_hidden) {
            (true, true) => 8.0,
            (false, true) => edge(left_rect, 252.0),
            (true, false) => edge(right_rect, 316.0),
            (false, false) => edge(right_rect, 568.0),
        };
        let right = 8.0;
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

    /// The bar must receive the opening press before canvas selection and
    /// mounted faces. Once open, its popup owns input through the event that
    /// closes it; only an outside press dismisses and passes through.
    fn route_menu_input(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let bar = self.ui.menu_bar(cx, ids!(menu_bar));
        let area = bar.area();
        let rect = area.clipped_rect(cx);
        let was_open = bar.is_open();
        let over_bar = match event {
            Event::MouseDown(e) => rect.contains(e.abs),
            Event::MouseMove(e) => rect.contains(e.abs),
            Event::MouseUp(e) => rect.contains(e.abs),
            Event::Scroll(e) => rect.contains(e.abs),
            Event::LongPress(e) => rect.contains(e.abs),
            Event::TouchUpdate(e) => e.touches.iter().any(|touch| rect.contains(touch.abs)),
            _ => false,
        };
        let pointer = matches!(
            event,
            Event::MouseDown(_)
                | Event::MouseMove(_)
                | Event::MouseUp(_)
                | Event::MouseLeave(_)
                | Event::Scroll(_)
                | Event::TouchUpdate(_)
                | Event::LongPress(_)
        );
        let key = is_focus_routed_input(event) || matches!(event, Event::PhysicalKeyboard(_));
        if !(was_open && (pointer || key)
            || pointer && (over_bar || cx.fingers.is_area_captured(area)))
        {
            return false;
        }
        bar.handle_event(cx, event, &mut Scope::empty());
        // A title toggles on down too: closing it must not dispatch that down
        // a second time through the tree and immediately reopen the menu.
        let dismiss_press = match event {
            Event::MouseDown(_) => true,
            Event::TouchUpdate(e) => !e.touches.is_empty() && e.touches.iter().all(|touch| {
                touch.state == makepad_widgets::makepad_platform::event::TouchState::Start
            }),
            _ => false,
        };
        if dismiss_press && !over_bar && !bar.is_open() {
            return false;
        }
        true
    }

    fn handle_menu(&mut self, cx: &mut Cx, id: LiveId) {
        if self.nested_selection.is_some()
            && [live_id!(delete_selection), live_id!(duplicate)].contains(&id)
        {
            self.set_error(cx, "this component is imported; edit its module and repin the package");
            return;
        }
        match id {
            id if id == live_id!(new_from_template) => self.show_templates(cx, true),
            id if id == live_id!(open_flow) => self.open_next_flow(cx),
            id if id == live_id!(save) => {
                if let Some(name) = self.selected.clone() {
                    let source = self.ui.widget(cx, ids!(source)).text();
                    if self.source_mode && !source.is_empty() {
                        self.save_source(cx, name, source, true);
                    }
                }
            }
            id if id == live_id!(revert) || id == live_id!(undo) => self.undo(cx),
            id if id == live_id!(redo) => self.redo(cx),
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
                        to_node, to_port, key, ..
                    }) => Some(CanvasEdit::Disconnect { to_node, to_port, key }),
                    None => None,
                };
                if let Some(edit) = edit {
                    if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                        FlowCanvas::select(&mut canvas, cx, None);
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
                    FlowCanvas::select(&mut canvas, cx, first.clone());
                }
                self.selected_node = first;
                self.refresh_inspector(cx);
            }
            id if id == live_id!(duplicate) => self.duplicate_selected(cx),
            id if id == live_id!(wires_routed) => self.set_wire_mode(cx, WireMode::Routed),
            id if id == live_id!(wires_bezier) => self.set_wire_mode(cx, WireMode::Bezier),
            id if id == live_id!(view_studio) => self.show_studio(cx, true),
            id if id == live_id!(view_graphs) => self.show_studio(cx, false),
            id if id == live_id!(view_source) => {
                self.source_mode = true;
                self.set_modes(cx);
            }
            id if id == live_id!(view_inspector) => {
                self.source_mode = false;
                self.inspector_hidden = !self.inspector_hidden;
                self.set_modes(cx);
            }
            id if id == live_id!(flip_card) => {
                if let Some((path, Selection::Node(node))) = self.nested_selection.clone() {
                    self.apply_component_layout(cx, path, CanvasEdit::Flip { node });
                } else if let Some(node) = self.selected_node.clone() {
                    self.apply_edit(cx, CanvasEdit::Flip { node });
                }
            }
            id if id == live_id!(toggle_left) => {
                self.left_hidden = !self.left_hidden;
                self.set_modes(cx);
            }
            id if id == live_id!(mode_app) => {
                self.shell_mode = ShellMode::App;
                self.set_modes(cx);
            }
            id if id == live_id!(mode_split) => {
                self.shell_mode = ShellMode::Split;
                self.set_modes(cx);
            }
            id if id == live_id!(mode_graph) => {
                self.shell_mode = ShellMode::Graph;
                self.set_modes(cx);
            }
            id if id == live_id!(leave_container) => self.leave_container(cx),
            id if id == live_id!(open_package) => {
                use makepad_widgets::makepad_platform::file_dialogs::FileDialog;
                self.package_dialog_open = true;
                cx.open_select_folder_dialog(FileDialog::new().set_title("Open a Flow package".to_string()));
            }
            id if id == live_id!(reload_package) => {
                if let Some(root) = self.package.as_ref().map(|session| session.package.root.clone()) {
                    self.open_package(cx, root);
                }
            }
            id if id == live_id!(close_package) => self.close_package(cx),
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
            id => {
                let template = self
                    .templates
                    .iter()
                    .find(|template| template_menu_id(&template.name) == id)
                    .map(|template| template.name.clone());
                if let Some(template) = template {
                    self.create_from_template(&template, false);
                }
            }
        }
    }

    fn with_canvas(&mut self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut FlowCanvas)) {
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            f(cx, &mut canvas);
        }
    }

    /// Reconcile the engine's instance listing, independent of editor
    /// selection. Failed fetches/evaluations retain last-good audio.
    fn sync_live(&mut self, cx: &mut Cx) {
        self.refresh_live();
        let previous_status = self.instance.as_ref().map(|instance| self.live_runtime.status_line(instance));
        if let (Some(registry), Some(snapshot)) = (&self.registry, &self.live_snapshot) {
            self.live_runtime.reconcile(cx, registry, snapshot);
        }
        let current_status = self.instance.as_ref().map(|instance| self.live_runtime.status_line(instance));
        if previous_status != current_status {
            self.refresh_surface(cx, false);
        }
        self.ui.widget(cx, ids!(frame_stage)).as_frame_stage()
            .set_producers(cx, self.live_runtime.producers());
        let running = self.live_runtime.is_running();
        if running && self.live_timer.is_empty() {
            self.live_timer = cx.start_interval(1.0 / 30.0);
        } else if !running && !self.live_timer.is_empty() {
            cx.stop_timer(self.live_timer);
            self.live_timer = Timer::empty();
        }
    }

    /// One step of the live host, then the surface's live readouts.
    fn pump_live(&mut self, cx: &mut Cx) {
        let events = self.live_runtime.pump(cx);
        for refusal in self.live_runtime.take_write_refusals() {
            log!("{refusal}");
            self.live_refusal = Some(refusal);
        }
        let Some(instance) = self.instance.clone() else { return };
        let changed = events.iter().any(|event| match event {
            makepad_flow::live::LiveEvent::Installed { instance: id, .. }
            | makepad_flow::live::LiveEvent::Failed { instance: id, .. }
            | makepad_flow::live::LiveEvent::Idle { instance: id, .. }
            | makepad_flow::live::LiveEvent::Stopped { instance: id } => id.0 == instance,
        });
        if changed || self.live_refusal.is_some() {
            if changed {
                    self.bind_graph_frames(cx);
            }
            // The endpoints changed with the plan: bind the surface again.
            self.refresh_surface(cx, false);
            if let Some(refusal) = self.live_refusal.take() {
                self.ui.label(cx, ids!(surface_error)).set_text(cx, &refusal);
            }
        }
        let bindings = self.with_surface(cx, |_, view| view.endpoint_bindings()).unwrap_or_default();
        for (node, name, writes) in bindings {
            // A control follows the host when the plan changes (it may carry
            // a value over); while it is only being moved, it is the source.
            if writes && !changed {
                continue;
            }
            if let Some(value) = self.live_runtime.read(&instance, &node, &name) {
                self.with_surface(cx, |cx, view| view.push_number(cx, &node, &name, value));
            }
        }
    }

    fn frame_sources(&self) -> frames::FrameSources {
        let mut sources = frames::FrameSources::default();
        for producer in self.live_runtime.producers() {
            sources.add(producer.source());
        }
        sources
    }

    fn bind_graph_frames(&mut self, cx: &mut Cx) {
        let sources = self.frame_sources();
        if let Some(faces) = &self.faces {
            sources.bind_host(cx, faces);
            if let Some(hierarchy) = &faces.hierarchy {
                hierarchy.bind_frames(cx, &sources);
            }
        }
    }

    fn tick_frames(&mut self, cx: &mut Cx) {
        if self.live_runtime.tick_frames(cx, Cx::time_now()) {
            self.ui.redraw(cx);
            self.frame_next = cx.new_next_frame();
        }
    }

    /// Show the generation studio, or the graph workbench.
    fn show_studio(&mut self, cx: &mut Cx, shown: bool) {
        self.studio_shown = shown;
        self.ui.view(cx, ids!(studio_page)).set_visible(cx, shown);
        self.ui.widget(cx, ids!(shell_split)).set_visible(cx, !shown);
        self.ui.redraw(cx);
    }

    /// The studio's pump: its worker's and engines' results, bounded.
    fn pump_studio(&mut self, cx: &mut Cx) {
        // The page runs a preset with its own Run; nothing else here acts
        // on what it reports.
        let _ = self.studio.pump(cx, &self.ui, self.studio_shown, &makepad_flow_studio::NoLiveRoots);
    }

    fn shutdown(&mut self, cx: &mut Cx) {
        self.studio.stop();
        // Dispatch a best-effort fade/removal without waiting on audio.
        // Process shutdown may end callbacks before they acknowledge it.
        self.live_runtime.stop_all();
        self.services.shutdown();
        self.detach_faces(cx);
        self.with_surface(cx, |cx, view| view.unmount(cx));
        if let Some(subscriber) = self.subscriber.take() {
            subscriber.request_stop();
        }
        self.session_status = None;
        self.client_handle = None;
        if let Some(service) = self.service.as_mut() {
            service.stop();
        }
    }

    fn refresh_ai_context(&mut self) {
        let selected = self
            .selected
            .as_deref()
            .and_then(|name| self.flows.iter().find(|flow| flow.name == name));
        let open_view = if self.source_mode {
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

    /// Open a port's value: the one value, or the element `index` of an
    /// ordered port (an ordered port without an index opens nothing; no
    /// element stands in for the sequence).
    fn open_value(&mut self, cx: &mut Cx, node: &str, port: &str, index: Option<usize>) {
        let value = self
            .outputs
            .get(node)
            .and_then(|ports| ports.iter().find(|(name, _)| name == port))
            .and_then(|(_, slot)| match (slot, index) {
                (ValueSlotRef::One(value), _) => Some(value.clone()),
                (ValueSlotRef::Seq(sequence), Some(index)) => sequence.values.get(index).cloned(),
                (ValueSlotRef::Seq(_), None) => None,
            });
        let Some(value) = value else {
            return;
        };
        let label = match index {
            Some(index) => format!("{node}.{port}[{index}]"),
            None => format!("{node}.{port}"),
        };
        self.preview_bytes = None;
        self.preview_digest = Some((value.digest.clone(), label));
        self.preview_ref = Some(value.clone());
        // The value's node is qualified; it is selected when the level
        // shown has it (a wire probe's source may lie in a container).
        if let Some(local) = self.level_node(node) {
            if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                FlowCanvas::select(&mut canvas, cx, Some(local.clone()));
            }
            self.selected_node = Some(local);
        }
        self.source_mode = false;
        self.set_modes(cx);
        match self.values.get(&value.digest).map(|bytes| self.typed_preview_bytes(&bytes)) {
            Some(bytes) if self.preview_is_media(&bytes) => {
                self.show_image_viewer(cx, node, port, &bytes)
            }
            Some(bytes) => self.show_preview(cx, &bytes),
            None => {
                if let Some(client) = self.client() {
                    self.values.request(&value.digest, client, &self.data_lane);
                }
                self.refresh_inspector(cx);
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.studio_shown {
            self.studio.handle_actions(cx, &self.ui, actions, &makepad_flow_studio::NoLiveRoots);
        }
        for action in actions {
            if let Some(dialog) = action.downcast_ref::<
                makepad_widgets::makepad_platform::file_dialogs::FileDialogAction,
            >() {
                use makepad_widgets::makepad_platform::file_dialogs::FileDialogAction;
                if self.package_dialog_open {
                    match dialog {
                        FileDialogAction::FolderSelected(path) => {
                            self.package_dialog_open = false;
                            let path = path.clone();
                            self.open_package(cx, path);
                        }
                        FileDialogAction::FolderCancelled => self.package_dialog_open = false,
                        _ => {}
                    }
                }
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
                        self.nested_selection = None;
                        self.selected_node = Some(node.clone());
                        if let Some(mut canvas) =
                            self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                        {
                            FlowCanvas::select(&mut canvas, cx, Some(node.clone()));
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
            self.create_from_template(&template, false);
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
        if !self.editing_locked() && self.ui.button(cx, ids!(save_btn)).clicked(actions) {
            if let Some(name) = self.selected.clone() {
                let source = self.ui.widget(cx, ids!(source)).text();
                self.save_source(cx, name, source, true);
            }
        }
        if self.ui.button(cx, ids!(discard_source)).clicked(actions) {
            if self.pending_graph || self.graph_retry.is_some() {
                self.set_error(cx, "wait for the graph layout to finish saving before discarding the source draft");
            } else {
                self.unsaved = false;
                self.pending_source_edit = None;
                self.source_layout_conflict = false;
                self.ui.button(cx, ids!(discard_source)).set_visible(cx, false);
                if let Some(definition) = self.definition.as_ref() {
                    self.ui.widget(cx, ids!(source)).set_text(cx, &definition.source);
                }
                if let Some(name) = self.selected.clone() { self.load_flow(name); }
            }
        }
        let source_changed = self
            .ui
            .widget(cx, ids!(source))
            .widget_uid();
        if actions.find_widget_action(source_changed).is_some_and(|action| {
            matches!(action.cast::<CodeViewAction>(), CodeViewAction::Changed)
        }) {
            self.unsaved = true;
            if !self.editing_locked() && !self.source_layout_conflict {
                if let Some(name) = self.selected.clone() {
                    self.pending_source_edit = Some((
                        name,
                        self.ui.widget(cx, ids!(source)).text(),
                        std::time::Instant::now() + std::time::Duration::from_millis(350),
                    ));
                }
            }
        }
        if let Some(selected) = self.ui.combo_box(cx, ids!(parallel)).changed(actions) {
            self.parallel_use_max = selected == 1;
        }
        if self.ui.button(cx, ids!(run_btn)).clicked(actions) {
            self.queue_run(cx);
        }
        if self.ui.button(cx, ids!(design_btn)).clicked(actions) {
            self.attach_instance(cx, None, true);
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
            self.leave_run_view(cx);
        }
        if self.ui.button(cx, ids!(fit_btn)).clicked(actions) {
            self.with_canvas(cx, |cx, canvas| canvas.fit(cx));
        }
        if self.ui.button(cx, ids!(side_btn)).clicked(actions) {
            self.source_mode = !self.source_mode;
            self.set_modes(cx);
        }
        if self.ui.button(cx, ids!(close_source)).clicked(actions) {
            self.source_mode = false;
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
            // The folded bar was dragged out: the panels come back where
            // the hand left them, not at the width remembered from before.
            self.left_hidden = false;
            self.left_panel_align = None;
            self.set_modes(cx);
        }
        let shell_reopened = self
            .ui
            .widget(cx, ids!(shell_split))
            .borrow::<Splitter>()
            .is_some_and(|splitter| {
                splitter.collapsed(actions) == Some(SplitterCollapse::None) || splitter.changed(actions).is_some()
            });
        if shell_reopened {
            // The bar of a folded app pane was dragged out. With a package
            // that means Split; without one there is nothing to show and the
            // pane folds again.
            if self.package.is_some() && self.package_shown && self.shell_mode == ShellMode::Graph {
                self.shell_mode = ShellMode::Split;
                self.set_modes(cx);
            } else if self.package.is_none() || !self.package_shown {
                self.set_modes(cx);
            }
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
                    self.nested_selection = None;
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
                FlowCanvasAction::Open { node } => self.enter_container(cx, node),
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
                        graph_edit::types_with_compatible_input(&self.catalog, ty.clone())
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
                // Automatic port facing is a canvas display choice, including
                // in imported modules. Opening or laying out a graph does not
                // edit its source. Explicit user flips use CanvasEdit::Flip.
                FlowCanvasAction::AutoFlip(changes) => {
                    self.display_flips.entry(self.path.clone()).or_default().extend(changes);
                    self.refresh_inspector(cx);
                }
            }
        }

        let mut nested_actions = Vec::new();
        if let Some(hierarchy) = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref()) {
            hierarchy.actions(actions, &mut nested_actions);
        }
        for (path, action) in nested_actions {
            match action {
                FlowCanvasAction::Select(selection) => {
                    self.selected_node = selection.as_ref().and_then(Selection::node)
                        .map(|node| format!("{}{node}", hierarchy::prefix(&path)));
                    self.nested_selection = selection.map(|selection| (path, selection));
                    self.refresh_inspector(cx);
                    self.update_menu_state(cx);
                }
                FlowCanvasAction::Open { node } => {
                    let is_component = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref())
                        .and_then(|hierarchy| hierarchy.graph(&path))
                        .is_some_and(|graph| graph.nodes.iter().any(|item| item.id == node && item.kind == "component"));
                    if is_component {
                        self.remember_level(cx);
                        self.path = path;
                        self.path.push(node);
                        self.show_level(cx);
                    }
                }
                FlowCanvasAction::Edit(edit) => self.apply_component_layout(cx, path, edit),
                FlowCanvasAction::OpenPalette { .. } => {
                    if let Some(hierarchy) = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref()) {
                        hierarchy.restore_readonly(cx, &path);
                    }
                    self.set_error(cx, "this component is imported; edit its module and repin the package");
                }
                FlowCanvasAction::AutoFlip(changes) => {
                    self.display_flips.entry(path).or_default().extend(changes);
                }
                _ => {}
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
            if (self.editing_locked() || self.nested_selection.is_some()) && inspector_action_edits(&action)
                && !matches!(action, InspectorAction::FlipCard { .. }) {
                continue;
            }
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
                    if let Some((path, _)) = self.nested_selection.clone() {
                        self.apply_component_layout(cx, path, CanvasEdit::Flip { node });
                    } else {
                        self.apply_edit(cx, CanvasEdit::Flip { node });
                    }
                }
                InspectorAction::EnterComponent(node) => self.enter_inspected_container(cx, node),
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
                        FlowCanvas::select(&mut canvas, cx, Some(new_id));
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
                    self.select_inspection(cx, Selection::Node(node));
                    self.refresh_models(true);
                }
                InspectorAction::Disconnect { node, port, key } => {
                    if let Some(graph) = self.current_graph() {
                        self.put_graph(cx, graph_edit::disconnect(&graph, &node, &port, key));
                    }
                }
                InspectorAction::MoveEntry { node, port, key, delta } => {
                    if let Some(graph) = self.current_graph() {
                        self.put_graph(cx, graph_edit::move_entry(&graph, &node, &port, key, delta));
                    }
                }
                InspectorAction::SelectEntry { from_node, from_port, node, port, key } => {
                    self.select_inspection(cx, Selection::Edge { from_node, from_port, to_node: node, to_port: port, key });
                }
                InspectorAction::JumpSource(node) => {
                    if let Some((path, _)) = self.nested_selection.clone() {
                        self.remember_level(cx);
                        self.path = path;
                        self.show_level(cx);
                    }
                    self.source_mode = true;
                    self.set_modes(cx);
                    self.jump_to_node(cx, &node);
                }
                InspectorAction::SaveValue { node, port, index } => {
                    let digest = self
                        .outputs
                        .get(&node)
                        .and_then(|ports| ports.iter().find(|(name, _)| name == &port))
                        .and_then(|(_, slot)| match (slot, index) {
                            (ValueSlotRef::One(value), _) => Some(value.digest.clone()),
                            (ValueSlotRef::Seq(sequence), Some(index)) => {
                                sequence.values.get(index).map(|value| value.digest.clone())
                            }
                            (ValueSlotRef::Seq(_), None) => None,
                        });
                    self.save_when_ready = digest;
                    self.open_value(cx, &node, &port, index);
                    if self.preview_bytes.is_some() {
                        self.save_when_ready = None;
                        self.save_preview(cx);
                    }
                }
                InspectorAction::CopyDigest(digest) => cx.copy_to_clipboard(&digest),
                InspectorAction::OpenValue { node, port, index } => self.open_value(cx, &node, &port, index),
                InspectorAction::RefreshAssets => self.refresh_assets(cx, false, false, false),
                InspectorAction::LoadMoreAssets => self.refresh_assets(cx, true, true, false),
                InspectorAction::OpenAsset(asset) => self.open_asset(cx, asset),
            }
        }

        // Queue.
        let running_actions = self
            .ui
            .widget(cx, ids!(running))
            .borrow_mut::<QueueList>()
            .map(|mut list| list.actions(cx, actions))
            .unwrap_or_default();
        for action in running_actions {
            match action {
                QueueAction::Select {
                    run_id,
                    instance,
                    flow,
                    revision,
                    state,
                    planned_nodes,
                    started_ms,
                    finished_ms,
                } => {
                    self.focus_instance(cx, instance, Some(flow));
                    let now_ms = wall_clock_ms();
                    self.display_run(
                        cx,
                        RunInfo {
                            run_id,
                            state: run_state_name(state).into(),
                            started: self.time
                                - now_ms.saturating_sub(started_ms) as f64 / 1000.0,
                            finished_secs: finished_ms
                                .map(|finished| finished.saturating_sub(started_ms) as f64 / 1000.0),
                            revision,
                            planned_nodes,
                        },
                    );
                    self.fetch_run_snapshot();
                }
                QueueAction::CancelRun {
                    run_id,
                    instance,
                    batch,
                } => {
                    if self.run.as_ref().is_some_and(|run| run.run_id == run_id) {
                        self.clear_run_view(cx);
                    }
                    if batch {
                        self.io(move |client| IoResult::Done(client.cancel_run(&run_id)));
                    } else {
                        // A single run's x is Stop: the run goes, its instance
                        // goes, and the canvas returns to the design.
                        let target = instance.clone();
                        self.live_runtime.stop(&target, self.connected_server);
                        self.io(move |client| {
                            let _ = client.cancel_run(&run_id);
                            IoResult::Done(delete_instance_if_present(client, &target))
                        });
                        if self.instance.as_deref() == Some(instance.as_str()) {
                            self.attach_instance(cx, None, true);
                        }
                    }
                }
                QueueAction::CancelBatch(batch) => {
                    self.io(move |client| {
                        IoResult::Done(client.cancel_batch(&batch).map(|_| ()))
                    });
                }
                QueueAction::ClearAll(plan) => {
                    self.queue_generation = self.queue_generation.wrapping_add(1);
                    self.queue_refresh_after_stale = false;
                    for batch in plan.batches {
                        self.io(move |client| {
                            IoResult::Done(client.clear_batch(&batch).map(|_| ()))
                        });
                    }
                    let mut detach = false;
                    for (run_id, instance) in plan.singles {
                        detach |= self.instance.as_deref() == Some(instance.as_str());
                        self.live_runtime.stop(&instance, self.connected_server);
                        self.io(move |client| {
                            let _ = client.cancel_run(&run_id);
                            IoResult::Done(delete_instance_if_present(client, &instance))
                        });
                    }
                    if detach {
                        self.attach_instance(cx, None, true);
                    }
                    self.clear_run_view(cx);
                }
                QueueAction::OpenAsset(asset) => {
                    if let Some(mut inspector) =
                        self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>()
                    {
                        inspector.show_asset(cx, &asset);
                    }
                    self.refresh_assets(cx, false, false, false);
                }
            }
        }

        // Faces: the centralized router commits only in editable design mode.
        let mut opens = Vec::new();
        if let Some(faces) = self.faces.as_mut() {
            let binds = faces.bind_changes(cx, actions);
            let mut params = faces.param_changes(cx, actions);
            params.extend(faces.model_changes(cx, actions));
            opens = faces.open_requests(actions);
            self.route_face_commits(binds, params);
        }
        for (node, port) in opens {
            // A face opens its one value; an ordered port is opened element
            // by element from the inspector.
            self.open_value(cx, &node, &port, None);
        }
        self.refresh_ai_context();
    }
}

/// Queue cleanup is intentionally idempotent for a stale failed row: the
/// server may have retired its instance while the retained run row remains.
/// Other delete failures still reach the normal UI error path.
fn delete_instance_if_present(
    client: &makepad_flow::client::FlowClient,
    instance: &str,
) -> Result<(), makepad_flow::client::ClientError> {
    match client.delete_instance(instance) {
        Err(error) if is_absent_instance_error(&error) => Ok(()),
        result => result,
    }
}

fn is_absent_instance_error(error: &makepad_flow::client::ClientError) -> bool {
    matches!(error, makepad_flow::client::ClientError::Http { status: 404, body }
        if body.to_ascii_lowercase().contains("instance not found"))
}

impl App {
    fn close_preview(&mut self, cx: &mut Cx) {
        self.preview_digest = None;
        self.preview_ref = None;
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

    fn typed_preview_bytes(&self, bytes: &makepad_flow::ValueBytes) -> makepad_flow::ValueBytes {
        let mut typed = bytes.clone();
        if let Some(value) = self.preview_ref.as_ref().filter(|value| value.digest == bytes.digest) {
            typed.content_type = value.content_type.clone();
        }
        typed
    }

    fn preview_is_media(&self, bytes: &makepad_flow::ValueBytes) -> bool {
        !self.preview_ref.as_ref().is_some_and(|value| matches!(&value.ty, PortType::Registered(_)))
            && is_viewable_media(bytes)
    }

    fn show_preview(&mut self, cx: &mut Cx, bytes: &makepad_flow::ValueBytes) {
        let opaque = self.preview_ref.as_ref()
            .filter(|value| matches!(&value.ty, PortType::Registered(_)))
            .map(|value| format!("{} · {} · sha256:{}", value.ty.as_str(), faces::size_text(value.bytes), value.digest));
        let text = if let Some(text) = opaque.as_ref() {
            text.clone()
        } else if bytes.content_type.starts_with("image/") {
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
            if let Some(text) = opaque.as_ref() {
                value.set_text(cx, text);
            } else if bytes.content_type.starts_with("image/") {
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

    fn step_image_viewer(&mut self, cx: &mut Cx, direction: i32) {
        if let Some((id, _)) = self.preview_digest.as_ref().filter(|(id, _)| id.starts_with("ast_")) {
            let next = self.ui.widget(cx, ids!(inspector)).borrow::<Inspector>()
                .and_then(|inspector| inspector.adjacent_asset(id, direction));
            if let Some(asset) = next { self.open_asset(cx, asset); }
            return;
        }
        let current_digest = self.preview_digest.as_ref().map(|(digest, _)| digest.as_str());
        let running_pictures = current_digest.and_then(|digest| {
            self.instances
                .iter()
                .find(|row| row.outputs.values().any(|value| value.digest == digest))
                .map(|row| {
                    row.outputs
                        .iter()
                        .filter(|(_, value)| {
                            !matches!(&value.ty, PortType::Registered(_)) && matches!(
                                makepad_media_view::media_kind(&value.content_type, &[]),
                                makepad_media_view::MediaKind::Image
                                    | makepad_media_view::MediaKind::Video
                                    | makepad_media_view::MediaKind::Audio
                                    | makepad_media_view::MediaKind::Mesh
                                    | makepad_media_view::MediaKind::Splat
                            )
                        })
                        .map(|(label, value)| {
                            (row.instance.clone(), label.clone(), value.digest.clone())
                        })
                        .collect::<Vec<_>>()
                })
        });
        let running_gallery = running_pictures.is_some();
        let running_pictures = running_pictures.map(|pictures| {
            pictures
                .into_iter()
                .map(|(node, port, digest)| (node, port, None, digest))
                .collect::<Vec<_>>()
        });
        // Every media value, an ordered port's elements each with its index.
        let mut pictures: Vec<(String, String, Option<usize>, String)> = running_pictures.unwrap_or_else(|| {
            self.outputs
                .iter()
                .flat_map(|(node, ports)| {
                    ports.iter().flat_map(move |(port, slot)| {
                        let ordered = slot.is_ordered();
                        slot.values().iter().enumerate().filter_map(move |(index, value)| {
                            (!matches!(&value.ty, PortType::Registered(_)) && matches!(
                                makepad_media_view::media_kind(&value.content_type, &[]),
                                makepad_media_view::MediaKind::Image
                                    | makepad_media_view::MediaKind::Video
                                    | makepad_media_view::MediaKind::Audio
                                    | makepad_media_view::MediaKind::Mesh
                                    | makepad_media_view::MediaKind::Splat
                            ))
                            .then(|| (node.clone(), port.clone(), ordered.then_some(index), value.digest.clone()))
                        })
                    })
                })
                .collect()
        });
        pictures.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
        if pictures.is_empty() {
            return;
        }
        let current = current_digest
            .and_then(|digest| pictures.iter().position(|(_, _, _, item)| item == digest))
            .unwrap_or(0);
        let next = (current as i32 + direction).rem_euclid(pictures.len() as i32) as usize;
        let (node, port, index, _) = pictures[next].clone();
        if running_gallery {
            let digest = pictures[next].3.clone();
            self.preview_digest = Some((digest.clone(), format!("{node}.{port}")));
            self.preview_ref = self.instances.iter().find(|row| row.instance == node)
                .and_then(|row| row.outputs.get(&port)).cloned();
            self.preview_bytes = None;
            if let Some(bytes) = self.values.get(&digest).map(|bytes| self.typed_preview_bytes(&bytes)) {
                self.show_image_viewer(cx, &node, &port, &bytes);
            } else if let Some(client) = self.client() {
                self.values.request(&digest, client, &self.data_lane);
            }
        } else {
            self.open_value(cx, &node, &port, index);
        }
    }

    fn drain_values(&mut self, cx: &mut Cx) {
        for arrival in self.values.drain() {
            match arrival {
                Ok(digest) => {
                    if let Some(faces) = self.faces.as_mut() {
                        faces.deliver_bytes(cx, &mut self.values, &digest);
                    }
                    let mut values = std::mem::take(&mut self.values);
                    self.with_surface(cx, |cx, view| view.deliver_bytes(cx, &mut values, &digest));
                    self.values = values;
                    if self.inspector_shows_edge(cx) {
                        self.refresh_inspector(cx);
                    }
                    self.refill_boundary_faces(cx, &digest);
                    if self
                        .preview_digest
                        .as_ref()
                        .is_some_and(|(wanted, _)| *wanted == digest)
                    {
                        if let Some(bytes) = self.values.get(&digest).map(|bytes| self.typed_preview_bytes(&bytes)) {
                            let label = self
                                .preview_digest
                                .as_ref()
                                .map(|(_, label)| label.clone())
                                .unwrap_or_default();
                            if self.preview_is_media(&bytes) {
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

fn is_viewable_media(value: &makepad_flow::ValueBytes) -> bool {
    matches!(
        values::media_kind(value),
        makepad_media_view::MediaKind::Image
            | makepad_media_view::MediaKind::Video
            | makepad_media_view::MediaKind::Audio
            | makepad_media_view::MediaKind::Mesh
            | makepad_media_view::MediaKind::Splat
    )
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
    /// A live edit that needs a restart waits while the open flow has unsaved or pending
    /// changes: the user saves or discards first.
    fn can_restart(&mut self, _cx: &mut Cx) -> Result<(), String> {
        if self.unsaved || self.source_save_pending || self.pending_source_edit.is_some() || self.source_layout_conflict {
            return Err("Flow has unsaved changes in the open flow; save or discard them to apply the edit".to_string());
        }
        if self.save_task.is_some() {
            return Err("Flow is saving".to_string());
        }
        Ok(())
    }

    /// The open flow, reopened after a restart.
    fn restart_state(&mut self, _cx: &mut Cx) -> Option<String> {
        self.selected.clone()
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &str) {
        if !state.is_empty() {
            self.restart_flow = Some(state.to_string());
        }
    }

    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // The extra style sheets join the catalogue before the first theme load.
        makepad_widgets_sheets::register();
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        makepad_app_aichat::script_mod(vm);
        makepad_flowgraph::script_mod(vm);
        makepad_look::script_mod(vm);
        theme::script_mod(vm);
        makepad_flow_studio::register(vm);
        faces::register_host_widgets(vm);
        panels::script_mod(vm);
        queue::script_mod(vm);
        viewer::script_mod(vm);
        surface::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.unsaved != self.unsaved_published {
            self.unsaved_published = self.unsaved;
            let what = self.selected.clone().unwrap_or_else(|| "the open flow".to_string());
            makepad_widgets::makepad_platform::restart::set_unsaved("flow", self.unsaved.then_some(what.as_str()));
        }
        if matches!(event, Event::LiveEdit | Event::ScriptReapply) {
            appearance::sync(cx);
            self.update_connection(cx);
            if let Some(faces) = self.faces.as_mut() {
                faces.apply_appearance(cx);
            }
            let restyled = self.with_surface(cx, |cx, surface| surface.apply_appearance(cx))
                .unwrap_or(false);
            if restyled {
                self.refresh_surface(cx, false);
            }
            self.ui.redraw(cx);
        }
        if let Event::Startup = event {
            self.startup(cx);
        }
        if let Event::NextFrame(nf) = event {
            self.time = nf.time;
            if self.source_mode {
                let bounds = self.ui.view(cx, ids!(workspace)).area().rect(cx);
                self.apply_source_geometry(cx, bounds);
            }
        }
        if self.studio_timer.is_event(event).is_some() || matches!(event, Event::Signal) {
            self.pump_studio(cx);
        }
        // The studio page: the menu bar and the page's own widgets; the
        // workbench's canvas, faces and shortcuts are not its.
        if self.studio_shown {
            if self.route_menu_input(cx, event) {
                return;
            }
            self.services.handle_event(cx, event);
            self.match_event(cx, event);
            self.ui.handle_event(cx, event, &mut Scope::empty());
        } else {
        // The full-window viewer is modal. Route user input directly to it
        // before selection, faces, the ordinary widget tree, and shortcuts.
        if self.route_viewer_modal_input(cx, event) {
            return;
        }
        if self.nested_capture.is_none() && self.route_menu_input(cx, event) {
            return;
        }
        if self.nested_capture.is_none() && self.handle_source_pointer(cx, event) {
            return;
        }
        if is_queue_shortcut(event) {
            self.queue_run(cx);
            // Do not pass Enter to a focused field; its text remains exactly
            // the settings that were submitted with this batch.
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
        // Resolve one graph level for this event without changing App.path.
        let root_canvas = self.ui.widget(cx, ids!(canvas));
        let root_popup = self.faces.as_ref().is_some_and(FaceHost::has_open_popup);
        let mut nested_target = self.nested_capture.clone();
        if nested_target.is_none() && !root_popup {
            if let Some(hierarchy) = self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref()) {
                nested_target = if let Some(point) = hierarchy::pointer(event) {
                    if self.point_over_canvas_chrome(cx, point) || self.point_over_app_pane(cx, point) { None }
                    else { root_canvas.borrow::<FlowCanvas>().and_then(|canvas| hierarchy.target(&canvas, point)) }
                } else if is_focus_routed_input(event) { hierarchy.focused_path(cx) } else { None };
            }
        }
        let press = matches!(event, Event::MouseDown(_))
            || matches!(event, Event::TouchUpdate(e) if e.touches.iter().any(|touch| touch.state == makepad_widgets::makepad_platform::event::TouchState::Start));
        let release = matches!(event, Event::MouseUp(_) | Event::WindowLostFocus(_))
            || matches!(event, Event::TouchUpdate(e) if e.touches.iter().all(|touch| touch.state == makepad_widgets::makepad_platform::event::TouchState::Stop));
        let nested_background_pan = press && nested_target.as_ref().is_some_and(|path| {
            self.faces.as_ref().and_then(|faces| faces.hierarchy.as_ref())
                .and_then(|hierarchy| hierarchy.canvas(path)).is_some_and(|widget| {
                    widget.borrow::<FlowCanvas>().is_some_and(|canvas| {
                        hierarchy::pointer(event).is_some_and(|point| matches!(canvas.pick(point), Some(CanvasPick::Background)))
                    })
                })
        });
        let root_navigation = nested_background_pan
            || matches!(event, Event::MouseDown(e) if nested_target.is_some() && e.button != MouseButton::PRIMARY);
        if press && !root_navigation {
            self.nested_capture = nested_target.clone();
            if nested_target.is_some() {
                if let Some(mut canvas) = root_canvas.borrow_mut::<FlowCanvas>() {
                    let camera = canvas.camera();
                    canvas.set_camera(cx, camera);
                }
            }
        }
        let canvas_policy = if nested_target.is_some() {
            CanvasEventPolicy {
                pointer: false, keyboard: false,
                navigation: root_navigation || (matches!(event, Event::Scroll(_)) && self.nested_capture.is_none()),
            }
        } else { CanvasEventPolicy::ALL };
        if !matches!(event, Event::Draw(_)) {
            if let Some(hierarchy) = self.faces.as_mut().and_then(|faces| faces.hierarchy.as_mut()) {
                hierarchy.handle_event(cx, event, if root_navigation { None } else { nested_target.as_deref() });
            }
        }
        // Selection belongs to the card, even when the press is about to be
        // handled by an interactive child in its mounted face. Only a press
        // in the graph pane selects: the app pane and the chrome are not the
        // canvas, whatever the camera would map their points onto.
        if let Event::MouseDown(e) = event {
            let in_canvas = self.ui.widget(cx, ids!(canvas)).area().rect(cx).contains(e.abs);
            if nested_target.is_none() && in_canvas && !self.point_over_canvas_chrome(cx, e.abs) && !self.point_over_app_pane(cx, e.abs) {
                let changed_context = self.nested_selection.take().is_some();
                if let Some(mut canvas) =
                    self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                {
                    canvas.select_at(cx, e.abs);
                    self.selected_node = canvas.selection().and_then(Selection::node).map(str::to_string);
                }
                if changed_context { self.refresh_inspector(cx); }
            }
        }
        // The faces first: a click inside a face is the face's, and the
        // canvas then sees it as handled. Their positions go through the
        // canvas camera.
        if !matches!(event, Event::Draw(_)) {
            let (camera, resize_press) = self
                .ui
                .widget(cx, ids!(canvas))
                .borrow::<FlowCanvas>()
                .map(|canvas| {
                    (
                        canvas.camera(),
                        matches!(event, Event::MouseDown(e) if canvas.is_resize_handle_at(e.abs)),
                    )
                })
                .map(|(camera, resize)| (Some(camera), resize))
                .unwrap_or((None, false));
            let mapped = true;
            // Graph faces belong to the graph pane: a pointer over the app
            // pane or the chrome is not theirs, nor is a key the surface
            // holds the focus for.
            let over_chrome = match event {
                Event::MouseDown(e) => self.point_over_canvas_chrome(cx, e.abs) || self.point_over_app_pane(cx, e.abs),
                Event::MouseMove(e) => self.point_over_canvas_chrome(cx, e.abs) || self.point_over_app_pane(cx, e.abs),
                Event::MouseUp(e) => self.point_over_canvas_chrome(cx, e.abs) || self.point_over_app_pane(cx, e.abs),
                Event::Scroll(e) => self.point_over_canvas_chrome(cx, e.abs) || self.point_over_app_pane(cx, e.abs),
                Event::LongPress(e) => self.point_over_canvas_chrome(cx, e.abs) || self.point_over_app_pane(cx, e.abs),
                _ => false,
            };
            let surface_keys = is_focus_routed_input(event)
                && self.with_surface(cx, |cx, view| view.owns_key_focus(cx)).unwrap_or(false);
            if (!over_chrome || root_popup) && !resize_press && !surface_keys
                && (nested_target.is_none() || !hierarchy::is_input(event)) {
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
        // Focused key input goes to exactly one owner: the app surface
        // when it holds the focus (delivered directly, the tree skipped), a
        // graph face when one does (delivered above), else the widget tree.
        let focus_routed = is_focus_routed_input(event);
        let surface_owns_key_focus =
            focus_routed && self.with_surface(cx, |cx, view| view.owns_key_focus(cx)).unwrap_or(false);
        let face_owns_key_focus =
            focus_routed && self.faces.as_ref().is_some_and(|faces| faces.owns_key_focus(cx));
        if surface_owns_key_focus {
            self.with_surface(cx, |cx, view| view.handle_event(cx, event, &mut Scope::empty()));
        } else if !face_owns_key_focus {
            match self.faces.as_mut() {
                Some(faces) => {
                    let mut faces = NodeFacesScope::new(faces)
                        .with_canvas_policy(root_canvas.widget_uid(), canvas_policy);
                    let mut scope = Scope::with_data(&mut faces);
                    self.ui.handle_event(cx, event, &mut scope);
                }
                None => self.ui.handle_event(cx, event, &mut Scope::empty()),
            }
        }
        if release { self.nested_capture = None; }
        }
        let text_editing = cx.text_input_has_focus();
        self.ui
            .menu_bar(cx, ids!(menu_bar))
            .handle_shortcut(cx, event, text_editing);
        if let Event::AudioDevices(devices) = event {
            self.live_runtime.audio_devices(cx, devices);
        }
        if self.live_timer.is_event(event).is_some() {
            self.pump_live(cx);
        }
        if self.live_timer.is_event(event).is_some() || self.frame_next.is_event(event).is_some() {
            self.tick_frames(cx);
        }
        if self.poll_timer.is_event(event).is_some() || matches!(event, Event::Signal) {
            self.sync_live(cx);
            self.drain_service(cx);
            self.control_lane.pump();
            self.data_lane.pump();
            self.persist_lane.pump();
            self.drain_package_task(cx);
            self.drain_io(cx);
            self.retry_deferred_work(cx);
            self.refresh_visible_asset_previews(cx);
            self.drain_values(cx);
            self.update_connection(cx);
            self.poll_subscription(cx);
            self.flush_pending(cx);
            self.continue_package_reload(cx);
            if self.source_mode {
                self.highlight_caret_node(cx);
            }
            if self.run_is_active() {
                self.update_run_bar(cx);
            }
            if let Some(mut queue) = self.ui.widget(cx, ids!(running)).borrow_mut::<QueueList>() {
                queue.set_now(cx, wall_clock_ms());
            }
            let mut mounted_faces = false;
            if let Some(hierarchy) = self.faces.as_mut().and_then(|faces| faces.hierarchy.as_mut()) {
                mounted_faces = hierarchy.prepare_one(cx);
                hierarchy.sync(cx, &self.outputs, &self.statuses, &mut self.values);
            }
            if mounted_faces { self.bind_graph_frames(cx); }
            self.request_wanted_values(cx);
            self.refresh_models(false);
            let refresh_assets = self
                .ui
                .widget(cx, ids!(inspector))
                .borrow_mut::<Inspector>()
                .is_some_and(|mut inspector| inspector.refresh_assets_due(self.time));
            if refresh_assets {
                self.refresh_assets(cx, false, true, true);
            }
        }
        self.refresh_ai_context();
        if let Event::Shutdown = event {
            self.shutdown(cx);
        }
    }
}

fn assets_response_is_current(
    request_generation: u64,
    current_generation: u64,
    request_query: &str,
    request_namespace: Option<&str>,
    current_query: &str,
    current_namespace: Option<&str>,
) -> bool {
    request_generation == current_generation
        && request_query == current_query
        && request_namespace == current_namespace
}

fn inspector_action_edits(action: &InspectorAction) -> bool {
    matches!(
        action,
        InspectorAction::SetParam { .. }
            | InspectorAction::SetFnSrc { .. }
            | InspectorAction::SetFaceSrc { .. }
            | InspectorAction::FlipCard { .. }
            | InspectorAction::RenameNode { .. }
            | InspectorAction::SetNodeDoc { .. }
            | InspectorAction::SetNodeMeta { .. }
            | InspectorAction::Disconnect { .. }
            | InspectorAction::MoveEntry { .. }
    )
}

fn face_text_literal(ty: PortType, text: String) -> Literal {
    if matches!(ty, PortType::Json | PortType::List) {
        if let Ok(value) = makepad_strict_json::parse_depth(text.as_bytes(), 32) {
            return strict_json_literal(value);
        }
    }
    Literal::Str(text)
}

fn strict_json_literal(value: makepad_strict_json::Value) -> Literal {
    use makepad_strict_json::Value as Json;
    match value {
        Json::Null => Literal::Null,
        Json::Bool(value) => Literal::Bool(value),
        Json::Int(value) => Literal::Num(value as f64),
        Json::F64(value) => Literal::Num(value),
        Json::Str(value) => Literal::Str(value),
        Json::Arr(values) => Literal::Arr(values.into_iter().map(strict_json_literal).collect()),
        Json::Obj(values) => Literal::Obj(
            values
                .into_iter()
                .map(|(name, value)| (name, strict_json_literal(value)))
                .collect(),
        ),
    }
}

fn run_label_ordinal(label: &str) -> Option<u64> {
    label
        .strip_prefix("run #")?
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Read and validate a package the way the windowless host does: a
/// directory package, no pinned component modules (that codec slice is
/// next), and every `required` role refused by name, because this window
/// binds no capability roles yet either.
fn open_package_dir(dir: &std::path::Path) -> Result<Package, String> {
    let package = Package::open(dir).map_err(|error| error.to_string())?;
    let unmet: Vec<String> = package
        .manifest
        .roles
        .iter()
        .filter(|role| role.required)
        .map(|role| format!("`{}` ({})", role.role, role.kind))
        .collect();
    if !unmet.is_empty() {
        return Err(format!(
            "package `{}` requires capability roles this window cannot bind: {}",
            package.manifest.id.0,
            unmet.join(", ")
        ));
    }
    Ok(package)
}

/// An input carried from the open package's instance to its reload: the
/// literal itself when the preview is the whole value, else the stored digest.
fn carried_input(value: &ValueRef) -> makepad_flow::InputValueDto {
    let mut input = makepad_flow::InputValueDto {
        ty: value.ty.clone(),
        text: None,
        json: None,
        digest: Some(value.digest.clone()),
        values: None,
    };
    if let Some(Literal::Str(text)) = &value.preview {
        if text.len() == value.bytes {
            match &value.ty {
                PortType::Text => {
                    input.text = Some(text.clone());
                    input.digest = None;
                }
                PortType::Json | PortType::List => {
                    if let Ok(json) = MicroJsonValue::deserialize_json(text) {
                        input.json = Some(json);
                        input.digest = None;
                    }
                }
                _ => {}
            }
        }
    }
    input
}

fn fresh_run_instance_request(label: String) -> CreateInstanceRequest {
    CreateInstanceRequest {
        label: Some(label),
        ..CreateInstanceRequest::default()
    }
}

fn is_queue_shortcut(event: &Event) -> bool {
    matches!(
        event,
        Event::KeyDown(key)
            if !key.is_repeat
                && matches!(key.key_code, KeyCode::ReturnKey | KeyCode::NumpadEnter)
                && key.modifiers.control
                && !key.modifiers.shift
                && !key.modifiers.alt
                && !key.modifiers.logo
    )
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
                .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
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

fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A stage's own fraction is detail for the label, never bar progress.
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

/// Longest breadcrumb the toolbar shows, in characters (the label's own
/// width bound ellipsizes the pixels on top of that).
const BREADCRUMB_MAX_CHARS: usize = 48;
/// Longest a shown level id keeps when the breadcrumb has to give.
const BREADCRUMB_LEVEL_CHARS: usize = 14;

/// `text`, or its first `room` characters and `…` when longer.
fn cut_to(text: &str, room: usize) -> String {
    if text.chars().count() <= room {
        return text.to_string();
    }
    let kept: String = text.chars().take(room.saturating_sub(1).max(1)).collect();
    format!("{}…", kept.trim_end())
}

/// `title › a › shout`, never over `BREADCRUMB_MAX_CHARS`: a deep path
/// folds its middle levels into one `…` (the root and the level shown
/// always stay), then the title gives way to the levels, then the levels
/// themselves are cut (the level shown last, so it stays recognisable).
fn breadcrumb_text(title: &str, path: &[String]) -> String {
    let join = |title: &str, levels: &[String]| {
        let mut text = title.to_string();
        for level in levels {
            text.push_str(" › ");
            text.push_str(level);
        }
        text
    };
    let count = |text: &str| text.chars().count();
    let full = join(title, path);
    if count(&full) <= BREADCRUMB_MAX_CHARS {
        return full;
    }
    let mut levels: Vec<String> = match path {
        [first, .., last] if path.len() > 2 => vec![first.clone(), "…".to_string(), last.clone()],
        _ => path.to_vec(),
    };
    let mut title = title.to_string();
    let room = BREADCRUMB_MAX_CHARS.saturating_sub(count(&join("", &levels)));
    title = cut_to(&title, room.max(1));
    if count(&join(&title, &levels)) > BREADCRUMB_MAX_CHARS {
        for level in levels.iter_mut().filter(|level| *level != "…") {
            *level = cut_to(level, BREADCRUMB_LEVEL_CHARS);
        }
    }
    if count(&join(&title, &levels)) > BREADCRUMB_MAX_CHARS {
        if let Some((last, before)) = levels.split_last_mut() {
            let used = count(&join(&title, before)) + count(" › ");
            *last = cut_to(last, BREADCRUMB_MAX_CHARS.saturating_sub(used).max(2));
        }
    }
    join(&title, &levels)
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
    use makepad_widgets::makepad_platform::makepad_micro_serde::DeJson;
    use makepad_widgets::makepad_platform::event::{
        DigitId, ScrollEvent, ScrollPhase, TouchPoint, TouchState, TouchUpdateEvent,
    };
    use makepad_widgets::widget::WidgetActionGroup;
    use std::cell::Cell;

    #[test]
    fn stale_failed_instance_delete_is_idempotent_but_other_errors_are_not_hidden() {
        assert!(is_absent_instance_error(&ClientError::Http {
            status: 404,
            body: r#"{"error":"instance not found"}"#.into(),
        }));
        assert!(!is_absent_instance_error(&ClientError::Http {
            status: 404,
            body: r#"{"error":"run not found"}"#.into(),
        }));
        assert!(!is_absent_instance_error(&ClientError::Http {
            status: 500,
            body: "instance not found".into(),
        }));
    }

    #[test]
    fn new_menu_groups_a_fake_templates_response_in_wire_group_order() {
        let json = r#"[
            {"name":"mesh","group":"3D","label":"Mesh","brief":"mesh","node_count":2,"inputs":[],"outputs":[]},
            {"name":"video","group":"Video","label":"Video","brief":"video","node_count":2,"inputs":[],"outputs":[]},
            {"name":"image-b","group":"Image","label":"Image B","brief":"image","node_count":2,"inputs":[],"outputs":[]},
            {"name":"image-a","group":"Image","label":"Image A","brief":"image","node_count":2,"inputs":[],"outputs":[]}
        ]"#;
        let rows = Vec::<TemplateSummary>::deserialize_json(json).unwrap();
        let grouped = grouped_new_menu_templates(rows);
        assert_eq!(
            grouped.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            ["image-a", "image-b", "video", "mesh"]
        );
        assert_eq!(
            grouped.iter().map(|row| row.label.as_str()).collect::<Vec<_>>(),
            ["IMAGE · Image A", "IMAGE · Image B", "VIDEO · Video", "3D · Mesh"]
        );
    }

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
        overlay: &Overlay,
        size: DVec2,
    ) {
        let event = DrawEvent {
            redraw_all: true,
            ..Default::default()
        };
        let canvas = app.ui.widget(cx, ids!(canvas));
        let viewer = app.ui.widget(cx, ids!(image_viewer));
        let menu = app.ui.widget(cx, ids!(menu_bar));
        let mut cx_draw = CxDraw::new(cx, &event);
        let cx = &mut Cx2d::new(&mut cx_draw);
        cx.begin_pass(pass, Some(1.0));
        draw_list.begin_always(cx);
        overlay.begin(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        if let Some(faces) = app.faces.as_mut() {
            let mut faces = NodeFacesScope::new(faces);
            canvas.draw_walk_all(cx, &mut Scope::with_data(&mut faces), Walk::fill());
        } else {
            canvas.draw_walk_all(cx, &mut Scope::empty(), Walk::fill());
        }
        menu.draw_walk_all(cx, &mut Scope::empty(), Walk {
            abs_pos: Some(dvec2(0.0, 32.0)),
            width: Size::fill(),
            height: Size::Fixed(28.0),
            ..Walk::default()
        });
        viewer.draw_walk_all(cx, &mut Scope::empty(), Walk::fill());
        cx.end_pass_sized_turtle();
        overlay.end(cx);
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

    // CPU draw lists only: no event loop, native window, renderer, or server.
    struct MenuRoutingTest {
        cx: Cx,
        app: App,
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
        input: WidgetRef,
        window: WindowId,
        menu_actions: Vec<makepad_widgets::menu_bar::MenuBarAction>,
        content_actions: usize,
    }

    impl MenuRoutingTest {
        fn new() -> Self {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.init_cx_os();
            let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
            app.left_hidden = true;
            let template = makepad_flow::templates::template(FIRST_TEMPLATE).unwrap();
            app.templates = vec![makepad_flow::templates::template_summary(template)];
            app.refresh_templates_menu(&mut cx);
            // Keep the real Templates entries, with a deterministic first title.
            let menu = app.ui.menu_bar(&cx, ids!(menu_bar));
            let templates = menu.borrow().unwrap().menus()[1].clone();
            menu.set_menus(&mut cx, vec![templates]);

            let source = "use mod.flow.*\nlet text = Text{value: \"unchanged\"}\nFlow{text}\n";
            let mut graph = makepad_flow::graph::evaluate(source, "<menu-routing>").unwrap();
            graph.nodes[0].at = Some((100.0, 100.0));
            let catalog = makepad_flow::graph::prelude_catalog().unwrap();
            let faces = FaceHost::mount_with(
                &mut cx, app.ui.widget_uid(), "test", "<menu-routing>", source, &graph, &catalog, &Closure::default(), None,
            );
            assert!(faces.error.is_none(), "{:?}", faces.error);
            let input = faces.faces["text"].binds.iter()
                .find(|bind| bind.widget.borrow::<TextInput>().is_some())
                .unwrap().widget.clone();
            app.with_canvas(&mut cx, |cx, canvas| canvas.set_graph(cx, Some(graph_view::view_of(&graph))));
            app.faces = Some(faces);
            let window = app.ui.window(&cx, ids!(main_window)).window_id().unwrap();
            let pass = DrawPass::new(&mut cx);
            pass.set_size(&mut cx, dvec2(1000.0, 700.0));
            let draw_list = DrawList2d::new(&mut cx);
            let overlay = Overlay { draw_list: DrawList::new(&mut cx) };
            let mut test = Self {
                cx, app, pass, draw_list, overlay, input, window,
                menu_actions: Vec::new(), content_actions: 0,
            };
            test.draw();
            test.draw();
            // Pan the actual canvas so the mounted editor covers both the
            // title and the dropdown entry. This is the reported overlap.
            let face = test.input_rect();
            let delta = dvec2(10.0, 25.0) - face.pos;
            let start = dvec2(900.0, 600.0);
            let down = test.mouse_down(start, MouseButton::MIDDLE);
            test.send(&Event::MouseMove(MouseMoveEvent {
                abs: start + delta, window_id: test.window,
                modifiers: KeyModifiers::default(), time: 1.1,
                handled: Cell::new(Area::Empty), lock_delta: Default::default(),
            }));
            test.mouse_up(start + delta, MouseButton::MIDDLE, &down);
            test.draw();
            assert!(test.camera().pan.length() > 1.0);
            assert!(test.input_rect().contains(test.title()));
            assert!(test.input_rect().contains(test.entry()));
            test.app.with_canvas(&mut test.cx, |cx, canvas| canvas.select(cx, None));
            test.cx.new_actions.clear();
            test.menu_actions.clear();
            test.content_actions = 0;
            test
        }

        fn draw(&mut self) {
            draw_canvas_and_viewer(
                &mut self.cx, &mut self.app, &self.pass, &mut self.draw_list,
                &self.overlay, dvec2(1000.0, 700.0),
            );
        }

        fn camera(&self) -> makepad_flowgraph::Camera {
            self.app.ui.widget(&self.cx, ids!(canvas)).borrow::<FlowCanvas>().unwrap().camera()
        }

        fn input_rect(&self) -> Rect {
            let rect = self.input.area().clipped_rect(&self.cx);
            let camera = self.camera();
            Rect { pos: camera.local_to_screen(rect.pos), size: rect.size * camera.scale }
        }

        fn title(&self) -> DVec2 {
            let rect = self.app.ui.menu_bar(&self.cx, ids!(menu_bar)).area().rect(&self.cx);
            rect.pos + dvec2(20.0, rect.size.y * 0.5)
        }

        fn entry(&self) -> DVec2 {
            // Default menu geometry: 4 px padding, one 22 px group heading,
            // then the real template row. The widget's own hit test decides.
            let rect = self.app.ui.menu_bar(&self.cx, ids!(menu_bar)).area().rect(&self.cx);
            rect.pos + dvec2(20.0, rect.size.y + 4.0 + 22.0 + 11.0)
        }

        fn open(&self) -> bool {
            self.app.ui.menu_bar(&self.cx, ids!(menu_bar)).is_open()
        }

        fn send(&mut self, event: &Event) {
            self.app.handle_event(&mut self.cx, event);
            while !self.cx.new_actions.is_empty() {
                let actions = std::mem::take(&mut self.cx.new_actions);
                let menu_uid = self.app.ui.menu_bar(&self.cx, ids!(menu_bar)).widget_uid();
                let canvas_uid = self.app.ui.widget(&self.cx, ids!(canvas)).widget_uid();
                for action in actions.iter().filter_map(|action| action.as_widget_action()) {
                    if action.widget_uid == menu_uid {
                        self.menu_actions.push(action.cast());
                    }
                    if action.widget_uid == canvas_uid
                        || action.widget_uid == self.input.widget_uid()
                            && !matches!(action.cast::<TextInputAction>(),
                                TextInputAction::KeyFocus | TextInputAction::KeyFocusLost)
                    {
                        self.content_actions += 1;
                    }
                }
                self.app.handle_event(&mut self.cx, &Event::Actions(actions));
            }
            // Advance the platform's pending focus after action delivery,
            // just as call_event_handler does, and deliver its transition.
            let prev = self.cx.key_focus();
            self.cx.action(());
            self.cx.handle_actions();
            let focus = self.cx.key_focus();
            if prev != focus {
                self.send(&Event::KeyFocus(KeyFocusEvent { prev, focus }));
            }
        }

        fn mouse_down(&mut self, abs: DVec2, button: MouseButton) -> Event {
            self.cx.fingers.first_mouse_button = Some((button, self.window));
            let down = Event::MouseDown(MouseDownEvent {
                abs, button, window_id: self.window, modifiers: KeyModifiers::default(),
                time: 1.0, handled: Cell::new(Area::Empty),
            });
            self.send(&down);
            down
        }

        fn mouse_up(&mut self, abs: DVec2, button: MouseButton, down: &Event) {
            self.send(&Event::MouseUp(MouseUpEvent {
                abs, button, window_id: self.window, modifiers: KeyModifiers::default(), time: 1.2,
            }));
            // Same capture cleanup as the platform after an up event.
            down.unhandle(&mut self.cx, &down.pointer_claimed_area());
            self.cx.fingers.first_mouse_button = None;
        }

        fn click(&mut self, abs: DVec2) {
            let down = self.mouse_down(abs, MouseButton::PRIMARY);
            self.mouse_up(abs, MouseButton::PRIMARY, &down);
        }

        fn touch(&mut self, abs: DVec2) {
            let mut event = TouchUpdateEvent {
                time: 2.0, window_id: self.window, modifiers: KeyModifiers::default(),
                touches: vec![TouchPoint {
                    state: TouchState::Start, abs, time: 2.0, uid: 1,
                    rotation_angle: 0.0, force: 1.0, radius: dvec2(1.0, 1.0),
                    handled: Cell::new(Area::Empty), sweep_lock: Cell::new(Area::Empty),
                }],
            };
            let down = Event::TouchUpdate(event.clone());
            self.send(&down);
            event.touches[0].state = TouchState::Stop;
            event.time = 2.1;
            self.send(&Event::TouchUpdate(event));
            down.unhandle(&mut self.cx, &down.pointer_claimed_area());
        }

        fn key(&mut self, key_code: KeyCode, control: bool) {
            self.send(&Event::KeyDown(KeyEvent {
                key_code, modifiers: KeyModifiers { control, ..Default::default() },
                ..Default::default()
            }));
        }

        fn assert_content_untouched(&self) {
            assert_eq!(self.content_actions, 0);
            assert!(self.app.ui.widget(&self.cx, ids!(canvas))
                .borrow::<FlowCanvas>().unwrap().selection().is_none());
            assert_eq!(self.input.as_text_input().text(), "unchanged");
            assert!(self.app.pending_inputs.is_empty());
            assert!(self.app.pending_params.is_empty());
            assert!(!self.app.run_start_in_flight);
        }
    }

    impl Drop for MenuRoutingTest {
        fn drop(&mut self) {
            if let Some(faces) = self.app.faces.take() {
                faces.free(&mut self.cx);
            }
        }
    }

    #[test]
    fn menu_routing_opens_and_toggles_over_a_panned_mounted_editor() {
        use makepad_widgets::menu_bar::MenuBarAction;
        let mut test = MenuRoutingTest::new();
        test.click(test.title());
        assert!(test.open());
        assert!(matches!(test.menu_actions.as_slice(), [MenuBarAction::Opened]));
        test.draw();
        test.click(test.title());
        assert!(!test.open());
        assert!(matches!(test.menu_actions.as_slice(), [MenuBarAction::Opened, MenuBarAction::Closed]));
        test.assert_content_untouched();
        assert!(test.app.io.template_requests.is_empty());
        let camera = test.camera();
        let down = test.mouse_down(test.title(), MouseButton::PRIMARY);
        test.draw();
        let end = test.entry();
        test.send(&Event::MouseMove(MouseMoveEvent {
            abs: end, window_id: test.window, modifiers: KeyModifiers::default(), time: 1.1,
            handled: Cell::new(Area::Empty), lock_delta: Default::default(),
        }));
        test.mouse_up(end, MouseButton::PRIMARY, &down);
        assert!(test.open());
        assert_eq!(test.camera().pan, camera.pan);
        test.assert_content_untouched();
        assert!(test.app.io.template_requests.is_empty());
    }

    #[test]
    fn menu_routing_selects_one_real_template_and_opens_the_created_flow() {
        use makepad_widgets::menu_bar::MenuBarAction;
        let mut test = MenuRoutingTest::new();
        test.click(test.title());
        test.draw();
        test.click(test.entry());
        assert!(!test.open());
        assert!(matches!(test.menu_actions.as_slice(), [
            MenuBarAction::Opened, MenuBarAction::Closed, MenuBarAction::Selected(id)
        ] if *id == template_menu_id(FIRST_TEMPLATE)));
        test.assert_content_untouched();
        assert_eq!(test.app.io.template_requests, [(FIRST_TEMPLATE.into(), FIRST_TEMPLATE.into())]);
        // Complete the request at the mailbox boundary without a server or
        // worker. The graph is evaluated from the actual bundled template.
        let template = makepad_flow::templates::template(FIRST_TEMPLATE).unwrap();
        let graph = makepad_flow::graph::evaluate(template.source, template.name).unwrap();
        test.app.io.start();
        test.app.io.sender.as_ref().unwrap().send(IoResult::Created {
            name: FIRST_TEMPLATE.into(), automatic: false, package_generation: test.app.package_generation,
            result: Ok(PutFlowResponse { revision: 1, graph }),
        }).unwrap();
        test.app.drain_io(&mut test.cx);
        assert_eq!(test.app.selected.as_deref(), Some(FIRST_TEMPLATE));
        assert_eq!(test.app.ui.label(&test.cx, ids!(flow_name)).text(), FIRST_TEMPLATE);
        assert_eq!(test.app.io.template_requests.len(), 1);
    }

    #[test]
    fn menu_routing_owns_scroll_keys_and_escape_then_restores_face_input() {
        let mut test = MenuRoutingTest::new();
        // Establish real editor focus before opening the menu.
        let outside = test.input_rect().pos + test.input_rect().size - dvec2(10.0, 2.0);
        test.click(outside);
        assert_eq!(test.cx.key_focus(), test.input.area());
        test.app.with_canvas(&mut test.cx, |cx, canvas| canvas.select(cx, None));
        test.cx.new_actions.clear();
        test.content_actions = 0;
        test.click(test.title());
        test.draw();
        test.click(test.entry() - dvec2(0.0, 22.0));
        assert!(test.open(), "the disabled group heading must retain modal ownership");
        assert!(test.app.io.template_requests.is_empty());
        let camera = test.camera();
        let zoom = test.app.ui.widget(&test.cx, ids!(canvas))
            .borrow::<FlowCanvas>().unwrap().zoom();
        test.send(&scroll_event(test.window, test.entry()));
        test.send(&scroll_event(test.window, dvec2(800.0, 500.0)));
        test.send(&Event::TextInput(TextInputEvent { input: "blocked".into(), ..Default::default() }));
        test.key(KeyCode::Backspace, false);
        assert_eq!(test.camera().pan, camera.pan);
        assert_eq!(test.camera().scale, camera.scale);
        assert_eq!(test.app.ui.widget(&test.cx, ids!(canvas))
            .borrow::<FlowCanvas>().unwrap().zoom(), zoom);
        test.key(KeyCode::Escape, false);
        assert!(!test.open());
        assert_eq!(test.cx.key_focus(), test.input.area());
        test.assert_content_untouched();
        test.send(&Event::TextInput(TextInputEvent { input: "works".into(), ..Default::default() }));
        assert!(test.input.as_text_input().text().contains("works"));
        assert!(test.content_actions > 0);
    }

    #[test]
    fn menu_routing_outside_dismissal_passes_through_and_touch_selection_is_modal() {
        let mut test = MenuRoutingTest::new();
        test.touch(test.title());
        assert!(test.open());
        test.draw();
        test.touch(test.title());
        assert!(!test.open());
        test.touch(test.title());
        test.draw();
        test.touch(test.entry());
        assert!(!test.open());
        test.assert_content_untouched();
        assert_eq!(test.app.io.template_requests.len(), 1);
        test.click(test.title());
        test.draw();
        let outside = test.input_rect().pos + test.input_rect().size - dvec2(10.0, 2.0);
        test.click(outside);
        assert!(!test.open());
        assert_eq!(test.cx.key_focus(), test.input.area());
        assert!(test.content_actions > 0);
        test.send(&Event::TextInput(TextInputEvent { input: "outside".into(), ..Default::default() }));
        assert!(test.input.as_text_input().text().contains("outside"));
        test.touch(test.title());
        test.draw();
        test.touch(outside);
        assert!(!test.open());
        assert_eq!(test.cx.key_focus(), test.input.area());
    }

    #[test]
    fn menu_routing_precedes_queue_shortcut_and_closes_on_focus_loss() {
        let mut test = MenuRoutingTest::new();
        test.click(test.title());
        test.key(KeyCode::ReturnKey, true);
        assert!(!test.open(), "Ctrl+Enter must reach the menu before queue_run");
        assert_eq!(test.app.io.template_requests.len(), 1);
        test.assert_content_untouched();
        for event in [Event::WindowLostFocus(test.window), Event::PopupDismissed(
            makepad_widgets::makepad_platform::event::PopupDismissedEvent {
                window_id: test.window,
                reason: makepad_widgets::makepad_platform::event::PopupDismissReason::FocusLost,
            },
        )] {
            test.click(test.title());
            assert!(test.open());
            test.send(&event);
            assert!(!test.open());
        }
        test.click(test.title());
        test.cx.set_key_focus(test.input.area());
        test.send(&Event::Signal);
        assert!(!test.open(), "the platform KeyFocus transition closes the menu");
    }

    #[test]
    fn splitter_panel_layout_mounts_headlessly() {
        use makepad_code_editor::code_view::CodeView;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        for path in [
            ids!(column_split),
            ids!(left_flow_split),
            ids!(left_running_split),
            ids!(canvas_right_split),
        ] {
            assert!(app.ui.widget(&cx, path).borrow::<Splitter>().is_some());
        }
        assert!(app.ui.widget(&cx, ids!(flow_list)).borrow::<FlowList>().is_some());
        assert!(app
            .ui
            .widget(&cx, ids!(running))
            .borrow::<QueueList>()
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
            .borrow::<CodeView>()
            .is_some());
        assert!(app
            .ui
            .widget(&cx, ids!(preview_value))
            .borrow::<faces::ValueView>()
            .is_some());
    }

    #[test]
    fn locked_inspector_keeps_asset_navigation_and_opening_but_blocks_edits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        let inspector_ref = app.ui.widget(&cx, ids!(inspector));
        {
            let mut inspector = inspector_ref.borrow_mut::<Inspector>().unwrap();
            inspector.set_assets_page(
                &mut cx,
                AssetsResponse {
                    assets: vec![makepad_flow::FlowAsset {
                        id: "generated".into(),
                        alias: None,
                        namespace: "flows".into(),
                        title: "Generated asset".into(),
                        kind: "texture".into(),
                        tags: vec!["flow".into()],
                        created_ms: 10,
                    }],
                    cursor: None,
                },
                false,
                false,
            );
            inspector.set_locked(&mut cx, true);
        }

        let assets_tab = app.ui.widget(&cx, ids!(inspector.tabs.assets_tab));
        let tab_actions: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(ButtonAction::Clicked(KeyModifiers::default())),
            widget_uid: assets_tab.widget_uid(),
            group: None,
        })];
        let tab_changes = inspector_ref
            .borrow_mut::<Inspector>()
            .unwrap()
            .changes(&mut cx, &tab_actions);
        assert!(inspector_ref.borrow::<Inspector>().unwrap().assets_visible());
        assert!(tab_changes
            .iter()
            .any(|action| matches!(action, InspectorAction::RefreshAssets)));

        let asset_list = app.ui.portal_list(&cx, ids!(inspector.assets_view.asset_list));
        let (asset_item, _) = asset_list.item_with_existed(&mut cx, 0, live_id!(Asset));
        let asset_actions: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(ViewAction::FingerUp(FingerUpEvent {
                window_id: WindowId(0, 0),
                abs: dvec2(0.0, 0.0),
                abs_start: dvec2(0.0, 0.0),
                capture_time: 0.0,
                time: 0.1,
                digit_id: DigitId::default(),
                device: DigitDevice::Mouse { button: MouseButton::PRIMARY },
                has_long_press_occurred: false,
                tap_count: 1,
                modifiers: KeyModifiers::default(),
                rect: Rect::default(),
                is_over: true,
                is_sweep: false,
                cancelled: false,
            })),
            widget_uid: asset_item.widget_uid(),
            group: Some(WidgetActionGroup {
                group_uid: asset_list.widget_uid(),
                item_uid: asset_item.widget_uid(),
            }),
        })];
        let opened = inspector_ref
            .borrow_mut::<Inspector>()
            .unwrap()
            .changes(&mut cx, &asset_actions);
        assert!(opened.iter().any(|action| matches!(
            action,
            InspectorAction::OpenAsset(asset) if asset.id == "generated"
        )));

        let source = "use mod.flow.*\nlet text = Text{value: \"x\"}\nFlow{text}\n";
        let graph = makepad_flow::graph::evaluate(source, "<locked-assets>").unwrap();
        let catalog = makepad_flow::graph::prelude_catalog().unwrap();
        inspector_ref
            .borrow_mut::<Inspector>()
            .unwrap()
            .show_node(&mut cx, Some(&graph), &catalog, Some("text"), Some("text"), &[], &[], Some(source));
        let inspector_list = app.ui.portal_list(&cx, ids!(inspector.inspector_view.list));
        let head = inspector_list.item(&mut cx, 0, live_id!(Head));
        let node_id = head.text_input(&mut cx, ids!(node_id));
        let edit_actions: ActionsBuf = vec![Box::new(WidgetAction {
            data: None,
            action: Box::new(TextInputAction::Returned(
                "renamed".into(),
                KeyModifiers::default(),
            )),
            widget_uid: node_id.widget_uid(),
            group: Some(WidgetActionGroup {
                group_uid: inspector_list.widget_uid(),
                item_uid: head.widget_uid(),
            }),
        })];
        assert!(inspector_ref
            .borrow_mut::<Inspector>()
            .unwrap()
            .changes(&mut cx, &edit_actions)
            .is_empty());
    }

    #[test]
    fn stale_asset_page_is_rejected_after_query_or_scope_changes() {
        assert!(assets_response_is_current(
            7,
            7,
            "sunset",
            Some("flows"),
            "sunset",
            Some("flows"),
        ));
        assert!(!assets_response_is_current(
            6,
            7,
            "sunset",
            Some("flows"),
            "sunset",
            Some("flows"),
        ));
        assert!(!assets_response_is_current(
            7,
            7,
            "sunset",
            Some("flows"),
            "portrait",
            Some("flows"),
        ));
        assert!(!assets_response_is_current(
            7,
            7,
            "sunset",
            Some("flows"),
            "sunset",
            None,
        ));
    }

    #[test]
    fn the_templates_menu_lists_every_template_and_opens_one() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        let template = |name: &str, group: &str, label: &str| TemplateSummary {
            name: name.into(),
            group: group.into(),
            label: label.into(),
            brief: String::new(),
            node_count: 3,
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        app.templates = vec![
            template("text-to-video", "Video", "Text to video"),
            template("prompt-to-image", "Image", "Prompt to image"),
        ];
        app.refresh_templates_menu(&mut cx);
        let labels: Vec<String> = app
            .ui
            .menu_bar(&cx, ids!(menu_bar))
            .borrow()
            .unwrap()
            .menus()
            .iter()
            .map(|menu| menu.label.clone())
            .collect();
        assert_eq!(labels[1], "Templates", "{labels:?}");
        assert_eq!(menu_label(&app, &cx, template_menu_id("text-to-video")), "  Text to video");
        // Group order (Image before Video), each under a greyed heading.
        let entries: Vec<(String, bool, bool)> = app
            .ui
            .menu_bar(&cx, ids!(menu_bar))
            .borrow()
            .unwrap()
            .menus()[1]
            .items
            .iter()
            .map(|entry| (entry.label.clone(), entry.enabled, entry.separator))
            .collect();
        assert_eq!(
            entries,
            vec![
                ("Image".to_string(), false, false),
                ("  Prompt to image".to_string(), true, false),
                (String::new(), true, true),
                ("Video".to_string(), false, false),
                ("  Text to video".to_string(), true, false),
            ]
        );
        // A refresh replaces the menu instead of stacking a second one.
        app.refresh_templates_menu(&mut cx);
        let count = app
            .ui
            .menu_bar(&cx, ids!(menu_bar))
            .borrow()
            .unwrap()
            .menus()
            .iter()
            .filter(|menu| menu.label == "Templates")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_cancelled_run_clears_the_canvas_and_the_bar() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        app.run = Some(RunInfo {
            run_id: "run-c".into(),
            state: "running".into(),
            started: 0.0,
            finished_secs: None,
            revision: 1,
            planned_nodes: vec!["image".into()],
        });
        app.set_node_status(&mut cx, "image", "running", 500, true, "denoise", None);
        app.handle_flow_event(
            &mut cx,
            FlowEvent {
                topic: "run".into(),
                kind: "run.finished".into(),
                run_id: Some("run-c".into()),
                state: Some(makepad_widgets::makepad_micro_serde::JsonValue::String(
                    "cancelled".into(),
                )),
                ..FlowEvent::default()
            },
        );
        assert!(app.run.is_none());
        assert!(app
            .ui
            .widget(&cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .unwrap()
            .statuses
            .is_empty());
    }

    #[test]
    fn control_enter_is_the_queue_shortcut() {
        let event = Event::KeyDown(KeyEvent {
            key_code: KeyCode::ReturnKey,
            modifiers: KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
            ..KeyEvent::default()
        });
        assert!(is_queue_shortcut(&event));

        let plain = Event::KeyDown(KeyEvent {
            key_code: KeyCode::ReturnKey,
            ..KeyEvent::default()
        });
        assert!(!is_queue_shortcut(&plain));
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
    fn design_face_commits_persist_graph_values_and_run_views_refuse_them() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));
        let source = "use mod.flow.*\nlet prompt = Input{value: \"from file\"}\nlet expand = Llm{prompt: \"literal\"}\nFlow{prompt expand}\n";
        let graph = makepad_flow::graph::evaluate(source, "design.splash").unwrap();
        app.definition = Some(FlowDefinition {
            source: source.to_string(),
            revision: 1,
            graph: Some(graph),
            tools: Default::default(),
            error: None,
            modules: Vec::new(),
            valid_source: None,
            canonical: true,
            ephemeral: false,
        });

        app.route_face_commits(
            vec![
                ("prompt".into(), "text".into(), "edited design".into()),
                ("expand".into(), "prompt".into(), "edited literal".into()),
            ],
            Vec::new(),
        );
        assert_eq!(
            app.pending_params.get(&("prompt".into(), "value".into())),
            Some(&Literal::Str("edited design".into()))
        );
        assert_eq!(
            app.pending_params.get(&("expand".into(), "prompt".into())),
            Some(&Literal::Str("edited literal".into()))
        );

        app.instance = Some("run-1".into());
        app.route_face_commits(
            vec![("prompt".into(), "text".into(), "must be ignored".into())],
            vec![("prompt".into(), "value".into(), Literal::Str("ignored".into()))],
        );
        assert_eq!(
            app.pending_params.get(&("prompt".into(), "value".into())),
            Some(&Literal::Str("edited design".into()))
        );
    }

    #[test]
    fn run_attachment_is_locked_and_design_detach_restores_source_editing() {
        use makepad_code_editor::code_view::CodeView;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));

        app.attach_instance(&mut cx, Some("run-1".into()), false);
        assert_eq!(app.instance.as_deref(), Some("run-1"));
        assert!(app.ui.widget(&cx, ids!(design_btn)).visible());
        // Source is a CodeView; its editor's read-only gate is applied by
        // the attachment transition. The locked-inspector regression covers
        // the editable controls that remain reachable through the same lock.
        assert!(app.ui.widget(&cx, ids!(source)).borrow::<CodeView>().is_some());

        app.attach_instance(&mut cx, None, false);
        assert!(app.instance.is_none());
        assert!(!app.ui.widget(&cx, ids!(design_btn)).visible());
        assert!(app.ui.widget(&cx, ids!(source)).borrow::<CodeView>().is_some());
    }

    #[test]
    fn every_play_gets_a_fresh_unoverridden_numbered_instance_request() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut app = cx.with_vm(|vm| App::from_script_mod(vm, <App as AppMain>::script_mod));

        for expected in 1..=6 {
            let label = app.next_run_label("demo");
            assert_eq!(label, format!("run #{expected}"));
            let request = fresh_run_instance_request(label.clone());
            assert_eq!(request.label.as_deref(), Some(label.as_str()));
            assert!(request.inputs.is_none());
            assert!(request.pin.is_none());
        }
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
        let overlay = Overlay { draw_list: DrawList::new(&mut cx) };
        draw_canvas_and_viewer(&mut cx, &mut app, &pass, &mut draw_list, &overlay, size);
        draw_canvas_and_viewer(&mut cx, &mut app, &pass, &mut draw_list, &overlay, size);
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
                batch: None,
                batch_index: None,
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
                            sequences: None,
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
                            sequences: None,
                            error: Some("boom".into()),
                            text: None,
                        },
                    ),
                ]),
                outputs: HashMap::new(),
                sequences: None,
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
            .find_map(|port| (port.name == port_name).then_some(port.ty.clone()))
    } else {
        node.inputs
            .iter()
            .find_map(|port| (port.port == port_name).then_some(port.ty.clone()))
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
        | PortType::Bytes
        | PortType::Registered(_) => {
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

#[cfg(test)]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}

#[cfg(test)]
makepad_script_test::shader_inventory_test!("flow", <App as AppMain>::script_mod);
