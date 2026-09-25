//! Playback control (design 12.6) and the engine API around it, with the
//! GSAP 3.15 control traces (tests/golden/callbacks.rs).

mod util;

#[path = "golden"]
mod golden {
    pub mod callbacks;
    pub mod common;
}

use golden::callbacks::{Trace, TRACES};
use golden::common::{num, text};
use makepad_tween::*;
use util::*;

fn trace(id: &str) -> &'static Trace {
    TRACES.iter().find(|t| t.id == id).unwrap()
}

#[test]
fn golden_restart_fires_start_again() {
    // The golden's R (a smooth paused timeline stepped relatively) is the
    // engine root. GSAP's R never auto-removes, so the tween is kept.
    let tr = trace("design_restart_reverse_controls");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(
        one(0),
        &[to(X, 100.0)],
        lin(1.0).delay(0.5).keep(true).events(CBS),
    );
    names.name(t, "t");
    for op in &tr.ops[..6] {
        match op.op {
            "R +2.0" => e.advance(2.0),
            "R +0.3" => e.advance(0.3),
            "t.restart()" => {
                e.anim(t).restart(false, Emit::Suppress);
            }
            "t.restart(true)" => {
                e.anim(t).restart(true, Emit::Suppress);
            }
            other => panic!("{other}"),
        }
        assert_eq!(names.drain(&mut e).0, op.fired_all, "{}", op.op);
        close(val(&e, 0, X), num(op.values, "x"), VAL_TOL, op.op);
        close(
            e.anim_ref(t).total_time(),
            num(op.values, "t_total_time"),
            1e-9,
            op.op,
        );
    }
}

#[test]
fn golden_reverse_sets_reversed_and_unpauses() {
    // GSAP reverse() is reversed(true).paused(false): not a toggle.
    let tr = trace("design_restart_reverse_controls");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let u = e.to(one(0), &[to(X, 100.0)], lin(1.0).keep(true).events(CBS));
    names.name(u, "u");
    for op in &tr.ops[6..] {
        match op.op {
            "R +0.6" => e.advance(0.6),
            "R +0.1" => e.advance(0.1),
            "u.reverse()" => {
                e.anim(u).reverse();
            }
            "u.pause()" => {
                e.anim(u).pause();
            }
            "u.resume()" => {
                e.anim(u).resume();
            }
            "u.play()" => {
                e.anim(u).play();
            }
            other => panic!("{other}"),
        }
        assert_eq!(names.drain(&mut e).0, op.fired_all, "{}", op.op);
        close(val(&e, 0, X), num(op.values, "x"), VAL_TOL, op.op);
        let r = e.anim_ref(u);
        close(r.total_time(), num(op.values, "u_total_time"), 1e-9, op.op);
        assert_eq!(r.reversed(), num(op.values, "reversed") == 1.0, "{}", op.op);
        assert_eq!(r.paused(), num(op.values, "paused") == 1.0, "{}", op.op);
    }
}

#[test]
fn golden_kill_emits_interrupt_only_below_progress_1() {
    let tr = trace("design_kill_interrupt");
    for (op, at) in tr.ops.iter().zip([Some(0.5), Some(1.0), None]) {
        let mut e = TweenEngine::new();
        let mut names = Names::new();
        let t = e.to(
            one(0),
            &[to(X, 1.0)],
            lin(1.0)
                .paused(true)
                .events(EventMask::INTERRUPT.with(EventMask::COMPLETE)),
        );
        names.name(t, op.op.split(' ').next().unwrap());
        if let Some(at) = at {
            e.anim(t).set_total_time(at, Emit::Fire);
        }
        names.drain(&mut e);
        e.anim(t).kill();
        assert_eq!(names.drain(&mut e).0, op.fired_all, "{}", op.op);
        assert!(!e.anim_ref(t).is_alive());
    }
}

#[test]
fn golden_progress_setter_is_yoyo_aware() {
    let tr = trace("design_progress_time_iteration_setters");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(
        one(0),
        &[to(X, 100.0)],
        lin(1.0).repeat(1).yoyo(true).paused(true).events(CBS),
    );
    names.name(t, "t");
    e.anim(t).set_total_time(1.5, Emit::Suppress);
    e.anim(t).set_progress(0.25, Emit::Fire);
    let op = &tr.ops[0];
    assert_eq!(names.drain(&mut e).0, op.fired_all);
    let s = op.state.unwrap();
    assert_eq!(e.anim_ref(t).total_time(), s.total_time);
    assert_eq!(e.anim_ref(t).progress(), 0.25);
    close(val(&e, 0, X), num(op.values, "x"), VAL_TOL, op.op);
}

