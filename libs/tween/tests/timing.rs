//! Timing (design 12.3): durations, iteration maths, repeats, yoyo, time
//! scales, stepping, landing; replayed against the GSAP 3.15 sample and time
//! scale goldens (tests/golden/samples.rs, timescale.rs) and the dt-stepping
//! trace. Times compare within 1e-9, values within 5e-7 (GSAP rounds
//! written values to 1e-6).

mod util;

#[path = "golden"]
mod golden {
    pub mod callbacks;
    pub mod common;
    pub mod samples;
    pub mod timescale;
}

use golden::common::{column, flag, num, text, Pairs, Series};
use golden::samples::{SampleCase, CASES};
use makepad_tween::*;
use util::*;

fn sample_case(id: &str) -> &'static SampleCase {
    CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no sample case {id}"))
}

fn ease(name: &str) -> Easing {
    parse_gsap_ease(name).unwrap_or_else(|| panic!("ease {name}"))
}

/// What a series column reads.
fn read(e: &TweenEngine, anim: TweenId, name: &str) -> f64 {
    let r = e.anim_ref(anim);
    match name {
        "x" => val(e, 0, X),
        "y" => val(e, 0, Y),
        "anim.time" | "tl.time" => r.time(),
        "anim.iteration" => r.iteration() as f64,
        "anim.totalTime" => r.total_time(),
        "anim.progress" => r.progress(),
        "anim.totalProgress" => r.total_progress(),
        other => {
            // "o3.x"
            let (o, p) = other.split_once('.').unwrap();
            let i: u32 = o[1..].parse().unwrap();
            val(e, i, if p == "x" { X } else { Y })
        }
    }
}

/// Drives `anim` over the case's `t` column with `drive`, comparing every
/// series (times 1e-9, values 5e-7).
fn check_series(
    e: &mut TweenEngine,
    anim: TweenId,
    c: &SampleCase,
    mut drive: impl FnMut(&mut TweenEngine, f64),
) {
    for (i, t) in c.t.iter().enumerate() {
        drive(e, *t);
        for s in c.series {
            let want = s.v[i];
            let got = read(e, anim, s.name);
            let tol = if s.name.contains("time")
                || s.name.contains("rogress")
                || s.name.contains("iteration")
            {
                TIME_TOL
            } else {
                VAL_TOL
            };
            close(got, want, tol, &format!("{}: {} at t={t}", c.id, s.name));
        }
    }
}

fn total_time(e: &mut TweenEngine, a: TweenId, t: f64) {
    e.anim(a).set_total_time(t, Emit::Fire);
}

fn seed_xy(e: &mut TweenEngine, x: f64, y: f64) {
    e.seed(tg(0), X, TweenValue::F64(x));
    e.seed(tg(0), Y, TweenValue::F64(y));
}

/// A paused root-level tween of x and y on target 0.
fn tween_xy(e: &mut TweenEngine, x: f64, y: f64, o: TweenOpts) -> TweenId {
    e.to(one(0), &[to(X, x), to(Y, y)], o.paused(true))
}

