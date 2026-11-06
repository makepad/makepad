//! Print-in-place joints: one declaration writes both mating halves and the
//! clearance gap. Clearances come from the printer profile; the cuts are the
//! partner swept over the joint's motion (a hinge's fork is a stack of
//! cylinders about the pin, a ball's shell is cut from the limb by a sphere of
//! the shell's largest radius, the ball's opening is the neck swept over the
//! cone limit), so the gap holds in every pose, not only the printed one.
//!
//! A ball prints standing on something ([`Seat`]): cut flat on the bed in a socket
//! open underneath, or on its neck coming up from below. A ball over a socket
//! floor would float a clearance above it, so a joint that has neither is refused.
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
    /// Measured on this printer and filament (a calibration coupon): they replace the rules below.
    pub measured: Measured,
}

/// Clearances and friction measured with calibration coupons (none: the first-principles rules).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Measured {
    /// In-plane clearance that prints free, mm.
    pub c_xy: Option<f64>,
    /// Empty layers between parts across layers that print free.
    pub c_z_layers: Option<u32>,
    /// Joint friction coefficient.
    pub mu: Option<f64>,
}

impl Default for Profile {
    fn default() -> Profile {
        Profile { nozzle: 0.4, layer: 0.2, material: Material::named("PLA"), cross_nozzle: false, up: [0.0, 1.0, 0.0], measured: Measured::default() }
    }
}

impl Profile {
    /// In-plane clearance: the squish, motion, ooze and offset budget of a 2-wall gap, or as measured.
    pub fn c_xy(&self) -> f64 {
        if let Some(c) = self.measured.c_xy {
            return c + if self.cross_nozzle { 0.1 } else { 0.0 };
        }
        (0.875 * self.nozzle).max(0.2) * self.material.gap_factor + if self.cross_nozzle { 0.1 } else { 0.0 }
    }
    /// Clearance across layers: two whole empty layers (one layer tacks), or as measured.
    pub fn c_z(&self) -> f64 {
        self.measured.c_z_layers.unwrap_or(2) as f64 * self.layer
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
    /// Shoes that hold the pose ([`BrakeDecl`]); none: it turns free.
    pub brake: Option<BrakeDecl>,
}

/// A brake on a hinge: a part of a flexible filament (`shoe`) the hinge shapes into two shoes set
/// in B's eye either side of the pin, each with a tooth in a groove of the pin as it prints, a
/// clearance clear of it. Turned off its printed angle, the pin's round comes under the teeth and
/// presses them in: the shoes hold the pose by friction. Near its printed angle it turns free.
#[derive(Clone, Debug, PartialEq)]
pub struct BrakeDecl {
    /// The shoes' part. What of it lies outside the shoes is cut away; they print fused to B's eye.
    pub shoe: usize,
    /// The torque it holds once turned (N mm); none: the teeth run the eye's length.
    pub hold: Option<f64>,
}

/// How far a brake's teeth press into the pin once it has turned off its grooves (mm).
pub const BRAKE_SQUEEZE: f64 = 0.3;
/// A shoe's flexible filament beside its tooth, and behind it (mm).
const SHOE_SIDE: f64 = 1.2;
const SHOE_BACK: f64 = 1.6;
/// The eye's wall round its shoes (mm).
const SHOE_EYE_WALL: f64 = 1.2;

/// A sized brake ([`BrakeDecl`]), in the hinge's frame: the pin's axis through `at` along `axis`.
#[derive(Clone, Debug, PartialEq)]
pub struct Brake {
    pub shoe: usize,
    /// Unit, across the pin: the first tooth's way from the axis as it prints (the second's is
    /// the opposite way).
    pub dir: P3,
    pub r_pin: f64,
    /// The pin's grooves' depth, and how far from the axis the teeth's tips are.
    pub groove: f64,
    pub tip: f64,
    /// The bore's radius, the shoes' outer radius, and their half angle about `dir` (radians).
    pub bore: f64,
    pub outer: f64,
    pub half: f64,
    /// Along the axis from `at`: the eye's span, which the shoes fill, and the teeth's.
    pub eye: (f64, f64),
    pub teeth: (f64, f64),
    /// The shoes' filament: its Young's modulus (MPa) and its friction on the pin.
    pub modulus: f64,
    pub mu: f64,
}

impl Brake {
    /// How far each tooth is pressed in with the joint turned `ang` from its printed angle (mm):
    /// across the pin, the deepest point of a tooth's flanks inside the turned pin (round, less
    /// its two grooves).
    pub fn squeeze(&self, ang: f64) -> f64 {
        let r2 = std::f64::consts::FRAC_1_SQRT_2;
        // the plane across the pin: x along the first tooth, y across it
        let wedge_out = |q: [f64; 2], theta: f64, apex: f64| {
            let (s, c) = theta.sin_cos();
            let a = [apex * c, apex * s];
            let n1 = [r2 * (c - s), r2 * (s + c)];
            let n2 = [r2 * (c + s), r2 * (s - c)];
            let d = [q[0] - a[0], q[1] - a[1]];
            let (h1, h2) = (d[0] * n1[0] + d[1] * n1[1], d[0] * n2[0] + d[1] * n2[1]);
            (h1.min(0.0).powi(2) + h2.min(0.0).powi(2)).sqrt()
        };
        let groove_apex = self.r_pin - self.groove;
        let inside = |q: [f64; 2]| {
            let round = self.r_pin - (q[0] * q[0] + q[1] * q[1]).sqrt();
            round.min(wedge_out(q, ang, groove_apex)).min(wedge_out(q, ang + std::f64::consts::PI, groove_apex))
        };
        let reach = (self.bore - self.tip) * std::f64::consts::SQRT_2;
        let mut deepest: f64 = 0.0;
        for k in 0..=96 {
            let s = reach * k as f64 / 96.0;
            for side in [1.0, -1.0] {
                deepest = deepest.max(inside([self.tip + s * r2, side * s * r2]));
            }
        }
        deepest
    }

