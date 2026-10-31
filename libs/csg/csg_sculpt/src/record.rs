//! Turning brush input (mouse, tablet pen, XR controller or hand) into a
//! stroke's compact path.
//!
//! Samples arrive at input rate (pen ~200 Hz, headset 72-120 Hz). The
//! recorder quantises each one to what the program text will hold (2
//! decimals of a millimetre and of pressure, read back through f64 like the
//! language's lowering does) and keeps a point only when the chord from the
//! last kept point stops describing the samples in between: deviation over
//! `tol` (a fraction of the radius), a pressure change, or a chord longer
//! than `max_chord` radii. The newest sample is the provisional tip. So the
//! live stroke and the committed `Stroke{..}` are the same numbers, and the
//! live bake equals the committed bake bit for bit.

use crate::math::*;

/// Rounds like the program text does: `{:.decimals}` read back as f64, then
/// f32 (the lowering's path).
pub fn quantise(x: f32, decimals: usize) -> f32 {
    let s = format!("{:.*}", decimals, x as f64);
    s.parse::<f64>().unwrap() as f32
}

pub struct Recorder {
    pub radius: f32,
    /// chord deviation allowed, as a fraction of the radius
    pub tol: f32,
    pub pressure_tol: f32,
    /// longest chord, in radii
    pub max_chord: f32,
    kept: Vec<[f32; 4]>,
    /// samples since the last kept point (the last one is the tip)
    pending: Vec<[f32; 4]>,
}

impl Recorder {
    pub fn new(radius: f32) -> Recorder {
        Recorder { radius, tol: 0.05, pressure_tol: 0.08, max_chord: 1.0, kept: Vec::new(), pending: Vec::new() }
    }

    fn fits(&self, a: [f32; 4], b: [f32; 4], mid: &[[f32; 4]]) -> bool {
        let (pa, pb) = ([a[0], a[1], a[2]], [b[0], b[1], b[2]]);
        let ab = sub(pb, pa);
        let l2 = dot(ab, ab);
        if l2.sqrt() > self.max_chord * self.radius {
            return false;
        }
        for m in mid {
            let pm = [m[0], m[1], m[2]];
            let t = if l2 > 0.0 { (dot(sub(pm, pa), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let c = add(pa, scale(ab, t));
            if len(sub(pm, c)) > self.tol * self.radius {
                return false;
            }
            if (m[3] - mix(a[3], b[3], t)).abs() > self.pressure_tol {
                return false;
            }
        }
        true
    }

    /// One input sample (layer-frame position, pressure 0..1; 1 for a mouse).
    pub fn push(&mut self, p: V3, pressure: f32) {
        let s = [quantise(p[0], 2), quantise(p[1], 2), quantise(p[2], 2), quantise(pressure.clamp(0.0, 1.0), 2)];
        if self.kept.is_empty() {
            self.kept.push(s);
            return;
        }
        if self.pending.last() == Some(&s) || (self.pending.is_empty() && *self.kept.last().unwrap() == s) {
            return;
        }
        let a = *self.kept.last().unwrap();
        if self.fits(a, s, &self.pending) {
            self.pending.push(s);
            return;
        }
        // the previous tip becomes a kept point; restart from it
        if let Some(t) = self.pending.pop() {
            self.kept.push(t);
        }
        self.pending.clear();
        self.pending.push(s);
    }

    /// The path now: kept points and the provisional tip.
    pub fn path(&self) -> Vec<[f32; 4]> {
        let mut p = self.kept.clone();
        if let Some(t) = self.pending.last() {
            p.push(*t);
        }
        p
    }
}
