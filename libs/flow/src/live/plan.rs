//! Partition a lowered revision by owner.
//!
//! The input is the same lowered graph a run executes (`engine::lower`), so
//! node ids are qualified (`lead/filter`) and component boundaries are real
//! `bound` nodes. A boundary is transparent here: a live wire is resolved
//! through every boundary it crosses to the node that produces it, keeping
//! the entry order of ordered inputs and recording the boundaries crossed.
//!
//! What cannot run is refused here, located, before anything is prepared:
//! a live type on a flow input or output, a job reading a live port, a live
//! node reading a job port (no latch in this slice), an unwired live
//! boundary, and a stream that would leave its owner.

use crate::document::ident::{DNode, DPort, InstancePath, NodeId, ParamRef};
use crate::document::registry::EndpointClass;
use crate::document::types::{Carrier, TypeKey};
use crate::document::Registry;
use crate::engine::lower::{boundary_side, BOUND_KIND};
use crate::{Graph, Literal, Node, NodeInputValue, PortType};
use std::collections::{BTreeMap, HashMap};

pub const MAX_LIVE_NODES: usize = 512;
pub const MAX_ENDPOINTS: usize = 1024;
/// Boundaries one wire may cross: the component depth limit, with room.
const MAX_BOUNDARY_HOPS: usize = 64;
const MAX_RESOLVED_SOURCES: usize = 16_384;
const MAX_RESOLVE_STEPS: usize = 262_144;

#[derive(Clone, Debug, PartialEq)]
pub struct PlanError {
    /// The qualified node the error is about, when there is one.
    pub node: Option<String>,
    pub port: Option<String>,
    pub message: String,
}

impl PlanError {
    pub fn at(node: &str, port: Option<&str>, message: impl Into<String>) -> Self {
        Self { node: Some(node.to_string()), port: port.map(str::to_string), message: message.into() }
    }

