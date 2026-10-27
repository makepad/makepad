//! The run scheduler.
//!
//! One implementation of run semantics, [`RunTask`]. Every host runs it on
//! the engine's long-lived run lanes (`runtime::lanes`); `engine::spawn_run*`
//! steps the same task on a thread of its own for callers that drive one run
//! by hand. A task is thread-affine because it owns the run's Splash VM;
//! whichever thread creates it steps it to the end.

use super::executors::ask::AskExecutor;
use super::executors::choose::ChooseExecutor;
use super::executors::lineage::Lineage;
use super::executors::map::{MapChild, MapRun};
#[cfg(feature = "media")]
use super::executors::archive::ArchiveExecutor;
#[cfg(feature = "ai")]
use super::executors::chat::ChatExecutor;
use super::executors::func::FuncExecutor;
#[cfg(feature = "ai")]
use super::executors::gen::{unsupported_params, GenExecutor};
#[cfg(feature = "ai")]
use makepad_ai_hub::job::UsedProviders;
#[cfg(feature = "http")]
use super::executors::http::HttpExecutor;
use super::executors::input::InputExecutor;
use super::executors::output::OutputExecutor;
#[cfg(feature = "media")]
use super::executors::publish::PublishExecutor;
use super::executors::{Executor, Poll, bound::BoundExecutor};
use super::results::ResultStore;
use super::{HttpLogEntry, NetPolicy, RunEvent, RunInput, Seams};
use crate::graph::FlowVm;
use crate::{Literal, Node, NodeInputValue, NodeState, PortValue, RunState, Sequence, Shape, Value};

/// A run's values by `(node, port)`, each in its port's shape: a present
/// empty sequence is a value; an absent key is a port that never produced.
type ValueTable = HashMap<(String, String), PortValue>;

/// A caller's values for node ports, by node then port.
type Overrides = std::collections::BTreeMap<String, std::collections::BTreeMap<String, PortValue>>;

/// Invalid caller values belong to their executing node's failure policy.
type OverrideErrors = std::collections::BTreeMap<String, String>;

/// The caller's values re-keyed from the authored graph (what admission
/// checked them against) to the lowered graph (what runs). A root node
/// keeps its id and port. A container is not a node of the lowered graph:
/// a value set on its port arrives at that port's boundary node, on
/// `value` when the parent wires it (a `bound` passthrough) or on the
/// boundary Input's own port when it does not. The shape is the same on
/// both sides, so the value admitted is the value resolved.
fn executable_overrides(
    authored: &crate::Graph,
    lowered: &crate::Graph,
    inputs: &Overrides,
) -> Result<(Overrides, OverrideErrors), String> {
    let mut out = Overrides::new();
    let mut errors = OverrideErrors::new();
    for (node_id, ports) in inputs {
        let node = authored
            .nodes
            .iter()
            .find(|node| node.id == *node_id)
            .ok_or_else(|| format!("a value is set on `{node_id}`, which is not declared"))?;
        let container = node.kind == "component";
        for (port, value) in ports {
            let shape = crate::instance::input_shape(node, port)
                .ok_or_else(|| format!("a value is set on `{node_id}.{port}`, which is not declared"))?;
            let (target, target_port) = if container {
                let boundary = format!("{node_id}/{port}");
                let node = lowered
                    .nodes
                    .iter()
                    .find(|lowered_node| lowered_node.id == boundary)
                    .ok_or_else(|| format!("the value set on `{node_id}.{port}` has no boundary `{boundary}` to arrive at"))?;
                // The boundary carries the port's shape on whichever side
                // the value arrives at; it must match the authored shape.
                let (target_port, target_shape) = if node.kind == super::lower::BOUND_KIND {
                    let input = node
                        .inputs
                        .iter()
                        .find(|input| input.port == "value")
                        .ok_or_else(|| format!("boundary `{boundary}` has no value input"))?;
                    ("value".to_string(), input.shape())
                } else {
                    let output = node.outputs.first().ok_or_else(|| format!("boundary `{boundary}` has no port"))?;
                    (output.name.clone(), output.shape())
                };
                if target_shape != shape {
                    return Err(format!(
                        "`{node_id}.{port}` is {} but its boundary `{boundary}` is {}",
                        shape.describe(),
                        target_shape.describe()
                    ));
                }
                (boundary, target_port)
            } else {
                (node_id.clone(), port.clone())
            };
            // Direct runs can bypass store admission. Reject invalid values
            // before cloning them, but let the executing node apply on_fail
            // rather than ending the whole run before its fallback can run.
            if let Err(error) = value.matches(shape) {
                errors.entry(target).or_insert_with(|| format!("the value set on `{node_id}.{port}`: {error}"));
                continue;
            }
            out.entry(target).or_default().insert(target_port, value.clone());
        }
    }
    Ok((out, errors))
}
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

enum ActiveExecutor {
    Input(InputExecutor),
    Output(OutputExecutor),
    Bound(BoundExecutor),
    Provider(Box<dyn Executor>),
    #[cfg(feature = "media")]
    Archive(ArchiveExecutor),
    #[cfg(feature = "ai")]
    Chat(ChatExecutor),
    #[cfg(feature = "ai")]
    Gen(GenExecutor),
    Func(FuncExecutor),
    #[cfg(feature = "http")]
    Http(HttpExecutor),
    Ask(AskExecutor),
    Choose(ChooseExecutor),
    Map(MapRun<ActiveExecutor>),
    #[cfg(feature = "media")]
    Publish(PublishExecutor),
}

impl MapChild for ActiveExecutor {
    fn poll(&mut self) -> Poll {
        ActiveExecutor::poll(self)
    }

    fn cancel(&mut self) {
        ActiveExecutor::cancel(self)
    }
}

