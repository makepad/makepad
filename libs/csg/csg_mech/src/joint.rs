//! Print-in-place joints: one declaration writes both mating halves and the
//! clearance gap. Clearances come from the printer profile; the cuts are the
//! partner swept over the joint's motion (a hinge's fork is a stack of
//! cylinders about the pin, a ball's shell is cut from the limb by a sphere of
//! the shell's largest radius, the ball's opening is the neck swept over the
//! cone limit), so the gap holds in every pose, not only the printed one.
//!
//! Model frame, millimetres, radians. `up` is the print direction in the model
//! frame (the model's +Y unless a placement turns the part).

use crate::geom::*;

/// A filament's mechanical data.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    /// g / mm^3.
    pub density: f64,
    /// Multiplies the in-plane clearance (stringy materials need more).
    pub gap_factor: f64,
    /// Dry friction coefficient of the material on itself.
    pub mu: f64,
}

impl Material {
    pub fn named(name: &str) -> Material {
        let (density, gap_factor, mu) = match name.to_ascii_lowercase().as_str() {
            "petg" => (1.27e-3, 1.15, 0.35),
            "abs" | "asa" => (1.05e-3, 1.05, 0.35),
            "tpu" => (1.21e-3, 1.3, 0.6),
            _ => (1.24e-3, 1.0, 0.35),
        };
        Material { name: name.to_string(), density, gap_factor, mu }
    }
}

/// What the joints are sized for.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub nozzle: f64,
    pub layer: f64,
    pub material: Material,
    /// The two halves print from different nozzles (their offset adds to every in-plane gap).
    pub cross_nozzle: bool,
    /// Print up in the model frame.
    pub up: P3,
}

impl Default for Profile {
    fn default() -> Profile {
        Profile { nozzle: 0.4, layer: 0.2, material: Material::named("PLA"), cross_nozzle: false, up: [0.0, 1.0, 0.0] }
    }
}

impl Profile {
    /// In-plane clearance: the squish, motion, ooze and offset budget of a 2-wall gap.
    pub fn c_xy(&self) -> f64 {
        (0.875 * self.nozzle).max(0.2) * self.material.gap_factor + if self.cross_nozzle { 0.1 } else { 0.0 }
    }
    /// Clearance across layers: two whole empty layers (one layer tacks).
    pub fn c_z(&self) -> f64 {
        2.0 * self.layer
    }
}

/// Where a part reaches along a direction (from its bounds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub lo: f64,
    pub hi: f64,
}

impl Extent {
    /// The projection of an axis-aligned box onto `dir` (a unit vector), measured from `origin`.
    pub fn of_box(min: P3, max: P3, origin: P3, dir: P3) -> Extent {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..8 {
            let c = [if i & 1 == 0 { min[0] } else { max[0] }, if i & 2 == 0 { min[1] } else { max[1] }, if i & 4 == 0 { min[2] } else { max[2] }];
            let d = dot(sub(c, origin), dir);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        Extent { lo, hi }
    }
    pub fn size(&self) -> f64 {
        self.hi - self.lo
    }
}

/// A part's bounds as the joint sizing reads them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartBox {
    pub min: P3,
    pub max: P3,
}

impl PartBox {
    pub fn center(&self) -> P3 {
        scale(add(self.min, self.max), 0.5)
    }
    pub fn extent(&self, origin: P3, dir: P3) -> Extent {
        Extent::of_box(self.min, self.max, origin, dir)
    }
    /// The smaller width across `axis`.
    pub fn width_across(&self, axis: P3) -> f64 {
        let (u, v) = basis(axis);
        let c = self.center();
        self.extent(c, u).size().min(self.extent(c, v).size())
    }
}

/// A declared hinge: A gets the fork (plates and pin), B the eye.
#[derive(Clone, Debug, PartialEq)]
pub struct HingeDecl {
    pub a: usize,
    pub b: usize,
    pub at: P3,
    /// The pin axis (unit). Along `up` is best.
    pub axis: P3,
    /// Declared rotation limits (radians, B relative to A about `axis`); the field sweep may narrow them.
    pub limits: Option<(f64, f64)>,
    pub pin: Option<f64>,
    pub plate: Option<f64>,
    pub clearance: Option<f64>,
    pub clearance_axial: Option<f64>,
}

