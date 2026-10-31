//! The tier-up decision log: what the JIT decided about each slot's way to O3, so `why-tier <fn>`
//! can say why a hot fn is not at O3 (its decayed cost stays under the threshold, a dropped build with
//! no retier budget left, a queue entry counted again, a refused build, ...).
//!
//! Decision points (jit.rs: count stubs, the O3 queue, counted builds, O3 builds) record an event:
//! program threads (rt_hot, rt_loop_hot: rule I-CALL, atomics only) push into a lock-free ring;
//! the O3 workers and `why-tier` fold the ring into per-slot histories (counts per kind, the
//! cost checks per code version, the last events). Only slots with a decision beyond being armed get
//! a history; a count stub being armed (every O1 install) only updates an existing one.

use super::flightrec::Slots;
use relax_session::sync::Mutex;
use std::collections::HashMap;

/// Event kinds (`Ev::ev`).
pub mod ev {
    /// a count stub went into the slot: `why` = ARM_* , `b` = the code it counts
    pub const ARM: u8 = 1;
    /// a cost check stayed under the threshold (the budget ran out, the decayed cost is short of
    /// it): `a` = the decayed cost, `b` = the code counted
    pub const CHECK: u8 = 2;
    /// queued for O3: `why` = the queue reason (Q_*), `a` = rate (calls/s), `b` = the code
    pub const QUEUE: u8 = 3;
    /// an O3 queue entry that waited past REVALIDATE_NS went back to counting: `a` = its age
    pub const REVALIDATE: u8 = 4;
    /// ... but other code was installed meanwhile: it stays queued (built)
    pub const KEPT: u8 = 5;
    /// a worker took it from the queue: `why` = queue reason, `a` = ns it waited
    pub const POP: u8 = 6;
    /// its counted (PGO) build was installed: `why` = 1 lukewarm
    pub const COUNTED: u8 = 7;
    /// its counted window ended: `why` = WIN_*, `a` = entries per second
    pub const WINDOW_END: u8 = 8;
    /// a lukewarm counted slot showed too little work: parked in counted code
    pub const PARKED: u8 = 9;
    /// O3 code installed: `a` = build ns (cpu)
    pub const O3_INSTALLED: u8 = 10;
    /// an O3 build was dropped: `why` = DROP_*
    pub const O3_DROPPED: u8 = 11;
    /// an O3 build was not even started: `why` = REFUSE_*
    pub const O3_REFUSED: u8 = 12;
    /// queued to be built again (retier): `a` = retiers used
    pub const RETIER: u8 = 13;
    /// a retier was refused: the budget (RETIERS) is used up
    pub const RETIER_SPENT: u8 = 14;
    /// its O2 build was queued: `why` = 0 slow baseline, 1 hot (with its O3 build)
    pub const O2_QUEUED: u8 = 15;
    /// an O3 build panicked (dropped for it)
    pub const O3_PANIC: u8 = 16;
    pub const N: usize = 17;
    pub const NAMES: [&str; N] = [
        "?", "armed", "cost check", "queued", "counted again", "kept queued", "taken", "counted build", "counted window ended", "parked",
        "O3 installed", "O3 dropped", "O3 refused", "retier", "retier refused", "O2 queued", "O3 panicked",
    ];
}

pub const ARM_NEW: u8 = 0;
pub const ARM_REARM: u8 = 1;
pub const ARM_REVALIDATE: u8 = 2;
pub const ARM_SANDBOX: u8 = 3;
pub const ARM_IMAGE: u8 = 4;
pub const ARM_WHY: [&str; 5] = ["new code", "new code under the open window", "counted again (queued too long)", "sandbox twin", "startup image"];

pub const WIN_DONE: u8 = 0;
pub const WIN_IDLE: u8 = 1;
pub const WIN_WORK: u8 = 2;
pub const WIN_CREDITED: u8 = 3;
pub const WIN_WHY: [&str; 4] = ["entries/blocks/time reached", "went idle (built with what it has)", "lukewarm code showed work", "a hot loop ran under it"];

pub const DROP_PANIC: u8 = 0;
pub const DROP_CODEGEN: u8 = 1;
pub const DROP_PATCH: u8 = 2;
pub const DROP_SLOT_CHANGED: u8 = 3;
pub const DROP_LINK: u8 = 4;
pub const DROP_WHY: [&str; 5] = ["build panicked", "codegen error", "a patch changed meanwhile", "the slot changed meanwhile (other code installed, e.g. its O2 build)", "link check failed"];

