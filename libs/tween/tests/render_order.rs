//! Render order and callbacks (design 12.5), replayed against the GSAP 3.15
//! callback traces (tests/golden/callbacks.rs): the same timeline is built,
//! the same ops are applied and the fired callbacks must match exactly, in
//! order. Values compare at 5e-7 (GSAP rounds written values to 1e-6).

mod util;

#[path = "golden"]
mod golden {
    pub mod callbacks;
    pub mod common;
    pub mod samples;
}

use golden::callbacks::{Op, Trace, IMMEDIATE_RENDER, TRACES};
use golden::common::{column, num, val as gval, Val};
use makepad_tween::*;
use util::*;

fn trace(id: &str) -> &'static Trace {
    TRACES
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no trace {id}"))
}

fn sample_case(id: &str) -> &'static golden::samples::SampleCase {
    golden::samples::CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no sample case {id}"))
}

const CALL: Tag = Tag(100);
const MID: Tag = Tag(200);

/// The main golden timeline: tl {paused, repeat 1} holding a (0..2, repeat
/// 1 yoyo), b (2.5..3.5), a call at 1.25, label "mid" at 2 and the inner
/// timeline (3..4). Targets: a = 0, b = 1, c = 2.
fn build_main(e: &mut TweenEngine, names: &mut Names) -> TweenId {
    seed_all(e, 3, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true).repeat(1).events(CBS));
    let inner = e.timeline(TimelineOpts::new().events(CBS));
    e.tl(inner)
        .to(one(2), &[to(X, 1.0)], lin(1.0), Position::END);
    let (a, b);
    {
        let mut t = e.tl(tl);
        t.to(
            one(0),
            &[to(X, 1.0)],
            lin(1.0).repeat(1).yoyo(true).events(CBS),
            Position::END,
        );
        a = t.last();
        t.to(
            one(1),
            &[to(X, 1.0)],
            lin(1.0).events(CBS),
            Position::rel(0.5),
        );
        b = t.last();
        t.call(CALL, 1.25);
        t.add_label(MID, 2.0);
        t.add(inner, 3.0);
    }
    names.name(tl, "tl");
    names.name(a, "a");
    names.name(b, "b");
    names.name(inner, "inner");
    names.tag(CALL, "call");
    assert_eq!(e.anim_ref(tl).duration(), 4.0);
    assert_eq!(e.anim_ref(tl).total_duration(), 8.0);
    tl
}

#[track_caller]
fn check_state(e: &TweenEngine, tl: TweenId, op: &Op, ctx: &str) {
    let Some(s) = op.state else { return };
    let r = e.anim_ref(tl);
    close(r.time(), s.time, TIME_TOL, &format!("{ctx}: time"));
    close(
        r.total_time(),
        s.total_time,
        TIME_TOL,
        &format!("{ctx}: totalTime"),
    );
    close(
        r.progress(),
        s.progress,
        TIME_TOL,
        &format!("{ctx}: progress"),
    );
    close(
        r.total_progress(),
        s.total_progress,
        TIME_TOL,
        &format!("{ctx}: totalProgress"),
    );
    assert_eq!(r.iteration() as f64, s.iteration, "{ctx}: iteration");
    assert_eq!(r.reversed(), s.reversed, "{ctx}: reversed");
    assert_eq!(r.paused(), s.paused, "{ctx}: paused");
}

#[track_caller]
fn check_values(e: &TweenEngine, op: &Op, keys: &[(&str, u32, PropKey)], ctx: &str) {
    for (k, t, p) in keys {
        if let Some(Val::F(want)) = gval(op.values, k) {
            close(val(e, *t, *p), want, VAL_TOL, &format!("{ctx}: value {k}"));
        }
    }
}

/// Replays a main-timeline trace. GSAP's `time(v)` lands in the first
/// iteration (a bug, deviation C14): the replay applies GSAP's resulting
/// total time instead, so the rest of the trace stays aligned. A jump that
/// reaches the end from an earlier iteration also reports Complete here
/// (GSAP returns early and omits it; deviation 7).
fn replay_main(id: &str) {
    let tr = trace(id);
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let tl = build_main(&mut e, &mut names);
    names.drain(&mut e);
    for (i, op) in tr.ops.iter().enumerate() {
        let ctx = format!("{id} op {i} {:?}", op.op);
        let mut want: Vec<String> = op.fired_all.iter().map(|s| s.to_string()).collect();
        if op.op.starts_with("time(") {
            let st = op.state.unwrap();
            e.anim(tl).set_total_time(st.total_time, Emit::Fire);
        } else {
            apply_op(&mut e, tl, op.op);
        }
        if op.op == "totalTime(8)"
            && want.iter().any(|s| s == "tl:onRepeat")
            && !want.iter().any(|s| s == "tl:onComplete")
        {
            want.push("tl:onComplete".into());
        }
        let (all, _) = names.drain(&mut e);
        assert_eq!(all, want, "{ctx}");
        check_state(&e, tl, op, &ctx);
        check_values(&e, op, &[("a", 0, X), ("b", 1, X), ("c", 2, X)], &ctx);
    }
}

#[test]
fn golden_main_ops_as_specified() {
    replay_main("main_ops_as_specified");
}

#[test]
fn golden_main_ops_seek_events() {
    replay_main("main_ops_seek_events");
}

#[test]
fn golden_setter_seek_false() {
    replay_main("setter_seek_false");
}