/// A declared ball joint: A gets the socket, B the ball and its neck.
#[derive(Clone, Debug, PartialEq)]
pub struct BallDecl {
    pub a: usize,
    pub b: usize,
    pub at: P3,
    pub radius: Option<f64>,
    pub neck: Option<f64>,
    /// The socket's opening direction, from the centre toward B (unit); none: toward B's centre.
    pub axis: Option<P3>,
    /// Cone limit (radians) about `axis`.
    pub cone: f64,
    /// Twist limits about the neck (radians); none: free.
    pub twist: Option<(f64, f64)>,
    pub clearance: Option<f64>,
    pub wall: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JointDecl {
    Hinge(HingeDecl),
    Ball(BallDecl),
}

impl JointDecl {
    pub fn parts(&self) -> (usize, usize) {
        match self {
            JointDecl::Hinge(h) => (h.a, h.b),
            JointDecl::Ball(b) => (b.a, b.b),
        }
    }
}

/// A sized joint.
#[derive(Clone, Debug, PartialEq)]
pub enum JointGeom {
    Hinge {
        at: P3,
        axis: P3,
        /// The band along the axis (from `at`) both parts share.
        s0: f64,
        s1: f64,
        r_pin: f64,
        r_plate: f64,
        t_plate: f64,
        c_r: f64,
        c_z: f64,
        limits: Option<(f64, f64)>,
    },
    Ball {
        c: P3,
        r: f64,
        clr: f64,
        wall: f64,
        axis: P3,
        rn: f64,
        cone: f64,
        twist: Option<(f64, f64)>,
        up: P3,
        /// A's extent along `up` from the centre (the shell is clipped to it).
        up_lo: f64,
        up_hi: f64,
    },
}

/// A note on how a joint was sized (shown to the user and the AI).
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub warn: bool,
    pub text: String,
}

const S2: f64 = std::f64::consts::SQRT_2;

/// Cuts reach this far into what an add fills back, so a cut's face never lands on an add's face.
pub const EPS: f64 = 0.01;


/// Size a hinge for the profile from the two parts' bounds.
pub fn size_hinge(d: &HingeDecl, a: &PartBox, b: &PartBox, p: &Profile, notes: &mut Vec<Note>) -> Result<JointGeom, String> {
    let axis = norm(d.axis);
    let ea = a.extent(d.at, axis);
    let eb = b.extent(d.at, axis);
    let s0 = ea.lo.max(eb.lo);
    let s1 = ea.hi.min(eb.hi);
    let t = s1 - s0;
    let along_up = dot(axis, norm(p.up)).abs() > 10f64.to_radians().cos();
    // Across the layers the axial gap is whole layers; a pin lying in the bed plane gets the vertical rule on its
    // radius (its top is a hole roof) and the in-plane rule along it.
    let (c_r_auto, c_z_auto) = if along_up { (p.c_xy(), p.c_z()) } else { (p.c_xy().max(p.c_z()) + 0.5 * p.layer, p.c_xy()) };
    if !along_up {
        notes.push(Note { warn: true, text: "pin not along print Z: looser fit, the hole roof is printed over the pin".into() });
    }
    let c_r = d.clearance.unwrap_or(c_r_auto);
    let c_z = d.clearance_axial.unwrap_or(c_z_auto);
    let quant = |x: f64| if along_up { (x / p.layer).round().max(1.0) * p.layer } else { x };
    let t_plate = d.plate.unwrap_or_else(|| quant((0.2 * t).max(4.0 * p.layer)));
    let w = a.width_across(axis).min(b.width_across(axis));
    let r_pin = d.pin.unwrap_or_else(|| (0.24 * t.min(w)).clamp(3.0 * p.nozzle, 3.0));
    let r_plate = r_pin + (5.0 * p.nozzle).max(1.6) + c_r;
    let eye = t - 2.0 * t_plate - 2.0 * c_z;
    if t <= 0.0 {
        return Err(format!("the parts do not overlap along the pin axis at [{:.2} {:.2} {:.2}]", d.at[0], d.at[1], d.at[2]));
    }
    if eye < 3.0 * p.layer.max(0.2) {
        return Err(format!("too thin for a hinge: {t:.2} mm along the pin leaves an eye of {eye:.2} mm (plates {t_plate:.2}, gaps {c_z:.2}); make the parts thicker there or give plate:"));
    }
    if r_pin < 2.0 * p.nozzle {
        notes.push(Note { warn: true, text: format!("pin radius {r_pin:.2} mm is under 2 nozzle widths: weak") });
    }
    Ok(JointGeom::Hinge { at: d.at, axis, s0, s1, r_pin, r_plate, t_plate, c_r, c_z, limits: d.limits })
}

