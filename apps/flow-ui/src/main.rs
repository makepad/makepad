// integration notes
// F3 supplies the canvas, faces, inspector, source/App views, and running list.
// F4 supplies `services` and the aichat port/event/worker wiring retained here.
// The client keeps typed instance/run/value methods plus `_json` render projections.

mod services;

use crate::services::{BridgeContext, FlowServices, FlowUiAction};
pub use makepad_widgets;

mod canvas;
mod faces;
mod graph_edit;
mod panels;
mod values;

use canvas::{CanvasEdit, FlowCanvas, FlowCanvasAction, NodeStatus};
use faces::{BridgeCall, FaceBridgeCall, FaceHost};
use makepad_flow::client::{
    ClientError, FlowClient, FlowSubscriber, FlowSubscriberConfig, SessionConfig,
    SessionConnector, SessionStatus, SubscriptionEvent,
};
use makepad_flow::embed::{default_root, resolve, EmbedPolicy, Resolved};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{
    CreateInstanceRequest, CreateInstanceResponse, CreateRunResponse, Event as FlowEvent,
    FlowDefinition, FlowSummary, Graph, InstanceRow, Literal, NodeTypeCatalog, NodesResponse,
    PortType, PutFlowResponse, ValueRef,
};
use makepad_widgets::makepad_draw::text::selection::Cursor;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;
use panels::{AppView, Inspector, InspectorAction, Palette, PaletteAction, RunningAction, RunningList};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use values::ValueCache;

app_main!(App);

const EXAMPLE_FLOW_SOURCE: &str = r#"use mod.flow.*

/** What to paint. */
let prompt = Input{ type: @text  default: "a lighthouse at dusk"  at: vec2(40, 120) }

let expand = Llm{
    system: "Rewrite the prompt as one vivid paragraph for an image model.
             Keep the subject. Add light, lens, material, mood. No lists."
    prompt: prompt.text()
    at: vec2(360, 120)
}

let styled = Fn{
    in: { text: expand.text()  style: "photo" }
    out: [@text]
    run: |i| { {text: i.text + ", " + i.style + " style"} }
    at: vec2(680, 120)
}

let image = Image{
    prompt: styled.text()
    width: 1024  height: 1024  steps: 8
    at: vec2(1000, 120)
    ui: ImageFace{
        style := DropDown{ labels: ["photo", "anime", "oil paint"]  bind := "styled.style" }
    }
}

/** The finished picture. */
let picture = Output{ type: @image  value: image.image() }

Flow{
    label: "Prompt to image"
    brief: "Expands a short prompt into a rich one and paints it."
    prompt, expand, styled, image, picture
}
"#;

const EMPTY_FLOW_SOURCE: &str = r#"use mod.flow.*

