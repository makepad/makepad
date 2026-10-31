//! The live host: one per process, owned by one thread (the UI thread of a
//! window, the main loop of a headless host), which it never blocks.
//!
//! An instance's live generation goes through
//!
//! ```text
//! wanted -> preparing (worker) -> staged (every owner reserved)
//!        -> committed (published) -> running (every owner acknowledged)
//! ```
//!
//! and any failure before the commit leaves the running generation exactly
//! as it was, with the error kept beside it. A newer request supersedes one
//! that has not been committed. Processing belongs to the instance: nothing
//! here knows which package or pane a window shows.

use super::owner::{
    BaseInfo, LiveOwner, OwnerCommand, OwnerRuntime, PrepareRequest, PreparedSlice, ResourceState, SessionKey, StageRefusal,
};
use super::plan::{normalize_control, partition, Endpoint, EndpointKind, LivePlanSpec, PlanError};
use super::value::{EndpointValue, ProbeValue, WriteRefusal};
use crate::document::ident::{Generation, InstanceId};
use crate::document::Registry;
use crate::Graph;
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct LiveLimits {
    /// Live sessions this host keeps at a time.
    pub max_sessions: usize,
    /// Prepares queued for the worker.
    pub prepare_lane: usize,
    /// Pumps a refused stage waits before its revision is prepared again.
    pub retry_pumps: u32,
}

impl Default for LiveLimits {
    fn default() -> Self {
        Self { max_sessions: 8, prepare_lane: 2, retry_pumps: 8 }
    }
}