pub const REFUSE_VERDICT: u8 = 0;
pub const REFUSE_TAKE: u8 = 1;
pub const REFUSE_PROFILE_GONE: u8 = 2;
pub const REFUSE_WHY: [&str; 3] = ["not allowed (borrow-check verdict pending, or a patch without a clean verdict)", "its snapshot bailed (a front could not be made off the owner)", "its counted profile is gone (patched or rebuilt)"];

pub const Q_WHY: [&str; 5] = ["calls", "loop", "retier", "marked", "profiled"];

/// One decision.
#[derive(Clone, Copy, Default)]
pub struct Ev {
    /// wall ns since the process start
    pub t: u64,
    pub slot: u32,
    pub ev: u8,
    pub why: u8,
    pub a: u64,
    pub b: u64,
}

const RING_LEN: usize = 1 << 15;
static RING: std::sync::OnceLock<Slots<Ev>> = std::sync::OnceLock::new();

/// The ring exists (called where allocating is fine: arming counters, starting the O3 workers).
pub fn init() {
    if RING.get().is_none() {
        crate::zone_alloc::kept_with(|| {
            let _ = RING.get_or_init(|| Slots::new(RING_LEN));
        });
    }
}

/// Records a decision (any thread; atomics only: program threads call it from rt_hot).
#[inline]
pub fn note(slot: u32, ev: u8, why: u8, a: u64, b: u64) {
    if let Some(r) = RING.get() {
        r.push(Ev { t: crate::clock::wall_ns(), slot, ev, why, a, b });
    }
}

/// A count stub went in (unit lock held, never a program thread's hot path): only a slot that
/// already has a history records it (every O1 install arms its slot: through the ring that flooded
/// it at startup).
pub fn note_arm(slot: u32, why: u8, code: u64) {
    let Ok(mut g) = STORE.try_lock() else { return };
    let Some(st) = g.as_mut() else { return };
    if let Some(h) = st.by.get_mut(&slot) {
        let _k = crate::zone_alloc::kept();
        fold(h, Ev { t: crate::clock::wall_ns(), slot, ev: ev::ARM, why, a: 0, b: code });
    }
}

/// Recent events kept per slot.
const RECENT: usize = 8;
/// At most this many slots get a history (the rest are counted).
const MAX_SLOTS: usize = 1 << 15;

#[derive(Clone, Default)]
pub struct Hist {
    /// events by kind
    pub n: [u32; ev::N],
    /// first and last time (ms of the run) per kind
    pub first_ms: [u32; ev::N],
    pub last_ms: [u32; ev::N],
    /// cost checks per code version: (code, queued, checks under the threshold)
    pub windows: Vec<(u64, u32, u32)>,
    /// the last events, oldest first
    pub recent: Vec<Ev>,
}

struct Store {
    cursor: u64,
    lost: u64,
    untracked: u64,
    by: HashMap<u32, Hist>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Folds the ring into the per-slot histories (O3 workers between builds; `why-tier`).
pub fn drain() {
    let Some(r) = RING.get() else { return };
    let Ok(mut g) = STORE.try_lock() else { return };
    let _k = crate::zone_alloc::kept();
    let st = g.get_or_insert_with(|| Store { cursor: 0, lost: 0, untracked: 0, by: HashMap::new() });
    let mut evs: Vec<Ev> = Vec::new();
    let (c, lost) = r.read(st.cursor, &mut |_, e| evs.push(e));
    st.cursor = c;
    st.lost += lost;
    for e in evs {
        let tracked = st.by.contains_key(&e.slot);
        if !tracked {
            // being armed alone is no decision (every O1 install arms its slot)
            if e.ev == ev::ARM {
                continue;
            }
            if st.by.len() >= MAX_SLOTS {
                st.untracked += 1;
                continue;
            }
        }
        let h = st.by.entry(e.slot).or_default();
        fold(h, e);
    }
}

fn fold(h: &mut Hist, e: Ev) {
    let ms = (e.t / 1_000_000).min(u32::MAX as u64) as u32;
    let k = (e.ev as usize).min(ev::N - 1);
    if h.n[k] == 0 {
        h.first_ms[k] = ms;
    }
    h.n[k] = h.n[k].saturating_add(1);
    h.last_ms[k] = ms;
    if e.ev == ev::CHECK || (e.ev == ev::QUEUE && e.why == 0) {
        let w = match h.windows.iter_mut().position(|w| w.0 == e.b) {
            Some(i) => &mut h.windows[i],
            None => {
                if h.windows.len() >= 4 {
                    h.windows.remove(0);
                }
                h.windows.push((e.b, 0, 0));
                h.windows.last_mut().unwrap()
            }
        };
        if e.ev == ev::CHECK {
            w.2 += 1;
        } else {
            w.1 += 1;
        }
    }
    if h.recent.len() >= RECENT {
        h.recent.remove(0);
    }
    h.recent.push(e);
}

/// Slot `id`'s history (after a drain), and the log's own losses: (ring entries lost, slots not
/// tracked).
pub fn history(id: u32) -> (Option<Hist>, u64, u64) {
    drain();
    let g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some(st) => (st.by.get(&id).cloned(), st.lost, st.untracked),
        None => (None, 0, 0),
    }
}

