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
//! 3. [`flex::measure_all`]: where parts bonded to a flexible part bend, on
//!    the final parts (the host keeps them in [`Plan::flexes`]).
//! 4. [`assembly`]: bodies, joints and anchors for [`sim::simulate`] or a live
//!    [`sim::MechWorld`].

pub mod bead;
pub mod check;
pub mod dyn_world;
pub mod flex;
pub mod geom;
pub mod hulls;
pub mod joint;
mod keep_out;
pub mod mass;
pub mod runner;
pub mod sim;
pub mod way;
pub mod weave;

pub use check::{Found, Gap, LayerReport, Risk, Tack, Turned};
pub use flex::{BondDecl, Flex};
pub use geom::{Prim, Solid, P3};
pub use joint::{AnchorDecl, BallDecl, BearingDecl, Brake, BrakeDecl, Extent, BRAKE_SQUEEZE, HingeDecl, JointDecl, JointGeom, Material, Measured, MotorDecl, MotorKind, Note, PartBox, Profile, Seat, SliderDecl};

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
    /// Parts bonded to others as they print, and the soft joints of those bonded to a flexible
    /// part as measured on the final parts ([`flex::measure_all`]; empty until the host does).
    pub bonds: Vec<BondDecl>,
    pub flexes: Vec<Flex>,
    pub profile: Profile,
}

/// A planning error: the joint (index) and why.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanError {
    pub joint: usize,
    pub message: String,
}

/// A bond that cannot be: the bond (index) and why.
#[derive(Clone, Debug, PartialEq)]
pub struct BondError {
    pub bond: usize,
    pub message: String,
}

