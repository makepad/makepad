//! Adversarial review, lens: performance, zero allocation and soundness.
//!
//! - A counting `#[global_allocator]` (per thread, so parallel tests do not
//!   disturb each other) proves the per-frame path and the controls do not
//!   allocate, with a bigger scene than the spec's 12.12 (10k tweens,
//!   stagger groups, nested repeating yoyo timelines, kill / relink storms,
//!   track compaction, `swap_events`).
//! - Soundness: stale handles, NaN / infinite / negative inputs, zero
//!   durations, time scale 0, huge repeats, empty timelines, deep nesting.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use makepad_tween::*;

// ---------------------------------------------------------------------------
// Counting allocator
// ---------------------------------------------------------------------------

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    // `try_with`: the allocator may run while thread locals are torn down.
    let _ = ARMED.try_with(|a| {
        if a.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note();
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        note();
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        note();
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `f` and answers how many allocations this thread made inside it.
fn allocations<R>(f: impl FnOnce() -> R) -> (u64, R) {
    COUNT.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (COUNT.with(|c| c.get()), r)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const X: PropKey = PropKey(1);
const Y: PropKey = PropKey(2);
const C: PropKey = PropKey(3);

fn tg(i: u32) -> TargetId {
    TargetId(i)
}

fn one(i: u32) -> Targets<'static> {
    Targets::One(TargetId(i))
}

fn to(k: PropKey, v: f64) -> PropTo<'static> {
    PropTo::to_f64(k, v)
}

fn lin(d: f64) -> TweenOpts {
    TweenOpts::new().duration(d).ease(Easing::Linear)
}

fn x(e: &TweenEngine, t: u32) -> f64 {
    e.get_f64(tg(t), X).unwrap_or(f64::NAN)
}

/// The spec 12.12 scene, scaled up: 10k+ tweens in every shape the engine
/// has. Answers the handles the storm plays with.
struct Scene {
    grid_tl: TweenId,
    nested: Vec<TweenId>,
    roots: Vec<TweenId>,
    colour_tl: TweenId,
}

fn build_scene(e: &mut TweenEngine) -> Scene {
    // 200-target staggered grid (20 x 10, two props), events on the group
    // and the timeline, three calls, watched labels, one add_pause.
    for i in 0..200 {
        e.seed(tg(i), X, TweenValue::F64(0.0));
        e.seed(tg(i), Y, TweenValue::F64(0.0));
    }
    let grid_tl = e.timeline(
        TimelineOpts::new()
            .repeat(-1)
            .yoyo(true)
            .watch_labels()
            .events(EventMask::ALL),
    );
    {
        let mut tl = e.tl(grid_tl);
        tl.add_label(Tag(1), 0.0)
            .to(
                Targets::Range {
                    first: 0,
                    count: 200,
                },
                &[to(X, 100.0), to(Y, 50.0)],
                TweenOpts::new()
                    .duration(0.4)
                    .ease(Easing::OutCubic)
                    .stagger(Stagger::each(0.01).grid(20, 10).from(StaggerFrom::Center))
                    .events(EventMask::EDGES),
                0.0,
            )
            .add_label(Tag(2), Position::rel(0.0))
            .call(Tag(10), 0.1)
            .call(Tag(11), 0.5)
            .call(Tag(12), 1.0)
            .add_pause(1.9, Tag(13))
            .add_label(Tag(3), 2.0);
    }

    // 100 repeating yoyo timelines nested 2 deep.
    let mut nested = Vec::new();
    for i in 0..100u32 {
        let t = 1000 + i;
        e.seed(tg(t), X, TweenValue::F64(0.0));
        let outer = e.timeline(
            TimelineOpts::new()
                .repeat(-1)
                .yoyo(true)
                .events(EventMask::EDGES),
        );
        let inner = e.timeline(
            TimelineOpts::new()
                .repeat(3)
                .yoyo(true)
                .events(EventMask::EDGES),
        );
        e.tl(inner)
            .to(
                one(t),
                &[to(X, 1.0)],
                lin(0.05).events(EventMask::EDGES),
                0.0,
            )
            .to(one(t), &[to(X, 0.0)], lin(0.05), Position::rel(0.0));
        e.tl(outer).add(inner, 0.0);
        nested.push(outer);
    }

    // 1000 root tweens with overwrite Auto (they fight over 100 targets).
    let mut roots = Vec::new();
    for i in 0..1000u32 {
        let t = 2000 + i % 100;
        e.seed(tg(t), X, TweenValue::F64(0.0));
        roots.push(
            e.to(
                one(t),
                &[to(X, i as f64)],
                TweenOpts::new()
                    .duration(0.5 + (i % 7) as f64 * 0.1)
                    .overwrite(Overwrite::Auto)
                    .events(EventMask::EDGES),
            ),
        );
    }

    // 9000 plain repeating root tweens: 10k tweens in total.
    for i in 0..9000u32 {
        let t = 10_000 + i;
        e.seed(tg(t), X, TweenValue::F64(0.0));
        roots.push(
            e.to(
                one(t),
                &[to(X, 1.0)],
                TweenOpts::new()
                    .duration(0.3 + (i % 11) as f64 * 0.05)
                    .repeat(-1)
                    .yoyo(true)
                    .ease(Easing::OutElastic),
            ),
        );
    }

    // A 20-colour OKLCH timeline.
    let colour_tl = e.timeline(TimelineOpts::new().repeat(-1).yoyo(true));
    for i in 0..20u32 {
        let t = 30_000 + i;
        e.seed(tg(t), C, TweenValue::Color(Rgba::new(1.0, 0.0, 0.0, 1.0)));
        e.tl(colour_tl).to(
            one(t),
            &[PropTo::to(
                C,
                TweenValue::Color(Rgba::new(0.0, 0.2, 1.0, 0.5)),
            )],
            lin(0.25).color_space(ColorSpace::Oklch),
            Position::rel(-0.1),
        );
    }
    Scene {
        grid_tl,
        nested,
        roots,
        colour_tl,
    }
}

// ---------------------------------------------------------------------------
// Zero allocation
// ---------------------------------------------------------------------------

/// Spec 6.3 / 12.12, bigger: advance, swap_events, every control, kills,
/// kill_tweens_of, finish, relinks (start time moves) and reaping. The
/// buffers are warm, so the whole measured loop must allocate nothing.
#[test]
fn storm_of_frames_and_controls_allocates_nothing() {
    let mut e = TweenEngine::new();
    let s = build_scene(&mut e);
    let mut buf: Vec<TweenEvent> = Vec::new();
    for _ in 0..3 {
        e.advance(1.0 / 120.0);
        e.swap_events(&mut buf);
        e.clear_changes();
    }
    let before = e.stats();
    let (n, seen) = allocations(|| {
        let mut seen = 0usize;
        for f in 0..10_000u32 {
            let dt = if f % 13 == 0 { 0.3 } else { 1.0 / 120.0 };
            e.advance(dt);
            e.swap_events(&mut buf);
            seen += buf.len();
            seen += e.changes().len();
            e.clear_changes();
            if f % 97 == 0 {
                let k = (f / 97) as usize;
                let r = s.roots[k % s.roots.len()];
                e.anim(s.grid_tl).seek(Seek::Time(0.7), Emit::Fire);
                e.anim(s.grid_tl).resume();
                e.anim(s.nested[k % 100]).reverse();
                e.anim(s.nested[(k + 1) % 100]).play();
                e.anim(s.colour_tl)
                    .set_time_scale(if k % 2 == 0 { 2.0 } else { -0.5 });
                e.anim(r).pause();
                e.anim(r).resume();
                e.anim(r).set_progress(0.3, Emit::Fire);
                e.anim(r).set_start_time(k as f64 * 0.01);
                e.kill_tweens_of(one(2000 + (k % 100) as u32), Some(&[X]));
                e.finish(s.roots[(k + 7) % s.roots.len()]);
                e.anim(s.roots[(k + 13) % s.roots.len()]).kill();
                e.anim(s.grid_tl).set_paused(false);
            }
        }
        seen
    });
    let after = e.stats();
    assert!(seen > 0, "the storm produced no events or changes");
    assert_eq!(
        after.event_overflows, before.event_overflows,
        "the event reserve was too small although the queue was drained every frame"
    );
    assert_eq!(n, 0, "{n} allocations in the per-frame / control path");
}

/// Kills outnumber live tracks, so `advance` compacts the track arrays; the
/// compaction and the frames around it must not allocate either.
#[test]
fn compaction_inside_advance_allocates_nothing() {
    let mut e = TweenEngine::new();
    let mut ids = Vec::new();
    for i in 0..4000u32 {
        e.seed(tg(i), X, TweenValue::F64(0.0));
        e.seed(tg(i), Y, TweenValue::F64(0.0));
        ids.push(e.to(
            one(i),
            &[to(X, 1.0), to(Y, 2.0)],
            TweenOpts::new().duration(10.0).repeat(-1),
        ));
    }
    let mut buf = Vec::new();
    e.advance(0.01);
    e.swap_events(&mut buf);
    e.advance(0.01);
    e.swap_events(&mut buf);
    let mut y0 = None;
    let (n, ()) = allocations(|| {
        for (k, id) in ids.iter().enumerate() {
            if k % 4 != 0 {
                e.anim(*id).kill();
            }
            if k % 500 == 0 {
                e.advance(1.0 / 120.0);
                e.swap_events(&mut buf);
                e.clear_changes();
            }
        }
        // Per-property kills leave dead tracks inside live tweens too.
        e.kill_tweens_of(
            Targets::Range {
                first: 0,
                count: 4000,
            },
            Some(&[Y]),
        );
        y0 = e.get_f64(tg(0), Y);
        for _ in 0..4 {
            e.advance(1.0 / 120.0);
            e.swap_events(&mut buf);
            e.clear_changes();
        }
    });
    let st = e.stats();
    assert!(st.compactions > 0, "no compaction happened: {st:?}");
    assert_eq!(n, 0, "{n} allocations around track compaction");
    // The survivors still animate X, and Y no longer moves.
    let v = x(&e, 0);
    assert!(v > 0.0 && v < 1.0, "survivor 0 x = {v}");
    assert_eq!(e.get_f64(tg(0), Y), y0);
    assert!(
        st.tracks <= 1000 + 64 + 1,
        "compaction left {} tracks",
        st.tracks
    );
}

/// Every getter, `mark_all_changed`, `clear_events`, `finish_all`,
/// `kill_all` and the leaf helpers are allocation-free too (spec 6.3).
#[test]
fn getters_and_leaf_helpers_allocate_nothing() {
    let mut e = TweenEngine::new();
    let s = build_scene(&mut e);
    let mut buf = Vec::new();
    e.advance(0.05);
    e.swap_events(&mut buf);
    let mut out = [0.0f64; 64];
    let (n, sum) = allocations(|| {
        let mut sum = 0.0;
        for id in s.roots.iter().take(500) {
            let a = e.anim_ref(*id);
            sum += a.time() + a.total_time() + a.progress() + a.total_progress();
            sum += a.duration() + a.total_duration() + a.time_scale() + a.start_time();
            sum += a.end_time(true) + a.delay() + a.global_time(0.1);
            sum += a.iteration() as f64 + a.repeat() as f64 + a.child_count() as f64;
            sum += (a.is_alive() as u8 + a.is_active() as u8 + a.paused() as u8) as f64;
        }
        let g = e.anim_ref(s.grid_tl);
        sum += g.current_label().map_or(0.0, |t| t.0 as f64);
        sum += g.label_time(Tag(2)).unwrap_or(0.0);
        sum += e.is_tweening(tg(5)) as u8 as f64;
        sum += e.is_active() as u8 as f64;
        sum += e.get_by_tag(Tag(999)).is_some() as u8 as f64;
        e.mark_all_changed();
        sum += e.changes().len() as f64;
        e.clear_changes();
        e.clear_events();
        let mut q = QuickTo::at([0.0f64; 4]);
        q.aim(
            [1.0; 4],
            0.3,
            Easing::OutBack,
            Retime::ByDistance { per_unit: 1.0 },
        );
        while q.step(1.0 / 60.0) {}
        sum += q.value()[0];
        Stagger::each(0.1)
            .grid(8, 8)
            .from(StaggerFrom::Random(7))
            .ease(Easing::InOutSine)
            .delays(64, &mut out);
        sum += out[63] + Stagger::amount(1.0).from(StaggerFrom::Edges).delay(3, 64);
        sum += Easing::Elastic {
            dir: EaseDir::InOut,
            amplitude: 1.0,
            period: 0.45,
        }
        .map(0.3);
        e.finish_all();
        e.swap_events(&mut buf);
        e.kill_all();
        e.swap_events(&mut buf);
        sum
    });
    assert!(sum.is_finite());
    assert_eq!(n, 0, "{n} allocations in getters / leaf helpers");
}

// ---------------------------------------------------------------------------
// Stale handles
// ---------------------------------------------------------------------------

/// A killed tween's handle is stale: every control is a no-op, every getter
/// answers its default, and it never reaches the node that reuses its slot.
#[test]
fn stale_handles_never_touch_the_reused_node() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    e.seed(tg(2), X, TweenValue::F64(0.0));
    let old = e.to(one(1), &[to(X, 10.0)], lin(1.0));
    e.advance(0.5);
    e.anim(old).kill();
    let fresh = e.to(one(2), &[to(X, 10.0)], lin(1.0));
    let tl_stale = e.timeline(TimelineOpts::new());
    e.anim(tl_stale).kill();
    e.advance(0.25);
    let before = x(&e, 2);
    let mut buf = Vec::new();
    e.swap_events(&mut buf);

    {
        let mut a = e.anim(old);
        a.play()
            .pause()
            .resume()
            .reverse()
            .restart(true, Emit::Fire)
            .seek(Seek::Time(0.9), Emit::Fire)
            .set_progress(1.0, Emit::Fire)
            .set_total_progress(1.0, Emit::Fire)
            .set_time(0.9, Emit::Fire)
            .set_total_time(0.9, Emit::Fire)
            .set_iteration(3, Emit::Fire)
            .set_time_scale(5.0)
            .set_reversed(true)
            .set_paused(true)
            .set_duration(9.0)
            .set_delay(3.0)
            .set_start_time(4.0)
            .set_repeat(-1)
            .set_repeat_delay(1.0)
            .set_yoyo(true)
            .invalidate()
            .play_from(Seek::Progress(0.5), Emit::Fire)
            .pause_at(Seek::Time(0.1), Emit::Fire)
            .reverse_from(None, Emit::Fire);
    }
    e.anim(old).kill();
    e.finish(old);
    e.tl(tl_stale)
        .to(one(2), &[to(X, -5.0)], lin(1.0), 0.0)
        .add(fresh, 0.0)
        .add_label(Tag(1), 0.0)
        .call(Tag(2), 0.0)
        .add_pause(0.0, Tag(3))
        .remove(fresh)
        .clear(true)
        .shift_children(1.0, true, 0.0);
    e.tl(fresh).to(one(2), &[to(X, -5.0)], lin(1.0), 0.0);

    let a = e.anim_ref(old);
    assert!(!a.is_alive() && !a.is_active() && !a.paused() && !a.reversed());
    assert_eq!(
        (
            a.time(),
            a.total_time(),
            a.progress(),
            a.duration(),
            a.delay()
        ),
        (0.0, 0.0, 0.0, 0.0, 0.0)
    );
    assert_eq!((a.repeat(), a.child_count(), a.tag()), (0, 0, Tag::NONE));
    assert!(e.sample(old, e.slot(tg(1), X).unwrap(), 0.5).is_none());
    assert_eq!(e.events().len(), 0, "a stale handle produced events");
    assert_eq!(x(&e, 2), before, "a stale handle moved the live tween");
    assert!(e.anim_ref(fresh).is_alive());
    assert_eq!(e.anim_ref(fresh).time_scale(), 1.0);
    e.advance(0.25);
    assert!((x(&e, 2) - 5.0).abs() < 1e-9, "x = {}", x(&e, 2));
}