#[test]
fn golden_tween_samples_match_gsap() {
    // to() power2.inOut.
    let c = sample_case("to_power2_inout");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 10.0);
    let t = tween_xy(
        &mut e,
        100.0,
        -50.0,
        TweenOpts::new().duration(1.0).ease(ease("power2.inOut")),
    );
    assert_eq!(e.anim_ref(t).total_duration(), num(c.meta, "totalDuration"));
    check_series(&mut e, t, c, |e, s| total_time(e, t, s));

    // from() and fromTo(): immediate render.
    let c = sample_case("from_power2_inout");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 10.0);
    let t = e.from(
        one(0),
        &[PropTo::from(X, 100.0.into()), PropTo::from(Y, 60.0.into())],
        TweenOpts::new()
            .duration(1.0)
            .ease(ease("power2.inOut"))
            .paused(true),
    );
    assert_eq!(val(&e, 0, X), num(c.meta, "immediate_after_create.x"));
    assert_eq!(val(&e, 0, Y), num(c.meta, "immediate_after_create.y"));
    check_series(&mut e, t, c, |e, s| total_time(e, t, s));

    let c = sample_case("from_immediate_render_false");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 10.0);
    let t = e.from(
        one(0),
        &[PropTo::from(X, 100.0.into()), PropTo::from(Y, 60.0.into())],
        TweenOpts::new()
            .duration(1.0)
            .ease(ease("power2.inOut"))
            .paused(true)
            .immediate_render(false),
    );
    assert_eq!(val(&e, 0, X), num(c.meta, "immediate_after_create.x"));
    assert_eq!(val(&e, 0, Y), num(c.meta, "immediate_after_create.y"));
    check_series(&mut e, t, c, |e, s| total_time(e, t, s));

    let c = sample_case("from_unpaused_immediate_only");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 10.0);
    e.from(
        one(0),
        &[PropTo::from(X, 100.0.into()), PropTo::from(Y, 60.0.into())],
        TweenOpts::new().duration(1.0).ease(ease("power2.inOut")),
    );
    assert_eq!(val(&e, 0, X), num(c.meta, "immediate_after_create.x"));
    assert_eq!(val(&e, 0, Y), num(c.meta, "immediate_after_create.y"));

    let c = sample_case("fromto_sine_inout");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 0.0);
    let t = e.from_to(
        one(0),
        &[
            PropTo::from_to(X, (-20.0).into(), 80.0.into()),
            PropTo::from_to(Y, 5.0.into(), 25.0.into()),
        ],
        TweenOpts::new()
            .duration(1.5)
            .ease(ease("sine.inOut"))
            .paused(true),
    );
    assert_eq!(val(&e, 0, X), num(c.meta, "immediate_after_create.x"));
    assert_eq!(val(&e, 0, Y), num(c.meta, "immediate_after_create.y"));
    check_series(&mut e, t, c, |e, s| total_time(e, t, s));

    // Repeats and delays.
    for (id, o) in [
        (
            "repeat2_yoyo_rdelay03",
            TweenOpts::new()
                .duration(1.0)
                .repeat(2)
                .yoyo(true)
                .repeat_delay(0.3)
                .ease(ease("power1.in")),
        ),
        (
            "repeat_infinite",
            TweenOpts::new()
                .duration(1.0)
                .repeat(-1)
                .ease(ease("power1.inOut")),
        ),
        (
            "delay05_standalone",
            TweenOpts::new()
                .duration(1.0)
                .delay(0.5)
                .ease(Easing::Linear),
        ),
        ("default_ease_power1_out", TweenOpts::new().duration(1.0)),
        ("default_duration", TweenOpts::new()),
    ] {
        let c = sample_case(id);
        let mut e = TweenEngine::new();
        seed_xy(&mut e, 0.0, 0.0);
        let t = tween_xy(&mut e, 100.0, 50.0, o);
        if let Some(golden::common::Val::F(d)) = golden::common::val(c.meta, "totalDuration") {
            assert_eq!(e.anim_ref(t).total_duration(), d, "{id}");
        }
        if let Some(golden::common::Val::F(d)) = golden::common::val(c.meta, "duration") {
            assert_eq!(e.anim_ref(t).duration(), d, "{id}");
        }
        check_series(&mut e, t, c, |e, s| total_time(e, t, s));
    }
    let c = sample_case("delay05_standalone");
    let mut e = TweenEngine::new();
    let t = e.to(one(0), &[to(X, 1.0)], lin(1.0).delay(0.5).paused(true));
    assert_eq!(e.anim_ref(t).start_time(), num(c.meta, "startTime_on_root"));
    assert_eq!(e.anim_ref(t).delay(), num(c.meta, "delay"));

    // Inside a timeline, and timeline defaults.
    let c = sample_case("delay05_in_timeline");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(
        one(0),
        &[to(X, 100.0), to(Y, 50.0)],
        lin(1.0).delay(0.5),
        Position::END,
    );
    let child = e.tl(tl).last();
    assert_eq!(
        e.anim_ref(child).start_time(),
        num(c.meta, "child_startTime")
    );
    assert_eq!(e.anim_ref(tl).duration(), num(c.meta, "timeline_duration"));
    check_series(&mut e, tl, c, |e, s| {
        e.anim(tl).seek(Seek::Time(s), Emit::Suppress);
    });

    let c = sample_case("timeline_defaults_ease");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .defaults(TweenOpts::new().ease(ease("power2.inOut")).duration(1.0)),
    );
    e.tl(tl)
        .to(one(0), &[to(X, 100.0)], TweenOpts::new(), Position::END)
        .to(
            one(0),
            &[to(Y, 100.0)],
            TweenOpts::new().ease(Easing::Linear),
            Position::END,
        );
    assert_eq!(e.anim_ref(tl).duration(), num(c.meta, "timeline_duration"));
    check_series(&mut e, tl, c, |e, s| {
        e.anim(tl).seek(Seek::Time(s), Emit::Suppress);
    });
}