impl ActiveExecutor {
    fn poll(&mut self) -> Poll {
        match self {
            Self::Input(value) => value.poll(),
            Self::Output(value) => value.poll(),
            Self::Bound(value) => value.poll(),
            Self::Provider(value) => value.poll(),
            #[cfg(feature = "media")]
            Self::Archive(value) => value.poll(),
            #[cfg(feature = "ai")]
            Self::Chat(value) => value.poll(),
            #[cfg(feature = "ai")]
            Self::Gen(value) => value.poll(),
            Self::Func(value) => value.poll(),
            #[cfg(feature = "http")]
            Self::Http(value) => value.poll(),
            Self::Ask(value) => value.poll(),
            Self::Choose(value) => value.poll(),
            // A Map starts its elements from the run step, which holds the
            // run's executors; here it only polls what already runs.
            Self::Map(value) => value.step(None),
            #[cfg(feature = "media")]
            Self::Publish(value) => value.poll(),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Input(value) => value.cancel(),
            Self::Output(value) => value.cancel(),
            Self::Bound(value) => value.cancel(),
            Self::Provider(value) => value.cancel(),
            #[cfg(feature = "media")]
            Self::Archive(value) => value.cancel(),
            #[cfg(feature = "ai")]
            Self::Chat(value) => value.cancel(),
            #[cfg(feature = "ai")]
            Self::Gen(value) => value.cancel(),
            Self::Func(value) => value.cancel(),
            #[cfg(feature = "http")]
            Self::Http(value) => value.cancel(),
            Self::Ask(value) => value.cancel(),
            Self::Choose(value) => value.cancel(),
            Self::Map(value) => value.cancel(),
            #[cfg(feature = "media")]
            Self::Publish(value) => value.cancel(),
        }
    }

    fn answer(&mut self, node: &str, value: Value) -> Result<bool, String> {
        match self {
            Self::Ask(ask) => ask.answer(node, value),
            Self::Choose(choose) => choose.answer(node, value),
            _ => Ok(false),
        }
    }

    fn is_archive(&self) -> bool {
        #[cfg(feature = "media")]
        {
            matches!(self, Self::Archive(_))
        }
        #[cfg(not(feature = "media"))]
        {
            false
        }
    }
}

/// What one [`RunTask::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStep {
    /// A node started, completed or reported; stepping again may do more.
    Progressed,
    /// Nothing changed; every live node is waiting on its executor.
    Idle,
    /// `RunFinished` was emitted. The task must not be stepped again.
    Finished,
}

/// Counts emitted events so a step can report whether it made progress.
struct Emitter<'a> {
    sink: &'a mut dyn FnMut(RunEvent),
    count: usize,
}

impl Emitter<'_> {
    fn send(&mut self, event: RunEvent) {
        self.count += 1;
        (self.sink)(event);
    }
}

/// One run: its VM, node states, live executors and values.
pub struct RunTask {
    input: RunInput,
    /// The lowered graph this run executes (`input.graph` is the authored one).
    graph: crate::Graph,
    vm: FlowVm,
    seams: Seams,
    policy: NetPolicy,
    cancel: Arc<AtomicBool>,
    results: Option<Arc<dyn ResultStore>>,
    started: Instant,
    #[cfg(feature = "ai")]
    used_providers: UsedProviders,
    http_log: Arc<Mutex<Vec<HttpLogEntry>>>,
    selected: HashSet<String>,
    states: HashMap<String, NodeState>,
    /// The caller's values keyed by what executes: a root node's port as
    /// given, a container's port as its boundary node (see
    /// `executable_overrides`).
    overrides: Overrides,
    /// Applied once, before dependency propagation, for selected nodes only.
    override_errors: OverrideErrors,
    values: ValueTable,
    active: HashMap<String, ActiveExecutor>,
    // Ordinary Output nodes are catalog sinks when the host has an asset
    // worker. Keep one publication per content digest for duplicate outputs
    // in the same run; explicit Publish nodes retain their own semantics.
    published_output_digests: HashSet<String>,
    in_flight_output_digests: HashSet<String>,
    output_publish_keys: HashMap<String, String>,
    archive_publish_keys: HashMap<String, Vec<String>>,
    warnings: Vec<String>,
    answers: VecDeque<(String, Value)>,
    /// Each Map body (a job the run executes only through its Map) and its
    /// Map. A body is never started on its own.
    map_bodies: HashMap<String, String>,
    finished: bool,
}

impl Drop for RunTask {
    fn drop(&mut self) {
        // Lane teardown and a caught executor panic also retire its work.
        // Normal cancellation already notified every owner before setting
        // `finished`; successful completion has no live executors left.
        if !self.finished {
            for executor in self.active.values_mut() {
                // One broken owner must not stop the other owners' cleanup
                // or trigger a second panic while a lane is unwinding.
                let _ = catch_unwind(AssertUnwindSafe(|| executor.cancel()));
            }
            #[cfg(feature = "ai")]
            let _ = catch_unwind(AssertUnwindSafe(|| self.used_providers.bye_all()));
        }
    }
}

