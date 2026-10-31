//! A note's motion, tick by tick (its pitch, level and pan as a tracker or
//! a sound chip played them), as the few breakpoints a Score note keeps
//! ([`NoteFx`]): the line through the ticks simplified until it fits,
//! within a tolerance that is doubled until it does.

use crate::arrangement::{NoteFx, FX_POINTS};

/// One tick of a sounding note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// Beats from the note's start.
    pub beat: f32,
    /// A MIDI number, fractional (vibrato and arpeggio apart).
    pub pitch: f32,
    pub level: f32,
    pub pan: f32,
}

/// The points of `pts` (x, y) that keep the line within `tol` of every
/// point (Ramer-Douglas-Peucker), as indices.
fn simplify(pts: &[(f32, f32)], tol: f32) -> Vec<usize> {
    if pts.len() <= 2 {
        return (0..pts.len()).collect();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (x0, y0) = pts[a];
        let (x1, y1) = pts[b];
        let mut worst = (0.0f32, a);
        for (i, &(x, y)) in pts.iter().enumerate().take(b).skip(a + 1) {
            let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
            let d = (y - (y0 + (y1 - y0) * t)).abs();
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tol {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    (0..pts.len()).filter(|&i| keep[i]).collect()
}

/// `pts` as at most `max` breakpoints, the tolerance starting at `tol`;
/// none when the line is flat at `flat` within `tol`.
pub fn breakpoints(pts: &[(f32, f32)], tol: f32, flat: f32, max: usize) -> Vec<(f32, f32)> {
    if pts.iter().all(|p| (p.1 - flat).abs() <= tol) {
        return Vec::new();
    }
    // Steps (a tracker's ticks) are kept as steps: a point just before
    // each jump keeps the line flat up to it.
    let mut tol = tol;
    loop {
        let idx = simplify(pts, tol);
        if idx.len() <= max {
            return idx.into_iter().map(|i| pts[i]).collect();
        }
        tol *= 2.0;
    }
}

/// A note's motion as a [`NoteFx`] and the velocity it is struck at:
/// the bend from `key`, the level relative to its loudest, the pan from
/// `part_pan`.
pub fn shape(key: f32, points: &[Point], part_pan: f32) -> (f64, NoteFx) {
    let mut fx = NoteFx::default();
    let vel = points.iter().map(|p| p.level).fold(0.0f32, f32::max).max(1e-4);
    let bend: Vec<(f32, f32)> = points.iter().map(|p| (p.beat, p.pitch - key)).collect();
    for (i, &(b, v)) in breakpoints(&bend, 0.04, 0.0, FX_POINTS).iter().enumerate() {
        fx.bend[i] = (b, (v * 1000.0).round() / 1000.0);
        fx.bend_n = i as u8 + 1;
    }
    let level: Vec<(f32, f32)> = points.iter().map(|p| (p.beat, p.level / vel)).collect();
    for (i, &(b, v)) in breakpoints(&level, 0.03, 1.0, FX_POINTS).iter().enumerate() {
        fx.level[i] = (b, (v * 1000.0).round() / 1000.0);
        fx.level_n = i as u8 + 1;
    }
    if let Some(p) = points.first() {
        let d = p.pan - part_pan;
        if d.abs() > 0.05 {
            fx.pan = (d * 100.0).round() / 100.0;
        }
    }
    (((vel as f64) * 100.0).round() / 100.0, fx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_keep_their_corners_and_fit() {
        let pts: Vec<(f32, f32)> = (0..40).map(|i| (i as f32 * 0.04, if i < 20 { 0.0 } else { (i - 20) as f32 * 0.1 })).collect();
        let bp = breakpoints(&pts, 0.04, 0.0, 8);
        assert!(bp.len() <= 3, "{bp:?}");
        assert!(bp.iter().any(|p| (p.0 - 19.0 * 0.04).abs() < 0.05));
        assert!(breakpoints(&[(0.0, 1.0), (1.0, 1.01)], 0.03, 1.0, 8).is_empty());
        // A wobble too fine for eight points is simplified until it fits.
        let wobble: Vec<(f32, f32)> = (0..200).map(|i| (i as f32, (i as f32 * 0.7).sin())).collect();
        assert!(breakpoints(&wobble, 0.01, 0.0, 8).len() <= 8);
        let (vel, fx) = shape(60.0, &[Point { beat: 0.0, pitch: 60.0, level: 0.5, pan: 0.0 }, Point { beat: 1.0, pitch: 62.0, level: 0.25, pan: 0.0 }], 0.0);
        assert_eq!(vel, 0.5);
        assert_eq!(fx.bend(), &[(0.0, 0.0), (1.0, 2.0)]);
        assert_eq!(fx.level(), &[(0.0, 1.0), (1.0, 0.5)]);
    }
}
