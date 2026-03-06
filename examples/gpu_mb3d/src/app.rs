use makepad_mb3d_render::{formulas, m3p, render};
use makepad_widgets::*;
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawGpuMb3d = set_type_default() do #(DrawGpuMb3d::script_shader(vm)){
        ..mod.draw.DrawQuad

        bg_color: #x0d1014
        sky_color: #x535f73
        sky_color2: #xa8b4c4
        surface_color: #xb2b1ab
        surface_color2: #x7f7f79
        light_color: #xddd8cf
        amb_top: #x8f8b82
        amb_bottom: #x1b1c20

        cam_right: vec3(1.0, 0.0, 0.0)
        cam_up: vec3(0.0, 1.0, 0.0)
        cam_forward: vec3(0.0, 0.0, 1.0)
        light_dir: vec3(-0.35, 0.8, 0.45)
        rot0: vec3(1.0, 0.0, 0.0)
        rot1: vec3(0.0, 1.0, 0.0)
        rot2: vec3(0.0, 0.0, 1.0)

        mid_x: vec2(0.0, 0.0)
        mid_y: vec2(0.0, 0.0)
        mid_z: vec2(0.0, 0.0)

        fov_y: 45.0
        step_width: 0.001
        z_start_delta: 0.0
        max_ray_length: 128.0
        de_stop: 0.001
        de_stop_factor: 0.0
        s_z_step_div: 1.0
        ms_de_sub: 1.0
        mct_mh04_zsd: 1.0
        de_floor: 0.00025
        rstop: 20.0
        max_iters: 48.0
        slot0_iters: 1.0
        slot1_iters: 1.0
        repeat_from_slot: 0.0

        ab_scale: -1.0
        ab_scale_div_min_r2: -1.0
        ab_min_r2: 0.25
        ab_fold: 1.0

        menger_scale: 3.0
        menger_cx: 1.0
        menger_cy: 1.0
        menger_cz: 0.5

        ds_make: fn(v) {
            return vec2(v, 0.0)
        }

        ds_norm: fn(v) {
            let s = v.x + v.y
            let e = v.y - (s - v.x)
            return vec2(s, e)
        }

        ds_add: fn(a, b) {
            let s = a.x + b.x
            let bb = s - a.x
            let e = (a.x - (s - bb)) + (b.x - bb) + a.y + b.y
            return self.ds_norm(vec2(s, e))
        }

        ds_sub: fn(a, b) {
            return self.ds_add(a, vec2(-b.x, -b.y))
        }

        ds_add_f: fn(a, b) {
            return self.ds_add(a, vec2(b, 0.0))
        }

        ds_mul_f: fn(a, b) {
            return self.ds_norm(vec2(a.x * b, a.y * b))
        }

        ds_mul: fn(a, b) {
            let p = a.x * b.x
            let e = a.x * b.y + a.y * b.x + a.y * b.y
            return self.ds_norm(vec2(p, e))
        }

        ds_div: fn(a, b) {
            let q1 = a.x / b.x
            let r = self.ds_sub(a, self.ds_mul_f(b, q1))
            let q2 = r.x / b.x
            return self.ds_norm(vec2(q1, q2))
        }

        ds_abs: fn(a) {
            if a.x < 0.0 || (a.x == 0.0 && a.y < 0.0) {
                return vec2(-a.x, -a.y)
            }
            return a
        }

        ds_box_fold: fn(a, fold) {
            let plus_abs = self.ds_abs(self.ds_add_f(a, fold))
            let minus_abs = self.ds_abs(self.ds_add_f(a, -fold))
            return self.ds_sub(self.ds_sub(plus_abs, minus_abs), a)
        }

        ds_to_f: fn(a) {
            return a.x + a.y
        }

        ds_lt_f: fn(a, b) {
            return self.ds_to_f(a) < b
        }

        ds_sqrt: fn(a) {
            let root = sqrt(max(self.ds_to_f(a), 0.0))
            return vec2(root, 0.0)
        }

        safe_normalize3: fn(v) {
            let len = max(length(v), 0.000001)
            return v / len
        }

        sky_for_y: fn(y) {
            let t = clamp(pow(1.0 - y, 0.7), 0.0, 1.0)
            return mix(self.sky_color.rgb, self.sky_color2.rgb, t)
        }

        hybrid_de: fn(px, py, pz) {
            let cx = px
            let cy = py
            let cz = pz
            var x = px
            var y = py
            var z = pz
            var w = vec2(1.0, 0.0)
            var r2 = vec2(0.0, 0.0)
            var iters = 0.0
            var slot = 0.0
            var remaining = self.slot0_iters

            for i in 0..128 {
                if remaining <= 0.0 {
                    slot += 1.0
                    if slot >= 2.0 {
                        slot = self.repeat_from_slot
                    }
                    if slot < 0.5 {
                        remaining = self.slot0_iters
                    } else {
                        remaining = self.slot1_iters
                    }
                }

                if slot < 0.5 {
                    x = self.ds_box_fold(x, self.ab_fold)
                    y = self.ds_box_fold(y, self.ab_fold)
                    z = self.ds_box_fold(z, self.ab_fold)

                    let rr = self.ds_to_f(self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z)))
                    var m = self.ab_scale
                    if rr < self.ab_min_r2 {
                        m = self.ab_scale_div_min_r2
                    } else if rr < 1.0 {
                        m = self.ab_scale / max(rr, 0.0000001)
                    }

                    w = self.ds_mul_f(w, m)
                    x = self.ds_add(self.ds_mul_f(x, m), cx)
                    y = self.ds_add(self.ds_mul_f(y, m), cy)
                    z = self.ds_add(self.ds_mul_f(z, m), cz)
                } else {
                    x = self.ds_abs(x)
                    y = self.ds_abs(y)
                    z = self.ds_abs(z)

                    if self.ds_to_f(x) < self.ds_to_f(y) {
                        let t = x
                        x = y
                        y = t
                    }
                    if self.ds_to_f(x) < self.ds_to_f(z) {
                        let t = x
                        x = z
                        z = t
                    }
                    if self.ds_to_f(y) < self.ds_to_f(z) {
                        let t = y
                        y = z
                        z = t
                    }

                    let nx = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot0.x), self.ds_mul_f(y, self.rot0.y)), self.ds_mul_f(z, self.rot0.z))
                    let ny = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot1.x), self.ds_mul_f(y, self.rot1.y)), self.ds_mul_f(z, self.rot1.z))
                    let nz = self.ds_add(self.ds_add(self.ds_mul_f(x, self.rot2.x), self.ds_mul_f(y, self.rot2.y)), self.ds_mul_f(z, self.rot2.z))

                    let sf = self.menger_scale - 1.0
                    x = self.ds_add_f(self.ds_mul_f(nx, self.menger_scale), -self.menger_cx * sf)
                    y = self.ds_add_f(self.ds_mul_f(ny, self.menger_scale), -self.menger_cy * sf)

                    let z_scaled = self.ds_mul_f(nz, self.menger_scale)
                    let c = self.menger_cz * sf
                    z = self.ds_add_f(self.ds_abs(self.ds_add_f(z_scaled, -c)), -c)
                    z = vec2(-z.x, -z.y)

                    w = self.ds_mul_f(w, self.menger_scale)
                }

                iters += 1.0
                remaining -= 1.0

                r2 = self.ds_add(self.ds_add(self.ds_mul(x, x), self.ds_mul(y, y)), self.ds_mul(z, z))
                if self.ds_to_f(r2) > self.rstop || iters >= self.max_iters {
                    break
                }
            }

            let r = self.ds_sqrt(r2)
            let de = self.ds_div(r, self.ds_abs(w))
            return vec3(iters, de.x, de.y)
        }

        calc_de: fn(px, py, pz) {
            let raw = self.hybrid_de(px, py, pz)
            let de_raw = max(raw.y + raw.z, self.de_floor)
            return vec2(raw.x, de_raw)
        }

        pos_x: fn(ox, dir, t) {
            return self.ds_add(ox, self.ds_mul_f(self.ds_make(t), dir.x))
        }

        pos_y: fn(oy, dir, t) {
            return self.ds_add(oy, self.ds_mul_f(self.ds_make(t), dir.y))
        }

        pos_z: fn(oz, dir, t) {
            return self.ds_add(oz, self.ds_mul_f(self.ds_make(t), dir.z))
        }

        ray_march: fn(ox, oy, oz, dir) {
            var t = 0.0
            var last_de = 0.0
            var last_step = 0.0
            var rsfmul = 1.0

            let first_eval = self.calc_de(ox, oy, oz)
            let first_destop = self.de_stop
            if first_eval.x >= self.max_iters || first_eval.y < first_destop {
                return vec2(0.0, first_eval.x)
            }

            last_de = first_eval.y
            last_step = max(first_eval.y * self.s_z_step_div, 0.11 * self.step_width)

            for step_idx in 0..128 {
                let depth_steps = abs(t) / max(self.step_width, 0.0000001)
                let current_destop = self.de_stop * (1.0 + depth_steps * self.de_stop_factor)

                let px = self.pos_x(ox, dir, t)
                let py = self.pos_y(oy, dir, t)
                let pz = self.pos_z(oz, dir, t)
                let eval = self.calc_de(px, py, pz)
                var de = eval.y
                if de > last_de + last_step {
                    de = last_de + last_step
                }

                if eval.x < self.max_iters && de >= current_destop {
                    var step = max((de - self.ms_de_sub * current_destop) * self.s_z_step_div * rsfmul, 0.11 * self.step_width)
                    let max_step_here = max(current_destop, 0.4 * self.step_width) * self.mct_mh04_zsd
                    if max_step_here < step {
                        step = max_step_here
                    }

                    if last_de > de + 0.0000001 {
                        let ratio = last_step / max(last_de - de, 0.0000001)
                        if ratio < 1.0 {
                            rsfmul = max(ratio, 0.5)
                        } else {
                            rsfmul = 1.0
                        }
                    } else {
                        rsfmul = 1.0
                    }

                    last_de = de
                    last_step = step
                    t += step
                    if t > self.max_ray_length {
                        return vec2(-1.0, 0.0)
                    }
                } else {
                    var refine_t = t
                    var refine_step = -0.5 * last_step
                    for i in 0..8 {
                        refine_t += refine_step
                        let rx = self.pos_x(ox, dir, refine_t)
                        let ry = self.pos_y(oy, dir, refine_t)
                        let rz = self.pos_z(oz, dir, refine_t)
                        let depth_steps = abs(refine_t) / max(self.step_width, 0.0000001)
                        let stop_here = self.de_stop * (1.0 + depth_steps * self.de_stop_factor)
                        let reval = self.calc_de(rx, ry, rz)
                        if reval.x >= self.max_iters || reval.y < stop_here {
                            refine_step = -abs(refine_step) * 0.55
                        } else {
                            refine_step = abs(refine_step) * 0.55
                        }
                    }
                    let fx = self.pos_x(ox, dir, refine_t)
                    let fy = self.pos_y(oy, dir, refine_t)
                    let fz = self.pos_z(oz, dir, refine_t)
                    let final_eval = self.calc_de(fx, fy, fz)
                    return vec2(refine_t, final_eval.x)
                }
            }

            return vec2(-1.0, 0.0)
        }

        de_only_f: fn(px, py, pz) {
            let eval = self.calc_de(px, py, pz)
            return eval.y
        }

        estimate_normal: fn(px, py, pz) {
            let eps = max(self.de_stop * 6.0, self.step_width * 0.8)
            let d1 = self.de_only_f(self.ds_add_f(px, eps), self.ds_add_f(py, -eps), self.ds_add_f(pz, -eps))
            let d2 = self.de_only_f(self.ds_add_f(px, -eps), self.ds_add_f(py, -eps), self.ds_add_f(pz, eps))
            let d3 = self.de_only_f(self.ds_add_f(px, -eps), self.ds_add_f(py, eps), self.ds_add_f(pz, -eps))
            let d4 = self.de_only_f(self.ds_add_f(px, eps), self.ds_add_f(py, eps), self.ds_add_f(pz, eps))
            let n = vec3(
                d1 - d2 - d3 + d4,
                -d1 - d2 + d3 + d4,
                -d1 + d2 - d3 + d4
            )
            return self.safe_normalize3(n)
        }

        shade_hit: fn(dir, depth, iters, px, py, pz) {
            let n = self.estimate_normal(px, py, pz)
            let l = self.safe_normalize3(self.light_dir)
            let v = self.safe_normalize3(-dir)
            let h = self.safe_normalize3(l + v)

            let ndotl = max(dot(n, l), 0.0)
            let ndoth = max(dot(n, h), 0.0)
            let hemi = mix(self.amb_bottom.rgb, self.amb_top.rgb, clamp(n.y * 0.5 + 0.5, 0.0, 1.0))
            let iter_t = clamp(iters / max(self.max_iters, 1.0), 0.0, 1.0)
            let stone = mix(self.surface_color2.rgb, self.surface_color.rgb, pow(1.0 - iter_t, 0.6))
            let lit = stone * (hemi * 0.9 + self.light_color.rgb * (0.18 + 0.82 * ndotl))
            let spec = self.light_color.rgb * pow(ndoth, 28.0) * 0.16
            let fog_y = clamp(self.pos.y, 0.0, 1.0)
            let fog = mix(self.sky_color.rgb, self.sky_color2.rgb, pow(1.0 - fog_y, 0.65))
            let fog_t = clamp(depth / max(self.max_ray_length, 0.001), 0.0, 1.0)
            return mix(lit + spec, fog, fog_t * fog_t * 0.8)
        }

        render_color: fn() {
            let frag = self.pos * self.rect_size
            let half_w = self.rect_size.x * 0.5
            let half_h = self.rect_size.y * 0.5
            let fov_mul = (self.fov_y * 0.017453292519943295) / max(self.rect_size.y, 1.0)

            let cafx = (half_w - frag.x) * fov_mul
            let cafy = (frag.y - half_h) * fov_mul
            let sx = sin(cafx)
            let cx = cos(cafx)
            let sy = sin(cafy)
            let cy = cos(cafy)

            let local_dir = self.safe_normalize3(vec3(-sx, sy, cx * cy))
            let dir = self.safe_normalize3(
                self.cam_right * local_dir.x +
                self.cam_up * local_dir.y +
                self.cam_forward * local_dir.z
            )

            let x_offset = (frag.x - half_w) * self.step_width
            let y_offset = (frag.y - half_h) * self.step_width

            let ox = self.ds_add_f(self.ds_add_f(self.ds_add_f(self.mid_x, self.cam_forward.x * self.z_start_delta), self.cam_right.x * x_offset), self.cam_up.x * y_offset)
            let oy = self.ds_add_f(self.ds_add_f(self.ds_add_f(self.mid_y, self.cam_forward.y * self.z_start_delta), self.cam_right.y * x_offset), self.cam_up.y * y_offset)
            let oz = self.ds_add_f(self.ds_add_f(self.ds_add_f(self.mid_z, self.cam_forward.z * self.z_start_delta), self.cam_right.z * x_offset), self.cam_up.z * y_offset)

            let hit = self.ray_march(ox, oy, oz, dir)
            if hit.x < 0.0 {
                return self.sky_for_y(self.pos.y)
            }

            let px = self.pos_x(ox, dir, hit.x)
            let py = self.pos_y(oy, dir, hit.x)
            let pz = self.pos_z(oz, dir, hit.x)
            return self.shade_hit(dir, hit.x, hit.y, px, py, pz)
        }

        pixel: fn() {
            return vec4(self.render_color(), 1.0)
        }
    }

    mod.widgets.GpuMb3dViewBase = #(GpuMb3dView::register_widget(vm))

    mod.widgets.GpuMb3dView = set_type_default() do mod.widgets.GpuMb3dViewBase{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 540)
                body +: {
                    flow: Overlay
                    fractal := GpuMb3dView{}
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
                            text: "double-single fragment shader experiment"
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
    scale: f32,
    scale_div_min_r2: f32,
    min_r2: f32,
    fold: f32,
}

