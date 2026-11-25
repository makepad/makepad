//! Interactive UF850 workcell. The camera, the measured joints and the taught
//! volumes all share the robot base frame. Geometry is uploaded once per scene.
use crate::kinematics::{frames, Frame, Point};
use makepad_gltf::{decode_mesh_primitive, load_gltf_from_bytes};
use makepad_widgets::*;

type Zone = (bool, Point, Point);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.UfSurface = mod.std.set_type_default() do #(UfSurface::script_shader(vm)){
        alpha_blend: false
        backface_culling: false
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.IcoVertex, geom.IcoGeom)
        world: varying(vec3f)
        normal: varying(vec3f)

        vertex: fn() {
            let p = self.model * self.geom.pos
            self.world = p.xyz
            self.normal = (self.model * self.geom.normal).xyz
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * p)
        }

        grid_line: fn(p: vec2) -> float {
            let delta = abs(fract(p + vec2(0.5)) - vec2(0.5))
            let aa = max(abs(dFdx(p)) + abs(dFdy(p)), vec2(0.0001))
            let d = delta / aa
            return 1.0 - smoothstep(0.0, 1.0, min(d.x, d.y))
        }

        shade_segment: fn(p: vec2, a: vec2, b: vec2, width: float) -> float {
            let ab = b-a
            let h = clamp(dot(p-a,ab)/max(dot(ab,ab),0.00001),0.0,1.0)
            let d = p - mix(a,b,h)
            return exp(-dot(d,d)/(width*width))
        }
        pixel: fn() {
            let p = self.world
            if self.finish > 2.5 {
                let sky = vec3(0.055,0.082,0.102)
                let light_pool = exp(-dot(p.xy-vec2(0.21,-0.20),p.xy-vec2(0.21,-0.20))*1.65)
                let ground = vec3(0.117,0.150,0.173) + vec3(0.043,0.049,0.051)*light_pool
                let grid = self.grid_line(p.xy*10.0)
                let major = self.grid_line(p.xy*2.0)
                let contact = exp(-dot(p.xy,p.xy)*55.0)
                let shoulder = self.shadow_shoulder.xy
                let elbow = self.shadow_elbow.xy
                let wrist = self.shadow_wrist.xy
                let tip = self.shadow_flange.xy
                let base_s = self.shade_segment(p.xy,vec2(0.0),shoulder,0.095)
                let arm_s = self.shade_segment(p.xy,shoulder,elbow,0.125)
                let wrist_s = self.shade_segment(p.xy,elbow,wrist,0.135)
                let end_s = self.shade_segment(p.xy,wrist,tip,0.09)
                let cast = max(max(base_s*0.39,arm_s*0.31),max(wrist_s*0.26,end_s*0.23))
                let lit = (ground + vec3(0.055,0.080,0.095)*(grid*0.52+major*0.60))*(1.0-max(contact*0.62,cast))
                return vec4(mix(sky,lit,exp(-dot(p.xy,p.xy)*0.29)),1.0)
            }
            if self.finish > 1.5 {
                return vec4(self.color.xyz,1.0)
            }
            let n = normalize(self.normal)
            let eye = (self.draw_pass.camera_inv * vec4(0.0,0.0,0.0,1.0)).xyz
            let v = normalize(eye-p)
            let l = normalize(vec3(-0.53,-0.64,0.85))
            let h = normalize(l+v)
            let nv = clamp(dot(n,v),0.0,1.0)
            let rim = pow(1.0-nv,3.5)
            let r = 2.0*dot(v,n)*n-v
            // A large warm softbox, a vertical strip and a high cool kicker
            // reflected in the curved covers. Room reflections move with the camera.
            let main = smoothstep(0.61,0.90,dot(r,normalize(vec3(-0.42,-0.70,0.68))))
            let strip = (1.0-smoothstep(0.08,0.25,abs(r.x-0.45)))*smoothstep(-0.48,0.26,r.z)
            let kick = smoothstep(0.69,0.96,dot(r,normalize(vec3(0.36,0.83,0.56))))
            let reflection = vec3(0.93,0.87,0.76)*main + vec3(0.70,0.82,0.92)*strip + vec3(0.82,0.93,1.0)*kick
            let key = max(dot(n,l),0.0)
            let fill = max(dot(n,normalize(vec3(0.80,0.18,0.68))),0.0)
            let diffuse = self.color.xyz*(0.40 + 0.43*key + 0.15*fill + 0.07*n.z)
            let enamel = diffuse + reflection*(0.17+0.26*rim) + vec3(0.095)*pow(max(dot(n,h),0.0),38.0)
            let room = vec3(0.15,0.18,0.21) + vec3(0.13,0.15,0.16)*(0.50+0.50*r.z)
            let metal = self.color.xyz*0.26 + room*(0.43+0.26*key) + reflection*0.58 + vec3(0.12,0.16,0.19)*rim
            return vec4(mix(enamel,metal,self.finish),1.0)
        }
        fragment: fn(){ self.fb0 = self.pixel() }
    }
    mod.widgets.RobotScene = set_type_default() do #(RobotScene::register_widget(vm)){
        width: Fill
        height: Fill
        draw_surface: mod.draw.UfSurface{}
        draw_image +: {
            image: texture_2d(float)
            pixel: fn(){
                let dx = dFdx(self.pos)*0.23
                let dy = dFdy(self.pos)*0.23
                return (self.image.sample(self.pos+dx+dy)+self.image.sample(self.pos+dx-dy)+self.image.sample(self.pos-dx+dy)+self.image.sample(self.pos-dx-dy))*0.25
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct UfSurface {
    #[deref]
    draw_vars: DrawVars,
    #[live]
    model: Mat4f,
    #[live]
    color: Vec4f,
    #[live]
    finish: f32,
    #[live]
    shadow_shoulder: Vec4f,
    #[live]
    shadow_elbow: Vec4f,
    #[live]
    shadow_wrist: Vec4f,
    #[live]
    shadow_flange: Vec4f,
}

impl UfSurface {
    fn draw(
        &mut self,
        cx: &mut Cx2d,
        geometry: &Geometry,
        model: Mat4f,
        color: Vec4f,
        finish: f32,
    ) {
        self.model = model;
        self.color = color;
        self.finish = finish;
        self.draw_vars.geometry_id = Some(geometry.geometry_id());
        if self.draw_vars.can_instance() {
            let area = cx.add_instance(&mut self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, area);
        }
    }
}

struct LinkPart {
    link: usize,
    metal: bool,
    geometry: Geometry,
}

// Assets from xArm-Developer/xarm_ros cf51617d98bc55a88d9aa735acfa43a0bbb8e425,
// xarm_description/meshes/uf850/visual; base and flange use vendor STL and
// links 1–5 use DAE material groups. See resources/UF850-LICENSE.txt.
fn load_arm(cx: &mut Cx) -> Vec<LinkPart> {
    let loaded = load_gltf_from_bytes(include_bytes!("../resources/uf850.glb"), None)
        .expect("embedded UF850 model");
    let mut parts = Vec::new();
    for (link, mesh) in loaded.document.meshes_slice().iter().enumerate() {
        for part in 0..mesh.primitives.len() {
            let data = decode_mesh_primitive(&loaded, link, part).expect("embedded link geometry");
            let normals = data.normals.expect("embedded link normals");
            let mut vertices = Vec::with_capacity(data.positions.len() * 8);
            for (p, n) in data.positions.iter().zip(normals) {
                vertices.extend_from_slice(&[p[0], p[1], p[2], 1., n[0], n[1], n[2], 0.]);
            }
            let geometry = Geometry::new(cx);
            geometry.update(cx, data.indices, vertices);
            parts.push(LinkPart {
                link,
                metal: data.material == Some(1),
                geometry,
            });
        }
    }
    parts
}

fn cube(cx: &mut Cx) -> Geometry {
    let mut indices = Vec::new();
    let mut vertices = Vec::new();
    for axis in 0..3 {
        for sign in [-1., 1.] {
            let mut n = [0.; 3];
            n[axis] = sign;
            let first = (vertices.len() / 8) as u32;
            for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                let mut p = [0.; 3];
                p[axis] = sign * 0.5;
                p[(axis + 1) % 3] = a;
                p[(axis + 2) % 3] = b * sign;
                vertices.extend_from_slice(&[p[0], p[1], p[2], 1., n[0], n[1], n[2], 0.]);
            }
            indices.extend_from_slice(&[first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }
    let mesh = Geometry::new(cx);
    mesh.update(cx, indices, vertices);
    mesh
}

fn metres(p: Point) -> Vec3f {
    vec3(
        (p[0] * 0.001) as f32,
        (p[1] * 0.001) as f32,
        (p[2] * 0.001) as f32,
    )
}
fn affine(x: Vec3f, y: Vec3f, z: Vec3f, t: Vec3f) -> Mat4f {
    Mat4f {
        v: [
            x.x, x.y, x.z, 0., y.x, y.y, y.z, 0., z.x, z.y, z.z, 0., t.x, t.y, t.z, 1.,
        ],
    }
}
fn model(f: Frame) -> Mat4f {
    let [x, y, z] = f.axes.map(|a| vec3(a[0] as f32, a[1] as f32, a[2] as f32));
    affine(x, y, z, metres(f.origin))
}

#[derive(Clone, Copy)]
struct Camera {
    yaw: f32,
    pitch: f32,
    radius: f32,
    target: Vec3f,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: -1.05,
            pitch: 0.32,
            radius: 1.48,
            target: vec3(0.27, 0., 0.405),
        }
    }
}
impl Camera {
    fn forward(&self) -> Vec3f {
        vec3(
            self.yaw.cos() * self.pitch.cos(),
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
        )
    }
}

#[derive(Script, Widget)]
pub struct RobotScene {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    draw_surface: UfSurface,
    #[live]
    draw_image: DrawQuad,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    parts: Vec<LinkPart>,
    #[rust]
    cube: Option<Geometry>,
    #[rust]
    camera: Camera,
    #[rust]
    ortho: bool,
    #[rust]
    last_drag: Vec2d,
    #[rust]
    pan: bool,
    #[rust]
    q: [f64; 6],
    #[rust]
    zones: Vec<Zone>,
    #[rust]
    conflict: Option<usize>,
    #[rust]
    tcp: Option<Point>,
    #[rust]
    trail: Vec<Point>,
}
impl ScriptHook for RobotScene {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.parts = load_arm(cx);
            self.cube = Some(cube(cx));
            self.color_texture = Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.depth_texture = Texture::new_with_format(
                cx,
                TextureFormat::DepthD32 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.pass.set_color_texture(
                cx,
                &self.color_texture,
                DrawPassClearColor::ClearWith(vec4(0.055, 0.082, 0.102, 1.)),
            );
            self.pass
                .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.));
            self.pass.set_keep_camera_matrix(cx, true);
        });
    }
}
impl RobotScene {
    pub fn set_scene(&mut self, cx: &mut Cx, q: [f64; 6], zones: &[Zone], conflict: Option<usize>) {
        if self.q == q && self.zones == zones && self.conflict == conflict {
            return;
        }
        self.q = q;
        if self.zones != zones {
            self.zones.clear();
            self.zones.extend_from_slice(zones);
        }
        self.conflict = conflict;
        self.area.redraw(cx);
    }

