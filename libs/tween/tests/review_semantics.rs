//! Adversarial review, lens "semantics vs GSAP 3.15": replays golden cases
//! (gsap/out/*.json, emitted as Rust in tests/golden) through the public
//! engine API and pins the tricky corners named in the review brief.
//!
//! Every test cites its golden case. Documented deviations of the design
//! (GSAP 3.15 `time(v)` landing in the first iteration, the missing
//! `tl:onComplete` after a sweep that ends on `tt`, the extra `onUpdate` of a
//! re-render at exactly 0, value rounding to 1e-6, path-dependent yoyoEase)
//! are skipped or compared without the affected field, and say so.

use makepad_tween::*;

#[allow(dead_code, unused_imports)]
mod golden {
    pub mod callbacks;
    pub mod common;
    pub mod overwrite;
    pub mod positions;
    pub mod samples;
    pub mod timescale;
}

use golden::callbacks::{Op, Trace, TRACES};
use golden::common::{column, num, Val};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Every callback owner the goldens name, in a fixed order: the tag of a name
/// is its index + 1.
const NAMES: &[&str] = &[
    "tl", "a", "b", "inner", "mark", "set", "zero", "z1", "z2", "z3", "z4", "t", "tw", "u", "tl2",
    "c", "pause", "t1", "t2", "t3", "to0", "fromTo", "from", "d", "cb",
];

fn tag(name: &str) -> Tag {
    let i = NAMES
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("unknown callback owner {name}"));
    Tag(i as u64 + 1)
}

fn owner(t: Tag) -> &'static str {
    NAMES
        .get((t.0 as usize).wrapping_sub(1))
        .copied()
        .unwrap_or("?")
}

/// Every GSAP callback the goldens record (onStart .. onInterrupt).
const EV: EventMask = EventMask::EDGES.with(EventMask::UPDATE);

const X: PropKey = PropKey(1);
const Y: PropKey = PropKey(2);

fn tid(i: u32) -> Targets<'static> {
    Targets::One(TargetId(i))
}

fn seed(e: &mut TweenEngine, t: u32, p: PropKey, v: f64) {
    e.seed(TargetId(t), p, TweenValue::F64(v));
}

fn val(e: &TweenEngine, t: u32, p: PropKey) -> f64 {
    e.get_f64(TargetId(t), p).unwrap_or(f64::NAN)
}

fn lin(d: f64) -> TweenOpts {
    TweenOpts::new().duration(d).ease(Easing::Linear)
}

/// The queued events as the goldens spell them ("a:onStart", "call:mark"),
/// then clears the queue.
fn drain(e: &mut TweenEngine, updates: bool) -> Vec<String> {
    let out = e
        .events()
        .iter()
        .filter_map(|ev| {
            let who = owner(ev.tag);
            Some(match ev.kind {
                EventKind::Start => format!("{who}:onStart"),
                EventKind::Update if updates => format!("{who}:onUpdate"),
                EventKind::Update => return None,
                EventKind::Repeat => format!("{who}:onRepeat"),
                EventKind::Complete => format!("{who}:onComplete"),
                EventKind::ReverseComplete => format!("{who}:onReverseComplete"),
                EventKind::Interrupt => format!("{who}:onInterrupt"),
                EventKind::Call { .. } => format!("call:{who}"),
                EventKind::Pause => format!("{who}:cb"),
                EventKind::Label(_) => return None,
            })
        })
        .collect();
    e.clear_events();
    out
}

fn close(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps || (a.is_nan() && b.is_nan())
}

/// Runs one golden op string on `id` (a timeline or tween). `None`: the op
/// is a documented deviation (or not a playhead op) and the replay stops.
fn run_op(e: &mut TweenEngine, id: TweenId, op: &str) -> Option<()> {
    let op = op.split("  ").next().unwrap().trim();
    if let Some((first, rest)) = op.split_once(" then ") {
        run_op(e, id, first)?;
        return run_op(e, id, rest);
    }
    let (name, args) = op.split_once('(')?;
    let args = args.strip_suffix(')')?;
    let parts: Vec<&str> = args
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let arg = |i: usize| parts[i].parse::<f64>().unwrap();
    let second = parts.get(1).copied();
    let mut a = e.anim(id);
    match name {
        "seek" => {
            let ev = if second == Some("false") {
                Emit::Fire
            } else {
                Emit::Suppress
            };
            a.seek(Seek::Time(arg(0)), ev);
        }
        "totalTime" => {
            let ev = if second == Some("true") {
                Emit::Suppress
            } else {
                Emit::Fire
            };
            a.set_total_time(arg(0), ev);
        }
        "progress" => {
            a.set_progress(arg(0), Emit::Fire);
        }
        "reverse" => {
            a.reverse();
        }
        "restart" => {
            a.restart(false, Emit::Suppress);
        }
        "play" => {
            a.play();
        }
        // time(v): GSAP 3.15 lands in the FIRST iteration; the design keeps the
        // current one (documented deviation, C14).
        _ => return None,
    }
    Some(())
}

/// Compares the timeline state recorded after a golden op.
fn check_state(e: &TweenEngine, id: TweenId, op: &Op, errs: &mut Vec<String>) {
    let Some(s) = op.state else { return };
    let r = e.anim_ref(id);
    let got = [
        ("time", r.time(), s.time),
        ("total_time", r.total_time(), s.total_time),
        ("progress", r.progress(), s.progress),
        ("total_progress", r.total_progress(), s.total_progress),
        ("iteration", r.iteration() as f64, s.iteration),
    ];
    for (k, g, w) in got {
        if !close(g, w, 1e-9) {
            errs.push(format!("{}: {k} = {g}, GSAP {w}", op.op));
        }
    }
    if r.reversed() != s.reversed {
        errs.push(format!(
            "{}: reversed = {}, GSAP {}",
            op.op,
            r.reversed(),
            s.reversed
        ));
    }
    if r.paused() != s.paused {
        errs.push(format!(
            "{}: paused = {}, GSAP {}",
            op.op,
            r.paused(),
            s.paused
        ));
    }
}

fn check_values(e: &TweenEngine, op: &Op, map: &[(&str, u32, PropKey)], errs: &mut Vec<String>) {
    for (k, v) in op.values {
        let Val::F(want) = v else { continue };
        if let Some((_, t, p)) = map.iter().find(|(n, _, _)| n == k) {
            let got = val(e, *t, *p);
            if !close(got, *want, 5e-7) {
                errs.push(format!("{}: {k} = {got}, GSAP {want}", op.op));
            }
        }
    }
}

/// Compares each queued non-Update event's `total_time` with the firing
/// animation's `totalTime()` read inside the GSAP callback (`fired_detail`).
/// Only runs when the event names line up with the golden's.
fn check_detail(e: &TweenEngine, op: &Op, errs: &mut Vec<String>) {
    let evs: Vec<&TweenEvent> = e
        .events()
        .iter()
        .filter(|ev| !matches!(ev.kind, EventKind::Update | EventKind::Label(_)))
        .collect();
    if evs.len() != op.fired_detail.len() {
        return;
    }
    for (ev, d) in evs.iter().zip(op.fired_detail) {
        if !close(ev.total_time, d.total_time, 1e-9) {
            errs.push(format!(
                "{}: {} event total_time {}, GSAP {} ({:?})",
                op.op, d.cb, ev.total_time, d.total_time, ev.kind
            ));
        }
    }
}

fn trace(id: &str) -> &'static Trace {
    TRACES
        .iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| panic!("no trace {id}"))
}

/// Replays `trace` on the animation `id`: callbacks (with or without
/// onUpdate), values and state after every op, until the first op the
/// interpreter does not run. Returns every mismatch.
fn replay(
    e: &mut TweenEngine,
    id: TweenId,
    trace: &Trace,
    map: &[(&str, u32, PropKey)],
    updates: bool,
) -> Vec<String> {
    let mut errs = Vec::new();
    drain(e, updates);
    for op in trace.ops {
        if run_op(e, id, op.op).is_none() {
            break;
        }
        check_detail(e, op, &mut errs);
        let got = drain(e, updates);
        let want: Vec<String> = if updates { op.fired_all } else { op.fired }
            .iter()
            .map(|s| s.to_string())
            .collect();
        if got != want {
            errs.push(format!("{}: fired {got:?}, GSAP {want:?}", op.op));
        }
        check_values(e, op, map, &mut errs);
        check_state(e, id, op, &mut errs);
    }
    errs
}

fn assert_clean(case: &str, errs: Vec<String>) {
    assert!(
        errs.is_empty(),
        "{case}: {} mismatch(es) vs GSAP 3.15:\n  {}",
        errs.len(),
        errs.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// The callbacks.json main timeline
// ---------------------------------------------------------------------------

/// `tl` {paused, repeat 1}: a 0..2 (1 s, repeat 1, yoyo), b 2.5..3.5,
/// call "mark" at 1.25, label "mid" at 2, inner 3..4 (c).
fn main_timeline(e: &mut TweenEngine) -> TweenId {
    for t in 1..=3 {
        seed(e, t, X, 0.0);
    }
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(1)
            .tag(tag("tl"))
            .events(EV),
    );
    let inner = e.timeline(TimelineOpts::new().tag(tag("inner")).events(EV));
    e.tl(inner)
        .to(tid(3), &[PropTo::to_f64(X, 1.0)], lin(1.0), Position::END);
    e.tl(tl)
        .to(
            tid(1),
            &[PropTo::to_f64(X, 1.0)],
            lin(1.0).repeat(1).yoyo(true).tag(tag("a")).events(EV),
            Position::END,
        )
        .to(
            tid(2),
            &[PropTo::to_f64(X, 1.0)],
            lin(1.0).tag(tag("b")).events(EV),
            Position::rel(0.5),
        )
        .call(tag("mark"), 1.25)
        .add_label(Tag(999), 2.0)
        .add(inner, 3.0);
    tl
}

const ABC: &[(&str, u32, PropKey)] = &[("a", 1, X), ("b", 2, X), ("c", 3, X)];

fn main_trace(id: &str) -> Vec<String> {
    let mut e = TweenEngine::new();
    let tl = main_timeline(&mut e);
    assert_eq!(e.anim_ref(tl).duration(), 4.0);
    assert_eq!(e.anim_ref(tl).total_duration(), 8.0);
    replay(&mut e, tl, trace(id), ABC, true)
}

/// [callbacks main_ops_as_specified] every op up to `time(0.7)`: seeks,
/// progress(1), reverse(), a backward totalTime scrub 3.75 -> 0, restart(),
/// play(), a forward scrub 0.25 -> 8 across the timeline repeat.
#[test]
fn golden_main_ops_as_specified() {
    assert_clean("main_ops_as_specified", main_trace("main_ops_as_specified"));
}

/// [callbacks main_ops_seek_events] the same script with seek(t, false).
#[test]
fn golden_main_ops_seek_events() {
    assert_clean("main_ops_seek_events", main_trace("main_ops_seek_events"));
}

/// [callbacks setter_seek_false] 0.7, 1.5, 0.2, 5.5 (iteration 2), 0.7: a
/// big backward jump across the timeline's own repeat boundary.
#[test]
fn golden_setter_seek_false() {
    assert_clean("setter_seek_false", main_trace("setter_seek_false"));
}

/// [callbacks setter_seek_default]
#[test]
fn golden_setter_seek_default() {
    assert_clean("setter_seek_default", main_trace("setter_seek_default"));
}

/// [callbacks setter_totalTime]
#[test]
fn golden_setter_total_time() {
    assert_clean("setter_totalTime", main_trace("setter_totalTime"));
}

/// [callbacks setter_totalTime_suppress]
#[test]
fn golden_setter_total_time_suppress() {
    assert_clean(
        "setter_totalTime_suppress",
        main_trace("setter_totalTime_suppress"),
    );
}

// ---------------------------------------------------------------------------
// Zero-duration direction rule
// ---------------------------------------------------------------------------

/// [callbacks zero_duration_in_timeline] tl.set at 1 and a 0 s to() at 1.5 on
/// a paused timeline: arrive-or-leave firing, "back to exactly 1 fires
/// Complete again", leaving silently, and a suppressed seek then a Fire seek.
#[test]
fn golden_zero_duration_in_timeline() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0); // d (filler)
    seed(&mut e, 2, X, 0.0); // z.x
    seed(&mut e, 2, Y, 0.0); // z.y
    let tl = e.timeline(
        TimelineOpts::new().paused(true).tag(tag("tl")).events(
            EventMask::START
                .with(EventMask::COMPLETE)
                .with(EventMask::REVERSE_COMPLETE),
        ),
    );
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 1.0)], lin(2.0), 0.0)
        .set(
            tid(2),
            &[PropTo::to_f64(X, 1.0)],
            TweenOpts::new().tag(tag("set")).events(EV),
            1.0,
        )
        .to(
            tid(2),
            &[PropTo::to_f64(Y, 1.0)],
            TweenOpts::new().duration(0.0).tag(tag("zero")).events(EV),
            1.5,
        );
    let map = [("z_x", 2, X), ("z_y", 2, Y)];
    let t = trace("zero_duration_in_timeline");
    // Skip the "created" pseudo-op; the last op runs a hidden
    // `tl.totalTime(0.9)` first (golden.js clears the log after it).
    let (last, body) = t.ops[1..].split_last().unwrap();
    let rest = Trace { ops: body, ..*t };
    let mut errs = replay(&mut e, tl, &rest, &map, true);
    assert_eq!(last.op, "totalTime(1, true) then totalTime(1.1)");
    e.anim(tl).set_total_time(0.9, Emit::Fire);
    drain(&mut e, true);
    e.anim(tl).set_total_time(1.0, Emit::Suppress);
    e.anim(tl).set_total_time(1.1, Emit::Fire);
    let got = drain(&mut e, true);
    if got != last.fired_all {
        errs.push(format!(
            "{}: fired {got:?}, GSAP {:?}",
            last.op, last.fired_all
        ));
    }
    check_values(&e, last, &map, &mut errs);
    check_state(&e, tl, last, &mut errs);
    assert_clean("zero_duration_in_timeline", errs);
}