    pub fn general(message: impl Into<String>) -> Self {
        Self { node: None, port: None, message: message.into() }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.node, &self.port) {
            (Some(node), Some(port)) => write!(f, "`{node}.{port}`: {}", self.message),
            (Some(node), None) => write!(f, "`{node}`: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

/// Dense per plan. An endpoint is addressed by its [`ParamRef`]; the id is
/// only a position in this plan's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub enum EndpointKind {
    Control { min: f64, max: f64, step: Option<f64>, default: f64 },
    Toggle { default: bool },
    Trigger,
    /// Read-only: an output port an owner publishes the newest value of.
    Probe { ty: TypeKey },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub id: EndpointId,
    /// Qualified node and parameter (or probe port) name.
    pub addr: ParamRef,
    pub owner: String,
    pub kind: EndpointKind,
    pub doc: String,
    /// `node.port` of the wire that drives this control or toggle; a driven
    /// endpoint refuses writes and reads back the driven (effective) value.
    pub driver: Option<String>,
}

impl Endpoint {
    pub fn qualified(&self) -> String {
        format!("{}.{}", qualified(&self.addr.node), self.addr.param)
    }
}

/// One ordered entry's identity, qualified by its destination input.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionHop {
    pub input: DPort,
    pub conn: Option<u64>,
}

/// One producer of a live input, after every boundary between them.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSource {
    pub from: DPort,
    /// The entry id when this is one entry of an ordered input.
    pub conn: Option<u64>,
    /// The boundary nodes the wire crosses, consumer side first.
    pub via: Vec<DNode>,
    /// Consumer entry followed by each crossed boundary entry. Repeated
    /// references retain distinct identities even when they share a source.
    pub lineage: Vec<ConnectionHop>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveInput {
    pub port: String,
    pub ty: TypeKey,
    pub many: Option<u32>,
    /// In entry order. A one input has at most one.
    pub sources: Vec<LiveSource>,
    /// For a control input (a parameter with a signal): the range a driving
    /// value is normalized into, see [`normalize_signal`]. The default is the
    /// descriptor's, not the authored fallback.
    pub control: Option<EndpointKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveNode {
    pub node: DNode,
    /// `node` as the lowered graph spells it.
    pub id: String,
    pub type_name: String,
    /// Every descriptor parameter with its authored (or default) literal.
    pub params: Vec<(String, Literal)>,
    pub inputs: Vec<LiveInput>,
    pub outputs: Vec<(String, TypeKey)>,
}

impl LiveNode {
    pub fn param(&self, name: &str) -> Option<&Literal> {
        self.params.iter().find_map(|(key, value)| (key == name).then_some(value))
    }

    pub fn input(&self, port: &str) -> Option<&LiveInput> {
        self.inputs.iter().find(|input| input.port == port)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OwnerSlice {
    pub owner: String,
    /// Topological: a node follows every node it reads.
    pub nodes: Vec<LiveNode>,
}

/// The live part of one revision. Empty `slices` means the revision has no
/// live node and nothing runs.
#[derive(Clone, Debug, PartialEq)]
pub struct LivePlanSpec {
    pub definition: String,
    pub revision: u64,
    pub slices: Vec<OwnerSlice>,
    pub endpoints: Vec<Endpoint>,
    /// Public component output to its concrete producer's read-only endpoint.
    /// Scoped to this plan; aliases never participate in write resolution.
    pub probe_aliases: Vec<(DPort, EndpointId)>,
}

impl LivePlanSpec {
    pub fn endpoint(&self, node: &str, param: &str) -> Option<&Endpoint> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.addr.param == param && qualified(&endpoint.addr.node) == node)
    }

    pub fn probe_endpoint(&self, node: &str, port: &str) -> Option<&Endpoint> {
        if let Some(endpoint) = self.endpoint(node, port) {
            return matches!(endpoint.kind, EndpointKind::Probe { .. }).then_some(endpoint);
        }
        let (_, target) = self.probe_aliases.iter()
            .find(|(alias, _)| alias.port == port && qualified(&alias.node) == node)?;
        self.endpoints.get(target.0 as usize)
            .filter(|endpoint| matches!(endpoint.kind, EndpointKind::Probe { .. }))
    }
}

/// `lead/filter` as a [`DNode`].
pub fn dnode(id: &str) -> DNode {
    let mut parts: Vec<&str> = id.split('/').collect();
    let node = parts.pop().unwrap_or_default();
    DNode {
        path: InstancePath(parts.into_iter().map(|part| NodeId(part.to_string())).collect()),
        node: NodeId(node.to_string()),
    }
}

/// A [`DNode`] as the lowered graph spells it.
pub fn qualified(node: &DNode) -> String {
    let mut out = String::new();
    for part in &node.path.0 {
        out.push_str(&part.0);
        out.push('/');
    }
    out.push_str(&node.node.0);
    out
}

fn type_key(registry: &Registry, ty: &PortType, node: &str, port: &str) -> Result<(TypeKey, Carrier), PlanError> {
    let schema = registry
        .types()
        .resolve_port(ty)
        .map_err(|error| PlanError::at(node, Some(port), error.to_string()))?;
    Ok((schema.key.clone(), schema.carrier))
}

enum Class<'a> {
    Boundary,
    Job,
    Live(&'a str),
}

/// Partition `lowered`. `is_live` names the live owners; every other owner
/// is a job owner (the run scheduler checks those). The authored graph and
/// its closure must have passed `validate_projection_with_registry` before
/// lowering. `LiveHost` enforces this before calling this function.
pub fn partition(
    lowered: &Graph,
    registry: &Registry,
    is_live: &dyn Fn(&str) -> bool,
    definition: &str,
    revision: u64,
) -> Result<LivePlanSpec, PlanError> {
    let by_id: HashMap<&str, &Node> = lowered.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let classify = |node: &'_ Node| -> Result<Class<'_>, PlanError> {
        if node.kind == BOUND_KIND {
            return Ok(Class::Boundary);
        }
        let descriptor = registry
            .descriptor(&node.type_name)
            .ok_or_else(|| PlanError::at(&node.id, None, format!("node type `{}` is not registered", node.type_name)))?;
        Ok(if is_live(&descriptor.owner) { Class::Live(descriptor.owner.as_str()) } else { Class::Job })
    };

    let mut resolver = Resolver { by_id: &by_id, registry, steps: 0, emitted: 0 };

    let mut slices: BTreeMap<String, Vec<LiveNode>> = BTreeMap::new();
    let mut owner_of: HashMap<String, String> = HashMap::new();
    for node in &lowered.nodes {
        if let Class::Live(owner) = classify(node)? {
            owner_of.insert(node.id.clone(), owner.to_string());
        }
    }

    if owner_of.len() > MAX_LIVE_NODES {
        return Err(PlanError::general(format!("{} live nodes exceed the limit of {MAX_LIVE_NODES}", owner_of.len())));
    }

    for node in &lowered.nodes {
        let class = classify(node)?;
        match class {
            Class::Boundary => continue,
            Class::Job => {
                for (port, ty) in node.inputs.iter().map(|input| (&input.port, &input.ty))
                    .chain(node.outputs.iter().map(|output| (&output.name, &output.ty)))
                {
                    let (key, carrier) = type_key(registry, ty, &node.id, port)?;
                    if carrier != Carrier::Artifact && node.kind == "input" && node.id.contains('/') {
                        return Err(PlanError::at(&node.id, Some(port), format!(
                            "this component input carries the live type `{key}` and its parent does not wire it"
                        )));
                    }
                    if carrier != Carrier::Artifact {
                        return Err(PlanError::at(&node.id, Some(port), format!(
                            "`{key}` is a live type: it stays inside a live partition and cannot be a flow input, a result or a job's port"
                        )));
                    }
                    registry.types().require_artifact(ty)
                        .map_err(|error| PlanError::at(&node.id, Some(port), error.to_string()))?;
                }
                continue;
            }
            Class::Live(_) => {}
        }
        let owner = owner_of[&node.id].clone();
        let descriptor = registry.descriptor(&node.type_name).expect("classified as live through its descriptor");
        let mut inputs = Vec::with_capacity(node.inputs.len());
        for input in &node.inputs {
            let (ty, carrier) = type_key(registry, &input.ty, &node.id, &input.port)?;
            let wires: Vec<(&str, &str, Option<u64>)> = match &input.value {
                NodeInputValue::Edge(edge) => vec![(edge.from_node.as_str(), edge.from_port.as_str(), None)],
                NodeInputValue::Edges(entries) => entries
                    .iter()
                    .map(|entry| (entry.from.from_node.as_str(), entry.from.from_port.as_str(), Some(entry.id)))
                    .collect(),
                NodeInputValue::Literal(Literal::Null) => Vec::new(),
                NodeInputValue::Literal(_) => {
                    return Err(PlanError::at(&node.id, Some(&input.port), "a live input takes a wire, not a literal; set a parameter instead"))
                }
            };
            let mut sources = Vec::new();
            let limit = input.many.unwrap_or(1) as usize;
            for (from_node, from_port, conn) in wires {
                let lineage = vec![ConnectionHop { input: DPort { node: dnode(&node.id), port: input.port.clone() }, conn }];
                for source in resolver.resolve(&node.id, &input.port, from_node, from_port, conn,
                    &[], &lineage, limit.saturating_sub(sources.len()), false)? {
                    let from_id = qualified(&source.from.node);
                    match owner_of.get(&from_id) {
                        None => {
                            return Err(PlanError::at(&node.id, Some(&input.port), format!(
                                "it reads `{from_id}.{}`, a job's port; job results do not feed live nodes yet", source.from.port
                            )))
                        }
                        Some(from_owner) if *from_owner != owner => {
                            let reason = match carrier {
                                Carrier::AudioPort | Carrier::Frame | Carrier::Ref | Carrier::Event => "cannot leave the owner that produces it",
                                _ => "would cross owners, which no host relays yet",
                            };
                            return Err(PlanError::at(&node.id, Some(&input.port), format!(
                                "`{ty}` from `{from_id}` (owner `{from_owner}`) {reason}"
                            )));
                        }
                        Some(_) => {}
                    }
                    sources.push(source);
                }
            }
            if input.many.is_none() && sources.len() > 1 {
                return Err(PlanError::at(&node.id, Some(&input.port), "a one input resolved to several sources"));
            }
            if let Some(bound) = input.many {
                if sources.len() > bound as usize {
                    return Err(PlanError::at(&node.id, Some(&input.port), format!(
                        "{} sources arrive at an input bounded at {bound}", sources.len()
                    )));
                }
            }
            let control = descriptor.control_input(&input.port).map(|param| {
                let (min, max, step) = param.range.expect("the registry requires a range on an endpoint");
                let default = match param.default { Literal::Num(value) => value, _ => min };
                match param.endpoint {
                    Some(EndpointClass::Toggle) => EndpointKind::Toggle { default: default != 0.0 },
                    _ => EndpointKind::Control { min, max, step, default },
                }
            });
            if control.is_some() && sources.len() > 1 {
                return Err(PlanError::at(&node.id, Some(&input.port), "a control input takes one wire"));
            }
            inputs.push(LiveInput { port: input.port.clone(), ty, many: input.many, sources, control });
        }
        let mut outputs = Vec::with_capacity(node.outputs.len());
        for output in &node.outputs {
            if output.many.is_some() {
                return Err(PlanError::at(&node.id, Some(&output.name),
                    "ordered live producer outputs are not supported; use explicit scalar sources through an ordered input"));
            }
            outputs.push((output.name.clone(), type_key(registry, &output.ty, &node.id, &output.name)?.0));
        }
        let params = descriptor
            .params
            .iter()
            .map(|param| {
                let value = node.params.iter().find_map(|(key, value)| (*key == param.name).then(|| value.clone()));
                let mut value = value.unwrap_or_else(|| param.default.clone());
                if let Some(class @ (EndpointClass::Control | EndpointClass::Toggle)) = param.endpoint {
                    let Literal::Num(number) = value else {
                        return Err(PlanError::at(&node.id, Some(&param.name), "this parameter is a number"));
                    };
                    if !number.is_finite() {
                        return Err(PlanError::at(&node.id, Some(&param.name), "this parameter must be finite"));
                    }
                    if class == EndpointClass::Toggle {
                        if number != 0.0 && number != 1.0 {
                            return Err(PlanError::at(&node.id, Some(&param.name), "a toggle must be exactly 0 or 1"));
                        }
                    } else {
                        let (min, max, step) = param.range.expect("registered endpoint range");
                        value = Literal::Num(normalize_control(number, min, max, step));
                    }
                }
                Ok((param.name.clone(), value))
            })
            .collect::<Result<Vec<_>, PlanError>>()?;
        slices.entry(owner).or_default().push(LiveNode {
            node: dnode(&node.id), id: node.id.clone(), type_name: node.type_name.clone(), params, inputs, outputs,
        });
    }

    // A job never reads a live port, directly or through a boundary.
    for node in &lowered.nodes {
        if !matches!(classify(node)?, Class::Job) {
            continue;
        }
        for input in &node.inputs {
            let wires: Vec<(&str, &str)> = match &input.value {
                NodeInputValue::Edge(edge) => vec![(edge.from_node.as_str(), edge.from_port.as_str())],
                NodeInputValue::Edges(entries) => entries.iter().map(|entry| (entry.from.from_node.as_str(), entry.from.from_port.as_str())).collect(),
                NodeInputValue::Literal(_) => Vec::new(),
            };
            for (from_node, from_port) in wires {
                for source in resolver.resolve(&node.id, &input.port, from_node, from_port, None,
                    &[], &[], MAX_RESOLVED_SOURCES, true)? {
                    let from_id = qualified(&source.from.node);
                    if owner_of.contains_key(&from_id) {
                        return Err(PlanError::at(&node.id, Some(&input.port), format!(
                            "it reads live node `{from_id}`; a job cannot read a live port without an explicit capture node"
                        )));
                    }
                }
            }
        }
    }

    let total: usize = slices.values().map(Vec::len).sum();
    if total > MAX_LIVE_NODES {
        return Err(PlanError::general(format!("{total} live nodes exceed the limit of {MAX_LIVE_NODES}")));
    }

    let mut endpoints = Vec::new();
    let mut ordered_slices = Vec::with_capacity(slices.len());
    for (owner, nodes) in slices {
        let nodes = topological(nodes)?;
        for node in &nodes {
            let descriptor = registry.descriptor(&node.type_name).expect("live node has a descriptor");
            for param in &descriptor.params {
                let Some(class) = param.endpoint else { continue };
                let authored = node.param(&param.name).cloned().unwrap_or_else(|| param.default.clone());
                let kind = match class {
                    EndpointClass::Trigger => EndpointKind::Trigger,
                    EndpointClass::Control | EndpointClass::Toggle => {
                        let (min, max, step) = param.range.expect("the registry requires a range on an endpoint");
                        let Literal::Num(value) = authored else {
                            return Err(PlanError::at(&node.id, Some(&param.name), "this parameter is a number"));
                        };
                        if !value.is_finite() || !(min..=max).contains(&value) {
                            return Err(PlanError::at(&node.id, Some(&param.name), format!("{value} is outside {min}..{max}")));
                        }
                        if class == EndpointClass::Toggle {
                            EndpointKind::Toggle { default: value != 0.0 }
                        } else {
                            EndpointKind::Control { min, max, step, default: value }
                        }
                    }
                };
                if endpoints.len() >= MAX_ENDPOINTS {
                    return Err(PlanError::at(&node.id, Some(&param.name), "the live endpoint budget is exhausted"));
                }
                endpoints.push(Endpoint {
                    id: EndpointId(endpoints.len() as u32),
                    addr: ParamRef { node: node.node.clone(), param: param.name.clone() },
                    owner: owner.clone(),
                    kind,
                    doc: param.doc.clone().unwrap_or_default(),
                    driver: node.input(&param.name).filter(|input| input.control.is_some())
                        .and_then(|input| input.sources.first())
                        .map(|source| format!("{}.{}", qualified(&source.from.node), source.from.port)),
                });
            }
            for (port, ty) in &node.outputs {
                let carrier = registry.types().resolve(ty.as_str()).map(|schema| schema.carrier);
                if matches!(carrier, Ok(Carrier::Control | Carrier::Clock | Carrier::AudioPort)) {
                    if descriptor.params.iter().any(|param| param.name == *port && param.endpoint.is_some()) {
                        return Err(PlanError::at(&node.id, Some(port), "a probe output and control cannot share an endpoint name"));
                    }
                    if endpoints.len() >= MAX_ENDPOINTS {
                        return Err(PlanError::at(&node.id, Some(port), "the live endpoint budget is exhausted"));
                    }
                    endpoints.push(Endpoint {
                        id: EndpointId(endpoints.len() as u32),
                        addr: ParamRef { node: node.node.clone(), param: port.clone() },
                        owner: owner.clone(),
                        kind: EndpointKind::Probe { ty: ty.clone() },
                        doc: String::new(),
                        driver: None,
                    });
                }
            }
        }
        ordered_slices.push(OwnerSlice { owner, nodes });
    }
    if endpoints.len() > MAX_ENDPOINTS {
        return Err(PlanError::general(format!("{} endpoints exceed the limit of {MAX_ENDPOINTS}", endpoints.len())));
    }
    let mut probe_aliases = Vec::new();
    for node in lowered.nodes.iter().filter(|node| node.kind == BOUND_KIND && boundary_side(node) == Some("out")) {
        if endpoints.len() + probe_aliases.len() >= MAX_ENDPOINTS { break; }
        // Lowering spells component.output as component/output.value. An
        // ordered output has no single observable producer, so never sum or
        // choose an arbitrary entry merely to supply a probe.
        let Some((component, port)) = node.id.rsplit_once('/') else { continue };
        let Some(output) = node.outputs.iter().find(|output| output.name == "value" && output.many.is_none()) else { continue };
        let (_, carrier) = type_key(registry, &output.ty, &node.id, &output.name)?;
        if !matches!(carrier, Carrier::Control | Carrier::Clock | Carrier::AudioPort) { continue; }
        // Observation is optional: an unused, unwired boundary does not gain
        // an invented value or turn an otherwise valid execution into an error.
        // The shared resolver's visit/source/depth budgets still bound this walk.
        let Ok(sources) = resolver.resolve(&node.id, "value", &node.id, "value", None, &[], &[], 1, false) else { continue };
        let [source] = sources.as_slice() else { continue };
        let Some(target) = endpoints.iter().find(|endpoint| {
            endpoint.addr.node == source.from.node && endpoint.addr.param == source.from.port
                && matches!(endpoint.kind, EndpointKind::Probe { .. })
        }) else { continue };
        probe_aliases.push((DPort { node: dnode(component), port: port.to_string() }, target.id));
    }
    Ok(LivePlanSpec { definition: definition.to_string(), revision, slices: ordered_slices, endpoints, probe_aliases })
}

/// A driving value in `kind`'s terms: `None` for a non-finite value (the
/// consumer holds what it had); a toggle is on at `>= 0.5`; a control is
/// clamped to its range and snapped to its step. This scalar-to-toggle
/// conversion does not weaken the exact 0/1 rule for direct endpoint writes.
pub fn normalize_signal(kind: &EndpointKind, value: f64) -> Option<f64> {
    if !value.is_finite() { return None }
    match kind {
        EndpointKind::Toggle { .. } => Some(if value >= 0.5 { 1.0 } else { 0.0 }),
        EndpointKind::Control { min, max, step, .. } => Some(normalize_control(value, *min, *max, *step)),
        _ => None,
    }
}

/// Shared normalization for authored defaults, endpoint writes and signals.
pub fn normalize_control(number: f64, min: f64, max: f64, step: Option<f64>) -> f64 {
    let number = number.clamp(min, max);
    match step {
        Some(step) => (min + ((number - min) / step).round() * step).clamp(min, max),
        None => number,
    }
}

struct Resolver<'a> {
    by_id: &'a HashMap<&'a str, &'a Node>,
    registry: &'a Registry,
    steps: usize,
    emitted: usize,
}

impl Resolver<'_> {
    // Limits are checked before growing a result, including at every
    // intermediate boundary. The global visit/leaf budgets bound repeated
    // expansion even when a small authored graph fans out exponentially.
    fn resolve(
        &mut self,
        consumer: &str,
        port: &str,
        node_id: &str,
        node_port: &str,
        conn: Option<u64>,
        via: &[DNode],
        lineage: &[ConnectionHop],
        limit: usize,
        allow_job_literals: bool,
    ) -> Result<Vec<LiveSource>, PlanError> {
        if via.len() > MAX_BOUNDARY_HOPS {
            return Err(PlanError::at(consumer, Some(port), "the wire crosses too many component boundaries"));
        }
        if self.steps >= MAX_RESOLVE_STEPS {
            return Err(PlanError::at(consumer, Some(port), "live wire expansion exceeds its work budget"));
        }
        self.steps += 1;
        let node = *self.by_id.get(node_id)
            .ok_or_else(|| PlanError::at(consumer, Some(port), format!("the wire reads undeclared node `{node_id}`")))?;
        if !node.outputs.iter().any(|output| output.name == node_port) {
            return Err(PlanError::at(node_id, Some(node_port), "the wire reads an undeclared output"));
        }
        if node.kind != BOUND_KIND {
            if limit == 0 {
                return Err(PlanError::at(consumer, Some(port), "resolved sources exceed this input's bound"));
            }
            if self.emitted >= MAX_RESOLVED_SOURCES {
                return Err(PlanError::at(consumer, Some(port), "live wire expansion exceeds its source budget"));
            }
            self.emitted += 1;
            return Ok(vec![LiveSource {
                from: DPort { node: dnode(node_id), port: node_port.to_string() },
                conn, via: via.to_vec(), lineage: lineage.to_vec(),
            }]);
        }
        let input = node.inputs.iter().find(|input| input.port == "value")
            .ok_or_else(|| PlanError::at(node_id, Some("value"), "the boundary has no input"))?;
        let own_limit = input.many.unwrap_or(1) as usize;
        let effective_limit = own_limit.min(limit);
        let (limit_node, limit_port) = if own_limit <= limit { (node_id, "value") } else { (consumer, port) };
        let mut next_via = via.to_vec();
        next_via.push(dnode(node_id));
        let mut out = Vec::new();
        let mut follow = |from_node: &str, from_port: &str, entry: Option<u64>| -> Result<(), PlanError> {
            let mut next_lineage = lineage.to_vec();
            next_lineage.push(ConnectionHop { input: DPort { node: dnode(node_id), port: "value".to_string() }, conn: entry });
            let resolved = self.resolve(limit_node, limit_port, from_node, from_port, conn, &next_via,
                &next_lineage, effective_limit.saturating_sub(out.len()), allow_job_literals)?;
            out.extend(resolved);
            Ok(())
        };
        match &input.value {
            NodeInputValue::Edge(edge) => follow(&edge.from_node, &edge.from_port, None)?,
            NodeInputValue::Edges(entries) => {
                for entry in entries {
                    follow(&entry.from.from_node, &entry.from.from_port, Some(entry.id))?;
                }
            }
            NodeInputValue::Literal(_) if allow_job_literals => {
                self.registry.types().require_artifact(&input.ty)
                    .map_err(|error| PlanError::at(node_id, Some("value"), error.to_string()))?;
            }
            NodeInputValue::Literal(_) => return Err(PlanError::at(node_id, Some("value"), "this live boundary is not wired")),
        }
        Ok(out)
    }
}

