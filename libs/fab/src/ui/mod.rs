//! Lane D owns this directory (plus `../theme.rs`).
//!
//! The Fab-grade UI shell: the area system (Dock tree, per-area header
//! with an editor-type dropdown that swaps the content, corner split/join,
//! Ctrl+Space maximize), the top bar with workspace tabs, the context-sensitive
//! status bar, the 3D viewport chrome (header, T toolbar, N sidebar, overlay
//! text, gizmo placement, Z pie), the outliner, the properties editor
//! (vertical icon tabs, collapsible panels, drag-numeric fields, swatches),
//! the in-app file browser (Cmd+O, drag-and-drop, recent), the keymap help
//! and the F3 command palette — every icon an SVG of ours.
//!
//! Every lane's widgets are *placed* here (`area.rs`, `viewport_area.rs`,
//! `shell.rs`), never re-implemented: `FabViewport` (B), `FabNavGizmo` (C),
//! `FabToolOverlay` / `FabSheetView` (E), `FabRenderView` (F),
//! `FabToursPanel` (G).

pub mod area;
pub mod colorpick;
pub mod command_palette;
pub mod dragnum;
pub mod dropdown;
pub mod file_browser;
pub mod icons;
pub mod info;
pub mod keymap;
pub mod menu;
pub mod menubar;
pub mod outliner;
pub mod pie;
pub mod popover;
pub mod properties;
pub mod shell;
pub mod statusbar;
pub mod texview;
pub mod topbar;
pub mod viewport_area;
pub mod widgets;

use crate::api::*;
use makepad_widgets::*;

/// The control kit (icons + styled base widgets + the two controls that are
/// ours). Registered right after the theme so every lane's `script_mod!` can
/// use `FabButton`, `FabIcon`, `FabDragNumber`… .
pub fn script_mod_kit(vm: &mut ScriptVm) {
    icons::script_mod(vm);
    widgets::script_mod(vm);
    dragnum::script_mod(vm);
    colorpick::script_mod(vm);
    texview::script_mod(vm);
    dropdown::script_mod(vm);
    popover::script_mod(vm);
    pie::script_mod(vm);
}

/// The panels and the shell. Registered after every lane so it can place
/// their widgets.
pub fn script_mod(vm: &mut ScriptVm) {
    menubar::script_mod(vm);
    topbar::script_mod(vm);
    statusbar::script_mod(vm);
    outliner::script_mod(vm);
    properties::script_mod(vm);
    info::script_mod(vm);
    viewport_area::script_mod(vm);
    area::script_mod(vm);
    file_browser::script_mod(vm);
    keymap::script_mod(vm);
    command_palette::script_mod(vm);
    shell::script_mod(vm);
}

/// Lane D's action hook, called from `App::dispatch`.
pub fn apply(_cx: &mut Cx, _state: &mut AppState, _action: &ShellAction) -> bool {
    false
}
