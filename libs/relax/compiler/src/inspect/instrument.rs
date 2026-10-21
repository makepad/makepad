//! Instrumented rebuilds, breakpoints and pause (DESIGN.md §10, review decision 6).
//!
//! - `count <fn>`: every slot whose name matches is built again with a call at its entry (and,
//!   with time=1, before each return), installed as one patch group; `rm` rolls the group back
//!   (the replaced code comes back as it was: no recompile, no code growth). Exact call counts,
//!   inclusive time.
//! - `break <fn>|<file>:<line>`: the call is at the fn's entry or the line's first instruction;
//!   a hit parks the thread (its stack intact) and raises a `break` event; `continue` lets it go.
//! - `pause` / `resume`: the program's threads stand still (macOS: Mach suspend; Linux: running
//!   threads hold in the capture signal's handler -- blocked ones are left alone and held once
//!   they run); stacks keep working.
//!
//! While a slot is instrumented, O3 and O2 installs skip it (they would replace the probes);
//! callers that inlined it before do not see the probes.

use super::{json_str, now_ms, short, Out, Req};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::{Relaxed, SeqCst}};
use relax_session::sync::Mutex;

include!("../rt_abi.rs");

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Count,
    Break,
    /// a panic through the fn can restart it or make it return (DESIGN.md §10b)
    Restart,
}

struct Site {
    kind: Kind,
    slot: u32,
    label: String,
    /// a breakpoint: how its argument words read (the hooked fn's parameters)
    args: std::sync::OnceLock<ArgPlan>,
    /// a restart site: its return values' count when `return` can be offered (all integer-class,
    /// not returned in memory), None when it cannot
    returns: std::sync::OnceLock<Option<usize>>,
    /// the probe group (a site's patch generation; `rm` rolls the generation back)
    group: u32,
    hits: AtomicU64,
    ns: AtomicU64,
    on: AtomicBool,
}

static SITES: Mutex<Vec<std::sync::Arc<Site>>> = Mutex::new(Vec::new());
/// (group id, patch generation, slots, description)
static GROUPS: Mutex<Vec<(u32, u32, Vec<u32>, String)>> = Mutex::new(Vec::new());
static NEXT_GROUP: AtomicU64 = AtomicU64::new(1);

pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    if let Ok(sites) = SITES.try_lock() {
        a.field("inspect.sites");
        a.vec(&sites);
        for s in sites.iter() {
            a.arc(s);
            a.string(&s.label);
            if let Some(p) = s.args.get() {
                a.vec(&p.params);
                for x in &p.params {
                    a.string(&x.0);
                    a.string(&x.1);
                }
            }
        }
    }
    if let Ok(g) = GROUPS.try_lock() {
        a.field("inspect.groups");
        a.vec(&g);
        for x in g.iter() {
            a.vec(&x.2);
            a.string(&x.3);
        }
    }
    if let Ok(p) = PARKED.try_lock() {
        a.field("inspect.parked");
        a.vec(&p);
        for x in p.iter() {
            a.arc(x);
        }
    }
}

/// Are any counters installed (count groups or injected probes)? Cheap: a frame log asks it
/// at each frame's start.
pub fn any_counters() -> bool {
    !GROUPS.lock().unwrap_or_else(|e| e.into_inner()).is_empty() || super::probes::any()
}

/// Every installed counter: ("<site> <fn>", hits, ns) for count sites, then the probes'.
pub fn counters() -> Vec<(String, u64, u64)> {
    let mut out = Vec::new();
    {
        let sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
        for (i, s) in sites.iter().enumerate() {
            if s.on.load(Relaxed) && s.kind == Kind::Count {
                out.push((format!("{}{} {}", site_letter(s.kind), i, s.label), s.hits.load(Relaxed), s.ns.load(Relaxed)));
            }
        }
    }
    super::probes::counters(&mut out);
    out
}

/// `probes` snapshots: (seq, wall ns, counters), the newest last (`probes since=<seq>`).
static SNAPS: Mutex<std::collections::VecDeque<(u64, u64, Vec<(String, u64, u64)>)>> = Mutex::new(std::collections::VecDeque::new());
static SNAP_SEQ: AtomicU64 = AtomicU64::new(0);

/// `probes since=<seq>`: each counter's change since snapshot `seq`, per second, and whether it
/// started or stopped moving compared with the interval before `seq`.
fn probes_since(since: u64, now: &[(String, u64, u64)], t: u64, out: &mut Out) {
    let snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(k) = snaps.iter().position(|x| x.0 == since) else {
        out.err(&format!("no probes snapshot {} (kept: the last 32); `probes` starts a new one", since));
        return;
    };
    let (_, t0, base) = &snaps[k];
    let prev = if k > 0 { Some(&snaps[k - 1]) } else { None };
    let secs = (t.saturating_sub(*t0) as f64 / 1e9).max(1e-3);
    let get = |v: &[(String, u64, u64)], key: &str| v.iter().find(|x| x.0 == key).map(|x| (x.1, x.2));
    for (key, hits, ns) in now {
        let (h0, ns0) = get(base, key).unwrap_or((0, 0));
        let delta = hits.saturating_sub(h0);
        let before = prev.and_then(|p| get(&p.2, key).map(|x| h0.saturating_sub(x.0)));
        let change = match (get(base, key).is_some(), before, delta > 0) {
            (false, _, _) => "new",
            (true, Some(0), true) => "started",
            (true, _, true) => "active",
            (true, Some(b), false) if b > 0 => "stopped",
            _ => "idle",
        };
        let time = if ns.saturating_sub(ns0) > 0 { format!(",\"ms\":{:.3}", ns.saturating_sub(ns0) as f64 / 1e6) } else { String::new() };
        out.line(&format!("{{\"counter\":{},\"delta\":{},\"per_s\":{:.1},\"change\":\"{}\"{}}}", json_str(&short(key, 160)), delta, delta as f64 / secs, change, time));
    }
    out.line(&format!("{{\"interval_s\":{:.2}}}", secs));
}

/// Is slot `id` instrumented now? (jit.rs: O3 / O2 installs keep its probes)
pub fn active(id: u32) -> bool {
    super::patches::active(id)
}

fn site(i: u64) -> Option<std::sync::Arc<Site>> {
    SITES.lock().unwrap_or_else(|e| e.into_inner()).get(i as usize).cloned()
}

thread_local! {
    /// entry times of counted calls in flight on this thread (site, ns)
    static ENTRIES: std::cell::RefCell<Vec<(u64, u64)>> = const { std::cell::RefCell::new(Vec::new()) };
}

rt_fn! {
fn rt_inspect_enter(i: u64) {
    if let Some(s) = site(i) {
        if s.on.load(Relaxed) {
            s.hits.fetch_add(1, Relaxed);
            // a counted fn may run inside a compile (constant evaluation): the thread-local stack
            // is the thread's, never the compiler arena's
            let _k = crate::zone_alloc::kept();
            let _ = ENTRIES.try_with(|e| e.borrow_mut().push((i, crate::clock::wall_ns())));
        }
    }
}
}

