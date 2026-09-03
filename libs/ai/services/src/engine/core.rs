//! The conversation: the model on one side, the services on the other,
//! the transcript in the middle.
//!
//! Free of `Cx` and of any real model, so every rule here is pinned by a
//! test with a scripted model: how a tool call finds its instance, when a
//! destructive call waits for the person, what a silent service costs,
//! what an unknown name is told, how a registry change reaches the model,
//! and what a result's disposition does to the turn.
//!
//! Time is a number of seconds the host supplies (`Cx::seconds_since_app_start`
//! or a test counter) — never `Instant`, which the web has no
//! implementation of.

use crate::engine::registry::{RegistryUp, ServiceRegistry};
use crate::engine::{Model, ModelEvent, ToolDefinition};
use crate::state::*;
use crate::wire::*;
use makepad_strict_json::Value;
use std::collections::{HashMap, VecDeque};

/// The base doctrine every conversation starts with.
pub const DOCTRINE: &str = include_str!("doctrine.md");
/// Seconds a service has to answer or report progress.
pub const CALL_DEADLINE_SECS: f64 = 60.0;
/// Seconds after which even a progressing call is given up.
pub const CALL_HARD_CAP_SECS: f64 = 600.0;
/// Tool rounds one user turn may take before the engine stops it.
pub const MAX_TOOL_ROUNDS: u32 = 16;
/// The reserved argument the model may use to pick an instance.
pub const INSTANCE_KEY: &str = "instance";
/// Live subscriptions one conversation lease may own.
pub const MAX_SUBSCRIPTIONS: usize = 16;
/// Messages retained for one subscription while wakes are throttled.
pub const MAX_SUBSCRIPTION_QUEUE: usize = 64;
/// Minimum time between model wakes for one conversation.
pub const WAKE_INTERVAL_SECS: f64 = 2.0;

/// What the host should do after a `pump`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// The state changed; redraw.
    Changed,
    /// A destructive call is parked; the panel shows its confirm card.
    Confirm { call_id: String },
}

struct Pending {
    endpoint: EndpointId,
    deadline: f64,
    hard_deadline: f64,
    /// Typed by the person at the console, not asked by the model: the
    /// result is shown, not sent back.
    from_console: bool,
}

struct Parked {
    endpoint: EndpointId,
    call: ServiceCall,
    from_console: bool,
}

/// Seconds an `os.launch` answer is held for the app it started to
/// register, so the model can use that app's tools in the same turn.
pub const LAUNCH_PATIENCE_SECS: f64 = 8.0;

/// An `os.launch` that answered ok, held until the launched app is on
/// the bus (or the patience runs out) — see `pump_awaiting`.
struct AwaitingApp {
    call_id: String,
    app: String,
    result: ToolResult,
    from_console: bool,
    seen_generation: u64,
    deadline: f64,
}

struct Subscription {
    endpoint: EndpointId,
    service_label: String,
    topic: String,
    filter: Option<String>,
    created: f64,
    queue: VecDeque<Message>,
    dropped: u32,
    /// A final publication has arrived; remove the row when it is delivered.
    closing: bool,
}

pub struct EngineCore {
    registry: ServiceRegistry,
    model: Box<dyn Model>,
    state: EngineState,
    pending: HashMap<String, Pending>,
    parked: HashMap<String, Parked>,
    awaiting: Option<AwaitingApp>,
    seen_generation: Option<u64>,
    tool_rounds: u32,
    next_call: u64,
    doctrine: String,
    turn_active: bool,
    subscriptions: HashMap<String, Subscription>,
    subscription_order: Vec<String>,
    lease_id: u64,
    next_subscription: u64,
    wake_cursor: usize,
    last_wake: Option<f64>,
}

impl EngineCore {
    /// Start a conversation lease. `lease_id` must be unique for this host
    /// process; it namespaces every subscription id exposed to services.
    pub fn new(
        registry: ServiceRegistry,
        model: Box<dyn Model>,
        doctrine: Option<&str>,
        lease_id: u64,
    ) -> EngineCore {
        let mut state = EngineState::default();
        state.provider_label = model.label();
        EngineCore {
            registry,
            model,
            state,
            pending: HashMap::new(),
            parked: HashMap::new(),
            awaiting: None,
            seen_generation: None,
            tool_rounds: 0,
            next_call: 0,
            doctrine: doctrine.unwrap_or(DOCTRINE).to_string(),
            turn_active: false,
            subscriptions: HashMap::new(),
            subscription_order: Vec::new(),
            lease_id,
            next_subscription: 0,
            wake_cursor: 0,
            last_wake: None,
        }
    }