/// Plan the joints; `bonds` must name parts in range, each bond a part and others than it (see
/// [`check_bonds`]).
pub fn plan(parts: &[PartBox], decls: &[JointDecl], bonds: &[BondDecl], anchors: &[AnchorDecl], profile: &Profile) -> Result<Plan, PlanError> {
    let mut joints = Vec::new();
    let mut notes = Vec::new();
    for (i, d) in decls.iter().enumerate() {
        let (a, b) = d.parts();
        if a == b || a >= parts.len() || b >= parts.len() {
            return Err(PlanError { joint: i, message: "a joint needs two different parts".into() });
        }
        if let Some(s) = d.shoe() {
            let fail = |message: &str| Err(PlanError { joint: i, message: message.into() });
            if s >= parts.len() || s == a || s == b {
                return fail("a brake's shoe is a part of its own, not one the hinge joins");
            }
            if decls.iter().any(|o| o.parts().0 == s || o.parts().1 == s) {
                return fail("a brake's shoe cannot be a part a joint joins");
            }
            if decls[..i].iter().any(|o| o.shoe() == Some(s)) {
                return fail("each brake needs a shoe part of its own");
            }
        }
    }
    let ground = grounds(parts, decls, bonds, norm(profile.up));
    for (i, d) in decls.iter().enumerate() {
        let (a, b) = d.parts();
        let mut n = Vec::new();
        let g = match d {
            JointDecl::Hinge(h) => joint::size_hinge(h, &parts[a], &parts[b], profile, &mut n),
            JointDecl::Ball(bd) => joint::size_ball(bd, &parts[a], &parts[b], ground[a], profile, &mut n),
            JointDecl::Slider(sd) => way::size_slider(sd, &parts[a], &parts[b], ground[a], profile, &mut n).map(JointGeom::Slider),
            JointDecl::Bearing(rd) => way::size_bearing(rd, &parts[a], &parts[b], ground[a], profile, &mut n).map(JointGeom::Bearing),
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
    let mut solids = joint::assemble(parts.len(), &joints, decls, &anchor_ext);
    // a bonded part has the part it is bonded to cut out of it, exactly: they print touching
    for bond in bonds {
        for &p in &bond.parts {
            let s = std::mem::replace(&mut solids[p], Solid::Part(p));
            solids[p] = Solid::diff(s, vec![Solid::Part(bond.to)]);
        }
    }
    Ok(Plan { joints, decls: decls.to_vec(), notes, solids, anchors: anchors.to_vec(), bonds: bonds.to_vec(), flexes: Vec::new(), profile: profile.clone() })
}

/// Why these bonds cannot be, the first that cannot: each names its part and at least one other,
/// all in range and none twice in it.
pub fn check_bonds(parts: usize, bonds: &[BondDecl]) -> Result<(), BondError> {
    for (i, b) in bonds.iter().enumerate() {
        let fail = |message: &str| Err(BondError { bond: i, message: message.into() });
        if b.parts.is_empty() {
            return fail("a bond needs parts to bond");
        }
        if b.to >= parts || b.parts.iter().any(|p| *p >= parts) {
            return fail("a bond names a part that is not there");
        }
        if b.parts.contains(&b.to) {
            return fail("a part cannot be bonded to itself");
        }
        if b.parts.iter().enumerate().any(|(k, p)| b.parts[..k].contains(p)) {
            return fail("a part is bonded twice in one bond");
        }
    }
    Ok(())
}

/// Per part: the bed under it along `up`, the lowest point of the parts its joints connect it to
/// (they print in place together, dropped as one; a brake's shoes with the part they print in).
fn grounds(parts: &[PartBox], decls: &[JointDecl], bonds: &[BondDecl], up: P3) -> Vec<f64> {
    let mut group: Vec<usize> = (0..parts.len()).collect();
    fn root(g: &mut [usize], mut i: usize) -> usize {
        while g[i] != i {
            g[i] = g[g[i]];
            i = g[i];
        }
        i
    }
    let shoes = decls.iter().filter_map(|d| d.shoe().map(|s| (d.parts().1, s)));
    let pairs = decls.iter().map(|d| d.parts()).chain(shoes).chain(bonds.iter().flat_map(|b| b.parts.iter().map(|p| (b.to, *p))));
    for (a, b) in pairs {
        let (ra, rb) = (root(&mut group, a), root(&mut group, b));
        group[ra.max(rb)] = ra.min(rb);
    }
    let mut low = vec![f64::INFINITY; parts.len()];
    for (i, p) in parts.iter().enumerate() {
        let r = root(&mut group, i);
        low[r] = low[r].min(p.extent([0.0; 3], up).lo);
    }
    (0..parts.len()).map(|i| low[root(&mut group, i)]).collect()
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
    fn distance_bounded(&self, p: makepad_csg_math::Vec3d, lower: f64, upper: f64, hints: &mut makepad_csg_sdf::Hints) -> f64 {
        self.solid.distance_bounded(&self.parts, p3(p), lower, upper, hints)
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
    /// (gap, layers, limits) milliseconds; the layers' include those sliced ahead of the gap
    /// search.
    pub phases: (f64, f64, f64),
    /// Where each way of the sweep stopped and where the parts touch there (a hinge's two ways,
    /// a ball's four).
    pub turns: Vec<Turned>,
    /// Why it is not free as printed, worst first: empty when [`JointCheck::ok`].
    pub binds: Vec<Bind>,
}

impl JointCheck {
    /// Free to move as printed: no fused verdict and a positive range.
    pub fn ok(&self) -> bool {
        self.risk < Risk::High && self.gap.gap >= GAP_TOL && self.gap.gap > 0.5 * self.clearance && self.effective.1 - self.effective.0 > min_range(self.kind) && self.retention.map_or(true, |r| r >= 0.2)
    }
}

/// The least range a joint of this kind moves through to be free: two degrees, a slider's half a
/// millimetre.
pub fn min_range(kind: &str) -> f64 {
    if kind == "slider" { 0.5 } else { 2f64.to_radians() }
}

/// The range a joint of `kind` moves through (`effective`), when too short to be free.
pub fn short_range(kind: &str, effective: (f64, f64)) -> Option<f64> {
    let range = effective.1 - effective.0;
    (range <= min_range(kind)).then_some(range)
}

/// What keeps a joint from moving as printed (see [`JointCheck::binds`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bind {
    /// The parts overlap (or touch) at `at`: the gap there, mm (negative: they overlap by at
    /// least that much).
    Fused { gap: f64, at: P3 },
    /// They come closer than half the clearance the joint was built with, at `at`.
    Tight { gap: f64, at: P3 },
    /// One part's beads print over the other's in this layer, with `empty` layers between (none:
    /// straight onto it).
    Tacks(Tack),
    /// In the layer at height `z` the two parts' sections come this close (mm): too close for
    /// the slicer to keep them apart.
    Sections { gap: f64, z: f64 },
    /// It turns through less than two degrees (radians; a slider: slides less than half a
    /// millimetre, mm).
    Range { range: f64 },
    /// The pin holds the parts by this little (mm).
    Loose { retention: f64 },
    /// As printed: support is printed between the two parts.
    Supports,
}

/// The gap search's tolerance (mm): parts closer than this touch, as far as it can tell.
pub const GAP_TOL: f64 = 0.02;

/// Layers [`check_joints`] slices ahead of a joint's gap search, to time its layer check by.
pub const LAYERS_AHEAD: usize = 3;

/// Why a joint checked like this is not free, by the same tests as [`JointCheck::ok`] (a check
/// as printed adds [`Bind::Supports`] where support bridges the parts).
/// `short`: the range it moves through, when under its kind's [`min_range`] ([`short_range`]).
pub fn binds(gap: &Gap, layers: &LayerReport, score: f64, clearance: f64, c_xy: f64, short: Option<f64>, retention: Option<f64>) -> Vec<Bind> {
    let mut out = Vec::new();
    if gap.gap < GAP_TOL {
        out.push(Bind::Fused { gap: gap.gap, at: gap.at });
    } else if gap.gap <= 0.5 * clearance {
        out.push(Bind::Tight { gap: gap.gap, at: gap.at });
    }
    // the layer whose beads weigh most in the score: straight onto the other part first
    let weight = [3.0, 1.0, 0.3, 0.05];
    let worst = |onto: bool| layers.tacks.iter().filter(|t| !onto || t.empty == 0).max_by(|a, b| (weight[a.empty] * a.area).total_cmp(&(weight[b.empty] * b.area))).copied();
    if layers.tack[0] > 0.5 {
        out.extend(worst(true).map(Bind::Tacks));
    } else if score > 40.0 {
        out.extend(worst(false).map(Bind::Tacks));
    }
    if layers.layer_gap < 0.7 * c_xy {
        out.push(Bind::Sections { gap: layers.layer_gap, z: layers.layer_gap_at });
    }
    if let Some(range) = short {
        out.push(Bind::Range { range });
    }
    if let Some(r) = retention.filter(|r| *r < 0.2) {
        out.push(Bind::Loose { retention: r });
    }
    out
}

/// Where a running check reports to, from its threads (joints are checked in parallel; one
/// joint's steps come in order). A closure `Fn() -> bool` is a sink that only cancels.
pub trait CheckSink: Sync {
    /// Joint `joint`'s check found this.
    fn found(&self, joint: usize, found: Found);
    /// Stop: polled at every step, so a check stops within one.
    fn cancelled(&self) -> bool;
}

impl<F: Fn() -> bool + Sync> CheckSink for F {
    fn found(&self, _: usize, _: Found) {}
    fn cancelled(&self) -> bool {
        self()
    }
}

/// The lowest point of the piece along `up` (the bed under an in-place print). A bed at zero is
/// +0.0 (`+ 0.0`), whichever sign the lowest vertex's zero has: `min` ties them by order.
pub fn bed_level(parts: &[FinalPart], up: P3) -> f64 {
    let up = norm(up);
    parts.iter().flat_map(|p| p.vertices.iter()).map(|v| dot(*v, up)).fold(f64::INFINITY, f64::min) + 0.0
}

/// What a joint's sweeps found: its kind, its declared range, where the parts touch each way
/// (positive amounts back and on), the range the sim takes, and each sweep's stop.
pub struct Swept {
    pub kind: &'static str,
    pub declared: Option<(f64, f64)>,
    pub field: (f64, f64),
    pub effective: (f64, f64),
    pub turns: Vec<Turned>,
}

/// Sweep a joint's moving part about (or along) `axis` through `pivot` inside `region`, every
/// way its kind moves, by `sweep` (the fields it runs on are the caller's): a hinge's and a
/// bearing's two turns, a ball's four tilts, a slider's two slides a millimetre past its travel
/// (where it stops is its end stop). None when `sweep` is stopped.
pub fn sweep_joint(g: &JointGeom, pivot: P3, axis: P3, region: (P3, P3), contact: f64, sweep: &mut dyn FnMut(check::Sweep) -> Option<Turned>) -> Option<Swept> {
    let pi = std::f64::consts::PI;
    let (lo, hi) = region;
    let mut turns = Vec::new();
    let mut run = |turns: &mut Vec<Turned>, axis: P3, dir: f64, max: f64, turn: (usize, usize), slide: bool| -> Option<f64> {
        let t = sweep(check::Sweep { pivot, axis, dir, lo, hi, contact, max, turn, slide })?;
        turns.push(t);
        Some(t.angle.abs())
    };
    let (kind, declared, field, effective) = match g {
        JointGeom::Hinge { limits, .. } => {
            let up = run(&mut turns, axis, 1.0, pi, (0, 2), false)?;
            let dn = run(&mut turns, axis, -1.0, pi, (1, 2), false)?;
            let (l, u) = limits.unwrap_or((-pi, pi));
            ("hinge", *limits, (dn, up), (l.max(-dn), u.min(up)))
        }
        JointGeom::Ball { cone, .. } => {
            let (u, v) = basis(axis);
            let mut m = pi;
            for (k, (t, dir)) in [(u, 1.0), (u, -1.0), (v, 1.0), (v, -1.0)].into_iter().enumerate() {
                m = m.min(run(&mut turns, t, dir, pi * 0.5, (k, 4), false)?);
            }
            ("ball", Some((0.0, *cone)), (0.0, m), (0.0, cone.min(m)))
        }
        JointGeom::Slider(w) => {
            let on = run(&mut turns, axis, 1.0, w.travel.1 + 1.0, (0, 2), true)?;
            let back = run(&mut turns, axis, -1.0, -w.travel.0 + 1.0, (1, 2), true)?;
            ("slider", Some(w.travel), (back, on), (w.travel.0.max(-back), w.travel.1.min(on)))
        }
        JointGeom::Bearing(_) => {
            let on = run(&mut turns, axis, 1.0, pi, (0, 2), false)?;
            let back = run(&mut turns, axis, -1.0, pi, (1, 2), false)?;
            ("bearing", None, (back, on), (-back, on))
        }
    };
    Some(Swept { kind, declared, field, effective, turns })
}

/// Check every joint on the parts' fields (in parallel), telling `sink` every step and stopping
/// within one once it cancels: a cancelled joint is left out. A closure `|| false` checks them
/// all and reports nothing.
pub fn check_joints(plan: &Plan, parts: &[FinalPart], geometry: Geometry, sink: &dyn CheckSink) -> Vec<JointCheck> {
    let up = norm(plan.profile.up);
    let bed = bed_level(parts, up);
    let c_xy = plan.profile.c_xy();
    let one = |i: usize| -> Option<JointCheck> {
        if sink.cancelled() {
            return None;
        }
        let step = &mut |f: Found| {
            sink.found(i, f);
            !sink.cancelled()
        };
        let started = std::time::Instant::now();
        let g = &plan.joints[i];
        let (a, b) = plan.decls[i].parts();
        let (fa, fb) = (parts[a].field, parts[b].field);
        // a brake's shoes print in B: they keep clear of A as it prints, as B does, but give as
        // it turns (the sweep turns B alone)
        let welded = g.shoe().map(|s| Welded(fb, parts[s].field));
        let fbs: &(dyn Sdf3 + Sync) = match &welded {
            Some(w) => w,
            None => fb,
        };
        let (lo, hi) = g.region();
        // a few layers first, so a bar knows the layer check's time while the gap search runs
        let slicer = check::Slicer::new(fa, fbs, check::LayerSetup { lo, hi, up, layer: plan.profile.layer, res: 0.05, bed });
        let ahead = slicer.ahead(LAYERS_AHEAD, step)?;
        let t_ahead = started.elapsed().as_secs_f64() * 1000.0;
        let gap = check::min_gap_steps(fa, fbs, lo, hi, GAP_TOL, step)?;
        let t_gap = started.elapsed().as_secs_f64() * 1000.0 - t_ahead;
        let layers = slicer.check(ahead, step)?;
        let t_layers = started.elapsed().as_secs_f64() * 1000.0 - t_gap;
        let (risk, risk_score) = check::risk(gap.gap, layers.layer_gap, &layers.tack, c_xy);
        let (pivot, axis) = g.pivot();
        let Swept { kind, declared, field, effective, turns } = sweep_joint(g, pivot, axis, (lo, hi), 0.02, &mut |s| check::sweep_steps(fa, fb, s, step))?;
        let binds = binds(&gap, &layers, risk_score, g.clearance(), c_xy, short_range(kind, effective), g.retention());
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
            turns,
            binds,
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

/// A part with a brake's shoes printed in it, as one field.
struct Welded<'a>(&'a (dyn Sdf3 + Sync), &'a (dyn Sdf3 + Sync));

impl Sdf3 for Welded<'_> {
    fn distance(&self, p: makepad_csg_math::Vec3d) -> f64 {
        self.0.distance(p).min(self.1.distance(p))
    }
    fn distance_bounded(&self, p: makepad_csg_math::Vec3d, lower: f64, upper: f64, hints: &mut makepad_csg_sdf::Hints) -> f64 {
        let d = self.0.distance_bounded(p, lower, upper, hints.part(0));
        if d <= lower {
            return d;
        }
        self.1.distance_bounded(p, lower, d, hints.part(1))
    }
}

/// Convex hulls of a part away from its joints: its vertices outside every keep-out sphere, cut into slabs along
/// its longest side, each slab's hull as few of its corners as keep within [`GAP_TOL`] of it
/// ([`hulls::coarse_hull`]).
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
    // A slab is a solid piece only when its points span it: the few corners of a coarse mesh leave a slab
    // with a single face of points (flat: no hull) or none, so such slabs grow into the next until they do.
    let solid = |slab: &[P3]| {
        let (mut a, mut b) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for p in slab {
            for k in 0..3 {
                a[k] = a[k].min(p[k]);
                b[k] = b[k].max(p[k]);
            }
        }
        slab.len() >= 4 && b[ax] - a[ax] >= 0.5 * w && (0..3).all(|k| b[k] - a[k] > 1e-3 * thin)
    };
    let mut out: Vec<Vec<P3>> = Vec::new();
    let mut open: Vec<P3> = Vec::new();
    for s in 0..slabs {
        let (a, b) = (lo[ax] + s as f64 * w - 0.05 * w, lo[ax] + (s + 1) as f64 * w + 0.05 * w);
        open.extend(pts.iter().copied().filter(|p| p[ax] >= a && p[ax] <= b));
        if solid(&open) {
            out.push(std::mem::take(&mut open));
        }
    }
    if !open.is_empty() {
        match out.last_mut() {
            Some(last) => last.extend(open),
            None => out.push(open),
        }
    }
    out.retain(|slab| slab.len() >= 4);
    out.iter().map(|slab| hulls::coarse_hull(slab, GAP_TOL)).collect()
}

/// The sim's view of the piece: a body per part (mass from its mesh at `fill`), a joint per joint with its
/// effective limits and pin friction, then each bond's: a part bonded to a rigid part welded to it; parts
/// bonded to a flexible part bent each to the next by their [`Plan::flexes`], the flexible part itself
/// riding the first of them (it weighs nothing and meets nothing: the host shows it bent with them, and its
/// own weight, a few percent of theirs, is left out). The anchors.
pub fn assembly(plan: &Plan, parts: &[FinalPart], checks: &[JointCheck], fill: f64) -> sim::Assembly {
    let density = plan.profile.material.density;
    let mu = plan.profile.measured.mu.unwrap_or(plan.profile.material.mu);
    let mut keep: Vec<Vec<(P3, f64)>> = vec![Vec::new(); parts.len()];
    for (g, d) in plan.joints.iter().zip(&plan.decls) {
        let (a, b) = d.parts();
        let k = g.keep_out();
        keep[a].extend(k.iter().copied());
        if let Some(s) = g.shoe() {
            keep[s].extend(k.iter().copied());
        }
        keep[b].extend(k);
    }
    for f in &plan.flexes {
        keep[f.a].push(f.keep_out());
        keep[f.b].push(f.keep_out());
    }
    let riders: Vec<usize> = plan.bonds.iter().filter(|b| b.flexible).map(|b| b.to).collect();
    let bodies = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut body = mesh_body(p.name, p.vertices, p.triangles, density, fill, &keep[i]);
            if riders.contains(&i) {
                body.hulls.clear();
                body.mass.mass *= RIDER_WEIGHT;
                body.mass.inertia = body.mass.inertia.map(|r| r.map(|x| x * RIDER_WEIGHT));
            }
            body
        })
        .collect();
    let joints = plan
        .joints
        .iter()
        .zip(&plan.decls)
        .enumerate()
        .map(|(i, (g, d))| {
            let (a, b) = d.parts();
            let (pivot, axis) = g.pivot();
            let declared = match g {
                JointGeom::Hinge { limits, .. } => {
                    let (l, u) = limits.unwrap_or((-std::f64::consts::PI, std::f64::consts::PI));
                    sim::JointKind::Revolute { lower: l, upper: u }
                }
                JointGeom::Ball { cone, twist, .. } => sim::JointKind::Spherical { cone: *cone, twist: *twist },
                JointGeom::Slider(w) => sim::JointKind::Prismatic { lower: w.travel.0, upper: w.travel.1 },
                JointGeom::Bearing(_) => sim::JointKind::Revolute { lower: -std::f64::consts::PI, upper: std::f64::consts::PI },
            };
            let kind = match checks.iter().find(|c| c.joint == i) {
                Some(c) => declared.measured(c.effective),
                None => declared,
            };
            let collide = matches!(g, JointGeom::Hinge { .. } | JointGeom::Ball { .. });
            let brake = match g {
                JointGeom::Hinge { brake: Some(bk), .. } => Some(sim::BrakeSpec { torque: (0..=sim::BRAKE_SAMPLES).map(|k| bk.torque(k as f64 * sim::BRAKE_STEP) * sim::NEWTON).collect() }),
                _ => None,
            };
            // a hinge's limit is in N mm, a slider's in N: both a million of the sim's units
            let motor = d.motor().map(|m| sim::Motor { kind: m.kind, limit: m.limit * sim::NEWTON });
            sim::JointSpec { name: format!("{}-{}", parts[a].name, parts[b].name), a, b, pivot, axis, kind, friction: sim::Friction { mu, radius: g.bearing_radius(), torque: 0.0 }, collide, brake, motor }
        })
        .chain(bond_joints(plan, parts))
        .chain(plan.joints.iter().zip(&plan.decls).filter_map(|(g, d)| g.shoe().map(|s| shoe_weld(plan, parts, d.parts().1, s))))
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
    sim::Assembly { bodies, joints, anchors, up: plan.profile.up, sleep: sim::SLEEP_SPEED, workers: 1, contact_margin: 0.0, max_speed: 0.0, contact_hertz: 0.0, substeps: sim::JOINT_SUBSTEPS, ball: None, shake_about: [0.0; 3], shake_tempo: 0.0, lift: 0.0, strings: Vec::new(), interlocks: Vec::new() }
}