rt_fn! {
fn rt_inspect_exit(i: u64) {
    let now = crate::clock::wall_ns();
    let t = ENTRIES.try_with(|e| {
        let mut e = e.borrow_mut();
        // an unwind skipped exits of inner calls: drop them
        while let Some((k, t)) = e.pop() {
            if k == i {
                return Some(t);
            }
        }
        None
    });
    if let (Ok(Some(t)), Some(s)) = (t, site(i)) {
        s.ns.fetch_add(now.saturating_sub(t), Relaxed);
    }
}
}

// ------------------------------------------------------------------ breakpoints

struct Parked {
    id: u64,
    site: u64,
    os_id: u64,
    go: AtomicBool,
}

static PARKED: Mutex<Vec<std::sync::Arc<Parked>>> = Mutex::new(Vec::new());
static GO: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
static PARK_IDS: AtomicU64 = AtomicU64::new(0);

/// How the words a breakpoint receives map to the hooked fn's parameters: per parameter its name,
/// its type as source spells it, and how many integer words it takes (0: passed in a float register,
/// not received). `exact` is false when the words did not line up with the types (then only the
/// raw words are shown).
#[derive(Clone, Default)]
struct ArgPlan {
    params: Vec<(String, String, u8, Shape)>,
    exact: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Signed(u8),
    Unsigned(u8),
    Bool,
    Char,
    Str,
    Slice,
    Pointer,
    Other,
}

/// The parameters of slot `id` (on the owner) and how they map onto `f`'s integer parameter words.
fn arg_plan(u: &crate::jit::Unit, id: u32, f: &relax_backend::rir::Func) -> ArgPlan {
    use crate::types::TyKind;
    let crate::jit::FnKey::Inst(d, gargs) = &u.fns[id as usize].key else { return ArgPlan::default() };
    let Some(sig) = u.tcx.fn_sig(&u.prog, *d) else { return ArgPlan::default() };
    let mut names: Vec<String> = Vec::new();
    {
        let def = u.prog.def(*d);
        let file = &u.prog.files[def.file as usize];
        if let crate::ast::ItemKind::Fn(fsig, _, _) = &file.ast.item(def.item).kind {
            if sig.self_kind != 0 {
                names.push("self".into());
            }
            for p in file.ast.list(file.ast.fn_sig(*fsig).params) {
                names.push(match file.ast.pat(p.pat) {
                    crate::ast::Pat::Ident { name, .. } => file.text(*name).to_string(),
                    _ => "_".into(),
                });
            }
        }
    }
    let mut params = Vec::new();
    let mut words = 0usize;
    for (k, t) in sig.params.iter().enumerate() {
        let t = u.tcx.tys.erase_regions(u.tcx.tys.subst(*t, gargs));
        let text = crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t);
        let (n, shape) = match u.tcx.tys.kind(t) {
            TyKind::Bool => (1, Shape::Bool),
            TyKind::Char => (1, Shape::Char),
            TyKind::Float(_) => (0, Shape::Other),
            TyKind::Int(p) => {
                let s = format!("{:?}", p).to_lowercase();
                let bits: u8 = s.trim_start_matches(|c| c == 'i' || c == 'u').parse().unwrap_or(64);
                (1, if s.starts_with('i') { Shape::Signed(bits) } else { Shape::Unsigned(bits) })
            }
            TyKind::Ref(_, inner, _) | TyKind::Ptr(_, inner) => match u.tcx.tys.kind(*inner) {
                TyKind::Str => (2, Shape::Str),
                TyKind::Slice(_) | TyKind::Dyn(..) => (2, Shape::Slice),
                _ => (1, Shape::Pointer),
            },
            _ => (1, Shape::Other),
        };
        words += n as usize;
        params.push((names.get(k).cloned().unwrap_or_else(|| format!("arg{}", k)), text, n, shape));
    }
    let int_params = f.params.iter().filter(|v| Some(**v) != f.sret && f.vregs[v.0 as usize] == relax_backend::rir::Cls::I).count();
    ArgPlan { params, exact: int_params == words }
}

/// The breakpoint's argument words, decoded by the plan (scalars, pointers, &str read safely).
fn args_json(plan: &ArgPlan, words: &[u64; 5]) -> String {
    if !plan.exact {
        let w: Vec<String> = words.iter().map(|x| format!("\"{:#x}\"", x)).collect();
        let p: Vec<String> = plan.params.iter().map(|x| json_str(&format!("{}: {}", x.0, x.1))).collect();
        return format!(",\"params\":[{}],\"arg_words\":[{}]", p.join(","), w.join(","));
    }
    let mut out = Vec::new();
    let mut k = 0usize;
    for (name, ty, n, shape) in &plan.params {
        let v = |i: usize| words.get(k + i).copied();
        let shown = match (*n, shape) {
            (0, _) => "(float register)".to_string(),
            _ if k + *n as usize > words.len() => "(not received)".to_string(),
            (_, Shape::Bool) => (v(0).unwrap_or(0) & 1 != 0).to_string(),
            (_, Shape::Char) => char::from_u32(v(0).unwrap_or(0) as u32).map(|c| format!("{:?}", c)).unwrap_or_else(|| "?".into()),
            (_, Shape::Signed(b)) => {
                let x = v(0).unwrap_or(0);
                let sh = 64 - (*b as u32).min(64);
                (((x << sh) as i64) >> sh).to_string()
            }
            (_, Shape::Unsigned(b)) => {
                let x = v(0).unwrap_or(0);
                (if *b >= 64 { x } else { x & ((1u64 << b) - 1) }).to_string()
            }
            (_, Shape::Str) => {
                let (p, len) = (v(0).unwrap_or(0), v(1).unwrap_or(0));
                match super::threads::read_mem(p, len.min(200) as usize) {
                    Some(b) => format!("{:?}{}", String::from_utf8_lossy(&b), if len > 200 { format!(" (+{} bytes)", len - 200) } else { String::new() }),
                    None => format!("<str {:#x} len {}>", p, len),
                }
            }
            (_, Shape::Slice) => format!("<{:#x} len {}>", v(0).unwrap_or(0), v(1).unwrap_or(0)),
            (_, Shape::Pointer) | (_, Shape::Other) => format!("{:#x}", v(0).unwrap_or(0)),
        };
        k += *n as usize;
        out.push(format!("{{\"name\":{},\"type\":{},\"value\":{}}}", json_str(name), json_str(&short(ty, 80)), json_str(&shown)));
    }
    format!(",\"args\":[{}]", out.join(","))
}

rt_fn! {
fn rt_inspect_break(i: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) {
    let Some(s) = site(i) else { return };
    if !s.on.load(Relaxed) {
        return;
    }
    s.hits.fetch_add(1, Relaxed);
    let args = s.args.get().map(|plan| args_json(plan, &[a0, a1, a2, a3, a4])).unwrap_or_default();
    park_here(i, &format!("B{}", i), &s.label, &args);
}
}

