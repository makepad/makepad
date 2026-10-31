//! Curves at constant arc length: a kit's `curve_fn` gives points along a
//! path (any parameterisation); [`resample`] spaces them evenly by arc
//! length and frames them by parallel transport (no twist, no flips at
//! inflections), so a ribbon or a tube built on the curve in the look's
//! `surface(uv)` hook keeps its letters evenly spaced and upright.
//!
//! The result is a small float texture, `n` texels wide and four rows tall:
//! row 0 the point and its arc-length fraction, rows 1..3 the tangent,
//! normal and binormal; `k_curve.x` is the length.
//!
//! [`Frames`] bends the framing: `closed` spreads the twist parallel
//! transport leaves between a loop's ends evenly along it (no jump where
//! the ends meet), `up` holds the normal square to a world direction (a
//! camera-facing ribbon that must never turn edge-on).

/// Rows of the curve texture.
pub const ROWS: usize = 4;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-9 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 0.0]
    }
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// How a curve is framed (`curve: {closed: true up: vec3(0, 1, 0)}`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frames {
    /// The curve is a loop: tangents wrap and the transport's leftover
    /// twist is spread along it.
    pub closed: bool,
    /// Hold the normal square to this direction (where the tangent is not
    /// along it) instead of transporting it.
    pub up: Option<[f32; 3]>,
}

/// Turn `v` (square to unit `t`) by `a` radians about `t`.
fn turn(v: [f32; 3], t: [f32; 3], a: f32) -> [f32; 3] {
    let c = cross(t, v);
    let (sa, ca) = a.sin_cos();
    [v[0] * ca + c[0] * sa, v[1] * ca + c[1] * sa, v[2] * ca + c[2] * sa]
}

