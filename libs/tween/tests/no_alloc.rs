//! The zero-allocation guarantee (design 6.3 / 12.12): once everything is
//! built, `advance`, controls, kills, overwrites, reclamation and track
//! compaction never touch the heap, provided the event queue is drained
//! with `swap_events` every frame. This binary counts allocations made by
//! the test thread with a counting global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use makepad_tween::*;

struct Counting;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

fn count() {
    // `try_with`: the allocator can run while thread-locals are torn down.
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: every call forwards to the system allocator unchanged; the only
// addition is a thread-local counter increment, which does not allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count();
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        count();
        System.realloc(p, l, n)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> usize {
    ALLOCS.with(|c| c.get())
}

const X: PropKey = PropKey(1);
const Y: PropKey = PropKey(2);
const C: PropKey = PropKey(3);

fn lin(d: f64) -> TweenOpts {
    TweenOpts::new().duration(d).ease(Easing::Linear)
}

struct World {
    e: TweenEngine,
    grid_tl: TweenId,
    nested: Vec<TweenId>,
    roots: Vec<TweenId>,
    colours: TweenId,
}

fn build() -> World {
    let mut e = TweenEngine::new();
    // A 20 x 10 staggered grid, 2 props, EDGES on the group and the
    // timeline, 3 calls, watched labels, one add_pause.
    for i in 0..200 {
        e.seed(TargetId(i), X, TweenValue::F64(0.0));
        e.seed(TargetId(i), Y, TweenValue::F64(0.0));
    }
    let grid_tl = e.timeline(
        TimelineOpts::new()
            .repeat(-1)
            .yoyo(true)
            .events(EventMask::EDGES)
            .watch_labels(),
    );
    e.tl(grid_tl)
        .to(
            Targets::Range {
                first: 0,
                count: 200,
            },
            &[PropTo::to_f64(X, 100.0), PropTo::to_f64(Y, -50.0)],
            TweenOpts::new()
                .duration(0.6)
                .ease(Easing::OutBack)
                .stagger(Stagger::each(0.01).grid(20, 10).from(StaggerFrom::Center))
                .events(EventMask::EDGES),
            Position::END,
        )
        .call(Tag(1), 0.2)
        .call(Tag(2), 0.5)
        .call(Tag(3), 0.9)
        .add_label(Tag(10), 0.1)
        .add_label(Tag(11), 0.4)
        .add_pause(0.7, Tag(4));
    // 100 repeating yoyo timelines nested 2 deep.
    let mut nested = Vec::new();
    for i in 0..100u32 {
        let t = TargetId(1000 + i);
        e.seed(t, X, TweenValue::F64(0.0));
        let outer = e.timeline(
            TimelineOpts::new()
                .repeat(-1)
                .yoyo(true)
                .events(EventMask::ALL),
        );
        let inner = e.timeline(
            TimelineOpts::new()
                .repeat(3)
                .yoyo(true)
                .events(EventMask::EDGES),
        );
        e.tl(inner)
            .to(
                t.into(),
                &[PropTo::to_f64(X, 1.0)],
                lin(0.1).events(EventMask::ALL),
                Position::END,
            )
            .to(
                t.into(),
                &[PropTo::to_f64(X, 0.0)],
                lin(0.07),
                Position::END,
            );
        e.tl(outer).add(inner, 0.05);
        nested.push(outer);
    }
    // 1000 root tweens with overwrite Auto (rivals on 100 targets).
    let mut roots = Vec::new();
    for i in 0..1000u32 {
        let t = TargetId(2000 + i % 100);
        if i < 100 {
            e.seed(t, X, TweenValue::F64(0.0));
        }
        let id = e.to(
            t.into(),
            &[PropTo::to_f64(X, i as f64)],
            TweenOpts::new()
                .duration(0.2 + (i % 7) as f64 * 0.1)
                .delay((i % 13) as f64 * 0.05)
                .repeat(-1)
                .yoyo(true)
                .overwrite(Overwrite::Auto)
                .events(EventMask::INTERRUPT.with(EventMask::REPEAT)),
        );
        roots.push(id);
    }
    // A 20-colour OKLCH timeline.
    let colours = e.timeline(TimelineOpts::new().repeat(-1).yoyo(true));
    for i in 0..20u32 {
        let t = TargetId(5000 + i);
        e.seed(t, C, TweenValue::Color(Rgba::from_u32(0xff0000ff)));
        e.tl(colours).to(
            t.into(),
            &[PropTo::to(
                C,
                TweenValue::Color(Rgba::from_u32(0x0000ffff ^ (i << 8))),
            )],
            TweenOpts::new()
                .duration(0.5)
                .ease(Easing::InOutSine)
                .color_space(ColorSpace::Oklch),
            Position::prev_start(0.05),
        );
    }
    World {
        e,
        grid_tl,
        nested,
        roots,
        colours,
    }
}

#[test]
fn per_frame_path_does_not_allocate() {
    let mut w = build();
    let mut buf: Vec<TweenEvent> = Vec::new();
    // Warm up both event buffers.
    for _ in 0..3 {
        w.e.advance(1.0 / 120.0);
        w.e.swap_events(&mut buf);
    }
    let before = allocs();
    let mut next_kill = 0usize;
    let (mut events, mut interrupts, mut pauses) = (0usize, 0usize, 0usize);
    for i in 0..10_000usize {
        let dt = if i % 13 == 0 { 0.3 } else { 1.0 / 120.0 };
        w.e.advance(dt);
        w.e.swap_events(&mut buf);
        events += buf.len();
        for ev in buf.iter() {
            match ev.kind {
                EventKind::Interrupt => interrupts += 1,
                EventKind::Pause => pauses += 1,
                _ => {}
            }
        }
        if i % 97 == 0 {
            let k = i / 97;
            w.e.anim(w.grid_tl)
                .seek(Seek::Time((k % 7) as f64 * 0.13), Emit::Fire);
            w.e.anim(w.grid_tl).reverse();
            w.e.anim(w.nested[k % 100])
                .set_time_scale(if k % 2 == 0 { 0.5 } else { 1.5 });
            w.e.anim(w.nested[(k + 7) % 100]).pause().resume();
            w.e.kill_tweens_of(Targets::One(TargetId(1000 + (k % 100) as u32)), Some(&[X]));
            w.e.finish(w.nested[(k + 13) % 100]);
            if next_kill < w.roots.len() {
                w.e.anim(w.roots[next_kill]).kill();
                next_kill += 1;
            }
            w.e.anim(w.colours)
                .set_progress((k % 10) as f64 / 10.0, Emit::Fire);
            w.e.anim(w.grid_tl).play();
            w.e.swap_events(&mut buf);
        }
    }
    let during = allocs() - before;
    assert_eq!(during, 0, "allocations on the per-frame path");
    // The loop exercised what it claims to.
    assert!(events > 10_000, "{events} events");
    assert!(
        interrupts > 0 && pauses > 0,
        "{interrupts} interrupts, {pauses} pauses"
    );
    assert!(w.e.stats().live_nodes < 3000);
    assert!(w.e.stats().event_overflows == 0, "{:?}", w.e.stats());
}

#[test]
fn track_compaction_does_not_allocate() {
    let mut e = TweenEngine::new();
    for i in 0..400u32 {
        e.seed(TargetId(i), X, TweenValue::F64(0.0));
    }
    let mut ids = Vec::new();
    for i in 0..400u32 {
        ids.push(e.to(
            TargetId(i).into(),
            &[PropTo::to_f64(X, 1.0)],
            lin(10.0).repeat(-1),
        ));
    }
    let mut buf: Vec<TweenEvent> = Vec::new();
    e.advance(0.01);
    e.swap_events(&mut buf);
    e.advance(0.01);
    e.swap_events(&mut buf);
    let before = allocs();
    for id in ids.iter().take(300) {
        e.anim(*id).kill();
    }
    for _ in 0..10 {
        e.advance(0.01);
        e.swap_events(&mut buf);
    }
    let during = allocs() - before;
    assert_eq!(during, 0, "allocations while killing and compacting");
    let s = e.stats();
    assert_eq!(s.compactions, 1);
    assert_eq!((s.tracks, s.dead_tracks), (100, 0));
    // The survivors still animate.
    e.advance(1.0);
    assert!(e.get_f64(TargetId(399), X).unwrap() > 0.0);
}
