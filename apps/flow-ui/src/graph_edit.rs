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
        node.at = Some((at.0.round(), at.1.round()));
    }
    next
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
pub fn would_cycle(graph: &Graph, from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let mut upstream: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        upstream
            .entry(edge.to_node.as_str())
            .or_default()
            .push(edge.from_node.as_str());
    }
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        if node == to {
            return true;
        }
        if let Some(sources) = upstream.get(node) {
            stack.extend(sources.iter().copied());
        }
    }
    false
}

/// Every input port an output can legally be wired to: same type, not the
/// same node, no cycle. `Fn` inputs are flexible (any type re-types the
/// port) and `Http.body`/`Http.headers` accept anything as well.
pub fn compatible_inputs(graph: &Graph, from_node: &str, from_port: &str) -> Vec<(String, String)> {
    let Some(ty) = output_port_type(graph, from_node, from_port) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in &graph.nodes {
        if node.id == from_node || would_cycle(graph, from_node, &node.id) {
            continue;
        }
        for input in &node.inputs {
            let flexible = node.type_name == "Fn"
                || (node.type_name == "Http" && (input.port == "body" || input.port == "headers"))
                || (node.type_name == "Output" && input.port == "value");
            if flexible || input.ty == ty {
                out.push((node.id.clone(), input.port.clone()));
            }
        }
    }
    out
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

/// Drop the edge into an input; the input becomes literal `nil`.
pub fn disconnect(graph: &Graph, to_node: &str, to_port: &str) -> Graph {
    let mut next = graph.clone();
    next.edges
        .retain(|edge| !(edge.to_node == to_node && edge.to_port == to_port));
    if let Some(node) = next.nodes.iter_mut().find(|node| node.id == to_node) {
        if let Some(input) = node.inputs.iter_mut().find(|input| input.port == to_port) {
            input.value = NodeInputValue::Literal(Literal::Null);
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
    let mut upstream: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &graph.edges {
        upstream
            .entry(edge.to_node.clone())
            .or_default()
            .push(edge.from_node.clone());
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
    for index in order {
        if graph.nodes[index].at.is_some() {
            continue;
        }
        let id = graph.nodes[index].id.clone();
        let anchor = upstream
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|from| graph.nodes.iter().find(|node| node.id == *from))
            .filter_map(|node| node.at)
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let mut at = match anchor {
            Some((x, y)) => (x + pitch, y),
            None => {
                first_column_bottom += ROW_GAP;
                (FIRST_AT.0, first_column_bottom)
            }
        };
        // Never on top of a placed node: step down a row until clear.
        while graph.nodes.iter().any(|node| {
            node.at.is_some_and(|(x, y)| (x - at.0).abs() < NODE_WIDTH && (y - at.1).abs() < ROW_GAP * 0.5)
        }) {
            at.1 += ROW_GAP;
        }
        graph.nodes[index].at = Some(at);
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
            NodeInputValue::Literal(Literal::Null)
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
        assert_eq!(g.nodes[1].at, Some((300.0, 100.0)));
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
}