/// Out-of-range slot handles must not panic in any value-store getter.
#[test]
fn out_of_range_slot_ids_do_not_panic() {
    let e = TweenEngine::new();
    let bad = SlotId(7);
    assert_eq!(e.value(bad), TweenValue::F64(0.0));
    assert!(!e.is_changed(bad));
    let r = std::panic::catch_unwind(|| e.slot_key(bad));
    assert!(
        r.is_ok(),
        "slot_key panics on an unknown SlotId (value() does not)"
    );
}

// ---------------------------------------------------------------------------
// Bad numbers
// ---------------------------------------------------------------------------

/// dt NaN / infinite / negative counts as 0 and leaves everything finite.
#[test]
fn nan_inf_negative_dt_are_zero() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    let id = e.to(one(1), &[to(X, 10.0)], lin(1.0));
    e.advance(0.25);
    for dt in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, -0.0, 0.0] {
        e.advance(dt);
        assert_eq!(x(&e, 1), 2.5, "dt {dt}");
        assert_eq!(e.anim_ref(id).total_time(), 0.25, "dt {dt}");
    }
    e.advance(0.75);
    assert_eq!(x(&e, 1), 10.0);
}

/// A NaN root time scale is a bad input, but it must not poison the root
/// clock for good: setting it back to 1 has to bring the engine back.
#[test]
fn nan_root_time_scale_is_recoverable() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    e.to(one(1), &[to(X, 10.0)], lin(1.0));
    e.advance(0.25);
    e.set_root_time_scale(f64::NAN);
    e.advance(0.1);
    e.set_root_time_scale(1.0);
    e.advance(0.25);
    let v = x(&e, 1);
    assert!(
        v.is_finite() && v > 2.5,
        "after a NaN root time scale and a reset to 1, x = {v} (the root clock stays NaN)"
    );
}