Flow{
    label: "New flow"
    brief: ""
}
"#;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.FlowListBase = #(FlowList::register_widget(vm))
    mod.widgets.FlowList = set_type_default() do mod.widgets.FlowListBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Item := View{
                width: Fill
                height: 30
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_1
                align: Align{y: 0.5}
                state_dot := Label{
                    width: 12
                    text: "●"
                }
                select := ButtonFlatter{
                    width: Fill
                    text: ""
                }
            }
        }
    }

    let Column = SolidView{
        width: 220
        height: Fill
        flow: Down
        padding: theme.mspace_2
        spacing: theme.space_2
        draw_bg +: {color: theme.color_bg_container}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Flow"
                window.inner_size: vec2(1500, 940)
                pass.clear_color: theme.color_bg_app
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down

                    SolidView{
                        width: Fill
                        height: 38
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_2
                        align: Align{y: 0.5}
                        draw_bg +: {color: theme.color_bg_container}
                        status_chip := Label{
                            width: 230
                            text: "Discovering"
                        }
                        flow_name := Label{
                            width: 260
                            text: "No flow selected"
                        }
                        run_btn := Button{text: "▶ Run"}
                        cancel_btn := Button{text: "Cancel"}
                        run_state := Label{
                            width: Fill
                            text: ""
                        }
                        undo_btn := ButtonFlat{text: "Undo"}
                        view_btn := ButtonFlat{text: "App view"}
                        side_btn := ButtonFlat{text: "Source"}
                        new_btn := Button{text: "New"}
                        example_btn := Button{text: "Example"}
                    }

                    View{
                        width: Fill
                        height: Fill
                        flow: Right

                        Column{
                            H3{text: "Flows"}
                            flow_list := mod.widgets.FlowList{height: 150}
                            H3{text: "Running"}
                            running := mod.widgets.RunningList{height: 170}
                            H3{text: "Palette"}
                            palette_note := Label{
                                width: Fill
                                height: Fit
                                text: "press a type, release on the canvas"
                                draw_text +: {color: theme.color_text_meta}
                            }
                            palette := mod.widgets.Palette{height: Fill}
                        }

                        View{
                            width: Fill
                            height: Fill
                            flow: Down
                            canvas_view := View{
                                width: Fill
                                height: Fill
                                canvas := mod.widgets.FlowCanvas{}
                            }
                            app_view_view := View{
                                width: Fill
                                height: Fill
                                visible: false
                                app_view := mod.widgets.AppView{}
                            }
                            error_label := Label{
                                width: Fill
                                height: Fit
                                text: ""
                                draw_text +: {color: theme.color_makepad}
                            }
                        }

                        SolidView{
                            width: 340
                            height: Fill
                            flow: Down
                            padding: theme.mspace_2
                            spacing: theme.space_2
                            draw_bg +: {color: theme.color_bg_container}
                            inspector_view := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                H3{text: "Inspector"}
                                inspector := mod.widgets.Inspector{}
                            }
                            source_view := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                visible: false
                                spacing: theme.space_2
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    align: Align{y: 0.5}
                                    H3{width: Fill text: "Source"}
                                    save_btn := Button{text: "Save"}
                                }
                                source := TextInput{
                                    width: Fill
                                    height: Fill
                                    is_multiline: true
                                    empty_text: "Select a flow to edit its source"
                                    draw_text +: {text_style: theme.font_code{}}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
struct FlowList {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<FlowSummary>,
}

impl FlowList {
    fn set_rows(&mut self, cx: &mut Cx, rows: Vec<FlowSummary>) {
        self.rows = rows;
        self.redraw(cx);
    }

    fn selected(&self, cx: &mut Cx, actions: &Actions) -> Option<usize> {
        let list = self.portal_list(cx, ids!(list));
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
                let title = if row.instances > 0 {
                    format!("{} · {}", row.name, row.instances)
                } else {
                    row.name.clone()
                };
                item.button(cx, ids!(select)).set_text(cx, &title);
                let mut dot = item.label(cx, ids!(state_dot));
                let color = if row.state == "ok" {
                    vec4(0.20, 0.78, 0.38, 1.0)
                } else {
                    vec4(0.90, 0.24, 0.24, 1.0)
                };
                script_apply_eval!(cx, dot, {draw_text +: {color: #(color)}});
                item.draw_all_unscoped(cx);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

enum IoResult {
    Flows(Result<Vec<FlowSummary>, ClientError>),
    Flow {
        name: String,
        result: Result<FlowDefinition, ClientError>,
    },
    Saved {
        name: String,
        source: String,
        result: Result<PutFlowResponse, ClientError>,
    },
    GraphPut {
        name: String,
        result: Result<PutFlowResponse, ClientError>,
    },
    Catalog(Result<NodesResponse, ClientError>),
    InstanceCreated {
        flow: String,
        result: Result<CreateInstanceResponse, ClientError>,
    },
    InstanceRunStarted {
        flow: String,
        result: Result<(String, CreateRunResponse), ClientError>,
    },
    Instances(Result<Vec<InstanceRow>, ClientError>),
    Instance(Result<InstanceRow, ClientError>),
    RunStarted(Result<CreateRunResponse, ClientError>),
    Done(Result<(), ClientError>),
}

#[derive(Default)]
struct IoMailbox {
    sender: Option<Sender<IoResult>>,
    receiver: Option<Receiver<IoResult>>,
    fetching_flows: bool,
    fetching_instances: bool,
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
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    host: Option<FlowServer>,
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
    catalog: Vec<NodeTypeCatalog>,
    #[rust]
    selected: Option<String>,
    #[rust]
    definition: Option<FlowDefinition>,
    #[rust]
    revisions: Vec<u64>,
    #[rust]
    instance: Option<String>,
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
    unsaved: bool,
    #[rust]
    connected_server: Option<[u8; 16]>,
    #[rust]
    embedded: bool,
    #[rust]
    warned_custom: HashSet<String>,
    #[rust]
    palette_drop: Option<((f64, f64), String, String, PortType)>,
    #[rust]
    pending_inputs: HashMap<(String, String), String>,
    #[rust]
    pending_params: HashMap<(String, String), Literal>,
    #[rust]
    pending_graph: bool,
    #[rust]
    app_mode: bool,
    #[rust]
    source_mode: bool,
    #[rust]
    preview_digest: Option<(String, String)>,
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
    }

    fn client(&self) -> Option<Arc<Mutex<FlowClient>>> {
        self.session.as_ref().and_then(|session| session.client())
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
        let text = match &status {
            SessionStatus::Discovering => "Discovering".to_string(),
            SessionStatus::Connecting { .. } => "Connecting…".to_string(),
            SessionStatus::Retrying { in_secs, .. } => format!("Retrying in {in_secs} s"),
            SessionStatus::Connected { .. } => format!(
                "Connected · {} · r{} · {} KB",
                if self.embedded { "embedded" } else { "attached" },
                self.definition
                    .as_ref()
                    .map(|definition| definition.revision)
                    .unwrap_or(0),
                self.values.bytes() / 1024
            ),
        };
        self.ui.label(cx, ids!(status_chip)).set_text(cx, &text);
        if let SessionStatus::Connected { server_id, .. } = status {
            if self.connected_server != Some(server_id) {
                self.connected_server = Some(server_id);
                if let Some(client) = session.client() {
                    self.services.connect(cx, client.clone());
                    self.subscriber =
                        FlowSubscriber::start(client, FlowSubscriberConfig::default()).ok();
                    self.refresh_flows();
                    self.io(|client| IoResult::Catalog(client.nodes_catalog()));
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

    fn fetch_instance(&mut self) {
        let Some(id) = self.instance.clone() else {
            return;
        };
        self.io(move |client| IoResult::Instance(client.instance(&id)));
    }

    fn load_flow(&mut self, name: String) {
        self.io(move |client| {
            let result = client.flow(&name);
            IoResult::Flow { name, result }
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

    fn open_flow(&mut self, cx: &mut Cx, name: String) {
        self.selected = Some(name.clone());
        self.unsaved = false;
        self.instance = None;
        self.instance_row = None;
        self.run = None;
        self.outputs.clear();
        self.revisions.clear();
        self.ui.label(cx, ids!(flow_name)).set_text(cx, &name);
        self.set_error(cx, "");
        self.load_flow(name.clone());
    }

    /// Every canvas / inspector / face edit ends here: the new graph goes to
    /// the server, the file is rewritten, and `flow.changed` redraws.
    fn put_graph(&mut self, cx: &mut Cx, graph: Graph) {
        let Some(name) = self.selected.clone() else {
            return;
        };
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
        if let Some(definition) = self.definition.as_mut() {
            // The canvas draws the edit at once; the server's re-evaluation
            // confirms or corrects it through flow.changed.
            definition.graph = Some(graph.clone());
        }
        self.show_graph(cx);
        self.pending_graph = true;
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
                            self.flows = flows.clone();
                            if let Some(mut list) =
                                self.ui.widget(cx, ids!(flow_list)).borrow_mut::<FlowList>()
                            {
                                list.set_rows(cx, flows);
                            }
                            self.update_connection(cx);
                        }
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::Flow { name, result } => {
                    if self.selected.as_deref() != Some(&name) {
                        continue;
                    }
                    match result {
                        Ok(definition) => self.show_flow(cx, definition),
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
                IoResult::GraphPut { name, result } => {
                    self.pending_graph = false;
                    match result {
                        Ok(response) => {
                            if self.selected.as_deref() == Some(&name) {
                                if let Some(definition) = self.definition.as_mut() {
                                    definition.revision = response.revision;
                                    definition.graph = Some(response.graph);
                                    definition.error = None;
                                }
                                self.push_revision(response.revision);
                                self.show_graph(cx);
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
                IoResult::InstanceCreated { flow, result } => {
                    if self.selected.as_deref() != Some(&flow) {
                        continue;
                    }
                    match result {
                        Ok(response) => {
                            self.instance = Some(response.instance);
                            self.remount_faces(cx);
                            self.fetch_instance();
                            self.refresh_instances();
                        }
                        Err(error) => {
                            self.set_error(cx, &format!("no instance: {error}"));
                        }
                    }
                }
                IoResult::InstanceRunStarted { flow, result } => {
                    if self.selected.as_deref() != Some(&flow) {
                        continue;
                    }
                    match result {
                        Ok((instance, response)) => {
                            self.instance = Some(instance);
                            self.run = Some(RunInfo {
                                run_id: response.run_id,
                                state: if response.queued == 0 {
                                    "running".into()
                                } else {
                                    "queued".into()
                                },
                                started: self.time,
                                finished_secs: None,
                            });
                            self.remount_faces(cx);
                            self.fetch_instance();
                            self.refresh_instances();
                            self.update_run_state(cx);
                        }
                        Err(error) => self.show_error(cx, &error),
                    }
                }
                IoResult::Instances(result) => {
                    self.io.fetching_instances = false;
                    match result {
                        Ok(rows) => {
                            self.instances = rows.clone();
                            if let Some(mut list) =
                                self.ui.widget(cx, ids!(running)).borrow_mut::<RunningList>()
                            {
                                list.set_rows(cx, rows, self.instance.clone());
                            }
                        }
                        Err(_) => {}
                    }
                }
                IoResult::Instance(result) => match result {
                    Ok(row) => {
                        if self.instance.as_deref() == Some(row.instance.as_str()) {
                            if self.selected.as_deref() != Some(row.flow.as_str()) {
                                self.selected = Some(row.flow.clone());
                                self.unsaved = false;
                                self.revisions.clear();
                                self.ui
                                    .label(cx, ids!(flow_name))
                                    .set_text(cx, &row.flow);
                                self.load_flow(row.flow.clone());
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
                            self.update_run_state(cx);
                        }
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::RunStarted(result) => match result {
                    Ok(response) => {
                        self.run = Some(RunInfo {
                            run_id: response.run_id,
                            state: if response.queued == 0 {
                                "running".into()
                            } else {
                                "queued".into()
                            },
                            started: self.time,
                            finished_secs: None,
                        });
                        self.update_run_state(cx);
                    }
                    Err(error) => self.show_error(cx, &error),
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

    fn show_flow(&mut self, cx: &mut Cx, definition: FlowDefinition) {
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
        let source_changed = self
            .definition
            .as_ref()
            .map(|old| old.source != definition.source)
            .unwrap_or(true);
        self.definition = Some(definition);
        self.show_graph(cx);
        if source_changed || self.faces.is_none() {
            self.remount_faces(cx);
        }
        self.update_connection(cx);
    }

    /// Push the current graph into the canvas, the app view and the inspector.
    fn show_graph(&mut self, cx: &mut Cx) {
        let graph = self.current_graph();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_graph(cx, graph.clone());
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
        let selected = self
            .ui
            .widget(cx, ids!(canvas))
            .borrow::<FlowCanvas>()
            .and_then(|canvas| canvas.selected().map(str::to_string));
        let outputs = selected
            .as_ref()
            .and_then(|node| self.outputs.get(node).cloned())
            .unwrap_or_default();
        if let Some(mut inspector) = self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>() {
            inspector.show_node(
                cx,
                self.current_graph().as_ref(),
                &self.catalog,
                selected.as_deref(),
                &outputs,
            );
        }
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
        if let Some(error) = faces.error.as_ref() {
            self.set_error(cx, &format!("faces: {error}"));
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

    fn update_run_state(&mut self, cx: &mut Cx) {
        let text = match (&self.run, &self.instance_row) {
            (Some(run), _) => {
                let secs = run
                    .finished_secs
                    .unwrap_or_else(|| (self.time - run.started).max(0.0));
                format!("{} · {:.1} s", run.state, secs)
            }
            (None, Some(row)) => row.state.clone(),
            (None, None) => String::new(),
        };
        self.ui.label(cx, ids!(run_state)).set_text(cx, &text);
    }

    // -- events from the server ------------------------------------------------

    fn poll_subscription(&mut self, cx: &mut Cx) {
        let Some(subscriber) = self.subscriber.as_ref() else {
            return;
        };
        let events = subscriber.poll();
        for event in events {
            match event {
                SubscriptionEvent::Ready | SubscriptionEvent::ResyncRequired => {
                    self.refresh_flows();
                    self.refresh_instances();
                    self.fetch_instance();
                }
                SubscriptionEvent::Events(events) => {
                    for event in events {
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
                    if let Some(name) = event.name {
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
        let node = event.node.clone().unwrap_or_default();
        match event.kind.as_str() {
            "run.started" => {
                self.current_node = None;
                self.outputs.clear();
                self.run = Some(RunInfo {
                    run_id: event.run_id.clone().unwrap_or_default(),
                    state: "running".into(),
                    started: self.time,
                    finished_secs: None,
                });
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    canvas.clear_run(cx);
                }
                if let Some(faces) = self.faces.as_mut() {
                    faces.reset_run();
                }
                self.refresh_instances();
            }
            "node.started" => self.set_node_status(cx, &node, "running", 0, None),
            "node.progress" => {
                let permille = event.permille.unwrap_or(0).min(1000) as u16;
                self.set_node_status(cx, &node, "running", permille, None);
            }
            "node.delta" => {
                let port = event.port.clone().unwrap_or_else(|| "text".into());
                let text = event.text.clone().unwrap_or_default();
                if let Some(faces) = self.faces.as_mut() {
                    faces.push_delta(cx, &node, &port, &text);
                }
                if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
                    canvas.set_streaming(cx, &node, true);
                }
            }
            "node.waiting" => {
                self.current_node = Some(node.clone());
                self.set_node_status(cx, &node, "waiting", 0, None);
                if let Some(mut app_view) = self.ui.widget(cx, ids!(app_view)).borrow_mut::<AppView>() {
                    app_view.waiting = Some(node.clone());
                }
            }
            "node.answered" => self.set_node_status(cx, &node, "running", 0, None),
            "node.done" => {
                self.set_node_status(cx, &node, "done", 1000, None);
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
                self.set_node_status(cx, &node, "failed", 0, error);
            }
            "node.skipped" => self.set_node_status(cx, &node, "skipped", 0, event.reason.clone()),
            "run.finished" => {
                let state = event.state_text().unwrap_or_else(|| "done".into());
                for (output_node, value) in event.output_values() {
                    let port = self.output_face_port(&output_node);
                    self.record_value(cx, &output_node, &port, value);
                }
                self.request_wanted_values(cx);
                if let Some(run) = self.run.as_mut() {
                    run.state = state.clone();
                    run.finished_secs = event.secs;
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
                self.update_run_state(cx);
            }
            _ => {}
        }
    }

    fn set_node_status(&mut self, cx: &mut Cx, node: &str, state: &str, permille: u16, error: Option<String>) {
        if matches!(state, "running" | "waiting") {
            self.current_node = Some(node.to_string());
        }
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_status(
                cx,
                node,
                NodeStatus {
                    state: state.to_string(),
                    permille,
                    error,
                },
            );
        }
        if let Some(faces) = self.faces.as_mut() {
            faces.push_state(cx, node, state);
        }
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
        let chip = faces::preview_text(&value)
            .unwrap_or_else(|| format!("{} · {} b", value.content_type, value.bytes));
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_chip(cx, node, port, chip);
            canvas.set_streaming(cx, node, false);
        }
        let bytes = self.values.get(&value.digest);
        if let Some(faces) = self.faces.as_mut() {
            faces.push_value(cx, node, port, &value, bytes.as_ref());
        }
    }

    // -- pending writes ----------------------------------------------------------

    /// Text inputs are debounced by the poll tick (250 ms); everything else
    /// is written on the same tick it changed.
    fn flush_pending(&mut self, cx: &mut Cx) {
        if !self.pending_inputs.is_empty() && self.instance.is_some() {
            let id = self.instance.clone().unwrap();
            let body = self.take_pending_input_body();
            self.io(move |client| {
                IoResult::Done(
                    client
                        .put_inputs(&id, "tab", &body)
                        .map(|_| ()),
                )
            });
        }
        if !self.pending_params.is_empty() && !self.pending_graph {
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
        let inputs = (!self.pending_inputs.is_empty()).then(|| self.take_pending_input_body());
        if let Some(id) = self.instance.clone() {
            self.io(move |client| {
                let result = (|| {
                    if let Some(inputs) = inputs.as_ref() {
                        client.put_inputs(&id, "tab", inputs)?;
                    }
                    client.start_run(&id, outputs.as_deref())
                })();
                IoResult::RunStarted(result)
            });
            return;
        }
        let Some(flow) = self.selected.clone() else {
            return;
        };
        self.io(move |client| {
            let result = (|| {
                let created = client.create_instance(&flow, &CreateInstanceRequest::default())?;
                if let Some(inputs) = inputs.as_ref() {
                    client.put_inputs(&created.instance, "tab", inputs)?;
                }
                let started = client.start_run(&created.instance, outputs.as_deref())?;
                Ok((created.instance, started))
            })();
            IoResult::InstanceRunStarted { flow, result }
        });
    }

    fn take_pending_input_body(&mut self) -> makepad_strict_json::Value {
        let graph = self.current_graph();
        let pending = std::mem::take(&mut self.pending_inputs);
        let mut by_node: HashMap<String, Vec<(String, makepad_strict_json::Value)>> =
            HashMap::new();
        for ((node, port), text) in pending {
            let ty = graph
                .as_ref()
                .and_then(|graph| instance_input_type(graph, &node, &port))
                .unwrap_or(PortType::Text);
            by_node
                .entry(node)
                .or_default()
                .push((port, input_value_json(ty, text)));
        }
        makepad_strict_json::Value::Obj(
            by_node
                .into_iter()
                .map(|(node, ports)| (node, makepad_strict_json::Value::Obj(ports)))
                .collect(),
        )
    }

    fn cancel_run(&mut self) {
        let Some(run) = self.run.clone() else {
            return;
        };
        self.io(move |client| IoResult::Done(client.cancel_run(&run.run_id)));
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
            .view(cx, ids!(inspector_view))
            .set_visible(cx, !self.source_mode);
        self.ui
            .view(cx, ids!(source_view))
            .set_visible(cx, self.source_mode);
        self.ui.button(cx, ids!(view_btn)).set_text(
            cx,
            if self.app_mode { "Canvas" } else { "App view" },
        );
        self.ui.button(cx, ids!(side_btn)).set_text(
            cx,
            if self.source_mode { "Inspector" } else { "Source" },
        );
        self.ui.redraw(cx);
    }

    fn fresh_flow_name(&self) -> String {
        let mut number = self.flows.len() + 1;
        loop {
            let name = format!("flow-{number}");
            if !self.flows.iter().any(|flow| flow.name == name) {
                return name;
            }
            number += 1;
        }
    }

    fn shutdown(&mut self, cx: &mut Cx) {
        self.services.shutdown();
        if let Some(mut canvas) = self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>() {
            canvas.set_face_roots(cx, Vec::new());
        }
        if let Some(faces) = self.faces.take() {
            faces.free(cx);
        }
        if let Some(subscriber) = self.subscriber.take() {
            subscriber.request_stop();
        }
        if let Some(mut session) = self.session.take() {
            session.stop();
        }
        if let Some(host) = self.host.take() {
            host.shutdown();
        }
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
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(call) = action.downcast_ref::<FaceBridgeCall>() {
                self.handle_bridge_call(call.clone());
            }
            if let Some(action) = action.downcast_ref::<FlowUiAction>() {
                match action {
                    FlowUiAction::Focus { instance } => {
                        self.instance = Some(instance.clone());
                        self.instance_row = None;
                        self.run = None;
                        self.outputs.clear();
                        self.fetch_instance();
                        self.refresh_instances();
                    }
                    FlowUiAction::Select { node } => {
                        self.selected_node = Some(node.clone());
                        if let Some(mut canvas) =
                            self.ui.widget(cx, ids!(canvas)).borrow_mut::<FlowCanvas>()
                        {
                            canvas.select(cx, Some(node.clone()));
                        }
                        self.refresh_inspector(cx);
                    }
                }
            }
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
            let name = self.fresh_flow_name();
            self.save_source(name, EMPTY_FLOW_SOURCE.to_string());
        }
        if self.ui.button(cx, ids!(example_btn)).clicked(actions) {
            let name = self.fresh_flow_name();
            self.save_source(name, EXAMPLE_FLOW_SOURCE.to_string());
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
        if self.ui.button(cx, ids!(undo_btn)).clicked(actions) {
            if self.revisions.len() >= 2 {
                self.revisions.pop();
                let revision = *self.revisions.last().unwrap();
                if let Some(name) = self.selected.clone() {
                    self.io(move |client| {
                        let result = client.revert(&name, revision);
                        IoResult::GraphPut { name, result }
                    });
                }
            }
        }
        if self.ui.button(cx, ids!(view_btn)).clicked(actions) {
            self.app_mode = !self.app_mode;
            self.set_modes(cx);
        }
        if self.ui.button(cx, ids!(side_btn)).clicked(actions) {
            self.source_mode = !self.source_mode;
            self.set_modes(cx);
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
                FlowCanvasAction::Select(node) => {
                    self.selected_node = node.clone();
                    if let Some(node) = node.as_deref() {
                        if self.source_mode {
                            self.jump_to_node(cx, node);
                        }
                    }
                    self.refresh_inspector(cx);
                }
                FlowCanvasAction::Edit(edit) => self.apply_edit(cx, edit),
                FlowCanvasAction::OpenPalette {
                    at,
                    from_node,
                    from_port,
                    ty,
                } => {
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
                        .set_text(cx, "click a type to add it at the drop point");
                    self.palette_drop = Some((at, from_node, from_port, ty));
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
                        .set_text(cx, "press a type, release on the canvas");
                }
            }
        }

        // Inspector.
        let inspector_actions = self
            .ui
            .widget(cx, ids!(inspector))
            .borrow::<Inspector>()
            .map(|inspector| inspector.changes(cx, actions))
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
                InspectorAction::OpenValue { node, port, value } => {
                    self.preview_digest = Some((value.digest.clone(), format!("{node}.{port}")));
                    match self.values.get(&value.digest) {
                        Some(bytes) => self.show_preview(cx, &bytes),
                        None => {
                            if let Some(client) = self.client() {
                                self.values.request(&value.digest, client);
                            }
                        }
                    }
                }
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
                    if let Some(flow) = flow {
                        if self.selected.as_deref() != Some(flow.as_str()) {
                            self.selected = Some(flow.clone());
                            self.unsaved = false;
                            self.revisions.clear();
                            self.ui.label(cx, ids!(flow_name)).set_text(cx, &flow);
                            self.load_flow(flow);
                        }
                        self.instance = Some(id);
                        self.run = None;
                        self.outputs.clear();
                        self.remount_faces(cx);
                        self.fetch_instance();
                        self.refresh_instances();
                    }
                }
                RunningAction::Stop(id) => {
                    let target = id.clone();
                    self.io(move |client| IoResult::Done(client.delete_instance(&target)));
                    if self.instance.as_deref() == Some(id.as_str()) {
                        self.instance = None;
                        self.instance_row = None;
                    }
                }
                RunningAction::Duplicate(id) => {
                    let flow = self
                        .instances
                        .iter()
                        .find(|row| row.instance == id)
                        .map(|row| row.flow.clone());
                    if let Some(flow) = flow {
                        self.io(move |client| {
                            let request = CreateInstanceRequest {
                                label: Some("copy".to_string()),
                                ..CreateInstanceRequest::default()
                            };
                            let result = client.create_instance(&flow, &request);
                            IoResult::InstanceCreated { flow, result }
                        });
                    }
                }
                RunningAction::CopyId(id) => cx.copy_to_clipboard(&id),
            }
        }

        // Faces: bound widgets → instance inputs / graph params.
        if let Some(faces) = self.faces.as_ref() {
            for (node, port, text) in faces.bind_changes(actions) {
                self.pending_inputs.insert((node, port), text);
            }
            let mut params = faces.param_changes(actions);
            params.extend(faces.model_changes(cx, actions));
            for (node, key, value) in params {
                self.pending_params.insert((node, key), value);
            }
        }
        self.refresh_ai_context();
    }
}

impl App {
    fn show_preview(&mut self, cx: &mut Cx, bytes: &makepad_flow::ValueBytes) {
        let text = if bytes.content_type.starts_with("image/") {
            String::new()
        } else {
            String::from_utf8_lossy(&bytes.bytes).chars().take(4000).collect()
        };
        if let Some(mut inspector) = self.ui.widget(cx, ids!(inspector)).borrow_mut::<Inspector>() {
            inspector.set_preview(Some((text, bytes.clone())));
        }
        self.refresh_inspector(cx);
    }

    fn drain_values(&mut self, cx: &mut Cx) {
        for arrival in self.values.drain() {
            match arrival {
                Ok(digest) => {
                    if let Some(faces) = self.faces.as_mut() {
                        faces.deliver_bytes(cx, &mut self.values, &digest);
                    }
                    if self
                        .preview_digest
                        .as_ref()
                        .is_some_and(|(wanted, _)| *wanted == digest)
                    {
                        if let Some(bytes) = self.values.get(&digest) {
                            self.show_preview(cx, &bytes);
                        }
                    }
                }
                Err((digest, error)) => {
                    self.set_error(cx, &format!("value {}…: {error}", &digest[..8.min(digest.len())]));
                }
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_code_editor::script_mod(vm);
        makepad_aichat::script_mod(vm);
        canvas::script_mod(vm);
        panels::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.startup(cx);
        }
        if let Event::NextFrame(nf) = event {
            self.time = nf.time;
        }
        self.services.handle_event(cx, event);
        self.match_event(cx, event);
        // The faces first: a click inside a face is the face's, and the
        // canvas then sees it as handled.
        if !matches!(event, Event::Draw(_)) {
            if let Some(faces) = self.faces.as_mut() {
                faces.handle_event(cx, event, &mut Scope::empty());
            }
        }
        match self.faces.as_mut() {
            Some(faces) => {
                let mut scope = Scope::with_data(faces);
                self.ui.handle_event(cx, event, &mut scope);
            }
            None => self.ui.handle_event(cx, event, &mut Scope::empty()),
        }
        if self.poll_timer.is_event(event).is_some() || matches!(event, Event::Signal) {
            self.drain_io(cx);
            self.drain_values(cx);
            self.update_connection(cx);
            self.poll_subscription(cx);
            self.flush_pending(cx);
            if self.source_mode {
                self.highlight_caret_node(cx);
            }
            if self.run.as_ref().is_some_and(|run| run.state == "running") {
                self.update_run_state(cx);
            }
        }
        self.refresh_ai_context();
        if let Event::Shutdown = event {
            self.shutdown(cx);
        }
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
