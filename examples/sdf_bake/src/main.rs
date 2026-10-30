//! makepad-example-sdf-bake: the SDF engine's brick bake checked end to end.
//!
//! ```text
//! makepad-example-sdf-bake [--selftest] [--bench]
//! ```
//! For each scene (a CAD bracket, an organic part with every warp, primitive
//! and material rule):
//! 1. the CPU twin of the lowered program against `csg_sdf`'s f64 field and
//!    slots on the lattice samples within the band (the lowering is right);
//! 2. the volume built and baked on the GPU, the atlases read back, every
//!    sample of every baked brick equal to the CPU twin's bake, distance and
//!    slot (`precise_math` bit-exactness);
//! 3. an edit (a hole moved, a blend changed) re-built and re-baked only
//!    where it changed, compared with a fresh full build of the edited shape
//!    (which bricks, and every texel).
//!
//! `--selftest` prints `SDFBAKE-OK ...` or `SDFBAKE-FAIL ...`; `--bench` also
//! prints the GPU ms of the bake passes (HAL GPU profiler).

pub use makepad_widgets;

use makepad_csg_math::Vec3d;
use makepad_sdf_engine::bake::{copy_rect, slot_rect, Baker, DrawSdfBake, DrawSdfCopy, COPY_W};
use makepad_sdf_engine::makepad_csg_sdf::{distance_slot, part_id, quat_from_degrees, SdfDeform, SdfNode, SdfOp, SdfPlace};
use makepad_sdf_engine::march::{draw_object, material_texture, override_texture, DrawSdfMarch, MarchView};
use makepad_sdf_engine::program::lower;
use makepad_sdf_engine::twin;
use makepad_sdf_engine::volume::{Brick, Volume, ATLAS_W, BS};
use makepad_widgets::makepad_platform::makepad_hal::gpu_prof;
use makepad_widgets::*;
use std::sync::{Arc, Mutex};

app_main!(App);

const CELL: f32 = 0.25;
/// The march check's image and camera.
const MW: usize = 320;
const MH: usize = 240;
const FOV_Y: f32 = 40.0;
/// The bench's march target (4K).
const BW: usize = 3840;
const BH: usize = 2160;

