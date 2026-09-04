//! Reusable graph-canvas widget and its display-only view model.

pub mod canvas;
pub mod model;
pub mod wire_route;

pub use canvas::{
    Camera, CanvasEdit, FlowCanvas, FlowCanvasAction, NodeStatus, Selection, LOCAL_ORIGIN,
};
pub use model::{
    CompatiblePorts, EdgeView, GraphIndex, GraphView, NodeFaces, NodeFacesScope, NodeStyle,
    NodeView, PortStyle, PortView, FIRST_AT, NODE_WIDTH,
};

use makepad_widgets::ScriptVm;

/// Register the flow graph widget and its DSL-facing style records.
pub fn script_mod(vm: &mut ScriptVm) {
    canvas::script_mod(vm);
}
