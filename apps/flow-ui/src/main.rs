mod services;

use crate::services::{BridgeContext, FlowServices, FlowUiAction};
pub use makepad_widgets;

use makepad_flow::client::{
    ClientError, FlowSubscriber, FlowSubscriberConfig, SessionConfig, SessionConnector,
    SessionStatus, SubscriptionEvent,
};
use makepad_flow::embed::{default_root, resolve, EmbedPolicy, Resolved};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{FlowDefinition, FlowSummary, PutFlowResponse};
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;
use std::sync::mpsc::{channel, Receiver, Sender};

app_main!(App);

const NEW_FLOW_SOURCE: &str = r#"use mod.flow.*

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
        style := DropDown{ labels: ["photo", "anime", "oil paint"]  bind: styled.style }
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

#[derive(Debug)]
struct IoPing;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.FlowListBase = #(FlowList::register_widget(vm))
    mod.widgets.FlowList = set_type_default() do mod.widgets.FlowListBase{
        width: 220
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Item := View{
                width: Fill
                height: 36
                flow: Right
                spacing: theme.space_2
                padding: theme.mspace_2
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

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Flow"
                window.inner_size: vec2(1400, 900)
                pass.clear_color: theme.color_bg_app
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down

                    SolidView{
                        width: Fill
                        height: 36
                        flow: Right
                        spacing: theme.space_2
                        padding: theme.mspace_2
                        align: Align{y: 0.5}
                        draw_bg +: {color: theme.color_bg_container}
                        status_chip := Label{
                            width: 260
                            text: "Discovering"
                        }
                        flow_name := Label{
                            width: Fill
                            text: "No flow selected"
                        }
                        new_btn := Button{text: "New"}
                        save_btn := Button{text: "Save"}
                    }

                    View{
                        width: Fill
                        height: Fill
                        flow: Right

                        SolidView{
                            width: 220
                            height: Fill
                            flow: Down
                            padding: theme.mspace_2
                            spacing: theme.space_2
                            draw_bg +: {color: theme.color_bg_container}
                            H2{text: "Flows"}
                            flow_list := mod.widgets.FlowList{}
                        }

                        View{
                            width: Fill
                            height: Fill
                            flow: Down
                            padding: theme.mspace_2
                            spacing: theme.space_2
                            source := TextInput{
                                width: Fill
                                height: Fill
                                is_multiline: true
                                empty_text: "Select a flow to edit its source"
                                draw_text +: {text_style: theme.font_code{}}
                            }
                            error_label := Label{
                                width: Fill
                                height: Fit
                                text: ""
                            }
                        }

                        SolidView{
                            width: 320
                            height: Fill
                            flow: Down
                            padding: theme.mspace_2
                            spacing: theme.space_2
                            draw_bg +: {color: theme.color_bg_container}
                            H2{text: "Graph"}
                            graph_summary := Label{
                                width: Fill
                                height: Fill
                                text: "Select a flow to inspect its nodes and tools."
                                draw_text +: {text_style: theme.font_code{}}
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
                item.button(cx, ids!(select)).set_text(cx, &row.name);
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
    FocusedInstance {
        id: String,
        result: Result<makepad_strict_json::Value, ClientError>,
    },
}

#[derive(Default)]
struct IoMailbox {
    sender: Option<Sender<IoResult>>,
    receiver: Option<Receiver<IoResult>>,
    fetching_flows: bool,
    fetching_focused: bool,
}

impl IoMailbox {
    fn start(&mut self) {
        let (sender, receiver) = channel();
        self.sender = Some(sender);
        self.receiver = Some(receiver);
    }
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
    selected: Option<String>,
    #[rust]
    unsaved: bool,
    #[rust]
    connected_server: Option<[u8; 16]>,
    #[rust]
    embedded: bool,
    #[rust]
    services: FlowServices,
    #[rust]
    focused_instance: Option<String>,
    #[rust]
    focused_instance_state: Option<String>,
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
                        // The host's endpoints carry the token and server id; the
                        // session only needs the two addresses plus the bearer.
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
                        self.ui
                            .label(cx, ids!(error_label))
                            .set_text(cx, &format!("Could not host flow server: {error}"));
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
                "Connected · {} · {} flows",
                if self.embedded { "embedded" } else { "attached" },
                self.flows.len()
            ),
        };
        self.ui.label(cx, ids!(status_chip)).set_text(cx, &text);
        if let SessionStatus::Connected { server_id, .. } = status {
            if self.connected_server != Some(server_id) {
                self.connected_server = Some(server_id);
                if let Some(client) = session.client() {
                    self.services.connect(cx, client.clone());
                    self.subscriber = FlowSubscriber::start(
                        client,
                        FlowSubscriberConfig::default(),
                    )
                    .ok();
                    self.refresh_flows();
                }
            }
        } else {
            if self.connected_server.take().is_some() {
                self.services.disconnect();
                self.subscriber = None;
            }
        }
    }

    fn refresh_flows(&mut self) {
        if self.io.fetching_flows {
            return;
        }
        let (Some(session), Some(sender)) = (self.session.as_ref(), self.io.sender.clone()) else {
            return;
        };
        let Some(client) = session.client() else {
            return;
        };
        self.io.fetching_flows = true;
        std::thread::spawn(move || {
            let result = client
                .lock()
                .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))
                .and_then(|client| client.flows());
            let _ = sender.send(IoResult::Flows(result));
            SignalToUI::set_ui_signal();
        });
    }

    fn load_flow(&mut self, name: String) {
        let (Some(session), Some(sender)) = (self.session.as_ref(), self.io.sender.clone()) else {
            return;
        };
        let Some(client) = session.client() else {
            return;
        };
        std::thread::spawn(move || {
            let result = client
                .lock()
                .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))
                .and_then(|client| client.flow(&name));
            let _ = sender.send(IoResult::Flow { name, result });
            SignalToUI::set_ui_signal();
        });
    }

    fn save_source(&mut self, name: String, source: String) {
        let (Some(session), Some(sender)) = (self.session.as_ref(), self.io.sender.clone()) else {
            return;
        };
        let Some(client) = session.client() else {
            return;
        };
        std::thread::spawn(move || {
            let result = client
                .lock()
                .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))
                .and_then(|client| client.put_source(&name, &source));
            let _ = sender.send(IoResult::Saved {
                name,
                source,
                result,
            });
            SignalToUI::set_ui_signal();
        });
    }

    fn refresh_focused_instance(&mut self) {
        if self.io.fetching_focused {
            return;
        }
        let (Some(id), Some(session), Some(sender)) = (
            self.focused_instance.clone(),
            self.session.as_ref(),
            self.io.sender.clone(),
        ) else {
            return;
        };
        let Some(client) = session.client() else {
            return;
        };
        self.io.fetching_focused = true;
        std::thread::spawn(move || {
            let result = client
                .lock()
                .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))
                .and_then(|client| client.instance(&id));
            let _ = sender.send(IoResult::FocusedInstance { id, result });
            SignalToUI::set_ui_signal();
        });
    }

    fn drain_io(&mut self, cx: &mut Cx) {
        while let Some(Ok(next)) = self.io.receiver.as_ref().map(Receiver::try_recv) {
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
                            self.refresh_ai_context();
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
                        self.selected = Some(name.clone());
                        self.ui.label(cx, ids!(flow_name)).set_text(cx, &name);
                        self.ui.label(cx, ids!(error_label)).set_text(cx, "");
                        self.refresh_flows();
                        self.services.refresh_definitions();
                        self.load_flow(name);
                    }
                    Err(error) => self.show_error(cx, &error),
                },
                IoResult::FocusedInstance { id, result } => {
                    self.io.fetching_focused = false;
                    if self.focused_instance.as_deref() != Some(&id) {
                        continue;
                    }
                    match result {
                        Ok(instance) => {
                            self.focused_instance_state =
                                json_string(&instance, &["state"]);
                            self.current_node =
                                json_string(&instance, &["current_node", "node"]);
                            if let Some(flow) = json_string(&instance, &["flow"]) {
                                if self.selected.as_deref() != Some(&flow) {
                                    self.selected = Some(flow.clone());
                                    self.selected_node = None;
                                    self.ui.label(cx, ids!(flow_name)).set_text(cx, &flow);
                                    self.load_flow(flow);
                                }
                            }
                            self.refresh_ai_context();
                        }
                        Err(error) => self.show_error(cx, &error),
                    }
                }
            }
        }
    }

    fn show_flow(&mut self, cx: &mut Cx, definition: FlowDefinition) {
        self.ui
            .text_input(cx, ids!(source))
            .set_text(cx, &definition.source);
        self.unsaved = false;
        self.ui.label(cx, ids!(error_label)).set_text(
            cx,
            &definition
                .error
                .as_ref()
                .map(format_eval_error)
                .unwrap_or_default(),
        );
        self.last_error = definition.error.as_ref().map(format_eval_error);
        let mut lines = Vec::new();
        if let Some(graph) = &definition.graph {
            for node in &graph.nodes {
                let ports = node
                    .inputs
                    .iter()
                    .map(|input| input.port.as_str())
                    .chain(node.outputs.iter().map(|output| output.name.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("{} · {} · {}", node.id, node.type_name, ports));
            }
            if !graph.tools.is_empty() {
                lines.push(String::new());
                for tool in &graph.tools {
                    lines.push(format!(
                        "{}{{{}}} → {{{}}}",
                        tool.name,
                        tool.inputs.join(", "),
                        tool.outputs.join(", ")
                    ));
                }
            }
        } else {
            lines.push("no graph yet — the source has never evaluated".to_string());
        }
        self.ui
            .label(cx, ids!(graph_summary))
            .set_text(cx, &lines.join("\n"));
        self.refresh_ai_context();
    }

    fn show_error(&mut self, cx: &mut Cx, error: &ClientError) {
        let text = match error {
            ClientError::Eval(error) => format_eval_error(error),
            other => other.to_string(),
        };
        self.last_error = Some(text.clone());
        self.ui.label(cx, ids!(error_label)).set_text(cx, &text);
        self.refresh_ai_context();
    }

    fn poll_subscription(&mut self) {
        let Some(subscriber) = self.subscriber.as_ref() else {
            return;
        };
        let events = subscriber.poll();
        for event in events {
            match event {
                SubscriptionEvent::Ready | SubscriptionEvent::ResyncRequired => {
                    self.refresh_flows();
                    self.refresh_focused_instance();
                }
                SubscriptionEvent::Events(events) => {
                    for event in events {
                        if event.kind == "flow.changed" {
                            self.refresh_flows();
                            self.services.refresh_definitions();
                            if !self.unsaved
                                && event.name.as_deref() == self.selected.as_deref()
                            {
                                if let Some(name) = event.name {
                                    self.load_flow(name);
                                }
                            }
                        } else if event.kind == "flow.removed" || event.kind == "flow.error" {
                            self.refresh_flows();
                            self.services.refresh_definitions();
                        } else if (event.kind.starts_with("instance.")
                            || event.kind.starts_with("run.")
                            || event.kind.starts_with("node."))
                            && self.focused_instance.as_deref() == event.instance.as_deref()
                        {
                            self.refresh_focused_instance();
                        }
                    }
                }
                SubscriptionEvent::Retry { .. } => {}
            }
        }
    }

    fn shutdown(&mut self) {
        self.services.shutdown();
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
        self.services.set_context(BridgeContext {
            flow: self.selected.clone(),
            revision: selected.map(|flow| flow.revision),
            canonical: selected.map(|flow| flow.canonical),
            instance: self.focused_instance.clone(),
            instance_state: self.focused_instance_state.clone(),
            current_node: self.current_node.clone(),
            selected_node: self.selected_node.clone(),
            open_view: "source".to_string(),
            last_error: self.last_error.clone(),
        });
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if action.downcast_ref::<IoPing>().is_some() {
                self.drain_io(cx);
            }
            if let Some(action) = action.downcast_ref::<FlowUiAction>() {
                match action {
                    FlowUiAction::Focus { instance } => {
                        self.focused_instance = Some(instance.clone());
                        self.focused_instance_state = None;
                        self.current_node = None;
                        self.refresh_focused_instance();
                    }
                    FlowUiAction::Select { node } => {
                        self.selected_node = Some(node.clone());
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
                self.selected = Some(name.clone());
                self.selected_node = None;
                self.last_error = None;
                self.unsaved = false;
                self.ui.label(cx, ids!(flow_name)).set_text(cx, &name);
                self.load_flow(name);
            }
        }
        if self.ui.button(cx, ids!(new_btn)).clicked(actions) {
            let mut number = self.flows.len() + 1;
            let name = loop {
                let name = format!("flow-{number}");
                if !self.flows.iter().any(|flow| flow.name == name) {
                    break name;
                }
                number += 1;
            };
            self.save_source(name, NEW_FLOW_SOURCE.to_string());
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
        self.refresh_ai_context();
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_aichat::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.startup(cx);
        }
        self.services.handle_event(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if self.poll_timer.is_event(event).is_some() || matches!(event, Event::Signal) {
            self.drain_io(cx);
            self.update_connection(cx);
            self.poll_subscription();
        }
        self.refresh_ai_context();
        if let Event::Shutdown = event {
            self.shutdown();
        }
    }
}

fn format_eval_error(error: &makepad_flow::EvalError) -> String {
    format!("{}:{} {}", error.line, error.col, error.message)
}

fn json_string(value: &makepad_strict_json::Value, names: &[&str]) -> Option<String> {
    let direct = names
        .iter()
        .find_map(|name| value.get(name).and_then(|value| value.as_str()))
        .map(str::to_string);
    direct.or_else(|| {
        let instance = value.get("instance")?;
        names
            .iter()
            .find_map(|name| instance.get(name).and_then(|value| value.as_str()))
            .map(str::to_string)
    })
}
