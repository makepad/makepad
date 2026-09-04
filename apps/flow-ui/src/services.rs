//! AI-service bridge for the embedded flow server.
//!
//! The UI thread only owns ports and drains channels. Every HTTP call runs
//! on a worker, including manifest refreshes and the 500 ms run wait loop.

use makepad_ai_services::{
    AiServicePort, Disposition, Message, PortEvent, Risk, ServiceCall, ServiceManifest,
    SubscriptionRequest, ToolDef, ToolResult, TopicDef,
};
use makepad_flow::client::{ClientError, FlowClient};
use makepad_flow::{
    Event as FlowEvent, FlowDefinition, FlowSummary, Graph, ModelsResponse, Node, NodeInputValue,
    PortType, TemplateSummary, ToolEntry, ToolSchema, AUTHORING_BRIEF,
};
use makepad_strict_json::Value as Json;
use makepad_widgets::makepad_platform::makepad_micro_serde::SerJson;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SERVICE_ID: &str = "flows";
const UI_SERVICE_ID: &str = "flow_ui";
const RUN_WAIT: Duration = Duration::from_secs(8);
const RUN_POLL: Duration = Duration::from_millis(500);
const PROGRESS_INTERVAL_SECS: f64 = 0.5;
const MAX_READ_TEXT: usize = 16 * 1024;
const MAX_INLINE_VALUE: usize = 2 * 1024;

const EXAMPLE_FLOW: &str = r#"use mod.flow.*
/** What to paint. */
let prompt = Input{ type: @text default: "a lighthouse at dusk" at: vec2(40, 120) }
let expand = Llm{
    system: "Rewrite the prompt as one vivid paragraph for an image model."
    prompt: prompt.text()
    at: vec2(360, 120)
}
/** Appends the chosen style to the expanded prompt. */
let add_style = Fn{
    in: { text: expand.text() style: "photo" }
    out: [@text]
    run: |i| { {text: i.text + ", " + i.style + " style"} }
    at: vec2(680, 120)
}
let image = Image{ prompt: add_style.text() width: 1024 height: 1024 steps: 8 at: vec2(1000, 120) }
/** The finished picture. */
let picture = Output{ type: @image value: image.image() }
Flow{
    label: "Prompt to image"
    brief: "Expands a short prompt into a rich one and paints it."
    prompt, expand, add_style, image, picture
}
"#;

type ClientHandle = Arc<Mutex<FlowClient>>;

/// Actions posted by the tiny `flow_ui` service. The current source shell
/// records these; the canvas lane can consume the same action without a new
/// service contract.
#[derive(Clone, Debug, PartialEq)]
pub enum FlowUiAction {
    Focus { instance: String },
    Select { node: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeContext {
    pub flow: Option<String>,
    pub revision: Option<u64>,
    pub canonical: Option<bool>,
    pub instance: Option<String>,
    pub instance_state: Option<String>,
    pub current_node: Option<String>,
    pub selected_node: Option<String>,
    pub open_view: String,
    pub last_error: Option<String>,
}

struct DefinitionPort {
    name: String,
    definition: FlowDefinition,
    port: AiServicePort,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SubscriptionFilter {
    All,
    Instance(String),
    Run(String),
}

#[derive(Clone, Debug)]
struct LiveSubscription {
    service: String,
    service_flow: Option<String>,
    sub_id: String,
    topic: String,
    filter: SubscriptionFilter,
    last_progress_at: Option<f64>,
    pending_progress: Option<FlowEvent>,
}

#[derive(Clone, Debug)]
struct Publication {
    service: String,
    sub_id: String,
    message: Message,
}

#[derive(Default)]
struct SubscriptionTable {
    live: BTreeMap<(String, String), LiveSubscription>,
    run_instances: BTreeMap<String, String>,
}

impl SubscriptionTable {
    fn insert(
        &mut self,
        service: String,
        service_flow: Option<String>,
        sub_id: String,
        topic: String,
        filter: SubscriptionFilter,
    ) {
        let key = (service.clone(), sub_id.clone());
        self.live.insert(
            key,
            LiveSubscription {
                service,
                service_flow,
                sub_id,
                topic,
                filter,
                last_progress_at: None,
                pending_progress: None,
            },
        );
    }

    fn remove(&mut self, service: &str, sub_id: &str) {
        self.live
            .remove(&(service.to_string(), sub_id.to_string()));
    }

    fn remove_service(&mut self, service: &str) {
        self.live.retain(|(owner, _), _| owner != service);
    }

    fn clear(&mut self) {
        self.live.clear();
        self.run_instances.clear();
    }

    fn route(&mut self, event: &FlowEvent, now: f64) -> Vec<Publication> {
        if let (Some(run_id), Some(instance)) = (&event.run_id, &event.instance) {
            self.run_instances
                .insert(run_id.clone(), instance.clone());
        }

        let mut publications = self.flush_due(now);
        let mut finished = Vec::new();
        for (key, subscription) in &mut self.live {
            if !subscription_matches(subscription, event, &self.run_instances) {
                continue;
            }
            let final_ = subscription.topic == "run" && event.kind == "run.finished"
                || subscription_target_vanished(subscription, event, &self.run_instances);
            if subscription.topic == "run" && event.kind == "node.progress" && !final_ {
                if subscription
                    .last_progress_at
                    .is_some_and(|last| now - last < PROGRESS_INTERVAL_SECS)
                {
                    subscription.pending_progress = Some(event.clone());
                    continue;
                }
                subscription.last_progress_at = Some(now);
            }
            if final_ {
                subscription.pending_progress = None;
                finished.push(key.clone());
            }
            publications.push(publication(subscription, event, final_));
        }
        for key in finished {
            self.live.remove(&key);
        }
        publications
    }

    fn flush_due(&mut self, now: f64) -> Vec<Publication> {
        let mut publications = Vec::new();
        for subscription in self.live.values_mut() {
            let due = subscription
                .last_progress_at
                .is_some_and(|last| now - last >= PROGRESS_INTERVAL_SECS);
            if !due {
                continue;
            }
            if let Some(event) = subscription.pending_progress.take() {
                subscription.last_progress_at = Some(now);
                publications.push(publication(subscription, &event, false));
            }
        }
        publications
    }

    fn len(&self) -> usize {
        self.live.len()
    }
}

enum WorkerMessage {
    Definitions {
        epoch: u64,
        result: Result<Vec<(String, FlowDefinition)>, ClientError>,
    },
    Progress {
        epoch: u64,
        service: String,
        call_id: String,
        note: String,
        permille: u16,
    },
    Done {
        epoch: u64,
        service: String,
        call_id: String,
        result: ToolResult,
    },
}

/// All service ports exposed by flow-ui while its session is connected.
pub struct FlowServices {
    flows: Option<AiServicePort>,
    flow_ui: Option<AiServicePort>,
    definitions: BTreeMap<String, DefinitionPort>,
    client: Option<ClientHandle>,
    tx: Sender<WorkerMessage>,
    rx: Receiver<WorkerMessage>,
    cancelling: HashMap<(String, String), Arc<AtomicBool>>,
    instances: Arc<Mutex<BTreeMap<String, String>>>,
    syncing: bool,
    resync: bool,
    epoch: u64,
    subscriptions: SubscriptionTable,
    context: BridgeContext,
    last_message_summary: String,
    last_context: String,
}

impl Default for FlowServices {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            flows: None,
            flow_ui: None,
            definitions: BTreeMap::new(),
            client: None,
            tx,
            rx,
            cancelling: HashMap::new(),
            instances: Arc::new(Mutex::new(BTreeMap::new())),
            syncing: false,
            resync: false,
            epoch: 0,
            subscriptions: SubscriptionTable::default(),
            context: BridgeContext::default(),
            last_message_summary: String::new(),
            last_context: String::new(),
        }
    }
}

impl FlowServices {
    pub fn connect(&mut self, cx: &mut Cx, client: ClientHandle) {
        self.disconnect();
        self.epoch = self.epoch.wrapping_add(1);
        self.client = Some(client);
        self.flows = AiServicePort::open(cx, flows_manifest());
        self.flow_ui = AiServicePort::open(cx, flow_ui_manifest());
        self.refresh_definitions();
    }

    pub fn disconnect(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        for cancel in self.cancelling.values() {
            cancel.store(true, Ordering::Release);
        }
        self.cancelling.clear();
        if let Some(port) = self.flows.take() {
            port.unregister();
        }
        if let Some(port) = self.flow_ui.take() {
            port.unregister();
        }
        for (_, service) in std::mem::take(&mut self.definitions) {
            service.port.unregister();
        }
        self.client = None;
        self.syncing = false;
        self.resync = false;
        self.subscriptions.clear();
        self.context = BridgeContext::default();
        self.last_message_summary.clear();
        self.last_context.clear();
        if let Ok(mut instances) = self.instances.lock() {
            instances.clear();
        }
    }

