//! Surface bindings: what a control of a bound Splash surface addresses in
//! the flow it is bound to.
//!
//! A surface names its targets the way node faces do (`bind := "name.text"`
//! for an input port, `show := "greeting.value"` for an output). Resolving
//! them against the flow's [`FlowInterface`] gives each binding a type and
//! says whether its target still exists and whether the control can carry
//! that type. A binding whose target is gone or incompatible is **unbound**,
//! kept and shown as such with its reason; it is never dropped, so a graph
//! edit that brings the target back binds it again.

use super::interface::FlowInterface;
use crate::PortType;

/// What kind of control declared a binding; decides which port types it
/// can write or show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// A text field: writes text or JSON.
    Text,
    /// A slider or numeric field: writes a number as text.
    Number,
    /// A check box: writes `true`/`false` as text.
    Bool,
    /// A drop-down or combo box: writes one of its labels.
    Choice,
    /// A button: emits a pulse to a trigger endpoint, never a value.
    Action,
    /// A readout: shows a value of any type.
    Readout,
}

impl ControlKind {
    /// Whether a control of this kind can carry a port of type `ty`.
    pub fn accepts(self, ty: &PortType) -> bool {
        match self {
            Self::Readout => true,
            Self::Action => false,
            Self::Text => matches!(ty, PortType::Text | PortType::Json),
            Self::Number | Self::Bool | Self::Choice => matches!(ty, PortType::Text),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Bool => "bool",
            Self::Choice => "choice",
            Self::Action => "action",
            Self::Readout => "readout",
        }
    }
}

/// What kind of live endpoint a control addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointBinding {
    /// A number inside the endpoint's range; the host clamps it.
    Control,
    Toggle,
    /// Write-only, edge-triggered action with no numeric value.
    Trigger,
    /// Read-only: the newest value an owner published.
    Probe,
}

/// An endpoint the running live plan of the instance admits, as a surface
/// may address it: by qualified node and name, like a port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredEndpoint {
    pub node: String,
    pub name: String,
    pub class: EndpointBinding,
}

/// What one surface control addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingTarget {
    /// An endpoint of the instance's running live plan: a write goes to the
    /// live host, not into a run; a readout follows a probe or a control.
    Endpoint { node: String, name: String, class: EndpointBinding },
    /// An `Input` (or `Ask`) node's port: the control writes the instance's
    /// input.
    Input { node: String, port: String, ty: PortType, ask: bool },
    /// An `Output` node's value port: the control shows the instance's live
    /// value.
    Output { node: String, port: String, ty: PortType },
}

/// One resolved (or unresolved) surface binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// The `node.port` text the surface wrote.
    pub node: String,
    pub port: String,
    /// Whether the control writes (`bind`) or shows (`show`).
    pub writes: bool,
    pub control: ControlKind,
    /// `None` while the flow has no such target or the control cannot carry
    /// its type; `reason` then says which.
    pub target: Option<BindingTarget>,
    pub reason: Option<String>,
}

impl Binding {
    pub fn bound(&self) -> bool {
        self.target.is_some()
    }
}