#[test]
fn golden_setter_seek_default() {
    replay_main("setter_seek_default");
}

#[test]
fn golden_setter_total_time() {
    replay_main("setter_totalTime");
}

#[test]
fn golden_setter_total_time_suppress() {
    replay_main("setter_totalTime_suppress");
}

/// GSAP 3.15's `time(v)` always lands in the first iteration; the engine
/// stays in the current one (documented deviation). Inside iteration 1 the
/// two agree.
#[test]
fn golden_setter_time_deviate_stays_in_the_current_iteration() {
    let tr = trace("setter_time");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let tl = build_main(&mut e, &mut names);
    names.drain(&mut e);
    for (i, op) in tr.ops.iter().enumerate().take(3) {
        apply_op(&mut e, tl, op.op);
        let (all, _) = names.drain(&mut e);
        assert_eq!(all, op.fired_all, "setter_time op {i}");
        check_state(&e, tl, op, &format!("setter_time op {i}"));
    }
    // time(5.5) from iteration 1: GSAP lands at total 1.5; the engine clamps
    // to the end of the current iteration.
    e.anim(tl).set_time(5.5, Emit::Fire);
    assert_eq!(e.anim_ref(tl).total_time(), 4.0);
    // From iteration 2 the engine stays in iteration 2.
    e.anim(tl).set_total_time(5.0, Emit::Suppress);
    e.anim(tl).set_time(0.7, Emit::Fire);
    close(
        e.anim_ref(tl).total_time(),
        4.7,
        TIME_TOL,
        "time(0.7) in iteration 2",
    );
    assert_eq!(e.anim_ref(tl).iteration(), 2);
}

#[test]
fn golden_zero_duration_in_timeline() {
    let tr = trace("zero_duration_in_timeline");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    // d = target 0 (filler), z = target 1 (x and y).
    seed_all(&mut e, 2, X, 0.0);
    e.seed(tg(1), Y, TweenValue::F64(0.0));
    let tl = e.timeline(
        TimelineOpts::new().paused(true).events(
            EventMask::START
                .with(EventMask::COMPLETE)
                .with(EventMask::REVERSE_COMPLETE),
        ),
    );
    let (set, zero);
    {
        let mut t = e.tl(tl);
        t.to(one(0), &[to(X, 1.0)], lin(2.0), 0.0);
        t.set(one(1), &[to(X, 1.0)], TweenOpts::new().events(CBS), 1.0);
        set = t.last();
        t.to(
            one(1),
            &[to(Y, 1.0)],
            TweenOpts::new().duration(0.0).events(CBS),
            1.5,
        );
        zero = t.last();
    }
    names.name(tl, "tl");
    names.name(set, "set");
    names.name(zero, "zero");
    names.drain(&mut e);
    for (i, op) in tr.ops.iter().enumerate() {
        let ctx = format!("zero_duration_in_timeline op {i} {:?}", op.op);
        match op.op {
            "created" => {}
            "totalTime(1, true) then totalTime(1.1)" => {
                e.anim(tl).set_total_time(0.9, Emit::Fire);
                names.drain(&mut e);
                e.anim(tl).set_total_time(1.0, Emit::Suppress);
                e.anim(tl).set_total_time(1.1, Emit::Fire);
            }
            other => apply_op(&mut e, tl, other),
        }
        let (all, _) = names.drain(&mut e);
        assert_eq!(all, op.fired_all, "{ctx}");
        check_state(&e, tl, op, &ctx);
        check_values(&e, op, &[("z_x", 1, X), ("z_y", 1, Y)], &ctx);
    }
}

#[test]
fn golden_zero_duration_standalone() {
    let tr = trace("zero_duration_standalone");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 5, X, 0.0);
    let z = |d: f64| TweenOpts::new().duration(d).events(CBS);
    // z1: root, not paused: renders during construction.
    let z1 = e.to(one(1), &[to(X, 5.0)], z(0.0));
    names.name(z1, "z1");
    assert_eq!(names.drain(&mut e).0, tr.ops[0].fired_all, "z1");
    assert_eq!(val(&e, 1, X), 5.0);
    // z2: paused: nothing until totalTime(0).
    let z2 = e.to(one(2), &[to(X, 5.0)], z(0.0).paused(true));
    names.name(z2, "z2");
    assert_eq!(names.drain(&mut e).0, tr.ops[1].fired_all, "z2 created");
    assert_eq!(val(&e, 2, X), 0.0);
    e.anim(z2).set_total_time(0.0, Emit::Fire);
    assert_eq!(
        names.drain(&mut e).0,
        tr.ops[2].fired_all,
        "z2 totalTime(0)"
    );
    assert_eq!(val(&e, 2, X), 5.0);
    // The paused tween stays addressable after completing (GSAP state).
    assert_eq!(
        e.anim_ref(z2).total_progress(),
        1.0,
        "z2 after totalTime(0)"
    );
    assert!(e.anim_ref(z2).paused());
    e.anim(z2).set_progress(1.0, Emit::Fire);
    assert_eq!(names.drain(&mut e).0, tr.ops[3].fired_all, "z2 progress(1)");
    assert_eq!(e.anim_ref(z2).total_progress(), 1.0, "z2 progress(1)");
    e.anim(z2).restart(false, Emit::Suppress);
    assert_eq!(names.drain(&mut e).0, tr.ops[4].fired_all, "z2 restart");
    assert!(!e.anim_ref(z2).paused(), "z2 restart un-pauses it");
    // z3: gsap.set with onComplete.
    let z3 = e.set(
        one(3),
        &[to(X, 7.0)],
        TweenOpts::new().events(EventMask::COMPLETE),
    );
    names.name(z3, "z3");
    assert_eq!(names.drain(&mut e).0, tr.ops[5].fired_all, "z3 set");
    assert_eq!(val(&e, 3, X), 7.0);
    // z4: a zero tween at 0 of a paused timeline.
    let tl4 = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl4).to(one(4), &[to(X, 5.0)], z(0.0), 0.0);
    let z4 = e.tl(tl4).last();
    names.name(z4, "z4");
    assert_eq!(names.drain(&mut e).0, tr.ops[6].fired_all, "z4 created");
    assert_eq!(val(&e, 4, X), 0.0);
    e.anim(tl4).set_total_time(0.0, Emit::Fire);
    assert_eq!(
        names.drain(&mut e).0,
        tr.ops[7].fired_all,
        "tl4.totalTime(0)"
    );
    assert_eq!(val(&e, 4, X), 5.0);
    e.anim(tl4).set_total_time(0.0, Emit::Fire);
    assert_eq!(names.drain(&mut e).0, tr.ops[8].fired_all, "again");
    e.anim(tl4).seek(Seek::Time(0.0), Emit::Suppress);
    assert_eq!(names.drain(&mut e).0, tr.ops[9].fired_all, "seek(0)");
    e.anim(tl4).play();
    assert_eq!(names.drain(&mut e).0, tr.ops[10].fired_all, "play");
    assert_eq!(val(&e, 4, X), 5.0);
}

