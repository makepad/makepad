//! Which code runs (lane INSPECT-TIERS): every code body the JIT installed is one *code version*
//! of its slot -- an address range with the tier it was made at (baseline, O1, O2, O3, O3 with
//! guards, inspect's instrumented build, an error stub) and where it came from (compiled in this
//! run, the warm disk cache, the startup image, a live patch, inspect). The code tables record
//! that per body (jit::CodeKind), so frames, probes and `versions` say exactly which optimised
//! code a pc is in -- also without the owner -- and a frame whose position lies outside its
//! slot's fn is named as the inlined fn it is ("inlined into <slot> (its version)").
//!
//! Also here: finding the code that holds a fn's or a line's source *as inlined copies* in
//! other slots' code (their pc maps), `versions <fn>` and `tier <fn> o1|o3|auto`.

use super::{json_str, short, Out, Req};
use crate::jit::{CodeKind, FnKey, PcMap, Unit};
use std::sync::atomic::Ordering::Relaxed;

/// Source lines a slot's own fn spans: (file, first line, last line). Closures and twins are
/// their parent's / base's fn (a closure's source lies inside its parent fn).
pub(crate) fn fn_lines(u: &Unit, id: u32) -> Option<(u32, u32, u32)> {
    fn_lines_depth(u, id, 0)
}

fn fn_lines_depth(u: &Unit, id: u32, depth: u32) -> Option<(u32, u32, u32)> {
    if depth > 8 || id as usize >= u.fns.len() {
        return None;
    }
    match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => def_lines(u, *d),
        FnKey::Plain(b) | FnKey::Sandbox(b) => fn_lines_depth(u, *b, depth + 1),
        FnKey::Closure(_, _, p) => fn_lines_depth(u, *p, depth + 1),
        _ => None,
    }
}

/// The lines of fn item `d` (None: not a fn with a body in a loaded file).
pub(crate) fn def_lines(u: &Unit, d: crate::program::DefId) -> Option<(u32, u32, u32)> {
    let def = u.prog.def(d);
    let file = u.prog.files.get(def.file as usize)?;
    let item = file.ast.item(def.item);
    if !matches!(item.kind, crate::ast::ItemKind::Fn(..)) {
        return None;
    }
    Some((def.file, file.line_col(item.lo).0 as u32, file.line_col(item.hi).0 as u32))
}

/// (file, line) of a RIR / pc-map position.
pub(crate) fn pos_line(u: &Unit, p: u64) -> Option<(u32, u32)> {
    if p == 0 {
        return None;
    }
    let file = relax_backend::rir::pos_file(p);
    let f = u.prog.files.get(file as usize)?;
    Some((file, f.line_col(p as u32).0 as u32))
}

/// Same source file (any loaded version of the path).
pub(crate) fn same_file(u: &Unit, a: u32, b: u32) -> bool {
    a == b || matches!((u.prog.file_paths.get(a as usize), u.prog.file_paths.get(b as usize)), (Some(x), Some(y)) if x == y)
}

/// A source range code is looked for in: a fn's lines or one line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Lines {
    pub file: u32,
    pub lo: u32,
    pub hi: u32,
}

impl Lines {
    pub fn holds(&self, u: &Unit, p: u64) -> bool {
        matches!(pos_line(u, p), Some((f, l)) if l >= self.lo && l <= self.hi && same_file(u, f, self.file))
    }
}

/// Whether a code body's pc map has an instruction in `lines`.
pub(crate) fn map_holds(u: &Unit, map: &PcMap, lines: &[Lines]) -> bool {
    let mut hit = false;
    map.each(|o, p| {
        // (the variable table's entries hold no positions)
        if o != relax_backend::dbgloc::PC_VARS && lines.iter().any(|l| l.holds(u, p)) {
            hit = true;
            return false;
        }
        true
    });
    hit
}

/// One code body of a slot.
pub(crate) struct Body<'a> {
    pub start: u64,
    pub len: u32,
    pub map: &'a PcMap,
    /// "installed" (the slot's code now), "replaced" (an earlier version, kept for rollback),
    /// "retired" (replaced by O3 code; frames may still run in it)
    pub state: &'static str,
    pub gen: u32,
}

/// Every code body of slot `id`: its code now first, then the replaced and retired ones.
pub(crate) fn bodies(u: &Unit, id: u32) -> Vec<Body<'_>> {
    let f = &u.fns[id as usize];
    let mut v = Vec::new();
    if f.compiled && f.code_len > 0 {
        v.push(Body { start: f.code, len: f.code_len, map: &f.pc_map, state: "installed", gen: f.patch_gen });
    }
    for (c, l, m, g) in f.history().prev.iter().rev() {
        if *l > 0 && *c != f.code {
            v.push(Body { start: *c, len: *l, map: m, state: "replaced", gen: *g });
        }
    }
    for (c, l, m) in f.history().retired.iter().rev() {
        if *l > 0 && *c != f.code {
            v.push(Body { start: *c, len: *l, map: m, state: "retired", gen: 0 });
        }
    }
    v
}

/// The kind of the code body at `start` (the code tables: a leaf lock).
pub(crate) fn kind_at(u: &Unit, start: u64) -> CodeKind {
    u.code.read().unwrap_or_else(|e| e.into_inner()).kind_of(start)
}

/// A short label of a code version: "O3 warm-cache", "O1 fresh", "instrumented inspect".
pub(crate) fn label(k: CodeKind) -> String {
    format!("{} {}", k.tier_name(), k.origin_name())
}

