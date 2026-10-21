//! The instrumented builds of slots (DESIGN.md §10): every instrument on a slot (counters,
//! breakpoints, restart landings, probes) is an edit of its plain RIR; the slot's code is rebuilt
//! from the plain RIR with all its current edits and installed under a new patch generation, so
//! instruments compose. When the last edit goes, the code from before the first instrumented
//! install comes back (the installs' saved previous versions), with no recompile.
//! All of it runs on the owner thread.
//!
//! Zones: SLOTS and the edits are kept state (the program heap): the instrumented code calls the
//! sites they name long after a settle swap destroyed the compiler arena the owner compiles in.
//! The entries take `zone_alloc::kept()`; only `rebuild`'s compile runs in the compiler phase.

use std::collections::HashMap;
use relax_session::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    /// a `count` / `break` / `restartable` group
    Group(u32),
    /// an injected probe
    Probe(u32),
}

#[derive(Clone)]
pub enum Edit {
    Count { site: u64, time: bool },
    Break { site: u64, line: Option<u32> },
    Restart { site: u64 },
    Probe { idx: u64, probe_slot: u32, line: Option<u32> },
    /// a by-name probe: its call is lowered into the body (lower.rs ProbePoint), then guarded
    ProbeLocals { idx: u64, point: std::sync::Arc<crate::lower::ProbePoint> },
}

struct SlotState {
    /// the generation of the slot's first instrumented install (its code before it is the plain one)
    first_gen: u32,
    edits: Vec<(Owner, Edit)>,
}

static SLOTS: Mutex<Option<HashMap<u32, SlotState>>> = Mutex::new(None);
/// how many slots SLOTS holds (every install asks `replaced`: one load when nothing is instrumented)
static INSTRUMENTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_slots(m: &HashMap<u32, SlotState>) {
    INSTRUMENTED.store(m.len(), std::sync::atomic::Ordering::Release);
}

pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    let Ok(g) = SLOTS.try_lock() else { return };
    let Some(m) = g.as_ref() else { return };
    a.field("inspect.patch_slots");
    a.map(m, |a, _, s| {
        a.vec(&s.edits);
        for (_, e) in &s.edits {
            if let Edit::ProbeLocals { point, .. } = e {
                a.arc(point);
                a.vec(&point.params);
                for p in &point.params {
                    a.string(&p.0);
                }
            }
        }
    });
}

/// Is slot `id` instrumented now? (O3 / O2 installs keep its instruments)
pub fn active(id: u32) -> bool {
    SLOTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains_key(&id))
}

