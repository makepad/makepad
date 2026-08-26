//! Lane E owns this directory (L6 — 2D sheets / layouts).
//!
//! * `view.rs` — `FabSheetView`: paper-millimetre drawing with pan/zoom, the
//!   sheet picker, and sheet ↔ model cross-highlight through `SheetLink`.
//! * `fixture.rs` — plans and an elevation generated from the loaded model,
//!   used while `Scene::sheets` is empty (which is *always*, today: Fab
//!   publishes sheets as raster tile pyramids, which `fab::model::Sheet`
//!   cannot express — see lane E's report, R5).

pub mod export;
pub mod fixture;
pub mod plan;
pub mod slice;
pub mod view;

use crate::api::*;
use makepad_widgets::*;

pub fn script_mod(vm: &mut ScriptVm) {
    view::script_mod(vm);
}

/// Lane E's sheets action hook, called from `App::dispatch`.
pub fn apply(_cx: &mut Cx, state: &mut AppState, action: &ShellAction) -> bool {
    match action {
        ShellAction::Loaded(_) => {
            // The new scene's sheets are numbered from zero again.
            state.ui.active_sheet = None;
            true
        }
        ShellAction::HoverElement(id) => {
            let hit = id.and_then(|eid| {
                let el = state.scene.element(eid)?;
                Some(RayHit {
                    element: eid,
                    batch: 0,
                    triangle: 0,
                    t: 0.0,
                    point: aabb_center(&el.bounds),
                    normal: vec3(0.0, 0.0, 1.0),
                    bary: [0.0, 0.0],
                })
            });
            for v in &mut state.views {
                v.hover = hit;
            }
            true
        }
        _ => false,
    }
}
