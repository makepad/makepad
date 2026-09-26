//! Timeline positions, labels and insertion (design 12.4), replayed against
//! the GSAP 3.15 position cases (tests/golden/positions.rs). The simple
//! cases are interpreted straight from their GSAP calls; the rest are built
//! by hand. Starts and durations are round7 outputs: compared within 1e-9.

mod util;

#[path = "golden"]
mod golden {
    pub mod common;
    pub mod positions;
}

use golden::common::{num, val as gval, Val};
use golden::positions::{PositionCase, CASES};
use makepad_tween::*;
use util::*;

fn case(id: &str) -> &'static PositionCase {
    CASES
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no position case {id}"))
}

/// Label names to tags (stable per name).
fn label_tag(name: &str) -> Tag {
    Tag(name.bytes().fold(1469598103934665603u64, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    }))
}

/// A built timeline and the names of its children.
struct Built {
    e: TweenEngine,
    tl: TweenId,
    kids: Vec<(String, TweenId)>,
    at_add: Vec<(String, f64)>,
    labels: Vec<String>,
}

impl Built {
    fn new(paused: bool) -> Self {
        let mut e = TweenEngine::new();
        e.seed(tg(0), X, TweenValue::F64(0.0));
        let tl = e.timeline(TimelineOpts::new().paused(paused));
        Built {
            e,
            tl,
            kids: Vec::new(),
            at_add: Vec::new(),
            labels: Vec::new(),
        }
    }

    fn id(&self, name: &str) -> TweenId {
        self.kids
            .iter()
            .find(|k| k.0 == name)
            .unwrap_or_else(|| panic!("no child {name}"))
            .1
    }

    fn pos(&mut self, s: &str) -> Position {
        let s = s.trim();
        if s.is_empty() {
            return Position::END;
        }
        let unq = s.trim_matches('"');
        if unq.len() != s.len() {
            for part in split_label(unq) {
                if !self.labels.iter().any(|l| l == part) {
                    self.labels.push(part.to_string());
                }
            }
            return Position::parse(unq, label_tag).unwrap_or_else(|e| panic!("{s}: {e:?}"));
        }
        Position::at(eval_num(s))
    }

    fn named(&mut self, name: &str) {
        // Unnamed children are A, B, C... in the golden tables.
        let auto = ((b'A' + self.kids.len() as u8) as char).to_string();
        let name = if name.is_empty() { auto.as_str() } else { name };
        let id = self.e.tl(self.tl).last();
        let start = self.e.anim_ref(id).start_time();
        self.kids.push((name.to_string(), id));
        self.at_add.push((name.to_string(), start));
    }

    /// Runs one GSAP call line (`tl.to(o,{duration:1},"+=0.5") // B`).
    fn call(&mut self, line: &str) {
        let (code, name) = match line.split_once("//") {
            Some((c, n)) => (c.trim(), n.trim().split_whitespace().next().unwrap_or("")),
            None => (line.trim(), ""),
        };
        let code = code.trim_end_matches(';').trim();
        if code.starts_with("tl = gsap.timeline") || code.is_empty() {
            return;
        }
        let (head, rest) = code.split_once('(').unwrap();
        let args = rest.strip_suffix(')').unwrap();
        match head {
            "tl.to" | "tl.set" => {
                let open = args.find('{').unwrap();
                let open = args[open + 1..]
                    .find('{')
                    .map(|i| open + 1 + i)
                    .unwrap_or(open);
                let close = args[open..].find('}').unwrap() + open;
                let vars = &args[open + 1..close];
                let pos = args[close + 1..].trim_start_matches(',');
                let mut o = TweenOpts::new();
                for kv in vars.split(',') {
                    let (k, v) = kv.split_once(':').unwrap();
                    let v = eval_num(v);
                    o = match k.trim() {
                        "duration" => o.duration(v),
                        "repeat" => o.repeat(v as i32),
                        "repeatDelay" => o.repeat_delay(v),
                        "delay" => o.delay(v),
                        _ => o,
                    };
                }
                let p = self.pos(pos);
                let tl = self.tl;
                if head == "tl.set" {
                    self.e.tl(tl).set(one(0), &[to(X, 1.0)], o, p);
                } else {
                    self.e.tl(tl).to(one(0), &[to(X, 1.0)], o, p);
                }
                self.named(name);
            }
            "tl.call" => {
                let pos = args.rsplit(',').next().unwrap();
                let p = self.pos(pos);
                let tl = self.tl;
                self.e.tl(tl).call(Tag(99), p);
                self.named(name);
            }
            "tl.addPause" => {
                let p = self.pos(args);
                let tl = self.tl;
                self.e.tl(tl).add_pause(p, Tag::NONE);
                self.named("<pause>");
            }
            "tl.addLabel" => {
                let (l, pos) = match args.split_once(',') {
                    Some((l, p)) => (l, p),
                    None => (args, ""),
                };
                let l = l.trim().trim_matches('"').to_string();
                let p = self.pos(pos);
                if !self.labels.contains(&l) {
                    self.labels.push(l.clone());
                }
                let tl = self.tl;
                self.e.tl(tl).add_label(label_tag(&l), p);
            }
            "tl.shiftChildren" => {
                let a: Vec<&str> = args.split(',').map(str::trim).collect();
                let amount = eval_num(a[0]);
                let adjust = a.get(1) == Some(&"true");
                let ignore = a.get(2).map(|v| eval_num(v)).unwrap_or(0.0);
                let tl = self.tl;
                self.e.tl(tl).shift_children(amount, adjust, ignore);
            }
            "tl.duration" => self.settle(),
            other => panic!("unsupported call {other}"),
        }
    }