#[test]
fn golden_time_snapping_and_rounding() {
    let c = sample_case("rounding_probes");
    let n = num(c.meta, "probes.len") as usize;
    for i in 0..n {
        let k = |f: &str| format!("probes.{i}.{f}");
        let kind = text(c.meta, &k("probe")).unwrap();
        let t_in = num(c.meta, &k("t_in"));
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        let anim = match kind {
            "plain_tween" => e.to(one(0), &[to(X, 100.0)], lin(1.0).paused(true)),
            "repeating_tween" => e.to(one(0), &[to(X, 100.0)], lin(1.0).repeat(1).paused(true)),
            "tween_in_timeline" => {
                let tl = e.timeline(TimelineOpts::new().paused(true));
                e.tl(tl)
                    .to(one(0), &[to(X, 100.0)], lin(1.0), Position::END);
                tl
            }
            other => panic!("{other}"),
        };
        e.anim(anim).set_total_time(t_in, Emit::Fire);
        let ctx = format!("{kind} totalTime({t_in})");
        close(val(&e, 0, X), num(c.meta, &k("x")), VAL_TOL, &ctx);
        close(
            e.anim_ref(anim).time(),
            num(c.meta, &k("time")),
            1e-12,
            &ctx,
        );
        close(
            e.anim_ref(anim).total_time(),
            num(c.meta, &k("totalTime")),
            1e-12,
            &ctx,
        );
    }
    // GSAP rounds written values to 1e-6; the engine keeps full precision
    // (deviation) [golden: design_value_rounding].
    let c = sample_case("design_value_rounding");
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).paused(true));
    check_series(&mut e, t, c, |e, s| total_time(e, t, s));
    e.anim(t).set_total_time(0.123456789, Emit::Fire);
    assert_eq!(val(&e, 0, X), 12.3456789, "not rounded");
}

#[test]
fn yoyo_repeat_delay_table() {
    // T4 [golden: design_t4_yoyo_repeat_delay_table].
    let c = sample_case("design_t4_yoyo_repeat_delay_table");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 0.0);
    let t = e.to(
        one(0),
        &[to(X, 100.0)],
        lin(1.0).repeat(2).yoyo(true).repeat_delay(0.5).paused(true),
    );
    assert_eq!(e.anim_ref(t).total_duration(), num(c.meta, "totalDuration"));
    check_series(&mut e, t, c, |e, s| total_time(e, t, s));
    e.anim(t).seek(Seek::Time(1.75), Emit::Suppress);
    let r = e.anim_ref(t);
    assert_eq!(
        (r.progress(), r.total_progress(), r.iteration()),
        (0.75, 0.4375, 2)
    );
}

#[test]
fn exact_cycle_boundary_belongs_to_the_ending_iteration() {
    for id in ["design_t4b_exact_cycle_boundary"] {
        let c = sample_case(id);
        let mut e = TweenEngine::new();
        seed_xy(&mut e, 0.0, 0.0);
        let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).repeat(2).paused(true));
        check_series(&mut e, t, c, |e, s| total_time(e, t, s));
    }
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).repeat(2).paused(true));
    e.anim(t).seek(Seek::Time(1.0), Emit::Suppress);
    assert_eq!(val(&e, 0, X), 100.0);
    assert_eq!(e.anim_ref(t).progress(), 1.0);
    assert_eq!(e.anim_ref(t).iteration(), 1);
}

fn yoyo_tween(
    e: &mut TweenEngine,
    ease_: Easing,
    yoyo_ease: Option<YoyoEase>,
    yoyo: bool,
) -> TweenId {
    let mut o = TweenOpts::new()
        .duration(1.0)
        .repeat(1)
        .ease(ease_)
        .paused(true);
    if yoyo {
        o = o.yoyo(true);
    }
    if let Some(y) = yoyo_ease {
        o = o.yoyo_ease(y);
    }
    e.to(one(0), &[to(X, 100.0), to(Y, 50.0)], o)
}

/// The design_* yoyo cases tween x only.
fn yoyo_tween_x(
    e: &mut TweenEngine,
    ease_: Easing,
    yoyo_ease: Option<YoyoEase>,
    yoyo: bool,
) -> TweenId {
    let mut o = TweenOpts::new()
        .duration(1.0)
        .repeat(1)
        .ease(ease_)
        .paused(true);
    if yoyo {
        o = o.yoyo(true);
    }
    if let Some(y) = yoyo_ease {
        o = o.yoyo_ease(y);
    }
    e.to(one(0), &[to(X, 100.0)], o)
}

#[test]
fn yoyo_ease_invert_uses_the_opposite_curve() {
    let out = Easing::OutQuad;
    for (id, e_, y, yoyo) in [
        (
            "design_yoyo_ease_true_power1_out_step_0_05",
            out,
            Some(YoyoEase::Invert),
            true,
        ),
        ("design_yoyo_plain_power1_out", out, None, true),
        (
            "design_yoyo_ease_implies_yoyo",
            Easing::Linear,
            Some(YoyoEase::Invert),
            false,
        ),
        (
            "yoyoEase_power1_out_step_0_05",
            Easing::Linear,
            Some(YoyoEase::Ease(Easing::OutQuad)),
            true,
        ),
        (
            "yoyoEase_true",
            Easing::InQuad,
            Some(YoyoEase::Invert),
            true,
        ),
        ("yoyo_plain_power1_in", Easing::InQuad, None, true),
    ] {
        let c = sample_case(id);
        let mut e = TweenEngine::new();
        seed_xy(&mut e, 0.0, 0.0);
        let t = if id.starts_with("design_") {
            yoyo_tween_x(&mut e, e_, y, yoyo)
        } else {
            yoyo_tween(&mut e, e_, y, yoyo)
        };
        if id == "design_yoyo_ease_implies_yoyo" {
            assert_eq!(e.anim_ref(t).yoyo(), flag(c.meta, "yoyo_getter"));
        }
        check_series(&mut e, t, c, |e, s| total_time(e, t, s));
    }
    // The design's spot values.
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = yoyo_tween(&mut e, out, Some(YoyoEase::Invert), true);
    e.anim(t).set_total_time(1.25, Emit::Fire);
    close(val(&e, 0, X), 56.25, 1e-9, "Invert");
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = yoyo_tween(&mut e, out, None, true);
    e.anim(t).set_total_time(1.25, Emit::Fire);
    close(val(&e, 0, X), 93.75, 1e-9, "plain");
}

