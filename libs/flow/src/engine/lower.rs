//! Lowering: the authored graph with its container nodes becomes the flat
//! graph one run executes.
//!
//! Every container instance expands to its component's graph with node ids
//! qualified by the instance path (`a/greet`, `a/shout/bang`). Boundary
//! nodes are **kept**: an inner `Input` becomes a `bound` passthrough fed by
//! the parent's wire (or stays an `input` with the parent's literal, or
//! its own default), and an inner `Output` becomes a `bound` passthrough
//! the parent's consumers read (`a.out(@greeting)` → `a/greeting.value`).
//! So every port a wire crosses is a real node with its own events, value
//! and row. The container itself is not a node of the lowered graph; a
//! host derives its state from its inner nodes. Nothing here runs: the
//! result goes to the unchanged scheduler and executors.

use crate::document::closure::{MAX_COMPONENT_DEPTH, MAX_INSTANCES, MAX_LOWERED_EDGES, MAX_LOWERED_NODES};
use crate::graph::edges_of;
use crate::{ComponentDef, ConnectionEntry, EdgeRef, Graph, Literal, Node, NodeInput, NodeInputValue, Port};
use std::collections::HashSet;

/// The kind of a boundary passthrough node.
pub const BOUND_KIND: &str = "bound";
/// The param naming which side of the boundary a `bound` node is.
pub const BOUNDARY_PARAM: &str = "boundary";

/// Whether a lowered node is a boundary node, and which side (`in` or
/// `out`).
pub fn boundary_side(node: &Node) -> Option<&str> {
    node.params.iter().find_map(|(key, value)| match value {
        Literal::Id(side) | Literal::Str(side) if key == BOUNDARY_PARAM => Some(side.as_str()),
        _ => None,
    })
}

/// Whether a lowered node may be requested as a run output: a root
/// `Output`/`Publish`, or a container's output boundary.
pub fn is_requestable_output(node: &Node) -> bool {
    matches!(node.kind.as_str(), "output" | "publish") || boundary_side(node) == Some("out")
}

/// The qualified id of `node` under `prefix` (`""` at the root).
pub fn qualified(prefix: &str, node: &str) -> String {
    format!("{prefix}{node}")
}

/// Lower `graph` (with `graph.components` as the closure's definitions).
pub fn lower(graph: &Graph) -> Result<Graph, String> {
    let mut out = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        components: Vec::new(),
        ..graph.clone()
    };
    let mut budget = Budget { edges: 0, instances: 0 };
    expand(graph, &graph.components, "", None, 0, &mut budget, &mut out)?;
    // Every edge (every entry of an ordered input) was counted against the
    // budget as its node was lowered; the projection is the shared one.
    out.edges = edges_of(&out.nodes);
    Ok(out)
}

/// What the lowering has grown to, checked before every step that grows.
struct Budget {
    edges: usize,
    instances: usize,
}

/// The parent's connection into one container port.
struct Parent<'a> {
    inputs: &'a [NodeInput],
}

