//! Pure edits over the wire `Graph`: every canvas gesture builds the next
//! graph here and the app PUTs it. Nothing in this module talks to the
//! server or the widgets, so the helpers are unit-tested as plain data.

use makepad_flow::{
    Edge, EdgeRef, Graph, Literal, Node, NodeInput, NodeInputValue, NodeTypeCatalog, Port,
    PortType,
};
use makepad_widgets::makepad_micro_serde::JsonValue;
use std::collections::{HashMap, HashSet};

/// Canvas geometry shared by auto-placement and the canvas itself.
pub const NODE_WIDTH: f64 = 300.0;
pub const COLUMN_GAP: f64 = 60.0;
pub const ROW_GAP: f64 = 260.0;
pub const FIRST_AT: (f64, f64) = (40.0, 120.0);

/// Node lookup and reverse adjacency, rebuilt once when a graph changes.
/// A wire drag walks the source's ancestors once and then scans candidate
/// ports, instead of rebuilding adjacency for every candidate.
#[derive(Default)]
pub struct GraphIndex {
    nodes: HashMap<String, usize>,
    upstream: Vec<Vec<usize>>,
}

impl GraphIndex {
    pub fn new(graph: &Graph) -> Self {
        let nodes: HashMap<String, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        let mut upstream = vec![Vec::new(); graph.nodes.len()];
        for edge in &graph.edges {
            let (Some(from), Some(to)) = (nodes.get(&edge.from_node), nodes.get(&edge.to_node)) else {
                continue;
            };
            upstream[*to].push(*from);
        }
        Self { nodes, upstream }
    }

    pub fn node(&self, id: &str) -> Option<usize> {
        self.nodes.get(id).copied()
    }

    fn ancestors(&self, node: usize) -> HashSet<usize> {
        let mut seen = HashSet::new();
        let mut stack = vec![node];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            stack.extend(self.upstream[node].iter().copied());
        }
        seen
    }

    pub fn ancestor_indices(&self, node: &str) -> HashSet<usize> {
        self.node(node)
            .map(|node| self.ancestors(node))
            .unwrap_or_default()
    }

    pub fn compatible_inputs(
        &self,
        graph: &Graph,
        from_node: &str,
        from_port: &str,
    ) -> Vec<(usize, usize)> {
        let Some(from) = self.node(from_node) else {
            return Vec::new();
        };
        let Some(ty) = graph.nodes[from]
            .outputs
            .iter()
            .find(|output| output.name == from_port)
            .map(|output| output.ty)
        else {
            return Vec::new();
        };
        let ancestors = self.ancestors(from);
        let mut out = Vec::new();
        for (node_index, node) in graph.nodes.iter().enumerate() {
            if node_index == from || ancestors.contains(&node_index) {
                continue;
            }
            for (port_index, input) in node.inputs.iter().enumerate() {
                let flexible = node.type_name == "Fn"
                    || (node.type_name == "Http"
                        && (input.port == "body" || input.port == "headers"))
                    || (node.type_name == "Output" && input.port == "value");
                if flexible || input.ty == ty {
                    out.push((node_index, port_index));
                }
            }
        }
        out
    }
}

