use crate::scene::{xr_widget_world_transform, XrNode};
use makepad_widgets::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use crate::util::scene_draw::apply_scene_to_draw_cube;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.CubeBase = #(Cube::register_widget(vm))
    mod.widgets.Cube = set_type_default() do mod.widgets.CubeBase{
        body: mod.widgets.XrBodyKind.Dynamic
        shared_object_policy: mod.widgets.XrSharedObjectPolicy.BootstrapShared
        size: vec3(0.1, 0.1, 0.1)
        color: vec4(0.82, 0.48, 0.28, 1.0)
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Script, Widget)]
pub struct Cube {
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[live(vec3(0.1, 0.1, 0.1))]
    size: Vec3f,
    #[live(vec4(0.82, 0.48, 0.28, 1.0))]
    color: Vec4f,
    #[live(0.02)]
    metallic: f32,
    #[live(0.52)]
    roughness: f32,
    #[live(0.02)]
    corner_radius: f32,
    #[live(3u32)]
    corner_segments: u32,
    #[cast]
    #[deref]
    node: XrNode,
}

impl Cube {
    pub fn half_extents(&self) -> Vec3f {
        vec3f(
            self.size.x.max(0.0) * 0.5,
            self.size.y.max(0.0) * 0.5,
            self.size.z.max(0.0) * 0.5,
        )
    }

    pub fn set_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.color != color {
            self.color = color;
            self.node.redraw(cx);
        }
    }

    pub fn node(&self) -> &XrNode {
        &self.node
    }
}

impl ScriptHook for Cube {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.node.set_implicit_physics_size(self.size);
    }
}

impl Widget for Cube {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(_scene) = apply_scene_to_draw_cube(&mut self.draw_cube, cx) else {
            return DrawStep::done();
        };
        let _ = (
            self.metallic,
            self.roughness,
            self.corner_radius,
            self.corner_segments,
        );
        self.draw_cube.transform =
            xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        self.draw_cube.cube_pos = vec3(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = self.size;
        self.draw_cube.color = self.color;
        self.draw_cube.depth_clip = 1.0;
        self.draw_cube.draw(cx);

        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
