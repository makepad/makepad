//! Mechanical parts: involute spur gears, ISO metric threads and hole tools
//! (simple, counterbore, countersink).

use crate::region::{Loop, Region, P2};
use crate::sketch::{circle_loop, Tolerance};
use crate::solids::{orient_outward, revolve};
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::mesh::TriMesh;
use std::f64::consts::{PI, TAU};

/// An involute spur gear (standard full-depth teeth).
#[derive(Clone, Debug)]
pub struct Gear {
    /// Module: pitch diameter / teeth (mm).
    pub module: f64,
    pub teeth: usize,
    /// Pressure angle, degrees (20 is standard).
    pub pressure_deg: f64,
    /// Circular backlash of this gear on the pitch circle (mm): each tooth is
    /// this much thinner than half the pitch.
    pub backlash: f64,
    /// Profile shift coefficient (0: none).
    pub shift: f64,
    /// Bore diameter (0: solid).
    pub bore: f64,
}

impl Gear {
    pub fn pitch_radius(&self) -> f64 {
        self.module * self.teeth as f64 / 2.0
    }
    pub fn outer_radius(&self) -> f64 {
        self.pitch_radius() + self.module * (1.0 + self.shift)
    }
    pub fn root_radius(&self) -> f64 {
        self.pitch_radius() - self.module * (1.25 - self.shift)
    }

    /// Centre distance to a meshing gear of the same module.
    pub fn center_distance(&self, other: &Gear) -> f64 {
        self.module * (self.teeth + other.teeth) as f64 / 2.0 + self.module * (self.shift + other.shift)
    }

    pub fn check(&self) -> Result<(), String> {
        if !(self.module > 0.0) {
            return Err("a gear needs a positive module".into());
        }
        if self.teeth < 6 || self.teeth > 400 {
            return Err("a gear needs 6 to 400 teeth".into());
        }
        if !(self.pressure_deg >= 10.0 && self.pressure_deg <= 35.0) {
            return Err("the pressure angle must be 10 to 35 degrees".into());
        }
        if self.backlash < 0.0 || self.backlash > self.module {
            return Err("the backlash must be between 0 and one module".into());
        }
        if self.bore > 0.0 && self.bore / 2.0 >= self.root_radius() - self.module * 0.5 {
            return Err(format!("a {} mm bore leaves no rim under the teeth (root diameter {:.2} mm)", self.bore, 2.0 * self.root_radius()));
        }
        Ok(())
    }