/// A fresh id `<type>_<n>`, lower-cased, that no node in the graph uses.
pub fn fresh_node_id(graph: &Graph, type_name: &str) -> String {
    let base: String = type_name
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let base = if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
        format!("node_{base}")
    } else {
        base
    };
    let mut n = 1;
    loop {
        let candidate = format!("{base}_{n}");
        if !graph.nodes.iter().any(|node| node.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn literal_from_json(value: &JsonValue) -> Literal {
    match value {
        JsonValue::Null | JsonValue::Undefined => Literal::Null,
        JsonValue::Bool(value) => Literal::Bool(*value),
        JsonValue::U64(value) => Literal::Num(*value as f64),
        JsonValue::U128(value) => Literal::Num(*value as f64),
        JsonValue::I64(value) => Literal::Num(*value as f64),
        JsonValue::I128(value) => Literal::Num(*value as f64),
        JsonValue::F64(value) => Literal::Num(*value),
        JsonValue::String(value) => Literal::Str(value.clone()),
        JsonValue::BareIdent(value) => Literal::Id(value.clone()),
        JsonValue::Char(value) => Literal::Str(value.to_string()),
        JsonValue::Array(values) => Literal::Arr(values.iter().map(literal_from_json).collect()),
        JsonValue::Object(fields) => {
            let mut pairs: Vec<(String, Literal)> = fields
                .iter()
                .map(|(key, value)| (key.clone(), literal_from_json(value)))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Literal::Obj(pairs)
        }
    }
}

/// The catalog stores id-typed params (`type: @text`, `method: @get`) as
/// plain strings; the writer must emit them as `@ids`, so the params that
/// the prelude declares as ids are re-typed here.
fn id_param(type_name: &str, name: &str) -> bool {
    matches!(
        (type_name, name),
        ("Input" | "Text" | "Output" | "Ask", "type")
            | ("Http", "method" | "out")
            | (_, "on_fail")
    )
}

pub fn port_type_from_name(name: &str) -> Option<PortType> {
    Some(match name {
        "text" => PortType::Text,
        "image" => PortType::Image,
        "audio" => PortType::Audio,
        "video" => PortType::Video,
        "mesh" => PortType::Mesh,
        "json" => PortType::Json,
        "list" => PortType::List,
        "bytes" => PortType::Bytes,
        _ => return None,
    })
}

fn param_port_type(params: &[(String, Literal)], key: &str) -> Option<PortType> {
    params.iter().find_map(|(name, value)| {
        if name != key {
            return None;
        }
        match value {
            Literal::Id(text) | Literal::Str(text) => port_type_from_name(text),
            _ => None,
        }
    })
}

/// Build a node of `catalog`'s type with the prelude's defaults.
pub fn node_from_catalog(id: String, catalog: &NodeTypeCatalog, at: (f64, f64)) -> Node {
    let mut params: Vec<(String, Literal)> = catalog
        .params
        .iter()
        .map(|param| {
            let mut literal = literal_from_json(&param.default);
            if id_param(&catalog.type_name, &param.name) {
                if let Literal::Str(text) = &literal {
                    literal = Literal::Id(text.clone());
                }
            }
            (param.name.clone(), literal)
        })
        .collect();
    if catalog.type_name == "Fn" && !params.iter().any(|(name, _)| name == "out") {
        params.push(("out".to_string(), Literal::Arr(vec![Literal::Id("text".into())])));
    }
    let mut inputs: Vec<NodeInput> = catalog
        .ports
        ._in
        .iter()
        .map(|port| NodeInput {
            port: port.name.clone(),
            ty: port.ty,
            value: NodeInputValue::Literal(Literal::Null),
        })
        .collect();
    let mut outputs: Vec<Port> = catalog.ports.out.clone();
    match catalog.type_name.as_str() {
        "Input" | "Text" | "Ask" => {
            let ty = param_port_type(&params, "type").unwrap_or(PortType::Text);
            outputs = vec![Port {
                name: ty.as_str().to_string(),
                ty,
            }];
        }
        "Output" => {
            let ty = param_port_type(&params, "type").unwrap_or(PortType::Text);
            inputs = vec![NodeInput {
                port: "value".to_string(),
                ty,
                value: NodeInputValue::Literal(Literal::Null),
            }];
            outputs.clear();
        }
        "Fn" => {
            inputs = vec![NodeInput {
                port: "text".to_string(),
                ty: PortType::Text,
                value: NodeInputValue::Literal(Literal::Null),
            }];
            outputs = vec![Port {
                name: "text".to_string(),
                ty: PortType::Text,
            }];
        }
        _ => {}
    }
    Node {
        id,
        kind: catalog.kind.clone(),
        type_name: catalog.type_name.clone(),
        params,
        inputs,
        outputs,
        at: Some(at),
        size: None,
        flip: false,
        loc: Default::default(),
        fn_src: (catalog.type_name == "Fn").then(|| "|i| { {text: i.text} }".to_string()),
        face_src: None,
        on_fail: "fail".to_string(),
        label: None,
        domain: catalog.domain.clone(),
        doc: None,
    }
}

/// Add a node of the catalog type at `at`; returns the graph and the new id.
pub fn add_node(graph: &Graph, catalog: &NodeTypeCatalog, at: (f64, f64)) -> (Graph, String) {
    let id = fresh_node_id(graph, &catalog.type_name);
    let mut next = graph.clone();
    next.nodes.push(node_from_catalog(id.clone(), catalog, at));
    (next, id)
}

pub fn move_node(graph: &Graph, id: &str, at: (f64, f64)) -> Graph {
    let mut next = graph.clone();
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == id) {
        node.at = Some(at);
    }
    next
}

pub fn resize_node(graph: &Graph, id: &str, size: (f64, f64)) -> Graph {
    let mut next = graph.clone();
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == id) {
        node.size = Some(size);
    }
    next
}