#[derive(Clone)]
struct MengerUniforms {
    scale: f32,
    cx: f32,
    cy: f32,
    cz: f32,
    rot0: Vec3f,
    rot1: Vec3f,
    rot2: Vec3f,
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
    light_dir: Vec3f,
    light_color: Vec4f,
    ambient_top: Vec4f,
    ambient_bottom: Vec4f,
    sky_color: Vec4f,
    sky_color2: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGpuMb3d {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub bg_color: Vec4f,
    #[live]
    pub sky_color: Vec4f,
    #[live]
    pub sky_color2: Vec4f,
    #[live]
    pub surface_color: Vec4f,
    #[live]
    pub surface_color2: Vec4f,
    #[live]
    pub light_color: Vec4f,
    #[live]
    pub amb_top: Vec4f,
    #[live]
    pub amb_bottom: Vec4f,
    #[live]
    pub cam_right: Vec3f,
    #[live]
    pub cam_up: Vec3f,
    #[live]
    pub cam_forward: Vec3f,
    #[live]
    pub light_dir: Vec3f,
    #[live]
    pub rot0: Vec3f,
    #[live]
    pub rot1: Vec3f,
    #[live]
    pub rot2: Vec3f,
    #[live]
    pub mid_x: Vec2f,
    #[live]
    pub mid_y: Vec2f,
    #[live]
    pub mid_z: Vec2f,
    #[live]
    pub fov_y: f32,
    #[live]
    pub step_width: f32,
    #[live]
    pub z_start_delta: f32,
    #[live]
    pub max_ray_length: f32,
    #[live]
    pub de_stop: f32,
    #[live]
    pub de_stop_factor: f32,
    #[live]
    pub s_z_step_div: f32,
    #[live]
    pub ms_de_sub: f32,
    #[live]
    pub mct_mh04_zsd: f32,
    #[live]
    pub de_floor: f32,
    #[live]
    pub rstop: f32,
    #[live]
    pub max_iters: f32,
    #[live]
    pub slot0_iters: f32,
    #[live]
    pub slot1_iters: f32,
    #[live]
    pub repeat_from_slot: f32,
    #[live]
    pub ab_scale: f32,
    #[live]
    pub ab_scale_div_min_r2: f32,
    #[live]
    pub ab_min_r2: f32,
    #[live]
    pub ab_fold: f32,
    #[live]
    pub menger_scale: f32,
    #[live]
    pub menger_cx: f32,
    #[live]
    pub menger_cy: f32,
    #[live]
    pub menger_cz: f32,
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

    fn configure_shader(&mut self, rect: Rect) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }

