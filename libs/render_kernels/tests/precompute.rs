//! AK5: a stepper run ahead of time into a history, sampled by t.

use makepad_render_kernels::compute::sched::{InlineExecutor, Job, Priority};
use makepad_render_kernels::precompute::precompute;
use makepad_render_kernels::{compile, engine};
use std::time::{Duration, Instant};

// `math: portable`: the kernels promise the host loop's f32 bits (a fast
// kernel fuses multiply-adds).
const SPRING: &str = "let math = portable\nlet prev = input(vec2)\nlet next = output(vec2)\nlet k = param(400)\nlet d = param(8)\nlet dt = param(0.001)\nfn element(i) { let s = prev[i]\n let target = if time < 4.0 { 20.0 } else { -5.0 }\n let v = s.y + (k * (target - s.x) - d * s.y) * dt\n next[i] = vec2(s.x + v * dt, v) }";

#[test]
fn a_one_kilohertz_spring_over_ten_seconds() {
    let kern = compile(SPRING, &[], &[]).unwrap();
    let steps = 10_001;
    let run = || precompute(kern.clone(), vec![0; 2], 1, 2, steps, 0.0, 0.001, &InlineExecutor, 1, |j| {
        j.set_param("dt", 0.001);
    }).unwrap();
    let h = run();
    // The host's own loop, the same f32 operations.
    let (mut x, mut v) = (0.0f32, 0.0f32);
    for s in 1..steps {
        let t = 0.0 + 0.001f32 * s as f32;
        let target = if t < 4.0 { 20.0 } else { -5.0 };
        v = v + (400.0 * (target - x) - 8.0 * v) * 0.001;
        x = x + v * 0.001;
        assert_eq!(h.table[s * 2], x.to_bits(), "step {s}");
    }
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        std::hint::black_box(run());
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    println!("10k steps of a one-element stepper: {best:.3} ms");
    // Sampled in a kernel by t, exactly as the host samples it.
    let sample = compile("let math = portable\nuse std.anim.*\nlet table = input(f32)\nlet out = output(f32)\nlet steps = param(2)\nfn element(i) { out[i] = history_at(table, 1, 2, 0, 0, 0.0, 0.001, int(steps), float(i) * 0.0137 - 0.5) }", &[], &[]).unwrap();
    let n = 1000;
    let mut job = Job::new(sample, n);
    job.input_vec_u32("table", h.table.clone()).unwrap();
    job.output_u32("out", vec![0; n]).unwrap();
    job.set_param("steps", steps as f32);
    let mut job = engine().run(job, Priority::Near, Duration::from_secs(5)).unwrap_or_else(|(_, e)| panic!("{}", e));
    let out = job.take_output_u32("out").unwrap();
    for i in 0..n {
        assert_eq!(out[i], h.at(0, 0, i as f32 * 0.0137 - 0.5).to_bits(), "sample {i}");
    }
}

#[test]
fn coupled_elements_step_in_lockstep() {
    // Each element relaxes toward its neighbours' previous values: the
    // steps must see the whole previous state.
    let src = "let math = portable\nlet prev = input(f32)\nlet next = output(f32)\nfn element(i) { let l = prev[max(i - 1, 0)]\n let r = prev[min(i + 1, count - 1)]\n next[i] = prev[i] + (l + r - 2.0 * prev[i]) * 0.25 }";
    let kern = compile(src, &[], &[]).unwrap();
    let n = 20_000;
    let init: Vec<f32> = (0..n).map(|i| if i % 1000 == 0 { 10.0 } else { 0.0 }).collect();
    let h = precompute(kern, init.iter().map(|v| v.to_bits()).collect(), n, 1, 51, 0.0, 0.1, engine().executor(), engine().threads(), |_| {}).unwrap();
    let mut s = init;
    for step in 1..51 {
        let p = s.clone();
        for i in 0..n {
            s[i] = p[i] + (p[i.saturating_sub(1)] + p[(i + 1).min(n - 1)] - 2.0 * p[i]) * 0.25;
        }
        assert!(s.iter().enumerate().all(|(i, v)| v.to_bits() == h.table[step * n + i]), "step {step}");
    }
}