/// A brake's shoes ride the part they print in.
fn shoe_weld(plan: &Plan, parts: &[FinalPart], b: usize, shoe: usize) -> sim::JointSpec {
    sim::JointSpec {
        name: format!("{}+{}", parts[b].name, parts[shoe].name),
        a: b,
        b: shoe,
        pivot: flex::centre(&parts[shoe]),
        axis: plan.profile.up,
        kind: sim::JointKind::Weld,
        friction: sim::Friction::default(),
        collide: false,
        brake: None,
        motor: None,
    }
}

/// What a flexible part a row of parts is bonded to weighs in the sim, of what it would: it rides the
/// first of them and the soft joints between them carry its stiffness.
const RIDER_WEIGHT: f64 = 1e-4;

/// The bonds' joints: welds to rigid parts, a flexible part's weld to the first part on it and the
/// soft joints between the parts along it.
fn bond_joints<'a>(plan: &'a Plan, parts: &'a [FinalPart]) -> impl Iterator<Item = sim::JointSpec> + 'a {
    let weld = move |a: usize, b: usize| sim::JointSpec {
        name: format!("{}+{}", parts[a].name, parts[b].name),
        a,
        b,
        pivot: flex::centre(&parts[b]),
        axis: plan.profile.up,
        kind: sim::JointKind::Weld,
        friction: sim::Friction::default(),
        collide: false,
        brake: None,
        motor: None,
    };
    let welds = plan.bonds.iter().flat_map(move |b| if b.flexible { vec![weld(b.parts[0], b.to)] } else { b.parts.iter().map(|p| weld(b.to, *p)).collect() });
    let flexes = plan.flexes.iter().map(move |f| {
        let (soft, stiff) = f.stiffness(flex::FLEX_MODULUS);
        let one_way = f.one_way();
        sim::JointSpec {
            name: format!("{}-{}", parts[f.a].name, parts[f.b].name),
            a: f.a,
            b: f.b,
            pivot: f.pivot,
            axis: if one_way { f.section.soft_axis } else { f.axis },
            kind: sim::JointKind::Flex { stiffness: if one_way { soft } else { (soft * stiff).sqrt() }, damping: flex::FLEX_DAMPING, one_way },
            friction: sim::Friction::default(),
            collide: true,
            brake: None,
            motor: None,
        }
    });
    flexes.chain(welds)
}