pub fn flip_node(graph: &Graph, id: &str, flip: bool) -> Graph {
    let mut next = graph.clone();
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == id) {
        node.flip = flip;
    }
    next
}

/// Mounted face trees only need rebuilding when their declarations or port
/// shape changed. Position, size, values, docs and run state are refreshed
/// in place and must not tear down an open popup or editor.
pub fn needs_face_remount(old: &Graph, new: &Graph) -> bool {
    if old.flow_ui_src != new.flow_ui_src || old.nodes.len() != new.nodes.len() {
        return true;
    }
    old.nodes.iter().any(|old_node| {
        let Some(new_node) = new.nodes.iter().find(|node| node.id == old_node.id) else {
            return true;
        };
        old_node.type_name != new_node.type_name
            || old_node.face_src != new_node.face_src
            || old_node
                .inputs
                .iter()
                .map(|input| (&input.port, input.ty))
                .ne(new_node.inputs.iter().map(|input| (&input.port, input.ty)))
            || old_node
                .outputs
                .iter()
                .map(|output| (&output.name, output.ty))
                .ne(new_node.outputs.iter().map(|output| (&output.name, output.ty)))
    })
}

/// Remove a node; edges into and out of it go, dependents' inputs become
/// literal `nil`, and a tool projection that named it drops the name.
pub fn delete_node(graph: &Graph, id: &str) -> Graph {
    let mut next = graph.clone();
    next.nodes.retain(|node| node.id != id);
    next.edges
        .retain(|edge| edge.from_node != id && edge.to_node != id);
    for node in &mut next.nodes {
        for input in &mut node.inputs {
            if let NodeInputValue::Edge(edge) = &input.value {
                if edge.from_node == id {
                    input.value = NodeInputValue::Literal(Literal::Null);
                }
            }
        }
    }
    for tool in &mut next.tools {
        tool.inputs.retain(|name| name != id);
        tool.outputs.retain(|name| name != id);
        tool.nodes.retain(|name| name != id);
    }
    next
}

pub fn output_port_type(graph: &Graph, node_id: &str, port: &str) -> Option<PortType> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)?
        .outputs
        .iter()
        .find(|output| output.name == port)
        .map(|output| output.ty)
}

/// Would an edge `from → to` close a loop? True when `from` already
/// depends on `to` (or they are the same node).
#[cfg(test)]
pub fn would_cycle(graph: &Graph, from: &str, to: &str) -> bool {
    let index = GraphIndex::new(graph);
    let (Some(from), Some(to)) = (index.node(from), index.node(to)) else {
        return from == to;
    };
    index.ancestors(from).contains(&to)
}

/// Every input port an output can legally be wired to: same type, not the
/// same node, no cycle. `Fn` inputs are flexible (any type re-types the
/// port) and `Http.body`/`Http.headers` accept anything as well.
pub fn compatible_inputs(graph: &Graph, from_node: &str, from_port: &str) -> Vec<(String, String)> {
    let index = GraphIndex::new(graph);
    index
        .compatible_inputs(graph, from_node, from_port)
        .into_iter()
        .map(|(node, port)| {
            (
                graph.nodes[node].id.clone(),
                graph.nodes[node].inputs[port].port.clone(),
            )
        })
        .collect()
}

