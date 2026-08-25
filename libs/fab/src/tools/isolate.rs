//! Lane E: hide / isolate / unhide, and solo mode.
//!
//! Thin over `SceneState` (which owns the semantics) so the keymap, the tool
//! panel and — when lane D wires it — the outliner's context menu all go
//! through one implementation. Everything here emits actions; nothing mutates
//! `AppState` behind the app's back.
//!
//! * `H` hide selected · `Shift+H` isolate selected · `Alt+H` unhide all
//! * `/` solo: isolate the selection, press again to come back (Fab's
//!   local view, and how you read a storey in a Fab model)

use crate::api::*;
use makepad_widgets::*;

pub fn hide_selected(cx: &mut Cx) {
    cx.action(ShellAction::HideSelected);
}

pub fn isolate_selected(cx: &mut Cx) {
    cx.action(ShellAction::IsolateSelected);
}

pub fn unhide_all(cx: &mut Cx) {
    cx.action(ShellAction::UnhideAll);
}

/// True while an isolation is in force.
pub fn is_solo(state: &AppState) -> bool {
    state.scene_state.isolated.is_some()
}

/// `/` — isolate the selection, or come back if already isolated.
pub fn toggle_solo(cx: &mut Cx, state: &AppState) -> &'static str {
    if is_solo(state) {
        unhide_all(cx);
        "Solo off"
    } else if state.scene_state.selection.is_empty() {
        "Nothing selected to isolate"
    } else {
        isolate_selected(cx);
        "Solo on"
    }
}

/// Select a storey's elements and isolate them.
pub fn isolate_story(cx: &mut Cx, scene: &Scene, story: StoryId) -> bool {
    let Some(s) = scene.stories.get(story.index()) else {
        return false;
    };
    let ids: Vec<ElementId> = s
        .elements
        .iter()
        .copied()
        .filter(|id| scene.element(*id).map_or(false, |e| e.has_geometry()))
        .collect();
    if ids.is_empty() {
        return false;
    }
    cx.action(ShellAction::SelectSet(ids));
    cx.action(ShellAction::IsolateSelected);
    true
}

/// Select a layer's elements and isolate them.
pub fn isolate_layer(cx: &mut Cx, scene: &Scene, layer: LayerId) -> bool {
    let Some(l) = scene.layers.get(layer.index()) else {
        return false;
    };
    let ids: Vec<ElementId> = l
        .elements
        .iter()
        .copied()
        .filter(|id| scene.element(*id).map_or(false, |e| e.has_geometry()))
        .collect();
    if ids.is_empty() {
        return false;
    }
    cx.action(ShellAction::SelectSet(ids));
    cx.action(ShellAction::IsolateSelected);
    true
}

/// Isolate everything of one class — "show me only the windows".
pub fn isolate_class(cx: &mut Cx, scene: &Scene, class: &ElementClass) -> bool {
    let ids: Vec<ElementId> = scene
        .elements
        .iter()
        .filter(|e| e.has_geometry() && e.class == *class)
        .map(|e| e.id)
        .collect();
    if ids.is_empty() {
        return false;
    }
    cx.action(ShellAction::SelectSet(ids));
    cx.action(ShellAction::IsolateSelected);
    true
}

/// Hide everything of one class (the "turn the furniture off" move).
pub fn hide_class(cx: &mut Cx, scene: &Scene, class: &ElementClass) -> usize {
    let mut n = 0;
    for e in scene.elements.iter().filter(|e| e.has_geometry() && e.class == *class) {
        cx.action(ShellAction::SetHidden(e.id, true));
        n += 1;
    }
    n
}

/// One line for the tool panel: what is currently switched off.
pub fn describe(state: &AppState) -> String {
    let hidden = state.scene_state.hidden.len();
    let iso = state.scene_state.isolated.as_ref().map(|s| s.len());
    match (hidden, iso) {
        (0, None) => "Everything visible".into(),
        (h, None) => format!("{h} hidden"),
        (0, Some(n)) => format!("Isolated: {n}"),
        (h, Some(n)) => format!("Isolated: {n} · {h} hidden"),
    }
}