/// Parks the calling thread as a breakpoint hit (site index `i`, u64::MAX for none) and raises
/// the `break` event; returns once an agent continues it (or after 10 minutes).
pub fn park_here(i: u64, site_tag: &str, label: &str, extra: &str) {
    let _k = crate::zone_alloc::kept();
    let id = PARK_IDS.fetch_add(1, SeqCst) + 1;
    let p = std::sync::Arc::new(Parked { id, site: i, os_id: super::threads::self_os_id(), go: AtomicBool::new(false) });
    let me = crate::os::thread::current();
    let thread = if me == crate::jit::MAIN_TID.load(Relaxed) { "main".to_string() } else { crate::os::thread::name(me) };
    let mut list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    list.push(p.clone());
    super::events::emit(
        "break",
        &format!("K{}", id),
        &format!(
            "\"id\":\"K{}\",\"site\":\"{}\",\"at\":{},\"thread\":{},\"tid\":\"T{}\"{}{},\"more\":[\"stacks thread=T{}\",\"continue K{}\"]",
            id,
            site_tag,
            json_str(label),
            json_str(&thread),
            p.os_id,
            extra,
            super::script_stack_field("script_stack", me),
            p.os_id,
            id
        ),
    );
    let t0 = std::time::Instant::now();
    while !p.go.load(SeqCst) && t0.elapsed().as_secs() < 600 {
        list = GO.wait_timeout(list, std::time::Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
    }
    list.retain(|x| x.id != id);
}

// ------------------------------------------------------------------ finding slots

/// Slots matching a fn query (exact display name, else a suffix of it, else a substring) that
/// have code, at most `max`; or the slots of `file:line` whose RIR has an instruction on that
/// line. Run on the owner.
pub(super) fn find(u: &crate::jit::Unit, query: &str, max: usize) -> Vec<(u32, Option<u32>, String)> {
    if let Some((file, line)) = query.rsplit_once(':').and_then(|(f, l)| l.parse::<u32>().ok().map(|l| (f, l))) {
        let files: Vec<u32> = u.prog.file_paths.iter().enumerate().filter(|(_, p)| p.ends_with(file)).map(|(i, _)| i as u32).collect();
        let mut out = Vec::new();
        for id in 0..u.fns.len() {
            let f = &u.fns[id];
            if !f.compiled || !files.contains(&f.file) {
                continue;
            }
            // the fn whose item spans the line (its RIR, built for the instrumented build when it
            // is gone -- code from the disk cache has none -- places the hook)
            let crate::jit::FnKey::Inst(d, _) = &f.key else { continue };
            let def = u.prog.def(*d);
            let file = &u.prog.files[def.file as usize];
            let item = file.ast.item(def.item);
            let (lo, hi) = (file.line_col(item.lo).0 as u32, file.line_col(item.hi).0 as u32);
            if lo <= line && line <= hi && matches!(item.kind, crate::ast::ItemKind::Fn(..)) {
                out.push((id as u32, Some(line), f.name.to_string()));
            }
            if out.len() >= max {
                break;
            }
        }
        return out;
    }
    let mut exact = Vec::new();
    let mut tail = Vec::new();
    let mut loose = Vec::new();
    for id in 0..u.fns.len() {
        let f = &u.fns[id];
        if !f.compiled || f.name.is_empty() {
            continue;
        }
        match name_match(&f.name, query) {
            Match::Exact => exact.push((id as u32, None, f.name.to_string())),
            Match::Tail if tail.len() < max => tail.push((id as u32, None, f.name.to_string())),
            Match::Loose if loose.len() < max => loose.push((id as u32, None, f.name.to_string())),
            _ => {}
        }
        if exact.len() >= max {
            break;
        }
    }
    for v in [exact, tail, loose] {
        if !v.is_empty() {
            return v;
        }
    }
    Vec::new()
}

#[derive(PartialEq, Debug)]
pub(super) enum Match {
    None,
    /// a substring of the name
    Loose,
    /// the name's last path segments
    Tail,
    Exact,
}

/// `<A as Trait>` in a slot name (lower.rs display_path's trait-impl form) as `A`: a trait impl's
/// method is also found as `Type::method`.
fn collapse_qself(n: &str) -> String {
    let mut out = String::with_capacity(n.len());
    let mut rest = n;
    while let Some(k) = rest.find('<') {
        out.push_str(&rest[..k]);
        let r = &rest[k + 1..];
        // the matching `>` and the top-level ` as ` inside it
        let (mut depth, mut end, mut as_at) = (0i32, None, None);
        for (i, c) in r.char_indices() {
            match c {
                '<' => depth += 1,
                '>' if depth == 0 => {
                    end = Some(i);
                    break;
                }
                '>' => depth -= 1,
                ' ' if depth == 0 && as_at.is_none() && r[i..].starts_with(" as ") => as_at = Some(i),
                _ => {}
            }
        }
        match (end, as_at) {
            (Some(e), Some(a)) => {
                out.push_str(&r[..a]);
                rest = &r[e + 1..];
            }
            _ => {
                out.push('<');
                rest = r;
            }
        }
    }
    out.push_str(rest);
    out
}

/// How a fn query names slot `name`: the whole path (`ttf_parser::Face::parse`), its last
/// segments (`Face::parse`; `crate::Face::parse` is the same), `<Face as Trait>::method` or
/// `Face::method` for a trait impl's method, else any substring.
pub(super) fn name_match(name: &str, query: &str) -> Match {
    let q = query.trim();
    let q = q.strip_prefix("crate::").or_else(|| q.strip_prefix("::")).unwrap_or(q);
    let crate_rel = query.trim().starts_with("crate::");
    let collapsed = if name.contains(" as ") { Some(collapse_qself(name)) } else { None };
    let forms = std::iter::once(name).chain(collapsed.as_deref());
    let mut best = Match::None;
    for n in forms {
        if !crate_rel && n == q {
            return Match::Exact;
        }
        let tail = n.len() > q.len() && n.ends_with(q) && n[..n.len() - q.len()].ends_with("::");
        let tail = tail || (q.starts_with('<') && n.ends_with(q));
        if crate_rel && tail {
            // `crate::X`: X directly under a crate root
            let prefix = &n[..n.len() - q.len()];
            if prefix.strip_suffix("::").is_some_and(|root| !root.is_empty() && !root.contains("::")) {
                return Match::Exact;
            }
        }
        if tail {
            best = Match::Tail;
        } else if best == Match::None && n.contains(q) {
            best = Match::Loose;
        }
    }
    best
}

/// The line of a RIR position ((file << 32) | byte offset).
pub(super) fn pos_line(u: &crate::jit::Unit, p: u64) -> Option<u32> {
    if p == 0 {
        return None;
    }
    let s = u.pos_str((p >> 32) as u32, p as u32);
    let mut parts = s.rsplitn(3, ':');
    let _col = parts.next()?;
    parts.next()?.parse().ok()
}

/// Adds the probe calls to slot `id`'s RIR: `enter` (Some) at the entry or before the first
/// instruction on `line`, `exit` before every return.
fn edit(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, site: u64, enter: (&'static str, u64), exit: Option<u64>, line: Option<u32>, with_args: bool, hit: &mut bool) {
    use relax_backend::rir::{Callee, Inst, Term};
    let sym = |name: &'static str, addr: u64| u.syms.intern(crate::syms::SymKind::Rt(name), addr);
    let enter_sym = sym(enter.0, enter.1);
    // a breakpoint also receives the fn's first five integer parameter words (zeros past them)
    let words: Vec<relax_backend::rir::VReg> = if with_args {
        f.params.iter().copied().filter(|v| Some(*v) != f.sret && f.vregs[v.0 as usize] == relax_backend::rir::Cls::I).take(5).collect()
    } else {
        Vec::new()
    };
    let call = |f: &mut relax_backend::rir::Func, b: usize, at: usize, s: relax_backend::rir::Sym, pos: u64, words: &[relax_backend::rir::VReg]| {
        let v = f.vreg(relax_backend::rir::Cls::I);
        let mut args = vec![v];
        let mut consts = vec![(v, site as i64)];
        if with_args {
            args.extend_from_slice(words);
            while args.len() < 6 {
                let z = f.vreg(relax_backend::rir::Cls::I);
                consts.push((z, 0));
                args.push(z);
            }
        }
        f.blocks[b].insts.insert(at, Inst::Call(Callee::Host(s), args, Vec::new()));
        f.blocks[b].pos.insert(at, pos);
        for (r, c) in consts.into_iter().rev() {
            f.blocks[b].insts.insert(at, Inst::Iconst(r, c));
            f.blocks[b].pos.insert(at, pos);
        }
    };
    match line {
        None => {
            let pos = f.blocks.first().and_then(|b| b.pos.first().copied()).unwrap_or(0);
            call(f, 0, 0, enter_sym, pos, &words);
            *hit = true;
        }
        Some(l) => {
            'found: for b in 0..f.blocks.len() {
                for k in 0..f.blocks[b].insts.len() {
                    let p = f.blocks[b].pos[k];
                    if pos_line(u, p) == Some(l) {
                        call(f, b, k, enter_sym, p, &words);
                        *hit = true;
                        break 'found;
                    }
                }
            }
        }
    }
    if let Some(x) = exit {
        let xs = sym("inspect_exit", x);
        for b in 0..f.blocks.len() {
            if matches!(f.blocks[b].term, Term::Ret(_)) {
                let at = f.blocks[b].insts.len();
                let pos = f.blocks[b].term_pos;
                call(f, b, at, xs, pos, &[]);
            }
        }
    }
}

// ------------------------------------------------------------------ restart / return

/// A panic's resume decision for the innermost restartable frame of a slot (set by the parked
/// panic's thread before it unwinds; consumed by that frame's unwind landing).
#[derive(Clone)]
pub struct Pending {
    pub slot: u32,
    /// 1 restart, 2 return
    pub mode: u64,
    pub words: Vec<u64>,
}

thread_local! {
    static PENDING: std::cell::RefCell<Option<Pending>> = const { std::cell::RefCell::new(None) };
}

pub fn set_pending(p: Pending) {
    let _ = PENDING.try_with(|x| *x.borrow_mut() = Some(p));
}

/// After a watched panic with no restartable frame: the innermost program frame (not std / core /
/// alloc) becomes restartable in the background, so the next panic through it offers restart /
/// return.
pub fn auto_restartable(frames: &[u64]) {
    let up = crate::jit::current_unit();
    if up.is_null() {
        return;
    }
    let u = unsafe { &*up };
    let slot = frames.iter().filter_map(|&pc| u.find_fn(pc)).find(|&id| {
        let n = &u.fns[id as usize].name;
        !(n.starts_with("core::") || n.starts_with("std::") || n.starts_with("alloc::") || n.is_empty())
    });
    let Some(slot) = slot else { return };
    if SITES.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|s| s.kind == Kind::Restart && s.slot == slot && s.on.load(Relaxed)) {
        return;
    }
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-inspect-restartable".into()).spawn(move || {
        super::heap::exclude_this_thread();
        let _ = add_to(Kind::Restart, Target::Slot(slot), false);
    });
    }
}