    /// The gear's outline (and its bore) as a region in the XY plane, centred
    /// on the origin, a tooth on +X.
    pub fn region(&self, tol: &Tolerance) -> Result<Region, String> {
        self.check()?;
        let alpha = self.pressure_deg.to_radians();
        let rp = self.pitch_radius();
        let rb = rp * alpha.cos();
        let ra = self.outer_radius();
        let rf = self.root_radius().max(0.1);
        let inv = |phi: f64| phi.tan() - phi;
        // Half the tooth's angular thickness at the pitch circle.
        let s = PI * self.module / 2.0 + 2.0 * self.shift * self.module * alpha.tan() - self.backlash;
        let psi_p = s / (2.0 * rp);
        let half = |r: f64| -> f64 {
            let r = r.max(rb);
            psi_p + inv(alpha) - inv((rb / r).clamp(-1.0, 1.0).acos())
        };
        let z = self.teeth;
        let pitch_angle = TAU / z as f64;
        // Flank samples from the root (or the base circle) to the tip.
        let start = rf.max(rb);
        let flank_steps = ((ra - start) / (tol.chord * 10.0).max(0.02)).ceil().clamp(6.0, 40.0) as usize;
        let mut flank: Vec<(f64, f64)> = Vec::new(); // (radius, half-angle)
        if rf < rb {
            flank.push((rf, half(rb)));
        }
        for i in 0..=flank_steps {
            let r = start + (ra - start) * i as f64 / flank_steps as f64;
            let h = half(r);
            if h <= 0.0 {
                // The tooth comes to a point before the tip circle.
                break;
            }
            flank.push((r, h));
        }
        if flank.len() < 2 {
            return Err("the teeth come to a point: fewer teeth, less backlash or less shift".into());
        }
        let (r_tip, h_tip) = *flank.last().unwrap();
        let (_, h_root) = flank[0];
        if h_root >= pitch_angle / 2.0 {
            return Err("the teeth overlap at the root: more teeth or a smaller pressure angle".into());
        }
        let polar = |r: f64, a: f64| -> P2 { [r * a.cos(), r * a.sin()] };
        let mut outline: Loop = Vec::new();
        for k in 0..z {
            let c = pitch_angle * k as f64;
            // Up the clockwise flank.
            for &(r, h) in &flank {
                outline.push(polar(r, c - h));
            }
            // Across the tip.
            let tip_steps = tol.segments(r_tip, 2.0 * h_tip).max(1);
            for i in 1..tip_steps {
                outline.push(polar(r_tip, c - h_tip + 2.0 * h_tip * i as f64 / tip_steps as f64));
            }
            // Down the counter-clockwise flank.
            for &(r, h) in flank.iter().rev() {
                outline.push(polar(r, c + h));
            }
            // Along the root to the next tooth.
            let gap = pitch_angle - 2.0 * h_root;
            let root_steps = tol.segments(rf, gap).max(1);
            for i in 1..root_steps {
                outline.push(polar(rf, c + h_root + gap * i as f64 / root_steps as f64));
            }
        }
        let holes = if self.bore > 0.0 {
            let mut h = circle_loop([0.0, 0.0], self.bore / 2.0, tol);
            h.reverse();
            vec![h]
        } else {
            Vec::new()
        };
        Ok(Region { outer: crate::region::clean_loop(outline), holes })
    }
}

/// An ISO metric (60 degree) thread.
#[derive(Clone, Debug)]
pub struct Thread {
    /// Major diameter (mm), e.g. 3 for M3.
    pub diameter: f64,
    pub pitch: f64,
    pub length: f64,
    /// Added to every radius (an internal thread's tool: positive, for printing clearance).
    pub clearance: f64,
    /// Left-handed.
    pub left: bool,
}

/// The coarse ISO pitch for a major diameter, when standard.
pub fn iso_coarse_pitch(diameter: f64) -> Option<f64> {
    const TABLE: &[(f64, f64)] = &[
        (1.0, 0.25), (1.2, 0.25), (1.6, 0.35), (2.0, 0.4), (2.5, 0.45), (3.0, 0.5), (3.5, 0.6), (4.0, 0.7), (5.0, 0.8), (6.0, 1.0),
        (8.0, 1.25), (10.0, 1.5), (12.0, 1.75), (14.0, 2.0), (16.0, 2.0), (18.0, 2.5), (20.0, 2.5), (24.0, 3.0), (30.0, 3.5), (36.0, 4.0),
    ];
    TABLE.iter().find(|(d, _)| (d - diameter).abs() < 1e-6).map(|(_, p)| *p)
}

impl Thread {
    pub fn check(&self) -> Result<(), String> {
        if !(self.diameter > 0.5 && self.pitch > 0.05 && self.length > 0.0) {
            return Err("a thread needs a diameter over 0.5 mm, a pitch and a length".into());
        }
        if self.pitch > self.diameter / 2.0 {
            return Err("the pitch is too coarse for the diameter".into());
        }
        Ok(())
    }

    /// Thread depth (major to minor radius): 5/8 of the fundamental triangle.
    pub fn depth(&self) -> f64 {
        0.625 * 0.866_025_403_784 * self.pitch
    }

