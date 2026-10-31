//! Groups (design 12.7 engine part and 12.8): keyframes and staggers built
//! as groups, replayed against the GSAP 3.15 sample and stagger goldens.

mod util;

#[path = "golden"]
mod golden {
    pub mod common;
    pub mod samples;
    pub mod stagger_gsap;
}

use golden::common::{num, Series};
use golden::samples::{SampleCase, CASES};
use golden::stagger_gsap::{DistConfig, From, DISTRIBUTE, IN_TWEEN};
use makepad_tween::*;
use util::*;

fn sample_case(id: &str) -> &'static SampleCase {
    CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no sample case {id}"))
}

fn read(e: &TweenEngine, a: TweenId, name: &str) -> f64 {
    match name {
        "x" => val(e, 0, X),
        "y" => val(e, 0, Y),
        "anim.time" => e.anim_ref(a).time(),
        "anim.iteration" => e.anim_ref(a).iteration() as f64,
        other => {
            let (o, p) = other.split_once('.').unwrap();
            let i: u32 = o[1..].parse().unwrap();
            val(e, i, if p == "x" { X } else { Y })
        }
    }
}

/// Drives `a` with `totalTime(t)` over the case's grid and compares every
/// series (times 1e-9, values 5e-7).
fn check(e: &mut TweenEngine, a: TweenId, c: &SampleCase) {
    check_tol(e, a, c, VAL_TOL);
}

/// [`check`] with a value tolerance.
fn check_tol(e: &mut TweenEngine, a: TweenId, c: &SampleCase, val_tol: f64) {
    let series: &[Series] = c.series;
    for (i, t) in c.t.iter().enumerate() {
        e.anim(a).set_total_time(*t, Emit::Fire);
        for s in series {
            let tol = if s.name.starts_with("anim") {
                TIME_TOL
            } else {
                val_tol
            };
            close(
                read(e, a, s.name),
                s.v[i],
                tol,
                &format!("{}: {} at {t}", c.id, s.name),
            );
        }
    }
}

fn fresh(x: f64) -> TweenEngine {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(x));
    e.seed(tg(0), Y, TweenValue::F64(0.0));
    e
}

fn step(props: &'static [PropTo<'static>], o: TweenOpts) -> KeyStep<'static> {
    KeyStep { props, opts: o }
}

fn key(at: f64, v: f64) -> Key {
    Key {
        at,
        value: TweenValue::F64(v),
        ease: None,
    }
}

static X50: [PropTo; 1] = [PropTo::to_f64(X, 50.0)];
static X20: [PropTo; 1] = [PropTo::to_f64(X, 20.0)];
static X100: [PropTo; 1] = [PropTo::to_f64(X, 100.0)];
static X0: [PropTo; 1] = [PropTo::to_f64(X, 0.0)];
static Y50: [PropTo; 1] = [PropTo::to_f64(Y, 50.0)];

#[test]
fn golden_array_keyframes() {
    for (id, outer) in [
        ("keyframes_array", None),
        ("keyframes_array_outer_ease", Some(Easing::InOutCubic)),
    ] {
        let c = sample_case(id);
        let mut e = fresh(0.0);
        let steps = [
            step(&X50, TweenOpts::new().duration(0.5)),
            step(&X20, TweenOpts::new().duration(1.0).ease(Easing::InQuad)),
        ];
        let mut o = TweenOpts::new().paused(true);
        if let Some(ea) = outer {
            o = o.ease(ea);
        }
        let g = e.keyframes(one(0), &steps, o);
        assert_eq!(
            val(&e, 0, X),
            num(c.meta, "immediate_after_create.x"),
            "{id}"
        );
        assert_eq!(e.anim_ref(g).duration(), num(c.meta, "duration"), "{id}");
        check(&mut e, g, c);
    }
    // Steps without durations take the default 0.5 each.
    let c = sample_case("design_kf_array_default_durations");
    let mut e = fresh(0.0);
    let g = e.keyframes(
        one(0),
        &[step(&X100, TweenOpts::new()), step(&X0, TweenOpts::new())],
        TweenOpts::new().paused(true),
    );
    assert_eq!(e.anim_ref(g).duration(), num(c.meta, "duration"));
    check(&mut e, g, c);
    // [{x: 100, d 1}, {y: 50, d .5}]
    let c = sample_case("design_kf_steps_x_then_y");
    let mut e = fresh(0.0);
    let g = e.keyframes(
        one(0),
        &[
            step(&X100, TweenOpts::new().duration(1.0)),
            step(&Y50, TweenOpts::new().duration(0.5)),
        ],
        TweenOpts::new().paused(true),
    );
    assert_eq!(e.anim_ref(g).duration(), num(c.meta, "duration"));
    check(&mut e, g, c);
}

