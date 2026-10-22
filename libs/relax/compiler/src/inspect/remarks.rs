//! Optimisation remarks: what the O3 tier decided about a fn and why, recorded at every O3 build
//! (cheap: the inliner's own refusals, kept as fixed-size records) and queryable afterwards
//! (`remarks <fn>`, and `asm` names the reason next to each call it kept). Keyed by slot; a slot's
//! next O3 build replaces its remarks.

use super::{json_str, short, Out, Req};
use relax_codegen::opt::SiteRemark;
use std::collections::HashMap;

static INLINE: relax_session::sync::Mutex<Option<HashMap<u32, Vec<SiteRemark>>>> = relax_session::sync::Mutex::new(None);

static SPILLS: relax_session::sync::Mutex<Option<HashMap<u32, Vec<relax_backend::remark::SpillRemark>>>> = relax_session::sync::Mutex::new(None);

/// The values slot `id`'s last O3 build spilled (the register allocator's choices).
pub fn record_spills(id: u32, v: Vec<relax_backend::remark::SpillRemark>) {
    let _k = crate::zone_alloc::kept();
    let mut v = v;
    v.shrink_to_fit();
    SPILLS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, v);
}

/// The spilled values of slot `id`'s last O3 build.
pub fn spills_of(id: u32) -> Vec<relax_backend::remark::SpillRemark> {
    SPILLS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).cloned()).unwrap_or_default()
}

/// Usable registers per class on this target (int, float) for the spill remarks' wording.
pub fn usable_regs(flt: bool) -> u32 {
    match (relax_backend::asm::Arch::host(), flt) {
        (relax_backend::asm::Arch::X64, false) => 14,
        (relax_backend::asm::Arch::X64, true) => 16,
        (relax_backend::asm::Arch::Arm64, false) => 27,
        (relax_backend::asm::Arch::Arm64, true) => 32,
    }
}

static LOOPS: relax_session::sync::Mutex<Option<HashMap<u32, Vec<relax_backend::remark::LoopRemark>>>> = relax_session::sync::Mutex::new(None);

/// The loops slot `id`'s last O3 build's vectorisers looked at, and how far each got.
pub fn record_loops(id: u32, v: Vec<relax_backend::remark::LoopRemark>) {
    let _k = crate::zone_alloc::kept();
    let mut v = v;
    v.shrink_to_fit();
    LOOPS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, v);
}

/// The vectoriser remarks of slot `id`'s last O3 build.
pub fn loops_of(id: u32) -> Vec<relax_backend::remark::LoopRemark> {
    LOOPS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).cloned()).unwrap_or_default()
}

static COUNTS: relax_session::sync::Mutex<Option<HashMap<u32, Vec<(u64, u64)>>>> = relax_session::sync::Mutex::new(None);

/// The PGO counts slot `id`'s O3 code was built from: (source position, executions of its block
/// in the counted window), one entry per position and block, sorted by position.
pub fn record_counts(id: u32, f: &relax_backend::rir::Func) {
    let _k = crate::zone_alloc::kept();
    let mut v: Vec<(u64, u64)> = Vec::new();
    for b in f.blocks.iter().filter(|b| b.count >= 0.0) {
        for &p in b.pos.iter().chain(std::iter::once(&b.term_pos)) {
            if p != 0 {
                v.push((p, b.count as u64));
            }
        }
    }
    v.sort_unstable();
    v.dedup_by(|a, b| a.0 == b.0 && {
        b.1 = b.1.max(a.1);
        true
    });
    v.shrink_to_fit();
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, v);
}

/// The PGO counts of slot `id`'s last O3 build by position (empty: none).
pub fn counts_of(id: u32) -> Vec<(u64, u64)> {
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).cloned()).unwrap_or_default()
}

/// The O3 build of slot `id` kept these call sites as calls (its last inlining round).
pub fn record_inline(id: u32, v: Vec<SiteRemark>) {
    let _k = crate::zone_alloc::kept();
    let mut g = INLINE.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    let mut v = v;
    v.shrink_to_fit();
    m.insert(id, v);
}

/// The kept call sites of slot `id`'s last O3 build.
pub fn inline_of(id: u32) -> Vec<SiteRemark> {
    INLINE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id).cloned()).unwrap_or_default()
}

pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    if let Ok(g) = INLINE.try_lock() {
        if let Some(m) = g.as_ref() {
            a.field("inspect.remarks");
            for v in m.values() {
                a.vec(v);
            }
        }
    }
    if let Ok(g) = LOOPS.try_lock() {
        if let Some(m) = g.as_ref() {
            a.field("inspect.loops");
            for v in m.values() {
                a.vec(v);
            }
        }
    }
    if let Ok(g) = SPILLS.try_lock() {
        if let Some(m) = g.as_ref() {
            a.field("inspect.spills");
            for v in m.values() {
                a.vec(v);
            }
        }
    }
    if let Ok(g) = COUNTS.try_lock() {
        if let Some(m) = g.as_ref() {
            a.field("inspect.counts");
            for v in m.values() {
                a.vec(v);
            }
        }
    }
}