/// One code body as a JSON object: slot, fn, tier, origin, address range, state, patch generation.
pub(crate) fn body_json(u: &Unit, id: u32, start: u64, len: u32, state: &str, gen: u32) -> String {
    let k = kind_at(u, start);
    let f = &u.fns[id as usize];
    let mut s = format!(
        "{{\"slot\":{},\"fn\":{},\"tier\":\"{}\",\"origin\":\"{}\",\"code\":\"{:#x}+{:#x}\",\"state\":\"{}\"",
        id,
        json_str(&short(&f.name, 140)),
        k.tier_name(),
        k.origin_name(),
        start,
        len,
        state
    );
    if gen > 0 {
        s.push_str(&format!(",\"gen\":{}", gen));
    }
    if k.origin == crate::jit::origin::CACHE || k.origin == crate::jit::origin::IMAGE {
        s.push_str(&format!(",\"build\":{}", json_str(crate::jit::PROVENANCE)));
    }
    s.push('}');
    s
}

/// The installed code of slot `id` as JSON (`body_json` of its code now; "none" when it has none).
pub(crate) fn installed_json(u: &Unit, id: u32) -> String {
    let f = &u.fns[id as usize];
    if !f.compiled || f.code_len == 0 {
        return format!("{{\"slot\":{},\"fn\":{},\"state\":\"no code (lazy: compiled on its next call)\"}}", id, json_str(&short(&f.name, 140)));
    }
    body_json(u, id, f.code, f.code_len, "installed", f.patch_gen)
}

/// Slot `id`'s installed code before a change: its JSON and kind.
pub(crate) fn snapshot(u: &Unit, id: u32) -> (String, Option<CodeKind>) {
    let f = &u.fns[id as usize];
    (installed_json(u, id), (f.compiled && f.code_len > 0).then(|| kind_at(u, f.code)))
}

/// The fields of an answer row about a slot inspect rebuilt: the code it replaced, the code now,
/// and what that means for the diagnosis.
pub(crate) fn patched_row(u: &Unit, id: u32, before: &(String, Option<CodeKind>), r: &Result<bool, String>) -> String {
    use crate::jit::tier;
    let now = installed_json(u, id);
    let mut s = format!("\"replaced\":{},\"now\":{}", before.0, now);
    let o3 = |t: u8| t == tier::O3 || t == tier::GUARDED || t == tier::INSTRUMENTED_O3;
    if let (Ok(true), Some(k)) = (r, before.1) {
        let new = kind_at(u, u.fns[id as usize].code);
        if (o3(k.tier) && !o3(new.tier)) || (k.tier == tier::O2 && !o3(new.tier) && new.tier != tier::O2) {
            s.push_str(&format!(
                ",\"note\":{}",
                json_str(&format!(
                    "its {} code was replaced by {} code: while this is installed the {} code does not run (a miscompile there cannot show). Compare in place: `tier <fn> o3` / `tier <fn> o1`",
                    label(k),
                    label(new),
                    k.tier_name()
                ))
            ));
        }
    }
    if let Some(n) = super::patches::tier_note(id) {
        s.push_str(&format!(",\"tier_note\":{}", json_str(&n)));
    }
    s
}

/// The lines a target covers: a fn's (None) or one line of its file.
pub(crate) fn target_lines(u: &Unit, targets: &[(u32, Option<u32>)]) -> Vec<Lines> {
    let mut v: Vec<Lines> = Vec::new();
    for &(id, line) in targets {
        if let Some((file, lo, hi)) = fn_lines(u, id) {
            let l = match line {
                Some(l) => Lines { file, lo: l, hi: l },
                None => Lines { file, lo, hi },
            };
            if !v.iter().any(|x| x.file == l.file && x.lo == l.lo && x.hi == l.hi) {
                v.push(l);
            }
        }
    }
    v
}

/// The slots whose installed code holds an inlined copy of `targets` (a fn or a line) are rebuilt
/// for `owner`: without inlining (probes, counters, breakpoints: they then call the instrumented
/// code) or, `pin`, at O1 (the pinned fn runs at O1 in them too). Later installs with a copy are
/// watched (patches::code_installed). On the owner. Returns one answer row per inliner.
pub(crate) unsafe fn cover_inliners(up: *mut Unit, owner: super::patches::Owner, targets: &[(u32, Option<u32>)], pin: bool) -> Vec<String> {
    let u = &*up;
    let ids: Vec<u32> = targets.iter().map(|t| t.0).collect();
    let lines = target_lines(u, targets);
    let mut rows = Vec::new();
    let found: Vec<u32> = {
        let mut v: Vec<u32> = holders(u, &lines, &[], false).into_iter().map(|x| x.0).filter(|&c| !own_slot(u, c, &ids)).collect();
        v.dedup();
        v
    };
    const MAX: usize = 64;
    for (k, &c) in found.iter().enumerate() {
        let u = &*up;
        let name = super::display_name(u, c, 0);
        if k >= MAX {
            rows.push(format!("{{\"inliner\":{},\"slot\":{},\"rebuilt\":false,\"why\":{}}}", json_str(&short(&name, 160)), c, json_str(&format!("more than {} slots hold an inlined copy: this one still runs unprobed", MAX))));
            continue;
        }
        let before = snapshot(u, c);
        let r = super::patches::add(up, c, owner, if pin { super::patches::Edit::Pin } else { super::patches::Edit::Uninline });
        let u = &*up;
        let still = map_holds(u, &u.fns[c as usize].pc_map, &lines);
        let now_tier = kind_at(u, u.fns[c as usize].code).tier_name();
        let why = match (&r, pin, still) {
            (Ok(true), true, _) => "its code held an inlined copy: built again at O1 (the pinned fn runs at O1 here too)".to_string(),
            (Ok(true), false, false) => format!("its code held an inlined copy: built again ({}) without inlining the fn, so it calls the instrumented code", now_tier),
            (Ok(true), false, true) => "rebuilt without inlining, but its code still holds the line (a core adapter's inlined closure?): that copy runs unprobed".to_string(),
            (Ok(false), ..) => "the rebuild found no place: its inlined copy runs unprobed".to_string(),
            (Err(e), ..) => format!("not rebuilt ({}): its inlined copy runs unprobed", e),
        };
        rows.push(format!(
            "{{\"inliner\":{},\"slot\":{},\"rebuilt\":{},\"unprobed_copy\":{},\"why\":{},{}}}",
            json_str(&short(&name, 160)),
            c,
            matches!(r, Ok(true)),
            !pin && (still || !matches!(r, Ok(true))),
            json_str(&why),
            patched_row(u, c, &before, &r)
        ));
    }
    super::patches::watch(owner, lines, ids, pin);
    rows
}