    /// GSAP's `tl.duration()` getter re-derives a dirty timeline (shifting a
    /// negative start); the engine's getters are pure, a render settles it.
    fn settle(&mut self) {
        let t = self.e.anim_ref(self.tl).total_time();
        self.e.anim(self.tl).seek(Seek::Time(t), Emit::Suppress);
    }

    #[track_caller]
    fn check(&mut self, c: &PositionCase) {
        let id = c.id;
        for (name, want) in c.start_time_at_add {
            if let Some((_, got)) = self.at_add.iter().find(|k| k.0 == *name) {
                close(*got, *want, TIME_TOL, &format!("{id}: {name} start at add"));
            }
        }
        self.settle();
        let e = &self.e;
        let tl = e.anim_ref(self.tl);
        close(
            tl.duration(),
            c.duration,
            TIME_TOL,
            &format!("{id}: duration"),
        );
        close(
            tl.total_duration(),
            c.total_duration,
            TIME_TOL,
            &format!("{id}: totalDuration"),
        );
        assert_eq!(tl.child_count(), c.child_count, "{id}: child count");
        for ch in c.children {
            let r = e.anim_ref(self.id(ch.name));
            let ctx = format!("{id}: child {}", ch.name);
            close(
                r.start_time(),
                ch.start_time,
                TIME_TOL,
                &format!("{ctx} start"),
            );
            close(
                r.duration(),
                ch.duration,
                TIME_TOL,
                &format!("{ctx} duration"),
            );
            close(
                r.total_duration(),
                ch.total_duration,
                TIME_TOL,
                &format!("{ctx} totalDuration"),
            );
            close(
                r.end_time(true),
                ch.end_time,
                TIME_TOL,
                &format!("{ctx} end"),
            );
            close(
                r.end_time(false),
                ch.end_time_no_repeats,
                TIME_TOL,
                &format!("{ctx} end without repeats"),
            );
            assert_eq!(r.time_scale(), ch.time_scale, "{ctx} timeScale");
        }
        for (name, want) in c.labels {
            let got = tl.label_time(label_tag(name));
            assert_eq!(
                got.map(|g| (g - want).abs() < TIME_TOL),
                Some(true),
                "{id}: label {name} = {got:?}, want {want}"
            );
        }
        if let Some(r) = c.recent {
            let got = self.e.tl(self.tl).last();
            assert_eq!(got, self.id(r), "{id}: recent");
        }
    }
}

/// The label names a position string mentions ("intro+=1" -> "intro").
fn split_label(s: &str) -> Vec<&str> {
    if s.starts_with('<') || s.starts_with('>') || s.starts_with('+') || s.starts_with('-') {
        return Vec::new();
    }
    if s.parse::<f64>().is_ok() {
        return Vec::new();
    }
    let name = match s.find('=') {
        Some(i) if i > 0 => &s[..i - 1],
        _ => s,
    };
    vec![name]
}

/// A number literal or `a/b`.
fn eval_num(s: &str) -> f64 {
    let s = s.trim();
    match s.split_once('/') {
        Some((a, b)) => eval_num(a) / eval_num(b),
        None => s
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("not a number: {s:?}")),
    }
}

