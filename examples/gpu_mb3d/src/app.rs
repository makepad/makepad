use makepad_mb3d_render::{formulas, m3p, render};
use makepad_widgets::*;
use std::path::PathBuf;

app_main!(App);

fn gpu_mb3d_uniforms_pod(vm: &mut ScriptVm) -> ScriptValue {
    let pod = GpuMb3dUniforms::script_pod(vm).expect("Cant make a pod type");
    vm.bx.heap.pod_type_name_set(pod, id_lut!(GpuMb3dUniforms));
    pod.into()
}

script_mod! {
    use mod.prelude.widgets.*

    let gpu_mb3d_uniforms = #(gpu_mb3d_uniforms_pod(vm))

    mod.widgets.DrawGpuMb3d = set_type_default() do #(DrawGpuMb3d::script_shader(vm)){
        ..mod.draw.DrawQuad
        debug_layout: true
        debug_code: true

        scene: uniform_buffer(gpu_mb3d_uniforms)

        ds_new: fn(v) {
            return vec2(v, 0.0)
        }

        ds_quick_two_sum: fn(a, b) {
            let s = a + b
            let e = b - (s - a)
            return vec2(s, e)
        }

        ds_two_sum: fn(a, b) {
            let s = a + b
            let bb = s - a
            let e = (a - (s - bb)) + (b - bb)
            return vec2(s, e)
        }

        ds_split: fn(a) {
            let c = 4097.0 * a
            let hi = c - (c - a)
            let lo = a - hi
            return vec2(hi, lo)
        }

        ds_two_prod: fn(a, b) {
            let p = a * b
            let a_split = self.ds_split(a)
            let b_split = self.ds_split(b)
            let e = ((a_split.x * b_split.x - p) + a_split.x * b_split.y + a_split.y * b_split.x) + a_split.y * b_split.y
            return vec2(p, e)
        }

        ds_norm: fn(v) {
            return self.ds_quick_two_sum(v.x, v.y)
        }

        ds_add: fn(a, b) {
            let s = self.ds_two_sum(a.x, b.x)
            return self.ds_quick_two_sum(s.x, s.y + a.y + b.y)
        }

        ds_sub: fn(a, b) {
            return self.ds_add(a, vec2(-b.x, -b.y))
        }

        ds_add_f: fn(a, b) {
            return self.ds_add(a, vec2(b, 0.0))
        }

        ds_mul_f: fn(a, b) {
            let p = self.ds_two_prod(a.x, b)
            return self.ds_norm(self.ds_quick_two_sum(p.x, p.y + a.y * b))
        }

        ds_mul: fn(a, b) {
            let p = self.ds_two_prod(a.x, b.x)
            return self.ds_norm(self.ds_quick_two_sum(p.x, p.y + a.x * b.y + a.y * b.x + a.y * b.y))
        }

        ds_div: fn(a, b) {
            let q1 = a.x / b.x
            let r = self.ds_sub(a, self.ds_mul_f(b, q1))
            let q2 = r.x / b.x
            let r2 = self.ds_sub(r, self.ds_mul_f(b, q2))
            let q3 = r2.x / b.x
            return self.ds_norm(self.ds_add_f(self.ds_quick_two_sum(q1, q2), q3))
        }

        ds_abs: fn(a) {
            if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                return vec2(-a.x, -a.y)
            }
            return a
        }

        ds_box_fold: fn(a, fold) {
            return self.ds_sub(
                self.ds_sub(
                    self.ds_abs(self.ds_add(a, fold)),
                    self.ds_abs(self.ds_sub(a, fold))
                ),
                a
            )
        }

        ds_to_f: fn(a) {
            return a.x + a.y
        }

        ds_lt: fn(a, b) {
            return a.x < b.x || (a.x == b.x && a.y < b.y)
        }

        ds_gt: fn(a, b) {
            return a.x > b.x || (a.x == b.x && a.y > b.y)
        }

        ds_max: fn(a, b) {
            if self.ds_lt(a, b) {
                return b
            }
            return a
        }

        ds_sqrt: fn(a) {
            let root = sqrt(max(self.ds_to_f(a), 0.0))
            if root == 0.0 {
                return self.ds_new(0.0)
            }
            let xds = self.ds_new(root)
            return self.ds_mul_f(self.ds_add(xds, self.ds_div(a, xds)), 0.5)
        }

        ds3_len: fn(x, y, z) {
            return self.ds_sqrt(
                self.ds_add(
                    self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)),
                    self.ds_mul(z, z)
                )
            )
        }

        safe_normalize3: fn(v) {
            let len = max(length(v), 0.000001)
            return v / len
        }

        frag_coord: fn() {
            return self.pos * self.rect_size
        }

        primary_dir: fn() {
            let frag = self.frag_coord()
            let half_w = self.rect_size.x * 0.5
            let half_h = self.rect_size.y * 0.5
            let fov_mul = (self.scene.controls0.x * 0.017453292519943295) / max(self.rect_size.y, 1.0)

            let cafx = (half_w - frag.x) * fov_mul
            let cafy = (frag.y - half_h) * fov_mul
            let sx = sin(cafx)
            let cx = cos(cafx)
            let sy = sin(cafy)
            let cy = cos(cafy)

            let local_dir = self.safe_normalize3(vec3(-sx, sy, cx * cy))
            let cam_right = vec3(
                self.ds_to_f(self.scene.cam_right_x),
                self.ds_to_f(self.scene.cam_right_y),
                self.ds_to_f(self.scene.cam_right_z)
            )
            let cam_up = vec3(
                self.ds_to_f(self.scene.cam_up_x),
                self.ds_to_f(self.scene.cam_up_y),
                self.ds_to_f(self.scene.cam_up_z)
            )
            let cam_forward = vec3(
                self.ds_to_f(self.scene.cam_forward_x),
                self.ds_to_f(self.scene.cam_forward_y),
                self.ds_to_f(self.scene.cam_forward_z)
            )

            return self.safe_normalize3(
                cam_right * local_dir.x +
                cam_up * local_dir.y +
                cam_forward * local_dir.z
            )
        }

        primary_march: fn() {
            let frag = self.frag_coord()
            let half_w = self.rect_size.x * 0.5
            let half_h = self.rect_size.y * 0.5
            let x_offset = self.ds_mul(self.ds_new(frag.x - half_w), self.scene.step_width)
            let y_offset = self.ds_mul(self.ds_new(frag.y - half_h), self.scene.step_width)
            let dir = self.primary_dir()

            let ox = self.ds_add(
                self.ds_add(
                    self.ds_add(self.scene.mid_x, self.ds_mul(self.scene.cam_forward_x, self.scene.z_start_delta)),
                    self.ds_mul(self.scene.cam_right_x, x_offset)
                ),
                self.ds_mul(self.scene.cam_up_x, y_offset)
            )
            let oy = self.ds_add(
                self.ds_add(
                    self.ds_add(self.scene.mid_y, self.ds_mul(self.scene.cam_forward_y, self.scene.z_start_delta)),
                    self.ds_mul(self.scene.cam_right_y, x_offset)
                ),
                self.ds_mul(self.scene.cam_up_y, y_offset)
            )
            let oz = self.ds_add(
                self.ds_add(
                    self.ds_add(self.scene.mid_z, self.ds_mul(self.scene.cam_forward_z, self.scene.z_start_delta)),
                    self.ds_mul(self.scene.cam_right_z, x_offset)
                ),
                self.ds_mul(self.scene.cam_up_z, y_offset)
            )

            return self.ray_march(
                ox,
                oy,
                oz,
                self.ds_new(dir.x),
                self.ds_new(dir.y),
                self.ds_new(dir.z)
            )
        }

        sky_for_y: fn(y) {
            let t = clamp(pow(1.0 - y, 0.7), 0.0, 1.0)
            return mix(self.scene.sky_color.rgb, self.scene.sky_color2.rgb, t)
        }

        calc_de: fn(px, py, pz) {
            let cx = if self.scene.controls1.y > 0.5 { self.scene.julia_x } else { px }
            let cy = if self.scene.controls1.y > 0.5 { self.scene.julia_y } else { py }
            let cz = if self.scene.controls1.y > 0.5 { self.scene.julia_z } else { pz }

            var x = px
            var y = py
            var z = pz
            var w = vec2(1.0, 0.0)
            var r2 = vec2(0.0, 0.0)
            var iters = 0.0
            var slot = 0.0
            var remaining = self.scene.controls0.z

            for i in 0..128 {
                if remaining <= 0.0 {
                    slot += 1.0
                    if slot >= 2.0 {
                        slot = self.scene.controls1.x
                    }
                    if slot < 0.5 {
                        remaining = self.scene.controls0.z
                    } else {
                        remaining = self.scene.controls0.w
                    }
                }

                if slot < 0.5 {
                    x = self.ds_box_fold(x, self.scene.ab_fold)
                    y = self.ds_box_fold(y, self.scene.ab_fold)
                    z = self.ds_box_fold(z, self.scene.ab_fold)

                    let rr = self.ds_add(
                        self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)),
                        self.ds_mul(z, z)
                    )
                    var m = self.scene.ab_scale
                    if self.ds_lt(rr, self.scene.ab_min_r2) {
                        m = self.scene.ab_scale_div_min_r2
                    } else if self.ds_lt(rr, self.ds_new(1.0)) {
                        m = self.ds_div(self.scene.ab_scale, rr)
                    }

                    w = self.ds_mul(w, m)
                    x = self.ds_add(self.ds_mul(x, m), cx)
                    y = self.ds_add(self.ds_mul(y, m), cy)
                    z = self.ds_add(self.ds_mul(z, m), cz)
                } else {
                    x = self.ds_abs(x)
                    y = self.ds_abs(y)
                    z = self.ds_abs(z)

                    if self.ds_lt(x, y) {
                        let t = x
                        x = y
                        y = t
                    }
                    if self.ds_lt(x, z) {
                        let t = x
                        x = z
                        z = t
                    }
                    if self.ds_lt(y, z) {
                        let t = y
                        y = z
                        z = t
                    }

                    let nx = self.ds_add(
                        self.ds_add(
                            self.ds_mul(x, self.scene.rot0_x),
                            self.ds_mul(y, self.scene.rot0_y)
                        ),
                        self.ds_mul(z, self.scene.rot0_z)
                    )
                    let ny = self.ds_add(
                        self.ds_add(
                            self.ds_mul(x, self.scene.rot1_x),
                            self.ds_mul(y, self.scene.rot1_y)
                        ),
                        self.ds_mul(z, self.scene.rot1_z)
                    )
                    let nz = self.ds_add(
                        self.ds_add(
                            self.ds_mul(x, self.scene.rot2_x),
                            self.ds_mul(y, self.scene.rot2_y)
                        ),
                        self.ds_mul(z, self.scene.rot2_z)
                    )

                    let sf = self.ds_sub(self.scene.menger_scale, self.ds_new(1.0))
                    x = self.ds_sub(self.ds_mul(nx, self.scene.menger_scale), self.ds_mul(self.scene.menger_cx, sf))
                    y = self.ds_sub(self.ds_mul(ny, self.scene.menger_scale), self.ds_mul(self.scene.menger_cy, sf))
                    let c = self.ds_mul(self.scene.menger_cz, sf)
                    z = self.ds_sub(c, self.ds_abs(self.ds_sub(self.ds_mul(nz, self.scene.menger_scale), c)))
                    w = self.ds_mul(w, self.scene.menger_scale)
                }

                iters += 1.0
                remaining -= 1.0
                r2 = self.ds_add(
                    self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)),
                    self.ds_mul(z, z)
                )
                if self.ds_gt(r2, self.scene.rstop) || iters >= self.scene.controls0.y {
                    break
                }
            }

            let de = self.ds_max(self.ds_div(self.ds_sqrt(r2), self.ds_abs(w)), self.scene.de_floor)
            return vec4(iters, de.x, de.y, 0.0)
        }

        scene_destop_at_steps: fn(depth_steps: vec2) -> vec2 {
            return self.ds_mul(
                self.scene.de_stop,
                self.ds_add(self.ds_new(1.0), self.ds_mul(self.ds_abs(depth_steps), self.scene.de_stop_factor))
            )
        }

        ray_march: fn(ox: vec2, oy: vec2, oz: vec2, dx: vec2, dy: vec2, dz: vec2) -> vec4 {
            var t = vec2(0.0, 0.0)
            var last_de = vec2(0.0, 0.0)
            var last_step = vec2(0.0, 0.0)
            var rsfmul = vec2(1.0, 0.0)

            let first_eval = self.calc_de(ox, oy, oz)
            let first_de = vec2(first_eval.y, first_eval.z)
            let first_stop = self.scene_destop_at_steps(self.ds_div(t, self.scene.step_width))
            if first_eval.x >= self.scene.controls0.y || self.ds_lt(first_de, first_stop) {
                return vec4(t.x, t.y, first_eval.x, 1.0)
            }

            last_step = self.ds_max(
                self.ds_mul(first_de, self.scene.s_z_step_div),
                self.ds_mul(self.scene.step_width, self.ds_new(0.11))
            )
            last_de = first_de

            for step_idx in 0..16384 {
                let depth_steps = self.ds_div(t, self.scene.step_width)
                let current_stop = self.scene_destop_at_steps(depth_steps)
                let px = self.ds_add(ox, self.ds_mul(dx, t))
                let py = self.ds_add(oy, self.ds_mul(dy, t))
                let pz = self.ds_add(oz, self.ds_mul(dz, t))
                let eval = self.calc_de(px, py, pz)
                var de = vec2(eval.y, eval.z)

                let max_de = self.ds_add(last_de, last_step)
                if self.ds_gt(de, max_de) {
                    de = max_de
                }

                if eval.x < self.scene.controls0.y && !self.ds_lt(de, current_stop) {
                    var step = self.ds_max(
                        self.ds_mul(
                            self.ds_mul(
                                self.ds_sub(de, self.ds_mul(self.scene.ms_de_sub, current_stop)),
                                self.scene.s_z_step_div
                            ),
                            rsfmul
                        ),
                        self.ds_mul(self.scene.step_width, self.ds_new(0.11))
                    )
                    let max_step_here = self.ds_mul(
                        self.ds_max(current_stop, self.ds_mul(self.scene.step_width, self.ds_new(0.4))),
                        self.scene.mct_mh04_zsd
                    )
                    if self.ds_lt(max_step_here, step) {
                        step = max_step_here
                    }

                    let de_eps = self.ds_add(de, self.ds_new(1.0e-30))
                    if self.ds_gt(last_de, de_eps) {
                        let denom = max(self.ds_to_f(self.ds_sub(last_de, de)), 1.0e-30)
                        let ratio = self.ds_to_f(last_step) / denom
                        if ratio < 1.0 {
                            rsfmul = self.ds_new(max(ratio, 0.5))
                        } else {
                            rsfmul = self.ds_new(1.0)
                        }
                    } else {
                        rsfmul = self.ds_new(1.0)
                    }

                    last_de = de
                    last_step = step
                    t = self.ds_add(t, step)

                    if self.ds_gt(t, self.scene.max_ray_length) {
                        return vec4(0.0, 0.0, 0.0, 0.0)
                    }
                } else {
                    return vec4(t.x, t.y, 0.0, 1.0)
                }
            }

            return vec4(0.0, 0.0, 0.0, 0.0)
        }

        calc_de_f: fn(px: vec2, py: vec2, pz: vec2) -> float {
            let eval = self.calc_de(px, py, pz)
            return eval.y + eval.z
        }

        estimate_normal: fn(px: vec2, py: vec2, pz: vec2, depth: vec2) -> vec3 {
            let step_width = max(self.ds_to_f(self.scene.step_width), 1.0e-30)
            let de_stop_header = self.ds_to_f(self.scene.de_stop_header)
            let de_stop_factor = self.ds_to_f(self.scene.de_stop_factor)
            let forward = self.safe_normalize3(vec3(
                self.ds_to_f(self.scene.cam_forward_x),
                self.ds_to_f(self.scene.cam_forward_y),
                self.ds_to_f(self.scene.cam_forward_z)
            ))
            let right = self.safe_normalize3(vec3(
                self.ds_to_f(self.scene.cam_right_x),
                self.ds_to_f(self.scene.cam_right_y),
                self.ds_to_f(self.scene.cam_right_z)
            ))
            let up = self.safe_normalize3(vec3(
                self.ds_to_f(self.scene.cam_up_x),
                self.ds_to_f(self.scene.cam_up_y),
                self.ds_to_f(self.scene.cam_up_z)
            ))

            let m_zz = self.ds_to_f(depth) / step_width
            let n_offset = min(de_stop_header, 1.0) * (1.0 + abs(m_zz) * de_stop_factor) * 0.15 * step_width

            let dz = self.calc_de_f(
                self.ds_add_f(px, forward.x * n_offset),
                self.ds_add_f(py, forward.y * n_offset),
                self.ds_add_f(pz, forward.z * n_offset)
            ) - self.calc_de_f(
                self.ds_add_f(px, -forward.x * n_offset),
                self.ds_add_f(py, -forward.y * n_offset),
                self.ds_add_f(pz, -forward.z * n_offset)
            )
            let dx = self.calc_de_f(
                self.ds_add_f(px, right.x * n_offset),
                self.ds_add_f(py, right.y * n_offset),
                self.ds_add_f(pz, right.z * n_offset)
            ) - self.calc_de_f(
                self.ds_add_f(px, -right.x * n_offset),
                self.ds_add_f(py, -right.y * n_offset),
                self.ds_add_f(pz, -right.z * n_offset)
            )
            let dy = self.calc_de_f(
                self.ds_add_f(px, up.x * n_offset),
                self.ds_add_f(py, up.y * n_offset),
                self.ds_add_f(pz, up.z * n_offset)
            ) - self.calc_de_f(
                self.ds_add_f(px, -up.x * n_offset),
                self.ds_add_f(py, -up.y * n_offset),
                self.ds_add_f(pz, -up.z * n_offset)
            )

            return self.safe_normalize3(
                right * dx +
                up * dy +
                forward * dz
            )
        }

        render_color: fn() -> vec3 {
            let dir = self.primary_dir()
            let frag = self.frag_coord()
            let half_w = self.rect_size.x * 0.5
            let half_h = self.rect_size.y * 0.5
            let x_offset = self.ds_mul(self.ds_new(frag.x - half_w), self.scene.step_width)
            let y_offset = self.ds_mul(self.ds_new(frag.y - half_h), self.scene.step_width)
            let march = self.primary_march()

            if march.w < 0.5 {
                return self.sky_for_y(dir.y * 0.5 + 0.5)
            }

            let depth = vec2(march.x, march.y)
            let ox = self.ds_add(
                self.ds_add(
                    self.ds_add(self.scene.mid_x, self.ds_mul(self.scene.cam_forward_x, self.scene.z_start_delta)),
                    self.ds_mul(self.scene.cam_right_x, x_offset)
                ),
                self.ds_mul(self.scene.cam_up_x, y_offset)
            )
            let oy = self.ds_add(
                self.ds_add(
                    self.ds_add(self.scene.mid_y, self.ds_mul(self.scene.cam_forward_y, self.scene.z_start_delta)),
                    self.ds_mul(self.scene.cam_right_y, x_offset)
                ),
                self.ds_mul(self.scene.cam_up_y, y_offset)
            )
            let oz = self.ds_add(
                self.ds_add(
                    self.ds_add(self.scene.mid_z, self.ds_mul(self.scene.cam_forward_z, self.scene.z_start_delta)),
                    self.ds_mul(self.scene.cam_right_z, x_offset)
                ),
                self.ds_mul(self.scene.cam_up_z, y_offset)
            )
            let dx = self.ds_new(dir.x)
            let dy = self.ds_new(dir.y)
            let dz = self.ds_new(dir.z)
            let px = self.ds_add(ox, self.ds_mul(dx, depth))
            let py = self.ds_add(oy, self.ds_mul(dy, depth))
            let pz = self.ds_add(oz, self.ds_mul(dz, depth))
            let normal = self.estimate_normal(px, py, pz, depth)
            let light_dir = self.safe_normalize3(self.scene.debug_light_dir)
            let diffuse = max(dot(normal, light_dir), 0.0)
            let base = normal * 0.5 + 0.5
            return base * (0.2 + 0.8 * diffuse)
        }

        pixel: fn() {
            return vec4(self.render_color(), 1.0)
        }
    }

    mod.widgets.GpuMb3dViewBase = #(GpuMb3dView::register_widget(vm))

    mod.widgets.GpuMb3dView = set_type_default() do mod.widgets.GpuMb3dViewBase{
        width: Fill
        height: Fill
        draw_gpu: mod.widgets.DrawGpuMb3d{}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 540)
                body +: {
                    flow: Overlay
                    fractal := mod.widgets.GpuMb3dView{}
                    RoundedView{
                        width: Fit
                        height: Fit
                        margin: Inset{left: 18 top: 18}
                        padding: Inset{left: 14 right: 14 top: 10 bottom: 10}
                        flow: Down
                        spacing: 4
                        draw_bg.color: #x10151dcc
                        draw_bg.radius: 8.0
                        Label{
                            text: "GPU MB3D cathedral"
                            draw_text.color: #xf2f2ef
                            draw_text.text_style.font_size: 14
                        }
                        Label{
                            text: "primary march + raw normals"
                            draw_text.color: #xa9b2bf
                            draw_text.text_style.font_size: 10
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Clone)]
struct AmazingUniforms {
    scale: f64,
    scale_div_min_r2: f64,
    min_r2: f64,
    fold: f64,
}

#[derive(Clone)]
struct MengerUniforms {
    scale: f64,
    cx: f64,
    cy: f64,
    cz: f64,
    rot: formulas::Mat3,
}

#[derive(Clone)]
struct CathedralScene {
    m3p: m3p::M3PFile,
    base_width: f64,
    formula0_iters: f32,
    formula1_iters: f32,
    repeat_from_slot: f32,
    amazing: AmazingUniforms,
    menger: MengerUniforms,
    debug_light_dir: Vec3f,
    sky_color: Vec4f,
    sky_color2: Vec4f,
}

#[derive(Clone, Copy, Default, Script, ScriptHook)]
#[repr(C, align(16))]
pub struct GpuMb3dUniforms {
    #[live]
    pub sky_color: Vec4f,
    #[live]
    pub sky_color2: Vec4f,
    #[live]
    pub debug_light_dir: Vec3f,
    #[live]
    pub debug_light_pad: f32,
    #[live]
    pub cam_right_x: Vec2f,
    #[live]
    pub cam_right_y: Vec2f,
    #[live]
    pub cam_right_z: Vec2f,
    #[live]
    pub cam_right_pad: Vec2f,
    #[live]
    pub cam_up_x: Vec2f,
    #[live]
    pub cam_up_y: Vec2f,
    #[live]
    pub cam_up_z: Vec2f,
    #[live]
    pub cam_up_pad: Vec2f,
    #[live]
    pub cam_forward_x: Vec2f,
    #[live]
    pub cam_forward_y: Vec2f,
    #[live]
    pub cam_forward_z: Vec2f,
    #[live]
    pub cam_forward_pad: Vec2f,
    #[live]
    pub rot0_x: Vec2f,
    #[live]
    pub rot0_y: Vec2f,
    #[live]
    pub rot0_z: Vec2f,
    #[live]
    pub rot0_pad: Vec2f,
    #[live]
    pub rot1_x: Vec2f,
    #[live]
    pub rot1_y: Vec2f,
    #[live]
    pub rot1_z: Vec2f,
    #[live]
    pub rot1_pad: Vec2f,
    #[live]
    pub rot2_x: Vec2f,
    #[live]
    pub rot2_y: Vec2f,
    #[live]
    pub rot2_z: Vec2f,
    #[live]
    pub rot2_pad: Vec2f,
    #[live]
    pub mid_x: Vec2f,
    #[live]
    pub mid_y: Vec2f,
    #[live]
    pub mid_z: Vec2f,
    #[live]
    pub mid_pad: Vec2f,
    #[live]
    pub julia_x: Vec2f,
    #[live]
    pub julia_y: Vec2f,
    #[live]
    pub julia_z: Vec2f,
    #[live]
    pub julia_pad: Vec2f,
    #[live]
    pub step_width: Vec2f,
    #[live]
    pub z_start_delta: Vec2f,
    #[live]
    pub max_ray_length: Vec2f,
    #[live]
    pub de_stop_header: Vec2f,
    #[live]
    pub de_stop: Vec2f,
    #[live]
    pub de_stop_factor: Vec2f,
    #[live]
    pub s_z_step_div: Vec2f,
    #[live]
    pub ms_de_sub: Vec2f,
    #[live]
    pub mct_mh04_zsd: Vec2f,
    #[live]
    pub de_floor: Vec2f,
    #[live]
    pub rstop: Vec2f,
    #[live]
    pub ab_scale: Vec2f,
    #[live]
    pub ab_scale_div_min_r2: Vec2f,
    #[live]
    pub ab_min_r2: Vec2f,
    #[live]
    pub ab_fold: Vec2f,
    #[live]
    pub menger_scale: Vec2f,
    #[live]
    pub menger_cx: Vec2f,
    #[live]
    pub menger_cy: Vec2f,
    #[live]
    pub menger_cz: Vec2f,
    #[live]
    pub controls0: Vec4f,
    #[live]
    pub controls1: Vec4f,
    #[live]
    pub tail_pad: Vec2f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGpuMb3d {
    #[rust]
    scene_uniforms: Option<UniformBuffer>,
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct GpuMb3dView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_gpu: DrawGpuMb3d,
    #[rust]
    area: Area,
    #[rust]
    scene: Option<CathedralScene>,
    #[rust]
    scene_error: Option<String>,
}

impl GpuMb3dView {
    fn ensure_scene_loaded(&mut self) {
        if self.scene.is_some() || self.scene_error.is_some() {
            return;
        }

        match load_cathedral_scene() {
            Ok(scene) => self.scene = Some(scene),
            Err(err) => {
                self.scene_error = Some(err.clone());
                log!("gpu_mb3d: {}", err);
            }
        }
    }

    fn configure_shader(&mut self, cx: &mut Cx2d, rect: Rect) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }

        let uniforms = build_uniforms(scene, rect.size.x.max(1.0) as usize);
        let uniform_buffer = self
            .draw_gpu
            .scene_uniforms
            .get_or_insert_with(|| UniformBuffer::new(cx.cx))
            .clone();
        uniform_buffer.set_struct(cx.cx, &uniforms);
        self.draw_gpu.draw_vars.set_uniform_buffer(0, &uniform_buffer);
    }
}

