//! Variable values at a stop (makepad inspect `locals`): a parked panic, a breakpoint hit or a
//! paused thread. Every JIT code body carries its variable table (relax_backend::dbgloc, in its
//! pc map): per call site, where each named local of each inline level is -- a callee-saved
//! register, a frame slot, a constant -- or that it is held nowhere there (optimised out). The
//! frames are walked by their frame pointers; each frame's callee-saved registers come from the
//! stop's register block (the panic / breakpoint trampoline stores it) and from where the inner
//! frames' prologues saved them, as the unwinder restores them. A value is read only from a
//! place the table names for that call; nothing is recompiled, nothing runs in the program's
//! frames. The value's `{:#?}` comes from the program's own Debug impl (std::relax::probe::__eval
//! instantiated at the local's type, run on the inspect thread in the program's domain, on a
//! copy of the bytes); a type that is not Debug shows its bytes.

use super::{json_str, short};
use crate::jit::{FnKey, Unit};
use relax_backend::dbgloc::{self, Home, VarKind};
use relax_backend::rir::{pos_ctx, POS_CTX_LOST};

/// A stopped thread's JIT frames: (lookup pc, frame pointer), innermost first, and the
/// callee-saved register block at the innermost one (jit.rs REG_WORDS layout; None: not known).
pub(crate) struct Stop {
    pub frames: Vec<(u64, u64)>,
    pub regs: Vec<Option<u64>>,
    /// the innermost frame is stopped at a call (its pc is a return address minus one)
    pub top_at_call: bool,
}

fn rd(a: u64) -> Option<u64> {
    let b = super::threads::read_mem(a, 8)?;
    Some(u64::from_le_bytes(b.try_into().ok()?))
}

/// The frames above a JIT frame stopped at a call: its (frame pointer, return address), then the
/// frame records' chain while the return addresses are JIT code.
fn walk(u: &Unit, mut fp: u64, mut ret: u64, top_at_call: bool) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut first = true;
    for _ in 0..256 {
        let pc = if first && !top_at_call { ret } else { ret.wrapping_sub(1) };
        first = false;
        if fp == 0 || fp & 7 != 0 || u.find_fn(pc).is_none() {
            break;
        }
        out.push((pc, fp));
        let (Some(next), Some(r)) = (rd(fp), rd(fp + 8)) else { break };
        if next <= fp {
            break;
        }
        fp = next;
        ret = r & 0x0000_ffff_ffff_ffff;
    }
    out
}

impl Stop {
    /// From a panic / breakpoint trampoline's register block (fp, return address, callee-saved).
    pub fn from_block(u: &Unit, block: &[u64]) -> Option<Stop> {
        if block.len() < crate::jit::REG_WORDS {
            return None;
        }
        let frames = walk(u, block[0], block[1], true);
        Some(Stop { frames, regs: block.iter().map(|&w| Some(w)).collect(), top_at_call: true })
    }

    /// From a thread's registers at an arbitrary point (a pause): `pc`, `fp`, and the callee-saved
    /// integer registers in block order when the stop is in JIT code (host code may hold its own
    /// values in them: then they are not known).
    pub fn from_thread(u: &Unit, pc: u64, fp: u64, ints: &[(usize, u64)]) -> Stop {
        let mut regs = vec![None; crate::jit::REG_WORDS];
        let in_jit = u.find_fn(pc).is_some();
        if in_jit {
            for &(k, v) in ints {
                if k < regs.len() {
                    regs[k] = Some(v);
                }
            }
            let frames = walk(u, fp, pc, false);
            return Stop { frames, regs, top_at_call: false };
        }
        // a stop in host code: the JIT frames are those the frame records reach
        let mut f = fp;
        for _ in 0..64 {
            let (Some(next), Some(r)) = (rd(f), rd(f + 8)) else { break };
            let r = r & 0x0000_ffff_ffff_ffff;
            if u.find_fn(r.wrapping_sub(1)).is_some() {
                return Stop { frames: walk(u, next, r, true), regs, top_at_call: true };
            }
            if next <= f {
                break;
            }
            f = next;
        }
        Stop { frames: Vec::new(), regs, top_at_call: true }
    }

