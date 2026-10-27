//! The window's live runtime: the one [`LiveHost`] of this process and the
//! owner runtimes and frame producers that go with it, whichever component
//! libraries this build linked (see `components`).
//!
//! It is driven by instances, not by what the window shows. A revision of an
//! instance's flow is applied when the app learns it; the instance keeps
//! running when another flow, package or pane is opened, and stops when the
//! instance is deleted or the app shuts down. Every live instance sounds
//! through the one engine the device callback owns.
//!
//! The device is opened the first time a revision with audio nodes is
//! applied, and its callback is installed exactly once. Every output gate is
//! closed until something writes it open.

use makepad_flow::document::binding::{DeclaredEndpoint, EndpointBinding};
use makepad_flow::document::ident::InstanceId;
use makepad_flow::document::Registry;
use makepad_flow::live::{ApplyRevision, EndpointKind, EndpointValue, LiveEvent, LiveHost, LiveLimits, ProbeValue, SessionKey, WriteRefusal, WriteTarget};
use makepad_flow::client::{ClientError, FlowClient};
use makepad_flow::Graph;
use crate::components::{self, AudioOutput, FrameProducer, ProducerTick};
use makepad_widgets::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

const MAX_PENDING_WRITES: usize = 256;
const MAX_INSTANCE_WRITES: usize = 64;
// One release per host session remains admissible when ordinary traffic is full.
const MAX_PENDING_RELEASES: usize = 8;
const WRITES_PER_PUMP: usize = 64;
const MAX_WRITE_REFUSALS: usize = 32;

struct PendingWrite {
    target: WriteTarget,
    value: EndpointValue,
    release: bool,
}

/// Complete authoritative instance listing, assembled off the UI thread.
/// Unchanged graph payloads are shared across instances and refreshes.
pub struct LiveSnapshot {
    server_id: [u8; 16],
    observed_at: Instant,
    instances: Vec<(String, String)>,
    definitions: HashMap<String, LiveDefinition>,
}

struct LiveDefinition {
    revision: u64,
    graph: Option<Arc<Graph>>,
    error: Option<String>,
}

impl LiveSnapshot {
    pub fn capture(client: &FlowClient, previous: Option<&Self>) -> Result<Self, ClientError> {
        let observed_at = Instant::now();
        let server_id = client.server_id();
        let previous = previous.filter(|old| old.server_id == server_id);
        let rows = client.instances(None, false)?;
        let flows = client.flows()?;
        // Refuse an oversized listing whole; a truncated listing must not
        // stop unseen instances. These are the engine's default limits.
        if rows.len() > 1024 || flows.len() > 256 {
            return Err(ClientError::Protocol("live snapshot exceeds 1024 instances or 256 definitions".into()));
        }
        let instances: Vec<_> = rows.into_iter().filter(|row| row.live)
            .map(|row| (row.instance, row.flow)).collect();
        let used: HashSet<_> = instances.iter().map(|(_, flow)| flow.as_str()).collect();
        let previous_instances: HashMap<_, _> = previous.into_iter()
            .flat_map(|snapshot| snapshot.instances.iter())
            .map(|(id, flow)| (id.as_str(), flow.as_str())).collect();
        // A deleted/recreated definition may reuse its name and revision,
        // but all its old instances disappear. Do not reuse that graph.
        let retained_flows: HashSet<_> = instances.iter()
            .filter(|(id, flow)| previous_instances.get(id.as_str()) == Some(&flow.as_str()))
            .map(|(_, flow)| flow.as_str()).collect();
        let mut definitions = HashMap::new();
        for flow in flows.into_iter().filter(|flow| used.contains(flow.name.as_str())) {
            let definition = if let Some(error) = flow.error {
                LiveDefinition { revision: flow.revision, graph: None, error: Some(error.to_string()) }
            } else if let Some(cached) = previous.and_then(|old| old.definitions.get(&flow.name))
                .filter(|old| retained_flows.contains(flow.name.as_str())
                    && old.revision == flow.revision && old.error.is_none() && old.graph.is_some())
            {
                LiveDefinition { revision: cached.revision, graph: cached.graph.clone(), error: None }
            } else {
                let current = client.flow(&flow.name)?;
                LiveDefinition {
                    revision: current.revision,
                    graph: current.graph.map(Arc::new),
                    error: current.error.map(|error| error.to_string()),
                }
            };
            definitions.insert(flow.name, definition);
        }
        Ok(Self { server_id, observed_at, instances, definitions })
    }
}