/// Connect an output to an input. Fails with a reason when the types do
/// not match or the edge would loop.
pub fn connect(
    graph: &Graph,
    from_node: &str,
    from_port: &str,
    to_node: &str,
    to_port: &str,
) -> Result<Graph, String> {
    let ty = output_port_type(graph, from_node, from_port)
        .ok_or_else(|| format!("{from_node} has no output {from_port}"))?;
    if !compatible_inputs(graph, from_node, from_port)
        .iter()
        .any(|(node, port)| node == to_node && port == to_port)
    {
        return Err(format!(
            "{to_node}.{to_port} does not accept {} from {from_node}.{from_port}",
            ty.as_str()
        ));
    }
    let mut next = graph.clone();
    let node = next
        .nodes
        .iter_mut()
        .find(|node| node.id == to_node)
        .ok_or_else(|| format!("no node {to_node}"))?;
    let input = node
        .inputs
        .iter_mut()
        .find(|input| input.port == to_port)
        .ok_or_else(|| format!("{to_node} has no input {to_port}"))?;
    input.value = NodeInputValue::Edge(EdgeRef {
        from_node: from_node.to_string(),
        from_port: from_port.to_string(),
    });
    if node.type_name == "Fn" || node.type_name == "Output" {
        input.ty = ty;
    }
    if node.type_name == "Output" {
        if let Some((_, value)) = node.params.iter_mut().find(|(name, _)| name == "type") {
            *value = Literal::Id(ty.as_str().to_string());
        }
    }
    next.edges
        .retain(|edge| !(edge.to_node == to_node && edge.to_port == to_port));
    next.edges.push(Edge {
        from_node: from_node.to_string(),
        from_port: from_port.to_string(),
        to_node: to_node.to_string(),
        to_port: to_port.to_string(),
    });
    Ok(next)
}

fn empty_literal(ty: PortType) -> Literal {
    match ty {
        PortType::Text => Literal::Str(String::new()),
        PortType::Json => Literal::Obj(Vec::new()),
        PortType::List => Literal::Arr(Vec::new()),
        PortType::Image
        | PortType::Audio
        | PortType::Video
        | PortType::Mesh
        | PortType::Bytes => Literal::Null,
    }
}

fn disconnected_default(type_name: &str, input: &NodeInput) -> Literal {
    match (type_name, input.port.as_str()) {
        ("Llm", "prompt") | ("Http", "url") | ("Image" | "Gen", "prompt") => {
            Literal::Str(String::new())
        }
        ("Http", "headers") => Literal::Obj(Vec::new()),
        ("Fn", _) => empty_literal(input.ty),
        _ => Literal::Null,
    }
}

/// Drop the edge into an input. Dynamic `Fn.in` fields and built-in inputs
/// regain their empty literal default; an already-literal input is untouched.
pub fn disconnect(graph: &Graph, to_node: &str, to_port: &str) -> Graph {
    let mut next = graph.clone();
    next.edges
        .retain(|edge| !(edge.to_node == to_node && edge.to_port == to_port));
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == to_node) {
        let type_name = node.type_name.clone();
        if let Some(input) = node.inputs.iter_mut().find(|input| input.port == to_port) {
            if matches!(input.value, NodeInputValue::Edge(_)) {
                input.value = NodeInputValue::Literal(disconnected_default(&type_name, input));
            }
        }
    }
    next
}

/// Set a param, or a literal input of the same name. `type` on an Input /
/// Output / Ask re-types its port so the graph stays consistent.
pub fn set_param(graph: &Graph, node_id: &str, key: &str, value: Literal) -> Graph {
    let mut next = graph.clone();
    let Some(node) = next.nodes.iter_mut().find(|node| node.id == node_id) else {
        return next;
    };
    let mut is_param = false;
    if let Some((_, slot)) = node.params.iter_mut().find(|(name, _)| name == key) {
        *slot = value.clone();
        is_param = true;
    }
    if let Some(input) = node.inputs.iter_mut().find(|input| input.port == key) {
        if !is_param || matches!(input.value, NodeInputValue::Literal(_)) {
            input.value = NodeInputValue::Literal(value.clone());
        }
    } else if !is_param {
        node.params.push((key.to_string(), value.clone()));
    }
    if key == "type" {
        let ty = match &value {
            Literal::Id(text) | Literal::Str(text) => port_type_from_name(text),
            _ => None,
        };
        if let Some(ty) = ty {
            match node.type_name.as_str() {
                "Input" | "Text" | "Ask" => {
                    node.outputs = vec![Port {
                        name: ty.as_str().to_string(),
                        ty,
                    }];
                    let id = node.id.clone();
                    next.edges.retain(|edge| edge.from_node != id);
                    for other in &mut next.nodes {
                        for input in &mut other.inputs {
                            if matches!(&input.value, NodeInputValue::Edge(edge) if edge.from_node == id)
                            {
                                input.value = NodeInputValue::Literal(Literal::Null);
                            }
                        }
                    }
                }
                "Output" => {
                    if let Some(input) = node.inputs.first_mut() {
                        input.ty = ty;
                    }
                }
                _ => {}
            }
        }
    }
    next
}