/// Replays a trace of `totalTime` ops on a paused timeline built by `build`.
fn replay_ops(
    id: &str,
    e: &mut TweenEngine,
    names: &mut Names,
    tl: TweenId,
    keys: &[(&str, u32, PropKey)],
) {
    let tr = trace(id);
    names.drain(e);
    for (i, op) in tr.ops.iter().enumerate() {
        let ctx = format!("{id} op {i} {:?}", op.op);
        apply_op(e, tl, op.op);
        let (all, _) = names.drain(e);
        assert_eq!(all, op.fired_all, "{ctx}");
        check_state(e, tl, op, &ctx);
        check_values(e, op, keys, &ctx);
    }
}

fn t9(yoyo: bool) -> (TweenEngine, Names, TweenId) {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(1)
            .yoyo(yoyo)
            .events(CBS),
    );
    e.tl(tl)
        .to(one(0), &[to(A, 100.0)], lin(1.0).events(CBS), Position::END);
    let a = e.tl(tl).last();
    names.name(tl, "tl");
    names.name(a, "a");
    (e, names, tl)
}

#[test]
fn golden_timeline_repeat_order() {
    let (mut e, mut names, tl) = t9(false);
    replay_ops(
        "design_t9_timeline_repeat_order",
        &mut e,
        &mut names,
        tl,
        &[("a", 0, A)],
    );
    let (mut e, mut names, tl) = t9(true);
    replay_ops(
        "design_t9_timeline_repeat_order_yoyo",
        &mut e,
        &mut names,
        tl,
        &[("a", 0, A)],
    );
}

#[test]
fn golden_repeating_timeline_wrap() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    e.seed(tg(1), B, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true).repeat(2).events(CBS));
    let (a, b);
    {
        let mut t = e.tl(tl);
        t.to(one(0), &[to(A, 100.0)], lin(1.0).events(CBS), 0.0);
        a = t.last();
        t.call(CALL, 0.5);
        t.to(one(1), &[to(B, 100.0)], lin(0.1).events(CBS), 0.9);
        b = t.last();
    }
    names.name(tl, "tl");
    names.name(a, "a");
    names.name(b, "b");
    names.tag(CALL, "call");
    replay_ops(
        "design_t20_repeating_timeline_wrap",
        &mut e,
        &mut names,
        tl,
        &[("a", 0, A), ("b", 1, B)],
    );
}

#[test]
fn golden_forward_order() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true).events(CBS));
    e.tl(tl)
        .to(one(0), &[to(A, 1.0)], lin(1.0).events(CBS), Position::END);
    let a = e.tl(tl).last();
    names.name(tl, "tl");
    names.name(a, "a");
    replay_ops(
        "design_forward_order",
        &mut e,
        &mut names,
        tl,
        &[("a", 0, A)],
    );
}

#[test]
fn golden_nested_wraps_in_one_frame() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true).repeat(3).events(CBS));
    let tl2 = e.timeline(TimelineOpts::new().repeat(3).events(CBS));
    e.tl(tl2)
        .to(one(0), &[to(X, 1.0)], lin(0.25).events(CBS), Position::END);
    let a = e.tl(tl2).last();
    e.tl(tl).add(tl2, 0.0);
    names.name(tl, "tl");
    names.name(tl2, "tl2");
    names.name(a, "a");
    let tr = trace("design_nested_wraps_one_frame");
    names.drain(&mut e);
    for (i, op) in tr.ops.iter().enumerate() {
        let ctx = format!("nested wraps op {i} {:?}", op.op);
        apply_op(&mut e, tl, op.op);
        let (all, _) = names.drain(&mut e);
        // tl2 jumps onto its own end from an earlier iteration: GSAP returns
        // after its boundary sweep and never reports Complete (a bug); the
        // engine does (deviation 7 of the reconciliation).
        let mut want: Vec<String> = op.fired_all.iter().map(|s| s.to_string()).collect();
        if i > 0 {
            let at = want.iter().position(|s| s == "tl2:onRepeat").unwrap() + 1;
            want.insert(at, "tl2:onComplete".into());
        }
        assert_eq!(all, want, "{ctx}");
        check_state(&e, tl, op, &ctx);
        close(val(&e, 0, X), num(op.values, "x"), VAL_TOL, &ctx);
        close(
            e.anim_ref(tl2).total_time(),
            num(op.values, "tl2_total_time"),
            TIME_TOL,
            &ctx,
        );
        assert_eq!(
            e.anim_ref(tl2).iteration() as f64,
            num(op.values, "tl2_iteration"),
            "{ctx}"
        );
    }
}