/// The innermost restartable frame among a panic's frames (lookup pcs): (slot, fn, return words
/// count when return is possible).
pub fn restartable_in(frames: &[u64]) -> Option<(u32, String, Option<usize>)> {
    let up = crate::jit::current_unit();
    if up.is_null() {
        return None;
    }
    let u = unsafe { &*up };
    let sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
    for &pc in frames {
        let Some(slot) = u.find_fn(pc) else { continue };
        if let Some(s) = sites.iter().find(|s| s.kind == Kind::Restart && s.slot == slot && s.on.load(Relaxed)) {
            return Some((slot, s.label.clone(), s.returns.get().copied().flatten()));
        }
    }
    None
}

rt_fn! {
/// The unwind landing of a restartable frame: 0 unwind on, 1 restart the fn, 2 return the pending
/// words. A decision taken here ends the unwinding (the panic is over).
fn rt_inspect_unwind_decide(i: u64) -> u64 {
    let Some(s) = site(i) else { return 0 };
    let take = PENDING.try_with(|p| {
        let mut p = p.borrow_mut();
        match p.as_ref() {
            Some(x) if x.slot == s.slot => p.take(),
            _ => None,
        }
    });
    match take {
        Ok(Some(x)) => {
            s.hits.fetch_add(1, Relaxed);
            if x.mode == 2 {
                let _ = RETURN_WORDS.try_with(|w| *w.borrow_mut() = x.words.clone());
            }
            crate::jit::abandon_unwind();
            x.mode
        }
        _ => 0,
    }
}
}

thread_local! {
    static RETURN_WORDS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}

rt_fn! {
/// The k-th word of an agent-supplied return value (0 past the given ones).
fn rt_inspect_return_word(k: u64) -> u64 {
    RETURN_WORDS.try_with(|w| w.borrow().get(k as usize).copied().unwrap_or(0)).unwrap_or(0)
}
}