/// What a JIT pc is in: its slot's code version and, when its source position lies outside the
/// slot's own fn, the fn whose inlined copy it is.
pub(crate) struct PcCode {
    pub slot: u32,
    pub start: u64,
    pub len: u32,
    pub kind: CodeKind,
    pub state: &'static str,
    pub gen: u32,
    /// file:line:col
    pub at: String,
    /// the inlined fn, when the pc's position lies outside the slot's own fn
    pub inlined: Option<Inlined>,
}

/// The fn an inlined copy comes from.
pub(crate) struct Inlined {
    /// its slot, when the code's inlined slots name it
    pub slot: Option<u32>,
    pub name: String,
    /// the inline stack (the code's line table), innermost first: (fn, its def, file:line:col);
    /// the last level is the slot's own fn at the outermost call site. Empty: code made before
    /// the line table kept inline stacks (the fn was found by its source span)
    pub stack: Vec<(String, Option<crate::program::DefId>, String)>,
    /// a chain deeper than the context field holds
    pub lost: bool,
}

/// Finds which fn an inlined position belongs to: the slots the code inlined (O3 bodies record
/// them; O1 code its slot's body_deps), else any fn item of the file spanning the line.
pub(crate) struct Resolver {
    by_file: std::collections::HashMap<u32, Vec<(u32, u32, crate::program::DefId)>>,
}

impl Resolver {
    pub fn new() -> Resolver {
        Resolver { by_file: Default::default() }
    }

    fn file_fns(&mut self, u: &Unit, file: u32) -> &[(u32, u32, crate::program::DefId)] {
        self.by_file.entry(file).or_insert_with(|| {
            // every fn item of the file (a fn that only runs inlined into code from the disk
            // cache has no slot of its own)
            let mut v = Vec::new();
            for &d in u.prog.defs_in_file(file) {
                let d = crate::program::DefId(d);
                if matches!(u.prog.def(d).kind, crate::program::DefKind::Fn | crate::program::DefKind::AssocFn) {
                    if let Some((_, lo, hi)) = def_lines(u, d) {
                        v.push((lo, hi, d));
                    }
                }
            }
            v
        })
    }

    /// The fn a position inside slot `id`'s code at `start` comes from, when not its own.
    /// The innermost fn item spanning a position (by its file's fn items).
    pub fn fn_at(&mut self, u: &Unit, p: u64) -> Option<crate::program::DefId> {
        let (pf, pl) = pos_line(u, p)?;
        let mut inner: Option<(u32, crate::program::DefId)> = None;
        for &(lo, hi, d) in self.file_fns(u, pf) {
            if pl >= lo && pl <= hi && inner.map_or(true, |x| hi - lo < x.0) {
                inner = Some((hi - lo, d));
            }
        }
        inner.map(|x| x.1)
    }

    /// The inline stack of position `p` in a body whose pc map is `map` (its inline entries,
    /// rir::inline_pc_entries): None when `p` is the body's own code.
    pub fn inline_stack(&mut self, u: &Unit, id: u32, map: &PcMap, p: u64) -> Option<Inlined> {
        use relax_backend::rir::{pos_ctx, POS_CTX_LOST};
        if pos_ctx(p) == 0 {
            return None;
        }
        let mut table: Vec<u64> = Vec::new();
        map.each(|o, q| {
            if o == u32::MAX {
                table.push(q);
            }
            true
        });
        let pstr = |q: u64| u.pos_str(relax_backend::rir::pos_file(q), q as u32);
        let mut stack = Vec::new();
        let mut cur = p;
        let mut lost = false;
        for _ in 0..256 {
            let c = pos_ctx(cur);
            if c == 0 {
                break;
            }
            if c == POS_CTX_LOST || (c as usize) * 2 > table.len() {
                lost = true;
                break;
            }
            let (call, anchor) = (table[(c as usize - 1) * 2], table[(c as usize - 1) * 2 + 1]);
            let def = self.fn_at(u, anchor);
            let name = def.map_or_else(|| "(inlined fn)".to_string(), |d| crate::lower::display_path(&u.prog, d));
            stack.push((name, def, pstr(cur)));
            cur = call;
        }
        stack.push((super::display_name(u, id, 0), None, pstr(cur)));
        let (name, def) = (stack[0].0.clone(), stack[0].1);
        let slot = def.and_then(|d| u.fns.iter().position(|f| matches!(&f.key, FnKey::Inst(x, _) if *x == d)).map(|i| i as u32));
        Some(Inlined { slot, name, stack, lost })
    }

