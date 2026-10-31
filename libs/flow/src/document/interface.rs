//! A flow's external interface: what a host may set and what it gets back.
//!
//! Derived from the graph, so a host never has to evaluate a source itself
//! to learn which port an Input node exposes or what type an Output returns.

use crate::{Graph, PortType};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceInput {
    pub node: String,
    /// The port a caller sets on this Input (or answers on this Ask).
    pub port: String,
    pub ty: PortType,
    /// The bound of an ordered input; absent for one value.
    pub many: Option<u32>,
    /// An Ask node: it takes its value while a run waits on it.
    pub ask: bool,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceOutput {
    pub node: String,
    /// The port the Output node takes its value on, the one a reader names.
    pub port: String,
    pub ty: PortType,
    /// The bound of an ordered result; absent for one value.
    pub many: Option<u32>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlowInterface {
    pub label: Option<String>,
    pub inputs: Vec<InterfaceInput>,
    pub outputs: Vec<InterfaceOutput>,
}

impl FlowInterface {
    pub fn of(graph: &Graph) -> Self {
        let mut interface = Self {
            label: (!graph.label.is_empty()).then(|| graph.label.clone()),
            ..Self::default()
        };
        for node in &graph.nodes {
            match node.kind.as_str() {
                "input" | "ask" => {
                    if let Some(port) = node.outputs.first() {
                        interface.inputs.push(InterfaceInput {
                            node: node.id.clone(),
                            port: port.name.clone(),
                            ty: port.ty.clone(),
                            many: port.many,
                            ask: node.kind == "ask",
                            label: node.label.clone(),
                        });
                    }
                }
                "output" => {
                    if let Some(input) = node.inputs.iter().find(|input| input.port == "value") {
                        interface.outputs.push(InterfaceOutput {
                            node: node.id.clone(),
                            port: input.port.clone(),
                            ty: input.ty.clone(),
                            many: input.many,
                            label: node.label.clone(),
                        });
                    }
                }
                // A container's output ports are readouts under their
                // boundary node ids (`a/greeting`, port `value`), the same
                // identity a run publishes them under.
                "component" => {
                    let Some(def) = graph.components.iter().find(|def| def.alias == node.type_name) else {
                        continue;
                    };
                    for port in &def.outputs {
                        interface.outputs.push(InterfaceOutput {
                            node: format!("{}/{}", node.id, port.name),
                            port: "value".to_string(),
                            ty: port.ty.clone(),
                            many: port.many,
                            label: None,
                        });
                    }
                }
                _ => {}
            }
        }
        interface
    }

    pub fn input(&self, node: &str) -> Option<&InterfaceInput> {
        self.inputs.iter().find(|input| input.node == node)
    }
}