/// [callbacks zero_duration_standalone] a 0 s to() on the root fires Update +
/// Complete at creation; paused: nothing until totalTime(0); progress(1)
/// then fires nothing; gsap.set fires Complete; a 0 s child at 0 of a paused
/// empty timeline renders on tl.totalTime(0) and then never again.
#[test]
fn golden_zero_duration_standalone() {
    let mut e = TweenEngine::new();
    for t in 1..=4 {
        seed(&mut e, t, X, 0.0);
    }
    let zo = |n: &str| TweenOpts::new().duration(0.0).tag(tag(n)).events(EV);
    let mut errs = Vec::new();
    fn expect(
        errs: &mut Vec<String>,
        e: &mut TweenEngine,
        what: &str,
        want: &[&str],
        x: (u32, f64),
    ) {
        let got = drain(e, true);
        if got != want {
            errs.push(format!("{what}: fired {got:?}, GSAP {want:?}"));
        }
        let v = val(e, x.0, X);
        if v != x.1 {
            errs.push(format!("{what}: x = {v}, GSAP {}", x.1));
        }
    }
    e.to(tid(1), &[PropTo::to_f64(X, 5.0)], zo("z1"));
    expect(
        &mut errs,
        &mut e,
        "z1 create",
        &["z1:onUpdate", "z1:onComplete"],
        (1, 5.0),
    );
    let z2 = e.to(tid(2), &[PropTo::to_f64(X, 5.0)], zo("z2").paused(true));
    expect(&mut errs, &mut e, "z2 create", &[], (2, 0.0));
    e.anim(z2).set_total_time(0.0, Emit::Fire);
    expect(
        &mut errs,
        &mut e,
        "z2 totalTime(0)",
        &["z2:onUpdate", "z2:onComplete"],
        (2, 5.0),
    );
    e.anim(z2).set_progress(1.0, Emit::Fire);
    expect(&mut errs, &mut e, "z2 progress(1)", &[], (2, 5.0));
    // GSAP then reads progress 0 / totalProgress 1 on z2; here z2 completed
    // on the root and was freed (design D9: bare tweens are freed when they
    // complete, even paused ones), so its handle is stale and the following
    // controls are no-ops. The callbacks still match (none fire).
    e.anim(z2).restart(false, Emit::Suppress);
    expect(&mut errs, &mut e, "z2 restart()", &[], (2, 5.0));
    e.set(
        tid(3),
        &[PropTo::to_f64(X, 7.0)],
        TweenOpts::new().tag(tag("z3")).events(EventMask::COMPLETE),
    );
    expect(
        &mut errs,
        &mut e,
        "gsap.set z3",
        &["z3:onComplete"],
        (3, 7.0),
    );
    let tl4 = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl4)
        .to(tid(4), &[PropTo::to_f64(X, 5.0)], zo("z4"), 0.0);
    expect(&mut errs, &mut e, "tl4 build", &[], (4, 0.0));
    e.anim(tl4).set_total_time(0.0, Emit::Fire);
    expect(
        &mut errs,
        &mut e,
        "tl4.totalTime(0)",
        &["z4:onUpdate", "z4:onComplete"],
        (4, 5.0),
    );
    e.anim(tl4).set_total_time(0.0, Emit::Fire);
    expect(&mut errs, &mut e, "tl4.totalTime(0) again", &[], (4, 5.0));
    e.anim(tl4).seek(Seek::Time(0.0), Emit::Suppress);
    expect(&mut errs, &mut e, "tl4.seek(0)", &[], (4, 5.0));
    let tp = e.anim_ref(tl4).total_progress();
    if tp != 1.0 {
        errs.push(format!("tl4 total_progress {tp}, GSAP 1 (initted, dur 0)"));
    }
    assert_clean("zero_duration_standalone", errs);
}

// ---------------------------------------------------------------------------
// immediateRender variants
// ---------------------------------------------------------------------------

/// [callbacks.immediate_render two_froms_same_prop] (T13) two from()s on the
/// same slot, 0..1 and 1..2. At seek(1) GSAP shows 50: the second from() is
/// rendered at its own total 0 and writes its from value over the first
/// one's end. The playhead gives [[0,50],[.25,75],[.5,50],[.75,25],[1,50],
/// [1.5,75],[2,100],[0,100]].
#[test]
fn golden_two_froms_same_prop() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).from(
        tid(1),
        &[PropTo::from(X, TweenValue::F64(100.0))],
        lin(1.0),
        Position::END,
    );
    let after_first = val(&e, 1, X);
    e.tl(tl).from(
        tid(1),
        &[PropTo::from(X, TweenValue::F64(50.0))],
        lin(1.0),
        Position::END,
    );
    let mut errs = Vec::new();
    if after_first != 100.0 {
        errs.push(format!("after first create x = {after_first}, GSAP 100"));
    }
    let after = val(&e, 1, X);
    if after != 50.0 {
        errs.push(format!("after create x = {after}, GSAP 50"));
    }
    for (t, want) in [
        (0.0, 50.0),
        (0.25, 75.0),
        (0.5, 50.0),
        (0.75, 25.0),
        (1.0, 50.0),
        (1.5, 75.0),
        (2.0, 100.0),
        (0.0, 100.0),
    ] {
        e.anim(tl).seek(Seek::Time(t), Emit::Suppress);
        let got = val(&e, 1, X);
        if !close(got, want, 5e-7) {
            errs.push(format!("seek({t}) x = {got}, GSAP {want}"));
        }
    }
    assert_clean("two_froms_same_prop", errs);
}

/// [callbacks.immediate_render two_froms_same_prop_immediateRender_false_on_second]
#[test]
fn golden_two_froms_second_not_immediate() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .from(
            tid(1),
            &[PropTo::from(X, TweenValue::F64(100.0))],
            lin(1.0),
            Position::END,
        )
        .from(
            tid(1),
            &[PropTo::from(X, TweenValue::F64(50.0))],
            lin(1.0).immediate_render(false),
            Position::END,
        );
    let mut errs = Vec::new();
    let after = val(&e, 1, X);
    if after != 100.0 {
        errs.push(format!("after create x = {after}, GSAP 100"));
    }
    for (t, want) in [
        (0.0, 100.0),
        (0.25, 75.0),
        (0.5, 50.0),
        (0.75, 25.0),
        (1.0, 50.0),
        (1.5, 25.0),
        (2.0, 0.0),
        (0.0, 100.0),
    ] {
        e.anim(tl).seek(Seek::Time(t), Emit::Suppress);
        let got = val(&e, 1, X);
        if !close(got, want, 5e-7) {
            errs.push(format!("seek({t}) x = {got}, GSAP {want}"));
        }
    }
    assert_clean("two_froms_same_prop_immediateRender_false_on_second", errs);
}

/// [callbacks.immediate_render timeline_from_at_2] tl.from at 2 renders its
/// from value at once; seek(1) keeps it, seek(2.5) -> 25, seek(0) -> 100.
#[test]
fn golden_timeline_from_at_2() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).from(
        tid(1),
        &[PropTo::from(X, TweenValue::F64(100.0))],
        TweenOpts::new().duration(1.0),
        2.0,
    );
    let mut errs = Vec::new();
    for (what, t, want) in [
        ("create", None, 100.0),
        ("seek(1)", Some(1.0), 100.0),
        ("seek(2.5)", Some(2.5), 25.0),
        ("seek(0)", Some(0.0), 100.0),
    ] {
        if let Some(t) = t {
            e.anim(tl).seek(Seek::Time(t), Emit::Suppress);
        }
        let got = val(&e, 1, X);
        if !close(got, want, 5e-7) {
            errs.push(format!("{what}: x = {got}, GSAP {want}"));
        }
    }
    assert_clean("timeline_from_at_2", errs);
}

/// [callbacks.immediate_render timeline_set_at_0] tl.set at 0 is not
/// immediate; seek(0) applies it (crossing the start); 0.01 keeps it.
#[test]
fn golden_timeline_set_at_0() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .set(tid(1), &[PropTo::to_f64(X, 5.0)], TweenOpts::new(), 0.0);
    let mut errs = Vec::new();
    let c = val(&e, 1, X);
    if c != 0.0 {
        errs.push(format!("create: x = {c}, GSAP 0"));
    }
    e.anim(tl).seek(Seek::Time(0.0), Emit::Suppress);
    let s = val(&e, 1, X);
    if s != 5.0 {
        errs.push(format!("seek(0): x = {s}, GSAP 5"));
    }
    e.anim(tl).set_total_time(0.01, Emit::Fire);
    let s = val(&e, 1, X);
    if s != 5.0 {
        errs.push(format!("totalTime(0.01): x = {s}, GSAP 5"));
    }
    assert_clean("timeline_set_at_0", errs);
}

/// [callbacks.immediate_render to_paused_default_start_capture,
/// to_paused_immediateRender_true] start capture at first render vs at
/// creation.
#[test]
fn golden_to_start_capture() {
    for (imm, want) in [(false, 75.0), (true, 50.0)] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let mut o = lin(1.0).paused(true);
        if imm {
            o = o.immediate_render(true);
        }
        let t = e.to(tid(1), &[PropTo::to_f64(X, 100.0)], o);
        assert_eq!(val(&e, 1, X), 0.0, "immediate {imm}: after create");
        seed(&mut e, 1, X, 50.0);
        e.anim(t).set_total_time(0.5, Emit::Fire);
        assert_eq!(val(&e, 1, X), want, "immediate {imm}: after 0.5");
    }
}

// ---------------------------------------------------------------------------
// Timeline repeat, yoyo, repeatDelay, exact boundaries
// ---------------------------------------------------------------------------

/// [positions timeline_repeat2_rdelay05_yoyo] timeline repeat 2, repeatDelay
/// .5, yoyo, one 1 s tween: iteration/time/progress/x at every sample, the
/// exact boundary belonging to the earlier iteration, the delay holding.
#[test]
fn golden_timeline_repeat2_rdelay05_yoyo() {
    let case = golden::positions::CASES
        .iter()
        .find(|c| c.id == "timeline_repeat2_rdelay05_yoyo")
        .unwrap();
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(2)
            .repeat_delay(0.5)
            .yoyo(true),
    );
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0), Position::END);
    let mut errs = Vec::new();
    let n = num(case.extra, "samples.len") as usize;
    for i in 0..n {
        let k = |f: &str| num(case.extra, &format!("samples.{i}.{f}"));
        let tt = k("totalTime_set");
        e.anim(tl).set_total_time(tt, Emit::Fire);
        let r = e.anim_ref(tl);
        for (f, g) in [
            ("iteration", r.iteration() as f64),
            ("time", r.time()),
            ("totalTime", r.total_time()),
            ("progress", r.progress()),
            ("totalProgress", r.total_progress()),
        ] {
            if !close(g, k(f), 1e-9) {
                errs.push(format!("totalTime({tt}): {f} = {g}, GSAP {}", k(f)));
            }
        }
        let x = val(&e, 1, X);
        if !close(x, k("x"), 5e-7) {
            errs.push(format!("totalTime({tt}): x = {x}, GSAP {}", k("x")));
        }
    }
    assert_clean("timeline_repeat2_rdelay05_yoyo", errs);
}

/// [samples repeat2_yoyo_rdelay03] a TWEEN with repeat 2, yoyo, repeatDelay
/// .3 (power1.in), sampled at 0.01 steps plus boundaries +-1e-6: x and
/// iteration (the exact end of a delay still belongs to the earlier one).
#[test]
fn golden_tween_repeat2_yoyo_rdelay03() {
    let case = golden::samples::CASES
        .iter()
        .find(|c| c.id == "repeat2_yoyo_rdelay03")
        .unwrap();
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let t = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        TweenOpts::new()
            .duration(1.0)
            .ease(Easing::InQuad)
            .repeat(2)
            .yoyo(true)
            .repeat_delay(0.3)
            .paused(true),
    );
    let xs = column(case.series, "x");
    let its = case.series.iter().find(|s| s.name == "anim.iteration");
    let mut errs = Vec::new();
    for (i, &tt) in case.t.iter().enumerate() {
        e.anim(t).set_total_time(tt, Emit::Fire);
        let x = val(&e, 1, X);
        if !close(x, xs[i], 5e-7) {
            errs.push(format!("totalTime({tt}): x = {x}, GSAP {}", xs[i]));
        }
        if let Some(its) = its {
            let it = e.anim_ref(t).iteration() as f64;
            if it != its.v[i] {
                errs.push(format!(
                    "totalTime({tt}): iteration = {it}, GSAP {}",
                    its.v[i]
                ));
            }
        }
    }
    assert_clean("repeat2_yoyo_rdelay03", errs);
}

