pub use makepad_widgets;
pub use makepad_widgets::*;

use makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::path::PathBuf;

app_main!(App);

mod css_3d_plane;
mod mp_surface;

const CARD_W: f32 = 320.0;
const CARD_H: f32 = 240.0;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x0b1118
                body +: {
                    root := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: Inset{left: 56 top: 56 right: 56 bottom: 56}
                        align: Align{x: 0.5 y: 0.5}

                        row := View{
                            width: Fit
                            height: Fit
                            flow: Right
                            spacing: 40
                            align: Align{x: 0.5 y: 0.5}

                            col_x := View{
                                width: 320
                                height: Fit
                                flow: Down
                                spacing: 12
                                align: Align{x: 0.5 y: 0.0}

                                plane_x := Css3dPlane{
                                    width: 320
                                    height: 240
                                    accent_color: #x4f8cff
                                    action_color: #x4f8cff
                                    variant: 0.0
                                }
                            }

                            col_y := View{
                                width: 320
                                height: Fit
                                flow: Down
                                spacing: 12
                                align: Align{x: 0.5 y: 0.0}

                                plane_y := Css3dPlane{
                                    width: 320
                                    height: 240
                                    accent_color: #x45b97a
                                    action_color: #x45b97a
                                    variant: 1.0
                                }
                            }

                            col_z := View{
                                width: 320
                                height: Fit
                                flow: Down
                                spacing: 12
                                align: Align{x: 0.5 y: 0.0}

                                plane_z := Css3dPlane{
                                    width: 320
                                    height: 240
                                    accent_color: #xe48a3a
                                    action_color: #xe48a3a
                                    variant: 2.0
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PlanePose {
    transform_matrix: Mat4f,
    perspective_matrix: Mat4f,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    pending_capture_request: Option<u64>,
    #[rust]
    capture_poll: Option<NextFrame>,
    #[rust]
    capture_path: PathBuf,
    #[rust]
    frames_until_capture: u32,
}

impl App {
    fn css_rotate_x(angle: f32) -> Mat4f {
        let c = angle.cos();
        let s = angle.sin();
        Mat4f {
            v: [
                1.0, 0.0, 0.0, 0.0,
                0.0, c, s, 0.0,
                0.0, -s, c, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    fn css_rotate_y(angle: f32) -> Mat4f {
        let c = angle.cos();
        let s = angle.sin();
        Mat4f {
            v: [
                c, 0.0, -s, 0.0,
                0.0, 1.0, 0.0, 0.0,
                s, 0.0, c, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    fn css_rotate_z(angle: f32) -> Mat4f {
        let c = angle.cos();
        let s = angle.sin();
        Mat4f {
            v: [
                c, s, 0.0, 0.0,
                -s, c, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    fn css_rotation_matrix(rotation: Vec3f) -> Mat4f {
        Mat4f::mul(
            &Self::css_rotate_z(rotation.z),
            &Mat4f::mul(&Self::css_rotate_y(rotation.y), &Self::css_rotate_x(rotation.x)),
        )
    }

    fn css_perspective_matrix(distance: f32) -> Mat4f {
        if distance <= 0.0 {
            return Mat4f::identity();
        }
        Mat4f {
            v: [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, -1.0 / distance,
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    fn around_origin(origin: Vec3f, transform: Mat4f) -> Mat4f {
        Mat4f::mul(
            &Mat4f::translation(origin),
            &Mat4f::mul(
                &transform,
                &Mat4f::translation(vec3(-origin.x, -origin.y, -origin.z)),
            ),
        )
    }

    fn demo_plane_pose(size: DVec2, rotation: Vec3f, perspective: f32) -> PlanePose {
        let origin = vec3(size.x as f32 * 0.5, size.y as f32 * 0.5, 0.0);
        PlanePose {
            transform_matrix: Self::around_origin(origin, Self::css_rotation_matrix(rotation)),
            perspective_matrix: Self::around_origin(origin, Self::css_perspective_matrix(perspective)),
        }
    }

    fn demo_plane_poses() -> [PlanePose; 3] {
        let size = dvec2(CARD_W as f64, CARD_H as f64);
        [
            Self::demo_plane_pose(size, vec3(0.62, 0.0, 0.0), 960.0),
            Self::demo_plane_pose(size, vec3(0.0, -0.70, 0.0), 960.0),
            Self::demo_plane_pose(size, vec3(0.0, 0.0, 0.26), 0.0),
        ]
    }

    fn bind_plane_matrices(&mut self, cx: &mut Cx) {
        for (plane_id, pose) in [ids!(plane_x), ids!(plane_y), ids!(plane_z)]
            .into_iter()
            .zip(Self::demo_plane_poses())
        {
            if let Some(mut plane) = self
                .ui
                .widget(cx, plane_id)
                .borrow_mut::<css_3d_plane::Css3dPlane>()
            {
                plane.set_matrices(pose.transform_matrix, pose.perspective_matrix);
            }
        }
    }

    fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| "rgba size overflow while encoding png".to_string())?;
        if rgba.len() != expected {
            return Err(format!("expected {} rgba bytes, got {}", expected, rgba.len()));
        }

        let options = EncoderOptions::default()
            .set_width(width as usize)
            .set_height(height as usize)
            .set_depth(BitDepth::Eight)
            .set_colorspace(ColorSpace::RGBA);
        let mut encoder = PngEncoder::new(rgba, options);
        let mut out = Vec::new();
        encoder
            .encode(&mut out)
            .map_err(|err| format!("png encode failed: {err:?}"))?;
        Ok(out)
    }

    fn request_capture(&mut self, cx: &mut Cx) {
        self.capture_path = PathBuf::from("/tmp/comp-3d.png");
        self.pending_capture_request = Some(cx.request_capture(CaptureSource::Framebuffer));
        self.capture_poll = Some(cx.new_next_frame());
    }

    fn poll_capture(&mut self, cx: &mut Cx) {
        let Some(expected_request_id) = self.pending_capture_request else {
            return;
        };

        for result in cx.drain_capture_results() {
            if result.request_id != expected_request_id {
                continue;
            }
            if let Ok(png) = Self::encode_png_rgba(result.width, result.height, &result.rgba) {
                let _ = std::fs::write(&self.capture_path, png);
                println!("screenshot {}", self.capture_path.display());
            }
            self.pending_capture_request = None;
            self.capture_poll = None;
            cx.quit();
            return;
        }

        self.capture_poll = Some(cx.new_next_frame());
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.bind_plane_matrices(cx);
        self.frames_until_capture = 6;
        self.capture_poll = Some(cx.new_next_frame());
        self.ui.redraw(cx);
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::css_3d_plane::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.bind_plane_matrices(cx);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Some(next) = self.capture_poll {
            if next.is_event(event).is_some() {
                if self.pending_capture_request.is_some() {
                    self.poll_capture(cx);
                } else if self.frames_until_capture > 0 {
                    self.frames_until_capture -= 1;
                    self.capture_poll = Some(cx.new_next_frame());
                } else {
                    self.request_capture(cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_rotate_x(point: Vec3f, angle: f32) -> Vec3f {
        let c = angle.cos();
        let s = angle.sin();
        vec3(point.x, point.y * c - point.z * s, point.y * s + point.z * c)
    }

    fn legacy_rotate_y(point: Vec3f, angle: f32) -> Vec3f {
        let c = angle.cos();
        let s = angle.sin();
        vec3(point.x * c + point.z * s, point.y, point.z * c - point.x * s)
    }

    fn legacy_rotate_z(point: Vec3f, angle: f32) -> Vec3f {
        let c = angle.cos();
        let s = angle.sin();
        vec3(point.x * c - point.y * s, point.x * s + point.y * c, point.z)
    }

    fn legacy_project_point(size: DVec2, rotation: Vec3f, perspective: f32, point: DVec2) -> Vec3f {
        let origin = vec3(size.x as f32 * 0.5, size.y as f32 * 0.5, 0.0);
        let around_origin = vec3(point.x as f32 - origin.x, point.y as f32 - origin.y, 0.0);
        let rotated = legacy_rotate_z(
            legacy_rotate_y(legacy_rotate_x(around_origin, rotation.x), rotation.y),
            rotation.z,
        );
        let plane = rotated + origin;
        if perspective <= 0.0 {
            return plane;
        }
        let local = plane - origin;
        let scale = perspective / (perspective - local.z);
        vec3(origin.x + local.x * scale, origin.y + local.y * scale, local.z)
    }

    fn matrix_project_point(pose: PlanePose, point: DVec2) -> Vec3f {
        let local = vec4f(point.x as f32, point.y as f32, 0.0, 1.0);
        let transformed = pose.transform_matrix.transform_vec4(local);
        let projected = pose.perspective_matrix.transform_vec4(transformed);
        vec3(projected.x / projected.w, projected.y / projected.w, transformed.z)
    }

    fn assert_close(actual: Vec3f, expected: Vec3f) {
        assert!((actual.x - expected.x).abs() < 0.0001, "x mismatch: actual={actual:?} expected={expected:?}");
        assert!((actual.y - expected.y).abs() < 0.0001, "y mismatch: actual={actual:?} expected={expected:?}");
        assert!((actual.z - expected.z).abs() < 0.0001, "z mismatch: actual={actual:?} expected={expected:?}");
    }

    #[test]
    fn matrix_plane_matches_previous_rotate_x_path() {
        let size = dvec2(CARD_W as f64, CARD_H as f64);
        let rotation = vec3(0.62, 0.0, 0.0);
        let perspective = 960.0;
        let pose = App::demo_plane_pose(size, rotation, perspective);

        for point in [
            dvec2(0.0, 0.0),
            dvec2(CARD_W as f64, 0.0),
            dvec2(0.0, CARD_H as f64),
            dvec2(CARD_W as f64, CARD_H as f64),
        ] {
            assert_close(
                matrix_project_point(pose, point),
                legacy_project_point(size, rotation, perspective, point),
            );
        }
    }

    #[test]
    fn matrix_plane_matches_previous_rotate_y_path() {
        let size = dvec2(CARD_W as f64, CARD_H as f64);
        let rotation = vec3(0.0, -0.70, 0.0);
        let perspective = 960.0;
        let pose = App::demo_plane_pose(size, rotation, perspective);

        for point in [
            dvec2(0.0, 0.0),
            dvec2(CARD_W as f64, 0.0),
            dvec2(0.0, CARD_H as f64),
            dvec2(CARD_W as f64, CARD_H as f64),
        ] {
            assert_close(
                matrix_project_point(pose, point),
                legacy_project_point(size, rotation, perspective, point),
            );
        }
    }

    #[test]
    fn matrix_plane_matches_previous_rotate_z_path() {
        let size = dvec2(CARD_W as f64, CARD_H as f64);
        let rotation = vec3(0.0, 0.0, 0.26);
        let perspective = 0.0;
        let pose = App::demo_plane_pose(size, rotation, perspective);

        for point in [
            dvec2(0.0, 0.0),
            dvec2(CARD_W as f64, 0.0),
            dvec2(0.0, CARD_H as f64),
            dvec2(CARD_W as f64, CARD_H as f64),
        ] {
            assert_close(
                matrix_project_point(pose, point),
                legacy_project_point(size, rotation, perspective, point),
            );
        }
    }
}