    pub fn refresh_definitions(&mut self) {
        if self.syncing {
            self.resync = true;
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        self.syncing = true;
        let tx = self.tx.clone();
        let epoch = self.epoch;
        let spawned = std::thread::Builder::new()
            .name("flow-ai-manifests".into())
            .spawn(move || {
                let result = (|| {
                    let rows = call_client(&client, FlowClient::flows)?;
                    let mut definitions = Vec::new();
                    for row in rows {
                        let definition = call_client(&client, |client| client.flow(&row.name))?;
                        if definition.graph.is_some() {
                            definitions.push((row.name, definition));
                        }
                    }
                    Ok(definitions)
                })();
                let _ = tx.send(WorkerMessage::Definitions { epoch, result });
                SignalToUI::set_ui_signal();
            });
        if spawned.is_err() {
            self.syncing = false;
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let mut events = Vec::new();
        if let Some(port) = self.flows.as_mut() {
            events.extend(
                port.handle_event(cx, event)
                    .into_iter()
                    .map(|event| (SERVICE_ID.to_string(), event)),
            );
        }
        if let Some(port) = self.flow_ui.as_mut() {
            events.extend(
                port.handle_event(cx, event)
                    .into_iter()
                    .map(|event| (UI_SERVICE_ID.to_string(), event)),
            );
        }
        for (service_id, service) in &mut self.definitions {
            events.extend(
                service
                    .port
                    .handle_event(cx, event)
                    .into_iter()
                    .map(|event| (service_id.clone(), event)),
            );
        }
        for (service, event) in events {
            match event {
                PortEvent::Call(call) if service == UI_SERVICE_ID => {
                    self.answer_ui(cx, call);
                }
                PortEvent::Call(call) if service == SERVICE_ID => {
                    self.start_flows_call(call);
                }
                PortEvent::Call(call) => self.start_definition_call(&service, call),
                PortEvent::Cancel { call_id } => {
                    if let Some(cancel) = self.cancelling.get(&(service, call_id)) {
                        cancel.store(true, Ordering::Release);
                    }
                }
                PortEvent::Subscribe {
                    sub_id,
                    topic,
                    filter,
                } => self.subscribe(&service, sub_id, topic, filter),
                PortEvent::Unsubscribe { sub_id } => {
                    self.subscriptions.remove(&service, &sub_id);
                    self.publish_context();
                }
                PortEvent::Registered(_) | PortEvent::ChatOpen { .. } => {}
            }
        }
        let publications = self.subscriptions.flush_due(Cx::monotonic_now());
        self.publish_all(publications);
        self.drain_workers(cx);
    }

    /// Fan one event from the UI's existing `FlowSubscriber` into the AI bus.
    pub fn handle_flow_event(&mut self, event: &FlowEvent) {
        let publications = self.subscriptions.route(event, Cx::monotonic_now());
        self.publish_all(publications);
    }

    pub fn set_context(&mut self, context: BridgeContext) {
        self.context = context;
        self.publish_context();
    }

    fn publish_context(&mut self) {
        let text = render_context(
            &self.context,
            self.subscriptions.len(),
            &self.last_message_summary,
        );
        if text == self.last_context {
            return;
        }
        self.last_context = text.clone();
        if let Some(port) = self.flows.as_ref() {
            port.set_context(&text);
        }
    }

    fn subscribe(
        &mut self,
        service: &str,
        sub_id: String,
        topic: String,
        filter: Option<String>,
    ) {
        let service_flow = self
            .definitions
            .get(service)
            .map(|definition| definition.name.clone());
        match subscription_filter(service, service_flow.as_deref(), &topic, filter.as_deref()) {
            Ok(filter) => self.subscriptions.insert(
                service.to_string(),
                service_flow,
                sub_id,
                topic,
                filter,
            ),
            Err(message) => {
                if let Some(port) = self.port(service) {
                    port.publish(
                        sub_id,
                        Message::new(topic, message).final_message(),
                    );
                }
            }
        }
        self.publish_context();
    }

    fn publish_all(&mut self, publications: Vec<Publication>) {
        for publication in publications {
            if let Some(port) = self.port(&publication.service) {
                port.publish(publication.sub_id, publication.message.clone());
            }
            self.last_message_summary = publication.message.text;
        }
        self.publish_context();
    }

    pub fn shutdown(&mut self) {
        self.disconnect();
    }

    fn answer_ui(&self, cx: &mut Cx, call: ServiceCall) {
        let result = match call.tool.as_str() {
            "focus" => match required_string(&call, "instance") {
                Ok(instance) => {
                    cx.action(FlowUiAction::Focus {
                        instance: instance.clone(),
                    });
                    ToolResult::ok(
                        &call.call_id,
                        format!("focused instance {instance}"),
                        "focused instance",
                    )
                }
                Err(result) => result,
            },
            "select" => match required_string(&call, "node") {
                Ok(node) => {
                    cx.action(FlowUiAction::Select { node: node.clone() });
                    ToolResult::ok(
                        &call.call_id,
                        format!("selected node {node}"),
                        "selected node",
                    )
                }
                Err(result) => result,
            },
            other => ToolResult::refused(
                &call.call_id,
                format!("flow_ui has no tool `{other}`; it has focus and select"),
            ),
        };
        if let Some(port) = self.flow_ui.as_ref() {
            port.reply(result);
        }
    }

    fn start_flows_call(&mut self, call: ServiceCall) {
        let Some(client) = self.client.clone() else {
            self.reply(
                SERVICE_ID,
                ToolResult::unavailable(&call.call_id, "flow server is not connected"),
            );
            return;
        };
        self.spawn_call(SERVICE_ID.to_string(), call, move |call, cancel, tx, epoch, service| {
            run_flows_call(&client, &call, &cancel, &tx, epoch, &service)
        });
    }

    fn start_definition_call(&mut self, service_id: &str, call: ServiceCall) {
        let Some(client) = self.client.clone() else {
            self.reply(
                service_id,
                ToolResult::unavailable(&call.call_id, "flow server is not connected"),
            );
            return;
        };
        let Some(service) = self.definitions.get(service_id) else {
            return;
        };
        let name = service.name.clone();
        let definition = service.definition.clone();
        let instances = self.instances.clone();
        self.spawn_call(
            service_id.to_string(),
            call,
            move |call, cancel, tx, epoch, service_id| {
                run_definition_call(
                    &client,
                    &service_id,
                    &name,
                    &definition,
                    &instances,
                    &call,
                    &cancel,
                    &tx,
                    epoch,
                )
            },
        );
    }

    fn spawn_call(
        &mut self,
        service: String,
        call: ServiceCall,
        work: impl FnOnce(
                ServiceCall,
                Arc<AtomicBool>,
                Sender<WorkerMessage>,
                u64,
                String,
            ) -> ToolResult
            + Send
            + 'static,
    ) {
        let call_id = call.call_id.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancelling
            .insert((service.clone(), call_id.clone()), cancel.clone());
        let tx = self.tx.clone();
        let done_tx = tx.clone();
        let epoch = self.epoch;
        let worker_service = service.clone();
        let worker_call_id = call_id.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("flow-ai-{service}"))
            .spawn(move || {
                let result = work(call, cancel, tx, epoch, worker_service.clone());
                let _ = done_tx.send(WorkerMessage::Done {
                    epoch,
                    service: worker_service,
                    call_id: worker_call_id,
                    result,
                });
                SignalToUI::set_ui_signal();
            });
        if let Err(error) = spawned {
            self.cancelling.remove(&(service.clone(), call_id.clone()));
            self.reply(
                &service,
                ToolResult::failed(&call_id, format!("could not start service worker: {error}")),
            );
        }
    }

    fn drain_workers(&mut self, cx: &mut Cx) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                WorkerMessage::Definitions { epoch, result } if epoch == self.epoch => {
                    self.syncing = false;
                    match result {
                        Ok(definitions) => self.install_definitions(cx, definitions),
                        Err(error) => {
                            makepad_widgets::error!("flow AI manifest refresh failed: {error}")
                        }
                    }
                    if std::mem::take(&mut self.resync) {
                        self.refresh_definitions();
                    }
                }
                WorkerMessage::Progress {
                    epoch,
                    service,
                    call_id,
                    note,
                    permille,
                } if epoch == self.epoch => {
                    if let Some(port) = self.port(&service) {
                        port.progress(&call_id, &note, permille);
                    }
                }
                WorkerMessage::Done {
                    epoch,
                    service,
                    call_id,
                    result,
                } if epoch == self.epoch => {
                    self.cancelling.remove(&(service.clone(), call_id));
                    self.reply(&service, result);
                }
                WorkerMessage::Definitions { .. }
                | WorkerMessage::Progress { .. }
                | WorkerMessage::Done { .. } => {}
            }
        }
    }

    fn install_definitions(&mut self, cx: &mut Cx, definitions: Vec<(String, FlowDefinition)>) {
        for (service_id, service) in std::mem::take(&mut self.definitions) {
            self.subscriptions.remove_service(&service_id);
            service.port.unregister();
        }
        for (name, definition) in definitions {
            let service_id = per_flow_service_id(&name);
            let manifest = definition_manifest(&name, &definition);
            let Some(port) = AiServicePort::open(cx, manifest) else {
                continue;
            };
            self.definitions.insert(
                service_id,
                DefinitionPort {
                    name,
                    definition,
                    port,
                },
            );
        }
        let live: Vec<String> = self.definitions.keys().cloned().collect();
        if let Ok(mut instances) = self.instances.lock() {
            instances.retain(|service, _| live.contains(service));
        }
    }

    fn port(&self, service: &str) -> Option<&AiServicePort> {
        match service {
            SERVICE_ID => self.flows.as_ref(),
            UI_SERVICE_ID => self.flow_ui.as_ref(),
            _ => self.definitions.get(service).map(|service| &service.port),
        }
    }

    fn reply(&self, service: &str, result: ToolResult) {
        if let Some(port) = self.port(service) {
            port.reply(result);
        }
    }
}

fn subscription_filter(
    service: &str,
    service_flow: Option<&str>,
    topic: &str,
    raw: Option<&str>,
) -> Result<SubscriptionFilter, String> {
    let fields = match raw {
        Some(raw) => match makepad_strict_json::parse(raw.as_bytes()) {
            Ok(Json::Obj(fields)) => fields,
            _ => return Err(format!("cannot subscribe to {topic}: filter must be a JSON object")),
        },
        None => Vec::new(),
    };
    if service == SERVICE_ID && topic == "flows" {
        return if fields.is_empty() {
            Ok(SubscriptionFilter::All)
        } else {
            Err("cannot subscribe to flows: this topic has no filters".into())
        };
    }
    if service == SERVICE_ID && topic == "instance" {
        return one_filter(&fields, "instance").map(SubscriptionFilter::Instance);
    }
    if (service == SERVICE_ID || service_flow.is_some()) && topic == "run" {
        return one_filter(&fields, "run_id").map(SubscriptionFilter::Run);
    }
    Err(format!("service {service} does not publish topic {topic}"))
}

fn one_filter(fields: &[(String, Json)], name: &str) -> Result<String, String> {
    if fields.len() != 1 {
        return Err(format!("topic needs exactly one {{{name}}} filter"));
    }
    string_field(fields, name)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("topic needs a string {{{name}}} filter"))
}

fn subscription_matches(
    subscription: &LiveSubscription,
    event: &FlowEvent,
    run_instances: &BTreeMap<String, String>,
) -> bool {
    if subscription
        .service_flow
        .as_ref()
        .is_some_and(|flow| event.flow.as_deref() != Some(flow.as_str()))
    {
        return false;
    }
    match (&subscription.topic[..], &subscription.filter) {
        ("flows", SubscriptionFilter::All) => event.topic == "flows",
        ("instance", SubscriptionFilter::Instance(instance)) => {
            event.instance.as_deref() == Some(instance.as_str())
                && (event.topic == "instance"
                    || event.kind.starts_with("instance.")
                    || event.kind == "node.failed")
        }
        ("run", SubscriptionFilter::Run(run_id)) => {
            event.topic == "run" && event.run_id.as_deref() == Some(run_id.as_str())
                || event.kind == "instance.removed"
                    && event.instance.as_deref().is_some_and(|instance| {
                        run_instances.get(run_id).map(String::as_str) == Some(instance)
                    })
        }
        _ => false,
    }
}

fn subscription_target_vanished(
    subscription: &LiveSubscription,
    event: &FlowEvent,
    run_instances: &BTreeMap<String, String>,
) -> bool {
    if event.kind != "instance.removed" {
        return false;
    }
    match &subscription.filter {
        SubscriptionFilter::Instance(instance) => event.instance.as_deref() == Some(instance),
        SubscriptionFilter::Run(run_id) => event.instance.as_deref().is_some_and(|instance| {
            run_instances.get(run_id).map(String::as_str) == Some(instance)
        }),
        SubscriptionFilter::All => false,
    }
}

fn publication(subscription: &LiveSubscription, event: &FlowEvent, final_: bool) -> Publication {
    let mut message = Message::new(&subscription.topic, render_event_message(event))
        .with_data(event.serialize_json());
    if final_ {
        message = message.final_message();
    }
    Publication {
        service: subscription.service.clone(),
        sub_id: subscription.sub_id.clone(),
        message,
    }
}

fn render_event_message(event: &FlowEvent) -> String {
    let flow = event
        .name
        .as_deref()
        .or(event.flow.as_deref())
        .unwrap_or("?");
    let instance = event.instance.as_deref().unwrap_or("?");
    let run = event.run_id.as_deref().unwrap_or("?");
    let node = event.node.as_deref().unwrap_or("?");
    let state = event.state_text().unwrap_or_else(|| "done".into());
    let outputs = render_event_outputs(event);
    let text = match event.kind.as_str() {
        "flow.changed" => format!(
            "flow {flow} changed · revision {} · canonical={}",
            event.revision.map_or_else(|| "?".into(), |value| value.to_string()),
            event.canonical.map_or_else(|| "?".into(), |value| value.to_string())
        ),
        "flow.error" => format!(
            "flow {flow} error · {}",
            one_line(&event.error_text().unwrap_or_else(|| "unknown error".into()))
        ),
        "flow.removed" => format!("flow {flow} removed"),
        "instance.created" => format!("instance {instance} started · {flow}"),
        "instance.removed" => format!("instance {instance} stopped · {flow}"),
        "instance.inputs" => format!("instance {instance} inputs changed"),
        "run.started" => format!("run {run} started · instance {instance}"),
        "node.started" => format!("node {node} · started"),
        "node.progress" => format!(
            "node {node} · progress {} %{}",
            event.permille.unwrap_or(0).div_ceil(10),
            event
                .stage
                .as_deref()
                .filter(|stage| !stage.is_empty())
                .map(|stage| format!(" · {}", one_line(stage)))
                .unwrap_or_default()
        ),
        "node.delta" => format!(
            "node {node} · {} · {}",
            event.port.as_deref().unwrap_or("output"),
            one_line(event.text.as_deref().unwrap_or(""))
        ),
        "node.waiting" => format!(
            "instance {instance} waiting · {}",
            one_line(event.question.as_deref().unwrap_or("answer required"))
        ),
        "node.answered" => format!(
            "node {node} · answered · by {}",
            event.by.as_deref().unwrap_or("unknown")
        ),
        "node.done" => append_event_outputs(format!("node {node} · done"), &outputs),
        "node.failed" => format!(
            "node {node} · failed · {}",
            one_line(&event.error_text().unwrap_or_else(|| "unknown error".into()))
        ),
        "node.skipped" => format!(
            "node {node} · skipped · {}",
            one_line(event.reason.as_deref().unwrap_or("no reason given"))
        ),
        "run.finished" => {
            let text = append_event_outputs(format!("run {run} finished · {state}"), &outputs);
            match event.secs {
                Some(secs) => format!("{text} · {secs:.1} s"),
                None => text,
            }
        }
        other => format!("{other} · instance {instance} · run {run}"),
    };
    one_line(&text)
}

