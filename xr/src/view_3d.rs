use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

use super::scene_3d::apply_draw_call_reorder_for_draw_list;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.View3DBase = #(View3D::register_widget(vm))
    mod.widgets.View3D = set_type_default() do mod.widgets.View3DBase{
        width: Fill
        height: Fill
        sort_draw_calls_by_depth: true
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct View3D {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live(vec3(0.0, 0.0, 0.0))]
    pub position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    pub rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub scale: Vec3f,
    #[live(vec2(1.0, 1.0))]
    pub size_3d: Vec2f,
    #[live(false)]
    pub billboard: bool,
    #[live(true)]
    pub sort_draw_calls_by_depth: bool,
}

impl Widget for View3D {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        // Placeholder for full UI-to-texture 3D embedding path:
        // for now, forward to the 2D subtree so View3D behaves as a wrapper node.
        let cx2d = &mut Cx2d::new(cx.cx);
        self.view.draw_all(cx2d, scope);
        if self.sort_draw_calls_by_depth {
            if let Some(draw_list_id) = cx2d.get_current_draw_list_id() {
                apply_draw_call_reorder_for_draw_list(cx2d, scope, draw_list_id, true);
            }
        }
        DrawStep::done()
    }
}