#[test]
fn golden_set_time_deviate_and_set_iteration() {
    // GSAP 3.15's time(v) lands in the first iteration; the engine stays in
    // the current one (deviation). iteration(k) agrees.
    let tr = trace("design_progress_time_iteration_setters");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(
        one(0),
        &[to(X, 100.0)],
        lin(1.0).repeat(2).paused(true).events(CBS),
    );
    names.name(t, "t");
    e.anim(t).set_total_time(1.5, Emit::Suppress);
    e.anim(t).set_time(0.25, Emit::Fire);
    assert_eq!(tr.ops[1].state.unwrap().total_time, 0.25, "GSAP");
    assert_eq!(
        e.anim_ref(t).total_time(),
        1.25,
        "engine: current iteration"
    );
    assert_eq!(names.drain(&mut e).0, ["t:onUpdate"]);
    close(val(&e, 0, X), 25.0, 1e-9, "x");
    e.anim(t).set_iteration(3, Emit::Fire);
    let op = &tr.ops[2];
    assert_eq!(names.drain(&mut e).0, op.fired_all);
    assert_eq!(e.anim_ref(t).total_time(), op.state.unwrap().total_time);
    assert_eq!(e.anim_ref(t).iteration(), 3);
    e.anim(t).set_time(1.0, Emit::Fire);
    assert_eq!(
        e.anim_ref(t).total_time(),
        3.0,
        "end of iteration 3 = the end"
    );
    // repeat 2, rd .5, at 2 (iteration 2): time(0.25) -> 1.75 (GSAP 0.25).
    let mut e = TweenEngine::new();
    let t = e.to(
        one(0),
        &[to(X, 100.0)],
        lin(1.0).repeat(2).repeat_delay(0.5).paused(true),
    );
    e.anim(t).set_total_time(2.0, Emit::Suppress);
    e.anim(t).set_time(0.25, Emit::Fire);
    assert_eq!(tr.ops[4].state.unwrap().total_time, 0.25, "GSAP");
    assert_eq!(e.anim_ref(t).total_time(), 1.75);
    assert_eq!(e.anim_ref(t).iteration(), 2);
}

#[test]
fn golden_labels_current_next_previous() {
    let tr = trace("design_labels_directions");
    let tag = |s: Option<&str>| s.map(|n| Tag(n.as_bytes()[0] as u64));
    let mut e = TweenEngine::new();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(3.0), 0.0)
        .add_label(Tag(b'a' as u64), 0.0)
        .add_label(Tag(b'b' as u64), 1.0)
        .add_label(Tag(b'c' as u64), 1.0)
        .add_label(Tag(b'd' as u64), 2.0);
    for op in tr.ops {
        let (_, nums, _) = parse_op(op.op);
        e.anim(tl).seek(Seek::Time(nums[0]), Emit::Suppress);
        let r = e.anim_ref(tl);
        assert_eq!(
            r.current_label(),
            tag(text(op.values, "current")),
            "{}",
            op.op
        );
        assert_eq!(r.next_label(), tag(text(op.values, "next")), "{}", op.op);
        assert_eq!(
            r.previous_label(),
            tag(text(op.values, "previous")),
            "{}",
            op.op
        );
    }
    assert_eq!(e.anim_ref(tl).label_time(Tag(b'd' as u64)), Some(2.0));
    e.anim(tl)
        .seek(Seek::Label(Tag(b'b' as u64), 0.5), Emit::Suppress);
    assert_eq!(e.anim_ref(tl).time(), 1.5);
    e.anim(tl).seek(Seek::Label(Tag(9), 0.0), Emit::Suppress);
    assert_eq!(e.anim_ref(tl).time(), 1.5, "an unknown label is a no-op");
}

