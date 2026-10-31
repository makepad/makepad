//! Who keeps the app awake: `MAKEPAD_FRAME_TRACE=1` counts every frame request (next frame,
//! redraw, timer start and fire, `Event::Signal`) by the source line that made it and prints
//! the busiest sources once a second on stderr:
//!
//! ```text
//! [frame-sources] 1.00s beats=61 next_frame=60 redraw=0 timer=30 signal=0
//!     60 next_frame  apps/commercial/stage/src/foo.rs:123
//!     30 timer       apps/commercial/stage/src/app.rs:456 (fired)
//! ```
//!
//! The header also counts what drawing cost: `lists` draw lists re-recorded and `items` draw
//! items they recorded (a list reused from the last frame counts nothing).
//!
//! An idle app prints nothing (a second with no requests has no line). Off, each hook is one
//! cached `bool` load.

use std::{
    cell::RefCell,
    collections::HashMap,
    panic::Location,
    sync::OnceLock,
    time::Instant,
};

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_FRAME_TRACE").is_some_and(|v| v != "0" && !v.is_empty()))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    NextFrame,
    Redraw,
    TimerStart,
    TimerFire,
    Signal,
    /// A thread raised the app signal (`SignalToUI::set_ui_signal`) here.
    SignalRaise,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::NextFrame => "next_frame",
            Kind::Redraw => "redraw",
            Kind::TimerStart => "timer start",
            Kind::TimerFire => "timer fire",
            Kind::Signal => "signal",
            Kind::SignalRaise => "signal by",
        }
    }
}

type Loc = &'static Location<'static>;

struct Trace {
    window_start: Instant,
    beats: u32,
    lists: u32,
    items: u64,
    counts: HashMap<(Kind, Option<Loc>), u32>,
    timers: HashMap<u64, Loc>,
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

fn with(f: impl FnOnce(&mut Trace)) {
    TRACE.with(|t| {
        let mut t = t.borrow_mut();
        let t = t.get_or_insert_with(|| Trace {
            window_start: Instant::now(),
            beats: 0,
            lists: 0,
            items: 0,
            counts: HashMap::new(),
            timers: HashMap::new(),
        });
        f(t);
        flush_if_due(t);
    });
}

/// A frame request from `loc`.
pub fn note(kind: Kind, loc: Loc) {
    if !on() { return; }
    with(|t| *t.counts.entry((kind, Some(loc))).or_default() += 1);
}

/// A timer was started at `loc`; its fires are counted against that line.
pub fn timer_started(timer_id: u64, loc: Loc) {
    if !on() { return; }
    with(|t| {
        t.timers.insert(timer_id, loc);
        *t.counts.entry((Kind::TimerStart, Some(loc))).or_default() += 1;
    });
}

pub fn timer_fired(timer_id: u64) {
    if !on() { return; }
    with(|t| {
        let loc = t.timers.get(&timer_id).copied();
        *t.counts.entry((Kind::TimerFire, loc)).or_default() += 1;
    });
}

pub fn signal() {
    if !on() { return; }
    with(|t| *t.counts.entry((Kind::Signal, None)).or_default() += 1);
}

/// A frame beat ran (the paint clock ticked).
pub fn beat() {
    if !on() { return; }
    with(|t| t.beats += 1);
}

/// A draw list was re-recorded with `items` draw items.
pub fn recorded(items: usize) {
    if !on() { return; }
    with(|t| {
        t.lists += 1;
        t.items += items as u64;
    });
}

fn flush_if_due(t: &mut Trace) {
    let el = t.window_start.elapsed().as_secs_f64();
    if el < 1.0 { return; }
    t.window_start = Instant::now();
    let beats = std::mem::take(&mut t.beats);
    let lists = std::mem::take(&mut t.lists);
    let items = std::mem::take(&mut t.items);
    if t.counts.is_empty() && beats == 0 && lists == 0 { return; }
    let mut per_kind: HashMap<Kind, u32> = HashMap::new();
    for ((k, _), n) in &t.counts { *per_kind.entry(*k).or_default() += n; }
    let g = |k| per_kind.get(&k).copied().unwrap_or(0);
    let mut out = format!(
        "[frame-sources] {el:.2}s beats={beats} next_frame={} redraw={} timer_fire={} timer_start={} signal={} lists={lists} items={items}\n",
        g(Kind::NextFrame), g(Kind::Redraw), g(Kind::TimerFire), g(Kind::TimerStart), g(Kind::Signal)
    );
    for (at, n) in crate::makepad_network::ui_signal::take_signal_sources() {
        *t.counts.entry((Kind::SignalRaise, Some(at))).or_default() += n;
    }
    let mut rows: Vec<_> = t.counts.drain().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.0.cmp(&b.0.0)));
    for ((kind, loc), n) in rows.into_iter().take(12) {
        let at = loc.map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_else(|| "-".into());
        out.push_str(&format!("    {n:5} {:<11} {at}\n", kind.name()));
    }
    eprint!("{out}");
}
