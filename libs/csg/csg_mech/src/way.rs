//! Joints that print as they stand: a slider (a carriage in a rail) and a bearing (an inner in an
//! outer race), each part standing on the bed, the moving one held in by a chevron: a ridge on
//! it, as tall as the joint, in a matching groove of the other.
//!
//! In section (across a slider's way, or radially for a bearing) the moving part's edge runs out
//! from the bed at its neck, at most 45 degrees from upright, to its ridge halfway up, and back in
//! to its neck at the top; the groove is that edge moved out by the flank gap, so its upper half
//! overhangs the ridge by no more than 45 degrees. Both print without support, and the ridge
//! passing under the groove's lip by more than the gap keeps the moving part in, up and down.
//! Above the joint the moving part's neck comes up through a slot to whatever it carries over
//! the other part, which it clears by two layers at the slot and more out from it: its underside
//! there climbs at 45 degrees from the neck, so nothing of it prints over thin air.
//!
//! A slider's groove runs as far along the rail as the carriage travels and stops short of the
//! rail's ends: the end stops. A bearing's turns all the way round.

use crate::geom::*;
use crate::joint::{Note, PartBox, Profile};

/// The narrowest wall a groove may leave in the part it is cut into (nozzle widths; and 1.2 mm).
const WALL: f64 = 3.0;
/// The least a ridge passes under its groove's lip, beyond the gap (mm).
const HOLD: f64 = 0.3;

/// The chevron's section, shared by the slider and the bearing (millimetres, along `up` absolute).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chevron {
    /// The bed along up, and the ridge's top (the lower of the two parts' tops).
    pub ground: f64,
    pub top: f64,
    /// The moving part's half-width (radius) at the bed and the top, and its ridge's reach past it.
    pub neck: f64,
    pub depth: f64,
    /// The flank's gap across (horizontal): its upper flank is that far over the ridge's.
    pub gap: f64,
    pub c_xy: f64,
    pub c_z: f64,
    /// The fixed part's top along up: over it, the moving part clears it by `c_z`.
    pub fixed_top: f64,
    /// The moving part's top along up.
    pub carry_top: f64,
}

impl Chevron {
    /// Size it: the ridge's depth (default 0.3 of its height, held to 0.6..2.5 mm and to keep it
    /// captive), and a flank gap as wide as the in-layer clearance and as tall as the across-layers
    /// one.
    fn size(ground: f64, (fixed_top, carry_top): (f64, f64), neck: f64, depth: Option<f64>, clearance: Option<f64>, p: &Profile, what: &str) -> Result<Chevron, String> {
        let top = fixed_top.min(carry_top);
        let h = top - ground;
        if h < 2.0 {
            return Err(format!("too low for a {what}: {h:.2} mm of both parts over the bed; make them 2 mm tall or more where they meet"));
        }
        let (c_xy, c_z) = (p.c_xy(), p.c_z());
        let gap_for = |d: f64| clearance.unwrap_or(c_xy.max(c_z * 2.0 * d / h));
        let mut d = depth.unwrap_or((0.3 * h).clamp(0.6, 2.5)).min(0.5 * h);
        if depth.is_none() {
            d = d.max(gap_for(d) + HOLD + 0.05).min(0.45 * h);
        }
        let gap = gap_for(d);
        if d - gap < HOLD {
            return Err(format!("the {what} cannot hold: its ridge reaches {d:.2} mm past a {gap:.2} mm gap, under {HOLD} mm more; make the parts taller there or give depth:"));
        }
        Ok(Chevron { ground, top, neck, depth: d, gap, c_xy, c_z, fixed_top, carry_top })
    }

    pub fn height(&self) -> f64 {
        self.top - self.ground
    }

    /// The flank's slope: across per height, at most 1 (45 degrees).
    pub fn slope(&self) -> f64 {
        2.0 * self.depth / self.height()
    }

    /// The smallest gap it is built with: across the flank, the slot and over the fixed part.
    pub fn clearance(&self) -> f64 {
        let cos = 1.0 / (1.0 + self.slope() * self.slope()).sqrt();
        (self.gap * cos).min(self.c_xy).min(self.c_z)
    }