/// One tween built with a NaN delay (for example from a 0/0 layout
/// computation) must not freeze every tween added after it: the forward
/// child walk breaks at the first unstarted child, and a NaN start sorts
/// last and never counts as started.
#[test]
fn nan_delay_does_not_block_later_siblings() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    e.seed(tg(2), X, TweenValue::F64(0.0));
    e.to(one(1), &[to(X, 10.0)], lin(1.0).delay(f64::NAN));
    e.to(one(2), &[to(X, 10.0)], lin(1.0));
    e.advance(0.5);
    assert_eq!(x(&e, 2), 5.0, "a NaN-delay sibling blocked a healthy tween");
}

/// Same through a control: `set_start_time(NaN)` on one child.
#[test]
fn nan_start_time_does_not_block_later_siblings() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    e.seed(tg(2), X, TweenValue::F64(0.0));
    let a = e.to(one(1), &[to(X, 10.0)], lin(1.0));
    e.anim(a).set_start_time(f64::NAN);
    e.to(one(2), &[to(X, 10.0)], lin(1.0));
    e.advance(0.5);
    assert_eq!(x(&e, 2), 5.0, "a NaN-start sibling blocked a healthy tween");
}

/// NaN durations, time scales, progress and seeks must leave the slot store
/// finite (garbage in may be ignored, but not spread to the host's widgets).
#[test]
fn nan_controls_do_not_write_nan_values() {
    let mut e = TweenEngine::new();
    for i in 0..6 {
        e.seed(tg(i), X, TweenValue::F64(0.0));
    }
    let a = e.to(one(0), &[to(X, 1.0)], lin(f64::NAN));
    let b = e.to(one(1), &[to(X, 1.0)], lin(1.0).time_scale(f64::NAN));
    let c = e.to(one(2), &[to(X, 1.0)], lin(1.0));
    let d = e.to(one(3), &[to(X, 1.0)], lin(1.0));
    let f = e.to(one(4), &[to(X, 1.0)], lin(1.0));
    let g = e.to(
        one(5),
        &[to(X, 1.0)],
        lin(1.0).repeat_delay(f64::NAN).repeat(2),
    );
    e.advance(0.1);
    e.anim(c).set_progress(f64::NAN, Emit::Fire);
    e.anim(d).seek(Seek::Time(f64::NAN), Emit::Fire);
    e.anim(f).set_duration(f64::NAN);
    e.advance(0.1);
    for (i, id) in [a, b, c, d, f, g].iter().enumerate() {
        let v = x(&e, i as u32);
        assert!(v.is_finite(), "target {i}: x = {v}");
        let r = e.anim_ref(*id);
        let _ = (r.time(), r.progress(), r.total_duration());
    }
}

