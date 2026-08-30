// Performance measurements (ignored by default; run explicitly, in release):
//   cargo test -p makepad-piano-model --release -- --ignored perf_ --nocapture
// Reports realtime headroom for full 88-key polyphony on the scalar and SIMD
// kernels, per-voice cost, and multicore scaling of the offline path.

mod common;

use common::*;
use makepad_piano_model::{Piano, PianoEvent::*};
use std::time::Instant;

/// Strike all 88 keys, pedal down, re-strike every 1.5 s so every voice and
/// every sympathetic bank stays busy for the whole run.
fn full_load_script(seconds: f64) -> Vec<Ev> {
    let mut script = vec![ev(0.0, Sustain { value: 1.0 })];
    let mut t = 0.0;
    while t < seconds - 0.1 {
        for key in 21..=108u8 {
            script.push(ev(t + (key as f64 - 21.0) * 0.0001, NoteOn { key, velocity: 110 }));
        }
        t += 1.5;
    }
    script
}

fn run_load(p: &mut Piano, seconds: f64, block: usize, workers: usize) -> f64 {
    let script = full_load_script(seconds);
    let total = (seconds * FS as f64) as usize;
    let start = Instant::now();
    if workers <= 1 {
        let (l, r) = render(p, &script, total, block);
        assert!(l[total - 1].is_finite() && r[total - 1].is_finite());
    } else {
        let (l, r) = render_mt(p, &script, total, block, workers);
        assert!(l[total - 1].is_finite() && r[total - 1].is_finite());
    }
    start.elapsed().as_secs_f64()
}

#[test]
#[ignore]
fn perf_polyphony() {
    let seconds = 10.0;
    for (name, scalar) in [("simd", false), ("scalar", true)] {
        let mut p = Piano::new(FS);
        p.set_force_scalar(scalar);
        let path = p.kernel_path();
        let wall = run_load(&mut p, seconds, 512, 1);
        let xrt = seconds / wall;
        println!(
            "{name} ({path:?}): 88 keys ringing + pedal down + full fx: {wall:.3} s wall for {seconds} s audio = {xrt:.1}x realtime ({:.1}% of one core, {:.1} us/voice/block-of-512)",
            100.0 / xrt,
            wall / (seconds * FS as f64 / 512.0) / 88.0 * 1e6
        );
    }
}

#[test]
#[ignore]
fn perf_single_voice() {
    for (name, scalar) in [("simd", false), ("scalar", true)] {
        let mut p = Piano::new(FS);
        p.set_force_scalar(scalar);
        let script = vec![ev(0.0, NoteOn { key: 24, velocity: 110 })]; // worst-case voice (128x2 modes)
        let total = (10.0 * FS as f64) as usize;
        let start = Instant::now();
        let (l, _) = render(&mut p, &script, total, 512);
        let wall = start.elapsed().as_secs_f64();
        assert!(l[total - 1].is_finite());
        println!("{name}: single A0 voice 10 s: {wall:.3} s wall ({:.2}% of one core incl. shared fx)", wall * 10.0);
    }
}

#[test]
#[ignore]
fn perf_multicore() {
    let seconds = 10.0;
    let mut base = 0.0;
    for workers in [1usize, 2, 4, 8] {
        let mut p = Piano::new(FS);
        let wall = run_load(&mut p, seconds, 512, workers);
        if workers == 1 {
            base = wall;
        }
        println!(
            "offline multicore, {workers} workers: {wall:.3} s wall = {:.1}x realtime (speedup {:.2}x)",
            seconds / wall,
            base / wall
        );
    }
}