fn interpret(id: &str) {
    let c = case(id);
    let mut b = Built::new(true);
    for line in c.gsap_calls {
        for part in line.split(" ; ") {
            b.call(part);
        }
    }
    b.check(c);
}

/// Every case the interpreter can build from its calls alone.
const INTERPRETED: &[&str] = &[
    "seq_defaults",
    "absolute_out_of_order",
    "relative_plus_minus",
    "relative_plus_first_child",
    "relative_plus_is_timeline_end_not_previous",
    "default_position_is_timeline_end",
    "anchor_lt",
    "anchor_lt0_25",
    "anchor_ltneg_0_1",
    "anchor_gt",
    "anchor_gt0_3",
    "anchor_gtneg_0_2",
    "anchor_ltplus_25pct",
    "anchor_gtminus_50pct",
    "anchor_lt25pct",
    "anchor_gt50pct",
    "anchor_gtneg_50pct",
    "anchor_plus_25pct",
    "anchor_minus_50pct",
    "labels_cumulative",
    "label_missing_with_offset",
    "label_isolated_intro_plus_50pct",
    "addLabel_no_position",
    "previous_lt_is_last_added",
    "previous_gt_is_last_added",
    "previous_chain",
    "negative_start_then_more",
    "gt_after_repeat2",
    "gt_after_repeat2_repeatDelay",
    "gt_after_repeat_infinite",
    "call_plus_1",
    "set_at_2",
    "zero_duration_at_lt",
    "shiftChildren_1",
    "shiftChildren_1_adjustLabels",
    "shiftChildren_1_ignoreBefore_1",
    "rounding_start_and_duration",
    "design_t1_positions",
    "design_t2_repeats_delays_infinite",
    "design_round7_negative_ties",
];

#[test]
fn golden_interpreted_position_cases() {
    for id in INTERPRETED {
        interpret(id);
    }
}

#[test]
fn gsap_positions_golden() {
    // T1 [golden: design_t1_positions]: also checked through the interpreter.
    interpret("design_t1_positions");
    let c = case("design_t1_positions");
    assert_eq!(c.duration, 11.0);
}

#[test]
fn repeats_delays_and_infinite_recent() {
    interpret("design_t2_repeats_delays_infinite");
}

#[test]
fn golden_add_pause_is_a_zero_duration_child() {
    let c = case("addPause_2");
    let mut b = Built::new(true);
    for n in ["A", "B", "C"] {
        b.call(&format!("tl.to(o,{{duration:1}}) // {n}"));
    }
    b.call("tl.addPause(2)");
    b.check(c);
}

#[test]
fn golden_remove_recent_falls_back_to_last() {
    let c = case("design_remove_recent");
    let mut b = Built::new(true);
    b.call("tl.to(o,{duration:1},0) // A");
    b.call("tl.to(o,{duration:1},5) // B");
    b.call("tl.to(o,{duration:1},2) // X");
    let x = b.id("X");
    let tl = b.tl;
    b.e.tl(tl).remove(x);
    b.call("tl.to(o,{duration:1},\"<\") // C");
    b.kids.retain(|k| k.0 != "X");
    b.check(c);
}

#[test]
fn golden_empty_timeline_anchors() {
    let c = case("design_empty_timeline_anchors");
    for (pos, key) in [
        (">", "gt"),
        (">0.5", "gt_0_5"),
        ("<0.5", "lt_0_5"),
        ("+=1", "plus_1"),
    ] {
        let mut b = Built::new(true);
        b.call(&format!("tl.to(o,{{duration:1}},\"{pos}\") // A"));
        let got = b.e.anim_ref(b.id("A")).start_time();
        assert_eq!(got, num(c.extra, key), "{pos}");
    }
    let mut b = Built::new(true);
    b.call("tl.to(o,{duration:1},\"L\") // A");
    assert_eq!(
        b.e.anim_ref(b.tl).label_time(label_tag("L")),
        Some(num(c.extra, "label_l"))
    );
}