#[test]
fn reverse_is_not_a_toggle_by_frames() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).keep(true).events(CBS));
    names.name(t, "t");
    e.advance(0.6);
    e.anim(t).reverse();
    e.advance(0.1);
    close(val(&e, 0, X), 50.0, 1e-9, "reversed");
    e.anim(t).pause().resume();
    assert!(e.anim_ref(t).reversed(), "resume keeps the direction");
    e.advance(0.1);
    close(val(&e, 0, X), 40.0, 1e-9, "still reversed");
    e.anim(t).reverse();
    assert!(e.anim_ref(t).reversed(), "reverse() again stays reversed");
    names.drain(&mut e);
    e.advance(1.0);
    assert_eq!(names.drain(&mut e).1, ["t:onReverseComplete"]);
    assert_eq!(val(&e, 0, X), 0.0);
    e.anim(t).play();
    assert!(!e.anim_ref(t).reversed(), "play un-reverses");
    e.advance(0.1);
    close(val(&e, 0, X), 10.0, 1e-9, "forward again");
    // Toggling is set_reversed(!reversed).
    let r = e.anim_ref(t).reversed();
    e.anim(t).set_reversed(!r);
    assert!(e.anim_ref(t).reversed());
}

#[test]
fn negative_time_scale_reverses() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0));
    e.advance(0.5);
    e.anim(t).set_time_scale(-2.0);
    assert!(e.anim_ref(t).reversed());
    e.advance(0.1);
    close(val(&e, 0, X), 30.0, 1e-9, "backward at 2x");
}

#[test]
fn completed_root_timeline_restart_relinks_it() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl)
        .to(one(0), &[to(X, 100.0)], lin(1.0), Position::END);
    e.advance(1.5);
    assert!(!e.is_active());
    assert!(e.anim_ref(tl).is_alive(), "timelines keep by default");
    e.anim(tl).restart(false, Emit::Suppress);
    assert!(e.is_active());
    e.advance(0.25);
    close(val(&e, 0, X), 25.0, 1e-9, "plays again");
}

#[test]
fn completed_bare_tween_is_freed() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).tag(Tag(5)));
    assert_eq!(e.get_by_tag(Tag(5)), Some(t));
    let before = e.stats().live_nodes;
    e.advance(2.0);
    assert!(!e.anim_ref(t).is_alive());
    assert_eq!(e.stats().live_nodes, before - 1);
    assert_eq!(e.get_by_tag(Tag(5)), None);
    // Controls on a stale handle do nothing; getters answer defaults.
    e.anim(t)
        .seek(Seek::Time(0.5), Emit::Fire)
        .play()
        .set_time_scale(3.0);
    assert_eq!(val(&e, 0, X), 100.0);
    let r = e.anim_ref(t);
    assert_eq!(
        (r.time(), r.duration(), r.iteration(), r.tag()),
        (0.0, 0.0, 0, Tag::NONE)
    );
    // The slot is reused by the next tween without growth.
    let t2 = e.to(one(0), &[to(X, 0.0)], lin(1.0));
    assert!(e.anim_ref(t2).is_alive());
    assert_eq!(e.stats().nodes, 2);
}

#[test]
fn invalidate_and_repeat_refresh_recapture() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).paused(true));
    e.anim(t).set_total_time(0.5, Emit::Fire);
    assert_eq!(val(&e, 0, X), 50.0);
    e.seed(tg(0), X, TweenValue::F64(80.0));
    e.anim(t).invalidate().set_total_time(0.5, Emit::Fire);
    close(val(&e, 0, X), 90.0, 1e-9, "re-captured from 80");
    // repeat_refresh with a relative end accumulates.
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    e.to(
        one(0),
        &[PropTo::by(X, 10.0.into())],
        lin(1.0).repeat(3).repeat_refresh(true),
    );
    for _ in 0..500 {
        e.advance(0.01);
    }
    assert_eq!(val(&e, 0, X), 40.0);
}

