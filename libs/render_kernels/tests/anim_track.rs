//! AK1: a Motion key track evaluated per element in a kernel, bit for bit
//! Motion's `sample_track` (the reference below is its code, with the
//! portable maths the kernel uses for sine, expo, elastic and spring).

use makepad_csg_math::portable as pm;
use makepad_render_kernels::anim::{ease_id, pack_track, TrackEase, CURVES};
use makepad_render_kernels::compute::sched::{InlineExecutor, Job, Priority};
use makepad_render_kernels::engine;
use std::time::{Duration, Instant};

fn ease_in(c: usize, u: f64) -> f64 {
    let pi = std::f64::consts::PI;
    match CURVES[c] {
        "quad" => u * u,
        "cubic" => u * u * u,
        "quart" => u * u * u * u,
        "quint" => u * u * u * u * u,
        "sine" => 1.0 - pm::cos(u * pi * 0.5),
        "expo" => if u <= 0.0 { 0.0 } else { (pm::powf(2.0, 10.0 * u - 10.0) - pm::powf(2.0, -10.0)) / (1.0 - pm::powf(2.0, -10.0)) },
        "circ" => 1.0 - (1.0 - u * u).max(0.0).sqrt(),
        "back" => { let c1 = 1.70158; let c3 = c1 + 1.0; c3 * u * u * u - c1 * u * u }
        "elastic" => if u <= 0.0 { 0.0 } else { let c4 = (2.0 * pi) / 3.0; -pm::powf(2.0, 10.0 * u - 10.0) * pm::sin((u * 10.0 - 10.75) * c4) },
        _ => 1.0 - bounce_out(1.0 - u),
    }
}

fn bounce_out(u: f64) -> f64 {
    let (n1, d1) = (7.5625, 2.75);
    if u < 1.0 / d1 { n1 * u * u } else if u < 2.0 / d1 { let u = u - 1.5 / d1; n1 * u * u + 0.75 } else if u < 2.5 / d1 { let u = u - 2.25 / d1; n1 * u * u + 0.9375 } else { let u = u - 2.625 / d1; n1 * u * u + 0.984375 }
}

fn spring(k: f64, d: f64, t: f64) -> f64 {
    let k = k.max(1e-6);
    let d = d.max(0.0);
    let w0 = k.sqrt();
    let zeta = d / (2.0 * w0);
    if zeta < 1.0 - 1e-9 {
        let wd = w0 * (1.0 - zeta * zeta).sqrt();
        1.0 - pm::exp(-zeta * w0 * t) * (pm::cos(wd * t) + zeta * w0 / wd * pm::sin(wd * t))
    } else if zeta <= 1.0 + 1e-9 {
        1.0 - pm::exp(-w0 * t) * (1.0 + w0 * t)
    } else {
        let s = (zeta * zeta - 1.0).sqrt();
        let (r1, r2) = (-w0 * (zeta - s), -w0 * (zeta + s));
        1.0 + r2 / (r1 - r2) * pm::exp(r1 * t) + (-r1 / (r1 - r2)) * pm::exp(r2 * t)
    }
}

fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, u: f64) -> f64 {
    let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
    let coeffs = |p1: f64, p2: f64| { let c = 3.0 * p1; let b = 3.0 * (p2 - p1) - c; (1.0 - c - b, b, c) };
    let (ax, bx, cx) = coeffs(x1, x2);
    let (ay, by, cy) = coeffs(y1, y2);
    let x_at = |s: f64| ((ax * s + bx) * s + cx) * s;
    let mut s = u;
    let mut solved = false;
    for _ in 0..8 {
        let err = x_at(s) - u;
        if err.abs() < 1e-12 { solved = true; break; }
        let d = (3.0 * ax * s + 2.0 * bx) * s + cx;
        if d.abs() < 1e-9 { break; }
        s -= err / d;
        if !(0.0..=1.0).contains(&s) { break; }
    }
    if !solved || !(0.0..=1.0).contains(&s) {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        s = u;
        for _ in 0..64 {
            let x = x_at(s);
            if (x - u).abs() < 1e-12 { break; }
            if x < u { lo = s } else { hi = s }
            s = 0.5 * (lo + hi);
        }
    }
    ((ay * s + by) * s + cy) * s
}