#[test]
fn golden_percent_keyframes() {
    let keys = [key(0.0, 0.0), key(50.0, 80.0), key(100.0, 100.0)];
    for (id, each, outer) in [
        (
            "keyframes_percent_easeEach_none",
            Some(Easing::Linear),
            None,
        ),
        ("keyframes_percent_default_easeEach", None, None),
        (
            "keyframes_percent_outer_ease",
            Some(Easing::Linear),
            Some(Easing::InCubic),
        ),
    ] {
        let c = sample_case(id);
        let mut e = fresh(5.0);
        let mut o = TweenOpts::new().duration(2.0).paused(true);
        if let Some(ea) = each {
            o = o.ease_each(ea);
        }
        if let Some(ea) = outer {
            o = o.ease(ea);
        }
        let g = e.to(one(0), &[PropTo::keys(X, &keys)], o);
        assert_eq!(
            val(&e, 0, X),
            num(c.meta, "immediate_after_create.x"),
            "{id}: no immediate render"
        );
        // With an outer ease the inner time goes through round7; GSAP's
        // power2.in is `p ** 3` (V8 pow), the parity InCubic is `p * p * p`:
        // a last-bit difference on a round7 tie moves the inner time by
        // 1e-7 s (8e-6 in x) at a few points.
        let tol = if outer.is_some() { 1e-5 } else { VAL_TOL };
        check_tol(&mut e, g, c, tol);
    }
}

#[test]
fn percent_and_value_keyframes() {
    // {0%: 0, 50%: 100}, duration 1: padded to 100% [golden: design_kf_percent_0_50_warm].
    let keys = [key(0.0, 0.0), key(50.0, 100.0)];
    let c = sample_case("design_kf_percent_0_50_warm");
    let mut e = fresh(0.0);
    let g = e.to(
        one(0),
        &[PropTo::keys(X, &keys)],
        TweenOpts::new().duration(1.0).paused(true),
    );
    e.anim(g).set_total_time(1.0, Emit::Fire);
    e.anim(g).set_total_time(0.0, Emit::Fire);
    check(&mut e, g, c);
    // Values [0, 100, 50] at 0/50/100% [golden: design_kf_values_0_100_50].
    let vals = [
        TweenValue::F64(0.0),
        TweenValue::F64(100.0),
        TweenValue::F64(50.0),
    ];
    let c = sample_case("design_kf_values_0_100_50");
    let mut e = fresh(0.0);
    let g = e.to(
        one(0),
        &[PropTo::values(X, &vals)],
        TweenOpts::new().duration(1.0).paused(true),
    );
    assert_eq!(e.anim_ref(g).duration(), num(c.meta, "duration"));
    check(&mut e, g, c);
}

#[test]
fn padded_percent_keyframes_first_frame_deviate() {
    // GSAP's first render of a padded percent-keyframes tween uses a stale
    // inner duration (x(0.125) = 3.125); the engine uses the padded one.
    let c = sample_case("design_kf_percent_0_50_fresh");
    assert_eq!(c.series[0].v[0], 3.125);
    let keys = [key(0.0, 0.0), key(50.0, 100.0)];
    let mut e = fresh(0.0);
    let g = e.to(
        one(0),
        &[PropTo::keys(X, &keys)],
        TweenOpts::new().duration(1.0).paused(true),
    );
    let want = [12.5, 50.0, 100.0];
    for (t, w) in c.t.iter().zip(want) {
        e.anim(g).set_total_time(*t, Emit::Fire);
        close(val(&e, 0, X), w, 1e-9, &format!("t {t}"));
    }
}