#[test]
fn golden_big_dt_crossing_a_child() {
    let tr = trace("design_big_dt_crosses_child");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let r = e.timeline(TimelineOpts::new().paused(true));
    e.tl(r)
        .to(one(0), &[to(X, 100.0)], lin(0.5).events(CBS), 0.5);
    let c = e.tl(r).last();
    names.name(c, "c");
    replay_ops(
        "design_big_dt_crosses_child",
        &mut e,
        &mut names,
        r,
        &[("x", 0, X)],
    );
    assert_eq!(val(&e, 0, X).to_bits(), 100.0f64.to_bits());
    assert_eq!(tr.ops.len(), 1);
}

#[test]
fn golden_add_pause_stops_exactly() {
    let tr = trace("design_t11_add_pause");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().events(CBS));
    e.tl(tl)
        .to(one(0), &[to(A, 100.0)], lin(2.0), Position::END)
        .add_pause(1.0, Tag(7));
    names.name(tl, "tl");
    names.tag(Tag(7), "pause");
    names.drain(&mut e);
    // render(0.9) / render(1.1) are what the parent walk does: advance.
    for (op, dt) in tr.ops.iter().zip([0.9, 0.2]) {
        e.advance(dt);
        let (all, _) = names.drain(&mut e);
        assert_eq!(all, op.fired_all, "{:?}", op.op);
        check_state(&e, tl, op, op.op);
        check_values(&e, op, &[("a", 0, A)], op.op);
    }
    assert!(e.anim_ref(tl).paused());
    assert_eq!(e.anim_ref(tl).total_time(), 1.0);
}

#[test]
fn golden_add_pause_is_skipped_by_seeks() {
    // [golden positions addPause_2 extra]: totalTime()/seek() jump over a
    // pause (GSAP _forcing); only the frame walk stops on it.
    let mut e = TweenEngine::new();
    seed_all(&mut e, 3, X, 0.0);
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(1.0), Position::END)
        .to(one(1), &[to(X, 1.0)], lin(1.0), Position::END)
        .to(one(2), &[to(X, 1.0)], lin(1.0), Position::END)
        .add_pause(2.0, Tag::NONE);
    e.anim(tl).set_total_time(1.5, Emit::Fire);
    e.anim(tl).set_total_time(2.5, Emit::Fire);
    assert_eq!(e.anim_ref(tl).time(), 2.5);
    assert!(!e.anim_ref(tl).paused());
}

#[test]
fn golden_post_add_checks_render_a_child_behind_the_playhead() {
    let tr = trace("design_post_add_behind_playhead");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 2, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(
        one(0),
        &[to(X, 1.0)],
        TweenOpts::new().duration(2.0),
        Position::END,
    );
    e.anim(tl).set_total_time(1.0, Emit::Fire);
    names.drain(&mut e);
    e.tl(tl).to(
        one(1),
        &[to(X, 100.0)],
        TweenOpts::new().duration(0.5).events(CBS),
        0.2,
    );
    let (all, _) = names.drain(&mut e);
    assert_eq!(all, tr.ops[0].fired_all);
    close(val(&e, 1, X), num(tr.ops[0].values, "x"), VAL_TOL, "x");
}

#[test]
fn golden_a_completed_timeline_grows_and_plays_again() {
    let tr = trace("design_completed_timeline_grows");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 2, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().events(EventMask::START.with(EventMask::COMPLETE)));
    e.tl(tl).to(one(0), &[to(X, 1.0)], lin(1.0), Position::END);
    names.name(tl, "tl");
    names.drain(&mut e);
    e.anim(tl).set_total_time(1.0, Emit::Fire);
    let (all, _) = names.drain(&mut e);
    assert_eq!(all, tr.ops[0].fired_all);
    assert!(!e.is_active(), "completed and removed from the root");
    assert!(e.anim_ref(tl).is_alive(), "timelines are kept");
    e.tl(tl).to(
        one(1),
        &[to(X, 1.0)],
        TweenOpts::new().duration(1.0),
        Position::END,
    );
    let (all, _) = names.drain(&mut e);
    assert_eq!(all, tr.ops[1].fired_all);
    assert!(e.is_active(), "relinked");
    assert_eq!(e.anim_ref(tl).duration(), num(tr.ops[1].values, "duration"));
    e.advance(1.0);
    assert_eq!(val(&e, 1, X), 1.0, "plays the new child");
}

