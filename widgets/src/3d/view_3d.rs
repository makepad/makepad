use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.View3DBase = #(View3D::register_widget(vm))
    mod.widgets.View3D = set_type_default() do mod.widgets.View3DBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct View3D {
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
}

impl Widget for View3D {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        // Placeholder for full UI-to-texture 3D embedding path:
        // for now, forward to the 2D subtree so View3D behaves as a wrapper node.
        let cx2d = &mut Cx2d::new(cx.cx);
        self.view.draw_all(cx2d, scope);
        DrawStep::done()
    }
}