thread_local! {
    /// this thread is installing an instrumented build (rebuild): not a replacement
    static INSTRUMENTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Slots whose instrumented code was replaced (or that got code while their twin is
/// instrumented): (slot, the slot whose edits it takes), waiting for the owner.
static REPLACED: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// Unit::install (and rollback): slot `id` gets code that is not an instrumented build. When it
/// has instruments, they go into the new code again (posted to the owner: the installer may hold
/// the unit). Cheap when nothing is instrumented.
pub fn replaced(id: u32, sibling: Option<u32>) {
    if INSTRUMENTED.load(std::sync::atomic::Ordering::Acquire) == 0 || INSTRUMENTING.try_with(|c| c.get()).unwrap_or(false) {
        return;
    }
    let from = if active(id) {
        id
    } else {
        match sibling.filter(|&s| active(s)) {
            Some(s) => s,
            None => return,
        }
    };
    let _k = crate::zone_alloc::kept();
    let first = {
        let mut r = REPLACED.lock().unwrap_or_else(|e| e.into_inner());
        let first = r.is_empty();
        if !r.iter().any(|x| x.0 == id) {
            r.push((id, from));
        }
        first
    };
    if first {
        crate::owner::post(crate::owner::Class::App, || {
            let up = crate::jit::current_unit();
            if !up.is_null() {
                unsafe { reinstrument_replaced(up) };
            }
        });
    }
}

/// The owner: every replaced instrumented slot is rebuilt from its current RIR (the new body)
/// with its edits; `rm` then restores the code that replaced the old instrumented build.
unsafe fn reinstrument_replaced(up: *mut crate::jit::Unit) {
    let _kept = crate::zone_alloc::kept();
    let ids: Vec<(u32, u32)> = std::mem::take(&mut *REPLACED.lock().unwrap_or_else(|e| e.into_inner()));
    let mut g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(m) = g.as_mut() else { return };
    for (id, from) in ids {
        // a twin takes its sibling's edits, except by-name probes (their points name a slot)
        let Some(edits) = m.get(&from).map(|s| s.edits.iter().filter(|e| from == id || !matches!(e.1, Edit::ProbeLocals { .. })).cloned().collect::<Vec<_>>()) else { continue };
        if edits.is_empty() {
            continue;
        }
        match rebuild(up, id, &edits) {
            Ok((gen, _)) => {
                let first_gen = if from == id { gen } else { m.get(&id).map_or(gen, |s| s.first_gen) };
                m.insert(id, SlotState { first_gen, edits });
                count_slots(m);
                let why = if from == id { "its code was replaced (live patch, verdict rebuild or rollback): the counters and probes are in the new code".to_string() } else { format!("it runs the same fn as instrumented slot {} (its SPEC-V twin or base) and got code: it counts into the same sites", from) };
                super::events::emit("probe", &format!("reinstrumented {} {}", id, gen), &format!("\"slot\":{},\"reinstrumented\":true,\"why\":{}", id, super::json_str(&why)));
            }
            Err(e) => {
                super::events::emit("probe", &format!("reinstrument failed {}", id), &format!("\"slot\":{},\"reinstrumented\":false,\"error\":{}", id, super::json_str(&e)));
            }
        }
    }
}

/// Applies one edit to `f`; true when it found its place (a line that is in the code).
fn apply_edit(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, slot: u32, e: Edit) -> bool {
    match e {
        Edit::Count { site, time } => super::instrument::edit_count(f, u, site, time),
        Edit::Break { site, line } => super::instrument::edit_break(f, u, slot, site, line),
        Edit::Restart { site } => super::instrument::edit_restart_site(f, u, site),
        Edit::Probe { idx, probe_slot, line } => super::probes::edit(f, u, idx, probe_slot, line),
        Edit::ProbeLocals { idx, point } => super::probes::wrap_calls(f, u, idx, point.probe_slot),
    }
}

/// Rebuilds slot `id` with `edits` (restart landings last: they turn the calls the other edits
/// added into invokes too) and installs it; the result per edit.
unsafe fn rebuild(up: *mut crate::jit::Unit, id: u32, edits: &[(Owner, Edit)]) -> Result<(u32, Vec<bool>), String> {
    // the build is compile work (the compiler arena, like any compile on the owner); the edits'
    // records it reads stay kept (SLOTS, the sites)
    let _compile = crate::zone_alloc::compiler();
    struct Flag;
    impl Drop for Flag {
        fn drop(&mut self) {
            let _ = INSTRUMENTING.try_with(|c| c.set(false));
        }
    }
    let _ = INSTRUMENTING.try_with(|c| c.set(true));
    let _flag = Flag;
    let u = &mut *up;
    let gen = u.begin_patch();
    let mut hits = vec![false; edits.len()];
    let mut order: Vec<usize> = (0..edits.len()).filter(|&k| !matches!(edits[k].1, Edit::Restart { .. })).collect();
    order.extend((0..edits.len()).filter(|&k| matches!(edits[k].1, Edit::Restart { .. })));
    // by-name probes: the body is lowered again with their calls in it
    let points: Vec<crate::lower::ProbePoint> = edits.iter().filter_map(|e| match &e.1 {
        Edit::ProbeLocals { point, .. } => Some((**point).clone()),
        _ => None,
    }).collect();
    let mut placed: Vec<(u32, Result<(), String>)> = Vec::new();
    let base = if points.is_empty() {
        None
    } else {
        let (f, r) = crate::jit::relower_with_probes(up, id, points)?;
        placed = r;
        Some(f)
    };
    if let Some((_, Err(e))) = placed.iter().find(|r| r.1.is_err()) {
        return Err(e.clone());
    }
    crate::jit::install_instrumented(up, id, gen, base, &mut |f, u| {
        for &k in &order {
            hits[k] = apply_edit(f, u, id, edits[k].1.clone());
        }
    })?;
    Ok((gen, hits))
}

/// Adds an edit to slot `id` and reinstalls it; whether the edit found its place.
pub unsafe fn add(up: *mut crate::jit::Unit, id: u32, owner: Owner, edit: Edit) -> Result<bool, String> {
    let _kept = crate::zone_alloc::kept();
    let mut g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let mut edits = m.get(&id).map(|s| s.edits.clone()).unwrap_or_default();
    edits.push((owner, edit));
    let (gen, hits) = rebuild(up, id, &edits)?;
    if !hits.last().copied().unwrap_or(false) {
        // the new edit found no place: put the slot back as it was
        edits.pop();
        restore_or_rebuild(up, id, m, edits, gen);
        return Ok(false);
    }
    let first = m.get(&id).map_or(gen, |s| s.first_gen);
    m.insert(id, SlotState { first_gen: first, edits });
    count_slots(m);
    Ok(true)
}

/// After a change of slot `id`'s edits (the install of generation `gen` is the newest): with
/// edits left, rebuild; with none, the plain code from before the first instrumented install.
unsafe fn restore_or_rebuild(up: *mut crate::jit::Unit, id: u32, m: &mut HashMap<u32, SlotState>, edits: Vec<(Owner, Edit)>, gen: u32) {
    let first = m.get(&id).map_or(gen, |s| s.first_gen);
    if edits.is_empty() {
        (&mut *up).restore_before(id, first);
        m.remove(&id);
    } else {
        let _ = rebuild(up, id, &edits);
        m.insert(id, SlotState { first_gen: first, edits });
    }
    count_slots(m);
}

/// Removes every edit of `owner`; the slots it touched are rebuilt or restored.
pub unsafe fn remove_owner(up: *mut crate::jit::Unit, owner: Owner) -> usize {
    let _kept = crate::zone_alloc::kept();
    let mut g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(m) = g.as_mut() else { return 0 };
    let ids: Vec<u32> = m.iter().filter(|(_, s)| s.edits.iter().any(|e| e.0 == owner)).map(|(k, _)| *k).collect();
    for &id in &ids {
        let mut edits = m.get(&id).map(|s| s.edits.clone()).unwrap_or_default();
        edits.retain(|e| e.0 != owner);
        let gen = (&*up).fns[id as usize].patch_gen;
        restore_or_rebuild(up, id, m, edits, gen);
    }
    ids.len()
}