#[test]
fn yoyo_ease_deviate_is_path_independent() {
    // GSAP 3.15 re-anchors the reverse ease wherever the local time changes
    // direction; the engine's yoyo ease is a pure function of local time.
    // Where 3.15 flips exactly at the iteration end the two agree (above);
    // a fresh jump or a flip inside a frame differs.
    let rule = |p: f64| 100.0 * (1.0 - Easing::OutQuad.map(1.0 - p));
    for id in [
        "design_yoyo_ease_true_power1_out_fresh",
        "yoyoEase_power1_out_fresh_instance",
    ] {
        let c = sample_case(id);
        let lin_forward = id.starts_with("yoyoEase");
        let mut differs = 0;
        for (i, t) in c.t.iter().enumerate() {
            let mut e = TweenEngine::new();
            seed_xy(&mut e, 0.0, 0.0);
            let (ease_, y) = if lin_forward {
                (Easing::Linear, YoyoEase::Ease(Easing::OutQuad))
            } else {
                (Easing::OutQuad, YoyoEase::Invert)
            };
            let tw = yoyo_tween(&mut e, ease_, Some(y), true);
            e.anim(tw).set_total_time(*t, Emit::Fire);
            let got = val(&e, 0, X);
            let want = if *t <= 1.0 {
                100.0 * ease_.map(*t)
            } else {
                rule(2.0 - t)
            };
            close(got, want, 1e-9, &format!("{id} t={t}"));
            if (got - column(c.series, "x")[i]).abs() > 1e-6 {
                differs += 1;
            }
        }
        assert!(differs > 0, "{id}: GSAP 3.15 differs on the return half");
    }
    let c = sample_case("yoyoEase_power1_out_step_0_03");
    let mut e = TweenEngine::new();
    seed_xy(&mut e, 0.0, 0.0);
    let tw = yoyo_tween(
        &mut e,
        Easing::Linear,
        Some(YoyoEase::Ease(Easing::OutQuad)),
        true,
    );
    let mut differs = 0;
    for (i, t) in c.t.iter().enumerate() {
        e.anim(tw).set_total_time(*t, Emit::Fire);
        let want = if *t <= 1.0 { 100.0 * t } else { rule(2.0 - t) };
        close(val(&e, 0, X), want, 1e-9, &format!("step 0.03 t={t}"));
        if (val(&e, 0, X) - column(c.series, "x")[i]).abs() > 1e-6 {
            differs += 1;
        }
    }
    assert!(differs > 0);
}

#[test]
fn golden_repeat_refresh_renders_the_boundary_first() {
    for id in [
        "design_repeat_refresh_relative_step",
        "design_repeat_refresh_relative_jump",
    ] {
        let c = sample_case(id);
        let mut e = TweenEngine::new();
        seed_xy(&mut e, 0.0, 0.0);
        let t = e.to(
            one(0),
            &[PropTo::by(X, 100.0.into())],
            lin(1.0).repeat(2).repeat_refresh(true).paused(true),
        );
        check_series(&mut e, t, c, |e, s| total_time(e, t, s));
    }
}

#[test]
fn total_duration_formula() {
    let mut e = TweenEngine::new();
    let mk = |e: &mut TweenEngine, o: TweenOpts| e.to(one(0), &[to(X, 1.0)], o.paused(true));
    let a = mk(&mut e, lin(1.0));
    let b = mk(&mut e, lin(1.0).repeat(2).repeat_delay(0.5));
    let c = mk(&mut e, lin(1.0).repeat(-1));
    let d = mk(&mut e, lin(0.0).repeat(3));
    assert_eq!(e.anim_ref(a).total_duration(), 1.0);
    assert_eq!(e.anim_ref(b).total_duration(), 4.0);
    assert_eq!(e.anim_ref(c).total_duration(), INFINITE);
    assert_eq!(e.anim_ref(d).total_duration(), 0.0);
}