/// [callbacks design_t9_timeline_repeat_order(_yoyo), design_t20_repeating_
/// timeline_wrap, design_forward_order] the boundary sweep of a repeating
/// timeline: children complete, then tl:onUpdate, tl:onRepeat, then the new
/// iteration's children.
#[test]
fn golden_timeline_repeat_sweeps() {
    let mut all = Vec::new();
    for (id, yoyo) in [
        ("design_t9_timeline_repeat_order", false),
        ("design_t9_timeline_repeat_order_yoyo", true),
    ] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let tl = e.timeline(
            TimelineOpts::new()
                .paused(true)
                .repeat(1)
                .yoyo(yoyo)
                .tag(tag("tl"))
                .events(EV),
        );
        e.tl(tl).to(
            tid(1),
            &[PropTo::to_f64(X, 100.0)],
            lin(1.0).tag(tag("a")).events(EV),
            Position::END,
        );
        for m in replay(&mut e, tl, trace(id), &[("a", 1, X)], true) {
            all.push(format!("{id}: {m}"));
        }
    }
    {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        seed(&mut e, 2, X, 0.0);
        let tl = e.timeline(
            TimelineOpts::new()
                .paused(true)
                .repeat(2)
                .tag(tag("tl"))
                .events(EV),
        );
        e.tl(tl)
            .to(
                tid(1),
                &[PropTo::to_f64(X, 100.0)],
                lin(1.0).tag(tag("a")).events(EV),
                0.0,
            )
            .call(tag("mark"), 0.5)
            .to(
                tid(2),
                &[PropTo::to_f64(X, 100.0)],
                lin(0.1).tag(tag("b")).events(EV),
                0.9,
            );
        let id = "design_t20_repeating_timeline_wrap";
        for m in replay(&mut e, tl, trace(id), &[("a", 1, X), ("b", 2, X)], true) {
            all.push(format!("{id}: {m}"));
        }
    }
    {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let tl = e.timeline(TimelineOpts::new().paused(true).tag(tag("tl")).events(EV));
        e.tl(tl).to(
            tid(1),
            &[PropTo::to_f64(X, 1.0)],
            lin(1.0).tag(tag("a")).events(EV),
            Position::END,
        );
        let id = "design_forward_order";
        for m in replay(&mut e, tl, trace(id), &[("a", 1, X)], true) {
            all.push(format!("{id}: {m}"));
        }
    }
    assert_clean("timeline repeat sweeps", all);
}

/// [callbacks design_nested_wraps_one_frame] tl {repeat 3} holding tl2
/// {repeat 3} with a 0.25 s tween: nested wraps inside one render.
#[test]
fn golden_nested_wraps_one_frame() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(3)
            .tag(tag("tl"))
            .events(EV),
    );
    let tl2 = e.timeline(TimelineOpts::new().repeat(3).tag(tag("tl2")).events(EV));
    e.tl(tl2).to(
        tid(1),
        &[PropTo::to_f64(X, 1.0)],
        lin(0.25).tag(tag("a")).events(EV),
        Position::END,
    );
    e.tl(tl).add(tl2, 0.0);
    let t = trace("design_nested_wraps_one_frame");
    // Design deviation L5 (5.10, "goto step 9"): when tl2's boundary sweep
    // lands exactly on its total duration (here: the outer sweep renders tl2
    // at 1.0 = its tdur from inside its 3rd iteration), GSAP returns early and
    // never reports tl2:onComplete; the engine runs the completion step, as
    // GSAP itself does when the same point is reached in small steps. The
    // expectation is the golden with that one Complete inserted after
    // tl2:onRepeat.
    let mut errs = Vec::new();
    for (k, op) in t.ops.iter().enumerate() {
        e.anim(tl).set_total_time(
            op.op
                .trim_start_matches("totalTime(")
                .trim_end_matches(')')
                .parse()
                .unwrap(),
            Emit::Fire,
        );
        let got = drain(&mut e, true);
        let mut want: Vec<String> = op.fired_all.iter().map(|s| s.to_string()).collect();
        if k > 0 {
            let at = want.iter().position(|s| s == "tl2:onRepeat").unwrap() + 1;
            want.insert(at, "tl2:onComplete".to_string());
        }
        if got != want {
            errs.push(format!("{}: fired {got:?}, want {want:?}", op.op));
        }
        check_values(&e, op, &[("x", 1, X)], &mut errs);
        check_state(&e, tl, op, &mut errs);
    }
    // The inner timeline's own state after the last op.
    let last = t.ops.last().unwrap();
    let r = e.anim_ref(tl2);
    let want_tt = num(last.values, "tl2_total_time");
    let want_it = num(last.values, "tl2_iteration");
    if !close(r.total_time(), want_tt, 1e-9) || r.iteration() as f64 != want_it {
        errs.push(format!(
            "tl2 after {}: total_time {} iteration {}, GSAP {want_tt} / {want_it}",
            last.op,
            r.total_time(),
            r.iteration()
        ));
    }
    assert_clean("design_nested_wraps_one_frame", errs);
}

/// [callbacks design_start_rules] onStart of a repeating tween fires on a
/// jump from 0 into a later iteration or onto a yoyo end; a backward jump
/// across a boundary fires Repeat but not Start; a repeating timeline's
/// second-iteration children start again.
#[test]
fn golden_start_rules() {
    let t = trace("design_start_rules");
    let mut errs = Vec::new();
    let mut check = |e: &mut TweenEngine, op: &Op, a: TweenId, x: u32| {
        let got = drain(e, true);
        let want: Vec<String> = op.fired_all.iter().map(|s| s.to_string()).collect();
        if got != want {
            errs.push(format!("{}: fired {got:?}, GSAP {want:?}", op.op));
        }
        check_values(e, op, &[("x", x, X)], &mut errs);
        check_state(e, a, op, &mut errs);
    };
    let tw = |r: i32, yoyo: bool| {
        lin(1.0)
            .repeat(r)
            .yoyo(yoyo)
            .paused(true)
            .tag(tag("t"))
            .events(EV)
    };
    // Tweens live in a paused timeline so that Repeat (which needs a parent
    // in GSAP) is reported like in the golden (gsap.to: parent = root).
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(tid(1), &[PropTo::to_f64(X, 100.0)], tw(2, false));
    drain(&mut e, true);
    e.anim(a).set_total_time(1.5, Emit::Fire);
    check(&mut e, &t.ops[0], a, 1);

    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(tid(1), &[PropTo::to_f64(X, 100.0)], tw(1, true));
    drain(&mut e, true);
    e.anim(a).set_total_time(2.0, Emit::Fire);
    check(&mut e, &t.ops[1], a, 1);

    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(tid(1), &[PropTo::to_f64(X, 100.0)], tw(2, false));
    e.anim(a).set_total_time(1.5, Emit::Suppress);
    drain(&mut e, true);
    e.anim(a).set_total_time(0.5, Emit::Fire);
    check(&mut e, &t.ops[2], a, 1);

    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(2)
            .tag(tag("tl"))
            .events(EV),
    );
    e.tl(tl).to(
        tid(1),
        &[PropTo::to_f64(X, 1.0)],
        lin(1.0).tag(tag("a")).events(EV),
        Position::END,
    );
    drain(&mut e, true);
    e.anim(tl).set_total_time(1.5, Emit::Fire);
    check(&mut e, &t.ops[3], tl, 1);
    e.anim(tl).set_total_time(0.5, Emit::Fire);
    check(&mut e, &t.ops[4], tl, 1);
    assert_clean("design_start_rules", errs);
}

// ---------------------------------------------------------------------------
// Callbacks on big jumps with dt stepping
// ---------------------------------------------------------------------------

/// [callbacks design_dt_stepping_repeat_counts] a root-level tween with
/// repeat 3, repeatDelay .25 driven by advance(dt) at several dt: exactly
/// one Start, three Repeats and one Complete, value exact at the end.
#[test]
fn golden_dt_stepping_repeat_counts() {
    let mut errs = Vec::new();
    for (dt, yoyo, want_x) in [
        (1.0 / 60.0, false, 100.0),
        (1.0 / 7.0, false, 100.0),
        (0.37, false, 100.0),
        (1.0 / 60.0, true, 0.0),
    ] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        e.to(
            tid(1),
            &[PropTo::to_f64(X, 100.0)],
            lin(1.0)
                .repeat(3)
                .repeat_delay(0.25)
                .yoyo(yoyo)
                .tag(tag("tw"))
                .events(EventMask::EDGES),
        );
        let mut fired = Vec::new();
        for _ in 0..1000 {
            e.advance(dt);
            fired.extend(drain(&mut e, false));
            if !e.is_active() {
                break;
            }
        }
        let want = [
            "tw:onStart",
            "tw:onRepeat",
            "tw:onRepeat",
            "tw:onRepeat",
            "tw:onComplete",
        ];
        if fired != want {
            errs.push(format!(
                "dt {dt} yoyo {yoyo}: fired {fired:?}, GSAP {want:?}"
            ));
        }
        let x = val(&e, 1, X);
        if x != want_x {
            errs.push(format!("dt {dt} yoyo {yoyo}: x = {x}, GSAP {want_x}"));
        }
    }
    assert_clean("design_dt_stepping_repeat_counts", errs);
}

/// [callbacks design_big_dt_crosses_child] one advance from 0 to 2.0 over a
/// root child 0.5..1.0: Start, Update, Complete, value exact.
#[test]
fn golden_big_dt_crosses_child() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        lin(0.5).delay(0.5).tag(tag("c")).events(EV),
    );
    e.advance(2.0);
    let got = drain(&mut e, true);
    assert_eq!(got, ["c:onStart", "c:onUpdate", "c:onComplete"]);
    assert_eq!(val(&e, 1, X), 100.0);
}

// ---------------------------------------------------------------------------
// add_pause
// ---------------------------------------------------------------------------

/// [callbacks design_t11_add_pause] an un-paused root-level timeline with a
/// 2 s tween and add_pause(1): the frame walk stops exactly at 1, pauses,
/// reports the pause before tl:onUpdate.
#[test]
fn golden_add_pause_t11() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new().tag(tag("tl")).events(
            EventMask::START
                .with(EventMask::UPDATE)
                .with(EventMask::COMPLETE),
        ),
    );
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END)
        .add_pause(1.0, tag("pause"));
    let t = trace("design_t11_add_pause");
    let mut errs = Vec::new();
    for (op, dt) in t.ops.iter().zip([0.9, 0.2]) {
        e.advance(dt);
        let got = drain(&mut e, true);
        if got != op.fired_all {
            errs.push(format!("{}: fired {got:?}, GSAP {:?}", op.op, op.fired_all));
        }
        check_values(&e, op, &[("a", 1, X)], &mut errs);
        check_state(&e, tl, op, &mut errs);
    }
    // Stays paused on further frames.
    e.advance(0.5);
    if e.anim_ref(tl).time() != 1.0 || !drain(&mut e, true).is_empty() {
        errs.push(format!(
            "after the pause, a frame moved the timeline to {}",
            e.anim_ref(tl).time()
        ));
    }
    assert_clean("design_t11_add_pause", errs);
}

/// [positions addPause_2] totalTime()/seek() jump over a pause
/// (time 2.5, not paused); the frame walk (render) stops on it (time 2,
/// paused).
#[test]
fn golden_add_pause_seek_vs_frame() {
    let build = |e: &mut TweenEngine| {
        seed(e, 1, X, 0.0);
        let tl = e.timeline(TimelineOpts::new());
        for _ in 0..3 {
            e.tl(tl).to(
                tid(1),
                &[PropTo::to_f64(X, 1.0)],
                TweenOpts::new().duration(1.0),
                Position::END,
            );
        }
        e.tl(tl).add_pause(2.0, tag("pause"));
        tl
    };
    let mut e = TweenEngine::new();
    let tl = build(&mut e);
    e.anim(tl).set_total_time(1.5, Emit::Fire);
    e.anim(tl).set_total_time(2.5, Emit::Fire);
    let r = e.anim_ref(tl);
    assert_eq!((r.time(), r.paused()), (2.5, false), "totalTime jumps over");

    let mut e = TweenEngine::new();
    let tl = build(&mut e);
    e.advance(1.5);
    e.advance(1.0);
    let r = e.anim_ref(tl);
    assert_eq!((r.time(), r.paused()), (2.0, true), "the frame stops on it");
}

/// A pause crossed BACKWARD by a reversed timeline stops it too (GSAP
/// _findNextPauseTween scans in the direction of travel).
#[test]
fn add_pause_stops_a_reversed_frame_walk() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END)
        .add_pause(1.0, tag("pause"));
    e.anim(tl).set_total_time(1.8, Emit::Suppress);
    e.anim(tl).reverse();
    drain(&mut e, true);
    e.advance(1.0); // 1.8 -> 0.8 would cross the pause at 1
    let r = e.anim_ref(tl);
    assert_eq!(r.time(), 1.0, "stopped at the pause");
    assert!(r.paused());
    assert!(drain(&mut e, true).contains(&"pause:cb".to_string()));
    assert!(close(val(&e, 1, X), 50.0, 1e-9));
}

// ---------------------------------------------------------------------------
// Controls: restart / reverse / play_from / set_progress
// ---------------------------------------------------------------------------

/// [callbacks design_restart_reverse_controls] controls on children of a
/// paused smooth-child-timing timeline R stepped with R.totalTime(+dt):
/// restart() suppresses and re-aligns; restart(true) re-aligns before the
/// delay; reverse() un-pauses, is not a toggle; resume keeps direction;
/// play un-reverses.
#[test]
fn golden_restart_reverse_controls() {
    let t = trace("design_restart_reverse_controls");
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let mk_r = |e: &mut TweenEngine| {
        e.timeline(TimelineOpts::new().paused(true).smooth_child_timing(true))
    };
    let r = mk_r(&mut e);
    let mut cur = e
        .tl(r)
        .to(
            tid(1),
            &[PropTo::to_f64(X, 100.0)],
            lin(1.0).delay(0.5).tag(tag("t")).events(EV),
            Position::END,
        )
        .last();
    let mut rr = r;
    let mut errs = Vec::new();
    let mut second_block = false;
    for op in t.ops {
        let o = op.op;
        if let Some(dt) = o.strip_prefix("R +") {
            let dt: f64 = dt.parse().unwrap();
            if o == "R +0.6" && !second_block {
                // Second block: a fresh R with u.
                second_block = true;
                e = TweenEngine::new();
                seed(&mut e, 1, X, 0.0);
                rr = mk_r(&mut e);
                cur = e
                    .tl(rr)
                    .to(
                        tid(1),
                        &[PropTo::to_f64(X, 100.0)],
                        lin(1.0).tag(tag("u")).events(EV),
                        Position::END,
                    )
                    .last();
                drain(&mut e, true);
            }
            let now = e.anim_ref(rr).total_time();
            e.anim(rr).set_total_time(now + dt, Emit::Fire);
        } else {
            let mut a = e.anim(cur);
            match o.split_once('.').map(|x| x.1).unwrap_or(o) {
                "restart()" => {
                    a.restart(false, Emit::Suppress);
                }
                "restart(true)" => {
                    a.restart(true, Emit::Suppress);
                }
                "reverse()" => {
                    a.reverse();
                }
                "pause()" => {
                    a.pause();
                }
                "resume()" => {
                    a.resume();
                }
                "play()" => {
                    a.play();
                }
                other => panic!("unhandled op {other}"),
            }
        }
        let got = drain(&mut e, true);
        if got != op.fired_all {
            errs.push(format!("{o}: fired {got:?}, GSAP {:?}", op.fired_all));
        }
        check_values(&e, op, &[("x", 1, X)], &mut errs);
        check_state(&e, cur, op, &mut errs);
        for (k, v) in op.values {
            let Val::F(w) = v else { continue };
            let a = e.anim_ref(cur);
            let g = match *k {
                "t_start" => a.start_time(),
                "t_total_time" | "u_total_time" => a.total_time(),
                _ => continue,
            };
            if !close(g, *w, 1e-9) {
                errs.push(format!("{o}: {k} = {g}, GSAP {w}"));
            }
        }
    }
    assert_clean("design_restart_reverse_controls", errs);
}