impl RunTask {
    /// Announce the run, load its VM and validate the request. Returns
    /// `None` when the run already finished (its `RunFinished` was emitted).
    pub fn start(
        input: RunInput,
        seams: Seams,
        policy: NetPolicy,
        cancel: Arc<AtomicBool>,
        results: Option<Arc<dyn ResultStore>>,
        sink: &mut dyn FnMut(RunEvent),
    ) -> Option<Self> {
        let started = Instant::now();
        let flow = std::path::Path::new(&input.file_name)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(&input.file_name)
            .to_string();
        // Lowering needs only the authored graph, so the run starts at once
        // with the plan it will actually execute; loading the VM (the slow
        // part) comes after and a failure there ends a run that has begun.
        let lowered = match super::lower::lower(&input.graph) {
            Ok(lowered) => lowered,
            Err(error) => {
                let mut planned: Vec<String> = selected_nodes(&input.graph, input.outputs.as_deref()).into_iter().collect();
                planned.sort();
                sink(RunEvent::RunStarted {
                    run_id: input.run_id.clone(),
                    instance: input.instance.clone(),
                    flow: flow.clone(),
                    revision: input.graph_revision,
                    planned_nodes: planned,
                });
                sink(RunEvent::RunFinished {
                    state: RunState::Failed,
                    secs: started.elapsed().as_secs_f64(),
                    outputs: Vec::new(),
                    http_log: Vec::new(),
                    warnings: vec![format!("run graph lowering failed: {error}")],
                });
                return None;
            }
        };
        let selected_for_run = selected_nodes(&lowered, input.outputs.as_deref());
        let mut planned_nodes: Vec<String> = selected_for_run.iter().cloned().collect();
        planned_nodes.sort();
        sink(RunEvent::RunStarted {
            run_id: input.run_id.clone(),
            instance: input.instance.clone(),
            flow,
            revision: input.graph_revision,
            planned_nodes,
        });
        if let Err(error) = input
            .executors
            .validate_graph(&lowered, &input.registry, &selected_for_run)
            .and_then(|()| crate::graph::validate_maps(&lowered.nodes).map_err(|(_, error)| error))
        {
            sink(RunEvent::RunFinished {
                state: RunState::Failed,
                secs: started.elapsed().as_secs_f64(),
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: vec![error],
            });
            return None;
        }
        let (overrides, override_errors) = match executable_overrides(&input.graph, &lowered, &input.inputs) {
            Ok(overrides) => overrides,
            Err(error) => {
                sink(RunEvent::RunFinished {
                    state: RunState::Failed,
                    secs: started.elapsed().as_secs_f64(),
                    outputs: Vec::new(),
                    http_log: Vec::new(),
                    warnings: vec![error],
                });
                return None;
            }
        };
        let (mut vm, mut run_graph) = match FlowVm::load_with_registry(&input.source, &input.file_name, &input.closure, &input.registry) {
            Ok(loaded) => loaded,
            Err(error) => {
                sink(RunEvent::RunFinished {
                    state: RunState::Failed,
                    secs: started.elapsed().as_secs_f64(),
                    outputs: Vec::new(),
                    http_log: Vec::new(),
                    warnings: vec![format!("run source evaluation failed: {error}")],
                });
                return None;
            }
        };
        run_graph.revision = input.graph.revision;
        if super::graph_digest(&run_graph) != super::graph_digest(&input.graph) {
            sink(RunEvent::RunFinished {
                state: RunState::Failed,
                secs: started.elapsed().as_secs_f64(),
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: vec!["run source graph does not match requested graph".to_string()],
            });
            return None;
        }
        vm.set_run_graph(lowered.clone());
        let http_log = Arc::new(Mutex::new(Vec::<HttpLogEntry>::new()));
        let graph = &lowered;
        if let Some(invalid) = input.outputs.as_ref().and_then(|outputs| {
            outputs.iter().find(|output| {
                !graph.nodes.iter().any(|node| node.id == **output && super::lower::is_requestable_output(node))
            })
        }) {
            let mut emitter = Emitter { sink, count: 0 };
            finish(
                &mut emitter,
                RunState::Failed,
                started,
                graph,
                &HashMap::new(),
                input.outputs.as_deref(),
                &http_log,
                vec![format!("requested Output node `{invalid}` is not declared")],
            );
            return None;
        }
        let selected = selected_nodes(graph, input.outputs.as_deref());
        let states: HashMap<String, NodeState> = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.id.clone(),
                    if selected.contains(&node.id) {
                        NodeState::Pending
                    } else {
                        NodeState::Skipped
                    },
                )
            })
            .collect();
        let warnings = provider_warnings(graph, &selected);
        let map_bodies = map_bodies(graph);
        Some(Self {
            input,
            graph: lowered,
            vm,
            seams,
            policy,
            cancel,
            results,
            started,
            #[cfg(feature = "ai")]
            used_providers: UsedProviders::default(),
            http_log,
            selected,
            states,
            overrides,
            override_errors,
            values: HashMap::new(),
            active: HashMap::new(),
            published_output_digests: HashSet::new(),
            in_flight_output_digests: HashSet::new(),
            output_publish_keys: HashMap::new(),
            archive_publish_keys: HashMap::new(),
            warnings,
            answers: VecDeque::new(),
            map_bodies,
            finished: false,
        })
    }

    /// Queue an answer for a waiting Ask node; consumed by the next step.
    pub fn answer(&mut self, node: String, value: Value) {
        self.answers.push_back((node, value));
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Advance the run once: start every ready node, deliver answers, poll
    /// every live executor. Never sleeps and never blocks on a channel.
    pub fn step(&mut self, sink: &mut dyn FnMut(RunEvent)) -> RunStep {
        if self.finished {
            return RunStep::Finished;
        }
        let mut events = Emitter { sink, count: 0 };
        let mut changed = false;
        let graph = &self.graph;
        if self.cancel.load(Ordering::Relaxed) {
            for (node, executor) in &mut self.active {
                executor.cancel();
                self.states.insert(node.clone(), NodeState::Cancelled);
            }
            #[cfg(feature = "ai")]
            self.used_providers.bye_all();
            finish(
                &mut events,
                RunState::Cancelled,
                self.started,
                graph,
                &self.values,
                self.input.outputs.as_deref(),
                &self.http_log,
                std::mem::take(&mut self.warnings),
            );
            self.finished = true;
            return RunStep::Finished;
        }

        // A bad override is the node's own failure even if its wires are
        // not ready or have failed. Apply its fallback before considering
        // dependent nodes; errors outside the requested run stay silent.
        for (node, error) in std::mem::take(&mut self.override_errors) {
            if self.selected.contains(&node) {
                fail_node(&node, error, graph, &mut self.states, &mut self.values, &mut events);
            }
        }
        propagate_upstream(graph, &self.selected, &mut self.states, &self.values, &self.overrides, &mut events);
        settle_map_bodies(&self.map_bodies, &mut self.states, &mut events);

        let ready: Vec<String> = graph
            .nodes
            .iter()
            .filter(|node| {
                self.selected.contains(&node.id)
                    && self.states.get(&node.id) == Some(&NodeState::Pending)
                    && !self.map_bodies.contains_key(&node.id)
                    && is_ready(node, &self.overrides, &self.values)
                    // A Map is ready when its job could start: the body's
                    // own inputs are ready too.
                    && map_body(graph, node).is_none_or(|body| is_ready(body, &self.overrides, &self.values))
            })
            .map(|node| node.id.clone())
            .collect();
        for node_id in ready {
            let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
            self.states.insert(node_id.clone(), NodeState::Ready);
            if let Some(body) = map_body(graph, node) {
                let started = resolve_inputs(node, &self.overrides, &self.values)
                    .and_then(|map_inputs| {
                        let body_inputs = resolve_inputs(body, &self.overrides, &self.values)?;
                        MapRun::new(node, body, &map_inputs, body_inputs)
                    });
                match started {
                    Ok(map) => {
                        changed = true;
                        // The body runs inside its Map, once per element; as
                        // a node of its own it never runs.
                        self.states.insert(body.id.clone(), NodeState::Skipped);
                        events.send(RunEvent::NodeSkipped {
                            node: body.id.clone(),
                            reason: format!("runs inside Map `{node_id}`, once per element"),
                        });
                        self.states.insert(node_id.clone(), NodeState::Running);
                        events.send(RunEvent::NodeStarted { node: node_id.clone() });
                        self.active.insert(node_id, ActiveExecutor::Map(map));
                    }
                    Err(error) => fail_node(&node_id, error, graph, &mut self.states, &mut self.values, &mut events),
                }
                continue;
            }
            let resolved =
                match resolve_inputs(node, &self.overrides, &self.values) {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        fail_node(&node_id, error, graph, &mut self.states, &mut self.values, &mut events);
                        continue;
                    }
                };
            // Legacy auto-archive understands builtin scalar asset kinds.
            // Ordered and provider-defined artifacts remain flow results:
            // publishing a registered type needs its own explicit sink so
            // its metadata is not silently discarded by a builtin codec.
            let output_publish_key = (node.kind == "output" && archive_outputs(&self.results))
                .then(|| {
                    resolved.iter().find_map(|(port, value)| {
                        (port == "value").then(|| value.one()).flatten()
                            .filter(|value| !matches!(&value.ty, crate::PortType::Registered(_)))
                            .map(|value| {
                            format!("{}:{}:{}", value.digest_hex(), value.ty.as_str(), value.content_type)
                        })
                    })
                })
                .flatten();
            if output_publish_key
                .as_ref()
                .is_some_and(|key| self.in_flight_output_digests.contains(key))
            {
                // Wait for the first publisher instead of silently completing
                // a duplicate while its publication is still uncertain.
                self.states.insert(node_id.clone(), NodeState::Pending);
                continue;
            }
            let publish_output = output_publish_key
                .as_ref()
                .is_some_and(|key| !self.published_output_digests.contains(key));
            match start_executor(
                node,
                &resolved,
                &mut self.vm,
                &self.seams,
                &self.policy,
                &self.http_log,
                #[cfg(feature = "ai")]
                &self.used_providers,
                self.results.clone(),
                &self.input,
                default_publish_description(graph, &node.id, &self.values),
                if node.kind == "publish" { Lineage::of(graph, &node.id, &self.values) } else { Lineage::default() },
                publish_output,
            ) {
                Ok((executor, waiting)) => {
                    changed = true;
                    if publish_output {
                        let key = output_publish_key.unwrap();
                        self.in_flight_output_digests.insert(key.clone());
                        self.output_publish_keys.insert(node_id.clone(), key);
                    }
                    if waiting {
                        self.states.insert(node_id.clone(), NodeState::Waiting);
                        let output = node.outputs.first().unwrap();
                        // A Choose offers its candidates' indices.
                        let options = match &executor {
                            ActiveExecutor::Choose(choose) => choose.options(),
                            _ => literal_array_param(node, "options"),
                        };
                        events.send(RunEvent::NodeWaiting {
                            node: node_id.clone(),
                            question: string_param(node, "question"),
                            ty: output.ty.clone(),
                            options,
                        });
                    } else {
                        self.states.insert(node_id.clone(), NodeState::Running);
                        if !matches!(node.kind.as_str(), "input" | "output" | "fn") {
                            events.send(RunEvent::NodeStarted {
                                node: node_id.clone(),
                            });
                        }
                    }
                    self.active.insert(node_id, executor);
                }
                Err(error) => {
                    fail_node(&node_id, error, graph, &mut self.states, &mut self.values, &mut events)
                }
            }
        }

        let mut rejected_answers = Vec::new();
        while let Some((node, value)) = self.answers.pop_front() {
            let answer = self
                .active
                .get_mut(&node)
                .map(|executor| executor.answer(&node, value));
            match answer {
                Some(Ok(true)) => {
                    events.send(RunEvent::NodeAnswered {
                        node,
                        by: "caller".to_string(),
                    });
                }
                Some(Err(error)) => {
                    fail_node(&node, error, graph, &mut self.states, &mut self.values, &mut events);
                    rejected_answers.push(node);
                }
                _ => {}
            }
        }
        for node in rejected_answers {
            self.active.remove(&node);
        }

        let active_ids: Vec<_> = self.active.keys().cloned().collect();
        let mut completed = Vec::new();
        for node_id in active_ids {
            let is_archive = self.active.get(&node_id).is_some_and(ActiveExecutor::is_archive);
            let poll = match self.active.get_mut(&node_id).unwrap() {
                ActiveExecutor::Map(map) => {
                    let (vm, seams, policy, http_log, results, input) =
                        (&mut self.vm, &self.seams, &self.policy, &self.http_log, &self.results, &self.input);
                    #[cfg(feature = "ai")]
                    let used_providers = &self.used_providers;
                    let mut start = |element: &Node, inputs: &[(String, PortValue)]| {
                        start_executor(
                            element,
                            inputs,
                            vm,
                            seams,
                            policy,
                            http_log,
                            #[cfg(feature = "ai")]
                            used_providers,
                            results.clone(),
                            input,
                            String::new(),
                            Lineage::default(),
                            false,
                        )
                        .map(|(executor, _)| executor)
                    };
                    map.step(Some(&mut start))
                }
                executor => executor.poll(),
            };
            match poll {
                Poll::Pending => {}
                Poll::Progress { permille, stage } => {
                    events.send(RunEvent::NodeProgress {
                        node: node_id,
                        permille: Some(permille),
                        stage,
                        stage_permille: None,
                    });
                }
                Poll::Stage { stage, permille } => {
                    events.send(RunEvent::NodeProgress {
                        node: node_id,
                        permille: None,
                        stage,
                        stage_permille: permille,
                    });
                }
                Poll::Delta { port, text } => {
                    let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
                    if node.kind == "provider" && !node.outputs.iter().any(|output| {
                        output.name == port && output.ty == crate::PortType::Text && output.many.is_none()
                    }) {
                        self.active.get_mut(&node_id).unwrap().cancel();
                        fail_node(
                            &node_id,
                            format!("node `{node_id}` sent text delta on `{port}`, which is not a scalar text output"),
                            graph,
                            &mut self.states,
                            &mut self.values,
                            &mut events,
                        );
                        changed = true;
                        completed.push(node_id);
                        continue;
                    }
                    events.send(RunEvent::NodeDelta {
                        node: node_id,
                        port,
                        text,
                    });
                }
                Poll::Done(outputs) => {
                    changed = true;
                    let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
                    // What a node completes with must be what it declares:
                    // every declared port once, in its shape; an auxiliary
                    // port one value. Anything else fails the node.
                    if let Err(error) = validate_outputs(node, &outputs, &self.input.registry) {
                        self.active.get_mut(&node_id).unwrap().cancel();
                        fail_node(&node_id, error, graph, &mut self.states, &mut self.values, &mut events);
                        completed.push(node_id);
                        continue;
                    }
                    for (port, value) in &outputs {
                        self.values.insert((node_id.clone(), port.clone()), value.clone());
                    }
                    self.states.insert(node_id.clone(), NodeState::Done);
                    #[cfg(feature = "media")]
                    {
                        let archive = if node.kind == "gen" && !is_archive {
                            if let Some(worker) = self.results.clone().filter(|worker| worker.archive_outputs()) {
                                let flow = std::path::Path::new(&self.input.file_name)
                                    .file_stem().and_then(|name| name.to_str())
                                    .unwrap_or(&self.input.file_name).to_string();
                                let queued = self.published_output_digests.union(&self.in_flight_output_digests).cloned().collect();
                                match ArchiveExecutor::new(node, one_values(&outputs), worker, flow, self.input.instance.clone(), &queued) {
                                    Ok(archive) => Some(archive),
                                    Err(error) => {
                                        fail_node(&node_id, format!("Could not archive generated content: {error}"), graph, &mut self.states, &mut self.values, &mut events);
                                        completed.push(node_id);
                                        continue;
                                    }
                                }
                            } else { None }
                        } else { None };
                        if let Some(archive) = archive {
                            let keys: Vec<_> = archive.keys().collect();
                            if !keys.is_empty() {
                                for key in &keys {
                                    if !self.published_output_digests.contains(key) {
                                        self.in_flight_output_digests.insert(key.clone());
                                    }
                                }
                                self.states.insert(node_id.clone(), NodeState::Running);
                                self.archive_publish_keys.insert(node_id.clone(), keys);
                                self.active.insert(node_id.clone(), ActiveExecutor::Archive(archive));
                                continue;
                            }
                        }
                    }
                    #[cfg(not(feature = "media"))]
                    let _ = is_archive;
                    if let Some(key) = self.output_publish_keys.remove(&node_id) {
                        self.in_flight_output_digests.remove(&key);
                        self.published_output_digests.insert(key);
                    }
                    if let Some(keys) = self.archive_publish_keys.remove(&node_id) {
                        for key in keys {
                            self.in_flight_output_digests.remove(&key);
                            self.published_output_digests.insert(key);
                        }
                    }
                    if node.kind != "output" {
                        events.send(RunEvent::NodeDone {
                            node: node_id.clone(),
                            outputs,
                        });
                    }
                    completed.push(node_id);
                }
                Poll::Failed(error) => {
                    changed = true;
                    if let Some(keys) = self.archive_publish_keys.remove(&node_id) {
                        for key in keys { self.in_flight_output_digests.remove(&key); }
                    }
                    if let Some(key) = self.output_publish_keys.remove(&node_id) {
                        self.in_flight_output_digests.remove(&key);
                    }
                    fail_node(&node_id, error, graph, &mut self.states, &mut self.values, &mut events);
                    completed.push(node_id);
                }
            }
        }
        for node in completed {
            self.active.remove(&node);
        }

        propagate_upstream(graph, &self.selected, &mut self.states, &self.values, &self.overrides, &mut events);
        settle_map_bodies(&self.map_bodies, &mut self.states, &mut events);
        let finished = self.selected.iter().all(|node| {
            matches!(
                self.states.get(node),
                Some(
                    NodeState::Done
                        | NodeState::Failed
                        | NodeState::Skipped
                        | NodeState::Cancelled
                )
            )
        });
        if finished && self.active.is_empty() {
            let state = if self.states.values().any(|state| *state == NodeState::Failed) {
                RunState::Failed
            } else {
                RunState::Done
            };
            finish(
                &mut events,
                state,
                self.started,
                graph,
                &self.values,
                self.input.outputs.as_deref(),
                &self.http_log,
                std::mem::take(&mut self.warnings),
            );
            self.finished = true;
            return RunStep::Finished;
        }
        if changed || events.count > 0 {
            RunStep::Progressed
        } else {
            RunStep::Idle
        }
    }
}

