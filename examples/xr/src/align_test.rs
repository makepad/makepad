use makepad_xr::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::{ScriptAsyncId, ScriptAsyncResult},
    xr_node::xr_widget_world_transform,
    *,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.AlignTestBase = #(AlignTest::register_widget(vm))
    mod.widgets.AlignTest = set_type_default() do mod.widgets.AlignTestBase{
        body: mod.widgets.XrBodyKind.Disabled
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct AlignTest {
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[rust]
    enabled: bool,
    #[rust]
    last_mesh_generation: u64,
    #[rust]
    last_mesh_update_sequence: u64,
    #[rust]
    last_status: String,
    #[rust]
    local_markers: Option<[Vec3f; 2]>,
    #[rust]
    remote_markers_local: Option<[Vec3f; 2]>,
    #[rust]
    last_solution: Option<XrNetAlignmentSolution>,
    #[cast]
    #[deref]
    node: XrNode,
}

impl AlignTest {
    pub fn status_text(&self) -> &str {
        if self.last_status.is_empty() {
            "Test align: off"
        } else {
            &self.last_status
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) -> bool {
        if self.enabled == enabled {
            return self.enabled;
        }
        self.enabled = enabled;
        cx.xr_depth_mesh().set_surface_analysis_enabled(enabled);
        cx.xr_depth_mesh().set_alignment_preview_enabled(enabled);
        self.last_mesh_generation = 0;
        self.last_mesh_update_sequence = 0;
        self.last_solution = None;
        self.local_markers = None;
        self.remote_markers_local = None;
        self.last_status = if enabled {
            "Test align: waiting for TSDF alignment descriptor from loopback sync".to_string()
        } else {
            "Test align: off".to_string()
        };
        self.redraw(cx);
        self.enabled
    }

    fn refresh_alignment(&mut self, cx: &mut Cx, _time: f64) {
        if !self.enabled {
            return;
        }

        let Some(depth_mesh) = cx.xr_depth_mesh().latest_mesh() else {
            self.last_solution = None;
            self.local_markers = None;
            self.remote_markers_local = None;
            self.last_status =
                "Test align: waiting for TSDF depth snapshot for loopback packets".to_string();
            return;
        };

        let mesh_unchanged = depth_mesh.mesh_generation == self.last_mesh_generation
            && depth_mesh.update_sequence == self.last_mesh_update_sequence;
        if mesh_unchanged {
            return;
        }

        self.last_mesh_generation = depth_mesh.mesh_generation;
        self.last_mesh_update_sequence = depth_mesh.update_sequence;
        let Some(_local_descriptor) = depth_mesh.alignment_descriptor.as_ref() else {
            self.last_solution = None;
            self.local_markers = None;
            self.remote_markers_local = None;
            self.last_status = Self::missing_patch_status(&depth_mesh);
            self.redraw(cx);
            return;
        };
        let preview = &depth_mesh.alignment_preview;
        self.local_markers = preview.local_markers;
        self.remote_markers_local = preview.remote_markers_local;
        self.last_solution = preview.solution;

        let Some(solution) = preview.solution else {
            if preview.local_markers.is_none() {
                self.last_status =
                    "Test align: descriptor exists but floor marker frame is unstable".to_string();
            } else {
                let (local_floor, local_wall) =
                    (preview.local_floor_sample_count, preview.local_wall_sample_count);
                let (remote_floor, remote_wall) =
                    (preview.remote_floor_sample_count, preview.remote_wall_sample_count);
                self.last_status = format!(
                    "Test align: no loopback solve yet | local samples {} (f{} w{}) | remote samples {} (f{} w{})",
                    preview.local_sample_count,
                    local_floor,
                    local_wall,
                    preview.remote_sample_count,
                    remote_floor,
                    remote_wall
                );
            }
            self.redraw(cx);
            return;
        };

        let ground_truth_translation = vec3f(-0.82, 0.0, 0.67);
        let ground_truth_yaw = 0.58f32;
        let position_error_cm = (solution.translation - ground_truth_translation).length() * 100.0;
        let yaw_error_deg = wrap_angle(solution.yaw_radians - ground_truth_yaw)
            .abs()
            .to_degrees();
        let overlap_error_cm = marker_overlap_error(self.local_markers, self.remote_markers_local);

        self.last_status = format!(
            "Test align: loopback {:.0}% conf | yaw err {:.1} deg | pos err {:.0} cm | overlap err {:.0} cm | matched {} | samples {} (f{} w{})",
            solution.confidence * 100.0,
            yaw_error_deg,
            position_error_cm,
            overlap_error_cm,
            solution.matched_samples,
            preview.local_sample_count,
            preview.local_floor_sample_count,
            preview.local_wall_sample_count
        );
        self.redraw(cx);
    }

    fn missing_patch_status(depth_mesh: &XrDepthMesh) -> String {
        let debug = depth_mesh.alignment_debug;
        format!(
            "Test align: no TSDF align descriptor yet | near {} | floor cand {} -> {} | wall cand {} -> {} | tris {}",
            debug.near_surface_voxel_count,
            debug.floor_candidate_count,
            debug.floor_sample_count,
            debug.wall_candidate_count,
            debug.wall_sample_count,
            depth_mesh.mesh_triangle_count
        )
    }

    fn marker_color(index: usize, alpha: f32) -> Vec4f {
        match index {
            0 => vec4f(1.0, 0.20, 0.20, alpha),
            1 => vec4f(0.22, 0.48, 1.0, alpha),
            _ => vec4f(0.92, 0.92, 0.92, alpha),
        }
    }

    fn draw_marker(
        &mut self,
        cx: &mut Cx3d,
        world: &Mat4f,
        center: Vec3f,
        size: Vec3f,
        color: Vec4f,
    ) {
        self.draw_cube.transform = Mat4f::mul(world, &Pose::new(Quat::default(), center).to_mat4());
        self.draw_cube.cube_pos = vec3f(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = 1.0;
        self.draw_cube.draw(cx);
    }
}

impl Widget for AlignTest {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_enabled) {
            let mut enabled = self.enabled;
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                enabled = vm
                    .bx
                    .heap
                    .cast_to_bool(vm.bx.heap.vec_value(args_obj, 0, trap));
            }
            let enabled = vm.with_cx_mut(|cx| self.set_enabled(cx, enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(toggle_enabled) || method == live_id!(toggle_test) {
            let enabled = vm.with_cx_mut(|cx| self.set_enabled(cx, !self.enabled));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(enabled));
        }
        if method == live_id!(enabled) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.enabled));
        }
        self.node.script_call(vm, method, args)
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        self.node.script_result(vm, id, result);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::XrUpdate(update) = event {
            self.refresh_alignment(cx, update.state.time);
        }
        self.node.handle_event(cx, event, scope);
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.enabled {
            return self.node.draw_3d(cx, scope);
        }
        if cx.scene_state_3d().is_none() {
            return DrawStep::done();
        }
        let world = xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        if let Some(local_markers) = self.local_markers {
            for (index, center) in local_markers.into_iter().enumerate() {
                self.draw_marker(
                    cx,
                    &world,
                    center,
                    vec3f(0.060, 0.060, 0.060),
                    Self::marker_color(index, 1.0),
                );
            }
        }
        if let Some(remote_markers_local) = self.remote_markers_local {
            for (index, center) in remote_markers_local.into_iter().enumerate() {
                self.draw_marker(
                    cx,
                    &world,
                    center,
                    vec3f(0.066, 0.066, 0.066),
                    Self::marker_color(index, 0.34),
                );
            }
        }
        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

fn marker_overlap_error(local: Option<[Vec3f; 2]>, remote: Option<[Vec3f; 2]>) -> f32 {
    let (Some(local), Some(remote)) = (local, remote) else {
        return 0.0;
    };
    ((remote[0] - local[0]).length() + (remote[1] - local[1]).length()) * 50.0
}

fn wrap_angle(mut angle: f32) -> f32 {
    while angle <= -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    angle
}
