// N-sphere union scaling probe — the regression bench for N-ary boolean cost.
//
// Unions N ~272-tri spheres (sphere(0.4, 16, 9)) laid out on a cubic grid,
// through the public `union_all`, in two regimes:
//   - overlapping: spacing 0.5 (every grid neighbor intersects)
//   - disjoint:    spacing 2.0 (no sphere touches any other)
//
// This is the probe from PERF_REVIEW.md §3. Baseline (left-fold union_all,
// 2026-08-27, Apple Silicon): N=128 overlapping 4.6 s, disjoint 5.1 s.
//
//   cargo run --release --example scaling            # full table
//   cargo run --release --example scaling 32         # single N, both regimes
//
// Each case is run up to 3x (median). Cases slower than SLOW_SECS after the
// first run are reported from that single sample.

use makepad_csg::{union_all, Solid};
use std::time::Instant;

const SLOW_SECS: f64 = 10.0;

fn spheres(n: usize, spacing: f64) -> Vec<Solid> {
    let side = (n as f64).cbrt().ceil() as usize;
    (0..n)
        .map(|i| {
            let x = (i % side) as f64;
            let y = ((i / side) % side) as f64;
            let z = (i / (side * side)) as f64;
            Solid::sphere(0.4, 16, 9).translate(x * spacing, y * spacing, z * spacing)
        })
        .collect()
}

fn median3<F: FnMut() -> Solid>(mut f: F) -> (f64, Solid) {
    let t = Instant::now();
    let mut r = f();
    let first = t.elapsed().as_secs_f64();
    let mut samples = vec![first];
    if first <= SLOW_SECS {
        for _ in 0..2 {
            let t = Instant::now();
            r = f();
            samples.push(t.elapsed().as_secs_f64());
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (samples[samples.len() / 2], r)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ns: Vec<usize> = if args.is_empty() {
        vec![2, 8, 32, 128]
    } else {
        args.iter().map(|a| a.parse().expect("N")).collect()
    };

    println!("# N-sphere union scaling (sphere(0.4, 16, 9), cubic grid)");
    println!();
    println!("| N | overlapping (0.5) | tris | vol | valid | disjoint (2.0) | tris | vol | valid |");
    println!("|---:|---:|---:|---:|---|---:|---:|---:|---|");

    for &n in &ns {
        let over = spheres(n, 0.5);
        let (t_over, r_over) = median3(|| union_all(&over));
        let disj = spheres(n, 2.0);
        let (t_disj, r_disj) = median3(|| union_all(&disj));
        println!(
            "| {} | {:.1} ms | {} | {:.3} | {} | {:.1} ms | {} | {:.3} | {} |",
            n,
            t_over * 1000.0,
            r_over.triangle_count(),
            r_over.volume(),
            r_over.is_valid(),
            t_disj * 1000.0,
            r_disj.triangle_count(),
            r_disj.volume(),
            r_disj.is_valid(),
        );
    }
}
