//! makepad-example-depth-mix: shader-written depth and raster geometry
//! occluding each other per pixel, both ways.
//!
//! A raster cube (`DrawCube`) intersects a sphere that a fullscreen quad
//! ray-traces and writes depth for (`fragment_depth(..)` + `depth_of`). Six
//! offscreen cases: the three depth modes (@any, @greater with the quad at
//! the near plane, @less with it at the far plane) times both draw orders
//! (cube first, sphere first). Each case must show the intersection curve
//! where the CPU's analytic answer puts it: per pixel, the nearer surface.
//!
//! ```text
//! makepad-example-depth-mix [--selftest]
//! ```
//! `--selftest` captures every case, compares it per pixel with the analytic
//! image, prints `DEPTHMIX-OK` or `DEPTHMIX-FAIL ...` and exits 0 / 1.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

const W: usize = 320;
const H: usize = 240;

/// The scene, shared by the GPU draws and the CPU reference.
const EYE: [f32; 3] = [2.4, 1.7, 4.2];
const TARGET: [f32; 3] = [0.25, 0.0, 0.1];
const FOV_Y: f32 = 40.0;
const NEAR: f32 = 0.5;
const FAR: f32 = 20.0;
const SPHERE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const CUBE_CENTER: [f32; 3] = [0.85, 0.15, 0.45];
const CUBE_SIZE: f32 = 1.3;
const CUBE_ROT_Y: f32 = 0.6;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.pod.*
    use mod.math.*
    use mod.shader.*

    // A sphere ray-traced per pixel from a fullscreen quad. The quad's own
    // (rasterised) depth is `proxy_z`; the fragment replaces it with the
    // hit's depth, so the depth test against the cube is per pixel.
    mod.draw.DepthSphere = mod.std.set_type_default() do #(DrawDepthSphere::script_shader(vm)){
        ..mod.draw.DrawQuad
        depth_out: fragment_depth(@any)

        vertex: fn() {
            self.pos = self.geom.pos
            let p = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            self.vertex_pos = vec4(p.x, p.y, self.proxy_z, 1.0)
        }

        fragment: fn() {
            let ndc = self.pos * 2.0 - vec2(1.0, 1.0)
            let a = self.inv_view_proj * vec4(ndc.x, ndc.y, 0.0, 1.0)
            let b = self.inv_view_proj * vec4(ndc.x, ndc.y, 1.0, 1.0)
            let ro = a.xyz / a.w
            let rd = normalize(b.xyz / b.w - ro)
            let oc = ro - self.sphere.xyz
            let bb = dot(oc, rd)
            let c = dot(oc, oc) - self.sphere.w * self.sphere.w
            let h = bb * bb - c
            if h < 0.0 { discard() }
            let t = (0.0 - bb) - sqrt(h)
            let hit = ro + rd * t
            self.fb0 = vec4(1.0, 0.0, 0.0, 1.0)
            // the control run leaves the rasterised (proxy) depth: it must fail
            if self.write_depth > 0.5 { self.depth_out = self.depth_of(vec4(hit.x, hit.y, hit.z, 1.0)) }
        }
    }

    mod.draw.DepthMixBlit = mod.std.set_type_default() do #(DrawDepthMixBlit::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        pixel: fn() {
            return self.image.sample(self.pos)
        }
    }

    mod.widgets.DepthMixView = set_type_default() do #(DepthMixView::register_widget(vm)){
        width: Fill
        height: Fill
        cube +: { color: #x0000ff }
        sphere_any +: { proxy_z: 0.5 }
        sphere_greater +: { depth_out: mod.shader.fragment_depth(@greater) proxy_z: 0.0 }
        sphere_less +: { depth_out: mod.shader.fragment_depth(@less) proxy_z: 0.999 }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 480)
                window.title: "depth mix"
                body +: {
                    view := mod.widgets.DepthMixView{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDepthSphere {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub inv_view_proj: Mat4f,
    #[live]
    pub sphere: Vec4f,
    #[live]
    pub proxy_z: f32,
    #[live(1.0)]
    pub write_depth: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDepthMixBlit {
    #[deref]
    pub draw_super: DrawQuad,
}

struct Case {
    name: &'static str,
    mode: usize,
    sphere_first: bool,
    pass: DrawPass,
    list: DrawList,
    color: Texture,
    depth: Texture,
    captured: Option<Vec<u8>>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct DepthMixView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    cube: DrawCube,
    #[live]
    sphere_any: DrawDepthSphere,
    #[live]
    sphere_greater: DrawDepthSphere,
    #[live]
    sphere_less: DrawDepthSphere,
    #[live]
    blit: DrawDepthMixBlit,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    cases: Vec<Case>,
    #[rust]
    frame: u64,
    #[rust]
    requested: bool,
}

fn v3(a: [f32; 3]) -> Vec3f {
    vec3f(a[0], a[1], a[2])
}

/// The camera both sides use.
fn camera() -> (Mat4f, Mat4f) {
    let view = Mat4f::look_at(v3(EYE), v3(TARGET), vec3f(0.0, 1.0, 0.0));
    let proj = Mat4f::perspective(FOV_Y, W as f32 / H as f32, NEAR, FAR);
    (view, proj)
}

/// The cube's model transform: translate(centre) * rotate_y (column-major).
fn cube_transform() -> Mat4f {
    let (s, c) = CUBE_ROT_Y.sin_cos();
    Mat4f {
        v: [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, CUBE_CENTER[0], CUBE_CENTER[1], CUBE_CENTER[2], 1.0],
    }
}

impl DepthMixView {
    fn make_cases(&mut self, cx: &mut Cx) {
        let modes = ["any", "greater", "less"];
        for mode in 0..3 {
            for sphere_first in [false, true] {
                let size = TextureSize::Fixed { width: W, height: H };
                let name: &'static str = Box::leak(
                    format!("{}/{}", modes[mode], if sphere_first { "sphere_first" } else { "cube_first" }).into_boxed_str(),
                );
                self.cases.push(Case {
                    name,
                    mode,
                    sphere_first,
                    pass: DrawPass::new_with_name(cx, name),
                    list: DrawList::new(cx),
                    color: Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: size.clone(), initial: true }),
                    depth: Texture::new_with_format(cx, TextureFormat::DepthD32 { size, initial: true }),
                    captured: None,
                });
            }
        }
    }

    fn draw_case(&mut self, cx: &mut Cx2d, i: usize) {
        let (view, proj) = camera();
        let inv_vp = Mat4f::mul(&proj, &view).invert();
        let size = dvec2(W as f64, H as f64);
        let Self { cases, cube, sphere_any, sphere_greater, sphere_less, .. } = self;
        let case = &mut cases[i];
        let sphere = match case.mode {
            0 => sphere_any,
            1 => sphere_greater,
            _ => sphere_less,
        };
        sphere.inv_view_proj = inv_vp;
        sphere.sphere = vec4(SPHERE[0], SPHERE[1], SPHERE[2], SPHERE[3]);
        sphere.write_depth = if std::env::var_os("DEPTHMIX_CONTROL").is_some() { 0.0 } else { 1.0 };
        cube.transform = cube_transform();
        cube.cube_pos = vec3f(0.0, 0.0, 0.0);
        cube.cube_size = vec3f(CUBE_SIZE, CUBE_SIZE, CUBE_SIZE);
        cube.depth_clip = 1.0;

        case.pass.set_size(cx, size);
        case.pass.clear_color_textures(cx.cx);
        case.pass.set_color_texture(cx, &case.color, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)));
        case.pass.set_depth_texture(cx, &case.depth, DrawPassClearDepth::ClearWith(1.0));
        case.pass.set_camera(cx.cx, view, proj);
        case.pass.set_keep_camera_matrix(cx.cx, true);
        cx.make_child_pass(&case.pass);
        cx.begin_pass(&case.pass, Some(1.0));
        case.list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        let full = Rect { pos: dvec2(0.0, 0.0), size };
        if case.sphere_first {
            sphere.draw_abs(cx, full);
            cube.draw(cx);
        } else {
            cube.draw(cx);
            sphere.draw_abs(cx, full);
        }
        cx.end_pass_sized_turtle();
        case.list.end(cx);
        cx.end_pass(&case.pass);
    }

    fn selftest_step(&mut self, cx: &mut Cx) {
        if self.frame % 100 == 1 {
            let got = self.cases.iter().filter(|c| c.captured.is_some()).count();
            println!("DEPTHMIX frame={} cases={} requested={} captured={}", self.frame, self.cases.len(), self.requested, got);
        }
        if !self.requested && self.frame >= 4 && !self.cases.is_empty() {
            for c in &self.cases {
                cx.request_render_texture_capture(&c.color);
            }
            self.requested = true;
        }
        for (tid, w, h, bytes) in cx.take_render_texture_captures() {
            if let Some(c) = self.cases.iter_mut().find(|c| c.color.texture_id() == tid) {
                if w == W && h == H {
                    c.captured = Some(bytes);
                }
            }
        }
        if self.requested && self.cases.iter().all(|c| c.captured.is_some()) {
            let expected = expected_image();
            let mut fails = 0;
            for c in &self.cases {
                let r = compare(&expected, c.captured.as_ref().unwrap());
                println!(
                    "DEPTHMIX case={} raw={} hard={} flipped={} sphere={} cube={} curve_sphere_front={} curve_cube_front={}",
                    c.name, r.raw, r.hard, r.flipped, r.sphere, r.cube, r.curve_sphere, r.curve_cube
                );
                if !r.pass() {
                    fails += 1;
                }
            }
            if fails == 0 {
                println!("DEPTHMIX-OK cases={}", self.cases.len());
                std::process::exit(0);
            }
            println!("DEPTHMIX-FAIL {} of {} cases", fails, self.cases.len());
            std::process::exit(1);
        }
        // a frame budget, not a wall-clock one
        if self.frame > 2000 {
            println!("DEPTHMIX-FAIL no captures after {} frames", self.frame);
            std::process::exit(1);
        }
    }
}