    pub fn inlined_fn(&mut self, u: &Unit, id: u32, start: u64, p: u64) -> Option<Inlined> {
        let (pf, pl) = pos_line(u, p)?;
        let own = fn_lines(u, id);
        if let Some((f, lo, hi)) = own {
            if same_file(u, f, pf) && pl >= lo && pl <= hi {
                return None;
            }
        } else if !matches!(u.fns[id as usize].key, FnKey::Inst(..) | FnKey::Closure(..) | FnKey::Plain(..)) {
            // glue and shims: generated code, positions of what they forward to
            return None;
        }
        // candidates: what this body (O3) or the slot's RIR (O1) took in
        let mut cands: Vec<u32> = u.code.read().unwrap_or_else(|e| e.into_inner()).inlined_at(start).map(|s| s.to_vec()).unwrap_or_default();
        {
            let mut work = vec![id];
            let mut seen = std::collections::HashSet::new();
            while let Some(s) = work.pop() {
                if !seen.insert(s) || seen.len() > 4096 {
                    continue;
                }
                if let Some(d) = u.body_deps.get(&s) {
                    for &c in d {
                        cands.push(c);
                        work.push(c);
                    }
                }
            }
        }
        // the innermost fn spanning the line
        let mut best: Option<(u32, u32)> = None;
        for &c in &cands {
            if let Some((f, lo, hi)) = fn_lines(u, c) {
                if same_file(u, f, pf) && pl >= lo && pl <= hi && best.map_or(true, |b| hi - lo < b.1) {
                    best = Some((c, hi - lo));
                }
            }
        }
        if let Some((c, _)) = best {
            return Some(Inlined { slot: Some(c), name: super::display_name(u, c, 0), stack: Vec::new(), lost: false });
        }
        // any fn item of the file that spans the line (innermost)
        let mut inner: Option<(u32, crate::program::DefId)> = None;
        for &(lo, hi, d) in self.file_fns(u, pf) {
            if pl >= lo && pl <= hi && inner.map_or(true, |x| hi - lo < x.0) {
                inner = Some((hi - lo, d));
            }
        }
        match inner {
            Some((_, d)) => Some(Inlined { slot: None, name: crate::lower::display_path(&u.prog, d), stack: Vec::new(), lost: false }),
            None => Some(Inlined { slot: None, name: format!("(inlined code of {}:{})", u.prog.file_paths.get(pf as usize).map(String::as_str).unwrap_or("?"), pl), stack: Vec::new(), lost: false }),
        }
    }
}

/// What JIT pc `pc` is in (on the owner or with the unit otherwise stable: reads slots).
pub(crate) fn describe_pc(u: &Unit, r: &mut Resolver, pc: u64) -> Option<PcCode> {
    let (start, len, id) = u.code.read().unwrap_or_else(|e| e.into_inner()).entry_at(pc)?;
    let kind = kind_at(u, start);
    let all = bodies(u, id);
    let b = all.iter().find(|b| b.start == start);
    let (state, gen, map) = match b {
        Some(b) => (b.state, b.gen, Some(b.map)),
        None => ("replaced", 0, None),
    };
    let pos = map.and_then(|m| m.lookup((pc - start) as u32));
    let at = match pos {
        Some(p) => u.pos_str(relax_backend::rir::pos_file(p), p as u32),
        None => u.prog.file_paths.get(u.fns[id as usize].file as usize).cloned().unwrap_or_default(),
    };
    // the line table's inline stack, else (older code) the fn whose span holds the position
    let inlined = pos.and_then(|p| map.and_then(|m| r.inline_stack(u, id, m, p)).or_else(|| r.inlined_fn(u, id, start, p)));
    Some(PcCode { slot: id, start, len, kind, state, gen, at, inlined })
}

/// The frame fields of a described pc: (fn name, file:line:col, module label, code JSON).
pub(crate) fn frame_fields(u: &Unit, c: &PcCode) -> (String, String, String, String) {
    let slot_name = super::display_name(u, c.slot, 0);
    let mut module = format!("jit/{}", label(c.kind));
    if c.state != "installed" {
        module.push_str(&format!(" ({})", c.state));
    }
    let mut code = format!(
        "{{\"slot\":{},\"tier\":\"{}\",\"origin\":\"{}\",\"code\":\"{:#x}+{:#x}\",\"state\":\"{}\"",
        c.slot,
        c.kind.tier_name(),
        c.kind.origin_name(),
        c.start,
        c.len,
        c.state
    );
    if c.gen > 0 {
        code.push_str(&format!(",\"gen\":{}", c.gen));
    }
    if c.kind.origin == crate::jit::origin::CACHE || c.kind.origin == crate::jit::origin::IMAGE {
        code.push_str(&format!(",\"build\":{}", json_str(crate::jit::PROVENANCE)));
    }
    if super::patches::active(c.slot) {
        code.push_str(",\"inspect\":\"instrumented or pinned by makepad inspect (probes)\"");
    }
    let name = match &c.inlined {
        Some(i) => {
            code.push_str(&format!(",\"inlined_into\":{}", json_str(&short(&slot_name, 140))));
            if let Some(cs) = i.slot {
                code.push_str(&format!(",\"inlined_slot\":{}", cs));
            }
            if i.stack.is_empty() {
                module.push_str(&format!(" inlined into {}", short(&slot_name, 80)));
                // code made before the line table kept inline stacks
                code.push_str(",\"call_site\":\"not recorded (code without an inline stack)\"");
            } else {
                let call = &i.stack[i.stack.len() - 1].2;
                module.push_str(&format!(" inlined at {} in {}", call, short(&slot_name, 80)));
                let levels: Vec<String> = i.stack.iter().map(|(n, _, at)| format!("{{\"fn\":{},\"at\":{}}}", json_str(&short(n, 140)), json_str(at))).collect();
                code.push_str(&format!(",\"call_site\":{},\"inline_stack\":[{}]", json_str(call), levels.join(",")));
                if i.lost {
                    code.push_str(",\"inline_stack_cut\":true");
                }
            }
            i.name.clone()
        }
        None => slot_name,
    };
    code.push('}');
    (name, c.at.clone(), module, code)
}

