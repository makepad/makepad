pub mod graph_edit;
pub mod graph_view;
pub mod editor;
pub mod bindings;
pub mod camera_events;
pub mod theme;
pub mod node_picker;

pub use editor::{DebugInstance, FlowEditor, FlowEditorAction, FlowEditorRef, FlowEditorWidgetRefExt};
pub use node_picker::NodeChoice;

/// Register the flow editor after widgets and the shared flow canvas.
pub fn script_mod(vm: &mut makepad_widgets::ScriptVm) {
    node_picker::script_mod(vm);
    editor::script_mod(vm);
}

#[cfg(feature = "remote-values")]
pub mod values;