#[derive(Default)]
pub struct LiveRuntime {
    host: Option<LiveHost>,
    audio: Option<AudioOutput>,
    /// The linked frame producers, in the order the stage draws them.
    producers: Vec<Rc<dyn FrameProducer>>,
    /// Frame owners can retire resources after the host retires their identity.
    frames_busy: bool,
    pending_writes: VecDeque<PendingWrite>,
    write_refusals: VecDeque<String>,
    omitted_refusals: usize,
    /// The device callback owns the engine; never installed twice.
    audio_installed: bool,
    /// Why there is no host, when building one failed.
    error: Option<String>,
    instance_errors: HashMap<String, String>,
    /// Avoid restarting a locally retired instance while its delete is queued.
    stopped: HashMap<(Option<[u8; 16]>, String), Instant>,
    default_outputs: Vec<AudioDeviceId>,
    origin: Option<[u8; 16]>,
    pending_origin: Option<[u8; 16]>,
}

impl LiveRuntime {
    fn host(&mut self, cx: &mut Cx, registry: &Arc<Registry>) -> Option<&mut LiveHost> {
        if self.host.is_none() && self.error.is_none() {
            let owners = match components::owners(cx) {
                Ok(owners) => owners,
                Err(error) => {
                    self.error = Some(error);
                    return None;
                }
            };
            match LiveHost::new(registry.clone(), owners.owners, LiveLimits::default()) {
                Ok(host) => {
                    self.host = Some(host);
                    self.audio = owners.audio;
                    self.producers = owners.producers;
                }
                Err(error) => self.error = Some(error),
            }
        }
        self.host.as_mut()
    }

    /// Make `graph` (revision `revision` of `definition`) what `instance`
    /// runs live, unless it already is or is already on its way. A flow
    /// without live nodes that never ran live is left alone.
    fn sync(&mut self, cx: &mut Cx, registry: &Arc<Registry>, instance: &str, definition: &str, revision: u64, graph: &Arc<Graph>) {
        let Some(host) = self.host(cx, registry) else { return };
        let id = InstanceId(instance.to_string());
        if host.applied_revision(&id).is_some_and(|(name, applied)| name == definition && applied == revision) {
            return;
        }
        if host.applied_revision(&id).is_none() && !host.has_live_nodes(graph) {
            return;
        }
        let request = ApplyRevision { instance: id, definition: definition.to_string(), revision, authored: graph.clone() };
        if let Err(error) = host.apply(request) {
            self.instance_errors.insert(instance.to_string(), error);
        } else {
            self.instance_errors.remove(instance);
        }
    }

