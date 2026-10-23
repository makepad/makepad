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
    /// a `tier <fn> o1` pin
    Pin(u32),
    /// a field watch (`writes`)
    Watch(u32),
}

impl Owner {
    pub fn label(&self) -> String {
        match self {
            Owner::Group(g) => format!("G{}", g),
            Owner::Probe(p) => format!("P{}", p),
            Owner::Pin(n) => format!("pin{}", n),
            Owner::Watch(w) => format!("W{}", w),
        }
    }
}

#[derive(Clone)]
pub enum Edit {
    Count { site: u64, time: bool },
    /// at the entry, or before the first instruction on (file, line)
    Break { site: u64, line: Option<(u32, u32)> },
    Restart { site: u64 },
    Probe { idx: u64, probe_slot: u32, line: Option<(u32, u32)> },
    /// the slot's code held an inlined copy of an instrumented fn: it is built again from its
    /// baseline RIR (no inlining of program fns), so it calls the instrumented code instead
    Uninline,
    /// `tier <fn> o1`: O1 code generation of the slot's O1 RIR; no O2 / O3 install replaces it
    /// and no new build inlines it while the pin lasts
    Pin,
    /// a by-name probe: its call is lowered into the body (lower.rs ProbePoint), then guarded
    ProbeLocals { idx: u64, point: std::sync::Arc<crate::lower::ProbePoint> },
    /// a field watch: the body is lowered again with the watch hit after each write of the field
    Watch { field: std::sync::Arc<crate::lower::WatchField> },
}

struct SlotState {
    /// the generation of the slot's first instrumented install (its code before it is the plain one)
    first_gen: u32,
    edits: Vec<(Owner, Edit)>,
    /// the code the first instrumented install replaced (its start; 0: the slot had none)
    before: u64,
}

static SLOTS: Mutex<Option<HashMap<u32, SlotState>>> = Mutex::new(None);
/// how many slots SLOTS holds (every install asks `replaced`: one load when nothing is instrumented)
static INSTRUMENTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The instrumented slots (sorted; pinned ones flagged): what installs and inlining ask, a lock of
/// its own (SLOTS is held across rebuilds, whose compiles ask too).
static ACTIVE: Mutex<Vec<(u32, bool)>> = Mutex::new(Vec::new());

fn count_slots(m: &HashMap<u32, SlotState>) {
    let _k = crate::zone_alloc::kept();
    let mut v: Vec<(u32, bool)> = m.iter().map(|(k, s)| (*k, s.edits.iter().any(|e| matches!(e.1, Edit::Pin)))).collect();
    v.sort_unstable();
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = v;
    INSTRUMENTED.store(m.len(), std::sync::atomic::Ordering::Release);
}

fn active_entry(id: u32) -> Option<bool> {
    if INSTRUMENTED.load(std::sync::atomic::Ordering::Acquire) == 0 {
        return None;
    }
    let a = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    a.binary_search_by_key(&id, |x| x.0).ok().map(|k| a[k].1)
}

pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    if let Ok(v) = NOTES.try_lock() {
        a.field("inspect.patch_notes");
        a.vec(&v);
    }
    if let Ok(v) = ACTIVE.try_lock() {
        a.field("inspect.patch_active");
        a.vec(&v);
    }
    if let Ok(w) = WATCH.try_lock() {
        a.field("inspect.patch_watch");
        a.vec(&w);
        for x in w.iter() {
            a.vec(&x.lines);
            a.vec(&x.targets);
        }
    }
    let Ok(g) = SLOTS.try_lock() else { return };
    let Some(m) = g.as_ref() else { return };
    a.field("inspect.patch_slots");
    a.map(m, |a, _, s| {
        a.vec(&s.edits);
        for (_, e) in &s.edits {
            if let Edit::Watch { field } = e {
                a.arc(field);
            }
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
    active_entry(id).is_some()
}

/// Inlining (jit::callee_attrs): an instrumented or pinned slot is never inlined into new code
/// (the copy would run unprobed, or at another tier than its pin).
pub fn no_inline(id: u32) -> bool {
    active(id)
}

/// Is slot `id` pinned (`tier <fn> o1`)?
pub fn pinned(id: u32) -> bool {
    active_entry(id) == Some(true)
}

/// The slots `owner` has edits on: (slot, role, the code its first instrumented install replaced).
/// Roles: "target" (the probed / counted / pinned fn), "inliner" (rebuilt so the target's code
/// runs instead of an inlined copy), "pinned inliner".
pub fn owner_slots(owner: Owner) -> Vec<(u32, &'static str, u64)> {
    let g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(m) = g.as_ref() else { return Vec::new() };
    let mut v: Vec<(u32, &'static str, u64)> = m
        .iter()
        .filter_map(|(id, s)| {
            let mine: Vec<&Edit> = s.edits.iter().filter(|e| e.0 == owner).map(|e| &e.1).collect();
            if mine.is_empty() {
                return None;
            }
            let role = if mine.iter().all(|e| matches!(e, Edit::Uninline)) {
                "inliner"
            } else if matches!(owner, Owner::Pin(_)) && WATCH.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|w| w.owner == owner && !w.targets.contains(id)) {
                "pinned inliner"
            } else {
                "target"
            };
            Some((*id, role, s.before))
        })
        .collect();
    v.sort();
    v
}

/// Code a later install could bring in with an inlined copy of an owner's target (the disk cache's
/// O3 code of a caller, a build that started before the probe): watched by `code_installed`.
struct Watch {
    owner: Owner,
    lines: Vec<super::codever::Lines>,
    /// the owner's target slots (their own code holds the lines: not an inlined copy)
    targets: Vec<u32>,
    /// what an inliner gets: Uninline (probes, counters, breakpoints) or Pin
    pin: bool,
}

static WATCH: Mutex<Vec<Watch>> = Mutex::new(Vec::new());
static WATCHING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Slots that got code with an inlined copy of a watched target: (slot, owner, pin), waiting for the owner.
static INLINED_NEW: Mutex<Vec<(u32, Owner, bool)>> = Mutex::new(Vec::new());

/// Starts watching installs for inlined copies of `lines` (the owner's targets `targets`).
pub(crate) fn watch(owner: Owner, lines: Vec<super::codever::Lines>, targets: Vec<u32>, pin: bool) {
    if lines.is_empty() {
        return;
    }
    let _k = crate::zone_alloc::kept();
    let lines = lines.as_slice().to_vec();
    let targets = targets.as_slice().to_vec();
    let mut w = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    w.push(Watch { owner, lines, targets, pin });
    WATCHING.store(w.len(), std::sync::atomic::Ordering::Release);
}

fn unwatch(owner: Owner) {
    let mut w = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    w.retain(|x| x.owner != owner);
    WATCHING.store(w.len(), std::sync::atomic::Ordering::Release);
}

/// Every install (Unit::install, O3, the startup image): new code of slot `id` that holds an
/// inlined copy of a watched target is rebuilt on the owner (an event says so). Cheap when
/// nothing is watched.
pub fn code_installed(u: &crate::jit::Unit, id: u32) {
    let watching = WATCHING.load(std::sync::atomic::Ordering::Acquire) != 0;
    if !watching && !super::watch::waiting() || INSTRUMENTING.try_with(|c| c.get()).unwrap_or(false) {
        return;
    }
    // a field watch waiting for a writer that had no code yet
    super::watch::code_installed(u, id);
    if !watching {
        return;
    }
    let hits: Vec<(Owner, bool)> = {
        let w = WATCH.lock().unwrap_or_else(|e| e.into_inner());
        w.iter()
            .filter(|x| !super::codever::own_slot(u, id, &x.targets) && super::codever::map_holds(u, &u.fns[id as usize].pc_map, &x.lines))
            .map(|x| (x.owner, x.pin))
            .collect()
    };
    if hits.is_empty() {
        return;
    }
    let _k = crate::zone_alloc::kept();
    let first = {
        let mut r = INLINED_NEW.lock().unwrap_or_else(|e| e.into_inner());
        let first = r.is_empty();
        for (o, pin) in hits {
            if !r.iter().any(|x| x.0 == id && x.1 == o) {
                r.push((id, o, pin));
            }
        }
        first
    };
    if first {
        crate::owner::post(crate::owner::Class::Program, || {
            let up = crate::jit::current_unit();
            if !up.is_null() {
                unsafe { cover_new_inliners(up) };
            }
        });
    }
}

unsafe fn cover_new_inliners(up: *mut crate::jit::Unit) {
    let _kept = crate::zone_alloc::kept();
    let list: Vec<(u32, Owner, bool)> = std::mem::take(&mut *INLINED_NEW.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, owner, pin) in list {
        let before = super::codever::installed_json(&*up, id);
        let r = add(up, id, owner, if pin { Edit::Pin } else { Edit::Uninline });
        let now = super::codever::installed_json(&*up, id);
        let (ok, why) = match &r {
            Ok(true) => (true, if pin { "it got code with an inlined copy of a pinned fn: built again at O1 (pinned with it)".to_string() } else { "it got code with an inlined copy of a probed fn: built again without inlining, so it calls the probed code".to_string() }),
            Ok(false) => (false, "the rebuild found no place".to_string()),
            Err(e) => (false, format!("not rebuilt ({}): this copy runs unprobed", e)),
        };
        super::events::emit("probe", &format!("inliner {} {}", owner.label(), id), &format!("\"owner\":\"{}\",\"slot\":{},\"rebuilt\":{},\"why\":{},\"replaced\":{},\"now\":{}", owner.label(), id, ok, super::json_str(&why), before, now));
    }
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
        crate::owner::post(crate::owner::Class::Program, || {
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
        let Some(edits) = m.get(&from).map(|s| s.edits.iter().filter(|e| from == id || !matches!(e.1, Edit::ProbeLocals { .. } | Edit::Watch { .. })).cloned().collect::<Vec<_>>()) else { continue };
        if edits.is_empty() {
            continue;
        }
        let under = { let u = &*up; let f = &u.fns[id as usize]; (f.compiled && f.code_len > 0).then(|| super::codever::kind_at(u, f.code)) };
        match rebuild(up, id, &edits, under) {
            Ok((gen, _)) => {
                let first_gen = if from == id { gen } else { m.get(&id).map_or(gen, |s| s.first_gen) };
                let before = m.get(&id).map_or(0, |s| s.before);
                m.insert(id, SlotState { first_gen, edits, before });
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
        // placed by the relowering (lower.rs watch_write); `rebuild` says whether it hit
        Edit::Watch { .. } => true,
        Edit::Uninline | Edit::Pin => true,
    }
}

/// What a rebuild with `edits` makes: instrumented code, an inliner's baseline code, or a pin's O1.
fn build_kind(edits: &[(Owner, Edit)]) -> crate::jit::CodeKind {
    use crate::jit::{origin, tier, CodeKind};
    let t = if edits.iter().any(|e| !matches!(e.1, Edit::Uninline | Edit::Pin)) {
        tier::INSTRUMENTED
    } else if edits.iter().any(|e| matches!(e.1, Edit::Uninline)) {
        tier::BASELINE
    } else {
        tier::O1
    };
    CodeKind::new(t, origin::INSPECT)
}

/// Rebuilds slot `id` with `edits` (restart landings last: they turn the calls the other edits
/// added into invokes too) and installs it; the result per edit.
unsafe fn rebuild(up: *mut crate::jit::Unit, id: u32, edits: &[(Owner, Edit)], under: Option<crate::jit::CodeKind>) -> Result<(u32, Vec<bool>), String> {
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
    let watches: Vec<crate::lower::WatchField> = edits.iter().filter_map(|e| match &e.1 {
        Edit::Watch { field } => Some((**field).clone()),
        _ => None,
    }).collect();
    let uninline = edits.iter().any(|e| matches!(e.1, Edit::Uninline));
    // the tier of the code under inspection: an inspect build keeps it (an O3 fn gets an O3
    // instrumented build) unless the agent asked for the lower tier (`tier=o1`)
    let o3_under = under.is_some_and(|k| k.tier == crate::jit::tier::O3 || k.tier == crate::jit::tier::GUARDED || k.tier == crate::jit::tier::INSTRUMENTED_O3);
    let baseline_under = under.is_some_and(|k| k.tier == crate::jit::tier::BASELINE);
    let low = LOW_TIER.try_with(|c| c.get()).unwrap_or(false);
    // an inliner at O1 / O3 keeps its tier: its RIR is built again without the instrumented fn
    // inlined; baseline code (and `tier=o1`) is built again from baseline RIR
    let mut uninline_base = uninline && (baseline_under || low || under.is_none());
    if uninline && !uninline_base && crate::jit::rebuild_rir_for_inspect(up, id).is_err() {
        uninline_base = true;
    }
    let mut placed: Vec<(u32, Result<(), String>)> = Vec::new();
    let mut watched: Vec<(u32, u64)> = Vec::new();
    let base = if points.is_empty() && watches.is_empty() && !uninline_base {
        None
    } else {
        let (f, r, w) = crate::jit::relower_with_probes(up, id, points, watches)?;
        placed = r;
        watched = w;
        Some(f)
    };
    if let Some((_, Err(e))) = placed.iter().find(|r| r.1.is_err()) {
        return Err(e.clone());
    }
    let mut kind = build_kind(edits);
    if uninline && !uninline_base {
        kind.tier = crate::jit::tier::O1;
    }
    crate::jit::install_instrumented(up, id, gen, base, kind, &mut |f, u| {
        for &k in &order {
            hits[k] = apply_edit(f, u, id, edits[k].1.clone());
        }
    })?;
    // a watch hit when its field is written in this body
    for k in 0..edits.len() {
        if let Edit::Watch { field } = &edits[k].1 {
            hits[k] = watched.iter().any(|w| w.0 == id);
            super::watch::note_sites(field.idx, id, &watched);
        }
    }
    // the same tier as the code it replaced: an O3 build with the edits in its RIR before the O3
    // pipeline (by-name probes and restart landings are built at O1 only: their RIR is relowered /
    // reshaped for O1)
    let note = if o3_under && !low {
        let o3able = edits.iter().all(|e| matches!(e.1, Edit::Count { .. } | Edit::Break { .. } | Edit::Probe { .. } | Edit::Uninline | Edit::Pin));
        let pinned = edits.iter().any(|e| matches!(e.1, Edit::Pin));
        if pinned {
            None
        } else if !o3able {
            Some("a by-name probe or restart landing is built at O1 only: the O3 code it replaced does not run while it is installed")
        } else {
            let plain = edits.iter().all(|e| matches!(e.1, Edit::Uninline));
            let mut ed = |f: &mut relax_backend::rir::Func, u: &crate::jit::Unit| {
                for &k in &order {
                    let _ = apply_edit(f, u, id, edits[k].1.clone());
                }
            };
            let ok = crate::jit::compile_tier1_inspect(up, id, if plain { None } else { Some(&mut ed) });
            if ok {
                None
            } else {
                Some("its O3 instrumented build was dropped (not verified, outdated, or it failed): it runs the O1 instrumented build, and the O3 code it replaced does not run while it is installed")
            }
        }
    } else if o3_under && low {
        Some("tier=o1 asked: built at O1; the O3 code it replaced does not run while it is installed")
    } else {
        None
    };
    // (static text: the note outlives the compiler generation this build ran in)
    let _k = crate::zone_alloc::kept();
    let mut n = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    n.retain(|x| x.0 != id);
    if let Some(t) = note {
        n.push((id, t));
    }
    Ok((gen, hits))
}

/// Why a slot's inspect build is not at the tier of the code it replaced (patched_row says it).
static NOTES: Mutex<Vec<(u32, &'static str)>> = Mutex::new(Vec::new());

pub(crate) fn tier_note(id: u32) -> Option<String> {
    NOTES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| x.0 == id).map(|x| x.1.to_string())
}

thread_local! {
    /// `count` / `break` / `probe add` with `tier=o1`: inspect builds at O1 even under O3 code
    static LOW_TIER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with inspect builds at O1 (`tier=o1`) when `low`.
pub(crate) fn with_low_tier<R>(low: bool, f: impl FnOnce() -> R) -> R {
    let prev = LOW_TIER.try_with(|c| c.replace(low)).unwrap_or(false);
    let r = f();
    let _ = LOW_TIER.try_with(|c| c.set(prev));
    r
}

/// Adds an edit to slot `id` and reinstalls it; whether the edit found its place.
pub unsafe fn add(up: *mut crate::jit::Unit, id: u32, owner: Owner, edit: Edit) -> Result<bool, String> {
    let _kept = crate::zone_alloc::kept();
    let mut g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let mut edits = m.get(&id).map(|s| s.edits.clone()).unwrap_or_default();
    // (one Uninline / Pin per owner is enough)
    if matches!(edit, Edit::Uninline | Edit::Pin) && edits.iter().any(|e| e.0 == owner && std::mem::discriminant(&e.1) == std::mem::discriminant(&edit)) {
        return Ok(true);
    }
    let before = m.get(&id).map_or_else(|| { let f = &(&*up).fns[id as usize]; if f.compiled { f.code } else { 0 } }, |s| s.before);
    edits.push((owner, edit));
    let under = (before != 0).then(|| super::codever::kind_at(&*up, before));
    let (gen, hits) = rebuild(up, id, &edits, under)?;
    if !hits.last().copied().unwrap_or(false) {
        // the new edit found no place: put the slot back as it was
        edits.pop();
        restore_or_rebuild(up, id, m, edits, gen);
        return Ok(false);
    }
    let first = m.get(&id).map_or(gen, |s| s.first_gen);
    m.insert(id, SlotState { first_gen: first, edits, before });
    count_slots(m);
    Ok(true)
}

/// After a change of slot `id`'s edits (the install of generation `gen` is the newest): with
/// edits left, rebuild; with none, the plain code from before the first instrumented install.
unsafe fn restore_or_rebuild(up: *mut crate::jit::Unit, id: u32, m: &mut HashMap<u32, SlotState>, edits: Vec<(Owner, Edit)>, gen: u32) {
    let first = m.get(&id).map_or(gen, |s| s.first_gen);
    let before = m.get(&id).map_or(0, |s| s.before);
    if edits.is_empty() {
        (&mut *up).restore_before(id, first);
        m.remove(&id);
    } else {
        let under = (before != 0).then(|| super::codever::kind_at(&*up, before));
        let _ = rebuild(up, id, &edits, under);
        m.insert(id, SlotState { first_gen: first, edits, before });
    }
    count_slots(m);
}

/// Removes every edit of `owner`; the slots it touched are rebuilt or restored.
pub unsafe fn remove_owner(up: *mut crate::jit::Unit, owner: Owner) -> usize {
    let _kept = crate::zone_alloc::kept();
    unwatch(owner);
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