    /// The radius at axial phase `u` (0..1 of a pitch): crest P/8, flank,
    /// root P/4, flank.
    fn radius_at(&self, u: f64) -> f64 {
        let r_major = self.diameter / 2.0 + self.clearance;
        let d = self.depth();
        let u = u.rem_euclid(1.0);
        let (crest, flank, root) = (1.0 / 8.0, 5.0 / 16.0, 1.0 / 4.0);
        if u < crest {
            r_major
        } else if u < crest + flank {
            r_major - d * (u - crest) / flank
        } else if u < crest + flank + root {
            r_major - d
        } else {
            r_major - d + d * (u - crest - flank - root) / flank
        }
    }

    /// A threaded rod along +Z from 0 to `length`: the helical surface over a
    /// little more than the length, closed by caps, then cut to length.
    /// Subtract it (with a clearance) to make a threaded hole.
    pub fn solid(&self, tol: &Tolerance) -> Result<TriMesh, String> {
        self.check()?;
        let p = self.pitch;
        let r_major = self.diameter / 2.0 + self.clearance;
        let around = tol.segments(r_major, TAU).clamp(24, 180);
        // Axial stations per pitch: the profile's corners exactly.
        let corners = [0.0, 1.0 / 8.0, 1.0 / 8.0 + 5.0 / 16.0, 1.0 / 8.0 + 5.0 / 16.0 + 1.0 / 4.0];
        let s0 = -2.0 * p;
        let s1 = self.length + 2.0 * p;
        let mut stations: Vec<f64> = Vec::new();
        let mut k = (s0 / p).floor() as i64;
        while (k as f64) * p <= s1 {
            for c in corners {
                let s = (k as f64 + c) * p;
                if s >= s0 && s <= s1 {
                    stations.push(s);
                }
            }
            k += 1;
        }
        let hand = if self.left { -1.0 } else { 1.0 };
        let mut m = TriMesh::new();
        // Vertex (i, j): angle i, helical station j; z = s + p * angle / TAU.
        let mut ids = vec![vec![0u32; around]; stations.len()];
        for (j, &s) in stations.iter().enumerate() {
            let r = self.radius_at(s / p);
            for i in 0..around {
                let a = TAU * i as f64 / around as f64;
                let z = s + p * (i as f64 / around as f64);
                ids[j][i] = m.add_vertex(dvec3(r * (hand * a).cos(), r * (hand * a).sin(), z));
            }
        }
        // Rows wrap: (i = around) is (i = 0) one pitch up, which is station j + 4.
        let per_pitch = corners.len();
        for j in 0..stations.len() - 1 {
            for i in 0..around {
                let (a0, b0) = (ids[j][i], ids[j + 1][i]);
                let (a1, b1) = if i + 1 < around {
                    (ids[j][i + 1], ids[j + 1][i + 1])
                } else if j + per_pitch + 1 < stations.len() {
                    (ids[j + per_pitch][0], ids[j + per_pitch + 1][0])
                } else {
                    continue;
                };
                m.add_triangle(a0, a1, b1);
                m.add_triangle(a0, b1, b0);
            }
        }
        // The open ends: fan each helical edge (the first and last pitch of
        // stations along the seam) to a point on the axis.
        let bottom = m.add_vertex(dvec3(0.0, 0.0, stations[0]));
        let top = m.add_vertex(dvec3(0.0, 0.0, *stations.last().unwrap() + p));
        let ring = |j: usize, i: usize| ids[j][i];
        let last = stations.len() - 1;
        // Bottom boundary: row 0 (all i), then the seam up rows 0..per_pitch at i = 0.
        // Bottom boundary: row 0, then from one pitch up back down the seam at i = 0.
        let mut b: Vec<u32> = (0..around).map(|i| ring(0, i)).collect();
        for j in (1..=per_pitch.min(last)).rev() {
            b.push(ring(j, 0));
        }
        for w in b.windows(2) {
            m.add_triangle(bottom, w[1], w[0]);
        }
        m.add_triangle(bottom, b[0], *b.last().unwrap());
        // Top boundary: the last row, then down the seam at i = around - 1.
        let mut t: Vec<u32> = (0..around).map(|i| ring(last, i)).collect();
        for j in (last.saturating_sub(per_pitch)..last).rev() {
            t.push(ring(j, around - 1));
        }
        for w in t.windows(2) {
            m.add_triangle(top, w[0], w[1]);
        }
        m.add_triangle(top, *t.last().unwrap(), t[0]);
        orient_outward(&mut m);
        // Cut to length: flat ends at z = 0 and z = length.
        let slab_region = Region { outer: circle_loop([0.0, 0.0], r_major + self.pitch, tol), holes: Vec::new() };
        let slab = crate::solids::extrude(&[slab_region], self.length, 0.0, false)?;
        let cut = crate::bool3::intersection(&m, &slab);
        if cut.triangles.is_empty() {
            return Err("the thread could not be cut to length".into());
        }
        Ok(cut)
    }
}