    /// The torque the two shoes hold there (N mm): each tooth pressed in `s` along its length,
    /// over its width there (`2 s`: its flanks are square), the shoe behind its tip giving at the
    /// filament's modulus; each rubs on the pin's round.
    pub fn torque(&self, ang: f64) -> f64 {
        let s = self.squeeze(ang);
        let press = self.modulus * (self.teeth.1 - self.teeth.0) * 2.0 * s * s / (self.outer - self.tip);
        2.0 * self.mu * press * self.r_pin
    }

    /// The torque it holds turned well off its grooves (N mm).
    pub fn hold(&self) -> f64 {
        self.torque(0.5 * std::f64::consts::PI)
    }

    /// Up to which angle off its printed one it turns free, and from which it holds fully (radians).
    pub fn band(&self) -> (f64, f64) {
        let full = self.squeeze(0.5 * std::f64::consts::PI);
        let mut free = 0.0;
        let mut whole = 0.5 * std::f64::consts::PI;
        for k in 0..=900 {
            let a = k as f64 * 0.1f64.to_radians();
            let s = self.squeeze(a);
            if s <= 1e-6 {
                free = a;
            }
            if s >= full - 1e-6 {
                whole = a;
                break;
            }
        }
        (free, whole)
    }

    fn across(&self, axis: P3, way: f64) -> (P3, P3) {
        let u = scale(self.dir, way);
        (u, cross(axis, u))
    }

    /// The pin's two grooves, along the eye, across the gaps past it (`gap` long) and on into
    /// the plates, which fill them back: the teeth's ends are the gaps' width from the round.
    fn grooves(&self, at: P3, axis: P3, gap: f64) -> Vec<Solid> {
        let r2 = std::f64::consts::FRAC_1_SQRT_2;
        let (s0, s1) = (self.eye.0 - gap - 0.5, self.eye.1 + gap + 0.5);
        let mid = madd(at, axis, 0.5 * (s0 + s1));
        let half = 0.5 * (s1 - s0) + self.r_pin + 2.0;
        [1.0, -1.0]
            .iter()
            .map(|&way| {
                let (u, t) = self.across(axis, way);
                let apex = madd(mid, u, self.r_pin - self.groove);
                let side = |n: P3| Solid::Prim(Prim::Slab { n, lo: dot(n, apex), hi: dot(n, apex) + 2.0 * self.r_pin + 2.0, c: apex, half });
                Solid::Inter(vec![
                    side(norm(add(scale(u, r2), scale(t, r2)))),
                    side(norm(sub(scale(u, r2), scale(t, r2)))),
                    Solid::Prim(Prim::Slab { n: axis, lo: dot(axis, at) + s0, hi: dot(axis, at) + s1, c: mid, half }),
                ])
            })
            .collect()
    }