fn apply(e: TrackEase, u: f64, seconds: f64) -> f64 {
    if !(u > 0.0) { return 0.0; }
    if u >= 1.0 { return 1.0; }
    match e {
        TrackEase::Id(0) => u,
        TrackEase::Id(1) => 0.0,
        TrackEase::Id(id) => {
            let (c, dir) = (((id - 2) / 3) as usize, (id - 2) % 3);
            match dir { 0 => ease_in(c, u), 1 => 1.0 - ease_in(c, 1.0 - u), _ => if u < 0.5 { 0.5 * ease_in(c, 2.0 * u) } else { 1.0 - 0.5 * ease_in(c, 2.0 - 2.0 * u) } }
        }
        TrackEase::Spring { k, d } => spring(k, d, u * seconds.max(0.0)),
        TrackEase::Bezier { x1, y1, x2, y2 } => bezier(x1, y1, x2, y2, u),
    }
}

/// Motion's sample_track (no repeat).
fn sample(keys: &[(f64, &[f64], TrackEase)], t: f64, c: usize) -> f64 {
    if !(t > keys[0].0) { return keys[0].1[c]; }
    let next = keys.partition_point(|k| k.0 <= t);
    if next >= keys.len() { return keys[keys.len() - 1].1[c]; }
    let (a, b) = (&keys[next - 1], &keys[next]);
    let span = b.0 - a.0;
    let u = apply(b.2, (t - a.0) / span.max(1e-12), span);
    a.1[c] + (b.1[c] - a.1[c]) * u
}

const SRC: &str = "let math = portable\nuse std.anim.*\nlet times = input(f64)\nlet values = input(f64)\nlet eases = input(i32)\nlet eparams = input(f64)\nlet out = output(f64)\nlet n = param(2)\nlet width = param(1)\nlet comp = param(0)\nlet t0 = param(0)\nlet dt = param(0.01)\nfn element(i) { out[i] = track_d(times, values, int(width), int(comp), eases, eparams, int(n), f64(t0) + f64(i) * f64(dt)) }";

#[test]
fn a_motion_track_evaluates_to_its_bits_per_element() {
    let k = makepad_render_kernels::compile(SRC, &[], &[]).unwrap_or_else(|e| panic!("{:?}", e));
    // Every named ease, a spring, a bezier (Newton and bisection), a hold,
    // equal times, a two-component value.
    let mut eases: Vec<TrackEase> = (0..32).map(TrackEase::Id).collect();
    eases.push(TrackEase::Spring { k: 170.0, d: 26.0 });
    eases.push(TrackEase::Spring { k: 100.0, d: 4.0 });
    eases.push(TrackEase::Spring { k: 100.0, d: 40.0 });
    eases.push(TrackEase::Bezier { x1: 0.7, y1: 0.0, x2: 0.3, y2: 1.0 });
    eases.push(TrackEase::Bezier { x1: 0.0, y1: 1.4, x2: 1.0, y2: -0.4 });
    assert_eq!(ease_id("ease_in_out_elastic"), Some(2 + 3 * 8 + 2));
    let vals: Vec<[f64; 2]> = (0..eases.len() + 1).map(|k| [(k as f64 * 1.37).sin() * 50.0, k as f64 * 3.0 - 7.5]).collect();
    let mut times = Vec::new();
    let mut t = -0.25;
    for k in 0..vals.len() {
        times.push(t);
        t += if k == 5 { 0.0 } else { 0.5 + (k % 3) as f64 * 0.25 };
    }
    let keys: Vec<(f64, &[f64], TrackEase)> = (0..vals.len()).map(|k| (times[k], &vals[k][..], if k == 0 { TrackEase::Id(0) } else { eases[k - 1] })).collect();
    let packed = pack_track(&keys);
    let count = 6000usize;
    let (t0, dt) = (-0.5f32, (t + 0.5) as f32 / count as f32);
    for comp in 0..2 {
        let mut job = Job::new(k.clone(), count);
        job.input_vec_u32("times", packed.times.clone()).unwrap();
        job.input_vec_u32("values", packed.values.clone()).unwrap();
        job.input_vec_u32("eases", packed.eases.clone()).unwrap();
        job.input_vec_u32("eparams", packed.eparams.clone()).unwrap();
        job.output_u32("out", vec![0; count * 2]).unwrap();
        for (p, v) in [("n", packed.keys as f32), ("width", 2.0), ("comp", comp as f32), ("t0", t0), ("dt", dt)] {
            job.set_param(p, v);
        }
        let mut job = engine().run(job, Priority::Near, Duration::from_secs(10)).unwrap_or_else(|(_, e)| panic!("{}", e));
        let out = job.take_output_u32("out").unwrap();
        for i in 0..count {
            let got = f64::from_bits(out[2 * i] as u64 | (out[2 * i + 1] as u64) << 32);
            let t = t0 as f64 + i as f64 * dt as f64;
            let want = sample(&keys, t, comp);
            assert_eq!(got.to_bits(), want.to_bits(), "comp {comp} t {t}: {got} vs {want}");
        }
    }
}