    /// Each frame's callee-saved registers (as far as known) at its call: the block's at the
    /// innermost, then what each frame's prologue saved below its frame pointer.
    pub fn frame_regs(&self, u: &Unit) -> Vec<Vec<Option<u64>>> {
        let mut r = self.regs.clone();
        let mut out = Vec::with_capacity(self.frames.len());
        for &(pc, fp) in &self.frames {
            out.push(r.clone());
            // the caller-saved registers are the innermost call's only
            for k in 0..32 {
                if let Some(i) = crate::jit::call_reg_slot(k) {
                    if let Some(x) = r.get_mut(i) {
                        *x = None;
                    }
                }
            }
            match u.saved_regs_at(pc) {
                Some((_, saved)) => {
                    for (k, reg) in saved.iter().enumerate() {
                        if let Some(i) = crate::jit::reg_slot(*reg) {
                            r[i] = rd(fp - 8 * (k as u64 + 1));
                        }
                    }
                }
                // a frame we cannot unwind: nothing further up is known in registers
                None => r.iter_mut().for_each(|x| *x = None),
            }
        }
        out
    }
}

/// A local's value as found at the stop.
pub(crate) enum Got {
    /// its bytes (the type's size)
    Bytes(Vec<u8>, String),
    /// why it has none
    No(String),
}

/// One local of one inline level of a frame.
pub(crate) struct Local {
    pub name: String,
    pub ty: String,
    pub t: Option<crate::types::TyId>,
    pub got: Got,
}

/// One inline level of a frame: its fn, where it is, its locals.
pub(crate) struct Level {
    pub level: usize,
    pub name: String,
    pub at: String,
    pub inlined_into: Option<String>,
    pub note: Option<String>,
    pub locals: Vec<Local>,
}

/// The variable table of the code body holding `pc` and the body's start and pc map entries.
fn table_at(u: &Unit, pc: u64) -> Option<(u64, Vec<(u32, u64)>, dbgloc::Table)> {
    let id = u.find_fn(pc)?;
    let b = super::codever::bodies(u, id).into_iter().find(|b| pc >= b.start && pc < b.start + b.len as u64)?;
    let entries = b.map.entries();
    let bytes = dbgloc::from_pc_entries(entries.iter().copied());
    let t = if bytes.is_empty() { dbgloc::Table::default() } else { dbgloc::decode(&bytes)? };
    Some((b.start, entries, t))
}

fn reg_word(regs: &[Option<u64>], h: Home) -> Option<u64> {
    let i = match h {
        Home::Reg(r) => crate::jit::reg_slot(r as u32)?,
        Home::Flt(x) => crate::jit::reg_slot(0x100 | x as u32)?,
        Home::CallReg(r) => crate::jit::call_reg_slot(r as u32)?,
        _ => return None,
    };
    regs.get(i).copied().flatten()
}

fn home_name(h: Home) -> String {
    match h {
        Home::Reg(r) => format!("{}{}", if cfg!(target_arch = "aarch64") { "x" } else { "r" }, r),
        Home::Flt(x) => format!("d{}", x),
        Home::Fp(d) => format!("fp{:+}", d),
        Home::Const(_) => "constant".into(),
        Home::CallReg(r) => format!("{}{} (as the call began)", if cfg!(target_arch = "aarch64") { "x" } else { "r" }, r),
    }
}

/// The 8 bytes of a value's home (register, frame memory, constant).
fn home_word(regs: &[Option<u64>], fp: u64, h: Home) -> Option<u64> {
    match h {
        Home::Reg(_) | Home::Flt(_) | Home::CallReg(_) => reg_word(regs, h),
        Home::Fp(d) => rd(fp.wrapping_add(d as i64 as u64)),
        Home::Const(k) => Some(k as u64),
    }
}

/// def path lookup in a crate (`["fmt", "Debug", "fmt"]` in core), types first as rustc's lang
/// paths are walked (typeck lang_item).
fn def_in(u: &Unit, krate: &str, path: &[&str]) -> Option<crate::program::DefId> {
    let prog = &u.prog;
    let k = prog.syms.get(krate)?;
    let c = prog.crates.iter().find(|c| c.name == k)?;
    let mut cur = prog.mods[c.root_mod as usize].def;
    for p in path {
        let s = prog.syms.get(p)?;
        cur = prog.lookup_in_container(cur, s, true).or_else(|| prog.lookup_in_container(cur, s, false))?;
    }
    Some(cur)
}