/// R.totalTime(R.totalTime() + dt) in the goldens is the engine root's advance.
#[test]
fn golden_repeat_count_is_exact_at_any_dt() {
    let tr = golden::callbacks::TRACES
        .iter()
        .find(|t| t.id == "design_dt_stepping_repeat_counts")
        .unwrap();
    for op in tr.ops {
        let label = op.op;
        let mut o = lin(1.0).events(EventMask::EDGES.with(EventMask::START));
        let (repeat, rd, yoyo) = if label.contains("T4") {
            (2, 0.5, true)
        } else if label.contains("repeat -1") {
            (-1, 0.0, false)
        } else {
            (3, 0.25, label.contains("yoyo"))
        };
        o = o.repeat(repeat).repeat_delay(rd).yoyo(yoyo);
        let dt_of = |k: usize| -> f64 {
            if label.contains("dt_k") {
                (((k as u64 * 7919) % 97 + 1) as f64) / 500.0
            } else if label.contains("dt 1/60") {
                1.0 / 60.0
            } else if label.contains("dt 1/7") {
                1.0 / 7.0
            } else {
                0.37
            }
        };
        let mut e = TweenEngine::new();
        let mut names = Names::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        // A long filler keeps the root busy like the golden's R.
        e.to(one(1), &[to(X, 1.0)], lin(1000.0));
        let tw = e.to(one(0), &[to(X, 100.0)], o.keep(true));
        names.name(tw, "tw");
        let frames = num(op.values, "frames") as usize;
        let mut fired = Vec::new();
        let mut times = Vec::new();
        // The fixture's root emulation R rounds its total time every step
        // (`R.totalTime(R.totalTime() + dt)`); the engine root integrates
        // `dt` unrounded like GSAP's real root, so feed R's own times.
        let mut r = 0.0;
        for k in 0..frames {
            let next = round7(r + dt_of(k));
            e.advance(next - r);
            r = next;
            for ev in names.raw(&mut e) {
                if ev.kind != EventKind::Update {
                    fired.push(names.fmt(&ev));
                    times.push(ev.total_time);
                }
            }
        }
        assert_eq!(fired, op.fired, "{label}");
        for (got, want) in times.iter().zip(op.fired_detail) {
            close(
                *got,
                want.total_time,
                1e-9,
                &format!("{label}: {} total time", want.cb),
            );
        }
        close(val(&e, 0, X), num(op.values, "x"), VAL_TOL, label);
    }
}

#[test]
fn a_big_dt_crossing_a_child_fires_start_and_complete_and_lands_it() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl)
        .to(one(0), &[to(X, 100.0)], lin(0.5).events(CBS), 0.5);
    let c = e.tl(tl).last();
    names.name(c, "c");
    e.advance(2.0);
    assert_eq!(names.drain(&mut e).1, ["c:onStart", "c:onComplete"]);
    assert_eq!(val(&e, 0, X).to_bits(), 100.0f64.to_bits());
}

#[test]
fn infinite_repeat_never_completes() {
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).repeat(-1).events(CBS));
    names.name(t, "t");
    for _ in 0..1000 {
        e.advance(1.0 / 60.0);
    }
    let (_, fired) = names.drain(&mut e);
    assert!(!fired.iter().any(|s| s.ends_with("onComplete")));
    assert_eq!(fired.iter().filter(|s| s.ends_with("onRepeat")).count(), 16);
    assert!(e.is_active());
    assert!(e.anim_ref(t).is_active());
}

