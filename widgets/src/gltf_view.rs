use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::{path::PathBuf, rc::Rc};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.GltfViewBase = #(GltfView::register_widget(vm))

    mod.widgets.GltfView = set_type_default() do mod.widgets.GltfViewBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x1b2028
            // Push background to far depth so the reserved 3D depth slice stays visible.
            draw_depth: -99.0
        }
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.22
            spec_power: 128.0
            spec_strength: 0.9
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct GltfView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    draw_list_3d: DrawList2d,

    /// glTF/GLB resource handle. Set from script with `crate_resource()`.
    #[live]
    src: Option<ScriptHandleRef>,
    /// Optional environment equirect image (jpg/png) resource.
    #[live]
    env_src: Option<ScriptHandleRef>,

    #[live(true)]
    animating: bool,
    #[live(0.35)]
    spin_speed: f32,
    #[live(45.0)]
    camera_fov_y: f32,
    #[live(2.6)]
    camera_distance: f32,
    #[live(0.6)]
    camera_distance_min: f32,
    #[live(40.0)]
    camera_distance_max: f32,
    #[live(0.1)]
    wheel_zoom_step: f32,
    #[live(0.05)]
    camera_near: f32,
    #[live(100.0)]
    camera_far: f32,
    /// NDC depth range used for 3D content.
    #[live(vec2(0.0, 1.0))]
    depth_range: Vec2f,
    /// Extra forward depth bias; keep at 0 unless specifically needed.
    #[live(0.0)]
    depth_forward_bias: f32,

    #[rust]
    renderer: Option<GltfRenderer>,
    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    loaded_env_handle: Option<ScriptHandle>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    area: Area,
    #[rust]
    orbit_yaw: f32,
    #[rust]
    orbit_pitch: f32,
    #[rust]
    drag_last_abs: Option<DVec2>,
    #[rust]
    camera_target: Vec3f,
}

enum ResourceResolve {
    Ready {
        handle: ScriptHandle,
        abs_path: PathBuf,
        data: Rc<Vec<u8>>,
    },
    Pending {
        handle: ScriptHandle,
    },
    Error {
        handle: ScriptHandle,
    },
    Missing,
}

impl GltfView {
    fn resource_metadata_by_handle(cx: &mut Cx, handle: ScriptHandle) -> Option<(PathBuf, bool)> {
        let resources = cx.script_data.resources.resources.borrow();
        let resource = resources.iter().find(|resource| resource.handle == handle)?;
        Some((PathBuf::from(&resource.abs_path), resource.is_error()))
    }

    fn resolve_resource(cx: &mut Cx, handle_ref: &ScriptHandleRef) -> ResourceResolve {
        let handle = handle_ref.as_handle();

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        cx.load_all_script_resources();

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        if let Some((_, is_error)) = Self::resource_metadata_by_handle(cx, handle) {
            if is_error {
                return ResourceResolve::Error { handle };
            }
            return ResourceResolve::Pending { handle };
        }

        ResourceResolve::Missing
    }