/// A rigid part from its final mesh: the same mass and collision proxies for free and jointed
/// bodies. All vertices are in the assembly's common frame (mm); free parts have no keep-outs.
pub fn mesh_body(name: &str, vertices: &[P3], triangles: &[[u32; 3]], density: f64, fill: f64, keep_out: &[(P3, f64)]) -> sim::BodySpec {
    sim::BodySpec { name: name.into(), mass: mass::mesh_mass(vertices, triangles, density, fill), hulls: proxy_hulls(vertices, keep_out), tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: None }
}

/// A rigid part from its final mesh when that keeps to its convex hull ([`hulls::fits_its_hull`] to
/// [`GAP_TOL`]: a ball, a roller, a pin): its one coarse hull is all of it, and it meets the fields of
/// the parts around it through that hull (a field of its own would be the same shape, for more work).
pub fn convex_body(name: &str, vertices: &[P3], triangles: &[[u32; 3]], density: f64, fill: f64) -> sim::BodySpec {
    let hulls = vec![hulls::coarse_hull(vertices, GAP_TOL)];
    sim::BodySpec { name: name.into(), mass: mass::mesh_mass(vertices, triangles, density, fill), hulls, tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: None }
}

/// A mesh's loose pieces, each with its own (welded) vertices and triangles, in the mesh's frame: what of
/// it prints as separate solids. A mesh may hold several closed shells (corners at one place, to 10 nm,
/// are one vertex). A shell facing in (its volume negative) is a hollow of the smallest shell around it, and
/// a piece is a shell with its hollows. Pieces that come within `fused` mm of each other, or overlap,
/// print as one and are one piece. One piece: the mesh as it is.
pub fn mesh_pieces(vertices: &[P3], triangles: &[[u32; 3]], fused: f64) -> Vec<(Vec<P3>, Vec<[u32; 3]>)> {
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    // the shells: triangles joined by the corners they share
    let mut welded: std::collections::HashMap<[i64; 3], usize> = std::collections::HashMap::new();
    let corner: Vec<usize> = vertices.iter().map(|p| { let n = welded.len(); *welded.entry(p.map(|v| (v * 1e5).round() as i64)).or_insert(n) }).collect();
    let mut joined: Vec<usize> = (0..welded.len()).collect();
    for t in triangles {
        let a = root(&mut joined, corner[t[0] as usize]);
        for &v in &t[1..] {
            let b = root(&mut joined, corner[v as usize]);
            if a != b {
                joined[b] = a;
            }
        }
    }
    let mut shell_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut shells: Vec<Vec<usize>> = Vec::new();
    for (k, t) in triangles.iter().enumerate() {
        let r = root(&mut joined, corner[t[0] as usize]);
        let s = *shell_of.entry(r).or_insert_with(|| { shells.push(Vec::new()); shells.len() - 1 });
        shells[s].push(k);
    }
    if shells.len() < 2 {
        return vec![(vertices.to_vec(), triangles.to_vec())];
    }
    // each shell with its own vertices
    let mesh = |tris: &mut dyn Iterator<Item = usize>| -> (Vec<P3>, Vec<[u32; 3]>) {
        let mut index: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
        let (mut pos, mut out) = (Vec::new(), Vec::new());
        for k in tris {
            out.push(triangles[k].map(|v| *index.entry(corner[v as usize]).or_insert_with(|| { pos.push(vertices[v as usize]); pos.len() as u32 - 1 })));
        }
        (pos, out)
    };
    let field = |(pos, tris): &(Vec<P3>, Vec<[u32; 3]>)| makepad_csg_sdf::mesh_field::MeshField::new(makepad_csg_sdf::mesh_field::MeshData { positions: pos.iter().map(|p| p.map(|v| v as f32)).collect(), triangles: tris.clone() });
    let span = |pos: &[P3]| pos.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), p| (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k]))));
    let volume = |tris: &[usize]| tris.iter().map(|&k| { let [a, b, c] = triangles[k].map(|v| vertices[v as usize]); dot(a, cross(b, c)) }).sum::<f64>() / 6.0;
    let volumes: Vec<f64> = shells.iter().map(|t| volume(t)).collect();
    let solids: Vec<usize> = (0..shells.len()).filter(|&s| volumes[s] >= 0.0).collect();
    let alone: Vec<(Vec<P3>, Vec<[u32; 3]>)> = solids.iter().map(|&s| mesh(&mut shells[s].iter().copied())).collect();
    let alone_fields: Vec<_> = alone.iter().map(field).collect();
    // a hollow belongs to the smallest solid shell around it (one around none is a piece of its own)
    let mut hollows: Vec<Vec<usize>> = vec![Vec::new(); solids.len()];
    let mut lone = Vec::new();
    for s in (0..shells.len()).filter(|&s| volumes[s] < 0.0) {
        let at = vertices[triangles[shells[s][0]][0] as usize];
        let around = (0..solids.len()).filter(|&k| alone_fields[k].distance(makepad_csg_math::dvec3(at[0], at[1], at[2])) < 0.0).min_by(|&a, &b| volumes[solids[a]].total_cmp(&volumes[solids[b]]));
        match around {
            Some(k) => hollows[k].push(s),
            None => lone.push(s),
        }
    }
    let mut pieces: Vec<Vec<usize>> = (0..solids.len()).map(|k| std::iter::once(solids[k]).chain(hollows[k].iter().copied()).collect()).collect();
    pieces.extend(lone.into_iter().map(|s| vec![s]));
    let meshes: Vec<(Vec<P3>, Vec<[u32; 3]>)> = pieces.iter().map(|shells_of| mesh(&mut shells_of.iter().flat_map(|&s| shells[s].iter().copied()))).collect();
    // pieces that print fused are one
    let fields: Vec<_> = meshes.iter().map(field).collect();
    let spans: Vec<(P3, P3)> = meshes.iter().map(|(pos, _)| span(pos)).collect();
    let mut one: Vec<usize> = (0..meshes.len()).collect();
    for a in 0..meshes.len() {
        for b in a + 1..meshes.len() {
            let lo: P3 = std::array::from_fn(|k| spans[a].0[k].max(spans[b].0[k]) - fused);
            let hi: P3 = std::array::from_fn(|k| spans[a].1[k].min(spans[b].1[k]) + fused);
            if (0..3).all(|k| lo[k] <= hi[k]) && check::closer_than(&fields[a], &fields[b], lo, hi, fused) {
                let (ra, rb) = (root(&mut one, a), root(&mut one, b));
                one[rb] = ra;
            }
        }
    }
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for k in 0..meshes.len() {
        let r = root(&mut one, k);
        match groups.iter_mut().find(|g| g.0 == r) {
            Some(g) => g.1.push(k),
            None => groups.push((r, vec![k])),
        }
    }
    if groups.len() < 2 {
        return vec![(vertices.to_vec(), triangles.to_vec())];
    }
    groups.iter().map(|(_, members)| mesh(&mut members.iter().flat_map(|&m| pieces[m].iter().flat_map(|&s| shells[s].iter().copied())))).collect()
}