/// [callbacks design_progress_time_iteration_setters] progress(p) on a yoyo
/// tween in its mirrored (2nd) iteration lands on the value p gives, and
/// iteration(k) keeps the local time. (time(v) ops are the documented
/// deviation and are not replayed.)
#[test]
fn golden_progress_in_mirrored_iteration() {
    let t = trace("design_progress_time_iteration_setters");
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        lin(1.0)
            .repeat(1)
            .yoyo(true)
            .paused(true)
            .tag(tag("t"))
            .events(EV),
    );
    e.anim(a).set_total_time(1.5, Emit::Suppress);
    drain(&mut e, true);
    e.anim(a).set_progress(0.25, Emit::Fire);
    let op = &t.ops[0];
    let mut errs = Vec::new();
    let got = drain(&mut e, true);
    if got != op.fired_all {
        errs.push(format!("{}: fired {got:?}, GSAP {:?}", op.op, op.fired_all));
    }
    check_values(&e, op, &[("x", 1, X)], &mut errs);
    check_state(&e, a, op, &mut errs);

    // iteration(3) of a repeat-2 tween at 0.25 -> 2.25 (x 25, Update +
    // Repeat), then set_progress(1) in iteration 3 -> total 3.
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let b = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        lin(1.0).repeat(2).paused(true).tag(tag("t")).events(EV),
    );
    e.anim(b).set_total_time(0.25, Emit::Suppress);
    drain(&mut e, true);
    e.anim(b).set_iteration(3, Emit::Fire);
    let op = &t.ops[2];
    let got = drain(&mut e, true);
    if got != op.fired_all {
        errs.push(format!("{}: fired {got:?}, GSAP {:?}", op.op, op.fired_all));
    }
    check_values(&e, op, &[("x", 1, X)], &mut errs);
    check_state(&e, b, op, &mut errs);
    assert_clean("design_progress_time_iteration_setters", errs);
}

/// set_progress on a yoyo TIMELINE in a mirrored iteration: GSAP
/// progress(p) = totalTime(dur * (1 - p) + elapsed) in even iterations, so
/// progress() reads back p and the child shows p's value.
#[test]
fn set_progress_on_mirrored_timeline_iteration() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true).repeat(1).yoyo(true));
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END);
    e.anim(tl).set_total_time(3.0, Emit::Suppress); // iteration 2, time 1
    e.anim(tl).set_progress(0.25, Emit::Fire);
    let r = e.anim_ref(tl);
    assert_eq!(r.iteration(), 2);
    assert!(close(r.total_time(), 3.5, 1e-9), "total {}", r.total_time());
    assert!(close(r.progress(), 0.25, 1e-9), "progress {}", r.progress());
    assert!(close(val(&e, 1, X), 25.0, 1e-9), "x {}", val(&e, 1, X));
}

/// play_from / reverse_from on a paused tween: GSAP play(from) = seek(from)
/// then reversed(false).paused(false); reverse(0) seeks the END first.
#[test]
fn play_from_and_reverse_from() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        lin(1.0).paused(true).tag(tag("t")).events(EV),
    );
    e.anim(a).reverse_from(None, Emit::Suppress);
    assert!(close(val(&e, 1, X), 100.0, 1e-12));
    assert!(e.anim_ref(a).reversed() && !e.anim_ref(a).paused());
    drain(&mut e, true);
    e.advance(0.25);
    assert!(close(val(&e, 1, X), 75.0, 1e-9), "x {}", val(&e, 1, X));
    e.anim(a).play_from(Seek::Time(0.5), Emit::Suppress);
    assert!(!e.anim_ref(a).reversed());
    assert!(close(val(&e, 1, X), 50.0, 1e-9));
    drain(&mut e, true);
    e.advance(0.25);
    assert!(close(val(&e, 1, X), 75.0, 1e-9), "x {}", val(&e, 1, X));
    e.advance(1.0);
    assert_eq!(val(&e, 1, X), 100.0);
    let got = drain(&mut e, false);
    assert_eq!(got, ["t:onComplete"]);
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

/// Builds the positions case lines of the form
/// `tl.to(o,{duration:D[,repeat:R][,repeatDelay:RD][,delay:DL]}[,POS]) // NAME`,
/// `tl.addLabel("L"[, POS])`, `tl.call(fn, null, POS) // call`,
/// `tl.set(o,{x:1},POS) // S`, `tl.addPause(POS)`, `tl.remove(NAME)`,
/// `tl.shiftChildren(a[, adjust[, ignore]])`. Returns None for other lines.
fn position_case(id: &str) -> Option<Vec<String>> {
    let case = golden::positions::CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap();
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    let labels = std::cell::RefCell::new(Vec::<String>::new());
    let label_tag = |s: &str| -> Tag {
        let mut labels = labels.borrow_mut();
        if let Some(i) = labels.iter().position(|l| l == s) {
            Tag(1000 + i as u64)
        } else {
            labels.push(s.to_string());
            Tag(1000 + labels.len() as u64 - 1)
        }
    };
    let mut named: Vec<(String, TweenId)> = Vec::new();
    let num_of = |v: &str, key: &str| -> Option<f64> {
        let i = v.find(&format!("{key}:"))? + key.len() + 1;
        let rest = &v[i..];
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        let s = rest[..end].trim();
        if let Some((a, b)) = s.split_once('/') {
            Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?)
        } else {
            s.parse().ok()
        }
    };
    for line in case.gsap_calls {
        let (code, name) = match line.split_once("//") {
            Some((c, n)) => (c.trim(), n.trim().split(' ').next().unwrap_or("")),
            None => (line.trim(), ""),
        };
        if code.starts_with("tl = gsap.timeline({paused:true})") || code == "tl.duration()" {
            continue;
        }
        let pos_of = |s: &str| -> Option<Position> {
            let s = s.trim();
            if s.is_empty() {
                return Some(Position::END);
            }
            if let Some(q) = s.strip_prefix('"').and_then(|q| q.strip_suffix('"')) {
                return Position::parse(q, label_tag).ok();
            }
            if let Some((a, b)) = s.split_once('/') {
                return Some(Position::at(
                    a.parse::<f64>().ok()? / b.parse::<f64>().ok()?,
                ));
            }
            s.parse::<f64>().ok().map(Position::at)
        };
        if let Some(rest) = code.strip_prefix("tl.to(") {
            let rest = rest.strip_suffix(')')?;
            let close_brace = rest.rfind('}')?;
            let vars = &rest[..=close_brace];
            let pos = rest[close_brace + 1..].trim_start_matches(',');
            let mut o = TweenOpts::new().duration(num_of(vars, "duration")?);
            if let Some(r) = num_of(vars, "repeat") {
                o = o.repeat(r as i32);
            }
            if let Some(r) = num_of(vars, "repeatDelay") {
                o = o.repeat_delay(r);
            }
            if let Some(d) = num_of(vars, "delay") {
                o = o.delay(d);
            }
            let p = pos_of(pos)?;
            let id = e.tl(tl).to(tid(1), &[PropTo::to_f64(X, 1.0)], o, p).last();
            named.push((name.to_string(), id));
        } else if let Some(rest) = code.strip_prefix("tl.set(o,{x:1},") {
            let p = pos_of(rest.strip_suffix(')')?)?;
            let id = e
                .tl(tl)
                .set(tid(1), &[PropTo::to_f64(X, 1.0)], TweenOpts::new(), p)
                .last();
            named.push((name.to_string(), id));
        } else if let Some(rest) = code.strip_prefix("tl.call(fn, null, ") {
            let p = pos_of(rest.strip_suffix(')')?)?;
            let id = e.tl(tl).call(tag("mark"), p).last();
            named.push((name.to_string(), id));
        } else if let Some(rest) = code.strip_prefix("tl.addLabel(") {
            let rest = rest.strip_suffix(')')?;
            let (l, p) = match rest.split_once(',') {
                Some((l, p)) => (l, p),
                None => (rest, ""),
            };
            let l = l.trim().trim_matches('"');
            let lt = label_tag(l);
            let p = pos_of(p)?;
            e.tl(tl).add_label(lt, p);
        } else if let Some(rest) = code.strip_prefix("tl.remove(") {
            let n = rest.strip_suffix(')')?;
            let id = named.iter().find(|(k, _)| k == n)?.1;
            e.tl(tl).remove(id);
            named.retain(|(k, _)| k != n);
        } else if let Some(rest) = code.strip_prefix("tl.shiftChildren(") {
            let a: Vec<&str> = rest.strip_suffix(')')?.split(',').map(str::trim).collect();
            let amount: f64 = a[0].parse().ok()?;
            let adjust = a.get(1) == Some(&"true");
            let ignore: f64 = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(0.0);
            e.tl(tl).shift_children(amount, adjust, ignore);
        } else {
            return None;
        }
    }
    // Force the duration evaluation GSAP's harness performs (tl.duration()):
    // a suppressed seek to 0 recomputes a dirty timeline.
    e.anim(tl).seek(Seek::Time(0.0), Emit::Suppress);
    let mut errs = Vec::new();
    for c in case.children {
        let Some((_, id)) = named.iter().find(|(k, _)| k == c.name) else {
            continue;
        };
        let r = e.anim_ref(*id);
        for (k, g, w) in [
            ("startTime", r.start_time(), c.start_time),
            ("endTime", r.end_time(true), c.end_time),
            ("totalDuration", r.total_duration(), c.total_duration),
        ] {
            if !close(g, w, 1e-9) {
                errs.push(format!("{}: {k} = {g}, GSAP {w}", c.name));
            }
        }
    }
    for (l, w) in case.labels {
        let lt = label_tag(l);
        match e.anim_ref(tl).label_time(lt) {
            Some(g) if close(g, *w, 1e-9) => {}
            g => errs.push(format!("label {l}: {g:?}, GSAP {w}")),
        }
    }
    let r = e.anim_ref(tl);
    if !close(r.duration(), case.duration, 1e-9) {
        errs.push(format!("duration {}, GSAP {}", r.duration(), case.duration));
    }
    if !close(r.total_duration(), case.total_duration, 1e-9) {
        errs.push(format!(
            "totalDuration {}, GSAP {}",
            r.total_duration(),
            case.total_duration
        ));
    }
    if r.child_count() != case.child_count {
        errs.push(format!(
            "child_count {}, GSAP {}",
            r.child_count(),
            case.child_count
        ));
    }
    Some(errs)
}

/// Every positions case the line interpreter can build, except the
/// documented deviations (numeric-string labels, addLabel percent quirk).
#[test]
fn golden_positions_replayed() {
    let skip = [
        "label_numeric_string",           // P13 deviation: a numeric string is a time
        "design_add_label_percent_quirk", // P12 deviation
    ];
    let mut all = Vec::new();
    let mut ran = 0;
    for c in golden::positions::CASES {
        if skip.contains(&c.id) {
            continue;
        }
        if let Some(errs) = position_case(c.id) {
            ran += 1;
            for m in errs {
                all.push(format!("{}: {m}", c.id));
            }
        }
    }
    assert!(ran >= 30, "only {ran} position cases ran");
    assert_clean("positions", all);
}

/// [positions negative_start_then_more] a child at -0.5 shifts every child
/// by +0.5 when the next position is parsed, but labels stay: label L=0.25
/// stays 0.25, so C at "L" lands at 0.25 in the shifted frame.
#[test]
fn golden_negative_start_labels_not_shifted() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    let l = Tag(77);
    let d1 = TweenOpts::new().duration(1.0);
    let a = e
        .tl(tl)
        .add_label(l, 0.25)
        .to(tid(1), &[PropTo::to_f64(X, 1.0)], d1, -0.5)
        .last();
    let b = e
        .tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 1.0)], d1, Position::END)
        .last();
    let c = e.tl(tl).to(tid(1), &[PropTo::to_f64(X, 1.0)], d1, l).last();
    let case = golden::positions::CASES
        .iter()
        .find(|c| c.id == "negative_start_then_more")
        .unwrap();
    let want = |n: &str| {
        case.children
            .iter()
            .find(|c| c.name == n)
            .unwrap()
            .start_time
    };
    let got = [
        e.anim_ref(a).start_time(),
        e.anim_ref(b).start_time(),
        e.anim_ref(c).start_time(),
    ];
    assert_eq!(got, [want("A"), want("B"), want("C")], "A, B, C starts");
    assert_eq!(e.anim_ref(tl).label_time(l), Some(0.25));
}