fn append_event_outputs(mut text: String, outputs: &str) -> String {
    if !outputs.is_empty() {
        text.push_str(" · ");
        text.push_str(outputs);
    }
    text
}

fn render_event_outputs(event: &FlowEvent) -> String {
    event
        .output_values()
        .into_iter()
        .map(|(name, value)| {
            let mut text = format!("{name} sha256:{}", value.digest);
            if !value.content_type.is_empty() {
                text.push_str(&format!(" {}", value.content_type));
            }
            if value.bytes > 0 {
                text.push_str(&format!(" {} bytes", value.bytes));
            }
            text
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn flows_manifest() -> ServiceManifest {
    let brief = bounded(
        format!(
            "{AUTHORING_BRIEF}\nRead flows.nodes before writing a type you have not seen.\nRead flows.templates to discover reusable starting points.\nRead flows.models to inspect the live model fleet before choosing a model.\nUse flows.create to create a named flow from a template.\n\nExample file:\n{EXAMPLE_FLOW}"
        ),
        makepad_ai_services::wire::MAX_BRIEF_BYTES,
    );
    ServiceManifest::new(SERVICE_ID, "Flows", brief)
        .with_topic(TopicDef::new(
            "flows",
            "Flow definitions and live instances being created, changed, or removed.",
        ))
        .with_topic(TopicDef::new(
            "instance",
            "One instance's input, run, node, question, output, and error events; filter with {instance}.",
        ))
        .with_topic(TopicDef::new(
            "run",
            "One run's full event stream including progress; filter with {run_id}; final on run.finished.",
        ))
        .with_tool(tool("list", "List flow definitions with state, canonical status, and live instance counts.", empty_schema(), Risk::Read))
        .with_tool(tool("templates", "List flow templates with labels, briefs, inputs, and outputs.", empty_schema(), Risk::Read))
        .with_tool(tool("models", "List the live fleet's models, optionally restricted to one generation domain.", r#"{"type":"object","properties":{"domain":{"type":"string"}},"additionalProperties":false}"#, Risk::Read))
        .with_tool(tool("assets", "List assets written by flows, newest first; set ns to * to widen beyond flow assets.", r#"{"type":"object","properties":{"q":{"type":"string"},"ns":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"additionalProperties":false}"#, Risk::Read))
        .with_tool(tool("create", "Create and evaluate a named flow definition from a built-in template.", r#"{"type":"object","properties":{"name":{"type":"string"},"template":{"type":"string"}},"required":["name","template"],"additionalProperties":false}"#, Risk::Act))
        .with_tool(tool("nodes", "Read runnable flow node types, ports, defaults, docs, and range hints before authoring unfamiliar nodes.", empty_schema(), Risk::Read))
        .with_tool(tool("read", "Read one flow's splash source, graph node summary, and last evaluation error.", one_string_schema("name", "flow definition name"), Risk::Read))
        .with_tool(tool("write", "Write and evaluate a complete splash flow definition; evaluation errors include file, line, and column for correction.", r#"{"type":"object","properties":{"name":{"type":"string"},"source":{"type":"string"}},"required":["name","source"],"additionalProperties":false}"#, Risk::Act))
        .with_tool(tool("delete", "Delete one flow definition and stop using its service manifest.", one_string_schema("name", "flow definition name"), Risk::Destructive))
        .with_tool(tool("instances", "List all live instances with owner, state, current node, and last outputs.", empty_schema(), Risk::Read))
        .with_tool(tool("instance", "Read one instance including its current inputs and outputs.", one_string_schema("id", "instance id"), Risk::Read))
        .with_tool(tool("start", "Create one live instance of a flow, optionally with an instance label and initial node inputs.", r#"{"type":"object","properties":{"name":{"type":"string"},"inputs":{"type":"object"},"label":{"type":"string"}},"required":["name"],"additionalProperties":false}"#, Risk::Act))
        .with_tool(tool("stop", "Stop and remove one live instance and cancel its runs.", one_string_schema("id", "instance id"), Risk::Act))
        .with_tool(tool("send", "Set one node input on a live instance; omit port only when the node has one input value port.", r#"{"type":"object","properties":{"id":{"type":"string"},"node":{"type":"string"},"port":{"type":"string"},"value":{}},"required":["id","node","value"],"additionalProperties":false}"#, Risk::Act))
        .with_tool(tool("answer", "Answer the Ask node where an instance is waiting; fails if that instance is not waiting there.", r#"{"type":"object","properties":{"id":{"type":"string"},"node":{"type":"string"},"value":{}},"required":["id","node","value"],"additionalProperties":false}"#, Risk::Act))
        .with_tool(tool("waiting", "List instances currently parked on an Ask question, including its options.", empty_schema(), Risk::Read))
        .with_tool(tool("run", "Start a run on an existing instance; after 8 seconds continue through a run subscription.", r#"{"type":"object","properties":{"id":{"type":"string"},"outputs":{"type":"array","items":{"type":"string"}}},"required":["id"],"additionalProperties":false}"#, Risk::Act))
        .with_tool(tool("status", "Read one run's state, node states, progress, errors, and outputs.", one_string_schema("run_id", "run id"), Risk::Read))
        .with_tool(tool("cancel", "Cancel one queued, running, or waiting run.", one_string_schema("run_id", "run id"), Risk::Act))
        .with_tool(tool("outputs", "Read an instance's last outputs, materializing media values to scratch paths.", one_string_schema("id", "instance id"), Risk::Read))
        .with_tool(tool("watch", "Subscribe to one live instance's input, run, node, question, output, and error events.", one_string_schema("id", "instance id"), Risk::Act))
        .with_tool(tool("save", "Materialize one content-addressed value under the flow scratch values directory and return its path.", one_string_schema("digest", "64-character sha256 digest"), Risk::Act))
}

fn flow_ui_manifest() -> ServiceManifest {
    ServiceManifest::new(
        UI_SERVICE_ID,
        "Flow UI",
        "Navigation for the visible flow-ui source/canvas shell. Focus an instance or select one graph node.",
    )
    .with_tool(tool(
        "focus",
        "Attach flow-ui to an existing instance.",
        one_string_schema("instance", "instance id"),
        Risk::Act,
    ))
    .with_tool(tool(
        "select",
        "Select one node in the attached flow.",
        one_string_schema("node", "node id"),
        Risk::Act,
    ))
}

fn definition_manifest(name: &str, definition: &FlowDefinition) -> ServiceManifest {
    let graph = definition.graph.as_ref();
    let label = graph.map(|graph| graph.label.as_str()).unwrap_or(name);
    let brief = graph.map(|graph| graph.brief.as_str()).unwrap_or_default();
    definition_manifest_from_schema(name, label, brief, &definition.tools)
}

fn definition_manifest_from_schema(
    name: &str,
    label: &str,
    flow_brief: &str,
    schema: &ToolSchema,
) -> ServiceManifest {
    let mut signatures = String::new();
    for tool in schema.tools.iter().take(62) {
        let fields = tool
            .result_fields
            .iter()
            .map(|(name, ty)| format!("{name}:{}", ty.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        signatures.push_str(&format!("\n{}{} -> {{{fields}}}", tool.name, tool.parameters));
    }
    let brief = bounded(
        format!("{}\nTools:{}", flow_brief.trim(), signatures),
        makepad_ai_services::wire::MAX_BRIEF_BYTES,
    );
    let mut manifest = ServiceManifest::new(
        per_flow_service_id(name),
        bounded(nonempty(label, name).to_string(), 48),
        brief,
    );
    for flow_tool in schema.tools.iter().take(62) {
        if !wire_ident(&flow_tool.name)
            || flow_tool.name == "status"
            || flow_tool.name == "outputs"
        {
            continue;
        }
        manifest = manifest.with_tool(tool(
            &flow_tool.name,
            &bounded(
                nonempty(&flow_tool.description, "Run this flow projection.").to_string(),
                makepad_ai_services::wire::MAX_DESCRIPTION_BYTES,
            ),
            &schema_with_instance(&flow_tool.parameters),
            Risk::Act,
        ));
    }
    manifest
        .with_topic(TopicDef::new(
            "run",
            "This flow's run events including progress; filter with {run_id}; final on run.finished.",
        ))
        .with_tool(tool(
            "status",
            "List this flow's live instances and their current state.",
            empty_schema(),
            Risk::Read,
        ))
        .with_tool(tool(
            "outputs",
            "Read one instance's last outputs and scratch media paths.",
            one_string_schema("instance", "instance id; defaults to this service's instance"),
            Risk::Read,
        ))
}

fn run_flows_call(
    client: &ClientHandle,
    call: &ServiceCall,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<WorkerMessage>,
    epoch: u64,
    service: &str,
) -> ToolResult {
    match call.tool.as_str() {
        "list" => match call_client(client, FlowClient::flows) {
            Ok(rows) => ToolResult::ok(
                &call.call_id,
                render_flow_list(&rows),
                format!("{} flows", rows.len()),
            )
            .with_data(rows.serialize_json()),
            Err(error) => client_error_result(&call.call_id, error),
        },
        "templates" => match call_client(client, FlowClient::templates) {
            Ok(templates) => ToolResult::ok(
                &call.call_id,
                render_templates(&templates),
                format!("{} flow templates", templates.len()),
            )
            .with_data(templates.serialize_json()),
            Err(error) => client_error_result(&call.call_id, error),
        },
        "models" => {
            let fields = match call_fields(call) {
                Ok(fields) => fields,
                Err(result) => return result,
            };
            let domain = string_field(&fields, "domain").map(str::to_string);
            match call_client(client, |client| client.models(domain.as_deref())) {
                Ok(models) => ToolResult::ok(
                    &call.call_id,
                    render_models(&models),
                    format!("{} fleet models", models.models.len()),
                )
                .with_data(models.serialize_json()),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "assets" => {
            let fields = match call_fields(call) {
                Ok(fields) => fields,
                Err(result) => return result,
            };
            let query = string_field(&fields, "q").unwrap_or("").to_string();
            let namespace = string_field(&fields, "ns").unwrap_or("flows").to_string();
            let limit = field(&fields, "limit")
                .and_then(Json::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(50);
            match call_client(client, |client| client.assets(&query, Some(&namespace), limit)) {
                Ok(response) => {
                    let text = if response.assets.is_empty() {
                        "No matching assets.".to_string()
                    } else {
                        response
                            .assets
                            .iter()
                            .map(|asset| {
                                format!(
                                    "{} · {} · {} · {}",
                                    asset.title,
                                    asset.kind,
                                    asset.namespace,
                                    asset.alias.as_deref().unwrap_or(&asset.id)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    ToolResult::ok(
                        &call.call_id,
                        text,
                        format!("{} assets", response.assets.len()),
                    )
                    .with_data(response.serialize_json())
                }
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "create" => {
            let fields = match call_fields(call) {
                Ok(fields) => fields,
                Err(result) => return result,
            };
            let name = match string_field(&fields, "name") {
                Some(name) => name.to_string(),
                None => return refused_missing(call, "name"),
            };
            let template = match string_field(&fields, "template") {
                Some(template) => template.to_string(),
                None => return refused_missing(call, "template"),
            };
            match call_client(client, |client| {
                client.create_from_template(&name, &template)
            }) {
                Ok(response) => ToolResult::ok(
                    &call.call_id,
                    render_write(&name, response.revision, &response.graph),
                    format!("created {name} from {template}"),
                )
                .with_data(response.serialize_json()),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "nodes" => match call_client(client, FlowClient::nodes) {
            Ok(nodes) => ToolResult::ok(
                &call.call_id,
                render_nodes(&nodes),
                "read node catalog",
            )
            .with_data(nodes.to_json()),
            Err(error) => client_error_result(&call.call_id, error),
        },
        "read" => {
            let name = match required_string(call, "name") {
                Ok(name) => name,
                Err(result) => return result,
            };
            match call_client(client, |client| client.flow(&name)) {
                Ok(definition) => ToolResult::ok(
                    &call.call_id,
                    render_read(&name, &definition),
                    format!("read {name}"),
                )
                .with_data(definition.serialize_json()),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "write" => {
            let fields = match call_fields(call) {
                Ok(fields) => fields,
                Err(result) => return result,
            };
            let name = match string_field(&fields, "name") {
                Some(name) => name.to_string(),
                None => return refused_missing(call, "name"),
            };
            let source = match string_field(&fields, "source") {
                Some(source) => source.to_string(),
                None => return refused_missing(call, "source"),
            };
            match call_client(client, |client| client.put_source(&name, &source)) {
                Ok(response) => ToolResult::ok(
                    &call.call_id,
                    render_write(&name, response.revision, &response.graph),
                    format!("wrote {name} r{}", response.revision),
                )
                .with_data(response.serialize_json()),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "delete" => {
            let name = match required_string(call, "name") {
                Ok(name) => name,
                Err(result) => return result,
            };
            match call_client(client, |client| client.delete(&name)) {
                Ok(()) => ToolResult::ok(
                    &call.call_id,
                    format!("deleted flow {name}"),
                    format!("deleted {name}"),
                ),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "instances" => match call_client(client, |client| client.instances_json(None, false)) {
            Ok(rows) => ToolResult::ok(
                &call.call_id,
                render_instances(&rows),
                "listed instances",
            )
            .with_data(rows.to_json()),
            Err(error) => client_error_result(&call.call_id, error),
        },
        "instance" => {
            let id = match required_string(call, "id") {
                Ok(id) => id,
                Err(result) => return result,
            };
            match call_client(client, |client| client.instance_json(&id)) {
                Ok(instance) => ToolResult::ok(
                    &call.call_id,
                    render_instance(&instance),
                    format!("read {id}"),
                )
                .with_data(instance.to_json()),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "start" => start_instance(client, call),
        "stop" => {
            let id = match required_string(call, "id") {
                Ok(id) => id,
                Err(result) => return result,
            };
            match call_client(client, |client| client.delete_instance(&id)) {
                Ok(()) => ToolResult::ok(
                    &call.call_id,
                    format!("stopped instance {id}"),
                    format!("stopped {id}"),
                ),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "send" | "answer" => put_one_input(client, call),
        "waiting" => match call_client(client, |client| client.instances_json(None, true)) {
            Ok(rows) => ToolResult::ok(
                &call.call_id,
                render_waiting(&rows),
                "listed waiting instances",
            )
            .with_data(rows.to_json()),
            Err(error) => client_error_result(&call.call_id, error),
        },
        "run" => {
            let fields = match call_fields(call) {
                Ok(fields) => fields,
                Err(result) => return result,
            };
            let id = match string_field(&fields, "id") {
                Some(id) => id.to_string(),
                None => return refused_missing(call, "id"),
            };
            let outputs = match optional_string_array(&fields, "outputs") {
                Ok(outputs) => outputs,
                Err(message) => return ToolResult::refused(&call.call_id, message),
            };
            start_and_wait(client, call, &id, outputs, cancel, tx, epoch, service)
        }
        "status" => {
            let run_id = match required_string(call, "run_id") {
                Ok(run_id) => run_id,
                Err(result) => return result,
            };
            match call_client(client, |client| client.run_json(&run_id)) {
                Ok(run) => ToolResult::ok(
                    &call.call_id,
                    render_run_row(&run),
                    format!("run {run_id}"),
                )
                .with_data(run.to_json()),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "cancel" => {
            let run_id = match required_string(call, "run_id") {
                Ok(run_id) => run_id,
                Err(result) => return result,
            };
            match call_client(client, |client| client.cancel_run(&run_id)) {
                Ok(()) => ToolResult::ok(
                    &call.call_id,
                    format!("cancelled run {run_id}"),
                    format!("cancelled {run_id}"),
                ),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "outputs" => {
            let id = match required_string(call, "id") {
                Ok(id) => id,
                Err(result) => return result,
            };
            match call_client(client, |client| client.instance_json(&id)) {
                Ok(instance) => output_result(client, &call.call_id, &id, instance),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "watch" => {
            let id = match required_string(call, "id") {
                Ok(id) => id,
                Err(result) => return result,
            };
            match call_client(client, |client| client.instance_json(&id)) {
                Ok(instance) => ToolResult::ok(
                    &call.call_id,
                    format!("watching instance {id}"),
                    format!("watching {id}"),
                )
                .with_data(instance.to_json())
                .with_disposition(Disposition::Continue)
                .with_subscription(
                    SubscriptionRequest::new("instance").with_filter(
                        Json::Obj(vec![("instance".into(), Json::Str(id))]).to_json(),
                    ),
                ),
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        "save" => {
            let digest = match required_string(call, "digest") {
                Ok(digest) => digest,
                Err(result) => return result,
            };
            match materialize(client, &digest, None) {
                Ok(path) => {
                    let path = path.to_string_lossy().to_string();
                    ToolResult::ok(
                        &call.call_id,
                        format!("saved scratch value {digest} at {path}; flow values are scratch and may expire"),
                        "saved scratch value",
                    )
                    .with_data(Json::Obj(vec![("digest".into(), Json::Str(digest)), ("path".into(), Json::Str(path))]).to_json())
                }
                Err(error) => client_error_result(&call.call_id, error),
            }
        }
        other => ToolResult::refused(
            &call.call_id,
            format!("flows has no tool `{other}`"),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_definition_call(
    client: &ClientHandle,
    service_id: &str,
    flow_name: &str,
    definition: &FlowDefinition,
    instances: &Arc<Mutex<BTreeMap<String, String>>>,
    call: &ServiceCall,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<WorkerMessage>,
    epoch: u64,
) -> ToolResult {
    if call.tool == "status" {
        return match call_client(client, |client| client.instances_json(Some(flow_name), false)) {
            Ok(rows) => ToolResult::ok(
                &call.call_id,
                render_instances(&rows),
                format!("{} instances", flow_name),
            )
            .with_data(rows.to_json()),
            Err(error) => client_error_result(&call.call_id, error),
        };
    }
    if call.tool == "outputs" {
        let fields = match call_fields(call) {
            Ok(fields) => fields,
            Err(result) => return result,
        };
        let instance = string_field(&fields, "instance")
            .map(str::to_string)
            .or_else(|| instances.lock().ok()?.get(service_id).cloned());
        let Some(instance) = instance else {
            return ToolResult::refused(
                &call.call_id,
                "outputs needs `instance` before this service has run",
            );
        };
        return match call_client(client, |client| client.instance_json(&instance)) {
            Ok(value) => output_result(client, &call.call_id, &instance, value),
            Err(error) => client_error_result(&call.call_id, error),
        };
    }
    let Some(graph) = definition.graph.as_ref() else {
        return ToolResult::unavailable(&call.call_id, "the flow has no evaluated graph");
    };
    let Some(entry) = graph.tools.iter().find(|entry| entry.name == call.tool) else {
        return ToolResult::refused(
            &call.call_id,
            format!("{} has no tool `{}`", flow_name, call.tool),
        );
    };
    let fields = match call_fields(call) {
        Ok(fields) => fields,
        Err(result) => return result,
    };
    let explicit = string_field(&fields, "instance").map(str::to_string);
    let instance = explicit
        .or_else(|| instances.lock().ok()?.get(service_id).cloned())
        .or_else(|| {
            let created = call_client(client, |client| {
                client.create_instance_json(flow_name, &Json::Obj(Vec::new()))
            })
            .ok()?;
            json_string_field(&created, &["instance", "id"])
        });
    let Some(instance) = instance else {
        return ToolResult::failed(&call.call_id, "could not create the flow instance");
    };
    if let Ok(mut map) = instances.lock() {
        map.insert(service_id.to_string(), instance.clone());
    }
    let inputs = match args_to_inputs(graph, entry, &fields) {
        Ok(inputs) => inputs,
        Err(message) => return ToolResult::refused(&call.call_id, message),
    };
    if !matches!(&inputs, Json::Obj(fields) if fields.is_empty()) {
        if let Err(error) =
            call_client(client, |client| client.put_inputs_json(&instance, "chat", &inputs))
        {
            return client_error_result(&call.call_id, error);
        }
    }
    start_and_wait(
        client,
        call,
        &instance,
        Some(entry.outputs.clone()),
        cancel,
        tx,
        epoch,
        service_id,
    )
}

fn start_instance(client: &ClientHandle, call: &ServiceCall) -> ToolResult {
    let fields = match call_fields(call) {
        Ok(fields) => fields,
        Err(result) => return result,
    };
    let name = match string_field(&fields, "name") {
        Some(name) => name.to_string(),
        None => return refused_missing(call, "name"),
    };
    let mut request = Vec::new();
    if let Some(label) = field(&fields, "label") {
        request.push(("label".into(), label.clone()));
    }
    if let Some(inputs) = field(&fields, "inputs") {
        let inputs = match normalize_inputs_for_flow(client, &name, inputs) {
            Ok(inputs) => inputs,
            Err(error) => return client_error_result(&call.call_id, error),
        };
        request.push(("inputs".into(), inputs));
    }
    match call_client(client, |client| {
        client.create_instance_json(&name, &Json::Obj(request))
    }) {
        Ok(instance) => {
            let id = json_string_field(&instance, &["instance", "id"])
                .unwrap_or_else(|| "created".to_string());
            ToolResult::ok(
                &call.call_id,
                format!("started {name} instance {id}"),
                format!("started {id}"),
            )
            .with_data(instance.to_json())
        }
        Err(error) => client_error_result(&call.call_id, error),
    }
}

fn normalize_inputs_for_flow(
    client: &ClientHandle,
    flow_name: &str,
    inputs: &Json,
) -> Result<Json, ClientError> {
    let definition = call_client(client, |client| client.flow(flow_name))?;
    let graph = definition.graph.ok_or_else(|| {
        ClientError::Protocol(format!("flow `{flow_name}` has no evaluated graph"))
    })?;
    let Json::Obj(nodes) = inputs else {
        return Err(ClientError::Protocol("instance inputs must be an object".into()));
    };
    let mut normalized = Vec::with_capacity(nodes.len());
    for (node_id, value) in nodes {
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == *node_id)
            .ok_or_else(|| ClientError::Protocol(format!("flow has no node `{node_id}`")))?;
        let Json::Obj(ports) = value else {
            return Err(ClientError::Protocol(format!(
                "inputs for `{node_id}` must be an object"
            )));
        };
        let mut normalized_ports = Vec::with_capacity(ports.len());
        for (port_name, value) in ports {
            let ty = instance_port_type(node, port_name).ok_or_else(|| {
                    ClientError::Protocol(format!(
                        "node `{node_id}` has no port `{port_name}`"
                    ))
                })?;
            let value = wire_input_value(ty, value.clone()).map_err(ClientError::Protocol)?;
            normalized_ports.push((port_name.clone(), value));
        }
        normalized.push((node_id.clone(), Json::Obj(normalized_ports)));
    }
    Ok(Json::Obj(normalized))
}

fn put_one_input(client: &ClientHandle, call: &ServiceCall) -> ToolResult {
    let fields = match call_fields(call) {
        Ok(fields) => fields,
        Err(result) => return result,
    };
    let id = match string_field(&fields, "id") {
        Some(id) => id.to_string(),
        None => return refused_missing(call, "id"),
    };
    let node = match string_field(&fields, "node") {
        Some(node) => node.to_string(),
        None => return refused_missing(call, "node"),
    };
    let Some(value) = field(&fields, "value").cloned() else {
        return refused_missing(call, "value");
    };
    let requested_port = string_field(&fields, "port").filter(|_| call.tool == "send");
    let (port, ty) = match instance_value_port(client, &id, &node, requested_port) {
        Ok(port) => port,
        Err(error) => return client_error_result(&call.call_id, error),
    };
    let value = match wire_input_value(ty, value) {
        Ok(value) => value,
        Err(message) => return ToolResult::refused(&call.call_id, message),
    };
    let body = Json::Obj(vec![(
        node.clone(),
        Json::Obj(vec![(port.clone(), value)]),
    )]);
    match call_client(client, |client| client.put_inputs_json(&id, "chat", &body)) {
        Ok(response) => ToolResult::ok(
            &call.call_id,
            format!("set {id} {node}.{port}"),
            format!("set {node}.{port}"),
        )
        .with_data(response.to_json()),
        Err(ClientError::Http { status: 409, .. }) if call.tool == "answer" => {
            ToolResult::failed(&call.call_id, format!("instance {id} is not waiting at {node}"))
        }
        Err(error) => client_error_result(&call.call_id, error),
    }
}

fn instance_value_port(
    client: &ClientHandle,
    instance_id: &str,
    node_id: &str,
    requested_port: Option<&str>,
) -> Result<(String, PortType), ClientError> {
    let instance = call_client(client, |client| client.instance_json(instance_id))?;
    let flow = json_string_field(&instance, &["flow"]).ok_or_else(|| {
        ClientError::Protocol("instance detail has no flow name".to_string())
    })?;
    let definition = call_client(client, |client| client.flow(&flow))?;
    let node = definition
        .graph
        .as_ref()
        .and_then(|graph| graph.nodes.iter().find(|node| node.id == node_id))
        .ok_or_else(|| ClientError::Protocol(format!("flow has no node `{node_id}`")))?;
    if let Some(port_name) = requested_port {
        let ty = instance_port_type(node, port_name).ok_or_else(|| {
                ClientError::Protocol(format!("node `{node_id}` has no port `{port_name}`"))
            })?;
        return Ok((port_name.to_string(), ty));
    }
    if node.outputs.len() != 1 {
        return Err(ClientError::Protocol(format!(
            "node `{node_id}` has {} output value ports; specify `port`",
            node.outputs.len()
        )));
    }
    Ok((node.outputs[0].name.clone(), node.outputs[0].ty))
}

#[allow(clippy::too_many_arguments)]
fn start_and_wait(
    client: &ClientHandle,
    call: &ServiceCall,
    instance: &str,
    outputs: Option<Vec<String>>,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<WorkerMessage>,
    epoch: u64,
    service: &str,
) -> ToolResult {
    start_and_wait_for(
        client, call, instance, outputs, cancel, tx, epoch, service, RUN_WAIT,
    )
}

trait RunWaitClient {
    fn start_run(&self, instance: &str, outputs: Option<&[String]>) -> Result<Json, ClientError>;
    fn run(&self, run_id: &str) -> Result<Json, ClientError>;
    fn instance(&self, instance: &str) -> Result<Json, ClientError>;
    fn cancel(&self, run_id: &str) -> Result<(), ClientError>;
    fn finished(&self, call_id: &str, instance: &str, row: Json) -> ToolResult;
}

impl RunWaitClient for ClientHandle {
    fn start_run(&self, instance: &str, outputs: Option<&[String]>) -> Result<Json, ClientError> {
        call_client(self, |client| client.start_run_json(instance, outputs))
    }

    fn run(&self, run_id: &str) -> Result<Json, ClientError> {
        call_client(self, |client| client.run_json(run_id))
    }

    fn instance(&self, instance: &str) -> Result<Json, ClientError> {
        call_client(self, |client| client.instance_json(instance))
    }

    fn cancel(&self, run_id: &str) -> Result<(), ClientError> {
        call_client(self, |client| client.cancel_run(run_id))
    }

    fn finished(&self, call_id: &str, instance: &str, row: Json) -> ToolResult {
        run_output_result(self, call_id, instance, row)
    }
}

#[allow(clippy::too_many_arguments)]
fn start_and_wait_for<C: RunWaitClient>(
    client: &C,
    call: &ServiceCall,
    instance: &str,
    outputs: Option<Vec<String>>,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<WorkerMessage>,
    epoch: u64,
    service: &str,
    max_wait: Duration,
) -> ToolResult {
    let started = match client.start_run(instance, outputs.as_deref()) {
        Ok(started) => started,
        Err(error) => return client_error_result(&call.call_id, error),
    };
    let Some(run_id) = json_string_field(&started, &["run_id", "id"]) else {
        return ToolResult::failed(&call.call_id, "run start response has no run_id");
    };
    wait_for_run(
        client,
        &call.call_id,
        instance,
        &run_id,
        cancel,
        tx,
        epoch,
        service,
        max_wait,
    )
}

#[allow(clippy::too_many_arguments)]
fn wait_for_run<C: RunWaitClient>(
    client: &C,
    call_id: &str,
    instance: &str,
    run_id: &str,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<WorkerMessage>,
    epoch: u64,
    service: &str,
    max_wait: Duration,
) -> ToolResult {
    let deadline = Instant::now() + max_wait;
    let mut previous = BTreeMap::new();
    loop {
        if cancel.load(Ordering::Acquire) {
            let _ = client.cancel(run_id);
            return ToolResult::cancelled(call_id);
        }
        let row = match client.run(run_id) {
            Ok(row) => row,
            Err(error) => return client_error_result(call_id, error),
        };
        let states = node_states(&row);
        if states != previous {
            let (note, permille) = progress_note(&states);
            let _ = tx.send(WorkerMessage::Progress {
                epoch,
                service: service.to_string(),
                call_id: call_id.to_string(),
                note,
                permille,
            });
            SignalToUI::set_ui_signal();
            previous = states;
        }
        let state = json_string_field(&row, &["state"]).unwrap_or_default();
        if terminal_state(&state) {
            return client.finished(call_id, instance, row);
        }
        if state.eq_ignore_ascii_case("waiting") {
            let waiting = client
                .instance(instance)
                .ok()
                .and_then(|instance| instance.get("waiting").cloned());
            return continuing_run_result(call_id, instance, run_id, &state, waiting);
        }
        if Instant::now() >= deadline {
            return continuing_run_result(call_id, instance, run_id, &state, None);
        }
        wait_for_next_poll();
    }
}

fn continuing_run_result(
    call_id: &str,
    instance: &str,
    run_id: &str,
    state: &str,
    waiting: Option<Json>,
) -> ToolResult {
    let state = nonempty(state, "running").to_ascii_lowercase();
    let mut fields = vec![
        ("run_id".into(), Json::Str(run_id.to_string())),
        ("instance".into(), Json::Str(instance.to_string())),
        ("state".into(), Json::Str(state.clone())),
    ];
    let text = if let Some(waiting) = waiting {
        let question = waiting
            .get("question")
            .and_then(Json::as_str)
            .unwrap_or("answer required")
            .to_string();
        fields.push(("waiting".into(), waiting));
        format!("run {run_id} is waiting · {}", one_line(&question))
    } else {
        format!("run {run_id} is {state}; its result will arrive as a message")
    };
    ToolResult::ok(call_id, text, format!("run {run_id} {state}"))
        .with_data(Json::Obj(fields).to_json())
        .with_disposition(Disposition::Continue)
        .with_subscription(
            SubscriptionRequest::new("run").with_filter(
                Json::Obj(vec![("run_id".into(), Json::Str(run_id.to_string()))]).to_json(),
            ),
        )
}

// flow-ui's HTTP client and embedded host are native-only. This worker wait
// is correspondingly never linked into a wasm app.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)]
fn wait_for_next_poll() {
    std::thread::sleep(RUN_POLL);
}

fn output_result(
    client: &ClientHandle,
    call_id: &str,
    instance: &str,
    row: Json,
) -> ToolResult {
    let rendered = prepare_outputs(client, row.get("outputs").unwrap_or(&row));
    let state = json_string_field(&row, &["state"]).unwrap_or_else(|| "outputs".into());
    let mut text = format!("instance {instance} · {state}");
    if !rendered.text.is_empty() {
        text.push('\n');
        text.push_str(&rendered.text);
    }
    if rendered.has_media {
        text.push_str("\nMedia paths are scratch values and may expire; publish or copy them to keep them.");
    }
    let data = Json::Obj(vec![
        ("result".into(), row),
        ("paths".into(), Json::Obj(rendered.paths)),
    ])
    .to_json();
    ToolResult::ok(call_id, text, format!("{instance} {state}")).with_data(data)
}

fn run_output_result(
    client: &ClientHandle,
    call_id: &str,
    instance: &str,
    row: Json,
) -> ToolResult {
    let state = json_string_field(&row, &["state"])
        .unwrap_or_else(|| "finished".into())
        .to_ascii_lowercase();
    let rendered = prepare_outputs(client, row.get("outputs").unwrap_or(&row));
    let mut text = format!("instance {instance} · {state}");
    if !rendered.text.is_empty() {
        text.push('\n');
        text.push_str(&rendered.text);
    }
    if rendered.has_media {
        text.push_str(
            "\nMedia paths are scratch values and may expire; publish or copy them to keep them.",
        );
    }
    let data = Json::Obj(vec![
        ("run".into(), row),
        ("paths".into(), Json::Obj(rendered.paths)),
    ])
    .to_json();
    match state.as_str() {
        "failed" => ToolResult::failed(call_id, text).with_data(data),
        "cancelled" => ToolResult::cancelled(call_id).with_data(data),
        _ => ToolResult::ok(call_id, text, format!("{instance} {state}")).with_data(data),
    }
}

struct RenderedOutputs {
    text: String,
    paths: Vec<(String, Json)>,
    has_media: bool,
}

fn prepare_outputs(client: &ClientHandle, outputs: &Json) -> RenderedOutputs {
    let mut refs = Vec::new();
    collect_value_refs(outputs, "output", &mut refs);
    let metadata = render_run_outputs_metadata(outputs);
    let mut lines = Vec::new();
    let mut paths = Vec::new();
    let mut has_media = false;
    for value in refs {
        if textual_type(&value.ty) {
            match call_client(client, |client| client.value(&value.digest)) {
                Ok(fetched) => {
                    let text = String::from_utf8_lossy(&fetched.bytes);
                    lines.push(format!(
                        "{} · {} · {}",
                        value.name,
                        value.digest,
                        bounded(text.into_owned(), MAX_INLINE_VALUE)
                    ));
                }
                Err(_) => lines.push(format!(
                    "{} · {} · {} bytes",
                    value.name, value.digest, value.bytes
                )),
            }
        } else {
            has_media = true;
            match materialize(client, &value.digest, Some(&value.content_type)) {
                Ok(path) => {
                    let path = path.to_string_lossy().to_string();
                    lines.push(format!(
                        "{} · {} · {} · {} bytes · {}",
                        value.name, value.digest, value.content_type, value.bytes, path
                    ));
                    paths.push((value.digest, Json::Str(path)));
                }
                Err(error) => lines.push(format!(
                    "{} · {} · {} · {} bytes · path unavailable ({error})",
                    value.name, value.digest, value.content_type, value.bytes
                )),
            }
        }
    }
    if lines.is_empty() {
        lines.push(if metadata.is_empty() {
            bounded(outputs.to_json(), MAX_INLINE_VALUE)
        } else {
            metadata
        });
    }
    RenderedOutputs {
        text: lines.join("\n"),
        paths,
        has_media,
    }
}

#[derive(Clone)]
struct OutputRef {
    name: String,
    ty: String,
    digest: String,
    content_type: String,
    bytes: u64,
}

fn collect_value_refs(value: &Json, name: &str, out: &mut Vec<OutputRef>) {
    if let Json::Obj(fields) = value {
        if let Some(digest) = string_field(fields, "digest") {
            let ty = string_field(fields, "type")
                .or_else(|| string_field(fields, "ty"))
                .unwrap_or("bytes")
                .to_string();
            let content_type = string_field(fields, "content_type")
                .unwrap_or("application/octet-stream")
                .to_string();
            let bytes = field(fields, "bytes").and_then(Json::as_u64).unwrap_or(0);
            out.push(OutputRef {
                name: name.to_string(),
                ty,
                digest: digest.to_string(),
                content_type,
                bytes,
            });
            return;
        }
        if let (Some(port), Some(value)) = (string_field(fields, "port"), field(fields, "value")) {
            collect_value_refs(value, port, out);
            return;
        }
        for (key, child) in fields {
            collect_value_refs(child, key, out);
        }
    } else if let Json::Arr(values) = value {
        if let [Json::Str(output_name), output] = values.as_slice() {
            collect_value_refs(output, output_name, out);
            return;
        }
        for (index, child) in values.iter().enumerate() {
            collect_value_refs(child, &format!("{name}[{index}]"), out);
        }
    }
}

fn render_run_outputs_metadata(outputs: &Json) -> String {
    let mut refs = Vec::new();
    collect_value_refs(outputs, "output", &mut refs);
    refs.into_iter()
        .map(|value| {
            format!(
                "{} · {} · {} · {} bytes",
                value.name, value.digest, value.content_type, value.bytes
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn materialize(
    client: &ClientHandle,
    digest: &str,
    content_type: Option<&str>,
) -> Result<PathBuf, ClientError> {
    let fetched = call_client(client, |client| client.value(digest))?;
    let content_type = content_type
        .filter(|value| !value.is_empty() && *value != "application/octet-stream")
        .unwrap_or(&fetched.content_type);
    let extension = extension_for(content_type);
    let directory = makepad_flow::embed::default_root().join("values");
    std::fs::create_dir_all(&directory).map_err(|error| ClientError::Io {
        op: "create saved flow value directory",
        kind: error.kind(),
    })?;
    let path = directory.join(format!("{digest}.{extension}"));
    std::fs::write(&path, fetched.bytes).map_err(|error| ClientError::Io {
        op: "write saved flow value",
        kind: error.kind(),
    })?;
    Ok(path)
}

fn extension_for(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or("").trim() {
        "text/plain" => "txt",
        "application/json" => "json",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "audio/wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "model/gltf-binary" => "glb",
        "model/gltf+json" => "gltf",
        _ => "bin",
    }
}

fn args_to_inputs(
    graph: &Graph,
    entry: &ToolEntry,
    fields: &[(String, Json)],
) -> Result<Json, String> {
    let mut inputs = Vec::new();
    for input_id in &entry.inputs {
        let Some(value) = field(fields, input_id) else {
            continue;
        };
        let node = graph
            .nodes
            .iter()
            .find(|node| &node.id == input_id)
            .ok_or_else(|| format!("tool input node `{input_id}` is missing"))?;
        let port = single_output_port(node)?;
        let value = wire_input_value(port.ty, value.clone())?;
        inputs.push((
            input_id.clone(),
            Json::Obj(vec![(port.name.clone(), value)]),
        ));
    }
    Ok(Json::Obj(inputs))
}

fn single_output_port(node: &Node) -> Result<&makepad_flow::Port, String> {
    match node.outputs.as_slice() {
        [port] => Ok(port),
        ports => Err(format!(
            "input node `{}` has {} output ports",
            node.id,
            ports.len()
        )),
    }
}

fn wire_input_value(ty: PortType, value: Json) -> Result<Json, String> {
    if value.get("type").and_then(Json::as_str).is_some()
        && (value.get("text").is_some()
            || value.get("json").is_some()
            || value.get("digest").is_some())
    {
        return Ok(value);
    }
    let ty_name = ty.as_str().to_string();
    let payload = match ty {
        PortType::Text => (
            "text".to_string(),
            Json::Str(value.as_str().map(str::to_string).unwrap_or_else(|| value.to_json())),
        ),
        PortType::Json | PortType::List => ("json".to_string(), value),
        PortType::Image
        | PortType::Audio
        | PortType::Video
        | PortType::Mesh
        | PortType::Bytes => {
            let digest = value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.get("digest").and_then(Json::as_str).map(str::to_string))
                .ok_or_else(|| format!("{} input requires a value digest", ty.as_str()))?;
            ("digest".to_string(), Json::Str(digest))
        }
    };
    Ok(Json::Obj(vec![("type".into(), Json::Str(ty_name)), payload]))
}

fn instance_port_type(node: &Node, port_name: &str) -> Option<PortType> {
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

fn render_flow_list(rows: &[FlowSummary]) -> String {
    if rows.is_empty() {
        return "no flow definitions".to_string();
    }
    rows.iter()
        .map(|row| {
            format!(
                "{} · {} · {} · canonical={} · instances={}",
                row.name, row.label, row.state, row.canonical, row.instances
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_templates(templates: &[TemplateSummary]) -> String {
    if templates.is_empty() {
        return "no flow templates".to_string();
    }
    templates
        .iter()
        .map(|template| {
            let inputs = template
                .inputs
                .iter()
                .map(|(name, ty)| format!("{name}:{ty}"))
                .collect::<Vec<_>>()
                .join(", ");
            let outputs = template
                .outputs
                .iter()
                .map(|(name, ty)| format!("{name}:{ty}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{} · {} · {} · inputs [{}] · outputs [{}]",
                template.name,
                template.label,
                one_line(&template.brief),
                inputs,
                outputs
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_models(response: &ModelsResponse) -> String {
    if response.models.is_empty() {
        return "no fleet models available".to_string();
    }
    response
        .models
        .iter()
        .map(|model| {
            let availability = if model.available {
                "available"
            } else {
                "unavailable"
            };
            let gated = if model.gated { " · gated" } else { "" };
            format!(
                "{} · {} · {} · {} · {}{}",
                model.id, model.domain, model.backend, model.node, availability, gated
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_nodes(root: &Json) -> String {
    let Some(types) = root.get("types").and_then(Json::as_arr) else {
        return bounded(root.to_json(), MAX_READ_TEXT);
    };
    let mut lines = Vec::new();
    for ty in types {
        let Some(fields) = object_fields(ty) else {
            continue;
        };
        let name = string_field(fields, "type_name").unwrap_or("?");
        let kind = string_field(fields, "kind").unwrap_or("?");
        let domain = string_field(fields, "domain")
            .map(|domain| format!("/{domain}"))
            .unwrap_or_default();
        lines.push(format!("{name} · {kind}{domain}"));
        if let Some(ports) = field(fields, "ports").and_then(object_fields) {
            let inputs = field(ports, "in").map(render_ports).unwrap_or_default();
            let outputs = field(ports, "out").map(render_ports).unwrap_or_default();
            lines.push(format!("  ports: in [{inputs}] · out [{outputs}]"));
        }
        if let Some(params) = field(fields, "params").and_then(Json::as_arr) {
            for param in params {
                let Some(param) = object_fields(param) else {
                    continue;
                };
                let param_name = string_field(param, "name").unwrap_or("?");
                let default = field(param, "default")
                    .map(Json::to_json)
                    .unwrap_or_else(|| "null".into());
                let doc = string_field(param, "doc").unwrap_or("");
                let range = field(param, "range")
                    .and_then(object_fields)
                    .map(render_range)
                    .unwrap_or_default();
                let separator = if doc.is_empty() { "" } else { " — " };
                lines.push(format!(
                    "  {param_name} = {default}{separator}{doc}{range}"
                ));
            }
        }
    }
    bounded(lines.join("\n"), MAX_READ_TEXT)
}

fn render_ports(value: &Json) -> String {
    value
        .as_arr()
        .unwrap_or(&[])
        .iter()
        .filter_map(object_fields)
        .map(|port| {
            format!(
                "{}: {}",
                string_field(port, "name").unwrap_or("?"),
                string_field(port, "ty")
                    .or_else(|| string_field(port, "type"))
                    .unwrap_or("?")
                    .to_ascii_lowercase()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_range(fields: &[(String, Json)]) -> String {
    let min = field(fields, "min").map(Json::to_json).unwrap_or_default();
    let max = field(fields, "max").map(Json::to_json).unwrap_or_default();
    let step = field(fields, "step")
        .filter(|value| !value.is_null())
        .map(|value| format!(" step {}", value.to_json()))
        .unwrap_or_default();
    if min.is_empty() || max.is_empty() {
        String::new()
    } else {
        format!(" [{min}..{max}{step}]")
    }
}

fn render_read(name: &str, definition: &FlowDefinition) -> String {
    let mut text = format!("{name} r{}\n{}", definition.revision, definition.source);
    if let Some(graph) = &definition.graph {
        text.push_str("\n\nNodes:\n");
        for node in &graph.nodes {
            text.push_str(&render_graph_node(node));
            text.push('\n');
        }
    }
    if let Some(error) = &definition.error {
        text.push_str(&format!("\nLast error: {error}"));
    }
    bounded_with_note(text, MAX_READ_TEXT, "\n…[flow source and summary truncated]")
}

fn render_graph_node(node: &Node) -> String {
    let inputs = node
        .inputs
        .iter()
        .map(|input| match &input.value {
            NodeInputValue::Edge(edge) => {
                format!("{}<={}.{}", input.port, edge.from_node, edge.from_port)
            }
            NodeInputValue::Literal(_) => format!("{}=<literal>", input.port),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let outputs = node
        .outputs
        .iter()
        .map(|port| format!("{}:{}", port.name, port.ty.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} · {} · {} · in [{}] · out [{}]",
        node.id, node.type_name, node.kind, inputs, outputs
    )
}

fn render_write(name: &str, revision: u64, graph: &Graph) -> String {
    let mut lines = vec![format!(
        "wrote {name} r{revision} · {} nodes · {} tools",
        graph.nodes.len(),
        graph.tools.len()
    )];
    lines.extend(graph.nodes.iter().map(render_graph_node));
    bounded(lines.join("\n"), MAX_READ_TEXT)
}

fn render_instances(root: &Json) -> String {
    let rows = root.as_arr().unwrap_or(&[]);
    if rows.is_empty() {
        return "no live instances".to_string();
    }
    rows.iter()
        .map(|row| {
            let id = json_string_field(row, &["instance", "id"]).unwrap_or_else(|| "?".into());
            let flow = json_string_field(row, &["flow"]).unwrap_or_else(|| "?".into());
            let label = json_string_field(row, &["label"]).unwrap_or_else(|| "-".into());
            let owner = json_string_field(row, &["owner"]).unwrap_or_else(|| "?".into());
            let state = json_string_field(row, &["state"]).unwrap_or_else(|| "?".into());
            let node = json_string_field(row, &["current_node", "node"])
                .unwrap_or_else(|| "-".into());
            let outputs = row
                .get("outputs")
                .map(|value| bounded(value.to_json(), 256))
                .unwrap_or_else(|| "-".into());
            format!("{id} · {flow} · {label} · {owner} · {state} · {node} · {outputs}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_waiting(root: &Json) -> String {
    let rows = root.as_arr().unwrap_or(&[]);
    if rows.is_empty() {
        return "no instances are waiting for an answer".to_string();
    }
    rows.iter()
        .map(|row| {
            let id =
                json_string_field(row, &["instance", "id"]).unwrap_or_else(|| "?".into());
            let flow = json_string_field(row, &["flow"]).unwrap_or_else(|| "?".into());
            let waiting = row.get("waiting").unwrap_or(row);
            let node =
                json_string_field(waiting, &["node", "current_node"]).unwrap_or_else(|| "?".into());
            let question =
                json_string_field(waiting, &["question"]).unwrap_or_else(|| "?".into());
            let ty = json_string_field(waiting, &["type", "ty"]).unwrap_or_else(|| "?".into());
            let options = waiting
                .get("options")
                .map(Json::to_json)
                .unwrap_or_else(|| "[]".into());
            format!("{id} · {flow} · {node} · {ty} · {question} · options={options}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_instance(root: &Json) -> String {
    let id = json_string_field(root, &["instance", "id"]).unwrap_or_else(|| "?".into());
    let flow = json_string_field(root, &["flow"]).unwrap_or_else(|| "?".into());
    let state = json_string_field(root, &["state"]).unwrap_or_else(|| "?".into());
    let inputs = root
        .get("inputs")
        .map(Json::to_json)
        .unwrap_or_else(|| "{}".into());
    let outputs = root
        .get("outputs")
        .map(Json::to_json)
        .unwrap_or_else(|| "{}".into());
    bounded(
        format!("{id} · {flow} · {state}\ninputs: {inputs}\noutputs: {outputs}"),
        MAX_READ_TEXT,
    )
}

fn render_run_row(root: &Json) -> String {
    let id = json_string_field(root, &["run_id", "id"]).unwrap_or_else(|| "?".into());
    let instance = json_string_field(root, &["instance"]).unwrap_or_else(|| "?".into());
    let state = json_string_field(root, &["state"]).unwrap_or_else(|| "?".into());
    let states = node_states(root)
        .into_iter()
        .map(|(node, state)| format!("{node}:{state}"))
        .collect::<Vec<_>>()
        .join(", ");
    bounded(
        format!("{id} · {instance} · {state}\nnodes: {states}"),
        MAX_READ_TEXT,
    )
}

fn render_context(
    context: &BridgeContext,
    subscription_count: usize,
    last_message_summary: &str,
) -> String {
    let mut lines = vec![format!(
        "flow={} revision={} canonical={}",
        context.flow.as_deref().unwrap_or("none"),
        context
            .revision
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".into()),
        context
            .canonical
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into())
    )];
    lines.push(format!(
        "instance={} state={} current_node={}",
        context.instance.as_deref().unwrap_or("none"),
        context.instance_state.as_deref().unwrap_or("unknown"),
        context.current_node.as_deref().unwrap_or("none")
    ));
    lines.push(format!(
        "selected_node={} view={}",
        context.selected_node.as_deref().unwrap_or("none"),
        nonempty(&context.open_view, "source")
    ));
    lines.push(format!(
        "subscriptions={} last_message={}",
        subscription_count,
        nonempty(last_message_summary, "none")
    ));
    if let Some(error) = &context.last_error {
        lines.push(format!("last_error={}", error.lines().next().unwrap_or(error)));
    }
    bounded(
        lines.join("\n"),
        makepad_ai_services::wire::MAX_CONTEXT_BYTES,
    )
}

fn node_states(root: &Json) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(nodes) = root.get("nodes").and_then(object_fields) else {
        return out;
    };
    for (node, value) in nodes {
        if let Some(state) = json_string_field(value, &["state"]) {
            out.insert(node.clone(), state);
        }
    }
    out
}

fn progress_note(states: &BTreeMap<String, String>) -> (String, u16) {
    if states.is_empty() {
        return ("run started".into(), 0);
    }
    let done = states
        .values()
        .filter(|state| terminal_node_state(state))
        .count();
    let active = states
        .iter()
        .find(|(_, state)| !terminal_node_state(state))
        .map(|(node, state)| format!("{node}: {state}"))
        .unwrap_or_else(|| "nodes finished".into());
    (active, ((done * 1000) / states.len()) as u16)
}

fn terminal_node_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "done" | "failed" | "skipped" | "cancelled"
    )
}

fn terminal_state(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "done" | "finished" | "failed" | "cancelled"
    )
}

fn textual_type(ty: &str) -> bool {
    matches!(
        ty.to_ascii_lowercase().as_str(),
        "text" | "json" | "list"
    )
}

fn client_error_result(call_id: &str, error: ClientError) -> ToolResult {
    match error {
        ClientError::Eval(error) => ToolResult::failed(call_id, error.to_string()),
        ClientError::Unauthorized => ToolResult::denied(call_id, "flow server denied access"),
        error if error.is_connection_loss() => ToolResult::unavailable(call_id, error.to_string()),
        error => ToolResult::failed(call_id, error.to_string()),
    }
}

fn call_client<T>(
    client: &ClientHandle,
    call: impl FnOnce(&FlowClient) -> Result<T, ClientError>,
) -> Result<T, ClientError> {
    let client = client
        .lock()
        .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))?;
    call(&client)
}

fn call_fields(call: &ServiceCall) -> Result<Vec<(String, Json)>, ToolResult> {
    match makepad_strict_json::parse_depth(call.args.as_bytes(), 32) {
        Ok(Json::Obj(fields)) => Ok(fields),
        Ok(_) => Err(ToolResult::refused(
            &call.call_id,
            format!("{}.{} arguments must be a JSON object", SERVICE_ID, call.tool),
        )),
        Err(error) => Err(ToolResult::refused(
            &call.call_id,
            format!("invalid arguments for {}.{}: {error}", SERVICE_ID, call.tool),
        )),
    }
}

fn required_string(call: &ServiceCall, name: &str) -> Result<String, ToolResult> {
    let fields = call_fields(call)?;
    string_field(&fields, name)
        .map(str::to_string)
        .ok_or_else(|| refused_missing(call, name))
}

fn refused_missing(call: &ServiceCall, name: &str) -> ToolResult {
    ToolResult::refused(
        &call.call_id,
        format!("{}.{} needs a `{name}`", SERVICE_ID, call.tool),
    )
}

fn optional_string_array(
    fields: &[(String, Json)],
    name: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = field(fields, name) else {
        return Ok(None);
    };
    let Some(values) = value.as_arr() else {
        return Err(format!("`{name}` must be an array of strings"));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("`{name}` must contain only strings"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn object_fields(value: &Json) -> Option<&[(String, Json)]> {
    match value {
        Json::Obj(fields) => Some(fields),
        _ => None,
    }
}

fn field<'a>(fields: &'a [(String, Json)], name: &str) -> Option<&'a Json> {
    fields
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

fn string_field<'a>(fields: &'a [(String, Json)], name: &str) -> Option<&'a str> {
    field(fields, name).and_then(Json::as_str)
}

fn json_string_field(value: &Json, names: &[&str]) -> Option<String> {
    let fields = object_fields(value)?;
    for name in names {
        let Some(value) = field(fields, name) else {
            continue;
        };
        if let Some(value) = value.as_str() {
            return Some(value.to_string());
        }
        if let Some(nested) = object_fields(value) {
            if let Some(value) = string_field(nested, "id")
                .or_else(|| string_field(nested, "instance"))
                .or_else(|| string_field(nested, "run_id"))
            {
                return Some(value.to_string());
            }
            if let [(variant, Json::Arr(fields))] = nested {
                if fields.is_empty() {
                    return Some(variant.to_ascii_lowercase());
                }
            }
        }
    }
    if let Some(nested) = field(fields, "instance").and_then(object_fields) {
        for name in names {
            if let Some(value) = string_field(nested, name) {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn schema_with_instance(parameters: &str) -> String {
    let Ok(Json::Obj(mut fields)) = makepad_strict_json::parse_depth(parameters.as_bytes(), 32)
    else {
        return empty_schema().to_string();
    };
    let mut found = false;
    if let Some(Json::Obj(properties)) = fields
        .iter_mut()
        .find_map(|(name, value)| (name == "properties").then_some(value))
    {
        if !properties.iter().any(|(name, _)| name == "instance") {
            properties.push((
                "instance".into(),
                Json::Obj(vec![
                    ("type".into(), Json::Str("string".into())),
                    (
                        "description".into(),
                        Json::Str("existing instance id; optional".into()),
                    ),
                ]),
            ));
        }
        found = true;
    }
    if !found {
        fields.push((
            "properties".into(),
            Json::Obj(vec![(
                "instance".into(),
                Json::Obj(vec![("type".into(), Json::Str("string".into()))]),
            )]),
        ));
    }
    Json::Obj(fields).to_json()
}

fn tool(name: &str, description: &str, parameters: &str, risk: Risk) -> ToolDef {
    ToolDef::new(name, description, parameters, risk)
}

fn empty_schema() -> &'static str {
    r#"{"type":"object","properties":{},"additionalProperties":false}"#
}

fn one_string_schema(name: &str, description: &str) -> &'static str {
    match (name, description) {
        ("name", "flow definition name") => r#"{"type":"object","properties":{"name":{"type":"string","description":"flow definition name"}},"required":["name"],"additionalProperties":false}"#,
        ("id", "instance id") => r#"{"type":"object","properties":{"id":{"type":"string","description":"instance id"}},"required":["id"],"additionalProperties":false}"#,
        ("run_id", "run id") => r#"{"type":"object","properties":{"run_id":{"type":"string","description":"run id"}},"required":["run_id"],"additionalProperties":false}"#,
        ("digest", "64-character sha256 digest") => r#"{"type":"object","properties":{"digest":{"type":"string","description":"64-character sha256 digest"}},"required":["digest"],"additionalProperties":false}"#,
        ("instance", _) => r#"{"type":"object","properties":{"instance":{"type":"string","description":"instance id"}},"required":["instance"],"additionalProperties":false}"#,
        ("node", _) => r#"{"type":"object","properties":{"node":{"type":"string","description":"node id"}},"required":["node"],"additionalProperties":false}"#,
        _ => empty_schema(),
    }
}

fn per_flow_service_id(name: &str) -> String {
    let clean: String = name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' {
                byte as char
            } else {
                '_'
            }
        })
        .collect();
    if clean == name && clean.len() <= 19 {
        return format!("flow_{clean}");
    }
    let mut hash = 0x811c9dc5u32;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    let stem: String = clean.chars().take(10).collect();
    format!("flow_{stem}_{hash:08x}")
}

fn wire_ident(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.bytes().next().is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn nonempty<'a>(text: &'a str, fallback: &'a str) -> &'a str {
    if text.trim().is_empty() {
        fallback
    } else {
        text
    }
}

fn bounded(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let suffix = "…";
    truncate_boundary(&mut text, max.saturating_sub(suffix.len()));
    text.push_str(suffix);
    text
}

fn bounded_with_note(mut text: String, max: usize, note: &str) -> String {
    if text.len() <= max {
        return text;
    }
    truncate_boundary(&mut text, max.saturating_sub(note.len()));
    text.push_str(note);
    text
}

fn truncate_boundary(text: &mut String, mut length: usize) {
    length = length.min(text.len());
    while length > 0 && !text.is_char_boundary(length) {
        length -= 1;
    }
    text.truncate(length);
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::ToolOutcome;
    use makepad_flow::{EvalError, Loc, Port, PortType};
    use makepad_widgets::makepad_platform::makepad_micro_serde::JsonValue;

    fn test_node() -> Node {
        Node {
            id: "prompt".into(),
            kind: "input".into(),
            type_name: "Input".into(),
            params: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![Port {
                name: "text".into(),
                ty: PortType::Text,
            }],
            at: None,
            size: None,
            flip: false,
            loc: Loc { line: 2, col: 1 },
            fn_src: None,
            face_src: None,
            on_fail: "fail".into(),
            label: None,
            domain: None,
            doc: Some("prompt".into()),
        }
    }

    fn test_graph() -> Graph {
        Graph {
            revision: 3,
            label: "Test flow".into(),
            brief: "Does a test.".into(),
            trigger: "manual".into(),
            concurrency: 1,
            autostart: false,
            nodes: vec![test_node()],
            edges: Vec::new(),
            tools: vec![ToolEntry {
                name: "run".into(),
                inputs: vec!["prompt".into()],
                outputs: Vec::new(),
                nodes: vec!["prompt".into()],
            }],
            flow_ui_src: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn manifests_from_flow_schema_are_bounded_and_object_typed() {
        let schema = ToolSchema {
            tools: vec![makepad_flow::ToolDef {
                name: "run".into(),
                description: "Run it".into(),
                parameters: r#"{"type":"object","properties":{"prompt":{"type":"string"}}}"#.into(),
                result_fields: vec![("answer".into(), PortType::Text)],
            }],
        };
        let manifest = definition_manifest_from_schema(
            "prompt-to-image",
            "Prompt to image",
            &"brief ".repeat(1000),
            &schema,
        );
        manifest.validate().unwrap();
        assert!(manifest.id.starts_with("flow_"));
        assert!(manifest.brief.len() <= 4 * 1024);
        assert!(manifest.tools.len() <= 64);
        for tool in &manifest.tools {
            let value = makepad_strict_json::parse(tool.parameters.as_bytes()).unwrap();
            assert_eq!(value.get("type").and_then(Json::as_str), Some("object"));
        }
        let flows = flows_manifest();
        flows.validate().unwrap();
        assert_eq!(flows.tools.len(), 22);
        assert_eq!(flows.topics.len(), 3);
        assert!(flows.topic("flows").is_some());
        assert!(flows.topic("instance").is_some());
        assert!(flows.topic("run").is_some());
        assert_eq!(flows.tool("templates").unwrap().risk, Risk::Read);
        assert_eq!(flows.tool("models").unwrap().risk, Risk::Read);
        assert_eq!(flows.tool("assets").unwrap().risk, Risk::Read);
        assert_eq!(flows.tool("create").unwrap().risk, Risk::Act);
        assert_eq!(flows.tool("watch").unwrap().risk, Risk::Act);
        assert_eq!(flows.tool("delete").unwrap().risk, Risk::Destructive);
        assert!(manifest.topic("run").is_some());
    }

    #[test]
    fn text_renderers_cover_lists_nodes_reads_and_outputs() {
        let rows = vec![FlowSummary {
            name: "demo".into(),
            label: "Demo".into(),
            state: "ok".into(),
            canonical: true,
            instances: 2,
            ..FlowSummary::default()
        }];
        assert!(render_flow_list(&rows).contains("demo · Demo · ok"));

        let nodes = Json::Obj(vec![(
            "types".into(),
            Json::Arr(vec![Json::Obj(vec![
                ("type_name".into(), Json::Str("Image".into())),
                ("kind".into(), Json::Str("gen".into())),
                ("domain".into(), Json::Str("image".into())),
                (
                    "ports".into(),
                    Json::Obj(vec![(
                        "out".into(),
                        Json::Arr(vec![Json::Obj(vec![
                            ("name".into(), Json::Str("image".into())),
                            ("ty".into(), Json::Str("Image".into())),
                        ])]),
                    )]),
                ),
                ("params".into(), Json::Arr(Vec::new())),
            ])]),
        )]);
        assert!(render_nodes(&nodes).contains("Image · gen/image"));

        let definition = FlowDefinition {
            source: "x".repeat(MAX_READ_TEXT + 10),
            revision: 3,
            graph: Some(test_graph()),
            tools: ToolSchema::default(),
            error: None,
        };
        let read = render_read("demo", &definition);
        assert!(read.len() <= MAX_READ_TEXT);
        assert!(read.contains("truncated"));

        let outputs = Json::Obj(vec![(
            "picture".into(),
            Json::Obj(vec![
                ("type".into(), Json::Str("Image".into())),
                ("content_type".into(), Json::Str("image/png".into())),
                ("digest".into(), Json::Str("a".repeat(64))),
                ("bytes".into(), Json::Int(42)),
            ]),
        )]);
        let mut refs = Vec::new();
        collect_value_refs(&outputs, "output", &mut refs);
        assert_eq!(refs[0].name, "picture");
        assert_eq!(refs[0].bytes, 42);
        let rendered = render_run_outputs_metadata(&outputs);
        assert!(rendered.contains("picture · aaaaaaaaaa"));
        assert!(rendered.contains("image/png · 42 bytes"));
    }

    #[test]
    fn call_arguments_map_to_input_node_ports() {
        let graph = test_graph();
        let fields = vec![("prompt".into(), Json::Str("a cat".into()))];
        let mapped = args_to_inputs(&graph, &graph.tools[0], &fields).unwrap();
        assert_eq!(
            mapped,
            Json::Obj(vec![(
                "prompt".into(),
                Json::Obj(vec![(
                    "text".into(),
                    Json::Obj(vec![
                        ("type".into(), Json::Str("text".into())),
                        ("text".into(), Json::Str("a cat".into())),
                    ]),
                )]),
            )])
        );
    }

    #[test]
    fn write_evaluation_error_is_failed_with_location() {
        let result = client_error_result(
            "call",
            ClientError::Eval(EvalError {
                file: "demo.splash".into(),
                line: 7,
                col: 11,
                message: "expected expression".into(),
            }),
        );
        assert_eq!(result.outcome, ToolOutcome::Failed);
        assert_eq!(
            result.text,
            "demo.splash:7:11: expected expression"
        );
    }

    fn flow_event(topic: &str, kind: &str) -> FlowEvent {
        FlowEvent {
            seq: 1,
            topic: topic.into(),
            kind: kind.into(),
            name: Some("demo".into()),
            revision: Some(3),
            canonical: Some(true),
            error: Some(JsonValue::String("boom\nnow".into())),
            instance: Some("inst_1".into()),
            run_id: Some("r_1".into()),
            flow: Some("demo".into()),
            node: Some("image".into()),
            port: Some("picture".into()),
            text: Some("hello\nworld".into()),
            permille: Some(410),
            stage: Some("encode".into()),
            state: Some(JsonValue::String("done".into())),
            secs: Some(5.5),
            by: Some("chat".into()),
            reason: Some("dependency failed".into()),
            question: Some("which one?".into()),
            outputs: Some(JsonValue::Array(vec![JsonValue::Array(vec![
                JsonValue::String("picture".into()),
                JsonValue::Object(HashMap::from([
                    ("type".into(), JsonValue::String("image".into())),
                    (
                        "content_type".into(),
                        JsonValue::String("image/png".into()),
                    ),
                    ("digest".into(), JsonValue::String("a".repeat(64))),
                    ("bytes".into(), JsonValue::U64(42)),
                ])),
            ])])),
            planned_nodes: Some(vec!["prompt".into(), "image".into()]),
        }
    }

    #[test]
    fn every_flow_event_kind_renders_as_one_model_readable_line() {
        let cases = [
            ("flows", "flow.changed"),
            ("flows", "flow.error"),
            ("flows", "flow.removed"),
            ("flows", "instance.created"),
            ("flows", "instance.removed"),
            ("flows", "instance.inputs"),
            ("run", "run.started"),
            ("run", "node.started"),
            ("run", "node.progress"),
            ("run", "node.delta"),
            ("run", "node.waiting"),
            ("run", "node.answered"),
            ("run", "node.done"),
            ("run", "node.failed"),
            ("run", "node.skipped"),
            ("run", "run.finished"),
        ];
        for (topic, kind) in cases {
            let text = render_event_message(&flow_event(topic, kind));
            assert!(!text.is_empty(), "{kind}");
            assert!(!text.contains('\n'), "{kind}: {text:?}");
        }
        assert_eq!(
            render_event_message(&flow_event("run", "node.progress")),
            "node image · progress 41 % · encode"
        );
        let finished = render_event_message(&flow_event("run", "run.finished"));
        assert!(finished.starts_with("run r_1 finished · done · picture sha256:"));
        assert!(finished.ends_with("· 5.5 s"));
    }

    fn test_subscriptions() -> SubscriptionTable {
        let mut table = SubscriptionTable::default();
        table.insert(
            SERVICE_ID.into(),
            None,
            "flows_sub".into(),
            "flows".into(),
            SubscriptionFilter::All,
        );
        table.insert(
            SERVICE_ID.into(),
            None,
            "instance_sub".into(),
            "instance".into(),
            SubscriptionFilter::Instance("inst_1".into()),
        );
        table.insert(
            SERVICE_ID.into(),
            None,
            "run_sub".into(),
            "run".into(),
            SubscriptionFilter::Run("r_1".into()),
        );
        table.insert(
            "flow_demo".into(),
            Some("demo".into()),
            "definition_sub".into(),
            "run".into(),
            SubscriptionFilter::Run("r_1".into()),
        );
        table
    }

    #[test]
    fn topic_and_filters_match_only_the_requested_stream() {
        let mut table = test_subscriptions();
        let flows = table.route(&flow_event("flows", "flow.changed"), 0.0);
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].sub_id, "flows_sub");

        let instance = table.route(&flow_event("instance", "node.done"), 0.0);
        assert_eq!(instance.len(), 1);
        assert_eq!(instance[0].sub_id, "instance_sub");

        let run = table.route(&flow_event("run", "node.done"), 0.0);
        let ids = run
            .iter()
            .map(|publication| publication.sub_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["definition_sub", "run_sub"]);

        let mut other = flow_event("run", "node.done");
        other.run_id = Some("r_2".into());
        assert!(table.route(&other, 0.0).is_empty());
    }

    #[test]
    fn run_progress_is_coalesced_per_subscription_for_half_a_second() {
        let mut table = SubscriptionTable::default();
        table.insert(
            SERVICE_ID.into(),
            None,
            "run_sub".into(),
            "run".into(),
            SubscriptionFilter::Run("r_1".into()),
        );
        let first = flow_event("run", "node.progress");
        assert_eq!(table.route(&first, 0.0).len(), 1);
        let mut latest = first.clone();
        latest.permille = Some(730);
        latest.stage = Some("decode".into());
        assert!(table.route(&latest, 0.2).is_empty());
        assert!(table.flush_due(0.49).is_empty());
        let flushed = table.flush_due(0.5);
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].message.text, "node image · progress 73 % · decode");
    }

    #[test]
    fn run_finished_is_final_and_closes_only_the_run_subscription() {
        let mut table = test_subscriptions();
        let published = table.route(&flow_event("run", "run.finished"), 1.0);
        assert_eq!(published.len(), 2);
        assert!(published.iter().all(|item| item.message.final_));
        assert_eq!(table.len(), 2);
        assert!(table
            .live
            .values()
            .any(|subscription| subscription.topic == "instance"));
    }

    struct FakeRunClient {
        row: Json,
        polls: Mutex<usize>,
    }

    impl RunWaitClient for FakeRunClient {
        fn start_run(
            &self,
            _instance: &str,
            _outputs: Option<&[String]>,
        ) -> Result<Json, ClientError> {
            Ok(Json::Obj(vec![(
                "run_id".into(),
                Json::Str("r_fake".into()),
            )]))
        }

        fn run(&self, _run_id: &str) -> Result<Json, ClientError> {
            *self.polls.lock().unwrap() += 1;
            Ok(self.row.clone())
        }

        fn instance(&self, _instance: &str) -> Result<Json, ClientError> {
            Ok(Json::Obj(Vec::new()))
        }

        fn cancel(&self, _run_id: &str) -> Result<(), ClientError> {
            Ok(())
        }

        fn finished(&self, _call_id: &str, _instance: &str, _row: Json) -> ToolResult {
            panic!("a running fake must return early")
        }
    }

    #[test]
    fn long_run_returns_continue_data_and_a_run_subscription() {
        let fake = FakeRunClient {
            row: Json::Obj(vec![("state".into(), Json::Str("running".into()))]),
            polls: Mutex::new(0),
        };
        let call = ServiceCall {
            call_id: "call_1".into(),
            tool: "run".into(),
            args: "{}".into(),
        };
        let (tx, _rx) = channel();
        let result = start_and_wait_for(
            &fake,
            &call,
            "inst_fake",
            None,
            &Arc::new(AtomicBool::new(false)),
            &tx,
            1,
            SERVICE_ID,
            Duration::ZERO,
        );
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert_eq!(result.disposition, Disposition::Continue);
        assert_eq!(result.subscribe.len(), 1);
        assert_eq!(result.subscribe[0].topic, "run");
        assert_eq!(
            result.subscribe[0].filter.as_deref(),
            Some(r#"{"run_id":"r_fake"}"#)
        );
        let data = makepad_strict_json::parse(result.data.as_bytes()).unwrap();
        assert_eq!(data.get("instance").and_then(Json::as_str), Some("inst_fake"));
        assert_eq!(data.get("state").and_then(Json::as_str), Some("running"));
        assert_eq!(*fake.polls.lock().unwrap(), 1);
    }
}