/// The instance of `d` whose copy is inlined in the body at `start` of slot `id`: the one
/// instance among what that code took in (O3: its inlined slots; O1: the slot's body_deps), else
/// the one instance of `d` in the program. None: several (a generic fn's copies are not told
/// apart by the line table).
fn instance_of(u: &Unit, id: u32, start: u64, d: crate::program::DefId) -> Result<FnKey, String> {
    let mut cands: Vec<u32> = u.code.read().unwrap_or_else(|e| e.into_inner()).inlined_at(start).map(|s| s.to_vec()).unwrap_or_default();
    let mut work = vec![id];
    let mut seen = std::collections::HashSet::new();
    while let Some(s) = work.pop() {
        if !seen.insert(s) || seen.len() > 4096 {
            continue;
        }
        if let Some(dd) = u.body_deps.get(&s) {
            for &c in dd {
                cands.push(c);
                work.push(c);
            }
        }
    }
    let pick = |it: &mut dyn Iterator<Item = &FnKey>| -> Option<Result<FnKey, String>> {
        let mut found: Option<FnKey> = None;
        for k in it {
            if let FnKey::Inst(x, _) = k {
                if *x == d {
                    match &found {
                        Some(f) if f != k => return Some(Err("a generic fn inlined here: its instance (and so its locals' types) is not known from the code".into())),
                        _ => found = Some(k.clone()),
                    }
                }
            }
        }
        found.map(Ok)
    };
    if let Some(r) = pick(&mut cands.iter().map(|&c| &u.fns[c as usize].key)) {
        return r;
    }
    if let Some(r) = pick(&mut u.fns.iter().map(|f| &f.key)) {
        return r;
    }
    // no slot of its own (it only runs inlined, e.g. in code from the disk cache): a fn without
    // type or const parameters has one instance
    if u.tcx.generic_indices_of(&u.prog, d).types_consts.is_empty() {
        return Ok(FnKey::Inst(d, Vec::new()));
    }
    Err("the inlined generic fn has no instance here: its locals' types are not known".into())
}

/// Every inline level of every frame of `stop` (`only`: that frame), innermost first, with each
/// local's bytes as the variable table places them. On the owner (reads the code tables, the
/// checked bodies and the stopped thread's stack).
pub(crate) fn levels(u: &Unit, stop: &Stop, only: Option<usize>) -> Vec<(usize, Vec<Level>)> {
    let regs = stop.frame_regs(u);
    let mut res = super::codever::Resolver::new();
    let mut out = Vec::new();
    for (k, &(pc, fp)) in stop.frames.iter().enumerate() {
        if only.is_some_and(|o| o != k) {
            continue;
        }
        let at_call = k > 0 || stop.top_at_call;
        let Some(id) = u.find_fn(pc) else { continue };
        let Some((start, entries, table)) = table_at(u, pc) else { continue };
        let off = (pc - start) as u32;
        let ret_off = off + 1;
        // the pc's position and its inline contexts, innermost first (ctx 0: the slot's fn)
        let mut pos = None;
        for &(o, p) in &entries {
            if o <= off {
                pos = Some(p);
            } else {
                break;
            }
        }
        let inl: Vec<u64> = entries.iter().filter(|e| e.0 == u32::MAX).map(|e| e.1).collect();
        let mut chain: Vec<(u64, u64)> = Vec::new(); // (ctx, position in that level)
        let mut cur = pos.unwrap_or(0);
        let mut lost = false;
        for _ in 0..256 {
            let c = pos_ctx(cur);
            chain.push((c, cur));
            if c == 0 {
                break;
            }
            if c == POS_CTX_LOST || (c as usize) * 2 > inl.len() {
                lost = true;
                break;
            }
            cur = inl[(c as usize - 1) * 2];
        }
        let site = if at_call { table.site(ret_off) } else { None };
        let slot_name = super::display_name(u, id, 0);
        let mut levels = Vec::new();
        for (lv, &(c, p)) in chain.iter().enumerate() {
            let at = u.pos_str(relax_backend::rir::pos_file(p), p as u32);
            // the level's fn instance
            let key: Result<FnKey, String> = if c == 0 {
                Ok(u.fns[id as usize].key.clone())
            } else if c == POS_CTX_LOST {
                Err("an inline chain deeper than the line table holds".into())
            } else {
                let anchor = inl.get((c as usize - 1) * 2 + 1).copied().unwrap_or(0);
                match res.fn_at(u, anchor) {
                    Some(d) => instance_of(u, id, start, d),
                    None => Err("the inlined fn is not identified".into()),
                }
            };
            let name = match &key {
                Ok(FnKey::Inst(d, _)) => crate::lower::display_path(&u.prog, *d),
                _ if c == 0 => slot_name.clone(),
                _ => "(inlined fn)".into(),
            };
            let mut lvl = Level { level: lv, name, at, inlined_into: (c != 0).then(|| slot_name.clone()), note: None, locals: Vec::new() };
            let (d, args) = match key {
                Ok(FnKey::Inst(d, args)) => (d, args),
                Ok(_) => {
                    lvl.note = Some("not a fn instance (a closure or glue): its locals are not listed".into());
                    levels.push(lvl);
                    continue;
                }
                Err(e) => {
                    lvl.note = Some(e);
                    levels.push(lvl);
                    continue;
                }
            };
            let body = crate::typeck::instantiate_body(&crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()), &u.prog, &u.tcx, d, &args);
            let vars: Vec<(usize, &dbgloc::Var)> = table.vars.iter().enumerate().filter(|(_, v)| v.ctx as u64 == c).collect();
            if table.vars.is_empty() {
                lvl.note = Some("this code keeps no variable locations (built without them: MAKEPAD_RUN_NO_VARS, or before they were recorded)".into());
            } else if !at_call {
                lvl.note = Some("the innermost frame is not stopped at a call: values in registers are not known here (frame slots are)".into());
            }
            if lost && lv + 1 == chain.len() {
                lvl.note = Some("the inline chain was cut (deeper than the line table holds): outer levels are not listed".into());
            }
            for (li, l) in body.locals.iter().enumerate() {
                let n = u.prog.syms.str(l.name);
                if n.is_empty() || n.starts_with("__") {
                    continue;
                }
                let t = u.tcx.tys.erase_regions(u.tcx.tys.subst(l.ty, &args));
                let ty = crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t);
                let got = match vars.iter().find(|(_, v)| v.local == li as u32) {
                    None if table.vars.is_empty() => Got::No("no variable table".into()),
                    None => Got::No("not kept (no storage at this point: optimised out, or not yet bound)".into()),
                    Some(&(vi, v)) => read_var(u, t, vi as u32, v, site, at_call, &regs[k], fp),
                };
                lvl.locals.push(Local { name: n.to_string(), ty, t: Some(t), got });
            }
            levels.push(lvl);
        }
        out.push((k, levels));
    }
    out
}