/// [positions gt_after_repeat_infinite] ">" after repeat -1 uses ONE
/// iteration (1); the timeline becomes 1e10 long; default and "+=" then clip
/// to the recent child's end (C 2, D 3.5).
#[test]
fn golden_gt_after_infinite_repeat() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    let d1 = TweenOpts::new().duration(1.0);
    let p = [PropTo::to_f64(X, 1.0)];
    let a = e.tl(tl).to(tid(1), &p, d1.repeat(-1), Position::END).last();
    let b = e.tl(tl).to(tid(1), &p, d1, Position::prev_end(0.0)).last();
    let c = e.tl(tl).to(tid(1), &p, d1, Position::END).last();
    let d = e.tl(tl).to(tid(1), &p, d1, Position::rel(0.5)).last();
    let s = |x| e.anim_ref(x).start_time();
    assert_eq!([s(a), s(b), s(c), s(d)], [0.0, 1.0, 2.0, 3.5]);
    assert_eq!(e.anim_ref(tl).duration(), INFINITE);
}

// ---------------------------------------------------------------------------
// Nested timeScale and reversed children
// ---------------------------------------------------------------------------

/// Samples one timescale.json case: `build` makes the outer timeline and
/// returns (outer, inner); columns outer.time / inner.time / inner.totalTime
/// / inner.progress / x are compared at outer.totalTime(t).
fn timescale_case(
    id: &str,
    build: impl FnOnce(&mut TweenEngine) -> (TweenId, TweenId),
) -> Vec<String> {
    let case = golden::timescale::CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap();
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let (outer, inner) = build(&mut e);
    let mut errs = Vec::new();
    let col = |n: &str| case.series.iter().find(|s| s.name == n).map(|s| s.v);
    for (i, &t) in case.t.iter().enumerate() {
        e.anim(outer).set_total_time(t, Emit::Fire);
        let ri = e.anim_ref(inner);
        let checks = [
            ("outer.time", e.anim_ref(outer).time(), 1e-9),
            ("inner.time", ri.time(), 1e-9),
            ("inner.totalTime", ri.total_time(), 1e-9),
            ("inner.progress", ri.progress(), 1e-9),
            ("x", val(&e, 1, X), 5e-7),
        ];
        for (k, g, eps) in checks {
            if let Some(w) = col(k) {
                if !close(g, w[i], eps) {
                    errs.push(format!("{id} t={t}: {k} = {g}, GSAP {}", w[i]));
                }
            }
        }
    }
    errs
}

/// [timescale inner_reversed_timescale_0_5] a reversed child with time scale
/// 0.5 (occupies 1..5, plays from its end at its start).
#[test]
fn golden_inner_reversed_timescale_0_5() {
    let errs = timescale_case("inner_reversed_timescale_0_5", |e| {
        let outer = e.timeline(TimelineOpts::new().paused(true));
        let inner = e.timeline(TimelineOpts::new().time_scale(0.5).reversed(true));
        e.tl(inner)
            .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END);
        e.tl(outer).add(inner, 1.0);
        (outer, inner)
    });
    assert_clean("inner_reversed_timescale_0_5", errs);
}

/// [timescale inner_repeat1_yoyo_timescale_2]
#[test]
fn golden_inner_repeat1_yoyo_timescale_2() {
    let errs = timescale_case("inner_repeat1_yoyo_timescale_2", |e| {
        let outer = e.timeline(TimelineOpts::new().paused(true));
        let inner = e.timeline(TimelineOpts::new().repeat(1).yoyo(true).time_scale(2.0));
        e.tl(inner)
            .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END);
        e.tl(outer).add(inner, 1.0);
        (outer, inner)
    });
    assert_clean("inner_repeat1_yoyo_timescale_2", errs);
}

/// [timescale three_levels_outer_ts2_inner_ts0_5] mid ts 2 > inner ts 0.5.
#[test]
fn golden_three_levels_time_scales() {
    let case = golden::timescale::CASES
        .iter()
        .find(|c| c.id == "three_levels_outer_ts2_inner_ts0_5")
        .unwrap();
    let errs = timescale_case("three_levels_outer_ts2_inner_ts0_5", |e| {
        let outer = e.timeline(TimelineOpts::new().paused(true));
        let mid = e.timeline(TimelineOpts::new().time_scale(2.0));
        let inner = e.timeline(TimelineOpts::new().time_scale(0.5));
        e.tl(inner)
            .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END);
        e.tl(mid).add(inner, 0.0);
        e.tl(outer).add(mid, 0.0);
        (outer, inner)
    });
    let _ = case;
    assert_clean("three_levels_outer_ts2_inner_ts0_5", errs);
}

/// [timescale negative_timescale] set_time_scale(-1) reverses; -2 then
/// reversed(false) -> 2; time scale 0 then reversed(true): reports 0 but
/// reversed; then reversed(false) reports 1e-8.
#[test]
fn golden_negative_timescale() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(tid(1), &[PropTo::to_f64(X, 1.0)], lin(1.0).paused(true));
    e.anim(a).set_time_scale(-1.0);
    assert_eq!(
        (e.anim_ref(a).time_scale(), e.anim_ref(a).reversed()),
        (-1.0, true)
    );
    e.anim(a).set_time_scale(-2.0).set_reversed(false);
    assert_eq!(
        (e.anim_ref(a).time_scale(), e.anim_ref(a).reversed()),
        (2.0, false)
    );
    e.anim(a).set_time_scale(0.0).set_reversed(true);
    assert_eq!(
        (e.anim_ref(a).time_scale(), e.anim_ref(a).reversed()),
        (0.0, true)
    );
    e.anim(a).set_reversed(false);
    assert_eq!(e.anim_ref(a).time_scale(), 1e-8);
}

// ---------------------------------------------------------------------------
// Overwrite auto window
// ---------------------------------------------------------------------------

/// Replays one overwrite.json case (t1 on the root, t2 paused with MODE).
fn overwrite_case(id: &str, mode: Overwrite, t1_opts: TweenOpts, drive_t1: bool) -> Vec<String> {
    let case = golden::overwrite::CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap();
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    seed(&mut e, 1, Y, 0.0);
    let t1 = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0), PropTo::to_f64(Y, 100.0)],
        lin(1.0)
            .tag(tag("t1"))
            .events(EventMask::INTERRUPT)
            .or(&t1_opts),
    );
    let mut errs = Vec::new();
    let mut step = 0;
    let mut check = |e: &mut TweenEngine, errs: &mut Vec<String>| {
        let s = &case.steps[step];
        step += 1;
        let v = s.vals;
        for (k, p) in [("obj.x", X), ("obj.y", Y)] {
            if let Some(Val::F(w)) = golden::common::val(v, k) {
                let g = val(e, 1, p);
                if !close(g, w, 5e-7) {
                    errs.push(format!("{} {k}: {g}, GSAP {w}", s.at));
                }
            }
        }
        let fired = drain(e, false);
        let want = golden::common::list(v, "callbacks");
        if fired != want {
            errs.push(format!("{}: fired {fired:?}, GSAP {want:?}", s.at));
        }
    };
    if drive_t1 {
        e.anim(t1).set_total_time(0.5, Emit::Fire);
    }
    check(&mut e, &mut errs);
    let t2 = e.to(
        tid(1),
        &[PropTo::to_f64(X, 200.0)],
        lin(1.0).overwrite(mode).paused(true),
    );
    check(&mut e, &mut errs);
    e.anim(t2).set_total_time(0.0, Emit::Fire);
    check(&mut e, &mut errs);
    e.anim(t1).set_total_time(1.0, Emit::Fire);
    check(&mut e, &mut errs);
    e.anim(t2).set_total_time(1.0, Emit::Fire);
    check(&mut e, &mut errs);
    errs
}

/// [overwrite auto_t1_playing, auto_t1_paused, auto_t1_not_started,
/// true_t1_playing, false_t1_playing] Auto kills only the overlapping
/// property of an initted, un-paused rival whose window contains t2's
/// start, at t2's first render; All kills at creation and reports Interrupt.
#[test]
fn golden_overwrite_modes() {
    let mut all = Vec::new();
    for (id, mode, o, drive) in [
        ("auto_t1_playing", Overwrite::Auto, TweenOpts::new(), true),
        (
            "auto_t1_paused",
            Overwrite::Auto,
            TweenOpts::new().paused(true),
            true,
        ),
        (
            "auto_t1_not_started",
            Overwrite::Auto,
            TweenOpts::new().delay(1.0),
            false,
        ),
        ("true_t1_playing", Overwrite::All, TweenOpts::new(), true),
        ("false_t1_playing", Overwrite::None, TweenOpts::new(), true),
    ] {
        for m in overwrite_case(id, mode, o, drive) {
            all.push(format!("{id}: {m}"));
        }
    }
    assert_clean("overwrite", all);
}

// ---------------------------------------------------------------------------
// Backward scrubs through nested children and reversed children
// ---------------------------------------------------------------------------

/// A child timeline with a from() inside, scrubbed forward past it and back
/// to before the parent's start: the from value must be written again (GSAP
/// renders the child at a negative total, the from() rewinds to its start).
#[test]
fn backward_scrub_rewrites_from_values_in_nested_child() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let outer = e.timeline(TimelineOpts::new().paused(true));
    let inner = e.timeline(TimelineOpts::new());
    e.tl(inner).from(
        tid(1),
        &[PropTo::from(X, TweenValue::F64(100.0))],
        lin(1.0),
        Position::END,
    );
    e.tl(outer).add(inner, 1.0);
    assert_eq!(val(&e, 1, X), 100.0, "immediate render");
    e.anim(outer).set_total_time(1.5, Emit::Fire);
    assert!(close(val(&e, 1, X), 50.0, 1e-9));
    e.anim(outer).set_total_time(0.2, Emit::Fire);
    assert_eq!(val(&e, 1, X), 100.0, "back before the start");
}

/// A reversed tween child (ts -1) in a paused timeline: at the parent's
/// child start it shows its END (x 100), halfway 50, at its end 0; before
/// its start the target is not pre-set [timescale tween_child_reversed].
#[test]
fn golden_tween_child_reversed() {
    let errs = timescale_case("tween_child_reversed", |e| {
        let outer = e.timeline(TimelineOpts::new().paused(true));
        let a = e
            .tl(outer)
            .to(
                tid(1),
                &[PropTo::to_f64(X, 100.0)],
                lin(1.0).reversed(true),
                0.0,
            )
            .last();
        (outer, a)
    });
    assert_clean("tween_child_reversed", errs);
}

// ---------------------------------------------------------------------------
// samples.json replays
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Drive {
    /// `anim.totalTime(t)` on the driven animation.
    TotalTime,
    /// `tl.seek(t)` (suppressed).
    Seek,
}

/// Replays one samples.json case. `build` seeds and builds, returning the
/// driven animation. Columns: x / y / a (target 1), o<i>.x / o<i>.y (target
/// i + 1), anim.* and tl.* getters. `warm` runs before sampling.
fn sample_case(
    id: &str,
    drive: Drive,
    build: impl FnOnce(&mut TweenEngine) -> TweenId,
) -> Vec<String> {
    let case = golden::samples::CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no sample case {id}"));
    let mut e = TweenEngine::new();
    for t in 1..=5 {
        seed(&mut e, t, X, 0.0);
        seed(&mut e, t, Y, 0.0);
    }
    let a = build(&mut e);
    // keyframes_percent_outer_ease: the inner time 2 * InCubic(0.075) =
    // 0.00084375 sits on a round7 tie; GSAP's Math.pow lands one ulp below it
    // (0.0008437), the engine's p*p*p on it (0.0008438): 8e-6 in x, float
    // noise, not semantics.
    let vtol = if id == "keyframes_percent_outer_ease" {
        1e-5
    } else {
        5e-7
    };
    let mut errs = Vec::new();
    for (i, &t) in case.t.iter().enumerate() {
        match drive {
            Drive::TotalTime => e.anim(a).set_total_time(t, Emit::Fire),
            Drive::Seek => e.anim(a).seek(Seek::Time(t), Emit::Suppress),
        };
        let r = e.anim_ref(a);
        for s in case.series {
            let w = s.v[i];
            let (g, eps) = match s.name {
                "x" | "a" => (val(&e, 1, X), vtol),
                "y" => (val(&e, 1, Y), vtol),
                "anim.time" | "tl.time" => (r.time(), 1e-9),
                "anim.iteration" => (r.iteration() as f64, 0.0),
                "anim.progress" => (r.progress(), 1e-9),
                "anim.totalTime" => (r.total_time(), 1e-9),
                "anim.totalProgress" => (r.total_progress(), 1e-9),
                n if n.starts_with('o') && n.len() == 4 => {
                    let k: u32 = n[1..2].parse().unwrap();
                    let p = if n.ends_with('x') { X } else { Y };
                    (val(&e, k + 1, p), 5e-7)
                }
                _ => continue,
            };
            if !close(g, w, eps) {
                errs.push(format!("{id} t={t}: {} = {g}, GSAP {w}", s.name));
            }
        }
    }
    errs
}

fn paused_lin(d: f64) -> TweenOpts {
    lin(d).paused(true)
}

/// [samples design_t4_yoyo_repeat_delay_table, design_t4b_exact_cycle_boundary,
/// repeat_infinite, delay05_standalone] repeat / yoyo / repeatDelay holds,
/// exact boundaries, infinite repeat, delay excluded from totalTime.
#[test]
fn golden_samples_repeat_and_delay() {
    let mut all = Vec::new();
    let px = |v: f64| [PropTo::to_f64(X, v), PropTo::to_f64(Y, v / 2.0)];
    all.extend(sample_case(
        "design_t4_yoyo_repeat_delay_table",
        Drive::TotalTime,
        |e| {
            e.to(
                tid(1),
                &[PropTo::to_f64(X, 100.0)],
                paused_lin(1.0).repeat(2).yoyo(true).repeat_delay(0.5),
            )
        },
    ));
    all.extend(sample_case(
        "design_t4b_exact_cycle_boundary",
        Drive::TotalTime,
        |e| {
            e.to(
                tid(1),
                &[PropTo::to_f64(X, 100.0)],
                paused_lin(1.0).repeat(2),
            )
        },
    ));
    all.extend(sample_case("repeat_infinite", Drive::TotalTime, |e| {
        e.to(
            tid(1),
            &px(100.0),
            paused_lin(1.0).repeat(-1).ease(Easing::InOutQuad),
        )
    }));
    all.extend(sample_case("delay05_standalone", Drive::TotalTime, |e| {
        e.to(tid(1), &px(100.0), paused_lin(1.0).delay(0.5))
    }));
    all.extend(sample_case("delay05_in_timeline", Drive::Seek, |e| {
        let tl = e.timeline(TimelineOpts::new().paused(true));
        e.tl(tl)
            .to(tid(1), &px(100.0), lin(1.0).delay(0.5), Position::END);
        tl
    }));
    all.extend(sample_case("default_duration", Drive::TotalTime, |e| {
        e.to(tid(1), &px(100.0), TweenOpts::new().paused(true))
    }));
    assert_clean("samples repeat/delay", all);
}