#[test]
fn golden_timescale_children_match_gsap() {
    for c in golden::timescale::CASES.iter().filter(|c| !c.t.is_empty()) {
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        let outer = e.timeline(TimelineOpts::new().paused(true));
        let inner = match c.id {
            "tween_child_timescale_2" | "tween_child_reversed" | "negative_timescale_child" => {
                let tw = e.to(one(0), &[to(X, 100.0)], lin(1.0));
                if c.id == "tween_child_timescale_2" {
                    // The generator also adds a 1 s filler (not in gsap_calls).
                    e.tl(outer).to(one(9), &[to(X, 1.0)], lin(1.0), 0.0);
                }
                match c.id {
                    "tween_child_timescale_2" => {
                        e.anim(tw).set_time_scale(2.0);
                    }
                    "tween_child_reversed" => {
                        e.anim(tw).set_reversed(true);
                    }
                    _ => {}
                }
                e.tl(outer).add(tw, 0.0);
                if c.id == "negative_timescale_child" {
                    e.anim(tw).set_time_scale(-1.0);
                }
                tw
            }
            "three_levels_outer_ts2_inner_ts0_5" => {
                let mid = e.timeline(TimelineOpts::new());
                let inner = e.timeline(TimelineOpts::new());
                e.tl(inner)
                    .to(one(0), &[to(X, 100.0)], lin(2.0), Position::END);
                e.anim(inner).set_time_scale(0.5);
                e.tl(mid).add(inner, 0.0);
                e.anim(mid).set_time_scale(2.0);
                e.tl(outer).add(mid, 0.0);
                assert_eq!(e.anim_ref(mid).start_time(), num(c.meta, "mid_startTime"));
                assert_eq!(e.anim_ref(mid).end_time(true), num(c.meta, "mid_endTime"));
                assert_eq!(e.anim_ref(mid).duration(), num(c.meta, "mid_duration"));
                inner
            }
            _ => {
                let repeat = c.id.contains("repeat1");
                let inner = e.timeline(TimelineOpts::new().repeat(i32::from(repeat)).yoyo(repeat));
                e.tl(inner)
                    .to(one(0), &[to(X, 100.0)], lin(2.0), Position::END);
                match c.id {
                    "inner_timescale_2" | "inner_repeat1_yoyo_timescale_2" => {
                        e.anim(inner).set_time_scale(2.0);
                    }
                    "inner_timescale_0_5" => {
                        e.anim(inner).set_time_scale(0.5);
                    }
                    "inner_reversed" => {
                        e.anim(inner).set_reversed(true);
                    }
                    "inner_reversed_timescale_0_5" => {
                        e.anim(inner).set_time_scale(0.5).set_reversed(true);
                    }
                    other => panic!("{other}"),
                }
                e.tl(outer).add(inner, 1.0);
                inner
            }
        };
        let meta: Pairs = c.meta;
        let m = |k: &str| golden::common::val(meta, k);
        if let Some(golden::common::Val::F(d)) = m("outer_duration") {
            assert_eq!(e.anim_ref(outer).duration(), d, "{}", c.id);
        }
        for (k, got) in [
            ("inner_startTime", e.anim_ref(inner).start_time()),
            ("tween_startTime", e.anim_ref(inner).start_time()),
            ("inner_endTime", e.anim_ref(inner).end_time(true)),
            ("tween_endTime", e.anim_ref(inner).end_time(true)),
            ("inner_timeScale", e.anim_ref(inner).time_scale()),
            ("inner_duration", e.anim_ref(inner).duration()),
            ("inner_totalDuration", e.anim_ref(inner).total_duration()),
        ] {
            if let Some(golden::common::Val::F(want)) = m(k) {
                assert_eq!(got, want, "{}: {k}", c.id);
            }
        }
        if let Some(golden::common::Val::B(want)) = m("inner_reversed") {
            assert_eq!(e.anim_ref(inner).reversed(), want, "{}", c.id);
        }
        let series: &[Series] = c.series;
        for (i, t) in c.t.iter().enumerate() {
            e.anim(outer).set_total_time(*t, Emit::Fire);
            for s in series {
                let got = match s.name {
                    "outer.time" => e.anim_ref(outer).time(),
                    "inner.time" => e.anim_ref(inner).time(),
                    "inner.totalTime" => e.anim_ref(inner).total_time(),
                    "inner.progress" => e.anim_ref(inner).progress(),
                    "tween_time" => e.anim_ref(inner).time(),
                    "x" => val(&e, 0, X),
                    other => panic!("{other}"),
                };
                let tol = if s.name == "x" { VAL_TOL } else { TIME_TOL };
                close(got, s.v[i], tol, &format!("{}: {} at {t}", c.id, s.name));
            }
        }
    }
}

#[test]
fn golden_negative_time_scale_is_reversal() {
    let c = golden::timescale::CASES
        .iter()
        .find(|c| c.id == "negative_timescale")
        .unwrap();
    let mut e = TweenEngine::new();
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0).paused(true));
    for op in c.ops {
        match text(op, "op").unwrap() {
            "created" => {}
            "timeScale(-1)" => {
                e.anim(t).set_time_scale(-1.0);
            }
            "timeScale(-2)" => {
                e.anim(t).set_time_scale(-2.0);
            }
            "timeScale(0)" => {
                e.anim(t).set_time_scale(0.0);
            }
            "timeScale(1)" => {
                e.anim(t).set_time_scale(1.0);
            }
            "reversed(false)" => {
                e.anim(t).set_reversed(false);
            }
            "reversed(true)" | "reversed(true) while timeScale 0" => {
                e.anim(t).set_reversed(true);
            }
            other => panic!("{other}"),
        }
        let r = e.anim_ref(t);
        let name = text(op, "op").unwrap();
        assert_eq!(r.time_scale(), num(op, "timeScale"), "{name}");
        assert_eq!(r.reversed(), flag(op, "reversed"), "{name}");
        assert_eq!(r.paused(), flag(op, "paused"), "{name}");
    }
}

