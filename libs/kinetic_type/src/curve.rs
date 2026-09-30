//! Curves at constant arc length: a kit's `curve_fn` gives points along a
//! path (any parameterisation); [`resample`] spaces them evenly by arc
//! length and frames them by parallel transport (no twist, no flips at
//! inflections), so a ribbon or a tube built on the curve in the look's
//! `surface(uv)` hook keeps its letters evenly spaced and upright.
//!
//! The result is a small float texture, `n` texels wide and four rows tall:
//! row 0 the point and its arc-length fraction, rows 1..3 the tangent,
//! normal and binormal; `k_curve.x` is the length.

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

/// `points` resampled to `n` points evenly spaced by arc length, framed by
/// parallel transport; the texture data (RGBA, `n` x [`ROWS`]) and the
/// length.
pub fn resample(points: &[[f32; 3]], n: usize) -> (Vec<f32>, f32) {
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
    let tan: Vec<[f32; 3]> = (0..n).map(|i| norm(sub(pos[(i + 1).min(n - 1)], pos[i.saturating_sub(1)]))).collect();
    let mut nrm = if dot(tan[0], [0.0, 1.0, 0.0]).abs() < 0.95 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    for i in 0..n {
        let t = tan[i];
        let projected = norm(sub(nrm, [t[0] * dot(nrm, t), t[1] * dot(nrm, t), t[2] * dot(nrm, t)]));
        if dot(projected, projected) > 0.5 {
            nrm = projected;
        }
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
        let (d, len) = resample(&pts, 64);
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
}