    /// How far the ridge passes under the groove's lip.
    pub fn retention(&self) -> f64 {
        self.depth - self.gap
    }

    /// The moving part's cut over the fixed one reaches up to here.
    pub fn zone_top(&self) -> f64 {
        self.fixed_top + self.c_z
    }

    fn mid(&self) -> f64 {
        0.5 * (self.ground + self.top)
    }

    /// Where the 45 degree underside over the fixed part leaves the neck, and how high the moving
    /// part must reach to carry anything `out` from its axis over the fixed part with a wall over
    /// the underside there.
    fn carries(&self, out: f64, wall: f64) -> f64 {
        self.zone_top() + (out - self.neck).max(0.0) + wall
    }

    /// The floor of what the underside spares inside the neck (the neck stands there): over the
    /// middle, under the zone's top by half the neck, so no face of the cut runs along the neck's.
    fn spare_floor(&self) -> f64 {
        self.zone_top() - 0.5 * self.neck.min(self.zone_top() - self.mid())
    }

    /// The neck's top: inside the moving part, over where the underside leaves it.
    fn neck_top(&self) -> f64 {
        if self.carry_top > self.zone_top() + 0.1 {
            0.5 * (self.zone_top() + self.carry_top)
        } else {
            self.carry_top + 1.0
        }
    }
}

/// A slider's way.
#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    pub at: P3,
    pub axis: P3,
    /// Across the way, in the bed plane, and the print's up.
    pub side: P3,
    pub up: P3,
    pub chevron: Chevron,
    /// The rail across the way (from `at`, along `side`): where the carriage clears it.
    pub rail: (f64, f64),
    /// The carriage along the way (from `at`, along `axis`) as it prints.
    pub carriage: (f64, f64),
    /// How far it slides back and on from there.
    pub travel: (f64, f64),
}

/// A bearing's race.
#[derive(Clone, Debug, PartialEq)]
pub struct Race {
    /// A point on the turning axis, and the axis (the print's up).
    pub c: P3,
    pub up: P3,
    pub chevron: Chevron,
    /// How far from the axis the inner clears the outer below the outer's top.
    pub reach: f64,
}

/// A part's reach from `at` along the two directions across `up`, both ways: the nearest side and
/// the farthest corner.
fn across(b: &PartBox, at: P3, up: P3) -> (f64, f64) {
    let (u, v) = basis(up);
    let (eu, ev) = (b.extent(at, u), b.extent(at, v));
    let near = (-eu.lo).min(eu.hi).min(-ev.lo).min(ev.hi);
    let far = ((-eu.lo).max(eu.hi).powi(2) + (-ev.lo).max(ev.hi).powi(2)).sqrt();
    (near, far)
}

fn wall(p: &Profile) -> f64 {
    (WALL * p.nozzle).max(1.2)
}