#[test]
fn a_hundred_thousand_track_evaluations() {
    // The AK1 target: 100k evaluations <= 0.5 ms on 8 threads. f32 tracks
    // through std.anim.keys with a named ease run four-wide; the f64
    // Motion track (track_d) runs scalar.
    let keys_src = "use std.anim.*\nlet times = input(f32)\nlet values = input(f32)\nlet out = output(f32)\nlet dt = param(0.0001)\nfn element(i) { let t = float(i % 1000) * dt * 1000.0\n out[i] = keys(times, values, 16, t) }";
    let k = makepad_render_kernels::compile(keys_src, &[], &[]).unwrap_or_else(|e| panic!("{:?}", e));
    let times: Vec<u32> = (0..16).map(|k| (k as f32 * 0.07).to_bits()).collect();
    let n = 100_000;
    let mut best = f64::MAX;
    for _ in 0..20 {
        let mut job = Job::new(k.clone(), n);
        job.input_vec_u32("times", times.clone()).unwrap();
        job.input_vec_u32("values", times.clone()).unwrap();
        job.output_u32("out", vec![0; n]).unwrap();
        let t = Instant::now();
        let _ = engine().run(job, Priority::MustComplete, Duration::from_secs(10)).unwrap_or_else(|(_, e)| panic!("{}", e));
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    let kd = makepad_render_kernels::compile(SRC, &[], &[]).unwrap();
    let keys: Vec<(f64, &[f64], TrackEase)> = vec![(0.0, &[0.0][..], TrackEase::Id(0)), (1.0, &[1.0][..], TrackEase::Id(ease_id("ease_out_cubic").unwrap())), (2.0, &[0.0][..], TrackEase::Spring { k: 170.0, d: 26.0 })];
    let packed = pack_track(&keys);
    let mut best_d = f64::MAX;
    for _ in 0..10 {
        let mut job = Job::new(kd.clone(), n);
        job.input_vec_u32("times", packed.times.clone()).unwrap();
        job.input_vec_u32("values", packed.values.clone()).unwrap();
        job.input_vec_u32("eases", packed.eases.clone()).unwrap();
        job.input_vec_u32("eparams", packed.eparams.clone()).unwrap();
        job.output_u32("out", vec![0; n * 2]).unwrap();
        job.set_param("n", 3.0);
        job.set_param("dt", 2.0 / n as f32);
        let t = Instant::now();
        let _ = engine().run(job, Priority::MustComplete, Duration::from_secs(10)).unwrap_or_else(|(_, e)| panic!("{}", e));
        best_d = best_d.min(t.elapsed().as_secs_f64() * 1e3);
    }
    println!("100k evaluations: f32 keys {best:.3} ms, f64 Motion track {best_d:.3} ms on {} threads", engine().threads());
    let _ = InlineExecutor;
}