    pub fn set_trail(&mut self, cx: &mut Cx, path: &[Point]) {
        if self.trail != path {
            self.trail.clear();
            self.trail.extend_from_slice(path);
            self.area.redraw(cx);
        }
    }

    pub fn set_tool_tip(&mut self, cx: &mut Cx, tcp: Option<Point>) {
        if self.tcp != tcp {
            self.tcp = tcp;
            self.area.redraw(cx);
        }
    }

    /// Reset framing to include the arm and all taught boxes. The first
    /// preset orbits freely; the front and plan views are orthographic.
    pub fn set_camera(&mut self, cx: &mut Cx, preset: usize) {
        self.camera = Camera::default();
        if preset == 1 || preset == 2 {
            self.camera.yaw = -std::f32::consts::FRAC_PI_2;
            self.camera.pitch = if preset == 1 {
                0.
            } else {
                std::f32::consts::FRAC_PI_2 - 0.0001
            };
        }
        self.ortho = preset == 1 || preset == 2;
        let corners: Vec<Vec3f> = crate::kinematics::link_boxes(self.q)
            .into_iter()
            .chain(self.zones.iter().map(|(_, a, b)| (*a, *b)))
            .chain(self.tcp.map(|p| (p, p)))
            .flat_map(|(lo, hi)| {
                (0..8).map(move |mask| {
                    metres(std::array::from_fn(|k| {
                        if mask & (1 << k) == 0 {
                            lo[k]
                        } else {
                            hi[k]
                        }
                    }))
                })
            })
            .chain(self.trail.iter().copied().map(metres))
            .collect();
        let (mut lo, mut hi) = (corners[0], corners[0]);
        for p in &corners {
            lo = vec3(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = vec3(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        self.camera.target = (lo + hi) * 0.5;
        let r = self.area.rect(cx);
        let aspect = if r.size.x > 0. && r.size.y > 0. {
            (r.size.x / r.size.y) as f32
        } else {
            1.
        };
        let fwd = self.camera.forward();
        let right = Vec3f::cross(vec3(0., 0., 1.), fwd).normalize();
        let up = Vec3f::cross(fwd, right);
        let half_fov_tan = (38f32.to_radians() * 0.5).tan() * 0.90;
        self.camera.radius = corners.iter().fold(0.35f32, |radius, p| {
            let v = *p - self.camera.target;
            let depth = if self.ortho { 0. } else { v.dot(fwd) };
            let edge = (v.dot(right) / aspect).abs().max(v.dot(up).abs());
            radius.max(depth + edge / half_fov_tan)
        });
        self.area.redraw(cx);
    }

    fn segment(&mut self, cx: &mut Cx2d, a: Vec3f, b: Vec3f, w: f32, color: Vec4f) {
        let along = b - a;
        if along.length() < 1e-6 {
            return;
        }
        let d = along.normalize();
        let up = if d.z.abs() > 0.95 {
            vec3(0., 1., 0.)
        } else {
            vec3(0., 0., 1.)
        };
        let side = Vec3f::cross(d, up).normalize();
        let tf = affine(side * w, Vec3f::cross(d, side) * w, along, (a + b) * 0.5);
        self.draw_surface
            .draw(cx, self.cube.as_ref().unwrap(), tf, color, 2.);
    }

    fn box_edges(&mut self, cx: &mut Cx2d, zone: Zone, width: f32) {
        let (allow, lo, hi) = zone;
        let color = if allow {
            vec4(0.21, 0.73, 0.66, 1.)
        } else {
            vec4(0.98, 0.36, 0.24, 1.)
        };
        for mask in 0..8 {
            let a: Point =
                std::array::from_fn(|j| if mask & (1 << j) == 0 { lo[j] } else { hi[j] });
            for axis in 0..3 {
                if mask & (1 << axis) == 0 {
                    let mut b = a;
                    b[axis] = hi[axis];
                    self.segment(cx, metres(a), metres(b), width, color);
                }
            }
        }
    }

    fn draw_scene(&mut self, cx: &mut Cx2d, stroke: f32) {
        let link_frames = frames(self.q);
        let floor_shadow = |f: Frame| {
            let p = metres(f.origin);
            vec4(p.x + p.z * 0.32, p.y + p.z * 0.42, 0., 0.)
        };
        self.draw_surface.shadow_shoulder = floor_shadow(link_frames[2]);
        self.draw_surface.shadow_elbow = floor_shadow(link_frames[3]);
        self.draw_surface.shadow_wrist = floor_shadow(link_frames[5]);
        self.draw_surface.shadow_flange = floor_shadow(link_frames[6]);
        let t = Mat4f::nonuniform_scaled_translation(vec3(22., 22., 0.01), vec3(0., 0., -0.006));
        self.draw_surface
            .draw(cx, self.cube.as_ref().unwrap(), t, vec4(1., 1., 1., 1.), 3.);
        for i in 0..self.zones.len() {
            self.box_edges(cx, self.zones[i], stroke);
        }
        for i in 1..self.trail.len() {
            self.segment(
                cx,
                metres(self.trail[i - 1]),
                metres(self.trail[i]),
                stroke * 1.65,
                vec4(0.19, 0.88, 0.83, 1.),
            );
        }
        for part in &self.parts {
            let (color, finish) = if self.conflict == Some(part.link) {
                (vec4(0.99, 0.48, 0.24, 1.), 0.)
            } else if part.metal {
                (vec4(0.32, 0.37, 0.40, 1.), 1.)
            } else {
                (vec4(0.88, 0.88, 0.82, 1.), 0.)
            };
            self.draw_surface.draw(
                cx,
                &part.geometry,
                model(link_frames[part.link]),
                color,
                finish,
            );
        }
        let flange = link_frames[6];
        for (i, col) in [
            vec4(1., 0.42, 0.29, 1.),
            vec4(0.46, 0.87, 0.53, 1.),
            vec4(0.27, 0.70, 1., 1.),
        ]
        .into_iter()
        .enumerate()
        {
            let mut p = [0., 0., 5.];
            p[i] += if i == 2 { 50. } else { 38. };
            self.segment(
                cx,
                metres(flange.point([0., 0., 5.])),
                metres(flange.point(p)),
                stroke * 1.7,
                col,
            );
        }
        if let Some(tcp) = self.tcp {
            let tip = metres(tcp);
            let color = vec4(0.99, 0.78, 0.34, 1.);
            self.segment(cx, metres(flange.origin), tip, stroke * 1.5, color);
            let d = stroke * 4.0;
            let tf = Mat4f::nonuniform_scaled_translation(vec3(d, d, d), tip);
            self.draw_surface
                .draw(cx, self.cube.as_ref().unwrap(), tf, color, 2.);
        }
    }
}
impl Widget for RobotScene {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        if rect.size.x < 1. || rect.size.y < 1. {
            return DrawStep::done();
        }
        let aspect = (rect.size.x / rect.size.y) as f32;
        let cam = self.camera;
        let view = Mat4f::look_at(
            cam.target + cam.forward() * cam.radius,
            cam.target,
            vec3(0., 0., 1.),
        );
        let projection = if self.ortho {
            let h = cam.radius * (38f32.to_radians() * 0.5).tan();
            // Right-handed view space looks down -Z, as in Mat4f::perspective.
            let (near, far) = (0.024, cam.radius + 18.);
            let mut m = Mat4f::identity();
            m.v[0] = 1. / (h * aspect);
            m.v[5] = 1. / h;
            m.v[10] = 2. / (near - far);
            m.v[14] = (far + near) / (near - far);
            m
        } else {
            Mat4f::perspective(38., aspect, 0.024, cam.radius + 18.)
        };
        self.pass.set_size(cx, rect.size);
        self.pass.set_camera(cx, view, projection);
        cx.make_child_pass(&self.pass);
        let sample_dpi = cx.current_dpi_factor() * 1.5;
        cx.begin_pass(&self.pass, Some(sample_dpi));
        self.draw_list.begin_always(cx);
        self.draw_scene(cx, (cam.radius / rect.size.y as f32).max(0.0003));
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);
        self.draw_image
            .draw_vars
            .set_texture(0, &self.color_texture);
        self.draw_image.draw_abs(cx, rect);
        self.area = self.draw_image.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(_) => cx.set_cursor(MouseCursor::Grab),
            Hit::FingerDown(e) => {
                self.last_drag = e.abs;
                self.pan = e.modifiers.shift;
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(e) => {
                let delta = e.abs - self.last_drag;
                self.last_drag = e.abs;
                if self.pan {
                    let right = vec3(-self.camera.yaw.sin(), self.camera.yaw.cos(), 0.);
                    let up = Vec3f::cross(self.camera.forward(), right);
                    let scale =
                        self.camera.radius * 0.70 / self.area.rect(cx).size.y.max(1.) as f32;
                    self.camera.target = self.camera.target - right * (delta.x as f32 * scale)
                        + up * (delta.y as f32 * scale);
                } else {
                    self.camera.yaw -= delta.x as f32 * 0.008;
                    self.camera.pitch =
                        (self.camera.pitch + delta.y as f32 * 0.008).clamp(-1.54, 1.54);
                    self.ortho = false;
                }
                self.area.redraw(cx);
            }
            Hit::FingerUp(_) => cx.set_cursor(MouseCursor::Grab),
            Hit::FingerScroll(e) => {
                let zoom = (e.scroll.y as f32 * 0.003).clamp(-1., 1.).exp();
                self.camera.radius = (self.camera.radius * zoom).clamp(0.18, 30.);
                self.area.redraw(cx);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keep the comparison tied to the geometry people see when reviewing a
    /// clearance. A new model pack must stay inside the same UI bounds.
    #[test]
    fn nominal_link_bounds_enclose_rendered_cad() {
        let model = load_gltf_from_bytes(include_bytes!("../resources/uf850.glb"), None).unwrap();
        let pose = [0.2, -0.7, -0.8, -0.5, 0.3, 0.4];
        let chain = frames(pose);
        let bounds = crate::kinematics::link_boxes(pose);
        let meshes = model.document.meshes_slice();
        assert_eq!(meshes.len(), 7);
        for (i, mesh) in meshes.iter().enumerate() {
            for part in 0..mesh.primitives.len() {
                let primitive = decode_mesh_primitive(&model, i, part).unwrap();
                assert!(!primitive.indices.is_empty());
                for xyz in primitive.positions {
                    let world = chain[i].point(xyz.map(|v| f64::from(v) * 1000.0));
                    let (lo, hi) = bounds[i];
                    assert!((0..3).all(|k| world[k] >= lo[k] - 0.001 && world[k] <= hi[k] + 0.001));
                }
            }
        }
    }
}