#[test]
fn negative_start_shifts_children_not_labels() {
    // T3, nested under a non-smooth (paused) parent [golden: design_t3_negative_start_nested].
    let c = case("design_t3_negative_start_nested");
    let mut e = TweenEngine::new();
    let outer = e.timeline(TimelineOpts::new().paused(true));
    let inner = e.timeline(TimelineOpts::new());
    e.tl(outer).add(inner, 0.0);
    e.tl(inner).to(
        one(0),
        &[to(X, 1.0)],
        TweenOpts::new().duration(1.0),
        Position::END,
    );
    let a = e.tl(inner).last();
    e.tl(inner).add_label(Tag(1), 0.5);
    e.tl(inner).to(
        one(0),
        &[to(X, 1.0)],
        TweenOpts::new().duration(1.0),
        Position::rel(-3.0),
    );
    let bb = e.tl(inner).last();
    assert_eq!(e.anim_ref(bb).start_time(), -2.0, "at add");
    // Settle (GSAP inner.duration()): render the outer.
    e.anim(outer).seek(Seek::Time(0.0), Emit::Suppress);
    assert_eq!(e.anim_ref(bb).start_time(), 0.0);
    assert_eq!(e.anim_ref(a).start_time(), 2.0);
    assert_eq!(e.anim_ref(inner).duration(), c.duration);
    assert_eq!(
        e.anim_ref(inner).label_time(Tag(1)),
        Some(0.5),
        "labels do not move"
    );
    assert_eq!(
        e.anim_ref(inner).start_time(),
        c.timeline_start_time_in_parent
    );
    assert_eq!(e.anim_ref(outer).duration(), num(c.extra, "outer_duration"));

    // Root-level, unpaused [golden: design_t3_negative_start_root_level]:
    // the timeline's own start moves back and its playhead reads 2.
    let c = case("design_t3_negative_start_root_level");
    let mut e = TweenEngine::new();
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl)
        .to(
            one(0),
            &[to(X, 1.0)],
            TweenOpts::new().duration(1.0),
            Position::END,
        )
        .to(
            one(0),
            &[to(X, 1.0)],
            TweenOpts::new().duration(1.0),
            Position::rel(-3.0),
        );
    e.advance(0.0);
    assert_eq!(e.anim_ref(tl).duration(), c.duration);
    assert_eq!(e.anim_ref(tl).time(), num(c.extra, "timeline_time_after"));
    assert_eq!(
        e.anim_ref(tl).total_time(),
        num(c.extra, "timeline_total_time_after")
    );
}

#[test]
fn golden_negative_start_owner_moves() {
    // Unpaused root-level: start -0.5, time 0.5 [golden: negative_start_first_child_unpaused_root].
    let c = case("negative_start_first_child_unpaused_root");
    let mut e = TweenEngine::new();
    let tl = e.timeline(TimelineOpts::new());
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], TweenOpts::new().duration(1.0), -0.5);
    let a = e.tl(tl).last();
    e.advance(0.0);
    assert_eq!(e.anim_ref(a).start_time(), 0.0);
    assert_eq!(e.anim_ref(tl).time(), num(c.extra, "timeline_time_after"));
    // Paused root-level: GSAP divides by _ts = 0 and moves the start to
    // -Infinity (bug); the engine divides by the requested scale (deviation).
    let c = case("negative_start_first_child");
    assert_eq!(c.timeline_start_time_in_parent, f64::NEG_INFINITY);
    let mut e = TweenEngine::new();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], TweenOpts::new().duration(1.0), -0.5);
    let a = e.tl(tl).last();
    e.anim(tl).seek(Seek::Time(0.0), Emit::Suppress);
    assert_eq!(e.anim_ref(a).start_time(), 0.0);
    assert_eq!(e.anim_ref(tl).duration(), c.duration);
    assert!(e.anim_ref(tl).start_time().is_finite());
    // Nested in a paused (non-smooth) parent: the inner start stays.
    let c = case("negative_start_nested_in_paused_parent");
    let mut e = TweenEngine::new();
    let outer = e.timeline(TimelineOpts::new().paused(true));
    let inner = e.timeline(TimelineOpts::new());
    e.tl(outer).add(inner, 1.0);
    e.tl(inner)
        .to(one(0), &[to(X, 1.0)], TweenOpts::new().duration(1.0), -0.5);
    e.anim(outer).seek(Seek::Time(0.0), Emit::Suppress);
    assert_eq!(
        e.anim_ref(inner).start_time(),
        c.timeline_start_time_in_parent
    );
    assert_eq!(e.anim_ref(outer).duration(), num(c.extra, "outer_duration"));
}