    /// Reconcile unpinned engine instances independently of editor selection.
    /// Failed evaluations retain their last good running generation.
    pub fn reconcile(&mut self, cx: &mut Cx, registry: &Arc<Registry>, snapshot: &LiveSnapshot) {
        if self.origin != Some(snapshot.server_id) || self.pending_origin.is_some() {
            if self.pending_origin != Some(snapshot.server_id) {
                self.stop_all();
                self.pending_origin = Some(snapshot.server_id);
            }
            if self.is_running() { return; }
            if let Some(host) = self.host.as_mut() {
                if let Err(error) = host.replace_registry(registry.clone()) {
                    self.error = Some(error);
                    return;
                }
            }
            self.origin = Some(snapshot.server_id);
            self.pending_origin = None;
            self.instance_errors.clear();
        }
        let present: HashSet<_> = snapshot.instances.iter().map(|(id, _)| id.as_str()).collect();
        self.stopped.retain(|(origin, id), stopped_at| {
            (origin.is_none() || *origin == Some(snapshot.server_id))
                && (present.contains(id.as_str()) || snapshot.observed_at <= *stopped_at)
        });
        self.instance_errors.retain(|id, _| present.contains(id.as_str()));
        if let Some(host) = self.host.as_mut() {
            let removed: Vec<_> = host.instances().filter(|id| !present.contains(id.0.as_str())).cloned().collect();
            for id in removed {
                self.pending_writes.retain(|write| write.target.instance() != &id);
                host.stop(&id);
                self.frames_busy |= !self.producers.is_empty();
            }
        }
        for (instance, name) in &snapshot.instances {
            if self.stopped.contains_key(&(Some(snapshot.server_id), instance.clone()))
                || self.stopped.contains_key(&(None, instance.clone())) { continue; }
            let Some(definition) = snapshot.definitions.get(name) else { continue };
            if let Some(error) = &definition.error {
                self.instance_errors.insert(instance.clone(), format!("revision {}: {error}", definition.revision));
            } else if let Some(graph) = &definition.graph {
                self.instance_errors.remove(instance);
                self.sync(cx, registry, instance, name, definition.revision, graph);
            }
        }
    }

    /// Advance the host and open the device when a revision wants one.
    pub fn pump(&mut self, cx: &mut Cx) -> Vec<LiveEvent> {
        let Some(host) = self.host.as_mut() else { return Vec::new() };
        let events = host.pump();
        for event in &events {
            log!("[flow-live] {event:?}");
        }
        self.pump_writes();
        if !self.audio_installed {
            if let Some(audio) = self.audio.as_ref() {
                if components::install_audio(cx, audio) {
                    self.audio_installed = true;
                    if !self.default_outputs.is_empty() {
                        cx.use_audio_outputs(&self.default_outputs);
                    }
                }
            }
        }
        events
    }

