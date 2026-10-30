//! Mechanisms for printed parts: print-in-place joints declared once and
//! written into both parts with clearances swept over their motion, field
//! checks of every joint (gap, per-layer gap, fused risk, limits), and the
//! assembled piece as a box3d world.
//!
//! The flow a host runs:
//! 1. [`plan`]: joints sized from the parts' bounds and the printer
//!    [`Profile`]; every part's [`Solid`] (the original part with its joint
//!    halves). The host lowers the solids to its mesh graph
//!    ([`Solid::lower`]) or to fields ([`Solid::to_node`]).
//! 2. [`check_joints`]: on the final parts' fields (design meshes, or the
//!    slicer's beads near each joint: [`bead::BeadField`]).
//! 3. [`assembly`]: bodies, joints and anchors for [`sim::simulate`] or a live
//!    [`sim::MechWorld`].

pub mod bead;
pub mod check;
pub mod geom;
pub mod joint;
pub mod mass;
pub mod runner;
pub mod sim;

pub use check::{Gap, LayerReport, Risk};
pub use geom::{Prim, Solid, P3};
pub use joint::{AnchorDecl, BallDecl, Extent, HingeDecl, JointDecl, JointGeom, Material, Note, PartBox, Profile};

use geom::*;
use makepad_csg_sdf::Sdf3;

/// The sized joints and every part's solid.
#[derive(Clone, Debug)]
pub struct Plan {
    pub joints: Vec<JointGeom>,
    pub decls: Vec<JointDecl>,
    /// Per joint.
    pub notes: Vec<Vec<Note>>,
    /// Per part: its solid (`Solid::Part(i)` when no joint touches it).
    pub solids: Vec<Solid>,
    pub anchors: Vec<AnchorDecl>,
    pub profile: Profile,
}

/// A planning error: the joint (index) and why.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanError {
    pub joint: usize,
    pub message: String,
}

pub fn plan(parts: &[PartBox], decls: &[JointDecl], anchors: &[AnchorDecl], profile: &Profile) -> Result<Plan, PlanError> {
    let mut joints = Vec::new();
    let mut notes = Vec::new();
    for (i, d) in decls.iter().enumerate() {
        let (a, b) = d.parts();
        if a == b || a >= parts.len() || b >= parts.len() {
            return Err(PlanError { joint: i, message: "a joint needs two different parts".into() });
        }
        let mut n = Vec::new();
        let g = match d {
            JointDecl::Hinge(h) => joint::size_hinge(h, &parts[a], &parts[b], profile, &mut n),
            JointDecl::Ball(bd) => joint::size_ball(bd, &parts[a], &parts[b], profile, &mut n),
        }
        .map_err(|message| PlanError { joint: i, message })?;
        joints.push(g);
        notes.push(n);
    }
    let anchor_ext: Vec<(AnchorDecl, Extent)> = anchors
        .iter()
        .map(|a| {
            let axis = a.hole.map(|h| h.0).unwrap_or(profile.up);
            (a.clone(), parts[a.part].extent(a.at, norm(axis)))
        })
        .collect();
    let solids = joint::assemble(parts.len(), &joints, decls, &anchor_ext);
    Ok(Plan { joints, decls: decls.to_vec(), notes, solids, anchors: anchors.to_vec(), profile: profile.clone() })
}

/// A part's design field: its solid over the original parts' fields (joint primitives exact).
pub struct SolidField<'a> {
    pub solid: &'a Solid,
    pub parts: Vec<&'a (dyn Sdf3 + Sync)>,
}

impl<'a> Sdf3 for SolidField<'a> {
    fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
        self.solid.distance(&self.parts, p3(p))
    }
}

/// A final part as the checks and the sim read it.
pub struct FinalPart<'a> {
    pub name: &'a str,
    pub field: &'a (dyn Sdf3 + Sync),
    pub vertices: &'a [P3],
    pub triangles: &'a [[u32; 3]],
}

/// Which geometry a check or a sim ran on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    Design,
    AsPrinted,
}