/// [samples design_t5_backward_scrub, design_t6_forward_seek_chained,
/// design_t7_no_early_capture, design_t13_stacked_from,
/// design_t14_zero_duration_direction] chained captures, stacked from()s and
/// the zero-duration direction rule under seeks.
#[test]
fn golden_samples_chained_and_zero() {
    let mut all = Vec::new();
    let chained = |e: &mut TweenEngine| {
        let tl = e.timeline(TimelineOpts::new().paused(true));
        e.tl(tl)
            .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0), Position::END)
            .to(tid(1), &[PropTo::to_f64(X, 200.0)], lin(1.0), Position::END);
        tl
    };
    all.extend(sample_case(
        "design_t5_backward_scrub",
        Drive::Seek,
        chained,
    ));
    all.extend(sample_case(
        "design_t6_forward_seek_chained",
        Drive::Seek,
        chained,
    ));
    all.extend(sample_case(
        "design_t7_no_early_capture",
        Drive::TotalTime,
        chained,
    ));
    all.extend(sample_case("design_t13_stacked_from", Drive::Seek, |e| {
        let tl = e.timeline(TimelineOpts::new().paused(true));
        e.tl(tl)
            .from(
                tid(1),
                &[PropTo::from(X, TweenValue::F64(50.0))],
                lin(1.0),
                Position::END,
            )
            .from(
                tid(1),
                &[PropTo::from(X, TweenValue::F64(80.0))],
                lin(1.0),
                Position::END,
            );
        tl
    }));
    all.extend(sample_case(
        "design_t14_zero_duration_direction",
        Drive::Seek,
        |e| {
            let tl = e.timeline(TimelineOpts::new().paused(true));
            e.tl(tl)
                .to(tid(5), &[PropTo::to_f64(X, 1.0)], lin(2.0), 0.0)
                .set(tid(1), &[PropTo::to_f64(X, 5.0)], TweenOpts::new(), 1.0);
            tl
        },
    ));
    assert_clean("samples chained/zero", all);
}

/// [samples design_repeat_refresh_relative_step, _jump] repeatRefresh with a
/// relative end: stepped .5/1.5/2.5 -> 50/150/250; jumped .5 -> 2.5 -> 150.
#[test]
fn golden_samples_repeat_refresh() {
    let mut all = Vec::new();
    for id in [
        "design_repeat_refresh_relative_step",
        "design_repeat_refresh_relative_jump",
    ] {
        all.extend(sample_case(id, Drive::TotalTime, |e| {
            e.to(
                tid(1),
                &[PropTo::by(X, TweenValue::F64(100.0))],
                paused_lin(1.0).repeat(2).repeat_refresh(true),
            )
        }));
    }
    assert_clean("repeat_refresh", all);
}

/// [samples keyframes_array, keyframes_array_outer_ease,
/// keyframes_percent_default_easeEach, keyframes_percent_outer_ease,
/// design_kf_percent_0_50_warm, design_kf_values_0_100_50,
/// design_kf_ordinary_prop, design_kf_ordinary_prop_outer_ease]
#[test]
fn golden_samples_keyframes() {
    let mut all = Vec::new();
    let steps = [
        KeyStep {
            props: &[PropTo::to_f64(X, 50.0)],
            opts: TweenOpts::new().duration(0.5),
        },
        KeyStep {
            props: &[PropTo::to_f64(X, 20.0)],
            opts: TweenOpts::new().duration(1.0).ease(Easing::InQuad),
        },
    ];
    all.extend(sample_case("keyframes_array", Drive::TotalTime, |e| {
        e.keyframes(tid(1), &steps, TweenOpts::new().paused(true))
    }));
    all.extend(sample_case(
        "keyframes_array_outer_ease",
        Drive::TotalTime,
        |e| {
            e.keyframes(
                tid(1),
                &steps,
                TweenOpts::new().paused(true).ease(Easing::InOutCubic),
            )
        },
    ));
    let k = |at: f64, v: f64| Key {
        at,
        value: TweenValue::F64(v),
        ease: None,
    };
    let keys = [k(0.0, 0.0), k(50.0, 80.0), k(100.0, 100.0)];
    all.extend(sample_case(
        "keyframes_percent_default_easeEach",
        Drive::TotalTime,
        |e| {
            seed(e, 1, X, 5.0);
            e.to(
                tid(1),
                &[PropTo::keys(X, &keys)],
                TweenOpts::new().duration(2.0).paused(true),
            )
        },
    ));
    all.extend(sample_case(
        "keyframes_percent_outer_ease",
        Drive::TotalTime,
        |e| {
            seed(e, 1, X, 5.0);
            e.to(
                tid(1),
                &[PropTo::keys(X, &keys)],
                TweenOpts::new()
                    .duration(2.0)
                    .paused(true)
                    .ease_each(Easing::Linear)
                    .ease(Easing::InCubic),
            )
        },
    ));
    let half = [k(0.0, 0.0), k(50.0, 100.0)];
    all.extend(sample_case(
        "design_kf_percent_0_50_warm",
        Drive::TotalTime,
        |e| {
            let a = e.to(
                tid(1),
                &[PropTo::keys(X, &half)],
                TweenOpts::new().duration(1.0).paused(true),
            );
            e.anim(a).set_total_time(1.0, Emit::Fire);
            e.anim(a).set_total_time(0.0, Emit::Fire);
            a
        },
    ));
    let vals = [
        TweenValue::F64(0.0),
        TweenValue::F64(100.0),
        TweenValue::F64(50.0),
    ];
    all.extend(sample_case(
        "design_kf_values_0_100_50",
        Drive::TotalTime,
        |e| {
            e.to(
                tid(1),
                &[PropTo::values(X, &vals)],
                TweenOpts::new().duration(1.0).paused(true),
            )
        },
    ));
    let ord = [k(50.0, 100.0), k(100.0, 100.0)];
    for (id, ease) in [
        ("design_kf_ordinary_prop", None),
        ("design_kf_ordinary_prop_outer_ease", Some(Easing::InCubic)),
    ] {
        all.extend(sample_case(id, Drive::TotalTime, |e| {
            let mut o = TweenOpts::new().duration(1.0).paused(true);
            if let Some(x) = ease {
                o = o.ease(x);
            }
            e.to(tid(1), &[PropTo::keys(X, &ord), PropTo::to_f64(Y, 50.0)], o)
        }));
    }
    assert_clean("keyframes", all);
}

/// [samples stagger_5_each02_power2in, design_stagger_yoyo_ease,
/// timeline_defaults_ease]
#[test]
fn golden_samples_stagger_and_defaults() {
    let mut all = Vec::new();
    all.extend(sample_case(
        "stagger_5_each02_power2in",
        Drive::TotalTime,
        |e| {
            e.to(
                Targets::Range { first: 1, count: 5 },
                &[PropTo::to_f64(X, 100.0), PropTo::to_f64(Y, 50.0)],
                TweenOpts::new()
                    .duration(1.0)
                    .ease(Easing::InCubic)
                    .stagger(Stagger::each(0.2))
                    .paused(true),
            )
        },
    ));
    all.extend(sample_case(
        "design_stagger_yoyo_ease",
        Drive::TotalTime,
        |e| {
            e.to(
                Targets::Range { first: 1, count: 3 },
                &[PropTo::to_f64(X, 100.0)],
                paused_lin(1.0)
                    .stagger(Stagger::each(0.2))
                    .repeat(1)
                    .yoyo(true)
                    .yoyo_ease(YoyoEase::Ease(Easing::OutQuad)),
            )
        },
    ));
    all.extend(sample_case("timeline_defaults_ease", Drive::Seek, |e| {
        let tl = e.timeline(
            TimelineOpts::new()
                .paused(true)
                .defaults(TweenOpts::new().ease(Easing::InOutCubic).duration(1.0)),
        );
        e.tl(tl)
            .to(
                tid(1),
                &[PropTo::to_f64(X, 100.0)],
                TweenOpts::new(),
                Position::END,
            )
            .to(
                tid(1),
                &[PropTo::to_f64(Y, 100.0)],
                TweenOpts::new().ease(Easing::Linear),
                Position::END,
            );
        tl
    }));
    assert_clean("stagger/defaults", all);
}

// ---------------------------------------------------------------------------
// The forward-walk `break`
// ---------------------------------------------------------------------------

/// GSAP's forward walk renders every child with `_act || time >= _start`; the
/// engine stops at the first child that has not started and is not ACT. A
/// child that was moved directly (a control on a child of a non-smooth
/// timeline makes it ACT with a start AFTER the parent's playhead) and sits
/// behind such an unstarted sibling is then skipped. GSAP renders it at a
/// negative total, which lands its start value (from = 0 here).
#[test]
fn forward_walk_renders_act_children_behind_an_unstarted_sibling() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    seed(&mut e, 2, X, 0.0);
    seed(&mut e, 3, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0), 0.0)
        .to(tid(2), &[PropTo::to_f64(X, 100.0)], lin(1.0), 1.8);
    let b = e
        .tl(tl)
        .to(tid(3), &[PropTo::to_f64(X, 100.0)], lin(1.0), 2.0)
        .last();
    // Drive B by hand while the parent sits at 0 (GSAP: B._act = 1).
    e.anim(b).set_total_time(0.5, Emit::Fire);
    assert!(close(val(&e, 3, X), 50.0, 1e-9));
    // The parent moves forward to 1.5: GSAP renders B at 1.5 - 2 = -0.5 -> 0.
    e.anim(tl).set_total_time(1.5, Emit::Fire);
    assert_eq!(
        val(&e, 3, X),
        0.0,
        "B (ACT, start 2) behind the unstarted C (start 1.8) was not rendered"
    );
    assert_eq!(e.anim_ref(b).total_time(), 0.0);
}

/// [samples keyframes_percent_default_easeEach, reconciliation K3] a key at
/// 0% is a zero-duration set at 0: the first render of the keyframe tween at
/// total time exactly 0 must write it (GSAP: x = 0 at t = 0 on a target
/// that starts at 5). The engine's group inner render skips the walk on a
/// first render at 0 (not initted, inner duration != 0, so no crossing), so
/// the 0% value only appears once the playhead moves past 0.
#[test]
fn keyframes_zero_percent_key_applies_on_first_render_at_zero() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 5.0);
    let keys = [
        Key {
            at: 0.0,
            value: TweenValue::F64(0.0),
            ease: None,
        },
        Key {
            at: 100.0,
            value: TweenValue::F64(100.0),
            ease: None,
        },
    ];
    let a = e.to(
        tid(1),
        &[PropTo::keys(X, &keys)],
        TweenOpts::new().duration(1.0).paused(true),
    );
    assert_eq!(val(&e, 1, X), 5.0, "no immediate render");
    e.anim(a).set_total_time(0.0, Emit::Fire);
    assert_eq!(val(&e, 1, X), 0.0, "the 0% key at t = 0");
    // The same through a timeline seek to the group's start.
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 5.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(
        tid(1),
        &[PropTo::keys(X, &keys)],
        TweenOpts::new().duration(1.0),
        1.0,
    );
    e.anim(tl).set_total_time(1.0, Emit::Fire);
    assert_eq!(
        val(&e, 1, X),
        0.0,
        "the 0% key when the parent reaches the group's start"
    );
}

// ---------------------------------------------------------------------------
// Smooth child timing, time scales without jumps
// ---------------------------------------------------------------------------

fn smooth_r(e: &mut TweenEngine) -> TweenId {
    e.timeline(TimelineOpts::new().paused(true).smooth_child_timing(true))
}

fn step_r(e: &mut TweenEngine, r: TweenId, dt: f64) {
    let now = e.anim_ref(r).total_time();
    e.anim(r).set_total_time(now + dt, Emit::Fire);
}

fn ts_ops(id: &str) -> &'static [golden::common::Pairs] {
    golden::timescale::CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap()
        .ops
}

/// Compares the keys of one timescale.json `ops` record.
fn check_pairs(id: &str, rec: golden::common::Pairs, got: &[(&str, f64)], errs: &mut Vec<String>) {
    let op = golden::common::text(rec, "op").unwrap_or("?");
    for (k, g) in got {
        let w = match golden::common::val(rec, k) {
            Some(Val::F(w)) => w,
            Some(Val::B(b)) => b as u8 as f64,
            _ => continue,
        };
        if !close(*g, w, 1e-9) {
            errs.push(format!("{id} {op}: {k} = {g}, GSAP {w}"));
        }
    }
}