#[test]
fn golden_repeating_tween_start_rules() {
    let tr = trace("design_start_rules");
    let mk = |e: &mut TweenEngine, o: TweenOpts| {
        e.seed(tg(0), X, TweenValue::F64(0.0));
        let host = e.timeline(TimelineOpts::new().paused(true));
        e.tl(host).to(
            one(0),
            &[to(X, 100.0)],
            lin(1.0).events(CBS).paused(false).merge(o),
            0.0,
        );
        let t = e.tl(host).last();
        e.anim(host).set_total_time(10.0, Emit::Suppress);
        e.anim(host).set_total_time(0.0, Emit::Suppress);
        t
    };
    // 0: repeat 2 tween: totalTime(1.5) from 0.
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let t = mk(&mut e, TweenOpts::new().repeat(2));
    names.name(t, "t");
    names.drain(&mut e);
    e.anim(t).set_total_time(1.5, Emit::Fire);
    assert_eq!(
        names.drain(&mut e).0,
        tr.ops[0].fired_all,
        "{}",
        tr.ops[0].op
    );
    close(val(&e, 0, X), 50.0, VAL_TOL, "x");
    // 2: then backward across a boundary to 0.5.
    e.anim(t).set_total_time(0.5, Emit::Fire);
    assert_eq!(
        names.drain(&mut e).0,
        tr.ops[2].fired_all,
        "{}",
        tr.ops[2].op
    );
    // 1: repeat 1 yoyo: totalTime(2) from 0.
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let t = mk(&mut e, TweenOpts::new().repeat(1).yoyo(true));
    names.name(t, "t");
    names.drain(&mut e);
    e.anim(t).set_total_time(2.0, Emit::Fire);
    assert_eq!(
        names.drain(&mut e).0,
        tr.ops[1].fired_all,
        "{}",
        tr.ops[1].op
    );
    close(val(&e, 0, X), 0.0, VAL_TOL, "x");
    // 3, 4: repeat 2 timeline holding a 1 s tween.
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true).repeat(2).events(CBS));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(1.0).events(CBS), Position::END);
    let a = e.tl(tl).last();
    names.name(tl, "tl");
    names.name(a, "a");
    names.drain(&mut e);
    for op in &tr.ops[3..] {
        let (_, nums, _) = parse_op(
            op.op
                .split(" from")
                .next()
                .unwrap()
                .split("then ")
                .last()
                .unwrap(),
        );
        e.anim(tl).set_total_time(nums[0], Emit::Fire);
        assert_eq!(names.drain(&mut e).0, op.fired_all, "{}", op.op);
        close(val(&e, 0, X), num(op.values, "x"), VAL_TOL, op.op);
    }
}

/// Helper: TweenOpts field-wise merge (`self` wins).
trait Merge {
    fn merge(self, o: TweenOpts) -> TweenOpts;
}
impl Merge for TweenOpts {
    fn merge(self, o: TweenOpts) -> TweenOpts {
        let mut r = o.or(&self);
        r.tag = self.tag;
        r.events = self.events.with(o.events);
        r
    }
}

#[test]
fn golden_immediate_render_callbacks_deviate_from_to_is_silent() {
    let tr = trace("design_immediate_render_callbacks");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 5, X, 0.0);
    let to0 = e.to(
        one(1),
        &[to(X, 5.0)],
        TweenOpts::new().duration(0.0).events(CBS),
    );
    names.name(to0, "to0");
    assert_eq!(names.drain(&mut e).0, tr.ops[0].fired_all);
    let set = e.set(one(2), &[to(X, 7.0)], TweenOpts::new().events(CBS));
    names.name(set, "set");
    assert_eq!(names.drain(&mut e).0, tr.ops[1].fired_all);
    let ft = e.from_to(
        one(3),
        &[PropTo::from_to(X, 10.0.into(), 20.0.into())],
        TweenOpts::new().duration(1.0).paused(true).events(CBS),
    );
    names.name(ft, "fromTo");
    // GSAP fires onUpdate through fromTo's start-at tween; the engine's
    // creation render is silent (D22).
    assert_eq!(tr.ops[2].fired_all, &["fromTo:onUpdate"]);
    assert!(names.drain(&mut e).0.is_empty());
    assert_eq!(val(&e, 3, X), 10.0);
    let fr = e.from(
        one(4),
        &[PropTo::from(X, 100.0.into())],
        TweenOpts::new().duration(1.0).paused(true).events(CBS),
    );
    names.name(fr, "from");
    assert_eq!(names.drain(&mut e).0, tr.ops[3].fired_all);
    assert_eq!(val(&e, 4, X), 100.0);
}