/// A hole tool: the solid to subtract, its axis along -Z from the surface at
/// z = 0 (it rises `above` over the surface so the cut is clean).
#[derive(Clone, Debug)]
pub enum Hole {
    Simple { diameter: f64, depth: f64 },
    Counterbore { diameter: f64, depth: f64, bore_diameter: f64, bore_depth: f64 },
    Countersink { diameter: f64, depth: f64, sink_diameter: f64, angle_deg: f64 },
}

pub fn hole_tool(hole: &Hole, above: f64, tol: &Tolerance) -> Result<TriMesh, String> {
    // A profile in (radius, height) revolved about the axis; height is -Z.
    let profile: Vec<P2> = match *hole {
        Hole::Simple { diameter, depth } => {
            if !(diameter > 0.0 && depth > 0.0) {
                return Err("a hole needs a diameter and a depth".into());
            }
            let r = diameter / 2.0;
            vec![[0.0, -depth], [r, -depth], [r, above], [0.0, above]]
        }
        Hole::Counterbore { diameter, depth, bore_diameter, bore_depth } => {
            if !(diameter > 0.0 && depth > 0.0 && bore_diameter > diameter && bore_depth > 0.0 && bore_depth < depth) {
                return Err("a counterbore needs bore_diameter > diameter and 0 < bore_depth < depth".into());
            }
            let (r, rb) = (diameter / 2.0, bore_diameter / 2.0);
            vec![[0.0, -depth], [r, -depth], [r, -bore_depth], [rb, -bore_depth], [rb, above], [0.0, above]]
        }
        Hole::Countersink { diameter, depth, sink_diameter, angle_deg } => {
            if !(diameter > 0.0 && depth > 0.0 && sink_diameter > diameter && angle_deg > 30.0 && angle_deg < 150.0) {
                return Err("a countersink needs sink_diameter > diameter and an angle of 30 to 150 degrees".into());
            }
            let (r, rs) = (diameter / 2.0, sink_diameter / 2.0);
            let sink = (rs - r) / (angle_deg.to_radians() / 2.0).tan();
            if sink >= depth {
                return Err("the countersink is deeper than the hole".into());
            }
            vec![[0.0, -depth], [r, -depth], [r, -sink], [rs, 0.0], [rs, above], [0.0, above]]
        }
    };
    // revolve() turns about Y; we want Z: build in XY (y = height), then swap.
    let region = Region { outer: profile, holes: Vec::new() };
    let region = Region { outer: crate::region::oriented(region.outer, true), holes: Vec::new() };
    let mut m = revolve(&[region], 360.0, tol)?;
    for v in &mut m.vertices {
        *v = dvec3(v.x, -v.z, v.y);
    }
    orient_outward(&mut m);
    Ok(m)
}

/// A point on the axis, for tools placed by callers.
pub fn axis_point(z: f64) -> Vec3d {
    dvec3(0.0, 0.0, z)
}