#[test]
fn golden_nested_timelines_and_time_scale() {
    for (id, before, after) in [
        ("nested_inner_repeat1", false, false),
        ("nested_inner_repeat1_timescale2_before_add", true, false),
        ("nested_inner_repeat1_timescale2_after_add", false, true),
    ] {
        let c = case(id);
        let mut e = TweenEngine::new();
        let tl = e.timeline(TimelineOpts::new().paused(true));
        e.tl(tl).to(
            one(0),
            &[to(X, 1.0)],
            TweenOpts::new().duration(1.0),
            Position::END,
        );
        let inner = e.timeline(TimelineOpts::new().repeat(1));
        e.tl(inner)
            .to(
                one(0),
                &[to(X, 1.0)],
                TweenOpts::new().duration(1.0),
                Position::END,
            )
            .to(
                one(0),
                &[to(X, 1.0)],
                TweenOpts::new().duration(1.0),
                Position::END,
            );
        if before {
            e.anim(inner).set_time_scale(2.0);
        }
        e.tl(tl).add(inner, Position::rel(1.0));
        if after {
            e.anim(inner).set_time_scale(2.0);
        }
        let r = e.anim_ref(inner);
        assert_eq!(r.start_time(), num(c.extra, "inner_startTime"), "{id}");
        assert_eq!(r.duration(), num(c.extra, "inner_duration"), "{id}");
        assert_eq!(
            r.total_duration(),
            num(c.extra, "inner_totalDuration"),
            "{id}"
        );
        assert_eq!(r.end_time(true), num(c.extra, "inner_endTime"), "{id}");
        assert_eq!(r.time_scale(), num(c.extra, "inner_timeScale"), "{id}");
        assert_eq!(
            e.anim_ref(tl).duration(),
            num(c.extra, "outer_duration"),
            "{id}"
        );
    }
}

#[test]
fn golden_repeating_yoyo_timeline_table() {
    let c = case("timeline_repeat2_rdelay05_yoyo");
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    let tl = e.timeline(
        TimelineOpts::new()
            .paused(true)
            .repeat(2)
            .repeat_delay(0.5)
            .yoyo(true),
    );
    e.tl(tl)
        .to(one(0), &[to(X, 100.0)], lin(1.0), Position::END);
    let n = num(c.extra, "samples.len") as usize;
    for i in 0..n {
        let k = |f: &str| num(c.extra, &format!("samples.{i}.{f}"));
        let t = k("totalTime_set");
        e.anim(tl).set_total_time(t, Emit::Fire);
        let r = e.anim_ref(tl);
        let ctx = format!("totalTime({t})");
        assert_eq!(r.iteration() as f64, k("iteration"), "{ctx}");
        close(r.time(), k("time"), TIME_TOL, &ctx);
        close(r.total_time(), k("totalTime"), TIME_TOL, &ctx);
        close(r.progress(), k("progress"), TIME_TOL, &ctx);
        close(r.total_progress(), k("totalProgress"), TIME_TOL, &ctx);
        close(val(&e, 0, X), k("x"), VAL_TOL, &ctx);
    }
}

#[test]
fn golden_paused_child_duration_deviate_resume_rederives() {
    // GSAP keeps the stale duration 1 after B resumes until something
    // uncaches the timeline; the engine re-derives at once (deviation).
    let c = case("design_paused_child_duration");
    let mut e = TweenEngine::new();
    let tl = e.timeline(TimelineOpts::new().paused(true));
    e.tl(tl)
        .to(one(0), &[to(X, 1.0)], TweenOpts::new().duration(1.0), 0.0);
    let bb = e.to(
        one(0),
        &[to(X, 1.0)],
        TweenOpts::new().duration(2.0).paused(true),
    );
    e.tl(tl).add(bb, 0.0);
    assert_eq!(
        e.anim_ref(tl).duration(),
        num(c.extra, "duration_with_b_paused")
    );
    e.anim(bb).set_paused(false);
    assert_eq!(
        e.anim_ref(tl).duration(),
        num(c.extra, "duration_after_uncache")
    );
}

#[test]
fn golden_zero_duration_repeat_deviate_ignored() {
    // GSAP: 0 (repeat 2), 1e10 (repeat -1), 1 (repeat 2, delay .5); the
    // engine ignores repeat on zero-duration nodes (D11).
    let c = case("design_zero_duration_repeat_tdur");
    assert_eq!(num(c.extra, "tdur_repeat_2"), 0.0);
    let mut e = TweenEngine::new();
    for o in [
        TweenOpts::new().repeat(2),
        TweenOpts::new().repeat(-1),
        TweenOpts::new().repeat(2).repeat_delay(0.5),
    ] {
        let t = e.to(one(0), &[to(X, 1.0)], o.duration(0.0).paused(true));
        assert_eq!(e.anim_ref(t).total_duration(), 0.0);
    }
}