#[test]
fn golden_immediate_render_defaults() {
    let g = |id: &str, key: &str| {
        let c = IMMEDIATE_RENDER.iter().find(|c| c.id == id).unwrap();
        num(c.values, key)
    };
    let fresh = || {
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        e
    };
    let x = |e: &TweenEngine| val(e, 0, X);
    let from100 = [PropTo::from(X, 100.0.into())];
    // from(): immediate, paused or not.
    let mut e = fresh();
    e.from(one(0), &from100, TweenOpts::new().duration(1.0));
    assert_eq!(x(&e), g("from_unpaused", "after_create.x"));
    let mut e = fresh();
    e.from(
        one(0),
        &from100,
        TweenOpts::new().duration(1.0).paused(true),
    );
    assert_eq!(x(&e), g("from_paused", "after_create.x"));
    // from() with immediate_render(false).
    let mut e = fresh();
    let t = e.from(
        one(0),
        &from100,
        TweenOpts::new()
            .duration(1.0)
            .paused(true)
            .immediate_render(false),
    );
    let id = "from_paused_immediateRender_false";
    assert_eq!(x(&e), g(id, "after_create.x"));
    e.anim(t).set_total_time(0.0, Emit::Fire);
    assert_eq!(x(&e), g(id, "after_totalTime_0.x"));
    e.anim(t).set_total_time(0.5, Emit::Fire);
    close(x(&e), g(id, "after_totalTime_0_5.x"), VAL_TOL, id);
    // fromTo, with and without immediate render.
    let ft = [PropTo::from_to(X, 10.0.into(), 20.0.into())];
    let mut e = fresh();
    e.from_to(one(0), &ft, TweenOpts::new().duration(1.0).paused(true));
    assert_eq!(x(&e), g("fromTo_paused", "after_create.x"));
    let mut e = fresh();
    e.from_to(
        one(0),
        &ft,
        TweenOpts::new()
            .duration(1.0)
            .paused(true)
            .immediate_render(false),
    );
    assert_eq!(
        x(&e),
        g("fromTo_paused_immediateRender_false", "after_create.x")
    );
    // to(): capture at creation with immediate_render(true), else at the first render.
    for (id, ir) in [
        ("to_paused_immediateRender_true", true),
        ("to_paused_default_start_capture", false),
    ] {
        let mut e = fresh();
        let mut o = lin(1.0).paused(true);
        if ir {
            o = o.immediate_render(true);
        }
        let t = e.to(one(0), &[to(X, 100.0)], o);
        assert_eq!(x(&e), g(id, "after_create.x"), "{id}");
        e.seed(tg(0), X, TweenValue::F64(50.0));
        e.anim(t).set_total_time(0.5, Emit::Fire);
        close(
            x(&e),
            g(id, "after_external_change_then_totalTime_0_5.x"),
            VAL_TOL,
            id,
        );
    }
    // tl.from at 2 renders its start values at once; seeks.
    let mut e = fresh();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .from(one(0), &from100, TweenOpts::new().duration(1.0), 2.0);
    let id = "timeline_from_at_2";
    assert_eq!(x(&e), g(id, "after_create.x"));
    for (t, key) in [
        (1.0, "after_seek_1.x"),
        (2.5, "after_seek_2_5.x"),
        (0.0, "after_seek_0.x"),
    ] {
        e.anim(tl).seek(Seek::Time(t), Emit::Suppress);
        close(x(&e), g(id, key), VAL_TOL, key);
    }
    // tl.set at 0: not immediate; seek(0) renders it.
    let mut e = fresh();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).set(one(0), &[to(X, 5.0)], TweenOpts::new(), 0.0);
    let id = "timeline_set_at_0";
    assert_eq!(x(&e), g(id, "after_create.x"));
    e.anim(tl).seek(Seek::Time(0.0), Emit::Suppress);
    assert_eq!(x(&e), g(id, "after_seek_0.x"));
    e.anim(tl).set_total_time(0.01, Emit::Fire);
    assert_eq!(x(&e), g(id, "after_totalTime_0_01.x"));
    let mut e = fresh();
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl).set(one(0), &[to(X, 5.0)], TweenOpts::new(), 0.0);
    assert_eq!(
        x(&e),
        g("timeline_set_at_0_unpaused_root", "after_create.x")
    );
    // Root-level set and zero-duration to().
    let mut e = fresh();
    e.set(one(0), &[to(X, 5.0)], TweenOpts::new());
    assert_eq!(x(&e), g("gsap_set", "after_create.x"));
    let mut e = fresh();
    e.to(one(0), &[to(X, 5.0)], TweenOpts::new().duration(0.0));
    assert_eq!(x(&e), g("to_zero_duration_root", "after_create.x"));
    let mut e = fresh();
    e.to(
        one(0),
        &[to(X, 5.0)],
        TweenOpts::new().duration(0.0).paused(true),
    );
    assert_eq!(x(&e), g("to_zero_duration_paused", "after_create.x"));
    // Two from()s on one property.
    for (id, second_immediate) in [
        ("two_froms_same_prop", true),
        ("two_froms_same_prop_immediateRender_false_on_second", false),
    ] {
        let c = IMMEDIATE_RENDER.iter().find(|c| c.id == id).unwrap();
        let mut e = fresh();
        let tl = e.timeline(TimelineOpts::new().paused(true));
        e.tl(tl).from(one(0), &from100, lin(1.0), Position::END);
        let mut o = lin(1.0);
        if !second_immediate {
            o = o.immediate_render(false);
        }
        e.tl(tl)
            .from(one(0), &[PropTo::from(X, 50.0.into())], o, Position::END);
        assert_eq!(x(&e), num(c.values, "after_create.x"), "{id}");
        let n = num(c.values, "seek.len") as usize;
        for i in 0..n {
            let t = num(c.values, &format!("seek.{i}.t"));
            e.anim(tl).seek(Seek::Time(t), Emit::Suppress);
            close(
                x(&e),
                num(c.values, &format!("seek.{i}.x")),
                VAL_TOL,
                &format!("{id} seek {t}"),
            );
        }
    }
}