/// Size a slider from the rail's (`a`) and the carriage's (`b`) bounds; `ground` is the bed.
pub fn size_slider(d: &crate::joint::SliderDecl, a: &PartBox, b: &PartBox, ground: f64, p: &Profile, _notes: &mut Vec<Note>) -> Result<Way, String> {
    let up = norm(p.up);
    let given = norm(d.axis);
    if dot(given, up).abs() > 10f64.to_radians().sin() {
        return Err("a slider runs along the bed: give an axis across the print's up".into());
    }
    let axis = norm(sub(given, scale(up, dot(given, up))));
    let side = norm(cross(up, axis));
    let (rail_up, car_up) = (a.extent([0.0; 3], up), b.extent([0.0; 3], up));
    if rail_up.lo > ground + 0.5 * p.layer {
        return Err(format!("the rail must stand on the bed, the carriage printing on the bed in it: its bottom is {:.2} mm above the bed", rail_up.lo - ground));
    }
    let (rs, cs) = (a.extent(d.at, side), b.extent(d.at, side));
    if !(rs.lo < 0.0 && rs.hi > 0.0) {
        return Err("`at` must lie inside the rail's width".into());
    }
    if !(cs.lo < 0.0 && cs.hi > 0.0) {
        return Err("the carriage must reach across `at`, over the way".into());
    }
    let room = (-rs.lo).min(rs.hi);
    let neck = d.width.map(|w| 0.5 * w).unwrap_or((0.25 * room).clamp(1.0, 5.0));
    let chevron = Chevron::size(ground, (rail_up.hi, car_up.hi), neck, d.depth, d.clearance, p, "slider")?;
    let cheek = room - (neck + chevron.depth + chevron.gap);
    if cheek < wall(p) {
        return Err(format!(
            "the rail is too narrow round the slider: it leaves walls of {cheek:.2} mm beside the groove, under {:.2}; widen the rail to {:.1} mm or more at `at`, or give a smaller width:",
            wall(p),
            2.0 * (neck + chevron.depth + chevron.gap + wall(p))
        ));
    }
    // what the carriage carries past the rail's sides hangs on what it carries over the rail
    let edge = |rail: f64, car: f64| if car > rail + chevron.c_xy { rail + chevron.c_xy } else { 0.0 };
    let out = edge(-rs.lo, -cs.lo).max(edge(rs.hi, cs.hi));
    if out > 0.0 && car_up.hi < chevron.carries(out, wall(p)) {
        return Err(format!(
            "the carriage would come apart over the rail: its underside there climbs at 45 degrees from its neck so it prints without support, and past the rail's sides it needs {:.2} mm over that; make the carriage {:.1} mm tall or more, or the rail narrower",
            wall(p),
            chevron.carries(out, wall(p)) - ground
        ));
    }
    let (re, ce) = (a.extent(d.at, axis), b.extent(d.at, axis));
    let end = wall(p).max(2.0);
    if ce.lo < re.lo + end - 1e-9 || ce.hi > re.hi - end + 1e-9 {
        return Err(format!("the carriage must sit inside the rail's length, {end:.1} mm or more from either end: the end stops"));
    }
    let travel = (d.travel.0.unwrap_or(re.lo + end - ce.lo), d.travel.1.unwrap_or(re.hi - end - ce.hi));
    if travel.0 > 0.0 || travel.1 < 0.0 {
        return Err("the travel runs from where it prints: back (lower) at most 0, on (upper) at least 0".into());
    }
    if ce.lo + travel.0 < re.lo + end - 1e-9 || ce.hi + travel.1 > re.hi - end + 1e-9 {
        return Err(format!("the carriage would slide past the rail's end stops: keep it {end:.1} mm from either end, or make the rail longer"));
    }
    if travel.1 - travel.0 < 1.0 {
        return Err(format!("no room to slide: {:.2} mm between the end stops; make the rail longer than the carriage", travel.1 - travel.0));
    }
    Ok(Way { at: d.at, axis, side, up, chevron, rail: (rs.lo, rs.hi), carriage: (ce.lo, ce.hi), travel })
}