    pub fn state(&self) -> &EngineState {
        &self.state
    }

    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
    }

    pub fn model_mut(&mut self) -> &mut dyn Model {
        self.model.as_mut()
    }

    pub fn subscription_count(&self) -> usize {
        self.subscriptions.values().filter(|subscription| !subscription.closing).count()
    }

    pub fn queued_message_count(&self) -> usize {
        self.subscriptions.values().map(|subscription| subscription.queue.len()).sum()
    }

    /// End this conversation lease while its transport is still available.
    /// Hosts must flush their hosted adapter after this call.
    pub fn shutdown(&mut self) {
        self.drop_subscriptions();
    }

    /// Whether the host should keep scheduling pumps even though the model
    /// is idle (normally because a rate-limited publication is waiting).
    pub fn needs_pump(&self) -> bool {
        self.turn_active || self.awaiting.is_some() || self.queued_message_count() != 0
    }

    /// What the provider chip shows: the choice, the rows it may switch
    /// to, and whether the lock is on. Presentation only.
    pub fn set_provider_facts(&mut self, provider: ProviderChoice, rows: Vec<ProviderRow>, local_only: bool) {
        self.state.provider = provider;
        self.state.providers = rows;
        self.state.local_only = local_only;
        self.state.touch();
    }

    /// Swap the model (a provider change). The conversation restarts.
    pub fn set_model(&mut self, model: Box<dyn Model>) {
        self.model.cancel();
        self.model = model;
        self.state.provider_label = self.model.label();
        self.seen_generation = None;
        self.end_turn();
        self.state.push(Entry::System { text: format!("now answering with {}", self.state.provider_label) });
    }

    /// The person's turn. A line starting with `/` is the local tool
    /// console: `/files.list_dir {"path":"~"}` runs the tool directly and
    /// shows the result, no model involved.
    pub fn send(&mut self, text: &str, now: f64) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(rest) = text.strip_prefix('/') {
            self.console(rest.trim(), now);
            return;
        }
        if self.turn_active {
            self.cancel(now);
        }
        self.reconfigure_if_needed();
        self.state.push(Entry::User { text: text.to_string() });
        self.state.status = Status::Thinking;
        self.state.thinking.clear();
        self.state.rate = None;
        self.turn_active = true;
        self.tool_rounds = 0;
        let dynamic = self.registry.dynamic_context();
        self.model.send_user(text, &dynamic);
        self.state.touch();
    }

    fn console(&mut self, line: &str, now: f64) {
        if line.is_empty() || line == "help" {
            let mut names: Vec<String> = self.registry.tool_definitions().into_iter().map(|t| t.name).collect();
            names.push("bus.unsubscribe".into());
            let text = if names.is_empty() {
                "no tools: no app is connected".to_string()
            } else {
                format!("tools: {}", names.join(", "))
            };
            self.state.push(Entry::System { text });
            return;
        }
        let (name, args) = match line.split_once(char::is_whitespace) {
            Some((n, a)) => (n.trim(), a.trim()),
            None => (line, "{}"),
        };
        let args = if args.is_empty() { "{}" } else { args };
        let call_id = self.mint_call_id();
        self.dispatch(call_id, name, args, true, now);
    }

    /// Stop the turn: the model, every running call, every parked one.
    pub fn cancel(&mut self, _now: f64) {
        self.model.cancel();
        let running: Vec<(String, EndpointId)> = self.pending.drain().map(|(id, p)| (id, p.endpoint)).collect();
        for (call_id, endpoint) in running {
            self.registry.send(&endpoint, ServiceDown::Cancel { call_id: call_id.clone() });
            self.mark_done(&call_id, &ToolResult::cancelled(&call_id));
        }
        let parked: Vec<String> = self.parked.drain().map(|(id, _)| id).collect();
        for call_id in parked {
            self.mark_done(&call_id, &ToolResult::denied(&call_id, "cancelled"));
        }
        if let Some(a) = self.awaiting.take() {
            self.mark_done(&a.call_id, &ToolResult::cancelled(&a.call_id));
        }
        self.end_turn();
    }

    /// A new conversation: same services, same provider, empty transcript.
    pub fn clear(&mut self, now: f64) {
        self.cancel(now);
        self.drop_subscriptions();
        self.model.reset();
        self.state.entries.clear();
        self.state.thinking.clear();
        self.state.rate = None;
        self.state.status = Status::Idle;
        self.seen_generation = None;
        self.state.touch();
    }

    /// The person's answer to a confirm card.
    pub fn confirm(&mut self, call_id: &str, run: bool, now: f64) {
        let Some(parked) = self.parked.remove(call_id) else { return };
        if run {
            self.launch(parked.endpoint, parked.call, parked.from_console, now);
        } else {
            let result = ToolResult::denied(call_id, "the person did not confirm");
            self.finish_call(call_id, result, parked.from_console);
        }
    }

    pub fn toggle_tool(&mut self, call_id: &str) {
        if let Some(t) = self.state.tool_mut(call_id) {
            t.expanded = !t.expanded;
            self.state.touch();
        }
    }

    /// Tell the services whether the pane is showing.
    pub fn set_chat_open(&mut self, open: bool) {
        self.registry.broadcast_chat_open(open);
    }

    /// Drive everything once. Call on every host event and on a timer
    /// while a turn is active.
    pub fn pump(&mut self, now: f64) -> Vec<EngineEvent> {
        let before = self.state.generation;
        let mut events = Vec::new();
        for up in self.registry.pump() {
            match up {
                RegistryUp::Result(endpoint, result) => self.on_result(&endpoint, result, now),
                RegistryUp::Progress { endpoint, call_id, note, permille } => {
                    if let Some(p) = self.pending.get_mut(&call_id) {
                        if p.endpoint == endpoint {
                            p.deadline = (now + CALL_DEADLINE_SECS).min(p.hard_deadline);
                            if let Some(t) = self.state.tool_mut(&call_id) {
                                t.status = ToolStatus::Running { note, permille };
                                self.state.touch();
                            }
                        }
                    }
                }
                RegistryUp::Message { endpoint, sub_id, message } => {
                    self.on_message(&endpoint, &sub_id, message);
                }
            }
        }
        self.close_gone_subscriptions();
        self.pump_awaiting(now);
        if !self.turn_active {
            self.reconfigure_if_needed();
        }
        let model_events = self.model.poll();
        for ev in model_events {
            if let Some(e) = self.on_model_event(ev, now) {
                events.push(e);
            }
        }
        let late: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(id, _)| id.clone())
            .collect();
        for call_id in late {
            let p = self.pending.remove(&call_id).unwrap();
            self.registry.send(&p.endpoint, ServiceDown::Cancel { call_id: call_id.clone() });
            let label = self.registry.meta(&p.endpoint).map(|m| m.display_name).unwrap_or_default();
            let result = ToolResult::timed_out(&call_id, format!("{label} did not answer in time"));
            self.finish_call(&call_id, result, p.from_console);
        }
        if !self.turn_active {
            self.deliver_next_message(now);
        }
        let services = self.registry.services();
        if services != self.state.services {
            self.state.services = services;
            self.state.touch();
        }
        if self.state.generation != before {
            events.push(EngineEvent::Changed);
        }
        events
    }

    fn reconfigure_if_needed(&mut self) {
        let generation = self.registry.generation();
        if self.seen_generation == Some(generation) {
            return;
        }
        let system = format!(
            "{}\n# Applications\n{}{}",
            self.doctrine.trim(),
            self.registry.briefs(),
            self.subscription_prompt()
        );
        let mut tools: Vec<ToolDefinition> = self.registry.tool_definitions();
        tools.push(ToolDefinition {
            name: "bus.unsubscribe".into(),
            description: "Stop one live asynchronous subscription by its sub_id.".into(),
            parameters: r#"{"type":"object","properties":{"sub_id":{"type":"string"}},"required":["sub_id"]}"#.into(),
        });
        if let Err(e) = self.model.configure(&system, &tools) {
            // Never let the table drift: restart the conversation instead.
            self.model.reset();
            self.state.push(Entry::System { text: format!("the tools changed; the conversation restarted ({e})") });
            let _ = self.model.configure(&system, &tools);
        }
        self.seen_generation = Some(generation);
        self.state.services = self.registry.services();
        self.state.touch();
    }

    fn mint_call_id(&mut self) -> String {
        self.next_call += 1;
        format!("c{}", self.next_call)
    }

    fn subscription_prompt(&self) -> String {
        if self.subscriptions.is_empty() {
            return String::new();
        }
        let mut rows: Vec<(&String, &Subscription)> =
            self.subscriptions.iter().filter(|(_, subscription)| !subscription.closing).collect();
        if rows.is_empty() {
            return String::new();
        }
        rows.sort_by(|(a_id, a), (b_id, b)| {
            a.created.total_cmp(&b.created).then_with(|| a_id.cmp(b_id))
        });
        let mut out = String::from("\n# Live subscriptions\n");
        for (sub_id, subscription) in rows {
            out.push_str(&format!(
                "- {} · {} (`{}`)",
                subscription.service_label, subscription.topic, sub_id
            ));
            if let Some(filter) = &subscription.filter {
                out.push_str(&format!(" filter {filter}"));
            }
            out.push('\n');
        }
        out.push_str("Use `bus.unsubscribe` with a sub_id when updates are no longer wanted.\n");
        out
    }

    fn on_model_event(&mut self, ev: ModelEvent, now: f64) -> Option<EngineEvent> {
        match ev {
            ModelEvent::Loading { phase, fraction } => {
                self.state.status = Status::Loading { phase, fraction };
            }
            ModelEvent::Ready => {
                // The chip follows the model's own account of where it runs
                // (the local election settles on a route only now).
                self.state.provider_label = self.model.label();
                if !self.turn_active {
                    self.state.status = Status::Idle;
                }
            }
            ModelEvent::Delta(text) => {
                self.state.status = Status::Streaming;
                match self.state.streaming_mut() {
                    Some(s) => s.push_str(&text),
                    // Leading whitespace opens no row: a model that breathes
                    // a newline before its first tool call must not leave a
                    // blank block above the card.
                    None if text.trim().is_empty() => {}
                    None => self.state.push(Entry::Assistant { text: text.trim_start().to_string(), streaming: true }),
                }
            }
            ModelEvent::Thinking(text) => {
                self.state.thinking.push_str(&text);
                if self.state.thinking.len() > MAX_THINKING_BYTES {
                    let cut = self.state.thinking.len() - MAX_THINKING_BYTES;
                    let mut keep = self.state.thinking.split_off(cut);
                    while !keep.is_char_boundary(0) {
                        keep.remove(0);
                    }
                    self.state.thinking = keep;
                }
            }
            ModelEvent::ToolCall { call_id, name, args } => {
                self.tool_rounds += 1;
                if self.tool_rounds > MAX_TOOL_ROUNDS {
                    let result = ToolResult::refused(&call_id, format!("tool round limit ({MAX_TOOL_ROUNDS}) reached; answer with what you have"));
                    self.state.push(Entry::Tool(self.card(&call_id, "", &name, &args)));
                    self.finish_call(&call_id, result, false);
                    self.model.cancel();
                    self.end_turn();
                } else {
                    // A row that never got text sits above the card as a gap.
                    self.drop_empty_assistant();
                    self.state.status = Status::WaitingForTool;
                    return self.dispatch(call_id, &name, &args, false, now);
                }
            }
            ModelEvent::Rate(r) => self.state.rate = Some(r),
            ModelEvent::TurnDone { tool_calls } => {
                if tool_calls == 0 && self.pending.is_empty() && self.parked.is_empty() {
                    self.end_turn();
                }
            }
            ModelEvent::Error(e) => {
                self.state.push(Entry::System { text: e.clone() });
                self.end_turn();
                self.state.status = Status::Error(e);
            }
        }
        self.state.touch();
        None
    }

    fn end_turn(&mut self) {
        if let Some(Entry::Assistant { streaming, .. }) = self.state.entries.last_mut() {
            *streaming = false;
        }
        self.drop_empty_assistant();
        self.turn_active = false;
        self.state.status = Status::Idle;
        self.state.thinking.clear();
        self.state.touch();
    }

    fn card(&self, call_id: &str, service: &str, name: &str, args: &str) -> ToolEntry {
        let (app, tool) = split_name(name);
        let label = if app == "bus" {
            "Bus".into()
        } else {
            self.registry
                .instances_of(app)
                .first()
                .and_then(|e| self.registry.meta(e))
                .map(|m| m.display_name)
                .unwrap_or_else(|| if app.is_empty() { "?".into() } else { app.to_string() })
        };
        let mut summary = args.trim().trim_start_matches('{').trim_end_matches('}').trim().to_string();
        truncate_to_char_boundary(&mut summary, 60);
        ToolEntry {
            call_id: call_id.to_string(),
            service: if service.is_empty() { app.to_string() } else { service.to_string() },
            service_label: label.clone(),
            tool: tool.to_string(),
            title: if summary.is_empty() { format!("{label} · {tool}") } else { format!("{label} · {tool}  {summary}") },
            args: args.to_string(),
            status: ToolStatus::Running { note: String::new(), permille: 0 },
            preview: false,
            expanded: false,
        }
    }

    /// Resolve a call by name to an instance and either launch it, park
    /// it, or answer it at once with why it cannot run.
    fn dispatch(&mut self, call_id: String, name: &str, args: &str, from_console: bool, now: f64) -> Option<EngineEvent> {
        let (app, tool) = split_name(name);
        let (app, tool) = (app.to_string(), tool.to_string());
        self.state.push(Entry::Tool(self.card(&call_id, &app, name, args)));
        if app == "bus" {
            self.dispatch_bus(call_id, &tool, args, from_console);
            return None;
        }
        if app.is_empty() || self.registry.instances_of(&app).is_empty() {
            let result = if !app.is_empty() && self.registry.is_launchable(&app) {
                ToolResult::unavailable(&call_id, format!("{app} is not running; `os.launch` can start it"))
            } else {
                let known = self.registry.known_app_ids();
                ToolResult::refused(
                    &call_id,
                    format!("no service '{app}'; the name is the whole dotted name and the services are: {}", known.join(", ")),
                )
            };
            self.finish_call(&call_id, result, from_console);
            return None;
        }
        // Arguments: an object, with the instance selector taken out.
        let (args_json, wanted) = match makepad_strict_json::parse(args.as_bytes()) {
            Ok(Value::Obj(fields)) => {
                let wanted = fields
                    .iter()
                    .find(|(k, _)| k == INSTANCE_KEY)
                    .and_then(|(_, v)| v.as_str().map(|s| s.to_string()));
                let rest: Vec<(String, Value)> = fields.into_iter().filter(|(k, _)| k != INSTANCE_KEY).collect();
                (Value::Obj(rest).to_json(), wanted)
            }
            _ => {
                let result = ToolResult::refused(&call_id, "arguments must be a JSON object");
                self.finish_call(&call_id, result, from_console);
                return None;
            }
        };
        let Some(endpoint) = self.registry.pick_instance(&app, wanted.as_deref()) else {
            let result = ToolResult::refused(&call_id, format!("no instance of {app} matches '{}'", wanted.unwrap_or_default()));
            self.finish_call(&call_id, result, from_console);
            return None;
        };
        let manifest = self.registry.manifest(&endpoint).unwrap_or_default();
        let Some(def) = manifest.tool(&tool) else {
            let names: Vec<String> = manifest.tools.iter().map(|t| canonical_name(&app, &t.name)).collect();
            let result = ToolResult::refused(&call_id, format!("{app} has no tool '{tool}'; its tools are: {}", names.join(", ")));
            self.finish_call(&call_id, result, from_console);
            return None;
        };
        if let Some(t) = self.state.tool_mut(&call_id) {
            t.preview = def.preview;
            t.service_label = self.registry.meta(&endpoint).map(|m| m.display_name).unwrap_or_default();
        }
        if args_json.len() > MAX_ARGS_BYTES {
            let result = ToolResult::refused(&call_id, "arguments over the size cap");
            self.finish_call(&call_id, result, from_console);
            return None;
        }
        let risk = def.risk.max(self.registry.risk_floor(&app));
        let call = ServiceCall { call_id: call_id.clone(), tool: tool.clone(), args: args_json };
        if risk == Risk::Destructive && !from_console {
            if let Some(t) = self.state.tool_mut(&call_id) {
                t.status = ToolStatus::Confirm;
            }
            self.parked.insert(call_id.clone(), Parked { endpoint, call, from_console });
            self.state.touch();
            return Some(EngineEvent::Confirm { call_id });
        }
        self.launch(endpoint, call, from_console, now);
        None
    }

    fn dispatch_bus(&mut self, call_id: String, tool: &str, args: &str, from_console: bool) {
        if tool != "unsubscribe" {
            self.finish_call(
                &call_id,
                ToolResult::refused(&call_id, "bus has one tool: bus.unsubscribe"),
                from_console,
            );
            return;
        }
        let sub_id = match makepad_strict_json::parse(args.as_bytes()) {
            Ok(Value::Obj(fields)) => fields
                .into_iter()
                .find(|(key, _)| key == "sub_id")
                .and_then(|(_, value)| value.as_str().map(str::to_string)),
            _ => None,
        };
        let Some(sub_id) = sub_id.filter(|id| is_opaque_id(id)) else {
            self.finish_call(
                &call_id,
                ToolResult::refused(&call_id, "bus.unsubscribe needs a valid string sub_id"),
                from_console,
            );
            return;
        };
        let Some(subscription) = self.subscriptions.remove(&sub_id) else {
            self.finish_call(
                &call_id,
                ToolResult::refused(&call_id, format!("no live subscription '{sub_id}'")),
                from_console,
            );
            return;
        };
        self.subscription_order.retain(|id| id != &sub_id);
        self.registry.send(&subscription.endpoint, ServiceDown::Unsubscribe { sub_id: sub_id.clone() });
        self.seen_generation = None;
        self.finish_call(
            &call_id,
            ToolResult::ok(&call_id, format!("unsubscribed {sub_id}"), "unsubscribed"),
            from_console,
        );
    }

    fn launch(&mut self, endpoint: EndpointId, call: ServiceCall, from_console: bool, now: f64) {
        let call_id = call.call_id.clone();
        if !self.registry.send(&endpoint, ServiceDown::Call(call)) {
            let result = ToolResult::unavailable(&call_id, "the app went away");
            self.finish_call(&call_id, result, from_console);
            return;
        }
        if let Some(t) = self.state.tool_mut(&call_id) {
            t.status = ToolStatus::Running { note: String::new(), permille: 0 };
        }
        self.pending.insert(
            call_id,
            Pending { endpoint, deadline: now + CALL_DEADLINE_SECS, hard_deadline: now + CALL_HARD_CAP_SECS, from_console },
        );
        self.state.touch();
    }

    fn on_result(&mut self, endpoint: &EndpointId, mut result: ToolResult, now: f64) {
        let Some(p) = self.pending.get(&result.call_id) else { return };
        if &p.endpoint != endpoint {
            return;
        }
        let from_console = p.from_console;
        self.pending.remove(&result.call_id);
        result.bound();
        let call_id = result.call_id.clone();
        self.install_subscriptions(endpoint, &mut result, now);
        // An `os.launch` that worked is held until the app it started is on
        // the bus, so the model gets its tools with the result and goes on
        // in the same turn instead of stopping at "it is opening".
        if !from_console && result.outcome.is_ok() && self.awaiting.is_none() {
            if let Some(app) = self.launched_app(&call_id) {
                if self.registry.instances_of(&app).is_empty() {
                    if let Some(t) = self.state.tool_mut(&call_id) {
                        t.status = ToolStatus::Running { note: format!("waiting for {app} to connect…"), permille: 500 };
                    }
                    self.awaiting = Some(AwaitingApp {
                        call_id,
                        app,
                        result,
                        from_console,
                        seen_generation: self.registry.generation(),
                        deadline: now + LAUNCH_PATIENCE_SECS,
                    });
                    self.state.touch();
                    return;
                }
            }
        }
        self.finish_call(&call_id, result, from_console);
    }

    fn install_subscriptions(&mut self, endpoint: &EndpointId, result: &mut ToolResult, now: f64) {
        if !result.outcome.is_ok() || result.subscribe.is_empty() {
            return;
        }
        let requests = std::mem::take(&mut result.subscribe);
        if self.subscriptions.len() + requests.len() > MAX_SUBSCRIPTIONS {
            *result = ToolResult::refused(
                result.call_id.clone(),
                format!("subscription limit ({MAX_SUBSCRIPTIONS}) reached"),
            );
            return;
        }
        let Some(manifest) = self.registry.manifest(endpoint) else {
            *result = ToolResult::unavailable(result.call_id.clone(), "the app went away before subscribing");
            return;
        };
        if let Some(missing) = requests.iter().find(|request| manifest.topic(&request.topic).is_none()) {
            *result = ToolResult::refused(
                result.call_id.clone(),
                format!("{} does not publish topic '{}'", manifest.id, missing.topic),
            );
            return;
        }
        let service_label = self
            .registry
            .meta(endpoint)
            .map(|meta| meta.display_name)
            .unwrap_or_else(|| manifest.label.clone());
        let mut enrolled = Vec::new();
        for request in requests {
            self.next_subscription += 1;
            let sub_id = format!("l{:x}-s{}", self.lease_id, self.next_subscription);
            if !self.registry.send(
                endpoint,
                ServiceDown::Subscribe {
                    sub_id: sub_id.clone(),
                    topic: request.topic.clone(),
                    filter: request.filter.clone(),
                },
            ) {
                for (created_id, _) in &enrolled {
                    self.subscriptions.remove(created_id);
                    self.subscription_order.retain(|id| id != created_id);
                }
                *result = ToolResult::unavailable(result.call_id.clone(), "the app went away before subscribing");
                return;
            }
            self.subscriptions.insert(
                sub_id.clone(),
                Subscription {
                    endpoint: endpoint.clone(),
                    service_label: service_label.clone(),
                    topic: request.topic.clone(),
                    filter: request.filter,
                    created: now,
                    queue: VecDeque::new(),
                    dropped: 0,
                    closing: false,
                },
            );
            self.subscription_order.push(sub_id.clone());
            enrolled.push((sub_id, request.topic));
        }
        self.seen_generation = None;
        let subscriptions = enrolled
            .iter()
            .map(|(sub_id, topic)| format!("{topic} (sub_id: {sub_id})"))
            .collect::<Vec<_>>()
            .join(", ");
        if !result.text.trim().is_empty() {
            result.text.push('\n');
        }
        result.text.push_str(&format!("Subscribed: {subscriptions}."));
        result.bound();
    }

    fn on_message(&mut self, endpoint: &EndpointId, sub_id: &str, message: Message) {
        let Some(subscription) = self.subscriptions.get_mut(sub_id) else { return };
        if &subscription.endpoint != endpoint || subscription.topic != message.topic || subscription.closing {
            return;
        }
        if self.turn_active && !subscription.queue.is_empty() {
            subscription.dropped = subscription.dropped.saturating_add(subscription.queue.len() as u32);
            subscription.queue.clear();
        } else if subscription.queue.len() >= MAX_SUBSCRIPTION_QUEUE {
            subscription.queue.pop_front();
            subscription.dropped = subscription.dropped.saturating_add(1);
        }
        subscription.closing = message.final_;
        subscription.queue.push_back(message);
        if subscription.closing {
            self.seen_generation = None;
        }
    }

    fn close_gone_subscriptions(&mut self) {
        let gone: Vec<String> = self
            .subscriptions
            .iter()
            .filter(|(_, subscription)| {
                !subscription.closing && self.registry.manifest(&subscription.endpoint).is_none()
            })
            .map(|(sub_id, _)| sub_id.clone())
            .collect();
        for sub_id in gone {
            let Some(subscription) = self.subscriptions.get_mut(&sub_id) else { continue };
            let message = Message::new(&subscription.topic, "service disconnected").final_message();
            if self.turn_active && !subscription.queue.is_empty() {
                subscription.dropped = subscription.dropped.saturating_add(subscription.queue.len() as u32);
                subscription.queue.clear();
            } else if subscription.queue.len() >= MAX_SUBSCRIPTION_QUEUE {
                subscription.queue.pop_front();
                subscription.dropped = subscription.dropped.saturating_add(1);
            }
            subscription.closing = true;
            subscription.queue.push_back(message);
            self.seen_generation = None;
        }
    }

    fn deliver_next_message(&mut self, now: f64) {
        if self
            .last_wake
            .is_some_and(|last_wake| now - last_wake < WAKE_INTERVAL_SECS)
        {
            return;
        }
        let count = self.subscription_order.len();
        if count == 0 {
            return;
        }
        let mut selected = None;
        for offset in 0..count {
            let index = (self.wake_cursor + offset) % count;
            let sub_id = &self.subscription_order[index];
            if self.subscriptions.get(sub_id).is_some_and(|subscription| !subscription.queue.is_empty()) {
                selected = Some((index, sub_id.clone()));
                break;
            }
        }
        let Some((index, sub_id)) = selected else { return };
        self.wake_cursor = (index + 1) % count;
        let (message, service_label, dropped, closing) = {
            let subscription = self.subscriptions.get_mut(&sub_id).unwrap();
            let message = subscription.queue.pop_front().unwrap();
            let dropped = std::mem::take(&mut subscription.dropped);
            let closing = message.final_;
            (message, subscription.service_label.clone(), dropped, closing)
        };
        if closing {
            self.subscriptions.remove(&sub_id);
            self.subscription_order.retain(|id| id != &sub_id);
            self.seen_generation = None;
        }
        let entry = EventEntry {
            sub_id: sub_id.clone(),
            service_label: service_label.clone(),
            topic: message.topic.clone(),
            text: message.text.clone(),
            data: message.data.clone(),
            dropped,
            final_: message.final_,
        };
        let mut input = format!("[event] {service_label} · {} · {}\nsub_id: {sub_id}", message.topic, message.text);
        if let Some(data) = &message.data {
            input.push_str(&format!("\ndata: {data}"));
        }
        if dropped != 0 {
            input.push_str(&format!("\ndropped: {dropped}"));
        }
        self.reconfigure_if_needed();
        self.state.push(Entry::Event(entry));
        self.state.status = Status::Thinking;
        self.state.thinking.clear();
        self.state.rate = None;
        self.turn_active = true;
        self.tool_rounds = 0;
        self.last_wake = Some(now);
        let dynamic = self.registry.dynamic_context();
        self.model.send_user(&input, &dynamic);
        self.state.touch();
    }

    fn drop_subscriptions(&mut self) {
        for (sub_id, subscription) in self.subscriptions.drain() {
            if !subscription.closing {
                self.registry.send(&subscription.endpoint, ServiceDown::Unsubscribe { sub_id });
            }
        }
        self.subscription_order.clear();
        self.wake_cursor = 0;
        self.last_wake = None;
        self.seen_generation = None;
    }

    /// The app an `os.launch` card asked for, when `call_id` is one.
    fn launched_app(&self, call_id: &str) -> Option<String> {
        let t = self.state.tool(call_id)?;
        if t.service != "os" || t.tool != "launch" {
            return None;
        }
        match makepad_strict_json::parse(t.args.as_bytes()) {
            Ok(Value::Obj(fields)) => fields
                .into_iter()
                .find(|(k, _)| k == "app")
                .and_then(|(_, v)| v.as_str().map(|s| s.trim().to_lowercase())),
            _ => None,
        }
    }

    /// The held `os.launch` answer: delivered as soon as the launched app
    /// registers — with the app's tools named in the result, and the
    /// model's tool table rebound when it can rebind mid-turn — or, when
    /// the patience runs out, as it is with a note that the app has not
    /// connected yet.
    fn pump_awaiting(&mut self, now: f64) {
        let Some(a) = self.awaiting.as_ref() else { return };
        let registered = self.registry.generation() != a.seen_generation && !self.registry.instances_of(&a.app).is_empty();
        if !registered && now < a.deadline {
            return;
        }
        let a = self.awaiting.take().unwrap();
        let mut result = a.result;
        if registered {
            let label = self
                .registry
                .instances_of(&a.app)
                .first()
                .and_then(|e| self.registry.meta(e))
                .map(|m| m.display_name)
                .unwrap_or_else(|| a.app.clone());
            let prefix = format!("{}.", a.app);
            let tools: Vec<String> = self
                .registry
                .tool_definitions()
                .into_iter()
                .map(|t| t.name)
                .filter(|n| n.starts_with(&prefix))
                .collect();
            result.text = format!("{label} is running now. Its tools: {}. Call them directly.", tools.join(", "));
            result.note = "running".into();
            if self.model.can_rebind_mid_turn() {
                self.reconfigure_if_needed();
            }
        } else {
            result.text = format!("{} ({} has not connected yet; its tools are not available in this turn)", result.text, a.app);
            result.note = "not connected yet".into();
        }
        result.bound();
        self.finish_call(&a.call_id, result, a.from_console);
    }

    /// Assistant rows that never got visible text are not rows: they sit
    /// in the transcript as blank blocks. Left alone while a confirm card
    /// waits, so the panel's row indices stay what it drew.
    fn drop_empty_assistant(&mut self) {
        if !self.parked.is_empty() {
            return;
        }
        let before = self.state.entries.len();
        self.state.entries.retain(|e| !matches!(e, Entry::Assistant { text, .. } if text.trim().is_empty()));
        if self.state.entries.len() != before {
            self.state.touch();
        }
    }

    fn mark_done(&mut self, call_id: &str, result: &ToolResult) {
        if let Some(t) = self.state.tool_mut(call_id) {
            t.status = ToolStatus::Done { outcome: result.outcome, note: result.note.clone(), text: result.text.clone() };
            if !result.outcome.is_ok() {
                t.preview = false;
            } else if result.preview {
                t.preview = true;
            }
        }
        self.state.touch();
    }

    /// A call is over: the card lands, the model hears about it (unless
    /// the person typed it), and the disposition does what it says.
    fn finish_call(&mut self, call_id: &str, result: ToolResult, from_console: bool) {
        self.mark_done(call_id, &result);
        if from_console {
            return;
        }
        let body = if result.text.trim().is_empty() { result.note.clone() } else { result.text.clone() };
        let text = if result.outcome.is_ok() { body } else { format!("[{}] {body}", result.outcome.slug()) };
        self.model.send_tool_result(call_id, &text, !result.outcome.is_ok());
        match result.disposition {
            Disposition::Continue => {}
            Disposition::EndTurn => {
                self.model.cancel();
                self.end_turn();
            }
            Disposition::ResetConversation => {
                self.drop_subscriptions();
                self.model.reset();
                self.seen_generation = None;
                self.end_turn();
                self.state.push(Entry::System { text: "the conversation was reset by the app".into() });
            }
        }
    }
}