#[test]
fn golden_zero_duration_direction() {
    // T14: tl.set(a: 5) at 1 beside a 2 s filler [golden: design_t14_zero_duration_direction].
    let c = sample_case("design_t14_zero_duration_direction");
    let mut e = TweenEngine::new();
    seed_all(&mut e, 2, A, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(1), &[to(A, 1.0)], TweenOpts::new().duration(2.0), 0.0)
        .set(one(0), &[to(A, 5.0)], TweenOpts::new(), 1.0);
    assert_eq!(val(&e, 0, A), 0.0);
    let want = column(c.series, "a");
    for (t, w) in c.t.iter().zip(want) {
        e.anim(tl).seek(Seek::Time(*t), Emit::Suppress);
        assert_eq!(val(&e, 0, A), *w, "seek {t}");
    }
}

#[test]
fn golden_stacked_from_reads_each_other() {
    // T13 [golden: design_t13_stacked_from].
    let c = sample_case("design_t13_stacked_from");
    let mut e = TweenEngine::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).from(
        one(0),
        &[PropTo::from(A, 50.0.into())],
        lin(1.0),
        Position::END,
    );
    assert_eq!(val(&e, 0, A), num(c.meta, "a_after_first_from"));
    e.tl(tl).from(
        one(0),
        &[PropTo::from(A, 80.0.into())],
        lin(1.0),
        Position::END,
    );
    assert_eq!(val(&e, 0, A), num(c.meta, "a_after_build"));
    for (t, w) in c.t.iter().zip(column(c.series, "a")) {
        e.anim(tl).seek(Seek::Time(*t), Emit::Suppress);
        close(val(&e, 0, A), *w, VAL_TOL, &format!("seek {t}"));
    }
}

fn chained(e: &mut TweenEngine) -> TweenId {
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(0), &[to(A, 100.0)], lin(1.0), Position::END)
        .to(one(0), &[to(A, 200.0)], lin(1.0), Position::END);
    tl
}

#[test]
fn golden_backward_scrub_restores_the_earlier_sibling() {
    for id in ["design_t5_backward_scrub", "design_t6_forward_seek_chained"] {
        let c = sample_case(id);
        let mut e = TweenEngine::new();
        let tl = chained(&mut e);
        for (t, w) in c.t.iter().zip(column(c.series, "a")) {
            e.anim(tl).seek(Seek::Time(*t), Emit::Suppress);
            close(val(&e, 0, A), *w, VAL_TOL, &format!("{id} seek {t}"));
        }
    }
}

#[test]
fn golden_no_early_capture() {
    // T7: stepped by 0.1; the second tween captures only when it starts.
    let c = sample_case("design_t7_no_early_capture");
    let mut e = TweenEngine::new();
    let tl = chained(&mut e);
    for (t, w) in c.t.iter().zip(column(c.series, "a")) {
        e.anim(tl).set_total_time(*t, Emit::Fire);
        close(val(&e, 0, A), *w, 1e-9, &format!("t {t}"));
        if *t < 1.0 {
            assert!(val(&e, 0, A) <= 100.0);
        }
    }
    // The same by frames of the engine root.
    let mut e = TweenEngine::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl)
        .to(one(0), &[to(A, 100.0)], lin(1.0), Position::END)
        .to(one(0), &[to(A, 200.0)], lin(1.0), Position::END);
    for _ in 0..15 {
        e.advance(0.1);
    }
    close(val(&e, 0, A), 150.0, 1e-9, "a(1.5) by frames");
}

// ---- design 12.5 unit tests ----

#[test]
fn tween_repeat_fires_after_update() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 1.0)], lin(1.0).repeat(1).events(CBS));
    names.name(t, "t");
    e.advance(0.9);
    names.drain(&mut e);
    e.advance(0.2);
    assert_eq!(names.drain(&mut e).0, ["t:onUpdate", "t:onRepeat"]);
}

#[test]
fn call_fires_both_ways() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(2.0), Position::END)
        .call(CALL, 1.0);
    names.tag(CALL, "call");
    names.drain(&mut e);
    e.anim(tl).set_total_time(1.5, Emit::Fire);
    let ev = names.raw(&mut e);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].kind, EventKind::Call { forward: true });
    assert_eq!(ev[0].tag, CALL);
    e.anim(tl).seek(Seek::Time(0.5), Emit::Fire);
    let ev = names.raw(&mut e);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].kind, EventKind::Call { forward: false });
    e.anim(tl).seek(Seek::Time(1.5), Emit::Suppress);
    e.anim(tl).seek(Seek::Time(0.5), Emit::Suppress);
    assert!(names.raw(&mut e).is_empty());
}

#[test]
fn seek_suppresses_progress_fires() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 1.0)], lin(1.0).paused(true).events(CBS));
    names.name(t, "t");
    e.anim(t).seek(Seek::Time(0.5), Emit::Suppress);
    assert!(names.drain(&mut e).0.is_empty());
    e.anim(t).set_progress(1.0, Emit::Fire);
    assert_eq!(names.drain(&mut e).0, ["t:onUpdate", "t:onComplete"]);
}