fn march_camera(scene: &str) -> (Mat4f, Mat4f) {
    let eye = if scene == "bracket" { vec3f(40.0, 45.0, 70.0) } else { vec3f(-30.0, 25.0, 80.0) };
    let view = Mat4f::look_at(eye, vec3f(0.0, 2.0, 0.0), vec3f(0.0, 1.0, 0.0));
    let proj = Mat4f::perspective(FOV_Y, MW as f32 / MH as f32, 1.0, 1000.0);
    (view, proj)
}
const BAND: f32 = 1.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.pod.*
    use mod.math.*
    use mod.shader.*

    mod.draw.SdfBakeShow = mod.std.set_type_default() do #(DrawSdfBakeShow::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        pixel: fn() {
            let v = self.image.sample_nearest(self.pos).x
            return vec4(0.5 + v * 0.5, 0.5 - v * 0.5, 0.5, 1.0)
        }
    }

    mod.widgets.SdfBakeView = set_type_default() do #(SdfBakeView::register_widget(vm)){
        width: Fill
        height: Fill
        march +: { color_format: @Rgba32F alpha_blend: false debug_out: 1.0 }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 360)
                window.title: "sdf bake"
                body +: {
                    view := mod.widgets.SdfBakeView{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfBakeShow {
    #[deref]
    pub draw_super: DrawQuad,
}

// ---------------------------------------------------------------------------
// Scenes
// ---------------------------------------------------------------------------

fn at(at: [f64; 3], rotate: [f64; 3], of: SdfNode) -> SdfNode {
    SdfNode::Transform { at, rotate, scale: 1.0, of: Box::new(of) }
}
fn mat(slot: u8, of: SdfNode) -> SdfNode {
    SdfNode::Material { slot, within: None, of: Box::new(of) }
}
fn comb(op: SdfOp, blend: f64, of: Vec<SdfNode>) -> SdfNode {
    SdfNode::Combine { op, blend, of }
}

/// A plate with ribs (smooth union), a boss, a row of holes (an Instance
/// cut) and a slot: the CAD case. `hole_x` moves the first hole (the edit).
fn bracket(hole_x: f64, rib_blend: f64) -> SdfNode {
    let plate = SdfNode::Box { size: [60.0, 6.0, 40.0], radius: 1.0 };
    let rib = |z: f64| at([0.0, 6.0, z], [0.0, 0.0, 0.0], SdfNode::Box { size: [50.0, 8.0, 3.0], radius: 0.5 });
    let boss = at([18.0, 6.0, 0.0], [0.0, 0.0, 0.0], SdfNode::Cylinder { radius: 6.0, top_radius: 6.0, height: 14.0 });
    let solid = comb(SdfOp::Union, rib_blend, vec![mat(1, plate), rib(-12.0), rib(12.0), mat(2, boss)]);
    let hole = SdfNode::Cylinder { radius: 2.5, top_radius: 2.5, height: 30.0 };
    let places: Vec<SdfPlace> = [hole_x, -10.0, 0.0, 10.0]
        .iter()
        .map(|x| SdfPlace { at: [*x, 0.0, -6.0], rotate: [0.0, 0.0, 0.0, 1.0], scale: 1.0 })
        .collect();
    let holes = SdfNode::Instance { places, of: Box::new(hole) };
    let slot = at([-22.0, 0.0, 10.0], [0.0, 30.0, 0.0], SdfNode::Capsule { a: [-5.0, 0.0, 0.0], b: [5.0, 0.0, 0.0], radius: 2.0 });
    let bore = at([18.0, 6.0, 0.0], [0.0, 0.0, 0.0], SdfNode::Cylinder { radius: 3.0, top_radius: 4.5, height: 16.0 });
    SdfNode::Id { id: part_id("bracket"), of: Box::new(comb(SdfOp::Difference, 0.8, vec![solid, holes, slot, bore])) }
}

/// Every warp, the curved primitives, nested materials, a shell and an
/// intersection: the organic case. `blend` changes the body's blend (the
/// edit).
fn organic(blend: f64) -> SdfNode {
    let body = comb(
        SdfOp::Union,
        blend,
        vec![
            mat(3, SdfNode::Ellipsoid { radii: [12.0, 8.0, 9.0] }),
            at([0.0, 10.0, 0.0], [0.0, 0.0, 0.0], SdfNode::Sphere { radius: 6.0 }),
            SdfNode::Capsule { a: [-14.0, -4.0, 0.0], b: [-4.0, 2.0, 3.0], radius: 2.5 },
        ],
    );
    let twist = at([24.0, 0.0, 0.0], [0.0, 0.0, 0.0], SdfNode::deform(SdfDeform::Twist { rate: 0.12 }, SdfNode::Box { size: [6.0, 20.0, 4.0], radius: 0.5 }));
    let bend = at([0.0, -14.0, 0.0], [0.0, 0.0, 0.0], SdfNode::deform(SdfDeform::Bend { rate: 0.04 }, SdfNode::Box { size: [26.0, 3.0, 3.0], radius: 0.3 }));
    let taper = at([-24.0, 0.0, 0.0], [10.0, 0.0, 0.0], SdfNode::deform(SdfDeform::Taper { rate: -0.03 }, SdfNode::Cylinder { radius: 4.0, top_radius: 4.0, height: 18.0 }));
    let mirror = SdfNode::deform(SdfDeform::Mirror { axis: 2 }, at([6.0, 4.0, 10.0], [0.0, 0.0, 0.0], mat(4, SdfNode::Torus { radius: 3.0, tube: 1.0 })));
    let cone = at([0.0, 20.0, 0.0], [0.0, 0.0, 0.0], SdfNode::Cylinder { radius: 4.0, top_radius: 1.0, height: 8.0 });
    let lattice = comb(
        SdfOp::Intersection,
        0.0,
        vec![SdfNode::Gyroid { period: 6.0, wall: 0.8 }, at([0.0, 0.0, -22.0], [0.0, 0.0, 0.0], SdfNode::Sphere { radius: 8.0 })],
    );
    let shell = at([0.0, 0.0, 22.0], [0.0, 0.0, 0.0], SdfNode::Shell { thickness: 0.6, of: Box::new(SdfNode::Round { radius: 1.0, of: Box::new(SdfNode::Box { size: [10.0, 10.0, 10.0], radius: 0.0 }) }) });
    let inst = SdfNode::Instance {
        places: (0..5).map(|i| SdfPlace { at: [-20.0 + i as f64 * 10.0, -24.0, 12.0], rotate: quat_from_degrees([0.0, 30.0 * i as f64, 20.0]), scale: 1.0 + 0.1 * i as f64 }).collect(),
        of: Box::new(mat(5, SdfNode::Box { size: [4.0, 2.0, 6.0], radius: 0.4 })),
    };
    mat(6, comb(SdfOp::Union, 0.0, vec![body, twist, bend, taper, mirror, cone, lattice, shell, inst]))
}

struct Scene {
    name: &'static str,
    before: SdfNode,
    after: SdfNode,
}

fn scenes() -> Vec<Scene> {
    vec![
        Scene { name: "bracket", before: bracket(-20.0, 2.0), after: bracket(-18.5, 2.0) },
        Scene { name: "organic", before: organic(3.0), after: organic(4.0) },
    ]
}

// ---------------------------------------------------------------------------
// CPU checks
// ---------------------------------------------------------------------------

/// The twin of the whole program against csg_sdf (f64) on the samples within
/// the band: the distance to 1e-3 mm, the slot exactly (away from ties).
fn check_lowering(name: &str, n: &SdfNode, vol: &Volume) -> bool {
    let p = match lower(n) {
        Ok(p) => p,
        Err(e) => {
            println!("SDFBAKE scene={name} lowering error {e:?}");
            return false;
        }
    };
    let (mut checked, mut bad_d, mut bad_s, mut max_err) = (0usize, 0usize, 0usize, 0f64);
    for (i, b) in vol.bricks.iter().enumerate() {
        if !matches!(b, Brick::Baked { .. }) {
            continue;
        }
        let bc = vol.brick_of(i);
        for k in (0..512).step_by(7) {
            let g = [bc[0] * BS + k % 8, bc[1] * BS + (k / 8) % 8, bc[2] * BS + k / 64];
            let q = g.map(|v| v as f32 * CELL);
            let (d32, s32) = twin::eval(&p.texels, q);
            let (d64, s64) = distance_slot(n, Vec3d::new(q[0] as f64, q[1] as f64, q[2] as f64));
            if d64.abs() >= BAND as f64 {
                continue;
            }
            checked += 1;
            let err = (d32 as f64 - d64).abs();
            max_err = max_err.max(err);
            if err > 1e-3 {
                bad_d += 1;
            }
            if s32 != s64 && d64.abs() < BAND as f64 * 0.5 {
                bad_s += 1;
            }
        }
    }
    println!("SDFBAKE scene={name} lowering samples={checked} max_err={max_err:.2e} dist_bad={bad_d} slot_bad={bad_s}");
    checked > 0 && bad_d == 0 && bad_s * 1000 <= checked
}

/// The CPU twin's atlas texels of every baked brick (by brick coordinate).
fn cpu_bake(vol: &Volume) -> std::collections::BTreeMap<[i32; 3], (Vec<f32>, Vec<u8>)> {
    let mut out = std::collections::BTreeMap::new();
    for i in 0..vol.bricks.len() {
        if let Some(v) = vol.bake_brick_cpu(i) {
            out.insert(vol.brick_of(i), v);
        }
    }
    out
}

/// The GPU atlases' texels of every baked brick (by brick coordinate).
fn gpu_bricks(vol: &Volume, rows: usize, atlas: &[u8], slots: &[u8]) -> std::collections::BTreeMap<[i32; 3], (Vec<f32>, Vec<u8>)> {
    let mut out = std::collections::BTreeMap::new();
    for i in 0..vol.bricks.len() {
        let Brick::Baked { slot, .. } = vol.bricks[i] else { continue };
        let r = slot_rect(slot);
        let (x0, y0) = (r.pos.x as usize, r.pos.y as usize);
        let mut d = vec![0.0f32; 512];
        let mut s = vec![0u8; 512];
        for y in 0..8 {
            for x in 0..64 {
                let t = (y0 + y) * ATLAS_W + x0 + x;
                if t * 4 + 4 > atlas.len() || y0 + y >= rows {
                    continue;
                }
                d[y * 64 + x] = f32::from_le_bytes([atlas[t * 4], atlas[t * 4 + 1], atlas[t * 4 + 2], atlas[t * 4 + 3]]);
                s[y * 64 + x] = slots[t];
            }
        }
        out.insert(vol.brick_of(i), (d, s));
    }
    out
}

/// Differing samples between two bakes, and bricks only one has.
fn compare(a: &std::collections::BTreeMap<[i32; 3], (Vec<f32>, Vec<u8>)>, b: &std::collections::BTreeMap<[i32; 3], (Vec<f32>, Vec<u8>)>) -> (usize, usize, usize, String) {
    let (mut dd, mut ds, mut only) = (0, 0, 0);
    let mut first = String::new();
    for (k, (d0, s0)) in a {
        match b.get(k) {
            None => only += 1,
            Some((d1, s1)) => {
                for i in 0..512 {
                    if d0[i].to_bits() != d1[i].to_bits() {
                        if dd == 0 {
                            first = format!(" first brick {k:?} sample {i}: {:e} vs {:e}", d0[i], d1[i]);
                        }
                        dd += 1;
                    }
                    if s0[i] != s1[i] {
                        ds += 1;
                    }
                }
            }
        }
    }
    only += b.keys().filter(|k| !a.contains_key(*k)).count();
    (dd, ds, only, first)
}

/// The march's steps for one ray on the CPU (the shader's logic over the CPU
/// bake, f32 instead of the f16 copy), printed.
fn trace_march(vol: &Volume, cpu: &std::collections::BTreeMap<[i32; 3], (Vec<f32>, Vec<u8>)>, ro: [f32; 3], rd: [f32; 3], pix_angle: f32) {
    let bs = CELL * 8.0;
    let sample = |g: [i32; 3]| -> f32 {
        let nb = g.map(|v| v.div_euclid(BS));
        let inside = (0..3).all(|a| nb[a] >= vol.min[a] && nb[a] < vol.min[a] + vol.dim[a]);
        if !inside {
            return BAND;
        }
        match vol.bricks[vol.index(nb)] {
            Brick::Baked { .. } => {
                let l = [g[0] - nb[0] * BS, g[1] - nb[1] * BS, g[2] - nb[2] * BS];
                cpu[&nb].0[(l[1] * 64 + l[2] * 8 + l[0]) as usize]
            }
            Brick::Empty { skip } => if skip < 0.0 { -BAND } else { BAND },
        }
    };
    let lo = vol.min.map(|v| v as f32 * bs);
    let hi: [f32; 3] = std::array::from_fn(|i| (vol.min[i] + vol.dim[i]) as f32 * bs);
    let mut t0 = 0f32;
    let mut t1 = f32::MAX;
    for i in 0..3 {
        let (a, b) = ((lo[i] - ro[i]) / rd[i], (hi[i] - ro[i]) / rd[i]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    let mut t = t0 + CELL * 0.01;
    for step in 0..256 {
        if t > t1 {
            println!("    {step}: past the box");
            return;
        }
        let p: [f32; 3] = std::array::from_fn(|i| ro[i] + rd[i] * t);
        let b = p.map(|v| (v / bs).floor() as i32);
        let inside = (0..3).all(|a| b[a] >= vol.min[a] && b[a] < vol.min[a] + vol.dim[a]);
        let entry = if inside { Some(vol.bricks[vol.index(b)]) } else { None };
        match entry {
            Some(Brick::Baked { .. }) => {
                let u = p.map(|v| v / CELL);
                let g0 = u.map(|v| v.floor() as i32);
                let f: [f32; 3] = std::array::from_fn(|i| u[i] - g0[i] as f32);
                let mut d = 0.0;
                for k in 0..8 {
                    let o = [k & 1, (k >> 1) & 1, k >> 2];
                    let wgt: f32 = (0..3).map(|i| if o[i] == 1 { f[i] } else { 1.0 - f[i] }).product();
                    d += wgt * sample([g0[0] + o[0], g0[1] + o[1], g0[2] + o[2]]);
                }
                let d = d * 0.9;
                let eps = (CELL * 0.02).max(t * pix_angle);
                println!("    {step}: t={t:.3} brick {b:?} baked d={d:.4} eps={eps:.3}");
                if d < eps {
                    return;
                }
                t += d.max(CELL * 0.05);
            }
            other => {
                let skip = match other { Some(Brick::Empty { skip }) => skip, _ => BAND };
                let bmin = b.map(|v| v as f32 * bs);
                let te: Vec<f32> = (0..3).map(|i| ((if rd[i] >= 0.0 { bmin[i] + bs } else { bmin[i] }) - ro[i]) / rd[i]).collect();
                let t_exit = te[0].min(te[1]).min(te[2]);
                println!("    {step}: t={t:.3} brick {b:?} empty skip={skip:.3} exit={t_exit:.3}");
                t = t_exit.max(t + skip.abs()) + CELL * 0.01;
            }
        }
    }
}

/// f32 to f16 bits, round to nearest even (what a render target write does).
fn f16_bits(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rest = m & ((1 << shift) - 1);
        let mut h = m >> shift;
        if rest > half || (rest == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let rest = man & 0x1fff;
    if rest > 0x1000 || (rest == 0x1000 && h & 1 == 1) {
        h += 1;
    }
    sign | h as u16
}

/// The render copy of every baked brick against the CPU bake: each 9^3
/// sample (the apron from the neighbour, the band with the skip's sign for
/// an empty one) as f16 bits, and the slots.
fn check_copy(vol: &Volume, cpu: &std::collections::BTreeMap<[i32; 3], (Vec<f32>, Vec<u8>)>, copy: &[u8], copy_slots: &[u8]) -> (usize, usize, String) {
    let (mut dd, mut ds) = (0, 0);
    let mut first = String::new();
    for (i, b) in vol.bricks.iter().enumerate() {
        let Brick::Baked { slot, .. } = *b else { continue };
        let bc = vol.brick_of(i);
        let r = copy_rect(slot);
        for z in 0..9i32 {
            for y in 0..9i32 {
                for x in 0..9i32 {
                    let g = [bc[0] * BS + x, bc[1] * BS + y, bc[2] * BS + z];
                    let nb = g.map(|v| v.div_euclid(BS));
                    let l = [g[0] - nb[0] * BS, g[1] - nb[1] * BS, g[2] - nb[2] * BS];
                    let inside = (0..3).all(|a| nb[a] >= vol.min[a] && nb[a] < vol.min[a] + vol.dim[a]);
                    let (want, want_s) = if !inside {
                        (BAND, 0u8)
                    } else {
                        match vol.bricks[vol.index(nb)] {
                            Brick::Baked { .. } => {
                                let (d, s) = &cpu[&nb];
                                let k = (l[1] * 64 + l[2] * 8 + l[0]) as usize;
                                (d[k], s[k])
                            }
                            Brick::Empty { skip } => (if skip < 0.0 { -BAND } else { BAND }, 0),
                        }
                    };
                    let t = (r.pos.y as usize + y as usize) * COPY_W + r.pos.x as usize + (z * 9 + x) as usize;
                    let got = u16::from_le_bytes([copy[t * 2], copy[t * 2 + 1]]);
                    if got != f16_bits(want) {
                        if dd == 0 {
                            first = format!(" first brick {bc:?} ({x},{y},{z}): want {:04x} ({want}) got {got:04x}", f16_bits(want));
                        }
                        dd += 1;
                    }
                    if copy_slots[t] != want_s {
                        ds += 1;
                    }
                }
            }
        }
    }
    (dd, ds, first)
}

// ---------------------------------------------------------------------------
// The view: bakes each scene, then its edit, reads the atlases back.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Build scene `n` and queue it.
    Build,
    /// Baking until the queue is idle, then read back.
    Bake,
    Read,
    /// The march over the baked scene into an RGBA32F check target, then
    /// its readback against a CPU ray cast of the exact program.
    March,
    MarchWait(u64),
    /// `--bench`: the march re-recorded every frame at 4K, timed.
    MarchBench(u32),
    MarchRead,
    /// The edit: re-build the changed blocks, re-bake.
    Edit,
    Done,
}

type Readbacks = Arc<Mutex<Vec<(u8, Result<(usize, usize, Arc<[u8]>), String>)>>>;

#[derive(Script, ScriptHook, Widget)]
pub struct SdfBakeView {
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
    bake: DrawSdfBake,
    #[live]
    copy: DrawSdfCopy,
    #[live]
    march: DrawSdfMarch,
    #[rust]
    march_pass: Option<(DrawPass, DrawList, Texture, Texture, (usize, usize))>,
    #[rust]
    march_ms: Vec<f64>,
    #[rust]
    materials: Option<Texture>,
    #[rust]
    overrides: Option<Texture>,
    #[live]
    show: DrawSdfBakeShow,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    frame: u64,
    #[rust]
    scenes: Vec<Scene>,
    #[rust]
    scene: usize,
    #[rust]
    edited: bool,
    #[rust(Step::Build)]
    step: Step,
    #[rust]
    baker: Option<Baker>,
    #[rust]
    vol: Option<Volume>,
    #[rust]
    readbacks: Readbacks,
    #[rust]
    idle_frames: u32,
    #[rust]
    fails: Vec<String>,
    #[rust]
    prof_from: u64,
    #[rust]
    gpu_ms: Vec<f64>,
}

impl SdfBakeView {
    fn step_frame(&mut self, cx: &mut Cx2d) {
        match self.step {
            Step::Build => {
                if self.scenes.is_empty() {
                    self.scenes = scenes();
                }
                let sc = &self.scenes[self.scene];
                let bounds = Volume::bounds_of(&sc.before).expect("bounded scene");
                // room for the edit
                let bounds = (bounds.0.map(|v| v - 4.0), bounds.1.map(|v| v + 4.0));
                let mut vol = Volume::new(bounds, CELL, BAND);
                let st = vol.build(&sc.before, None).expect("lowers");
                println!(
                    "SDFBAKE scene={} build {:.1} ms: {} blocks ({} skipped), {} bricks tested, {} baked, {} programs, gpu-unfit {}",
                    sc.name, st.ms, st.blocks, st.blocks_skipped, st.bricks_tested, st.bricks_baked, st.programs, vol.cpu_only.len()
                );
                if !check_lowering(sc.name, &sc.before, &vol) {
                    self.fails.push(format!("{}-lowering", sc.name));
                }
                if self.baker.is_none() {
                    self.baker = Some(Baker::new(cx.cx.cx));
                }
                let baker = self.baker.as_mut().unwrap();
                baker.reset();
                baker.sync(cx.cx.cx, &mut vol);
                self.vol = Some(vol);
                self.step = Step::Bake;
                self.idle_frames = 0;
            }
            Step::Edit => {
                let sc = &self.scenes[self.scene];
                let vol = self.vol.as_mut().unwrap();
                let st = vol.edit(&sc.before, &sc.after).expect("lowers").expect("fits");
                println!("SDFBAKE scene={} edit {:.1} ms: {} blocks re-built, {} bricks baked", sc.name, st.ms, st.blocks, st.bricks_baked);
                self.baker.as_mut().unwrap().sync(cx.cx.cx, vol);
                self.edited = true;
                self.step = Step::Bake;
                self.idle_frames = 0;
            }
            Step::March => {
                self.record_march(cx);
                self.step = Step::MarchWait(cx.cx.repaint_id());
            }
            Step::MarchBench(n) => {
                self.record_march(cx);
                self.step = if n >= 90 {
                    let mut v = std::mem::take(&mut self.march_ms);
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let med = v.get(v.len() / 2).copied().unwrap_or(0.0);
                    println!("SDFBAKE-BENCH scene={} march {}x{} gpu_ms median={med:.3} (n={})", self.scenes[self.scene].name, BW, BH, v.len());
                    Step::Edit
                } else {
                    Step::MarchBench(n + 1)
                }
            }
            Step::MarchWait(serial) => {
                let (pass, ..) = self.march_pass.as_ref().unwrap();
                cx.make_child_pass(pass);
                if pass.painted_whole_since(cx.cx, serial) {
                    let tex = self.march_pass.as_ref().unwrap().2.clone();
                    self.readbacks.lock().unwrap().clear();
                    let out = self.readbacks.clone();
                    if let Err(e) = cx.cx.cx.read_render_texture(&tex, move |res| out.lock().unwrap().push((9, res.map_err(|e| format!("{e:?}"))))) {
                        println!("SDFBAKE-FAIL march readback request: {e:?}");
                        std::process::exit(1);
                    }
                    self.step = Step::MarchRead;
                }
            }
            Step::MarchRead => {
                let (pass, ..) = self.march_pass.as_ref().unwrap();
                cx.make_child_pass(pass);
            }
            _ => {}
        }
        if let (Some(baker), Some(vol)) = (self.baker.as_mut(), self.vol.as_ref()) {
            baker.draw(cx, vol, &mut self.bake, &mut self.copy);
        }
        if self.step == Step::Bake && self.baker.as_ref().is_some_and(|b| b.idle()) {
            // a few idle frames so the readback sees the finished atlas
            self.idle_frames += 1;
            if self.idle_frames > 3 {
                self.request_readback(cx.cx.cx);
                self.step = Step::Read;
            }
        }
    }

    fn request_readback(&mut self, cx: &mut Cx) {
        self.readbacks.lock().unwrap().clear();
        let baker = self.baker.as_ref().unwrap();
        for (k, tex) in [(0u8, baker.atlas.clone()), (1u8, baker.slots.clone()), (2u8, baker.copy.clone()), (3u8, baker.copy_slots.clone())] {
            let out = self.readbacks.clone();
            if let Err(e) = cx.read_render_texture(&tex, move |res| out.lock().unwrap().push((k, res.map_err(|e| format!("{e:?}"))))) {
                println!("SDFBAKE-FAIL readback request: {e:?}");
                std::process::exit(1);
            }
        }
    }

    fn check_read(&mut self) {
        let got = {
            let r = self.readbacks.lock().unwrap();
            if r.len() < 4 {
                return;
            }
            r.clone()
        };
        let sc = &self.scenes[self.scene];
        let vol = self.vol.as_ref().unwrap();
        let rows = self.baker.as_ref().unwrap().rows;
        let find = |k: u8| got.iter().find(|(kk, _)| *kk == k).and_then(|(_, r)| r.as_ref().ok().cloned());
        let (Some((_, _, atlas)), Some((_, _, slots)), Some((_, _, copy)), Some((_, _, copy_slots))) = (find(0), find(1), find(2), find(3)) else {
            self.fails.push(format!("{}-readback", sc.name));
            self.next_scene();
            return;
        };
        let stage = if self.edited { "edit" } else { "full" };
        let gpu = gpu_bricks(vol, rows, &atlas, &slots);
        let cpu = cpu_bake(vol);
        let (dd, ds, only, first) = compare(&gpu, &cpu);
        let bricks = cpu.len();
        println!("SDFBAKE scene={} {stage} gpu-vs-twin bricks={bricks} dist_differing={dd} slot_differing={ds} unmatched={only}{first}", sc.name);
        if dd != 0 || ds != 0 || only != 0 {
            self.fails.push(format!("{}-{stage}-gpu", sc.name));
        }
        let (cd, cs, cfirst) = check_copy(vol, &cpu, &copy, &copy_slots);
        println!("SDFBAKE scene={} {stage} render-copy bricks={bricks} dist_differing={cd} slot_differing={cs}{cfirst}", sc.name);
        if cd != 0 || cs != 0 {
            self.fails.push(format!("{}-{stage}-copy", sc.name));
        }
        if !self.edited {
            self.step = Step::March;
            return;
        }
        // the dirty re-build against a fresh full build of the edited shape
        let mut fresh = Volume::new(
            {
                let b = Volume::bounds_of(&sc.before).unwrap();
                (b.0.map(|v| v - 4.0), b.1.map(|v| v + 4.0))
            },
            CELL,
            BAND,
        );
        fresh.build(&sc.after, None).expect("lowers");
        let (fd, fs, fonly, ffirst) = compare(&cpu, &cpu_bake(&fresh));
        println!("SDFBAKE scene={} dirty-vs-full bricks={} dist_differing={fd} slot_differing={fs} unmatched={fonly}{ffirst}", sc.name, cpu.len());
        if fd != 0 || fs != 0 || fonly != 0 {
            self.fails.push(format!("{}-dirty", sc.name));
        }
        self.next_scene();
    }

    fn record_march(&mut self, cx: &mut Cx2d) {
        let (w, h) = if matches!(self.step, Step::MarchBench(_)) { (BW, BH) } else { (MW, MH) };
        let size = dvec2(w as f64, h as f64);
        if self.march_pass.as_ref().is_some_and(|m| m.4 != (w, h)) {
            self.march_pass = None;
        }
        if self.march_pass.is_none() {
            let c = &mut *cx.cx.cx;
            self.march_pass = Some((
                DrawPass::new_with_name(c, "sdf_march"),
                DrawList::new(c),
                Texture::new_with_format(c, TextureFormat::RenderRGBAf32 { size: TextureSize::Fixed { width: w, height: h }, initial: true }),
                Texture::new_with_format(c, TextureFormat::DepthD32 { size: TextureSize::Fixed { width: w, height: h }, initial: true }),
                (w, h),
            ));
            self.materials = Some(material_texture(c, &[]));
            self.overrides = Some(override_texture(c, &[]));
        }
        let (view, proj) = march_camera(&self.scenes[self.scene].name);
        let mv = MarchView { view, proj, size, fov_y: FOV_Y.to_radians() };
        let (pass, list, color, depth, _) = self.march_pass.as_mut().unwrap();
        pass.set_size(cx, size);
        pass.clear_color_textures(cx.cx);
        pass.set_color_texture(cx, color, DrawPassClearColor::ClearWith(vec4(-1.0, 0.0, 0.0, 0.0)));
        pass.set_depth_texture(cx, depth, DrawPassClearDepth::ClearWith(1.0));
        pass.set_camera(cx.cx, view, proj);
        pass.set_keep_camera_matrix(cx.cx, true);
        cx.make_child_pass(pass);
        cx.begin_pass(pass, Some(1.0));
        list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        let full = Rect { pos: dvec2(0.0, 0.0), size };
        draw_object(cx, &mut self.march, full, &mv, Mat4f::identity(), self.vol.as_ref().unwrap(), self.baker.as_ref().unwrap(), self.materials.as_ref().unwrap(), self.overrides.as_ref().unwrap(), true);
        cx.end_pass_sized_turtle();
        list.end(cx);
        cx.end_pass(pass);
    }

    /// The march's (t, slot) per pixel against a CPU sphere trace of the
    /// whole lowered program (the twin, exact): hit masks and depths.
    fn check_march(&mut self) {
        let got = {
            let r = self.readbacks.lock().unwrap();
            if r.is_empty() {
                return;
            }
            r.clone()
        };
        let sc = &self.scenes[self.scene];
        let Some((_, Ok((w, h, px)))) = got.first().cloned() else {
            self.fails.push(format!("{}-march-readback", sc.name));
            self.step = Step::Edit;
            return;
        };
        let prog = lower(&sc.before).unwrap();
        let vol = self.vol.as_ref().unwrap();
        let cpu = cpu_bake(vol);
        let (view, proj) = march_camera(&sc.name);
        let ivp = Mat4f::mul(&proj, &view).invert();
        let pix_angle = 2.0 * (FOV_Y.to_radians() * 0.5).tan() / MH as f32;
        let gpu_t = |x: usize, y: usize| f32::from_le_bytes([px[(y * w + x) * 16], px[(y * w + x) * 16 + 1], px[(y * w + x) * 16 + 2], px[(y * w + x) * 16 + 3]]);
        let mut cpu_hit = vec![None; w * h];
        let mut rays = vec![([0f32; 3], [0f32; 3]); w * h];
        for y in 0..h {
            for x in 0..w {
                let ndc = [(x as f32 + 0.5) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / h as f32 * 2.0];
                let a = ivp.transform_vec4(vec4(ndc[0], ndc[1], 0.0, 1.0));
                let b = ivp.transform_vec4(vec4(ndc[0], ndc[1], 1.0, 1.0));
                let ro = [a.x / a.w, a.y / a.w, a.z / a.w];
                let far = [b.x / b.w, b.y / b.w, b.z / b.w];
                let d = [far[0] - ro[0], far[1] - ro[1], far[2] - ro[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let rd = d.map(|v| v / l);
                rays[y * w + x] = (ro, rd);
                let mut t = 0.0f32;
                for _ in 0..600 {
                    let p = [ro[0] + rd[0] * t, ro[1] + rd[1] * t, ro[2] + rd[2] * t];
                    let (dist, _) = twin::eval(&prog.texels, p);
                    // the same cone stopping test as the march (a pixel's footprint)
                    if dist < (CELL * 0.02).max(t * pix_angle) {
                        cpu_hit[y * w + x] = Some(t);
                        break;
                    }
                    t += dist.max(0.01);
                    if t > 400.0 {
                        break;
                    }
                }
            }
        }
        // pixels whose 3x3 neighbourhood agrees on hit/miss (silhouette pixels may differ)
        let (mut checked, mut mask_bad, mut depth_bad, mut hits, mut max_dt) = (0usize, 0usize, 0usize, 0usize, 0f32);
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let c = cpu_hit[y * w + x].is_some();
                let interior = (0..9).all(|k| cpu_hit[(y + k / 3 - 1) * w + x + k % 3 - 1].is_some() == c);
                if !interior {
                    continue;
                }
                checked += 1;
                let gt = gpu_t(x, y);
                let g = gt >= 0.0;
                if g != c {
                    if mask_bad < 3 {
                        let (ro, rd) = rays[y * w + x];
                        println!("  mask px ({x},{y}) gpu_hit={g} gpu_t={gt} cpu_t={:?}", cpu_hit[y * w + x]);
                        trace_march(vol, &cpu, ro, rd, pix_angle);
                    }
                    mask_bad += 1;
                    continue;
                }
                if cpu_hit[y * w + x].is_some() {
                    // the GPU's hit lies on the surface within its stopping cone
                    hits += 1;
                    let (ro, rd) = rays[y * w + x];
                    let p = [ro[0] + rd[0] * gt, ro[1] + rd[1] * gt, ro[2] + rd[2] * gt];
                    let (dist, _) = twin::eval(&prog.texels, p);
                    let eps = (CELL * 0.02).max(gt * pix_angle);
                    max_dt = max_dt.max(dist.abs());
                    if dist > 2.0 * eps + 0.01 || dist < -(eps + 0.05) {
                        if depth_bad < 2 {
                            println!("  bad px ({x},{y}) t={gt} dist={dist} eps={eps} cpu_t={:?}", cpu_hit[y * w + x]);
                            trace_march(vol, &cpu, ro, rd, pix_angle);
                        }
                        depth_bad += 1;
                    }
                }
            }
        }
        if std::env::var_os("SDFBAKE_ASCII").is_some() {
            for (x, y) in [(160, 120), (100, 120), (200, 100), (160, 150)] {
                println!("px ({x},{y}) gpu t={} cpu t={:?} gpu_slot={}", gpu_t(x, y), cpu_hit[y * w + x], f32::from_le_bytes([px[(y * w + x) * 16 + 4], px[(y * w + x) * 16 + 5], px[(y * w + x) * 16 + 6], px[(y * w + x) * 16 + 7]]));
            }
            for y in (0..h).step_by(12) {
                let row: String = (0..w).step_by(6).map(|x| match (gpu_t(x, y) >= 0.0, cpu_hit[y * w + x].is_some()) { (true, true) => '#', (true, false) => 'G', (false, true) => 'C', _ => '.' }).collect();
                println!("{row}");
            }
        }
        println!("SDFBAKE scene={} march {w}x{h} checked={checked} hits={hits} mask_bad={mask_bad} depth_bad={depth_bad} max_hit_dist={max_dt:.4}mm", sc.name);
        if hits == 0 || mask_bad * 1000 > checked || depth_bad * 1000 > hits.max(1) {
            self.fails.push(format!("{}-march", sc.name));
        }
        self.step = if bench() { Step::MarchBench(0) } else { Step::Edit };
    }

    fn next_scene(&mut self) {
        self.scene += 1;
        self.edited = false;
        self.step = if self.scene < self.scenes.len() { Step::Build } else { Step::Done };
    }

    fn take_prof(&mut self) {
        if let Some(rec) = gpu_prof::recording_of(None) {
            for f in rec.frames.iter().filter(|f| f.id > self.prof_from && !f.split) {
                for p in &f.passes {
                    if rec.s(p.name) == "sdf_bake" && p.end > p.start {
                        self.gpu_ms.push((p.end - p.start) as f64 / 1.0e6);
                    }
                    if rec.s(p.name) == "sdf_march" && p.end > p.start && matches!(self.step, Step::MarchBench(_)) {
                        self.march_ms.push((p.end - p.start) as f64 / 1.0e6);
                    }
                }
            }
            self.prof_from = rec.frames.last().map_or(self.prof_from, |f| f.id);
        }
    }
}

impl Widget for SdfBakeView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
            if bench() {
                gpu_prof::start(gpu_prof::StartOptions { keep_s: 20, split_every: 0, ..Default::default() });
            }
        }
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.frame += 1;
                if self.step == Step::Read {
                    self.check_read();
                }
                if self.step == Step::MarchRead {
                    self.check_march();
                }
                if bench() {
                    self.take_prof();
                }
                if self.step == Step::Done && selftest() {
                    if bench() {
                        let mut v = self.gpu_ms.clone();
                        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        let total: f64 = v.iter().sum();
                        println!("SDFBAKE-BENCH bake passes={} gpu_total_ms={total:.3} max_ms={:.3}", v.len(), v.last().copied().unwrap_or(0.0));
                    }
                    if self.fails.is_empty() {
                        println!("SDFBAKE-OK scenes={}", self.scenes.len());
                        std::process::exit(0);
                    }
                    println!("SDFBAKE-FAIL {}", self.fails.join(" "));
                    std::process::exit(1);
                }
                if self.frame > 6000 && selftest() {
                    println!("SDFBAKE-FAIL did not finish in {} frames (step {:?})", self.frame, self.step);
                    std::process::exit(1);
                }
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        if self.step != Step::Done {
            self.step_frame(cx);
        }
        // the atlas, shown so its pass paints
        if let Some(baker) = &self.baker {
            let rect = cx.turtle().rect();
            self.show.draw_super.draw_vars.set_texture(0, &baker.atlas);
            self.show.draw_super.draw_abs(cx, rect);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

fn selftest() -> bool {
    std::env::args().any(|a| a == "--selftest" || a == "--bench") || std::env::var_os("SDFBAKE_SELFTEST").is_some()
}

fn bench() -> bool {
    std::env::args().any(|a| a == "--bench")
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_sdf_engine::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