    /// The two pockets in B's eye the shoes fill, as far as `out` from the axis.
    fn pockets(&self, at: P3, axis: P3, out: f64, ends: (f64, f64)) -> Vec<Solid> {
        let mid = madd(at, axis, 0.5 * (ends.0 + ends.1));
        let half = 0.5 * (ends.1 - ends.0) + out + 2.0;
        let (sh, ch) = self.half.sin_cos();
        [1.0, -1.0]
            .iter()
            .map(|&way| {
                let (u, t) = self.across(axis, way);
                let side = |n: P3| Solid::Prim(Prim::Slab { n, lo: dot(n, mid), hi: dot(n, mid) + 2.0 * out + 2.0, c: mid, half });
                Solid::Inter(vec![
                    Solid::Prim(Prim::cyl(madd(at, axis, ends.0), madd(at, axis, ends.1), out)),
                    side(add(scale(u, sh), scale(t, ch))),
                    side(sub(scale(u, sh), scale(t, ch))),
                ])
            })
            .collect()
    }

    /// The shoes: each pocket clear of the bore, and its tooth reaching in from the bore.
    pub fn shoes(&self, at: P3, axis: P3) -> Solid {
        let r2 = std::f64::consts::FRAC_1_SQRT_2;
        let p = |s: f64| madd(at, axis, s);
        let bodies = Solid::diff(Solid::Union(self.pockets(at, axis, self.outer, self.eye)), vec![Solid::Prim(Prim::cyl(p(self.eye.0 - 1.0), p(self.eye.1 + 1.0), self.bore))]);
        let mid = p(0.5 * (self.teeth.0 + self.teeth.1));
        let half = 0.5 * (self.teeth.1 - self.teeth.0) + self.bore + 2.0;
        let teeth = [1.0, -1.0].iter().map(|&way| {
            let (u, t) = self.across(axis, way);
            let apex = madd(mid, u, self.tip);
            let side = |n: P3| Solid::Prim(Prim::Slab { n, lo: dot(n, apex), hi: dot(n, apex) + 2.0 * self.bore + 2.0, c: apex, half });
            Solid::Inter(vec![
                side(norm(add(scale(u, r2), scale(t, r2)))),
                side(norm(sub(scale(u, r2), scale(t, r2)))),
                Solid::Prim(Prim::cyl(p(self.teeth.0), p(self.teeth.1), self.bore + EPS)),
            ])
        });
        Solid::Union(std::iter::once(bodies).chain(teeth).collect())
    }
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

/// A declared slider: A is the rail, B the carriage that slides along it. Both stand on the bed;
/// a chevron way (a ridge on the carriage in a groove of the rail) keeps the carriage in, up and
/// down, and both print as they stand.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderDecl {
    pub a: usize,
    pub b: usize,
    /// A point on the way's centre line (its height does not matter).
    pub at: P3,
    /// Along the bed (unit).
    pub axis: P3,
    /// How far the carriage slides back and on from where it prints (mm, back <= 0 <= on);
    /// either not given: to the rail's end stop that way.
    pub travel: (Option<f64>, Option<f64>),
    /// The carriage's neck through the rail's slot, across (mm).
    pub width: Option<f64>,
    /// How far its ridge reaches past the neck each side (mm).
    pub depth: Option<f64>,
    pub clearance: Option<f64>,
}

/// A declared bearing: A is the outer race, B the inner that turns in it about the print's up.
/// Both stand on the bed; a chevron race (the inner's ridge in a groove of the outer) keeps the
/// inner in, and both print as they stand.
#[derive(Clone, Debug, PartialEq)]
pub struct BearingDecl {
    pub a: usize,
    pub b: usize,
    /// A point on the turning axis (its height does not matter).
    pub at: P3,
    /// The inner's radius at the bed and at the race's top (mm); its ridge reaches `depth` past it.
    pub radius: Option<f64>,
    pub depth: Option<f64>,
    pub clearance: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JointDecl {
    Hinge(HingeDecl),
    Ball(BallDecl),
    Slider(SliderDecl),
    Bearing(BearingDecl),
}

impl JointDecl {
    pub fn parts(&self) -> (usize, usize) {
        match self {
            JointDecl::Hinge(h) => (h.a, h.b),
            JointDecl::Ball(b) => (b.a, b.b),
            JointDecl::Slider(s) => (s.a, s.b),
            JointDecl::Bearing(r) => (r.a, r.b),
        }
    }