/// `points` resampled to `n` points evenly spaced by arc length, framed by
/// parallel transport (or as `frames` asks); the texture data (RGBA, `n` x
/// [`ROWS`]) and the length.
pub fn resample(points: &[[f32; 3]], n: usize, frames: Frames) -> (Vec<f32>, f32) {
    let n = n.max(2);
    let mut data = vec![0.0f32; n * ROWS * 4];
    if points.len() < 2 {
        return (data, 0.0);
    }
    let mut cum = Vec::with_capacity(points.len());
    cum.push(0.0f32);
    for w in points.windows(2) {
        let d = sub(w[1], w[0]);
        cum.push(cum.last().unwrap() + dot(d, d).sqrt());
    }
    let len = *cum.last().unwrap();
    let mut pos = Vec::with_capacity(n);
    let mut seg = 0;
    for i in 0..n {
        let s = len * i as f32 / (n - 1) as f32;
        while seg + 2 < cum.len() && cum[seg + 1] < s {
            seg += 1;
        }
        let span = (cum[seg + 1] - cum[seg]).max(1e-9);
        pos.push(lerp(points[seg], points[seg + 1], ((s - cum[seg]) / span).clamp(0.0, 1.0)));
    }
    // Tangents by central differences, then the normal carried along
    // (projected off each new tangent), starting from the world up (or x
    // where the curve starts vertical).
    // A loop's ends are the same point: its tangents wrap around them.
    let tan: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            if frames.closed && (i == 0 || i == n - 1) {
                norm(sub(pos[1], pos[n - 2]))
            } else {
                norm(sub(pos[(i + 1).min(n - 1)], pos[i.saturating_sub(1)]))
            }
        })
        .collect();
    let off = |v: [f32; 3], t: [f32; 3]| norm(sub(v, [t[0] * dot(v, t), t[1] * dot(v, t), t[2] * dot(v, t)]));
    let mut nrm = if dot(tan[0], [0.0, 1.0, 0.0]).abs() < 0.95 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let mut normals = Vec::with_capacity(n);
    for t in &tan {
        let from = match frames.up {
            Some(up) if dot(off(up, *t), off(up, *t)) > 0.5 && dot(norm(up), *t).abs() < 0.98 => up,
            _ => nrm,
        };
        let projected = off(from, *t);
        if dot(projected, projected) > 0.5 {
            nrm = projected;
        }
        normals.push(nrm);
    }
    // Closed and transported: the last normal should be the first; spread
    // the angle between them along the loop.
    if frames.closed && frames.up.is_none() && n > 2 {
        let (first, last, t0) = (normals[0], normals[n - 1], tan[0]);
        let twist = dot(cross(last, first), t0).atan2(dot(last, first));
        for (i, v) in normals.iter_mut().enumerate() {
            *v = turn(*v, tan[i], twist * i as f32 / (n - 1) as f32);
        }
    }
    for i in 0..n {
        let t = tan[i];
        let nrm = normals[i];
        let b = cross(t, nrm);
        let rows = [[pos[i][0], pos[i][1], pos[i][2], i as f32 / (n - 1) as f32], [t[0], t[1], t[2], 0.0], [nrm[0], nrm[1], nrm[2], 0.0], [b[0], b[1], b[2], 0.0]];
        for (r, v) in rows.iter().enumerate() {
            let at = (r * n + i) * 4;
            data[at..at + 4].copy_from_slice(v);
        }
    }
    (data, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_even_by_arc_length_and_frames_orthonormal() {
        // A polyline whose points bunch up at the start.
        let pts: Vec<[f32; 3]> = (0..50).map(|i| {
            let u = (i as f32 / 49.0).powi(3);
            [u * 10.0, (u * 6.0).sin(), 0.0]
        }).collect();
        let (d, len) = resample(&pts, 64, Frames::default());
        assert!(len > 10.0);
        let p = |i: usize| [d[i * 4], d[i * 4 + 1], d[i * 4 + 2]];
        let step: Vec<f32> = (1..64).map(|i| dot(sub(p(i), p(i - 1)), sub(p(i), p(i - 1))).sqrt()).collect();
        let (lo, hi) = step.iter().fold((f32::MAX, 0.0f32), |a, s| (a.0.min(*s), a.1.max(*s)));
        assert!(hi / lo < 1.3, "even spacing: {lo} .. {hi}");
        for i in 0..64 {
            let row = |r: usize| [d[(r * 64 + i) * 4], d[(r * 64 + i) * 4 + 1], d[(r * 64 + i) * 4 + 2]];
            let (t, nn, b) = (row(1), row(2), row(3));
            assert!(dot(t, nn).abs() < 1e-3 && dot(t, b).abs() < 1e-3 && (dot(nn, nn) - 1.0).abs() < 1e-3, "frame at {i}");
        }
    }

    fn frame(d: &[f32], n: usize, i: usize, r: usize) -> [f32; 3] {
        [d[(r * n + i) * 4], d[(r * n + i) * 4 + 1], d[(r * n + i) * 4 + 2]]
    }

    /// A closed knot's framing meets itself; an `up` frame never leaves the
    /// plane square to it.
    #[test]
    fn closed_loops_meet_and_up_frames_hold() {
        let knot: Vec<[f32; 3]> = (0..=400)
            .map(|i| {
                let a = i as f32 / 400.0 * std::f32::consts::TAU;
                let (p, q) = (2.0, 3.0);
                let r = 2.0 + (q * a).cos();
                [r * (p * a).cos(), r * (p * a).sin(), (q * a).sin()]
            })
            .collect();
        let n = 256;
        let (open, _) = resample(&knot, n, Frames::default());
        let (closed, _) = resample(&knot, n, Frames { closed: true, up: None });
        let gap = |d: &[f32]| dot(frame(d, n, 0, 2), frame(d, n, n - 1, 2));
        assert!(gap(&closed) > 0.999, "closed: the ends' normals agree ({})", gap(&closed));
        assert!(gap(&open) < gap(&closed));
        // A flat loop in the xz plane framed with up y: normals stay up.
        let ring: Vec<[f32; 3]> = (0..=100).map(|i| {
            let a = i as f32 / 100.0 * std::f32::consts::TAU;
            [a.cos() * 3.0, 0.0, a.sin() * 3.0]
        }).collect();
        let (d, _) = resample(&ring, 64, Frames { closed: true, up: Some([0.0, 1.0, 0.0]) });
        for i in 0..64 {
            assert!(frame(&d, 64, i, 2)[1] > 0.999, "normal {i} is up");
        }
    }
}