#[test]
fn finish_policies() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 4, X, 0.0);
    // JumpToEnd, finite: one render to the end, children complete, calls fire.
    let tl = e.timeline(TimelineOpts::new().events(CBS));
    e.tl(tl)
        .to(one(0), &[to(X, 100.0)], lin(1.0).events(CBS), Position::END)
        .call(CALL_TAG, 0.5)
        .add_pause(0.75, Tag::NONE);
    let a = e.tl(tl).last();
    let _ = a;
    names.name(tl, "tl");
    names.tag(CALL_TAG, "call");
    e.advance(0.1);
    names.drain(&mut e);
    e.finish(tl);
    let (_, fired) = names.drain(&mut e);
    assert_eq!(fired.last().map(String::as_str), Some("tl:onComplete"));
    assert!(fired.iter().any(|s| s == "call:mark"));
    assert_eq!(val(&e, 0, X), 100.0);
    assert!(
        !e.anim_ref(tl).paused(),
        "add_pause is ignored when finishing"
    );
    // Infinite: end of the first iteration, then paused.
    let inf = e.to(one(1), &[to(X, 100.0)], lin(1.0).repeat(-1));
    e.advance(0.3);
    e.finish(inf);
    assert_eq!(val(&e, 1, X), 100.0);
    assert!(e.anim_ref(inf).paused());
    // Freeze: back to the start and killed, no Interrupt.
    let fr = e.to(
        one(2),
        &[to(X, 100.0)],
        lin(1.0).reduce(Reduce::Freeze).events(EventMask::INTERRUPT),
    );
    names.name(fr, "fr");
    e.advance(0.5);
    names.drain(&mut e);
    e.finish(fr);
    assert_eq!(val(&e, 2, X), 0.0);
    assert!(!e.anim_ref(fr).is_alive());
    assert!(names.drain(&mut e).1.is_empty());
    // Keep: untouched.
    let kp = e.to(one(3), &[to(X, 100.0)], lin(1.0).reduce(Reduce::Keep));
    e.advance(0.5);
    e.finish(kp);
    let kept = val(&e, 3, X);
    close(kept, 50.0, 1e-9, "Keep");
    e.finish_all();
    assert_eq!(val(&e, 3, X), kept);
}

const CALL_TAG: Tag = Tag(77);

#[test]
fn delayed_call_reports_its_tag() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.delayed_call(0.5, Tag(3));
    e.advance(0.4);
    assert!(names.raw(&mut e).is_empty());
    e.advance(0.2);
    let ev = names.raw(&mut e);
    assert_eq!(ev.len(), 1);
    assert_eq!(
        (ev[0].kind, ev[0].tag),
        (EventKind::Call { forward: true }, Tag(3))
    );
    assert!(!e.is_active());
}

#[test]
fn value_store_changes_and_seeding() {
    let mut e = TweenEngine::with_capacity(8, 8, 8);
    let s = e.seed(tg(1), X, TweenValue::F64(2.0));
    assert_eq!(e.slot(tg(1), X), Some(s));
    assert_eq!(e.slot_key(s), (tg(1), X));
    assert_eq!(e.changes(), &[s]);
    assert!(e.is_changed(s));
    e.clear_changes();
    assert!(e.changes().is_empty() && !e.is_changed(s));
    let g = e.slot_generation();
    e.to(one(1), &[to(X, 4.0), to(Y, 1.0)], lin(1.0));
    assert!(e.slot_generation() != g, "a slot was created for Y");
    e.advance(0.5);
    assert_eq!(e.get_f64(tg(1), X), Some(3.0));
    // Y was never seeded: it does not animate, it is set to its end.
    assert_eq!(e.get_f64(tg(1), Y), Some(1.0));
    assert_eq!(e.changes().len(), 2);
    assert_eq!(e.stats().unseeded_starts, 1);
    e.clear_changes();
    e.mark_all_changed();
    assert_eq!(e.changes().len(), 2);
    assert_eq!(e.slot_count(), 2);
    // sample() is pure.
    let t = e.to(one(1), &[to(X, 10.0)], lin(2.0).paused(true));
    e.anim(t).set_total_time(0.0, Emit::Suppress);
    let v = e.sample(t, s, 1.0).unwrap();
    assert_eq!(v, TweenValue::F64(3.0 + (10.0 - 3.0) * 0.5));
    assert_eq!(e.get_f64(tg(1), X), Some(3.0));
}

#[test]
fn colour_tracks_lerp_in_their_space() {
    let red = Rgba::from_u32(0xff0000ff);
    let blue = Rgba::from_u32(0x0000ffff);
    for space in [
        ColorSpace::Srgb,
        ColorSpace::Linear,
        ColorSpace::Hsv,
        ColorSpace::Oklab,
        ColorSpace::Oklch,
    ] {
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::Color(red));
        e.to(
            one(0),
            &[PropTo::to(X, TweenValue::Color(blue))],
            lin(1.0).color_space(space),
        );
        e.advance(0.5);
        let want = TweenValue::lerp(
            &TweenValue::Color(red),
            &TweenValue::Color(blue),
            0.5,
            space,
        );
        assert_eq!(e.get(tg(0), X), Some(want), "{space:?}");
        e.advance(0.5);
        assert_eq!(
            e.get(tg(0), X),
            Some(TweenValue::Color(blue)),
            "{space:?} lands"
        );
    }
}