    /// Its brake's shoe part, when it has one.
    pub fn shoe(&self) -> Option<usize> {
        match self {
            JointDecl::Hinge(HingeDecl { brake: Some(b), .. }) => Some(b.shoe),
            _ => None,
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
        brake: Option<Brake>,
        /// Print up, and A's and B's floors along it: a cut about a pin across up that crosses
        /// its part's floor runs on straight down through it ([`JointGeom::slot`]).
        up: P3,
        floors: [f64; 2],
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
        seat: Seat,
    },
    /// A chevron way along `axis` ([`crate::way`]): the rail's groove, the carriage's ridge.
    Slider(crate::way::Way),
    /// A chevron race about `up` ([`crate::way`]): the outer's groove, the inner's ridge.
    Bearing(crate::way::Race),
}

/// What a ball stands on while it prints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seat {
    /// Cut flat on the bed (`ground`: the bed along `up`), in a socket open underneath that
    /// comes down to the bed round it and holds it by more than its opening there.
    Bed { ground: f64 },
    /// On its neck, which comes up to it from below.
    Neck,
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
    let mut r_plate = r_pin + (5.0 * p.nozzle).max(1.6) + c_r;
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
    let brake = match &d.brake {
        Some(bd) => {
            // the teeth's flanks are square to each other, at 45 degrees to the bed with the pin
            // across it: apart by the clearance square to them, they are apart by 1.41 times it
            // along the bed and up, as the rules ask
            let c_t = if along_up { c_r } else { c_r.min(p.c_xy().max(p.c_z()) * std::f64::consts::FRAC_1_SQRT_2) };
            let b = size_brake(bd, sub(b.center(), d.at), axis, (s0 + t_plate + c_z, s1 - t_plate - c_z), (r_pin, c_r, c_t), p, notes)?;
            r_plate = r_plate.max(b.outer + SHOE_EYE_WALL);
            Some(b)
        }
        None => None,
    };
    let up = norm(p.up);
    let floors = [a.extent([0.0; 3], up).lo, b.extent([0.0; 3], up).lo];
    Ok(JointGeom::Hinge { at: d.at, axis, s0, s1, r_pin, r_plate, t_plate, c_r, c_z, limits: d.limits, brake, up, floors })
}

/// Size a hinge's brake: grooves a squeeze plus a clearance (along the flanks) deep, the teeth's
/// tips a squeeze inside the pin's round, their shoes in pockets either side of the bore. Across
/// the layers (a pin along up) the teeth face B; a pin in the bed plane has them along the bed,
/// across it, so their flanks and the grooves' lean 45 degrees.
/// `to_b`: from the pin's point toward B's centre; `eye`: the eye's span along the axis; `pin`:
/// its radius and the clearance round it.
/// Size a hinge's brake: `eye` the eye's span along the axis, `pin` the pin's radius, the bore's
/// clearance round it and the teeth's from the grooves' flanks.
fn size_brake(bd: &BrakeDecl, to_b: P3, axis: P3, eye: (f64, f64), pin: (f64, f64, f64), p: &Profile, notes: &mut Vec<Note>) -> Result<Brake, String> {
    let (r_pin, c_r, c_t) = pin;
    let groove = BRAKE_SQUEEZE + c_t * std::f64::consts::SQRT_2;
    if r_pin < 2.5 * groove {
        return Err(format!("a brake needs a pin of at least {:.1} mm radius (its grooves are {groove:.2} mm deep); give pin:", 2.5 * groove));
    }
    let up = norm(p.up);
    let dir = if dot(axis, up).abs() > 10f64.to_radians().cos() {
        let across = sub(to_b, scale(axis, dot(to_b, axis)));
        if len(across) > 1e-6 { norm(across) } else { basis(axis).0 }
    } else {
        norm(cross(axis, up))
    };
    let tip = r_pin - BRAKE_SQUEEZE;
    let bore = r_pin + c_r;
    // no steeper than 45 degrees off the shoe's middle: its inner face prints on its own over the
    // pin with the pin across the bed
    let half = ((bore - tip + SHOE_SIDE) / bore).min(45f64.to_radians());
    let tpu = Material::named("TPU");
    let mut brake = Brake { shoe: bd.shoe, dir, r_pin, groove, tip, bore, outer: bore + SHOE_BACK, half, eye, teeth: eye, modulus: crate::flex::FLEX_MODULUS, mu: tpu.mu };
    if let Some(hold) = bd.hold {
        let most = brake.hold();
        let length = (eye.1 - eye.0) * (hold / most).min(1.0);
        let length = length.max(1.0f64.min(eye.1 - eye.0));
        let mid = 0.5 * (eye.0 + eye.1);
        brake.teeth = (mid - 0.5 * length, mid + 0.5 * length);
        if hold > most * 1.001 {
            notes.push(Note { warn: true, text: format!("the brake holds at most {most:.0} N mm here (its teeth run the eye's length); asked {hold:.0}") });
        }
    }
    let (free, whole) = brake.band();
    notes.push(Note { warn: false, text: format!("brake: two shoes hold {:.0} N mm from {:.0} degrees off its printed angle (free within {:.0})", brake.hold(), whole.to_degrees(), free.to_degrees()) });
    Ok(brake)
}

/// Size a ball joint for the profile. `ground` is the bed along the print direction: the lowest
/// point of the parts printed in place with it.
pub fn size_ball(d: &BallDecl, a: &PartBox, b: &PartBox, ground: f64, p: &Profile, notes: &mut Vec<Note>) -> Result<JointGeom, String> {
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
    let seat = if on_the_bed(dot(d.at, up) - ground, -dot(axis, up), r, clr, rn, p, notes)? { Seat::Bed { ground } } else { Seat::Neck };
    // The top: the teardrop roof may open through the part's top face; that hole must stay smaller than the ball.
    let top_hole = if up_hi >= rc * S2 { 0.0 } else if up_hi >= rc / S2 { rc * S2 - up_hi } else { (rc * rc - up_hi.max(0.0).powi(2)).max(0.0).sqrt() };
    if top_hole > r - 0.3 {
        return Err(format!("the socket part is too thin over the ball: its top opens {:.2} mm wide; move the ball down or make the part thicker", 2.0 * top_hole));
    }
    Ok(JointGeom::Ball { c: d.at, r, clr, wall, axis, rn, cone, twist: d.twist, up, up_lo, up_hi, seat })
}

/// The narrowest a ball's flat on the bed may be, as a radius in nozzle widths: four lines across.
const BED_FLAT: f64 = 2.0;

/// Whether a ball of radius `r` whose centre is `h` over the bed stands on the bed (true) or on
/// its neck (false; `down`: how far its neck's axis points down, the cosine). It stands on the bed
/// when it reaches it, else on its neck when that comes up from below. Neither: it would print over
/// its socket's floor with nothing under it, and the joint is refused with where it would print.
fn on_the_bed(h: f64, down: f64, r: f64, clr: f64, rn: f64, p: &Profile, notes: &mut Vec<Note>) -> Result<bool, String> {
    let rc = r + clr;
    // On the bed: the socket comes down to the bed, so its opening there is the cavity's section
    // at the bed, which must stay 0.3 mm inside the ball; the ball's flat must hold lines.
    let lowest = (rc * rc - (r - 0.3).powi(2)).max(0.0).sqrt();
    let highest = (r * r - (BED_FLAT * p.nozzle).powi(2)).max(0.0).sqrt();
    if h < r {
        if h < lowest {
            return Err(format!(
                "the ball sits too low to stay in: its socket's opening on the bed is {:.2} mm wide; raise its centre to {lowest:.2} mm above the bed or more (it is {h:.2})",
                2.0 * (rc * rc - h * h).max(0.0).sqrt()
            ));
        }
        if h > highest {
            return Err(format!(
                "the ball only grazes the bed: its flat there is {:.2} mm wide; lower its centre to {highest:.2} mm above the bed or less (it is {h:.2}), or to {:.2} for a ball whose underside needs no support",
                2.0 * (r * r - h * h).max(0.0).sqrt(),
                r / S2
            ));
        }
        if h > r / S2 + 0.5 * p.layer {
            notes.push(Note { warn: true, text: format!("the ball's underside overhangs past 45 degrees for {:.2} mm over the bed; a centre {:.2} mm above the bed needs no support", h - r / S2, r / S2) });
        }
        return Ok(true);
    }
    // On its neck: the ball's lowest point lies inside the neck, a line in from its side.
    if down > 0.0 && r * (1.0 - down * down).max(0.0).sqrt() <= rn - p.nozzle {
        return Ok(false);
    }
    let tilt = ((rn - p.nozzle).max(0.0) / r).min(1.0).asin().to_degrees();
    Err(format!(
        "the ball would print in mid-air: its lowest point is {:.2} mm above the bed, over nothing but its socket's floor. Put its centre {lowest:.2} to {highest:.2} mm above the bed ({:.2} needs no support) so it prints flat on the bed in a socket open underneath, or point its neck down (axis: within {tilt:.0} degrees of straight down) so the neck carries it",
        h - r,
        r / S2
    ))
}

impl JointGeom {
    fn p(at: P3, axis: P3, s: f64) -> P3 {
        madd(at, axis, s)
    }