/// Size a ball joint for the profile.
pub fn size_ball(d: &BallDecl, a: &PartBox, b: &PartBox, p: &Profile, notes: &mut Vec<Note>) -> Result<JointGeom, String> {
    let up = norm(p.up);
    let axis = norm(d.axis.unwrap_or_else(|| sub(b.center(), d.at)));
    let ea = a.extent(d.at, up);
    let t_a = ea.size();
    let w_b = b.width_across(axis);
    let r = d.radius.unwrap_or_else(|| (0.4 * t_a.min(1.6 * w_b)).clamp(2.5, 12.0));
    let clr = d.clearance.unwrap_or((0.86 * p.c_xy()).max(1.5 * p.layer));
    let wall = d.wall.unwrap_or((0.35 * r).max(5.0 * p.nozzle).max(1.6));
    let rn = d.neck.unwrap_or(0.38 * r).min(r * 0.7);
    let rc = r + clr;
    // Captive: the opening's chord radius at the inner shell must stay at least 0.3 mm inside the ball.
    let neck_half = ((rn + clr) / rc).asin();
    let open_max = ((r - 0.3) / rc).asin();
    let cone_max = open_max - neck_half;
    if cone_max <= 0.0 {
        return Err(format!("ball radius {r:.2} with neck {rn:.2} cannot be captive; give a larger radius: or a thinner neck:"));
    }
    let mut cone = d.cone;
    if cone > cone_max {
        notes.push(Note { warn: true, text: format!("cone {:.0} deg is too wide to keep the ball captive; using {:.0} deg", cone.to_degrees(), cone_max.to_degrees()) });
        cone = cone_max;
    }
    let up_lo = ea.lo;
    let up_hi = ea.hi;
    let floor = -up_lo - rc;
    if floor < 2.0 * p.layer {
        return Err(format!("the socket part is too thin under the ball: {floor:.2} mm of floor; move the ball up or make the part thicker"));
    }
    if floor < 4.0 * p.layer {
        notes.push(Note { warn: true, text: format!("socket floor is only {floor:.2} mm under the ball") });
    }
    // The top: the teardrop roof may open through the part's top face; that hole must stay smaller than the ball.
    let top_hole = if up_hi >= rc * S2 { 0.0 } else if up_hi >= rc / S2 { rc * S2 - up_hi } else { (rc * rc - up_hi.max(0.0).powi(2)).max(0.0).sqrt() };
    if top_hole > r - 0.3 {
        return Err(format!("the socket part is too thin over the ball: its top opens {:.2} mm wide; move the ball down or make the part thicker", 2.0 * top_hole));
    }
    Ok(JointGeom::Ball { c: d.at, r, clr, wall, axis, rn, cone, twist: d.twist, up, up_lo, up_hi })
}

impl JointGeom {
    fn p(at: P3, axis: P3, s: f64) -> P3 {
        madd(at, axis, s)
    }

    /// The smallest gap the joint is built with.
    pub fn clearance(&self) -> f64 {
        match self {
            JointGeom::Hinge { c_r, c_z, .. } => c_r.min(*c_z),
            JointGeom::Ball { clr, .. } => *clr,
        }
    }

    /// The bearing radius (friction lever).
    pub fn bearing_radius(&self) -> f64 {
        match self {
            JointGeom::Hinge { r_pin, .. } => *r_pin,
            JointGeom::Ball { r, .. } => *r,
        }
    }