/// Every tracked slot's counts of kind `k` (why-tier over many slots; `top` lists).
pub fn count_of(id: u32, k: u8) -> u32 {
    let g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|st| st.by.get(&id)).map_or(0, |h| h.n[k as usize])
}

/// The words the inspect answer prints for one event (`code` names a code address: tier origin).
pub fn describe(e: &Ev, code: &dyn Fn(u64) -> String) -> String {
    let ms = |ns: u64| format!("{:.1} ms", ns as f64 / 1e6);
    match e.ev {
        ev::ARM => format!("count stub armed on {} ({})", code(e.b), ARM_WHY.get(e.why as usize).unwrap_or(&"?")),
        ev::CHECK => format!("cost check on {}: decayed cost {} under the threshold", code(e.b), e.a),
        ev::QUEUE => format!("queued for O3 by {}{}{}", Q_WHY.get(e.why as usize).unwrap_or(&"?"), if e.a > 0 { format!(" at {} cost/s", e.a) } else { String::new() }, if e.b != 0 { format!(" on {}", code(e.b)) } else { String::new() }),
        ev::REVALIDATE => format!("O3 queue entry {} old: counted again", ms(e.a)),
        ev::KEPT => "O3 queue entry stays queued (other code was installed meanwhile)".into(),
        ev::POP => format!("taken by an O3 worker ({}) after {} in the queue", Q_WHY.get(e.why as usize).unwrap_or(&"?"), ms(e.a)),
        ev::COUNTED => format!("counted (PGO) build installed{}", if e.why == 1 { " (lukewarm: O3 once its counts show work)" } else { "" }),
        ev::WINDOW_END => format!("counted window ended: {} ({} entries/s)", WIN_WHY.get(e.why as usize).unwrap_or(&"?"), e.a),
        ev::PARKED => "parked in counted code: too little work for O3".into(),
        ev::O3_INSTALLED => format!("O3 code installed ({} build)", ms(e.a)),
        ev::O3_DROPPED => format!("O3 build dropped: {}", DROP_WHY.get(e.why as usize).unwrap_or(&"?")),
        ev::O3_REFUSED => format!("O3 build not started: {}", REFUSE_WHY.get(e.why as usize).unwrap_or(&"?")),
        ev::RETIER => format!("queued to be built again (retier {} of 4)", e.a),
        ev::RETIER_SPENT => "retier refused: its 4 retiers are used up".into(),
        ev::O2_QUEUED => format!("O2 build queued ({})", if e.why == 0 { "baseline code ran slowly but long" } else { "with its O3 build" }),
        ev::O3_PANIC => "O3 build panicked: O3 dropped for it".into(),
        _ => "?".into(),
    }
}

/// `why-tier uncounted`: the slots whose count stub is out of the slot while they still count
/// (other code was installed over it): their calls are not counted.
fn cmd_uncounted(req: &super::Req, out: &mut super::Out) {
    let n = req.num("n", 20) as usize;
    let up = crate::jit::current_unit();
    if up.is_null() {
        return out.err("no running unit");
    }
    let r = crate::owner::call_within(crate::owner::Class::Job, 10_000, move || {
        let u = unsafe { &*up };
        let v = crate::jit::tier_uncounted(u);
        let mut rows = vec![format!("{} slots are not counted (their count stub replaced while they count): they never queue O3", v.len())];
        if !v.is_empty() {
            rows.push("| slot | fn | code |".into());
            rows.push("|---|---|---|".into());
            for id in v.iter().take(n) {
                let f = &u.fns[*id as usize];
                rows.push(format!("| S{} | `{}` | {} |", id, super::short(&super::display_name(u, *id, 0), 80), super::codever::label(super::codever::kind_at(u, f.code))));
            }
            if v.len() > n {
                rows.push(format!("+{} more (n=)", v.len() - n));
            }
        }
        rows
    });
    match r {
        Some(rows) => {
            for l in rows {
                out.line(&l);
            }
            out.always("more: why-tier @<slot>");
        }
        None => out.err("the owner did not answer in 10 s"),
    }
}