/// The time scale cases that drive a smooth root emulation R: the engine
/// root is R; it settles its own time shift on the next frame (advance(0)).
#[test]
fn golden_nested_time_scales_compose() {
    let c = golden::timescale::CASES
        .iter()
        .find(|c| c.id == "design_t10_nested_time_scales")
        .unwrap();
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new());
    let tl2 = e.timeline(TimelineOpts::new());
    e.tl(tl2)
        .to(one(0), &[to(X, 100.0)], lin(2.0), Position::END);
    e.anim(tl2).set_time_scale(2.0);
    e.tl(tl).add(tl2, 1.0);
    for op in c.ops {
        let name = text(op, "op").unwrap();
        match name {
            "R +1.5" => e.advance(1.5),
            "R +0.5" => e.advance(0.5),
            "tl.timeScale(0.5)" => {
                e.anim(tl).set_time_scale(0.5);
                e.advance(0.0);
            }
            "tl.reverse()" => {
                e.anim(tl).reverse();
                e.advance(0.0);
            }
            other => panic!("{other}"),
        }
        let r = e.anim_ref(tl);
        close(val(&e, 0, X), num(op, "x"), VAL_TOL, name);
        close(r.time(), num(op, "tl_time"), TIME_TOL, name);
        assert_eq!(r.time_scale(), num(op, "tl_time_scale"), "{name}");
        assert_eq!(r.reversed(), flag(op, "tl_reversed"), "{name}");
        close(r.start_time(), num(op, "tl_start"), TIME_TOL, name);
    }
}

#[test]
fn golden_time_scale_change_does_not_jump() {
    let c = golden::timescale::CASES
        .iter()
        .find(|c| c.id == "design_time_scale_no_jump_root_child")
        .unwrap();
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 100.0)], lin(1.0));
    let mut root = 0.0;
    for op in c.ops {
        let name = text(op, "op").unwrap();
        match name {
            "R.totalTime(0.5)" | "R.totalTime(0.6)" | "R.totalTime(0.7)" => {
                let target: f64 = name[12..15].parse().unwrap();
                e.advance(target - root);
                root = target;
            }
            "t.timeScale(2)" => {
                e.anim(t).set_time_scale(2.0);
            }
            "t.timeScale(-1)" => {
                e.anim(t).set_time_scale(-1.0);
            }
            other => panic!("{other}"),
        }
        let r = e.anim_ref(t);
        close(val(&e, 0, X), num(op, "x"), VAL_TOL, name);
        close(r.total_time(), num(op, "t_total_time"), 1e-9, name);
        close(r.start_time(), num(op, "t_start"), TIME_TOL, name);
        assert_eq!(r.time_scale(), num(op, "t_time_scale"), "{name}");
        assert_eq!(r.reversed(), flag(op, "t_reversed"), "{name}");
    }

    let c = golden::timescale::CASES
        .iter()
        .find(|c| c.id == "design_time_scale_no_jump_nested_smooth")
        .unwrap();
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new().smooth_child_timing(true));
    e.tl(tl)
        .to(one(0), &[to(X, 100.0)], lin(1.0), Position::END);
    let t = e.tl(tl).last();
    for op in c.ops {
        let name = text(op, "op").unwrap();
        match name {
            "R +0.5" => e.advance(0.5),
            "t.timeScale(0.5)" => {
                e.anim(t).set_time_scale(0.5);
                e.advance(0.0);
            }
            other => panic!("{other}"),
        }
        close(val(&e, 0, X), num(op, "x"), VAL_TOL, name);
        close(
            e.anim_ref(tl).duration(),
            num(op, "tl_duration"),
            TIME_TOL,
            name,
        );
        close(
            e.anim_ref(t).start_time(),
            num(op, "t_start"),
            TIME_TOL,
            name,
        );
        close(
            e.anim_ref(t).total_time(),
            num(op, "t_total_time"),
            TIME_TOL,
            name,
        );
    }
}

#[test]
fn golden_smooth_child_timing_recomputes_the_parent() {
    for (id, smooth) in [
        ("design_smooth_child_timing_duration", true),
        ("design_non_smooth_child_timing_duration", false),
    ] {
        let c = golden::timescale::CASES
            .iter()
            .find(|c| c.id == id)
            .unwrap();
        let mut e = TweenEngine::new();
        let tl = e.timeline(TimelineOpts::new().paused(true).smooth_child_timing(smooth));
        e.tl(tl).to(one(0), &[to(X, 1.0)], lin(1.0), Position::END);
        let ch = e.tl(tl).last();
        for op in c.ops {
            let name = text(op, "op").unwrap();
            if name == "c.timeScale(0.5)" {
                e.anim(ch).set_time_scale(0.5);
            }
            assert_eq!(
                e.anim_ref(tl).duration(),
                num(op, "tl_duration"),
                "{id} {name}"
            );
            assert_eq!(
                e.anim_ref(ch).start_time(),
                num(op, "c_start"),
                "{id} {name}"
            );
            assert_eq!(
                e.anim_ref(ch).end_time(true),
                num(op, "c_end"),
                "{id} {name}"
            );
        }
    }
}