/// Rebuilds `f` so a panic through it lands in a decision block: unwind on, restart from the
/// entry with the original arguments, or return agent-supplied words. Every call reachable on the
/// normal path becomes an invoke whose unwind edge goes there (existing cleanup pads end there
/// instead of resuming); the arguments are kept in copies made at entry. Returns the return-word
/// count when `return` can be offered.
fn edit_restart(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, site: u64) -> Option<usize> {
    use relax_backend::rir::{Callee, Cls, Cond, Inst, Term};
    let nb = f.blocks.len();
    // blocks on the normal path (entry, jumps, branches, invoke normal edges)
    let mut normal = vec![false; nb];
    let mut work = vec![0usize];
    while let Some(b) = work.pop() {
        if b >= nb || normal[b] {
            continue;
        }
        normal[b] = true;
        match &f.blocks[b].term {
            Term::Jump(t) => work.push(*t as usize),
            Term::Branch(_, t, e) => {
                work.push(*t as usize);
                work.push(*e as usize);
            }
            Term::Invoke(t, _) => work.push(*t as usize),
            _ => {}
        }
    }
    let pad = f.block();
    // existing cleanup pads end in the decision instead of resuming
    for b in 0..nb {
        if !normal[b] && matches!(f.blocks[b].term, Term::Resume) {
            f.blocks[b].term = Term::Jump(pad);
        }
    }
    // every Relax-ABI call on the normal path that can unwind gets the decision as its unwind edge
    let (land, resume) = (u.rs.land, u.rs.resume);
    for b in 0..nb {
        if !normal[b] {
            continue;
        }
        let mut k = 0;
        let mut cur = b;
        loop {
            let n = f.blocks[cur].insts.len();
            let Some(at) = (k..n).find(|&i| matches!(&f.blocks[cur].insts[i], Inst::Call(Callee::Fn(_) | Callee::Indirect(_), _, _))
                || matches!(&f.blocks[cur].insts[i], Inst::Call(Callee::Host(h), _, _) if *h != land && *h != resume)) else { break };
            let last = at + 1 == n;
            if last && matches!(f.blocks[cur].term, Term::Invoke(..)) {
                break;
            }
            // split after the call: the rest moves to a new block, the call becomes an invoke
            let rest_insts = f.blocks[cur].insts.split_off(at + 1);
            let rest_pos = f.blocks[cur].pos.split_off(at + 1);
            let next = f.block();
            let term = std::mem::replace(&mut f.blocks[cur].term, Term::Invoke(next, pad));
            let term_pos = f.blocks[cur].term_pos;
            f.blocks[next as usize].insts = rest_insts;
            f.blocks[next as usize].pos = rest_pos;
            f.blocks[next as usize].term = term;
            f.blocks[next as usize].term_pos = term_pos;
            cur = next as usize;
            k = 0;
        }
    }
    // entry: argument copies, then the original entry (now a block of its own: block 0 keeps no
    // predecessors)
    let body = f.block();
    let entry_insts = std::mem::take(&mut f.blocks[0].insts);
    let entry_pos = std::mem::take(&mut f.blocks[0].pos);
    let entry_term = std::mem::replace(&mut f.blocks[0].term, Term::Jump(body));
    f.blocks[body as usize].insts = entry_insts;
    f.blocks[body as usize].pos = entry_pos;
    f.blocks[body as usize].term = entry_term;
    f.blocks[body as usize].term_pos = f.blocks[0].term_pos;
    // blocks that jumped to 0 (none should) now reach the body
    let params = f.params.clone();
    let mut saved = Vec::new();
    for p in &params {
        let c = f.vregs[p.0 as usize];
        let v = f.vreg(c);
        saved.push((v, *p));
        f.blocks[0].insts.push(Inst::Mov(v, *p));
        f.blocks[0].pos.push(0);
    }
    // the decision
    let sym = |name: &'static str, addr: u64| u.syms.intern(crate::syms::SymKind::Rt(name), addr);
    let decide = sym("inspect_unwind_decide", rt_inspect_unwind_decide as *const () as u64);
    let sv = f.vreg(Cls::I);
    let d = f.vreg(Cls::I);
    let c1 = f.vreg(Cls::I);
    let c2 = f.vreg(Cls::I);
    let restart = f.block();
    let second = f.block();
    let resume_blk = f.block();
    {
        let b = &mut f.blocks[pad as usize];
        b.insts.push(Inst::Iconst(sv, site as i64));
        b.insts.push(Inst::Call(Callee::Host(decide), vec![sv], vec![d]));
        b.insts.push(Inst::ICmpI(Cond::Eq, false, c1, d, 1));
        b.pos.extend([0, 0, 0]);
        b.term = Term::Branch(c1, restart, second);
    }
    for (v, p) in &saved {
        f.blocks[restart as usize].insts.push(Inst::Mov(*p, *v));
        f.blocks[restart as usize].pos.push(0);
    }
    f.blocks[restart as usize].term = Term::Jump(body);
    f.blocks[resume_blk as usize].term = Term::Resume;
    let returnable = f.sret.is_none() && f.cabi.is_none() && f.rets.iter().all(|c| *c == Cls::I);
    if returnable {
        let ret = f.block();
        f.blocks[second as usize].insts.push(Inst::ICmpI(Cond::Eq, false, c2, d, 2));
        f.blocks[second as usize].pos.push(0);
        f.blocks[second as usize].term = Term::Branch(c2, ret, resume_blk);
        let word = sym("inspect_return_word", rt_inspect_return_word as *const () as u64);
        let mut rv = Vec::new();
        for k in 0..f.rets.len() {
            let kv = f.vreg(Cls::I);
            let r = f.vreg(Cls::I);
            let b = &mut f.blocks[ret as usize];
            b.insts.push(Inst::Iconst(kv, k as i64));
            b.insts.push(Inst::Call(Callee::Host(word), vec![kv], vec![r]));
            b.pos.extend([0, 0]);
            rv.push(r);
        }
        f.blocks[ret as usize].term = Term::Ret(rv);
        Some(f.rets.len())
    } else {
        f.blocks[second as usize].term = Term::Jump(resume_blk);
        None
    }
}

/// `count`'s edit: entry (and with `time`, exit) calls.
pub(super) fn edit_count(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, site: u64, time: bool) -> bool {
    let mut hit = false;
    edit(f, u, site, ("inspect_enter", rt_inspect_enter as *const () as u64), time.then_some(rt_inspect_exit as *const () as u64), None, false, &mut hit);
    hit
}

/// `break`'s edit: the parking call with the fn's argument words at the entry or a line.
pub(super) fn edit_break(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, slot: u32, site: u64, line: Option<u32>) -> bool {
    if let Some(s) = self::site(site) {
        // kept state (the site outlives the compiler arena); this edit runs inside a compile
        let _ = s.args.set(crate::zone_alloc::kept_with(|| arg_plan(u, slot, f)));
    }
    let mut hit = false;
    edit(f, u, site, ("inspect_break", rt_inspect_break as *const () as u64), None, line, true, &mut hit);
    hit
}

/// `restartable`'s edit.
pub(super) fn edit_restart_site(f: &mut relax_backend::rir::Func, u: &crate::jit::Unit, site: u64) -> bool {
    let n = edit_restart(f, u, site);
    if let Some(s) = self::site(site) {
        let _ = s.returns.set(n);
    }
    true
}