#[test]
fn golden_plain_props_next_to_keyframes_are_linear() {
    let keys = [key(50.0, 100.0), key(100.0, 100.0)];
    for (id, outer) in [
        ("design_kf_ordinary_prop", None),
        ("design_kf_ordinary_prop_outer_ease", Some(Easing::InCubic)),
    ] {
        let c = sample_case(id);
        let mut e = fresh(0.0);
        let mut o = TweenOpts::new().duration(1.0).paused(true);
        if let Some(ea) = outer {
            o = o.ease(ea);
        }
        let g = e.to(
            one(0),
            &[PropTo::keys(X, &keys), PropTo::to_f64(Y, 50.0)],
            o,
        );
        check(&mut e, g, c);
    }
}

#[test]
fn outer_ease_defaults_to_linear_and_ease_each_to_in_out_quad() {
    let keys = [key(100.0, 100.0)];
    let mut e = fresh(0.0);
    let g = e.to(
        one(0),
        &[PropTo::keys(X, &keys)],
        TweenOpts::new().duration(1.0).paused(true),
    );
    e.anim(g).set_total_time(0.25, Emit::Fire);
    close(
        val(&e, 0, X),
        100.0 * Easing::InOutQuad.map(0.25),
        1e-12,
        "ease_each",
    );
    // The engine default ease (OutQuad) does not reach keyframes.
    let mut e = fresh(0.0);
    let g = e.keyframes(
        one(0),
        &[step(&X100, TweenOpts::new().duration(1.0))],
        TweenOpts::new().paused(true),
    );
    e.anim(g).set_total_time(0.25, Emit::Fire);
    close(val(&e, 0, X), 25.0, 1e-12, "array steps are linear");
}

#[test]
fn golden_stagger_samples_match_gsap() {
    for (id, ea) in [
        ("stagger_5_each02_none", Easing::Linear),
        ("stagger_5_each02_power2in", Easing::InCubic),
    ] {
        let c = sample_case(id);
        let mut e = TweenEngine::new();
        seed_all(&mut e, 5, X, 0.0);
        seed_all(&mut e, 5, Y, 0.0);
        let g = e.to(
            Targets::Range { first: 0, count: 5 },
            &[to(X, 100.0), to(Y, 50.0)],
            TweenOpts::new()
                .duration(1.0)
                .ease(ea)
                .stagger(Stagger::each(0.2))
                .paused(true),
        );
        assert_eq!(e.anim_ref(g).duration(), num(c.meta, "duration"), "{id}");
        assert_eq!(e.anim_ref(g).child_count(), 5);
        check(&mut e, g, c);
    }
}

#[test]
fn golden_group_yoyo_ease_reaches_children() {
    let c = sample_case("design_stagger_yoyo_ease");
    let mut e = TweenEngine::new();
    seed_all(&mut e, 3, X, 0.0);
    let g = e.to(
        Targets::Range { first: 0, count: 3 },
        &[to(X, 100.0)],
        TweenOpts::new()
            .duration(1.0)
            .ease(Easing::Linear)
            .stagger(Stagger::each(0.2))
            .repeat(1)
            .yoyo(true)
            .yoyo_ease(YoyoEase::Ease(Easing::OutQuad))
            .paused(true),
    );
    assert_eq!(e.anim_ref(g).duration(), num(c.meta, "duration"));
    assert_eq!(e.anim_ref(g).total_duration(), num(c.meta, "totalDuration"));
    check(&mut e, g, c);
}