impl Drop for EngineCore {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::{AiServicePort, PortEvent};
    use std::collections::VecDeque;

    /// A model that answers each user turn or tool result with the next
    /// scripted batch of events.
    struct Scripted {
        script: VecDeque<Vec<ModelEvent>>,
        out: Vec<ModelEvent>,
        configured: Vec<(String, Vec<ToolDefinition>)>,
        sends: Vec<(String, String)>,
        tool_results: Vec<(String, String, bool)>,
        cancels: usize,
        resets: usize,
        fail_configure_once: bool,
    }

    impl Scripted {
        fn new(script: Vec<Vec<ModelEvent>>) -> Self {
            Scripted {
                script: script.into(),
                out: Vec::new(),
                configured: Vec::new(),
                sends: Vec::new(),
                tool_results: Vec::new(),
                cancels: 0,
                resets: 0,
                fail_configure_once: false,
            }
        }
        fn next(&mut self) {
            if let Some(batch) = self.script.pop_front() {
                self.out.extend(batch);
            }
        }
    }

    /// Shared handle so tests can inspect the model the core owns.
    struct Handle(std::sync::Arc<std::sync::Mutex<Scripted>>);

    impl Model for Handle {
        fn label(&self) -> String {
            "Scripted".into()
        }
        fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
            let mut m = self.0.lock().unwrap();
            m.configured.push((system.to_string(), tools.to_vec()));
            if m.fail_configure_once {
                m.fail_configure_once = false;
                return Err("cannot rebind".into());
            }
            Ok(())
        }
        fn send_user(&mut self, text: &str, dynamic_context: &str) {
            let mut m = self.0.lock().unwrap();
            m.sends.push((text.to_string(), dynamic_context.to_string()));
            m.next();
        }
        fn send_tool_result(&mut self, call_id: &str, text: &str, is_error: bool) {
            let mut m = self.0.lock().unwrap();
            m.tool_results.push((call_id.to_string(), text.to_string(), is_error));
            m.next();
        }
        fn cancel(&mut self) {
            self.0.lock().unwrap().cancels += 1;
        }
        fn reset(&mut self) {
            self.0.lock().unwrap().resets += 1;
        }
        fn poll(&mut self) -> Vec<ModelEvent> {
            std::mem::take(&mut self.0.lock().unwrap().out)
        }
    }

    fn scripted(script: Vec<Vec<ModelEvent>>) -> (Box<dyn Model>, std::sync::Arc<std::sync::Mutex<Scripted>>) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Scripted::new(script)));
        (Box::new(Handle(shared.clone())), shared)
    }

    fn files() -> ServiceManifest {
        ServiceManifest::new("files", "Files", "The file browser. Ask it what is in a folder.")
            .with_tool(ToolDef::new("list_dir", "List a folder.", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#, Risk::Read))
            .with_tool(ToolDef::new("trash", "Move a file to the trash.", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#, Risk::Destructive))
            .with_tool(ToolDef::new("open", "Show a folder.", r#"{"type":"object","properties":{"path":{"type":"string"}}}"#, Risk::Act).with_preview())
            .with_topic(TopicDef::new("watch", "Changes to a watched folder."))
    }

    fn call(name: &str, args: &str) -> ModelEvent {
        ModelEvent::ToolCall { call_id: "m1".into(), name: name.into(), args: args.into() }
    }

    fn done(text: &str) -> Vec<ModelEvent> {
        vec![ModelEvent::Delta(text.into()), ModelEvent::TurnDone { tool_calls: 0 }]
    }

    fn setup_with_lease(
        script: Vec<Vec<ModelEvent>>,
        lease_id: u64,
    ) -> (EngineCore, std::sync::Arc<std::sync::Mutex<Scripted>>, AiServicePort, EndpointId) {
        let reg = ServiceRegistry::new();
        let (mut port, link) = AiServicePort::in_process(files()).unwrap();
        let e = reg.register(link, "the Files tile", None).unwrap();
        reg.pump();
        port.test_drain();
        let (model, shared) = scripted(script);
        (EngineCore::new(reg, model, None, lease_id), shared, port, e)
    }

    fn setup(script: Vec<Vec<ModelEvent>>) -> (EngineCore, std::sync::Arc<std::sync::Mutex<Scripted>>, AiServicePort, EndpointId) {
        setup_with_lease(script, 1)
    }

    #[test]
    fn a_turn_routes_a_tool_call_to_the_instance_and_feeds_the_result_back() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Three folders."),
        ]);
        core.send("what is in my home folder", 0.0);
        {
            let m = model.lock().unwrap();
            assert_eq!(m.sends.len(), 1);
            let (system, tools) = &m.configured[0];
            assert!(system.contains("Files"), "brief in the system prompt");
            assert_eq!(
                tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
                vec!["files.list_dir", "files.trash", "files.open", "bus.unsubscribe"]
            );
        }
        core.pump(0.1);
        assert_eq!(core.state().status, Status::WaitingForTool);
        let ev = port.test_drain();
        let ServiceCall { call_id, tool, args } = match &ev[0] {
            PortEvent::Call(c) => c.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!((tool.as_str(), args.as_str()), ("list_dir", r#"{"path":"~"}"#));
        port.reply(ToolResult::ok(&call_id, "Desktop/ Documents/ Pictures/", "listed ~"));
        let events = core.pump(0.2);
        assert!(events.contains(&EngineEvent::Changed));
        {
            let m = model.lock().unwrap();
            assert_eq!(m.tool_results, vec![("m1".to_string(), "Desktop/ Documents/ Pictures/".to_string(), false)]);
        }
        assert_eq!(core.state().status, Status::Idle);
        let entries = &core.state().entries;
        assert!(matches!(&entries[0], Entry::User { .. }));
        assert!(matches!(&entries[1], Entry::Tool(t) if t.title.starts_with("Files · list_dir") && matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. })));
        assert!(matches!(&entries[2], Entry::Assistant { text, streaming: false } if text == "Three folders."));
    }

    #[test]
    fn destructive_calls_wait_for_the_person() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.trash", r#"{"path":"~/old.txt"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Done."),
        ]);
        core.send("trash old.txt", 0.0);
        let events = core.pump(0.1);
        assert!(events.contains(&EngineEvent::Confirm { call_id: "m1".into() }));
        assert!(port.test_drain().is_empty(), "nothing reached the app yet");
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Confirm)));
        core.confirm("m1", false, 0.2);
        assert_eq!(model.lock().unwrap().tool_results[0].2, true, "denied is an error to the model");
        assert!(port.test_drain().is_empty());
        // The floor makes a Read tool destructive too.
        let (mut core, _model, mut port, _e) = setup(vec![vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }]]);
        core.registry().set_risk_floor("files", Risk::Destructive);
        core.send("list", 0.0);
        assert!(core.pump(0.1).contains(&EngineEvent::Confirm { call_id: "m1".into() }));
        core.confirm("m1", true, 0.2);
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "list_dir"));
    }

    #[test]
    fn a_silent_service_times_out_and_progress_buys_time() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Sorry."),
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        port.progress("m1", "walking", 300);
        core.pump(50.0);
        assert!(core.pump(100.0).is_empty() || model.lock().unwrap().tool_results.is_empty(), "progress at t=50 moved the deadline to 110");
        core.pump(111.0);
        let results = model.lock().unwrap().tool_results.clone();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.starts_with("[timed_out]"), "{results:?}");
        assert!(matches!(&port.test_drain()[0], PortEvent::Cancel { call_id } if call_id == "m1"));
    }

    #[test]
    fn unknown_names_get_the_real_ones_and_a_missing_app_is_unavailable() {
        let (mut core, model, _port, _e) = setup(vec![
            vec![call("files.delete_everything", "{}"), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![call("route.plan", r#"{"to":"Utrecht"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![call("plan", "{}"), ModelEvent::TurnDone { tool_calls: 1 }],
            done("ok"),
        ]);
        core.registry().mark_launchable("route", "Route");
        core.send("go", 0.0);
        for i in 1..=4 {
            core.pump(i as f64 * 0.1);
        }
        let r = model.lock().unwrap().tool_results.clone();
        assert!(r[0].1.contains("files.list_dir") && r[0].1.starts_with("[refused]"), "{r:?}");
        assert!(r[1].1.contains("os.launch") && r[1].1.starts_with("[unavailable]"), "{r:?}");
        assert!(r[2].1.contains("whole dotted name") && r[2].1.contains("files"), "{r:?}");
    }

    #[test]
    fn with_two_instances_the_named_one_wins_and_the_selector_is_stripped() {
        let reg = ServiceRegistry::new();
        let (mut p1, l1) = AiServicePort::in_process(files()).unwrap();
        let (mut p2, l2) = AiServicePort::in_process(files()).unwrap();
        let _e1 = reg.register(l1, "left", None).unwrap();
        let e2 = reg.register(l2, "right", None).unwrap();
        reg.pump();
        p1.test_drain();
        p2.test_drain();
        reg.focus(&e2);
        let (model, _shared) = scripted(vec![
            vec![call("files.list_dir", r#"{"path":"~","instance":"Files"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        let mut core = EngineCore::new(reg, model, None, 1);
        core.send("list", 0.0);
        core.pump(0.1);
        let got = p1.test_drain();
        assert!(matches!(&got[0], PortEvent::Call(c) if c.args == r#"{"path":"~"}"#), "{got:?}");
        assert!(p2.test_drain().is_empty(), "the focused one loses to the named one");
    }

    #[test]
    fn a_registry_change_reconfigures_the_model_between_turns_or_restarts_it() {
        let (mut core, model, _port, _e) = setup(vec![done("hi"), done("again")]);
        core.send("hello", 0.0);
        core.pump(0.1);
        assert_eq!(model.lock().unwrap().configured.len(), 1);
        core.registry().mark_launchable("route", "Route");
        core.pump(0.2);
        assert_eq!(model.lock().unwrap().configured.len(), 2, "idle: reconfigured at once");
        assert!(model.lock().unwrap().configured[1].0.contains("os.launch"));
        model.lock().unwrap().fail_configure_once = true;
        core.registry().mark_launchable("photos", "Photos");
        core.send("again", 0.3);
        let m = model.lock().unwrap();
        assert_eq!(m.resets, 1, "a model that cannot rebind is restarted");
        assert_eq!(m.configured.len(), 4);
        drop(m);
        assert!(core.state().entries.iter().any(|e| matches!(e, Entry::System { text } if text.contains("restarted"))));
    }

    #[test]
    fn the_console_drives_tools_without_a_model() {
        let (mut core, model, mut port, _e) = setup(vec![]);
        core.send("/", 0.0);
        assert!(matches!(core.state().entries.last(), Some(Entry::System { text }) if text.contains("files.list_dir")));
        core.send(r#"/files.open {"path":"~/Pictures"}"#, 0.0);
        let ev = port.test_drain();
        let id = match &ev[0] {
            PortEvent::Call(c) => c.call_id.clone(),
            other => panic!("{other:?}"),
        };
        port.reply(ToolResult::ok(&id, "showing ~/Pictures", "opened").with_preview());
        core.pump(0.1);
        assert!(model.lock().unwrap().tool_results.is_empty(), "console results never reach the model");
        assert!(core.state().entries.iter().any(|e| matches!(e, Entry::Tool(t) if t.preview && matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. }))));
        // A destructive console call runs without a confirm card: the person typed it.
        core.send(r#"/files.trash {"path":"~/x"}"#, 0.0);
        assert!(matches!(&port.test_drain()[0], PortEvent::Call(c) if c.tool == "trash"));
    }

    #[test]
    fn an_end_turn_disposition_stops_the_model_and_a_reset_restarts_it() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.open", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("should not stream"),
        ]);
        core.send("open home", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        port.reply(ToolResult::ok("m1", "opened", "opened").with_disposition(Disposition::EndTurn));
        core.pump(0.2);
        assert_eq!(core.state().status, Status::Idle);
        assert_eq!(model.lock().unwrap().cancels, 1);
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.open", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        core.send("open", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        port.reply(ToolResult::ok("m1", "new world", "").with_disposition(Disposition::ResetConversation));
        core.pump(0.2);
        assert_eq!(model.lock().unwrap().resets, 1);
        assert!(matches!(core.state().entries.last(), Some(Entry::System { text }) if text.contains("reset")));
    }

    #[test]
    fn the_tool_round_limit_ends_a_runaway_turn() {
        let mut script: Vec<Vec<ModelEvent>> = Vec::new();
        for i in 0..(MAX_TOOL_ROUNDS + 1) {
            script.push(vec![ModelEvent::ToolCall { call_id: format!("m{i}"), name: "files.list_dir".into(), args: r#"{"path":"~"}"#.into() }, ModelEvent::TurnDone { tool_calls: 1 }]);
        }
        let (mut core, model, mut port, _e) = setup(script);
        core.send("loop", 0.0);
        for t in 0..(MAX_TOOL_ROUNDS + 2) {
            core.pump(t as f64);
            for ev in port.test_drain() {
                if let PortEvent::Call(c) = ev {
                    port.reply(ToolResult::ok(&c.call_id, "x", ""));
                }
            }
            core.pump(t as f64 + 0.5);
        }
        let m = model.lock().unwrap();
        assert!(m.tool_results.iter().any(|(_, text, _)| text.contains("round limit")), "{:?}", m.tool_results.last());
        assert_eq!(m.cancels, 1);
        drop(m);
        assert_eq!(core.state().status, Status::Idle);
    }

    #[test]
    fn cancel_and_clear_leave_nothing_pending() {
        let (mut core, model, mut port, _e) = setup(vec![
            vec![call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        let _ = port.test_drain();
        core.cancel(0.2);
        assert!(matches!(&port.test_drain()[0], PortEvent::Cancel { .. }));
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Done { outcome: ToolOutcome::Cancelled, .. })));
        assert_eq!(core.state().status, Status::Idle);
        core.clear(0.3);
        assert!(core.state().entries.is_empty());
        assert_eq!(model.lock().unwrap().resets, 1);
    }

    fn os() -> ServiceManifest {
        ServiceManifest::new("os", "Desktop", "The desktop.")
            .with_tool(ToolDef::new("launch", "Start an app.", r#"{"type":"object","properties":{"app":{"type":"string"}},"required":["app"]}"#, Risk::Act))
    }

    fn photos() -> ServiceManifest {
        ServiceManifest::new("photos", "Photos", "The picture wall.")
            .with_tool(ToolDef::new("search", "Find pictures.", r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"#, Risk::Read))
    }

    #[test]
    fn a_turn_of_only_a_tool_call_leaves_no_empty_assistant_row() {
        let (mut core, _model, mut port, _e) = setup(vec![
            vec![ModelEvent::Delta("\n\n".into()), call("files.list_dir", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Three folders."),
        ]);
        core.send("list", 0.0);
        core.pump(0.1);
        assert!(!core.state().entries.iter().any(|e| matches!(e, Entry::Assistant { .. })), "a whitespace delta opens no row: {:?}", core.state().entries);
        let ev = port.test_drain();
        let PortEvent::Call(c) = &ev[0] else { panic!() };
        port.reply(ToolResult::ok(&c.call_id, "a b c", ""));
        core.pump(0.2);
        let assistant: Vec<&Entry> = core.state().entries.iter().filter(|e| matches!(e, Entry::Assistant { .. })).collect();
        assert_eq!(assistant.len(), 1, "exactly one row, the text: {:?}", core.state().entries);
        assert!(matches!(assistant[0], Entry::Assistant { text, streaming: false } if text == "Three folders."));
    }

    #[test]
    fn a_launch_waits_for_the_app_and_the_turn_goes_on_with_its_tools() {
        let reg = ServiceRegistry::new();
        let (mut os_port, os_link) = AiServicePort::in_process(os()).unwrap();
        reg.register(os_link, "the desktop", None).unwrap();
        reg.mark_launchable("photos", "Photos");
        reg.pump();
        os_port.test_drain();
        let (model, shared) = scripted(vec![
            vec![call("os.launch", r#"{"app":"photos"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            vec![ModelEvent::ToolCall { call_id: "m2".into(), name: "photos.search".into(), args: r#"{"query":"dogs"}"#.into() }, ModelEvent::TurnDone { tool_calls: 1 }],
            done("Two dog comics."),
        ]);
        let mut core = EngineCore::new(reg.clone(), model, None, 1);
        core.send("find the pictures about dogs", 0.0);
        core.pump(0.1);
        let ev = os_port.test_drain();
        let PortEvent::Call(c) = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(c.tool, "launch");
        os_port.reply(ToolResult::ok(&c.call_id, "Photos is starting", "starting"));
        core.pump(0.5);
        assert!(shared.lock().unwrap().tool_results.is_empty(), "the launch result is held until photos registers");
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Running { .. })));
        // Photos comes up two seconds later.
        let (mut photos_port, photos_link) = AiServicePort::in_process(photos()).unwrap();
        reg.register(photos_link, "a tile", None).unwrap();
        core.pump(2.0);
        {
            let m = shared.lock().unwrap();
            assert_eq!(m.tool_results.len(), 1);
            let (id, text, is_error) = &m.tool_results[0];
            assert_eq!(id, "m1");
            assert!(!is_error);
            assert!(text.contains("Photos is running now") && text.contains("photos.search"), "{text}");
        }
        // ...and the same turn calls photos.search.
        let ev = photos_port.test_drain();
        let Some(PortEvent::Call(c)) = ev.iter().find(|e| matches!(e, PortEvent::Call(_))) else { panic!("{ev:?}") };
        assert_eq!(c.tool, "search");
        photos_port.reply(ToolResult::ok(&c.call_id, "2 matches", ""));
        core.pump(2.5);
        assert_eq!(core.state().status, Status::Idle);
        assert!(matches!(core.state().entries.last(), Some(Entry::Assistant { text, .. }) if text == "Two dog comics."));
    }

    #[test]
    fn a_launch_nobody_answers_goes_to_the_model_after_the_patience() {
        let reg = ServiceRegistry::new();
        let (mut os_port, os_link) = AiServicePort::in_process(os()).unwrap();
        reg.register(os_link, "the desktop", None).unwrap();
        reg.mark_launchable("photos", "Photos");
        reg.pump();
        os_port.test_drain();
        let (model, shared) = scripted(vec![
            vec![call("os.launch", r#"{"app":"photos"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Photos did not come up."),
        ]);
        let mut core = EngineCore::new(reg, model, None, 1);
        core.send("open photos", 0.0);
        core.pump(0.1);
        let ev = os_port.test_drain();
        let PortEvent::Call(c) = &ev[0] else { panic!() };
        os_port.reply(ToolResult::ok(&c.call_id, "Photos is starting", "starting"));
        core.pump(1.0);
        assert!(shared.lock().unwrap().tool_results.is_empty());
        core.pump(LAUNCH_PATIENCE_SECS + 1.5);
        {
            let m = shared.lock().unwrap();
            assert_eq!(m.tool_results.len(), 1);
            assert!(m.tool_results[0].1.contains("has not connected yet"), "{}", m.tool_results[0].1);
        }
        assert!(matches!(core.state().tool("m1").map(|t| t.status.clone()), Some(ToolStatus::Done { note, .. }) if note == "not connected yet"));
    }

    #[test]
    fn the_dynamic_context_names_what_runs_and_what_needs_a_launch() {
        let (core, _model, _port, _e) = setup(vec![]);
        core.registry().mark_launchable("photos", "Photos");
        let ctx = core.registry().dynamic_context();
        assert!(ctx.contains("Running now — call their tools directly: Files (files.list_dir, files.trash, files.open)"), "{ctx}");
        assert!(ctx.contains("Not running — os.launch starts them: Photos (`photos`)"), "{ctx}");
    }

    #[test]
    fn a_tool_result_subscribes_the_port_and_an_idle_message_wakes_as_an_event() {
        let (mut core, model, mut port, _endpoint) = setup(vec![
            vec![call("files.open", r#"{"path":"~"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Watching."),
            done("I saw it."),
        ]);
        core.send("watch home", 0.0);
        core.pump(0.1);
        let call = match &port.test_drain()[0] {
            PortEvent::Call(call) => call.clone(),
            other => panic!("{other:?}"),
        };
        port.reply(
            ToolResult::ok(&call.call_id, "watch started", "watching")
                .with_subscription(SubscriptionRequest::new("watch").with_filter(r#"{"path":"~"}"#)),
        );
        core.pump(0.2);
        let subscribed = port.test_drain();
        let sub_id = match &subscribed[0] {
            PortEvent::Subscribe { sub_id, topic, filter } => {
                assert_eq!(topic, "watch");
                assert_eq!(filter.as_deref(), Some(r#"{"path":"~"}"#));
                sub_id.clone()
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(core.subscription_count(), 1);
        port.publish(&sub_id, Message::new("watch", "notes.txt changed"));
        core.pump(0.3);
        let m = model.lock().unwrap();
        assert_eq!(m.sends.len(), 2, "the idle publication starts exactly one new turn");
        assert!(m.sends[1].0.starts_with("[event] Files · watch · notes.txt changed"));
        assert!(m.sends[1].0.contains(&format!("sub_id: {sub_id}")));
        assert!(m.configured.last().unwrap().0.contains(&format!("watch (`{sub_id}`)")));
        drop(m);
        assert!(matches!(
            core.state().entries.iter().find(|entry| matches!(entry, Entry::Event(_))),
            Some(Entry::Event(event)) if event.sub_id == sub_id && event.text == "notes.txt changed"
        ));
    }

    fn add_subscription(core: &mut EngineCore, endpoint: &EndpointId, now: f64) -> String {
        let mut result = ToolResult::ok("seed", "watching", "")
            .with_subscription(SubscriptionRequest::new("watch"));
        core.install_subscriptions(endpoint, &mut result, now);
        assert!(result.outcome.is_ok(), "{result:?}");
        core.subscription_order.last().unwrap().clone()
    }

    #[test]
    fn a_busy_turn_coalesces_per_subscription_and_reports_drops() {
        let (mut core, model, mut port, endpoint) = setup(vec![vec![], done("Noted.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.send("keep working", 0.0);
        for index in 0..3 {
            port.publish(&sub_id, Message::new("watch", format!("change {index}")));
        }
        core.pump(0.1);
        assert_eq!(core.queued_message_count(), 1);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "a busy turn is not interrupted");
        model.lock().unwrap().out.push(ModelEvent::TurnDone { tool_calls: 0 });
        core.pump(0.2);
        let m = model.lock().unwrap();
        assert_eq!(m.sends.len(), 2);
        assert!(m.sends[1].0.contains("change 2") && m.sends[1].0.contains("dropped: 2"), "{:?}", m.sends[1]);
        drop(m);
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event)) if event.dropped == 2 && event.text == "change 2"
        ));
    }

    #[test]
    fn five_idle_messages_in_a_burst_obey_the_two_second_wake_law() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("one"), done("two")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        for index in 0..5 {
            port.publish(&sub_id, Message::new("watch", format!("change {index}")));
        }
        core.pump(0.1);
        assert_eq!(model.lock().unwrap().sends.len(), 1);
        assert_eq!(core.queued_message_count(), 4);
        core.pump(0.2);
        core.pump(1.0);
        core.pump(2.09);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "no second wake inside two seconds");
        core.pump(2.11);
        assert_eq!(model.lock().unwrap().sends.len(), 2);
        assert_eq!(core.queued_message_count(), 3);
    }

    #[test]
    fn a_subscription_queue_keeps_sixty_four_and_reports_overflow() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Noted.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        for index in 0..(MAX_SUBSCRIPTION_QUEUE + 1) {
            port.publish(&sub_id, Message::new("watch", format!("change {index}")));
        }
        core.pump(0.1);
        assert_eq!(core.queued_message_count(), MAX_SUBSCRIPTION_QUEUE - 1);
        let input = &model.lock().unwrap().sends[0].0;
        assert!(input.contains("change 1") && input.contains("dropped: 1"), "{input}");
    }

    #[test]
    fn a_final_message_delivers_once_and_closes_the_subscription() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Complete.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        port.publish(&sub_id, Message::new("watch", "finished").final_message());
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert_eq!(model.lock().unwrap().sends.len(), 1);
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event)) if event.final_ && event.text == "finished"
        ));
        port.publish(&sub_id, Message::new("watch", "after final"));
        core.pump(3.0);
        assert_eq!(model.lock().unwrap().sends.len(), 1);
    }

    #[test]
    fn a_queued_message_then_final_keeps_only_the_final_while_busy() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Complete.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.turn_active = true;
        port.publish(&sub_id, Message::new("watch", "working"));
        port.publish(&sub_id, Message::new("watch", "finished").final_message());
        core.pump(0.1);
        assert_eq!(core.queued_message_count(), 1);
        core.turn_active = false;
        core.deliver_next_message(0.2);
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event))
                if event.final_ && event.text == "finished" && event.dropped == 1
        ));
        assert_eq!(model.lock().unwrap().sends.len(), 1);
    }

    #[test]
    fn a_pending_final_is_removed_from_the_next_turns_prompt() {
        let (mut core, model, mut port, endpoint) = setup(vec![vec![]]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.pump(0.0);
        assert!(model.lock().unwrap().configured.last().unwrap().0.contains(&sub_id));
        core.last_wake = Some(0.0);
        port.publish(&sub_id, Message::new("watch", "finished").final_message());
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert_eq!(core.queued_message_count(), 1, "the rate-limited final remains bounded until delivery");
        assert!(!model.lock().unwrap().configured.last().unwrap().0.contains(&sub_id));
        core.send("what is live now?", 0.2);
        assert!(!model.lock().unwrap().configured.last().unwrap().0.contains(&sub_id));
    }

    #[test]
    fn bus_unsubscribe_ends_delivery() {
        let (mut core, model, mut port, endpoint) = setup(vec![
            vec![call("bus.unsubscribe", r#"{"sub_id":"l1-s1"}"#), ModelEvent::TurnDone { tool_calls: 1 }],
            done("Stopped."),
        ]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        assert_eq!(sub_id, "l1-s1");
        let _ = port.test_drain();
        core.send("stop the updates", 0.0);
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id }] if sub_id == "l1-s1"
        ));
        port.publish("l1-s1", Message::new("watch", "too late"));
        core.pump(3.0);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "an ended subscription cannot wake the model");
        assert!(matches!(
            core.state().tool("m1").map(|tool| &tool.status),
            Some(ToolStatus::Done { outcome: ToolOutcome::Ok, .. })
        ));
    }

    #[test]
    fn clearing_the_conversation_drops_its_subscription_table() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        core.clear(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id: ended }] if ended == &sub_id
        ));
    }

    #[test]
    fn clear_discards_publications_already_queued_upstream() {
        let (mut core, model, mut port, endpoint) = setup(vec![]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        port.publish(&sub_id, Message::new("watch", "queued before clear"));
        core.clear(0.1);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id: ended }] if ended == &sub_id
        ));
        core.pump(0.2);
        assert!(model.lock().unwrap().sends.is_empty());
        assert!(!core.state().entries.iter().any(|entry| matches!(entry, Entry::Event(_))));
    }

    #[test]
    fn ending_the_engine_lease_unsubscribes_every_service() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        drop(core);
        assert!(matches!(
            port.test_drain().as_slice(),
            [PortEvent::Unsubscribe { sub_id: ended }] if ended == &sub_id
        ));
    }

    #[test]
    fn a_seventeenth_subscription_is_a_typed_refusal() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        for index in 0..MAX_SUBSCRIPTIONS {
            let sub_id = add_subscription(&mut core, &endpoint, index as f64);
            assert_eq!(sub_id, format!("l1-s{}", index + 1));
        }
        let _ = port.test_drain();
        let mut overflow = ToolResult::ok("overflow", "watching", "")
            .with_subscription(SubscriptionRequest::new("watch"));
        core.install_subscriptions(&endpoint, &mut overflow, 17.0);
        assert_eq!(overflow.outcome, ToolOutcome::Refused);
        assert!(overflow.text.contains("subscription limit"));
        assert_eq!(core.subscription_count(), MAX_SUBSCRIPTIONS);
        assert!(port.test_drain().is_empty(), "nothing is sent for the refused seventeenth subscription");
    }

    #[test]
    fn pending_finals_cannot_expand_retained_subscription_state() {
        let (mut core, _model, mut port, endpoint) = setup(vec![]);
        let mut sub_ids = Vec::new();
        for index in 0..MAX_SUBSCRIPTIONS {
            sub_ids.push(add_subscription(&mut core, &endpoint, index as f64));
        }
        let _ = port.test_drain();
        core.turn_active = true;
        for sub_id in &sub_ids {
            port.publish(sub_id, Message::new("watch", "finished").final_message());
        }
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert_eq!(core.subscriptions.len(), MAX_SUBSCRIPTIONS);
        assert_eq!(core.queued_message_count(), MAX_SUBSCRIPTIONS);
        for attempt in 0..MAX_SUBSCRIPTIONS {
            let mut next = ToolResult::ok(format!("next-{attempt}"), "watching", "")
                .with_subscription(SubscriptionRequest::new("watch"));
            core.install_subscriptions(&endpoint, &mut next, 1.0 + attempt as f64);
            assert_eq!(next.outcome, ToolOutcome::Refused);
        }
        assert_eq!(core.subscriptions.len(), MAX_SUBSCRIPTIONS);
        assert!(core.queued_message_count() <= MAX_SUBSCRIPTIONS * MAX_SUBSCRIPTION_QUEUE);
        assert!(port.test_drain().is_empty(), "refused subscriptions send no frames");
    }

    #[test]
    fn subscription_ids_are_namespaced_by_conversation_lease() {
        let (mut first, _model, mut first_port, first_endpoint) = setup_with_lease(vec![], 0x2a);
        let first_id = add_subscription(&mut first, &first_endpoint, 0.0);
        let _ = first_port.test_drain();
        let (mut second, _model, mut second_port, second_endpoint) = setup_with_lease(vec![], 0x2b);
        let second_id = add_subscription(&mut second, &second_endpoint, 0.0);
        let _ = second_port.test_drain();
        assert_eq!(first_id, "l2a-s1");
        assert_eq!(second_id, "l2b-s1");
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn a_service_unregister_is_delivered_as_a_synthetic_final_event() {
        let (mut core, model, mut port, endpoint) = setup(vec![done("Gone.")]);
        let sub_id = add_subscription(&mut core, &endpoint, 0.0);
        let _ = port.test_drain();
        port.unregister();
        port.publish(&sub_id, Message::new("watch", "after unregister"));
        core.pump(0.1);
        assert_eq!(core.subscription_count(), 0);
        assert!(model.lock().unwrap().sends[0].0.contains("service disconnected"));
        assert!(matches!(
            core.state().entries.last(),
            Some(Entry::Event(event)) if event.sub_id == sub_id && event.final_
        ));
        core.pump(3.0);
        assert_eq!(model.lock().unwrap().sends.len(), 1, "the synthetic final is emitted once");
    }
}