/// Without the owner (it is busy): the code tables still name the version.
pub(crate) fn frame_fields_no_owner(u: &Unit, pc: u64) -> Option<(String, String, String, String)> {
    let t = u.code.try_read().ok()?;
    let (start, len, id) = t.entry_at(pc)?;
    let k = t.kind_of(start);
    Some((
        format!("slot#{}+{:#x}", id, pc - start),
        String::new(),
        format!("jit/{}", label(k)),
        format!("{{\"slot\":{},\"tier\":\"{}\",\"origin\":\"{}\",\"code\":\"{:#x}+{:#x}\",\"state\":\"? (owner busy)\"}}", id, k.tier_name(), k.origin_name(), start, len),
    ))
}

/// JIT code that is no fn body: a slot's thunk (what calls branch to), its lazy stub, or a veneer
/// (a branch to a host function or a far target): (name, module label). A profile sample on one
/// is the cost of reaching the callee (a fetch of a thunk line that is not in the i-cache).
pub(crate) fn glue_name(u: &Unit, pc: u64) -> Option<(String, &'static str)> {
    for (kind, what, module) in [(0usize, "thunk of", "jit/glue thunk"), (1, "lazy stub of", "jit/glue stub")] {
        if let Some((id, _)) = u.slots.find(kind, pc).filter(|(id, _)| (*id as usize) < u.fns.len()) {
            return Some((format!("[{} {}]", what, super::display_name(u, id, 0)), module));
        }
    }
    // a call counter's stub (tier record word 2: where the slot's counting stub is)
    #[cfg(target_arch = "aarch64")]
    let cs = relax_backend::arm64::COUNT_STUB_SIZE as u64;
    #[cfg(not(target_arch = "aarch64"))]
    let cs = relax_backend::x64::COUNT_STUB_SIZE as u64;
    for id in 0..u.fns.len() as u32 {
        if !u.slots.has_seg(relax_backend::rir::SlotTable::locate(id).0) {
            break;
        }
        let stub = unsafe { (*(u.slots.tier(id) as *const std::sync::atomic::AtomicU64).add(2)).load(Relaxed) };
        if stub != 0 && pc >= stub && pc < stub + cs {
            return Some((format!("[count stub of {}]", super::display_name(u, id, 0)), "jit/glue count"));
        }
    }
    let v = u.veneers.try_lock().ok()?;
    let (target, _) = v.iter().find(|(_, &at)| pc >= at && pc < at + 16)?;
    let to = match u.code.try_read().ok().and_then(|t| t.entry_at(*target)) {
        Some((_, _, id)) => super::display_name(u, id, 0),
        None => match u.slots.find(0, *target).filter(|(id, _)| (*id as usize) < u.fns.len()) {
            Some((id, _)) => format!("thunk of {}", super::display_name(u, id, 0)),
            None => super::symbols::host_symbol(*target).map_or(format!("{:#x}", target), |s| s.0),
        },
    };
    Some((format!("[veneer to {}]", to), "jit/glue veneer"))
}

// ------------------------------------------------------------------ inlined copies

/// Slots (other than `skip`) whose code holds an instruction of `lines` (their installed code;
/// also an older body a thread may still run, when `all` is set): (slot, body start, state).
pub(crate) fn holders(u: &Unit, lines: &[Lines], skip: &[u32], all: bool) -> Vec<(u32, u64, &'static str)> {
    let mut out = Vec::new();
    if lines.is_empty() {
        return out;
    }
    for id in 0..u.fns.len() as u32 {
        if skip.contains(&id) {
            continue;
        }
        let f = &u.fns[id as usize];
        if !f.compiled {
            continue;
        }
        // (a body is only read when the slot's file or what it inlined could hold the lines: any
        // file; the pc map decides)
        for b in bodies(u, id) {
            if b.state != "installed" && !all {
                continue;
            }
            if map_holds(u, b.map, lines) {
                out.push((id, b.start, b.state));
            }
        }
    }
    out
}

/// The slots that are the target fn itself for `lines` of a fn query: the target slots and their
/// twins and closures (whose source is inside the fn).
pub(crate) fn own_slot(u: &Unit, id: u32, targets: &[u32]) -> bool {
    let mut s = id;
    for _ in 0..8 {
        if targets.contains(&s) {
            return true;
        }
        s = match u.fns[s as usize].key {
            FnKey::Plain(b) | FnKey::Sandbox(b) => b,
            FnKey::Closure(_, _, p) => p,
            _ => return false,
        };
    }
    false
}

// ------------------------------------------------------------------ versions <fn>