fn to_stagger(d: &DistConfig) -> (Stagger, Option<Easing>) {
    let mut s = match (d.each, d.amount) {
        (_, Some(a)) => Stagger::amount(a),
        (Some(e), None) => Stagger::each(e),
        (None, None) => Stagger::each(0.0),
    };
    s = s.from(match d.from {
        From::Index(v) if v.fract() == 0.0 && (v == 0.0 || v >= 1.0) => {
            StaggerFrom::Index(v as u32)
        }
        From::Index(v) | From::Ratio(v) => StaggerFrom::Ratio(v, v),
        From::Ratio2(x, y) => StaggerFrom::Ratio(x, y),
        From::Named("center") => StaggerFrom::Center,
        From::Named("edges") => StaggerFrom::Edges,
        From::Named("end") => StaggerFrom::End,
        From::Named("start") => StaggerFrom::Start,
        From::Named(other) => panic!("from {other}"),
    });
    if let Some([rows, cols]) = d.grid {
        s = s.grid(rows, cols);
    }
    if let Some(a) = d.axis {
        s = s.axis(if a == "x" {
            StaggerAxis::X
        } else {
            StaggerAxis::Y
        });
    }
    if let Some(b) = d.base {
        s = s.base(b);
    }
    if let Some(r) = d.repeat {
        s = s.each_repeat(r as i32);
    }
    let ease = d.ease.map(|n| parse_gsap_ease(n).unwrap());
    if let Some(ea) = ease {
        s = s.ease(ea);
    }
    (s, ease)
}

#[test]
fn golden_staggered_group_duration() {
    for c in IN_TWEEN {
        let (st, _) = to_stagger(&c.stagger);
        let mut e = TweenEngine::new();
        seed_all(&mut e, c.n, X, 0.0);
        let mut o = lin(1.0).stagger(st).paused(true);
        if let Some(r) = c.tween_repeat {
            o = o.repeat(r as i32);
        }
        let g = e.to(
            Targets::Range {
                first: 0,
                count: c.n,
            },
            &[to(X, 1.0)],
            o,
        );
        let r = e.anim_ref(g);
        close(r.duration(), c.tween_duration, TIME_TOL, c.id);
        close(r.total_duration(), c.tween_total_duration, TIME_TOL, c.id);
        // Each child starts at its distribute delay: target i is at x = 0
        // until the group reaches that delay.
        for (i, d) in c.child_start_times_by_target_index.iter().enumerate() {
            e.anim(g).set_total_time(d + 0.05, Emit::Suppress);
            close(
                val(&e, i as u32, X),
                0.05,
                1e-9,
                &format!("{} child {i}", c.id),
            );
            if *d > 0.0 {
                e.anim(g).set_total_time(0.0, Emit::Suppress);
                e.anim(g).set_total_time(d - 0.01, Emit::Suppress);
                assert_eq!(val(&e, i as u32, X), 0.0, "{} child {i} not started", c.id);
            }
            e.anim(g).set_total_time(0.0, Emit::Suppress);
        }
    }
    // Design values: 5 targets, each .1, duration 1 -> 1.4; group repeat 1
    // -> 2.8; stagger each_repeat 1 -> 2.4.
    let mut e = TweenEngine::new();
    let t5 = Targets::Range { first: 0, count: 5 };
    let a = e.to(
        t5,
        &[to(X, 1.0)],
        lin(1.0).stagger(Stagger::each(0.1)).paused(true),
    );
    let b = e.to(
        t5,
        &[to(X, 1.0)],
        lin(1.0).stagger(Stagger::each(0.1)).repeat(1).paused(true),
    );
    let c = e.to(
        t5,
        &[to(X, 1.0)],
        lin(1.0)
            .stagger(Stagger::each(0.1).each_repeat(1))
            .paused(true),
    );
    assert_eq!(e.anim_ref(a).duration(), 1.4);
    assert_eq!(e.anim_ref(b).total_duration(), 2.8);
    assert_eq!(e.anim_ref(c).total_duration(), 2.4);
}

#[test]
fn golden_distribute_cases() {
    // Every GSAP distribute() case (the reconciled superset) through
    // Stagger::delays, within 1e-12 (the 1-D fast path skips round7).
    for c in DISTRIBUTE {
        let (st, _) = to_stagger(&c.config);
        let mut out = vec![0.0; c.n as usize];
        st.delays(c.n, &mut out);
        for (i, (got, want)) in out.iter().zip(c.delays).enumerate() {
            close(
                *got,
                *want,
                1e-12,
                &format!("{} [{i}] {}", c.id, c.config_json),
            );
            assert_eq!(st.delay(i as u32, c.n).to_bits(), got.to_bits(), "{}", c.id);
        }
    }
}