/// A distance-field part as a body, straight from its field: the field is sampled on a grid of cells
/// (about [`FIELD_BODY_CELLS`] along the longest side), the cells inside give the mass, centre and
/// inertia, and the inside cells that touch the outside give the collision proxies' points. No mesh is
/// built (a field part is meshed for export and Mesh shading only). Everything is put into the
/// assembly's common frame by `to_frame`. Call on the scene-building worker.
pub fn field_body(name: &str, shape: &makepad_csg_graph::SdfShape, to_frame: &[[f64; 4]; 3], density: f64, fill: f64) -> Result<sim::BodySpec, String> {
    let (lo, hi) = (shape.min, shape.max);
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let longest = size.iter().copied().fold(0.0, f64::max);
    if !(longest > 0.0) {
        return Err(format!("{name} has no solid volume to simulate"));
    }
    let cell = longest / FIELD_BODY_CELLS as f64;
    let n = size.map(|s| (s / cell).ceil().max(1.0) as usize);
    let at = |i: usize, j: usize, k: usize| makepad_csg_math::Vec3d::new(lo[0] + (i as f64 + 0.5) * cell, lo[1] + (j as f64 + 0.5) * cell, lo[2] + (k as f64 + 0.5) * cell);
    let mut points = Vec::with_capacity(n[0] * n[1] * n[2]);
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                points.push(at(i, j, k));
            }
        }
    }
    let mut dist = vec![0.0; points.len()];
    shape.field().distance_many(&points, &mut dist);
    let index = |i: usize, j: usize, k: usize| (k * n[1] + j) * n[0] + i;
    let inside = |i: isize, j: isize, k: isize| i >= 0 && j >= 0 && k >= 0 && (i as usize) < n[0] && (j as usize) < n[1] && (k as usize) < n[2] && dist[index(i as usize, j as usize, k as usize)] < 0.0;
    let to = |p: makepad_csg_math::Vec3d| -> P3 { std::array::from_fn(|r| to_frame[r][0] * p.x + to_frame[r][1] * p.y + to_frame[r][2] * p.z + to_frame[r][3]) };
    let lin = [[to_frame[0][0], to_frame[0][1], to_frame[0][2]], [to_frame[1][0], to_frame[1][1], to_frame[1][2]], [to_frame[2][0], to_frame[2][1], to_frame[2][2]]];
    let det = (lin[0][0] * (lin[1][1] * lin[2][2] - lin[1][2] * lin[2][1]) - lin[0][1] * (lin[1][0] * lin[2][2] - lin[1][2] * lin[2][0]) + lin[0][2] * (lin[1][0] * lin[2][1] - lin[1][1] * lin[2][0])).abs();
    let voxel = det * cell * cell * cell;
    let mut centres = Vec::new();
    let mut skin = Vec::new();
    for k in 0..n[2] {
        for j in 0..n[1] {
            for i in 0..n[0] {
                if !inside(i as isize, j as isize, k as isize) {
                    continue;
                }
                let q = to(at(i, j, k));
                centres.push(q);
                let (a, b, c) = (i as isize, j as isize, k as isize);
                if !(inside(a - 1, b, c) && inside(a + 1, b, c) && inside(a, b - 1, c) && inside(a, b + 1, c) && inside(a, b, c - 1) && inside(a, b, c + 1)) {
                    skin.push(q);
                }
            }
        }
    }
    if centres.is_empty() {
        return Err(format!("{name} has no solid volume to simulate"));
    }
    Ok(sim::BodySpec { name: name.into(), mass: mass::voxel_mass(&centres, voxel, density, fill), hulls: proxy_hulls(&skin, &[]), tubes: Vec::new(), balls: Vec::new(), rods: Vec::new(), field: None })
}

/// Cells along the longest side of a field part sampled for its body.
pub const FIELD_BODY_CELLS: usize = 48;