/// [timescale design_t10_nested_time_scales] R (smooth) > tl > tl2 (ts 2, at
/// 1) > tween 0 -> 100 over 2 s: tl.timeScale(0.5) at R 1.5 re-aligns tl to
/// -1.5, R shifts its children and its own time to 3; tl.reverse() then
/// plays back without a jump.
#[test]
fn golden_t10_nested_time_scales() {
    let id = "design_t10_nested_time_scales";
    let ops = ts_ops(id);
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let r = smooth_r(&mut e);
    let tl = e.timeline(TimelineOpts::new());
    e.tl(r).add(tl, 0.0);
    let tl2 = e.timeline(TimelineOpts::new().time_scale(2.0));
    e.tl(tl2)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END);
    e.tl(tl).add(tl2, 1.0);
    let mut errs = Vec::new();
    for rec in ops {
        match golden::common::text(rec, "op").unwrap() {
            "R +1.5" => step_r(&mut e, r, 1.5),
            "R +0.5" => step_r(&mut e, r, 0.5),
            "tl.timeScale(0.5)" => {
                e.anim(tl).set_time_scale(0.5);
            }
            "tl.reverse()" => {
                e.anim(tl).reverse();
            }
            o => panic!("unhandled {o}"),
        }
        let a = e.anim_ref(tl);
        let got = [
            ("x", val(&e, 1, X)),
            ("R_total_time", e.anim_ref(r).total_time()),
            ("tl_time", a.time()),
            ("tl_time_scale", a.time_scale()),
            ("tl_reversed", a.reversed() as u8 as f64),
            ("tl_start", a.start_time()),
        ];
        check_pairs(id, rec, &got, &mut errs);
    }
    assert_clean(id, errs);
}

/// [timescale design_time_scale_no_jump_root_child,
/// design_time_scale_no_jump_nested_smooth, design_smooth_child_timing_duration]
#[test]
fn golden_time_scale_changes_do_not_jump() {
    let mut errs = Vec::new();
    {
        let id = "design_time_scale_no_jump_root_child";
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let r = smooth_r(&mut e);
        let t = e
            .tl(r)
            .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0), Position::END)
            .last();
        for rec in ts_ops(id) {
            match golden::common::text(rec, "op").unwrap() {
                "R.totalTime(0.5)" => e.anim(r).set_total_time(0.5, Emit::Fire),
                "R.totalTime(0.6)" => e.anim(r).set_total_time(0.6, Emit::Fire),
                "R.totalTime(0.7)" => e.anim(r).set_total_time(0.7, Emit::Fire),
                "t.timeScale(2)" => e.anim(t).set_time_scale(2.0),
                "t.timeScale(-1)" => e.anim(t).set_time_scale(-1.0),
                o => panic!("unhandled {o}"),
            };
            let a = e.anim_ref(t);
            let got = [
                ("x", val(&e, 1, X)),
                ("R_total_time", e.anim_ref(r).total_time()),
                ("t_total_time", a.total_time()),
                ("t_start", a.start_time()),
                ("t_time_scale", a.time_scale()),
                ("t_reversed", a.reversed() as u8 as f64),
            ];
            check_pairs(id, rec, &got, &mut errs);
        }
    }
    {
        let id = "design_time_scale_no_jump_nested_smooth";
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let r = smooth_r(&mut e);
        let tl = e.timeline(TimelineOpts::new().smooth_child_timing(true));
        e.tl(r).add(tl, 0.0);
        let t = e
            .tl(tl)
            .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0), Position::END)
            .last();
        for rec in ts_ops(id) {
            match golden::common::text(rec, "op").unwrap() {
                "R +0.5" => step_r(&mut e, r, 0.5),
                "t.timeScale(0.5)" => {
                    e.anim(t).set_time_scale(0.5);
                }
                o => panic!("unhandled {o}"),
            }
            let got = [
                ("x", val(&e, 1, X)),
                ("R_total_time", e.anim_ref(r).total_time()),
                ("tl_duration", e.anim_ref(tl).duration()),
                ("t_start", e.anim_ref(t).start_time()),
                ("t_total_time", e.anim_ref(t).total_time()),
            ];
            check_pairs(id, rec, &got, &mut errs);
        }
    }
    for (id, smooth) in [
        ("design_smooth_child_timing_duration", true),
        ("design_non_smooth_child_timing_duration", false),
    ] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let tl = e.timeline(TimelineOpts::new().paused(true).smooth_child_timing(smooth));
        let c = e
            .tl(tl)
            .to(tid(1), &[PropTo::to_f64(X, 1.0)], lin(1.0), Position::END)
            .last();
        for rec in ts_ops(id) {
            if golden::common::text(rec, "op") == Some("c.timeScale(0.5)") {
                e.anim(c).set_time_scale(0.5);
            }
            let got = [
                ("tl_duration", e.anim_ref(tl).duration()),
                ("c_start", e.anim_ref(c).start_time()),
                ("c_end", e.anim_ref(c).end_time(true)),
            ];
            check_pairs(id, rec, &got, &mut errs);
        }
    }
    assert_clean("time scale no-jump", errs);
}

// ---------------------------------------------------------------------------
// Negative starts at the root level
// ---------------------------------------------------------------------------

/// [positions negative_start_first_child_unpaused_root,
/// design_t3_negative_start_root_level] an un-paused root-level timeline
/// whose child lands before 0: children shift, the timeline's own start
/// moves back so its playhead reads the shift (no visual jump).
#[test]
fn golden_negative_start_root_level() {
    let mut errs = Vec::new();
    for (id, build) in [
        ("negative_start_first_child_unpaused_root", 0u8),
        ("design_t3_negative_start_root_level", 1u8),
    ] {
        let case = golden::positions::CASES
            .iter()
            .find(|c| c.id == id)
            .unwrap();
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let tl = e.timeline(TimelineOpts::new());
        let d1 = TweenOpts::new().duration(1.0);
        let p = [PropTo::to_f64(X, 1.0)];
        let mut ids = Vec::new();
        if build == 0 {
            ids.push(("A", e.tl(tl).to(tid(1), &p, d1, -0.5).last()));
        } else {
            ids.push(("A", e.tl(tl).to(tid(1), &p, d1, Position::END).last()));
            ids.push(("B", e.tl(tl).to(tid(1), &p, d1, Position::rel(-3.0)).last()));
        }
        // GSAP's tl.duration() call: any recompute. A suppressed zero-length
        // control does not move anything.
        // GSAP's tl.duration() recomputes; here the next frame does (the
        // root walk re-derives dirty durations). advance(0) moves no time.
        e.advance(0.0);
        for (n, id2) in &ids {
            let w = case
                .children
                .iter()
                .find(|c| c.name == *n)
                .unwrap()
                .start_time;
            let g = e.anim_ref(*id2).start_time();
            if !close(g, w, 1e-9) {
                errs.push(format!("{id}: {n} start {g}, GSAP {w}"));
            }
        }
        // The engine root then shifts too (5.6.3: the root is a timeline
        // without parent, GSAP's global timeline does the same on its next
        // totalDuration, which the frozen harness never runs): the timeline's
        // start returns to 0 while the root's time moves by the same amount.
        // What must match is the timeline's own playhead.
        let want = num(case.extra, "timeline_time_after");
        let g = e.anim_ref(tl).time();
        if !close(g, want, 1e-9) {
            errs.push(format!("{id}: timeline time {g}, GSAP {want}"));
        }
        // No visual jump: the next frame continues from there.
        e.advance(0.25);
        let g = e.anim_ref(tl).time();
        if !close(g, want + 0.25, 1e-9) {
            errs.push(format!(
                "{id}: after +0.25 the time is {g}, want {}",
                want + 0.25
            ));
        }
    }
    assert_clean("negative start root level", errs);
}

// ---------------------------------------------------------------------------
// Overwrite (design cases)
// ---------------------------------------------------------------------------

/// [overwrite design_auto_single_prop] A loses its only prop at B's first
/// render: Interrupt, removed; B starts from 50 (A's value then).
#[test]
fn golden_overwrite_auto_single_prop() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        lin(1.0).tag(tag("a")).events(EventMask::INTERRUPT),
    );
    e.anim(a).set_total_time(0.5, Emit::Fire);
    let b = e.to(
        tid(1),
        &[PropTo::to_f64(X, 0.0)],
        lin(1.0).overwrite(Overwrite::Auto).paused(true),
    );
    assert!(drain(&mut e, true).is_empty());
    e.anim(b).set_total_time(0.0, Emit::Fire);
    assert_eq!(drain(&mut e, false), ["a:onInterrupt"]);
    assert_eq!(val(&e, 1, X), 50.0);
    e.anim(b).set_total_time(0.5, Emit::Fire);
    assert!(close(val(&e, 1, X), 25.0, 1e-9));
    e.anim(a).set_total_time(0.8, Emit::Fire);
    assert!(
        close(val(&e, 1, X), 25.0, 1e-9),
        "the killed A writes nothing"
    );
}

/// [overwrite design_auto_two_rivals] two running rivals die at the third's
/// first render, newest first (GSAP killTweensOf loops getTweensOf backward).
#[test]
fn golden_overwrite_auto_two_rivals_order() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let ev = EventMask::INTERRUPT;
    let a = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        TweenOpts::new().duration(1.0).tag(tag("a")).events(ev),
    );
    e.anim(a).set_total_time(0.5, Emit::Fire);
    let a2 = e.to(
        tid(1),
        &[PropTo::to_f64(X, 50.0)],
        TweenOpts::new().duration(2.0).tag(tag("b")).events(ev),
    );
    e.anim(a2).set_total_time(0.5, Emit::Fire);
    let c = e.to(
        tid(1),
        &[PropTo::to_f64(X, -100.0)],
        TweenOpts::new()
            .duration(1.0)
            .overwrite(Overwrite::Auto)
            .paused(true),
    );
    drain(&mut e, true);
    e.anim(c).set_total_time(0.0, Emit::Fire);
    assert_eq!(drain(&mut e, false), ["b:onInterrupt", "a:onInterrupt"]);
}

/// [overwrite design_true_multi_target] All is per target: A on [t1, t2]
/// keeps moving t2, no Interrupt.
#[test]
fn golden_overwrite_all_multi_target() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    seed(&mut e, 2, X, 0.0);
    let two = [TargetId(1), TargetId(2)];
    let a = e.to(
        Targets::List(&two),
        &[PropTo::to_f64(X, 100.0)],
        lin(1.0).tag(tag("a")).events(EventMask::INTERRUPT),
    );
    e.anim(a).set_total_time(0.5, Emit::Fire);
    e.to(
        tid(1),
        &[PropTo::to_f64(X, 0.0)],
        lin(1.0).overwrite(Overwrite::All).paused(true),
    );
    e.anim(a).set_total_time(1.0, Emit::Fire);
    assert_eq!((val(&e, 1, X), val(&e, 2, X)), (50.0, 100.0));
    assert!(drain(&mut e, false).is_empty());
}

/// [overwrite design_auto_ignores_finished] a finished tween is not a rival.
#[test]
fn golden_overwrite_auto_ignores_finished() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let a = e.to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        lin(1.0)
            .paused(true)
            .tag(tag("a"))
            .events(EventMask::INTERRUPT),
    );
    e.anim(a).set_total_time(1.0, Emit::Fire);
    let b = e.to(
        tid(1),
        &[PropTo::to_f64(X, 0.0)],
        lin(1.0).overwrite(Overwrite::Auto).paused(true),
    );
    e.anim(b).set_total_time(0.0, Emit::Fire);
    assert!(drain(&mut e, false).is_empty());
    e.anim(a).set_total_time(0.5, Emit::Fire);
    assert!(close(val(&e, 1, X), 50.0, 1e-9), "A still writes");
}

/// [overwrite design_auto_deferred_fromto, design_auto_deferred_to_immediaterender]
/// the auto kill of an immediately rendered tween waits for the first
/// render with time > 0 (fromTo) / happens at creation (to + immediate).
#[test]
fn golden_overwrite_auto_deferred() {
    for (fromto, at_create) in [(true, false), (false, true)] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let a = e.to(
            tid(1),
            &[PropTo::to_f64(X, 100.0)],
            lin(1.0).tag(tag("a")).events(EventMask::INTERRUPT),
        );
        e.anim(a).set_total_time(0.5, Emit::Fire);
        let b = if fromto {
            e.from_to(
                tid(1),
                &[PropTo::from_to(
                    X,
                    TweenValue::F64(-50.0),
                    TweenValue::F64(0.0),
                )],
                lin(1.0).overwrite(Overwrite::Auto).paused(true),
            )
        } else {
            e.to(
                tid(1),
                &[PropTo::to_f64(X, 0.0)],
                TweenOpts::new()
                    .duration(1.0)
                    .overwrite(Overwrite::Auto)
                    .paused(true)
                    .immediate_render(true),
            )
        };
        let at_c = drain(&mut e, false);
        e.anim(b).set_total_time(0.0, Emit::Fire);
        let at_0 = drain(&mut e, false);
        e.anim(b).set_total_time(0.1, Emit::Fire);
        let at_01 = drain(&mut e, false);
        let fired = ["a:onInterrupt".to_string()];
        if at_create {
            assert_eq!(
                (at_c.as_slice(), at_0.len(), at_01.len()),
                (&fired[..], 0, 0)
            );
        } else {
            assert_eq!(
                (at_c.len(), at_0.len(), at_01.as_slice()),
                (0, 0, &fired[..])
            );
            assert!(close(val(&e, 1, X), -45.0, 1e-9));
        }
    }
}

// ---------------------------------------------------------------------------
// Post-add checks, completed timeline growth, kill, labels
// ---------------------------------------------------------------------------

/// [callbacks design_post_add_behind_playhead] a paused timeline at 1 gets a
/// child at 0.2..0.7: rendered at once (suppressed): x = 100, nothing fires.
#[test]
fn golden_post_add_behind_playhead() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    seed(&mut e, 2, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl).to(
        tid(2),
        &[PropTo::to_f64(X, 1.0)],
        TweenOpts::new().duration(2.0),
        Position::END,
    );
    e.anim(tl).set_total_time(1.0, Emit::Fire);
    drain(&mut e, true);
    e.tl(tl).to(
        tid(1),
        &[PropTo::to_f64(X, 100.0)],
        TweenOpts::new().duration(0.5).tag(tag("a")).events(EV),
        0.2,
    );
    assert_eq!(val(&e, 1, X), 100.0);
    assert!(drain(&mut e, true).is_empty());
}