/// A variable's bytes at a frame (`site`: the call's located leaves).
#[allow(clippy::too_many_arguments)]
fn read_var(u: &Unit, t: crate::types::TyId, vi: u32, v: &dbgloc::Var, site: Option<&[(u32, u32, Home)]>, at_call: bool, regs: &[Option<u64>], fp: u64) -> Got {
    let l = u.layout_of(t);
    let size = l.size as usize;
    if size > 1 << 20 {
        return Got::No(format!("{} bytes: too big to show", size));
    }
    // a leaf's home at this call; a constant leaf is one everywhere
    let leaf_home = |k: u32| site.and_then(|s| s.iter().find(|x| x.0 == vi && x.1 == k)).map(|x| x.2).or_else(|| v.consts.iter().find(|c| c.0 == k).map(|c| Home::Const(c.1)));
    match v.kind {
        VarKind::Frame(d) => match super::threads::read_mem(fp.wrapping_add(d as i64 as u64), size) {
            Some(b) => Got::Bytes(b, format!("frame slot fp{:+}", d)),
            None => Got::No("its frame slot is not readable".into()),
        },
        VarKind::Shared => Got::No("its stack slot shares storage with other values (lifetimes that do not overlap): not known to hold it here".into()),
        VarKind::Mem(off) => {
            if !at_call {
                return Got::No("not at a call: its address is not known here".into());
            }
            let Some(a) = leaf_home(0).and_then(|h| home_word(regs, fp, h)) else { return Got::No("optimised out (its address is held nowhere at this call)".into()) };
            match super::threads::read_mem(a.wrapping_add(off as i64 as u64), size) {
                Some(b) => Got::Bytes(b, format!("memory at {:#x}", a.wrapping_add(off as i64 as u64))),
                None => Got::No(format!("its memory at {:#x} is not readable", a)),
            }
        }
        VarKind::Regs(n) => {
            if !at_call && v.consts.len() < n as usize {
                return Got::No("not at a call: register values are not known here".into());
            }
            let Some(leaves) = &l.leaves else { return Got::No("its layout has no leaves".into()) };
            if leaves.len() != n as usize {
                return Got::No("its layout differs from the code's".into());
            }
            let mut b = vec![0u8; size];
            let mut wher = Vec::new();
            for (k, lf) in leaves.iter().enumerate() {
                let Some(h) = leaf_home(k as u32) else {
                    return Got::No(if n > 1 { format!("optimised out (part {} of {} is held nowhere at this call)", k + 1, n) } else { "optimised out (held nowhere at this call)".into() });
                };
                let Some(w) = home_word(regs, fp, h) else {
                    return Got::No(format!("in {} at this call, whose value is not known (no frame of the stop saved it)", home_name(h)));
                };
                wher.push(home_name(h));
                let sz = relax_backend::passes::mem_size(lf.mem) as usize;
                let o = lf.off as usize;
                if o + sz <= b.len() {
                    b[o..o + sz].copy_from_slice(&w.to_le_bytes()[..sz.min(8)]);
                }
            }
            Got::Bytes(b, wher.join(", "))
        }
    }
}