fn topological(nodes: Vec<LiveNode>) -> Result<Vec<LiveNode>, PlanError> {
    let index: HashMap<String, usize> = nodes.iter().enumerate().map(|(index, node)| (node.id.clone(), index)).collect();
    let mut pending: Vec<usize> = nodes
        .iter()
        .map(|node| {
            node.inputs
                .iter()
                .flat_map(|input| &input.sources)
                .filter(|source| index.contains_key(&qualified(&source.from.node)))
                .count()
        })
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    let mut ready: Vec<usize> = (0..nodes.len()).filter(|index| pending[*index] == 0).collect();
    ready.reverse();
    while let Some(next) = ready.pop() {
        order.push(next);
        for (other, node) in nodes.iter().enumerate() {
            let reads = node
                .inputs
                .iter()
                .flat_map(|input| &input.sources)
                .filter(|source| qualified(&source.from.node) == nodes[next].id)
                .count();
            if reads > 0 {
                pending[other] -= reads;
                if pending[other] == 0 {
                    ready.push(other);
                }
            }
        }
    }
    if order.len() != nodes.len() {
        return Err(PlanError::general("the live nodes of one owner form a cycle"));
    }
    let mut slots: Vec<Option<LiveNode>> = nodes.into_iter().map(Some).collect();
    Ok(order.into_iter().map(|index| slots[index].take().expect("each node is ordered once")).collect())
}