    fn ensure_env_loaded(&mut self, cx: &mut Cx2d) {
        let Some(handle_ref) = self.env_src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();
        if self.loaded_env_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                if self
                    .draw_pbr
                    .load_default_env_equirect_from_bytes(cx, &data, Some(&abs_path))
                    .is_ok()
                {
                    self.area.redraw(cx);
                }
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn ensure_renderer_loaded(&mut self, cx: &mut Cx2d) {
        let Some(handle_ref) = self.src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();

        if self.loaded_src_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                match GltfRenderer::load_from_bytes(&mut self.draw_pbr, cx, &data, Some(&abs_path)) {
                    Ok(renderer) => {
                        self.apply_default_view_from_gltf(&renderer);
                        self.renderer = Some(renderer);
                    }
                    Err(_) => {
                        self.renderer = None;
                    }
                }
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.renderer = None;
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn apply_default_view_from_gltf(&mut self, renderer: &GltfRenderer) {
        self.camera_target = renderer.scene_center;

        let Some(default_view) = renderer.default_view.as_ref() else {
            return;
        };

        let eye = default_view.eye - self.camera_target;
        let distance = eye.length().max(0.001);
        self.camera_distance = distance;
        self.orbit_yaw = eye.x.atan2(eye.z);
        self.orbit_pitch = (eye.y / distance).clamp(-1.0, 1.0).asin();

        if let Some(fov_y_degrees) = default_view.fov_y_degrees {
            self.camera_fov_y = fov_y_degrees.clamp(1.0, 179.0);
        }
        if let Some(near) = default_view.near {
            self.camera_near = near.max(0.001);
        }
        if let Some(far) = default_view.far {
            self.camera_far = far.max(self.camera_near + 0.001);
        }
        self.camera_distance = self.camera_distance.clamp(
            self.camera_distance_min.max(0.01),
            self.camera_distance_max.max(self.camera_distance_min.max(0.01) + 0.01),
        );
    }

    fn clip_ndc_for_rect(rect: Rect, pass_size: Vec2d) -> Vec4f {
        let pass_w = pass_size.x.max(1.0) as f32;
        let pass_h = pass_size.y.max(1.0) as f32;

        let x0 = (2.0 * rect.pos.x as f32 / pass_w) - 1.0;
        let x1 = (2.0 * (rect.pos.x + rect.size.x) as f32 / pass_w) - 1.0;

        let y0 = 1.0 - (2.0 * rect.pos.y as f32 / pass_h);
        let y1 = 1.0 - (2.0 * (rect.pos.y + rect.size.y) as f32 / pass_h);

        vec4(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
    }

    fn clip_space_viewport_matrix(clip_ndc: Vec4f) -> Mat4f {
        let sx = (clip_ndc.z - clip_ndc.x) * 0.5;
        let sy = (clip_ndc.w - clip_ndc.y) * 0.5;
        let tx = (clip_ndc.z + clip_ndc.x) * 0.5;
        let ty = (clip_ndc.w + clip_ndc.y) * 0.5;

        Mat4f {
            v: [
                sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0,
            ],
        }
    }

    fn compute_subengine_transform(
        &self,
        rect: Rect,
        pass_size: Vec2d,
        pass_from_world_inv: Mat4f,
    ) -> (Mat4f, Vec3f, Vec4f, Mat4f, Mat4f) {
        let clip_ndc = Self::clip_ndc_for_rect(rect, pass_size);

        let viewport = Self::clip_space_viewport_matrix(clip_ndc);
        let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
        let fov = self.camera_fov_y.clamp(1.0, 179.0);
        let near = self.camera_near.max(0.001);
        let far = self.camera_far.max(near + 0.001);
        let projection = Mat4f::perspective(fov, aspect, near, far);

        let distance = self.camera_distance.max(0.001);
        let yaw_angle = self.orbit_yaw + (self.time as f32) * self.spin_speed;
        let pitch_angle = self.orbit_pitch.clamp(-1.45, 1.45);
        let cos_pitch = pitch_angle.cos();
        let camera_pos = vec3(
            distance * yaw_angle.sin() * cos_pitch,
            distance * pitch_angle.sin(),
            distance * yaw_angle.cos() * cos_pitch,
        ) + self.camera_target;
        let view = Mat4f::look_at(camera_pos, self.camera_target, vec3(0.0, 1.0, 0.0));

        let clip_from_world = Mat4f::mul(&viewport, &Mat4f::mul(&view, &projection));

        let draw_list_view = Mat4f::mul(&pass_from_world_inv, &clip_from_world);
        (draw_list_view, camera_pos, clip_ndc, view, projection)
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, rect: Rect) {
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }

        let pass_size = cx.current_pass_size();
        if pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            return;
        }

        let Some(draw_list_id) = cx.get_current_draw_list_id() else {
            return;
        };

        let pass_id = cx.draw_lists[draw_list_id].draw_pass_id.unwrap();
        let pass_from_world = Mat4f::mul(
            &cx.passes[pass_id].pass_uniforms.camera_projection,
            &cx.passes[pass_id].pass_uniforms.camera_view,
        );
        let pass_from_world_inv = pass_from_world.invert();

        let (view_transform, camera_pos, clip_ndc, view_3d, projection_3d) =
            self.compute_subengine_transform(rect, pass_size, pass_from_world_inv);
        cx.draw_lists[draw_list_id].draw_list_uniforms.view_transform = view_transform;

        self.draw_pbr.set_clip_ndc(clip_ndc);
        self.draw_pbr
            .set_depth_range(self.depth_range.x, self.depth_range.y);
        self.draw_pbr
            .set_depth_forward_bias(self.depth_forward_bias);
        self.draw_pbr.set_view_projection(view_3d, projection_3d);
        self.draw_pbr.camera_pos = camera_pos;

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };

        let _ = renderer.draw(&mut self.draw_pbr, cx);
    }
}

impl Widget for GltfView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.handle_event(cx, event);
        }

        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.drag_last_abs {
                    let delta = fe.abs - last_abs;
                    let sensitivity = 0.01_f32;
                    self.orbit_yaw -= (delta.x as f32) * sensitivity;
                    self.orbit_pitch = (self.orbit_pitch + (delta.y as f32) * sensitivity)
                        .clamp(-1.45, 1.45);
                    self.drag_last_abs = Some(fe.abs);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let step = self.wheel_zoom_step.max(0.001);
                let zoom_factor = if scroll > 0.0 { 1.0 / (1.0 - step) } else { 1.0 - step };
                self.camera_distance = (self.camera_distance * zoom_factor).clamp(
                    self.camera_distance_min.max(0.01),
                    self.camera_distance_max.max(self.camera_distance_min.max(0.01) + 0.01),
                );
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if self.drag_last_abs.take().is_some() {
                    if fe.is_over {
                        cx.set_cursor(MouseCursor::Grab);
                    } else {
                        cx.set_cursor(MouseCursor::Default);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                if self.drag_last_abs.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                } else {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.drag_last_abs.is_none() {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            _ => {}
        }

        if self.animating {
            if let Event::NextFrame(ne) = event {
                self.time = ne.time;
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
            if let Event::Startup = event {
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_env_loaded(cx);
        self.ensure_renderer_loaded(cx);

        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        self.draw_list_3d.begin_always(cx);
        self.draw_scene(cx, rect);
        self.draw_list_3d.end(cx);
        DrawStep::done()
    }
}