/// ms of the run, for tables.
pub fn at_s(t_ns: u64) -> String {
    format!("{:.2}", t_ns as f64 / 1e9)
}

/// `why-tier <fn|@slot>`: why each slot of the fn is (not) at O3 -- the verdict first, then its
/// tier-up state now and its decision history (counts by kind, the last events).
pub fn cmd_why_tier(req: &super::Req, out: &mut super::Out) {
    let Some(q) = req.pos.first().cloned() else {
        return out.err("usage: why-tier <fn|file:line|@slot> | why-tier uncounted [n=20]");
    };
    if q == "uncounted" {
        return cmd_uncounted(req, out);
    }
    let q2 = q.clone();
    let up = crate::jit::current_unit();
    if up.is_null() {
        return out.err("no running unit");
    }
    let r = crate::owner::call_within(crate::owner::Class::Job, 10_000, move || -> Result<Vec<String>, String> {
        let u = unsafe { &*up };
        let ids: Vec<u32> = match q.strip_prefix('@').and_then(|s| s.parse::<u32>().ok()) {
            Some(id) if (id as usize) < u.fns.len() => vec![id],
            _ => super::instrument::find_all(u, &q, 8).into_iter().map(|x| x.0).collect(),
        };
        if ids.is_empty() {
            return Err(format!("no fn matches `{}`{}", q, super::instrument::closest(u, &q)));
        }
        let kind = |a: u64| super::codever::label(super::codever::kind_at(u, a));
        let mut rows = Vec::new();
        let (_, lost, untracked) = history(u32::MAX);
        for id in ids {
            let (verdict, state) = crate::jit::tier_explain(u, id);
            rows.push(format!("`{}` (S{}): {}", super::short(&super::display_name(u, id, 0), 100), id, verdict));
            rows.push("| what | now |".into());
            rows.push("|---|---|".into());
            for (w, v) in state {
                rows.push(format!("| {} | {} |", w, v.replace('|', "/")));
            }
            match history(id).0 {
                Some(h) => {
                    let counts: Vec<String> = (1..ev::N).filter(|&k| h.n[k] > 0).map(|k| format!("{} {}", ev::NAMES[k], h.n[k])).collect();
                    rows.push(format!("decisions: {}", counts.join(", ")));
                    rows.push("| at s | decision |".into());
                    rows.push("|---|---|".into());
                    for e in &h.recent {
                        rows.push(format!("| {} | {} |", at_s(e.t), describe(e, &kind)));
                    }
                }
                None => rows.push("decisions: none recorded beyond being armed (no cost check, queue, build or drop)".into()),
            }
            rows.push(String::new());
        }
        if lost > 0 {
            rows.push(format!("(the tier log's ring overflowed: {} events lost between drains)", lost));
        }
        if untracked > 0 {
            rows.push(format!("(the tier log tracks at most {} slots: {} events of other slots not kept)", MAX_SLOTS, untracked));
        }
        Ok(rows)
    });
    match r.unwrap_or_else(|| Err(format!("the owner did not answer in 10 s ({})", crate::owner::serving_note()))) {
        Ok(rows) => {
            for l in rows {
                out.line(&l);
            }
            out.always(&format!("more: versions {q2}  remarks {q2}  tier {q2} o3  count {q2}"));
        }
        Err(e) => out.err(&e),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Armed alone makes no history; a cost check does, and arming then counts; checks are kept
    /// per code version.
    #[test]
    fn histories_start_at_a_decision() {
        init();
        let slot = 0x7fff_fff0;
        note_arm(slot, ARM_NEW, 0x1000);
        drain();
        assert!(history(slot).0.is_none());
        note(slot, ev::CHECK, 0, 15_000, 0x1000);
        note(slot, ev::CHECK, 0, 27_000, 0x1000);
        drain();
        note_arm(slot, ARM_REARM, 0x2000);
        note(slot, ev::QUEUE, 0, 9000, 0x2000);
        let h = history(slot).0.unwrap();
        assert_eq!(h.n[ev::CHECK as usize], 2);
        assert_eq!(h.n[ev::ARM as usize], 1);
        assert_eq!(h.windows, vec![(0x1000, 0, 2), (0x2000, 1, 0)]);
        assert_eq!(h.recent.len(), 4);
    }
}