/// Resolve `node.port` for a control against the interface, exactly: a
/// writing control needs an input with that node and port; a showing
/// control takes the output with that node and port first, then such an
/// input (a field may echo what it wrote). The port type must be one the
/// control can carry.
pub fn resolve(
    interface: &FlowInterface,
    node: &str,
    port: &str,
    writes: bool,
    control: ControlKind,
) -> Result<BindingTarget, String> {
    let input = interface.inputs.iter().find(|input| input.node == node && input.port == port);
    let output = interface.outputs.iter().find(|output| output.node == node && output.port == port);
    let (target, ty) = if writes {
        match input {
            Some(input) => (
                BindingTarget::Input { node: input.node.clone(), port: input.port.clone(), ty: input.ty.clone(), ask: input.ask },
                input.ty.clone(),
            ),
            None => {
                return Err(match interface.inputs.iter().find(|input| input.node == node) {
                    Some(input) => format!("input `{node}` has port `{}`, not `{port}`", input.port),
                    None => format!("no input node `{node}`"),
                })
            }
        }
    } else {
        match (output, input) {
            (Some(output), _) => (
                BindingTarget::Output { node: output.node.clone(), port: output.port.clone(), ty: output.ty.clone() },
                output.ty.clone(),
            ),
            (None, Some(input)) => (
                BindingTarget::Input { node: input.node.clone(), port: input.port.clone(), ty: input.ty.clone(), ask: input.ask },
                input.ty.clone(),
            ),
            (None, None) => {
                return Err(match interface.outputs.iter().find(|output| output.node == node) {
                    Some(output) => format!("output `{node}` has port `{}`, not `{port}`", output.port),
                    None => format!("no output or input node `{node}`"),
                })
            }
        }
    };
    if !control.accepts(&ty) {
        return Err(format!("a {} control cannot carry the {ty:?} port `{node}.{port}`", control.name()));
    }
    Ok(target)
}

/// Resolve against the running live plan's endpoints when the interface has
/// no such port. The interface's own error is kept when neither has it.
fn resolve_endpoint(
    endpoints: &[DeclaredEndpoint],
    node: &str,
    port: &str,
    writes: bool,
    control: ControlKind,
) -> Option<Result<BindingTarget, String>> {
    let endpoint = endpoints.iter().find(|endpoint| endpoint.node == node && endpoint.name == port)?;
    let fits = match (endpoint.class, writes) {
        (EndpointBinding::Trigger, false) => return Some(Err(format!("`{node}.{port}` is a trigger; it accepts pulses and has no value to show"))),
        (EndpointBinding::Trigger, true) => control == ControlKind::Action,
        (EndpointBinding::Probe, true) => return Some(Err(format!("`{node}.{port}` is a probe; it can be shown, not written"))),
        (EndpointBinding::Control, true) => control == ControlKind::Number,
        (EndpointBinding::Toggle, true) => matches!(control, ControlKind::Bool | ControlKind::Number),
        (_, false) => control != ControlKind::Action,
    };
    Some(if fits {
        Ok(BindingTarget::Endpoint { node: node.to_string(), name: port.to_string(), class: endpoint.class })
    } else {
        Err(format!("a {} control cannot {} the live endpoint `{node}.{port}`", control.name(), if writes { "write" } else { "show" }))
    })
}

/// [`resolve_all`] for an instance that also runs a live plan.
pub fn resolve_all_with(
    interface: &FlowInterface,
    declared: &[(String, String, bool, ControlKind)],
    endpoints: &[DeclaredEndpoint],
) -> Vec<Binding> {
    declared
        .iter()
        .map(|(node, port, writes, control)| {
            let resolved = match resolve(interface, node, port, *writes, *control) {
                Ok(target) => Ok(target),
                Err(reason) => resolve_endpoint(endpoints, node, port, *writes, *control).unwrap_or(Err(reason)),
            };
            let (target, reason) = match resolved {
                Ok(target) => (Some(target), None),
                Err(reason) => (None, Some(reason)),
            };
            Binding { node: node.clone(), port: port.clone(), writes: *writes, control: *control, target, reason }
        })
        .collect()
}

/// Resolve every `(node, port, writes, control)` a surface declared.
pub fn resolve_all(interface: &FlowInterface, declared: &[(String, String, bool, ControlKind)]) -> Vec<Binding> {
    declared
        .iter()
        .map(|(node, port, writes, control)| {
            let (target, reason) = match resolve(interface, node, port, *writes, *control) {
                Ok(target) => (Some(target), None),
                Err(reason) => (None, Some(reason)),
            };
            Binding { node: node.clone(), port: port.clone(), writes: *writes, control: *control, target, reason }
        })
        .collect()
}