/// Debug formatting: `std::relax::probe::__eval::<T>` (T: Debug) entries, compiled on the owner.
pub(crate) fn debug_entry(u: &Unit, t: crate::types::TyId) -> Option<u64> {
    let fmt = def_in(u, "core", &["fmt", "Debug", "fmt"])?;
    let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
    u.tcx.resolve_instance_raw(&u.prog, env, fmt, &[t]).ok()?;
    let ev = def_in(u, "std", &["relax", "probe", "__eval"])?;
    let slot = u.fn_id(FnKey::Inst(ev, vec![t]));
    Some(u.entry(slot))
}

/// The JSON of one local: name, type, then its value (`{:#?}` of a copy, or its bytes) or why
/// there is none.
pub(crate) fn local_json(l: &Local, shown: Option<&Result<String, String>>) -> String {
    let head = format!("{{\"name\":{},\"type\":{}", json_str(&l.name), json_str(&short(&l.ty, 160)));
    match &l.got {
        Got::No(why) => format!("{},\"value\":null,\"why\":{}}}", head, json_str(why)),
        Got::Bytes(b, from) => {
            let hex: String = b.iter().take(64).map(|x| format!("{:02x}", x)).collect::<Vec<_>>().join(" ");
            match shown {
                Some(Ok(v)) => format!("{},\"value\":{},\"from\":{}}}", head, json_str(&short(v, 4000)), json_str(from)),
                Some(Err(e)) => format!("{},\"value\":null,\"bytes\":{},\"from\":{},\"why\":{}}}", head, json_str(&hex), json_str(from), json_str(&short(e, 300))),
                None => format!("{},\"value\":null,\"bytes\":{},\"from\":{},\"why\":\"the type is not Debug\"}}", head, json_str(&hex), json_str(from)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::jit::PcMap;
    use relax_backend::dbgloc::{self, Home, Table, Var, VarKind};

    #[test]
    fn variable_table_round_trips_through_the_pc_map_bytes() {
        let t = Table {
            vars: vec![Var { ctx: 0, local: 2, kind: VarKind::Regs(1), consts: Vec::new() }, Var { ctx: 1, local: 0, kind: VarKind::Frame(-16), consts: vec![] }],
            sites: vec![(20, vec![(0, 0, Home::Reg(19))]), (48, vec![(0, 0, Home::CallReg(3))])],
        };
        let bytes = dbgloc::encode(&t);
        // code entries, the table, the inline stack's entries
        let mut v: Vec<(u32, u64)> = vec![(0, 5 << 32 | 10), (8, 5 << 32 | 14), (16, 1 << 48 | 5 << 32 | 20)];
        v.extend(dbgloc::pc_entries(&bytes, 5));
        v.push((u32::MAX, 5 << 32 | 3));
        v.push((u32::MAX, 5 << 32 | 9));
        let m = PcMap::new(&v);
        let back = m.entries();
        assert_eq!(dbgloc::from_pc_entries(back.iter().copied()), bytes);
        assert_eq!(dbgloc::decode(&dbgloc::from_pc_entries(back.iter().copied())), Some(t));
        // the code entries and the inline stack come back as they were; a lookup never reaches the table
        assert_eq!(&back[..3], &v[..3]);
        assert_eq!(&back[back.len() - 2..], &v[v.len() - 2..]);
        assert_eq!(m.lookup(17), Some(1 << 48 | 5 << 32 | 20));
        // stored compactly: the table's bytes as they are after one length entry
        let without: Vec<(u32, u64)> = v.iter().copied().filter(|e| e.0 != dbgloc::PC_VARS).collect();
        let extra = m.bytes() - PcMap::new(&without).bytes();
        assert!(extra <= bytes.len() + 12, "{} more bytes for a {}-byte table", extra, bytes.len());
    }
}