/// A new site (its index): kept state whatever the caller's phase (the owner compiles in the
/// compiler arena; the instrumented code reads the site after a settle swap destroyed it).
fn register_site(kind: Kind, slot: u32, label: String, group: u32) -> u64 {
    let _k = crate::zone_alloc::kept();
    let label = label.as_str().to_string();
    let mut sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
    sites.push(std::sync::Arc::new(Site { kind, slot, label, args: std::sync::OnceLock::new(), returns: std::sync::OnceLock::new(), group, hits: AtomicU64::new(0), ns: AtomicU64::new(0), on: AtomicBool::new(true) }));
    sites.len() as u64 - 1
}

/// A new probe group's record (kept state, like the sites).
fn register_group(group: u32, slots: Vec<u32>, query: &str) {
    let _k = crate::zone_alloc::kept();
    let slots = slots.as_slice().to_vec();
    GROUPS.lock().unwrap_or_else(|e| e.into_inner()).push((group, 0, slots, query.to_string()));
}

/// Installs probes of `kind` for `query` (on the owner, within `ms`): (group, sites).
fn add(kind: Kind, query: &str, time: bool) -> Result<(u32, Vec<(u64, String, u32)>), String> {
    add_to(kind, Target::Query(query.to_string()), time)
}

enum Target {
    Query(String),
    Slot(u32),
}

fn add_to(kind: Kind, target: Target, time: bool) -> Result<(u32, Vec<(u64, String, u32)>), String> {
    let up = crate::jit::current_unit();
    if up.is_null() {
        return Err("no running unit".into());
    }
    let query = match &target {
        Target::Query(q) => q.clone(),
        Target::Slot(id) => format!("slot#{}", id),
    };
    let group = NEXT_GROUP.fetch_add(1, SeqCst) as u32;
    let r = crate::owner::call_within(crate::owner::Class::App, 10_000, move || -> Result<(u32, Vec<(u64, String, u32)>), String> {
        // the sites, groups and patch edits are kept state (the instrumented code calls into them
        // after a settle swap destroyed the compiler arena); patches::rebuild compiles in the
        // compiler phase
        let _kept = crate::zone_alloc::kept();
        let u = unsafe { &mut *up };
        let found = match target {
            Target::Query(ref q) => find(u, q, if kind == Kind::Break { 4 } else { 32 }),
            Target::Slot(id) => vec![(id, None, u.fns[id as usize].name.to_string())],
        };
        if found.is_empty() {
            return Err(format!("no compiled fn or line matches `{}` (a fn not called yet has no code to patch)", query));
        }
        // a slot and its SPEC-V twin run the same fn (unverified callers call the twin): both are
        // instrumented, into one site each (a twin compiled later joins on its first install)
        let mut found = found;
        for k in 0..found.len() {
            let (id, line, _) = found[k].clone();
            let sib = match u.fns[id as usize].key {
                crate::jit::FnKey::Plain(b) => Some(b),
                crate::jit::FnKey::Inst(..) | crate::jit::FnKey::Closure(..) => u.plain_twin_of(id),
                _ => None,
            };
            if let Some(s) = sib.filter(|&s| u.fns[s as usize].compiled && !found.iter().any(|f| f.0 == s)) {
                found.push((s, line, u.fns[s as usize].name.to_string()));
            }
        }
        let mut out = Vec::new();
        for (id, line, name) in found {
            let si = register_site(kind, id, match line { Some(l) => format!("{} (line {})", name, l), None => name.clone() }, group);
            let e = match kind {
                Kind::Count => super::patches::Edit::Count { site: si, time },
                Kind::Break => super::patches::Edit::Break { site: si, line },
                Kind::Restart => super::patches::Edit::Restart { site: si },
            };
            match unsafe { super::patches::add(up, id, super::patches::Owner::Group(group), e) } {
                Ok(true) => out.push((si, name, id)),
                Ok(false) => out.push((si, format!("{} (line not found in its code)", name), id)),
                Err(e) => out.push((si, format!("{}: not instrumented: {}", name, e), id)),
            }
        }
        register_group(group, out.iter().map(|x| x.2).collect(), &query);
        Ok((group, out))
    });
    r.unwrap_or_else(|| Err(format!("the owner did not take the request in 10 s ({})", crate::owner::serving_note())))
}

/// Removes probe group `group` (all groups with None): the code they replaced comes back.
fn remove(group: Option<u32>) -> Result<usize, String> {
    let _k = crate::zone_alloc::kept();
    let up = crate::jit::current_unit();
    let groups: Vec<(u32, u32, Vec<u32>, String)> = {
        let mut g = GROUPS.lock().unwrap_or_else(|e| e.into_inner());
        let (take, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|x| group.map_or(true, |k| k == x.0));
        *g = keep;
        take
    };
    if groups.is_empty() {
        return Err("no such probe group".into());
    }
    for s in SITES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if groups.iter().any(|g| g.0 == s.group) {
            s.on.store(false, Relaxed);
        }
    }
    // breakpoints of the group let their threads go
    {
        let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
        let sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
        for p in list.iter() {
            if sites.get(p.site as usize).map_or(false, |s| groups.iter().any(|g| g.0 == s.group)) {
                p.go.store(true, SeqCst);
            }
        }
        GO.notify_all();
    }
    let n = groups.len();
    let ids: Vec<u32> = groups.iter().map(|g| g.0).collect();
    let r = crate::owner::call_within(crate::owner::Class::App, 10_000, move || {
        let _kept = crate::zone_alloc::kept();
        for g in ids {
            unsafe { super::patches::remove_owner(up, super::patches::Owner::Group(g)) };
        }
    });
    r.map(|_| n).ok_or_else(|| format!("the owner did not take the request in 10 s ({}); the probes are off but still in the code", crate::owner::serving_note()))
}

// ------------------------------------------------------------------ pause

static PAUSED: AtomicBool = AtomicBool::new(false);

/// True when the thread is stopped for inspection: parked at a breakpoint or a panic, or a
/// program thread while the program is paused (a thread not yet held by the pause sweep, at most
/// 5 ms, may still move: the script stack read is then best effort).
pub fn stopped(t: &super::threads::Thread) -> bool {
    PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.os_id == t.os_id)
        || super::events::panic_parked(t.os_id)
        || (PAUSED.load(Relaxed) && program_thread(t))
}

/// Program threads (main and the threads the program started; not Relax's own, not inspect's).
fn program_thread(t: &super::threads::Thread) -> bool {
    let main = crate::jit::MAIN_TID.load(Relaxed);
    if (t.handle != 0 && t.handle == main) || t.os_id == std::process::id() as u64 {
        return true;
    }
    !(t.name.starts_with("run-") || t.name.starts_with("gandalf") || (t.handle != 0 && t.handle == crate::owner::owner_tid()))
}

