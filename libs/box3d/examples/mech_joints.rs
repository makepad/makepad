//! Print-in-place mechanism prototype (research lane FABMECH, see local/agent_state/fab/MECH-DESIGN.md).
//!
//! Two assemblies are generated from joint declarations: an 8-link hinge chain and a 3-part puppet arm
//! (torso socket, shoulder ball, elbow hinge). Each joint op writes both mating halves and the clearance gap.
//! The example then:
//!   1. checks the exact 3D minimum gap between every joined pair from the distance fields (branch and bound),
//!   2. rasterises each joint region per layer (0.2 mm) for the in-layer gap and the fused-joint (tack) risk,
//!   3. derives the joint limits by sweeping the moving part through the field until the gap closes,
//!   4. builds a box3d world (mass and inertia integrated from the field, collision proxies from the
//!      joint-op primitives, joints from the declarations) and simulates dangling under gravity.
//! Units: mm, g, s. Run: cargo run --release -p makepad-box3d --example mech_joints

use makepad_box3d::body::*;
use makepad_box3d::hull::{create_hull, make_offset_box_hull};
use makepad_box3d::id::{BodyId, JointId};
use makepad_box3d::joint::*;
use makepad_box3d::math_functions::{make_quat_from_axis_angle, vec3, Matrix3, Quat, Transform, Vec3};
use makepad_box3d::physics_world::*;
use makepad_box3d::revolute_joint::*;
use makepad_box3d::shape::{create_hull_shape, create_sphere_shape};
use makepad_box3d::spherical_joint::*;
use makepad_box3d::types::*;

// ---------------------------------------------------------------- tiny SDF IR (stand-in for csg_sdf SdfNode)