/// `time_scale(0)` in the options: nothing moves, nothing is NaN, the engine
/// reports itself idle (GSAP: timeScale 0 stops the playhead).
#[test]
fn time_scale_zero_is_inert_and_idle() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    let id = e.to(one(1), &[to(X, 1.0)], lin(1.0).time_scale(0.0));
    for _ in 0..10 {
        e.advance(0.1);
    }
    assert_eq!(x(&e, 1), 0.0);
    assert!(e.anim_ref(id).end_time(true).is_finite());
    assert!(
        !e.is_active(),
        "a time-scale-0 tween keeps the host ticking"
    );
    e.anim(id).set_time_scale(1.0);
    e.advance(0.5);
    assert!((x(&e, 1) - 0.5).abs() < 1e-9, "x = {}", x(&e, 1));
}

/// Zero-duration storm: 5000 `set`s on one timeline, scrubbed back and
/// forth, stay exact and allocation-free.
#[test]
fn zero_duration_storm() {
    let mut e = TweenEngine::new();
    for i in 0..5000u32 {
        e.seed(tg(i), X, TweenValue::F64(0.0));
    }
    let tl = e.timeline(TimelineOpts::new().paused(true));
    for i in 0..5000u32 {
        e.tl(tl)
            .set(one(i), &[to(X, 1.0)], TweenOpts::new(), i as f64 * 0.001);
    }
    let mut buf = Vec::new();
    e.swap_events(&mut buf);
    let (n, ()) = allocations(|| {
        for k in 0..20 {
            let t = if k % 2 == 0 { 5.0 } else { 0.0 };
            e.anim(tl).seek(Seek::Time(t), Emit::Fire);
            e.swap_events(&mut buf);
        }
        e.anim(tl).seek(Seek::Time(2.5), Emit::Fire);
    });
    assert_eq!(n, 0);
    assert_eq!(x(&e, 2499), 1.0);
    assert_eq!(x(&e, 2501), 0.0);
}