pub fn set_fn_src(graph: &Graph, node_id: &str, src: &str) -> Graph {
    let mut next = graph.clone();
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == node_id) {
        node.fn_src = Some(src.trim().to_string());
    }
    next
}

pub fn set_face_src(graph: &Graph, node_id: &str, src: &str) -> Graph {
    let mut next = graph.clone();
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == node_id) {
        let src = src.trim();
        node.face_src = (!src.is_empty() && src != "nil").then(|| src.to_string());
    }
    next
}

/// Dependency depth of every node: sources are 0, a node is one past its
/// deepest input.
pub fn depths(graph: &Graph) -> HashMap<String, usize> {
    let mut depth: HashMap<String, usize> = HashMap::new();
    let mut upstream: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        upstream
            .entry(edge.to_node.as_str())
            .or_default()
            .push(edge.from_node.as_str());
    }
    fn walk<'a>(
        id: &'a str,
        upstream: &HashMap<&'a str, Vec<&'a str>>,
        depth: &mut HashMap<String, usize>,
        guard: &mut HashSet<&'a str>,
    ) -> usize {
        if let Some(value) = depth.get(id) {
            return *value;
        }
        if !guard.insert(id) {
            return 0;
        }
        let value = upstream
            .get(id)
            .map(|sources| {
                sources
                    .iter()
                    .map(|source| walk(source, upstream, depth, guard) + 1)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        guard.remove(id);
        depth.insert(id.to_string(), value);
        value
    }
    for node in &graph.nodes {
        let mut guard = HashSet::new();
        walk(&node.id, &upstream, &mut depth, &mut guard);
    }
    depth
}

/// Give every node without an `at` a place. A node with wired inputs sits
/// one column to the right of its right-most upstream node, on its row,
/// using the column pitch the placed nodes already use (else the default);
/// a source sits in the first column, below anything already there. Nodes
/// are placed in dependency order, so every upstream node has a place first.
pub fn auto_place(graph: &mut Graph) {
    let depth = depths(graph);
    let mut xs: Vec<f64> = graph.nodes.iter().filter_map(|node| node.at.map(|at| at.0)).collect();
    xs.sort_by(|a, b| a.total_cmp(b));
    xs.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    let mut deltas: Vec<f64> = xs
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|delta| *delta >= NODE_WIDTH)
        .collect();
    deltas.sort_by(|a, b| a.total_cmp(b));
    let pitch = deltas
        .get(deltas.len() / 2)
        .copied()
        .unwrap_or(NODE_WIDTH + COLUMN_GAP);
    let node_index: HashMap<String, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), index))
        .collect();
    let mut upstream = vec![Vec::new(); graph.nodes.len()];
    for edge in &graph.edges {
        if let (Some(from), Some(to)) = (
            node_index.get(&edge.from_node).copied(),
            node_index.get(&edge.to_node).copied(),
        ) {
            upstream[to].push(from);
        }
    }
    let mut order: Vec<usize> = (0..graph.nodes.len()).collect();
    order.sort_by_key(|index| depth.get(&graph.nodes[*index].id).copied().unwrap_or(0));
    let mut first_column_bottom = graph
        .nodes
        .iter()
        .filter_map(|node| node.at)
        .filter(|(x, _)| (*x - FIRST_AT.0).abs() < NODE_WIDTH)
        .map(|(_, y)| y)
        .fold(FIRST_AT.1 - ROW_GAP, f64::max);
    let cell_height = ROW_GAP * 0.5;
    let mut occupied: HashMap<(i64, i64), Vec<(f64, f64)>> = HashMap::new();
    let cell = |at: (f64, f64)| {
        (
            (at.0 / NODE_WIDTH).floor() as i64,
            (at.1 / cell_height).floor() as i64,
        )
    };
    for at in graph.nodes.iter().filter_map(|node| node.at) {
        occupied.entry(cell(at)).or_default().push(at);
    }
    for index in order {
        if graph.nodes[index].at.is_some() {
            continue;
        }
        let anchor = upstream
            .get(index)
            .into_iter()
            .flatten()
            .filter_map(|from| graph.nodes[*from].at)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let mut at = match anchor {
            Some((x, y)) => (x + pitch, y),
            None => {
                first_column_bottom += ROW_GAP;
                (FIRST_AT.0, first_column_bottom)
            }
        };
        // Never on top of a placed node: step down a row until clear.
        while {
            let (cell_x, cell_y) = cell(at);
            (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    occupied
                        .get(&(cell_x + dx, cell_y + dy))
                        .is_some_and(|points| {
                            points.iter().any(|(x, y)| {
                                (*x - at.0).abs() < NODE_WIDTH
                                    && (*y - at.1).abs() < cell_height
                            })
                        })
                })
            })
        } {
            at.1 += ROW_GAP;
        }
        graph.nodes[index].at = Some(at);
        occupied.entry(cell(at)).or_default().push(at);
    }
}

