//! Generations and compaction: a branching tree grown pass by pass, the
//! same for any thread count and backend, and compaction against the serial
//! reference.

use makepad_script_compute::kernel::{compact, compile, compile_with, Kernel, K_PARAMS};
use makepad_script_compute::pipeline::{compact_into, Generations};
use makepad_script_compute::sched::{Executor, InlineExecutor, ThreadExecutor};
use makepad_script_compute::Backend;
use std::sync::Arc;
use std::time::Instant;

/// A segment: start (x, y), angle, length, depth, birth: 6 words. Each
/// grows 5 children (4 for a tenth of them, by a hash of its position).
const TREE: &str = r#"
    struct Seg { x: 0.0, y: 0.0, a: 0.0, l: 0.0, depth: 0.0, birth: 0.0 }
    let prev = input(Seg)
    let next = emit_buffer(Seg, 5)
    let gen_time = param(0.0)
    fn element(i) {
        let s = prev[i]
        let ex = s.x + cos(s.a) * s.l
        let ey = s.y + sin(s.a) * s.l
        let kids = if hash01(int(ex * 1000.0), int(ey * 1000.0)) < 0.1 { 4 } else { 5 }
        for k in 0..kids {
            let c = Seg{}
            c.x = ex
            c.y = ey
            c.a = s.a + (float(k) - float(kids - 1) * 0.5) * 0.4
            c.l = s.l * 0.8
            c.depth = s.depth + 1.0
            c.birth = gen_time
            emit(next, c)
        }
    }
"#;

fn seed() -> Vec<u32> {
    [0.0f32, 0.0, 1.5707964, 1.0, 0.0, 0.0].iter().map(|x| x.to_bits()).collect()
}

fn grow(k: &Arc<Kernel>, exec: &dyn Executor, threads: usize) -> (makepad_script_compute::pipeline::Grown, f64) {
    let mut g = Generations::new(k.clone(), "prev", "next").unwrap();
    let t0 = Instant::now();
    let grown = g.grow(&seed(), 6, exec, threads, |job, gen| { job.set_param("gen_time", gen as f32 * 0.5); }).unwrap();
    (grown, t0.elapsed().as_secs_f64() * 1e3)
}

#[test]
fn generations_are_identical_for_any_thread_count_and_backend() {
    let k = compile(TREE).unwrap_or_else(|e| panic!("{:?}", e));
    let (one, _) = grow(&k, &InlineExecutor, 1);
    let exec = ThreadExecutor::new(7);
    let (many, _) = grow(&k, &exec, 8);
    let ki = compile_with(TREE, &[], Backend::Interp).unwrap();
    let (interp, _) = grow(&ki, &InlineExecutor, 1);
    assert_eq!(one, many);
    assert_eq!(one, interp);
    assert!(!one.overflowed);
    let n = one.records.len() / 6;
    // Every generation's records carry their depth and birth.
    for g in 0..one.starts.len() {
        let end = one.starts.get(g + 1).copied().unwrap_or(n);
        for r in one.starts[g]..end {
            assert_eq!(f32::from_bits(one.records[r * 6 + 4]), g as f32);
            assert_eq!(f32::from_bits(one.records[r * 6 + 5]), if g == 0 { 0.0 } else { g as f32 * 0.5 });
        }
    }
    // The target: ~20k segments over 6 generations within 2 ms (8 threads).
    let mut best = f64::MAX;
    for _ in 0..5 {
        best = best.min(grow(&k, &exec, 8).1);
    }
    eprintln!("{} segments over {} generations: {:.2} ms on 8 workers", n, one.starts.len() - 1, best);
    assert!(n > 15_000, "{} segments", n);
    let _ = K_PARAMS;
}

#[test]
fn parallel_compaction_equals_the_serial_one() {
    let (width, cap) = (3usize, 4usize);
    let n = 30_000;
    let counts: Vec<u32> = (0..n).map(|e| ((e * 7919) % 6) as u32).collect();
    let data: Vec<u32> = (0..n * cap * width).map(|i| i as u32).collect();
    let want: Vec<u32> = compact(&data.iter().map(|x| f32::from_bits(*x)).collect::<Vec<_>>(), &counts, width, cap).iter().map(|x| x.to_bits()).collect();
    let exec = ThreadExecutor::new(5);
    let mut out = Vec::new();
    for threads in [1, 3, 6] {
        let made = compact_into(&data, &counts, width, cap, &mut out, &exec, threads);
        assert_eq!(made * width, want.len());
        assert_eq!(out, want, "{} threads", threads);
    }
    // Counts past the capacity clamp; short data never reads out of bounds.
    let made = compact_into(&data[..10 * cap * width], &[9, 1, 0, 2], width, cap, &mut out, &InlineExecutor, 1);
    assert_eq!(made, 4 + 1 + 0 + 2);
}