#[test]
fn golden_add_label_percent_deviate_is_zero() {
    // GSAP reads addLabel("p", "+=50%") as end + 50 SECONDS (52) and throws
    // on "<+=50%"; the engine adds 0 for a percent of a missing child.
    let c = case("design_add_label_percent_quirk");
    assert_eq!(gval(&c.labels_as_pairs(), "p"), Some(Val::F(52.0)));
    let mut b = Built::new(true);
    b.call("tl.to(o,{duration:2}) // A");
    b.call("tl.addLabel(\"p\",\"+=50%\")");
    b.call("tl.addLabel(\"r\",\"<50%\")");
    b.call("tl.addLabel(\"q\",\"<+=50%\")");
    let r = b.e.anim_ref(b.tl);
    assert_eq!(r.label_time(label_tag("p")), Some(2.0));
    assert_eq!(
        r.label_time(label_tag("r")),
        Some(1.0),
        "percent of the previous child: GSAP too"
    );
    assert_eq!(r.label_time(label_tag("q")), Some(0.0));
}

trait LabelsAsPairs {
    fn labels_as_pairs(&self) -> Vec<(&'static str, Val)>;
}
impl LabelsAsPairs for PositionCase {
    fn labels_as_pairs(&self) -> Vec<(&'static str, Val)> {
        self.labels.iter().map(|(k, v)| (*k, Val::F(*v))).collect()
    }
}

#[test]
fn golden_numeric_string_deviate_is_a_time() {
    // GSAP prefers an existing label named "3"; Position::parse makes every
    // numeric string a time.
    let c = case("label_numeric_string");
    let mut b = Built::new(true);
    b.call("tl.to(o,{duration:1},\"2\") // A");
    b.call("tl.addLabel(\"3\", 0.5)");
    b.call("tl.to(o,{duration:1},\"3\") // B");
    assert_eq!(b.e.anim_ref(b.id("A")).start_time(), 2.0);
    let gsap_b = c
        .children
        .iter()
        .find(|ch| ch.name == "B")
        .unwrap()
        .start_time;
    assert_eq!(gsap_b, 0.5);
    assert_eq!(b.e.anim_ref(b.id("B")).start_time(), 3.0);
}

#[test]
fn position_parse_accepts_every_gsap_form() {
    // Every anchor case string resolves through Position::parse to GSAP's start.
    for c in CASES.iter().filter(|c| c.id.starts_with("anchor_")) {
        let pos = match gval(c.extra, "position") {
            Some(Val::S(s)) => s,
            other => panic!("{}: {other:?}", c.id),
        };
        assert!(Position::parse(pos, label_tag).is_ok(), "{pos}");
    }
    assert!(Position::parse("", label_tag).is_err());
    assert!(Position::parse("+=x", label_tag).is_err());
    assert!(Position::parse("<<", label_tag).is_err());
}

#[test]
fn add_label_does_not_change_recent() {
    let mut b = Built::new(true);
    b.call("tl.to(o,{duration:1}) // A");
    b.call("tl.addLabel(\"L\", 5)");
    assert_eq!(b.e.tl(b.tl).last(), b.id("A"));
}

#[test]
fn equal_starts_keep_insertion_order() {
    let mut b = Built::new(true);
    b.call("tl.to(o,{duration:3},0) // A");
    b.call("tl.to(o,{duration:1},0) // B");
    b.call("tl.to(o,{duration:2},0) // C");
    // "<" then ">" chain off the last added, not the latest start.
    b.call("tl.to(o,{duration:1},\">\") // D");
    assert_eq!(b.e.anim_ref(b.id("D")).start_time(), 2.0);
}

#[test]
fn root_level_start_is_root_time_plus_delay() {
    let mut e = TweenEngine::new();
    e.seed(tg(0), X, TweenValue::F64(0.0));
    e.to(one(0), &[to(X, 1.0)], lin(10.0));
    e.advance(0.25);
    let t = e.to(one(1), &[to(X, 1.0)], lin(1.0).delay(0.5));
    assert_eq!(e.anim_ref(t).start_time(), 0.75);
}