fn archive_outputs(results: &Option<Arc<dyn ResultStore>>) -> bool {
    #[cfg(feature = "media")]
    {
        results.as_ref().is_some_and(|worker| worker.archive_outputs())
    }
    #[cfg(not(feature = "media"))]
    {
        let _ = results;
        false
    }
}

fn provider_warnings(graph: &crate::Graph, selected: &HashSet<String>) -> Vec<String> {
    #[cfg(feature = "ai")]
    {
        graph
            .nodes
            .iter()
            .filter(|node| selected.contains(&node.id) && node.kind == "gen")
            .flat_map(unsupported_params)
            .collect()
    }
    #[cfg(not(feature = "ai"))]
    {
        let _ = (graph, selected);
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)]
fn start_executor(
    node: &Node,
    inputs: &[(String, PortValue)],
    vm: &mut FlowVm,
    seams: &Seams,
    policy: &NetPolicy,
    http_log: &Arc<Mutex<Vec<HttpLogEntry>>>,
    #[cfg(feature = "ai")] used_providers: &UsedProviders,
    results: Option<Arc<dyn ResultStore>>,
    run: &RunInput,
    publish_description: String,
    lineage: Lineage,
    publish_output: bool,
) -> Result<(ActiveExecutor, bool), String> {
    // Parameters only some feature sets read.
    let _ = (seams, policy, http_log, &results, &publish_description, &lineage);
    // Boundaries are internal lowering artifacts. Every authored node uses
    // its descriptor's owner and kind, never a domain or serialized hint.
    let kind = if node.kind == super::lower::BOUND_KIND {
        super::lower::BOUND_KIND
    } else {
        let descriptor = run.registry.descriptor(&node.type_name).ok_or_else(|| {
            format!("node `{}` has unregistered type `{}`", node.id, node.type_name)
        })?;
        if descriptor.owner != crate::document::registry::BUILTIN_OWNER {
            let mut executor = run.executors.create(&descriptor.owner, node, &run.registry)?;
            if let Err(error) = executor.start(node, inputs) {
                executor.cancel();
                return Err(error);
            }
            return Ok((ActiveExecutor::Provider(executor), false));
        }
        descriptor.kind.as_str()
    };
    match kind {
        "input" => {
            let mut executor = InputExecutor::default();
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Input(executor), false))
        }
        "bound" => {
            let mut executor = BoundExecutor::default();
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Bound(executor), false))
        }
        "output" => {
            let mut executor = OutputExecutor::default();
            #[cfg(feature = "media")]
            if publish_output {
                let flow = std::path::Path::new(&run.file_name)
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&run.file_name)
                    .to_string();
                executor.start_with_publish(
                    node,
                    inputs,
                    results.ok_or_else(|| "result store missing for Output publication".to_string())?,
                    flow,
                    run.instance.clone(),
                )?;
                return Ok((ActiveExecutor::Output(executor), false));
            }
            #[cfg(not(feature = "media"))]
            let _ = (publish_output, run);
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Output(executor), false))
        }
        #[cfg(feature = "ai")]
        "chat" => {
            let mut executor = ChatExecutor::new(seams.chat.clone());
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Chat(executor), false))
        }
        #[cfg(feature = "ai")]
        "gen" => {
            let mut executor = GenExecutor::with_used_providers(
                seams.gen.clone(),
                run.origin.clone(),
                used_providers.clone(),
            );
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Gen(executor), false))
        }
        "fn" => {
            let mut executor = FuncExecutor::default();
            executor.start_with_vm(vm, node, inputs);
            Ok((ActiveExecutor::Func(executor), false))
        }
        #[cfg(feature = "http")]
        "http" => {
            let mut executor = HttpExecutor::new(
                seams.http.clone(),
                policy.clone(),
                http_log.clone(),
            );
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Http(executor), false))
        }
        "ask" => {
            let mut executor = AskExecutor::default();
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Ask(executor), true))
        }
        "choose" => {
            let mut executor = ChooseExecutor::default();
            let waiting = executor.begin(node, inputs)?;
            Ok((ActiveExecutor::Choose(executor), waiting))
        }
        #[cfg(feature = "media")]
        "publish" => {
            let flow = std::path::Path::new(&run.file_name)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(&run.file_name)
                .to_string();
            let mut executor = PublishExecutor::new(
                results,
                super::executors::publish::PublishRun {
                    flow,
                    instance: run.instance.clone(),
                    run: run.run_id.0.clone(),
                    revision: run.graph_revision,
                    description_default: publish_description,
                    lineage,
                },
            );
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Publish(executor), false))
        }
        // A provider kind reaches this arm only when its feature is off.
        kind => Err(if matches!(kind, "chat" | "gen" | "http" | "publish") {
            format!(
                "node `{}` needs the `{kind}` executor, which this build of makepad-flow does not include",
                node.id
            )
        } else {
            format!("node `{}` has unknown executor `{kind}`", node.id)
        }),
    }
}

