//! Overwrite (design 12.9), replayed against the GSAP 3.15 overwrite cases
//! (tests/golden/overwrite.rs).

mod util;

#[path = "golden"]
mod golden {
    pub mod common;
    pub mod overwrite;
}

use golden::common::{list, num, val as gval, Val};
use golden::overwrite::{OverwriteCase, CASES};
use makepad_tween::*;
use util::*;

fn case(id: &str) -> &'static OverwriteCase {
    CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no overwrite case {id}"))
}

/// Checks one snapshot: property values on target 0 (and 1 for `t2`) and
/// the Interrupt callbacks since the last snapshot.
#[track_caller]
fn snap(e: &mut TweenEngine, names: &mut Names, c: &OverwriteCase, step: usize) {
    let s = &c.steps[step];
    let ctx = format!("{} [{}]", c.id, s.at);
    for (key, t, p) in [
        ("obj.x", 0, X),
        ("obj.y", 0, Y),
        ("obj.a", 0, A),
        ("t1.a", 0, A),
        ("t2.a", 1, A),
    ] {
        if let Some(Val::F(want)) = gval(s.vals, key) {
            close(val(e, t, p), want, VAL_TOL, &format!("{ctx}: {key}"));
        }
    }
    let got = names.drain(e).1;
    let want = list(s.vals, "callbacks");
    assert_eq!(got, want, "{ctx}: callbacks");
}

fn t1_opts(variant: &str) -> TweenOpts {
    let o = lin(1.0).events(EventMask::INTERRUPT);
    match variant {
        "paused" => o.paused(true),
        "not_started" => o.delay(1.0),
        _ => o,
    }
}

/// The standard sequence: t1 x,y -> 100 (playing at 0.5, paused at 0.5 or
/// not started); t2 x -> 200 with the given overwrite, paused.
fn standard(id: &str, mode: Overwrite, variant: &str) {
    let c = case(id);
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    e.seed(tg(0), Y, TweenValue::F64(0.0));
    let t1 = e.to(one(0), &[to(X, 100.0), to(Y, 100.0)], t1_opts(variant));
    names.name(t1, "t1");
    if variant != "not_started" {
        e.anim(t1).set_total_time(0.5, Emit::Fire);
    }
    snap(&mut e, &mut names, c, 0);
    let t2 = e.to(
        one(0),
        &[to(X, 200.0)],
        lin(1.0).overwrite(mode).paused(true),
    );
    snap(&mut e, &mut names, c, 1);
    e.anim(t2).set_total_time(0.0, Emit::Fire);
    snap(&mut e, &mut names, c, 2);
    e.anim(t1).set_total_time(1.0, Emit::Fire);
    if mode == Overwrite::All && variant == "not_started" {
        // GSAP re-initialises the killed, never-started t1 when driven and
        // it writes 100/100; the engine frees a killed tween, so its handle
        // is stale and nothing moves (deviation).
        assert!(!e.anim_ref(t1).is_alive());
        assert_eq!((val(&e, 0, X), val(&e, 0, Y)), (0.0, 0.0));
        names.drain(&mut e);
        e.anim(t2).set_total_time(1.0, Emit::Fire);
        assert_eq!((val(&e, 0, X), val(&e, 0, Y)), (200.0, 0.0));
        return;
    }
    snap(&mut e, &mut names, c, 3);
    e.anim(t2).set_total_time(1.0, Emit::Fire);
    snap(&mut e, &mut names, c, 4);
}

#[test]
fn golden_auto_kills_the_active_rival_at_first_render() {
    standard("auto_t1_playing", Overwrite::Auto, "playing");
    // T12a [golden: design_auto_single_prop].
    let c = case("design_auto_single_prop");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let a = e.to(
        one(0),
        &[to(A, 100.0)],
        lin(1.0).events(EventMask::INTERRUPT),
    );
    names.name(a, "A");
    e.anim(a).set_total_time(0.5, Emit::Fire);
    snap(&mut e, &mut names, c, 0);
    let b = e.to(
        one(0),
        &[to(A, 0.0)],
        lin(1.0).overwrite(Overwrite::Auto).paused(true),
    );
    snap(&mut e, &mut names, c, 1);
    e.anim(b).set_total_time(0.0, Emit::Fire);
    snap(&mut e, &mut names, c, 2);
    assert!(!e.anim_ref(a).is_alive(), "A has no track left: killed");
    e.anim(b).set_total_time(0.5, Emit::Fire);
    snap(&mut e, &mut names, c, 3);
    e.anim(a).set_total_time(0.8, Emit::Fire);
    snap(&mut e, &mut names, c, 4);
}

