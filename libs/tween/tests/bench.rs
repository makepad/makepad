//! Micro benchmark (design 12.13): ns per tween per frame for the engine's
//! per-frame path. Ignored by default; run with
//! `cargo test -p makepad-tween --release -- --ignored --nocapture`.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::hint::black_box;
use std::time::Instant;

use makepad_tween::*;

const X: PropKey = PropKey(1);
const Y: PropKey = PropKey(2);
const R: PropKey = PropKey(3);
const FRAMES: usize = 600;
const RUNS: usize = 5;

/// Min over runs of the mean ns per tween per frame.
fn measure(name: &str, tweens: usize, mut build: impl FnMut() -> TweenEngine) -> f64 {
    let mut best = f64::INFINITY;
    let mut buf = Vec::new();
    for _ in 0..RUNS {
        let mut e = build();
        e.advance(1.0 / 120.0);
        e.swap_events(&mut buf);
        let t0 = Instant::now();
        for _ in 0..FRAMES {
            e.advance(1.0 / 120.0);
            e.swap_events(&mut buf);
            black_box(e.changes().len());
            e.clear_changes();
        }
        let ns = t0.elapsed().as_nanos() as f64 / (FRAMES * tweens) as f64;
        best = best.min(ns);
        black_box(e.get_f64(TargetId(0), X));
    }
    println!("bench {name:<44} {tweens:>6} tweens  {best:>8.2} ns/tween/frame");
    best
}

fn root_tweens(n: u32, ease: Easing) -> TweenEngine {
    let mut e = TweenEngine::with_capacity(n as usize, n as usize, n as usize);
    for i in 0..n {
        e.seed(TargetId(i), X, TweenValue::F64(0.0));
        e.to(
            TargetId(i).into(),
            &[PropTo::to_f64(X, 100.0 + i as f64)],
            TweenOpts::new()
                .duration(0.5 + (i % 10) as f64 * 0.1)
                .ease(ease)
                .repeat(-1)
                .yoyo(true),
        );
    }
    e
}

#[test]
#[ignore]
fn bench_engine() {
    let f64_ns = measure("10k F64 OutCubic", 10_000, || {
        root_tweens(10_000, Easing::OutCubic)
    });
    measure("10k parity Bezier (standard)", 10_000, || {
        root_tweens(
            10_000,
            Easing::Bezier {
                x1: 0.2,
                y1: 0.0,
                x2: 0.0,
                y2: 1.0,
            },
        )
    });
    measure("10k OutElastic", 10_000, || {
        root_tweens(10_000, Easing::OutElastic)
    });
    measure("1k OKLCH colours", 1_000, || {
        let mut e = TweenEngine::new();
        for i in 0..1_000u32 {
            e.seed(
                TargetId(i),
                X,
                TweenValue::Color(Rgba::from_u32(0xff0000ff)),
            );
            e.to(
                TargetId(i).into(),
                &[PropTo::to(X, TweenValue::Color(Rgba::from_u32(0x00ff80ff)))],
                TweenOpts::new()
                    .duration(0.7)
                    .ease(Easing::InOutSine)
                    .color_space(ColorSpace::Oklch)
                    .repeat(-1)
                    .yoyo(true),
            );
        }
        e
    });
    measure("100 x 100 nested repeat + yoyo timelines", 10_000, || {
        let mut e = TweenEngine::new();
        for t in 0..100u32 {
            let tl = e.timeline(TimelineOpts::new().repeat(-1).yoyo(true));
            for c in 0..100u32 {
                let id = TargetId(t * 100 + c);
                e.seed(id, X, TweenValue::F64(0.0));
                e.tl(tl).to(
                    id.into(),
                    &[PropTo::to_f64(X, 1.0)],
                    TweenOpts::new().duration(0.3).ease(Easing::OutQuad),
                    Position::at(c as f64 * 0.003),
                );
            }
        }
        e
    });
    measure("10k finished (kept, idle)", 10_000, || {
        let mut e = TweenEngine::new();
        for i in 0..10_000u32 {
            e.seed(TargetId(i), X, TweenValue::F64(0.0));
            e.to(
                TargetId(i).into(),
                &[PropTo::to_f64(X, 1.0)],
                TweenOpts::new().duration(0.001).keep(true),
            );
        }
        e.advance(0.1);
        e
    });
    measure("1 timeline, 10k children, 99% unstarted", 10_000, || {
        let mut e = TweenEngine::new();
        let tl = e.timeline(TimelineOpts::new());
        for i in 0..10_000u32 {
            e.seed(TargetId(i), X, TweenValue::F64(0.0));
            e.tl(tl).to(
                TargetId(i).into(),
                &[PropTo::to_f64(X, 1.0)],
                TweenOpts::new().duration(0.5),
                Position::at(i as f64 * 0.5),
            );
        }
        e
    });
    let path_ns = measure("1k path tweens (x, y, rot)", 1_000, || {
        let mut e = TweenEngine::new();
        let path = e.add_path(
            MotionPath::from_svg(
                "M20,160 C120,-20 220,220 320,100 S520,0 580,160 A60,60 0 0,1 460,160 L400,120 Q340,60 300,140 T160,160 Z",
            )
            .unwrap(),
        );
        for i in 0..1_000u32 {
            e.to(
                TargetId(i).into(),
                &[PropTo::path(
                    X,
                    Y,
                    path,
                    PathOpts::new().auto_rotate(R, 90.0),
                )],
                TweenOpts::new()
                    .duration(0.5 + (i % 10) as f64 * 0.1)
                    .ease(Easing::InOutSine)
                    .repeat(-1)
                    .yoyo(true),
            );
        }
        e.release_path(path);
        e
    });
    #[cfg(not(debug_assertions))]
    assert!(f64_ns < 60.0, "F64 tween {f64_ns} ns/frame");
    // Soft bound (design R1): one path sample per tween per frame.
    #[cfg(not(debug_assertions))]
    assert!(path_ns < 600.0, "path tween {path_ns} ns/frame");
    black_box((f64_ns, path_ns));
}