/// Whether the caller supplied `node.port` for this run: such an input is
/// read from the caller, so it neither waits for its wires nor fails with
/// them. (The producers still run: demand is not pruned by an override.)
fn overridden(overrides: &Overrides, node: &str, port: &str) -> bool {
    overrides.get(node).is_some_and(|ports| ports.contains_key(port))
}

fn is_ready(node: &Node, overrides: &Overrides, values: &ValueTable) -> bool {
    node.inputs.iter().all(|input| overridden(overrides, &node.id, &input.port) || is_map_body_port(node, &input.port) || match &input.value {
        NodeInputValue::Literal(_) => true,
        NodeInputValue::Edge(edge) => values.contains_key(&(edge.from_node.clone(), edge.from_port.clone())),
        NodeInputValue::Edges(entries) => entries
            .iter()
            .all(|entry| values.contains_key(&(entry.from.from_node.clone(), entry.from.from_port.clone()))),
    })
}

/// Whether `port` of `node` is a Map's `body`: the wire naming the job it
/// runs per element, never read as a value.
fn is_map_body_port(node: &Node, port: &str) -> bool {
    node.kind == "map" && port == "body"
}

/// The body node of a Map.
fn map_body<'a>(graph: &'a crate::Graph, node: &Node) -> Option<&'a Node> {
    if node.kind != "map" {
        return None;
    }
    let edge = node.inputs.iter().find_map(|input| match &input.value {
        NodeInputValue::Edge(edge) if input.port == "body" => Some(edge),
        _ => None,
    })?;
    graph.nodes.iter().find(|candidate| candidate.id == edge.from_node)
}

