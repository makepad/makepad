use super::executors::ask::AskExecutor;
use super::executors::archive::ArchiveExecutor;
use super::executors::chat::ChatExecutor;
use super::executors::func::FuncExecutor;
use super::executors::gen::{unsupported_params, GenExecutor, UsedProviders};
use super::executors::http::HttpExecutor;
use super::executors::input::InputExecutor;
use super::executors::output::OutputExecutor;
use super::executors::publish::{AssetWorkerHandle, PublishExecutor};
use super::executors::{Executor, Poll};
use super::{HttpLogEntry, NetPolicy, RunEvent, RunInput, Seams};
use crate::graph::FlowVm;
use crate::{Literal, Node, NodeInputValue, NodeState, RunState, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum ActiveExecutor {
    Input(InputExecutor),
    Output(OutputExecutor),
    Archive(ArchiveExecutor),
    Chat(ChatExecutor),
    Gen(GenExecutor),
    Func(FuncExecutor),
    Http(HttpExecutor),
    Ask(AskExecutor),
    Publish(PublishExecutor),
}

impl ActiveExecutor {
    fn poll(&mut self) -> Poll {
        match self {
            Self::Input(value) => value.poll(),
            Self::Output(value) => value.poll(),
            Self::Archive(value) => value.poll(),
            Self::Chat(value) => value.poll(),
            Self::Gen(value) => value.poll(),
            Self::Func(value) => value.poll(),
            Self::Http(value) => value.poll(),
            Self::Ask(value) => value.poll(),
            Self::Publish(value) => value.poll(),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Input(value) => value.cancel(),
            Self::Output(value) => value.cancel(),
            Self::Archive(value) => value.cancel(),
            Self::Chat(value) => value.cancel(),
            Self::Gen(value) => value.cancel(),
            Self::Func(value) => value.cancel(),
            Self::Http(value) => value.cancel(),
            Self::Ask(value) => value.cancel(),
            Self::Publish(value) => value.cancel(),
        }
    }

    fn answer(&mut self, node: &str, value: Value) -> Result<bool, String> {
        match self {
            Self::Ask(ask) => ask.answer(node, value),
            _ => Ok(false),
        }
    }
}

pub(crate) fn run(
    input: RunInput,
    vm: &mut FlowVm,
    seams: Seams,
    policy: NetPolicy,
    events: Sender<RunEvent>,
    answers: Receiver<(String, Value)>,
    cancel: Arc<AtomicBool>,
    assets: Option<AssetWorkerHandle>,
) {
    let started = Instant::now();
    let graph = &input.graph;
    let used_providers = UsedProviders::default();
    let http_log = Arc::new(Mutex::new(Vec::<HttpLogEntry>::new()));
    if let Some(invalid) = input.outputs.as_ref().and_then(|outputs| {
        outputs.iter().find(|output| {
            !graph
                .nodes
                .iter()
                .any(|node| {
                    node.id == **output && matches!(node.kind.as_str(), "output" | "publish")
                })
        })
    }) {
        finish(
            &events,
            RunState::Failed,
            started,
            graph,
            &HashMap::new(),
            input.outputs.as_deref(),
            &http_log,
            vec![format!("requested Output node `{invalid}` is not declared")],
        );
        return;
    }
    let selected = selected_nodes(graph, input.outputs.as_deref());
    let mut states: HashMap<String, NodeState> = graph
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
    let mut values: HashMap<(String, String), Value> = HashMap::new();
    let mut active: HashMap<String, ActiveExecutor> = HashMap::new();
    // Ordinary Output nodes are catalog sinks when the host has an asset
    // worker. Keep one publication per content digest for duplicate outputs
    // in the same run; explicit Publish nodes retain their own semantics.
    let mut published_output_digests = HashSet::new();
    let mut in_flight_output_digests = HashSet::new();
    let mut output_publish_keys = HashMap::new();
    let mut archive_publish_keys: HashMap<String, Vec<String>> = HashMap::new();
    let warnings: Vec<_> = graph
        .nodes
        .iter()
        .filter(|node| selected.contains(&node.id) && node.kind == "gen")
        .flat_map(unsupported_params)
        .collect();
    loop {
        if cancel.load(Ordering::Relaxed) {
            for (node, executor) in &mut active {
                executor.cancel();
                states.insert(node.clone(), NodeState::Cancelled);
            }
            used_providers.bye_all();
            finish(
                &events,
                RunState::Cancelled,
                started,
                graph,
                &values,
                input.outputs.as_deref(),
                &http_log,
                warnings,
            );
            return;
        }

        propagate_upstream(graph, &selected, &mut states, &values, &events);

        let ready: Vec<String> = graph
            .nodes
            .iter()
            .filter(|node| {
                selected.contains(&node.id)
                    && states.get(&node.id) == Some(&NodeState::Pending)
                    && is_ready(node, &values)
            })
            .map(|node| node.id.clone())
            .collect();
        for node_id in ready {
            let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
            states.insert(node_id.clone(), NodeState::Ready);
            let resolved = match resolve_inputs(node, &input.inputs, &values) {
                Ok(resolved) => resolved,
                Err(error) => {
                    fail_node(
                        &node_id,
                        error,
                        graph,
                        &mut states,
                        &mut values,
                        &events,
                    );
                    continue;
                }
            };
            let output_publish_key = (node.kind == "output" && assets.as_ref().is_some_and(|worker| worker.archive_outputs))
                .then(|| {
                    resolved.iter().find_map(|(port, value)| {
                        (port == "value").then(|| {
                            format!("{}:{}:{}", value.digest_hex(), value.ty.as_str(), value.content_type)
                        })
                    })
                })
                .flatten();
            if output_publish_key
                .as_ref()
                .is_some_and(|key| in_flight_output_digests.contains(key))
            {
                // Wait for the first publisher instead of silently completing
                // a duplicate while its publication is still uncertain.
                states.insert(node_id.clone(), NodeState::Pending);
                continue;
            }
            let publish_output = output_publish_key
                .as_ref()
                .is_some_and(|key| !published_output_digests.contains(key));
            match start_executor(
                node,
                &resolved,
                vm,
                &seams,
                &input.origin,
                &policy,
                &http_log,
                &used_providers,
                assets.clone(),
                &input,
                default_publish_description(graph, &node.id, &values),
                publish_output,
            ) {
                Ok((executor, waiting)) => {
                    if publish_output {
                        let key = output_publish_key.unwrap();
                        in_flight_output_digests.insert(key.clone());
                        output_publish_keys.insert(node_id.clone(), key);
                    }
                    if waiting {
                        states.insert(node_id.clone(), NodeState::Waiting);
                        let output = node.outputs.first().unwrap();
                        let _ = events.send(RunEvent::NodeWaiting {
                            node: node_id.clone(),
                            question: string_param(node, "question"),
                            ty: output.ty,
                            options: literal_array_param(node, "options"),
                        });
                    } else {
                        states.insert(node_id.clone(), NodeState::Running);
                        if !matches!(node.kind.as_str(), "input" | "output" | "fn") {
                            let _ = events.send(RunEvent::NodeStarted {
                                node: node_id.clone(),
                            });
                        }
                    }
                    active.insert(node_id, executor);
                }
                Err(error) => fail_node(
                    &node_id,
                    error,
                    graph,
                    &mut states,
                    &mut values,
                    &events,
                ),
            }
        }

        let mut rejected_answers = Vec::new();
        for (node, value) in answers.try_iter() {
            let answer = active
                .get_mut(&node)
                .map(|executor| executor.answer(&node, value));
            match answer {
                Some(Ok(true)) => {
                    let _ = events.send(RunEvent::NodeAnswered {
                        node,
                        by: "caller".to_string(),
                    });
                }
                Some(Err(error)) => {
                    fail_node(
                        &node,
                        error,
                        graph,
                        &mut states,
                        &mut values,
                        &events,
                    );
                    rejected_answers.push(node);
                }
                _ => {}
            }
        }
        for node in rejected_answers {
            active.remove(&node);
        }

        let active_ids: Vec<_> = active.keys().cloned().collect();
        let mut completed = Vec::new();
        for node_id in active_ids {
            let is_archive = matches!(active.get(&node_id), Some(ActiveExecutor::Archive(_)));
            let poll = active.get_mut(&node_id).unwrap().poll();
            match poll {
                Poll::Pending => {}
                Poll::Progress { permille, stage } => {
                    let _ = events.send(RunEvent::NodeProgress {
                        node: node_id,
                        permille,
                        stage,
                    });
                }
                Poll::Delta { port, text } => {
                    let _ = events.send(RunEvent::NodeDelta {
                        node: node_id,
                        port,
                        text,
                    });
                }
                Poll::Done(outputs) => {
                    let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
                    for (port, value) in &outputs {
                        values.insert((node_id.clone(), port.clone()), value.clone());
                    }
                    states.insert(node_id.clone(), NodeState::Done);
                    let archive = if node.kind == "gen" && !is_archive {
                        if let Some(worker) = assets.clone().filter(|worker| worker.archive_outputs) {
                            let flow = std::path::Path::new(&input.file_name)
                                .file_stem().and_then(|name| name.to_str())
                                .unwrap_or(&input.file_name).to_string();
                            let queued = published_output_digests.union(&in_flight_output_digests).cloned().collect();
                            match ArchiveExecutor::new(node, outputs.clone(), worker, flow, input.instance.clone(), &queued) {
                                Ok(archive) => Some(archive),
                                Err(error) => {
                                    fail_node(&node_id, format!("Could not archive generated content: {error}"), graph, &mut states, &mut values, &events);
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
                                if !published_output_digests.contains(key) {
                                    in_flight_output_digests.insert(key.clone());
                                }
                            }
                            states.insert(node_id.clone(), NodeState::Running);
                            archive_publish_keys.insert(node_id.clone(), keys);
                            active.insert(node_id.clone(), ActiveExecutor::Archive(archive));
                            continue;
                        }
                    }
                    if let Some(key) = output_publish_keys.remove(&node_id) {
                        in_flight_output_digests.remove(&key);
                        published_output_digests.insert(key);
                    }
                    if let Some(keys) = archive_publish_keys.remove(&node_id) {
                        for key in keys {
                            in_flight_output_digests.remove(&key);
                            published_output_digests.insert(key);
                        }
                    }
                    if node.kind != "output" {
                        let _ = events.send(RunEvent::NodeDone {
                            node: node_id.clone(),
                            outputs,
                        });
                    }
                    completed.push(node_id);
                }
                Poll::Failed(error) => {
                    if let Some(keys) = archive_publish_keys.remove(&node_id) {
                        for key in keys { in_flight_output_digests.remove(&key); }
                    }
                    if let Some(key) = output_publish_keys.remove(&node_id) {
                        in_flight_output_digests.remove(&key);
                    }
                    fail_node(
                        &node_id,
                        error,
                        graph,
                        &mut states,
                        &mut values,
                        &events,
                    );
                    completed.push(node_id);
                }
            }
        }
        for node in completed {
            active.remove(&node);
        }

        propagate_upstream(graph, &selected, &mut states, &values, &events);
        let finished = selected.iter().all(|node| {
            matches!(
                states.get(node),
                Some(
                    NodeState::Done
                        | NodeState::Failed
                        | NodeState::Skipped
                        | NodeState::Cancelled
                )
            )
        });
        if finished && active.is_empty() {
            let state = if states.values().any(|state| *state == NodeState::Failed) {
                RunState::Failed
            } else {
                RunState::Done
            };
            finish(
                &events,
                state,
                started,
                graph,
                &values,
                input.outputs.as_deref(),
                &http_log,
                warnings,
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn start_executor(
    node: &Node,
    inputs: &[(String, Value)],
    vm: &mut FlowVm,
    seams: &Seams,
    origin: &(String, u64),
    policy: &NetPolicy,
    http_log: &Arc<Mutex<Vec<HttpLogEntry>>>,
    used_providers: &UsedProviders,
    assets: Option<AssetWorkerHandle>,
    run: &RunInput,
    publish_description: String,
    publish_output: bool,
) -> Result<(ActiveExecutor, bool), String> {
    match node.kind.as_str() {
        "input" => {
            let mut executor = InputExecutor::default();
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Input(executor), false))
        }
        "output" => {
            let mut executor = OutputExecutor::default();
            if publish_output {
                let flow = std::path::Path::new(&run.file_name)
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&run.file_name)
                    .to_string();
                executor.start_with_asset_publish(
                    node,
                    inputs,
                    assets.ok_or_else(|| "asset worker missing for Output publication".to_string())?,
                    flow,
                    run.instance.clone(),
                )?;
            } else {
                executor.start(node, inputs)?;
            }
            Ok((ActiveExecutor::Output(executor), false))
        }
        "chat" => {
            let mut executor = ChatExecutor::new(seams.chat.clone());
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Chat(executor), false))
        }
        "gen" => {
            let mut executor = GenExecutor::with_used_providers(
                seams.gen.clone(),
                origin.clone(),
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
        "publish" => {
            let flow = std::path::Path::new(&run.file_name)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(&run.file_name)
                .to_string();
            let mut executor = PublishExecutor::new(
                assets,
                flow,
                run.instance.clone(),
                publish_description,
            );
            executor.start(node, inputs)?;
            Ok((ActiveExecutor::Publish(executor), false))
        }
        kind => Err(format!("node `{}` has unknown executor `{kind}`", node.id)),
    }
}

fn is_ready(node: &Node, values: &HashMap<(String, String), Value>) -> bool {
    node.inputs.iter().all(|input| match &input.value {
        NodeInputValue::Literal(_) => true,
        NodeInputValue::Edge(edge) => values.contains_key(&(
            edge.from_node.clone(),
            edge.from_port.clone(),
        )),
    })
}

fn resolve_inputs(
    node: &Node,
    instance_inputs: &std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, Value>,
    >,
    values: &HashMap<(String, String), Value>,
) -> Result<Vec<(String, Value)>, String> {
    let mut resolved = Vec::new();
    for input in &node.inputs {
        let override_value = instance_inputs
            .get(&node.id)
            .and_then(|ports| ports.get(&input.port));
        let value = if let Some(value) = override_value {
            Some(value.clone())
        } else {
            match &input.value {
                NodeInputValue::Literal(Literal::Null) => None,
                NodeInputValue::Literal(literal) => {
                    Some(Value::from_literal(input.ty, literal).map_err(|error| {
                        format!("node `{}` input `{}`: {error}", node.id, input.port)
                    })?)
                }
                NodeInputValue::Edge(edge) => Some(
                    values
                        .get(&(edge.from_node.clone(), edge.from_port.clone()))
                        .cloned()
                        .ok_or_else(|| format!("upstream value for `{}` is missing", input.port))?,
                ),
            }
        };
        if let Some(value) = value {
            if value.ty != input.ty {
                return Err(format!(
                    "node `{}` input `{}` expected {}, got {}",
                    node.id,
                    input.port,
                    input.ty.as_str(),
                    value.ty.as_str()
                ));
            }
            resolved.push((input.port.clone(), value));
        }
    }
    if node.kind == "input" {
        if let Some(ports) = instance_inputs.get(&node.id) {
            for (port, value) in ports {
                let expected = node
                    .outputs
                    .iter()
                    .find_map(|output| (output.name == *port).then_some(output.ty))
                    .ok_or_else(|| {
                        format!("Input node `{}` has no output port `{port}`", node.id)
                    })?;
                if value.ty != expected {
                    return Err(format!(
                        "Input node `{}` expected {}, got {}",
                        node.id,
                        expected.as_str(),
                        value.ty.as_str()
                    ));
                }
                resolved.push((port.clone(), value.clone()));
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
    values: &mut HashMap<(String, String), Value>,
    events: &Sender<RunEvent>,
) {
    let node = graph.nodes.iter().find(|node| node.id == node_id).unwrap();
    if node.on_fail == "skip" {
        for (port, value) in fallback_outputs(node) {
            values.insert((node_id.to_string(), port), value);
        }
        states.insert(node_id.to_string(), NodeState::Skipped);
        let _ = events.send(RunEvent::NodeSkipped {
            node: node_id.to_string(),
            reason: error,
        });
    } else {
        states.insert(node_id.to_string(), NodeState::Failed);
        let _ = events.send(RunEvent::NodeFailed {
            node: node_id.to_string(),
            error,
        });
    }
}

fn fallback_outputs(node: &Node) -> Vec<(String, Value)> {
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
                        NodeInputValue::Edge(_) => None,
                    })?
                })
            })
        };
        if let Some(literal) = candidate {
            if !matches!(literal, Literal::Null) {
                if let Ok(value) = Value::from_literal(port.ty, literal) {
                    out.push((port.name.clone(), value));
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
    values: &HashMap<(String, String), Value>,
    events: &Sender<RunEvent>,
) {
    loop {
        let blocked: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| {
                selected.contains(&node.id)
                    && states.get(&node.id) == Some(&NodeState::Pending)
                    && node.inputs.iter().any(|input| match &input.value {
                        NodeInputValue::Literal(_) => false,
                        NodeInputValue::Edge(edge) => {
                            !values.contains_key(&(edge.from_node.clone(), edge.from_port.clone()))
                                && matches!(
                                    states.get(&edge.from_node),
                                    Some(
                                        NodeState::Failed
                                            | NodeState::Skipped
                                            | NodeState::Cancelled
                                    )
                                )
                        }
                    })
            })
            .map(|node| node.id.clone())
            .collect();
        if blocked.is_empty() {
            break;
        }
        for node in blocked {
            states.insert(node.clone(), NodeState::Skipped);
            let _ = events.send(RunEvent::NodeSkipped {
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

fn finish(
    events: &Sender<RunEvent>,
    state: RunState,
    started: Instant,
    graph: &crate::Graph,
    values: &HashMap<(String, String), Value>,
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
    let _ = events.send(RunEvent::RunFinished {
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
    values: &HashMap<(String, String), Value>,
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