    /// A hinge's cut about a pin across up (radius `r`, over `span` along the pin) that crosses
    /// its part's `floor` leaves the part an edge as thin as nothing on the bed each side of the
    /// pin, where the cut's round meets the floor. This is the box that runs the cut on straight
    /// down through the floor, a slot open underneath with walls square to the bed. None where
    /// the cut over `within` (the part's own share of the span) clears the floor, so those cuts
    /// are as they were.
    fn slot(at: P3, axis: P3, up: P3, span: (f64, f64), within: (f64, f64), r: f64, floor: f64) -> Option<Solid> {
        let along = dot(axis, up);
        let sin = (1.0 - along * along).max(0.0).sqrt();
        if sin < 10f64.to_radians().sin() {
            return None;
        }
        let height = |s: f64| dot(Self::p(at, axis, s), up);
        let (lo, hi) = (span.0.max(within.0), span.1.min(within.1));
        if height(lo).min(height(hi)) - r * sin >= floor {
            return None;
        }
        // square to the pin: `rise` the way up, `across` along the bed
        let rise = norm(sub(up, scale(axis, along)));
        let across = cross(axis, rise);
        // its ends a hair past the cut's and its walls a hair inside the cut's widest (no face
        // of the one lands on the other's); from the pin down till all of it is under the floor
        let (lo, hi) = (span.0 - EPS, span.1 + EPS);
        let w = r - EPS;
        let depth = (height(lo).max(height(hi)) - floor) / sin + 1.0;
        let c = madd(Self::p(at, axis, 0.5 * (lo + hi)), rise, -0.5 * depth);
        let half = (0.25 * (hi - lo) * (hi - lo) + w * w + 0.25 * depth * depth).sqrt();
        let slab = |n: P3, h: f64| Solid::Prim(Prim::Slab { n, lo: dot(c, n) - h, hi: dot(c, n) + h, c, half });
        Some(Solid::Inter(vec![slab(axis, 0.5 * (hi - lo)), slab(across, w), slab(rise, 0.5 * depth)]))
    }

