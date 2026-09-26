//! Reusable graph-canvas widget and its display-only view model.

pub mod canvas;
pub mod model;
pub mod wire_route;

pub use canvas::{
    Camera, CanvasEdit, CanvasEventPolicy, CanvasNodeRegion, CanvasPick, CanvasViewport,
    BoundaryLink, CanvasViewportError, EmbeddedCanvasRoot, FlowCanvas, FlowCanvasAction, NodeGeometry,
    NodeStatus, Selection, CANVAS_SCALE_MAX, CANVAS_SCALE_MIN, LOCAL_ORIGIN,
};
pub use model::{
    CompatiblePorts, EdgeView, FacePort, FaceViewport, GraphIndex, GraphView, NodeFaces, NodeFacesScope, NodeStyle,
    NodeView, PortIconOverrides, PortStyle, PortView, WirePainter, FIRST_AT, NODE_WIDTH,
};

use makepad_widgets::ScriptVm;

/// Register the flow graph widget and its DSL-facing style records.
pub fn script_mod(vm: &mut ScriptVm) {
    canvas::script_mod(vm);
}