#[test]
fn int_and_snap_tracks() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::Int(0));
    e.seed(tg(0), Y, TweenValue::F64(0.0));
    e.to(
        one(0),
        &[
            PropTo::to(X, TweenValue::Int(5)),
            PropTo::to_f64(Y, 10.0).snap(4.0),
        ],
        lin(1.0),
    );
    e.advance(0.5);
    assert_eq!(e.get(tg(0), X), Some(TweenValue::Int(3)));
    assert_eq!(e.get_f64(tg(0), Y), Some(4.0));
}

#[test]
fn per_target_values_and_is_tweening() {
    let mut e = TweenEngine::new();
    seed_all(&mut e, 3, X, 0.0);
    let vals = [TweenValue::F64(10.0), TweenValue::F64(20.0)];
    let t = e.to(
        Targets::Range { first: 0, count: 3 },
        &[PropTo::each(X, &vals)],
        lin(1.0),
    );
    assert!(e.is_tweening(tg(2)));
    assert!(!e.is_tweening(tg(7)));
    e.advance(1.0);
    assert_eq!(
        [val(&e, 0, X), val(&e, 1, X), val(&e, 2, X)],
        [10.0, 20.0, 20.0]
    );
    assert!(!e.anim_ref(t).is_alive());
    assert!(!e.is_tweening(tg(2)));
}

#[test]
fn set_duration_delay_start_repeat() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).paused(true));
    e.anim(t).set_total_time(0.5, Emit::Fire);
    e.anim(t).set_duration(2.0);
    let r = e.anim_ref(t);
    assert_eq!(
        (r.duration(), r.total_time(), r.total_progress()),
        (2.0, 1.0, 0.5)
    );
    e.anim(t).set_repeat(1);
    assert_eq!(e.anim_ref(t).total_duration(), 4.0);
    assert_eq!(e.anim_ref(t).repeat(), 1);
    e.anim(t).set_yoyo(true).set_repeat_delay(0.5);
    assert!(e.anim_ref(t).yoyo());
    assert_eq!(e.anim_ref(t).total_duration(), 4.5);
    // A timeline's duration is fitted through its time scale.
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(one(1), &[to(X, 1.0)], lin(2.0), Position::END);
    e.anim(tl).set_duration(1.0);
    assert_eq!(e.anim_ref(tl).time_scale(), 2.0);
    // set_start_time re-inserts; set_delay moves the start only under a
    // smooth parent (the root is one).
    let a = e.to(one(2), &[to(X, 1.0)], lin(1.0));
    e.anim(a).set_start_time(3.0);
    assert_eq!(e.anim_ref(a).start_time(), 3.0);
    e.anim(a).set_delay(0.5);
    assert_eq!(
        (e.anim_ref(a).start_time(), e.anim_ref(a).delay()),
        (3.5, 0.5)
    );
    let c = e.tl(tl).last();
    e.anim(c).set_delay(0.5);
    assert_eq!(
        (e.anim_ref(c).start_time(), e.anim_ref(c).delay()),
        (0.0, 0.5)
    );
}

#[test]
fn timeline_builder_utilities() {
    let mut e = TweenEngine::new();
    seed_all(&mut e, 3, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(1.0), Position::END)
        .to(one(1), &[to(X, 1.0)], lin(1.0), Position::END)
        .add_label(Tag(1), 1.0);
    assert_eq!(e.anim_ref(tl).child_count(), 2);
    e.tl(tl).shift_children(1.0, true, 0.0);
    assert_eq!(e.anim_ref(tl).label_time(Tag(1)), Some(2.0));
    assert_eq!(e.anim_ref(tl).duration(), 3.0);
    e.tl(tl).remove_label(Tag(1));
    assert_eq!(e.anim_ref(tl).label_time(Tag(1)), None);
    let last = e.tl(tl).last();
    e.tl(tl).remove(last);
    assert_eq!(e.anim_ref(tl).child_count(), 1);
    assert!(e.anim_ref(last).is_alive(), "removed, not killed");
    e.tl(tl).clear(true);
    assert_eq!(e.anim_ref(tl).child_count(), 0);
    assert_eq!(e.tl(tl).last(), TweenId::NONE);
    // A timeline cannot be added into itself or its descendant.
    let inner = e.timeline(TimelineOpts::new());
    e.tl(tl).add(inner, 0.0);
    e.tl(inner).add(tl, 0.0);
    assert_eq!(e.anim_ref(inner).child_count(), 0);
    // A stale timeline handle builds nothing.
    e.kill_all();
    assert!(!e.anim_ref(tl).is_alive());
    e.tl(tl).to(one(0), &[to(X, 1.0)], lin(1.0), Position::END);
    assert_eq!(e.stats().live_nodes, 0);
}