#[test]
fn paused_child_does_not_count_in_duration() {
    let mut e = TweenEngine::new();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(one(0), &[to(X, 1.0)], lin(1.0), 0.0);
    let b = e.to(one(1), &[to(X, 1.0)], lin(2.0).paused(true));
    e.tl(tl).add(b, 0.0);
    assert_eq!(e.anim_ref(tl).duration(), 1.0);
    e.anim(b).resume();
    assert_eq!(e.anim_ref(tl).duration(), 2.0);
}

#[test]
fn split_dt_matches_single_dt() {
    let run = |steps: usize, dt: f64| {
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        e.to(one(0), &[to(X, 100.0)], lin(1.0));
        for _ in 0..steps {
            e.advance(dt);
        }
        val(&e, 0, X)
    };
    let one_step = run(1, 0.5);
    assert!((run(50, 0.01) - one_step).abs() <= 1e-5);
    assert_eq!(run(64, 1.0 / 128.0).to_bits(), one_step.to_bits());
}

#[test]
fn same_dt_sequence_same_bits() {
    let run = || {
        let mut e = TweenEngine::new();
        seed_all(&mut e, 8, X, 0.0);
        for i in 0..8u32 {
            e.to(
                one(i),
                &[to(X, 10.0 * i as f64)],
                TweenOpts::new()
                    .duration(0.3 + 0.1 * i as f64)
                    .ease(Easing::OutElastic)
                    .repeat(1)
                    .yoyo(true),
            );
        }
        let mut out = Vec::new();
        let mut k = 1u64;
        for _ in 0..200 {
            k = splitmix64(k);
            e.advance((k % 1000) as f64 / 40000.0);
            for i in 0..8 {
                out.push(val(&e, i, X).to_bits());
            }
        }
        out
    };
    assert_eq!(run(), run());
}

#[test]
fn landing_is_exact_for_every_ease() {
    let eases = [
        Easing::InBack,
        Easing::OutBack,
        Easing::Constant(0.3),
        Easing::Instant,
        Easing::css_preset("ease_in_out_back").unwrap(),
        Easing::css_preset("ease_out_quad").unwrap(),
    ];
    for ease_ in eases {
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(3.0));
        e.to(
            one(0),
            &[to(X, 7.1)],
            TweenOpts::new().duration(0.3).ease(ease_),
        );
        for _ in 0..40 {
            e.advance(1.0 / 60.0);
        }
        assert_eq!(val(&e, 0, X).to_bits(), 7.1f64.to_bits(), "{ease_:?}");
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(3.0));
        e.to(
            one(0),
            &[to(X, 7.1)],
            TweenOpts::new()
                .duration(0.3)
                .ease(ease_)
                .repeat(1)
                .yoyo(true),
        );
        for _ in 0..60 {
            e.advance(1.0 / 60.0);
        }
        assert_eq!(val(&e, 0, X).to_bits(), 3.0f64.to_bits(), "yoyo {ease_:?}");
    }
}

#[test]
fn nested_time_scales_compose() {
    // T10 by frames: tl2 (ts 2) at 1 in tl, a 2 s tween.
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(TimelineOpts::new());
    let tl2 = e.timeline(TimelineOpts::new());
    e.tl(tl2)
        .to(one(0), &[to(X, 100.0)], lin(2.0), Position::END);
    e.anim(tl2).set_time_scale(2.0);
    e.tl(tl).add(tl2, 1.0);
    e.advance(1.5);
    close(val(&e, 0, X), 50.0, 1e-9, "1.5");
    e.anim(tl).set_time_scale(0.5);
    e.advance(0.5);
    close(val(&e, 0, X), 75.0, 1e-9, "after slow-down");
    e.anim(tl).reverse();
    e.advance(0.5);
    close(val(&e, 0, X), 50.0, 1e-9, "reversed");
}

#[test]
fn the_root_clock_does_not_drift() {
    // GSAP's root reads the absolute ticker time, so its 1e-7 rounding never
    // accumulates: 120 frames of 1/120 s complete a 1 s tween.
    let mut e = TweenEngine::new();
    let mut names = Names::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(
        one(0),
        &[to(X, 1.0)],
        lin(1.0).events(EventMask::COMPLETE).keep(true),
    );
    names.name(t, "t");
    for _ in 0..120 {
        e.advance(1.0 / 120.0);
    }
    let (_, fired) = names.drain(&mut e);
    assert_eq!(fired, ["t:onComplete"]);
    assert_eq!(e.anim_ref(t).progress(), 1.0);
    // Ten minutes at 144 Hz stay within a microsecond.
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let t = e.to(one(0), &[to(X, 600.0)], lin(600.0));
    for _ in 0..(600 * 144 - 1) {
        e.advance(1.0 / 144.0);
    }
    close(
        e.anim_ref(t).total_time(),
        600.0 - 1.0 / 144.0,
        1e-6,
        "drift",
    );
}