/// Every Map body, with its Map.
fn map_bodies(graph: &crate::Graph) -> HashMap<String, String> {
    graph
        .nodes
        .iter()
        .filter_map(|node| map_body(graph, node).map(|body| (body.id.clone(), node.id.clone())))
        .collect()
}

/// A Map that settled without starting (skipped, failed) leaves its body
/// pending forever; it settles with it.
fn settle_map_bodies(bodies: &HashMap<String, String>, states: &mut HashMap<String, NodeState>, events: &mut Emitter<'_>) {
    for (body, map) in bodies {
        let map_settled = matches!(
            states.get(map),
            Some(NodeState::Failed | NodeState::Skipped | NodeState::Cancelled)
        );
        if map_settled && states.get(body) == Some(&NodeState::Pending) {
            states.insert(body.clone(), NodeState::Skipped);
            events.send(RunEvent::NodeSkipped { node: body.clone(), reason: format!("Map `{map}` did not run") });
        }
    }
}

/// The values of one ports only, for an executor path that takes no other.
#[cfg(feature = "media")]
fn one_values(inputs: &[(String, PortValue)]) -> Vec<(String, Value)> {
    inputs
        .iter()
        .filter_map(|(port, value)| value.one().map(|value| (port.clone(), value.clone())))
        .collect()
}