        let scale = (rect.size.x / scene.base_width).max(0.001);
        let mut params = render::RenderParams::from_m3p(&scene.m3p);
        params.apply_image_scale(scale);

        let inv_step = 1.0 / params.step_width.max(1.0e-30);
        self.draw_gpu.cam_right = vec3f(
            (params.camera.right.x * inv_step) as f32,
            (params.camera.right.y * inv_step) as f32,
            (params.camera.right.z * inv_step) as f32,
        );
        self.draw_gpu.cam_up = vec3f(
            (params.camera.up.x * inv_step) as f32,
            (params.camera.up.y * inv_step) as f32,
            (params.camera.up.z * inv_step) as f32,
        );
        self.draw_gpu.cam_forward = vec3f(
            (params.camera.forward.x * inv_step) as f32,
            (params.camera.forward.y * inv_step) as f32,
            (params.camera.forward.z * inv_step) as f32,
        );

        self.draw_gpu.mid_x = split_f64(params.camera.mid.x);
        self.draw_gpu.mid_y = split_f64(params.camera.mid.y);
        self.draw_gpu.mid_z = split_f64(params.camera.mid.z);
        self.draw_gpu.fov_y = params.camera.fov_y as f32;
        self.draw_gpu.step_width = params.step_width as f32;
        self.draw_gpu.z_start_delta = (params.camera.z_start - params.camera.mid.z) as f32;
        self.draw_gpu.max_ray_length = params.max_ray_length as f32;
        self.draw_gpu.de_stop = params.de_stop as f32;
        self.draw_gpu.de_stop_factor = params.de_stop_factor as f32;
        self.draw_gpu.s_z_step_div = params.s_z_step_div as f32;
        self.draw_gpu.ms_de_sub = params.ms_de_sub as f32;
        self.draw_gpu.mct_mh04_zsd = params.mct_mh04_zsd as f32;
        self.draw_gpu.de_floor = params.de_floor as f32;
        self.draw_gpu.rstop = params.iter_params.rstop as f32;
        self.draw_gpu.max_iters = params.iter_params.max_iters as f32;

