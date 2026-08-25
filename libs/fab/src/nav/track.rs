//! Lane C. C2-smooth sampling of [`CameraTrack`]s for tour playback.
//!
//! Lane G bakes keys at 24 or 30 fps. The frozen `CameraTrack::sample` is
//! linear; playback here uses a natural cubic spline (C2, interpolating) so
//! a scrub or a playhead sitting between keys does not show a crease. Two
//! keys fall back to linear; one key is a still.

use crate::api::*;
use crate::nav::orbit::WORLD_UP;
use makepad_widgets::*;

/// Sample `track` at time `t` with C2 interpolation between keys.
pub fn sample_c2(track: &CameraTrack, t: f32) -> Option<CameraKey> {
    let n = track.keys.len();
    if n == 0 {
        return None;
    }
    if n == 1 {
        return Some(track.keys[0]);
    }
    let t0 = track.keys[0].t;
    let t1 = track.keys[n - 1].t;
    if t <= t0 {
        return Some(track.keys[0]);
    }
    if t >= t1 {
        return Some(track.keys[n - 1]);
    }
    if n == 2 {
        return CameraTrack::sample(track, t);
    }

    let times: Vec<f32> = track.keys.iter().map(|k| k.t).collect();
    let px: Vec<f32> = track.keys.iter().map(|k| k.pos.x).collect();
    let py: Vec<f32> = track.keys.iter().map(|k| k.pos.y).collect();
    let pz: Vec<f32> = track.keys.iter().map(|k| k.pos.z).collect();
    let lx: Vec<f32> = track.keys.iter().map(|k| k.look_at.x).collect();
    let ly: Vec<f32> = track.keys.iter().map(|k| k.look_at.y).collect();
    let lz: Vec<f32> = track.keys.iter().map(|k| k.look_at.z).collect();
    let ux: Vec<f32> = track.keys.iter().map(|k| k.up.x).collect();
    let uy: Vec<f32> = track.keys.iter().map(|k| k.up.y).collect();
    let uz: Vec<f32> = track.keys.iter().map(|k| k.up.z).collect();
    let fov: Vec<f32> = track.keys.iter().map(|k| k.fov_y_deg).collect();

    let pos = vec3(
        eval_natural(&times, &px, t),
        eval_natural(&times, &py, t),
        eval_natural(&times, &pz, t),
    );
    let look_at = vec3(
        eval_natural(&times, &lx, t),
        eval_natural(&times, &ly, t),
        eval_natural(&times, &lz, t),
    );
    let mut up = vec3(
        eval_natural(&times, &ux, t),
        eval_natural(&times, &uy, t),
        eval_natural(&times, &uz, t),
    );
    let nrm = up.normalize();
    if nrm.is_finite() {
        up = nrm;
    } else {
        up = WORLD_UP;
    }
    Some(CameraKey {
        t,
        pos,
        look_at,
        up,
        fov_y_deg: eval_natural(&times, &fov, t),
    })
}

/// Natural cubic spline: second derivatives via the Thomas algorithm, then
/// evaluate on the segment that contains `x`. Endpoints have m = 0 so the
/// curve is C2 on the open interval and interpolates every knot.
fn second_derivs(t: &[f32], y: &[f32]) -> Vec<f32> {
    let n = y.len();
    let mut m = vec![0.0f32; n];
    if n < 3 {
        return m;
    }
    // Interior system of size n-2, 1-based in the classical write-up:
    // h[i-1] m[i-1] + 2(h[i-1]+h[i]) m[i] + h[i] m[i+1]
    //   = 6 (Δy[i]/h[i] − Δy[i-1]/h[i-1])  for i = 1..n-2
    // with m[0] = m[n-1] = 0.
    let k = n - 2;
    let mut a = vec![0.0f32; k];
    let mut b = vec![0.0f32; k];
    let mut c = vec![0.0f32; k];
    let mut d = vec![0.0f32; k];
    for i in 0..k {
        let i1 = i + 1;
        let h0 = (t[i1] - t[i1 - 1]).max(1e-9);
        let h1 = (t[i1 + 1] - t[i1]).max(1e-9);
        a[i] = h0;
        b[i] = 2.0 * (h0 + h1);
        c[i] = h1;
        d[i] = 6.0 * ((y[i1 + 1] - y[i1]) / h1 - (y[i1] - y[i1 - 1]) / h0);
    }
    // Forward sweep.
    for i in 1..k {
        let w = a[i] / b[i - 1];
        b[i] -= w * c[i - 1];
        d[i] -= w * d[i - 1];
    }
    let mut x = vec![0.0f32; k];
    x[k - 1] = d[k - 1] / b[k - 1];
    for i in (0..k - 1).rev() {
        x[i] = (d[i] - c[i] * x[i + 1]) / b[i];
    }
    for i in 0..k {
        m[i + 1] = x[i];
    }
    m
}