    /// The smallest gap the joint is built with.
    pub fn clearance(&self) -> f64 {
        match self {
            JointGeom::Hinge { c_r, c_z, .. } => c_r.min(*c_z),
            JointGeom::Ball { clr, .. } => *clr,
            JointGeom::Slider(w) => w.chevron.clearance(),
            JointGeom::Bearing(r) => r.chevron.clearance(),
        }
    }

    /// The bearing radius (friction lever).
    pub fn bearing_radius(&self) -> f64 {
        match self {
            JointGeom::Hinge { r_pin, .. } => *r_pin,
            JointGeom::Ball { r, .. } => *r,
            JointGeom::Slider(w) => w.chevron.neck + w.chevron.depth,
            JointGeom::Bearing(r) => r.chevron.neck + 0.5 * r.chevron.depth,
        }
    }

    /// The sim pivot and axis (revolute: the pin; spherical: the cone axis; a slider: its way).
    pub fn pivot(&self) -> (P3, P3) {
        match self {
            JointGeom::Hinge { at, axis, s0, s1, .. } => (Self::p(*at, *axis, 0.5 * (s0 + s1)), *axis),
            JointGeom::Ball { c, axis, .. } => (*c, *axis),
            JointGeom::Slider(w) => w.pivot(),
            JointGeom::Bearing(r) => r.pivot(),
        }
    }