        self.draw_gpu.slot0_iters = scene.formula0_iters;
        self.draw_gpu.slot1_iters = scene.formula1_iters;
        self.draw_gpu.repeat_from_slot = scene.repeat_from_slot;

        self.draw_gpu.ab_scale = scene.amazing.scale;
        self.draw_gpu.ab_scale_div_min_r2 = scene.amazing.scale_div_min_r2;
        self.draw_gpu.ab_min_r2 = scene.amazing.min_r2;
        self.draw_gpu.ab_fold = scene.amazing.fold;

        self.draw_gpu.menger_scale = scene.menger.scale;
        self.draw_gpu.menger_cx = scene.menger.cx;
        self.draw_gpu.menger_cy = scene.menger.cy;
        self.draw_gpu.menger_cz = scene.menger.cz;
        self.draw_gpu.rot0 = scene.menger.rot0;
        self.draw_gpu.rot1 = scene.menger.rot1;
        self.draw_gpu.rot2 = scene.menger.rot2;

        self.draw_gpu.light_dir = scene.light_dir;
        self.draw_gpu.light_color = scene.light_color;
        self.draw_gpu.amb_top = scene.ambient_top;
        self.draw_gpu.amb_bottom = scene.ambient_bottom;
        self.draw_gpu.sky_color = scene.sky_color;
        self.draw_gpu.sky_color2 = scene.sky_color2;
    }
}