/// `versions <fn|file:line>`: every code version of the fn's slots (installed, replaced, retired:
/// tier, origin, address, patch generation, cache key), which thread runs in which, and the
/// slots whose code holds an inlined copy of it (with their versions and threads).
pub fn cmd_versions(req: &Req, out: &mut Out) {
    let Some(q) = req.pos.first().cloned() else {
        out.err("usage: versions <fn|file:line>");
        return;
    };
    let up = crate::jit::current_unit();
    if up.is_null() {
        return out.err("no running unit");
    }
    // every thread's stack, captured now (no owner): which code each one is in
    let caps = super::capture_all(&|_| true).unwrap_or_default();
    let stacks: Vec<(String, Vec<u64>)> = caps
        .iter()
        .map(|c| (format!("{} ({})", if c.t.name.is_empty() { "-" } else { &c.t.name }, super::tid_label(c.t.os_id)), c.frames.iter().enumerate().map(|(i, &pc)| if i == 0 { pc } else { pc.wrapping_sub(1) }).collect()))
        .collect();
    let r = crate::owner::call_within(crate::owner::Class::Job, 10_000, move || -> Result<Vec<String>, String> {
        let u = unsafe { &mut *up };
        let found = super::instrument::find_all(u, &q, 32);
        if found.is_empty() {
            return Err(format!("no fn or line matches `{}`{}", q, super::instrument::closest(u, &q)));
        }
        let threads_in = |start: u64, len: u32| -> Vec<String> {
            stacks
                .iter()
                .filter_map(|(n, pcs)| pcs.iter().position(|&pc| pc >= start && pc < start + len as u64).map(|d| if d == 0 { n.clone() } else { format!("{} (frame {})", n, d) }))
                .collect()
        };
        let mut rows = Vec::new();
        let mut lines = Vec::new();
        let mut targets = Vec::new();
        for (id, line, _) in &found {
            targets.push(*id);
            let cache_key = {
                let mut c = u.cache.take();
                let k = c.as_mut().and_then(|c| c.inspect_key(u, *id));
                u.cache = c;
                k
            };
            let f = &u.fns[*id as usize];
            let word = crate::jit::slot_word(u, *id);
            let key = match &f.key {
                FnKey::Inst(..) => "fn".to_string(),
                FnKey::Closure(..) => "closure".to_string(),
                FnKey::Plain(b) => format!("SPEC-V unverified twin of slot {}", b),
                FnKey::Sandbox(b) => format!("sandboxed twin of slot {}", b),
                FnKey::Glue(..) => "glue".to_string(),
                FnKey::Shim(..) => "shim".to_string(),
            };
            let pin = pin_of(*id);
            rows.push(format!(
                "{{\"slot\":{},\"fn\":{},\"kind\":{},\"compiled\":{},\"version\":{},\"table_word\":\"{:#x}\",\"runs\":{},\"o3_mark\":{},\"unverified\":{},\"cache_key\":{},\"build\":{}{}{}}}",
                id,
                json_str(&short(&super::display_name(u, *id, 0), 160)),
                json_str(&key),
                f.compiled,
                f.version,
                word,
                json_str(&if !f.compiled { "nothing yet (lazy stub: compiled on its next call)".to_string() } else if word == f.code { format!("{:#x} (its installed code)", f.code) } else { format!("{:#x} (a stub in front of {:#x}: tier-up counter or replay)", word, f.code) }),
                f.o3.load(Relaxed),
                f.unverified.load(Relaxed),
                cache_key.map_or("null".to_string(), |k| format!("\"{:032x}\"", k)),
                json_str(crate::jit::PROVENANCE),
                line.map_or(String::new(), |l| format!(",\"line\":{}", l)),
                pin.map_or(String::new(), |p| format!(",\"pinned\":{}", json_str(&p))),
            ));
            for b in bodies(u, *id) {
                let mut j = body_json(u, *id, b.start, b.len, b.state, b.gen);
                j.pop();
                let inl = u.code.read().unwrap_or_else(|e| e.into_inner()).inlined_at(b.start).map(|s| s.to_vec());
                if let Some(inl) = inl {
                    let names: Vec<String> = inl.iter().take(24).map(|c| json_str(&short(&u.fns[*c as usize].name, 100))).collect();
                    j.push_str(&format!(",\"inlined\":[{}]{}", names.join(","), if inl.len() > 24 { format!(",\"inlined_more\":{}", inl.len() - 24) } else { String::new() }));
                }
                let th = threads_in(b.start, b.len);
                j.push_str(&format!(",\"threads\":[{}]}}", th.iter().map(|t| json_str(t)).collect::<Vec<_>>().join(",")));
                rows.push(format!("{{\"version\":{}}}", j));
            }
            match line {
                Some(l) => {
                    if let Some((file, _, _)) = fn_lines(u, *id) {
                        lines.push(Lines { file, lo: *l, hi: *l });
                    }
                }
                None => {
                    if let Some((file, lo, hi)) = fn_lines(u, *id) {
                        lines.push(Lines { file, lo, hi });
                    }
                }
            }
        }
        // inlined copies: every slot whose code (installed, or an older body a thread runs) holds
        // the fn's lines
        let mut n = 0;
        for (c, start, state) in holders(u, &lines, &[], true) {
            if own_slot(u, c, &targets) {
                continue;
            }
            let th = threads_in(start, u.fns[c as usize].code_len.max(1));
            if state != "installed" && th.is_empty() {
                continue;
            }
            n += 1;
            if n > 64 {
                continue;
            }
            let len = bodies(u, c).iter().find(|b| b.start == start).map_or(0, |b| b.len);
            let th = threads_in(start, len);
            let mut j = body_json(u, c, start, len, state, 0);
            j.pop();
            j.push_str(&format!(",\"threads\":[{}]}}", th.iter().map(|t| json_str(t)).collect::<Vec<_>>().join(",")));
            rows.push(format!("{{\"inlined_into\":{}}}", j));
        }
        if n > 64 {
            rows.push(format!("{{\"inlined_into_more\":{}}}", n - 64));
        }
        rows.push(format!("{{\"inliners\":{},\"more\":[{},{},{}]}}", n, json_str(&format!("tier {} o1", q)), json_str(&format!("tier {} o3", q)), json_str(&format!("count {}", q))));
        Ok(rows)
    });
    match r.unwrap_or_else(|| Err(format!("the owner did not answer in 10 s ({})", crate::owner::serving_note()))) {
        Ok(rows) => {
            out.measured("code versions", "code tables + slot history (owner) + stacks (os)", &[]);
            for l in rows {
                out.line(&l);
            }
        }
        Err(e) => out.err(&e),
    }
}

// ------------------------------------------------------------------ tier <fn> o1|o3|auto

/// The `tier` pins: (pin, query, tier asked, target slots).
static PINS: relax_session::sync::Mutex<Vec<(u32, String, &'static str, Vec<u32>)>> = relax_session::sync::Mutex::new(Vec::new());
static NEXT_PIN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    if let Ok(p) = PINS.try_lock() {
        a.field("inspect.pins");
        a.vec(&p);
        for x in p.iter() {
            a.string(&x.1);
            a.vec(&x.3);
        }
    }
}

/// The pin a slot is under: "o1 (pin<n>)" / "o3 (pin<n>)".
pub(crate) fn pin_of(id: u32) -> Option<String> {
    PINS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|p| p.3.contains(&id)).map(|p| format!("{} (pin{}: tier {} {})", p.2, p.0, p.1, p.2))
}