/// Size a bearing from the outer's (`a`) and the inner's (`b`) bounds; `ground` is the bed.
pub fn size_bearing(d: &crate::joint::BearingDecl, a: &PartBox, b: &PartBox, ground: f64, p: &Profile, _notes: &mut Vec<Note>) -> Result<Race, String> {
    let up = norm(p.up);
    let (outer_up, inner_up) = (a.extent([0.0; 3], up), b.extent([0.0; 3], up));
    if outer_up.lo > ground + 0.5 * p.layer {
        return Err(format!("the outer race must stand on the bed, the inner printing on the bed in it: its bottom is {:.2} mm above the bed", outer_up.lo - ground));
    }
    let (outer_near, outer_far) = across(a, d.at, up);
    let (inner_near, _) = across(b, d.at, up);
    if inner_near <= 0.0 || outer_near <= 0.0 {
        return Err("`at` must lie inside both parts: the bearing turns about it".into());
    }
    let mut chevron = Chevron::size(ground, (outer_up.hi, inner_up.hi), 0.0, d.depth, d.clearance, p, "bearing")?;
    let r = d.radius.unwrap_or(inner_near - chevron.depth);
    if r < 2.0 {
        return Err(format!("the bearing's inner is too small: a race radius of {r:.2} mm; make the inner wider or give radius:"));
    }
    chevron.neck = r;
    let outer_wall = outer_near - (r + chevron.depth + chevron.gap);
    if outer_wall < wall(p) {
        return Err(format!(
            "the outer race is too thin round the bearing: {outer_wall:.2} mm of wall outside the groove, under {:.2}; make the outer {:.1} mm wide or more, or give a smaller radius:",
            wall(p),
            2.0 * (r + chevron.depth + chevron.gap + wall(p))
        ));
    }
    let reach = outer_far + chevron.c_xy;
    if across(b, d.at, up).1 > reach && inner_up.hi < chevron.carries(reach, wall(p)) {
        return Err(format!(
            "the inner would come apart over the outer: its underside there climbs at 45 degrees from its neck so it prints without support, and past the outer it needs {:.2} mm over that; make the inner {:.1} mm tall or more, or the outer smaller",
            wall(p),
            chevron.carries(reach, wall(p)) - ground
        ));
    }
    Ok(Race { c: d.at, up, chevron, reach })
}

impl Way {
    /// A point `t` along the way, `s` across it, at height `y` (along up, absolute).
    fn point(&self, t: f64, s: f64, y: f64) -> P3 {
        let p = madd(madd(self.at, self.axis, t), self.side, s);
        madd(p, self.up, y - dot(self.at, self.up))
    }

    /// The groove's run along the way (from `at`): the carriage over its whole travel.
    pub fn groove(&self) -> (f64, f64) {
        (self.carriage.0 + self.travel.0, self.carriage.1 + self.travel.1)
    }

    fn slab(&self, n: P3, lo: f64, hi: f64, t: (f64, f64), half: f64) -> Solid {
        let c = self.point(0.5 * (t.0 + t.1), 0.0, self.chevron.mid());
        Solid::Prim(Prim::Slab { n, lo, hi, c, half })
    }

    /// A box of the way: `t` along, `s` across, `y` up (absolute).
    fn block(&self, t: (f64, f64), s: (f64, f64), y: (f64, f64)) -> Solid {
        // each slab is a box round the way's middle: as wide as the farthest side from it
        let mid = self.chevron.mid();
        let half = (0.5 * (t.1 - t.0)).max(s.0.abs()).max(s.1.abs()).max((y.0 - mid).abs()).max((y.1 - mid).abs()) + 2.0;
        let (ta, sa) = (dot(self.at, self.axis), dot(self.at, self.side));
        Solid::Inter(vec![
            self.slab(self.axis, ta + t.0, ta + t.1, t, half),
            self.slab(self.side, sa + s.0, sa + s.1, t, half),
            self.slab(self.up, y.0, y.1, t, half),
        ])
    }

    /// The ridge's section (the carriage's edge moved out by `grow` across), run `t` along the
    /// way, from `y0` (absolute) to the ridge's top.
    fn ridge(&self, grow: f64, t: (f64, f64), y0: f64) -> Solid {
        let ch = &self.chevron;
        let k = ch.slope();
        let (cos, sin) = (1.0 / (1.0 + k * k).sqrt(), k / (1.0 + k * k).sqrt());
        let e = (ch.neck + ch.depth + grow) * cos;
        let mid = self.point(0.0, 0.0, ch.mid());
        let half = 0.5 * (t.1 - t.0) + ch.height() + ch.neck + ch.depth + grow + 2.0;
        let flank = |n: P3| {
            let k = dot(n, mid);
            self.slab(n, k - e, k + e, t, half)
        };
        let ta = dot(self.at, self.axis);
        Solid::Inter(vec![
            flank(add(scale(self.side, cos), scale(self.up, sin))),
            flank(sub(scale(self.side, cos), scale(self.up, sin))),
            self.slab(self.up, y0, ch.top, t, half),
            self.slab(self.axis, ta + t.0, ta + t.1, t, half),
        ])
    }