impl Widget for GpuMb3dView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_scene_loaded();

        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.configure_shader(cx, rect);
        self.draw_gpu.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

fn load_cathedral_scene() -> Result<CathedralScene, String> {
    let path = find_cathedral_path()?;
    let path_string = path.to_string_lossy().to_string();
    let m3p = m3p::parse(&path_string).map_err(|err| format!("failed to parse {path_string}: {err}"))?;

    let end_to = (m3p.addon.b_hyb_opt1 & 7) as usize;
    let repeat_from = (m3p.addon.b_hyb_opt1 >> 4) as usize;
    let mut repeat_from_slot = None;
    let mut active = Vec::new();

    for i in 0..=end_to.min(5) {
        let formula = &m3p.addon.formulas[i];
        if formula.iteration_count <= 0 {
            continue;
        }
        if repeat_from_slot.is_none() && i >= repeat_from {
            repeat_from_slot = Some(active.len());
        }
        active.push(formula.clone());
    }

    if active.len() != 2 {
        return Err(format!(
            "gpu_mb3d expects exactly 2 active formulas in cathedral.m3p, found {}",
            active.len()
        ));
    }

    let amazing_formula = &active[0];
    if amazing_formula.formula_nr != 4 {
        return Err(format!(
            "gpu_mb3d expects slot 0 to be AmazingBox (#4), found #{} '{}'",
            amazing_formula.formula_nr, amazing_formula.custom_name
        ));
    }

    let amazing = AmazingUniforms {
        scale: amazing_formula.option_values[0],
        scale_div_min_r2: amazing_formula.option_values[0]
            / (amazing_formula.option_values[1] * amazing_formula.option_values[1]).max(1.0e-40),
        min_r2: (amazing_formula.option_values[1] * amazing_formula.option_values[1]).max(1.0e-40),
        fold: amazing_formula.option_values[2],
    };

    let menger_formula = &active[1];
    if !(menger_formula.custom_name.contains("Menger") || menger_formula.formula_nr == 20) {
        return Err(format!(
            "gpu_mb3d expects slot 1 to be MengerIFS, found #{} '{}'",
            menger_formula.formula_nr, menger_formula.custom_name
        ));
    }

    let rot = if menger_formula.option_values[4] == 0.0
        && menger_formula.option_values[5] == 0.0
        && menger_formula.option_values[6] == 0.0
    {
        formulas::Mat3::identity()
    } else {
        let d2r = std::f64::consts::PI / 180.0;
        formulas::Mat3::from_euler(
            menger_formula.option_values[4] * d2r,
            menger_formula.option_values[5] * d2r,
            menger_formula.option_values[6] * d2r,
        )
    };

    let menger = MengerUniforms {
        scale: menger_formula.option_values[0],
        cx: menger_formula.option_values[1],
        cy: menger_formula.option_values[2],
        cz: menger_formula.option_values[3],
        rot,
    };

    let camera = render::Camera::from_m3p(&m3p);
    let debug_light_dir = select_primary_light(&m3p, &camera);

    Ok(CathedralScene {
        base_width: m3p.width as f64,
        formula0_iters: active[0].iteration_count as f32,
        formula1_iters: active[1].iteration_count as f32,
        repeat_from_slot: repeat_from_slot.unwrap_or(0) as f32,
        amazing,
        menger,
        debug_light_dir,
        sky_color: rgb4(m3p.lighting.depth_col),
        sky_color2: rgb4(m3p.lighting.depth_col2),
        m3p,
    })
}