/// The live pins as owners (`probes` lists their slots).
pub(crate) fn pin_owners() -> Vec<(super::patches::Owner, String)> {
    PINS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| p.2 == "o1").map(|p| (super::patches::Owner::Pin(p.0), format!("tier {} o1", p.1))).collect()
}

/// `tier <fn|file:line> o1|o3|auto [fresh=1]` | `tier` (the pins): runs a fn -- and every slot whose
/// code holds an inlined copy of it -- at one tier, in the live process, to A/B a suspected
/// miscompile. o1: O1 code generation of their O1 RIR, kept (no O2 / O3 install replaces it, no new
/// build inlines the fn). o3: the o1 pin goes (the code from before it comes back) and every one of
/// them not running O3 code -- all of them with fresh=1 -- gets an O3 build now (fresh: compiled in
/// this run, never from the disk cache). auto: the pins go.
pub fn cmd_tier(req: &Req, out: &mut Out) {
    let (Some(q), Some(level)) = (req.pos.first().cloned(), req.pos.get(1).cloned()) else {
        let pins = PINS.lock().unwrap_or_else(|e| e.into_inner()).clone();
        out.measured(&format!("{} tier pin(s)", pins.len()), "inspect pins", &[]);
        for (n, q, t, slots) in pins {
            out.line(&format!("{{\"pin\":\"pin{}\",\"fn\":{},\"tier\":\"{}\",\"slots\":{:?}}}", n, json_str(&q), t, slots));
        }
        out.line("{\"usage\":\"tier <fn|file:line> o1|o3|auto [fresh=1]\"}");
        return;
    };
    let level: &'static str = match level.to_ascii_lowercase().as_str() {
        "o1" => "o1",
        "o3" => "o3",
        "auto" | "off" | "rm" => "auto",
        l => return out.err(&format!("tier {}: o1 | o3 | auto", l)),
    };
    let fresh = req.get("fresh") == Some("1");
    let up = crate::jit::current_unit();
    if up.is_null() {
        return out.err("no running unit");
    }
    let qq = q.clone();
    // on the owner: the targets, the old pins gone, the o1 pin's rebuilds; for o3 the slots to build
    let r = crate::owner::call_within(crate::owner::Class::Program, 30_000, move || -> Result<(Vec<String>, Vec<u32>), String> {
        let _kept = crate::zone_alloc::kept();
        let u = unsafe { &mut *up };
        let mut found: Vec<(u32, Option<u32>)> = super::instrument::find_all(u, &qq, 16).into_iter().map(|x| (x.0, x.1)).collect();
        if found.is_empty() {
            return Err(format!("no fn or line matches `{}`{}", qq, super::instrument::closest(u, &qq)));
        }
        // a slot's SPEC-V twin runs the same fn
        for k in 0..found.len() {
            let (id, line) = found[k];
            let sib = match u.fns[id as usize].key {
                FnKey::Plain(b) => Some(b),
                FnKey::Inst(..) | FnKey::Closure(..) => u.plain_twin_of(id),
                _ => None,
            };
            if let Some(s) = sib.filter(|&s| u.fns[s as usize].compiled && !found.iter().any(|f| f.0 == s)) {
                found.push((s, line));
            }
        }
        let ids: Vec<u32> = found.iter().map(|x| x.0).collect();
        let mut rows = Vec::new();
        // earlier pins of these slots go first (their code from before the pin comes back)
        let old: Vec<(u32, String)> = {
            let mut p = PINS.lock().unwrap_or_else(|e| e.into_inner());
            let (gone, keep): (Vec<_>, Vec<_>) = p.drain(..).partition(|x| x.3.iter().any(|s| ids.contains(s)));
            *p = keep;
            gone.into_iter().map(|x| (x.0, format!("tier {} {}", x.1, x.2))).collect()
        };
        for (n, what) in old {
            let owner = super::patches::Owner::Pin(n);
            let slots = super::patches::owner_slots(owner);
            unsafe { super::patches::remove_owner(up, owner) };
            let u = unsafe { &*up };
            rows.push(format!("{{\"unpinned\":\"pin{}\",\"was\":{},\"slots\":[{}]}}", n, json_str(&what), slots.iter().map(|s| installed_json(u, s.0)).collect::<Vec<_>>().join(",")));
        }
        let u = unsafe { &*up };
        match level {
            "auto" => {
                for &id in &ids {
                    rows.push(format!("{{\"slot\":{},\"now\":{}}}", id, installed_json(u, id)));
                }
                Ok((rows, Vec::new()))
            }
            "o1" => {
                let n = NEXT_PIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let owner = super::patches::Owner::Pin(n);
                for &(id, _) in &found {
                    let before = snapshot(unsafe { &*up }, id);
                    let r = unsafe { super::patches::add(up, id, owner, super::patches::Edit::Pin) };
                    let u = unsafe { &*up };
                    rows.push(format!("{{\"pinned\":{},\"slot\":{},\"ok\":{},{}}}", json_str(&short(&super::display_name(u, id, 0), 160)), id, matches!(r, Ok(true)), patched_row(u, id, &before, &r)));
                    if let Err(e) = r {
                        rows.push(format!("{{\"slot\":{},\"error\":{}}}", id, json_str(&e)));
                    }
                }
                rows.extend(unsafe { cover_inliners(up, owner, &found, true) });
                let mut slots = super::patches::owner_slots(owner).into_iter().map(|s| s.0).collect::<Vec<u32>>();
                slots.extend(ids.iter().copied());
                slots.sort_unstable();
                slots.dedup();
                let _k = crate::zone_alloc::kept();
                PINS.lock().unwrap_or_else(|e| e.into_inner()).push((n, qq.as_str().to_string(), "o1", slots.as_slice().to_vec()));
                rows.push(format!("{{\"pin\":\"pin{}\",\"tier\":\"o1\",\"more\":[{},{},{}]}}", n, json_str(&format!("tier {} o3", qq)), json_str(&format!("tier {} auto", qq)), json_str(&format!("versions {}", qq))));
                Ok((rows, Vec::new()))
            }
            _ => {
                if u.tier1.is_none() {
                    return Err("O3 is off in this run (no tier-up): nothing to build at o3".into());
                }
                let lines = target_lines(u, &found);
                let mut build: Vec<u32> = ids.clone();
                for (c, _, _) in holders(u, &lines, &[], false) {
                    if !build.contains(&c) {
                        build.push(c);
                    }
                }
                let build: Vec<u32> = build
                    .into_iter()
                    .filter(|&id| {
                        let f = &u.fns[id as usize];
                        if !f.compiled {
                            return false;
                        }
                        let k = kind_at(u, f.code);
                        fresh || !(k.tier == crate::jit::tier::O3 || k.tier == crate::jit::tier::GUARDED)
                    })
                    .collect();
                for &id in &ids {
                    if !build.contains(&id) {
                        rows.push(format!("{{\"slot\":{},\"kept\":true,\"why\":{},\"now\":{}}}", id, json_str(if u.fns[id as usize].compiled { "it runs O3 code already (fresh=1 builds it again in this run)" } else { "it has no code of its own (it runs inlined)" }), installed_json(u, id)));
                    }
                }
                Ok((rows, build))
            }
        }
    });
    let (mut rows, build) = match r.unwrap_or_else(|| Err(format!("the owner did not take the request in 30 s ({})", crate::owner::serving_note()))) {
        Ok(x) => x,
        Err(e) => return out.err(&e),
    };
    if level == "o3" {
        // O3 builds run off the owner, as the tier-up threads run them
        let n = NEXT_PIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        let list = build.clone();
        // (the unit's address crosses to the build thread as a number)
        let upv = up as usize;
        let spawned = {
            let _k = crate::zone_alloc::kept();
            std::thread::Builder::new().name("run-inspect-tier".into()).spawn(move || {
                super::heap::exclude_this_thread();
                let up = upv as *mut Unit;
                let mut rows = Vec::new();
                for id in list {
                    let before = crate::owner::call(crate::owner::Class::Program, || snapshot(unsafe { &*up }, id));
                    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { crate::jit::compile_tier1(up, id) })).unwrap_or(false);
                    let row = crate::owner::call(crate::owner::Class::Program, || {
                        let u = unsafe { &*up };
                        let r: Result<bool, String> = if ok { Ok(true) } else { Err("the O3 build was dropped (not verified, instrumented by a probe, outdated, or failed: MAKEPAD_RUN_TIER1_TRACE=1 says why)".into()) };
                        let mut s = format!("{{\"o3\":{},\"slot\":{},\"installed\":{},{}", json_str(&short(&super::display_name(u, id, 0), 160)), id, ok, patched_row(u, id, &before, &r));
                        if !ok {
                            s.push_str(&format!(",\"why\":{}", json_str(r.as_ref().err().unwrap())));
                        }
                        s.push('}');
                        s
                    });
                    rows.push(row);
                }
                let _ = tx.send(rows);
            })
        };
        if spawned.is_err() {
            return out.err("cannot start the O3 build thread");
        }
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(r) => rows.extend(r),
            Err(_) => rows.push("{\"o3\":\"still building after 120 s: `versions` shows what is installed\"}".into()),
        }
        let _k = crate::zone_alloc::kept();
        PINS.lock().unwrap_or_else(|e| e.into_inner()).push((n, q.as_str().to_string(), "o3", build.as_slice().to_vec()));
        rows.push(format!("{{\"pin\":\"pin{}\",\"tier\":\"o3\",\"note\":\"O3 code stays until a live patch or a pin replaces it\",\"more\":[{},{}]}}", n, json_str(&format!("tier {} o1", q)), json_str(&format!("versions {}", q))));
    }
    out.measured(&format!("tier {} {}", q, level), "owner: pins and rebuilds", &[]);
    for r in rows {
        out.line(&r);
    }
}