    fn ball_reach(r: f64, clr: f64, wall: f64, up_hi: f64) -> f64 {
        let rs = r + clr + wall;
        let tip = rs * S2;
        let dz = up_hi;
        let shell = if dz >= tip { tip } else if dz > rs / S2 { (dz * dz + (tip - dz) * (tip - dz)).sqrt() } else { rs };
        shell.max(rs) + clr
    }

    /// Where joint-internal contact can happen: balls (a centre and a radius). The sim's collision
    /// proxies leave them out (the joint constraint owns it).
    pub fn keep_out(&self) -> Vec<(P3, f64)> {
        match self {
            JointGeom::Hinge { at, axis, s0, s1, r_plate, c_r, .. } => {
                let mid = Self::p(*at, *axis, 0.5 * (s0 + s1));
                let half = 0.5 * (s1 - s0) + 0.5;
                let rad = r_plate + c_r + 1.0;
                vec![(mid, (half * half + rad * rad).sqrt())]
            }
            JointGeom::Ball { c, r, clr, wall, up_hi, .. } => vec![(*c, Self::ball_reach(*r, *clr, *wall, *up_hi) + 1.0)],
            JointGeom::Slider(w) => w.keep_out(),
            JointGeom::Bearing(r) => r.keep_out(),
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
                let (c, r) = self.keep_out()[0];
                (sub(c, [r; 3]), add(c, [r; 3]))
            }
            JointGeom::Slider(w) => w.region(),
            JointGeom::Bearing(r) => r.region(),
        }
    }

    /// A's solid with this joint's half.
    pub fn apply_a(&self, a: Solid) -> Solid {
        match *self {
            JointGeom::Hinge { at, axis, s0, s1, r_pin, r_plate, t_plate, c_r, c_z, ref brake, up, floors, .. } => {
                let p = |s: f64| Self::p(at, axis, s);
                // a braked pin is grooved where the teeth print
                let pin = Solid::Prim(Prim::cyl(p(s0), p(s1), r_pin));
                let pin = match brake {
                    Some(b) => Solid::diff(pin, b.grooves(at, axis, c_z)),
                    None => pin,
                };
                let span = (s0 + t_plate - EPS, s1 - t_plate + EPS);
                let mut cuts = vec![Solid::Prim(Prim::cyl(p(span.0), p(span.1), r_plate + c_r))];
                cuts.extend(Self::slot(at, axis, up, span, (s0, s1), r_plate + c_r, floors[0]));
                Solid::Union(vec![
                    Solid::diff(a, cuts),
                    // flush with the parts' outer faces (a hair inset leaves the mesh boolean slivers it cannot close)
                    Solid::Prim(Prim::cyl(p(s0), p(s0 + t_plate), r_plate)),
                    Solid::Prim(Prim::cyl(p(s1 - t_plate), p(s1), r_plate)),
                    pin,
                ])
            }
            JointGeom::Ball { c, r, clr, wall, axis, rn, cone, up, up_lo, up_hi, seat, .. } => {
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
                // The shell is clipped to A's own extent along up (it never pokes under the bed or over A's
                // top); on the bed it comes down to the bed, round the ball's flat, whatever A's own bottom.
                let lo = match seat {
                    Seat::Bed { ground } => ground,
                    Seat::Neck => c_up + up_lo,
                };
                let shell = Solid::Inter(vec![
                    teardrop(rc + wall),
                    Solid::Prim(Prim::Slab { n: up, lo: lo + 5.0 * EPS, hi: c_up + up_hi - 5.0 * EPS, c, half: Self::ball_reach(r, clr, wall, up_hi) + 2.0 }),
                ]);
                Solid::diff(Solid::Union(vec![a, shell]), vec![teardrop(rc), Solid::Prim(opening)])
            }
            JointGeom::Slider(ref w) => w.apply_a(a),
            JointGeom::Bearing(ref r) => r.apply_a(a),
        }
    }