fn eval_natural(t: &[f32], y: &[f32], x: f32) -> f32 {
    let n = y.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 || x <= t[0] {
        return y[0];
    }
    if x >= t[n - 1] {
        return y[n - 1];
    }
    if n == 2 {
        let span = (t[1] - t[0]).max(1e-9);
        let f = ((x - t[0]) / span).clamp(0.0, 1.0);
        return y[0] + (y[1] - y[0]) * f;
    }
    let m = second_derivs(t, y);
    let mut i = t.partition_point(|u| *u <= x).saturating_sub(1);
    if i >= n - 1 {
        i = n - 2;
    }
    let h = (t[i + 1] - t[i]).max(1e-9);
    let dt = x - t[i];
    // S(x) = y_i + b dt + (m_i/2) dt² + ((m_{i+1}−m_i)/(6h)) dt³
    // with b = Δy/h − h (2 m_i + m_{i+1}) / 6
    let b = (y[i + 1] - y[i]) / h - h * (2.0 * m[i] + m[i + 1]) / 6.0;
    let c = m[i] * 0.5;
    let d = (m[i + 1] - m[i]) / (6.0 * h);
    y[i] + b * dt + c * dt * dt + d * dt * dt * dt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: f32, x: f32) -> CameraKey {
        CameraKey {
            t,
            pos: vec3(x, 0.0, 1.0),
            look_at: vec3(x + 1.0, 0.0, 1.0),
            up: WORLD_UP,
            fov_y_deg: 40.0,
        }
    }

    #[test]
    fn interpolates_the_knots() {
        let track = CameraTrack {
            name: "t".into(),
            kind: "test".into(),
            fps: 30.0,
            keys: (0..12)
                .map(|i| {
                    let t = i as f32 / 30.0;
                    key(t, (t * std::f32::consts::TAU).sin())
                })
                .collect(),
        };
        for k in &track.keys {
            let s = sample_c2(&track, k.t).unwrap();
            assert!(
                (s.pos.x - k.pos.x).abs() < 1e-4,
                "knot t={} {} vs {}",
                k.t,
                s.pos.x,
                k.pos.x
            );
        }
    }

    #[test]
    fn two_keys_are_linear() {
        let track = CameraTrack {
            name: "t".into(),
            kind: "test".into(),
            fps: 24.0,
            keys: vec![key(0.0, 0.0), key(1.0, 10.0)],
        };
        let s = sample_c2(&track, 0.5).unwrap();
        assert!((s.pos.x - 5.0).abs() < 1e-5);
    }

    #[test]
    fn c2_is_smooth_across_a_knot() {
        // A sine sampled at 24 fps: the spline's first derivative from the
        // left and from the right of an interior knot must agree.
        let fps = 24.0f32;
        let track = CameraTrack {
            name: "t".into(),
            kind: "test".into(),
            fps,
            keys: (0..48)
                .map(|i| {
                    let t = i as f32 / fps;
                    key(t, (t * 3.0).sin())
                })
                .collect(),
        };
        let t = 10.0 / fps;
        let h = 1e-4f32;
        let p0 = sample_c2(&track, t - h).unwrap().pos.x;
        let p1 = sample_c2(&track, t).unwrap().pos.x;
        let p2 = sample_c2(&track, t + h).unwrap().pos.x;
        let d_left = (p1 - p0) / h;
        let d_right = (p2 - p1) / h;
        assert!(
            (d_left - d_right).abs() < 0.05,
            "C1 break at knot: {d_left} vs {d_right}"
        );
    }
}