#[cfg(target_os = "macos")]
mod pause_imp {
    static HELD: relax_session::sync::Mutex<Vec<crate::inspect::threads::Thread>> = relax_session::sync::Mutex::new(Vec::new());
    extern "C" {
        static mach_task_self_: u32;
        fn malloc_get_all_zones(task: u32, reader: *const u8, out: *mut *const usize, count: *mut u32) -> i32;
    }

    /// Every malloc zone's lock taken (zone->introspect->force_lock, as fork() does) while the
    /// threads are suspended: no thread stops holding an allocator lock, so inspect's commands
    /// (which allocate) keep working while the program is paused. Nothing may allocate between
    /// `lock_zones` and `unlock_zones` (this thread would wait for itself).
    unsafe fn zones() -> &'static [usize] {
        let mut addrs: *const usize = std::ptr::null();
        let mut n = 0u32;
        if malloc_get_all_zones(mach_task_self_, std::ptr::null(), &mut addrs, &mut n) != 0 || addrs.is_null() {
            return &[];
        }
        std::slice::from_raw_parts(addrs, n as usize)
    }
    /// malloc_zone_t: introspect is its 13th word; malloc_introspection_t: force_lock, force_unlock
    /// are its 6th and 7th
    unsafe fn zone_fn(zone: usize, k: usize) -> Option<unsafe extern "C" fn(usize)> {
        let intro = *((zone as *const usize).add(12));
        if intro == 0 {
            return None;
        }
        let f = *((intro as *const usize).add(k));
        (f != 0).then(|| std::mem::transmute::<usize, unsafe extern "C" fn(usize)>(f))
    }

    pub fn pause(want: &dyn Fn(&crate::inspect::threads::Thread) -> bool) -> usize {
        let skip = crate::inspect::SERVER_TIDS.lock().unwrap().clone();
        let mut list = crate::inspect::threads::list();
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        held.reserve(list.len());
        let picked: Vec<bool> = list.iter().map(|t| !(crate::inspect::is_inspect_thread(t, &skip) || !want(t))).collect();
        let _ = crate::inspect::threads::self_os_id();
        let mut rest: Vec<crate::inspect::threads::Thread> = Vec::with_capacity(list.len());
        unsafe {
            let zs = zones();
            for &z in zs {
                if let Some(f) = zone_fn(z, 5) {
                    f(z);
                }
            }
            // no allocation or free from here to the unlock: `held` has room, the threads move
            // out of `list` by drain (its buffer is freed after the unlock)
            for (k, t) in list.drain(..).enumerate() {
                if picked[k] && crate::inspect::threads::suspend(&t) {
                    held.push(t);
                } else {
                    // dropping a thread frees its name: after the unlock
                    rest.push(t);
                }
            }
            for &z in zs.iter().rev() {
                if let Some(f) = zone_fn(z, 6) {
                    f(z);
                }
            }
        }
        drop((list, picked, rest));
        held.len()
    }
    pub fn resume() -> usize {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        let n = held.len();
        for t in held.drain(..) {
            crate::inspect::threads::resume(&t);
        }
        n
    }
}