/// [callbacks design_completed_timeline_grows] an un-paused root-level
/// timeline completed; a child appended at its end re-activates it and the
/// next frames play the new child.
#[test]
fn golden_completed_timeline_grows() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    seed(&mut e, 2, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .tag(tag("tl"))
            .events(EventMask::START.with(EventMask::COMPLETE)),
    );
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 1.0)], lin(1.0), Position::END);
    e.anim(tl).set_total_time(1.0, Emit::Fire);
    assert_eq!(drain(&mut e, false), ["tl:onStart", "tl:onComplete"]);
    e.tl(tl)
        .to(tid(2), &[PropTo::to_f64(X, 1.0)], lin(1.0), Position::END);
    assert!(drain(&mut e, false).is_empty());
    let r = e.anim_ref(tl);
    assert_eq!((r.time(), r.progress()), (1.0, 0.5));
    assert!(e.is_active(), "the grown timeline is active again");
    e.advance(0.5);
    assert!(close(val(&e, 2, X), 0.5, 1e-9), "x2 {}", val(&e, 2, X));
}

/// [callbacks design_kill_interrupt] Interrupt only below progress 1.
#[test]
fn golden_kill_interrupt() {
    let mut out = Vec::new();
    for (name, at) in [("t1", Some(0.5)), ("t2", Some(1.0)), ("t3", None)] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let t = e.to(
            tid(1),
            &[PropTo::to_f64(X, 1.0)],
            TweenOpts::new()
                .duration(1.0)
                .paused(true)
                .tag(tag(name))
                .events(EventMask::INTERRUPT.with(EventMask::COMPLETE)),
        );
        if let Some(at) = at {
            e.anim(t).set_total_time(at, Emit::Suppress);
        }
        e.anim(t).kill();
        out.extend(drain(&mut e, false));
    }
    assert_eq!(out, ["t1:onInterrupt", "t3:onInterrupt"]);
}

/// [callbacks design_labels_directions] currentLabel = previousLabel(time +
/// 1e-8); next/previous strict; ties (b and c both at 1): the first added.
#[test]
fn golden_labels_directions() {
    let t = trace("design_labels_directions");
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().paused(true));
    let l = |s: &str| Tag(500 + s.as_bytes()[0] as u64);
    e.tl(tl)
        .to(
            tid(1),
            &[PropTo::to_f64(X, 1.0)],
            TweenOpts::new().duration(3.0),
            0.0,
        )
        .add_label(l("a"), 0.0)
        .add_label(l("b"), 1.0)
        .add_label(l("c"), 1.0)
        .add_label(l("d"), 2.0);
    let name = |x: Option<Tag>| x.map(|t| ((t.0 - 500) as u8 as char).to_string());
    let mut errs = Vec::new();
    for op in t.ops {
        run_op(&mut e, tl, op.op).unwrap();
        let r = e.anim_ref(tl);
        for (k, g) in [
            ("current", name(r.current_label())),
            ("next", name(r.next_label())),
            ("previous", name(r.previous_label())),
        ] {
            let w = golden::common::text(op.values, k).map(str::to_string);
            if g != w {
                errs.push(format!("{}: {k} = {g:?}, GSAP {w:?}", op.op));
            }
        }
    }
    assert_clean("design_labels_directions", errs);
}

// ---------------------------------------------------------------------------
// Root-level lifecycle, staggered from(), root time scale
// ---------------------------------------------------------------------------

/// A root-level timeline reversed to 0 by frames fires ReverseComplete and
/// leaves the root (GSAP `_removeFromParent` at `!tTime && ts < 0`); play()
/// then re-links it and the frames drive it forward again from 0.
#[test]
fn reversed_root_timeline_reverse_completes_then_plays_again() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new().tag(tag("tl")).events(
            EventMask::START
                .with(EventMask::COMPLETE)
                .with(EventMask::REVERSE_COMPLETE),
        ),
    );
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0), Position::END);
    e.advance(0.6);
    e.anim(tl).reverse();
    e.advance(0.5);
    assert!(close(val(&e, 1, X), 10.0, 1e-9), "x {}", val(&e, 1, X));
    e.advance(0.5);
    assert_eq!(val(&e, 1, X), 0.0);
    assert_eq!(drain(&mut e, false), ["tl:onStart", "tl:onReverseComplete"]);
    assert!(e.anim_ref(tl).is_alive(), "a timeline is kept (keep: true)");
    assert!(!e.is_active(), "it left the root");
    e.anim(tl).play();
    assert!(e.is_active(), "play() re-links it");
    e.advance(0.25);
    assert!(close(val(&e, 1, X), 25.0, 1e-9), "x {}", val(&e, 1, X));
    e.advance(1.0);
    assert_eq!(val(&e, 1, X), 100.0);
    assert_eq!(drain(&mut e, false), ["tl:onStart", "tl:onComplete"]);
}

/// A staggered from() renders every target's from value at creation (GSAP:
/// each sub-tween is an immediate-render from), even the ones that start
/// later; the frames then run them in stagger order.
#[test]
fn staggered_from_renders_all_start_values_at_creation() {
    let mut e = TweenEngine::new();
    for t in 1..=3 {
        seed(&mut e, t, X, 0.0);
    }
    let a = e.from(
        Targets::Range { first: 1, count: 3 },
        &[PropTo::from(X, TweenValue::F64(100.0))],
        lin(1.0).stagger(Stagger::each(0.5)).paused(true),
    );
    assert_eq!([val(&e, 1, X), val(&e, 2, X), val(&e, 3, X)], [100.0; 3]);
    e.anim(a).set_total_time(0.75, Emit::Fire);
    let got = [val(&e, 1, X), val(&e, 2, X), val(&e, 3, X)];
    assert!(
        close(got[0], 25.0, 1e-9) && close(got[1], 75.0, 1e-9) && got[2] == 100.0,
        "{got:?}"
    );
    e.anim(a).set_total_time(0.0, Emit::Fire);
    assert_eq!([val(&e, 1, X), val(&e, 2, X), val(&e, 3, X)], [100.0; 3]);
}

/// The root time scale (GSAP `gsap.globalTimeline.timeScale()`) scales every
/// frame's dt.
#[test]
fn root_time_scale_scales_frames() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    e.to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(1.0));
    e.set_root_time_scale(2.0);
    e.advance(0.25);
    assert!(close(val(&e, 1, X), 50.0, 1e-9), "x {}", val(&e, 1, X));
}

/// A timeline paused by add_pause going BACKWARD resumes backward past it
/// (the pause is not found again from its own start) and reverse-completes.
#[test]
fn add_pause_resume_continues_in_the_same_direction() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .tag(tag("tl"))
            .events(EventMask::REVERSE_COMPLETE),
    );
    e.tl(tl)
        .to(tid(1), &[PropTo::to_f64(X, 100.0)], lin(2.0), Position::END)
        .add_pause(1.0, tag("pause"));
    e.anim(tl).set_total_time(1.8, Emit::Suppress);
    e.anim(tl).reverse();
    e.advance(1.0);
    assert_eq!(drain(&mut e, false), ["pause:cb"]);
    assert_eq!(e.anim_ref(tl).time(), 1.0);
    e.anim(tl).resume();
    e.advance(0.5);
    assert!(close(val(&e, 1, X), 25.0, 1e-9), "x {}", val(&e, 1, X));
    e.advance(1.0);
    assert_eq!(val(&e, 1, X), 0.0);
    assert_eq!(drain(&mut e, false), ["tl:onReverseComplete"]);
}

// ---------------------------------------------------------------------------
// Found by differential fuzzing against GSAP 3.15 (scratchpad fuzz/gen*.js):
// random timelines replayed in real GSAP and through the engine.
// ---------------------------------------------------------------------------

/// A root-level timeline reversed to 0 by frames is reverse-completed and
/// unlinked from the root, and the root, now empty, rebases its clock to 0.
/// `pause()` then records its paused time from `raw_time()`, which folds the
/// STALE start through the rebased root clock and yields the total duration
/// instead of 0; `resume()` / `reverse()` / `play()` then jump the timeline to
/// its END, firing Start, every child callback and Complete.
/// GSAP (monotonic root clock): `_pTime = _tTime || max(-delay, rawTime())`
/// is 0 here (rawTime is negative for a reversed timeline past its start),
/// so resume() stays at 0 and fires nothing [fuzz seed 21 idx 281].
#[test]
fn pause_resume_after_reverse_complete_stays_at_zero() {
    for resume_with in ["resume", "reverse", "play"] {
        let mut e = TweenEngine::new();
        seed(&mut e, 1, X, 0.0);
        let tl = e.timeline(TimelineOpts::new().tag(tag("tl")).events(EV));
        e.tl(tl).to(
            tid(1),
            &[PropTo::to_f64(X, 100.0)],
            lin(1.0).tag(tag("a")).events(EV),
            Position::END,
        );
        e.advance(0.5);
        e.anim(tl).reverse();
        e.advance(1.0); // reverse-completes at 0
        assert_eq!(val(&e, 1, X), 0.0);
        drain(&mut e, true);
        e.anim(tl).pause();
        match resume_with {
            "resume" => {
                e.anim(tl).resume();
            }
            "reverse" => {
                e.anim(tl).reverse();
            }
            _ => {
                e.anim(tl).play();
            }
        }
        let fired = drain(&mut e, false);
        let r = e.anim_ref(tl);
        assert_eq!(
            (r.total_time(), val(&e, 1, X), fired.len()),
            (0.0, 0.0, 0),
            "{resume_with}: the timeline jumped (fired {fired:?})"
        );
        if resume_with == "play" {
            e.advance(0.25);
            assert!(close(val(&e, 1, X), 25.0, 1e-9), "play runs forward from 0");
        }
    }
}

/// A playing (un-paused) repeating timeline whose last child is a nested
/// timeline starting with a zero-duration set, moved backward across its
/// repeat boundary: the boundary sweep renders the nested timeline at its
/// start (0) and the set then sees ratio 1 (x = 7) and stays applied,
/// although the playhead (1.4) is before the nested timeline (1.5). GSAP's
/// `_renderZeroDurationTween` also yields ratio 0 when `!start &&
/// _parentPlayheadIsBeforeStart` (the parent's REAL raw time, here -0.1), a
/// clause the design dropped (5.9: "the walk passes the negative total down")
/// but which the lock-2 wrap render at exactly 0 bypasses. GSAP 3.15:
/// x = 0 [fuzz seed 11 idx 298; probe: nested=true paused=false].
/// (A PAUSED parent gives 7 in GSAP too: its rawTime is the sweep's own
/// time.)
#[test]
fn nested_zero_duration_start_after_backward_repeat_crossing() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    seed(&mut e, 9, X, 0.0);
    let tl = e.timeline(TimelineOpts::new().repeat(1));
    e.tl(tl).to(
        tid(9),
        &[PropTo::to_f64(X, 1.0)],
        TweenOpts::new().duration(1.5),
        0.0,
    );
    let inner = e.timeline(TimelineOpts::new());
    e.tl(inner).set(
        tid(1),
        &[PropTo::to_f64(X, 7.0)],
        TweenOpts::new().tag(tag("set")).events(EventMask::EDGES),
        0.0,
    );
    e.tl(tl).add(inner, 1.5);
    e.anim(tl).set_total_time(2.9, Emit::Fire);
    assert_eq!(val(&e, 1, X), 0.0);
    assert_eq!(drain(&mut e, false), ["set:onComplete"]);
    e.anim(tl).set_total_time(1.4, Emit::Fire);
    assert_eq!(val(&e, 1, X), 0.0, "the set at 1.5 is still applied at 1.4");
}

/// GSAP keeps a rendered nested timeline `_act = !!timeScale`, so every
/// forward walk renders it even before its start; after the outer repeat
/// wraps, that render crosses the child's zTime and reports its onUpdate in
/// the same call. The engine's ACT (0 < tt < tdur) and the forward `break`
/// skip it, so the child's Update arrives one render late (at the start of
/// the next call). Values are unaffected [fuzz seed 7 idx 201].
#[test]
fn nested_timeline_update_after_outer_wrap_is_not_deferred() {
    let mut e = TweenEngine::new();
    seed(&mut e, 1, X, 0.0);
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(2)
            .tag(tag("tl"))
            .events(EV),
    );
    let c0 = e.timeline(
        TimelineOpts::new()
            .repeat(1)
            .repeat_delay(0.25)
            .tag(tag("c"))
            .events(EV),
    );
    e.tl(c0)
        .to(
            tid(1),
            &[PropTo::to_f64(X, 10.0)],
            lin(0.5).tag(tag("a")).events(EV),
            1.5,
        )
        .set(
            tid(1),
            &[PropTo::to_f64(X, 7.0)],
            TweenOpts::new().tag(tag("b")).events(EV),
            2.0,
        );
    e.tl(tl).add(c0, 1.5);
    e.anim(tl).set_total_time(1.9, Emit::Suppress);
    drain(&mut e, true);
    e.anim(tl).set_total_time(6.2, Emit::Fire);
    let got = drain(&mut e, true);
    let want = [
        "a:onStart",
        "a:onUpdate",
        "a:onComplete",
        "b:onUpdate",
        "b:onComplete",
        "c:onUpdate",
        "c:onRepeat",
        "a:onStart",
        "a:onUpdate",
        "a:onComplete",
        "b:onUpdate",
        "b:onComplete",
        "c:onUpdate",
        "c:onComplete",
        "tl:onUpdate",
        "tl:onRepeat",
        "c:onUpdate",
        "tl:onUpdate",
    ];
    assert_eq!(got, want);
    e.anim(tl).set_total_time(1.3, Emit::Fire);
    let got = drain(&mut e, true);
    assert_eq!(
        got.first().map(String::as_str),
        Some("tl:onUpdate"),
        "no deferred c:onUpdate at the start of the next render: {got:?}"
    );
}