impl Widget for DepthMixView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
        }
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.frame += 1;
                if selftest() {
                    self.selftest_step(cx);
                }
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        if self.cases.is_empty() {
            self.make_cases(cx.cx.cx);
        }
        for i in 0..self.cases.len() {
            self.draw_case(cx, i);
        }
        // the six cases in a 3x2 grid
        let cw = rect.size.x / 3.0;
        let ch = rect.size.y / 2.0;
        for (i, c) in self.cases.iter().enumerate() {
            let r = Rect { pos: dvec2(rect.pos.x + (i / 2) as f64 * cw, rect.pos.y + (i % 2) as f64 * ch), size: dvec2(cw, ch) };
            self.blit.draw_super.draw_vars.set_texture(0, &c.color);
            self.blit.draw_super.draw_abs(cx, r);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

fn selftest() -> bool {
    std::env::args().any(|a| a == "--selftest") || std::env::var_os("DEPTHMIX_SELFTEST").is_some()
}

// ---------------------------------------------------------------------------
// The CPU reference: per pixel centre, the nearer of the sphere and the cube.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Background,
    Sphere,
    Cube,
}

struct Expected {
    class: Vec<Class>,
    /// pixels where both surfaces are hit (the curve's two sides)
    sphere_front: usize,
    cube_front: usize,
}