fn find_cathedral_path() -> Result<PathBuf, String> {
    let relative = PathBuf::from("local/mb3d/cathedral.m3p");
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            let candidate = dir.join(&relative);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            let candidate = dir.join(&relative);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    for dir in PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors() {
        let candidate = dir.join(&relative);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "could not locate {} from current_dir, current_exe, or manifest dir {}",
        relative.display(),
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn build_uniforms(scene: &CathedralScene, width: usize) -> GpuMb3dUniforms {
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);

    let cam_right = params.camera.right.normalize();
    let cam_up = params.camera.up.normalize();
    let cam_forward = params.camera.forward.normalize();

    GpuMb3dUniforms {
        sky_color: scene.sky_color,
        sky_color2: scene.sky_color2,
        debug_light_dir: scene.debug_light_dir,
        debug_light_pad: 0.0,
        cam_right_x: split_f64(cam_right.x),
        cam_right_y: split_f64(cam_right.y),
        cam_right_z: split_f64(cam_right.z),
        cam_right_pad: Vec2f::default(),
        cam_up_x: split_f64(cam_up.x),
        cam_up_y: split_f64(cam_up.y),
        cam_up_z: split_f64(cam_up.z),
        cam_up_pad: Vec2f::default(),
        cam_forward_x: split_f64(cam_forward.x),
        cam_forward_y: split_f64(cam_forward.y),
        cam_forward_z: split_f64(cam_forward.z),
        cam_forward_pad: Vec2f::default(),
        rot0_x: split_f64(scene.menger.rot.m[0][0]),
        rot0_y: split_f64(scene.menger.rot.m[0][1]),
        rot0_z: split_f64(scene.menger.rot.m[0][2]),
        rot0_pad: Vec2f::default(),
        rot1_x: split_f64(scene.menger.rot.m[1][0]),
        rot1_y: split_f64(scene.menger.rot.m[1][1]),
        rot1_z: split_f64(scene.menger.rot.m[1][2]),
        rot1_pad: Vec2f::default(),
        rot2_x: split_f64(scene.menger.rot.m[2][0]),
        rot2_y: split_f64(scene.menger.rot.m[2][1]),
        rot2_z: split_f64(scene.menger.rot.m[2][2]),
        rot2_pad: Vec2f::default(),
        mid_x: split_f64(params.camera.mid.x),
        mid_y: split_f64(params.camera.mid.y),
        mid_z: split_f64(params.camera.mid.z),
        mid_pad: Vec2f::default(),
        julia_x: split_f64(params.iter_params.julia_x),
        julia_y: split_f64(params.iter_params.julia_y),
        julia_z: split_f64(params.iter_params.julia_z),
        julia_pad: Vec2f::default(),
        step_width: split_f64(params.step_width),
        z_start_delta: split_f64(params.camera.z_start - params.camera.mid.z),
        max_ray_length: split_f64(params.max_ray_length),
        de_stop_header: split_f64(params.de_stop_header),
        de_stop: split_f64(params.de_stop),
        de_stop_factor: split_f64(params.de_stop_factor),
        s_z_step_div: split_f64(params.s_z_step_div),
        ms_de_sub: split_f64(params.ms_de_sub),
        mct_mh04_zsd: split_f64(params.mct_mh04_zsd),
        de_floor: split_f64(params.de_floor),
        rstop: split_f64(params.iter_params.rstop),
        ab_scale: split_f64(scene.amazing.scale),
        ab_scale_div_min_r2: split_f64(scene.amazing.scale_div_min_r2),
        ab_min_r2: split_f64(scene.amazing.min_r2),
        ab_fold: split_f64(scene.amazing.fold),
        menger_scale: split_f64(scene.menger.scale),
        menger_cx: split_f64(scene.menger.cx),
        menger_cy: split_f64(scene.menger.cy),
        menger_cz: split_f64(scene.menger.cz),
        controls0: Vec4f {
            x: params.camera.fov_y as f32,
            y: params.iter_params.max_iters as f32,
            z: scene.formula0_iters,
            w: scene.formula1_iters,
        },
        controls1: Vec4f {
            x: scene.repeat_from_slot,
            y: if params.iter_params.is_julia { 1.0 } else { 0.0 },
            z: 0.0,
            w: 0.0,
        },
        tail_pad: Vec2f::default(),
    }
}

fn select_primary_light(m3p: &m3p::M3PFile, camera: &render::Camera) -> Vec3f {
    for light in &m3p.lighting.lights {
        let mut opt = (light.l_option & 3) as i32;
        if opt == 3 {
            opt = 1;
        }
        if opt != 0 || light.l_amp <= 0.0 {
            continue;
        }

        let local_dir = render::Vec3::new(
            -light.angle_xy.sin(),
            -light.angle_z.sin(),
            -(light.angle_xy.cos() * light.angle_z.cos()),
        )
        .normalize();
        let r = camera.right.normalize();
        let u = camera.up.normalize();
        let f = camera.forward.normalize();
        let dir = r
            .scale(local_dir.x)
            .add(u.scale(local_dir.y))
            .add(f.scale(local_dir.z))
            .normalize();

        return vec3f(dir.x as f32, dir.y as f32, dir.z as f32);
    }

    vec3f(-0.35, 0.8, 0.45)
}

fn rgb4(color: [u8; 3]) -> Vec4f {
    Vec4f {
        x: color[0] as f32 / 255.0,
        y: color[1] as f32 / 255.0,
        z: color[2] as f32 / 255.0,
        w: 1.0,
    }
}

fn split_f64(value: f64) -> Vec2f {
    let hi = value as f32;
    Vec2f {
        x: hi,
        y: (value - hi as f64) as f32,
    }
}