    /// The rail with its groove and slot cut along the travel.
    pub fn apply_a(&self, a: Solid) -> Solid {
        let ch = &self.chevron;
        let g = self.groove();
        // the slot comes down inside the groove to its middle: its sides cross the upper flanks at
        // the groove's top, never along it
        Solid::diff(a, vec![self.ridge(ch.gap, g, ch.ground - 1.0), self.block(g, (-(ch.neck + ch.c_xy), ch.neck + ch.c_xy), (ch.mid(), ch.fixed_top + 1.0))])
    }

    /// The carriage cleared of the rail, its ridge standing on the bed and its neck up to what it
    /// carries.
    pub fn apply_b(&self, b: Solid) -> Solid {
        let ch = &self.chevron;
        let c = self.carriage;
        let (t, s) = ((c.0 - 1.0, c.1 + 1.0), (self.rail.0 - ch.c_xy, self.rail.1 + ch.c_xy));
        let y1 = ch.zone_top() + (s.0.abs().max(s.1.abs()) - ch.neck).max(0.0) + 1.0;
        let zone = Solid::diff(self.block(t, s, (ch.ground - 1.0, y1)), vec![self.spared(t, s, y1)]);
        // the neck comes down inside the ridge to its middle (as the slot does)
        let neck = self.block(c, (-ch.neck, ch.neck), (ch.mid(), ch.neck_top()));
        Solid::Union(vec![Solid::diff(b.clone(), vec![zone]), self.ridge(0.0, c, ch.ground), Solid::Inter(vec![b, neck])])
    }

    /// What the carriage keeps over the rail, in the zone `t` along and `s` across up to `y1`:
    /// over its underside, which climbs at 45 degrees from the neck's sides at the zone's top
    /// (under them it runs on down inside the neck to [`Chevron::spare_floor`]).
    fn spared(&self, t: (f64, f64), s: (f64, f64), y1: f64) -> Solid {
        let ch = &self.chevron;
        let r = std::f64::consts::FRAC_1_SQRT_2;
        let k = ch.zone_top() - ch.neck;
        let sa = dot(self.at, self.side);
        let half = (0.5 * (t.1 - t.0)).max(s.0.abs()).max(s.1.abs()).max(y1 - ch.ground) + 4.0;
        // y - s >= k and y + s >= k, across the way from `at`
        let (n1, n2) = (scale(sub(self.up, self.side), r), scale(add(self.up, self.side), r));
        Solid::Inter(vec![
            self.slab(n1, r * (k - sa), r * (k - sa) + 2.0 * half, t, half),
            self.slab(n2, r * (k + sa), r * (k + sa) + 2.0 * half, t, half),
            self.slab(self.up, ch.spare_floor(), y1 + 2.0, t, half),
        ])
    }

    /// The carriage's middle, halfway up the ridge, and the way.
    pub fn pivot(&self) -> (P3, P3) {
        (self.point(0.5 * (self.carriage.0 + self.carriage.1), 0.0, self.chevron.mid()), self.axis)
    }

    /// Balls along the groove holding where the carriage meets the rail.
    pub fn keep_out(&self) -> Vec<(P3, f64)> {
        let ch = &self.chevron;
        let r = (ch.neck + ch.depth + ch.gap).hypot(0.5 * ch.height().max(ch.zone_top() - ch.ground)) + 1.0;
        let g = self.groove();
        let n = ((g.1 - g.0) / r).ceil().max(1.0) as usize;
        (0..=n).map(|k| (self.point(g.0 + (g.1 - g.0) * k as f64 / n as f64, 0.0, ch.mid()), r)).collect()
    }