    /// B's solid with this joint's half.
    pub fn apply_b(&self, b: Solid) -> Solid {
        match *self {
            JointGeom::Hinge { at, axis, s0, s1, r_pin, r_plate, t_plate, c_r, c_z, ref brake, up, floors, .. } => {
                let p = |s: f64| Self::p(at, axis, s);
                let (e0, e1) = (s0 + t_plate + c_z, s1 - t_plate - c_z);
                let rr = r_plate + c_r;
                let mut plates = Vec::new();
                for span in [(s0 - 1.0, e0 + EPS), (e1 - EPS, s1 + 1.0)] {
                    plates.push(Solid::Prim(Prim::cyl(p(span.0), p(span.1), rr)));
                    plates.extend(Self::slot(at, axis, up, span, (s0, s1), rr, floors[1]));
                }
                let body = Solid::diff(b, plates);
                let mut cuts = vec![Solid::Prim(Prim::cyl(p(s0 - 1.0), p(s1 + 1.0), r_pin + c_r))];
                // the shoes print in pockets of the eye, fused to it
                if let Some(b) = brake {
                    cuts.extend(b.pockets(at, axis, b.outer, (e0 - EPS, e1 + EPS)));
                }
                Solid::diff(Solid::Union(vec![body, Solid::Prim(Prim::cyl(p(e0), p(e1), r_plate))]), cuts)
            }
            JointGeom::Ball { c, r, clr, wall, axis, rn, up, up_hi, seat, .. } => {
                let reach = Self::ball_reach(r, clr, wall, up_hi);
                let ball = Solid::Union(vec![Solid::Prim(Prim::Sphere { c, r }), Solid::Prim(Prim::cyl(c, madd(c, axis, reach + 0.5), rn))]);
                // on the bed the ball and its neck are cut flat there
                let ball = match seat {
                    Seat::Bed { ground } => {
                        Solid::Inter(vec![ball, Solid::Prim(Prim::Slab { n: up, lo: ground, hi: dot(c, up) + reach + 1.0, c, half: reach + 1.0 })])
                    }
                    Seat::Neck => ball,
                };
                Solid::Union(vec![Solid::diff(b, vec![Solid::Prim(Prim::Sphere { c, r: reach })]), ball])
            }
            JointGeom::Slider(ref w) => w.apply_b(b),
            JointGeom::Bearing(ref r) => r.apply_b(b),
        }
    }

    /// A brake's shoe part with this joint's shoes: what of it lies in them.
    pub fn apply_shoe(&self, shoe: Solid) -> Solid {
        match self {
            JointGeom::Hinge { at, axis, brake: Some(b), .. } => Solid::Inter(vec![shoe, b.shoes(*at, *axis)]),
            _ => shoe,
        }
    }

    /// The brake's shoe part, when it has one.
    pub fn shoe(&self) -> Option<usize> {
        match self {
            JointGeom::Hinge { brake: Some(b), .. } => Some(b.shoe),
            _ => None,
        }
    }

    /// Ball joints: how far the ball reaches under the narrower opening's edge, the neck's or the
    /// bed's (>= 0.3 mm keeps it captive).
    pub fn retention(&self) -> Option<f64> {
        match *self {
            JointGeom::Ball { c, r, clr, rn, cone, up, seat, .. } => {
                let rc = r + clr;
                let open = (cone + ((rn + clr) / rc).asin()).min(std::f64::consts::FRAC_PI_2);
                let neck = r - rc * open.sin();
                // on the bed it also passes the socket's opening there
                let bed = match seat {
                    Seat::Bed { ground } => r - (rc * rc - (dot(c, up) - ground).powi(2)).max(0.0).sqrt(),
                    Seat::Neck => f64::INFINITY,
                };
                Some(neck.min(bed))
            }
            JointGeom::Slider(ref w) => Some(w.chevron.retention()),
            JointGeom::Bearing(ref r) => Some(r.chevron.retention()),
            JointGeom::Hinge { .. } => None,
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
        if let Some(s) = g.shoe() {
            let ss = std::mem::replace(&mut out[s], Solid::Part(s));
            out[s] = g.apply_shoe(ss);
        }
    }
    for (anchor, extent) in anchors {
        let s = std::mem::replace(&mut out[anchor.part], Solid::Part(anchor.part));
        out[anchor.part] = anchor.apply(s, *extent);
    }
    out
}