#[test]
fn root_time_scale_and_activity() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    assert!(!e.is_active());
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).delay(1.0));
    assert!(e.is_active(), "a future child counts");
    e.set_root_time_scale(2.0);
    assert_eq!(e.root_time_scale(), 2.0);
    e.advance(0.75);
    close(val(&e, 0, X), 50.0, 1e-9, "root at 1.5");
    e.anim(t).pause();
    assert!(!e.is_active(), "paused-only engines idle");
    e.advance(-1.0);
    e.advance(f64::NAN);
    close(val(&e, 0, X), 50.0, 1e-9, "bad dt is 0");
}

#[test]
fn defaults_chain() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    e.set_defaults(TweenOpts::new().duration(2.0).ease(Easing::Linear));
    assert_eq!(e.defaults().duration, Some(2.0));
    let t = e.to(one(0), &[to(X, 100.0)], TweenOpts::new().paused(true));
    assert_eq!(e.anim_ref(t).duration(), 2.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .defaults(TweenOpts::new().duration(3.0))
            .paused(true),
    );
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], TweenOpts::new(), Position::END);
    let c = e.tl(tl).last();
    assert_eq!(e.anim_ref(c).duration(), 3.0);
    e.tl(tl).to(
        one(0),
        &[to(X, 1.0)],
        TweenOpts::new().inherit(false),
        Position::END,
    );
    let d = e.tl(tl).last();
    assert_eq!(
        e.anim_ref(d).duration(),
        2.0,
        "inherit(false) skips the timeline defaults"
    );
}

#[test]
fn non_finite_inputs_never_reach_the_clock() {
    let mut e = TweenEngine::new();
    seed_all(&mut e, 4, X, 0.0);
    // A NaN delay counts as 0 and does not stall the siblings after it.
    e.to(one(0), &[to(X, 10.0)], lin(1.0).delay(f64::NAN));
    let b = e.to(one(1), &[to(X, 10.0)], lin(1.0));
    e.anim(b).set_start_time(f64::NAN); // ignored
    let c = e.to(one(2), &[to(X, 10.0)], lin(1.0));
    e.advance(0.5);
    for i in 0..3 {
        close(val(&e, i, X), 5.0, 1e-9, "healthy siblings run");
    }
    e.anim(c).seek(Seek::Time(f64::NAN), Emit::Fire);
    e.anim(c).set_progress(f64::NAN, Emit::Fire);
    e.anim(c).set_duration(f64::NAN);
    e.anim(c).set_time_scale(f64::NAN);
    e.anim(c).set_delay(f64::INFINITY);
    close(val(&e, 2, X), 5.0, 1e-9, "ignored controls");
    assert_eq!(e.anim_ref(c).time_scale(), 1.0);
    // A NaN root time scale counts as 0; setting 1 brings the clock back.
    e.set_root_time_scale(f64::NAN);
    assert_eq!(e.root_time_scale(), 0.0);
    e.advance(0.1);
    e.set_root_time_scale(1.0);
    e.advance(0.25);
    close(val(&e, 2, X), 7.5, 1e-9, "root recovered");
    // A NaN position reads as an unknown label: the timeline's end.
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(one(3), &[to(X, 1.0)], lin(1.0), 0.0);
    e.tl(tl).to(one(3), &[to(X, 2.0)], lin(1.0), f64::NAN);
    let last = e.tl(tl).last();
    assert_eq!(e.anim_ref(last).start_time(), 1.0);
}

#[test]
fn iteration_saturates_far_into_an_infinite_repeat() {
    let mut e = TweenEngine::new();
    seed_all(&mut e, 1, X, 0.0);
    let t = e.to(
        one(0),
        &[to(X, 1.0)],
        lin(0.001).repeat(-1).events(EventMask::ALL),
    );
    e.advance(0.0005);
    e.anim(t).seek(Seek::Time(1e7 + 0.0005), Emit::Fire);
    assert_eq!(e.anim_ref(t).iteration(), u32::MAX);
    assert!(e
        .events()
        .iter()
        .all(|ev| ev.iteration == u32::MAX || ev.iteration == 1));
}

#[test]
fn slot_key_of_an_unknown_slot_is_the_default() {
    let e = TweenEngine::new();
    assert_eq!(e.slot_key(SlotId(7)), (TargetId(0), PropKey(0)));
}