/// A repeat-forever tween sought far ahead: the 1-based iteration must not
/// overflow (`animation_cycle` saturates at u32::MAX, then `+ 1`).
#[test]
fn huge_iteration_counts_do_not_overflow() {
    let mut e = TweenEngine::new();
    e.seed(tg(1), X, TweenValue::F64(0.0));
    let id = e.to(
        one(1),
        &[to(X, 1.0)],
        lin(0.001).repeat(-1).yoyo(true).events(EventMask::ALL),
    );
    e.advance(0.0005);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        e.anim(id).seek(Seek::Time(1e7 + 0.0005), Emit::Fire);
        e.anim_ref(id).iteration()
    }));
    assert!(
        r.is_ok(),
        "iteration() / event iteration overflowed and panicked"
    );
}

/// Empty timelines, repeating forever with and without a repeat delay:
/// no panic, finite state, and no unbounded event stream.
#[test]
fn empty_repeating_timelines_are_quiet() {
    let mut e = TweenEngine::new();
    let a = e.timeline(TimelineOpts::new().repeat(-1).events(EventMask::ALL));
    let b = e.timeline(
        TimelineOpts::new()
            .repeat(-1)
            .repeat_delay(0.1)
            .yoyo(true)
            .events(EventMask::ALL),
    );
    let c = e.timeline(TimelineOpts::new().repeat(5).events(EventMask::ALL));
    let mut buf = Vec::new();
    let mut total = 0;
    for _ in 0..120 {
        e.advance(1.0 / 60.0);
        e.swap_events(&mut buf);
        total += buf.len();
    }
    for id in [a, b, c] {
        let r = e.anim_ref(id);
        assert!(r.time().is_finite() && r.total_time().is_finite());
        let _ = r.iteration();
    }
    // b repeats every 0.1 s for 2 s: about 20 repeats plus a few edges.
    assert!(
        total < 200,
        "{total} events from three empty timelines in 2 s"
    );
}

/// Timelines nested 64 deep (far beyond real use) render, seek, reverse and
/// die without overflowing the stack of a test thread (2 MiB).
#[test]
fn deep_nesting_is_fine() {
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let mut e = TweenEngine::new();
            e.seed(tg(1), X, TweenValue::F64(0.0));
            let top = e.timeline(TimelineOpts::new().repeat(1).yoyo(true));
            let mut cur = top;
            for _ in 0..63 {
                let t = e.timeline(TimelineOpts::new());
                e.tl(cur).add(t, 0.0);
                cur = t;
            }
            e.tl(cur).to(one(1), &[to(X, 1.0)], lin(1.0), 0.0);
            e.advance(0.5);
            let mid = x(&e, 1);
            e.advance(1.0);
            e.anim(top).reverse();
            e.advance(0.25);
            e.anim(top).seek(Seek::Time(0.75), Emit::Fire);
            let v = x(&e, 1);
            e.anim(top).kill();
            e.advance(0.1);
            (mid, v)
        })
        .unwrap();
    let (mid, v) = h.join().expect("deep nesting overflowed the stack");
    assert!((mid - 0.5).abs() < 1e-9, "mid = {mid}");
    assert!((v - 0.75).abs() < 1e-9, "v = {v}");
}
