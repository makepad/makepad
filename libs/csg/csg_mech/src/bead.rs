//! The as-printed field of one part near a joint: the slicer's beads (centre
//! line segments of a layer, bead width, layer height) as a distance field,
//! so the joint checks and the limit sweep run on what the printer lays down
//! rather than on the design.
//!
//! Printer frame (Z up), millimetres. A bead is a flat-ended stadium slab:
//! `max(dist_xy(segment) - w/2, |z - zc| - h/2)`, a 1-Lipschitz bound of its
//! true distance. The field searches the beads in the 3 x 3 grid cells and 3
//! layers round a point and returns the smaller of the nearest bead and the
//! distance to the edge of what it searched, so it never overestimates.

use crate::geom::*;
use makepad_csg_math::Vec3d;
use makepad_csg_sdf::Sdf3;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bead {
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Bottom and top of the bead.
    pub z0: f64,
    pub z1: f64,
    pub width: f64,
}

pub struct BeadField {
    beads: Vec<Bead>,
    cell: f64,
    bed: f64,
    layer: f64,
    /// (layer, cx, cy) -> beads
    grid: HashMap<(i64, i64, i64), Vec<u32>>,
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ax, ay) = (b[0] - a[0], b[1] - a[1]);
    let (px, py) = (p[0] - a[0], p[1] - a[1]);
    let l2 = ax * ax + ay * ay;
    let t = if l2 > 0.0 { ((px * ax + py * ay) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (px - ax * t, py - ay * t);
    (dx * dx + dy * dy).sqrt()
}

impl BeadField {
    /// `layer`: the layer height the beads sit on (from `bed`); `cell`: in-plane grid cell (>= the widest bead).
    pub fn new(beads: Vec<Bead>, bed: f64, layer: f64, cell: f64) -> BeadField {
        let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        for (i, b) in beads.iter().enumerate() {
            let k0 = ((b.z0 - bed) / layer).floor() as i64;
            let k1 = (((b.z1 - bed) / layer).ceil() as i64 - 1).max(k0);
            let r = 0.5 * b.width;
            let x0 = ((b.a[0].min(b.b[0]) - r) / cell).floor() as i64;
            let x1 = ((b.a[0].max(b.b[0]) + r) / cell).floor() as i64;
            let y0 = ((b.a[1].min(b.b[1]) - r) / cell).floor() as i64;
            let y1 = ((b.a[1].max(b.b[1]) + r) / cell).floor() as i64;
            for k in k0..=k1 {
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        grid.entry((k, x, y)).or_default().push(i as u32);
                    }
                }
            }
        }
        BeadField { beads, cell, bed, layer, grid }
    }

    pub fn beads(&self) -> &[Bead] {
        &self.beads
    }

    fn bead_distance(b: &Bead, p: P3) -> f64 {
        let dxy = seg_dist([p[0], p[1]], b.a, b.b) - 0.5 * b.width;
        let zc = 0.5 * (b.z0 + b.z1);
        dxy.max((p[2] - zc).abs() - 0.5 * (b.z1 - b.z0))
    }
}

impl Sdf3 for BeadField {
    fn distance(&self, q: Vec3d) -> f64 {
        let p = [q.x, q.y, q.z];
        let k = ((p[2] - self.bed) / self.layer).floor() as i64;
        let cx = (p[0] / self.cell).floor() as i64;
        let cy = (p[1] / self.cell).floor() as i64;
        let mut best = f64::INFINITY;
        for dk in -1..=1 {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(l) = self.grid.get(&(k + dk, cx + dx, cy + dy)) {
                        for &i in l {
                            best = best.min(Self::bead_distance(&self.beads[i as usize], p));
                        }
                    }
                }
            }
        }
        // anything not searched is at least this far
        let fx = p[0] / self.cell - cx as f64;
        let fy = p[1] / self.cell - cy as f64;
        let fz = (p[2] - self.bed) / self.layer - k as f64;
        let margin = ((fx.min(1.0 - fx) + 1.0) * self.cell).min((fy.min(1.0 - fy) + 1.0) * self.cell).min((fz.min(1.0 - fz) + 1.0) * self.layer);
        best.min(margin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bead_walls_gap() {
        // two parallel walls 0.35 apart between bead edges, 5 layers
        let mut a = Vec::new();
        let mut b = Vec::new();
        for k in 0..5 {
            let z0 = k as f64 * 0.2;
            a.push(Bead { a: [0.0, 0.0], b: [10.0, 0.0], z0, z1: z0 + 0.2, width: 0.42 });
            b.push(Bead { a: [0.0, 0.77], b: [10.0, 0.77], z0, z1: z0 + 0.2, width: 0.42 });
        }
        let fa = BeadField::new(a, 0.0, 0.2, 1.0);
        let fb = BeadField::new(b, 0.0, 0.2, 1.0);
        let g = crate::check::min_gap(&fa, &fb, [-1.0, -1.0, 0.0], [11.0, 2.0, 1.0], 0.005);
        assert!((g.gap - 0.35).abs() < 0.015, "{g:?}");
    }
}