#[test]
fn label_events_are_opt_in_and_ordered() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let plain = e.timeline(TimelineOpts::new().paused(true));
    e.tl(plain)
        .to(one(0), &[to(X, 1.0)], lin(3.0), Position::END)
        .add_label(Tag(1), 1.0);
    e.anim(plain).set_total_time(2.0, Emit::Fire);
    assert!(names.raw(&mut e).is_empty(), "not watched");
    let tl = e.timeline(TimelineOpts::new().paused(true).watch_labels());
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(3.0), Position::END)
        .add_label(Tag(2), 2.0)
        .add_label(Tag(1), 1.0)
        .add_label(Tag(3), 1.0);
    names.raw(&mut e);
    e.anim(tl).set_total_time(2.5, Emit::Fire);
    let labels: Vec<Tag> = names
        .raw(&mut e)
        .iter()
        .filter_map(|ev| match ev.kind {
            EventKind::Label(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(labels, [Tag(1), Tag(3), Tag(2)]);
    e.anim(tl).set_total_time(0.5, Emit::Fire);
    let labels: Vec<Tag> = names
        .raw(&mut e)
        .iter()
        .filter_map(|ev| match ev.kind {
            EventKind::Label(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(labels, [Tag(2), Tag(1), Tag(3)]);
    e.anim(tl).set_total_time(1.5, Emit::Suppress);
    assert!(names.raw(&mut e).is_empty());
}

/// Labels moved after they were added (re-added at a new time, or shifted
/// by `shift_children`) are still reported in time order.
#[test]
fn moved_labels_are_reported_in_time_order() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    let tl = e.timeline(TimelineOpts::new().paused(true).watch_labels());
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(8.0), Position::END)
        .add_label(Tag(1), 1.0)
        .add_label(Tag(2), 2.0)
        .add_label(Tag(1), 3.0) // moves label 1 after label 2
        .add_label(Tag(3), 0.5);
    let crossed = |e: &mut TweenEngine, names: &mut Names| -> Vec<Tag> {
        names
            .raw(e)
            .iter()
            .filter_map(|ev| match ev.kind {
                EventKind::Label(l) => Some(l),
                _ => None,
            })
            .collect()
    };
    names.raw(&mut e);
    e.anim(tl).set_total_time(3.5, Emit::Fire);
    assert_eq!(crossed(&mut e, &mut names), [Tag(3), Tag(2), Tag(1)]);
    // Labels from 1.0 on move by +2: label 3 (0.5) stays first.
    e.tl(tl).shift_children(2.0, true, 1.0);
    assert_eq!(e.anim_ref(tl).label_time(Tag(2)), Some(4.0));
    e.anim(tl).set_total_time(0.0, Emit::Suppress);
    names.raw(&mut e);
    e.anim(tl).set_total_time(5.5, Emit::Fire);
    assert_eq!(crossed(&mut e, &mut names), [Tag(3), Tag(2), Tag(1)]);
    // A negative shift moves label 2 (4.0 -> 0.0) before label 3 (0.5);
    // label 1 lands at 1.0. Travelling back to 0 reports them descending.
    e.tl(tl).shift_children(-4.0, true, 1.0);
    assert_eq!(e.anim_ref(tl).label_time(Tag(2)), Some(0.0));
    e.anim(tl).set_total_time(0.0, Emit::Fire);
    assert_eq!(crossed(&mut e, &mut names), [Tag(1), Tag(3), Tag(2)]);
}

#[test]
fn forward_order_by_frames() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let m = EventMask::START
        .with(EventMask::UPDATE)
        .with(EventMask::COMPLETE);
    let tl = e.timeline(TimelineOpts::new().events(m));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], lin(1.0).events(m), Position::END);
    let c = e.tl(tl).last();
    names.name(tl, "tl");
    names.name(c, "c");
    e.advance(0.5);
    assert_eq!(
        names.drain(&mut e).0,
        ["tl:onStart", "c:onStart", "c:onUpdate", "tl:onUpdate"]
    );
    e.advance(0.6);
    assert_eq!(
        names.drain(&mut e).0,
        ["c:onUpdate", "c:onComplete", "tl:onUpdate", "tl:onComplete"]
    );
}

#[test]
fn nested_wraps_by_frames() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().repeat(3));
    let tl2 = e.timeline(TimelineOpts::new().repeat(3));
    e.tl(tl2)
        .to(one(0), &[to(X, 1.0)], lin(0.25), Position::END);
    e.tl(tl).add(tl2, 0.0);
    e.advance(0.6);
    close(val(&e, 0, X), 0.4, 1e-9, "x at 0.6");
    for _ in 0..6 {
        e.advance(0.6);
    }
    assert_eq!(val(&e, 0, X), 1.0, "lands exactly");
    assert!(!e.anim_ref(tl).is_active());
}

#[test]
fn an_act_child_ahead_of_the_playhead_is_still_rendered() {
    // GSAP renders `_act || time >= _start`: a child driven by a control
    // while its parent sits before it is rendered by the parent's next
    // forward walk even behind an unstarted sibling.
    let mut e = TweenEngine::new();
    seed_all(&mut e, 3, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(one(0), &[to(X, 100.0)], lin(1.0), 0.0);
    e.tl(tl).to(one(1), &[to(X, 100.0)], lin(1.0), 1.8);
    e.tl(tl).to(one(2), &[to(X, 100.0)], lin(1.0), 2.0);
    let b = e.tl(tl).last();
    e.anim(b).set_total_time(0.5, Emit::Fire);
    close(val(&e, 2, X), 50.0, 1e-9, "driven directly");
    e.anim(tl).set_total_time(1.5, Emit::Fire);
    assert_eq!(val(&e, 2, X), 0.0, "rendered at 1.5 - 2 = -0.5");
    assert_eq!(e.anim_ref(b).total_time(), 0.0);
}