    /// The sim pivot and axis (revolute: the pin; spherical: the cone axis).
    pub fn pivot(&self) -> (P3, P3) {
        match self {
            JointGeom::Hinge { at, axis, s0, s1, .. } => (Self::p(*at, *axis, 0.5 * (s0 + s1)), *axis),
            JointGeom::Ball { c, axis, .. } => (*c, *axis),
        }
    }

    fn ball_reach(r: f64, clr: f64, wall: f64, up_hi: f64) -> f64 {
        let rs = r + clr + wall;
        let tip = rs * S2;
        let dz = up_hi;
        let shell = if dz >= tip { tip } else if dz > rs / S2 { (dz * dz + (tip - dz) * (tip - dz)).sqrt() } else { rs };
        shell.max(rs) + clr
    }

    /// Where joint-internal contact can happen: a centre and a radius. The checks look here; the sim's collision
    /// proxies leave it out (the joint constraint owns it).
    pub fn keep_out(&self) -> (P3, f64) {
        match self {
            JointGeom::Hinge { at, axis, s0, s1, r_plate, c_r, .. } => {
                let mid = Self::p(*at, *axis, 0.5 * (s0 + s1));
                let half = 0.5 * (s1 - s0) + 0.5;
                let rad = r_plate + c_r + 1.0;
                (mid, (half * half + rad * rad).sqrt())
            }
            JointGeom::Ball { c, r, clr, wall, up_hi, .. } => (*c, Self::ball_reach(*r, *clr, *wall, *up_hi) + 1.0),
        }
    }

    /// The box the joint's checks search.
    pub fn region(&self) -> (P3, P3) {
        match self {
            JointGeom::Hinge { at, axis, s0, s1, r_plate, c_r, .. } => {
                let a = Self::p(*at, *axis, s0 - 0.5);
                let b = Self::p(*at, *axis, s1 + 0.5);
                let r = r_plate + c_r + 1.5;
                (std::array::from_fn(|i| a[i].min(b[i]) - r), std::array::from_fn(|i| a[i].max(b[i]) + r))
            }
            JointGeom::Ball { .. } => {
                let (c, r) = self.keep_out();
                (sub(c, [r; 3]), add(c, [r; 3]))
            }
        }
    }

    /// A's solid with this joint's half.
    pub fn apply_a(&self, a: Solid) -> Solid {
        match *self {
            JointGeom::Hinge { at, axis, s0, s1, r_pin, r_plate, t_plate, c_r, .. } => {
                let p = |s: f64| Self::p(at, axis, s);
                Solid::Union(vec![
                    Solid::diff(a, vec![Solid::Prim(Prim::cyl(p(s0 + t_plate - EPS), p(s1 - t_plate + EPS), r_plate + c_r))]),
                    // flush with the parts' outer faces (a hair inset leaves the mesh boolean slivers it cannot close)
                    Solid::Prim(Prim::cyl(p(s0), p(s0 + t_plate), r_plate)),
                    Solid::Prim(Prim::cyl(p(s1 - t_plate), p(s1), r_plate)),
                    Solid::Prim(Prim::cyl(p(s0), p(s1), r_pin)),
                ])
            }
            JointGeom::Ball { c, r, clr, wall, axis, rn, cone, up, up_lo, up_hi, .. } => {
                // A sphere with a 45 degree roof. The roof cone starts a little inside the sphere (not on the
                // tangent circle, which leaves the mesh boolean a degenerate seam), so it leans 44.5 degrees.
                let teardrop = |rad: f64| {
                    let h1 = rad / S2 - 0.1 * rad;
                    let r1 = (rad * rad - h1 * h1).sqrt() - 0.004 * rad;
                    Solid::Union(vec![
                        Solid::Prim(Prim::Sphere { c, r: rad }),
                        Solid::Prim(Prim::Frustum { a: madd(c, up, h1), b: madd(c, up, rad * S2), ra: r1, rb: 0.0 }),
                    ])
                };
                let rc = r + clr;
                let d = rn + clr;
                let back = d / cone.sin().max(1e-3);
                let length = Self::ball_reach(r, clr, wall, up_hi) + 2.0;
                let opening = Prim::Frustum { a: madd(c, axis, -back), b: madd(c, axis, length), ra: 0.0, rb: (length + back) * cone.tan() };
                let c_up = dot(c, up);
                // the shell is clipped to A's own extent along up (it never pokes under the bed or over A's top)
                let shell = Solid::Inter(vec![
                    teardrop(rc + wall),
                    Solid::Prim(Prim::Slab { n: up, lo: c_up + up_lo + 5.0 * EPS, hi: c_up + up_hi - 5.0 * EPS, c, half: Self::ball_reach(r, clr, wall, up_hi) + 2.0 }),
                ]);
                Solid::diff(Solid::Union(vec![a, shell]), vec![teardrop(rc), Solid::Prim(opening)])
            }
        }
    }