impl Widget for GpuMb3dView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_scene_loaded();

        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.configure_shader(rect);
        self.draw_gpu.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

fn load_cathedral_scene() -> Result<CathedralScene, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/mb3d/cathedral.m3p");
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
        return Err(format!("gpu_mb3d expects exactly 2 active formulas in cathedral.m3p, found {}", active.len()));
    }

    let amazing_formula = &active[0];
    if amazing_formula.formula_nr != 4 {
        return Err(format!(
            "gpu_mb3d expects slot 0 to be AmazingBox (#4), found #{} '{}'",
            amazing_formula.formula_nr, amazing_formula.custom_name
        ));
    }

    let amazing = AmazingUniforms {
        scale: amazing_formula.option_values[0] as f32,
        scale_div_min_r2: (amazing_formula.option_values[0] / (amazing_formula.option_values[1] * amazing_formula.option_values[1]).max(1.0e-40)) as f32,
        min_r2: (amazing_formula.option_values[1] * amazing_formula.option_values[1]).max(1.0e-40) as f32,
        fold: amazing_formula.option_values[2] as f32,
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
        scale: menger_formula.option_values[0] as f32,
        cx: menger_formula.option_values[1] as f32,
        cy: menger_formula.option_values[2] as f32,
        cz: menger_formula.option_values[3] as f32,
        rot0: vec3f(rot.m[0][0] as f32, rot.m[0][1] as f32, rot.m[0][2] as f32),
        rot1: vec3f(rot.m[1][0] as f32, rot.m[1][1] as f32, rot.m[1][2] as f32),
        rot2: vec3f(rot.m[2][0] as f32, rot.m[2][1] as f32, rot.m[2][2] as f32),
    };

    let camera = render::Camera::from_m3p(&m3p);
    let light = select_primary_light(&m3p, &camera);

    Ok(CathedralScene {
        base_width: m3p.width as f64,
        formula0_iters: active[0].iteration_count as f32,
        formula1_iters: active[1].iteration_count as f32,
        repeat_from_slot: repeat_from_slot.unwrap_or(0) as f32,
        amazing,
        menger,
        light_dir: light.0,
        light_color: light.1,
        ambient_top: rgb4(m3p.lighting.ambient_top),
        ambient_bottom: rgb4(m3p.lighting.ambient_bottom),
        sky_color: rgb4(m3p.lighting.depth_col),
        sky_color2: rgb4(m3p.lighting.depth_col2),
        m3p,
    })
}

fn select_primary_light(m3p: &m3p::M3PFile, camera: &render::Camera) -> (Vec3f, Vec4f) {
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

        let lamp_mul = if ((light.l_option >> 2) & 1) != 0 {
            light.l_amp * 1.3
        } else {
            light.l_amp
        } as f32;

        let color = Vec4f {
            x: (light.color[0] as f32 / 255.0) * lamp_mul,
            y: (light.color[1] as f32 / 255.0) * lamp_mul,
            z: (light.color[2] as f32 / 255.0) * lamp_mul,
            w: 1.0,
        };

        return (
            vec3f(dir.x as f32, dir.y as f32, dir.z as f32),
            color,
        );
    }

    (
        vec3f(-0.35, 0.8, 0.45),
        Vec4f {
            x: 0.85,
            y: 0.82,
            z: 0.78,
            w: 1.0,
        },
    )
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