fn expand(
    graph: &Graph,
    defs: &[ComponentDef],
    prefix: &str,
    parent: Option<&Parent<'_>>,
    depth: usize,
    budget: &mut Budget,
    out: &mut Graph,
) -> Result<(), String> {
    if depth > MAX_COMPONENT_DEPTH {
        return Err(format!("container `{prefix}` nests deeper than the {MAX_COMPONENT_DEPTH} level limit"));
    }
    let containers: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "component")
        .map(|node| node.id.as_str())
        .collect();
    let qualify_edge = |edge: &EdgeRef| -> EdgeRef {
        if containers.contains(edge.from_node.as_str()) {
            // A container's output port is its output boundary node.
            EdgeRef {
                from_node: format!("{prefix}{}/{}", edge.from_node, edge.from_port),
                from_port: "value".to_string(),
            }
        } else {
            EdgeRef { from_node: qualified(prefix, &edge.from_node), from_port: edge.from_port.clone() }
        }
    };
    // Entries keep their ids: an entry's identity is its destination input
    // and its id, the same in every instance of the component.
    let qualify_value = |value: &NodeInputValue| -> NodeInputValue {
        match value {
            NodeInputValue::Edge(edge) => NodeInputValue::Edge(qualify_edge(edge)),
            NodeInputValue::Edges(entries) => NodeInputValue::Edges(
                entries
                    .iter()
                    .map(|entry| ConnectionEntry { id: entry.id, from: qualify_edge(&entry.from) })
                    .collect(),
            ),
            literal => literal.clone(),
        }
    };
    for node in &graph.nodes {
        if node.kind == "component" {
            let def = defs
                .iter()
                .find(|def| def.alias == node.type_name)
                .ok_or_else(|| format!("container `{}{}` uses component `{}`, which is not pinned", prefix, node.id, node.type_name))?;
            // The container's inputs, as seen from inside: already qualified
            // to this level.
            let inputs: Vec<NodeInput> = node
                .inputs
                .iter()
                .map(|input| NodeInput {
                    port: input.port.clone(),
                    ty: input.ty.clone(),
                    many: input.many,
                    value: qualify_value(&input.value),
                })
                .collect();
            budget.instances += 1;
            if budget.instances > MAX_INSTANCES {
                return Err(format!("container `{prefix}{}` is past the {MAX_INSTANCES} instance limit", node.id));
            }
            let inner_prefix = format!("{prefix}{}/", node.id);
            expand(&def.graph, defs, &inner_prefix, Some(&Parent { inputs: &inputs }), depth + 1, budget, out)?;
            continue;
        }
        // Grown only within the budgets: nodes and edges are counted before
        // this node is cloned in.
        if out.nodes.len() + 1 > MAX_LOWERED_NODES {
            return Err(format!("the lowered graph has more than {MAX_LOWERED_NODES} nodes"));
        }
        let wires = |value: &NodeInputValue| match value {
            NodeInputValue::Edge(_) => 1,
            NodeInputValue::Edges(entries) => entries.len(),
            NodeInputValue::Literal(_) => 0,
        };
        // A boundary Input the parent wires lowers to the parent's wire or
        // entries, not to its own (it has none): those are what is counted,
        // before anything is cloned.
        let boundary_feed = match (node.kind.as_str(), parent) {
            ("input", Some(parent)) => parent.inputs.iter().find(|input| input.port == node.id).map(|input| &input.value),
            _ => None,
        };
        let node_edges = match boundary_feed {
            Some(value @ (NodeInputValue::Edge(_) | NodeInputValue::Edges(_))) => wires(value),
            _ => node.inputs.iter().map(|input| wires(&input.value)).sum::<usize>(),
        };
        if budget.edges + node_edges > MAX_LOWERED_EDGES {
            return Err(format!("the lowered graph has more than {MAX_LOWERED_EDGES} edges"));
        }
        budget.edges += node_edges;
        let id = qualified(prefix, &node.id);
        let mut lowered = node.clone();
        lowered.id = id;
        for input in &mut lowered.inputs {
            input.value = qualify_value(&input.value);
        }
        match (node.kind.as_str(), parent) {
            ("input", Some(_)) => {
                let side = (BOUNDARY_PARAM.to_string(), Literal::Id("in".to_string()));
                match boundary_feed {
                    Some(value @ (NodeInputValue::Edge(_) | NodeInputValue::Edges(_))) => {
                        // Wired from the parent: a passthrough of that wire
                        // (or of the ordered entries, in the port's shape)
                        // to the port the inner graph reads.
                        let port = node
                            .outputs
                            .first()
                            .cloned()
                            .ok_or_else(|| format!("boundary `{}` has no output port", lowered.id))?;
                        lowered.kind = BOUND_KIND.to_string();
                        lowered.type_name = "Bound".to_string();
                        lowered.inputs = vec![NodeInput {
                            port: "value".to_string(),
                            ty: port.ty.clone(),
                            many: port.many,
                            value: value.clone(),
                        }];
                        lowered.outputs = vec![port];
                        lowered.params = vec![side];
                    }
                    Some(NodeInputValue::Literal(literal)) => {
                        // Given literally by the parent: the input's value.
                        lowered.params.retain(|(key, _)| key != "value" && key != "default");
                        lowered.params.push(("value".to_string(), literal.clone()));
                        lowered.params.push(side);
                    }
                    None => lowered.params.push(side),
                }
            }
            ("output", Some(_)) => {
                let (ty, many) = node
                    .inputs
                    .iter()
                    .find(|input| input.port == "value")
                    .map(|input| (input.ty.clone(), input.many))
                    .ok_or_else(|| format!("boundary `{}` has no value input", lowered.id))?;
                lowered.kind = BOUND_KIND.to_string();
                lowered.type_name = "Bound".to_string();
                lowered.outputs = vec![Port { name: "value".to_string(), ty, many }];
                lowered.params = vec![(BOUNDARY_PARAM.to_string(), Literal::Id("out".to_string()))];
            }
            _ => {}
        }
        out.nodes.push(lowered);
    }
    Ok(())
}