#[cfg(target_os = "linux")]
mod pause_imp {
    use std::sync::atomic::Ordering::Relaxed;
    static HELD: relax_session::sync::Mutex<Vec<(u64, &'static crate::inspect::threads::HoldSlot)>> = relax_session::sync::Mutex::new(Vec::new());
    /// Holds the running threads now and keeps holding threads as they start running (every
    /// 5 ms) until resume; blocked threads are never signalled.
    pub fn pause(want: &'static (dyn Fn(&crate::inspect::threads::Thread) -> bool + Sync)) -> usize {
        let n = sweep(want);
        { let _k = crate::zone_alloc::kept();
        let _ = std::thread::Builder::new().name("run-inspect-pause".into()).spawn(move || {
            crate::inspect::heap::exclude_this_thread();
            while super::PAUSED.load(Relaxed) {
                sweep(want);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        }
        n
    }
    fn sweep(want: &dyn Fn(&crate::inspect::threads::Thread) -> bool) -> usize {
        let skip = crate::inspect::SERVER_TIDS.lock().unwrap().clone();
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        let mut list = crate::inspect::threads::list();
        crate::inspect::name_handles(&mut list);
        for t in list {
            if skip.contains(&t.os_id) || t.name.starts_with("run-inspect") || t.name.starts_with("makepad-remote") || !want(&t) || held.iter().any(|h| h.0 == t.os_id) || t.state != "running" {
                continue;
            }
            if let Ok(slot) = crate::inspect::threads::hold_paused(t.os_id) {
                held.push((t.os_id, slot));
            }
        }
        held.len()
    }
    pub fn resume() -> usize {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        let n = held.len();
        for (_, s) in held.drain(..) {
            crate::inspect::threads::release_hold(s);
        }
        n
    }
}

#[cfg(any(windows, target_os = "android"))]
mod pause_imp {
    pub fn pause(_want: &'static (dyn Fn(&crate::inspect::threads::Thread) -> bool + Sync)) -> usize {
        0
    }
    pub fn resume() -> usize {
        0
    }
}

fn site_letter(k: Kind) -> &'static str {
    match k {
        Kind::Count => "C",
        Kind::Break => "B",
        Kind::Restart => "R",
    }
}

// ------------------------------------------------------------------ commands

/// `count <fn> [time=1]` | `break <fn>|<file>:<line>` | `probes` | `rm <group|all>` |
/// `continue [K<n>|all]` | `pause` | `resume`
pub fn cmd(req: &Req, out: &mut Out) {
    match req.cmd.as_str() {
        "count" | "break" | "restartable" => {
            let Some(q) = req.pos.first() else {
                out.err("usage: count <fn> [time=1] | break <fn>|<file>:<line> | restartable <fn>");
                return;
            };
            let kind = match req.cmd.as_str() {
                "count" => Kind::Count,
                "break" => Kind::Break,
                _ => Kind::Restart,
            };
            match add(kind, q, req.get("time") == Some("1")) {
                Ok((group, sites)) => {
                    out.measured(&format!("probe group G{} installed ({} slot(s))", group, sites.len()), "owner: instrumented builds", &[]);
                    for (si, name, slot) in sites {
                        out.line(&format!("{{\"site\":\"{}{}\",\"slot\":{},\"fn\":{}}}", site_letter(kind), si, slot, json_str(&short(&name, 160))));
                    }
                    out.line(&format!("{{\"more\":[\"probes\",\"rm G{}\"{}]}}", group, if kind == Kind::Break { ",\"watch on=break\",\"continue all\"" } else { "" }));
                }
                Err(e) => out.err(&e),
            }
        }
        "probes" => {
            // a snapshot per call: `probes since=<seq>` reports the change since one
            let now = counters();
            let t = crate::clock::wall_ns();
            let seq = SNAP_SEQ.fetch_add(1, SeqCst) + 1;
            out.measured("installed probes", "probe counters", &[]);
            if let Some(since) = req.get("since").and_then(|v| v.parse::<u64>().ok()) {
                probes_since(since, &now, t, out);
            }
            {
                let mut snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
                snaps.push_back((seq, t, now));
                while snaps.len() > 32 {
                    snaps.pop_front();
                }
            }
            let sites = SITES.lock().unwrap_or_else(|e| e.into_inner());
            for (i, s) in sites.iter().enumerate() {
                if !s.on.load(Relaxed) {
                    continue;
                }
                let hits = s.hits.load(Relaxed);
                let ns = s.ns.load(Relaxed);
                let time = if ns > 0 { format!(",\"total_ms\":{:.3},\"per_call_us\":{:.3}", ns as f64 / 1e6, ns as f64 / 1e3 / hits.max(1) as f64) } else { String::new() };
                out.line(&format!("{{\"site\":\"{}{}\",\"group\":\"G{}\",\"slot\":{},\"fn\":{},\"hits\":{}{}}}", site_letter(s.kind), i, s.group, s.slot, json_str(&short(&s.label, 160)), hits, time));
            }
            let parked = PARKED.lock().unwrap_or_else(|e| e.into_inner());
            for p in parked.iter() {
                out.line(&format!("{{\"parked\":\"K{}\",\"site\":\"B{}\",\"tid\":\"T{}\"}}", p.id, p.site, p.os_id));
            }
            drop(sites);
            let mut injected = Vec::new();
            super::probes::counters(&mut injected);
            for (k, h, ns) in injected.iter().filter(|x| !x.0.contains(" count ")) {
                out.line(&format!("{{\"probe\":{},\"hits\":{}{}}}", json_str(&short(k, 160)), h, if *ns > 0 { format!(",\"est_ms\":{:.3}", *ns as f64 / 1e6) } else { String::new() }));
            }
            out.line(&format!("{{\"paused\":{},\"seq\":{},\"rearm\":\"probes since={}\"}}", PAUSED.load(Relaxed), seq, seq));
        }
        "rm" => {
            let w = req.pos.first().map(String::as_str).unwrap_or("all");
            let g = if w == "all" { None } else { w.trim_start_matches('G').parse::<u32>().ok() };
            if w != "all" && g.is_none() {
                out.err("usage: rm <G<n>|all>");
                return;
            }
            match remove(g) {
                Ok(n) => out.line(&format!("{{\"removed_groups\":{}}}", n)),
                Err(e) => out.err(&e),
            }
        }
        "continue" => {
            let w = req.pos.first().map(String::as_str).unwrap_or("all");
            let id = w.trim_start_matches('K').parse::<u64>().ok();
            let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
            let mut n = 0;
            for p in list.iter() {
                if w == "all" || Some(p.id) == id {
                    p.go.store(true, SeqCst);
                    n += 1;
                }
            }
            GO.notify_all();
            out.line(&format!("{{\"continued\":{}}}", n));
        }
        "pause" => {
            if PAUSED.swap(true, SeqCst) {
                out.err("already paused");
                return;
            }
            let n = pause_imp::pause(&program_thread);
            super::mem::mark("pause");
            out.line(&format!("{{\"paused_threads\":{},\"at_ms\":{},\"more\":[\"stacks\",\"why-hung\",\"resume\"]}}", n, now_ms()));
        }
        "resume" if req.pos.is_empty() => {
            PAUSED.store(false, SeqCst);
            let n = pause_imp::resume();
            super::mem::mark("resume");
            out.line(&format!("{{\"resumed_threads\":{}}}", n));
        }
        _ => out.err("unknown probe command"),
    }
}

#[cfg(test)]
mod zone_tests {
    /// The P0 of 2026-10-20 (Stage: count / rm / count, then the settle swap: "use after
    /// compiler-zone destroy" in the counted fn): inspect's records were allocated in the owner's
    /// compiler phase. Registering, removing and registering again from the compiler phase -- as
    /// the owner does -- leaves nothing of inspect's state in the compiler arena (the kept scan
    /// that MAKEPAD_RUN_ZONE_CHECK=1 runs at every settle reports 0 for it).
    #[test]
    fn probe_records_outlive_the_compiler_arena() {
        assert!(crate::zone_alloc::compiler_arena_enabled());
        let count = || {
            let mut a = crate::jit::zone_scan::Acc::new_for_test();
            super::super::scan_kept(&mut a);
            a.total()
        };
        // the control: the scan sees an inspect-like record built in the compiler phase
        {
            let _c = crate::zone_alloc::compiler();
            let mut a = crate::jit::zone_scan::Acc::new_for_test();
            a.field("control");
            let v: Vec<String> = vec!["x".repeat(40)];
            a.vec(&v);
            a.string(&v[0]);
            assert_eq!(a.total(), 2, "the compiler arena is not in use: the fixture would prove nothing");
        }
        for round in 0..3 {
            {
                let _c = crate::zone_alloc::compiler();
                let label = format!("fixture::counted_fn round {}", round);
                let g = 9000 + round;
                let si = super::register_site(super::Kind::Count, 1, label, g);
                super::register_group(g, vec![1, 2, 3], "fixture::counted_fn");
                let (p, _) = super::super::probes::register_probe_for_test(g, "fixture::probed_fn");
                assert!(si < u64::MAX && p.id == g);
            }
            assert_eq!(count(), 0, "round {}: inspect state in the compiler arena", round);
            // rm: the groups go, the sites stay (off), as `rm all` does
            {
                let _c = crate::zone_alloc::compiler();
                let mut g = super::GROUPS.lock().unwrap_or_else(|e| e.into_inner());
                let _k = crate::zone_alloc::kept();
                g.retain(|x| x.0 < 9000);
            }
            assert_eq!(count(), 0, "round {} after rm", round);
        }
    }
}

#[cfg(test)]
mod name_tests {
    use super::{name_match, Match};

    #[test]
    fn fn_queries_name_impl_methods() {
        let n = "ttf_parser::Face::parse";
        assert_eq!(name_match(n, "ttf_parser::Face::parse"), Match::Exact);
        assert_eq!(name_match(n, "Face::parse"), Match::Tail);
        assert_eq!(name_match(n, "crate::Face::parse"), Match::Exact);
        assert_eq!(name_match(n, "ace::parse"), Match::Loose);
        assert_eq!(name_match(n, "Glyph::parse"), Match::None);
        let t = "makepad_model_script::vector::<Outline as ttf_parser::OutlineBuilder>::move_to";
        assert_eq!(name_match(t, "<Outline as ttf_parser::OutlineBuilder>::move_to"), Match::Tail);
        assert_eq!(name_match(t, "Outline::move_to"), Match::Tail);
        assert_eq!(name_match(t, "makepad_model_script::vector::Outline::move_to"), Match::Exact);
        assert_eq!(name_match(t, "crate::vector::Outline::move_to"), Match::Exact);
        assert_eq!(name_match("a::b::f", "crate::f"), Match::Tail);
    }
}