    /// The platform listed its devices: play through the default output,
    /// if this runtime opened the audio layer.
    pub fn audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        self.default_outputs = devices.default_output();
        if self.audio_installed {
            cx.use_audio_outputs(&self.default_outputs);
        }
    }

    pub fn is_running(&self) -> bool {
        self.frames_busy || !self.pending_writes.is_empty()
            || self.host.as_ref().is_some_and(|host| host.instances().next().is_some())
    }

    /// The linked frame producers, for the stage that draws them and the
    /// previews that bind their outputs.
    pub fn producers(&self) -> Vec<Rc<dyn FrameProducer>> {
        self.producers.clone()
    }

    /// Tick every producer even when no view shows it: installation and
    /// charged retirement must keep progressing. GPU encoding is the app's
    /// draw hook through the cloned producer handles.
    pub fn tick_frames(&mut self, cx: &mut Cx, now: f64) -> bool {
        let mut tick = ProducerTick::default();
        for producer in &self.producers {
            let one = producer.tick(cx, now);
            tick.busy |= one.busy;
            tick.wants_frame |= one.wants_frame;
        }
        self.frames_busy = tick.busy;
        self.frames_busy || tick.wants_frame
    }

    pub fn stop(&mut self, instance: &str, origin: Option<[u8; 16]>) {
        self.stopped.insert((origin.or(self.origin), instance.to_string()), Instant::now());
        self.pending_writes.retain(|write| write.target.instance().0 != instance);
        if let Some(host) = self.host.as_mut() {
            host.stop(&InstanceId(instance.to_string()));
            self.frames_busy |= !self.producers.is_empty();
        }
    }

    pub fn stop_all(&mut self) {
        self.pending_writes.clear();
        if let Some(host) = self.host.as_mut() {
            let instances: Vec<InstanceId> = host.instances().cloned().collect();
            for instance in instances {
                host.stop(&instance);
            }
            // Dispatch now: process exit may not give us another UI frame.
            // Completion still depends on a subsequent audio callback.
            let _ = host.pump();
            self.frames_busy |= !self.producers.is_empty();
        }
    }

    /// The endpoints a surface or the inspector may bind for `instance`.
    pub fn declared(&self, instance: &str) -> Vec<DeclaredEndpoint> {
        let Some(host) = self.host.as_ref() else { return Vec::new() };
        host.endpoints(&InstanceId(instance.to_string()))
            .iter()
            .map(|endpoint| {
                let class = match endpoint.kind {
                    EndpointKind::Control { .. } => EndpointBinding::Control,
                    EndpointKind::Toggle { .. } => EndpointBinding::Toggle,
                    EndpointKind::Probe { .. } => EndpointBinding::Probe,
                    EndpointKind::Trigger => EndpointBinding::Trigger,
                };
                DeclaredEndpoint {
                    node: makepad_flow::live::plan::qualified(&endpoint.addr.node),
                    name: endpoint.addr.param.clone(),
                    class,
                }
            })
            .collect()
    }

    pub fn write(&mut self, instance: &str, node: &str, name: &str, value: f64) -> Result<Option<f64>, WriteRefusal> {
        self.submit_write(instance, node, name, EndpointValue::Number(value), false)
    }

    pub fn pulse(&mut self, instance: &str, node: &str, name: &str) -> Result<Option<f64>, WriteRefusal> {
        self.submit_write(instance, node, name, EndpointValue::Pulse, false)
    }

    fn submit_write(&mut self, instance: &str, node: &str, name: &str, value: EndpointValue, release: bool) -> Result<Option<f64>, WriteRefusal> {
        let host = self.host.as_mut().ok_or(WriteRefusal::NotRunning)?;
        let target = host.resolve_write(&InstanceId(instance.to_string()), node, name)?;
        // Validate before retaining behind another command. Normalization is
        // still owned by LiveHost::write_to and its exact admitted endpoint.
        let value = match (&target.endpoint().kind, value) {
            (EndpointKind::Control { .. }, EndpointValue::Number(value)) if value.is_finite() => EndpointValue::Number(value),
            (EndpointKind::Control { .. }, EndpointValue::Number(_)) => return Err(WriteRefusal::NotFinite),
            (EndpointKind::Control { .. }, _) => return Err(WriteRefusal::WrongKind { expected: "number" }),
            (EndpointKind::Toggle { .. }, EndpointValue::Number(value)) if value == 0.0 || value == 1.0 => EndpointValue::Toggle(value != 0.0),
            (EndpointKind::Toggle { .. }, value @ EndpointValue::Toggle(_)) => value,
            (EndpointKind::Toggle { .. }, _) => return Err(WriteRefusal::WrongKind { expected: "toggle" }),
            (EndpointKind::Trigger, value @ EndpointValue::Pulse) => value,
            (EndpointKind::Trigger, _) => return Err(WriteRefusal::WrongKind { expected: "pulse" }),
            (EndpointKind::Probe { .. }, _) => return Err(WriteRefusal::WrongKind { expected: "nothing: a probe is read-only" }),
        };
        if matches!(value, EndpointValue::Number(_) | EndpointValue::Toggle(_)) {
            if let Some(queued) = self.pending_writes.iter_mut().find(|queued| !queued.release && queued.target == target) {
                queued.value = value;
                return Ok(None);
            }
        }
        let route_pending = self.pending_writes.iter().any(|queued|
            queued.target.session() == target.session() && queued.target.endpoint().owner == target.endpoint().owner);
        // A new action never overtakes an older queued action for this owner.
        // Focus release deliberately bypasses that queue: it cancels input.
        if release || !route_pending {
            match host.write_to(&target, value) {
                Err(WriteRefusal::Busy) => {}
                result => return result,
            }
        }
        if release {
            if self.pending_writes.iter().filter(|write| write.release).count() >= MAX_PENDING_RELEASES {
                self.report_write_refusal(&target, "release retry queue is full; command was not accepted");
                return Err(WriteRefusal::Busy);
            }
            self.pending_writes.push_front(PendingWrite { target, value, release });
        } else {
            let total = self.pending_writes.iter().filter(|write| !write.release).count();
            let in_session = self.pending_writes.iter().filter(|write| !write.release && write.target.session() == target.session()).count();
            if total >= MAX_PENDING_WRITES || in_session >= MAX_INSTANCE_WRITES {
                self.report_write_refusal(&target, "input retry queue is full; command was not accepted");
                return Err(WriteRefusal::Busy);
            }
            self.pending_writes.push_back(PendingWrite { target, value, release });
        }
        // Accepted by the bounded app queue. Busy is returned only if this
        // queue could not retain it, so callers never retry an accepted pulse.
        Ok(None)
    }

    fn pump_writes(&mut self) {
        let mut blocked: HashSet<(SessionKey, String)> = HashSet::new();
        let mut attempts = 0;
        let count = self.pending_writes.len();
        for _ in 0..count {
            let Some(write) = self.pending_writes.pop_front() else { break };
            if attempts >= WRITES_PER_PUMP {
                // Finish rotating the bounded initial queue even after the
                // call quota. Leaving its tail at the front could put a
                // later pulse ahead of an earlier Busy pulse moved back.
                self.pending_writes.push_back(write);
                continue;
            }
            let route = (write.target.session().clone(), write.target.endpoint().owner.clone());
            if !write.release && blocked.contains(&route) {
                self.pending_writes.push_back(write);
                continue;
            }
            attempts += 1;
            let result = self.host.as_mut().ok_or(WriteRefusal::NotRunning)
                .and_then(|host| host.write_to(&write.target, write.value));
            match result {
                Ok(_) => {}
                Err(WriteRefusal::Busy) => {
                    blocked.insert(route);
                    self.pending_writes.push_back(write);
                }
                Err(refusal) => {
                    self.report_write_refusal(&write.target, refusal);
                }
            }
        }
    }

    fn report_write_refusal(&mut self, target: &WriteTarget, reason: impl std::fmt::Display) {
        let message = format!("live: `{}` instance `{}` generation {} was not written: {reason}",
            target.endpoint().qualified(), target.instance().0, target.generation().0);
        log!("[flow-live] {message}");
        if self.write_refusals.len() < MAX_WRITE_REFUSALS {
            self.write_refusals.push_back(message);
        } else {
            self.omitted_refusals = self.omitted_refusals.saturating_add(1);
        }
    }

    pub fn take_write_refusals(&mut self) -> Vec<String> {
        let mut refusals: Vec<_> = self.write_refusals.drain(..).collect();
        if self.omitted_refusals > 0 {
            refusals.push(format!("live: {} additional queued writes were refused", std::mem::take(&mut self.omitted_refusals)));
        }
        refusals
    }

    /// What `node.name` reads now: a probe's newest value, or the target a
    /// control was last given.
    pub fn read(&self, instance: &str, node: &str, name: &str) -> Option<f64> {
        let host = self.host.as_ref()?;
        let id = InstanceId(instance.to_string());
        match host.probe(&id, node, name) {
            Some(ProbeValue::Number(value)) => Some(value),
            Some(ProbeValue::Flag(on)) => Some(if on { 1.0 } else { 0.0 }),
            Some(ProbeValue::Audio(_)) => None,
            None => host.control(&id, node, name),
        }
    }

    /// One line on what `instance` runs live, for the status row.
    pub fn status_line(&self, instance: &str) -> String {
        if let Some(error) = &self.error {
            return format!("live: {error}");
        }
        let Some(status) = self.host.as_ref().and_then(|host| host.status(&InstanceId(instance.to_string()))) else {
            return self.instance_errors.get(instance).map(|error| format!("live: {error}")).unwrap_or_default();
        };
        let mut line = match status.running {
            Some((generation, revision)) => format!("live: generation {} of revision {revision}", generation.0),
            None => "live: nothing running".to_string(),
        };
        if let Some((_, revision, phase)) = status.pending {
            line.push_str(&format!(" · revision {revision} {phase}"));
        }
        if let Some((revision, error)) = &status.error {
            line.push_str(&format!(" · revision {revision} refused: {error}"));
        }
        if let Some(error) = self.instance_errors.get(instance) {
            line.push_str(&format!(" · {error}"));
        }
        line
    }
}