/// Catalog types with at least one input that accepts `ty`.
pub fn types_with_compatible_input<'a>(
    catalog: &'a [NodeTypeCatalog],
    ty: PortType,
) -> Vec<&'a NodeTypeCatalog> {
    catalog
        .iter()
        .filter(|entry| {
            entry.type_name == "Fn"
                || entry.type_name == "Output"
                || entry.ports._in.iter().any(|port| port.ty == ty)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_flow::{NodeParamCatalog, NodePortsCatalog};

    fn catalog(type_name: &str, kind: &str, ins: &[(&str, PortType)], outs: &[(&str, PortType)]) -> NodeTypeCatalog {
        NodeTypeCatalog {
            type_name: type_name.to_string(),
            kind: kind.to_string(),
            domain: None,
            models: Vec::new(),
            ports: NodePortsCatalog {
                _in: ins
                    .iter()
                    .map(|(name, ty)| Port { name: name.to_string(), ty: *ty })
                    .collect(),
                out: outs
                    .iter()
                    .map(|(name, ty)| Port { name: name.to_string(), ty: *ty })
                    .collect(),
            },
            params: vec![
                NodeParamCatalog {
                    name: "type".to_string(),
                    default: JsonValue::String("text".to_string()),
                    doc: String::new(),
                    range: None,
                },
                NodeParamCatalog {
                    name: "steps".to_string(),
                    default: JsonValue::F64(8.0),
                    doc: "1..50".to_string(),
                    range: None,
                },
            ],
            face: format!("{type_name}Face"),
            doc: String::new(),
        }
    }

    fn empty() -> Graph {
        Graph {
            revision: 1,
            label: "t".into(),
            brief: String::new(),
            trigger: "manual".into(),
            concurrency: 1,
            autostart: false,
            nodes: Vec::new(),
            edges: Vec::new(),
            tools: Vec::new(),
            flow_ui_src: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn add_node_gets_a_fresh_id_and_prelude_defaults() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let (g, id) = add_node(&empty(), &input, (10.0, 20.0));
        assert_eq!(id, "input_1");
        let (g, id2) = add_node(&g, &input, (10.0, 20.0));
        assert_eq!(id2, "input_2");
        let node = &g.nodes[0];
        assert_eq!(node.at, Some((10.0, 20.0)));
        assert_eq!(node.outputs[0].name, "text");
        assert!(matches!(
            node.params.iter().find(|(n, _)| n == "type").unwrap().1,
            Literal::Id(ref t) if t == "text"
        ));
        assert!(matches!(
            node.params.iter().find(|(n, _)| n == "steps").unwrap().1,
            Literal::Num(v) if v == 8.0
        ));
    }

    #[test]
    fn face_remount_ignores_layout_but_catches_structure() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let (graph, id) = add_node(&empty(), &input, (10.0, 20.0));
        let moved = resize_node(&move_node(&graph, &id, (31.5, 47.25)), &id, (440.0, 180.0));
        assert!(!needs_face_remount(&graph, &moved));

        let mut changed_face = graph.clone();
        changed_face.nodes[0].face_src = Some("View{}".into());
        assert!(needs_face_remount(&graph, &changed_face));

        let mut changed_ports = graph.clone();
        changed_ports.nodes[0].outputs[0].ty = PortType::Image;
        assert!(needs_face_remount(&graph, &changed_ports));
    }

    #[test]
    fn connect_checks_types_and_writes_the_edge() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let image = catalog(
            "Image",
            "gen",
            &[("prompt", PortType::Text), ("image", PortType::Image)],
            &[("image", PortType::Image)],
        );
        let (g, a) = add_node(&empty(), &input, (0.0, 0.0));
        let (g, b) = add_node(&g, &image, (0.0, 0.0));
        let compatible = compatible_inputs(&g, &a, "text");
        assert_eq!(compatible, vec![(b.clone(), "prompt".to_string())]);
        assert!(connect(&g, &a, "text", &b, "image").is_err());
        let g = connect(&g, &a, "text", &b, "prompt").unwrap();
        assert_eq!(g.edges.len(), 1);
        let prompt = g.nodes[1].inputs.iter().find(|i| i.port == "prompt").unwrap();
        assert!(matches!(&prompt.value, NodeInputValue::Edge(e) if e.from_node == a && e.from_port == "text"));
        assert!(would_cycle(&g, &b, &a));
        let g = disconnect(&g, &b, "prompt");
        assert!(g.edges.is_empty());
        assert!(matches!(
            g.nodes[1].inputs.iter().find(|i| i.port == "prompt").unwrap().value,
            NodeInputValue::Literal(Literal::Str(ref value)) if value.is_empty()
        ));
    }

    #[test]
    fn disconnect_restores_llm_prompt_default() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let llm = catalog("Llm", "chat", &[("prompt", PortType::Text)], &[("text", PortType::Text)]);
        let (graph, source) = add_node(&empty(), &input, (0.0, 0.0));
        let (graph, target) = add_node(&graph, &llm, (300.0, 0.0));
        let graph = connect(&graph, &source, "text", &target, "prompt").unwrap();
        let graph = disconnect(&graph, &target, "prompt");
        assert!(matches!(
            graph.nodes[1].inputs[0].value,
            NodeInputValue::Literal(Literal::Str(ref value)) if value.is_empty()
        ));
    }

    #[test]
    fn disconnect_restores_fn_text_input_and_preserves_a_literal() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let function = catalog("Fn", "fn", &[("text", PortType::Text)], &[("text", PortType::Text)]);
        let (graph, source) = add_node(&empty(), &input, (0.0, 0.0));
        let (graph, target) = add_node(&graph, &function, (300.0, 0.0));
        let graph = connect(&graph, &source, "text", &target, "text").unwrap();
        let graph = disconnect(&graph, &target, "text");
        assert!(matches!(
            graph.nodes[1].inputs[0].value,
            NodeInputValue::Literal(Literal::Str(ref value)) if value.is_empty()
        ));

        let mut literal_graph = graph.clone();
        literal_graph.nodes[1].inputs[0].value =
            NodeInputValue::Literal(Literal::Str("kept".into()));
        let literal_graph = disconnect(&literal_graph, &target, "text");
        assert!(matches!(
            literal_graph.nodes[1].inputs[0].value,
            NodeInputValue::Literal(Literal::Str(ref value)) if value == "kept"
        ));
    }

    #[test]
    fn output_adopts_the_type_it_is_wired_to() {
        let image = catalog("Image", "gen", &[("prompt", PortType::Text)], &[("image", PortType::Image)]);
        let output = catalog("Output", "output", &[], &[]);
        let (g, a) = add_node(&empty(), &image, (0.0, 0.0));
        let (g, b) = add_node(&g, &output, (0.0, 0.0));
        let g = connect(&g, &a, "image", &b, "value").unwrap();
        let out = &g.nodes[1];
        assert_eq!(out.inputs[0].ty, PortType::Image);
        assert!(matches!(
            out.params.iter().find(|(n, _)| n == "type").unwrap().1,
            Literal::Id(ref t) if t == "image"
        ));
    }

    #[test]
    fn move_and_delete() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let llm = catalog("Llm", "chat", &[("prompt", PortType::Text)], &[("text", PortType::Text)]);
        let (g, a) = add_node(&empty(), &input, (0.0, 0.0));
        let (g, b) = add_node(&g, &llm, (0.0, 0.0));
        let g = connect(&g, &a, "text", &b, "prompt").unwrap();
        let g = move_node(&g, &b, (300.4, 99.6));
        assert_eq!(g.nodes[1].at, Some((300.4, 99.6)));
        let g = delete_node(&g, &a);
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty());
        assert!(matches!(g.nodes[0].inputs[0].value, NodeInputValue::Literal(Literal::Null)));
    }

    #[test]
    fn set_param_retypes_an_input_port() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let (g, a) = add_node(&empty(), &input, (0.0, 0.0));
        let g = set_param(&g, &a, "type", Literal::Id("image".into()));
        assert_eq!(g.nodes[0].outputs[0].name, "image");
        assert_eq!(g.nodes[0].outputs[0].ty, PortType::Image);
        let g = set_param(&g, &a, "default", Literal::Str("x".into()));
        assert!(matches!(
            g.nodes[0].params.iter().find(|(n, _)| n == "default").unwrap().1,
            Literal::Str(ref t) if t == "x"
        ));
    }

    #[test]
    fn auto_place_walks_left_to_right_by_dependency() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let llm = catalog("Llm", "chat", &[("prompt", PortType::Text)], &[("text", PortType::Text)]);
        let (g, a) = add_node(&empty(), &input, (0.0, 0.0));
        let (g, b) = add_node(&g, &llm, (0.0, 0.0));
        let (g, c) = add_node(&g, &llm, (0.0, 0.0));
        let mut g = connect(&g, &a, "text", &b, "prompt").unwrap();
        g = connect(&g, &b, "text", &c, "prompt").unwrap();
        for node in &mut g.nodes {
            node.at = None;
        }
        auto_place(&mut g);
        let at = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap().at.unwrap();
        assert_eq!(at(&a), FIRST_AT);
        assert_eq!(at(&b).0, FIRST_AT.0 + NODE_WIDTH + COLUMN_GAP);
        assert_eq!(at(&c).0, FIRST_AT.0 + 2.0 * (NODE_WIDTH + COLUMN_GAP));
        assert_eq!(at(&b).1, FIRST_AT.1);
    }

    #[test]
    fn compatible_types_filter() {
        let cats = vec![
            catalog("Input", "input", &[], &[("text", PortType::Text)]),
            catalog("Image", "gen", &[("prompt", PortType::Text)], &[("image", PortType::Image)]),
            catalog("Upscale", "gen", &[("image", PortType::Image)], &[("image", PortType::Image)]),
        ];
        let names: Vec<_> = types_with_compatible_input(&cats, PortType::Image)
            .iter()
            .map(|c| c.type_name.as_str())
            .collect();
        assert_eq!(names, vec!["Upscale"]);
    }

    #[test]
    fn graph_index_storage_and_compatibility_scan_are_linear_in_graph_size() {
        let input = catalog("Input", "input", &[], &[("text", PortType::Text)]);
        let llm = catalog(
            "Llm",
            "chat",
            &[("prompt", PortType::Text)],
            &[("text", PortType::Text)],
        );
        let (seed, _) = add_node(&empty(), &input, (0.0, 0.0));
        let (template, _) = add_node(&empty(), &llm, (0.0, 0.0));
        let mut graph = empty();
        let mut source = seed.nodes[0].clone();
        source.id = "n0".into();
        graph.nodes.push(source);
        for index in 1..512 {
            let mut node = template.nodes[0].clone();
            node.id = format!("n{index}");
            node.inputs[0].value = NodeInputValue::Edge(EdgeRef {
                from_node: format!("n{}", index - 1),
                from_port: "text".into(),
            });
            graph.edges.push(Edge {
                from_node: format!("n{}", index - 1),
                from_port: "text".into(),
                to_node: node.id.clone(),
                to_port: "prompt".into(),
            });
            graph.nodes.push(node);
        }
        let index = GraphIndex::new(&graph);
        assert_eq!(index.nodes.len(), graph.nodes.len());
        assert_eq!(index.upstream.iter().map(Vec::len).sum::<usize>(), graph.edges.len());
        assert_eq!(
            index.compatible_inputs(&graph, "n0", "text").len(),
            graph.nodes.len() - 1
        );
    }
}
