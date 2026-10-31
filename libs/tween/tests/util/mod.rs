//! Shared helpers for the engine tests: property keys, builders, an event
//! recorder that names events like the GSAP golden traces
//! (`"<who>:<callback>"`), and a tiny interpreter for the golden op strings.
//!
//! Every test binary that declares `mod util;` uses a different subset.
#![allow(dead_code)]

use makepad_tween::*;

pub const X: PropKey = PropKey(1);
pub const Y: PropKey = PropKey(2);
pub const A: PropKey = PropKey(3);
pub const B: PropKey = PropKey(4);

/// Target `i`.
pub fn tg(i: u32) -> TargetId {
    TargetId(i)
}

/// One target.
pub fn one(i: u32) -> Targets<'static> {
    Targets::One(TargetId(i))
}

/// `key: v` (to).
pub fn to(key: PropKey, v: f64) -> PropTo<'static> {
    PropTo::to_f64(key, v)
}

/// Linear tween options of `d` seconds.
pub fn lin(d: f64) -> TweenOpts {
    TweenOpts::new().duration(d).ease(Easing::Linear)
}

/// The five GSAP callbacks a golden trace watches.
pub const CBS: EventMask = EventMask(1 | 2 | 4 | 8 | 16 | 32);

/// Seeds `key` of targets `0..n` with `v`.
pub fn seed_all(e: &mut TweenEngine, n: u32, key: PropKey, v: f64) {
    for i in 0..n {
        e.seed(tg(i), key, TweenValue::F64(v));
    }
}

/// Reads a number (0 when the slot does not exist).
pub fn val(e: &TweenEngine, t: u32, key: PropKey) -> f64 {
    e.get_f64(tg(t), key).unwrap_or(0.0)
}

/// Asserts |a - b| <= tol.
#[track_caller]
pub fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol || (a.is_nan() && b.is_nan()),
        "{what}: got {a:?}, want {b:?} (tol {tol:e})"
    );
}

/// GSAP writes plain numbers rounded to 1e-6; the engine keeps full
/// precision, so golden value columns compare at this tolerance.
pub const VAL_TOL: f64 = 5e-7 + 1e-9;
/// Times and positions.
pub const TIME_TOL: f64 = 1e-9;

/// Maps handles and tags to the golden trace names.
#[derive(Default)]
pub struct Names {
    pub ids: Vec<(TweenId, &'static str)>,
    pub tags: Vec<(Tag, &'static str)>,
    buf: Vec<TweenEvent>,
}

impl Names {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn name(&mut self, id: TweenId, who: &'static str) {
        self.ids.push((id, who));
    }

    pub fn tag(&mut self, tag: Tag, who: &'static str) {
        self.tags.push((tag, who));
    }

    fn who(&self, e: &TweenEvent) -> &'static str {
        if let Some((_, w)) = self.ids.iter().find(|(i, _)| *i == e.id) {
            return w;
        }
        if let Some((_, w)) = self.tags.iter().find(|(t, _)| *t == e.tag) {
            return w;
        }
        "?"
    }

    /// The event as `"<who>:<callback>"`.
    pub fn fmt(&self, e: &TweenEvent) -> String {
        let who = self.who(e);
        match e.kind {
            EventKind::Start => format!("{who}:onStart"),
            EventKind::Update => format!("{who}:onUpdate"),
            EventKind::Repeat => format!("{who}:onRepeat"),
            EventKind::Complete => format!("{who}:onComplete"),
            EventKind::ReverseComplete => format!("{who}:onReverseComplete"),
            EventKind::Interrupt => format!("{who}:onInterrupt"),
            EventKind::Call { .. } => format!("{who}:mark"),
            EventKind::Pause => format!("{who}:cb"),
            EventKind::Label(_) => format!("{who}:label"),
        }
    }

    /// Drains the engine's events: (all names, names without onUpdate).
    pub fn drain(&mut self, e: &mut TweenEngine) -> (Vec<String>, Vec<String>) {
        let mut buf = std::mem::take(&mut self.buf);
        e.swap_events(&mut buf);
        let all: Vec<String> = buf.iter().map(|ev| self.fmt(ev)).collect();
        let fired = all
            .iter()
            .filter(|s| !s.ends_with(":onUpdate"))
            .cloned()
            .collect();
        self.buf = buf;
        (all, fired)
    }

    /// Drains the raw events.
    pub fn raw(&mut self, e: &mut TweenEngine) -> Vec<TweenEvent> {
        let mut buf = std::mem::take(&mut self.buf);
        e.swap_events(&mut buf);
        let v = buf.clone();
        self.buf = buf;
        v
    }
}

/// Parses a golden op such as `"seek(0.5, false)"` into (name, numeric
/// args, boolean arg).
pub fn parse_op(op: &str) -> (&str, Vec<f64>, Option<bool>) {
    let op = op.split(" [").next().unwrap().trim();
    let open = op.find('(').unwrap_or(op.len());
    let name = &op[..open];
    let inner = op[open..].trim_start_matches('(').trim_end_matches(')');
    let mut nums = Vec::new();
    let mut flag = None;
    for p in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match p {
            "true" => flag = Some(true),
            "false" => flag = Some(false),
            _ => nums.push(
                p.parse::<f64>()
                    .unwrap_or_else(|_| panic!("op arg {p:?} in {op:?}")),
            ),
        }
    }
    (name, nums, flag)
}

/// Applies a GSAP control op to `id` with GSAP's default event suppression
/// (seek and restart suppress; totalTime, time, progress fire).
pub fn apply_op(e: &mut TweenEngine, id: TweenId, op: &str) {
    let (name, nums, flag) = parse_op(op);
    let ev = |suppress_default: bool| {
        let suppress = flag.unwrap_or(suppress_default);
        if suppress {
            Emit::Suppress
        } else {
            Emit::Fire
        }
    };
    match name {
        "seek" => {
            e.anim(id).seek(Seek::Time(nums[0]), ev(true));
        }
        "totalTime" => {
            e.anim(id).set_total_time(nums[0], ev(false));
        }
        "time" => {
            e.anim(id).set_time(nums[0], ev(false));
        }
        "progress" => {
            e.anim(id).set_progress(nums[0], ev(false));
        }
        "totalProgress" => {
            e.anim(id).set_total_progress(nums[0], ev(false));
        }
        "iteration" => {
            e.anim(id).set_iteration(nums[0] as u32, ev(false));
        }
        "reverse" => {
            e.anim(id).reverse();
        }
        "play" => {
            e.anim(id).play();
        }
        "pause" => {
            e.anim(id).pause();
        }
        "resume" => {
            e.anim(id).resume();
        }
        "restart" => {
            let include_delay = !nums.is_empty() && nums[0] != 0.0;
            e.anim(id).restart(include_delay, ev(true));
        }
        other => panic!("unsupported op {other:?}"),
    }
}