fn ray_sphere(ro: Vec3f, rd: Vec3f) -> Option<f32> {
    let c = v3([SPHERE[0], SPHERE[1], SPHERE[2]]);
    let oc = ro - c;
    let b = oc.dot(rd);
    let cc = oc.dot(oc) - SPHERE[3] * SPHERE[3];
    let h = b * b - cc;
    (h >= 0.0).then(|| -b - h.sqrt())
}

fn ray_cube(ro: Vec3f, rd: Vec3f) -> Option<f32> {
    // into the cube's frame: local = R^T (p - c)
    let (s, c) = CUBE_ROT_Y.sin_cos();
    let to_local = |v: Vec3f| vec3f(c * v.x - s * v.z, v.y, s * v.x + c * v.z);
    let o = to_local(ro - v3(CUBE_CENTER));
    let d = to_local(rd);
    let h = CUBE_SIZE * 0.5;
    let (mut t0, mut t1) = (f32::MIN, f32::MAX);
    for (oa, da) in [(o.x, d.x), (o.y, d.y), (o.z, d.z)] {
        if da.abs() < 1e-9 {
            if oa.abs() > h {
                return None;
            }
            continue;
        }
        let (a, b) = ((-h - oa) / da, (h - oa) / da);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    (t0 <= t1 && t1 > 0.0).then_some(t0)
}

fn expected_image() -> Expected {
    let (view, proj) = camera();
    let inv = Mat4f::mul(&proj, &view).invert();
    let mut class = vec![Class::Background; W * H];
    let (mut sphere_front, mut cube_front) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            let nx = (x as f32 + 0.5) / W as f32 * 2.0 - 1.0;
            let ny = 1.0 - (y as f32 + 0.5) / H as f32 * 2.0;
            let a = inv.transform_vec4(vec4(nx, ny, 0.0, 1.0));
            let b = inv.transform_vec4(vec4(nx, ny, 1.0, 1.0));
            let ro = vec3f(a.x / a.w, a.y / a.w, a.z / a.w);
            let rd = (vec3f(b.x / b.w, b.y / b.w, b.z / b.w) - ro).normalize();
            let ts = ray_sphere(ro, rd);
            let tc = ray_cube(ro, rd);
            class[y * W + x] = match (ts, tc) {
                (Some(s), Some(c)) => {
                    if s < c {
                        sphere_front += 1;
                        Class::Sphere
                    } else {
                        cube_front += 1;
                        Class::Cube
                    }
                }
                (Some(_), None) => Class::Sphere,
                (None, Some(_)) => Class::Cube,
                (None, None) => Class::Background,
            };
        }
    }
    Expected { class, sphere_front, cube_front }
}