impl Geometry {
    pub fn name(self) -> &'static str {
        match self {
            Geometry::Design => "design",
            Geometry::AsPrinted => "as_printed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JointCheck {
    pub joint: usize,
    pub kind: &'static str,
    pub a: usize,
    pub b: usize,
    /// The clearance it was built with.
    pub clearance: f64,
    pub gap: Gap,
    pub layers: LayerReport,
    pub risk: Risk,
    pub risk_score: f64,
    /// Declared limits (revolute: lower, upper; ball: 0, cone).
    pub declared: Option<(f64, f64)>,
    /// Where the parts touch when turned (revolute: -lower, upper as positive angles; ball: smallest cone).
    pub field: (f64, f64),
    /// What the sim uses.
    pub effective: (f64, f64),
    pub retention: Option<f64>,
    pub geometry: Geometry,
    pub notes: Vec<Note>,
    pub millis: f64,
    /// (gap, layers, limits) milliseconds.
    pub phases: (f64, f64, f64),
}

impl JointCheck {
    /// Free to move as printed: no fused verdict and a positive range.
    pub fn ok(&self) -> bool {
        self.risk < Risk::High && self.gap.gap > 0.5 * self.clearance && self.effective.1 - self.effective.0 > 2f64.to_radians() && self.retention.map_or(true, |r| r >= 0.2)
    }
}

/// The lowest point of the piece along `up` (the bed under an in-place print).
pub fn bed_level(parts: &[FinalPart], up: P3) -> f64 {
    let up = norm(up);
    parts.iter().flat_map(|p| p.vertices.iter()).map(|v| dot(*v, up)).fold(f64::INFINITY, f64::min)
}

/// Check every joint on the parts' fields (in parallel; `cancelled` is polled between joints).
pub fn check_joints(plan: &Plan, parts: &[FinalPart], geometry: Geometry, cancelled: &(dyn Fn() -> bool + Sync)) -> Vec<JointCheck> {
    let up = norm(plan.profile.up);
    let bed = bed_level(parts, up);
    let c_xy = plan.profile.c_xy();
    let one = |i: usize| -> Option<JointCheck> {
        if cancelled() {
            return None;
        }
        let started = std::time::Instant::now();
        let g = &plan.joints[i];
        let (a, b) = plan.decls[i].parts();
        let (fa, fb) = (parts[a].field, parts[b].field);
        let (lo, hi) = g.region();
        let gap = check::min_gap(fa, fb, lo, hi, 0.02);
        let t_gap = started.elapsed().as_secs_f64() * 1000.0;
        let layers = check::layer_check(fa, fb, lo, hi, up, plan.profile.layer, 0.05, bed);
        let t_layers = started.elapsed().as_secs_f64() * 1000.0 - t_gap;
        let (risk, risk_score) = check::risk(gap.gap, layers.layer_gap, &layers.tack, c_xy);
        let (pivot, axis) = g.pivot();
        let pi = std::f64::consts::PI;
        let (kind, declared, field, effective) = match g {
            JointGeom::Hinge { limits, .. } => {
                let up_lim = check::sweep_limit(fa, fb, pivot, axis, 1.0, lo, hi, 0.02, pi);
                let dn_lim = check::sweep_limit(fa, fb, pivot, axis, -1.0, lo, hi, 0.02, pi);
                let (l, u) = limits.unwrap_or((-pi, pi));
                ("hinge", *limits, (dn_lim, up_lim), (l.max(-dn_lim), u.min(up_lim)))
            }
            JointGeom::Ball { cone, .. } => {
                let (u, v) = basis(axis);
                let mut m: f64 = pi;
                for t in [u, v] {
                    for dir in [1.0, -1.0] {
                        m = m.min(check::sweep_limit(fa, fb, pivot, t, dir, lo, hi, 0.02, pi * 0.5));
                    }
                }
                ("ball", Some((0.0, *cone)), (0.0, m), (0.0, cone.min(m)))
            }
        };
        Some(JointCheck {
            joint: i,
            kind,
            a,
            b,
            clearance: g.clearance(),
            gap,
            layers,
            risk,
            risk_score,
            declared,
            field,
            effective,
            retention: g.retention(),
            geometry,
            notes: plan.notes[i].clone(),
            millis: started.elapsed().as_secs_f64() * 1000.0,
            phases: (t_gap, t_layers, started.elapsed().as_secs_f64() * 1000.0 - t_gap - t_layers),
        })
    };
    let n = plan.joints.len();
    let threads = std::thread::available_parallelism().map(|t| t.get()).unwrap_or(4).min(8).min(n.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= n {
                    break;
                }
                if let Some(c) = one(i) {
                    out.lock().unwrap().push(c);
                }
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|c| c.joint);
    v
}

/// Convex hulls of a part away from its joints: its vertices outside every keep-out sphere, cut into slabs along
/// its longest side.
pub fn proxy_hulls(vertices: &[P3], keep_out: &[(P3, f64)]) -> Vec<Vec<P3>> {
    let pts: Vec<P3> = vertices.iter().copied().filter(|v| keep_out.iter().all(|(c, r)| len(sub(*v, *c)) > *r)).collect();
    if pts.len() < 4 {
        return Vec::new();
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &pts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let size = sub(hi, lo);
    let ax = (0..3).max_by(|a, b| size[*a].total_cmp(&size[*b])).unwrap();
    let thin = (0..3).filter(|k| *k != ax).map(|k| size[k]).fold(f64::INFINITY, f64::min).max(0.5);
    let slabs = ((size[ax] / (2.0 * thin)).ceil() as usize).clamp(1, 8);
    let w = size[ax] / slabs as f64;
    let mut out = Vec::new();
    for s in 0..slabs {
        let (a, b) = (lo[ax] + s as f64 * w - 0.05 * w, lo[ax] + (s + 1) as f64 * w + 0.05 * w);
        let slab: Vec<P3> = pts.iter().copied().filter(|p| p[ax] >= a && p[ax] <= b).collect();
        if slab.len() >= 4 {
            out.push(slab);
        }
    }
    out
}

/// The sim's view of the piece: a body per part (mass from its mesh at `fill`), a joint per joint with its
/// effective limits and pin friction, the anchors.
pub fn assembly(plan: &Plan, parts: &[FinalPart], checks: &[JointCheck], fill: f64) -> sim::Assembly {
    let density = plan.profile.material.density;
    let mu = plan.profile.material.mu;
    let mut keep: Vec<Vec<(P3, f64)>> = vec![Vec::new(); parts.len()];
    for (g, d) in plan.joints.iter().zip(&plan.decls) {
        let (a, b) = d.parts();
        let k = g.keep_out();
        keep[a].push(k);
        keep[b].push(k);
    }
    let bodies = parts
        .iter()
        .enumerate()
        .map(|(i, p)| sim::BodySpec { name: p.name.to_string(), mass: mass::mesh_mass(p.vertices, p.triangles, density, fill), hulls: proxy_hulls(p.vertices, &keep[i]) })
        .collect();
    let joints = plan
        .joints
        .iter()
        .zip(&plan.decls)
        .enumerate()
        .map(|(i, (g, d))| {
            let (a, b) = d.parts();
            let (pivot, axis) = g.pivot();
            let eff = checks.iter().find(|c| c.joint == i).map(|c| c.effective);
            let kind = match g {
                JointGeom::Hinge { limits, .. } => {
                    let (l, u) = eff.unwrap_or(limits.unwrap_or((-std::f64::consts::PI, std::f64::consts::PI)));
                    sim::JointKind::Revolute { lower: l, upper: u }
                }
                JointGeom::Ball { cone, twist, .. } => sim::JointKind::Spherical { cone: eff.map(|e| e.1).unwrap_or(*cone), twist: *twist },
            };
            sim::JointSpec { name: format!("{}-{}", parts[a].name, parts[b].name), a, b, pivot, axis, kind, friction: sim::Friction { mu, radius: g.bearing_radius(), torque: 0.0 } }
        })
        .collect();
    let anchors = plan
        .anchors
        .iter()
        .map(|a| sim::AnchorSpec {
            body: a.part,
            pivot: a.at,
            kind: match a.hole {
                Some((axis, _)) => sim::AnchorKind::Pin(norm(axis)),
                None => sim::AnchorKind::Fixed,
            },
            friction: sim::Friction { mu, radius: a.hole.map_or(0.0, |h| h.1), torque: 0.0 },
        })
        .collect();
    sim::Assembly { bodies, joints, anchors, up: plan.profile.up }
}