/// `remarks <fn|@slot> [n=20]`: the O3 decisions of the fn's slots: the calls the inliner kept
/// (hot sites first) with the refusal and its numbers, the tier history of its code.
pub fn cmd_remarks(req: &Req, out: &mut Out) {
    let Some(q) = req.pos.first().cloned() else {
        return out.err("usage: remarks <fn|@slot> [n=20]");
    };
    let n = req.num("n", 20) as usize;
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
            return Err(format!("no fn matches `{}`{}; try: asm top", q, super::instrument::closest(u, &q)));
        }
        let mut rows = Vec::new();
        for id in ids {
            let f = &u.fns[id as usize];
            let mut v = inline_of(id);
            let tiers: Vec<String> = super::codever::bodies(u, id).iter().map(|b| format!("{} ({})", super::codever::label(super::codever::kind_at(u, b.start)), b.state)).collect();
            let built = INLINE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains_key(&id));
            if v.is_empty() && spills_of(id).is_empty() && loops_of(id).is_empty() {
                rows.push(format!(
                    "`{}` (@{}): {}; code: {}",
                    short(&f.name, 100),
                    id,
                    if built { "its last O3 build kept no call as a call and spilled no value" } else { "no O3 build recorded in this run (O3 code from the disk cache or the startup image carries no remarks: `tier <fn> o3 fresh=1` builds it again)" },
                    if tiers.is_empty() { "none".into() } else { tiers.join(", ") }
                ));
                continue;
            }
            v.sort_by_key(|r| (!r.hot, r.pos));
            let hot = v.iter().filter(|r| r.hot).count();
            rows.push(format!("`{}` (@{}): its last O3 build kept {} call(s) ({} in hot blocks); code: {}", short(&super::display_name(u, id, 0), 100), id, v.len(), hot, tiers.join(", ")));
            if !v.is_empty() {
                rows.push(String::new());
                rows.push("| line | callee | hot | why it stayed a call |".into());
                rows.push("|---|---|---|---|".into());
            }
            for r in v.iter().take(n) {
                let at = super::codever::pos_line(u, r.pos).map(|(fi, l)| format!("{}:{}", u.prog.file_paths.get(fi as usize).map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("?"), l)).unwrap_or_else(|| "?".into());
                let callee = if (r.callee as usize) < u.fns.len() { super::display_name(u, r.callee, 0) } else { format!("fn#{}", r.callee) };
                rows.push(format!("| {} | `{}` | {} | {} |", at, short(&callee.replace('|', "/"), 80), if r.hot { "yes" } else { "" }, r.describe()));
            }
            if v.len() > n {
                rows.push(format!("+{} more (n=)", v.len() - n));
            }
            let lp = loops_of(id);
            if !lp.is_empty() {
                rows.push(String::new());
                rows.push(format!("vectorisers: {} loop look(s), {} vectorised", lp.len(), lp.iter().filter(|l| l.done).count()));
                rows.push("| loop at | remark |".into());
                rows.push("|---|---|".into());
                for l in lp.iter().take(n) {
                    let at = super::codever::pos_line(u, l.pos).map(|(fi, ln)| format!("{}:{}", u.prog.file_paths.get(fi as usize).map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("?"), ln)).unwrap_or_else(|| "?".into());
                    rows.push(format!("| {} | {} |", at, l.describe()));
                }
            }
            let mut sp = spills_of(id);
            if !sp.is_empty() {
                // the most used first: their stack traffic costs most
                sp.sort_by_key(|r| (std::cmp::Reverse(r.uses), r.pos));
                rows.push(String::new());
                rows.push(format!("register allocation: {} value(s) spilled to stack slots ({} across calls, {} under pressure)", sp.len(), sp.iter().filter(|r| r.calls > 0).count(), sp.iter().filter(|r| r.pressure + 1 >= usable_regs(r.flt)).count()));
                rows.push("| defined at | spilled value |".into());
                rows.push("|---|---|".into());
                for r in sp.iter().take(n) {
                    let at = super::codever::pos_line(u, r.pos).map(|(fi, l)| format!("{}:{}", u.prog.file_paths.get(fi as usize).map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("?"), l)).unwrap_or_else(|| "(entry)".into());
                    rows.push(format!("| {} | {} |", at, r.describe(usable_regs(r.flt))));
                }
                if sp.len() > n {
                    rows.push(format!("+{} more spilled values", sp.len() - n));
                }
            }
        }
        Ok(rows)
    });
    match r.unwrap_or_else(|| Err(format!("the owner did not answer in 10 s ({})", crate::owner::serving_note()))) {
        Ok(rows) => {
            for l in rows {
                out.line(&l);
            }
            out.always("more: asm <fn> vs=rustc  versions <fn>  tier <fn> o3 fresh=1");
        }
        Err(e) => out.err(&e),
    }
    let _ = json_str;
}
