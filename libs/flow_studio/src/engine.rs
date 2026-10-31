//! The studio's job engine: a `makepad_flow` engine of its own, apart from
//! the live roots' controller, so a generation never touches the running
//! show. Everything goes through the engine's bounded non-blocking handle:
//! a fired-up preset is a chain of commands (load its source as an
//! ephemeral definition, instantiate, set its media inputs, start the run)
//! whose replies are polled each pump; run transitions become
//! [`StudioEvent`]s. A full command queue keeps the command and retries it
//! on the next pump; nothing is dropped.

use makepad_flow::document::closure::Closure;
use makepad_flow::document::ident::InstanceId;
use makepad_flow::engine::{RunEvent, RunId, Seams};
use makepad_flow::runtime::{
    Engine, EngineCommand, EngineConfig, EngineHandle, HostServices, Observation, Reply, ReplyPayload, RequestId, Submit,
    Transition, TransitionKind, TransitionPoll, READY_REQUEST,
};
use makepad_flow::engine::results::ResultStore;
use makepad_flow::{Graph, NodeState, Owner, PortValue, RunState, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const REPLIES_PER_PUMP: usize = 32;
const TRANSITIONS_PER_PUMP: usize = 128;
/// Progress is useful live and worthless late: the lane keeps the newest.
const OBSERVATIONS_PER_PUMP: usize = 256;

/// The engine definition a job runs as: its own, per job id.
pub fn definition_name(preset: &str, run: u64) -> String {
    format!("studio.{}.job{run}", super::presets::slug(preset))
}

/// What the studio asks one run to be.
pub struct RunSpec {
    pub run: u64,
    /// The definition name (one per preset: a re-run is a new revision).
    pub definition: String,
    pub source: String,
    pub file_name: String,
    /// Media inputs from the library: `(node, port, value)`.
    pub inputs: Vec<(String, String, Value)>,
}

#[derive(Debug)]
pub enum NodeEvent {
    Started,
    Progress { permille: Option<u16>, stage: String },
    Done(Vec<(String, PortValue)>),
    Failed(String),
    Skipped(String),
    /// A choice gate waits for the user: the candidates' indices.
    Waiting { question: String, options: usize },
    Answered,
}

#[derive(Debug)]
pub enum StudioEvent {
    Ready,
    /// The engine evaluated the run's source: the graph it will execute.
    Loaded { run: u64, graph: Graph },
    Started { run: u64, planned: Vec<String> },
    Node { run: u64, node: String, event: NodeEvent },
    /// `outputs`: the run's `Output` values (none after a resync).
    Finished { run: u64, state: RunState, secs: f64, warnings: Vec<String>, outputs: Vec<(String, PortValue)> },
    /// The run never started, or the engine refused a step.
    Failed { run: u64, error: String },
    /// Transitions were dropped under pressure: node states re-read from
    /// the engine's snapshot (previews of the nodes it covers are lost).
    Resync { run: u64, nodes: Vec<(String, NodeState, Option<String>)> },
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Load,
    Instantiate,
    Inputs,
    Start,
    Running,
}

struct Active {
    step: Step,
    definition: String,
    inputs: Vec<(String, String, Value)>,
    instance: Option<InstanceId>,
    engine_run: Option<RunId>,
    cancelled: bool,
}

pub struct StudioEngine {
    handle: EngineHandle,
    ready: bool,
    replies: Vec<Reply>,
    transitions: Vec<Transition>,
    observations: Vec<Observation>,
    /// What each outstanding request was for.
    pending: HashMap<RequestId, (u64, Step)>,
    /// Commands the queue had no room for, in order.
    deferred: VecDeque<(u64, Step, EngineCommand)>,
    active: HashMap<u64, Active>,
    stop_request: Option<RequestId>,
    stopped: bool,
    /// Runs a refused command ended, reported on the next pump.
    failures: Vec<(u64, String)>,
}

impl StudioEngine {
    pub fn start(cx: &makepad_widgets::Cx, seams: Seams, results: Option<Arc<dyn ResultStore>>) -> Result<StudioEngine, String> {
        let values = crate::host::host().values.clone();
        Self::start_with(crate::work::AppWork::new(cx), values, seams, results)
    }

    pub(super) fn start_with(work: Arc<dyn makepad_flow::runtime::HostWork>, values: std::path::PathBuf, seams: Seams,
        results: Option<Arc<dyn ResultStore>>) -> Result<StudioEngine, String> {
        let (registry, executors) = (crate::host::host().registries)()?;
        let mut config = EngineConfig::new(values);
        config.registry = registry;
        config.executors = executors;
        config.seams = seams;
        config.run_lanes = 2;
        let services = HostServices {
            work,
            results,
            log: Arc::new(|line: &str| makepad_widgets::log!("[flow-studio] {line}")),
            wake: Some(Arc::new(|| makepad_widgets::SignalToUI::set_ui_signal())),
        };
        let handle = Engine::start(config, services).map_err(|error| error.to_string())?;
        Ok(StudioEngine {
            handle,
            ready: false,
            replies: Vec::new(),
            transitions: Vec::new(),
            observations: Vec::new(),
            pending: HashMap::new(),
            deferred: VecDeque::new(),
            active: HashMap::new(),
            stop_request: None,
            stopped: false,
            failures: Vec::new(),
        })
    }

    pub fn ready(&self) -> bool {
        self.ready && !self.stopped && self.stop_request.is_none()
    }

    pub fn stopped(&self) -> bool {
        self.stopped || !self.handle.is_alive()
    }

    pub fn run(&mut self, spec: RunSpec) {
        let command = EngineCommand::LoadDefinition {
            name: spec.definition.clone(),
            source: spec.source,
            file_name: spec.file_name,
            closure: Some(Arc::new(Closure { modules: Vec::new() })),
            ephemeral: true,
        };
        self.active.insert(spec.run, Active {
            step: Step::Load, definition: spec.definition, inputs: spec.inputs,
            instance: None, engine_run: None, cancelled: false,
        });
        self.submit(spec.run, Step::Load, command);
    }

    /// Answer a waiting `Choose` node with the candidate at `index`.
    pub fn answer(&mut self, run: u64, node: &str, index: usize) -> Result<(), String> {
        let Some(active) = self.active.get(&run) else { return Err("the run is no longer active".into()) };
        let Some(instance) = active.instance.clone() else { return Err("the run has no instance yet".into()) };
        let inputs = vec![(node.to_string(), "pick".to_string(),
            PortValue::One(makepad_flow::engine::executors::choose::choice_answer(index)))];
        self.submit(run, Step::Running, EngineCommand::SetInputs { instance, inputs, actor: "flow-studio".into() });
        Ok(())
    }

    /// Cancel a run wherever it is: a started run is cancelled in the
    /// engine; one still being set up stops at its next reply.
    pub fn cancel(&mut self, run: u64) {
        let Some(active) = self.active.get_mut(&run) else { return };
        active.cancelled = true;
        if let Some(engine_run) = active.engine_run.clone() {
            self.submit(run, Step::Running, EngineCommand::CancelRun { run: engine_run });
        }
    }

    pub fn stop(&mut self) {
        if self.stop_request.is_some() || self.stopped {
            return;
        }
        match self.handle.stop() {
            Submit::Accepted(id) => self.stop_request = Some(id),
            _ => self.stopped = true,
        }
    }

    /// A finished run lets go of its instance and its own definition, so
    /// the engine holds only what is running.
    fn retire(&mut self, run: u64) {
        let Some(active) = self.active.remove(&run) else { return };
        if let Some(instance) = active.instance {
            self.submit(run, Step::Running, EngineCommand::DeleteInstance { instance });
        }
        self.submit(run, Step::Running, EngineCommand::RemoveDefinition { name: active.definition });
    }

    fn submit(&mut self, run: u64, step: Step, command: EngineCommand) {
        if !self.deferred.is_empty() {
            self.deferred.push_back((run, step, command));
            return;
        }
        self.submit_now(run, step, command);
    }

    fn submit_now(&mut self, run: u64, step: Step, command: EngineCommand) -> bool {
        match self.handle.submit(command) {
            Submit::Accepted(id) => {
                self.pending.insert(id, (run, step));
                true
            }
            Submit::Full(command) => {
                self.deferred.push_front((run, step, command));
                false
            }
            Submit::Closed(_) => {
                self.fail_later(run, "the studio engine stopped".into());
                true
            }
            Submit::Refused(_, refusal) => {
                self.fail_later(run, format!("refused: {refusal:?}"));
                true
            }
        }
    }

    fn fail_later(&mut self, run: u64, error: String) {
        // Once per run: retiring may itself meet the closed queue.
        let known = self.active.contains_key(&run);
        self.retire(run);
        if known {
            self.failures.push((run, error));
        }
    }

    pub fn pump(&mut self, out: &mut Vec<StudioEvent>) {
        for (run, error) in self.failures.drain(..) {
            out.push(StudioEvent::Failed { run, error });
        }
        while let Some((run, step, command)) = self.deferred.pop_front() {
            if !self.submit_now(run, step, command) {
                break;
            }
        }
        self.handle.poll_replies(&mut self.replies, REPLIES_PER_PUMP);
        for reply in std::mem::take(&mut self.replies) {
            self.reply(reply, out);
        }
        let poll = self.handle.poll_transitions(&mut self.transitions, TRANSITIONS_PER_PUMP);
        for transition in std::mem::take(&mut self.transitions) {
            self.transition(transition, out);
        }
        // The hub's progress for a generator (stage text and permille)
        // travels on the observation lane, apart from the transitions.
        self.handle.poll_observations(&mut self.observations, OBSERVATIONS_PER_PUMP);
        for observation in std::mem::take(&mut self.observations) {
            self.run_event(observation.run, observation.instance, observation.event, out);
        }
        if let TransitionPoll::Resync { snapshot, .. } = poll {
            for row in &snapshot.runs {
                let Some((&run, _)) = self.active.iter().find(|(_, active)| active.engine_run.as_ref().is_some_and(|id| id.0 == row.run_id)) else { continue };
                let nodes = row.nodes.iter().map(|(node, dto)| (node.clone(), dto.state, dto.error.clone())).collect();
                out.push(StudioEvent::Resync { run, nodes });
                if matches!(row.state, RunState::Done | RunState::Failed | RunState::Cancelled) {
                    self.retire(run);
                    let secs = row.finished_ms.map(|end| end.saturating_sub(row.started_ms) as f64 / 1000.0).unwrap_or(0.0);
                    out.push(StudioEvent::Finished { run, state: row.state, secs, warnings: vec!["some progress was dropped; states were re-read".into()], outputs: Vec::new() });
                }
            }
        }
        for (run, error) in self.failures.drain(..) {
            out.push(StudioEvent::Failed { run, error });
        }
    }

    fn reply(&mut self, reply: Reply, out: &mut Vec<StudioEvent>) {
        if reply.id == READY_REQUEST {
            self.ready = reply.result.is_ok();
            if let Err(error) = reply.result {
                makepad_widgets::log!("[flow-studio] engine failed to start: {error}");
                self.stopped = true;
            } else {
                out.push(StudioEvent::Ready);
            }
            return;
        }
        if Some(reply.id) == self.stop_request {
            self.stopped = true;
            out.push(StudioEvent::Stopped);
            return;
        }
        let Some((run, step)) = self.pending.remove(&reply.id) else { return };
        let payload = match reply.result {
            Ok(payload) => payload,
            Err(error) => {
                if step != Step::Running {
                    self.retire(run);
                    out.push(StudioEvent::Failed { run, error: error.to_string() });
                }
                return;
            }
        };
        let Some(active) = self.active.get_mut(&run) else { return };
        match (step, payload) {
            (Step::Load, ReplyPayload::Definition { error, graph, .. }) => {
                if let Some(error) = error {
                    self.retire(run);
                    out.push(StudioEvent::Failed { run, error: format!("{}:{}: {}", error.line, error.col, error.message) });
                    return;
                }
                if let Some(graph) = graph {
                    out.push(StudioEvent::Loaded { run, graph });
                }
                if active.cancelled {
                    self.retire(run);
                    out.push(StudioEvent::Finished { run, state: RunState::Cancelled, secs: 0.0, warnings: Vec::new(), outputs: Vec::new() });
                    return;
                }
                active.step = Step::Instantiate;
                let command = EngineCommand::Instantiate {
                    flow: active.definition.clone(), label: None, pinned: true, owner: Owner::Service, inputs: Default::default(),
                };
                self.submit(run, Step::Instantiate, command);
            }
            (Step::Instantiate, ReplyPayload::Instance(instance)) => {
                active.instance = Some(instance.clone());
                if active.cancelled {
                    self.retire(run);
                    out.push(StudioEvent::Finished { run, state: RunState::Cancelled, secs: 0.0, warnings: Vec::new(), outputs: Vec::new() });
                    return;
                }
                let inputs: Vec<(String, String, PortValue)> = std::mem::take(&mut active.inputs).into_iter()
                    .map(|(node, port, value)| (node, port, PortValue::One(value))).collect();
                if inputs.is_empty() {
                    active.step = Step::Start;
                    self.submit(run, Step::Start, EngineCommand::StartRun { instance, outputs: None });
                } else {
                    active.step = Step::Inputs;
                    self.submit(run, Step::Inputs, EngineCommand::SetInputs { instance, inputs, actor: "flow-studio".into() });
                }
            }
            (Step::Inputs, ReplyPayload::InputsSet(_)) => {
                let instance = active.instance.clone().expect("an instance before its inputs");
                active.step = Step::Start;
                self.submit(run, Step::Start, EngineCommand::StartRun { instance, outputs: None });
            }
            (Step::Start, ReplyPayload::RunStarted { run: engine_run, .. }) => {
                active.step = Step::Running;
                active.engine_run = Some(engine_run.clone());
                if active.cancelled {
                    self.submit(run, Step::Running, EngineCommand::CancelRun { run: engine_run });
                }
            }
            (Step::Running, _) => {}
            (step, payload) => {
                self.retire(run);
                out.push(StudioEvent::Failed { run, error: format!("unexpected engine reply at {step:?}: {payload:?}") });
            }
        }
    }

    fn transition(&mut self, transition: Transition, out: &mut Vec<StudioEvent>) {
        let TransitionKind::Run { run: engine_run, instance, event, .. } = transition.kind else { return };
        self.run_event(engine_run, instance, event, out);
    }

    fn run_event(&mut self, engine_run: RunId, instance: InstanceId, event: RunEvent, out: &mut Vec<StudioEvent>) {
        let Some((&run, active)) = self.active.iter_mut().find(|(_, active)| {
            active.engine_run.as_ref() == Some(&engine_run) || active.instance.as_ref() == Some(&instance)
        }) else { return };
        if active.engine_run.is_none() {
            active.engine_run = Some(engine_run);
        }
        let node = |node: String, event: NodeEvent| StudioEvent::Node { run, node, event };
        match event {
            RunEvent::RunStarted { planned_nodes, .. } => out.push(StudioEvent::Started { run, planned: planned_nodes }),
            RunEvent::NodeStarted { node: id } => out.push(node(id, NodeEvent::Started)),
            RunEvent::NodeProgress { node: id, permille, stage, stage_permille } => {
                let stage = match stage_permille {
                    Some(fraction) if permille.is_none() => format!("{stage} {}%", fraction / 10),
                    _ => stage,
                };
                out.push(node(id, NodeEvent::Progress { permille, stage }));
            }
            RunEvent::NodeDone { node: id, outputs } => out.push(node(id, NodeEvent::Done(outputs))),
            RunEvent::NodeFailed { node: id, error } => out.push(node(id, NodeEvent::Failed(error))),
            RunEvent::NodeSkipped { node: id, reason } => out.push(node(id, NodeEvent::Skipped(reason))),
            RunEvent::NodeWaiting { node: id, question, options, .. } => out.push(node(id, NodeEvent::Waiting { question, options: options.len() })),
            RunEvent::NodeAnswered { node: id, .. } => out.push(node(id, NodeEvent::Answered)),
            RunEvent::RunFinished { state, secs, warnings, outputs, .. } => {
                self.retire(run);
                out.push(StudioEvent::Finished { run, state, secs, warnings, outputs });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_flow::engine::Seams;

    const SOURCE: &str = "use mod.flow.*\nlet prompt = Input{ type: @text default: \"a fox\" }\nlet image = Image{ prompt: prompt.text() width: 64 height: 64 }\nlet out = Output{ type: @image value: image.image() }\nFlow{ prompt, image, out }\n";

    #[test]
    fn jobs_of_one_preset_run_side_by_side() {
        let values = std::env::temp_dir().join(format!("flow-studio-jobs-{}", std::process::id()));
        let work = makepad_flow::runtime::StdHostWork::new(2, 32, 4).unwrap();
        let seams = Seams { gen: Arc::new(crate::preview_gen::PreviewGen), ..Seams::unconfigured() };
        let mut engine = StudioEngine::start_with(work, values.clone(), seams, None).unwrap();
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut started = false;
        let mut finished: HashMap<u64, RunState> = HashMap::new();
        while finished.len() < 3 && std::time::Instant::now() < deadline {
            engine.pump(&mut events);
            if engine.ready() && !started {
                started = true;
                // Three jobs of the same preset, fired together.
                for run in 1..=3 {
                    engine.run(RunSpec { run, definition: definition_name("studio:test", run), source: SOURCE.into(), file_name: "test.splash".into(), inputs: Vec::new() });
                }
            }
            for event in events.drain(..) {
                match event {
                    StudioEvent::Finished { run, state, .. } => { finished.insert(run, state); }
                    StudioEvent::Failed { run, error } => panic!("job {run} failed: {error}"),
                    _ => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        engine.stop();
        let _ = std::fs::remove_dir_all(&values);
        assert_eq!(finished.len(), 3, "{finished:?}");
        assert!(finished.values().all(|state| *state == RunState::Done), "{finished:?}");
        assert!(engine.active.is_empty(), "every job retired its instance and definition");
    }

    #[test]
    fn every_job_has_its_own_definition() {
        assert_ne!(definition_name("studio:vj-loop", 1), definition_name("studio:vj-loop", 2));
    }
}