#[test]
fn golden_auto_kills_every_active_rival() {
    // T12b: Interrupts in reverse creation order [golden: design_auto_two_rivals].
    let c = case("design_auto_two_rivals");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let a = e.to(
        one(0),
        &[to(A, 100.0)],
        lin(1.0).events(EventMask::INTERRUPT),
    );
    e.anim(a).set_total_time(0.5, Emit::Fire);
    let a2 = e.to(
        one(0),
        &[to(A, 50.0)],
        lin(2.0).events(EventMask::INTERRUPT),
    );
    e.anim(a2).set_total_time(0.5, Emit::Fire);
    names.name(a, "A");
    names.name(a2, "A2");
    snap(&mut e, &mut names, c, 0);
    let cc = e.to(
        one(0),
        &[to(A, -100.0)],
        lin(1.0).overwrite(Overwrite::Auto).paused(true),
    );
    e.anim(cc).set_total_time(0.0, Emit::Fire);
    snap(&mut e, &mut names, c, 1);
    assert!(!e.anim_ref(a).is_alive() && !e.anim_ref(a2).is_alive());
}

#[test]
fn golden_all_is_per_target() {
    standard("true_t1_playing", Overwrite::All, "playing");
    standard("true_t1_paused", Overwrite::All, "paused");
    standard("true_t1_not_started", Overwrite::All, "not_started");
    // T12c [golden: design_true_multi_target].
    let c = case("design_true_multi_target");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 2, A, 0.0);
    let a = e.to(
        Targets::Range { first: 0, count: 2 },
        &[to(A, 100.0)],
        lin(1.0).events(EventMask::INTERRUPT),
    );
    names.name(a, "A");
    e.anim(a).set_total_time(0.5, Emit::Fire);
    snap(&mut e, &mut names, c, 0);
    e.to(
        one(0),
        &[to(A, 0.0)],
        lin(1.0).overwrite(Overwrite::All).paused(true),
    );
    snap(&mut e, &mut names, c, 1);
    assert!(e.anim_ref(a).is_alive(), "A keeps its t2 track");
    e.anim(a).set_total_time(1.0, Emit::Fire);
    snap(&mut e, &mut names, c, 2);
}

#[test]
fn golden_auto_ignores_unstarted_paused_and_finished() {
    standard("auto_t1_paused", Overwrite::Auto, "paused");
    standard("auto_t1_not_started", Overwrite::Auto, "not_started");
    // A finished tween is not a rival [golden: design_auto_ignores_finished].
    // GSAP keeps the completed A object and re-links it on totalTime(); the
    // engine frees completed bare tweens, so A is built with keep(true).
    let c = case("design_auto_ignores_finished");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), A, TweenValue::F64(0.0));
    let a = e.to(
        one(0),
        &[to(A, 100.0)],
        lin(1.0).keep(true).events(EventMask::INTERRUPT),
    );
    names.name(a, "A");
    e.anim(a).set_total_time(1.0, Emit::Fire);
    snap(&mut e, &mut names, c, 0);
    let b = e.to(
        one(0),
        &[to(A, 0.0)],
        lin(1.0).overwrite(Overwrite::Auto).paused(true),
    );
    e.anim(b).set_total_time(0.0, Emit::Fire);
    snap(&mut e, &mut names, c, 1);
    e.anim(a).set_total_time(0.5, Emit::Fire);
    snap(&mut e, &mut names, c, 2);
}

#[test]
fn golden_none_lets_the_later_in_walk_win() {
    standard("false_t1_playing", Overwrite::None, "playing");
}