fn classify(px: &[u8]) -> Option<Class> {
    // BGRA
    let (b, _g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
    if r > 160 && b < 60 {
        Some(Class::Sphere)
    } else if b > 40 && r < 30 {
        Some(Class::Cube)
    } else if r < 30 && b < 30 {
        Some(Class::Background)
    } else {
        None
    }
}

struct Report {
    raw: usize,
    hard: usize,
    flipped: bool,
    sphere: usize,
    cube: usize,
    curve_sphere: usize,
    curve_cube: usize,
}

impl Report {
    /// Edge pixels may differ (sample position vs rasterisation rules);
    /// nothing may differ where every neighbour agrees on the answer.
    fn pass(&self) -> bool {
        self.hard == 0 && self.raw * 200 < W * H && self.curve_sphere > 100 && self.curve_cube > 100 && self.sphere > 1000 && self.cube > 1000
    }
}

fn compare(e: &Expected, bytes: &[u8]) -> Report {
    let observed: Vec<Option<Class>> = (0..W * H).map(|i| classify(&bytes[i * 4..i * 4 + 4])).collect();
    let run = |flip: bool| {
        let (mut raw, mut hard, mut sphere, mut cube) = (0, 0, 0, 0);
        for y in 0..H {
            for x in 0..W {
                let oy = if flip { H - 1 - y } else { y };
                let o = observed[oy * W + x];
                match o {
                    Some(Class::Sphere) => sphere += 1,
                    Some(Class::Cube) => cube += 1,
                    _ => {}
                }
                let want = e.class[y * W + x];
                if o == Some(want) {
                    continue;
                }
                raw += 1;
                // a hard miss: no expected class in the 3x3 neighbourhood matches
                let mut near = false;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= W as i32 || ny >= H as i32 {
                            continue;
                        }
                        if Some(e.class[ny as usize * W + nx as usize]) == o {
                            near = true;
                        }
                    }
                }
                if !near {
                    hard += 1;
                }
            }
        }
        (raw, hard, sphere, cube)
    };
    let a = run(false);
    let b = run(true);
    let (flipped, r) = if b.1 < a.1 || (b.1 == a.1 && b.0 < a.0) { (true, b) } else { (false, a) };
    Report { raw: r.0, hard: r.1, flipped, sphere: r.2, cube: r.3, curve_sphere: e.sphere_front, curve_cube: e.cube_front }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// The selftest's own frame count: if the view never runs (a script
    /// error), the test still ends.
    #[rust]
    frames: u64,
    #[rust]
    next_frame: NextFrame,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if selftest() {
            if matches!(event, Event::Startup) {
                self.next_frame = cx.new_next_frame();
            }
            if let Event::NextFrame(ne) = event {
                if ne.set.contains(&self.next_frame) {
                    self.frames += 1;
                    if self.frames > 3000 {
                        println!("DEPTHMIX-FAIL the view did not finish in {} frames", self.frames);
                        std::process::exit(1);
                    }
                    self.next_frame = cx.new_next_frame();
                }
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