#[derive(Clone, Copy, Debug)]
struct V {
    x: f64,
    y: f64,
    z: f64,
}
fn v(x: f64, y: f64, z: f64) -> V {
    V { x, y, z }
}
impl V {
    fn add(self, o: V) -> V {
        v(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    fn sub(self, o: V) -> V {
        v(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn mul(self, s: f64) -> V {
        v(self.x * s, self.y * s, self.z * s)
    }
    fn dot(self, o: V) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn f(self) -> Vec3 {
        vec3(self.x as f32, self.y as f32, self.z as f32)
    }
}

#[derive(Clone)]
enum F {
    Sphere(V, f64),
    Box(V, V),              // min, max
    Seg(V, V, f64),         // capped cylinder between two points
    Cone45Down(V),          // infinite 45 degree cone, apex on top, opening downward
    ConeAxis(V, V, f64),    // infinite cone: apex, unit axis, half angle
    Above(f64),             // half space z >= h
    Below(f64),             // half space z <= h
    Union(Vec<F>),
    Inter(Vec<F>),
    Sub(Box<F>, Vec<F>),
    Offset(Box<F>, f64),    // grow by r (shell outer, clearance)
    RotAxis(V, V, f64, Box<F>), // rotation about an arbitrary unit axis through a point
}

fn cyl_z(cx: f64, cy: f64, r: f64, z0: f64, z1: f64) -> F {
    F::Seg(v(cx, cy, z0), v(cx, cy, z1), r)
}

fn rot_axis(p: V, axis: V, ang: f64) -> V {
    // Rodrigues
    let (s, c) = ang.sin_cos();
    let k = axis;
    let kxp = v(k.y * p.z - k.z * p.y, k.z * p.x - k.x * p.z, k.x * p.y - k.y * p.x);
    p.mul(c).add(kxp.mul(s)).add(k.mul(k.dot(p) * (1.0 - c)))
}

impl F {
    fn d(&self, p: V) -> f64 {
        match self {
            F::Sphere(c, r) => p.sub(*c).len() - r,
            F::Box(lo, hi) => {
                let c = lo.add(*hi).mul(0.5);
                let h = hi.sub(*lo).mul(0.5);
                let q = p.sub(c);
                let q = v(q.x.abs() - h.x, q.y.abs() - h.y, q.z.abs() - h.z);
                v(q.x.max(0.0), q.y.max(0.0), q.z.max(0.0)).len() + q.x.max(q.y).max(q.z).min(0.0)
            }
            F::Seg(a, b, r) => {
                let ba = b.sub(*a);
                let pa = p.sub(*a);
                let baba = ba.dot(ba);
                let paba = pa.dot(ba);
                let x = pa.mul(baba).sub(ba.mul(paba)).len() - r * baba;
                let y = (paba - baba * 0.5).abs() - baba * 0.5;
                let x2 = x * x;
                let y2 = y * y * baba;
                let d = if x.max(y) < 0.0 {
                    -x2.min(y2)
                } else {
                    (if x > 0.0 { x2 } else { 0.0 }) + (if y > 0.0 { y2 } else { 0.0 })
                };
                d.signum() * d.abs().sqrt() / baba
            }
            F::ConeAxis(apex, axis, ha) => {
                let q = p.sub(*apex);
                let h = q.dot(*axis);
                let rr = q.sub(axis.mul(h)).len();
                rr * ha.cos() - h * ha.sin()
            }
            F::Cone45Down(apex) => {
                let q = p.sub(*apex);
                ((q.x * q.x + q.y * q.y).sqrt() + q.z) * std::f64::consts::FRAC_1_SQRT_2
            }
            F::Above(h) => h - p.z,
            F::Below(h) => p.z - h,
            F::Union(l) => l.iter().fold(f64::INFINITY, |m, f| m.min(f.d(p))),
            F::Inter(l) => l.iter().fold(f64::NEG_INFINITY, |m, f| m.max(f.d(p))),
            F::Sub(a, l) => l.iter().fold(a.d(p), |m, f| m.max(-f.d(p))),
            F::Offset(a, r) => a.d(p) - r,
            F::RotAxis(c, axis, ang, a) => a.d(c.add(rot_axis(p.sub(*c), *axis, -ang))),
        }
    }
}

#[derive(Clone)]
struct Part {
    name: String,
    f: F,
    lo: V,
    hi: V,
    proxies: Vec<F>, // convex primitives for box3d (Box, Seg along z, Sphere)
}

// ---------------------------------------------------------------- joint ops

/// Print-in-place hinge with the pin along print Z. A gets the fork (two plates + pin), B gets the eye.
/// Clearances: radial c_r in every layer, axial c_z between plates and eye (whole layers).
#[derive(Clone, Copy)]
struct Hinge {
    cx: f64,
    cy: f64,
    z0: f64,
    z1: f64,
    r_pin: f64,
    r_plate: f64,
    t_plate: f64,
    c_r: f64,
    c_z: f64,
}
impl Hinge {
    fn auto(cx: f64, cy: f64, z0: f64, z1: f64, layer: f64, nozzle: f64) -> Hinge {
        // Defaults from the rules in MECH-DESIGN.md section 1: radial gap >= 0.35 for a 0.4 nozzle,
        // axial gap = 2 whole layers, plates >= 4 layers, pin >= 5 extrusion widths across.
        let t = z1 - z0;
        let c_z = 2.0 * layer;
        let t_plate = ((t * 0.2) / layer).round().max(4.0) * layer;
        let r_pin = (t * 0.2).max(2.5 * nozzle).min(3.0);
        Hinge { cx, cy, z0, z1, r_pin, r_plate: r_pin + 2.0, t_plate, c_r: 0.35_f64.max(0.875 * nozzle), c_z }
    }
    fn eye_band(&self) -> (f64, f64) {
        (self.z0 + self.t_plate + self.c_z, self.z1 - self.t_plate - self.c_z)
    }
    fn apply_a(&self, a: F) -> F {
        let cut = cyl_z(self.cx, self.cy, self.r_plate + self.c_r, self.z0 + self.t_plate, self.z1 - self.t_plate);
        F::Union(vec![
            F::Sub(Box::new(a), vec![cut]),
            cyl_z(self.cx, self.cy, self.r_plate, self.z0, self.z0 + self.t_plate),
            cyl_z(self.cx, self.cy, self.r_plate, self.z1 - self.t_plate, self.z1),
            cyl_z(self.cx, self.cy, self.r_pin, self.z0, self.z1),
        ])
    }
    fn apply_b(&self, b: F) -> F {
        let (e0, e1) = self.eye_band();
        let r = self.r_plate + self.c_r;
        let body = F::Sub(
            Box::new(b),
            vec![cyl_z(self.cx, self.cy, r, self.z0 - 1.0, e0), cyl_z(self.cx, self.cy, r, e1, self.z1 + 1.0)],
        );
        F::Sub(
            Box::new(F::Union(vec![body, cyl_z(self.cx, self.cy, self.r_plate, e0, e1)])),
            vec![cyl_z(self.cx, self.cy, self.r_pin + self.c_r, self.z0 - 1.0, self.z1 + 1.0)],
        )
    }
}

/// Captive ball and socket. A owns the socket (shell = offset of the void), B owns the ball and a neck
/// that leaves through an opening along `axis`. The void gets a 45 degree teardrop roof so its top never
/// overhangs; the shell is clipped to the bed.
#[derive(Clone, Copy)]
struct Ball {
    c: V,
    r: f64,
    clr: f64,
    wall: f64,
    axis: V,
    rn: f64,
    cone: f64,  // declared cone limit (radians); the opening is the neck swept over it
    z_top: f64, // A's top face (the shell is clipped to the bed and to this)
}
impl Ball {
    /// Largest radius of the clipped shell from the centre: B's body stays outside this sphere (+ clearance),
    /// so it can rotate freely about the centre without touching the shell.
    fn shell_max_r(&self) -> f64 {
        let rs = self.r + self.clr + self.wall;
        let tip = self.c.z + (self.r + self.clr) * std::f64::consts::SQRT_2 + self.wall * std::f64::consts::SQRT_2;
        let dz = self.z_top - self.c.z;
        rs.max((dz * dz + (tip - self.z_top).max(0.0).powi(2)).sqrt())
    }
    /// Radial overlap that keeps the ball captive: r minus the opening's radius at the inner shell surface.
    fn retention(&self) -> f64 {
        let rc = self.r + self.clr;
        let open = (self.cone + ((self.rn + self.clr) / rc).asin()).min(std::f64::consts::FRAC_PI_2);
        self.r - rc * open.sin()
    }
    fn void(&self) -> F {
        let rc = self.r + self.clr;
        let s2 = std::f64::consts::SQRT_2;
        F::Union(vec![
            F::Sphere(self.c, rc),
            F::Inter(vec![F::Cone45Down(self.c.add(v(0.0, 0.0, rc * s2))), F::Above(self.c.z + rc / s2)]),
        ])
    }
    fn reach(&self) -> f64 {
        self.shell_max_r() + self.clr
    }
    fn apply_a(&self, a: F) -> F {
        let z_top = self.z_top;
        let opening = F::Offset(Box::new(F::ConeAxis(self.c, self.axis, self.cone)), self.rn + self.clr);
        F::Inter(vec![
            F::Sub(Box::new(F::Union(vec![a, F::Offset(Box::new(self.void()), self.wall)])), vec![self.void(), opening]),
            F::Above(0.0),
            F::Below(z_top),
        ])
    }
    fn apply_b(&self, b: F) -> F {
        F::Union(vec![
            F::Sub(Box::new(b), vec![F::Sphere(self.c, self.reach())]),
            F::Sphere(self.c, self.r),
            F::Seg(self.c, self.c.add(self.axis.mul(self.reach() + 0.5)), self.rn),
        ])
    }
}

// ---------------------------------------------------------------- field queries

/// Minimum gap between two parts: 2 * min over space of max(dA, dB). The fields are 1-Lipschitz and never
/// overestimate the true distance (min/max CSG), so a field gap never exceeds the true gap. Branch and bound
/// over a region, cells pruned by the Lipschitz bound; the search itself may overstate by <= 1.8 * tol.
/// Returns (gap, location, evaluations).
fn min_gap(a: &F, b: &F, lo: V, hi: V, tol: f64) -> (f64, V, u64) {
    let mut best = f64::INFINITY;
    let mut at = lo;
    let mut evals = 0u64;
    let s0 = 1.0;
    let n = |l: f64, h: f64| (((h - l) / s0).ceil() as i32).max(1);
    let (nx, ny, nz) = (n(lo.x, hi.x), n(lo.y, hi.y), n(lo.z, hi.z));
    let mut stack: Vec<(V, f64)> = Vec::new();
    for i in 0..nx {
        for j in 0..ny {
            for k in 0..nz {
                let c = v(lo.x + (i as f64 + 0.5) * s0, lo.y + (j as f64 + 0.5) * s0, lo.z + (k as f64 + 0.5) * s0);
                stack.push((c, s0 * 0.5));
            }
        }
    }
    let sq3 = 3f64.sqrt();
    while let Some((c, h)) = stack.pop() {
        let g = a.d(c).max(b.d(c));
        evals += 1;
        if g < best {
            best = g;
            at = c;
        }
        if h * 2.0 > tol && g - h * sq3 < best - tol * 0.25 {
            let q = h * 0.5;
            for dx in [-q, q] {
                for dy in [-q, q] {
                    for dz in [-q, q] {
                        stack.push((v(c.x + dx, c.y + dy, c.z + dz), q));
                    }
                }
            }
        }
    }
    (2.0 * best, at, evals)
}

fn overlap_box(a: &Part, b: &Part, pad: f64) -> Option<(V, V)> {
    let lo = v(a.lo.x.max(b.lo.x) - pad, a.lo.y.max(b.lo.y) - pad, a.lo.z.max(b.lo.z) - pad);
    let hi = v(a.hi.x.min(b.hi.x) + pad, a.hi.y.min(b.hi.y) + pad, a.hi.z.min(b.hi.z) + pad);
    if lo.x < hi.x && lo.y < hi.y && lo.z < hi.z {
        Some((lo, hi))
    } else {
        None
    }
}

/// Per-layer slicer view of one joint region: in-layer gap between the two parts' cross sections and
/// tack area (cells of one part printed over air with the other part only n empty layers below).
struct LayerReport {
    min_layer_gap: f64,
    min_layer_z: f64,
    tack: [f64; 4], // mm^2 by empty layers between: [0 = resting on the other part, 1, 2, 3]
}

fn layer_check(a: &F, b: &F, lo: V, hi: V, layer: f64, res: f64) -> LayerReport {
    let nx = ((hi.x - lo.x) / res).ceil() as usize;
    let ny = ((hi.y - lo.y) / res).ceil() as usize;
    let nl = ((hi.z.max(0.0)) / layer).ceil() as usize;
    let k0 = ((lo.z.max(0.0)) / layer).floor() as usize;
    // occupancy per layer: 0 empty, 1 a, 2 b
    let mut occ: Vec<Vec<u8>> = Vec::new();
    for k in k0..nl {
        let z = (k as f64 + 0.5) * layer;
        let mut row = vec![0u8; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let p = v(lo.x + (i as f64 + 0.5) * res, lo.y + (j as f64 + 0.5) * res, z);
                row[j * nx + i] = if a.d(p) < 0.0 { 1 } else if b.d(p) < 0.0 { 2 } else { 0 };
            }
        }
        occ.push(row);
    }
    // in-layer gap: boundary cells of a vs boundary cells of b, within 1 mm
    let mut min_layer_gap = f64::INFINITY;
    let mut min_layer_z = 0.0;
    let reach = (1.0 / res).ceil() as isize;
    for (kk, row) in occ.iter().enumerate() {
        let bnd = |m: u8| -> Vec<(isize, isize)> {
            let mut out = Vec::new();
            for j in 0..ny {
                for i in 0..nx {
                    if row[j * nx + i] != m {
                        continue;
                    }
                    let edge = [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)].iter().any(|(di, dj)| {
                        let (ii, jj) = (i as isize + di, j as isize + dj);
                        ii < 0 || jj < 0 || ii >= nx as isize || jj >= ny as isize || row[jj as usize * nx + ii as usize] != m
                    });
                    if edge {
                        out.push((i as isize, j as isize));
                    }
                }
            }
            out
        };
        let ba = bnd(1);
        let bb = bnd(2);
        if ba.is_empty() || bb.is_empty() {
            continue;
        }
        // bucket b
        let bs = reach.max(1);
        let mut buckets: std::collections::HashMap<(isize, isize), Vec<(isize, isize)>> = Default::default();
        for &c in &bb {
            buckets.entry((c.0 / bs, c.1 / bs)).or_default().push(c);
        }
        for &(i, j) in &ba {
            for di in -1..=1 {
                for dj in -1..=1 {
                    if let Some(l) = buckets.get(&(i / bs + di, j / bs + dj)) {
                        for &(bi, bj) in l {
                            // centre-to-centre distance minus one cell = gap estimate (boundary cells are inside)
                            let d = (((bi - i) * (bi - i) + (bj - j) * (bj - j)) as f64).sqrt() * res - res;
                            if d < min_layer_gap {
                                min_layer_gap = d;
                                min_layer_z = (k0 + kk) as f64 * layer;
                            }
                        }
                    }
                }
            }
        }
    }
    // tack: a cell of part X at layer k with no own support within the 45 degree reach (one layer of
    // lateral offset = `layer` mm) in layer k-1, and the nearest occupied cell below is the other part.
    let mut tack = [0.0f64; 4];
    let reach_s = (layer / res).round() as isize;
    for kk in 1..occ.len() {
        for j in 0..ny {
            for i in 0..nx {
                let m = occ[kk][j * nx + i];
                if m == 0 {
                    continue;
                }
                let mut supported = false;
                'n: for dj in -reach_s..=reach_s {
                    for di in -reach_s..=reach_s {
                        let (ii, jj) = (i as isize + di, j as isize + dj);
                        if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny && occ[kk - 1][jj as usize * nx + ii as usize] == m {
                            supported = true;
                            break 'n;
                        }
                    }
                }
                if supported {
                    continue;
                }
                let mut empty = 0;
                let mut k = kk as isize - 1;
                while k >= 0 && occ[k as usize][j * nx + i] == 0 && empty < 4 {
                    empty += 1;
                    k -= 1;
                }
                if k >= 0 && empty < 4 {
                    let below = occ[k as usize][j * nx + i];
                    if below != 0 && below != m {
                        tack[empty] += res * res;
                    }
                }
            }
        }
    }
    LayerReport { min_layer_gap, min_layer_z, tack }
}

/// Sweep the B part about an axis until the gap closes; returns the angle (radians) where it first does.
fn sweep_limit(a: &F, b: &F, c: V, axis: V, dir: f64, lo: V, hi: V, contact: f64) -> f64 {
    let step = 2f64.to_radians();
    let posed = |ang: f64| F::RotAxis(c, axis, ang * dir, Box::new(b.clone()));
    let mut prev = 0.0;
    let mut ang = step;
    while ang <= std::f64::consts::PI {
        let (g, _, _) = min_gap(a, &posed(ang), lo, hi, 0.05);
        if g < contact {
            // bisect between prev and ang
            let (mut l, mut h) = (prev, ang);
            for _ in 0..6 {
                let m = 0.5 * (l + h);
                if min_gap(a, &posed(m), lo, hi, 0.05).0 < contact {
                    h = m
                } else {
                    l = m
                }
            }
            return l;
        }
        prev = ang;
        ang += step;
    }
    std::f64::consts::PI
}

/// Volume, centre of mass and inertia (about the COM) by voxel integration of the field.
fn mass_props(p: &Part, h: f64, density: f64) -> (f64, V, [[f64; 3]; 3]) {
    let mut pts = Vec::new();
    let n = |l: f64, hh: f64| ((hh - l) / h).ceil() as usize;
    for i in 0..n(p.lo.x, p.hi.x) {
        for j in 0..n(p.lo.y, p.hi.y) {
            for k in 0..n(p.lo.z, p.hi.z) {
                let q = v(p.lo.x + (i as f64 + 0.5) * h, p.lo.y + (j as f64 + 0.5) * h, p.lo.z + (k as f64 + 0.5) * h);
                if p.f.d(q) < 0.0 {
                    pts.push(q);
                }
            }
        }
    }
    let dv = h * h * h;
    let vol = pts.len() as f64 * dv;
    let mut c = v(0.0, 0.0, 0.0);
    for q in &pts {
        c = c.add(*q);
    }
    c = c.mul(1.0 / pts.len() as f64);
    let dm = dv * density;
    let mut i = [[0.0; 3]; 3];
    for q in &pts {
        let r = q.sub(c);
        let rr = [r.x, r.y, r.z];
        let r2 = r.dot(r);
        for a in 0..3 {
            for b in 0..3 {
                i[a][b] += dm * ((if a == b { r2 } else { 0.0 }) - rr[a] * rr[b]);
            }
        }
    }
    // voxel self-inertia (each voxel a small cube)
    let s = dm * h * h / 6.0 * pts.len() as f64;
    for a in 0..3 {
        i[a][a] += s;
    }
    (vol, c, i)
}

// ---------------------------------------------------------------- box3d assembly

struct Sim {
    world: World,
}

fn mm_world() -> World {
    let mut def = default_world_def();
    def.gravity = vec3(0.0, -9810.0, 0.0); // print frame: bed is xy, gravity in-plane (held upright)
    def.maximum_linear_speed = 400_000.0;
    def.contact_speed = 3000.0;
    create_world(&def)
}

fn add_body(sim: &mut Sim, part: &Part, dynamic: bool, density: f64) -> (BodyId, V) {
    let (vol, com, inertia) = mass_props(part, 0.2, density);
    let mut bd = default_body_def();
    bd.body_type = if dynamic { BodyType::Dynamic } else { BodyType::Static };
    bd.position = com.f();
    let body = create_body(&mut sim.world, &bd);
    let mut sd = default_shape_def();
    sd.density = density as f32;
    sd.base_material.friction = 0.35;
    sd.enable_contact_events = true;
    sd.update_body_mass = false;
    for pr in &part.proxies {
        match pr {
            F::Box(lo, hi) => {
                let c = lo.add(*hi).mul(0.5).sub(com);
                let h = hi.sub(*lo).mul(0.5);
                let hull = make_offset_box_hull(h.x as f32, h.y as f32, h.z as f32, c.f());
                create_hull_shape(&mut sim.world, body, &sd, &hull);
            }
            F::Seg(a, b, r) => {
                let mut pts = Vec::new();
                for k in 0..16 {
                    let t = k as f64 / 16.0 * std::f64::consts::TAU;
                    let o = v(r * t.cos(), r * t.sin(), 0.0);
                    pts.push(a.add(o).sub(com).f());
                    pts.push(b.add(o).sub(com).f());
                }
                let hull = create_hull(&pts, 64).expect("hull");
                create_hull_shape(&mut sim.world, body, &sd, &hull);
            }
            F::Sphere(c, r) => {
                create_sphere_shape(&mut sim.world, body, &sd, &Sphere { center: c.sub(com).f(), radius: *r as f32 });
            }
            _ => panic!("proxy must be a primitive"),
        }
    }
    if dynamic {
        let m = vol * density;
        let ix = Matrix3 {
            cx: vec3(inertia[0][0] as f32, inertia[1][0] as f32, inertia[2][0] as f32),
            cy: vec3(inertia[0][1] as f32, inertia[1][1] as f32, inertia[2][1] as f32),
            cz: vec3(inertia[0][2] as f32, inertia[1][2] as f32, inertia[2][2] as f32),
        };
        body_set_mass_data(&mut sim.world, body, MassData { mass: m as f32, center: Vec3::ZERO, inertia: ix });
    }
    (body, com)
}

fn frame(p: V, com: V, q: Quat) -> Transform {
    Transform { p: p.sub(com).f(), q }
}

// ---------------------------------------------------------------- report helpers

fn risk_class(min_gap3d: f64, layer_gap: f64, tack: &[f64; 4]) -> (&'static str, f64) {
    // Provisional weights: tack over 1 empty layer bonds hard, over 2 lightly, over 3 rarely.
    let score = tack[0] * 3.0 + tack[1] * 1.0 + tack[2] * 0.3 + tack[3] * 0.05;
    let class = if min_gap3d <= 0.0 || tack[0] > 0.0 || layer_gap < 0.15 {
        "FUSED"
    } else if layer_gap < 0.25 || score > 40.0 {
        "high"
    } else if score > 10.0 {
        "medium"
    } else {
        "low"
    };
    (class, score)
}

fn check_joint(name: &str, a: &Part, b: &Part, layer: f64) -> (f64, f64) {
    let (lo, hi) = overlap_box(a, b, 0.5).expect("parts overlap in bounds");
    let t = std::time::Instant::now();
    let (g, at, evals) = min_gap(&a.f, &b.f, lo, hi, 0.01);
    let t3 = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let lr = layer_check(&a.f, &b.f, lo, hi, layer, 0.05);
    let tl = t.elapsed().as_secs_f64() * 1e3;
    let (class, score) = risk_class(g, lr.min_layer_gap, &lr.tack);
    println!(
        "  joint {:<16} {}-{} min gap {:.3} mm at ({:.2},{:.2},{:.2}) [{} evals, {:.0} ms] | in-layer gap {:.2} mm at z {:.1} | tack mm2 n0 {:.1} n1 {:.1} n2 {:.1} n3 {:.1} [{:.0} ms] | fuse risk {} ({:.1})",
        name, a.name, b.name, g, at.x, at.y, at.z, evals, t3, lr.min_layer_gap, lr.min_layer_z, lr.tack[0], lr.tack[1], lr.tack[2], lr.tack[3], tl, class, score
    );
    (g, score)
}

// ---------------------------------------------------------------- assemblies

const LAYER: f64 = 0.2;
const NOZZLE: f64 = 0.4;
const PLA: f64 = 1.24e-3; // g / mm^3

fn chain(n: usize, pitch: f64, w: f64, t: f64) -> (Vec<Part>, Vec<Hinge>) {
    let hinges: Vec<Hinge> = (0..n).map(|i| Hinge::auto((i + 1) as f64 * pitch, 0.0, 0.0, t, LAYER, NOZZLE)).collect();
    let mut parts = Vec::new();
    for i in 0..n {
        let x0 = i as f64 * pitch;
        let mut f = F::Box(v(x0, -w * 0.5, 0.0), v(x0 + pitch, w * 0.5, t));
        if i > 0 {
            f = hinges[i - 1].apply_b(f);
        } else {
            // the first link hangs from a fixed anchor pin through its eye
            let anchor = Hinge::auto(0.0, 0.0, 0.0, t, LAYER, NOZZLE);
            f = anchor.apply_b(f);
        }
        if i + 1 < n {
            f = hinges[i].apply_a(f);
        }
        let h = &hinges[i];
        let proxies = vec![
            F::Box(v(x0 + h.r_plate + h.c_r, -w * 0.5, 0.0), v(x0 + pitch - h.r_plate - h.c_r, w * 0.5, t)),
            cyl_z(x0, 0.0, h.r_plate, h.eye_band().0, h.eye_band().1),
        ];
        let mut proxies = proxies;
        if i + 1 < n {
            proxies.push(cyl_z(x0 + pitch, 0.0, h.r_plate, 0.0, t));
        }
        let r = h.r_plate + 0.5;
        parts.push(Part { name: format!("link[{}]", i), f, lo: v(x0 - r, -r, 0.0), hi: v(x0 + pitch + r, r, t), proxies });
    }
    (parts, hinges)
}

fn puppet() -> (Part, Part, Part, Ball, Hinge) {
    let torso_top = 15.0;
    let s = v(13.5, 20.0, 7.5);
    let ball = Ball { c: s, r: 5.5, clr: 0.3, wall: 2.0, axis: v(0.0, -1.0, 0.0), rn: 2.0, cone: 35f64.to_radians(), z_top: torso_top };
    let arm_t = 11.0;
    let elbow = Hinge::auto(13.5, 6.0, 0.0, arm_t, LAYER, NOZZLE);
    let torso_f = ball.apply_a(F::Box(v(-8.0, 0.0, 0.0), v(8.0, 26.0, torso_top)));
    let upper_f = elbow.apply_a(ball.apply_b(F::Box(v(11.0, 6.0, 0.0), v(16.0, 20.0, arm_t))));
    let fore_f = elbow.apply_b(F::Box(v(11.0, -8.0, 0.0), v(16.0, 6.0, arm_t)));
    let rs = ball.shell_max_r();
    let torso = Part {
        name: "torso".into(),
        f: torso_f,
        lo: v(-8.0, 0.0, 0.0),
        hi: v(s.x + rs + 0.5, s.y + rs + 0.5, torso_top),
        proxies: vec![F::Box(v(-8.0, 0.0, 0.0), v(8.0, 26.0, torso_top))],
    };
    let upper = Part {
        name: "upper_arm".into(),
        f: upper_f,
        lo: v(9.0, 1.5, 0.0),
        hi: v(18.0, 26.0, arm_t),
        proxies: vec![
            F::Box(v(11.0, 6.0 + elbow.r_plate + elbow.c_r, 0.0), v(16.0, s.y - ball.reach(), arm_t)),
            cyl_z(13.5, 6.0, elbow.r_plate, 0.0, arm_t),
        ],
    };
    let fore = Part {
        name: "forearm".into(),
        f: fore_f,
        lo: v(9.0, -8.5, 0.0),
        hi: v(18.0, 10.5, arm_t),
        proxies: vec![
            F::Box(v(11.0, -8.0, 0.0), v(16.0, 6.0 - elbow.r_plate - elbow.c_r, arm_t)),
            cyl_z(13.5, 6.0, elbow.r_plate, elbow.eye_band().0, elbow.eye_band().1),
        ],
    };
    (torso, upper, fore, ball, elbow)
}

// ---------------------------------------------------------------- main

fn main() {
    println!("== FABMECH prototype: print-in-place joints from SDF joint ops, layer {} mm, nozzle {} mm", LAYER, NOZZLE);

    // ---------------- chain
    let n = 8;
    let (links, hinges) = chain(n, 12.0, 5.0, 6.0);
    let h0 = hinges[0];
    println!(
        "chain: {} links, pitch 12 mm, 5 x 6 mm section; auto hinge r_pin {:.2} r_plate {:.2} t_plate {:.2} c_r {:.2} c_z {:.2}",
        n, h0.r_pin, h0.r_plate, h0.t_plate, h0.c_r, h0.c_z
    );
    let mut worst_gap: f64 = f64::INFINITY;
    for i in 0..n - 1 {
        let (g, _) = check_joint(&format!("hinge {}-{}", i, i + 1), &links[i], &links[i + 1], LAYER);
        worst_gap = worst_gap.min(g);
    }
    // non-adjacent pairs must not touch either
    let (g02, _, _) = min_gap(&links[0].f, &links[2].f, links[0].lo, links[2].hi, 0.02);
    println!("  non-adjacent link0-link2 min gap {:.3} mm", g02);

    // limits from the field: sweep link 1 about hinge 0's pin
    let t = std::time::Instant::now();
    let (lo, hi) = (links[0].lo, links[0].hi);
    let pin = v(h0.cx, h0.cy, 0.0);
    let up = sweep_limit(&links[0].f, &links[1].f, pin, v(0.0, 0.0, 1.0), 1.0, lo, hi, 0.02);
    let dn = sweep_limit(&links[0].f, &links[1].f, pin, v(0.0, 0.0, 1.0), -1.0, lo, hi, 0.02);
    println!(
        "  hinge limits from field sweep: +{:.1} / -{:.1} deg [{:.0} ms]",
        up.to_degrees(),
        dn.to_degrees(),
        t.elapsed().as_secs_f64() * 1e3
    );

    for &mu in &[0.0, 0.35] {
        let mut sim = Sim { world: mm_world() };
        let ground = create_body(&mut sim.world, &default_body_def());
        let mut ids = Vec::new();
        let mut coms = Vec::new();
        let mut total_mass = 0.0;
        for l in &links {
            let (b, c) = add_body(&mut sim, l, true, PLA);
            total_mass += body_get_mass(&sim.world, b) as f64;
            ids.push(b);
            coms.push(c);
        }
        let mut joints: Vec<JointId> = Vec::new();
        for i in 0..n {
            let mut jd = default_revolute_joint_def();
            let pivot = v(i as f64 * 12.0, 0.0, 3.0);
            if i == 0 {
                jd.base.body_id_a = ground;
                jd.base.local_frame_a = Transform { p: pivot.f(), q: Quat::IDENTITY };
            } else {
                jd.base.body_id_a = ids[i - 1];
                jd.base.local_frame_a = frame(pivot, coms[i - 1], Quat::IDENTITY);
                jd.enable_limit = true;
                jd.lower_angle = -dn as f32;
                jd.upper_angle = up as f32;
            }
            jd.base.body_id_b = ids[i];
            jd.base.local_frame_b = frame(pivot, coms[i], Quat::IDENTITY);
            jd.base.collide_connected = false;
            // pin friction: mu * r_pin * hanging load below this pin
            let load = (n - i) as f64 * total_mass / n as f64 * 9810.0;
            jd.enable_motor = mu > 0.0;
            jd.motor_speed = 0.0;
            jd.max_motor_torque = (mu * h0.r_pin * load) as f32;
            joints.push(create_revolute_joint(&mut sim.world, &jd));
        }
        let dt: f32 = 1.0 / 240.0;
        let mut settle = 0.0;
        let mut max_ang: f64 = 0.0;
        let mut limit_hits = 0;
        let mut contacts = 0;
        let mut min_dyn: f64 = 1e9;
        let mut swing = (f64::INFINITY, f64::NEG_INFINITY);
        let t = std::time::Instant::now();
        let steps = 240 * 12;
        for s in 0..steps {
            world_step(&mut sim.world, dt, 4);
            contacts += world_get_contact_events(&sim.world).begin_events.len();
            let mut wmax: f64 = 0.0;
            for b in &ids {
                let w = body_get_angular_velocity(&sim.world, *b);
                wmax = wmax.max(((w.x * w.x + w.y * w.y + w.z * w.z) as f64).sqrt());
            }
            if wmax > 0.2 {
                settle = (s + 1) as f64 * dt as f64;
            }
            for j in joints.iter().skip(1) {
                let a = revolute_joint_get_angle(&mut sim.world, *j) as f64;
                max_ang = max_ang.max(a.abs());
                if a > up - 0.02 || a < -dn + 0.02 {
                    limit_hits += 1;
                }
            }
            let tip = body_get_position(&sim.world, ids[n - 1]);
            min_dyn = min_dyn.min(tip.y as f64);
            if s >= steps - 240 {
                let a0 = revolute_joint_get_angle(&mut sim.world, joints[0]) as f64;
                swing = (swing.0.min(a0), swing.1.max(a0));
            }
        }
        let wall = t.elapsed().as_secs_f64() * 1e3;
        let tip = body_get_position(&sim.world, ids[n - 1]);
        let anchor_f = joint_get_constraint_force(&sim.world, joints[0]);
        let weight = total_mass * 9810.0;
        let mut finals = String::new();
        for j in joints.iter().skip(1) {
            finals += &format!("{:.1} ", (revolute_joint_get_angle(&mut sim.world, *j) as f64).to_degrees());
        }
        let a0 = (revolute_joint_get_angle(&mut sim.world, joints[0]) as f64).to_degrees();
        println!(
            "  sim mu={:.2}: mass {:.3} g, released flat, 12 s at 240 Hz x4 [{:.0} ms wall] | settle {:.2} s | tip lowest y {:.1}, final tip ({:.1},{:.1}) expected (~0,{:.1}) | anchor angle {:.1} deg (last-second swing {:.1}..{:.1}), link angles {}deg | max |angle| {:.1} deg, limit-contact steps {}, self-contacts {} | anchor force {:.0} vs weight {:.0} (uN)",
            mu, total_mass, wall, settle, min_dyn, tip.x, tip.y, -(n as f64 - 0.5) * 12.0, a0, swing.0.to_degrees(), swing.1.to_degrees(), finals,
            max_ang.to_degrees(), limit_hits, contacts,
            ((anchor_f.x * anchor_f.x + anchor_f.y * anchor_f.y + anchor_f.z * anchor_f.z) as f64).sqrt(), weight
        );
    }

    // ---------------- puppet arm
    let (torso, upper, fore, ball, elbow) = puppet();
    println!(
        "puppet arm: shoulder ball r {} clr {} wall {} neck {}; elbow auto hinge r_pin {:.2} c_r {:.2} c_z {:.2}",
        ball.r, ball.clr, ball.wall, ball.rn, elbow.r_pin, elbow.c_r, elbow.c_z
    );
    check_joint("shoulder ball", &torso, &upper, LAYER);
    check_joint("elbow hinge", &upper, &fore, LAYER);
    let (gtf, _, _) = min_gap(&torso.f, &fore.f, v(-9.0, -9.0, 0.0), v(22.0, 27.0, 15.0), 0.02);
    println!("  non-adjacent torso-forearm min gap {:.3} mm", gtf);
    let t = std::time::Instant::now();
    let (blo, bhi) = (torso.lo, torso.hi);
    let cone_z = sweep_limit(&torso.f, &upper.f, ball.c, v(0.0, 0.0, 1.0), 1.0, blo, bhi, 0.02);
    let body_z = sweep_limit(&torso.f, &upper.f, ball.c, v(0.0, 0.0, 1.0), -1.0, blo, bhi, 0.02);
    let cone_x = sweep_limit(&torso.f, &upper.f, ball.c, v(1.0, 0.0, 0.0), 1.0, blo, bhi, 0.02);
    let cone_xn = sweep_limit(&torso.f, &upper.f, ball.c, v(1.0, 0.0, 0.0), -1.0, blo, bhi, 0.02);
    println!(
        "  shoulder: declared cone {:.0} deg, captive retention {:.2} mm, field limits: outward {:.1}, forward {:.1}, back {:.1}, toward torso {:.1} deg (body contact, left to collision)",
        ball.cone.to_degrees(), ball.retention(), cone_z.to_degrees(), cone_x.to_degrees(), cone_xn.to_degrees(), body_z.to_degrees()
    );
    let (elo, ehi) = (upper.lo, upper.hi);
    let el_up = sweep_limit(&upper.f, &fore.f, v(13.5, 6.0, 0.0), v(0.0, 0.0, 1.0), 1.0, elo, ehi, 0.02);
    let el_dn = sweep_limit(&upper.f, &fore.f, v(13.5, 6.0, 0.0), v(0.0, 0.0, 1.0), -1.0, elo, ehi, 0.02);
    println!(
        "  limits from field: shoulder cone {:.1} deg (tilt about z) / {:.1} deg (tilt about x), elbow +{:.1} / -{:.1} deg [{:.0} ms]",
        cone_z.to_degrees(), cone_x.to_degrees(), el_up.to_degrees(), el_dn.to_degrees(), t.elapsed().as_secs_f64() * 1e3
    );
    let cone = cone_z.min(cone_x).min(cone_xn);
    for &mu in &[0.0, 0.35] {
        let mut sim = Sim { world: mm_world() };
        let (tb, tc) = add_body(&mut sim, &torso, false, PLA);
        let (ub, uc) = add_body(&mut sim, &upper, true, PLA);
        let (fb, fc) = add_body(&mut sim, &fore, true, PLA);
        let mu_m = body_get_mass(&sim.world, ub) as f64;
        let mf_m = body_get_mass(&sim.world, fb) as f64;
        let mut sd = default_spherical_joint_def();
        let down = make_quat_from_axis_angle(vec3(1.0, 0.0, 0.0), std::f32::consts::FRAC_PI_2);
        sd.base.body_id_a = tb;
        sd.base.body_id_b = ub;
        sd.base.local_frame_a = frame(ball.c, tc, down);
        sd.base.local_frame_b = frame(ball.c, uc, down);
        sd.base.collide_connected = true; // joint-internal proxies are omitted; limb vs torso must collide
        sd.enable_cone_limit = true;
        sd.cone_angle = cone as f32;
        sd.enable_motor = mu > 0.0;
        sd.motor_velocity = Vec3::ZERO;
        sd.max_motor_torque = (mu * ball.r * (mu_m + mf_m) * 9810.0) as f32;
        let sj = create_spherical_joint(&mut sim.world, &sd);
        let mut rd = default_revolute_joint_def();
        rd.base.body_id_a = ub;
        rd.base.body_id_b = fb;
        let ep = v(13.5, 6.0, 5.5);
        rd.base.local_frame_a = frame(ep, uc, Quat::IDENTITY);
        rd.base.local_frame_b = frame(ep, fc, Quat::IDENTITY);
        rd.enable_limit = true;
        rd.lower_angle = -el_dn as f32;
        rd.upper_angle = el_up as f32;
        rd.enable_motor = mu > 0.0;
        rd.max_motor_torque = (mu * elbow.r_pin * mf_m * 9810.0) as f32;
        let ej = create_revolute_joint(&mut sim.world, &rd);
        // start raised: the whole arm lifted 25 degrees outward about the shoulder, elbow bent 30 degrees, then released
        let lift = 25f64.to_radians();
        let zq = |ang: f64| make_quat_from_axis_angle(vec3(0.0, 0.0, 1.0), ang as f32);
        let up_p = ball.c.add(rot_axis(uc.sub(ball.c), v(0.0, 0.0, 1.0), lift));
        body_set_transform(&mut sim.world, ub, up_p.f(), zq(lift));
        let ep_w = ball.c.add(rot_axis(ep.sub(ball.c), v(0.0, 0.0, 1.0), lift));
        let bend = 30f64.to_radians();
        let fc_p = ep_w.add(rot_axis(ball.c.add(rot_axis(fc.sub(ball.c), v(0.0, 0.0, 1.0), lift)).sub(ep_w), v(0.0, 0.0, 1.0), bend));
        body_set_transform(&mut sim.world, fb, fc_p.f(), zq(lift + bend));
        let dt: f32 = 1.0 / 240.0;
        let mut settle = 0.0;
        let mut max_cone: f64 = 0.0;
        let mut el_range = (0.0f64, 0.0f64);
        let mut contacts = 0;
        let t = std::time::Instant::now();
        for s in 0..240 * 12 {
            world_step(&mut sim.world, dt, 4);
            contacts += world_get_contact_events(&sim.world).begin_events.len();
            let c = spherical_joint_get_cone_angle(&mut sim.world, sj) as f64;
            max_cone = max_cone.max(c);
            let e = revolute_joint_get_angle(&mut sim.world, ej) as f64;
            el_range = (el_range.0.min(e), el_range.1.max(e));
            let mut wmax: f64 = 0.0;
            for b in [ub, fb] {
                let w = body_get_angular_velocity(&sim.world, b);
                wmax = wmax.max(((w.x * w.x + w.y * w.y + w.z * w.z) as f64).sqrt());
            }
            if wmax > 0.2 {
                settle = (s + 1) as f64 * dt as f64;
            }
        }
        let wall = t.elapsed().as_secs_f64() * 1e3;
        let fc_now = spherical_joint_get_cone_angle(&mut sim.world, sj) as f64;
        let fe = revolute_joint_get_angle(&mut sim.world, ej) as f64;
        println!(
            "  sim mu={:.2}: masses upper {:.3} g fore {:.3} g, lifted 25 deg + elbow 30 deg, released, 12 s [{:.0} ms wall] | settle {:.2} s | max cone {:.1} deg (limit {:.1}) | elbow range {:.1}..{:.1} deg | final cone {:.1} deg elbow {:.1} deg | self-contacts {}",
            mu, mu_m, mf_m, wall, settle, max_cone.to_degrees(), cone.to_degrees(), el_range.0.to_degrees(), el_range.1.to_degrees(),
            fc_now.to_degrees(), fe.to_degrees(), contacts
        );
    }
    // pose holding: friction torque available vs gravity torque of the forearm held horizontal
    let mf = mass_props(&fore, 0.2, PLA);
    let lever = (mf.1.y - 6.0).abs();
    let m = mf.0 * PLA;
    println!(
        "  pose hold (elbow, forearm horizontal): gravity torque {:.0} vs pin friction {:.0} (mu 0.35, r_pin {:.2}) g mm^2/s^2 -> holds: {}",
        m * 9810.0 * lever,
        0.35 * elbow.r_pin * m * 9810.0,
        elbow.r_pin,
        0.35 * elbow.r_pin >= lever
    );
    println!("worst chain joint gap {:.3} mm", worst_gap);
}