    /// The box the checks search: the groove's run, across the rail, up over the rail's top.
    pub fn region(&self) -> (P3, P3) {
        let ch = &self.chevron;
        let g = self.groove();
        let w = ch.neck + ch.depth + ch.gap + 1.5;
        let (s0, s1) = ((self.rail.0 - 1.5).min(-w), (self.rail.1 + 1.5).max(w));
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for k in 0..8 {
            let q = self.point(if k & 1 == 0 { g.0 - 0.5 } else { g.1 + 0.5 }, if k & 2 == 0 { s0 } else { s1 }, if k & 4 == 0 { ch.ground - 0.5 } else { ch.zone_top() + 1.0 });
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        (lo, hi)
    }
}

impl Race {
    /// The point on the axis at height `y` (along up, absolute).
    fn point(&self, y: f64) -> P3 {
        madd(self.c, self.up, y - dot(self.c, self.up))
    }

    fn cyl(&self, r: f64, y0: f64, y1: f64) -> Solid {
        Solid::Prim(Prim::cyl(self.point(y0), self.point(y1), r))
    }

    /// The inner's edge moved out by `grow`, from `y0` (absolute; under the bed it keeps its slope)
    /// to the ridge's top.
    fn ridge(&self, grow: f64, y0: f64) -> Solid {
        let ch = &self.chevron;
        let (r, k) = (ch.neck + grow, ch.slope());
        // the cone widening up from the bed and the one narrowing to the top, each run past the
        // other's end: they cross at the ridge's crest, and no face of one lies on the other's
        let widening = |y: f64| r + k * (y - ch.ground);
        let narrowing = |y: f64| r + k * (ch.top - y);
        Solid::Inter(vec![
            Solid::Prim(Prim::Frustum { a: self.point(y0), b: self.point(ch.top + 1.0), ra: widening(y0), rb: widening(ch.top + 1.0) }),
            Solid::Prim(Prim::Frustum { a: self.point(y0 - 1.0), b: self.point(ch.top), ra: narrowing(y0 - 1.0), rb: narrowing(ch.top) }),
        ])
    }

    /// The outer with its groove and the hole the inner's neck comes up through.
    pub fn apply_a(&self, a: Solid) -> Solid {
        let ch = &self.chevron;
        Solid::diff(a, vec![self.ridge(ch.gap, ch.ground - 1.0), self.cyl(ch.neck + ch.c_xy, ch.mid(), ch.fixed_top + 1.0)])
    }

    /// The inner cleared of the outer, its ridge standing on the bed and its neck up to what it
    /// carries.
    pub fn apply_b(&self, b: Solid) -> Solid {
        let ch = &self.chevron;
        let y1 = ch.zone_top() + (self.reach - ch.neck).max(0.0) + 1.0;
        // what it keeps over the outer: over a cone climbing at 45 degrees from the neck at the
        // zone's top (under it the cone runs on down inside the neck)
        let f = ch.spare_floor();
        let spared = Solid::Prim(Prim::Frustum { a: self.point(f), b: self.point(y1 + 1.0), ra: ch.neck - (ch.zone_top() - f), rb: ch.neck + (y1 + 1.0 - ch.zone_top()) });
        let zone = Solid::diff(self.cyl(self.reach, ch.ground - 1.0, y1), vec![spared]);
        let neck = self.cyl(ch.neck, ch.mid(), ch.neck_top());
        Solid::Union(vec![Solid::diff(b.clone(), vec![zone]), self.ridge(0.0, ch.ground), Solid::Inter(vec![b, neck])])
    }

    pub fn pivot(&self) -> (P3, P3) {
        (self.point(self.chevron.mid()), self.up)
    }

    pub fn keep_out(&self) -> Vec<(P3, f64)> {
        let ch = &self.chevron;
        vec![(self.point(ch.mid()), (ch.neck + ch.depth + ch.gap).hypot(0.5 * (ch.zone_top() - ch.ground)) + 1.0)]
    }

    pub fn region(&self) -> (P3, P3) {
        let ch = &self.chevron;
        let r = self.reach + 0.5;
        let (u, v) = basis(self.up);
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for k in 0..8 {
            let y = if k & 4 == 0 { ch.ground - 0.5 } else { ch.zone_top() + 1.0 };
            let q = madd(madd(self.point(y), u, if k & 1 == 0 { -r } else { r }), v, if k & 2 == 0 { -r } else { r });
            for i in 0..3 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        (lo, hi)
    }
}
