use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use super::{node::xr_runtime_body_from_scope, scene_3d::apply_scene_to_draw_cube, XrNode};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.XrCubeBase = #(XrCube::register_widget(vm))
    mod.widgets.XrCube = set_type_default() do mod.widgets.XrCubeBase{
        body: mod.widgets.XrBodyKind.Dynamic
        size: vec3(0.1, 0.1, 0.1)
        color: vec4(0.82, 0.48, 0.28, 1.0)
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrCube {
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
    #[deref]
    node: XrNode,
}

impl XrCube {
    pub fn half_extents(&self) -> Vec3f {
        vec3f(
            self.size.x.max(0.0) * 0.5,
            self.size.y.max(0.0) * 0.5,
            self.size.z.max(0.0) * 0.5,
        )
    }

    pub fn node(&self) -> &XrNode {
        &self.node
    }
}

impl Widget for XrCube {
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
        if let Some(runtime_body) = xr_runtime_body_from_scope(scope, self.widget_uid()) {
            self.draw_cube.transform = Mat4f::mul(
                &runtime_body.pose.to_mat4(),
                &Mat4f::nonuniform_scaled_translation(
                    vec3(
                        runtime_body.scale.x,
                        runtime_body.scale.y,
                        runtime_body.scale.z,
                    ),
                    vec3(0.0, 0.0, 0.0),
                ),
            );
        } else {
            let parent_world = cx.scene_world_transform_3d();
            self.draw_cube.transform = Mat4f::mul(&parent_world, &self.node.local_transform());
        }
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