#[test]
fn golden_kill_tweens_of_is_property_level() {
    let c = case("design_kill_tweens_of_props");
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    e.seed(tg(0), Y, TweenValue::F64(0.0));
    let a = e.to(
        one(0),
        &[to(X, 100.0), to(Y, 100.0)],
        lin(1.0).events(EventMask::INTERRUPT),
    );
    names.name(a, "A");
    e.anim(a).set_total_time(0.5, Emit::Fire);
    snap(&mut e, &mut names, c, 0);
    e.kill_tweens_of(one(0), Some(&[X]));
    snap(&mut e, &mut names, c, 1);
    e.anim(a).set_total_time(1.0, Emit::Fire);
    snap(&mut e, &mut names, c, 2);
    e.kill_tweens_of(one(0), Some(&[Y]));
    snap(&mut e, &mut names, c, 3);
}

#[test]
fn golden_auto_waits_for_the_start_of_an_immediate_render() {
    for (id, kind) in [
        ("design_auto_deferred_from", 0),
        ("design_auto_deferred_fromto", 1),
        ("design_auto_deferred_to_immediaterender", 2),
    ] {
        let c = case(id);
        let mut e = TweenEngine::new();
        let mut names = Names::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        let a = e.to(
            one(0),
            &[to(X, 100.0)],
            lin(1.0).events(EventMask::INTERRUPT),
        );
        names.name(a, "A");
        e.anim(a).set_total_time(0.5, Emit::Fire);
        snap(&mut e, &mut names, c, 0);
        let o = lin(1.0).overwrite(Overwrite::Auto).paused(true);
        let b = match kind {
            0 => e.from(one(0), &[PropTo::from(X, (-50.0).into())], o),
            1 => e.from_to(one(0), &[PropTo::from_to(X, (-50.0).into(), 0.0.into())], o),
            _ => e.to(one(0), &[to(X, 0.0)], o.immediate_render(true)),
        };
        snap(&mut e, &mut names, c, 1);
        e.anim(b).set_total_time(0.0, Emit::Fire);
        if kind == 0 {
            // GSAP's from() kills already at this render at exactly 0; the
            // engine waits for a render past 0 for from() and fromTo()
            // alike (deviation 17).
            assert_eq!(list(c.steps[2].vals, "callbacks"), ["A:onInterrupt"]);
            assert!(names.drain(&mut e).1.is_empty());
            close(val(&e, 0, X), num(c.steps[2].vals, "obj.x"), VAL_TOL, id);
            e.anim(b).set_total_time(0.1, Emit::Fire);
            assert_eq!(names.drain(&mut e).1, ["A:onInterrupt"]);
            close(val(&e, 0, X), num(c.steps[3].vals, "obj.x"), VAL_TOL, id);
            continue;
        }
        snap(&mut e, &mut names, c, 2);
        e.anim(b).set_total_time(0.1, Emit::Fire);
        snap(&mut e, &mut names, c, 3);
    }
}

#[test]
fn kill_survives_rewind() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(one(0), &[to(X, 100.0)], lin(1.0), 0.0).to(
        one(0),
        &[to(X, 0.0)],
        lin(1.0).overwrite(Overwrite::Auto),
        0.5,
    );
    e.anim(tl).set_total_time(0.75, Emit::Fire);
    // The first rendered to 75, then the second captured 75 and killed it.
    close(val(&e, 0, X), 56.25, 1e-9, "the second took over from 75");
    e.anim(tl).set_total_time(0.25, Emit::Fire);
    // Rewound before the second's start: the first stays dead, the second
    // renders its start value.
    close(val(&e, 0, X), 75.0, 1e-9, "no resurrection");
}

#[test]
fn overwrite_all_in_a_stagger_kills_the_group() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    seed_all(&mut e, 3, X, 0.0);
    let g = e.to(
        Targets::Range { first: 0, count: 3 },
        &[to(X, 100.0)],
        lin(1.0)
            .stagger(Stagger::each(0.1))
            .events(EventMask::INTERRUPT),
    );
    names.name(g, "g");
    e.advance(0.2);
    e.to(
        Targets::Range { first: 0, count: 3 },
        &[to(X, 0.0)],
        lin(1.0).overwrite(Overwrite::All),
    );
    assert_eq!(names.drain(&mut e).1, ["g:onInterrupt"]);
    assert!(!e.anim_ref(g).is_alive());
}