/// The shape a node's completion must give `port`: a declared output's,
/// an `Output`'s or boundary's `value` in its input's shape, and one value
/// for any auxiliary port an executor adds.
fn expected_output_shape(node: &Node, port: &str) -> Option<Shape> {
    if let Some(output) = node.outputs.iter().find(|output| output.name == port) {
        return Some(output.shape());
    }
    if node.kind == "output" && port == "value" {
        return node.inputs.iter().find(|input| input.port == "value").map(|input| input.shape());
    }
    None
}

/// Every declared output present exactly once in its shape; no port
/// twice; an undeclared port one value only.
fn validate_outputs(node: &Node, shaped: &[(String, PortValue)], registry: &crate::document::Registry) -> Result<(), String> {
    for (index, (port, value)) in shaped.iter().enumerate() {
        registry.types().require_artifact(&value.ty())
            .map_err(|error| format!("node `{}` output `{port}`: {error}", node.id))?;
        if shaped[..index].iter().any(|(other, _)| other == port) {
            return Err(format!("node `{}` produced `{port}` twice", node.id));
        }
        match expected_output_shape(node, port) {
            Some(shape) => value
                .matches(shape)
                .map_err(|error| format!("node `{}` output `{port}`: {error}", node.id))?,
            None if value.is_ordered() || node.kind == "provider" => {
                return Err(format!("node `{}` produced `{port}` it does not declare", node.id))
            }
            None => {}
        }
    }
    for output in &node.outputs {
        if !shaped.iter().any(|(port, _)| *port == output.name) {
            return Err(format!("node `{}` produced no `{}`", node.id, output.name));
        }
    }
    Ok(())
}

/// A node's inputs resolved in their declared shapes. An override (a
/// caller's value for any node port) must fit the port's shape; an ordered
/// input concatenates its entries' sources in order, every element checked
/// against the element type and the bound before it is appended.
fn resolve_inputs(
    node: &Node,
    instance_inputs: &Overrides,
    values: &ValueTable,
) -> Result<Vec<(String, PortValue)>, String> {
    let override_for =
        |port: &str| -> Option<PortValue> { instance_inputs.get(&node.id).and_then(|ports| ports.get(port)).cloned() };
    let mut resolved = Vec::new();
    for input in &node.inputs {
        // A Map's body is the job it runs, not a value it reads.
        if is_map_body_port(node, &input.port) {
            continue;
        }
        let value = if let Some(value) = override_for(&input.port) {
            Some(value)
        } else {
            match &input.value {
                NodeInputValue::Literal(Literal::Null) => None,
                NodeInputValue::Literal(literal) => Some(PortValue::One(
                    Value::from_literal(input.ty.clone(), literal).map_err(|error| {
                        format!("node `{}` input `{}`: {error}", node.id, input.port)
                    })?,
                )),
                NodeInputValue::Edge(edge) => Some(
                    values
                        .get(&(edge.from_node.clone(), edge.from_port.clone()))
                        .cloned()
                        .ok_or_else(|| format!("upstream value for `{}` is missing", input.port))?,
                ),
                NodeInputValue::Edges(entries) => {
                    let bound = input.many.ok_or_else(|| {
                        format!("node `{}` input `{}` lists entries but takes one value", node.id, input.port)
                    })?;
                    let mut sequence = Sequence::empty(input.ty.clone());
                    for entry in entries {
                        let source = values
                            .get(&(entry.from.from_node.clone(), entry.from.from_port.clone()))
                            .ok_or_else(|| format!("upstream value for `{}` is missing", input.port))?;
                        for element in source.values() {
                            if sequence.values.len() >= bound as usize {
                                return Err(format!(
                                    "node `{}` input `{}` would carry more than its bound of {bound} values",
                                    node.id, input.port
                                ));
                            }
                            if element.ty != input.ty {
                                return Err(format!(
                                    "node `{}` input `{}` expected {} elements, got {}",
                                    node.id,
                                    input.port,
                                    input.ty.as_str(),
                                    element.ty.as_str()
                                ));
                            }
                            sequence.values.push(element.clone());
                        }
                    }
                    Some(PortValue::Seq(sequence))
                }
            }
        };
        if let Some(value) = value {
            value
                .matches(input.shape())
                .map_err(|error| format!("node `{}` input `{}` {error}", node.id, input.port))?;
            resolved.push((input.port.clone(), value));
        }
    }
    if node.kind == "input" {
        for output in &node.outputs {
            if let Some(value) = override_for(&output.name) {
                value
                    .matches(output.shape())
                    .map_err(|error| format!("Input node `{}` {error}", node.id))?;
                resolved.push((output.name.clone(), value));
            }
        }
    }
    Ok(resolved)
}

fn fail_node(
    node_id: &str,
    error: String,
    graph: &crate::Graph,
    states: &mut HashMap<String, NodeState>,
    values: &mut ValueTable,
    events: &mut Emitter<'_>,
) {
    let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
    if node.on_fail == "skip" {
        for (port, value) in fallback_outputs(node) {
            values.insert((node_id.to_string(), port), value);
        }
        states.insert(node_id.to_string(), NodeState::Skipped);
        events.send(RunEvent::NodeSkipped {
            node: node_id.to_string(),
            reason: error,
        });
    } else {
        states.insert(node_id.to_string(), NodeState::Failed);
        events.send(RunEvent::NodeFailed {
            node: node_id.to_string(),
            error,
        });
    }
}