/// Make `authored` (the evaluated graph of `definition` at `revision`) the
/// live generation of `instance`.
#[derive(Clone)]
pub struct ApplyRevision {
    pub instance: InstanceId,
    pub definition: String,
    pub revision: u64,
    pub authored: Arc<Graph>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LiveEvent {
    /// Every owner acknowledged `generation`. `skew_pumps` is how many host
    /// pumps lay between the commit and the last acknowledgement.
    Installed { instance: InstanceId, generation: Generation, revision: u64, skew_pumps: u32 },
    /// `revision` did not become live; what ran before still runs.
    Failed { instance: InstanceId, revision: u64, error: PlanError },
    /// The revision has no live node; nothing of the instance runs live.
    Idle { instance: InstanceId, revision: u64 },
    Stopped { instance: InstanceId },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveStatus {
    pub incarnation: u64,
    pub running: Option<(Generation, u64)>,
    /// `(generation, revision, phase)` of what is on its way.
    pub pending: Option<(Generation, u64, &'static str)>,
    /// The revision that failed last and why, until a later one succeeds.
    pub error: Option<(u64, PlanError)>,
    /// What each owner acknowledges as running.
    pub owners: Vec<(String, Option<Generation>)>,
    pub stopping: bool,
}

/// One admitted endpoint in one generation and incarnation of this host.
/// Keep this target with a refused value when retrying [`WriteRefusal::Busy`];
/// resolving its address again could send an old command to a replacement.
#[derive(Clone, Debug)]
pub struct WriteTarget {
    host: Arc<()>,
    session: SessionKey,
    generation: Generation,
    endpoint: Endpoint,
}

impl WriteTarget {
    pub fn session(&self) -> &SessionKey {
        &self.session
    }

    pub fn instance(&self) -> &InstanceId {
        &self.session.instance
    }

    pub fn generation(&self) -> Generation {
        self.generation
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl PartialEq for WriteTarget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.host, &other.host)
            && self.session == other.session
            && self.generation == other.generation
            && self.endpoint == other.endpoint
    }
}

struct Live {
    generation: Generation,
    plan: Arc<LivePlanSpec>,
}

struct Committed {
    live: Live,
    removed: Vec<String>,
    pumps: u32,
}

struct Session {
    key: SessionKey,
    wanted: Option<ApplyRevision>,
    /// Newest accepted request, including in-flight and failed requests.
    /// Reconciliation and context invalidation must never fall back to an
    /// older running revision while a newer edit is being prepared.
    latest: Option<ApplyRevision>,
    preparing: Option<(Generation, u64)>,
    committed: Option<Committed>,
    running: Option<Live>,
    error: Option<(u64, PlanError)>,
    retry_in: u32,
    stopping: bool,
}

struct Job {
    session: SessionKey,
    generation: Generation,
    request: ApplyRevision,
    contexts: BTreeMap<String, Arc<dyn Any + Send + Sync>>,
    bases: BTreeMap<String, BaseInfo>,
}

enum Outcome {
    Ready { plan: Arc<LivePlanSpec>, slices: Vec<(String, Box<dyn PreparedSlice>)> },
    /// The plan has a slice of an owner whose runtime was not ready.
    NeedsContext,
    Failed(PlanError),
}

struct Done {
    session: SessionKey,
    generation: Generation,
    request: ApplyRevision,
    outcome: Outcome,
}

pub struct LiveHost {
    identity: Arc<()>,
    registry: Arc<Registry>,
    owners: Arc<BTreeMap<String, Arc<dyn LiveOwner>>>,
    runtimes: BTreeMap<String, Box<dyn OwnerRuntime>>,
    limits: LiveLimits,
    sessions: BTreeMap<InstanceId, Session>,
    next_generation: u64,
    next_incarnation: u64,
    jobs: Option<SyncSender<Job>>,
    done: Option<Receiver<Done>>,
    events: Vec<LiveEvent>,
}

impl LiveHost {
    /// Every owner comes with the runtime that runs it; an owner without a
    /// runtime could prepare but never run.
    pub fn new(
        registry: Arc<Registry>,
        owners: Vec<(Arc<dyn LiveOwner>, Box<dyn OwnerRuntime>)>,
        limits: LiveLimits,
    ) -> Result<Self, String> {
        let mut factories = BTreeMap::new();
        let mut runtimes = BTreeMap::new();
        for (owner, runtime) in owners {
            let key = owner.key().to_string();
            if runtime.key() != key {
                return Err(format!("live owner `{key}` was given the runtime of `{}`", runtime.key()));
            }
            if factories.insert(key.clone(), owner).is_some() {
                return Err(format!("live owner `{key}` is registered twice"));
            }
            runtimes.insert(key, runtime);
        }
        Ok(Self {
            identity: Arc::new(()),
            registry,
            owners: Arc::new(factories),
            runtimes,
            limits,
            sessions: BTreeMap::new(),
            next_generation: 1,
            next_incarnation: 1,
            jobs: None,
            done: None,
            events: Vec::new(),
        })
    }

    /// Adopt another server/catalog after every old session has acknowledged
    /// stopping. Keep owner runtimes and their device callback, but abandon
    /// the old preparation lane so future workers capture the new registry.
    /// Dropping these endpoints never waits for an in-flight preparation;
    /// that worker finishes against its old immutable snapshot and exits.
    pub fn replace_registry(&mut self, registry: Arc<Registry>) -> Result<(), String> {
        if !self.sessions.is_empty() {
            return Err("live sessions must finish stopping before replacing their registry".to_string());
        }
        self.jobs = None;
        self.done = None;
        self.registry = registry;
        Ok(())
    }

    pub fn is_live_owner(&self, owner: &str) -> bool {
        self.owners.contains_key(owner)
    }

    /// Whether `authored` has a node a live owner runs: a cheap test a host
    /// makes before it bothers to apply a revision.
    pub fn has_live_nodes(&self, authored: &Graph) -> bool {
        !owners_used(authored, &self.registry, &self.owners).is_empty()
    }

    /// Ask for a revision to become live. Returns at once; the outcome is a
    /// later [`LiveEvent`]. A newer request replaces one not yet prepared.
    pub fn apply(&mut self, request: ApplyRevision) -> Result<(), String> {
        if !self.sessions.contains_key(&request.instance) {
            if self.sessions.len() >= self.limits.max_sessions {
                return Err(format!("{} live sessions are the limit", self.limits.max_sessions));
            }
            let key = SessionKey { instance: request.instance.clone(), incarnation: self.next_incarnation };
            self.next_incarnation += 1;
            self.sessions.insert(request.instance.clone(), Session {
                key, wanted: None, latest: None, preparing: None, committed: None, running: None,
                error: None, retry_in: 0, stopping: false,
            });
        }
        let session = self.sessions.get_mut(&request.instance).expect("inserted above");
        if session.stopping {
            return Err("the instance is still stopping".to_string());
        }
        // Moving an editor card does not replace an installed execution.
        // Restrict this to a settled, successful revision: in-flight owner
        // payloads retain their original revision and retry identities.
        let layout_only = session.wanted.is_none()
            && session.preparing.is_none()
            && session.committed.is_none()
            && session.error.is_none()
            && session.running.as_ref().zip(session.latest.as_ref()).is_some_and(|(running, latest)| {
                running.plan.definition == request.definition
                    && latest.definition == request.definition
                    && running.plan.revision == latest.revision
                    && super::same_root_layout_execution(&latest.authored, &request.authored)
            });
        if layout_only {
            let running = session.running.as_mut().expect("settled revision checked above");
            // Owners may retain the old Arc. Publish fresh host metadata;
            // leave their plan, generation and exact endpoints untouched.
            let mut plan = running.plan.as_ref().clone();
            plan.revision = request.revision;
            running.plan = Arc::new(plan);
            session.latest = Some(request);
            return Ok(());
        }
        session.latest = Some(request.clone());
        session.wanted = Some(request);
        session.retry_in = 0;
        Ok(())
    }

    /// The revision the instance runs, is about to run, or was last asked to.
    pub fn applied_revision(&self, instance: &InstanceId) -> Option<(String, u64)> {
        let session = self.sessions.get(instance)?;
        session.latest.as_ref().map(|request| (request.definition.clone(), request.revision))
    }

    pub fn stop(&mut self, instance: &InstanceId) {
        let Some(session) = self.sessions.get_mut(instance) else { return };
        session.stopping = true;
        session.wanted = None;
        session.preparing = None;
        for runtime in self.runtimes.values_mut() {
            runtime.stop(&session.key);
        }
    }

    pub fn instances(&self) -> impl Iterator<Item = &InstanceId> {
        self.sessions.keys()
    }

    pub fn status(&self, instance: &InstanceId) -> Option<LiveStatus> {
        let session = self.sessions.get(instance)?;
        let pending = if let Some(committed) = &session.committed {
            Some((committed.live.generation, committed.live.plan.revision, "committed"))
        } else if let Some((generation, revision)) = session.preparing {
            Some((generation, revision, "preparing"))
        } else {
            session.wanted.as_ref().map(|request| (Generation(0), request.revision, "wanted"))
        };
        Some(LiveStatus {
            incarnation: session.key.incarnation,
            running: session.running.as_ref().map(|live| (live.generation, live.plan.revision)),
            pending,
            error: session.error.clone(),
            owners: self.runtimes.iter().map(|(key, runtime)| (key.clone(), runtime.active(&session.key))).collect(),
            stopping: session.stopping,
        })
    }

    /// The plan whose endpoints the instance admits now: the committed one
    /// once there is one (it is what every owner is moving to), else the
    /// running one.
    pub fn plan(&self, instance: &InstanceId) -> Option<(Generation, &LivePlanSpec)> {
        let session = self.sessions.get(instance)?;
        session
            .committed
            .as_ref()
            .map(|committed| &committed.live)
            .or(session.running.as_ref())
            .map(|live| (live.generation, &*live.plan))
    }

    pub fn endpoints(&self, instance: &InstanceId) -> &[Endpoint] {
        self.plan(instance).map(|(_, plan)| plan.endpoints.as_slice()).unwrap_or_default()
    }

    /// Resolve the endpoint admitted now, selecting the committed generation
    /// when present and otherwise the running generation. The target remains
    /// valid through acknowledgement, but never follows another generation.
    pub fn resolve_write(&self, instance: &InstanceId, node: &str, param: &str) -> Result<WriteTarget, WriteRefusal> {
        let session = self.sessions.get(instance).ok_or(WriteRefusal::NotRunning)?;
        if session.stopping {
            return Err(WriteRefusal::NotRunning);
        }
        let live = session
            .committed
            .as_ref()
            .map(|committed| &committed.live)
            .or(session.running.as_ref())
            .ok_or(WriteRefusal::NotRunning)?;
        let endpoint = live.plan.endpoint(node, param).ok_or(WriteRefusal::UnknownEndpoint)?;
        Ok(WriteTarget {
            host: self.identity.clone(),
            session: session.key.clone(),
            generation: live.generation,
            endpoint: endpoint.clone(),
        })
    }

    /// Whether `target` is still exactly what this host admits: this host,
    /// this session (instance and incarnation), the generation admitted now
    /// and the same endpoint at the same id. A replacement generation, a
    /// recreated instance or a stopping one refuses; nothing retargets.
    fn admitted(&self, target: &WriteTarget) -> Result<(), WriteRefusal> {
        if !Arc::ptr_eq(&self.identity, &target.host) {
            return Err(WriteRefusal::UnknownEndpoint);
        }
        let session = self.sessions.get(target.instance()).ok_or(WriteRefusal::NotRunning)?;
        if session.stopping {
            return Err(WriteRefusal::NotRunning);
        }
        if session.key != target.session {
            return Err(WriteRefusal::UnknownEndpoint);
        }
        let live = session
            .committed
            .as_ref()
            .map(|committed| &committed.live)
            .or(session.running.as_ref())
            .ok_or(WriteRefusal::NotRunning)?;
        if live.generation != target.generation
            || live.plan.endpoints.get(target.endpoint.id.0 as usize) != Some(&target.endpoint)
        {
            return Err(WriteRefusal::UnknownEndpoint);
        }
        Ok(())
    }

    /// Write or retry without retargeting. A stopped/recreated instance,
    /// replacement generation or changed endpoint refuses the stale command
    /// before value normalization or any owner call.
    pub fn write_to(&mut self, target: &WriteTarget, value: EndpointValue) -> Result<Option<f64>, WriteRefusal> {
        self.admitted(target)?;
        let endpoint = &target.endpoint;
        if let Some(by) = &endpoint.driver {
            return Err(WriteRefusal::Driven { by: by.clone() });
        }
        let value = match (&endpoint.kind, value) {
            (EndpointKind::Control { min, max, step, .. }, EndpointValue::Number(number)) => {
                if !number.is_finite() {
                    return Err(WriteRefusal::NotFinite);
                }
                EndpointValue::Number(normalize_control(number, *min, *max, *step))
            }
            (EndpointKind::Control { .. }, _) => return Err(WriteRefusal::WrongKind { expected: "number" }),
            (EndpointKind::Toggle { .. }, EndpointValue::Toggle(on)) => EndpointValue::Toggle(on),
            (EndpointKind::Toggle { .. }, EndpointValue::Number(number)) if number == 0.0 || number == 1.0 => {
                EndpointValue::Toggle(number == 1.0)
            }
            (EndpointKind::Toggle { .. }, _) => return Err(WriteRefusal::WrongKind { expected: "toggle" }),
            (EndpointKind::Trigger, value @ (EndpointValue::Note { .. } | EndpointValue::Pulse)) => value,
            (EndpointKind::Trigger, _) => return Err(WriteRefusal::WrongKind { expected: "note or pulse" }),
            (EndpointKind::Probe { .. }, _) => return Err(WriteRefusal::WrongKind { expected: "nothing: a probe is read-only" }),
        };
        let runtime = self.runtimes.get_mut(&endpoint.owner).ok_or(WriteRefusal::UnknownEndpoint)?;
        runtime.write(&target.session, target.generation, endpoint, value)
    }

    /// Write a typed value to `node.param` of `instance`. For a Busy retry,
    /// keep a [`WriteTarget`] from [`Self::resolve_write`] and use
    /// [`Self::write_to`] so a replacement cannot receive the old value.
    pub fn write(&mut self, instance: &InstanceId, node: &str, param: &str, value: EndpointValue) -> Result<Option<f64>, WriteRefusal> {
        let target = self.resolve_write(instance, node, param)?;
        self.write_to(&target, value)
    }

    /// Send an owner-typed command to exactly the trigger endpoint the
    /// target names, with the checks of [`Self::write_to`]: a stale target
    /// is refused, not retargeted. Only a trigger endpoint takes a command;
    /// the command's owner must be the endpoint's. Every refusal returns
    /// the command untouched; keep this target and the command on `Busy`
    /// and retry on a later pump.
    pub fn command_to(&mut self, target: &WriteTarget, command: OwnerCommand) -> Result<(), (WriteRefusal, OwnerCommand)> {
        if let Err(refusal) = self.admitted(target) {
            return Err((refusal, command));
        }
        let endpoint = &target.endpoint;
        if !matches!(endpoint.kind, EndpointKind::Trigger) {
            return Err((WriteRefusal::WrongKind { expected: "trigger endpoint for a command" }, command));
        }
        if command.owner != endpoint.owner {
            return Err((WriteRefusal::WrongKind { expected: "command of the endpoint's owner" }, command));
        }
        let Some(runtime) = self.runtimes.get_mut(&endpoint.owner) else {
            return Err((WriteRefusal::UnknownEndpoint, command));
        };
        runtime.command(&target.session, target.generation, endpoint, command)
    }

    /// The resource behind exactly the trigger endpoint the target names,
    /// with the checks of [`Self::write_to`]: a widget holding a target of a
    /// replaced generation or a recreated instance is refused rather than
    /// shown a replacement's resource. `Ok(None)` is an owner that keeps no
    /// resource for that endpoint.
    pub fn resource_to(&self, target: &WriteTarget) -> Result<Option<ResourceState>, WriteRefusal> {
        self.admitted(target)?;
        let endpoint = &target.endpoint;
        if !matches!(endpoint.kind, EndpointKind::Trigger) {
            return Err(WriteRefusal::WrongKind { expected: "trigger endpoint for a resource" });
        }
        let runtime = self.runtimes.get(&endpoint.owner).ok_or(WriteRefusal::UnknownEndpoint)?;
        Ok(runtime.resource(&target.session, target.generation, endpoint))
    }

    pub fn probe(&self, instance: &InstanceId, node: &str, port: &str) -> Option<ProbeValue> {
        let session = self.sessions.get(instance)?;
        let live = session.running.as_ref()?;
        let endpoint = live.plan.probe_endpoint(node, port)?;
        self.runtimes.get(&endpoint.owner)?.probe(&session.key, live.generation, endpoint)
    }

    /// An owner-defined node observation from the acknowledged running plan.
    /// The host proves node ownership; the owner also checks execution identity.
    pub fn probe_node(&self, instance: &InstanceId, node: &str, name: &str) -> Option<ProbeValue> {
        let session = self.sessions.get(instance)?;
        let live = session.running.as_ref()?;
        let slice = live.plan.slices.iter().find(|slice| slice.nodes.iter().any(|item| item.id == node))?;
        self.runtimes.get(&slice.owner)?.probe_node(&session.key, live.generation, node, name)
    }

    /// The target `node.param` has now (not its authored default).
    pub fn control(&self, instance: &InstanceId, node: &str, param: &str) -> Option<f64> {
        let session = self.sessions.get(instance)?;
        let live = session.committed.as_ref().map(|committed| &committed.live).or(session.running.as_ref())?;
        let endpoint = live.plan.endpoint(node, param)?;
        self.runtimes.get(&endpoint.owner)?.control(&session.key, live.generation, endpoint)
    }

    /// Advance everything that can advance without waiting. Call it on a
    /// timer and while anything live is shown.
    pub fn pump(&mut self) -> Vec<LiveEvent> {
        let mut context_changed = Vec::new();
        for (key, runtime) in &mut self.runtimes {
            if runtime.pump() {
                context_changed.push(key.clone());
            }
        }
        for session in self.sessions.values_mut() {
            if session.stopping {
                continue;
            }
            let uses_changed = |live: &Live| live.plan.slices.iter().any(|slice| context_changed.contains(&slice.owner));
            let affected = !context_changed.is_empty() && (
                session.running.as_ref().is_some_and(uses_changed)
                || session.committed.as_ref().is_some_and(|committed| uses_changed(&committed.live))
                || session.latest.as_ref().is_some_and(|request| {
                    owners_used(&request.authored, &self.registry, &self.owners)
                        .iter().any(|owner| context_changed.contains(owner))
                })
            );
            if affected {
                session.wanted = session.latest.clone();
                // Old completions are discarded even if they are already in
                // the result queue. A newer authored edit retains precedence.
                session.preparing = None;
                session.retry_in = 0;
            }
            for (owner, runtime) in &mut self.runtimes {
                if let Some((generation, message)) = runtime.rejected(&session.key) {
                    if session.committed.as_ref().is_some_and(|committed| committed.live.generation == generation) {
                        let revision = session.committed.as_ref().unwrap().live.plan.revision;
                        session.committed = None;
                        session.wanted = session.latest.clone();
                        session.retry_in = self.limits.retry_pumps;
                        session.error = Some((revision, PlanError::general(format!(
                            "owner `{owner}` could not activate generation {}: {message}; preparing the latest revision again", generation.0
                        ))));
                    }
                }
            }
        }

        self.take_results();

        let instances: Vec<InstanceId> = self.sessions.keys().cloned().collect();
        for instance in instances {
            self.advance(&instance);
        }
        std::mem::take(&mut self.events)
    }

    fn take_results(&mut self) {
        let mut results = Vec::new();
        let mut disconnected = false;
        if let Some(done) = &self.done {
            loop {
                match done.try_recv() {
                    Ok(result) => results.push(result),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => { disconnected = true; break; }
                }
            }
        }
        for Done { session: key, generation, request, outcome } in results {
            let Some(session) = self.sessions.get_mut(&key.instance) else { continue };
            // A result for another life of the instance, for a prepare that
            // was abandoned, or for a revision a newer request replaced, is
            // dropped here; its slices were never staged anywhere.
            if session.key != key || session.preparing.map(|(generation, _)| generation) != Some(generation) || session.stopping {
                continue;
            }
            session.preparing = None;
            if session.wanted.is_some() {
                continue;
            }
            match outcome {
                Outcome::Failed(error) => {
                    session.error = Some((request.revision, error.clone()));
                    self.events.push(LiveEvent::Failed { instance: key.instance.clone(), revision: request.revision, error });
                }
                Outcome::NeedsContext => {
                    session.wanted = Some(request);
                    session.retry_in = self.limits.retry_pumps;
                }
                Outcome::Ready { plan, slices } => self.stage_and_commit(&key, generation, request, plan, slices),
            }
        }
        if disconnected {
            self.worker_disconnected();
        }
    }

    fn stage_and_commit(
        &mut self,
        key: &SessionKey,
        generation: Generation,
        request: ApplyRevision,
        plan: Arc<LivePlanSpec>,
        slices: Vec<(String, Box<dyn PreparedSlice>)>,
    ) {
        let mut staged: Vec<String> = Vec::new();
        let mut refusal = None;
        for (owner, prepared) in slices {
            if refusal.is_some() {
                // Prepared for an owner that is never staged: dropped here,
                // on the host's thread.
                continue;
            }
            let runtime = self.runtimes.get_mut(&owner).expect("a slice exists only for an owner with a runtime");
            match runtime.stage(key, generation, prepared) {
                Ok(()) => staged.push(owner),
                Err((reason, _returned)) => refusal = Some((owner, reason)),
            }
        }
        let mut removed = Vec::new();
        if refusal.is_none() {
            for (owner, runtime) in &mut self.runtimes {
                if !staged.contains(owner) && !runtime.stopped(key) {
                    match runtime.stage_stop(key, generation) {
                        Ok(()) => removed.push(owner.clone()),
                        Err(reason) => { refusal = Some((owner.clone(), reason)); break; }
                    }
                }
            }
        }
        let session = self.sessions.get_mut(&key.instance).expect("checked by the caller");
        if let Some((owner, reason)) = refusal {
            for owner in &staged {
                self.runtimes.get_mut(owner).expect("staged above").abort(key, generation);
            }
            for owner in &removed {
                self.runtimes.get_mut(owner).expect("removal staged above").abort_stop(key, generation);
            }
            match reason {
                // Nothing was published, so the running generation is whole.
                // Capacity and a moved base both pass: prepare it again.
                StageRefusal::Busy | StageRefusal::StaleBase => {
                    session.wanted = Some(request);
                    session.retry_in = self.limits.retry_pumps;
                }
                StageRefusal::Refused(message) => {
                    let error = PlanError::general(format!("owner `{owner}` refused the revision: {message}"));
                    session.error = Some((request.revision, error.clone()));
                    self.events.push(LiveEvent::Failed { instance: key.instance.clone(), revision: request.revision, error });
                }
            }
            return;
        }
        // Every owner holds its reservation: publication cannot fail.
        // Execution admission may still refuse an invalidated device
        // context, reported through `rejected` and prepared again.
        for owner in &staged {
            self.runtimes.get_mut(owner).expect("staged above").commit(key, generation);
        }
        for owner in &removed {
            self.runtimes.get_mut(owner).expect("removal staged above").commit_stop(key, generation);
        }
        session.committed = Some(Committed { live: Live { generation, plan }, removed, pumps: 0 });
    }

    fn advance(&mut self, instance: &InstanceId) {
        let Some(session) = self.sessions.get_mut(instance) else { return };
        if session.stopping {
            if self.runtimes.values().all(|runtime| runtime.stopped(&session.key)) {
                self.sessions.remove(instance);
                self.events.push(LiveEvent::Stopped { instance: instance.clone() });
            }
            return;
        }
        if let Some(committed) = &mut session.committed {
            committed.pumps += 1;
            let acknowledged = committed.live.plan.slices.iter().all(|slice| {
                self.runtimes.get(&slice.owner).and_then(|runtime| runtime.active(&session.key)) == Some(committed.live.generation)
            });
            let removed = committed.removed.iter().all(|owner| self.runtimes[owner].stopped(&session.key));
            if !acknowledged || !removed {
                return;
            }
            let committed = session.committed.take().expect("matched above");
            let (generation, revision) = (committed.live.generation, committed.live.plan.revision);
            session.error = None;
            if committed.live.plan.slices.is_empty() {
                session.running = None;
                self.events.push(LiveEvent::Idle { instance: instance.clone(), revision });
            } else {
                session.running = Some(committed.live);
                self.events.push(LiveEvent::Installed { instance: instance.clone(), generation, revision, skew_pumps: committed.pumps });
            }
        }
        if session.preparing.is_some() || session.wanted.is_none() {
            return;
        }
        if session.retry_in > 0 {
            session.retry_in -= 1;
            return;
        }
        let request = session.wanted.take().expect("checked above");
        // Only an owner the revision uses is asked for its context: asking
        // is what makes an audio runtime want its device.
        let used = owners_used(&request.authored, &self.registry, &self.owners);
        let mut contexts = BTreeMap::new();
        let mut bases = BTreeMap::new();
        for (owner, runtime) in &mut self.runtimes {
            if !used.contains(owner) {
                continue;
            }
            if let Ok(context) = runtime.context() {
                contexts.insert(owner.clone(), context);
            }
            if let Some(base) = runtime.base(&session.key) {
                bases.insert(owner.clone(), base);
            }
        }
        let generation = Generation(self.next_generation);
        self.next_generation += 1;
        let job = Job { session: session.key.clone(), generation, request, contexts, bases };
        let revision = job.request.revision;
        let sent = match self.ensure_worker() {
            Ok(()) => self.jobs.as_ref().expect("worker started").try_send(job),
            Err(message) => {
                let session = self.sessions.get_mut(instance).expect("session retained");
                let error = PlanError::general(message);
                session.error = Some((revision, error.clone()));
                self.events.push(LiveEvent::Failed { instance: instance.clone(), revision, error });
                return;
            }
        };
        let session = self.sessions.get_mut(instance).expect("session retained");
        match sent {
            Ok(()) => session.preparing = Some((generation, revision)),
            Err(TrySendError::Full(job)) => session.wanted = Some(job.request),
            Err(TrySendError::Disconnected(_job)) => {
                // Include this unsent job and every other abandoned job;
                // none may remain indefinitely marked as preparing.
                session.preparing = Some((generation, revision));
                self.worker_disconnected();
            }
        }
    }

    fn worker_disconnected(&mut self) {
        self.jobs = None;
        self.done = None;
        for session in self.sessions.values_mut() {
            if let Some((_, revision)) = session.preparing.take() {
                let error = PlanError::general("the live preparation worker disconnected");
                session.error = Some((revision, error.clone()));
                self.events.push(LiveEvent::Failed { instance: session.key.instance.clone(), revision, error });
            }
        }
    }

    fn ensure_worker(&mut self) -> Result<(), String> {
        if self.jobs.is_none() {
            let (jobs, queue) = sync_channel::<Job>(self.limits.prepare_lane.max(1));
            let (results, done) = channel::<Done>();
            let (registry, owners) = (self.registry.clone(), self.owners.clone());
            std::thread::Builder::new().name("stage-live-prepare".into()).spawn(move || {
                while let Ok(job) = queue.recv() {
                    let outcome = prepare(&registry, &owners, &job);
                    let done = Done { session: job.session, generation: job.generation, request: job.request, outcome };
                    if results.send(done).is_err() {
                        return;
                    }
                }
            }).map_err(|error| format!("cannot start the live preparation worker: {error}"))?;
            self.done = Some(done);
            self.jobs = Some(jobs);
        }
        Ok(())
    }

}

/// Owners reachable through instantiated components. An unused component
/// definition must not activate a device or invalidate an unrelated session.
fn owners_used(authored: &Graph, registry: &Registry, owners: &BTreeMap<String, Arc<dyn LiveOwner>>) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut graphs = vec![authored];
    while let Some(graph) = graphs.pop() {
        for node in &graph.nodes {
            if node.kind == "component" {
                if visited.insert(node.type_name.as_str()) {
                    if let Some(component) = authored.components.iter().find(|component| component.alias == node.type_name) {
                        graphs.push(&component.graph);
                    }
                }
            } else if let Some(descriptor) = registry.descriptor(&node.type_name) {
                if owners.contains_key(&descriptor.owner) {
                    used.insert(descriptor.owner.clone());
                }
            }
        }
    }
    used
}

fn prepare(registry: &Registry, owners: &BTreeMap<String, Arc<dyn LiveOwner>>, job: &Job) -> Outcome {
    let authored = &job.request.authored;
    for graph in std::iter::once(authored.as_ref()).chain(authored.components.iter().map(|component| &component.graph)) {
        if let Err(error) = crate::graph::validate_projection_with_registry(graph, &authored.components, registry) {
            return Outcome::Failed(PlanError::general(format!("invalid graph: {error}")));
        }
    }
    let lowered = match crate::engine::lower::lower(authored) {
        Ok(lowered) => lowered,
        Err(error) => return Outcome::Failed(PlanError::general(format!("lowering failed: {error}"))),
    };
    let is_live = |owner: &str| owners.contains_key(owner);
    let plan = match partition(&lowered, registry, &is_live, &job.request.definition, job.request.revision) {
        Ok(plan) => Arc::new(plan),
        Err(error) => return Outcome::Failed(error),
    };
    let mut slices = Vec::with_capacity(plan.slices.len());
    for slice in &plan.slices {
        let Some(context) = job.contexts.get(&slice.owner) else { return Outcome::NeedsContext };
        let owner = &owners[&slice.owner];
        let request = PrepareRequest {
            session: &job.session,
            generation: job.generation,
            plan: &plan,
            slice,
            base: job.bases.get(&slice.owner).cloned(),
            context: context.clone(),
        };
        match owner.prepare(request) {
            Ok(prepared) => slices.push((slice.owner.clone(), prepared)),
            Err(error) => return Outcome::Failed(error),
        }
    }
    Outcome::Ready { plan, slices }
}
