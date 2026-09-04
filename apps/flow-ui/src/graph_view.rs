//! Thin projection from the editable flow graph into the reusable canvas model.

use crate::graph_edit;
use makepad_flow::{Graph, Literal, Node, NodeInputValue, PortType};
use makepad_flowgraph::{CompatiblePorts, EdgeView, GraphView, NodeView, PortView};
use std::collections::{HashMap, HashSet};

pub(crate) fn declared_output_type(node: &Node) -> Option<PortType> {
    if node.type_name != "Output" {
        return None;
    }
    node.params
        .iter()
        .find_map(|(name, value)| {
            if name != "type" {
                return None;
            }
            match value {
                Literal::Id(name) | Literal::Str(name) => PortType::from_str(name),
                _ => None,
            }
        })
        .or_else(|| node.inputs.first().map(|input| input.ty))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortIcon {
    Text,
    Image,
    Audio,
    Video,
    Mesh,
    Json,
    Bytes,
}

impl PortIcon {
    pub(crate) fn for_type(ty: PortType) -> Self {
        match ty {
            PortType::Text => Self::Text,
            PortType::Image => Self::Image,
            PortType::Audio => Self::Audio,
            PortType::Video => Self::Video,
            PortType::Mesh => Self::Mesh,
            PortType::Json | PortType::List => Self::Json,
            PortType::Bytes => Self::Bytes,
        }
    }
}

fn full_bleed(node: &Node) -> bool {
    match node.kind.as_str() {
        "output" => declared_output_type(node)
            .or_else(|| node.inputs.first().map(|input| input.ty))
            == Some(PortType::Image),
        "input" | "gen" => node
            .outputs
            .first()
            .is_some_and(|port| port.ty == PortType::Image),
        _ => false,
    }
}

/// Project the host graph, including its app-owned automatic placement, into
/// the string-keyed data consumed by `FlowCanvas`.
pub fn view_of(graph: &Graph) -> GraphView {
    let mut graph = graph.clone();
    graph_edit::auto_place(&mut graph);
    let connected_outputs: HashSet<(&str, &str)> = graph
        .edges
        .iter()
        .map(|edge| (edge.from_node.as_str(), edge.from_port.as_str()))
        .collect();
    GraphView {
        nodes: graph
            .nodes
            .iter()
            .map(|node| NodeView {
                id: node.id.clone(),
                title: node.id.clone(),
                type_name: node.type_name.clone(),
                kind: node.kind.clone(),
                at: node.at.unwrap_or(makepad_flowgraph::FIRST_AT),
                size: node.size,
                flip: node.flip,
                inputs: node
                    .inputs
                    .iter()
                    .map(|input| PortView {
                        name: input.port.clone(),
                        kind: input.ty.as_str().to_string(),
                        connected: matches!(input.value, NodeInputValue::Edge(_)),
                    })
                    .collect(),
                outputs: node
                    .outputs
                    .iter()
                    .map(|output| PortView {
                        name: output.name.clone(),
                        kind: output.ty.as_str().to_string(),
                        connected: connected_outputs
                            .contains(&(node.id.as_str(), output.name.as_str())),
                    })
                    .collect(),
                full_bleed: full_bleed(node),
                params: node
                    .params
                    .iter()
                    .filter_map(|(name, value)| match value {
                        Literal::Id(value) | Literal::Str(value) => {
                            Some((name.clone(), value.clone()))
                        }
                        _ => None,
                    })
                    .collect(),
            })
            .collect(),
        edges: graph
            .edges
            .iter()
            .map(|edge| EdgeView {
                from: edge.from_node.clone(),
                from_port: edge.from_port.clone(),
                to: edge.to_node.clone(),
                to_port: edge.to_port.clone(),
            })
            .collect(),
    }
}

/// Precompute host type/cycle policy for every output. The canvas only turns
/// the selected output's string targets into node/port indices during a drag.
pub fn compatibility_of(graph: &Graph) -> CompatiblePorts {
    let mut compatible = HashMap::new();
    for node in &graph.nodes {
        for output in &node.outputs {
            compatible.insert(
                (node.id.clone(), output.name.clone()),
                graph_edit::compatible_inputs(graph, &node.id, &output.name)
                    .into_iter()
                    .collect(),
            );
        }
    }
    compatible
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_flow::{Edge, EdgeRef, Loc, NodeInput, Port};

    fn node(id: &str, kind: &str, ty: PortType) -> Node {
        Node {
            id: id.into(),
            kind: kind.into(),
            type_name: if kind == "output" { "Output" } else { "Input" }.into(),
            params: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![Port {
                name: ty.as_str().into(),
                ty,
            }],
            at: Some((10.0, 20.0)),
            size: None,
            flip: false,
            loc: Loc::default(),
            fn_src: None,
            face_src: None,
            on_fail: "fail".into(),
            label: None,
            domain: None,
            doc: None,
        }
    }

    #[test]
    fn projects_ports_edges_layout_and_full_bleed() {
        let mut source = node("picture", "input", PortType::Image);
        source.size = Some((420.0, 260.0));
        source.flip = true;
        let mut sink = node("result", "output", PortType::Text);
        sink.outputs.clear();
        sink.params = vec![("type".into(), Literal::Id("image".into()))];
        sink.inputs.push(NodeInput {
            port: "value".into(),
            ty: PortType::Image,
            value: NodeInputValue::Edge(EdgeRef {
                from_node: "picture".into(),
                from_port: "image".into(),
            }),
        });
        let graph = Graph {
            revision: 1,
            label: "test".into(),
            brief: String::new(),
            trigger: "manual".into(),
            concurrency: 1,
            autostart: false,
            nodes: vec![source, sink],
            edges: vec![Edge {
                from_node: "picture".into(),
                from_port: "image".into(),
                to_node: "result".into(),
                to_port: "value".into(),
            }],
            tools: Vec::new(),
            flow_ui_src: None,
            warnings: Vec::new(),
        };

        let view = view_of(&graph);
        assert_eq!(view.edges[0].from_port, "image");
        assert_eq!(view.nodes[0].at, (10.0, 20.0));
        assert_eq!(view.nodes[0].size, Some((420.0, 260.0)));
        assert!(view.nodes[0].flip);
        assert!(view.nodes[0].full_bleed);
        assert!(view.nodes[0].outputs[0].connected);
        assert!(view.nodes[1].inputs[0].connected);
        assert!(view.nodes[1].full_bleed);
    }
}