/// What a skipped node stands in with: a one port's authored literal, as
/// before; an ordered port only what its schema authored (an Input's
/// literal array). An ordered port with no authored value has no fallback
/// and stays absent, so its consumers settle as skipped.
fn fallback_outputs(node: &Node) -> Vec<(String, PortValue)> {
    let mut out = Vec::new();
    for port in &node.outputs {
        let candidate = if node.kind == "input" {
            param(node, "value").or_else(|| param(node, "default"))
        } else if node.kind == "ask" {
            param(node, "default")
        } else {
            param(node, &port.name).or_else(|| {
                node.inputs.iter().find_map(|input| {
                    (input.port == port.name).then_some(match &input.value {
                        NodeInputValue::Literal(value) => Some(value),
                        NodeInputValue::Edge(_) | NodeInputValue::Edges(_) => None,
                    })?
                })
            })
        };
        let Some(literal) = candidate else {
            continue;
        };
        match port.many {
            Some(_) => {
                if let (true, Literal::Arr(items)) = (node.kind == "input", literal) {
                    let values: Result<Vec<Value>, String> =
                        items.iter().map(|item| Value::from_literal(port.ty.clone(), item)).collect();
                    if let Ok(values) = values {
                        out.push((port.name.clone(), PortValue::Seq(Sequence { ty: port.ty.clone(), values })));
                    }
                }
            }
            None => {
                if !matches!(literal, Literal::Null) {
                    if let Ok(value) = Value::from_literal(port.ty.clone(), literal) {
                        out.push((port.name.clone(), PortValue::One(value)));
                    }
                }
            }
        }
    }
    out
}

fn propagate_upstream(
    graph: &crate::Graph,
    selected: &HashSet<String>,
    states: &mut HashMap<String, NodeState>,
    values: &ValueTable,
    overrides: &Overrides,
    events: &mut Emitter<'_>,
) {
    // A source that ended without a value blocks its consumer: one wire or
    // any entry of an ordered input, whatever the other entries did. An
    // input the caller supplied does not read its wires, so they cannot
    // block it.
    fn ended_empty(
        states: &HashMap<String, NodeState>,
        values: &ValueTable,
        from_node: &str,
        from_port: &str,
    ) -> bool {
        !values.contains_key(&(from_node.to_string(), from_port.to_string()))
            && matches!(
                states.get(from_node),
                Some(NodeState::Failed | NodeState::Skipped | NodeState::Cancelled)
            )
    }
    loop {
        let blocked: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| {
                selected.contains(&node.id)
                    && states.get(&node.id) == Some(&NodeState::Pending)
                    && node.inputs.iter().any(|input| !overridden(overrides, &node.id, &input.port) && match &input.value {
                        NodeInputValue::Literal(_) => false,
                        NodeInputValue::Edge(edge) => ended_empty(states, values, &edge.from_node, &edge.from_port),
                        NodeInputValue::Edges(entries) => entries
                            .iter()
                            .any(|entry| ended_empty(states, values, &entry.from.from_node, &entry.from.from_port)),
                    })
            })
            .map(|node| node.id.clone())
            .collect();
        if blocked.is_empty() {
            break;
        }
        for node in blocked {
            states.insert(node.clone(), NodeState::Skipped);
            events.send(RunEvent::NodeSkipped {
                node,
                reason: "upstream".to_string(),
            });
        }
    }
}

pub(crate) fn selected_nodes(graph: &crate::Graph, outputs: Option<&[String]>) -> HashSet<String> {
    let requested: Vec<String> = outputs
        .map(|outputs| outputs.to_vec())
        .unwrap_or_else(|| {
            graph
                .nodes
                .iter()
                .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
                .map(|node| node.id.clone())
                .collect()
        });
    let mut selected: HashSet<String> = requested.into_iter().collect();
    loop {
        let before = selected.len();
        for edge in &graph.edges {
            if selected.contains(&edge.to_node) {
                selected.insert(edge.from_node.clone());
            }
        }
        if selected.len() == before {
            return selected;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish(
    events: &mut Emitter<'_>,
    state: RunState,
    started: Instant,
    graph: &crate::Graph,
    values: &ValueTable,
    requested: Option<&[String]>,
    http_log: &Arc<Mutex<Vec<HttpLogEntry>>>,
    warnings: Vec<String>,
) {
    let output_ids: Vec<String> = requested
        .map(|values| values.to_vec())
        .unwrap_or_else(|| {
            graph
                .nodes
                .iter()
                .filter(|node| matches!(node.kind.as_str(), "output" | "publish"))
                .map(|node| node.id.clone())
                .collect()
        });
    let outputs = output_ids
        .into_iter()
        .filter_map(|node| {
            let port = graph
                .nodes
                .iter()
                .find(|candidate| candidate.id == node)
                .map(|candidate| if candidate.kind == "publish" { "asset" } else { "value" })
                .unwrap_or("value");
            values
                .get(&(node.clone(), port.to_string()))
                .cloned()
                .map(|value| (node, value))
        })
        .collect();
    events.send(RunEvent::RunFinished {
        state,
        secs: started.elapsed().as_secs_f64(),
        outputs,
        http_log: http_log.lock().unwrap().clone(),
        warnings,
    });
}

fn default_publish_description(
    graph: &crate::Graph,
    publish_node: &str,
    values: &ValueTable,
) -> String {
    let mut upstream = HashSet::from([publish_node.to_string()]);
    loop {
        let before = upstream.len();
        for edge in &graph.edges {
            if upstream.contains(&edge.to_node) {
                upstream.insert(edge.from_node.clone());
            }
        }
        if upstream.len() == before {
            break;
        }
    }
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == "input" && upstream.contains(&node.id))
        .find_map(|node| {
            node.outputs.iter().find_map(|port| {
                (port.ty == crate::PortType::Text)
                    .then(|| values.get(&(node.id.clone(), port.name.clone())))
                    .flatten()
                    .and_then(PortValue::one)
                    .and_then(|value| value.as_text().ok())
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_string)
            })
        })
        .unwrap_or_default()
}

fn param<'a>(node: &'a Node, name: &str) -> Option<&'a Literal> {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

fn string_param(node: &Node, name: &str) -> String {
    match param(node, name) {
        Some(Literal::Str(value) | Literal::Id(value)) => value.clone(),
        _ => String::new(),
    }
}

fn literal_array_param(node: &Node, name: &str) -> Vec<Literal> {
    match param(node, name) {
        Some(Literal::Arr(values)) => values.clone(),
        _ => Vec::new(),
    }
}