// ------------------------------------------------------------------ probes: where each one is

/// `probes`: per probe / group / pin, the code versions it is on -- every slot it rebuilt (the fn
/// itself, inliners rebuilt to call it, pinned inliners), the code there now and the code it
/// replaced. Asks the owner briefly (2 s); None when it is busy.
pub(crate) fn owner_rows(owners: Vec<(super::patches::Owner, String)>) -> Option<Vec<String>> {
    if owners.is_empty() {
        return Some(Vec::new());
    }
    let up = crate::jit::current_unit();
    if up.is_null() {
        return None;
    }
    crate::owner::call_within(crate::owner::Class::Job, 2_000, move || {
        let u = unsafe { &*up };
        let mut rows = Vec::new();
        for (owner, what) in owners {
            for (slot, role, before) in super::patches::owner_slots(owner) {
                let replaced = if before == 0 {
                    "\"none (the slot had no code: lazy)\"".to_string()
                } else {
                    let b = bodies(u, slot);
                    let (len, state, gen) = b.iter().find(|x| x.start == before).map_or((0, "replaced", 0), |x| (x.len, x.state, x.gen));
                    body_json(u, slot, before, len, state, gen)
                };
                rows.push(format!(
                    "{{\"on\":\"{}\",\"what\":{},\"slot\":{},\"role\":\"{}\",\"now\":{},\"replaced\":{}}}",
                    owner.label(),
                    json_str(&short(&what, 120)),
                    slot,
                    role,
                    installed_json(u, slot),
                    replaced
                ));
            }
        }
        rows
    })
}