    /// B's solid with this joint's half.
    pub fn apply_b(&self, b: Solid) -> Solid {
        match *self {
            JointGeom::Hinge { at, axis, s0, s1, r_pin, r_plate, t_plate, c_r, c_z, .. } => {
                let p = |s: f64| Self::p(at, axis, s);
                let (e0, e1) = (s0 + t_plate + c_z, s1 - t_plate - c_z);
                let rr = r_plate + c_r;
                let body = Solid::diff(b, vec![Solid::Prim(Prim::cyl(p(s0 - 1.0), p(e0 + EPS), rr)), Solid::Prim(Prim::cyl(p(e1 - EPS), p(s1 + 1.0), rr))]);
                Solid::diff(
                    Solid::Union(vec![body, Solid::Prim(Prim::cyl(p(e0), p(e1), r_plate))]),
                    vec![Solid::Prim(Prim::cyl(p(s0 - 1.0), p(s1 + 1.0), r_pin + c_r))],
                )
            }
            JointGeom::Ball { c, r, clr, wall, axis, rn, up_hi, .. } => {
                let reach = Self::ball_reach(r, clr, wall, up_hi);
                Solid::Union(vec![
                    Solid::diff(b, vec![Solid::Prim(Prim::Sphere { c, r: reach })]),
                    Solid::Prim(Prim::Sphere { c, r }),
                    Solid::Prim(Prim::cyl(c, madd(c, axis, reach + 0.5), rn)),
                ])
            }
        }
    }

    /// Ball joints: how far the ball reaches under the opening's edge (>= 0.3 mm keeps it captive).
    pub fn retention(&self) -> Option<f64> {
        match *self {
            JointGeom::Ball { r, clr, rn, cone, .. } => {
                let rc = r + clr;
                let open = (cone + ((rn + clr) / rc).asin()).min(std::f64::consts::FRAC_PI_2);
                Some(r - rc * open.sin())
            }
            _ => None,
        }
    }
}

/// An anchor: a part held by the world in the sim (a hanging hole through it, or held fixed).
#[derive(Clone, Debug, PartialEq)]
pub struct AnchorDecl {
    pub part: usize,
    pub at: P3,
    /// Pin axis of a hanging hole; none: the part is held fixed.
    pub hole: Option<(P3, f64)>,
}

impl AnchorDecl {
    /// The part with its hole cut (none when held fixed).
    pub fn apply(&self, s: Solid, extent: Extent) -> Solid {
        match self.hole {
            Some((axis, r)) => {
                let axis = norm(axis);
                Solid::diff(s, vec![Solid::Prim(Prim::cyl(madd(self.at, axis, extent.lo - 1.0), madd(self.at, axis, extent.hi + 1.0), r))])
            }
            None => s,
        }
    }
}

/// Every part's solid with all joints applied in declaration order.
pub fn assemble(parts: usize, joints: &[JointGeom], decls: &[JointDecl], anchors: &[(AnchorDecl, Extent)]) -> Vec<Solid> {
    let mut out: Vec<Solid> = (0..parts).map(Solid::Part).collect();
    for (g, d) in joints.iter().zip(decls) {
        let (a, b) = d.parts();
        let sa = std::mem::replace(&mut out[a], Solid::Part(a));
        out[a] = g.apply_a(sa);
        let sb = std::mem::replace(&mut out[b], Solid::Part(b));
        out[b] = g.apply_b(sb);
    }
    for (anchor, extent) in anchors {
        let s = std::mem::replace(&mut out[anchor.part], Solid::Part(anchor.part));
        out[anchor.part] = anchor.apply(s, *extent);
    }
    out
}
