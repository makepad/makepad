//! One owner for the finite engine and the live host together.
//!
//! An application that embeds flows holds one [`FlowController`]. It owns
//! the [`EngineHandle`] and the [`LiveHost`] and nobody else touches them:
//! commands go in through [`FlowController::submit`], replies come back out
//! of [`FlowController::pump`], and the controller keeps the live host in
//! step with what the engine's authoritative snapshot says exists.
//!
//! The rule it enforces: an instance runs live exactly the execution the
//! engine accepted for it — the definition and revision of its row, pinned
//! or following — fetched with `EngineQuery::InstanceExecution` and applied
//! only while that row still says so. A row whose execution is not kept, or
//! whose newest draft failed, changes nothing live: the last good generation
//! keeps running and the fact is reported once. Nothing here evaluates a
//! graph, holds a widget or a `Cx`, or talks HTTP.

use super::engine::{
    Engine, EngineConfig, EngineError, EngineHandle, EngineQuery, EngineSnapshot, HostServices, JournalEntry,
    JournalPoll, LaneGap, Observation, Reply, ReplyPayload, RequestId, Submit, Transition, TransitionPoll,
    READY_REQUEST,
};
use super::store::InstanceExecution;
use crate::document::ident::InstanceId;
use crate::live::{
    ApplyRevision, EndpointValue, LiveEvent, LiveHost, LiveLimits, LiveOwner, LiveStatus, OwnerCommand, OwnerRuntime,
    ProbeValue, ResourceState, WriteRefusal, WriteTarget,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::sync::Arc;

/// What the controller itself found out during a pump; the engine's own
/// replies and the live host's events travel beside it, untouched.
#[derive(Clone, Debug, PartialEq)]
pub enum ControllerEvent {
    /// The engine keeps no execution for the row's revision (a pin whose
    /// revision left the ring, a definition never accepted). Not asked again
    /// until the row changes; nothing else is run in its place.
    ExecutionUnavailable { instance: InstanceId, definition: String, revision: u64 },
    /// The execution query failed for good; as above, not asked again until
    /// the row changes.
    ExecutionError { instance: InstanceId, definition: String, revision: u64, error: EngineError },
    /// The live host would not take the accepted execution (its session
    /// limit, or the instance's previous incarnation still stopping). Tried
    /// again when a live session ends.
    LiveApplyRefused { instance: InstanceId, revision: u64, error: String },
    /// The engine worker is gone without a `Stopped` reply. Every live
    /// session is being stopped.
    EngineGone,
}

/// One pump's harvest.
#[derive(Debug, Default)]
pub struct Pump {
    /// Terminal replies to the application's own commands and queries,
    /// including `Stopped`. The controller's execution queries are not here.
    pub replies: Vec<Reply>,
    pub live: Vec<LiveEvent>,
    pub events: Vec<ControllerEvent>,
}

struct Tracked {
    /// `(definition, revision)` of the row as last seen.
    row: (String, u64),
    /// The accepted execution for `row`, once fetched; kept for inspection
    /// and as what the live host was, or is to be, given.
    accepted: Option<Arc<InstanceExecution>>,
    /// The `(definition, revision)` the live host took last.
    applied: Option<(String, u64)>,
    /// A row the engine answered "no execution" or a terminal error for.
    unavailable: Option<(String, u64)>,
}

pub struct FlowController {
    engine: EngineHandle,
    live: LiveHost,
    snapshot: Arc<EngineSnapshot>,
    tracked: BTreeMap<InstanceId, Tracked>,
    /// Execution queries in flight: what each asked for.
    queries: BTreeMap<RequestId, (InstanceId, String, u64)>,
    /// Accepted application commands must each reach its caller, including
    /// synthetic failures when a dead engine's replies drain over many pumps.
    pending_requests: BTreeSet<RequestId>,
    /// Where the next round of queries starts, so no instance starves
    /// behind the reply budget.
    cursor: Option<InstanceId>,
    stopping: bool,
    engine_stopped: bool,
}

impl FlowController {
    /// Start the engine and the live host on the same registry. Rows are
    /// always in the snapshot: the controller reads them, not queries.
    pub fn start(
        mut config: EngineConfig,
        services: HostServices,
        owners: Vec<(Arc<dyn LiveOwner>, Box<dyn OwnerRuntime>)>,
        limits: LiveLimits,
    ) -> Result<Self, String> {
        config.snapshot_rows = true;
        let live = LiveHost::new(config.registry.clone(), owners, limits)?;
        let mut engine = Engine::start(config, services).map_err(|error| error.to_string())?;
        let snapshot = engine.snapshot();
        Ok(Self {
            engine,
            live,
            snapshot,
            tracked: BTreeMap::new(),
            queries: BTreeMap::new(),
            pending_requests: BTreeSet::from([READY_REQUEST]),
            cursor: None,
            stopping: false,
            engine_stopped: false,
        })
    }

    /// Queue an engine command. Never blocks; the reply arrives in a later
    /// [`Self::pump`]. A `Stop` that is accepted also stops every live
    /// session. Once stopping begins, new commands return `Submit::Closed`;
    /// already accepted commands still receive their terminal replies.
    pub fn submit(&mut self, command: super::engine::EngineCommand) -> Submit {
        if self.stopping {
            return Submit::Closed(command);
        }
        let is_stop = matches!(command, super::engine::EngineCommand::Stop);
        let submit = self.engine.submit(command);
        if let Submit::Accepted(id) = &submit {
            self.pending_requests.insert(*id);
        }
        if is_stop && matches!(submit, Submit::Accepted(_)) {
            self.stopping = true;
            self.stop_live();
        }
        submit
    }

    /// Ask the engine to stop. See [`Self::submit`] and [`Self::shutdown_complete`].
    pub fn stop(&mut self) -> Submit {
        self.submit(super::engine::EngineCommand::Stop)
    }

    /// Whether a stop was requested, or the engine went away.
    pub fn is_stopping(&self) -> bool {
        self.stopping
    }

    /// True once the engine has stopped (its `Stopped` reply, or its lane
    /// closed), every accepted request's terminal reply has been returned,
    /// and every live session has acknowledged its retirement. Keep
    /// calling [`Self::pump`], and keep pumping the owners' own paths (the
    /// audio callback, the frame producers) until this is true: retirement
    /// is acknowledged there. Detaching the audio callback itself is the
    /// host's, afterwards.
    pub fn shutdown_complete(&self) -> bool {
        self.stopping
            && (self.engine_stopped || !self.engine.is_alive())
            && self.pending_requests.is_empty()
            && self.queries.is_empty()
            && self.live.instances().next().is_none()
    }

    /// One bounded step: take up to `max_replies` engine replies, refresh
    /// the snapshot, bring the live host in step with its rows, and pump the
    /// live host. Pending execution queries count against `max_replies`.
    pub fn pump(&mut self, max_replies: usize) -> Pump {
        let mut out = Pump::default();
        let mut replies = Vec::new();
        self.engine.poll_replies(&mut replies, max_replies);
        if !self.engine.is_alive() && !self.engine_stopped {
            self.engine_stopped = true;
            if !self.stopping {
                self.stopping = true;
                out.events.push(ControllerEvent::EngineGone);
            }
        }
        let snapshot = self.engine.snapshot();
        let changed = snapshot.serial != self.snapshot.serial;
        self.snapshot = snapshot;
        if self.snapshot.stopping {
            self.stopping = true;
        }
        if changed {
            self.reconcile_rows();
        }
        for reply in replies {
            match self.queries.remove(&reply.id) {
                Some(asked) => self.take_execution(asked, reply.result, &mut out.events),
                None => {
                    self.pending_requests.remove(&reply.id);
                    if matches!(reply.result, Ok(ReplyPayload::Stopped { .. })) {
                        self.engine_stopped = true;
                        self.stopping = true;
                    }
                    out.replies.push(reply);
                }
            }
        }
        if self.stopping {
            self.stop_live();
        } else {
            self.submit_queries(max_replies);
        }
        out.live = self.live.pump();
        if !self.stopping && out.live.iter().any(|event| matches!(event, LiveEvent::Stopped { .. })) {
            // A session ended: a refused apply may have room now.
            self.retry_refused(&mut out.events);
        }
        out
    }

    /// Rows are the authority: an instance the snapshot no longer lists is
    /// stopped live; a row whose definition or revision changed is fetched
    /// again while its last accepted execution keeps running.
    fn reconcile_rows(&mut self) {
        let mut present = BTreeMap::new();
        for row in &self.snapshot.instances {
            present.insert(InstanceId(row.instance.clone()), (row.flow.clone(), row.revision));
        }
        let gone: Vec<InstanceId> = self.tracked.keys().filter(|id| !present.contains_key(*id)).cloned().collect();
        for id in &gone {
            self.tracked.remove(id);
        }
        // Keep the tags of outstanding internal queries until their replies
        // arrive, even for deleted instances. Otherwise a late controller
        // reply would leak into the application's command-reply stream.
        let live: Vec<InstanceId> = self.live.instances().cloned().collect();
        for id in live {
            if !present.contains_key(&id) && self.live.status(&id).is_some_and(|status| !status.stopping) {
                self.live.stop(&id);
            }
        }
        for (id, row) in present {
            let tracked = self.tracked.entry(id).or_insert_with(|| Tracked {
                row: row.clone(),
                accepted: None,
                applied: None,
                unavailable: None,
            });
            if tracked.row != row {
                tracked.row = row;
                tracked.unavailable = None;
            }
        }
    }

    fn wants_execution(tracked: &Tracked) -> bool {
        let fetched = tracked
            .accepted
            .as_ref()
            .is_some_and(|accepted| accepted.definition == tracked.row.0 && accepted.revision == tracked.row.1);
        !fetched && tracked.unavailable.as_ref() != Some(&tracked.row)
    }

    /// Ask for the execution of every row that needs one, round robin from
    /// the cursor, one query per instance, within the reply budget. A full
    /// queue ends the round; the rest is asked on a later pump.
    fn submit_queries(&mut self, max_replies: usize) {
        let mut budget = max_replies.saturating_sub(self.queries.len());
        if budget == 0 || self.tracked.is_empty() {
            return;
        }
        let start = self.cursor.clone();
        let order: Vec<InstanceId> = match &start {
            Some(cursor) => self
                .tracked
                .range((Bound::Excluded(cursor.clone()), Bound::Unbounded))
                .chain(self.tracked.range((Bound::Unbounded, Bound::Included(cursor.clone()))))
                .map(|(id, _)| id.clone())
                .collect(),
            None => self.tracked.keys().cloned().collect(),
        };
        for id in order {
            if budget == 0 {
                break;
            }
            let Some(tracked) = self.tracked.get(&id) else { continue };
            if !Self::wants_execution(tracked) || self.queries.values().any(|(pending, _, _)| *pending == id) {
                continue;
            }
            let row = tracked.row.clone();
            match self.engine.submit(super::engine::EngineCommand::Query(EngineQuery::InstanceExecution {
                instance: id.clone(),
            })) {
                Submit::Accepted(request) => {
                    self.queries.insert(request, (id.clone(), row.0, row.1));
                    self.cursor = Some(id);
                    budget -= 1;
                }
                Submit::Full(_) => break,
                Submit::Closed(_) => {
                    self.stopping = true;
                    break;
                }
                Submit::Refused(..) => break,
            }
        }
    }

    /// An execution reply, applied only while the row still asks for it.
    fn take_execution(
        &mut self,
        asked: (InstanceId, String, u64),
        result: Result<ReplyPayload, EngineError>,
        events: &mut Vec<ControllerEvent>,
    ) {
        let (id, definition, revision) = asked;
        let Some(tracked) = self.tracked.get_mut(&id) else { return };
        if tracked.row != (definition.clone(), revision) {
            // The row moved on; its new execution is asked for separately.
            return;
        }
        match result {
            Ok(ReplyPayload::InstanceExecution(Some(execution))) => {
                if execution.instance != id || execution.definition != definition || execution.revision != revision {
                    return;
                }
                tracked.accepted = Some(execution);
                if !self.stopping {
                    Self::apply_live(&mut self.live, &id, tracked, events);
                }
            }
            Ok(ReplyPayload::InstanceExecution(None)) => {
                tracked.unavailable = Some((definition.clone(), revision));
                events.push(ControllerEvent::ExecutionUnavailable { instance: id, definition, revision });
            }
            // A full lane: nothing is recorded, the next pump asks again.
            Err(EngineError::Busy(_)) => {}
            Err(error) => {
                tracked.unavailable = Some((definition.clone(), revision));
                events.push(ControllerEvent::ExecutionError { instance: id, definition, revision, error });
            }
            Ok(_) => {}
        }
    }

    fn apply_live(live: &mut LiveHost, id: &InstanceId, tracked: &mut Tracked, events: &mut Vec<ControllerEvent>) {
        let Some(accepted) = tracked.accepted.as_ref() else { return };
        let row = (accepted.definition.clone(), accepted.revision);
        if tracked.applied.as_ref() == Some(&row) {
            return;
        }
        if !live.has_live_nodes(&accepted.graph) {
            // Job-only instances must not consume the bounded live-session
            // capacity. A revision that removed its last live node retires
            // the previous owners instead of leaving an empty session.
            if live.status(id).is_some_and(|status| !status.stopping) {
                live.stop(id);
            }
            tracked.applied = Some(row);
            return;
        }
        let request = ApplyRevision {
            instance: id.clone(),
            definition: accepted.definition.clone(),
            revision: accepted.revision,
            authored: accepted.graph.clone(),
        };
        match live.apply(request) {
            Ok(()) => tracked.applied = Some(row),
            Err(error) => events.push(ControllerEvent::LiveApplyRefused {
                instance: id.clone(),
                revision: accepted.revision,
                error,
            }),
        }
    }

    fn retry_refused(&mut self, events: &mut Vec<ControllerEvent>) {
        for (id, tracked) in &mut self.tracked {
            let fetched = tracked
                .accepted
                .as_ref()
                .is_some_and(|accepted| accepted.definition == tracked.row.0 && accepted.revision == tracked.row.1);
            if fetched && tracked.applied.as_ref() != Some(&tracked.row) {
                Self::apply_live(&mut self.live, id, tracked, events);
            }
        }
    }

    fn stop_live(&mut self) {
        let sessions: Vec<InstanceId> = self.live.instances().cloned().collect();
        for id in sessions {
            if self.live.status(&id).is_some_and(|status| !status.stopping) {
                self.live.stop(&id);
            }
        }
    }

    // -- inspection ------------------------------------------------------

    /// The newest authoritative snapshot, as of the last pump.
    pub fn snapshot(&self) -> &Arc<EngineSnapshot> {
        &self.snapshot
    }

    /// The last fetched accepted execution, shared. It may lag the wanted
    /// row while a query is pending, or lead live activation while an owner
    /// prepares it. Compare its revision with `snapshot` and `live_status`;
    /// this is not a claim that every owner already runs this graph.
    /// `None` until the first successful fetch; unavailable replacements
    /// retain the previous accepted execution alongside their error event.
    pub fn execution(&self, instance: &InstanceId) -> Option<&Arc<InstanceExecution>> {
        self.tracked.get(instance)?.accepted.as_ref()
    }

    /// Every instance with a fetched execution, in instance order.
    pub fn executions(&self) -> impl Iterator<Item = (&InstanceId, &Arc<InstanceExecution>)> {
        self.tracked.iter().filter_map(|(id, tracked)| tracked.accepted.as_ref().map(|accepted| (id, accepted)))
    }

    /// The live host, read-only: status, plans, endpoints, probes, targets.
    pub fn live(&self) -> &LiveHost {
        &self.live
    }

    pub fn live_status(&self, instance: &InstanceId) -> Option<LiveStatus> {
        self.live.status(instance)
    }

    pub fn resolve_write(&self, instance: &InstanceId, node: &str, param: &str) -> Result<WriteTarget, WriteRefusal> {
        if self.stopping {
            return Err(WriteRefusal::NotRunning);
        }
        self.live.resolve_write(instance, node, param)
    }

    /// Write an admitted endpoint of exactly the generation the target
    /// names; a replaced generation refuses it rather than taking it.
    pub fn write_to(&mut self, target: &WriteTarget, value: EndpointValue) -> Result<Option<f64>, WriteRefusal> {
        if self.stopping {
            return Err(WriteRefusal::NotRunning);
        }
        self.live.write_to(target, value)
    }

    /// Send an owner-typed command to exactly the trigger endpoint the
    /// target names. Every refusal returns the command untouched; a
    /// stopping controller refuses with `NotRunning`.
    pub fn command_to(&mut self, target: &WriteTarget, command: OwnerCommand) -> Result<(), (WriteRefusal, OwnerCommand)> {
        if self.stopping {
            return Err((WriteRefusal::NotRunning, command));
        }
        self.live.command_to(target, command)
    }

    /// The resource behind exactly the trigger endpoint the target names;
    /// a stale target is refused rather than answered for a replacement.
    pub fn resource_to(&self, target: &WriteTarget) -> Result<Option<ResourceState>, WriteRefusal> {
        if self.stopping {
            return Err(WriteRefusal::NotRunning);
        }
        self.live.resource_to(target)
    }

    pub fn probe(&self, instance: &InstanceId, node: &str, port: &str) -> Option<ProbeValue> {
        self.live.probe(instance, node, port)
    }

    // -- the engine's event lanes, forwarded ------------------------------

    pub fn poll_transitions(&mut self, out: &mut Vec<Transition>, max: usize) -> TransitionPoll {
        self.engine.poll_transitions(out, max)
    }

    pub fn poll_observations(&mut self, out: &mut Vec<Observation>, max: usize) -> LaneGap {
        self.engine.poll_observations(out, max)
    }

    pub fn poll_journal(&mut self, out: &mut Vec<JournalEntry>, max: usize) -> JournalPoll {
        self.engine.poll_journal(out, max)
    }
}
